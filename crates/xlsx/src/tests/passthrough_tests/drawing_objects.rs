//! What a sheet's drawing holds that the model doesn't: the shapes part of a chart
//! (`c:userShapes`) and the hyperlinks of pictures and shapes. Both go back with the object they
//! belong to, with fresh relationship ids and every part they name.

use std::collections::HashSet;

use super::*;

const CT_PREFIX: &str = "application/vnd.openxmlformats-officedocument";
const NS_XDR: &str = "http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing";
const NS_A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const PNG: &[u8] = b"\x89PNG\r\n\x1a\n not really a picture";

fn types(overrides: &[(&str, &str)]) -> String {
    let mut s = format!(
        r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="xml" ContentType="application/xml"/><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="png" ContentType="image/png"/><Override PartName="/xl/workbook.xml" ContentType="{CT_PREFIX}.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="{CT_PREFIX}.spreadsheetml.worksheet+xml"/>"#
    );
    for (part, ct) in overrides {
        s.push_str(&format!(r#"<Override PartName="/{part}" ContentType="{CT_PREFIX}.{ct}"/>"#));
    }
    s.push_str("</Types>");
    s
}

fn anchor(col: u32, body: &str) -> String {
    format!(
        r#"<xdr:twoCellAnchor><xdr:from><xdr:col>{col}</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>1</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from><xdr:to><xdr:col>{}</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>8</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:to>{body}<xdr:clientData/></xdr:twoCellAnchor>"#,
        col + 3
    )
}

/// One sheet with a drawing; `drawing_rels` and `extra` add to it.
fn package(drawing_body: &str, drawing_rels: &[(&str, &str, &str)], extra: &[(&str, &[u8])], overrides: &[(&str, &str)]) -> Vec<u8> {
    let workbook = format!(
        r#"<?xml version="1.0"?><workbook xmlns="{NS_MAIN}" xmlns:r="{NS_R}"><sheets><sheet name="Data" sheetId="1" r:id="rId1"/></sheets></workbook>"#
    );
    let sheet = format!(
        r#"<?xml version="1.0"?><worksheet xmlns="{NS_MAIN}" xmlns:r="{NS_R}"><sheetData><row r="1"><c r="A1"><v>1</v></c></row><row r="2"><c r="A2"><v>2</v></c></row></sheetData><drawing r:id="rId1"/></worksheet>"#
    );
    let drawing = format!(r#"<?xml version="1.0"?><xdr:wsDr xmlns:xdr="{NS_XDR}" xmlns:a="{NS_A}" xmlns:r="{NS_R}">{drawing_body}</xdr:wsDr>"#);
    let mut overrides = overrides.to_vec();
    overrides.push(("xl/drawings/drawing1.xml", "drawing+xml"));
    let types = types(&overrides);
    let wb_rels = rels(&[("rId1", "worksheet", "worksheets/sheet1.xml")]);
    let sheet_rels = rels(&[("rId1", "drawing", "../drawings/drawing1.xml")]);
    let drawing_rels = rels(drawing_rels);
    let mut parts: Vec<(&str, &[u8])> = vec![
        ("[Content_Types].xml", types.as_bytes()),
        ("_rels/.rels", super::super::ROOT_RELS.as_bytes()),
        ("xl/workbook.xml", workbook.as_bytes()),
        ("xl/_rels/workbook.xml.rels", wb_rels.as_bytes()),
        ("xl/worksheets/sheet1.xml", sheet.as_bytes()),
        ("xl/worksheets/_rels/sheet1.xml.rels", sheet_rels.as_bytes()),
        ("xl/drawings/drawing1.xml", drawing.as_bytes()),
        ("xl/drawings/_rels/drawing1.xml.rels", drawing_rels.as_bytes()),
    ];
    parts.extend_from_slice(extra);
    make_zip(&parts)
}

// ---------------------------------------------------------------- chart shapes

fn user_shapes() -> String {
    format!(
        r#"<?xml version="1.0"?><c:userShapes xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart"><cdr:relSizeAnchor xmlns:cdr="http://schemas.openxmlformats.org/drawingml/2006/chartDrawing"><cdr:from><cdr:x>0.1</cdr:x><cdr:y>0.1</cdr:y></cdr:from><cdr:to><cdr:x>0.4</cdr:x><cdr:y>0.3</cdr:y></cdr:to><cdr:sp macro="" textlink=""><cdr:nvSpPr><cdr:cNvPr id="2" name="Callout"/><cdr:cNvSpPr/></cdr:nvSpPr><cdr:spPr><a:prstGeom xmlns:a="{NS_A}" prst="rect"><a:avLst/></a:prstGeom></cdr:spPr><cdr:txBody><a:bodyPr xmlns:a="{NS_A}"/><a:p xmlns:a="{NS_A}"><a:r><a:t>Peak load</a:t></a:r></a:p></cdr:txBody></cdr:sp></cdr:relSizeAnchor><cdr:relSizeAnchor xmlns:cdr="http://schemas.openxmlformats.org/drawingml/2006/chartDrawing"><cdr:from><cdr:x>0.6</cdr:x><cdr:y>0.1</cdr:y></cdr:from><cdr:to><cdr:x>0.8</cdr:x><cdr:y>0.3</cdr:y></cdr:to><cdr:pic><cdr:nvPicPr><cdr:cNvPr id="3" name="Badge" descr="ChartBadge"/><cdr:cNvPicPr/></cdr:nvPicPr><cdr:blipFill><a:blip xmlns:a="{NS_A}" xmlns:r="{NS_R}" r:embed="rId1"/></cdr:blipFill><cdr:spPr/></cdr:pic></cdr:relSizeAnchor></c:userShapes>"#
    )
}

fn chart_package() -> (Vec<u8>, String) {
    let frame = r#"<xdr:graphicFrame macro=""><xdr:nvGraphicFramePr><xdr:cNvPr id="2" name="Chart 1"/><xdr:cNvGraphicFramePr/></xdr:nvGraphicFramePr><xdr:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/></xdr:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/chart"><c:chart xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" r:id="rId1"/></a:graphicData></a:graphic></xdr:graphicFrame>"#;
    let chart = format!(
        r#"<?xml version="1.0"?><c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:r="{NS_R}"><c:chart><c:plotArea><c:lineChart><c:grouping val="standard"/><c:ser><c:idx val="0"/><c:order val="0"/><c:val><c:numRef><c:f>Data!$A$1:$A$2</c:f></c:numRef></c:val></c:ser><c:axId val="1"/><c:axId val="2"/></c:lineChart><c:catAx><c:axId val="1"/><c:crossAx val="2"/></c:catAx><c:valAx><c:axId val="2"/><c:crossAx val="1"/></c:valAx></c:plotArea></c:chart><c:userShapes r:id="rId7"/></c:chartSpace>"#
    );
    let shapes = user_shapes();
    let chart_rels = rels(&[("rId7", "chartUserShapes", "../drawings/drawing2.xml")]);
    let shape_rels = rels(&[("rId1", "image", "../media/badge.png")]);
    let bytes = package(
        &anchor(2, frame),
        &[("rId1", "chart", "../charts/chart1.xml")],
        &[
            ("xl/charts/chart1.xml", chart.as_bytes()),
            ("xl/charts/_rels/chart1.xml.rels", chart_rels.as_bytes()),
            ("xl/drawings/drawing2.xml", shapes.as_bytes()),
            ("xl/drawings/_rels/drawing2.xml.rels", shape_rels.as_bytes()),
            ("xl/media/badge.png", PNG),
        ],
        &[("xl/charts/chart1.xml", "drawingml.chart+xml"), ("xl/drawings/drawing2.xml", "drawingml.chartshapes+xml")],
    );
    (bytes, shapes)
}

/// The shapes part of the only chart of sheet "Data", as (part name, XML), if it has one.
fn written_chart_shapes(u: &Unpacked) -> Option<(String, String)> {
    let sheet = sheet_part(u, "Data");
    let (_, drawing) = u.rels(&sheet).into_iter().find(|r| r.1.ends_with("/drawing")).map(|r| (r.0, r.2))?;
    let chart = u.rels(&drawing).into_iter().find(|r| r.1.ends_with("/chart"))?.2;
    let (id, _, part, _) = u.rels(&chart).into_iter().find(|r| r.1.ends_with("/chartUserShapes"))?;
    assert_eq!(attr_of(&u.text(&chart), "c:userShapes", "r:id"), id);
    let xml = u.text(&part);
    Some((part, xml))
}

#[test]
fn chart_shapes_part_goes_back_with_its_chart() {
    let (bytes, shapes) = chart_package();
    let (wb, report) = read_xlsx(&bytes).unwrap();
    let sheet = wb.sheet(0).unwrap();
    assert_eq!((sheet.charts.len(), sheet.shapes.len(), sheet.images.len()), (1, 1, 1), "{:?}", report.warnings);
    let out = write_xlsx(&wb).unwrap();
    let u = Unpacked::new(&out);
    u.assert_valid();
    let (part, xml) = written_chart_shapes(&u).expect("the chart lost its shapes part");
    assert_eq!(xml, shapes, "the shapes part is written as the file had it");
    let (_, ty, image, _) = u.rels(&part).into_iter().next().expect("the picture's relationship");
    assert!(ty.ends_with("/image"));
    assert_eq!(u.parts.get(&image).map(Vec::as_slice), Some(PNG));
    // The shapes are not written a second time as objects of the sheet: the drawing holds the
    // chart's frame only.
    let sheet = sheet_part(&u, "Data");
    let drawing = u.rels(&sheet).into_iter().find(|r| r.1.ends_with("/drawing")).unwrap().2;
    let dx = u.text(&drawing);
    assert_eq!((count(&dx, "<xdr:sp>") + count(&dx, "<xdr:sp "), count(&dx, "<xdr:pic>") + count(&dx, "<xdr:pic ")), (0, 0), "{dx}");
    assert_eq!((count(&dx, "Peak load"), count(&dx, "<xdr:graphicFrame")), (0, 1), "{dx}");
    // And again: the second save writes what the first read back.
    let (again, _) = read_xlsx(&out).unwrap();
    let u2 = Unpacked::new(&write_xlsx(&again).unwrap());
    assert_eq!(written_chart_shapes(&u2).map(|(_, x)| x), Some(shapes));
}

fn assert_no_shapes_part(u: &Unpacked, kept_part: &str) {
    let types = u.text("[Content_Types].xml");
    assert!(!types.contains("chartshapes+xml"), "a content type for a shapes part");
    assert!(!types.contains(&format!("PartName=\"/{kept_part}\"")), "an orphan content type override for {kept_part}");
    assert!(!u.parts.contains_key(kept_part), "an orphan shapes part: {kept_part}");
}

fn assert_reopened_chart_objects(bytes: &[u8], charts: usize, text: &str) {
    let (wb, _) = read_xlsx(bytes).unwrap();
    let sheet = wb.sheet(0).unwrap();
    assert_eq!((sheet.charts.len(), sheet.shapes.len(), sheet.images.len()), (charts, 1, 1));
    assert_eq!(sheet.shapes[0].text, text);
    assert_eq!(sheet.images[0].alt, "ChartBadge");
    assert_eq!(sheet.images[0].data.as_slice(), PNG);
}

/// How many times `s` is in `x`.
fn count(x: &str, s: &str) -> usize {
    x.matches(s).count()
}

#[test]
fn chart_shapes_become_sheet_shapes_once_the_chart_is_gone() {
    let (bytes, _) = chart_package();
    let (mut wb, _) = read_xlsx(&bytes).unwrap();
    let (part, _) = written_chart_shapes(&Unpacked::new(&write_xlsx(&wb).unwrap())).unwrap();
    Arc::make_mut(&mut wb.sheets[0]).charts.clear();
    let out = write_xlsx(&wb).unwrap();
    let u = Unpacked::new(&out);
    u.assert_valid();
    assert!(written_chart_shapes(&u).is_none());
    assert_no_shapes_part(&u, &part);
    let dx = u.text(&written_drawing(&u));
    assert_eq!((count(&dx, "<xdr:sp>") + count(&dx, "<xdr:sp "), count(&dx, "<xdr:pic>") + count(&dx, "<xdr:pic ")), (1, 1), "{dx}");
    assert_eq!((count(&dx, "Peak load"), count(&dx, "ChartBadge")), (1, 1), "the shapes show on the sheet once: {dx}");
    assert_reopened_chart_objects(&out, 0, "Peak load");
}

#[test]
fn edited_chart_shapes_are_written_as_they_show() {
    let (bytes, _) = chart_package();
    let (mut wb, _) = read_xlsx(&bytes).unwrap();
    let (part, _) = written_chart_shapes(&Unpacked::new(&write_xlsx(&wb).unwrap())).unwrap();
    Arc::make_mut(&mut wb.sheets[0]).shapes[0].text = "Edited".into();
    let out = write_xlsx(&wb).unwrap();
    let u = Unpacked::new(&out);
    u.assert_valid();
    assert!(written_chart_shapes(&u).is_none(), "the kept part no longer describes the shapes");
    assert_no_shapes_part(&u, &part);
    let dx = u.text(&written_drawing(&u));
    assert_eq!((count(&dx, "Edited"), count(&dx, "Peak load"), count(&dx, "ChartBadge")), (1, 0, 1), "{dx}");
    assert_reopened_chart_objects(&out, 1, "Edited");
}

#[test]
fn a_new_chart_cannot_take_a_deleted_charts_annotation_part() {
    let (bytes, _) = chart_package();
    let (mut wb, _) = read_xlsx(&bytes).unwrap();
    let (part, _) = written_chart_shapes(&Unpacked::new(&write_xlsx(&wb).unwrap())).unwrap();
    let sheet = Arc::make_mut(&mut wb.sheets[0]);
    let old = sheet.charts.remove(0);
    // A live object at the top of the id space makes the allocator reuse gaps below it.
    sheet.shapes.push(gridcraft_model::Shape { id: u32::MAX, ..sheet.shapes[0].clone() });
    let id = wb.next_object_id();
    Arc::make_mut(&mut wb.sheets[0]).charts.push(gridcraft_model::Chart { id, ..old });
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    u.assert_valid();
    assert!(written_chart_shapes(&u).is_none(), "the replacement chart inherited the deleted chart's annotations");
    assert_no_shapes_part(&u, &part);
    assert_eq!(wb.sheet(0).unwrap().shapes[1].id, u32::MAX);
}

#[test]
fn an_equal_replacement_annotation_is_written_as_a_sheet_image() {
    let (bytes, _) = chart_package();
    let (mut wb, _) = read_xlsx(&bytes).unwrap();
    let (part, _) = written_chart_shapes(&Unpacked::new(&write_xlsx(&wb).unwrap())).unwrap();
    // The picture originally had the highest id, so deleting it exposes that id to reuse.
    let old = Arc::make_mut(&mut wb.sheets[0]).images.remove(0);
    let id = wb.next_object_id();
    Arc::make_mut(&mut wb.sheets[0]).images.push(gridcraft_model::Image { id, ..old });
    let out = write_xlsx(&wb).unwrap();
    let u = Unpacked::new(&out);
    u.assert_valid();
    assert!(written_chart_shapes(&u).is_none(), "the replacement picture took the original annotation's place");
    assert_no_shapes_part(&u, &part);
    assert_reopened_chart_objects(&out, 1, "Peak load");
}

// ---------------------------------------------------------------- object hyperlinks

fn linked_package() -> Vec<u8> {
    let pic = r#"<xdr:pic><xdr:nvPicPr><xdr:cNvPr id="2" name="Logo" descr="Logo"><a:hlinkClick r:id="rId2" tooltip="Home"/></xdr:cNvPr><xdr:cNvPicPr/></xdr:nvPicPr><xdr:blipFill><a:blip r:embed="rId1"/><a:stretch><a:fillRect/></a:stretch></xdr:blipFill><xdr:spPr><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></xdr:spPr></xdr:pic>"#;
    let sp = r#"<xdr:sp macro="" textlink=""><xdr:nvSpPr><xdr:cNvPr id="3" name="Go"><a:hlinkClick r:id="rId3"/></xdr:cNvPr><xdr:cNvSpPr/></xdr:nvSpPr><xdr:spPr><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></xdr:spPr><xdr:txBody><a:bodyPr/><a:p><a:r><a:t>Go</a:t></a:r></a:p></xdr:txBody></xdr:sp>"#;
    let body = format!("{}{}", anchor(1, pic), anchor(6, sp));
    package(
        &body,
        &[("rId1", "image", "../media/image1.png"), ("rId2", "hyperlink", "https://example.com/home"), ("rId3", "hyperlink", "#Data!A2")],
        &[("xl/media/image1.png", PNG)],
        &[],
    )
}

/// The raw relationships of `part`: (id, type, target as written, external).
fn raw_rels(u: &Unpacked, part: &str) -> Vec<(String, String, String, bool)> {
    let (dir, file) = split_dir(part);
    let Some(b) = u.parts.get(&format!("{dir}/_rels/{file}.rels")) else { return vec![] };
    xml::parse(b)
        .unwrap()
        .kids("Relationship")
        .map(|r| {
            (r.attr("Id").unwrap().into(), r.attr("Type").unwrap().into(), r.attr("Target").unwrap().into(), r.attr("TargetMode") == Some("External"))
        })
        .collect()
}

/// The hyperlink target of the object whose element holds `marker` (GridCraft names the objects
/// it writes itself), if it has one.
fn link_of(u: &Unpacked, drawing: &str, marker: &str) -> Option<(String, bool)> {
    let dx = u.text(drawing);
    let at = dx[..dx.find(marker)?].rfind("<xdr:cNvPr ")?;
    let open = at + dx[at..].find('>')?;
    if dx[..open].ends_with('/') {
        return None;
    }
    let end = at + dx[at..].find("</xdr:cNvPr>")?;
    let id = attr_of(&dx[at..end], "a:hlinkClick", "r:id");
    let (_, ty, target, ext) = raw_rels(u, drawing).into_iter().find(|r| r.0 == id).unwrap_or_else(|| panic!("{drawing}: dangling {id}"));
    assert!(ty.ends_with("/hyperlink"));
    Some((target, ext))
}

fn written_drawing(u: &Unpacked) -> String {
    let sheet = sheet_part(u, "Data");
    u.rels(&sheet).into_iter().find(|r| r.1.ends_with("/drawing")).unwrap().2
}

#[test]
fn picture_and_shape_hyperlinks_survive_a_round_trip() {
    let (wb, report) = read_xlsx(&linked_package()).unwrap();
    assert_eq!((wb.sheet(0).unwrap().images.len(), wb.sheet(0).unwrap().shapes.len()), (1, 1), "{:?}", report.warnings);
    let out = write_xlsx(&wb).unwrap();
    let u = Unpacked::new(&out);
    u.assert_valid();
    let drawing = written_drawing(&u);
    assert_eq!(link_of(&u, &drawing, LOGO), Some(("https://example.com/home".into(), true)), "{}", u.text(&drawing));
    assert_eq!(link_of(&u, &drawing, GO), Some(("#Data!A2".into(), false)));
    assert!(u.text(&drawing).contains("tooltip=\"Home\""));
    let ids: Vec<String> = raw_rels(&u, &drawing).into_iter().map(|r| r.0).collect();
    assert_eq!(ids.len(), ids.iter().collect::<HashSet<_>>().len(), "duplicate ids: {ids:?}");
    // A second round trip keeps them too.
    let (again, _) = read_xlsx(&out).unwrap();
    assert_linked_objects(again.sheet(0).unwrap(), "Go", "Logo");
    let u2 = Unpacked::new(&write_xlsx(&again).unwrap());
    let d2 = written_drawing(&u2);
    assert_eq!(link_of(&u2, &d2, LOGO), Some(("https://example.com/home".into(), true)));
    assert_eq!(link_of(&u2, &d2, GO), Some(("#Data!A2".into(), false)));
}

#[test]
fn a_deleted_picture_takes_its_hyperlink_with_it() {
    let (mut wb, _) = read_xlsx(&linked_package()).unwrap();
    Arc::make_mut(&mut wb.sheets[0]).images.clear();
    let out = write_xlsx(&wb).unwrap();
    let u = Unpacked::new(&out);
    let drawing = written_drawing(&u);
    assert_eq!(link_of(&u, &drawing, GO), Some(("#Data!A2".into(), false)));
    let links: Vec<_> = raw_rels(&u, &drawing).into_iter().filter(|r| r.1.ends_with("/hyperlink")).collect();
    assert_eq!(links.len(), 1, "{links:?}");
    let (again, _) = read_xlsx(&out).unwrap();
    let sheet = again.sheet(0).unwrap();
    assert!(sheet.images.is_empty());
    assert_eq!(sheet.shapes[0].text, "Go");
}

#[test]
fn a_new_object_never_gets_a_deleted_objects_hyperlink() {
    let (mut wb, _) = read_xlsx(&linked_package()).unwrap();
    let image_id = wb.sheet(0).unwrap().images[0].id;
    let old = Arc::make_mut(&mut wb.sheets[0]).shapes.remove(0);
    // A new shape like the deleted one in every way but its id: the id the model gives a new
    // object is never one a kept hyperlink is bound to.
    let id = wb.next_object_id();
    assert_ne!(id, old.id);
    Arc::make_mut(&mut wb.sheets[0]).shapes.push(gridcraft_model::Shape { id, ..old });
    let out = write_xlsx(&wb).unwrap();
    let u = Unpacked::new(&out);
    u.assert_valid();
    let drawing = written_drawing(&u);
    assert_eq!(link_of(&u, &drawing, GO), None, "{}", u.text(&drawing));
    assert!(!raw_rels(&u, &drawing).iter().any(|r| r.2 == "#Data!A2"), "no relationship is left for the dropped link");
    // The picture keeps its own.
    assert_eq!(link_of(&u, &drawing, LOGO), Some(("https://example.com/home".into(), true)));
    assert_eq!(wb.sheet(0).unwrap().images[0].id, image_id);
    let (again, _) = read_xlsx(&out).unwrap();
    assert_linked_objects(again.sheet(0).unwrap(), "Go", "Logo");
}

#[test]
fn edited_objects_keep_their_hyperlinks_and_undo_restores_a_deleted_ones() {
    let (mut wb, _) = read_xlsx(&linked_package()).unwrap();
    let read = wb.clone();
    let sheet = Arc::make_mut(&mut wb.sheets[0]);
    sheet.shapes[0].text = "Edited".into();
    sheet.shapes[0].anchor.cell = gridcraft_core::CellRef::new(20, 20);
    sheet.images[0].alt = "New logo".into();
    let out = write_xlsx(&wb).unwrap();
    let u = Unpacked::new(&out);
    u.assert_valid();
    let drawing = written_drawing(&u);
    assert_eq!(link_of(&u, &drawing, "<a:t>Edited</a:t>"), Some(("#Data!A2".into(), false)), "{}", u.text(&drawing));
    assert_eq!(link_of(&u, &drawing, "descr=\"New logo\""), Some(("https://example.com/home".into(), true)));
    let (again, _) = read_xlsx(&out).unwrap();
    let sheet = again.sheet(0).unwrap();
    assert_linked_objects(sheet, "Edited", "New logo");
    assert_eq!(sheet.shapes[0].anchor.cell, gridcraft_core::CellRef::new(20, 20));
    // Deleted, then back as it was (what undo does): the link comes back with it.
    let mut wb = read.clone();
    Arc::make_mut(&mut wb.sheets[0]).shapes.clear();
    let out = write_xlsx(&wb).unwrap();
    let u = Unpacked::new(&out);
    assert!(!u.text(&written_drawing(&u)).contains(GO));
    let (again, _) = read_xlsx(&out).unwrap();
    assert!(again.sheet(0).unwrap().shapes.is_empty());
    let out = write_xlsx(&read).unwrap();
    let u = Unpacked::new(&out);
    assert_eq!(link_of(&u, &written_drawing(&u), GO), Some(("#Data!A2".into(), false)));
    let (again, _) = read_xlsx(&out).unwrap();
    assert_linked_objects(again.sheet(0).unwrap(), "Go", "Logo");
}

fn assert_linked_objects(sheet: &gridcraft_model::Sheet, text: &str, alt: &str) {
    assert_eq!((sheet.shapes.len(), sheet.images.len()), (1, 1));
    assert_eq!(sheet.shapes[0].text, text);
    assert_eq!(sheet.images[0].alt, alt);
    assert_eq!(sheet.images[0].data.as_slice(), PNG);
}

/// `bytes` with part `name` holding `data`.
fn with_part(bytes: &[u8], name: &str, data: &str) -> Vec<u8> {
    let mut parts = Unpacked::new(bytes).parts;
    parts.insert(name.to_string(), data.as_bytes().to_vec());
    let list: Vec<(&str, &[u8])> = parts.iter().map(|(k, v)| (k.as_str(), v.as_slice())).collect();
    make_zip(&list)
}

#[test]
fn deleting_a_link_writes_no_orphan_sound_part() {
    let input = Unpacked::new(&linked_package());
    let drawing = "xl/drawings/drawing1.xml";
    let rel_part = "xl/drawings/_rels/drawing1.xml.rels";
    let sound = "xl/media/chime.wav";
    let wave = b"RIFF\x24\0\0\0WAVEfmt \x10\0\0\0\x01\0\x01\0\x40\x1f\0\0\x40\x1f\0\0\x01\0\x08\0data\0\0\0\0";
    let drawing_xml = input
        .text(drawing)
        .replace("<a:hlinkClick r:id=\"rId3\"/>", "<a:hlinkClick r:id=\"rId3\"><a:snd r:embed=\"rId4\" name=\"Chime\"/></a:hlinkClick>");
    let shared_drawing = drawing_xml.replace(
        "<a:hlinkClick r:id=\"rId2\" tooltip=\"Home\"/>",
        "<a:hlinkClick r:id=\"rId2\" tooltip=\"Home\"><a:snd r:embed=\"rId4\" name=\"Chime\"/></a:hlinkClick>",
    );
    let rel_xml = input
        .text(rel_part)
        .replace("</Relationships>", &format!("<Relationship Id=\"rId4\" Type=\"{NS_R}/audio\" Target=\"../media/chime.wav\"/></Relationships>"));
    let types = input.text("[Content_Types].xml").replace("</Types>", "<Default Extension=\"wav\" ContentType=\"audio/wav\"/></Types>");
    let mut files = input.parts;
    files.insert(drawing.into(), drawing_xml.into_bytes());
    files.insert(rel_part.into(), rel_xml.into_bytes());
    files.insert(sound.into(), wave.to_vec());
    files.insert("[Content_Types].xml".into(), types.into_bytes());
    let parts: Vec<_> = files.iter().map(|(k, v)| (k.as_str(), v.as_slice())).collect();
    let (mut wb, _) = read_xlsx(&make_zip(&parts)).unwrap();
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    let drawing = written_drawing(&u);
    let id = attr_of(&u.text(&drawing), "a:snd", "r:embed");
    let (_, target) = u.target(&drawing, &id);
    assert_eq!(u.parts[&target], wave);
    // If the picture also uses the sound, deleting the shape must not drop the shared part.
    let shared = with_part(&make_zip(&parts), "xl/drawings/drawing1.xml", &shared_drawing);
    let (mut shared, _) = read_xlsx(&shared).unwrap();
    Arc::make_mut(&mut shared.sheets[0]).shapes.clear();
    let shared = Unpacked::new(&write_xlsx(&shared).unwrap());
    shared.assert_valid();
    let drawing = written_drawing(&shared);
    let id = attr_of(&shared.text(&drawing), "a:snd", "r:embed");
    let (_, target) = shared.target(&drawing, &id);
    assert_eq!(shared.parts[&target], wave);
    assert_eq!(shared.find(wave).len(), 1);

    Arc::make_mut(&mut wb.sheets[0]).shapes.clear();
    let out = write_xlsx(&wb).unwrap();
    let u = Unpacked::new(&out);
    u.assert_valid();
    assert!(u.find(wave).is_empty(), "the deleted hyperlink left its audio in the saved file");
    assert!(!u.text("[Content_Types].xml").contains("audio/wav"));
    let (again, _) = read_xlsx(&out).unwrap();
    assert_eq!(again.sheet(0).unwrap().images[0].alt, "Logo");
    assert_eq!(link_of(&u, &written_drawing(&u), LOGO), Some(("https://example.com/home".into(), true)));
}

#[test]
fn a_place_link_in_external_mode_is_checked_too() {
    let rels = format!(
        r##"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{NS_R}/image" Target="../media/image1.png"/><Relationship Id="rId2" Type="{NS_R}/hyperlink" Target="https://example.com/home" TargetMode="External"/><Relationship Id="rId3" Type="{NS_R}/hyperlink" Target="#Data!A2" TargetMode="External"/></Relationships>"##
    );
    let bytes = with_part(&linked_package(), "xl/drawings/_rels/drawing1.xml.rels", &rels);
    let (wb, _) = read_xlsx(&bytes).unwrap();
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    assert_eq!(link_of(&u, &written_drawing(&u), GO), Some(("#Data!A2".into(), true)), "kept while Data is there");
    let mut wb = wb;
    Arc::make_mut(&mut wb.sheets[0]).name = "Renamed".into();
    let (out, report) = crate::write_xlsx_named_with(&wb, crate::XlsxKind::Workbook, "drawing.xlsx", crate::FunctionInfo::default()).unwrap();
    assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
    assert!(report.warnings[0].contains("#Data!A2"));
    let u = Unpacked::new(&out);
    u.assert_valid();
    let sheet = sheet_part(&u, "Renamed");
    let drawing = u.rels(&sheet).into_iter().find(|r| r.1.ends_with("/drawing")).unwrap().2;
    assert_eq!(link_of(&u, &drawing, GO), None);
    assert!(!raw_rels(&u, &drawing).iter().any(|r| r.2.starts_with('#')));
    let (again, _) = read_xlsx(&out).unwrap();
    let sheet = again.sheet(0).unwrap();
    assert_eq!(sheet.name, "Renamed");
    assert_linked_objects(sheet, "Go", "Logo");
}

#[test]
fn a_link_to_a_sheets_name_needs_that_sheets_name() {
    let rels = format!(
        r##"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{NS_R}/image" Target="../media/image1.png"/><Relationship Id="rId2" Type="{NS_R}/hyperlink" Target="https://example.com/home" TargetMode="External"/><Relationship Id="rId3" Type="{NS_R}/hyperlink" Target="#Data!LocalSource"/></Relationships>"##
    );
    let workbook = format!(
        r#"<?xml version="1.0"?><workbook xmlns="{NS_MAIN}" xmlns:r="{NS_R}"><sheets><sheet name="Data" sheetId="1" r:id="rId1"/></sheets><definedNames><definedName name="LocalSource" localSheetId="0">Data!$A$1</definedName><definedName name="LocalSource">Data!$A$2</definedName></definedNames></workbook>"#
    );
    let bytes = with_part(&with_part(&linked_package(), "xl/drawings/_rels/drawing1.xml.rels", &rels), "xl/workbook.xml", &workbook);
    let (wb, _) = read_xlsx(&bytes).unwrap();
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    u.assert_valid();
    assert_eq!(link_of(&u, &written_drawing(&u), GO), Some(("#Data!LocalSource".into(), false)));
    // The sheet's name deleted, the workbook's same-named one still there: the link is left out.
    let mut wb = wb;
    wb.names.retain(|n| n.scope.is_none());
    let out = write_xlsx(&wb).unwrap();
    let u = Unpacked::new(&out);
    u.assert_valid();
    assert_eq!(link_of(&u, &written_drawing(&u), GO), None);
    let (again, _) = read_xlsx(&out).unwrap();
    assert_linked_objects(again.sheet(0).unwrap(), "Go", "Logo");
    assert_eq!(again.names.len(), 1);
    assert!(again.names[0].scope.is_none());
}

#[test]
fn a_click_link_survives_a_hover_link_without_its_relationship() {
    for (click, hover, kept) in [("rId3", "rId9", "click"), ("rId9", "rId3", "hover")] {
        let sp = format!(
            r#"<xdr:sp macro="" textlink=""><xdr:nvSpPr><xdr:cNvPr id="3" name="Go"><a:hlinkClick r:id="{click}"/><a:hlinkHover r:id="{hover}"/></xdr:cNvPr><xdr:cNvSpPr/></xdr:nvSpPr><xdr:spPr><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></xdr:spPr><xdr:txBody><a:bodyPr/><a:p><a:r><a:t>Go</a:t></a:r></a:p></xdr:txBody></xdr:sp>"#
        );
        let bytes = package(&anchor(6, &sp), &[("rId3", "hyperlink", "#Data!A2")], &[], &[]);
        let (wb, report) = read_xlsx(&bytes).unwrap();
        let out = write_xlsx(&wb).unwrap();
        let u = Unpacked::new(&out);
        u.assert_valid();
        let drawing = written_drawing(&u);
        let dx = u.text(&drawing);
        let (good, gone) = if kept == "click" { ("a:hlinkClick", "<a:hlinkHover") } else { ("a:hlinkHover", "<a:hlinkClick") };
        assert_eq!(count(&dx, gone), 0, "{dx}");
        let id = attr_of(&dx, good, "r:id");
        let r = raw_rels(&u, &drawing).into_iter().find(|r| r.0 == id).unwrap_or_else(|| panic!("{dx}"));
        assert_eq!(r.2, "#Data!A2", "the {kept} link is kept: {:?}", report.warnings);
        let (again, _) = read_xlsx(&out).unwrap();
        assert_eq!(again.sheet(0).unwrap().shapes[0].text, "Go");
    }
}

#[test]
fn a_link_to_a_renamed_sheet_is_left_out() {
    let (mut wb, _) = read_xlsx(&linked_package()).unwrap();
    Arc::make_mut(&mut wb.sheets[0]).name = "Renamed".into();
    let out = write_xlsx(&wb).unwrap();
    let u = Unpacked::new(&out);
    u.assert_valid();
    let sheet = sheet_part(&u, "Renamed");
    let drawing = u.rels(&sheet).into_iter().find(|r| r.1.ends_with("/drawing")).unwrap().2;
    assert_eq!(link_of(&u, &drawing, GO), None, "the link no longer leads anywhere");
    assert!(!raw_rels(&u, &drawing).iter().any(|r| r.2.starts_with('#')));
    assert_eq!(link_of(&u, &drawing, LOGO), Some(("https://example.com/home".into(), true)));
    let (again, _) = read_xlsx(&out).unwrap();
    let sheet = again.sheet(0).unwrap();
    assert_eq!(sheet.name, "Renamed");
    assert_linked_objects(sheet, "Go", "Logo");
}

#[test]
fn grouped_links_and_click_and_hover_go_back_on_their_own() {
    // A picture with only a hover link, and a shape inside a group whose hover link names no
    // relationship: its click link still goes back.
    let pic = r#"<xdr:pic><xdr:nvPicPr><xdr:cNvPr id="2" name="Logo" descr="Logo"><a:hlinkHover r:id="rId2"/></xdr:cNvPr><xdr:cNvPicPr/></xdr:nvPicPr><xdr:blipFill><a:blip r:embed="rId1"/><a:stretch><a:fillRect/></a:stretch></xdr:blipFill><xdr:spPr><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></xdr:spPr></xdr:pic>"#;
    let sp = r#"<xdr:sp macro="" textlink=""><xdr:nvSpPr><xdr:cNvPr id="3" name="Go"><a:hlinkClick r:id="rId3"/><a:hlinkHover r:id=""/></xdr:cNvPr><xdr:cNvSpPr/></xdr:nvSpPr><xdr:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="100" cy="100"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></xdr:spPr><xdr:txBody><a:bodyPr/><a:p><a:r><a:t>Go</a:t></a:r></a:p></xdr:txBody></xdr:sp>"#;
    let other = r#"<xdr:sp macro="" textlink=""><xdr:nvSpPr><xdr:cNvPr id="5" name="Other"/><xdr:cNvSpPr/></xdr:nvSpPr><xdr:spPr><a:xfrm><a:off x="100" y="0"/><a:ext cx="100" cy="100"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></xdr:spPr><xdr:txBody><a:bodyPr/><a:p><a:r><a:t>Other</a:t></a:r></a:p></xdr:txBody></xdr:sp>"#;
    let group = format!(
        r#"<xdr:grpSp><xdr:nvGrpSpPr><xdr:cNvPr id="4" name="Group"/><xdr:cNvGrpSpPr/></xdr:nvGrpSpPr><xdr:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="200" cy="100"/><a:chOff x="0" y="0"/><a:chExt cx="200" cy="100"/></a:xfrm></xdr:grpSpPr>{sp}{other}</xdr:grpSp>"#
    );
    let body = format!("{}{}", anchor(1, pic), anchor(6, &group));
    let bytes = package(
        &body,
        &[("rId1", "image", "../media/image1.png"), ("rId2", "hyperlink", "https://example.com/home"), ("rId3", "hyperlink", "#Data!A2")],
        &[("xl/media/image1.png", PNG)],
        &[],
    );
    let (wb, report) = read_xlsx(&bytes).unwrap();
    let out = write_xlsx(&wb).unwrap();
    let u = Unpacked::new(&out);
    u.assert_valid();
    let drawing = written_drawing(&u);
    let dx = u.text(&drawing);
    assert_eq!(link_of(&u, &drawing, GO), Some(("#Data!A2".into(), false)), "{dx} {:?}", report.warnings);
    assert_eq!(count(&dx, "<a:hlinkHover"), 1, "only the picture's hover link: {dx}");
    let at = dx.find(LOGO).unwrap();
    let hover = attr_of(&dx[at..], "a:hlinkHover", "r:id");
    let r = raw_rels(&u, &drawing).into_iter().find(|r| r.0 == hover).unwrap_or_else(|| panic!("dangling {hover}"));
    assert_eq!((r.2.as_str(), r.3), ("https://example.com/home", true));
    let (again, _) = read_xlsx(&out).unwrap();
    let sheet = again.sheet(0).unwrap();
    assert_eq!((sheet.groups.len(), sheet.shapes.len(), sheet.images.len()), (1, 2, 1));
    assert_eq!(sheet.shapes.iter().map(|s| s.text.as_str()).collect::<Vec<_>>(), ["Go", "Other"]);
    assert_eq!(sheet.images[0].alt, "Logo");
}

/// What marks the picture and the shape in the written drawing.
const LOGO: &str = "descr=\"Logo\"";
const GO: &str = "<a:t>Go</a:t>";
