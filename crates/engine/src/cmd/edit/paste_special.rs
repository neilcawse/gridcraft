use std::collections::BTreeMap;

use gridcraft_model::{LineInfo, Sheet, Workbook};

use crate::cmd::Ctx;

use super::clipboard::What;
use super::{
    Cell, CellRef, EngineError, Formula, Json, RangeRef, Result, Session, StyleId, Value, bad, bool_param, cell_param_on, drop_linked_books, edit,
    json, shared_links, str_param, target_sheet,
};

#[derive(Clone, Copy)]
pub(super) enum PasteRoute {
    Clipboard,
    Special,
    Drag,
}

pub(super) fn paste_special(s: &mut Session, p: &Json) -> Result<Json> {
    paste_internal(s, p, PasteRoute::Special)
}

#[allow(clippy::cognitive_complexity)] // The command applies the selected paste mode in one atomic edit.
pub(super) fn paste_internal(s: &mut Session, p: &Json, route: PasteRoute) -> Result<Json> {
    let Some(clip) = s.clipboard.clone() else { return Err(EngineError::Other("Nothing to paste.".into())) };
    let what = match str_param(p, "what").unwrap_or("all") {
        "all" => What::All,
        "formulas" => What::Formulas,
        "values" => What::Values,
        "formats" => What::Formats,
        "comments" | "notes" => What::Comments,
        "validation" => What::Validation,
        "allExceptBorders" => What::AllExceptBorders,
        "columnWidths" => What::ColumnWidths,
        "formulasAndNumberFormats" => What::FormulasAndNumberFormats,
        "valuesAndNumberFormats" => What::ValuesAndNumberFormats,
        other => return Err(bad("edit.pasteSpecial", format!("unknown `what`: {other}"))),
    };
    let op = str_param(p, "operation").unwrap_or("none").to_string();
    let skip_blanks = bool_param(p, "skipBlanks").unwrap_or(false);
    let transpose = bool_param(p, "transpose").unwrap_or(false);
    let link = bool_param(p, "link").unwrap_or(false);
    if link && !clip.areas.is_empty() {
        return Err(EngineError::Other("This action won't work on multiple selections.".into()));
    }
    let dest_sheet = target_sheet(s, p)?;
    let sel = s.doc()?.selection.current();
    let at = cell_param_on(s, p, "at", dest_sheet)?.unwrap_or(sel.start);
    // Whole columns or rows paste whole (the copy keeps only their used part as its text), so
    // what the destination columns hold further down is replaced too, as in Excel. The cells are
    // visited sparsely below, so this costs what the columns hold, not 1,048,576 rows.
    let whole = clip.areas.is_empty() && (clip.selected.is_full_cols() || clip.selected.is_full_rows()) && clip.selected.contains_range(&clip.range);
    let src = if whole { clip.selected } else { clip.range };
    let (h, w) = if transpose { (src.width(), src.height()) } else { (src.height(), src.width()) };
    // Tile the copy over a larger destination that is a multiple of its size. A selection
    // narrower than the copy but a multiple of its height takes it down every row, at the copy's
    // width (and a shorter one a multiple of its width across), as in Excel.
    let multiple = sel.height() % h == 0 && sel.width() % w == 0;
    let down = sel.width() < w && sel.height() > h && sel.height() % h == 0;
    let across = sel.height() < h && sel.width() > w && sel.width() % w == 0;
    // The cells the tiles cover: the selection itself, or for `down`/`across` the selection at the
    // copy's width/height, which can be much larger than what was selected.
    let area = if down {
        u64::from(sel.height()) * u64::from(w)
    } else if across {
        u64::from(h) * u64::from(sel.width())
    } else {
        sel.count()
    };
    let tile = (multiple || down || across)
        && !sel.is_single()
        && cell_param_on(s, p, "at", dest_sheet)?.is_none()
        && !sel.is_full_cols()
        && !sel.is_full_rows()
        && area <= 4_000_000;
    let (tiles_r, tiles_c) = if tile { ((sel.height() / h).max(1), (sel.width() / w).max(1)) } else { (1, 1) };
    // A single cell or row repeated down a filtered selection skips the rows the filter hides; a
    // taller block is pasted straight down, hidden rows included, as in Excel.
    let skip_rows = match s.doc()?.wb.sheet(dest_sheet) {
        Some(sh) if tile && h == 1 => super::filtered_rows(sh, sel),
        _ => Default::default(),
    };
    // Excel refuses a paste that would run off the sheet (whole columns pasted below row 1, whole
    // rows right of column A, a whole column transposed) instead of pasting part of it.
    if at.offset(i64::from(tiles_r * h) - 1, i64::from(tiles_c * w) - 1).is_none() {
        return Err(EngineError::Other("The copied cells don't fit here: paste them where the whole copied area fits on the sheet.".into()));
    }
    // Paste Link writes a formula into every cell, blank ones included, so it is the one paste
    // whose work grows with the area rather than with what the source holds.
    let link_src = clip.range;
    if link && link_src.count().saturating_mul(u64::from(tiles_r) * u64::from(tiles_c)) > MAX_PASTE_LINK_CELLS {
        return Err(EngineError::Other("Too many cells to paste as links at once.".into()));
    }
    let same_doc = s.doc()?.uid == clip.doc_uid;
    // A cut in this workbook moves its cells: the source is emptied before the destination is
    // written. As in Excel, it moves only as a plain Paste onto one cell or an area of its own size
    // and shape. Every other way (a Paste Special option, tiling over a larger or differently shaped
    // area, a row the filter hides) would write the cut cells somewhere other than one place, or not
    // at all, after their source was gone, so it is refused here, before anything changes, and the
    // cut stays pending.
    let cut_move = clip.cut && same_doc;
    if cut_move {
        let plain = what == What::All && !link && !skip_blanks && !transpose && op == "none";
        if !plain {
            return Err(EngineError::Other("Cut cells can only be pasted with Paste. To use Paste Special, copy them instead.".into()));
        }
        clip.check_cut_source(&s.doc()?.wb)?;
        let anchored = cell_param_on(s, p, "at", dest_sheet)?.is_some() || sel.is_single();
        if !anchored && (sel.height() != h || sel.width() != w) {
            return Err(EngineError::Other(
                "The cut cells and the paste area aren't the same size and shape: select one cell, or an area the size of the cut cells.".into(),
            ));
        }
        // The rows the cut itself would land on, whatever the paste would skip: an anchored or
        // taller block is pasted straight down, so `skip_rows` (which only a tiled single row uses)
        // can't tell whether it would write into a hidden row.
        if let Some(end) = at.offset(i64::from(h) - 1, i64::from(w) - 1) {
            let dest = RangeRef::new(at, end);
            if s.doc()?.wb.sheet(dest_sheet).is_some_and(|sh| !super::filtered_rows(sh, dest).is_empty()) {
                return Err(EngineError::Other("The cut area can't be pasted into rows the filter hides.".into()));
            }
        }
    }
    let src_wb = clip.wb.clone();
    let Some(src_sheet) = src_wb.sheet(clip.sheet) else { return Err(EngineError::Other("The copied sheet no longer exists.".into())) };
    // The source sheet's name now: a cut whose sheet was renamed after it moves its cells,
    // rules and the references to them on the sheet as it is named, not as the clipboard saw it.
    let src_sheet_name = match s.doc()?.wb.sheet(clip.sheet) {
        Some(live) if cut_move => live.name.clone(),
        _ => src_sheet.name.clone(),
    };
    let shared = if same_doc { vec![] } else { shared_links(&src_wb, &s.doc()?.wb) };
    // A cut moves its block as it is, as Excel's does: it can't be transposed, tiled over a
    // larger selection or pasted skipping blanks (the move's references, rules and arrays follow
    // the whole block unturned, and its rules replace the destination's under every cell).
    if cut_move && (transpose || skip_blanks || tiles_r > 1 || tiles_c > 1) {
        return Err(EngineError::Other("The cut area can only be pasted as it is: paste it without Transpose or Skip Blanks, into one cell.".into()));
    }
    // A geometric full-width or full-height source counts regardless of how it was selected.
    // Packed/filtered, tiled and full-sheet copies are outside the contiguous line case.
    // Only plain Paste All transfers line formats; modifiers keep their cell-only behavior.
    let line_axis = if !matches!(route, PasteRoute::Drag)
        && what == What::All
        && op == "none"
        && !skip_blanks
        && !transpose
        && !link
        && whole
        && !clip.filtered
        && !tile
    {
        match (src.is_full_rows(), src.is_full_cols()) {
            (true, false) => Some(LineAxis::Rows),
            (false, true) => Some(LineAxis::Columns),
            _ => None,
        }
    } else {
        None
    };
    edit(s, |cx| {
        // Styles from another workbook must be re-interned.
        let dest_name = cx.wb.sheet(dest_sheet).map(|sh| sh.name.clone()).unwrap_or_default();
        let mut style_map = std::collections::HashMap::new();
        let mut map_style = |id: StyleId, wb: &mut gridcraft_model::Workbook| -> StyleId {
            if same_doc {
                return id;
            }
            *style_map.entry(id).or_insert_with(|| wb.styles.intern(src_wb.styles.get(id).clone()))
        };
        let protected_dest = line_axis.and_then(|_| cx.wb.sheets.get(dest_sheet).filter(|sh| sh.is_protected()).cloned());
        let mut protected_source = None;
        if let Some(axis) = line_axis {
            let (first, last) = (axis.index(src.start), axis.index(src.end));
            let dest = axis.index(at);
            // A Cut moves its lines as they are now, like its cells; a copy pastes the snapshot.
            // Hold the pre-reset sheet (checked above) for overlapping moves.
            let live = if cut_move { cx.wb.sheets.get(clip.sheet).cloned() } else { None };
            let from = live.as_deref().unwrap_or(src_sheet);
            let incoming = axis.lines(from).range(first..=last).map(|(&i, &info)| (dest + (i - first), info));
            let (dest_last, moving) = (dest + (last - first), cut_move && (clip.sheet != dest_sheet || first != dest));
            if moving && clip.sheet == dest_sheet && first <= dest_last && dest <= last {
                // Replace overlapping lines directly with their final format so the row/column
                // formatting permission is checked against actual changes, not a temporary reset.
                axis.replace(cx, dest_sheet, first.min(dest)..=last.max(dest_last), incoming, &mut map_style, true)?;
            } else {
                if moving {
                    axis.replace(cx, clip.sheet, first..=last, std::iter::empty(), &mut map_style, true)?;
                }
                axis.replace(cx, dest_sheet, dest..=dest_last, incoming, &mut map_style, true)?;
            }
            let plain_copy = matches!(route, PasteRoute::Clipboard) && same_doc && !clip.cut;
            axis.mark_collapsed_summary(cx, dest_sheet, dest, dest_last, plain_copy)?;
            if moving && clip.sheet != dest_sheet {
                protected_source = live.filter(|sh| sh.is_protected());
            }
        }
        let mut moved_formulas = std::collections::BTreeMap::new();
        let mut moved_pictures = std::collections::BTreeMap::new();
        // A cut moves the rules its cells hold now, not the clipboard's: rules cleared or added
        // since the cut would otherwise come back or be lost.
        let live_source = if cut_move { cx.wb.sheet(clip.sheet).cloned() } else { None };
        if cut_move {
            // Remove the source first (contents and formats), and fix references to it.
            let source = cx.sheet_mut(clip.sheet)?;
            moved_pictures = source.cell_pictures.iter().filter(|(c, _)| src.contains(**c)).map(|(c, p)| (*c, p.clone())).collect();
            let taken = source.take_cells(src);
            // Notes, hyperlinks and merged areas sit beside the cells, and a cut takes them
            // with the block: the destination is written from the clipboard snapshot taken
            // before the cut, so removing them here loses nothing and leaves the old place
            // clean instead of leaving one merged block and one note behind.
            source.comments.retain(|c, _| !src.contains(*c));
            source.hyperlinks.retain(|c, _| !src.contains(*c));
            source.merges.retain(|m| !src.contains_range(m));
            for (c, _) in &taken {
                cx.touch(clip.sheet, *c);
            }
            moved_formulas = taken.into_iter().collect();
        }
        // The cells of the copied block that hold something (or that a link reads), as offsets
        // inside it. Blank source cells matter only where the destination holds something.
        let source_offsets: std::collections::BTreeSet<(u32, u32)> = if link {
            (0..link_src.height()).flat_map(|r| (0..link_src.width()).map(move |c| (r, c))).collect()
        } else {
            let offset = |c: CellRef| (c.row - src.start.row, c.col - src.start.col);
            let mut held: std::collections::BTreeSet<(u32, u32)> = src_sheet.cells.iter_range(src).map(|(c, _)| offset(c)).collect();
            let side =
                src_sheet.spill.keys().chain(src_sheet.cell_pictures.keys()).chain(src_sheet.comments.keys()).chain(src_sheet.hyperlinks.keys());
            held.extend(side.chain(moved_formulas.keys()).chain(moved_pictures.keys()).filter(|c| src.contains(**c)).map(|c| offset(*c)));
            held
        };
        let skip_cells = matches!(what, What::Validation | What::ColumnWidths);
        // A blank source cell clears (or reformats) a destination cell that holds something, so
        // those cells join the visit. One scan of the whole pasted area sorts them by tile.
        let mut dest_held: std::collections::BTreeMap<(u32, u32), Vec<(u32, u32)>> = Default::default();
        if !link && !skip_cells {
            let pasted = RangeRef::new(at, at.offset_clamped(i64::from(tiles_r * h) - 1, i64::from(tiles_c * w) - 1));
            let held =
                cx.wb.sheet(dest_sheet).into_iter().flat_map(|sh| {
                    sh.cells.iter_range(pasted).map(|(c, _)| c).chain(sh.cell_pictures.keys().copied().filter(|c| pasted.contains(*c)))
                });
            for d in held.collect::<Vec<_>>() {
                let (dr, dc) = (d.row - at.row, d.col - at.col);
                let local = (dr % h, dc % w);
                dest_held.entry((dr / h, dc / w)).or_default().push(if transpose { (local.1, local.0) } else { local });
            }
        }
        let no_cells = std::collections::BTreeSet::new();
        for tr in 0..tiles_r {
            for tc in 0..tiles_c {
                let merged;
                let offsets = match dest_held.get(&(tr, tc)) {
                    Some(extra) => {
                        merged = source_offsets.iter().chain(extra).copied().collect::<std::collections::BTreeSet<_>>();
                        &merged
                    }
                    None if skip_cells => &no_cells,
                    None => &source_offsets,
                };
                for &(r, c) in offsets {
                    let sc = CellRef::new(src.start.row + r, src.start.col + c);
                    let (dr, dc) = if transpose { (c, r) } else { (r, c) };
                    let Some(dest) = at.offset((tr * h + dr) as i64, (tc * w + dc) as i64) else { continue };
                    if skip_rows.contains(&dest.row) {
                        continue;
                    }
                    let src_cell = if cut_move { moved_formulas.get(&sc).cloned() } else { src_sheet.cell(sc).cloned() };
                    let src_val = src_sheet.value(sc);
                    let src_picture = if cut_move { moved_pictures.get(&sc) } else { src_sheet.cell_pictures.get(&sc) }.cloned();
                    if skip_blanks
                        && src_picture.is_none()
                        && src_val.is_empty()
                        && src_cell.as_ref().is_none_or(|x| x.formula.is_none() && x.py.is_none())
                    {
                        continue;
                    }
                    let old = cx.wb.sheet(dest_sheet).and_then(|sh| sh.cell(dest)).cloned().unwrap_or_default();
                    let old_picture = cx.wb.sheet(dest_sheet).and_then(|sh| sh.cell_pictures.get(&dest)).cloned();
                    let mut new_picture = old_picture.clone();
                    let mut new = old.clone();
                    let shift = |cell: &Cell| -> Option<std::sync::Arc<Formula>> {
                        let f = cell.formula.as_ref()?;
                        let e = f.expr()?;
                        // `[1]…` is the source workbook's first link: kept where this one shares it.
                        let e = if same_doc { e } else { drop_linked_books(e, &shared) };
                        if cut_move {
                            // A moved formula still reads cells on its original sheet. Qualify
                            // those references before the workbook-wide move adjustment below.
                            let e = if dest_sheet != clip.sheet {
                                e.map(&mut |x| match x {
                                    gridcraft_formula::Expr::Ref(mut r) if r.sheet == gridcraft_formula::SheetSel::Current => {
                                        r.sheet = gridcraft_formula::SheetSel::Named(src_sheet_name.clone());
                                        gridcraft_formula::Expr::Ref(r)
                                    }
                                    other => other,
                                })
                            } else {
                                e
                            };
                            let mut moved = f.rewritten(e);
                            // The array range moves with its formula.
                            let (dr, dc) = (dest.row as i64 - sc.row as i64, dest.col as i64 - sc.col as i64);
                            moved.array = f.array.and_then(|a| Some(RangeRef::new(a.start.offset(dr, dc)?, a.end.offset(dr, dc)?)));
                            moved.dynamic = f.dynamic;
                            return Some(std::sync::Arc::new(moved));
                        }
                        let moved = if transpose {
                            // A transposed paste turns the references instead of shifting them (#2450).
                            gridcraft_formula::adjust::transpose_relative(e, sc, dest)
                        } else {
                            gridcraft_formula::adjust::shift_relative(e, dest.row as i64 - sc.row as i64, dest.col as i64 - sc.col as i64)
                        };
                        Some(std::sync::Arc::new(f.rewritten(moved).with_dynamic(f.dynamic)))
                    };
                    let moved_py = |cell: &Cell| -> Result<Option<std::sync::Arc<gridcraft_model::PyCode>>> {
                        let Some(py) = cell.py.clone() else { return Ok(None) };
                        if !cut_move || dest_sheet == clip.sheet {
                            return Ok(Some(py));
                        }
                        let qualified = gridcraft_model::python::rewrite_xl(&py.code, |reference| {
                            let e = gridcraft_formula::parse(reference).ok()?.map(&mut |x| match x {
                                gridcraft_formula::Expr::Ref(mut r) if r.sheet == gridcraft_formula::SheetSel::Current => {
                                    r.sheet = gridcraft_formula::SheetSel::Named(src_sheet_name.clone());
                                    gridcraft_formula::Expr::Ref(r)
                                }
                                other => other,
                            });
                            Some(gridcraft_formula::print(&e))
                        });
                        Ok(Some(match qualified {
                            Some(code) => {
                                let moved = gridcraft_model::PyCode { code, ..(*py).clone() };
                                check_python_fits(&moved, &dest_name, dest)?;
                                std::sync::Arc::new(moved)
                            }
                            None => py,
                        }))
                    };
                    if link {
                        let sheet_prefix = if dest_sheet == clip.sheet && same_doc {
                            String::new()
                        } else {
                            format!("{}!", gridcraft_formula::quote_sheet(&src_sheet_name))
                        };
                        new.formula = Some(std::sync::Arc::new(Formula::new(&format!("={sheet_prefix}{}", sc.a1()))));
                        new.value = Value::Empty;
                        new.py = None;
                        new_picture = None;
                    } else {
                        let sc_cell = src_cell.clone().unwrap_or_default();
                        match what {
                            What::All | What::AllExceptBorders => {
                                new_picture = src_picture.clone();
                                new.formula = shift(&sc_cell);
                                // Keep cached values for formulas that may read an unavailable link.
                                new.value = if new.formula.is_some() && !cut_move { Value::Empty } else { sc_cell.value.clone() };
                                new.py = moved_py(&sc_cell)?;
                                let st = map_style(sc_cell.style, &mut cx.wb);
                                new.style = if what == What::AllExceptBorders {
                                    let borders = cx.wb.styles.get(old.style).borders;
                                    cx.wb.styles.derive(st, |s| s.borders = borders)
                                } else {
                                    st
                                };
                            }
                            What::Formulas | What::FormulasAndNumberFormats => {
                                new_picture = src_picture.clone();
                                new.formula = shift(&sc_cell);
                                new.value = if new.formula.is_some() && !cut_move { Value::Empty } else { sc_cell.value.clone() };
                                new.py = moved_py(&sc_cell)?;
                                if what == What::FormulasAndNumberFormats {
                                    let nf = src_wb.styles.get(sc_cell.style).num_fmt.clone();
                                    new.style = cx.wb.styles.derive(old.style, |s| s.num_fmt = nf);
                                }
                            }
                            What::Values | What::ValuesAndNumberFormats => {
                                new_picture = src_picture.clone();
                                new.formula = None;
                                new.py = None;
                                new.value = src_val.clone();
                                if what == What::ValuesAndNumberFormats {
                                    let nf = src_wb.styles.get(sc_cell.style).num_fmt.clone();
                                    new.style = cx.wb.styles.derive(old.style, |s| s.num_fmt = nf);
                                }
                            }
                            What::Formats => new.style = map_style(sc_cell.style, &mut cx.wb),
                            What::Comments | What::Validation | What::ColumnWidths => {}
                        }
                        // Operations combine numbers with what is already there.
                        if op != "none"
                            && matches!(what, What::All | What::Values | What::ValuesAndNumberFormats | What::Formulas)
                            && new.formula.is_none()
                            && new.py.is_none()
                        {
                            if old_picture.is_some() || new_picture.is_some() {
                                new_picture = None;
                                new.value = Value::Error(gridcraft_core::CellError::Value);
                            }
                            let a = old.value.clone();
                            let b = new.value.clone();
                            // Numbers stored as text take part (multiplying by 1 turns them into
                            // numbers); other text and errors stay as they are.
                            if let (Ok(x), Ok(y)) = (a.to_number(), b.to_number())
                                && (a.is_number() || a.is_empty() || a.is_text())
                                && (b.is_number() || b.is_empty())
                            {
                                new.value = match op.as_str() {
                                    "add" => Value::number(x + y),
                                    "subtract" => Value::number(x - y),
                                    "multiply" => Value::number(x * y),
                                    "divide" => {
                                        if y == 0.0 {
                                            Value::Error(gridcraft_core::CellError::Div0)
                                        } else {
                                            Value::number(x / y)
                                        }
                                    }
                                    _ => b.clone(),
                                };
                            } else if (a.is_text() || a.is_error()) && (b.is_number() || b.is_empty()) {
                                new.value = a;
                            }
                            // A formula already there is kept and combined with the pasted number.
                            if let Some(f) = &old.formula
                                && let Ok(y) = b.to_number()
                                && (b.is_number() || b.is_empty())
                                && let Some(sym) = match op.as_str() {
                                    "add" => Some('+'),
                                    "subtract" => Some('-'),
                                    "multiply" => Some('*'),
                                    "divide" => Some('/'),
                                    _ => None,
                                }
                            {
                                let text = format!("=({}){sym}{}", f.text(), gridcraft_core::number_to_text(y));
                                new.formula = Some(std::sync::Arc::new(Formula::new(&text)));
                                new.value = Value::Empty;
                            }
                        }
                    }
                    match what {
                        What::Comments => {
                            if let Some(cm) = src_sheet.comments.get(&sc).cloned() {
                                cx.sheet_mut(dest_sheet)?.comments.insert(dest, cm);
                            }
                        }
                        What::All => {
                            if let Some(cm) = src_sheet.comments.get(&sc).cloned() {
                                cx.sheet_mut(dest_sheet)?.comments.insert(dest, cm);
                            }
                            if let Some(h) = src_sheet.hyperlinks.get(&sc).cloned() {
                                cx.sheet_mut(dest_sheet)?.hyperlinks.insert(dest, h);
                            }
                            cx.sheet_mut(dest_sheet)?.set_cell(dest, new);
                        }
                        _ => cx.sheet_mut(dest_sheet)?.set_cell(dest, new),
                    }
                    if let Some(picture) = new_picture {
                        cx.sheet_mut(dest_sheet)?.cell_pictures.insert(dest, picture);
                    }
                    cx.touch(dest_sheet, dest);
                }
            }
        }
        let pasted = RangeRef::new(at, at.offset_clamped(i64::from(tiles_r * h) - 1, i64::from(tiles_c * w) - 1));
        if let Some(axis) = line_axis {
            if let Some(before) = &protected_dest {
                if cut_move && clip.sheet == dest_sheet && src.intersects(&pasted) {
                    axis.check_protection(&cx.wb, dest_sheet, before, src.union(&pasted))?;
                } else {
                    axis.check_protection(&cx.wb, dest_sheet, before, pasted)?;
                    if cut_move && clip.sheet == dest_sheet {
                        axis.check_protection(&cx.wb, dest_sheet, before, src)?;
                    }
                }
            }
            if let Some(before) = &protected_source {
                axis.check_protection(&cx.wb, clip.sheet, before, src)?;
            }
        }
        // A cut takes its rules along itself (`move_cut_rules`, below).
        if !link && !cut_move && matches!(what, What::All | What::AllExceptBorders | What::Validation) {
            paste_validations(cx, src_sheet, src, dest_sheet, pasted, PasteMode { transpose, skip_blanks, skip_rows: &skip_rows })?;
        }
        if what == What::ColumnWidths {
            // Each pasted column takes the width of the column copied to it (transposed, the
            // copied rows land in columns and all take the last copied column's width).
            let cols = cx.sheet_mut(dest_sheet)?;
            for dc in 0..pasted.width() {
                let info = if transpose {
                    (0..src.width()).rev().find_map(|c| src_sheet.cols.get(&(src.start.col + c)).copied())
                } else {
                    src_sheet.cols.get(&(src.start.col + dc % w)).copied()
                };
                if let Some(info) = info {
                    cols.cols.insert(pasted.start.col + dc, info);
                }
            }
        }
        // Merged areas inside the copied block travel with it.
        if matches!(what, What::All | What::Formats) && !transpose {
            let merges: Vec<RangeRef> = src_sheet.merges.iter().filter(|m| src.contains_range(m)).copied().collect();
            for m in merges {
                let dr = at.row as i64 - src.start.row as i64;
                let dc = at.col as i64 - src.start.col as i64;
                if let (Some(a), Some(b)) = (m.start.offset(dr, dc), m.end.offset(dr, dc)) {
                    let nm = RangeRef::new(a, b);
                    let sh = cx.sheet_mut(dest_sheet)?;
                    // A table can't hold merged cells (Excel removes the table on open): the
                    // cells are pasted there unmerged.
                    if sh.tables.iter().any(|t| t.range.intersects(&nm)) {
                        continue;
                    }
                    sh.merges.retain(|x| !x.intersects(&nm));
                    sh.merges.push(nm);
                }
            }
        }
        if let Some(live) = &live_source {
            move_cut_rules(cx, live, (clip.sheet, dest_sheet), src, at)?;
            // References elsewhere to the moved block now point to its new place.
            let e = if dest_sheet == clip.sheet {
                gridcraft_formula::adjust::Edit::Move { from: src, to_row: at.row, to_col: at.col }
            } else {
                gridcraft_formula::adjust::Edit::MoveToSheet { from: src, to_sheet: dest_name.clone(), to_row: at.row, to_col: at.col }
            };
            let (forgotten, moved, changed) = rewrite_all_formulas(&mut cx.wb, &src_sheet_name, &e)?;
            cx.changed.extend(forgotten);
            cx.moved_names.extend(moved);
            cx.changed_names.extend(changed);
            cx.structural = true;
        }
        let end = at.offset_clamped((tiles_r * h) as i64 - 1, (tiles_c * w) as i64 - 1);
        *cx.sel = crate::Selection { active: at, anchor: at, ranges: vec![RangeRef::new(at, end)] };
        Ok(())
    })?;
    // Formulas and validation from an untrusted file leave the destination untrusted; plain values,
    // formats (cell styles and merged areas, no conditional formats), column widths and notes
    // carry nothing active. A linked paste writes a formula into every cell whatever `what` says,
    // except for column widths and notes, which write no cells at all.
    let passive =
        matches!(what, What::ColumnWidths | What::Comments) || (!link && matches!(what, What::Formats | What::Values | What::ValuesAndNumberFormats));
    if !same_doc && !passive {
        let d = s.doc_mut()?;
        d.trust = d.trust.receive(clip.trust);
    }
    if cut_move {
        s.clipboard = None;
    }
    Ok(json!({"pasted": s.doc()?.selection.a1()}))
}

/// Check whole rows using their inherited style and sparse explicit cells. A row without a style
/// that is unlocked with formulas shown, or a cell overriding that style, is outside the
/// measured case.
fn unlocked_hidden_rows(wb: &Workbook, sh: &Sheet, first: u32, last: u32) -> bool {
    let unlocked = |style| {
        let protection = wb.styles.get(style).protection;
        !protection.locked && !protection.hidden
    };
    (first..=last).all(|row| sh.rows.get(&row).is_some_and(|info| info.hidden && info.style.is_some_and(unlocked)))
        && sh.cells.iter_range(RangeRef::rows(first, last)).all(|(at, cell)| sh.rows.get(&at.row).and_then(|row| row.style) == Some(cell.style))
}

fn formatting_denied(what: &str) -> EngineError {
    EngineError::Other(format!("{what} isn't allowed on this protected sheet (Review › Unprotect Sheet)."))
}

/// The axis of a whole-line transfer: Paste All of entire rows or columns, and AutoFill of them.
#[derive(Clone, Copy)]
pub(super) enum LineAxis {
    Rows,
    Columns,
}

impl LineAxis {
    pub(super) fn index(self, c: CellRef) -> u32 {
        match self {
            Self::Rows => c.row,
            Self::Columns => c.col,
        }
    }

    pub(super) fn span(self, first: u32, last: u32) -> RangeRef {
        match self {
            Self::Rows => RangeRef::rows(first, last),
            Self::Columns => RangeRef::cols(first, last),
        }
    }

    pub(super) fn lines(self, sheet: &Sheet) -> &BTreeMap<u32, LineInfo> {
        match self {
            Self::Rows => &sheet.rows,
            Self::Columns => &sheet.cols,
        }
    }

    /// Replaces the line formatting of `first..=last` on `sheet` with `incoming` (ascending
    /// `(index, line)` pairs inside that span; a missing index resets to the default line).
    /// Size, style and visibility transfer on both paths. A `clipboard` transfer (whole-line
    /// Copy/Cut: X25, X33/X44) also moves the outline level and the collapsed-summary marker.
    /// AutoFill (X64) leaves those on their own lines and keeps filter-hidden rows hidden.
    pub(super) fn replace(
        self,
        cx: &mut Ctx,
        sheet: usize,
        span: std::ops::RangeInclusive<u32>,
        incoming: impl Iterator<Item = (u32, LineInfo)>,
        map_style: &mut impl FnMut(StyleId, &mut Workbook) -> StyleId,
        clipboard: bool,
    ) -> Result<()> {
        let target = cx.wb.sheets.get(sheet).ok_or(EngineError::NoDocument)?;
        let filter_spans = if !clipboard && matches!(self, Self::Rows) { target.filter_row_spans() } else { Vec::new() };
        let lines = self.lines(target);
        let mut incoming = incoming.peekable();
        // Take the union of recorded source and destination lines, not the entire axis.
        let mut existing = lines.range(span).map(|(&i, &info)| (i, info)).peekable();
        let mut changes = Vec::new();
        while let Some(i) = existing.peek().map(|x| x.0).into_iter().chain(incoming.peek().map(|x| x.0)).min() {
            let old = existing.next_if(|x| x.0 == i).map(|x| x.1).unwrap_or_default();
            let new = incoming.next_if(|x| x.0 == i).map(|x| x.1).unwrap_or_default();
            // Boundary edges and bare-row markers stay on their own lines.
            let kept = if clipboard { LineInfo { outline: new.outline, collapsed: new.collapsed, ..old } } else { old };
            let info = LineInfo {
                size: new.size,
                hidden: new.hidden || (old.hidden && filter_spans.iter().any(|&(first, last)| (first..=last).contains(&i))),
                custom: new.custom,
                saved_size: new.saved_size,
                saved_col_flags: new.saved_col_flags,
                style: new.style,
                ..kept
            };
            if info != old || info.style.is_some() {
                changes.push((i, old, info));
            }
        }
        changes.retain_mut(|(_, old, info)| {
            info.style = info.style.map(|id| map_style(id, &mut cx.wb));
            *info != *old
        });
        if changes.is_empty() {
            return Ok(());
        }
        self.refuse_unless_formatting_allowed(cx, sheet)?;
        if matches!(self, Self::Rows) {
            cx.recalc_all |= changes.iter().any(|(_, old, info)| old.hidden != info.hidden);
        }
        let target = cx.sheet_mut(sheet)?;
        let lines = match self {
            Self::Rows => &mut target.rows,
            Self::Columns => &mut target.cols,
        };
        for (i, _, info) in changes {
            if info == LineInfo::default() {
                lines.remove(&i);
            } else {
                lines.insert(i, info);
            }
        }
        Ok(())
    }

    /// Infer a collapsed summary just outside the paste if a fully hidden group meets its edge.
    /// Batch 5 measured this with summaries below/right; before-detail summaries are symmetric.
    fn mark_collapsed_summary(self, cx: &mut Ctx, sheet: usize, first: u32, last: u32, plain_copy: bool) -> Result<()> {
        let target = cx.wb.sheets.get(sheet).ok_or(EngineError::NoDocument)?;
        let (rows, end) = match self {
            Self::Rows => (true, gridcraft_core::MAX_ROWS),
            Self::Columns => (false, gridcraft_core::MAX_COLS),
        };
        let before = crate::outline::summary_before(target, rows);
        let lines = self.lines(target);
        let Some(i) = (if before { first.checked_sub(1) } else { last.checked_add(1).filter(|&i| i < end) }) else { return Ok(()) };
        let boundary = if before { first } else { last };
        let Some(detail) = lines.get(&boundary) else { return Ok(()) };
        let level = detail.outline.min(crate::outline::MAX_LEVEL);
        if level == 0 || !detail.hidden || lines.get(&i).is_some_and(|l| l.collapsed || l.outline >= level) {
            return Ok(());
        }
        // Only the deepest group touching the boundary matters: all shallower groups contain
        // it and share its summary coordinate if they also end here.
        let mut run = if before { lines.range(first + 1..) } else { lines.range(..last) };
        let mut prev = boundary;
        while let Some((&index, line)) = if before { run.next() } else { run.next_back() } {
            if index.abs_diff(prev) != 1 || line.outline < level {
                break;
            }
            if !line.hidden {
                return Ok(());
            }
            prev = index;
        }
        // When Format Rows is denied, reaching here means replace() left the row records
        // unchanged. Only a plain copy onto entirely unlocked, already hidden rows whose group
        // starts inside the paste may add the below-detail summary marker in this case (X65).
        if let Some(protection) = &target.protection
            && let Some(what) = self.denied_formatting(protection)
            && !(rows && !before && plain_copy && prev >= first && unlocked_hidden_rows(&cx.wb, target, first, last))
        {
            return Err(formatting_denied(what));
        }
        let target = cx.sheet_mut(sheet)?;
        let lines = match self {
            Self::Rows => &mut target.rows,
            Self::Columns => &mut target.cols,
        };
        lines.entry(i).or_default().collapsed = true;
        Ok(())
    }

    fn refuse_unless_formatting_allowed(self, cx: &Ctx, sheet: usize) -> Result<()> {
        if let Some(what) = cx.wb.sheet(sheet).and_then(|sh| sh.protection.as_ref()).and_then(|p| self.denied_formatting(p)) {
            return Err(formatting_denied(what));
        }
        Ok(())
    }

    fn denied_formatting(self, protection: &gridcraft_model::SheetProtection) -> Option<&'static str> {
        match self {
            Self::Rows if !protection.format_rows => Some("Formatting rows"),
            Self::Columns if !protection.format_columns => Some("Formatting columns"),
            _ => None,
        }
    }

    /// Checks protection against the final staged state, including cells that gain or lose
    /// explicit formatting. Only unrepresented cells need a separate inheritance check.
    pub(super) fn check_protection(self, wb: &Workbook, sheet: usize, before: &Sheet, span: RangeRef) -> Result<()> {
        let after = wb.sheet(sheet).ok_or(EngineError::NoDocument)?;
        let flags = |id| wb.styles.get(id).protection;
        for (at, _) in before.cells.iter_range(span).chain(after.cells.iter_range(span).filter(|(at, _)| before.cell(*at).is_none())) {
            if flags(before.style_id(at)) != flags(after.style_id(at)) {
                return Err(EngineError::Other(crate::cmd::PROTECTION_FLAGS.into()));
            }
        }
        let range = self.index(span.start)..=self.index(span.end);
        let old = self.lines(before);
        let new = self.lines(after);
        let indices = old.range(range.clone()).chain(new.range(range).filter(|(i, _)| !old.contains_key(i)));
        for (&i, _) in indices {
            self.check_blank_protection(wb, before, after, i)?;
        }
        Ok(())
    }

    fn check_blank_protection(self, wb: &Workbook, before: &Sheet, after: &Sheet, i: u32) -> Result<()> {
        // Whole-line transfers never edit the perpendicular line formats.
        let style = |sh| self.lines(sh).get(&i).and_then(|line| line.style);
        let flags = |id| wb.styles.get(id).protection;
        let (old, new) = (style(before).map(flags), style(after).map(flags));
        if old == new {
            return Ok(());
        }
        let default = flags(StyleId::DEFAULT);
        let flips = |fallback| old.unwrap_or(fallback) != new.unwrap_or(fallback);
        // Count cells whose *inherited* flags would change using the sparse perpendicular
        // formats, then subtract explicit cells (which override both line styles).
        let flips_default = flips(default);
        let n = match self {
            Self::Rows => {
                let mut n = if flips_default { gridcraft_core::MAX_COLS } else { 0 };
                if old.is_none() || new.is_none() {
                    for id in before.cols.values().filter_map(|line| line.style) {
                        match (flips_default, flips(flags(id))) {
                            (true, false) => n -= 1,
                            (false, true) => n += 1,
                            _ => (),
                        }
                    }
                }
                n
            }
            Self::Columns if flips_default => gridcraft_core::MAX_ROWS - before.rows.values().filter(|line| line.style.is_some()).count() as u32,
            Self::Columns => 0,
        };
        if n != 0 {
            let span = self.span(i, i);
            let explicit = before.cells.iter_range(span).chain(after.cells.iter_range(span).filter(|(at, _)| before.cell(*at).is_none()));
            let masked = explicit.filter(|(at, _)| match self {
                Self::Rows => flips(before.cols.get(&at.col).and_then(|line| line.style).map_or(default, flags)),
                Self::Columns => before.rows.get(&at.row).is_none_or(|line| line.style.is_none()),
            });
            if masked.take(n as usize).count() < n as usize {
                return Err(EngineError::Other(crate::cmd::PROTECTION_FLAGS.into()));
            }
        }
        Ok(())
    }
}

/// Whether a structural edit made a formula read other cells than it did, not the same cells
/// moved: a reference became `#REF!`, or a range grew or shrank.
pub(crate) fn reads_differently(old: &gridcraft_formula::Expr, new: &gridcraft_formula::Expr) -> bool {
    fn reads(e: &gridcraft_formula::Expr) -> Vec<Option<(u32, u32)>> {
        use gridcraft_formula::Expr;
        let mut out = Vec::new();
        e.walk(&mut |x| match x {
            Expr::Ref(r) => out.push(Some((r.range().height(), r.range().width()))),
            Expr::Error(gridcraft_core::CellError::Ref) | Expr::LinkedRefError(_) => out.push(None),
            _ => {}
        });
        out
    }
    reads(old) != reads(new)
}

/// A formula whose references a structural edit changed no longer shows its value: clearing it
/// means a formula over a linked name that can't be calculated is calculated again, rather than
/// keeping the value it had, and the edit counts it as changed for the formulas that read it
/// (see `Calc::recalc_all_after_edit`).
pub(crate) fn forget_value(cell: &mut Cell) {
    cell.value = Value::Empty;
}

pub(crate) type RewriteChanges = (Vec<(usize, CellRef)>, Vec<String>, Vec<String>);

/// Most cells one Paste Link writes (it writes a formula into every cell of the copied area,
/// blank ones included).
const MAX_PASTE_LINK_CELLS: u64 = 1_048_576;
/// Most validation ranges one paste adds (a rule covering part of a small block tiled over a huge
/// selection makes one range per tile).
const MAX_PASTE_VALIDATION_RANGES: usize = 100_000;

/// How a validation paste lays the rules out: transposed or not, keeping the destination's rule under
/// blank source cells, and leaving the rows a filter hides as they are.
#[derive(Clone, Copy)]
struct PasteMode<'a> {
    transpose: bool,
    skip_blanks: bool,
    skip_rows: &'a std::collections::BTreeSet<u32>,
}

/// `range` less the rows in `skip`: the runs of rows between them, as ranges of its columns.
fn without_rows(range: RangeRef, skip: &std::collections::BTreeSet<u32>) -> Vec<RangeRef> {
    let mut out = Vec::new();
    let mut next = range.start.row;
    for &row in skip.range(range.start.row..=range.end.row) {
        if row > next {
            out.push(RangeRef::new(CellRef::new(next, range.start.col), CellRef::new(row - 1, range.end.col)));
        }
        next = row + 1;
    }
    if next <= range.end.row {
        out.push(RangeRef::new(CellRef::new(next, range.start.col), CellRef::new(range.end.row, range.end.col)));
    }
    out
}

/// Pasting validation works on ranges, as Excel stores it: the pasted rectangle is cut out of the
/// destination's rules (cells whose source had no rule end up with none), then each source rule
/// that covers part of the copied block `src` comes along as one rule over the translated parts.
/// `size` is one tile's (height, width) at the destination; `pasted` is all the tiles together.
fn paste_validations(
    cx: &mut crate::cmd::Ctx<'_>,
    src_sheet: &gridcraft_model::Sheet,
    src: RangeRef,
    dest_sheet: usize,
    pasted: RangeRef,
    mode: PasteMode<'_>,
) -> Result<()> {
    let PasteMode { transpose, skip_blanks, skip_rows } = mode;
    let (h, w) = if transpose { (src.width(), src.height()) } else { (src.height(), src.width()) };
    let (tiles_r, tiles_c) = (pasted.height() / h.max(1), pasted.width() / w.max(1));
    let mut rules = Vec::new();
    // Under Skip Blanks the destination keeps its rule below a blank source cell that has none, so
    // only the destination of cells that hold something (and of the rules) is cleared. Nothing is
    // collected when no destination rule is under the paste.
    let mut cleared: Vec<RangeRef> = Vec::new();
    let dest_rules =
        cx.wb.sheet(dest_sheet).is_some_and(|sh| sh.validations.iter().any(|dv| dv.ranges.iter().any(|r| r.intersection(&pasted).is_some())));
    if skip_blanks && dest_rules {
        let mut cells = Vec::new();
        for (sc, cell) in src_sheet.cells.iter_range(src) {
            if cell.formula.is_none() && cell.py.is_none() && src_sheet.value(sc).is_empty() {
                continue;
            }
            let (r, c) = (sc.row - src.start.row, sc.col - src.start.col);
            let (r, c) = if transpose { (c, r) } else { (r, c) };
            for tr in 0..tiles_r {
                for tc in 0..tiles_c {
                    if let Some(d) = pasted.start.offset(i64::from(tr * h + r), i64::from(tc * w + c))
                        && !skip_rows.contains(&d.row)
                    {
                        cells.push((d.row, d.col));
                    }
                }
            }
        }
        // Coalesce populated cells into horizontal runs, then join identical runs in adjacent
        // rows. The range limit applies to the cuts, not the number of populated cells.
        cells.sort_unstable();
        let mut runs: Vec<RangeRef> = Vec::new();
        let mut i = 0;
        while i < cells.len() {
            let (row, start_col) = cells[i];
            let mut end_col = start_col;
            i += 1;
            while i < cells.len() && cells[i].0 == row && cells[i].1 <= end_col.saturating_add(1) {
                end_col = end_col.max(cells[i].1);
                i += 1;
            }
            runs.push(RangeRef::new(CellRef::new(row, start_col), CellRef::new(row, end_col)));
        }
        let mut last_run_by_columns = std::collections::BTreeMap::<(u32, u32), usize>::new();
        for run in runs {
            let columns = (run.start.col, run.end.col);
            if let Some(&index) = last_run_by_columns.get(&columns)
                && cleared[index].end.row.saturating_add(1) == run.start.row
            {
                cleared[index].end.row = run.end.row;
            } else {
                let index = cleared.len();
                cleared.push(run);
                last_run_by_columns.insert(columns, index);
            }
        }
    }
    let mut total = 0usize;
    for dv in &src_sheet.validations {
        let parts: Vec<RangeRef> = dv.ranges.iter().filter_map(|r| r.intersection(&src)).collect();
        let Some(first) = dv.ranges.first().map(|r| r.start) else { continue };
        if parts.is_empty() {
            continue;
        }
        let mut ranges = Vec::new();
        if parts.contains(&src) {
            ranges.push(pasted);
        } else {
            total = total.saturating_add(parts.len().saturating_mul(tiles_r as usize).saturating_mul(tiles_c as usize));
            if total > MAX_PASTE_VALIDATION_RANGES {
                return Err(EngineError::Other("Too many separate validation areas to paste at once.".into()));
            }
            for tr in 0..tiles_r {
                for tc in 0..tiles_c {
                    let Some(origin) = pasted.start.offset(i64::from(tr * h), i64::from(tc * w)) else { continue };
                    for p in &parts {
                        let (r0, c0) = (p.start.row - src.start.row, p.start.col - src.start.col);
                        let (r1, c1) = (p.end.row - src.start.row, p.end.col - src.start.col);
                        let ((r0, c0), (r1, c1)) = if transpose { ((c0, r0), (c1, r1)) } else { ((r0, c0), (r1, c1)) };
                        let Some(a) = origin.offset(i64::from(r0), i64::from(c0)) else { continue };
                        let b = origin.offset_clamped(i64::from(r1), i64::from(c1));
                        ranges.push(RangeRef::new(a, b));
                    }
                }
            }
        }
        // The rows a filter hides keep what they have.
        let ranges: Vec<RangeRef> = ranges.into_iter().flat_map(|r| without_rows(r, skip_rows)).collect();
        let Some(anchor) = ranges.first().map(|r| r.start) else { continue };
        let mut rule = dv.clone();
        // Relative references move with the rule, as pasted formulas do: they are written for the
        // rule's first cell.
        rule.f1 = super::super::data::move_rule_formula(&dv.f1, first, anchor);
        rule.f2 = dv.f2.as_ref().map(|f| super::super::data::move_rule_formula(f, first, anchor));
        rule.ranges = ranges;
        rules.push(rule);
    }
    if cleared.len() > MAX_PASTE_VALIDATION_RANGES {
        return Err(EngineError::Other("Too many separate validation areas to paste at once.".into()));
    }
    let sh = cx.sheet_mut(dest_sheet)?;
    if skip_blanks {
        cleared.extend(rules.iter().flat_map(|r| r.ranges.iter().copied()));
        super::super::data::remove_validation_cells(&mut sh.validations, &cleared);
    } else {
        super::super::data::remove_validation_cells(&mut sh.validations, &without_rows(pasted, skip_rows));
    }
    sh.validations.extend(rules);
    Ok(())
}

/// A cut, or Move Columns, takes its cells' conditional formats and validation with it, from
/// sheet `from` to the block at `at` on sheet `to`, replacing the rules there. The moved parts'
/// formulas are handled as the moved cells' are: kept written for where their cells were (on
/// another sheet, with their references to it named), so the reference rewrite for the move
/// shifts those into the block once. `snapshot` is the source sheet before the block moved.
pub(crate) fn move_cut_rules(
    cx: &mut crate::cmd::Ctx,
    snapshot: &gridcraft_model::Sheet,
    (from, to): (usize, usize),
    src: RangeRef,
    at: CellRef,
) -> Result<()> {
    let mut cfs: Vec<gridcraft_model::CondFormat> = crate::cmd::data::moved_rule_parts(&snapshot.cond_formats, src, at);
    let mut dvs: Vec<gridcraft_model::Validation> = crate::cmd::data::moved_rule_parts(&snapshot.validations, src, at);
    if from != to {
        let name = |t: &mut String| {
            if let Ok(e) = gridcraft_formula::parse(t.trim_start_matches('=')) {
                let e = e.map(&mut |x| match x {
                    gridcraft_formula::Expr::Ref(mut r) if r.sheet == gridcraft_formula::SheetSel::Current => {
                        r.sheet = gridcraft_formula::SheetSel::Named(snapshot.name.clone());
                        gridcraft_formula::Expr::Ref(r)
                    }
                    other => other,
                });
                *t = format!("{}{}", if t.starts_with('=') { "=" } else { "" }, gridcraft_formula::print(&e));
            }
        };
        // Every formula, thresholds included: an unnamed reference would read the destination sheet.
        cfs.iter_mut().flat_map(|r| r.rule.formulas_mut()).for_each(name);
        dvs.iter_mut().flat_map(|r| r.formulas_mut()).for_each(name);
    }
    place_moved_rules(cx, (from, to), src, at, (cfs, dvs))
}

/// Takes the rules off the cells of `src` on sheet `from` and puts `parts` (the moved parts of
/// them, as [`move_cut_rules`] builds them) on the block at `at` on sheet `to`, in place of the
/// rules there.
pub(crate) fn place_moved_rules(
    cx: &mut crate::cmd::Ctx,
    (from, to): (usize, usize),
    src: RangeRef,
    at: CellRef,
    (cfs, dvs): (Vec<gridcraft_model::CondFormat>, Vec<gridcraft_model::Validation>),
) -> Result<()> {
    let source = cx.sheet_mut(from)?;
    crate::cmd::data::remove_rule_cells(&mut source.cond_formats, &[src]);
    crate::cmd::data::remove_rule_cells(&mut source.validations, &[src]);
    let block = RangeRef::new(at, at.offset_clamped(i64::from(src.height()) - 1, i64::from(src.width()) - 1));
    let dest = cx.sheet_mut(to)?;
    crate::cmd::data::remove_rule_cells(&mut dest.cond_formats, &[block]);
    crate::cmd::data::remove_rule_cells(&mut dest.validations, &[block]);
    dest.cond_formats.extend(cfs);
    dest.validations.extend(dvs);
    Ok(())
}

/// Applies a reference adjustment to every reference the workbook holds
/// ([`Workbook::rewrite_references`]) and its Python cells, and moves the legacy array ranges
/// on `target`. Returns formulas whose values were cleared, names whose references only moved,
/// and names whose references changed (lowercase).
///
/// [`Workbook::rewrite_references`]: gridcraft_model::Workbook::rewrite_references
pub(crate) fn rewrite_all_formulas(wb: &mut gridcraft_model::Workbook, target: &str, e: &gridcraft_formula::adjust::Edit) -> Result<RewriteChanges> {
    rewrite_all_formulas_many(wb, target, std::slice::from_ref(e))
}

/// Applies edits in order while collecting cache invalidation and name changes.
pub(crate) fn rewrite_all_formulas_many(
    wb: &mut gridcraft_model::Workbook,
    target: &str,
    edits: &[gridcraft_formula::adjust::Edit],
) -> Result<RewriteChanges> {
    use gridcraft_formula::adjust::Edit;
    let mut forgotten = Vec::new();
    for (si, sh) in wb.sheets.iter().enumerate() {
        let host = &sh.name;
        for (c, cell) in sh.cells.iter() {
            let Some(f) = cell.formula.as_ref() else { continue };
            let diff = f.with_expr(|x| {
                let mut cur: Option<gridcraft_formula::Expr> = None;
                for e in edits {
                    if let Some(n) = gridcraft_formula::adjust::adjust_if_changed(cur.as_ref().unwrap_or(x), host, target, e) {
                        cur = Some(n);
                    }
                }
                cur.is_some_and(|n| reads_differently(x, &n))
            });
            if diff == Some(true) {
                forgotten.push((si, c));
            }
        }
    }
    let old_names: Vec<(String, String, Option<gridcraft_formula::Expr>)> =
        wb.names.iter().map(|n| (n.name.to_lowercase(), n.formula.clone(), gridcraft_formula::parse_name(&n.formula).ok())).collect();
    let adjust_all =
        |host: &str, expr: gridcraft_formula::Expr| edits.iter().fold(expr, |expr, e| gridcraft_formula::adjust::adjust(expr, host, target, e));
    // A legacy-grid `.xls` name's relative parts are offsets from the cell using it: only its `$`
    // parts move.
    let adjust_legacy = |host: &str, expr: gridcraft_formula::Expr| {
        edits.iter().fold(expr, |expr, e| gridcraft_formula::adjust::adjust_legacy_name(expr, host, target, e))
    };
    wb.rewrite_references(&|host, expr| if host.legacy_grid { adjust_legacy(host.name, expr) } else { adjust_all(host.name, expr) });
    for &(si, c) in &forgotten {
        if let Some(cell) = wb.sheet_mut(si).and_then(|sh| sh.cells.get_mut(c)) {
            forget_value(cell);
        }
    }
    let mut moved = Vec::new();
    let mut changed = Vec::new();
    for (n, (lower, old_formula, old_expr)) in wb.names.iter().zip(old_names) {
        if n.formula != old_formula {
            let diff = match (old_expr, gridcraft_formula::parse_name(&n.formula).ok()) {
                (Some(old_e), Some(new_e)) => reads_differently(&old_e, &new_e),
                _ => true,
            };
            if diff {
                changed.push(lower);
            } else {
                moved.push(lower);
            }
        }
    }
    // Rows and columns inserted or deleted on the target sheet move its legacy array ranges too
    // (a cut moves its arrays itself).
    let moves_arrays = |e: &&Edit| matches!(e, Edit::Insert { .. } | Edit::Delete { .. } | Edit::InsertCells { .. } | Edit::DeleteCells { .. });
    if edits.iter().any(|e| moves_arrays(&e))
        && let Some(si) = wb.sheet_index(target)
        && let Some(sheet) = wb.sheet_mut(si)
    {
        let arrays: Vec<(CellRef, std::sync::Arc<Formula>)> =
            sheet.cells.iter().filter_map(|(c, cell)| cell.formula.clone().filter(|f| f.array.is_some()).map(|f| (c, f))).collect();
        for (c, f) in arrays {
            let array = edits.iter().filter(moves_arrays).fold(f.array, |a, e| a.and_then(|a| crate::cmd::sheet::map_range(a, e)));
            if array != f.array
                && let Some(cell) = sheet.cells.get_mut(c)
            {
                let mut updated = (*f).clone();
                updated.array = array;
                cell.formula = Some(std::sync::Arc::new(updated));
            }
        }
    }
    rewrite_cell_links(wb, None, adjust_all);
    rewrite_python_refs(wb, &adjust_all)?;
    Ok((forgotten, moved, changed))
}

/// Runs `f` over each form control's drop-down source range, the one floating-object reference
/// [`gridcraft_model::Workbook::rewrite_references`] does not visit (it rewrites charts and linked
/// cells). Every edit that moves references calls it after that rewrite: inserting and deleting
/// lines and cells and cut-moves (through [`rewrite_all_formulas`]), and renaming and deleting
/// sheets. `f` maps a reference held by a control on the sheet named `host` to its new form; text
/// that does not parse is kept. If the edit shortens a drop-down's list, its present scroll
/// offsets are clamped. Call this before removing a source sheet so its old list resolves, naming
/// that sheet as `doomed`: its own objects are left alone, so the sheet (shared with the undo
/// copy) is not copied only to be dropped. A sheet whose lists don't change stays shared.
pub(crate) fn rewrite_cell_links(
    wb: &mut gridcraft_model::Workbook,
    doomed: Option<usize>,
    f: impl Fn(&str, gridcraft_formula::Expr) -> gridcraft_formula::Expr,
) {
    // Only resolved lists with a nonzero offset can need clamping. Keep lengths, not range text.
    let before: Vec<(usize, usize, u32)> = (0..wb.sheets.len())
        .filter(|&si| Some(si) != doomed)
        .flat_map(|si| {
            let controls = wb.sheets.get(si).map(|sh| sh.controls.as_slice()).unwrap_or_default();
            let wb = &*wb;
            controls.iter().enumerate().filter_map(move |(i, c)| {
                let has_offset = c.drop_display.scroll_offset.unwrap_or(0) > 0 || c.drop_vml_display.scroll_offset.unwrap_or(0) > 0;
                if !has_offset {
                    return None;
                }
                let len = crate::cmd::controls::drop_len(wb, si, c);
                (len > 0).then_some((si, i, len))
            })
        })
        .collect();
    for si in (0..wb.sheets.len()).filter(|&si| Some(si) != doomed) {
        let Some(sh) = wb.sheets.get(si) else { continue };
        let lists: Vec<(usize, String)> = sh
            .controls
            .iter()
            .enumerate()
            .filter_map(|(i, c)| {
                let text = c.list_range.as_deref()?;
                let fixed = gridcraft_formula::print(&f(&sh.name, gridcraft_formula::parse(text).ok()?));
                (fixed != text).then_some((i, fixed))
            })
            .collect();
        if lists.is_empty() {
            continue;
        }
        let Some(sh) = wb.sheet_mut(si) else { continue };
        for (i, fixed) in lists {
            if let Some(c) = sh.controls.get_mut(i) {
                c.list_range = Some(fixed);
            }
        }
    }
    for (si, i, was) in before {
        let Some(c) = wb.sheet(si).and_then(|sh| sh.controls.get(i)) else { continue };
        let len = crate::cmd::controls::drop_len(wb, si, c);
        if len < was
            && let Some(c) = wb.sheet_mut(si).and_then(|sh| sh.controls.get_mut(i))
        {
            crate::cmd::controls::clamp_scroll_offsets(c, len);
        }
    }
}

/// Rewrites the `xl("…")` literals in every Python cell with `f`, which maps a reference held on
/// sheet `host` to its new form. Every reference-moving edit (insert and delete of lines or cells,
/// cut-move, sheet rename and delete) runs it, as it adjusts formulas. A literal is only replaced
/// when the reference it names changed, and text that doesn't parse is kept. A cell's cached
/// result is kept while its references still name the same data (a rename, a move); a reference
/// the edit deleted (`#REF!`), or one that grew or shrank (rows or columns inserted or deleted
/// inside it), leaves the result stale, so the cell is new code (`#BLOCKED!`) and the
/// recalculation removes its spill. A rewrite that would
/// make a cell's code longer than one cell holds (a longer sheet name, `Z1` becoming `AA1`) refuses
/// the whole edit, which then changes nothing: see [`check_python_fits`].
pub(crate) fn rewrite_python_refs(
    wb: &mut gridcraft_model::Workbook,
    f: &impl Fn(&str, gridcraft_formula::Expr) -> gridcraft_formula::Expr,
) -> Result<()> {
    for si in 0..wb.sheets.len() {
        let Some(sh) = wb.sheets.get(si) else { continue };
        let host = sh.name.clone();
        let mut new_code: Vec<(CellRef, String, bool)> = Vec::new();
        for (c, cell) in sh.cells.iter() {
            let Some(py) = &cell.py else { continue };
            let mut lost = false;
            let changed = gridcraft_model::python::rewrite_xl(&py.code, |reference| {
                let expr = gridcraft_formula::parse(reference).ok()?;
                let before = gridcraft_formula::print(&expr);
                let after = gridcraft_formula::print(&f(&host, expr));
                lost |= (after.contains("#REF!") && !before.contains("#REF!")) || area_shape(&before) != area_shape(&after);
                (after != before).then_some(after)
            });
            if let Some(code) = changed {
                check_python_fits(&gridcraft_model::PyCode { code: code.clone(), object: py.object, ..Default::default() }, &host, c)?;
                new_code.push((c, code, lost));
            }
        }
        if new_code.is_empty() {
            continue;
        }
        let Some(sh) = wb.sheet_mut(si) else { continue };
        for (c, code, lost) in new_code {
            if let Some(cell) = sh.cells.get_mut(c)
                && let Some(py) = cell.py.as_mut()
            {
                let py = std::sync::Arc::make_mut(py);
                py.code = code;
                if lost {
                    py.output = None;
                    cell.value = Value::Error(gridcraft_core::CellError::Blocked);
                }
            }
        }
    }
    Ok(())
}

/// The rows and columns a printed reference covers (`A1:B3`, `Sheet2!$A$1`, `C:E`), or `None`
/// for any other form (a name, an error).
fn area_shape(reference: &str) -> Option<(u32, u32)> {
    let r = RangeRef::parse(reference.rsplit('!').next()?)?;
    Some((r.height(), r.width()))
}

/// Refuses Python code an edit has made longer than one cell holds: the limit `python.set`
/// enforces and opening a file applies (code over it is dropped to its value on open). The edit
/// is refused rather than made, so no code is lost and none keeps a reference the edit moved.
pub(crate) fn check_python_fits(py: &gridcraft_model::PyCode, sheet: &str, at: CellRef) -> Result<()> {
    if py.fits() {
        return Ok(());
    }
    Err(EngineError::Other(format!(
        "The edit would make the Python code in {}!{} longer than Excel's {}-character formula limit, so nothing was changed. Shorten the code first.",
        gridcraft_formula::quote_sheet(sheet),
        at.a1(),
        gridcraft_model::python::MAX_FORMULA_CHARS
    )))
}
