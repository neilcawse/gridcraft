use super::*;

/// A part's relationships with full type URIs, in their Transitional spelling (a Strict package's
/// are renamed, so that one rule decides what each kind is). `Ok(vec![])` when it has none; an
/// error when the rels part is damaged or too large (its part then can't be kept faithfully).
fn raw_rels(cx: &mut Ctx<'_>, part: &str) -> Result<Vec<PreservedRel>, String> {
    raw_rels_marked(cx, part).map(|(rels, _)| rels)
}

/// [`raw_rels`], and whether any of the types was a Strict one.
fn raw_rels_marked(cx: &mut Ctx<'_>, part: &str) -> Result<(Vec<PreservedRel>, bool), String> {
    let name = rels_part_name(part);
    let bytes = match cx.pkg.read_limited(&name, MAX_META) {
        Ok(Some(b)) => b,
        Ok(None) => return Ok((vec![], false)),
        Err(e) => return Err(e.to_string()),
    };
    let root = xml::parse(&bytes).map_err(|e| format!("{name}: {e}"))?;
    let (dir, _) = package::split_dir(part);
    let mut out = vec![];
    let mut strict = false;
    for r in root.kids("Relationship") {
        if out.len() >= MAX_RELS {
            return Err(format!("{name}: too many relationships"));
        }
        let (Some(id), Some(target)) = (r.attr("Id"), r.attr("Target")) else { continue };
        let external = r.attr("TargetMode").is_some_and(|m| m.eq_ignore_ascii_case("External"));
        let rel_type = strict::rel_type(r.attr("Type").unwrap_or(""));
        strict |= matches!(rel_type, Cow::Owned(_));
        out.push(PreservedRel {
            id: id.to_string(),
            rel_type: rel_type.into_owned(),
            target: if external { target.to_string() } else { package::resolve(dir, target) },
            external,
        });
    }
    Ok((out, strict))
}

// ---------------------------------------------------------------- reading

/// A `<sheet>` entry of the workbook that GridCraft keeps whole.
struct KeptEntry<'a> {
    /// Its position in the file's list of sheets (what `localSheetId` counts).
    file_index: usize,
    /// The number of worksheets (model sheets) before it.
    before: usize,
    se: &'a xml::El,
    kind: PreservedSheetKind,
}

/// The `<sheet>` entries of the workbook that GridCraft keeps whole (chart, dialog and macro
/// sheets).
fn kept_sheet_entries<'a>(wb_xml: &'a xml::El, wb_rels: &[PreservedRel], file_to_model: &[Option<usize>]) -> Vec<KeptEntry<'a>> {
    let mut out = vec![];
    let mut before = 0usize;
    for (file_index, se) in wb_xml.child("sheets").into_iter().flat_map(|s| s.kids("sheet")).enumerate() {
        let kind = if file_to_model.get(file_index).copied().flatten().is_some() {
            before += 1;
            continue;
        } else if super::is_module_entry(se) {
            PreservedSheetKind::Module
        } else if let Some(kind) = se.attr("id").and_then(|id| wb_rels.iter().find(|r| r.id == id)).and_then(|r| sheet_kind(super::kind(&r.rel_type)))
        {
            kind
        } else {
            continue;
        };
        out.push(KeptEntry { file_index, before, se, kind });
    }
    out
}

/// The attributes of `CT_DefinedName` (ECMA-376 Part 1, §18.2.5) other than `localSheetId`.
const NAME_ATTRS: &[&str] = &[
    "name",
    "comment",
    "customMenu",
    "description",
    "help",
    "statusBar",
    "hidden",
    "function",
    "vbProcedure",
    "xlm",
    "functionGroupId",
    "shortcutKey",
    "publishToServer",
    "workbookParameter",
];

/// The defined names of the workbook scoped to the sheet at `file_index`.
fn sheet_names(wb_xml: &xml::El, file_index: usize) -> Vec<PreservedName> {
    let names = wb_xml.child("definedNames").into_iter().flat_map(|d| d.kids("definedName"));
    names
        .filter(|d| d.attr_u32("localSheetId").and_then(|i| usize::try_from(i).ok()) == Some(file_index))
        .map(|d| {
            let mut attrs = String::new();
            for k in NAME_ATTRS {
                if let Some(v) = d.attr(k) {
                    let _ = write!(attrs, " {k}=\"{}\"", esc_attr(v));
                }
            }
            PreservedName { attrs, formula: d.text.clone() }
        })
        .collect()
}

/// An index found in a kept sheet's XML, as written there: `None` when it isn't a number.
enum RawIndex {
    /// The `<v>` of a `t="s"` cell: a shared string.
    String(Option<usize>),
    /// `s` of a cell or row, `style` of a column: a cell format.
    Style(Option<u32>),
    /// `dxfId`: a differential format.
    Dxf(Option<u32>),
    /// A cell's `vm` or `cm`, the whole attribute: an index into the cell and value metadata.
    Metadata,
}

/// The attributes of the start tag `tag` (`<c r="A1" s="2">`): each one's name as written, the
/// byte range of its value (quotes excluded) and of the whole attribute with the white space before
/// it. Stops at the first one that isn't well formed.
pub(super) fn attribute_spans(tag: &str) -> Vec<(&str, std::ops::Range<usize>, std::ops::Range<usize>)> {
    let b = tag.as_bytes();
    let at = |i: usize| b.get(i).copied();
    let name_end = |c: u8| c.is_ascii_whitespace() || matches!(c, b'=' | b'>' | b'/');
    let mut i = 1;
    while at(i).is_some_and(|c| !name_end(c)) {
        i += 1;
    }
    let mut out = vec![];
    loop {
        let whole = i;
        while at(i).is_some_and(|c| c.is_ascii_whitespace()) {
            i += 1;
        }
        let name_start = i;
        while at(i).is_some_and(|c| !name_end(c)) {
            i += 1;
        }
        let Some(name) = tag.get(name_start..i).filter(|n| !n.is_empty()) else { return out };
        while at(i).is_some_and(|c| c.is_ascii_whitespace()) {
            i += 1;
        }
        if at(i) != Some(b'=') {
            return out;
        }
        i += 1;
        while at(i).is_some_and(|c| c.is_ascii_whitespace()) {
            i += 1;
        }
        let Some(quote) = at(i).filter(|q| matches!(q, b'"' | b'\'')) else { return out };
        let value = i + 1;
        let Some(len) = b.get(value..).and_then(|rest| rest.iter().position(|&c| c == quote)) else { return out };
        i = value + len + 1;
        out.push((name, value..value + len, whole..i));
    }
}

/// The indexes the sheet XML `text` holds into the workbook's shared strings and cell and
/// differential formats, each with the byte range it takes in `text`, in order. `None` when the
/// XML is damaged.
fn sheet_index_spans(text: &str) -> Option<Vec<(std::ops::Range<usize>, RawIndex)>> {
    let mut reader = Reader::from_str(text);
    let mut out = vec![];
    let mut in_string_cell = false;
    loop {
        let start = usize::try_from(reader.buffer_position()).ok()?;
        let event = reader.read_event().ok()?;
        let end = usize::try_from(reader.buffer_position()).ok()?;
        let (e, open) = match event {
            Event::Eof => return Some(out),
            Event::Start(e) if in_string_cell && e.local_name().as_ref() == b"v" => {
                reader.read_to_end(e.name()).ok()?;
                let after = usize::try_from(reader.buffer_position()).ok()?;
                let close = end + text.get(end..after)?.rfind("</")?;
                out.push((end..close, RawIndex::String(text.get(end..close)?.trim().parse().ok())));
                continue;
            }
            Event::End(e) => {
                if e.local_name().as_ref() == b"c" {
                    in_string_cell = false;
                }
                continue;
            }
            Event::Start(e) => (e, true),
            Event::Empty(e) => (e, false),
            _ => continue,
        };
        let local = e.local_name();
        let tag = text.get(start..end)?;
        let style_attr: &str = match local.as_ref() {
            b"c" | b"row" => "s",
            b"col" => "style",
            _ => "",
        };
        let mut string_cell = false;
        for (name, value, whole) in attribute_spans(tag) {
            let text_of = tag.get(value.clone()).unwrap_or("").trim();
            let span = start + value.start..start + value.end;
            if name == style_attr {
                out.push((span, RawIndex::Style(text_of.parse().ok())));
            } else if name == "dxfId" {
                out.push((span, RawIndex::Dxf(text_of.parse().ok())));
            } else if local.as_ref() == b"c" && name == "t" {
                string_cell = text_of == "s";
            } else if local.as_ref() == b"c" && matches!(name, "vm" | "cm") {
                out.push((start + whole.start..start + whole.end, RawIndex::Metadata));
            }
        }
        if local.as_ref() == b"c" {
            in_string_cell = string_cell && open;
        }
    }
}

/// The indexes the cells of the kept macro sheet `root` (in `q`) hold into tables of the
/// workbook that are written from the model, with what each refers to: they are given anew on
/// save. A text cell whose index is no shared string is written empty, with a warning; a format
/// index that is no format is written as the default one.
fn sheet_indexes(cx: &mut Ctx<'_>, q: &Preserved, root: &PreservedRel, name: &str) -> Result<Vec<PreservedIndex>, String> {
    let Some(part) = q.parts.iter().find(|p| normalize(&p.name) == normalize(&root.target)) else { return Ok(vec![]) };
    let Ok(text) = std::str::from_utf8(&part.data) else {
        // Not UTF-8: kept as it is unless it has indexes, which can't be rewritten.
        let utf8 = xml::to_utf8(&part.data);
        let spans = std::str::from_utf8(&utf8).ok().and_then(sheet_index_spans);
        return match spans {
            Some(s) if s.is_empty() => Ok(vec![]),
            _ => Err("its cells are not in UTF-8 and their text and formats can't be renumbered".into()),
        };
    };
    let spans = sheet_index_spans(text).ok_or("its XML is damaged")?;
    let mut missing = 0usize;
    let indexes = spans
        .into_iter()
        .map(|(at, raw)| {
            let to = match raw {
                RawIndex::String(i) => {
                    let text = i.and_then(|i| cx.sst.get(i)).cloned();
                    missing += usize::from(text.is_none());
                    let reading = i.and_then(|i| cx.si_phonetics.get(i).copied().flatten());
                    PreservedIndexed::String { text: text.unwrap_or_else(|| Arc::from("")), reading }
                }
                RawIndex::Style(i) => PreservedIndexed::Style(cx.style(i)),
                RawIndex::Dxf(i) => PreservedIndexed::Dxf(i.and_then(|i| cx.dxfs.get(i as usize)).cloned().unwrap_or_default()),
                RawIndex::Metadata => PreservedIndexed::Metadata,
            };
            PreservedIndex { at, to }
        })
        .collect();
    if missing > 0 {
        cx.warn(format!("macro sheet \"{name}\": {missing} text cells refer to no shared string and are written empty"));
    }
    Ok(indexes)
}

/// Collects kept content while a package is read.
pub struct KeptReader {
    on: bool,
    strict: bool,
    defaults: HashMap<String, String>,
    overrides: HashMap<String, String>,
    budget: usize,
    parts_left: usize,
    /// Whether a VML drawing draws more than notes, by part name: many sheets can point at one.
    vml_objects: HashMap<String, Result<bool, String>>,
    /// Whether the workbook lists custom views: only then can its sheets hold `<customSheetViews>`,
    /// so only then are they looked for.
    custom_views: bool,
    /// Cells with value metadata GridCraft doesn't model, over all sheets read so far.
    value_cells: usize,
}

impl KeptReader {
    pub fn new(cx: &mut Ctx<'_>, wb_xml: &xml::El) -> KeptReader {
        let mut k = KeptReader {
            on: true,
            strict: false,
            defaults: HashMap::new(),
            overrides: HashMap::new(),
            budget: kept_total(),
            parts_left: MAX_KEPT_PARTS,
            vml_objects: HashMap::new(),
            custom_views: wb_xml.child(SLOT_CUSTOM_WORKBOOK_VIEWS).is_some(),
            value_cells: 0,
        };
        // A Strict package's kept parts are written as Transitional, like the rest.
        k.strict = raw_rels_marked(cx, "").is_ok_and(|(_, strict)| strict);
        match cx.pkg.read_limited("[Content_Types].xml", MAX_META).ok().flatten().and_then(|b| xml::parse(&b).ok()) {
            Some(types) => {
                for d in types.kids("Default") {
                    if let (Some(e), Some(t)) = (d.attr("Extension"), d.attr("ContentType")) {
                        k.defaults.insert(e.to_ascii_lowercase(), t.to_string());
                    }
                }
                for o in types.kids("Override") {
                    if let (Some(p), Some(t)) = (o.attr("PartName"), o.attr("ContentType")) {
                        k.overrides.insert(normalize(p), t.to_string());
                    }
                }
            }
            None => k.on = false,
        }
        k
    }

    fn content_type(&self, part: &str) -> Option<String> {
        let key = normalize(part);
        if let Some(t) = self.overrides.get(&key) {
            return Some(t.clone());
        }
        let (_, file) = package::split_dir(&key);
        let ext = file.rsplit_once('.').map(|(_, e)| e)?;
        self.defaults.get(ext).cloned()
    }

    /// The bytes of the part `name` (of `content_type`) as they are kept: the XML of a Strict
    /// package with its namespaces renamed.
    fn kept_bytes(&self, name: &str, content_type: &str, data: Vec<u8>) -> Result<Vec<u8>, String> {
        if self.strict { strict::part(data, content_type).map_err(|why| format!("{name}: {why}")) } else { Ok(data) }
    }

    /// XML cut out of a part, as it is kept.
    fn kept_xml(&self, x: String) -> String {
        if self.strict { strict::xml(&x).into_owned() } else { x }
    }

    /// The parts reachable from `root` that aren't in `seen` yet. Fails (and keeps nothing) when the
    /// graph reaches rewritten content, a missing or damaged part, or a size limit.
    fn closure(&mut self, cx: &mut Ctx<'_>, root: &PreservedRel, seen: &HashSet<String>, through: &[&str]) -> Result<Vec<PreservedPart>, String> {
        if root.external {
            return Ok(vec![]);
        }
        let blocked = |r: &PreservedRel| rewritten(r) && !through.iter().any(|k| k.eq_ignore_ascii_case(kind(&r.rel_type)));
        if blocked(root) {
            return Err(format!("it refers to {}", kind(&root.rel_type)));
        }
        let mut parts = vec![];
        let mut bytes = 0usize;
        let mut local: HashSet<String> = HashSet::new();
        let mut queue = VecDeque::from([(root.target.clone(), 0usize)]);
        while let Some((name, depth)) = queue.pop_front() {
            let key = normalize(&name);
            if seen.contains(&key) || !local.insert(key) {
                continue;
            }
            if depth > MAX_CHAIN {
                return Err("its relationships are nested too deeply".into());
            }
            if parts.len() >= self.parts_left {
                return Err("it has too many parts".into());
            }
            let content_type = self.content_type(&name).ok_or_else(|| format!("{name} has no content type"))?;
            // A part the directory says is empty reads as empty (no stream is decoded). One that
            // holds bytes the zip reader can't produce can't be carried through the writer, which
            // takes decoded parts: it is dropped, and the caller's warning says so.
            let data = match cx.pkg.read_limited(&name, MAX_KEPT_PART) {
                Ok(Some(d)) => d,
                Ok(None) => return Err(format!("{name} is missing")),
                Err(IoError::Zip(why)) => return Err(format!("it can't be read ({why})")),
                Err(e) => return Err(e.to_string()),
            };
            let data = self.kept_bytes(&name, &content_type, data)?;
            bytes = bytes.saturating_add(data.len());
            if bytes > self.budget {
                return Err("it is too large".into());
            }
            let rels = raw_rels(cx, &name)?;
            for r in rels.iter().filter(|r| !r.external) {
                if blocked(r) {
                    return Err(format!("it refers to {}", kind(&r.rel_type)));
                }
                queue.push_back((r.target.clone(), depth + 1));
            }
            parts.push(PreservedPart { name, content_type, data: Arc::from(data), rels });
        }
        self.budget = self.budget.saturating_sub(bytes);
        self.parts_left = self.parts_left.saturating_sub(parts.len());
        Ok(parts)
    }

    /// Keeps a set of relationships (all or nothing) and the parts behind them.
    fn keep(&mut self, cx: &mut Ctx<'_>, p: &mut Preserved, roots: &[PreservedRel], seen: &mut HashSet<String>) -> Result<(), String> {
        self.keep_through(cx, p, roots, seen, &[])
    }

    /// [`Self::keep`] for roots whose graph may reach the rewritten kinds in `through`.
    fn keep_through(
        &mut self,
        cx: &mut Ctx<'_>,
        p: &mut Preserved,
        roots: &[PreservedRel],
        seen: &mut HashSet<String>,
        through: &[&str],
    ) -> Result<(), String> {
        let mut new_parts: Vec<PreservedPart> = vec![];
        let mut scratch = seen.clone();
        for r in roots {
            let parts = self.closure(cx, r, &scratch, through)?;
            scratch.extend(parts.iter().map(|q| normalize(&q.name)));
            new_parts.extend(parts);
        }
        *seen = scratch;
        p.parts.extend(new_parts);
        for r in roots {
            if !p.rels.iter().any(|x| x.id == r.id) {
                p.rels.push(r.clone());
            }
        }
        Ok(())
    }

    /// The elements of the document properties part `rel` points at that GridCraft doesn't write
    /// itself (`written`), each as XML. An absent part has none. `dest_default` is the default
    /// namespace of the part GridCraft writes.
    fn property_extras(&mut self, cx: &mut Ctx<'_>, rel: &PreservedRel, written: &[&str], dest_default: &str) -> Result<Vec<String>, String> {
        let bytes = match cx.pkg.read_limited(&rel.target, MAX_META) {
            Ok(Some(b)) => b,
            Ok(None) => return Ok(vec![]),
            Err(e) => return Err(e.to_string()),
        };
        // The document properties parts are XML whatever content type they were given.
        let bytes = self.kept_bytes(&rel.target, "application/xml", bytes)?;
        root_children_except(&bytes, written, PROPERTY_NAMESPACES, dest_default)
            .ok_or_else(|| "the part is damaged or has an element that is too large".to_string())
    }

    /// Package-root and workbook-level content.
    pub fn workbook(&mut self, cx: &mut Ctx<'_>, wb_part: &str, wb_xml: &xml::El, file_to_model: &[Option<usize>]) -> Option<Arc<Preserved>> {
        if !self.on {
            let wb_rels = raw_rels(cx, wb_part).unwrap_or_default();
            for KeptEntry { se, kind, .. } in kept_sheet_entries(wb_xml, &wb_rels, file_to_model) {
                cx.warn(format!("{} \"{}\" was skipped and is not kept on save", sheet_label(kind), se.attr("name").unwrap_or("")));
            }
            if self.value_cells > 0 {
                cx.warn(format!("{} cells have value metadata GridCraft doesn't model; they keep their values only", self.value_cells));
            }
            return None;
        }
        let mut p = Preserved::default();
        let mut seen = HashSet::new();
        // Package root: custom document properties, the thumbnail, sensitivity labels, ribbon
        // customisation, task panes… everything but the parts GridCraft writes itself.
        if let Ok(root) = raw_rels(cx, "") {
            for r in root.iter().filter(|r| is_root(Owner::Package, r)) {
                let mut q = Preserved::default();
                match self.keep(cx, &mut q, std::slice::from_ref(r), &mut seen) {
                    Ok(()) => {
                        p.parts.extend(q.parts);
                        p.package_rels.extend(q.rels);
                    }
                    Err(why) => cx.warn(format!("{} part {} was dropped: {why}", kind(&r.rel_type), r.target)),
                }
            }
            // The core and the extended properties are written from the document properties
            // GridCraft models: the rest of those parts is kept as XML.
            for (slot, written, dest_default) in
                [(SLOT_CORE_PROPERTIES, CORE_WRITTEN, ""), (SLOT_EXTENDED_PROPERTIES, EXTENDED_WRITTEN, NS_EXTENDED_PROPERTIES)]
            {
                let Some(r) = root.iter().find(|r| !r.external && is_kind(r, &[slot])) else { continue };
                match self.property_extras(cx, r, written, dest_default) {
                    Ok(xs) => p.elements.extend(xs.into_iter().map(|xml| PreservedXml { slot: slot.into(), xml })),
                    Err(why) => cx.warn(format!("{} part {}: elements GridCraft does not model were dropped: {why}", kind(&r.rel_type), r.target)),
                }
            }
        }
        let wb_rels = raw_rels(cx, wb_part).unwrap_or_default();
        for r in wb_rels.iter().filter(|r| is_root(Owner::Workbook, r)) {
            if let Err(why) = self.keep(cx, &mut p, std::slice::from_ref(r), &mut seen) {
                cx.warn(format!("{} part {} was dropped: {why}", kind(&r.rel_type), r.target));
            }
        }
        // Chart, dialog and macro sheets: the sheet entry (name, state, position) plus the sheet part
        // and the drawing, chart, VML, style and colour parts behind it, and the names scoped to it.
        for KeptEntry { file_index, before, se, kind: what } in kept_sheet_entries(wb_xml, &wb_rels, file_to_model) {
            let name = se.attr("name").unwrap_or("").to_string();
            let entry = |rel_id: String, indexes: Vec<PreservedIndex>| PreservedSheet {
                name: name.clone(),
                kind: what,
                visibility: match se.attr("state") {
                    Some("hidden") => Visibility::Hidden,
                    Some("veryHidden") => Visibility::VeryHidden,
                    _ => Visibility::Visible,
                },
                rel_id,
                before,
                sheet_id: se.attr_u32("sheetId").unwrap_or(0),
                indexes,
                names: sheet_names(wb_xml, file_index),
            };
            // A module sheet's entry is all there is of it.
            if what == PreservedSheetKind::Module {
                p.sheets.push(entry(String::new(), vec![]));
                continue;
            }
            let Some(r) = se.attr("id").and_then(|id| wb_rels.iter().find(|r| r.id == id)) else { continue };
            let mut q = Preserved::default();
            let mut q_seen = seen.clone();
            let kept = self
                .keep_through(cx, &mut q, std::slice::from_ref(r), &mut q_seen, KEPT_SHEET_GRAPH)
                .and_then(|()| if what == PreservedSheetKind::Macro { sheet_indexes(cx, &q, r, &name) } else { Ok(vec![]) });
            match kept {
                Ok(indexes) => {
                    seen = q_seen;
                    p.parts.extend(q.parts);
                    p.rels.extend(q.rels);
                    p.sheets.push(entry(r.id.clone(), indexes));
                }
                Err(why) => cx.warn(format!("{} \"{name}\" was dropped: {why}", sheet_label(what))),
            }
        }
        // Links to external workbooks: `[1]` in formulas is the position in <externalReferences>,
        // so they are kept all together or not at all.
        let ids: Vec<&str> =
            wb_xml.child("externalReferences").map(|e| e.kids("externalReference").filter_map(|x| x.attr("id")).collect()).unwrap_or_default();
        if !ids.is_empty() {
            let roots: Option<Vec<PreservedRel>> =
                ids.iter().map(|id| wb_rels.iter().find(|r| r.id == *id && is_kind(r, &["externalLink"])).cloned()).collect();
            let kept = match roots {
                Some(roots) => self.keep(cx, &mut p, &roots, &mut seen),
                None => Err("a link has no part".into()),
            };
            match kept {
                Ok(()) => {
                    let mut x = format!("<externalReferences xmlns:r=\"{NS_REL}\">");
                    for id in &ids {
                        let _ = write!(x, "<externalReference r:id=\"{}\"/>", esc_attr(id));
                    }
                    x.push_str("</externalReferences>");
                    p.elements.push(PreservedXml { slot: "externalReferences".into(), xml: x });
                    cx.warn("links to external workbooks were kept; they are not followed or refreshed (formulas use the values saved with them)");
                }
                Err(why) => cx.warn(format!("links to external workbooks were dropped: {why}")),
            }
        }
        // Excel trusts the saved cell values only when both are present; without them it
        // recalculates everything on open, and links into closed workbooks come back as errors.
        if let Some(v) = wb_xml.child("fileVersion") {
            let mut x = String::from("<fileVersion");
            for k in FILE_VERSION_ATTRS {
                if let Some(a) = v.attr(k) {
                    let _ = write!(x, " {k}=\"{}\"", esc_attr(a));
                }
            }
            x.push_str("/>");
            p.elements.push(PreservedXml { slot: "fileVersion".into(), xml: x });
        }
        let calc_features = wb_xml
            .child("extLst")
            .and_then(|l| l.kids("ext").find(|e| e.attr("uri").is_some_and(|u| u.eq_ignore_ascii_case(CALC_FEATURES_URI))))
            .and_then(|e| e.child("calcFeatures"));
        if let Some(c) = calc_features {
            let mut x = format!("<ext uri=\"{CALC_FEATURES_URI}\" xmlns:xcalcf=\"{NS_CALC_FEATURES}\"><xcalcf:calcFeatures>");
            for name in c.kids("feature").filter_map(|f| f.attr("name")) {
                let _ = write!(x, "<xcalcf:feature name=\"{}\"/>", esc_attr(name));
            }
            x.push_str("</xcalcf:calcFeatures></ext>");
            p.elements.push(PreservedXml { slot: "ext".into(), xml: x });
        }
        // The custom views (View › Custom Views): the workbook lists them and each sheet holds the
        // settings of its own part of every view, so the one without the other means nothing.
        if self.custom_views {
            match cx.pkg.read_limited(wb_part, MAX_META) {
                Ok(Some(bytes)) => match root_child(&bytes, SLOT_CUSTOM_WORKBOOK_VIEWS) {
                    Some(x) => p.elements.push(PreservedXml { slot: SLOT_CUSTOM_WORKBOOK_VIEWS.into(), xml: self.kept_xml(x) }),
                    None => cx.warn("custom views were dropped: their list is damaged or too large"),
                },
                _ => cx.warn("custom views were dropped: the workbook could not be read again"),
            }
        }
        p.theme = self.theme(cx, &wb_rels);
        // A kept macro sheet's cells carry value and cell metadata as a worksheet's do (MS-OFFMACRO2
        // CT_Worksheet holds ordinary sheet data): the metadata they name is kept with theirs.
        let macro_cells = p.sheets.iter().flat_map(|s| &s.indexes).filter(|x| x.to == PreservedIndexed::Metadata).count();
        self.value_cells = self.value_cells.saturating_add(macro_cells);
        p.metadata = self.metadata(cx, &wb_rels);
        (!p.is_empty()).then(|| Arc::new(p))
    }

    /// The theme part with the parts its relationships reach (the pictures of its fill styles).
    /// GridCraft writes a theme from the model's colours and fonts only, so the rest of the part is
    /// kept, to be written back while those are unchanged. Not kept when the part's relationships
    /// can't be (it would refer to parts that are not there): the theme is then rewritten.
    fn theme(&mut self, cx: &mut Ctx<'_>, wb_rels: &[PreservedRel]) -> Option<PreservedTheme> {
        let root = wb_rels.iter().find(|r| !r.external && is_kind(r, &["theme"]))?;
        let data = cx.pkg.read_limited(&root.target, MAX_KEPT_PART).ok().flatten()?;
        let data = match self.kept_bytes(&root.target, "application/xml", data) {
            Ok(d) => d,
            Err(why) => {
                cx.warn(format!("{why}; the theme was rewritten from its colours and fonts"));
                return None;
            }
        };
        if data.len() > self.budget {
            cx.warn(format!("{} is too large to keep and was rewritten from the theme's colours and fonts", root.target));
            return None;
        }
        let rels = match raw_rels(cx, &root.target) {
            Ok(rels) => rels,
            Err(e) => {
                cx.warn(format!("the relationships of {} could not be read ({e}); the theme was rewritten from its colours and fonts", root.target));
                return None;
            }
        };
        let mut q = Preserved::default();
        if let Err(why) = self.keep(cx, &mut q, &rels, &mut HashSet::new()) {
            cx.warn(format!("what {} refers to was not kept ({why}); the theme was rewritten from its colours and fonts", root.target));
            return None;
        }
        self.budget = self.budget.saturating_sub(data.len());
        Some(PreservedTheme { data: Arc::from(data), rels: q.rels, parts: q.parts })
    }

    /// The cell and value metadata with the rich-value parts behind it ([`PreservedMetadata`]).
    ///
    /// GridCraft writes metadata of its own for its cell pictures and dynamic-array formulas.
    /// When cells carry value metadata beyond those (rich errors, linked data types, cube
    /// members), the whole set is kept, all or nothing, to be written back with those cells' `vm`.
    /// It is not kept next to cell pictures, which GridCraft writes with a metadata part of its
    /// own. Without such cells only the rich-value types part is kept.
    fn metadata(&mut self, cx: &mut Ctx<'_>, wb_rels: &[PreservedRel]) -> Option<PreservedMetadata> {
        let mut values = self.value_cells > 0;
        let has_part = wb_rels.iter().any(|r| !r.external && is_kind(r, &["sheetMetadata"]));
        let refused = if !values {
            None
        } else if cx.cell_pictures.iter().any(Option::is_some) {
            Some("the workbook also has cell pictures, which GridCraft writes with metadata of its own".to_string())
        } else if self.value_cells > MAX_CELL_META {
            Some(format!("there are more than {MAX_CELL_META} of them"))
        } else if !has_part {
            Some("the workbook has no metadata part".to_string())
        } else {
            None
        };
        if let Some(why) = refused {
            cx.warn(format!("{} cells have value metadata GridCraft doesn't model; they keep their values only: {why}", self.value_cells));
            values = false;
        }
        let kinds: &[&str] = if values { METADATA_KINDS } else { &[KIND_RICH_TYPES] };
        let roots: Vec<PreservedRel> = wb_rels.iter().filter(|r| !r.external && is_kind(r, kinds)).cloned().collect();
        if roots.is_empty() {
            return None;
        }
        let mut q = Preserved::default();
        if let Err(why) = self.keep_through(cx, &mut q, &roots, &mut HashSet::new(), METADATA_KINDS) {
            if values {
                cx.warn(format!("{} cells have value metadata GridCraft doesn't model; they keep their values only: {why}", self.value_cells));
            }
            return None;
        }
        let dynamic_arrays = roots
            .iter()
            .find(|r| is_kind(r, &["sheetMetadata"]))
            .and_then(|r| q.parts.iter().find(|part| normalize(&part.name) == normalize(&r.target)))
            .and_then(|part| xml::parse(&part.data).ok())
            .is_some_and(|m| first_cell_metadata_is_dynamic(&m));
        Some(PreservedMetadata { rels: q.rels, parts: q.parts, values, dynamic_arrays })
    }

    /// Sheet-level content of the sheet part `part`. `controls` says which form controls the
    /// sheet reader modelled (their VML shapes are left out of the kept content) and whether the
    /// VML drawing holds others that only it lists.
    pub fn sheet(
        &mut self,
        cx: &mut Ctx<'_>,
        part: &str,
        name: &str,
        sheet: &Sheet,
        value_meta: Vec<(CellRef, u32)>,
        controls: &crate::controls::ReadControls,
    ) -> Option<Arc<Preserved>> {
        self.value_cells = self.value_cells.saturating_add(value_meta.len());
        if !self.on {
            return None;
        }
        let mut p = Preserved::default();
        if self.value_cells <= MAX_CELL_META {
            // The cells as they will be (value and formula) are noted once all sheets are read.
            p.cell_meta = value_meta.into_iter().map(|(cell, vm)| PreservedCellMeta { cell, vm, ..Default::default() }).collect();
        }
        let rels = match raw_rels(cx, part) {
            Ok(r) => r,
            Err(e) => {
                cx.warn(format!("sheet \"{name}\": relationships could not be read ({e}); unmodelled content was dropped"));
                return (!p.is_empty()).then(|| Arc::new(p));
            }
        };
        let mut seen = HashSet::new();
        for r in rels.iter().filter(|r| is_root(Owner::Sheet, r)) {
            if let Err(why) = self.keep(cx, &mut p, std::slice::from_ref(r), &mut seen) {
                cx.warn(format!("sheet \"{name}\": {} was dropped: {why}", kind(&r.rel_type)));
            }
        }
        // Tables are written from the model, but what hangs off a table part (the query table
        // behind an external data range) is kept, with the file's numbering of the table's columns
        // that it refers to.
        for r in rels.iter().filter(|r| !r.external && is_kind(r, &["table"])) {
            self.table_children(cx, name, r, &sheet.tables, &mut p);
        }
        // A sheet's legacy VML drawing holds its notes (GridCraft writes those) and may hold more:
        // check boxes, buttons and pictures with no element of their own in the sheet XML. The
        // header and footer pictures are another VML drawing.
        let vml_objects =
            rels.iter().filter(|r| !r.external && is_kind(r, &["vmlDrawing"])).any(|r| self.vml_has_objects(cx, &r.target).unwrap_or(true));
        if vml_objects || self.custom_views || rels.iter().any(|r| is_kind(r, SHEET_ELEMENT_KINDS)) {
            self.sheet_elements(cx, part, name, &rels, &mut p, &mut seen, controls);
        }
        if rels.iter().any(|r| is_kind(r, &["comments"])) {
            self.note_fills(cx, name, &rels, &mut p, &mut seen);
        }
        (!p.is_empty()).then(|| Arc::new(p))
    }

    /// The pictures that fill the boxes of the sheet's notes, with the images they show. GridCraft
    /// writes the shapes of the notes itself, with the plain fill, so each `<v:fill>` is kept to
    /// go back into the shape of the note at its cell.
    fn note_fills(&mut self, cx: &mut Ctx<'_>, name: &str, rels: &[PreservedRel], p: &mut Preserved, seen: &mut HashSet<String>) {
        for rel in rels.iter().filter(|r| !r.external && is_kind(r, &["vmlDrawing"])) {
            let text = match cx.pkg.read_limited(&rel.target, MAX_META) {
                Ok(Some(d)) => String::from_utf8_lossy(&xml::to_utf8(&d)).into_owned(),
                _ => continue,
            };
            let found = note_fill_shapes(&text);
            if found.is_empty() {
                continue;
            }
            let vrels = match raw_rels(cx, &rel.target) {
                Ok(r) => r,
                Err(e) => {
                    cx.warn(format!("sheet \"{name}\": the pictures in the boxes of notes were dropped: {e}"));
                    return;
                }
            };
            let mut nf = PreservedNoteFills::default();
            for (cell, xml) in found {
                let wanted: Option<Vec<PreservedRel>> =
                    relids_in(&xml).iter().map(|id| vrels.iter().find(|r| r.id == *id && !r.external).cloned()).collect();
                let mut q = Preserved::default();
                let kept = match wanted.filter(|w| !w.is_empty()) {
                    Some(w) => self.keep(cx, &mut q, &w, seen),
                    None => Err("the drawing has no relationship for its picture".to_string()),
                };
                match kept {
                    Ok(()) => {
                        nf.parts.extend(q.parts);
                        for r in q.rels {
                            if !nf.rels.iter().any(|x| x.id == r.id) {
                                nf.rels.push(r);
                            }
                        }
                        nf.fills.push(PreservedNoteFill { cell, xml: self.kept_xml(xml) });
                    }
                    Err(why) => cx.warn(format!("sheet \"{name}\": the picture in the box of the note at {} was dropped: {why}", cell.a1())),
                }
            }
            if !nf.fills.is_empty() {
                p.note_fills = Some(nf);
            }
            // A sheet has one drawing for its notes.
            return;
        }
    }

    /// The relationships of the table part `table` to parts GridCraft doesn't rebuild, with those
    /// parts and the numbering of the table's columns, for the table of `tables` that part holds.
    fn table_children(&mut self, cx: &mut Ctx<'_>, sheet: &str, table: &PreservedRel, tables: &[Table], p: &mut Preserved) {
        let children: Vec<PreservedRel> = match raw_rels(cx, &table.target) {
            Ok(rels) => rels.into_iter().filter(|r| !rewritten(r)).collect(),
            Err(e) => {
                cx.warn(format!(
                    "sheet \"{sheet}\": the relationships of {} could not be read ({e}); what hangs off the table was dropped",
                    table.target
                ));
                return;
            }
        };
        let Some(first) = children.first() else { return };
        let what = kind(&first.rel_type).to_string();
        let table_xml = cx.pkg.read_limited(&table.target, MAX_META).ok().flatten().and_then(|b| xml::parse(&b).ok());
        let columns: Option<Vec<PreservedTableColumn>> = table_xml.as_ref().and_then(|x| x.child("tableColumns")).and_then(|tc| {
            tc.kids("tableColumn")
                .map(|c| {
                    Some(PreservedTableColumn {
                        name: xml::decode_xstring(c.attr("name").unwrap_or("")),
                        id: c.attr_u32("id")?,
                        query_field_id: c.attr_u32("queryTableFieldId"),
                    })
                })
                .collect()
        });
        let Some((x, columns)) = table_xml.as_ref().zip(columns) else {
            cx.warn(format!("sheet \"{sheet}\": {what} of {} was dropped: the table's columns are not numbered", table.target));
            return;
        };
        // The model's table, found by its name: the model numbers its tables itself.
        let file_name = x.attr("displayName").or_else(|| x.attr("name")).unwrap_or("Table");
        let Some(model) = tables.iter().find(|t| t.name == file_name) else { return };
        let mut q = Preserved::default();
        if let Err(why) = self.keep(cx, &mut q, &children, &mut HashSet::new()) {
            cx.warn(format!("sheet \"{sheet}\": {what} of {} was dropped: {why}", table.target));
            return;
        }
        p.tables.push(PreservedTable {
            id: model.id,
            table_type: x.attr("tableType").map(str::to_string),
            connection_id: x.attr_u32("connectionId"),
            columns,
            rels: q.rels,
            parts: q.parts,
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn sheet_elements(
        &mut self,
        cx: &mut Ctx<'_>,
        part: &str,
        name: &str,
        rels: &[PreservedRel],
        p: &mut Preserved,
        seen: &mut HashSet<String>,
        controls: &crate::controls::ReadControls,
    ) {
        let read_controls = &controls.modelled;
        let scan = match cx.pkg.read(part) {
            Ok(Some(bytes)) => scan_sheet(&bytes),
            _ => None,
        };
        let Some(scan) = scan else {
            cx.warn(format!("sheet \"{name}\": controls, embedded objects and custom views could not be read and were dropped"));
            return;
        };
        for slot in &scan.oversized {
            cx.warn(format!("sheet \"{name}\": {slot} was dropped: it is too large"));
        }
        // Printer settings (a binary part per sheet) hang off <pageSetup r:id>.
        if let Some(id) = scan.page_setup.as_deref()
            && let Some(r) = rels.iter().find(|r| r.id == id && is_kind(r, &["printerSettings"]))
        {
            match self.keep(cx, p, std::slice::from_ref(r), seen) {
                Ok(()) => p.elements.push(PreservedXml { slot: "pageSetup".into(), xml: r.id.clone() }),
                Err(why) => cx.warn(format!("sheet \"{name}\": printer settings were dropped: {why}")),
            }
        }
        // Header and footer pictures (`&G`): a VML drawing of their own and the images it shows,
        // kept byte for byte (GridCraft writes the header text, not the pictures).
        if let Some(id) = scan.legacy_drawing_hf.as_deref()
            && scan.legacy_drawing.as_deref() != Some(id)
            && let Some(r) = rels.iter().find(|r| r.id == id && is_kind(r, &["vmlDrawing"]))
        {
            match self.keep_through(cx, p, std::slice::from_ref(r), seen, &["vmlDrawing"]) {
                Ok(()) => {
                    let xml = format!("<legacyDrawingHF xmlns:r=\"{NS_REL}\" r:id=\"{}\"/>", esc_attr(&r.id));
                    p.elements.push(PreservedXml { slot: "legacyDrawingHF".into(), xml });
                    cx.warn(format!("sheet \"{name}\": pictures in the header and footer are not shown; they are kept for saving"));
                }
                Err(why) => cx.warn(format!("sheet \"{name}\": header and footer pictures were dropped: {why}")),
            }
        }
        // Form controls the sheet reader modelled are written from the model; the others stay.
        let mut frags: Vec<(String, String)> = scan
            .fragments
            .into_iter()
            .filter_map(|(slot, x)| {
                let x = self.kept_xml(x);
                if slot == "controls" { without_controls(&x, read_controls).map(|x| (slot, x)) } else { Some((slot, x)) }
            })
            .collect();
        // Controls and OLE objects are drawn from shapes in the legacy VML drawing, and so are the
        // check boxes, buttons and pictures that have no element in the sheet XML.
        let needs_vml = |slot: &str| matches!(slot, "oleObjects" | "controls");
        let wants_vml = frags.iter().any(|(slot, _)| needs_vml(slot));
        match self.vml(cx, rels, scan.legacy_drawing.as_deref(), name, seen, read_controls) {
            Ok(Some((vml, parts))) => {
                p.parts.extend(parts);
                p.vml = Some(vml);
            }
            Ok(None) if !wants_vml => {}
            Ok(None) => {
                let why = if scan.legacy_drawing.is_none() { "the sheet has no VML drawing" } else { "its VML drawing holds no shapes for them" };
                cx.warn(format!("sheet \"{name}\": controls and embedded objects were dropped: {why}"));
                frags.retain(|(slot, _)| !needs_vml(slot));
            }
            Err(why) => {
                cx.warn(format!("sheet \"{name}\": shapes of the legacy drawing (controls, buttons, pictures) were dropped: {why}"));
                frags.retain(|(slot, _)| !needs_vml(slot));
            }
        }
        for (slot, x) in frags {
            let mut ids = vec![];
            let ok = map_rel_ids(&x, &mut |id| {
                ids.push(id.to_string());
                Some(id.to_string())
            })
            .is_some();
            let roots: Option<Vec<PreservedRel>> = ids.iter().map(|id| rels.iter().find(|r| r.id == *id).cloned()).collect();
            let kept = match (ok, roots) {
                (true, Some(roots)) => self.keep(cx, p, &roots, seen),
                (false, _) => Err("its XML is damaged".into()),
                (_, None) => Err("it refers to a missing relationship".into()),
            };
            match kept {
                Ok(()) => p.elements.push(PreservedXml { slot, xml: x }),
                Err(why) => cx.warn(format!("sheet \"{name}\": {slot} were dropped: {why}")),
            }
        }
        if p.vml.is_some() || p.elements.iter().any(|e| e.slot == "oleObjects" || e.slot == "controls") {
            cx.warn(format!("sheet \"{name}\": embedded objects and form controls are not shown; they are kept for saving"));
        }
    }

    /// Whether the VML drawing `target` draws more than notes (form controls, buttons,
    /// pictures…). Answered once per part. A drawing that can't be read counts as drawing more, so
    /// that reading it for real reports why it is lost.
    fn vml_has_objects(&mut self, cx: &mut Ctx<'_>, target: &str) -> Result<bool, String> {
        let key = normalize(target);
        if let Some(known) = self.vml_objects.get(&key) {
            return known.clone();
        }
        let answer = match cx.pkg.read_limited(target, MAX_META) {
            Ok(Some(d)) => Ok(has_object_shapes(&String::from_utf8_lossy(&xml::to_utf8(&d)))),
            Ok(None) => Ok(false),
            Err(IoError::TooLarge(_)) => Err("it is too large".to_string()),
            Err(e) => Err(e.to_string()),
        };
        self.vml_objects.insert(key, answer.clone());
        answer
    }

    /// The sheet's VML drawing without its notes and the form controls read into the model
    /// (GridCraft writes those), and the parts its shapes refer to (preview images). `None` when
    /// those are all it draws. A part that one of its shapes shows and that can't be kept is left
    /// out; the shape stays.
    fn vml(
        &mut self,
        cx: &mut Ctx<'_>,
        rels: &[PreservedRel],
        rid: Option<&str>,
        sheet: &str,
        seen: &mut HashSet<String>,
        read_controls: &HashSet<u64>,
    ) -> Result<Option<(PreservedPart, Vec<PreservedPart>)>, String> {
        let Some(rel) = rid.and_then(|id| rels.iter().find(|r| r.id == id && !r.external && is_kind(r, &["vmlDrawing"]))) else {
            return Ok(None);
        };
        if matches!(self.vml_has_objects(cx, &rel.target), Ok(false)) {
            return Ok(None);
        }
        // Charged to the kept total like any other part: many sheets can point at one highly
        // compressed VML drawing, and each keeps its own copy.
        if self.budget == 0 {
            return Err("it is too large".into());
        }
        let limit = MAX_META.min(u64::try_from(self.budget).unwrap_or(u64::MAX));
        let data = match cx.pkg.read_limited(&rel.target, limit) {
            Ok(Some(d)) => d,
            Ok(None) => return Err(format!("{} is missing", rel.target)),
            Err(IoError::TooLarge(_)) => return Err("it is too large".into()),
            Err(e) => return Err(e.to_string()),
        };
        let raw_len = data.len();
        let text = String::from_utf8(data).map_err(|_| "the VML drawing is not UTF-8".to_string())?;
        let text = strip_shapes(&text, read_controls);
        if !has_object_shapes(&text) {
            return Ok(None);
        }
        // GridCraft's notes are merged in before the closing tag.
        if !text.contains("</xml>") {
            return Err("the VML drawing is damaged".into());
        }
        self.budget = self.budget.saturating_sub(raw_len);
        let vrels = raw_rels(cx, &rel.target)?;
        let mut q = Preserved::default();
        for r in vrels.iter().filter(|r| !r.external) {
            if let Err(why) = self.keep(cx, &mut q, std::slice::from_ref(r), seen) {
                cx.warn(format!("sheet \"{sheet}\": {} {} of the legacy drawing was dropped: {why}", kind(&r.rel_type), r.target));
            }
        }
        let vml = PreservedPart { name: rel.target.clone(), content_type: CT_VML.into(), data: Arc::from(text.into_bytes()), rels: vrels };
        Ok(Some((vml, q.parts)))
    }
}

/// What a sheet part holds for pass-through.
struct SheetScan {
    /// (slot, XML with the namespace declarations it needs).
    fragments: Vec<(String, String)>,
    legacy_drawing: Option<String>,
    /// The relationship id on `<legacyDrawingHF>` (header and footer pictures).
    legacy_drawing_hf: Option<String>,
    /// The relationship id on `<pageSetup>` (printer settings).
    page_setup: Option<String>,
    /// Kept elements that were left out because they are too large.
    oversized: Vec<String>,
}

/// Finds the sheet's `<picture>`, `<oleObjects>`, `<controls>` (also inside
/// `<mc:AlternateContent>`), `<customProperties>`, `<customSheetViews>` and the `<legacyDrawing>`
/// and `<legacyDrawingHF>` ids. `None` when the XML is damaged.
fn scan_sheet(bytes: &[u8]) -> Option<SheetScan> {
    let data = xml::to_utf8(bytes);
    let text = std::str::from_utf8(&data).ok()?;
    let mut reader = Reader::from_str(text);
    let mut decls: Vec<(String, String)> = vec![];
    let mut out = SheetScan { fragments: vec![], legacy_drawing: None, legacy_drawing_hf: None, page_setup: None, oversized: vec![] };
    let mut in_root = false;
    loop {
        let start = usize::try_from(reader.buffer_position()).ok()?;
        match reader.read_event().ok()? {
            Event::Eof => break,
            Event::Start(e) if !in_root => {
                in_root = true;
                decls = namespace_decls(&e, crate::write::NS_MAIN);
            }
            Event::Start(e) => {
                let local = e.local_name().as_ref().to_vec();
                match local.as_slice() {
                    b"pageSetup" => out.page_setup = rel_id_attr(&e),
                    b"legacyDrawing" => out.legacy_drawing = rel_id_attr(&e),
                    b"legacyDrawingHF" => out.legacy_drawing_hf = rel_id_attr(&e),
                    _ => {}
                }
                reader.read_to_end(e.name()).ok()?;
                let end = usize::try_from(reader.buffer_position()).ok()?;
                let wanted = matches!(
                    local.as_slice(),
                    b"picture" | b"oleObjects" | b"controls" | b"customProperties" | b"customSheetViews" | b"AlternateContent"
                );
                if wanted && end.saturating_sub(start) <= MAX_FRAGMENT {
                    let frag = text.get(start..end)?;
                    push_fragment(&mut out, frag, &local, &decls);
                } else if wanted {
                    out.oversized.push(String::from_utf8_lossy(&local).into_owned());
                }
            }
            Event::Empty(e) if in_root => {
                let local = e.local_name().as_ref().to_vec();
                if local == b"legacyDrawing" || local == b"legacyDrawingHF" || local == b"pageSetup" {
                    let id = rel_id_attr(&e);
                    match local.as_slice() {
                        b"pageSetup" => out.page_setup = id,
                        b"legacyDrawing" => out.legacy_drawing = id,
                        _ => out.legacy_drawing_hf = id,
                    }
                } else if local == b"picture"
                    && let Some(frag) = text.get(start..usize::try_from(reader.buffer_position()).ok()?)
                {
                    push_fragment(&mut out, frag, &local, &decls);
                }
            }
            Event::End(_) => break,
            _ => {}
        }
    }
    Some(out)
}

/// The namespace declarations of a root element that its children inherit: the `xmlns:prefix`
/// ones, and the default namespace under the empty prefix (the empty name when the root has
/// none) unless it is `dest_default`, the default namespace of the part the children are written
/// into. Strict and Transitional spellings of a namespace are the same namespace here: kept XML
/// is renamed to Transitional.
fn namespace_decls(e: &BytesStart<'_>, dest_default: &str) -> Vec<(String, String)> {
    let mut decls = vec![];
    let mut default = String::new();
    for a in e.attributes().with_checks(false).flatten() {
        let Ok(v) = std::str::from_utf8(&a.value) else { continue };
        if v.contains('"') {
            continue;
        }
        if a.key.as_ref() == b"xmlns" {
            default = v.to_string();
        } else if let Some(p) = a.key.as_ref().strip_prefix(b"xmlns:")
            && let Ok(p) = std::str::from_utf8(p)
        {
            decls.push((p.to_string(), v.to_string()));
        }
    }
    if strict::transitional(&default) != dest_default {
        decls.push((String::new(), default));
    }
    decls
}

/// The first child of the root element of the XML part `bytes` whose local name is `name`, as text
/// with the namespace declarations of the root that it needs. `None` when there is no such child,
/// it is larger than [`MAX_FRAGMENT`] or the XML is damaged.
fn root_child(bytes: &[u8], name: &str) -> Option<String> {
    let data = xml::to_utf8(bytes);
    let text = std::str::from_utf8(&data).ok()?;
    let mut reader = Reader::from_str(text);
    let mut decls: Vec<(String, String)> = vec![];
    let mut in_root = false;
    loop {
        let start = usize::try_from(reader.buffer_position()).ok()?;
        match reader.read_event().ok()? {
            Event::Eof | Event::End(_) => return None,
            Event::Start(e) if !in_root => {
                in_root = true;
                decls = namespace_decls(&e, crate::write::NS_MAIN);
            }
            Event::Start(e) => {
                let found = e.local_name().as_ref() == name.as_bytes();
                reader.read_to_end(e.name()).ok()?;
                if found {
                    let end = usize::try_from(reader.buffer_position()).ok()?;
                    if end.saturating_sub(start) > MAX_FRAGMENT {
                        return None;
                    }
                    return Some(with_decls(text.get(start..end)?, &decls));
                }
            }
            _ => {}
        }
    }
}

/// The `r:id` of an element (an `id` attribute with a namespace prefix).
fn rel_id_attr(e: &BytesStart<'_>) -> Option<String> {
    e.attributes()
        .with_checks(false)
        .flatten()
        .find(|a| a.key.local_name().as_ref() == b"id" && a.key.prefix().is_some())
        .and_then(|a| String::from_utf8(a.value.to_vec()).ok())
}

fn push_fragment(out: &mut SheetScan, frag: &str, local: &[u8], decls: &[(String, String)]) {
    let frag = with_decls(frag, decls);
    let slot = if local == b"AlternateContent" {
        // The kept element is the one the first choice holds.
        let Ok(el) = xml::parse(frag.as_bytes()) else { return };
        match el.child("Choice").and_then(|c| c.children.first()) {
            Some(c) => c.name.clone(),
            None => return,
        }
    } else {
        String::from_utf8_lossy(local).into_owned()
    };
    if is_sheet_slot(&slot) && !out.fragments.iter().any(|(s, _)| *s == slot) {
        out.fragments.push((slot, frag));
    }
}

/// The children of the root element of the XML part `bytes` whose local names aren't in `skip`,
/// each as text with the namespace declarations of the root that it needs, other than the ones in
/// `known` and the default namespace `dest_default` (the part they are written into declares
/// those). `None` when the XML is damaged, or a child is larger than [`MAX_FRAGMENT`], or there
/// are more than [`MAX_PROPERTY_ELEMENTS`].
fn root_children_except(bytes: &[u8], skip: &[&str], known: &[(&str, &str)], dest_default: &str) -> Option<Vec<String>> {
    let data = xml::to_utf8(bytes);
    let text = std::str::from_utf8(&data).ok()?;
    let mut reader = Reader::from_str(text);
    let mut decls: Vec<(String, String)> = vec![];
    let mut in_root = false;
    let mut out: Vec<String> = vec![];
    loop {
        let start = usize::try_from(reader.buffer_position()).ok()?;
        let kept = match reader.read_event().ok()? {
            Event::Eof | Event::End(_) => return Some(out),
            Event::Start(e) if !in_root => {
                in_root = true;
                decls = namespace_decls(&e, dest_default).into_iter().filter(|(p, v)| !known.iter().any(|(kp, kv)| kp == p && kv == v)).collect();
                continue;
            }
            Event::Empty(_) if !in_root => return Some(out),
            Event::Start(e) => {
                let skipped = skip.iter().any(|s| e.local_name().as_ref() == s.as_bytes());
                reader.read_to_end(e.name()).ok()?;
                !skipped
            }
            Event::Empty(e) => !skip.iter().any(|s| e.local_name().as_ref() == s.as_bytes()),
            _ => continue,
        };
        if kept {
            let end = usize::try_from(reader.buffer_position()).ok()?;
            if end.saturating_sub(start) > MAX_FRAGMENT || out.len() >= MAX_PROPERTY_ELEMENTS {
                return None;
            }
            let frag = text.get(start..end)?;
            let used: Vec<(String, String)> = decls.iter().filter(|(p, _)| uses_prefix(frag, p)).cloned().collect();
            out.push(with_decls(frag, &used));
        }
    }
}

/// Whether the XML `frag` uses the namespace prefix `p`: in an element or attribute name, or as
/// the prefix of a qualified name in an attribute value. The empty prefix is the default
/// namespace, which only element names without a prefix use.
fn uses_prefix(frag: &str, p: &str) -> bool {
    if p.is_empty() {
        return uses_default(frag);
    }
    ["<", "</", " ", "\t", "\n", "\r", "\"", "'"].iter().any(|before| frag.contains(&format!("{before}{p}:")))
}

/// Whether an element of the XML `frag` has a name without a prefix.
fn uses_default(frag: &str) -> bool {
    frag.match_indices('<').any(|(i, _)| {
        let name = frag[i + 1..].split(|c: char| c.is_whitespace() || c == '/' || c == '>').next().unwrap_or("");
        !name.is_empty() && !name.starts_with(['!', '?']) && !name.contains(':')
    })
}

/// Whether the start tag `head` declares the namespace prefix `p` (the default one when empty).
fn declares(head: &str, p: &str) -> bool {
    let attr = if p.is_empty() { "xmlns".to_string() } else { format!("xmlns:{p}") };
    head.match_indices(&attr).any(|(i, _)| {
        let before = head[..i].chars().next_back().is_some_and(char::is_whitespace);
        before && head[i + attr.len()..].trim_start().starts_with('=')
    })
}

/// Adds the root's namespace declarations a fragment doesn't declare itself: each prefix, and the
/// default namespace (the empty prefix) when the fragment has elements in it.
fn with_decls(frag: &str, decls: &[(String, String)]) -> String {
    let Some(name_end) = frag.char_indices().skip(1).find(|(_, c)| c.is_whitespace() || *c == '/' || *c == '>').map(|(i, _)| i) else {
        return frag.to_string();
    };
    let head = frag.get(..frag.find('>').unwrap_or(frag.len())).unwrap_or("");
    let mut add = String::new();
    for (p, v) in decls {
        if declares(head, p) {
            continue;
        }
        if p.is_empty() {
            if uses_default(frag) {
                let _ = write!(add, " xmlns=\"{v}\"");
            }
        } else {
            let _ = write!(add, " xmlns:{p}=\"{v}\"");
        }
    }
    format!("{}{add}{}", frag.get(..name_end).unwrap_or(""), frag.get(name_end..).unwrap_or(""))
}

/// Byte ranges of the `<v:shape …>…</v:shape>` (or self-closed) elements in VML text.
fn shape_spans(vml: &str) -> Vec<(usize, usize)> {
    let mut spans = vec![];
    let mut from = 0;
    while let Some(i) = vml.get(from..).and_then(|s| s.find("<v:shape")).map(|i| i + from) {
        let after = i + "<v:shape".len();
        let is_shape = vml.get(after..).and_then(|s| s.chars().next()).is_some_and(|c| c.is_whitespace() || c == '>' || c == '/');
        if !is_shape {
            from = after;
            continue;
        }
        let Some(tag_end) = vml.get(i..).and_then(|s| s.find('>')).map(|j| i + j + 1) else { break };
        let end = if vml.get(..tag_end).is_some_and(|t| t.ends_with("/>")) {
            tag_end
        } else {
            match vml.get(tag_end..).and_then(|s| s.find("</v:shape>")) {
                Some(j) => tag_end + j + "</v:shape>".len(),
                None => break,
            }
        };
        spans.push((i, end));
        from = end;
    }
    spans
}

/// Whether a `<v:shape>` is a note: its client data's `ObjectType` is `Note`
/// ([`crate::controls::is_note_data`]), as the control reader judges it, in any case and either
/// quote style. The client data's tag is read on its own ([`xml::first_start`]), so a caption's
/// unclosed `<br>` doesn't hide it and text elsewhere in the shape never makes a shape a note.
fn is_note_shape(shape: &str) -> bool {
    xml::first_start(shape.as_bytes(), "ClientData").is_some_and(|d| crate::controls::is_note_data(&d))
}

/// Whether VML text draws anything but notes: a form control, a button, a picture…
fn has_object_shapes(vml: &str) -> bool {
    shape_spans(vml).into_iter().any(|(a, b)| !is_note_shape(vml.get(a..b).unwrap_or("")))
}

/// VML without its note shapes and the shapes numbered in `drop` (each shape's number from its
/// `id`, else its `o:spid`: [`crate::controls::number_of`]). A shape with no number that isn't a
/// note is kept.
fn strip_shapes(vml: &str, drop: &HashSet<u64>) -> String {
    let mut out = String::with_capacity(vml.len());
    let mut last = 0;
    for (a, b) in shape_spans(vml) {
        let shape = vml.get(a..b).unwrap_or("");
        let numbered = !drop.is_empty()
            && xml::first_start(shape.as_bytes(), "shape").and_then(|s| crate::controls::number_of(&s)).is_some_and(|n| drop.contains(&n));
        if numbered || is_note_shape(shape) {
            out.push_str(vml.get(last..a).unwrap_or(""));
            last = b;
        }
    }
    out.push_str(vml.get(last..).unwrap_or(""));
    out
}

/// The number in the element `<tag>…</tag>` of `s` (`<x:Row>48</x:Row>`).
fn tag_number(s: &str, tag: &str) -> Option<u32> {
    let open = format!("<{tag}>");
    let from = s.find(&open)? + open.len();
    s.get(from..)?.split('<').next()?.trim().parse().ok()
}

/// The byte range of the `<v:fill>` element of a shape: self-closed, or up to `</v:fill>`.
fn fill_span(shape: &str) -> Option<(usize, usize)> {
    let mut from = 0;
    loop {
        let i = from + shape.get(from..)?.find("<v:fill")?;
        let after = i + "<v:fill".len();
        if !shape.get(after..)?.chars().next().is_some_and(|c| c.is_whitespace() || c == '>' || c == '/') {
            from = after;
            continue;
        }
        let tag_end = i + shape.get(i..)?.find('>')? + 1;
        if shape.get(..tag_end)?.ends_with("/>") {
            return Some((i, tag_end));
        }
        let close = tag_end + shape.get(tag_end..)?.find("</v:fill>")? + "</v:fill>".len();
        return Some((i, close));
    }
}

/// The relationship ids (`o:relid="rId1"`) an element names.
pub(super) fn relids_in(xml: &str) -> Vec<String> {
    let mut ids: Vec<String> = vec![];
    for (i, _) in xml.match_indices("relid=") {
        let rest = xml.get(i + "relid=".len()..).unwrap_or("");
        let Some(quote) = rest.chars().next().filter(|c| matches!(c, '"' | '\'')) else { continue };
        if let Some(id) = rest.get(1..).and_then(|r| r.split(quote).next())
            && !ids.iter().any(|x| x == id)
        {
            ids.push(id.to_string());
        }
    }
    ids
}

/// The element with the attributes it repeats left out. VML from some producers repeats
/// `o:relid` for each use of a picture; XML allows an attribute once.
fn dedupe_attributes(element: &str) -> String {
    let mut reader = Reader::from_str(element);
    let mut writer = Writer::new(Vec::with_capacity(element.len()));
    loop {
        let written = match reader.read_event() {
            Ok(Event::Eof) => break,
            Ok(Event::Start(e)) => unique_attributes(&e).map(Event::Start),
            Ok(Event::Empty(e)) => unique_attributes(&e).map(Event::Empty),
            Ok(other) => Some(other),
            Err(_) => None,
        };
        let Some(event) = written else { return element.to_string() };
        if writer.write_event(event).is_err() {
            return element.to_string();
        }
    }
    String::from_utf8(writer.into_inner()).unwrap_or_else(|_| element.to_string())
}

fn unique_attributes(e: &BytesStart<'_>) -> Option<BytesStart<'static>> {
    let mut out = BytesStart::new(String::from_utf8(e.name().as_ref().to_vec()).ok()?);
    let mut seen: HashSet<Vec<u8>> = HashSet::new();
    for a in e.attributes().with_checks(false) {
        let a = a.ok()?;
        if seen.insert(a.key.as_ref().to_vec()) {
            out.push_attribute(Attribute { key: QName(a.key.as_ref()), value: a.value.clone() });
        }
    }
    Some(out)
}

/// The note shapes of VML text whose box is filled with a picture: the note's cell and its
/// `<v:fill>`.
fn note_fill_shapes(vml: &str) -> Vec<(CellRef, String)> {
    let mut out = vec![];
    for (a, b) in shape_spans(vml) {
        let shape = vml.get(a..b).unwrap_or("");
        if !is_note_shape(shape) {
            continue;
        }
        let (Some(row), Some(col)) = (tag_number(shape, "x:Row"), tag_number(shape, "x:Column")) else { continue };
        if row >= MAX_ROWS || col >= MAX_COLS {
            continue;
        }
        let fill = fill_span(shape).and_then(|(i, j)| shape.get(i..j)).filter(|f| f.contains("relid="));
        if let Some(fill) = fill {
            out.push((CellRef::new(row, col), dedupe_attributes(fill)));
        }
        if out.len() >= MAX_NOTE_FILLS {
            break;
        }
    }
    out
}

/// `vml` with the picture fills of `fills` put into the shapes of the notes at their cells; the
/// fills that went in.
pub(super) fn apply_note_fills<'a>(vml: &str, fills: &'a [PreservedNoteFill]) -> (String, Vec<&'a PreservedNoteFill>) {
    let by_cell: HashMap<CellRef, &PreservedNoteFill> = fills.iter().map(|f| (f.cell, f)).collect();
    let mut out = String::with_capacity(vml.len());
    let mut used = vec![];
    let mut last = 0;
    for (a, b) in shape_spans(vml) {
        let shape = vml.get(a..b).unwrap_or("");
        if !is_note_shape(shape) {
            continue;
        }
        let (Some(row), Some(col)) = (tag_number(shape, "x:Row"), tag_number(shape, "x:Column")) else { continue };
        let Some(fill) = by_cell.get(&CellRef::new(row, col)) else { continue };
        let Some((i, j)) = fill_span(shape) else { continue };
        out.push_str(vml.get(last..a + i).unwrap_or(""));
        out.push_str(&fill.xml);
        last = a + j;
        used.push(*fill);
    }
    out.push_str(vml.get(last..).unwrap_or(""));
    (out, used)
}

/// Shape numbers (`_x0000_s1025` → 1025) used in VML text.
fn shape_numbers(vml: &str) -> Vec<u64> {
    vml.match_indices("_x0000_s")
        .filter_map(|(i, m)| {
            let digits: String = vml.get(i + m.len()..)?.chars().take_while(char::is_ascii_digit).take(18).collect();
            digits.parse().ok()
        })
        .collect()
}

/// The highest shape block VML text claims in `<o:idmap data>` or uses in a shape id.
pub(super) fn vml_block_max(vml: &str) -> u64 {
    let used = shape_numbers(vml).into_iter().max().unwrap_or(0) / 1024;
    let claimed = vml
        .find("<o:idmap")
        .and_then(|i| vml.get(i..)?.find("data=\"").map(|d| i + d + "data=\"".len()))
        .and_then(|d| Some((d, d + vml.get(d..)?.find('"')?)))
        .and_then(|(d, e)| vml.get(d..e))
        .map(|list| list.split(',').filter_map(|b| b.trim().parse::<u64>().ok()).max().unwrap_or(0))
        .unwrap_or(0);
    used.max(claimed)
}

/// The first shape block free in kept VML: GridCraft's own shapes (notes, then form controls) are
/// numbered from `block * 1024 + 1` when they join it ([`merge_vml`]).
pub(super) fn first_free_block(kept: &str) -> u64 {
    shape_numbers(kept).into_iter().max().unwrap_or(0) / 1024 + 1
}

/// Kept VML (controls, OLE objects) plus GridCraft's note and form control shapes: ours get ids
/// in fresh shape blocks so they can't collide with the kept shapes the sheet XML refers to.
pub(super) fn merge_vml(kept: &str, ours: &str) -> String {
    let Some(close) = kept.rfind("</xml>") else { return ours.to_string() };
    let block = first_free_block(kept);
    let mut add = String::new();
    // Our shape types (notes, form controls) that the kept drawing doesn't define.
    let mut from = 0;
    while let Some(i) = ours.get(from..).and_then(|s| s.find("<v:shapetype")).map(|i| i + from) {
        let Some(j) = ours.get(i..).and_then(|s| s.find("</v:shapetype>")).map(|j| i + j + "</v:shapetype>".len()) else { break };
        let ty = ours.get(i..j).unwrap_or("");
        let id = attr_value(ty, "id").unwrap_or("");
        if !id.is_empty() && !kept.contains(&format!("id=\"{id}\"")) && !kept.contains(&format!("id='{id}'")) {
            add.push_str(ty);
        }
        from = j;
    }
    let spans = shape_spans(ours);
    let blocks = (spans.len() as u64 + 1).div_ceil(1024).max(1);
    for (k, (a, b)) in spans.into_iter().enumerate() {
        let shape = ours.get(a..b).unwrap_or("");
        let id = block.saturating_mul(1024).saturating_add(1 + k as u64);
        match (shape.find("id=\"_x0000_s"), shape.find("id=\"_x0000_s").and_then(|i| shape.get(i + 4..)?.find('"').map(|j| i + 4 + j))) {
            (Some(i), Some(j)) => {
                let _ = write!(add, "{}id=\"_x0000_s{id}\"{}", shape.get(..i).unwrap_or(""), shape.get(j + 1..).unwrap_or(""));
            }
            _ => add.push_str(shape),
        }
    }
    let mut head = kept.get(..close).unwrap_or("").to_string();
    // Claim the blocks in the drawing's id map.
    if let Some(i) = head.find("<o:idmap")
        && let Some(d) = head.get(i..).and_then(|s| s.find("data=\"")).map(|d| i + d + "data=\"".len())
        && let Some(e) = head.get(d..).and_then(|s| s.find('"')).map(|e| d + e)
    {
        let claimed: String = (block..block.saturating_add(blocks)).map(|b| format!(",{b}")).collect();
        head.insert_str(e, &claimed);
    }
    format!("{head}{add}{}", kept.get(close..).unwrap_or(""))
}

/// The value of the first attribute `name` in XML text `x`, in either quote style (`name="v"` or
/// `name='v'`), when it has one. The name must start a token (`id` is not the end of `spid`).
fn attr_value<'a>(x: &'a str, name: &str) -> Option<&'a str> {
    let mut from = 0;
    while let Some(i) = x.get(from..)?.find(name).map(|i| i + from) {
        from = i + name.len();
        let starts_token = x.get(..i).is_some_and(|before| before.ends_with(|c: char| c.is_whitespace() || c == '<'));
        let Some(rest) = x.get(from..).map(str::trim_start).and_then(|r| r.strip_prefix('=')).map(str::trim_start) else { continue };
        if !starts_token {
            continue;
        }
        if let Some(q) = rest.chars().next().filter(|q| *q == '"' || *q == '\'') {
            let body = rest.get(q.len_utf8()..)?;
            return body.find(q).and_then(|e| body.get(..e));
        }
    }
    None
}

/// A kept `<controls>` fragment without the controls whose `shapeId` is in `drop` (they are
/// written from the model): a bare `<control>` child goes when it is one of them, and within an
/// `mc:AlternateContent` child only those controls go, in each of its branches, the wrapper
/// staying while a control remains. `None` when nothing is left. A fragment that doesn't parse
/// is returned unchanged.
fn without_controls(frag: &str, drop: &HashSet<u64>) -> Option<String> {
    if drop.is_empty() {
        return Some(frag.to_string());
    }
    let mut reader = Reader::from_str(frag);
    let mut out = String::with_capacity(frag.len());
    let (mut last, mut left, mut inside) = (0usize, 0usize, false);
    loop {
        let Ok(start) = usize::try_from(reader.buffer_position()) else { return Some(frag.to_string()) };
        let ev = match reader.read_event() {
            Ok(ev) => ev,
            Err(_) => return Some(frag.to_string()),
        };
        let child = match ev {
            Event::Eof => break,
            Event::Start(e) if e.local_name().as_ref() == b"controls" => {
                inside = true;
                continue;
            }
            Event::End(e) if e.local_name().as_ref() == b"controls" => {
                inside = false;
                continue;
            }
            Event::Start(e) if inside => {
                if reader.read_to_end(e.name()).is_err() {
                    return Some(frag.to_string());
                }
                true
            }
            Event::Empty(_) if inside => true,
            _ => false,
        };
        if !child {
            continue;
        }
        let Ok(end) = usize::try_from(reader.buffer_position()) else { return Some(frag.to_string()) };
        let text = frag.get(start..end).unwrap_or("");
        let Some((kept, controls, dropped)) = controls_kept(text, drop) else { return Some(frag.to_string()) };
        if dropped == 0 {
            // Nothing of ours in it: the child stays as it is.
            left += 1;
        } else {
            out.push_str(frag.get(last..start).unwrap_or(""));
            if controls > 0 {
                out.push_str(&kept);
                left += 1;
            }
            last = end;
        }
    }
    out.push_str(frag.get(last..).unwrap_or(""));
    (left > 0).then_some(out)
}

/// One child of a `<controls>` element with the `<control>` elements numbered in `drop` taken
/// out, wherever they are in it (a bare control, or the Choice and Fallback branches of an
/// `mc:AlternateContent`), as (the text, the controls that remain, the controls taken out). The
/// `shapeId` is read as the control reader reads it (an attribute, in either quote style). `None`
/// when the child doesn't parse.
fn controls_kept(child: &str, drop: &HashSet<u64>) -> Option<(String, usize, usize)> {
    let mut reader = Reader::from_str(child);
    let mut out = String::with_capacity(child.len());
    let (mut last, mut kept, mut dropped) = (0usize, 0usize, 0usize);
    loop {
        let start = usize::try_from(reader.buffer_position()).ok()?;
        let (e, has_end) = match reader.read_event().ok()? {
            Event::Eof => break,
            Event::Start(e) => (e, true),
            Event::Empty(e) => (e, false),
            _ => continue,
        };
        if e.local_name().as_ref() != b"control" {
            // A wrapper or a branch: its controls are the events that follow.
            continue;
        }
        let number =
            e.try_get_attribute("shapeId").ok().flatten().and_then(|a| std::str::from_utf8(&a.value).ok().and_then(crate::controls::shape_number));
        if has_end {
            reader.read_to_end(e.name()).ok()?;
        }
        let end = usize::try_from(reader.buffer_position()).ok()?;
        if number.is_some_and(|n| drop.contains(&n)) {
            out.push_str(child.get(last..start)?);
            last = end;
            dropped += 1;
        } else {
            kept += 1;
        }
    }
    out.push_str(child.get(last..)?);
    Some((out, kept, dropped))
}

/// Rewrites relationship-id attributes (`r:id`, `r:embed`…: any attribute in the relationships
/// namespace) of an XML fragment through `map`. `None` when the fragment is damaged or `map` has
/// no id for one of them.
pub fn map_rel_ids(x: &str, map: &mut dyn FnMut(&str) -> Option<String>) -> Option<String> {
    let mut reader = Reader::from_str(x);
    let mut writer = Writer::new(Vec::with_capacity(x.len()));
    let mut prefixes: HashSet<Vec<u8>> = HashSet::new();
    let mut depth = 0usize;
    loop {
        match reader.read_event().ok()? {
            Event::Eof => break,
            Event::Start(e) => {
                depth += 1;
                if depth > MAX_FRAGMENT_DEPTH {
                    return None;
                }
                let e = map_start(&e, &mut prefixes, map)?;
                writer.write_event(Event::Start(e)).ok()?;
            }
            Event::Empty(e) => {
                let e = map_start(&e, &mut prefixes, map)?;
                writer.write_event(Event::Empty(e)).ok()?;
            }
            Event::End(e) => {
                depth = depth.saturating_sub(1);
                writer.write_event(Event::End(e)).ok()?;
            }
            ev => writer.write_event(ev).ok()?,
        }
    }
    String::from_utf8(writer.into_inner()).ok()
}

fn map_start(e: &BytesStart<'_>, prefixes: &mut HashSet<Vec<u8>>, map: &mut dyn FnMut(&str) -> Option<String>) -> Option<BytesStart<'static>> {
    for a in e.attributes().with_checks(false) {
        let a = a.ok()?;
        if let Some(p) = a.key.as_ref().strip_prefix(b"xmlns:")
            && (a.value.as_ref() == NS_REL.as_bytes() || a.value.as_ref() == NS_REL_STRICT.as_bytes())
        {
            prefixes.insert(p.to_vec());
        }
    }
    let name = String::from_utf8(e.name().as_ref().to_vec()).ok()?;
    let mut out = BytesStart::new(name);
    for a in e.attributes().with_checks(false) {
        let a = a.ok()?;
        let key = a.key.as_ref();
        let is_rel = !key.starts_with(b"xmlns") && a.key.prefix().is_some_and(|p| prefixes.contains(p.as_ref()));
        if is_rel {
            let old = std::str::from_utf8(&a.value).ok()?;
            let new = map(old)?;
            out.push_attribute(Attribute { key: QName(key), value: Cow::Owned(esc_attr(&new).into_bytes()) });
        } else {
            out.push_attribute(Attribute { key: QName(key), value: a.value.clone() });
        }
    }
    Some(out)
}

/// Notes the cells of [`Preserved::cell_meta`] as the workbook now holds them (value and formula),
/// once every sheet is read and settled: the value metadata describes that value, and is written
/// back only while the cell still holds it. A cell that is not there has no metadata to keep.
pub fn seal_cell_meta(wb: &mut Workbook) {
    for sheet in &mut wb.sheets {
        let Some(sheet) = Arc::get_mut(sheet) else { continue };
        let cells = &sheet.cells;
        let Some(p) = sheet.preserved.as_mut().and_then(Arc::get_mut) else { continue };
        p.cell_meta.retain_mut(|m| match cells.get(m.cell) {
            Some(c) => {
                m.value = c.value.scalar();
                m.formula = c.formula.as_ref().map(|f| f.text.clone());
                true
            }
            None => false,
        });
    }
}

/// Whether entry 1 of the cell metadata of `m` (the part's root) is the dynamic-array flag: a
/// record of the `XLDAPR` type pointing at future metadata that says `fDynamic`. GridCraft's own
/// `cm="1"` means exactly that.
fn first_cell_metadata_is_dynamic(m: &xml::El) -> bool {
    let types: Vec<&str> =
        m.child("metadataTypes").map(|t| t.kids("metadataType").map(|e| e.attr("name").unwrap_or("")).collect()).unwrap_or_default();
    let Some(bk) = m.child("cellMetadata").and_then(|c| c.kids("bk").next()) else { return false };
    let mut records = bk.kids("rc");
    let (Some(rc), None) = (records.next(), records.next()) else { return false };
    let (Some(t), Some(v)) = (rc.attr_u32("t"), rc.attr_u32("v")) else { return false };
    let named = usize::try_from(t).ok().and_then(|t| t.checked_sub(1)).and_then(|i| types.get(i));
    if named != Some(&"XLDAPR") {
        return false;
    }
    let blocks = m.kids("futureMetadata").filter(|f| f.attr("name") == Some("XLDAPR")).flat_map(|f| f.kids("bk"));
    blocks
        .into_iter()
        .nth(usize::try_from(v).unwrap_or(usize::MAX))
        .and_then(|bk| bk.path(&["extLst", "ext", "dynamicArrayProperties"]))
        .is_some_and(|d| d.attr_bool("fDynamic") == Some(true) && d.attr_bool("fCollapsed") != Some(true))
}
