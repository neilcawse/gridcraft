//! Number mismatches that the calculation itself leaves undecided: a difference within the
//! rounding error the formula's own operations propagate from its inputs, and a value on an
//! iterative loop that Excel's stopping rule explains. Both are graded as matches of their own kind.
//!
//! The rounding bound is a running error analysis of the formula ([`Bounds::of`]): each operation
//! adds the rounding of its result to the errors it carries in from its operands, so a large
//! operand multiplied by an exact zero carries nothing, and a sum that cancels carries the
//! errors of what it adds. Only operations IEEE 754 arithmetic fixes are bounded; any other call
//! leaves the formula unbounded, and its mismatch stands. So does a formula with a subnormal
//! number anywhere in its calculation ([`excel_number`]): there Excel's arithmetic isn't IEEE 754's.

use super::{HashSet, Key, Value, Workbook, taint};
use gridcraft_engine::calc::Calc;
use gridcraft_engine::formula::{BinOp, Expr, RefKind, Reference, SheetSel, UnOp};

/// The rounding of one operation of two implementations, relative to its result: each rounds to
/// within half a unit in the last place of a double, so the two differ by at most one.
const ROUNDING: f64 = f64::EPSILON;

/// Microsoft, "Floating-point arithmetic may give inaccurate results in Excel": "if an addition
/// or subtraction operation results in a value at or very close to zero, Excel 97 and later will
/// compensate for any error introduced as a result of converting an operand to and from binary".
/// A sum or difference no larger than this share of its larger operand (below the 15 digits Excel
/// keeps) may be 0 in Excel's cache, and its whole value is within the bound, when it is the
/// formula's last operation: Excel 16.114 gives 0 for `=0.5-0.4-0.1` and `=(0.5-0.4)-0.1`, but
/// -2.78E-17 for `=(0.5-0.4-0.1)`, `=1*(0.5-0.4-0.1)`, `=ABS(0.5-0.4-0.1)` and `=SUM(0.5-0.4-0.1)`,
/// and 1E15 for `=((1E15+1)-1E15)*1E15`: an operation under parentheses, another operator or a
/// function is never compensated, and its residue is carried like any other.
const NEAR_ZERO: f64 = 1e-14;

/// The deepest expression [`Bounds::of`] follows; a deeper one is unbounded.
const MAX_DEPTH: usize = 256;

/// The most cells read for rounding bounds across one workbook, bounding the check on large files.
pub(super) const OPERAND_BUDGET: usize = 65_536;

/// `b` when Excel's arithmetic holds its value as IEEE 754 does. Microsoft, "Floating-point
/// arithmetic may give inaccurate results in Excel": Excel "does not support denormalized
/// numbers" and a result below 2.2250738585072E-308 underflows to 0 (`=1E-307/10*1E307*10` is 0
/// in Excel 16.114, 1 in IEEE 754). A subnormal value, read or computed, leaves the bound
/// undecided: no rounding error accounts for what Excel does with it.
fn excel_number(b: Bounded) -> Option<Bounded> {
    (b.value == 0.0 || b.value.abs() >= f64::MIN_POSITIVE || !b.value.is_finite()).then_some(b)
}

/// 2^-970 (2^-1022 / 2^-52): a double at least this large is a whole multiple of 2^-1022, the
/// smallest normal double, since its own last place, 52 bits down, is no finer.
const SUM_EXACT_BELOW: f64 = f64::MIN_POSITIVE / f64::EPSILON;

/// A number with the most two implementations of what computed it can differ by.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Bounded {
    pub value: f64,
    pub err: f64,
    /// An iterative solver's result is among what the error is made of (see [`Bounds::solver`]).
    pub solver: bool,
}

impl Bounded {
    pub(super) fn exact(value: f64) -> Bounded {
        Bounded { value, err: 0.0, solver: false }
    }
}

/// How [`Bounds`] takes a call of a function other than ABS, SUM, MIN and MAX (`name`, and the
/// call): its result with the accuracy the caller can show for it, or `None` to leave it unbounded.
pub(super) type Call<'a> = &'a mut dyn FnMut(&str, &Expr) -> Option<Bounded>;

/// What [`Bounds::of`] reads.
pub(super) struct Bounds<'a> {
    /// The workbook whose sheet names the references use.
    pub wb: &'a Workbook,
    /// The cell whose formula is bounded.
    pub at: Key,
    /// The value of a cell the formula reads with the most it can differ by: `None` for what
    /// isn't a number or empty (a text converted, a logical, an error are semantics, not rounding).
    pub input: &'a mut dyn FnMut(Key) -> Option<Bounded>,
    /// The result of any other call with its accuracy (a solver's documented one, an older
    /// Excel's algorithm's); `None` leaves every other call unbounded.
    pub call: Option<Call<'a>>,
    /// Cells left to read for the workbook ([`OPERAND_BUDGET`]).
    pub budget: &'a mut usize,
}

impl Bounds<'_> {
    /// The value of `e` and the most two implementations can differ on it by, or `None` when `e`
    /// does something not bounded here.
    pub(super) fn of(&mut self, e: &Expr) -> Option<Bounded> {
        let b = self.bound(e, 0)?;
        (b.value.is_finite() && b.err.is_finite()).then_some(b)
    }

    fn bound(&mut self, e: &Expr, depth: usize) -> Option<Bounded> {
        if depth > MAX_DEPTH {
            return None;
        }
        let d = depth + 1;
        let b = match e {
            Expr::Number(n) => Some(Bounded::exact(*n)),
            Expr::Paren(x) | Expr::Unary(UnOp::Plus, x) => self.bound(x, d),
            Expr::Unary(UnOp::Neg, x) => self.bound(x, d).map(|b| Bounded { value: -b.value, ..b }),
            Expr::Unary(UnOp::Percent, x) => {
                let b = self.bound(x, d)?;
                let value = b.value / 100.0;
                Some(Bounded { value, err: b.err / 100.0 + ROUNDING * value.abs(), solver: b.solver })
            }
            Expr::Binary(op @ (BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div), x, y) => {
                let (a, b) = (self.bound(x, d)?, self.bound(y, d)?);
                arithmetic(*op, a, b, depth == 0)
            }
            Expr::Binary(BinOp::Pow, x, y) => {
                let (a, b) = (self.bound(x, d)?, self.bound(y, d)?);
                power(a, b)
            }
            Expr::Ref(r @ Reference { kind: RefKind::Cell(c), .. }) => {
                let si = self.sheet(r)?;
                self.read((si, c.cell()))
            }
            Expr::Name(n) => {
                let k = self.name_cell(n)?;
                self.read(k)
            }
            Expr::Call(name, args) => self.call(e, name, args, d),
            _ => None,
        };
        b.and_then(excel_number)
    }

    /// The one cell a defined name refers to as an absolute reference on a named sheet
    /// (`RECYR3` = `Flowsheet!$E$17`): the sheet's own name of that spelling first, then the
    /// workbook's. A name that is anything else (a range, a formula, a relative reference that
    /// depends on where it is used) isn't a cell to read here.
    fn name_cell(&self, n: &str) -> Option<Key> {
        if n.starts_with('[') || n.contains('!') {
            return None;
        }
        let of = |scope: Option<usize>| self.wb.names.iter().find(|d| d.scope == scope && d.name.eq_ignore_ascii_case(n));
        let def = of(Some(self.at.0)).or_else(|| of(None))?;
        match gridcraft_engine::formula::parse_name(&def.formula).ok()? {
            Expr::Ref(Reference { sheet: SheetSel::Named(s), kind: RefKind::Cell(a) }) if a.row_abs && a.col_abs && !s.is_empty() => {
                Some((self.wb.sheet_index(&s)?, a.cell()))
            }
            _ => None,
        }
    }

    fn sheet(&self, r: &Reference) -> Option<usize> {
        match &r.sheet {
            SheetSel::Current => Some(self.at.0),
            SheetSel::Named(n) if !n.is_empty() => self.wb.sheet_index(n),
            _ => None,
        }
    }

    fn read(&mut self, k: Key) -> Option<Bounded> {
        *self.budget = self.budget.checked_sub(1)?;
        if k == self.at {
            return None;
        }
        (self.input)(k).and_then(excel_number)
    }

    /// The numbers an argument of SUM, MIN or MAX gives: a range's cells that hold something
    /// (the functions skip empty cells), or the argument's own value. A range is written as a
    /// reference (`A1:B9`) or as the two cells its ends name (`_YR1:YP`, defined names of one
    /// cell each) on one sheet.
    fn operands(&mut self, arg: &Expr, depth: usize, out: &mut Vec<Bounded>) -> Option<()> {
        let (si, range) = match arg {
            Expr::Ref(r @ Reference { kind: RefKind::Range(a, b), .. }) => {
                (self.sheet(r)?, gridcraft_engine::core::RangeRef::new(a.cell(), b.cell()))
            }
            Expr::Binary(BinOp::Range, x, y) => {
                let (from, to) = (self.cell_of(x)?, self.cell_of(y)?);
                if from.0 != to.0 {
                    return None;
                }
                (from.0, gridcraft_engine::core::RangeRef::new(from.1, to.1))
            }
            _ => {
                out.push(self.bound(arg, depth)?);
                return Some(());
            }
        };
        let cells: Vec<_> =
            self.wb.sheet(si)?.cells.iter_range(range).filter(|(_, cell)| !cell.value.is_empty() || cell.formula.is_some()).map(|(c, _)| c).collect();
        for c in cells {
            out.push(self.read((si, c))?);
        }
        Some(())
    }

    /// The one cell `e` names: a reference to a cell, or a defined name that is one ([`Self::name_cell`]).
    fn cell_of(&self, e: &Expr) -> Option<Key> {
        match e {
            Expr::Ref(r @ Reference { kind: RefKind::Cell(c), .. }) => Some((self.sheet(r)?, c.cell())),
            Expr::Name(n) => self.name_cell(n),
            _ => None,
        }
    }

    /// Whether the condition `e` is TRUE or FALSE however the two implementations round what it
    /// compares: a comparison whose sides are further apart than the errors they carry (equal
    /// only when both are exact), OR, AND and NOT of such conditions, and ISERROR of a cell or
    /// name (a cell that holds a number or nothing isn't one; one that holds an error is). Anything
    /// else, a comparison inside the errors included, is undecided: `None`.
    fn truth(&mut self, e: &Expr, depth: usize) -> Option<bool> {
        if depth > MAX_DEPTH {
            return None;
        }
        let d = depth + 1;
        match e {
            Expr::Paren(x) => self.truth(x, d),
            Expr::Bool(b) => Some(*b),
            Expr::Binary(op @ (BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge), x, y) => {
                let (a, b) = (self.bound(x, d)?, self.bound(y, d)?);
                let (gap, reach) = (a.value - b.value, a.err + b.err);
                if gap.abs() <= reach && !(reach == 0.0 && gap == 0.0) {
                    return None;
                }
                Some(match op {
                    BinOp::Eq => gap == 0.0,
                    BinOp::Ne => gap != 0.0,
                    BinOp::Lt => gap < 0.0,
                    BinOp::Gt => gap > 0.0,
                    BinOp::Le => gap <= 0.0,
                    _ => gap >= 0.0,
                })
            }
            Expr::Call(name, args) if name == "NOT" => match args.as_slice() {
                [x] => self.truth(x, d).map(|t| !t),
                _ => None,
            },
            Expr::Call(name, args) if name == "OR" || name == "AND" => {
                let (decider, mut open) = (name == "OR", false);
                for a in args {
                    match self.truth(a, d) {
                        Some(t) if t == decider => return Some(decider),
                        Some(_) => {}
                        None => open = true,
                    }
                }
                (!open).then_some(!decider)
            }
            Expr::Call(name, args) if name == "ISERROR" => {
                let [x] = args.as_slice() else { return None };
                let k = match x {
                    Expr::Ref(r @ Reference { kind: RefKind::Cell(c), .. }) => (self.sheet(r)?, c.cell()),
                    Expr::Name(n) => self.name_cell(n)?,
                    _ => return None,
                };
                *self.budget = self.budget.checked_sub(1)?;
                if (self.input)(k).is_some() {
                    return Some(false);
                }
                let held = self.wb.sheet(k.0)?.cell(k.1).map(|c| c.value.scalar());
                matches!(held, Some(Value::Error(_))).then_some(true)
            }
            _ => None,
        }
    }

    /// `whole` is the call `name(args)` itself.
    fn call(&mut self, whole: &Expr, name: &str, args: &[Expr], depth: usize) -> Option<Bounded> {
        if name == "ABS" {
            let [x] = args else { return None };
            return self.bound(x, depth).map(|b| Bounded { value: b.value.abs(), ..b });
        }
        if name == "EXP" {
            let [x] = args else { return None };
            return exp(self.bound(x, depth)?);
        }
        if name == "IF" {
            // The branch the condition picks, whatever the rounding does to it; an omitted branch
            // or an undecided condition is not bounded here.
            let (cond, then, otherwise) = match args {
                [c, t, o] => (c, t, o),
                _ => return None,
            };
            let pick = if self.truth(cond, depth)? { then } else { otherwise };
            return if matches!(pick, Expr::Missing) { None } else { self.bound(pick, depth) };
        }
        if !matches!(name, "SUM" | "MIN" | "MAX") {
            let call = self.call.as_mut()?;
            return call(name, whole);
        }
        let mut xs = Vec::new();
        for a in args {
            self.operands(a, depth, &mut xs)?;
        }
        let solver = xs.iter().any(|b| b.solver);
        if name == "SUM" {
            // Summed in any order, n terms round n − 1 times, each partial sum no larger than the
            // sum of the magnitudes. A partial sum of terms that are multiples of 2^-1022 (every
            // number of at least 2^-970, its last place no finer) is one too, so never subnormal;
            // with a smaller term an order may pass through one ([`excel_number`]).
            if xs.iter().any(|b| b.value != 0.0 && b.value.abs() < SUM_EXACT_BELOW) {
                return None;
            }
            let value: f64 = xs.iter().map(|b| b.value).sum();
            let magnitude: f64 = xs.iter().map(|b| b.value.abs()).sum();
            let err = xs.iter().map(|b| b.err).sum::<f64>() + ROUNDING * (xs.len() as f64) * magnitude;
            return Some(Bounded { value, err, solver });
        }
        // MIN and MAX round nothing; the extreme moves no further than the largest error.
        let pick = |a: f64, b: f64| if name == "MIN" { a.min(b) } else { a.max(b) };
        let value = xs.iter().map(|b| b.value).reduce(pick).unwrap_or(0.0);
        let err = xs.iter().map(|b| b.err).fold(0.0, f64::max);
        Some(Bounded { value, err, solver })
    }
}

/// `a op b`, with the error each carries in propagated to first order and then some (`ea·eb`),
/// plus the rounding of the result; a sum near zero may be Excel's compensated 0 ([`NEAR_ZERO`])
/// when the operation is the formula's root (`root`).
fn arithmetic(op: BinOp, a: Bounded, b: Bounded, root: bool) -> Option<Bounded> {
    let solver = a.solver || b.solver;
    let (value, carried) = match op {
        BinOp::Add | BinOp::Sub => {
            let value = if op == BinOp::Add { a.value + b.value } else { a.value - b.value };
            let compensated = if root && value.abs() <= NEAR_ZERO * a.value.abs().max(b.value.abs()) { value.abs() } else { 0.0 };
            (value, a.err + b.err + compensated)
        }
        BinOp::Mul => (a.value * b.value, a.value.abs() * b.err + b.value.abs() * a.err + a.err * b.err),
        BinOp::Div => {
            let room = b.value.abs() - b.err;
            if room <= 0.0 {
                return None;
            }
            let value = a.value / b.value;
            (value, (a.err + value.abs() * b.err) / room)
        }
        _ => return None,
    };
    Some(Bounded { value, err: carried + ROUNDING * value.abs(), solver })
}

/// `e^a`: Excel's and GridCraft's exponentials differ by a few units in the last place (neither is
/// a correctly rounded one), and a carried error in the exponent is the relative error out.
fn exp(a: Bounded) -> Option<Bounded> {
    let value = a.value.exp();
    let relative = a.err.exp_m1() + EXP_ULPS * ROUNDING;
    Some(Bounded { value, err: value.abs() * relative, solver: a.solver })
}

/// `a^b` as the errors of `a` and `b` and Excel's way of computing a power move it: a whole `b`
/// is multiplied out (at most `|b|` roundings), any other is `e^(b·ln a)` (`|b·ln a|` of absolute
/// error in the exponent, plus a few units of rounding in the logarithm and the exponential), and
/// `a`'s relative error is multiplied by `|b|` through either. A base the error could take through
/// zero or a negative base with a `b` that isn't a whole number leaves it undecided.
fn power(a: Bounded, b: Bounded) -> Option<Bounded> {
    if a.value == 0.0 {
        return None;
    }
    let whole = b.err == 0.0 && b.value.fract() == 0.0;
    if a.value < 0.0 && !whole {
        return None;
    }
    let relative_in = a.err / a.value.abs();
    if relative_in >= 0.5 {
        return None;
    }
    let value = a.value.powf(b.value);
    let ln = a.value.abs().ln();
    let carried = (b.value.abs() * relative_in.ln_1p() + ln.abs() * b.err).exp_m1();
    let own = if whole { (b.value.abs() + 1.0) * ROUNDING } else { ((b.value * ln).abs() + EXP_ULPS) * ROUNDING };
    Some(Bounded { value, err: value.abs() * (carried + own), solver: a.solver || b.solver })
}

/// Units in the last place two exponentials, or a logarithm and an exponential, may differ by.
const EXP_ULPS: f64 = 4.0;

/// The formulas on a circular reference of a workbook calculated iteratively, and the ones that
/// read them: the loops are found by calculating a copy with iteration off, which names one
/// formula per loop at least ([`Calc::recalc_all`]: a loop closed through a defined name or a
/// lookup is only found when the formulas are evaluated, the dependency graph doesn't hold what
/// a name refers to), and followed downstream through the graph and defined names, which reaches
/// the rest of each loop.
pub(super) fn iterated_cells(wb: &Workbook, scan: &taint::Scan<'_>) -> HashSet<Key> {
    let mut plain = wb.clone();
    plain.calc.iterative = false;
    let mut loops = Calc::new();
    loops.recalc_all(&mut plain);
    if loops.circular.is_empty() {
        return HashSet::new();
    }
    scan.spread_from(&loops, &plain, loops.circular.iter().copied().collect(), None).cells
}

/// Whether a number on an iterative loop (`expected` cached, `actual` GridCraft's) is within
/// `max_change` of the other: Excel stops iterating once no value changes by more than Maximum
/// Change ("the maximum amount of change between recalculation results").
pub(super) fn within_iteration_change(max_change: f64, expected: &Value, actual: &Value) -> bool {
    matches!((expected, actual), (Value::Number(e), Value::Number(a)) if (e - a).abs() <= max_change)
}

/// Whether the cache `saved` of a cell on (or reading) an iterative loop is a state Excel's
/// stopping rule leaves: `once` is the cell's formula as arithmetic ([`Bounds`], never GridCraft's
/// functions) on the file's saved state, `reach` how far moving what it reads of the loop
/// ([`iterated_cells`]) by Maximum Change moves it (`calc::iteration_reach`). Excel caches the
/// pass after the last, `S = F(S′)` with `S′` within Maximum Change of `S`; to first order
/// `F(S) − F(S′)` is no larger than that `reach`. A formula that doesn't read the loop's state at
/// all (`B1*0+…`) moves by nothing, and its cache must be its value. `F` is Excel's arithmetic, so
/// what this explains isn't an error of GridCraft's evaluator: a formula `Bounds` doesn't bound (a
/// function call, a subnormal, a self-reference) has no such `once`, and its mismatch stands.
pub(super) fn iteration_explains(saved: f64, once: Bounded, reach: f64, tolerance: f64) -> bool {
    let off = (saved - once.value).abs();
    off <= tolerance * saved.abs().max(once.value.abs()).max(1.0) + once.err || off <= reach + 2.0 * once.err
}
