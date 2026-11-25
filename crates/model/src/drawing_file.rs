use gridcraft_core::{MAX_COLS, MAX_ROWS};

use crate::{Anchor, AnchorMode, DrawingRef, Sheet};

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

impl DrawingFile {
    /// Whether the placement stays put when earlier rows or columns change: an absolute
    /// anchor, or a cell-marker spelling of "Don't move or size with cells".
    pub fn is_fixed(&self) -> bool {
        self.anchor.mode == AnchorMode::Absolute || matches!(self.placement, DrawingPlacement::Absolute { .. })
    }
}

impl Sheet {
    /// The rows (`rows`) or columns that `file`, the stored placement of `r`, and the editable
    /// geometry it was read as cover, a group's members included, measured on this sheet's
    /// current lines. `None` when the placement is stale or its group can't be reasoned about
    /// line by line (an edited transform, members with different anchor modes).
    pub fn drawing_file_span(&self, r: DrawingRef, file: &DrawingFile, rows: bool) -> Option<(u32, u32)> {
        let a = self.raw_drawing_anchor(r).filter(|a| *a == file.anchor)?;
        let lines = |a: &Anchor| {
            let (start, end) = if rows {
                (a.cell.row, self.row_at(self.row_top(a.cell.row) + a.dy as f64 + a.height.max(0.0) as f64))
            } else {
                (a.cell.col, self.col_at(self.col_left(a.cell.col) + a.dx as f64 + a.width.max(0.0) as f64))
            };
            (start.min(end), start.max(end))
        };
        let mut span = lines(&a);
        if let DrawingRef::Group(id) = r {
            self.source_group(self.drawing_group(id)?)?;
            for member in self.group_contents(id).into_iter().filter_map(|m| self.raw_drawing_anchor(m)) {
                if member.mode != a.mode {
                    return None;
                }
                let (start, end) = lines(&member);
                span = (span.0.min(start), span.1.max(end));
            }
        }
        Some(file.placement.physical_span(rows, span, |line, offset| {
            let pixels = offset as f64 / 9525.0;
            if rows {
                self.row_at(self.row_top(line.min(MAX_ROWS - 1)) + pixels)
            } else {
                self.col_at(self.col_left(line.min(MAX_COLS - 1)) + pixels)
            }
        }))
    }
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
