//! Axis-aligned DrawingML layout (ECMA-376 Part 1, §20.1.7.5 and §20.1.9 `xfrm`).

use crate::ObjectRect;

/// The stored frame of a drawing in its parent's child space, in EMUs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ChildTransform {
    pub off: (i64, i64),
    pub ext: (i64, i64),
    pub rotation: i64,
    pub flip: (bool, bool),
}

impl ChildTransform {
    pub fn from_rect(r: ObjectRect, rotation: i64, flip: (bool, bool)) -> Self {
        Self { off: (whole(r.x), whole(r.y)), ext: (whole(r.w), whole(r.h)), rotation, flip }
    }

    pub fn rect(self) -> ObjectRect {
        ObjectRect { x: self.off.0 as f64, y: self.off.1 as f64, w: self.ext.0.max(0) as f64, h: self.ext.1.max(0) as f64 }
    }

    /// The established leaf layout: keep the box centered and swap its axes for angles in
    /// the quarter-turn ranges. A leaf's own flips change its content, not its box.
    pub fn leaf_box(self) -> ObjectRect {
        swap_about_center(self.rect(), leaf_swaps_axes(self.rotation))
    }

    pub fn from_leaf_box(r: ObjectRect, rotation: i64, flip: (bool, bool)) -> Self {
        Self::from_rect(swap_about_center(r, leaf_swaps_axes(rotation)), rotation, flip)
    }
}

fn leaf_swaps_axes(rotation: i64) -> bool {
    let rot = rotation.rem_euclid(21_600_000);
    (2_700_000..8_100_000).contains(&rot) || (13_500_000..18_900_000).contains(&rot)
}

fn whole(v: f64) -> i64 {
    v.round().clamp(-1e15, 1e15) as i64
}

/// Group rotation is applied to the geometry only at exact multiples of 90°. The stored
/// rotation still goes back to the file for every angle.
pub fn group_quarter_turn(rotation: i64) -> Option<usize> {
    let rot = rotation.rem_euclid(21_600_000);
    (rot % 5_400_000 == 0).then_some((rot / 5_400_000) as usize)
}

/// A box swapped about its centre for an exact 90° or 270° group turn, else unchanged. It is
/// its own inverse: it takes an outermost group's anchor to the box its members fill, and a
/// nested group's placed (turned) box back to its frame.
pub fn group_unturned_box(r: ObjectRect, rotation: i64) -> ObjectRect {
    swap_about_center(r, group_quarter_turn(rotation).is_some_and(|q| q % 2 == 1))
}

fn swap_about_center(r: ObjectRect, swap: bool) -> ObjectRect {
    if swap { ObjectRect { x: r.x + (r.w - r.h) / 2.0, y: r.y + (r.h - r.w) / 2.0, w: r.h, h: r.w } } else { r }
}

#[derive(Clone, Copy, Debug)]
struct Axis {
    input: usize,
    scale: f64,
    input_center: f64,
    output_center: f64,
}

/// An affine signed axis permutation. Mapping boxes (not just origins) makes flips,
/// quarter turns and their inverses exact, even with different scales on the two axes.
#[derive(Clone, Copy, Debug)]
pub struct GroupProjection([Axis; 2]);

impl GroupProjection {
    /// Map a group's child coordinates into its enclosing space: scale to the unturned
    /// frame, then flip and turn clockwise around the frame's center. `fallback` is pixels
    /// per EMU for a root group, or 1 for a nested frame already in the parent's EMU space.
    pub fn new(frame: ObjectRect, child: ObjectRect, (flip, rotation): ((bool, bool), i64), fallback: f64) -> Self {
        let inputs = [(child.x, child.w, frame.w, flip.0), (child.y, child.h, frame.h, flip.1)];
        let center = [frame.x + frame.w / 2.0, frame.y + frame.h / 2.0];
        let (order, backwards) = match group_quarter_turn(rotation) {
            Some(1) => ([1, 0], [true, false]),
            Some(2) => ([0, 1], [true, true]),
            Some(3) => ([1, 0], [false, true]),
            _ => ([0, 1], [false, false]),
        };
        Self(std::array::from_fn(|out| {
            let input = order[out];
            let (start, extent, len, flip) = inputs[input];
            let scale = if extent > 0.0 { len / extent } else { fallback };
            Axis { input, scale: if flip ^ backwards[out] { -scale } else { scale }, input_center: start + extent / 2.0, output_center: center[out] }
        }))
    }

    /// Compose a map from child space with this map from its parent space.
    pub fn compose(self, child: Self) -> Self {
        Self(self.0.map(|outer| {
            let inner = child.0[outer.input];
            Axis {
                input: inner.input,
                scale: outer.scale * inner.scale,
                input_center: inner.input_center,
                output_center: outer.output_center + (inner.output_center - outer.input_center) * outer.scale,
            }
        }))
    }

    pub fn project(self, rect: ObjectRect) -> ObjectRect {
        let src = [(rect.x, rect.w), (rect.y, rect.h)];
        let [(x, w), (y, h)] = self.0.map(|axis| {
            let (start, len) = src[axis.input];
            let length = len * axis.scale.abs();
            (axis.output_center + (start - axis.input_center + len / 2.0) * axis.scale - length / 2.0, length)
        });
        ObjectRect { x, y, w, h }
    }

    /// The inverse is undefined on a collapsed axis. Use the child center on that axis;
    /// callers with live file coordinates should retain them instead of inferring them.
    pub fn unproject(self, rect: ObjectRect) -> ObjectRect {
        let mut axes = self.0;
        for (output, axis) in self.0.into_iter().enumerate() {
            axes[axis.input] = Axis {
                input: output,
                scale: if axis.scale == 0.0 { 0.0 } else { axis.scale.recip() },
                input_center: axis.output_center,
                output_center: axis.input_center,
            };
        }
        Self(axes).project(rect)
    }
}

/// Effective child frame for missing or unusable DrawingML fields. In particular an explicit
/// positive `chExt` survives a collapsed layout axis. Missing extents fall back to `ext`,
/// then to the enclosing coordinate space at its native scale.
pub fn group_child_box(frame: ObjectRect, off: (i64, i64), ch_ext: (i64, i64), ext: (i64, i64), fallback: f64) -> ObjectRect {
    let extent = |child, own, len| {
        if child > 0 {
            child as f64
        } else if own > 0 {
            own as f64
        } else {
            len / fallback
        }
    };
    ObjectRect { x: off.0 as f64, y: off.1 as f64, w: extent(ch_ext.0, ext.0, frame.w), h: extent(ch_ext.1, ext.1, frame.h) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_exact_quarter_turns_turn_a_group_whatever_the_stored_angle() {
        for (rot, turn) in [
            (0, Some(0)),
            (5_400_000, Some(1)),
            (-16_200_000, Some(1)),
            (27_000_000, Some(1)),
            (-10_800_000, Some(2)),
            (16_200_000, Some(3)),
            (-5_400_000, Some(3)),
            (21_600_000, Some(0)),
            (-21_600_000, Some(0)),
            (5_399_999, None),
            (5_400_001, None),
            (-5_400_001, None),
            (2_700_000, None),
            (i64::MIN, None),
        ] {
            assert_eq!(group_quarter_turn(rot), turn, "rot {rot}");
        }
    }

    #[test]
    fn nested_flipped_quarter_turns_invert_on_both_axes() {
        // Outer: flipped vertically and turned -90° (that is 270°), child box 100x100 on a
        // 120x80 frame. Inner: flipped horizontally and turned 90°, child box 100x100 on 20x30.
        let outer = GroupProjection::new(
            ObjectRect { x: 40.0, y: 30.0, w: 120.0, h: 80.0 },
            ObjectRect { x: 100.0, y: 200.0, w: 100.0, h: 100.0 },
            ((false, true), -5_400_000),
            1.0 / 9525.0,
        );
        let inner = GroupProjection::new(
            ObjectRect { x: 110.0, y: 220.0, w: 20.0, h: 30.0 },
            ObjectRect { x: 30.0, y: 40.0, w: 100.0, h: 100.0 },
            ((true, false), 5_400_000),
            1.0,
        );
        let map = outer.compose(inner);
        let leaf = ObjectRect { x: 40.0, y: 60.0, w: 20.0, h: 30.0 };
        let placed = map.project(leaf);
        // Inner: centre offset (-30, -15) scales to (-6, -4.5), flips to (6, -4.5) and turns to
        // (4.5, 6) about (120, 235); the 4x9 box turns to 9x4. Outer: offset (-25.5, -9) scales
        // to (-30.6, -7.2), flips to (-30.6, 7.2) and turns to (7.2, 30.6) about (100, 70); the
        // 10.8x3.2 box turns to 3.2x10.8.
        let expected = ObjectRect { x: 105.6, y: 95.2, w: 3.2, h: 10.8 };
        let back = map.unproject(placed);
        for (a, b) in [(placed, expected), (back, leaf)] {
            assert!([a.x - b.x, a.y - b.y, a.w - b.w, a.h - b.h].iter().all(|d| d.abs() < 1e-9), "{a:?} vs {b:?}");
        }
    }
}
