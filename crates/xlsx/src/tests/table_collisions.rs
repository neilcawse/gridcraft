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
    let (mut wb, _) = read_xlsx(&malformed)?;
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
            let saved = write_xlsx(&wb)?;
            assert!(xml(&saved, "xl/tables/table1.xml")?.contains("name=\"Items_2\" displayName=\"Items_2\""));
            assert!(xml(&saved, "xl/tables/table2.xml")?.contains("name=\"ITEMS_3\" displayName=\"ITEMS_3\""));
            (wb, _) = read_xlsx(&saved)?;
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
    crate::table_names::repair(&mut borrowed, ["_XLCHART.V1.0".into()]);
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
            names: vec![PreservedName { attrs: r#" name="LocalRef" hidden="1""#.into(), formula: "Source?Rows[Value]".into() }],
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
