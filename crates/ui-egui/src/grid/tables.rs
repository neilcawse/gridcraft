use super::geo::Geo;
use super::{CellRef, Painter, Pos2, RangeRef, Rect, Sheet, Stroke, StrokeKind, Tokens, pos2, vec2};

pub fn filter_button_rect(geo: &Geo, sh: &Sheet, c: CellRef) -> Rect {
    let rect = geo.cell_rect(sh, c);
    // Keep the button within the same header slot that AutoFit reserves.
    let pad = 1.5 * geo.z;
    let s = (rect.height() - 2.0).clamp(10.0, 16.0).min((sh.filter_button_space(c) * geo.z - pad).max(0.0));
    let by = if rect.height() > 24.0 * geo.z { (rect.bottom() - s - pad).max(rect.top() + 1.0) } else { rect.center().y - s / 2.0 };
    Rect::from_min_size(pos2(rect.right() - s - pad, by), vec2(s, s))
}

/// A Table column heading that shows the column's header name in place of its letters.
#[derive(Clone, Debug)]
pub struct TableHeading {
    pub col: u32,
    /// The table's header row, which the filter menu is keyed by.
    pub header_row: u32,
    pub name: String,
    /// The heading slot over the column (logical coordinates).
    pub rect: Rect,
    /// The filter button, when the table shows filter buttons.
    pub button: Option<Rect>,
    /// The column is filtered (the button shows the funnel).
    pub filtered: bool,
}

/// The Table whose header names replace the column letters: the one holding the active cell,
/// with its header row scrolled out of view and some of its rows still in view (Excel, #1423).
pub fn heading_table<'a>(geo: &Geo, sh: &'a Sheet, active: CellRef) -> Option<&'a gridcraft_engine::model::Table> {
    // Page Layout view keeps the letters.
    if geo.paper.is_some() || geo.header_h <= 0.0 {
        return None;
    }
    let tb = sh.tables.iter().find(|t| t.header_row && t.range.contains(active))?;
    let header = tb.range.start.row;
    // A header row in the frozen rows is always on screen.
    if header < geo.fr {
        return None;
    }
    let top = geo.cells.top() + geo.frozen_h;
    let bottom = |r: u32| geo.y(sh, r) + sh.row_height(r) * geo.zy;
    (bottom(header) <= top + 0.5 && bottom(tb.range.end.row) > top + 0.5).then_some(tb)
}

/// The headings of `heading_table`'s visible columns.
pub fn table_headings(geo: &Geo, sh: &Sheet, active: CellRef) -> Vec<TableHeading> {
    let Some(tb) = heading_table(geo, sh, active) else { return Vec::new() };
    let af = sh.autofilter.as_ref().filter(|af| af.range == tb.range);
    let mut out = Vec::new();
    for col in tb.range.start.col..=tb.range.end.col {
        let w = sh.col_width(col) * geo.zx;
        let off = col - tb.range.start.col;
        let Some(column) = tb.columns.get(off as usize) else { continue };
        if w <= 0.0 {
            continue;
        }
        let rect = Rect::from_min_size(pos2(geo.x(sh, col), geo.head_top()), vec2(w, geo.header_h));
        // Clear of the 4-point column resize edge, and never wider than the column.
        let s = (rect.height() - 6.0).clamp(10.0, 16.0).min(w - 7.0);
        let button = (tb.filter_button && s > 0.0).then(|| Rect::from_min_size(pos2(rect.right() - s - 5.0, rect.center().y - s / 2.0), vec2(s, s)));
        let filtered = tb.filter.iter().any(|(o, _)| *o == off) || af.is_some_and(|af| af.criteria.iter().any(|(o, _)| *o == off));
        out.push(TableHeading { col, header_row: tb.range.start.row, name: column.name.clone(), rect, button, filtered });
    }
    out
}

pub(super) fn paint_filter_button(p: &Painter, r: Rect, filtered: bool) {
    paint_pivot_button(p, r, filtered, gridcraft_engine::model::PivotSort::None);
}

pub(super) fn paint_pivot_button(p: &Painter, r: Rect, filtered: bool, sort: gridcraft_engine::model::PivotSort) {
    let t = Tokens::get(p.ctx());
    let bg = if t.sheet_control_bg.r() > 240 { egui::Color32::from_rgb(238, 238, 238) } else { t.sheet_control_bg };
    let border = if t.sheet_control_border.r() > 190 { egui::Color32::from_rgb(180, 180, 180) } else { t.sheet_control_border };
    let fg = if t.sheet_control_bg.r() > 200 { egui::Color32::from_rgb(105, 105, 105) } else { t.sheet_control_text };
    p.rect_filled(r, 2.0, bg);
    p.rect_stroke(r, 2.0, Stroke::new(1.0, border), StrokeKind::Inside);
    if sort != gridcraft_engine::model::PivotSort::None {
        let lx = r.left() + r.width() * 0.16;
        let rx = r.left() + r.width() * 0.48;
        for (k, frac) in [(0, 1.0f32), (1, 0.72), (2, 0.45)] {
            let yy = r.top() + r.height() * (0.28 + k as f32 * 0.20);
            let bw = (rx - lx) * frac;
            p.line_segment([pos2(lx, yy), pos2(lx + bw, yy)], Stroke::new(1.15, fg));
        }
        let cx = r.left() + r.width() * 0.70;
        let cy = r.center().y;
        let tw = r.width() * 0.17;
        let th = r.height() * 0.15;
        p.add(egui::epaint::PathShape::convex_polygon(vec![pos2(cx - tw, cy - th), pos2(cx + tw, cy - th), pos2(cx, cy + th)], fg, Stroke::NONE));
    } else if filtered {
        crate::icons::paint_on_sheet(p, r.shrink(1.5), crate::icons::Icon::Filter, fg);
    } else {
        let c = r.center();
        let tw = r.width() * 0.24;
        let th = r.height() * 0.16;
        p.add(egui::epaint::PathShape::convex_polygon(
            vec![pos2(c.x - tw, c.y - th * 0.8), pos2(c.x + tw, c.y - th * 0.8), pos2(c.x, c.y + th * 1.1)],
            fg,
            Stroke::NONE,
        ));
    }
}

pub(super) fn is_filter_button(sh: &Sheet, geo: &Geo, p: Pos2) -> bool {
    let c = geo.cell_at(sh, p);
    sh.has_filter_button(c) && filter_button_rect(geo, sh, c).contains(p)
}

/// The table whose header cell (`row`, `col`) was clicked, not just the first table in the column.
pub(super) fn filter_table_range(sh: &Sheet, col: u32, row: u32) -> Option<RangeRef> {
    sh.tables.iter().find(|t| t.range.start.row == row && col >= t.range.start.col && col <= t.range.end.col).map(|t| t.range)
}
