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
}

impl KeptOut {
    /// Reserves a part name GridCraft writes itself, so kept parts don't take it. False when it is
    /// taken.
    pub fn reserve(&mut self, name: &str) -> bool {
        self.used.insert(name.to_ascii_lowercase())
    }

    /// Reserves the VML shape blocks the kept header and footer drawings of `wb` use, so that the
    /// notes drawings GridCraft writes don't reuse their shape ids.
    pub fn reserve_vml_blocks(&mut self, wb: &Workbook) {
        let vmls =
            wb.sheets.iter().filter_map(|s| s.preserved.as_deref()).flat_map(|p| &p.parts).filter(|q| q.content_type.eq_ignore_ascii_case(CT_VML));
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
    let by_name: HashMap<String, &PreservedPart> = p.parts.iter().map(|q| (normalize(&q.name), q)).collect();
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
/// other workbooks (`[1]Sheet!A1`) are not sheets of this one and are left out.
pub(crate) fn sheet_names_in(formula: &str) -> Vec<String> {
    let mut names = vec![];
    let chars: Vec<char> = formula.chars().collect();
    let at = |i: usize| chars.get(i).copied();
    let mut i = 0;
    while let Some(c) = at(i) {
        let mut name = String::new();
        let start = i;
        if c == '\'' {
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
            if !name.is_empty() && !name.starts_with('[') {
                names.push(name);
            }
            i += 1;
        } else if i == start {
            i += 1;
        }
    }
    names
}

/// Whether a kept chart sheet can be written back: its chart's series must still refer to sheets
/// that exist under the same names (the chart is kept as it was, so a series on a deleted or
/// renamed sheet would be left pointing at nothing), and no worksheet may have taken its name.
///
/// `usable` caches, by the normalized target of the chart sheet's relationship, whether its graph
/// refers only to sheets that exist (`false` when the relationship is missing): entries sharing one
/// graph, under the same or different relationship ids, are analysed once, and only the verdict is
/// kept, not the references it came from.
fn chart_sheet_usable(wb: &Workbook, p: &Preserved, s: &PreservedSheet, usable: &mut HashMap<String, bool>) -> bool {
    let same = |a: &str, b: &str| a.chars().take(31).collect::<String>().eq_ignore_ascii_case(&b.chars().take(31).collect::<String>());
    if wb.sheets.iter().any(|w| same(&w.name, &s.name)) {
        return false;
    }
    let Some(root) = p.rels.iter().find(|r| r.id == s.rel_id) else { return false };
    *usable.entry(normalize(&root.target)).or_insert_with(|| {
        let parts = reachable(p, root);
        let mut formulas = vec![];
        for q in p.parts.iter().filter(|q| parts.contains(&normalize(&q.name)) && q.content_type.contains("chart")) {
            if let Ok(x) = xml::parse(&q.data) {
                chart_formulas(&x, &mut formulas);
            }
        }
        formulas.iter().flat_map(|f| sheet_names_in(f)).all(|n| wb.sheets.iter().any(|w| same(&w.name, &n)))
    })
}

/// Writes the workbook's kept parts and relationships. Chart sheets that can't be written back
/// (see [`chart_sheet_usable`]) are left out with the parts only they reach.
pub fn write_workbook(wb: &Workbook, wb_rels: &mut Rels, out: &mut Out) -> WorkbookKept {
    let Some(p) = wb.preserved.as_deref() else { return WorkbookKept::default() };
    let (live, dropped): (Vec<&PreservedSheet>, Vec<&PreservedSheet>) = {
        let mut refs = HashMap::new();
        p.sheets.iter().partition(|s| chart_sheet_usable(wb, p, s, &mut refs))
    };
    let root_of = |s: &PreservedSheet| p.rels.iter().find(|r| r.id == s.rel_id);
    let dropped_ids: HashSet<&str> = dropped.iter().map(|s| s.rel_id.as_str()).collect();
    // Parts reached only by dropped chart sheets go with them.
    let mut gone: HashSet<String> = dropped.iter().filter_map(|s| root_of(s)).flat_map(|r| reachable(p, r)).collect();
    if !gone.is_empty() {
        let others = p.rels.iter().filter(|r| !dropped_ids.contains(r.id.as_str())).flat_map(|r| reachable(p, r));
        for n in others {
            gone.remove(&n);
        }
    }
    let parts: Vec<PreservedPart> = p.parts.iter().filter(|q| !gone.contains(&normalize(&q.name))).cloned().collect();
    let rels: Vec<PreservedRel> = p.rels.iter().filter(|r| !dropped_ids.contains(r.id.as_str())).cloned().collect();
    let names = write_parts(out, &parts);
    let ids = attach(wb_rels, &rels, &names);
    for r in &p.package_rels {
        if let Some(target) = rel_target(r, &names) {
            out.kept.root.push((r.rel_type.clone(), target, r.external));
        }
    }
    WorkbookKept {
        elements: p.elements.iter().filter(|e| e.slot == "externalReferences").filter_map(|e| remap(&e.xml, &ids)).collect(),
        sheets: live
            .iter()
            .filter_map(|s| {
                Some(KeptSheet {
                    before: s.before,
                    name: s.name.clone(),
                    visibility: s.visibility,
                    rid: ids.get(&s.rel_id)?.clone(),
                    sheet_id: s.sheet_id,
                })
            })
            .collect(),
    }
}

/// The workbook's kept elements of `slot` that refer to no part (`fileVersion`, `ext`).
pub fn workbook_elements(wb: &Workbook, slot: &str) -> String {
    let Some(p) = wb.preserved.as_deref() else { return String::new() };
    p.elements.iter().filter(|e| e.slot == slot).map(|e| e.xml.as_str()).collect()
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

fn active_user_shapes<'a>(sheet: &Sheet, p: &'a Preserved) -> Vec<UserShapesMeta<'a>> {
    p.elements
        .iter()
        .filter(|e| e.slot == SLOT_CHART_USER_SHAPES)
        .filter_map(|e| parse_user_shapes_meta(&e.xml))
        .filter(|m| {
            let key = normalize(m.target);
            sheet.charts.iter().any(|c| c.id == m.chart_id && !crate::chartex::is_chartex(c.kind))
                && p.parts.iter().any(|q| normalize(&q.name) == key)
                && m.mirrored_images.iter().all(|&(id, fp)| sheet.images.iter().find(|i| i.id == id).is_some_and(|i| fingerprint_image(i) == fp))
                && m.mirrored_shapes.iter().all(|&(id, fp)| sheet.shapes.iter().find(|s| s.id == id).is_some_and(|s| fingerprint_shape(s) == fp))
        })
        .collect()
}

/// Sheet-level kept parts (excluding drawing SVG companions and chart `userShapes` graphs, which
/// are written alongside their owning drawing/chart parts).
pub(super) fn sheet_owned_parts(p: &Preserved) -> Vec<PreservedPart> {
    let has_companions = p.elements.iter().any(|e| e.slot == SLOT_DRAWING_SVG_BLIP || e.slot == SLOT_CHART_USER_SHAPES);
    if !has_companions {
        return p.parts.clone();
    }
    let mut keep: HashSet<String> = HashSet::new();
    for r in p.rels.iter().chain(p.vml.iter().flat_map(|v| v.rels.iter())) {
        keep.extend(reachable(p, r));
    }
    p.parts.iter().filter(|q| keep.contains(&normalize(&q.name))).cloned().collect()
}

/// Model drawing objects mirrored from preserved chart `userShapes` drawings on `sheet`, omitted
/// from the sheet's own drawing XML so the chart drawing can round-trip without duplicating them.
pub fn mirrored_user_shape_refs(sheet: &Sheet) -> HashSet<gridcraft_model::DrawingRef> {
    let Some(p) = sheet.preserved.as_deref() else { return HashSet::new() };
    let mut out = HashSet::new();
    for m in active_user_shapes(sheet, p) {
        for (id, _) in m.mirrored_images {
            out.insert(gridcraft_model::DrawingRef::Image(id));
        }
        for (id, _) in m.mirrored_shapes {
            out.insert(gridcraft_model::DrawingRef::Shape(id));
        }
    }
    out
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

fn write_user_shapes_subparts(p: &Preserved, root_part: &PreservedPart, out: &mut Out) -> NameMap {
    let mut sub_keys: HashSet<String> = HashSet::new();
    for r in &root_part.rels {
        sub_keys.extend(reachable(p, r));
    }
    sub_keys.remove(&normalize(&root_part.name));
    let mut names = NameMap::new();
    let mut non_media: Vec<PreservedPart> = Vec::new();
    for q in &p.parts {
        let key = normalize(&q.name);
        if !sub_keys.contains(&key) {
            continue;
        }
        if key.starts_with("xl/media/") {
            let ext = crate::drawing::image_ext(&q.content_type, &q.data);
            out.default_type(ext, crate::drawing::image_content_type(ext));
            let file = out.image_part(ext, &q.data);
            names.insert(key, format!("xl/media/{file}"));
        } else {
            names.entry(key).or_insert_with(|| out.kept.claim(&q.name));
            non_media.push(q.clone());
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

fn attach_svg_blips(sheet: &Sheet, p: &Preserved, drawing_num: u32, omitted_imgs: &HashSet<u32>, out: &mut Out) {
    let svg_map: HashMap<u32, &str> =
        p.elements.iter().filter(|e| e.slot == SLOT_DRAWING_SVG_BLIP).filter_map(|e| parse_svg_blip_meta(&e.xml)).collect();
    if svg_map.is_empty() && omitted_imgs.is_empty() {
        return;
    }
    let drawing_part = format!("xl/drawings/drawing{drawing_num}.xml");
    let drels_part = format!("xl/drawings/_rels/drawing{drawing_num}.xml.rels");
    let mut rel_items = out.part_mut(&drels_part).map(|b| parse_rels_bytes(b)).unwrap_or_default();
    let mut next_rid = next_rid_num(&rel_items);
    let mut rels_changed = false;
    let mut omitted_rids: HashSet<String> = HashSet::new();
    let mut svg_patches: Vec<(String, String)> = Vec::new();
    for (idx, img) in sheet.images.iter().filter(|i| !i.data.is_empty()).enumerate() {
        let img_rid = format!("rId{}", idx + 1);
        if omitted_imgs.contains(&img.id) {
            omitted_rids.insert(img_rid);
            continue;
        }
        let Some(&target) = svg_map.get(&img.id) else { continue };
        let key = normalize(target);
        let Some(svg_part) = p.parts.iter().find(|q| normalize(&q.name) == key) else { continue };
        out.default_type("svg", "image/svg+xml");
        let file = out.image_part("svg", &svg_part.data);
        let svg_rid = format!("rId{next_rid}");
        next_rid = next_rid.saturating_add(1);
        rel_items.push((svg_rid.clone(), format!("{NS_REL}/image"), format!("../media/{file}"), false));
        svg_patches.push((img_rid, svg_rid));
        rels_changed = true;
    }
    if !omitted_rids.is_empty() {
        rel_items.retain(|(id, _, _, _)| !omitted_rids.contains(id));
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

fn attach_chart_user_shapes(sheet: &Sheet, p: &Preserved, user_shapes: &[UserShapesMeta<'_>], chart_before: u32, out: &mut Out) {
    for meta in user_shapes {
        let Some(chart_idx) = sheet.charts.iter().position(|c| c.id == meta.chart_id) else { continue };
        let chart_num = chart_before + chart_idx as u32 + 1;
        let chart_part = format!("xl/charts/chart{chart_num}.xml");
        if !out.has_part(&chart_part) {
            continue;
        }
        let key = normalize(meta.target);
        let Some(us_part_data) = p.parts.iter().find(|q| normalize(&q.name) == key) else { continue };
        let sub_names = write_user_shapes_subparts(p, us_part_data, out);
        out.drawings += 1;
        let us_dn = out.drawings;
        let us_part = format!("xl/drawings/drawing{us_dn}.xml");
        out.part(&us_part, Some(&us_part_data.content_type), us_part_data.data.to_vec());
        if !us_part_data.rels.is_empty() {
            out.part(&rels_part_name(&us_part), None, part_rels_xml(&us_part_data.rels, &sub_names).into_bytes());
        }
        let crels_part = format!("xl/charts/_rels/chart{chart_num}.xml.rels");
        let mut crels = out.part_mut(&crels_part).map(|b| parse_rels_bytes(b)).unwrap_or_default();
        let us_rid = format!("rId{}", next_rid_num(&crels));
        crels.push((us_rid.clone(), REL_CHART_USER_SHAPES.to_string(), format!("../drawings/drawing{us_dn}.xml"), false));
        let crels_bytes = format_rels_xml(&crels).into_bytes();
        if let Some(slot) = out.part_mut(&crels_part) {
            *slot = crels_bytes;
        } else {
            out.part(&crels_part, None, crels_bytes);
        }
        if let Some(bytes) = out.part_mut(&chart_part)
            && let Ok(mut xml) = String::from_utf8(std::mem::take(bytes))
        {
            if let Some(pos) = xml.rfind("</c:chartSpace>") {
                let tag = format!("<c:userShapes r:id=\"{us_rid}\"/>");
                xml.insert_str(pos, &tag);
            }
            *bytes = xml.into_bytes();
        }
    }
}

/// Attaches preserved drawing SVG companions (`asvg:svgBlip`) and chart `userShapes` drawings for
/// the sheet whose drawing was just written as `xl/drawings/drawing{drawing_num}.xml`.
pub fn attach_sheet_drawing_companions(sheet: &Sheet, drawing_num: u32, chart_before: u32, out: &mut Out) {
    let Some(p) = sheet.preserved.as_deref() else { return };
    let user_shapes = active_user_shapes(sheet, p);
    let omitted_imgs: HashSet<u32> = user_shapes.iter().flat_map(|m| m.mirrored_images.iter().map(|&(id, _)| id)).collect();
    attach_svg_blips(sheet, p, drawing_num, &omitted_imgs, out);
    attach_chart_user_shapes(sheet, p, &user_shapes, chart_before, out);
}
