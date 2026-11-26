//! Move or Copy > Create a copy: the copy's tab name and the defined names it gets, as Excel for
//! Mac 16.115.2 did in the X23 probe `x23-p1-copies` (`Data` copied twice, then the first copy).

use std::io::Read;

use gridcraft_core::CellRef;
use gridcraft_model::Workbook;
use serde_json::json;

use super::s;
use crate::Session;

fn wb(s: &Session) -> &Workbook {
    &s.doc().unwrap().wb
}

fn sheet_names(s: &Session) -> Vec<String> {
    wb(s).sheets.iter().map(|sh| sh.name.clone()).collect()
}

fn formula(s: &Session, sheet: &str, a1: &str) -> String {
    let wb = wb(s);
    let si = wb.sheet_index(sheet).unwrap_or_else(|| panic!("no sheet {sheet}"));
    wb.sheets[si].cell(CellRef::parse(a1).unwrap()).and_then(|c| c.formula.clone()).map(|f| f.text.clone()).unwrap_or_default()
}

/// Every defined name as (scope's sheet name or `None`, name, formula), sorted.
fn names(s: &Session) -> Vec<(Option<String>, String, String)> {
    let wb = wb(s);
    let mut out: Vec<_> = wb.names.iter().map(|n| (n.scope.map(|i| wb.sheets[i].name.clone()), n.name.clone(), n.formula.clone())).collect();
    out.sort();
    out
}

fn local(sheet: &str, name: &str, formula: &str) -> (Option<String>, String, String) {
    (Some(sheet.into()), name.into(), formula.into())
}

/// The probe's fixture: `Data` holds `Sales` (Region, Qty, Dbl = `[@Qty]*2`, a totals row) and
/// `Keys`, with formulas using both; `Report` holds `Targets` and sums `Sales`. Workbook name
/// `Global` and `Data`-level name `Local` are both `=SUM(Sales[Qty])`.
fn book() -> Session {
    let mut s = s();
    s.execute("sheet.rename", json!({"sheet": 0, "name": "Data"})).unwrap();
    s.execute("range.setValues", json!({"range": "A1", "values": [["Region", "Qty"], ["East", 3], ["West", 5], ["East", 2], ["North", 8]]})).unwrap();
    s.execute("insert.table", json!({"range": "A1:B5", "name": "Sales"})).unwrap();
    s.execute("table.totalRow", json!({"table": "Sales", "on": true})).unwrap();
    s.execute("range.setValues", json!({"range": "F1", "values": [["K"], [1], [2]]})).unwrap();
    s.execute("insert.table", json!({"range": "F1:F3", "name": "Keys"})).unwrap();
    s.execute("cell.set", json!({"cell": "H1", "input": "=SUM(Sales[Qty])"})).unwrap();
    s.execute("cell.set", json!({"cell": "H4", "input": "=SUM(Sales[Qty])"})).unwrap();
    s.execute("cell.set", json!({"cell": "H5", "input": "=SUM(Keys[K])"})).unwrap();
    s.execute("home.insertSheet", json!({"name": "Report"})).unwrap();
    s.execute("range.setValues", json!({"sheet": "Report", "range": "A3", "values": [["Target"], [10], [20]]})).unwrap();
    s.execute("insert.table", json!({"sheet": "Report", "range": "A3:A5", "name": "Targets"})).unwrap();
    s.execute("cell.set", json!({"sheet": "Report", "cell": "A1", "input": "=SUM(Sales[Qty])"})).unwrap();
    s.execute("sheet.activate", json!({"sheet": "Data"})).unwrap();
    s.execute("formulas.defineName", json!({"name": "Global", "refersTo": "=SUM(Sales[Qty])"})).unwrap();
    s.execute("formulas.defineName", json!({"name": "Local", "refersTo": "=SUM(Sales[Qty])", "scope": "Data"})).unwrap();
    s
}

/// The probe's three copies: `Data` after itself, `Data` at the end, then `Data (2)` at the end.
fn copy(s: &mut Session, sheet: &str, to: usize) {
    s.execute("sheet.move", json!({"sheet": sheet, "to": to, "copy": true})).unwrap();
}

fn copy_three_times(s: &mut Session) {
    copy(s, "Data", 1);
    copy(s, "Data", 3);
    copy(s, "Data (2)", 4);
}

#[test]
fn a_copy_of_a_copy_numbers_from_the_original_name() {
    let mut s = book();
    copy(&mut s, "Data", 1);
    assert_eq!(sheet_names(&s), ["Data", "Data (2)", "Report"]);
    copy(&mut s, "Data", 3);
    assert_eq!(sheet_names(&s), ["Data", "Data (2)", "Report", "Data (3)"]);
    // Excel: `Data (4)`, not `Data (2) (2)`.
    copy(&mut s, "Data (2)", 4);
    assert_eq!(sheet_names(&s), ["Data", "Data (2)", "Report", "Data (3)", "Data (4)"]);
    // Table names are allocated as before (X23).
    let tables: Vec<Vec<String>> = wb(&s).sheets.iter().map(|sh| sh.tables.iter().map(|t| t.name.clone()).collect()).collect();
    assert_eq!(tables, [vec!["Sales", "Keys"], vec!["Sales5", "Keys6"], vec!["Targets"], vec!["Sales7", "Keys8"], vec!["Sales59", "Keys610"]]);
    assert_eq!(formula(&s, "Data (4)", "H4"), "SUM(Sales59[Qty])");
}

/// Choices the probe did not measure: the lowest free number from 2 (with gaps), a base that is
/// shortened so the name fits in 31 characters, and only a trailing ` (digits)` counting as a
/// copy number.
#[test]
fn copy_numbers_take_the_lowest_free_number_of_the_base() {
    let mut gaps = s();
    gaps.execute("sheet.rename", json!({"sheet": 0, "name": "Data (3)"})).unwrap();
    copy(&mut gaps, "Data (3)", 1);
    assert_eq!(sheet_names(&gaps), ["Data (3)", "Data (2)"]);
    copy(&mut gaps, "Data (3)", 2);
    assert_eq!(sheet_names(&gaps), ["Data (3)", "Data (2)", "Data (4)"]);

    let mut long_book = s();
    let long = "A".repeat(31);
    long_book.execute("sheet.rename", json!({"sheet": 0, "name": long})).unwrap();
    copy(&mut long_book, &long, 1);
    let first = format!("{} (2)", "A".repeat(27));
    assert_eq!(sheet_names(&long_book)[1], first);
    copy(&mut long_book, &first, 2);
    assert_eq!(sheet_names(&long_book)[2], format!("{} (3)", "A".repeat(27)));

    for (name, copied) in [("Q (2a)", "Q (2a) (2)"), ("Q(2)", "Q(2) (2)"), ("Q ()", "Q () (2)"), ("Q (12)", "Q (2)")] {
        let mut one = s();
        one.execute("sheet.rename", json!({"sheet": 0, "name": name})).unwrap();
        copy(&mut one, name, 1);
        assert_eq!(sheet_names(&one), [name, copied]);
    }
}

#[test]
fn each_copy_gets_sheet_level_copies_of_the_workbook_names_that_use_the_sheet() {
    let mut s = book();
    let original = names(&s);
    copy(&mut s, "Data", 1);
    // Excel: `'Data (2)'!Global` and `'Data (2)'!Local`, both `=SUM(Sales5[Qty])`.
    let mut after_first = original.clone();
    after_first.extend([local("Data (2)", "Global", "SUM(Sales5[Qty])"), local("Data (2)", "Local", "SUM(Sales5[Qty])")]);
    after_first.sort();
    assert_eq!(names(&s), after_first);
    copy(&mut s, "Data", 3);
    let mut after_second = after_first.clone();
    after_second.extend([local("Data (3)", "Global", "SUM(Sales7[Qty])"), local("Data (3)", "Local", "SUM(Sales7[Qty])")]);
    after_second.sort();
    assert_eq!(names(&s), after_second);
    // The copy of `Data (2)` gets that sheet's own `Global` (it shadows the workbook's there).
    // Excel kept both at `Sales5` while the copied cells use `Sales59`; GridCraft retargets the
    // names like the cells (X35 NEEDS DECISION: no single measured rule explains the difference).
    copy(&mut s, "Data (2)", 4);
    let mut after_third = after_second.clone();
    after_third.extend([local("Data (4)", "Global", "SUM(Sales59[Qty])"), local("Data (4)", "Local", "SUM(Sales59[Qty])")]);
    after_third.sort();
    assert_eq!(names(&s), after_third);
    // The workbook name and the original's names are unchanged.
    assert!(after_third.contains(&(None, "Global".into(), "SUM(Sales[Qty])".into())));
    assert!(after_third.contains(&local("Data", "Local", "SUM(Sales[Qty])")));
    // On the copy, `Global` now means the copy's own.
    s.execute("cell.set", json!({"sheet": "Data (2)", "cell": "B2", "input": "103"})).unwrap();
    s.execute("cell.set", json!({"sheet": "Data (2)", "cell": "J1", "input": "=Global"})).unwrap();
    s.execute("cell.set", json!({"sheet": "Data", "cell": "J1", "input": "=Global"})).unwrap();
    s.execute("sheet.activate", json!({"sheet": "Data (2)"})).unwrap();
    assert_eq!(super::v(&s, "J1"), gridcraft_core::Value::Number(118.0));
    s.execute("sheet.activate", json!({"sheet": "Data"})).unwrap();
    assert_eq!(super::v(&s, "J1"), gridcraft_core::Value::Number(18.0));
}

/// Unmeasured choices: a workbook name gets a copy only when it refers to the copied sheet (by
/// sheet name, or through a structured reference to one of its tables); `_xl…` names don't.
#[test]
fn only_workbook_names_that_use_the_copied_sheet_get_sheet_level_copies() {
    let mut s = book();
    for (name, refers) in
        [("Cell", "=Data!$A$2"), ("Other", "=Report!$A$1"), ("Bare", "=ROWS(Sales)"), ("Rate", "=0.07"), ("Targ", "=SUM(Targets[Target])")]
    {
        s.execute("formulas.defineName", json!({"name": name, "refersTo": refers})).unwrap();
    }
    std::sync::Arc::make_mut(&mut s.doc_mut().unwrap().wb).names.push(gridcraft_model::DefinedName {
        name: "_xlchart.v1.0".into(),
        scope: None,
        formula: "Data!$A$2:$A$5".into(),
        comment: String::new(),
        hidden: true,
        uid: Default::default(),
    });
    copy(&mut s, "Data", 1);
    let copies: Vec<(String, String)> = names(&s).into_iter().filter(|n| n.0.as_deref() == Some("Data (2)")).map(|n| (n.1, n.2)).collect();
    assert_eq!(
        copies,
        [
            ("Cell".to_string(), "'Data (2)'!$A$2".to_string()),
            ("Global".into(), "SUM(Sales5[Qty])".into()),
            ("Local".into(), "SUM(Sales5[Qty])".into())
        ]
    );
}

#[test]
fn undo_and_redo_of_a_copy_take_and_bring_back_its_names() {
    let mut s = book();
    copy(&mut s, "Data", 1);
    copy(&mut s, "Data", 3);
    let before = (sheet_names(&s), names(&s));
    copy(&mut s, "Data (2)", 4);
    let after = (sheet_names(&s), names(&s));
    assert!(after.1.contains(&local("Data (4)", "Global", "SUM(Sales59[Qty])")));
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!((sheet_names(&s), names(&s)), before);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(sheet_names(&s), ["Data", "Data (2)", "Report"]);
    assert!(!names(&s).iter().any(|n| n.0.as_deref() == Some("Data (3)")));
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!((sheet_names(&s), names(&s)), before);
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!((sheet_names(&s), names(&s)), after);
}

/// `(name, localSheetId, formula)` of each `<definedName>` in `xl/workbook.xml`, sorted.
fn saved_names(bytes: &[u8]) -> Vec<(String, Option<String>, String)> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut xml = String::new();
    z.by_name("xl/workbook.xml").unwrap().read_to_string(&mut xml).unwrap();
    let attr = |tag: &str, key: &str| {
        let at = tag.find(&format!(" {key}=\""))? + key.len() + 3;
        Some(tag[at..].split('"').next()?.to_string())
    };
    let mut out: Vec<_> = xml
        .split("<definedName ")
        .skip(1)
        .filter_map(|d| {
            let (attrs, rest) = d.split_once('>')?;
            // Each attribute after a space, the first one too.
            let tag = format!(" {attrs}");
            Some((attr(&tag, "name")?, attr(&tag, "localSheetId"), rest.split("</definedName>").next()?.to_string()))
        })
        .collect();
    out.sort();
    out
}

#[test]
fn sheet_level_copies_save_with_their_sheet_and_reopen() {
    let mut s = book();
    copy_three_times(&mut s);
    let made = (sheet_names(&s), names(&s));
    let (bytes, warnings) = crate::io::save_bytes_reported(wb(&s), "book.xlsx", crate::io::FileKind::Xlsx).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    // Sheet order: Data 0, Data (2) 1, Report 2, Data (3) 3, Data (4) 4.
    let sid = |i: &str| Some(i.to_string());
    let expected = vec![
        ("Global".to_string(), None, "SUM(Sales[Qty])".to_string()),
        ("Global".into(), sid("1"), "SUM(Sales5[Qty])".into()),
        ("Global".into(), sid("3"), "SUM(Sales7[Qty])".into()),
        ("Global".into(), sid("4"), "SUM(Sales59[Qty])".into()),
        ("Local".into(), sid("0"), "SUM(Sales[Qty])".into()),
        ("Local".into(), sid("1"), "SUM(Sales5[Qty])".into()),
        ("Local".into(), sid("3"), "SUM(Sales7[Qty])".into()),
        ("Local".into(), sid("4"), "SUM(Sales59[Qty])".into()),
    ];
    assert_eq!(saved_names(&bytes), expected);
    s.execute("file.open", json!({"name": "book.xlsx", "base64": crate::io::base64_encode(&bytes)})).unwrap();
    assert_eq!((sheet_names(&s), names(&s)), made);
}
