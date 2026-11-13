use super::*;

/// Largest package part read while validating a written package.
const MAX_XML_PART: u64 = 64 * 1024 * 1024;

/// Most zip entries looked at while validating a written package.
pub(super) const MAX_ENTRIES: usize = 200_000;

/// The zip package `bytes` is, when it is one from its first byte (a local file header). The zip
/// reader also finds an archive stored inside another file (an `.xls` that embeds an `.xlsx` as an
/// OLE object), which says nothing about the file itself.
pub(super) fn zip_package(bytes: &[u8]) -> Option<zip::ZipArchive<Cursor<&[u8]>>> {
    if !bytes.starts_with(b"PK\x03\x04") {
        return None;
    }
    zip::ZipArchive::new(Cursor::new(bytes)).ok()
}

// ---------------------------------------------------------------- package validation

pub(super) fn local_name(name: &[u8]) -> &[u8] {
    match name.iter().rposition(|&b| b == b':') {
        Some(i) => name.get(i + 1..).unwrap_or(name),
        None => name,
    }
}

/// `(element local name, attributes)` for every start/empty element of an XML part.
type XmlElement = (String, Vec<(String, String)>);

pub(super) fn xml_elements(xml: &[u8]) -> Result<Vec<XmlElement>, String> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_reader(xml);
    let mut buf = Vec::new();
    let mut out = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                let attrs = e
                    .attributes()
                    .with_checks(false)
                    .flatten()
                    .map(|a| {
                        let v = match a.normalized_value(quick_xml::XmlVersion::Explicit1_0) {
                            Ok(v) => v.into_owned(),
                            Err(_) => String::from_utf8_lossy(&a.value).into_owned(),
                        };
                        (String::from_utf8_lossy(local_name(a.key.as_ref())).into_owned(), v)
                    })
                    .collect();
                out.push((String::from_utf8_lossy(local_name(e.name().as_ref())).into_owned(), attrs));
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(e) => return Err(format!("invalid XML: {e}")),
        }
        buf.clear();
    }
    Ok(out)
}

pub(super) fn attr<'a>(attrs: &'a [(String, String)], k: &str) -> Option<&'a str> {
    attrs.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
}

/// Lower-cased part name without a leading slash: OPC part names compare case-insensitively.
pub(super) fn part_key(name: &str) -> String {
    name.trim_start_matches('/').to_ascii_lowercase()
}

/// `%XX` escapes in a relationship target.
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while let Some(&c) = b.get(i) {
        let hex = |j: usize| b.get(j).and_then(|h| (*h as char).to_digit(16));
        match (c, hex(i + 1), hex(i + 2)) {
            (b'%', Some(h), Some(l)) => {
                out.push((h * 16 + l) as u8);
                i += 3;
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Resolves a relationship target against the source part's directory.
pub(super) fn resolve_target(source_dir: &str, target: &str) -> String {
    let target = percent_decode(target.split('#').next().unwrap_or(target));
    let joined = match target.strip_prefix('/') {
        Some(abs) => abs.to_string(),
        None if source_dir.is_empty() => target,
        None => format!("{source_dir}/{target}"),
    };
    let mut parts: Vec<&str> = Vec::new();
    for seg in joined.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

pub(super) type Zip<'a> = zip::ZipArchive<Cursor<&'a [u8]>>;

pub(super) fn zip_names(zip: &mut Zip<'_>) -> Vec<String> {
    (0..zip.len().min(MAX_ENTRIES)).filter_map(|i| zip.by_index_raw(i).ok().filter(|f| !f.is_dir()).map(|f| f.name().to_string())).collect()
}

pub(super) fn read_zip_part(zip: &mut Zip<'_>, name: &str) -> Result<Vec<u8>, String> {
    let f = zip.by_name(name).map_err(|e| format!("{name}: {e}"))?;
    let mut buf = Vec::new();
    f.take(MAX_XML_PART).read_to_end(&mut buf).map_err(|e| format!("{name}: {e}"))?;
    Ok(buf)
}

pub(super) enum PartError {
    /// Reading the part would spend more than the remaining budget.
    OverBudget,
    Unreadable,
}

/// Reads a part like [`read_zip_part`], charging the decompressed bytes to `budget`. A part larger
/// than the budget left or than [`MAX_XML_PART`] isn't returned cut short: it is `OverBudget`.
pub(super) fn read_zip_part_within(zip: &mut Zip<'_>, name: &str, budget: &mut u64) -> Result<Vec<u8>, PartError> {
    let f = zip.by_name(name).map_err(|_| PartError::Unreadable)?;
    // One byte past what is allowed shows that the part doesn't fit.
    let allowed = MAX_XML_PART.min(*budget);
    let mut buf = Vec::new();
    // Bytes read before a decompression error still count against the budget.
    let res = f.take(allowed.saturating_add(1)).read_to_end(&mut buf);
    let read = buf.len() as u64;
    if read > allowed {
        *budget = 0;
        return Err(PartError::OverBudget);
    }
    *budget -= read;
    res.map_err(|_| PartError::Unreadable)?;
    Ok(buf)
}

/// The zip entry holding the workbook part: the target of the package's `officeDocument`
/// relationship, `xl/workbook.xml` when the package doesn't say.
pub(super) fn workbook_entry(zip: &mut Zip<'_>, names: &[String]) -> Option<String> {
    let find = |key: &str| names.iter().find(|n| part_key(n) == key).cloned();
    let mut workbook = "xl/workbook.xml".to_string();
    if let Some(rels) = find("_rels/.rels")
        && let Ok(xml) = read_zip_part(zip, &rels)
        && let Ok(els) = xml_elements(&xml)
        && let Some(target) = els
            .iter()
            .find(|(e, a)| e == "Relationship" && attr(a, "Type").is_some_and(|t| t.ends_with("/officeDocument")))
            .and_then(|(_, a)| attr(a, "Target"))
    {
        workbook = resolve_target("", target);
    }
    find(&part_key(&workbook))
}

/// An XML Schema boolean (`1`/`0`/`true`/`false`); `None` for anything else.
fn xsd_bool(v: &str) -> Option<bool> {
    match v.trim().to_ascii_lowercase().as_str() {
        "1" | "true" => Some(true),
        "0" | "false" => Some(false),
        _ => None,
    }
}

/// What the workbook part's `<calcPr>` says about how the producer calculated, read from the
/// package itself (the importer keeps only part of it). A package without `<calcPr>`, or one that
/// can't be read, has the defaults: automatic calculation, completed.
pub(super) fn calc_props(bytes: &[u8]) -> CalcProps {
    let mut props = CalcProps::default();
    let Some(mut zip) = zip_package(bytes) else { return props };
    let names = zip_names(&mut zip);
    let Some(name) = workbook_entry(&mut zip, &names) else { return props };
    let Ok(xml) = read_zip_part(&mut zip, &name) else { return props };
    let Some((_, a)) = xml_elements(&xml).ok().into_iter().flatten().find(|(e, _)| e == "calcPr") else { return props };
    props.mode = match attr(&a, "calcMode").map(|m| m.trim().to_ascii_lowercase()).as_deref() {
        Some("manual") => CalcMode::Manual,
        Some("autonotable") => CalcMode::AutoNoTable,
        _ => CalcMode::Auto,
    };
    props.completed = attr(&a, "calcCompleted").and_then(xsd_bool);
    props.full_calc_on_load = attr(&a, "fullCalcOnLoad").and_then(xsd_bool).unwrap_or(false);
    props.iterate = attr(&a, "iterate").and_then(xsd_bool).unwrap_or(false);
    props.calc_id = attr(&a, "calcId").and_then(|v| v.trim().parse().ok());
    props
}

/// Problems Excel would repair in a written package: parts without a content type, content-type
/// overrides for missing parts, relationship files for missing parts, duplicate relationship ids
/// and internal relationships pointing at missing parts. `Err` when the package can't be read.
pub fn validate_package(bytes: &[u8]) -> Result<Vec<String>, String> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| format!("not a zip package: {e}"))?;
    let names = zip_names(&mut zip);
    let parts: HashSet<String> = names.iter().map(|n| part_key(n)).collect();
    let mut read_part = |name: &str| read_zip_part(&mut zip, name);
    let mut issues = Vec::new();
    let ct_name = names.iter().find(|n| part_key(n) == "[content_types].xml").cloned();
    let (mut defaults, mut overrides) = (HashSet::new(), HashSet::new());
    match ct_name {
        None => issues.push("no [Content_Types].xml".to_string()),
        Some(n) => {
            for (el, attrs) in xml_elements(&read_part(&n)?).map_err(|e| format!("[Content_Types].xml: {e}"))? {
                match el.as_str() {
                    "Default" => {
                        if let Some(x) = attr(&attrs, "Extension") {
                            defaults.insert(x.to_ascii_lowercase());
                        }
                    }
                    "Override" => {
                        if let Some(p) = attr(&attrs, "PartName") {
                            overrides.insert(part_key(p));
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    let mut sorted: Vec<&String> = names.iter().collect();
    sorted.sort();
    for n in &sorted {
        let key = part_key(n);
        if key == "[content_types].xml" {
            continue;
        }
        let ext = key.rsplit('/').next().and_then(|f| f.rsplit_once('.')).map(|(_, e)| e.to_string());
        if !overrides.contains(&key) && !ext.is_some_and(|e| defaults.contains(&e)) {
            issues.push(format!("part {n} has no content type"));
        }
    }
    let mut missing_overrides: Vec<&String> = overrides.iter().filter(|p| !parts.contains(*p)).collect();
    missing_overrides.sort();
    for p in missing_overrides {
        issues.push(format!("content type override for missing part /{p}"));
    }
    for n in sorted {
        let key = part_key(n);
        let Some(rels_file) = key.strip_suffix(".rels") else { continue };
        // `dir/_rels/name.rels` describes `dir/name`; `_rels/.rels` the package itself.
        let (dir, file) = match rels_file.rsplit_once('/') {
            Some((d, f)) => (d, f),
            None => continue,
        };
        let Some(src_dir) = dir.strip_suffix("_rels").map(|d| d.trim_end_matches('/')) else { continue };
        let source = if src_dir.is_empty() { file.to_string() } else { format!("{src_dir}/{file}") };
        if !source.is_empty() && !parts.contains(&source) {
            issues.push(format!("{n} describes missing part {source}"));
        }
        let mut ids = HashSet::new();
        for (el, attrs) in xml_elements(&read_part(n)?).map_err(|e| format!("{n}: {e}"))? {
            if el != "Relationship" {
                continue;
            }
            let id = attr(&attrs, "Id").unwrap_or("");
            if !ids.insert(id.to_string()) {
                issues.push(format!("{n}: duplicate relationship id {id}"));
            }
            if attr(&attrs, "TargetMode").is_some_and(|m| m.eq_ignore_ascii_case("External")) {
                continue;
            }
            let target = attr(&attrs, "Target").unwrap_or("");
            // A target that is only a fragment (`#Sheet1!A1`, how Excel links a shape or picture to a
            // cell) resolves, by RFC 3986, to the source part itself: no other part can be missing.
            if target.starts_with('#') {
                continue;
            }
            let resolved = resolve_target(src_dir, target);
            if !parts.contains(&resolved.to_ascii_lowercase()) {
                let kind = attr(&attrs, "Type").and_then(|t| t.rsplit('/').next()).unwrap_or("?");
                issues.push(format!("{n}: {id} ({kind}) targets missing part {resolved}"));
            }
        }
    }
    Ok(issues)
}

/// Whether a part has content: an element under its root, or for comments (whose authors list
/// comes first) a comment, for VML a shape. A part that isn't XML (printer settings, a VBA
/// project, a thumbnail) is binary content, and an empty one holds nothing. Read only as far as
/// needed; an unreadable part counts.
/// `verdicts` remembers the answer per (part, named content); `budget` is the decompression this
/// package's remaining checks may cause, and what the check reads is charged to it. `None` when the
/// answer lies past what the budget (or the largest part read) allows: what the part holds is
/// unknown, which is neither holding something nor holding nothing.
pub(super) fn holds_something(
    zip: &mut Zip<'_>,
    name: &str,
    kind: &str,
    verdicts: &mut HashMap<(String, &'static str), bool>,
    budget: &mut u64,
) -> Option<bool> {
    let wanted: Option<&'static [u8]> = match kind.to_ascii_lowercase().as_str() {
        "comments" => Some(b"comment"),
        "vmldrawing" => Some(b"shape"),
        _ => None,
    };
    let marker = match wanted {
        Some(b"comment") => "comment",
        Some(_) => "shape",
        None => "any",
    };
    if let Some(v) = verdicts.get(&(name.to_ascii_lowercase(), marker)) {
        return Some(*v);
    }
    let v = holds_something_uncached(zip, name, marker, budget)?;
    verdicts.insert((name.to_ascii_lowercase(), marker), v);
    Some(v)
}

fn holds_something_uncached(zip: &mut Zip<'_>, name: &str, marker: &'static str, budget: &mut u64) -> Option<bool> {
    let Ok(f) = zip.by_name(name) else { return Some(true) };
    // The directory says the part has no bytes: it is empty, whatever its (possibly missing)
    // compressed stream looks like. Some producers write an empty file as a deflate entry with no
    // stream at all, which can't be decoded and must not be mistaken for an unreadable part.
    if f.size() == 0 {
        return Some(false);
    }
    // One byte past what is allowed shows that the answer isn't within it. Only what the scan
    // reads is charged, not the size of the part: it stops at the first element that decides.
    let allowed = MAX_XML_PART.min(*budget);
    let mut window = f.take(allowed.saturating_add(1));
    let holds = has_content(std::io::BufReader::new(&mut window), marker);
    let read = allowed.saturating_add(1) - window.limit();
    if read > allowed {
        *budget = 0;
        return None;
    }
    *budget -= read;
    Some(holds)
}

/// [`holds_something`] on the bytes of the part: `marker` names the content wanted (`comment`,
/// `shape`), any element under the root otherwise.
fn has_content(mut input: impl std::io::BufRead, marker: &str) -> bool {
    use quick_xml::events::Event;
    // XML starts with `<` (after a byte order mark and white space). Whatever else it starts
    // with is not XML, and parsing its bytes as XML would decide by chance.
    let head = input.fill_buf().unwrap_or_default();
    let head = head.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(head);
    if let Some(first) = head.iter().find(|b| !b.is_ascii_whitespace())
        && *first != b'<'
    {
        return true;
    }
    let mut reader = quick_xml::Reader::from_reader(input);
    let wanted: Option<&'static [u8]> = match marker {
        "comment" => Some(b"comment"),
        "shape" => Some(b"shape"),
        _ => None,
    };
    let mut buf = Vec::new();
    let mut in_root = false;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                let found = match wanted {
                    Some(w) => local_name(e.name().as_ref()) == w,
                    None => in_root,
                };
                if found {
                    return true;
                }
                in_root = true;
            }
            Ok(Event::Eof) => return false,
            Ok(_) => {}
            Err(_) => return true,
        }
        buf.clear();
    }
}
