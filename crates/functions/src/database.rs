//! Database functions (DSUM, DCOUNT, DGET…): aggregate one field of the records of a table
//! that satisfy a criteria table.
//!
//! The database and the criteria are arrays whose first row holds column labels. Each criteria
//! row is a set of conditions that must all hold (AND); a record matches when any criteria row
//! holds (OR). An empty criteria cell matches anything. Plain text conditions match values that
//! begin with the text, as in Excel's advanced filter, so an empty string matches any text. A
//! formula under a label that names no field is a computed criterion, which the host evaluates
//! for each record ([`Ctx::computed_criterion`]).

use std::borrow::Cow;

use gridcraft_core::{Array, CellError, Value, compare_text};

use crate::criteria::Criterion;
use crate::util::{A, R, as_array, has, num_val};
use crate::{Arg, Ctx, FnSpec};

/// A label as Excel compares it: text trimmed, a number as it displays in General, a logical as
/// `TRUE` / `FALSE`. A criteria label `true` (text) names a column headed by the logical TRUE, and
/// a label 1 a column headed 1, not TRUE (Excel 365).
fn label_text(v: &Value) -> Option<Cow<'_, str>> {
    match v {
        Value::Text(t) => Some(Cow::Borrowed(t.trim())),
        Value::Number(n) => Some(Cow::Owned(gridcraft_core::number_to_text(*n))),
        Value::Bool(b) => Some(Cow::Borrowed(if *b { "TRUE" } else { "FALSE" })),
        _ => None,
    }
}

/// Column index of a label in the header row: the first header that reads the same, ignoring case.
fn find_column(db: &Array, label: &str) -> Option<usize> {
    let label = label.trim();
    (0..db.cols).find(|&c| db.get(0, c).and_then(label_text).is_some_and(|h| compare_text(&h, label).is_eq()))
}

/// Resolves the field argument: a label or a 1-based column number. `None` when omitted.
fn field_index(db: &Array, a: &[Arg]) -> R<Option<usize>> {
    if !has(a, 1) {
        return Ok(None);
    }
    let v = match a.get(1).map(|x| &x.value) {
        // A field given as a range of more than one cell (`E1:F1`) names no single column.
        Some(Value::Array(arr)) if arr.data.len() > 1 => return Err(CellError::Value),
        Some(v) => v.scalar(),
        None => Value::Empty,
    };
    let column_number = |n: f64| {
        let i = n.trunc();
        // NaN too: it would pass both comparisons and become index 0 (#2163).
        if i.is_nan() || i < 1.0 || i > db.cols as f64 {
            return Err(CellError::Value);
        }
        Ok(Some((i as usize).saturating_sub(1)))
    };
    match v {
        Value::Number(n) => column_number(n),
        // A logical is a column number: TRUE the first column, FALSE none (Excel 365).
        Value::Bool(b) => column_number(if b { 1.0 } else { 0.0 }),
        Value::Text(t) => find_column(db, &t).map(Some).ok_or(CellError::Value),
        Value::Error(e) => Err(e),
        _ => Ok(None),
    }
}

/// One criteria cell as a matcher. Plain text becomes a "begins with" pattern.
fn cell_criterion(v: &Value) -> Option<Criterion> {
    match v {
        Value::Empty => None,
        Value::Text(t) => {
            let s: &str = t;
            let has_op = s.starts_with(['=', '<', '>']);
            let plain_text = !has_op
                && gridcraft_core::parse::parse_number_text(s).is_none()
                && !s.eq_ignore_ascii_case("TRUE")
                && !s.eq_ignore_ascii_case("FALSE")
                && CellError::parse(s).is_none();
            if plain_text && !s.ends_with('*') { Some(Criterion::parse(&Value::from(format!("{s}*")))) } else { Some(Criterion::parse(v)) }
        }
        other => Some(Criterion::parse(other)),
    }
}

/// A criteria row: its index and its conditions (criteria column, database column or `None`
/// when the label is unknown, criterion).
type CriteriaRow = (usize, Vec<(usize, Option<usize>, Criterion)>);

/// Most condition tests one selection may make (records times conditions): every record can be
/// tested against every criteria row when none matches, so a modest database and criteria table
/// multiply into billions of tests. Past it the function is `#NUM!`, as lifted lookups are; 5e8
/// is a few seconds of matching.
const SELECTION_WORK_BUDGET: usize = 500_000_000;

/// Rows (1-based within the database, excluding the header) of matching records.
fn matching_records(db: &Array, crit: &Array, ctx: &mut dyn Ctx) -> R<Vec<usize>> {
    // The database column under each criteria label, found once per label rather than per cell.
    let columns: Vec<Option<usize>> = (0..crit.cols).map(|c| crit.get(0, c).and_then(label_text).and_then(|l| find_column(db, &l))).collect();
    // Per criteria row: its index and a list of (criteria column, database column or None when
    // the label is unknown, criterion).
    let mut rows: Vec<CriteriaRow> = Vec::new();
    let mut tests = 0usize;
    for r in 1..crit.rows {
        let mut conds = Vec::new();
        for c in 0..crit.cols {
            let Some(cell) = crit.get(r, c) else { continue };
            let Some(cr) = cell_criterion(cell) else { continue };
            conds.push((c, columns.get(c).copied().flatten(), cr));
        }
        tests = tests.saturating_add(conds.len().max(1));
        rows.push((r, conds));
    }
    // An empty criteria row matches every record without testing a condition, so nothing is charged.
    let all_match = rows.is_empty() || rows.iter().any(|(_, conds)| conds.is_empty());
    // Computed criteria (a formula evaluated per record) count like any other test.
    if !all_match && db.rows.saturating_sub(1).saturating_mul(tests) > SELECTION_WORK_BUDGET {
        return Err(CellError::Num);
    }
    Ok((1..db.rows)
        .filter(|&rec| {
            all_match
                // Out of work: a computed criterion would fail at once, so records are left out
                // without scanning the criteria rows; the per-row check still stops mid-scan.
                || (!ctx.exhausted()
                    && rows.iter().any(|(r, conds)| {
                        !ctx.exhausted()
                            && conds.iter().all(|(c, col, cr)| !ctx.exhausted() && match col {
                                Some(f) => cr.matches(db.get(rec, *f).unwrap_or(&Value::Empty)),
                                // Under a label that names no field, only a formula (a computed
                                // criterion) can hold.
                                None => ctx.computed_criterion(*r, *c, db, rec).unwrap_or(false),
                            })
                    }))
        })
        .collect())
}

struct Selection<'a> {
    db: Cow<'a, Array>,
    field: Option<usize>,
    records: Vec<usize>,
}

fn select<'a>(a: &'a [Arg], ctx: &mut dyn Ctx) -> R<Selection<'a>> {
    let dbv = &a.first().ok_or(CellError::Value)?.value;
    let critv = &a.get(2).ok_or(CellError::Value)?.value;
    if let Value::Error(e) = dbv {
        return Err(*e);
    }
    if let Value::Error(e) = critv {
        return Err(*e);
    }
    let db = as_array(dbv);
    // A database is its label row and at least one record: a label row alone is `#VALUE!`.
    if db.rows < 2 {
        return Err(CellError::Value);
    }
    let crit = as_array(critv);
    let field = field_index(&db, a)?;
    let records = matching_records(&db, &crit, ctx)?;
    Ok(Selection { db, field, records })
}

/// Numbers of the field among matching records (field required). An error there is the result,
/// as for SUM over the field's cells: DSUM and DAVERAGE over records holding `#N/A` are `#N/A`.
fn field_numbers(a: &[Arg], c: &mut dyn Ctx) -> R<Vec<f64>> {
    let s = select(a, c)?;
    let f = s.field.ok_or(CellError::Value)?;
    let mut out = Vec::with_capacity(s.records.len());
    for &r in &s.records {
        match s.db.get(r, f) {
            Some(Value::Error(e)) => return Err(*e),
            Some(v) => out.extend(v.as_f64()),
            None => {}
        }
    }
    Ok(out)
}

fn dsum(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    num_val(field_numbers(a, c)?.iter().sum())
}

fn daverage(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    let v = field_numbers(a, c)?;
    if v.is_empty() {
        return Err(CellError::Div0);
    }
    num_val(v.iter().sum::<f64>() / v.len() as f64)
}

fn dcount(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    let s = select(a, c)?;
    let n = match s.field {
        Some(f) => s.records.iter().filter(|&&r| matches!(s.db.get(r, f), Some(Value::Number(_)))).count(),
        None => s.records.len(),
    };
    num_val(n as f64)
}

fn dcounta(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    let s = select(a, c)?;
    let n = match s.field {
        Some(f) => s.records.iter().filter(|&&r| !matches!(s.db.get(r, f), None | Some(Value::Empty))).count(),
        None => s.records.len(),
    };
    num_val(n as f64)
}

fn dget(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    let s = select(a, c)?;
    let f = s.field.ok_or(CellError::Value)?;
    // Records whose field is blank (or "") don't count, as in Excel.
    let mut found =
        s.records.iter().filter_map(|&r| s.db.get(r, f)).filter(|v| !matches!(v, Value::Empty) && !matches!(v, Value::Text(t) if t.is_empty()));
    match (found.next(), found.next()) {
        (None, _) => Err(CellError::Value),
        (Some(v), None) => Ok(v.clone()),
        _ => Err(CellError::Num),
    }
}

fn dmax(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    let v = field_numbers(a, c)?;
    num_val(v.iter().copied().reduce(f64::max).unwrap_or(0.0))
}

fn dmin(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    let v = field_numbers(a, c)?;
    num_val(v.iter().copied().reduce(f64::min).unwrap_or(0.0))
}

fn dproduct(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    let v = field_numbers(a, c)?;
    if v.is_empty() {
        return num_val(0.0);
    }
    num_val(v.iter().product())
}

fn variance(v: &[f64], sample: bool) -> R<f64> {
    let n = v.len() as f64;
    let min = if sample { 2.0 } else { 1.0 };
    if n < min {
        return Err(CellError::Div0);
    }
    let mean = v.iter().sum::<f64>() / n;
    let ss: f64 = v.iter().map(|x| (x - mean) * (x - mean)).sum();
    Ok(ss / if sample { n - 1.0 } else { n })
}

fn dvar(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    num_val(variance(&field_numbers(a, c)?, true)?)
}

fn dvarp(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    num_val(variance(&field_numbers(a, c)?, false)?)
}

fn dstdev(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    num_val(variance(&field_numbers(a, c)?, true)?.sqrt())
}

fn dstdevp(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    num_val(variance(&field_numbers(a, c)?, false)?.sqrt())
}

pub(crate) fn specs() -> Vec<FnSpec> {
    vec![
        f!("DSUM", 3, 3, Database, A, "DSUM(database, field, criteria)", "Adds the numbers in a field of the records that meet the criteria.", dsum),
        f!(
            "DAVERAGE",
            3,
            3,
            Database,
            A,
            "DAVERAGE(database, field, criteria)",
            "Averages the numbers in a field of the records that meet the criteria.",
            daverage
        ),
        f!(
            "DCOUNT",
            3,
            3,
            Database,
            A,
            "DCOUNT(database, [field], criteria)",
            "Counts numeric cells in a field of the matching records (or the matching records).",
            dcount
        ),
        f!(
            "DCOUNTA",
            3,
            3,
            Database,
            A,
            "DCOUNTA(database, [field], criteria)",
            "Counts non-blank cells in a field of the matching records (or the matching records).",
            dcounta
        ),
        f!(
            "DGET",
            3,
            3,
            Database,
            A,
            "DGET(database, field, criteria)",
            "Returns the field value of the single record that meets the criteria.",
            dget
        ),
        f!("DMAX", 3, 3, Database, A, "DMAX(database, field, criteria)", "Largest number in a field of the matching records.", dmax),
        f!("DMIN", 3, 3, Database, A, "DMIN(database, field, criteria)", "Smallest number in a field of the matching records.", dmin),
        f!(
            "DPRODUCT",
            3,
            3,
            Database,
            A,
            "DPRODUCT(database, field, criteria)",
            "Multiplies the numbers in a field of the matching records.",
            dproduct
        ),
        f!(
            "DSTDEV",
            3,
            3,
            Database,
            A,
            "DSTDEV(database, field, criteria)",
            "Sample standard deviation of a field over the matching records.",
            dstdev
        ),
        f!(
            "DSTDEVP",
            3,
            3,
            Database,
            A,
            "DSTDEVP(database, field, criteria)",
            "Population standard deviation of a field over the matching records.",
            dstdevp
        ),
        f!("DVAR", 3, 3, Database, A, "DVAR(database, field, criteria)", "Sample variance of a field over the matching records.", dvar),
        f!("DVARP", 3, 3, Database, A, "DVARP(database, field, criteria)", "Population variance of a field over the matching records.", dvarp),
    ]
}

#[cfg(test)]
mod tests {
    use crate::util::testutil::*;
    use gridcraft_core::{CellError, Value};

    /// The classic orchard table.
    fn db() -> Value {
        arr(vec![
            vec![tv("Tree"), tv("Height"), tv("Age"), tv("Yield"), tv("Profit")],
            vec![tv("Apple"), nv(18.0), nv(20.0), nv(14.0), nv(105.0)],
            vec![tv("Pear"), nv(12.0), nv(12.0), nv(10.0), nv(96.0)],
            vec![tv("Cherry"), nv(13.0), nv(14.0), nv(9.0), nv(105.0)],
            vec![tv("Apple"), nv(14.0), nv(15.0), nv(10.0), nv(75.0)],
            vec![tv("Pear"), nv(9.0), nv(8.0), nv(8.0), nv(76.8)],
            vec![tv("Apple"), nv(8.0), nv(9.0), nv(6.0), nv(45.0)],
        ])
    }

    /// Apple with height > 10, OR any pear.
    fn crit() -> Value {
        arr(vec![
            vec![tv("Tree"), tv("Height"), tv("Age"), tv("Yield"), tv("Profit"), tv("Height")],
            vec![tv("=Apple"), tv(">10"), Value::Empty, Value::Empty, Value::Empty, tv("<16")],
            vec![tv("=Pear"), Value::Empty, Value::Empty, Value::Empty, Value::Empty, Value::Empty],
        ])
    }

    fn d(f: &str, field: crate::Arg) -> Value {
        ev(f, vec![rf(db()), field, rf(crit())])
    }

    #[test]
    fn orchard() {
        close(d("DCOUNT", t("Age")), 3.0);
        close(d("DCOUNTA", t("Profit")), 3.0);
        close(d("DMAX", t("Profit")), 96.0);
        close(d("DMIN", t("Profit")), 75.0);
        close(d("DSUM", t("Profit")), 247.8);
        close(d("DSUM", n(5.0)), 247.8);
        close(d("DPRODUCT", t("Yield")), 800.0);
        close(d("DAVERAGE", t("Yield")), 28.0 / 3.0);
        close_tol(d("DSTDEV", t("Yield")), 1.154700538, 1e-9);
        close_tol(d("DSTDEVP", t("Yield")), 0.942809042, 1e-9);
        close_tol(d("DVAR", t("Yield")), 1.333333333, 1e-9);
        close_tol(d("DVARP", t("Yield")), 0.888888889, 1e-9);
        close(d("DCOUNT", empty()), 3.0);
        is_err(d("DSUM", t("Nope")), CellError::Value);
        is_err(d("DSUM", n(9.0)), CellError::Value);
    }

    /// Excel 365 on a table whose labels include the logical TRUE, the number 1 and the text
    /// "true": labels compare as they read, a logical field is a column number, a label row alone
    /// or a field of two cells is `#VALUE!`, and an error in a summed field is the result.
    #[test]
    fn labels_fields_and_errors() {
        let table = arr(vec![
            vec![tv("Region"), Value::Bool(true), nv(1.0), tv("true"), tv("Revenue")],
            vec![tv("East"), tv("a"), tv("b"), tv("b"), nv(10.0)],
            vec![tv("West"), tv("b"), Value::Empty, tv("a"), nv(20.0)],
            vec![tv("East"), tv("A"), tv("b"), tv("x"), nv(40.0)],
        ]);
        let crit = |label: Value, cond: &str| arr(vec![vec![label], vec![tv(cond)]]);
        let dsum = |c: Value| ev("DSUM", vec![rf(table.clone()), t("Revenue"), rf(c)]);
        // The logical TRUE and the text "true" both name the column headed TRUE, the first that
        // reads "TRUE"; the number 1 names the column headed 1.
        close(dsum(crit(Value::Bool(true), "a")), 50.0);
        close(dsum(crit(tv("true"), "<>a")), 20.0);
        close(dsum(crit(nv(1.0), "b")), 50.0);
        // TRUE as the field is column 1; "true" the column headed TRUE.
        let all = crit(tv("Region"), "*");
        close(ev("DCOUNTA", vec![rf(table.clone()), b(true), rf(all.clone())]), 3.0);
        close(ev("DCOUNT", vec![rf(table.clone()), b(true), rf(all.clone())]), 0.0);
        close(ev("DCOUNTA", vec![rf(table.clone()), t("true"), rf(all.clone())]), 3.0);
        // A field of two cells, and a database that is only its labels.
        let two = arr(vec![vec![tv("Region"), tv("Revenue")]]);
        is_err(ev("DSUM", vec![rf(table.clone()), rf(two), rf(all.clone())]), CellError::Value);
        let labels = arr(vec![vec![tv("Region"), tv("Revenue")]]);
        for f in ["DSUM", "DCOUNT", "DCOUNTA", "DAVERAGE", "DMIN", "DPRODUCT"] {
            is_err(ev(f, vec![rf(labels.clone()), t("Revenue"), rf(all.clone())]), CellError::Value);
        }
        // An error in the field of a matching record is the result of the numeric functions;
        // DCOUNT skips it and DCOUNTA counts it.
        let with_error =
            arr(vec![vec![tv("Month"), tv("Avg")], vec![nv(1.0), nv(30.0)], vec![nv(2.0), Value::Error(CellError::NA)], vec![nv(2.0), nv(40.0)]]);
        let feb = arr(vec![vec![tv("Month")], vec![nv(2.0)]]);
        let jan = arr(vec![vec![tv("Month")], vec![nv(1.0)]]);
        for f in ["DSUM", "DAVERAGE", "DMAX", "DMIN", "DPRODUCT"] {
            is_err(ev(f, vec![rf(with_error.clone()), t("Avg"), rf(feb.clone())]), CellError::NA);
        }
        close(ev("DAVERAGE", vec![rf(with_error.clone()), t("Avg"), rf(jan)]), 30.0);
        close(ev("DCOUNT", vec![rf(with_error.clone()), t("Avg"), rf(feb.clone())]), 1.0);
        close(ev("DCOUNTA", vec![rf(with_error), t("Avg"), rf(feb)]), 2.0);
    }

    /// Computed criteria are no longer asked for once the host is out of work.
    #[test]
    fn matching_stops_asking_computed_criteria_when_exhausted() {
        struct Spent {
            asked: usize,
        }
        impl crate::Ctx for Spent {
            fn date_system(&self) -> gridcraft_core::DateSystem {
                gridcraft_core::DateSystem::D1900
            }
            fn now_serial(&self) -> f64 {
                0.0
            }
            fn random(&mut self) -> f64 {
                0.0
            }
            fn computed_criterion(&mut self, _: usize, _: usize, _: &gridcraft_core::Array, _: usize) -> Option<bool> {
                self.asked += 1;
                Some(true)
            }
            fn exhausted(&self) -> bool {
                self.asked > 0
            }
        }
        let criteria = arr(vec![vec![tv("Formula")], vec![nv(1.0)]]);
        let mut ctx = Spent { asked: 0 };
        let spec = crate::lookup("DCOUNT").unwrap();
        crate::call(spec, &[rf(db()), t("Age"), rf(criteria)], &mut ctx);
        assert_eq!(ctx.asked, 1);
        // Within one criteria row, the conditions after the one that spent the work are skipped too.
        let criteria = arr(vec![vec![tv("Formula"), tv("Other")], vec![nv(1.0), nv(1.0)]]);
        let mut ctx = Spent { asked: 0 };
        crate::call(spec, &[rf(db()), t("Age"), rf(criteria)], &mut ctx);
        assert_eq!(ctx.asked, 1);
    }

    /// Security: records times criteria conditions past the selection budget is `#NUM!` before
    /// any record is tested; a nonmatching criteria table used to test every record against
    /// every criteria row.
    #[test]
    fn selection_work_is_bounded() {
        use gridcraft_core::Array;
        let column = |label: &str, len: usize, v: Value| {
            let mut data = vec![tv(label)];
            data.extend(std::iter::repeat_n(v, len));
            Value::from(Array::column(data))
        };
        let db = column("Tree", 100_000, tv("Apple"));
        let wide = column("Tree", 5_001, tv("=Plum"));
        is_err(ev("DCOUNTA", vec![rf(db.clone()), t("Tree"), rf(wide)]), CellError::Num);
        let narrow = column("Tree", 2, tv("=Apple"));
        close(ev("DCOUNTA", vec![rf(db.clone()), t("Tree"), rf(narrow)]), 100_000.0);
        // Empty criteria rows test no condition: the first one matches every record, so they cost nothing.
        let empty_rows = column("Tree", 5_001, Value::Empty);
        close(ev("DCOUNTA", vec![rf(db), t("Tree"), rf(empty_rows)]), 100_000.0);
    }

    #[test]
    fn simple_criteria() {
        let c = |cells: Vec<Value>| arr(vec![vec![tv("Tree"), tv("Height")], cells]);
        close(ev("DSUM", vec![rf(db()), t("Profit"), rf(c(vec![tv("Apple"), Value::Empty]))]), 225.0);
        // Plain text matches values that begin with it.
        close(ev("DCOUNT", vec![rf(db()), t("Age"), rf(c(vec![tv("P"), Value::Empty]))]), 2.0);
        close(ev("DCOUNT", vec![rf(db()), t("Age"), rf(c(vec![tv("=P"), Value::Empty]))]), 0.0);
        close(ev("DSUM", vec![rf(db()), t("Yield"), rf(c(vec![Value::Empty, tv(">=13")]))]), 33.0);
        close(ev("DSUM", vec![rf(db()), t("Yield"), rf(c(vec![Value::Empty, nv(18.0)]))]), 14.0);
        close(ev("DSUM", vec![rf(db()), t("Yield"), rf(c(vec![Value::Empty, Value::Empty]))]), 57.0);
        close(ev("DSUM", vec![rf(db()), t("Yield"), rf(c(vec![tv("*e*"), tv("<10")]))]), 14.0);
        // Header only: every record matches.
        close(ev("DCOUNTA", vec![rf(db()), t("tree"), rf(arr(vec![vec![tv("Tree")]]))]), 6.0);
        // Unknown criteria label never matches.
        close(ev("DCOUNT", vec![rf(db()), t("Age"), rf(arr(vec![vec![tv("Color")], vec![tv("red")]]))]), 0.0);
    }

    #[test]
    fn dget_and_empty() {
        let c = |tree: &str| arr(vec![vec![tv("Tree")], vec![tv(tree)]]);
        close(ev("DGET", vec![rf(db()), t("Yield"), rf(c("=Cherry"))]), 9.0);
        is_err(ev("DGET", vec![rf(db()), t("Yield"), rf(c("=Apple"))]), CellError::Num);
        is_err(ev("DGET", vec![rf(db()), t("Yield"), rf(c("=Plum"))]), CellError::Value);
        is_err(ev("DAVERAGE", vec![rf(db()), t("Yield"), rf(c("=Plum"))]), CellError::Div0);
        close(ev("DMAX", vec![rf(db()), t("Yield"), rf(c("=Plum"))]), 0.0);
        close(ev("DPRODUCT", vec![rf(db()), t("Yield"), rf(c("=Plum"))]), 0.0);
        is_err(ev("DSTDEV", vec![rf(db()), t("Yield"), rf(c("=Cherry"))]), CellError::Div0);
        close(ev("DSTDEVP", vec![rf(db()), t("Yield"), rf(c("=Cherry"))]), 0.0);
        is_err(ev("DSUM", vec![e(CellError::Ref), t("Yield"), rf(c("=Cherry"))]), CellError::Ref);
        close(ev("DCOUNTA", vec![rf(db()), empty(), rf(c("=Pear"))]), 2.0);
    }

    /// Expected results are Excel's, from Apache POI's `DGet.xls` (rows 74-86).
    #[test]
    fn empty_string_criterion() {
        let db = arr(vec![
            vec![tv("Index"), tv("Value")],
            vec![tv("a"), tv("=")],
            vec![tv("b"), Value::Empty],
            vec![tv("c"), tv("")],
            vec![tv("d"), tv(" ")],
            vec![tv("f"), Value::Error(CellError::Value)],
            vec![tv("h"), tv("abc")],
            vec![tv("i"), nv(0.0)],
        ]);
        let get = |index: &str| {
            let crit = arr(vec![vec![tv("Index"), tv("Value")], vec![tv(index), tv("")]]);
            ev("DGET", vec![rf(db.clone()), t("Index"), rf(crit)])
        };
        // Like any plain text, "" is a "begins with" pattern: it matches every text value.
        is_text(get("c"), "c");
        is_text(get("h"), "h");
        is_err(get("b"), CellError::Value);
        is_err(get("i"), CellError::Value);
        is_err(get("f"), CellError::Value);
        let all = arr(vec![vec![tv("Value")], vec![tv("")]]);
        close(ev("DCOUNTA", vec![rf(db.clone()), t("Index"), rf(all)]), 4.0);

        // Under a label that names no field, "" is a condition that never holds.
        let db = arr(vec![vec![tv("One"), tv("Two"), tv("Three")], vec![tv("a"), tv("c"), tv("x")], vec![tv("b"), tv("d"), tv("y")]]);
        let get = |label: Value, value: Value| {
            let crit = arr(vec![vec![tv("One"), label, tv("Three")], vec![tv("a"), value, tv("x")]]);
            ev("DGET", vec![rf(db.clone()), t("One"), rf(crit)])
        };
        is_text(get(Value::Empty, Value::Empty), "a");
        is_text(get(tv(""), Value::Empty), "a");
        is_err(get(Value::Empty, tv("")), CellError::Value);
        is_err(get(tv(""), tv("")), CellError::Value);
    }

    /// Expected results are Excel's, from Apache POI's `DGet.xls` (rows 108-119).
    #[test]
    fn dget_skips_empty_fields() {
        let db = arr(vec![
            vec![tv("One"), tv("Two")],
            vec![tv("a"), Value::Empty],
            vec![tv("b"), tv(" ")],
            vec![tv("c"), tv("")],
            vec![tv("d"), Value::Error(CellError::NA)],
            vec![tv("j"), Value::Empty],
            vec![tv("j"), tv("Text")],
            vec![tv("j"), Value::Empty],
            vec![tv("k"), Value::Empty],
            vec![tv("k"), tv("")],
        ]);
        let get = |one: &str| {
            let crit = arr(vec![vec![tv("One")], vec![tv(one)]]);
            ev("DGET", vec![rf(db.clone()), t("Two"), rf(crit)])
        };
        is_err(get("a"), CellError::Value);
        is_text(get("b"), " ");
        is_err(get("c"), CellError::Value);
        is_err(get("d"), CellError::NA);
        is_text(get("j"), "Text");
        is_err(get("k"), CellError::Value);
    }
}
