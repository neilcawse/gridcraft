//! Reference adjustment: copying formulas, inserting/deleting rows and columns, moving cells,
//! renaming and deleting sheets.

use gridcraft_core::{CellError, CellRef, MAX_COLS, MAX_ROWS, RangeRef};

use crate::ast::{Anchor, Expr, RefKind, Reference, SheetSel, clamp_anchor};

/// Copy/fill: relative parts move by `(dr, dc)`. References pushed off the sheet become `#REF!`.
pub fn shift_relative(e: Expr, dr: i64, dc: i64) -> Expr {
    e.map(&mut |x| match x {
        Expr::Ref(r) => match shift_ref(r, dr, dc) {
            Some(r) => Expr::Ref(r),
            None => Expr::Error(CellError::Ref),
        },
        other => other,
    })
}

/// Rows of the Excel 97-2003 (BIFF8) grid, on which Excel keeps an `.xls` workbook in
/// Compatibility Mode. [MS-XLS] RgceLocRel adjusts a relative row by 0x10000 when it leaves it.
pub const BIFF8_ROWS: u32 = 65_536;
/// Columns of the BIFF8 grid (A:IV); a relative column is adjusted by 0x100.
pub const BIFF8_COLS: u32 = 256;

/// Defined names: relative parts move by `(dr, dc)`, wrapping around the sheet's edges. Files
/// store a name's relative references as seen from A1 (a name for "the cell to the left" defined
/// in B1 is `Sheet1!XFD1`), and a use in another cell moves them by that cell's offset from A1.
pub fn shift_relative_wrapping(e: Expr, dr: i64, dc: i64) -> Expr {
    shift_relative_wrapping_on(e, dr, dc, (MAX_ROWS, MAX_COLS))
}

/// [`shift_relative_wrapping`] on a `(rows, cols)` grid: a relative part lands at the caller's
/// row (column) plus its residue, modulo the grid's extent, even from a caller past that grid.
/// `$` parts and sheet qualifiers are left as they are. With [`BIFF8_ROWS`] × [`BIFF8_COLS`] this
/// is how a legacy `.xls` name evaluates (`A65531` from F21 is F15).
pub fn shift_relative_wrapping_on(e: Expr, dr: i64, dc: i64, (rows, cols): (u32, u32)) -> Expr {
    let wrap = |v: u32, abs: bool, d: i64, max: u32| if abs { v } else { (v as i64 + d).rem_euclid(max as i64) as u32 };
    let anchor = |a: Anchor| Anchor { row: wrap(a.row, a.row_abs, dr, rows), col: wrap(a.col, a.col_abs, dc, cols), ..a };
    e.map(&mut |x| match x {
        Expr::Ref(r) => {
            let kind = match r.kind {
                RefKind::Cell(a) => RefKind::Cell(anchor(a)),
                RefKind::Range(a, b) => RefKind::Range(anchor(a), anchor(b)),
                RefKind::Rows(r0, a0, r1, a1) => RefKind::Rows(wrap(r0, a0, dr, rows), a0, wrap(r1, a1, dr, rows), a1),
                RefKind::Cols(c0, a0, c1, a1) => RefKind::Cols(wrap(c0, a0, dc, cols), a0, wrap(c1, a1, dc, cols), a1),
            };
            Expr::Ref(Reference { sheet: r.sheet, kind })
        }
        other => other,
    })
}

/// The signed offset a BIFF8 residue `k` (`0..extent`) is saved as for the big grid, until Excel's
/// own Save As rule is measured: the shortest one, negative on a tie (`k >= extent / 2` is
/// `k − extent`). [MS-XLS] makes `k` and `k − extent` the same legacy offset, so this is a choice,
/// not a decoding.
pub fn legacy_signed_offset(k: u32, extent: u32) -> i64 {
    let k = i64::from(k % extent);
    if k >= i64::from(extent / 2) { k - i64::from(extent) } else { k }
}

/// A legacy-grid name's formula for an XLSX file: every relative residue is re-expressed for
/// the big grid through [`legacy_signed_offset`] (`A65531` → `A1048571`, `IV1` → `XFD1`,
/// `B1` stays). `$` parts are left alone.
pub fn legacy_to_xlsx_residues(e: Expr) -> Expr {
    let line = |v: u32, abs: bool, legacy: u32, max: u32| if abs { v } else { legacy_signed_offset(v, legacy).rem_euclid(i64::from(max)) as u32 };
    let anchor = |a: Anchor| Anchor { row: line(a.row, a.row_abs, BIFF8_ROWS, MAX_ROWS), col: line(a.col, a.col_abs, BIFF8_COLS, MAX_COLS), ..a };
    e.map(&mut |x| match x {
        Expr::Ref(r) => {
            let kind = match r.kind {
                RefKind::Cell(a) => RefKind::Cell(anchor(a)),
                RefKind::Range(a, b) => RefKind::Range(anchor(a), anchor(b)),
                RefKind::Rows(r0, a0, r1, a1) => RefKind::Rows(line(r0, a0, BIFF8_ROWS, MAX_ROWS), a0, line(r1, a1, BIFF8_ROWS, MAX_ROWS), a1),
                RefKind::Cols(c0, a0, c1, a1) => RefKind::Cols(line(c0, a0, BIFF8_COLS, MAX_COLS), a0, line(c1, a1, BIFF8_COLS, MAX_COLS), a1),
            };
            Expr::Ref(Reference { sheet: r.sheet, kind })
        }
        other => other,
    })
}

/// A legacy-grid name's formula text as an XLSX file holds it ([`legacy_to_xlsx_residues`]), or
/// `None` when it is written as it is (no residue changes, or the text doesn't parse).
pub fn legacy_name_text_for_xlsx(text: &str) -> Option<String> {
    let e = crate::parse_name(text).ok()?;
    let converted = legacy_to_xlsx_residues(e.clone());
    (converted != e).then(|| crate::print(&converted))
}

/// Paste Special ▸ Transpose: the cell holding the formula moves from `src` to `dest`, so every
/// reference turns a quarter turn with it — its row offset becomes its column offset and the other
/// way round, `(r, c)` → `dest + (c − src.col, r − src.row)`.
///
/// As Excel writes it out: a reference that pins a row or a column with `$` is left exactly as it
/// stands, a whole-row or whole-column reference comes back as a cell range whose open side runs
/// from the transposed far edge of the sheet, and a result that falls off the grid becomes `#REF!`.
pub fn transpose_relative(e: Expr, src: CellRef, dest: CellRef) -> Expr {
    e.map(&mut |x| match x {
        Expr::Ref(r) if !has_absolute(&r) => match transpose_ref(&r, src, dest) {
            Some(r) => Expr::Ref(r),
            None => Expr::Error(CellError::Ref),
        },
        other => other,
    })
}

/// Whether any end of the reference pins a row or a column with `$`.
fn has_absolute(r: &Reference) -> bool {
    let abs = |a: &Anchor| a.row_abs || a.col_abs;
    match &r.kind {
        RefKind::Cell(a) => abs(a),
        RefKind::Range(a, b) => abs(a) || abs(b),
        RefKind::Rows(_, a0, _, a1) | RefKind::Cols(_, a0, _, a1) => *a0 || *a1,
    }
}

/// Where an end lands after the turn: its column drives the new row, its row the new column.
fn transpose_anchor(a: &Anchor, src: CellRef, dest: CellRef) -> Option<(u32, u32)> {
    clamp_anchor(dest.row as i64 + a.col as i64 - src.col as i64, dest.col as i64 + a.row as i64 - src.row as i64)
}

/// Turns one reference with `src` → `dest`: `None` when an end falls outside the sheet, which
/// the caller turns into `#REF!`.
fn transpose_ref(r: &Reference, src: CellRef, dest: CellRef) -> Option<Reference> {
    let kind = match &r.kind {
        RefKind::Cell(a) => {
            let (row, col) = transpose_anchor(a, src, dest)?;
            RefKind::Cell(Anchor { row, col, ..*a })
        }
        RefKind::Range(a, b) => {
            let (row, col) = transpose_anchor(a, src, dest)?;
            let (row2, col2) = transpose_anchor(b, src, dest)?;
            RefKind::Range(Anchor { row, col, ..*a }, Anchor { row: row2, col: col2, ..*b })
        }
        // A whole row turns into a cell range: its rows become columns, while its open columns
        // become rows pinned with `$` — from the transposed first column down to the last row.
        RefKind::Rows(r0, _, r1, _) => {
            let (row, col) = clamp_anchor(dest.row as i64 - src.col as i64, dest.col as i64 + *r0 as i64 - src.row as i64)?;
            let (row2, col2) = clamp_anchor(MAX_ROWS as i64 - 1, dest.col as i64 + *r1 as i64 - src.row as i64)?;
            RefKind::Range(Anchor { row, col, row_abs: true, col_abs: false }, Anchor { row: row2, col: col2, row_abs: true, col_abs: false })
        }
        // A whole column likewise: its columns become rows, its open rows become columns up to XFD.
        RefKind::Cols(c0, _, c1, _) => {
            let (row, col) = clamp_anchor(dest.row as i64 + *c0 as i64 - src.col as i64, dest.col as i64 - src.row as i64)?;
            let (row2, col2) = clamp_anchor(dest.row as i64 + *c1 as i64 - src.col as i64, MAX_COLS as i64 - 1)?;
            RefKind::Range(Anchor { row, col, row_abs: false, col_abs: true }, Anchor { row: row2, col: col2, row_abs: false, col_abs: true })
        }
    };
    Some(Reference { sheet: r.sheet.clone(), kind })
}

/// Whether the formula has a relative (no `$`) row or column anywhere.
pub fn has_relative_refs(e: &Expr) -> bool {
    e.references().iter().any(|r| match r.kind {
        RefKind::Cell(a) => !a.row_abs || !a.col_abs,
        RefKind::Range(a, b) => !a.row_abs || !a.col_abs || !b.row_abs || !b.col_abs,
        RefKind::Rows(_, a0, _, a1) | RefKind::Cols(_, a0, _, a1) => !a0 || !a1,
    })
}

fn shift_anchor(a: &Anchor, dr: i64, dc: i64) -> Option<Anchor> {
    let row = if a.row_abs { a.row as i64 } else { a.row as i64 + dr };
    let col = if a.col_abs { a.col as i64 } else { a.col as i64 + dc };
    let (row, col) = clamp_anchor(row, col)?;
    Some(Anchor { row, col, ..*a })
}

fn shift_line(v: u32, abs: bool, d: i64, max: u32) -> Option<u32> {
    if abs {
        return Some(v);
    }
    let n = v as i64 + d;
    if (0..max as i64).contains(&n) { Some(n as u32) } else { None }
}

/// Shifts a [`RefKind`]'s relative coordinates by `(dr, dc)` without cloning sheet names.
/// Returns `None` if any relative coordinate leaves the worksheet grid (`#REF!`).
pub fn shift_ref_kind(kind: &RefKind, dr: i64, dc: i64) -> Option<RefKind> {
    Some(match kind {
        RefKind::Cell(a) => RefKind::Cell(shift_anchor(a, dr, dc)?),
        RefKind::Range(a, b) => RefKind::Range(shift_anchor(a, dr, dc)?, shift_anchor(b, dr, dc)?),
        RefKind::Rows(r0, a0, r1, a1) => RefKind::Rows(shift_line(*r0, *a0, dr, MAX_ROWS)?, *a0, shift_line(*r1, *a1, dr, MAX_ROWS)?, *a1),
        RefKind::Cols(c0, a0, c1, a1) => RefKind::Cols(shift_line(*c0, *a0, dc, MAX_COLS)?, *a0, shift_line(*c1, *a1, dc, MAX_COLS)?, *a1),
    })
}

fn shift_ref(r: Reference, dr: i64, dc: i64) -> Option<Reference> {
    Some(Reference { sheet: r.sheet, kind: shift_ref_kind(&r.kind, dr, dc)? })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    Rows,
    Cols,
}

/// A structural edit on one sheet.
#[derive(Clone, Debug, PartialEq)]
pub enum Edit {
    /// Insert `count` rows/cols before index `at`.
    Insert { axis: Axis, at: u32, count: u32 },
    /// Delete `count` rows/cols starting at `at`.
    Delete { axis: Axis, at: u32, count: u32 },
    /// Cut-paste of a block: references into `from` now point into the block at `to` (top-left).
    Move { from: RangeRef, to_row: u32, to_col: u32 },
    /// Insert Cells shifting down (`Rows`) or right (`Cols`): the cells of `range` and those
    /// below (right of) it move by its height (width), within its columns (rows) only.
    InsertCells { axis: Axis, range: RangeRef },
    /// Delete Cells shifting up (`Rows`) or left (`Cols`): `range` goes and the cells below
    /// (right of) it move back by its height (width), within its columns (rows) only.
    DeleteCells { axis: Axis, range: RangeRef },
    /// Cut-paste to another sheet; fully contained references follow the destination sheet too.
    MoveToSheet { from: RangeRef, to_sheet: String, to_row: u32, to_col: u32 },
}

/// Whether `r`, in a formula living on sheet `host`, points into sheet `target`.
fn on_target(r: &Reference, host: &str, target: &str) -> bool {
    match &r.sheet {
        SheetSel::Current => host.eq_ignore_ascii_case(target),
        SheetSel::Named(n) => n.eq_ignore_ascii_case(target),
        SheetSel::Span(..) => false,
    }
}

/// Whether [`adjust`] would change `r`. It runs the same [`adjust_ref`] on a copy without the
/// sheet name, so no name is copied: the adjustment reads only the position, and the sheet a cut
/// to another sheet assigns is compared afterwards.
fn ref_changes(r: &Reference, edit: &Edit) -> bool {
    let probe = Reference { sheet: SheetSel::Current, kind: r.kind.clone() };
    match adjust_ref(probe, edit) {
        None => true,
        Some(p) => p.kind != r.kind || (p.sheet != SheetSel::Current && p.sheet != r.sheet),
    }
}

/// Whether [`adjust`] would change the formula `e` at all, without copying or rebuilding it.
fn changes(e: &Expr, host: &str, target: &str, edit: &Edit) -> bool {
    let mut changed = false;
    e.walk(&mut |x| {
        if !changed && let Expr::Ref(r) = x {
            changed = on_target(r, host, target) && ref_changes(r, edit);
        }
    });
    changed
}

/// Adjusts a formula living on sheet `host` after `edit` on sheet `target`.
pub fn adjust(e: Expr, host: &str, target: &str, edit: &Edit) -> Expr {
    e.map(&mut |x| match x {
        Expr::Ref(r) if on_target(&r, host, target) => match adjust_ref(r, edit) {
            Some(r) => Expr::Ref(r),
            None => Expr::Error(CellError::Ref),
        },
        other => other,
    })
}

/// [`adjust`] for a legacy-grid `.xls` name ([`shift_relative_wrapping_on`] with the BIFF8
/// grid). Its relative parts are offsets from the cell that uses it, not positions, so an edit
/// leaves them where they are; its `$` parts move and are deleted as in [`adjust`]. A reference
/// with a relative part keeps its place under a move or a cell shift, which relocate positions.
pub fn adjust_legacy_name(e: Expr, host: &str, target: &str, edit: &Edit) -> Expr {
    e.map(&mut |x| match x {
        Expr::Ref(r) if on_target(&r, host, target) => match adjust_legacy_ref(r, edit) {
            Some(r) => Expr::Ref(r),
            None => Expr::Error(CellError::Ref),
        },
        other => other,
    })
}

fn adjust_legacy_ref(r: Reference, edit: &Edit) -> Option<Reference> {
    let (Edit::Insert { axis, at, count } | Edit::Delete { axis, at, count }) = edit else {
        return if has_absolute_only(&r) { adjust_ref(r, edit) } else { Some(r) };
    };
    let insert = matches!(edit, Edit::Insert { .. });
    let rows = *axis == Axis::Rows;
    let absolute = |a: &Anchor| if rows { a.row_abs } else { a.col_abs };
    let pinned = match &r.kind {
        RefKind::Cell(a) => absolute(a),
        RefKind::Range(a, b) => absolute(a) && absolute(b),
        RefKind::Rows(_, a, _, b) if rows => *a && *b,
        RefKind::Cols(_, a, _, b) if !rows => *a && *b,
        _ => return Some(r),
    };
    if pinned {
        return adjust_ref(r, edit);
    }
    let max = if rows { MAX_ROWS } else { MAX_COLS };
    let map = |v: u32, abs: bool| if abs { map_index(v, *at, *count, insert, max) } else { Some(v) };
    let anchor = |a: &Anchor| -> Option<Anchor> {
        Some(if rows { Anchor { row: map(a.row, a.row_abs)?, ..*a } } else { Anchor { col: map(a.col, a.col_abs)?, ..*a } })
    };
    let kind = match &r.kind {
        RefKind::Cell(a) => RefKind::Cell(anchor(a)?),
        RefKind::Range(a, b) => RefKind::Range(anchor(a)?, anchor(b)?),
        RefKind::Rows(r0, a0, r1, a1) => RefKind::Rows(map(*r0, *a0)?, *a0, map(*r1, *a1)?, *a1),
        RefKind::Cols(c0, a0, c1, a1) => RefKind::Cols(map(*c0, *a0)?, *a0, map(*c1, *a1)?, *a1),
    };
    Some(Reference { sheet: r.sheet, kind })
}

/// Whether every row and column of the reference is pinned with `$`.
fn has_absolute_only(r: &Reference) -> bool {
    let abs = |a: &Anchor| a.row_abs && a.col_abs;
    match &r.kind {
        RefKind::Cell(a) => abs(a),
        RefKind::Range(a, b) => abs(a) && abs(b),
        RefKind::Rows(_, a0, _, a1) | RefKind::Cols(_, a0, _, a1) => *a0 && *a1,
    }
}

/// [`adjust`] for a borrowed formula: `None` when the edit leaves it exactly as it is, so a
/// caller can skip unaffected formulas without copying, rebuilding or printing them.
pub fn adjust_if_changed(e: &Expr, host: &str, target: &str, edit: &Edit) -> Option<Expr> {
    changes(e, host, target, edit).then(|| adjust(e.clone(), host, target, edit))
}

/// New position of an index after an insert (`None` never) or delete (`None` = deleted).
fn map_index(v: u32, at: u32, count: u32, insert: bool, max: u32) -> Option<u32> {
    if insert {
        if v >= at {
            let n = v as u64 + count as u64;
            if n >= max as u64 { None } else { Some(n as u32) }
        } else {
            Some(v)
        }
    } else if v < at {
        Some(v)
    } else if v >= at.saturating_add(count) {
        Some(v - count)
    } else {
        None
    }
}

/// Adjusts a span `[lo, hi]` (inclusive) on one axis.
fn map_span(lo: u32, hi: u32, at: u32, count: u32, insert: bool, max: u32) -> Option<(u32, u32)> {
    if insert {
        let lo2 = if lo >= at { lo as u64 + count as u64 } else { lo as u64 };
        let hi2 = if hi >= at { hi as u64 + count as u64 } else { hi as u64 };
        // Whole-sheet spans (e.g. A:A) stay whole.
        let hi2 = hi2.min(max as u64 - 1);
        if lo2 > hi2 {
            return None;
        }
        Some((lo2 as u32, hi2 as u32))
    } else {
        let end = at.saturating_add(count); // exclusive
        if lo >= at && hi < end {
            return None; // fully deleted
        }
        let lo2 = if lo < at {
            lo
        } else if lo >= end {
            lo - count
        } else {
            at
        };
        let hi2 = if hi < at {
            hi
        } else if hi >= end {
            hi - count
        } else {
            at.saturating_sub(1)
        };
        Some((lo2, hi2.max(lo2)))
    }
}

fn adjust_ref(r: Reference, edit: &Edit) -> Option<Reference> {
    match edit {
        Edit::Insert { axis, at, count } | Edit::Delete { axis, at, count } => {
            let insert = matches!(edit, Edit::Insert { .. });
            let kind = match (&r.kind, axis) {
                (RefKind::Cell(a), Axis::Rows) => RefKind::Cell(Anchor { row: map_index(a.row, *at, *count, insert, MAX_ROWS)?, ..*a }),
                (RefKind::Cell(a), Axis::Cols) => RefKind::Cell(Anchor { col: map_index(a.col, *at, *count, insert, MAX_COLS)?, ..*a }),
                (RefKind::Range(a, b), Axis::Rows) => {
                    let (lo, hi) = map_span(a.row.min(b.row), a.row.max(b.row), *at, *count, insert, MAX_ROWS)?;
                    RefKind::Range(Anchor { row: lo, ..*a }, Anchor { row: hi, ..*b })
                }
                (RefKind::Range(a, b), Axis::Cols) => {
                    let (lo, hi) = map_span(a.col.min(b.col), a.col.max(b.col), *at, *count, insert, MAX_COLS)?;
                    RefKind::Range(Anchor { col: lo, ..*a }, Anchor { col: hi, ..*b })
                }
                (RefKind::Rows(r0, a0, r1, a1), Axis::Rows) => {
                    let (lo, hi) = map_span(*r0.min(r1), *r0.max(r1), *at, *count, insert, MAX_ROWS)?;
                    RefKind::Rows(lo, *a0, hi, *a1)
                }
                (RefKind::Cols(c0, a0, c1, a1), Axis::Cols) => {
                    let (lo, hi) = map_span(*c0.min(c1), *c0.max(c1), *at, *count, insert, MAX_COLS)?;
                    RefKind::Cols(lo, *a0, hi, *a1)
                }
                (k, _) => k.clone(),
            };
            Some(Reference { sheet: r.sheet, kind })
        }
        Edit::InsertCells { axis, range } | Edit::DeleteCells { axis, range } => {
            // Like inserting/deleting lines, for references that lie within the shifted
            // columns (rows); Excel leaves references reaching outside them as they are.
            let span = r.range();
            let (inside, at, count) = match axis {
                Axis::Rows => (range.start.col <= span.start.col && span.end.col <= range.end.col, range.start.row, range.height()),
                Axis::Cols => (range.start.row <= span.start.row && span.end.row <= range.end.row, range.start.col, range.width()),
            };
            if !inside {
                return Some(r);
            }
            let axis = *axis;
            let lines = if matches!(edit, Edit::InsertCells { .. }) { Edit::Insert { axis, at, count } } else { Edit::Delete { axis, at, count } };
            adjust_ref(r, &lines)
        }
        Edit::Move { from, to_row, to_col } => {
            let dr = *to_row as i64 - from.start.row as i64;
            let dc = *to_col as i64 - from.start.col as i64;
            let range = r.range();
            if !from.contains_range(&range) {
                return Some(r);
            }
            let mv = |a: &Anchor| -> Option<Anchor> {
                let (row, col) = clamp_anchor(a.row as i64 + dr, a.col as i64 + dc)?;
                Some(Anchor { row, col, ..*a })
            };
            let kind = match &r.kind {
                RefKind::Cell(a) => RefKind::Cell(mv(a)?),
                RefKind::Range(a, b) => RefKind::Range(mv(a)?, mv(b)?),
                RefKind::Rows(a, aa, b, ba) => {
                    let dr = *to_row as i64 - from.start.row as i64;
                    RefKind::Rows(shift_line(*a, false, dr, MAX_ROWS)?, *aa, shift_line(*b, false, dr, MAX_ROWS)?, *ba)
                }
                RefKind::Cols(a, aa, b, ba) => {
                    let dc = *to_col as i64 - from.start.col as i64;
                    RefKind::Cols(shift_line(*a, false, dc, MAX_COLS)?, *aa, shift_line(*b, false, dc, MAX_COLS)?, *ba)
                }
            };
            Some(Reference { sheet: r.sheet, kind })
        }
        Edit::MoveToSheet { from, to_sheet, to_row, to_col } => {
            if !from.contains_range(&r.range()) {
                return Some(r);
            }
            let mut moved = adjust_ref(r, &Edit::Move { from: *from, to_row: *to_row, to_col: *to_col })?;
            moved.sheet = SheetSel::Named(to_sheet.clone());
            Some(moved)
        }
    }
}

/// Renames sheet references `old` → `new`. A span into a linked workbook (`[1]Jan:Dec`) names
/// that workbook's sheets at both ends, so it never matches a sheet of this one.
pub fn rename_sheet(e: Expr, old: &str, new: &str) -> Expr {
    e.map(&mut |x| match x {
        Expr::Ref(mut r) => {
            match &mut r.sheet {
                SheetSel::Named(n) if n.eq_ignore_ascii_case(old) => *n = new.to_string(),
                SheetSel::Span(a, b) if !linked_span(a) => {
                    if a.eq_ignore_ascii_case(old) {
                        *a = new.to_string();
                    }
                    if b.eq_ignore_ascii_case(old) {
                        *b = new.to_string();
                    }
                }
                _ => {}
            }
            Expr::Ref(r)
        }
        // Sheet-qualified defined name `Old!MyName` is stored as one string, with the bare sheet name as the parser
        // stores it (the printer adds the quotes).
        // A sheet name may itself hold a '!', so the name splits at its last one.
        Expr::Name(n) => match n.rsplit_once('!') {
            Some((sheet, rest)) if sheet.eq_ignore_ascii_case(old) => Expr::Name(format!("{new}!{rest}")),
            _ => Expr::Name(n),
        },
        other => other,
    })
}

/// References to a deleted sheet become `#REF!`. `order` lists the sheet names (before the deletion) in tab order:
/// a 3-D span that loses an end sheet shrinks to the neighbouring surviving sheet, as in Excel.
pub fn delete_sheet(e: Expr, name: &str, order: &[String]) -> Expr {
    let pos = |n: &str| order.iter().position(|o| o.eq_ignore_ascii_case(n));
    e.map(&mut |x| match x {
        Expr::Ref(r) if r.sheet_name().is_some_and(|n| n.eq_ignore_ascii_case(name)) => Expr::Error(CellError::Ref),
        Expr::Ref(mut r) => {
            if let SheetSel::Span(a, b) = &r.sheet
                && !linked_span(a)
                && (a.eq_ignore_ascii_case(name) || b.eq_ignore_ascii_case(name))
            {
                let (Some(ia), Some(ib)) = (pos(a), pos(b)) else { return Expr::Ref(r) };
                if ia == ib {
                    return Expr::Error(CellError::Ref);
                }
                // The deleted end moves one sheet towards the other end; the span keeps its direction.
                let inward = |from: usize, to: usize| order.get(if from < to { from + 1 } else { from.saturating_sub(1) });
                let (na, nb) = if a.eq_ignore_ascii_case(name) { (inward(ia, ib), order.get(ib)) } else { (order.get(ia), inward(ib, ia)) };
                let (Some(na), Some(nb)) = (na, nb) else { return Expr::Error(CellError::Ref) };
                r.sheet = if na.eq_ignore_ascii_case(nb) { SheetSel::Named(na.clone()) } else { SheetSel::Span(na.clone(), nb.clone()) };
            }
            Expr::Ref(r)
        }
        other => other,
    })
}

/// Whether a 3-D span whose first sheet is `first` is into a linked workbook (`[1]Jan:Dec`).
/// Excel forbids `[` in sheet names, so no sheet of this workbook starts with one.
fn linked_span(first: &str) -> bool {
    first.starts_with('[')
}
