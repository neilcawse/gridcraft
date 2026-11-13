//! ODS formatting import: documents written in Rust, not files from another application.

use gridcraft_core::{CellRef, Value};
use gridcraft_model::{BorderStyle, Color, HAlign, Underline, VAlign, VertAlign, Visibility, Workbook};

use super::make_zip;
use crate::{read_ods, write_xlsx};

const MIME: &[u8] = b"application/vnd.oasis.opendocument.spreadsheet";
const NS: &str = r#"xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0" xmlns:number="urn:oasis:names:tc:opendocument:xmlns:datastyle:1.0" xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0" xmlns:loext="urn:org:documentfoundation:names:experimental:office:xmlns:loext:1.0" xmlns:tableooo="http://openoffice.org/2009/table""#;

/// A package whose styles.xml holds `common` (inside `office:styles`) and whose content.xml holds
/// `automatic` styles and the `body` of one spreadsheet.
fn package(common: &str, automatic: &str, body: &str) -> Vec<u8> {
    let styles = format!(
        r#"<?xml version="1.0"?><office:document-styles {NS}><office:font-face-decls><style:font-face style:name="Liberation Sans" svg:font-family="'Liberation Sans'"/></office:font-face-decls><office:styles>{common}</office:styles><office:automatic-styles><style:style style:name="ce1" style:family="table-cell"><style:text-properties fo:font-weight="bold"/></style:style></office:automatic-styles></office:document-styles>"#
    );
    let content = format!(
        r#"<?xml version="1.0"?><office:document-content {NS}><office:automatic-styles>{automatic}</office:automatic-styles><office:body><office:spreadsheet>{body}</office:spreadsheet></office:body></office:document-content>"#
    );
    make_zip(&[("mimetype", MIME), ("styles.xml", styles.as_bytes()), ("content.xml", content.as_bytes())])
}

fn read(common: &str, automatic: &str, body: &str) -> Workbook {
    match read_ods(&package(common, automatic, body)) {
        Ok((wb, _)) => wb,
        Err(e) => panic!("ODS read failed: {e}"),
    }
}

fn float(style: &str, v: f64) -> String {
    format!(r#"<table:table-cell table:style-name="{style}" office:value-type="float" office:value="{v}"/>"#)
}

fn table(rows: &str) -> String {
    format!(r#"<table:table table:name="Data">{rows}</table:table>"#)
}

fn code(wb: &Workbook, at: &str) -> String {
    let sheet = &wb.sheets[0];
    let pos = CellRef::parse(at).unwrap_or_default();
    wb.styles.get(sheet.style_id(pos)).num_fmt.as_str().to_string()
}

const DEFAULT: &str = r#"<style:default-style style:family="table-cell"><style:text-properties style:font-name="Liberation Sans" fo:font-size="10pt"/></style:default-style><style:style style:name="Default" style:family="table-cell"/>"#;

/// Data styles become Excel number format codes: digits, grouping, conditional sections with
/// their colour, percentages, currencies, scientific and fractions, scaling, embedded and
/// literal text, dates, elapsed times, and text formats.
#[test]
fn ods_data_styles_become_number_format_codes() {
    let data = r##"
        <number:number-style style:name="N2P0" style:volatile="true"><number:number number:decimal-places="2" loext:min-decimal-places="2" number:min-integer-digits="1" number:grouping="true"/></number:number-style>
        <number:number-style style:name="N2"><style:text-properties fo:color="#ff0000"/><number:text>-</number:text><number:number number:decimal-places="2" loext:min-decimal-places="2" number:min-integer-digits="1" number:grouping="true"/><style:map style:condition="value()&gt;=0" style:apply-style-name="N2P0"/></number:number-style>
        <number:percentage-style style:name="N3"><number:number number:decimal-places="1" number:min-integer-digits="1"/><number:text>%</number:text></number:percentage-style>
        <number:currency-style style:name="N4"><number:number number:decimal-places="2" number:min-integer-digits="1" number:grouping="true"/><number:text> </number:text><number:currency-symbol number:language="de" number:country="DE">€</number:currency-symbol></number:currency-style>
        <number:number-style style:name="N5"><number:scientific-number number:decimal-places="2" number:min-integer-digits="1" number:min-exponent-digits="2"/></number:number-style>
        <number:number-style style:name="N6"><number:fraction number:min-integer-digits="0" number:min-numerator-digits="1" number:min-denominator-digits="2"/></number:number-style>
        <number:number-style style:name="N7"><number:number number:decimal-places="0" number:min-integer-digits="1" number:display-factor="1000"/><number:text> k€</number:text></number:number-style>
        <number:number-style style:name="N8"><number:number number:decimal-places="0" number:min-integer-digits="6"><number:embedded-text number:position="3">-</number:embedded-text></number:number></number:number-style>
        <number:date-style style:name="N9"><number:day number:style="long"/><number:text>.</number:text><number:month number:style="long"/><number:text>.</number:text><number:year number:style="long"/></number:date-style>
        <number:time-style style:name="N10" number:truncate-on-overflow="false"><number:hours number:style="long"/><number:text>:</number:text><number:minutes number:style="long"/><number:text>:</number:text><number:seconds number:style="long" number:decimal-places="2"/></number:time-style>
        <number:date-style style:name="N11"><number:day-of-week number:style="long"/><number:text>, </number:text><number:month number:textual="true"/><number:text> </number:text><number:day/><number:text> </number:text><number:hours/><number:text>:</number:text><number:minutes number:style="long"/><number:text> </number:text><number:am-pm/></number:date-style>
        <number:text-style style:name="N12"><number:text>Name: </number:text><number:text-content/></number:text-style>
        <number:number-style style:name="N13"><number:number number:min-integer-digits="1"/></number:number-style>
        <number:number-style style:name="N14P0"><number:number number:decimal-places="0" number:min-integer-digits="1"/></number:number-style>
        <number:number-style style:name="N14P1"><number:text>(</number:text><number:number number:decimal-places="0" number:min-integer-digits="1"/><number:text>)</number:text></number:number-style>
        <number:number-style style:name="N14"><number:text>zero</number:text><style:map style:condition="value()&gt;0" style:apply-style-name="N14P0"/><style:map style:condition="value()&lt;0" style:apply-style-name="N14P1"/></number:number-style>
    "##;
    let cells: String =
        (2..=14).map(|n| format!(r#"<style:style style:name="c{n}" style:family="table-cell" style:data-style-name="N{n}"/>"#)).collect();
    let row: String = (2..=14).map(|n| float(&format!("c{n}"), 1.0)).collect();
    let wb = read(DEFAULT, &format!("{data}{cells}"), &table(&format!("<table:table-row>{row}</table:table-row>")));
    let expected = [
        ("A1", "#,##0.00;[Red]-#,##0.00"),
        ("B1", "0.0%"),
        ("C1", "#,##0.00 [$€]"),
        ("D1", "0.00E+00"),
        ("E1", "# ?/??"),
        ("F1", "0, \"k€\""),
        ("G1", "000-000"),
        ("H1", "dd\".\"mm\".\"yyyy"),
        ("I1", "[hh]:mm:ss.00"),
        ("J1", "dddd, mmm d h:mm AM/PM"),
        ("K1", "\"Name\": @"),
        ("L1", "General"),
        ("M1", "0;(0);\"zero\""),
    ];
    let got: Vec<(&str, String)> = expected.iter().map(|(at, _)| (*at, code(&wb, at))).collect();
    let want: Vec<(&str, String)> = expected.iter().map(|(at, w)| (*at, (*w).to_string())).collect();
    assert_eq!(got, want);
}

/// A map condition's namespace prefix is resolved (ODF 1.3 part 3 §19.472): bound to OpenFormula
/// it is the same condition as one without a prefix; in another namespace it is not one
/// GridCraft recognises, and its map is ignored.
#[test]
fn ods_map_conditions_resolve_their_namespace_prefix() {
    let style = |name: &str, decl: &str, cond: &str| {
        format!(
            r#"<number:number-style style:name="{name}" {decl}><number:text>-</number:text><number:number number:decimal-places="0" number:min-integer-digits="1"/><style:map style:condition="{cond}" style:apply-style-name="P"/></number:number-style>"#
        )
    };
    let data = [
        r#"<number:number-style style:name="P"><number:number number:decimal-places="0" number:min-integer-digits="1"/></number:number-style>"#.to_string(),
        style("Bare", "", "value()&gt;=0"),
        style("Of", r#"xmlns:f="urn:oasis:names:tc:opendocument:xmlns:of:1.2""#, "f:value()&gt;=0"),
        style("Other", r#"xmlns:x="urn:example:other""#, "x:value()&gt;=0"),
        r#"<style:style style:name="c1" style:family="table-cell" style:data-style-name="Bare"/><style:style style:name="c2" style:family="table-cell" style:data-style-name="Of"/><style:style style:name="c3" style:family="table-cell" style:data-style-name="Other"/>"#.to_string(),
    ]
    .concat();
    let wb =
        read(DEFAULT, &data, &table(&format!("<table:table-row>{}{}{}</table:table-row>", float("c1", 1.0), float("c2", 1.0), float("c3", 1.0))));
    assert_eq!((code(&wb, "A1"), code(&wb, "B1"), code(&wb, "C1")), ("0;-0".to_string(), "0;-0".to_string(), "-0".to_string()));
}

/// A common style names a data style of its own file: styles.xml may keep it among its automatic
/// styles (ODF 1.3 part 3 §3.15.3, §16.1), and the content's automatic data style of the same
/// name is another style. The other automatic styles of styles.xml never format cells.
#[test]
fn ods_common_styles_use_the_data_styles_of_styles_xml() {
    let styles = format!(
        r#"<?xml version="1.0"?><office:document-styles {NS}><office:styles>{DEFAULT}<style:style style:name="CommonPercent" style:family="table-cell" style:parent-style-name="Default" style:data-style-name="N123"/></office:styles><office:automatic-styles><number:percentage-style style:name="N123"><number:number number:decimal-places="0" number:min-integer-digits="1"/><number:text>%</number:text></number:percentage-style><style:style style:name="PageOnly" style:family="table-cell" style:data-style-name="N123"/></office:automatic-styles></office:document-styles>"#
    );
    let content = format!(
        r#"<?xml version="1.0"?><office:document-content {NS}><office:automatic-styles><number:number-style style:name="N123"><number:number number:decimal-places="2" number:min-integer-digits="1"/></number:number-style><style:style style:name="ce1" style:family="table-cell" style:data-style-name="N123"/></office:automatic-styles><office:body><office:spreadsheet>{}</office:spreadsheet></office:body></office:document-content>"#,
        table(&format!("<table:table-row>{}{}{}</table:table-row>", float("CommonPercent", 0.5), float("ce1", 0.5), float("PageOnly", 0.5)))
    );
    let ods = make_zip(&[("mimetype", MIME), ("styles.xml", styles.as_bytes()), ("content.xml", content.as_bytes())]);
    let (wb, _) = read_ods(&ods).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((code(&wb, "A1"), code(&wb, "B1"), code(&wb, "C1")), ("0%".to_string(), "0.00".to_string(), "General".to_string()));
}

/// Cell styles: the document's default cell style is the Normal style, common styles inherit
/// from their parents, and fonts, fills, borders, alignment, wrapping, rotation and protection
/// are read; the formatting survives saving as XLSX.
#[test]
fn ods_cell_styles_read_fonts_fills_borders_alignment_and_protection() {
    let common = format!(
        r##"{DEFAULT}<style:style style:name="Heading" style:family="table-cell" style:parent-style-name="Default"><style:text-properties fo:font-size="14pt" fo:font-style="italic" fo:color="#1f497d"/></style:style><style:style style:name="Boxed" style:family="table-cell" style:parent-style-name="Default"><style:table-cell-properties fo:background-color="#ffff00" fo:border="0.74pt solid #000000" style:text-align-source="fix"/><style:paragraph-properties fo:text-align="center"/></style:style>"##
    );
    let automatic = r##"
        <style:style style:name="ce1" style:family="table-cell" style:parent-style-name="Heading"><style:text-properties fo:font-weight="bold" style:text-underline-style="solid" style:text-underline-type="double"/></style:style>
        <style:style style:name="ce2" style:family="table-cell" style:parent-style-name="Default"><style:table-cell-properties fo:background-color="#ffff00" fo:border="0.74pt solid #000000" fo:border-bottom="2.5pt solid #ff0000" fo:wrap-option="wrap" style:vertical-align="middle" style:rotation-angle="45" style:text-align-source="fix" style:cell-protect="none"/><style:paragraph-properties fo:text-align="center"/><style:text-properties style:text-line-through-style="solid" style:text-position="super 58%"/></style:style>
        <style:style style:name="ce3" style:family="table-cell" style:parent-style-name="Boxed"><style:table-cell-properties style:text-align-source="value-type" fo:background-color="transparent" style:cell-protect="formula-hidden protected"/></style:style>
    "##;
    let row = format!("<table:table-row>{}{}{}{}</table:table-row>", float("ce1", 1.0), float("ce2", 2.0), float("ce3", 3.0), float("Default", 4.0));
    let wb = read(&common, automatic, &table(&row));
    let sheet = &wb.sheets[0];
    let style = |at: &str| wb.styles.get(sheet.style_id(CellRef::parse(at).unwrap_or_default())).clone();
    let normal = wb.styles.get(gridcraft_model::StyleId::DEFAULT);
    assert_eq!((normal.font.name.as_str(), normal.font.size), ("Liberation Sans", 10.0), "the default cell style is Normal");
    assert_eq!(sheet.style_id(CellRef::new(0, 3)), gridcraft_model::StyleId::DEFAULT, "the Default style is Normal");
    let a = style("A1");
    assert_eq!((a.font.name.as_str(), a.font.size, a.font.bold, a.font.italic), ("Liberation Sans", 14.0, true, true));
    assert_eq!((a.font.color, a.font.underline), (Color::Rgb(0x1F497D), Underline::Double));
    let b = style("B1");
    assert_eq!(
        (b.fill.fg, b.borders.left.style, b.borders.bottom.style, b.borders.bottom.color),
        (Color::Rgb(0xFFFF00), BorderStyle::Thin, BorderStyle::Thick, Color::Rgb(0xFF0000))
    );
    assert_eq!((b.align.h, b.align.v, b.align.wrap, b.align.rotation), (HAlign::Center, VAlign::Center, true, 45));
    assert_eq!((b.font.strike, b.font.vert, b.protection.locked), (true, VertAlign::Superscript, false));
    let c = style("C1");
    assert_eq!(
        (c.align.h, c.fill.pattern, c.borders.top.style),
        (HAlign::General, gridcraft_model::PatternType::None, BorderStyle::Thin),
        "a child overrides and inherits"
    );
    assert!(c.protection.locked && c.protection.hidden);
    let saved = write_xlsx(&wb).unwrap_or_default();
    let (back, _) = crate::read_xlsx(&saved).unwrap_or_else(|e| panic!("{e}"));
    let back_style = back.styles.get(back.sheets[0].style_id(CellRef::new(0, 1)));
    assert_eq!((back_style.fill.fg, back_style.align.h), (Color::Rgb(0xFFFF00), HAlign::Center));
}

/// The alignment's source and the paragraph alignment are inherited separately (ODF 1.3 part 3
/// §16.2, §20.364): a child that sets the source to `fix` uses the alignment its parent keeps
/// beside `value-type`, and a child of a `value-type` parent is General whatever it aligns.
#[test]
fn ods_alignment_source_and_paragraph_alignment_inherit_separately() {
    let common = format!(
        r#"{DEFAULT}<style:style style:name="Parent" style:family="table-cell" style:parent-style-name="Default"><style:table-cell-properties style:text-align-source="value-type"/><style:paragraph-properties fo:text-align="center"/></style:style>"#
    );
    let automatic = r#"
        <style:style style:name="ce1" style:family="table-cell" style:parent-style-name="Parent"><style:table-cell-properties style:text-align-source="fix"/></style:style>
        <style:style style:name="ce2" style:family="table-cell" style:parent-style-name="Parent"><style:paragraph-properties fo:text-align="end"/></style:style>"#;
    let wb = read(&common, automatic, &table(&format!("<table:table-row>{}{}</table:table-row>", float("ce1", 1.0), float("ce2", 2.0))));
    let h = |at: &str| wb.styles.get(wb.sheets[0].style_id(CellRef::parse(at).unwrap_or_default())).align.h;
    assert_eq!((h("A1"), h("B1")), (HAlign::Center, HAlign::General));
}

/// Columns and rows: the most common width is the sheet's standard width, other widths,
/// hidden columns and rows, set heights and column default styles are kept; heights fitted to
/// the content stay automatic. A table style hides a sheet and colours its tab.
#[test]
fn ods_columns_rows_and_sheet_styles_are_read() {
    let automatic = r##"
        <style:style style:name="co1" style:family="table-column"><style:table-column-properties style:column-width="0.889in"/></style:style>
        <style:style style:name="co2" style:family="table-column"><style:table-column-properties style:column-width="2in"/></style:style>
        <style:style style:name="ro1" style:family="table-row"><style:table-row-properties style:row-height="0.178in" style:use-optimal-row-height="true"/></style:style>
        <style:style style:name="ro2" style:family="table-row"><style:table-row-properties style:row-height="0.5in" style:use-optimal-row-height="false"/></style:style>
        <style:style style:name="ta1" style:family="table"><style:table-properties table:display="false" tableooo:tab-color="#00ff00"/></style:style>
        <style:style style:name="ce9" style:family="table-cell"><style:table-cell-properties fo:background-color="#c0c0c0"/></style:style>
    "##;
    let body = r#"<table:table table:name="Data">
        <table:table-column table:style-name="co2" table:default-cell-style-name="ce9"/>
        <table:table-column table:style-name="co1" table:visibility="collapse"/>
        <table:table-column table:style-name="co1" table:number-columns-repeated="1022" table:default-cell-style-name="Default"/>
        <table:table-row table:style-name="ro1"><table:table-cell office:value-type="float" office:value="1"/><table:table-cell table:style-name="ce9"/></table:table-row>
        <table:table-row table:style-name="ro2"><table:table-cell table:style-name="ce9"/></table:table-row>
        <table:table-row table:style-name="ro1" table:visibility="collapse" table:number-rows-repeated="2"><table:table-cell/></table:table-row>
        <table:table-row table:style-name="ro1" table:number-rows-repeated="1048570"><table:table-cell table:number-columns-repeated="1024"/></table:table-row>
        </table:table>
        <table:table table:name="Hidden" table:style-name="ta1"><table:table-row><table:table-cell/></table:table-row></table:table>"#;
    let wb = read(DEFAULT, automatic, body);
    let sheet = &wb.sheets[0];
    assert!((sheet.default_col_width - 85.344).abs() < 0.01, "standard width {}", sheet.default_col_width);
    assert!((sheet.col_width(0) - 192.0).abs() < 0.01);
    assert!(sheet.is_col_hidden(1) && !sheet.is_col_hidden(2));
    assert_eq!(sheet.cols.len(), 2, "columns at the standard width need no entry");
    let grey = wb.styles.get(sheet.style_id(CellRef::new(5, 0))).fill.fg;
    assert_eq!(grey, Color::Rgb(0xC0C0C0), "a column's default cell style formats its empty cells");
    assert!(sheet.cells.get(CellRef::new(1, 0)).is_none(), "an empty cell styled as its column needs no cell");
    assert_eq!(wb.styles.get(sheet.style_id(CellRef::new(0, 1))).fill.fg, Color::Rgb(0xC0C0C0), "a styled empty cell is kept");
    assert_eq!(sheet.value(CellRef::new(0, 0)), Value::Number(1.0));
    assert!(!sheet.rows.contains_key(&0), "a height fitted to the content is automatic");
    assert_eq!(sheet.rows.get(&1).map(|r| (r.size, r.custom)), Some((Some(48.0), true)));
    assert!(sheet.is_row_hidden(2) && sheet.is_row_hidden(3) && !sheet.is_row_hidden(4));
    assert_eq!(sheet.rows.len(), 3);
    assert_eq!((wb.sheets[1].visibility, wb.sheets[1].tab_color), (Visibility::Hidden, Some(Color::Rgb(0x00FF00))));
}

/// A property a cell's style doesn't set comes from its row's default cell style, then its
/// column's, before the document's default style (ODF 1.3 part 3 §16.2): a 24pt cell in a 0%
/// column shows 50%, and a repeated cell spanning columns of different default styles takes
/// each column's.
#[test]
fn ods_cell_styles_fall_back_to_row_then_column_defaults() {
    let automatic = r#"
        <number:percentage-style style:name="NPct"><number:number number:decimal-places="0" number:min-integer-digits="1"/><number:text>%</number:text></number:percentage-style>
        <number:number-style style:name="NTwo"><number:number number:decimal-places="2" number:min-integer-digits="1"/></number:number-style>
        <style:style style:name="Pct" style:family="table-cell" style:data-style-name="NPct"/>
        <style:style style:name="Two" style:family="table-cell" style:data-style-name="NTwo"/>
        <style:style style:name="Big" style:family="table-cell"><style:text-properties fo:font-size="24pt"/></style:style>"#;
    let body = format!(
        r#"<table:table table:name="Data"><table:table-column table:default-cell-style-name="Pct"/><table:table-column/><table:table-column table:default-cell-style-name="Pct"/>
        <table:table-row><table:table-cell table:style-name="Big" office:value-type="float" office:value="0.5" table:number-columns-repeated="2"/></table:table-row>
        <table:table-row table:default-cell-style-name="Two">{}<table:table-cell/>{}</table:table-row></table:table>"#,
        float("Big", 0.5),
        float("Big", 0.5)
    );
    let wb = read(DEFAULT, automatic, &body);
    let size = |at: &str| wb.styles.get(wb.sheets[0].style_id(CellRef::parse(at).unwrap_or_default())).font.size;
    assert_eq!((code(&wb, "A1"), code(&wb, "B1")), ("0%".to_string(), "General".to_string()));
    assert_eq!((size("A1"), size("B1")), (24.0, 24.0));
    assert_eq!(wb.sheets[0].value(CellRef::parse("B1").unwrap_or_default()), Value::Number(0.5));
    assert_eq!((code(&wb, "A2"), code(&wb, "C2")), ("0.00".to_string(), "0.00".to_string()), "the row's default comes before the column's");
}

/// A set (not optimal) row height stays fixed when it becomes the sheet's standard height: the
/// rows of that height with content keep their own fixed entry, so opening them fits nothing
/// (ODF 1.3 part 3 §20.394), and an XLSX save writes `customHeight`.
#[test]
fn ods_fixed_rows_of_the_standard_height_stay_fixed() {
    let automatic = r#"
        <style:style style:name="ro1" style:family="table-row"><style:table-row-properties style:row-height="15pt" style:use-optimal-row-height="false"/></style:style>
        <style:style style:name="ce1" style:family="table-cell"><style:text-properties fo:font-size="24pt"/></style:style>"#;
    let body = table(
        r#"<table:table-row table:style-name="ro1"><table:table-cell table:style-name="ce1" office:value-type="string" office:string-value="Big"/></table:table-row><table:table-row table:style-name="ro1" table:number-rows-repeated="3"><table:table-cell/></table:table-row>"#,
    );
    let wb = read(DEFAULT, automatic, &body);
    let sheet = &wb.sheets[0];
    assert!((sheet.default_row_height - 20.0).abs() < 0.01, "standard height {}", sheet.default_row_height);
    assert_eq!(sheet.rows.get(&0).map(|r| r.custom), Some(true), "the row with content stays fixed");
    assert!((sheet.row_height(0) - 20.0).abs() < 0.01);
    assert_eq!(sheet.rows.len(), 1, "empty rows of the standard height need no entry");
    let xlsx = write_xlsx(&wb).unwrap_or_else(|e| panic!("{e}"));
    let (back, _) = crate::read_xlsx(&xlsx).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(back.sheets[0].rows.get(&0).map(|r| r.custom), Some(true));
}

/// A document formatting more empty cells than GridCraft keeps still opens: the values are
/// read and the formatting past the budget is dropped with a warning.
#[test]
fn ods_formatted_empty_cells_past_the_budget_keep_the_values() {
    let automatic =
        r##"<style:style style:name="ce9" style:family="table-cell"><style:table-cell-properties fo:background-color="#c0c0c0"/></style:style>"##;
    let body = table(
        r#"<table:table-row><table:table-cell office:value-type="float" office:value="7"/></table:table-row><table:table-row table:number-rows-repeated="1048575"><table:table-cell table:style-name="ce9" table:number-columns-repeated="1024"/></table:table-row>"#,
    );
    let (wb, report) = read_ods(&package(DEFAULT, automatic, &body)).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(wb.sheets[0].value(CellRef::new(0, 0)), Value::Number(7.0));
    assert!(report.warnings.iter().any(|w| w.contains("empty ODS cells")));
}

/// A flat document holds its common and automatic styles itself.
#[test]
fn ods_flat_documents_read_their_styles() {
    let doc = format!(
        r#"<?xml version="1.0"?><office:document {NS} office:mimetype="application/vnd.oasis.opendocument.spreadsheet"><office:styles>{DEFAULT}<number:number-style style:name="N1"><number:number number:decimal-places="3" number:min-integer-digits="1"/></number:number-style></office:styles><office:automatic-styles><style:style style:name="ce1" style:family="table-cell" style:data-style-name="N1"/></office:automatic-styles><office:body><office:spreadsheet>{}</office:spreadsheet></office:body></office:document>"#,
        table(&format!("<table:table-row>{}</table:table-row>", float("ce1", 2.5)))
    );
    let (wb, _) = read_ods(doc.as_bytes()).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(code(&wb, "A1"), "0.000");
}

/// Frozen panes come from the first view's settings for each sheet: a frozen split (mode 2)
/// counts the frozen columns and rows; a plain split is not a freeze.
#[test]
fn ods_frozen_panes_come_from_the_view_settings() {
    let item = |name: &str, v: u32| format!(r#"<config:config-item config:name="{name}" config:type="int">{v}</config:config-item>"#);
    let entry = |sheet: &str, mode: u32| {
        format!(
            r#"<config:config-item-map-entry config:name="{sheet}">{}{}{}{}{}{}</config:config-item-map-entry>"#,
            item("HorizontalSplitMode", mode),
            item("VerticalSplitMode", mode),
            item("HorizontalSplitPosition", 1),
            item("VerticalSplitPosition", 2),
            item("PositionRight", 4),
            item("PositionBottom", 10)
        )
    };
    let settings = format!(
        r#"<?xml version="1.0"?><office:document-settings xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:config="urn:oasis:names:tc:opendocument:xmlns:config:1.0"><office:settings><config:config-item-set config:name="ooo:view-settings"><config:config-item-map-indexed config:name="Views"><config:config-item-map-entry><config:config-item-map-named config:name="Tables">{}{}</config:config-item-map-named></config:config-item-map-entry></config:config-item-map-indexed></config:config-item-set></office:settings></office:document-settings>"#,
        entry("Data", 2),
        entry("Split", 1)
    );
    let content = format!(
        r#"<?xml version="1.0"?><office:document-content {NS}><office:body><office:spreadsheet>{}<table:table table:name="Split"><table:table-row><table:table-cell/></table:table-row></table:table></office:spreadsheet></office:body></office:document-content>"#,
        table("<table:table-row><table:table-cell/></table:table-row>")
    );
    let bytes = make_zip(&[("mimetype", MIME), ("content.xml", content.as_bytes()), ("settings.xml", settings.as_bytes())]);
    let (wb, _) = read_ods(&bytes).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(wb.sheets[0].freeze, Some((2, 1)));
    assert_eq!(wb.sheets[0].view_top_left, CellRef::new(10, 4));
    assert_eq!(wb.sheets[1].freeze, None);
}

/// A text style's own section is Excel's text section, after the three numeric sections its
/// sign maps make (ODF 1.3 part 3 §16.3, §19.472): the zero map is the third section, not lost.
#[test]
fn ods_text_style_sign_maps_keep_the_zero_section() {
    let data = r#"
        <number:number-style style:name="Pos"><number:number number:decimal-places="0" number:min-integer-digits="1"/></number:number-style>
        <number:number-style style:name="Neg"><number:text>-</number:text><number:number number:decimal-places="0" number:min-integer-digits="1"/></number:number-style>
        <number:number-style style:name="Zero"><number:text>zero</number:text></number:number-style>
        <number:text-style style:name="Bare"><number:text-content/><style:map style:condition="value()&gt;0" style:apply-style-name="Pos"/><style:map style:condition="value()&lt;0" style:apply-style-name="Neg"/><style:map style:condition="value()=0" style:apply-style-name="Zero"/></number:text-style>
        <number:text-style style:name="Named"><number:text>Name: </number:text><number:text-content/><style:map style:condition="value()=0" style:apply-style-name="Zero"/><style:map style:condition="value()&lt;0" style:apply-style-name="Neg"/><style:map style:condition="value()&gt;0" style:apply-style-name="Pos"/></number:text-style>
        <number:text-style style:name="TwoMaps"><number:text-content/><style:map style:condition="value()&gt;0" style:apply-style-name="Pos"/><style:map style:condition="value()&lt;0" style:apply-style-name="Neg"/></number:text-style>
        <style:style style:name="c1" style:family="table-cell" style:data-style-name="Bare"/>
        <style:style style:name="c2" style:family="table-cell" style:data-style-name="Named"/>
        <style:style style:name="c3" style:family="table-cell" style:data-style-name="TwoMaps"/>"#;
    let wb = read(DEFAULT, data, &table(&format!("<table:table-row>{}{}{}</table:table-row>", float("c1", 0.0), float("c2", 0.0), float("c3", 0.0))));
    assert_eq!(code(&wb, "A1"), "0;-0;\"zero\";@");
    assert_eq!(code(&wb, "B1"), "0;-0;\"zero\";\"Name\": @");
    assert_eq!(code(&wb, "C1"), "0;-0;General;@", "zero is the text style's: the number as itself");
}

/// Embedded text at position k has k digits to its right (ODF 1.3 part 3 §19.358); past the
/// minimum integer digits it still needs a placeholder to its left for the digits there.
#[test]
fn ods_embedded_text_past_the_minimum_digits_is_kept() {
    let number = |name: &str, min: u32, position: u32| {
        format!(
            r#"<number:number-style style:name="{name}"><number:number number:decimal-places="0" number:min-integer-digits="{min}"><number:embedded-text number:position="{position}">-</number:embedded-text></number:number></number:number-style><style:style style:name="c{name}" style:family="table-cell" style:data-style-name="{name}"/>"#
        )
    };
    let data = [number("N1", 1, 4), number("N2", 4, 4), number("N3", 2, 0), number("N4", 6, 3)].concat();
    let row: String = (1..=4).map(|n| float(&format!("cN{n}"), 5551234.0)).collect();
    let wb = read(DEFAULT, &data, &table(&format!("<table:table-row>{row}</table:table-row>")));
    let got: Vec<String> = ["A1", "B1", "C1", "D1"].iter().map(|at| code(&wb, at)).collect();
    assert_eq!(got, ["#-###0", "#-0000", "00-", "000-000"]);
}

/// A map every number meets ends the maps and formats every number (ODF 1.3 part 3 §19.472),
/// and a threshold is written in its short spelling: the code stays within Excel's 255
/// characters.
#[test]
fn ods_map_thresholds_stay_short() {
    let data = r#"
        <number:number-style style:name="Two"><number:number number:decimal-places="2" number:min-integer-digits="1"/></number:number-style>
        <number:number-style style:name="Big"><number:text>big</number:text></number:number-style>
        <number:text-style style:name="All"><number:text-content/><style:map style:condition="value()&lt;=1.7976931348623157E+308" style:apply-style-name="Two"/></number:text-style>
        <number:text-style style:name="AllNamed"><number:text>Name: </number:text><number:text-content/><style:map style:condition="value()&gt;=-1.7976931348623157E+308" style:apply-style-name="Two"/></number:text-style>
        <number:number-style style:name="Huge"><number:number number:decimal-places="0" number:min-integer-digits="1"/><style:map style:condition="value()&gt;=1E+300" style:apply-style-name="Big"/></number:number-style>
        <number:number-style style:name="Tiny"><number:number number:decimal-places="0" number:min-integer-digits="1"/><style:map style:condition="value()&lt;0.0000001" style:apply-style-name="Big"/></number:number-style>
        <style:style style:name="c1" style:family="table-cell" style:data-style-name="All"/>
        <style:style style:name="c2" style:family="table-cell" style:data-style-name="AllNamed"/>
        <style:style style:name="c3" style:family="table-cell" style:data-style-name="Huge"/>
        <style:style style:name="c4" style:family="table-cell" style:data-style-name="Tiny"/>"#;
    let row: String = (1..=4).map(|n| float(&format!("c{n}"), 1.234)).collect();
    let wb = read(DEFAULT, data, &table(&format!("<table:table-row>{row}</table:table-row>")));
    let got: Vec<String> = ["A1", "B1", "C1", "D1"].iter().map(|at| code(&wb, at)).collect();
    assert_eq!(got, ["0.00", "0.00;-0.00;0.00;\"Name\": @", "[>=1E+300]\"big\";0", "[<1E-7]\"big\";0"]);
}

/// A text style's own section, which formats text (ODF 1.3 part 3 §16.29.26), stays in the code
/// when its maps (§16.3) end in one every number meets (§19.472). After one condition the last
/// section holding `@` is the code's text section (live Excel 16.114 shows
/// `[>5]"big";0.00;"Name: "@` as `Name: x` for "x", 1.23 for 1.234 and -3.00 for -3); after two
/// it is the fourth. A bare `@` and a style that is not a text style need no text section, and a
/// text section without `@` can only be the fourth.
#[test]
fn ods_a_trailing_all_numbers_map_keeps_the_text_section() {
    let maps = |conditions: &[(&str, &str)]| -> String {
        conditions.iter().map(|(c, target)| format!(r#"<style:map style:condition="value(){c}" style:apply-style-name="{target}"/>"#)).collect()
    };
    let max = "&lt;=1.7976931348623157E+308";
    let one = maps(&[("&gt;5", "Big"), (max, "Two")]);
    let two = maps(&[("&gt;5", "Big"), ("&lt;0", "Neg"), (max, "Two")]);
    let data = format!(
        r#"<number:number-style style:name="Two"><number:number number:decimal-places="2" number:min-integer-digits="1"/></number:number-style>
        <number:number-style style:name="Big"><number:text>big</number:text></number:number-style>
        <number:number-style style:name="Neg"><number:text>neg</number:text></number:number-style>
        <number:text-style style:name="One"><number:text>Name: </number:text><number:text-content/>{one}</number:text-style>
        <number:text-style style:name="Two2"><number:text>Name: </number:text><number:text-content/>{two}</number:text-style>
        <number:text-style style:name="Bare"><number:text-content/>{one}</number:text-style>
        <number:text-style style:name="Bare2"><number:text-content/>{two}</number:text-style>
        <number:text-style style:name="Fixed"><number:text>fixed</number:text>{one}</number:text-style>
        <number:number-style style:name="Numbers"><number:number number:decimal-places="3" number:min-integer-digits="1"/>{one}</number:number-style>
        <style:style style:name="c1" style:family="table-cell" style:data-style-name="One"/>
        <style:style style:name="c2" style:family="table-cell" style:data-style-name="Two2"/>
        <style:style style:name="c3" style:family="table-cell" style:data-style-name="Bare"/>
        <style:style style:name="c4" style:family="table-cell" style:data-style-name="Bare2"/>
        <style:style style:name="c5" style:family="table-cell" style:data-style-name="Fixed"/>
        <style:style style:name="c6" style:family="table-cell" style:data-style-name="Numbers"/>"#
    );
    let row: String = (1..=6).map(|n| float(&format!("c{n}"), 1.234)).collect();
    let wb = read(DEFAULT, &data, &table(&format!("<table:table-row>{row}</table:table-row>")));
    let got: Vec<String> = ["A1", "B1", "C1", "D1", "E1", "F1"].iter().map(|at| code(&wb, at)).collect();
    assert_eq!(
        got,
        [
            "[>5]\"big\";0.00;\"Name\": @",
            "[>5]\"big\";[<0]\"neg\";0.00;\"Name\": @",
            "[>5]\"big\";0.00",
            "[>5]\"big\";[<0]\"neg\";0.00",
            "[>5]\"big\";-0.00;0.00;\"fixed\"",
            "[>5]\"big\";0.00",
        ]
    );
}

/// A data style's maps name data styles of the file that defines it (ODF 1.3 part 3 §16.1,
/// §19.472): a content cell style using a common data style of styles.xml gets the map targets
/// of styles.xml, not the content's styles of the same name.
#[test]
fn ods_map_targets_resolve_where_the_data_style_is_defined() {
    let styles = format!(
        r#"<?xml version="1.0"?><office:document-styles {NS}><office:styles>{DEFAULT}<number:number-style style:name="Main"><number:text>fallback</number:text><style:map style:condition="value()&gt;=0" style:apply-style-name="Positive"/></number:number-style></office:styles><office:automatic-styles><number:percentage-style style:name="Positive"><number:number number:decimal-places="0" number:min-integer-digits="1"/><number:text>%</number:text></number:percentage-style></office:automatic-styles></office:document-styles>"#
    );
    let content = format!(
        r#"<?xml version="1.0"?><office:document-content {NS}><office:automatic-styles><number:number-style style:name="Positive"><number:number number:decimal-places="3" number:min-integer-digits="1"/></number:number-style><style:style style:name="ce1" style:family="table-cell" style:data-style-name="Main"/></office:automatic-styles><office:body><office:spreadsheet>{}</office:spreadsheet></office:body></office:document-content>"#,
        table(&format!("<table:table-row>{}</table:table-row>", float("ce1", 0.5)))
    );
    let ods = make_zip(&[("mimetype", MIME), ("styles.xml", styles.as_bytes()), ("content.xml", content.as_bytes())]);
    let (wb, _) = read_ods(&ods).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(code(&wb, "A1"), "0%;\"fallback\"");
}

/// Column, row and table styles inherit what they don't set from their common parent styles,
/// then from the family's default style (ODF 1.3 part 3 §16.2, §19.510); a child's own
/// property wins.
#[test]
fn ods_column_row_and_table_styles_inherit_from_their_parents() {
    let common = format!(
        r##"{DEFAULT}<style:default-style style:family="table"><style:table-properties tableooo:tab-color="#0000ff"/></style:default-style>
        <style:style style:name="BaseRow" style:family="table-row"><style:table-row-properties style:row-height="30pt" style:use-optimal-row-height="false"/></style:style>
        <style:style style:name="MidRow" style:family="table-row" style:parent-style-name="BaseRow"/>
        <style:style style:name="BaseCol" style:family="table-column"><style:table-column-properties style:column-width="2in"/></style:style>
        <style:style style:name="BaseTable" style:family="table"><style:table-properties table:display="false"/></style:style>"##
    );
    let automatic = r#"
        <style:style style:name="ro1" style:family="table-row" style:parent-style-name="MidRow"/>
        <style:style style:name="ro2" style:family="table-row" style:parent-style-name="BaseRow"><style:table-row-properties style:row-height="45pt"/></style:style>
        <style:style style:name="co1" style:family="table-column" style:parent-style-name="BaseCol"/>
        <style:style style:name="ta1" style:family="table" style:parent-style-name="BaseTable"/>"#;
    let body = r#"<table:table table:name="Data">
        <table:table-column table:style-name="co1"/>
        <table:table-row table:style-name="ro1"><table:table-cell office:value-type="float" office:value="1"/></table:table-row>
        <table:table-row table:style-name="ro2"><table:table-cell office:value-type="float" office:value="2"/></table:table-row>
        </table:table><table:table table:name="Hidden" table:style-name="ta1"><table:table-row><table:table-cell/></table:table-row></table:table>"#;
    let wb = read(&common, automatic, body);
    let sheet = &wb.sheets[0];
    assert!((sheet.col_width(0) - 192.0).abs() < 0.01, "inherited width {}", sheet.col_width(0));
    assert_eq!(sheet.rows.get(&0).map(|r| (r.size, r.custom)), Some((Some(40.0), true)), "a grandparent's fixed height");
    assert_eq!(sheet.rows.get(&1).map(|r| (r.size, r.custom)), Some((Some(60.0), true)), "the child's own height");
    assert_eq!((wb.sheets[1].visibility, wb.sheets[1].tab_color), (Visibility::Hidden, Some(Color::Rgb(0x0000FF))));
}

/// A percentage font size scales the parent style's size (ODF 1.3 part 3 §20.190), through
/// each level of the chain.
#[test]
fn ods_percentage_font_sizes_scale_the_parent_size() {
    let common = format!(
        r#"{DEFAULT}<style:style style:name="Base" style:family="table-cell" style:parent-style-name="Default"><style:text-properties fo:font-size="12pt"/></style:style>
        <style:style style:name="Scaled" style:family="table-cell" style:parent-style-name="Base"><style:text-properties fo:font-size="200%"/></style:style>
        <style:style style:name="Half" style:family="table-cell" style:parent-style-name="Scaled"><style:text-properties fo:font-size="50%"/></style:style>"#
    );
    let automatic = r#"<style:style style:name="ce1" style:family="table-cell" style:parent-style-name="Scaled"/>"#;
    let row = format!("<table:table-row>{}{}</table:table-row>", float("ce1", 1.0), float("Half", 1.0));
    let wb = read(&common, automatic, &table(&row));
    let size = |at: &str| wb.styles.get(wb.sheets[0].style_id(CellRef::parse(at).unwrap_or_default())).font.size;
    assert_eq!((size("A1"), size("B1")), (24.0, 12.0));
    let saved = write_xlsx(&wb).unwrap_or_default();
    let (back, _) = crate::read_xlsx(&saved).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(back.styles.get(back.sheets[0].style_id(CellRef::new(0, 0))).font.size, 24.0);
}

/// A percentage font size is of the parent style's size (ODF 1.3 part 3 §20.190), "rather than
/// … the attributes neighborhood": the default cell style of the row or column a cell is in
/// supplies what the cell's own chain leaves unset (§16.2) but is not its parent. A 50% child
/// of a 10pt style is 5pt whatever the row or column default gives, and a percentage in a row
/// or column default style is of that style's own parent too.
#[test]
fn ods_percentage_font_sizes_ignore_the_row_and_column_default_styles() {
    let style = |name: &str, size: &str, parent: &str| {
        format!(
            r#"<style:style style:name="{name}" style:family="table-cell" style:parent-style-name="{parent}"><style:text-properties fo:font-size="{size}"/></style:style>"#
        )
    };
    let common = [
        DEFAULT.to_string(),
        style("Half", "50%", "Default"),
        style("Quarter", "50%", "Half"),
        style("RowBig", "20pt", "Default"),
        style("ColBig", "30pt", "Default"),
        style("RowHalf", "50%", "Default"),
        style("ColHalf", "50%", "Default"),
    ]
    .concat();
    let plain = r#"<table:table-cell office:value-type="float" office:value="1"/>"#;
    let body = format!(
        r#"<table:table table:name="Data">
        <table:table-column table:default-cell-style-name="ColBig"/>
        <table:table-column table:default-cell-style-name="ColHalf"/>
        <table:table-column/>
        <table:table-row table:default-cell-style-name="RowBig">{}{}{}{plain}</table:table-row>
        <table:table-row>{}{}{plain}</table:table-row>
        <table:table-row>{plain}{plain}{plain}</table:table-row>
        <table:table-row table:default-cell-style-name="RowHalf">{plain}{plain}{plain}</table:table-row>
        </table:table>"#,
        float("Half", 1.0),
        float("Half", 1.0),
        float("Quarter", 1.0),
        float("Half", 1.0),
        float("Half", 1.0),
    );
    let wb = read(&common, "", &body);
    let size = |at: &str| wb.styles.get(wb.sheets[0].style_id(CellRef::parse(at).unwrap_or_default())).font.size;
    let want = [
        // A row default of 20pt: the cells' own 50% is of 10pt, in a column of 30pt or 5pt or none.
        ("A1", 5.0),
        ("B1", 5.0),
        ("C1", 2.5),
        ("D1", 20.0),
        // No row default: the column's does not scale it either.
        ("A2", 5.0),
        ("B2", 5.0),
        ("C2", 10.0),
        // Unstyled cells take the column default, and 50% of 10pt in a default style is 5pt.
        ("A3", 30.0),
        ("B3", 5.0),
        ("C3", 10.0),
        // The row default wins over the column's, and its 50% is of its own parent, not the column's size.
        ("A4", 5.0),
        ("B4", 5.0),
        ("C4", 5.0),
    ];
    let got: Vec<(&str, f32)> = want.iter().map(|(at, _)| (*at, size(at))).collect();
    assert_eq!(got, want);
}

/// Aligned by value type, a string is on the left and every other value type on the right (ODF
/// 1.3 part 3 §20.364): a logical value, which Excel's General centres, is right-aligned, and so
/// is an error of another type than string; an error LibreOffice-style, typed as the string it
/// shows, is left-aligned; a number and a string keep General.
#[test]
fn ods_value_type_alignment_follows_the_value_type() {
    let automatic = r#"
        <style:style style:name="ce1" style:family="table-cell"><style:table-cell-properties style:text-align-source="value-type"/></style:style>
        <style:style style:name="ce2" style:family="table-cell"><style:table-cell-properties style:text-align-source="fix"/><style:paragraph-properties fo:text-align="center"/></style:style>"#;
    let error = |value_type: &str| {
        format!(
            r#"<table:table-cell table:style-name="ce1" table:formula="of:=1/0" office:value-type="{value_type}" office:value="0" xmlns:calcext="urn:org:documentfoundation:names:experimental:calc:xmlns:calcext:1.0" calcext:value-type="error"><text:p>#DIV/0!</text:p></table:table-cell>"#
        )
    };
    let row = format!(
        r#"<table:table-row>
        <table:table-cell table:style-name="ce1" office:value-type="boolean" office:boolean-value="true"/>
        <table:table-cell table:style-name="ce1" office:value-type="string"><text:p>abc</text:p></table:table-cell>
        <table:table-cell table:style-name="ce1" office:value-type="float" office:value="1"/>
        {}
        <table:table-cell table:style-name="ce2" office:value-type="boolean" office:boolean-value="false"/>
        <table:table-cell office:value-type="boolean" office:boolean-value="false"/>
        {}
        </table:table-row>"#,
        error("float"),
        error("string")
    );
    let wb = read(DEFAULT, automatic, &table(&row));
    let h = |at: &str| wb.styles.get(wb.sheets[0].style_id(CellRef::parse(at).unwrap_or_default())).align.h;
    let got: Vec<HAlign> = ["A1", "B1", "C1", "D1", "E1", "F1", "G1"].iter().map(|at| h(at)).collect();
    assert_eq!(got, [HAlign::Right, HAlign::General, HAlign::General, HAlign::Right, HAlign::Center, HAlign::General, HAlign::Left]);
}
