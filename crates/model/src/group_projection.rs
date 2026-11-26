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
