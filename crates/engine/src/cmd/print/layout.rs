use super::header_footer::HF_FIXED_SIDE_MARGIN;
use super::{
    Anchor, Borders, CellRef, Font, Json, MAX_COLS, MAX_PAGES, MAX_ROWS, Orientation, PX_TO_PT, PageOrder, PatternType, PrintSettings, RangeRef,
    Result, Sheet, StyleId, Visibility, Workbook, bad, bool_param, split_sheet, str_param,
};

/// Cells in a page block above which only stored cells are visited (not every grid position).
pub(super) const DENSE_BLOCK: u64 = 60_000;

/// Windows Excel reserves 4 printer dots (0.48 pt at 600 dpi) on each edge of the cell grid,
/// reducing the usable width and height between margins by 0.96 pt.
pub(super) const GRID_EDGE_INSET_PT: f32 = 0.48;

// ---------------------------------------------------------------- paper

/// Paper size in points (portrait) for a paper name; unknown names fall back to Letter.
pub fn paper_size(name: &str) -> (f32, f32) {
    known_paper(name).unwrap_or((612.0, 792.0))
}

/// Paper sizes in points, portrait. `B4`/`B5`/`B6` are the **JIS** sizes Japanese Excel and its
/// e-mail attachments use (257×364, 182×257 and 128×182 mm); the ISO B series stays reachable as
/// `B4 (ISO)`/`B5 (ISO)`/`B6 (ISO)`. The Japanese postcards and envelopes are the JIS ones too.
pub(super) fn known_paper(name: &str) -> Option<(f32, f32)> {
    // Spaces, hyphens, underscores and parentheses don't matter: `B5 (JIS)`, `b5-jis` and `b5`
    // are one name.
    let n = name.trim().to_ascii_lowercase().replace([' ', '-', '_', '(', ')'], "");
    Some(match n.as_str() {
        "letter" | "usletter" => (612.0, 792.0),
        "legal" | "uslegal" => (612.0, 1008.0),
        "a4" => (595.28, 841.89),
        "a3" => (841.89, 1190.55),
        "a5" => (419.53, 595.28),
        "a6" => (297.64, 419.53),
        "tabloid" | "11x17" => (792.0, 1224.0),
        "ledger" => (1224.0, 792.0),
        "executive" => (522.0, 756.0),
        "b4" | "b4jis" => (728.50, 1031.81),
        "b5" | "b5jis" => (515.91, 728.50),
        "b6" | "b6jis" => (362.83, 515.91),
        "b4iso" => (708.66, 1000.63),
        "b5iso" => (498.90, 708.66),
        "b6iso" => (354.33, 498.90),
        "japanesepostcard" | "postcard" => (283.46, 419.53),
        "japanesedoublepostcard" | "doublepostcard" => (419.53, 566.93),
        "japaneseenvelope#3" | "chou#3" => (340.16, 666.14),
        // 長形4号 is 90×205 mm; 90×225 mm is 長形40号, a different envelope, so it stays out.
        "japaneseenvelope#4" | "chou#4" => (255.12, 581.10),
        "japaneseenvelopekaku#2" | "kaku#2" => (680.31, 941.10),
        "japaneseenvelopekaku#3" | "kaku#3" => (612.28, 785.20),
        _ => return None,
    })
}

// ---------------------------------------------------------------- layout

/// The pagination of one sheet.
#[derive(Clone, Debug)]
pub struct SheetLayout {
    pub sheet: usize,
    pub settings: PrintSettings,
    pub page_w: f32,
    pub page_h: f32,
    /// Left, right, top, bottom, header, footer margins in points.
    pub margins: [f32; 6],
    /// Horizontal points per sheet column pixel (at 100% scale), matching Excel's Normal-font
    /// printer digit-width geometry.
    pub col_px_to_pt: f32,
    /// Vertical points per sheet row pixel (at 100% scale), matching Excel's Normal-font
    /// 600-dpi printer-to-96-dpi screen row-height ratio.
    pub row_px_to_pt: f32,
    /// Rows without `customHeight` whose content font requires a taller 96-dpi row height than
    /// stored: sorted `(row, effective_px, cumulative_delta_px_including_row)`.
    pub(crate) row_deltas: Vec<(u32, f32, f64)>,
    pub scale: f32,
    pub area: RangeRef,
    pub row_bands: Vec<(u32, u32)>,
    pub col_bands: Vec<(u32, u32)>,
    /// Pages in print order: (row band, column band).
    pub pages: Vec<(usize, usize)>,
    pub title_rows: Option<(u32, u32)>,
    pub title_cols: Option<(u32, u32)>,
    /// Heading sizes in sheet pixels (0 when headings are off).
    pub heading_w: f32,
    pub heading_h: f32,
    /// Manual page breaks apply (they don't when the sheet is fitted to pages).
    pub manual_breaks: bool,
}

impl SheetLayout {
    /// Physical points per 96-dpi pixel at the page scale (borders, padding, MoveOnly/Absolute objects).
    pub(super) fn k(&self) -> f32 {
        PX_TO_PT * self.scale
    }
    /// Horizontal points per sheet column pixel at the page scale.
    pub(super) fn kx(&self) -> f32 {
        self.col_px_to_pt * self.scale
    }
    /// Vertical points per sheet row pixel at the page scale.
    pub(super) fn ky(&self) -> f32 {
        self.row_px_to_pt * self.scale
    }
    /// Effective print height of `row` in 96-dpi sheet pixels (auto-fitted for non-custom rows).
    pub(super) fn row_height(&self, sh: &Sheet, row: u32) -> f32 {
        if let Ok(i) = self.row_deltas.binary_search_by_key(&row, |e| e.0) {
            return self.row_deltas[i].1;
        }
        sh.row_height(row)
    }
    /// Effective top offset of `row` in 96-dpi sheet pixels, including auto-fitted row deltas.
    pub(super) fn row_top(&self, sh: &Sheet, row: u32) -> f64 {
        let idx = self.row_deltas.partition_point(|e| e.0 < row);
        let delta = if idx > 0 { self.row_deltas[idx - 1].2 } else { 0.0 };
        sh.row_top(row) + delta
    }
    pub(crate) fn avail(&self) -> (f32, f32) {
        let [l, r, t, b, ..] = self.margins;
        ((self.page_w - l - r - 2.0 * GRID_EDGE_INSET_PT).max(36.0), (self.page_h - t - b - 2.0 * GRID_EDGE_INSET_PT).max(36.0))
    }
    /// Left and right margins, in points, that header and footer sections line up with: the
    /// page's side margins with Align with Page Margins on, otherwise 0.7 inch on each side, at
    /// most a quarter of the page width.
    pub(crate) fn hf_sides(&self) -> (f32, f32) {
        if self.settings.hf_align_with_margins {
            (self.margins[0], self.margins[1])
        } else {
            let side = HF_FIXED_SIDE_MARGIN.min(self.page_w / 4.0);
            (side, side)
        }
    }
    /// Scale of header and footer text: the print scale with Scale with Document on, otherwise 1.
    pub(crate) fn hf_text_scale(&self) -> f32 {
        if self.settings.hf_scale_with_doc { self.scale } else { 1.0 }
    }
}

/// Options shared by the print commands.
#[derive(Clone, Debug, Default)]
pub struct PrintOptions {
    pub all_sheets: bool,
    pub range: Option<RangeRef>,
    pub fit_to_page: Option<bool>,
}

impl PrintOptions {
    pub(super) fn from_params(p: &Json, cmd: &str) -> Result<PrintOptions> {
        let all_sheets = match str_param(p, "sheets") {
            None | Some("active") => false,
            Some("all") | Some("workbook") => true,
            Some(other) => return Err(bad(cmd, format!("`sheets` must be active or all, not `{other}`"))),
        };
        let range = match str_param(p, "range").map(str::trim).filter(|r| !r.is_empty()) {
            Some(r) => Some(RangeRef::parse(split_sheet(r).1).ok_or_else(|| bad(cmd, format!("not a range: `{r}`")))?),
            None => None,
        };
        Ok(PrintOptions { all_sheets, range, fit_to_page: bool_param(p, "fitToPage") })
    }
}

/// Excel's printer column-width scale (points per model column pixel at 100% scale).
///
/// Model column widths are stored in 96-dpi pixels assuming a 7-px screen MDW (`col_px ≈ chars × 7`).
/// When Windows Excel prints (600-dpi device), it scales stored column widths using the workbook's
/// Normal style font (`StyleId::DEFAULT`) maximum digit width measured at 600 dpi:
/// `lfHeight = round(size_pt × 600 / 72)`, `mdw_dots = round(lfHeight × digit_advance / upm)`,
/// `mdw_pt = mdw_dots × 72 / 600`.
pub(crate) fn excel_col_px_to_pt(wb: &Workbook) -> f32 {
    let font = &wb.styles.get(StyleId::DEFAULT).font;
    let size = if font.size.is_finite() && font.size > 0.0 { font.size.clamp(1.0, 409.0) } else { 11.0 };
    let lower = font.name.trim().to_ascii_lowercase();
    let digit_ratio: f32 = match lower.as_str() {
        "calibri" | "calibri light" | "carlito" => 1038.0 / 2048.0,
        "arial" | "arial narrow" | "helvetica" | "liberation sans" | "arimo" => 1139.0 / 2048.0,
        "aptos" | "aptos narrow" => 1113.0 / 2048.0,
        "segoe ui" => 1104.0 / 2048.0,
        "times new roman" | "times" | "liberation serif" => 1024.0 / 2048.0,
        "verdana" => 1302.0 / 2048.0,
        "tahoma" => 1119.0 / 2048.0,
        "courier new" | "courier" => 1229.0 / 2048.0,
        _ => Font::styled(Font::family_for(&font.name), false, false).char_width('0') as f32 / 1000.0,
    };
    let lf_height = (size * (600.0 / 72.0)).round().max(1.0);
    let mdw_dots = (lf_height * digit_ratio).round().max(1.0);
    let mdw_pt = mdw_dots * (72.0 / 600.0);
    mdw_pt / 7.0
}

/// Excel's printer vertical row-height scale (points per 96-dpi sheet row pixel at 100% scale).
///
/// Windows Excel prints row heights using the ratio of the workbook Normal font's 600-dpi
/// printer standard row height to its 96-dpi screen row height:
/// - Arial 10 pt:   103 dots (12.36 pt) / 17 px => 0.72706 pt/px (~0.9694 × 0.75)
/// - Calibri 11 pt: 121 dots (14.52 pt) / 20 px => 0.72600 pt/px (~0.9680 × 0.75)
pub(crate) fn excel_row_px_to_pt(wb: &Workbook) -> f32 {
    let font = &wb.styles.get(StyleId::DEFAULT).font;
    let size = if font.size.is_finite() && font.size > 0.0 { font.size.clamp(1.0, 409.0) } else { 11.0 };
    let lower = font.name.trim().to_ascii_lowercase();
    let (ad96, ad600, pad96, pad600): (f32, f32, f32, f32) = match lower.as_str() {
        "arial" | "arial narrow" | "helvetica" | "liberation sans" | "arimo" => (2288.0 / 2048.0, 2355.0 / 2048.0, 2.0, 8.0),
        "calibri" | "calibri light" | "carlito" => (2500.0 / 2048.0, 2500.0 / 2048.0, 2.0, 9.0),
        "segoe ui" => (2724.0 / 2048.0, 2724.0 / 2048.0, 2.0, 9.0),
        "verdana" => (2489.0 / 2048.0, 2489.0 / 2048.0, 2.0, 9.0),
        "tahoma" => (2472.0 / 2048.0, 2472.0 / 2048.0, 2.0, 9.0),
        "times new roman" | "times" | "liberation serif" => (2268.0 / 2048.0, 2268.0 / 2048.0, 2.0, 9.0),
        _ => return PX_TO_PT * (121.0 / 125.0),
    };
    let lf96 = (size * (96.0 / 72.0)).round().max(1.0);
    let px96 = ((lf96 * ad96).round() + pad96).max(1.0);
    let lf600 = (size * (600.0 / 72.0)).round().max(1.0);
    let dots600 = ((lf600 * ad600).round() + pad600).max(1.0);
    (dots600 * (72.0 / 600.0)) / px96
}

/// Windows 96-dpi single-line auto-fit row height in pixels for a cell font.
fn font_autofit_row_px(font: &gridcraft_model::Font) -> f32 {
    let size = if font.size.is_finite() && font.size > 0.0 { font.size.clamp(1.0, 409.0) } else { 11.0 };
    let lower = font.name.trim().to_ascii_lowercase();
    let ad96: f32 = match lower.as_str() {
        "segoe ui" => 2724.0 / 2048.0,
        "calibri" | "calibri light" | "carlito" => 2500.0 / 2048.0,
        "verdana" => 2489.0 / 2048.0,
        "tahoma" => 2472.0 / 2048.0,
        "arial" | "arial narrow" | "helvetica" | "liberation sans" | "arimo" => 2288.0 / 2048.0,
        "times new roman" | "times" | "liberation serif" => 2268.0 / 2048.0,
        _ => 2500.0 / 2048.0,
    };
    let lf96 = (size * (96.0 / 72.0)).round().max(1.0);
    ((lf96 * ad96).round() + 2.0).max(1.0)
}

/// Computes auto-fit row height deltas for non-custom, non-hidden rows in `0..=end_row` whose
/// content cells require a taller Windows 96-dpi single-line height than stored.
fn compute_row_deltas(wb: &Workbook, sh: &Sheet, end_row: u32) -> Vec<(u32, f32, f64)> {
    let mut max_px: std::collections::BTreeMap<u32, f32> = std::collections::BTreeMap::new();
    for (&row, info) in sh.rows.range(0..=end_row) {
        if info.hidden || info.custom {
            continue;
        }
        if let Some(sid) = info.style {
            let px = font_autofit_row_px(&wb.styles.get(sid).font);
            let e = max_px.entry(row).or_insert(0.0);
            if px > *e {
                *e = px;
            }
        }
    }
    let scan = RangeRef::new(CellRef::new(0, 0), CellRef::new(end_row, MAX_COLS - 1));
    for (pos, cell) in sh.cells.iter_range(scan) {
        if !cell.has_content() {
            continue;
        }
        if sh.rows.get(&pos.row).is_some_and(|info| info.hidden || info.custom) {
            continue;
        }
        let px = font_autofit_row_px(&wb.styles.get(sh.style_id(pos)).font);
        let e = max_px.entry(pos.row).or_insert(0.0);
        if px > *e {
            *e = px;
        }
    }
    let mut out = Vec::new();
    let mut cum = 0.0f64;
    for (row, auto_px) in max_px {
        let stored = sh.row_height(row);
        if auto_px > stored + 0.01 {
            cum += f64::from(auto_px - stored);
            out.push((row, auto_px, cum));
        }
    }
    out
}

/// True when `sid` paints something visible on paper even for an empty cell: a visible fill,
/// any border, or a cell checkbox format.
fn style_ref_is_printable(wb: &Workbook, style: &super::Style) -> bool {
    let has_fill = match style.fill.pattern {
        PatternType::None => false,
        PatternType::Solid => style.fill.fg.resolve(&wb.theme).is_some(),
        _ => true,
    };
    has_fill || style.borders != Borders::default() || style.num_fmt.as_str() == "checkbox"
}

fn style_is_printable(wb: &Workbook, sid: StyleId) -> bool {
    style_ref_is_printable(wb, wb.styles.get(sid))
}

fn cf_rule_can_paint(wb: &Workbook, rule: &gridcraft_model::CfRule) -> bool {
    match rule {
        gridcraft_model::CfRule::ColorScale { .. } | gridcraft_model::CfRule::DataBar { .. } => true,
        _ => rule.style().is_some_and(|s| style_ref_is_printable(wb, s)),
    }
}

fn cf_look_is_printable(wb: &Workbook, look: &crate::cf::CfLook) -> bool {
    look.scale_fill.is_some() || look.bar.is_some() || look.style.as_ref().is_some_and(|s| style_ref_is_printable(wb, s))
}

fn union_anchor(sh: &Sheet, r: &mut Option<RangeRef>, a: &Anchor) {
    let x = sh.col_left(a.cell.col) + a.dx as f64;
    let y = sh.row_top(a.cell.row) + a.dy as f64;
    let end_x = if a.width > 0.0 { (x + a.width as f64 - 0.01).max(x) } else { x };
    let end_y = if a.height > 0.0 { (y + a.height as f64 - 0.01).max(y) } else { y };
    let end = CellRef::new(sh.row_at(end_y), sh.col_at(end_x));
    let ar = RangeRef::new(a.cell, end);
    *r = Some(r.map_or(ar, |u| u.union(&ar)));
}

/// Printable cell range without workbook styles (legacy public signature).
pub fn print_range(sh: &Sheet) -> Option<RangeRef> {
    if let Some(a) = sh.print.print_area {
        return Some(trim_full_used(sh.used_range(), a));
    }
    let mut r = sh.used_range();
    for c in &sh.charts {
        union_anchor(sh, &mut r, &c.anchor);
    }
    for i in &sh.images {
        union_anchor(sh, &mut r, &i.anchor);
    }
    for s in &sh.shapes {
        union_anchor(sh, &mut r, &s.anchor);
    }
    r
}

/// The range a sheet prints, matching Excel: its explicit print area, else the tightest range
/// covering cells with values/formulas/visible fills/visible borders, spills, cell pictures,
/// sparklines, tables, printable merges, and floating objects. Empty cells carrying only font,
/// alignment, or number-format styling do not expand the print range.
pub(crate) fn sheet_print_range(wb: &Workbook, si: usize, sh: &Sheet) -> Option<RangeRef> {
    let mut cf = crate::cf::CfCache::new();
    sheet_print_range_with_cf(wb, si, sh, &mut cf).0
}

type OverflowSpan = (u32, u32, u32);

fn sheet_print_range_with_cf(wb: &Workbook, si: usize, sh: &Sheet, cf: &mut crate::cf::CfCache) -> (Option<RangeRef>, Vec<OverflowSpan>) {
    if let Some(a) = sh.print.print_area {
        let (used, overflows) = printable_cells_range(wb, si, sh, cf);
        return (Some(trim_full_used(used, a)), overflows);
    }
    let (mut r, overflows) = printable_cells_range(wb, si, sh, cf);
    for c in &sh.charts {
        union_anchor(sh, &mut r, &c.anchor);
    }
    for i in &sh.images {
        union_anchor(sh, &mut r, &i.anchor);
    }
    for s in &sh.shapes {
        union_anchor(sh, &mut r, &s.anchor);
    }
    (r, overflows)
}

fn printable_cells_range(wb: &Workbook, si: usize, sh: &Sheet, cf: &mut crate::cf::CfCache) -> (Option<RangeRef>, Vec<OverflowSpan>) {
    let mut out: Option<RangeRef> = None;
    let mut extend = |rng: RangeRef| {
        out = Some(out.map_or(rng, |u| u.union(&rng)));
    };
    for (pos, cell) in sh.cells.iter() {
        if sh.is_row_hidden(pos.row) || sh.is_col_hidden(pos.col) {
            continue;
        }
        if cell.has_content() || style_is_printable(wb, sh.style_id(pos)) {
            extend(RangeRef::cell(pos));
        }
    }
    // A row with a visible row-level fill or border paints across the sheet's used columns
    // (matching Excel for full-width header banners set via `<row s="...">`).
    if let Some(used) = sh.used_range() {
        for (&row, info) in &sh.rows {
            if info.style.is_some_and(|sid| style_is_printable(wb, sid)) {
                extend(RangeRef::new(CellRef::new(row, used.start.col), CellRef::new(row, used.end.col)));
            }
        }
    }
    for sr in sh.spill_ranges.values() {
        extend(*sr);
    }
    for &pos in sh.spill.keys() {
        extend(RangeRef::cell(pos));
    }
    for &pos in sh.cell_pictures.keys() {
        extend(RangeRef::cell(pos));
    }
    for sp in &sh.sparklines {
        extend(RangeRef::cell(sp.cell));
    }
    for t in &sh.tables {
        extend(t.range);
    }
    for m in &sh.merges {
        let anchor_printable = sh.cells.get(m.start).is_some_and(|c| c.has_content())
            || style_is_printable(wb, sh.style_id(m.start))
            || style_is_printable(wb, sh.style_id(m.end));
        if anchor_printable {
            extend(*m);
        }
    }
    extend_printable_cf_cells(wb, si, sh, cf, &mut extend);
    let overflows = if let Some(cur_out) = out {
        let (ovs, max_col) = collect_text_overflows(wb, sh, cur_out.end.col);
        if max_col > cur_out.end.col {
            out = Some(cur_out.union(&RangeRef::cell(CellRef::new(cur_out.start.row, max_col))));
        }
        ovs
    } else {
        Vec::new()
    };
    (out, overflows)
}

/// Includes cells whose conditional format produces a visible fill, border, colour scale, or data bar.
fn extend_printable_cf_cells(wb: &Workbook, si: usize, sh: &Sheet, cf: &mut crate::cf::CfCache, extend: &mut impl FnMut(RangeRef)) {
    if sh.cond_formats.is_empty() {
        return;
    }
    for cond in &sh.cond_formats {
        if !cf_rule_can_paint(wb, &cond.rule) {
            continue;
        }
        for &rng in &cond.ranges {
            if rng.count() <= DENSE_BLOCK {
                for pos in rng.iter() {
                    if cf.look(wb, si, pos).is_some_and(|l| cf_look_is_printable(wb, &l)) {
                        extend(RangeRef::cell(pos));
                    }
                }
            } else {
                for (pos, cell) in sh.cells.iter_range(rng) {
                    if !cell.has_content() && cf.look(wb, si, pos).is_some_and(|l| cf_look_is_printable(wb, &l)) {
                        extend(RangeRef::cell(pos));
                    }
                }
            }
        }
    }
}

/// Collects rightward overflow spans of unwrapped text cells into empty neighbours and returns
/// the furthest column reached beyond `initial_max_col`.
fn collect_text_overflows(wb: &Workbook, sh: &Sheet, initial_max_col: u32) -> (Vec<OverflowSpan>, u32) {
    let col_kx = excel_col_px_to_pt(wb);
    let pad = 2.0 * PX_TO_PT;
    let mut overflows = Vec::new();
    let mut max_col = initial_max_col;
    for (pos, cell) in sh.cells.iter() {
        if sh.is_row_hidden(pos.row) || sh.is_col_hidden(pos.col) {
            continue;
        }
        if !matches!(&cell.value, gridcraft_core::Value::Text(_)) && !sh.show_formulas {
            continue;
        }
        let st = wb.styles.get(sh.style_id(pos));
        if st.align.wrap
            || st.align.shrink
            || st.align.rotation != 0
            || matches!(st.align.h, super::HAlign::Fill | super::HAlign::Right | super::HAlign::CenterAcross)
        {
            continue;
        }
        let cell_w = sh.col_width(pos.col) * col_kx;
        let text = if sh.show_formulas {
            crate::display::cell_text(wb, sh, pos)
        } else {
            crate::display::value_text_as(wb, &cell.value, st.num_fmt.as_str())
        };
        let text = text.replace(['\n', '\r'], " ");
        let size = st.font.size.clamp(1.0, 409.0);
        if text.is_empty() || (text.len() as f32) * size * 0.85 <= cell_w || sh.merges.iter().any(|m| m.contains(pos)) {
            continue;
        }
        let font = Font::styled(Font::family_for(&st.font.name), st.font.bold, st.font.italic);
        let indent = st.align.indent as f32 * 9.0 * PX_TO_PT;
        let excess = gridcraft_pdf::text_width(font, size, &text) + 2.0 * pad + indent - cell_w;
        if excess <= 0.5 {
            continue;
        }
        let mut rem = if matches!(st.align.h, super::HAlign::Center | super::HAlign::Distributed) { excess / 2.0 } else { excess };
        let mut col = pos.col + 1;
        let mut reached = pos.col;
        while rem > 0.5 && col < MAX_COLS {
            let nc = CellRef::new(pos.row, col);
            if sh.is_col_hidden(col) {
                col += 1;
                continue;
            }
            if sh.value_ref(nc).is_some_and(|v| !v.is_empty()) || sh.merges.iter().any(|m| m.contains(nc)) {
                break;
            }
            reached = col;
            rem -= sh.col_width(col) * col_kx;
            col += 1;
        }
        if reached > pos.col {
            overflows.push((pos.row, pos.col, reached));
            max_col = max_col.max(reached);
        }
    }
    (overflows, max_col)
}

/// True when page `(rband, cband)` contains at least one printable cell, visible style, merge,
/// table, conditional format, overflowing text span, or floating object. Used to trim completely
/// blank trailing pages.
fn page_has_content(
    wb: &Workbook,
    si: usize,
    sh: &Sheet,
    rband: (u32, u32),
    cband: (u32, u32),
    overflows: &[OverflowSpan],
    cf: &mut crate::cf::CfCache,
) -> bool {
    if overflows.iter().any(|&(r, c0, c1)| r >= rband.0 && r <= rband.1 && c1 >= cband.0 && c0 <= cband.1) {
        return true;
    }
    let block = RangeRef::new(CellRef::new(rband.0, cband.0), CellRef::new(rband.1, cband.1));
    for (pos, cell) in sh.cells.iter_range(block) {
        if sh.is_row_hidden(pos.row) || sh.is_col_hidden(pos.col) {
            continue;
        }
        if cell.has_content() || style_is_printable(wb, sh.style_id(pos)) {
            return true;
        }
    }
    for (_, info) in sh.rows.range(rband.0..=rband.1) {
        if info.style.is_some_and(|sid| style_is_printable(wb, sid)) {
            return true;
        }
    }
    if sh.spill.range(block.start..=block.end).any(|(c, _)| block.contains(*c)) {
        return true;
    }
    if sh.cell_pictures.range(block.start..=block.end).any(|(c, _)| block.contains(*c)) {
        return true;
    }
    if sh.sparklines.iter().any(|sp| block.contains(sp.cell)) {
        return true;
    }
    if sh.tables.iter().any(|t| t.range.intersects(&block)) {
        return true;
    }
    if sh.merges.iter().any(|m| {
        m.intersects(&block)
            && (sh.cells.get(m.start).is_some_and(|c| c.has_content())
                || style_is_printable(wb, sh.style_id(m.start))
                || style_is_printable(wb, sh.style_id(m.end)))
    }) {
        return true;
    }
    if !sh.cond_formats.is_empty() {
        for cond in &sh.cond_formats {
            if !cf_rule_can_paint(wb, &cond.rule) {
                continue;
            }
            for &rng in &cond.ranges {
                let Some(sub) = rng.intersection(&block) else { continue };
                if sub.count() <= DENSE_BLOCK {
                    if sub.iter().any(|pos| cf.look(wb, si, pos).is_some_and(|l| cf_look_is_printable(wb, &l))) {
                        return true;
                    }
                } else if sh.cells.iter_range(sub).any(|(pos, _)| cf.look(wb, si, pos).is_some_and(|l| cf_look_is_printable(wb, &l))) {
                    return true;
                }
            }
        }
    }
    let (sx0, sy0) = (sh.col_left(block.start.col), sh.row_top(block.start.row));
    let (sx1, sy1) = (sh.col_left(block.end.col.saturating_add(1)), sh.row_top(block.end.row.saturating_add(1)));
    let hits_anchor = |a: &Anchor| {
        let x = sh.col_left(a.cell.col) + a.dx as f64;
        let y = sh.row_top(a.cell.row) + a.dy as f64;
        a.width > 0.0 && a.height > 0.0 && x < sx1 && x + a.width as f64 > sx0 && y < sy1 && y + a.height as f64 > sy0
    };
    sh.charts.iter().any(|c| hits_anchor(&c.anchor))
        || sh.images.iter().any(|i| hits_anchor(&i.anchor))
        || sh.shapes.iter().any(|s| hits_anchor(&s.anchor))
}

/// Whole rows/columns are trimmed to the printable/used range.
fn trim_full_used(used: Option<RangeRef>, r: RangeRef) -> RangeRef {
    if !(r.is_full_cols() || r.is_full_rows()) {
        return r;
    }
    match used {
        Some(u) => r.intersection(&RangeRef::new(CellRef::new(0, 0), u.end)).unwrap_or(RangeRef::cell(r.start)),
        None => RangeRef::cell(r.start),
    }
}

/// Splits `start..=end` into bands whose sizes fit `avail(band_start)`; `breaks` force a new
/// band before a line. Stops after `cap` bands.
pub(super) fn bands(start: u32, end: u32, size: impl Fn(u32) -> f32, avail: impl Fn(u32) -> f32, breaks: &[u32], cap: usize) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    let mut cur = start;
    let mut used = 0.0f32;
    let mut room = avail(start);
    let mut line = start;
    loop {
        let sz = size(line);
        let manual = line > cur && breaks.binary_search(&line).is_ok();
        if line > cur && (manual || used + sz > room + 0.01) {
            out.push((cur, line - 1));
            if out.len() >= cap {
                return out;
            }
            cur = line;
            used = 0.0;
            room = avail(line);
        }
        used += sz;
        if line >= end {
            break;
        }
        line += 1;
    }
    out.push((cur, end));
    out
}

fn title_span(t: Option<(u32, u32)>, max: u32) -> Option<(u32, u32)> {
    let (a, b) = t?;
    let (a, b) = (a.min(b), a.max(b));
    if a >= max {
        return None;
    }
    // Repeated titles are capped (Excel needs them to fit on a page anyway).
    Some((a, b.min(max - 1).min(a.saturating_add(499))))
}

/// Paginates one sheet.
pub fn layout_sheet(wb: &Workbook, si: usize, opts: &PrintOptions, range_override: Option<RangeRef>) -> Option<SheetLayout> {
    let sh = wb.sheet(si)?;
    let st = sh.print.clone();
    let (mut pw, mut ph) = paper_size(&st.paper);
    if st.orientation == Orientation::Landscape {
        std::mem::swap(&mut pw, &mut ph);
    }
    let inch = |i: usize| st.margins.get(i).copied().filter(|v| v.is_finite()).unwrap_or(0.5).clamp(0.0, 20.0) * 72.0;
    let mut margins = [inch(0), inch(1), inch(2), inch(3), inch(4), inch(5)];
    // Keep at least an inch of printable space.
    for (a, b, total) in [(0usize, 1usize, pw), (2, 3, ph)] {
        let sum = margins[a] + margins[b];
        if total - sum < 72.0 {
            let f = ((total - 72.0).max(0.0)) / sum.max(1.0);
            margins[a] *= f;
            margins[b] *= f;
        }
    }
    let mut cf = crate::cf::CfCache::new();
    let (area, overflows) = match range_override {
        Some(r) => {
            let (used, ov) = printable_cells_range(wb, si, sh, &mut cf);
            (trim_full_used(used, r), ov)
        }
        None => {
            let (used, ov) = sheet_print_range_with_cf(wb, si, sh, &mut cf);
            (used.unwrap_or(RangeRef::cell(CellRef::new(0, 0))), ov)
        }
    };
    // A print area from a file or JSON can name coordinates past the grid; pagination walks every
    // line of the area (zero-height rows never fill a band), so it is held to the grid first.
    let area = RangeRef::new(
        CellRef::new(area.start.row.min(MAX_ROWS - 1), area.start.col.min(MAX_COLS - 1)),
        CellRef::new(area.end.row.min(MAX_ROWS - 1), area.end.col.min(MAX_COLS - 1)),
    );
    let title_rows = title_span(st.title_rows, MAX_ROWS);
    let title_cols = title_span(st.title_cols, MAX_COLS);
    let digits = format!("{}", area.end.row + 1).len() as f32;
    let (heading_w, heading_h) = if st.headings { (10.0 + 7.0 * digits, sh.default_row_height.clamp(10.0, 60.0)) } else { (0.0, 0.0) };
    let fit = match opts.fit_to_page {
        Some(true) => Some((1u16, 1u16)),
        Some(false) => None,
        None => st.fit_to.filter(|(w, h)| *w > 0 || *h > 0),
    };
    let col_px_to_pt = excel_col_px_to_pt(wb);
    let row_px_to_pt = excel_row_px_to_pt(wb);
    let delta_end = title_rows.map_or(area.end.row, |(_, b)| area.end.row.max(b));
    let row_deltas = compute_row_deltas(wb, sh, delta_end);
    let mut lay = SheetLayout {
        sheet: si,
        settings: st.clone(),
        page_w: pw,
        page_h: ph,
        margins,
        col_px_to_pt,
        row_px_to_pt,
        row_deltas,
        scale: (st.scale.clamp(10, 400) as f32) / 100.0,
        area,
        row_bands: vec![],
        col_bands: vec![],
        pages: vec![],
        title_rows,
        title_cols,
        heading_w,
        heading_h,
        manual_breaks: fit.is_none(),
    };
    let can_trim = !lay.settings.gridlines
        && !lay.settings.headings
        && !lay.settings.has_header_footer()
        && sh.print.print_area.is_none()
        && range_override.is_none()
        && lay.title_rows.is_none()
        && lay.title_cols.is_none();
    let mut paginate = |lay: &mut SheetLayout, manual: bool| {
        lay.manual_breaks = manual;
        (lay.row_bands, lay.col_bands) = split_bands(sh, lay, MAX_PAGES);
        lay.pages = page_sequence(lay.row_bands.len(), lay.col_bands.len(), lay.settings.page_order, MAX_PAGES);
        let truncated = lay.row_bands.len().saturating_mul(lay.col_bands.len()) > lay.pages.len();
        if !truncated && can_trim {
            while lay.pages.len() > 1 {
                let Some(&(ri, ci)) = lay.pages.last() else { break };
                let (Some(&rb), Some(&cb)) = (lay.row_bands.get(ri), lay.col_bands.get(ci)) else { break };
                if page_has_content(wb, si, sh, rb, cb, &overflows, &mut cf) {
                    break;
                }
                lay.pages.pop();
            }
        }
    };
    match fit {
        Some((fw, fh)) => {
            let total_w: f32 = (area.start.col..=area.end.col).map(|c| sh.col_width(c)).sum::<f32>() + heading_w;
            let total_h: f32 = if area.height() > 200_000 {
                f32::INFINITY
            } else {
                (area.start.row..=area.end.row).map(|r| lay.row_height(sh, r)).sum::<f32>() + heading_h
            };
            let (aw, ah) = lay.avail();
            let sw = if fw > 0 { aw * fw as f32 / (total_w * col_px_to_pt).max(1.0) } else { f32::INFINITY };
            let sh_ = if fh > 0 { ah * fh as f32 / (total_h * row_px_to_pt).max(1.0) } else { f32::INFINITY };
            lay.scale = sw.min(sh_).min(1.0).clamp(0.1, 1.0);
            for _ in 0..40 {
                paginate(&mut lay, false);
                let ok_w = fw == 0 || lay.col_bands.len() <= fw as usize;
                let ok_h = fh == 0 || lay.row_bands.len() <= fh as usize;
                if (ok_w && ok_h) || lay.scale <= 0.1 {
                    break;
                }
                lay.scale = (lay.scale * 0.97).max(0.1);
            }
        }
        None => paginate(&mut lay, true),
    }
    Some(lay)
}

/// The pages of a sheet `rows` pages tall and `cols` pages wide as (row band, column band), in
/// print order, at most `cap` of them: down each column of pages, then over to the next (Excel's
/// default), or over each row of pages, then down to the next.
pub(crate) fn page_sequence(rows: usize, cols: usize, order: PageOrder, cap: usize) -> Vec<(usize, usize)> {
    let (outer, inner) = match order {
        PageOrder::DownThenOver => (cols, rows),
        PageOrder::OverThenDown => (rows, cols),
    };
    (0..outer)
        .flat_map(|o| {
            (0..inner).map(move |i| match order {
                PageOrder::DownThenOver => (i, o),
                PageOrder::OverThenDown => (o, i),
            })
        })
        .take(cap)
        .collect()
}

/// Inclusive-start/end index spans of the pages along one axis.
pub(crate) type Bands = Vec<(u32, u32)>;

/// Splits a layout's area into row and column bands that fit its pages (at most `cap` of each),
/// honouring manual breaks (when `lay.manual_breaks`), print headings and repeated titles. The
/// one pagination rule shared by printing, PDF export, Page Break Preview and Page Layout view.
pub(crate) fn split_bands(sh: &Sheet, lay: &SheetLayout, cap: usize) -> (Bands, Bands) {
    let kx = lay.kx();
    let ky = lay.ky();
    let (aw, ah) = lay.avail();
    let (aw, ah) = (aw / kx, ah / ky);
    let st = &lay.settings;
    let mut rb: Vec<u32> = if lay.manual_breaks { st.row_breaks.clone() } else { vec![] };
    let mut cb: Vec<u32> = if lay.manual_breaks { st.col_breaks.clone() } else { vec![] };
    rb.sort_unstable();
    cb.sort_unstable();
    let (title_rows, title_cols) = (lay.title_rows, lay.title_cols);
    let title_h = |start: u32| -> f32 {
        match title_rows {
            Some((a, b)) if a < start => (a..=b.min(start - 1)).map(|r| lay.row_height(sh, r)).sum(),
            _ => 0.0,
        }
    };
    let title_w = |start: u32| -> f32 {
        match title_cols {
            Some((a, b)) if a < start => (a..=b.min(start - 1)).map(|c| sh.col_width(c)).sum(),
            _ => 0.0,
        }
    };
    let area = lay.area;
    let mut rows = bands(area.start.row, area.end.row, |r| lay.row_height(sh, r), |start| (ah - lay.heading_h - title_h(start)).max(1.0), &rb, cap);
    let mut cols = bands(area.start.col, area.end.col, |c| sh.col_width(c), |start| (aw - lay.heading_w - title_w(start)).max(1.0), &cb, cap);
    while rows.len() > 1 && rows.last().is_some_and(|&(a, b)| (a..=b).all(|r| lay.row_height(sh, r) <= 0.0)) {
        rows.pop();
    }
    while cols.len() > 1 && cols.last().is_some_and(|&(a, b)| (a..=b).all(|c| sh.col_width(c) <= 0.0)) {
        cols.pop();
    }
    (rows, cols)
}

/// Lays out every sheet a print command covers.
pub fn layouts(wb: &Workbook, opts: &PrintOptions) -> Vec<SheetLayout> {
    let mut out = Vec::new();
    let mut pages = 0usize;
    let sheets: Vec<usize> = if opts.all_sheets {
        (0..wb.sheets.len()).filter(|i| wb.sheet(*i).is_some_and(|s| s.visibility == Visibility::Visible)).collect()
    } else {
        vec![wb.active_sheet()]
    };
    for si in sheets {
        if pages >= MAX_PAGES {
            break;
        }
        let ovr = if si == wb.active_sheet() { opts.range } else { None };
        if opts.all_sheets && ovr.is_none() && wb.sheet(si).is_some_and(|s| sheet_print_range(wb, si, s).is_none()) {
            continue;
        }
        if let Some(mut l) = layout_sheet(wb, si, opts, ovr) {
            l.pages.truncate(MAX_PAGES - pages);
            pages += l.pages.len();
            out.push(l);
        }
    }
    if out.is_empty() && opts.all_sheets {
        out.extend(layout_sheet(wb, wb.active_sheet(), opts, None));
    }
    out
}

/// Segments of a page along one axis: repeated titles (before the band) then the band.
pub(super) fn segments(titles: Option<(u32, u32)>, band: (u32, u32)) -> Vec<(u32, u32)> {
    let mut v = Vec::new();
    if let Some((a, b)) = titles
        && a < band.0
    {
        v.push((a, b.min(band.0 - 1)));
    }
    v.push(band);
    v
}

pub(super) fn seg_size(lay: &SheetLayout, sh: &Sheet, seg: (u32, u32), rows: bool) -> f32 {
    if rows { (seg.0..=seg.1).map(|r| lay.row_height(sh, r)).sum() } else { (seg.0..=seg.1).map(|c| sh.col_width(c)).sum() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_print_area_past_the_grid_is_held_to_the_grid() {
        let mut wb = Workbook::new();
        let sh = wb.sheet_mut(0).unwrap();
        sh.default_row_height = 0.0;
        sh.print.print_area = Some(RangeRef { start: CellRef::new(0, 0), end: CellRef::new(u32::MAX, u32::MAX) });
        let lay = layout_sheet(&wb, 0, &PrintOptions::default(), None).unwrap();
        assert_eq!((lay.area.end.row, lay.area.end.col), (MAX_ROWS - 1, MAX_COLS - 1));
        assert!(lay.pages.len() <= MAX_PAGES);
    }
}
