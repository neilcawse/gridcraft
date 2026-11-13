//! Strict Open XML (ISO/IEC 29500) kept as Transitional.
//!
//! GridCraft writes Transitional packages. Strict is the same schemas under other names, so what a
//! Strict package carries over (links to workbooks, custom XML, printer settings, form controls…)
//! only needs renaming: the XML namespaces of its parts and the relationship types, which differ in
//! the base URI and, for two kinds, in the spelling. Nothing else is touched: data inside a part
//! that merely mentions a Strict name is not a namespace and stays as it was.

use std::borrow::Cow;

/// The start of every Strict namespace and relationship type.
const STRICT: &str = "http://purl.oclc.org/ooxml/";
/// Relationship types of the office-document namespace: `<base>officeDocument`, `<base>worksheet`…
const REL_STRICT: &str = "http://purl.oclc.org/ooxml/officeDocument/relationships/";
const REL_TRANSITIONAL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/";
/// The families of schemas a Strict namespace belongs to: `<STRICT><area>/<name>`.
const AREAS: &[&str] = &["spreadsheetml", "drawingml", "officeDocument", "wordprocessingml", "presentationml"];

/// The Transitional spelling of a name that is not the Strict one with `/2006` added: the two
/// document-property schemas and their relationship types are hyphenated.
fn hyphenated(name: &str) -> &str {
    match name {
        "extendedProperties" => "extended-properties",
        "customProperties" => "custom-properties",
        other => other,
    }
}

/// The Transitional spelling of a relationship type; the type itself when it isn't Strict.
pub(crate) fn rel_type(rel_type: &str) -> Cow<'_, str> {
    match rel_type.strip_prefix(REL_STRICT) {
        Some(kind) => Cow::Owned(format!("{REL_TRANSITIONAL}{}", hyphenated(kind))),
        None => Cow::Borrowed(rel_type),
    }
}

/// The Transitional spelling of the Strict XML namespace `uri`. `None` for a name that isn't one
/// of a spreadsheet package's schemas.
fn namespace(uri: &str) -> Option<String> {
    let (area, name) = uri.strip_prefix(STRICT)?.split_once('/')?;
    (AREAS.contains(&area) && !name.is_empty()).then(|| format!("http://schemas.openxmlformats.org/{area}/2006/{}", hyphenated(name)))
}

/// The Transitional spelling of the namespace `uri`: `uri` itself when it isn't a Strict one.
pub(super) fn transitional(uri: &str) -> Cow<'_, str> {
    namespace(uri).map_or(Cow::Borrowed(uri), Cow::Owned)
}

/// The position just past the first `end` in `text` at or after `from` (the end of the text when
/// there is none).
fn past(text: &str, from: usize, end: &str) -> usize {
    text.get(from..).and_then(|s| s.find(end)).map_or(text.len(), |i| from + i + end.len())
}

/// The end of a `<!DOCTYPE …>` declaration starting at `from`: its `>`, outside quotes and its
/// internal subset.
fn past_declaration(b: &[u8], from: usize) -> usize {
    let (mut depth, mut quote) = (0usize, None);
    for (i, &c) in b.iter().enumerate().skip(from) {
        match (quote, c) {
            (Some(q), _) if c == q => quote = None,
            (Some(_), _) => {}
            (None, b'"' | b'\'') => quote = Some(c),
            (None, b'[') => depth += 1,
            (None, b']') => depth = depth.saturating_sub(1),
            (None, b'>') if depth == 0 => return i + 1,
            _ => {}
        }
    }
    b.len()
}

/// The namespace names of the start tag at `lt`: the namespace declarations (`xmlns`,
/// `xmlns:prefix`) and the `uri` of a DrawingML `graphicData`, which names the namespace of the
/// content it holds (a chart, a diagram: ECMA-376 Part 1, §20.1.2.2.17). The byte range of each
/// value, quotes excluded, and where the tag ends. `None` when the tag is cut off or malformed.
fn start_tag(b: &[u8], lt: usize, decls: &mut Vec<(usize, usize)>) -> Option<usize> {
    let name_char = |c: u8| !(c.is_ascii_whitespace() || matches!(c, b'/' | b'>' | b'=' | b'"' | b'\''));
    let mut i = lt + 1;
    while b.get(i).copied().is_some_and(name_char) {
        i += 1;
    }
    let element = b.get(lt + 1..i)?;
    let graphic_data = element.rsplit(|&c| c == b':').next() == Some(b"graphicData".as_slice());
    loop {
        while b.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        match b.get(i)? {
            b'>' => return Some(i + 1),
            b'/' if b.get(i + 1) == Some(&b'>') => return Some(i + 2),
            _ => {}
        }
        let name_start = i;
        while b.get(i).copied().is_some_and(name_char) {
            i += 1;
        }
        let name = &b[name_start..i];
        while b.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        if name.is_empty() || b.get(i) != Some(&b'=') {
            return None;
        }
        i += 1;
        while b.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        let quote = *b.get(i).filter(|q| matches!(q, b'"' | b'\''))?;
        let start = i + 1;
        let end = start + b.get(start..)?.iter().position(|&c| c == quote)?;
        if name == b"xmlns" || name.starts_with(b"xmlns:") || (graphic_data && name == b"uri") {
            decls.push((start, end));
        }
        i = end + 1;
    }
}

/// XML text with the Strict namespaces it names renamed to the Transitional ones. Only the values
/// of `xmlns` and `xmlns:prefix` attributes of start tags are namespace declarations, and the
/// `uri` of a `graphicData` names one: character data, CDATA sections, comments, processing
/// instructions and other attributes are left as they are, whatever they spell.
pub(crate) fn xml(text: &str) -> Cow<'_, str> {
    if !text.contains(STRICT) {
        return Cow::Borrowed(text);
    }
    let b = text.as_bytes();
    let mut decls = vec![];
    let mut i = 0;
    while let Some(lt) = b.get(i..).and_then(|s| s.iter().position(|&c| c == b'<')).map(|j| i + j) {
        let rest = &text[lt..];
        i = if rest.starts_with("<!--") {
            past(text, lt + 4, "-->")
        } else if rest.starts_with("<![CDATA[") {
            past(text, lt + 9, "]]>")
        } else if rest.starts_with("<?") {
            past(text, lt + 2, "?>")
        } else if rest.starts_with("<!") {
            past_declaration(b, lt + 2)
        } else if rest.starts_with("</") {
            past(text, lt + 2, ">")
        } else {
            match start_tag(b, lt, &mut decls) {
                Some(end) => end,
                // Not XML past this point: nothing more is renamed.
                None => break,
            }
        };
    }
    let mut out: Option<String> = None;
    let mut copied = 0;
    for (start, end) in decls {
        let Some(new) = namespace(&text[start..end]) else { continue };
        let o = out.get_or_insert_with(|| String::with_capacity(text.len()));
        o.push_str(&text[copied..start]);
        o.push_str(&new);
        copied = end;
    }
    match out {
        Some(mut o) => {
            o.push_str(&text[copied..]);
            Cow::Owned(o)
        }
        None => Cow::Borrowed(text),
    }
}

/// Whether a part of `content_type` is XML: `…+xml`, `application/xml`, `text/xml`, and the
/// legacy VML drawings, which are XML under a type of their own.
pub(super) fn is_xml_type(content_type: &str) -> bool {
    let t = content_type.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    t.ends_with("+xml") || t.ends_with("/xml") || t == super::CT_VML.to_ascii_lowercase()
}

/// A part of a Strict package as it is kept: XML in Strict namespaces renamed, anything else (a
/// part whose content type isn't XML, XML in other namespaces) untouched. `Err` when the part is
/// XML that can't be renamed (UTF-16): writing it as it is would put Strict names into a
/// Transitional package.
pub(crate) fn part(data: Vec<u8>, content_type: &str) -> Result<Vec<u8>, &'static str> {
    if !is_xml_type(content_type) {
        return Ok(data);
    }
    let Ok(text) = std::str::from_utf8(&data) else {
        let utf16 = data.starts_with(&[0xFF, 0xFE]) || data.starts_with(&[0xFE, 0xFF]);
        return if utf16 && crate::xml::to_utf8(&data).windows(STRICT.len()).any(|w| w == STRICT.as_bytes()) {
            Err("it is Strict Open XML in UTF-16")
        } else {
            Ok(data)
        };
    };
    match xml(text) {
        Cow::Borrowed(_) => Ok(data),
        Cow::Owned(renamed) => Ok(renamed.into_bytes()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relationship_types_are_renamed() {
        let strict = |kind: &str| format!("http://purl.oclc.org/ooxml/officeDocument/relationships/{kind}");
        let transitional = |kind: &str| format!("http://schemas.openxmlformats.org/officeDocument/2006/relationships/{kind}");
        for kind in ["officeDocument", "worksheet", "printerSettings", "externalLink", "externalLinkPath", "customXml", "xlStartup"] {
            assert_eq!(rel_type(&strict(kind)), transitional(kind));
        }
        // The two kinds with a hyphen in Transitional.
        assert_eq!(rel_type(&strict("extendedProperties")), transitional("extended-properties"));
        assert_eq!(rel_type(&strict("customProperties")), transitional("custom-properties"));
        // Types that aren't Strict are as they were.
        for same in [
            transitional("worksheet"),
            "http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties".to_string(),
            "http://schemas.microsoft.com/office/2006/relationships/wsSortMap".to_string(),
            String::new(),
        ] {
            assert!(matches!(rel_type(&same), Cow::Borrowed(_)), "{same}");
        }
    }

    #[test]
    fn namespaces_follow_one_rule() {
        for (strict, transitional) in [
            ("spreadsheetml/main", "spreadsheetml/2006/main"),
            ("drawingml/main", "drawingml/2006/main"),
            ("drawingml/chart", "drawingml/2006/chart"),
            ("drawingml/spreadsheetDrawing", "drawingml/2006/spreadsheetDrawing"),
            ("officeDocument/relationships", "officeDocument/2006/relationships"),
            ("officeDocument/sharedTypes", "officeDocument/2006/sharedTypes"),
            ("officeDocument/docPropsVTypes", "officeDocument/2006/docPropsVTypes"),
            ("officeDocument/extendedProperties", "officeDocument/2006/extended-properties"),
            ("officeDocument/customProperties", "officeDocument/2006/custom-properties"),
            ("officeDocument/customXml", "officeDocument/2006/customXml"),
        ] {
            assert_eq!(
                namespace(&format!("{STRICT}{strict}")).as_deref(),
                Some(format!("http://schemas.openxmlformats.org/{transitional}").as_str())
            );
        }
        for other in
            ["urn:example", "http://purl.oclc.org/ooxml/unknown/main", "http://purl.oclc.org/ooxml/drawingml/", "http://purl.oclc.org/ooxml/"]
        {
            assert_eq!(namespace(other), None, "{other}");
        }
    }

    #[test]
    fn declared_namespaces_are_renamed() {
        let strict = r#"<externalLink xmlns="http://purl.oclc.org/ooxml/spreadsheetml/main" xmlns:r = 'http://purl.oclc.org/ooxml/officeDocument/relationships' xmlns:a="http://purl.oclc.org/ooxml/drawingml/main" xmlns:x="urn:example"><externalBook r:id="rId1"/></externalLink>"#;
        let out = xml(strict);
        assert!(!out.contains("purl.oclc.org"), "{out}");
        assert!(out.contains(r#"xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main""#), "{out}");
        assert!(out.contains("xmlns:r = 'http://schemas.openxmlformats.org/officeDocument/2006/relationships'"), "{out}");
        assert!(out.contains(r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main""#), "{out}");
        assert!(out.contains(r#"xmlns:x="urn:example""#) && out.contains(r#"<externalBook r:id="rId1"/>"#));
        assert!(matches!(xml("<a xmlns=\"urn:example\"/>"), Cow::Borrowed(_)));
    }

    #[test]
    fn the_content_a_graphic_frame_holds_is_renamed() {
        let frame = r#"<a:graphicData xmlns:a="http://purl.oclc.org/ooxml/drawingml/main" uri="http://purl.oclc.org/ooxml/drawingml/chart"><c:chart xmlns:c="http://purl.oclc.org/ooxml/drawingml/chart"/></a:graphicData>"#;
        let out = xml(frame);
        assert!(!out.contains("purl.oclc.org"), "{out}");
        assert!(out.contains(r#"uri="http://schemas.openxmlformats.org/drawingml/2006/chart""#), "{out}");
        // Another element's `uri` (an extension's) is not a namespace.
        let ext = r#"<ext uri="http://purl.oclc.org/ooxml/drawingml/chart"/>"#;
        assert!(matches!(xml(ext), Cow::Borrowed(_)));
    }

    #[test]
    fn only_declarations_are_renamed() {
        // A name in data or in an attribute that isn't a namespace declaration is the file's.
        let kept = r#"<a xmlns="urn:example" href="http://purl.oclc.org/ooxml/spreadsheetml/main" xmlnsx="http://purl.oclc.org/ooxml/spreadsheetml/main">see http://purl.oclc.org/ooxml/drawingml/main</a>"#;
        assert!(matches!(xml(kept), Cow::Borrowed(_)));
        // Nor is a declaration's spelling in text, CDATA, a comment, a processing instruction, a
        // document type or an attribute value: only the real declaration after them is renamed.
        let decl = r#"xmlns="http://purl.oclc.org/ooxml/spreadsheetml/main""#;
        let data = format!(
            r#"<?xml version="1.0"?><!DOCTYPE a [<!ENTITY e '<b {decl}>'>]><!-- <b {decl}> --><?pi <b {decl}>?><a t='<b {decl}>' {decl}><![CDATA[<b {decl}>]]>x {decl} y</a>"#
        );
        let out = xml(&data);
        assert_eq!(out.matches("purl.oclc.org").count(), data.matches("purl.oclc.org").count() - 1, "{out}");
        assert!(out.contains(r#"' xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><![CDATA["#), "{out}");
        // A cut-off tag ends the renaming; what came before it is renamed.
        let cut = format!("<a {decl}><b xmlns:c=\"http://purl.oclc.org/ooxml/drawingml/main");
        let out = xml(&cut);
        assert!(
            out.starts_with(r#"<a xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">"#) && out.ends_with("drawingml/main"),
            "{out}"
        );
    }

    #[test]
    fn only_xml_parts_are_renamed() {
        const XML: &str = "application/xml";
        let named = |ns: &str| format!("<?xml version=\"1.0\"?><a xmlns=\"{ns}\"/>");
        let renamed = part(named("http://purl.oclc.org/ooxml/spreadsheetml/main").into_bytes(), XML).unwrap();
        assert_eq!(String::from_utf8(renamed).unwrap(), named("http://schemas.openxmlformats.org/spreadsheetml/2006/main"));
        for ct in ["application/vnd.ms-excel.externalLink+xml", "text/xml; charset=utf-8", "application/vnd.openxmlformats-officedocument.vmlDrawing"]
        {
            assert!(part(named("http://purl.oclc.org/ooxml/drawingml/main").into_bytes(), ct).unwrap().ends_with(b"2006/main\"/>"), "{ct}");
        }
        // A byte-order mark is kept.
        let with_bom = [b"\xEF\xBB\xBF".as_slice(), named("http://purl.oclc.org/ooxml/drawingml/main").as_bytes()].concat();
        assert!(part(with_bom, XML).unwrap().starts_with(b"\xEF\xBB\xBF<?xml"));
        // Binary data that holds the name is not XML, and neither is text in a part that isn't.
        let binary = [b"\x00\x01 xmlns=\"http://purl.oclc.org/ooxml/spreadsheetml/main\"".as_slice(), &[0xFF, 0xFE]].concat();
        assert_eq!(part(binary.clone(), XML).unwrap(), binary);
        let text = named("http://purl.oclc.org/ooxml/spreadsheetml/main").into_bytes();
        for ct in ["application/vnd.openxmlformats-officedocument.spreadsheetml.printerSettings", "application/octet-stream", "text/plain"] {
            assert_eq!(part(text.clone(), ct).unwrap(), text, "{ct}");
        }
        // XML that holds no Strict name, and XML that can't be renamed.
        let other = named("urn:example").into_bytes();
        assert_eq!(part(other.clone(), XML).unwrap(), other);
        let utf16 = |s: String| -> Vec<u8> { [0xFF, 0xFE].into_iter().chain(s.encode_utf16().flat_map(u16::to_le_bytes)).collect() };
        assert!(part(utf16(named("http://purl.oclc.org/ooxml/spreadsheetml/main")), XML).is_err());
        assert!(part(utf16(named("urn:example")), XML).is_ok());
    }
}
