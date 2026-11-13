#![allow(clippy::unwrap_used)]
use std::io::Write as _;
use std::sync::Arc;

use gridcraft_engine::model::Formula;

use super::calc::error_text_cached;
use super::rels::{Limits, lost_relationships, relationship_tree};
use super::*;

#[test]
fn discovery_reports_a_directory_read_error() {
    let path = std::env::temp_dir().join(format!("gridcraft-check-not-a-directory-{}", std::process::id()));
    std::fs::write(&path, "not a directory").unwrap();
    let mut discovery = Discovery::default();
    let mut seen = std::collections::HashSet::new();
    assert!(walk(&path, 0, &mut discovery, &mut seen));
    assert!(discovery.files.is_empty());
    assert_eq!(discovery.failures.len(), 1);
    assert_eq!(discovery.failures[0].path, path.to_string_lossy());
    assert!(discovery.failures[0].error.as_ref().unwrap().contains("cannot read directory"));
    let _ = std::fs::remove_file(path);
}

#[test]
fn discovery_reports_omissions_only_after_the_file_limit() {
    let mut discovery = Discovery { files: vec![PathBuf::from("book.xlsx"); MAX_FILES - 1], ..Default::default() };
    assert!(discovery.add_file(PathBuf::from("last.xlsx")));
    assert!(discovery.failures.is_empty());
    assert!(!discovery.add_file(PathBuf::from("omitted.xlsx")));
    assert_eq!(discovery.files.len(), MAX_FILES);
    assert_eq!(discovery.failures[0].path, "omitted.xlsx");
    assert!(discovery.failures[0].error.as_ref().unwrap().contains("file limit"));
}

fn at(a1: &str) -> CellRef {
    CellRef::parse(a1).unwrap()
}

/// A workbook whose cells are `(A1, input, cached value)`: an input starting with `=` is a
/// formula with that cached value, anything else a constant.
fn book(cells: &[(&str, &str, Value)]) -> Workbook {
    let mut wb = Workbook::new();
    let sh = wb.sheet_mut(0).unwrap();
    for (a1, input, cached) in cells {
        let cell = if input.starts_with('=') {
            Cell { value: cached.clone(), formula: Some(Arc::new(Formula::new(input))), ..Default::default() }
        } else {
            Cell::value(cached.clone())
        };
        sh.set_cell(at(a1), cell);
    }
    wb
}

fn check(wb: &Workbook, opts: &Options) -> FileReport {
    let bytes = gridcraft_engine::io::save_bytes(wb, "t.xlsx").unwrap();
    check_bytes("t.xlsx", &bytes, opts)
}

fn n(x: f64) -> Value {
    Value::number(x)
}

#[test]
fn matching_cached_values_pass() {
    let wb = book(&[("A1", "2", n(2.0)), ("B1", "=A1*3", n(6.0)), ("C1", "=A1&\"x\"", Value::text("2x")), ("D1", "=A1>1", Value::Bool(true))]);
    let r = check(&wb, &Options::default());
    assert_eq!(r.error, None);
    assert_eq!((r.calc.formula_cells, r.calc.checked, r.calc.matched, r.calc.mismatched), (3, 3, 3, 0));
    assert!(r.is_clean(), "{r:?}");
}

#[test]
fn wrong_cached_value_is_a_mismatch_and_ranks_its_function() {
    // Excel "said" SUM gives 7; GridCraft computes 6. C1 reads B1 and is consistent with
    // Excel's 7, so it mismatches too, but only as a downstream effect.
    let wb = book(&[("A1", "2", n(2.0)), ("A2", "4", n(4.0)), ("B1", "=SUM(A1:A2)", n(7.0)), ("C1", "=B1+1", n(8.0))]);
    let r = check(&wb, &Options::default());
    assert_eq!((r.calc.checked, r.calc.mismatched), (2, 2));
    let first = &r.calc.mismatches[0];
    assert_eq!(first.cell, "Sheet1!B1");
    assert_eq!(first.formula, "=SUM(A1:A2)");
    assert_eq!((first.expected.clone(), first.actual.clone(), first.kind, first.primary), (n(7.0), n(6.0), "numeric", true));
    assert!(!r.calc.mismatches[1].primary);
    assert_eq!(r.calc.functions["SUM"], FnStat { checked: 1, mismatched: 1, primary: 1 });
    assert_eq!(r.calc.functions["(no function)"], FnStat { checked: 1, mismatched: 1, primary: 0 });
    assert!(!r.is_clean());
}

#[test]
fn errors_compare_by_kind() {
    let wb = book(&[("A1", "=1/0", Value::Error(CellError::Div0)), ("A2", "=1/0", Value::Error(CellError::NA)), ("A3", "=1/0", n(0.0))]);
    let r = check(&wb, &Options::default());
    assert_eq!((r.calc.matched, r.calc.mismatched), (1, 2));
    assert!(r.calc.mismatches.iter().all(|m| m.kind == "error"));
    assert_eq!(r.calc.signatures.get("#N/A → #DIV/0!"), Some(&1));
    assert_eq!(r.calc.signatures.get("number → #DIV/0!"), Some(&1));
    assert!(values_match(&Value::Error(CellError::NA), &Value::Error(CellError::NA), 0.0));
    assert!(!values_match(&Value::Error(CellError::NA), &Value::Error(CellError::Value), 0.0));
}
#[test]
fn error_text_is_an_error_only_as_google_sheets_writes_it() {
    for (literal, error) in [
        ("#DIV/0!", CellError::Div0),
        ("#N/A", CellError::NA),
        ("#VALUE!", CellError::Value),
        ("#REF!", CellError::Ref),
        ("#NAME?", CellError::Name),
        ("#NUM!", CellError::Num),
        ("#NULL!", CellError::Null),
        ("#GETTING_DATA", CellError::GettingData),
        ("#CALC!", CellError::Calc),
        ("#SPILL!", CellError::Spill),
    ] {
        let (expected, actual) = (Value::text(literal), Value::Error(error));
        // Text is text: Excel writes an error as `t="e"`.
        assert!(!values_match(&expected, &actual, 0.0), "{literal}");
        assert!(!error_text_cached(ProducerKind::Excel, &expected, &actual), "{literal}");
        assert!(error_text_cached(ProducerKind::GoogleSheets, &expected, &actual), "{literal}");
    }
    assert!(!error_text_cached(ProducerKind::GoogleSheets, &Value::text("#N/A"), &Value::Error(CellError::Value)));
    assert!(!error_text_cached(ProducerKind::GoogleSheets, &Value::text(" #N/A "), &Value::Error(CellError::NA)));
}

/// `wb` saved as a package that `app` (an `<Application>` element, or nothing) names, saved by it.
fn saved_by(wb: &Workbook, app: &str) -> Vec<u8> {
    let bytes = gridcraft_engine::io::save_bytes(wb, "t.xlsx").unwrap();
    let b = patched(&bytes, "xl/workbook.xml", r#"appName="gridcraft""#, r#"appName="xl""#);
    patched(&b, "docProps/app.xml", "<Application>GridCraft</Application>", app)
}

#[test]
fn numbers_compare_within_tolerance() {
    assert!(values_match(&n(3.0000000001), &n(3.0), 1e-9));
    assert!(!values_match(&n(3.0000000001), &n(3.0), 1e-12));
    // Absolute near zero.
    assert!(values_match(&n(1e-12), &n(0.0), 1e-9));
    assert!(values_match(&n(1e12 + 1.0), &n(1e12), 1e-9));
    assert!(!values_match(&n(0.0), &Value::Empty, 1.0));
    assert!(!values_match(&Value::text("1"), &n(1.0), 1.0));
    assert!(values_match(&Value::text("a"), &Value::text("a"), 0.0));
    assert!(!values_match(&Value::text("a"), &Value::text("A"), 0.0));
    assert!(!values_match(&Value::Bool(true), &n(1.0), 1.0));
    let wb = book(&[("A1", "=1+2", n(3.0000000001))]);
    assert_eq!(check(&wb, &Options::default()).calc.matched, 1);
    assert_eq!(check(&wb, &Options { tolerance: 1e-12, ..Default::default() }).calc.mismatched, 1);
}

/// Business Central writes TRUE and FALSE as `<c><f>TRUE()</f><v>1</v></c>`, without `t="b"`:
/// the cache holds the right value under a number type, so it matches and is counted apart.
#[test]
fn booleans_cached_as_numbers_match_and_are_counted_apart() {
    let wb = book(&[
        ("A1", "=TRUE()", n(1.0)),
        ("B1", "=FALSE()", n(0.0)),
        ("C1", "=TRUE()", Value::Bool(true)),
        ("D1", "=1=1", n(0.0)),
        ("E1", "=TRUE()+0", n(1.0)),
    ]);
    let r = check(&wb, &Options::default());
    assert_eq!((r.calc.checked, r.calc.matched, r.calc.mismatched), (5, 4, 1));
    // A cached 0 for TRUE is still a mismatch, and a number result is compared as a number.
    assert_eq!(r.calc.mismatches[0].cell, "Sheet1!D1");
    assert_eq!(r.calc.signatures.get("number → bool"), Some(&1));
    assert_eq!(r.calc.bool_as_number, 2);
    let v = report_json(std::slice::from_ref(&r), &Options::default());
    assert_eq!((v["files"][0]["calc"]["boolAsNumber"].clone(), v["totals"]["boolAsNumber"].clone()), (json!(2), json!(2)));
    assert!(bool_cached_as_number(&n(1.0), &Value::Bool(true)));
    assert!(!bool_cached_as_number(&n(1.0), &Value::Bool(false)));
    assert!(!bool_cached_as_number(&n(2.0), &Value::Bool(true)));
    assert!(!bool_cached_as_number(&Value::Bool(true), &n(1.0)));
}

#[test]
fn missing_cached_values_and_volatile_formulas_are_skipped() {
    let wb =
        book(&[("A1", "=1+1", Value::Empty), ("B1", "=NOW()", n(1.0)), ("B2", "=B1+1", n(2.0)), ("B3", "=CELL(\"filename\")", Value::text("x"))]);
    let r = check(&wb, &Options::default());
    // B2 reads the value NOW() saved, which the check holds it to: it is graded (against an
    // untrusted cache: A1's missing value makes GridCraft's writer ask for a full calculation on load).
    assert_eq!((r.calc.formula_cells, r.calc.untrusted_matched, r.calc.no_oracle, r.calc.volatile), (4, 1, 1, 2));
}

/// A volatile formula that saved a value holds it while the workbook is calculated, so what
/// reads it is graded on the inputs Excel's calculation had.
#[test]
fn readers_of_a_volatile_value_the_file_saved_are_graded() {
    let wb = book(&[
        ("A1", "=RAND()", n(0.25)),
        ("A2", "=A1*4", n(1.0)),         // Excel's answer from the saved draw
        ("A3", "=A2+TODAY()*0", n(1.0)), // volatile itself: not graded
        ("A4", "=A3+1", n(3.0)),         // a wrong cache of a reader: gates
    ]);
    let r = check(&wb, &Options::default());
    assert_eq!((r.calc.formula_cells, r.calc.volatile, r.calc.no_oracle), (4, 2, 0));
    assert_eq!((r.calc.checked, r.calc.matched, r.calc.mismatched), (2, 1, 1));
    assert_eq!(r.calc.mismatches[0].cell, "Sheet1!A4");
    // The formulas are put back, holding the saved values: the round trip sees the file's workbook.
    assert!(r.round_trip.is_clean());
}

#[test]
fn cells_lists_every_formula_cell_with_its_bucket() {
    let wb = book(&[("A1", "=1+1", n(2.0)), ("A2", "=1+2", n(4.0)), ("A3", "=RAND()", n(0.5)), ("A4", "=TRUE()", n(1.0))]);
    let opts = Options { cells: true, ..Default::default() };
    let r = check(&wb, &opts);
    let v = report_json(std::slice::from_ref(&r), &opts);
    assert_eq!(
        v["files"][0]["calc"]["cells"],
        json!({"Sheet1!A1": "matched", "Sheet1!A2": "calc_mismatch", "Sheet1!A3": "volatile", "Sheet1!A4": "matched_alt:boolAsNumber"})
    );
    // Without the flag nothing is collected or printed.
    let plain = check(&wb, &Options::default());
    assert!(plain.calc.cells.is_empty());
    assert!(report_json(&[plain], &Options::default())["files"][0]["calc"].get("cells").is_none());
}

/// Formulas that call something Excel couldn't compute from the file alone have no oracle: the
/// cache holds the answer of the add-in, the user's function or the service, which Excel without
/// it, like GridCraft, answers `#NAME?`. What reads them has none either. Functions Excel does
/// have are graded whether or not GridCraft implements them.
#[test]
fn calls_to_what_excel_lacks_have_no_oracle_and_say_why() {
    let wb = book(&[
        ("A1", "=MYADDIN(2)", n(42.0)),
        ("A2", "=A1+1", n(43.0)),
        ("B1", "=_xll.EURO(1)", n(0.5)),
        ("B2", "=CALL(\"lib\",\"fn\",\"A\",1)", n(7.0)),
        ("B3", "=B1*2", n(2.0)),
        ("C1", "=RTD(\"srv\",\"\",\"topic\")", n(10.0)),
        ("C2", "=CUBEVALUE(\"conn\")", n(11.0)),
        // Excel's own, whatever GridCraft thinks of them.
        ("D1", "=SUM(1,2)", n(3.0)),
        ("D2", "=SUM(1,2)", n(4.0)),
    ]);
    let r = check(&wb, &Options::default());
    let c = &r.calc;
    assert_eq!((c.formula_cells, c.no_oracle, c.checked, c.matched, c.mismatched), (9, 7, 2, 1, 1), "{c:?}");
    let reasons: Vec<_> = c.no_oracle_reasons.iter().map(|(w, n)| (*w, *n)).collect();
    assert_eq!(reasons, [("addin", 1), ("downstream", 2), ("externalData", 2), ("udf", 1), ("xlm", 1)]);
    let mismatched: BTreeSet<&str> = c.mismatches.iter().map(|m| m.cell.as_str()).collect();
    assert_eq!(mismatched, BTreeSet::from(["Sheet1!D2"]));
}

/// What reads a call to what Excel lacks has no oracle when the file saved no value for the call:
/// there is nothing to keep, so it isn't what Excel read either.
#[test]
fn readers_of_a_call_without_a_saved_value_have_no_oracle() {
    let r = check(&book(&[("A3", "=MYADDIN(3)", Value::Empty), ("A4", "=A3+1", n(5.0))]), &Options::default());
    let reasons: Vec<_> = r.calc.no_oracle_reasons.iter().map(|(w, n)| (*w, *n)).collect();
    assert_eq!((r.calc.formula_cells, r.calc.no_oracle), (2, 2), "{:?}", r.calc);
    assert_eq!(reasons, [("downstream", 1), ("noCachedValue", 1)]);
}

/// A formula over a sheet the workbook doesn't have has no oracle: Excel turns the references to
/// a sheet it deletes into `#REF!`, so the cache is from before the package lost the sheet.
#[test]
fn references_to_a_sheet_the_workbook_lacks_have_no_oracle() {
    let wb = book(&[
        ("A1", "=IF(ISNA(VLOOKUP(1,Gone!A1:B2,2,0)),0,1)", n(0.0)),
        ("A2", "=SUM('Also gone'!A1:A3)", n(6.0)),
        ("A3", "=Sheet1!B1+1", n(1.0)),
    ]);
    let r = check(&wb, &Options::default());
    let reasons: Vec<_> = r.calc.no_oracle_reasons.iter().map(|(w, n)| (*w, *n)).collect();
    assert_eq!(reasons, [("missingSheet", 2)], "{:?}", r.calc);
    assert_eq!((r.calc.checked, r.calc.matched), (1, 1), "{:?}", r.calc);
}

/// `_xludf.` before the name of a function Excel has is still graded: the user-defined call is a
/// function the workbook doesn't have (#726), so GridCraft's `#NAME?` against the cached value is a
/// mismatch to look at, not a reclassification.
#[test]
fn user_defined_calls_of_excel_names_are_graded() {
    let r = check(&book(&[("E1", "=_xludf.SUM(1,2)", n(3.0))]), &Options::default());
    assert_eq!((r.calc.formula_cells, r.calc.no_oracle, r.calc.checked, r.calc.mismatched), (1, 0, 1, 1), "{:?}", r.calc);
}

/// `LET` and `LAMBDA` variables and the functions that names define are called like functions;
/// they aren't add-ins.
#[test]
fn functions_the_workbook_defines_are_graded() {
    let mut wb = book(&[("A1", "=Twice(2)", n(4.0)), ("A2", "=LET(f,LAMBDA(x,x+1),f(2))", n(3.0)), ("A3", "=Nowhere(2)", n(2.0))]);
    define(&mut wb, "Twice", None, "LAMBDA(x,x*2)");
    let r = check(&wb, &Options::default());
    assert_eq!((r.calc.checked, r.calc.matched, r.calc.no_oracle), (2, 2, 1), "{:?}", r.calc);
    assert_eq!(r.calc.no_oracle_reasons.get("udf"), Some(&1));
}

/// `bytes` with `from` replaced by `to` in the text part `name`.
fn patched(bytes: &[u8], name: &str, from: &str, to: &str) -> Vec<u8> {
    let mut src = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for i in 0..src.len() {
        let mut f = src.by_index(i).unwrap();
        let entry = f.name().to_string();
        let mut body = String::new();
        f.read_to_string(&mut body).unwrap();
        if entry == name {
            assert!(body.contains(from), "{name} has no `{from}`");
            body = body.replace(from, to);
        }
        out.start_file(entry, zip::write::SimpleFileOptions::default()).unwrap();
        out.write_all(body.as_bytes()).unwrap();
    }
    out.finish().unwrap().into_inner()
}

/// A package whose B1 caches 5 for `=2+2` (and A1 the right 2), saved with `<calcPr {calc_pr}/>`.
fn wrong_cache_with(calc_pr: &str) -> Vec<u8> {
    let wb = book(&[("A1", "=1+1", n(2.0)), ("B1", "=2+2", n(5.0))]);
    let bytes = gridcraft_engine::io::save_bytes(&wb, "t.xlsx").unwrap();
    patched(&bytes, "xl/workbook.xml", r#"<calcPr calcId="191029"/>"#, &format!("<calcPr {calc_pr}/>"))
}

#[test]
fn calc_props_are_read_from_the_workbook_part() {
    assert_eq!(calc_props(&plain()), CalcProps { calc_id: Some(191029), ..Default::default() });
    let all = wrong_cache_with(r#"calcId="0" calcMode="manual" calcCompleted="0" fullCalcOnLoad="true" iterate="1""#);
    assert_eq!(
        calc_props(&all),
        CalcProps { mode: CalcMode::Manual, completed: Some(false), full_calc_on_load: true, iterate: true, calc_id: Some(0) }
    );
    let some = wrong_cache_with(r#"calcMode="autoNoTable" calcCompleted="1""#);
    assert_eq!(calc_props(&some), CalcProps { mode: CalcMode::AutoNoTable, completed: Some(true), ..Default::default() });
    // What the schema doesn't allow reads as absent.
    let odd = wrong_cache_with(r#"calcMode="sometimes" calcCompleted="maybe" calcId="-3""#);
    assert_eq!(calc_props(&odd), CalcProps::default());
    assert_eq!(calc_props(b"not a zip"), CalcProps::default());
    // A package embedded in another file (an `.xls` holding an `.xlsx` as an OLE object) isn't the file's.
    let embedded = gridcraft_xlsx::testing::compound_file(&[("Workbook", b"\x09\x08\x00\x00"), ("Ole10Native", &all)]);
    assert_eq!(calc_props(&embedded), CalcProps::default());
    assert_eq!(producer::detect(&embedded), Producer::default());
}

#[test]
fn cache_trust_follows_what_the_producer_declares() {
    let props = |mode, completed, full_calc_on_load| CalcProps { mode, completed, full_calc_on_load, ..Default::default() };
    let excel = Producer { kind: ProducerKind::Excel, ..Default::default() };
    let of = |p: &CalcProps| CacheTrust::of(p, &excel);
    assert_eq!(of(&CalcProps::default()), CacheTrust::Trusted);
    // Manual calculation alone says nothing about the values being out of date: completed is the default.
    assert_eq!(of(&props(CalcMode::Manual, None, false)), CacheTrust::Trusted);
    assert_eq!(of(&props(CalcMode::Manual, Some(true), false)), CacheTrust::Trusted);
    // The producer's own statement that it did not finish calculating, in any mode.
    assert_eq!(of(&props(CalcMode::Manual, Some(false), false)), CacheTrust::StaleCache);
    assert_eq!(of(&props(CalcMode::Auto, Some(false), false)), CacheTrust::StaleCache);
    assert_eq!(of(&props(CalcMode::Auto, None, true)), CacheTrust::FullCalcOnLoad);
    // Both: the statement that it never finished is the stronger one.
    assert_eq!(of(&props(CalcMode::Manual, Some(false), true)), CacheTrust::StaleCache);
    // A product other than Excel is a reason of its own, weaker than what the producer says about its calculation.
    let calc = ProducerKind::LibreOffice;
    let foreign = Producer { kind: calc, ..Default::default() };
    assert_eq!(CacheTrust::of(&CalcProps::default(), &foreign), CacheTrust::ForeignProducer);
    assert_eq!(CacheTrust::of(&props(CalcMode::Auto, None, true), &foreign), CacheTrust::FullCalcOnLoad);
    assert_eq!(CacheTrust::of(&props(CalcMode::Manual, Some(false), false), &foreign), CacheTrust::StaleCache);
    // Nothing that is not known to be another product argues against the cache: not an unnamed producer, and not GridCraft itself.
    for kind in [ProducerKind::Unknown, ProducerKind::GridCraft, ProducerKind::Excel, ProducerKind::ExcelMac, ProducerKind::ExcelOnline] {
        assert_eq!(CacheTrust::of(&CalcProps::default(), &Producer { kind, ..Default::default() }), CacheTrust::Trusted, "{kind:?}");
    }
}

#[test]
fn a_calculation_that_never_completed_is_not_an_oracle() {
    let stale = check_bytes("t.xlsx", &wrong_cache_with(r#"calcId="0" calcMode="manual" calcCompleted="0""#), &Options::default());
    assert_eq!(stale.calc.cache_trust, CacheTrust::StaleCache);
    // The right cell is compared too, and the wrong one is listed: nothing is dropped.
    assert_eq!((stale.calc.checked, stale.calc.matched, stale.calc.mismatched), (0, 0, 0));
    assert_eq!((stale.calc.untrusted_matched, stale.calc.untrusted_mismatched, stale.calc.no_oracle), (1, 1, 0));
    assert_eq!(stale.calc.untrusted_signatures.get("number → number"), Some(&1));
    assert!(stale.calc.signatures.is_empty() && stale.calc.functions.is_empty());
    let m = &stale.calc.mismatches[0];
    assert_eq!((m.cell.as_str(), m.class, m.primary), ("Sheet1!B1", CacheTrust::StaleCache, true));
    assert!(stale.is_clean());
    // The same cache is a mismatch as soon as nothing in the file argues against it, in automatic
    // mode; manual mode alone doesn't untrust the file, only the cell (see the precedent-local oracle).
    for calc_pr in [r#"calcCompleted="true""#, r#"calcId="191029""#] {
        let r = check_bytes("t.xlsx", &wrong_cache_with(calc_pr), &Options::default());
        assert_eq!(r.calc.cache_trust, CacheTrust::Trusted, "{calc_pr}");
        assert_eq!((r.calc.checked, r.calc.mismatched, r.calc.untrusted_mismatched), (2, 1, 0), "{calc_pr}");
        assert!(!r.is_clean());
    }
    for calc_pr in [r#"calcMode="manual""#, r#"calcMode="manual" calcCompleted="1""#] {
        let r = check_bytes("t.xlsx", &wrong_cache_with(calc_pr), &Options::default());
        assert_eq!(r.calc.cache_trust, CacheTrust::Trusted, "{calc_pr}");
        assert_eq!((r.calc.checked, r.calc.mismatched, r.calc.stale_cell), (2, 0, 1), "{calc_pr}");
    }
}

/// The precedent-local oracle: in manual mode, a cache that an arithmetic formula doesn't give
/// from the file's own cached inputs is a stale cell; one it does give back stays a mismatch (its
/// inputs differ), and so does anything in automatic mode, and any formula whose result is
/// GridCraft's semantics (`SQRT`, or NUMBERVALUE of a text): those reproduce GridCraft's own
/// error on any inputs.
#[test]
fn a_cache_inconsistent_with_its_own_inputs_is_a_stale_cell_in_manual_mode() {
    let wb = book(&[
        ("A1", "=1+1", n(3.0)),            // stale: 2 from no inputs, cached 3
        ("B1", "=A1*10", n(30.0)),         // consistent with the cached A1: inherits A1's difference
        ("C1", "=ABS(-7)+1", n(6.0)),      // stale
        ("D1", "=C1+1", n(7.0)),           // consistent with the cached C1
        ("E1", "=1.0000000028-1", n(0.0)), // a difference rounding could make: not evidence
        ("F1", "=SQRT(4)+0", n(3.0)),      // a function's semantics: not evidence
    ]);
    let bytes = gridcraft_engine::io::save_bytes(&wb, "t.xlsx").unwrap();
    let manual = patched(&bytes, "xl/workbook.xml", r#"<calcPr calcId="191029"/>"#, r#"<calcPr calcId="191029" calcMode="manual"/>"#);
    let r = check_bytes("t.xlsx", &manual, &Options::default());
    assert_eq!(r.calc.cache_trust, CacheTrust::Trusted);
    assert_eq!((r.calc.checked, r.calc.matched, r.calc.mismatched, r.calc.stale_cell), (6, 0, 4, 2));
    let class: Vec<(&str, CacheTrust, bool)> = r.calc.mismatches.iter().map(|m| (m.cell.as_str(), m.class, m.primary)).collect();
    assert_eq!(
        class,
        [
            ("Sheet1!A1", CacheTrust::StaleCell, true),
            ("Sheet1!C1", CacheTrust::StaleCell, true),
            ("Sheet1!E1", CacheTrust::Trusted, true),
            ("Sheet1!F1", CacheTrust::Trusted, true),
            ("Sheet1!B1", CacheTrust::Trusted, false),
            ("Sheet1!D1", CacheTrust::Trusted, false),
        ]
    );
    assert_eq!(r.calc.signatures.get("number → number"), Some(&4));
    assert_eq!(r.calc.untrusted_signatures.get("number → number"), Some(&2));
    assert_eq!(r.calc.functions.get("ABS").map(|s| (s.checked, s.mismatched, s.primary)), Some((1, 0, 0)));
    assert!(!r.is_clean());
    // The same file in automatic mode: every difference gates.
    let auto = check_bytes("t.xlsx", &bytes, &Options::default());
    assert_eq!((auto.calc.mismatched, auto.calc.stale_cell), (6, 0));
}

/// Excel has no subnormal numbers: `1E-307/10` underflows to 0 (Microsoft, "Floating-point
/// arithmetic may give inaccurate results in Excel"; Excel 16.114 gives 0 for
/// `=1E-307/10*1E307*10`, IEEE 754 gives 1). IEEE 754 arithmetic through a subnormal isn't what
/// Excel calculates, so it can't show the cache stale: the mismatch stands, whatever the engine
/// gives. So does one through SUM terms small enough for a partial sum to be subnormal.
#[test]
fn arithmetic_through_a_subnormal_is_no_evidence_of_a_stale_cell() {
    let wb = book(&[
        ("A1", "=1E-307/10*1E307*10", n(5.0)),
        ("B1", "=SUM(3E-308,-2.5E-308,2.3E-308)*1E308", n(5.0)), // normal sum, subnormal partial sum
        ("C1", "=1E-300/10*1E300*10+1", n(5.0)),                 // normal throughout: stale
    ]);
    let bytes = gridcraft_engine::io::save_bytes(&wb, "t.xlsx").unwrap();
    let manual = patched(&bytes, "xl/workbook.xml", r#"<calcPr calcId="191029"/>"#, r#"<calcPr calcId="191029" calcMode="manual"/>"#);
    let r = check_bytes("t.xlsx", &manual, &Options::default());
    let class: Vec<(&str, CacheTrust)> = r.calc.mismatches.iter().map(|m| (m.cell.as_str(), m.class)).collect();
    assert_eq!(class, [("Sheet1!A1", CacheTrust::Trusted), ("Sheet1!B1", CacheTrust::Trusted), ("Sheet1!C1", CacheTrust::StaleCell)]);
    assert_eq!((r.calc.mismatched, r.calc.stale_cell), (2, 1));
}

#[test]
fn full_calc_on_load_is_compared_apart() {
    // A formula without a cached value makes GridCraft's writer flag the file
    // `fullCalcOnLoad`, like libraries that store 0 for every formula: B1's wrong "cached"
    // value is listed, but it isn't a mismatch that gates.
    let wb = book(&[("A1", "=1+1", Value::Empty), ("B1", "=2+2", n(0.0)), ("C1", "=1+2", n(3.0))]);
    let bytes = gridcraft_engine::io::save_bytes(&wb, "t.xlsx").unwrap();
    assert!(calc_props(&bytes).full_calc_on_load);
    let r = check_bytes("t.xlsx", &bytes, &Options::default());
    assert_eq!(r.calc.cache_trust, CacheTrust::FullCalcOnLoad);
    assert_eq!((r.calc.checked, r.calc.no_oracle, r.calc.mismatched), (0, 1, 0));
    assert_eq!((r.calc.untrusted_matched, r.calc.untrusted_mismatched), (1, 1));
    assert_eq!(r.calc.mismatches.iter().map(|m| (m.cell.as_str(), m.class)).collect::<Vec<_>>(), [("Sheet1!B1", CacheTrust::FullCalcOnLoad)]);
    assert!(r.is_clean());
    let fresh = gridcraft_engine::io::save_bytes(&book(&[("B1", "=2+2", n(0.0))]), "t.xlsx").unwrap();
    assert!(!calc_props(&fresh).full_calc_on_load);
    assert_eq!(check_bytes("t.xlsx", &fresh, &Options::default()).calc.mismatched, 1);
}

#[test]
fn json_reports_the_untrusted_bucket() {
    let stale = check_bytes("s.xlsx", &wrong_cache_with(r#"calcId="0" calcMode="manual" calcCompleted="0""#), &Options::default());
    let trusted = check_bytes("t.xlsx", &wrong_cache_with(r#"calcId="191029""#), &Options::default());
    let v = report_json(&[stale, trusted], &Options::default());
    let c = &v["files"][0]["calc"];
    assert_eq!(c["cacheTrust"], "staleCache");
    assert_eq!(c["staleCache"], true);
    assert_eq!(c["calcPr"], json!({"mode": "manual", "calcCompleted": false, "fullCalcOnLoad": false, "iterate": false, "calcId": 0}));
    assert_eq!((c["mismatched"].as_u64(), c["untrustedMismatched"].as_u64(), c["untrustedMatched"].as_u64()), (Some(0), Some(1), Some(1)));
    assert_eq!(c["untrustedSignatures"], json!({"number → number": 1}));
    assert_eq!(c["mismatches"][0]["class"], "staleCache");
    let t = &v["files"][1]["calc"];
    assert_eq!(
        (t["cacheTrust"].as_str(), t["staleCache"].as_bool(), t["mismatches"][0]["class"].as_str()),
        (Some("trusted"), Some(false), Some("trusted"))
    );
    assert_eq!(t["calcPr"]["calcCompleted"], Json::Null);
    assert_eq!(v["totals"]["mismatched"], 1);
    assert_eq!((v["totals"]["untrustedMismatched"].as_u64(), v["totals"]["untrustedMatched"].as_u64()), (Some(1), Some(1)));
    assert_eq!(v["totals"]["untrusted"], json!({"staleCache": {"files": 1, "matched": 1, "mismatched": 1}}));
}

/// `wrong_cache_with` (B1 caches 5 for `=2+2`), with the extended properties' `<Application>`
/// element replaced by `app`, saved by it (`<fileVersion appName="xl">`, as Excel and the products
/// that pass for it write).
fn wrong_cache_by(app: &str) -> Vec<u8> {
    let b = patched(&wrong_cache_with(r#"calcId="191029""#), "xl/workbook.xml", r#"appName="gridcraft""#, r#"appName="xl""#);
    patched(&b, "docProps/app.xml", "<Application>GridCraft</Application>", app)
}

/// The namespaces a Google Sheets export declares on the workbook root.
const GOOGLE_NAMESPACES: &str = r#"xmlns:mv="urn:schemas-microsoft-com:mac:vml" xmlns:xm="http://schemas.microsoft.com/office/excel/2006/main""#;

#[test]
fn the_producer_is_the_application_the_package_names() {
    for (app, kind) in [
        ("Microsoft Excel", ProducerKind::Excel),
        ("Microsoft Macintosh Excel", ProducerKind::ExcelMac),
        ("Microsoft Excel Online", ProducerKind::ExcelOnline),
        ("Excel Android", ProducerKind::Excel),
        ("LibreOffice/7.3.7.2$Linux_X86_64 LibreOffice_project/30$Build-2", ProducerKind::LibreOffice),
        ("LibreOfficeDev/25.2.0.0.alpha0$Linux_X86_64", ProducerKind::LibreOffice),
        ("Collabora_Office/25.04.6.1$Linux_X86_64", ProducerKind::LibreOffice),
        ("WPS 表格", ProducerKind::Wps),
        ("WPS Spreadsheets", ProducerKind::Wps),
        ("ONLYOFFICE/8.1.0.169", ProducerKind::OnlyOffice),
        ("Apache POI", ProducerKind::Poi),
        ("GridCraft", ProducerKind::GridCraft),
        // Libraries that borrow Excel's name say so after it; Access is a product of its own.
        ("Microsoft Excel Compatible / Openpyxl 3.1.2", ProducerKind::Other),
        ("Microsoft Access", ProducerKind::Other),
        ("  ", ProducerKind::Unknown),
    ] {
        assert_eq!(producer::kind_of_application(app), kind, "{app}");
    }
    assert!(ProducerKind::ExcelMac.is_excel() && !ProducerKind::LibreOffice.is_excel());
    // Only a package that names another product is foreign: not one that names none, nor GridCraft's own.
    let foreign: Vec<ProducerKind> = [
        ProducerKind::Excel,
        ProducerKind::ExcelMac,
        ProducerKind::ExcelOnline,
        ProducerKind::LibreOffice,
        ProducerKind::Wps,
        ProducerKind::OnlyOffice,
        ProducerKind::GoogleSheets,
        ProducerKind::Poi,
        ProducerKind::GridCraft,
        ProducerKind::Other,
        ProducerKind::Unknown,
    ]
    .into_iter()
    .filter(|k| k.is_foreign())
    .collect();
    assert_eq!(
        foreign,
        [ProducerKind::LibreOffice, ProducerKind::Wps, ProducerKind::OnlyOffice, ProducerKind::GoogleSheets, ProducerKind::Poi, ProducerKind::Other]
    );
}

#[test]
fn the_producer_is_read_from_the_package() {
    let mac = producer::detect(&wrong_cache_by("<Application>Microsoft Macintosh Excel</Application><AppVersion>16.0300</AppVersion>"));
    assert_eq!(
        (mac.application.as_deref(), mac.version.as_deref(), mac.kind),
        (Some("Microsoft Macintosh Excel"), Some("16.0300"), ProducerKind::ExcelMac)
    );
    assert_eq!(producer::detect(&plain()).kind, ProducerKind::GridCraft);
    // The extended properties part is the one the package root relates, whatever its name or spelling of the relationship.
    for kind in ["extended-properties", "http://purl.oclc.org/ooxml/officeDocument/relationships/extendedProperties"] {
        let pkg = zip_owned(&[
            part("_rels/.rels", rels_part(&[("rId1", "officeDocument", "xl/workbook.xml"), ("rId2", kind, "meta/props.xml")])),
            part("xl/workbook.xml", "<workbook><sheets/></workbook>"),
            part("meta/props.xml", "<Properties><Application>LibreOffice/7.3</Application><AppVersion>7.3</AppVersion></Properties>"),
        ]);
        let p = producer::detect(&pkg);
        assert_eq!(
            (p.application.as_deref(), p.version.as_deref(), p.kind),
            (Some("LibreOffice/7.3"), Some("7.3"), ProducerKind::LibreOffice),
            "{kind}"
        );
    }
    // No properties at all: Google Sheets exports are known by what they declare on the workbook root.
    let export = |root: &str| {
        zip_owned(&[
            part("_rels/.rels", rels_part(&[("rId1", "officeDocument", "xl/workbook.xml")])),
            part("xl/workbook.xml", format!("{root}<sheets/></workbook>")),
        ])
    };
    let google_root = format!(r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" {GOOGLE_NAMESPACES}>"#);
    assert_eq!(producer::detect(&export(&google_root)), Producer { application: None, version: None, kind: ProducerKind::GoogleSheets });
    // The Mac Excel namespace alone (Mac Excel declares it) is no signature.
    let mac_root = r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:mx="http://schemas.microsoft.com/office/mac/excel/2008/main">"#;
    assert_eq!(producer::detect(&export(mac_root)).kind, ProducerKind::Unknown);
    assert_eq!(producer::detect(&export("<workbook>")).kind, ProducerKind::Unknown);
    // An application that is named wins over the signature.
    let named = patched(
        &wrong_cache_by("<Application>Microsoft Excel</Application>"),
        "xl/workbook.xml",
        "<workbook ",
        &format!("<workbook {GOOGLE_NAMESPACES} "),
    );
    assert_eq!(producer::detect(&named).kind, ProducerKind::Excel);
    assert_eq!(producer::detect(b"not a zip"), Producer::default());
}

/// Excel never writes `xml:space` on the workbook root; a library that names its packages
/// `Microsoft Excel` does, and its cache is its own. `<fileVersion appName>` names the application
/// that last saved the workbook (ECMA-376 §18.2.10): Excel's is `xl`.
#[test]
fn an_excel_named_package_another_writer_saved_is_that_writer_s() {
    let excel = wrong_cache_by("<Application>Microsoft Excel</Application>");
    assert_eq!(producer::detect(&excel).kind, ProducerKind::Excel);
    let library = patched(&excel, "xl/workbook.xml", "<workbook ", r#"<workbook xml:space="preserve" "#);
    let p = producer::detect(&library);
    assert_eq!((p.application.as_deref(), p.kind), (Some("Microsoft Excel"), ProducerKind::Other));
    let r = check_bytes("t.xlsx", &library, &Options::default());
    assert_eq!((r.calc.cache_trust, r.calc.mismatched), (CacheTrust::ForeignProducer, 0));
    let saved_by = |app: &str| {
        let b = patched(&excel, "xl/workbook.xml", r#"appName="xl""#, &format!(r#"appName="{app}""#));
        producer::detect(&b).kind
    };
    assert_eq!(saved_by("xl"), ProducerKind::Excel);
    assert_eq!(saved_by("Calc"), ProducerKind::LibreOffice);
    assert_eq!(saved_by("SpreadsheetGear 8.7.17.102"), ProducerKind::Other);
    // A package that names no application is known by it too.
    let unnamed = patched(&wrong_cache_by(""), "xl/workbook.xml", r#"appName="xl""#, r#"appName="Calc""#);
    assert_eq!(producer::detect(&unnamed).kind, ProducerKind::LibreOffice);
}

/// OpenDocument files name their producer in `<meta:generator>`: the cached values of a
/// LibreOffice document are LibreOffice's results.
#[test]
fn an_opendocument_producer_is_its_generator() {
    let meta = r#"<office:document-meta xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:meta="urn:oasis:names:tc:opendocument:xmlns:meta:1.0"><office:meta><meta:generator>LibreOffice/25.2.7.2$Linux_X86_64 LibreOffice_project/520$Build-2</meta:generator></office:meta></office:document-meta>"#;
    let ods =
        zip_of(&[("mimetype", "application/vnd.oasis.opendocument.spreadsheet"), ("meta.xml", meta), ("content.xml", "<office:document-content/>")]);
    let p = producer::detect(&ods);
    assert_eq!(
        (p.application.as_deref(), p.kind),
        (Some("LibreOffice/25.2.7.2$Linux_X86_64 LibreOffice_project/520$Build-2"), ProducerKind::LibreOffice)
    );
    assert!(p.kind.is_foreign());
    // A flat document holds its metadata itself.
    let fods = r#"<?xml version="1.0"?><office:document xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:meta="urn:oasis:names:tc:opendocument:xmlns:meta:1.0"><office:meta><meta:generator>OpenOffice.org/3.2$Win32</meta:generator></office:meta></office:document>"#;
    assert_eq!(producer::detect(fods.as_bytes()).kind, ProducerKind::Other);
    // Neither says nothing: an XLS (not XML, not a zip) is unknown.
    assert_eq!(producer::detect(b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1").kind, ProducerKind::Unknown);
}

/// A document that names no generator is LibreOffice's when its root declares LibreOffice's
/// extension namespaces; one that declares only ODF's isn't Excel's (Excel's export names itself
/// in `<meta:generator>`), so its cache is another product's.
#[test]
fn an_opendocument_without_generator_is_known_by_its_libreoffice_namespaces() {
    let calcext = r#"xmlns:calcext="urn:org:documentfoundation:names:experimental:calc:xmlns:calcext:1.0""#;
    let flat = |extra: &str| {
        format!(
            r#"<?xml version="1.0"?><office:document xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" {extra}><office:body/></office:document>"#
        )
    };
    let p = producer::detect(flat(calcext).as_bytes());
    assert_eq!((p.application, p.kind), (None, ProducerKind::LibreOffice));
    let bare = producer::detect(flat("").as_bytes());
    assert_eq!((bare.application, bare.kind), (None, ProducerKind::Other));
    assert!(bare.kind.is_foreign());
    // XML that isn't an OpenDocument document says nothing.
    assert_eq!(producer::detect(b"<html><body/></html>").kind, ProducerKind::Unknown);
    let content = format!(r#"<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" {calcext}/>"#);
    let ods = zip_of(&[("mimetype", "application/vnd.oasis.opendocument.spreadsheet"), ("content.xml", content.as_str())]);
    assert_eq!(producer::detect(&ods).kind, ProducerKind::LibreOffice);
}

/// WPS Office saves as `Microsoft Excel` and tags the workbook with its build in a custom
/// property, which an Excel save keeps along with WPS's values (Excel doesn't recalculate them).
#[test]
fn a_workbook_wps_saved_is_wps() {
    let custom = r#"<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/custom-properties" xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes"><property fmtid="{D5CDD505-2E9C-101B-9397-08002B2CF9AE}" pid="2" name="KSOProductBuildVer"><vt:lpwstr>2052-12.1.0.16729</vt:lpwstr></property></Properties>"#;
    let by = |app: &str| {
        zip_owned(&[
            part("_rels/.rels", rels_part(&[("rId1", "officeDocument", "xl/workbook.xml"), ("rId2", "extended-properties", "docProps/app.xml")])),
            part("xl/workbook.xml", "<workbook><sheets/></workbook>"),
            part("docProps/app.xml", format!("<Properties>{app}</Properties>")),
            part("docProps/custom.xml", custom),
        ])
    };
    let wps = producer::detect(&by("<Application>Microsoft Excel</Application>"));
    assert_eq!((wps.kind, wps.version.as_deref()), (ProducerKind::Wps, Some("2052-12.1.0.16729")));
    assert!(wps.kind.is_foreign());
    let excel_saved = producer::detect(&by("<Application>Microsoft Excel</Application><AppVersion>16.0300</AppVersion>"));
    assert_eq!((excel_saved.kind, excel_saved.version.as_deref()), (ProducerKind::Wps, Some("16.0300")));
    let plain_excel = zip_owned(&[
        part("_rels/.rels", rels_part(&[("rId1", "officeDocument", "xl/workbook.xml"), ("rId2", "extended-properties", "docProps/app.xml")])),
        part("xl/workbook.xml", "<workbook><sheets/></workbook>"),
        part("docProps/app.xml", "<Properties><Application>Microsoft Excel</Application></Properties>"),
    ]);
    assert_eq!(producer::detect(&plain_excel).kind, ProducerKind::Excel);
}

/// An XLSB package's binary parts have no counterpart in the XLSX written from it: its
/// relationships aren't compared with that XLSX's, the cells are.
#[test]
fn the_relationships_of_an_xlsb_are_not_compared_with_its_xlsx() {
    let xlsb = zip_owned(&[
        part("_rels/.rels", rels_part(&[("rId1", "officeDocument", "xl/workbook.bin"), ("rId2", "madeUp", "docProps/madeUp.xml")])),
        part("xl/workbook.bin", "\u{83}\u{1}\u{0}binary"),
        part("docProps/madeUp.xml", "<x><y/></x>"),
    ]);
    let rt = round_trip(&book(&[("A1", "1", n(1.0))]), &xlsb, &Options::default());
    assert!(rt.is_clean(), "{:?}", rt.differing());
}

#[test]
fn a_cache_written_by_another_product_is_compared_apart() {
    let by = |app: &str| check_bytes("t.xlsx", &wrong_cache_by(&format!("<Application>{app}</Application>")), &Options::default());
    let libre = by("LibreOffice/7.3.7.2$Linux_X86_64");
    assert_eq!(libre.producer.kind, ProducerKind::LibreOffice);
    assert_eq!(libre.calc.cache_trust, CacheTrust::ForeignProducer);
    // Compared, listed and counted, but not a mismatch that gates.
    assert_eq!((libre.calc.checked, libre.calc.mismatched), (0, 0));
    assert_eq!((libre.calc.untrusted_matched, libre.calc.untrusted_mismatched), (1, 1));
    assert_eq!(libre.calc.untrusted_signatures.get("number → number"), Some(&1));
    assert_eq!(libre.calc.mismatches[0].class, CacheTrust::ForeignProducer);
    assert!(libre.is_clean());
    // The same cache from Excel is a mismatch.
    let excel = by("Microsoft Excel");
    assert_eq!((excel.calc.cache_trust, excel.calc.checked, excel.calc.mismatched), (CacheTrust::Trusted, 2, 1));
    // A package that names no producer is not excused.
    let unnamed = check_bytes("t.xlsx", &wrong_cache_by(""), &Options::default());
    assert_eq!((unnamed.producer.kind, unnamed.calc.cache_trust, unnamed.calc.mismatched), (ProducerKind::Unknown, CacheTrust::Trusted, 1));
    // Google Sheets: no application, the export's namespaces on the workbook root.
    let google_pkg = patched(&wrong_cache_by(""), "xl/workbook.xml", "<workbook ", &format!("<workbook {GOOGLE_NAMESPACES} "));
    let google = check_bytes("g.xlsx", &google_pkg, &Options::default());
    assert_eq!((google.producer.kind, google.calc.cache_trust, google.calc.mismatched), (ProducerKind::GoogleSheets, CacheTrust::ForeignProducer, 0));

    let v = report_json(&[libre, excel, unnamed], &Options::default());
    assert_eq!(v["files"][0]["producer"], json!({"application": "LibreOffice/7.3.7.2$Linux_X86_64", "version": null, "kind": "libreOffice"}));
    assert_eq!(v["files"][0]["calc"]["cacheTrust"], "foreignProducer");
    assert_eq!(v["files"][0]["calc"]["mismatches"][0]["class"], "foreignProducer");
    assert_eq!(v["files"][2]["producer"], json!({"application": null, "version": null, "kind": "unknown"}));
    assert_eq!(v["totals"]["producers"], json!({"excel": 1, "libreOffice": 1, "unknown": 1}));
    assert_eq!(v["totals"]["untrusted"], json!({"foreignProducer": {"files": 1, "matched": 1, "mismatched": 1}}));
    assert_eq!(v["totals"]["mismatched"], 2);
}

#[test]
fn a_cache_computed_under_another_locale_matches_under_it() {
    // 45306 is 2024-01-15. The cached month is Spanish; a wrong month stays a mismatch.
    let wb = book(&[
        ("A1", "45306", n(45306.0)),
        ("B1", "=TEXT(A1,\"mmmm\")", Value::text("enero")),
        ("C1", "=TEXT(A1,\"mmmm\")", Value::text("febrero")),
        ("D1", "=A1+1", n(45307.0)),
    ]);
    let r = check(&wb, &Options::default());
    assert_eq!((r.calc.checked, r.calc.matched, r.calc.mismatched, r.calc.locale_dependent, r.calc.locale), (3, 2, 1, 1, Some("es-ES")));
    assert_eq!(r.calc.mismatches.iter().map(|m| m.cell.as_str()).collect::<Vec<_>>(), ["Sheet1!C1"]);
    let v = report_json(&[r], &Options::default());
    assert_eq!((v["files"][0]["calc"]["localeDependent"].as_u64(), v["totals"]["localeDependent"].as_u64()), (Some(1), Some(1)));
    assert_eq!(v["files"][0]["calc"]["locale"], "es-ES");

    // A D/M/Y text date: en-GB is the first locale that reads it as the cache did (1 September).
    let wb = book(&[("A2", "01/09/2020", Value::text("01/09/2020")), ("B2", "=A2+0", n(44075.0)), ("C2", "=LEN(A2)", n(10.0))]);
    let r = check(&wb, &Options::default());
    assert_eq!((r.calc.mismatched, r.calc.locale_dependent, r.calc.locale), (0, 1, Some("en-GB")));
    assert!(r.is_clean());

    // A cache that agrees with en-US somewhere a locale disagrees is not that locale's.
    let wb = book(&[
        ("A1", "45306", n(45306.0)),
        ("B1", "=TEXT(A1,\"mmmm\")", Value::text("enero")),
        ("D1", "=TEXT(A1,\"mmmm\")", Value::text("January")),
    ]);
    let r = check(&wb, &Options::default());
    assert_eq!((r.calc.mismatched, r.calc.locale_dependent, r.calc.locale), (1, 0, None));
    assert_eq!(report_json(&[r], &Options::default())["files"][0]["calc"]["locale"], Json::Null);
}

fn define(wb: &mut Workbook, name: &str, scope: Option<usize>, formula: &str) {
    wb.names.push(gridcraft_engine::model::DefinedName { name: name.into(), scope, formula: formula.into(), comment: String::new(), hidden: false });
}

/// How many formulas of the checked book are volatile, and how many of those only through names.
fn volatile_of(wb: &Workbook) -> (usize, usize) {
    let r = check(wb, &Options::default());
    (r.calc.volatile, r.calc.volatile_via_names)
}

#[test]
fn volatility_reaches_the_formulas_that_use_a_volatile_name() {
    let mut wb = book(&[("A1", "=Stamp+1", n(1.0))]);
    define(&mut wb, "Stamp", None, "TODAY()");
    assert_eq!(volatile_of(&wb), (1, 1));
    // A name that uses a volatile name is volatile too.
    let mut wb = book(&[("A1", "=Outer*2", n(2.0))]);
    define(&mut wb, "Stamp", None, "NOW()");
    define(&mut wb, "Outer", None, "Stamp+1");
    assert_eq!(volatile_of(&wb), (1, 1));
    // One that doesn't read anything volatile is not.
    let mut wb = book(&[("A1", "=Rate*2", n(1.0))]);
    define(&mut wb, "Rate", None, "0.5");
    let r = check(&wb, &Options::default());
    assert_eq!((r.calc.volatile, r.calc.checked, r.calc.matched), (0, 1, 1));
}

#[test]
fn volatility_reaches_the_formulas_that_read_a_volatile_cell_through_a_name() {
    // A name for the cell that holds TODAY(), as workbooks name their "as of" date: the formula
    // reading the name is volatile, and so is what reads that formula. (TODAY() saved no value, so
    // it can't hold one: see `readers_of_a_volatile_value_the_file_saved_are_graded`.)
    let mut wb =
        book(&[("B1", "=TODAY()", Value::Empty), ("A1", "=AsOf+1", n(2.0)), ("A2", "=A1*2", n(4.0)), ("C1", "7", n(7.0)), ("C2", "=C1+1", n(8.0))]);
    define(&mut wb, "AsOf", None, "Sheet1!$B$1");
    assert_eq!(volatile_of(&wb), (3, 2));
    // A range that holds the volatile cell taints the name, and a name using that name too.
    let mut wb = book(&[
        ("B2", "=RAND()", Value::Empty),
        ("A1", "=SUM(Draws)", n(1.0)),
        ("A2", "=Total+1", n(2.0)),
        ("A3", "=Other", n(3.0)),
        ("C1", "5", n(5.0)),
    ]);
    define(&mut wb, "Draws", None, "Sheet1!$B$1:$B$3");
    define(&mut wb, "Total", None, "SUM(Draws)");
    define(&mut wb, "Other", None, "Sheet1!$C$1");
    assert_eq!(volatile_of(&wb), (3, 2));
}

#[test]
fn a_sheet_scoped_name_hides_the_workbook_one_of_the_same_name() {
    let mut wb = book(&[("A1", "=Stamp", n(1.0))]);
    wb.sheets.push(Arc::new(Sheet::new("Second")));
    wb.sheet_mut(1).unwrap().set_cell(at("A1"), Cell { value: n(1.0), formula: Some(Arc::new(Formula::new("=Stamp"))), ..Default::default() });
    define(&mut wb, "Stamp", None, "TODAY()");
    define(&mut wb, "Stamp", Some(0), "1");
    // Only the second sheet reads the workbook-scoped, volatile one.
    let r = check(&wb, &Options::default());
    assert_eq!((r.calc.formula_cells, r.calc.volatile, r.calc.checked, r.calc.matched), (2, 1, 1, 1));
}

#[test]
fn a_function_defined_by_a_name_is_volatile_when_its_body_is() {
    let mut wb = book(&[("A1", "=Twice(2)", n(4.0)), ("A2", "=Pure(2)", n(4.0))]);
    define(&mut wb, "Twice", None, "LAMBDA(x,x*2*SIGN(NOW()))");
    define(&mut wb, "Pure", None, "LAMBDA(x,x*2)");
    assert_eq!(volatile_of(&wb), (1, 1));
}

#[test]
fn a_mismatch_that_reads_a_mismatch_through_a_name_is_not_primary() {
    let mut wb = book(&[("C1", "=1+1", n(5.0)), ("A1", "=Base*2", n(99.0)), ("A2", "=1+1", n(7.0))]);
    define(&mut wb, "Base", None, "Sheet1!$C$1");
    let r = check(&wb, &Options::default());
    assert_eq!(r.calc.mismatched, 3);
    let mut primary: Vec<&str> = r.calc.mismatches.iter().filter(|m| m.primary).map(|m| m.cell.as_str()).collect();
    primary.sort();
    assert_eq!(primary, ["Sheet1!A2", "Sheet1!C1"]);
}

/// Google Sheets saves an error as the text `#CODE` or `#CODE:reason:detail`; Excel shows the
/// code. Any other producer's text is a text result, whatever it spells.
#[test]
fn errors_cached_as_text_match_by_their_code_only_from_google_sheets() {
    let wb = book(&[
        ("A1", "=NA()", Value::text("#N/A:lookupNotFound:Toxic")),
        ("A2", "=1/0", Value::text("#DIV/0!:divZero")),
        ("A3", "=NA()", Value::text("#N/A")),
        ("A4", "=NA()", Value::text("#VALUE!:notNumber:x")),
        ("A5", "=NA()", Value::text("N/A: not an error")),
    ]);
    let google_pkg = patched(&saved_by(&wb, ""), "xl/workbook.xml", "<workbook ", &format!("<workbook {GOOGLE_NAMESPACES} "));
    let google = check_bytes("g.xlsx", &google_pkg, &Options::default());
    assert_eq!(google.producer.kind, ProducerKind::GoogleSheets);
    assert_eq!((google.calc.untrusted_matched, google.calc.untrusted_mismatched, google.calc.error_detail_cache), (3, 2, 3), "{google:?}");
    let mut wrong: Vec<&str> = google.calc.mismatches.iter().map(|m| m.cell.as_str()).collect();
    wrong.sort();
    assert_eq!(wrong, ["Sheet1!A4", "Sheet1!A5"]);
    // Excel writes an error as `t="e"`: text that spells one (`=IF(…,"#DIV/0!:literal",1/0)`) is text.
    let excel = check_bytes("t.xlsx", &saved_by(&wb, "<Application>Microsoft Excel</Application>"), &Options::default());
    assert_eq!((excel.calc.checked, excel.calc.matched, excel.calc.mismatched, excel.calc.error_detail_cache), (5, 0, 5, 0), "{excel:?}");
    assert!(!excel.is_clean());
}

/// A writer other than Excel that doesn't write `xml:space="preserve"` can drop the whitespace
/// around a cached text; Excel writes the attribute (Excel 16.114 saves `="x "&""` as
/// `<v xml:space="preserve">x </v>`), so from Excel a cache without the whitespace differs.
#[test]
fn surrounding_whitespace_a_foreign_writer_dropped_is_lost_space() {
    let wb = book(&[
        ("A1", "=\"keep\"&\" \"", Value::text("keep")),
        ("A2", "=\" \"&\"lead\"", Value::text("lead")),
        ("A3", "=\" \"", Value::text("")),
        ("A4", "=\"in side\"", Value::text("in  side")),
        ("A5", "=\"x\"", Value::text("x ")),
    ]);
    let libre = saved_by(&wb, "<Application>LibreOffice/7.3.7.2$Linux_X86_64</Application>");
    let r = check_bytes("t.xlsx", &libre, &Options::default());
    assert_eq!(r.producer.kind, ProducerKind::LibreOffice);
    assert_eq!((r.calc.untrusted_matched, r.calc.untrusted_mismatched, r.calc.whitespace_cache), (3, 2, 3), "{r:?}");
    let mut wrong: Vec<&str> = r.calc.mismatches.iter().map(|m| m.cell.as_str()).collect();
    wrong.sort();
    // A5's cache has whitespace the result lacks: nothing was dropped.
    assert_eq!(wrong, ["Sheet1!A4", "Sheet1!A5"]);
    // The same cache with the attribute says the whitespace is part of the value.
    let preserved = patched(&libre, "xl/worksheets/sheet1.xml", "<v>keep</v>", "<v xml:space=\"preserve\">keep</v>");
    let r = check_bytes("t.xlsx", &preserved, &Options::default());
    assert_eq!((r.calc.untrusted_matched, r.calc.untrusted_mismatched, r.calc.whitespace_cache), (2, 3, 2), "{r:?}");
    // From Excel, every difference counts.
    let excel = check_bytes("t.xlsx", &saved_by(&wb, "<Application>Microsoft Excel</Application>"), &Options::default());
    assert_eq!((excel.calc.checked, excel.calc.matched, excel.calc.mismatched, excel.calc.whitespace_cache), (5, 0, 5, 0), "{excel:?}");
}

/// A package as Excel writes `#CALC!` and `#SPILL!`: the cell caches `#VALUE!` (`t="e"`) and `vm`
/// leads to the true error: `valueMetadata` → `futureMetadata` → `rdrichvalue.xml` → the `_error`
/// structure. The chain is deliberately not in cell order: A1 (`vm=1`) reaches the second rich
/// value, A3 (`vm=2`) the first. A6 is A1's formula with a plain `#VALUE!` and no `vm`.
fn rich_error_package() -> Vec<u8> {
    const MAIN: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
    let ext = |i: u32| format!(r#"<bk><extLst><ext uri="{{3e2802c4-a4d2-4d8b-9148-e3be6c30e623}}"><xlrd:rvb i="{i}"/></ext></extLst></bk>"#);
    let metadata = format!(
        r#"<metadata xmlns="{MAIN}" xmlns:xlrd="http://schemas.microsoft.com/office/spreadsheetml/2017/richdata" xmlns:xda="http://schemas.microsoft.com/office/spreadsheetml/2017/dynamicarray"><metadataTypes count="2"><metadataType name="XLDAPR" minSupportedVersion="120000" cellMeta="1"/><metadataType name="XLRICHVALUE" minSupportedVersion="120000"/></metadataTypes><futureMetadata name="XLDAPR" count="1"><bk><extLst><ext uri="{{bdbb8cdc-fa1e-496e-a857-3c3f30c029c3}}"><xda:dynamicArrayProperties fDynamic="1" fCollapsed="0"/></ext></extLst></bk></futureMetadata><futureMetadata name="XLRICHVALUE" count="2">{}{}</futureMetadata><cellMetadata count="1"><bk><rc t="1" v="0"/></bk></cellMetadata><valueMetadata count="2"><bk><rc t="2" v="0"/></bk><bk><rc t="2" v="1"/></bk></valueMetadata></metadata>"#,
        ext(1),
        ext(0)
    );
    let rd = "http://schemas.microsoft.com/office/spreadsheetml/2017/richdata";
    let structures = format!(
        r#"<rvStructures xmlns="{rd}" count="2"><s t="_error"><k n="errorType" t="i"/><k n="subType" t="i"/></s><s t="_error"><k n="colOffset" t="i"/><k n="errorType" t="i"/><k n="rwOffset" t="i"/><k n="subType" t="i"/></s></rvStructures>"#
    );
    // #SPILL! (errorType 8, with offsets) then #CALC! (13).
    let values =
        format!(r#"<rvData xmlns="{rd}" count="2"><rv s="1"><v>2</v><v>8</v><v>0</v><v>1</v></rv><rv s="0"><v>13</v><v>3</v></rv></rvData>"#);
    let calc = "_xlfn._xlws.FILTER(B1:B2,B1:B2&gt;10)";
    let sheet = format!(
        r#"<worksheet xmlns="{MAIN}"><sheetData><row r="1"><c r="A1" t="e" cm="1" vm="1"><f t="array" ref="A1">{calc}</f><v>#VALUE!</v></c><c r="B1"><v>1</v></c></row><row r="2"><c r="B2"><v>2</v></c></row><row r="3"><c r="A3" t="e" cm="1" vm="2"><f t="array" ref="A3">_xlfn.SEQUENCE(2)</f><v>#VALUE!</v></c></row><row r="4"><c r="A4"><v>5</v></c></row><row r="6"><c r="A6" t="e" cm="1"><f t="array" ref="A6">{calc}</f><v>#VALUE!</v></c></row></sheetData></worksheet>"#
    );
    let office = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
    let rich = "http://schemas.microsoft.com/office/2017/06/relationships";
    zip_of(&[
        (
            "[Content_Types].xml",
            r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/xl/metadata.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheetMetadata+xml"/></Types>"#,
        ),
        (
            "_rels/.rels",
            &format!(
                r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{office}/officeDocument" Target="xl/workbook.xml"/></Relationships>"#
            ),
        ),
        (
            "xl/workbook.xml",
            &format!(r#"<workbook xmlns="{MAIN}" xmlns:r="{office}"><sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets></workbook>"#),
        ),
        (
            "xl/_rels/workbook.xml.rels",
            &format!(
                r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{office}/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="{office}/sheetMetadata" Target="metadata.xml"/><Relationship Id="rId3" Type="{rich}/rdRichValue" Target="richData/rdrichvalue.xml"/><Relationship Id="rId4" Type="{rich}/rdRichValueStructure" Target="richData/rdrichvaluestructure.xml"/></Relationships>"#
            ),
        ),
        ("xl/worksheets/sheet1.xml", &sheet),
        ("xl/metadata.xml", &metadata),
        ("xl/richData/rdrichvaluestructure.xml", &structures),
        ("xl/richData/rdrichvalue.xml", &values),
    ])
}

/// Excel has no cell-error code for `#CALC!` and `#SPILL!`: it writes `#VALUE!` and gives the
/// true error in a rich value, which the importer reads as the cell's cached error. Such a cache
/// compares plainly as that error; without the rich value it is a `#VALUE!` that GridCraft doesn't
/// compute.
#[test]
fn errors_cached_as_value_with_a_rich_value_match_by_the_true_error() {
    let r = check_bytes("t.xlsx", &rich_error_package(), &Options::default());
    assert_eq!(r.error, None);
    let c = &r.calc;
    assert_eq!((c.formula_cells, c.checked, c.matched, c.mismatched), (3, 3, 2, 1), "{c:?}");
    assert_eq!(c.mismatches.iter().map(|m| m.cell.as_str()).collect::<Vec<_>>(), ["Sheet1!A6"]);
    assert_eq!(c.signatures.get("#VALUE! → #CALC!"), Some(&1));
}

/// Excel documents the accuracy of its iterative functions (IRR, RATE… stop at 0.00001 percent);
/// results that agree to 1e-7 relative are the same answer, and so are the formulas built on them
/// as far as that accuracy carries: `B1*2` within twice it, and `IF(B1>0,B1,0)`, which gives its
/// cache back from the cached B1. A solver whose result the formula multiplies by zero carries
/// nothing: `0*IRR(…)+1/3` is graded as `1/3`.
#[test]
fn iterative_solver_results_match_to_their_documented_accuracy() {
    let inputs = [("A1", "-100", n(-100.0)), ("A2", "60", n(60.0)), ("A3", "60", n(60.0))];
    let with = |irr: f64, other: f64| {
        let mut cells = inputs.to_vec();
        cells.extend([
            ("B1", "=IRR(A1:A3)", n(irr)),
            ("B2", "=B1*2", n(irr * 2.0)),
            ("B3", "=IF(B1>0,B1,0)", n(irr)),
            ("C1", "=1/3", n(other)),
            ("D1", "=0*IRR(A1:A3)+1/3", n(other)),
        ]);
        book(&cells)
    };
    let probe = check(&with(0.0, 0.0), &Options::default());
    let Some(irr) = probe.calc.mismatches.iter().find(|m| m.cell == "Sheet1!B1").and_then(|m| m.actual.as_f64()) else { panic!("{probe:?}") };
    let third = 1.0 / 3.0;
    let wrong = |r: &FileReport| {
        let mut cells: Vec<String> = r.calc.mismatches.iter().map(|m| m.cell.clone()).collect();
        cells.sort();
        cells
    };
    // A relative difference of 5e-8: beyond the default tolerance, within the solvers'.
    let r = check(&with(irr * (1.0 + 5e-8), third * (1.0 + 5e-8)), &Options::default());
    assert_eq!((r.calc.checked, r.calc.matched, r.calc.mismatched, r.calc.solver_tolerance), (5, 3, 2, 3), "{r:?}");
    assert_eq!(wrong(&r), ["Sheet1!C1", "Sheet1!D1"]);
    // Beyond the solvers' accuracy it is a mismatch again, and so are its readers.
    let r = check(&with(irr * (1.0 + 5e-6), third), &Options::default());
    assert_eq!((r.calc.matched, r.calc.mismatched, r.calc.solver_tolerance), (2, 3, 0), "{r:?}");
    assert_eq!(wrong(&r), ["Sheet1!B1", "Sheet1!B2", "Sheet1!B3"]);
}

/// A reader carries an input's difference only once the input is a proved match. `E1` reads the
/// solver IRR, but multiplies it by zero: its 5e-8 off the cache is no solver's accuracy and it
/// stays a mismatch, so `F1 = E1*1E8`, 5 off, is one too, however close E1 came. Over a solver
/// mismatch that is settled (`B1`), the reader `B2` still matches.
#[test]
fn a_reader_of_an_unsettled_solver_mismatch_is_a_mismatch() {
    let inputs = [("A1", "-100", n(-100.0)), ("A2", "60", n(60.0)), ("A3", "60", n(60.0))];
    let mut first = inputs.to_vec();
    first.push(("B1", "=IRR(A1:A3)", n(0.0)));
    let probe = check(&book(&first), &Options::default());
    let Some(irr) = probe.calc.mismatches.iter().find(|m| m.cell == "Sheet1!B1").and_then(|m| m.actual.as_f64()) else { panic!("{probe:?}") };
    let mut cells = inputs.to_vec();
    cells.extend([
        ("B1", "=IRR(A1:A3)", n(irr * (1.0 + 5e-8))),
        ("B2", "=B1*2", n(irr * (1.0 + 5e-8) * 2.0)),
        ("E1", "=5E-8+0*IRR(A1:A3)", n(0.0)),
        ("F1", "=E1*100000000", n(0.0)),
    ]);
    let c = check(&book(&cells), &Options::default()).calc;
    let mut wrong: Vec<&str> = c.mismatches.iter().map(|m| m.cell.as_str()).collect();
    wrong.sort();
    assert_eq!(wrong, ["Sheet1!E1", "Sheet1!F1"], "{c:?}");
    assert_eq!((c.matched, c.solver_tolerance), (2, 2));
}

#[test]
fn volatility_propagates_through_a_long_chain_of_names_in_one_walk() {
    // Chain0 uses Chain1, Chain1 uses Chain2, ... and only the last is volatile: listed in this order, a rescan
    // until nothing changes marked one name per pass (quadratic). Every formula that uses a name in
    // the chain is volatile; one that uses a name off the chain is not.
    const LEN: usize = 5_000;
    let mut wb = book(&[("A1", "=Chain0", n(1.0)), ("A2", "=Plain", n(1.0)), ("A3", "=Other", n(1.0))]);
    for i in 0..LEN {
        let formula = if i + 1 == LEN { "NOW()".to_string() } else { format!("Chain{}+1", i + 1) };
        define(&mut wb, &format!("Chain{i}"), None, &formula);
    }
    define(&mut wb, "Plain", None, "Chain0*0+Other");
    define(&mut wb, "Other", None, "1");
    assert_eq!(volatile_of(&wb), (2, 2));
}

#[test]
fn round_trip_of_a_simple_book_is_clean() {
    let mut wb = book(&[("A1", "2", n(2.0)), ("B1", "=A1*3", n(6.0)), ("A2", "x", Value::text("x"))]);
    wb.sheet_mut(0).unwrap().merges.push(gridcraft_engine::core::RangeRef::parse("C1:D2").unwrap());
    let source = gridcraft_engine::io::save_bytes(&wb, "t.xlsx").unwrap();
    let rt = round_trip(&wb, &source, &Options::default());
    assert_eq!(rt.error, None);
    assert!(rt.is_clean(), "{rt:?}");
    assert!(rt.categories.iter().any(|c| c.name == "package"));
}

#[test]
fn diff_detects_lost_features() {
    let mut a = book(&[("A1", "2", n(2.0)), ("B1", "=A1*3", n(6.0))]);
    a.sheet_mut(0).unwrap().merges.push(gridcraft_engine::core::RangeRef::parse("C1:D2").unwrap());
    let mut b = a.clone();
    let sh = b.sheet_mut(0).unwrap();
    sh.merges.clear();
    sh.set_cell(at("A1"), Cell::value(n(3.0)));
    sh.set_cell(at("B1"), Cell::value(n(6.0)));
    b.sheets.push(Arc::new(Sheet::new("Extra")));
    let cats = diff_workbooks(&a, &b, &Options { max_report: 1, ..Default::default() });
    let get = |name: &str| cats.iter().find(|c| c.name == name).unwrap();
    assert_eq!(get("values").diffs, 1);
    assert_eq!(get("values").examples, ["Sheet1!A1: 2 → 3"]);
    assert_eq!(get("formulas").examples, ["Sheet1!B1: =A1*3 → "]);
    assert_eq!(get("merges").examples, ["Sheet1: lost C1:D2"]);
    assert_eq!(get("sheets").diffs, 1);
    assert_eq!(get("tables").diffs, 0);
}

#[test]
fn diff_detects_changed_cond_format_rule() {
    use gridcraft_engine::model::{CfRule, CfValueKind, Color, CondFormat};
    let bar = |gradient: bool, priority: u32| CondFormat {
        ranges: vec![gridcraft_engine::core::RangeRef::parse("A1").unwrap()],
        rule: CfRule::DataBar { min: CfValueKind::Min, max: CfValueKind::Max, color: Color::Rgb(0x638EC6), gradient, show_value: true },
        priority,
        stop_if_true: false,
    };
    let with = |cf: CondFormat| {
        let mut wb = book(&[("A1", "2", n(2.0))]);
        wb.sheet_mut(0).unwrap().cond_formats.push(cf);
        wb
    };
    let diffs = |a: &Workbook, b: &Workbook| diff_workbooks(a, b, &Options::default()).into_iter().find(|c| c.name == "condFormats").unwrap().diffs;
    assert_eq!(diffs(&with(bar(false, 1)), &with(bar(true, 1))), 1);
    assert_eq!(diffs(&with(bar(true, 1)), &with(bar(true, 1))), 0);
    assert_eq!(diffs(&with(bar(true, 1)), &with(bar(true, 5))), 0);
}

fn zip_of(parts: &[(&str, &str)]) -> Vec<u8> {
    let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, body) in parts {
        z.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
        z.write_all(body.as_bytes()).unwrap();
    }
    z.finish().unwrap().into_inner()
}

#[test]
fn package_validation_finds_what_excel_would_repair() {
    let bytes = zip_of(&[
        (
            "[Content_Types].xml",
            r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="r"/><Override PartName="/xl/workbook.xml" ContentType="w"/><Override PartName="/xl/gone.xml" ContentType="g"/></Types>"#,
        ),
        ("_rels/.rels", r#"<Relationships><Relationship Id="rId1" Type="x/officeDocument" Target="xl/workbook.xml"/></Relationships>"#),
        ("xl/workbook.xml", "<workbook/>"),
        ("xl/styles.xml", "<styleSheet/>"),
        (
            "xl/_rels/workbook.xml.rels",
            r#"<Relationships><Relationship Id="rId1" Type="x/styles" Target="styles.xml"/><Relationship Id="rId2" Type="x/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="x/hyperlink" Target="https://example.com" TargetMode="External"/><Relationship Id="rId3" Type="x/theme" Target="/xl/../xl/styles.xml"/></Relationships>"#,
        ),
    ]);
    let issues = validate_package(&bytes).unwrap();
    assert_eq!(
        issues,
        [
            "part xl/styles.xml has no content type",
            "content type override for missing part /xl/gone.xml",
            "xl/_rels/workbook.xml.rels: rId2 (worksheet) targets missing part xl/worksheets/sheet1.xml",
            "xl/_rels/workbook.xml.rels: duplicate relationship id rId2",
        ]
    );
    assert!(validate_package(b"not a zip").is_err());
    let own = gridcraft_engine::io::save_bytes(&book(&[("A1", "=1", n(1.0))]), "t.xlsx").unwrap();
    assert_eq!(validate_package(&own).unwrap(), Vec::<String>::new());
}

#[test]
fn a_link_to_a_place_in_the_workbook_is_not_a_missing_part() {
    let bytes = zip_of(&[
        (
            "[Content_Types].xml",
            r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="r"/><Default Extension="xml" ContentType="x"/></Types>"#,
        ),
        ("xl/drawings/drawing1.xml", "<drawing/>"),
        (
            "xl/drawings/_rels/drawing1.xml.rels",
            r##"<Relationships><Relationship Id="rId1" Type="x/hyperlink" Target="#Sheet1!A1"/><Relationship Id="rId2" Type="x/hyperlink" Target="#"/><Relationship Id="rId3" Type="x/image" Target="../media/gone.png"/></Relationships>"##,
        ),
    ]);
    assert_eq!(validate_package(&bytes).unwrap(), ["xl/drawings/_rels/drawing1.xml.rels: rId3 (image) targets missing part xl/media/gone.png"]);
}

const WB_RELS: &str = "xl/_rels/workbook.xml.rels";
const SHEET_RELS: &str = "xl/worksheets/_rels/sheet1.xml.rels";
const MADE_UP: &str = r#"<Relationship Id="rId90" Type="http://example.com/relationships/madeUp" Target="madeUp.xml"/>"#;
/// Not empty: an empty part isn't a loss.
const MADE_UP_PART: (&str, &str) = ("xl/madeUp.xml", "<madeUp><item/></madeUp>");
/// The made-up part points at a worksheet, a part GridCraft rebuilds, so pass-through can't keep
/// it and it stays lost: the tests that need a known loss stay meaningful.
const MADE_UP_RELS: (&str, &str) = (
    "xl/_rels/madeUp.xml.rels",
    r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#,
);

/// `bytes` with relationships added to rels parts (created when missing) and `parts` added.
fn with_rels(bytes: &[u8], rels: &[(&str, &str)], parts: &[(&str, &str)]) -> Vec<u8> {
    let mut src = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let mut pending: Vec<(&str, &str)> = rels.to_vec();
    for i in 0..src.len() {
        let mut f = src.by_index(i).unwrap();
        let name = f.name().to_string();
        let mut body = String::new();
        f.read_to_string(&mut body).unwrap();
        if let Some(at) = pending.iter().position(|(r, _)| *r == name) {
            body = body.replace("</Relationships>", &format!("{}</Relationships>", pending.remove(at).1));
        }
        out.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
        out.write_all(body.as_bytes()).unwrap();
    }
    let created = pending.iter().map(|(r, extra)| {
        (*r, format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{extra}</Relationships>"#))
    });
    for (name, body) in created.collect::<Vec<_>>().iter().map(|(n, b)| (*n, b.as_str())).chain(parts.iter().copied()) {
        out.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
        out.write_all(body.as_bytes()).unwrap();
    }
    out.finish().unwrap().into_inner()
}

/// The `package` examples of checking `source`.
fn package_of(source: &[u8]) -> Vec<String> {
    package_with(source, &Options::default())
}

fn package_with(source: &[u8], opts: &Options) -> Vec<String> {
    let r = check_bytes("t.xlsx", source, opts);
    r.round_trip.categories.iter().find(|c| c.name == "package").unwrap().examples.clone()
}

fn plain() -> Vec<u8> {
    gridcraft_engine::io::save_bytes(&book(&[("A1", "1", n(1.0))]), "t.xlsx").unwrap()
}

#[test]
fn package_reports_relationship_kinds_the_output_lost() {
    // A kind GridCraft doesn't know is dropped on save; a calculation chain is never written
    // (Excel rebuilds it), so its absence isn't a loss.
    let calc_chain =
        r#"<Relationship Id="rId91" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/calcChain" Target="calcChain.xml"/>"#;
    let source =
        with_rels(&plain(), &[(WB_RELS, &format!("{MADE_UP}{calc_chain}"))], &[MADE_UP_PART, MADE_UP_RELS, ("xl/calcChain.xml", "<calcChain/>")]);
    assert_eq!(package_of(&source), ["lost relationship madeUp in workbook"]);
    assert!(check_bytes("t.xlsx", &plain(), &Options::default()).round_trip.is_clean());
}

// In the three tests below the made-up kind stays lost, so the comparison is known to run.

#[test]
fn an_internal_link_written_as_a_relationship_is_not_a_loss() {
    // openpyxl writes `#Sheet1!B2` as an external relationship; GridCraft writes `location`.
    let link = r##"<Relationship Id="rId5" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="#Sheet1!B2" TargetMode="External"/>"##;
    let source = with_rels(&plain(), &[(WB_RELS, MADE_UP), (SHEET_RELS, link)], &[MADE_UP_PART, MADE_UP_RELS]);
    assert_eq!(package_of(&source), ["lost relationship madeUp in workbook"]);
}

#[test]
fn comments_without_a_comment_are_not_a_loss() {
    let rels = r#"<Relationship Id="rId6" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/comments" Target="../comments1.xml"/><Relationship Id="rId7" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/vmlDrawing" Target="../drawings/vmlDrawing1.vml"/>"#;
    let comments = r#"<comments xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><authors><author>A</author></authors><commentList/></comments>"#;
    let vml = r#"<xml xmlns:v="urn:schemas-microsoft-com:vml" xmlns:o="urn:schemas-microsoft-com:office:office"><o:shapelayout><o:idmap data="1"/></o:shapelayout><v:shapetype id="_x0000_t202" path="m,l,21600r21600,l21600,xe"><v:path gradientshapeok="t"/></v:shapetype></xml>"#;
    let source = with_rels(
        &plain(),
        &[(WB_RELS, MADE_UP), (SHEET_RELS, rels)],
        &[MADE_UP_PART, MADE_UP_RELS, ("xl/comments1.xml", comments), ("xl/drawings/vmlDrawing1.vml", vml)],
    );
    assert_eq!(package_of(&source), ["lost relationship madeUp in workbook"]);
}

#[test]
fn empty_drawings_and_person_lists_are_not_a_loss() {
    let drawing = r#"<Relationship Id="rId8" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/drawing" Target="../drawings/drawing1.xml"/>"#;
    let person = r#"<Relationship Id="rId92" Type="http://schemas.microsoft.com/office/2017/10/relationships/person" Target="persons/person.xml"/>"#;
    let source = with_rels(
        &plain(),
        &[(WB_RELS, &format!("{MADE_UP}{person}")), (SHEET_RELS, drawing)],
        &[
            MADE_UP_PART,
            MADE_UP_RELS,
            ("xl/drawings/drawing1.xml", r#"<xdr:wsDr xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing"/>"#),
            ("xl/persons/person.xml", r#"<personList xmlns="http://schemas.microsoft.com/office/spreadsheetml/2018/threadedcomments"/>"#),
        ],
    );
    assert_eq!(package_of(&source), ["lost relationship madeUp in workbook"]);
}

/// The relationships are followed from the package root only so far. A chain of parts that goes
/// deeper isn't a package without a loss past the limit, it is a package that wasn't compared: the
/// parts beyond it are not looked at, and a check that stops short must say so.
#[test]
fn a_package_whose_relationships_go_past_the_depth_limit_is_not_compared() {
    let chain = 14;
    let rel = |to: usize| format!(r#"<Relationship Id="rChain{to}" Type="http://example.com/relationships/madeUp" Target="c{to}.xml"/>"#);
    let mut rels: Vec<(String, String)> = vec![(WB_RELS.to_string(), rel(1))];
    let mut parts: Vec<(String, String)> = Vec::new();
    for i in 1..=chain {
        parts.push((format!("xl/c{i}.xml"), "<c><item/></c>".to_string()));
        if i < chain {
            rels.push((format!("xl/_rels/c{i}.xml.rels"), rel(i + 1)));
        }
    }
    let rels: Vec<(&str, &str)> = rels.iter().map(|(name, body)| (name.as_str(), body.as_str())).collect();
    let parts: Vec<(&str, &str)> = parts.iter().map(|(name, body)| (name.as_str(), body.as_str())).collect();
    let source = with_rels(&plain(), &rels, &parts);
    let package = package_of(&source);
    let stopped = "relationships of the checked file not compared: relationship check stopped at its limit of 12 owners deep";
    assert!(package.iter().any(|d| d == stopped), "{package:?}");
    assert!(!check_bytes("t.xlsx", &source, &Options::default()).round_trip.is_clean());
}

/// What comparing the relationships may read is a budget, and a package that needs more is a
/// package that wasn't compared, not one without a loss: the check used to skip the comparison
/// and report the file clean. The made-up relationship is a known loss, which the same package
/// shows with the budget it has.
#[test]
fn a_package_whose_relationships_go_past_the_decompression_budget_is_not_compared() {
    let source = with_rels(&plain(), &[(WB_RELS, &format!("{MADE_UP}{}", " ".repeat(64 * 1024)))], &[MADE_UP_PART, MADE_UP_RELS]);
    assert_eq!(package_of(&source), ["lost relationship madeUp in workbook"]);
    let tight = Options { relationship_limits: Limits { bytes: 32 * 1024, ..Limits::default() }, ..Options::default() };
    let stopped = "relationships of the checked file not compared: xl/_rels/workbook.xml.rels: relationship check stopped at its decompression limit";
    assert_eq!(package_with(&source, &tight), [stopped]);
    let report = check_bytes("t.xlsx", &source, &tight);
    assert!(!report.round_trip.is_clean());
    assert_eq!(report.round_trip.differing(), ["package"]);
}

/// Padding that the check doesn't have to read (each target is decided by its first element)
/// can't use the budget up and so hide a loss elsewhere: eight 1 MiB targets against a budget of
/// 512 KiB, and the workbook's relationships are still listed after them.
#[test]
fn padding_unrelated_parts_cannot_hide_a_loss() {
    let padding = " ".repeat(1024 * 1024);
    let root: String = (1..=8)
        .map(|i| format!(r#"<Relationship Id="rIdPad{i}" Type="http://example.com/relationships/padded" Target="xl/padded{i}.xml"/>"#))
        .collect();
    let bodies: Vec<(String, String)> = (1..=8).map(|i| (format!("xl/padded{i}.xml"), format!("<part><child/></part>{padding}"))).collect();
    let mut parts: Vec<(&str, &str)> = vec![MADE_UP_PART, MADE_UP_RELS];
    parts.extend(bodies.iter().map(|(name, body)| (name.as_str(), body.as_str())));
    let source = with_rels(&plain(), &[("_rels/.rels", &root), (WB_RELS, MADE_UP)], &parts);
    let tight = Options { relationship_limits: Limits { bytes: 512 * 1024, ..Limits::default() }, ..Options::default() };
    let package = package_with(&source, &tight);
    assert!(package.iter().any(|d| d == "lost relationship madeUp in workbook"), "{package:?}");
    assert!(package.iter().all(|d| !d.contains("not compared")), "{package:?}");
}

const REL_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

/// A rels part with `(id, kind, target)` relationships; a kind without a `/` is under [`REL_NS`].
fn rels_part(rels: &[(&str, &str, &str)]) -> String {
    let body: String = rels
        .iter()
        .map(|(id, kind, target)| {
            let ty = if kind.contains('/') { kind.to_string() } else { format!("{REL_NS}/{kind}") };
            format!(r#"<Relationship Id="{id}" Type="{ty}" Target="{target}"/>"#)
        })
        .collect();
    format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{body}</Relationships>"#)
}

/// A package part for [`zip_owned`].
fn part(name: &str, body: impl Into<String>) -> (String, String) {
    (name.to_string(), body.into())
}

fn zip_owned(parts: &[(String, String)]) -> Vec<u8> {
    let borrowed: Vec<(&str, &str)> = parts.iter().map(|(n, b)| (n.as_str(), b.as_str())).collect();
    zip_of(&borrowed)
}

/// A package of two sheets (One, Two), each with a drawing. `charts` is how many charts One's
/// drawing holds; Two keeps its drawing only when `two_drawing`.
fn two_sheet_package(charts: usize, two_drawing: bool) -> Vec<u8> {
    let ids: Vec<(String, String)> = (1..=charts).map(|i| (format!("rId{i}"), format!("../charts/chart{i}.xml"))).collect();
    let chart_rels: Vec<(&str, &str, &str)> = ids.iter().map(|(id, target)| (id.as_str(), "chart", target.as_str())).collect();
    let sheet = "<worksheet><sheetData/></worksheet>";
    // A drawing names each chart it shows by relationship id.
    let frames = |n: usize| {
        (1..=n).map(|i| format!(r#"<twoCellAnchor><graphicFrame><chart r:id="rId{i}"/></graphicFrame></twoCellAnchor>"#)).collect::<String>()
    };
    let drawing_of = |n: usize| format!(r#"<wsDr xmlns:r="{REL_NS}">{}</wsDr>"#, frames(n));
    let drawing = drawing_of(charts);
    let mut parts = vec![
        part("_rels/.rels", rels_part(&[("rId1", "officeDocument", "xl/workbook.xml")])),
        part(
            "xl/workbook.xml",
            format!(
                r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="{REL_NS}"><sheets><sheet name="One" sheetId="1" r:id="rId1"/><sheet name="Two" sheetId="2" r:id="rId2"/></sheets></workbook>"#
            ),
        ),
        part(
            "xl/_rels/workbook.xml.rels",
            rels_part(&[("rId1", "worksheet", "worksheets/sheet1.xml"), ("rId2", "worksheet", "worksheets/sheet2.xml")]),
        ),
        part("xl/worksheets/sheet1.xml", sheet),
        part("xl/worksheets/sheet2.xml", sheet),
        part("xl/worksheets/_rels/sheet1.xml.rels", rels_part(&[("rId1", "drawing", "../drawings/drawing1.xml")])),
        part("xl/drawings/drawing1.xml", drawing),
        part("xl/drawings/_rels/drawing1.xml.rels", rels_part(&chart_rels)),
    ];
    if two_drawing {
        parts.push(part("xl/worksheets/_rels/sheet2.xml.rels", rels_part(&[("rId1", "drawing", "../drawings/drawing2.xml")])));
        parts.push(part("xl/drawings/drawing2.xml", drawing_of(1)));
        parts.push(part("xl/drawings/_rels/drawing2.xml.rels", rels_part(&[("rId1", "chart", "../charts/chart1.xml")])));
    }
    parts.extend((1..=charts).map(|i| part(&format!("xl/charts/chart{i}.xml"), "<chartSpace><chart/></chartSpace>")));
    zip_owned(&parts)
}

fn lost(source: &[u8], written: &[u8]) -> Vec<String> {
    lost_relationships(&relationship_tree(source, Limits::default()).unwrap(), &relationship_tree(written, Limits::default()).unwrap())
}

#[test]
fn relationship_losses_are_found_per_owner() {
    let source = two_sheet_package(2, true);
    assert_eq!(lost(&source, &source), Vec::<String>::new());
    // One chart of two gone from One's drawing, and Two's drawing gone with its chart: a count of
    // kinds over the package would see chart and drawing still present.
    assert_eq!(
        lost(&source, &two_sheet_package(1, false)),
        [r#"lost relationship chart in sheet "One" > drawing (1 of 2)"#, r#"lost relationship drawing in sheet "Two""#]
    );
}

/// Each sheet the workbook lists that isn't a worksheet is counted: one international macro
/// sheet of two lost is a loss even though the kind is still present at the workbook.
#[test]
fn losing_one_sheet_of_a_counted_kind_is_a_loss() {
    for kind in ["xlIntlMacrosheet", "xlMacrosheet", "dialogsheet", "chartsheet"] {
        let pkg = |sheets: usize| {
            let listed: String = (1..=sheets).map(|i| format!(r#"<sheet name="S{i}" sheetId="{i}" r:id="rId{i}"/>"#)).collect();
            let ids: Vec<(String, String)> = (1..=sheets).map(|i| (format!("rId{i}"), format!("sheets/sheet{i}.xml"))).collect();
            let rels: Vec<(&str, &str, &str)> = ids.iter().map(|(id, target)| (id.as_str(), kind, target.as_str())).collect();
            let mut parts = vec![
                part("_rels/.rels", rels_part(&[("rId1", "officeDocument", "xl/workbook.xml")])),
                part("xl/workbook.xml", format!(r#"<workbook xmlns:r="{REL_NS}"><sheets>{listed}</sheets></workbook>"#)),
                part("xl/_rels/workbook.xml.rels", rels_part(&rels)),
            ];
            parts.extend((1..=sheets).map(|i| part(&format!("xl/sheets/sheet{i}.xml"), "<worksheet><sheetData/></worksheet>")));
            zip_owned(&parts)
        };
        assert_eq!(lost(&pkg(2), &pkg(2)), Vec::<String>::new(), "{kind}");
        assert_eq!(lost(&pkg(2), &pkg(1)), [format!("lost relationship {kind} in workbook (1 of 2)")], "{kind}");
    }
}

#[test]
fn strict_and_transitional_relationship_names_are_one_kind() {
    let pkg = |kind: Option<&str>| {
        let mut rels = vec![("rId1", "officeDocument", "xl/workbook.xml")];
        rels.extend(kind.map(|k| ("rId2", k, "docProps/app.xml")));
        zip_owned(&[
            part("_rels/.rels", rels_part(&rels)),
            part("xl/workbook.xml", "<workbook><sheets/></workbook>"),
            part("docProps/app.xml", "<Properties><Application>x</Application></Properties>"),
        ])
    };
    let strict = pkg(Some("http://purl.oclc.org/ooxml/officeDocument/relationships/extendedProperties"));
    let transitional = pkg(Some("http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties"));
    assert_eq!(lost(&strict, &transitional), Vec::<String>::new());
    assert_eq!(lost(&transitional, &strict), Vec::<String>::new());
    assert_eq!(lost(&strict, &pkg(None)), ["lost relationship extendedProperties"]);
}

#[test]
fn rebuilt_parts_are_not_losses() {
    // Shared strings are rebuilt from the cells, the dynamic-array metadata from the formulas.
    // The made-up kind is a relationship GridCraft doesn't keep, so the comparison is known to
    // look at the workbook's relationships.
    let pkg = |rebuilt: bool, made_up: bool| {
        let mut rels = Vec::new();
        if made_up {
            rels.push(("rId1", "madeUp", "madeUp.xml"));
        }
        if rebuilt {
            rels.extend([("rId2", "sharedStrings", "sharedStrings.xml"), ("rId3", "sheetMetadata", "metadata.xml")]);
        }
        let mut parts = vec![
            part("_rels/.rels", rels_part(&[("rId1", "officeDocument", "xl/workbook.xml")])),
            part("xl/workbook.xml", "<workbook><sheets/></workbook>"),
            part("xl/_rels/workbook.xml.rels", rels_part(&rels)),
            part("xl/madeUp.xml", "<madeUp><item/></madeUp>"),
        ];
        if rebuilt {
            parts.push(part("xl/sharedStrings.xml", "<sst><si/></sst>"));
            parts.push(part("xl/metadata.xml", "<metadata><x/></metadata>"));
        }
        zip_owned(&parts)
    };
    assert_eq!(lost(&pkg(true, false), &pkg(false, false)), Vec::<String>::new());
    assert_eq!(lost(&pkg(false, false), &pkg(true, false)), Vec::<String>::new());
    assert_eq!(lost(&pkg(true, true), &pkg(false, false)), ["lost relationship madeUp in workbook"]);
}

#[test]
fn binary_parts_count_but_dangling_and_orphaned_relationships_do_not() {
    // Printer settings are a binary part of the sheet: dropping them is a loss. A relationship to
    // a part the package doesn't hold, and the relationships of a part nothing leads to, aren't.
    let pkg = |printer_settings: bool, defects: bool| {
        let mut wb_rels = vec![("rId1", "worksheet", "worksheets/sheet1.xml")];
        let mut parts = vec![
            part("_rels/.rels", rels_part(&[("rId1", "officeDocument", "xl/workbook.xml")])),
            part(
                "xl/workbook.xml",
                format!(r#"<workbook xmlns:r="{REL_NS}"><sheets><sheet name="One" sheetId="1" r:id="rId1"/></sheets></workbook>"#),
            ),
            part("xl/worksheets/sheet1.xml", "<worksheet><sheetData/></worksheet>"),
        ];
        if printer_settings {
            parts.push(part(
                "xl/worksheets/_rels/sheet1.xml.rels",
                rels_part(&[("rId1", "printerSettings", "../printerSettings/printerSettings1.bin")]),
            ));
            parts.push(part("xl/printerSettings/printerSettings1.bin", "\u{1}\u{2}binary settings"));
        }
        if defects {
            wb_rels.push(("rId2", "connections", "connections.xml"));
            parts.push(part("customXml/item2.xml", "<x><y/></x>"));
            parts.push(part("customXml/_rels/item2.xml.rels", rels_part(&[("rId1", "customXmlProps", "itemProps2.xml")])));
            parts.push(part("customXml/itemProps2.xml", "<datastoreItem><schemaRefs/></datastoreItem>"));
        }
        parts.push(part("xl/_rels/workbook.xml.rels", rels_part(&wb_rels)));
        zip_owned(&parts)
    };
    let source = pkg(true, true);
    assert_eq!(lost(&source, &source), Vec::<String>::new());
    assert_eq!(lost(&source, &pkg(false, false)), [r#"lost relationship printerSettings in sheet "One""#]);
}

/// A drawing (or chart) uses a part through the relationship id its XML names; a relationship it
/// never names shows nothing, and dropping it is no loss.
#[test]
fn relationships_a_drawing_never_names_are_not_losses() {
    let pkg = |named: bool, image: bool| {
        let pic = if named { r#"<pic><blipFill><blip r:embed="rId1"/></blipFill></pic>"# } else { "<pic><blipFill/></pic>" };
        let mut parts = vec![
            part("_rels/.rels", rels_part(&[("rId1", "officeDocument", "xl/workbook.xml")])),
            part(
                "xl/workbook.xml",
                format!(r#"<workbook xmlns:r="{REL_NS}"><sheets><sheet name="One" sheetId="1" r:id="rId1"/></sheets></workbook>"#),
            ),
            part("xl/_rels/workbook.xml.rels", rels_part(&[("rId1", "worksheet", "worksheets/sheet1.xml")])),
            part("xl/worksheets/sheet1.xml", "<worksheet><sheetData/></worksheet>"),
            part("xl/worksheets/_rels/sheet1.xml.rels", rels_part(&[("rId1", "drawing", "../drawings/drawing1.xml")])),
            part("xl/drawings/drawing1.xml", format!(r#"<wsDr xmlns:r="{REL_NS}"><twoCellAnchor>{pic}</twoCellAnchor></wsDr>"#)),
        ];
        if image {
            parts.push(part("xl/drawings/_rels/drawing1.xml.rels", rels_part(&[("rId1", "image", "../media/image1.png")])));
            parts.push(part("xl/media/image1.png", "\u{89}PNG data"));
        }
        zip_owned(&parts)
    };
    assert_eq!(lost(&pkg(false, true), &pkg(false, false)), Vec::<String>::new());
    assert_eq!(lost(&pkg(true, true), &pkg(true, false)), [r#"lost relationship image in sheet "One" > drawing"#]);
}

/// A pivot cache counts only while a pivot table of a sheet the workbook lists uses it: Excel
/// deletes a cache with the last pivot table using it, and a sheet relationship `<sheets>`
/// doesn't name is no sheet.
#[test]
fn a_pivot_cache_no_listed_pivot_table_uses_is_not_a_loss() {
    let pkg = |listed: bool, cache: bool| {
        let id = if listed { "rId2" } else { "" };
        let mut wb_rels = vec![("rId1", "worksheet", "worksheets/sheet1.xml"), ("rId2", "worksheet", "worksheets/sheet2.xml")];
        let mut parts = vec![
            part("_rels/.rels", rels_part(&[("rId1", "officeDocument", "xl/workbook.xml")])),
            part(
                "xl/workbook.xml",
                format!(
                    r#"<workbook xmlns:r="{REL_NS}"><sheets><sheet name="One" sheetId="1" r:id="rId1"/><sheet name="Pivot" sheetId="2" r:id="{id}"/></sheets></workbook>"#
                ),
            ),
            part("xl/worksheets/sheet1.xml", "<worksheet><sheetData/></worksheet>"),
            part("xl/worksheets/sheet2.xml", "<worksheet><sheetData/></worksheet>"),
            part("xl/worksheets/_rels/sheet2.xml.rels", rels_part(&[("rId1", "pivotTable", "../pivotTables/pivotTable1.xml")])),
            part("xl/pivotTables/pivotTable1.xml", "<pivotTableDefinition><location/></pivotTableDefinition>"),
            part(
                "xl/pivotTables/_rels/pivotTable1.xml.rels",
                rels_part(&[("rId1", "pivotCacheDefinition", "../pivotCache/pivotCacheDefinition1.xml")]),
            ),
            part("xl/pivotCache/pivotCacheDefinition1.xml", "<pivotCacheDefinition><cacheSource/></pivotCacheDefinition>"),
        ];
        if cache {
            wb_rels.push(("rId3", "pivotCacheDefinition", "pivotCache/pivotCacheDefinition1.xml"));
        }
        parts.push(part("xl/_rels/workbook.xml.rels", rels_part(&wb_rels)));
        zip_owned(&parts)
    };
    assert_eq!(lost(&pkg(false, true), &pkg(false, false)), Vec::<String>::new());
    assert_eq!(lost(&pkg(true, true), &pkg(true, false)), ["lost relationship pivotCacheDefinition in workbook"]);
}

#[test]
fn a_macro_enabled_source_is_saved_macro_enabled() {
    let mut wb = book(&[("A1", "1", n(1.0))]);
    // A compound file's signature: not XML, so its part counts as content.
    wb.vba_project = Some(Arc::new(b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1 vba project".to_vec()));
    let source = gridcraft_engine::io::save_bytes(&wb, "t.xlsm").unwrap();
    let as_xlsm = check_bytes("t.xlsm", &source, &Options::default());
    assert_eq!(as_xlsm.round_trip.error, None);
    assert!(as_xlsm.round_trip.is_clean(), "{:?}", as_xlsm.round_trip);
    // The same bytes under an `.xlsx` name are saved without the project: a loss, so the
    // comparison above is known to look at it.
    let as_xlsx = check_bytes("t.xlsx", &source, &Options::default());
    let package = as_xlsx.round_trip.categories.iter().find(|c| c.name == "package").unwrap();
    assert_eq!(package.examples, ["lost relationship vbaProject in workbook"]);
}

#[test]
fn the_output_name_keeps_the_package_kind() {
    use super::fidelity::output_name;
    assert_eq!(output_name("dir/Book.XLSM"), "roundtrip.xlsm");
    assert_eq!(output_name("a.xltx"), "roundtrip.xltx");
    assert_eq!(output_name("a.xltm"), "roundtrip.xltm");
    assert_eq!(output_name("a.xlsx"), "roundtrip.xlsx");
    assert_eq!(output_name("noextension"), "roundtrip.xlsx");
}

fn with_table(cells: &[(&str, &str, Value)], columns: &[&str]) -> Workbook {
    use gridcraft_engine::model::{Table, TableColumn, TotalsFn};
    let mut wb = book(cells);
    let end = CellRef::new(2, columns.len() as u32 - 1);
    wb.sheet_mut(0).unwrap().tables.push(Table {
        id: 1,
        name: "Table1".into(),
        range: gridcraft_engine::core::RangeRef::new(CellRef::new(0, 0), end),
        header_row: true,
        totals_row: false,
        columns: columns
            .iter()
            .map(|n| TableColumn { name: (*n).into(), totals: TotalsFn::None, totals_label: None, formula: None, unique_name: None, xml: None })
            .collect(),
        style: String::new(),
        banded_rows: false,
        banded_cols: false,
        first_col: false,
        last_col: false,
        filter_button: false,
        filter: Vec::new(),
        table_type: None,
        connection_id: None,
    });
    wb
}

fn value_diffs(a: &Workbook, b: &Workbook) -> Vec<String> {
    diff_workbooks(a, b, &Options::default()).into_iter().find(|c| c.name == "values").unwrap().examples
}

#[test]
fn a_blank_table_header_filled_with_its_column_name_is_not_a_difference() {
    // Excel requires a header cell to hold its column's name, so the writer fills a blank one.
    let body = [("A2", "x", Value::text("x")), ("A3", "y", Value::text("y")), ("B2", "1", n(1.0)), ("B3", "2", n(2.0))];
    let blank = with_table(&body, &["Column1", "Amount"]);
    let mut repaired = blank.clone();
    repaired.sheet_mut(0).unwrap().set_cell(at("A1"), Cell::value(Value::text("Column1")));
    repaired.sheet_mut(0).unwrap().set_cell(at("B1"), Cell::value(Value::text("Amount")));
    assert_eq!(value_diffs(&blank, &repaired), Vec::<String>::new());
    // A header that is filled with something other than its column name, or a cell outside the
    // header row, is still a difference.
    let mut wrong = blank.clone();
    wrong.sheet_mut(0).unwrap().set_cell(at("A1"), Cell::value(Value::text("Other")));
    assert_eq!(value_diffs(&blank, &wrong), ["Sheet1!A1: (blank) → \"Other\""]);
    let mut outside = blank.clone();
    outside.sheet_mut(0).unwrap().set_cell(at("C1"), Cell::value(Value::text("Column1")));
    assert_eq!(value_diffs(&blank, &outside), ["Sheet1!C1: (blank) → \"Column1\""]);
    // Nor does a header that had a value of its own count as repaired.
    let mut named = with_table(&body, &["Column1", "Amount"]);
    named.sheet_mut(0).unwrap().set_cell(at("A1"), Cell::value(Value::text("Old")));
    let mut renamed = named.clone();
    renamed.sheet_mut(0).unwrap().set_cell(at("A1"), Cell::value(Value::text("Column1")));
    assert_eq!(value_diffs(&named, &renamed), ["Sheet1!A1: \"Old\" → \"Column1\""]);
}

#[test]
fn sheets_are_compared_by_name() {
    // A dropped first sheet must not make every later sheet differ from its neighbour.
    let sheet = |name: &str, v: f64| {
        let mut s = Sheet::new(name);
        s.set_cell(at("A1"), Cell::value(n(v)));
        Arc::new(s)
    };
    let mut a = Workbook::new();
    a.sheets = vec![sheet("First", 1.0), sheet("Second", 2.0), sheet("Third", 3.0)];
    let mut b = a.clone();
    b.sheets.remove(0);
    let cats = diff_workbooks(&a, &b, &Options::default());
    let get = |name: &str| cats.iter().find(|c| c.name == name).unwrap();
    assert_eq!(get("sheets").diffs, 1);
    assert_eq!(get("values").diffs, 0);
    // A renamed sheet is still met in its own position.
    let mut renamed = a.clone();
    renamed.sheets[1] = sheet("Renamed", 20.0);
    let cats = diff_workbooks(&a, &renamed, &Options::default());
    let values = cats.iter().find(|c| c.name == "values").unwrap();
    assert_eq!(values.examples, ["Second!A1: 2 → 20"]);
}

#[test]
fn json_lists_warning_messages() {
    let with = FileReport { path: "a.ods".into(), warnings: vec!["ODS data-only import".into()], ..Default::default() };
    let without = FileReport { path: "b.xlsx".into(), ..Default::default() };
    let v = report_json(&[with, without], &Options::default());
    assert_eq!(v["files"][0]["warnings"], json!(["ODS data-only import"]));
    assert_eq!(v["files"][1]["warnings"], json!([]));
}

#[test]
fn unopenable_files_are_reported() {
    let r = check_bytes("broken.xlsx", b"PK\x03\x04 not really", &Options::default());
    assert!(r.error.is_some());
    assert!(!r.is_clean());
}

#[test]
fn json_schema_keys() {
    let wb = book(&[("A1", "=1+1", n(3.0))]);
    let reports = vec![check(&wb, &Options::default()), check_bytes("bad.xlsx", b"", &Options::default())];
    let v = report_json(&reports, &Options::default());
    let keys = |v: &Json| v.as_object().unwrap().keys().cloned().collect::<BTreeSet<String>>();
    let set = |k: &[&str]| k.iter().map(|s| s.to_string()).collect::<BTreeSet<String>>();
    assert_eq!(keys(&v), set(&["schema", "tolerance", "files", "totals", "functions", "signatures"]));
    assert_eq!(v["schema"], SCHEMA);
    let f = &v["files"][0];
    assert_eq!(keys(f), set(&["path", "opened", "error", "warnings", "producer", "calc", "roundTrip"]));
    assert_eq!(keys(&f["producer"]), set(&["application", "version", "kind"]));
    assert_eq!(
        keys(&f["calc"]),
        set(&[
            "formulaCells",
            "checked",
            "matched",
            "mismatched",
            "localeDependent",
            "locale",
            "noOracle",
            "noOracleReasons",
            "staleCache",
            "volatile",
            "volatileViaNames",
            "boolAsNumber",
            "errorDetailCache",
            "whitespaceCache",
            "solverTolerance",
            "cancellationResidue",
            "iterationChange",
            "staleCell",
            "legacyExcelVersion",
            "circularReference",
            "cacheTrust",
            "calcPr",
            "untrustedMatched",
            "untrustedMismatched",
            "mismatches",
            "functions",
            "signatures",
            "untrustedSignatures"
        ])
    );
    assert_eq!(f["calc"]["signatures"], json!({"number → number": 1}));
    assert_eq!(keys(&f["calc"]["mismatches"][0]), set(&["cell", "formula", "expected", "actual", "kind", "functions", "primary", "class"]));
    assert_eq!(keys(&f["calc"]["calcPr"]), set(&["mode", "calcCompleted", "fullCalcOnLoad", "iterate", "calcId"]));
    assert_eq!(f["calc"]["mismatches"][0]["expected"], json!(3.0));
    assert_eq!(keys(&f["roundTrip"]), set(&["error", "differing", "categories"]));
    assert_eq!(
        keys(&f["roundTrip"]["categories"]),
        set(&[
            "sheets",
            "values",
            "formulas",
            "numFmts",
            "merges",
            "names",
            "tables",
            "condFormats",
            "validations",
            "charts",
            "comments",
            "hyperlinks",
            "pivots",
            "package"
        ])
    );
    assert_eq!(keys(&f["roundTrip"]["categories"]["values"]), set(&["equal", "diffs", "examples"]));
    assert_eq!(v["files"][1]["opened"], json!(false));
    assert_eq!(
        keys(&v["totals"]),
        set(&[
            "files",
            "opened",
            "failedToOpen",
            "formulaCells",
            "checked",
            "matched",
            "mismatched",
            "localeDependent",
            "noOracle",
            "noOracleReasons",
            "volatile",
            "volatileViaNames",
            "boolAsNumber",
            "errorDetailCache",
            "whitespaceCache",
            "solverTolerance",
            "cancellationResidue",
            "iterationChange",
            "staleCell",
            "legacyExcelVersion",
            "circularReference",
            "untrustedMatched",
            "untrustedMismatched",
            "untrusted",
            "roundTripFailed",
            "roundTripDiffering",
            "categoriesDiffering",
            "producers"
        ])
    );
    assert_eq!(keys(&v["functions"][0]), set(&["name", "checked", "mismatched", "primary"]));
}

/// With iterative calculation off, a cell on a circular reference and a cell reading one hold what
/// Excel had when it gave up the loop: their mismatches are listed as circularReference and don't
/// gate. A mismatch outside the loop's reach still gates; with iteration on the loop's do too.
#[test]
fn mismatches_on_or_reading_a_circular_reference_do_not_gate() {
    let mut wb = book(&[("A1", "1", n(1.0)), ("B1", "=SUM(A1:B1)", n(5.0)), ("C1", "=B1*2", n(4.0)), ("D1", "=A1+1", n(3.0))]);
    let c = check(&wb, &Options::default()).calc;
    let class = |c: &CalcReport, cell: &str| c.mismatches.iter().find(|m| m.cell.ends_with(cell)).map(|m| m.class);
    assert_eq!((c.checked, c.circular_reference, c.mismatched), (3, 2, 1), "{:?}", c.mismatches);
    assert_eq!(class(&c, "!B1"), Some(CacheTrust::CircularReference));
    assert_eq!(class(&c, "!C1"), Some(CacheTrust::CircularReference));
    assert_eq!(class(&c, "!D1"), Some(CacheTrust::Trusted));
    wb.calc.iterative = true;
    let c = check(&wb, &Options::default()).calc;
    assert_eq!(c.circular_reference, 0, "{:?}", c.mismatches);
}

/// `bytes` as Excel saved it with the calculation engine `calc_id` (`124519` is Excel 2007's,
/// `145621` Excel 2010's): the producer the file names, and the version of its properties.
fn saved_by_excel(bytes: &[u8], calc_id: &str) -> Vec<u8> {
    saved_by_excel_version(bytes, calc_id, "12.0000")
}

/// [`saved_by_excel`] with the `AppVersion` of the application (`15.0300` is Excel 2013's,
/// `16.0300` Excel 2016's and later): the version that calculated the cache when `calc_id` is `0`.
fn saved_by_excel_version(bytes: &[u8], calc_id: &str, app_version: &str) -> Vec<u8> {
    let b = patched(bytes, "xl/workbook.xml", r#"<calcPr calcId="191029"/>"#, &format!(r#"<calcPr calcId="{calc_id}"/>"#));
    let b = patched(&b, "xl/workbook.xml", r#"appName="gridcraft""#, r#"appName="xl""#);
    patched(
        &b,
        "docProps/app.xml",
        "<Application>GridCraft</Application>",
        &format!("<Application>Microsoft Excel</Application><AppVersion>{app_version}</AppVersion>"),
    )
}

/// [`saved_by_excel`] for a package with a formula that has no saved value, which GridCraft
/// writes with `fullCalcOnLoad`: Excel saves the results of the calculation it finished, and a
/// formula it left without one is no reason to calculate on load.
fn saved_by_excel_uncached(bytes: &[u8], calc_id: &str) -> Vec<u8> {
    let b = patched(bytes, "xl/workbook.xml", r#"<calcPr calcId="191029" fullCalcOnLoad="1"/>"#, r#"<calcPr calcId="191029"/>"#);
    saved_by_excel(&b, calc_id)
}

/// Excel 2007 (calcId 12xxxx) cached NORMSINV from the algorithm Excel 2010 replaced: the
/// mismatch, the cell that only reads it, and arithmetic over the call itself are the older
/// version's answer and don't gate. A mismatch Excel 2007's algorithm doesn't explain (SUM) still
/// does, and so does a cell reading it; so does one where the call carries nothing to the result
/// (`X+0*NORMSINV(…)`, or `X+0*A1` over the legacy A1): the old algorithm is no explanation there.
#[test]
fn an_older_excel_s_algorithm_is_its_version_s_answer() {
    let old_normsinv = 1.644_853_000_470_902_8;
    let wb = book(&[
        ("A1", "=NORMSINV(0.95)", n(old_normsinv)),
        ("B1", "=A1*2", n(3.289_706_000_941_805_6)),
        ("C1", "=SUM(1,2)", n(4.0)),
        ("D1", "=A1+C1", n(5.0)),
        ("E1", "=NORMSINV(0.95)*2+1", n(old_normsinv * 2.0 + 1.0)),
        ("F1", "=0.0000005+0*NORMSINV(0.95)", n(0.0)),
        ("G1", "=0.0000005+0*A1", n(0.0)),
    ]);
    let bytes = gridcraft_engine::io::save_bytes(&wb, "t.xlsx").unwrap();
    let old = check_bytes("t.xlsx", &saved_by_excel(&bytes, "124519"), &Options::default()).calc;
    assert_eq!((old.checked, old.legacy_excel_version, old.mismatched), (7, 3, 4), "{:?}", old.mismatches);
    let class = |c: &CalcReport, cell: &str| c.mismatches.iter().find(|m| m.cell.ends_with(cell)).map(|m| m.class);
    for (cell, want) in [
        ("!A1", CacheTrust::LegacyExcelVersion),
        ("!B1", CacheTrust::LegacyExcelVersion),
        ("!D1", CacheTrust::Trusted),
        ("!E1", CacheTrust::LegacyExcelVersion),
        ("!F1", CacheTrust::Trusted),
        ("!G1", CacheTrust::Trusted),
    ] {
        assert_eq!(class(&old, cell), Some(want), "{cell}");
    }
    // Excel 2010's engine (calcId 14xxxx) has the new algorithm: the same caches gate.
    let new = check_bytes("t.xlsx", &saved_by_excel(&bytes, "145621"), &Options::default()).calc;
    assert_eq!((new.legacy_excel_version, new.mismatched), (0, 7));
}

/// An array formula over the older version's results is that version's answer too when, with the
/// legacy cell holding its saved value, it gives its cache back, as a scalar reader's is: `B1`
/// over a NORMSINV that Excel 2007 cached. `C1`, whose cache leaves out the `+1` its formula adds,
/// still gates.
#[test]
fn an_array_formula_over_a_legacy_result_is_its_versions_answer_only_when_it_gives_its_cache_back() {
    let old_normsinv = 1.644_853_000_470_902_8;
    let mut wb = book(&[("A1", "=NORMSINV(0.95)", n(old_normsinv))]);
    for (a1, formula) in [("B1", "=SQRT(MMULT(A1:A1,TRANSPOSE(A1:A1)))"), ("C1", "=SQRT(MMULT(A1:A1,TRANSPOSE(A1:A1)))+1")] {
        let mut f = Formula::new(formula);
        f.array = gridcraft_engine::core::RangeRef::parse(&format!("{a1}:{a1}"));
        wb.sheet_mut(0).unwrap().set_cell(at(a1), Cell { value: n(old_normsinv), formula: Some(Arc::new(f)), ..Default::default() });
    }
    let bytes = gridcraft_engine::io::save_bytes(&wb, "t.xlsx").unwrap();
    let c = check_bytes("t.xlsx", &saved_by_excel(&bytes, "124519"), &Options::default()).calc;
    assert_eq!((c.checked, c.legacy_excel_version, c.mismatched), (3, 2, 1), "{:?}", c.mismatches);
    let class = |cell: &str| c.mismatches.iter().find(|m| m.cell.ends_with(cell)).map(|m| m.class);
    assert_eq!(class("!A1"), Some(CacheTrust::LegacyExcelVersion));
    assert_eq!(class("!B1"), Some(CacheTrust::LegacyExcelVersion));
    assert_eq!(class("!C1"), Some(CacheTrust::Trusted));
}

/// SUMPRODUCT over a reference to one cell that holds text: Excel 2013 (AppVersion 15.0300, calcId
/// 152511) read it as the value in the cell and cached `#VALUE!`, Excel 2016 and later read it as an
/// array whose text counts as 0, and so does GridCraft (Excel 16.114). A cache is that version's
/// answer when the formula, evaluated as GridCraft calculated it with the older reading and
/// nothing else changed, gives it back, wherever the call is in the formula and whatever the cache is:
/// - `C1`: the poi shape, a one-cell range written with two corners at the root;
/// - `D1`: the shape of a workbook saved by Excel 15.0300 (156 cells of it), a division nested
///   in an IF over references built by INDEX, where the `#VALUE!` of SUMPRODUCT comes before the
///   `#DIV/0!` of its divisor `SQRT(0)` (Excel 16.114 gives `#DIV/0!` for it);
/// - `E1`: a reader of `D1`;
/// - `F1`: a cache that isn't an error, the fallback of an IFERROR that caught the older reading's.
///
/// The older reading doesn't give back the caches of `G1` (it gives 7) and `H1` (it gives 5, the
/// same as the current reading: the formula differs from its cache for no reason of the version),
/// which gate. Excel 2016's version, or no Excel's, gates every one.
#[test]
fn a_sumproduct_over_a_one_cell_reference_is_excel_2013_s_answer_when_its_reading_gives_the_cache_back() {
    let value = Value::Error(CellError::Value);
    let (text, number) = ("INDEX(A1:A2,1):INDEX(A1:A2,1)", "INDEX(B1:B2,1):INDEX(B1:B2,1)");
    let nested = format!("=IF(TRUE,SUMPRODUCT({text},{number})/SQRT(SUM({text})*SUM(A1)),\"\")");
    let wb = book(&[
        ("A1", "=IF(FALSE,1,\"\")", Value::text("")),
        ("B1", "2", n(2.0)),
        ("C1", "=SUMPRODUCT(A1:A1,B1:B1)", value.clone()),
        ("D1", &nested, value.clone()),
        ("E1", "=D1", value.clone()),
        ("F1", "=IFERROR(SUMPRODUCT(A1,B1),7)", n(7.0)),
        ("G1", "=IFERROR(SUMPRODUCT(A1,B1),7)", value.clone()),
        ("H1", "=SUMPRODUCT(B1:B1,B1)+1", n(4.0)),
    ]);
    let bytes = gridcraft_engine::io::save_bytes(&wb, "t.xlsx").unwrap();
    let class = |c: &CalcReport, cell: &str| c.mismatches.iter().find(|m| m.cell.ends_with(cell)).map(|m| m.class);
    for (name, file) in [("calcId 152511", saved_by_excel(&bytes, "152511")), ("AppVersion 15.0300", saved_by_excel_version(&bytes, "0", "15.0300"))]
    {
        let c = check_bytes("t.xlsx", &file, &Options::default()).calc;
        assert_eq!((c.checked, c.matched, c.legacy_excel_version, c.mismatched), (7, 1, 4, 2), "{name}: {:?}", c.mismatches);
        for cell in ["!C1", "!D1", "!E1", "!F1"] {
            assert_eq!(class(&c, cell), Some(CacheTrust::LegacyExcelVersion), "{name} {cell}");
        }
        for cell in ["!G1", "!H1"] {
            assert_eq!(class(&c, cell), Some(CacheTrust::Trusted), "{name} {cell}");
        }
    }
    for (name, file) in [
        ("calcId 191029", saved_by_excel(&bytes, "191029")),
        ("AppVersion 16.0300", saved_by_excel_version(&bytes, "0", "16.0300")),
        ("not Excel", bytes.clone()),
    ] {
        let c = check_bytes("t.xlsx", &file, &Options::default()).calc;
        assert_eq!((c.checked, c.matched, c.legacy_excel_version, c.mismatched), (7, 1, 0, 6), "{name}: {:?}", c.mismatches);
    }
}

/// The older reading explains what it changes and nothing else. Excel 2013 (calcId 150000) cached
/// `B1` as 0 after calculating `A1` as 0 (GridCraft's `A1` is 1: a cell it gets wrong), and `B1`
/// adds a SUMPRODUCT of two numbers that it multiplies by 0, which neither reading of SUMPRODUCT
/// changes. Evaluated on the cached `A1` the formula gives its cache back, and that is `A1`'s
/// error carried into `B1`, not the older reading: `B1` gates with `A1`, as it does for an Excel
/// 2016 and whether `A1` has a cache or not.
#[test]
fn a_sumproduct_the_older_reading_does_not_change_does_not_explain_its_mismatch() {
    let class = |c: &CalcReport, cell: &str| c.mismatches.iter().find(|m| m.cell.ends_with(cell)).map(|m| m.class);
    for (a1_name, a1, checked, no_oracle) in [("A1 cached", n(0.0), 2, 0), ("A1 uncached", Value::Empty, 1, 1)] {
        let uncached = a1.is_empty();
        let wb = book(&[("A1", "=0+1", a1), ("B1", "=A1+0*SUMPRODUCT(C1,D1)", n(0.0)), ("C1", "2", n(2.0)), ("D1", "3", n(3.0))]);
        let bytes = gridcraft_engine::io::save_bytes(&wb, "t.xlsx").unwrap();
        for (name, calc_id) in [("Excel 2013", "150000"), ("Excel 2016", "160000")] {
            let file = if uncached { saved_by_excel_uncached(&bytes, calc_id) } else { saved_by_excel(&bytes, calc_id) };
            let c = check_bytes("t.xlsx", &file, &Options::default()).calc;
            let what = format!("{name}, {a1_name}: {:?}", c.mismatches);
            assert_eq!((c.checked, c.legacy_excel_version, c.mismatched, c.no_oracle), (checked, 0, checked, no_oracle), "{what}");
            assert_eq!(class(&c, "!B1"), Some(CacheTrust::Trusted), "{what}");
        }
    }
}

/// A reader of a legacy cell is the older version's answer when its formula gives the cache back
/// from the legacy cell's saved value and GridCraft's own value of every other input, whether
/// that input has a cache or not. `L1` is a SUMPRODUCT over text, `#VALUE!` for Excel 2013. `M1`
/// reads it and an uncached `U1` that GridCraft calculates as 1 where Excel's was 0 (the cache
/// is `L1`'s old answer plus that 0): GridCraft's error, so `M1` gates. `M2` reads `L1` and an
/// uncached `U2` that GridCraft has right: its cache is the older version's.
#[test]
fn a_reader_of_a_legacy_cell_keeps_gridcrafts_value_of_its_other_inputs() {
    let wb = book(&[
        ("A1", "=IF(FALSE,1,\"\")", Value::text("")),
        ("B1", "2", n(2.0)),
        ("L1", "=SUMPRODUCT(A1,B1)", Value::Error(CellError::Value)),
        ("U1", "=0+1", Value::Empty),
        ("M1", "=IF(ISERROR(L1),0,1)+U1", n(0.0)),
        ("U2", "=1+1", Value::Empty),
        ("M2", "=IF(ISERROR(L1),0,1)+U2", n(2.0)),
    ]);
    let bytes = gridcraft_engine::io::save_bytes(&wb, "t.xlsx").unwrap();
    let c = check_bytes("t.xlsx", &saved_by_excel_uncached(&bytes, "150000"), &Options::default()).calc;
    let class = |cell: &str| c.mismatches.iter().find(|m| m.cell.ends_with(cell)).map(|m| m.class);
    assert_eq!((c.checked, c.matched, c.legacy_excel_version, c.mismatched, c.no_oracle), (4, 1, 2, 1, 2), "{:?}", c.mismatches);
    assert_eq!(class("!L1"), Some(CacheTrust::LegacyExcelVersion));
    assert_eq!(class("!M1"), Some(CacheTrust::Trusted));
    assert_eq!(class("!M2"), Some(CacheTrust::LegacyExcelVersion));
}

/// A reader of a legacy cell is judged on the cells it reads, not on the range it reads them from.
/// `R1` picks `L1` (a legacy `#VALUE!`) out of `L1:M1`, whose other cell `M1` is a mismatch of
/// GridCraft's own (it calculates 1 where the cache says 0), as the 78 readers in a workbook saved by
/// Excel 15.0300 pick one cell out of a block that holds each other. `R2` reads
/// `M1` itself, so the error that is GridCraft's reaches it, and it gates.
#[test]
fn a_reader_of_a_legacy_cell_is_not_held_open_by_the_mismatches_in_the_range_it_picks_from() {
    let wb = book(&[
        ("A1", "=IF(FALSE,1,\"\")", Value::text("")),
        ("B1", "2", n(2.0)),
        ("L1", "=SUMPRODUCT(A1,B1)", Value::Error(CellError::Value)),
        ("M1", "=0+1", n(0.0)),
        ("R1", "=IF(ISERROR(INDEX(L1:M1,1,1)),0,1)", n(0.0)),
        ("R2", "=IF(ISERROR(L1),0,1)+M1", n(0.0)),
    ]);
    let bytes = gridcraft_engine::io::save_bytes(&wb, "t.xlsx").unwrap();
    let c = check_bytes("t.xlsx", &saved_by_excel(&bytes, "150000"), &Options::default()).calc;
    let class = |cell: &str| c.mismatches.iter().find(|m| m.cell.ends_with(cell)).map(|m| m.class);
    assert_eq!((c.checked, c.matched, c.legacy_excel_version, c.mismatched), (5, 1, 2, 2), "{:?}", c.mismatches);
    assert_eq!(class("!L1"), Some(CacheTrust::LegacyExcelVersion));
    assert_eq!(class("!R1"), Some(CacheTrust::LegacyExcelVersion));
    assert_eq!(class("!M1"), Some(CacheTrust::Trusted));
    assert_eq!(class("!R2"), Some(CacheTrust::Trusted));
}

/// A reader of a legacy cell reads the cache's digits of the numbers GridCraft agrees with it on:
/// `A2` is cached 8e-10 above GridCraft's 0.5, within the tolerance, and `R1` multiplies that by
/// 1E9 (as `$H$7*H28` does in an Excel-cached option workbook, where the old NORMSDIST of 0 is 2e-10 off and
/// the cell 91 times it). `A3` is cached 1e-7 above, beyond the tolerance: a mismatch of its own,
/// so GridCraft's value stays and `R2`, which multiplies it by 1E6, gates.
#[test]
fn a_reader_of_a_legacy_cell_reads_the_digits_of_the_numbers_the_cache_agrees_on() {
    let (a2, a3) = (0.5000000008_f64, 0.5000001_f64);
    let wb = book(&[
        ("A1", "=IF(FALSE,1,\"\")", Value::text("")),
        ("B1", "2", n(2.0)),
        ("L1", "=SUMPRODUCT(A1,B1)", Value::Error(CellError::Value)),
        ("A2", "=1/2", n(a2)),
        ("A3", "=1/2", n(a3)),
        ("R1", "=IF(ISERROR(L1),100,0)+(A2-0.5)*1E9", n(100.0 + (a2 - 0.5) * 1E9)),
        ("R2", "=IF(ISERROR(L1),100,0)+(A3-0.5)*1E6", n(100.0 + (a3 - 0.5) * 1E6)),
    ]);
    let bytes = gridcraft_engine::io::save_bytes(&wb, "t.xlsx").unwrap();
    let c = check_bytes("t.xlsx", &saved_by_excel(&bytes, "150000"), &Options::default()).calc;
    let class = |cell: &str| c.mismatches.iter().find(|m| m.cell.ends_with(cell)).map(|m| m.class);
    assert_eq!((c.checked, c.matched, c.legacy_excel_version, c.mismatched), (6, 2, 2, 2), "{:?}", c.mismatches);
    assert_eq!(class("!L1"), Some(CacheTrust::LegacyExcelVersion));
    assert_eq!(class("!R1"), Some(CacheTrust::LegacyExcelVersion));
    assert_eq!(class("!A3"), Some(CacheTrust::Trusted));
    assert_eq!(class("!R2"), Some(CacheTrust::Trusted));
}

/// Excel 2007 answered `#DIV/0!` for `PMT(0,0,…)` where Excel 2010 and later answer `#NUM!` (GridCraft),
/// and its IRR gave up (`#NUM!`) on flows whose root it didn't find. Such a cache is the older version's
/// answer when the formula, calculated as GridCraft does with the call replaced by what the older
/// algorithm may have answered, gives it back, wherever the call is in the formula:
/// - `D1`: the shape of an Excel 2007 workbook's `Expenses!D90`, the call inside arithmetic;
/// - `E1`: a reader of `D1`;
/// - `F1`: the shape of an Excel 2007 workbook's `IF(ISERROR(IRR(…)),"NA",IRR(…))` over flows with a root.
///
/// Not a version's answer, whatever the calls: `G1` and `K1` (the formula gives another cache for no
/// older answer), `H1` (PMT finds a number in every version, there is no older error to put in), `I1`
/// (SQRT didn't change) and `J1` (the IF answers another text). Excel 2010's engine, or no Excel's,
/// gates every one.
#[test]
fn an_older_excel_s_error_answer_is_its_versions_when_the_formula_gives_the_cache_back_with_it() {
    let (div0, num) = (Value::Error(CellError::Div0), Value::Error(CellError::Num));
    let irr = "IF(ISERROR(IRR(A2:C2)),\"NA\",IRR(A2:C2))";
    let wb = book(&[
        ("A1", "0", n(0.0)),
        ("B1", "0", n(0.0)),
        ("A2", "-3600000", n(-3_600_000.0)),
        ("B2", "203454.9840005725", n(203_454.984_000_572_5)),
        ("C2", "200318.59927751697", n(200_318.599_277_516_97)),
        ("D1", "=PMT(A1,B1,5)*12/2", div0.clone()),
        ("E1", "=D1+1", div0.clone()),
        ("F1", &format!("={irr}"), Value::text("NA")),
        ("G1", "=PMT(A1,B1,5)*12/2", n(7.0)),
        ("H1", "=PMT(0.1,2,5)*2", num.clone()),
        ("I1", "=SQRT(-1)*2", div0.clone()),
        ("J1", &format!("={irr}"), Value::text("other")),
        ("K1", "=D1+1", n(8.0)),
    ]);
    let bytes = gridcraft_engine::io::save_bytes(&wb, "t.xlsx").unwrap();
    let class = |c: &CalcReport, cell: &str| c.mismatches.iter().find(|m| m.cell.ends_with(cell)).map(|m| m.class);
    let old = check_bytes("t.xlsx", &saved_by_excel(&bytes, "124519"), &Options::default()).calc;
    assert_eq!((old.checked, old.matched, old.legacy_excel_version, old.mismatched), (8, 0, 3, 5), "{:?}", old.mismatches);
    for cell in ["!D1", "!E1", "!F1"] {
        assert_eq!(class(&old, cell), Some(CacheTrust::LegacyExcelVersion), "{cell}");
    }
    for cell in ["!G1", "!H1", "!I1", "!J1", "!K1"] {
        assert_eq!(class(&old, cell), Some(CacheTrust::Trusted), "{cell}");
    }
    for (name, file) in [("Excel 2010", saved_by_excel(&bytes, "145621")), ("not Excel", bytes.clone())] {
        let c = check_bytes("t.xlsx", &file, &Options::default()).calc;
        assert_eq!((c.legacy_excel_version, c.mismatched), (0, 8), "{name}: {:?}", c.mismatches);
    }
}

/// Excel 2007's cache of a call may be the older answer only for that call, and only where the call's
/// own inputs are documented to give it. The cells are the formulas of the
/// repro workbooks (indep2, nested_mod, ns_na, et_ns), cached by Excel 2007 (calcId 120000):
/// - `A1` to `A4`: `MATCH("a",B1:B2,0)` is 1 in GridCraft and `#N/A` in the cache, alone, with
///   `+NORMSINV(0.5)` (0 in every version), and with `+MOD(5,0)` cached `#N/A` and `#REF!` (`#DIV/0!` in
///   every version): none of those calls has an older answer for its inputs, the MATCH is the
///   difference, and all gate;
/// - `B3` `MOD(5,0)*2` cached `#N/A`, `B4` `IF(ISNA(NORMSINV(2)),"na","other")` and `B5`
///   `ERROR.TYPE(NORMSINV(2))`: errors every version gives, nothing to replace, gate;
/// - `C1` `MATCH(…)+PMT(0,0,5)` cached `#DIV/0!`: PMT's documented older answer reproduces the cache
///   and the MATCH, a number, is not what differs, so it is excused; `C2` `NA()+PMT(0,0,5)` cached
///   `#DIV/0!`: the `#N/A` operand comes first with either answer of the PMT, so the cache is not
///   PMT's and it gates;
/// - `D1` `PMT(0,0,5)` cached `#DIV/0!` is excused (the documented case); `D2`
///   `MOD(940000000,7)+0` cached `#NUM!` is past 2^27 times the divisor: excused for Excel 2003's
///   engine (`114000`), gates for Excel 2007's, which no longer had the limit.
#[test]
fn an_older_answer_is_substituted_only_for_the_call_and_the_inputs_it_is_documented_for() {
    let (na, div0, num, refe) =
        (Value::Error(CellError::NA), Value::Error(CellError::Div0), Value::Error(CellError::Num), Value::Error(CellError::Ref));
    let wb = book(&[
        ("B1", "a", Value::text("a")),
        ("B2", "b", Value::text("b")),
        ("E1", "0", n(0.0)),
        ("A1", "=MATCH(\"a\",B1:B2,0)", na.clone()),
        ("A2", "=MATCH(\"a\",B1:B2,0)+NORMSINV(0.5)", na.clone()),
        ("A3", "=MATCH(\"a\",B1:B2,0)+MOD(5,0)", na.clone()),
        ("A4", "=MATCH(\"a\",B1:B2,0)+MOD(5,0)", refe.clone()),
        ("B3", "=MOD(5,0)*2", na.clone()),
        ("B4", "=IF(ISNA(NORMSINV(2)),\"na\",\"other\")", Value::text("na")),
        ("B5", "=ERROR.TYPE(NORMSINV(2))", n(4.0)),
        ("C1", "=MATCH(\"a\",B1:B2,0)+PMT(E1,E1,5)", div0.clone()),
        ("C2", "=NA()+PMT(E1,E1,5)", div0.clone()),
        ("D1", "=PMT(E1,E1,5)", div0.clone()),
        ("D2", "=MOD(940000000,7)+0", num.clone()),
    ]);
    let bytes = gridcraft_engine::io::save_bytes(&wb, "t.xlsx").unwrap();
    let class = |c: &CalcReport, cell: &str| c.mismatches.iter().find(|m| m.cell.ends_with(cell)).map(|m| m.class);
    let gates = |c: &CalcReport, cells: &[&str]| {
        for cell in cells {
            assert_eq!(class(c, cell), Some(CacheTrust::Trusted), "{cell}: {:?}", c.mismatches);
        }
    };
    let excel_2007 = check_bytes("t.xlsx", &saved_by_excel(&bytes, "120000"), &Options::default()).calc;
    // No older answer of the calls explains an operand that differs.
    gates(&excel_2007, &["!A1", "!A2", "!A3", "!A4", "!B3", "!B4", "!B5", "!C2", "!D2"]);
    assert_eq!(class(&excel_2007, "!C1"), Some(CacheTrust::LegacyExcelVersion), "{:?}", excel_2007.mismatches);
    assert_eq!(class(&excel_2007, "!D1"), Some(CacheTrust::LegacyExcelVersion), "{:?}", excel_2007.mismatches);
    // Excel 2003's MOD had the quotient limit; its PMT is the 2007 one.
    let excel_2003 = check_bytes("t.xlsx", &saved_by_excel(&bytes, "114000"), &Options::default()).calc;
    assert_eq!(class(&excel_2003, "!D2"), Some(CacheTrust::LegacyExcelVersion), "{:?}", excel_2003.mismatches);
    gates(&excel_2003, &["!A1", "!A2", "!A3", "!A4", "!B3", "!B4", "!B5", "!C2"]);
    assert_eq!(class(&excel_2003, "!D1"), Some(CacheTrust::LegacyExcelVersion));
    // Excel 2010 and no Excel at all have no older algorithm to blame.
    for (name, file) in [("Excel 2010", saved_by_excel(&bytes, "145621")), ("not Excel", bytes.clone())] {
        let c = check_bytes("t.xlsx", &file, &Options::default()).calc;
        assert_eq!(c.legacy_excel_version, 0, "{name}: {:?}", c.mismatches);
    }
}

/// An Excel before 2010 gave up on an IRR only where a cache of that version shows it.
/// The old rule ran Newton on the rate and excused `#NUM!` for any flows it left,
/// which Excel doesn't: Excel 16.115 finds -0.8 and -0.5 for `{-5,1}` and `{-100,50}` (Newton on the
/// discount factor), and Excel 2002's own cache of `IRR(…,22%)` over -85, 120, 100, -138 is 0.45188
/// (a published homework answer key). Cells, all cached `#NUM!` unless said:
/// - `B1`, `D1`: the two-flow repros, and `B2` `IF(ISERROR(IRR(…)),99,0)` cached 99;
/// - `J1`: the homework flows at 22%;
/// - `P1`: a workbook's flows where Excel 16.115 answers `#NUM!` and GridCraft's fallback search
///   finds a number (GridCraft's own difference): not an old answer, whatever the version;
/// - `E3` and `F3`: the flows (-3600000, 203454.98, 200318.60) an Excel 2007 workbook cached `NA` for, so an
///   Excel 2007 cache of `#NUM!` for them is that version's give-up (`E3`), the IF of that workbook too (`F3`);
///   Excel 2002 and 2003 have no such cache, and a third flow past the cached list (`E4`) gates.
///
/// Excel 2007, 2003 and 2002 gate every cell but `E3`/`F3` under 2007.
#[test]
fn an_older_excel_s_irr_gives_up_only_where_a_cache_shows_it() {
    let num = Value::Error(CellError::Num);
    let fallback_flows = [
        -12_644.438_140_059_743,
        -12_644.438_140_059_758,
        -12_644.438_140_059_738,
        -5_489.146_504_616_277_5,
        -12_024.894_441_835_102,
        5_172.260_881_731_922,
        6_111.951_989_683_528,
    ];
    let mut cells: Vec<(String, String, Value)> = vec![
        ("A1".into(), "-5".into(), n(-5.0)),
        ("A2".into(), "1".into(), n(1.0)),
        ("C1".into(), "-100".into(), n(-100.0)),
        ("C2".into(), "50".into(), n(50.0)),
        ("B1".into(), "=IRR(A1:A2)".into(), num.clone()),
        ("B2".into(), "=IF(ISERROR(IRR(A1:A2)),99,0)".into(), n(99.0)),
        ("D1".into(), "=IRR(C1:C2)".into(), num.clone()),
        ("J1".into(), "=IRR(F1:I1,0.22)".into(), num.clone()),
        ("A3".into(), "-3600000".into(), n(-3_600_000.0)),
        ("B3".into(), "203454.9840005725".into(), n(203_454.984_000_572_5)),
        ("C3".into(), "200318.59927751697".into(), n(200_318.599_277_516_97)),
        ("E3".into(), "=IRR(A3:C3)".into(), num.clone()),
        ("F3".into(), "=IF(ISERROR(IRR(A3:C3)),\"NA\",IRR(A3:C3))".into(), Value::text("NA")),
        ("D3".into(), "197179.11870029898".into(), n(197_179.118_700_298_98)),
        ("E4".into(), "=IRR(A3:D3,0.3)".into(), num.clone()),
    ];
    for (c, x) in ["F1", "G1", "H1", "I1"].into_iter().zip([-85.0, 120.0, 100.0, -138.0]) {
        cells.push((c.into(), x.to_string(), n(x)));
    }
    for (i, x) in fallback_flows.iter().enumerate() {
        let col = (b'L' + i as u8) as char;
        cells.push((format!("{col}1"), x.to_string(), n(*x)));
    }
    cells.push(("T1".into(), "=IRR(L1:R1)".into(), num.clone()));
    let refs: Vec<(&str, &str, Value)> = cells.iter().map(|(a, f, v)| (a.as_str(), f.as_str(), v.clone())).collect();
    let bytes = gridcraft_engine::io::save_bytes(&book(&refs), "t.xlsx").unwrap();
    let class = |c: &CalcReport, cell: &str| c.mismatches.iter().find(|m| m.cell.ends_with(cell)).map(|m| m.class);
    for (name, calc_id, app) in [("Excel 2007", "120000", "12.0000"), ("Excel 2003", "110000", "11.0000"), ("Excel 2002", "100000", "10.0000")] {
        let c = check_bytes("t.xlsx", &saved_by_excel_version(&bytes, calc_id, app), &Options::default()).calc;
        let want_legacy: &[&str] = if name == "Excel 2007" { &["!E3", "!F3"] } else { &[] };
        for cell in want_legacy {
            assert_eq!(class(&c, cell), Some(CacheTrust::LegacyExcelVersion), "{name} {cell}: {:?}", c.mismatches);
        }
        for cell in
            ["!B1", "!B2", "!D1", "!J1", "!E4", "!T1"].into_iter().chain((name != "Excel 2007").then_some(["!E3", "!F3"]).into_iter().flatten())
        {
            assert_ne!(class(&c, cell), Some(CacheTrust::LegacyExcelVersion), "{name} {cell}: {:?}", c.mismatches);
            assert!(class(&c, cell).is_some(), "{name} {cell} is a mismatch: {:?}", c.mismatches);
        }
        for cell in ["!B1", "!B2", "!D1", "!J1", "!E4"] {
            assert_eq!(class(&c, cell), Some(CacheTrust::Trusted), "{name} {cell} gates: {:?}", c.mismatches);
        }
    }
}

/// IRR's `#DIV/0!` of Excel 97 to 2002 for flows with no result (Microsoft KB 30567) is still excused, and
/// only for those versions: `A1`, `A2` of -5 and -3 (no positive flow: GridCraft `#NUM!`).
#[test]
fn an_excel_2002_irr_with_no_result_is_div0() {
    let (div0, num) = (Value::Error(CellError::Div0), Value::Error(CellError::Num));
    let wb = book(&[("A1", "-5", n(-5.0)), ("A2", "-3", n(-3.0)), ("B1", "=IRR(A1:A2)", div0), ("B2", "=IRR(A1:A2)", num)]);
    let bytes = gridcraft_engine::io::save_bytes(&wb, "t.xlsx").unwrap();
    let class = |c: &CalcReport, cell: &str| c.mismatches.iter().find(|m| m.cell.ends_with(cell)).map(|m| m.class);
    let old = check_bytes("t.xlsx", &saved_by_excel_version(&bytes, "100000", "10.0000"), &Options::default()).calc;
    assert_eq!(class(&old, "!B1"), Some(CacheTrust::LegacyExcelVersion), "{:?}", old.mismatches);
    assert_eq!(class(&old, "!B2"), None, "#NUM! is what GridCraft gives");
    let new = check_bytes("t.xlsx", &saved_by_excel_version(&bytes, "120000", "12.0000"), &Options::default()).calc;
    assert_eq!(class(&new, "!B1"), Some(CacheTrust::Trusted), "{:?}", new.mismatches);
}

/// Excel XP and 97 gave numbers for NORMSINV in the tail (KB 826772; McCullough and Wilson 2005, Table 1:
/// -4.75367 at 1E-6, -4.99152 at 3E-7), never `#N/A`: a cached `#N/A` there is no old answer,
/// and neither is the ISNA wrapper's 99. The cells are a repro workbook's, cached by Excel 2002 and 97, and
/// gate; the numbers Excel XP did give are still the
/// old algorithm's within its accuracy (`A4`, cached -4.75367 over GridCraft's -4.753424).
#[test]
fn a_normal_inverse_tail_is_no_na_to_an_older_excel() {
    let na = Value::Error(CellError::NA);
    let wb = book(&[
        ("A1", "=NORMSINV(0.000001)", na.clone()),
        ("A2", "=NORMSINV(0.0000001)", na.clone()),
        ("A3", "=NORMINV(0.9999999,0,1)", na.clone()),
        ("A4", "=NORMSINV(0.000001)", n(-4.753_67)),
        ("B1", "=IF(ISNA(NORMSINV(0.000001)),99,0)", n(99.0)),
        ("B2", "=NORMSINV(0.0000003)*2+1", na.clone()),
    ]);
    let bytes = gridcraft_engine::io::save_bytes(&wb, "t.xlsx").unwrap();
    let class = |c: &CalcReport, cell: &str| c.mismatches.iter().find(|m| m.cell.ends_with(cell)).map(|m| m.class);
    for (name, file) in [
        ("Excel 2002", saved_by_excel_version(&bytes, "100000", "10.0000")),
        ("Excel 97", saved_by_excel_version(&bytes, "0", "8.0000")),
        ("Excel 2007", saved_by_excel(&bytes, "120000")),
    ] {
        let c = check_bytes("t.xlsx", &file, &Options::default()).calc;
        for cell in ["!A1", "!A2", "!A3", "!B1", "!B2"] {
            assert_eq!(class(&c, cell), Some(CacheTrust::Trusted), "{name} {cell}: {:?}", c.mismatches);
        }
    }
}

/// A call's older answer excuses a cell only when the call's arguments are verified:
/// the control run swaps the whole call, so a wrong operand inside the arguments is not seen by it.
/// `MATCH("a",B1:B2,0)` is 1 to GridCraft and `#N/A` in the cache, so every cell whose arguments contain it
/// gates, as the bare MATCH (`A1`) does, however the older answer of the surrounding call fits the cache:
/// - `A2` `MOD(MATCH(…)*2^28,1)` cached `#NUM!` (past 2^27 times the divisor to Excel 2003);
/// - `A3` `IF(ISERROR(MOD(MATCH(…)*2^28,1)),"none",1)` cached "none";
/// - `A4` `PMT(MATCH(…)*0,0,5)` cached `#DIV/0!` (Excel 2007's PMT with no periods).
/// - `A5` `NORMSINV(MATCH(…)*1E-7)` of a repro workbook cached by Excel 2002, cached `#N/A`, and its
///   `NORMSINV` has no `#N/A` entry besides.
///
/// Held: `B4` `MOD(C3*2^28,1)` over a constant and `B5` over a formula cell whose cache GridCraft matches
/// (`C4`) are excused; `B6` over a formula cell GridCraft gets wrong (`C5`, cached `#N/A` for `MATCH`) gates.
#[test]
fn a_call_s_older_answer_needs_its_arguments_verified() {
    let (na, num, div0) = (Value::Error(CellError::NA), Value::Error(CellError::Num), Value::Error(CellError::Div0));
    let match_a = "MATCH(\"a\",B1:B2,0)";
    let wb = book(&[
        ("B1", "a", Value::text("a")),
        ("B2", "b", Value::text("b")),
        ("C3", "1", n(1.0)),
        ("C4", "=C3+0", n(1.0)),
        ("C5", &format!("={match_a}"), na.clone()),
        ("A1", &format!("={match_a}"), na.clone()),
        ("A2", &format!("=MOD({match_a}*2^28,1)"), num.clone()),
        ("A3", &format!("=IF(ISERROR(MOD({match_a}*2^28,1)),\"none\",1)"), Value::text("none")),
        ("A4", &format!("=PMT({match_a}*0,0,5)"), div0.clone()),
        ("A5", &format!("=NORMSINV({match_a}*1E-7)"), na.clone()),
        ("B4", "=MOD(C3*2^28,1)", num.clone()),
        ("B5", "=MOD(C4*2^28,1)", num.clone()),
        ("B6", "=MOD(C5*2^28,1)", num.clone()),
    ]);
    let bytes = gridcraft_engine::io::save_bytes(&wb, "t.xlsx").unwrap();
    let class = |c: &CalcReport, cell: &str| c.mismatches.iter().find(|m| m.cell.ends_with(cell)).map(|m| m.class);
    let excel_2003 = check_bytes("t.xlsx", &saved_by_excel_version(&bytes, "110000", "11.0000"), &Options::default()).calc;
    for cell in ["!A1", "!A2", "!A3", "!A5", "!B6"] {
        assert_eq!(class(&excel_2003, cell), Some(CacheTrust::Trusted), "{cell}: {:?}", excel_2003.mismatches);
    }
    for cell in ["!B4", "!B5"] {
        assert_eq!(class(&excel_2003, cell), Some(CacheTrust::LegacyExcelVersion), "{cell}: {:?}", excel_2003.mismatches);
    }
    let excel_2007 = check_bytes("t.xlsx", &saved_by_excel(&bytes, "120000"), &Options::default()).calc;
    for cell in ["!A4", "!A5"] {
        assert_eq!(class(&excel_2007, cell), Some(CacheTrust::Trusted), "{cell}: {:?}", excel_2007.mismatches);
    }
}

/// Excel 2007 answered `#NUM!` for a normal density below the smallest normal number
/// (a workbook calculated by Excel 12: `NORMDIST(220,P1,P2,0)` cached `#NUM!`); Excel 2010 and later underflow it to 0, as
/// GridCraft does. It is that version's answer when the cache is `#NUM!` for a density that underflows
/// (`A1`), not for one that doesn't (`A2`), not for the cumulative form (`A3`), and Excel 2010's
/// engine, or no Excel's, gates `A1`.
#[test]
fn an_older_excel_s_num_for_an_underflowing_normal_density_is_its_versions() {
    let num = Value::Error(CellError::Num);
    let wb = book(&[
        ("A1", "=NORMDIST(220,100,1,0)", num.clone()),
        ("A2", "=NORMDIST(5,0,1,0)", num.clone()),
        ("A3", "=NORMDIST(220,100,1,1)", num.clone()),
    ]);
    let bytes = gridcraft_engine::io::save_bytes(&wb, "t.xlsx").unwrap();
    let class = |c: &CalcReport, cell: &str| c.mismatches.iter().find(|m| m.cell.ends_with(cell)).map(|m| m.class);
    let old = check_bytes("t.xlsx", &saved_by_excel(&bytes, "124519"), &Options::default()).calc;
    assert_eq!(class(&old, "!A1"), Some(CacheTrust::LegacyExcelVersion), "{:?}", old.mismatches);
    assert_eq!(class(&old, "!A2"), Some(CacheTrust::Trusted));
    assert_eq!(class(&old, "!A3"), Some(CacheTrust::Trusted));
    for (name, file) in [("Excel 2010", saved_by_excel(&bytes, "145621")), ("not Excel", bytes.clone())] {
        let c = check_bytes("t.xlsx", &file, &Options::default()).calc;
        assert_eq!(class(&c, "!A1"), Some(CacheTrust::Trusted), "{name}");
    }
}

/// A date text without a year takes the year of the clock it's calculated under (Microsoft's
/// DATEVALUE): the cell is volatile, and holds its saved value for what reads it (graded). One
/// with a year, or one only compared as text, is graded.
#[test]
fn a_date_text_without_a_year_is_volatile() {
    let feb_2020 = 43_883.0;
    let wb = book(&[
        ("A1", "=DATEVALUE(\"Feb 22\")", n(feb_2020)),
        ("B1", "=A1+1", n(feb_2020 + 1.0)),
        ("C1", "=\"01-Sep\"+0", n(44_075.0)),
        ("D1", "=DATEVALUE(\"Feb 22 2020\")", n(feb_2020)),
        ("E1", "=IF(\"Feb 22\"=\"x\",1,2)", n(2.0)),
    ]);
    let c = check(&wb, &Options::default()).calc;
    assert_eq!((c.volatile, c.checked, c.mismatched), (2, 3, 0), "{:?}", c.mismatches);
}

/// Excel's WEEKNUM with a negative return type gives 1 or `#NUM!` from one calculation to the
/// next (Excel 16.114; poi FormulaEvalTestData.xls caches 1): the cell is volatile. A documented
/// or a zero return type is graded.
#[test]
fn weeknum_with_a_negative_return_type_is_volatile() {
    let wb = book(&[
        ("A1", "=WEEKNUM(DATE(2013,1,1),-1)", n(1.0)),
        ("B1", "=WEEKNUM(DATE(2013,1,1),2)", n(1.0)),
        ("C1", "=WEEKNUM(DATE(2013,1,1),0)", Value::Error(CellError::Num)),
    ]);
    let c = check(&wb, &Options::default()).calc;
    assert_eq!((c.volatile, c.checked, c.mismatched), (1, 2, 0), "{:?}", c.mismatches);
}

/// A difference within the rounding the formula's operations propagate from what it reads is
/// residue and matches: `A1−B1` carries the difference between A1's cache and GridCraft's
/// (Excel's 1e9 + 1e-6 where GridCraft gives 1e9, within the tolerance). Beyond the bound it is a
/// mismatch, and so is any difference when the inputs are the same constants, when a large
/// operand is multiplied by an exact zero, or when the formula calls what isn't arithmetic.
#[test]
fn residue_within_the_propagated_rounding_bound_matches() {
    let wb = book(&[
        ("A1", "=1000000000+0", n(1e9 + 1e-6)),
        ("B1", "=1000000000+0", n(1e9)),
        // What Excel calculates from its own A1 and B1.
        ("C1", "=A1-B1", n((1e9 + 1e-6) - 1e9)),
        ("D1", "=A1-B1", n(3e-4)),
        ("E1", "1000000000", n(1e9)),
        ("F1", "1000000000", n(1e9)),
        ("G1", "=E1-F1", n(3e-6)),
        ("H1", "100000000000000000000", n(1e20)),
        ("I1", "=2+0*H1", n(0.0)),
        ("J1", "=SQRT(4)+0*H1", n(0.0)),
    ]);
    let c = check(&wb, &Options::default()).calc;
    assert_eq!((c.checked, c.matched, c.cancellation_residue, c.mismatched), (7, 3, 1, 4), "{:?}", c.mismatches);
    let mut wrong: Vec<&str> = c.mismatches.iter().map(|m| m.cell.as_str()).collect();
    wrong.sort();
    assert_eq!(wrong, ["Sheet1!D1", "Sheet1!G1", "Sheet1!I1", "Sheet1!J1"]);
}

/// `^` and EXP carry the error of what they read like the arithmetic does, with a few units in the
/// last place of their own (Excel's `x^y` is `e^(y·ln x)` and its exponential isn't GridCraft's):
/// `A1^2-B1^2` over a `1e9 + 1e-6` that Excel read where GridCraft has `1e9` is up to ~2000 in
/// Excel, and `EXP(C1)-EXP(D1)` over a `10 + 1e-9` up to ~2.2e-5; a cache beyond the bound is a
/// mismatch, and so is a power of an input that is itself a mismatch (E1).
#[test]
fn power_and_exp_carry_the_error_of_their_operands() {
    let wb = book(&[
        ("A1", "=1000000000+0", n(1e9 + 1e-6)),
        ("B1", "1000000000", n(1e9)),
        ("A2", "=A1^2-B1^2", n(2000.0)),
        ("A3", "=A1^2-B1^2", n(5000.0)),
        ("C1", "=10+0", n(10.000_000_001)),
        ("D1", "10", n(10.0)),
        ("C2", "=EXP(C1)-EXP(D1)", n(2.0e-5)),
        ("C3", "=EXP(C1)-EXP(D1)", n(1e-3)),
        ("E1", "=1E-300*1E-300+0", n(1e-3)),
        ("E2", "=E1^0.5", n(3.2e-5)),
    ]);
    let c = check(&wb, &Options::default()).calc;
    let mut wrong: Vec<&str> = c.mismatches.iter().map(|m| m.cell.as_str()).collect();
    wrong.sort();
    assert_eq!(wrong, ["Sheet1!A3", "Sheet1!C3", "Sheet1!E1", "Sheet1!E2"], "{:?}", c.mismatches);
    assert_eq!(c.cancellation_residue, 2, "{c:?}");
}

/// IF is bounded by the branch its condition picks when no rounding can change the condition: a
/// comparison whose sides are further apart than their errors, OR and ISERROR of defined names that
/// each name one cell (an engineering workbook's `IF(OR(ISERROR(RECYR1),ISERROR(PURR1)),100,FEEDR1+RECYR1-PURR1)`).
/// A condition inside the errors (`X=A2` over a `1e9 + 1e-6` and `1e9`) is undecided and the
/// mismatch stands.
#[test]
fn if_over_names_is_bounded_by_the_branch_a_rounding_error_cannot_change() {
    let mut wb = book(&[
        ("A1", "=1000000000+0", n(1e9 + 1e-6)),
        ("A2", "1000000000", n(1e9)),
        ("B1", "=IF(OR(ISERROR(X),ISERROR(Y)),2,X-Y)", n((1e9 + 1e-6) - 1e9)),
        ("B2", "=IF(OR(ISERROR(X),ISERROR(Y)),2,X-Y)", n(3e-4)),
        ("B3", "=IF(X=Y,0,X-Y)", n((1e9 + 1e-6) - 1e9)),
        ("B4", "=IF(X>5,X-Y,0)", n((1e9 + 1e-6) - 1e9)),
        ("B5", "=IF(X>5,X-Y,0)", n(3e-4)),
    ]);
    define(&mut wb, "X", None, "Sheet1!$A$1");
    define(&mut wb, "Y", None, "Sheet1!$A$2");
    let c = check(&wb, &Options::default()).calc;
    let mut wrong: Vec<&str> = c.mismatches.iter().map(|m| m.cell.as_str()).collect();
    wrong.sort();
    assert_eq!(wrong, ["Sheet1!B2", "Sheet1!B3", "Sheet1!B5"], "{:?}", c.mismatches);
}

/// With iterative calculation on, a loop's cached value within Maximum Change of GridCraft's is a
/// state Excel stopped iterating at when the formula, once on the file's saved state, moves it no
/// further than moving the loop by Maximum Change moves the formula; the same difference off the
/// loop is a mismatch, and so is one on a loop whose formula doesn't read the loop's state.
#[test]
fn an_iterative_loop_matches_where_excel_s_stopping_rule_explains_it() {
    // B1 = A1 reads the loop's previous value, which is A1's cache: B1 matches as it is.
    let mut wb = book(&[("A1", "=0.5*B1+1", n(1.9995)), ("B1", "=A1", n(1.9995)), ("C1", "=C1*0+0.0004", n(0.0)), ("E1", "=1+0.5", n(1.5004))]);
    wb.calc.iterative = true;
    let c = check(&wb, &Options::default()).calc;
    assert_eq!((c.checked, c.matched, c.iteration_change, c.mismatched), (4, 2, 1, 2), "{:?}", c.mismatches);
    let mut wrong: Vec<&str> = c.mismatches.iter().map(|m| m.cell.as_str()).collect();
    wrong.sort();
    assert_eq!(wrong, ["Sheet1!C1", "Sheet1!E1"]);
}

/// The loop oracle's formula is Excel's arithmetic, never GridCraft's evaluator: `D1` reads the
/// loop, and its cache is what the loop gives without the `SQRT` term, an error of SQRT for all
/// the oracle can tell. GridCraft's value is within Maximum Change and the loop's sensitivity
/// would cover the term, but a function call has no independent value and the mismatch stands.
/// The same term as arithmetic (`D2`) is the stopping rule's.
#[test]
fn an_iterative_loop_explains_only_its_arithmetic() {
    let mut wb = book(&[
        ("A1", "=0.5*B1+1", n(1.9995)),
        ("B1", "=A1", n(1.9995)),
        ("D1", "=0.5*B1+SQRT(4)*0.0002", n(0.5 * 1.9995)),
        ("D2", "=0.5*B1+0.0004", n(0.5 * 1.9995)),
    ]);
    wb.calc.iterative = true;
    let c = check(&wb, &Options::default()).calc;
    let wrong: Vec<&str> = c.mismatches.iter().map(|m| m.cell.as_str()).collect();
    assert_eq!(wrong, ["Sheet1!D1"], "{c:?}");
    assert_eq!(c.matched, 3);
    assert!(c.iteration_change >= 1);
}

/// An engineering workbook's loops close through defined names (`RECYR3` = `Flowsheet!$E$17`), which the
/// dependency graph doesn't hold: the loop is found by calculating. `A1` reads `B1` only through
/// the name `N`, and its cache within Maximum Change is a state the stopping rule leaves.
#[test]
fn an_iterative_loop_closed_through_a_name_is_found() {
    let mut wb = book(&[("A1", "=0.5*N+1", n(1.9995)), ("B1", "=A1", n(1.9995)), ("E1", "=1+0.5", n(1.5004))]);
    define(&mut wb, "N", None, "Sheet1!$B$1");
    wb.calc.iterative = true;
    let c = check(&wb, &Options::default()).calc;
    let wrong: Vec<&str> = c.mismatches.iter().map(|m| m.cell.as_str()).collect();
    assert_eq!(wrong, ["Sheet1!E1"], "{c:?}");
    assert!(c.iteration_change >= 1, "{c:?}");
}

/// How far the loop moves a formula is taken one cell of it at a time: `A1-B1` over two loop
/// cells that each settle within Maximum Change (0.001) of GridCraft's moves by twice that, though
/// the two moved together cancel (`D1`, 8e-4 off); a cache beyond Maximum Change (`D2`) is not a
/// state the stopping rule leaves at all.
#[test]
fn an_iterative_loop_moves_a_difference_by_each_cell_it_reads() {
    let mut wb = book(&[("A1", "=0.5*B1+1", n(1.9995)), ("B1", "=A1", n(1.9995)), ("D1", "=A1-B1", n(0.0008)), ("D2", "=A1-B1", n(0.0025))]);
    wb.calc.iterative = true;
    let c = check(&wb, &Options::default()).calc;
    let wrong: Vec<&str> = c.mismatches.iter().map(|m| m.cell.as_str()).collect();
    assert_eq!(wrong, ["Sheet1!D2"], "{c:?}");
}

/// SUM over a range written as the two cells its ends name (`_YR1:YP`) is bounded like SUM over
/// the range: `TOPN:LOWN` is `A1:A2`, and the `1e9 + 1e-6` Excel read at A1 carries into the sum.
#[test]
fn sum_over_a_range_between_two_names_is_bounded() {
    let mut wb = book(&[
        ("A1", "=1000000000+0", n(1e9 + 1e-6)),
        ("A2", "1", n(1.0)),
        ("B1", "=SUM(TOPN:LOWN)-1000000000", n(1.0 + 1e-6)),
        ("B2", "=SUM(TOPN:LOWN)-1000000000", n(1.0 + 1e-3)),
    ]);
    define(&mut wb, "TOPN", None, "Sheet1!$A$1");
    define(&mut wb, "LOWN", None, "Sheet1!$A$2");
    let c = check(&wb, &Options::default()).calc;
    let wrong: Vec<&str> = c.mismatches.iter().map(|m| m.cell.as_str()).collect();
    assert_eq!(wrong, ["Sheet1!B2"], "{c:?}");
    assert_eq!(c.cancellation_residue, 1, "{c:?}");
}

/// Excel compensates a sum near zero only as the formula's last operation: Excel 16.114 gives
/// 1E15 for `=((1E15+1)-1E15)*1E15` and -2.78E-17 for `=(0.5-0.4-0.1)`. A nested sum's residue,
/// amplified, is no rounding allowance: caches of 0 for both mismatch.
#[test]
fn only_the_last_operation_is_compensated_near_zero() {
    let wb = book(&[("A1", "=((1000000000000000+1)-1000000000000000)*1000000000000000", n(0.0)), ("A2", "=(0.5-0.4-0.1)*1E20", n(0.0))]);
    let c = check(&wb, &Options::default()).calc;
    assert_eq!((c.mismatched, c.cancellation_residue), (2, 0), "{c:?}");
}

const THUMBNAIL_PART: &str = "docProps/thumbnail.wmf";

/// `plain()` with a thumbnail relationship on the package root and `thumbnail` as its entry. The
/// zip is built byte by byte: entries of a damaged kind can't be copied through a zip writer.
fn with_thumbnail(thumbnail: gridcraft_xlsx::testing::Entry<'_>) -> Vec<u8> {
    use gridcraft_xlsx::testing::{Entry, damaged_zip};
    let thumbnail_rel = r#"<Relationship Id="rId90" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/thumbnail" Target="docProps/thumbnail.wmf"/>"#;
    let mut src = zip::ZipArchive::new(Cursor::new(plain())).unwrap();
    let mut parts: Vec<(String, Vec<u8>)> = vec![];
    for i in 0..src.len() {
        let mut f = src.by_index(i).unwrap();
        let mut body = vec![];
        f.read_to_end(&mut body).unwrap();
        let body = match f.name() {
            "_rels/.rels" => String::from_utf8(body).unwrap().replace("</Relationships>", &format!("{thumbnail_rel}</Relationships>")).into_bytes(),
            "[Content_Types].xml" => String::from_utf8(body)
                .unwrap()
                .replacen("<Override", r#"<Default Extension="wmf" ContentType="image/x-wmf"/><Override"#, 1)
                .into_bytes(),
            _ => body,
        };
        parts.push((f.name().to_string(), body));
    }
    let mut entries: Vec<Entry<'_>> = parts.iter().map(|(n, b)| Entry::Intact(n, b)).collect();
    entries.push(thumbnail);
    damaged_zip(&entries)
}

/// A thumbnail whose entry declares 0 bytes holds nothing, whether or not its deflate entry has a
/// stream: reading it can't be done, and that is not a lost part. The writer keeps it as an empty
/// part, so the round trip is clean. (Before, the unreadable stream counted as content in the
/// source, and the empty part written back as none.)
#[test]
fn a_thumbnail_declared_empty_is_neither_content_nor_a_loss() {
    use gridcraft_xlsx::testing::Entry;
    let source = with_thumbnail(Entry::EmptyWithoutStream(THUMBNAIL_PART));
    assert_eq!(package_of(&source), Vec::<String>::new());
    assert_eq!(lost(&source, &plain()), Vec::<String>::new());
    // The same part with a stream that decodes to nothing is empty too.
    let empty_stored = with_thumbnail(Entry::Intact(THUMBNAIL_PART, b""));
    assert_eq!(lost(&empty_stored, &plain()), Vec::<String>::new());
}

/// What can't be told apart from content still counts: a part that holds bytes and is gone from
/// the written file is a loss, whether the bytes can be read (the checker reads it) or not.
#[test]
fn a_thumbnail_with_content_that_is_gone_is_still_a_loss() {
    use gridcraft_xlsx::testing::Entry;
    let readable = with_thumbnail(Entry::Intact(THUMBNAIL_PART, b"\xD7\xCD\xC6\x9A not really a wmf"));
    assert_eq!(lost(&readable, &plain()), ["lost relationship thumbnail"]);
    // GridCraft keeps a thumbnail it can read: the round trip loses nothing.
    assert_eq!(package_of(&readable), Vec::<String>::new());
    // One that can't be decoded at all is counted as holding something (nothing says it is
    // empty), so GridCraft dropping it (with a warning on import) is reported as the loss it is.
    for damaged in [Entry::BadStream(THUMBNAIL_PART, 40), Entry::BadChecksum(THUMBNAIL_PART, b"\xD7\xCD\xC6\x9A not really a wmf")] {
        let source = with_thumbnail(damaged);
        assert_eq!(lost(&source, &plain()), ["lost relationship thumbnail"]);
        assert_eq!(package_of(&source), ["lost relationship thumbnail"]);
    }
}
