//! Legacy form check boxes (`ObjectType="Checkbox"`, `objectType="CheckBox"`): read from the VML
//! drawing alone or with the sheet's `<controls>` and their properties parts, with their names,
//! states, links, captions and shading; saved with the parts they need and nothing of them left in
//! the kept drawing; and check boxes with settings the model can't hold, and option buttons, kept
//! as they were.

use gridcraft_core::{CellRef, Value};
use gridcraft_model::{CheckState, ControlKind, FormControl, Workbook};

use crate::package::Package;
use crate::testing::{CHECK_BOX_NAMES, legacy_check_boxes};
use crate::{read_xlsx, write_xlsx};

fn text(pkg: &mut Package<'_>, part: &str) -> String {
    String::from_utf8(pkg.read(part).unwrap().unwrap_or_else(|| panic!("missing {part}"))).unwrap()
}

/// What a check box is, as the tests compare it: (name, state, link, caption, flat, macro).
type Seen = (String, CheckState, Option<String>, String, Option<bool>, Option<String>);

fn seen(c: &FormControl) -> Seen {
    (c.display_name(), c.checked, c.link.clone(), c.text.clone(), c.no_three_d, c.kept_macro.clone())
}

fn check_boxes(wb: &Workbook) -> Vec<Seen> {
    let sh = wb.sheet(0).unwrap();
    assert!(sh.controls.iter().all(|c| c.kind == ControlKind::Check), "{:?}", sh.controls);
    sh.controls.iter().map(seen).collect()
}

fn expected() -> Vec<Seen> {
    vec![
        (CHECK_BOX_NAMES[0].into(), CheckState::Checked, Some("$B$2".into()), "Ship it".into(), Some(true), None),
        (CHECK_BOX_NAMES[1].into(), CheckState::Unchecked, None, "Second".into(), None, None),
        (CHECK_BOX_NAMES[2].into(), CheckState::Mixed, None, "Maybe".into(), None, Some("[0]!Mixed_Click".into())),
    ]
}

/// The kept VML drawing's text (the shapes GridCraft doesn't write), if any.
fn kept_vml(wb: &Workbook) -> String {
    wb.sheet(0).unwrap().preserved.as_ref().and_then(|p| p.vml.as_ref()).map(|v| String::from_utf8_lossy(&v.data).into_owned()).unwrap_or_default()
}

/// The sheet's written VML drawing.
fn written_vml(bytes: &[u8]) -> String {
    let mut pkg = Package::open(bytes).unwrap();
    let rels = pkg.rels("xl/worksheets/sheet1.xml").unwrap();
    let target = rels.iter().find(|r| r.kind == "vmlDrawing").unwrap().target.clone();
    text(&mut pkg, &target)
}

/// Both variants of the fixture: opened, the three check boxes are modelled with their names,
/// states, links, captions, shading and macro, and none of them stays in the kept drawing, which
/// keeps the option button. Saved and opened again (twice), all of it is the same, each check box
/// is written once, and the option button is still there, once.
#[test]
fn legacy_check_boxes_are_modelled_and_round_trip() {
    for x14 in [false, true] {
        let (wb, report) = read_xlsx(&legacy_check_boxes(x14)).unwrap();
        assert!(report.warnings.iter().all(|w| !w.contains("dropped")), "{x14}: {:?}", report.warnings);
        assert_eq!(check_boxes(&wb), expected(), "x14: {x14}");
        let kept = kept_vml(&wb);
        assert!(!kept.contains("ObjectType=\"Checkbox\""), "{x14}: a modelled check box is not kept: {kept}");
        assert!(kept.contains("ObjectType=\"Radio\""), "{x14}: the option button is kept: {kept}");
        if x14 {
            let controls =
                wb.sheet(0).unwrap().preserved.as_ref().and_then(|p| p.elements.iter().find(|e| e.slot == "controls")).map(|e| e.xml.clone());
            let controls = controls.unwrap_or_default();
            assert!(controls.contains("shapeId=\"1028\"") && !controls.contains("GC Check"), "{controls}");
        }
        let mut bytes = write_xlsx(&wb).unwrap();
        for round in 0..2 {
            let vml = written_vml(&bytes);
            assert_eq!(vml.matches("ObjectType=\"Checkbox\"").count(), 3, "{x14}/{round}: each check box once: {vml}");
            assert_eq!(vml.matches("ObjectType=\"Radio\"").count(), 1, "{x14}/{round}: the option button once: {vml}");
            assert_eq!(vml.matches("<x:FirstButton/>").count(), 1, "{x14}/{round}: {vml}");
            let mut pkg = Package::open(&bytes).unwrap();
            let sheet = text(&mut pkg, "xl/worksheets/sheet1.xml");
            for name in CHECK_BOX_NAMES {
                assert_eq!(sheet.matches(&format!("name=\"{name}\"")).count(), 2, "{x14}/{round}: Choice and Fallback: {sheet}");
            }
            let (back, report) = read_xlsx(&bytes).unwrap();
            assert!(report.warnings.iter().all(|w| !w.contains("dropped")), "{x14}/{round}: {:?}", report.warnings);
            assert_eq!(check_boxes(&back), expected(), "{x14}/{round}");
            assert_eq!(back.sheet(0).unwrap().value(CellRef::parse("B2").unwrap()), Value::Bool(true));
            assert!(!kept_vml(&back).contains("ObjectType=\"Checkbox\""), "{x14}/{round}");
            bytes = write_xlsx(&back).unwrap();
        }
    }
}

/// What Excel's parts say for each state, link and shading, in the properties part and the VML,
/// and the caption in the `<font>` it was read in.
#[test]
fn saved_check_boxes_write_state_link_shading_and_caption_in_both_parts() {
    let (wb, _) = read_xlsx(&legacy_check_boxes(false)).unwrap();
    let out = write_xlsx(&wb).unwrap();
    let mut pkg = Package::open(&out).unwrap();
    let rels = pkg.rels("xl/worksheets/sheet1.xml").unwrap();
    let props: Vec<String> = rels.iter().filter(|r| r.kind == "ctrlProp").map(|r| r.target.clone()).collect();
    let props: Vec<String> = props.iter().map(|p| text(&mut pkg, p)).collect();
    assert_eq!(props.len(), 3, "{props:?}");
    assert!(props.iter().all(|p| p.contains("objectType=\"CheckBox\"") && p.contains("lockText=\"1\"")), "{props:?}");
    assert!(
        props[0].contains("checked=\"Checked\"") && props[0].contains("fmlaLink=\"$B$2\"") && props[0].contains("noThreeD=\"1\""),
        "{}",
        props[0]
    );
    assert!(!props[1].contains("checked=") && !props[1].contains("noThreeD"), "{}", props[1]);
    assert!(props[2].contains("checked=\"Mixed\""), "{}", props[2]);
    let sheet = text(&mut pkg, "xl/worksheets/sheet1.xml");
    assert!(sheet.contains("macro=\"[0]!Mixed_Click\""), "{sheet}");
    let vml = written_vml(&out);
    for want in [
        "<x:Checked>1</x:Checked>",
        "<x:Checked>2</x:Checked>",
        "<x:NoThreeD/>",
        "<x:FmlaLink>$B$2</x:FmlaLink>",
        "<x:FmlaMacro>[0]!Mixed_Click</x:FmlaMacro>",
        r#"<div style="text-align:left"><font face="Segoe UI" size="160" color="auto">Ship it</font></div>"#,
        r#"<div style="text-align:left">Second</div>"#,
    ] {
        assert!(vml.contains(want), "{want}: {vml}");
    }
}

/// A changed state, caption and link are what the next save writes, and read back.
#[test]
fn edited_check_boxes_save_their_new_state() {
    let (mut wb, _) = read_xlsx(&legacy_check_boxes(true)).unwrap();
    if let Some(sh) = wb.sheet_mut(0) {
        let c = &mut sh.controls[1];
        c.checked = CheckState::Checked;
        c.text = "Second & last".into();
        c.link = Some("$C$4".into());
        sh.controls[0].checked = CheckState::Unchecked;
    }
    let (back, _) = read_xlsx(&write_xlsx(&wb).unwrap()).unwrap();
    let got = check_boxes(&back);
    assert_eq!(got[0].1, CheckState::Unchecked);
    assert_eq!((got[1].1, got[1].2.as_deref(), got[1].3.as_str()), (CheckState::Checked, Some("$C$4"), "Second & last"));
}

#[test]
fn check_box_caption_and_macro_round_trip_as_text() {
    let (mut wb, _) = read_xlsx(&legacy_check_boxes(false)).unwrap();
    let c = &mut wb.sheet_mut(0).unwrap().controls[0];
    let caption = "  Pick <é>\nOr & choose\r  ";
    let macro_ref = " [0]!Run\t ";
    c.text = caption.into();
    c.kept_macro = Some(macro_ref.into());
    let bytes = write_xlsx(&wb).unwrap();
    let vml_only = rebuild_with_vml(&written_vml(&bytes));
    for input in [&bytes, &vml_only] {
        let (back, _) = read_xlsx(input).unwrap();
        let boxes = &back.sheet(0).unwrap().controls;
        assert_eq!(boxes.len(), 3);
        assert_eq!(boxes[0].text, caption);
        assert_eq!(boxes[0].kept_macro.as_deref(), Some(macro_ref));
    }
}

#[test]
fn vml_only_reader_recovers_names_and_distinct_shape_numbers() {
    let (mut wb, _) = read_xlsx(&legacy_check_boxes(false)).unwrap();
    wb.sheet_mut(0).unwrap().controls[0].name = "First Box".into();
    wb.sheet_mut(0).unwrap().controls[1].name = " Check \"<&>\" ".into();
    wb.sheet_mut(0).unwrap().controls[2].name = "C".repeat(256);
    let out = write_xlsx(&wb).unwrap();
    let vml = written_vml(&out);
    let (back, _) = read_xlsx(&rebuild_with_vml(&vml)).unwrap();
    assert_eq!(check_boxes(&back), check_boxes(&wb));
    let (back, _) = read_xlsx(&out).unwrap();
    assert_eq!(check_boxes(&back), check_boxes(&wb));
    assert_eq!(kept_vml(&back).matches("ObjectType=\"Radio\"").count(), 1);
    assert!(!kept_vml(&back).contains("ObjectType=\"Checkbox\""));
}

#[test]
fn explicit_false_vml_anchor_flags_move_and_size_with_cells() {
    let base = legacy_check_boxes(false);
    let target = "xl/drawings/vmlDrawing1.vml";
    let vml = text(&mut Package::open(&base).unwrap(), target).replacen(
        "ObjectType=\"Checkbox\"><x:Anchor>",
        "ObjectType=\"Checkbox\"><x:MoveWithCells>False</x:MoveWithCells><x:SizeWithCells>False</x:SizeWithCells><x:Anchor>",
        1,
    );
    let (wb, _) = read_xlsx(&replace_part(&base, target, &vml)).unwrap();
    let (ordinary, _) = read_xlsx(&base).unwrap();
    assert_eq!(wb.sheet(0).unwrap().controls[0].anchor, ordinary.sheet(0).unwrap().controls[0].anchor);
}

#[test]
fn x14_checkbox_properties_are_preserved_when_type_is_inferred() {
    let original = legacy_check_boxes(true);
    let target = "xl/ctrlProps/ctrlProp1.xml";
    let props = text(&mut Package::open(&original).unwrap(), target);
    let unknown = props.replace("objectType=\"CheckBox\"", "colored=\"1\"");
    let (wb, _) = read_xlsx(&replace_part(&original, target, &unknown)).unwrap();
    assert_eq!(check_boxes(&wb), expected()[1..]);
    let out = write_xlsx(&wb).unwrap();
    let mut pkg = Package::open(&out).unwrap();
    let rels = pkg.rels("xl/worksheets/sheet1.xml").unwrap();
    assert!(rels.iter().filter(|r| r.kind == "ctrlProp").any(|r| text(&mut pkg, &r.target).contains("colored=\"1\"")));
}

#[test]
fn x14_default_caption_lock_is_not_changed_on_save() {
    let original = legacy_check_boxes(true);
    let target = "xl/ctrlProps/ctrlProp1.xml";
    let props = text(&mut Package::open(&original).unwrap(), target).replace(" lockText=\"1\"", "");
    let (wb, _) = read_xlsx(&replace_part(&original, target, &props)).unwrap();
    assert_eq!(check_boxes(&wb), expected());
    let out = write_xlsx(&wb).unwrap();
    let mut pkg = Package::open(&out).unwrap();
    let rels = pkg.rels("xl/worksheets/sheet1.xml").unwrap();
    let checked = rels
        .iter()
        .filter(|r| r.kind == "ctrlProp")
        .filter_map(|r| pkg.read_xml(&r.target).unwrap())
        .find(|p| p.attr("checked") == Some("Checked"))
        .unwrap();
    assert_ne!(checked.attr_bool("lockText"), Some(true));
}

#[test]
fn vml_caption_lock_survives_with_or_without_x14() {
    let target = "xl/drawings/vmlDrawing1.vml";
    for x14 in [false, true] {
        let original = legacy_check_boxes(x14);
        let vml = text(&mut Package::open(&original).unwrap(), target).replacen(
            "<x:Checked>1</x:Checked>",
            "<x:Checked>1</x:Checked><x:LockText>False</x:LockText>",
            1,
        );
        let (wb, _) = read_xlsx(&replace_part(&original, target, &vml)).unwrap();
        assert_eq!(check_boxes(&wb), expected());
        let out = write_xlsx(&wb).unwrap();
        assert_eq!(written_vml(&out).matches("<x:LockText>False</x:LockText>").count(), 1);
        let mut pkg = Package::open(&out).unwrap();
        let rels = pkg.rels("xl/worksheets/sheet1.xml").unwrap();
        let checked = rels
            .iter()
            .filter(|r| r.kind == "ctrlProp")
            .filter_map(|r| pkg.read_xml(&r.target).unwrap())
            .find(|p| p.attr("checked") == Some("Checked"))
            .unwrap();
        assert_eq!(checked.attr_bool("lockText").unwrap_or(false), x14);
    }
}

#[test]
fn supported_shape_style_and_aspect_lock_survive_vml_export() {
    for x14 in [false, true] {
        let original = legacy_check_boxes(x14);
        let target = "xl/drawings/vmlDrawing1.vml";
        let vml = text(&mut Package::open(&original).unwrap(), target);
        for (old, new, markup) in [
            ("z-index:1", "z-index:1;mso-wrap-style:tight", ";mso-wrap-style:tight"),
            ("rotation=\"t\"", "rotation=\"t\" aspectratio=\"t\"", " aspectratio=\"t\""),
            ("rotation=\"t\"", "rotation=\"t\" aspectratio=\"f\"", " aspectratio=\"f\""),
        ] {
            let source = vml.replacen(old, new, 1);
            assert!(source.contains(new));
            let (wb, _) = read_xlsx(&replace_part(&original, target, &source)).unwrap();
            assert_eq!(check_boxes(&wb), expected(), "x14={x14}: {new}");
            let out = write_xlsx(&wb).unwrap();
            assert_eq!(written_vml(&out).matches(markup).count(), 1);
            let (back, _) = read_xlsx(&out).unwrap();
            assert_eq!(check_boxes(&back), expected(), "x14={x14}: {new}");
        }
    }
}

#[test]
fn vml_inches_do_not_override_the_cell_anchor() {
    let original = legacy_check_boxes(false);
    let target = "xl/drawings/vmlDrawing1.vml";
    let vml = text(&mut Package::open(&original).unwrap(), target).replacen("margin-left:120pt", "margin-left:5in", 1);
    assert!(vml.contains("margin-left:5in"));
    let (wb, _) = read_xlsx(&replace_part(&original, target, &vml)).unwrap();
    let (ordinary, _) = read_xlsx(&original).unwrap();
    assert_eq!(check_boxes(&wb), expected());
    assert_eq!(wb.sheet(0).unwrap().controls[0].anchor, ordinary.sheet(0).unwrap().controls[0].anchor);
}

#[test]
fn inactive_shape_paint_does_not_make_a_checkbox_opaque() {
    let original = legacy_check_boxes(false);
    let target = "xl/drawings/vmlDrawing1.vml";
    let vml = text(&mut Package::open(&original).unwrap(), target);
    for (old, paint) in [("fillcolor=\"window [65]\"", "fillcolor=\"white [9]\""), ("<v:shape id=", "<v:shape strokeweight=\"3e-5mm\" id=")] {
        let vml = vml.replacen(old, paint, 1);
        assert!(vml.contains(paint));
        let (wb, _) = read_xlsx(&replace_part(&original, target, &vml)).unwrap();
        assert_eq!(check_boxes(&wb), expected(), "{paint}");
        let (back, _) = read_xlsx(&write_xlsx(&wb).unwrap()).unwrap();
        assert_eq!(check_boxes(&back), expected(), "{paint}");
    }
}

#[test]
fn worksheet_anchor_settings_outside_the_model_remain_in_the_sheet() {
    let base = legacy_check_boxes(true);
    let target = "xl/worksheets/sheet1.xml";
    let sheet = text(&mut Package::open(&base).unwrap(), target);
    let anchor = "<anchor moveWithCells=\"1\" extra=\"data\">";
    let (wb, _) = read_xlsx(&replace_part(&base, target, &sheet.replacen("<anchor moveWithCells=\"1\">", anchor, 1))).unwrap();
    assert_eq!(check_boxes(&wb), expected()[1..]);
    let out = write_xlsx(&wb).unwrap();
    assert!(text(&mut Package::open(&out).unwrap(), target).contains(anchor));
}

/// Unsupported shape and client-data settings survive even when x14 declares a check box.
#[test]
fn unmodelled_vml_metadata_stays_in_the_saved_drawing() {
    let base = legacy_check_boxes(true);
    let target = "xl/drawings/vmlDrawing1.vml";
    let original = text(&mut Package::open(&base).unwrap(), target);
    let mut lost = Vec::new();
    for (from, to) in [
        ("ObjectType=\"Checkbox\"", "ObjectType=\"Radio\""),
        ("z-index:1\"", "z-index:1;visibility:hidden\""),
        ("<x:Checked>1</x:Checked>", "<x:Checked>1</x:Checked><x:Checked>2</x:Checked>"),
        ("<x:FmlaLink>$B$2</x:FmlaLink>", "<x:FmlaLink extra=\"data\">$B$2</x:FmlaLink>"),
        ("3, 0, 1, 0, 5, 0, 2, 0", "3, 0, 1, invalid, 0, 5, 0, 2, 0"),
    ] {
        let (wb, _) = read_xlsx(&replace_part(&base, target, &original.replacen(from, to, 1))).unwrap();
        if check_boxes(&wb) != expected()[1..] || !written_vml(&write_xlsx(&wb).unwrap()).contains(to) {
            lost.push(to);
        }
    }
    assert!(lost.is_empty(), "rewritten VML: {lost:?}");
}

#[test]
fn check_boxes_beyond_the_model_are_kept_unchanged() {
    let base = String::from_utf8_lossy(&legacy_check_boxes(false)).into_owned();
    assert!(base.contains("<x:Checked>1</x:Checked>"));
    for (from, to) in [
        ("<x:Checked>1</x:Checked>", "<x:Checked>1</x:Checked><x:Disabled/>"),
        ("<x:Checked>1</x:Checked>", "<x:Checked>7</x:Checked>"),
        ("Ship it</font>", "Ship</font><font face=\"Arial\">it</font>"),
        ("Ship it</font>", "Ship<br/>it</font>"),
        ("<x:TextVAlign>Center</x:TextVAlign><x:FmlaLink>$B$2", "<x:TextVAlign>Top</x:TextVAlign><x:FmlaLink>$B$2"),
    ] {
        let vml = base.replacen(from, to, 1);
        // The zip stores parts uncompressed, so the text can be swapped in place only when its
        // length is unchanged; rebuild the package instead.
        let bytes = rebuild_with_vml(&vml);
        let (wb, _) = read_xlsx(&bytes).unwrap();
        let names: Vec<String> = wb.sheet(0).unwrap().controls.iter().map(FormControl::display_name).collect();
        assert_eq!(names, [CHECK_BOX_NAMES[1], CHECK_BOX_NAMES[2]], "{to}");
        let kept = kept_vml(&wb);
        assert!(kept.contains("GC Check 1") && kept.contains(to), "{to}: {kept}");
        let again = write_xlsx(&wb).unwrap();
        let out = written_vml(&again);
        assert!(out.contains(to) && out.matches("GC Check 1").count() == 1, "{to}: {out}");
    }
}

/// The fixture's package with its VML drawing replaced by the one `whole` (the fixture's bytes as
/// text, edited) holds.
fn rebuild_with_vml(whole: &str) -> Vec<u8> {
    let start = whole.find("<xml ").unwrap();
    let end = whole[start..].find("</xml>").unwrap() + start + "</xml>".len();
    replace_part(&legacy_check_boxes(false), "xl/drawings/vmlDrawing1.vml", &whole[start..end])
}

fn replace_part(original: &[u8], target: &str, body: &str) -> Vec<u8> {
    let mut pkg = Package::open(original).unwrap();
    let key = crate::package::normalize(target);
    let parts: Vec<_> = pkg
        .names()
        .into_iter()
        .filter(|n| n != &key)
        .map(|n| {
            let data = pkg.read(&n).unwrap().unwrap();
            (n, data)
        })
        .collect();
    let mut entries: Vec<(&str, &[u8])> = parts.iter().map(|(n, d)| (n.as_str(), d.as_slice())).collect();
    entries.push((target, body.as_bytes()));
    super::make_zip(&entries)
}

/// Hostile check box properties: unknown states, a link too long to be a cell, a huge caption.
/// Reading never fails, and what it models saves and reads back.
#[test]
fn hostile_check_box_properties_never_fail() {
    let base = String::from_utf8_lossy(&legacy_check_boxes(false)).into_owned();
    let long = format!("$B${}", "9".repeat(400));
    for (from, to) in [
        ("<x:Checked>1</x:Checked>", "<x:Checked></x:Checked>".to_string()),
        ("$B$2</x:FmlaLink>", format!("{long}</x:FmlaLink>")),
        ("Ship it", "x".repeat(5_000)),
        ("<x:NoThreeD/>", "<x:NoThreeD>maybe</x:NoThreeD>".to_string()),
    ] {
        let (wb, _) = read_xlsx(&rebuild_with_vml(&base.replacen(from, &to, 1))).unwrap();
        let n = wb.sheet(0).unwrap().controls.len();
        let (back, _) = read_xlsx(&write_xlsx(&wb).unwrap()).unwrap();
        assert_eq!(back.sheet(0).unwrap().controls.len(), n, "{to:.40}");
        assert!(back.sheet(0).unwrap().controls.iter().all(|c| c.link.as_ref().is_none_or(|l| l.len() <= 300)));
    }
}

/// Every `<control shapeId>` the sheet writes names exactly one shape in the written drawing, and
/// no two shapes share a number.
fn assert_shape_ids_resolve(bytes: &[u8]) -> Vec<String> {
    let mut pkg = Package::open(bytes).unwrap();
    let sheet = text(&mut pkg, "xl/worksheets/sheet1.xml");
    let vml = written_vml(bytes);
    let doc = crate::xml::parse(vml.as_bytes()).unwrap();
    let numbers: Vec<u64> = doc.kids("shape").filter_map(crate::controls::number_of).collect();
    let mut sorted = numbers.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), numbers.len(), "duplicate shape numbers {numbers:?}: {vml}");
    let ids: Vec<String> = sheet.split("<control shapeId=\"").skip(1).filter_map(|r| r.split('"').next()).map(str::to_string).collect();
    for id in &ids {
        let n: u64 = id.parse().unwrap();
        assert_eq!(numbers.iter().filter(|&&m| m == n).count(), 1, "control {id} has no shape: {vml}");
    }
    ids
}

/// GC Check 2 holds a setting the model doesn't (`x:Disabled`), so it stays in the kept drawing
/// beside the option button while GC Check 1 and 3 are written from the model into the same
/// drawing: every saved `<control>` still names its own shape, no two shapes share a number, and
/// three saves later all of it is the same.
#[test]
fn editable_and_kept_check_boxes_share_one_drawing_with_their_own_shape_ids() {
    let target = "xl/drawings/vmlDrawing1.vml";
    for x14 in [false, true] {
        let original = legacy_check_boxes(x14);
        let vml = text(&mut Package::open(&original).unwrap(), target).replacen(
            ">Second</div></v:textbox><x:ClientData ObjectType=\"Checkbox\">",
            ">Second</div></v:textbox><x:ClientData ObjectType=\"Checkbox\"><x:Disabled/>",
            1,
        );
        assert!(vml.contains("<x:Disabled/>"));
        let (wb, _) = read_xlsx(&replace_part(&original, target, &vml)).unwrap();
        let names: Vec<String> = wb.sheet(0).unwrap().controls.iter().map(FormControl::display_name).collect();
        assert_eq!(names, [CHECK_BOX_NAMES[0], CHECK_BOX_NAMES[2]], "x14={x14}");
        let mut out = write_xlsx(&wb).unwrap();
        for round in 0..3 {
            let ids = assert_shape_ids_resolve(&out);
            let written = written_vml(&out);
            assert_eq!(written.matches("<x:Disabled/>").count(), 1, "x14={x14} round {round}: {written}");
            assert_eq!(written.matches("ObjectType=\"Radio\"").count(), 1, "x14={x14} round {round}");
            assert_eq!(written.matches("ObjectType=\"Checkbox\"").count(), 3, "x14={x14} round {round}: {written}");
            let (back, _) = read_xlsx(&out).unwrap();
            let sh = back.sheet(0).unwrap();
            let got: Vec<(String, CheckState)> = sh.controls.iter().map(|c| (c.display_name(), c.checked)).collect();
            assert_eq!(
                got,
                [(CHECK_BOX_NAMES[0].to_string(), CheckState::Checked), (CHECK_BOX_NAMES[2].to_string(), CheckState::Mixed)],
                "x14={x14} round {round} ids {ids:?}"
            );
            out = write_xlsx(&back).unwrap();
        }
    }
}

/// The written VML shape of the check box captioned "Ship it", from `<v:shape ` on (not the
/// shape type before it).
fn ship_it_shape(bytes: &[u8]) -> String {
    let vml = written_vml(bytes);
    let shape = vml.split("</v:shape>").find(|s| s.contains("Ship it")).unwrap_or_default();
    shape.rfind("<v:shape ").map_or(shape, |i| &shape[i..]).to_string()
}

/// An editable check box is written back with the look its VML had: paint attributes, `v:fill`,
/// `o:lock` (with a text lock), `v:path`, and its text fitting elements, each present or left out
/// as read, and the properties part's `autoFill` follows the VML. A second save writes the same.
#[test]
fn an_editable_check_box_writes_back_the_look_it_was_read_with() {
    let target = "xl/drawings/vmlDrawing1.vml";
    let original = legacy_check_boxes(false);
    let vml = text(&mut Package::open(&original).unwrap(), target);
    let fill = "<v:fill color2=\"window [65]\" o:detectmouseclick=\"t\"/>";
    // (source text, its replacement, text the written shape has, text it doesn't)
    for (from, to, has, lacks) in [
        (fill, fill, fill, "<v:path"),
        ("x", "x", " filled=\"f\" fillcolor=\"window [65]\" stroked=\"f\" strokecolor=\"windowText [64]\"", "<x:LockText"),
        ("rotation=\"t\"/>", "rotation=\"t\" text=\"t\"/>", "<o:lock v:ext=\"edit\" rotation=\"t\" text=\"t\"/>", "aspectratio"),
        (fill, "", "<o:lock", "<v:fill"),
        ("rotation=\"t\"/>", "rotation=\"t\"/><v:path shadowok=\"t\" fillok=\"t\"/>", "<v:path shadowok=\"t\" fillok=\"t\"/>", "strokeok"),
        ("filled=\"f\" ", "", " fillcolor=\"window [65]\" stroked=\"f\"", "filled="),
        ("<x:TextVAlign>Center</x:TextVAlign>", "", "<x:AutoLine>False</x:AutoLine>", "TextVAlign"),
        ("<x:AutoLine>False</x:AutoLine>", "", "<x:TextVAlign>Center</x:TextVAlign>", "AutoLine"),
        ("<x:AutoFill>False</x:AutoFill>", "", "<x:TextVAlign>Center</x:TextVAlign>", "AutoFill"),
    ] {
        let source = vml.replacen(from, to, 1);
        let (wb, _) = read_xlsx(&replace_part(&original, target, &source)).unwrap();
        assert_eq!(check_boxes(&wb), expected(), "{to}");
        let out = write_xlsx(&wb).unwrap();
        let shape = ship_it_shape(&out);
        assert!(shape.contains(has) && !shape.contains(lacks), "{to}: {shape}");
        let mut pkg = Package::open(&out).unwrap();
        let sheet = text(&mut pkg, "xl/worksheets/sheet1.xml");
        let entry = sheet.split("</control>").find(|e| e.contains(CHECK_BOX_NAMES[0])).unwrap_or_default();
        assert_eq!(entry.contains("autoFill=\"0\""), from != "<x:AutoFill>False</x:AutoFill>", "{to}: {entry}");
        assert_eq!(entry.contains("autoLine=\"0\""), from != "<x:AutoLine>False</x:AutoLine>", "{to}: {entry}");
        let (back, _) = read_xlsx(&out).unwrap();
        assert_eq!(check_boxes(&back), expected(), "{to}");
        assert_eq!(ship_it_shape(&write_xlsx(&back).unwrap()), shape, "{to}");
    }
}

/// The two formats can disagree about automatic fill and line properties. Both keep their own
/// flags when the control is edited and saved.
#[test]
fn worksheet_and_vml_auto_flags_round_trip_independently() {
    let sheet_part = "xl/worksheets/sheet1.xml";
    let vml_part = "xl/drawings/vmlDrawing1.vml";
    let original = legacy_check_boxes(true);
    let sheet = text(&mut Package::open(&original).unwrap(), sheet_part);
    let vml = text(&mut Package::open(&original).unwrap(), vml_part);
    for (sheet_flag, vml_flag) in [("autoFill", "AutoLine"), ("autoLine", "AutoFill")] {
        let sheet_attr = format!(" {sheet_flag}=\"0\"");
        let vml_element = format!("<x:{vml_flag}>False</x:{vml_flag}>");
        let bytes = replace_part(&original, sheet_part, &sheet.replacen(&sheet_attr, "", 1));
        let bytes = replace_part(&bytes, vml_part, &vml.replacen(&vml_element, "", 1));
        let (mut wb, _) = read_xlsx(&bytes).unwrap();
        assert_eq!(check_boxes(&wb), expected(), "worksheet omits {sheet_flag}, VML omits {vml_flag}");
        wb.sheet_mut(0).unwrap().controls[0].checked = CheckState::Unchecked;
        for _ in 0..2 {
            let out = write_xlsx(&wb).unwrap();
            let saved_sheet = text(&mut Package::open(&out).unwrap(), sheet_part);
            let entry = saved_sheet.split("</control>").find(|e| e.contains(CHECK_BOX_NAMES[0])).unwrap();
            let shape = ship_it_shape(&out);
            for (sheet_name, vml_name) in [("autoFill", "AutoFill"), ("autoLine", "AutoLine")] {
                assert_eq!(entry.contains(&format!(" {sheet_name}=\"0\"")), sheet_name != sheet_flag, "{entry}");
                assert_eq!(shape.contains(&format!("<x:{vml_name}>False</x:{vml_name}>")), vml_name != vml_flag, "{shape}");
            }
            (wb, _) = read_xlsx(&out).unwrap();
            let controls = &wb.sheet(0).unwrap().controls;
            assert_eq!(controls.len(), expected().len());
            assert_eq!(controls[0].checked, CheckState::Unchecked);
        }
    }
}

/// Default sizing and automatic pictures are outside the editable model, and so is an automatic
/// fill or line flag set to anything but false: the check box stays kept, entry and all.
#[test]
fn a_control_entry_with_unsupported_sizing_picture_or_flag_stays_kept() {
    let sheet_part = "xl/worksheets/sheet1.xml";
    let original = legacy_check_boxes(true);
    let sheet = text(&mut Package::open(&original).unwrap(), sheet_part);
    for (from, to) in
        [("defaultSize=\"0\" ", ""), (" autoPict=\"0\"", ""), (" autoFill=\"0\"", " autoFill=\"1\""), (" autoLine=\"0\"", " autoLine=\"maybe\"")]
    {
        let bytes = replace_part(&original, sheet_part, &sheet.replacen(from, to, 1));
        let (wb, _) = read_xlsx(&bytes).unwrap();
        assert_eq!(check_boxes(&wb), expected()[1..], "{to:?}");
        let out = write_xlsx(&wb).unwrap();
        assert_eq!(written_vml(&out).matches("Ship it").count(), 1);
        if !to.is_empty() {
            assert!(text(&mut Package::open(&out).unwrap(), sheet_part).contains(to), "{to}");
        }
        let (back, _) = read_xlsx(&out).unwrap();
        assert_eq!(check_boxes(&back), expected()[1..], "{to:?}");
    }
}
