//! Caches an older Excel computed with what it had: a function's algorithm before Microsoft
//! replaced it, or no function at all. Such a cache is Excel's answer for its own version, not for
//! the one GridCraft models, and its mismatches are listed as
//! [`CacheTrust::LegacyExcelVersion`](super::CacheTrust::LegacyExcelVersion).

use super::{CalcProps, Producer, Value};
use gridcraft_engine::core::CellError;
use gridcraft_engine::formula::Expr;

/// Excel 2003 and Excel 2010, the versions that replaced the algorithms of
/// [`IMPROVED_IN_EXCEL_2003`] and [`IMPROVED_IN_EXCEL_2010`].
const EXCEL_2003: u32 = 11;
const EXCEL_2010: u32 = 14;
/// Excel 2013, the last version before Excel 2016 changed what a function does with text
/// ([`TYPED_TEXT_SKIPPED_SINCE_EXCEL_2016`], [`ONE_CELL_REFERENCE_READ_AS_ARRAY_SINCE_EXCEL_2016`]).
const EXCEL_2013: u32 = 15;

/// The functions whose algorithm Excel 2003 (and Excel 2004 for Mac, "updated by using the same
/// algorithms") replaced and Excel 2010 didn't again: "Description of improvements in the
/// statistical functions in Excel 2003 and in Excel 2004 for Mac" (Microsoft KB 828888) names
/// LINEST's QR decomposition and the functions that call it (LOGEST, TREND, GROWTH); the
/// two-pass sum of squared deviations of VAR, VARA, VARP, VARPA, STDEV, STDEVA, STDEVP, STDEVPA,
/// PEARSON, SLOPE and STEYX, and the functions that call one of them (FORECAST, INTERCEPT, RSQ,
/// TTEST, ZTEST); and DVAR, DVARP, DSTDEV and DSTDEVP. Its other functions (NORMSDIST and those
/// over it, the inverse distributions, the discrete distributions) Excel 2010 replaced again.
const IMPROVED_IN_EXCEL_2003: [&str; 19] = [
    "LOGEST",
    "TREND",
    "GROWTH",
    "VARA",
    "VARP",
    "VARPA",
    "STDEVA",
    "STDEVP",
    "STDEVPA",
    "PEARSON",
    "SLOPE",
    "STEYX",
    "FORECAST",
    "INTERCEPT",
    "RSQ",
    "DVAR",
    "DVARP",
    "DSTDEV",
    "DSTDEVP",
];

/// The functions whose algorithm Excel 2010 replaced "for better accuracy" ("Changes in Excel
/// 2010", Microsoft Learn: *Updated statistical functions*, and the additional functions "for
/// which the accuracy is improved"; "What's new: Changes made to Excel functions", Microsoft
/// Support: *Functions with accuracy improvements*, which adds BETADIST, BETAINV, CUMIPMT,
/// CUMPRINC, FACTDOUBLE, IMLOG2, IMPOWER, IPMT, IRR, LINEST, MOD, PMT, PPMT and XIRR), and those
/// Microsoft defines through one of those distributions (CONFIDENCE and ZTEST over the normal,
/// TTEST over Student's t, CHITEST over chi-squared, FTEST over F). RAND, also on the list, is
/// volatile and never compared.
const IMPROVED_IN_EXCEL_2010: [&str; 50] = [
    "BINOMDIST",
    "CRITBINOM",
    "CHIDIST",
    "CHIINV",
    "EXPONDIST",
    "FDIST",
    "FINV",
    "GAMMADIST",
    "GAMMAINV",
    "HYPGEOMDIST",
    "LOGNORMDIST",
    "LOGINV",
    "NEGBINOMDIST",
    "NORMDIST",
    "NORMINV",
    "NORMSDIST",
    "NORMSINV",
    "POISSON",
    "TDIST",
    "TINV",
    "WEIBULL",
    "ASINH",
    "CEILING",
    "CONVERT",
    "ERF",
    "ERFC",
    "FLOOR",
    "GAMMALN",
    "GEOMEAN",
    "STDEV",
    "VAR",
    "CONFIDENCE",
    "ZTEST",
    "TTEST",
    "CHITEST",
    "FTEST",
    "BETADIST",
    "BETAINV",
    "CUMIPMT",
    "CUMPRINC",
    "FACTDOUBLE",
    "IMLOG2",
    "IMPOWER",
    "IPMT",
    "IRR",
    "LINEST",
    "MOD",
    "PMT",
    "PPMT",
    "XIRR",
];

/// How far a pre-2010 result may be from the current algorithm's and still be the old
/// algorithm's inaccuracy: relative to the larger result, or absolute for results near 0.
/// Microsoft documents that the earlier searches and series stopped short ("Excel Statistical
/// Functions: NORMSINV", KB 826772: inaccuracies of NORMSDIST and of the search over it in
/// Excel 2002 and earlier) without bounding the error; the Excel 97–2003 caches of the corpus
/// differ by at most 9e-5 relative (NORMINV) and 5e-9 absolute (a NORMSDIST tail).
const RELATIVE: f64 = 1e-4;
const ABSOLUTE: f64 = 1e-6;

/// The major version of the Excel that calculated the cache, when the file says Excel did: the
/// `<calcPr calcId>` of an XLSX (the calculation engine, `124519` → 12), or else the producer's
/// version (`AppVersion`, or what an `.xls` says, see [`super::xls_meta`]).
pub(super) fn excel_version(producer: &Producer, props: &CalcProps) -> Option<u32> {
    if !producer.kind.is_excel() {
        return None;
    }
    props.calc_id.filter(|id| (100_000..1_000_000).contains(id)).map(|id| id / 10_000).or_else(|| {
        let v = producer.version.as_deref()?;
        v.split('.').next()?.trim().parse().ok()
    })
}

/// Whether a mismatch of the formula `expr` (calling `funcs`), cached `expected` where GridCraft
/// gives `actual`, is the calculating Excel's version (`version`, see [`excel_version`]) and not
/// GridCraft's error:
/// - `#NAME?` cached for a function newer than Excel 2007's file format (`is_future`: one files
///   write as `_xlfn.NAME`), which the Excel that calculated didn't have ("An _xlfn. prefix is
///   displayed in front of a formula": the version running doesn't support the function);
/// - the formula is one call (in parentheses or not) of a function Excel 2003 or Excel 2010 made
///   more accurate, cached by an Excel older than that, and the cache is what the old algorithm
///   can give ([`old_algorithm_s`]): the cache is that call's own result;
/// - or a number within `propagated(version)`: the most the formula's arithmetic can carry from
///   such calls taken within the old algorithms' accuracy ([`old_accuracy`]), each through what
///   the formula does with it, and nothing else (the caller bounds it, see
///   [`super::noise::Bounds`]). A call the formula multiplies by zero carries nothing, so it
///   explains nothing of the rest (`X+0*IRR(…)`), and a formula that isn't arithmetic around
///   the call has no such bound.
pub(super) fn explains(
    version: Option<u32>,
    expr: &Expr,
    funcs: &[String],
    expected: &Value,
    actual: &Value,
    is_future: &dyn Fn(&str) -> bool,
    propagated: &mut dyn FnMut(u32) -> Option<f64>,
) -> bool {
    let lacked =
        matches!(expected, Value::Error(CellError::Name)) && !matches!(actual, Value::Error(CellError::Name)) && funcs.iter().any(|f| is_future(f));
    if lacked {
        return true;
    }
    let Some(v) = version else { return false };
    let mut root = expr;
    while let Expr::Paren(inner) = root {
        root = inner;
    }
    if let Expr::Call(name, _) = root
        && improved_since(v, name)
        && old_algorithm_s(expected, actual)
    {
        return true;
    }
    match (expected, actual) {
        (Value::Number(e), Value::Number(a)) if funcs.iter().any(|f| improved_since(v, f)) => propagated(v).is_some_and(|err| (e - a).abs() <= err),
        _ => false,
    }
}

/// How far the old algorithm's result of a call may be from the current one's `v`: the accuracy
/// of [`old_algorithm_s`] as a bound around `v` (`1e-4` of the larger of the two, so of `v` over
/// `1 − 1e-4`, or `1e-6` absolute).
pub(super) fn old_accuracy(v: f64) -> f64 {
    ABSOLUTE.max(RELATIVE * v.abs() / (1.0 - RELATIVE))
}

/// Whether a mismatch of the formula `expr`, cached `#VALUE!` by Excel `version` where GridCraft
/// gives `actual`, is how Excel 2013 and earlier took a text argument that Excel 2016 reads: the
/// formula is one call (in parentheses or not) of [`TYPED_TEXT_SKIPPED_SINCE_EXCEL_2016`] with a
/// text typed in it as an argument, whose result is a logical for AND, OR and XOR, and anything
/// but `#VALUE!` for MODE. The cache is the call's own result, so what the version changed is all
/// that differs.
pub(super) fn typed_text_explains(version: Option<u32>, expr: &Expr, expected: &Value, actual: &Value) -> bool {
    if !version.is_some_and(|v| v <= EXCEL_2013) || !matches!(expected, Value::Error(CellError::Value)) {
        return false;
    }
    let mut root = expr;
    while let Expr::Paren(inner) = root {
        root = inner;
    }
    let Expr::Call(name, args) = root else { return false };
    let name = name.as_str();
    if !TYPED_TEXT_SKIPPED_SINCE_EXCEL_2016.contains(&name) {
        return false;
    }
    let result_fits = if name.starts_with("MODE") { !matches!(actual, Value::Error(CellError::Value)) } else { matches!(actual, Value::Bool(_)) };
    result_fits && args.iter().any(|a| matches!(a, Expr::Text(_)))
}

/// Whether Excel `version` read the SUMPRODUCT of a formula calling `funcs` as Excel 2013 and
/// earlier read a reference to one cell in it ([`ONE_CELL_REFERENCE_READ_AS_ARRAY_SINCE_EXCEL_2016`]):
/// as the value in the cell.
pub(super) fn reads_one_cell_as_value(version: Option<u32>, funcs: &[String]) -> bool {
    version.is_some_and(|v| v <= EXCEL_2013) && funcs.iter().any(|f| f == ONE_CELL_REFERENCE_READ_AS_ARRAY_SINCE_EXCEL_2016)
}

/// Whether a mismatch of a formula calling `funcs`, cached by Excel `version`, is how Excel 2013
/// and earlier read a reference to one cell in SUMPRODUCT
/// ([`ONE_CELL_REFERENCE_READ_AS_ARRAY_SINCE_EXCEL_2016`]) and not GridCraft's error. Excel 2016
/// changed that reading and nothing else of the formula, so the proof is the older reading itself:
/// `older_reading_gives_cache` evaluates the formula as GridCraft calculated it (every input
/// holding GridCraft's value) with SUMPRODUCT taking a reference to one cell as the value in it and
/// nothing else changed (see `calc::one_cell_reading_legacy`), and says whether that gives the
/// cache back. Wherever the call is in the formula (inside an IF, the dividend of a division), and
/// whatever the cache is (`#VALUE!` from the call, the `#VALUE!` a division passes on ahead of its
/// own `#DIV/0!`, the fallback of an IFERROR), a formula whose cache the older reading doesn't
/// give back is a mismatch for another reason, and gates.
pub(super) fn one_cell_reference_explains(version: Option<u32>, funcs: &[String], older_reading_gives_cache: impl FnOnce() -> bool) -> bool {
    reads_one_cell_as_value(version, funcs) && older_reading_gives_cache()
}

/// Whether Excel replaced the algorithm of `f` after Excel `version`.
pub(super) fn improved_since(version: u32, f: &str) -> bool {
    (version < EXCEL_2010 && IMPROVED_IN_EXCEL_2010.contains(&f)) || (version < EXCEL_2003 && IMPROVED_IN_EXCEL_2003.contains(&f))
}

/// The functions that skip a text argument typed in the formula since Excel 2016 where earlier
/// versions answered `#VALUE!`. The same formulas, cached by two Excels: poi
/// FormulaEvalTestData_Copy.xlsx (Excel 15.0300) caches `#VALUE!` for `MODE(C9,E16,"1",B8)`,
/// `MODE(B8,"1",2,3)`, `MODE("1",1,#DIV/0!,2,3)` and `AND("1",1)`, and poi
/// FormulaEvalTestData.xls, saved by Excel 16 (Office16 paths in its VBA project), caches `#N/A`,
/// `#N/A`, `#DIV/0!` and `TRUE` for them, as Excel 365 answers (GridCraft #736; Excel 16.114
/// calculates `AND("1",1)` as TRUE). OR and XOR take their arguments as AND does.
const TYPED_TEXT_SKIPPED_SINCE_EXCEL_2016: [&str; 6] = ["MODE", "MODE.SNGL", "MODE.MULT", "AND", "OR", "XOR"];

/// SUMPRODUCT, which since Excel 2016 reads a reference that holds one cell as an array, so text
/// there counts as 0, however the reference is written: Excel 16.114 calculates `SUMPRODUCT(A1,B1)`,
/// `SUMPRODUCT(A1:A1,B1:B1)`, `SUMPRODUCT(INDEX(A1:C3,2,2),B1)` and
/// `SUMPRODUCT(INDEX(A1:C3,2,2):INDEX(A1:C3,2,2),B1)` over the text "4" (or an empty text) and 2 as
/// 0, with the number in either argument and the reference written in any of those ways. Excel
/// 2013 and earlier read it as the value in the cell and answered `#VALUE!` for text. The same
/// formula over the text "4" and 2, cached by two Excels: `SUMPRODUCT(G1330:G1330,I1330:I1330)`
/// is `#VALUE!` in poi FormulaEvalTestData_Copy.xlsx (Excel 15.0300) and 0 as
/// `SUMPRODUCT(G1370:G1370,I1370:I1370)` in poi FormulaEvalTestData.xls (Excel 16). Two workbooks
/// saved by Excel 15.0300 cache
/// `#VALUE!` in 156 cells of `IF(…,SUMPRODUCT(INDEX(…):INDEX(…),INDEX(…):INDEX(…))/SQRT(SUM(…)*SUM(…)),…)`
/// where the first `INDEX(…):INDEX(…)` is one cell holding an empty text: Excel 16.114 calculates a
/// replica of the formula as `#DIV/0!` (SUMPRODUCT is 0, divided by `SQRT(0)`), and `#VALUE!` is
/// what the Excel that answers it for SUMPRODUCT passes on, since the error of a division's left
/// operand comes before its division by zero (`=NA()/0` is `#N/A`, `=("a"+1)/SQRT(0)` `#VALUE!`).
const ONE_CELL_REFERENCE_READ_AS_ARRAY_SINCE_EXCEL_2016: &str = "SUMPRODUCT";

/// The old answer of a replaced algorithm where the Excel that calculated the cache had another one
/// than GridCraft's: only the pairs with evidence, each for the inputs it is evidenced for, and never
/// "some other error". A function or an input with no entry here is not substituted, whatever the
/// cache says; every entry is backed by a cache that version saved or a published table:
/// - PMT with no periods: `#DIV/0!` to Excel 2007 (two workbooks calculated by Excel 12) where Excel
///   2010 and later give `#NUM!` (`PMT(0,0,0,0,0)`, Excel 16.114);
/// - IRR with no result: "XL: IRR Returns #DIV/0! Instead of #NUM! If No IRR Exists" (Microsoft KB
///   30567; Excel 97, 2000 and 2002), a version before Excel 2003;
/// - IRR whose search gave up, Excel 2007 only, for the flows of a cache that shows it
///   ([`EXCEL_2007_IRR_GIVE_UPS`], an Excel 2007 workbook's `NA` cells) with the default guess. No model of the
///   search stands in for more: Newton on the rate gives up on flows Excel 2002 solves
///   (a published homework answer key cached by Excel 2002: `IRR(C1:F1,22%)` over -85, 120, 100, -138 is 0.45188) and on
///   `{-5,1}`, which Excel 16.115 solves (-0.8); and for a workbook's flows where Excel 16.115
///   answers `#NUM!` and GridCraft's fallback search finds -0.388, no older answer is put in place
///   of a number GridCraft shouldn't give;
/// - MOD past the limit of Excel 2003 and earlier: `#NUM!` when the number is [`MOD_OLD_QUOTIENT`]
///   (2^27) times the divisor or more, gone in Excel 2007 (Pearson, "The MOD Function").
///
/// NORMSINV and NORMINV have no entry: no Excel 97 to 2002 cache or published table of them is `#N/A`
/// (Microsoft KB 826772; McCullough and Wilson 2005, Table 1; Knüsel: Excel XP and 97 give numbers
/// at `p` of 1E-6, 3E-7 and 2E-7), so their old results are numbers within [`RELATIVE`] only.
///
/// `args` are the call's arguments as GridCraft calculated them and `now` its value; the caller
/// has proved that those arguments are the ones Excel calculated with (`calc::old_answer_legacy`).
pub(super) fn old_answer(version: u32, f: &str, args: &[Value], now: &Value) -> Option<CellError> {
    if !improved_since(version, f) {
        return None;
    }
    let arg = |i: usize| args.get(i).and_then(scalar_number);
    match (f, now) {
        ("PMT", Value::Error(CellError::Num)) if arg(1) == Some(0.0) => Some(CellError::Div0),
        ("IRR", Value::Error(CellError::Num)) if version < EXCEL_2003 => Some(CellError::Div0),
        ("IRR", Value::Number(_)) if version == EXCEL_2007 && irr_2007_gave_up(args) => Some(CellError::Num),
        ("MOD", Value::Number(_)) if version < EXCEL_2007 => match (arg(0), arg(1)) {
            (Some(n), Some(d)) if d != 0.0 && n.abs() >= d.abs() * MOD_OLD_QUOTIENT => Some(CellError::Num),
            _ => None,
        },
        _ => None,
    }
}

/// Excel 2007, whose MOD no longer had the quotient limit of 2003 and earlier, and whose IRR gave
/// up on the flows of [`EXCEL_2007_IRR_GIVE_UPS`].
const EXCEL_2007: u32 = 12;
/// MOD's `#NUM!` limit of Excel 2003 and earlier: the number is at least this times the divisor.
const MOD_OLD_QUOTIENT: f64 = 134_217_728.0;

/// A number as a function argument reads it: a number or an empty cell, from a value or the first
/// of an array.
fn scalar_number(v: &Value) -> Option<f64> {
    match v.scalar() {
        Value::Number(n) => Some(n),
        Value::Empty => Some(0.0),
        _ => None,
    }
}

/// Whether `args` are the inputs of an IRR that an Excel 2007 cache shows giving up
/// ([`EXCEL_2007_IRR_GIVE_UPS`]): the default guess and, as the flows, one of the lists cached.
fn irr_2007_gave_up(args: &[Value]) -> bool {
    let [Value::Array(a)] = args else { return false };
    let mut flows = Vec::with_capacity(a.data.len());
    for v in a.data.iter() {
        match v {
            Value::Number(n) => flows.push(*n),
            _ => return false,
        }
    }
    EXCEL_2007_IRR_GIVE_UPS
        .iter()
        .any(|cached| flows.len() == cached.len() && flows.iter().zip(cached.iter()).all(|(a, b)| (a - b).abs() <= 1e-9 * a.abs().max(b.abs())))
}

/// The cash flows of the IRRs an Excel 2007 cache shows giving up (`#NUM!`, cached `NA` through
/// `IF(ISERROR(IRR(…)),"NA",IRR(…))`) where Excel 2010 and later find the root: an Excel 2007
/// workbook (calcId 125725), `Annual Cash Flows & Returns!G55:L55`,
/// `IRR($F54:G54)` to `IRR($F54:L54)`: the first two to seven flows of row 54, with the roots -94.3% to
/// -25.2% that the next cell, `IRR($F54:M54)` (root -20.0%), no longer gives up on (its cache is the
/// number). Nothing else is evidence that an Excel before 2010 gave up: not a model of its search
/// (Newton on the rate from the documented "20 tries" gives up on `{-5,1}` and on flows Excel 2002
/// solves, a published homework answer key's `IRR(C1:F1,22%)` over -85, 120, 100, -138 is 0.45188), and not an
/// input of another shape.
const EXCEL_2007_IRR_GIVE_UPS: [&[f64]; 6] = [
    &[-3_600_000.0, 203_454.984_000_572_5],
    &[-3_600_000.0, 203_454.984_000_572_5, 200_318.599_277_516_97],
    &[-3_600_000.0, 203_454.984_000_572_5, 200_318.599_277_516_97, 197_179.118_700_298_98],
    &[-3_600_000.0, 203_454.984_000_572_5, 200_318.599_277_516_97, 197_179.118_700_298_98, 194_036.077_748_189_16],
    &[-3_600_000.0, 203_454.984_000_572_5, 200_318.599_277_516_97, 197_179.118_700_298_98, 194_036.077_748_189_16, 190_889.004_623_062_38],
    &[
        -3_600_000.0,
        203_454.984_000_572_5,
        200_318.599_277_516_97,
        197_179.118_700_298_98,
        194_036.077_748_189_16,
        190_889.004_623_062_38,
        187_737.420_093_783_1,
    ],
];

/// Whether `expected` is the old algorithm's number where `actual` is GridCraft's: within its
/// accuracy. An error is never judged here: see [`old_answer`].
fn old_algorithm_s(expected: &Value, actual: &Value) -> bool {
    match (expected, actual) {
        (Value::Number(e), Value::Number(a)) => {
            let diff = (e - a).abs();
            diff <= ABSOLUTE || diff <= RELATIVE * e.abs().max(a.abs())
        }
        _ => false,
    }
}

/// Whether the normal density `NORMDIST(x, mean, sd, FALSE)` is a positive number below the
/// smallest normal double, 2.2250738585072E-308: `ln` of it is `-z²/2 - ln(sd·√(2π))`, and the
/// density is positive for every finite `x`. Excel 2007 answered `#NUM!` for such a result
/// (a workbook calculated by Excel 12: `NORMDIST(220, P1, P2, 0)` cached `#NUM!`), and Excel
/// 2010 and later underflow it to 0 ("Floating-point arithmetic may give inaccurate results in
/// Excel": no denormalized numbers; Excel 16.115 gives 0 for the same cells), which is what the
/// engine gives.
pub(super) fn normal_density_underflows(x: f64, mean: f64, sd: f64) -> bool {
    if !(sd > 0.0 && sd.is_finite() && x.is_finite() && mean.is_finite()) {
        return false;
    }
    let z = (x - mean) / sd;
    let ln_density = -0.5 * z * z - sd.ln() - 0.5 * (2.0 * std::f64::consts::PI).ln();
    ln_density < f64::MIN_POSITIVE.ln() - 1e-9
}

/// Whether `expected` (the cache) is the `#NUM!` the Excel `version` that calculated it answered
/// where `actual` is the 0 an underflowed density is to Excel 2010 and later: the call is
/// `NORMDIST` of Excel 2010's improved functions ([`improved_since`]) and its density
/// underflows (`density_underflows`, evaluated by the caller on the call's arguments).
pub(super) fn underflow_error_explains(
    version: Option<u32>,
    f: &str,
    expected: &Value,
    actual: &Value,
    density_underflows: impl FnOnce() -> bool,
) -> bool {
    f == "NORMDIST"
        && version.is_some_and(|v| improved_since(v, f))
        && matches!(expected, Value::Error(CellError::Num))
        && matches!(actual, Value::Number(n) if *n == 0.0)
        && density_underflows()
}

/// For the human report.
pub(super) fn reason() -> &'static str {
    "the cache was calculated by an older Excel: a function Excel 2003 or 2010 made more accurate, within the old algorithm's accuracy, or the older answer an Excel cache or published source documents for the call's own inputs (PMT with no periods: #DIV/0! to Excel 2007; IRR with no result: #DIV/0! before Excel 2003; IRR of the flows Excel 2007 cached as giving up: #NUM!; MOD past 2^27 times the divisor: #NUM! before Excel 2007) in a formula that gives the cache back with that answer in the call's place and whose call arguments GridCraft calculates as the cache does, or where a normal density underflowed (#NUM! to Excel 2007, 0 since), one the calculating version didn't have (#NAME?), a MODE, AND, OR or XOR whose typed text Excel 2013 and earlier answered with #VALUE!, or a SUMPRODUCT of a reference to one cell, which Excel 2013 and earlier read as the value in it (text there is #VALUE!) and whose formula, calculated as GridCraft does except for that reading, gives the cache back"
}

#[cfg(test)]
mod tests {
    use super::super::ProducerKind;
    use super::*;

    fn funcs(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    /// [`explains`] for the formula `f`, calling `names`, with no bound from the caller (as for a
    /// formula that isn't arithmetic around the call).
    fn ex(version: Option<u32>, f: &str, names: &[&str], expected: &Value, actual: &Value) -> bool {
        ex_bound(version, f, names, expected, actual, None)
    }

    fn ex_bound(version: Option<u32>, f: &str, names: &[&str], expected: &Value, actual: &Value, bound: Option<f64>) -> bool {
        let e = gridcraft_engine::formula::parse(f).unwrap();
        explains(version, &e, &funcs(names), expected, actual, &|f: &str| f == "MINIFS", &mut |_| bound)
    }

    #[test]
    fn the_version_is_the_calculation_engine_s_then_the_application_s() {
        let excel = |v: &str| Producer { application: Some("Microsoft Excel".into()), version: Some(v.into()), kind: ProducerKind::Excel };
        assert_eq!(excel_version(&excel("16.0300"), &CalcProps { calc_id: Some(124519), ..Default::default() }), Some(12));
        assert_eq!(excel_version(&excel("11.0000"), &CalcProps::default()), Some(11));
        assert_eq!(excel_version(&excel("12.0000"), &CalcProps { calc_id: Some(0), ..Default::default() }), Some(12));
        let libre = Producer { kind: ProducerKind::LibreOffice, ..excel("7.3") };
        assert_eq!(excel_version(&libre, &CalcProps::default()), None);
    }

    #[test]
    fn older_algorithms_explain_small_differences_only() {
        let normsinv = ["NORMSINV"];
        let (old, new) = (Value::Number(1.644_853_000_470_902_8), Value::Number(1.644_853_626_951_468_4));
        assert!(ex(Some(11), "NORMSINV(0.95)", &normsinv, &old, &new));
        assert!(ex(Some(11), "(NORMSINV(0.95))", &normsinv, &old, &new));
        assert!(!ex(Some(14), "NORMSINV(0.95)", &normsinv, &old, &new), "Excel 2010 has the new algorithm");
        assert!(!ex(None, "NORMSINV(0.95)", &normsinv, &old, &new));
        assert!(!ex(Some(11), "SUM(1)", &["SUM"], &old, &new), "SUM didn't change");
        assert!(!ex(Some(11), "NORMSINV(0.95)", &normsinv, &Value::Number(1.6), &new), "beyond the old accuracy");
        assert!(ex(Some(9), "NORMSDIST(-4.4)", &["NORMSDIST"], &Value::Number(4.378e-6), &Value::Number(4.3735e-6)));
        assert!(!ex(Some(9), "NORMSINV(0.95)", &normsinv, &Value::Error(CellError::Num), &new));
    }

    /// Inside a larger formula the call explains only what its accuracy carries to the result.
    #[test]
    fn an_older_algorithm_inside_a_formula_explains_what_it_carries() {
        let normsinv = ["NORMSINV"];
        let (old, new) = (Value::Number(3.289_706), Value::Number(3.289_707_2));
        assert!(ex_bound(Some(11), "NORMSINV(0.95)*2", &normsinv, &old, &new, Some(3.3e-4)));
        assert!(!ex_bound(Some(11), "5E-7+0*NORMSINV(0.95)", &normsinv, &Value::Number(0.0), &Value::Number(5e-7), Some(4e-16)));
        assert!(!ex(Some(11), "IF(TRUE,NORMSINV(0.95)*2)", &["IF", "NORMSINV"], &old, &new), "not arithmetic: no bound");
        assert!(!ex_bound(Some(14), "NORMSINV(0.95)*2", &normsinv, &old, &new, Some(3.3e-4)), "Excel 2010 has the new algorithm");
        // The error rules hold for the call itself only.
        let (num, div0) = (Value::Error(CellError::Num), Value::Error(CellError::Div0));
        assert!(!ex(Some(12), "5E-7+0*IRR(A1:A3)", &["IRR"], &num, &Value::Number(5e-7)));
        assert!(!ex(Some(12), "PMT(A1,B1,C1)*12/D1", &["PMT"], &div0, &num));
    }

    #[test]
    fn excel_2003_s_two_pass_functions_explain_an_excel_2000_cache_only() {
        // SLOPE over two points, cached by Excel 2000's one-pass formula (a financial workbook's cache).
        let (old, new) = (Value::Number(-0.160_278_980_669_921_52), Value::Number(-0.160_278_982_256_493_98));
        assert!(ex(Some(9), "SLOPE(F68:F69,G68:G69)", &["SLOPE"], &old, &new));
        assert!(!ex(Some(11), "SLOPE(F68:F69,G68:G69)", &["SLOPE"], &old, &new), "Excel 2003 has the two-pass algorithm");
        assert!(!ex(Some(12), "INTERCEPT(F68:F69,G68:G69)", &["INTERCEPT"], &old, &new));
    }

    #[test]
    fn a_cached_name_error_for_a_newer_function_is_the_version_s() {
        let name = Value::Error(CellError::Name);
        assert!(ex(Some(16), "MINIFS(C7:C10,C7:C10,\">0\")", &["MINIFS"], &name, &Value::Number(76.0)));
        assert!(!ex(Some(16), "SUM(C7:C10)", &["SUM"], &name, &Value::Number(76.0)));
        assert!(!ex(Some(16), "MINIFS(C7:C10,C7:C10,\">0\")", &["MINIFS"], &name, &name));
    }

    /// An error is never the single-call rule's: it has no numbers to compare, and what Excel's older
    /// algorithm answered instead is [`old_answer`]'s, tried by `calc::old_answer_legacy`.
    #[test]
    fn an_error_cache_is_not_a_number_the_old_algorithm_may_have_given() {
        let (num, na, div0) = (Value::Error(CellError::Num), Value::Error(CellError::NA), Value::Error(CellError::Div0));
        let root = Value::Number(-0.9435);
        assert!(!ex(Some(12), "IRR(A1:A3)", &["IRR"], &num, &root));
        assert!(!ex(Some(11), "NORMSINV(A1)", &["NORMSINV"], &na, &root));
        assert!(!ex(Some(12), "PMT(A1,B1,C1)", &["PMT"], &div0, &num));
    }

    #[test]
    fn value_error_for_typed_text_is_excel_2013_s_only_in_its_pattern() {
        let (value, na) = (Value::Error(CellError::Value), Value::Error(CellError::NA));
        let typed = |v: u32, f: &str, actual: &Value| {
            let e = gridcraft_engine::formula::parse(f).unwrap();
            typed_text_explains(Some(v), &e, &value, actual)
        };
        assert!(typed(15, "MODE(C9,E16,\"1\",B8)", &na));
        assert!(typed(12, "(MODE(\"1\",1,#DIV/0!,2,3))", &Value::Error(CellError::Div0)));
        assert!(!typed(16, "MODE(B8,\"1\",2,3)", &na), "Excel 2016 skips typed text");
        assert!(!typed(15, "MODE(B8,C8)", &na), "no typed text");
        assert!(!typed(15, "SUM(\"1\")", &na));
        assert!(!typed_text_explains(Some(15), &gridcraft_engine::formula::parse("MODE(\"1\",1)").unwrap(), &na, &value));
        assert!(typed(15, "AND(\"1\",1)", &Value::Bool(true)));
        assert!(!typed(16, "AND(\"1\",1)", &Value::Bool(true)));
        assert!(!typed(15, "AND(\"1\",1)", &Value::Number(1.0)), "AND gives a logical");
        assert!(!typed(15, "AND(TRUE)", &Value::Bool(true)), "no typed text");
        assert!(!typed(15, "NUMBERVALUE(\"2020-01-01\")+0*AND(\"1\")", &Value::Number(43831.0)), "AND's result isn't the cell's");
        assert!(!typed(15, "SUMPRODUCT(G1330:G1330,I1330:I1330)", &Value::Number(0.0)), "SUMPRODUCT is read again, not matched by its text");
    }

    /// The older reading is tried only for Excel 2013 and earlier and a formula that calls SUMPRODUCT,
    /// and it has to give the cache back.
    #[test]
    fn a_sumproduct_of_a_one_cell_reference_is_excel_2013_s_only_when_its_reading_gives_the_cache_back() {
        let sumproduct = funcs(&["IF", "SUMPRODUCT", "SQRT"]);
        assert!(one_cell_reference_explains(Some(15), &sumproduct, || true));
        assert!(one_cell_reference_explains(Some(9), &sumproduct, || true), "Excel 2000 read it as a value too");
        assert!(!one_cell_reference_explains(Some(15), &sumproduct, || false), "the older reading doesn't give the cache back");
        let mut tried = Vec::new();
        let mut try_reading = |why: &str| {
            tried.push(why.to_string());
            true
        };
        assert!(!one_cell_reference_explains(Some(16), &sumproduct, || try_reading("Excel 2016")), "Excel 2016 reads it as an array");
        assert!(!one_cell_reference_explains(None, &sumproduct, || try_reading("not Excel")), "not an Excel's version");
        assert!(!one_cell_reference_explains(Some(15), &funcs(&["IF", "SUM"]), || try_reading("no SUMPRODUCT")));
        assert!(tried.is_empty(), "the reading is evaluated only where it can explain: {tried:?}");
    }

    /// The reading is the calculating Excel's only for Excel 2013 and earlier and a formula that
    /// calls SUMPRODUCT: what a reader of a legacy cell is evaluated with.
    #[test]
    fn the_older_one_cell_reading_is_excel_2013_s_for_a_formula_calling_sumproduct() {
        let sumproduct = funcs(&["SUMPRODUCT"]);
        assert!(reads_one_cell_as_value(Some(15), &sumproduct));
        assert!(reads_one_cell_as_value(Some(9), &sumproduct));
        assert!(!reads_one_cell_as_value(Some(16), &sumproduct));
        assert!(!reads_one_cell_as_value(None, &sumproduct));
        assert!(!reads_one_cell_as_value(Some(15), &funcs(&["IF", "SUM"])));
    }

    fn number(x: f64) -> Value {
        Value::Number(x)
    }

    /// The older answers of a call: only the documented pair, for the inputs it is documented for,
    /// in the versions that had the older algorithm. No other error, no other function, no other
    /// input.
    #[test]
    fn the_older_answer_of_a_call_is_its_documented_one_for_its_inputs_only() {
        let (num, div0, na, root) = (Value::Error(CellError::Num), Value::Error(CellError::Div0), Value::Error(CellError::NA), number(-0.9435));
        // PMT(0,0,…): Excel 2007 answered #DIV/0!, Excel 2010 and later #NUM!; PMT with periods has none.
        let no_periods = [number(0.0), number(0.0), number(5.0)];
        assert_eq!(old_answer(12, "PMT", &no_periods, &num), Some(CellError::Div0));
        assert_eq!(old_answer(14, "PMT", &no_periods, &num), None, "Excel 2010 has the new algorithm");
        assert_eq!(old_answer(12, "PMT", &[number(0.1), number(2.0), number(5.0)], &num), None, "#NUM! with periods isn't PMT(0,0)");
        assert_eq!(old_answer(12, "PMT", &no_periods, &div0), None, "already #DIV/0!");
        assert_eq!(old_answer(12, "SUM", &no_periods, &num), None, "SUM didn't change");
        // IRR: #DIV/0! for no result before Excel 2003 (KB 30567).
        let column = |flows: &[f64]| Value::from(gridcraft_engine::core::Array::column(flows.iter().map(|x| number(*x)).collect()));
        let flows = column(&[-100.0, 0.0, 121.0]);
        assert_eq!(old_answer(9, "IRR", std::slice::from_ref(&flows), &num), Some(CellError::Div0));
        assert_eq!(old_answer(12, "IRR", std::slice::from_ref(&flows), &num), None, "KB 30567 is Excel 97 to 2002");
        assert_eq!(old_answer(12, "IRR", std::slice::from_ref(&flows), &root), None, "no cache shows Excel 2007 giving up on these");
        // #NUM! only for the flows an Excel 2007 workbook cached `NA` for, with the default guess.
        let gave_up = column(&[-3_600_000.0, 203_454.984_000_572_5, 200_318.599_277_516_97, 197_179.118_700_298_98]);
        assert_eq!(old_answer(12, "IRR", std::slice::from_ref(&gave_up), &root), Some(CellError::Num));
        assert_eq!(old_answer(14, "IRR", std::slice::from_ref(&gave_up), &root), None);
        assert_eq!(old_answer(11, "IRR", std::slice::from_ref(&gave_up), &root), None, "Excel 2003: no cache shows it");
        assert_eq!(old_answer(12, "IRR", &[gave_up.clone(), number(0.5)], &root), None, "another guess");
        let eight = column(&[
            -3_600_000.0,
            203_454.984_000_572_5,
            200_318.599_277_516_97,
            197_179.118_700_298_98,
            194_036.077_748_189_16,
            190_889.004_623_062_38,
            187_737.420_093_783_1,
            184_580.837_337_532_72,
        ]);
        assert_eq!(old_answer(12, "IRR", std::slice::from_ref(&eight), &root), None, "the cache of IRR($F54:M54) is its number");
        // Ordinary flows: Excel 16.115 finds -0.8,
        // -0.5 and 0.45188; Excel 2002 caches 0.45188 for the homework flows at 22%.
        for (flows, guess) in [(vec![-5.0, 1.0], None), (vec![-100.0, 50.0], None), (vec![-85.0, 120.0, 100.0, -138.0], Some(0.22))] {
            let mut args = vec![column(&flows)];
            args.extend(guess.map(number));
            for version in [9, 10, 12] {
                assert_eq!(old_answer(version, "IRR", &args, &root), None, "{flows:?} in Excel {version}");
            }
        }
        // NORMSINV and NORMINV have no entry: published Excel XP and 97 results in the tails are numbers.
        for (version, f, args) in [
            (10, "NORMSINV", vec![number(1e-7)]),
            (10, "NORMSINV", vec![number(1e-6)]),
            (9, "NORMSINV", vec![number(2e-7)]),
            (10, "NORMSINV", vec![number(1.0 - 1e-7)]),
            (10, "NORMINV", vec![number(1e-7), number(0.0), number(1.0)]),
            (12, "NORMSINV", vec![number(1e-7)]),
        ] {
            assert_eq!(old_answer(version, f, &args, &number(-5.2)), None, "{f}({args:?}) in Excel {version}");
        }
        assert_eq!(old_answer(10, "NORMSINV", &[number(1e-7)], &na), None);
        assert_eq!(old_answer(10, "CHIINV", &[number(1e-7), number(3.0)], &number(30.0)), None, "no documented failing input");
        // MOD past 2^27 times the divisor, Excel 2003 and earlier.
        assert_eq!(old_answer(11, "MOD", &[number(940_000_000.0), number(7.0)], &number(1.0)), Some(CellError::Num));
        assert_eq!(old_answer(11, "MOD", &[number(5.0), number(3.0)], &number(2.0)), None);
        assert_eq!(old_answer(12, "MOD", &[number(940_000_000.0), number(7.0)], &number(1.0)), None, "fixed as of Excel 2007");
        assert_eq!(old_answer(11, "MOD", &[number(5.0), number(0.0)], &div0), None, "MOD(5,0) is #DIV/0! in every version");
        // A text or an error where a number is read isn't judged.
        assert_eq!(old_answer(12, "PMT", &[number(0.0), Value::text("0")], &num), None);
    }

    /// An underflowing normal density is below `exp(-708.4)`: past 37.6 deviations. A density of
    /// 1.5e-6 (5 deviations) or at 37 is not one, nor is a standard deviation that isn't positive.
    #[test]
    fn a_normal_density_underflows_past_37_6_deviations() {
        assert!(normal_density_underflows(220.0, 100.0, 1.0));
        assert!(normal_density_underflows(-39.0, 0.0, 1.0));
        assert!(normal_density_underflows(38.0, 0.0, 1.0));
        assert!(!normal_density_underflows(37.0, 0.0, 1.0));
        assert!(!normal_density_underflows(5.0, 0.0, 1.0));
        assert!(!normal_density_underflows(220.0, 100.0, 0.0));
        let (num, zero) = (Value::Error(CellError::Num), Value::Number(0.0));
        assert!(underflow_error_explains(Some(12), "NORMDIST", &num, &zero, || true));
        assert!(!underflow_error_explains(Some(14), "NORMDIST", &num, &zero, || true));
        assert!(!underflow_error_explains(Some(12), "NORMDIST", &num, &zero, || false));
        assert!(!underflow_error_explains(Some(12), "SQRT", &num, &zero, || true));
        assert!(!underflow_error_explains(Some(12), "NORMDIST", &Value::Error(CellError::Div0), &zero, || true));
    }
}
