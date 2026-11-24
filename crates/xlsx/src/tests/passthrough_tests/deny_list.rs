//! The pass-through is a deny-list: every relationship of the package root, the workbook and a
//! sheet whose kind GridCraft does not rebuild is written back with the part behind it, whether or
//! not the kind is known (thumbnails, sensitivity labels, shared-workbook revisions, sort maps…).

use super::*;

const THUMBNAIL: &[u8] = b"\xFF\xD8\xFF\xE0 not really a jpeg";
const LABELS: &str = r#"<?xml version="1.0"?><clbl:labelList xmlns:clbl="http://schemas.microsoft.com/office/2020/mipLabelMetadata"><clbl:label id="{00000000-0000-0000-0000-000000000001}" enabled="1" method="Privileged" siteId="{00000000-0000-0000-0000-000000000002}" contentBits="0" removed="0"/></clbl:labelList>"#;
const ROOT_FUTURE: &[u8] = b"root part of a kind nobody knows \x00\x01";
const REVISION_HEADERS: &str = r#"<?xml version="1.0"?><headers xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" guid="{00000000-0000-0000-0000-000000000003}"><header guid="{00000000-0000-0000-0000-000000000004}" dateTime="2001-01-02T03:04:05" maxSheetId="1" userName="A" r:id="rId1" minRId="1" maxRId="1"><sheetIdMap count="1"><sheetId val="1"/></sheetIdMap></header></headers>"#;
const REVISION_LOG: &str = r#"<?xml version="1.0"?><revisions xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><rcc rId="1" sId="1"><nc r="A1"><v>1</v></nc></rcc></revisions>"#;
const USER_NAMES: &str = r#"<?xml version="1.0"?><users xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="1"><userInfo guid="{00000000-0000-0000-0000-000000000005}" name="A" id="0" dateTime="2001-01-02T03:04:05"/></users>"#;
const VOLATILE: &str = r#"<?xml version="1.0"?><volTypes xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><volType type="realTimeData"><main first="rtd.server"><tp t="n"><v>1</v><stp>topic</stp><tr r="A1" s="1"/></tp></main></volType></volTypes>"#;
const TOOLBARS: &[u8] = b"\x01\x02 attached toolbars \xFE\xFF";
const WORKBOOK_FUTURE: &[u8] = b"workbook part of a kind nobody knows \x02\x03";
const SORT_MAP: &str = r#"<?xml version="1.0"?><worksheetSortMap xmlns="http://schemas.microsoft.com/office/excel/2006/main"><rowSortMap ref="A1:A3" count="3"><row newVal="1" oldVal="2"/></rowSortMap></worksheetSortMap>"#;
const SHEET_VIEWS: &str = r#"<?xml version="1.0"?><namedSheetViews xmlns="http://schemas.microsoft.com/office/spreadsheetml/2019/namedsheetviews"><namedSheetView name="Mine" id="{00000000-0000-0000-0000-000000000006}"/></namedSheetViews>"#;
const CUSTOM_PROPERTY: &[u8] = b"\x00\x10 custom property \x00\x00 binary";
const SHEET_FUTURE: &[u8] = b"sheet part of a kind nobody knows \x04\x05";

const CONTENT_TYPES: &str = r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="xml" ContentType="application/xml"/><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="jpeg" ContentType="image/jpeg"/><Default Extension="bin" ContentType="application/octet-stream"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/docMetadata/LabelInfo.xml" ContentType="application/vnd.ms-office.classificationlabels+xml"/><Override PartName="/xl/revisions/revisionHeaders.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.revisionHeaders+xml"/><Override PartName="/xl/revisions/revisionLog1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.revisionLog+xml"/><Override PartName="/xl/revisions/userNames.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.userNames+xml"/><Override PartName="/xl/volatileDependencies.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.volatileDependencies+xml"/><Override PartName="/xl/wsSortMap1.xml" ContentType="application/vnd.ms-excel.wsSortMap+xml"/><Override PartName="/xl/namedSheetViews/namedSheetView1.xml" ContentType="application/vnd.ms-excel.namedsheetviews+xml"/><Override PartName="/xl/customProperty1.bin" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.customProperty"/></Types>"#;

const TY_THUMBNAIL: &str = "http://schemas.openxmlformats.org/package/2006/relationships/metadata/thumbnail";
const TY_LABELS: &str = "http://schemas.microsoft.com/office/2020/02/relationships/classificationlabels";
const TY_FUTURE: &str = "http://example.com/relationships/not-yet-invented";

fn package() -> Vec<u8> {
    let workbook = format!(
        r#"<?xml version="1.0"?><workbook xmlns="{NS_MAIN}" xmlns:r="{NS_R}"><sheets><sheet name="Data" sheetId="1" r:id="rId1"/></sheets></workbook>"#
    );
    let sheet = format!(
        r#"<?xml version="1.0"?><worksheet xmlns="{NS_MAIN}" xmlns:r="{NS_R}"><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>link</t></is></c></row></sheetData><hyperlinks><hyperlink ref="A1" r:id="rId4"/></hyperlinks><pageMargins left="0.7" right="0.7" top="0.75" bottom="0.75" header="0.3" footer="0.3"/><customProperties><customPr name="HsTable" r:id="rId3"/></customProperties></worksheet>"#
    );
    let root_rels = rels(&[
        ("rId1", "officeDocument", "xl/workbook.xml"),
        ("rId2", TY_THUMBNAIL, "docProps/thumbnail.jpeg"),
        ("rId3", TY_LABELS, "docMetadata/LabelInfo.xml"),
        ("rId4", TY_FUTURE, "future/root.bin"),
    ]);
    let wb_rels = rels(&[
        ("rId1", "worksheet", "worksheets/sheet1.xml"),
        ("rId2", "revisionHeaders", "revisions/revisionHeaders.xml"),
        ("rId3", "usernames", "revisions/userNames.xml"),
        ("rId4", "volatileDependencies", "volatileDependencies.xml"),
        ("rId5", "http://schemas.microsoft.com/office/2006/relationships/attachedToolbars", "attachedToolbars.bin"),
        ("rId6", TY_FUTURE, "future/workbook.bin"),
    ]);
    let revision_rels = rels(&[("rId1", "revisionLog", "revisionLog1.xml")]);
    let sheet_rels = rels(&[
        ("rId1", "http://schemas.microsoft.com/office/2006/relationships/wsSortMap", "../wsSortMap1.xml"),
        ("rId2", "http://schemas.microsoft.com/office/2019/04/relationships/namedSheetView", "../namedSheetViews/namedSheetView1.xml"),
        ("rId3", "customProperty", "../customProperty1.bin"),
        ("rId4", "hyperlink", "https://example.com/page"),
        ("rId5", TY_FUTURE, "../future/sheet.bin"),
    ]);
    make_zip(&[
        ("[Content_Types].xml", CONTENT_TYPES.as_bytes()),
        ("_rels/.rels", root_rels.as_bytes()),
        ("docProps/thumbnail.jpeg", THUMBNAIL),
        ("docMetadata/LabelInfo.xml", LABELS.as_bytes()),
        ("future/root.bin", ROOT_FUTURE),
        ("xl/workbook.xml", workbook.as_bytes()),
        ("xl/_rels/workbook.xml.rels", wb_rels.as_bytes()),
        ("xl/revisions/revisionHeaders.xml", REVISION_HEADERS.as_bytes()),
        ("xl/revisions/_rels/revisionHeaders.xml.rels", revision_rels.as_bytes()),
        ("xl/revisions/revisionLog1.xml", REVISION_LOG.as_bytes()),
        ("xl/revisions/userNames.xml", USER_NAMES.as_bytes()),
        ("xl/volatileDependencies.xml", VOLATILE.as_bytes()),
        ("xl/attachedToolbars.bin", TOOLBARS),
        ("xl/future/workbook.bin", WORKBOOK_FUTURE),
        ("xl/worksheets/sheet1.xml", sheet.as_bytes()),
        ("xl/worksheets/_rels/sheet1.xml.rels", sheet_rels.as_bytes()),
        ("xl/wsSortMap1.xml", SORT_MAP.as_bytes()),
        ("xl/namedSheetViews/namedSheetView1.xml", SHEET_VIEWS.as_bytes()),
        ("xl/customProperty1.bin", CUSTOM_PROPERTY),
        ("xl/future/sheet.bin", SHEET_FUTURE),
    ])
}

/// The bytes of the part the `owner` relationship whose type ends in `suffix` targets.
fn target_bytes(u: &Unpacked, owner: &str, suffix: &str) -> Vec<u8> {
    let r = u.rels(owner).into_iter().find(|r| r.1.ends_with(suffix)).unwrap_or_else(|| panic!("{owner:?} has no {suffix} relationship"));
    u.parts.get(&r.2).unwrap_or_else(|| panic!("{} is missing", r.2)).clone()
}

fn assert_kept_everywhere(u: &Unpacked) {
    // The package root.
    assert_eq!(target_bytes(u, "", "/thumbnail"), THUMBNAIL);
    assert_eq!(target_bytes(u, "", "/classificationlabels"), LABELS.as_bytes());
    assert_eq!(target_bytes(u, "", "/not-yet-invented"), ROOT_FUTURE);
    // The workbook.
    let wb = "xl/workbook.xml";
    assert_eq!(target_bytes(u, wb, "/revisionHeaders"), REVISION_HEADERS.as_bytes());
    assert_eq!(target_bytes(u, wb, "/usernames"), USER_NAMES.as_bytes());
    assert_eq!(target_bytes(u, wb, "/volatileDependencies"), VOLATILE.as_bytes());
    assert_eq!(target_bytes(u, wb, "/attachedToolbars"), TOOLBARS);
    assert_eq!(target_bytes(u, wb, "/not-yet-invented"), WORKBOOK_FUTURE);
    // The part behind a kept part: the revision log hangs off the revision headers.
    let headers = u.rels(wb).into_iter().find(|r| r.1.ends_with("/revisionHeaders")).unwrap().2;
    assert_eq!(target_bytes(u, &headers, "/revisionLog"), REVISION_LOG.as_bytes());
    // The sheet.
    let sheet = sheet_part(u, "Data");
    assert_eq!(target_bytes(u, &sheet, "/wsSortMap"), SORT_MAP.as_bytes());
    assert_eq!(target_bytes(u, &sheet, "/namedSheetView"), SHEET_VIEWS.as_bytes());
    assert_eq!(target_bytes(u, &sheet, "/customProperty"), CUSTOM_PROPERTY);
    assert_eq!(target_bytes(u, &sheet, "/not-yet-invented"), SHEET_FUTURE);
    // The sheet's `<customProperties>` names its relationship again, and sits where the schema
    // puts it: after the page setup, before the drawing and legacy drawing.
    let sx = u.text(&sheet);
    let id = attr_of(&sx, "customPr", "r:id");
    assert_eq!(attr_of(&sx, "customPr", "name"), "HsTable");
    let (ty, target) = u.target(&sheet, &id);
    assert!(ty.ends_with("/customProperty"));
    assert_eq!(u.parts.get(&target).unwrap(), CUSTOM_PROPERTY);
    assert!(sx.find("<pageMargins").unwrap() < sx.find("<customProperties").unwrap());
    // A hyperlink is rebuilt from the model: written once, not kept as well.
    let links: Vec<_> = u.rels(&sheet).into_iter().filter(|r| r.1.ends_with("/hyperlink")).collect();
    assert_eq!(links.len(), 1, "{links:?}");
    assert_eq!(links[0].2, "https://example.com/page");
}

#[test]
fn relationships_of_any_unrebuilt_kind_are_kept() {
    let (wb, rep) = read_xlsx(&package()).unwrap();
    assert!(!rep.warnings.iter().any(|w| w.contains("dropped")), "{:?}", rep.warnings);
    let out = write_xlsx(&wb).unwrap();
    let u = Unpacked::new(&out);
    u.assert_valid();
    assert_kept_everywhere(&u);
    // Content types travel with the parts.
    let types = u.text("[Content_Types].xml");
    for ct in ["image/jpeg", "revisionLog+xml", "wsSortMap+xml", "namedsheetviews+xml", "classificationlabels+xml"] {
        assert!(types.contains(ct), "{ct} missing from {types}");
    }
    // Kept parts of kinds GridCraft knows keep their names.
    assert!(u.parts.contains_key("docProps/thumbnail.jpeg") && u.parts.contains_key("docMetadata/LabelInfo.xml"));
}

#[test]
fn kept_relationships_survive_a_second_save() {
    let (wb, _) = read_xlsx(&package()).unwrap();
    let (wb2, rep) = read_xlsx(&write_xlsx(&wb).unwrap()).unwrap();
    assert!(!rep.warnings.iter().any(|w| w.contains("dropped")), "{:?}", rep.warnings);
    let u = Unpacked::new(&write_xlsx(&wb2).unwrap());
    u.assert_valid();
    assert_kept_everywhere(&u);
}

#[test]
fn a_kept_part_that_reaches_rebuilt_content_is_dropped_with_a_warning() {
    // A part of an unknown kind whose own relationship names a worksheet: GridCraft rewrites the
    // worksheet, so the kept part would point at something that no longer exists in that form.
    let into_sheet = rels(&[("rId1", "worksheet", "../xl/worksheets/sheet1.xml")]);
    let mut u = Unpacked::new(&package());
    u.parts.insert("future/_rels/root.bin.rels".into(), into_sheet.into_bytes());
    let list: Vec<(&str, &[u8])> = u.parts.iter().map(|(k, v)| (k.as_str(), v.as_slice())).collect();
    let (wb, rep) = read_xlsx(&make_zip(&list)).unwrap();
    let w = rep.warnings.join("\n");
    assert!(w.contains("not-yet-invented part future/root.bin was dropped: it refers to worksheet"), "{w}");
    let out = Unpacked::new(&write_xlsx(&wb).unwrap());
    out.assert_valid();
    assert!(out.find(ROOT_FUTURE).is_empty());
    assert_eq!(target_bytes(&out, "", "/thumbnail"), THUMBNAIL, "the other root content is still there");
}
