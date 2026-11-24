//! Custom views (View › Custom Views): the workbook lists them (`<customWorkbookViews>`) and each
//! sheet holds its part of every view (`<customSheetViews>`), with the printer settings of the
//! view as a part of its own. All of it is kept on save, in the places the schema gives it.

use super::*;

const GUID_LANDSCAPE: &str = "{00000000-0000-0000-0000-0000000000A1}";
const GUID_PLAIN: &str = "{00000000-0000-0000-0000-0000000000A2}";
const PRINTER_MAIN: &[u8] = b"printer settings of the sheet \x00\x01\x02";
const PRINTER_LANDSCAPE: &[u8] = b"printer settings of the Landscape view \x03\x04";
const PRINTER_PLAIN: &[u8] = b"printer settings of the Plain view \x05\x06\x07";

const CONTENT_TYPES: &str = r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="xml" ContentType="application/xml"/><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="bin" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.printerSettings"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/xl/worksheets/sheet2.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#;

/// "Data" holds the filter, merged cells and two custom views, each with printer settings next to
/// the sheet's own. "Views" holds one view with no relationships at all.
fn package(with_plain_printer: bool) -> Vec<u8> {
    let workbook = format!(
        r#"<?xml version="1.0"?><workbook xmlns="{NS_MAIN}" xmlns:r="{NS_R}"><bookViews><workbookView activeTab="0"/></bookViews><sheets><sheet name="Data" sheetId="1" r:id="rId1"/><sheet name="Views" sheetId="2" r:id="rId2"/></sheets><calcPr calcId="191029"/><customWorkbookViews><customWorkbookView name="Landscape" guid="{GUID_LANDSCAPE}" maximized="1" windowWidth="20000" windowHeight="10000" activeSheetId="1"/><customWorkbookView name="Plain" guid="{GUID_PLAIN}" includePrintSettings="0" windowWidth="800" windowHeight="600" activeSheetId="2"/></customWorkbookViews></workbook>"#
    );
    let data = format!(
        r#"<?xml version="1.0"?><worksheet xmlns="{NS_MAIN}" xmlns:r="{NS_R}"><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>a</t></is></c><c r="B1" t="inlineStr"><is><t>b</t></is></c></row><row r="2"><c r="A2"><v>1</v></c></row></sheetData><autoFilter ref="A1:B3"/><customSheetViews><customSheetView guid="{GUID_LANDSCAPE}" scale="85" filter="1" showAutoFilter="1"><pane ySplit="1" topLeftCell="A2" activePane="bottomLeft" state="frozen"/><selection pane="bottomLeft" activeCell="A2" sqref="A2"/><pageMargins left="0.5" right="0.5" top="0.5" bottom="0.5" header="0.3" footer="0.3"/><pageSetup orientation="landscape" r:id="rId2"/><autoFilter ref="A1:B3"/></customSheetView><customSheetView guid="{GUID_PLAIN}"><pageSetup r:id="rId3"/></customSheetView></customSheetViews><mergeCells count="1"><mergeCell ref="A5:B5"/></mergeCells><pageMargins left="0.7" right="0.7" top="0.75" bottom="0.75" header="0.3" footer="0.3"/><pageSetup orientation="portrait" r:id="rId1"/></worksheet>"#
    );
    let views = format!(
        r#"<?xml version="1.0"?><worksheet xmlns="{NS_MAIN}"><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>v</t></is></c></row></sheetData><customSheetViews><customSheetView guid="{GUID_LANDSCAPE}" showGridLines="0"><selection activeCell="B2" sqref="B2"/></customSheetView></customSheetViews></worksheet>"#
    );
    let root_rels = rels(&[("rId1", "officeDocument", "xl/workbook.xml")]);
    let wb_rels = rels(&[("rId1", "worksheet", "worksheets/sheet1.xml"), ("rId2", "worksheet", "worksheets/sheet2.xml")]);
    let data_rels = rels(&[
        ("rId1", "printerSettings", "../printerSettings/printerSettings1.bin"),
        ("rId2", "printerSettings", "../printerSettings/printerSettings2.bin"),
        ("rId3", "printerSettings", "../printerSettings/printerSettings3.bin"),
    ]);
    let mut parts: Vec<(&str, &[u8])> = vec![
        ("[Content_Types].xml", CONTENT_TYPES.as_bytes()),
        ("_rels/.rels", root_rels.as_bytes()),
        ("xl/workbook.xml", workbook.as_bytes()),
        ("xl/_rels/workbook.xml.rels", wb_rels.as_bytes()),
        ("xl/worksheets/sheet1.xml", data.as_bytes()),
        ("xl/worksheets/_rels/sheet1.xml.rels", data_rels.as_bytes()),
        ("xl/worksheets/sheet2.xml", views.as_bytes()),
        ("xl/printerSettings/printerSettings1.bin", PRINTER_MAIN),
        ("xl/printerSettings/printerSettings2.bin", PRINTER_LANDSCAPE),
    ];
    if with_plain_printer {
        parts.push(("xl/printerSettings/printerSettings3.bin", PRINTER_PLAIN));
    }
    make_zip(&parts)
}

/// The relationship id a `<pageSetup>` names, followed to the bytes of the part it targets.
fn printer_bytes(u: &Unpacked, sheet: &str, page_setup: &xml::El) -> Vec<u8> {
    let id = page_setup.attr("id").unwrap_or_else(|| panic!("<pageSetup> without r:id: {page_setup:?}"));
    let (ty, target) = u.target(sheet, id);
    assert!(ty.ends_with("/printerSettings"), "{ty}");
    u.parts.get(&target).unwrap_or_else(|| panic!("{target} is missing")).clone()
}

/// What the written file must hold, however many times it went through GridCraft.
fn assert_views_kept(out: &[u8]) {
    let u = Unpacked::new(out);
    u.assert_valid();
    // The workbook: both views, as they were, right after the calculation properties.
    let wbx = u.text("xl/workbook.xml");
    assert!(wbx.find("<calcPr").unwrap() < wbx.find("<customWorkbookViews").unwrap(), "{wbx}");
    let wb = xml::parse(wbx.as_bytes()).unwrap();
    let views: Vec<(&str, &str, Option<&str>)> = wb
        .child("customWorkbookViews")
        .unwrap()
        .kids("customWorkbookView")
        .map(|v| (v.attr("name").unwrap(), v.attr("guid").unwrap(), v.attr("activeSheetId")))
        .collect();
    assert_eq!(views, [("Landscape", GUID_LANDSCAPE, Some("1")), ("Plain", GUID_PLAIN, Some("2"))]);

    // "Data": the views sit after the filter and before the merged cells.
    let part = sheet_part(&u, "Data");
    let sx = u.text(&part);
    let (filter, custom, merges) = (sx.find("<autoFilter").unwrap(), sx.find("<customSheetViews").unwrap(), sx.find("<mergeCells").unwrap());
    assert!(filter < custom && custom < merges, "{sx}");
    let root = xml::parse(sx.as_bytes()).unwrap();
    let custom_views = root.child("customSheetViews").unwrap();
    let guids: Vec<&str> = custom_views.kids("customSheetView").map(|v| v.attr("guid").unwrap()).collect();
    assert_eq!(guids, [GUID_LANDSCAPE, GUID_PLAIN]);
    let first = custom_views.kids("customSheetView").next().unwrap();
    assert_eq!(first.attr("scale"), Some("85"));
    assert_eq!(first.child("pane").and_then(|p| p.attr("state")), Some("frozen"));
    assert_eq!(first.child("pageSetup").and_then(|p| p.attr("orientation")), Some("landscape"));
    // Each view names its own printer settings, and the sheet's own settings are still its own.
    let landscape = printer_bytes(&u, &part, first.child("pageSetup").unwrap());
    let plain = printer_bytes(&u, &part, custom_views.kids("customSheetView").nth(1).unwrap().child("pageSetup").unwrap());
    let main = printer_bytes(&u, &part, root.child("pageSetup").unwrap());
    assert_eq!((main.as_slice(), landscape.as_slice(), plain.as_slice()), (PRINTER_MAIN, PRINTER_LANDSCAPE, PRINTER_PLAIN));
    let printer_rels: Vec<_> = u.rels(&part).into_iter().filter(|r| r.1.ends_with("/printerSettings")).collect();
    assert_eq!(printer_rels.len(), 3, "{printer_rels:?}");

    // "Views": a view with nothing but settings of its own.
    let sx = u.text(&sheet_part(&u, "Views"));
    let root = xml::parse(sx.as_bytes()).unwrap();
    let view = root.child("customSheetViews").unwrap().child("customSheetView").unwrap();
    assert_eq!((view.attr("guid"), view.attr("showGridLines")), (Some(GUID_LANDSCAPE), Some("0")));
    assert_eq!(view.child("selection").and_then(|s| s.attr("activeCell")), Some("B2"));
}

#[test]
fn custom_views_and_their_printer_settings_survive_a_round_trip() {
    let (wb, rep) = read_xlsx(&package(true)).unwrap();
    assert!(!rep.warnings.iter().any(|w| w.contains("dropped")), "{:?}", rep.warnings);
    let out = write_xlsx(&wb).unwrap();
    assert_views_kept(&out);
    // And again: what was written is read back as the same views.
    let (wb2, rep2) = read_xlsx(&out).unwrap();
    assert!(!rep2.warnings.iter().any(|w| w.contains("dropped")), "{:?}", rep2.warnings);
    assert_views_kept(&write_xlsx(&wb2).unwrap());
}

#[test]
fn a_sheet_without_relationships_keeps_its_custom_views() {
    // "Views" has no relationships part: only the workbook's list of views says it may hold any.
    let (wb, _) = read_xlsx(&package(true)).unwrap();
    let views = wb.sheets.iter().find(|s| s.name == "Views").unwrap();
    let kept = views.preserved.as_deref().expect("the sheet's custom views are kept");
    assert!(kept.rels.is_empty() && kept.parts.is_empty());
    assert_eq!(kept.elements.iter().filter(|e| e.slot == "customSheetViews").count(), 1);
}

#[test]
fn custom_views_whose_printer_settings_are_missing_are_dropped_with_a_warning() {
    // The sheet's `customSheetViews` name a printer settings part that isn't in the package: they
    // are dropped whole (a view with a dangling relationship would make the file need repair),
    // the sheet's own printer settings stay.
    let (wb, rep) = read_xlsx(&package(false)).unwrap();
    let warnings = rep.warnings.join("\n");
    assert!(warnings.contains("sheet \"Data\": customSheetViews were dropped: xl/printerSettings/printerSettings3.bin is missing"), "{warnings}");
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    u.assert_valid();
    let part = sheet_part(&u, "Data");
    let sx = u.text(&part);
    assert!(!sx.contains("<customSheetViews"), "{sx}");
    let root = xml::parse(sx.as_bytes()).unwrap();
    assert_eq!(printer_bytes(&u, &part, root.child("pageSetup").unwrap()), PRINTER_MAIN);
    assert!(u.find(PRINTER_LANDSCAPE).is_empty(), "the part of a dropped view is not written");
    // The other sheet's view and the workbook's list are untouched.
    assert!(u.text(&sheet_part(&u, "Views")).contains("<customSheetView "));
    assert!(u.text("xl/workbook.xml").contains("<customWorkbookViews"));
}
