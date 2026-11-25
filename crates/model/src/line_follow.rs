//! Floating objects when row heights and column widths change without a structural shift.
//!
//! The model keeps an object's top-left cell and offset, and its size in pixels. A two-cell
//! object's far corner is therefore not stored as a cell: when lines under it change size, the
//! object has to be re-measured to keep following its cells the way Excel's `editAs` says
//! (ECMA-376 Part 1, 20.5.3.2 `ST_EditAs`): "move and size with cells" keeps both corners on
//! their cells, "move but don't size" keeps its size and follows its top-left cell, and "don't
//! move or size" keeps its place on the sheet.

use std::collections::{BTreeMap, HashSet};

use gridcraft_core::{MAX_COLS, MAX_ROWS};

use crate::group::map_rect;
use crate::sheet::drawing_reference;
use crate::{Anchor, AnchorMode, DrawingPlacement, DrawingRef, LineInfo, ObjKind, ObjRef, Sheet};

/// Pixels per EMU, for the half-EMU a stored marker can't resolve.
const PX_PER_EMU: f64 = 1.0 / 9525.0;
/// Positions this close (pixels) are the same place: the same line sizes summed in another order.
const SAME: f64 = 1e-6;

/// A top-level floating object: one that isn't inside a group.
#[derive(Clone, Copy)]
enum Floating {
    Drawing(DrawingRef),
    Control(u32),
}

impl Floating {
    fn obj_ref(self) -> ObjRef {
        match self {
            Floating::Drawing(r) => drawing_obj(r),
            Floating::Control(id) => ObjRef::new(ObjKind::Control, id),
        }
    }
}

fn drawing_obj(r: DrawingRef) -> ObjRef {
    match r {
        DrawingRef::Chart(id) => ObjRef::new(ObjKind::Chart, id),
        DrawingRef::Image(id) => ObjRef::new(ObjKind::Image, id),
        DrawingRef::Shape(id) => ObjRef::new(ObjKind::Shape, id),
        DrawingRef::Group(id) => ObjRef::new(ObjKind::Group, id),
        DrawingRef::Slicer(id) => ObjRef::new(ObjKind::Slicer, id),
    }
}

/// One axis of a sheet's grid.
#[derive(Clone, Copy)]
struct Axis<'a> {
    sheet: &'a Sheet,
    rows: bool,
}

impl Axis<'_> {
    fn start(self, line: u32) -> f64 {
        if self.rows { self.sheet.row_top(line) } else { self.sheet.col_left(line) }
    }
    fn at(self, pos: f64) -> u32 {
        if self.rows { self.sheet.row_at(pos) } else { self.sheet.col_at(pos) }
    }
    fn size(self, line: u32) -> f64 {
        f64::from(if self.rows { self.sheet.row_height(line) } else { self.sheet.col_width(line) })
    }
    fn limit(self) -> u32 {
        if self.rows { MAX_ROWS } else { MAX_COLS }
    }
}

/// The extent of an object starting `offset` into `line` and `extent` long, once its far edge
/// keeps its place in its own line while the lines change from `old` to `new`. A far edge on a
/// line boundary stays on that boundary.
fn resized(old: Axis<'_>, new: Axis<'_>, line: u32, offset: f32, extent: f32) -> f64 {
    let (offset, extent) = (f64::from(offset), f64::from(extent.max(0.0)));
    let end = old.start(line) + offset + extent;
    let mut far = old.at(end);
    let mut into = end - old.start(far);
    let tolerance = PX_PER_EMU / 2.0 + (offset.abs() + extent) * f64::from(f32::EPSILON);
    let size = old.size(far);
    if size > 0.0 && far + 1 < old.limit() && (into - size).abs() <= tolerance {
        far += 1;
        into = 0.0;
    }
    (new.start(far) + into - (new.start(line) + offset)).max(0.0)
}

/// Whether the place `offset` into `line` moved on the sheet: the line starts elsewhere, or it
/// became too short to hold the offset.
fn displaced(old: Axis<'_>, new: Axis<'_>, line: u32, offset: f32) -> bool {
    let offset = f64::from(offset);
    (new.start(line) - old.start(line)).abs() > SAME || (offset > 0.0 && offset >= new.size(line) && offset < old.size(line))
}

/// `a` re-measured for lines changed from `old` to `new` by its mode; `None` when it stays.
fn follow(old: &Sheet, new: &Sheet, a: &Anchor) -> Option<Anchor> {
    let axes = |rows| (Axis { sheet: old, rows }, Axis { sheet: new, rows });
    let ((old_x, new_x), (old_y, new_y)) = (axes(false), axes(true));
    match a.mode {
        AnchorMode::MoveOnly => None,
        AnchorMode::MoveAndSize => {
            let width = resized(old_x, new_x, a.cell.col, a.dx, a.width);
            let height = resized(old_y, new_y, a.cell.row, a.dy, a.height);
            let keep = |new: f64, old: f32| if (new - f64::from(old.max(0.0))).abs() <= SAME { old } else { new as f32 };
            let next = Anchor { width: keep(width, a.width), height: keep(height, a.height), ..*a };
            (next != *a).then_some(next)
        }
        AnchorMode::Absolute => (displaced(old_x, new_x, a.cell.col, a.dx) || displaced(old_y, new_y, a.cell.row, a.dy))
            .then(|| new.rect_anchor(old.anchor_rect(a), AnchorMode::Absolute)),
    }
}

/// `lines` with the visibility of each line `before` had: a line hidden or shown counts at the
/// size it had before.
fn keep_visibility(lines: &mut BTreeMap<u32, LineInfo>, before: &BTreeMap<u32, LineInfo>) {
    let hidden = |m: &BTreeMap<u32, LineInfo>, k: u32| m.get(&k).is_some_and(|i| i.hidden);
    let toggled: Vec<u32> = before
        .iter()
        .chain(lines.iter())
        .filter(|(_, i)| i.hidden)
        .map(|(&k, _)| k)
        .filter(|&k| hidden(before, k) != hidden(lines, k))
        .collect();
    for k in toggled {
        match before.get(&k) {
            Some(info) => lines.insert(k, *info),
            None => lines.remove(&k),
        };
    }
}

/// Where an object read with `placement` sits once the open has fitted its rows: its anchor over
/// the final lines. For a two-cell anchor that is both markers whatever its `editAs`, which
/// ECMA-376 `ST_EditAs` (20.5.3.2) ties to rows resized after it is placed. Whether Excel for Mac
/// counts its own open-time row fit as such a resize for `editAs="oneCell"` (keep the read size)
/// or `"absolute"` (keep the read box) is unmeasured; this is the one place to change if a
/// measurement says it does (returning the read anchor there, whose stored `to` marker then no
/// longer matches, so its placement must be dropped rather than kept current).
fn opened_anchor(placement: &DrawingPlacement, sheet: &Sheet) -> Anchor {
    placement.anchor(sheet)
}

impl Sheet {
    /// A sheet holding only this one's row heights, column widths and line visibility, to
    /// measure anchors against once the lines have changed.
    pub fn line_geometry(&self) -> Sheet {
        let mut lines = Sheet::new(String::new());
        lines.geometry = self.geometry;
        lines.default_row_height = self.default_row_height;
        lines.default_col_width = self.default_col_width;
        lines.rows = self.rows.clone();
        lines.cols = self.cols.clone();
        lines
    }

    /// Whether any row height, column width or line visibility differs from `other`'s.
    pub fn lines_differ(&self, other: &Sheet) -> bool {
        self.default_row_height != other.default_row_height
            || self.default_col_width != other.default_col_width
            || self.rows != other.rows
            || self.cols != other.cols
    }

    /// Floating objects outside any group, then each one's anchor.
    fn floating(&self) -> Vec<(Floating, Anchor)> {
        let grouped: HashSet<DrawingRef> = self.groups.iter().flat_map(|g| g.members.iter().copied()).collect();
        let drawings = (self.charts.iter().map(|o| (DrawingRef::Chart(o.id), o.anchor)))
            .chain(self.images.iter().map(|o| (DrawingRef::Image(o.id), o.anchor)))
            .chain(self.shapes.iter().map(|o| (DrawingRef::Shape(o.id), o.anchor)))
            .chain(self.slicers.iter().map(|o| (DrawingRef::Slicer(o.id), o.anchor)))
            .chain(self.groups.iter().map(|g| (DrawingRef::Group(g.id), g.anchor)))
            .filter(|(r, _)| !grouped.contains(r))
            .map(|(r, a)| (Floating::Drawing(r), a));
        drawings.chain(self.controls.iter().map(|c| (Floating::Control(c.id), c.anchor))).collect()
    }

    /// Moves everything in group `id` from where `from` (its old anchor, measured on `old`)
    /// put it to `to`, its new anchor on this sheet. The anchors only: stored file placements
    /// are left for the caller to keep or drop.
    fn carry_group(&mut self, id: u32, old: &Sheet, from: &Anchor, to: Anchor) {
        let (from_rect, to_rect) = (old.anchor_rect(from), self.anchor_rect(&to));
        for m in self.group_contents(id) {
            let Some(a) = self.raw_drawing_anchor(m) else { continue };
            let next = self.rect_anchor(map_rect(from_rect, to_rect, old.anchor_rect(&a)), a.mode);
            match m {
                DrawingRef::Group(gid) => {
                    if let Some(g) = self.groups.iter_mut().find(|g| g.id == gid) {
                        g.anchor = next;
                    }
                }
                leaf => {
                    if let Some(slot) = self.drawing_anchor_mut(leaf) {
                        *slot = next;
                    }
                }
            }
        }
        if let Some(g) = self.groups.iter_mut().find(|g| g.id == id) {
            g.anchor = to;
        }
    }

    /// Re-places floating objects after row heights or column widths changed from `before`'s
    /// (any sheet holding the old lines), each by its anchor mode: "move and size with cells"
    /// keeps both corners on their cells, "move but don't size" keeps its size, "don't move or
    /// size" keeps its place. A group follows its own mode and carries its members along.
    /// Lines hidden or shown are left out (an object keeps the size it had over them), as only
    /// the size, not the cell, of a collapsed line is known afterwards.
    ///
    /// `settled` names objects to leave alone, with their current anchor: ones the change
    /// itself already placed. A group is left alone when it or anything in it is settled.
    pub fn follow_line_sizes(&mut self, before: &Sheet, settled: &dyn Fn(ObjRef, &Anchor) -> bool) {
        if !self.lines_differ(before) {
            return;
        }
        let mut new = self.line_geometry();
        keep_visibility(&mut new.rows, &before.rows);
        keep_visibility(&mut new.cols, &before.cols);
        for (object, a) in self.floating() {
            if settled(object.obj_ref(), &a) {
                continue;
            }
            match object {
                Floating::Drawing(DrawingRef::Group(id)) => {
                    let contents = self.group_contents(id);
                    if contents.iter().any(|&m| self.raw_drawing_anchor(m).is_some_and(|ma| settled(drawing_obj(m), &ma))) {
                        continue;
                    }
                    if let Some(next) = follow(before, &new, &a) {
                        self.carry_group(id, before, &a, next);
                    }
                }
                Floating::Drawing(r) => {
                    if let (Some(next), Some(slot)) = (follow(before, &new, &a), self.drawing_anchor_mut(r)) {
                        *slot = next;
                    }
                }
                Floating::Control(id) => {
                    if let (Some(next), Some(c)) = (follow(before, &new, &a), self.controls.iter_mut().find(|c| c.id == id)) {
                        c.anchor = next;
                    }
                }
            }
        }
    }

    /// Places floating objects over the lines an open settled on, after sizing it does once the
    /// file is read (automatic row heights fitted on open). `before` holds the lines as read.
    ///
    /// An object read with a stored placement is placed by its anchor over the final lines, as
    /// the file describes it (ECMA-376 Part 1, 20.5.2.33 `twoCellAnchor`, 20.5.2.24
    /// `oneCellAnchor`, 20.5.2.1 `absoluteAnchor`), and its stored placement stays current so an
    /// unedited object saves its markers unchanged. A group's members go with it. Objects read
    /// without one (a chart's own drawing, form controls) follow the lines by their mode.
    pub fn place_after_open_sizing(&mut self, before: &Sheet) {
        if !self.lines_differ(before) {
            return;
        }
        let placed: Vec<(DrawingRef, Anchor)> = self
            .floating()
            .into_iter()
            .filter_map(|(object, a)| match object {
                Floating::Drawing(r) => self.drawing_files.get(&r).filter(|f| f.anchor == a).map(|f| (r, opened_anchor(&f.placement, self))),
                Floating::Control(_) => None,
            })
            .collect();
        let files: HashSet<DrawingRef> = placed.iter().map(|(r, _)| *r).collect();
        for (r, next) in placed {
            let Some(a) = self.raw_drawing_anchor(r) else { continue };
            if next != a {
                match r {
                    DrawingRef::Group(id) => {
                        self.carry_group(id, before, &a, next);
                        self.refresh_group_files(id);
                    }
                    leaf => {
                        if let Some(slot) = self.drawing_anchor_mut(leaf) {
                            *slot = next;
                        }
                    }
                }
            }
            if let Some(file) = self.drawing_files.get_mut(&r) {
                file.anchor = next;
            }
        }
        self.follow_line_sizes(before, &|o, _| drawing_reference(o).is_some_and(|r| files.contains(&r)));
    }

    /// Records group `id`'s current anchors, and those of the groups inside it, as their file
    /// transforms' anchors: the transforms still describe the groups after they moved with
    /// their placement.
    fn refresh_group_files(&mut self, id: u32) {
        let nested: Vec<u32> = std::iter::once(id)
            .chain(self.group_contents(id).into_iter().filter_map(|m| if let DrawingRef::Group(g) = m { Some(g) } else { None }))
            .collect();
        for gid in nested {
            let Some(i) = self.groups.iter().position(|g| g.id == gid) else { continue };
            let Some(mut file) = self.groups[i].file.take() else { continue };
            file.anchor = self.groups[i].anchor;
            for member in &mut file.members {
                if let Some(anchor) = self.raw_drawing_anchor(member.reference) {
                    member.anchor = anchor;
                }
            }
            self.groups[i].file = Some(file);
        }
    }
}
