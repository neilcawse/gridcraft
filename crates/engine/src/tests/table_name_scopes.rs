//! X27: a table may take a name only while no other table, no defined name of any scope (the
//! names of kept chart, dialog and macro sheets too) and no reserved name has it, in any case
//! (ECMA-376 Part 1 §18.5.1.2). Every way a table gets a name keeps to that rule.

use std::io::{Cursor, Write};

use serde_json::{Value as Json, json};

use super::s;
use crate::Session;

const NS_MAIN: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const NS_R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const TAKEN: &str = "The name entered already exists. Enter a unique name.";

/// A workbook of worksheet "Data" (a header and two numbers in A1:A3) and dialog sheet "Panel",
/// which GridCraft keeps as it was. Three names are scoped to "Panel" (`Local`, `Table1` and a
/// hidden `table2`), so they live with the kept sheet, not among the workbook's names.
fn kept_scope_package() -> Vec<u8> {
    let ct = "application/vnd.openxmlformats-officedocument.spreadsheetml";
    let types = format!(
        r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="xml" ContentType="application/xml"/><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Override PartName="/xl/workbook.xml" ContentType="{ct}.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="{ct}.worksheet+xml"/><Override PartName="/xl/dialogsheets/sheet1.xml" ContentType="{ct}.dialogsheet+xml"/></Types>"#
    );
    let root_rels = format!(
        r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{NS_R}/officeDocument" Target="xl/workbook.xml"/></Relationships>"#
    );
    let workbook = format!(
        r#"<?xml version="1.0"?><workbook xmlns="{NS_MAIN}" xmlns:r="{NS_R}"><sheets><sheet name="Data" sheetId="1" r:id="rId1"/><sheet name="Panel" sheetId="2" r:id="rId2"/></sheets><definedNames><definedName name="Local" localSheetId="1">Data!$A$1</definedName><definedName name="Table1" localSheetId="1">Data!$A$2</definedName><definedName name="table2" localSheetId="1" hidden="1">Data!$A$3</definedName></definedNames></workbook>"#
    );
    let workbook_rels = format!(
        r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{NS_R}/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="{NS_R}/dialogsheet" Target="dialogsheets/sheet1.xml"/></Relationships>"#
    );
    let data = format!(
        r#"<?xml version="1.0"?><worksheet xmlns="{NS_MAIN}"><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Head</t></is></c></row><row r="2"><c r="A2"><v>1</v></c></row><row r="3"><c r="A3"><v>2</v></c></row></sheetData></worksheet>"#
    );
    let panel = format!(r#"<?xml version="1.0"?><dialogsheet xmlns="{NS_MAIN}"><sheetData/></dialogsheet>"#);
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, part) in [
        ("[Content_Types].xml", types),
        ("_rels/.rels", root_rels),
        ("xl/workbook.xml", workbook),
        ("xl/_rels/workbook.xml.rels", workbook_rels),
        ("xl/worksheets/sheet1.xml", data),
        ("xl/dialogsheets/sheet1.xml", panel),
    ] {
        zip.start_file(name, options).unwrap();
        zip.write_all(part.as_bytes()).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

/// [`kept_scope_package`], opened.
fn kept_scope_book() -> Session {
    let mut s = s();
    s.execute("file.open", json!({"name": "kept.xlsx", "base64": crate::io::base64_encode(&kept_scope_package())})).unwrap();
    let wb = &s.doc().unwrap().wb;
    // The premise: the names are the kept sheet's, invisible to the workbook's own names.
    assert!(wb.names.is_empty(), "{:?}", wb.names);
    assert_eq!(wb.sheets.len(), 1);
    s
}

fn tables(s: &Session) -> Vec<String> {
    s.doc().unwrap().wb.sheets.iter().flat_map(|sh| sh.tables.iter().map(|t| t.name.clone())).collect()
}

fn refused(r: crate::Result<Json>) -> String {
    r.expect_err("the name is taken").to_string()
}

/// Saving writes every table under the name it has: the XLSX repair has nothing to rename.
fn saves_without_renames(s: &mut Session) {
    let saved = s.execute("file.saveBytes", json!({})).unwrap();
    assert!(saved.get("warnings").is_none(), "{saved:?}");
}

#[test]
fn rename_refuses_a_name_scoped_to_a_kept_sheet_in_any_case() {
    let mut s = kept_scope_book();
    s.execute("insert.table", json!({"range": "A1:A3", "name": "Items"})).unwrap();
    for name in ["Local", "LOCAL", "local", "TABLE1", "Table2"] {
        assert_eq!(refused(s.execute("table.rename", json!({"table": "Items", "name": name}))), TAKEN, "{name}");
    }
    assert_eq!(tables(&s), ["Items"]);
    saves_without_renames(&mut s);
}

#[test]
fn insert_refuses_a_given_name_scoped_to_a_kept_sheet_in_any_case() {
    let mut s = kept_scope_book();
    for name in ["Local", "lOcAl", "TABLE1", "table2"] {
        assert_eq!(refused(s.execute("insert.table", json!({"range": "A1:A3", "name": name}))), TAKEN, "{name}");
        assert_eq!(refused(s.execute("home.formatAsTable", json!({"range": "A1:A3", "style": "TableStyleLight1", "name": name}))), TAKEN, "{name}");
    }
    assert!(tables(&s).is_empty());
}

/// Insert Table, Format as Table and From Text/CSV name a new table `Table<n>` with the first `n`
/// no name has: `Table1` and `table2` are names of the kept sheet.
#[test]
fn new_table_names_skip_names_scoped_to_a_kept_sheet() {
    let mut s = kept_scope_book();
    assert_eq!(s.execute("insert.table", json!({"range": "A1:A3"})).unwrap()["table"], "Table3");
    s.execute("range.setValues", json!({"range": "C1:C2", "values": [["Other"], [3]]})).unwrap();
    assert_eq!(s.execute("home.formatAsTable", json!({"range": "C1:C2", "style": "TableStyleLight1"})).unwrap()["table"], "Table4");
    s.execute("data.fromTextCsv", json!({"text": "Key,Value\na,1\nb,2\n"})).unwrap();
    assert_eq!(tables(&s), ["Table3", "Table4", "Table5"]);
    saves_without_renames(&mut s);
}

#[test]
fn a_forecast_sheets_table_skips_names_scoped_to_a_kept_sheet() {
    let mut s = kept_scope_book();
    let mut rows = vec![json!(["Month", "Sales"])];
    rows.extend((1..=12).map(|t| json!([t, 10 + 2 * t])));
    s.execute("range.setValues", json!({"range": "D1", "values": rows})).unwrap();
    let r = s.execute("data.forecastSheet", json!({"range": "D1:E13", "periods": 2})).unwrap();
    assert_eq!(r["table"], "Table3");
    saves_without_renames(&mut s);
}

/// SpreadsheetML reserves `_xlnm.` for its built-in names (ECMA-376 Part 1 §18.2.5), and the
/// XLSX writer makes `_xlchart.` names for Excel 2016 charts: no table takes either, whether the
/// workbook has such a name or not.
#[test]
fn reserved_names_are_taken() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1:A2", "values": [["h"], [1]]})).unwrap();
    s.execute("insert.table", json!({"range": "A1:A2"})).unwrap();
    for name in ["_xlnm.Print_Area", "_XLNM.Criteria", "_xlnm.Anything", "_xlchart.v1.0", "_XLChart.x"] {
        assert_eq!(refused(s.execute("table.rename", json!({"table": "Table1", "name": name}))), TAKEN, "{name}");
        assert_eq!(refused(s.execute("insert.table", json!({"range": "C1:C2", "name": name}))), TAKEN, "{name}");
    }
    assert_eq!(tables(&s), ["Table1"]);
}

/// Names are the same in any case of any script, as the XLSX save's repair compares them, so a
/// name taken that way is refused instead of being renamed on save.
#[test]
fn a_name_differing_only_in_case_beyond_ascii_is_taken() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1:C2", "values": [["h", null, "g"], [1, null, 2]]})).unwrap();
    s.execute("formulas.defineName", json!({"name": "Größe", "refersTo": "=1"})).unwrap();
    s.execute("insert.table", json!({"range": "A1:A2", "name": "Straße"})).unwrap();
    for name in ["GRÖSSE", "grösse", "STRASSE"] {
        assert_eq!(refused(s.execute("insert.table", json!({"range": "C1:C2", "name": name}))), TAKEN, "{name}");
    }
    assert_eq!(refused(s.execute("table.rename", json!({"table": "Straße", "name": "GRÖSSE"}))), TAKEN);
    assert_eq!(tables(&s), ["Straße"]);
    saves_without_renames(&mut s);
}

/// An imported slicer cache keeps its name on save, written as a workbook name when the file had
/// no marker for it: a table may not take it, and a generated name skips it.
#[test]
fn an_imported_slicer_cache_name_is_taken() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1:B3", "values": [["Region", "Quarter"], ["East", "Q1"], ["West", "Q2"]]})).unwrap();
    s.execute("insert.table", json!({"range": "A1:B3", "name": "Sales"})).unwrap();
    s.execute("insert.slicer", json!({"table": "Sales", "column": "Region"})).unwrap();
    s.execute("insert.slicer", json!({"table": "Sales", "column": "Quarter"})).unwrap();
    let sheet = std::sync::Arc::make_mut(&mut s.doc_mut().unwrap().wb).sheet_mut(0).unwrap();
    sheet.slicers[0].imported_cache.name = "Imported_Cache".into();
    sheet.slicers[1].imported_cache.name = "TABLE1".into();
    for name in ["Imported_Cache", "IMPORTED_CACHE", "table1"] {
        assert_eq!(refused(s.execute("table.rename", json!({"table": "Sales", "name": name}))), TAKEN, "{name}");
        assert_eq!(refused(s.execute("insert.table", json!({"range": "D1:D2", "name": name}))), TAKEN, "{name}");
    }
    s.execute("range.setValues", json!({"range": "D1:D2", "values": [["h"], [1]]})).unwrap();
    assert_eq!(s.execute("insert.table", json!({"range": "D1:D2"})).unwrap()["table"], "Table2");
    let saved = s.execute("file.saveBytes", json!({})).unwrap();
    assert!(saved.get("warnings").is_none(), "{saved:?}");
    let xml = workbook_xml(&saved["base64"].as_str().and_then(crate::io::base64_decode).unwrap());
    for name in [r#"<definedName name="Imported_Cache">#N/A</definedName>"#, r#"<definedName name="TABLE1">#N/A</definedName>"#] {
        assert!(xml.contains(name), "{name} in {xml}");
    }
}

/// Guard: a free name is taken, a table keeps its own name in another case, and the kept
/// sheet's names are written back as they were.
#[test]
fn a_free_name_still_works_beside_names_of_a_kept_sheet() {
    let mut s = kept_scope_book();
    s.execute("insert.table", json!({"range": "A1:A3", "name": "Locale"})).unwrap();
    s.execute("table.rename", json!({"table": "Locale", "name": "LOCALE"})).unwrap();
    s.execute("table.rename", json!({"table": "LOCALE", "name": "Table21"})).unwrap();
    s.execute("range.setValues", json!({"range": "C1:C2", "values": [["Other"], [3]]})).unwrap();
    s.execute("insert.table", json!({"range": "C1:C2", "name": "Fresh"})).unwrap();
    assert_eq!(tables(&s), ["Table21", "Fresh"]);
    let saved = s.execute("file.saveBytes", json!({})).unwrap();
    assert!(saved.get("warnings").is_none(), "{saved:?}");
    let bytes = saved["base64"].as_str().and_then(crate::io::base64_decode).unwrap();
    let (wb, warnings) = crate::io::open_bytes("kept.xlsx", &bytes).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!((wb.table("Table21"), wb.table("Fresh")), (Some((0, 0)), Some((0, 1))));
    let kept: Vec<_> = wb.preserved.iter().flat_map(|p| &p.sheets).flat_map(|sh| &sh.names).map(|n| n.formula.as_str()).collect();
    assert_eq!(kept, ["Data!$A$1", "Data!$A$2", "Data!$A$3"]);
    let xml = workbook_xml(&bytes);
    for name in [
        r#"<definedName localSheetId="1" name="Local">Data!$A$1</definedName>"#,
        r#"<definedName localSheetId="1" name="Table1">Data!$A$2</definedName>"#,
        r#"<definedName localSheetId="1" name="table2" hidden="1">Data!$A$3</definedName>"#,
    ] {
        assert!(xml.contains(name), "{name} in {xml}");
    }
}

/// The written `workbook.xml` of an XLSX package.
fn workbook_xml(bytes: &[u8]) -> String {
    use std::io::Read;
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut xml = String::new();
    zip.by_name("xl/workbook.xml").unwrap().read_to_string(&mut xml).unwrap();
    xml
}
