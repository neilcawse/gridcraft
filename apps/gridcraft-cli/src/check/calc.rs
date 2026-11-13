use super::*;
use gridcraft_engine::core::RangeRef;

use std::sync::Arc;

use gridcraft_engine::calc::eval::OneCellReading;
use gridcraft_engine::calc::recalc::{evaluate_expr, evaluate_expr_as};
use gridcraft_engine::model::Formula;

use super::foreign::why;
use super::taint::{CellIndex, Scan, Source, calls_volatile};
use gridcraft_engine::core::locale::Locale;

// ---------------------------------------------------------------- calc oracle

fn functions_of(e: &Expr) -> Vec<String> {
    let mut set = BTreeSet::new();
    e.walk(&mut |x| {
        if let Expr::Call(n, _) = x {
            set.insert(n.clone());
        }
    });
    set.into_iter().collect()
}

/// The formula as the formula bar shows it.
pub(super) fn formula_text(cell: &Cell) -> String {
    cell.bar_text()
}

fn cell_name(wb: &Workbook, (sheet, c): Key) -> String {
    let name = wb.sheet(sheet).map(|s| s.name.as_str()).unwrap_or("?");
    format!("{}!{}", quote_sheet(name), c.a1())
}

/// Whether the producer cached GridCraft's logical result as the number 1 or 0. Excel types a
/// logical result `t="b"`; a writer that leaves the type out (`<c><f>TRUE()</f><v>1</v></c>`, as
/// Business Central's "Open in Excel" exports do) stores the same value, read back as a number.
pub fn bool_cached_as_number(expected: &Value, actual: &Value) -> bool {
    matches!((expected, actual), (Value::Number(n), Value::Bool(b)) if *n == if *b { 1.0 } else { 0.0 })
}

/// The error a Google Sheets text cache entry spells: `#N/A`, or `#N/A:lookupNotFound:Attract`,
/// the form Google Sheets writes, which gives the reason after the code (the code is what the
/// cell shows).
fn google_error_text(s: &str) -> Option<CellError> {
    let code = s.split_once(':').map_or(s, |(code, _)| code);
    [
        CellError::Div0,
        CellError::NA,
        CellError::Value,
        CellError::Ref,
        CellError::Name,
        CellError::Num,
        CellError::Null,
        CellError::GettingData,
        CellError::Calc,
        CellError::Spill,
    ]
    .into_iter()
    .find(|e| e.as_str() == code)
}

/// Whether `expected`, cached by a `producer` of that kind, is the error `actual` written as
/// text: Google Sheets saves an error as `#CODE` or `#CODE:reason:…` text in a `t="str"` cell.
/// Any other producer's text is text, whatever it spells: Excel writes an error as `t="e"`, so a
/// text cache that reads like one is a text result, and a different value than the error.
pub(super) fn error_text_cached(producer: ProducerKind, expected: &Value, actual: &Value) -> bool {
    producer == ProducerKind::GoogleSheets && matches!((expected, actual), (Value::Text(s), Value::Error(e)) if google_error_text(s) == Some(*e))
}

/// The whitespace XML treats as insignificant around element text.
fn trim_xml_space(s: &str) -> &str {
    s.trim_matches([' ', '\t', '\r', '\n'])
}

/// Whether the text `expected`, cached by a `producer` of that kind, is the text `actual` with
/// the whitespace around it dropped, as a writer other than Excel (WPS, LibreOffice) can save
/// it: Excel writes such a value with `xml:space="preserve"` (Excel 16.114 saves `="x "&""` as
/// `<v xml:space="preserve">x </v>`), so an Excel cache without the whitespace is a result
/// without it. The caller checks the cell's `<v>` has no `xml:space="preserve"` either.
fn lost_surrounding_space(producer: ProducerKind, expected: &Value, actual: &Value) -> bool {
    producer.is_foreign() && matches!((expected, actual), (Value::Text(a), Value::Text(b)) if a != b && **a == *trim_xml_space(b))
}

/// Relative tolerance for the results of iterative solvers (IRR, XIRR, RATE, YIELD…): Excel
/// documents their accuracy as 0.00001 percent, so two correct implementations, or two versions
/// of Excel, agree to about 1e-8 and not to the last digit.
pub(super) const SOLVER_TOLERANCE: f64 = 1e-7;

pub fn values_match(expected: &Value, actual: &Value, tolerance: f64) -> bool {
    match (expected, actual) {
        (Value::Number(a), Value::Number(b)) => (a - b).abs() <= tolerance * a.abs().max(b.abs()).max(1.0),
        (Value::Text(a), Value::Text(b)) => a == b,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Error(a), Value::Error(b)) => a == b,
        _ => false,
    }
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Empty => "blank",
        Value::Number(_) => "number",
        Value::Text(_) => "text",
        Value::Bool(_) => "bool",
        Value::Error(e) => e.as_str(),
        Value::Array(_) => "array",
    }
}

fn mismatch_kind(expected: &Value, actual: &Value) -> &'static str {
    match (expected, actual) {
        (_, Value::Empty) => "blank",
        (Value::Number(_), Value::Number(_)) => "numeric",
        (Value::Text(_), Value::Text(_)) => "text",
        (Value::Bool(_), Value::Bool(_)) => "bool",
        (Value::Error(_), _) | (_, Value::Error(_)) => "error",
        _ => "type",
    }
}

/// Captures the producer-cached scalar values of formula cells before recalculation so `check`
/// can recalculate the workbook in place without copy-on-write cloning every formula band.
pub(super) fn snapshot_formula_values(wb: &Workbook) -> HashMap<Key, Value> {
    let mut out = HashMap::new();
    for (si, sh) in wb.sheets.iter().enumerate() {
        for (c, cell) in sh.cells.iter() {
            if cell.formula.is_some() && !cell.value.is_empty() {
                out.insert((si, c), cell.value.clone());
            }
        }
    }
    out
}

/// Whether two numbers differ by more than the order of evaluation can make them: by more than
/// [`SOLVER_TOLERANCE`] of the larger, or of 1 near zero (a sum that cancels to 2.8e-9 where
/// Excel shows 0 is rounding, not a different calculation).
fn differs_beyond_rounding(expected: &Value, actual: &Value) -> bool {
    matches!((expected, actual), (Value::Number(e), Value::Number(a)) if (e - a).abs() > SOLVER_TOLERANCE * e.abs().max(a.abs()).max(1.0))
}

/// `fresh` as the file saved it: every formula cell holds the value the producer cached for it,
/// nothing where it cached none; every other cell is the file's own already.
fn as_saved(fresh: &Workbook, saved: &HashMap<Key, Value>) -> Workbook {
    let mut wb = fresh.clone();
    for (si, sh) in wb.sheets.iter_mut().enumerate() {
        let sh = std::sync::Arc::make_mut(sh);
        let formulas: Vec<CellRef> = sh.cells.iter().filter(|(_, cell)| cell.formula.is_some()).map(|(c, _)| c).collect();
        for c in formulas {
            if let Some(cell) = sh.cells.get_mut(c) {
                cell.value = saved.get(&(si, c)).cloned().unwrap_or_default();
            }
        }
    }
    wb
}

/// `fresh` with each number GridCraft calculated within `tolerance` of the one the file cached
/// holding the cached digits: the two are one value to the digits the comparison looks at, and a
/// formula that multiplies the difference (`H7*H28`, 91 times a tolerated 2e-10) reads what the
/// calculating Excel computed it from. A number that differs by more than that, and a cell the
/// file didn't cache, keep GridCraft's value: nothing proves Excel's agrees with it.
fn as_agreed(fresh: &Workbook, saved: &HashMap<Key, Value>, tolerance: f64) -> Workbook {
    let mut wb = fresh.clone();
    for (&(si, c), was) in saved {
        let Some(sh) = wb.sheets.get_mut(si) else { continue };
        let agreed = sh
            .cell(c)
            .is_some_and(|x| matches!((was, &x.value), (Value::Number(a), Value::Number(b)) if a != b) && values_match(was, &x.value, tolerance));
        if agreed && let Some(cell) = Arc::make_mut(sh).cells.get_mut(c) {
            cell.value = was.clone();
        }
    }
    wb
}

/// The volatile formulas of `wb` ([`calls_volatile`]) that saved a value, taken out so that a
/// recalculation reads their saved values: `RAND()`, `TODAY()` and the like hold what they held
/// when Excel last calculated, and every formula computed from them gets the inputs Excel's did,
/// which makes its cache an oracle again. An array or dynamic-array formula is left in (its other
/// cells weren't saved as its value). [`unpin_volatile`] puts the formulas back.
pub(super) fn pin_volatile(wb: &mut Workbook, saved: &HashMap<Key, Value>) -> Vec<(Key, Arc<Formula>)> {
    let mut pinned = Vec::new();
    for (si, sh) in wb.sheets.iter_mut().enumerate() {
        let seeds: Vec<CellRef> = sh
            .cells
            .iter()
            .filter(|(c, cell)| {
                cell.formula.as_ref().is_some_and(|f| f.array.is_none() && !f.dynamic && f.expr_arc().is_some_and(|e| calls_volatile(&e)))
                    && saved.get(&(si, *c)).is_some_and(|v| !v.is_empty())
            })
            .map(|(c, _)| c)
            .collect();
        if seeds.is_empty() {
            continue;
        }
        let sh = Arc::make_mut(sh);
        for c in seeds {
            if let Some(f) = sh.cells.get_mut(c).and_then(|cell| cell.formula.take()) {
                pinned.push(((si, c), f));
            }
        }
    }
    pinned
}

/// Puts back the formulas [`pin_volatile`] took out (their cells kept the saved values).
pub(super) fn unpin_volatile(wb: &mut Workbook, pinned: Vec<(Key, Arc<Formula>)>) -> HashSet<Key> {
    let mut keys = HashSet::with_capacity(pinned.len());
    for ((si, c), f) in pinned {
        if let Some(cell) = wb.sheets.get_mut(si).map(Arc::make_mut).and_then(|sh| sh.cells.get_mut(c)) {
            cell.formula = Some(f);
            keys.insert((si, c));
        }
    }
    keys
}

/// How a producer-cached value compares with the recalculated one.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Grade {
    /// Equal, to the tolerance.
    Same,
    /// The producer saved TRUE/FALSE as 1/0 without `t="b"` ([`bool_cached_as_number`]).
    BoolAsNumber,
    /// Google Sheets cached an error as `#CODE` or `#CODE:reason:…` text ([`error_text_cached`]).
    ErrorDetail,
    /// Within a solver's documented accuracy carried through the formula ([`settle_undecided`]).
    Solver,
    /// Text a writer other than Excel saved without the whitespace around the result
    /// ([`lost_surrounding_space`]).
    Whitespace,
    /// Reproduced exactly when the workbook is recalculated under another locale ([`locale`]).
    Locale,
    /// Within the rounding the formula's operations propagate ([`fresh_bound`]).
    CancellationResidue,
    /// On an iterative loop, a state Excel's stopping rule leaves ([`settle_undecided`]).
    IterationChange,
    Differs,
}

impl Grade {
    /// Counts a match on another grade than [`Grade::Same`] in `r`, returning the grade's name
    /// as `--cells` writes it (`matched_alt:<name>`).
    fn tally(self, r: &mut CalcReport) -> Option<&'static str> {
        let (count, name) = match self {
            Grade::BoolAsNumber => (&mut r.bool_as_number, "boolAsNumber"),
            Grade::ErrorDetail => (&mut r.error_detail_cache, "errorDetailCache"),
            Grade::Solver => (&mut r.solver_tolerance, "solverTolerance"),
            Grade::Whitespace => (&mut r.whitespace_cache, "whitespaceCache"),
            Grade::Locale => (&mut r.locale_dependent, "localeDependent"),
            Grade::CancellationResidue => (&mut r.cancellation_residue, "cancellationResidue"),
            Grade::IterationChange => (&mut r.iteration_change, "iterationChange"),
            Grade::Same | Grade::Differs => return None,
        };
        *count += 1;
        Some(name)
    }
}

/// The expression of the formula at `k` when its cell holds the first value the expression gives,
/// as a scalar formula's cell holds its value: a Ctrl+Shift+Enter array's anchor does (its other
/// cells aren't formulas). A dynamic-array formula's doesn't count: what it spills reads as
/// GridCraft calculated it, not as the file saved it.
fn anchored_formula(wb: &Workbook, (si, c): Key) -> Option<Arc<Expr>> {
    let f = wb.sheet(si)?.cell(c)?.formula.as_ref()?;
    if f.dynamic {
        return None;
    }
    f.expr_arc()
}

/// The expression of the formula at `k` when it computes one value: not an array or
/// dynamic-array formula.
fn scalar_formula(wb: &Workbook, (si, c): Key) -> Option<Arc<Expr>> {
    let f = wb.sheet(si)?.cell(c)?.formula.as_ref()?;
    if f.array.is_some() {
        return None;
    }
    anchored_formula(wb, (si, c))
}

/// A cell's value as arithmetic reads it: a number, or 0 for an empty cell; `None` for anything
/// else (converting text or a logical is semantics, not rounding).
fn arithmetic_value(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => Some(*n),
        Value::Empty => Some(0.0),
        _ => None,
    }
}

fn value_at(wb: &Workbook, (si, c): Key) -> Value {
    wb.sheet(si).and_then(|s| s.cell(c)).map(|x| x.value.scalar()).unwrap_or_default()
}

/// The rounding bound ([`super::noise::Bounds`]) of the formula `e` at `k` as GridCraft
/// calculated it in `fresh`: a constant it reads is exact; a formula it reads carries the
/// difference between its cache and GridCraft's value when that is within `tolerance`, or when
/// the cell is a match already proved (`accepted`, with its grade: a [`Grade::Solver`] carries a
/// solver's accuracy); a solver call carries [`SOLVER_TOLERANCE`] of its result. Anything else it
/// reads, a mismatch still undecided among it, leaves it unbounded: what such an input differs by
/// may be GridCraft's error, which no reader of it explains.
fn fresh_bound(
    fresh: &Workbook,
    saved: &HashMap<Key, Value>,
    accepted: &HashMap<Key, Grade>,
    tolerance: f64,
    k: Key,
    e: &Expr,
    budget: &mut usize,
) -> Option<super::noise::Bounded> {
    use super::noise::{Bounded, Bounds};
    let mut input = |q: Key| carried_input(fresh, saved, accepted, tolerance, q);
    let mut solve = |name: &str, call: &Expr| {
        let v = super::taint::SOLVERS.contains(&name).then(|| call_value(fresh, k, call))??;
        Some(Bounded { value: v, err: SOLVER_TOLERANCE * v.abs().max(1.0), solver: true })
    };
    Bounds { wb: fresh, at: k, input: &mut input, call: Some(&mut solve), budget }.of(e)
}

/// The value GridCraft calculated in `fresh` for a cell `q` a bounded formula reads, with what
/// it can differ from Excel's by (see [`fresh_bound`]): a constant is exact; a formula carries
/// the difference between its cache and GridCraft's value when that is within `tolerance` or the
/// cell is a proved match (`accepted`); anything else is unbounded.
fn carried_input(
    fresh: &Workbook,
    saved: &HashMap<Key, Value>,
    accepted: &HashMap<Key, Grade>,
    tolerance: f64,
    q: Key,
) -> Option<super::noise::Bounded> {
    use super::noise::Bounded;
    let cell = fresh.sheet(q.0).and_then(|s| s.cell(q.1));
    let now = arithmetic_value(&value_at(fresh, q))?;
    if !cell.is_some_and(|x| x.formula.is_some()) {
        return Some(Bounded::exact(now));
    }
    let was = arithmetic_value(saved.get(&q)?)?;
    let err = (was - now).abs();
    if values_match(&Value::Number(was), &Value::Number(now), tolerance) {
        return Some(Bounded { value: now, err, solver: false });
    }
    accepted.get(&q).map(|g| Bounded { value: now, err, solver: *g == Grade::Solver })
}

/// GridCraft's number for the call `call` in the formula at `k`.
fn call_value(fresh: &Workbook, k: Key, call: &Expr) -> Option<f64> {
    match evaluate_expr(fresh, k.0, k.1, call).scalar() {
        Value::Number(v) => Some(v),
        _ => None,
    }
}

/// How many times [`settle_undecided`] looks again at the mismatches still undecided: each pass
/// settles the cells whose differing inputs the one before settled.
const MAX_SETTLE_PASSES: usize = 16;

/// Grades the number mismatches of `graded` that the calculation leaves undecided (see
/// [`super::noise`]):
/// - on an iterative loop, within Maximum Change and a state Excel's stopping rule leaves
///   ([`super::noise::iteration_explains`] on the file's saved state);
/// - within the rounding error the formula's operations propagate from what it reads
///   ([`fresh_bound`]): cancellation residue, or a solver's accuracy when a solver's result is
///   among what the error is made of;
/// - a reader of a solver (`solver`) whose formula, on the file's saved values, gives its cache
///   back, and whose every input that differs from its cache is a solver mismatch settled here:
///   what differs is the solver's accuracy, carried in.
///
/// An input carries its difference into a reader only once its own grade is a match (`accepted`),
/// so the passes go on while one settles cells: a mismatch left undecided explains none of its
/// readers' differences.
#[allow(clippy::too_many_arguments)]
fn settle_undecided(
    graded: &mut [(Key, Vec<String>, Grade)],
    saved: &HashMap<Key, Value>,
    fresh: &Workbook,
    calc: &Calc,
    scan: &Scan<'_>,
    solver: &HashSet<Key>,
    tolerance: f64,
    own: &mut Option<Workbook>,
    budget: &mut usize,
) {
    let mut loops: Option<HashSet<Key>> = None;
    for (k, _, grade) in graded.iter_mut().filter(|(_, _, g)| *g == Grade::Differs) {
        let (Some(Value::Number(expected)), Value::Number(_)) = (saved.get(k), value_at(fresh, *k)) else { continue };
        let Some(e) = scalar_formula(fresh, *k) else { continue };
        if fresh.calc.iterative && super::noise::within_iteration_change(fresh.calc.max_change, &Value::Number(*expected), &value_at(fresh, *k)) {
            let loops = loops.get_or_insert_with(|| super::noise::iterated_cells(fresh, scan));
            if loops.contains(k) {
                let own = &*own.get_or_insert_with(|| as_saved(fresh, saved));
                // GridCraft's formula, once on the saved state, must be the arithmetic's.
                let engine = arithmetic_value(&evaluate_expr(own, k.0, k.1, &e).scalar());
                if let (Some((once, reach)), Some(engine)) = (iteration_reach(own, *k, &e, loops, fresh.calc.max_change, budget), engine)
                    && (engine - once.value).abs() <= once.err
                    && super::noise::iteration_explains(*expected, once, reach, tolerance)
                {
                    *grade = Grade::IterationChange;
                }
            }
        }
    }
    let mut accepted: HashMap<Key, Grade> = graded.iter().filter(|(_, _, g)| *g != Grade::Differs).map(|(k, _, g)| (*k, *g)).collect();
    for _ in 0..MAX_SETTLE_PASSES {
        let mut settled = false;
        for (k, _, grade) in graded.iter_mut().filter(|(_, _, g)| *g == Grade::Differs) {
            let residue = match (saved.get(k), value_at(fresh, *k), scalar_formula(fresh, *k)) {
                (Some(Value::Number(expected)), Value::Number(actual), Some(e)) => fresh_bound(fresh, saved, &accepted, tolerance, *k, &e, budget)
                    .filter(|b| (expected - actual).abs() <= b.err)
                    .map(|b| if b.solver { Grade::Solver } else { Grade::CancellationResidue }),
                _ => None,
            };
            let new = residue.or_else(|| {
                (solver.contains(k) && reads_settled_solvers(fresh, saved, calc, solver, &accepted, tolerance, own, *k, budget))
                    .then_some(Grade::Solver)
            });
            if let Some(g) = new {
                *grade = g;
                accepted.insert(*k, g);
                settled = true;
            }
        }
        if !settled {
            break;
        }
    }
}

/// The formula `e` at `k` as arithmetic ([`arithmetic_on`]) over the values of `own`, and how far
/// moving the loop by `by` can move it: each cell of `loops` the formula reads, taken `by` higher
/// and `by` lower with every other cell as it is, moves the formula by the larger of the two, and
/// the moves add (first order, the formula is no more than a few operations of a loop that
/// settles). Moving every cell of the loop together would let a difference (`H24-H33`) cancel it.
fn iteration_reach(own: &Workbook, k: Key, e: &Expr, loops: &HashSet<Key>, by: f64, budget: &mut usize) -> Option<(super::noise::Bounded, f64)> {
    use super::noise::{Bounded, Bounds};
    let mut read: Vec<Key> = Vec::new();
    let once = {
        let mut input = |q: Key| {
            if !read.contains(&q) {
                read.push(q);
            }
            arithmetic_value(&value_at(own, q)).map(Bounded::exact)
        };
        Bounds { wb: own, at: k, input: &mut input, call: None, budget: &mut *budget }.of(e)?
    };
    let mut reach = 0.0;
    for q in read.into_iter().filter(|q| loops.contains(q)) {
        let mut moved = |shift: f64| {
            let mut input = |p: Key| arithmetic_value(&value_at(own, p)).map(|v| Bounded::exact(if p == q { v + shift } else { v }));
            Bounds { wb: own, at: k, input: &mut input, call: None, budget: &mut *budget }.of(e).map(|b| (b.value - once.value).abs())
        };
        reach += moved(by)?.max(moved(-by)?);
    }
    Some((once, reach))
}

/// The solver-reader test of [`settle_undecided`] for `k`: its formula gives its cache back from
/// the file's saved values, every formula it reads whose cache isn't GridCraft's value is a graded
/// match (`accepted`: within the tolerance, or settled by [`settle_undecided`]), and one of those
/// is a solver's result or a reader of one. A solver's result that matches within the tolerance
/// still carries its difference into an ill-conditioned reader (`NPV(IRR(v),v)`, 0 but for the
/// solver's last digits).
#[allow(clippy::too_many_arguments)]
fn reads_settled_solvers(
    fresh: &Workbook,
    saved: &HashMap<Key, Value>,
    calc: &Calc,
    solver: &HashSet<Key>,
    accepted: &HashMap<Key, Grade>,
    tolerance: f64,
    own: &mut Option<Workbook>,
    k: Key,
    budget: &mut usize,
) -> bool {
    if !gives_cache_back(own.get_or_insert_with(|| as_saved(fresh, saved)), saved, k, tolerance, OneCellReading::Array) {
        return false;
    }
    let mut carried = false;
    for area in calc.graph.precedents_of(k) {
        let Some(sh) = fresh.sheet(area.sheet) else { continue };
        for (c, cell) in sh.cells.iter_range(area.range) {
            let q = (area.sheet, c);
            if q == k || cell.formula.is_none() {
                continue;
            }
            let Some(left) = budget.checked_sub(1) else { return false };
            *budget = left;
            if saved.get(&q).is_some_and(|was| *was == cell.value.scalar()) {
                continue;
            }
            if !accepted.contains_key(&q) {
                return false;
            }
            carried |= solver.contains(&q);
        }
    }
    carried
}

/// The precedent-local oracle's test (see [`oracle`]) for the mismatching cell `k`: its formula,
/// not an array or dynamic-array one, is IEEE 754 arithmetic on numbers ([`super::noise::Bounds`]
/// without solvers: `+ - * /`, `%`, ABS, SUM, MIN, MAX), and evaluated on the file's saved
/// values (`own`, built from `fresh` and `saved` on first use by [`as_saved`]) it gives `actual`
/// again, and a value `expected` is further from than that arithmetic's rounding, which differ
/// by more than rounding. What such a formula gives from its inputs is fixed by the arithmetic,
/// not by GridCraft's functions, so a cache it doesn't give is one its inputs no longer give.
#[allow(clippy::too_many_arguments)]
fn stale_against_own(
    own: &mut Option<Workbook>,
    fresh: &Workbook,
    saved: &HashMap<Key, Value>,
    k: Key,
    expected: &Value,
    actual: &Value,
    tolerance: f64,
    budget: &mut usize,
) -> bool {
    let (Value::Number(was), true, Some(e)) = (expected, differs_beyond_rounding(expected, actual), scalar_formula(fresh, k)) else { return false };
    let own = &*own.get_or_insert_with(|| as_saved(fresh, saved));
    let Some(local) = arithmetic_on(own, k, &e, budget) else { return false };
    values_match(&Value::Number(local.value), actual, tolerance) && (was - local.value).abs() > local.err
}

/// The formula `e` at `k` as IEEE 754 arithmetic on numbers ([`super::noise::Bounds`] without
/// solvers: `+ - * /`, `%`, ABS, SUM, MIN, MAX) over the values of `wb`, each exact: what Excel's
/// arithmetic gives from them, within the bound, independently of GridCraft's functions.
fn arithmetic_on(wb: &Workbook, k: Key, e: &Expr, budget: &mut usize) -> Option<super::noise::Bounded> {
    let mut input = |q: Key| arithmetic_value(&value_at(wb, q)).map(super::noise::Bounded::exact);
    super::noise::Bounds { wb, at: k, input: &mut input, call: None, budget }.of(e)
}

/// What the file says about its cache, for [`oracle`].
#[derive(Clone, Copy, Debug)]
pub(super) struct CacheFacts {
    pub trust: CacheTrust,
    /// Manual calculation, as `<calcPr>` or the reader's calculation mode says.
    pub manual: bool,
    /// The major version of the Excel that calculated the cache (see [`super::legacy`]).
    pub excel_version: Option<u32>,
    /// The kind of application the file says wrote it (see [`super::producer`]): a writer's own
    /// encodings of a cache are decoded only for the writer known to use them.
    pub producer: ProducerKind,
}

/// Compares every formula cell's producer-cached value in `saved` with `fresh` (recalculated
/// by GridCraft with the volatile formulas of `pinned` holding their saved values, see
/// [`pin_volatile`]). Every cell is compared whatever the file says about its cache, but only a
/// trusted cache gates: against any other (see [`CacheTrust`]) the results are counted as
/// `untrusted_matched` / `untrusted_mismatched` and each mismatch is listed with its class.
/// `bytes` is the file the workbook was read from, consulted for what the importer's model
/// doesn't keep (see [`source`]). `relocalized` recalculates a fresh copy of the workbook under
/// another locale, tried for the mismatches against a trusted cache (see [`locale`]).
///
/// The precedent-local oracle: in a manual-calculation workbook (`manual`), a trusted mismatch
/// (one that no locale reproduces either) whose formula is IEEE 754 arithmetic on numbers only
/// and, evaluated once on the file's own saved values for every cell it reads (see
/// [`as_saved`]), gives GridCraft's result again, by more than rounding, is a cache inconsistent
/// with its inputs: Excel in manual mode recalculates only what it marked dirty, and a full
/// recalculation replaces such a value with the one its inputs give. It is listed as
/// [`CacheTrust::StaleCell`], counted in `stale_cell`, and doesn't gate. A formula that calls
/// anything but ABS, SUM, MIN or MAX, or reads anything but numbers, is never judged: what it
/// gives from its inputs is GridCraft's semantics, the question under test, and a GridCraft
/// error reproduces itself on any inputs (see [`stale_against_own`]). One whose formula gives
/// the cache back from the saved inputs stays a mismatch: its inputs differ, not its formula.
/// Nothing in a workbook that spills is judged (spilled cells carry GridCraft's values, not the
/// file's).
///
/// The version oracle: a trusted mismatch the version of Excel that calculated the cache explains
/// (see [`super::legacy`]) is listed as [`CacheTrust::LegacyExcelVersion`], and so is one whose
/// only mismatching inputs are such cells (see [`spread_legacy`]); counted in
/// `legacy_excel_version`, not gating.
///
/// The loop oracle: with iterative calculation off, a trusted mismatch on a circular reference
/// the calculation found (`calc.circular`), or reading one directly, through a defined name or
/// through other formulas, is listed as [`CacheTrust::CircularReference`]: Excel can't finish
/// such a calculation, and what it cached depends on the order it took the cells in. Counted in
/// `circular_reference`, not gating.
#[allow(clippy::too_many_arguments)]
pub(super) fn oracle(
    saved: &HashMap<Key, Value>,
    fresh: &Workbook,
    calc: &Calc,
    cache: CacheFacts,
    pinned: &HashSet<Key>,
    opts: &Options,
    bytes: &[u8],
    relocalized: &mut dyn FnMut(&'static Locale) -> Option<Workbook>,
) -> CalcReport {
    let CacheFacts { trust, manual, excel_version, producer } = cache;
    let formula_wb = fresh;
    let gating = trust == CacheTrust::Trusted;
    let mut r = CalcReport { cache_trust: trust, ..Default::default() };
    let scan = Scan::new(formula_wb);
    // A pinned formula is volatile itself, but feeds its readers the value Excel's read.
    let volatile = scan.spread(calc, formula_wb, Source::Volatile, |k| pinned.contains(&k));
    let circular = if formula_wb.calc.iterative || calc.circular.is_empty() || !gating {
        HashSet::new()
    } else {
        scan.spread_from(calc, formula_wb, calc.circular.iter().copied().collect(), None).cells
    };
    r.volatile_via_names = volatile.via_names;
    let solver = scan.spread(calc, formula_wb, Source::Solver, |_| false);
    // A call to what Excel lacks whose saved value the engine kept feeds its readers what Excel
    // read, so they have an oracle. One the engine couldn't keep (no saved value, or a value it
    // replaced) leaves its readers without.
    let foreign_why = scan.seed_reasons(Source::Foreign);
    let kept = |(si, c): Key| {
        let now = fresh.sheet(si).and_then(|s| s.cell(c)).map(|x| x.value.scalar());
        saved.get(&(si, c)).is_some_and(|v| !v.is_empty() && now.is_some_and(|now| values_match(v, &now, 0.0)))
    };
    let foreign = scan.spread(calc, formula_wb, Source::Foreign, kept);
    // Read from the file only when a text differs in its surrounding whitespace.
    let mut preserved: Option<HashSet<Key>> = None;
    // The precedent-local oracle's workbook, built for the first candidate.
    let local_oracle = gating && manual && !formula_wb.sheets.iter().any(|s| !s.spill_ranges.is_empty());
    let mut own: Option<Workbook> = None;

    let mut graded: Vec<(Key, Vec<String>, Grade)> = Vec::new();
    // `--cells`: every formula cell's bucket, named as the corpus database names them.
    let mut cells: Vec<(Key, String)> = Vec::new();
    let mut note = |k: Key, bucket: &dyn Fn() -> String| {
        if opts.cells {
            cells.push((k, bucket()));
        }
    };
    for (si, sh) in formula_wb.sheets.iter().enumerate() {
        for (c, cell) in sh.cells.iter() {
            let Some(f) = &cell.formula else { continue };
            r.formula_cells += 1;
            if volatile.cells.contains(&(si, c)) || pinned.contains(&(si, c)) {
                r.volatile += 1;
                note((si, c), &|| "volatile".into());
                continue;
            }
            let Some(expected_val) = saved.get(&(si, c)).filter(|v| !v.is_empty()) else {
                r.no_oracle += 1;
                *r.no_oracle_reasons.entry(why::NO_CACHED_VALUE).or_default() += 1;
                note((si, c), &|| format!("no_oracle:{}", why::NO_CACHED_VALUE));
                continue;
            };
            if let Some(kind) = foreign_why.get(&(si, c)) {
                r.no_oracle += 1;
                *r.no_oracle_reasons.entry(*kind).or_default() += 1;
                note((si, c), &|| format!("no_oracle:{kind}"));
                continue;
            }
            if foreign.cells.contains(&(si, c)) {
                r.no_oracle += 1;
                *r.no_oracle_reasons.entry(why::DOWNSTREAM).or_default() += 1;
                note((si, c), &|| format!("no_oracle:{}", why::DOWNSTREAM));
                continue;
            }
            let actual = fresh.sheet(si).and_then(|s| s.cell(c)).map(|x| x.value.scalar()).unwrap_or_default();
            let funcs = match f.expr_arc() {
                Some(e) => {
                    let names = functions_of(&e);
                    if names.is_empty() { vec!["(no function)".to_string()] } else { names }
                }
                None => vec!["(unparsed)".to_string()],
            };
            let grade = if error_text_cached(producer, expected_val, &actual) {
                Grade::ErrorDetail
            } else if values_match(expected_val, &actual, opts.tolerance) {
                Grade::Same
            } else if bool_cached_as_number(expected_val, &actual) {
                Grade::BoolAsNumber
            } else if lost_surrounding_space(producer, expected_val, &actual)
                && !preserved.get_or_insert_with(|| source::preserved_space(bytes, fresh)).contains(&(si, c))
            {
                Grade::Whitespace
            } else {
                Grade::Differs
            };
            graded.push(((si, c), funcs, grade));
        }
    }

    let mut budget = super::noise::OPERAND_BUDGET;
    settle_undecided(&mut graded, saved, fresh, calc, &scan, &solver.cells, opts.tolerance, &mut own, &mut budget);

    // A cache computed under another locale: the cells only it explains match, and are counted.
    if gating && graded.iter().any(|(_, _, g)| *g == Grade::Differs) {
        let of = |want: Grade| graded.iter().filter(|(_, _, g)| *g == want).map(|(k, _, _)| *k).collect::<Vec<Key>>();
        let (differs, same) = (of(Grade::Differs), of(Grade::Same));
        if let Some((tag, reproduced)) = locale::reproducing(&differs, &same, saved, opts.tolerance, relocalized) {
            r.locale = Some(tag);
            for (k, _, g) in &mut graded {
                if *g == Grade::Differs && reproduced.contains(k) {
                    *g = Grade::Locale;
                }
            }
        }
    }

    let mut bad: Vec<(Key, Vec<String>, CacheTrust)> = Vec::new();
    for (k, funcs, grade) in graded {
        let (si, c) = k;
        r.checked += usize::from(gating);
        if gating {
            for n in &funcs {
                r.functions.entry(n.clone()).or_default().checked += 1;
            }
        }
        let alt = grade.tally(&mut r);
        if grade != Grade::Differs {
            if gating {
                r.matched += 1;
                note(k, &|| alt.map_or_else(|| "matched".into(), |a| format!("matched_alt:{a}")));
            } else {
                r.untrusted_matched += 1;
                note(k, &|| format!("untrusted_match:{}", trust.as_str()));
            }
            continue;
        }
        let expected_val = saved.get(&k).cloned().unwrap_or_default();
        let actual = fresh.sheet(si).and_then(|s| s.cell(c)).map(|x| x.value.scalar()).unwrap_or_default();
        // Still a mismatch after the locale pass: is the cache stale against its own inputs, or
        // the calculating version's answer?
        let class = if local_oracle && stale_against_own(&mut own, formula_wb, saved, k, &expected_val, &actual, opts.tolerance, &mut budget) {
            CacheTrust::StaleCell
        } else if gating && circular.contains(&k) {
            CacheTrust::CircularReference
        } else if gating
            && (version_explains(formula_wb, saved, k, excel_version, &funcs, &expected_val, &actual, opts.tolerance, &mut budget)
                || underflow_legacy(formula_wb, k, excel_version, &expected_val, &actual)
                || typed_text_legacy(formula_wb, k, excel_version, &expected_val, &actual)
                || one_cell_reading_legacy(formula_wb, saved, k, excel_version, &funcs, opts.tolerance)
                || old_answer_legacy(formula_wb, saved, k, excel_version, &funcs, opts.tolerance))
        {
            CacheTrust::LegacyExcelVersion
        } else {
            trust
        };
        bad.push((k, funcs, class));
    }
    if bad.iter().any(|(_, _, class)| *class == CacheTrust::LegacyExcelVersion) {
        spread_legacy(&mut bad, calc, &scan, formula_wb, saved, excel_version, opts.tolerance);
    }
    count_mismatches(&mut r, &bad, saved, fresh, opts.cells.then_some(&mut cells));
    r.cells = cells.into_iter().map(|(k, bucket)| (cell_name(formula_wb, k), bucket)).collect();

    list_mismatches(&mut r, bad, calc, &scan, formula_wb, saved, opts);
    r
}

/// Counts each mismatch of `bad` in its class, with its `expected type → actual type` signature;
/// with `cells`, notes its bucket.
fn count_mismatches(
    r: &mut CalcReport,
    bad: &[(Key, Vec<String>, CacheTrust)],
    saved: &HashMap<Key, Value>,
    fresh: &Workbook,
    mut cells: Option<&mut Vec<(Key, String)>>,
) {
    for (k, funcs, class) in bad {
        let (si, c) = *k;
        let expected_val = saved.get(k).cloned().unwrap_or_default();
        let actual = fresh.sheet(si).and_then(|s| s.cell(c)).map(|x| x.value.scalar()).unwrap_or_default();
        let signature = format!("{} → {}", type_name(&expected_val), type_name(&actual));
        let (count, signatures) = match class {
            CacheTrust::StaleCell => (&mut r.stale_cell, &mut r.untrusted_signatures),
            CacheTrust::LegacyExcelVersion => (&mut r.legacy_excel_version, &mut r.untrusted_signatures),
            CacheTrust::CircularReference => (&mut r.circular_reference, &mut r.untrusted_signatures),
            CacheTrust::Trusted => (&mut r.mismatched, &mut r.signatures),
            _ => (&mut r.untrusted_mismatched, &mut r.untrusted_signatures),
        };
        *count += 1;
        *signatures.entry(signature).or_default() += 1;
        if *class == CacheTrust::Trusted {
            for n in funcs {
                r.functions.entry(n.clone()).or_default().mismatched += 1;
            }
        }
        if let Some(cells) = cells.as_deref_mut() {
            let bucket = match class {
                CacheTrust::StaleCell => "stale_cell".into(),
                CacheTrust::LegacyExcelVersion => "legacy_excel_version".into(),
                CacheTrust::CircularReference => "circular_reference".into(),
                CacheTrust::Trusted => "calc_mismatch".into(),
                other => format!("untrusted_mismatch:{}", other.as_str()),
            };
            cells.push((*k, bucket));
        }
    }
}

/// [`super::legacy::explains`] for the formula at `k`, calling `funcs`: the bound it propagates
/// is [`super::noise::Bounds`] over the formula as GridCraft calculated it in `fresh`, each call of
/// a function the calculating version (`version`) had an older algorithm of taken within that
/// algorithm's accuracy ([`super::legacy::old_accuracy`]), what it reads carried as
/// [`carried_input`] carries it (within the tolerance), anything else unbounded.
#[allow(clippy::too_many_arguments)]
fn version_explains(
    fresh: &Workbook,
    saved: &HashMap<Key, Value>,
    k: Key,
    version: Option<u32>,
    funcs: &[String],
    expected: &Value,
    actual: &Value,
    tolerance: f64,
    budget: &mut usize,
) -> bool {
    use super::noise::{Bounded, Bounds};
    let Some(e) = fresh.sheet(k.0).and_then(|s| s.cell(k.1)).and_then(|x| x.formula.as_ref()).and_then(|f| f.expr_arc()) else { return false };
    let is_future = |name: &str| gridcraft_xlsx::file_function_name(name).is_some();
    let mut propagated = |v: u32| {
        let e = scalar_formula(fresh, k)?;
        let none = HashMap::new();
        let mut input = |q: Key| carried_input(fresh, saved, &none, tolerance, q);
        let mut old = |name: &str, call: &Expr| {
            let r = super::legacy::improved_since(v, name).then(|| call_value(fresh, k, call))??;
            Some(Bounded { value: r, err: super::legacy::old_accuracy(r), solver: false })
        };
        Bounds { wb: fresh, at: k, input: &mut input, call: Some(&mut old), budget: &mut *budget }.of(&e).map(|b| b.err)
    };
    super::legacy::explains(version, &e, funcs, expected, actual, &is_future, &mut propagated)
}

/// [`super::legacy::typed_text_explains`] for the formula at `(si, c)`.
fn typed_text_legacy(wb: &Workbook, (si, c): Key, version: Option<u32>, expected: &Value, actual: &Value) -> bool {
    let Some(e) = wb.sheet(si).and_then(|s| s.cell(c)).and_then(|x| x.formula.as_ref()).and_then(|f| f.expr_arc()) else { return false };
    super::legacy::typed_text_explains(version, &e, expected, actual)
}

/// [`super::legacy::underflow_error_explains`] for the formula at `k`: one call (in parentheses or
/// not) of `NORMDIST(x, mean, sd, FALSE)`, its arguments as GridCraft calculated them.
fn underflow_legacy(wb: &Workbook, k: Key, version: Option<u32>, expected: &Value, actual: &Value) -> bool {
    let Some(e) = scalar_formula(wb, k) else { return false };
    let mut call = &*e;
    while let Expr::Paren(inner) = call {
        call = inner;
    }
    let Expr::Call(name, args) = call else { return false };
    let [x, mean, sd, cumulative] = args.as_slice() else { return false };
    let number = |a: &Expr| match evaluate_expr(wb, k.0, k.1, a).scalar() {
        Value::Number(v) => Some(v),
        _ => None,
    };
    let density = matches!(evaluate_expr(wb, k.0, k.1, cumulative).scalar(), Value::Bool(false) | Value::Number(0.0));
    super::legacy::underflow_error_explains(version, name, expected, actual, || match (density, number(x), number(mean), number(sd)) {
        (true, Some(x), Some(m), Some(s)) => super::legacy::normal_density_underflows(x, m, s),
        _ => false,
    })
}

/// [`super::legacy::one_cell_reference_explains`] for the formula at `k`, calling `funcs`: the
/// older reading is the formula evaluated on the values GridCraft calculated (`fresh`) with
/// SUMPRODUCT taking a reference to one cell as the value in it ([`OneCellReading::Scalar`]), and
/// it has to give the cache back. That is the only thing Excel 2016 changed in it, and nothing else
/// differs from GridCraft's own calculation of the formula: every input holds GridCraft's value,
/// the ones the file cached and the ones it didn't, so a formula the older reading gives the cache
/// back for differs from the current reading in that reading alone. A mismatch with another cause
/// (an input GridCraft calculates differently from Excel, `X+0*SUMPRODUCT(…)`) is not explained
/// by it, whether or not the cache can be reproduced from the inputs the file saved.
fn one_cell_reading_legacy(fresh: &Workbook, saved: &HashMap<Key, Value>, k: Key, version: Option<u32>, funcs: &[String], tolerance: f64) -> bool {
    super::legacy::one_cell_reference_explains(version, funcs, || gives_cache_back(fresh, saved, k, tolerance, OneCellReading::Scalar))
}

/// Whether the argument `e` of a call at `k` is one whose value GridCraft has right: a literal, an
/// omitted argument, or a reference (to a cell or a range on a sheet of the workbook) in which
/// every cell is a constant or a formula whose cache GridCraft's value matches, with parentheses,
/// signs and arithmetic (`+ - * / ^`) over those. A call, a name, a whole row or column, a
/// reference to another workbook or anything else is not verified: nothing compares the value
/// GridCraft gives it with Excel's, so a call on it can't be excused by an older answer (the cache
/// of `NORMSINV(MATCH(…)*1E-7)` is `#N/A` for MATCH's own reason, whatever NORMSINV did).
fn verified_argument(fresh: &Workbook, saved: &HashMap<Key, Value>, k: Key, e: &Expr, tolerance: f64, budget: &mut usize) -> bool {
    use gridcraft_engine::formula::{BinOp, RefKind, Reference, SheetSel};
    let mut ok = |x: &Expr| verified_argument(fresh, saved, k, x, tolerance, budget);
    match e {
        Expr::Number(_) | Expr::Text(_) | Expr::Bool(_) | Expr::Error(_) | Expr::Missing => true,
        Expr::Paren(x) | Expr::Unary(_, x) => ok(x),
        Expr::Binary(BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Pow, x, y) => ok(x) && ok(y),
        Expr::Array(rows) => rows.iter().flatten().all(ok),
        Expr::Ref(Reference { sheet, kind }) => {
            let si = match sheet {
                SheetSel::Current => k.0,
                SheetSel::Named(n) if !n.is_empty() => match fresh.sheet_index(n) {
                    Some(si) => si,
                    None => return false,
                },
                _ => return false,
            };
            let range = match kind {
                RefKind::Cell(a) => RangeRef::cell(a.cell()),
                RefKind::Range(a, b) => RangeRef::new(a.cell(), b.cell()),
                RefKind::Rows(..) | RefKind::Cols(..) => return false,
            };
            let Some(sheet) = fresh.sheet(si) else { return false };
            for (c, cell) in sheet.cells.iter_range(range) {
                *budget = match budget.checked_sub(1) {
                    Some(left) => left,
                    None => return false,
                };
                if cell.formula.is_some() && !saved.get(&(si, c)).is_some_and(|was| values_match(was, &value_at(fresh, (si, c)), tolerance)) {
                    return false;
                }
            }
            true
        }
        _ => false,
    }
}

/// The older algorithm's documented answer ([`super::legacy::old_answer`]) of the calls in the formula
/// at `k` (calling `funcs`) that the calculating version (`version`) had another algorithm of: where
/// GridCraft's call gives another answer for the call's own inputs, the one answer evidenced for
/// them (`PMT(0,0,…)` is `#DIV/0!` to Excel 2007, `#NUM!` to Excel 2010 and later; IRR's `#NUM!` for
/// the flows an Excel 2007 cache shows it giving up on; MOD's `#NUM!` past 2^27 times the divisor).
/// A call with no evidenced answer for its inputs is never replaced, nor one whose arguments aren't
/// verified ([`verified_argument`]).
///
/// The proof is the formula itself, evaluated on the values GridCraft calculated (`fresh`, as
/// [`one_cell_reading_legacy`] does), and the cell is excused only when the older answer is the one
/// difference:
/// - with the calls replaced by their older answers it gives the cache back, wherever the calls are
///   in it (inside arithmetic, or `IF(ISERROR(IRR(…)),"NA",IRR(…))`) and whatever the cache is (the
///   call's error passed on, or the text an IF answers for it);
/// - that is not what GridCraft's own value of the formula is: the replacement changes the result
///   (the old answer differs from the call's current value, and the formula depends on it);
/// - with the calls replaced by the values GridCraft calculated for them, it gives GridCraft's value
///   again, so nothing else in it is evaluated differently between the two runs.
///
/// A mismatch of another operand outside the call (`MATCH(…)+MOD(5,0)`, where GridCraft's MATCH is a
/// number and the cache's `#N/A` is its) is not reproduced by any older answer of a call and gates;
/// so does one inside its arguments (`NORMSINV(MATCH(…)*1E-7)`), which the control run can't see:
/// the arguments are judged on their own. Each call is tried alone and then all of them together, so
/// a formula whose other calls went on working has its proof too.
fn old_answer_legacy(fresh: &Workbook, saved: &HashMap<Key, Value>, k: Key, version: Option<u32>, funcs: &[String], tolerance: f64) -> bool {
    let Some(v) = version.filter(|v| funcs.iter().any(|f| super::legacy::improved_since(*v, f))) else { return false };
    let (Some(expected), Some(e)) = (saved.get(&k), anchored_formula(fresh, k)) else { return false };
    let own = value_at(fresh, k);
    let mut calls: Vec<&Expr> = Vec::new();
    e.walk(&mut |x| {
        if matches!(x, Expr::Call(name, _) if super::legacy::improved_since(v, name)) && !calls.contains(&x) {
            calls.push(x);
        }
    });
    // Per call: its old answer, and its value as an expression for the control run. A call is
    // replaced only when every argument is verified: the control run below swaps the whole call, so it
    // says nothing of an argument GridCraft got wrong (see [`verified_argument`]).
    let mut budget = super::noise::OPERAND_BUDGET;
    let older: Vec<(&Expr, CellError, Option<Expr>)> = calls
        .into_iter()
        .filter_map(|call| {
            let Expr::Call(name, args) = call else { return None };
            let now = evaluate_expr(fresh, k.0, k.1, call).scalar();
            if !args.iter().all(|a| verified_argument(fresh, saved, k, a, tolerance, &mut budget)) {
                return None;
            }
            let inputs: Vec<Value> = args.iter().map(|a| evaluate_expr(fresh, k.0, k.1, a)).collect();
            let answer = super::legacy::old_answer(v, name, &inputs, &now)?;
            let literal = match &now {
                Value::Number(n) => Some(Expr::Number(*n)),
                Value::Error(c) => Some(Expr::Error(*c)),
                _ => None,
            };
            Some((call, answer, literal))
        })
        .collect();
    let mut tries: Vec<Vec<&(&Expr, CellError, Option<Expr>)>> = older.iter().map(|o| vec![o]).collect();
    if older.len() > 1 {
        tries.push(older.iter().collect());
    }
    let swap = |replaced: &[&(&Expr, CellError, Option<Expr>)], old: bool| -> Option<Value> {
        let mut complete = true;
        let trial = (*e).clone().map(&mut |n| match replaced.iter().find(|(call, _, _)| **call == n) {
            Some((_, answer, _)) if old => Expr::Error(*answer),
            Some((_, _, Some(literal))) => literal.clone(),
            Some(_) => {
                complete = false;
                n
            }
            None => n,
        });
        complete.then(|| evaluate_expr(fresh, k.0, k.1, &trial).scalar())
    };
    tries.iter().any(|replaced| {
        let with_old = swap(replaced, true).unwrap_or_default();
        values_match(expected, &with_old, tolerance)
            && !values_match(&own, &with_old, tolerance)
            && swap(replaced, false).is_none_or(|control| values_match(&own, &control, tolerance))
    })
}

/// How many times [`spread_legacy`] looks again at the mismatches still open: each pass settles
/// the cells whose open inputs the one before settled; what is left after the last stays open.
const MAX_LEGACY_PASSES: usize = 16;

/// Lists as [`CacheTrust::LegacyExcelVersion`] every trusted mismatch that reads a legacy one and
/// whose formula gives its cache back when evaluated on what GridCraft calculated (`fresh`) with
/// each legacy cell holding the value the file saved for it, and each number GridCraft agrees on
/// with the cache holding the cache's digits ([`as_agreed`]): what it caches was calculated from
/// the older version's results, and GridCraft's calculation of it agrees with Excel's on them. Every
/// other input keeps GridCraft's value, another mismatch or a cell the file never cached, so a
/// formula that differs from its cache by more than the legacy inputs (`X+0*A1` over a legacy A1,
/// or over an `X` GridCraft gets wrong) stays open. It needn't wait for the mismatches in the
/// ranges it reads, or the readers of one range would keep each other open: one it depends on
/// keeps the cache from being given back. The formula is read as the calculating Excel
/// (`version`) read it, see [`one_cell_reading_legacy`].
fn spread_legacy(
    bad: &mut [(Key, Vec<String>, CacheTrust)],
    calc: &Calc,
    scan: &Scan<'_>,
    fresh: &Workbook,
    saved: &HashMap<Key, Value>,
    version: Option<u32>,
    tolerance: f64,
) {
    // `fresh` with the agreed numbers and the legacy cells as the file saved them, built for the
    // first candidate.
    let mut world: Option<Workbook> = None;
    let mut placed: HashSet<Key> = HashSet::new();
    for _ in 0..MAX_LEGACY_PASSES {
        let legacy = CellIndex::new(bad.iter().filter(|(_, _, c)| *c == CacheTrust::LegacyExcelVersion).map(|(k, _, _)| *k));
        let new: Vec<Key> = bad.iter().filter(|(k, _, c)| *c == CacheTrust::LegacyExcelVersion && !placed.contains(k)).map(|(k, _, _)| *k).collect();
        let mut changed = false;
        for (k, funcs, class) in bad.iter_mut().filter(|(_, _, c)| *c == CacheTrust::Trusted) {
            if !reads_any(calc, scan, *k, &legacy) {
                continue;
            }
            let world = world.get_or_insert_with(|| as_agreed(fresh, saved, tolerance));
            for &(si, c) in new.iter().filter(|q| placed.insert(**q)) {
                if let Some(cell) = world.sheet_mut(si).and_then(|s| s.cells.get_mut(c)) {
                    cell.value = saved.get(&(si, c)).cloned().unwrap_or_default();
                }
            }
            let reading = if super::legacy::reads_one_cell_as_value(version, funcs) { OneCellReading::Scalar } else { OneCellReading::Array };
            if gives_cache_back(world, saved, *k, tolerance, reading) {
                *class = CacheTrust::LegacyExcelVersion;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
}

/// Whether the formula at `k`, evaluated on `world` with SUMPRODUCT reading a reference to one
/// cell as `reading` says, gives the value the file cached for its cell: its value, or an array's
/// first.
fn gives_cache_back(world: &Workbook, saved: &HashMap<Key, Value>, k: Key, tolerance: f64, reading: OneCellReading) -> bool {
    let (Some(expected), Some(e)) = (saved.get(&k), anchored_formula(world, k)) else { return false };
    values_match(expected, &evaluate_expr_as(world, k.0, k.1, &e, reading).scalar(), tolerance)
}

/// Whether the formula at `k` reads a cell of `index` other than itself, directly or through a
/// defined name.
fn reads_any(calc: &Calc, scan: &Scan<'_>, k: Key, index: &CellIndex) -> bool {
    calc.graph.precedents_of(k).iter().any(|a| index.any_in(a, Some(k))) || scan.any_name_area(k, &mut |a| index.any_in(a, Some(k)))
}

/// Lists the mismatches in `r`, primary ones first. A mismatch is primary when none of its
/// precedent cells mismatch, whether the formula reads them directly or through a defined name.
fn list_mismatches(
    r: &mut CalcReport,
    bad: Vec<(Key, Vec<String>, CacheTrust)>,
    calc: &Calc,
    scan: &Scan<'_>,
    wb: &Workbook,
    saved: &HashMap<Key, Value>,
    opts: &Options,
) {
    let index = CellIndex::new(bad.iter().map(|(k, _, _)| *k));
    let mut listed = Vec::with_capacity(bad.len());
    for (k, funcs, class) in bad {
        let upstream = reads_any(calc, scan, k, &index);
        if !upstream && class == CacheTrust::Trusted {
            for n in &funcs {
                r.functions.entry(n.clone()).or_default().primary += 1;
            }
        }
        listed.push((!upstream, (k, funcs, class)));
    }
    // Primary mismatches first, each group in sheet then row-major order (already sorted).
    listed.sort_by_key(|(primary, _)| !*primary);
    for (primary, (k, functions, class)) in listed.into_iter().take(opts.max_report) {
        let cell = wb.sheet(k.0).and_then(|s| s.cell(k.1));
        let expected = saved.get(&k).cloned().unwrap_or_default();
        let actual = cell.map(|x| x.value.scalar()).unwrap_or_default();
        r.mismatches.push(Mismatch {
            cell: cell_name(wb, k),
            formula: cell.map(formula_text).unwrap_or_default(),
            kind: mismatch_kind(&expected, &actual),
            expected,
            actual,
            functions,
            primary,
            class,
        });
    }
}
