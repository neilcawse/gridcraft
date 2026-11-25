use crate::{Anchor, AnchorMode};

/// A DrawingML cell marker. Offsets are signed EMUs and need not fit inside the marked cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DrawingMarker {
    pub col: u32,
    pub row: u32,
    pub col_off: i64,
    pub row_off: i64,
}

/// Original DrawingML placement, independent of the sheet's pixel geometry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DrawingPlacement {
    TwoCell { from: DrawingMarker, to: DrawingMarker, edit_as: Option<String> },
    OneCell { from: DrawingMarker, ext: (i64, i64) },
    Absolute { pos: (i64, i64), ext: (i64, i64) },
}

/// The original placement and the editable anchor it represented on import.
#[derive(Clone, Debug, PartialEq)]
pub struct DrawingFile {
    pub anchor: Anchor,
    pub placement: DrawingPlacement,
}

impl DrawingPlacement {
    /// Include the physical position of both cell markers, even when their signed offsets
    /// point outside their own cells. `line_at` uses the pre-edit sheet geometry.
    pub fn physical_span(&self, rows: bool, span: (u32, u32), mut line_at: impl FnMut(u32, i64) -> u32) -> (u32, u32) {
        let (from, to) = match self {
            Self::Absolute { .. } => return span,
            Self::OneCell { from, .. } => (from, None),
            Self::TwoCell { from, to, .. } => (from, Some(to)),
        };
        let mut span = (span.0.min(span.1), span.0.max(span.1));
        for m in std::iter::once(from).chain(to) {
            let line = if rows { line_at(m.row, m.row_off) } else { line_at(m.col, m.col_off) };
            span = (span.0.min(line), span.1.max(line));
        }
        span
    }

    /// Retain markers only outside the affected span. For a two-cell spelling of an absolute
    /// anchor, an edit before the span also invalidates its cell-relative encoding.
    pub fn shift_lines(&mut self, rows: bool, at: u32, count: u32, insert: bool, mode: AnchorMode, span: (u32, u32)) -> bool {
        let (from, to) = match self {
            Self::Absolute { .. } => return true,
            Self::OneCell { from, .. } => (from, None),
            Self::TwoCell { from, to, .. } => (from, Some(to)),
        };
        let axis = |m: &DrawingMarker| if rows { m.row } else { m.col };
        let near = span.0.min(axis(from)).min(to.as_ref().map_or(u32::MAX, |m| axis(m)));
        let far = span.1.max(axis(from)).max(to.as_ref().map_or(0, |m| axis(m)));
        if at > far {
            return true;
        }
        if mode == AnchorMode::Absolute || if insert { at > near } else { at.saturating_add(count) > near } {
            return false;
        }
        let shift = |m: &mut DrawingMarker| {
            let index = if rows { &mut m.row } else { &mut m.col };
            let limit = if rows { gridcraft_core::MAX_ROWS } else { gridcraft_core::MAX_COLS };
            let next = if insert { index.checked_add(count) } else { index.checked_sub(count) };
            if let Some(next) = next.filter(|&n| n < limit) {
                *index = next;
                true
            } else {
                false
            }
        };
        shift(from) && to.is_none_or(shift)
    }
}
