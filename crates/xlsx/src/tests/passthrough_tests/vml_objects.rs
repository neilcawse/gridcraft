//! A sheet's legacy VML drawing holds more than notes: check boxes, buttons and pictures drawn
//! there need no `<controls>` or `<oleObjects>` element in the sheet XML. GridCraft writes the
//! notes; everything else in the drawing is kept, shape ids and pictures included.

use gridcraft_core::CellRef;

use super::*;

const IMAGE: &[u8] = b"\x89PNG\r\n\x1a\n a picture a shape shows";
const HEAD: &str = r##"<xml xmlns:v="urn:schemas-microsoft-com:vml" xmlns:o="urn:schemas-microsoft-com:office:office" xmlns:x="urn:schemas-microsoft-com:office:excel"><o:shapelayout v:ext="edit"><o:idmap v:ext="edit" data="1"/></o:shapelayout>"##;
const TYPES: &str = r##"<v:shapetype id="_x0000_t201" coordsize="21600,21600" o:spt="201" path="m,l,21600r21600,l21600,xe"><v:stroke joinstyle="miter"/></v:shapetype><v:shapetype id="_x0000_t75" coordsize="21600,21600" o:spt="75"/>"##;

fn checkbox(id: u32, label: &str) -> String {
    format!(
        r##"<v:shape id="_x0000_s{id}" type="#_x0000_t201" style='position:absolute;margin-left:16.5pt;margin-top:25.5pt;width:68.25pt;height:16.5pt;z-index:1' filled="f" stroked="f"><v:textbox style='mso-direction-alt:auto'><div style='text-align:left'><font face="Tahoma" size="160" color="auto">{label}</font></div></v:textbox><x:ClientData ObjectType="Checkbox"><x:SizeWithCells/><x:Anchor>0, 22, 1, 6, 1, 46, 1, 28</x:Anchor><x:Checked>1</x:Checked><x:FmlaLink>$B$2</x:FmlaLink></x:ClientData></v:shape>"##
    )
}

/// A picture shape (`ObjectType="Pict"`) showing the image its `o:relid` names.
fn picture(id: u32, relid: &str) -> String {
    format!(
        r##"<v:shape id="_x0000_s{id}" type="#_x0000_t75" style='position:absolute;width:40pt;height:30pt'><v:imagedata o:relid="{relid}" o:title="logo"/><x:ClientData ObjectType="Pict"><x:Anchor>2, 0, 3, 0, 4, 0, 6, 0</x:Anchor><x:FmlaMacro>[0]!Logo_Click</x:FmlaMacro></x:ClientData></v:shape>"##
    )
}

/// A note shape, its `ObjectType` quoted as asked: producers differ.
fn note(id: u32, quote: char) -> String {
    format!(
        r##"<v:shape id="_x0000_s{id}" type="#_x0000_t202" style='position:absolute;width:100pt;height:50pt;visibility:hidden'><v:textbox><div>source note</div></v:textbox><x:ClientData ObjectType={quote}Note{quote}><x:Anchor>1, 15, 0, 2, 3, 15, 3, 16</x:Anchor><x:Row>0</x:Row><x:Column>0</x:Column></x:ClientData></v:shape>"##
    )
}

fn vml(shapes: &[String]) -> String {
    format!("{HEAD}{TYPES}{}</xml>", shapes.concat())
}

/// One sheet "Data" with the given legacy drawing (and notes when `notes`), the picture its
/// shapes show in `xl/media/image1.png` unless `image` is false.
fn package(vml: &str, notes: bool, image: bool) -> Vec<u8> {
    let types = format!(
        r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="xml" ContentType="application/xml"/><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="png" ContentType="image/png"/><Default Extension="vml" ContentType="application/vnd.openxmlformats-officedocument.vmlDrawing"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>{}</Types>"#,
        if notes {
            r#"<Override PartName="/xl/comments1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.comments+xml"/>"#
        } else {
            ""
        }
    );
    let workbook = format!(
        r#"<?xml version="1.0"?><workbook xmlns="{NS_MAIN}" xmlns:r="{NS_R}"><sheets><sheet name="Data" sheetId="1" r:id="rId1"/></sheets></workbook>"#
    );
    let sheet = format!(
        r#"<?xml version="1.0"?><worksheet xmlns="{NS_MAIN}" xmlns:r="{NS_R}"><sheetData><row r="1"><c r="A1"><v>1</v></c></row></sheetData><pageMargins left="0.7" right="0.7" top="0.75" bottom="0.75" header="0.3" footer="0.3"/><legacyDrawing r:id="rId1"/></worksheet>"#
    );
    let mut sheet_rels = vec![("rId1", "vmlDrawing", "../drawings/vmlDrawing1.vml")];
    if notes {
        sheet_rels.push(("rId2", "comments", "../comments1.xml"));
    }
    let root_rels = rels(&[("rId1", "officeDocument", "xl/workbook.xml")]);
    let wb_rels = rels(&[("rId1", "worksheet", "worksheets/sheet1.xml")]);
    let sheet_rels = rels(&sheet_rels);
    let vml_rels = rels(&[("rId1", "image", "../media/image1.png")]);
    let mut parts: Vec<(&str, &[u8])> = vec![
        ("[Content_Types].xml", types.as_bytes()),
        ("_rels/.rels", root_rels.as_bytes()),
        ("xl/workbook.xml", workbook.as_bytes()),
        ("xl/_rels/workbook.xml.rels", wb_rels.as_bytes()),
        ("xl/worksheets/sheet1.xml", sheet.as_bytes()),
        ("xl/worksheets/_rels/sheet1.xml.rels", sheet_rels.as_bytes()),
        ("xl/drawings/vmlDrawing1.vml", vml.as_bytes()),
        ("xl/drawings/_rels/vmlDrawing1.vml.rels", vml_rels.as_bytes()),
    ];
    if image {
        parts.push(("xl/media/image1.png", IMAGE));
    }
    if notes {
        parts.push(("xl/comments1.xml", COMMENTS.as_bytes()));
    }
    make_zip(&parts)
}

/// The sheet's legacy drawing as written: its text and the parts its relationships reach.
fn written_vml(u: &Unpacked) -> (String, String) {
    let sheet = sheet_part(u, "Data");
    let sx = u.text(&sheet);
    let (ty, part) = u.target(&sheet, &attr_of(&sx, "legacyDrawing", "r:id"));
    assert!(ty.ends_with("/vmlDrawing"), "{ty}");
    (u.text(&part), part)
}

fn count(text: &str, what: &str) -> usize {
    text.matches(what).count()
}

#[test]
fn shapes_without_a_controls_element_survive_a_save() {
    let source = vml(&[checkbox(1025, "1ST TRIP"), checkbox(1026, "2ND TRIP"), picture(1027, "rId1")]);
    let (wb, rep) = read_xlsx(&package(&source, false, true)).unwrap();
    assert!(!rep.warnings.iter().any(|w| w.contains("dropped")), "{:?}", rep.warnings);
    assert!(rep.warnings.iter().any(|w| w.contains("form controls are not shown")), "{:?}", rep.warnings);
    let out = write_xlsx(&wb).unwrap();
    let u = Unpacked::new(&out);
    u.assert_valid();
    let (text, part) = written_vml(&u);
    assert_eq!(text, source, "the drawing is written as it was read");
    // The shape ids the sheet and other drawings may name are the ones the file gave.
    for id in ["_x0000_s1025", "_x0000_s1026", "_x0000_s1027"] {
        assert_eq!(count(&text, id), 1, "{id}");
    }
    // The picture a shape shows follows the drawing, byte for byte, under the same `o:relid`.
    let image = u.rels(&part);
    assert_eq!(image.len(), 1, "{image:?}");
    assert_eq!((image[0].0.as_str(), u.parts.get(&image[0].2).unwrap().as_slice()), ("rId1", IMAGE));
    // Reading the output keeps the shapes again.
    let (wb2, rep2) = read_xlsx(&out).unwrap();
    assert!(wb2.sheet(0).unwrap().preserved.as_ref().is_some_and(|p| p.vml.is_some()), "{:?}", rep2.warnings);
    assert!(!rep2.warnings.iter().any(|w| w.contains("dropped")), "{:?}", rep2.warnings);
}

#[test]
fn notes_and_shapes_share_the_drawing_without_duplicates() {
    // The source drawing holds a note (quoted with apostrophes, as some producers do) and a check
    // box. GridCraft writes the note from the model; the check box is the drawing's own.
    let source = vml(&[checkbox(1025, "Done"), note(1026, '\'')]);
    let (mut wb, rep) = read_xlsx(&package(&source, true, true)).unwrap();
    assert!(!rep.warnings.iter().any(|w| w.contains("dropped")), "{:?}", rep.warnings);
    assert_eq!(wb.sheet(0).unwrap().comments.len(), 1, "the note is modelled");
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    u.assert_valid();
    let (text, _) = written_vml(&u);
    assert_eq!(count(&text, "ObjectType=\"Checkbox\""), 1, "{text}");
    assert_eq!(count(&text, "_x0000_s1025\""), 1);
    assert_eq!(count(&text, "ObjectType=\"Note\""), 1, "GridCraft's note only: {text}");
    assert_eq!(count(&text, "ObjectType='Note'"), 0, "the source note is not carried over: {text}");
    assert_eq!(count(&text, "<v:shapetype id=\"_x0000_t202\""), 1, "{text}");
    // The note takes a shape block the check box doesn't use, and the drawing claims it.
    assert!(text.contains("_x0000_s2049") && text.contains("data=\"1,2\""), "{text}");

    // Without its note the drawing is the check box alone.
    wb.sheet_mut(0).unwrap().comments.clear();
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    u.assert_valid();
    let (text, _) = written_vml(&u);
    assert_eq!(count(&text, "ObjectType=\"Checkbox\""), 1);
    assert_eq!(count(&text, "Note"), 0, "{text}");
    assert!(u.parts.keys().all(|n| !n.ends_with("comments1.xml")));
}

#[test]
fn a_drawing_of_notes_alone_is_left_to_the_notes_writer() {
    let source = vml(&[note(1025, '"'), note(1026, '\'')]);
    let (wb, rep) = read_xlsx(&package(&source, true, false)).unwrap();
    assert!(!rep.warnings.iter().any(|w| w.contains("dropped") || w.contains("not shown")), "{:?}", rep.warnings);
    assert!(wb.sheet(0).unwrap().preserved.as_ref().is_none_or(|p| p.vml.is_none()));
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    u.assert_valid();
    let (text, _) = written_vml(&u);
    assert_eq!(count(&text, "<x:ClientData ObjectType=\"Note\">"), 1, "one note is modelled, written once: {text}");
    assert!(!text.contains("source note"), "{text}");
}

#[test]
fn a_picture_that_is_gone_does_not_cost_the_shapes() {
    // The drawing's relationship names a picture the package doesn't hold: that picture can't be
    // kept (and is reported), the shapes are.
    let source = vml(&[checkbox(1025, "Done"), picture(1026, "rId1")]);
    let (wb, rep) = read_xlsx(&package(&source, false, false)).unwrap();
    assert!(rep.warnings.iter().any(|w| w.contains("image1.png") && w.contains("dropped")), "{:?}", rep.warnings);
    assert!(!rep.warnings.iter().any(|w| w.contains("legacy drawing were dropped")), "{:?}", rep.warnings);
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    u.assert_valid();
    let (text, _) = written_vml(&u);
    assert_eq!((count(&text, "_x0000_s1025\""), count(&text, "_x0000_s1026\"")), (1, 1));
}

#[test]
fn a_damaged_drawing_is_dropped_with_a_warning() {
    // No closing tag: the notes couldn't be merged in, so nothing is kept of it.
    let source = vml(&[checkbox(1025, "Done")]).replace("</xml>", "");
    let (wb, rep) = read_xlsx(&package(&source, false, true)).unwrap();
    assert!(rep.warnings.iter().any(|w| w.contains("\"Data\"") && w.contains("dropped") && w.contains("damaged")), "{:?}", rep.warnings);
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    u.assert_valid();
    assert!(!u.text("xl/worksheets/sheet1.xml").contains("legacyDrawing"));
    assert!(u.parts.keys().all(|n| !n.ends_with(".vml")));
}

// ------------------------------------------------------------ pictures in the boxes of notes

const PHOTO: &[u8] = b"\x89PNG\r\n\x1a\n a photo that fills a note box";
const PHOTO2: &[u8] = b"\x89PNG\r\n\x1a\n another photo";
const PLAIN_FILL: &str = r##"<v:fill color2="#ffffe1"/>"##;
const TWO_NOTES: &str = r#"<?xml version="1.0"?><comments xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><authors><author>A</author></authors><commentList><comment ref="A1" authorId="0"><text><t>first</t></text></comment><comment ref="B2" authorId="0"><text><t>second</t></text></comment></commentList></comments>"#;

/// A note shape of the cell (`row`, `col`) with the given `<v:fill>`.
fn note_at(id: u32, row: u32, col: u32, fill: &str) -> String {
    format!(
        r##"<v:shape id="_x0000_s{id}" type="#_x0000_t202" style='position:absolute;width:100pt;height:50pt;visibility:hidden' fillcolor="#ffffe1" o:insetmode="auto">{fill}<v:textbox><div>source note</div></v:textbox><x:ClientData ObjectType="Note"><x:Anchor>1, 15, 0, 2, 3, 15, 3, 16</x:Anchor><x:Row>{row}</x:Row><x:Column>{col}</x:Column></x:ClientData></v:shape>"##
    )
}

/// The fill a producer writes for a note with a picture: here with `o:relid` repeated, as some do.
fn photo_fill(relid: &str, times: usize) -> String {
    format!(r##"<v:fill {}o:title="photo" color2="#ffffe1" type="frame"/>"##, format!("o:relid=\"{relid}\" ").repeat(times))
}

/// Sheet "Data" with notes at A1 and B2 and the given legacy drawing, which has the images
/// `rId1` (`PHOTO`) and `rId2` (`PHOTO2`) in `xl/media`.
fn fill_package(vml: &str) -> Vec<u8> {
    let types = r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="xml" ContentType="application/xml"/><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="png" ContentType="image/png"/><Default Extension="vml" ContentType="application/vnd.openxmlformats-officedocument.vmlDrawing"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/xl/comments1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.comments+xml"/></Types>"#;
    let workbook = format!(
        r#"<?xml version="1.0"?><workbook xmlns="{NS_MAIN}" xmlns:r="{NS_R}"><sheets><sheet name="Data" sheetId="1" r:id="rId1"/></sheets></workbook>"#
    );
    let sheet = format!(
        r#"<?xml version="1.0"?><worksheet xmlns="{NS_MAIN}" xmlns:r="{NS_R}"><sheetData><row r="1"><c r="A1"><v>1</v></c></row></sheetData><pageMargins left="0.7" right="0.7" top="0.75" bottom="0.75" header="0.3" footer="0.3"/><legacyDrawing r:id="rId1"/></worksheet>"#
    );
    let root_rels = rels(&[("rId1", "officeDocument", "xl/workbook.xml")]);
    let wb_rels = rels(&[("rId1", "worksheet", "worksheets/sheet1.xml")]);
    let sheet_rels = rels(&[("rId1", "vmlDrawing", "../drawings/vmlDrawing1.vml"), ("rId2", "comments", "../comments1.xml")]);
    let vml_rels = rels(&[("rId1", "image", "../media/image1.png"), ("rId2", "image", "../media/image2.png")]);
    make_zip(&[
        ("[Content_Types].xml", types.as_bytes()),
        ("_rels/.rels", root_rels.as_bytes()),
        ("xl/workbook.xml", workbook.as_bytes()),
        ("xl/_rels/workbook.xml.rels", wb_rels.as_bytes()),
        ("xl/worksheets/sheet1.xml", sheet.as_bytes()),
        ("xl/worksheets/_rels/sheet1.xml.rels", sheet_rels.as_bytes()),
        ("xl/drawings/vmlDrawing1.vml", vml.as_bytes()),
        ("xl/drawings/_rels/vmlDrawing1.vml.rels", vml_rels.as_bytes()),
        ("xl/media/image1.png", PHOTO),
        ("xl/media/image2.png", PHOTO2),
        ("xl/comments1.xml", TWO_NOTES.as_bytes()),
    ])
}

/// The text of the written note shape of the cell (`row`, `col`).
fn shape_of(text: &str, row: u32, col: u32) -> String {
    let at = format!("<x:Row>{row}</x:Row><x:Column>{col}</x:Column>");
    let end = text.find(&at).unwrap_or_else(|| panic!("no note at {row},{col}: {text}"));
    let start = text[..end].rfind("<v:shape ").unwrap();
    text[start..end + at.len()].to_string()
}

#[test]
fn a_picture_in_a_note_box_goes_back_into_the_note() {
    let source = vml(&[note_at(1025, 0, 0, &photo_fill("rId1", 3)), note_at(1026, 1, 1, PLAIN_FILL)]);
    let (wb, rep) = read_xlsx(&fill_package(&source)).unwrap();
    assert!(!rep.warnings.iter().any(|w| w.contains("dropped")), "{:?}", rep.warnings);
    assert_eq!(wb.sheet(0).unwrap().comments.len(), 2);
    let out = write_xlsx(&wb).unwrap();
    let u = Unpacked::new(&out);
    u.assert_valid();
    let (text, part) = written_vml(&u);
    // The first note's box shows the picture, once named; the second keeps the plain fill.
    let first = shape_of(&text, 0, 0);
    assert_eq!(count(&first, "o:relid=\"rId1\""), 1, "the repeated attribute is written once: {first}");
    assert!(first.contains("type=\"frame\"") && first.contains("o:title=\"photo\""), "{first}");
    assert!(!first.contains(PLAIN_FILL), "{first}");
    assert!(shape_of(&text, 1, 1).contains(PLAIN_FILL), "{text}");
    // The drawing's relationship leads to the image, byte for byte; the other image is not used.
    let image = u.rels(&part);
    assert_eq!(image.len(), 1, "{image:?}");
    assert_eq!((image[0].0.as_str(), u.parts.get(&image[0].2).unwrap().as_slice()), ("rId1", PHOTO));
    assert!(u.find(PHOTO2).is_empty(), "{:?}", u.find(PHOTO2));
    // The written drawing is well-formed, so the next read finds the picture again.
    assert!(xml::parse(text.as_bytes()).is_ok(), "{text}");
    let (wb2, rep2) = read_xlsx(&out).unwrap();
    assert!(!rep2.warnings.iter().any(|w| w.contains("dropped")), "{:?}", rep2.warnings);
    let again = Unpacked::new(&write_xlsx(&wb2).unwrap());
    again.assert_valid();
    let (text2, part2) = written_vml(&again);
    assert_eq!(count(&shape_of(&text2, 0, 0), "o:relid=\"rId1\""), 1);
    assert_eq!(again.find(PHOTO).len(), 1);
    assert_eq!(again.rels(&part2).len(), 1);
}

#[test]
fn the_picture_of_a_note_that_is_gone_does_not_stay_in_the_file() {
    let source = vml(&[note_at(1025, 0, 0, &photo_fill("rId1", 1)), note_at(1026, 1, 1, &photo_fill("rId2", 1))]);
    let (mut wb, _) = read_xlsx(&fill_package(&source)).unwrap();
    wb.sheet_mut(0).unwrap().comments.remove(&CellRef::parse("A1").unwrap());
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    u.assert_valid();
    let (text, part) = written_vml(&u);
    assert!(text.contains("o:relid=\"rId2\"") && !text.contains("o:relid=\"rId1\""), "{text}");
    assert_eq!(u.find(PHOTO).len(), 0, "the picture of the deleted note is not written");
    assert_eq!(u.find(PHOTO2).len(), 1);
    assert_eq!(u.rels(&part).len(), 1);

    // With no note left the drawing is gone, and so are the pictures.
    wb.sheet_mut(0).unwrap().comments.clear();
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    u.assert_valid();
    assert!(u.find(PHOTO).is_empty() && u.find(PHOTO2).is_empty());
    assert!(u.parts.keys().all(|n| !n.ends_with(".vml")));
}

#[test]
fn note_pictures_and_kept_shapes_share_the_drawing() {
    // The drawing also holds a check box: it is kept with its relationships, and the note's
    // picture is one of them.
    let source = vml(&[checkbox(1025, "Done"), note_at(1026, 0, 0, &photo_fill("rId2", 1)), note_at(1027, 1, 1, PLAIN_FILL)]);
    let (wb, rep) = read_xlsx(&fill_package(&source)).unwrap();
    assert!(!rep.warnings.iter().any(|w| w.contains("dropped")), "{:?}", rep.warnings);
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    u.assert_valid();
    let (text, part) = written_vml(&u);
    assert_eq!(count(&text, "ObjectType=\"Checkbox\""), 1, "{text}");
    assert!(shape_of(&text, 0, 0).contains("o:relid=\"rId2\""), "{text}");
    assert!(shape_of(&text, 1, 1).contains(PLAIN_FILL), "{text}");
    // Each image is written once, under the id the drawing's text names.
    assert_eq!(u.find(PHOTO2).len(), 1);
    let by_id: Vec<(String, bool)> = u.rels(&part).into_iter().map(|r| (r.0, u.parts.get(&r.2).is_some_and(|d| d == PHOTO2))).collect();
    assert!(by_id.contains(&("rId2".to_string(), true)), "{by_id:?}");
}

#[test]
fn a_note_picture_the_drawing_has_no_relationship_for_is_reported() {
    let source = vml(&[note_at(1025, 0, 0, &photo_fill("rId9", 1)), note_at(1026, 1, 1, &photo_fill("rId1", 1))]);
    let (wb, rep) = read_xlsx(&fill_package(&source)).unwrap();
    assert!(rep.warnings.iter().any(|w| w.contains("A1") && w.contains("picture") && w.contains("dropped")), "{:?}", rep.warnings);
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    u.assert_valid();
    let (text, _) = written_vml(&u);
    assert!(shape_of(&text, 0, 0).contains(PLAIN_FILL), "{text}");
    assert!(shape_of(&text, 1, 1).contains("o:relid=\"rId1\""), "{text}");
}
