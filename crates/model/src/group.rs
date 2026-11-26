//! Grouped drawing objects (Excel's Group, `<xdr:grpSp>` in a file). A group lists its members
//! back to front; the members stay ordinary charts, pictures and shapes in their sheet's lists,
//! each with its own resolved anchor, so they draw exactly as ungrouped objects do. Moving,
//! resizing or deleting a member acts on its outermost group.

use gridcraft_core::CellRef;
use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::{Anchor, AnchorMode, Sheet};

/// Groups nest at most this deep; deeper references are ignored (and a cycle in hand-written
/// JSON can't recurse forever).
pub const MAX_GROUP_DEPTH: usize = 64;

/// A floating object by kind and id: what a group holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DrawingRef {
    Chart(u32),
    Image(u32),
    Shape(u32),
    Group(u32),
    Slicer(u32),
}
/// The original DrawingML transform and member anchors, retained for a lossless write when the
/// group has not been edited. This is transient import metadata, not part of the model's JSON.
#[derive(Clone, Debug, PartialEq)]
pub struct GroupFileMember {
    pub reference: DrawingRef,
    pub anchor: Anchor,
    /// A missing offset or extent means the member fills the enclosing frame.
    pub transform: Option<crate::ChildTransform>,
}

/// The transform a group was read with, for an unedited group to be written back exactly.
#[derive(Clone, Debug, PartialEq)]
pub struct GroupFile {
    pub anchor: Anchor,
    pub off: (i64, i64),
    pub ext: (i64, i64),
    pub child_offset: (i64, i64),
    pub child_extent: (i64, i64),
    /// Effective child space used by the editor, which can differ from the literal EMUs.
    pub model_child_offset: (i64, i64),
    pub model_child_extent: (i64, i64),
    /// Presence of off, ext, chOff and chExt respectively; absent fields use layout defaults.
    pub fields: [bool; 4],
    pub has_transform: bool,
    /// An explicit edit invalidates reuse even if the anchor later returns to the same value.
    pub edited: bool,
    /// Named line pairs (columns, rows) for a coincident two-cell span held in sheet space
    /// by a sort. This is layout-only: the original worksheet markers must not be exported.
    pub held_collapse: [Option<(u32, u32)>; 2],
    /// A row sort moved rows out of the held span. The editable anchor's inferred far
    /// edge must not resolve to a different coincident row and grow the rejected span.
    pub sort_lost_collapsed_rows: bool,
    /// Every retained leaf had its own offset and extent.
    pub complete: bool,
    pub flip: (bool, bool),
    pub rotation: i64,
    pub member_rotations: Vec<(DrawingRef, i64)>,
    pub members: Vec<GroupFileMember>,
}

/// A group of floating objects.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DrawingGroup {
    pub id: u32,
    /// The group's name (`cNvPr name`, e.g. "Group 3").
    pub name: String,
    /// The group's placed box on the sheet when it was laid out, moved or sized as a whole.
    /// At 90° or 270° its frame is this box swapped about its centre in the enclosing space:
    /// sheet space for a root group or the parent's child coordinates for a nested group.
    /// A rotated group is saved using its own box, not the current bounds of its members
    /// ([`Sheet::drawing_rect`]), since those bounds needn't share its centre.
    pub anchor: Anchor,
    /// The child box origin (`a:chOff`, EMUs), in the group's unturned frame.
    pub child_offset: (i64, i64),
    pub child_extent: (i64, i64),
    /// The group's flips (`a:xfrm flipH`/`flipV`). Members are stored where they appear on the
    /// sheet, mirrored about the group's box on these axes (their own content isn't mirrored).
    #[serde(default, skip_serializing_if = "is_default")]
    pub flip: (bool, bool),
    /// The group's rotation (`a:xfrm rot`, 60000ths of a degree), kept for saving. Exact
    /// quarter turns also change member boxes, but not their rendered content.
    #[serde(default, skip_serializing_if = "is_default")]
    pub rotation: i64,
    /// The rotations (`a:xfrm rot`) of the charts, pictures and shapes in the group that have
    /// one, kept for saving. They are drawn unrotated: in their own box, or, turned about a
    /// quarter (45°–135° or 225°–315°), in that box swapped about its centre, as Excel measures
    /// them when it sizes a group.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub member_rotations: Vec<(DrawingRef, i64)>,
    /// The source transform and member coordinates, when imported from DrawingML. This is
    /// transient and is discarded by JSON serialization.
    #[serde(skip)]
    pub file: Option<GroupFile>,
    /// Members, back to front (nested groups by [`DrawingRef::Group`]).
    pub members: Vec<DrawingRef>,
}

impl DrawingGroup {
    /// Whether the group's rotation turns it at all.
    pub fn rotated(&self) -> bool {
        rotates(self.rotation)
    }
}

/// Whether a rotation (`a:xfrm rot`, 60000ths of a degree) turns an object at all.
pub fn rotates(rot: i64) -> bool {
    rot % 21_600_000 != 0
}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

/// A floating object's box on the sheet, in points from the sheet's top-left corner.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ObjectRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl ObjectRect {
    fn union(self, o: ObjectRect) -> ObjectRect {
        let (x, y) = (self.x.min(o.x), self.y.min(o.y));
        ObjectRect { x, y, w: (self.x + self.w).max(o.x + o.w) - x, h: (self.y + self.h).max(o.y + o.h) - y }
    }
}

/// The axis-aligned map taking `from` onto `to` (an axis without extent keeps its scale).
pub(crate) fn map_rect(from: ObjectRect, to: ObjectRect, r: ObjectRect) -> ObjectRect {
    let sx = if from.w > 0.0 { to.w / from.w } else { 1.0 };
    let sy = if from.h > 0.0 { to.h / from.h } else { 1.0 };
    ObjectRect { x: to.x + (r.x - from.x) * sx, y: to.y + (r.y - from.y) * sy, w: r.w * sx, h: r.h * sy }
}

impl Sheet {
    /// An anchor's box on the sheet.
    pub fn anchor_rect(&self, a: &Anchor) -> ObjectRect {
        ObjectRect { x: self.col_left(a.cell.col) + a.dx as f64, y: self.row_top(a.cell.row) + a.dy as f64, w: a.width as f64, h: a.height as f64 }
    }

    /// The anchor (top-left cell plus offset, clamped onto the sheet) for a box, with `mode`.
    pub fn rect_anchor(&self, r: ObjectRect, mode: AnchorMode) -> Anchor {
        let px = |v: f64| if v.is_finite() { v.clamp(0.0, 1e7) } else { 0.0 };
        self.layout_rect_anchor(ObjectRect { x: px(r.x), y: px(r.y), w: px(r.w), h: px(r.h) }, mode)
    }

    /// Re-anchor internal layout geometry without the input cap for object commands.
    pub(crate) fn layout_rect_anchor(&self, r: ObjectRect, mode: AnchorMode) -> Anchor {
        let (x, y) = (r.x.max(0.0), r.y.max(0.0));
        let col = self.col_at(x);
        let row = self.row_at(y);
        Anchor {
            cell: CellRef::new(row, col),
            dx: (x - self.col_left(col)).max(0.0) as f32,
            dy: (y - self.row_top(row)).max(0.0) as f32,
            width: r.w.max(0.0) as f32,
            height: r.h.max(0.0) as f32,
            mode,
        }
    }

    pub fn drawing_group(&self, id: u32) -> Option<&DrawingGroup> {
        self.groups.iter().find(|g| g.id == id)
    }

    /// The anchor of a chart, picture or shape (`None` for a group or a missing object).
    pub fn drawing_anchor(&self, r: DrawingRef) -> Option<Anchor> {
        match r {
            DrawingRef::Chart(id) => self.charts.iter().find(|o| o.id == id).map(|o| o.anchor),
            DrawingRef::Image(id) => self.images.iter().find(|o| o.id == id).map(|o| o.anchor),
            DrawingRef::Shape(id) => self.shapes.iter().find(|o| o.id == id).map(|o| o.anchor),
            DrawingRef::Slicer(id) => self.slicers.iter().find(|o| o.id == id).map(|o| o.anchor),
            DrawingRef::Group(_) => None,
        }
    }

    /// The stored anchor, including a group's own box rather than its members' union.
    pub fn raw_drawing_anchor(&self, r: DrawingRef) -> Option<Anchor> {
        match r {
            DrawingRef::Group(id) => self.drawing_group(id).map(|g| g.anchor),
            _ => self.drawing_anchor(r),
        }
    }

    /// The file transform is reusable only if all dependent objects are unchanged.
    pub fn source_group<'a>(&self, g: &'a DrawingGroup) -> Option<&'a GroupFile> {
        self.source_group_at_depth(g, 0, true)
    }

    /// Whether the file transforms still lay the group out: nothing in it moved or was edited.
    /// Unlike [`Sheet::source_group`] this allows members skipped on open or read without a
    /// transform, which can't be written back from the file but still place the rest.
    pub(crate) fn source_layout_group(&self, g: &DrawingGroup) -> bool {
        self.source_group_at_depth(g, 0, false).is_some()
    }

    fn source_group_at_depth<'a>(&self, g: &'a DrawingGroup, depth: usize, complete: bool) -> Option<&'a GroupFile> {
        let f = g.file.as_ref()?;
        if depth >= MAX_GROUP_DEPTH
            || f.edited
            || (complete && (!f.complete || f.held_collapse.iter().any(Option::is_some)))
            || f.anchor != g.anchor
            || f.model_child_offset != g.child_offset
            || f.model_child_extent != g.child_extent
            || f.flip != g.flip
            || f.rotation != g.rotation
            || f.member_rotations != g.member_rotations
            || f.members.len() != g.members.len()
        {
            return None;
        }
        f.members
            .iter()
            .zip(&g.members)
            .all(|(m, &r)| {
                m.reference == r
                    && self.raw_drawing_anchor(r) == Some(m.anchor)
                    && match r {
                        DrawingRef::Group(id) => self.drawing_group(id).and_then(|g| self.source_group_at_depth(g, depth + 1, complete)).is_some(),
                        _ => true,
                    }
            })
            .then_some(f)
    }

    /// An object's box; a group's is the box around its members. `None` when the object is
    /// missing or a group holds none.
    pub fn drawing_rect(&self, r: DrawingRef) -> Option<ObjectRect> {
        self.drawing_rect_with(r, &|o| self.drawing_anchor(o).map(|a| self.anchor_rect(&a)))
    }

    /// [`Sheet::drawing_rect`] with the boxes of charts, pictures and shapes from `leaf` (a
    /// writer skipping some objects measures groups by the ones it keeps). Each nested group is
    /// visited once, so a group listing another (or itself) twice costs no more.
    pub fn drawing_rect_with(&self, r: DrawingRef, leaf: &dyn Fn(DrawingRef) -> Option<ObjectRect>) -> Option<ObjectRect> {
        let DrawingRef::Group(id) = r else { return leaf(r) };
        self.group_contents(id).into_iter().filter(|m| !matches!(m, DrawingRef::Group(_))).filter_map(leaf).reduce(ObjectRect::union)
    }

    /// A group's anchor: the box around its members, in the group's anchor mode.
    pub fn group_anchor(&self, id: u32) -> Option<Anchor> {
        let g = self.drawing_group(id)?;
        self.drawing_rect(DrawingRef::Group(id)).map(|r| self.rect_anchor(r, g.anchor.mode))
    }

    /// The group directly holding `r`.
    pub fn parent_group(&self, r: DrawingRef) -> Option<u32> {
        self.groups.iter().find(|g| g.members.contains(&r)).map(|g| g.id)
    }

    /// The outermost group holding `r` (`None` when `r` isn't grouped).
    pub fn outer_group(&self, r: DrawingRef) -> Option<u32> {
        let mut cur = self.parent_group(r)?;
        for _ in 0..MAX_GROUP_DEPTH {
            match self.parent_group(DrawingRef::Group(cur)) {
                Some(p) if p != cur => cur = p,
                _ => break,
            }
        }
        Some(cur)
    }

    /// Discard placement for `r` and any enclosing groups.
    pub fn invalidate_drawing_file(&mut self, mut r: DrawingRef) {
        for _ in 0..=MAX_GROUP_DEPTH {
            self.drawing_files.remove(&r);
            if let DrawingRef::Group(id) = r
                && let Some(file) = self.groups.iter_mut().find(|g| g.id == id).and_then(|g| g.file.as_mut())
            {
                file.edited = true;
            }
            let Some(parent) = self.parent_group(r) else { break };
            r = DrawingRef::Group(parent);
        }
    }

    /// A command can place an object on a changed grid without changing its editable anchor.
    /// Keep its file placement only if the markers still describe that anchor on this grid.
    pub fn discard_stale_drawing_placement(&mut self, o: crate::ObjRef, anchor: Anchor) {
        if let Some(r) = crate::sheet::drawing_reference(o)
            && let Some(file) = self.drawing_files.get(&r)
            && (file.anchor != anchor || file.placement.anchor(self) != anchor)
        {
            self.invalidate_drawing_file(r);
        }
    }

    /// Forget file placement after a bulk operation changes the anchors directly.
    pub fn discard_changed_drawing_files(&mut self) {
        let mut files = std::mem::take(&mut self.drawing_files);
        files.retain(|&r, f| self.raw_drawing_anchor(r) == Some(f.anchor));
        self.drawing_files = files;
        let changed: Vec<_> =
            self.groups.iter().filter(|g| g.file.as_ref().is_some_and(|f| !f.edited) && !self.source_layout_group(g)).map(|g| g.id).collect();
        for id in changed {
            self.invalidate_drawing_file(DrawingRef::Group(id));
        }
    }

    /// Everything inside group `id`, nested groups included (not `id` itself).
    pub fn group_contents(&self, id: u32) -> Vec<DrawingRef> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let mut stack = vec![(id, 0usize)];
        while let Some((gid, depth)) = stack.pop() {
            let Some(g) = self.drawing_group(gid) else { continue };
            for m in &g.members {
                if !seen.insert(*m) {
                    continue;
                }
                out.push(*m);
                if let DrawingRef::Group(n) = m
                    && depth + 1 < MAX_GROUP_DEPTH
                {
                    stack.push((*n, depth + 1));
                }
            }
        }
        out
    }

    /// Moves and sizes group `id` with everything in it by the map taking `from` onto `to`. A
    /// map that would put any of it left of column A or above row 1 is shifted so the group
    /// stops at the sheet's edge, as each object's anchor can't start off the sheet: moved there
    /// one by one, they would pile up on the edge. "Any of it" is its charts, pictures and
    /// shapes and the own boxes of rotated groups, which are saved; an unrotated group's own box
    /// only fixes its child space, and is wider than its members once one was erased or dropped
    /// on open.
    pub fn transform_group(&mut self, id: u32, from: ObjectRect, mut to: ObjectRect) {
        let mut groups = vec![id];
        let mut leaves = Vec::new();
        for r in self.group_contents(id) {
            match r {
                DrawingRef::Group(g) if !groups.contains(&g) => groups.push(g),
                DrawingRef::Group(_) => {}
                leaf => {
                    if let Some(a) = self.drawing_anchor(leaf) {
                        leaves.push((leaf, a));
                    }
                }
            }
        }
        let saved_boxes = groups.iter().filter_map(|&g| self.drawing_group(g).filter(|g| g.rotated()).map(|g| g.anchor));
        let boxes = saved_boxes.chain(leaves.iter().map(|&(_, a)| a));
        if let Some(all) = boxes.map(|a| map_rect(from, to, self.anchor_rect(&a))).reduce(ObjectRect::union) {
            to.x -= all.x.min(0.0);
            to.y -= all.y.min(0.0);
        }
        for (leaf, a) in leaves {
            self.drawing_files.remove(&leaf);
            let na = self.rect_anchor(map_rect(from, to, self.anchor_rect(&a)), a.mode);
            if let Some(slot) = self.drawing_anchor_mut(leaf) {
                *slot = na;
            }
        }
        for gid in groups {
            self.invalidate_drawing_file(DrawingRef::Group(gid));
            let Some(a) = self.drawing_group(gid).map(|g| g.anchor) else { continue };
            let na = self.rect_anchor(map_rect(from, to, self.anchor_rect(&a)), a.mode);
            if let Some(g) = self.groups.iter_mut().find(|g| g.id == gid) {
                g.anchor = na;
            }
        }
    }

    /// Sets the anchor mode of group `id`, everything in it and the group itself.
    pub fn set_group_anchor_mode(&mut self, id: u32, mode: AnchorMode) {
        let contents = self.group_contents(id);
        for r in &contents {
            self.drawing_files.remove(r);
            if let Some(a) = self.drawing_anchor_mut(*r) {
                a.mode = mode;
            }
        }
        self.invalidate_drawing_file(DrawingRef::Group(id));
        for g in self.groups.iter_mut().filter(|g| g.id == id || contents.contains(&DrawingRef::Group(g.id))) {
            self.drawing_files.remove(&DrawingRef::Group(g.id));
            if let Some(file) = &mut g.file {
                file.edited = true;
            }
            g.anchor.mode = mode;
        }
    }

    /// Deletes group `id` with everything in it.
    pub fn delete_group(&mut self, id: u32) {
        let contents = self.group_contents(id);
        for r in std::iter::once(&DrawingRef::Group(id)).chain(&contents) {
            self.drawing_files.remove(r);
            self.drawing_order.remove(r);
            self.drawing_names.remove(r);
        }
        let has = |r: DrawingRef| contents.contains(&r);
        self.charts.retain(|o| !has(DrawingRef::Chart(o.id)));
        self.images.retain(|o| !has(DrawingRef::Image(o.id)));
        self.shapes.retain(|o| !has(DrawingRef::Shape(o.id)));
        self.remove_from_group(DrawingRef::Group(id));
        self.groups.retain(|g| g.id != id && !has(DrawingRef::Group(g.id)));
    }

    /// Takes `r` out of the groups holding it, leaving the rest of each group grouped (Excel
    /// deleting one member of a group). A group left empty is taken out of its own group and
    /// dropped in turn.
    pub fn remove_from_group(&mut self, r: DrawingRef) {
        self.invalidate_drawing_file(r);
        let mut pending = vec![r];
        while let Some(cur) = pending.pop() {
            for g in &mut self.groups {
                if g.members.contains(&cur) {
                    g.members.retain(|m| *m != cur);
                    g.member_rotations.retain(|(m, _)| *m != cur);
                    if g.members.is_empty() {
                        pending.push(DrawingRef::Group(g.id));
                    }
                }
            }
            if let DrawingRef::Group(id) = cur
                && cur != r
            {
                self.drawing_order.remove(&cur);
                self.drawing_names.remove(&cur);
                self.groups.retain(|g| g.id != id);
            }
        }
    }

    pub(crate) fn drawing_anchor_mut(&mut self, r: DrawingRef) -> Option<&mut Anchor> {
        match r {
            DrawingRef::Chart(id) => self.charts.iter_mut().find(|o| o.id == id).map(|o| &mut o.anchor),
            DrawingRef::Image(id) => self.images.iter_mut().find(|o| o.id == id).map(|o| &mut o.anchor),
            DrawingRef::Shape(id) => self.shapes.iter_mut().find(|o| o.id == id).map(|o| &mut o.anchor),
            DrawingRef::Slicer(id) => self.slicers.iter_mut().find(|o| o.id == id).map(|o| &mut o.anchor),
            DrawingRef::Group(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Color, Shape, ShapeFormat, ShapeKind};

    fn shape(id: u32, x: f64, y: f64, sh: &Sheet) -> Shape {
        let anchor = sh.rect_anchor(ObjectRect { x, y, w: 20.0, h: 10.0 }, AnchorMode::MoveOnly);
        Shape {
            id,
            kind: ShapeKind::Rectangle,
            anchor,
            fill: Color::Auto,
            line: Color::Auto,
            text: String::new(),
            format: ShapeFormat::default(),
            rotation: 0.0,
            math: crate::EquationCache::default(),
        }
    }

    fn sheet_with_nested_group() -> Sheet {
        let mut sh = Sheet::new("S");
        let a = shape(1, 100.0, 50.0, &sh);
        let b = shape(2, 200.0, 90.0, &sh);
        let c = shape(3, 150.0, 70.0, &sh);
        sh.shapes = vec![a, b, c];
        let anchor = sh.rect_anchor(ObjectRect { x: 150.0, y: 70.0, w: 70.0, h: 30.0 }, AnchorMode::MoveOnly);
        sh.groups.push(DrawingGroup {
            id: 5,
            name: "Inner".into(),
            anchor,
            child_offset: (0, 0),
            child_extent: (1, 1),
            flip: (false, false),
            rotation: 0,
            member_rotations: Vec::new(),
            members: vec![DrawingRef::Shape(3), DrawingRef::Shape(2)],
            file: None,
        });
        let anchor = sh.rect_anchor(ObjectRect { x: 100.0, y: 50.0, w: 120.0, h: 50.0 }, AnchorMode::MoveOnly);
        sh.groups.push(DrawingGroup {
            id: 4,
            name: "Outer".into(),
            anchor,
            child_offset: (0, 0),
            child_extent: (1, 1),
            flip: (false, false),
            rotation: 0,
            member_rotations: Vec::new(),
            members: vec![DrawingRef::Shape(1), DrawingRef::Group(5)],
            file: None,
        });
        sh
    }

    #[test]
    fn groups_measure_move_and_delete_with_their_members() {
        let mut sh = sheet_with_nested_group();
        assert_eq!(sh.outer_group(DrawingRef::Shape(2)), Some(4));
        assert_eq!(sh.outer_group(DrawingRef::Group(5)), Some(4));
        assert_eq!(sh.outer_group(DrawingRef::Shape(9)), None);
        let r = sh.drawing_rect(DrawingRef::Group(4)).unwrap_or_default();
        assert_eq!((r.x, r.y, r.w, r.h), (100.0, 50.0, 120.0, 50.0));
        // Moving the group by (+30, +5) moves every member and both groups' boxes.
        sh.transform_group(4, r, ObjectRect { x: 130.0, y: 55.0, ..r });
        let moved = |sh: &Sheet, id| sh.drawing_rect(DrawingRef::Shape(id)).map(|r| (r.x, r.y));
        assert_eq!((moved(&sh, 1), moved(&sh, 2), moved(&sh, 3)), (Some((130.0, 55.0)), Some((230.0, 95.0)), Some((180.0, 75.0))));
        let inner = sh.drawing_group(5).map(|g| sh.anchor_rect(&g.anchor)).unwrap_or_default();
        assert_eq!((inner.x, inner.y), (180.0, 75.0));
        assert_eq!(sh.shapes.iter().map(|s| s.anchor.mode).collect::<Vec<_>>(), vec![AnchorMode::MoveOnly; 3]);
        sh.delete_group(4);
        assert!(sh.shapes.is_empty() && sh.groups.is_empty());
    }

    #[test]
    fn sort_group_overlap_requires_a_nonempty_block() {
        let sh = sheet_with_nested_group();
        for area in [ObjectRect { x: 160.0, y: 60.0, w: 0.0, h: 30.0 }, ObjectRect { x: 150.0, y: 80.0, w: 30.0, h: 0.0 }] {
            let grouped = sh.grouped_objects(area);
            assert_eq!(grouped.len(), 5);
            assert!(grouped.values().all(|&held| !held), "collapsed sort block {area:?} has no overlap");
        }
    }

    #[test]
    fn sort_group_overlap_uses_outer_box_and_half_open_edges() {
        let mut sh = sheet_with_nested_group();
        sh.groups[1].anchor = sh.rect_anchor(ObjectRect { x: 80.0, y: 40.0, w: 140.0, h: 60.0 }, AnchorMode::MoveOnly);
        let held = |sh: &Sheet, x| {
            let area = ObjectRect { x, y: 40.0, w: 10.0, h: 10.0 };
            let grouped = sh.grouped_objects(area);
            assert_eq!(grouped.len(), 5);
            assert!(grouped.values().all(|&hold| hold == grouped[&crate::ObjRef::new(crate::ObjKind::Group, 4)]));
            grouped[&crate::ObjRef::new(crate::ObjKind::Group, 4)]
        };
        assert!(!held(&sh, 70.0)); // block ends exactly at the stored box's left edge
        assert!(held(&sh, 75.0)); // stored box, not member bounds, overlaps
        assert!(!held(&sh, 220.0)); // block starts exactly at the stored box's right edge
        sh.groups[1].anchor = sh.rect_anchor(ObjectRect { x: 60.0, y: 40.0, w: 20.0, h: 10.0 }, AnchorMode::MoveOnly);
        assert!(!held(&sh, 85.0)); // the gap between stored and member boxes doesn't overlap
        sh.groups[0].members.push(DrawingRef::Group(4));
        assert!(!held(&sh, 75.0)); // a cycle without an outermost group does not hold
        sh.groups.remove(0);
        sh.groups[0].members.clear();
        sh.groups[0].anchor = sh.rect_anchor(ObjectRect { x: 80.0, y: 40.0, w: 0.0, h: 0.0 }, AnchorMode::MoveOnly);
        let group = crate::ObjRef::new(crate::ObjKind::Group, 4);
        assert!(sh.grouped_objects(ObjectRect { x: 80.0, y: 40.0, w: 10.0, h: 10.0 })[&group]);
        assert!(!sh.grouped_objects(ObjectRect { x: 70.0, y: 40.0, w: 10.0, h: 10.0 })[&group]);
    }

    #[test]
    fn a_group_cycle_does_not_recurse_forever() {
        let mut sh = sheet_with_nested_group();
        if let Some(g) = sh.groups.iter_mut().find(|g| g.id == 5) {
            g.members.push(DrawingRef::Group(4));
        }
        assert!(sh.outer_group(DrawingRef::Shape(1)).is_some());
        assert!(sh.drawing_rect(DrawingRef::Group(4)).is_some());
        assert_eq!(sh.group_contents(4).len(), 5);
        sh.delete_group(4);
        assert!(sh.shapes.is_empty() && sh.groups.is_empty());
    }

    #[test]
    fn a_group_listing_itself_twice_is_measured_once() {
        // Branching on every repeat would measure the shape about 2^64 times and never return.
        let mut sh = sheet_with_nested_group();
        if let Some(g) = sh.groups.iter_mut().find(|g| g.id == 5) {
            g.members = vec![DrawingRef::Shape(3), DrawingRef::Group(5), DrawingRef::Group(5)];
        }
        let calls = std::cell::Cell::new(0);
        let leaf = |r| {
            calls.set(calls.get() + 1);
            assert!(calls.get() < 100, "the group's members were measured over and over");
            sh.drawing_anchor(r).map(|a| sh.anchor_rect(&a))
        };
        let r = sh.drawing_rect_with(DrawingRef::Group(4), &leaf).unwrap_or_default();
        assert_eq!((r.x, r.y, r.w, r.h), (100.0, 50.0, 70.0, 30.0));
        assert_eq!(sh.group_anchor(5).map(|a| sh.anchor_rect(&a).w), Some(20.0));
    }

    #[test]
    fn only_a_rotated_groups_own_box_stops_it_at_the_sheet_edge() {
        // The inner group's stored box reaches 80 px left of its members (one was erased, or
        // a SmartArt member dropped on open): moved left by 120, the members still reach x 0.
        let mut sh = sheet_with_nested_group();
        let stale = sh.rect_anchor(ObjectRect { x: 70.0, y: 70.0, w: 150.0, h: 30.0 }, AnchorMode::MoveOnly);
        if let Some(g) = sh.groups.iter_mut().find(|g| g.id == 5) {
            g.anchor = stale;
        }
        let r = sh.drawing_rect(DrawingRef::Group(4)).unwrap_or_default();
        sh.transform_group(4, r, ObjectRect { x: r.x - 120.0, ..r });
        assert_eq!(sh.drawing_rect(DrawingRef::Group(4)).map(|r| r.x), Some(0.0));
        // A rotated group is saved with its own box, so that box stops it at the edge.
        let mut sh = sheet_with_nested_group();
        if let Some(g) = sh.groups.iter_mut().find(|g| g.id == 5) {
            (g.anchor, g.rotation) = (stale, 5_400_000);
        }
        sh.transform_group(4, r, ObjectRect { x: r.x - 120.0, ..r });
        assert_eq!(sh.drawing_rect(DrawingRef::Group(4)).map(|r| r.x), Some(30.0));
        assert_eq!(sh.drawing_group(5).map(|g| sh.anchor_rect(&g.anchor).x), Some(0.0));
    }
}
