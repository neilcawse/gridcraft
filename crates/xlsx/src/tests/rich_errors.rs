//! `#SPILL!` and `#CALC!` are saved as `#VALUE!` plus a `vm` that points at an `_error` rich value.

use gridcraft_core::{CellError, CellRef, Value};

use crate::read_xlsx;

fn at(s: &str) -> CellRef {
    CellRef::parse(s).unwrap()
}

const RELS: &str = concat!(
    r#"<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sheetMetadata" Target="metadata.xml"/>"#,
    r#"<Relationship Id="rId3" Type="http://schemas.microsoft.com/office/2017/06/relationships/rdRichValue" Target="richData/rdrichvalue.xml"/>"#,
    r#"<Relationship Id="rId4" Type="http://schemas.microsoft.com/office/2017/06/relationships/rdRichValueStructure" Target="richData/rdrichvaluestructure.xml"/>"#,
);

/// Rich values: 0 is `#CALC!` (errorType 13), 1 is `#SPILL!` (8), 2 an error type nothing here
/// knows (4), 3 a different structure. The value-metadata blocks (`vm`) point at them out of
/// order, so `vm` is not an index into the rich values.
fn parts(structures: &str) -> Vec<(&'static str, String)> {
    vec![
        (
            "xl/metadata.xml",
            concat!(
                r#"<metadata xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:xlrd="http://schemas.microsoft.com/office/spreadsheetml/2017/richdata">"#,
                r#"<metadataTypes count="2"><metadataType name="XLDAPR" cellMeta="1"/><metadataType name="XLRICHVALUE"/></metadataTypes>"#,
                r#"<futureMetadata name="XLRICHVALUE" count="4">"#,
                r#"<bk><extLst><ext uri="{3e2802c4-a4d2-4d8b-9148-e3be6c30e623}"><xlrd:rvb i="3"/></ext></extLst></bk>"#,
                r#"<bk><extLst><ext uri="{3e2802c4-a4d2-4d8b-9148-e3be6c30e623}"><xlrd:rvb i="1"/></ext></extLst></bk>"#,
                r#"<bk><extLst><ext uri="{3e2802c4-a4d2-4d8b-9148-e3be6c30e623}"><xlrd:rvb i="0"/></ext></extLst></bk>"#,
                r#"<bk><extLst><ext uri="{3e2802c4-a4d2-4d8b-9148-e3be6c30e623}"><xlrd:rvb i="2"/></ext></extLst></bk>"#,
                r#"</futureMetadata>"#,
                r#"<valueMetadata count="4"><bk><rc t="2" v="0"/></bk><bk><rc t="2" v="1"/></bk><bk><rc t="2" v="2"/></bk><bk><rc t="2" v="3"/></bk></valueMetadata>"#,
                r#"</metadata>"#
            )
            .to_string(),
        ),
        (
            "xl/richData/rdrichvalue.xml",
            r#"<rvData xmlns="http://schemas.microsoft.com/office/spreadsheetml/2017/richdata" count="4"><rv s="0"><v>13</v><v>3</v></rv><rv s="0"><v>8</v><v>2</v></rv><rv s="0"><v>4</v><v>1</v></rv><rv s="1"><v>13</v></rv></rvData>"#.to_string(),
        ),
        ("xl/richData/rdrichvaluestructure.xml", structures.to_string()),
    ]
}

fn read(structures: &str, body: &str) -> gridcraft_model::Workbook {
    let extra = parts(structures);
    let extra: Vec<(&str, &str)> = extra.iter().map(|(n, c)| (*n, c.as_str())).collect();
    read_xlsx(&super::minimal(body, &extra, RELS, "")).unwrap().0
}

fn value(wb: &gridcraft_model::Workbook, cell: &str) -> Value {
    wb.sheets[0].cell(at(cell)).unwrap().value.clone()
}

#[test]
fn error_cells_with_an_error_rich_value_are_that_error() {
    let structures = r#"<rvStructures xmlns="http://schemas.microsoft.com/office/spreadsheetml/2017/richdata" count="2"><s t="_error"><k n="errorType" t="i"/><k n="subType" t="i"/></s><s t="_other"><k n="errorType" t="i"/></s></rvStructures>"#;
    let body = concat!(
        "<sheetData><row r=\"1\">",
        // vm 1 -> metadata block 0 -> rich value 3 (not an error structure); vm 2 -> value 1, #SPILL!;
        // vm 3 -> value 0, #CALC!; vm 4 -> value 2, an error type that is kept as saved.
        r#"<c r="A1" t="e" vm="1"><v>#VALUE!</v></c>"#,
        r#"<c r="B1" t="e" vm="2"><v>#VALUE!</v></c>"#,
        r#"<c r="C1" t="e" vm="3"><v>#VALUE!</v></c>"#,
        r#"<c r="D1" t="e" vm="4"><v>#VALUE!</v></c>"#,
        // A formula cell, as FILTER with nothing to return is saved; a plain error; a vm that points nowhere.
        r#"<c r="E1" t="e" vm="3" cm="1"><f t="array" ref="E1">_xlfn._xlws.FILTER(A2:A3,A2:A3&gt;10)</f><v>#VALUE!</v></c>"#,
        r#"<c r="F1" t="e"><v>#N/A</v></c>"#,
        r#"<c r="G1" t="e" vm="9"><v>#VALUE!</v></c>"#,
        "</row></sheetData>"
    );
    let wb = read(structures, body);
    let got: Vec<Value> = ["A1", "B1", "C1", "D1", "E1", "F1", "G1"].iter().map(|c| value(&wb, c)).collect();
    assert_eq!(
        got,
        [
            Value::Error(CellError::Value),
            Value::Error(CellError::Spill),
            Value::Error(CellError::Calc),
            Value::Error(CellError::Value),
            Value::Error(CellError::Calc),
            Value::Error(CellError::NA),
            Value::Error(CellError::Value),
        ]
    );
}

#[test]
fn a_workbook_without_error_structures_keeps_the_saved_error() {
    let structures = r#"<rvStructures xmlns="http://schemas.microsoft.com/office/spreadsheetml/2017/richdata" count="1"><s t="_other"><k n="x" t="i"/></s></rvStructures>"#;
    let wb = read(structures, r#"<sheetData><row r="1"><c r="A1" t="e" vm="3"><v>#VALUE!</v></c></row></sheetData>"#);
    assert_eq!(value(&wb, "A1"), Value::Error(CellError::Value));
}
