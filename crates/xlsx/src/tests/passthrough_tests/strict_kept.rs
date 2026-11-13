//! A Strict Open XML package keeps its unmodelled content like a Transitional one. GridCraft
//! writes Transitional packages, so what is kept comes back under the Transitional names: the XML
//! namespaces of kept parts and of the XML cut out of the sheets and the workbook, and the
//! relationship types.

use super::*;

/// Each Transitional name of `rich_package` (and its relationship types) with its Strict spelling.
const STRICT_NAMES: [(&str, &str); 10] = [
    ("http://schemas.openxmlformats.org/spreadsheetml/2006/main", "http://purl.oclc.org/ooxml/spreadsheetml/main"),
    (
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/custom-properties",
        "http://purl.oclc.org/ooxml/officeDocument/relationships/customProperties",
    ),
    (
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties",
        "http://purl.oclc.org/ooxml/officeDocument/relationships/extendedProperties",
    ),
    ("http://schemas.openxmlformats.org/officeDocument/2006/relationships", "http://purl.oclc.org/ooxml/officeDocument/relationships"),
    ("http://schemas.openxmlformats.org/officeDocument/2006/custom-properties", "http://purl.oclc.org/ooxml/officeDocument/customProperties"),
    ("http://schemas.openxmlformats.org/officeDocument/2006/customXml", "http://purl.oclc.org/ooxml/officeDocument/customXml"),
    ("http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing", "http://purl.oclc.org/ooxml/drawingml/spreadsheetDrawing"),
    ("http://schemas.openxmlformats.org/drawingml/2006/main", "http://purl.oclc.org/ooxml/drawingml/main"),
    ("http://schemas.openxmlformats.org/officeDocument/2006/extended-properties", "http://purl.oclc.org/ooxml/officeDocument/extendedProperties"),
    ("http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes", "http://purl.oclc.org/ooxml/officeDocument/docPropsVTypes"),
];

/// `package` as a Strict one: the same parts, every XML part under the Strict names.
pub(super) fn strict_copy(package: &[u8]) -> Vec<u8> {
    let u = Unpacked::new(package);
    let renamed: Vec<(String, Vec<u8>)> = u
        .parts
        .iter()
        .map(|(name, data)| match std::str::from_utf8(data) {
            Ok(text) if text.trim_start().starts_with('<') => {
                (name.clone(), STRICT_NAMES.iter().fold(text.to_string(), |t, (from, to)| t.replace(from, to)).into_bytes())
            }
            _ => (name.clone(), data.clone()),
        })
        .collect();
    make_zip(&renamed.iter().map(|(n, d)| (n.as_str(), d.as_slice())).collect::<Vec<_>>())
}

fn has_strict_name(u: &Unpacked) -> Vec<&String> {
    u.parts.iter().filter(|(_, d)| String::from_utf8_lossy(d).contains("purl.oclc.org")).map(|(n, _)| n).collect()
}

#[test]
fn a_strict_package_keeps_what_a_transitional_one_keeps() {
    let strict = strict_copy(&rich_package());
    let fixture = Unpacked::new(&strict);
    let named = has_strict_name(&fixture);
    assert!(named.len() > 8, "the fixture is Strict: {named:?}");
    let (wb, rep) = read_xlsx(&strict).unwrap();
    assert!(wb.preserved.is_some() && wb.sheet(0).unwrap().preserved.is_some(), "{:?}", rep.warnings);
    assert!(!rep.warnings.iter().any(|w| w.contains("dropped") || w.contains("not kept")), "{:?}", rep.warnings);

    let out = write_xlsx(&wb).unwrap();
    let u = Unpacked::new(&out);
    u.assert_valid();
    // No Strict name is left in the Transitional package it is saved as...
    assert_eq!(has_strict_name(&u), Vec::<&String>::new());
    // ...and everything is kept as the Transitional original is: byte for byte, with its links.
    assert_kept_workbook_parts(&u);
    let sheet = sheet_part(&u, "Data");
    assert_kept_sheet_parts(&u, &sheet, &u.text(&sheet));
    // A second round trip changes nothing.
    assert_round_trip_kept_content(&wb, &out);
}

#[test]
fn what_a_strict_package_mentions_is_not_renamed() {
    // Kept data that only names a Strict URI (it isn't a namespace declaration) is the file's.
    let data = r#"<?xml version="1.0"?><root xmlns="urn:example:data"><v>http://purl.oclc.org/ooxml/spreadsheetml/main</v></root>"#;
    let u = Unpacked::new(&strict_copy(&rich_package()));
    let parts: Vec<(&str, &[u8])> =
        u.parts.iter().map(|(k, v)| (k.as_str(), if k == "customXml/item1.xml" { data.as_bytes() } else { v.as_slice() })).collect();
    let (wb, _) = read_xlsx(&make_zip(&parts)).unwrap();
    let out = Unpacked::new(&write_xlsx(&wb).unwrap());
    assert_eq!(out.find(data.as_bytes()).len(), 1, "{:?}", out.parts.keys());
}

#[test]
fn only_namespace_declarations_are_renamed_not_text_that_spells_one() {
    // A declaration's spelling in character data, CDATA, a comment, a processing instruction and
    // another attribute's value is data; the root's real declaration is renamed.
    let decl = r#"xmlns="http://purl.oclc.org/ooxml/spreadsheetml/main""#;
    let data = format!(
        r#"<?xml version="1.0"?><root xmlns="urn:example:data" xmlns:s="http://purl.oclc.org/ooxml/spreadsheetml/main" note='{}'><!-- <a {decl}/> --><?pi <a {decl}/>?><v><![CDATA[<a {decl}/>]]></v><w>&lt;a {}/&gt;</w><s:x/></root>"#,
        decl.replace('"', "&quot;"),
        decl.replace('"', "&quot;")
    );
    let expected = data.replace(
        r#"xmlns:s="http://purl.oclc.org/ooxml/spreadsheetml/main""#,
        r#"xmlns:s="http://schemas.openxmlformats.org/spreadsheetml/2006/main""#,
    );
    assert_ne!(expected, data);
    // A part that isn't XML is never parsed, whatever text it holds.
    let printer = format!("not xml <a {decl}/>");
    let u = Unpacked::new(&strict_copy(&rich_package()));
    let parts: Vec<(&str, &[u8])> = u
        .parts
        .iter()
        .map(|(k, v)| {
            let v = match k.as_str() {
                "customXml/item1.xml" => data.as_bytes(),
                "xl/printerSettings/printerSettings1.bin" => printer.as_bytes(),
                _ => v.as_slice(),
            };
            (k.as_str(), v)
        })
        .collect();
    let (wb, _) = read_xlsx(&make_zip(&parts)).unwrap();
    let out = Unpacked::new(&write_xlsx(&wb).unwrap());
    assert_eq!(out.find(expected.as_bytes()).len(), 1, "{:?}", out.parts.iter().filter(|(k, _)| k.contains("item")).collect::<Vec<_>>());
    assert_eq!(out.find(printer.as_bytes()).len(), 1, "{:?}", out.parts.keys());
}
