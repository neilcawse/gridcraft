//! A table part is written from the model, but a table can hang off more than the model holds: a
//! table showing an external data range has a query table behind it, whose fields name the table's
//! columns by the numbers the file gave them. Those parts and numbers are kept, as long as the
//! columns they refer to are there.

use gridcraft_core::RangeRef;
use gridcraft_model::TableColumn;

use super::*;

const QUERY_TABLE: &str = r#"<?xml version="1.0"?><queryTable xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" name="ExternalData_1" connectionId="1" autoFormatId="16" applyNumberFormats="0" applyBorderFormats="0" applyFontFormats="1" applyPatternFormats="1" applyAlignmentFormats="0" applyWidthHeightFormats="0"><queryTableRefresh nextId="4"><queryTableFields count="3"><queryTableField id="1" name="Level" tableColumnId="3"/><queryTableField id="2" name="Number" tableColumnId="7"/><queryTableField id="3" name="Revision" tableColumnId="9"/></queryTableFields></queryTableRefresh></queryTable>"#;
const TABLE: &str = r#"<?xml version="1.0"?><table xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" id="5" name="Results" displayName="Results" ref="A1:C3" tableType="queryTable" totalsRowShown="0"><autoFilter ref="A1:C3"/><tableColumns count="3"><tableColumn id="3" name="Level" uniqueName="1" queryTableFieldId="1" dataDxfId="0"/><tableColumn id="7" name="Number" uniqueName="2" queryTableFieldId="2"/><tableColumn id="9" name="Revision" uniqueName="3" queryTableFieldId="3"/></tableColumns><tableStyleInfo name="TableStyleMedium2" showFirstColumn="0" showLastColumn="0" showRowStripes="1" showColumnStripes="0"/></table>"#;
const DATA_CONNECTIONS: &str = r#"<?xml version="1.0"?><connections xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><connection id="1" name="Source" type="6" refreshedVersion="6"><textPr sourceFile="C:\data\results.csv"/></connection></connections>"#;

/// Sheet "Data" with the table "Results" (A1:C3, id 5) bound to a query table through the
/// connection of the workbook. The columns are numbered 3, 7 and 9, not 1, 2 and 3. `query_part`
/// leaves the query table part out when false.
fn package_with(query_part: bool) -> Vec<u8> {
    let types = format!(
        r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="xml" ContentType="application/xml"/><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Override PartName="/xl/workbook.xml" ContentType="{CT_PREFIX}.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="{CT_PREFIX}.spreadsheetml.worksheet+xml"/><Override PartName="/xl/tables/table1.xml" ContentType="{CT_PREFIX}.spreadsheetml.table+xml"/><Override PartName="/xl/queryTables/queryTable1.xml" ContentType="{CT_PREFIX}.spreadsheetml.queryTable+xml"/><Override PartName="/xl/connections.xml" ContentType="{CT_PREFIX}.spreadsheetml.connections+xml"/></Types>"#
    );
    let workbook = format!(
        r#"<?xml version="1.0"?><workbook xmlns="{NS_MAIN}" xmlns:r="{NS_R}"><sheets><sheet name="Data" sheetId="1" r:id="rId1"/></sheets></workbook>"#
    );
    let cell = |r: &str, t: &str| format!(r#"<c r="{r}" t="inlineStr"><is><t>{t}</t></is></c>"#);
    let sheet = format!(
        r#"<?xml version="1.0"?><worksheet xmlns="{NS_MAIN}" xmlns:r="{NS_R}"><sheetData><row r="1">{}{}{}</row><row r="2">{}{}{}</row></sheetData><pageMargins left="0.7" right="0.7" top="0.75" bottom="0.75" header="0.3" footer="0.3"/><tableParts count="1"><tablePart r:id="rId1"/></tableParts></worksheet>"#,
        cell("A1", "Level"),
        cell("B1", "Number"),
        cell("C1", "Revision"),
        cell("A2", "1"),
        cell("B2", "2"),
        cell("C2", "3"),
    );
    let root_rels = rels(&[("rId1", "officeDocument", "xl/workbook.xml")]);
    let wb_rels = rels(&[("rId1", "worksheet", "worksheets/sheet1.xml"), ("rId2", "connections", "connections.xml")]);
    let sheet_rels = rels(&[("rId1", "table", "../tables/table1.xml")]);
    let table_rels = rels(&[("rId1", "queryTable", "../queryTables/queryTable1.xml")]);
    let mut parts: Vec<(&str, &[u8])> = vec![
        ("[Content_Types].xml", types.as_bytes()),
        ("_rels/.rels", root_rels.as_bytes()),
        ("xl/workbook.xml", workbook.as_bytes()),
        ("xl/_rels/workbook.xml.rels", wb_rels.as_bytes()),
        ("xl/connections.xml", DATA_CONNECTIONS.as_bytes()),
        ("xl/worksheets/sheet1.xml", sheet.as_bytes()),
        ("xl/worksheets/_rels/sheet1.xml.rels", sheet_rels.as_bytes()),
        ("xl/tables/table1.xml", TABLE.as_bytes()),
        ("xl/tables/_rels/table1.xml.rels", table_rels.as_bytes()),
    ];
    if query_part {
        parts.push(("xl/queryTables/queryTable1.xml", QUERY_TABLE.as_bytes()));
    }
    make_zip(&parts)
}

fn package() -> Vec<u8> {
    package_with(true)
}

/// A table column as written: (id, name, queryTableFieldId).
type Column = (String, String, Option<String>);

/// The written table part of sheet "Data": its name, its attributes and its columns.
fn written_table(u: &Unpacked) -> (String, xml::El, Vec<Column>) {
    let sheet = sheet_part(u, "Data");
    let part = u.rels(&sheet).into_iter().find(|r| r.1.ends_with("/table")).expect("the sheet has a table").2;
    let x = xml::parse(u.parts.get(&part).unwrap()).unwrap();
    let cols = x
        .child("tableColumns")
        .unwrap()
        .kids("tableColumn")
        .map(|c| (c.attr("id").unwrap().to_string(), c.attr("name").unwrap().to_string(), c.attr("queryTableFieldId").map(str::to_string)))
        .collect();
    (part, x, cols)
}

fn query_tables(u: &Unpacked) -> Vec<&String> {
    u.find(QUERY_TABLE.as_bytes())
}

fn col(id: &str, name: &str, field: Option<&str>) -> Column {
    (id.to_string(), name.to_string(), field.map(str::to_string))
}

fn no_loss(warnings: &[String]) -> bool {
    !warnings.iter().any(|w| w.contains("skipped") || w.contains("dropped"))
}

#[test]
fn the_query_table_behind_a_table_survives_a_round_trip() {
    let (wb, rep) = read_xlsx(&package()).unwrap();
    assert!(no_loss(&rep.warnings), "{:?}", rep.warnings);
    let out = write_xlsx(&wb).unwrap();
    let u = Unpacked::new(&out);
    u.assert_valid();

    let (table, x, cols) = written_table(&u);
    // The table says what it is, and its columns keep the numbers the query table's fields name.
    assert_eq!(x.attr("tableType"), Some("queryTable"));
    assert_eq!(cols, [col("3", "Level", Some("1")), col("7", "Number", Some("2")), col("9", "Revision", Some("3"))]);
    // The relationship from the table part to the query table part, and the part itself.
    let rels = u.rels(&table);
    assert_eq!(rels.len(), 1, "{rels:?}");
    assert!(rels[0].1.ends_with("/queryTable"), "{rels:?}");
    assert_eq!(u.parts.get(&rels[0].2).unwrap(), QUERY_TABLE.as_bytes());
    assert_eq!(rels[0].2, "xl/queryTables/queryTable1.xml");
    // The connection the query table names is kept too.
    assert!(u.rels("xl/workbook.xml").iter().any(|r| r.1.ends_with("/connections") && u.parts.get(&r.2).unwrap() == DATA_CONNECTIONS.as_bytes()));

    // A second save writes the same thing.
    let (wb2, rep2) = read_xlsx(&out).unwrap();
    assert!(no_loss(&rep2.warnings), "{:?}", rep2.warnings);
    let again = Unpacked::new(&write_xlsx(&wb2).unwrap());
    again.assert_valid();
    let (_, _, cols2) = written_table(&again);
    assert_eq!(cols2, cols);
    assert_eq!(query_tables(&again).len(), 1);
}

fn edit_table(wb: &mut gridcraft_model::Workbook, f: impl FnOnce(&mut gridcraft_model::Table)) {
    f(&mut std::sync::Arc::make_mut(&mut wb.sheets[0]).tables[0]);
}

fn column(name: &str) -> TableColumn {
    TableColumn { name: name.into(), totals: Default::default(), totals_label: None, formula: None, unique_name: None, xml: None }
}

#[test]
fn a_renamed_column_keeps_its_place_in_the_query_table() {
    let (mut wb, _) = read_xlsx(&package()).unwrap();
    edit_table(&mut wb, |t| t.columns[1].name = "Count".into());
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    u.assert_valid();
    let (table, x, cols) = written_table(&u);
    assert_eq!(x.attr("tableType"), Some("queryTable"));
    assert_eq!(cols, [col("3", "Level", Some("1")), col("7", "Count", Some("2")), col("9", "Revision", Some("3"))]);
    assert_eq!(u.rels(&table).len(), 1);
    assert_eq!(query_tables(&u).len(), 1);
}

#[test]
fn a_new_column_gets_a_number_of_its_own() {
    let (mut wb, _) = read_xlsx(&package()).unwrap();
    edit_table(&mut wb, |t| {
        t.columns.insert(0, column("Added"));
        t.range = RangeRef::parse("A1:D3").unwrap();
    });
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    u.assert_valid();
    let (table, x, cols) = written_table(&u);
    assert_eq!(x.attr("tableType"), Some("queryTable"));
    // The fields still name the columns they named: the new one is beyond the highest number.
    assert_eq!(cols, [col("10", "Added", None), col("3", "Level", Some("1")), col("7", "Number", Some("2")), col("9", "Revision", Some("3"))]);
    assert_eq!(u.rels(&table).len(), 1);
    assert_eq!(query_tables(&u).len(), 1);
}

#[test]
fn a_table_that_lost_a_column_of_its_query_is_written_without_the_query() {
    let (mut wb, _) = read_xlsx(&package()).unwrap();
    edit_table(&mut wb, |t| {
        t.columns.remove(1);
        t.range = RangeRef::parse("A1:B3").unwrap();
    });
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    u.assert_valid();
    let (table, x, cols) = written_table(&u);
    // The query table would name a column that isn't there: the table is a plain one.
    assert_eq!(x.attr("tableType"), None);
    assert_eq!(cols, [col("1", "Level", None), col("2", "Revision", None)]);
    assert!(u.rels(&table).is_empty());
    assert!(query_tables(&u).is_empty(), "{:?}", u.parts.keys());
}

#[test]
fn a_query_table_that_cannot_be_kept_leaves_a_plain_table_and_a_warning() {
    let (wb, rep) = read_xlsx(&package_with(false)).unwrap();
    let said: Vec<&String> = rep.warnings.iter().filter(|w| w.contains("queryTable of xl/tables/table1.xml was dropped")).collect();
    assert_eq!(said.len(), 1, "{:?}", rep.warnings);
    assert!(said[0].contains("queryTable1.xml is missing"), "{said:?}");
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    u.assert_valid();
    let (table, x, cols) = written_table(&u);
    assert_eq!(x.attr("tableType"), None);
    assert_eq!(cols, [col("1", "Level", None), col("2", "Number", None), col("3", "Revision", None)]);
    assert!(u.rels(&table).is_empty());
}
