use super::sort::sort_key;
use super::{
    AutoFilter, CellRef, EngineError, FilterCriterion, Json, Ordering, RangeRef, Result, Session, Sheet, Value, Workbook, bad, bool_param, col_param,
    current_region, data_target, edit, json, str_param,
};

// ---------------------------------------------------------------- filter

pub(super) fn toggle_filter(s: &mut Session, p: &Json) -> Result<Json> {
    let (sheet, r) = data_target(s, p, "data.filter")?;
    crate::cmd::refuse_unless_allowed(s, sheet, "Filtering", |p| p.autofilter)?;
    // The active cell belongs to the active sheet: on another sheet the table is the one at `range`.
    let at = if sheet == s.doc()?.wb.active_sheet() { s.doc()?.selection.active } else { r.start };
    let has = s.doc()?.wb.sheet(sheet).is_some_and(|sh| sh.autofilter.is_some());
    let in_table = s.doc()?.wb.sheet(sheet).and_then(|sh| sh.table_at(at).map(|t| t.name.clone()));
    if let Some(name) = in_table {
        return edit(s, |cx| {
            let sh = cx.sheet_mut(sheet)?;
            let Some(i) = sh.tables.iter().position(|t| t.name == name) else { return Ok(Json::Null) };
            let t = &mut sh.tables[i];
            t.filter_button = !t.filter_button;
            if !t.filter_button && clear_table_filter(sh, i) {
                apply_filters(&mut cx.wb, sheet, Some(i), false);
                cx.structural = true;
            }
            Ok(Json::Null)
        });
    }
    if has {
        return edit(s, |cx| {
            let sh = cx.sheet_mut(sheet)?;
            // Only this worksheet filter is being removed. Other filters remain active.
            if let Some(af) = &mut sh.autofilter
                && !af.criteria.is_empty()
            {
                af.criteria.clear();
                apply_filters(&mut cx.wb, sheet, None, true);
            }
            cx.sheet_mut(sheet)?.autofilter = None;
            cx.structural = true;
            Ok(json!({"filter": false}))
        });
    }
    install_filter(s, p)
}

/// Turn a sheet AutoFilter on over `p.range`, or over the data range of the selection.
fn install_filter(s: &mut Session, p: &Json) -> Result<Json> {
    let (sheet, mut r) = data_target(s, p, "data.filter")?;
    // A selected header row filters the data below it (#201).
    if p.get("range").is_none() && r.height() == 1 {
        let d = s.doc()?;
        let sh = d.wb.active().ok_or(EngineError::NoDocument)?;
        let below = current_region(sh, r.start).end.row;
        if below > r.end.row {
            r = RangeRef::new(r.start, CellRef::new(below, r.end.col));
        }
    }
    edit(s, |cx| {
        cx.sheet_mut(sheet)?.autofilter = Some(AutoFilter { range: r, criteria: vec![] });
        Ok(json!({"filter": true, "range": r.a1()}))
    })
}

pub(super) fn filter_by(s: &mut Session, p: &Json) -> Result<Json> {
    let sheet = s.doc()?.wb.active_sheet();
    crate::cmd::refuse_unless_allowed(s, sheet, "Filtering", |p| p.autofilter)?;
    let Some(sh) = s.doc()?.wb.sheet(sheet) else { return Err(EngineError::NoDocument) };
    // `table` names the table whose own filter is changed (its header buttons); without it the
    // worksheet AutoFilter is, turned on over the selection's region when there is none.
    let table = match str_param(p, "table") {
        Some(name) => Some(
            sh.tables
                .iter()
                .position(|t| t.name.eq_ignore_ascii_case(name) && t.filter_button)
                .ok_or_else(|| bad("data.filterBy", format!("no table named `{name}` with filter buttons on the active sheet")))?,
        ),
        None => None,
    };
    let range = match (table.and_then(|i| sh.tables.get(i)), &sh.autofilter) {
        (Some(t), _) => t.range,
        (None, Some(a)) => a.range,
        (None, None) => {
            install_filter(s, &json!({}))?;
            s.doc()?.wb.sheet(sheet).and_then(|sh| sh.autofilter.as_ref()).map(|af| af.range).ok_or_else(|| EngineError::Other("no filter".into()))?
        }
    };
    let col = match col_param(p.get("column"), range.start.col) {
        Some(c) => c,
        // A number too large to be a column is outside the filter range, not a missing parameter.
        None if p.get("column").is_some_and(|v| v.as_u64().is_some()) => return Err(bad("data.filterBy", "column outside the filter range")),
        None => return Err(bad("data.filterBy", "missing `column`")),
    };
    let Some(off) = col.checked_sub(range.start.col).filter(|_| col <= range.end.col) else {
        return Err(bad("data.filterBy", "column outside the filter range"));
    };
    let crit = if bool_param(p, "clear").unwrap_or(false) {
        None
    } else if let Some(vals) = p.get("values").and_then(Json::as_array) {
        Some(FilterCriterion::Values {
            values: vals.iter().map(crate::cmd::edit::json_to_input).collect(),
            blanks: bool_param(p, "blanks").unwrap_or(false),
        })
    } else if let Some(c) = p.get("custom") {
        let a =
            (c.get("op").and_then(Json::as_str).unwrap_or("=").to_string(), c.get("value").map(crate::cmd::edit::json_to_input).unwrap_or_default());
        let b =
            c.get("op2").and_then(Json::as_str).map(|o| (o.to_string(), c.get("value2").map(crate::cmd::edit::json_to_input).unwrap_or_default()));
        Some(FilterCriterion::Custom { a, b, and: c.get("and").and_then(Json::as_bool).unwrap_or(true) })
    } else if let Some(t) = p.get("top") {
        Some(FilterCriterion::Top10 {
            bottom: t.get("bottom").and_then(Json::as_bool).unwrap_or(false),
            percent: t.get("percent").and_then(Json::as_bool).unwrap_or(false),
            count: t.get("count").and_then(Json::as_u64).unwrap_or(10) as u32,
        })
    } else {
        bool_param(p, "aboveAverage").map(FilterCriterion::AboveAverage)
    };
    edit(s, |cx| {
        let sh = cx.sheet_mut(sheet)?;
        let native = table.or_else(|| sh.tables.iter().position(|t| !t.filter.is_empty() && t.matches_autofilter(range)));
        let criteria = if let Some(i) = native {
            sh.adopt_table_autofilter(i);
            &mut sh.tables.get_mut(i).ok_or_else(|| EngineError::Other("no table".into()))?.filter
        } else {
            &mut sh.autofilter.as_mut().ok_or_else(|| EngineError::Other("no filter".into()))?.criteria
        };
        criteria.retain(|(o, _)| *o != off);
        if let Some(c) = crit {
            criteria.push((off, c));
        }
        apply_filters(&mut cx.wb, sheet, native, native.is_none());
        cx.structural = true;
        Ok(Json::Null)
    })?;
    let hidden = s.doc()?.wb.sheet(sheet).map(|sh| sh.rows.values().filter(|r| r.hidden).count()).unwrap_or(0);
    Ok(json!({"hiddenRows": hidden}))
}

/// Does a value pass a custom comparison (`>`, `=*abc*`, `<>x`…)?
fn custom_ok(v: &Value, text: &str, op: &str, crit: &str) -> bool {
    let num = gridcraft_core::parse::parse_number_text(crit);
    let cmp = match (v.as_f64(), num) {
        (Some(a), Some(b)) => a.partial_cmp(&b),
        // A numeric ordering comparison only matches numbers; blanks and text never do.
        (None, Some(_)) if matches!(op, "<" | "<=" | ">" | ">=") => None,
        _ => Some(gridcraft_core::compare_text(text, crit)),
    };
    let wild = || crate::cmd::edit::wildcard(&text.to_lowercase(), &crit.to_lowercase());
    match op {
        "=" => {
            if crit.contains(['*', '?']) {
                wild()
            } else {
                cmp == Some(Ordering::Equal)
            }
        }
        "<>" => {
            if crit.contains(['*', '?']) {
                !wild()
            } else {
                cmp != Some(Ordering::Equal)
            }
        }
        ">" => cmp == Some(Ordering::Greater),
        "<" => cmp == Some(Ordering::Less),
        ">=" => matches!(cmp, Some(Ordering::Greater | Ordering::Equal)),
        "<=" => matches!(cmp, Some(Ordering::Less | Ordering::Equal)),
        "beginsWith" => text.to_lowercase().starts_with(&crit.to_lowercase()),
        "endsWith" => text.to_lowercase().ends_with(&crit.to_lowercase()),
        "contains" => text.to_lowercase().contains(&crit.to_lowercase()),
        "notContains" => !text.to_lowercase().contains(&crit.to_lowercase()),
        _ => true,
    }
}

/// The filter's "(Blanks)" group: truly empty cells and zero-length text such as a `=""` result.
fn filter_blank(v: &Value) -> bool {
    v.is_empty() || matches!(v, Value::Text(t) if t.is_empty())
}

/// Excel grows an AutoFilter range when a value is entered in the row right below it, in one of its columns (#2342).
/// A table's own filter is left to the table, and a row typed after a blank row does not count.
/// A formula counts before it is calculated: this runs inside the edit, ahead of the recalc.
pub(crate) fn grow_autofilter(sh: &mut Sheet, at: CellRef) {
    // The AutoFilter checks come first: they are cheap, and most sheets have none.
    let Some(af) = sh.autofilter.as_ref() else { return };
    let r = af.range;
    if r.end.row.checked_add(1) != Some(at.row) || at.col < r.start.col || at.col > r.end.col || sh.tables.iter().any(|t| t.matches_autofilter(r)) {
        return;
    }
    if !sh.cells.get(at).is_some_and(|c| !filter_blank(&c.value) || c.formula.is_some()) {
        return;
    }
    let grown = RangeRef::new(r.start, CellRef::new(at.row, r.end.col));
    if let Some(af) = sh.autofilter.as_mut() {
        af.range = grown;
    }
}

/// Clear a table's criteria and any worksheet proxy, returning whether visibility must be recalculated.
pub(crate) fn clear_table_filter(sh: &mut Sheet, table: usize) -> bool {
    let t = &mut sh.tables[table];
    let was_active = !t.filter.is_empty();
    t.filter.clear();
    let proxy = sh.autofilter.take_if(|af| t.matches_autofilter(af.range));
    was_active || proxy.is_some_and(|af| !af.criteria.is_empty())
}

/// Apply worksheet and table criteria. Where two ranges cover the same row, either can hide it.
/// Re-evaluate a changed scope even when its last criterion was removed.
pub(crate) fn apply_filters(wb: &mut Workbook, sheet: usize, changed_table: Option<usize>, changed_sheet: bool) {
    let Some(sh) = wb.sheet(sheet) else { return };
    let mut rows = Vec::new();
    if let Some(af) = &sh.autofilter
        && (changed_sheet || !af.criteria.is_empty())
    {
        let r = sh.effective_autofilter_range(af.range);
        filter_rows(wb, sh, r, r.start.row + 1, &af.criteria, &mut rows);
    }
    for (i, t) in sh.tables.iter().enumerate() {
        if (changed_table == Some(i) || !t.filter.is_empty())
            && let Some(data) = t.data_range()
        {
            filter_rows(wb, sh, data, data.start.row, &t.filter, &mut rows);
        }
    }
    if rows.is_empty() {
        return;
    }
    let Some(sh) = wb.sheet_mut(sheet) else { return };
    // Apply reveals first and hides second, so intersecting filters combine without depending on order.
    for &(row, hidden) in &rows {
        if !hidden && let Some(info) = sh.rows.get_mut(&row) {
            info.hidden = false;
        }
    }
    for (row, hidden) in rows {
        if hidden {
            sh.rows.entry(row).or_default().hidden = true;
        }
    }
}

fn filter_rows(wb: &Workbook, sh: &Sheet, r: RangeRef, first: u32, criteria: &[(u32, FilterCriterion)], hide: &mut Vec<(u32, bool)>) {
    // An imported range can name rows past the grid; none of those exist to hide.
    let rows = first..=r.end.row.min(gridcraft_core::MAX_ROWS - 1);
    // Precompute per-criterion top10 threshold and average once, not per row.
    let mut col_stats: std::collections::HashMap<usize, (Option<f64>, f64)> = std::collections::HashMap::new();
    for (i, (off, c)) in criteria.iter().enumerate() {
        if matches!(c, FilterCriterion::Top10 { .. } | FilterCriterion::AboveAverage(_)) {
            let col = r.start.col + off;
            let mut nums: Vec<f64> = rows.clone().filter_map(|row| sh.value(CellRef::new(row, col)).as_f64()).collect();
            let avg = if nums.is_empty() { 0.0 } else { nums.iter().sum::<f64>() / nums.len() as f64 };
            let thr = match c {
                FilterCriterion::Top10 { bottom, percent, count } => {
                    nums.sort_by(|a, b| if *bottom { a.partial_cmp(b) } else { b.partial_cmp(a) }.unwrap_or(Ordering::Equal));
                    let k = if *percent { ((nums.len() as f64) * (*count as f64) / 100.0).ceil() as usize } else { *count as usize };
                    let k = k.clamp(1, nums.len().max(1));
                    nums.get(k - 1).copied()
                }
                _ => None,
            };
            col_stats.insert(i, (thr, avg));
        }
    }
    for row in rows {
        let mut visible = true;
        for (i, (off, crit)) in criteria.iter().enumerate() {
            let c = CellRef::new(row, r.start.col + off);
            let v = sh.value(c);
            let text = crate::display::cell_text(wb, sh, c);
            let ok = match crit {
                FilterCriterion::Values { values, blanks } => {
                    if filter_blank(&v) {
                        *blanks
                    } else {
                        values.iter().any(|x| x.eq_ignore_ascii_case(&text) || x.eq_ignore_ascii_case(&v.display()))
                    }
                }
                FilterCriterion::Custom { a, b, and } => {
                    let x = custom_ok(&v, &text, &a.0, &a.1);
                    match b {
                        Some(b) => {
                            let y = custom_ok(&v, &text, &b.0, &b.1);
                            if *and { x && y } else { x || y }
                        }
                        None => x,
                    }
                }
                FilterCriterion::Top10 { bottom, .. } => match (v.as_f64(), col_stats.get(&i)) {
                    (Some(n), Some((thr, _))) => {
                        let thr = thr.unwrap_or(n);
                        if *bottom { n <= thr } else { n >= thr }
                    }
                    _ => false,
                },
                FilterCriterion::AboveAverage(above) => match (v.as_f64(), col_stats.get(&i)) {
                    (Some(n), Some((_, avg))) => {
                        if *above {
                            n > *avg
                        } else {
                            n < *avg
                        }
                    }
                    _ => false,
                },
                FilterCriterion::FillColor(col) => wb.styles.get(sh.style_id(c)).fill.fg == *col,
                FilterCriterion::FontColor(col) => wb.styles.get(sh.style_id(c)).font.color == *col,
                // Icons are not evaluated here: the row keeps its current visibility.
                FilterCriterion::Icon { .. } => !sh.is_row_hidden(row),
            };
            if !ok {
                visible = false;
                break;
            }
        }
        hide.push((row, !visible));
    }
}

pub(super) fn clear_filter(s: &mut Session, _: &Json) -> Result<Json> {
    let sheet = s.doc()?.wb.active_sheet();
    crate::cmd::refuse_unless_allowed(s, sheet, "Filtering", |p| p.autofilter)?;
    edit(s, |cx| {
        let sh = cx.sheet_mut(sheet)?;
        let spans = sh.filter_row_spans();
        if let Some(a) = sh.autofilter.as_mut() {
            a.criteria.clear();
        }
        for t in &mut sh.tables {
            t.filter.clear();
        }
        for (first, last) in spans {
            if first <= last {
                for (_, row) in sh.rows.range_mut(first..=last) {
                    row.hidden = false;
                }
            }
        }
        cx.structural = true;
        Ok(Json::Null)
    })
}

pub(super) fn reapply(s: &mut Session, _: &Json) -> Result<Json> {
    let sheet = s.doc()?.wb.active_sheet();
    crate::cmd::refuse_unless_allowed(s, sheet, "Filtering", |p| p.autofilter)?;
    edit(s, |cx| {
        apply_filters(&mut cx.wb, sheet, None, false);
        cx.structural = true;
        Ok(Json::Null)
    })
}

/// Distinct display values of a filter column (for the dropdown list): of the named table's own
/// filter (`data.filterBy`'s `table`), or of the worksheet AutoFilter without one.
pub fn filter_values(wb: &Workbook, sheet: usize, table: Option<&str>, col: u32) -> Vec<(String, bool)> {
    let Some(sh) = wb.sheet(sheet) else { return vec![] };
    let (range, selected) =
        match table {
            Some(name) => {
                let Some(t) = sh.tables.iter().find(|t| t.name.eq_ignore_ascii_case(name)) else { return vec![] };
                let Some(range) = t.autofilter_range() else { return vec![] };
                let Some(off) = col.checked_sub(range.start.col).filter(|_| col <= range.end.col) else { return vec![] };
                // A worksheet proxy of an older GridCraft file still holds criteria the table adopts on its next change.
                let proxy = sh.autofilter.as_ref().filter(|af| t.matches_autofilter(af.range));
                let selected = t.filter.iter().chain(proxy.into_iter().flat_map(|af| af.criteria.iter())).find(|(o, _)| *o == off).map(|(_, c)| c);
                (range, selected)
            }
            None => {
                let Some(af) = &sh.autofilter else { return vec![] };
                let off = col.saturating_sub(af.range.start.col);
                let selected =
                    af.criteria.iter().find(|(o, _)| *o == off).map(|(_, c)| c).or_else(|| {
                        sh.tables.iter().find(|t| t.matches_autofilter(af.range))?.filter.iter().find(|(o, _)| *o == off).map(|(_, c)| c)
                    });
                (sh.effective_autofilter_range(af.range), selected)
            }
        };
    let mut seen = std::collections::BTreeMap::new();
    for row in range.start.row.saturating_add(1)..=range.end.row.min(range.start.row.saturating_add(1_000_000)) {
        let c = CellRef::new(row, col);
        let v = sh.value(c);
        let blank = filter_blank(&v);
        let t = if blank { "(Blanks)".to_string() } else { crate::display::cell_text(wb, sh, c) };
        let on = match selected {
            Some(FilterCriterion::Values { values, blanks }) => {
                if blank {
                    *blanks
                } else {
                    values.iter().any(|x| x.eq_ignore_ascii_case(&t))
                }
            }
            _ => true,
        };
        let key = if blank { sort_key(&Value::Empty) } else { sort_key(&v) };
        seen.entry((key, t)).or_insert(on);
    }
    let mut values: Vec<_> = seen.into_iter().collect();
    values.sort_by(|((a, ta), _), ((b, tb), _)| {
        a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then_with(|| gridcraft_core::compare_text(&a.2, &b.2)).then_with(|| ta.cmp(tb))
    });
    values.into_iter().map(|((_, t), on)| (t, on)).collect()
}
