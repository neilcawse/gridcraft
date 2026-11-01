use super::frame::Bx;
use super::trend::supports_trendline;
use super::{
    Chart, ChartData, ChartKind, Ctx, DOWN, HAlign, LABEL_SIZE, MAX_CATS, MAX_SERIES, MAX_TRENDLINES, Prim, Rgba, SeriesData, TEXT, Theme, UP,
    VAlign, auto_name, series_color,
};
use gridcraft_model::LegendPos;

/// How a legend entry's swatch is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Swatch {
    Box,
    Line,
    LineMarker,
    Marker,
}

/// The kind a series is drawn as (per-series kinds only apply to combo charts).
pub(super) fn plot_kind(chart: &Chart, s: &SeriesData) -> ChartKind {
    if chart.kind == ChartKind::Combo { if s.kind == ChartKind::Combo { ChartKind::Line } else { s.kind } } else { chart.kind }
}

/// Colour of point `j` for charts that colour by point (pie, doughnut, treemap, sunburst).
pub(super) fn point_color(data: &ChartData, j: usize) -> Rgba {
    match (j, data.series.first()) {
        (0, Some(s)) => s.color,
        _ => j.checked_rem(data.palette.len()).and_then(|k| data.palette.get(k)).copied().unwrap_or_else(|| series_color(&Theme::default(), j)),
    }
}

fn first_len(data: &ChartData) -> usize {
    data.series.first().map(|s| s.values.len()).unwrap_or(0)
}

pub(super) fn cat_label(data: &ChartData, j: usize) -> String {
    data.categories.get(j).cloned().unwrap_or_else(|| (j + 1).to_string())
}

/// Legend entries (text, colour, swatch) `render` draws for this chart; empty when hidden.
pub fn legend_entries(chart: &Chart, data: &ChartData) -> Vec<(String, Rgba, Swatch)> {
    if chart.legend == LegendPos::None {
        return vec![];
    }
    match chart.kind {
        ChartKind::Pie | ChartKind::Doughnut | ChartKind::Treemap | ChartKind::Sunburst => {
            let n = first_len(data).min(MAX_CATS);
            (0..n).map(|j| (cat_label(data, j), point_color(data, j), Swatch::Box)).collect()
        }
        ChartKind::Histogram | ChartKind::Funnel | ChartKind::Stock => vec![],
        ChartKind::Waterfall => {
            if data.series.is_empty() {
                return vec![];
            }
            let total = data.series.first().map(|s| s.color).unwrap_or(TEXT);
            vec![("Increase".into(), UP, Swatch::Box), ("Decrease".into(), DOWN, Swatch::Box), ("Total".into(), total, Swatch::Box)]
        }
        _ if data.series.len() == 1 && chart.axis_number_formats.point_colors.len() > 1 => {
            let n = first_len(data).min(MAX_CATS);
            (0..n).map(|j| (cat_label(data, j), point_color(data, j), Swatch::Box)).collect()
        }
        _ => {
            let series = data.series.iter().take(MAX_SERIES);
            let mut out: Vec<_> = series
                .clone()
                .map(|s| {
                    let sw = match plot_kind(chart, s) {
                        ChartKind::Line | ChartKind::Radar | ChartKind::ScatterLines => Swatch::Line,
                        ChartKind::LineStacked if s.stroke.is_none() => Swatch::Line,
                        ChartKind::LineMarkers | ChartKind::LineStacked => Swatch::LineMarker,
                        ChartKind::Scatter | ChartKind::Bubble => Swatch::Marker,
                        _ => Swatch::Box,
                    };
                    (s.name.clone(), s.color, sw)
                })
                .collect();
            // Trendlines follow the series, named "Linear (Sales)" unless given a name.
            for s in series.filter(|s| supports_trendline(plot_kind(chart, s))) {
                for t in s.trendlines.iter().take(MAX_TRENDLINES) {
                    out.push((t.spec.name.clone().unwrap_or_else(|| auto_name(t.spec.kind, &s.name)), t.color, Swatch::Line));
                }
            }
            out
        }
    }
}

// ---------------------------------------------------------------- legend

fn swatch_w(sw: Swatch, s: f32) -> f32 {
    if sw == Swatch::Box { s } else { s * 2.65 }
}

fn draw_swatch(ctx: &mut Ctx, sw: Swatch, x: f32, yc: f32, s: f32, color: Rgba, stroke: Option<Rgba>) {
    match sw {
        Swatch::Box => ctx.rect(x, yc - s / 2.0, s, s, color, stroke.map(|c| (c, 2.0))),
        Swatch::Line => ctx.hline(x, x + 2.65 * s, yc, color, 2.25),
        Swatch::LineMarker => {
            ctx.hline(x, x + 2.65 * s, yc, color, 2.25);
            let (m_fill, m_ring) = match stroke {
                Some(mf) => (mf, Some((color, 1.25))),
                None => (color, None),
            };
            ctx.out.push(Prim::Circle { cx: x + 1.325 * s, cy: yc, r: s * 0.35, fill: Some(m_fill), stroke: m_ring });
        }
        Swatch::Marker => ctx.out.push(Prim::Circle { cx: x + s / 2.0, cy: yc, r: s * 0.4, fill: Some(color), stroke: stroke.map(|c| (c, 1.25)) }),
    }
}

fn entry_stroke(data: &ChartData, i: usize) -> Option<Rgba> {
    if data.series.len() == 1
        && let Some(c) = data.series[0].point_strokes.iter().find(|&&(idx, _)| idx as usize == i).map(|&(_, c)| c)
    {
        return Some(c);
    }
    data.series.get(i).and_then(|s| s.stroke)
}

pub(super) fn draw_legend(ctx: &mut Ctx, entries: &[(String, Rgba, Swatch)], pos: LegendPos, area: Bx, data: &ChartData) -> Bx {
    let size = LABEL_SIZE;
    let row_h = size * 1.6;
    let s = size * 0.8;
    let gap = 3.0;
    let spacing = size * 0.85;
    let item_w = |ctx: &Ctx, e: &(String, Rgba, Swatch)| swatch_w(e.2, s) + gap + ctx.tw(&e.0, size);
    match pos {
        LegendPos::None => area,
        LegendPos::Top | LegendPos::Bottom => {
            let max_rows = ((area.h * 0.3 / row_h).floor() as usize).max(1);
            let mut rows: Vec<Vec<(usize, f32)>> = vec![vec![]];
            let mut cur = 0.0f32;
            for (i, e) in entries.iter().enumerate() {
                let iw = item_w(ctx, e).min(area.w);
                let need = if cur > 0.0 { spacing + iw } else { iw };
                if cur > 0.0 && cur + need > area.w {
                    if rows.len() >= max_rows {
                        break;
                    }
                    rows.push(vec![]);
                    cur = 0.0;
                }
                cur += if cur > 0.0 { spacing + iw } else { iw };
                if let Some(r) = rows.last_mut() {
                    r.push((i, iw));
                }
            }
            let lh = rows.len() as f32 * row_h;
            let y0 = if pos == LegendPos::Bottom { area.bottom() - lh } else { area.y };
            for (ri, row) in rows.iter().enumerate() {
                let total: f32 = row.iter().map(|(_, w)| w).sum::<f32>() + spacing * row.len().saturating_sub(1) as f32;
                let mut x = area.x + (area.w - total).max(0.0) / 2.0;
                let yc = y0 + ri as f32 * row_h + row_h / 2.0;
                for &(i, iw) in row {
                    if let Some(e) = entries.get(i) {
                        draw_swatch(ctx, e.2, x, yc, s, e.1, entry_stroke(data, i));
                        let sx = swatch_w(e.2, s) + gap;
                        let t = ctx.fit(&e.0, size, (iw - sx).max(0.0) + 0.5);
                        ctx.text(x + sx, yc, &t, size, TEXT, (HAlign::Left, VAlign::Middle));
                    }
                    x += iw + spacing;
                }
            }
            let used = lh + 4.0;
            if pos == LegendPos::Bottom { Bx { h: area.h - used, ..area } } else { Bx { y: area.y + used, h: area.h - used, ..area } }
        }
        LegendPos::Left | LegendPos::Right => {
            let manual_w = if pos == LegendPos::Right {
                ctx.legend_x.filter(|&lx| (0.45..=0.85).contains(&lx)).map(|lx| (area.right() - ctx.chart_w * lx).max(0.0))
            } else {
                None
            };
            let colw = manual_w.unwrap_or_else(|| entries.iter().map(|e| item_w(ctx, e)).fold(0.0f32, f32::max).min(area.w * 0.34));
            let sx = swatch_w(Swatch::Box, s) + gap;
            let avail_est = (colw - sx).max(0.0) + 0.5;
            let wraps = entries.iter().filter(|e| ctx.tw(&e.0, size) > avail_est && e.0.contains(' ')).count();
            let target_h = ctx.legend_h.filter(|&lh| (0.3..=1.0).contains(&lh)).map(|lh| (ctx.chart_h * lh).min(area.h));
            let (eff_row_h, max_rows) = match target_h {
                _ if !entries.is_empty() && wraps > 0 => {
                    let th = target_h.unwrap_or(area.h * 0.64);
                    let min_rh = row_h * 1.95;
                    let mr = ((th / min_rh).round() as usize).clamp(1, entries.len().min(4));
                    ((th / mr as f32).max(min_rh), mr)
                }
                Some(th) if !entries.is_empty() => {
                    let erh = (th / entries.len() as f32).clamp(row_h, row_h * 2.4);
                    (erh, ((area.h / erh).floor() as usize).max(1))
                }
                _ if !entries.is_empty() && entries.len() <= 6 => {
                    let erh = (row_h * 1.38).min(area.h / entries.len() as f32).max(row_h);
                    (erh, ((area.h / erh).floor() as usize).max(1))
                }
                _ => (row_h, ((area.h / row_h).floor() as usize).max(1)),
            };
            let n = entries.len().min(max_rows);
            let lh = n as f32 * eff_row_h;
            let y0 = area.y + (area.h - lh).max(0.0) / 2.0;
            let x0 = if pos == LegendPos::Right { area.right() - colw } else { area.x };
            for (i, e) in entries.iter().take(n).enumerate() {
                let yc = y0 + i as f32 * eff_row_h + eff_row_h / 2.0;
                draw_swatch(ctx, e.2, x0, yc, s, e.1, entry_stroke(data, i));
                let sx = swatch_w(e.2, s) + gap;
                let avail = (colw - sx).max(0.0) + 0.5;
                if eff_row_h >= size * 1.9
                    && (ctx.tw(&e.0, size) > avail || (wraps > 0 && e.0.contains(' ')))
                    && let Some(sp) = e.0.rfind(' ')
                {
                    let (l1, l2) = e.0.split_at(sp);
                    let t1 = ctx.fit(l1, size, avail);
                    let t2 = ctx.fit(l2.trim_start(), size, avail);
                    ctx.text(x0 + sx, yc - size * 0.55, &t1, size, TEXT, (HAlign::Left, VAlign::Middle));
                    ctx.text(x0 + sx, yc + size * 0.55, &t2, size, TEXT, (HAlign::Left, VAlign::Middle));
                } else {
                    let t = ctx.fit(&e.0, size, avail);
                    ctx.text(x0 + sx, yc, &t, size, TEXT, (HAlign::Left, VAlign::Middle));
                }
            }
            let used = colw + 8.0;
            if pos == LegendPos::Right { Bx { w: area.w - used, ..area } } else { Bx { x: area.x + used, w: area.w - used, ..area } }
        }
    }
}
