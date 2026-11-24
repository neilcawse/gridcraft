//! The theme part is written from the model's colours and fonts, but a theme holds more: a format
//! scheme whose fill styles can show a picture (a relationship and an image part of the theme's
//! own), object defaults and extensions. All of it is written back as the file had it, as long as
//! the theme is unchanged.

use super::*;

const IMAGE: &[u8] = b"\xFF\xD8\xFF\xE0 not really a jpeg";

/// A theme with the 12 colours and the fonts GridCraft models, a format scheme whose third
/// background fill style is the picture `rId1`, and an extension.
const THEME: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" name="Pictured"><a:themeElements><a:clrScheme name="Pictured"><a:dk1><a:sysClr val="windowText" lastClr="000000"/></a:dk1><a:lt1><a:sysClr val="window" lastClr="FFFFFF"/></a:lt1><a:dk2><a:srgbClr val="1F497D"/></a:dk2><a:lt2><a:srgbClr val="EEECE1"/></a:lt2><a:accent1><a:srgbClr val="4F81BD"/></a:accent1><a:accent2><a:srgbClr val="C0504D"/></a:accent2><a:accent3><a:srgbClr val="9BBB59"/></a:accent3><a:accent4><a:srgbClr val="8064A2"/></a:accent4><a:accent5><a:srgbClr val="4BACC6"/></a:accent5><a:accent6><a:srgbClr val="F79646"/></a:accent6><a:hlink><a:srgbClr val="0000FF"/></a:hlink><a:folHlink><a:srgbClr val="800080"/></a:folHlink></a:clrScheme><a:fontScheme name="Pictured"><a:majorFont><a:latin typeface="Cambria"/><a:ea typeface=""/><a:cs typeface=""/></a:majorFont><a:minorFont><a:latin typeface="Calibri"/><a:ea typeface=""/><a:cs typeface=""/></a:minorFont></a:fontScheme><a:fmtScheme name="Pictured"><a:fillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:gradFill rotWithShape="1"><a:gsLst><a:gs pos="0"><a:schemeClr val="phClr"/></a:gs><a:gs pos="100000"><a:schemeClr val="phClr"><a:shade val="80000"/></a:schemeClr></a:gs></a:gsLst><a:lin ang="5400000" scaled="0"/></a:gradFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:fillStyleLst><a:lnStyleLst><a:ln w="9525"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln><a:ln w="25400"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln><a:ln w="38100"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln></a:lnStyleLst><a:effectStyleLst><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle></a:effectStyleLst><a:bgFillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:blipFill><a:blip r:embed="rId1"/><a:stretch><a:fillRect/></a:stretch></a:blipFill></a:bgFillStyleLst></a:fmtScheme></a:themeElements><a:objectDefaults/><a:extraClrSchemeLst/><a:extLst><a:ext uri="{05A4C25C-085E-4340-85A3-A5531E510DB2}"><thm15:themeFamily xmlns:thm15="http://schemas.microsoft.com/office/thememl/2012/main" name="Pictured" id="{11111111-2222-3333-4444-555555555555}" vid="{66666666-7777-8888-9999-000000000000}"/></a:ext></a:extLst></a:theme>"#;

/// A workbook with one empty sheet and `THEME`. `theme_rels` are the theme part's relationships
/// (the picture is `../media/image1.jpeg`); `image` leaves the picture out when false.
fn package(theme_rels: &[(&str, &str, &str)], image: bool) -> Vec<u8> {
    let types = format!(
        r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="xml" ContentType="application/xml"/><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="jpeg" ContentType="image/jpeg"/><Override PartName="/xl/workbook.xml" ContentType="{CT_PREFIX}.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="{CT_PREFIX}.spreadsheetml.worksheet+xml"/><Override PartName="/xl/theme/theme1.xml" ContentType="{CT_PREFIX}.theme+xml"/></Types>"#
    );
    let workbook = format!(
        r#"<?xml version="1.0"?><workbook xmlns="{NS_MAIN}" xmlns:r="{NS_R}"><sheets><sheet name="Data" sheetId="1" r:id="rId1"/></sheets></workbook>"#
    );
    let sheet =
        format!(r#"<?xml version="1.0"?><worksheet xmlns="{NS_MAIN}"><sheetData><row r="1"><c r="A1"><v>1</v></c></row></sheetData></worksheet>"#);
    let root_rels = rels(&[("rId1", "officeDocument", "xl/workbook.xml")]);
    let wb_rels = rels(&[("rId1", "worksheet", "worksheets/sheet1.xml"), ("rId2", "theme", "theme/theme1.xml")]);
    let theme_rels = rels(theme_rels);
    let mut parts: Vec<(&str, &[u8])> = vec![
        ("[Content_Types].xml", types.as_bytes()),
        ("_rels/.rels", root_rels.as_bytes()),
        ("xl/workbook.xml", workbook.as_bytes()),
        ("xl/_rels/workbook.xml.rels", wb_rels.as_bytes()),
        ("xl/worksheets/sheet1.xml", sheet.as_bytes()),
        ("xl/theme/theme1.xml", THEME.as_bytes()),
        ("xl/theme/_rels/theme1.xml.rels", theme_rels.as_bytes()),
    ];
    if image {
        parts.push(("xl/media/image1.jpeg", IMAGE));
    }
    make_zip(&parts)
}

fn pictured() -> Vec<u8> {
    package(&[("rId1", "image", "../media/image1.jpeg")], true)
}

fn no_loss(warnings: &[String]) -> bool {
    !warnings.iter().any(|w| w.contains("skipped") || w.contains("dropped") || w.contains("theme"))
}

#[test]
fn a_theme_with_a_picture_fill_comes_back_whole() {
    let (wb, rep) = read_xlsx(&pictured()).unwrap();
    assert!(no_loss(&rep.warnings), "{:?}", rep.warnings);
    let out = write_xlsx(&wb).unwrap();
    let u = Unpacked::new(&out);
    u.assert_valid();

    // The part is byte for byte the file's, and its relationship still leads to the picture.
    assert_eq!(u.parts.get("xl/theme/theme1.xml").unwrap(), THEME.as_bytes());
    let rels = u.rels("xl/theme/theme1.xml");
    assert_eq!(rels.len(), 1, "{rels:?}");
    assert_eq!((rels[0].0.as_str(), rels[0].1.as_str()), ("rId1", format!("{NS_R}/image").as_str()));
    assert_eq!(u.parts.get(&rels[0].2).unwrap(), IMAGE);
    assert!(u.text("xl/_rels/workbook.xml.rels").contains("theme/theme1.xml"));

    // Saving what was saved changes nothing.
    let (wb2, rep2) = read_xlsx(&out).unwrap();
    assert!(no_loss(&rep2.warnings), "{:?}", rep2.warnings);
    let again = Unpacked::new(&write_xlsx(&wb2).unwrap());
    again.assert_valid();
    assert_eq!(again.parts.get("xl/theme/theme1.xml").unwrap(), THEME.as_bytes());
    let rels2 = again.rels("xl/theme/theme1.xml");
    assert_eq!(rels2.len(), 1, "{rels2:?}");
    assert_eq!(again.parts.get(&rels2[0].2).unwrap(), IMAGE);
}

#[test]
fn a_theme_without_relationships_keeps_its_format_scheme() {
    let (wb, rep) = read_xlsx(&package(&[], false)).unwrap();
    assert!(no_loss(&rep.warnings), "{:?}", rep.warnings);
    // What GridCraft models is read from the part as before.
    assert_eq!(wb.theme.name, "Pictured");
    assert_eq!(wb.theme.major_font, "Cambria");
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    u.assert_valid();
    assert_eq!(u.parts.get("xl/theme/theme1.xml").unwrap(), THEME.as_bytes());
    assert!(u.rels("xl/theme/theme1.xml").is_empty());
}

#[test]
fn a_changed_theme_is_written_from_the_model_without_the_old_pictures() {
    let (mut wb, _) = read_xlsx(&pictured()).unwrap();
    wb.theme.colors[4] = 0x123456;
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    u.assert_valid();
    let theme = u.text("xl/theme/theme1.xml");
    assert!(theme.contains(r#"<a:accent1><a:srgbClr val="123456"/></a:accent1>"#), "{theme}");
    assert!(!theme.contains("blipFill") && !theme.contains("thm15"), "{theme}");
    // No relationship to a picture nothing shows, and no orphan picture.
    assert!(u.rels("xl/theme/theme1.xml").is_empty());
    assert!(u.find(IMAGE).is_empty(), "{:?}", u.find(IMAGE));
}

#[test]
fn a_theme_picture_that_cannot_be_kept_leaves_a_rewritten_theme_and_a_warning() {
    // The relationship leads to a part the package doesn't have.
    let (wb, rep) = read_xlsx(&package(&[("rId1", "image", "../media/image1.jpeg")], false)).unwrap();
    assert!(rep.warnings.iter().any(|w| w.contains("theme1.xml") && w.contains("rewritten")), "{:?}", rep.warnings);
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    u.assert_valid();
    let theme = u.text("xl/theme/theme1.xml");
    assert!(!theme.contains("r:embed") && !theme.contains("blipFill"), "{theme}");
    assert!(u.rels("xl/theme/theme1.xml").is_empty());
    // What the model holds is still written.
    assert!(theme.contains(r#"<a:majorFont><a:latin typeface="Cambria"/>"#), "{theme}");
}
