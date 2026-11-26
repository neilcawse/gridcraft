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
            let controls = wb.sheet(0).unwrap().preserved.as_ref().and_then(|p| p.elements.iter().find(|e| e.slot == "controls")).map(|e| e.xml.clone());
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
    assert!(props[0].contains("checked=\"Checked\"") && props[0].contains("fmlaLink=\"$B$2\"") && props[0].contains("noThreeD=\"1\""), "{}", props[0]);
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

/// Check boxes with something the model can't write back as it was (a setting it doesn't hold,
/// another look, a caption in several runs or lines) are left to pass-through, unchanged.
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
    let vml = &whole[start..end];
    let original = legacy_check_boxes(false);
    let mut pkg = Package::open(&original).unwrap();
    let names = ["[Content_Types].xml", "_rels/.rels", "xl/workbook.xml", "xl/_rels/workbook.xml.rels", "xl/worksheets/sheet1.xml", "xl/worksheets/_rels/sheet1.xml.rels"];
    let parts: Vec<(String, Vec<u8>)> = names.iter().map(|n| (n.to_string(), pkg.read(n).unwrap().unwrap())).collect();
    let mut entries: Vec<(&str, &[u8])> = parts.iter().map(|(n, d)| (n.as_str(), d.as_slice())).collect();
    entries.push(("xl/drawings/vmlDrawing1.vml", vml.as_bytes()));
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
