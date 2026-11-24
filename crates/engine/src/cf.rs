//! Conditional formatting evaluation: what extra style, data bar, colour scale or icon a cell
//! gets.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

use gridcraft_core::{CellRef, RangeRef, Value, compare};
use gridcraft_model::{CfOperator, CfRule, CfValueKind, Color, PatternType, Sheet, Style, Table, Underline, Workbook, default_icon_thresholds};

/// The conditional look of one cell.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CfLook {
    /// Differential style (only non-default parts apply).
    pub style: Option<Style>,
    /// Fill from a colour scale (RGB).
    pub scale_fill: Option<[u8; 3]>,
    /// Data bar: fraction 0..=1 of the cell width, colour, gradient.
    pub bar: Option<(f32, [u8; 3], bool)>,
    /// Icon: set name and index (0 = lowest).
    pub icon: Option<(String, usize)>,
    pub hide_value: bool,
}

/// Precomputed statistics per rule (min/max/percentiles/top-N thresholds/duplicates), each
/// table's visible edges, and a per-sheet spatial index over conditional-format ranges.
/// The grid and print keep one per paint.
#[derive(Default)]
pub struct CfCache {
    stats: HashMap<usize, RuleStats>,
    table_edges: HashMap<RangeRef, crate::tables::ShownEdges>,
    index: Option<SheetCfIndex>,
}

struct RuleMeta {
    orig_idx: usize,
    first: Option<CellRef>,
    sets_num_fmt: bool,
}

struct RangeGroup {
    /// Rule ranks into `SheetCfIndex::rules` (already sorted by `(priority, orig_idx)`).
    rule_ranks: Vec<u32>,
    has_num_fmt: bool,
}

struct IntervalNode {
    center: u32,
    is_leaf: bool,
    /// Multi-cell `(range, group_id)` entries sorted ascending by `range.start.row`.
    by_start: Vec<(RangeRef, u32)>,
    /// Multi-cell `(range, group_id)` entries sorted descending by `range.end.row` (empty for leaves).
    by_end: Vec<(RangeRef, u32)>,
    left: Option<usize>,
    right: Option<usize>,
}

/// Spatial index over one sheet's conditional formats:
/// - identical `sqref` lists across rules are grouped once (`RangeGroup`);
/// - exact 1×1 ranges live in `single_cells` (`HashMap`) plus sorted `single_keys`;
/// - multi-cell ranges live in a balanced row interval tree (`multi_tree`).
struct SheetCfIndex {
    sheet: usize,
    rule_count: usize,
    bbox: Option<RangeRef>,
    any_num_fmt: bool,
    rules: Vec<RuleMeta>,
    groups: Vec<RangeGroup>,
    single_cells: HashMap<CellRef, Vec<u32>>,
    single_keys: Vec<CellRef>,
    unique_multi: Vec<(RangeRef, u32)>,
    multi_tree: Vec<IntervalNode>,
}

impl SheetCfIndex {
    fn build(sh: &Sheet, sheet: usize) -> Self {
        let mut order: Vec<(usize, u32)> = sh.cond_formats.iter().enumerate().map(|(i, cf)| (i, cf.priority)).collect();
        order.sort_by_key(|&(orig_idx, priority)| (priority, orig_idx));

        let mut rules = Vec::with_capacity(order.len());
        let mut any_num_fmt = false;
        for &(orig_idx, _) in &order {
            let sets_num_fmt = rule_style(&sh.cond_formats[orig_idx].rule).is_some_and(style_sets_num_fmt);
            any_num_fmt |= sets_num_fmt;
            rules.push(RuleMeta { orig_idx, first: None, sets_num_fmt });
        }

        let mut groups: Vec<RangeGroup> = Vec::new();
        let mut group_reps: Vec<usize> = Vec::new();
        let mut buckets: HashMap<u64, Vec<(u32, usize)>> = HashMap::new();
        for (rank, meta) in rules.iter().enumerate() {
            let ranges = &sh.cond_formats[meta.orig_idx].ranges;
            if ranges.is_empty() {
                continue;
            }
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            ranges.hash(&mut hasher);
            let h = hasher.finish();
            let bucket = buckets.entry(h).or_default();
            if let Some(&(gid, _)) = bucket.iter().find(|&&(_, rep)| sh.cond_formats[rep].ranges == *ranges) {
                let g = &mut groups[gid as usize];
                g.rule_ranks.push(rank as u32);
                g.has_num_fmt |= meta.sets_num_fmt;
            } else {
                let gid = groups.len() as u32;
                groups.push(RangeGroup { rule_ranks: vec![rank as u32], has_num_fmt: meta.sets_num_fmt });
                group_reps.push(meta.orig_idx);
                bucket.push((gid, meta.orig_idx));
            }
        }

        let mut bbox: Option<RangeRef> = None;
        let mut single_cells: HashMap<CellRef, Vec<u32>> = HashMap::new();
        let mut unique_multi: Vec<(RangeRef, u32)> = Vec::new();
        for (gid, &rep_idx) in group_reps.iter().enumerate() {
            let ranges = &sh.cond_formats[rep_idx].ranges;
            let mut seen_in_group = (ranges.len() > 1).then(|| HashSet::with_capacity(ranges.len()));
            let mut group_first: Option<CellRef> = None;
            for &r in ranges {
                if let Some(seen) = &mut seen_in_group
                    && !seen.insert(r)
                {
                    continue;
                }
                group_first = Some(match group_first {
                    None => r.start,
                    Some(f) => CellRef::new(f.row.min(r.start.row), f.col.min(r.start.col)),
                });
                bbox = Some(bbox.map_or(r, |b| b.union(&r)));
                if r.is_single() {
                    single_cells.entry(r.start).or_default().push(gid as u32);
                } else {
                    unique_multi.push((r, gid as u32));
                }
            }
            for &rank in &groups[gid].rule_ranks {
                rules[rank as usize].first = group_first;
            }
        }

        let mut single_keys: Vec<CellRef> = single_cells.keys().copied().collect();
        single_keys.sort_unstable();
        let multi_tree = build_interval_tree(unique_multi.clone());

        Self { sheet, rule_count: sh.cond_formats.len(), bbox, any_num_fmt, rules, groups, single_cells, single_keys, unique_multi, multi_tree }
    }

    fn query_point_groups(&self, c: CellRef, mut emit: impl FnMut(u32)) {
        if !self.bbox.is_some_and(|b| b.contains(c)) {
            return;
        }
        if let Some(gs) = self.single_cells.get(&c) {
            for &g in gs {
                emit(g);
            }
        }
        if self.multi_tree.is_empty() {
            return;
        }
        let mut node_idx = Some(0usize);
        while let Some(ni) = node_idx {
            let node = &self.multi_tree[ni];
            if node.is_leaf {
                for &(r, g) in &node.by_start {
                    if r.contains(c) {
                        emit(g);
                    }
                }
                break;
            }
            if c.row < node.center {
                for &(r, g) in &node.by_start {
                    if r.start.row > c.row {
                        break;
                    }
                    if c.col >= r.start.col && c.col <= r.end.col {
                        emit(g);
                    }
                }
                node_idx = node.left;
            } else if c.row > node.center {
                for &(r, g) in &node.by_end {
                    if r.end.row < c.row {
                        break;
                    }
                    if c.col >= r.start.col && c.col <= r.end.col {
                        emit(g);
                    }
                }
                node_idx = node.right;
            } else {
                for &(r, g) in &node.by_start {
                    if c.col >= r.start.col && c.col <= r.end.col {
                        emit(g);
                    }
                }
                break;
            }
        }
    }

    fn query_block_multi(&self, block: RangeRef, mut emit: impl FnMut(RangeRef, u32)) {
        if self.multi_tree.is_empty() || !self.bbox.is_some_and(|b| b.intersects(&block)) {
            return;
        }
        let mut stack = vec![0usize];
        while let Some(ni) = stack.pop() {
            let node = &self.multi_tree[ni];
            if node.is_leaf {
                for &(r, g) in &node.by_start {
                    if let Some(sub) = r.intersection(&block) {
                        emit(sub, g);
                    }
                }
                continue;
            }
            if block.end.row < node.center {
                for &(r, g) in &node.by_start {
                    if r.start.row > block.end.row {
                        break;
                    }
                    if let Some(sub) = r.intersection(&block) {
                        emit(sub, g);
                    }
                }
                if let Some(l) = node.left {
                    stack.push(l);
                }
            } else if block.start.row > node.center {
                for &(r, g) in &node.by_end {
                    if r.end.row < block.start.row {
                        break;
                    }
                    if let Some(sub) = r.intersection(&block) {
                        emit(sub, g);
                    }
                }
                if let Some(r) = node.right {
                    stack.push(r);
                }
            } else {
                for &(r, g) in &node.by_start {
                    if let Some(sub) = r.intersection(&block) {
                        emit(sub, g);
                    }
                }
                if let Some(l) = node.left {
                    stack.push(l);
                }
                if let Some(r) = node.right {
                    stack.push(r);
                }
            }
        }
    }

    fn matches_any(&self, c: CellRef) -> bool {
        if !self.bbox.is_some_and(|b| b.contains(c)) {
            return false;
        }
        if self.single_cells.contains_key(&c) {
            return true;
        }
        let mut hit = false;
        self.query_point_groups(c, |_| hit = true);
        hit
    }

    fn has_num_fmt_at(&self, c: CellRef) -> bool {
        if !self.any_num_fmt || !self.bbox.is_some_and(|b| b.contains(c)) {
            return false;
        }
        let mut hit = false;
        self.query_point_groups(c, |g| {
            if self.groups[g as usize].has_num_fmt {
                hit = true;
            }
        });
        hit
    }
}

fn build_interval_tree(entries: Vec<(RangeRef, u32)>) -> Vec<IntervalNode> {
    fn rec(mut entries: Vec<(RangeRef, u32)>, nodes: &mut Vec<IntervalNode>) -> Option<usize> {
        if entries.is_empty() {
            return None;
        }
        let idx = nodes.len();
        if entries.len() <= 8 {
            entries.sort_unstable_by_key(|(r, _)| r.start.row);
            nodes.push(IntervalNode { center: 0, is_leaf: true, by_start: entries, by_end: Vec::new(), left: None, right: None });
            return Some(idx);
        }
        entries.sort_unstable_by_key(|(r, _)| r.start.row);
        let center = entries[entries.len() / 2].0.start.row;
        let mut left = Vec::new();
        let mut mid = Vec::new();
        let mut right = Vec::new();
        for item in entries {
            if item.0.end.row < center {
                left.push(item);
            } else if item.0.start.row > center {
                right.push(item);
            } else {
                mid.push(item);
            }
        }
        let mut by_end = mid.clone();
        by_end.sort_unstable_by_key(|(a, _)| std::cmp::Reverse(a.end.row));
        nodes.push(IntervalNode { center, is_leaf: false, by_start: mid, by_end, left: None, right: None });
        let l = rec(left, nodes);
        let r = rec(right, nodes);
        nodes[idx].left = l;
        nodes[idx].right = r;
        Some(idx)
    }
    let mut nodes = Vec::new();
    rec(entries, &mut nodes);
    nodes
}

#[derive(Default, Clone)]
struct RuleStats {
    sorted: Vec<f64>,
    avg: f64,
    std: f64,
    counts: HashMap<String, usize>,
}

/// Which statistics a rule reads. Scanning the rule's whole range for them costs O(range), so
/// rules that read none (cell value, formula, text, blanks, errors, dates) never scan.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum StatsNeed {
    /// Sorted numbers, their mean and deviation (top/bottom N, above average, scales, bars, icons).
    Numbers,
    /// How often each displayed text occurs (duplicate / unique values).
    Counts,
}

fn stats_need(rule: &CfRule) -> Option<StatsNeed> {
    match rule {
        CfRule::Duplicate { .. } | CfRule::Unique { .. } => Some(StatsNeed::Counts),
        // only two- and three-colour scales paint: no statistics for the others
        CfRule::ColorScale { stops } if !matches!(stops.len(), 2 | 3) => None,
        CfRule::Top10 { .. } | CfRule::AboveAverage { .. } | CfRule::ColorScale { .. } | CfRule::DataBar { .. } | CfRule::IconSet { .. } => {
            Some(StatsNeed::Numbers)
        }
        _ => None,
    }
}

impl RuleStats {
    fn build(sh: &Sheet, ranges: &[RangeRef], need: StatsNeed) -> RuleStats {
        let (sorted, counts) = numbers_in(sh, ranges, need);
        let n = sorted.len().max(1) as f64;
        let avg = sorted.iter().sum::<f64>() / n;
        let std = (sorted.iter().map(|x| (x - avg).powi(2)).sum::<f64>() / n).sqrt();
        RuleStats { sorted, avg, std, counts }
    }
}

#[cfg(test)]
thread_local! {
    /// Cells `numbers_in` visited on this thread (tests count the work a lookup does).
    static SCANNED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn reset_scanned() {
    SCANNED.with(|n| n.set(0));
}

#[cfg(test)]
pub(crate) fn scanned_count() -> usize {
    SCANNED.with(std::cell::Cell::get)
}

/// Most cells one rule's statistics visit, across all of its ranges together: a rule repeating
/// the same range many times must not multiply the scan.
const STATS_SCAN_LIMIT: usize = 2_000_000;

/// Steps through the spilled cells inside `r`, row-major: `Some(cell)` for each, and `None` for a
/// jump past the spills left or right of the columns on a row (one step per row, however many
/// there are), so unrelated spills between a narrow range's corners cost little.
fn spill_steps(sh: &Sheet, r: RangeRef) -> impl Iterator<Item = Option<CellRef>> + '_ {
    let mut cur = Some(r.start);
    std::iter::from_fn(move || {
        let at = cur.filter(|at| *at <= r.end)?;
        let (&c, _) = sh.spill.range(at..=r.end).next()?;
        if c.col < r.start.col {
            cur = Some(CellRef::new(c.row, r.start.col));
            Some(None)
        } else if c.col > r.end.col {
            cur = c.row.checked_add(1).map(|row| CellRef::new(row, r.start.col));
            Some(None)
        } else {
            cur = c.col.checked_add(1).map(|col| CellRef::new(c.row, col));
            Some(Some(c))
        }
    })
}

fn numbers_in(sh: &Sheet, ranges: &[RangeRef], need: StatsNeed) -> (Vec<f64>, HashMap<String, usize>) {
    let mut nums = Vec::new();
    let mut counts = HashMap::new();
    let mut budget = STATS_SCAN_LIMIT;
    let mut seen: HashSet<RangeRef> = HashSet::new();
    let used = sh.used_range();
    for r in ranges {
        if budget == 0 {
            break;
        }
        let r = match used.and_then(|u| r.intersection(&u)) {
            Some(x) => x,
            None => continue,
        };
        // The same range listed twice counts its cells once.
        if !seen.insert(r) {
            continue;
        }
        // Only stored and spilled cells hold values: blank cells of a sparse range cost nothing,
        // so a value far down a huge range is still counted. Every cell looked at is charged.
        let stored = sh.cells.iter_range(r).map(|(c, _)| (Some(c), true));
        let spilled = spill_steps(sh, r).map(|c| (c, false));
        for (c, is_stored) in stored.chain(spilled).take(budget) {
            budget -= 1;
            // a skip over spills outside the columns (charged, not read), or a spilled value
            // under a stored cell, which was read with it
            let Some(c) = c.filter(|c| is_stored || sh.cells.get(*c).is_none()) else { continue };
            #[cfg(test)]
            SCANNED.with(|n| n.set(n.get() + 1));
            let v = sh.value(c);
            match need {
                StatsNeed::Numbers => {
                    if let Value::Number(n) = v {
                        nums.push(n);
                    }
                }
                StatsNeed::Counts => {
                    if !v.is_empty() {
                        *counts.entry(v.display().to_lowercase()).or_insert(0) += 1;
                    }
                }
            }
        }
    }
    nums.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    (nums, counts)
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = (p / 100.0).clamp(0.0, 1.0) * (sorted.len() - 1) as f64;
    let lo = rank.floor() as usize;
    let hi = rank.ceil() as usize;
    let a = sorted.get(lo).copied().unwrap_or(0.0);
    let b = sorted.get(hi).copied().unwrap_or(a);
    a + (b - a) * (rank - lo as f64)
}

fn threshold(wb: &Workbook, sheet: usize, at: CellRef, st: &RuleStats, k: &CfValueKind) -> f64 {
    let min = st.sorted.first().copied().unwrap_or(0.0);
    let max = st.sorted.last().copied().unwrap_or(0.0);
    match k {
        CfValueKind::Min => min,
        CfValueKind::Max => max,
        CfValueKind::Number(n) => *n,
        CfValueKind::Percent(p) => min + (max - min) * p / 100.0,
        CfValueKind::Percentile(p) => percentile(&st.sorted, *p),
        CfValueKind::Formula(f) => gridcraft_calc::evaluate(wb, sheet, at, f).to_number().unwrap_or(0.0),
    }
}

fn lerp(a: [u8; 3], b: [u8; 3], t: f64) -> [u8; 3] {
    let t = t.clamp(0.0, 1.0);
    [
        (a[0] as f64 + (b[0] as f64 - a[0] as f64) * t).round() as u8,
        (a[1] as f64 + (b[1] as f64 - a[1] as f64) * t).round() as u8,
        (a[2] as f64 + (b[2] as f64 - a[2] as f64) * t).round() as u8,
    ]
}

pub use gridcraft_model::icon_count;

struct RuleEval<'a> {
    wb: &'a Workbook,
    sheet: usize,
    cell: CellRef,
    first: CellRef,
    value: &'a Value,
    text: &'a str,
    stats: &'a RuleStats,
}

fn simple_rule_style<'a>(ctx: &RuleEval<'_>, rule: &'a CfRule) -> Option<&'a Style> {
    let RuleEval { wb, sheet, cell, first, value, text, stats } = ctx;
    match rule {
        CfRule::CellIs { op, a, b, style } => {
            let eval = |f: &str| gridcraft_calc::evaluate(wb, *sheet, *cell, &relative(f, *first, *cell));
            let av = eval(a);
            let matches = match op {
                CfOperator::Between | CfOperator::NotBetween => {
                    let bv = b.as_deref().map(eval).unwrap_or_default();
                    let (lo, hi) = if compare(&av, &bv) == std::cmp::Ordering::Greater { (bv, av) } else { (av, bv) };
                    let within = compare(value, &lo) != std::cmp::Ordering::Less && compare(value, &hi) != std::cmp::Ordering::Greater;
                    if *op == CfOperator::Between { within } else { !within }
                }
                CfOperator::Equal => compare(value, &av) == std::cmp::Ordering::Equal,
                CfOperator::NotEqual => compare(value, &av) != std::cmp::Ordering::Equal,
                CfOperator::Greater => compare(value, &av) == std::cmp::Ordering::Greater,
                CfOperator::Less => compare(value, &av) == std::cmp::Ordering::Less,
                CfOperator::GreaterOrEqual => compare(value, &av) != std::cmp::Ordering::Less,
                CfOperator::LessOrEqual => compare(value, &av) != std::cmp::Ordering::Greater,
            };
            (matches && !value.is_error()).then_some(style.as_ref())
        }
        CfRule::Expression { formula, style } => {
            gridcraft_calc::evaluate(wb, *sheet, *cell, &relative(formula, *first, *cell)).to_bool().unwrap_or(false).then_some(style.as_ref())
        }
        CfRule::ContainsText { text: needle, style } => {
            (!value.is_error() && text.to_lowercase().contains(&needle.to_lowercase())).then_some(style.as_ref())
        }
        CfRule::NotContainsText { text: needle, style } => {
            (value.is_error() || !text.to_lowercase().contains(&needle.to_lowercase())).then_some(style.as_ref())
        }
        CfRule::BeginsWith { text: needle, style } => {
            (!value.is_error() && text.to_lowercase().starts_with(&needle.to_lowercase())).then_some(style.as_ref())
        }
        CfRule::EndsWith { text: needle, style } => {
            (!value.is_error() && text.to_lowercase().ends_with(&needle.to_lowercase())).then_some(style.as_ref())
        }
        CfRule::Blanks { style } => (value.is_empty() || text.trim().is_empty()).then_some(style.as_ref()),
        CfRule::NoBlanks { style } => (!value.is_empty() && !text.trim().is_empty()).then_some(style.as_ref()),
        CfRule::Errors { style } => value.is_error().then_some(style.as_ref()),
        CfRule::NoErrors { style } => (!value.is_error()).then_some(style.as_ref()),
        CfRule::Duplicate { style } => {
            (!value.is_empty() && stats.counts.get(&text.to_lowercase()).copied().unwrap_or(0) > 1).then_some(style.as_ref())
        }
        CfRule::Unique { style } => {
            (!value.is_empty() && stats.counts.get(&text.to_lowercase()).copied().unwrap_or(0) == 1).then_some(style.as_ref())
        }
        _ => None,
    }
}

fn time_period_matches(wb: &Workbook, period: &str, serial: f64) -> bool {
    let today = gridcraft_core::date::serial_in_system(wb.date_system, gridcraft_calc::now_serial()).floor();
    let day = serial.floor();
    let date = |serial| gridcraft_core::date::datetime_from_serial(wb.date_system, serial);
    let week_start = today - date(today).map(|value| value.weekday as f64).unwrap_or(0.0);
    let (year, month) = date(today).map(|value| (value.year, value.month)).unwrap_or((2026, 1));
    let year_month = date(day).map(|value| (value.year, value.month));
    let month_matches = |offset: i32| {
        let shifted_month = month as i32 + offset;
        let shifted_year = year + (shifted_month - 1).div_euclid(12);
        let shifted_month = (shifted_month - 1).rem_euclid(12) + 1;
        year_month == Some((shifted_year, shifted_month as u32))
    };
    match period {
        "yesterday" => day == today - 1.0,
        "today" => day == today,
        "tomorrow" => day == today + 1.0,
        "last7Days" => day > today - 7.0 && day <= today,
        "lastWeek" => day >= week_start - 7.0 && day < week_start,
        "thisWeek" => day >= week_start && day < week_start + 7.0,
        "nextWeek" => day >= week_start + 7.0 && day < week_start + 14.0,
        "lastMonth" => month_matches(-1),
        "thisMonth" => month_matches(0),
        "nextMonth" => month_matches(1),
        _ => false,
    }
}

impl CfCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// [`crate::tables::shown_edges`] of `t`, found once per cache.
    pub fn table_edges(&mut self, sh: &Sheet, t: &Table) -> crate::tables::ShownEdges {
        *self.table_edges.entry(t.range).or_insert_with(|| crate::tables::shown_edges(sh, t))
    }

    fn ensure_index(&mut self, sh: &Sheet, sheet: usize) {
        let stale = self.index.as_ref().is_none_or(|idx| idx.sheet != sheet || idx.rule_count != sh.cond_formats.len());
        if stale {
            if self.index.as_ref().is_some_and(|idx| idx.sheet != sheet) {
                self.stats.clear();
            }
            self.index = Some(SheetCfIndex::build(sh, sheet));
        }
    }

    /// True when at least one conditional formatting rule on `sheet` covers `c`.
    pub(crate) fn may_apply(&mut self, wb: &Workbook, sheet: usize, c: CellRef) -> bool {
        let Some(sh) = wb.sheet(sheet) else { return false };
        if sh.cond_formats.is_empty() {
            return false;
        }
        self.ensure_index(sh, sheet);
        self.index.as_ref().is_some_and(|idx| idx.matches_any(c))
    }

    /// Deduplicated `(single_cells, multi_ranges)` of rules on `sh` matching `rule_pred`,
    /// optionally clipped to `block`. Uses the cached spatial index so repeated `sqref` lists
    /// across many rules are scanned once.
    pub(crate) fn indexed_ranges(
        &mut self,
        sh: &Sheet,
        sheet: usize,
        block: Option<RangeRef>,
        rule_pred: impl Fn(&CfRule) -> bool,
    ) -> (Vec<CellRef>, Vec<RangeRef>) {
        if sh.cond_formats.is_empty() {
            return (Vec::new(), Vec::new());
        }
        self.ensure_index(sh, sheet);
        let Some(index) = self.index.as_ref() else { return (Vec::new(), Vec::new()) };
        if let Some(b) = block
            && !index.bbox.is_some_and(|bb| bb.intersects(&b))
        {
            return (Vec::new(), Vec::new());
        }
        let mut group_ok = vec![false; index.groups.len()];
        let mut any_group = false;
        for (gid, grp) in index.groups.iter().enumerate() {
            if grp.rule_ranks.iter().any(|&rank| rule_pred(&sh.cond_formats[index.rules[rank as usize].orig_idx].rule)) {
                group_ok[gid] = true;
                any_group = true;
            }
        }
        if !any_group {
            return (Vec::new(), Vec::new());
        }
        let mut singles = Vec::new();
        match block {
            Some(b) => {
                let lo = index.single_keys.partition_point(|c| c.row < b.start.row);
                let hi = index.single_keys.partition_point(|c| c.row <= b.end.row);
                for &c in &index.single_keys[lo..hi] {
                    if c.col >= b.start.col
                        && c.col <= b.end.col
                        && index.single_cells.get(&c).is_some_and(|gs| gs.iter().any(|&g| group_ok[g as usize]))
                    {
                        singles.push(c);
                    }
                }
            }
            None => {
                for &c in &index.single_keys {
                    if index.single_cells.get(&c).is_some_and(|gs| gs.iter().any(|&g| group_ok[g as usize])) {
                        singles.push(c);
                    }
                }
            }
        }
        let mut multi = Vec::new();
        let mut seen_multi = HashSet::new();
        match block {
            Some(b) => {
                index.query_block_multi(b, |sub, g| {
                    if group_ok[g as usize] && seen_multi.insert(sub) {
                        multi.push(sub);
                    }
                });
            }
            None => {
                for &(r, g) in &index.unique_multi {
                    if group_ok[g as usize] && seen_multi.insert(r) {
                        multi.push(r);
                    }
                }
            }
        }
        (singles, multi)
    }

    /// Conditional look of `c`, or `None` when no rule applies.
    pub fn look(&mut self, wb: &Workbook, sheet: usize, c: CellRef) -> Option<CfLook> {
        self.look_inner(wb, sheet, c, None)
    }

    /// Conditional look of `c`, bounded by a remaining rule-evaluation work budget. Returns `None`
    /// immediately once `*budget == 0`.
    pub(crate) fn look_budgeted(&mut self, wb: &Workbook, sheet: usize, c: CellRef, budget: &mut usize) -> Option<CfLook> {
        self.look_inner(wb, sheet, c, Some(budget))
    }

    fn look_inner(&mut self, wb: &Workbook, sheet: usize, c: CellRef, mut budget: Option<&mut usize>) -> Option<CfLook> {
        if let Some(rem) = budget.as_deref()
            && *rem == 0
        {
            return None;
        }
        let sh = wb.sheet(sheet)?;
        if sh.cond_formats.is_empty() {
            return None;
        }
        self.ensure_index(sh, sheet);
        let index = self.index.as_ref()?;
        let mut matched_groups: Vec<u32> = Vec::new();
        index.query_point_groups(c, |g| matched_groups.push(g));
        if matched_groups.is_empty() {
            if let Some(rem) = budget.as_deref_mut() {
                *rem = rem.saturating_sub(1);
            }
            return None;
        }
        let owned_ranks: Vec<u32>;
        let rule_ranks: &[u32] = if matched_groups.len() == 1 {
            &index.groups[matched_groups[0] as usize].rule_ranks
        } else {
            matched_groups.sort_unstable();
            matched_groups.dedup();
            let mut ranks = Vec::new();
            for g in matched_groups {
                ranks.extend_from_slice(&index.groups[g as usize].rule_ranks);
            }
            ranks.sort_unstable();
            owned_ranks = ranks;
            &owned_ranks
        };
        let mut out = CfLook::default();
        let mut any = false;
        let v = sh.value(c);
        let text = v.display();
        let no_stats = RuleStats::default();
        let stats_map = &mut self.stats;
        for &rank in rule_ranks {
            if let Some(rem) = budget.as_deref_mut() {
                if *rem == 0 {
                    break;
                }
                *rem -= 1;
            }
            let meta = &index.rules[rank as usize];
            let idx = meta.orig_idx;
            let cf = &sh.cond_formats[idx];
            let first = meta.first.unwrap_or(c);
            let st: &RuleStats = match stats_need(&cf.rule) {
                Some(need) => &*stats_map.entry(idx).or_insert_with(|| RuleStats::build(sh, &cf.ranges, need)),
                None => &no_stats,
            };
            let context = RuleEval { wb, sheet, cell: c, first, value: &v, text: &text, stats: st };
            let matched_style = simple_rule_style(&context, &cf.rule).or_else(|| match &cf.rule {
                CfRule::Top10 { bottom, percent, rank, style } => match v.as_f64() {
                    Some(n) if !st.sorted.is_empty() => {
                        let k = if *percent { ((st.sorted.len() as f64) * (*rank as f64) / 100.0).floor().max(1.0) as usize } else { *rank as usize };
                        let k = k.clamp(1, st.sorted.len());
                        let ok = if *bottom {
                            n <= st.sorted.get(k - 1).copied().unwrap_or(n)
                        } else {
                            n >= st.sorted.get(st.sorted.len() - k).copied().unwrap_or(n)
                        };
                        ok.then_some(style.as_ref())
                    }
                    _ => None,
                },
                CfRule::AboveAverage { below, equal, std_dev, style } => match v.as_f64() {
                    Some(n) => {
                        let thr = st.avg + if *below { -1.0 } else { 1.0 } * st.std * *std_dev as f64;
                        let ok = if *below { n < thr || (*equal && n == thr) } else { n > thr || (*equal && n == thr) };
                        ok.then_some(style.as_ref())
                    }
                    None => None,
                },
                CfRule::TimePeriod { period, style } => match v.as_f64() {
                    Some(n) if time_period_matches(wb, period, n) => Some(style.as_ref()),
                    _ => None,
                },
                CfRule::ColorScale { stops } => {
                    // Only two- and three-colour scales paint: evaluate no thresholds for others.
                    if let Some(n) = v.as_f64().filter(|_| matches!(stops.len(), 2 | 3)) {
                        let pts: Vec<(f64, [u8; 3])> = stops
                            .iter()
                            .map(|(k, col)| (threshold(wb, sheet, c, st, k), col.resolve(&wb.theme).unwrap_or([255, 255, 255])))
                            .collect();
                        let rgb = match pts.as_slice() {
                            [(a, ca), (b, cb)] => lerp(*ca, *cb, if b > a { (n - a) / (b - a) } else { 0.0 }),
                            [(a, ca), (m, cm), (b, cb)] => {
                                if n <= *m {
                                    lerp(*ca, *cm, if m > a { (n - a) / (m - a) } else { 0.0 })
                                } else {
                                    lerp(*cm, *cb, if b > m { (n - m) / (b - m) } else { 1.0 })
                                }
                            }
                            _ => [255, 255, 255],
                        };
                        if out.scale_fill.is_none() {
                            out.scale_fill = Some(rgb);
                            any = true;
                        }
                    }
                    None
                }
                CfRule::DataBar { min, max, color, gradient, show_value } => {
                    if let Some(n) = v.as_f64() {
                        let lo = threshold(wb, sheet, c, st, min);
                        let hi = threshold(wb, sheet, c, st, max);
                        let lo = if lo > 0.0 { 0.0 } else { lo };
                        let frac = if hi > lo { ((n - lo) / (hi - lo)).clamp(0.0, 1.0) } else { 1.0 };
                        if out.bar.is_none() {
                            out.bar = Some((
                                (frac as f32) * 0.9 + 0.1 * (frac > 0.0) as u8 as f32,
                                color.resolve(&wb.theme).unwrap_or([0x63, 0x8E, 0xC6]),
                                *gradient,
                            ));
                            out.hide_value |= !show_value;
                            any = true;
                        }
                    }
                    None
                }
                CfRule::IconSet { set, thresholds, reverse, show_value } => {
                    if let Some(n) = v.as_f64() {
                        let k = icon_count(set);
                        // One threshold per icon, the first being the lower bound (as in the file);
                        // also accept the lower bound left out, else Excel's defaults.
                        let defaults;
                        let lower = match thresholds.len() {
                            n if n == k => thresholds.get(1..).unwrap_or_default(),
                            n if n + 1 == k => thresholds.as_slice(),
                            _ => {
                                defaults = default_icon_thresholds(set);
                                defaults.get(1..).unwrap_or_default()
                            }
                        };
                        let cuts: Vec<f64> = lower.iter().map(|t| threshold(wb, sheet, c, st, t)).collect();
                        let mut idx = cuts.iter().filter(|cut| n >= **cut).count();
                        if *reverse {
                            idx = k - 1 - idx.min(k - 1);
                        }
                        if out.icon.is_none() {
                            out.icon = Some((set.clone(), idx.min(k - 1)));
                            out.hide_value |= !show_value;
                            any = true;
                        }
                    }
                    None
                }
                _ => None,
            });
            if let Some(st) = matched_style {
                any = true;
                // Every matching rule applies; where two set the same part, the rule with the
                // higher priority (seen first) wins.
                match &mut out.style {
                    None => out.style = Some(st.clone()),
                    Some(prev) => layer_under(prev, st),
                }
                if cf.stop_if_true {
                    break;
                }
            }
        }
        any.then_some(out)
    }

    /// Effective number format code of `c`, honouring matching conditional formatting `<dxf><numFmt>`
    /// overrides. Zero overhead when no conditional formatting rule covering `c` specifies a `numFmt`.
    pub fn effective_num_fmt<'a>(&mut self, wb: &'a Workbook, sheet: usize, sh: &'a Sheet, c: CellRef) -> std::borrow::Cow<'a, str> {
        let base = wb.styles.get(sh.style_id(c)).num_fmt.as_str();
        if sh.cond_formats.is_empty() {
            return std::borrow::Cow::Borrowed(base);
        }
        self.ensure_index(sh, sheet);
        if !self.index.as_ref().is_some_and(|idx| idx.has_num_fmt_at(c)) {
            return std::borrow::Cow::Borrowed(base);
        }
        if let Some(l) = self.look(wb, sheet, c)
            && let Some(st) = l.style
            && style_sets_num_fmt(&st)
        {
            return std::borrow::Cow::Owned(st.num_fmt.as_str().to_string());
        }
        std::borrow::Cow::Borrowed(base)
    }
}

/// Whether a differential style explicitly sets a number format override (non-General, or stated General).
pub(crate) fn style_sets_num_fmt(st: &Style) -> bool {
    st.num_fmt.as_str() != "General" || st.dxf_stated.num_fmt
}

/// Whether any conditional formatting rule on `sh` covering `c` carries a differential `numFmt`.
pub(crate) fn has_num_fmt_rule(sh: &Sheet, c: CellRef) -> bool {
    if sh.cond_formats.is_empty() {
        return false;
    }
    sh.cond_formats.iter().any(|cf| rule_style(&cf.rule).is_some_and(style_sets_num_fmt) && cf.ranges.iter().any(|r| r.contains(c)))
}

fn rule_style(rule: &CfRule) -> Option<&Style> {
    match rule {
        CfRule::CellIs { style, .. }
        | CfRule::Expression { style, .. }
        | CfRule::ContainsText { style, .. }
        | CfRule::NotContainsText { style, .. }
        | CfRule::BeginsWith { style, .. }
        | CfRule::EndsWith { style, .. }
        | CfRule::Blanks { style }
        | CfRule::NoBlanks { style }
        | CfRule::Errors { style }
        | CfRule::NoErrors { style }
        | CfRule::Duplicate { style }
        | CfRule::Unique { style }
        | CfRule::Top10 { style, .. }
        | CfRule::AboveAverage { style, .. }
        | CfRule::TimePeriod { style, .. } => Some(style.as_ref()),
        CfRule::ColorScale { .. } | CfRule::DataBar { .. } | CfRule::IconSet { .. } => None,
    }
}

/// Adds the parts `lower` sets that `style` leaves unset (differential styles: default parts
/// are unset).
fn layer_under(style: &mut Style, lower: &Style) {
    if style.align.reading_order.is_none() {
        style.align.reading_order = lower.align.reading_order;
    }
    if style.fill.pattern == PatternType::None {
        style.fill = lower.fill;
    }
    if style.font.color == Color::Auto {
        style.font.color = lower.font.color;
    }
    style.font.bold |= lower.font.bold;
    style.font.italic |= lower.font.italic;
    style.font.strike |= lower.font.strike;
    if style.font.underline == Underline::None {
        style.font.underline = lower.font.underline;
    }
    let b = &mut style.borders;
    for (dst, src) in [
        (&mut b.left, lower.borders.left),
        (&mut b.right, lower.borders.right),
        (&mut b.top, lower.borders.top),
        (&mut b.bottom, lower.borders.bottom),
        (&mut b.diag_down, lower.borders.diag_down),
        (&mut b.diag_up, lower.borders.diag_up),
    ] {
        if dst.is_none() {
            *dst = src;
        }
    }
    if !style_sets_num_fmt(style) {
        style.num_fmt = lower.num_fmt.clone();
        style.dxf_stated.num_fmt = lower.dxf_stated.num_fmt;
    }
}

/// Shifts a rule formula written for the top-left cell of the range to `c`.
fn relative(f: &str, first: CellRef, c: CellRef) -> String {
    if first == c {
        return f.to_string();
    }
    match gridcraft_formula::parse(f.trim_start_matches('=')) {
        Ok(e) => {
            gridcraft_formula::print(&gridcraft_formula::adjust::shift_relative(e, c.row as i64 - first.row as i64, c.col as i64 - first.col as i64))
        }
        Err(_) => f.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use gridcraft_model::CondFormat;
    use serde_json::json;

    use super::*;

    #[test]
    fn reading_order_respects_conditional_rule_priority_and_explicit_context() {
        for (higher, expected) in [
            (json!({"font": {"bold": true}}), "RightToLeft"),
            (json!({"align": {"reading_order": "Context"}}), "Context"),
            (json!({"align": {"reading_order": "LeftToRight"}}), "LeftToRight"),
        ] {
            let mut s = crate::Session::new();
            s.new_workbook();
            s.execute("cell.set", json!({"cell": "A1", "input": "مرحبا 123 ABC"})).unwrap();
            for style in [json!({"align": {"reading_order": "RightToLeft"}}), higher] {
                s.execute("home.conditionalFormat", json!({"range": "A1", "rule": {"type": "noBlanks", "style": style}})).unwrap();
            }
            let wb = &s.doc().unwrap().wb;
            let (reopened, _) = gridcraft_xlsx::read_xlsx(&gridcraft_xlsx::write_xlsx(wb).unwrap()).unwrap();
            for wb in [wb, &reopened] {
                let style = CfCache::new().look(wb, 0, CellRef::new(0, 0)).unwrap().style.unwrap();
                assert_eq!(serde_json::to_value(style.align).unwrap()["reading_order"], expected);
            }
        }
    }

    #[test]
    fn every_matching_rule_applies() {
        let mut s = crate::Session::new();
        s.new_workbook();
        s.execute("range.setValues", json!({"range": "A1", "values": [[200], [400], [510]]})).unwrap();
        let rule = |op: &str, value: &str, preset: &str| json!({"range": "A1:A3", "rule": {"type": "cellIs", "operator": op, "value": value, "preset": preset}});
        s.execute("home.conditionalFormat", rule("greaterThan", "300", "lightRedFill")).unwrap();
        s.execute("home.conditionalFormat", rule("greaterThan", "500", "redBorder")).unwrap();
        let d = s.doc().unwrap();
        let mut cache = CfCache::new();
        let mut look = |a: &str| cache.look(&d.wb, 0, CellRef::parse(a).unwrap()).and_then(|l| l.style);
        assert!(look("A1").is_none());
        let only_fill = look("A2").unwrap();
        assert_eq!(only_fill.fill.pattern, PatternType::Solid);
        assert!(only_fill.borders.top.is_none());
        let both = look("A3").unwrap();
        assert_eq!(both.fill, only_fill.fill);
        assert!(!both.borders.top.is_none());
    }

    #[test]
    fn cell_value_rules_on_blanks_and_errors() {
        let mut s = crate::Session::new();
        s.new_workbook();
        s.execute("range.setValues", json!({"range": "A1", "values": [[3, "=NA()"], [null, 7], ["=NA()", null], [" ", null], [7, null]]})).unwrap();
        let rule = |range: &str, r: serde_json::Value| json!({"range": range, "rule": r});
        s.execute("home.conditionalFormat", rule("A1:A5", json!({"type": "cellIs", "operator": "less", "value": "5", "preset": "lightRedFill"})))
            .unwrap();
        s.execute("home.conditionalFormat", rule("A1:A5", json!({"type": "noBlanks", "preset": "redBorder"}))).unwrap();
        s.execute("home.conditionalFormat", rule("B1:B2", json!({"type": "cellIs", "operator": "greater", "value": "5", "preset": "lightRedFill"})))
            .unwrap();
        let d = s.doc().unwrap();
        let mut cache = CfCache::new();
        let mut look = |a: &str| cache.look(&d.wb, 0, CellRef::parse(a).unwrap()).and_then(|l| l.style);
        let fill = |st: Option<Style>| st.is_some_and(|st| st.fill.pattern == PatternType::Solid);
        let border = |st: Option<Style>| st.is_some_and(|st| !st.borders.top.is_none());
        // Less than 5: 3 and the blank cell (as 0), not #N/A.
        assert_eq!([look("A1"), look("A2"), look("A3"), look("A5")].map(fill), [true, true, false, false]);
        // Greater than 5: 7, not #N/A.
        assert_eq!([look("B1"), look("B2")].map(fill), [false, true]);
        // No blanks: neither the empty cell nor the one holding a space.
        assert_eq!([look("A1"), look("A2"), look("A4")].map(border), [true, false, false]);
    }

    /// The statistics as every rule used to get them: one pass collecting both parts.
    fn all_stats_before(sh: &Sheet, ranges: &[RangeRef]) -> RuleStats {
        let mut nums = Vec::new();
        let mut counts = HashMap::new();
        for r in ranges {
            let Some(r) = sh.used_range().and_then(|u| r.intersection(&u)) else { continue };
            for c in r.iter() {
                let v = sh.value(c);
                if let Value::Number(n) = v {
                    nums.push(n);
                }
                if !v.is_empty() {
                    *counts.entry(v.display().to_lowercase()).or_insert(0) += 1;
                }
            }
        }
        nums.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let n = nums.len().max(1) as f64;
        let avg = nums.iter().sum::<f64>() / n;
        let std = (nums.iter().map(|x| (x - avg).powi(2)).sum::<f64>() / n).sqrt();
        RuleStats { sorted: nums, avg, std, counts }
    }

    #[test]
    fn needed_statistics_equal_the_full_ones() {
        let mut s = crate::Session::new();
        s.new_workbook();
        s.execute("range.setValues", json!({"range": "A1", "values": [[3, "x"], [7, "X"], [3, null], ["=NA()", 2.5], [" ", 9], [-1, "y"]]})).unwrap();
        let d = s.doc().unwrap();
        let sh = d.wb.sheet(0).unwrap();
        for ranges in ["A1:B6", "A1:A3", "B2:B9", "D1:E4", "A1:A2"].map(|r| vec![RangeRef::parse(r).unwrap()]) {
            let full = all_stats_before(sh, &ranges);
            let numbers = RuleStats::build(sh, &ranges, StatsNeed::Numbers);
            assert_eq!((numbers.sorted, numbers.avg.to_bits(), numbers.std.to_bits()), (full.sorted, full.avg.to_bits(), full.std.to_bits()));
            assert_eq!(RuleStats::build(sh, &ranges, StatsNeed::Counts).counts, full.counts);
        }
    }

    #[test]
    fn rules_that_read_statistics_still_match() {
        let mut s = crate::Session::new();
        s.new_workbook();
        s.execute("range.setValues", json!({"range": "A1", "values": [[1, "a"], [2, "A"], [3, "b"], [10, "c"], [2, null]]})).unwrap();
        let rule = |range: &str, r: serde_json::Value| json!({"range": range, "rule": r});
        s.execute("home.conditionalFormat", rule("B1:B5", json!({"type": "duplicate", "preset": "lightRedFill"}))).unwrap();
        s.execute("home.conditionalFormat", rule("A1:A5", json!({"type": "top10", "rank": 1, "preset": "redBorder"}))).unwrap();
        s.execute("home.conditionalFormat", rule("A1:A5", json!({"type": "colorScale"}))).unwrap();
        let d = s.doc().unwrap();
        let mut cache = CfCache::new();
        let mut look = |a: &str| cache.look(&d.wb, 0, CellRef::parse(a).unwrap());
        let fill = |l: Option<CfLook>| l.and_then(|l| l.style).is_some_and(|st| st.fill.pattern == PatternType::Solid);
        // Duplicates are case-insensitive texts: "a" and "A".
        assert_eq!(["B1", "B2", "B3", "B4", "B5"].map(|a| fill(look(a))), [true, true, false, false, false]);
        // The top value gets the border, the scale runs from the minimum to the maximum.
        assert!(look("A4").and_then(|l| l.style).is_some_and(|st| !st.borders.top.is_none()));
        assert!(look("A1").and_then(|l| l.style).is_none_or(|st| st.borders.top.is_none()));
        assert_eq!(look("A1").and_then(|l| l.scale_fill), Some([0xF8, 0x69, 0x6B]));
        assert_eq!(look("A4").and_then(|l| l.scale_fill), Some([0x63, 0xBE, 0x7B]));
    }

    /// Looking cells up does work in proportion to the cells asked for, not the rule's range: the
    /// statistics are only gathered for rules that read them, and once per rule.
    #[test]
    fn lookups_do_not_scan_the_range_for_rules_without_statistics() {
        let mut s = crate::Session::new();
        s.new_workbook();
        let rows: Vec<serde_json::Value> = (1..=10_000).map(|i| json!([i, i % 7])).collect();
        s.execute("range.setValues", json!({"range": "A1", "values": rows})).unwrap();
        s.execute(
            "home.conditionalFormat",
            json!({"range": "A1:B10000", "rule": {"type": "expression", "formula": "=MOD(ROW(),2)=0", "preset": "lightRedFill"}}),
        )
        .unwrap();
        s.execute(
            "home.conditionalFormat",
            json!({"range": "A1:B10000", "rule": {"type": "cellIs", "operator": "greater", "value": "5", "preset": "redBorder"}}),
        )
        .unwrap();
        s.execute("home.conditionalFormat", json!({"range": "A1:B10000", "rule": {"type": "noBlanks", "preset": "redBorder"}})).unwrap();
        let d = s.doc().unwrap();
        let mut cache = CfCache::new();
        SCANNED.with(|n| n.set(0));
        for r in 1..=50 {
            for col in ["A", "B"] {
                cache.look(&d.wb, 0, CellRef::parse(&format!("{col}{r}")).unwrap());
            }
        }
        assert_eq!(SCANNED.with(|n| n.get()), 0);
        // A rule that does read them scans its range once, however many cells are looked up.
        s.execute("home.conditionalFormat", json!({"range": "A1:A10000", "rule": {"type": "top10", "preset": "redBorder"}})).unwrap();
        let d = s.doc().unwrap();
        let mut cache = CfCache::new();
        SCANNED.with(|n| n.set(0));
        for r in 1..=50 {
            cache.look(&d.wb, 0, CellRef::parse(&format!("A{r}")).unwrap());
        }
        assert_eq!(SCANNED.with(|n| n.get()), 10_000);
    }

    /// A rule listing the same range many times scans it once, and never more than the shared
    /// limit however many ranges it holds.
    #[test]
    fn repeated_ranges_share_one_scan_budget() {
        let mut s = crate::Session::new();
        s.new_workbook();
        s.execute("range.setValues", json!({"range": "A1", "values": [[1], [2], [3]]})).unwrap();
        s.execute("range.setValues", json!({"range": "XFD1048576", "values": [[4]]})).unwrap();
        let d = s.doc().unwrap();
        let sh = d.wb.sheet(0).unwrap();
        let dense = RangeRef::parse("A1:XFD1048576").unwrap();
        let small = RangeRef::parse("A1:A3").unwrap();
        SCANNED.with(|n| n.set(0));
        let st = RuleStats::build(sh, &vec![small; 1000], StatsNeed::Numbers);
        assert_eq!(st.sorted, vec![1.0, 2.0, 3.0]);
        assert_eq!(SCANNED.with(|n| n.get()), 3);
        let shifted: Vec<RangeRef> = (0..50).map(|i| RangeRef::new(CellRef::new(i, 0), dense.end)).collect();
        SCANNED.with(|n| n.set(0));
        RuleStats::build(sh, &shifted, StatsNeed::Numbers);
        assert!(SCANNED.with(|n| n.get()) <= STATS_SCAN_LIMIT);
    }

    /// Blank cells of a sparse range are not charged to the scan budget: a value at the end of two
    /// million-row columns still counts, so duplicates are found.
    #[test]
    fn sparse_ranges_keep_their_values_under_the_scan_budget() {
        let mut s = crate::Session::new();
        s.new_workbook();
        s.execute("range.setValues", json!({"range": "A1", "values": [[7]]})).unwrap();
        s.execute("range.setValues", json!({"range": "B1048576", "values": [[7]]})).unwrap();
        let d = s.doc().unwrap();
        let sh = d.wb.sheet(0).unwrap();
        let ranges = [RangeRef::parse("A1:A1048576").unwrap(), RangeRef::parse("B1:B1048576").unwrap()];
        SCANNED.with(|n| n.set(0));
        let st = RuleStats::build(sh, &ranges, StatsNeed::Counts);
        assert_eq!(st.counts.get("7"), Some(&2));
        assert_eq!(SCANNED.with(|n| n.get()), 2);
    }

    /// Spills left and right of a narrow range, between its corners, are skipped a row at a time:
    /// they do not use the budget that the in-range spills need.
    #[test]
    fn spills_outside_a_narrow_range_do_not_use_its_scan_budget() {
        let mut s = crate::Session::new();
        s.new_workbook();
        let d = s.doc().unwrap();
        let mut sh = d.wb.sheet(0).unwrap().clone();
        for row in 0..1000 {
            for col in [0u32, 5, 6, 7] {
                sh.spill.insert(CellRef::new(row, col), Value::Number(1.0));
            }
        }
        let r = RangeRef::parse("F1:G1000").unwrap();
        let steps: Vec<_> = spill_steps(&sh, r).collect();
        assert_eq!(steps.iter().flatten().count(), 2000);
        // column A is skipped one step a row, never one per cell
        assert!(steps.len() <= 2000 + 1000, "{}", steps.len());
        let narrow = RangeRef::parse("C1:C1000").unwrap();
        assert_eq!(spill_steps(&sh, narrow).flatten().count(), 0);
    }

    /// A colour scale with an unsupported number of stops paints nothing and evaluates none of them.
    #[test]
    fn colour_scales_with_other_stop_counts_paint_nothing() {
        let mut s = crate::Session::new();
        s.new_workbook();
        s.execute("range.setValues", json!({"range": "A1", "values": [[1], [5]]})).unwrap();
        let mut wb: Workbook = (*s.doc().unwrap().wb).clone();
        let stop = (CfValueKind::Formula("=1".into()), Color::default());
        let rule = |n: usize| CondFormat {
            ranges: vec![RangeRef::parse("A1:A2").unwrap()],
            rule: CfRule::ColorScale { stops: vec![stop.clone(); n] },
            priority: 1,
            stop_if_true: false,
        };
        wb.sheet_mut(0).unwrap().cond_formats.push(rule(4));
        let mut cache = CfCache::new();
        assert_eq!(cache.look(&wb, 0, CellRef::parse("A1").unwrap()).and_then(|l| l.scale_fill), None);
        wb.sheet_mut(0).unwrap().cond_formats = vec![rule(2)];
        let mut cache = CfCache::new();
        assert!(cache.look(&wb, 0, CellRef::parse("A1").unwrap()).and_then(|l| l.scale_fill).is_some());
    }

    #[test]
    fn cell_value_operator_names() {
        let mut s = crate::Session::new();
        s.new_workbook();
        s.execute("range.setValues", json!({"range": "A1", "values": [[3], [7]]})).unwrap();
        let rule = |op: &str| json!({"range": "A1:A2", "rule": {"type": "cellIs", "operator": op, "value": "5", "preset": "lightRedFill"}});
        s.execute("home.conditionalFormat", rule("lessThan")).unwrap();
        assert!(s.execute("home.conditionalFormat", rule("smaller")).is_err());
        let d = s.doc().unwrap();
        let mut cache = CfCache::new();
        let mut look = |a: &str| cache.look(&d.wb, 0, CellRef::parse(a).unwrap()).and_then(|l| l.style).is_some();
        assert_eq!([look("A1"), look("A2")], [true, false]);
    }

    #[test]
    fn rules_compare_with_cells_on_other_sheets() {
        let mut s = crate::Session::new();
        s.new_workbook();
        s.execute("range.setValues", json!({"range": "E1", "values": [["Suspended"], ["OK"], ["Terminated"], [7], [3]]})).unwrap();
        let rule = |r: serde_json::Value| json!({"range": "E1:E5", "rule": r});
        s.execute(
            "home.conditionalFormat",
            rule(json!({"type": "cellIs", "operator": "equal", "value": "='Vehicle Range'!$A$8", "preset": "redText"})),
        )
        .unwrap();
        s.execute(
            "home.conditionalFormat",
            rule(json!({"type": "cellIs", "operator": "equal", "value": "='Vehicle Range'!$A$9", "preset": "redText"})),
        )
        .unwrap();
        s.execute(
            "home.conditionalFormat",
            rule(json!({"type": "expression", "formula": "=AND(ISNUMBER(E1),E1>'Vehicle Range'!$B$1)", "preset": "yellowFill"})),
        )
        .unwrap();
        s.execute("home.insertSheet", json!({"name": "Vehicle Range"})).unwrap();
        s.execute("range.setValues", json!({"range": "'Vehicle Range'!A8", "values": [["Suspended"], ["Terminated"]]})).unwrap();
        s.execute("range.setValues", json!({"range": "'Vehicle Range'!B1", "values": [[5]]})).unwrap();
        let d = s.doc().unwrap();
        let report = d.wb.sheets.iter().position(|sh| sh.name != "Vehicle Range").unwrap();
        let mut cache = CfCache::new();
        let mut look = |a: &str| cache.look(&d.wb, report, CellRef::parse(a).unwrap()).and_then(|l| l.style).unwrap_or_default();
        let looks = ["E1", "E2", "E3", "E4", "E5"].map(&mut look);
        // The cells equal to 'Vehicle Range'!A8 or A9 get the red text.
        let red_text = looks[0].font.color;
        assert_ne!(red_text, Color::Auto);
        let red = |st: &Style| st.font.color == red_text;
        let filled = |st: &Style| st.fill.pattern == PatternType::Solid;
        assert_eq!(looks.each_ref().map(red), [true, false, true, false, false]);
        assert_eq!(looks.each_ref().map(filled), [false, false, false, true, false]);
    }
    #[test]
    fn text_rules_skip_error_values() {
        let mut s = crate::Session::new();
        s.new_workbook();
        let values = json!([["=NA()", "=NA()", "=1/0", "=NA()"], ["abc", "#x", "x!", "abc"], ["xyz", "y", "y", "xyz"]]);
        s.execute("range.setValues", json!({"range": "A1", "values": values})).unwrap();
        let rule = |range: &str, kind: &str, text: &str| json!({"range": range, "rule": {"type": kind, "text": text, "preset": "lightRedFill"}});
        s.execute("home.conditionalFormat", rule("A1:A3", "containsText", "a")).unwrap();
        s.execute("home.conditionalFormat", rule("B1:B3", "beginsWith", "#")).unwrap();
        s.execute("home.conditionalFormat", rule("C1:C3", "endsWith", "!")).unwrap();
        s.execute("home.conditionalFormat", rule("D1:D3", "notContainsText", "a")).unwrap();
        let d = s.doc().unwrap();
        let mut cache = CfCache::new();
        let mut look = |a: &str| cache.look(&d.wb, 0, CellRef::parse(a).unwrap()).and_then(|l| l.style).is_some();
        // Contains "a": abc, never #N/A although its text holds an "a".
        assert_eq!([look("A1"), look("A2"), look("A3")], [false, true, false]);
        // Begins with "#": #x, never #N/A.
        assert_eq!([look("B1"), look("B2"), look("B3")], [false, true, false]);
        // Ends with "!": x!, never #DIV/0!.
        assert_eq!([look("C1"), look("C2"), look("C3")], [false, true, false]);
        // Does not contain "a": #N/A and xyz, not abc.
        assert_eq!([look("D1"), look("D2"), look("D3")], [true, false, true]);
    }

    #[test]
    fn spatial_index_handles_interval_tree_single_cells_numfmt_and_budget() {
        let mut wb = Workbook::new();
        let mk_fill = |rgb: u32| Style {
            fill: gridcraft_model::Fill { pattern: PatternType::Solid, fg: Color::Rgb(rgb), bg: Color::Auto },
            ..Default::default()
        };
        let mut num_style = mk_fill(0x112233);
        num_style.num_fmt = gridcraft_model::NumFmt("0.00%".into());
        let sh = wb.sheet_mut(0).unwrap();
        for r in 0..40u32 {
            sh.set_value(CellRef::new(r, 0), Value::Number(r as f64));
            sh.set_value(CellRef::new(r, 1), Value::Number(r as f64));
        }
        // 20 disjoint multi-cell row bands (forces non-leaf interval tree nodes) + 1 full-column rule
        // + 1 single-cell rule overlapping a multi-cell band + 1 numFmt rule.
        for i in 0..20u32 {
            sh.cond_formats.push(CondFormat {
                ranges: vec![RangeRef::new(CellRef::new(i * 2, 0), CellRef::new(i * 2 + 1, 1))],
                rule: CfRule::NoBlanks { style: Box::new(mk_fill(0xAA0000 + i)) },
                priority: 10 + i,
                stop_if_true: false,
            });
        }
        sh.cond_formats.push(CondFormat {
            ranges: vec![RangeRef::cols(0, 0)],
            rule: CfRule::NoBlanks { style: Box::new(mk_fill(0x00FF00)) },
            priority: 1,
            stop_if_true: false,
        });
        sh.cond_formats.push(CondFormat {
            ranges: vec![RangeRef::cell(CellRef::new(10, 1)), RangeRef::new(CellRef::new(10, 1), CellRef::new(11, 2))],
            rule: CfRule::NoBlanks { style: Box::new(num_style) },
            priority: 2,
            stop_if_true: false,
        });

        let mut cache = CfCache::new();
        // Column A gets priority-1 full-column fill (0x00FF00).
        let a10 = cache.look(&wb, 0, CellRef::new(10, 0)).unwrap().style.unwrap();
        assert_eq!(a10.fill.fg, Color::Rgb(0x00FF00));
        // B11 (row 10, col 1) gets priority-2 numFmt rule (0x112233) over row-band rule (priority 15).
        let b11 = cache.look(&wb, 0, CellRef::new(10, 1)).unwrap().style.unwrap();
        assert_eq!(b11.fill.fg, Color::Rgb(0x112233));
        assert_eq!(cache.effective_num_fmt(&wb, 0, wb.sheet(0).unwrap(), CellRef::new(10, 1)).as_ref(), "0.00%");
        assert_eq!(cache.effective_num_fmt(&wb, 0, wb.sheet(0).unwrap(), CellRef::new(10, 0)).as_ref(), "General");
        // Cell outside all ranges returns None and may_apply == false.
        assert!(!cache.may_apply(&wb, 0, CellRef::new(10, 5)));
        assert!(cache.look(&wb, 0, CellRef::new(10, 5)).is_none());

        // Budgeted lookup stops once exhausted.
        let mut budget = 1usize;
        assert!(cache.look_budgeted(&wb, 0, CellRef::new(10, 0), &mut budget).is_some());
        assert_eq!(budget, 0);
        assert!(cache.look_budgeted(&wb, 0, CellRef::new(10, 0), &mut budget).is_none());
    }
}
