//! Dialog sheets (Excel 5.0) and Excel 4.0 macro sheets are not worksheets. GridCraft keeps each
//! one as it was, at its place among the sheets, with the VML drawing, the printer settings and the
//! notes behind it. A macro sheet is only written into a macro-enabled workbook, like the VBA
//! project.

use gridcraft_model::PreservedSheetKind;

use super::*;
use crate::{XlsxKind, write_xlsx_as};

const CT_DIALOG: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.dialogsheet+xml";
const CT_MACRO: &str = "application/vnd.ms-excel.macrosheet+xml";
const CT_INTL_MACRO: &str = "application/vnd.ms-excel.intlmacrosheet+xml";
const CT_MACRO_BOOK: &str = "application/vnd.ms-excel.sheet.macroEnabled.main+xml";
const TY_MACRO: &str = "http://schemas.microsoft.com/office/2006/relationships/xlMacrosheet";
const TY_INTL_MACRO: &str = "http://schemas.microsoft.com/office/2006/relationships/xlIntlMacrosheet";
const NS_XM: &str = "http://schemas.microsoft.com/office/excel/2006/main";

/// A dialog sheet's drawing: one button that runs a macro.
const FORM_VML: &str = r##"<xml xmlns:v="urn:schemas-microsoft-com:vml" xmlns:o="urn:schemas-microsoft-com:office:office" xmlns:x="urn:schemas-microsoft-com:office:excel"><o:shapelayout v:ext="edit"><o:idmap v:ext="edit" data="1"/></o:shapelayout><v:shapetype id="_x0000_t201" coordsize="21600,21600" o:spt="201" path="m,l,21600r21600,l21600,xe"/><v:shape id="_x0000_s1025" type="#_x0000_t201" style='position:absolute;width:60pt;height:20pt'><x:ClientData ObjectType="Button"><x:Anchor>1, 0, 1, 0, 3, 0, 3, 0</x:Anchor><x:FmlaMacro>[0]!OK_Click</x:FmlaMacro></x:ClientData></v:shape></xml>"##;
/// The note of worksheet "Data", drawn in the same block of shape ids as the dialog sheet's button.
const DATA_NOTES_VML: &str = r##"<xml xmlns:v="urn:schemas-microsoft-com:vml" xmlns:o="urn:schemas-microsoft-com:office:office" xmlns:x="urn:schemas-microsoft-com:office:excel"><o:shapelayout v:ext="edit"><o:idmap v:ext="edit" data="1"/></o:shapelayout><v:shapetype id="_x0000_t202" coordsize="21600,21600" o:spt="202" path="m,l,21600r21600,l21600,xe"><v:stroke joinstyle="miter"/></v:shapetype><v:shape id="_x0000_s1025" type="#_x0000_t202" style='position:absolute;visibility:hidden'><v:textbox><div>note</div></v:textbox><x:ClientData ObjectType="Note"><x:Row>0</x:Row><x:Column>0</x:Column></x:ClientData></v:shape></xml>"##;

fn dialog_xml(note: &str) -> String {
    format!(
        r#"<?xml version="1.0"?><dialogsheet xmlns="{NS_MAIN}" xmlns:r="{NS_R}"><sheetViews><sheetView workbookViewId="0"/></sheetViews><sheetFormatPr defaultRowHeight="15"/><pageSetup r:id="rId2"/><legacyDrawing r:id="rId1"/><!-- {note} --></dialogsheet>"#
    )
}

fn macro_xml(note: &str) -> String {
    format!(
        r#"<?xml version="1.0"?><xm:macrosheet xmlns:xm="{NS_XM}"><xm:sheetData><xm:row r="1"><xm:c r="A1" t="str"><xm:f>RETURN()</xm:f><xm:v>{note}</xm:v></xm:c></xm:row></xm:sheetData></xm:macrosheet>"#
    )
}

/// The sheet part, its relationships (if any) and the matching content-type override.
fn content_types() -> String {
    format!(
        r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="xml" ContentType="application/xml"/><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="vml" ContentType="application/vnd.openxmlformats-officedocument.vmlDrawing"/><Default Extension="bin" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.printerSettings"/><Override PartName="/xl/workbook.xml" ContentType="{CT_MACRO_BOOK}"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="{CT_PREFIX}.spreadsheetml.worksheet+xml"/><Override PartName="/xl/worksheets/sheet2.xml" ContentType="{CT_PREFIX}.spreadsheetml.worksheet+xml"/><Override PartName="/xl/comments1.xml" ContentType="{CT_PREFIX}.spreadsheetml.comments+xml"/><Override PartName="/xl/dialogsheets/sheet1.xml" ContentType="{CT_DIALOG}"/><Override PartName="/xl/dialogsheets/sheet2.xml" ContentType="{CT_DIALOG}"/><Override PartName="/xl/macrosheets/sheet1.xml" ContentType="{CT_MACRO}"/><Override PartName="/xl/intlmacrosheets/sheet1.xml" ContentType="{CT_INTL_MACRO}"/></Types>"#
    )
}

/// Sheets in file order: Data (a worksheet with a note), Form (a dialog sheet with a button and
/// printer settings), More (a worksheet with a print area), Macro1 (a very hidden macro sheet),
/// Intl (an international macro sheet) and Last (a hidden dialog sheet). The active tab is "More".
/// `form_drawing` leaves out the part the dialog sheet "Form" draws its button from.
fn package_with(form_drawing: bool) -> Vec<u8> {
    let workbook = format!(
        r#"<?xml version="1.0"?><workbook xmlns="{NS_MAIN}" xmlns:r="{NS_R}"><bookViews><workbookView activeTab="2"/></bookViews><sheets><sheet name="Data" sheetId="1" r:id="rId1"/><sheet name="Form" sheetId="2" r:id="rId2"/><sheet name="More" sheetId="3" r:id="rId3"/><sheet name="Macro1" sheetId="7" state="veryHidden" r:id="rId4"/><sheet name="Intl" sheetId="8" r:id="rId5"/><sheet name="Last" sheetId="9" state="hidden" r:id="rId6"/></sheets><definedNames><definedName name="_xlnm.Print_Area" localSheetId="2">More!$A$1:$B$2</definedName></definedNames></workbook>"#
    );
    let data = format!(
        r#"<?xml version="1.0"?><worksheet xmlns="{NS_MAIN}" xmlns:r="{NS_R}"><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>data</t></is></c></row></sheetData><pageMargins left="0.7" right="0.7" top="0.75" bottom="0.75" header="0.3" footer="0.3"/><legacyDrawing r:id="rId1"/></worksheet>"#
    );
    let more = worksheet_xml("more");
    let (form, last) = (dialog_xml("form"), dialog_xml("last"));
    let (macro1, intl) = (macro_xml("macro"), macro_xml("intl"));
    let types = content_types();
    let root_rels = rels(&[("rId1", "officeDocument", "xl/workbook.xml")]);
    let wb_rels = rels(&[
        ("rId1", "worksheet", "worksheets/sheet1.xml"),
        ("rId2", "dialogsheet", "dialogsheets/sheet1.xml"),
        ("rId3", "worksheet", "worksheets/sheet2.xml"),
        ("rId4", TY_MACRO, "macrosheets/sheet1.xml"),
        ("rId5", TY_INTL_MACRO, "intlmacrosheets/sheet1.xml"),
        ("rId6", "dialogsheet", "dialogsheets/sheet2.xml"),
    ]);
    let data_rels = rels(&[("rId1", "vmlDrawing", "../drawings/vmlDrawing1.vml"), ("rId2", "comments", "../comments1.xml")]);
    let form_rels =
        rels(&[("rId1", "vmlDrawing", "../drawings/vmlDrawing2.vml"), ("rId2", "printerSettings", "../printerSettings/printerSettings1.bin")]);
    // "Last" has no drawing, so it has no relationships; but it has to name nothing either.
    let last = last.replace("<pageSetup r:id=\"rId2\"/><legacyDrawing r:id=\"rId1\"/>", "");
    let mut parts: Vec<(&str, &[u8])> = vec![
        ("[Content_Types].xml", types.as_bytes()),
        ("_rels/.rels", root_rels.as_bytes()),
        ("xl/workbook.xml", workbook.as_bytes()),
        ("xl/_rels/workbook.xml.rels", wb_rels.as_bytes()),
        ("xl/worksheets/sheet1.xml", data.as_bytes()),
        ("xl/worksheets/_rels/sheet1.xml.rels", data_rels.as_bytes()),
        ("xl/worksheets/sheet2.xml", more.as_bytes()),
        ("xl/comments1.xml", COMMENTS.as_bytes()),
        ("xl/drawings/vmlDrawing1.vml", DATA_NOTES_VML.as_bytes()),
        ("xl/dialogsheets/sheet1.xml", form.as_bytes()),
        ("xl/dialogsheets/_rels/sheet1.xml.rels", form_rels.as_bytes()),
        ("xl/dialogsheets/sheet2.xml", last.as_bytes()),
        ("xl/printerSettings/printerSettings1.bin", PRINTER),
        ("xl/macrosheets/sheet1.xml", macro1.as_bytes()),
        ("xl/intlmacrosheets/sheet1.xml", intl.as_bytes()),
    ];
    if form_drawing {
        parts.push(("xl/drawings/vmlDrawing2.vml", FORM_VML.as_bytes()));
    }
    make_zip(&parts)
}

fn package() -> Vec<u8> {
    package_with(true)
}

/// The relationship type and part of the written sheet `name`, found through the workbook.
fn kept_sheet(u: &Unpacked, name: &str) -> (String, String) {
    let wb = xml::parse(u.parts.get("xl/workbook.xml").unwrap()).unwrap();
    let id = wb
        .child("sheets")
        .unwrap()
        .kids("sheet")
        .find(|s| s.attr("name") == Some(name))
        .unwrap_or_else(|| panic!("no sheet {name}"))
        .attr("id")
        .unwrap()
        .to_string();
    u.target("xl/workbook.xml", &id)
}

/// The content type a written part is declared with: its override, or the default for its extension.
fn content_type_of(u: &Unpacked, part: &str) -> String {
    let types = xml::parse(u.parts.get("[Content_Types].xml").unwrap()).unwrap();
    if let Some(o) = types.kids("Override").find(|o| o.attr("PartName").is_some_and(|p| normalize(p) == normalize(part))) {
        return o.attr("ContentType").unwrap().to_string();
    }
    let ext = part.rsplit('.').next().unwrap().to_ascii_lowercase();
    let default = types.kids("Default").find(|d| d.attr("Extension").is_some_and(|e| e.eq_ignore_ascii_case(&ext)));
    default.and_then(|d| d.attr("ContentType")).unwrap_or_else(|| panic!("{part} has no content type")).to_string()
}

fn names_of(u: &Unpacked) -> Vec<String> {
    written_sheets(u).into_iter().map(|(name, _)| name).collect()
}

fn no_loss(warnings: &[String]) -> bool {
    !warnings.iter().any(|w| w.contains("skipped") || w.contains("dropped"))
}

#[test]
fn dialog_and_macro_sheets_are_kept_not_skipped() {
    let (wb, rep) = read_xlsx(&package()).unwrap();
    assert!(no_loss(&rep.warnings), "{:?}", rep.warnings);
    let names: Vec<&str> = wb.sheets.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["Data", "More"], "only the worksheets are modelled");
    let kept: Vec<_> = wb.preserved.as_deref().unwrap().sheets.iter().map(|s| (s.name.as_str(), s.kind, s.before)).collect();
    assert_eq!(
        kept,
        [
            ("Form", PreservedSheetKind::Dialog, 1),
            ("Macro1", PreservedSheetKind::Macro, 2),
            ("Intl", PreservedSheetKind::Macro, 2),
            ("Last", PreservedSheetKind::Dialog, 2),
        ]
    );
}

#[test]
fn kept_sheets_survive_a_round_trip_in_place() {
    let (wb, _) = read_xlsx(&package()).unwrap();
    // A workbook holding macro sheets is saved as a macro-enabled one.
    let out = write_xlsx(&wb).unwrap();
    let u = Unpacked::new(&out);
    u.assert_valid();
    assert_eq!(content_type_of(&u, "xl/workbook.xml"), CT_MACRO_BOOK);
    // Same order, same visibility, the way Excel shows the tabs.
    assert_eq!(
        written_sheets(&u),
        [("Data", ""), ("Form", ""), ("More", ""), ("Macro1", "veryHidden"), ("Intl", ""), ("Last", "hidden")]
            .map(|(a, b)| (a.to_string(), b.to_string())),
    );
    // Sheet ids are unique; the file's positions, not the model's, are what the active tab and the
    // print area count.
    let wbx = u.text("xl/workbook.xml");
    let mut ids: Vec<String> = wbx.match_indices("<sheet ").map(|(i, _)| attr_of(&wbx[i..], "sheet", "sheetId")).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 6, "{wbx}");
    assert_eq!(attr_of(&wbx, "workbookView", "activeTab"), "2", "More is the third tab");
    assert_eq!(attr_of(&wbx, "definedName", "localSheetId"), "2", "{wbx}");

    // Every sheet part is back byte for byte, under the relationship type and content type the
    // file gave it, and keeps the folder Excel keeps it in.
    let expected = [
        ("Form", "/dialogsheet", CT_DIALOG, "xl/dialogsheets/sheet1.xml", dialog_xml("form")),
        ("Macro1", "/xlMacrosheet", CT_MACRO, "xl/macrosheets/sheet1.xml", macro_xml("macro")),
        ("Intl", "/xlIntlMacrosheet", CT_INTL_MACRO, "xl/intlmacrosheets/sheet1.xml", macro_xml("intl")),
    ];
    for (name, ty, ct, part, xml_text) in expected {
        let (got_ty, got_part) = kept_sheet(&u, name);
        assert!(got_ty.ends_with(ty), "{name}: {got_ty}");
        assert_eq!(got_part, part, "{name}");
        assert_eq!(content_type_of(&u, &got_part), ct, "{name}");
        assert_eq!(u.text(&got_part), xml_text, "{name}");
    }
    // The macro sheets keep the Microsoft relationship namespace Excel writes them in.
    assert_eq!(u.rels("xl/workbook.xml").into_iter().find(|r| r.2 == "xl/macrosheets/sheet1.xml").unwrap().1, TY_MACRO);
    assert_eq!(u.rels("xl/workbook.xml").into_iter().find(|r| r.2 == "xl/intlmacrosheets/sheet1.xml").unwrap().1, TY_INTL_MACRO);

    // The dialog sheet's button is drawn from its VML drawing, which stays as it was, and its
    // printer settings follow `<pageSetup r:id>`.
    let (_, form) = kept_sheet(&u, "Form");
    let fx = u.text(&form);
    let (ty, vml) = u.target(&form, &attr_of(&fx, "legacyDrawing", "r:id"));
    assert!(ty.ends_with("/vmlDrawing"), "{ty}");
    assert_eq!(u.text(&vml), FORM_VML);
    assert_eq!(content_type_of(&u, &vml), "application/vnd.openxmlformats-officedocument.vmlDrawing");
    let (ty, ps) = u.target(&form, &attr_of(&fx, "pageSetup", "r:id"));
    assert!(ty.ends_with("/printerSettings"), "{ty}");
    assert_eq!(u.parts.get(&ps).unwrap(), PRINTER);
    // A dialog sheet with no drawing needs no relationships.
    let (_, last) = kept_sheet(&u, "Last");
    assert!(u.rels(&last).is_empty());

    // Reading the output again finds the same six sheets, and a second save is stable.
    let (wb2, rep2) = read_xlsx(&out).unwrap();
    assert!(no_loss(&rep2.warnings), "{:?}", rep2.warnings);
    assert_eq!(wb2.preserved.as_deref().unwrap().sheets.len(), 4);
    let again = Unpacked::new(&write_xlsx(&wb2).unwrap());
    again.assert_valid();
    assert_eq!(written_sheets(&again), written_sheets(&u));
    for name in ["Form", "Macro1", "Intl", "Last"] {
        assert_eq!(again.text(&kept_sheet(&again, name).1), u.text(&kept_sheet(&u, name).1), "{name}");
    }
}

#[test]
fn a_macro_sheet_is_only_written_into_a_macro_enabled_workbook() {
    let (wb, _) = read_xlsx(&package()).unwrap();
    // Macro-free packages can't hold an Excel 4.0 macro sheet (Excel refuses the file), so they
    // are left out with the parts only they reach. Dialog sheets stay.
    for kind in [XlsxKind::Workbook, XlsxKind::Template] {
        let u = Unpacked::new(&write_xlsx_as(&wb, kind).unwrap());
        u.assert_valid();
        assert_eq!(names_of(&u), ["Data", "Form", "More", "Last"], "{kind:?}");
        assert!(!u.parts.keys().any(|k| k.contains("macrosheets")), "{kind:?}: {:?}", u.parts.keys());
        // The positions still count the sheets that are there.
        let wbx = u.text("xl/workbook.xml");
        assert_eq!(attr_of(&wbx, "workbookView", "activeTab"), "2", "{kind:?}");
        assert_eq!(attr_of(&wbx, "definedName", "localSheetId"), "2", "{kind:?}");
    }
    for kind in [XlsxKind::MacroWorkbook, XlsxKind::MacroTemplate] {
        let u = Unpacked::new(&write_xlsx_as(&wb, kind).unwrap());
        u.assert_valid();
        assert_eq!(names_of(&u), ["Data", "Form", "More", "Macro1", "Intl", "Last"], "{kind:?}");
    }
    // Once the macro sheets are gone, a plain `.xlsx` is what `write_xlsx` writes.
    let (plain, _) = read_xlsx(&write_xlsx_as(&wb, XlsxKind::Workbook).unwrap()).unwrap();
    let u = Unpacked::new(&write_xlsx(&plain).unwrap());
    assert!(content_type_of(&u, "xl/workbook.xml").ends_with("spreadsheetml.sheet.main+xml"));
    assert_eq!(names_of(&u), ["Data", "Form", "More", "Last"]);
    // The save says what it left out; a macro-enabled one leaves nothing out.
    let (_, report) = crate::write_xlsx_named_with(&wb, XlsxKind::Workbook, "book.xlsx", crate::FunctionInfo::default()).unwrap();
    assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
    assert!(report.warnings[0].contains("macro sheets were left out"), "{:?}", report.warnings);
    let (_, report) = crate::write_xlsx_named_with(&wb, XlsxKind::MacroWorkbook, "book.xlsm", crate::FunctionInfo::default()).unwrap();
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
}

/// The shape ids of a dialog sheet's drawing are kept, so the notes GridCraft writes for a
/// worksheet go in another block of ids.
#[test]
fn notes_do_not_reuse_the_shape_ids_of_a_kept_dialog_drawing() {
    let (wb, _) = read_xlsx(&package()).unwrap();
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    let data = sheet_part(&u, "Data");
    let (_, notes) = u.target(&data, &attr_of(&u.text(&data), "legacyDrawing", "r:id"));
    let (idmap, spids) = vml_blocks(&u.text(&notes));
    assert_eq!((idmap, spids), (vec![2], vec![2]), "the dialog drawing claims block 1");
    let (_, form) = kept_sheet(&u, "Form");
    let (_, vml) = u.target(&form, &attr_of(&u.text(&form), "legacyDrawing", "r:id"));
    assert_eq!(vml_blocks(&u.text(&vml)), (vec![1], vec![1]));
}

#[test]
fn a_dialog_sheet_whose_drawing_is_missing_is_dropped_with_a_warning() {
    let (wb, rep) = read_xlsx(&package_with(false)).unwrap();
    let said: Vec<&String> = rep.warnings.iter().filter(|w| w.contains("dialog sheet \"Form\" was dropped")).collect();
    assert_eq!(said.len(), 1, "{:?}", rep.warnings);
    assert!(said[0].contains("vmlDrawing2.vml is missing"), "{said:?}");
    // The others are untouched.
    let kept: Vec<&str> = wb.preserved.as_deref().unwrap().sheets.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(kept, ["Macro1", "Intl", "Last"]);
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    u.assert_valid();
    assert_eq!(names_of(&u), ["Data", "More", "Macro1", "Intl", "Last"]);
}

#[test]
fn a_worksheet_that_takes_a_kept_sheets_name_replaces_it() {
    let (mut wb, _) = read_xlsx(&package()).unwrap();
    // Sheet names are unique, case-insensitively: the sheet GridCraft models wins, and the parts
    // only the dialog sheet reached go with it.
    std::sync::Arc::make_mut(&mut wb.sheets[1]).name = "FORM".into();
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    u.assert_valid();
    assert_eq!(names_of(&u), ["Data", "FORM", "Macro1", "Intl", "Last"]);
    assert!(u.find(FORM_VML.as_bytes()).is_empty() && u.find(PRINTER).is_empty(), "{:?}", u.parts.keys());
}

#[test]
fn kept_sheets_are_reported_when_nothing_can_be_kept() {
    // Without readable content types no part can be kept: each sheet that is lost says so.
    let u = Unpacked::new(&package());
    let parts: Vec<(&str, &[u8])> =
        u.parts.iter().map(|(k, v)| (k.as_str(), if k == "[Content_Types].xml" { b"<Types".as_slice() } else { v.as_slice() })).collect();
    let (wb, rep) = read_xlsx(&make_zip(&parts)).unwrap();
    assert!(wb.preserved.is_none());
    for (what, name) in [("dialog sheet", "Form"), ("macro sheet", "Macro1"), ("macro sheet", "Intl"), ("dialog sheet", "Last")] {
        let said = format!("{what} \"{name}\" was skipped and is not kept on save");
        assert!(rep.warnings.contains(&said), "{said}: {:?}", rep.warnings);
    }
}

/// Excel writes an Excel 5.0 VBA module sheet as a `<sheet>` entry with an empty `r:id` and no
/// part. The entry is kept in place (name, state, `sheetId`), with no warning, so the sheets after
/// it keep their positions (`localSheetId`) and a plain workbook (no VBA project) can hold it.
#[test]
fn module_sheet_entries_are_kept_in_place() {
    let workbook = format!(
        r#"<?xml version="1.0"?><workbook xmlns="{NS_MAIN}" xmlns:r="{NS_R}"><sheets><sheet name="Data" sheetId="1" r:id="rId1"/><sheet name="Module1" sheetId="4" state="veryHidden" r:id=""/><sheet name="More" sheetId="2" r:id="rId2"/></sheets><definedNames><definedName name="_xlnm.Print_Area" localSheetId="2">More!$A$1:$B$2</definedName></definedNames></workbook>"#
    );
    let (data, more) = (worksheet_xml("data"), worksheet_xml("more"));
    let types = content_types();
    let root_rels = rels(&[("rId1", "officeDocument", "xl/workbook.xml")]);
    let wb_rels = rels(&[("rId1", "worksheet", "worksheets/sheet1.xml"), ("rId2", "worksheet", "worksheets/sheet2.xml")]);
    let bytes = make_zip(&[
        ("[Content_Types].xml", types.as_bytes()),
        ("_rels/.rels", root_rels.as_bytes()),
        ("xl/workbook.xml", workbook.as_bytes()),
        ("xl/_rels/workbook.xml.rels", wb_rels.as_bytes()),
        ("xl/worksheets/sheet1.xml", data.as_bytes()),
        ("xl/worksheets/sheet2.xml", more.as_bytes()),
    ]);
    let (wb, rep) = read_xlsx(&bytes).unwrap();
    assert!(no_loss(&rep.warnings) && !rep.warnings.iter().any(|w| w.contains("no part")), "{:?}", rep.warnings);
    assert_eq!(wb.sheets.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["Data", "More"]);
    assert!(wb.sheet(1).unwrap().print.print_area.is_some(), "localSheetId 2 is More");
    let kept: Vec<_> = wb.preserved.as_deref().unwrap().sheets.iter().map(|s| (s.name.as_str(), s.kind, s.before, s.sheet_id)).collect();
    assert_eq!(kept, [("Module1", PreservedSheetKind::Module, 1, 4)]);

    let out = write_xlsx(&wb).unwrap();
    let u = Unpacked::new(&out);
    u.assert_valid();
    assert_eq!(content_type_of(&u, "xl/workbook.xml"), "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml");
    assert_eq!(written_sheets(&u), [("Data", ""), ("Module1", "veryHidden"), ("More", "")].map(|(a, b)| (a.to_string(), b.to_string())));
    let wbx = u.text("xl/workbook.xml");
    assert!(wbx.contains(r#"<sheet name="Module1" sheetId="4" state="veryHidden" r:id=""/>"#), "{wbx}");
    assert_eq!(attr_of(&wbx, "definedName", "localSheetId"), "2", "{wbx}");
    let (again, rep) = read_xlsx(&out).unwrap();
    assert!(rep.warnings.is_empty(), "{:?}", rep.warnings);
    assert_eq!(again.preserved.as_deref().unwrap().sheets, wb.preserved.as_deref().unwrap().sheets);

    // A worksheet that takes the module's name replaces its entry.
    let mut renamed = wb.clone();
    std::sync::Arc::make_mut(&mut renamed.sheets[1]).name = "Module1".into();
    let u = Unpacked::new(&write_xlsx(&renamed).unwrap());
    assert_eq!(names_of(&u), ["Data", "Module1"]);
}

/// A macro-enabled workbook whose macro sheet "Macro1" (file sheet 1, between the worksheets
/// "Data" and "More") has text cells in the workbook's shared strings and a formula that uses a
/// name scoped to it. `macro1` is the macro sheet's XML.
fn macro_strings_book(macro1: &str) -> Vec<u8> {
    macro_book_with(macro1, &[])
}

/// [`macro_strings_book`] with more workbook parts: `(relationship id, type, target under xl/,
/// content)`.
fn macro_book_with(macro1: &str, extra: &[(&str, &str, &str, &str)]) -> Vec<u8> {
    macro_book_named(macro1, extra, MACRO_BOOK_NAMES)
}

/// The defined names of [`macro_strings_book`]: three scoped to "Macro1" (file sheet 1), "Data"
/// (file sheet 0), and the workbook.
const MACRO_BOOK_NAMES: &str = r#"<definedName name="Rate" localSheetId="1">12</definedName><definedName name="Auto_Open" localSheetId="1" hidden="1" function="1" xlm="1">Macro1!$A$3</definedName><definedName name="Rate" localSheetId="0">7</definedName><definedName name="Rate">3</definedName>"#;

/// [`macro_book_with`] with `names` as the workbook's `<definedName>` elements.
fn macro_book_named(macro1: &str, extra: &[(&str, &str, &str, &str)], names: &str) -> Vec<u8> {
    let workbook = format!(
        r#"<?xml version="1.0"?><workbook xmlns="{NS_MAIN}" xmlns:r="{NS_R}"><sheets><sheet name="Data" sheetId="1" r:id="rId1"/><sheet name="Macro1" sheetId="2" r:id="rId4"/><sheet name="More" sheetId="3" r:id="rId3"/></sheets><definedNames>{names}</definedNames></workbook>"#
    );
    let data = format!(
        r#"<?xml version="1.0"?><worksheet xmlns="{NS_MAIN}"><sheetData><row r="1"><c r="A1" t="s"><v>0</v></c></row></sheetData></worksheet>"#
    );
    let more = worksheet_xml("more");
    // One plain string only the macro sheet uses, one it shares with "Data", one with a reading.
    let sst = format!(
        r#"<?xml version="1.0"?><sst xmlns="{NS_MAIN}" count="4" uniqueCount="3"><si><t>visible</t></si><si><t>macro-only</t></si><si><t>東京</t><rPh sb="0" eb="2"><t>トウキョウ</t></rPh><phoneticPr fontId="0" type="Hiragana"/></si></sst>"#
    );
    let types = content_types();
    let root_rels = rels(&[("rId1", "officeDocument", "xl/workbook.xml")]);
    let mut wb_rels_list = vec![
        ("rId1", "worksheet", "worksheets/sheet1.xml"),
        ("rId3", "worksheet", "worksheets/sheet2.xml"),
        ("rId4", TY_MACRO, "macrosheets/sheet1.xml"),
        ("rId9", "sharedStrings", "sharedStrings.xml"),
    ];
    wb_rels_list.extend(extra.iter().map(|(id, ty, target, _)| (*id, *ty, *target)));
    let wb_rels = rels(&wb_rels_list);
    let mut parts: Vec<(&str, &[u8])> = vec![
        ("[Content_Types].xml", types.as_bytes()),
        ("_rels/.rels", root_rels.as_bytes()),
        ("xl/workbook.xml", workbook.as_bytes()),
        ("xl/_rels/workbook.xml.rels", wb_rels.as_bytes()),
        ("xl/worksheets/sheet1.xml", data.as_bytes()),
        ("xl/worksheets/sheet2.xml", more.as_bytes()),
        ("xl/sharedStrings.xml", sst.as_bytes()),
        ("xl/macrosheets/sheet1.xml", macro1.as_bytes()),
    ];
    let names: Vec<String> = extra.iter().map(|(_, _, target, _)| format!("xl/{target}")).collect();
    for ((_, _, _, content), name) in extra.iter().zip(&names) {
        parts.push((name, content.as_bytes()));
    }
    make_zip(&parts)
}

const MACRO_WITH_STRINGS: &str = r#"<?xml version="1.0"?><xm:macrosheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:xm="http://schemas.microsoft.com/office/excel/2006/main"><sheetData><row r="1"><c r="A1" t="s"><v>1</v></c><c r="B1" t="s" s="0"><v>0</v></c><c r="C1" t="s"><v> 2 </v></c><c r="D1"><v>1</v></c></row><row r="2"><c r="A2"><f>Rate</f><v>12</v></c></row><row r="3"><c r="A3" t="str"><f>RETURN()</f><v>1</v></c></row></sheetData></xm:macrosheet>"#;

/// The shared strings of a written package, with their readings.
fn written_strings(u: &Unpacked) -> Vec<(String, Option<String>)> {
    let sst = xml::parse(u.parts.get("xl/sharedStrings.xml").unwrap()).unwrap();
    sst.kids("si").map(|si| (xml::rich_text(si), si.child("rPh").and_then(|r| r.child("t")).map(|t| t.text.clone()))).collect()
}

/// The text cells of a written macro sheet, resolved against the written shared strings.
fn macro_texts(u: &Unpacked, sheet: &str) -> Vec<(String, String, Option<String>)> {
    let strings = written_strings(u);
    let part = xml::parse(u.parts.get(&kept_sheet(u, sheet).1).unwrap()).unwrap();
    let rows = part.child("sheetData").unwrap().kids("row");
    let cells = rows.flat_map(|r| r.kids("c")).filter(|c| c.attr("t") == Some("s"));
    cells
        .map(|c| {
            let i: usize = c.child("v").unwrap().text.trim().parse().unwrap();
            let (text, reading) = strings.get(i).cloned().unwrap_or_else(|| panic!("{:?} points past the shared strings", c.attr("r")));
            (c.attr("r").unwrap().to_string(), text, reading)
        })
        .collect()
}

#[test]
fn a_kept_macro_sheets_text_cells_keep_their_shared_strings() {
    let (wb, rep) = read_xlsx(&macro_strings_book(MACRO_WITH_STRINGS)).unwrap();
    assert!(no_loss(&rep.warnings), "{:?}", rep.warnings);
    let expected = [
        ("A1".to_string(), "macro-only".to_string(), None),
        ("B1".to_string(), "visible".to_string(), None),
        ("C1".to_string(), "東京".to_string(), Some("トウキョウ".to_string())),
    ];
    let out = write_xlsx(&wb).unwrap();
    let u = Unpacked::new(&out);
    u.assert_valid();
    assert_eq!(macro_texts(&u, "Macro1"), expected);
    // The worksheet's text is still its own; numbers and formula results are not strings.
    let data = xml::parse(u.parts.get(&sheet_part(&u, "Data")).unwrap()).unwrap();
    let a1 = data.child("sheetData").unwrap().child("row").unwrap().child("c").unwrap();
    assert_eq!(written_strings(&u)[a1.child("v").unwrap().text.parse::<usize>().unwrap()].0, "visible");
    let mx = u.text(&kept_sheet(&u, "Macro1").1);
    assert!(mx.contains(r#"<c r="D1"><v>1</v></c>"#) && mx.contains(r#"<c r="A3" t="str"><f>RETURN()</f><v>1</v></c>"#), "{mx}");
    let sst = u.text("xl/sharedStrings.xml");
    // Three macro-sheet cells, Data!A1 and More!A1.
    assert_eq!(attr_of(&sst, "sst", "count"), "5", "{sst}");
    // A second open and save finds the same texts.
    let (again, rep) = read_xlsx(&out).unwrap();
    assert!(no_loss(&rep.warnings), "{:?}", rep.warnings);
    assert_eq!(macro_texts(&Unpacked::new(&write_xlsx(&again).unwrap()), "Macro1"), expected);
    // The worksheet's text changing doesn't change the macro sheet's.
    let mut edited = wb.clone();
    edited.sheet_mut(0).unwrap().set_value(gridcraft_core::CellRef::parse("A1").unwrap(), gridcraft_core::Value::Text("other".into()));
    assert_eq!(macro_texts(&Unpacked::new(&write_xlsx(&edited).unwrap()), "Macro1"), expected);
}

#[test]
fn names_scoped_to_a_kept_macro_sheet_stay_with_it() {
    let (wb, rep) = read_xlsx(&macro_strings_book(MACRO_WITH_STRINGS)).unwrap();
    assert!(no_loss(&rep.warnings), "{:?}", rep.warnings);
    let names = |u: &Unpacked| -> Vec<(String, String, String, String)> {
        let wbx = xml::parse(u.parts.get("xl/workbook.xml").unwrap()).unwrap();
        let sheets: Vec<String> = wbx.child("sheets").unwrap().kids("sheet").map(|s| s.attr("name").unwrap().to_string()).collect();
        let mut v: Vec<_> = wbx
            .child("definedNames")
            .map(|d| {
                d.kids("definedName")
                    .map(|n| {
                        let scope = n.attr_u32("localSheetId").map_or(String::new(), |i| sheets[i as usize].clone());
                        let flags = ["hidden", "function", "xlm"].iter().filter(|k| n.attr(k) == Some("1")).copied().collect::<Vec<_>>().join(",");
                        (n.attr("name").unwrap().to_string(), scope, n.text.clone(), flags)
                    })
                    .collect()
            })
            .unwrap_or_default();
        v.sort();
        v
    };
    let s = |a: &str, b: &str, c: &str, d: &str| (a.to_string(), b.to_string(), c.to_string(), d.to_string());
    let all = vec![
        s("Auto_Open", "Macro1", "Macro1!$A$3", "hidden,function,xlm"),
        s("Rate", "", "3", ""),
        s("Rate", "Data", "7", ""),
        s("Rate", "Macro1", "12", ""),
    ];
    let out = write_xlsx(&wb).unwrap();
    let u = Unpacked::new(&out);
    u.assert_valid();
    assert_eq!(names(&u), all);
    // A second open and save keeps them where they are.
    let (again, rep) = read_xlsx(&out).unwrap();
    assert!(no_loss(&rep.warnings), "{:?}", rep.warnings);
    assert_eq!(names(&Unpacked::new(&write_xlsx(&again).unwrap())), all);
    // Without the macro sheet (a macro-free save) its names go with it.
    let u = Unpacked::new(&write_xlsx_as(&wb, XlsxKind::Workbook).unwrap());
    assert_eq!(names(&u), vec![s("Rate", "", "3", ""), s("Rate", "Data", "7", "")]);
}

/// The names scoped to a kept macro sheet name the workbook's sheets and cells like its own names
/// do, so the edits that rename a sheet, delete it or move the cells they name rewrite them
/// (`rewrite_kept_names`). A name an edit leaves alone stays as the file spelled it, and every
/// name keeps its other attributes.
#[test]
fn names_scoped_to_a_kept_macro_sheet_follow_the_sheets_and_cells_they_name() {
    use gridcraft_formula::adjust::{self, Axis, Edit};
    let names = concat!(
        r#"<definedName name="Rate" localSheetId="1">Data!$A$1</definedName>"#,
        r#"<definedName name="Total" localSheetId="1" hidden="1" comment="a &amp; b">SUM( Data!$B$2:$B$9 , More!$A$1 )*2</definedName>"#,
        r#"<definedName name="Own" localSheetId="1" function="1" xlm="1">Macro1!$A$3</definedName>"#,
        r#"<definedName name="Text" localSheetId="1">"Data!A1"&amp;Macro1!$A$3</definedName>"#,
        r#"<definedName name="Mine" localSheetId="0">Data!$C$1</definedName>"#,
    );
    let (wb, rep) = read_xlsx(&macro_book_named(MACRO_WITH_STRINGS, &[], names)).unwrap();
    assert!(no_loss(&rep.warnings), "{:?}", rep.warnings);
    // The names written for the macro sheet, with the attributes they were written with.
    let written = |wb: &gridcraft_model::Workbook| -> Vec<(String, String)> {
        let u = Unpacked::new(&write_xlsx(wb).unwrap());
        let wbx = xml::parse(u.parts.get("xl/workbook.xml").unwrap()).unwrap();
        let macro_at = wbx.child("sheets").unwrap().kids("sheet").position(|s| s.attr("name") == Some("Macro1")).unwrap() as u32;
        let list = wbx.child("definedNames").unwrap();
        let scoped = list.kids("definedName").filter(|n| n.attr_u32("localSheetId") == Some(macro_at));
        scoped
            .map(|n| {
                let attrs = ["hidden", "comment", "function", "xlm"].iter().filter_map(|k| n.attr(k).map(|v| format!("{k}={v}"))).collect::<Vec<_>>();
                (format!("{} {}", n.attr("name").unwrap(), attrs.join(" ")).trim().to_string(), n.text.clone())
            })
            .collect()
    };
    let pair = |a: &str, b: &str| (a.to_string(), b.to_string());
    let as_read = vec![
        pair("Rate", "Data!$A$1"),
        pair("Total hidden=1 comment=a & b", "SUM( Data!$B$2:$B$9 , More!$A$1 )*2"),
        pair("Own function=1 xlm=1", "Macro1!$A$3"),
        pair("Text", r#""Data!A1"&Macro1!$A$3"#),
    ];
    assert_eq!(written(&wb), as_read);

    let rewritten = |rewrite: &dyn Fn(gridcraft_formula::Expr) -> gridcraft_formula::Expr| {
        let mut edited = wb.clone();
        crate::rewrite_kept_names(&mut edited, rewrite);
        written(&edited)
    };
    // A sheet renamed: the names that refer to it follow; the others are written as they were.
    assert_eq!(
        rewritten(&|e| adjust::rename_sheet(e, "Data", "Renamed 1")),
        vec![
            pair("Rate", "'Renamed 1'!$A$1"),
            pair("Total hidden=1 comment=a & b", "SUM('Renamed 1'!$B$2:$B$9,More!$A$1)*2"),
            as_read[2].clone(),
            as_read[3].clone(),
        ]
    );
    // Rows inserted into it move the cells they name; qualified by another sheet, they don't.
    let insert = Edit::Insert { axis: Axis::Rows, at: 0, count: 2 };
    assert_eq!(
        rewritten(&|e| adjust::adjust(e, "\u{0}", "Data", &insert)),
        vec![
            pair("Rate", "Data!$A$3"),
            pair("Total hidden=1 comment=a & b", "SUM(Data!$B$4:$B$11,More!$A$1)*2"),
            as_read[2].clone(),
            as_read[3].clone(),
        ]
    );
    // The sheet deleted: what they refer to is gone.
    let order = ["Data".to_string(), "More".to_string()];
    assert_eq!(
        rewritten(&|e| adjust::delete_sheet(e, "Data", &order)),
        vec![pair("Rate", "#REF!"), pair("Total hidden=1 comment=a & b", "SUM(#REF!,More!$A$1)*2"), as_read[2].clone(), as_read[3].clone(),]
    );
    // An edit that changes nothing leaves the workbook's kept content shared, not copied.
    let mut same = wb.clone();
    crate::rewrite_kept_names(&mut same, |e| e);
    assert!(Arc::ptr_eq(same.preserved.as_ref().unwrap(), wb.preserved.as_ref().unwrap()));
    // The name scoped to the worksheet is the workbook's own, and not this function's to rewrite.
    let mut edited = wb.clone();
    crate::rewrite_kept_names(&mut edited, |e| adjust::rename_sheet(e, "Data", "Renamed 1"));
    assert_eq!(edited.names.iter().map(|n| n.formula.as_str()).collect::<Vec<_>>(), ["Data!$C$1"]);
}

/// Cell formats 0 and 1 are both Normal (the model keeps one), 2 is a date; differential format 0
/// is bold.
const DUPLICATE_NORMAL_STYLES: &str = r#"<?xml version="1.0"?><styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><fonts count="2"><font><sz val="11"/><name val="Calibri"/></font><font><b/><sz val="11"/><name val="Calibri"/></font></fonts><fills count="2"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill></fills><borders count="1"><border><left/><right/><top/><bottom/><diagonal/></border></borders><cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs><cellXfs count="3"><xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/><xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/><xf numFmtId="14" fontId="0" fillId="0" borderId="0" xfId="0" applyNumberFormat="1"/></cellXfs><cellStyles count="1"><cellStyle name="Normal" xfId="0" builtinId="0"/></cellStyles><dxfs count="1"><dxf><font><b/></font></dxf></dxfs></styleSheet>"#;

const MACRO_WITH_FORMATS: &str = r#"<?xml version="1.0"?><xm:macrosheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:xm="http://schemas.microsoft.com/office/excel/2006/main"><cols><col min="2" max="2" width="12" style="2" customWidth="1"/></cols><sheetData><row r="1" s="2" customFormat="1"><c r="A1" t="s" s="1"><v>1</v></c><c r="B1" s="2"><v>45000</v></c></row></sheetData><conditionalFormatting sqref="B1"><cfRule type="cellIs" dxfId="0" priority="1" operator="greaterThan"><formula>0</formula></cfRule></conditionalFormatting></xm:macrosheet>"#;

#[test]
fn a_kept_macro_sheets_formats_are_renumbered_with_the_written_styles() {
    // Each format index of the macro sheet, resolved against the written styles: the number
    // format of a cell format, whether a differential format is bold.
    let formats = |u: &Unpacked| -> Vec<(String, String)> {
        let styles = xml::parse(u.parts.get("xl/styles.xml").unwrap()).unwrap();
        let xfs: Vec<String> = styles.child("cellXfs").unwrap().kids("xf").map(|x| x.attr("numFmtId").unwrap_or("0").to_string()).collect();
        let dxfs: Vec<bool> =
            styles.child("dxfs").map(|d| d.kids("dxf").map(|x| x.child("font").and_then(|f| f.child("b")).is_some()).collect()).unwrap_or_default();
        let xf = |i: Option<&str>| xfs.get(i.unwrap().parse::<usize>().unwrap()).cloned().unwrap_or_else(|| "missing".into());
        let part = xml::parse(u.parts.get(&kept_sheet(u, "Macro1").1).unwrap()).unwrap();
        let row = part.child("sheetData").unwrap().child("row").unwrap();
        let mut v =
            vec![("col".to_string(), xf(part.child("cols").unwrap().child("col").unwrap().attr("style"))), ("row".to_string(), xf(row.attr("s")))];
        v.extend(row.kids("c").map(|c| (c.attr("r").unwrap().to_string(), xf(c.attr("s")))));
        let dxf = part.child("conditionalFormatting").unwrap().child("cfRule").unwrap().attr_u32("dxfId").unwrap();
        v.push(("dxf".into(), dxfs.get(dxf as usize).map_or("missing".into(), |b| if *b { "bold".into() } else { "plain".into() })));
        v
    };
    let s = |a: &str, b: &str| (a.to_string(), b.to_string());
    let expected = vec![s("col", "14"), s("row", "14"), s("A1", "0"), s("B1", "14"), s("dxf", "bold")];
    let (wb, rep) = read_xlsx(&macro_book_with(MACRO_WITH_FORMATS, &[("rId10", "styles", "styles.xml", DUPLICATE_NORMAL_STYLES)])).unwrap();
    assert!(no_loss(&rep.warnings), "{:?}", rep.warnings);
    let out = write_xlsx(&wb).unwrap();
    let u = Unpacked::new(&out);
    u.assert_valid();
    assert_eq!(formats(&u), expected);
    assert_eq!(macro_texts(&u, "Macro1"), [("A1".to_string(), "macro-only".to_string(), None)]);
    // A second open and save, and a save after the worksheets gain formats of their own.
    let (again, _) = read_xlsx(&out).unwrap();
    assert_eq!(formats(&Unpacked::new(&write_xlsx(&again).unwrap())), expected);
    let mut edited = wb.clone();
    let three_places = edited.styles.intern(gridcraft_model::Style { num_fmt: gridcraft_model::NumFmt::new("0.000"), ..Default::default() });
    let at = gridcraft_core::CellRef::parse("B2").unwrap();
    edited.sheet_mut(0).unwrap().set_value(at, gridcraft_core::Value::Number(1.0));
    edited.sheet_mut(0).unwrap().cells.get_mut(at).unwrap().style = three_places;
    assert_eq!(formats(&Unpacked::new(&write_xlsx(&edited).unwrap())), expected);
}

/// Value metadata 0 is the rich value 0, a `#CALC!` error.
const CALC_METADATA: &str = r#"<metadata xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:xlrd="http://schemas.microsoft.com/office/spreadsheetml/2017/richdata"><metadataTypes count="1"><metadataType name="XLRICHVALUE"/></metadataTypes><futureMetadata name="XLRICHVALUE" count="1"><bk><extLst><ext uri="{3e2802c4-a4d2-4d8b-9148-e3be6c30e623}"><xlrd:rvb i="0"/></ext></extLst></bk></futureMetadata><valueMetadata count="1"><bk><rc t="1" v="0"/></bk></valueMetadata></metadata>"#;
const CALC_RICH_VALUE: &str =
    r#"<rvData xmlns="http://schemas.microsoft.com/office/spreadsheetml/2017/richdata" count="1"><rv s="0"><v>13</v><v>3</v></rv></rvData>"#;
const ERROR_STRUCTURE: &str = r#"<rvStructures xmlns="http://schemas.microsoft.com/office/spreadsheetml/2017/richdata" count="1"><s t="_error"><k n="errorType" t="i"/><k n="subType" t="i"/></s></rvStructures>"#;

/// Whether a written package holds value metadata, and its rich values part.
fn written_metadata(u: &Unpacked) -> (bool, Option<String>) {
    let xml_parts = || u.parts.keys().filter(|n| n.ends_with(".xml"));
    let meta = xml_parts().any(|n| u.text(n).contains("<valueMetadata"));
    (meta, xml_parts().find(|n| n.to_ascii_lowercase().ends_with("rdrichvalue.xml")).map(|n| u.text(n)))
}

#[test]
fn value_metadata_only_a_kept_macro_sheet_uses_is_kept_with_it() {
    // Only the macro sheet's A1 carries value metadata: a rich #CALC! saved as #VALUE!.
    let macro1 = r#"<?xml version="1.0"?><xm:macrosheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:xm="http://schemas.microsoft.com/office/excel/2006/main"><sheetData><row r="1"><c r="A1" t="e" vm="1"><v>#VALUE!</v></c></row></sheetData></xm:macrosheet>"#;
    let book = macro_book_with(
        macro1,
        &[
            ("rId11", "sheetMetadata", "metadata.xml", CALC_METADATA),
            ("rId12", "http://schemas.microsoft.com/office/2017/06/relationships/rdRichValue", "richData/rdrichvalue.xml", CALC_RICH_VALUE),
            (
                "rId13",
                "http://schemas.microsoft.com/office/2017/06/relationships/rdRichValueStructure",
                "richData/rdrichvaluestructure.xml",
                ERROR_STRUCTURE,
            ),
        ],
    );
    let (mut wb, rep) = read_xlsx(&book).unwrap();
    assert!(no_loss(&rep.warnings), "{:?}", rep.warnings);
    wb.sheet_mut(0).unwrap().set_value(gridcraft_core::CellRef::parse("D4").unwrap(), gridcraft_core::Value::Number(7.0));
    let vm = |u: &Unpacked| {
        let part = xml::parse(u.parts.get(&kept_sheet(u, "Macro1").1).unwrap()).unwrap();
        part.child("sheetData").unwrap().child("row").unwrap().child("c").unwrap().attr("vm").map(str::to_string)
    };
    let out = write_xlsx(&wb).unwrap();
    let u = Unpacked::new(&out);
    u.assert_valid();
    assert_eq!(vm(&u).as_deref(), Some("1"));
    let (meta, rich) = written_metadata(&u);
    assert!(meta && rich.is_some_and(|r| r.contains("<v>13</v>")), "the metadata and the #CALC! rich value are written");
    let (again, rep) = read_xlsx(&out).unwrap();
    assert!(no_loss(&rep.warnings), "{:?}", rep.warnings);
    let u = Unpacked::new(&write_xlsx(&again).unwrap());
    assert_eq!((vm(&u).as_deref(), written_metadata(&u).0), (Some("1"), true));
    // A macro-free save leaves the macro sheet out, and nothing else names the metadata.
    let (meta, rich) = written_metadata(&Unpacked::new(&write_xlsx_as(&wb, XlsxKind::Workbook).unwrap()));
    assert!(!meta && rich.is_none());
}
