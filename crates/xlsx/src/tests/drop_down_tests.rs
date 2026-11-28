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
    drop_package(sel, prop, vml, drawing, &[])
}

fn drop_package(sel: u32, prop: Option<&str>, vml: Option<&str>, drawing: Option<&str>, extra: &[(&str, &str)]) -> Vec<u8> {
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
    parts.extend_from_slice(extra);
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

fn mixed_drop_drawing() -> String {
    drawing("_x0000_s1026", true)
        .replace("</mc:Choice></mc:AlternateContent>", "")
        .replace("</xdr:wsDr>", "</mc:Choice></mc:AlternateContent></xdr:wsDr>")
}

#[test]
fn mixed_alternate_drawing_warns_and_keeps_artwork_and_legacy_control() {
    let prop = ctrl_prop("widthMin=\"35\"");
    let input = hanson_with(2, Some(&prop), Some(&vml("Drop", "2")), Some(&mixed_drop_drawing()));
    let (wb, report) = read_xlsx(&input).unwrap();
    assert!(report.warnings.iter().any(|w| w.contains("mixed alternate drawing")), "{:?}", report.warnings);
    let sheet = wb.sheet(0).unwrap();
    assert!(sheet.controls.is_empty());
    assert_eq!(sheet.shapes.len(), 1);
    assert_eq!(sheet.shapes[0].text, "Note");

    let bytes = write_xlsx(&wb).unwrap();
    let mut pkg = Package::open(&bytes).unwrap();
    let sh = "xl/worksheets/sheet1.xml";
    let rels = pkg.rels(sh).unwrap();
    let part = |kind| rels.iter().find(|r| r.kind == kind).unwrap().target.as_str();
    let draw = text(&mut pkg, part("drawing"));
    assert_eq!(draw.matches("<a:t>Note</a:t>").count(), 1);
    assert!(!draw.contains("compatExt"));
    let sh = text(&mut pkg, sh);
    assert_eq!(sh.matches("shapeId=\"1026\"").count(), 1);
    assert!(sh.contains(&format!("r:id=\"{}\"", rels.iter().find(|r| r.kind == "ctrlProp").unwrap().id)));
    let vml = text(&mut pkg, part("vmlDrawing"));
    assert_eq!(vml.matches("id=\"_x0000_s1026\"").count(), 1);
    assert!(text(&mut pkg, part("ctrlProp")).contains("widthMin=\"35\""));

    let (reopened, report) = read_xlsx(&bytes).unwrap();
    assert!(report.warnings.iter().all(|w| w.contains("form controls are not shown")), "{:?}", report.warnings);
    let sheet = reopened.sheet(0).unwrap();
    assert!(sheet.controls.is_empty());
    assert_eq!(sheet.shapes.len(), 1);
    assert_eq!(sheet.shapes[0].text, "Note");
}

/// A control kind GridCraft doesn't model stays with pass-through, including its hidden anchor.
#[test]
fn a_pass_through_controls_stand_in_keeps_its_original_id_and_extension() {
    let prop = r#"<?xml version="1.0"?><formControlPr xmlns="http://schemas.microsoft.com/office/spreadsheetml/2009/9/main" objectType="List" fmlaRange="$AJ$6:$AJ$12" sel="2"/>"#;
    let (wb, _) = read_xlsx(&hanson_with(2, Some(prop), Some(&vml("List", "2")), Some(&drawing("_x0000_s1026", false)))).unwrap();
    let sh = wb.sheet(0).unwrap();
    assert!(sh.controls.is_empty());
    let saved = write_xlsx(&wb).unwrap();
    let mut pkg = Package::open(&saved).unwrap();
    let draw = text(&mut pkg, "xl/drawings/drawing1.xml");
    assert!(draw.contains("id=\"1026\"") && draw.contains("spid=\"_x0000_s1026\""), "{draw}");
    assert!(sh.shapes.is_empty());
    let kept = sh.preserved.as_ref().and_then(|k| k.elements.iter().find(|e| e.slot == "controls")).map(|e| e.xml.clone()).unwrap_or_default();
    assert!(kept.contains("shapeId=\"1026\""), "the list box is kept: {kept}");
}

/// A drop-down whose stand-in is a direct (not `mc:AlternateContent`) anchor with a hyperlink
/// relationship of the drawing's own.
fn direct_proxy_package() -> Vec<u8> {
    let wrapped = drawing("_x0000_s1026", false);
    let (head, wrapper) = wrapped.split_once("<mc:AlternateContent").unwrap();
    let (_, anchor) = wrapper.split_once("<xdr:twoCellAnchor").unwrap();
    let anchor = anchor
        .replace("</mc:Choice></mc:AlternateContent>", "")
        .replace("<xdr:cNvPr id=", "<xdr:cNvPr xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" id=")
        .replace("<a:extLst>", "<a:hlinkClick r:id=\"rId8\"/><a:extLst>");
    let d = format!("{head}<!-- direct anchor --><xdr:twoCellAnchor xmlns:a14=\"http://schemas.microsoft.com/office/drawing/2010/main\"{anchor}");
    let rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId8" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://example.invalid/" TargetMode="External"/></Relationships>"#;
    let v = vml("Drop", "2");
    let prop = ctrl_prop("widthMin=\"35\"");
    drop_package(2, Some(&prop), Some(&v), Some(&d), &[("xl/drawings/_rels/drawing1.xml.rels", rels)])
}

#[test]
fn direct_opaque_proxy_keeps_drawing_relationships_separate_from_sheet_relationships() {
    let (wb, _) = read_xlsx(&direct_proxy_package()).unwrap();
    let saved = write_xlsx(&wb).unwrap();
    let mut pkg = Package::open(&saved).unwrap();
    let part = "xl/drawings/drawing1.xml";
    let rels = pkg.rels(part).unwrap();
    let rel = rels.iter().find(|r| r.kind == "hyperlink").unwrap();
    assert!(rel.external);
    assert_eq!(rel.target, "https://example.invalid/");
    let draw = text(&mut pkg, part);
    assert!(!draw.contains("AlternateContent"));
    assert!(draw.contains(&format!("<a:hlinkClick r:id=\"{}\"/>", rel.id)), "{draw}");
}

/// A picture with a kept SVG companion beside a control stand-in, whose relationships are written
/// first: the SVG goes under the picture's own raster blip, by the id that blip was written with.
#[test]
fn an_svg_companion_goes_under_its_own_picture_beside_a_control_stand_in() {
    use gridcraft_model::{Anchor, Image, PreservedPart, PreservedXml};
    const SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"/>"#;
    let (mut wb, _) = read_xlsx(&direct_proxy_package()).unwrap();
    let sh = wb.sheet_mut(0).unwrap();
    let png = b"\x89PNG\r\n\x1a\nfake-png".to_vec();
    sh.images.push(Image {
        id: 900,
        anchor: Anchor { width: 40.0, height: 40.0, ..Default::default() },
        data: std::sync::Arc::new(png),
        mime: "image/png".into(),
        alt: String::new(),
    });
    let p = std::sync::Arc::make_mut(sh.preserved.as_mut().expect("the stand-in is kept"));
    assert!(p.control_proxies.is_some(), "the fixture has a control stand-in");
    p.elements.push(PreservedXml { slot: crate::passthrough::SLOT_DRAWING_SVG_BLIP.into(), xml: "900\txl/media/image9.svg".into() });
    p.parts.push(PreservedPart { name: "xl/media/image9.svg".into(), content_type: "image/svg+xml".into(), data: SVG.into(), ..Default::default() });
    let saved = write_xlsx(&wb).unwrap();
    let mut pkg = Package::open(&saved).unwrap();
    let part = "xl/drawings/drawing1.xml";
    let rels = pkg.rels(part).unwrap();
    let num = |id: &str| id.trim_start_matches("rId").parse::<u32>().unwrap();
    let link = rels.iter().find(|r| r.kind == "hyperlink").unwrap();
    let raster = rels.iter().find(|r| r.kind == "image" && r.target.ends_with(".png")).unwrap();
    let svg = rels.iter().find(|r| r.kind == "image" && r.target.ends_with(".svg")).unwrap();
    assert!(num(&link.id) < num(&raster.id), "the stand-in's relationship comes first: {rels:?}");
    assert_eq!(pkg.read(&svg.target).unwrap().as_deref(), Some(SVG));
    let draw = text(&mut pkg, part);
    let blip = format!(
        "<a:blip r:embed=\"{}\"><a:extLst><a:ext uri=\"{{96DAC541-7B7A-43D3-8B79-37D633B846F1}}\"><asvg:svgBlip xmlns:asvg=\"http://schemas.microsoft.com/office/drawing/2016/SVG/main\" r:embed=\"{}\"/></a:ext></a:extLst></a:blip>",
        raster.id, svg.id
    );
    assert!(draw.contains(&blip), "{draw}");
    assert_eq!(draw.matches("svgBlip").count(), 1, "{draw}");
}

/// A drop-down with its own item list (`itemLst`, VML `ListItem`) or a macro holds more than the
/// model does: it stays with pass-through, including its stand-in.
#[test]
fn a_drop_down_with_its_own_items_or_a_macro_is_kept_unchanged() {
    let items = r#"<?xml version="1.0"?><formControlPr xmlns="http://schemas.microsoft.com/office/spreadsheetml/2009/9/main" objectType="Drop" fmlaLink="$AJ$5" sel="2"><itemLst><item val="One"/><item val="Two"/></itemLst></formControlPr>"#.to_string();
    let vml_items = vml("Drop", "2").replace("<x:DropStyle>", "<x:ListItem>One</x:ListItem><x:ListItem>Two</x:ListItem><x:DropStyle>");
    let vml_macro = vml("Drop", "2").replace("<x:DropStyle>", "<x:FmlaMacro>[0]!Changed</x:FmlaMacro><x:DropStyle>");
    let range = ctrl_prop(r#"fmlaLink="$AJ$5" fmlaRange="$AJ$6:$AJ$12" sel="2""#);
    let bare = ctrl_prop(r#"fmlaLink="$AJ$5" sel="2""#);
    // The properties part has the item list, or only the VML has the items or the macro.
    let cases = [(items.as_str(), vml("Drop", "2")), (bare.as_str(), vml_items), (range.as_str(), vml_macro)];
    for (i, (prop, v)) in cases.iter().enumerate() {
        let (wb, _) = read_xlsx(&hanson_with(2, Some(prop), Some(v), Some(&drawing("_x0000_s1026", false)))).unwrap();
        let sh = wb.sheet(0).unwrap();
        assert!(sh.controls.is_empty(), "case {i}: {:?}", sh.controls);
        assert!(sh.shapes.is_empty(), "case {i}: the stand-in is not an editable shape");
        let kept = sh.preserved.as_ref().and_then(|k| k.elements.iter().find(|e| e.slot == "controls")).map(|e| e.xml.clone()).unwrap_or_default();
        assert!(kept.contains("shapeId=\"1026\""), "case {i}: {kept}");
        // Saved and read back, it is still kept, not modelled.
        let (back, _) = read_xlsx(&write_xlsx(&wb).unwrap()).unwrap();
        let bs = back.sheet(0).unwrap();
        assert!(bs.controls.is_empty(), "case {i}: {:?}", bs.controls);
        assert!(bs.shapes.is_empty(), "case {i}: the stand-in is still not editable");
        let kept = bs.preserved.as_ref().and_then(|k| k.elements.iter().find(|e| e.slot == "controls")).map(|e| e.xml.clone()).unwrap_or_default();
        assert!(kept.contains("shapeId=\"1026\""), "case {i} back: {kept}");
    }
}

#[test]
fn drop_display_settings_keep_values_and_omissions_in_each_part() {
    let base = vml("Drop", "2").replace("<x:Dx>22</x:Dx>", "").replace("<x:Val>0</x:Val>", "");
    let vml_settings = "<x:NoThreeD>False</x:NoThreeD><x:NoThreeD2/><x:Dx>47</x:Dx><x:Val>3</x:Val>";
    let props = ["noThreeD=\"1\"", "noThreeD2=\"0\"", "dx=\"35\"", "val=\"2\""];
    for (xml, vml) in [(false, false), (true, false), (false, true), (true, true)] {
        let input_props = if xml { props.join(" ") } else { String::new() };
        let input_vml = if vml { base.replace("<x:Min>", &format!("{vml_settings}<x:Min>")) } else { base.clone() };
        let mut bytes = hanson_with(2, Some(&ctrl_prop(&input_props)), Some(&input_vml), None);
        for _ in 0..2 {
            let (mut wb, _) = read_xlsx(&bytes).unwrap();
            let sh = wb.sheet_mut(0).unwrap();
            assert_eq!(sh.controls.len(), 1);
            sh.controls[0].name = "Renamed drop".into();
            bytes = write_xlsx(&wb).unwrap();
            let mut pkg = Package::open(&bytes).unwrap();
            let rels = pkg.rels("xl/worksheets/sheet1.xml").unwrap();
            let part = |kind| rels.iter().find(|r| r.kind == kind).unwrap().target.as_str();
            let p = text(&mut pkg, part("ctrlProp"));
            for (attr, value) in props.iter().map(|setting| setting.split_once('=').unwrap()) {
                assert_eq!(p.contains(&format!(" {attr}=")), xml, "{p}");
                if xml {
                    assert!(p.contains(&format!(" {attr}={value}")), "{p}");
                }
            }
            let d = text(&mut pkg, part("vmlDrawing"));
            for (tag, element) in [
                ("NoThreeD", "<x:NoThreeD>False</x:NoThreeD>"),
                ("NoThreeD2", "<x:NoThreeD2/>"),
                ("Dx", "<x:Dx>47</x:Dx>"),
                ("Val", "<x:Val>3</x:Val>"),
            ] {
                assert_eq!(d.contains(&format!("<x:{tag}>")) || d.contains(&format!("<x:{tag}/>")), vml, "{d}");
                if vml {
                    assert!(d.contains(element), "{d}");
                }
            }
        }
    }
}

#[test]
fn unsupported_drop_metadata_and_its_proxy_survive_repeated_saves() {
    let plain = ctrl_prop(r#"sel="2""#);
    let vml_base = vml("Drop", "2");
    let vml_flag = vml_base.replace("<x:DropStyle>", "<x:RecalcAlways/><x:DropStyle>");
    let vml_color = vml_base.replace("stroked=\"f\"", "fillcolor=\"red\"");
    // The original VML id is 1026 but its DrawingML id is 2049, the first free new VML id.
    // Retaining that drawing id must not duplicate it when a new modeled control is added.
    let proxy = drawing("_x0000_s1026", true).replace("id=\"1026\" name=", "id=\"2049\" name=");
    for (prop, vml, marker) in [
        (ctrl_prop(r#"fmlaMacro="[0]!Custom""#), &vml_base, "fmlaMacro"),
        (ctrl_prop(r#"widthMin="35""#), &vml_base, "widthMin=\"35\""),
        (ctrl_prop(r#"dropStyle="OtherStyle""#), &vml_base, "dropStyle=\"OtherStyle\""),
        (plain.clone(), &vml_base.replace(">Combo</x:DropStyle>", ">OtherStyle</x:DropStyle>"), ">OtherStyle</x:DropStyle>"),
        (plain.clone(), &vml_flag, "RecalcAlways"),
        (plain.clone(), &vml_color, "fillcolor=\"red\""),
        (plain.clone(), &vml_base.replace("<x:SelType>Single", "<x:SelType>Multi"), "<x:SelType>Multi"),
    ] {
        let mut bytes = hanson_with(2, Some(&prop), Some(vml), Some(&proxy));
        for save in 0..2 {
            let (mut wb, _) = read_xlsx(&bytes).unwrap();
            let sheet = wb.sheet_mut(0).unwrap();
            assert!(sheet.controls.iter().all(|c| c.name != "Drop Down 2"), "lost {marker}");
            assert_eq!(sheet.shapes.len(), 1, "only the actual drawing shape should be editable");
            assert_eq!(sheet.shapes[0].text, "Note");
            // A new control must share the drawing without claiming the kept control's id.
            if save == 0 {
                assert!(sheet.controls.is_empty());
                sheet.controls.push(FormControl { id: 3000, kind: ControlKind::Drop, name: "Modeled drop".into(), ..Default::default() });
            } else {
                assert_eq!(sheet.controls.len(), 1);
                assert_eq!(sheet.controls[0].name, "Modeled drop");
            }
            bytes = write_xlsx(&wb).unwrap();
            let mut pkg = Package::open(&bytes).unwrap();
            let draw = text(&mut pkg, "xl/drawings/drawing1.xml");
            assert_eq!(draw.matches("id=\"2049\"").count(), 1, "{draw}");
            assert!(draw.contains("id=\"2049\" name=\"Drop Down 2\""));
            assert!(draw.contains("spid=\"_x0000_s1026\"") && draw.contains("<a:t>Note</a:t>"), "{draw}");
            let rels = pkg.rels("xl/worksheets/sheet1.xml").unwrap();
            assert_eq!(rels.iter().filter(|r| r.kind == "drawing").count(), 1);
            let worksheet = text(&mut pkg, "xl/worksheets/sheet1.xml");
            assert_eq!(worksheet.matches("shapeId=\"1026\"").count(), 1, "{worksheet}");
            let vml = text(&mut pkg, &rels.iter().find(|r| r.kind == "vmlDrawing").unwrap().target);
            assert_eq!(vml.matches("id=\"_x0000_s1026\"").count(), 1, "{vml}");
            assert!(rels.iter().filter(|r| matches!(r.kind.as_str(), "ctrlProp" | "vmlDrawing")).any(|r| text(&mut pkg, &r.target).contains(marker)));
        }
    }
}

#[test]
fn missing_ctrl_prop_fields_fall_back_individually_to_vml() {
    let prop = ctrl_prop("");
    let vml = vml("Drop", "4");
    let (wb, _) = read_xlsx(&hanson_with(4, Some(&prop), Some(&vml), Some(&drawing("_x0000_s1026", false)))).unwrap();
    let c = only_control(&wb);
    assert_eq!((c.link.as_deref(), c.list_range.as_deref(), c.selected, c.drop_lines), (Some("$AJ$5"), Some("$AJ$6:$AJ$12"), 4, 7));
}

#[test]
fn hidden_unrelated_drawing_is_not_a_control_proxy_by_id_alone() {
    let d = drawing("_x0000_s1026", true)
        .replace("id=\"3\" name=\"Note box\"", "id=\"1026\" hidden=\"1\" name=\"Note box\"")
        .replace("id=\"1026\" name=\"Drop Down 2\"", "id=\"8\" name=\"Drop Down 2\"");
    let prop = ctrl_prop("sel=\"2\"");
    let (wb, _) = read_xlsx(&hanson_with(2, Some(&prop), Some(&vml("Drop", "2")), Some(&d))).unwrap();
    let sh = wb.sheet(0).unwrap();
    assert_eq!(sh.controls.len(), 1);
    assert_eq!(sh.shapes.len(), 1);
    assert!(sh.shapes[0].text.contains("Note"));
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
    // The drawing keeps the control's hidden stand-in, naming the same VML shape.
    assert!(rels.contains("/relationships/drawing\"") && sheet.contains("<drawing r:id="), "{rels}");
    let drawing_out = text(&mut pkg, "xl/drawings/drawing1.xml");
    assert!(!shape_ids.is_empty());
    for id in &shape_ids {
        let want = format!(r#"<xdr:cNvPr id="{id}" name="Drop Down 2" hidden="1">"#);
        assert!(drawing_out.contains(&want) && drawing_out.contains(&format!(r#"spid="_x0000_s{id}""#)), "{want}: {drawing_out}");
    }

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
