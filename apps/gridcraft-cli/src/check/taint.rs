//! Formulas whose cached value can't be graded because of what they, or something they read,
//! depend on. The taint starts at the formulas (and defined names) that are a source themselves,
//! follows the dependency graph to every formula that reads a tainted cell, and follows defined
//! names, which the graph doesn't know: a name is tainted by its own definition, by a name it
//! uses, or by a tainted cell in the range it refers to, and so is every formula that uses it.

use super::foreign::Foreign;
use super::*;
use gridcraft_engine::calc::Area;
use gridcraft_engine::formula::adjust::has_relative_refs;

/// Functions whose result depends on the clock, a random generator or the environment.
const VOLATILE: [&str; 6] = ["RAND", "RANDARRAY", "RANDBETWEEN", "NOW", "TODAY", "INFO"];

/// Functions that find their result by iteration: Excel documents their accuracy (IRR, RATE… stop
/// at 0.00001 percent), not their last digit.
pub(super) const SOLVERS: [&str; 6] = ["IRR", "XIRR", "RATE", "YIELD", "ODDFYIELD", "ODDLYIELD"];

/// What makes a cached value ungradable, or gradable only to a tolerance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Source {
    /// The clock, a random generator or the environment: the value changes between two
    /// calculations of the same file.
    Volatile,
    /// An iterative solver ([`SOLVERS`]): graded to the accuracy Excel documents for it.
    Solver,
    /// Something Excel couldn't compute from the file alone: an add-in's or a user's function, a
    /// service, a linked workbook it has no values of (see [`Foreign`]).
    Foreign,
}

impl Source {
    pub(super) const ALL: [Source; 3] = [Source::Volatile, Source::Solver, Source::Foreign];

    /// Why `e` is itself such a source (what it reads is not considered).
    fn reason(self, e: &Expr, foreign: &Foreign<'_>, defined: &HashSet<String>) -> Option<&'static str> {
        match self {
            Source::Volatile => calls_volatile(e).then_some("volatile"),
            Source::Solver => calls(e, |n, _| SOLVERS.contains(&n)).then_some("solver"),
            Source::Foreign => foreign.reason(e, defined),
        }
    }
}

/// Whether `e` calls a function whose result depends on the clock, a random generator or the
/// environment ([`VOLATILE`], and `CELL("filename")`), converts a date text without a year, or
/// gives WEEKNUM a negative return type.
pub(super) fn calls_volatile(e: &Expr) -> bool {
    calls(e, |n, args| {
        let filename = n == "CELL" && matches!(args.first(), Some(Expr::Text(t)) if t.eq_ignore_ascii_case("filename"));
        filename || VOLATILE.contains(&n) || (n == "WEEKNUM" && args.get(1).is_some_and(is_negative_literal))
    }) || converts_yearless_date(e)
}

/// A negative number typed in the formula (`-1`). WEEKNUM documents return types 1, 2, 11-17 and
/// 21 and `#NUM!` for any other, and Excel's own result for a negative one is not reproducible:
/// Excel 16.114 calculated `WEEKNUM(DATE(2013,1,1),-1)` in four cells of one sheet twice and gave
/// 1 or `#NUM!` per cell and per calculation, `WEEKNUM(DATE(2013,6,1),-1)` `#NUM!` and then 22.
fn is_negative_literal(e: &Expr) -> bool {
    use gridcraft_engine::formula::UnOp;
    match e {
        Expr::Unary(UnOp::Neg, x) => matches!(**x, Expr::Number(n) if n > 0.0),
        Expr::Number(n) => *n < 0.0,
        _ => false,
    }
}

/// Functions that read a date argument given as text (`DATEVALUE("Feb 22")`, `DAY("01-Sep")`).
/// NUMBERVALUE is one: Excel 16.114 gives `NUMBERVALUE("Feb 22")` 46075 (22 Feb 2026, the year it
/// was calculated in) and `NUMBERVALUE("2020-01-01")` 43831, not `#VALUE!`.
const DATE_READERS: [&str; 17] = [
    "DATEVALUE",
    "VALUE",
    "DAY",
    "MONTH",
    "YEAR",
    "WEEKDAY",
    "WEEKNUM",
    "ISOWEEKNUM",
    "EDATE",
    "EOMONTH",
    "DATEDIF",
    "DAYS",
    "DAYS360",
    "NETWORKDAYS",
    "WORKDAY",
    "YEARFRAC",
    "NUMBERVALUE",
];

/// Whether `e` converts a literal date text that has no year (`"Feb 22"`, `"01-Sep"`) to a
/// number: an argument of a function that reads dates ([`DATE_READERS`]) or an operand of
/// arithmetic. Microsoft's DATEVALUE: "If the year portion of the date_text argument is omitted,
/// DATEVALUE uses the current year from your computer's built-in clock", and arithmetic and the
/// date functions read text dates the same way: the result changes with the year it is
/// calculated in. GridCraft's recognizer marks such a date with the `d-mmm` format it gives it.
fn converts_yearless_date(e: &Expr) -> bool {
    use gridcraft_engine::formula::{BinOp, UnOp};
    let yearless = |x: &Expr| {
        matches!(x, Expr::Text(t) if gridcraft_engine::core::parse::parse_date_time_text(t, gridcraft_engine::core::DateSystem::D1900)
            .is_some_and(|(_, format)| format == "d-mmm"))
    };
    let mut found = false;
    e.walk(&mut |x| {
        found |= match x {
            Expr::Call(n, args) => DATE_READERS.contains(&n.as_str()) && args.iter().any(yearless),
            Expr::Binary(BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Pow, a, b) => yearless(a) || yearless(b),
            Expr::Unary(UnOp::Neg | UnOp::Plus | UnOp::Percent, a) => yearless(a),
            _ => false,
        }
    });
    found
}

/// Whether a call anywhere in `e` satisfies `f`, which gets the function name and its arguments.
fn calls(e: &Expr, f: impl Fn(&str, &[Expr]) -> bool) -> bool {
    let mut found = false;
    e.walk(&mut |x| {
        if let Expr::Call(n, args) = x {
            found |= f(n, args);
        }
    });
    found
}

/// Cells indexed by (sheet, column) → rows, for area queries.
pub(super) struct CellIndex {
    cols: HashMap<(usize, u32), BTreeSet<u32>>,
    /// Columns with cells per sheet (for very wide areas).
    sheet_cols: HashMap<usize, BTreeSet<u32>>,
}

impl CellIndex {
    pub(super) fn new(keys: impl IntoIterator<Item = Key>) -> CellIndex {
        let mut cols: HashMap<(usize, u32), BTreeSet<u32>> = HashMap::new();
        let mut sheet_cols: HashMap<usize, BTreeSet<u32>> = HashMap::new();
        for (s, c) in keys {
            cols.entry((s, c.col)).or_default().insert(c.row);
            sheet_cols.entry(s).or_default().insert(c.col);
        }
        CellIndex { cols, sheet_cols }
    }

    /// Whether a cell other than `own` lies in the area.
    pub(super) fn any_in(&self, a: &Area, own: Option<Key>) -> bool {
        let (r0, r1, c0, c1) = (a.range.start.row, a.range.end.row, a.range.start.col, a.range.end.col);
        let Some(cols) = self.sheet_cols.get(&a.sheet) else { return false };
        cols.range(c0..=c1).any(|&col| {
            self.cols.get(&(a.sheet, col)).is_some_and(|rows| rows.range(r0..=r1).any(|&row| Some((a.sheet, CellRef::new(row, col))) != own))
        })
    }
}

/// A defined name as the formulas of one sheet see it.
#[derive(Default)]
struct NameNode {
    expr: Option<std::sync::Arc<Expr>>,
    /// Names the definition uses.
    uses: Vec<usize>,
    /// Cells the definition refers to.
    areas: Vec<Area>,
}

/// The defined names of a workbook, resolved the way formulas resolve them: a name scoped to the
/// sheet the formula is on hides a workbook-scoped one of the same name. A node is a name together
/// with the sheet it is used from (that sheet matters to a workbook-scoped name only, whose
/// unqualified references and nested names are the using sheet's).
struct Names<'a> {
    wb: &'a Workbook,
    /// (scope, UPPER-CASED name) → index into `wb.names`; the first definition wins.
    by_name: HashMap<(Option<usize>, String), usize>,
    /// Upper-cased names of any scope, to skip the lookups of built-in function calls.
    upper: HashSet<String>,
    parsed: Vec<Option<std::sync::Arc<Expr>>>,
    ids: HashMap<(usize, usize), usize>,
    nodes: Vec<NameNode>,
    todo: Vec<(usize, usize, usize)>,
}

impl<'a> Names<'a> {
    fn new(wb: &'a Workbook) -> Names<'a> {
        let mut by_name = HashMap::new();
        let mut upper = HashSet::new();
        let mut parsed = Vec::with_capacity(wb.names.len());
        for (i, n) in wb.names.iter().enumerate() {
            let up = n.name.to_ascii_uppercase();
            upper.insert(up.clone());
            by_name.entry((n.scope, up)).or_insert(i);
            parsed.push(gridcraft_engine::formula::parse_name(&n.formula).ok().map(std::sync::Arc::new));
        }
        Names { wb, by_name, upper, parsed, ids: HashMap::new(), nodes: Vec::new(), todo: Vec::new() }
    }

    /// The defined name `n` (as written in a formula on `sheet`) refers to: `Sheet!Name` names the
    /// sheet's own, otherwise the formula's sheet first, then the workbook.
    fn resolve(&self, sheet: usize, n: &str) -> Option<usize> {
        if n.starts_with('[') {
            return None; // a name of a linked workbook
        }
        let (scope_sheet, bare) = match n.rsplit_once('!') {
            Some((s, b)) => (self.wb.sheet_index(s.trim_matches('\''))?, b),
            None => (sheet, n),
        };
        let up = bare.to_ascii_uppercase();
        let found = self.by_name.get(&(Some(scope_sheet), up.clone())).or_else(|| self.by_name.get(&(None, up)));
        found.copied()
    }

    /// The node of name `idx` used from `sheet`; its definition is read by [`Names::fill`].
    fn node_for(&mut self, idx: usize, sheet: usize) -> usize {
        let ctx = self.wb.names.get(idx).and_then(|n| n.scope).unwrap_or(sheet);
        if let Some(&id) = self.ids.get(&(idx, ctx)) {
            return id;
        }
        let id = self.nodes.len();
        self.ids.insert((idx, ctx), id);
        self.nodes.push(NameNode::default());
        self.todo.push((id, idx, ctx));
        id
    }

    /// Nodes of the names `e`, a formula on `sheet`, uses (`LAMBDA` names called as functions
    /// included), without repeats.
    fn used_by(&mut self, sheet: usize, e: &Expr) -> Vec<usize> {
        if self.upper.is_empty() {
            return Vec::new();
        }
        let mut found: Vec<usize> = Vec::new();
        e.walk(&mut |x| {
            let idx = match x {
                Expr::Name(n) => self.resolve(sheet, n),
                Expr::Call(n, _)
                    if self.upper.contains(n.as_str())
                        || (n.bytes().any(|b| b.is_ascii_lowercase()) && self.upper.contains(&n.to_ascii_uppercase())) =>
                {
                    self.resolve(sheet, n)
                }
                _ => None,
            };
            if let Some(i) = idx
                && !found.contains(&i)
            {
                found.push(i);
            }
        });
        let nodes = found.into_iter().map(|i| self.node_for(i, sheet)).collect();
        self.fill();
        nodes
    }

    /// Reads the definitions of the nodes made since the last call (and, through them, of the
    /// names they use).
    fn fill(&mut self) {
        while let Some((id, idx, ctx)) = self.todo.pop() {
            let Some(expr) = self.parsed.get(idx).cloned().flatten() else { continue };
            // A name with relative references refers to cells that depend on where it is used.
            let areas = if has_relative_refs(&expr) { Vec::new() } else { gridcraft_engine::calc::eval::precedents(self.wb, ctx, &expr).0 };
            let mut inner: Vec<usize> = Vec::new();
            expr.walk(&mut |x| {
                if let Expr::Name(n) = x
                    && let Some(i) = self.resolve(ctx, n)
                    && !inner.contains(&i)
                {
                    inner.push(i);
                }
            });
            let uses: Vec<usize> = inner.into_iter().map(|i| self.node_for(i, ctx)).collect();
            if let Some(node) = self.nodes.get_mut(id) {
                *node = NameNode { expr: Some(expr), uses, areas };
            }
        }
    }
}

/// The formulas of a workbook with what they use and where taint can start.
pub(super) struct Scan<'a> {
    names: Names<'a>,
    foreign: Foreign<'a>,
    /// Formulas that use defined names → the nodes they use.
    uses: HashMap<Key, Vec<usize>>,
    /// Per name node: the formulas that use it.
    readers: Vec<Vec<Key>>,
    /// Per node: the nodes that use it.
    used_by: Vec<Vec<usize>>,
    /// The formulas that are a source themselves, per [`Source`], with why.
    seeds: [Vec<(Key, &'static str)>; Source::ALL.len()],
}

/// The cells tainted by one source.
pub(super) struct Spread {
    pub cells: HashSet<Key>,
    /// Of `cells`, the ones reached through a defined name (the dependency graph doesn't see them).
    pub via_names: usize,
}

impl<'a> Scan<'a> {
    pub(super) fn new(wb: &'a Workbook) -> Scan<'a> {
        let mut names = Names::new(wb);
        let foreign = Foreign::new(wb);
        let mut uses: HashMap<Key, Vec<usize>> = HashMap::new();
        let mut seeds: [Vec<(Key, &'static str)>; Source::ALL.len()] = std::array::from_fn(|_| Vec::new());
        for (si, sh) in wb.sheets.iter().enumerate() {
            for (c, cell) in sh.cells.iter() {
                let Some(e) = cell.formula.as_ref().and_then(|f| f.expr_arc()) else { continue };
                for (s, seed) in Source::ALL.iter().zip(seeds.iter_mut()) {
                    if let Some(why) = s.reason(&e, &foreign, &names.upper) {
                        seed.push(((si, c), why));
                    }
                }
                let used = names.used_by(si, &e);
                if !used.is_empty() {
                    uses.insert((si, c), used);
                }
            }
        }
        let mut readers = vec![Vec::new(); names.nodes.len()];
        for (k, used) in &uses {
            for &n in used {
                if let Some(r) = readers.get_mut(n) {
                    r.push(*k);
                }
            }
        }
        let mut used_by = vec![Vec::new(); names.nodes.len()];
        for (i, node) in names.nodes.iter().enumerate() {
            for &u in &node.uses {
                if let Some(v) = used_by.get_mut(u) {
                    v.push(i);
                }
            }
        }
        Scan { names, foreign, uses, readers, used_by, seeds }
    }

    /// Every formula tainted by `source`: the sources, whatever reads them through the dependency
    /// graph, and whatever reads them through a defined name. A source formula for which `kept`
    /// holds (its value is the one the file saved, kept by the engine) taints nothing it feeds.
    pub(super) fn spread(&self, calc: &Calc, wb: &Workbook, source: Source, kept: impl Fn(Key) -> bool) -> Spread {
        let seeds = self.seeds.get(source as usize).map(Vec::as_slice).unwrap_or_default();
        let cells: HashSet<Key> = seeds.iter().map(|(k, _)| *k).filter(|k| !kept(*k)).collect();
        self.spread_from(calc, wb, cells, Some(source))
    }

    /// Every formula tainted by the formulas `cells`: them, whatever reads them through the
    /// dependency graph, and whatever reads them through a defined name; with `source`, also every
    /// formula that uses a name whose own definition is one.
    pub(super) fn spread_from(&self, calc: &Calc, wb: &Workbook, mut cells: HashSet<Key>, source: Option<Source>) -> Spread {
        let mut stack: Vec<Key> = cells.iter().copied().collect();
        let mut deps = Vec::new();
        let mut flow = |cells: &mut HashSet<Key>, stack: &mut Vec<Key>| {
            while let Some((s, c)) = stack.pop() {
                deps.clear();
                calc.graph.dependents(s, c, &mut deps);
                if let Some(range) = wb.sheet(s).and_then(|sh| sh.spill_ranges.get(&c)) {
                    calc.graph.dependents_of_range(s, *range, &mut deps);
                }
                for k in deps.drain(..) {
                    if cells.insert(k) {
                        stack.push(k);
                    }
                }
            }
        };
        flow(&mut cells, &mut stack);
        let through_graph = cells.len();

        let nodes = &self.names.nodes;
        let mut tainted: Vec<bool> = nodes
            .iter()
            .map(|n| source.is_some_and(|s| n.expr.as_ref().is_some_and(|e| s.reason(e, &self.foreign, &self.names.upper).is_some())))
            .collect();
        let mut queue: Vec<usize> = (0..nodes.len()).filter(|&i| tainted.get(i).copied().unwrap_or(false)).collect();
        let mut done = vec![false; nodes.len()];
        loop {
            // A name that uses a tainted name is tainted.
            while let Some(i) = queue.pop() {
                for &u in self.used_by.get(i).map(Vec::as_slice).unwrap_or_default() {
                    if let Some(t) = tainted.get_mut(u)
                        && !*t
                    {
                        *t = true;
                        queue.push(u);
                    }
                }
            }
            // So is every formula that uses one.
            for (i, t) in tainted.iter().enumerate() {
                if *t && !done.get(i).copied().unwrap_or(true) {
                    if let Some(d) = done.get_mut(i) {
                        *d = true;
                    }
                    for &k in self.readers.get(i).map(Vec::as_slice).unwrap_or_default() {
                        if cells.insert(k) {
                            stack.push(k);
                        }
                    }
                }
            }
            flow(&mut cells, &mut stack);
            // A name whose range holds a tainted cell is tainted.
            if tainted.iter().all(|t| *t) {
                break;
            }
            let index = CellIndex::new(cells.iter().copied());
            let mut found = false;
            for (i, node) in nodes.iter().enumerate() {
                if tainted.get(i).copied().unwrap_or(true) || !node.areas.iter().any(|a| index.any_in(a, None)) {
                    continue;
                }
                if let Some(t) = tainted.get_mut(i) {
                    *t = true;
                }
                queue.push(i);
                found = true;
            }
            if !found {
                break;
            }
        }
        Spread { via_names: cells.len().saturating_sub(through_graph), cells }
    }

    /// Why each formula that is a `source` itself is one (the ones in a [`Spread`] that aren't
    /// are tainted by what they read).
    pub(super) fn seed_reasons(&self, source: Source) -> HashMap<Key, &'static str> {
        self.seeds.get(source as usize).map(|v| v.iter().copied().collect()).unwrap_or_default()
    }

    /// Whether `f` holds for an area that formula `k` reads through a defined name (through the
    /// names those names use too).
    pub(super) fn any_name_area(&self, k: Key, f: &mut dyn FnMut(&Area) -> bool) -> bool {
        let Some(first) = self.uses.get(&k) else { return false };
        let mut seen: HashSet<usize> = HashSet::new();
        let mut stack: Vec<usize> = first.clone();
        while let Some(i) = stack.pop() {
            if !seen.insert(i) {
                continue;
            }
            let Some(node) = self.names.nodes.get(i) else { continue };
            if node.areas.iter().any(&mut *f) {
                return true;
            }
            stack.extend(node.uses.iter().copied());
        }
        false
    }
}
