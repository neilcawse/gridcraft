//! Legacy form drop-downs (`objectType="Drop"`): read from the properties part, the VML and the
//! sheet's `<controls>` without their DrawingML stand-in becoming a shape, saved with the parts
//! they need, and hostile properties repaired. The fixture follows the layout of the two public
//! Enron "Hanson Scheduling Model" workbooks: a nested `mc:AlternateContent` control, its
//! properties part, its VML shape and a hidden DrawingML stand-in that names the VML shape.

use gridcraft_core::{CellRef, Value};
use gridcraft_model::{AnchorMode, CONTROL_MAX, ControlKind, DropStyle, FormControl, Workbook};

use super::minimal;
use crate::package::Package;
use crate::{read_xlsx, write_xlsx};

const NS_MC: &str = "http://schemas.openxmlformats.org/markup-compatibility/2006";
const NS_XDR: &str = "http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing";

fn at(a: &str) -> CellRef {
    CellRef::parse(a).unwrap()
}

fn text(pkg: &mut Package<'_>, part: &str) -> String {
    String::from_utf8(pkg.read(part).unwrap().unwrap_or_else(|| panic!("missing {part}"))).unwrap()
}

/// The seven items AJ6:AJ12 list.
const ITEMS: [&str; 7] = ["North", "South", "East", "West", "Central", "Coast", "Valley"];

/// The properties part: `props` are its attributes after `objectType="Drop"`.
fn ctrl_prop(props: &str) -> String {
    format!(
        r#"<?xml version="1.0"?><formControlPr xmlns="http://schemas.microsoft.com/office/spreadsheetml/2009/9/main" objectType="Drop" {props}/>"#
    )
}

/// The DrawingML stand-in Excel writes for control `1026`, and optionally a shape of the user's.
fn drawing(stand_in_spid: &str, user_shape: bool) -> String {
    let anchor = |inner: &str| {
        format!(
            r#"<xdr:twoCellAnchor editAs="oneCell"><xdr:from><xdr:col>1</xdr:col><xdr:colOff>9525</xdr:colOff><xdr:row>7</xdr:row><xdr:rowOff>9525</xdr:rowOff></xdr:from><xdr:to><xdr:col>2</xdr:col><xdr:colOff>9525</xdr:colOff><xdr:row>8</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:to>{inner}<xdr:clientData/></xdr:twoCellAnchor>"#
        )
    };
    let stand_in = anchor(&format!(
        r#"<xdr:sp macro="" textlink=""><xdr:nvSpPr><xdr:cNvPr id="1026" name="Drop Down 2" hidden="1"><a:extLst><a:ext uri="{{63B3BB69-23CF-44E3-9099-C40C66FF867C}}"><a14:compatExt spid="{stand_in_spid}"/></a:ext></a:extLst></xdr:cNvPr><xdr:cNvSpPr/></xdr:nvSpPr><xdr:spPr bwMode="auto"><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:noFill/><a:ln><a:noFill/></a:ln></xdr:spPr></xdr:sp>"#
    ));
    let user = if user_shape {
        anchor(
            r#"<xdr:sp macro="" textlink=""><xdr:nvSpPr><xdr:cNvPr id="3" name="Note box"/><xdr:cNvSpPr/></xdr:nvSpPr><xdr:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></xdr:spPr><xdr:txBody><a:bodyPr/><a:p><a:r><a:t>Note</a:t></a:r></a:p></xdr:txBody></xdr:sp>"#,
        )
    } else {
        String::new()
    };
    format!(
        r#"<?xml version="1.0"?><xdr:wsDr xmlns:xdr="{NS_XDR}" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><mc:AlternateContent xmlns:mc="{NS_MC}"><mc:Choice xmlns:a14="http://schemas.microsoft.com/office/drawing/2010/main" Requires="a14">{stand_in}</mc:Choice></mc:AlternateContent>{user}</xdr:wsDr>"#
    )
}

/// The VML shape `_x0000_s1026` with client data of `object_type`.
fn vml(object_type: &str, sel: &str) -> String {
    format!(
        r##"<xml xmlns:v="urn:schemas-microsoft-com:vml" xmlns:o="urn:schemas-microsoft-com:office:office" xmlns:x="urn:schemas-microsoft-com:office:excel"><v:shape id="_x0000_s1026" type="#_x0000_t201" style="position:absolute;margin-left:8.25pt;margin-top:96.75pt;width:394.5pt;height:15.75pt;z-index:1" stroked="f" o:insetmode="auto"><x:ClientData ObjectType="{object_type}"><x:SizeWithCells/><x:Anchor>1, 1, 7, 1, 2, 1, 8, 0</x:Anchor><x:AutoLine>False</x:AutoLine><x:FmlaLink>$AJ$5</x:FmlaLink><x:Val>0</x:Val><x:Min>0</x:Min><x:Max>0</x:Max><x:Inc>1</x:Inc><x:Page>7</x:Page><x:Dx>22</x:Dx><x:FmlaRange>$AJ$6:$AJ$12</x:FmlaRange><x:Sel>{sel}</x:Sel><x:SelType>Single</x:SelType><x:LCT>Normal</x:LCT><x:DropStyle>Combo</x:DropStyle><x:DropLines>7</x:DropLines></x:ClientData></v:shape></xml>"##
    )
}

/// A Hanson-style sheet: the linked cell AJ5 holds `sel`, AJ6:AJ12 the items, and the control is
/// listed in a `<controls>` nested in two `mc:AlternateContent`s. `parts` replace the default
/// properties part, VML and drawing (`None` leaves that part out of the package).
fn hanson_with(sel: u32, prop: Option<&str>, vml: Option<&str>, drawing: Option<&str>) -> Vec<u8> {
    let mut rows = format!(r#"<row r="5"><c r="AJ5"><v>{sel}</v></c></row>"#);
    for (i, item) in ITEMS.iter().enumerate() {
        let r = i + 6;
        rows.push_str(&format!(r#"<row r="{r}"><c r="AJ{r}" t="inlineStr"><is><t>{item}</t></is></c></row>"#));
    }
    let controls = format!(
        r#"<mc:AlternateContent xmlns:mc="{NS_MC}"><mc:Choice Requires="x14"><controls xmlns:xdr="{NS_XDR}"><mc:AlternateContent xmlns:mc="{NS_MC}"><mc:Choice Requires="x14"><control shapeId="1026" r:id="rId8" name="Drop Down 2"><controlPr defaultSize="0" autoLine="0" autoPict="0"><anchor moveWithCells="1"><from><xdr:col>1</xdr:col><xdr:colOff>9525</xdr:colOff><xdr:row>7</xdr:row><xdr:rowOff>9525</xdr:rowOff></from><to><xdr:col>2</xdr:col><xdr:colOff>9525</xdr:colOff><xdr:row>8</xdr:row><xdr:rowOff>0</xdr:rowOff></to></anchor></controlPr></control></mc:Choice></mc:AlternateContent></controls></mc:Choice></mc:AlternateContent>"#
    );
    let body = format!(
        r#"<cols><col min="1" max="1" width="1.42578125" customWidth="1"/><col min="2" max="2" width="75.140625" customWidth="1"/></cols><sheetData>{rows}</sheetData><drawing r:id="rId6"/><legacyDrawing r:id="rId7"/>{controls}"#
    );
    let rels = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId8" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/ctrlProp" Target="../ctrlProps/ctrlProp1.xml"/><Relationship Id="rId7" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/vmlDrawing" Target="../drawings/vmlDrawing1.vml"/><Relationship Id="rId6" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/drawing" Target="../drawings/drawing1.xml"/></Relationships>"#;
    let mut parts = vec![("xl/worksheets/_rels/sheet1.xml.rels", rels)];
    for (name, part) in [("xl/ctrlProps/ctrlProp1.xml", prop), ("xl/drawings/vmlDrawing1.vml", vml), ("xl/drawings/drawing1.xml", drawing)] {
        if let Some(p) = part {
            parts.push((name, p));
        }
    }
    minimal(&body, &parts, "", "")
}

fn hanson(sel: u32) -> Vec<u8> {
    let prop = ctrl_prop(&format!(r#"dropLines="7" dropStyle="combo" dx="22" fmlaLink="$AJ$5" fmlaRange="$AJ$6:$AJ$12" sel="{sel}" val="0""#));
    hanson_with(sel, Some(&prop), Some(&vml("Drop", &sel.to_string())), Some(&drawing("_x0000_s1026", false)))
}

fn only_control(wb: &Workbook) -> FormControl {
    let sh = wb.sheet(0).unwrap();
    assert_eq!(sh.controls.len(), 1, "{:?}", sh.controls);
    sh.controls[0].clone()
}

/// Both public Hanson workbooks (`sel` 3 and 4): one drop-down at B8 with its link, list, item
/// and rows, and no shape for its hidden stand-in.
#[test]
fn hanson_drop_downs_read_as_one_control_and_no_shape() {
    for sel in [3, 4] {
        let (wb, report) = read_xlsx(&hanson(sel)).unwrap();
        assert!(report.warnings.iter().all(|w| !w.contains("control")), "{:?}", report.warnings);
        let sh = wb.sheet(0).unwrap();
        assert!(sh.shapes.is_empty(), "the stand-in is not a shape: {:?}", sh.shapes);
        let c = only_control(&wb);
        assert_eq!(c.kind, ControlKind::Drop);
        assert_eq!((c.link.as_deref(), c.list_range.as_deref()), (Some("$AJ$5"), Some("$AJ$6:$AJ$12")));
        assert_eq!((c.selected, c.drop_lines, c.drop_style), (sel, 7, DropStyle::Combo));
        assert_eq!((c.anchor.cell, c.anchor.mode, c.name.as_str()), (at("B8"), AnchorMode::MoveOnly, "Drop Down 2"));
        // It spans column B, offset a little into B8.
        let col_b = (sh.col_left(2) - sh.col_left(1)) as f32;
        assert!((c.anchor.width - col_b).abs() < 1.0 && c.anchor.dx > 0.0 && c.anchor.dy > 0.0, "{:?} vs {col_b}", c.anchor);
        assert!(c.anchor.height < sh.row_top(9) as f32 - sh.row_top(7) as f32);
        // Nothing of the control is kept for pass-through: GridCraft writes it.
        let kept = sh.preserved.as_ref().map(|k| k.elements.iter().any(|e| e.slot == "controls" || e.xml.contains("1026"))).unwrap_or(false);
        assert!(!kept, "{:?}", sh.preserved);
    }
}

/// Without `compatExt`, a hidden stand-in is still known by its `cNvPr id`; a shape of the user's
/// beside it stays a shape.
#[test]
fn a_stand_in_is_known_by_its_spid_or_hidden_id_and_other_shapes_stay() {
    let prop = ctrl_prop(r#"fmlaLink="$AJ$5" fmlaRange="$AJ$6:$AJ$12" sel="2""#);
    for d in [drawing("_x0000_s1026", true), drawing("", true).replace(r#"<a14:compatExt spid=""/>"#, "")] {
        let (wb, _) = read_xlsx(&hanson_with(2, Some(&prop), Some(&vml("Drop", "2")), Some(&d))).unwrap();
        let sh = wb.sheet(0).unwrap();
        assert_eq!(sh.controls.len(), 1);
        assert_eq!(sh.shapes.len(), 1, "only the user's shape: {:?}", sh.shapes);
        assert!(sh.shapes[0].text.contains("Note"), "{:?}", sh.shapes[0]);
    }
}

/// A control kind GridCraft doesn't model (a list box) stays with pass-through, and its stand-in
/// is read as it was before drop-downs were modelled.
#[test]
fn a_pass_through_controls_stand_in_is_read_as_before() {
    let prop = r#"<?xml version="1.0"?><formControlPr xmlns="http://schemas.microsoft.com/office/spreadsheetml/2009/9/main" objectType="List" fmlaRange="$AJ$6:$AJ$12" sel="2"/>"#;
    let (wb, _) = read_xlsx(&hanson_with(2, Some(prop), Some(&vml("List", "2")), Some(&drawing("_x0000_s1026", false)))).unwrap();
    let sh = wb.sheet(0).unwrap();
    assert!(sh.controls.is_empty());
    assert_eq!(sh.shapes.len(), 1, "the list box's stand-in is still read");
    let kept = sh.preserved.as_ref().and_then(|k| k.elements.iter().find(|e| e.slot == "controls")).map(|e| e.xml.clone()).unwrap_or_default();
    assert!(kept.contains("shapeId=\"1026\""), "the list box is kept: {kept}");
}

/// Saved without an edit: the properties part, the VML shape, the `<controls>` entry and their
/// relationships say what the file said, and the copy reads back the same and saves the same.
#[test]
fn an_unedited_drop_down_round_trips_with_its_parts() {
    let (wb, _) = read_xlsx(&hanson(3)).unwrap();
    let out = write_xlsx(&wb).unwrap();
    let mut pkg = Package::open(&out).unwrap();
    let sheet = text(&mut pkg, "xl/worksheets/sheet1.xml");
    let rels = text(&mut pkg, "xl/worksheets/_rels/sheet1.xml.rels");
    assert!(sheet.contains("<controls>") && sheet.contains("name=\"Drop Down 2\"") && sheet.contains("autoLine=\"0\""), "{sheet}");
    assert!(rels.contains("/relationships/ctrlProp\"") && rels.contains("/relationships/vmlDrawing\""), "{rels}");
    let prop = text(&mut pkg, "xl/ctrlProps/ctrlProp1.xml");
    for want in
        [r#"objectType="Drop""#, r#"fmlaLink="$AJ$5""#, r#"fmlaRange="$AJ$6:$AJ$12""#, r#"sel="3""#, r#"dropLines="7""#, r#"dropStyle="combo""#]
    {
        assert!(prop.contains(want), "{want}: {prop}");
    }
    let vml_out = text(&mut pkg, "xl/drawings/vmlDrawing1.vml");
    for want in ["ObjectType=\"Drop\"", "<x:FmlaRange>$AJ$6:$AJ$12</x:FmlaRange>", "<x:Sel>3</x:Sel>", "<x:FmlaLink>$AJ$5</x:FmlaLink>"] {
        assert!(vml_out.contains(want), "{want}: {vml_out}");
    }
    let shape_ids: Vec<&str> = sheet.split("<control shapeId=\"").skip(1).filter_map(|s| s.split('"').next()).collect();
    assert!(shape_ids.iter().all(|id| vml_out.contains(&format!("id=\"_x0000_s{id}\""))), "{shape_ids:?}");

    let (back, report) = read_xlsx(&out).unwrap();
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    let (a, b) = (only_control(&wb), only_control(&back));
    let same = |c: &FormControl| (c.kind, c.link.clone(), c.list_range.clone(), c.selected, c.drop_lines, c.drop_style, c.anchor.cell, c.anchor.mode);
    assert_eq!(same(&a), same(&b));
    assert!((a.anchor.width - b.anchor.width).abs() < 1.0 && (a.anchor.height - b.anchor.height).abs() < 1.0);
    assert!(back.sheet(0).unwrap().shapes.is_empty());
    assert_eq!(back.sheet(0).unwrap().value(at("AJ5")), Value::Number(3.0));
    // A second save writes the same control.
    let again = write_xlsx(&back).unwrap();
    assert_eq!(text(&mut Package::open(&again).unwrap(), "xl/ctrlProps/ctrlProp1.xml"), prop);
}

/// A changed choice is saved in `sel` and in the linked cell alike.
#[test]
fn a_changed_choice_saves_sel_and_the_linked_cell_together() {
    let (mut wb, _) = read_xlsx(&hanson(3)).unwrap();
    if let Some(sh) = wb.sheet_mut(0) {
        sh.controls[0].selected = 6;
        sh.set_value(at("AJ5"), Value::Number(6.0));
    }
    let out = write_xlsx(&wb).unwrap();
    let prop = text(&mut Package::open(&out).unwrap(), "xl/ctrlProps/ctrlProp1.xml");
    assert!(prop.contains(r#"sel="6""#) && !prop.contains(r#"sel="3""#), "{prop}");
    let (back, _) = read_xlsx(&out).unwrap();
    assert_eq!(only_control(&back).selected, 6);
    assert_eq!(back.sheet(0).unwrap().value(at("AJ5")), Value::Number(6.0));
}

/// Hostile or broken properties: ranges that aren't ranges, huge or negative numbers, unknown
/// styles, a missing properties part or VML, a part that isn't XML. Reading never fails and gives
/// controls the model can hold; saving and reading back works.
#[test]
fn hostile_drop_down_properties_are_repaired() {
    let long = format!("$A$1:$A${}", "9".repeat(400));
    let props = [
        r##"fmlaRange="#REF!" fmlaLink="not a cell" sel="99999999999999999999" dropLines="0" dropStyle="sideways""##.to_string(),
        r#"fmlaRange="$ZZZZZZ$0:$A$-4" sel="-7" dropLines="1e400" dropStyle="""#.to_string(),
        format!(r#"fmlaRange="{long}" sel="NaN" dropLines="-3""#),
        r#"fmlaRange="=Missing Sheet!$A$1:$A$9" sel="1e400""#.to_string(),
        r#"sel="3.7" dropLines="65536""#.to_string(),
    ];
    let mut cases: Vec<Vec<u8>> =
        props.iter().map(|p| hanson_with(1, Some(&ctrl_prop(p)), Some(&vml("Drop", "huge")), Some(&drawing("_x0000_s1026", false)))).collect();
    // No properties part: the VML says it all, even when its numbers are junk.
    cases.push(hanson_with(1, None, Some(&vml("Drop", "1e999")), Some(&drawing("_x0000_s1026", false))));
    // No VML either: the properties part alone, with the anchor of the `<controls>` entry.
    cases.push(hanson_with(1, Some(&ctrl_prop(r#"sel="2""#)), None, None));
    // A properties part that isn't XML, and nothing else.
    cases.push(hanson_with(1, Some("\u{0}<<not xml"), None, Some(&drawing("junk", false))));
    for (i, bytes) in cases.iter().enumerate() {
        let (wb, _) = read_xlsx(bytes).unwrap_or_else(|e| panic!("case {i}: {e}"));
        for c in &wb.sheet(0).unwrap().controls {
            assert_eq!(c.kind, ControlKind::Drop, "case {i}");
            assert!(c.selected <= CONTROL_MAX && (1..=CONTROL_MAX).contains(&c.drop_lines), "case {i}: {c:?}");
            assert!(c.list_range.as_ref().is_none_or(|r| r.chars().count() <= 300), "case {i}: {c:?}");
        }
        let (back, _) = read_xlsx(&write_xlsx(&wb).unwrap()).unwrap_or_else(|e| panic!("case {i} back: {e}"));
        assert_eq!(back.sheet(0).unwrap().controls.len(), wb.sheet(0).unwrap().controls.len(), "case {i}");
    }
}
