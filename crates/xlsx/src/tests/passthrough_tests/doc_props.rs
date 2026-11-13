//! The core and extended properties parts are written from the few document properties GridCraft
//! models. What else they hold (`cp:revision`, `cp:category`, `dc:language`, the manager, the
//! hyperlink base…) is kept as XML and written back.

use super::*;

const CORE_TYPE: &str = "http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties";
const CORE_NS: &str = r#"xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/" xmlns:dcmitype="http://purl.org/dc/dcmitype/" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance""#;
const APP_NS: &str = r#"xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties" xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes""#;

fn core(body: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><cp:coreProperties {CORE_NS} xmlns:x="urn:example:more">{body}</cp:coreProperties>"#
    )
}

fn app(body: &str) -> String {
    format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Properties {APP_NS}>{body}</Properties>"#)
}

/// `rich_package` with document properties parts.
fn package_with(core: &str, app: &str) -> Vec<u8> {
    let u = Unpacked::new(&rich_package());
    let root_rels = rels(&[
        ("rId1", "officeDocument", "xl/workbook.xml"),
        ("rId2", "custom-properties", "docProps/custom.xml"),
        ("rId3", CORE_TYPE, "docProps/core.xml"),
        ("rId4", "extended-properties", "docProps/app.xml"),
    ]);
    let mut parts: Vec<(&str, &[u8])> =
        u.parts.iter().map(|(k, v)| (k.as_str(), if k == "_rels/.rels" { root_rels.as_bytes() } else { v.as_slice() })).collect();
    parts.push(("docProps/core.xml", core.as_bytes()));
    parts.push(("docProps/app.xml", app.as_bytes()));
    make_zip(&parts)
}

const ELEMENTS: &str = concat!(
    "<dc:title>Budget</dc:title>",
    "<cp:revision>7</cp:revision>",
    "<cp:category>Plans</cp:category>",
    "<dc:language/>",
    r#"<cp:lastPrinted xsi:type="dcterms:W3CDTF">2020-01-02T03:04:05Z</cp:lastPrinted>"#,
    r#"<x:other a="1">kept</x:other>"#,
);

const APP_ELEMENTS: &str = concat!(
    "<Application>Microsoft Excel</Application>",
    "<DocSecurity>1</DocSecurity>",
    "<Company>Acme</Company>",
    "<Manager>Ann</Manager>",
    "<HyperlinkBase>https://example.com/base/</HyperlinkBase>",
    r#"<TitlesOfParts><vt:vector size="1" baseType="lpstr"><vt:lpstr>Data</vt:lpstr></vt:vector></TitlesOfParts>"#,
    "<AppVersion>16.0300</AppVersion>",
);

fn assert_extras_kept(u: &Unpacked) {
    let core = u.text("docProps/core.xml");
    xml::parse(core.as_bytes()).unwrap_or_else(|e| panic!("{e}: {core}"));
    assert_eq!(core.matches("<dc:title>Budget</dc:title>").count(), 1, "{core}");
    for kept in [
        "<cp:revision>7</cp:revision>",
        "<cp:category>Plans</cp:category>",
        r#"<cp:lastPrinted xsi:type="dcterms:W3CDTF">2020-01-02T03:04:05Z</cp:lastPrinted>"#,
        r#"xmlns:x="urn:example:more" a="1">kept</x:other>"#,
    ] {
        assert!(core.contains(kept), "{kept}: {core}");
    }
    assert!(core.contains("<dc:language") && !core.contains("<dc:language>"), "an empty element stays empty: {core}");
    // The prefixes the part declares are not declared again on the elements.
    assert_eq!(core.matches("xmlns:cp=").count(), 1, "{core}");

    let app = u.text("docProps/app.xml");
    xml::parse(app.as_bytes()).unwrap_or_else(|e| panic!("{e}: {app}"));
    for kept in ["<Manager>Ann</Manager>", "<HyperlinkBase>https://example.com/base/</HyperlinkBase>", "<Company>Acme</Company>"] {
        assert!(app.contains(kept), "{kept}: {app}");
    }
    // GridCraft writes the application; what describes the file as it was saved is left out.
    assert_eq!(app.matches("<Application>").count(), 1, "{app}");
    assert!(app.contains("<Application>GridCraft</Application>"), "{app}");
    for dropped in ["Microsoft Excel", "DocSecurity", "TitlesOfParts", "AppVersion"] {
        assert!(!app.contains(dropped), "{dropped}: {app}");
    }
}

#[test]
fn unmodelled_document_properties_survive_a_round_trip() {
    let (wb, rep) = read_xlsx(&package_with(&core(ELEMENTS), &app(APP_ELEMENTS))).unwrap();
    assert!(!rep.warnings.iter().any(|w| w.contains("dropped")), "{:?}", rep.warnings);
    assert_eq!((wb.props.title.as_str(), wb.props.company.as_str()), ("Budget", "Acme"));
    let out = write_xlsx(&wb).unwrap();
    let u = Unpacked::new(&out);
    u.assert_valid();
    assert_extras_kept(&u);
    // Read again, and written again: nothing is added or lost.
    let (wb2, rep2) = read_xlsx(&out).unwrap();
    assert!(!rep2.warnings.iter().any(|w| w.contains("dropped")), "{:?}", rep2.warnings);
    let again = Unpacked::new(&write_xlsx(&wb2).unwrap());
    assert_extras_kept(&again);
    assert_eq!(again.text("docProps/core.xml"), u.text("docProps/core.xml"));
    assert_eq!(again.text("docProps/app.xml"), u.text("docProps/app.xml"));
}

#[test]
fn a_part_holding_only_unmodelled_elements_is_not_emptied() {
    // `<cp:revision>` alone, as many files have it.
    let (wb, _) = read_xlsx(&package_with(&core("<cp:revision>0</cp:revision>"), &app(""))).unwrap();
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    assert!(u.text("docProps/core.xml").contains("<cp:revision>0</cp:revision>"), "{}", u.text("docProps/core.xml"));
}

#[test]
fn editing_the_modelled_properties_leaves_the_others() {
    let (mut wb, _) = read_xlsx(&package_with(&core(ELEMENTS), &app(APP_ELEMENTS))).unwrap();
    wb.props.title = "Renamed".into();
    wb.props.company = String::new();
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    let core = u.text("docProps/core.xml");
    assert!(core.contains("<dc:title>Renamed</dc:title>") && !core.contains("Budget"), "{core}");
    assert!(core.contains("<cp:revision>7</cp:revision>"), "{core}");
    let app = u.text("docProps/app.xml");
    assert!(!app.contains("Company") && app.contains("<Manager>Ann</Manager>"), "{app}");
}

#[test]
fn a_package_without_the_parts_writes_none_of_the_extras() {
    let (wb, _) = read_xlsx(&rich_package()).unwrap();
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    assert!(!u.text("docProps/core.xml").contains("cp:revision"));
    assert_eq!(wb.preserved.as_deref().unwrap().elements.iter().filter(|e| e.slot.ends_with("-properties")).count(), 0);
}

#[test]
fn a_prefix_the_source_declares_itself_travels_with_the_element() {
    // The part declares its Dublin Core prefixes under other names than the writer's.
    let source = r#"<?xml version="1.0"?><cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:d="http://purl.org/dc/elements/1.1/" xmlns:t="http://purl.org/dc/terms/" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"><d:language>fr</d:language><t:valid xsi:type="t:W3CDTF">2021-05-06T00:00:00Z</t:valid></cp:coreProperties>"#;
    let (wb, _) = read_xlsx(&package_with(source, &app(""))).unwrap();
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    let core = u.text("docProps/core.xml");
    let parsed = xml::parse(core.as_bytes()).unwrap_or_else(|e| panic!("{e}: {core}"));
    assert_eq!(parsed.child("language").map(|e| e.text.as_str()), Some("fr"), "{core}");
    assert!(core.contains(r#"xmlns:d="http://purl.org/dc/elements/1.1/""#) && core.contains(r#"xmlns:t="http://purl.org/dc/terms/""#), "{core}");
}

#[test]
fn damaged_document_properties_are_dropped_with_a_warning() {
    let (wb, rep) = read_xlsx(&package_with("<cp:coreProperties><a></b></cp:coreProperties>", &app(APP_ELEMENTS))).unwrap();
    assert!(rep.warnings.iter().any(|w| w.contains("core-properties") && w.contains("dropped")), "{:?}", rep.warnings);
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    u.assert_valid();
    assert!(u.text("docProps/app.xml").contains("<Manager>Ann</Manager>"));
}

#[test]
fn a_strict_package_keeps_them_too() {
    let strict = super::strict_kept::strict_copy(&package_with(&core(ELEMENTS), &app(APP_ELEMENTS)));
    let u = Unpacked::new(&strict);
    assert!(u.text("docProps/app.xml").contains("purl.oclc.org/ooxml/officeDocument/extendedProperties"));
    let (wb, rep) = read_xlsx(&strict).unwrap();
    assert!(!rep.warnings.iter().any(|w| w.contains("dropped")), "{:?}", rep.warnings);
    let out = Unpacked::new(&write_xlsx(&wb).unwrap());
    assert_extras_kept(&out);
    assert!(!out.text("docProps/app.xml").contains("purl.oclc.org"));
}

/// The children of the root of `xml`, by expanded name (namespace URI, local name) with their text.
fn expanded_children(xml: &str) -> Vec<(String, String, String)> {
    use quick_xml::events::Event;
    use quick_xml::name::ResolveResult;
    let mut reader = quick_xml::NsReader::from_str(xml);
    let mut depth = 0usize;
    let mut out: Vec<(String, String, String)> = vec![];
    loop {
        let event = reader.read_event().unwrap_or_else(|e| panic!("{e}: {xml}"));
        let (e, empty) = match &event {
            Event::Eof => return out,
            Event::Start(e) => (e, false),
            Event::Empty(e) => (e, true),
            Event::End(_) => {
                depth -= 1;
                continue;
            }
            Event::Text(t) if depth == 2 => {
                out.last_mut().unwrap().2.push_str(&t.decode().unwrap());
                continue;
            }
            _ => continue,
        };
        if depth == 1 {
            let (ns, local) = reader.resolver().resolve_element(e.name());
            let ns = match ns {
                ResolveResult::Bound(n) => String::from_utf8_lossy(n.as_ref()).into_owned(),
                _ => String::new(),
            };
            out.push((ns, String::from_utf8_lossy(local.as_ref()).into_owned(), String::new()));
        }
        if !empty {
            depth += 1;
        }
    }
}

#[test]
fn elements_in_the_parts_default_namespace_keep_it() {
    // The core properties namespace as the root's default rather than `cp:`, and the extended
    // properties under a prefix with an element in no namespace at all.
    const CP: &str = "http://schemas.openxmlformats.org/package/2006/metadata/core-properties";
    const EP: &str = "http://schemas.openxmlformats.org/officeDocument/2006/extended-properties";
    let core = format!(
        r#"<?xml version="1.0"?><coreProperties xmlns="{CP}" xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>T</dc:title><category>Confidential</category><revision>7</revision></coreProperties>"#
    );
    let app = format!(r#"<?xml version="1.0"?><ep:Properties xmlns:ep="{EP}"><ep:Manager>Ann</ep:Manager><Loose xmlns="">x</Loose></ep:Properties>"#);
    let (wb, rep) = read_xlsx(&package_with(&core, &app)).unwrap();
    assert!(!rep.warnings.iter().any(|w| w.contains("dropped")), "{:?}", rep.warnings);
    assert_eq!(wb.props.title, "T");
    let out = write_xlsx(&wb).unwrap();
    let u = Unpacked::new(&out);
    let s = |a: &str, b: &str, c: &str| (a.to_string(), b.to_string(), c.to_string());
    let core_kids = expanded_children(&u.text("docProps/core.xml"));
    for kept in [s(CP, "category", "Confidential"), s(CP, "revision", "7")] {
        assert!(core_kids.contains(&kept), "{kept:?}: {}", u.text("docProps/core.xml"));
    }
    let app_kids = expanded_children(&u.text("docProps/app.xml"));
    for kept in [s(EP, "Manager", "Ann"), s("", "Loose", "x")] {
        assert!(app_kids.contains(&kept), "{kept:?}: {}", u.text("docProps/app.xml"));
    }
    // Stable on a second open and save.
    let (again, _) = read_xlsx(&out).unwrap();
    let u2 = Unpacked::new(&write_xlsx(&again).unwrap());
    assert_eq!(expanded_children(&u2.text("docProps/core.xml")), core_kids);
    assert_eq!(expanded_children(&u2.text("docProps/app.xml")), app_kids);
}
