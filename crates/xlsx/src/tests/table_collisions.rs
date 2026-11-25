//! Synthetic fixtures: malformed import and direct-model export exercise separate boundaries.

use std::io::{Cursor, Read};
use std::sync::Arc;

use gridcraft_core::{CellRef, RangeRef, Value};
use gridcraft_model::*;

use super::{CT, ROOT_RELS, make_zip};
use crate::{read_xlsx, write_xlsx};

#[test]
fn malformed_tables_keep_their_reference_identity_through_three_saves() -> Result<(), Box<dyn std::error::Error>> {
    let book = r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="One" sheetId="1" r:id="rId1"/><sheet name="Two" sheetId="2" r:id="rId2"/></sheets><definedNames><definedName name="ITEMS">One!$H$1</definedName><definedName name="DataBody">Items[Value]</definedName></definedNames></workbook>"#;
    let book_rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet2.xml"/></Relationships>"#;
    let sheet = |rows: &str, value: u32| {
        format!(
            r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Value</t></is></c></row><row r="2"><c r="A2"><v>{value}</v></c></row>{rows}</sheetData><tableParts count="1"><tablePart r:id="rId1"/></tableParts></worksheet>"#
        )
    };
    let one = sheet(
        r#"<row r="4"><c r="B4"><f t="shared" si="0" ref="B4:B5">SUM(Items[Value])+A4</f><v>3</v></c><c r="C4"><f t="array" ref="C4:D4">TRANSPOSE(Items[Value])</f><v>2</v></c><c r="D4"><v>7</v></c></row><row r="5"><c r="B5"><f t="shared" si="0"/><v>4</v></c></row>"#,
        2,
    );
    let two = sheet(r#"<row r="4"><c r="B4"><f>SUM(ITEMS[Value])+ITEMS</f><v>2</v></c></row>"#, 19);
    let rels = |id: u32| {
        format!(
            r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/table" Target="../tables/table{id}.xml"/></Relationships>"#
        )
    };
    let table = |id: u32, n: &str| {
        format!(
            r#"<table xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" id="{id}" name="{n}" displayName="{n}" ref="A1:A2"><tableColumns count="1"><tableColumn id="1" name="Value"><calculatedColumnFormula>{n}[Value]</calculatedColumnFormula></tableColumn></tableColumns></table>"#
        )
    };
    let malformed = make_zip(&[
        ("[Content_Types].xml", CT.as_bytes()),
        ("_rels/.rels", ROOT_RELS.as_bytes()),
        ("xl/workbook.xml", book.as_bytes()),
        ("xl/_rels/workbook.xml.rels", book_rels.as_bytes()),
        ("xl/worksheets/sheet1.xml", one.as_bytes()),
        ("xl/worksheets/sheet2.xml", two.as_bytes()),
        ("xl/worksheets/_rels/sheet1.xml.rels", rels(1).as_bytes()),
        ("xl/worksheets/_rels/sheet2.xml.rels", rels(2).as_bytes()),
        ("xl/tables/table1.xml", table(1, "Items").as_bytes()),
        ("xl/tables/table2.xml", table(2, "ITEMS").as_bytes()),
    ]);
    let (mut wb, report) = read_xlsx(&malformed)?;
    let renames = |warnings: &[String]| warnings.iter().filter(|w| w.starts_with("Table ") && w.contains("was renamed")).cloned().collect::<Vec<_>>();
    assert_eq!(
        renames(&report.warnings),
        [r#"Table "Items" on sheet "One" was renamed to "Items_2"."#, r#"Table "ITEMS" on sheet "Two" was renamed to "ITEMS_3"."#,]
    );
    for cycle in 0..=3 {
        let a = &wb.sheets[0];
        let b = &wb.sheets[1];
        assert_eq!(a.tables[0].name, "Items_2");
        assert_eq!(b.tables[0].name, "ITEMS_3");
        assert_eq!(a.tables[0].columns[0].formula.as_deref(), Some("Items_2[Value]"));
        assert_eq!(b.tables[0].columns[0].formula.as_deref(), Some("ITEMS_3[Value]"));
        assert_eq!(wb.table("Items_2"), Some((0, 0)));
        assert_eq!(wb.name("items", 1).map(|n| n.formula.as_str()), Some("One!$H$1"));
        assert_eq!(wb.name("DataBody", 0).map(|n| n.formula.as_str()), Some("Items_2[Value]"));
        for (sheet, at, formula, cache) in [
            (a, CellRef::new(3, 1), "SUM(Items_2[Value])+A4", 3.0),
            (a, CellRef::new(4, 1), "SUM(Items_2[Value])+A5", 4.0),
            (a, CellRef::new(3, 2), "TRANSPOSE(Items_2[Value])", 2.0),
            (b, CellRef::new(3, 1), "SUM(Items_2[Value])+ITEMS", 2.0),
        ] {
            assert_eq!(sheet.cell(at).and_then(|c| c.formula.as_ref()).map(|f| f.text.as_str()), Some(formula), "cycle {cycle}");
            assert_eq!(sheet.value(at), Value::Number(cache));
        }
        assert_eq!(a.cell(CellRef::new(3, 2)).and_then(|c| c.formula.as_ref()).and_then(|f| f.array), RangeRef::parse("C4:D4"));
        if cycle != 3 {
            let (saved, report) = crate::write::write_xlsx_named_with(&wb, crate::XlsxKind::Workbook, None, Default::default())?;
            assert!(renames(&report.warnings).is_empty());
            assert!(xml(&saved, "xl/tables/table1.xml")?.contains("name=\"Items_2\" displayName=\"Items_2\""));
            assert!(xml(&saved, "xl/tables/table2.xml")?.contains("name=\"ITEMS_3\" displayName=\"ITEMS_3\""));
            let (back, report) = read_xlsx(&saved)?;
            assert!(renames(&report.warnings).is_empty());
            wb = back;
        }
    }
    Ok(())
}

fn xml(bytes: &[u8], path: &str) -> Result<String, Box<dyn std::error::Error>> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))?;
    let mut text = String::new();
    archive.by_name(path)?.read_to_string(&mut text)?;
    Ok(text)
}

fn table(name: &str, col: u32) -> Table {
    Table {
        id: col + 1,
        name: name.into(),
        range: RangeRef::new(CellRef::new(0, col), CellRef::new(2, col + 1)),
        header_row: true,
        totals_row: false,
        columns: ["Label", "Value"]
            .into_iter()
            .map(|name| TableColumn { name: name.into(), totals: TotalsFn::None, totals_label: None, formula: None, unique_name: None, xml: None })
            .collect(),
        style: String::new(),
        banded_rows: false,
        banded_cols: false,
        first_col: false,
        last_col: false,
        filter_button: true,
        filter: vec![],
        table_type: None,
        connection_id: None,
        uid: Default::default(),
    }
}

#[test]
fn direct_model_repairs_formula_objects_without_rewriting_literals() -> Result<(), Box<dyn std::error::Error>> {
    let mut wb = Workbook::new();
    let sh = Arc::make_mut(&mut wb.sheets[0]);
    sh.tables.push(table("Data?Tbl", 0));
    sh.tables.push(table("Data\\Tbl", 2));
    sh.tables.push(table("Data_Tbl", 4));
    sh.tables[0].columns[0].totals_label = Some("Data?Tbl[Value]".into());
    sh.tables[0].columns[1].totals = TotalsFn::Custom;
    sh.tables[0].columns[1].totals_label = Some("SUM(Data?Tbl[Value])".into());
    let at = |row, col| CellRef::new(row, col);
    let box_at = |row, col| RangeRef::new(at(row, col), at(row, col));
    for (row, f) in [
        (0, "SUM(Data?Tbl[Value])+SUM(Data\\Tbl[Value])"),
        (1, "INDIRECT(ADDRESS(SUM(Data?Tbl[Value]),1))"),
        (2, "INDIRECT(\"Data?Tbl[Value]\")"),
        (3, "HYPERLINK(Data?Tbl[Label],\"Data?Tbl[Value]\")"),
        (4, "LET(Data_Tbl_2,7,SUM(Data?Tbl)+Data_Tbl_2)"),
        (5, "LAMBDA(Data_Tbl_2,SUM(Data?Tbl)+Data_Tbl_2)(7)"),
    ] {
        sh.set_cell(at(row, 7), Cell::formula(Formula::new(f)));
    }
    for external in [false, true] {
        let at = at(6 + u32::from(external), 7);
        sh.set_value(at, Value::text("Data?Tbl[Value]"));
        sh.hyperlinks.insert(at, Hyperlink { target: "Data?Tbl[Value]".into(), tooltip: None, external });
    }
    let f = "SUM(Data?Tbl[Value])";
    let threshold = || CfValueKind::Formula(f.into());
    let color = Color::Rgb(0x123456);
    for (i, rule) in [
        CfRule::Expression { formula: f.into(), style: Box::default() },
        CfRule::CellIs { op: CfOperator::Between, a: f.into(), b: Some(f.into()), style: Box::default() },
        CfRule::ColorScale { stops: vec![(threshold(), color), (CfValueKind::Max, color)] },
        CfRule::DataBar { min: threshold(), max: threshold(), color, gradient: true, show_value: true },
        CfRule::IconSet { set: "3Arrows".into(), thresholds: vec![threshold(), threshold(), threshold()], reverse: false, show_value: true },
    ]
    .into_iter()
    .enumerate()
    {
        sh.cond_formats.push(CondFormat { ranges: vec![box_at(i as u32, 8)], rule, priority: (i + 1) as u32, stop_if_true: false });
    }
    sh.validations.push(Validation {
        ranges: vec![box_at(0, 9)],
        kind: ValidationKind::Decimal,
        f1: f.into(),
        f2: Some(f.into()),
        ..Default::default()
    });
    sh.validations.push(Validation { ranges: vec![box_at(1, 9)], kind: ValidationKind::List, f1: "Data?Tbl[Value]".into(), ..Default::default() });
    sh.validations.push(Validation {
        ranges: vec![box_at(2, 9)],
        kind: ValidationKind::List,
        f1: "\"Data?Tbl[Value]\"".into(),
        ..Default::default()
    });
    sh.charts.push(Chart {
        id: 9,
        kind: ChartKind::Bubble,
        series: vec![Series {
            name: Some("Data?Tbl[Label]".into()),
            categories: Some("Data?Tbl[Value]".into()),
            values: "Data\\Tbl[Value]".into(),
            bubble_sizes: Some("Data?Tbl[Value]".into()),
            ..Default::default()
        }],
        anchor: Anchor { width: 160.0, height: 120.0, ..Default::default() },
        title: None,
        legend: LegendPos::Right,
        data_labels: false,
        data_table: None,
        gridlines: false,
        style: 1,
        hole_size: 75,
        first_slice_angle: 0,
        x_title: None,
        y_title: None,
        source: None,
        by_rows: false,
        pivot: None,
        axis_number_formats: Default::default(),
        pivot_label_formats: vec![],
    });
    let mut extended = sh.charts[0].clone();
    extended.id = 10;
    extended.kind = ChartKind::Waterfall;
    extended.series[0].values = "_xlchart.v1.0[Value]".into();
    extended.series.push(Series { name: Some("\"Data?Tbl[Label]\"".into()), values: "{1,2}".into(), ..Default::default() });
    sh.charts.push(extended);
    sh.tables.push(table("_xlchart.v1.0", 10));
    sh.sparklines.push(Sparkline {
        cell: at(0, 12),
        source: "Data?Tbl[Value]".into(),
        kind: SparklineKind::Line,
        color: Color::Rgb(0),
        markers: false,
    });
    sh.controls.push(FormControl { id: 11, kind: ControlKind::Slider, link: Some("Data?Tbl[@Value]".into()), ..Default::default() });
    let saved = write_xlsx(&wb)?;
    let sheet_xml = xml(&saved, "xl/worksheets/sheet1.xml")?;
    let rules = crate::xml::parse(sheet_xml.as_bytes())?;
    for (rule_type, count) in [("expression", 1), ("cellIs", 2), ("colorScale", 1), ("dataBar", 2), ("iconSet", 3)] {
        let rule = rules.kids("conditionalFormatting").flat_map(|g| g.kids("cfRule")).find(|r| r.attr("type") == Some(rule_type)).ok_or(rule_type)?;
        let count_in_formulas = rule.kids("formula").filter(|f| f.text == "SUM(Data_Tbl_2[Value])").count();
        let count_in_thresholds =
            rule.children.iter().flat_map(|e| e.kids("cfvo")).filter(|v| v.attr("val") == Some("SUM(Data_Tbl_2[Value])")).count();
        assert_eq!(count_in_formulas + count_in_thresholds, count, "{rule_type}");
    }
    assert!(sheet_xml.contains("<formula1>Data_Tbl_2[Value]</formula1>"), "{sheet_xml}");
    assert!(sheet_xml.contains("location=\"Data_Tbl_2[Value]\""));
    assert!(xml(&saved, "xl/worksheets/_rels/sheet1.xml.rels")?.contains("Target=\"Data?Tbl[Value]\""));
    let chart = xml(&saved, "xl/charts/chart1.xml")?;
    for (reference, count) in [("Data_Tbl_2[Value]", 2), ("Data_Tbl_3[Value]", 1), ("Data_Tbl_2[Label]", 1)] {
        assert_eq!(chart.matches(&format!("<c:f>{reference}</c:f>")).count(), count, "{chart}");
    }
    let table = xml(&saved, "xl/tables/table1.xml")?;
    assert!(table.contains("<totalsRowFormula>SUM(Data_Tbl_2[Value])</totalsRowFormula>"));
    assert!(table.contains("totalsRowLabel=\"Data?Tbl[Value]\""));
    assert!(xml(&saved, "xl/tables/table3.xml")?.contains("name=\"Data_Tbl\" displayName=\"Data_Tbl\""));
    let (back, _) = read_xlsx(&saved)?;
    let extended = &back.sheets[0].charts[1].series[0];
    assert_eq!(extended.name.as_deref(), Some("Data_Tbl_2[Label]"));
    assert_eq!(extended.categories.as_deref(), Some("Data_Tbl_2[Value]"));
    assert_eq!(extended.values, "_xlchart.v1.0_2[Value]");
    assert_eq!(back.sheets[0].charts[1].series[1].name.as_deref(), Some("\"Data?Tbl[Label]\""));
    assert_eq!(back.sheets[0].sparklines[0].source, "Data_Tbl_2[Value]");
    assert_eq!(back.sheets[0].controls[0].link.as_deref(), Some("Data_Tbl_2[@Value]"));
    for (row, f) in [
        (0, "SUM(Data_Tbl_2[Value])+SUM(Data_Tbl_3[Value])"),
        (1, "INDIRECT(ADDRESS(SUM(Data_Tbl_2[Value]),1))"),
        (2, "INDIRECT(\"Data?Tbl[Value]\")"),
        (3, "HYPERLINK(Data_Tbl_2[Label],\"Data?Tbl[Value]\")"),
        (4, "LET(Data_Tbl_2,7,SUM(Data_Tbl_2[])+Data_Tbl_2)"),
        (5, "LAMBDA(Data_Tbl_2,SUM(Data_Tbl_2[])+Data_Tbl_2)(7)"),
    ] {
        assert_eq!(back.sheets[0].cell(at(row, 7)).and_then(|c| c.formula.as_ref()).map(|f| f.text.as_str()), Some(f));
    }
    assert_eq!(back.sheets[0].value(at(6, 7)), Value::text("Data?Tbl[Value]"));
    assert_eq!(wb.sheets[0].tables[0].name, "Data?Tbl");
    Ok(())
}

#[test]
fn bound_pivot_keeps_the_second_table_and_its_records() -> Result<(), Box<dyn std::error::Error>> {
    let mut wb = Workbook::new();
    wb.sheets.push(Arc::new(Sheet::new("Other")));
    for (si, name, record) in [(0, "RowsTbl", 3.0), (1, "ROWstbl", 17.0)] {
        let sh = Arc::make_mut(&mut wb.sheets[si]);
        sh.tables.push(table(name, 0));
        sh.set_value(CellRef::new(0, 0), Value::text("Label"));
        sh.set_value(CellRef::new(0, 1), Value::text("Value"));
        sh.set_value(CellRef::new(1, 0), Value::text("Record"));
        sh.set_value(CellRef::new(1, 1), Value::Number(record));
    }
    let source = "Other!ROWstbl";
    let binding = wb.bind_pivot_source(source, 0);
    Arc::make_mut(&mut wb.sheets[0]).pivots.push(PivotTable {
        id: 12,
        name: "Report".into(),
        source: source.into(),
        anchor: CellRef::new(5, 5),
        rows: vec![PivotField { source_col: "Label".into(), ..Default::default() }],
        binding,
        ..Default::default()
    });
    let saved = write_xlsx(&wb)?;
    let def = xml(&saved, "xl/pivotCache/pivotCacheDefinition1.xml")?;
    assert!(def.contains("name=\"ROWstbl_2\""), "{def}");
    let records = xml(&saved, "xl/pivotCache/pivotCacheRecords1.xml")?;
    assert!(records.contains("<n v=\"17\""), "{records}");
    let (back, _) = read_xlsx(&saved)?;
    let pivot = &back.sheets[0].pivots[0];
    assert!(matches!(back.pivot_source(pivot, 0).map(|s| s.source), Ok(PivotSource::Table { sheet: 1, table: 0 })));
    Ok(())
}

#[test]
fn allocator_preserves_valid_spellings_and_bounds_generated_names() {
    use std::borrow::Cow;
    let mut wb = Workbook::new();
    let long = "É".repeat(255);
    let sh = Arc::make_mut(&mut wb.sheets[0]);
    for (i, name) in ["R1C1", "XFD1048576", "", "_xlchart.v1.0", "KeepMe", "keepme", &long, &long].into_iter().enumerate() {
        sh.tables.push(table(name, 2 * i as u32));
    }
    let mut borrowed = Cow::Borrowed(&wb);
    crate::table_names::repair(&mut borrowed, ["_XLCHART.V1.0".into()], &mut Vec::new());
    let tables = &borrowed.sheets[0].tables;
    assert_eq!(tables[3].name, "_xlchart.v1.0_2");
    assert_eq!(tables[4].name, "KeepMe");
    assert_eq!(tables[5].name, "keepme_2");
    assert_eq!(tables[6].name, long);
    assert_eq!(tables[7].name, format!("{}_2", "É".repeat(253)));
    let keys: std::collections::HashSet<_> = tables.iter().map(|t| gridcraft_formula::fold_name(&t.name)).collect();
    assert_eq!(keys.len(), tables.len());
    assert!(tables.iter().all(|t| gridcraft_core::valid_table_name(&t.name)));
}

#[test]
fn a_name_scoped_to_a_kept_sheet_follows_the_repaired_table() -> Result<(), Box<dyn std::error::Error>> {
    let mut wb = Workbook::new();
    Arc::make_mut(&mut wb.sheets[0]).tables.push(table("Source?Rows", 0));
    wb.preserved = Some(Arc::new(Preserved {
        sheets: vec![PreservedSheet {
            name: "Module1".into(),
            kind: PreservedSheetKind::Module,
            before: 1,
            sheet_id: 2,
            names: vec![PreservedName { name: "LocalRef".into(), attrs: r#" hidden="1""#.into(), formula: "Source?Rows[Value]".into() }],
            ..Default::default()
        }],
        ..Default::default()
    }));
    let saved = crate::write_xlsx_as(&wb, crate::XlsxKind::MacroWorkbook)?;
    let book = crate::xml::parse(xml(&saved, "xl/workbook.xml")?.as_bytes())?;
    let local =
        book.child("definedNames").and_then(|dn| dn.kids("definedName").find(|n| n.attr("name") == Some("LocalRef"))).ok_or("missing kept name")?;
    assert_eq!(local.attr("localSheetId"), Some("1"));
    assert_eq!(local.attr("hidden"), Some("1"));
    assert_eq!(local.text, "Source_Rows[Value]");
    Ok(())
}

/// The calculated column and totals row cells of the renamed duplicate follow it as its column
/// formula does; a formula outside it keeps the first table of that spelling.
#[test]
fn a_tables_own_cells_follow_it_like_its_column_formula() -> Result<(), Box<dyn std::error::Error>> {
    let book = r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="One" sheetId="1" r:id="rId1"/><sheet name="Two" sheetId="2" r:id="rId2"/></sheets></workbook>"#;
    let book_rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet2.xml"/></Relationships>"#;
    let sheet = |n: &str, v: u32| {
        format!(
            r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Value</t></is></c><c r="B1" t="inlineStr"><is><t>Twice</t></is></c><c r="D1"><f>SUM({n}[Value])</f><v>1</v></c></row><row r="2"><c r="A2"><v>{v}</v></c><c r="B2"><f>{n}[[#This Row],[Value]]*2</f><v>{}</v></c></row><row r="3"><c r="A3" t="inlineStr"><is><t>Total</t></is></c><c r="B3"><f>SUBTOTAL(109,{n}[Twice])</f><v>{}</v></c></row></sheetData><tableParts count="1"><tablePart r:id="rId1"/></tableParts></worksheet>"#,
            2 * v,
            2 * v
        )
    };
    let rels = |id: u32| {
        format!(
            r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/table" Target="../tables/table{id}.xml"/></Relationships>"#
        )
    };
    let table = |id: u32, n: &str| {
        format!(
            r#"<table xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" id="{id}" name="{n}" displayName="{n}" ref="A1:B3" totalsRowCount="1"><tableColumns count="2"><tableColumn id="1" name="Value" totalsRowLabel="Total"/><tableColumn id="2" name="Twice" totalsRowFunction="sum"><calculatedColumnFormula>{n}[[#This Row],[Value]]*2</calculatedColumnFormula></tableColumn></tableColumns></table>"#
        )
    };
    let (one, two) = (sheet("Items", 1), sheet("ITEMS", 5));
    let malformed = make_zip(&[
        ("[Content_Types].xml", CT.as_bytes()),
        ("_rels/.rels", ROOT_RELS.as_bytes()),
        ("xl/workbook.xml", book.as_bytes()),
        ("xl/_rels/workbook.xml.rels", book_rels.as_bytes()),
        ("xl/worksheets/sheet1.xml", one.as_bytes()),
        ("xl/worksheets/sheet2.xml", two.as_bytes()),
        ("xl/worksheets/_rels/sheet1.xml.rels", rels(1).as_bytes()),
        ("xl/worksheets/_rels/sheet2.xml.rels", rels(2).as_bytes()),
        ("xl/tables/table1.xml", table(1, "Items").as_bytes()),
        ("xl/tables/table2.xml", table(2, "ITEMS").as_bytes()),
    ]);
    let (mut wb, _) = read_xlsx(&malformed)?;
    for cycle in 0..2 {
        let formula = |si: usize, row, col| wb.sheets[si].cell(CellRef::new(row, col)).and_then(|c| c.formula.as_ref()).map(|f| f.text.clone());
        assert_eq!(wb.sheets[0].tables[0].name, "Items", "cycle {cycle}");
        assert_eq!(wb.sheets[1].tables[0].name, "ITEMS_2", "cycle {cycle}");
        assert_eq!(wb.sheets[1].tables[0].columns[1].formula.as_deref(), Some("ITEMS_2[@Value]*2"), "cycle {cycle}");
        assert_eq!(formula(1, 1, 1).as_deref(), Some("ITEMS_2[@Value]*2"), "cycle {cycle}");
        assert_eq!(formula(1, 2, 1).as_deref(), Some("SUBTOTAL(109,ITEMS_2[Twice])"), "cycle {cycle}");
        assert_eq!(formula(1, 0, 3).as_deref(), Some("SUM(ITEMS[Value])"), "outside the table: the first table, cycle {cycle}");
        assert_eq!(formula(0, 1, 1).as_deref(), Some("Items[@Value]*2"), "cycle {cycle}");
        (wb, _) = read_xlsx(&write_xlsx(&wb)?)?;
    }
    Ok(())
}

fn chart(id: u32, kind: ChartKind, series: Series) -> Chart {
    Chart {
        id,
        kind,
        series: vec![series],
        anchor: Anchor { width: 160.0, height: 120.0, ..Default::default() },
        title: None,
        legend: LegendPos::Right,
        data_labels: false,
        data_table: None,
        gridlines: false,
        style: 1,
        hole_size: 75,
        first_slice_angle: 0,
        x_title: None,
        y_title: None,
        source: None,
        by_rows: false,
        pivot: None,
        axis_number_formats: Default::default(),
        pivot_label_formats: vec![],
    }
}

/// Bare series titles the model holds as literals (older files, other formats) stay literal when
/// they only spell a structured reference: no table has that name.
#[test]
fn a_literal_series_title_spelling_a_structured_reference_stays_literal() -> Result<(), Box<dyn std::error::Error>> {
    let mut wb = Workbook::new();
    let sh = Arc::make_mut(&mut wb.sheets[0]);
    sh.set_value(CellRef::new(0, 0), Value::Number(1.0));
    let series = Series { name: Some("Sales[2024]".into()), values: "Sheet1!$A$1:$A$1".into(), ..Default::default() };
    // This spelling is free until repair creates the second sanitized table name. It must not
    // turn a literal into a reference or increase the count of ChartEx alias names.
    sh.tables.push(table("Sale?", 2));
    sh.tables.push(table("Sale?", 5));
    sh.tables.push(table("_xlchart.v1.2", 8));
    let captured = Series { name: Some("Sale__2[Value]".into()), values: "Sheet1!$A$1:$A$1".into(), ..Default::default() };
    sh.charts.push(chart(1, ChartKind::ColumnClustered, series.clone()));
    sh.charts.push(chart(2, ChartKind::Waterfall, series));
    sh.charts.push(chart(3, ChartKind::ColumnClustered, captured.clone()));
    sh.charts.push(chart(4, ChartKind::Waterfall, captured));
    let saved = write_xlsx(&wb)?;
    let classic = xml(&saved, "xl/charts/chart1.xml")?;
    assert!(classic.contains("<c:tx><c:v>Sales[2024]</c:v></c:tx>"), "{classic}");
    let extended = xml(&saved, "xl/charts/chartEx2.xml")?;
    assert!(extended.contains("<cx:tx><cx:txData><cx:v>Sales[2024]</cx:v></cx:txData></cx:tx>"), "{extended}");
    let (back, _) = read_xlsx(&saved)?;
    let captured_classic = xml(&saved, "xl/charts/chart3.xml")?;
    assert!(captured_classic.contains("<c:tx><c:v>Sale__2[Value]</c:v></c:tx>"), "{captured_classic}");
    let captured_extended = xml(&saved, "xl/charts/chartEx4.xml")?;
    assert!(captured_extended.contains("<cx:tx><cx:txData><cx:v>Sale__2[Value]</cx:v></cx:txData></cx:tx>"), "{captured_extended}");
    assert_eq!(back.sheets[0].charts.iter().filter(|ch| ch.series[0].name.as_deref() == Some("\"Sales[2024]\"")).count(), 2);
    assert_eq!(back.sheets[0].charts.iter().filter(|ch| ch.series[0].name.as_deref() == Some("\"Sale__2[Value]\"")).count(), 2);
    assert!(back.table("_xlchart.v1.2").is_some());
    let workbook = xml(&saved, "xl/workbook.xml")?;
    assert_eq!(workbook.matches("<definedName name=\"_xlchart.v1.").count(), 2);
    Ok(())
}

/// ECMA-376 §18.5.1.2: a table's name is unique amongst all defined names, whatever their scope.
/// The sheet's own name keeps its spelling and its bare references; the table's references follow
/// the table.
#[test]
fn sheet_scoped_names_reserve_table_names() -> Result<(), Box<dyn std::error::Error>> {
    let mut wb = Workbook::new();
    wb.sheets.push(Arc::new(Sheet::new("Other")));
    let sh = Arc::make_mut(&mut wb.sheets[0]);
    sh.tables.push(table("Foo", 0));
    sh.tables.push(table("LocalRef", 3));
    sh.set_cell(CellRef::new(5, 0), Cell::formula(Formula::new("FOO+SUM(Foo[Value])")));
    Arc::make_mut(&mut wb.sheets[1]).set_cell(CellRef::new(0, 0), Cell::formula(Formula::new("SUM(Foo[Value])+COUNT(Foo)")));
    wb.names.push(DefinedName {
        name: "FOO".into(),
        scope: Some(0),
        formula: "Sheet1!$H$1".into(),
        comment: String::new(),
        hidden: false,
        uid: Default::default(),
    });
    wb.names.push(DefinedName {
        name: "Summary".into(),
        formula: "FOO+SUM(Foo[Value])".into(),
        scope: None,
        comment: String::new(),
        hidden: false,
        uid: Default::default(),
    });
    wb.preserved = Some(Arc::new(Preserved {
        sheets: vec![PreservedSheet {
            name: "Module1".into(),
            kind: PreservedSheetKind::Module,
            before: 2,
            sheet_id: 3,
            names: vec![PreservedName { name: "LOCALREF".into(), attrs: r#" comment="A &amp; B""#.into(), formula: "Sheet1!$H$2".into() }],
            ..Default::default()
        }],
        ..Default::default()
    }));
    let mut back = wb;
    for cycle in 0..2 {
        let (saved, report) = crate::write::write_xlsx_named_with(&back, crate::XlsxKind::MacroWorkbook, None, Default::default())?;
        assert_eq!(report.warnings.iter().filter(|w| w.contains("was renamed")).count(), if cycle == 0 { 2 } else { 0 });
        let reopened = read_xlsx(&saved)?;
        assert!(!reopened.1.warnings.iter().any(|w| w.contains("was renamed")));
        back = reopened.0;
        let formula = |si: usize, at: CellRef| back.sheets[si].cell(at).and_then(|c| c.formula.as_ref()).map(|f| f.text.clone());
        let names: Vec<_> = back.sheets[0].tables.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["Foo_2", "LocalRef_2"], "cycle {cycle}");
        assert_eq!(formula(0, CellRef::new(5, 0)).as_deref(), Some("FOO+SUM(Foo_2[Value])"), "cycle {cycle}");
        assert_eq!(formula(1, CellRef::new(0, 0)).as_deref(), Some("SUM(Foo_2[Value])+COUNT(Foo_2)"), "cycle {cycle}");
        assert_eq!(back.name("FOO", 0).map(|n| n.formula.as_str()), Some("Sheet1!$H$1"));
        assert_eq!(back.name("Summary", 0).map(|n| n.formula.as_str()), Some("FOO+SUM(Foo_2[Value])"));
        let kept = crate::xml::parse(xml(&saved, "xl/workbook.xml")?.as_bytes())?;
        let local = kept
            .child("definedNames")
            .and_then(|dn| dn.kids("definedName").find(|n| n.attr("name") == Some("LOCALREF")))
            .ok_or("missing kept definition")?;
        assert_eq!(local.attr("comment"), Some("A & B"));
    }
    Ok(())
}

#[test]
fn modeled_print_names_reserve_table_names_on_save() -> Result<(), Box<dyn std::error::Error>> {
    let mut wb = Workbook::new();
    let sh = Arc::make_mut(&mut wb.sheets[0]);
    sh.print.print_area = RangeRef::parse("A1:C4");
    sh.print.title_rows = Some((0, 0));
    sh.tables.push(table("_xlnm.Print_Area", 0));
    sh.tables.push(table("_xlnm.Print_Titles", 3));
    let (saved, report) = crate::write::write_xlsx_named_with(&wb, crate::XlsxKind::Workbook, None, Default::default())?;
    let (back, open) = read_xlsx(&saved)?;
    assert!(open.warnings.is_empty(), "{:?}", open.warnings);
    assert_eq!(
        report.warnings,
        [
            r#"Table "_xlnm.Print_Area" on sheet "Sheet1" was renamed to "_xlnm.Print_Area_2"."#,
            r#"Table "_xlnm.Print_Titles" on sheet "Sheet1" was renamed to "_xlnm.Print_Titles_2"."#,
        ]
    );
    assert_eq!(back.table("_xlnm.Print_Area_2"), Some((0, 0)));
    assert_eq!(back.table("_xlnm.Print_Titles_2"), Some((0, 1)));
    assert_eq!(back.sheets[0].print.print_area, RangeRef::parse("A1:C4"));
    assert_eq!(back.sheets[0].print.title_rows, Some((0, 0)));
    Ok(())
}

#[test]
fn headers_and_overlaps_do_not_claim_formula_ownership() {
    let mut wb = Workbook::new();
    let first = Arc::make_mut(&mut wb.sheets[0]);
    first.tables.push(table("Items", 0));
    first.tables.push(table("OtherRows", 3));
    let mut second = Sheet::new("Second");
    second.tables.push(table("ITEMS", 0));
    second.tables.push(table("OTHERROWS", 1));
    let cases = [
        (CellRef::new(0, 0), "SUM(ITEMS[Value])", "SUM(ITEMS[Value])"),
        (CellRef::new(1, 1), "SUM(ITEMS[Value])+SUM(OTHERROWS[Value])", "SUM(ITEMS[Value])+SUM(OTHERROWS[Value])"),
        (
            CellRef::new(1, 0),
            "SUM(ITEMS[Value])+SUM(OTHERROWS[Value])+COUNT(Sheet1!ITEMS)",
            "SUM(ITEMS_2[Value])+SUM(OTHERROWS[Value])+COUNT(Sheet1!ITEMS)",
        ),
    ];
    for (at, formula, _) in cases {
        second.set_cell(at, Cell::formula(Formula::new(formula)));
    }
    wb.sheets.push(Arc::new(second));
    let mut repaired = std::borrow::Cow::Borrowed(&wb);
    crate::table_names::repair(&mut repaired, [], &mut Vec::new());
    for (at, _, expected) in cases {
        let formula = repaired.sheets[1].cell(at).and_then(|c| c.formula.as_ref());
        assert_eq!(formula.map(|f| f.text.as_str()), Some(expected), "{at:?}");
    }
}

/// A rename warning shows the names as spelled, not Rust-escaped: a table may start with `\`
/// and a sheet name may hold `"`.
#[test]
fn rename_warnings_show_names_as_spelled() -> Result<(), Box<dyn std::error::Error>> {
    let mut wb = Workbook::new();
    let sh = Arc::make_mut(&mut wb.sheets[0]);
    sh.name = "Q\"1".into();
    sh.tables.push(table("\\Bad?Name", 0));
    let (_, report) = crate::write::write_xlsx_named_with(&wb, crate::XlsxKind::Workbook, None, Default::default())?;
    assert_eq!(report.warnings, [r#"Table "\Bad?Name" on sheet "Q"1" was renamed to "\Bad_Name"."#]);
    Ok(())
}

/// A reference to no table must not start naming a table that repair gives its spelling: it
/// stays the error it evaluates to.
#[test]
fn a_reference_to_no_table_is_not_captured_by_a_repaired_name() -> Result<(), Box<dyn std::error::Error>> {
    let mut wb = Workbook::new();
    let sh = Arc::make_mut(&mut wb.sheets[0]);
    sh.tables.push(table("Sale?", 0));
    sh.tables.push(table("Sale?", 3));
    for (row, f) in [
        (6, "SUM(Sale__2[Value])"),
        (7, "COUNT(SALE__2)"),
        (8, "SUM(Sale?[Value])"),
        (9, "LET(Sale__2,7,Sale__2)"),
        (10, "LAMBDA(Sale__2,Sale__2)(7)"),
    ] {
        sh.set_cell(CellRef::new(row, 0), Cell::formula(Formula::new(f)));
    }
    for (row, target) in [(6, "Sale__2[Value]"), (7, "#Sale__2"), (8, "#Sale?[Value]")] {
        sh.hyperlinks.insert(CellRef::new(row, 1), Hyperlink { target: target.into(), tooltip: None, external: false });
    }
    sh.tables[0].columns[0].formula = Some("Sale__2[Value]".into());
    sh.tables[0].columns[0].totals = TotalsFn::Custom;
    sh.tables[0].columns[0].totals_label = Some("SUM(Sale__2[Value])".into());
    let ranges = vec![RangeRef::cell(CellRef::new(6, 2))];
    sh.cond_formats.push(CondFormat {
        ranges: ranges.clone(),
        rule: CfRule::Expression { formula: "Sale__2[Value]".into(), style: Box::default() },
        priority: 1,
        stop_if_true: false,
    });
    sh.validations.push(Validation { ranges, kind: ValidationKind::List, f1: "=Sale__2[Value]".into(), ..Default::default() });
    let series = Series { name: None, categories: Some("Sale__2[Value]".into()), values: "Sale__2[Value]".into(), ..Default::default() };
    sh.charts.push(chart(1, ChartKind::Waterfall, series.clone()));
    sh.charts.push(chart(2, ChartKind::Bubble, Series { bubble_sizes: Some("Sale__2[Value]".into()), ..series }));
    wb.names.push(DefinedName {
        name: "Dangling".into(),
        scope: None,
        formula: "Sale__2[Value]".into(),
        comment: String::new(),
        hidden: false,
        uid: Default::default(),
    });
    let mut back = wb;
    for cycle in 0..2 {
        let saved = write_xlsx(&back)?;
        let table_xml = xml(&saved, "xl/tables/table1.xml")?;
        assert!(table_xml.contains("<calculatedColumnFormula>#REF!</calculatedColumnFormula>"));
        assert!(table_xml.contains("<totalsRowFormula>SUM(#REF!)</totalsRowFormula>"));
        (back, _) = read_xlsx(&saved)?;
        let names: Vec<_> = back.sheets[0].tables.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["Sale_", "Sale__2"], "cycle {cycle}");
        for (row, f) in
            [(6, "SUM(#REF!)"), (7, "COUNT(#NAME?)"), (8, "SUM(Sale_[Value])"), (9, "LET(Sale__2,7,Sale__2)"), (10, "LAMBDA(Sale__2,Sale__2)(7)")]
        {
            let text = back.sheets[0].cell(CellRef::new(row, 0)).and_then(|c| c.formula.as_ref()).map(|f| f.text.as_str());
            assert_eq!(text, Some(f), "cycle {cycle}");
        }
        let sh = &back.sheets[0];
        for (row, target) in [(6, "#REF!"), (7, "#NAME?"), (8, "Sale_[Value]")] {
            assert_eq!(sh.hyperlinks.get(&CellRef::new(row, 1)).map(|h| (h.target.as_str(), h.external)), Some((target, false)));
        }
        assert!(matches!(&sh.cond_formats[0].rule, CfRule::Expression { formula, .. } if formula == "#REF!"));
        assert_eq!(sh.validations[0].f1, "=#REF!");
        for ch in &sh.charts {
            let series = &ch.series[0];
            assert_eq!(series.values, "#REF!");
            assert_eq!(series.categories.as_deref(), Some("#REF!"));
            if ch.kind == ChartKind::Bubble {
                assert_eq!(series.bubble_sizes.as_deref(), Some("#REF!"));
            }
        }
        assert_eq!(sh.charts.len(), 2);
        assert_eq!(back.name("Dangling", 0).map(|n| n.formula.as_str()), Some("#REF!"), "cycle {cycle}");
    }
    Ok(())
}

/// A location's one `#` marker goes on read, write and repair; a second `#` before an error token
/// is the error's own, so `##NAME?` stays `#NAME?` rather than becoming the name `NAME?`.
#[test]
fn a_doubled_location_marker_keeps_the_error_token() -> Result<(), Box<dyn std::error::Error>> {
    let book = r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets></workbook>"#;
    let book_rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#;
    let links = [("C1", "##NAME?"), ("C2", "##REF!"), ("C3", "#NAME?"), ("C4", "#Sheet1!A2"), ("C5", "Sheet1!A2")];
    let hyperlinks: String = links.iter().map(|(at, loc)| format!(r#"<hyperlink ref="{at}" location="{loc}"/>"#)).collect();
    let sheet = format!(
        r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Value</t></is></c></row><row r="2"><c r="A2"><v>1</v></c></row></sheetData><hyperlinks>{hyperlinks}</hyperlinks><tableParts count="1"><tablePart r:id="rId1"/></tableParts></worksheet>"#
    );
    let sheet_rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/table" Target="../tables/table1.xml"/></Relationships>"#;
    // An invalid table name, so open repairs the workbook and its links.
    let table_xml = r#"<table xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" id="1" name="Sale?" displayName="Sale?" ref="A1:A2"><tableColumns count="1"><tableColumn id="1" name="Value"/></tableColumns></table>"#;
    let package = make_zip(&[
        ("[Content_Types].xml", CT.as_bytes()),
        ("_rels/.rels", ROOT_RELS.as_bytes()),
        ("xl/workbook.xml", book.as_bytes()),
        ("xl/_rels/workbook.xml.rels", book_rels.as_bytes()),
        ("xl/worksheets/sheet1.xml", sheet.as_bytes()),
        ("xl/worksheets/_rels/sheet1.xml.rels", sheet_rels.as_bytes()),
        ("xl/tables/table1.xml", table_xml.as_bytes()),
    ]);
    let (mut wb, _) = read_xlsx(&package)?;
    assert_eq!(wb.sheets[0].tables[0].name, "Sale_");
    let targets = |wb: &Workbook| links.map(|(at, _)| wb.sheets[0].hyperlinks.get(&CellRef::parse(at).unwrap()).map(|h| h.target.clone()));
    let read = ["#NAME?", "#REF!", "#NAME?", "Sheet1!A2", "Sheet1!A2"].map(|t| Some(t.to_string()));
    assert_eq!(targets(&wb), read);
    // Model targets keep their doubled marker until save, which repairs again (a duplicate table).
    let sh = Arc::make_mut(&mut wb.sheets[0]);
    for (at, target) in [("C1", "##NAME?"), ("C2", "##REF!")] {
        sh.hyperlinks.insert(CellRef::parse(at).unwrap(), Hyperlink { target: target.into(), tooltip: None, external: false });
    }
    sh.tables.push(table("Sale_", 3));
    let saved = write_xlsx(&wb)?;
    let sheet_xml = xml(&saved, "xl/worksheets/sheet1.xml")?;
    for (at, loc) in [("C1", "#NAME?"), ("C2", "#REF!"), ("C3", "#NAME?"), ("C4", "Sheet1!A2"), ("C5", "Sheet1!A2")] {
        let link = sheet_xml.split("<hyperlink ").find(|l| l.contains(&format!(r#"ref="{at}""#))).and_then(|l| l.split('>').next());
        assert!(link.is_some_and(|l| l.contains(&format!(r#"location="{loc}""#))), "{at}: {link:?}");
    }
    let (back, _) = read_xlsx(&saved)?;
    assert_eq!(back.sheets[0].tables.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["Sale_", "Sale__2"]);
    assert_eq!(targets(&back), read);
    Ok(())
}

#[test]
fn hash_sheet_locations_keep_their_destinations_when_reopened() -> Result<(), Box<dyn std::error::Error>> {
    use gridcraft_core::{link_location, split_sheet};

    let mut wb = Workbook::new();
    for name in ["Data", "#Data", "#O'Brien"] {
        wb.sheets.push(Arc::new(Sheet::new(name)));
    }
    let input = ["##Data!A1", "#Data!A1", "##O'Brien!$B$2", "  #'#Data'!C3  ", "##Something", "##NAME?", " #N/A "];
    let sh = wb.sheet_mut(0).ok_or("missing source sheet")?;
    for (row, target) in (0..).zip(input) {
        sh.hyperlinks.insert(CellRef::new(row, 0), Hyperlink { target: target.into(), tooltip: None, external: false });
    }
    sh.tables.push(table("Sale?", 1));
    for _ in 0..2 {
        let saved = write_xlsx(&wb)?;
        let (back, _) = read_xlsx(&saved)?;
        let sh = back.sheet(0).ok_or("missing reopened source sheet")?;
        for (row, (expected, sheet, cell)) in (0..).zip([
            ("'#Data'!A1", "#Data", "A1"),
            ("Data!A1", "Data", "A1"),
            ("'#O''Brien'!$B$2", "#O'Brien", "$B$2"),
            ("'#Data'!C3", "#Data", "C3"),
        ]) {
            let link = &sh.hyperlinks[&CellRef::new(row, 0)];
            assert!(!link.external);
            assert_eq!(link.target, expected);
            let (name, body) = split_sheet(link_location(&link.target));
            assert_eq!(name.as_deref().and_then(|n| back.sheet_index(n)), back.sheet_index(sheet));
            assert_eq!(CellRef::parse(body), CellRef::parse(cell));
        }
        assert_eq!(sh.hyperlinks[&CellRef::new(4, 0)].target, "##Something");
        assert_eq!(sh.hyperlinks[&CellRef::new(5, 0)].target, "#NAME?");
        assert_eq!(sh.hyperlinks[&CellRef::new(6, 0)].target, "#N/A");
        wb = back;
    }
    Ok(())
}
