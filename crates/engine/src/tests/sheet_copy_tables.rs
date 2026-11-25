//! Move or Copy > Create a copy on a sheet with tables: the copy has tables of its own, under new
//! names, and its own references and slicers use them.

use gridcraft_core::CellRef;
use gridcraft_model::{SlicerSource, Workbook};
use serde_json::json;

use super::s;
use crate::Session;

fn wb(s: &Session) -> &Workbook {
    &s.doc().unwrap().wb
}

fn formula(s: &Session, sheet: &str, a1: &str) -> String {
    let wb = wb(s);
    let si = wb.sheet_index(sheet).unwrap_or_else(|| panic!("no sheet {sheet}"));
    wb.sheets[si].cell(CellRef::parse(a1).unwrap()).and_then(|c| c.formula.clone()).map(|f| f.text.clone()).unwrap_or_default()
}

fn name_formula(s: &Session, name: &str, scope: Option<&str>) -> String {
    let wb = wb(s);
    let scope = scope.map(|sc| wb.sheet_index(sc).unwrap());
    wb.names.iter().find(|n| n.name == name && n.scope == scope).map(|n| n.formula.clone()).unwrap_or_default()
}

fn tables(s: &Session, sheet: &str) -> Vec<String> {
    let wb = wb(s);
    wb.sheet_index(sheet).map(|si| wb.sheets[si].tables.iter().map(|t| t.name.clone()).collect()).unwrap_or_default()
}

/// (table, column) of each table slicer on `sheet`.
fn slicers(s: &Session, sheet: &str) -> Vec<(String, String)> {
    let wb = wb(s);
    let Some(si) = wb.sheet_index(sheet) else { return vec![] };
    wb.sheets[si]
        .slicers
        .iter()
        .filter_map(|sl| match &sl.source {
            SlicerSource::Table { table, column, .. } => Some((table.clone(), column.clone())),
            SlicerSource::Pivot { .. } => None,
        })
        .collect()
}

fn filtered_rows(s: &Session, sheet: &str) -> Vec<u32> {
    let wb = wb(s);
    let si = wb.sheet_index(sheet).unwrap();
    (0..8).filter(|&r| wb.sheets[si].is_row_filtered(r)).collect()
}

/// `Data` holds table `Sales` (Region, Qty; a totals row) with a Region slicer, formulas using
/// it and a table on `Report`, and a sheet-level name using it. `Report` holds table `Targets`
/// and a formula using `Sales`; a workbook-level name uses `Sales` too.
fn book() -> Session {
    let mut s = s();
    s.execute("sheet.rename", json!({"sheet": 0, "name": "Data"})).unwrap();
    s.execute("range.setValues", json!({"range": "A1", "values": [["Region", "Qty"], ["East", 3], ["West", 5], ["East", 2], ["North", 8]]})).unwrap();
    s.execute("insert.table", json!({"range": "A1:B5", "name": "Sales"})).unwrap();
    s.execute("table.totalRow", json!({"table": "Sales", "on": true})).unwrap();
    s.execute("home.insertSheet", json!({"name": "Report"})).unwrap();
    s.execute("range.setValues", json!({"sheet": "Report", "range": "A3", "values": [["Target"], [10], [20]]})).unwrap();
    s.execute("insert.table", json!({"sheet": "Report", "range": "A3:A5", "name": "Targets"})).unwrap();
    s.execute("cell.set", json!({"sheet": "Report", "cell": "A1", "input": "=SUM(Sales[Qty])"})).unwrap();
    s.execute("sheet.activate", json!({"sheet": "Data"})).unwrap();
    s.execute("cell.set", json!({"cell": "D1", "input": "=SUM(Sales[Qty])"})).unwrap();
    s.execute("cell.set", json!({"cell": "D2", "input": "=ROWS(Sales)"})).unwrap();
    s.execute("cell.set", json!({"cell": "D3", "input": "=ROWS(Targets)"})).unwrap();
    s.execute("formulas.defineName", json!({"name": "Local", "refersTo": "=SUM(Sales[Qty])", "scope": "Data"})).unwrap();
    s.execute("formulas.defineName", json!({"name": "Global", "refersTo": "=SUM(Sales[Qty])"})).unwrap();
    s.execute("insert.slicer", json!({"table": "Sales", "column": "Region"})).unwrap();
    // A calculated column formula naming its own table.
    std::sync::Arc::make_mut(&mut s.doc_mut().unwrap().wb).sheet_mut(0).unwrap().tables[0].columns[1].formula = Some("Sales[@Qty]*1".into());
    s
}

fn copy_data(s: &mut Session) {
    s.execute("sheet.move", json!({"sheet": "Data", "to": 1, "copy": true})).unwrap();
}

#[test]
fn a_copied_sheet_has_its_tables_under_new_unique_names() {
    let mut s = book();
    copy_data(&mut s);
    // Excel names a copy `<name><n>`, `n` one more than the copy's table id. Two tables before
    // the copy: the copy takes id 3 and the number 4.
    assert_eq!(tables(&s, "Data"), ["Sales"]);
    assert_eq!(tables(&s, "Data (2)"), ["Sales4"]);
    let wb = wb(&s);
    let (orig, copy) = (&wb.sheets[0].tables[0], &wb.sheets[1].tables[0]);
    assert_eq!((copy.range, copy.totals_row, &copy.style, &copy.columns.len()), (orig.range, orig.totals_row, &orig.style, &orig.columns.len()));
    assert_ne!(copy.id, orig.id, "a table id of its own");
    let mut ids: Vec<u32> = wb.sheets.iter().flat_map(|sh| sh.tables.iter().map(|t| t.id)).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 3);
    // Copying again counts the copy: three tables, so number 5; a copy of the copy builds on its name.
    s.execute("sheet.move", json!({"sheet": "Data", "to": 2, "copy": true})).unwrap();
    assert_eq!(tables(&s, "Data (3)"), ["Sales5"]);
    s.execute("sheet.move", json!({"sheet": "Data (2)", "to": 3, "copy": true})).unwrap();
    assert_eq!(tables(&s, "Data (2) (2)"), ["Sales46"]);
}

#[test]
fn a_copied_table_name_skips_names_already_taken() {
    let mut s = book();
    // A defined name of any scope takes the name, as a table does.
    s.execute("formulas.defineName", json!({"name": "Sales4", "refersTo": "=1"})).unwrap();
    copy_data(&mut s);
    assert_eq!(tables(&s, "Data (2)"), ["Sales5"]);
    let mut s = book();
    s.execute("insert.table", json!({"sheet": "Report", "range": "C3:C5", "name": "sales5"})).unwrap();
    // Three tables: the number would be 5, taken by a table whatever its case.
    copy_data(&mut s);
    assert_eq!(tables(&s, "Data (2)"), ["Sales6"]);
    // Several tables on one sheet number in order.
    let mut s = book();
    s.execute("range.setValues", json!({"range": "F1", "values": [["K"], [1], [2]]})).unwrap();
    s.execute("insert.table", json!({"range": "F1:F3", "name": "Keys"})).unwrap();
    copy_data(&mut s);
    assert_eq!(tables(&s, "Data (2)"), ["Sales5", "Keys6"]);
}

#[test]
fn a_copied_sheets_own_references_use_its_tables() {
    let mut s = book();
    let totals = formula(&s, "Data", "B6");
    assert!(totals.contains("Sales[Qty]"), "the totals row sums the table: {totals}");
    copy_data(&mut s);
    // The copy's references to its own tables are rewritten (Excel-saved files: a copy's totals
    // row `SUBTOTAL(109,Table69[Sales])`); references to other sheets' tables are not.
    assert_eq!(formula(&s, "Data (2)", "B6"), totals.replace("Sales[", "Sales4["));
    assert_eq!(formula(&s, "Data (2)", "D1"), "SUM(Sales4[Qty])");
    assert_eq!(formula(&s, "Data (2)", "D2"), "ROWS(Sales4)");
    assert_eq!(formula(&s, "Data (2)", "D3"), "ROWS(Targets)");
    assert_eq!(wb(&s).sheets[1].tables[0].columns[1].formula.as_deref(), Some("Sales4[@Qty]*1"));
    assert_eq!(name_formula(&s, "Local", Some("Data (2)")), "SUM(Sales4[Qty])");
    // The original and the other sheets keep the original table.
    assert_eq!(formula(&s, "Data", "B6"), totals);
    assert_eq!(formula(&s, "Data", "D1"), "SUM(Sales[Qty])");
    assert_eq!(formula(&s, "Data", "D2"), "ROWS(Sales)");
    assert_eq!(wb(&s).sheets[0].tables[0].columns[1].formula.as_deref(), Some("Sales[@Qty]*1"));
    assert_eq!(formula(&s, "Report", "A1"), "SUM(Sales[Qty])");
    assert_eq!(name_formula(&s, "Local", Some("Data")), "SUM(Sales[Qty])");
    assert_eq!(name_formula(&s, "Global", None), "SUM(Sales[Qty])");
    // The copy's values come from its own table.
    s.execute("cell.set", json!({"sheet": "Data (2)", "cell": "B2", "input": "100"})).unwrap();
    s.execute("sheet.activate", json!({"sheet": "Data (2)"})).unwrap();
    assert_eq!(super::v(&s, "D1"), gridcraft_core::Value::Number(115.0));
    s.execute("sheet.activate", json!({"sheet": "Data"})).unwrap();
    assert_eq!(super::v(&s, "D1"), gridcraft_core::Value::Number(18.0));
}

#[test]
fn a_copied_slicer_filters_the_copied_table() {
    let mut s = book();
    // What the original's slicer hides when it shows East, before any copy.
    let original = wb(&s).sheets[0].slicers[0].id;
    s.execute("slicer.toggle", json!({"slicer": original, "value": "East", "multi": false})).unwrap();
    let east = filtered_rows(&s, "Data");
    assert!(east.contains(&2) && east.contains(&4) && !east.contains(&1) && !east.contains(&3), "{east:?}");
    s.execute("slicer.selectAll", json!({"slicer": original})).unwrap();
    assert!(filtered_rows(&s, "Data").is_empty());
    copy_data(&mut s);
    assert_eq!(slicers(&s, "Data (2)"), [("Sales4".to_string(), "Region".to_string())]);
    assert_eq!(slicers(&s, "Data"), [("Sales".to_string(), "Region".to_string())]);
    let wb = wb(&s);
    assert_ne!(wb.sheets[1].slicers[0].id, wb.sheets[0].slicers[0].id);
    let id = wb.sheets[1].slicers[0].id;
    // The copy is the active sheet; its slicer filters its rows, not the original's.
    s.execute("slicer.toggle", json!({"slicer": id, "value": "East", "multi": false})).unwrap();
    assert_eq!(filtered_rows(&s, "Data (2)"), east);
    assert!(filtered_rows(&s, "Data").is_empty(), "the original is untouched");
    s.execute("slicer.selectAll", json!({"slicer": id})).unwrap();
    assert!(filtered_rows(&s, "Data (2)").is_empty());
}

#[test]
fn copying_a_sheet_with_tables_is_one_undoable_edit() {
    let mut s = book();
    let before = (*wb(&s)).clone();
    copy_data(&mut s);
    let after = (*wb(&s)).clone();
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(wb(&s).sheets, before.sheets);
    assert_eq!(wb(&s).names, before.names);
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(wb(&s).sheets, after.sheets);
    assert_eq!(wb(&s).names, after.names);
    assert_eq!(tables(&s, "Data (2)"), ["Sales4"]);
    assert_eq!(slicers(&s, "Data (2)"), [("Sales4".to_string(), "Region".to_string())]);
}

/// What a saved and reopened copy must keep: each sheet's tables and ranges, table slicers and
/// formulas, and the names.
type Summary = (Vec<(String, Vec<(String, String)>, Vec<(String, String)>, Vec<String>)>, Vec<(String, Option<usize>, String)>);

fn summary(s: &Session) -> Summary {
    let wb = wb(s);
    let sheets = wb
        .sheets
        .iter()
        .map(|sh| {
            (
                sh.name.clone(),
                sh.tables.iter().map(|t| (t.name.clone(), t.range.a1())).collect(),
                slicers(s, &sh.name),
                ["A1", "B6", "D1", "D2", "D3"].iter().map(|a| formula(s, &sh.name, a)).collect(),
            )
        })
        .collect();
    let mut names: Vec<_> = wb.names.iter().map(|n| (n.name.clone(), n.scope, n.formula.clone())).collect();
    names.sort();
    (sheets, names)
}

fn save_and_reopen(s: &mut Session) -> Vec<String> {
    let (bytes, warnings) = crate::io::save_bytes_reported(wb(s), "book.xlsx", crate::io::FileKind::Xlsx).unwrap();
    s.execute("file.open", json!({"name": "book.xlsx", "base64": crate::io::base64_encode(&bytes)})).unwrap();
    warnings
}

#[test]
fn a_copied_sheet_saves_reopens_and_saves_again_with_the_same_bindings() {
    let mut s = book();
    copy_data(&mut s);
    let (made_sheets, mut made_names) = summary(&s);
    let warnings = save_and_reopen(&mut s);
    assert!(warnings.is_empty(), "{warnings:?}");
    let reopened = summary(&s);
    assert_eq!(reopened.0, made_sheets);
    // Each slicer has a cache of its own, named by X19's planner; the file adds their markers.
    made_names.extend([("Slicer_Region".to_string(), None, "#N/A".to_string()), ("Slicer_Region2".to_string(), None, "#N/A".to_string())]);
    made_names.sort();
    assert_eq!(reopened.1, made_names);
    let caches: Vec<String> = wb(&s).sheets[..2].iter().map(|sh| sh.slicers[0].imported_cache.name.clone()).collect();
    assert_eq!(caches, ["Slicer_Region", "Slicer_Region2"]);
    let warnings = save_and_reopen(&mut s);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(summary(&s), reopened);
    let caches_again: Vec<String> = wb(&s).sheets[..2].iter().map(|sh| sh.slicers[0].imported_cache.name.clone()).collect();
    assert_eq!(caches_again, caches);
    // The reopened copy's slicer still filters the copy.
    s.execute("sheet.activate", json!({"sheet": "Data (2)"})).unwrap();
    let id = wb(&s).sheets[1].slicers[0].id;
    s.execute("slicer.toggle", json!({"slicer": id, "value": "West", "multi": false})).unwrap();
    let west = filtered_rows(&s, "Data (2)");
    assert!(west.contains(&1) && west.contains(&3) && west.contains(&4) && !west.contains(&2), "{west:?}");
    assert!(filtered_rows(&s, "Data").is_empty());
}
