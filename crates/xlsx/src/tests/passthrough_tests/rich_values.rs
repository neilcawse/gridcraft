//! Value metadata GridCraft does not model: rich errors (`#CALC!`, `#SPILL!` with their reasons),
//! linked data types, cube members. The metadata part, the rich-value parts behind it and the
//! `vm` of the cells that carry it come back as they were, for as long as the cells hold the
//! values the metadata describes. The Power Pivot data model is an opaque part of the workbook.

use gridcraft_core::{CellRef, Value};
use gridcraft_model::Formula;

use super::*;

const MS_2017: &str = "http://schemas.microsoft.com/office/2017/06/relationships";
const REL_METADATA: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/sheetMetadata";
const NS_RICH: &str = "http://schemas.microsoft.com/office/spreadsheetml/2017/richdata";
const RICH_EXT: &str = "{3e2802c4-a4d2-4d8b-9148-e3be6c30e623}";
const DYNAMIC_EXT: &str = "{bdbb8cdc-fa1e-496e-a857-3c3f30c029c3}";
const TYPE_ATTRS: &str = r#"minSupportedVersion="120000" copy="1" pasteAll="1" pasteValues="1" merge="1" splitFirst="1" rowColShift="1" clearFormats="1" clearComments="1" assign="1" coerce="1""#;

/// Metadata with rich values only: `vm` 1 and 2 are rich values 0 and 1.
fn metadata_rich() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<metadata xmlns="{NS_MAIN}" xmlns:xlrd="{NS_RICH}"><metadataTypes count="1"><metadataType name="XLRICHVALUE" {TYPE_ATTRS}/></metadataTypes><futureMetadata name="XLRICHVALUE" count="2"><bk><extLst><ext uri="{RICH_EXT}"><xlrd:rvb i="0"/></ext></extLst></bk><bk><extLst><ext uri="{RICH_EXT}"><xlrd:rvb i="1"/></ext></extLst></bk></futureMetadata><valueMetadata count="2"><bk><rc t="1" v="0"/></bk><bk><rc t="1" v="1"/></bk></valueMetadata></metadata>"#
    )
}

/// The same with the dynamic-array flag as entry 1 of the cell metadata (`cm="1"`), as Excel
/// writes a workbook that has both.
fn metadata_rich_dynamic() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<metadata xmlns="{NS_MAIN}" xmlns:xlrd="{NS_RICH}" xmlns:xda="http://schemas.microsoft.com/office/spreadsheetml/2017/dynamicarray"><metadataTypes count="2"><metadataType name="XLDAPR" {TYPE_ATTRS} cellMeta="1"/><metadataType name="XLRICHVALUE" {TYPE_ATTRS}/></metadataTypes><futureMetadata name="XLDAPR" count="1"><bk><extLst><ext uri="{DYNAMIC_EXT}"><xda:dynamicArrayProperties fDynamic="1" fCollapsed="0"/></ext></extLst></bk></futureMetadata><futureMetadata name="XLRICHVALUE" count="2"><bk><extLst><ext uri="{RICH_EXT}"><xlrd:rvb i="0"/></ext></extLst></bk><bk><extLst><ext uri="{RICH_EXT}"><xlrd:rvb i="1"/></ext></extLst></bk></futureMetadata><cellMetadata count="1"><bk><rc t="1" v="0"/></bk></cellMetadata><valueMetadata count="2"><bk><rc t="2" v="0"/></bk><bk><rc t="2" v="1"/></bk></valueMetadata></metadata>"#
    )
}

const RICH_VALUES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<rvData xmlns="http://schemas.microsoft.com/office/spreadsheetml/2017/richdata" count="2"><rv s="0"><v>13</v><v>3</v></rv><rv s="0"><v>8</v><v>2</v></rv></rvData>"#;
const RICH_STRUCTURES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<rvStructures xmlns="http://schemas.microsoft.com/office/spreadsheetml/2017/richdata" count="1"><s t="_error"><k n="errorType" t="i"/><k n="subType" t="i"/></s></rvStructures>"#;
const RICH_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<rvTypesInfo xmlns="http://schemas.microsoft.com/office/spreadsheetml/2017/richdata2" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" mc:Ignorable="x" xmlns:x="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><global><keyFlags><key name="_Self"><flag name="ExcludeFromFile" value="1"/><flag name="ExcludeFromCalcComparison" value="1"/></key></keyFlags></global></rvTypesInfo>"#;
const MODEL: &[u8] = b"\xFF\xFES\x00T\x00R\x00E\x00A\x00M\x00 data model bytes \x00\x01\x02";

/// Cells: A1 a formula whose error is rich value 0, A2 a constant carrying rich value 1, B1 plain.
/// `dynamic` adds a dynamic-array formula in D1:D2.
fn sheet_xml(dynamic: bool) -> String {
    let spill = if dynamic { r#"<c r="D1" cm="1"><f t="array" ref="D1:D2">_xlfn.SEQUENCE(2)</f><v>1</v></c>"# } else { "" };
    let spill2 = if dynamic { r#"<c r="D2"><v>2</v></c>"# } else { "" };
    format!(
        r#"<?xml version="1.0"?><worksheet xmlns="{NS_MAIN}"><sheetData><row r="1"><c r="A1" t="e" vm="1"><f>SUM(C1:C2)</f><v>#VALUE!</v></c><c r="B1"><v>1</v></c>{spill}</row><row r="2"><c r="A2" vm="2"><v>7</v></c>{spill2}</row></sheetData></worksheet>"#
    )
}

struct Pack {
    metadata: String,
    /// Leaves out the rich-value structure part (the metadata then refers to nothing).
    structures: bool,
    dynamic_cells: bool,
}

fn pack(metadata: String) -> Pack {
    Pack { metadata, structures: true, dynamic_cells: false }
}

fn package(p: &Pack) -> Vec<u8> {
    let types = format!(
        r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="xml" ContentType="application/xml"/><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Override PartName="/xl/workbook.xml" ContentType="{CT_PREFIX}.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="{CT_PREFIX}.spreadsheetml.worksheet+xml"/><Override PartName="/xl/metadata.xml" ContentType="{CT_PREFIX}.spreadsheetml.sheetMetadata+xml"/><Override PartName="/xl/richData/rdrichvalue.xml" ContentType="application/vnd.ms-excel.rdrichvalue+xml"/><Override PartName="/xl/richData/rdrichvaluestructure.xml" ContentType="application/vnd.ms-excel.rdrichvaluestructure+xml"/><Override PartName="/xl/richData/rdRichValueTypes.xml" ContentType="application/vnd.ms-excel.rdrichvaluetypes+xml"/></Types>"#
    );
    let workbook = format!(
        r#"<?xml version="1.0"?><workbook xmlns="{NS_MAIN}" xmlns:r="{NS_R}"><sheets><sheet name="Data" sheetId="1" r:id="rId1"/></sheets></workbook>"#
    );
    let root_rels = rels(&[("rId1", "officeDocument", "xl/workbook.xml")]);
    let wb_rels = rels(&[
        ("rId1", "worksheet", "worksheets/sheet1.xml"),
        ("rId2", REL_METADATA, "metadata.xml"),
        ("rId3", &format!("{MS_2017}/rdRichValue"), "richData/rdrichvalue.xml"),
        ("rId4", &format!("{MS_2017}/rdRichValueStructure"), "richData/rdrichvaluestructure.xml"),
        ("rId5", &format!("{MS_2017}/rdRichValueTypes"), "richData/rdRichValueTypes.xml"),
    ]);
    let sheet = sheet_xml(p.dynamic_cells);
    let mut parts: Vec<(&str, &[u8])> = vec![
        ("[Content_Types].xml", types.as_bytes()),
        ("_rels/.rels", root_rels.as_bytes()),
        ("xl/workbook.xml", workbook.as_bytes()),
        ("xl/_rels/workbook.xml.rels", wb_rels.as_bytes()),
        ("xl/worksheets/sheet1.xml", sheet.as_bytes()),
        ("xl/metadata.xml", p.metadata.as_bytes()),
        ("xl/richData/rdrichvalue.xml", RICH_VALUES.as_bytes()),
        ("xl/richData/rdRichValueTypes.xml", RICH_TYPES.as_bytes()),
    ];
    if p.structures {
        parts.push(("xl/richData/rdrichvaluestructure.xml", RICH_STRUCTURES.as_bytes()));
    }
    make_zip(&parts)
}

/// No warning that value metadata, rich values or the data model were not kept. (A workbook with
/// rich values and no pictures also gets the cell-picture reader's "pictures were not loaded".)
fn no_loss(warnings: &[String]) -> bool {
    !warnings.iter().any(|w| w.contains("value metadata") || w.contains("dropped"))
}

fn has_rel(u: &Unpacked, kind_uri: &str, target: &str) -> bool {
    u.rels("xl/workbook.xml").iter().any(|r| r.1 == kind_uri && r.2 == target)
}

fn assert_rich_parts_written(u: &Unpacked, metadata: &str) {
    assert_eq!(u.parts.get("xl/metadata.xml").unwrap(), metadata.as_bytes());
    assert_eq!(u.parts.get("xl/richData/rdrichvalue.xml").unwrap(), RICH_VALUES.as_bytes());
    assert_eq!(u.parts.get("xl/richData/rdrichvaluestructure.xml").unwrap(), RICH_STRUCTURES.as_bytes());
    assert_eq!(u.parts.get("xl/richData/rdRichValueTypes.xml").unwrap(), RICH_TYPES.as_bytes());
    assert!(has_rel(u, REL_METADATA, "xl/metadata.xml"), "{:?}", u.rels("xl/workbook.xml"));
    assert!(has_rel(u, &format!("{MS_2017}/rdRichValue"), "xl/richData/rdrichvalue.xml"));
    assert!(has_rel(u, &format!("{MS_2017}/rdRichValueStructure"), "xl/richData/rdrichvaluestructure.xml"));
    assert!(has_rel(u, &format!("{MS_2017}/rdRichValueTypes"), "xl/richData/rdRichValueTypes.xml"));
    let types = u.text("[Content_Types].xml");
    for (part, ct) in [
        ("/xl/metadata.xml", format!("{CT_PREFIX}.spreadsheetml.sheetMetadata+xml")),
        ("/xl/richData/rdrichvalue.xml", "application/vnd.ms-excel.rdrichvalue+xml".to_string()),
        ("/xl/richData/rdrichvaluestructure.xml", "application/vnd.ms-excel.rdrichvaluestructure+xml".to_string()),
        ("/xl/richData/rdRichValueTypes.xml", "application/vnd.ms-excel.rdrichvaluetypes+xml".to_string()),
    ] {
        assert!(types.contains(&format!(r#"PartName="{part}" ContentType="{ct}""#)), "{part}: {types}");
    }
}

fn assert_no_rich_parts(u: &Unpacked) {
    assert!(u.parts.keys().all(|n| !n.starts_with("xl/richData/") && n != "xl/metadata.xml"), "{:?}", u.parts.keys());
    let rels = u.rels("xl/workbook.xml");
    assert!(rels.iter().all(|r| !r.1.ends_with("/sheetMetadata") && !r.1.contains("/rdRichValue")), "{rels:?}");
}

fn at(a1: &str) -> CellRef {
    CellRef::parse(a1).unwrap()
}

#[test]
fn rich_values_and_the_value_metadata_of_cells_come_back() {
    let metadata = metadata_rich();
    let (wb, rep) = read_xlsx(&package(&pack(metadata.clone()))).unwrap();
    assert!(no_loss(&rep.warnings), "{:?}", rep.warnings);
    let out = write_xlsx(&wb).unwrap();
    let u = Unpacked::new(&out);
    u.assert_valid();
    assert_rich_parts_written(&u, &metadata);
    let sheet = u.text("xl/worksheets/sheet1.xml");
    assert!(sheet.contains(r#"<c r="A1" vm="1" t="e"><f>SUM(C1:C2)</f><v>#VALUE!</v></c>"#), "{sheet}");
    assert!(sheet.contains(r#"<c r="A2" vm="2"><v>7</v></c>"#), "{sheet}");
    assert!(sheet.contains(r#"<c r="B1"><v>1</v></c>"#), "{sheet}");

    // Saving what was saved changes nothing.
    let (wb2, rep2) = read_xlsx(&out).unwrap();
    assert!(no_loss(&rep2.warnings), "{:?}", rep2.warnings);
    let again = Unpacked::new(&write_xlsx(&wb2).unwrap());
    again.assert_valid();
    assert_rich_parts_written(&again, &metadata);
    assert_eq!(again.text("xl/worksheets/sheet1.xml"), sheet);
}

#[test]
fn only_the_cells_that_still_hold_their_value_keep_their_value_metadata() {
    let metadata = metadata_rich();
    let (mut wb, _) = read_xlsx(&package(&pack(metadata.clone()))).unwrap();
    // A2 is overwritten; A1 is untouched, and B1 never had any.
    wb.sheet_mut(0).unwrap().set_value(at("A2"), Value::Number(8.0));
    wb.sheet_mut(0).unwrap().set_value(at("B1"), Value::Number(2.0));
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    u.assert_valid();
    let sheet = u.text("xl/worksheets/sheet1.xml");
    assert!(sheet.contains(r#"<c r="A1" vm="1" t="e">"#), "{sheet}");
    assert!(sheet.contains(r#"<c r="A2"><v>8</v></c>"#), "{sheet}");
    assert!(!sheet.contains(r#"vm="2""#), "{sheet}");
    assert_rich_parts_written(&u, &metadata);

    // The same value typed again is the value the metadata describes.
    let (mut wb, _) = read_xlsx(&package(&pack(metadata))).unwrap();
    wb.sheet_mut(0).unwrap().set_value(at("A2"), Value::Number(7.0));
    let sheet = Unpacked::new(&write_xlsx(&wb).unwrap()).text("xl/worksheets/sheet1.xml");
    assert!(sheet.contains(r#"<c r="A2" vm="2"><v>7</v></c>"#), "{sheet}");
}

/// A row put in above the cells (as the engine does: the cells, what is kept for them, and the
/// text of the formula whose references moved) takes the value metadata with them: it was looked
/// for at the old addresses, where the cells weren't, so the rich parts and every `vm` were dropped.
#[test]
fn value_metadata_follows_its_cell_when_the_cell_moves() {
    let metadata = metadata_rich();
    let (mut wb, _) = read_xlsx(&package(&pack(metadata.clone()))).unwrap();
    let sheet = wb.sheet_mut(0).unwrap();
    sheet.cells.shift_rows(0, 1);
    sheet.move_cell_sidecars(|c| c.offset(1, 0));
    let mut rewrites = gridcraft_model::FormulaRewrites::of(sheet);
    let a2 = at("A2");
    let mut cell = sheet.cell(a2).unwrap().clone();
    cell.formula = Some(Arc::new(Formula::new("SUM(C2:C3)")));
    sheet.cells.set(a2, cell);
    rewrites.note(a2, "SUM(C1:C2)", "SUM(C2:C3)");
    rewrites.apply(sheet);
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    u.assert_valid();
    assert_rich_parts_written(&u, &metadata);
    let xml = u.text("xl/worksheets/sheet1.xml");
    assert!(xml.contains(r#"<c r="A2" vm="1" t="e"><f>SUM(C2:C3)</f><v>#VALUE!</v></c>"#), "{xml}");
    assert!(xml.contains(r#"<c r="A3" vm="2"><v>7</v></c>"#), "{xml}");
    assert!(xml.contains(r#"<c r="B2"><v>1</v></c>"#), "{xml}");
}

#[test]
fn a_changed_formula_loses_the_value_metadata_of_its_error() {
    let (mut wb, _) = read_xlsx(&package(&pack(metadata_rich()))).unwrap();
    let a1 = at("A1");
    let f = Formula::new("SUM(C1:C3)");
    let mut cell = wb.sheet(0).unwrap().cell(a1).unwrap().clone();
    cell.formula = Some(Arc::new(f));
    wb.sheet_mut(0).unwrap().cells.set(a1, cell);
    let sheet = Unpacked::new(&write_xlsx(&wb).unwrap()).text("xl/worksheets/sheet1.xml");
    assert!(sheet.contains(r#"<c r="A1" t="e"><f>SUM(C1:C3)</f>"#), "{sheet}");
    assert!(sheet.contains(r#"<c r="A2" vm="2">"#), "{sheet}");
}

#[test]
fn rich_values_of_cells_that_are_gone_do_not_stay_in_the_file() {
    let (mut wb, _) = read_xlsx(&package(&pack(metadata_rich()))).unwrap();
    wb.sheet_mut(0).unwrap().set_value(at("A1"), Value::Number(1.0));
    wb.sheet_mut(0).unwrap().set_value(at("A2"), Value::Number(2.0));
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    u.assert_valid();
    assert_no_rich_parts(&u);
    assert!(!u.text("xl/worksheets/sheet1.xml").contains("vm="));
}

#[test]
fn dynamic_array_formulas_share_a_kept_metadata_part_that_has_the_flag() {
    let metadata = metadata_rich_dynamic();
    let (wb, rep) = read_xlsx(&package(&Pack { dynamic_cells: true, ..pack(metadata.clone()) })).unwrap();
    assert!(no_loss(&rep.warnings), "{:?}", rep.warnings);
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    u.assert_valid();
    assert_rich_parts_written(&u, &metadata);
    let sheet = u.text("xl/worksheets/sheet1.xml");
    assert!(sheet.contains(r#"<c r="D1" cm="1">"#), "{sheet}");
    assert!(sheet.contains(r#"<c r="A1" vm="1" t="e">"#), "{sheet}");
    assert!(sheet.contains(r#"<c r="A2" vm="2">"#), "{sheet}");
}

#[test]
fn dynamic_array_formulas_beside_a_kept_part_without_the_flag_get_a_metadata_part_of_their_own() {
    let (wb, _) = read_xlsx(&package(&Pack { dynamic_cells: true, ..pack(metadata_rich()) })).unwrap();
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    u.assert_valid();
    // `cm="1"` has to name the flag: the kept part has none, so the file gets GridCraft's part.
    let sheet = u.text("xl/worksheets/sheet1.xml");
    assert!(sheet.contains(r#"<c r="D1" cm="1">"#), "{sheet}");
    assert!(!sheet.contains("vm="), "{sheet}");
    let metadata = u.text("xl/metadata.xml");
    assert!(metadata.contains("XLDAPR") && !metadata.contains("XLRICHVALUE"), "{metadata}");
    assert!(u.parts.keys().all(|n| !n.starts_with("xl/richData/")), "{:?}", u.parts.keys());
}

#[test]
fn value_metadata_that_points_at_nothing_is_reported() {
    // The structure part is gone: the rich values can't be kept faithfully.
    let (wb, rep) = read_xlsx(&package(&Pack { structures: false, ..pack(metadata_rich()) })).unwrap();
    assert!(rep.warnings.iter().any(|w| w.contains("value metadata")), "{:?}", rep.warnings);
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    u.assert_valid();
    assert_no_rich_parts(&u);
    let sheet = u.text("xl/worksheets/sheet1.xml");
    assert!(!sheet.contains("vm="), "{sheet}");
    assert!(sheet.contains(r#"<c r="A2"><v>7</v></c>"#), "{sheet}");
}

/// A package with a workbook-level Power Pivot data model, the connection to it and the `.data`
/// default content type of the model part.
fn model_package() -> Vec<u8> {
    let types = format!(
        r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="xml" ContentType="application/xml"/><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="data" ContentType="{CT_PREFIX}.model+data"/><Override PartName="/xl/workbook.xml" ContentType="{CT_PREFIX}.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="{CT_PREFIX}.spreadsheetml.worksheet+xml"/><Override PartName="/xl/connections.xml" ContentType="{CT_PREFIX}.spreadsheetml.connections+xml"/></Types>"#
    );
    let workbook = format!(
        r#"<?xml version="1.0"?><workbook xmlns="{NS_MAIN}" xmlns:r="{NS_R}"><sheets><sheet name="Data" sheetId="1" r:id="rId1"/></sheets></workbook>"#
    );
    let root_rels = rels(&[("rId1", "officeDocument", "xl/workbook.xml")]);
    let wb_rels = rels(&[
        ("rId1", "worksheet", "worksheets/sheet1.xml"),
        ("rId2", "connections", "connections.xml"),
        ("rId3", "powerPivotData", "model/item.data"),
    ]);
    let sheet =
        format!(r#"<?xml version="1.0"?><worksheet xmlns="{NS_MAIN}"><sheetData><row r="1"><c r="A1"><v>1</v></c></row></sheetData></worksheet>"#);
    make_zip(&[
        ("[Content_Types].xml", types.as_bytes()),
        ("_rels/.rels", root_rels.as_bytes()),
        ("xl/workbook.xml", workbook.as_bytes()),
        ("xl/_rels/workbook.xml.rels", wb_rels.as_bytes()),
        ("xl/worksheets/sheet1.xml", sheet.as_bytes()),
        ("xl/connections.xml", CONNECTIONS.as_bytes()),
        ("xl/model/item.data", MODEL),
    ])
}

#[test]
fn the_power_pivot_data_model_comes_back_with_its_connection() {
    let (wb, rep) = read_xlsx(&model_package()).unwrap();
    assert!(!rep.warnings.iter().any(|w| w.contains("dropped") || w.contains("model")), "{:?}", rep.warnings);
    let out = write_xlsx(&wb).unwrap();
    let u = Unpacked::new(&out);
    u.assert_valid();
    assert_eq!(u.parts.get("xl/model/item.data").unwrap(), MODEL);
    assert_eq!(u.parts.get("xl/connections.xml").unwrap(), CONNECTIONS.as_bytes());
    assert!(has_rel(&u, &format!("{NS_R}/powerPivotData"), "xl/model/item.data"), "{:?}", u.rels("xl/workbook.xml"));
    assert!(has_rel(&u, &format!("{NS_R}/connections"), "xl/connections.xml"));
    let types = u.text("[Content_Types].xml");
    assert!(types.contains(&format!(r#"ContentType="{CT_PREFIX}.model+data""#)), "{types}");

    // And again.
    let (wb2, _) = read_xlsx(&out).unwrap();
    let again = Unpacked::new(&write_xlsx(&wb2).unwrap());
    assert_eq!(again.parts.get("xl/model/item.data").unwrap(), MODEL);
}

/// A workbook GridCraft writes with one cell picture, with a rich-value types part (and the
/// relationship to it) added the way Excel writes one.
fn picture_package_with_types() -> Vec<u8> {
    use gridcraft_model::CellPicture;
    // A 1x1 PNG header GridCraft accepts.
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend_from_slice(&[0, 0, 0, 13]);
    png.extend_from_slice(b"IHDR");
    png.extend_from_slice(&[0, 0, 0, 1, 0, 0, 0, 1, 8, 2, 0, 0, 0]);
    png.extend_from_slice(&[0, 0, 0, 0]);
    let mut wb = gridcraft_model::Workbook::new();
    wb.sheet_mut(0).unwrap().set_picture(at("A1"), Arc::new(CellPicture { data: png, mime: "image/png".into(), alt: "logo".into() }));
    let written = write_xlsx(&wb).unwrap();
    let u = Unpacked::new(&written);
    let mut parts: Vec<(String, Vec<u8>)> = u.parts.iter().map(|(n, d)| (n.clone(), d.clone())).collect();
    for (name, data) in &mut parts {
        if name == "xl/_rels/workbook.xml.rels" {
            let text = String::from_utf8(std::mem::take(data)).unwrap();
            let rel =
                format!(r#"<Relationship Id="rId99" Type="{MS_2017}/rdRichValueTypes" Target="richData/rdRichValueTypes.xml"/></Relationships>"#);
            *data = text.replace("</Relationships>", &rel).into_bytes();
        }
        if name == "[Content_Types].xml" {
            let text = String::from_utf8(std::mem::take(data)).unwrap();
            let ct =
                r#"<Override PartName="/xl/richData/rdRichValueTypes.xml" ContentType="application/vnd.ms-excel.rdrichvaluetypes+xml"/></Types>"#;
            *data = text.replace("</Types>", ct).into_bytes();
        }
    }
    parts.push(("xl/richData/rdRichValueTypes.xml".into(), RICH_TYPES.as_bytes().to_vec()));
    make_zip(&parts.iter().map(|(n, d)| (n.as_str(), d.as_slice())).collect::<Vec<_>>())
}

#[test]
fn the_rich_value_types_part_goes_back_next_to_the_rich_values_of_cell_pictures() {
    let (wb, rep) = read_xlsx(&picture_package_with_types()).unwrap();
    assert!(no_loss(&rep.warnings), "{:?}", rep.warnings);
    assert_eq!(wb.sheet(0).unwrap().cell_pictures.len(), 1);
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    u.assert_valid();
    assert_eq!(u.parts.get("xl/richData/rdRichValueTypes.xml").unwrap(), RICH_TYPES.as_bytes());
    assert!(has_rel(&u, &format!("{MS_2017}/rdRichValueTypes"), "xl/richData/rdRichValueTypes.xml"), "{:?}", u.rels("xl/workbook.xml"));
    // The pictures' own rich values are GridCraft's.
    assert!(u.text("xl/richData/rdrichvaluestructure.xml").contains("_localImage"));

    // A workbook whose pictures were deleted has no rich data at all, so no types part either.
    let (mut wb, _) = read_xlsx(&picture_package_with_types()).unwrap();
    wb.sheet_mut(0).unwrap().cell_pictures.clear();
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    u.assert_valid();
    assert_no_rich_parts(&u);
}

#[test]
fn another_error_in_the_cell_loses_the_value_metadata_of_the_one_it_had() {
    use gridcraft_core::CellError;
    // A1's rich value is a #CALC! error (errorType 13): the cell reads as #CALC!.
    let (wb, _) = read_xlsx(&package(&pack(metadata_rich()))).unwrap();
    let a1 = at("A1");
    assert_eq!(wb.sheet(0).unwrap().value(a1), Value::Error(CellError::Calc));
    let with = |e: CellError| {
        let mut wb = wb.clone();
        let mut cell = wb.sheet(0).unwrap().cell(a1).unwrap().clone();
        cell.value = Value::Error(e);
        wb.sheet_mut(0).unwrap().cells.set(a1, cell);
        let out = write_xlsx(&wb).unwrap();
        let sheet = Unpacked::new(&out).text("xl/worksheets/sheet1.xml");
        (sheet, read_xlsx(&out).unwrap().0.sheet(0).unwrap().value(a1))
    };
    // Still #CALC!: the metadata comes back with it.
    let (sheet, back) = with(CellError::Calc);
    assert!(sheet.contains(r#"<c r="A1" vm="1" t="e">"#), "{sheet}");
    assert_eq!(back, Value::Error(CellError::Calc));
    // #VALUE! and #SPILL! are written as #VALUE! too, but the #CALC! metadata no longer says what
    // the cell holds.
    for e in [CellError::Value, CellError::Spill] {
        let (sheet, back) = with(e);
        assert!(sheet.contains(r#"<c r="A1" t="e">"#) && !sheet.contains(r#"vm="1""#), "{e:?}: {sheet}");
        assert_eq!(back, Value::Error(CellError::Value), "{e:?}");
    }
}
