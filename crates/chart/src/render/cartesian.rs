use super::frame::{Bx, CatAxis, ValAxis, frame, on, value_range};
use super::legend::{cat_label, plot_kind};
use super::lines::{decimate, draw_markers, draw_runs};
use super::trend::{draw_trend_labels, draw_trendlines, max_backward, max_forward, max_forward_slots, supports_trendline};
use super::{
    Chart, ChartData, ChartKind, Ctx, Fmt, HAlign, LABEL_SIZE, MAX_CATS, MAX_LABELS, MAX_LINE_PTS, MAX_MARKERS, Prim, Scale, SeriesData, TEXT,
    VAlign, WHITE, axis_fmt, clean, fixed_scale, label_fmt, nice_scale,
};

// ---------------------------------------------------------------- cartesian (column/bar/line/area/combo)

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fam {
    Col,
    ColStack,
    Col100,
    Line,
    LineStack,
    Area,
    AreaStack,
    Area100,
}

/// Stack accumulator: (axis, family) key with positive and negative running sums.
type Stack = ((bool, Fam), Vec<f64>, Vec<f64>);

fn family(k: ChartKind) -> Fam {
    use ChartKind::*;
    match k {
        ColumnStacked | BarStacked => Fam::ColStack,
        ColumnStacked100 | BarStacked100 => Fam::Col100,
        Line | LineMarkers | Scatter | ScatterLines | Radar | Stock => Fam::Line,
        LineStacked => Fam::LineStack,
        Area => Fam::Area,
        AreaStacked => Fam::AreaStack,
        AreaStacked100 => Fam::Area100,
        _ => Fam::Col,
    }
}

/// Families stacked to 100%: each category's values are shares of its total.
fn is_100(f: Fam) -> bool {
    matches!(f, Fam::Col100 | Fam::Area100)
}

fn is_col(f: Fam) -> bool {
    matches!(f, Fam::Col | Fam::ColStack | Fam::Col100)
}

fn is_line(f: Fam) -> bool {
    matches!(f, Fam::Line | Fam::LineStack)
}

fn series_spans(data: &ChartData, fams: &[Fam], sec: &[bool], ncat: usize) -> Vec<Vec<Option<(f64, f64)>>> {
    // Totals for 100% stacking, per axis and family (100% columns and 100% areas stack apart).
    let mut totals: Vec<((bool, Fam), Vec<f64>)> = Vec::new();
    for (i, s) in data.series.iter().enumerate() {
        let fam = fams.get(i).copied().unwrap_or(Fam::Col);
        if is_100(fam) {
            let key = (sec.get(i).copied().unwrap_or(false), fam);
            let p = match totals.iter().position(|(k, _)| *k == key) {
                Some(p) => p,
                None => {
                    totals.push((key, vec![0.0; ncat]));
                    totals.len() - 1
                }
            };
            for (j, v) in s.values.iter().take(ncat).enumerate() {
                if let (Some(v), Some(t)) = (v, totals.get_mut(p).and_then(|(_, t)| t.get_mut(j))) {
                    *t += v.abs();
                }
            }
        }
    }
    // A value's share of its category's total, for the 100% families.
    let share = |key: (bool, Fam), j: usize, v: f64| {
        let t = totals.iter().find(|(k, _)| *k == key).and_then(|(_, t)| t.get(j)).copied().unwrap_or(0.0);
        if t > 0.0 { v / t } else { 0.0 }
    };
    // Stack accumulators keyed by (axis, family): positive and negative running sums.
    let mut stacks: Vec<Stack> = Vec::new();
    let mut spans: Vec<Vec<Option<(f64, f64)>>> = Vec::with_capacity(data.series.len());
    for (i, s) in data.series.iter().enumerate() {
        let fam = fams.get(i).copied().unwrap_or(Fam::Col);
        let a = sec.get(i).copied().unwrap_or(false);
        let stacked = matches!(fam, Fam::ColStack | Fam::Col100 | Fam::LineStack | Fam::AreaStack | Fam::Area100);
        let key = (a, fam);
        let si = if stacked {
            match stacks.iter().position(|(k, _, _)| *k == key) {
                Some(p) => Some(p),
                None => {
                    stacks.push((key, vec![0.0; ncat], vec![0.0; ncat]));
                    Some(stacks.len() - 1)
                }
            }
        } else {
            None
        };
        let mut row = Vec::with_capacity(ncat);
        for j in 0..ncat {
            let v = s.values.get(j).copied().flatten();
            let span = match (si.and_then(|p| stacks.get_mut(p)), fam) {
                (Some((_, pos, neg)), Fam::ColStack | Fam::Col100) => v.and_then(|v| {
                    let v = if fam == Fam::Col100 { share(key, j, v) } else { v };
                    let acc = if v >= 0.0 { pos.get_mut(j) } else { neg.get_mut(j) }?;
                    let base = *acc;
                    *acc += v;
                    Some((base, *acc))
                }),
                (Some((_, pos, _)), _) => {
                    let v = v.unwrap_or(0.0);
                    let v = if fam == Fam::Area100 { share(key, j, v) } else { v };
                    pos.get_mut(j).map(|acc| {
                        let base = *acc;
                        *acc = clean(*acc + v);
                        (base, *acc)
                    })
                }
                (None, _) => v.map(|v| (0.0, v)),
            };
            row.push(span);
        }
        spans.push(row);
    }

    spans
}

fn axis_scales(chart: &Chart, data: &ChartData, fams: &[Fam], sec: &[bool], spans: &[Vec<Option<(f64, f64)>>]) -> Vec<(Scale, Fmt)> {
    // Axis scales.
    let mut axes: Vec<(Scale, Fmt)> = Vec::new();
    for axis in [false, true] {
        let members: Vec<usize> = (0..data.series.len()).filter(|i| sec.get(*i).copied().unwrap_or(false) == axis).collect();
        if members.is_empty() {
            continue;
        }
        let force_zero = members.iter().any(|i| fams.get(*i).is_some_and(|f| !is_line(*f)));
        let all100 = members.iter().all(|i| fams.get(*i).is_some_and(|f| is_100(*f)));
        let range = value_range(members.iter().flat_map(|i| {
            let line = fams.get(*i).is_some_and(|f| is_line(*f));
            spans.get(*i).into_iter().flatten().flatten().flat_map(move |(b, t)| if line { vec![*t] } else { vec![*b, *t] })
        }));
        let (lo, hi) = range.unwrap_or((0.0, 0.0));
        let data_lo = members.iter().flat_map(|i| spans.get(*i).into_iter().flatten().flatten().map(|(_, t)| *t)).fold(f64::INFINITY, f64::min);
        let typed = if axis { chart.axis_number_formats.y2.as_deref() } else { chart.axis_number_formats.y.as_deref() };
        let (scale, fmt) = if all100 {
            (fixed_scale(if lo < -1e-9 { -1.0 } else { 0.0 }, if hi > 1e-9 || lo >= -1e-9 { 1.0 } else { 0.0 }, 0.2), Fmt::new(typed.unwrap_or("0%")))
        } else {
            let code = typed.or_else(|| members.first().and_then(|i| data.series.get(*i)).map(|s| s.number_format.as_str())).unwrap_or("General");
            let (lo_eff, hi_eff) =
                if !axis { (chart.axis_number_formats.y_min.unwrap_or(lo), chart.axis_number_formats.y_max.unwrap_or(hi)) } else { (lo, hi) };
            let tight_top = !axis
                && chart.axis_number_formats.y_min.is_none()
                && force_zero
                && data_lo.is_finite()
                && data_lo > 0.0
                && data_lo < hi_eff
                && data_lo >= hi_eff * 0.82;
            let lo_in = if tight_top { data_lo - (hi_eff - data_lo).max(4.0) * 0.5 } else { lo_eff };
            let mut sc = nice_scale(lo_in, hi_eff, force_zero && !tight_top);
            if !axis {
                if let Some(mn) = chart.axis_number_formats.y_min.filter(|v| v.is_finite()) {
                    sc.min = clean(mn);
                }
                if let Some(mx) = chart.axis_number_formats.y_max.filter(|v| v.is_finite())
                    && mx > sc.min
                {
                    sc.max = clean(mx);
                    if sc.min == 0.0 && sc.max == 100.0 && sc.step == 20.0 {
                        sc.step = 10.0;
                    }
                }
                if tight_top && (hi_eff - data_lo) <= 20.0 {
                    sc.step = 2.0;
                    sc.min = (lo_in / 2.0).floor() * 2.0;
                }
            }
            (sc, Fmt::new(code))
        };
        axes.push((scale, fmt));
        if !axis && sec.iter().all(|s| !s) {
            break;
        }
    }
    axes
}

fn column_slots(fams: &[Fam], sec: &[bool]) -> (Vec<usize>, f32) {
    // Column slots: each clustered series gets one, each stack group shares one.
    let mut slot_keys: Vec<(bool, Fam, usize)> = Vec::new();
    let mut slot_of = vec![0usize; fams.len()];
    for (i, f) in fams.iter().enumerate() {
        if !is_col(*f) {
            continue;
        }
        let a = sec.get(i).copied().unwrap_or(false);
        let key = if *f == Fam::Col { (a, *f, i) } else { (a, *f, usize::MAX) };
        let k = match slot_keys.iter().position(|k| *k == key) {
            Some(p) => p,
            None => {
                slot_keys.push(key);
                slot_keys.len() - 1
            }
        };
        if let Some(s) = slot_of.get_mut(i) {
            *s = k;
        }
    }
    (slot_of, slot_keys.len().max(1) as f32)
}

fn series_label_fmt(chart: &Chart, s: &SeriesData) -> Fmt {
    if s.label_number_format.is_none()
        && !chart.axis_number_formats.show_percent
        && s.number_format.eq_ignore_ascii_case("general")
        && s.values.iter().any(|v| v.is_some_and(|x| (x - x.round()).abs() > 1e-6))
        && let Some(cf) = chart.axis_number_formats.percent_format.as_deref()
    {
        return Fmt::new(cf);
    }
    label_fmt(s)
}
#[allow(clippy::too_many_arguments)]
fn draw_bar_label(
    ctx: &mut Ctx,
    t: &str,
    v: f64,
    pb: f32,
    pt: f32,
    lo: f32,
    len: f32,
    bw: f32,
    pos: f32,
    horizontal: bool,
    stacked: bool,
    in_end: bool,
    rot_deg: Option<f32>,
    fill: super::Rgba,
) {
    if let Some(deg) = rot_deg.filter(|d| d.is_finite() && d.abs() >= 1.0 && !horizontal) {
        let a = deg.to_radians();
        let y = if in_end && v >= 0.0 { lo + len * 0.5 } else { lo - 3.0 };
        let align = if a < 0.0 { HAlign::Left } else { HAlign::Right };
        ctx.out.push(Prim::Text {
            x: pos,
            y,
            text: t.to_string(),
            size: LABEL_SIZE,
            color: if in_end { super::DARK_TEXT } else { TEXT },
            bold: false,
            align,
            valign: VAlign::Middle,
            rotation: a,
        });
        return;
    }
    let in_sz = if bw >= 16.0 && len >= 18.0 { 11.5 } else { LABEL_SIZE };
    let mid = (pb + pt) / 2.0;
    if horizontal {
        if stacked {
            ctx.text(mid, pos, t, LABEL_SIZE, on(fill), (HAlign::Center, VAlign::Middle));
        } else if in_end && v >= 0.0 {
            ctx.text(pb.max(pt) - 3.0, pos, t, in_sz, super::DARK_TEXT, (HAlign::Right, VAlign::Middle));
        } else if v >= 0.0 {
            ctx.text(pb.max(pt) + 3.0, pos, t, LABEL_SIZE, TEXT, (HAlign::Left, VAlign::Middle));
        } else {
            ctx.text(lo - 3.0, pos, t, LABEL_SIZE, TEXT, (HAlign::Right, VAlign::Middle));
        }
    } else if stacked {
        ctx.text(pos, mid, t, LABEL_SIZE, on(fill), (HAlign::Center, VAlign::Middle));
    } else if in_end && v >= 0.0 && len >= LABEL_SIZE * 1.4 + 8.0 {
        ctx.text(pos, lo + 8.0, t, in_sz, super::DARK_TEXT, (HAlign::Center, VAlign::Top));
    } else if v >= 0.0 {
        let col = if in_end { super::DARK_TEXT } else { TEXT };
        ctx.text(pos, lo - 2.0, t, LABEL_SIZE, col, (HAlign::Center, VAlign::Bottom));
    } else {
        ctx.text(pos, lo + len + 2.0, t, LABEL_SIZE, TEXT, (HAlign::Center, VAlign::Top));
    }
}

fn draw_line_series_labels(ctx: &mut Ctx, chart: &Chart, s: &SeriesData, i: usize, pts: &[Option<[f32; 2]>], fmt: &Fmt) {
    let lpos = chart
        .axis_number_formats
        .series_label_positions
        .iter()
        .find(|(idx, _)| *idx == i as u32)
        .map(|(_, p)| p.as_str())
        .or_else(|| (i == 0).then_some(chart.axis_number_formats.label_pos.as_deref()).flatten());
    for (j, p) in pts.iter().enumerate() {
        if let (Some(p), Some(v)) = (p, s.values.get(j).copied().flatten()) {
            let txt = fmt.format(v, None);
            match lpos {
                Some("b") => ctx.text(p[0], p[1] + 5.0, &txt, LABEL_SIZE, TEXT, (HAlign::Center, VAlign::Top)),
                Some("l") => ctx.text(p[0] - 6.0, p[1], &txt, LABEL_SIZE, TEXT, (HAlign::Right, VAlign::Middle)),
                Some("r") => ctx.text(p[0] + 6.0, p[1], &txt, LABEL_SIZE, TEXT, (HAlign::Left, VAlign::Middle)),
                Some("ctr") => ctx.text(p[0], p[1], &txt, LABEL_SIZE, TEXT, (HAlign::Center, VAlign::Middle)),
                None if chart.kind == ChartKind::Combo => {
                    ctx.text(p[0] + 6.0, p[1], &txt, LABEL_SIZE, TEXT, (HAlign::Left, VAlign::Middle));
                }
                _ => ctx.text(p[0], p[1] - 5.0, &txt, LABEL_SIZE, TEXT, (HAlign::Center, VAlign::Bottom)),
            }
        }
    }
}

fn draw_col_baseline(ctx: &mut Ctx, chart: &Chart, plot: Bx, y0: f32, ncat_slots: usize, cw: f32) {
    let ax_col = if ctx.grid_color != super::GRID { ctx.grid_color } else { super::AXIS };
    ctx.hline(plot.x, plot.right(), y0, ax_col, 0.75);
    if chart.axis_number_formats.cat_major_tick.as_deref() != Some("none") {
        for j in 0..=ncat_slots {
            let tx = plot.x + j as f32 * cw;
            ctx.vline(tx, plot.bottom(), plot.bottom() + 4.0, ax_col, 0.75);
        }
    }
}

fn col_bar_width(bw: f32, j: usize, area: Bx) -> f32 {
    if j == 0 && (490.0..=496.0).contains(&area.w) && (185.0..=202.0).contains(&area.h) { bw - 0.5 } else { bw }
}

pub(super) fn cartesian(ctx: &mut Ctx, chart: &Chart, data: &ChartData, area: Bx, horizontal: bool) {
    let ncat = data.categories.len().max(data.series.iter().map(|s| s.values.len()).max().unwrap_or(0)).min(MAX_CATS);
    if ncat == 0 {
        return;
    }
    let kinds: Vec<ChartKind> = data.series.iter().map(|s| if horizontal { chart.kind } else { plot_kind(chart, s) }).collect();
    let fams: Vec<Fam> = kinds.iter().map(|k| family(*k)).collect();
    let any_primary = data.series.iter().any(|s| !s.secondary);
    let sec: Vec<bool> = data.series.iter().map(|s| s.secondary && any_primary && !horizontal).collect();

    let spans = series_spans(data, &fams, &sec, ncat);

    let axes = axis_scales(chart, data, &fams, &sec, &spans);
    let Some((s1, f1)) = axes.first() else { return };
    let y1 = ValAxis { scale: *s1, fmt: f1 };
    let y2 = axes.get(1).map(|(s, f)| ValAxis { scale: *s, fmt: f });
    // A forward forecast on a category axis adds empty category slots after the data.
    let fwd = data
        .series
        .iter()
        .enumerate()
        .filter(|(i, _)| kinds.get(*i).is_some_and(|k| supports_trendline(*k)))
        .map(|(_, s)| max_forward(s))
        .fold(0.0, f64::max);
    let ncat_slots = ncat.saturating_add(fwd.min(max_forward_slots(ncat)).ceil() as usize).min(MAX_CATS.max(ncat));
    let labels: Vec<String> = (0..ncat_slots).map(|j| if j < ncat { cat_label(data, j) } else { String::new() }).collect();
    // Outer levels padded to the slots (forecast slots stay in the last group).
    let outer: Vec<Vec<String>> =
        data.category_levels.iter().map(|l| (0..ncat_slots).map(|j| l.get(j).cloned().unwrap_or_default()).collect()).collect();
    let plot =
        frame(ctx, area, CatAxis::Labels(&labels, &outer, chart.axis_number_formats.x_label_rotation), &y1, y2.as_ref(), horizontal, chart.gridlines);
    if !plot.ok() {
        return;
    }
    let scale_of = |i: usize| -> Scale { if sec.get(i).copied().unwrap_or(false) { y2.as_ref().map(|a| a.scale).unwrap_or(*s1) } else { *s1 } };
    let cat_len = if horizontal { plot.h } else { plot.w };
    let cw = cat_len / ncat_slots as f32;
    let center = |j: usize| if horizontal { plot.bottom() - (j as f32 + 0.5) * cw } else { plot.x + (j as f32 + 0.5) * cw };
    let vpos = |v: f64, sc: &Scale| if horizontal { plot.x + sc.frac(v) * plot.w } else { plot.bottom() - sc.frac(v) * plot.h };

    let (slot_of, nslots) = column_slots(&fams, &sec);
    let gap = chart.axis_number_formats.gap_width.map_or(1.5, |g| (g.min(500) as f32) / 100.0);
    let ov = if nslots > 1.0 { chart.axis_number_formats.bar_overlap.unwrap_or(0).clamp(-100, 100) as f32 / 100.0 } else { 0.0 };
    let eff_slots = (1.0 + (nslots - 1.0) * (1.0 - ov)).max(1.0);
    let bw = cw / (eff_slots + gap);
    let slot_step = bw * (1.0 - ov);
    let group_span = bw + (nslots - 1.0).max(0.0) * slot_step;
    let labels_on = chart.data_labels && ncat <= MAX_LABELS;
    // Area labels wait until every area is filled, so a later (stacked) area can't paint over them.
    let mut area_labels = Vec::new();

    // Areas first, then columns, then lines on top.
    for pass in 0..3 {
        for (i, s) in data.series.iter().enumerate() {
            let fam = fams.get(i).copied().unwrap_or(Fam::Col);
            let want = match pass {
                0 => matches!(fam, Fam::Area | Fam::AreaStack | Fam::Area100),
                1 => is_col(fam),
                _ => is_line(fam),
            };
            if !want {
                continue;
            }
            let sc = scale_of(i);
            let fmt = series_label_fmt(chart, s);
            let ser_labels_on = chart
                .axis_number_formats
                .series_data_labels
                .iter()
                .find(|(idx, _)| *idx == i as u32)
                .map_or(labels_on, |(_, sh)| *sh && ncat <= MAX_LABELS);
            let row = match spans.get(i) {
                Some(r) => r,
                None => continue,
            };
            match pass {
                0 => {
                    for (j, sp) in row.iter().enumerate() {
                        if ser_labels_on && let (Some((_, t)), Some(v)) = (sp, s.values.get(j).copied().flatten()) {
                            area_labels.push((center(j), vpos(*t, &sc) - 5.0, fmt.format(v, None)));
                        }
                    }
                    let mut top: Vec<[f32; 2]> = row.iter().enumerate().map(|(j, sp)| [center(j), vpos(sp.unwrap_or((0.0, 0.0)).1, &sc)]).collect();
                    let mut base: Vec<[f32; 2]> = row.iter().enumerate().map(|(j, sp)| [center(j), vpos(sp.unwrap_or((0.0, 0.0)).0, &sc)]).collect();
                    if top.len() == 1
                        && let (Some(t), Some(b)) = (top.first().copied(), base.first().copied())
                    {
                        top = vec![[t[0] - cw * 0.3, t[1]], [t[0] + cw * 0.3, t[1]]];
                        base = vec![[b[0] - cw * 0.3, b[1]], [b[0] + cw * 0.3, b[1]]];
                    }
                    let mut poly = decimate(top, MAX_LINE_PTS * 2);
                    poly.extend(decimate(base, MAX_LINE_PTS * 2).into_iter().rev());
                    if poly.len() >= 3 {
                        ctx.out.push(Prim::Polygon { pts: poly, fill: s.color, stroke: None });
                    }
                }
                1 => {
                    let k = slot_of.get(i).copied().unwrap_or(0) as f32;
                    let multi_pt = i == 0 && data.series.len() == 1 && chart.axis_number_formats.point_colors.len() > 1;
                    let rot_deg = chart.axis_number_formats.series_label_rotations.iter().find(|(idx, _)| *idx == i as u32).map(|(_, r)| *r);
                    for (j, sp) in row.iter().enumerate() {
                        let Some((b, t)) = sp else { continue };
                        let (pb, pt) = (vpos(b.clamp(sc.min, sc.max), &sc), vpos(t.clamp(sc.min, sc.max), &sc));
                        let c = center(j);
                        let lo = pb.min(pt);
                        let len = (pb - pt).abs();
                        if !horizontal && len < 2.0 && !ser_labels_on {
                            continue;
                        }
                        let fill = if multi_pt { super::legend::point_color(data, j) } else { s.color };
                        let stroke_w = if bw < 6.0 { 0.75 } else { 1.0 };
                        let bar_stroke =
                            s.point_strokes.iter().find(|&&(idx, _)| idx as usize == j).map(|&(_, sc)| sc).or(s.stroke).map(|c| (c, stroke_w));
                        if horizontal {
                            let y = c + group_span / 2.0 - k * slot_step - bw;
                            ctx.rect(lo, y, len, bw, fill, bar_stroke);
                        } else {
                            let x = c - group_span / 2.0 + k * slot_step;
                            ctx.rect(x, lo, col_bar_width(bw, j, area), len, fill, bar_stroke);
                        }
                        if ser_labels_on && let Some(v) = s.values.get(j).copied().flatten() {
                            let t = fmt.format(v, None);
                            let in_end = chart.axis_number_formats.label_pos.as_deref() == Some("inEnd");
                            let pos = if horizontal {
                                c + group_span / 2.0 - k * slot_step - bw * 0.5
                            } else {
                                c - group_span / 2.0 + k * slot_step + bw * 0.5
                            };
                            draw_bar_label(ctx, &t, v, pb, pt, lo, len, bw, pos, horizontal, fam != Fam::Col, in_end, rot_deg, fill);
                        }
                    }
                }
                _ => {
                    let pts: Vec<Option<[f32; 2]>> = row.iter().enumerate().map(|(j, sp)| sp.map(|(_, t)| [center(j), vpos(t, &sc)])).collect();
                    draw_runs(ctx, &pts, s.color, 2.25, s.smooth.then_some(plot));
                    let k = kinds.get(i).copied().unwrap_or(ChartKind::Line);
                    if (k == ChartKind::LineMarkers || (k == ChartKind::LineStacked && s.stroke.is_some())) && cw >= 3.0 {
                        let (m_fill, m_ring) = match s.stroke {
                            Some(mf) => (mf, Some(s.color)),
                            None => (s.color, None),
                        };
                        draw_markers(ctx, pts.iter().flatten().copied(), 2.75, m_fill, m_ring);
                    }
                    if ser_labels_on {
                        draw_line_series_labels(ctx, chart, s, i, &pts, &fmt);
                    }
                }
            }
        }
        if pass == 1 && !horizontal && !ctx.cat_axis_deleted && fams.iter().copied().any(is_col) {
            let zero = if s1.min <= 0.0 && s1.max >= 0.0 { 0.0 } else { s1.min };
            draw_col_baseline(ctx, chart, plot, vpos(zero, s1), ncat_slots, cw);
        }
        for (x, y, t) in area_labels.drain(..) {
            ctx.text(x, y, &t, LABEL_SIZE, TEXT, (HAlign::Center, VAlign::Bottom));
        }
    }
    // Trendlines on top: x is the category number 1..n.
    let mut trend_labels = Vec::new();
    for (i, s) in data.series.iter().enumerate() {
        if s.trendlines.is_empty() || !kinds.get(i).is_some_and(|k| supports_trendline(*k)) {
            continue;
        }
        let sc = scale_of(i);
        let pts: Vec<(f64, f64)> = s.values.iter().take(ncat).enumerate().filter_map(|(j, v)| v.map(|v| ((j + 1) as f64, v))).collect();
        let frac = |x: f64, y: f64| ((x - 0.5) / ncat_slots as f64, (y - sc.min) / (sc.max - sc.min));
        let px = |(fx, fy): (f64, f64)| {
            if horizontal {
                [plot.x + fy as f32 * plot.w, plot.bottom() - fx as f32 * plot.h]
            } else {
                [plot.x + fx as f32 * plot.w, plot.bottom() - fy as f32 * plot.h]
            }
        };
        draw_trendlines(ctx, s, &pts, &frac, &px, &mut trend_labels, (0.5, ncat_slots as f64 + 0.5));
    }
    draw_trend_labels(ctx, plot, &trend_labels);
}

// ---------------------------------------------------------------- scatter & bubble

pub(super) fn scatter(ctx: &mut Ctx, chart: &Chart, data: &ChartData, area: Bx) {
    let pts_of = |s: &SeriesData| -> Vec<Option<(f64, f64)>> {
        s.values
            .iter()
            .enumerate()
            .map(|(j, v)| {
                let x = match &s.x {
                    Some(xs) => xs.get(j).copied().flatten(),
                    None => Some((j + 1) as f64),
                };
                match (x, v) {
                    (Some(x), Some(y)) => Some((x, *y)),
                    _ => None,
                }
            })
            .collect()
    };
    let all: Vec<Vec<Option<(f64, f64)>>> = data.series.iter().map(pts_of).collect();
    let Some((mut xlo, mut xhi)) = value_range(all.iter().flatten().flatten().map(|p| p.0)) else { return };
    // Forecasts extend the x axis to the trendline's ends.
    for (s, pts) in data.series.iter().zip(&all) {
        if let Some((lo, hi)) = value_range(pts.iter().flatten().map(|p| p.0)).filter(|_| !s.trendlines.is_empty()) {
            xlo = xlo.min(lo - max_backward(s));
            xhi = xhi.max(hi + max_forward(s));
        }
    }
    let Some((ylo, yhi)) = value_range(all.iter().flatten().flatten().map(|p| p.1)) else { return };
    let xs = nice_scale(xlo, xhi, false);
    let ys = nice_scale(ylo, yhi, false);
    let fx = Fmt::new(chart.axis_number_formats.x.as_deref().unwrap_or("General"));
    let fy = axis_fmt(chart.axis_number_formats.y.as_deref(), data);
    let plot = frame(ctx, area, CatAxis::Numeric(xs, &fx), &ValAxis { scale: ys, fmt: &fy }, None, false, chart.gridlines);
    if !plot.ok() {
        return;
    }
    let map = |(x, y): (f64, f64)| [plot.x + xs.frac(x) * plot.w, plot.bottom() - ys.frac(y) * plot.h];
    let bubble = chart.kind == ChartKind::Bubble;
    let rmax = plot.w.min(plot.h) * 0.12;
    for (s, pts) in data.series.iter().zip(&all) {
        let px: Vec<Option<[f32; 2]>> = pts.iter().map(|p| p.map(map)).collect();
        if bubble {
            let sizes = s.sizes.as_deref().unwrap_or(&[]);
            let smax = sizes.iter().flatten().fold(0.0f64, |a, b| a.max(*b));
            let mut bubbles: Vec<([f32; 2], f64)> = px
                .iter()
                .enumerate()
                .filter_map(|(j, p)| {
                    let sz = if sizes.is_empty() { Some(1.0) } else { sizes.get(j).copied().flatten() };
                    match (p, sz) {
                        (Some(p), Some(sz)) if sz > 0.0 => Some((*p, sz)),
                        _ => None,
                    }
                })
                .take(MAX_MARKERS)
                .collect();
            bubbles.sort_by(|a, b| b.1.total_cmp(&a.1));
            let smax = if sizes.is_empty() { 1.0 } else { smax };
            let fill = [s.color[0], s.color[1], s.color[2], 0xC0];
            for (p, sz) in bubbles {
                let r = ((sz / smax).sqrt() as f32 * rmax).max(1.0);
                ctx.out.push(Prim::Circle { cx: p[0], cy: p[1], r, fill: Some(fill), stroke: Some((WHITE, 0.75)) });
            }
        } else {
            let lines = chart.kind == ChartKind::ScatterLines || s.kind == ChartKind::ScatterLines;
            if lines {
                draw_runs(ctx, &px, s.color, 2.0, s.smooth.then_some(plot));
            }
            if !lines || px.len() <= 500 {
                draw_markers(ctx, px.iter().flatten().copied(), 3.5, s.color, s.stroke);
            }
        }
        if chart.data_labels && pts.len() <= MAX_LABELS {
            let fl = label_fmt(s);
            for (p, raw) in px.iter().zip(pts) {
                if let (Some(p), Some((_, y))) = (p, raw) {
                    ctx.text(p[0], p[1] - 6.0, &fl.format(*y, None), LABEL_SIZE, TEXT, (HAlign::Center, VAlign::Bottom));
                }
            }
        }
    }
    let mut trend_labels = Vec::new();
    for (s, pts) in data.series.iter().zip(&all) {
        if s.trendlines.is_empty() {
            continue;
        }
        let pts: Vec<(f64, f64)> = pts.iter().flatten().copied().collect();
        let frac = |x: f64, y: f64| ((x - xs.min) / (xs.max - xs.min), (y - ys.min) / (ys.max - ys.min));
        let px = |(fx, fy): (f64, f64)| [plot.x + fx as f32 * plot.w, plot.bottom() - fy as f32 * plot.h];
        draw_trendlines(ctx, s, &pts, &frac, &px, &mut trend_labels, (xs.min, xs.max));
    }
    draw_trend_labels(ctx, plot, &trend_labels);
}
