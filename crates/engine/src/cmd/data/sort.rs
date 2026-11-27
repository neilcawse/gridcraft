use super::{
    Anchor, AnchorMode, Arc, Cell, CellRef, Comment, EngineError, Hyperlink, Json, LineInfo, Ordering, RangeRef, Result, Session, Sheet, Value, bad,
    bool_param, col_param, data_range, edit, guess_header, json, ok, sort_compare, split_sheet, str_param, target_sheet,
};
use crate::cmd::sheet::{changed_lines, drawing_placement_hits};
use gridcraft_core::MAX_ROWS;
use gridcraft_model::{DrawingRef, ObjKind, ObjRef, ObjectRect};

enum SortGeometry {
    /// In a group overlapping the sorted block: keeps its place on the sheet.
    Grouped {
        top: f64,
    },
    /// In a group clear of the block: left to follow the line changes by its mode.
    Follow,
    Refit {
        bottom: (u32, u32, f32, f32),
    },
}

#[derive(Clone)]
struct SortKey {
    col: u32,
    desc: bool,
    by: String,
    list: Option<Vec<String>>,
}

/// The sheet and range a sort works on: the sheet `sheet` or a `Sheet!` prefix on `range` names,
/// else the active one. The selection belongs to the active sheet, so another sheet needs `range`.
fn sort_target(s: &Session, p: &Json) -> Result<(usize, RangeRef)> {
    let sheet = target_sheet(s, p)?;
    let d = s.doc()?;
    if let Some(t) = str_param(p, "range")
        && let (Some(name), _) = split_sheet(t)
        && d.wb.sheet_index(&name) != Some(sheet)
    {
        return Err(bad("data.sort", format!("`{t}` names a sheet other than `sheet`")));
    }
    if sheet != d.wb.active_sheet() && p.get("range").is_none() {
        return Err(bad("data.sort", "missing `range`: the selection is on the active sheet"));
    }
    Ok((sheet, data_range(s, p)?))
}

/// The worksheet (index), range and header guess the Custom Sort dialog works on: the sheet named by
/// `sheet` or a `Sheet!` prefix on `range`, else the active one.
pub fn sort_context(s: &Session, p: &Json) -> Result<(usize, RangeRef, bool)> {
    let (sheet, explicit) = sort_target(s, p)?;
    let d = s.doc()?;
    let sh = d.wb.sheet(sheet).ok_or(EngineError::NoDocument)?;
    let sel = d.selection.current();
    // An explicit block wins over the table range, so a selected totals row can be sorted.
    let r = match sh.used_range() {
        Some(used) if p.get("range").is_none() && !sel.is_single() => sel.intersection(&used).unwrap_or(sel),
        _ => explicit,
    };
    Ok((sheet, r, bool_param(p, "header").unwrap_or_else(|| guess_header(&d.wb, sh, r))))
}

pub(super) fn quick_sort(s: &mut Session, p: &Json, asc: bool) -> Result<Json> {
    let (sheet, r) = sort_target(s, p)?;
    let col = col_param(p.get("column"), r.start.col).unwrap_or(s.doc()?.selection.active.col).clamp(r.start.col, r.end.col);
    let d = s.doc()?;
    let sh = d.wb.sheet(sheet).ok_or(EngineError::NoDocument)?;
    let header = guess_header(&d.wb, sh, r);
    do_sort(s, sheet, r, header, vec![SortKey { col, desc: !asc, by: "values".into(), list: None }], false, false)
}

pub(super) fn sort(s: &mut Session, p: &Json) -> Result<Json> {
    let (sheet, r) = sort_target(s, p)?;
    let d = s.doc()?;
    let sh = d.wb.sheet(sheet).ok_or(EngineError::NoDocument)?;
    let header = bool_param(p, "header").unwrap_or_else(|| guess_header(&d.wb, sh, r));
    let by_cols = str_param(p, "orientation") == Some("columns");
    let case = bool_param(p, "matchCase").unwrap_or(false);
    let mut keys: Vec<SortKey> = Vec::new();
    for k in p.get("keys").and_then(Json::as_array).map(Vec::as_slice).unwrap_or_default() {
        let col = if by_cols {
            let row = match k.get("row").and_then(Json::as_u64) {
                // Worksheet rows are 1-based; 0 (or a value past u32) has no coordinate.
                Some(n) => {
                    let row = u32::try_from(n).ok().and_then(|n| n.checked_sub(1));
                    Some(row.ok_or_else(|| bad("data.sort", "sort key row must be 1 or greater"))?)
                }
                None => None,
            };
            row.or_else(|| col_param(k.get("column"), r.start.row))
        } else {
            col_param(k.get("column"), r.start.col)
        };
        let Some(col) = col else { continue };
        keys.push(SortKey {
            col,
            desc: k.get("order").and_then(Json::as_str) == Some("desc"),
            by: k.get("by").and_then(Json::as_str).unwrap_or("values").to_string(),
            list: k.get("customList").and_then(Json::as_array).map(|l| l.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()),
        });
    }
    if keys.is_empty() {
        let (sheet, r, header) = sort_context(s, p)?;
        let name = s.doc()?.wb.sheet(sheet).map(|sh| sh.name.clone()).ok_or(EngineError::NoDocument)?;
        s.ui_requests.push(crate::UiRequest::Dialog("sort".into(), json!({"range": r.a1(), "header": header, "sheet": name})));
        return ok();
    }
    if keys.len() > 64 {
        return Err(bad("data.sort", "too many sort levels (64 max)"));
    }
    do_sort(s, sheet, r, header, keys, by_cols, case)
}

fn do_sort(s: &mut Session, sheet: usize, r: RangeRef, header: bool, keys: Vec<SortKey>, by_cols: bool, case: bool) -> Result<Json> {
    // Protect Sheet ▸ Sort. Both Sort… and the A-to-Z buttons come through here.
    crate::cmd::refuse_unless_allowed(s, sheet, "Sorting", |p| p.sort)?;
    let body = if by_cols {
        if header && r.width() > 1 { RangeRef::new(CellRef::new(r.start.row, r.start.col + 1), r.end) } else { r }
    } else if header && r.height() > 1 {
        RangeRef::new(CellRef::new(r.start.row + 1, r.start.col), r.end)
    } else {
        r
    };
    if body.count() > 20_000_000 {
        return Err(bad("data.sort", "range too large"));
    }
    edit(s, |cx| {
        let Some(sh) = cx.wb.sheet(sheet) else { return Ok(Json::Null) };
        if sh.merges.iter().any(|m| m.intersects(&body) && !m.is_single()) {
            return Err(EngineError::Other("To do this, all the merged cells need to be the same size.".into()));
        }
        let n = if by_cols { body.width() } else { body.height() };
        let line = |i: u32, k: u32| if by_cols { CellRef::new(k, body.start.col + i) } else { CellRef::new(body.start.row + i, k) };
        let theme = cx.wb.theme.clone();
        let styles = &cx.wb.styles;
        let key_val = |i: u32, k: &SortKey| -> (Value, u32) {
            let c = line(i, k.col);
            let v = sh.value(c);
            let color = match k.by.as_str() {
                "cellColor" => {
                    styles.get(sh.style_id(c)).fill.fg.resolve(&theme).map(|[r, g, b]| u32::from_be_bytes([0, r, g, b])).unwrap_or(u32::MAX)
                }
                "fontColor" => {
                    styles.get(sh.style_id(c)).font.color.resolve(&theme).map(|[r, g, b]| u32::from_be_bytes([0, r, g, b])).unwrap_or(u32::MAX)
                }
                _ => 0,
            };
            (v, color)
        };
        // Hidden lines on the sort axis keep their place: only the lines showing are sorted, among
        // the positions they occupy ("Sort data in a range or table": hidden rows are not moved
        // when you sort rows, nor hidden columns when you sort columns). A line shown with no
        // size counts as hidden: Excel for Mac left rows saved as `ht="0"` without `hidden` in
        // place (Batch 5 probe `x38-collapsed-sort-moves`).
        let shown: Vec<u32> =
            (0..n).filter(|&i| if by_cols { sh.col_width(body.start.col + i) > 0.0 } else { sh.row_height(body.start.row + i) > 0.0 }).collect();
        if shown.len() < 2 {
            return Ok(Json::Null);
        }
        let keyed: Vec<Vec<(Value, u32)>> = shown.iter().map(|&i| keys.iter().map(|k| key_val(i, k)).collect()).collect();
        let mut ranked: Vec<usize> = (0..shown.len()).collect();
        ranked.sort_by(|&a, &b| {
            for (ki, k) in keys.iter().enumerate() {
                let (va, ca) = &keyed[a][ki];
                let (vb, cb) = &keyed[b][ki];
                let plain = || {
                    if case && va.is_text() && vb.is_text() {
                        sort_compare(va, vb).then_with(|| case_order(va.as_text().unwrap_or(""), vb.as_text().unwrap_or("")))
                    } else {
                        sort_compare(va, vb)
                    }
                };
                let o = if k.by != "values" {
                    ca.cmp(cb)
                } else if let Some(list) = &k.list {
                    let pos = |v: &Value| v.as_text().and_then(|t| list.iter().position(|x| x.eq_ignore_ascii_case(t)));
                    match (pos(va), pos(vb)) {
                        (Some(a), Some(b)) => a.cmp(&b),
                        (Some(_), None) => Ordering::Less,
                        (None, Some(_)) => Ordering::Greater,
                        // Values not in the list follow the listed ones in the usual order, as in Excel.
                        // A descending key is left as it was: they tie, so the next key decides.
                        // A blank has no place in the list either, but still goes last.
                        (None, None) if k.desc => va.is_empty().cmp(&vb.is_empty()),
                        (None, None) => plain(),
                    }
                } else {
                    plain()
                };
                // Blanks always sort last, whatever the order.
                let o = if k.desc && !(va.is_empty() || vb.is_empty()) { o.reverse() } else { o };
                if o != Ordering::Equal {
                    return o;
                }
            }
            Ordering::Equal
        });
        // Only moved lines have cells to rewrite, as (destination, source).
        let moves: Vec<(u32, u32)> = shown.iter().copied().zip(ranked.into_iter().map(|rank| shown[rank])).filter(|&(dst, src)| dst != src).collect();
        if moves.is_empty() {
            return Ok(Json::Null);
        }
        // Keep the same bounding-box limit for the separate automatic row fit. In a row
        // sort, fit every shown body row (even if its sorted position is unchanged), but
        // never fit a row the permutation excluded. In a column sort the rows are lanes,
        // not the lines being sorted.
        cx.explicit_fit_rows = true;
        if body.count() <= 200_000 {
            if by_cols {
                cx.fit_rows.extend((body.start.row..=body.end.row).map(|row| (sheet, row)));
            } else {
                cx.fit_rows.extend(shown.into_iter().map(|i| (sheet, body.start.row + i)));
            }
        }
        // Move whole lines (cells with formulas adjusted for relative references).
        let lanes: Vec<u32> = if by_cols { (body.start.row..=body.end.row).collect() } else { (body.start.col..=body.end.col).collect() };
        // Notes and hyperlinks hang off a cell, not a row, so they travel with the row they
        // describe — the way cell pictures and row heights do. Collect them before inserting,
        // so a swap can't resurrect one from a destination that is also a source.
        let side: Vec<(CellRef, Comment)> = moves
            .iter()
            .flat_map(|&(dst_i, src_i)| lanes.iter().map(move |&k| (src_i, dst_i, k)))
            .filter_map(|(src_i, dst_i, k)| sh.comments.get(&line(src_i, k)).map(|cm| (line(dst_i, k), cm.clone())))
            .collect();
        let links: Vec<(CellRef, Hyperlink)> = moves
            .iter()
            .flat_map(|&(dst_i, src_i)| lanes.iter().map(move |&k| (src_i, dst_i, k)))
            .filter_map(|(src_i, dst_i, k)| sh.hyperlinks.get(&line(src_i, k)).map(|h| (line(dst_i, k), h.clone())))
            .collect();
        let mut moved: Vec<(CellRef, Option<Cell>)> = Vec::with_capacity(moves.len() * lanes.len());
        let mut pictures = Vec::new();
        for &(dst_i, src_i) in &moves {
            for &k in &lanes {
                let src = line(src_i, k);
                let dst = line(dst_i, k);
                let mut cell = sh.cell(src).cloned();
                if let Some(c) = cell.as_mut()
                    && let Some(f) = &c.formula
                    && let Some(e) = f.expr()
                {
                    let shifted = gridcraft_formula::adjust::shift_relative(e, dst.row as i64 - src.row as i64, dst.col as i64 - src.col as i64);
                    c.formula = Some(Arc::new(f.rewritten(shifted)));
                }
                if let Some(picture) = sh.cell_pictures.get(&src) {
                    pictures.push((dst, picture.clone()));
                }
                moved.push((dst, cell));
            }
        }
        // Row heights travel with rows. Both ends of a move are shown rows, so the moved
        // line's settings, visibility included, replace the destination's.
        let heights: Vec<(u32, Option<LineInfo>)> = if by_cols {
            vec![]
        } else {
            moves.iter().map(|&(dst, src)| (body.start.row + dst, sh.rows.get(&(body.start.row + src)).copied())).collect()
        };
        // Ungrouped floating objects anchored inside the sorted body travel with their lines,
        // unless pinned absolutely. The identity holds on every line not in `moves`.
        let dst_of: Vec<u32> = {
            let mut v: Vec<u32> = (0..n).collect();
            for &(dst, src) in &moves {
                v[src as usize] = dst;
            }
            v
        };
        // Read geometry before the row heights are permuted. Ungrouped objects may be refitted
        // to their new lines using their bottom-right corner. Groups overlapping the sorted block,
        // and their members, keep their sheet position, not their position relative to the
        // permuted rows; other groups follow the line changes like any drawing outside it.
        let (x0, y0) = (sh.col_left(body.start.col), sh.row_top(body.start.row));
        let block = ObjectRect { x: x0, y: y0, w: sh.col_left(body.end.col + 1) - x0, h: sh.row_top(body.end.row + 1) - y0 };
        let grouped = sh.grouped_objects(block);
        let objs: Vec<_> = sh
            .object_refs()
            .zip(sh.object_anchors().copied())
            .map(|(o, a)| {
                let top = sh.row_top(a.cell.row) + f64::from(a.dy);
                match grouped.get(&o) {
                    Some(true) => return (o, a, SortGeometry::Grouped { top }),
                    Some(false) => return (o, a, SortGeometry::Follow),
                    None => {}
                }
                let x2 = sh.col_left(a.cell.col) + a.dx as f64 + a.width.max(0.0) as f64;
                let y2 = top + f64::from(a.height.max(0.0));
                let tc = sh.col_at(x2);
                let tr = sh.row_at(y2);
                let bottom = (tc, tr, (x2 - sh.col_left(tc)) as f32, (y2 - sh.row_top(tr)) as f32);
                (o, a, SortGeometry::Refit { bottom })
            })
            .collect();
        let before = (!by_cols
            && sh.groups.iter().any(|g| {
                grouped.get(&ObjRef::new(ObjKind::Group, g.id)) == Some(&true)
                    && g.file.as_ref().is_some_and(|f| !f.edited)
                    && g.anchor.mode == AnchorMode::MoveAndSize
                    && (g.anchor.width == 0.0 || g.anchor.height == 0.0)
            }))
        .then(|| Arc::clone(&cx.wb.sheets[sheet]));
        let shm = cx.wb.sheet_mut(sheet).ok_or_else(|| EngineError::Other("no such sheet".into()))?;
        for (c, cell) in moved {
            match cell {
                Some(cell) => shm.set_cell(c, cell),
                None => {
                    shm.remove_cell(c);
                }
            }
            cx.changed.push((sheet, c));
        }
        // Every cell of a moved line is the destination of exactly one source cell, so clearing
        // those lines and inserting the permuted entries is the same permutation the cells went
        // through.
        for &(dst_i, _) in &moves {
            for &k in &lanes {
                shm.comments.remove(&line(dst_i, k));
                shm.hyperlinks.remove(&line(dst_i, k));
            }
        }
        for (at, cm) in side {
            shm.comments.insert(at, cm);
        }
        for (at, h) in links {
            shm.hyperlinks.insert(at, h);
        }
        shm.cell_pictures.extend(pictures);
        for (row, h) in heights {
            match h {
                Some(info) => {
                    shm.rows.insert(row, info);
                }
                None => {
                    if let Some(e) = shm.rows.get_mut(&row) {
                        e.size = None;
                        e.saved_size = None;
                    }
                }
            }
        }
        // Re-anchor floating objects on the sorted grid, before automatic row fitting.
        // Only objects anchored inside the sorted block move: within its lines on the sort axis
        // and within its span on the other axis (a picture beside the block stays put).
        let (b0, b1) = if by_cols { (body.start.col, body.end.col) } else { (body.start.row, body.end.row) };
        let (o0, o1) = if by_cols { (body.start.row, body.end.row) } else { (body.start.col, body.end.col) };
        let new_anchors: Vec<_> = objs
            .into_iter()
            .map(|(o, a, geometry)| {
                let anchor = match geometry {
                    SortGeometry::Grouped { top } => Some(fixed_sort_anchor(shm, a, top)),
                    SortGeometry::Follow => None,
                    SortGeometry::Refit { bottom } => {
                        let other = if by_cols { a.cell.row } else { a.cell.col };
                        if other < o0 || other > o1 { None } else { sorted_anchor(shm, a, bottom, by_cols, b0, b1, &dst_of) }
                    }
                };
                (o, anchor)
            })
            .collect();
        for (a, (_, n)) in shm.object_anchors_mut().zip(&new_anchors) {
            if let Some(n) = n {
                *a = *n;
            }
        }
        if let Some(before) = before {
            let changed = changed_lines(&before.rows, &shm.rows, (before.default_row_height, shm.default_row_height), MAX_ROWS);
            // `before` is only kept for a row sort. Rows outside the intersecting part of
            // the run stay put; within it, the permutation must send no row out of the run.
            // Hidden and zero-height rows never move, so a run can lose a row only when it
            // shows one, which takes a marker offset past its cell.
            let rows_stay = |start: u32, end: u32| {
                let lo = start.max(b0);
                let hi = end.min(b1 + 1);
                lo >= hi || dst_of[(lo - b0) as usize..(hi - b0) as usize].iter().all(|&d| (start..end).contains(&(b0 + d)))
            };
            for &(o, _) in &new_anchors {
                if o.kind == ObjKind::Group && grouped.get(&o) == Some(&true) {
                    let r = DrawingRef::Group(o.id);
                    let affected = before.drawing_files.get(&r).is_some_and(|file| drawing_placement_hits(&before, r, file, true, &changed));
                    shm.keep_sorted_group_collapse(&before, o.id, affected, &rows_stay);
                }
            }
        }
        // What the sort placed is on the lines it leaves: following the sorted row heights must
        // not move it again (though the row fit after the sort does).
        let uid = shm.uid.get();
        for (o, anchor) in new_anchors {
            if let Some(anchor) = anchor {
                shm.discard_stale_drawing_placement(o, anchor);
                cx.placed.push((uid, o));
            }
        }
        shm.discard_changed_drawing_files();
        cx.structural = true;
        Ok(Json::Null)
    })?;
    Ok(json!({"sorted": body.a1()}))
}

/// Preserve the sheet-space top of a grouped drawing over permuted row heights. Leave an
/// unchanged row origin alone, including for a stored marker with an out-of-cell offset.
fn fixed_sort_anchor(sh: &Sheet, mut anchor: Anchor, top: f64) -> Anchor {
    if (sh.row_top(anchor.cell.row) + f64::from(anchor.dy) - top).abs() > 1e-6 {
        let row = sh.row_at(top);
        anchor.cell.row = row;
        anchor.dy = (top - sh.row_top(row)) as f32;
    }
    anchor
}

/// Re-anchor one floating object for a sort; `None` when the sort leaves it where it is. `bottom`
/// is the object's bottom-right line and the offset of that corner inside it, measured before the
/// sort. `map` (via `dst_of`) translates a line index on the moving axis and is the identity
/// outside `b0..=b1`. `sh` is the post-sort grid (row heights already reordered), so re-fitting
/// uses the heights the object lands on.
/// Objects pinned absolutely ("Don't move or size with cells") never move; "move but don't size"
/// keeps its size; "move and size" is re-fitted when its whole extent stays inside the body, and
/// keeps its size when the sort turns its lines around (its bottom line now sorts before its top
/// line), since the lines then enclose nothing to stretch it between.
fn sorted_anchor(sh: &Sheet, a: Anchor, bottom: (u32, u32, f32, f32), by_cols: bool, b0: u32, b1: u32, dst_of: &[u32]) -> Option<Anchor> {
    if a.mode == AnchorMode::Absolute {
        return None;
    }
    let (tc, tr, tdx, tdy) = bottom;
    let map = |v: u32| -> u32 { if v >= b0 && v <= b1 { dst_of.get((v - b0) as usize).map_or(v, |&d| b0 + d) } else { v } };
    let (axis_start, axis_end) = if by_cols { (a.cell.col, tc) } else { (a.cell.row, tr) };
    if axis_start < b0 || axis_start > b1 {
        return None;
    }
    let (nr, nc) = if by_cols { (a.cell.row, map(a.cell.col)) } else { (map(a.cell.row), a.cell.col) };
    if a.mode == AnchorMode::MoveAndSize && axis_end >= b0 && axis_end <= b1 {
        let (nbr, nbc) = if by_cols { (tr, map(tc)) } else { (map(tr), tc) };
        let x1 = sh.col_left(nc) + a.dx as f64;
        let y1 = sh.row_top(nr) + a.dy as f64;
        let x2 = sh.col_left(nbc) + tdx as f64;
        let y2 = sh.row_top(nbr) + tdy as f64;
        // An extent that is not positive means the sort put the bottom-right line before the
        // top-left one on that axis: the object keeps its old size there.
        let fit = |extent: f64, old: f32| if extent > 0.0 { extent as f32 } else { old.max(0.0) };
        Some(Anchor { cell: CellRef::new(nr, nc), dx: a.dx, dy: a.dy, width: fit(x2 - x1, a.width), height: fit(y2 - y1, a.height), mode: a.mode })
    } else {
        Some(Anchor { cell: CellRef::new(nr, nc), ..a })
    }
}

/// Case-sensitive tie-break for text that is equal ignoring case: as in Excel, lowercase sorts
/// before uppercase at the first letter that differs in case.
fn case_order(a: &str, b: &str) -> Ordering {
    a.chars().zip(b.chars()).find(|(x, y)| x != y).map_or(Ordering::Equal, |(x, y)| y.is_lowercase().cmp(&x.is_lowercase()))
}

pub(super) fn sort_key(v: &Value) -> (u8, i64, String) {
    match v {
        Value::Number(n) => (0, (n * 1e6) as i64, String::new()),
        Value::Text(t) => (1, 0, t.to_lowercase()),
        Value::Bool(b) => (2, *b as i64, String::new()),
        Value::Empty => (9, 0, String::new()),
        _ => (3, 0, v.display()),
    }
}
