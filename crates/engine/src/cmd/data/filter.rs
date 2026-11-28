use super::sort::sort_key;
use super::{
    AutoFilter, CellRef, EngineError, FilterCriterion, Json, Ordering, RangeRef, Result, Session, Sheet, Value, Workbook, bad, bool_param, col_param,
    data_target, edit, json,
};
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

// ---------------------------------------------------------------- filter

pub(super) fn toggle_filter(s: &mut Session, p: &Json) -> Result<Json> {
    let (sheet, r) = data_target(s, p, "data.filter")?;
    crate::cmd::refuse_unless_allowed(s, sheet, "Filtering", |p| p.autofilter)?;
    // The active cell belongs to the active sheet: on another sheet the table is the one at `range`.
    let at = if sheet == s.doc()?.wb.active_sheet() { s.doc()?.selection.active } else { r.start };
    let has = s.doc()?.wb.sheet(sheet).is_some_and(|sh| sh.autofilter.is_some());
    let in_table = s.doc()?.wb.sheet(sheet).and_then(|sh| sh.table_at(at).map(|t| t.name.clone()));
    if let Some(name) = in_table {
        // An explicit range over part of the table is refused, as when installing a filter on it.
        if p.get("range").is_some()
            && let Some(sh) = s.doc()?.wb.sheet(sheet)
            && let Some(t) = sh.tables.iter().find(|t| t.name == name)
            && t.range.intersects(&r)
            && !(t.header_row
                && t.range.start.row == r.start.row
                && t.range.contains(r.start)
                && t.range.contains(r.end)
                && r.start.col == t.range.start.col
                && r.end.col == t.range.end.col
                && t.data_range().is_none_or(|data| r.end.row >= data.end.row))
        {
            return Err(bad("data.filter", "a filter range can't overlap a table; filter inside the table instead"));
        }
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
///
/// A sheet AutoFilter never overlaps a table, because a file can't hold one (Excel refuses
/// `Range("A1:C8").AutoFilter` over part of a table, and from a cell outside a table it leaves the
/// table's cells out of the region). Inside a table the filter covers the table from its header
/// row, which is saved as the table's own filter, and takes over the criteria the table already
/// had so there is one filter to save.
fn install_filter(s: &mut Session, p: &Json) -> Result<Json> {
    let (sheet, mut r) = data_target(s, p, "data.filter")?;
    {
        let d = s.doc()?;
        let sh = d.wb.active().ok_or(EngineError::NoDocument)?;
        if p.get("range").is_none() && sh.table_at(d.selection.active).is_none() && d.selection.current().is_single() {
            r = crate::selection::current_region_outside_tables(sh, d.selection.active);
        }
        // A selected header row filters the data below it (#201).
        if p.get("range").is_none() && r.height() == 1 {
            let region = match sh.table_at(r.start) {
                Some(t) => t.data_range().map(|dr| RangeRef::new(t.range.start, dr.end)).unwrap_or(t.range),
                None => crate::selection::current_region_outside_tables(sh, r.start),
            };
            if region.end.row > r.end.row {
                r = RangeRef::new(r.start, CellRef::new(region.end.row, r.end.col));
            }
        }
    }
    let Some(sh) = s.doc()?.wb.sheet(sheet) else { return Err(EngineError::NoDocument) };
    // The table whose header row the filter starts on, when it lies inside that table.
    let host =
        sh.tables.iter().position(|t| t.header_row && t.range.start.row == r.start.row && t.range.contains(r.start) && t.range.contains(r.end));
    if host.is_none() && sh.tables.iter().any(|t| t.range.intersects(&r)) {
        return Err(bad("data.filter", "a filter range can't overlap a table; filter inside the table instead"));
    }
    // A narrower range would leave out the table's criteria on the columns it doesn't cover, and saving would drop them.
    if let Some(t) = host.and_then(|i| sh.tables.get(i)) {
        if t.data_range().is_some_and(|data| r.end.row < data.end.row) {
            return Err(bad("data.filter", "filter the table's full data range"));
        }
        let shift = r.start.col.saturating_sub(t.range.start.col);
        if t.filter.iter().any(|(o, _)| o.checked_sub(shift).is_none_or(|c| c >= r.width())) {
            return Err(bad("data.filter", "the table is filtered on a column this range leaves out; filter the table's full width"));
        }
    }
    edit(s, |cx| {
        let sh = cx.sheet_mut(sheet)?;
        let mut criteria = vec![];
        if let Some(t) = host.and_then(|i| sh.tables.get_mut(i)) {
            // The criteria are saved with the table only while its filter buttons are on.
            t.filter_button = true;
            let shift = r.start.col.saturating_sub(t.range.start.col);
            criteria = std::mem::take(&mut t.filter).into_iter().filter_map(|(o, c)| Some((o.checked_sub(shift)?, c))).collect();
        }
        sh.autofilter = Some(AutoFilter { range: r, criteria });
        Ok(json!({"filter": true, "range": r.a1()}))
    })
}

pub(super) fn filter_by(s: &mut Session, p: &Json) -> Result<Json> {
    let sheet = s.doc()?.wb.active_sheet();
    crate::cmd::refuse_unless_allowed(s, sheet, "Filtering", |p| p.autofilter)?;
    let Some(sh) = s.doc()?.wb.sheet(sheet) else { return Err(EngineError::NoDocument) };
    // Omission keeps the legacy install/proxy behavior. Null or empty explicitly targets the worksheet.
    let (name, worksheet_only) = match p.get("table") {
        None => (None, false),
        Some(Json::Null) => (None, true),
        Some(Json::String(name)) if name.trim().is_empty() => (None, true),
        Some(Json::String(name)) => (Some(name.as_str()), false),
        Some(_) => return Err(bad("data.filterBy", "`table` must be a name, null, or empty")),
    };
    let table = name
        .map(|name| {
            sh.tables
                .iter()
                .position(|t| t.name.eq_ignore_ascii_case(name) && t.filter_button && t.autofilter_range().is_some())
                .ok_or_else(|| bad("data.filterBy", format!("no table named `{name}` with filter buttons on the active sheet")))
        })
        .transpose()?;
    let range = match (table.and_then(|i| sh.tables.get(i)), &sh.autofilter) {
        (Some(t), _) => t.range,
        (None, Some(a)) => a.range,
        (None, None) if worksheet_only => return Err(bad("data.filterBy", "no worksheet AutoFilter on the active sheet")),
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
        let native =
            table.or_else(|| if worksheet_only { None } else { sh.tables.iter().position(|t| !t.filter.is_empty() && t.matches_autofilter(range)) });
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

/// Whether the worksheet AutoFilter `range` stands for table `t`'s filter: an older GridCraft
/// proxy over the whole table, or a filter installed from the table's header row (a worksheet
/// filter can't otherwise overlap a table).
fn hosts_table_filter(t: &gridcraft_model::Table, range: RangeRef) -> bool {
    t.matches_autofilter(range) || (t.header_row && range.start.row == t.range.start.row && range.intersects(&t.range))
}

/// Clear a table's criteria and any worksheet filter standing for it, returning whether visibility must be recalculated.
pub(crate) fn clear_table_filter(sh: &mut Sheet, table: usize) -> bool {
    let t = &mut sh.tables[table];
    let was_active = !t.filter.is_empty();
    t.filter.clear();
    let proxy = sh.autofilter.take_if(|af| hosts_table_filter(t, af.range));
    was_active || proxy.is_some_and(|af| !af.criteria.is_empty())
}

/// Apply worksheet and table criteria. Where two ranges cover the same row, either can hide it.
/// Re-evaluate a changed scope even when its last criterion was removed.
pub(crate) fn apply_filters(wb: &mut Workbook, sheet: usize, changed_table: Option<usize>, changed_sheet: bool) {
    let Some(sh) = wb.sheet(sheet) else { return };
    let mut scopes = Vec::new();
    if let Some(af) = &sh.autofilter
        && (changed_sheet || !af.criteria.is_empty())
    {
        scopes.push(FilterScope::worksheet(sh, af, Cow::Borrowed(&af.criteria)));
    }
    for (i, t) in sh.tables.iter().enumerate() {
        if (changed_table == Some(i) || !t.filter.is_empty())
            && let Some(data) = t.data_range()
        {
            scopes.push(FilterScope::new(data.start.col, data.start.row, data.end.row, Cow::Borrowed(&t.filter)));
        }
    }
    let mut rows = Vec::new();
    for scope in scopes {
        let matcher = FilterMatcher::new(wb, sheet, sh, scope);
        // Reapplying a retained icon filter preserves visibility until icons can be evaluated.
        let has_icon = matcher.scope.criteria.iter().any(|(_, c)| matches!(c, FilterCriterion::Icon { .. }));
        rows.extend(matcher.scope.rows().map(|row| (row, !matcher.row_matches(row, None) || (has_icon && sh.is_row_hidden(row)))));
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

/// One filter's criteria over its data rows, with column offsets counted from `col`.
struct FilterScope<'a> {
    col: u32,
    first: u32,
    /// The last data row, within the grid: an imported range can name rows past it, and none of
    /// those exist to hide, count or list.
    last: u32,
    criteria: Cow<'a, [(u32, FilterCriterion)]>,
}

impl<'a> FilterScope<'a> {
    fn new(col: u32, first: u32, last: u32, criteria: Cow<'a, [(u32, FilterCriterion)]>) -> Self {
        Self { col, first, last: last.min(gridcraft_core::MAX_ROWS - 1), criteria }
    }

    /// The data rows below a worksheet AutoFilter's header, bounded like a table's when it proxies one.
    fn worksheet(sh: &Sheet, af: &AutoFilter, criteria: Cow<'a, [(u32, FilterCriterion)]>) -> Self {
        let r = sh.effective_autofilter_range(af.range);
        Self::new(r.start.col, r.start.row.saturating_add(1), r.end.row, criteria)
    }

    fn rows(&self) -> std::ops::RangeInclusive<u32> {
        self.first..=self.last
    }
}

/// Statistics use the filter's whole data range within the grid, regardless of the other active criteria.
fn filter_stats(sh: &Sheet, scope: &FilterScope<'_>) -> HashMap<usize, (Option<f64>, f64)> {
    // Precompute per-criterion top10 threshold and average once, not per row.
    let mut col_stats = HashMap::new();
    for (i, (off, c)) in scope.criteria.iter().enumerate() {
        if matches!(c, FilterCriterion::Top10 { .. } | FilterCriterion::AboveAverage(_)) {
            let Some(col) = scope.col.checked_add(*off) else { continue };
            let mut nums: Vec<f64> = scope.rows().filter_map(|row| sh.value(CellRef::new(row, col)).as_f64()).collect();
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
    col_stats
}

/// Shared row and checkbox matching, with thresholds and Values lookups prepared once per query.
struct FilterMatcher<'a> {
    wb: &'a Workbook,
    sheet: usize,
    sh: &'a Sheet,
    scope: FilterScope<'a>,
    stats: HashMap<usize, (Option<f64>, f64)>,
    values: HashMap<usize, HashSet<String>>,
    cf: std::cell::RefCell<crate::cf::CfCache>,
}

impl<'a> FilterMatcher<'a> {
    fn new(wb: &'a Workbook, sheet: usize, sh: &'a Sheet, scope: FilterScope<'a>) -> Self {
        let values = scope
            .criteria
            .iter()
            .enumerate()
            .filter_map(|(i, (_, crit))| match crit {
                FilterCriterion::Values { values, .. } => Some((i, values.iter().map(|v| v.to_ascii_lowercase()).collect())),
                _ => None,
            })
            .collect();
        let stats = filter_stats(sh, &scope);
        Self { wb, sheet, sh, scope, stats, values, cf: std::cell::RefCell::new(crate::cf::CfCache::new()) }
    }

    fn cell_text(&self, c: CellRef) -> String {
        crate::display::cell_text_with_cf(self.wb, self.sheet, self.sh, c, &mut self.cf.borrow_mut())
    }

    fn row_matches(&self, row: u32, excluded_offset: Option<u32>) -> bool {
        self.scope.criteria.iter().enumerate().all(|(i, (off, crit))| Some(*off) == excluded_offset || self.criterion_matches(row, i, *off, crit))
    }

    fn criterion_matches(&self, row: u32, i: usize, off: u32, crit: &FilterCriterion) -> bool {
        let Some(col) = self.scope.col.checked_add(off) else { return true };
        let c = CellRef::new(row, col);
        let v = self.sh.value(c);
        let text = self.cell_text(c);
        match crit {
            FilterCriterion::Values { blanks, .. } => {
                if filter_blank(&v) {
                    *blanks
                } else {
                    self.values
                        .get(&i)
                        .is_some_and(|values| values.contains(&text.to_ascii_lowercase()) || values.contains(&v.display().to_ascii_lowercase()))
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
            FilterCriterion::Top10 { bottom, .. } => match (v.as_f64(), self.stats.get(&i)) {
                (Some(n), Some((thr, _))) => {
                    let thr = thr.unwrap_or(n);
                    if *bottom { n <= thr } else { n >= thr }
                }
                _ => false,
            },
            FilterCriterion::AboveAverage(above) => match (v.as_f64(), self.stats.get(&i)) {
                (Some(n), Some((_, avg))) => {
                    if *above {
                        n > *avg
                    } else {
                        n < *avg
                    }
                }
                _ => false,
            },
            FilterCriterion::FillColor(col) => self.wb.styles.get(self.sh.style_id(c)).fill.fg == *col,
            FilterCriterion::FontColor(col) => self.wb.styles.get(self.sh.style_id(c)).font.color == *col,
            // Icon criteria are retained from files but cannot be evaluated yet.
            FilterCriterion::Icon { .. } => true,
        }
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

/// The filter a dropdown query reads, chosen as `data.filterBy`'s `table` parameter chooses it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterOwner<'a> {
    /// `table` omitted: the worksheet AutoFilter, falling back to the criteria of the table an
    /// older GridCraft file's worksheet proxy stands for.
    Legacy,
    /// `table` null or empty: the worksheet AutoFilter alone, without a separate table filter's criteria.
    Worksheet,
    /// The named table's own filter, including criteria a legacy worksheet proxy still holds for it.
    Table(&'a str),
}

/// Distinct display values of a worksheet AutoFilter column ([`FilterOwner::Legacy`]); see [`filter_values_for`].
pub fn filter_values(wb: &Workbook, sheet: usize, col: u32) -> Vec<(String, bool)> {
    filter_values_for(wb, sheet, FilterOwner::Legacy, col)
}

/// The whole-column checkbox state of a worksheet AutoFilter column ([`FilterOwner::Legacy`]); see [`filter_selection_for`].
pub fn filter_selection(wb: &Workbook, sheet: usize, col: u32, choices: &[(String, bool)]) -> Vec<(String, bool)> {
    filter_selection_for(wb, sheet, FilterOwner::Legacy, col, choices)
}

/// Distinct display values of `owner`'s column, from rows matching its other columns' criteria
/// (for the dropdown list). The current criterion only determines checkboxes, so its unchecked
/// alternatives stay available. Other filters on the sheet don't restrict the choices.
pub fn filter_values_for(wb: &Workbook, sheet: usize, owner: FilterOwner<'_>, col: u32) -> Vec<(String, bool)> {
    column_filter_values(wb, sheet, owner, col, true)
}

/// Applies edited eligible choices to the whole column's checkbox state, retaining choices hidden by other criteria.
pub fn filter_selection_for(wb: &Workbook, sheet: usize, owner: FilterOwner<'_>, col: u32, choices: &[(String, bool)]) -> Vec<(String, bool)> {
    let mut edited = std::collections::BTreeMap::new();
    for (value, on) in choices {
        *edited.entry(value.to_ascii_lowercase()).or_insert(false) |= *on;
    }
    let mut selection = column_filter_values(wb, sheet, owner, col, false);
    for (value, on) in &mut selection {
        if let Some(checked) = edited.get(&value.to_ascii_lowercase()) {
            *on = *checked;
        }
    }
    selection
}

/// Criteria from `primary`, then those of `fallback` on columns `primary` leaves unset.
fn merged_criteria<'a>(primary: &'a [(u32, FilterCriterion)], fallback: &[(u32, FilterCriterion)]) -> Cow<'a, [(u32, FilterCriterion)]> {
    let extra: Vec<_> = fallback.iter().filter(|(o, _)| !primary.iter().any(|(p, _)| p == o)).cloned().collect();
    if extra.is_empty() { Cow::Borrowed(primary) } else { Cow::Owned(primary.iter().cloned().chain(extra).collect()) }
}

/// The scope a dropdown of `owner` lists, with its header range (whose columns bound the query).
fn owner_scope<'a>(sh: &'a Sheet, owner: FilterOwner<'_>) -> Option<(RangeRef, FilterScope<'a>)> {
    match owner {
        FilterOwner::Table(name) => {
            let t = sh.tables.iter().find(|t| t.name.eq_ignore_ascii_case(name))?;
            let range = t.autofilter_range()?;
            // A worksheet filter standing for the table still holds criteria the table adopts on its next change.
            let proxy = sh.autofilter.as_ref().filter(|af| t.matches_autofilter(af.range)).map_or(&[][..], |af| &af.criteria);
            let criteria = merged_criteria(&t.filter, proxy);
            Some((range, FilterScope::new(range.start.col, range.start.row.saturating_add(1), range.end.row, criteria)))
        }
        FilterOwner::Legacy | FilterOwner::Worksheet => {
            let af = sh.autofilter.as_ref()?;
            let table = sh.tables.iter().find(|t| t.matches_autofilter(af.range)).filter(|_| owner == FilterOwner::Legacy);
            let criteria = merged_criteria(&af.criteria, table.map_or(&[][..], |t| &t.filter));
            Some((sh.effective_autofilter_range(af.range), FilterScope::worksheet(sh, af, criteria)))
        }
    }
}

fn column_filter_values(wb: &Workbook, sheet: usize, owner: FilterOwner<'_>, col: u32, eligible_only: bool) -> Vec<(String, bool)> {
    let Some(sh) = wb.sheet(sheet) else { return vec![] };
    let Some((range, scope)) = owner_scope(sh, owner) else { return vec![] };
    let Some(off) = col.checked_sub(range.start.col).filter(|_| col <= range.end.col) else { return vec![] };
    // At most a million rows are listed; an imported end row can't add choices beyond the grid either.
    let rows = scope.first..=scope.last.min(scope.first.saturating_add(999_999));
    let matcher = FilterMatcher::new(wb, sheet, sh, scope);
    let selected = matcher.scope.criteria.iter().enumerate().find(|(_, (o, _))| *o == off);
    let mut seen = std::collections::BTreeMap::new();
    for row in rows {
        if eligible_only && !matcher.row_matches(row, Some(off)) {
            continue;
        }
        let c = CellRef::new(row, col);
        let v = sh.value(c);
        let blank = filter_blank(&v);
        let t = if blank { "(Blanks)".to_string() } else { matcher.cell_text(c) };
        let on = selected.is_none_or(|(i, (off, crit))| matcher.criterion_matches(row, i, *off, crit));
        let key = if blank { sort_key(&Value::Empty) } else { sort_key(&v) };
        // One choice can represent rows with different colors; keep any matching row checked.
        *seen.entry((key, t)).or_insert(false) |= on;
    }
    let mut values: Vec<_> = seen.into_iter().collect();
    values.sort_by(|((a, ta), _), ((b, tb), _)| {
        a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then_with(|| gridcraft_core::compare_text(&a.2, &b.2)).then_with(|| ta.cmp(tb))
    });
    values.into_iter().map(|((_, t), on)| (t, on)).collect()
}
