//! Form controls on the sheet: drawing buttons, sliders, spin buttons, drop-downs and check boxes,
//! and working them with the pointer and the keyboard. A click operates a control; Ctrl or
//! Cmd+click, or a right-click, selects it instead (to move, resize or format it). Values change
//! through `control.setValue`, and a whole drag or a held arrow is one undo step. A click on a
//! drop-down opens its list ([`drop_picker`]); choosing an item is one undo step. A click on a
//! check box (or Space while it has the focus) checks or clears it, one undo step.
//!
//! Positions are logical (see `Geo::rtl`): on a right-to-left sheet a control moves as a whole
//! but is not mirrored, so a point inside it is reflected back with [`inner`] first.

use egui::epaint::PathShape;
use egui::{Painter, Pos2, Rect, Stroke, StrokeKind, pos2, vec2};
use gridcraft_engine::model::{CheckState, ControlKind, FormControl, ObjKind, ObjRef, Sheet};
use serde_json::json;

use crate::SheetApp;
use crate::grid::Geo;
use crate::theme::{self, Tokens};

/// How long an arrow or the track waits before it repeats, and then between repeats (seconds).
const REPEAT_DELAY: f64 = 0.4;
const REPEAT_EVERY: f64 = 0.06;

/// The part of a control under the pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Button,
    /// A spin button's arrow that adds a step.
    Up,
    /// A spin button's arrow that takes a step away.
    Down,
    Thumb,
    /// The track on the minimum's side of the thumb (a page down).
    TrackBefore,
    /// The track on the maximum's side of the thumb (a page up).
    TrackAfter,
    /// A drop-down (its box or its arrow): opens or closes the list.
    Drop,
    /// A check box (its box or its caption): checks or clears it.
    Check,
}

/// A control being worked with the pointer.
#[derive(Clone, Debug, PartialEq)]
pub struct Gesture {
    pub id: u32,
    pub part: Part,
    /// Thumb drags: where along the track the pointer grabbed the thumb, from its centre.
    pub grab: f32,
    /// The undo stack's length when the gesture began: everything it changes is one step.
    pub undo_start: usize,
    /// When a held arrow or track repeats next (egui time, seconds).
    pub next_repeat: f64,
}

/// What a control looks like this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Look {
    pub hover: Option<Part>,
    pub pressed: Option<Part>,
    pub focused: bool,
    /// Show the value next to the thumb (while dragging).
    pub show_value: bool,
    /// A drop-down's list is open.
    pub open: bool,
}

/// A drop-down's box and the arrow button at its right end.
pub fn drop_parts(r: Rect) -> (Rect, Rect) {
    let w = r.height().min(r.width() / 2.0).max(0.0);
    let (b, a) = r.split_left_right_at_x(r.right() - w);
    (b, a)
}

fn fraction(c: &FormControl) -> f32 {
    let span = c.max.saturating_sub(c.min).max(1) as f32;
    (c.value.saturating_sub(c.min) as f32 / span).clamp(0.0, 1.0)
}

/// A slider's track and thumb in `r`. A horizontal slider has its minimum on the left; a vertical
/// one has it at the top (as Excel's scroll bars do).
pub fn slider_parts(r: Rect, c: &FormControl, z: f32) -> (Rect, Rect) {
    let f = fraction(c);
    if c.horizontal {
        let knob = (r.height() - 4.0 * z).clamp(6.0 * z, 16.0 * z).min(r.width() / 2.0).max(1.0);
        let track = Rect::from_center_size(r.center(), vec2((r.width() - knob).max(1.0), (4.0 * z).min(r.height())));
        let x = track.left() + f * track.width();
        (track, Rect::from_center_size(pos2(x, r.center().y), vec2(knob, (r.height() - 4.0 * z).max(4.0))))
    } else {
        let knob = (r.width() - 4.0 * z).clamp(6.0 * z, 16.0 * z).min(r.height() / 2.0).max(1.0);
        let track = Rect::from_center_size(r.center(), vec2((4.0 * z).min(r.width()), (r.height() - knob).max(1.0)));
        let y = track.top() + f * track.height();
        (track, Rect::from_center_size(pos2(r.center().x, y), vec2((r.width() - 4.0 * z).max(4.0), knob)))
    }
}

/// A spin button's halves: (the arrow that adds, the arrow that subtracts). Tall spin buttons
/// stack them (up on top); wide ones put them side by side (up on the right).
pub fn spin_parts(r: Rect) -> (Rect, Rect) {
    if r.height() >= r.width() {
        let (top, bottom) = r.split_top_bottom_at_fraction(0.5);
        (top, bottom)
    } else {
        let (left, right) = r.split_left_right_at_fraction(0.5);
        (right, left)
    }
}

/// A logical point `p` inside control rect `r`, as the control is drawn: on a right-to-left
/// sheet the control moves as a whole without mirroring, so `p` is reflected within `r`.
pub fn inner(geo: &Geo, r: Rect, p: Pos2) -> Pos2 {
    if geo.rtl { pos2(r.left() + r.right() - p.x, p.y) } else { p }
}

/// The part of control `c` (drawn in `r`) under `p`.
pub fn part_at(r: Rect, c: &FormControl, z: f32, p: Pos2) -> Option<Part> {
    if !r.contains(p) {
        return None;
    }
    Some(match c.kind {
        ControlKind::Button => Part::Button,
        ControlKind::Spin => {
            let (up, _) = spin_parts(r);
            if up.contains(p) { Part::Up } else { Part::Down }
        }
        ControlKind::Slider => {
            let (_, thumb) = slider_parts(r, c, z);
            if thumb.expand(2.0).contains(p) {
                Part::Thumb
            } else {
                let before = if c.horizontal { p.x < thumb.center().x } else { p.y < thumb.center().y };
                if before { Part::TrackBefore } else { Part::TrackAfter }
            }
        }
        ControlKind::Drop => Part::Drop,
        ControlKind::Check => Part::Check,
    })
}

/// The value a thumb dragged to `p` stands for (`grab`: the offset from the thumb's centre where
/// it was grabbed).
pub fn value_at(r: Rect, c: &FormControl, z: f32, p: Pos2, grab: f32) -> f64 {
    let (track, _) = slider_parts(r, c, z);
    let f = if c.horizontal { (p.x - grab - track.left()) / track.width().max(1.0) } else { (p.y - grab - track.top()) / track.height().max(1.0) };
    c.min as f64 + f.clamp(0.0, 1.0) as f64 * c.max.saturating_sub(c.min) as f64
}

/// Paints control `c` in `r`, in the sheet's control colours. `shown` is the item a drop-down
/// shows closed (ignored for other kinds).
pub fn paint(p: &Painter, r: Rect, c: &FormControl, z: f32, look: Look, shown: &str) {
    let t = Tokens::get(p.ctx());
    let border = Stroke::new(1.0, t.sheet_control_border);
    let bg = |part: Part| {
        if look.pressed == Some(part) {
            t.sheet_control_pressed
        } else if look.hover == Some(part) {
            t.sheet_control_hover
        } else {
            t.sheet_control_bg
        }
    };
    let clip = p.with_clip_rect(r.expand(2.0 * z.max(1.0) + 3.0).intersect(p.clip_rect()));
    match c.kind {
        ControlKind::Button => {
            let fill = bg(Part::Button);
            clip.rect_filled(r, 3.0 * z, fill);
            clip.rect_stroke(r, 3.0 * z, border, StrokeKind::Inside);
            let shift = if look.pressed == Some(Part::Button) { vec2(0.0, 1.0) } else { vec2(0.0, 0.0) };
            let g = clip.layout(c.text.clone(), theme::ui_font(12.0 * z), t.sheet_control_text, (r.width() - 8.0 * z).max(1.0));
            clip.with_clip_rect(r.shrink(1.0).intersect(clip.clip_rect())).galley(r.center() - g.size() / 2.0 + shift, g, t.sheet_control_text);
        }
        ControlKind::Slider => {
            clip.rect_filled(r, 2.0 * z, t.sheet_control_bg);
            clip.rect_stroke(r, 2.0 * z, border, StrokeKind::Inside);
            let (track, thumb) = slider_parts(r, c, z);
            clip.rect_filled(track, 2.0 * z, t.sheet_track);
            let filled = if c.horizontal {
                Rect::from_min_max(track.min, pos2(thumb.center().x, track.max.y))
            } else {
                Rect::from_min_max(track.min, pos2(track.max.x, thumb.center().y))
            };
            clip.rect_filled(filled, 2.0 * z, t.sheet_fill);
            let knob = if look.pressed == Some(Part::Thumb) || look.hover == Some(Part::Thumb) { t.sheet_control_text } else { t.sheet_thumb };
            clip.rect_filled(thumb, 3.0 * z, knob);
            clip.rect_stroke(thumb, 3.0 * z, Stroke::new(1.0, t.sheet_control_bg), StrokeKind::Inside);
            if look.show_value {
                let g = clip.layout_no_wrap(c.value.to_string(), theme::ui_font(11.0 * z), t.sheet_control_text);
                let at = if c.horizontal {
                    pos2(thumb.center().x - g.size().x / 2.0, r.top() - g.size().y - 4.0 * z)
                } else {
                    pos2(r.right() + 4.0 * z, thumb.center().y - g.size().y / 2.0)
                };
                let card = Rect::from_min_size(at, g.size()).expand(2.0 * z);
                p.rect_filled(card, 2.0 * z, t.sheet_control_bg);
                p.rect_stroke(card, 2.0 * z, border, StrokeKind::Inside);
                p.galley(at, g, t.sheet_control_text);
            }
        }
        ControlKind::Spin => {
            let (up, down) = spin_parts(r);
            let tall = r.height() >= r.width();
            for (half, part) in [(up, Part::Up), (down, Part::Down)] {
                clip.rect_filled(half, 2.0 * z, bg(part));
                clip.rect_stroke(half, 2.0 * z, border, StrokeKind::Inside);
                let s = (half.width().min(half.height()) * 0.28).max(1.5);
                let m = half.center();
                let tri = match (tall, part) {
                    (true, Part::Up) => vec![pos2(m.x, m.y - s * 0.6), pos2(m.x - s, m.y + s * 0.5), pos2(m.x + s, m.y + s * 0.5)],
                    (true, _) => vec![pos2(m.x, m.y + s * 0.6), pos2(m.x + s, m.y - s * 0.5), pos2(m.x - s, m.y - s * 0.5)],
                    (false, Part::Up) => vec![pos2(m.x + s * 0.6, m.y), pos2(m.x - s * 0.5, m.y + s), pos2(m.x - s * 0.5, m.y - s)],
                    (false, _) => vec![pos2(m.x - s * 0.6, m.y), pos2(m.x + s * 0.5, m.y - s), pos2(m.x + s * 0.5, m.y + s)],
                };
                clip.add(PathShape::convex_polygon(tri, t.sheet_control_text, Stroke::NONE));
            }
        }
        ControlKind::Drop => {
            let (field, arrow) = drop_parts(r);
            clip.rect_filled(r, 0.0, t.sheet_control_bg);
            clip.rect_stroke(r, 0.0, border, StrokeKind::Inside);
            let g = clip.layout_no_wrap(shown.to_string(), theme::ui_font(11.0 * z), t.sheet_control_text);
            let at = pos2(field.left() + 3.0 * z, field.center().y - g.size().y / 2.0);
            clip.with_clip_rect(field.shrink(1.0).intersect(clip.clip_rect())).galley(at, g, t.sheet_control_text);
            let fill = if look.open { t.sheet_control_pressed } else { bg(Part::Drop) };
            let button = arrow.shrink(1.0);
            clip.rect_filled(button, 0.0, fill);
            clip.rect_stroke(button, 0.0, border, StrokeKind::Inside);
            let s = (button.width().min(button.height()) * 0.22).max(1.5);
            let m = button.center();
            let tri = vec![pos2(m.x, m.y + s * 0.6), pos2(m.x + s, m.y - s * 0.5), pos2(m.x - s, m.y - s * 0.5)];
            clip.add(PathShape::convex_polygon(tri, t.sheet_control_text, Stroke::NONE));
        }
        ControlKind::Check => {
            let side = (r.height() - 4.0 * z).clamp(4.0, 13.0 * z).min(r.width());
            let mark = Rect::from_min_size(pos2(r.left() + 2.0 * z, r.center().y - side / 2.0), vec2(side, side));
            let mixed = c.checked == CheckState::Mixed;
            let fill = if mixed { t.sheet_track } else { bg(Part::Check) };
            clip.rect_filled(mark, 0.0, fill);
            // 3-D shading draws a heavier frame; a flat check box (`noThreeD`) a thin one.
            let frame = if c.no_three_d == Some(true) { border } else { Stroke::new(2.0 * z.max(0.5), t.sheet_control_border) };
            clip.rect_stroke(mark, 0.0, frame, StrokeKind::Inside);
            if c.checked != CheckState::Unchecked {
                let (m, s) = (mark.shrink(side * 0.22), side * 0.12);
                let tick =
                    vec![pos2(m.left(), m.center().y), pos2(m.left() + m.width() * 0.4, m.bottom() - s * 0.5), pos2(m.right(), m.top() + s * 0.5)];
                let ink = if mixed { t.sheet_thumb } else { t.sheet_control_text };
                clip.add(PathShape::line(tick, Stroke::new((1.6 * z).max(1.0), ink)));
            }
            let text_left = mark.right() + 4.0 * z;
            let g = clip.layout_no_wrap(c.text.clone(), theme::ui_font(11.0 * z), t.sheet_control_text);
            let at = pos2(text_left, r.center().y - g.size().y / 2.0);
            let caption = Rect::from_min_max(pos2(text_left, r.top()), r.max);
            clip.with_clip_rect(caption.intersect(clip.clip_rect())).galley(at, g, t.sheet_control_text);
        }
    }
    if look.focused {
        p.rect_stroke(r.expand(2.0), 3.0 * z, Stroke::new(2.0, t.sheet_focus), StrokeKind::Outside);
    }
}

/// The control with `id` on `sh`.
pub fn find(sh: &Sheet, id: u32) -> Option<&FormControl> {
    sh.controls.iter().find(|c| c.id == id)
}

/// How control `c` looks this frame, from the gesture, the focus and the pointer (`hover`, a
/// logical position).
pub fn look(app: &SheetApp, geo: &Geo, c: &FormControl, r: Rect, hover: Option<Pos2>) -> Look {
    let gesture = app.grid.control.as_ref().filter(|g| g.id == c.id);
    let selected = app.selected_object == Some(ObjRef::new(ObjKind::Control, c.id));
    let hover = if selected { None } else { hover.and_then(|p| part_at(r, c, geo.z, inner(geo, r, p))) };
    let pressed = gesture.map(|g| g.part).filter(|part| *part == Part::Thumb || hover == Some(*part));
    Look {
        hover,
        pressed,
        focused: app.grid.focused_control == Some(c.id) && !selected,
        show_value: gesture.is_some_and(|g| g.part == Part::Thumb),
        open: app.grid.drop_open == Some(c.id),
    }
}

impl SheetApp {
    /// Sets a slider or spin button's value; inside a gesture, keeps it one undo step.
    fn control_set(&mut self, id: u32, value: f64) -> bool {
        match self.session.run("control.setValue", json!({"id": id, "value": value})) {
            Ok(_) => {
                if let Some(start) = self.grid.control.as_ref().map(|g| g.undo_start)
                    && let Some(d) = self.session.active_mut()
                    && d.undo.len() > start + 1
                {
                    d.undo.truncate(start + 1);
                }
                true
            }
            Err(e) => {
                self.grid.control = None;
                self.message = Some(("Form Control".into(), crate::clean_error(&e)));
                false
            }
        }
    }

    /// Moves a slider or spin button by `steps` steps (negative: toward the minimum), or by pages
    /// when `page` is set.
    pub fn control_nudge(&mut self, id: u32, steps: i64, page: bool) -> bool {
        let Some(c) = self.session.active().and_then(|d| d.wb.active()).and_then(|sh| find(sh, id)).cloned() else { return false };
        if matches!(c.kind, ControlKind::Button | ControlKind::Check) {
            return false;
        }
        if c.kind == ControlKind::Drop {
            return self.drop_step(&c, steps);
        }
        let by = if page && c.kind == ControlKind::Slider { c.page } else { c.step } as f64;
        self.control_set(id, c.value as f64 + steps as f64 * by)
    }

    /// Moves a drop-down's choice `steps` items down its list (negative: up), stopping at the
    /// first and last items. True when the key was used.
    fn drop_step(&mut self, c: &FormControl, steps: i64) -> bool {
        let Some(len) = self.session.active().map(|d| gridcraft_engine::cmd::controls::drop_len(&d.wb, d.wb.active_sheet(), c)) else { return false };
        if len == 0 {
            return false;
        }
        let to = (c.selected as i64).saturating_add(steps).clamp(1, len as i64);
        to == c.selected as i64 || self.control_set(c.id, to as f64)
    }

    /// Runs a button's action. Questions (approve it, make room for a macro's output) become a
    /// prompt.
    pub fn press_button(&mut self, id: u32) {
        crate::control_ui::press(self, json!({"id": id}));
    }
}

/// Pointer handling for controls on the grid. Returns true when it used the pointer this frame
/// (so the grid neither selects cells nor objects).
pub fn interact(app: &mut SheetApp, ctx: &egui::Context, resp: &egui::Response, geo: &Geo, sh: &Sheet) -> bool {
    let (pointer, mods, time) = ctx.input(|i| (i.pointer.clone(), i.modifiers, i.time));
    let pos = pointer.interact_pos().or(pointer.hover_pos()).map(|p| geo.mirror_pos(p));
    // A control that no longer exists loses its gesture and focus.
    if let Some(id) = app.grid.focused_control
        && find(sh, id).is_none()
    {
        app.grid.focused_control = None;
    }
    if let Some(g) = app.grid.control.clone() {
        let Some(c) = find(sh, g.id).cloned() else {
            app.grid.control = None;
            return true;
        };
        let r = crate::chartview::anchor_rect(geo, sh, &c.anchor);
        if pointer.primary_down() {
            match g.part {
                Part::Thumb => {
                    if let Some(p) = pos {
                        let v = c.snap(value_at(r, &c, geo.z, inner(geo, r, p), g.grab));
                        if v != c.value {
                            app.control_set(c.id, v as f64);
                        }
                    }
                }
                Part::Button | Part::Drop | Part::Check => {}
                part => {
                    // Held arrows and track repeat while the pointer stays on them.
                    let over = pos.and_then(|p| part_at(r, &c, geo.z, inner(geo, r, p))) == Some(part);
                    if over && time >= g.next_repeat {
                        step_part(app, &c, part);
                        if let Some(gg) = app.grid.control.as_mut() {
                            gg.next_repeat = time + REPEAT_EVERY;
                        }
                    }
                    ctx.request_repaint_after(std::time::Duration::from_millis(30));
                }
            }
            return true;
        }
        // Released: a button runs when the pointer is still on it, and a drop-down opens its list
        // (or closes it). Toggling on the release, as the in-cell list's arrow does, keeps the
        // release from counting as a click outside the list that has just opened.
        app.grid.control = None;
        if pos.is_some_and(|p| r.contains(p)) {
            match g.part {
                Part::Button => app.press_button(c.id),
                Part::Drop => toggle_drop(app, c.id),
                Part::Check => {
                    app.control_set(c.id, c.checked.toggled().number() as f64);
                }
                _ => {}
            }
        }
        resp.request_focus();
        return true;
    }
    if !pointer.primary_pressed() || !resp.contains_pointer() {
        // Any other press, on the grid (a right-click on a cell) or off it, moves the focus off
        // the control, so a later Enter or Space doesn't work it.
        if pointer.any_pressed() && app.grid.focused_control.is_some() {
            app.grid.focused_control = None;
        }
        return false;
    }
    let batched = !pointer.primary_down();
    // egui clears press_origin on release, even when both events arrived in this frame.
    let click = batched
        .then(|| {
            ctx.input(|i| {
                let mut press = None;
                let mut click = None;
                for event in &i.events {
                    if let egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, .. } = event {
                        if *pressed {
                            press = Some(*pos);
                        } else if let Some(start) = press.take() {
                            click = Some((geo.mirror_pos(start), geo.mirror_pos(*pos)));
                        }
                    }
                }
                click
            })
        })
        .flatten();
    let Some(p) = click.map(|(start, _)| start).or_else(|| pointer.press_origin().map(|p| geo.mirror_pos(p))).or(pos) else { return false };
    // The selected object's handles reach past its frame and win over a control beneath them.
    if let Some(r) = app.selected_object.and_then(|o| crate::chartview::object_rect(geo, sh, o))
        && crate::objects::handle_at(r, p).is_some()
    {
        return false;
    }
    let hit = crate::chartview::hit(app, geo, sh, p).filter(|(o, _)| o.kind == ObjKind::Control);
    // Ctrl or Cmd+click selects; a selected control moves and resizes instead of working.
    let Some((o, r)) = hit.filter(|(o, _)| !mods.command && !mods.ctrl && app.selected_object != Some(*o)) else {
        app.grid.focused_control = None;
        return false;
    };
    let Some(c) = find(sh, o.id).cloned() else { return false };
    let p = inner(geo, r, p);
    let Some(part) = part_at(r, &c, geo.z, p) else { return false };
    app.selected_object = None;
    app.grid.focused_control = Some(c.id);
    let undo_start = app.session.active().map_or(0, |d| d.undo.len());
    let grab = if part == Part::Thumb {
        let (_, thumb) = slider_parts(r, &c, geo.z);
        if c.horizontal { p.x - thumb.center().x } else { p.y - thumb.center().y }
    } else {
        0.0
    };
    if part == Part::Drop && batched {
        if click.is_some_and(|(_, end)| r.contains(end)) {
            toggle_drop(app, c.id);
        }
        resp.request_focus();
        return true;
    }
    app.grid.control = Some(Gesture { id: c.id, part, grab, undo_start, next_repeat: time + REPEAT_DELAY });
    if !matches!(part, Part::Button | Part::Thumb | Part::Drop | Part::Check) {
        step_part(app, &c, part);
    }
    resp.request_focus();
    true
}

fn toggle_drop(app: &mut SheetApp, id: u32) {
    app.grid.drop_open = if app.grid.drop_open == Some(id) { None } else { Some(id) };
}

/// One click on an arrow or the track.
fn step_part(app: &mut SheetApp, c: &FormControl, part: Part) {
    match part {
        Part::Up => app.control_nudge(c.id, 1, false),
        Part::Down => app.control_nudge(c.id, -1, false),
        Part::TrackBefore => app.control_nudge(c.id, -1, true),
        Part::TrackAfter => app.control_nudge(c.id, 1, true),
        Part::Button | Part::Thumb | Part::Drop | Part::Check => false,
    };
}

/// Whether the focused control is a button or a check box. Space presses a button and checks or
/// clears a check box, so the space it types must not start a cell edit; a slider or spin button
/// doesn't take Space, and its space reaches the grid like any other character.
pub fn focused_button(app: &SheetApp, sh: &Sheet) -> bool {
    app.grid.focused_control.and_then(|id| find(sh, id)).is_some_and(|c| matches!(c.kind, ControlKind::Button | ControlKind::Check))
}

/// The grid takes a key while a control is focused: a button or check box gives the keyboard
/// back, so a later Enter or Space doesn't work it again and Space starts a cell edit; a slider or
/// spin button keeps it, as the control last worked, which the arrows still step.
pub fn key_to_grid(app: &mut SheetApp, sh: &Sheet) {
    if focused_button(app, sh) {
        app.grid.focused_control = None;
    }
}

/// Keys for the focused control: arrows move by a step, Page Up and Page Down by a page, Home and
/// End to the ends; Space or Enter presses a button, and Space checks or clears a check box. A
/// drop-down's arrows choose the item above or below, Page Up and Page Down move by its rows, and
/// Enter opens or closes its list. True when the key was used. `repeat` is a key held down: it
/// steps a slider or spin button again, but a button runs (and a check box toggles) once per
/// press.
pub fn key(app: &mut SheetApp, sh: &Sheet, key: egui::Key, repeat: bool) -> bool {
    use egui::Key;
    let Some(c) = app.grid.focused_control.and_then(|id| find(sh, id)).cloned() else { return false };
    if app.selected_object.is_some() {
        return false;
    }
    match (c.kind, key) {
        (_, Key::Escape) => {
            if app.grid.drop_open.take().is_none() {
                app.grid.focused_control = None;
            }
            true
        }
        (ControlKind::Button, Key::Space | Key::Enter) => {
            if !repeat {
                app.press_button(c.id);
            }
            true
        }
        (ControlKind::Button, _) => false,
        (ControlKind::Check, Key::Space) => {
            if !repeat {
                app.control_set(c.id, c.checked.toggled().number() as f64);
            }
            true
        }
        (ControlKind::Check, _) => false,
        (ControlKind::Drop, Key::Enter) => {
            if !repeat {
                app.grid.drop_open = if app.grid.drop_open == Some(c.id) { None } else { Some(c.id) };
            }
            true
        }
        (ControlKind::Drop, Key::ArrowDown | Key::ArrowRight) => app.drop_step(&c, 1),
        (ControlKind::Drop, Key::ArrowUp | Key::ArrowLeft) => app.drop_step(&c, -1),
        (ControlKind::Drop, Key::PageDown) => app.drop_step(&c, c.drop_lines.max(1) as i64),
        (ControlKind::Drop, Key::PageUp) => app.drop_step(&c, -(c.drop_lines.max(1) as i64)),
        (ControlKind::Drop, Key::Home) => app.drop_step(&c, i64::MIN),
        (ControlKind::Drop, Key::End) => app.drop_step(&c, i64::MAX),
        (ControlKind::Drop, _) => false,
        // A spin button's up arrow adds; a slider's minimum is at its left or top.
        (ControlKind::Spin, Key::ArrowUp | Key::ArrowRight) | (ControlKind::Slider, Key::ArrowDown | Key::ArrowRight) => {
            app.control_nudge(c.id, 1, false)
        }
        (ControlKind::Spin, Key::ArrowDown | Key::ArrowLeft) | (ControlKind::Slider, Key::ArrowUp | Key::ArrowLeft) => {
            app.control_nudge(c.id, -1, false)
        }
        (ControlKind::Spin, Key::PageUp) | (ControlKind::Slider, Key::PageDown) => app.control_nudge(c.id, 1, true),
        (ControlKind::Spin, Key::PageDown) | (ControlKind::Slider, Key::PageUp) => app.control_nudge(c.id, -1, true),
        (_, Key::Home) => app.control_set(c.id, c.min as f64),
        (_, Key::End) => app.control_set(c.id, c.max as f64),
        _ => false,
    }
}

/// The height of one row of a drop-down's open list (screen points).
const DROP_ROW_H: f32 = 18.0;

/// The open list of the drop-down `app.grid.drop_open` names, below the control (above it when
/// it would run off the window), showing its `drop_lines` rows and scrolling through the rest;
/// only the rows in view are read from the source range. A click chooses an item through
/// `control.setValue` (one undo step, which writes the linked cell) and closes the list, as do
/// Escape and a click elsewhere. `was_open`: the list was open last frame too.
pub fn drop_picker(app: &mut SheetApp, ui: &mut egui::Ui, grid: &egui::Response, geo: &Geo, was_open: bool) {
    use gridcraft_engine::cmd::controls::{drop_items, drop_len};
    let Some(id) = app.grid.drop_open else { return };
    let Some(d) = app.session.active() else { return };
    let host = d.wb.active_sheet();
    let Some(c) = d.wb.sheet(host).and_then(|sh| find(sh, id)).filter(|c| c.kind == ControlKind::Drop).cloned() else {
        app.grid.drop_open = None;
        return;
    };
    let Some(sh) = d.wb.sheet(host) else { return };
    let r = geo.mirror_rect(crate::chartview::anchor_rect(geo, sh, &c.anchor));
    let len = drop_len(&d.wb, host, &c) as usize;
    let gap = ui.spacing().item_spacing.y;
    let rows = len.clamp(1, c.drop_lines.max(1) as usize).min(64);
    let height = rows as f32 * DROP_ROW_H + (rows as f32 - 1.0) * gap;
    let screen = ui.ctx().content_rect();
    let above = r.bottom() + height + 8.0 > screen.bottom() && r.top() - height - 8.0 >= screen.top();
    let (at, pivot) = if above { (r.left_top(), egui::Align2::LEFT_BOTTOM) } else { (r.left_bottom(), egui::Align2::LEFT_TOP) };
    let width = r.width().max(60.0);
    let wb = &d.wb;
    let mut chosen = None;
    let response =
        egui::Area::new(egui::Id::unique(("drop_list", id))).fixed_pos(at).pivot(pivot).order(egui::Order::Foreground).show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).inner_margin(2.0).show(ui, |ui| {
                ui.set_min_width(width);
                let mut area = egui::ScrollArea::vertical().max_height(height).min_scrolled_height(height).auto_shrink([false, true]);
                // On opening, the chosen item is in view.
                if !was_open && c.selected > 0 {
                    area = area.vertical_scroll_offset((c.selected - 1) as f32 * (DROP_ROW_H + gap));
                }
                area.show_rows(ui, DROP_ROW_H, len, |ui, range| {
                    let start = u32::try_from(range.start).unwrap_or(u32::MAX);
                    let count = u32::try_from(range.len()).unwrap_or(0);
                    for (i, text) in (start..).zip(drop_items(wb, host, &c, start, count)) {
                        let item = i.saturating_add(1);
                        if ui.add(egui::Button::selectable(item == c.selected, text).min_size(vec2(width, DROP_ROW_H))).clicked() {
                            chosen = Some(item);
                        }
                    }
                });
            });
        });
    if let Some(item) = chosen {
        app.grid.drop_open = None;
        app.control_set(id, item as f64);
        app.grid.focused_control = Some(id);
        grid.request_focus();
    } else if crate::grid::popup_dismissed(&response.response, was_open) {
        app.grid.drop_open = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slider(value: u32, horizontal: bool) -> FormControl {
        FormControl { kind: ControlKind::Slider, min: 0, max: 100, value, horizontal, ..Default::default() }
    }

    #[test]
    fn the_thumb_sits_at_the_value_and_dragging_reads_it_back() {
        let r = Rect::from_min_size(pos2(10.0, 10.0), vec2(200.0, 20.0));
        for v in [0, 25, 50, 100] {
            let c = slider(v, true);
            let (_, thumb) = slider_parts(r, &c, 1.0);
            assert!((value_at(r, &c, 1.0, thumb.center(), 0.0) - v as f64).abs() < 0.5, "{v}");
            assert_eq!(part_at(r, &c, 1.0, thumb.center()), Some(Part::Thumb));
        }
        let c = slider(50, true);
        assert_eq!(part_at(r, &c, 1.0, pos2(15.0, 20.0)), Some(Part::TrackBefore));
        assert_eq!(part_at(r, &c, 1.0, pos2(205.0, 20.0)), Some(Part::TrackAfter));
        assert_eq!(part_at(r, &c, 1.0, pos2(5.0, 20.0)), None);
        // Past the ends clamps.
        assert_eq!(value_at(r, &c, 1.0, pos2(-500.0, 20.0), 0.0), 0.0);
        assert_eq!(value_at(r, &c, 1.0, pos2(900.0, 20.0), 0.0), 100.0);
        // Vertical: the minimum is at the top.
        let r = Rect::from_min_size(pos2(0.0, 0.0), vec2(20.0, 200.0));
        let c = slider(0, false);
        let (_, thumb) = slider_parts(r, &c, 1.0);
        assert!(thumb.center().y < 20.0);
        assert_eq!(part_at(r, &slider(50, false), 1.0, pos2(10.0, 190.0)), Some(Part::TrackAfter));
    }

    #[test]
    fn spin_halves() {
        let tall = Rect::from_min_size(pos2(0.0, 0.0), vec2(18.0, 36.0));
        let c = FormControl { kind: ControlKind::Spin, ..Default::default() };
        assert_eq!(part_at(tall, &c, 1.0, pos2(9.0, 5.0)), Some(Part::Up));
        assert_eq!(part_at(tall, &c, 1.0, pos2(9.0, 30.0)), Some(Part::Down));
        let wide = Rect::from_min_size(pos2(0.0, 0.0), vec2(40.0, 18.0));
        assert_eq!(part_at(wide, &c, 1.0, pos2(35.0, 9.0)), Some(Part::Up));
        assert_eq!(part_at(wide, &c, 1.0, pos2(5.0, 9.0)), Some(Part::Down));
    }

    #[test]
    fn degenerate_ranges_do_not_divide_by_zero() {
        let r = Rect::from_min_size(pos2(0.0, 0.0), vec2(1.0, 1.0));
        let c = FormControl { kind: ControlKind::Slider, min: 5, max: 5, value: 5, ..Default::default() };
        let (track, thumb) = slider_parts(r, &c, 1.0);
        assert!(track.width().is_finite() && thumb.center().x.is_finite());
        assert!(value_at(r, &c, 1.0, pos2(0.5, 0.5), 0.0).is_finite());
    }
}
