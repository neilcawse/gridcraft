use super::read::{map_rel_ids, vml_block_max};
use super::*;

// ---------------------------------------------------------------- writing

/// Names given to kept parts while a package is written.
#[derive(Default)]
pub struct KeptOut {
    used: HashSet<String>,
    n: u32,
    /// The highest VML shape block a kept header and footer drawing uses: notes go after it.
    vml_blocks: u32,
    /// Package-root relationships (type, target, external), added when `_rels/.rels` is written.
    root: Vec<(String, String, bool)>,
    /// Whether the kept cell and value metadata is written (see [`KeptOut::use_metadata`]).
    metadata: bool,
}

impl KeptOut {
    /// Reserves a part name GridCraft writes itself, so kept parts don't take it. False when it is
    /// taken.
    pub fn reserve(&mut self, name: &str) -> bool {
        self.used.insert(name.to_ascii_lowercase())
    }

    /// Reserves the VML shape blocks the kept header and footer drawings of `wb` use (and those of
    /// its kept dialog sheets), so that the notes drawings GridCraft writes don't reuse their shape
    /// ids.
    pub fn reserve_vml_blocks(&mut self, wb: &Workbook) {
        let owners = wb.sheets.iter().filter_map(|s| s.preserved.as_deref()).chain(wb.preserved.as_deref());
        let vmls = owners.flat_map(|p| &p.parts).filter(|q| q.content_type.eq_ignore_ascii_case(CT_VML));
        let max = vmls.map(|q| vml_block_max(&String::from_utf8_lossy(&q.data))).max().unwrap_or(0);
        self.vml_blocks = u32::try_from(max).unwrap_or(u32::MAX);
    }

    /// The shape block of the `n`th notes drawing.
    pub fn notes_block(&self, n: u32) -> u32 {
        n.saturating_add(self.vml_blocks)
    }

    /// A part name for a kept part: its original name when that can't collide with anything
    /// written, otherwise a fresh one in the same folder.
    pub(super) fn claim(&mut self, original: &str) -> String {
        let lower = original.to_ascii_lowercase();
        let own = OWN_DIRS.iter().any(|d| lower.starts_with(d));
        if own && self.used.insert(lower) {
            return original.to_string();
        }
        let (dir, file) = package::split_dir(original);
        let (stem, ext) = match file.rsplit_once('.') {
            Some((s, e)) => (s, format!(".{e}")),
            None => (file, String::new()),
        };
        let stem = if own { format!("{stem}_") } else { "kept".to_string() };
        loop {
            self.n = self.n.saturating_add(1);
            let file = format!("{stem}{}{ext}", self.n);
            let cand = if dir.is_empty() { file } else { format!("{dir}/{file}") };
            if self.used.insert(cand.to_ascii_lowercase()) || self.n == u32::MAX {
                return cand;
            }
        }
    }
}

/// Original part name (normalized) → written part name.
pub(super) type NameMap = HashMap<String, String>;

fn rel_target(r: &PreservedRel, names: &NameMap) -> Option<String> {
    if r.external { Some(r.target.clone()) } else { names.get(&normalize(&r.target)).map(|n| format!("/{n}")) }
}

/// A kept part's relationships, ids unchanged, targets renamed.
pub(super) fn part_rels_xml(rels: &[PreservedRel], names: &NameMap) -> String {
    let mut s = format!("{XML_DECL}<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">");
    for r in rels {
        let Some(target) = rel_target(r, names) else { continue };
        let _ = write!(
            s,
            "<Relationship Id=\"{}\" Type=\"{}\" Target=\"{}\"{}/>",
            esc_attr(&r.id),
            esc_attr(&r.rel_type),
            esc_attr(&target),
            if r.external { " TargetMode=\"External\"" } else { "" }
        );
    }
    s.push_str("</Relationships>");
    s
}

pub(super) fn write_parts(out: &mut Out, parts: &[PreservedPart]) -> NameMap {
    let mut names = NameMap::new();
    for p in parts {
        names.entry(normalize(&p.name)).or_insert_with(|| out.kept.claim(&p.name));
    }
    let mut written = HashSet::new();
    for p in parts {
        let Some(new) = names.get(&normalize(&p.name)).cloned() else { continue };
        if !written.insert(new.clone()) {
            continue;
        }
        out.part(&new, Some(&p.content_type), p.data.to_vec());
        if !p.rels.is_empty() {
            out.part(&rels_part_name(&new), None, part_rels_xml(&p.rels, &names).into_bytes());
        }
    }
    names
}

/// Adds the owner's relationships; returns old id → new id.
pub(super) fn attach(rels: &mut Rels, list: &[PreservedRel], names: &NameMap) -> HashMap<String, String> {
    let mut ids = HashMap::new();
    for r in list {
        if let Some(target) = rel_target(r, names) {
            let id = rels.add_raw(&r.rel_type, &target, r.external);
            ids.insert(r.id.clone(), id);
        }
    }
    ids
}

pub(super) fn remap(x: &str, ids: &HashMap<String, String>) -> Option<String> {
    map_rel_ids(x, &mut |id| ids.get(id).cloned())
}

/// A kept chart sheet being written: its `<sheet>` entry's fields and its new relationship id.
pub struct KeptSheet {
    before: usize,
    name: String,
    visibility: Visibility,
    rid: String,
    sheet_id: u32,
}

/// The `sheetId`s of the written sheets: each keeps the one the file gave it, unless another
/// sheet took it first (a copied sheet) or it has none (a new sheet), and those get the next id
/// that no sheet asks for.
pub struct SheetIds {
    wanted: HashSet<u32>,
    given: HashSet<u32>,
    next: u32,
}

impl SheetIds {
    pub fn new(wb: &Workbook, kept: &WorkbookKept) -> SheetIds {
        let wanted: HashSet<u32> = wb.sheets.iter().map(|s| s.file_id).chain(kept.sheets.iter().map(|s| s.sheet_id)).filter(|&id| id != 0).collect();
        SheetIds { wanted, given: HashSet::new(), next: 1 }
    }

    pub fn take(&mut self, wanted: u32) -> u32 {
        if wanted != 0 && self.given.insert(wanted) {
            return wanted;
        }
        while self.wanted.contains(&self.next) || self.given.contains(&self.next) {
            self.next += 1;
        }
        self.given.insert(self.next);
        self.next
    }
}

/// The workbook's kept content as written.
#[derive(Default)]
pub struct WorkbookKept {
    /// Workbook XML elements that go right after `<sheets>` (`<externalReferences>`).
    pub elements: String,
    /// The `<definedName>` elements scoped to the kept sheets, with their sheets' positions.
    pub names: String,
    sheets: Vec<KeptSheet>,
}

impl WorkbookKept {
    /// Where the model's sheet `i` is in the file's sheet list: chart sheets come between worksheets.
    pub fn file_index(&self, i: usize) -> usize {
        i + self.sheets.iter().filter(|s| s.before <= i).count()
    }

    /// The `<sheet>` entries of the chart sheets that go right before the model's sheet `i`
    /// (`i` = the number of sheets for those that come last), with their ids from `ids`.
    pub fn sheet_entries(&self, i: usize, last: bool, ids: &mut SheetIds) -> String {
        let mut x = String::new();
        for s in self.sheets.iter().filter(|s| if last { s.before >= i } else { s.before == i }) {
            let state = match s.visibility {
                Visibility::Visible => "",
                Visibility::Hidden => " state=\"hidden\"",
                Visibility::VeryHidden => " state=\"veryHidden\"",
            };
            let name: String = s.name.chars().take(31).collect();
            let id = ids.take(s.sheet_id);
            let _ = write!(x, "<sheet name=\"{}\" sheetId=\"{id}\"{state} r:id=\"{}\"/>", esc_attr(&name), esc_attr(&s.rid));
        }
        x
    }
}

/// The part names reachable from `root` through kept parts' relationships (normalized).
fn reachable(p: &Preserved, root: &PreservedRel) -> HashSet<String> {
    reachable_in(&p.parts, root)
}

/// The names of the parts of `parts` reachable from `root` (normalized, `root`'s target included).
pub(super) fn reachable_in(parts: &[PreservedPart], root: &PreservedRel) -> HashSet<String> {
    let by_name: HashMap<String, &PreservedPart> = parts.iter().map(|q| (normalize(&q.name), q)).collect();
    let mut seen = HashSet::new();
    let mut queue = VecDeque::from([normalize(&root.target)]);
    while let Some(n) = queue.pop_front() {
        if !seen.insert(n.clone()) {
            continue;
        }
        if let Some(q) = by_name.get(&n) {
            queue.extend(q.rels.iter().filter(|r| !r.external).map(|r| normalize(&r.target)));
        }
    }
    seen
}

/// Every formula (`<c:f>`) of a chart part, depth first.
fn chart_formulas(el: &xml::El, out: &mut Vec<String>) {
    if el.name == "f" && !el.text.trim().is_empty() {
        out.push(el.text.trim().to_string());
    }
    for c in &el.children {
        chart_formulas(c, out);
    }
}

/// The sheet names a formula such as `'My Sheet'!$A$1:$A$9,Data!$B$1` refers to. References to
/// other workbooks (`[1]Sheet!A1`) are not sheets of this one and are left out, and neither are
/// the error literals that end in `!` (`#REF!` in `Data!#REF!`, a reference to deleted cells): an
/// unquoted sheet name never starts with `#`.
pub(crate) fn sheet_names_in(formula: &str) -> Vec<String> {
    let mut names = vec![];
    let chars: Vec<char> = formula.chars().collect();
    let at = |i: usize| chars.get(i).copied();
    let mut i = 0;
    while let Some(c) = at(i) {
        let mut name = String::new();
        let start = i;
        let quoted = c == '\'';
        if quoted {
            i += 1;
            while let Some(c) = at(i) {
                if c == '\'' {
                    if at(i + 1) == Some('\'') {
                        name.push('\'');
                        i += 2;
                        continue;
                    }
                    break;
                }
                name.push(c);
                i += 1;
            }
            i += 1;
        } else {
            while let Some(c) = at(i).filter(|c| !matches!(c, '!' | ',' | '(' | ')' | '\'' | ' ')) {
                name.push(c);
                i += 1;
            }
        }
        if at(i) == Some('!') {
            if !name.is_empty() && !name.starts_with('[') && (quoted || !name.starts_with('#')) {
                names.push(name);
            }
            i += 1;
        } else if i == start {
            i += 1;
        }
    }
    names
}

/// Whether two sheet names name the same sheet (Excel compares the first 31 characters, without
/// case).
fn same_sheet(a: &str, b: &str) -> bool {
    a.chars().take(31).collect::<String>().eq_ignore_ascii_case(&b.chars().take(31).collect::<String>())
}

/// The sheets plotted by the charts behind each kept sheet, by the normalized target of the
/// sheet's relationship: entries sharing one graph, under the same or different relationship ids,
/// are analysed once. A sheet whose relationship is missing has no entry.
fn chart_sources(p: &Preserved) -> HashMap<String, Vec<String>> {
    let mut sources = HashMap::new();
    for s in p.sheets.iter().filter(|s| s.kind != PreservedSheetKind::Module) {
        let Some(root) = p.rels.iter().find(|r| r.id == s.rel_id) else { continue };
        sources.entry(normalize(&root.target)).or_insert_with(|| {
            let parts = reachable(p, root);
            let mut formulas = vec![];
            for q in p.parts.iter().filter(|q| parts.contains(&normalize(&q.name)) && q.content_type.contains("chart")) {
                if let Ok(x) = xml::parse(&q.data) {
                    chart_formulas(&x, &mut formulas);
                }
            }
            formulas.iter().flat_map(|f| sheet_names_in(f)).collect()
        });
    }
    sources
}

/// The kept sheets of `p` that are written back, and those left out. A kept sheet is left out
/// when a worksheet took its name, when it is a macro sheet and the package isn't macro-enabled
/// (`macros`), and when its charts plot a sheet that isn't written under the same name: the chart
/// is kept as it was, so a series on a deleted or renamed sheet would be left pointing at nothing.
/// A chart may plot a worksheet or another kept sheet that is written (a macro sheet's own cells).
fn live_sheets<'p>(wb: &Workbook, p: &'p Preserved, macros: bool) -> (Vec<&'p PreservedSheet>, Vec<&'p PreservedSheet>) {
    let graphs = chart_sources(p);
    let sources: Vec<Option<&[String]>> = p
        .sheets
        .iter()
        .map(|s| {
            if s.kind == PreservedSheetKind::Module {
                return Some(&[][..]);
            }
            let root = p.rels.iter().find(|r| r.id == s.rel_id)?;
            graphs.get(&normalize(&root.target)).map(Vec::as_slice)
        })
        .collect();
    let mut live: Vec<bool> = p
        .sheets
        .iter()
        .zip(&sources)
        .map(|(s, src)| src.is_some() && (macros || s.kind != PreservedSheetKind::Macro) && !wb.sheets.iter().any(|w| same_sheet(&w.name, &s.name)))
        .collect();
    // Leaving a sheet out can leave out the sheets whose charts plot it.
    loop {
        let written = |n: &str, live: &[bool]| {
            wb.sheets.iter().any(|w| same_sheet(&w.name, n)) || p.sheets.iter().zip(live).any(|(k, &l)| l && same_sheet(&k.name, n))
        };
        let Some(i) = (0..live.len()).find(|&i| {
            live.get(i).copied().unwrap_or(false) && sources.get(i).copied().flatten().is_some_and(|src| !src.iter().all(|n| written(n, &live)))
        }) else {
            break;
        };
        if let Some(l) = live.get_mut(i) {
            *l = false;
        }
    }
    let (kept, gone): (Vec<_>, Vec<_>) = p.sheets.iter().zip(live).partition(|(_, l)| *l);
    (kept.into_iter().map(|(s, _)| s).collect(), gone.into_iter().map(|(s, _)| s).collect())
}

/// Writes the workbook's kept parts and relationships. Kept sheets that can't be written back (see
/// [`live_sheets`]) are left out with the parts only they reach.
pub fn write_workbook(wb: &Workbook, wb_rels: &mut Rels, out: &mut Out, macros: bool) -> WorkbookKept {
    let Some(p) = wb.preserved.as_deref() else { return WorkbookKept::default() };
    let (live, dropped) = live_sheets(wb, p, macros);
    let root_of = |s: &PreservedSheet| p.rels.iter().find(|r| r.id == s.rel_id);
    let dropped_ids: HashSet<&str> = dropped.iter().map(|s| s.rel_id.as_str()).collect();
    if dropped.iter().any(|s| s.kind == PreservedSheetKind::Macro && !macros) {
        out.warn("Excel 4.0 macro sheets were left out: only a macro-enabled workbook (.xlsm) can hold them.");
    }
    if dropped.iter().any(|s| s.kind != PreservedSheetKind::Macro || macros) {
        out.warn("Chart, dialog and macro sheets kept from the file were left out where a worksheet took their name or their charts plot sheets no longer in the workbook.");
    }
    // Parts reached only by dropped chart sheets go with them.
    let mut gone: HashSet<String> = dropped.iter().filter_map(|s| root_of(s)).flat_map(|r| reachable(p, r)).collect();
    if !gone.is_empty() {
        let others = p.rels.iter().filter(|r| !dropped_ids.contains(r.id.as_str())).flat_map(|r| reachable(p, r));
        for n in others {
            gone.remove(&n);
        }
    }
    let mut parts: Vec<PreservedPart> = p.parts.iter().filter(|q| !gone.contains(&normalize(&q.name))).cloned().collect();
    // A macro sheet's cells take the indexes of their strings and formats in the tables written.
    for s in live.iter().filter(|s| !s.indexes.is_empty()) {
        let Some(root) = root_of(s) else { continue };
        let Some(q) = parts.iter_mut().find(|q| normalize(&q.name) == normalize(&root.target)) else { continue };
        q.data = Arc::from(with_indexes(&q.data, &s.indexes, out));
    }
    let rels: Vec<PreservedRel> = p.rels.iter().filter(|r| !dropped_ids.contains(r.id.as_str())).cloned().collect();
    let names = write_parts(out, &parts);
    let ids = attach(wb_rels, &rels, &names);
    for r in &p.package_rels {
        if let Some(target) = rel_target(r, &names) {
            out.kept.root.push((r.rel_type.clone(), target, r.external));
        }
    }
    let mut kept = WorkbookKept {
        elements: p.elements.iter().filter(|e| e.slot == "externalReferences").filter_map(|e| remap(&e.xml, &ids)).collect(),
        ..WorkbookKept::default()
    };
    for s in live {
        // A module sheet has no part: Excel writes its entry with an empty `r:id`.
        let rid = if s.kind == PreservedSheetKind::Module {
            String::new()
        } else {
            let Some(id) = ids.get(&s.rel_id) else { continue };
            id.clone()
        };
        // Its place among the written sheets: after the worksheets it follows and the kept sheets
        // before it.
        let position = s.before.min(wb.sheets.len()) + kept.sheets.len();
        for n in &s.names {
            let _ = write!(kept.names, "<definedName localSheetId=\"{position}\"{}>{}</definedName>", n.attrs, xml::esc(&n.formula));
        }
        kept.sheets.push(KeptSheet { before: s.before, name: s.name.clone(), visibility: s.visibility, rid, sheet_id: s.sheet_id });
    }
    kept
}

/// The bytes of a kept sheet part with each of its `indexes` replaced by the index of what it
/// refers to in the shared strings and formats being written: a cell format's is its model
/// style's (`styles.xml` has one cell format per model style, in order). A cell's `vm` and `cm`
/// stay while the kept metadata they index is written, and go when it isn't.
fn with_indexes(data: &[u8], indexes: &[PreservedIndex], out: &mut Out) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(data.len());
    let mut copied = 0;
    for x in indexes {
        let Some(before) = data.get(copied..x.at.start) else { continue };
        bytes.extend_from_slice(before);
        let i = match &x.to {
            PreservedIndexed::String { text, reading: Some(r) } => out.sst.index_with(text, *r),
            PreservedIndexed::String { text, reading: None } => out.sst.index(text),
            PreservedIndexed::Style(id) => id.0,
            PreservedIndexed::Dxf(style) => crate::styles::dxf_id(&mut out.dxfs, style),
            PreservedIndexed::Metadata => {
                if out.kept.metadata {
                    bytes.extend_from_slice(data.get(x.at.clone()).unwrap_or_default());
                }
                copied = x.at.end;
                continue;
            }
        };
        bytes.extend_from_slice(i.to_string().as_bytes());
        copied = x.at.end;
    }
    bytes.extend_from_slice(data.get(copied..).unwrap_or_default());
    bytes
}

/// The workbook's kept elements of `slot` that refer to no part (`fileVersion`, `ext`).
pub fn workbook_elements(wb: &Workbook, slot: &str) -> String {
    let Some(p) = wb.preserved.as_deref() else { return String::new() };
    p.elements.iter().filter(|e| e.slot == slot).map(|e| e.xml.as_str()).collect()
}

/// Writes the kept theme part as `part` with its relationships and the parts behind them, when the
/// model's theme is still the one the part describes: the format scheme, its picture fills and
/// everything else GridCraft doesn't model come back as they were. `false` when there is no kept
/// theme, or the theme was changed since (it is then written from the model, as a new theme).
pub fn write_theme(wb: &Workbook, part: &str, content_type: &str, out: &mut Out) -> bool {
    let Some(t) = wb.preserved.as_deref().and_then(|p| p.theme.as_ref()) else { return false };
    if !xml::parse(&t.data).is_ok_and(|x| crate::theme::read_theme(&x) == wb.theme) {
        return false;
    }
    out.part(part, Some(content_type), t.data.to_vec());
    if !t.rels.is_empty() {
        let names = write_parts(out, &t.parts);
        out.part(&rels_part_name(part), None, part_rels_xml(&t.rels, &names).into_bytes());
    }
    true
}

// ---------------------------------------------------------------- cell and value metadata

/// Whether `cell` still holds what the value metadata of `m` describes: its value and its formula.
/// The value is compared as the model holds it, not as the file caches it: #SPILL! and #CALC! are
/// both cached as #VALUE!, but the metadata of a #CALC! error doesn't describe a #VALUE! or a
/// #SPILL! one.
pub fn cell_meta_holds(m: &PreservedCellMeta, cell: Option<&Cell>) -> bool {
    let Some(c) = cell else { return false };
    c.value.scalar() == m.value && c.formula.as_ref().map(|f| f.text.as_str()) == m.formula.as_deref()
}

/// Whether the kept cell and value metadata ([`PreservedMetadata`]) goes back into the file with
/// `wb`: when cells that carry value metadata GridCraft doesn't model still hold their values, or
/// a kept macro sheet written with it (`macros`: into a macro-enabled package) has cells that name
/// it, and the workbook has no cell pictures, which GridCraft writes with a metadata part of its
/// own (a picture deleted from the workbook doesn't come back from the kept parts).
pub fn metadata_in_use(wb: &Workbook, macros: bool) -> bool {
    let Some(p) = wb.preserved.as_deref() else { return false };
    let kept = p.metadata.as_ref().is_some_and(|m| m.values);
    let in_macro_sheets = || live_sheets(wb, p, macros).0.iter().any(|s| s.indexes.iter().any(|x| x.to == PreservedIndexed::Metadata));
    let in_worksheets =
        || wb.sheets.iter().any(|s| s.preserved.as_deref().is_some_and(|p| p.cell_meta.iter().any(|m| cell_meta_holds(m, s.cells.get(m.cell)))));
    kept && wb.sheets.iter().all(|s| s.cell_pictures.is_empty()) && (in_worksheets() || in_macro_sheets())
}

impl KeptOut {
    /// Decides whether the kept cell and value metadata of `wb` is written (`allowed`: the caller
    /// has not turned it off; `macros`: the package is macro-enabled). Before any sheet is: it
    /// puts `vm` on the cells.
    pub fn use_metadata(&mut self, wb: &Workbook, allowed: bool, macros: bool) {
        self.metadata = allowed && metadata_in_use(wb, macros);
    }

    /// Whether the kept cell and value metadata is written.
    pub fn metadata(&self) -> bool {
        self.metadata
    }
}

/// Whether the kept metadata of `wb` can sit next to GridCraft's dynamic-array formulas, which
/// are marked `cm="1"`: its entry 1 of cell metadata must be the dynamic-array flag.
pub fn metadata_fits_dynamic_arrays(wb: &Workbook) -> bool {
    wb.preserved.as_deref().and_then(|p| p.metadata.as_ref()).is_some_and(|m| m.dynamic_arrays)
}

/// The kept value metadata of `sheet` by cell, when the kept metadata is written.
pub fn cell_meta_index<'a>(sheet: &'a Sheet, out: &Out) -> HashMap<CellRef, &'a PreservedCellMeta> {
    if !out.kept.metadata {
        return HashMap::new();
    }
    sheet.preserved.as_deref().map(|p| p.cell_meta.iter().map(|m| (m.cell, m)).collect()).unwrap_or_default()
}

/// Writes the kept cell and value metadata: the metadata part and the rich-value parts behind it,
/// with the workbook's relationships to them.
pub fn write_metadata(wb: &Workbook, wb_rels: &mut Rels, out: &mut Out) {
    let Some(m) = wb.preserved.as_deref().and_then(|p| p.metadata.as_ref()) else { return };
    let names = write_parts(out, &m.parts);
    attach(wb_rels, &m.rels, &names);
}

/// Writes the kept rich-value types part, next to the rich values of GridCraft's own cell
/// pictures. Nothing in it depends on the values: it holds the flags of the rich-value keys.
pub fn write_rich_types(wb: &Workbook, wb_rels: &mut Rels, out: &mut Out) {
    let Some(m) = wb.preserved.as_deref().and_then(|p| p.metadata.as_ref()) else { return };
    let Some(root) = m.rels.iter().find(|r| !r.external && is_kind(r, &[KIND_RICH_TYPES])) else { return };
    let Some(part) = m.parts.iter().find(|q| normalize(&q.name) == normalize(&root.target)) else { return };
    let names = write_parts(out, std::slice::from_ref(part));
    attach(wb_rels, std::slice::from_ref(root), &names);
}

/// Adds the kept package-root relationships (custom properties…) to `_rels/.rels`.
pub fn write_package_rels(root: &mut Rels, out: &mut Out) {
    for (ty, target, external) in std::mem::take(&mut out.kept.root) {
        root.add_raw(&ty, &target, external);
    }
}
