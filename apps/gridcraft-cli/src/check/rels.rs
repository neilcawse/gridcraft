//! The relationships a package keeps, grouped by the chain of parts that owns them.
//!
//! A round trip is compared owner by owner: a sheet with the sheet of the same name, its drawing
//! with the drawing it came back as. Losing one of four drawings is as visible as losing all of
//! them, which a count of kinds over the whole package can't show.

use std::collections::VecDeque;

use super::package::{
    MAX_ENTRIES, PartError, Zip, attr, holds_something, part_key, read_zip_part, read_zip_part_within, resolve_target, xml_elements, zip_names,
};
use super::{BTreeMap, BTreeSet, Cursor, HashMap, HashSet};

/// Relationship kinds whose absence after a save isn't a loss, in the folded spelling of
/// [`fold_kind`]: the calculation chain is a cache Excel rebuilds; chart style / user-shape /
/// theme-override sidecars are spelled out inline; pivot cache records are rebuilt with the
/// cache; the shared strings part is rebuilt from the cells (and absent when no string remains);
/// the workbook metadata that holds only dynamic-array flags is rebuilt from the formulas it
/// flagged.
pub(super) const REBUILT_KINDS: [&str; 9] = [
    "calcchain",
    "chartcolorstyle",
    "chartstyle",
    "chartusershapes",
    "pivotcacherecords",
    "sharedstrings",
    "sheetmetadata",
    "themeoverride",
    "workbookmetadata",
];

/// Kinds whose number per owner is compared. Each relationship of these kinds is one part that
/// GridCraft writes once per object (a chart, a table, a pivot table, a sheet that isn't a
/// worksheet), so fewer after a save means an object was dropped. Every other kind only has to be
/// present at its owner: images and hyperlinks are shared when equal, pivot caches are merged,
/// and the numbers differ without anything being lost. Worksheets are compared cell by cell.
const COUNTED_KINDS: [&str; 7] = ["chart", "chartsheet", "dialogsheet", "pivottable", "table", "xlintlmacrosheet", "xlmacrosheet"];

/// What the relationship check of one package may spend before it gives up. A check that gave up
/// has listed only the relationships it reached: what it lists can't be compared, as a
/// relationship it never reached is neither kept nor lost. It says which limit it stopped at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Limits {
    /// The deepest chain of owners followed from the package root.
    pub depth: usize,
    /// Most relationships looked at in one package, over all owners.
    pub relationships: usize,
    /// Most entries in the zip package.
    pub entries: usize,
    /// Most owners followed (a part reached through two chains of owners is two).
    pub owners: usize,
    /// Decompressed bytes that comparing one package's relationships may read in total: the
    /// relationship parts themselves and the targets whose content is checked.
    pub bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { depth: 12, relationships: 5_000_000, entries: MAX_ENTRIES, owners: MAX_ENTRIES, bytes: 512 * 1024 * 1024 }
    }
}

/// Owners, by the folded kind that leads to them, whose every relationship is explicit: the
/// DrawingML parts of a drawing and a chart name each part they use by its relationship id
/// (`r:embed`, `r:link`, `r:id`). A relationship of theirs that their XML never names is
/// nothing any reader shows (ECMA-376 Part 1, §9.2: an explicit relationship is used through the
/// id the part's markup references), so a save may drop it.
const EXPLICIT_OWNERS: [&str; 2] = ["drawing", "chart"];

/// Every attribute value in the XML part `name`, or `None` when it can't be read as XML (then
/// every relationship of it counts).
fn attribute_values(zip: &mut Zip<'_>, name: &str) -> Option<HashSet<String>> {
    let els = read_zip_part(zip, name).and_then(|xml| xml_elements(&xml)).ok()?;
    Some(els.into_iter().flat_map(|(_, attrs)| attrs.into_iter().map(|(_, v)| v)).collect())
}

/// The relationship kinds of the parts a workbook lists as its sheets.
const SHEET_KINDS: [&str; 5] = ["worksheet", "chartsheet", "dialogsheet", "xlmacrosheet", "xlintlmacrosheet"];

/// A relationship kind with case and the `-`/`_` separators dropped, so that the Strict OOXML
/// spelling of a type (`extendedProperties`, `customProperties`) and the transitional one
/// (`extended-properties`, `custom-properties`) name the same kind.
pub(super) fn fold_kind(kind: &str) -> String {
    kind.chars().filter(|c| !matches!(c, '-' | '_')).flat_map(char::to_lowercase).collect()
}

/// The relationships of a package, by owner.
///
/// An owner is a chain of segments from the package root: `[]` is the package itself,
/// `[officedocument]` the workbook part, `[officedocument, sheet "Data", drawing]` the drawing of
/// the sheet named Data. A segment is the folded kind of the relationship that leads to the part
/// (or `sheet "name"` for the parts the workbook lists as sheets); parts of one kind under one
/// owner share a segment, so their relationships are added up.
#[derive(Debug, Default)]
pub(super) struct RelTree {
    /// Owner → folded kind → number of relationships of that kind.
    owners: BTreeMap<Vec<String>, BTreeMap<String, usize>>,
    /// Every owner some relationship leads to, whether or not it has relationships itself.
    reached: BTreeSet<Vec<String>>,
    /// Folded kind → the kind as first written.
    labels: HashMap<String, String>,
}

/// One relationship of a rels part.
struct Rel {
    id: String,
    /// Last segment of the type URI as written.
    kind: String,
    target: String,
    external: bool,
}

/// A part still to follow: its owner, its name, and the parts it was reached through.
struct Pending {
    path: Vec<String>,
    source: String,
    chain: Vec<String>,
}

/// The key of the rels part of `source` (the package for ""), e.g. `xl/_rels/workbook.xml.rels`.
fn rels_part_key(source: &str) -> String {
    match source.rsplit_once('/') {
        Some((dir, file)) => format!("{dir}/_rels/{file}.rels"),
        None if source.is_empty() => "_rels/.rels".to_string(),
        None => format!("_rels/{source}.rels"),
    }
}

/// The directory of a part, empty for the package.
fn part_dir(source: &str) -> &str {
    source.rsplit_once('/').map_or("", |(dir, _)| dir)
}

fn rels_of(zip: &mut Zip<'_>, name: &str) -> Vec<Rel> {
    read_zip_part(zip, name).map(|xml| rels_in(&xml)).unwrap_or_default()
}

/// The relationships a rels part's XML lists.
fn rels_in(xml: &[u8]) -> Vec<Rel> {
    let Ok(els) = xml_elements(xml) else { return Vec::new() };
    els.iter()
        .filter(|(el, _)| el == "Relationship")
        .filter_map(|(_, attrs)| {
            let kind = attr(attrs, "Type").and_then(|t| t.rsplit('/').next())?;
            let id = attr(attrs, "Id").unwrap_or("");
            let target = attr(attrs, "Target").unwrap_or("");
            if id.is_empty() || target.is_empty() {
                return None;
            }
            let external = attr(attrs, "TargetMode").is_some_and(|m| m.eq_ignore_ascii_case("External"));
            Some(Rel { id: id.to_string(), kind: kind.to_string(), target: target.to_string(), external })
        })
        .collect()
}

/// `relationship id → sheet name` of the sheets the workbook part `name` lists.
fn sheet_names(zip: &mut Zip<'_>, name: &str) -> HashMap<String, String> {
    let Ok(els) = read_zip_part(zip, name).and_then(|xml| xml_elements(&xml)) else { return HashMap::new() };
    els.iter()
        .filter(|(el, _)| el == "sheet")
        .filter_map(|(_, attrs)| Some((attr(attrs, "id")?.to_string(), attr(attrs, "name")?.to_string())))
        .collect()
}

/// `(sheet name, part name)` of every sheet the workbook part `workbook` lists whose relationship
/// leads to a part of the package.
pub(super) fn sheet_parts(zip: &mut Zip<'_>, names: &[String], workbook: &str) -> Vec<(String, String)> {
    let ids = sheet_names(zip, workbook);
    let key = part_key(workbook);
    let rels_key = rels_part_key(&key);
    let Some(rels_name) = names.iter().find(|n| part_key(n) == rels_key) else { return Vec::new() };
    let dir = part_dir(&key).to_string();
    let by_key: HashMap<String, &String> = names.iter().map(|n| (part_key(n), n)).collect();
    rels_of(zip, rels_name)
        .into_iter()
        .filter(|r| !r.external)
        .filter_map(|r| {
            let sheet = ids.get(&r.id)?;
            let part = by_key.get(&resolve_target(&dir, &r.target).to_ascii_lowercase())?;
            Some((sheet.clone(), (*part).clone()))
        })
        .collect()
}

/// The part of the package that `source` reaches by its first internal relationship of `kind`
/// (compared by [`fold_kind`], so Strict and Transitional spellings are one kind).
pub(super) fn related_part(zip: &mut Zip<'_>, names: &[String], source: &str, kind: &str) -> Option<String> {
    let key = part_key(source);
    let rels_key = rels_part_key(&key);
    let rels_name = names.iter().find(|n| part_key(n) == rels_key)?;
    let wanted = fold_kind(kind);
    let rel = rels_of(zip, rels_name).into_iter().find(|r| !r.external && fold_kind(&r.kind) == wanted)?;
    let target = resolve_target(part_dir(&key), &rel.target).to_ascii_lowercase();
    names.iter().find(|n| part_key(n) == target).cloned()
}

/// The message of a check that gave up at `limit`.
fn stopped(limit: impl std::fmt::Display) -> String {
    format!("relationship check stopped at its {limit}")
}

/// The message of a check that gave up because reading `part` would have spent more than the
/// decompression it may cause.
fn over_budget(part: &str) -> String {
    format!("{part}: {}", stopped("decompression limit"))
}

/// Groups the relationships of the package by owner, following them from the package root, so the
/// relationships of parts nothing leads to (orphans, which Excel drops on save too) are not
/// counted. Relationships that carry nothing are left out, as a save may rightly drop them: a
/// link to a place in the workbook (`#Sheet1!A1`, written as a cell's `location`), a target
/// part with nothing in it or that the package doesn't hold, a relationship of an
/// [`EXPLICIT_OWNERS`] part that its XML never names, and a pivot cache of the workbook that no
/// pivot table reached from it uses (Excel deletes a cache with the last pivot table using it).
///
/// `Err` when a limit of `limits` stopped the check before every relationship was listed (the
/// message says which), never a tree that stops short: comparing one would take what wasn't
/// reached for kept or for lost.
pub(super) fn relationship_tree(bytes: &[u8], limits: Limits) -> Result<RelTree, String> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| format!("not a zip package: {e}"))?;
    if zip.len() > limits.entries {
        return Err(stopped(format_args!("limit of {} package entries", limits.entries)));
    }
    let names = zip_names(&mut zip);
    let by_key: HashMap<String, &String> = names.iter().map(|n| (part_key(n), n)).collect();
    // Several relationships name the same part; each fresh check decompressed it again, so
    // repeated references to one highly compressible part multiplied the work. The verdict is
    // remembered per (part, named content) and the decompression the package can cause is
    // budgeted as a whole, the relationship parts included: many large rels parts would
    // otherwise each spend a full per-part allowance.
    let mut budget = limits.bytes;
    let mut verdicts: HashMap<(String, &'static str), bool> = HashMap::new();
    let mut work = limits.relationships;
    let mut parsed: HashMap<String, Vec<Rel>> = HashMap::new();
    let mut tree = RelTree::default();
    let mut expanded: HashSet<(Vec<String>, String)> = HashSet::new();
    // Workbook pivot caches (owner, target), counted at the end when a pivot table uses them.
    let mut workbook_caches: Vec<(Vec<String>, String)> = Vec::new();
    let mut used_caches: HashSet<String> = HashSet::new();
    let mut queue = VecDeque::from([Pending { path: Vec::new(), source: String::new(), chain: Vec::new() }]);
    while let Some(Pending { path, source, mut chain }) = queue.pop_front() {
        let rels_key = rels_part_key(&source);
        // A part with no relationships has nothing to follow, however deep it is.
        let Some(rels_name) = by_key.get(&rels_key) else { continue };
        if !expanded.insert((path.clone(), source.clone())) {
            continue;
        }
        if path.len() > limits.depth {
            return Err(stopped(format_args!("limit of {} owners deep", limits.depth)));
        }
        if expanded.len() > limits.owners {
            return Err(stopped(format_args!("limit of {} owners", limits.owners)));
        }
        if !parsed.contains_key(&rels_key) {
            let rels = match read_zip_part_within(&mut zip, rels_name, &mut budget) {
                Ok(xml) => rels_in(&xml),
                Err(PartError::OverBudget) => return Err(over_budget(rels_name)),
                Err(PartError::Unreadable) => Vec::new(),
            };
            parsed.insert(rels_key.clone(), rels);
        }
        let rels = parsed.get(&rels_key).map(Vec::as_slice).unwrap_or_default();
        let Some(left) = work.checked_sub(rels.len()) else {
            return Err(stopped(format_args!("limit of {} relationships", limits.relationships)));
        };
        work = left;
        let in_workbook = path.len() == 1 && path.first().is_some_and(|s| s == "officedocument");
        let sheets = match by_key.get(&source) {
            Some(workbook) if in_workbook => sheet_names(&mut zip, workbook),
            _ => HashMap::new(),
        };
        let src_dir = part_dir(&source).to_string();
        let explicit_only = path.last().is_some_and(|seg| EXPLICIT_OWNERS.contains(&seg.as_str()));
        let referenced = match by_key.get(&source) {
            Some(part) if explicit_only => attribute_values(&mut zip, part),
            _ => None,
        };
        let from_pivot_table = path.last().is_some_and(|seg| seg == "pivottable");
        chain.push(source);
        for rel in rels {
            if rel.kind.eq_ignore_ascii_case("hyperlink") && rel.target.starts_with('#') {
                continue;
            }
            if referenced.as_ref().is_some_and(|ids| !ids.contains(&rel.id)) {
                continue;
            }
            // A sheet relationship the workbook doesn't list in `<sheets>` is no sheet of it.
            if in_workbook && SHEET_KINDS.contains(&fold_kind(&rel.kind).as_str()) && !sheets.contains_key(&rel.id) {
                continue;
            }
            let resolved = (!rel.external).then(|| resolve_target(&src_dir, &rel.target).to_ascii_lowercase());
            let part = resolved.as_ref().and_then(|r| by_key.get(r));
            // An internal target that isn't in the package holds nothing, as an empty part doesn't.
            if resolved.is_some() {
                let Some(part) = part else { continue };
                match holds_something(&mut zip, part, &rel.kind, &mut verdicts, &mut budget) {
                    Some(true) => {}
                    Some(false) => continue,
                    None => return Err(over_budget(part)),
                }
            }
            let key = fold_kind(&rel.kind);
            tree.labels.entry(key.clone()).or_insert_with(|| rel.kind.clone());
            let cache = key == "pivotcachedefinition";
            if cache
                && from_pivot_table
                && let Some(resolved) = &resolved
            {
                used_caches.insert(resolved.clone());
            }
            match &resolved {
                Some(resolved) if cache && in_workbook => workbook_caches.push((path.clone(), resolved.clone())),
                _ => *tree.owners.entry(path.clone()).or_default().entry(key.clone()).or_default() += 1,
            }
            if let Some(resolved) = resolved
                && part.is_some()
            {
                let segment = match sheets.get(&rel.id) {
                    Some(name) => format!("sheet \"{name}\""),
                    None => key,
                };
                let mut child = path.clone();
                child.push(segment);
                tree.reached.insert(child.clone());
                if !chain.contains(&resolved) {
                    queue.push_back(Pending { path: child, source: resolved, chain: chain.clone() });
                }
            }
        }
    }
    for (owner, target) in workbook_caches {
        if used_caches.contains(&target) {
            *tree.owners.entry(owner).or_default().entry("pivotcachedefinition".to_string()).or_default() += 1;
        }
    }
    Ok(tree)
}

impl RelTree {
    /// `sheet "Data" > drawing`: the owner as a reader names it; `workbook` for the workbook part
    /// and nothing for the package itself.
    fn describe(&self, path: &[String]) -> String {
        let shown: Vec<&str> = path
            .iter()
            .map(|seg| match seg.as_str() {
                "officedocument" => "workbook",
                s => self.labels.get(s).map_or(s, String::as_str),
            })
            .collect();
        match shown.as_slice() {
            ["workbook", rest @ ..] if !rest.is_empty() => rest.join(" > "),
            all => all.join(" > "),
        }
    }
}

/// What `written` lost of the relationships in `source`, one line per owner and kind. A kind is
/// lost when it was present at an owner and isn't any more (for [`COUNTED_KINDS`], when there
/// are fewer). Owners that did not survive are not listed again: the relationship that led to
/// them is the loss.
pub(super) fn lost_relationships(source: &RelTree, written: &RelTree) -> Vec<String> {
    let mut out = Vec::new();
    for (path, kinds) in &source.owners {
        if !path.is_empty() && !written.reached.contains(path) {
            continue;
        }
        let kept = written.owners.get(path);
        for (key, &had) in kinds {
            if REBUILT_KINDS.contains(&key.as_str()) {
                continue;
            }
            let have = kept.and_then(|k| k.get(key)).copied().unwrap_or(0);
            let lost = if COUNTED_KINDS.contains(&key.as_str()) {
                had.saturating_sub(have)
            } else if have == 0 {
                had
            } else {
                0
            };
            if lost == 0 {
                continue;
            }
            let label = source.labels.get(key).map_or(key.as_str(), String::as_str);
            let owner = source.describe(path);
            let at = if owner.is_empty() { String::new() } else { format!(" in {owner}") };
            let how_many = if lost < had { format!(" ({lost} of {had})") } else { String::new() };
            out.push(format!("lost relationship {label}{at}{how_many}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;

    /// A package whose root points at `count` parts, each with a relationship part of `size` bytes
    /// that lists one external relationship.
    fn many_rels(count: usize, size: usize) -> Vec<u8> {
        let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        let open = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#;
        let kind = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/custom";
        let external = format!(r#"<Relationship Id="rId1" Type="{kind}" Target="https://example.com" TargetMode="External"/>"#);
        let tail = "</Relationships>";
        let mut root = String::from(open);
        for i in 0..count {
            root.push_str(&format!(r#"<Relationship Id="rId{i}" Type="{kind}" Target="xl/part{i}.xml"/>"#));
        }
        root.push_str(tail);
        z.start_file("_rels/.rels", opts).unwrap();
        z.write_all(root.as_bytes()).unwrap();
        let filler = " ".repeat(size.saturating_sub(open.len() + external.len() + tail.len()));
        for i in 0..count {
            z.start_file(format!("xl/part{i}.xml"), opts).unwrap();
            z.write_all(b"<part><child/></part>").unwrap();
            z.start_file(format!("xl/_rels/part{i}.xml.rels"), opts).unwrap();
            z.write_all(open.as_bytes()).unwrap();
            z.write_all(external.as_bytes()).unwrap();
            z.write_all(filler.as_bytes()).unwrap();
            z.write_all(tail.as_bytes()).unwrap();
        }
        z.finish().unwrap().into_inner()
    }

    /// The relationships of `tree` of `kind` at `owner` (segments from the package root).
    fn count(tree: &RelTree, owner: &[&str], kind: &str) -> usize {
        let owner: Vec<String> = owner.iter().map(ToString::to_string).collect();
        tree.owners.get(&owner).and_then(|kinds| kinds.get(kind)).copied().unwrap_or(0)
    }

    fn part(name: &str, body: impl Into<String>) -> (String, String) {
        (name.to_string(), body.into())
    }

    /// A rels part listing `(kind, target)` relationships, the kinds under example.com.
    fn listing(rels: &[(&str, &str)]) -> String {
        let list: String = rels
            .iter()
            .enumerate()
            .map(|(i, (kind, target))| format!(r#"<Relationship Id="rId{i}" Type="http://example.com/relationships/{kind}" Target="{target}"/>"#))
            .collect();
        format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{list}</Relationships>"#)
    }

    /// A package whose root points at the workbook and at `targets`, and whose workbook has one
    /// `madeUp` relationship, which the check reads after the targets were looked at.
    fn with_targets(targets: &[(String, String)]) -> Vec<u8> {
        let mut root = vec![("officeDocument", "xl/workbook.xml")];
        root.extend(targets.iter().map(|(name, _)| ("target", name.as_str())));
        let mut parts = vec![
            part("_rels/.rels", listing(&root)),
            part("xl/workbook.xml", "<workbook><sheets/></workbook>"),
            part("xl/_rels/workbook.xml.rels", listing(&[("madeUp", "madeUp.xml")])),
            part("xl/madeUp.xml", "<madeUp><item/></madeUp>"),
        ];
        parts.extend(targets.iter().cloned());
        let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, body) in &parts {
            z.start_file(name, opts).unwrap();
            z.write_all(body.as_bytes()).unwrap();
        }
        z.finish().unwrap().into_inner()
    }

    #[test]
    fn relationship_parts_count_against_the_decompression_budget() {
        // Each part fits the per-part allowance, but together they need more than the budget.
        let bytes = many_rels(8, 64 * 1024);
        let tight = Limits { bytes: 256 * 1024, ..Limits::default() };
        let why = relationship_tree(&bytes, tight).unwrap_err();
        assert!(why.ends_with("relationship check stopped at its decompression limit"), "{why}");
        let tree = relationship_tree(&bytes, Limits { bytes: 1024 * 1024, ..Limits::default() }).unwrap();
        assert_eq!(tree.labels.get("custom").map(String::as_str), Some("custom"));
        assert_eq!(tree.owners.values().filter_map(|kinds| kinds.get("custom")).sum::<usize>(), 16);
    }

    /// A check that reached one of its limits has listed only some of the relationships; it must
    /// say so and name the limit, whichever it was, and never hand the partial list back as the
    /// package's.
    #[test]
    fn a_check_that_reaches_a_limit_says_which() {
        let bytes = many_rels(8, 200);
        let default = Limits::default();
        assert!(relationship_tree(&bytes, default).is_ok());
        for (limits, at) in [
            (Limits { entries: 10, ..default }, "limit of 10 package entries"),
            (Limits { relationships: 10, ..default }, "limit of 10 relationships"),
            (Limits { owners: 5, ..default }, "limit of 5 owners"),
            (Limits { depth: 0, ..default }, "limit of 0 owners deep"),
            (Limits { bytes: 1000, ..default }, "decompression limit"),
        ] {
            let why = relationship_tree(&bytes, limits).unwrap_err();
            assert!(why.ends_with(&format!("relationship check stopped at its {at}")), "{limits:?}: {why}");
        }
    }

    /// A target costs what looking at it reads, not its size: whatever follows the element that
    /// decides what it holds is never read, so padding can't use the budget up before the
    /// relationship parts after it are.
    #[test]
    fn padding_after_the_element_that_decides_cannot_use_up_the_budget() {
        let padding = " ".repeat(1024 * 1024);
        let targets: Vec<_> = (1..=8).map(|i| part(&format!("xl/padded{i}.xml"), format!("<part><child/></part>{padding}"))).collect();
        let tree = relationship_tree(&with_targets(&targets), Limits { bytes: 256 * 1024, ..Limits::default() }).unwrap();
        assert_eq!(count(&tree, &[], "target"), 8);
        assert_eq!(count(&tree, &["officedocument"], "madeup"), 1);
    }

    /// What lies past the budget is unknown. It is not an empty part, which would be dropped from
    /// the relationships as holding nothing.
    #[test]
    fn a_target_whose_content_is_past_the_budget_is_unknown_not_empty() {
        let late = part("xl/late.xml", format!("{}<part><child/></part>", " ".repeat(100 * 1024)));
        let bytes = with_targets(&[late]);
        let why = relationship_tree(&bytes, Limits { bytes: 64 * 1024, ..Limits::default() }).unwrap_err();
        assert_eq!(why, "xl/late.xml: relationship check stopped at its decompression limit");
        let tree = relationship_tree(&bytes, Limits { bytes: 256 * 1024, ..Limits::default() }).unwrap();
        assert_eq!(count(&tree, &[], "target"), 1);
    }
}
