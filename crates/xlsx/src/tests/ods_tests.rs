//! Original ODS packages assembled in Rust, not files from another spreadsheet application.

use gridcraft_core::{CellRef, RangeRef, Value};

use super::make_zip;
use crate::{Format, IoError, read_ods, read_xlsx, sniff, write_xlsx};

const MIME: &[u8] = b"application/vnd.oasis.opendocument.spreadsheet";
const MANIFEST: &[u8] = br#"<manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0"><manifest:file-entry manifest:full-path="/" manifest:media-type="application/vnd.oasis.opendocument.spreadsheet"/><manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml"/></manifest:manifest>"#;

fn content(tables: &str) -> String {
    format!(
        r#"<?xml version="1.0"?><office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:of="urn:oasis:names:tc:opendocument:xmlns:of:1.2"><office:body><office:spreadsheet>{tables}</office:spreadsheet></office:body></office:document-content>"#
    )
}
fn ods(tables: &str) -> Vec<u8> {
    make_zip(&[("mimetype", MIME), ("META-INF/manifest.xml", MANIFEST), ("content.xml", content(tables).as_bytes())])
}
fn row(cells: &str) -> Vec<u8> {
    ods(&format!(r#"<table:table table:name="Data"><table:table-row>{cells}</table:table-row></table:table>"#))
}
fn text(s: &str) -> Value {
    Value::Text(s.into())
}

/// A start tag with thousands of `xmlns:` declarations is refused instead of allocated
/// (RUSTSEC-2026-0195: the resolver kept a binding per declaration, with no cap).
#[test]
fn ods_namespace_declaration_bomb_is_rejected() {
    let bomb = |n: usize| -> String { (0..n).map(|i| format!(" xmlns:p{i}=\"urn:p{i}\"")).collect() };
    let doc = |decls: &str| {
        format!(
            r#"<?xml version="1.0"?><office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0"{decls}><office:body><office:spreadsheet><table:table table:name="Data"><table:table-row><table:table-cell office:value-type="float" office:value="1"><text:p>1</text:p></table:table-cell></table:table-row></table:table></office:spreadsheet></office:body></office:document-content>"#
        )
    };
    let build = |decls: &str| make_zip(&[("mimetype", MIME), ("META-INF/manifest.xml", MANIFEST), ("content.xml", doc(decls).as_bytes())]);
    let ok = read_ods(&build(&bomb(8)));
    assert!(ok.is_ok(), "a handful of namespaces still reads: {:?}", ok.err());
    assert!(read_ods(&build(&bomb(10_000))).is_err(), "a tag with thousands of declarations is refused");
}

#[test]
fn ods_typed_values_and_formula_caches_are_read_and_survive_xlsx() {
    let bytes = row(r#"
        <table:table-cell office:value-type="float" office:value="42.5"/>
        <table:table-cell office:value-type="boolean" office:boolean-value="true"/>
        <table:table-cell office:value-type="percentage" office:value="0.25"/>
        <table:table-cell office:value-type="currency" office:currency="USD" office:value="19.99"/>
        <table:table-cell office:value-type="string" office:string-value="literal =1+1"/>
        <table:table-cell table:formula="of:=[.A1]*2" office:value-type="float" office:value="85"><text:p>85</text:p></table:table-cell>
        <table:table-cell table:formula="of:=1/0" office:value-type="string"><text:p>#DIV/0!</text:p></table:table-cell>
        <table:table-cell table:formula="of:=ORG.EXAMPLE.UNKNOWN()"/>
        <table:table-cell office:value-type="float" office:value="NaN"><text:p>unavailable</text:p></table:table-cell>
        <table:table-cell office:value-type="boolean" office:boolean-value="0"/>
    "#);
    let (wb, report) = read_ods(&bytes).unwrap();
    assert!(report.warnings.iter().any(|w| w.contains("formulas") && w.contains("keep their saved results")));
    assert!(report.warnings.iter().any(|w| w.contains("unsupported or missing")));
    let expected = [
        Value::Number(42.5),
        Value::Bool(true),
        Value::Number(0.25),
        Value::Number(19.99),
        text("literal =1+1"),
        Value::Number(85.0),
        text("#DIV/0!"),
        text("[Unavailable ODS formula result]"),
        text("unavailable"),
        Value::Bool(false),
    ];
    for (c, value) in expected.iter().enumerate() {
        assert_eq!(&wb.sheets[0].value(CellRef::new(0, c as u32)), value);
        let imported = wb.sheets[0].cell(CellRef::new(0, c as u32)).unwrap().formula.is_some();
        assert_eq!(imported, c == 5 || c == 6, "column {c}: only translatable formulas are imported");
    }
    let (reopened, _) = read_xlsx(&write_xlsx(&wb).unwrap()).unwrap();
    for (c, value) in expected.iter().enumerate() {
        assert_eq!(&reopened.sheets[0].value(CellRef::new(0, c as u32)), value);
    }
}

#[test]
fn ods_ordered_text_preserves_paragraphs_runs_entities_spaces_tabs_and_breaks() {
    let bytes = row(
        r#"<table:table-cell office:value-type="string"><text:p>one <text:span>two &amp; three</text:span> four<text:s text:c="3"/><text:a>five</text:a><text:tab/>six<text:line-break/>seven&#33;<![CDATA[<eight>]]></text:p><text:p>nine</text:p><office:annotation><text:p>not cell text</text:p></office:annotation></table:table-cell>"#,
    );
    let (wb, _) = read_ods(&bytes).unwrap();
    assert_eq!(wb.sheets[0].value(CellRef::new(0, 0)), text("one two & three four   five\tsix\nseven!<eight>\nnine"));
    let bytes = row(r#"<table:table-cell office:value-type="string"><text:p>  one
        <text:span> two  </text:span>  three <text:s text:c="0"/>four<text:s text:c="2"/>  </text:p><text:p><text:s/>  five  </text:p></table:table-cell>"#);
    assert_eq!(read_ods(&bytes).unwrap().0.sheets[0].value(CellRef::new(0, 0)), text("one two three four  \n  five"));
}

#[test]
fn ods_repeats_merges_sheet_names_and_empty_tail_remain_sparse() {
    let bytes = ods(r#"
      <table:table table:name="First &amp; Second">
        <table:table-header-rows><table:table-row><table:table-cell table:number-columns-spanned="2" table:number-rows-spanned="2" office:value-type="string"><text:p>merged</text:p></table:table-cell><table:covered-table-cell office:value-type="float" office:value="99"/></table:table-row></table:table-header-rows>
        <table:table-row><table:covered-table-cell table:number-columns-repeated="2"/></table:table-row>
        <table:table-row-group><table:table-row table:number-rows-repeated="2"><table:table-cell table:number-columns-repeated="3" office:value-type="float" office:value="7"/></table:table-row></table:table-row-group>
        <table:table-row table:number-rows-repeated="1048572"><table:table-cell table:number-columns-repeated="16384"/></table:table-row>
      </table:table>
      <table:table table:name="Another"><table:table-row><table:table-cell office:value-type="string" office:string-value="second sheet"/></table:table-row></table:table>
    "#);
    let (wb, _) = read_ods(&bytes).unwrap();
    assert_eq!(wb.sheets.len(), 2);
    assert_eq!(wb.sheets[0].name, "First & Second");
    assert_eq!(wb.sheets[0].cells.len(), 8);
    assert_eq!(wb.sheets[0].value(CellRef::new(0, 1)), Value::Number(99.0));
    assert_eq!(wb.sheets[0].merges, vec![RangeRef::new(CellRef::new(0, 0), CellRef::new(1, 1))]);
    assert_eq!(wb.sheets[0].value(CellRef::new(3, 2)), Value::Number(7.0));
    assert_eq!(wb.sheets[1].value(CellRef::new(0, 0)), text("second sheet"));
}

#[test]
fn ods_dates_and_durations_use_formats_and_unsupported_values_stay_text() {
    let bytes = row(r#"
        <table:table-cell office:value-type="date" office:date-value="1900-02-28"/>
        <table:table-cell office:value-type="date" office:date-value="2026-10-07T18:30:00.125"/>
        <table:table-cell office:value-type="time" office:time-value="P1DT2H3M4.5S"/>
        <table:table-cell office:value-type="time" office:time-value="-PT2H"/>
        <table:table-cell office:value-type="date" office:date-value="1900-02-29"/>
        <table:table-cell office:value-type="date" office:date-value="2026-10-07T18:00:00Z"/>
        <table:table-cell office:value-type="time" office:time-value="P1M"/>
        <table:table-cell office:value-type="time" office:time-value="PT2M1H"/>
    "#);
    let (wb, report) = read_ods(&bytes).unwrap();
    let s = &wb.sheets[0];
    assert_eq!(s.value(CellRef::new(0, 0)), Value::Number(59.0));
    assert_eq!(s.value(CellRef::new(0, 1)), Value::Number(46302.0 + 66600.125 / 86400.0));
    assert_eq!(s.value(CellRef::new(0, 2)), Value::Number(1.0 + 7384.5 / 86400.0));
    assert_eq!(s.value(CellRef::new(0, 3)), Value::Number(-2.0 / 24.0));
    for (c, value) in [(4, "1900-02-29"), (5, "2026-10-07T18:00:00Z"), (6, "P1M"), (7, "PT2M1H")] {
        assert_eq!(s.value(CellRef::new(0, c)), text(value));
    }
    assert_eq!(wb.styles.get(s.cell(CellRef::new(0, 0)).unwrap().style).num_fmt.0.as_ref(), "yyyy-mm-dd");
    assert_eq!(wb.styles.get(s.cell(CellRef::new(0, 2)).unwrap().style).num_fmt.0.as_ref(), "[h]:mm:ss.000");
    assert_eq!(report.warnings.len(), 2);
}

#[test]
fn ods_namespaces_are_resolved_not_inferred_from_prefix_or_local_name() {
    let xml = content(r#"<table:table table:name="Actual"><table:table-row><table:table-cell office:value-type="float" office:value="9"/></table:table-row></table:table>"#)
        .replace("<office:", "<o:").replace("</office:", "</o:").replace(" office:", " o:").replace("xmlns:office=", "xmlns:o=")
        .replace("<table:", "<t:").replace("</table:", "</t:").replace(" table:", " t:").replace("xmlns:table=", "xmlns:t=");
    let bytes = make_zip(&[("mimetype", MIME), ("content.xml", xml.as_bytes())]);
    assert_eq!(read_ods(&bytes).unwrap().0.sheets[0].value(CellRef::new(0, 0)), Value::Number(9.0));
    // Tables in a namespace that is not the table namespace are not tables: none is read, and
    // the workbook gets the empty placeholder sheet instead of "Actual".
    let spoofed = xml.replace("urn:oasis:names:tc:opendocument:xmlns:table:1.0", "wrong-namespace");
    let (wb, _) = read_ods(&make_zip(&[("mimetype", MIME), ("content.xml", spoofed.as_bytes())])).unwrap();
    assert_eq!(wb.sheets.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["Sheet1"]);
    assert_eq!(wb.sheets[0].cells.len(), 0);
    let bytes =
        row(r#"<table:table-cell xmlns:fake="wrong-namespace" fake:value-type="float" fake:value="123"><text:p>kept</text:p></table:table-cell>"#);
    assert_eq!(read_ods(&bytes).unwrap().0.sheets[0].value(CellRef::new(0, 0)), text("kept"));
}

#[test]
fn ods_rejects_bad_packages_encryption_and_truncated_xml() {
    assert!(read_ods(b"not zip").is_err());
    let xml = content(r#"<table:table table:name="Data"/>"#);
    for mime in [b"application/zip".as_slice(), b"application/vnd.oasis.opendocument.spreadsheet "] {
        assert!(read_ods(&make_zip(&[("mimetype", mime), ("content.xml", xml.as_bytes())])).is_err());
    }
    let encrypted =
        br#"<manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0"><manifest:encryption-data/></manifest:manifest>"#;
    assert!(
        read_ods(&make_zip(&[("mimetype", MIME), ("META-INF/manifest.xml", encrypted), ("content.xml", xml.as_bytes())]))
            .unwrap_err()
            .to_string()
            .contains("encrypted")
    );
    for bad in [
        xml.trim_end_matches("</office:document-content>").to_owned(),
        xml.replace("</office:spreadsheet>", "</office:wrong>"),
        "<root/>".to_owned(),
        format!("{xml}{xml}"),
    ] {
        assert!(read_ods(&make_zip(&[("mimetype", MIME), ("content.xml", bad.as_bytes())])).is_err(), "{bad}");
    }
    let deep = content(&format!("{}{}", "<x>".repeat(300), "</x>".repeat(300)));
    assert!(read_ods(&make_zip(&[("mimetype", MIME), ("content.xml", deep.as_bytes())])).is_err());
}

#[test]
fn ods_expansion_bounds_stop_small_repeat_bombs() {
    for cells in [
        r#"<table:table-cell table:number-columns-repeated="4294967296"/>"#,
        r#"<table:table-cell table:number-columns-repeated="0"/>"#,
        r#"<table:table-cell table:number-columns-repeated="16384" table:number-columns-spanned="2" office:value-type="float" office:value="1"/>"#,
        r#"<table:table-cell office:value-type="string"><text:p><text:s text:c="4294967295"/></text:p></table:table-cell>"#,
    ] {
        assert!(read_ods(&row(cells)).is_err(), "{cells}");
    }
    let cells = ods(
        r#"<table:table table:name="Data"><table:table-row table:number-rows-repeated="1048576"><table:table-cell office:value-type="float" office:value="1"/></table:table-row></table:table>"#,
    );
    assert!(matches!(read_ods(&cells), Err(IoError::TooLarge(_))));
    let text = format!(
        r#"<table:table table:name="Data"><table:table-row table:number-rows-repeated="100000"><table:table-cell office:value-type="string" office:string-value="{}"/></table:table-row></table:table>"#,
        "x".repeat(1024)
    );
    assert!(matches!(read_ods(&ods(&text)), Err(IoError::TooLarge(_))));
    // One row of cells each just under the 1 MiB cell limit, past 64 MiB in all: refused while
    // the row is read, before it holds them all. The row's end is never reached.
    let long = format!(r#"<table:table-cell office:value-type="string" office:string-value="{}"/>"#, "x".repeat(1024 * 1024 - 1));
    let cells = ods(&format!(r#"<table:table table:name="Data"><table:table-row>{}"#, long.repeat(65)));
    assert!(matches!(read_ods(&cells), Err(IoError::TooLarge(_))));
    // A row repeated once more than its cells' text allows is refused too, and one that fits reads.
    let cell = format!(r#"<table:table-cell office:value-type="string" office:string-value="{}"/>"#, "y".repeat(1024));
    let rows = |n: u32| {
        ods(&format!(r#"<table:table table:name="Data"><table:table-row table:number-rows-repeated="{n}">{cell}</table:table-row></table:table>"#))
    };
    assert!(read_ods(&rows(65_536)).is_ok());
    assert!(matches!(read_ods(&rows(65_537)), Err(IoError::TooLarge(_))));
}

#[test]
fn ods_cells_past_the_sheet_edge_are_dropped_with_a_warning() {
    let beyond = |report: &crate::ReadReport| report.warnings.iter().any(|w| w.contains("beyond the last row or column"));
    // A value repeated past the last column keeps the columns that exist, and says so.
    let (wb, report) =
        read_ods(&row(r#"<table:table-cell table:number-columns-repeated="4294967295" office:value-type="float" office:value="1"/>"#)).unwrap();
    assert_eq!(wb.sheets[0].cells.len(), 16384);
    assert!(beyond(&report), "{:?}", report.warnings);
    // Cells that start past the last column (a value, a merge) are dropped whole.
    for past in [r#"<table:table-cell office:value-type="float" office:value="1"/>"#, r#"<table:table-cell table:number-columns-spanned="2"/>"#] {
        let cells = format!(
            r#"<table:table-cell office:value-type="float" office:value="5"/><table:table-cell table:number-columns-repeated="16383"/>{past}"#
        );
        let (wb, report) = read_ods(&row(&cells)).unwrap();
        assert_eq!(wb.sheets[0].cells.len(), 1, "{past}");
        assert_eq!(wb.sheets[0].value(CellRef::new(0, 0)), Value::Number(5.0));
        assert!(wb.sheets[0].merges.is_empty());
        assert!(beyond(&report), "{past}: {:?}", report.warnings);
    }
    // The same after the last row.
    let late = ods(
        r#"<table:table table:name="Data"><table:table-row table:number-rows-repeated="1048576"><table:table-cell/></table:table-row><table:table-row><table:table-cell office:value-type="float" office:value="1"/></table:table-row></table:table>"#,
    );
    let (wb, report) = read_ods(&late).unwrap();
    assert_eq!(wb.sheets[0].cells.len(), 0);
    assert!(beyond(&report), "{:?}", report.warnings);
    // A sheet from a program with 16,777,216 rows: what fits is kept, the rest is reported.
    let wide = ods(r#"<table:table table:name="Data">
            <table:table-row><table:table-cell office:value-type="float" office:value="1"/><table:table-cell table:number-columns-repeated="16380"/><table:table-cell office:value-type="float" office:value="2"/><table:table-cell table:number-columns-repeated="2"/><table:table-cell office:value-type="float" office:value="3"/></table:table-row>
            <table:table-row table:number-rows-repeated="16777212"><table:table-cell table:number-columns-repeated="4"/></table:table-row>
            <table:table-row><table:table-cell office:value-type="float" office:value="11"/></table:table-row>
            <table:table-row table:number-rows-repeated="2"><table:table-cell table:number-columns-repeated="4"/></table:table-row>
            <table:table-row><table:table-cell office:value-type="float" office:value="21"/></table:table-row>
          </table:table>"#);
    let (wb, report) = read_ods(&wide).unwrap();
    assert_eq!(wb.sheets[0].cells.len(), 2);
    assert_eq!(wb.sheets[0].value(CellRef::new(0, 0)), Value::Number(1.0));
    assert_eq!(wb.sheets[0].value(CellRef::new(0, 16381)), Value::Number(2.0));
    assert!(beyond(&report), "{:?}", report.warnings);
    // Dropping what is past the edge doesn't lift the cell budget for what is not.
    let bomb = ods(
        r#"<table:table table:name="Data"><table:table-row table:number-rows-repeated="999999999"><table:table-cell table:number-columns-repeated="999999999" office:value-type="float" office:value="1"/></table:table-row></table:table>"#,
    );
    assert!(matches!(read_ods(&bomb), Err(IoError::TooLarge(_))));
}

#[test]
fn ods_template_is_a_spreadsheet() {
    const TEMPLATE: &[u8] = b"application/vnd.oasis.opendocument.spreadsheet-template";
    let xml = content(
        r#"<table:table table:name="Data"><table:table-row><table:table-cell office:value-type="float" office:value="4"/></table:table-row></table:table>"#,
    );
    let bytes = make_zip(&[("mimetype", TEMPLATE), ("content.xml", xml.as_bytes())]);
    assert_eq!(sniff(&bytes), Format::Ods);
    assert_eq!(read_ods(&bytes).unwrap().0.sheets[0].value(CellRef::new(0, 0)), Value::Number(4.0));
    // Only the two exact types are spreadsheets.
    for mime in [
        b"application/vnd.oasis.opendocument.spreadsheet-template ".as_slice(),
        b"application/vnd.oasis.opendocument.spreadsheet-templat",
        b"application/vnd.oasis.opendocument.text-template",
    ] {
        assert_eq!(sniff(&make_zip(&[("mimetype", mime), ("content.xml", xml.as_bytes())])), Format::Xlsx);
    }
}

#[test]
fn ods_sniff_requires_exact_bounded_mimetype() {
    assert_eq!(sniff(&row("")), Format::Ods);
    assert_eq!(sniff(&make_zip(&[("mimetype", b"application/zip")])), Format::Xlsx);
    assert_eq!(sniff(&make_zip(&[("elsewhere", MIME)])), Format::Xlsx);
    assert_eq!(sniff(&make_zip(&[("mimetype", &vec![b'x'; 1_000_000])])), Format::Xlsx);
    assert_eq!(sniff(b"a,b\n1,2"), Format::Csv);
}

/// A flat ODS (`.fods`) is one XML document, `office:document`, with the spreadsheet mimetype as
/// an attribute and the body after the styles: it reads like a package, whatever its BOM or prolog.
#[test]
fn flat_ods_document_reads_like_a_package() {
    let flat = |mime: &str| {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!-- a comment --><office:document xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" office:version="1.3" office:mimetype="{mime}"><office:styles><text:p>not a cell</text:p></office:styles><office:body><office:spreadsheet><table:table table:name="Flat"><table:table-row><table:table-cell office:value-type="float" office:value="7"><text:p>7</text:p></table:table-cell><table:table-cell office:value-type="string"><text:p>seven</text:p></table:table-cell></table:table-row></table:table></office:spreadsheet></office:body></office:document>"#
        )
    };
    for bytes in [
        flat("application/vnd.oasis.opendocument.spreadsheet").into_bytes(),
        [b"\xEF\xBB\xBF".as_slice(), flat("application/vnd.oasis.opendocument.spreadsheet-template").as_bytes()].concat(),
    ] {
        assert_eq!(sniff(&bytes), Format::Ods);
        let (wb, _) = read_ods(&bytes).unwrap();
        assert_eq!(wb.sheets.len(), 1);
        assert_eq!(wb.sheets[0].name, "Flat");
        assert_eq!(wb.sheets[0].value(CellRef::new(0, 0)), Value::Number(7.0));
        assert_eq!(wb.sheets[0].value(CellRef::new(0, 1)), text("seven"));
    }
    // A flat text document, or a content.xml root on its own, is not a spreadsheet.
    let fodt = flat("application/vnd.oasis.opendocument.text");
    assert_eq!(sniff(fodt.as_bytes()), Format::Csv);
    assert!(read_ods(fodt.as_bytes()).is_err());
    assert_eq!(sniff(content("").as_bytes()), Format::Csv);
    assert_eq!(sniff(b"<html><body>a</body></html>"), Format::Csv);
}

#[test]
fn ods_invalid_sheet_names_are_made_safe_for_xlsx() {
    let long = "A".repeat(50);
    let bytes = ods(&format!(
        r#"<table:table table:name="{long}"/><table:table table:name="{long}"/><table:table table:name="'History'"/><table:table table:name="[]:*?"/>"#
    ));
    let (wb, report) = read_ods(&bytes).unwrap();
    for (i, sheet) in wb.sheets.iter().enumerate() {
        assert!(wb.check_sheet_name(&sheet.name, Some(i)).is_ok());
    }
    assert_eq!(
        wb.sheets.iter().map(|s| s.name.clone()).collect::<Vec<_>>(),
        ["A".repeat(31), format!("{} (2)", "A".repeat(25)), "History (2)".to_owned(), "Sheet4".to_owned()]
    );
    assert!(report.warnings.iter().any(|w| w.contains("sheet names")));
    assert_eq!(read_xlsx(&write_xlsx(&wb).unwrap()).unwrap().0.sheets.len(), 4);
}

#[test]
fn ods_error_cache_is_the_error_and_never_its_placeholder_zero() {
    let bytes = row(
        r#"<table:table-cell xmlns:calcext="urn:org:documentfoundation:names:experimental:calc:xmlns:calcext:1.0" calcext:value-type="error" office:value-type="float" office:value="0" table:formula="of:=1/0"><text:p>#DIV/0!</text:p></table:table-cell>
        <table:table-cell xmlns:calcext="urn:org:documentfoundation:names:experimental:calc:xmlns:calcext:1.0" calcext:value-type="error" office:value-type="float" office:value="0" table:formula="of:#ERR502!"><text:p>Err:502</text:p></table:table-cell>"#,
    );
    let (wb, report) = read_ods(&bytes).unwrap();
    assert_eq!(wb.sheets[0].value(CellRef::new(0, 0)), Value::Error(gridcraft_core::CellError::Div0));
    assert_eq!(wb.sheets[0].cell(CellRef::new(0, 0)).unwrap().formula.as_ref().unwrap().text, "1/0");
    // An application-specific error has no Excel equivalent: its text stays, with a warning.
    assert_eq!(wb.sheets[0].value(CellRef::new(0, 1)), text("Err:502"));
    assert!(wb.sheets[0].cell(CellRef::new(0, 1)).unwrap().formula.is_none());
    assert!(report.warnings.iter().any(|w| w.contains("unsupported or missing")));
}

/// OpenFormula cells become formulas with their saved results: references, separators, arrays,
/// vendor-prefixed functions, named ranges, references to other sheets and array (matrix)
/// formulas. Functions GridCraft doesn't have keep the saved result.
#[test]
fn ods_formulas_and_named_ranges_are_imported() {
    let bytes = ods(r#"<table:table table:name="Data"><table:table-row>
        <table:table-cell office:value-type="float" office:value="2"/>
        <table:table-cell table:formula="of:=[.A1]*2" office:value-type="float" office:value="4"/>
        <table:table-cell table:formula="of:=SUM([.A1:.B1]; 1)" office:value-type="float" office:value="7"/>
        <table:table-cell table:formula="of:=ORG.LIBREOFFICE.ROUNDSIG(1.234;2)" office:value-type="float" office:value="1.2"/>
        <table:table-cell table:formula="of:=COM.MICROSOFT.XLOOKUP(2;[.A1:.C1];[.A1:.C1])" office:value-type="float" office:value="2"/>
        <table:table-cell table:formula="of:={1;2}" table:number-matrix-columns-spanned="2" table:number-matrix-rows-spanned="1" office:value-type="float" office:value="1"/>
        <table:table-cell office:value-type="float" office:value="2"/>
        <table:table-cell table:formula="of:=Two*3" office:value-type="float" office:value="6"/>
        <table:table-cell table:formula="of:=Nowhere+1" office:value-type="float" office:value="9"/>
        <table:table-cell table:formula="of:=NOW()"/>
      </table:table-row>
      <table:named-expressions><table:named-range table:name="Two" table:base-cell-address="$Data.$A$1" table:cell-range-address="$Data.$A$1"/></table:named-expressions>
    </table:table>
    <table:table table:name="Other"><table:table-row><table:table-cell table:formula="of:=['Data'.A1]+[$Data.$B$1]" office:value-type="float" office:value="6"/></table:table-row></table:table>"#);
    let (wb, report) = read_ods(&bytes).unwrap();
    let data = &wb.sheets[0];
    let formula = |s: usize, c: u32| wb.sheets[s].cell(CellRef::new(0, c)).and_then(|c| c.formula.as_ref()).map(|f| f.text.clone());
    assert_eq!(formula(0, 1).as_deref(), Some("A1*2"));
    assert_eq!(data.value(CellRef::new(0, 1)), Value::Number(4.0));
    assert_eq!(formula(0, 2).as_deref(), Some("SUM(A1:B1,1)"));
    assert_eq!(formula(0, 3), None, "an application-specific function keeps its saved result");
    assert_eq!(data.value(CellRef::new(0, 3)), Value::Number(1.2));
    assert_eq!(formula(0, 4).as_deref(), Some("XLOOKUP(2,A1:C1,A1:C1)"));
    let array = data.cell(CellRef::new(0, 5)).and_then(|c| c.formula.clone()).unwrap();
    assert_eq!(array.text, "{1,2}");
    assert_eq!(array.array, Some(RangeRef::new(CellRef::new(0, 5), CellRef::new(0, 6))));
    assert_eq!(data.value(CellRef::new(0, 6)), Value::Empty, "the array's other cells are left to the formula");
    assert!(wb.names.iter().any(|n| n.name == "Two" && n.scope == Some(0) && n.formula.contains("$A$1")), "{:?}", wb.names);
    assert_eq!(formula(0, 7).as_deref(), Some("Two*3"));
    assert_eq!(formula(0, 8), None, "a name the file doesn't define keeps the saved result");
    assert_eq!(formula(0, 9).as_deref(), Some("NOW()"));
    assert_eq!(data.value(CellRef::new(0, 9)), Value::Empty, "no saved result: no placeholder once the formula imports");
    assert_eq!(formula(1, 0).as_deref(), Some("Data!A1+Data!$B$1"));
    assert!(report.warnings.iter().any(|w| w.contains("kept their saved results")), "{:?}", report.warnings);
    assert!(!report.warnings.iter().any(|w| w.contains("unsupported or missing")), "{:?}", report.warnings);
}

/// Names and formulas hold the model's own text, the one an XLSX reader gives back once the
/// workbook is saved: no quotes around a plain sheet name, no `_xlpm.` on LET parameters.
#[test]
fn ods_names_and_formulas_read_as_their_saved_xlsx_reads() {
    let bytes = ods(r#"<table:table table:name="Data"><table:table-row>
        <table:table-cell table:formula="of:=COM.MICROSOFT.LET(_xlpm.x; 1; _xlpm.y; _xlpm.x+_xlpm.y)" office:value-type="float" office:value="2"/>
      </table:table-row></table:table>
    <table:table table:name="My Data"><table:table-row><table:table-cell office:value-type="float" office:value="1"/></table:table-row></table:table>
    <table:named-expressions>
      <table:named-range table:name="Edge" table:base-cell-address="$Data.$A$1" table:cell-range-address="$Data.$A$1:.$B$2"/>
      <table:named-range table:name="Spaced" table:base-cell-address="$Data.$A$1" table:cell-range-address="$'My Data'.$A$1"/>
    </table:named-expressions>"#);
    let (wb, _) = read_ods(&bytes).unwrap();
    let name = |n: &str| wb.names.iter().find(|d| d.name == n).map(|d| d.formula.clone());
    assert_eq!(name("Edge").as_deref(), Some("Data!$A$1:$B$2"));
    assert_eq!(name("Spaced").as_deref(), Some("'My Data'!$A$1"));
    let let_text = wb.sheets[0].cell(CellRef::new(0, 0)).and_then(|c| c.formula.as_ref()).map(|f| f.text.clone());
    assert_eq!(let_text.as_deref(), Some("LET(x,1,y,x+y)"));
    let (back, _) = read_xlsx(&write_xlsx(&wb).unwrap()).unwrap();
    let names = |w: &gridcraft_model::Workbook| w.names.iter().map(|d| (d.name.clone(), d.formula.clone())).collect::<Vec<_>>();
    assert_eq!(names(&back), names(&wb));
    assert_eq!(back.sheets[0].cell(CellRef::new(0, 0)).and_then(|c| c.formula.as_ref()).map(|f| f.text.clone()), let_text);
}

/// Excel's built-in names a LibreOffice document keeps are what an XLSX read makes of them: a
/// sheet's print area and titles, and the autofilter's database range, not names.
#[test]
fn ods_excel_built_in_names_are_print_settings() {
    let bytes =
        ods(r#"<table:table table:name="Data"><table:table-row><table:table-cell office:value-type="float" office:value="1"/></table:table-row>
      <table:named-expressions>
        <table:named-range table:name="_xlnm.Print_Area" table:base-cell-address="$Data.$A$1" table:cell-range-address="$Data.$A$1:.$C$33"/>
        <table:named-range table:name="_xlnm.Print_Titles" table:base-cell-address="$Data.$A$1" table:cell-range-address="$Data.$A$1:.$AMJ$2"/>
        <table:named-range table:name="_xlnm._FilterDatabase" table:base-cell-address="$Data.$A$1" table:cell-range-address="$Data.$A$1:.$C$9"/>
      </table:named-expressions>
    </table:table>"#);
    let (wb, _) = read_ods(&bytes).unwrap();
    assert!(wb.names.is_empty(), "{:?}", wb.names);
    assert_eq!(wb.sheets[0].print.print_area, RangeRef::parse("A1:C33"));
}

#[test]
fn ods_blank_repeats_past_the_sheet_edge_are_clipped() {
    // Writers with larger sheets pad to their own edge; blank padding past ours carries no data.
    let bytes = ods(r#"<table:table table:name="Data">
        <table:table-row><table:table-cell office:value-type="float" office:value="1"/><table:table-cell table:number-columns-repeated="4294967295"/><table:table-cell/></table:table-row>
        <table:table-row table:number-rows-repeated="16777215"><table:table-cell table:number-columns-repeated="16777216"/></table:table-row>
        <table:table-row table:number-rows-repeated="5"><table:table-cell/></table:table-row>
      </table:table>"#);
    let (wb, _) = read_ods(&bytes).unwrap();
    assert_eq!(wb.sheets[0].cells.len(), 1);
    assert_eq!(wb.sheets[0].value(CellRef::new(0, 0)), Value::Number(1.0));
    // Data after the clipped edge is dropped with a warning, never silently (see
    // `ods_cells_past_the_sheet_edge_are_dropped_with_a_warning`).
    let late = ods(
        r#"<table:table table:name="Data"><table:table-row table:number-rows-repeated="1048576"><table:table-cell/></table:table-row><table:table-row><table:table-cell office:value-type="float" office:value="1"/></table:table-row></table:table>"#,
    );
    assert!(read_ods(&late).unwrap().1.warnings.iter().any(|w| w.contains("beyond the last row or column")));
}

/// The caller's known-function predicate decides which functions GridCraft calculates: a formula
/// calling one it doesn't keeps its saved result instead of evaluating to #NAME?.
#[test]
fn ods_formulas_with_functions_the_caller_doesnt_know_keep_their_saved_results() {
    let bytes = row(r#"<table:table-cell table:formula="of:=FORMULA([.B1])" office:value-type="string" office:string-value="=1+1"/>
        <table:table-cell table:formula="of:=1+1" office:value-type="float" office:value="2"/>
        <table:table-cell table:formula="of:=NOTAFUNCTION(1)" office:value-type="float" office:value="3"/>"#);
    let known = |name: &str| name != "NOTAFUNCTION";
    let (wb, _) =
        crate::read_ods_with(&bytes, (crate::FunctionInfo { is_builtin: |_| false, ..Default::default() }, &known as &dyn Fn(&str) -> bool)).unwrap();
    let formula = |c: u32| wb.sheets[0].cell(CellRef::new(0, c)).and_then(|c| c.formula.as_ref()).map(|f| f.text.clone());
    assert_eq!(formula(0).as_deref(), Some("FORMULATEXT(B1)"), "the predicate, not is_builtin, decides");
    assert_eq!(formula(2), None);
    assert_eq!(wb.sheets[0].value(CellRef::new(0, 2)), Value::Number(3.0));
}

/// Criteria and lookups match text as the document's calculation settings say (ODF 1.3 part 3,
/// 19.646 table 16: regular expressions unless the document turns them off, no wildcards unless
/// it turns them on, case told apart unless it says not, 19.594). A formula whose matching could
/// differ from Excel's keeps its saved result; one whose criteria mean the same either way is
/// imported.
#[test]
fn ods_criteria_follow_the_documents_matching_settings() {
    let cells = r#"<table:table table:name="Data"><table:table-row>
        <table:table-cell office:value-type="string" office:string-value="abc"/>
        <table:table-cell table:formula="of:=COUNTIF([.A1];&quot;a.*&quot;)" office:value-type="float" office:value="1"/>
        <table:table-cell table:formula="of:=COUNTIF([.A1];[.A1])" office:value-type="float" office:value="1"/>
        <table:table-cell table:formula="of:=COUNTIF([.A1];&quot;abc&quot;)+SUMIF([.A1];&quot;&gt;2&quot;)+MATCH(3;{1;2;3};0)" office:value-type="float" office:value="4"/>
        <table:table-cell table:formula="of:=SEARCH(&quot;b*&quot;;[.A1])" office:value-type="float" office:value="2"/>
      </table:table-row></table:table>"#;
    let read = |settings: &str| {
        let (wb, report) = read_ods(&ods(&format!("{settings}{cells}"))).unwrap();
        let imported: Vec<bool> = (1..5).map(|c| wb.sheets[0].cell(CellRef::new(0, c)).is_some_and(|c| c.formula.is_some())).collect();
        (imported, wb.sheets[0].value(CellRef::new(0, 1)), report.warnings.iter().any(|w| w.contains("regular expressions")))
    };
    let regex = r#"<table:calculation-settings table:use-regular-expressions="true" table:use-wildcards="false" table:case-sensitive="false"/>"#;
    assert_eq!(read(regex), (vec![false, false, true, false], Value::Number(1.0), true));
    // The defaults are regular expressions, no wildcards, and case-sensitive text criteria.
    let defaults = (vec![false, false, false, false], Value::Number(1.0), true);
    assert_eq!(read(""), defaults);
    assert_eq!(read("<table:calculation-settings/>"), defaults);
    // Wildcards without regular expressions, criteria against the whole cell, case ignored:
    // Excel's matching.
    let excel = r#"<table:calculation-settings table:use-regular-expressions="false" table:use-wildcards="true" table:case-sensitive="false"/>"#;
    assert_eq!(read(excel), (vec![true, true, true, true], Value::Number(1.0), false));
    // Case-sensitive matching changes a criterion with letters (or one the formula computes),
    // not SEARCH's text, which ignores case.
    let case = r#"<table:calculation-settings table:use-regular-expressions="false" table:use-wildcards="true"/>"#;
    assert_eq!(read(case), (vec![false, false, false, true], Value::Number(1.0), false));
    // Partial-cell matching changes any text criterion; numbers and ordered comparisons stay.
    let partial = r#"<table:calculation-settings table:use-regular-expressions="false" table:use-wildcards="true" table:search-criteria-must-apply-to-whole-cell="false" table:case-sensitive="false"/>"#;
    assert_eq!(read(partial).0, vec![false, false, false, true]);
}

/// `#REF!` inside a quoted sheet name is part of a valid sheet name (ODF 1.3 part 4, 5.8), so the
/// reference stays a reference to that sheet; only an unquoted `#REF!` is a broken reference.
#[test]
fn ods_sheet_names_containing_ref_errors_are_references() {
    let bytes = ods(r#"<table:table table:name="Sheet1"><table:table-row>
        <table:table-cell table:formula="of:=SUM(['Budget#REF!'.A1])" office:value-type="float" office:value="5"/>
        <table:table-cell table:formula="of:=[$#REF!.A1]" office:value-type="string"><text:p>#REF!</text:p></table:table-cell>
      </table:table-row></table:table>
    <table:table table:name="Budget#REF!"><table:table-row><table:table-cell office:value-type="float" office:value="5"/></table:table-row></table:table>"#);
    let (wb, _) = read_ods(&bytes).unwrap();
    let formula = |c: u32| wb.sheets[0].cell(CellRef::new(0, c)).and_then(|c| c.formula.clone()).unwrap();
    assert_eq!(wb.sheets[1].name, "Budget#REF!");
    assert_eq!(formula(0).text, "SUM('Budget#REF!'!A1)");
    let mut refs = 0;
    formula(0).expr.as_deref().unwrap().walk(&mut |e| refs += usize::from(matches!(e, gridcraft_formula::Expr::Ref(_))));
    assert_eq!(refs, 1, "a reference to the sheet, not an error constant");
    assert_eq!(formula(1).text, "#REF!");
}

/// A DataPilot report is not imported: its cells are plain cells and its area is kept on its
/// sheet as a PivotTable GridCraft did not import (quoted sheet names included).
#[test]
fn data_pilot_report_areas_are_kept_as_unimported_pivots() {
    let cell =
        r#"<table:table-row><table:table-cell office:value-type="float" office:value="1"><text:p>1</text:p></table:table-cell></table:table-row>"#;
    let doc = ods(&format!(
        r#"<table:table table:name="Data">{cell}</table:table><table:table table:name="My Sheet">{cell}</table:table><table:data-pilot-tables><table:data-pilot-table table:name="DataPilot1" table:target-range-address="'My Sheet'.M1:'My Sheet'.O7"/><table:data-pilot-table table:name="DataPilot2" table:target-range-address="Data.$B$2:.$C$5"/><table:data-pilot-table table:name="Bad" table:target-range-address="Data.B2"/></table:data-pilot-tables>"#
    ));
    let (wb, _) = read_ods(&doc).unwrap();
    let areas = |i: usize| wb.sheets[i].unimported_pivots.iter().map(|r| r.a1()).collect::<Vec<_>>();
    assert_eq!((areas(0), areas(1)), (vec!["B2:C5".to_string()], vec!["M1:O7".to_string()]));
    assert_eq!(wb.sheets[0].value(CellRef::new(0, 0)), Value::Number(1.0));
}

/// A named range whose name reads as a cell in today's grid (`TAX2020`, `Q1`) or is otherwise one
/// Excel can't hold is renamed (a `_` in front, as the `.xls` and XLSB readers do), and every
/// formula that uses it, `INDIRECT` text included, reads the renamed name: left as `TAX2020` it
/// would read the empty cell of that address and recalculate to 0 where the document says 10.
#[test]
fn ods_names_that_read_as_cells_are_renamed_and_formulas_follow() {
    let bytes = ods(r#"<table:table table:name="Data"><table:table-row>
        <table:table-cell office:value-type="float" office:value="5"/>
        <table:table-cell table:formula="of:=TAX2020*2" office:value-type="float" office:value="10"/>
        <table:table-cell table:formula="of:=SUM(tax2020;1)" office:value-type="float" office:value="6"/>
        <table:table-cell table:formula="of:=INDIRECT(&quot;TAX2020&quot;)" office:value-type="float" office:value="5"/>
        <table:table-cell table:formula="of:=INDIRECT(&quot;TAX2020&quot;;0)" office:value-type="float" office:value="7"/>
        <table:table-cell table:formula="of:=Q1*2" office:value-type="float" office:value="8"/>
        <table:table-cell table:formula="of:=A1+1" office:value-type="float" office:value="6"/>
      </table:table-row>
      <table:named-expressions><table:named-range table:name="Q1" table:base-cell-address="$Data.$A$1" table:cell-range-address="$Data.$A$1"/></table:named-expressions>
    </table:table>
    <table:named-expressions>
      <table:named-range table:name="TAX2020" table:base-cell-address="$Data.$A$1" table:cell-range-address="$Data.$A$1"/>
      <table:named-range table:name="_TAX2020" table:base-cell-address="$Data.$A$1" table:cell-range-address="$Data.$B$1"/>
    </table:named-expressions>"#);
    let (wb, report) = read_ods(&bytes).unwrap();
    let defined = |n: &str, scope: Option<usize>| wb.names.iter().find(|d| d.name == n && d.scope == scope).map(|d| d.formula.clone());
    // `_TAX2020` is taken by a name Excel accepts, so TAX2020 takes two underscores.
    assert_eq!(defined("__TAX2020", None).as_deref(), Some("Data!$A$1"));
    assert_eq!(defined("_TAX2020", None).as_deref(), Some("Data!$B$1"));
    assert_eq!(defined("_Q1", Some(0)).as_deref(), Some("Data!$A$1"));
    assert!(wb.names.iter().all(|d| gridcraft_core::valid_defined_name(&d.name)), "{:?}", wb.names);
    let formula = |c: u32| wb.sheets[0].cell(CellRef::new(0, c)).and_then(|c| c.formula.as_ref()).map(|f| f.text.clone());
    assert_eq!(formula(1).as_deref(), Some("__TAX2020*2"));
    assert_eq!(formula(2).as_deref(), Some("SUM(__TAX2020,1)"), "names are read without regard to case");
    assert_eq!(formula(3).as_deref(), Some("INDIRECT(\"__TAX2020\")"));
    assert_eq!(formula(4), None, "R1C1 INDIRECT text reads a cell, not the renamed name: the saved result stays");
    assert_eq!(wb.sheets[0].value(CellRef::new(0, 4)), Value::Number(7.0));
    assert_eq!(formula(5).as_deref(), Some("_Q1*2"));
    assert_eq!(formula(6).as_deref(), Some("A1+1"), "a bare cell address no name defines is the cell, as LibreOffice reads it");
    assert_eq!(wb.sheets[0].value(CellRef::new(0, 6)), Value::Number(6.0));
    assert!(report.warnings.iter().any(|w| w.contains("renamed")), "{:?}", report.warnings);
    // The saved workbook defines and uses one spelling.
    let saved = write_xlsx(&wb).unwrap();
    let (back, _) = read_xlsx(&saved).unwrap();
    assert!(back.names.iter().any(|d| d.name == "__TAX2020"));
    assert_eq!(back.sheets[0].cell(CellRef::new(0, 1)).and_then(|c| c.formula.as_ref()).map(|f| f.text.as_str()), Some("__TAX2020*2"));
}

/// Names with spaces, hyphens, slashes or a leading digit, and R1C1 spellings (`R`, `RC`, `R1C1`),
/// are names LibreOffice allows and Excel doesn't: each is renamed to a valid, unique name before
/// it reaches the XLSX, and formulas follow, `INDIRECT` text with a sheet in front included.
#[test]
fn ods_names_excel_forbids_are_renamed_before_they_reach_xlsx() {
    let bytes = ods(r#"<table:table table:name="Data"><table:table-row>
        <table:table-cell office:value-type="float" office:value="3"/>
        <table:table-cell table:formula="of:=[.A1]+my_name" office:value-type="float" office:value="6"/>
        <table:table-cell table:formula="of:=INDIRECT(&quot;my name&quot;)" office:value-type="float" office:value="3"/>
        <table:table-cell table:formula="of:=R+Fine.Name" office:value-type="float" office:value="6"/>
        <table:table-cell table:formula="of:=INDIRECT(&quot;Data!1st&quot;)" office:value-type="float" office:value="3"/>
        <table:table-cell table:formula="of:=RC+R1C1" office:value-type="float" office:value="6"/>
      </table:table-row></table:table>
    <table:named-expressions>
      <table:named-range table:name="my name" table:base-cell-address="$Data.$A$1" table:cell-range-address="$Data.$A$1"/>
      <table:named-range table:name="my_name" table:base-cell-address="$Data.$A$1" table:cell-range-address="$Data.$A$1"/>
      <table:named-range table:name="1st" table:base-cell-address="$Data.$A$1" table:cell-range-address="$Data.$A$1"/>
      <table:named-range table:name="a-b" table:base-cell-address="$Data.$A$1" table:cell-range-address="$Data.$A$1"/>
      <table:named-range table:name="a/b" table:base-cell-address="$Data.$A$1" table:cell-range-address="$Data.$A$1"/>
      <table:named-range table:name="R" table:base-cell-address="$Data.$A$1" table:cell-range-address="$Data.$A$1"/>
      <table:named-range table:name="RC" table:base-cell-address="$Data.$A$1" table:cell-range-address="$Data.$A$1"/>
      <table:named-range table:name="R1C1" table:base-cell-address="$Data.$A$1" table:cell-range-address="$Data.$A$1"/>
      <table:named-range table:name="R1x" table:base-cell-address="$Data.$A$1" table:cell-range-address="$Data.$A$1"/>
      <table:named-range table:name="RCx" table:base-cell-address="$Data.$A$1" table:cell-range-address="$Data.$A$1"/>
      <table:named-range table:name="Fine.Name" table:base-cell-address="$Data.$A$1" table:cell-range-address="$Data.$A$1"/>
    </table:named-expressions>"#);
    let (wb, _) = read_ods(&bytes).unwrap();
    let names: Vec<&str> = wb.names.iter().map(|d| d.name.as_str()).collect();
    let mut seen = std::collections::HashSet::new();
    for d in &wb.names {
        assert!(gridcraft_core::valid_defined_name(&d.name), "{:?} is a name Excel holds", d.name);
        assert!(seen.insert(d.name.to_lowercase()), "{:?} is unique", d.name);
    }
    assert_eq!(wb.names.len(), 11);
    // Live Excel 16.115 refuses `R1x` (R1C1 notation then a letter) and takes `RCx`.
    for want in ["_my_name", "my_name", "_1st", "a_b", "_a_b", "_R", "_RC", "_R1C1", "_R1x", "RCx", "Fine.Name"] {
        assert!(names.contains(&want), "{want} in {names:?}");
    }
    let formula = |c: u32| wb.sheets[0].cell(CellRef::new(0, c)).and_then(|c| c.formula.as_ref()).map(|f| f.text.clone());
    assert_eq!(formula(1).as_deref(), Some("A1+my_name"));
    assert_eq!(formula(2).as_deref(), Some("INDIRECT(\"_my_name\")"));
    assert_eq!(formula(3).as_deref(), Some("_R+Fine.Name"));
    // `1st` is a workbook-level name here, and LibreOffice finds only a sheet's own names behind a
    // sheet in INDIRECT text (`Data!1st` is #REF!, where Excel finds the name): the saved result stays.
    assert_eq!(formula(4), None);
    assert_eq!(formula(5).as_deref(), Some("_RC+_R1C1"));
    // What reaches the XLSX holds only names Excel accepts.
    let saved = write_xlsx(&wb).unwrap();
    let mut pkg = crate::package::Package::open(&saved).unwrap();
    let book = pkg.read_xml("xl/workbook.xml").unwrap().unwrap();
    let written: Vec<&str> = book.child("definedNames").unwrap().kids("definedName").filter_map(|n| n.attr("name")).collect();
    for bad in ["my name", "1st", "a-b", "a/b", "R", "RC", "R1C1"] {
        assert!(!written.contains(&bad), "{bad} in {written:?}");
    }
    assert!(written.contains(&"_my_name") && written.contains(&"_R1C1"), "{written:?}");
}
