use super::{AXIS, AXIS_TITLE_SIZE, Chart, ChartData, Ctx, DARK_TEXT, Fmt, HAlign, LABEL_SIZE, PI, Prim, Rgba, Scale, TEXT, VAlign, WHITE};

#[derive(Clone, Copy, Debug)]
pub(super) struct Bx {
    pub(super) x: f32,
    pub(super) y: f32,
    pub(super) w: f32,
    pub(super) h: f32,
}

impl Bx {
    pub(super) fn right(&self) -> f32 {
        self.x + self.w
    }
    pub(super) fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub(super) fn ok(&self) -> bool {
        self.w.is_finite() && self.h.is_finite() && self.w > 4.0 && self.h > 4.0
    }
    pub(super) fn cx(&self) -> f32 {
        self.x + self.w / 2.0
    }
    pub(super) fn cy(&self) -> f32 {
        self.y + self.h / 2.0
    }
}

/// Text colour readable on `fill`.
pub(super) fn on(fill: Rgba) -> Rgba {
    let l = 0.299 * fill[0] as f32 + 0.587 * fill[1] as f32 + 0.114 * fill[2] as f32;
    if l < 150.0 { WHITE } else { DARK_TEXT }
}

pub(super) fn chart_title(chart: &Chart, data: &ChartData) -> Option<String> {
    match &chart.title {
        Some(t) => Some(t.trim().to_string()).filter(|t| !t.is_empty()),
        None if data.series.len() == 1 => data.series.first().map(|s| s.name.clone()).filter(|t| !t.is_empty()),
        None => None,
    }
}

pub(super) fn axis_titles(ctx: &mut Ctx, chart: &Chart, mut area: Bx) -> Bx {
    let bold = chart.axis_number_formats.axis_title_bold;
    let lh = if bold { AXIS_TITLE_SIZE * 1.8 + 6.0 } else { AXIS_TITLE_SIZE * 1.5 + 3.0 };
    if let Some(t) = chart.x_title.as_deref().filter(|t| !t.is_empty()) {
        let trimmed = t.trim();
        if !trimmed.is_empty() {
            let t = ctx.fit(trimmed, AXIS_TITLE_SIZE, area.w);
            if !t.is_empty() {
                ctx.out.push(Prim::Text {
                    x: area.cx(),
                    y: area.bottom() - 1.5,
                    text: t,
                    size: AXIS_TITLE_SIZE,
                    color: TEXT,
                    bold,
                    align: HAlign::Center,
                    valign: VAlign::Bottom,
                    rotation: 0.0,
                });
            }
        }
        area.h -= lh;
    }
    if let Some(t) = chart.y_title.as_deref().filter(|t| !t.is_empty()) {
        let trimmed = t.trim();
        if !trimmed.is_empty() {
            let t = ctx.fit(trimmed, AXIS_TITLE_SIZE, area.h);
            if !t.is_empty() {
                ctx.y_title_prims.push(ctx.out.len());
                ctx.out.push(Prim::Text {
                    x: area.x,
                    y: area.cy(),
                    text: t,
                    size: AXIS_TITLE_SIZE,
                    color: TEXT,
                    bold,
                    align: HAlign::Center,
                    valign: VAlign::Top,
                    rotation: -PI / 2.0,
                });
            }
            area.x += lh;
            area.w -= lh;
        } else {
            let pad = lh * 1.6;
            area.x += pad;
            area.w -= pad;
        }
    }
    if let Some(t) = chart.axis_number_formats.y2_title.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        let t = ctx.fit(t, AXIS_TITLE_SIZE, area.h);
        if !t.is_empty() {
            ctx.y_title_prims.push(ctx.out.len());
            ctx.out.push(Prim::Text {
                x: area.right() - lh * 0.45,
                y: area.cy(),
                text: t,
                size: AXIS_TITLE_SIZE,
                color: TEXT,
                bold,
                align: HAlign::Center,
                valign: VAlign::Middle,
                rotation: PI / 2.0,
            });
        }
        area.w -= lh;
    }
    area
}

// ---------------------------------------------------------------- axes frame

pub(super) enum CatAxis<'a> {
    /// Category labels and, for a multi-level axis, the outer levels (nearest the labels first;
    /// one entry per category, non-empty where a group starts).
    Labels(&'a [String], &'a [Vec<String>], Option<f32>),
    Numeric(Scale, &'a Fmt),
}

/// Groups of an outer category level: `(first, end)` index pairs, a group starting at every
/// non-empty entry (and at the first category).
fn level_groups(level: &[String], n: usize) -> Vec<(usize, usize)> {
    let mut starts: Vec<usize> = (0..n).filter(|&j| j == 0 || level.get(j).is_some_and(|t| !t.is_empty())).collect();
    starts.push(n);
    starts.windows(2).filter_map(|w| Some((*w.first()?, *w.get(1)?))).collect()
}

pub(super) struct ValAxis<'a> {
    pub(super) scale: Scale,
    pub(super) fmt: &'a Fmt,
}

impl ValAxis<'_> {
    fn labels(&self) -> Vec<(f64, String)> {
        self.scale.ticks().into_iter().map(|v| (v, self.fmt.format(v, Some(self.scale.step)))).collect()
    }
}

fn draw_cat_labels(ctx: &mut Ctx, plot: Bx, l: &[String], outer: &[Vec<String>], rot: Option<f32>, cat_h: f32) {
    if l.is_empty() || ctx.cat_axis_deleted {
        return;
    }
    let ls = LABEL_SIZE;
    let lh = ls * 1.4;
    let n = l.len().max(1);
    let cw = plot.w / n as f32;
    let lw = l.iter().take(2000).map(|t| ctx.tw(t, ls)).fold(0.0f32, f32::max);
    let rot_rad = rot.filter(|r| r.is_finite() && r.abs() >= 1.0).map(|r| r.to_radians());
    let k = match rot_rad {
        Some(a) => (((lh * 1.15) / (cw * a.sin().abs()).max(1e-3)).ceil() as usize).clamp(1, n),
        None if cw >= ls * 2.4 => 1,
        None => (((lw + 6.0) / cw.max(1e-3)).ceil() as usize).clamp(1, n),
    };
    for (j, t) in l.iter().enumerate().step_by(k) {
        let cx = plot.x + (j as f32 + 0.5) * cw;
        if let Some(a) = rot_rad {
            let eff_cat_h = if ctx.has_bottom_legend && !ctx.y_title_prims.is_empty() { cat_h + 15.0 } else { cat_h };
            let r_v = (eff_cat_h - ls * a.cos() * 0.5).max(lh * 0.8) / a.sin().abs().max(0.1);
            let (align, room) = if a > 0.0 {
                let r_h = (ctx.chart_w - cx - 2.0).max(20.0) / a.cos().abs().max(0.1);
                (HAlign::Left, r_v.min(r_h))
            } else {
                let r_h = (cx - 2.0).max(20.0) / a.cos().abs().max(0.1);
                (HAlign::Right, r_v.min(r_h))
            };
            let t = ctx.fit(t, ls, room);
            if !t.is_empty() {
                ctx.out.push(Prim::Text {
                    x: cx,
                    y: plot.bottom() + 3.0,
                    text: t,
                    size: ls,
                    color: TEXT,
                    bold: false,
                    align,
                    valign: VAlign::Top,
                    rotation: a,
                });
            }
        } else {
            let maxw = (cw * k as f32 - 2.0).max(1.0);
            if ctx.tw(t, ls) > maxw
                && let Some((a, b)) = split_two_lines(t)
            {
                let wrap_w = (cw * k as f32 + 6.0).max(1.0);
                let l1 = ctx.fit(a, ls, wrap_w);
                let l2 = ctx.fit(b, ls, wrap_w);
                ctx.text(cx, plot.bottom() + 3.0, &l1, ls, TEXT, (HAlign::Center, VAlign::Top));
                ctx.text(cx, plot.bottom() + 3.0 + lh * 0.85, &l2, ls, TEXT, (HAlign::Center, VAlign::Top));
            } else {
                let t = ctx.fit(t, ls, maxw);
                ctx.text(cx, plot.bottom() + 3.0, &t, ls, TEXT, (HAlign::Center, VAlign::Top));
            }
        }
    }
    for (i, level) in outer.iter().enumerate() {
        let top = plot.bottom() + 3.0 + cat_h + lh * i as f32;
        for (first, end) in level_groups(level, n) {
            let (x0, x1) = (plot.x + first as f32 * cw, plot.x + end as f32 * cw);
            if let Some(t) = level.get(first).filter(|t| !t.is_empty()) {
                let t = ctx.fit(t, ls, x1 - x0 - 2.0);
                ctx.text((x0 + x1) / 2.0, top, &t, ls, TEXT, (HAlign::Center, VAlign::Top));
            }
            if first > 0 {
                ctx.vline(x0, plot.bottom(), top + lh - 2.0, AXIS, 0.75);
            }
        }
        ctx.vline(plot.right(), plot.bottom(), top + lh - 2.0, AXIS, 0.75);
    }
}

fn split_two_lines(s: &str) -> Option<(&str, &str)> {
    let mid = s.len() / 2;
    let pos = s.match_indices(' ').map(|(i, _)| i).min_by_key(|&i| i.abs_diff(mid))?;
    let (a, b) = s.split_at(pos);
    let b = b.trim_start();
    (!a.is_empty() && !b.is_empty()).then_some((a, b))
}

/// Draws gridlines, axis labels and the category axis line; returns the plot rectangle.
fn vertical_cat_band(ctx: &Ctx, area: Bx, cat: &CatAxis, has_y2: bool, ls: f32, lh: f32) -> (f32, f32) {
    if ctx.cat_axis_deleted {
        return (0.0, 2.0);
    }
    match cat {
        CatAxis::Labels(l, o, Some(rot)) if *rot != 0.0 => {
            let lw = l.iter().take(2000).map(|t| ctx.tw(t, ls)).fold(0.0f32, f32::max);
            let a = rot.abs().to_radians();
            let raw_h = lw * a.sin() + ls * a.cos();
            let ch = if ctx.has_bottom_legend && has_y2 && !ctx.y_title_prims.is_empty() {
                raw_h.min(area.h * 0.31).min(50.0).max(lh)
            } else if ctx.has_bottom_legend && has_y2 {
                raw_h.min(area.h * 0.42).max(lh)
            } else if ctx.has_bottom_legend {
                (raw_h + 6.0).min(area.h * 0.47).max(lh)
            } else if area.h < 220.0 {
                raw_h.min(area.h * 0.35).min(62.0).max(lh)
            } else {
                (raw_h + 4.0).min(area.h * 0.45).max(area.h * 0.295).max(lh)
            };
            (o.len() as f32, ch)
        }
        CatAxis::Labels(l, o, _) => {
            let cw = (area.w * 0.85) / l.len().max(1) as f32;
            let wraps = cw >= ls * 2.4 && l.iter().take(200).any(|t| ctx.tw(t, ls) > cw - 2.0 && t.contains(' '));
            (o.len() as f32, if wraps { lh * 1.85 } else { lh })
        }
        CatAxis::Numeric(..) => (0.0, lh),
    }
}

fn vertical_right_margin(ctx: &Ctx, area: Bx, left: f32, cat_h: f32, y2labels: &Option<Vec<(f64, String)>>, cat: &CatAxis) -> f32 {
    let ls = LABEL_SIZE;
    let lh = ls * 1.4;
    match (y2labels, cat) {
        (Some(l), _) => (l.iter().map(|(_, t)| ctx.tw(t, ls)).fold(0.0f32, f32::max) + 6.0).min(area.w * 0.3),
        (None, CatAxis::Numeric(s, f)) => (ctx.tw(&f.format(s.max, Some(s.step)), ls) / 2.0).min(area.w * 0.2),
        (None, CatAxis::Labels(l, _, Some(rot))) if *rot > 0.0 && !l.is_empty() && !ctx.cat_axis_deleted => {
            let a = rot.to_radians();
            let n = l.len();
            let cw_est = (area.w - left - 4.0).max(10.0) / n as f32;
            let r_v = (cat_h - ls * a.cos() * 0.5).max(lh * 0.8) / a.sin().abs().max(0.1);
            let mut over = 4.0f32;
            for (j, t) in l.iter().enumerate().rev().take(4) {
                let w = ctx.tw(t, ls).min(r_v) * a.cos();
                let dist = (n as f32 - 0.5 - j as f32) * cw_est;
                over = over.max(w - dist + 4.0);
            }
            over.min(area.w * 0.12).max(4.0)
        }
        _ => 4.0,
    }
}

pub(super) fn frame(ctx: &mut Ctx, area: Bx, cat: CatAxis, y: &ValAxis, y2: Option<&ValAxis>, horizontal: bool, grid: bool) -> Bx {
    let ls = LABEL_SIZE;
    let lh = ls * 1.4;
    let ylabels = y.labels();
    let maxw = |ctx: &Ctx, l: &[(f64, String)]| l.iter().map(|(_, t)| ctx.tw(t, ls)).fold(0.0f32, f32::max);
    if !horizontal {
        let left = (maxw(ctx, &ylabels) + 6.0).min(area.w * 0.4);
        let y2labels = y2.map(ValAxis::labels);
        let (levels, cat_h) = vertical_cat_band(ctx, area, &cat, y2.is_some(), ls, lh);
        let right = vertical_right_margin(ctx, area, left, cat_h, &y2labels, &cat);
        let raw_w = area.w - left - right;
        let (px, pw) = if ctx.cat_axis_deleted { (area.x + left + raw_w * 0.14, raw_w * 0.66) } else { (area.x + left, raw_w) };
        let plot = Bx { x: px, y: area.y + lh * 0.5, w: pw, h: area.h - lh * 0.5 - cat_h - 4.0 - lh * levels };
        if !plot.ok() {
            return plot;
        }
        for &idx in &ctx.y_title_prims {
            if let Some(Prim::Text { y, .. }) = ctx.out.get_mut(idx) {
                *y = plot.cy();
            }
        }
        for (v, t) in &ylabels {
            let yy = plot.bottom() - y.scale.frac(*v) * plot.h;
            if grid {
                ctx.hline(plot.x, plot.right(), yy, ctx.grid_color, 0.75);
            }
            ctx.text(plot.x - 4.0, yy, t, ls, TEXT, (HAlign::Right, VAlign::Middle));
        }
        if let (Some(a), Some(l)) = (y2, &y2labels) {
            for (v, t) in l {
                let yy = plot.bottom() - a.scale.frac(*v) * plot.h;
                ctx.text(plot.right() + 4.0, yy, t, ls, TEXT, (HAlign::Left, VAlign::Middle));
            }
        }
        let zero = if y.scale.min <= 0.0 && y.scale.max >= 0.0 { 0.0 } else { y.scale.min };
        let y0 = plot.bottom() - y.scale.frac(zero) * plot.h;
        let ax_col = if ctx.grid_color != super::GRID { ctx.grid_color } else { AXIS };
        ctx.hline(plot.x, plot.right(), y0, ax_col, 0.75);
        if ctx.grid_color != super::GRID {
            ctx.vline(plot.x, plot.y, plot.bottom(), ctx.grid_color, 0.75);
        }
        match cat {
            CatAxis::Labels(l, outer, rot) => draw_cat_labels(ctx, plot, l, outer, rot, cat_h),
            CatAxis::Numeric(s, f) => {
                let ticks = s.ticks();
                let labels: Vec<String> = ticks.iter().map(|v| f.format(*v, Some(s.step))).collect();
                let lw = labels.iter().map(|t| ctx.tw(t, ls)).fold(0.0f32, f32::max);
                let slot = plot.w / ticks.len().max(1) as f32;
                let k = (((lw + 6.0) / slot.max(1e-3)).ceil() as usize).max(1);
                for (v, t) in ticks.iter().zip(&labels).step_by(k) {
                    let xx = plot.x + s.frac(*v) * plot.w;
                    ctx.text(xx, plot.bottom() + 3.0, t, ls, TEXT, (HAlign::Center, VAlign::Top));
                }
                if s.min <= 0.0 && s.max >= 0.0 {
                    let xx = plot.x + s.frac(0.0) * plot.w;
                    ctx.vline(xx, plot.y, plot.bottom(), AXIS, 0.75);
                }
            }
        }
        plot
    } else {
        let (labels, outer): (Vec<String>, &[Vec<String>]) = match cat {
            CatAxis::Labels(l, o, _) => (l.to_vec(), o),
            CatAxis::Numeric(..) => (vec![], &[]),
        };
        let lw = (labels.iter().take(2000).map(|t| ctx.tw(t, ls)).fold(0.0f32, f32::max) + 6.0).min(area.w * 0.35);
        // Outer levels stand in columns left of the labels, each as wide as its longest name.
        let level_w: Vec<f32> =
            outer.iter().map(|l| (l.iter().take(2000).map(|t| ctx.tw(t, ls)).fold(0.0f32, f32::max) + 6.0).min(area.w * 0.15)).collect();
        let levels_w: f32 = level_w.iter().sum();
        let right = ylabels.last().map(|(_, t)| ctx.tw(t, ls) / 2.0).unwrap_or(0.0).min(area.w * 0.2) + 2.0;
        let plot = Bx { x: area.x + lw + levels_w, y: area.y + 2.0, w: area.w - lw - levels_w - right, h: area.h - lh - 6.0 };
        if !plot.ok() {
            return plot;
        }
        let vw = maxw(ctx, &ylabels);
        let slot = plot.w / ylabels.len().max(1) as f32;
        let k = (((vw + 6.0) / slot.max(1e-3)).ceil() as usize).max(1);
        for (i, (v, t)) in ylabels.iter().enumerate() {
            let xx = plot.x + y.scale.frac(*v) * plot.w;
            if grid {
                ctx.vline(xx, plot.y, plot.bottom(), ctx.grid_color, 0.75);
            }
            if i % k == 0 {
                ctx.text(xx, plot.bottom() + 3.0, t, ls, TEXT, (HAlign::Center, VAlign::Top));
            }
        }
        let zero = if y.scale.min <= 0.0 && y.scale.max >= 0.0 { 0.0 } else { y.scale.min };
        let x0 = plot.x + y.scale.frac(zero) * plot.w;
        ctx.vline(x0, plot.y, plot.bottom(), AXIS, 0.75);
        let n = labels.len().max(1);
        let ch = plot.h / n as f32;
        let k = ((lh / ch.max(1e-3)).ceil() as usize).clamp(1, n);
        for (j, t) in labels.iter().enumerate().step_by(k) {
            let t = ctx.fit(t, ls, lw - 6.0);
            ctx.text(plot.x - 4.0, plot.bottom() - (j as f32 + 0.5) * ch, &t, ls, TEXT, (HAlign::Right, VAlign::Middle));
        }
        // Outer levels: a group's name centred beside its categories, separators at the
        // boundaries running right to the axis.
        let mut x1 = plot.x - lw;
        for (level, w) in outer.iter().zip(&level_w) {
            let x0 = x1 - w;
            for (first, end) in level_groups(level, n) {
                let (y0, y1) = (plot.bottom() - end as f32 * ch, plot.bottom() - first as f32 * ch);
                if let Some(t) = level.get(first).filter(|t| !t.is_empty()) {
                    let t = ctx.fit(t, ls, w - 4.0);
                    ctx.text((x0 + x1) / 2.0, (y0 + y1) / 2.0, &t, ls, TEXT, (HAlign::Center, VAlign::Middle));
                }
                if first > 0 {
                    ctx.hline(x0, plot.x, y1, AXIS, 0.75);
                }
            }
            ctx.hline(x0, plot.x, plot.y, AXIS, 0.75);
            x1 = x0;
        }
        plot
    }
}

pub(super) fn value_range(it: impl Iterator<Item = f64>) -> Option<(f64, f64)> {
    it.fold(None, |acc, v| match acc {
        None => Some((v, v)),
        Some((a, b)) => Some((a.min(v), b.max(v))),
    })
}
