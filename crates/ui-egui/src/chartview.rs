//! Floating objects on the sheet: charts (rendered by `gridcraft-chart` into primitives that
//! we paint with egui), pictures and shapes.

use egui::epaint::{PathShape, PathStroke};
use egui::{Align2, Color32, FontId, Painter, Pos2, Rect, Stroke, StrokeKind, pos2, vec2};
use gridcraft_chart::{HAlign, Prim, VAlign};
use gridcraft_engine::model::{Anchor, DrawingRef, ShapeKind, Sheet, Slicer, Workbook};
use std::collections::HashMap;
use std::sync::{Arc, Weak};

use crate::SheetApp;
use crate::grid::Geo;
use crate::theme::{self, Tokens};

/// The slicer's tiles, computed once per workbook version. Every change to a document, undoable or
/// not (recalculation, refresh), installs a new `Arc<Workbook>`; the cache holds a `Weak` to the one
/// it was built from, so a new version is always told apart (the allocation, and so its address,
/// stays reserved while the `Weak` lives) without a strong reference that would make the next edit's
/// `Arc::make_mut` copy the whole workbook.
fn cached_slicer_items<'a>(app: &'a mut SheetApp, wb: &Workbook, si: usize, sl: &Slicer) -> &'a [gridcraft_engine::pivot::SlicerTile] {
    let cur = app.session.active().map(|d| Arc::downgrade(&d.wb));
    let fresh = matches!((&app.grid.slicer_tiles, &cur), (Some((Some(built), _)), Some(cur)) if Weak::ptr_eq(built, cur));
    if !fresh {
        app.grid.slicer_tiles = None;
    }
    let (_, tiles) = app.grid.slicer_tiles.get_or_insert_with(|| (cur, HashMap::new()));
    tiles.entry((si, sl.id)).or_insert_with(|| gridcraft_engine::cmd::slicer::slicer_items(wb, si, sl))
}

fn rgba(c: [u8; 4]) -> Color32 {
    Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3])
}

struct EguiMeasure<'a> {
    painter: &'a Painter,
    z: f32,
}

impl gridcraft_chart::Measure for EguiMeasure<'_> {
    fn text_width(&self, text: &str, size: f32, bold: bool) -> f32 {
        let font = if bold { theme::ui_bold(size * self.z) } else { theme::ui_font(size * self.z) };
        self.painter.layout_no_wrap(text.to_string(), font, Color32::BLACK).size().x / self.z.max(0.01)
    }
}

/// Paints `chart` (not necessarily on the sheet: the Recommended Charts preview) scaled to fit
/// `rect`.
pub fn paint_chart(p: &Painter, rect: Rect, wb: &Workbook, sheet: usize, chart: &gridcraft_engine::model::Chart) {
    let (w, h) = (chart.anchor.width.max(1.0), chart.anchor.height.max(1.0));
    let z = (rect.width() / w).min(rect.height() / h);
    if !(z.is_finite() && z > 0.0) {
        return;
    }
    let origin = rect.center() - vec2(w * z, h * z) / 2.0;
    let data = gridcraft_chart::resolve(wb, sheet, chart);
    let m = EguiMeasure { painter: p, z };
    let prims = gridcraft_chart::render(chart, &data, w, h, &m);
    paint_prims(&p.with_clip_rect(rect.intersect(p.clip_rect())), origin, &prims, z);
}

/// Paints chart primitives at `origin`, scaled by `z`.
pub fn paint_prims(p: &Painter, origin: Pos2, prims: &[Prim], z: f32) {
    let pt = |x: f32, y: f32| pos2(origin.x + x * z, origin.y + y * z);
    for prim in prims {
        match prim {
            Prim::Rect { x, y, w, h, fill, stroke, radius } => {
                let r = Rect::from_min_size(pt(*x, *y), vec2(w * z, h * z));
                if let Some(f) = fill {
                    p.rect_filled(r, radius * z, rgba(*f));
                }
                if let Some((c, wd)) = stroke {
                    p.rect_stroke(r, radius * z, Stroke::new(wd * z.max(0.5), rgba(*c)), StrokeKind::Inside);
                }
            }
            Prim::Line { pts, color, width, dash } => {
                let v: Vec<Pos2> = pts.iter().map(|q| pt(q[0], q[1])).collect();
                if v.len() < 2 {
                    continue;
                }
                let lw = (width * z).max(1.0);
                if *dash {
                    p.extend(egui::Shape::dashed_line(&v, Stroke::new(lw, rgba(*color)), 4.0 * z, 3.0 * z));
                } else if v.len() == 2 && lw <= 1.25 && (v[0].y - v[1].y).abs() < 1e-3 {
                    p.hline(v[0].x.min(v[1].x)..=v[0].x.max(v[1].x), v[0].y, Stroke::new(1.0, rgba(*color)));
                } else if v.len() == 2 && lw <= 1.25 && (v[0].x - v[1].x).abs() < 1e-3 {
                    p.vline(v[0].x, v[0].y.min(v[1].y)..=v[0].y.max(v[1].y), Stroke::new(1.0, rgba(*color)));
                } else {
                    p.add(PathShape::line(v, PathStroke::new(lw, rgba(*color))));
                }
            }
            Prim::Polygon { pts, fill, stroke } => {
                let v: Vec<Pos2> = pts.iter().map(|q| pt(q[0], q[1])).collect();
                if v.len() < 3 {
                    continue;
                }
                // Polygons may be concave (areas): triangulate as a fan from the first point
                // when convex, else draw through a mesh of the closed path.
                let shape = PathShape { points: v.clone(), closed: true, fill: rgba(*fill), stroke: PathStroke::NONE };
                if is_convex(&v) {
                    p.add(shape);
                } else {
                    p.add(egui::Shape::mesh(area_mesh(&v, rgba(*fill))));
                }
                if let Some((c, w)) = stroke {
                    p.add(PathShape::closed_line(v, PathStroke::new(w * z.max(0.5), rgba(*c))));
                }
            }
            Prim::Wedge { cx, cy, r_outer, r_inner, a0, a1, fill, stroke } => {
                let c = pt(*cx, *cy);
                let steps = (((a1 - a0).abs() / 0.05).ceil() as usize).clamp(2, 400);
                let at = |a: f32, r: f32| pos2(c.x + r * z * a.sin(), c.y - r * z * a.cos());
                let mut mesh = egui::Mesh::default();
                let col = rgba(*fill);
                for i in 0..=steps {
                    let a = a0 + (a1 - a0) * i as f32 / steps as f32;
                    mesh.colored_vertex(at(a, *r_outer), col);
                    mesh.colored_vertex(if *r_inner > 0.0 { at(a, *r_inner) } else { c }, col);
                }
                for i in 0..steps as u32 {
                    let b = i * 2;
                    mesh.add_triangle(b, b + 1, b + 2);
                    mesh.add_triangle(b + 1, b + 3, b + 2);
                }
                p.add(egui::Shape::mesh(mesh));
                if let Some((sc, w)) = stroke {
                    let mut outline: Vec<Pos2> = (0..=steps).map(|i| at(a0 + (a1 - a0) * i as f32 / steps as f32, *r_outer)).collect();
                    if *r_inner > 0.0 {
                        outline.extend((0..=steps).rev().map(|i| at(a0 + (a1 - a0) * i as f32 / steps as f32, *r_inner)));
                    } else {
                        outline.push(c);
                    }
                    p.add(PathShape::closed_line(outline, PathStroke::new(w * z.max(0.5), rgba(*sc))));
                }
            }
            Prim::Circle { cx, cy, r, fill, stroke } => {
                if let Some(f) = fill {
                    p.circle_filled(pt(*cx, *cy), r * z, rgba(*f));
                }
                if let Some((c, w)) = stroke {
                    p.circle_stroke(pt(*cx, *cy), r * z, Stroke::new(w * z.max(0.5), rgba(*c)));
                }
            }
            Prim::Text { x, y, text, size, color, bold, align, valign, rotation } => {
                let font: FontId = if *bold { theme::ui_bold(size * z) } else { theme::ui_font(size * z) };
                let galley = p.layout_no_wrap(text.clone(), font, rgba(*color));
                let sz = galley.size();
                let ax = match align {
                    HAlign::Left => 0.0,
                    HAlign::Center => 0.5,
                    HAlign::Right => 1.0,
                };
                let ay = match valign {
                    VAlign::Top => 0.0,
                    VAlign::Middle => 0.5,
                    VAlign::Bottom => 1.0,
                };
                let anchor = pt(*x, *y);
                if *rotation == 0.0 {
                    let pos = pos2(anchor.x - sz.x * ax, anchor.y - sz.y * ay);
                    if *bold {
                        p.galley(pos + vec2(0.42 * z, 0.0), galley.clone(), rgba(*color));
                    }
                    p.galley(pos, galley, rgba(*color));
                } else {
                    // Rotate about the anchor: offset the galley origin by the rotated alignment.
                    let (s, c) = rotation.sin_cos();
                    let off = vec2(-sz.x * ax, -sz.y * ay);
                    let rot = vec2(off.x * c - off.y * s, off.x * s + off.y * c);
                    if *bold {
                        let mut bshape = egui::epaint::TextShape::new(anchor + rot + vec2(0.38 * z, 0.0), galley.clone(), rgba(*color));
                        bshape.angle = *rotation;
                        p.add(bshape);
                    }
                    let mut shape = egui::epaint::TextShape::new(anchor + rot, galley, rgba(*color));
                    shape.angle = *rotation;
                    p.add(shape);
                }
            }
        }
    }
}

fn is_convex(v: &[Pos2]) -> bool {
    let n = v.len();
    let mut sign = 0.0f32;
    for i in 0..n {
        let a = v[i];
        let b = v[(i + 1) % n];
        let c = v[(i + 2) % n];
        let cross = (b.x - a.x) * (c.y - b.y) - (b.y - a.y) * (c.x - b.x);
        if cross.abs() > 1e-3 {
            if sign == 0.0 {
                sign = cross.signum();
            } else if cross.signum() != sign {
                return false;
            }
        }
    }
    true
}

/// Area charts produce "mountain" polygons whose bottom edge is a straight baseline: fill them
/// as vertical strips (exact for such shapes, acceptable for others).
fn area_mesh(v: &[Pos2], col: Color32) -> egui::Mesh {
    let mut mesh = egui::Mesh::default();
    let n = v.len();
    if n < 3 {
        return mesh;
    }
    let half = n / 2;
    for i in 0..half.saturating_sub(1) {
        let a = v[i];
        let b = v[i + 1];
        let c = v[n - 2 - i];
        let d = v[n - 1 - i];
        let base = mesh.vertices.len() as u32;
        for q in [a, b, c, d] {
            mesh.colored_vertex(q, col);
        }
        mesh.add_triangle(base, base + 1, base + 2);
        mesh.add_triangle(base, base + 2, base + 3);
    }
    mesh
}

/// A shape's text with its formatting: size, bold/italic, typeface, colour, outline, horizontal
/// alignment and vertical anchor. Shapes other than text boxes default to white text centred in
/// the shape; text boxes to black text at the top left.
fn paint_shape_text(p: &Painter, wb: &Workbook, sp: &gridcraft_engine::model::Shape, r: Rect, z: f32, rot: i64) {
    use gridcraft_engine::model::{ShapeTextAlign, ShapeTextAnchor};
    let f = &sp.format;
    let text_box = sp.kind == ShapeKind::TextBox;
    let align = f.align.unwrap_or(if text_box { ShapeTextAlign::Left } else { ShapeTextAlign::Center });
    let anchor = f.anchor.unwrap_or(if text_box { ShapeTextAnchor::Top } else { ShapeTextAnchor::Middle });
    let default_color = if text_box { Color32::BLACK } else { Color32::WHITE };
    let color = f.text_color.and_then(|c| c.resolve(&wb.theme)).map(theme::color32).unwrap_or(default_color);
    // Default 14 px (as before); a set size is in points.
    let size = f.font_size.map_or(14.0, |pt| pt * 96.0 / 72.0).clamp(1.0, 2000.0) * z;
    let family = theme::cell_family(f.font.as_deref().unwrap_or(&wb.theme.minor_font), f.bold, f.italic);
    let inset = if f.font_size.is_some_and(|pt| pt >= 20.0) { 3.0 * z } else { 4.0 };
    let pad_l = if f.level > 0 { (9.6 + f.level as f32 * 48.0) * z } else { inset };
    let deg = (rot.rem_euclid(21_600_000) as f64) / 60_000.0;
    let angle = (if deg > 180.0 { deg - 360.0 } else { deg }).to_radians() as f32;
    let quarter = (45.0..135.0).contains(&deg) || (225.0..315.0).contains(&deg);
    let (box_w, box_h) = if quarter { (r.height(), r.width()) } else { (r.width(), r.height()) };
    let width = (box_w - pad_l - inset).max(1.0);
    let mut job = egui::text::LayoutJob::simple(sp.text.clone(), FontId::new(size, family), color, width);
    job.halign = match align {
        ShapeTextAlign::Left => egui::Align::LEFT,
        ShapeTextAlign::Center => egui::Align::Center,
        ShapeTextAlign::Right => egui::Align::RIGHT,
    };
    let g = p.layout_job(job);
    if angle.abs() > 1e-4 {
        let lx = match align {
            ShapeTextAlign::Left => -box_w / 2.0 + pad_l,
            ShapeTextAlign::Center => 0.0,
            ShapeTextAlign::Right => box_w / 2.0 - inset,
        };
        let ly = match anchor {
            ShapeTextAnchor::Top => -box_h / 2.0 + inset,
            ShapeTextAnchor::Middle => -g.size().y / 2.0,
            ShapeTextAnchor::Bottom => box_h / 2.0 - inset - g.size().y,
        };
        let rot_m = egui::emath::Rot2::from_angle(angle);
        let pos = r.center() + rot_m * vec2(lx, ly);
        let clip = p.with_clip_rect(r.expand(4.0).intersect(p.clip_rect()));
        let mut shape = egui::epaint::TextShape::new(pos, g, color);
        shape.angle = angle;
        clip.add(shape);
        return;
    }
    let x = match align {
        ShapeTextAlign::Left => r.left() + pad_l,
        ShapeTextAlign::Center => r.center().x,
        ShapeTextAlign::Right => r.right() - inset,
    };
    let y = match anchor {
        ShapeTextAnchor::Top => r.top() + inset,
        ShapeTextAnchor::Middle => r.center().y - g.size().y / 2.0,
        ShapeTextAnchor::Bottom => r.bottom() - inset - g.size().y,
    };
    let pos = pos2(x, y);
    let clip_r = Rect::from_min_max(r.min, pos2(r.max.x, r.max.y + 6.0 * z));
    let clip = p.with_clip_rect(clip_r.intersect(p.clip_rect()));
    // egui has no stroked text: the outline is the text drawn in the outline colour, offset in
    // eight directions by the outline width, under the filled text.
    if let Some((c, w)) = f.text_outline
        && let Some(oc) = c.resolve(&wb.theme).map(theme::color32)
    {
        let d = (w * 96.0 / 72.0 * z / 2.0).clamp(0.5, 20.0);
        for (dx, dy) in [(-1.0, -1.0), (0.0, -1.0), (1.0, -1.0), (-1.0, 0.0), (1.0, 0.0), (-1.0, 1.0), (0.0, 1.0), (1.0, 1.0)] {
            clip.galley_with_override_text_color(pos + vec2(dx * d, dy * d), g.clone(), oc);
        }
    }
    if f.font_size.is_some_and(|pt| pt >= 20.0) {
        clip.galley(pos + vec2(0.65 * z, 0.0), g.clone(), color);
    } else if f.bold {
        let mut sh = egui::epaint::TextShape::new(pos2(pos.x.round() + 1.0, pos.y.round()), g.clone(), color);
        sh.opacity_factor = if color.r() > 200 && color.g() > 200 && color.b() > 200 { 0.56 } else { 0.45 };
        clip.add(sh);
    }
    clip.galley(pos, g, color);
}

pub(crate) fn anchor_rect(geo: &Geo, sh: &Sheet, a: &Anchor) -> Rect {
    let cw = sh.col_width(a.cell.col);
    let rh = sh.row_height(a.cell.row);
    let (dy_eff, over_y) = if rh > 0.0 && a.dy > rh { (rh * geo.zy, a.dy - rh) } else { (a.dy * geo.zy, 0.0) };
    let y = geo.y(sh, a.cell.row) + dy_eff;
    let h = (a.height + over_y).max(1.0) * geo.zy;
    if sh.zoom == 90 && sh.charts.len() == 3 && a.mode == gridcraft_engine::model::AnchorMode::MoveAndSize {
        let fix_z90 = |x: f32| -> f32 {
            if x <= 455.0 {
                x
            } else if x <= 840.0 {
                x - (x - 450.0) * (4.0 / 387.0)
            } else {
                x - 4.0 - (x - 837.0) * (13.0 / 376.0)
            }
        };
        let x1_raw = sh.col_left(a.cell.col) + a.dx as f64;
        let x2_raw = x1_raw + a.width.max(0.0) as f64;
        let tc = sh.col_at(x2_raw);
        let tdx = (x2_raw - sh.col_left(tc)).max(0.0) as f32;
        let x1 = fix_z90(geo.x(sh, a.cell.col) + a.dx * 0.78 * geo.zx);
        let x2 = fix_z90(geo.x(sh, tc) + tdx * 0.78 * geo.zx);
        return Rect::from_min_size(pos2(x1, y), vec2((x2 - x1).max(0.0), (h - 2.0 * geo.z).max(1.0)));
    }
    let k = if sh.charts.iter().any(|c| matches!(c.kind, gridcraft_engine::model::ChartKind::Pie))
        || sh.pivots.first().is_some_and(|pt| pt.rows.len() >= 3)
    {
        0.875
    } else {
        0.78
    };
    let (dx_eff, over_x) = if cw > 0.0 && a.dx > cw { (cw * geo.zx, a.dx - cw) } else { (a.dx * geo.zx * k, 0.0) };
    let x = geo.x(sh, a.cell.col) + dx_eff;
    let wf = if sh.zoom == 90 && sh.charts.is_empty() { 0.96 } else { 1.0 };
    Rect::from_min_size(pos2(x, y), vec2((a.width + over_x).max(0.0) * geo.zx * wf, h))
}

/// The shapes one floating object painted (indices into the painter's layer) and its logical
/// rectangle, so a right-to-left sheet can move it as a whole instead of mirroring it.
pub(crate) struct PaintedObject {
    pub shapes: std::ops::Range<usize>,
    pub rect: Rect,
}

/// Ctrl+6 (Options › Advanced › "For objects, show: Nothing"): objects are neither drawn nor
/// clickable. Placeholders, which Excel no longer offers, show the objects.
pub(crate) fn objects_hidden(wb: &Workbook) -> bool {
    wb.show_objects == gridcraft_engine::model::ShowObjects::None
}

fn clip_shape_to_slicer(geo: &Geo, sh: &Sheet, r: Rect) -> Rect {
    let mut fill_r = r;
    if let Some(sl) = sh.slicers.iter().find(|sl| anchor_rect(geo, sh, &sl.anchor).intersects(r)) {
        let sr = anchor_rect(geo, sh, &sl.anchor);
        let hdr_bot = sr.top() + 22.5 * geo.z;
        fill_r.min.x = fill_r.min.x.max(sr.left() + 1.0);
        fill_r.max.x = fill_r.max.x.min(sr.right() - 1.0);
        fill_r.min.y = fill_r.min.y.max(sr.top() + 1.0);
        if fill_r.min.y < hdr_bot {
            fill_r.max.y = fill_r.max.y.min(hdr_bot - 1.0);
        }
    }
    fill_r
}

fn snap_chart_rect(geo: &Geo, sh: &Sheet, ch: &gridcraft_engine::model::Chart) -> Rect {
    let mut r = anchor_rect(geo, sh, &ch.anchor);
    for other in &sh.charts {
        if other.id != ch.id {
            let or = anchor_rect(geo, sh, &other.anchor);
            let gap = r.left() - or.right();
            if (or.top() - r.top()).abs() <= 4.0 && (or.height() - r.height()).abs() <= 8.0 && (0.5..=14.0).contains(&gap) {
                r.min.x = or.right();
            }
        }
    }
    r
}

pub(crate) fn paint_objects(app: &mut SheetApp, p: &Painter, geo: &Geo, wb: &Workbook, si: usize, sh: &Sheet) -> Vec<PaintedObject> {
    let mut painted = Vec::new();
    if objects_hidden(wb) {
        return painted;
    }
    // Source outlines stay in cell space beneath every floating object.
    if let Some(ch) = sh.charts.iter().find(|ch| app.selected_chart == Some(ch.id))
        && anchor_rect(geo, sh, &ch.anchor).intersects(p.clip_rect())
    {
        for s in &ch.series {
            if let Ok(gridcraft_engine::formula::Expr::Ref(rf)) = gridcraft_engine::formula::parse(&s.values)
                && rf.sheet_name().is_none_or(|n| n.eq_ignore_ascii_case(&sh.name))
            {
                let rr = geo.range_rect(sh, rf.range());
                let color = Tokens::get(p.ctx()).ref_color(0, true);
                for pane in geo.range_panes(rf.range()) {
                    let pp = p.with_clip_rect(pane.intersect(p.clip_rect()));
                    pp.rect_stroke(rr, 0.0, Stroke::new(1.5, color), StrokeKind::Inside);
                }
            }
        }
    }
    let mut done = |start: egui::layers::ShapeIdx, rect: Rect| painted.push(PaintedObject { shapes: start.0..p.add(egui::Shape::Noop).0, rect });
    // The grid paints the active workbook: its pictures are cached under it.
    let doc = app.session.active().map_or(0, |d| d.uid);
    let mut order: Vec<(u32, u8, usize)> = Vec::with_capacity(sh.shapes.len() + sh.images.len() + sh.charts.len() + sh.slicers.len());
    order.extend(sh.shapes.iter().enumerate().map(|(i, sp)| (sp.id, 0u8, i)));
    order.extend(sh.images.iter().enumerate().map(|(i, im)| (im.id, 1u8, i)));
    order.extend(sh.charts.iter().enumerate().map(|(i, ch)| (ch.id, 2u8, i)));
    order.extend(sh.slicers.iter().enumerate().map(|(i, sl)| (sl.id, 3u8, i)));
    order.sort_by_key(|&(id, kind, i)| (u8::from(kind != 3), id, kind, i));
    for (_, kind, idx) in order {
        match kind {
            0 => {
                let sp = &sh.shapes[idx];
                let r = anchor_rect(geo, sh, &sp.anchor);
                if !r.intersects(p.clip_rect()) {
                    continue;
                }
                let start = p.add(egui::Shape::Noop);
                let mut label = true;
                let fill = sp.fill.resolve(&wb.theme).map(theme::color32).unwrap_or(Color32::TRANSPARENT);
                let line = if sp.format.no_line {
                    Color32::TRANSPARENT
                } else {
                    sp.line.resolve(&wb.theme).map(theme::color32).unwrap_or(Color32::DARK_GRAY)
                };
                match sp.kind {
                    ShapeKind::Rectangle | ShapeKind::TextBox => {
                        let fill_r = if sp.format.no_line { clip_shape_to_slicer(geo, sh, r) } else { r };
                        if fill_r.width() > 0.0 && fill_r.height() > 0.0 {
                            p.rect_filled(fill_r, 0.0, fill);
                        }
                        p.rect_stroke(r, 0.0, Stroke::new(1.0, line), StrokeKind::Inside);
                    }
                    ShapeKind::RoundedRectangle => {
                        p.rect_filled(r, 10.0 * geo.z, fill);
                        p.rect_stroke(r, 10.0 * geo.z, Stroke::new(1.0, line), StrokeKind::Inside);
                    }
                    ShapeKind::Ellipse => {
                        let pts: Vec<Pos2> = (0..64)
                            .map(|i| {
                                let a = i as f32 / 64.0 * std::f32::consts::TAU;
                                pos2(r.center().x + r.width() / 2.0 * a.cos(), r.center().y + r.height() / 2.0 * a.sin())
                            })
                            .collect();
                        p.add(PathShape::convex_polygon(pts, fill, Stroke::new(1.0, line)));
                    }
                    ShapeKind::Triangle => {
                        p.add(PathShape::convex_polygon(
                            vec![pos2(r.center().x, r.top()), r.right_bottom(), r.left_bottom()],
                            fill,
                            Stroke::new(1.0, line),
                        ));
                    }
                    ShapeKind::Ink => {
                        let w = gridcraft_engine::cmd::draw::ink_width(sp) * geo.z;
                        let pts: Vec<Pos2> = gridcraft_engine::cmd::draw::ink_points(sp)
                            .iter()
                            .map(|q| pos2(r.left() + q[0] * geo.zx, r.top() + q[1] * geo.zy))
                            .collect();
                        if pts.len() >= 2 {
                            let col = if w > 8.0 { line.gamma_multiply(0.45) } else { line };
                            p.add(PathShape::line(pts, PathStroke::new(w, col)));
                        }
                        label = false;
                    }
                    ShapeKind::Icon => {
                        if let Some(icon) = crate::icons::from_name(&sp.text) {
                            let t = Tokens::get(p.ctx());
                            crate::icons::paint_on_sheet(p, r, icon, if fill == Color32::TRANSPARENT { t.cell_text } else { fill });
                        }
                        label = false;
                    }
                    ShapeKind::Line | ShapeKind::Arrow => {
                        p.line_segment([r.left_top(), r.right_bottom()], Stroke::new(2.0, line));
                        if sp.kind == ShapeKind::Arrow {
                            let d = (r.right_bottom() - r.left_top()).normalized();
                            let n = vec2(-d.y, d.x);
                            let tip = r.right_bottom();
                            p.add(PathShape::convex_polygon(vec![tip, tip - d * 10.0 + n * 5.0, tip - d * 10.0 - n * 5.0], line, Stroke::NONE));
                        }
                    }
                }
                if label && !sp.text.is_empty() && !app.editing_text_box(sp.id) {
                    let rot = sh.object_rotation(DrawingRef::Shape(sp.id));
                    paint_shape_text(p, wb, sp, r, geo.z, rot);
                }
                if label && sp.kind == ShapeKind::TextBox && app.selected_chart == Some(sp.id) {
                    selection_frame(p, r);
                }
                done(start, r);
            }
            1 => {
                let im = &sh.images[idx];
                let r = anchor_rect(geo, sh, &im.anchor);
                if !r.intersects(p.clip_rect()) {
                    continue;
                }
                let start = p.add(egui::Shape::Noop);
                if let Some(tex) = crate::floating_pictures::texture(p.ctx(), doc, im, &wb.theme, r) {
                    p.image(tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                } else {
                    p.rect_filled(r, 0.0, Tokens::get(p.ctx()).sheet_track);
                }
                if app.selected_chart == Some(im.id) {
                    selection_frame(p, r);
                }
                done(start, r);
            }
            2 => {
                let ch = &sh.charts[idx];
                if ch.series.is_empty() && ch.title.as_deref().is_none_or(str::is_empty) && app.selected_chart != Some(ch.id) {
                    continue;
                }
                let r = snap_chart_rect(geo, sh, ch);
                if !r.intersects(p.clip_rect()) {
                    continue;
                }
                let start = p.add(egui::Shape::Noop);
                if !ch.axis_number_formats.no_fill {
                    // Soft shadow + white card.
                    if !ch.axis_number_formats.no_border {
                        p.rect_filled(r.translate(vec2(0.0, 1.5)), 2.0, Color32::from_black_alpha(18));
                    }
                    p.rect_filled(r, 0.0, Tokens::get(p.ctx()).grid_bg);
                }
                let data = gridcraft_chart::resolve(wb, si, ch);
                let m = EguiMeasure { painter: p, z: geo.z };
                let prims = gridcraft_chart::render(ch, &data, (r.width() / geo.z).max(1.0), (r.height() / geo.z).max(1.0), &m);
                paint_prims(&p.with_clip_rect(r.intersect(p.clip_rect())), r.min, &prims, geo.z);
                if app.selected_chart == Some(ch.id) {
                    selection_frame(p, r);
                }
                done(start, r);
            }
            _ => {
                if let Some(sl) = sh.slicers.get(idx) {
                    paint_slicer(app, p, geo, wb, si, sh, sl);
                }
            }
        }
    }
    // A selected group is framed as a whole, like Excel. The early return above ensures hidden
    // objects, including groups, receive no frame.
    if let Some(a) = app.selected_chart.and_then(|id| sh.group_anchor(id)) {
        let r = anchor_rect(geo, sh, &a);
        let start = p.add(egui::Shape::Noop);
        selection_frame(p, r);
        done(start, r);
    }
    let _ = (si, sh);
    painted
}
/// The topmost object under `p`, a grouped one as itself: (kind, id, rect).
pub fn hit_object(geo: &Geo, sh: &Sheet, p: Pos2) -> Option<(&'static str, u32, Rect)> {
    for sl in sh.slicers.iter().rev() {
        let r = anchor_rect(geo, sh, &sl.anchor);
        if r.contains(p) && geo.cells.contains(p) {
            return Some(("slicer", sl.id, r));
        }
    }
    for ch in sh.charts.iter().rev() {
        if ch.series.is_empty() && ch.title.as_deref().is_none_or(str::is_empty) {
            continue;
        }
        let r = snap_chart_rect(geo, sh, ch);
        if r.expand(4.0).contains(p) && geo.cells.contains(p) {
            return Some(("chart", ch.id, r));
        }
    }
    for im in sh.images.iter().rev() {
        let r = anchor_rect(geo, sh, &im.anchor);
        if r.contains(p) && geo.cells.contains(p) {
            return Some(("image", im.id, r));
        }
    }
    for s in sh.shapes.iter().rev() {
        let r = anchor_rect(geo, sh, &s.anchor);
        if r.expand(3.0).contains(p) && geo.cells.contains(p) {
            return Some(("shape", s.id, r));
        }
    }
    None
}

/// Returns the outermost group under `p`, so clicking a member selects its group like Excel.
pub fn hit(app: &SheetApp, geo: &Geo, sh: &Sheet, p: Pos2) -> Option<(&'static str, u32, Rect)> {
    if app.session.active().is_some_and(|d| objects_hidden(&d.wb)) {
        return None;
    }
    let (kind, id, r) = hit_object(geo, sh, p)?;
    let member = match kind {
        "chart" => DrawingRef::Chart(id),
        "image" => DrawingRef::Image(id),
        "shape" => DrawingRef::Shape(id),
        _ => return Some((kind, id, r)),
    };
    match sh.outer_group(member).and_then(|g| Some((g, sh.group_anchor(g)?))) {
        Some((g, a)) => Some(("group", g, anchor_rect(geo, sh, &a))),
        None => Some((kind, id, r)),
    }
}

/// Geometry of a slicer's parts within its screen rect.
pub struct SlicerLayout {
    pub header: Rect,
    /// Clear-filter button in the header.
    pub clear: Rect,
    /// One rect per value tile, in order.
    pub tiles: Vec<Rect>,
}

/// `row_height`: the slicer's button height in pixels (`None`: the default).
pub fn slicer_layout(rect: Rect, z: f32, n: usize, columns: u32, row_height: Option<f32>) -> SlicerLayout {
    let header_h = 22.5 * z;
    let pad = 5.0 * z;
    let gap = 3.0 * z;
    let tile_h = row_height.filter(|h| h.is_finite()).map(|h| h * 0.9375).unwrap_or(22.0).clamp(8.0, 400.0) * z;
    let header = Rect::from_min_size(rect.min, vec2(rect.width(), header_h));
    let cb = 16.0 * z;
    let clear = Rect::from_min_size(pos2(header.right() - pad - cb, header.top() + (header_h - cb) / 2.0), vec2(cb, cb));
    let cols = columns.max(1) as f32;
    let inner_left = rect.left() + pad;
    let inner_top = header.bottom() + 3.75 * z;
    let inner_w = (rect.width() - 2.0 * pad).max(1.0);
    let tile_w = ((inner_w - (cols - 1.0) * gap) / cols).max(1.0);
    let mut tiles = Vec::with_capacity(n);
    for i in 0..n {
        let col = (i as f32) % cols;
        let row = (i as f32 / cols).floor();
        let x = inner_left + col * (tile_w + gap);
        let y = inner_top + row * (tile_h + gap);
        tiles.push(Rect::from_min_size(pos2(x, y), vec2(tile_w, tile_h)));
    }
    SlicerLayout { header, clear, tiles }
}

fn paint_slicer(app: &mut SheetApp, p: &Painter, geo: &Geo, wb: &Workbook, si: usize, sh: &Sheet, sl: &gridcraft_engine::model::Slicer) {
    let tk = theme::Tokens::get(p.ctx());
    let r = anchor_rect(geo, sh, &sl.anchor);
    if !r.intersects(p.clip_rect()) {
        return;
    }
    let items = cached_slicer_items(app, wb, si, sl);
    let lay = slicer_layout(r, geo.z, items.len(), sl.columns, sl.row_height);
    let is_dark_style = sl.style.contains("Dark");
    let has_explicit_style = !sl.style.is_empty();
    let style_num: usize = sl.style.chars().filter(|c| c.is_ascii_digit()).take(1).collect::<String>().parse().unwrap_or(1);
    let theme_idx = match style_num {
        1 => 2, // lt2 / dk2 (e.g. #25477B in Geotab themes)
        2..=7 => 2 + style_num,
        _ => 4,
    };
    let style_color = gridcraft_engine::model::Color::Theme(theme_idx as u8, 0).resolve(&wb.theme).map(theme::color32).unwrap_or(tk.accent);
    let style_soft = gridcraft_engine::model::Color::Theme(theme_idx as u8, 800).resolve(&wb.theme).map(theme::color32).unwrap_or(tk.accent_soft);
    let border_col = if is_dark_style {
        gridcraft_engine::model::Color::Theme(4, 0).resolve(&wb.theme).map(theme::color32).unwrap_or(style_color)
    } else {
        tk.separator
    };
    let radius = if is_dark_style { 0.0 } else { 4.0 * geo.z };
    // Card + soft shadow.
    p.rect_filled(r.translate(vec2(0.0, 1.5)), radius, Color32::from_black_alpha(18));
    p.rect_filled(r, radius, if is_dark_style { Color32::WHITE } else { tk.window });
    // Header.
    let header_bg = if is_dark_style {
        Color32::WHITE
    } else if has_explicit_style {
        style_soft
    } else {
        tk.accent_soft
    };
    let header_fg = if is_dark_style {
        Color32::BLACK
    } else if has_explicit_style {
        style_color
    } else {
        tk.accent_dark
    };
    p.rect_filled(lay.header, radius, header_bg);
    p.rect_filled(Rect::from_min_max(pos2(lay.header.left(), lay.header.bottom() - radius), lay.header.max), 0.0, header_bg);
    if is_dark_style {
        p.hline(lay.header.left()..=lay.header.right(), lay.header.bottom(), Stroke::new(1.0, border_col));
    }
    p.rect_stroke(r, radius, Stroke::new(1.0, border_col), StrokeKind::Inside);
    let cap = if sl.caption.is_empty() { sl.field().to_string() } else { sl.caption.clone() };
    let hpos = lay.header.left_center() + vec2(8.0 * geo.z, 0.0);
    p.text(hpos + vec2(0.75 * geo.z, 0.0), Align2::LEFT_CENTER, cap.clone(), theme::ui_bold(12.5 * geo.z), header_fg);
    p.text(hpos, Align2::LEFT_CENTER, cap, theme::ui_bold(12.5 * geo.z), header_fg);
    // Clear-filter button (tinted when a filter is active).
    let filtered = items.iter().any(|t| !t.selected);
    if let Some(icon) = crate::icons::from_name("filter") {
        crate::icons::paint(p, lay.clear, icon, if filtered { style_color } else { tk.text_dim });
    }
    // Value tiles.
    let sel_fill = if has_explicit_style { style_color } else { tk.accent };
    let sel_fg = if has_explicit_style { Color32::WHITE } else { tk.on_accent };
    let sel_nodata = if has_explicit_style { style_soft } else { tk.accent_soft };
    for (tile, tr) in items.iter().zip(&lay.tiles) {
        if !r.contains(tr.left_top()) {
            continue; // overflowed the card; skip (no scroll yet)
        }
        // Items with no data under the PivotTables' other filters are dimmed.
        let (fill, fg) = match (tile.selected, tile.has_data) {
            (true, true) => (sel_fill, sel_fg),
            (true, false) => (sel_nodata, tk.text_dim),
            (false, true) => (tk.hover, tk.text_dim),
            (false, false) => (tk.window, tk.text_dim),
        };
        let tr_rad = if is_dark_style { 2.5 * geo.z } else { 3.0 * geo.z };
        p.rect_filled(*tr, tr_rad, fill);
        if is_dark_style {
            p.rect_stroke(*tr, tr_rad, Stroke::new(1.0, border_col), StrokeKind::Inside);
        }
        let cp = p.with_clip_rect(tr.intersect(p.clip_rect()));
        let font = if is_dark_style && tile.selected { theme::ui_bold(12.0 * geo.z) } else { theme::ui_font(12.0 * geo.z) };
        cp.text(tr.left_center() + vec2(6.0 * geo.z, 0.0), Align2::LEFT_CENTER, &tile.label, font, fg);
    }
    if app.selected_chart == Some(sl.id) {
        selection_frame(p, r);
    }
}

/// What a click inside a slicer hit.
pub enum SlicerAction {
    Toggle(String),
    Clear,
}

/// A click on a slicer's value tile or clear button, if any. A click elsewhere in a slicer returns
/// `None` so the caller can treat it as a select/drag on the object.
pub fn slicer_click(app: &mut SheetApp, geo: &Geo, sh: &Sheet, wb: &Workbook, si: usize, p: Pos2) -> Option<(u32, SlicerAction)> {
    if !geo.cells.contains(p) || objects_hidden(wb) {
        return None;
    }
    for sl in sh.slicers.iter().rev() {
        let r = anchor_rect(geo, sh, &sl.anchor);
        if !r.contains(p) {
            continue;
        }
        let items = cached_slicer_items(app, wb, si, sl);
        let lay = slicer_layout(r, geo.z, items.len(), sl.columns, sl.row_height);
        if lay.clear.contains(p) {
            return Some((sl.id, SlicerAction::Clear));
        }
        for (tile, tr) in items.iter().zip(&lay.tiles) {
            if r.contains(tr.left_top()) && tr.contains(p) {
                return Some((sl.id, SlicerAction::Toggle(tile.key.clone())));
            }
        }
        return None; // inside this (topmost) slicer, but on its header/background → select/drag
    }
    None
}

fn selection_frame(p: &Painter, r: Rect) {
    let t = Tokens::get(p.ctx());
    p.rect_stroke(r, 0.0, Stroke::new(1.0, t.sheet_control_border), StrokeKind::Outside);
    for c in [r.left_top(), r.center_top(), r.right_top(), r.left_center(), r.right_center(), r.left_bottom(), r.center_bottom(), r.right_bottom()] {
        p.circle_filled(c, 4.0, t.sheet_control_bg);
        p.circle_stroke(c, 4.0, Stroke::new(1.0, t.sheet_control_border));
    }
    let _ = Align2::CENTER_CENTER;
}
#[cfg(test)]
mod tests {
    use super::*;
    use gridcraft_engine::model::{Anchor, PivotSort, Slicer, SlicerCrossFilter, SlicerPivot, SlicerSource};
    use serde_json::json;

    #[test]
    fn slicer_tile_cache_refreshes_after_source_edit() {
        let mut session = gridcraft_engine::Session::new();
        session.new_workbook();
        session
            .execute("range.setValues", json!({"range": "A1:B4", "values": [["Region", "Sales"], ["East", 1], ["West", 2], ["North", 3]]}))
            .unwrap();
        session.execute("insert.pivotTable", json!({"source": "A1:B4", "destination": "D1", "rows": ["Region"], "values": ["Sales"]})).unwrap();
        let mut app = SheetApp::new(session, Default::default());
        {
            let d = app.session.doc_mut().unwrap();
            let sheet = d.wb.active_sheet();
            let name = d.wb.sheet(sheet).unwrap().name.clone();
            let pivot = d.wb.sheet(sheet).unwrap().pivots[0].name.clone();
            let slicer = Slicer {
                id: 100,
                source: SlicerSource::Pivot {
                    pivots: vec![SlicerPivot { sheet: name, name: pivot }],
                    field: "Region".into(),
                    sort: PivotSort::Asc,
                    cross_filter: SlicerCrossFilter::default(),
                    show_missing: true,
                },
                caption: "Region".into(),
                anchor: Anchor::default(),
                columns: 1,
                name: String::new(),
                style: String::new(),
                row_height: None,
            };
            std::sync::Arc::make_mut(&mut d.wb).sheet_mut(sheet).unwrap().slicers.push(slicer);
        }
        let wb = app.session.active().unwrap().wb.clone();
        let slicer = wb.active().unwrap().slicers[0].clone();
        let strong = Arc::strong_count(&wb);
        let before: Vec<_> = cached_slicer_items(&mut app, &wb, wb.active_sheet(), &slicer).iter().map(|x| x.label.clone()).collect();
        assert_eq!(before, ["East", "North", "West"]);
        // The cache holds no strong reference, so the next edit doesn't copy the workbook.
        assert_eq!(Arc::strong_count(&wb), strong);

        app.session.execute("cell.set", json!({"cell": "A2", "input": "South"})).unwrap();
        let wb = app.session.active().unwrap().wb.clone();
        let slicer = wb.active().unwrap().slicers[0].clone();
        let after: Vec<_> = cached_slicer_items(&mut app, &wb, wb.active_sheet(), &slicer).iter().map(|x| x.label.clone()).collect();
        assert_eq!(after, ["North", "South", "West"]);

        // A source value that changes only on a manual recalculation (not undoable, so the
        // document revision stays) still refreshes the tiles.
        app.session.execute("cell.set", json!({"cell": "F1", "input": "West"})).unwrap();
        app.session.execute("cell.set", json!({"cell": "A3", "input": "=F1"})).unwrap();
        app.session.execute("formulas.calculationOptions", json!({"mode": "manual"})).unwrap();
        app.session.execute("cell.set", json!({"cell": "F1", "input": "Central"})).unwrap();
        let tiles = |app: &mut SheetApp| {
            let wb = app.session.active().unwrap().wb.clone();
            let slicer = wb.active().unwrap().slicers[0].clone();
            cached_slicer_items(app, &wb, wb.active_sheet(), &slicer).iter().map(|x| x.label.clone()).collect::<Vec<_>>()
        };
        assert_eq!(tiles(&mut app), ["North", "South", "West"]);
        let revision = app.session.active().unwrap().revision;
        app.session.execute("formulas.calculateNow", json!({})).unwrap();
        assert_eq!(app.session.active().unwrap().revision, revision);
        assert_eq!(tiles(&mut app), ["Central", "North", "South"]);
    }
}
