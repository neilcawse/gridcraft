//! Kept parts whose zip entry is empty or can't be read. A part whose directory entry declares no
//! bytes is an empty part (Excel and other producers write an empty thumbnail that way, some as a
//! deflate entry with no stream at all) and is kept as one. An entry that can't be decoded is
//! dropped with a warning that names it, and costs the package's decompression budget only what
//! was read of it: the parts read after it (the document properties) are still read.

use super::*;
use crate::testing::{Entry, damaged_zip};

const TY_THUMBNAIL: &str = "http://schemas.openxmlformats.org/package/2006/relationships/metadata/thumbnail";
const CORE_TYPE: &str = "http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties";
const CORE: &str = r#"<?xml version="1.0"?><cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Budget</dc:title><cp:revision>7</cp:revision></cp:coreProperties>"#;
const APP: &str = r#"<?xml version="1.0"?><Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties"><Application>Microsoft Excel</Application><Manager>Ann</Manager></Properties>"#;
const TYPES: &str = r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="xml" ContentType="application/xml"/><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="wmf" ContentType="image/x-wmf"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#;
const THUMBNAIL: &str = "docProps/thumbnail.wmf";

/// A package whose thumbnail is the entry `thumbnail` (listed first, before the properties parts
/// that are read after it).
fn package_with_thumbnail(thumbnail: Entry<'_>) -> Vec<u8> {
    let workbook = format!(
        r#"<?xml version="1.0"?><workbook xmlns="{NS_MAIN}" xmlns:r="{NS_R}"><sheets><sheet name="Data" sheetId="1" r:id="rId1"/></sheets></workbook>"#
    );
    let sheet =
        format!(r#"<?xml version="1.0"?><worksheet xmlns="{NS_MAIN}"><sheetData><row r="1"><c r="A1"><v>1</v></c></row></sheetData></worksheet>"#);
    let root = rels(&[
        ("rId1", "officeDocument", "xl/workbook.xml"),
        ("rId2", TY_THUMBNAIL, THUMBNAIL),
        ("rId3", CORE_TYPE, "docProps/core.xml"),
        ("rId4", "extended-properties", "docProps/app.xml"),
    ]);
    let wb_rels = rels(&[("rId1", "worksheet", "worksheets/sheet1.xml")]);
    damaged_zip(&[
        Entry::Intact("[Content_Types].xml", TYPES.as_bytes()),
        Entry::Intact("_rels/.rels", root.as_bytes()),
        thumbnail,
        Entry::Intact("docProps/core.xml", CORE.as_bytes()),
        Entry::Intact("docProps/app.xml", APP.as_bytes()),
        Entry::Intact("xl/_rels/workbook.xml.rels", wb_rels.as_bytes()),
        Entry::Intact("xl/workbook.xml", workbook.as_bytes()),
        Entry::Intact("xl/worksheets/sheet1.xml", sheet.as_bytes()),
    ])
}

fn assert_properties_kept(u: &Unpacked) {
    let core = u.text("docProps/core.xml");
    assert!(core.contains("<cp:revision>7</cp:revision>"), "{core}");
    assert!(u.text("docProps/app.xml").contains("<Manager>Ann</Manager>"));
}

/// The warnings about kept content (the test packages have no styles part, which is reported too).
fn notices(warnings: &[String]) -> Vec<&String> {
    warnings.iter().filter(|w| !w.contains("styles")).collect()
}

#[test]
fn a_thumbnail_declared_empty_is_kept_as_an_empty_part() {
    // The directory says 0 bytes; the entry is a deflate entry with no stream at all (a data
    // descriptor follows its header), which no reader can decode.
    let (wb, rep) = read_xlsx(&package_with_thumbnail(Entry::EmptyWithoutStream(THUMBNAIL))).unwrap();
    assert_eq!(notices(&rep.warnings), Vec::<&String>::new());
    let out = write_xlsx(&wb).unwrap();
    let u = Unpacked::new(&out);
    u.assert_valid();
    let kept = u.rels("").into_iter().find(|r| r.1.ends_with("/thumbnail")).expect("the thumbnail relationship is kept");
    assert_eq!(u.parts.get(&kept.2).map(Vec::as_slice), Some(&[][..]), "the part is written with 0 bytes");
    assert!(u.parts.contains_key(THUMBNAIL));
    assert_properties_kept(&u);
    // The written package reads and writes again the same way.
    let (again, rep) = read_xlsx(&out).unwrap();
    assert_eq!(notices(&rep.warnings), Vec::<&String>::new());
    let u2 = Unpacked::new(&write_xlsx(&again).unwrap());
    assert!(u2.parts.get(THUMBNAIL).is_some_and(|d| d.is_empty()));
}

#[test]
fn an_unreadable_thumbnail_is_dropped_with_a_warning_and_costs_the_budget_nothing() {
    for (what, thumbnail) in [
        ("a stream that isn't deflate", Entry::BadStream(THUMBNAIL, 40)),
        ("a bad checksum", Entry::BadChecksum(THUMBNAIL, b"\xD7\xCD\xC6\x9A not really a wmf")),
    ] {
        let (wb, rep) = read_xlsx(&package_with_thumbnail(thumbnail)).unwrap();
        let w = rep.warnings.join("\n");
        // Dropping the relationship is announced, naming the part and why.
        assert!(w.contains("thumbnail part docProps/thumbnail.wmf was dropped"), "{what}: {w}");
        // The parts read after it were not starved of the package's budget.
        assert!(!w.contains("1 GB"), "{what}: {w}");
        assert!(!w.contains("elements GridCraft does not model were dropped"), "{what}: {w}");
        let u = Unpacked::new(&write_xlsx(&wb).unwrap());
        u.assert_valid();
        assert!(u.rels("").iter().all(|r| !r.1.ends_with("/thumbnail")), "{what}");
        assert!(!u.parts.contains_key(THUMBNAIL), "{what}");
        assert_properties_kept(&u);
    }
}
