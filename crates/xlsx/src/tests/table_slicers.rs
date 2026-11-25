//! Table slicers (Excel 2013's `x15:tableSlicerCache`): written from the model and read back with
//! the same table binding, selection and geometry, through renames and X17's name repair.

use std::collections::BTreeMap;
use std::io::{Cursor, Read};
use std::sync::Arc;

use gridcraft_core::{CellRef, RangeRef, Value};
use gridcraft_model::*;

use super::{CT, ROOT_RELS, make_zip};
use crate::read_xlsx;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn unzip(bytes: &[u8]) -> Result<BTreeMap<String, String>, Box<dyn std::error::Error>> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))?;
    let mut parts = BTreeMap::new();
    for i in 0..archive.len() {
        let mut f = archive.by_index(i)?;
        let mut text = String::new();
        if f.read_to_string(&mut text).is_ok() {
            parts.insert(f.name().to_string(), text);
        }
    }
    Ok(parts)
}

fn save(wb: &Workbook) -> Result<(Vec<u8>, Vec<String>), Box<dyn std::error::Error>> {
    let (bytes, report) = crate::write::write_xlsx_named_with(wb, crate::XlsxKind::Workbook, None, Default::default())?;
    Ok((bytes, report.warnings))
}

fn column(name: &str) -> TableColumn {
    TableColumn { name: name.into(), totals: TotalsFn::None, totals_label: None, formula: None, unique_name: None, xml: None }
}

/// "Data" holds table "Sales" (A1:C5: Amount, Region, Channel) with Region filtered to East and
/// West, as the engine applies a table slicer's selection (the sheet's AutoFilter over the table);
/// "Dash" holds a slicer on Sales[Region] with that selection.
fn workbook() -> Workbook {
    let mut wb = Workbook::new();
    let mut data = Sheet::new("Data");
    let rows = [["Amount", "Region", "Channel"], ["10", "East", "Web"], ["20", "West", "Store"], ["30", "North", "Web"], ["40", "East", "Store"]];
    for (r, row) in rows.iter().enumerate() {
        for (c, v) in row.iter().enumerate() {
            let value = v.parse::<f64>().map(Value::Number).unwrap_or_else(|_| Value::text(*v));
            data.set_value(CellRef::new(r as u32, c as u32), value);
        }
    }
    let range = RangeRef::new(CellRef::new(0, 0), CellRef::new(4, 2));
    data.tables.push(Table {
        id: 1,
        name: "Sales".into(),
        range,
        header_row: true,
        totals_row: false,
        columns: vec![column("Amount"), column("Region"), column("Channel")],
        style: "TableStyleMedium2".into(),
        banded_rows: true,
        banded_cols: false,
        first_col: false,
        last_col: false,
        filter_button: true,
        filter: vec![],
        table_type: None,
        connection_id: None,
        uid: Default::default(),
    });
    let selected = vec!["East".to_string(), "West".to_string()];
    data.autofilter = Some(AutoFilter { range, criteria: vec![(1, FilterCriterion::Values { values: selected.clone(), blanks: false })] });
    let mut dash = Sheet::new("Dash");
    dash.slicers.push(Slicer {
        id: 7,
        source: SlicerSource::Table { table: "Sales".into(), column: "Region".into(), selected: Some(selected) },
        caption: "Pick a region".into(),
        anchor: Anchor { cell: CellRef::new(2, 3), dx: 4.0, dy: 2.0, width: 180.0, height: 220.0, mode: AnchorMode::default() },
        columns: 2,
        name: "Region 1".into(),
        style: "SlicerStyleDark2".into(),
        row_height: Some(30.0),
        imported_cache: Default::default(),
    });
    wb.sheets = vec![Arc::new(data), Arc::new(dash)];
    wb
}

fn assert_same_slicer(a: &Slicer, b: &Slicer) {
    assert_eq!((&b.source, &b.caption, b.columns, &b.name, &b.style), (&a.source, &a.caption, a.columns, &a.name, &a.style));
    assert_eq!(b.anchor.cell, a.anchor.cell);
    for (x, y) in [(a.anchor.dx, b.anchor.dx), (a.anchor.dy, b.anchor.dy), (a.anchor.width, b.anchor.width), (a.anchor.height, b.anchor.height)] {
        assert!((x - y).abs() < 1.0, "{:?} vs {:?}", a.anchor, b.anchor);
    }
    assert!(b.row_height.zip(a.row_height).is_some_and(|(x, y)| (x - y).abs() < 1.0), "{:?} vs {:?}", a.row_height, b.row_height);
}

#[test]
fn table_slicer_round_trips_binding_selection_and_geometry() -> TestResult {
    let wb = workbook();
    let (bytes, warnings) = save(&wb)?;
    assert!(warnings.is_empty(), "{warnings:?}");
    let parts = unzip(&bytes)?;
    let part = |k: &str| parts.get(k).cloned().ok_or_else(|| format!("missing {k}: {:?}", parts.keys().collect::<Vec<_>>()));
    // The cache names the table and column by the ids they are written with.
    let table = part("xl/tables/table1.xml")?;
    assert!(table.contains(" id=\"1\" name=\"Sales\"") && table.contains("<tableColumn id=\"2\" name=\"Region\""), "{table}");
    let cache = part("xl/slicerCaches/slicerCache1.xml")?;
    assert!(cache.contains("name=\"Slicer_Region\" sourceName=\"Region\""), "{cache}");
    assert!(cache.contains("<x:ext uri=\"{2F2917AC-EB37-4324-AD4E-5DD8C200BD13}\""), "{cache}");
    assert!(cache.contains("<x15:tableSlicerCache tableId=\"1\" column=\"2\"/>"), "{cache}");
    assert!(!cache.contains("pivotTables") && !cache.contains("<data>"), "{cache}");
    let book = part("xl/workbook.xml")?;
    assert!(book.contains("{46BE6895-7355-4a93-B00E-2C351335B9C9}") && book.contains("<x15:slicerCaches"), "{book}");
    assert!(!book.contains("{BBE1A952-AA13-448e-AADC-164F8A28A991}"), "a table slicer cache is not listed as a PivotTable one: {book}");
    assert!(book.contains("<definedName name=\"Slicer_Region\">#N/A</definedName>"), "{book}");
    let book_rels = part("xl/_rels/workbook.xml.rels")?;
    assert!(book_rels.contains("relationships/slicerCache\" Target=\"slicerCaches/slicerCache1.xml\""), "{book_rels}");
    let dash = part("xl/worksheets/sheet2.xml")?;
    assert!(dash.contains("{3A4CF648-6AED-40f4-86FF-DC5316D8AED3}") && dash.contains("<x14:slicerList"), "{dash}");
    assert!(!dash.contains("{A8765BA9-456A-4dab-B4F3-ACF838C121DE}"), "{dash}");
    let slicer = part("xl/slicers/slicer1.xml")?;
    assert!(
        slicer.contains("name=\"Region 1\" cache=\"Slicer_Region\" caption=\"Pick a region\" columnCount=\"2\" style=\"SlicerStyleDark2\""),
        "{slicer}"
    );
    let ct = part("[Content_Types].xml")?;
    assert!(ct.contains("/xl/slicers/slicer1.xml") && ct.contains("/xl/slicerCaches/slicerCache1.xml"), "{ct}");
    let drawing = parts.iter().find(|(k, v)| k.starts_with("xl/drawings/drawing") && v.contains("sle:slicer")).map(|(_, v)| v).ok_or("frame")?;
    assert!(drawing.contains("Requires=\"sle15\"") && drawing.contains("name=\"Region 1\"") && drawing.contains("mc:Fallback"), "{drawing}");
    // The selection is the table's filter on the column.
    assert!(table.contains("<filterColumn colId=\"1\">") && table.contains("val=\"East\"") && table.contains("val=\"West\""), "{table}");

    let (back, report) = read_xlsx(&bytes)?;
    assert!(report.warnings.iter().all(|w| !w.contains("slicer")), "{:?}", report.warnings);
    assert_eq!(back.sheets[1].slicers.len(), 1);
    assert_same_slicer(&wb.sheets[1].slicers[0], &back.sheets[1].slicers[0]);
    assert_eq!(back.sheets[1].slicers[0].imported_cache.name, "Slicer_Region");

    // A second save keeps the cache's name and marker (written once) and the slicer.
    let (again, warnings) = save(&back)?;
    assert!(warnings.is_empty(), "{warnings:?}");
    let parts = unzip(&again)?;
    let book = parts.get("xl/workbook.xml").ok_or("workbook")?;
    assert_eq!(book.matches("name=\"Slicer_Region\"").count(), 1, "{book}");
    let (third, _) = read_xlsx(&again)?;
    assert_same_slicer(&wb.sheets[1].slicers[0], &third.sheets[1].slicers[0]);
    Ok(())
}

#[test]
fn renamed_table_keeps_its_slicer() -> TestResult {
    let mut wb = workbook();
    wb.rename_table(0, 0, "Revenue");
    let SlicerSource::Table { table, .. } = &wb.sheets[1].slicers[0].source else { return Err("table slicer".into()) };
    assert_eq!(table, "Revenue");
    let (bytes, warnings) = save(&wb)?;
    assert!(warnings.is_empty(), "{warnings:?}");
    let (back, _) = read_xlsx(&bytes)?;
    assert_same_slicer(&wb.sheets[1].slicers[0], &back.sheets[1].slicers[0]);
    Ok(())
}

/// X17 renames a table that shares a defined name's spelling on save: the slicer follows the
/// written name, and the cache keeps naming it by id.
#[test]
fn table_repaired_on_save_keeps_its_slicer() -> TestResult {
    let mut wb = workbook();
    wb.names.push(DefinedName {
        name: "SALES".into(),
        scope: None,
        formula: "Data!$H$1".into(),
        comment: String::new(),
        hidden: false,
        uid: Default::default(),
    });
    let (bytes, warnings) = save(&wb)?;
    assert!(warnings.iter().any(|w| w.contains("was renamed")), "{warnings:?}");
    assert!(warnings.iter().all(|w| !w.contains("slicer")), "{warnings:?}");
    let (back, _) = read_xlsx(&bytes)?;
    let renamed = back.sheets[0].tables[0].name.clone();
    assert_ne!(renamed.to_ascii_lowercase(), "sales");
    let SlicerSource::Table { table, column, selected } = &back.sheets[1].slicers[0].source else { return Err("table slicer".into()) };
    assert_eq!((table.as_str(), column.as_str()), (renamed.as_str(), "Region"));
    assert_eq!(selected.as_deref(), Some(&["East".to_string(), "West".to_string()][..]));
    Ok(())
}

#[test]
fn slicer_without_its_table_or_column_is_dropped_with_a_warning() -> TestResult {
    let removed_table = {
        let mut wb = workbook();
        Arc::make_mut(&mut wb.sheets[0]).tables.clear();
        wb
    };
    let removed_column = {
        let mut wb = workbook();
        if let SlicerSource::Table { column, .. } = &mut Arc::make_mut(&mut wb.sheets[1]).slicers[0].source {
            *column = "Country".into();
        }
        wb
    };
    for (wb, expected) in [
        (removed_table, "slicer \"Region 1\" was dropped because its table \"Sales\" is not in the saved workbook"),
        (removed_column, "slicer \"Region 1\" was dropped because table \"Sales\" has no column \"Country\""),
    ] {
        let (bytes, warnings) = save(&wb)?;
        assert_eq!(warnings.iter().filter(|w| w.contains("slicer")).collect::<Vec<_>>(), [expected]);
        let parts = unzip(&bytes)?;
        assert!(parts.keys().all(|k| !k.contains("slicer")), "{:?}", parts.keys().collect::<Vec<_>>());
        for (k, v) in &parts {
            assert!(!v.contains("slicer") && !v.contains("Slicer_"), "{k} still names a slicer: {v}");
        }
        let (back, report) = read_xlsx(&bytes)?;
        assert!(back.sheets.iter().all(|s| s.slicers.is_empty()));
        assert!(report.warnings.iter().all(|w| !w.contains("slicer")), "{:?}", report.warnings);
    }
    Ok(())
}

#[test]
fn empty_table_drops_its_slicer_but_preserves_imported_marker() -> TestResult {
    let (mut wb, _) = read_xlsx(&save(&workbook())?.0)?;
    let cache_name = wb.sheets[1].slicers[0].imported_cache.name.clone();
    let marker = wb.names.iter().find(|n| n.name == cache_name).ok_or("imported marker")?.clone();
    for (range, totals) in [("A1:C1", false), ("A1:C2", true)] {
        let table = &mut wb.sheet_mut(0).ok_or("source sheet")?.tables[0];
        table.range = RangeRef::parse(range).ok_or("test range")?;
        table.totals_row = totals;
        let (bytes, warnings) = save(&wb)?;
        let parts = unzip(&bytes)?;
        assert!(parts.contains_key("xl/tables/table1.xml"));
        assert!(parts.keys().all(|k| !k.contains("slicer")));
        assert!(parts.values().all(|xml| !xml.contains("slicerList") && !xml.contains("slicerCaches") && !xml.contains("sle:slicer")));
        assert!(warnings.iter().any(|w| w.contains("slicer \"Region 1\"") && w.contains("no data rows")), "{warnings:?}");
        let (back, _) = read_xlsx(&bytes)?;
        assert!(back.names.contains(&marker));
        assert!(back.sheets[1].slicers.is_empty());
    }
    Ok(())
}

/// A file's table slicer keeps its cache name, its marker (written once, with its attributes) and
/// the cache's item options, and is bound by file id even when another table has a column of the
/// same name.
#[test]
fn imported_table_slicer_keeps_its_cache() -> TestResult {
    let book = r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Data" sheetId="1" r:id="rId1"/></sheets><definedNames><definedName name="Slicer_Status" hidden="1">#N/A</definedName></definedNames><extLst><ext uri="{46BE6895-7355-4a93-B00E-2C351335B9C9}" xmlns:x15="http://schemas.microsoft.com/office/spreadsheetml/2010/11/main"><x15:slicerCaches xmlns:x14="http://schemas.microsoft.com/office/spreadsheetml/2009/9/main"><x14:slicerCache r:id="rId2"/></x15:slicerCaches></ext></extLst></workbook>"#;
    let book_rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="http://schemas.microsoft.com/office/2007/relationships/slicerCache" Target="slicerCaches/slicerCache1.xml"/></Relationships>"#;
    let sheet = r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Status</t></is></c><c r="C1" t="inlineStr"><is><t>Status</t></is></c></row><row r="2"><c r="A2" t="inlineStr"><is><t>Open</t></is></c><c r="C2" t="inlineStr"><is><t>Done</t></is></c></row></sheetData><drawing r:id="rId1"/><tableParts count="2"><tablePart r:id="rId2"/><tablePart r:id="rId3"/></tableParts><extLst><ext uri="{3A4CF648-6AED-40f4-86FF-DC5316D8AED3}" xmlns:x15="http://schemas.microsoft.com/office/spreadsheetml/2010/11/main"><x14:slicerList xmlns:x14="http://schemas.microsoft.com/office/spreadsheetml/2009/9/main"><x14:slicer r:id="rId4"/></x14:slicerList></ext></extLst></worksheet>"#;
    let sheet_rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/drawing" Target="../drawings/drawing1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/table" Target="../tables/table1.xml"/><Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/table" Target="../tables/table2.xml"/><Relationship Id="rId4" Type="http://schemas.microsoft.com/office/2007/relationships/slicer" Target="../slicers/slicer1.xml"/></Relationships>"#;
    let drawing = r#"<xdr:wsDr xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><xdr:twoCellAnchor editAs="oneCell"><xdr:from><xdr:col>4</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>1</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from><xdr:to><xdr:col>7</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>12</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:to><mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" xmlns:sle15="http://schemas.microsoft.com/office/drawing/2012/slicer"><mc:Choice Requires="sle15"><xdr:graphicFrame macro=""><xdr:nvGraphicFramePr><xdr:cNvPr id="2" name="Status"/><xdr:cNvGraphicFramePr/></xdr:nvGraphicFramePr><xdr:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/></xdr:xfrm><a:graphic><a:graphicData uri="http://schemas.microsoft.com/office/drawing/2010/slicer"><sle:slicer xmlns:sle="http://schemas.microsoft.com/office/drawing/2010/slicer" name="Status"/></a:graphicData></a:graphic></xdr:graphicFrame></mc:Choice><mc:Fallback/></mc:AlternateContent><xdr:clientData/></xdr:twoCellAnchor></xdr:wsDr>"#;
    let table = |id: u32, name: &str, r: &str| {
        format!(
            r#"<table xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" id="{id}" name="{name}" displayName="{name}" ref="{r}"><autoFilter ref="{r}"/><tableColumns count="1"><tableColumn id="5" name="Status"/></tableColumns></table>"#
        )
    };
    let slicer = r#"<slicers xmlns="http://schemas.microsoft.com/office/spreadsheetml/2009/9/main"><slicer name="Status" cache="Slicer_Status" caption="Status" rowHeight="241300"/></slicers>"#;
    let cache = r#"<slicerCacheDefinition xmlns="http://schemas.microsoft.com/office/spreadsheetml/2009/9/main" xmlns:x="http://schemas.openxmlformats.org/spreadsheetml/2006/main" name="Slicer_Status" sourceName="Status"><extLst><x:ext uri="{2F2917AC-EB37-4324-AD4E-5DD8C200BD13}" xmlns:x15="http://schemas.microsoft.com/office/spreadsheetml/2010/11/main"><x15:tableSlicerCache tableId="9" column="5" sortOrder="descending" customListSort="0" crossFilter="none"/></x:ext></extLst></slicerCacheDefinition>"#;
    let (first, second) = (table(4, "Open", "A1:A2"), table(9, "Done", "C1:C2"));
    let package = |second: &str, cache: &str| {
        let sheet = if second.contains("name=\"Other\"") {
            sheet.replace(r#"r="C1" t="inlineStr"><is><t>Status"#, r#"r="C1" t="inlineStr"><is><t>Other"#)
        } else {
            sheet.to_owned()
        };
        make_zip(&[
            ("[Content_Types].xml", CT.as_bytes()),
            ("_rels/.rels", ROOT_RELS.as_bytes()),
            ("xl/workbook.xml", book.as_bytes()),
            ("xl/_rels/workbook.xml.rels", book_rels.as_bytes()),
            ("xl/worksheets/sheet1.xml", sheet.as_bytes()),
            ("xl/worksheets/_rels/sheet1.xml.rels", sheet_rels.as_bytes()),
            ("xl/drawings/drawing1.xml", drawing.as_bytes()),
            ("xl/tables/table1.xml", first.as_bytes()),
            ("xl/tables/table2.xml", second.as_bytes()),
            ("xl/slicers/slicer1.xml", slicer.as_bytes()),
            ("xl/slicerCaches/slicerCache1.xml", cache.as_bytes()),
        ])
    };
    let mut mismatches = Vec::new();
    for (second, cache, target) in [
        (second.clone(), cache.replace("column=\"5\"", "column=\"77\""), Some("Done")),
        (second.replace("name=\"Status\"", "name=\"Other\""), cache.replace("column=\"5\"", "column=\"77\""), None),
        (second.clone(), cache.replace("tableId=\"9\"", "tableId=\"77\""), None),
        (second.replace("name=\"Status\"", "name=\"Other\""), cache.replace("tableId=\"9\"", "tableId=\"invalid\""), None),
        (second.clone(), cache.replace(" tableId=\"9\"", ""), None),
        (second.replace("name=\"Status\"", "name=\"Other\""), cache.replace(" tableId=\"9\"", ""), Some("Open")),
    ] {
        let (wb, report) = read_xlsx(&package(&second, &cache))?;
        let actual = wb.sheets[0].slicers.first().and_then(|s| match &s.source {
            SlicerSource::Table { table, .. } => Some(table.as_str()),
            _ => None,
        });
        let warned = report.warnings.iter().any(|w| w.contains("table column could not be found"));
        if actual != target || warned != target.is_none() {
            mismatches.push(format!("expected {target:?}, got {actual:?} with warnings {:?}: {cache}", report.warnings));
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
    let file = package(&second, cache);
    let (wb, report) = read_xlsx(&file)?;
    assert!(report.warnings.iter().all(|w| !w.contains("slicer")), "{:?}", report.warnings);
    let sl = wb.sheets[0].slicers.first().ok_or("slicer")?;
    assert!(matches!(&sl.source, SlicerSource::Table { table, column, selected: None } if table == "Done" && column == "Status"), "{:?}", sl.source);
    assert_eq!((sl.name.as_str(), sl.imported_cache.name.as_str()), ("Status", "Slicer_Status"));

    let (bytes, warnings) = save(&wb)?;
    assert!(warnings.is_empty(), "{warnings:?}");
    let parts = unzip(&bytes)?;
    let book = parts.get("xl/workbook.xml").ok_or("workbook")?;
    assert_eq!(book.matches("name=\"Slicer_Status\"").count(), 1, "{book}");
    assert!(book.contains("<definedName name=\"Slicer_Status\" hidden=\"1\">#N/A</definedName>"), "{book}");
    let cache = parts.get("xl/slicerCaches/slicerCache1.xml").ok_or("cache")?;
    // "Done" is the second table written.
    assert!(cache.contains("name=\"Slicer_Status\" sourceName=\"Status\""), "{cache}");
    assert!(cache.contains("tableId=\"2\" column=\"1\" sortOrder=\"descending\" customListSort=\"0\" crossFilter=\"none\""), "{cache}");
    let (back, _) = read_xlsx(&bytes)?;
    let again = back.sheets[0].slicers.first().ok_or("slicer")?;
    assert_eq!(again.source, sl.source);
    assert_eq!(again.imported_cache.table_items, sl.imported_cache.table_items);
    Ok(())
}
