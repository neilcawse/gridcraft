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
    /// Names of the workbook's kept parts, which sheets' kept parts can refer to (the cache of a
    /// kept PivotTable).
    pub(super) wb_names: NameMap,
    /// The `cacheId`s of the workbook's kept pivot caches: a kept PivotTable that names its cache
    /// only by id is written only when its cache is among them.
    pub(super) cache_ids: HashSet<u32>,
    patches: StylePatches,
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
    pub(crate) fn claim(&mut self, original: &str) -> String {
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

/// Rewrites the values of `dxfId` and `numFmtId` attributes through the maps (ids not in a map
/// are kept). `None` when the XML is damaged.
fn map_style_ids(data: &[u8], dxfs: &HashMap<u32, u32>, fmts: &HashMap<u32, u32>) -> Option<Vec<u8>> {
    let text = xml::to_utf8(data);
    let mut reader = Reader::from_reader(text.as_ref());
    let mut writer = Writer::new(Vec::with_capacity(data.len()));
    let mut buf = vec![];
    let map = |e: &BytesStart<'_>| -> Option<BytesStart<'static>> {
        let name = String::from_utf8(e.name().as_ref().to_vec()).ok()?;
        let mut out = BytesStart::new(name);
        for a in e.attributes().with_checks(false) {
            let a = a.ok()?;
            let k = a.key.local_name();
            let table = match k.as_ref() {
                b"dxfId" => Some(dxfs),
                b"numFmtId" => Some(fmts),
                _ => None,
            };
            let new = table.and_then(|t| std::str::from_utf8(&a.value).ok()?.trim().parse::<u32>().ok().and_then(|id| t.get(&id)));
            match new {
                Some(n) => out.push_attribute(Attribute { key: a.key, value: Cow::Owned(n.to_string().into_bytes()) }),
                None => out.push_attribute(Attribute { key: a.key, value: Cow::Owned(a.value.to_vec()) }),
            }
        }
        Some(out)
    };
    loop {
        match reader.read_event_into(&mut buf).ok()? {
            Event::Eof => break,
            Event::Start(e) => writer.write_event(Event::Start(map(&e)?)).ok()?,
            Event::Empty(e) => writer.write_event(Event::Empty(map(&e)?)).ok()?,
            ev => writer.write_event(ev).ok()?,
        }
        buf.clear();
    }
    Some(writer.into_inner())
}

/// Kept parts whose style ids are renumbered once the styles part is written.
#[derive(Default)]
pub struct StylePatches {
    /// (written part name, source bytes, dxf id map, source number format id → code).
    pub(super) list: Vec<StylePatch>,
}

pub(super) type StylePatch = (String, Arc<[u8]>, HashMap<u32, u32>, Vec<(u32, String)>);

impl KeptOut {
    /// Number format codes kept parts refer to (to be registered in the styles part).
    pub fn num_fmt_codes(&self) -> Vec<String> {
        let mut v: Vec<String> = vec![];
        for (_, _, _, fmts) in &self.patches.list {
            for (_, code) in fmts {
                if !v.contains(code) {
                    v.push(code.clone());
                }
            }
        }
        v
    }
}

/// Renumbers the `dxfId` and `numFmtId` attributes of kept PivotTables and pivot caches to the
/// written styles part. `fmt_id` maps a number format code to its id there.
pub fn finish_style_ids(out: &mut Out, fmt_id: &dyn Fn(&str) -> Option<u32>) {
    for (name, data, dxfs, fmts) in std::mem::take(&mut out.kept.patches.list) {
        let fmts: HashMap<u32, u32> = fmts.iter().filter_map(|(old, code)| Some((*old, fmt_id(code)?))).collect();
        if let Some(x) = map_style_ids(&data, &dxfs, &fmts) {
            out.replace(&name, x);
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

pub(super) fn write_parts<'a>(out: &mut Out, parts: impl IntoIterator<Item = &'a PreservedPart, IntoIter: Clone>) -> NameMap {
    write_parts_named(out, parts, NameMap::new())
}

/// [`write_parts`] with some parts' new names already chosen in `names`.
fn write_parts_named<'a>(out: &mut Out, parts: impl IntoIterator<Item = &'a PreservedPart, IntoIter: Clone>, mut names: NameMap) -> NameMap {
    let parts = parts.into_iter();
    for p in parts.clone() {
        names.entry(normalize(&p.name)).or_insert_with(|| out.kept.claim(&p.name));
    }
    let own: HashSet<String> = names.values().cloned().collect();
    for (k, v) in &out.kept.wb_names {
        names.entry(k.clone()).or_insert_with(|| v.clone());
    }
    let mut written = HashSet::new();
    for p in parts {
        let Some(new) = names.get(&normalize(&p.name)).cloned() else { continue };
        if !own.contains(&new) || !written.insert(new.clone()) {
            continue;
        }
        out.part(&new, Some(&p.content_type), p.data.to_vec());
        if !p.dxfs.is_empty() || !p.num_fmts.is_empty() {
            let dxfs = p.dxfs.iter().map(|(old, s)| (*old, crate::styles::dxf_id(&mut out.dxfs, s))).collect();
            out.kept.patches.list.push((new.clone(), p.data.clone(), dxfs, p.num_fmts.clone()));
        }
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
    /// Kept pivot caches: (`cacheId`, new relationship id).
    pivot_caches: Vec<(u32, String)>,
}

impl WorkbookKept {
    /// The `cacheId`s of the kept pivot caches (written PivotTables must not reuse them).
    pub fn pivot_cache_ids(&self) -> Vec<u32> {
        self.pivot_caches.iter().map(|(id, _)| *id).collect()
    }

    /// The `<pivotCache>` entries of the kept pivot caches.
    pub fn pivot_cache_entries(&self) -> String {
        let mut x = String::new();
        for (id, rid) in &self.pivot_caches {
            let _ = write!(x, "<pivotCache cacheId=\"{id}\" r:id=\"{}\"/>", esc_attr(rid));
        }
        x
    }

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
    reachable_in(&p.parts, std::iter::once(root))
}

/// The part names reachable from the roots through kept parts' relationships (normalized).
pub(super) fn reachable_in<'a>(parts: &[PreservedPart], roots: impl Iterator<Item = &'a PreservedRel>) -> HashSet<String> {
    let by_name: HashMap<String, &PreservedPart> = parts.iter().map(|q| (normalize(&q.name), q)).collect();
    let mut seen = HashSet::new();
    let mut queue: VecDeque<_> = roots.map(|r| normalize(&r.target)).collect();
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

/// The sheet names the formulas of a kept chart-kind part refer to; `None` when its XML doesn't
/// parse. Parts without formulas (styles, colours) give none.
fn plotted_sheets(q: &PreservedPart) -> Option<Vec<String>> {
    if !q.content_type.contains("chart") {
        return Some(vec![]);
    }
    let x = xml::parse(&q.data).ok()?;
    let mut formulas = vec![];
    chart_formulas(&x, &mut formulas);
    Some(formulas.iter().flat_map(|f| sheet_names_in(f)).collect())
}

/// Sheet names as Excel compares them: the first 31 characters, ignoring ASCII case.
fn same_sheet(a: &str, b: &str) -> bool {
    a.chars().take(31).collect::<String>().eq_ignore_ascii_case(&b.chars().take(31).collect::<String>())
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

/// One owner for chart annotations: the kept chart, a graft onto a rewritten chart, or the
/// live worksheet mirrors. The chart paths suppress mirrors only for the part they write.
pub(crate) enum ChartSave<'a> {
    Kept(&'a PreservedChart),
    /// The original annotation part `root` and every part it reaches, written with the chart.
    Graft {
        root: &'a PreservedPart,
        closure: Vec<&'a PreservedPart>,
    },
    Mirrors,
}

impl ChartSave<'_> {
    /// Whether this plan writes the annotation part `target` with its chart.
    fn writes(&self, target: &str) -> bool {
        let target = normalize(target);
        match self {
            Self::Kept(k) => k.parts.iter().any(|q| normalize(&q.name) == target),
            Self::Graft { root, .. } => normalize(&root.name) == target,
            Self::Mirrors => false,
        }
    }
}

/// The save plan of each chart of `sheet`, in order; empty (every chart [`ChartSave::Mirrors`])
/// when the sheet keeps nothing. Adds to `mirrored` the drawing objects that mirror annotations a
/// plan writes with its chart: the sheet's own drawing leaves them out.
pub(crate) fn chart_save_plans<'a>(wb: &Workbook, sheet: &'a Sheet, mirrored: &mut HashSet<gridcraft_model::DrawingRef>) -> Vec<ChartSave<'a>> {
    use gridcraft_model::DrawingRef;
    let Some(p) = sheet.preserved.as_deref() else { return Vec::new() };
    // Each annotation record is parsed once for the sheet, then grouped by chart (a stable sort,
    // so each chart's records stay in file order).
    let mut metas: Vec<UserShapesMeta<'a>> =
        p.elements.iter().filter(|e| e.slot == SLOT_CHART_USER_SHAPES).filter_map(|e| parse_user_shapes_meta(&e.xml)).collect();
    metas.sort_by_key(|m| m.chart_id);
    sheet
        .charts
        .iter()
        .map(|ch| {
            let start = metas.partition_point(|m| m.chart_id < ch.id);
            let annotations = &metas[start..start + metas[start..].partition_point(|m| m.chart_id == ch.id)];
            let plan = chart_save(wb, sheet, p, ch, annotations);
            for m in annotations.iter().filter(|m| plan.writes(m.target)) {
                mirrored.extend(m.mirrored_images.iter().map(|&(id, _)| DrawingRef::Image(id)));
                mirrored.extend(m.mirrored_shapes.iter().map(|&(id, _)| DrawingRef::Shape(id)));
            }
            plan
        })
        .collect()
}

/// The plan of chart `ch`, whose annotation records are `annotations`.
fn chart_save<'a>(wb: &Workbook, sheet: &Sheet, p: &'a Preserved, ch: &Chart, annotations: &[UserShapesMeta<'a>]) -> ChartSave<'a> {
    if crate::chartex::is_chartex(ch.kind) || !annotations.iter().all(|m| mirrors_unmodified(sheet, m)) {
        return ChartSave::Mirrors;
    }
    if let Some(k) = p.charts.iter().find(|k| k.id == ch.id) {
        if unplaced(ch) != k.chart {
            return ChartSave::Mirrors;
        }
        if kept_chart_parts_are_valid(wb, k) {
            return ChartSave::Kept(k);
        }
    }
    annotations.first().and_then(|m| annotation_graft(p, m.target)).unwrap_or(ChartSave::Mirrors)
}

/// Structural checks only: an edited chart must not graft annotations from its original snapshot.
fn kept_chart_parts_are_valid(wb: &Workbook, k: &PreservedChart) -> bool {
    if !k.parts.iter().any(|q| normalize(&q.name) == normalize(&k.part)) {
        return false;
    }
    k.parts.iter().all(|part| plotted_sheets(part).is_some_and(|names| names.iter().all(|n| wb.sheets.iter().any(|w| same_sheet(&w.name, n)))))
}

/// Writes a kept chart: the chart part as `name` (the name GridCraft's writer
/// gives that chart), the parts behind it under fresh names. Returns the id of the drawing's new
/// relationship to it.
pub fn write_kept_chart(k: &PreservedChart, name: &str, drawing_rels: &mut Rels, out: &mut Out) -> String {
    write_parts_named(out, &k.parts, NameMap::from([(normalize(&k.part), name.to_string())]));
    drawing_rels.add("chart", &format!("/{name}"))
}

/// Writes the workbook's kept parts and relationships. Kept sheets that can't be written back (see
/// [`live_sheets`]) are left out with the parts only they reach.
pub fn write_workbook(wb: &Workbook, wb_rels: &mut Rels, out: &mut Out, macros: bool) -> WorkbookKept {
    let Some(p) = wb.preserved.as_deref() else { return WorkbookKept::default() };
    let (live, dropped) = live_sheets(wb, p, macros);
    let root_of = |s: &PreservedSheet| p.rels.iter().find(|r| r.id == s.rel_id);
    if dropped.iter().any(|s| s.kind == PreservedSheetKind::Macro && !macros) {
        out.warn("Excel 4.0 macro sheets were left out: only a macro-enabled workbook (.xlsm) can hold them.");
    }
    if dropped.iter().any(|s| s.kind != PreservedSheetKind::Macro || macros) {
        out.warn("Chart, dialog and macro sheets kept from the file were left out where a worksheet took their name or their charts plot sheets no longer in the workbook.");
    }
    // Pivot caches no kept PivotTable of a sheet uses any more (their sheets were deleted).
    let sheet_parts = || wb.sheets.iter().filter_map(|s| s.preserved.as_deref()).flat_map(|q| q.parts.iter());
    let mut used_caches: HashSet<String> = sheet_parts()
        .flat_map(|part| part.rels.iter())
        .filter(|r| !r.external && is_kind(r, &["pivotCacheDefinition"]))
        .map(|r| normalize(&r.target))
        .collect();
    // A PivotTable can name its cache only by the workbook's `cacheId`.
    let cache_of_id: HashMap<u32, String> = p
        .elements
        .iter()
        .filter(|e| e.slot == "pivotCache")
        .filter_map(|e| {
            let x = xml::parse(e.xml.as_bytes()).ok()?;
            let rel = p.rels.iter().find(|r| Some(r.id.as_str()) == x.attr("id"))?;
            Some((x.attr_u32("cacheId")?, normalize(&rel.target)))
        })
        .collect();
    used_caches.extend(
        sheet_parts()
            .filter(|part| part.content_type == CT_PIVOT_TABLE)
            .filter_map(|part| xml::parse(&part.data).ok()?.attr_u32("cacheId"))
            .filter_map(|id| cache_of_id.get(&id).cloned()),
    );
    let unused_caches: Vec<&PreservedRel> =
        p.rels.iter().filter(|r| !r.external && is_kind(r, &["pivotCacheDefinition"]) && !used_caches.contains(&normalize(&r.target))).collect();
    let mut dropped_ids: HashSet<&str> = dropped.iter().map(|s| s.rel_id.as_str()).collect();
    dropped_ids.extend(unused_caches.iter().map(|r| r.id.as_str()));
    // Parts reached only by dropped chart sheets and pivot caches go with them.
    let mut gone: HashSet<String> =
        dropped.iter().filter_map(|s| root_of(s)).chain(unused_caches.iter().copied()).flat_map(|r| reachable(p, r)).collect();
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
    let pivot_caches: Vec<(u32, String)> = p
        .elements
        .iter()
        .filter(|e| e.slot == "pivotCache")
        .filter_map(|e| {
            let x = xml::parse(e.xml.as_bytes()).ok()?;
            Some((x.attr_u32("cacheId")?, ids.get(x.attr("id")?)?.clone()))
        })
        .collect();
    out.kept.wb_names = names;
    out.kept.cache_ids = pivot_caches.iter().map(|(id, _)| *id).collect();
    let mut kept = WorkbookKept {
        pivot_caches,
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
            let (name, attrs, formula) = (esc_attr(&n.name), &n.attrs, xml::esc(&n.formula));
            let _ = write!(kept.names, "<definedName localSheetId=\"{position}\" name=\"{name}\"{attrs}>{formula}</definedName>");
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
    c.value.scalar() == m.value && c.formula.as_ref().map(|f| f.text()).as_deref() == m.formula.as_deref()
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

// ---------------------------------------------------------------- drawing & chart companions

fn hash_f32(h: &mut sha2::Sha256, v: f32) {
    use sha2::Digest as _;
    let canonical = if v == 0.0 { 0.0f32 } else { v };
    h.update(canonical.to_le_bytes());
}

fn hash_bytes(h: &mut sha2::Sha256, bytes: &[u8]) {
    use sha2::Digest as _;
    h.update((bytes.len() as u64).to_le_bytes());
    h.update(bytes);
}

fn hash_color(h: &mut sha2::Sha256, color: gridcraft_model::Color) {
    use sha2::Digest as _;
    match color {
        gridcraft_model::Color::Auto => h.update([0u8]),
        gridcraft_model::Color::Rgb(rgb) => {
            h.update([1u8]);
            h.update(rgb.to_le_bytes());
        }
        gridcraft_model::Color::Theme(idx, tint) => {
            h.update([2u8, idx]);
            h.update(tint.to_le_bytes());
        }
    }
}

fn hash_anchor(h: &mut sha2::Sha256, anchor: &gridcraft_model::Anchor) {
    use sha2::Digest as _;
    let gridcraft_model::Anchor { cell, dx, dy, width, height, mode } = *anchor;
    h.update(cell.row.to_le_bytes());
    h.update(cell.col.to_le_bytes());
    hash_f32(h, dx);
    hash_f32(h, dy);
    hash_f32(h, width);
    hash_f32(h, height);
    let mode_tag = match mode {
        gridcraft_model::AnchorMode::MoveAndSize => 0u8,
        gridcraft_model::AnchorMode::MoveOnly => 1u8,
        gridcraft_model::AnchorMode::Absolute => 2u8,
    };
    h.update([mode_tag]);
}

fn finish_fingerprint(h: sha2::Sha256) -> u128 {
    use sha2::Digest as _;
    let digest = h.finalize();
    let mut prefix = [0u8; 16];
    prefix.copy_from_slice(&digest[..16]);
    u128::from_le_bytes(prefix)
}

/// Deterministic fingerprint of a mirrored chart `userShapes` picture (anchor, bytes, mime, alt).
pub(crate) fn fingerprint_image(img: &gridcraft_model::Image) -> u128 {
    use sha2::Digest as _;
    let gridcraft_model::Image { id: _, anchor, data, mime, alt } = img;
    let mut h = sha2::Sha256::new();
    h.update(b"chart-user-image:v1");
    hash_anchor(&mut h, anchor);
    hash_bytes(&mut h, data);
    hash_bytes(&mut h, mime.as_bytes());
    hash_bytes(&mut h, alt.as_bytes());
    finish_fingerprint(h)
}

/// Deterministic fingerprint of a mirrored chart `userShapes` shape (kind, anchor, fill, line,
/// text, rotation, and text/outline formatting).
pub(crate) fn fingerprint_shape(shape: &gridcraft_model::Shape) -> u128 {
    use gridcraft_model::{ShapeFormat, ShapeKind, ShapeTextAlign, ShapeTextAnchor};
    use sha2::Digest as _;
    let gridcraft_model::Shape { id: _, kind, anchor, fill, line, text, format, rotation, math: _ } = shape;
    let mut h = sha2::Sha256::new();
    h.update(b"chart-user-shape:v1");
    let kind_tag = match kind {
        ShapeKind::Rectangle => 0u8,
        ShapeKind::RoundedRectangle => 1u8,
        ShapeKind::Ellipse => 2u8,
        ShapeKind::Triangle => 3u8,
        ShapeKind::Line => 4u8,
        ShapeKind::Arrow => 5u8,
        ShapeKind::TextBox => 6u8,
        ShapeKind::Icon => 7u8,
        ShapeKind::Ink => 8u8,
        ShapeKind::WordArt => 9u8,
        ShapeKind::Equation => 10u8,
    };
    h.update([kind_tag]);
    hash_anchor(&mut h, anchor);
    hash_color(&mut h, *fill);
    hash_color(&mut h, *line);
    hash_bytes(&mut h, text.as_bytes());
    hash_f32(&mut h, *rotation);
    let ShapeFormat { no_line, font_size, bold, italic, font, text_color, text_outline, text_shadow, align, anchor: v_anchor, rtl, left_indent } =
        format;
    h.update([u8::from(*no_line), u8::from(*bold), u8::from(*italic), u8::from(*rtl)]);
    hash_f32(&mut h, *left_indent);
    match font_size {
        Some(sz) => {
            h.update([1u8]);
            hash_f32(&mut h, *sz);
        }
        None => h.update([0u8]),
    }
    match font {
        Some(name) => {
            h.update([1u8]);
            hash_bytes(&mut h, name.as_bytes());
        }
        None => h.update([0u8]),
    }
    match text_color {
        Some(c) => {
            h.update([1u8]);
            hash_color(&mut h, *c);
        }
        None => h.update([0u8]),
    }
    match text_outline {
        Some((c, w)) => {
            h.update([1u8]);
            hash_color(&mut h, *c);
            hash_f32(&mut h, *w);
        }
        None => h.update([0u8]),
    }
    match text_shadow {
        Some(c) => {
            h.update([1u8]);
            hash_color(&mut h, *c);
        }
        None => h.update([0u8]),
    }
    let align_tag = match align {
        None => 0u8,
        Some(ShapeTextAlign::Left) => 1u8,
        Some(ShapeTextAlign::Center) => 2u8,
        Some(ShapeTextAlign::Right) => 3u8,
    };
    let v_anchor_tag = match v_anchor {
        None => 0u8,
        Some(ShapeTextAnchor::Top) => 1u8,
        Some(ShapeTextAnchor::Middle) => 2u8,
        Some(ShapeTextAnchor::Bottom) => 3u8,
    };
    h.update([align_tag, v_anchor_tag]);
    finish_fingerprint(h)
}

struct UserShapesMeta<'a> {
    chart_id: u32,
    target: &'a str,
    mirrored_images: Vec<(u32, u128)>,
    mirrored_shapes: Vec<(u32, u128)>,
}

fn parse_id_fp_list(s: &str) -> Option<Vec<(u32, u128)>> {
    if s.is_empty() {
        return Some(Vec::new());
    }
    let mut out = Vec::new();
    for item in s.split(',') {
        let (id, fp) = item.split_once(':')?;
        out.push((id.parse::<u32>().ok()?, u128::from_str_radix(fp, 16).ok()?));
    }
    Some(out)
}

fn parse_user_shapes_meta(xml: &str) -> Option<UserShapesMeta<'_>> {
    let mut it = xml.split('\t');
    let chart_id = it.next()?.parse::<u32>().ok()?;
    let target = it.next()?;
    let mirrored_images = parse_id_fp_list(it.next()?)?;
    let mirrored_shapes = parse_id_fp_list(it.next()?)?;
    Some(UserShapesMeta { chart_id, target, mirrored_images, mirrored_shapes })
}

fn parse_svg_blip_meta(xml: &str) -> Option<(u32, &str)> {
    let (id, target) = xml.split_once('\t')?;
    Some((id.parse::<u32>().ok()?, target))
}

fn mirrors_unmodified(sheet: &Sheet, m: &UserShapesMeta<'_>) -> bool {
    m.mirrored_images.iter().all(|&(id, fp)| sheet.images.iter().find(|i| i.id == id).is_some_and(|i| fingerprint_image(i) == fp))
        && m.mirrored_shapes.iter().all(|&(id, fp)| sheet.shapes.iter().find(|s| s.id == id).is_some_and(|s| fingerprint_shape(s) == fp))
}

fn annotation_graft<'a>(p: &'a Preserved, target: &str) -> Option<ChartSave<'a>> {
    let key = normalize(target);
    let root = p.parts.iter().find(|q| normalize(&q.name) == key)?;
    xml::parse(&root.data).ok().filter(|x| x.name == "userShapes")?;
    let mut keys = reachable_in(&p.parts, root.rels.iter().filter(|r| !r.external));
    keys.remove(&key);
    let parts = p.parts.iter().filter(|q| keys.remove(&normalize(&q.name))).collect();
    keys.is_empty().then_some(ChartSave::Graft { root, closure: parts })
}

/// Sheet-level kept parts (excluding drawing SVG companions and chart `userShapes` graphs, which
/// are written alongside their owning drawing/chart parts).
pub(super) fn sheet_owned_parts(p: &Preserved) -> Vec<PreservedPart> {
    let has_companions =
        p.elements.iter().any(|e| e.slot == SLOT_DRAWING_SVG_BLIP || e.slot == SLOT_CHART_USER_SHAPES || e.slot == SLOT_PIVOT_PASSTHROUGH);
    if !has_companions {
        return p.parts.clone();
    }
    let mut keep: HashSet<String> = HashSet::new();
    for r in p.rels.iter().chain(p.vml.iter().flat_map(|v| v.rels.iter())) {
        keep.extend(reachable(p, r));
    }
    p.parts.iter().filter(|q| keep.contains(&normalize(&q.name))).cloned().collect()
}

fn parse_rels_bytes(bytes: &[u8]) -> Vec<(String, String, String, bool)> {
    let Ok(root) = xml::parse(bytes) else { return Vec::new() };
    root.kids("Relationship")
        .filter_map(|r| {
            let id = r.attr("Id")?.to_string();
            let ty = r.attr("Type")?.to_string();
            let target = r.attr("Target")?.to_string();
            let ext = r.attr("TargetMode").is_some_and(|m| m.eq_ignore_ascii_case("external"));
            Some((id, ty, target, ext))
        })
        .collect()
}

fn format_rels_xml(items: &[(String, String, String, bool)]) -> String {
    let mut s = format!("{XML_DECL}<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">");
    for (id, ty, target, ext) in items {
        let _ = write!(
            s,
            "<Relationship Id=\"{}\" Type=\"{}\" Target=\"{}\"{}/>",
            esc_attr(id),
            esc_attr(ty),
            esc_attr(target),
            if *ext { " TargetMode=\"External\"" } else { "" }
        );
    }
    s.push_str("</Relationships>");
    s
}

fn next_rid_num(items: &[(String, String, String, bool)]) -> u32 {
    items.iter().filter_map(|(id, _, _, _)| id.strip_prefix("rId").and_then(|n| n.parse::<u32>().ok())).max().unwrap_or(0).saturating_add(1)
}

fn write_user_shapes_subparts(parts: &[&PreservedPart], out: &mut Out) -> NameMap {
    let mut names = NameMap::new();
    let mut non_media = Vec::new();
    for &q in parts {
        let key = normalize(&q.name);
        if key.starts_with("xl/media/") {
            let ext = crate::drawing::image_ext(&q.content_type, &q.data);
            out.default_type(ext, crate::drawing::image_content_type(ext));
            let file = out.image_part(ext, &q.data);
            names.insert(key, format!("xl/media/{file}"));
        } else {
            names.entry(key).or_insert_with(|| out.kept.claim(&q.name));
            non_media.push(q);
        }
    }
    let mut written = HashSet::new();
    for q in &non_media {
        let Some(new) = names.get(&normalize(&q.name)).cloned() else { continue };
        if !written.insert(new.clone()) {
            continue;
        }
        out.part(&new, Some(&q.content_type), q.data.to_vec());
        if !q.rels.is_empty() {
            out.part(&rels_part_name(&new), None, part_rels_xml(&q.rels, &names).into_bytes());
        }
    }
    names
}

/// Adds the kept SVG companions of the pictures written in drawing `drawing_num`; `images` gives
/// each written picture's id with the relationship id its `<a:blip>` was written with.
pub(crate) fn attach_svg_blips<'r>(sheet: &Sheet, drawing_num: u32, images: impl Iterator<Item = (u32, &'r str)>, out: &mut Out) {
    let Some(p) = sheet.preserved.as_deref() else { return };
    let svg_map: HashMap<u32, &str> =
        p.elements.iter().filter(|e| e.slot == SLOT_DRAWING_SVG_BLIP).filter_map(|e| parse_svg_blip_meta(&e.xml)).collect();
    if svg_map.is_empty() {
        return;
    }
    let drawing_part = format!("xl/drawings/drawing{drawing_num}.xml");
    let drels_part = format!("xl/drawings/_rels/drawing{drawing_num}.xml.rels");
    let mut rel_items = out.part_mut(&drels_part).map(|b| parse_rels_bytes(b)).unwrap_or_default();
    let mut next_rid = next_rid_num(&rel_items);
    let mut rels_changed = false;
    let mut svg_patches: Vec<(String, String)> = Vec::new();
    for (img_id, img_rid) in images {
        let Some(&target) = svg_map.get(&img_id) else { continue };
        let key = normalize(target);
        let Some(svg_part) = p.parts.iter().find(|q| normalize(&q.name) == key) else { continue };
        out.default_type("svg", "image/svg+xml");
        let file = out.image_part("svg", &svg_part.data);
        let svg_rid = format!("rId{next_rid}");
        next_rid = next_rid.saturating_add(1);
        rel_items.push((svg_rid.clone(), format!("{NS_REL}/image"), format!("../media/{file}"), false));
        svg_patches.push((img_rid.to_string(), svg_rid));
        rels_changed = true;
    }
    if rels_changed {
        let xml_bytes = format_rels_xml(&rel_items).into_bytes();
        if let Some(slot) = out.part_mut(&drels_part) {
            *slot = xml_bytes;
        } else if !rel_items.is_empty() {
            out.part(&drels_part, None, xml_bytes);
        }
    }
    if !svg_patches.is_empty()
        && let Some(bytes) = out.part_mut(&drawing_part)
        && let Ok(mut xml) = String::from_utf8(std::mem::take(bytes))
    {
        for (img_rid, svg_rid) in svg_patches {
            let needle = format!("<a:blip r:embed=\"{img_rid}\"/>");
            let replacement = format!(
                "<a:blip r:embed=\"{img_rid}\"><a:extLst><a:ext uri=\"{SVG_BLIP_EXT_URI}\"><asvg:svgBlip xmlns:asvg=\"{NS_ASVG}\" r:embed=\"{svg_rid}\"/></a:ext></a:extLst></a:blip>"
            );
            if let Some(pos) = xml.find(&needle) {
                xml.replace_range(pos..pos + needle.len(), &replacement);
            }
        }
        *bytes = xml.into_bytes();
    }
}

/// Writes the annotation closure for a regenerated chart and returns its relationship id.
pub(crate) fn attach_chart_user_shapes(part: &PreservedPart, parts: &[&PreservedPart], chart_num: u32, out: &mut Out) -> String {
    let sub_names = write_user_shapes_subparts(parts, out);
    out.drawings += 1;
    let drawing = format!("xl/drawings/drawing{}.xml", out.drawings);
    out.part(&drawing, Some(&part.content_type), part.data.to_vec());
    if !part.rels.is_empty() {
        out.part(&rels_part_name(&drawing), None, part_rels_xml(&part.rels, &sub_names).into_bytes());
    }
    let mut rels = Rels::default();
    let rid = rels.add_uri(REL_CHART_USER_SHAPES, &format!("../drawings/drawing{}.xml", out.drawings));
    out.part(&format!("xl/charts/_rels/chart{chart_num}.xml.rels"), None, rels.xml().into_bytes());
    rid
}
