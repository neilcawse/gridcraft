use super::{
    Arc, CellRef, Ctx, EngineError, Json, Result, Session, Sheet, Visibility, bad, bool_param, edit, json, mark_changed_name_spans, ok,
    spans_changed, str_param, target_sheet,
};

pub(super) fn insert_sheet(s: &mut Session, p: &Json) -> Result<Json> {
    if s.doc()?.wb.protected_structure {
        return Err(EngineError::Other("The workbook is protected and cannot be changed.".into()));
    }
    let name = match str_param(p, "name") {
        Some(n) => n.to_string(),
        None => s.doc()?.wb.next_sheet_name(),
    };
    s.doc()?.wb.check_sheet_name(&name, None).map_err(EngineError::Other)?;
    let d = s.doc_mut()?;
    let before = p.get("before").and_then(Json::as_u64).map(|v| v as usize);
    let idx = edit_doc(d, |cx| {
        // Excel inserts before the active sheet; the "+" button appends after it.
        let at = before.unwrap_or(cx.wb.active_sheet() + 1).min(cx.wb.sheets.len());
        cx.wb.sheets.insert(at, Arc::new(Sheet::new(name.clone())));
        // Names scoped to later sheets shift.
        for n in cx.wb.names.iter_mut() {
            if let Some(sc) = n.scope.as_mut()
                && *sc >= at
            {
                *sc += 1;
            }
        }
        cx.wb.set_active_sheet(at);
        *cx.sel = crate::Selection::default();
        cx.structural = true;
        Ok(at)
    })?;
    d.sheet_selections.insert(idx.min(d.sheet_selections.len()), crate::Selection::default());
    Ok(json!({"index": idx, "name": name}))
}

fn edit_doc<R>(d: &mut crate::DocState, f: impl FnOnce(&mut Ctx) -> Result<R>) -> Result<R> {
    crate::cmd::commit(d, f)
}

pub(super) fn delete_sheet(s: &mut Session, p: &Json) -> Result<Json> {
    let i = target_sheet(s, p)?;
    let d = s.doc()?;
    if d.wb.protected_structure {
        return Err(EngineError::Other("The workbook is protected and cannot be changed.".into()));
    }
    if d.wb.sheets.iter().filter(|s| s.visibility == Visibility::Visible).count() <= 1 {
        return Err(EngineError::Other("A workbook must contain at least one visible worksheet.".into()));
    }
    let name = d.wb.sheet(i).map(|s| s.name.clone()).unwrap_or_default();
    let order: Vec<String> = d.wb.sheets.iter().map(|s| s.name.clone()).collect();
    let before = Arc::clone(&d.wb);
    let d = s.doc_mut()?;
    edit_doc(d, |cx| {
        cx.wb.sheets.remove(i);
        super::super::slicer::remove_orphaned_table_slicers(&before, &mut cx.wb);
        let new_order: Vec<String> = cx.wb.sheets.iter().map(|s| s.name.clone()).collect();
        for si in 0..cx.wb.sheets.len() {
            let keys: Vec<(CellRef, Arc<gridcraft_model::Formula>)> =
                cx.wb.sheets.get(si).map(|s| s.cells.iter().filter_map(|(c, x)| x.formula.clone().map(|f| (c, f))).collect()).unwrap_or_default();
            let Some(sh) = cx.wb.sheet_mut(si) else { continue };
            for (c, f) in keys {
                // Only formulas whose tree or 3-D span changes are rewritten or forgotten; the
                // others keep their text.
                let changed = f.with_expr(|e| {
                    let ne = gridcraft_formula::adjust::delete_sheet(e.clone(), &name, &order);
                    let diff = ne != *e;
                    (diff, diff || spans_changed(e, &order, &new_order), ne)
                });
                if let Some((tree_changed, reads_changed, ne)) = changed
                    && (tree_changed || reads_changed)
                    && let Some(cell) = sh.cells.get_mut(c)
                {
                    if reads_changed {
                        super::edit::forget_value(cell);
                        cx.changed.push((si, c));
                    }
                    if tree_changed {
                        cell.formula = Some(Arc::new(f.rewritten(ne)));
                    }
                }
            }
        }
        crate::cmd::edit::rewrite_python_refs(&mut cx.wb, &|_, e| gridcraft_formula::adjust::delete_sheet(e, &name, &order))?;
        // Chart series into the deleted sheet become #REF!, like formulas.
        crate::cmd::edit::rewrite_cell_links(&mut cx.wb, |_, e| gridcraft_formula::adjust::delete_sheet(e, &name, &order));
        cx.wb.names.retain(|n| n.scope != Some(i));
        for n in cx.wb.names.iter_mut() {
            if let Some(sc) = n.scope.as_mut()
                && *sc > i
            {
                *sc -= 1;
            }
            // Names referring to the sheet become #REF!.
            if let Ok(e) = gridcraft_formula::parse_name(&n.formula) {
                let ne = gridcraft_formula::adjust::delete_sheet(e.clone(), &name, &order);
                if ne != e {
                    n.formula = gridcraft_formula::print(&ne);
                }
            }
        }
        mark_changed_name_spans(cx, &order, &new_order);
        if cx.wb.active_sheet() >= cx.wb.sheets.len() || cx.wb.active_sheet() > i {
            cx.wb.set_active_sheet(cx.wb.active_sheet().saturating_sub(1).min(cx.wb.sheets.len().saturating_sub(1)));
        }
        *cx.sel = crate::Selection::default();
        cx.structural = true;
        Ok(())
    })?;
    if i < d.sheet_selections.len() {
        d.sheet_selections.remove(i);
    }
    ok()
}

pub(super) fn rename_sheet(s: &mut Session, p: &Json) -> Result<Json> {
    if s.doc()?.wb.protected_structure {
        return Err(EngineError::Other("The workbook is protected and cannot be changed.".into()));
    }
    let i = target_sheet(s, p)?;
    let Some(name) = str_param(p, "name").map(|n| n.trim().to_string()) else { return Err(bad("sheet.rename", "missing `name`")) };
    s.doc()?.wb.check_sheet_name(&name, Some(i)).map_err(EngineError::Other)?;
    edit(s, |cx| {
        let old = cx.wb.sheet(i).map(|s| s.name.clone()).unwrap_or_default();
        cx.sheet_mut(i)?.name = name.clone();
        for si in 0..cx.wb.sheets.len() {
            let keys: Vec<(CellRef, Arc<gridcraft_model::Formula>)> =
                cx.wb.sheets.get(si).map(|s| s.cells.iter().filter_map(|(c, x)| x.formula.clone().map(|f| (c, f))).collect()).unwrap_or_default();
            let Some(sh) = cx.wb.sheet_mut(si) else { continue };
            for (c, f) in keys {
                // Compares trees, so unchanged formulas are not printed; they keep their text.
                let changed = f.with_expr(|e| Some(gridcraft_formula::adjust::rename_sheet(e.clone(), &old, &name)).filter(|ne| ne != e));
                if let Some(Some(ne)) = changed
                    && let Some(cell) = sh.cells.get_mut(c)
                {
                    cell.formula = Some(Arc::new(f.rewritten(ne)));
                }
            }
            for ch in sh.charts.iter_mut() {
                // A PivotChart follows its PivotTable's sheet.
                if let Some((sheet, pivot)) = ch.pivot.as_deref().and_then(crate::pivot::parse_chart_link)
                    && sheet.eq_ignore_ascii_case(&old)
                {
                    ch.pivot = Some(crate::pivot::chart_link(&name, &pivot));
                }
            }
            // So does a slicer.
            for sl in sh.slicers.iter_mut() {
                if let gridcraft_model::SlicerSource::Pivot { pivots, .. } = &mut sl.source {
                    for p in pivots.iter_mut().filter(|p| p.sheet.eq_ignore_ascii_case(&old)) {
                        p.sheet = name.clone();
                    }
                }
            }
            // A PivotTable's source names the sheet (`Data!$A$1:$C$9`, `Data!rData`): it follows,
            // still bound to what it was.
            for pt in sh.pivots.iter_mut() {
                let (sheet, body) = gridcraft_core::split_sheet(&pt.source);
                if sheet.is_some_and(|s| s.eq_ignore_ascii_case(&old)) {
                    let source = format!("{}!{body}", gridcraft_core::quote_sheet(&name));
                    if let Some(b) = pt.binding.0.as_mut().filter(|b| b.text == pt.source) {
                        b.text.clone_from(&source);
                    }
                    pt.source = source;
                }
            }
        }
        crate::cmd::edit::rewrite_cell_links(&mut cx.wb, |_, e| gridcraft_formula::adjust::rename_sheet(e, &old, &name));
        for n in cx.wb.names.iter_mut() {
            if let Ok(e) = gridcraft_formula::parse_name(&n.formula) {
                // The same cells, under the sheet's new name.
                cx.moved_names.push(n.name.to_lowercase());
                n.formula = gridcraft_formula::print(&gridcraft_formula::adjust::rename_sheet(e, &old, &name));
            }
        }
        crate::cmd::edit::rewrite_python_refs(&mut cx.wb, &|_, e| gridcraft_formula::adjust::rename_sheet(e, &old, &name))?;
        cx.structural = true;
        Ok(Json::Null)
    })
}

pub(super) fn move_sheet(s: &mut Session, p: &Json) -> Result<Json> {
    if s.doc()?.wb.protected_structure {
        return Err(EngineError::Other("The workbook is protected and cannot be changed.".into()));
    }
    let i = target_sheet(s, p)?;
    let copy = bool_param(p, "copy").unwrap_or(false);
    let to = p.get("to").and_then(Json::as_u64).map(|v| v as usize).ok_or_else(|| bad("sheet.move", "missing `to`"))?;
    let old_order: Vec<String> = s.doc()?.wb.sheets.iter().map(|sh| sh.name.clone()).collect();
    edit(s, |cx| {
        let n = cx.wb.sheets.len();
        let Some(src) = cx.wb.sheets.get(i).cloned() else { return Err(bad("sheet.move", "no such sheet")) };
        if copy {
            let mut sh = (*src).clone();
            // The copy is a sheet of its own, not the one it was copied from.
            sh.uid = gridcraft_model::SheetUid::fresh();
            let base = sh.name.clone();
            sh.name = copied_sheet_name(&cx.wb, &base);
            // The tables come along under names of their own (table names are unique).
            let renames = copied_table_names(&cx.wb, &sh.tables);
            sh.preserved = None; // kept file content (form controls, OLE objects…) stays with the original
            // Object ids are unique across the workbook (selection and the `{kind, id}` commands
            // rely on it), so the copied objects get fresh ones (free ones, even when a file used
            // the largest id).
            let count = sh.charts.len() + sh.images.len() + sh.shapes.len() + sh.tables.len() + sh.slicers.len() + sh.controls.len();
            let ids = cx.wb.free_object_ids(count);
            if ids.len() < count {
                return Err(bad("sheet.move", "no free object ids are left for the copied objects"));
            }
            let mut ids = ids.into_iter();
            let mut fresh = || ids.next().unwrap_or(u32::MAX);
            sh.charts.iter_mut().for_each(|o| o.id = fresh());
            sh.images.iter_mut().for_each(|o| o.id = fresh());
            sh.shapes.iter_mut().for_each(|o| o.id = fresh());
            // Tables of their own: nothing bound to the original's (a PivotTable's source) is bound to them.
            let mut retargets = Vec::with_capacity(renames.len());
            for (t, name) in sh.tables.iter_mut().zip(renames) {
                t.id = fresh();
                t.uid = Default::default();
                retargets.push((std::mem::replace(&mut t.name, name), t.name.clone()));
            }
            // A copied slicer's cache is new: the imported cache name and marker stay the original's.
            sh.slicers.iter_mut().for_each(|o| {
                o.id = fresh();
                o.imported_cache = Default::default();
            });
            sh.controls.iter_mut().for_each(|o| o.id = fresh());
            let at = to.min(n);
            // The copy gets its own copies of the sheet's names, referring to the copy, and of
            // the workbook's names that refer to the sheet (`Data (2)!Global` beside workbook
            // `Global`), unless a name of the sheet shadows them there.
            let local_copy = |nm: &gridcraft_model::DefinedName, formula: String| gridcraft_model::DefinedName {
                name: nm.name.clone(),
                scope: Some(at),
                formula,
                comment: nm.comment.clone(),
                hidden: nm.hidden,
                // Nothing bound to the original's name is bound to its copy.
                uid: Default::default(),
            };
            let rename = |e| gridcraft_formula::adjust::rename_sheet(e, &base, &sh.name);
            let mut seen = std::collections::HashSet::new();
            let mut copies = Vec::new();
            // Source locals take priority regardless of where they occur in the workbook's list.
            // Readers can retain duplicate definitions; as in Workbook::name, the first wins.
            for nm in cx.wb.names.iter().filter(|nm| nm.scope == Some(i)) {
                if seen.insert(nm.name.to_ascii_lowercase()) {
                    let formula = gridcraft_formula::parse_name(&nm.formula)
                        .map(|e| gridcraft_formula::print(&rename(e)))
                        .unwrap_or_else(|_| nm.formula.clone());
                    copies.push(local_copy(nm, formula));
                }
            }
            for nm in cx.wb.names.iter().filter(|nm| nm.scope.is_none()) {
                if nm.name.get(..3).is_some_and(|p| p.eq_ignore_ascii_case("_xl")) || !seen.insert(nm.name.to_ascii_lowercase()) {
                    continue;
                }
                if let Ok(e) = gridcraft_formula::parse_name(&nm.formula)
                    && refers_to_copied_sheet(&e, &base, &retargets)
                {
                    copies.push(local_copy(nm, gridcraft_formula::print(&rename(e))));
                }
            }
            for nm in cx.wb.names.iter_mut() {
                if let Some(sc) = nm.scope.as_mut()
                    && *sc >= at
                {
                    *sc += 1;
                }
            }
            cx.wb.names.extend(copies);
            cx.wb.sheets.insert(at, Arc::new(sh));
            cx.wb.set_active_sheet(at);
            // The copy's own references, sheet-level names and slicers use its tables.
            for (old, new) in &retargets {
                cx.wb.retarget_table_on_sheet(at, old, new);
            }
        } else {
            let sh = cx.wb.sheets.remove(i);
            let at = to.min(n - 1);
            cx.wb.sheets.insert(at, sh);
            cx.wb.set_active_sheet(at);
            // Sheet-level names follow their sheet.
            for nm in cx.wb.names.iter_mut() {
                if let Some(sc) = nm.scope.as_mut() {
                    *sc = match *sc {
                        x if x == i => at,
                        x if i < x && x <= at => x - 1,
                        x if at <= x && x < i => x + 1,
                        x => x,
                    };
                }
            }
        }
        let new_order: Vec<String> = cx.wb.sheets.iter().map(|sh| sh.name.clone()).collect();
        for si in 0..cx.wb.sheets.len() {
            let keys: Vec<(CellRef, Arc<gridcraft_model::Formula>)> =
                cx.wb.sheets.get(si).map(|sh| sh.cells.iter().filter_map(|(c, x)| x.formula.clone().map(|f| (c, f))).collect()).unwrap_or_default();
            let Some(sh) = cx.wb.sheet_mut(si) else { continue };
            for (c, f) in keys {
                if let Some(e) = f.expr_arc()
                    && spans_changed(&e, &old_order, &new_order)
                    && let Some(cell) = sh.cells.get_mut(c)
                {
                    super::edit::forget_value(cell);
                    cx.changed.push((si, c));
                }
            }
        }
        mark_changed_name_spans(cx, &old_order, &new_order);
        cx.structural = true;
        Ok(Json::Null)
    })
}

/// The tab name of a copy of sheet `source`: `Data (2)`, or the lowest free number after it. A
/// source already numbered like a copy counts from its base, so with `Data`, `Data (2)` and
/// `Data (3)` taken, a copy of `Data (2)` is `Data (4)` (Excel for Mac, X23 probe P1). The base is
/// shortened so the suffix fits in Excel's 31 characters.
fn copied_sheet_name(wb: &gridcraft_model::Workbook, source: &str) -> String {
    use std::fmt::Write;

    let base = copy_base(source);
    let mut candidate = String::new();
    for k in 2usize.. {
        let keep = 31 - (k.ilog10() as usize + 4); // ` (` + the digits + `)`
        let end = base.char_indices().nth(keep).map_or(base.len(), |(i, _)| i);
        let prefix = base[..end].trim_end();
        candidate.clear();
        candidate.push_str(if prefix.is_empty() { "Sheet" } else { prefix });
        let tail = candidate.len();
        let _ = write!(candidate, " ({k})");
        let collides = |name: &str| name.ends_with(&candidate[tail..]) && gridcraft_formula::same_name(name, &candidate);
        // Kept chart, dialog and macro tabs are absent from `wb.sheets`. A worksheet with the
        // same name would displace the kept tab on export.
        let kept_collision = wb.preserved.as_deref().is_some_and(|p| {
            p.sheets.iter().any(|s| {
                let end = s.name.char_indices().nth(31).map_or(s.name.len(), |(i, _)| i);
                collides(&s.name[..end])
            })
        });
        if !wb.sheets.iter().any(|sh| collides(&sh.name)) && !kept_collision {
            break;
        }
    }
    candidate
}

/// `name` without a trailing copy number ` (n)`, n decimal digits; `name` itself otherwise.
fn copy_base(name: &str) -> &str {
    name.strip_suffix(')')
        .and_then(|s| s.rsplit_once(" ("))
        .filter(|(base, n)| !base.trim().is_empty() && !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        .map_or(name, |(base, _)| base)
}

/// Whether a workbook name explicitly uses the source sheet or one of its tables. Match the
/// sheet selectors rewritten by `rename_sheet`, and the structured tokens retargeted by X23.
fn refers_to_copied_sheet(e: &gridcraft_formula::Expr, source: &str, retargets: &[(String, String)]) -> bool {
    use gridcraft_formula::{Expr, SheetSel};
    let matches_source = |name: &str| name.eq_ignore_ascii_case(source);
    let mut found = false;
    let uses_source = |x: &Expr| match x {
        Expr::Ref(r) => match &r.sheet {
            SheetSel::Named(n) => matches_source(n),
            SheetSel::Span(a, b) if !a.starts_with('[') => matches_source(a) || matches_source(b),
            _ => false,
        },
        Expr::Name(n) => n.rsplit_once('!').is_some_and(|(sheet, _)| matches_source(sheet)),
        Expr::Struct(r) => retargets.iter().any(|(old, _)| r.table.eq_ignore_ascii_case(old)),
        _ => false,
    };
    e.walk(&mut |x| found = found || uses_source(x));
    found
}

/// Names for the copies of `tables` (the tables of a sheet being copied), in order.
///
/// In two Excel-saved public files, copied names append one more than the copy's saved table
/// id: `Table6` became `Table69` (id 8) and `Table610` (id 9), and `Table3` became `Table38`
/// (id 7). GridCraft doesn't retain Excel's table ids and writes them in sheet order, so the
/// suffix starts at `N + k + 1` for `N` existing tables and the `k`th copy (from 1).
///
/// The synthetic Excel for Mac probes confirm this order for successive multi-table copies and
/// a skip over a taken defined name. Gapped ids and multi-table collisions remain unmeasured.
fn copied_table_names(wb: &gridcraft_model::Workbook, tables: &[gridcraft_model::Table]) -> Vec<String> {
    if tables.is_empty() {
        return Vec::new();
    }
    use gridcraft_core::valid_table_name;
    use gridcraft_formula::fold_name;
    let reserved = reserved_table_names(wb);
    // Names given to earlier tables of this copy.
    let mut allocated = std::collections::HashSet::new();
    let mut next = wb.sheets.iter().map(|s| s.tables.len()).sum::<usize>() + 1;
    tables
        .iter()
        .map(|t| {
            next += 1;
            let base = if valid_table_name(&t.name) { t.name.as_str() } else { "Table" };
            let normalized_r = base.eq_ignore_ascii_case("R_");
            let base_len = base.chars().count();
            let fit = |tail: String| {
                // P4: a full-length source was shortened to 254 characters including the suffix.
                let keep = if base_len + tail.len() > 255 { 254 - tail.len() } else { base_len };
                let mut name: String = base.chars().take(keep).collect();
                name.push_str(&tail);
                name
            };
            (next..)
                .map(|n| {
                    // P3: Excel's formula-visible name R_ copied as R__3, not R_3.
                    if !normalized_r {
                        let name = fit(n.to_string());
                        if valid_table_name(&name) {
                            return name;
                        }
                    }
                    fit(format!("_{n}"))
                })
                .find(|name| {
                    let folded = fold_name(name);
                    !reserved.contains(&folded) && allocated.insert(folded)
                })
                .unwrap_or_default()
        })
        .collect()
}

/// The spellings, folded, that a copied table can't take: the namespace tables share with defined
/// names holds every table, every defined name of any scope (those of kept sheets too) and the
/// name of an imported slicer cache, written as a workbook name when the file had no marker for it.
/// After X27 merges, use `!wb.table_name_taken(name, None)` for the workbook check (pass the
/// original spelling), drop this helper, and keep the folded `allocated` check for this copy.
/// X27 additionally reserves the `_xlnm.` and `_xlchart.` prefixes.
fn reserved_table_names(wb: &gridcraft_model::Workbook) -> std::collections::HashSet<String> {
    use gridcraft_formula::fold_name;
    let tables = wb.sheets.iter().flat_map(|s| &s.tables).map(|t| fold_name(&t.name));
    let names = wb.names.iter().map(|n| fold_name(&n.name));
    let kept = gridcraft_xlsx::kept_defined_names(wb).map(|n| fold_name(&n));
    let caches = wb.sheets.iter().flat_map(|s| &s.slicers).map(|s| &s.imported_cache.name).filter(|n| !n.is_empty()).map(|n| fold_name(n));
    tables.chain(names).chain(kept).chain(caches).collect()
}

pub(super) fn tab_color(s: &mut Session, p: &Json) -> Result<Json> {
    let i = target_sheet(s, p)?;
    let c = crate::cmd::format::color_param(p.get("color"));
    edit(s, |cx| {
        cx.sheet_mut(i)?.tab_color = c.filter(|c| *c != gridcraft_model::Color::Auto);
        Ok(Json::Null)
    })
}

pub(super) fn hide_sheet(s: &mut Session, p: &Json) -> Result<Json> {
    if s.doc()?.wb.protected_structure {
        return Err(EngineError::Other("The workbook is protected and cannot be changed.".into()));
    }
    let i = target_sheet(s, p)?;
    if s.doc()?.wb.sheets.iter().filter(|s| s.visibility == Visibility::Visible).count() <= 1 {
        return Err(EngineError::Other("A workbook must contain at least one visible worksheet.".into()));
    }
    edit(s, |cx| {
        cx.sheet_mut(i)?.visibility = Visibility::Hidden;
        if cx.wb.active_sheet() == i {
            cx.wb.set_active_sheet(cx.wb.sheets.iter().position(|s| s.visibility == Visibility::Visible).unwrap_or(0));
        }
        Ok(Json::Null)
    })
}

pub(super) fn unhide_sheet(s: &mut Session, p: &Json) -> Result<Json> {
    if s.doc()?.wb.protected_structure {
        return Err(EngineError::Other("The workbook is protected and cannot be changed.".into()));
    }
    if p.get("sheet").is_none() {
        let hidden: Vec<String> = s.doc()?.wb.sheets.iter().filter(|s| s.visibility != Visibility::Visible).map(|s| s.name.clone()).collect();
        if hidden.is_empty() {
            return Err(EngineError::Other("No sheets are hidden.".into()));
        }
        s.ui_requests.push(crate::UiRequest::Dialog("unhideSheet".into(), json!({"sheets": hidden})));
        return ok();
    }
    let i = target_sheet(s, p)?;
    edit(s, |cx| {
        cx.sheet_mut(i)?.visibility = Visibility::Visible;
        cx.wb.set_active_sheet(i);
        Ok(Json::Null)
    })
}

pub(super) fn activate(s: &mut Session, p: &Json) -> Result<Json> {
    let i = target_sheet(s, p)?;
    let d = s.doc_mut()?;
    d.wb_switch_sheet(i);
    Ok(json!({"sheet": i, "name": d.wb.active().map(|s| s.name.clone())}))
}

pub(super) fn step_sheet(s: &mut Session, dir: isize) -> Result<Json> {
    let d = s.doc()?;
    let mut i = d.wb.active_sheet();
    while let Some(next) = i.checked_add_signed(dir) {
        let Some(sheet) = d.wb.sheet(next) else { break };
        if sheet.visibility == Visibility::Visible {
            s.doc_mut()?.wb_switch_sheet(next);
            return ok();
        }
        i = next;
    }
    // No visible sheet in this direction: keep the current sheet and its selection.
    ok()
}
