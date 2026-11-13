//! OpenDocument Spreadsheet import: packages (`.ods`, `.ots`) and flat XML documents (`.fods`).
//! Formula caches become constants; styles and other ODF features are deliberately not
//! translated. All fixtures are authored in Rust.
//! Format reference: ODF 1.3, part 3, sections 6.1 and 9.1:
//! <https://docs.oasis-open.org/office/OpenDocument/v1.3/os/part3-schema/OpenDocument-v1.3-os-part3-schema.html>.

use std::io::{Cursor, Read};
use std::sync::Arc;

use gridcraft_core::date::{days_in_month, serial_from_ymd};
use gridcraft_core::{CellError, CellRef, DateSystem, MAX_COLS, MAX_ROWS, RangeRef, Value};
use gridcraft_formula::Expr;
use gridcraft_model::{Cell, DefinedName, Formula, HAlign, LineInfo, NumFmt, Sheet, StyleId, StyleTable, Visibility, Workbook};
use quick_xml::NsReader;
use quick_xml::XmlVersion;
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::ResolveResult;

use crate::ods_formula::{Base, IndirectName, Matching, Sheets, Unsupported};
use crate::ods_style::{Origin, Styles};
use crate::package::{MAX_ENTRIES, Package};
use crate::{IoError, ReadOptions, ReadReport};

const MIME: &[u8] = b"application/vnd.oasis.opendocument.spreadsheet";
/// The same document saved as a template (`.ots`): its content is a spreadsheet's.
const MIME_TEMPLATE: &[u8] = b"application/vnd.oasis.opendocument.spreadsheet-template";
/// Said once when data lay past the last row or column.
const BEYOND_EDGE: &str = "Some cells lay beyond the last row or column of the sheet (XFD1048576) and were dropped.";
const OFFICE: &[u8] = b"urn:oasis:names:tc:opendocument:xmlns:office:1.0";
const TABLE: &[u8] = b"urn:oasis:names:tc:opendocument:xmlns:table:1.0";
const TEXT: &[u8] = b"urn:oasis:names:tc:opendocument:xmlns:text:1.0";
const CALC_EXT: &[u8] = b"urn:org:documentfoundation:names:experimental:calc:xmlns:calcext:1.0";
const MANIFEST: &[u8] = b"urn:oasis:names:tc:opendocument:xmlns:manifest:1.0";
const MAX_CELLS: u64 = 1_000_000;
const MAX_MERGES: u64 = 100_000;
const MAX_CELL_TEXT: usize = 1024 * 1024;
const MAX_TEXT: u64 = 64 * 1024 * 1024;
const MAX_SHEETS: usize = 1024;
const MAX_NAMES: usize = 65_536;
/// Array formulas this large or smaller blank their other cells position by position; larger
/// ones scan the sheet's cells instead.
const MAX_ARRAY_SCAN: u64 = 65_536;
const UNSUPPORTED_VALUE: &str = "Some ODS values or formula caches were unsupported or missing; their displayed text (or raw value) was kept as text. A formula without either is marked [Unavailable ODS formula result].";
const UNAVAILABLE: &str = "[Unavailable ODS formula result]";
const IMPORTED: &str = "ODS import: values, formulas, named ranges and cell formatting are read. Formulas GridCraft can't translate keep their saved results. Charts, images, comments, conditional formatting, data validation and other features are not imported. Save a new .xlsx file; ODS saving is not supported.";
const STYLES_UNREADABLE: &str = "The ODS styles part (styles.xml) could not be read; the formatting defined in it was not imported.";
/// Formatted empty cells kept (a value's cell is always kept); past it their formatting is lost.
const MAX_STYLED_BLANKS: u64 = 1_000_000;
const BLANKS_DROPPED: &str =
    "Some formatting of empty ODS cells was not imported: the document formats more empty cells than GridCraft keeps (1 million).";
/// Rows and columns with their own size, visibility or style, across the workbook.
const MAX_LINES: u64 = 2_000_000;
const LINES_DROPPED: &str =
    "Some ODS row heights, column widths or hidden rows and columns were not imported: the document sets more than GridCraft keeps (2 million).";

const FOREIGN_EPOCH: &str = "a null date (table:null-date) that neither Excel date system counts from";
/// Why a formula keeps its saved result when a name it uses was left out of the import.
const LEFT_OUT: &str = "names GridCraft can't import";
/// Why a name is left out when a name it uses was.
const USES_LEFT_OUT: &str = "names that use names or functions GridCraft can't import";
/// Excel's built-in names an import applies as sheet settings rather than defining them.
const BUILT_IN_NAMES: [&str; 3] = ["_xlnm._filterdatabase", "_xlnm.print_area", "_xlnm.print_titles"];

/// Reads only the small, exact mimetype entry, including when the ZIP uses compression.
/// Looking at a filename or at arbitrary bytes inside the archive is not sufficient. A
/// spreadsheet template is a spreadsheet.
pub(crate) fn has_mimetype(bytes: &[u8]) -> bool {
    let Ok(mut zip) = zip::ZipArchive::new(Cursor::new(bytes)) else { return false };
    if zip.len() > MAX_ENTRIES {
        return false;
    }
    let Ok(mut file) = zip.by_name("mimetype") else { return false };
    if file.size() > MIME_TEMPLATE.len() as u64 {
        return false;
    }
    let mut value = Vec::new();
    (&mut file).take(MIME_TEMPLATE.len() as u64 + 1).read_to_end(&mut value).is_ok() && (value == MIME || value == MIME_TEMPLATE)
}

/// A flat OpenDocument spreadsheet (`.fods`): one XML document whose root is `office:document`
/// with a spreadsheet (or template) `office:mimetype` (ODF 1.3 part 3, sections 3.1.1 and 19.389).
/// Only the prolog and the root's start tag are read.
pub(crate) fn is_flat(bytes: &[u8]) -> bool {
    let body = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    if !body.trim_ascii_start().starts_with(b"<") {
        return false;
    }
    let mut reader = NsReader::from_reader(body);
    loop {
        match reader.read_event() {
            Ok(Event::Start(e) | Event::Empty(e)) => {
                if tag(&mut reader, &e) != Tag::Flat {
                    return false;
                }
                let mime = attr(&mut reader, &e, OFFICE, b"mimetype");
                return matches!(mime, Ok(Some(m)) if m.as_bytes() == MIME || m.as_bytes() == MIME_TEMPLATE);
            }
            Ok(Event::Decl(_) | Event::Comment(_) | Event::PI(_)) => {}
            Ok(Event::Text(t)) if t.trim_ascii().is_empty() => {}
            _ => return false,
        }
    }
}

/// Imports ODS sheets, typed values, displayed text, merges, formulas and named ranges, from a
/// package or a flat XML document, with the default [`ReadOptions`]. Always inspect the returned
/// warnings. Saving to ODS is not supported.
pub fn read_ods(bytes: &[u8]) -> Result<(Workbook, ReadReport), IoError> {
    read_ods_with(bytes, ReadOptions::default())
}

/// [`read_ods`] with the caller's function facts: a formula calling a function the
/// `known_function` predicate (else [`crate::FunctionInfo::is_builtin`]) doesn't know keeps its saved
/// result, and formulas are read with implicit intersection like Excel's legacy formulas
/// (OpenFormula's scalar parameters intersect a range argument the same way).
pub fn read_ods_with<'a>(bytes: &[u8], opts: impl Into<ReadOptions<'a>>) -> Result<(Workbook, ReadReport), IoError> {
    let opts = opts.into();
    let mut import = Import::new();
    let panes = if is_flat(bytes) {
        let doc = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
        import.parse(doc, Tag::Flat)?;
        crate::ods_settings::frozen_panes(doc)
    } else {
        let mut package = Package::open(bytes)?;
        if !has_mimetype(bytes) {
            return Err(IoError::Format("missing or invalid ODS mimetype".into()));
        }
        if let Some(manifest) = package.read("META-INF/manifest.xml")? {
            check_manifest(&manifest)?;
        }
        // The common styles (and the data styles they name) are in styles.xml. A part that
        // can't be read costs its formatting, not the values.
        match package.read("styles.xml") {
            Ok(Some(styles)) => {
                let mut part = Import::new();
                match part.parse(&styles, Tag::StylesRoot) {
                    Ok(()) => import.styles = part.styles,
                    Err(_) => import.report.warn(STYLES_UNREADABLE),
                }
            }
            Ok(None) => {}
            Err(_) => import.report.warn(STYLES_UNREADABLE),
        }
        let content = package.read("content.xml")?.ok_or_else(|| IoError::Format("ODS content.xml is missing".into()))?;
        import.parse(&content, Tag::Document)?;
        package.read("settings.xml").ok().flatten().map(|s| crate::ods_settings::frozen_panes(&s)).unwrap_or_default()
    };
    import.freeze(&panes);
    import.formulas(opts);
    import.data_pilots();
    crate::import_rules::ensure_a_sheet(&mut import.wb, &mut import.report);
    Ok((import.wb, import.report))
}

fn is_ns(ns: ResolveResult<'_>, expected: &[u8]) -> bool {
    matches!(ns, ResolveResult::Bound(n) if n.as_ref() == expected)
}

fn attr(reader: &mut NsReader<&[u8]>, e: &BytesStart<'_>, ns: &[u8], name: &[u8]) -> Result<Option<String>, IoError> {
    let mut found = None;
    for a in e.attributes() {
        let a = a.map_err(|e| IoError::Xml(e.to_string()))?;
        let (resolved, local) = reader.resolver_mut().resolve_attribute(a.key);
        if is_ns(resolved, ns) && local.as_ref() == name {
            let value = a.normalized_value(XmlVersion::Explicit1_0).map_err(|e| IoError::Xml(e.to_string()))?;
            if value.len() > MAX_CELL_TEXT {
                return Err(IoError::TooLarge("ODS attribute exceeds 1 MiB".into()));
            }
            found = Some(value.into_owned());
        }
    }
    Ok(found)
}

fn positive(value: Option<String>, default: u32, limit: u32) -> Result<u32, IoError> {
    let n = match value {
        Some(s) => s.parse::<u32>().map_err(|_| IoError::Format("invalid ODS repeat or span count".into()))?,
        None => default,
    };
    if n == 0 || n > limit {
        return Err(IoError::TooLarge("ODS repeat or span exceeds worksheet bounds".into()));
    }
    Ok(n)
}

fn xml_error(e: impl std::fmt::Display) -> IoError {
    IoError::Xml(format!("ODS: {e}"))
}

fn check_manifest(bytes: &[u8]) -> Result<(), IoError> {
    let mut reader = NsReader::from_reader(bytes);
    let mut depth = 0usize;
    let mut root_seen = false;
    loop {
        let event = reader.read_event().map_err(xml_error)?;
        match &event {
            Event::Start(e) | Event::Empty(e) => {
                let (ns, name) = reader.resolver_mut().resolve_element(e.name());
                if is_ns(ns.clone(), MANIFEST) && name.as_ref() == b"encryption-data" {
                    return Err(IoError::Format("encrypted ODS files are not supported".into()));
                }
                if depth == 0 {
                    if root_seen || !is_ns(ns, MANIFEST) || name.as_ref() != b"manifest" {
                        return Err(xml_error("expected one manifest:manifest root"));
                    }
                    root_seen = true;
                }
                if matches!(event, Event::Start(_)) {
                    depth += 1;
                    if depth > crate::xml::MAX_DEPTH {
                        return Err(xml_error("manifest nested too deeply"));
                    }
                }
            }
            Event::End(_) => depth = depth.checked_sub(1).ok_or_else(|| xml_error("unexpected manifest closing element"))?,
            Event::DocType(_) => return Err(xml_error("DOCTYPE is not supported")),
            Event::Eof => break,
            _ => {}
        }
    }
    if !root_seen || depth != 0 {
        return Err(xml_error("incomplete manifest"));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tag {
    /// `office:document-content`, the root of a package's `content.xml`.
    Document,
    /// `office:document`, the root of a flat document.
    Flat,
    /// `office:document-styles`, the root of a package's `styles.xml`.
    StylesRoot,
    /// `office:styles` (common styles), `office:automatic-styles` and `office:font-face-decls`.
    Styles(Origin),
    Body,
    Spreadsheet,
    Table,
    /// `table:table-column`.
    Column,
    /// `table:table-column-group`, `table:table-header-columns` and `table:table-columns`.
    Columns,
    Rows,
    Row,
    Cell,
    Covered,
    Paragraph,
    Inline,
    Space,
    Tab,
    Break,
    /// `table:named-range` and `table:named-expression`.
    Named,
    /// `table:data-pilot-table`, a DataPilot (PivotTable) report.
    DataPilot,
    /// `table:calculation-settings`: how the document's functions match text and round.
    CalcSettings,
    /// `table:null-date` in the calculation settings: the day numeric date 0 is.
    NullDate,
    Other,
}

fn tag(reader: &mut NsReader<&[u8]>, e: &BytesStart<'_>) -> Tag {
    let (ns, name) = reader.resolver_mut().resolve_element(e.name());
    let local = name.as_ref();
    if is_ns(ns.clone(), OFFICE) {
        return match local {
            b"document-content" => Tag::Document,
            b"document" => Tag::Flat,
            b"document-styles" => Tag::StylesRoot,
            b"styles" | b"font-face-decls" => Tag::Styles(Origin::Common),
            b"automatic-styles" => Tag::Styles(Origin::Automatic),
            b"body" => Tag::Body,
            b"spreadsheet" => Tag::Spreadsheet,
            _ => Tag::Other,
        };
    }
    if is_ns(ns.clone(), TABLE) {
        return match local {
            b"table" => Tag::Table,
            b"table-column" => Tag::Column,
            b"table-column-group" | b"table-header-columns" | b"table-columns" => Tag::Columns,
            b"table-row-group" | b"table-header-rows" | b"table-rows" => Tag::Rows,
            b"table-row" => Tag::Row,
            b"table-cell" => Tag::Cell,
            b"covered-table-cell" => Tag::Covered,
            b"named-range" | b"named-expression" => Tag::Named,
            b"data-pilot-table" => Tag::DataPilot,
            b"calculation-settings" => Tag::CalcSettings,
            b"null-date" => Tag::NullDate,
            _ => Tag::Other,
        };
    }
    if is_ns(ns, TEXT) {
        return match local {
            b"p" | b"h" => Tag::Paragraph,
            b"span" | b"a" | b"meta" | b"meta-field" | b"ruby" | b"ruby-base" => Tag::Inline,
            b"s" => Tag::Space,
            b"tab" => Tag::Tab,
            b"line-break" => Tag::Break,
            _ => Tag::Other,
        };
    }
    Tag::Other
}

struct SheetDraft {
    sheet: Sheet,
    depth: usize,
    row: u32,
    /// The `table:table-column` runs read so far.
    cols: Vec<ColumnRun>,
    next_col: u32,
    /// The rows with their own size, visibility or style, and the heights fitted to the
    /// content of the others, applied when the sheet ends.
    rows: Vec<RowRun>,
}
/// `table:number-rows-repeated` rows with the same size, visibility and style.
struct RowRun {
    start: u32,
    count: u32,
    info: LineInfo,
    /// The height the writer fitted to the content of an automatic row.
    fitted: Option<f32>,
}
/// `table:number-columns-repeated` columns with the same style.
struct ColumnRun {
    start: u32,
    count: u32,
    width: Option<f32>,
    hidden: bool,
    /// `table:default-cell-style-name`: the style of the column's cells without their own, and
    /// its [`Styles::key`](crate::ods_style::Styles::key) for the cells that have one.
    style: StyleId,
    key: u32,
}
struct RowDraft {
    depth: usize,
    repeat: u32,
    col: u32,
    cells: Vec<RepeatedCell>,
    /// The row's size and visibility; its `style` is the row's default cell style.
    info: LineInfo,
    /// The [`Styles::key`](crate::ods_style::Styles::key) of the row's default cell style.
    key: u32,
    fitted: Option<f32>,
}
struct RepeatedCell {
    col: u32,
    repeat: u32,
    width: u32,
    height: u32,
    cell: Cell,
    formula: Option<CellFormula>,
}
/// A cell's `table:formula`, translated once every sheet and name is known.
#[derive(Clone)]
struct CellFormula {
    src: Arc<str>,
    /// `table:number-matrix-columns-spanned` / `-rows-spanned`: an array formula's size.
    matrix: Option<(u32, u32)>,
    /// Whether the cell has a saved result (else it shows a placeholder unless the formula imports).
    cached: bool,
}
struct PendingFormula {
    sheet: usize,
    pos: CellRef,
    formula: CellFormula,
}
/// A named range (`table:cell-range-address`) or named expression (`table:expression`).
struct PendingName {
    name: String,
    scope: Option<usize>,
    expression: String,
    base: Base,
}
struct CellDraft {
    depth: usize,
    repeat: u32,
    width: u32,
    height: u32,
    covered: bool,
    kind: Option<String>,
    /// `office:value-type` is `string` (or, of an error, absent): LibreOffice writes an error
    /// result as a string, `calcext:value-type` saying it is an error.
    string_typed: bool,
    raw: Option<String>,
    formula: Option<String>,
    matrix: Option<(u32, u32)>,
    text: String,
    paragraph: Option<usize>,
    paragraphs: usize,
    paragraph_started: bool,
    pending_space: bool,
    /// `table:style-name`.
    style: Option<String>,
}
struct Import {
    wb: Workbook,
    report: ReadReport,
    stack: Vec<Tag>,
    sheet: Option<SheetDraft>,
    row: Option<RowDraft>,
    cell: Option<CellDraft>,
    cells: u64,
    merges: u64,
    text: u64,
    /// Formatted empty cells kept, and row and column entries, against their budgets.
    blanks: u64,
    lines: u64,
    /// The style derived from a cell style with a value type's default format.
    formats: Vec<(&'static str, StyleId, StyleId)>,
    styles: Styles,
    /// The depth and origin of the style section (`office:styles`…) being read.
    style_section: Option<(usize, Origin)>,
    spreadsheet_seen: bool,
    /// Each sheet's name as the document writes it, then as imported.
    sheet_names: Vec<(String, String)>,
    formulas: Vec<PendingFormula>,
    names: Vec<PendingName>,
    /// DataPilot reports (`table:target-range-address` as written), not imported.
    pilots: Vec<String>,
    /// The document's text matching and comparisons (`table:calculation-settings`).
    matching: Matching,
    /// Whether the document's null date (`table:null-date`) is neither Excel date system's day 0:
    /// its formulas count days from a day Excel's never do.
    foreign_epoch: bool,
    /// The named expressions the import leaves out, as (scope, name in lower case): a formula
    /// using one keeps its saved result, even where a name of another scope has the spelling.
    unimported: std::collections::HashSet<(Option<usize>, String)>,
    /// The imported workbook names (in lower case) each sheet can't use, because they read a name
    /// of that sheet's the import leaves out (see [`shadowed_names`]): a formula using one on that
    /// sheet keeps its saved result too.
    shadowed: std::collections::HashMap<String, SheetSet>,
}

/// The document formulas are translated in: each sheet's name as written and as imported, and
/// its named expressions' names and expressions, in lower case.
struct Doc<'a> {
    sheets: &'a [(String, String)],
    names: &'a str,
    /// The names Excel holds for the named expressions it can't hold as written, by the written
    /// name in lower case (see [`crate::import_rules::excel_names`]).
    renames: &'a std::collections::HashMap<String, String>,
    /// The sheet the formula is on, `None` for a named expression, which is read wherever it is used.
    at: Option<usize>,
    /// Every named expression the document declares, as (scope, written name in lower case).
    declared: &'a std::collections::HashSet<(Option<usize>, String)>,
    /// The names the import leaves out, and the workbook names a sheet can't use (see
    /// [`Import::unimported`] and [`Import::shadowed`]); empty while the names are translated.
    unimported: &'a std::collections::HashSet<(Option<usize>, String)>,
    shadowed: &'a std::collections::HashMap<String, SheetSet>,
}

impl Sheets for Doc<'_> {
    fn sheet(&self, written: &str) -> String {
        self.sheets.iter().find(|(w, _)| w == written).map_or_else(|| written.to_string(), |(_, n)| n.clone())
    }
    fn repaired(&self) -> bool {
        self.sheets.iter().any(|(written, kept)| written != kept)
    }
    fn names_mention(&self, name: &str) -> bool {
        self.names.contains(name)
    }
    fn excel_name(&self, written: &str) -> Option<String> {
        if self.renames.is_empty() { None } else { self.renames.get(&written.to_lowercase()).cloned() }
    }
    fn indirect_name(&self, qualifier: Option<&str>, written: &str) -> Result<Option<IndirectName>, Unsupported> {
        let spelling = written.to_lowercase();
        let declares = |scope: Option<usize>| self.declared.contains(&(scope, spelling.clone()));
        // The sheet whose own name the text means (`None`: a workbook name), as LibreOffice finds it:
        // a qualifier names a sheet in any case and only that sheet's names count; without one the
        // formula's own sheet's name hides the workbook's.
        let scope = match qualifier {
            Some(q) => {
                let Some(sheet) = self.sheets.iter().position(|(w, _)| w.to_lowercase() == q.to_lowercase()) else { return Ok(None) };
                if !declares(Some(sheet)) {
                    // LibreOffice finds only that sheet's own names, so a workbook name through a sheet
                    // (`Sheet2.Rate`) is no name there and no cell either (#REF!), where Excel reads the
                    // workbook's: the saved result is kept. A cell-like spelling is the cell.
                    let global = declares(None) && !crate::import_rules::reads_as_reference(written);
                    return if global { Err(Unsupported("INDIRECT text qualifying a workbook-level name with a sheet")) } else { Ok(None) };
                }
                Some(sheet)
            }
            None => match self.at {
                Some(at) if declares(Some(at)) => Some(at),
                _ if declares(None) => None,
                Some(_) => return Ok(None),
                // A named expression is read on whichever sheet uses it: a name only some sheets have
                // is the cell on the others, which no one rewritten text can say.
                None if self.excel_name(written).is_some() && self.declared.iter().any(|(scope, name)| scope.is_some() && *name == spelling) => {
                    return Err(Unsupported("INDIRECT text in a named expression naming a renamed name only some sheets have"));
                }
                None => return Ok(None),
            },
        };
        let renamed = self.excel_name(written);
        let name = renamed.clone().unwrap_or_else(|| written.to_string());
        let key = name.to_ascii_lowercase();
        let left_out = |scope| self.unimported.contains(&(scope, key.clone()));
        let hidden = scope.is_none() && self.at.is_some_and(|at| self.shadowed.get(&key).is_some_and(|sheets| sheets.contains(at)));
        if left_out(scope) || hidden {
            return Err(Unsupported(LEFT_OUT));
        }
        Ok(Some(IndirectName {
            name,
            renamed: renamed.is_some(),
            sheet: qualifier.and(scope).and_then(|sheet| self.sheets.get(sheet)).map(|(_, kept)| kept.clone()),
        }))
    }
}

impl Import {
    fn new() -> Self {
        let mut wb = Workbook::new();
        wb.sheets.clear();
        let mut report = ReadReport::default();
        report.warn(IMPORTED);
        Self {
            wb,
            report,
            stack: vec![],
            sheet: None,
            row: None,
            cell: None,
            cells: 0,
            merges: 0,
            text: 0,
            blanks: 0,
            lines: 0,
            formats: vec![],
            styles: Styles::new(),
            style_section: None,
            spreadsheet_seen: false,
            sheet_names: vec![],
            formulas: vec![],
            names: vec![],
            pilots: vec![],
            matching: Matching::default(),
            foreign_epoch: false,
            unimported: std::collections::HashSet::new(),
            shadowed: std::collections::HashMap::new(),
        }
    }

    /// Reads a document whose root must be `root` (`Document` or `Flat`).
    fn parse(&mut self, bytes: &[u8], root: Tag) -> Result<(), IoError> {
        let mut reader = NsReader::from_reader(bytes);
        let mut root_seen = false;
        loop {
            match reader.read_event().map_err(xml_error)? {
                Event::Start(e) => {
                    if self.stack.len() >= crate::xml::MAX_DEPTH {
                        return Err(xml_error("XML nested too deeply"));
                    }
                    if self.stack.is_empty() {
                        if root_seen || tag(&mut reader, &e) != root {
                            return Err(xml_error("expected one office:document-content (or flat office:document) root"));
                        }
                        root_seen = true;
                    }
                    self.start(&mut reader, &e)?;
                }
                Event::Empty(e) => {
                    if self.stack.is_empty() {
                        return Err(xml_error("expected office:document-content with a spreadsheet"));
                    }
                    self.start(&mut reader, &e)?;
                    self.end()?;
                }
                Event::End(_) => self.end()?,
                Event::Text(t) => {
                    let text = t.xml_content(XmlVersion::Explicit1_0).map_err(xml_error)?;
                    if self.stack.is_empty() && !text.trim().is_empty() {
                        return Err(xml_error("text outside the document"));
                    }
                    self.append(&text)?;
                }
                Event::CData(t) => self.append(&t.decode().map_err(xml_error)?)?,
                Event::GeneralRef(r) => {
                    let ch = match r.resolve_char_ref().map_err(xml_error)? {
                        Some(c) => c,
                        None => match r.as_ref() {
                            b"amp" => '&',
                            b"lt" => '<',
                            b"gt" => '>',
                            b"apos" => '\'',
                            b"quot" => '"',
                            _ => return Err(xml_error("unsupported XML entity")),
                        },
                    };
                    self.append(ch.encode_utf8(&mut [0; 4]))?;
                }
                Event::DocType(_) => return Err(xml_error("DOCTYPE is not supported")),
                Event::Eof => break,
                _ => {}
            }
        }
        if !root_seen || !self.stack.is_empty() || (!self.spreadsheet_seen && root != Tag::StylesRoot) {
            return Err(xml_error("incomplete ODS document"));
        }
        Ok(())
    }

    fn start(&mut self, reader: &mut NsReader<&[u8]>, e: &BytesStart<'_>) -> Result<(), IoError> {
        let depth = self.stack.len();
        if let Some((section, origin)) = self.style_section
            && depth > section
        {
            self.styles.start(reader, e, depth, origin)?;
            self.stack.push(Tag::Other);
            return Ok(());
        }
        let t = tag(reader, e);
        match t {
            Tag::Styles(origin) if depth == 1 => {
                // The automatic styles of styles.xml are a part's own (its data styles serve its
                // common styles); a flat document's one file holds every style.
                let flat = self.stack.first() == Some(&Tag::Flat);
                self.styles.flat |= flat;
                let origin = if self.stack.first() == Some(&Tag::StylesRoot) && origin == Origin::Automatic { Origin::Part } else { origin };
                self.style_section = Some((depth, origin));
            }
            // Every style is known before the body: cells without a style of their own use the
            // document's default cell style, the workbook's Normal style.
            Tag::Body if depth == 1 => {
                self.wb.styles = StyleTable::with_default(self.styles.normal());
                self.formats.clear();
            }
            Tag::Spreadsheet if matches!(self.stack[..], [Tag::Document | Tag::Flat, Tag::Body]) => {
                if self.spreadsheet_seen {
                    return Err(xml_error("multiple spreadsheets in one document"));
                }
                self.spreadsheet_seen = true;
            }
            Tag::Table if matches!(self.stack[..], [Tag::Document | Tag::Flat, Tag::Body, Tag::Spreadsheet]) => self.start_table(reader, e, depth)?,
            Tag::Column if self.sheet.is_some() && self.row.is_none() => self.start_column(reader, e)?,
            Tag::Named if self.spreadsheet_seen && self.cell.is_none() => {
                let name = attr(reader, e, TABLE, b"name")?.unwrap_or_default();
                let expression = match attr(reader, e, TABLE, b"cell-range-address")? {
                    Some(range) => format!("[{range}]"),
                    None => attr(reader, e, TABLE, b"expression")?.unwrap_or_default(),
                };
                let base = attr(reader, e, TABLE, b"base-cell-address")?.and_then(|b| crate::ods_formula::base_cell(&b)).unwrap_or_default();
                if self.names.len() >= MAX_NAMES {
                    return Err(IoError::TooLarge("ODS defines more than 65536 names".into()));
                }
                let scope = self.sheet.as_ref().map(|_| self.wb.sheets.len());
                self.names.push(PendingName { name, scope, expression, base });
            }
            Tag::CalcSettings | Tag::NullDate => self.calc_settings(t, reader, e)?,
            Tag::DataPilot if self.spreadsheet_seen && self.pilots.len() < MAX_NAMES => {
                if let Some(range) = attr(reader, e, TABLE, b"target-range-address")? {
                    self.pilots.push(range);
                }
            }
            Tag::Row if self.sheet.as_ref().is_some_and(|s| self.stack.get(s.depth + 1..).is_some_and(|p| p.iter().all(|t| *t == Tag::Rows))) => {
                self.start_row(reader, e, depth)?;
            }
            Tag::Cell | Tag::Covered if self.row.as_ref().is_some_and(|r| depth == r.depth + 1) => {
                let office = attr(reader, e, OFFICE, b"value-type")?;
                let string_typed = office.as_deref().is_none_or(|t| t == "string");
                let kind = if attr(reader, e, CALC_EXT, b"value-type")?.as_deref() == Some("error") { Some("error".into()) } else { office };
                let key: &[u8] = match kind.as_deref() {
                    Some("boolean") => b"boolean-value",
                    Some("date") => b"date-value",
                    Some("time") => b"time-value",
                    Some("string") => b"string-value",
                    _ => b"value",
                };
                self.cell = Some(CellDraft {
                    depth,
                    repeat: positive(attr(reader, e, TABLE, b"number-columns-repeated")?, 1, u32::MAX)?,
                    width: positive(attr(reader, e, TABLE, b"number-columns-spanned")?, 1, MAX_COLS)?,
                    height: positive(attr(reader, e, TABLE, b"number-rows-spanned")?, 1, MAX_ROWS)?,
                    covered: t == Tag::Covered,
                    kind,
                    string_typed,
                    raw: attr(reader, e, OFFICE, key)?,
                    formula: attr(reader, e, TABLE, b"formula")?,
                    matrix: match (attr(reader, e, TABLE, b"number-matrix-columns-spanned")?, attr(reader, e, TABLE, b"number-matrix-rows-spanned")?)
                    {
                        (None, None) => None,
                        (cols, rows) => Some((positive(cols, 1, MAX_COLS)?, positive(rows, 1, MAX_ROWS)?)),
                    },
                    text: String::new(),
                    paragraph: None,
                    paragraphs: 0,
                    paragraph_started: false,
                    pending_space: false,
                    style: attr(reader, e, TABLE, b"style-name")?,
                });
            }
            Tag::Paragraph if self.cell.as_ref().is_some_and(|c| depth == c.depth + 1) => {
                if let Some(cell) = self.cell.as_mut() {
                    if cell.paragraphs > 0 {
                        push_text(&mut cell.text, "\n")?;
                    }
                    cell.paragraph = Some(depth);
                    cell.paragraphs += 1;
                    cell.paragraph_started = false;
                    cell.pending_space = false;
                }
            }
            Tag::Space | Tag::Tab | Tag::Break if self.captures_text() => {
                let count = if t == Tag::Space {
                    let n = attr(reader, e, TEXT, b"c")?.map(|s| s.parse::<u32>()).transpose().map_err(xml_error)?.unwrap_or(1);
                    if n as usize > MAX_CELL_TEXT {
                        return Err(IoError::TooLarge("ODS repeated spaces exceed 1 MiB".into()));
                    }
                    n
                } else {
                    1
                };
                let value = match t {
                    Tag::Space => " ",
                    Tag::Tab => "\t",
                    _ => "\n",
                };
                if let Some(cell) = self.cell.as_mut() {
                    if cell.pending_space {
                        push_text(&mut cell.text, " ")?;
                        cell.pending_space = false;
                    }
                    cell.paragraph_started = true;
                    if cell.text.len().saturating_add(count as usize) > MAX_CELL_TEXT {
                        return Err(IoError::TooLarge("ODS cell text exceeds 1 MiB".into()));
                    }
                    for _ in 0..count {
                        cell.text.push_str(value);
                    }
                }
            }
            _ => {}
        }
        self.stack.push(t);
        Ok(())
    }

    /// `table:calculation-settings` (ODF 1.3 part 3, 9.4.1), whose attributes say how the
    /// document's functions match text, compare it and round, and its `table:null-date` child.
    fn calc_settings(&mut self, t: Tag, reader: &mut NsReader<&[u8]>, e: &BytesStart<'_>) -> Result<(), IoError> {
        match (t, &self.stack[..]) {
            (Tag::CalcSettings, [Tag::Document | Tag::Flat, Tag::Body, Tag::Spreadsheet]) => {
                let mut flag = |name: &[u8], default: bool| -> Result<bool, IoError> {
                    Ok(attr(reader, e, TABLE, name)?.map_or(default, |v| matches!(v.trim(), "true" | "1")))
                };
                let d = Matching::default();
                self.matching = Matching {
                    regex: flag(b"use-regular-expressions", d.regex)?,
                    wildcards: flag(b"use-wildcards", d.wildcards)?,
                    whole_cell: flag(b"search-criteria-must-apply-to-whole-cell", d.whole_cell)?,
                    // Part 3, 19.594 (default true).
                    case_sensitive: flag(b"case-sensitive", d.case_sensitive)?,
                };
                // HOST-PRECISION-AS-SHOWN (part 3, 19.696, default false): calculations use the
                // values as displayed, Excel's "Set precision as displayed".
                self.wb.calc.precision_as_displayed = flag(b"precision-as-shown", false)?;
            }
            // Numeric date 0 (part 3, 9.4.2; `table:date-value` 19.618, default 1899-12-30): Excel's
            // 1900 date system counts from that day (from March 1900), its 1904 system from
            // 1904-01-01. Any other day has no Excel date system.
            (Tag::NullDate, [Tag::Document | Tag::Flat, Tag::Body, Tag::Spreadsheet, Tag::CalcSettings]) => {
                let value = attr(reader, e, TABLE, b"date-value")?;
                let day = value.as_deref().map_or("1899-12-30", |v| v.trim().strip_suffix("T00:00:00").unwrap_or(v.trim()));
                self.foreign_epoch = false;
                match day {
                    "1899-12-30" => self.wb.date_system = DateSystem::D1900,
                    "1904-01-01" => self.wb.date_system = DateSystem::D1904,
                    _ => self.foreign_epoch = true,
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// `table:table`: a sheet, with its name repaired if Excel forbids it, and its table style.
    fn start_table(&mut self, reader: &mut NsReader<&[u8]>, e: &BytesStart<'_>, depth: usize) -> Result<(), IoError> {
        if self.wb.sheets.len() >= MAX_SHEETS {
            return Err(IoError::TooLarge("ODS contains more than 1024 sheets".into()));
        }
        let original = attr(reader, e, TABLE, b"name")?.unwrap_or_default();
        let name = crate::import_rules::readable_sheet_name(&self.wb, &original, self.wb.sheets.len());
        if name != original {
            self.report.warn(crate::import_rules::RENAMED);
        }
        self.sheet_names.push((original, name.clone()));
        let mut sheet = Sheet::new(name);
        if let Some(style) = attr(reader, e, TABLE, b"style-name")?.and_then(|s| self.styles.table(&s)) {
            sheet.visibility = if style.display { Visibility::Visible } else { Visibility::Hidden };
            sheet.tab_color = style.tab_color;
        }
        self.sheet = Some(SheetDraft { sheet, depth, row: 0, cols: vec![], next_col: 0, rows: vec![] });
        Ok(())
    }

    /// `table:table-column`: a run of columns with their width, visibility and default cell style.
    fn start_column(&mut self, reader: &mut NsReader<&[u8]>, e: &BytesStart<'_>) -> Result<(), IoError> {
        let count = positive(attr(reader, e, TABLE, b"number-columns-repeated")?, 1, u32::MAX)?;
        let width = attr(reader, e, TABLE, b"style-name")?.and_then(|s| self.styles.column(&s)).and_then(|c| c.width);
        let hidden = matches!(attr(reader, e, TABLE, b"visibility")?.as_deref(), Some("collapse" | "filter"));
        let key = match attr(reader, e, TABLE, b"default-cell-style-name")? {
            Some(name) => self.styles.key(&name),
            None => 0,
        };
        let style = self.styles.cascade([key, 0, 0], &mut self.wb.styles);
        if let Some(sheet) = self.sheet.as_mut() {
            let start = sheet.next_col;
            let count = count.min(MAX_COLS.saturating_sub(start));
            sheet.next_col = start.saturating_add(count);
            if count > 0 && sheet.cols.len() < MAX_COLS as usize {
                sheet.cols.push(ColumnRun { start, count, width, hidden, style, key });
            }
        }
        Ok(())
    }

    /// `table:table-row`: its height, visibility and default cell style.
    fn start_row(&mut self, reader: &mut NsReader<&[u8]>, e: &BytesStart<'_>, depth: usize) -> Result<(), IoError> {
        let mut info = LineInfo::default();
        let mut fitted = None;
        if let Some(style) = attr(reader, e, TABLE, b"style-name")?.and_then(|s| self.styles.row(&s)) {
            // A height LibreOffice fitted to the content is automatic, as in Excel.
            if style.optimal {
                fitted = style.height;
            } else {
                info.size = style.height;
                info.custom = style.height.is_some();
            }
        }
        info.hidden = matches!(attr(reader, e, TABLE, b"visibility")?.as_deref(), Some("collapse" | "filter"));
        let mut key = 0;
        if let Some(name) = attr(reader, e, TABLE, b"default-cell-style-name")? {
            key = self.styles.key(&name);
            let id = self.styles.cascade([key, 0, 0], &mut self.wb.styles);
            info.style = (id != StyleId::DEFAULT).then_some(id);
        }
        let repeat = positive(attr(reader, e, TABLE, b"number-rows-repeated")?, 1, u32::MAX)?;
        self.row = Some(RowDraft { depth, repeat, col: 0, cells: vec![], info, key, fitted });
        Ok(())
    }

    fn captures_text(&self) -> bool {
        self.cell
            .as_ref()
            .and_then(|c| c.paragraph)
            .is_some_and(|depth| self.stack.get(depth..).is_some_and(|p| p.iter().all(|t| matches!(t, Tag::Paragraph | Tag::Inline))))
    }

    fn append(&mut self, text: &str) -> Result<(), IoError> {
        if self.style_section.is_some_and(|(d, _)| self.stack.len() > d) {
            self.styles.text(text);
            return Ok(());
        }
        if self.captures_text()
            && let Some(cell) = self.cell.as_mut()
        {
            // ODF 1.3 section 6.1.2: collapse XML whitespace across inline runs,
            // but expand explicit text:s/tab/line-break elements separately.
            for ch in text.chars() {
                if matches!(ch, ' ' | '\t' | '\n' | '\r') {
                    cell.pending_space = cell.paragraph_started;
                } else {
                    if cell.pending_space {
                        push_text(&mut cell.text, " ")?;
                        cell.pending_space = false;
                    }
                    push_text(&mut cell.text, ch.encode_utf8(&mut [0; 4]))?;
                    cell.paragraph_started = true;
                }
            }
        }
        Ok(())
    }

    fn end(&mut self) -> Result<(), IoError> {
        self.stack.pop().ok_or_else(|| xml_error("unexpected closing element"))?;
        let depth = self.stack.len();
        if let Some((section, _)) = self.style_section {
            if depth > section {
                return self.styles.end(depth);
            }
            self.style_section = None;
            return Ok(());
        }
        if let Some(cell) = self.cell.as_mut()
            && cell.paragraph == Some(depth)
        {
            cell.paragraph = None;
        }
        if self.cell.as_ref().is_some_and(|c| c.depth == depth) {
            self.finish_cell()?;
        }
        if self.row.as_ref().is_some_and(|r| r.depth == depth) {
            self.finish_row()?;
        }
        if self.sheet.as_ref().is_some_and(|s| s.depth == depth)
            && let Some(sheet) = self.sheet.take()
        {
            let sheet = self.finish_sheet(sheet);
            self.wb.sheets.push(Arc::new(sheet));
        }
        Ok(())
    }

    /// The sheet with its columns and rows: the width most of its columns have is the sheet's
    /// standard width, and each column that differs from it (in width, visibility or style)
    /// gets its own; likewise for row heights (see [`Import::finish_rows`]).
    fn finish_sheet(&mut self, draft: SheetDraft) -> Sheet {
        let mut sheet = draft.sheet;
        self.finish_rows(&mut sheet, &draft.rows, draft.row);
        let mut widths: Vec<(f32, u64)> = vec![];
        for run in draft.cols.iter().filter(|r| !r.hidden) {
            let Some(w) = run.width else { continue };
            match widths.iter_mut().find(|(x, _)| *x == w) {
                Some((_, n)) => *n += u64::from(run.count),
                None => widths.push((w, u64::from(run.count))),
            }
        }
        if let Some((w, _)) = widths.iter().max_by_key(|(_, n)| *n) {
            sheet.default_col_width = *w;
        }
        for run in &draft.cols {
            let info = LineInfo {
                size: run.width.filter(|w| *w != sheet.default_col_width),
                hidden: run.hidden,
                style: (run.style != StyleId::DEFAULT).then_some(run.style),
                ..LineInfo::default()
            };
            if info == LineInfo::default() {
                continue;
            }
            if self.lines.saturating_add(u64::from(run.count)) > MAX_LINES {
                self.report.warn(LINES_DROPPED);
                continue;
            }
            self.lines += u64::from(run.count);
            for c in run.start..run.start.saturating_add(run.count).min(MAX_COLS) {
                sheet.cols.insert(c, info);
            }
        }
        sheet
    }

    /// The rows of the sheet's `total`: when more rows have one set height than fit their
    /// content, that height is the sheet's standard height, and the rows fitted to their content
    /// keep the height the writer fitted (as an XLSX row height without `customHeight`). Each row
    /// that differs from the standard (in height, visibility or style) gets its own entry, and so
    /// does a row of the standard height with cells: its height is fixed
    /// (`style:use-optimal-row-height="false"`, ODF 1.3 part 3 §20.394), so its content must not
    /// fit it on open. Empty rows of the standard height have nothing to fit and need no entry.
    fn finish_rows(&mut self, sheet: &mut Sheet, runs: &[RowRun], total: u32) {
        let mut heights: Vec<(f32, u64)> = vec![];
        let mut set = 0u64;
        for run in runs.iter().filter(|r| !r.info.hidden) {
            let Some(h) = run.info.size else { continue };
            set += u64::from(run.count);
            match heights.iter_mut().find(|(x, _)| *x == h) {
                Some((_, n)) => *n += u64::from(run.count),
                None => heights.push((h, u64::from(run.count))),
            }
        }
        let fitted = u64::from(total).saturating_sub(set);
        let standard = heights.iter().max_by_key(|(_, n)| *n).filter(|(_, n)| *n > fitted).map(|(h, _)| *h);
        let mut with_cells = std::collections::BTreeSet::new();
        if let Some(h) = standard {
            sheet.default_row_height = h;
            with_cells.extend(sheet.cells.iter().map(|(at, _)| at.row));
        }
        for run in runs {
            let end = run.start.saturating_add(run.count).min(MAX_ROWS);
            let mut info = run.info;
            let mut fixed: Vec<u32> = vec![];
            match (standard, info.size) {
                (Some(h), Some(size)) if size == h => {
                    if info.custom {
                        fixed.extend(with_cells.range(run.start..end));
                    }
                    info.size = None;
                    info.custom = false;
                }
                (Some(_), None) => info.size = run.fitted,
                _ => {}
            }
            let lines = if info == LineInfo::default() { fixed.len() as u64 } else { u64::from(run.count) };
            if lines == 0 {
                continue;
            }
            if self.lines.saturating_add(lines) > MAX_LINES {
                self.report.warn(LINES_DROPPED);
                continue;
            }
            self.lines += lines;
            if info != LineInfo::default() {
                for r in run.start..end {
                    sheet.rows.insert(r, info);
                }
            }
            for r in fixed {
                sheet.rows.insert(r, run.info);
            }
        }
    }

    /// A cell's properties its style doesn't set come from the row's default cell style, then
    /// the column's (ODF 1.3 part 3 §16.2), so repeated cells over columns with different
    /// default styles become one run per column style.
    fn finish_cell(&mut self) -> Result<(), IoError> {
        let Some(draft) = self.cell.take() else { return Ok(()) };
        let key = draft.style.as_deref().map_or(0, |name| self.styles.key(name));
        let Some((row_key, row_style, first)) = self.row.as_ref().map(|r| (r.key, r.info.style, r.col)) else { return Ok(()) };
        let (width, height) = if draft.covered { (1, 1) } else { (draft.width, draft.height) };
        // Writers with wider sheets pad rows with repeats up to their own edge, and some keep data
        // out there. Past ours, blank cells carry nothing; the others are dropped with a warning
        // (the rest of the file is not refused for it). The repeats that fit are bounded by the
        // cell budget when the row is finished.
        let repeat = draft.repeat.min(MAX_COLS.saturating_sub(first));
        let mut cells = vec![];
        let mut kept_any = false;
        for (col, count, col_key, col_style) in self.column_runs(first, repeat) {
            let (base, by_type) = self.styles.cascade_aligned([key, row_key, col_key], &mut self.wb.styles);
            let (mut cell, cached) = self.convert(&draft, base, by_type);
            let formula = draft.formula.clone().map(|src| CellFormula { src: Arc::from(src), matrix: draft.matrix, cached });
            // An empty cell styled as its row or column needs no cell: the sheet gives it that style.
            if cell.value.is_empty() && formula.is_none() && row_style.unwrap_or(col_style) == cell.style {
                cell.style = StyleId::DEFAULT;
            }
            let kept = !cell.is_blank() || width > 1 || height > 1;
            kept_any |= kept;
            if kept && count > 0 {
                // Charge the text a row keeps before the row is done, so a row of many long cells
                // is refused as it grows; finish_row charges the row's repeats beyond the first.
                if let Value::Text(t) = &cell.value {
                    self.text = self.text.saturating_add(u64::from(count).saturating_mul(t.len() as u64));
                    if self.text > MAX_TEXT {
                        return Err(IoError::TooLarge("ODS expands beyond the import limit (64 MiB of text)".into()));
                    }
                }
                cells.push(RepeatedCell { col, repeat: count, width, height, cell, formula });
            }
        }
        if repeat < draft.repeat && kept_any {
            self.report.warn(BEYOND_EDGE);
        }
        let Some(row) = self.row.as_mut() else { return Ok(()) };
        row.cells.extend(cells);
        row.col = row.col.saturating_add(repeat);
        Ok(())
    }

    /// The `repeat` columns from `first` as runs of one column default cell style: (first
    /// column, count, style key, style). At least one run, of the first column, is returned even
    /// for no columns, so a cell past the sheet's edge is still seen.
    fn column_runs(&self, first: u32, repeat: u32) -> Vec<(u32, u32, u32, StyleId)> {
        let cols = self.sheet.as_ref().map_or(&[][..], |s| &s.cols[..]);
        let end = first.saturating_add(repeat);
        // Runs are in column order and don't overlap; columns no run declares have no style.
        let mut at = cols.partition_point(|r| r.start.saturating_add(r.count) <= first);
        let mut out: Vec<(u32, u32, u32, StyleId)> = vec![];
        let mut next = first;
        loop {
            let (stop, key, style) = match cols.get(at) {
                Some(run) if run.start <= next => {
                    at += 1;
                    (run.start.saturating_add(run.count), run.key, run.style)
                }
                Some(run) => (run.start, 0, StyleId::DEFAULT),
                None => (u32::MAX, 0, StyleId::DEFAULT),
            };
            let stop = stop.min(end);
            match out.last_mut() {
                Some(last) if last.2 == key => last.1 += stop - next,
                _ => out.push((next, stop - next, key, style)),
            }
            next = stop;
            if next >= end {
                return out;
            }
        }
    }

    /// The cell and whether it holds a saved value: a formula without one shows a placeholder,
    /// which the formula's result replaces if it imports. `base` is the cell's style, and
    /// `by_type` whether it aligns by value type.
    fn convert(&mut self, draft: &CellDraft, base: StyleId, by_type: bool) -> (Cell, bool) {
        let formula = draft.formula.is_some();
        let raw = draft.raw.as_deref();
        let mut format = None;
        let value = match draft.kind.as_deref() {
            Some("float" | "percentage" | "currency") => raw.and_then(|s| s.parse::<f64>().ok()).filter(|n| n.is_finite()).map(|n| {
                if draft.kind.as_deref() == Some("percentage") {
                    format = Some("0.00%");
                }
                Value::Number(n)
            }),
            Some("boolean") => match raw {
                Some("true" | "1") => Some(Value::Bool(true)),
                Some("false" | "0") => Some(Value::Bool(false)),
                _ => None,
            },
            // A formula's empty text result is written without a paragraph.
            Some("string") => Some(Value::Text(Arc::from(raw.unwrap_or(&draft.text)))),
            Some("void") if !formula => Some(Value::Empty),
            // An error result (calcext:value-type="error") is written as its text; the value is
            // a placeholder. Application-specific errors (`Err:502`) have no Excel equivalent.
            Some("error") => CellError::ALL.iter().find(|e| e.as_str() == draft.text).map(|e| Value::Error(*e)),
            Some("date") => raw.and_then(|s| parse_date(s, self.wb.date_system)).map(|(n, f)| {
                format = Some(f);
                Value::Number(n)
            }),
            Some("time") => raw.and_then(parse_duration).map(|n| {
                format = Some("[h]:mm:ss.000");
                Value::Number(n)
            }),
            None if !formula => Some(if draft.text.is_empty() { Value::Empty } else { Value::Text(Arc::from(draft.text.as_str())) }),
            _ => None,
        };
        let cached = value.is_some() || !draft.text.is_empty() || raw.is_some();
        let value = value.unwrap_or_else(|| {
            if cached || !formula {
                self.report.warn(UNSUPPORTED_VALUE);
            }
            let text = if !draft.text.is_empty() {
                draft.text.as_str()
            } else if let Some(raw) = raw {
                raw
            } else if formula {
                UNAVAILABLE
            } else {
                "[Unsupported ODS value]"
            };
            Value::Text(Arc::from(text))
        });
        // A value type's default format (a date's, a percentage's) applies where the cell's
        // style names no number format of its own.
        let style = match format {
            Some(code) if self.wb.styles.get(base).num_fmt.as_str() == "General" => {
                match self.formats.iter().find(|(f, b, _)| *f == code && *b == base) {
                    Some((_, _, id)) => *id,
                    None => {
                        let id = self.wb.styles.derive(base, |s| s.num_fmt = NumFmt::new(code));
                        self.formats.push((code, base, id));
                        id
                    }
                }
            }
            _ => base,
        };
        // Aligned by value type, ODF puts a string on the left and every other value type on the
        // right (ODF 1.3 part 3 §20.364). Excel's General does the same but for logical values
        // and errors, which it centres. LibreOffice types an error result as a string, which
        // stays on the left. The cached result decides for a formula.
        let aligned = match value {
            Value::Bool(_) => Some(HAlign::Right),
            Value::Error(_) => Some(if draft.string_typed { HAlign::Left } else { HAlign::Right }),
            _ => None,
        };
        let style = match aligned {
            Some(h) if by_type => self.wb.styles.derive(style, |s| s.align.h = h),
            _ => style,
        };
        (Cell { style, ..Cell::value(value) }, cached)
    }

    fn finish_row(&mut self) -> Result<(), IoError> {
        let Some(row) = self.row.take() else { return Ok(()) };
        let Some(sheet) = self.sheet.as_mut() else { return Ok(()) };
        // Rows past the sheet's last row (a larger sheet in the writing program) are clipped like
        // columns: blank ones carry nothing, the others are dropped with a warning.
        let repeat = row.repeat.min(MAX_ROWS.saturating_sub(sheet.row));
        if repeat < row.repeat && !row.cells.is_empty() {
            self.report.warn(BEYOND_EDGE);
        }
        let end = sheet.row.saturating_add(repeat);
        if repeat == 0 {
            return Ok(());
        }
        for entry in row.cells {
            let count = u64::from(entry.repeat) * u64::from(repeat);
            let mut set = !entry.cell.is_blank();
            if set && entry.cell.value.is_empty() {
                // A formatted empty cell: kept while the budget lasts, its formatting lost after.
                if self.blanks.saturating_add(count) > MAX_STYLED_BLANKS {
                    self.report.warn(BLANKS_DROPPED);
                    set = false;
                } else {
                    self.blanks += count;
                }
            } else if set {
                self.cells = self.cells.saturating_add(count);
                if let Value::Text(t) = &entry.cell.value {
                    let repeats = count.saturating_sub(u64::from(entry.repeat));
                    self.text = self.text.saturating_add(repeats.saturating_mul(t.len() as u64));
                }
            }
            let merged = entry.width > 1 || entry.height > 1;
            if merged {
                self.merges = self.merges.saturating_add(count);
            }
            if self.cells > MAX_CELLS || self.merges > MAX_MERGES || self.text > MAX_TEXT {
                return Err(IoError::TooLarge("ODS expands beyond the import limit (1 million cells, 100000 merges or 64 MiB of text)".into()));
            }
            let last_col = entry.col.checked_add(entry.repeat - 1).and_then(|c| c.checked_add(entry.width - 1));
            let last_row = end.checked_add(entry.height - 1);
            if last_col.is_none_or(|c| c >= MAX_COLS) || last_row.is_none_or(|r| r > MAX_ROWS) {
                return Err(IoError::TooLarge("ODS merged cell exceeds worksheet bounds".into()));
            }
            if merged && ((entry.repeat > 1 && entry.width > 1) || (repeat > 1 && entry.height > 1)) {
                return Err(IoError::Format("repeated ODS cells have overlapping merges".into()));
            }
            if !set && !merged && entry.formula.is_none() {
                continue;
            }
            for r in sheet.row..end {
                for c in entry.col..entry.col + entry.repeat {
                    let pos = CellRef::new(r, c);
                    if set {
                        sheet.sheet.cells.set(pos, entry.cell.clone());
                    }
                    if let Some(formula) = &entry.formula {
                        self.formulas.push(PendingFormula { sheet: self.wb.sheets.len(), pos, formula: formula.clone() });
                    }
                    if merged {
                        sheet.sheet.merges.push(RangeRef::new(pos, CellRef::new(r + entry.height - 1, c + entry.width - 1)));
                    }
                }
            }
        }
        if (row.info != LineInfo::default() || row.fitted.is_some()) && sheet.rows.len() < MAX_ROWS as usize {
            sheet.rows.push(RowRun { start: sheet.row, count: repeat, info: row.info, fitted: row.fitted });
        }
        sheet.row = end;
        Ok(())
    }

    /// Translates the named ranges, then every cell formula, once all sheets are known. What
    /// can't be translated, or calls a function GridCraft doesn't have, or uses a name the file
    /// doesn't define, keeps its saved result (one warning per reason). Both hold the model's own
    /// text (parsed and printed, as an XLSX reader leaves them): the translation quotes every
    /// sheet name and keeps Excel's `_xlpm.` parameter prefixes, which the model doesn't spell.
    fn formulas(&mut self, opts: ReadOptions<'_>) {
        let functions = opts.functions;
        let known = |name: &str| opts.known_function.map_or_else(|| (functions.is_builtin)(name), |f| f(name));
        let mut reasons: Vec<&'static str> = vec![];
        // Names Excel can't hold are renamed, in their definitions and in every formula that uses them.
        let renames = crate::import_rules::excel_names(self.names.iter().map(|n| n.name.as_str()), |n| !gridcraft_core::valid_defined_name(n));
        if !renames.is_empty() {
            self.report.warn(crate::import_rules::NAMES_RENAMED);
        }
        let declared: std::collections::HashSet<(Option<usize>, String)> = self.names.iter().map(|n| (n.scope, n.name.to_lowercase())).collect();
        let mentioned = self
            .names
            .iter()
            .flat_map(|n| [n.name.to_lowercase(), n.expression.to_lowercase()])
            .chain(renames.values().map(|n| n.to_lowercase()))
            .collect::<Vec<_>>()
            .join("\n");
        self.import_names(&mentioned, &renames, &declared, &known, &mut reasons);
        let mut legacy: Vec<(usize, Vec<CellRef>)> = vec![];
        for p in std::mem::take(&mut self.formulas) {
            let doc = Doc {
                sheets: &self.sheet_names,
                names: &mentioned,
                renames: &renames,
                at: Some(p.sheet),
                declared: &declared,
                unimported: &self.unimported,
                shadowed: &self.shadowed,
            };
            let formula = crate::ods_formula::cell_formula(&p.formula.src, &doc).and_then(|text| self.importable(&text, p.sheet, &known));
            let Some(sheet) = self.wb.sheets.get_mut(p.sheet) else { continue };
            let sheet = Arc::make_mut(sheet);
            let mut f = match formula {
                Ok(f) => f,
                Err(Unsupported(why)) => {
                    reasons.push(why);
                    if !p.formula.cached {
                        self.report.warn(UNSUPPORTED_VALUE);
                    }
                    continue;
                }
            };
            if let Some((cols, rows)) = p.formula.matrix {
                let last = CellRef::new(p.pos.row.saturating_add(rows - 1).min(MAX_ROWS - 1), p.pos.col.saturating_add(cols - 1).min(MAX_COLS - 1));
                let range = RangeRef::new(p.pos, last);
                // The other cells of the array hold only its saved results: blanked so the
                // formula fills them, as when an .xlsx is read.
                let followers: Vec<CellRef> = if u64::from(cols) * u64::from(rows) <= MAX_ARRAY_SCAN {
                    (range.start.row..=range.end.row)
                        .flat_map(|r| (range.start.col..=range.end.col).map(move |c| CellRef::new(r, c)))
                        .filter(|at| *at != p.pos && sheet.cells.get(*at).is_some())
                        .collect()
                } else {
                    sheet.cells.iter().filter(|(at, _)| *at != p.pos && range.contains(*at)).map(|(at, _)| at).collect()
                };
                for at in followers {
                    let style = sheet.style_id(at);
                    sheet.cells.set(at, Cell { value: Value::Empty, formula: None, style, py: None });
                }
                f.array = Some(range);
            }
            let cell = sheet.cells.get_mut(p.pos);
            match cell {
                Some(cell) => {
                    if !p.formula.cached {
                        cell.value = Value::Empty;
                    }
                    cell.formula = Some(Arc::new(f));
                }
                None => {
                    let style = sheet.style_id(p.pos);
                    sheet.cells.set(p.pos, Cell { style, ..Cell::formula(f) });
                }
            }
            match legacy.last_mut() {
                Some((s, cells)) if *s == p.sheet => cells.push(p.pos),
                _ => legacy.push((p.sheet, vec![p.pos])),
            }
        }
        crate::array_formula::apply_legacy(&mut self.wb, legacy, functions);
        reasons.sort_unstable();
        reasons.dedup();
        for why in reasons {
            self.report.warn(format!("Some ODS formulas kept their saved results instead of being imported: {why}."));
        }
    }

    /// Each sheet's frozen panes from the view settings, by the name the document writes.
    fn freeze(&mut self, panes: &std::collections::HashMap<String, crate::ods_settings::Frozen>) {
        for ((written, _), sheet) in self.sheet_names.iter().zip(self.wb.sheets.iter_mut()) {
            if let Some(f) = panes.get(written) {
                let sheet = Arc::make_mut(sheet);
                sheet.freeze = Some((f.rows, f.cols));
                sheet.view_top_left = CellRef::new(f.top, f.left);
            }
        }
    }

    /// The areas of the DataPilot reports, kept on their sheets as PivotTables GridCraft did not
    /// import (their cells are read as plain cells).
    fn data_pilots(&mut self) {
        for written in std::mem::take(&mut self.pilots) {
            let Some((name, range)) = crate::ods_formula::range_address(&written) else { continue };
            let Some(at) = self.sheet_names.iter().position(|(w, _)| *w == name) else { continue };
            if let Some(sheet) = self.wb.sheets.get_mut(at) {
                Arc::make_mut(sheet).unimported_pivots.push(range);
            }
        }
    }

    /// Imports the named ranges and expressions as the model's own text, recording in `reasons`
    /// why one can't be. A name is imported only when every name and function it uses is: one
    /// using a name that isn't would read `#NAME?` where the document has a value. Excel's
    /// built-in names a LibreOffice document keeps (`_xlnm.Print_Area`…) become what an XLSX read
    /// makes of them (see [`Import::built_in_name`]).
    fn import_names(
        &mut self,
        mentioned: &str,
        renames: &std::collections::HashMap<String, String>,
        declared: &std::collections::HashSet<(Option<usize>, String)>,
        known: &dyn Fn(&str) -> bool,
        reasons: &mut Vec<&'static str>,
    ) {
        let names = self.translate_names(mentioned, renames, declared, reasons);
        let NameUse { usable, shadowed } = usable_names(&names, known, reasons);
        let mut left_out = vec![];
        for ((n, expr), usable) in names.into_iter().zip(usable) {
            let Some(expr) = expr.filter(|_| usable) else {
                left_out.push((n.scope, n.name.to_ascii_lowercase()));
                continue;
            };
            let formula = gridcraft_formula::print(&crate::fmla::strip_param_prefixes(expr));
            if !self.built_in_name(&n.name, n.scope, &formula) {
                self.wb.names.push(DefinedName { name: n.name, scope: n.scope, formula, comment: String::new(), hidden: false });
            }
        }
        // A second name of a spelling the first imported is no loss.
        let imported: std::collections::HashSet<(Option<usize>, String)> =
            self.wb.names.iter().map(|d| (d.scope, d.name.to_ascii_lowercase())).collect();
        self.unimported = left_out.into_iter().filter(|spelling| !imported.contains(spelling)).collect();
        self.shadowed = shadowed;
    }

    /// Each pending name, under the name Excel holds it by (`renames`), with its expression in
    /// Excel's syntax, parsed; `None` where it can't be (the first of two names of one spelling in
    /// a scope wins). A formula uses the renamed spelling too, so a name found unusable is
    /// unusable by the one name everywhere.
    fn translate_names(
        &mut self,
        mentioned: &str,
        renames: &std::collections::HashMap<String, String>,
        declared: &std::collections::HashSet<(Option<usize>, String)>,
        reasons: &mut Vec<&'static str>,
    ) -> Vec<(PendingName, Option<Expr>)> {
        // Which names the import leaves out isn't known yet: a name's expression is read wherever
        // the name is used, and `usable_names` finds the unusable ones after.
        let (unimported, shadowed) = (std::collections::HashSet::new(), std::collections::HashMap::new());
        let doc = Doc { sheets: &self.sheet_names, names: mentioned, renames, at: None, declared, unimported: &unimported, shadowed: &shadowed };
        let mut taken = std::collections::HashSet::new();
        let mut names = vec![];
        for mut n in std::mem::take(&mut self.names) {
            let expression = n.expression.strip_prefix("of:").unwrap_or(&n.expression);
            let text = crate::ods_formula::translate(expression, &doc, Some(n.base));
            if let Some(excel) = renames.get(&n.name.to_lowercase()) {
                n.name.clone_from(excel);
            }
            let spelling = (n.scope, n.name.to_ascii_lowercase());
            let valid = !n.name.is_empty() && !taken.contains(&spelling);
            let expr = match text.map(|t| gridcraft_formula::parse(&t)) {
                Ok(Ok(e)) if valid => match self.refusal(&e) {
                    Some(why) => {
                        reasons.push(why);
                        None
                    }
                    None => Some(e),
                },
                Ok(_) => {
                    reasons.push("names Excel doesn't allow or GridCraft can't parse");
                    None
                }
                Err(Unsupported(why)) => {
                    reasons.push(why);
                    None
                }
            };
            if expr.is_some() {
                taken.insert(spelling);
            }
            names.push((n, expr));
        }
        names
    }

    /// Applies an Excel built-in name as an XLSX read does, returning whether `name` is one: a
    /// sheet's `_xlnm.Print_Area` and `_xlnm.Print_Titles` are its print settings, and
    /// `_xlnm._FilterDatabase` belongs to the autofilter (not imported from ODS).
    fn built_in_name(&mut self, name: &str, scope: Option<usize>, formula: &str) -> bool {
        let lname = name.to_ascii_lowercase();
        if !BUILT_IN_NAMES.contains(&lname.as_str()) {
            return false;
        }
        if let Some(sheet) = scope.and_then(|si| self.wb.sheets.get_mut(si)) {
            let sheet = Arc::make_mut(sheet);
            if lname == "_xlnm.print_area" {
                sheet.print.print_area = crate::read::parse_print_area(formula);
            } else if lname == "_xlnm.print_titles" {
                (sheet.print.title_rows, sheet.print.title_cols) = crate::read::parse_print_titles(formula);
            }
        }
        true
    }

    /// Why the document means `expr` otherwise than Excel would, if it does: its null date has no
    /// Excel date system, or it matches or compares text differently (see [`Matching::differs`]).
    fn refusal(&self, expr: &Expr) -> Option<&'static str> {
        if self.foreign_epoch {
            return Some(FOREIGN_EPOCH);
        }
        self.matching.differs(expr)
    }

    /// The formula for Excel-syntax `text` on the sheet at `sheet`, if GridCraft parses it and
    /// has every function and name it uses there, in the model's spelling (LET/LAMBDA parameters
    /// without `_xlpm.`).
    fn importable(&self, text: &str, sheet: usize, known: &dyn Fn(&str) -> bool) -> Result<Formula, Unsupported> {
        let f = Formula::new(text);
        let expr = f.expr.as_deref().ok_or(Unsupported("syntax GridCraft can't parse yet"))?;
        if let Some(why) = self.refusal(expr) {
            return Err(Unsupported(why));
        }
        let mut bad = None;
        let mut prefixed = false;
        expr.walk(&mut |e| match e {
            Expr::Call(name, _) if !known(name) && bad.is_none() => bad = self.name_problem(name, sheet, "functions GridCraft doesn't have"),
            Expr::Name(name) if name.to_ascii_lowercase().starts_with("_xlpm.") => prefixed = true,
            Expr::Name(name) if bad.is_none() => bad = self.name_problem(name, sheet, "names the file doesn't define"),
            _ => {}
        });
        match bad {
            Some(why) => Err(why),
            None if prefixed => Ok(Formula::from_expr(crate::fmla::strip_param_prefixes(Expr::clone(expr)))),
            None => Ok(f),
        }
    }

    /// Why a formula on `sheet` can't use the name or function `name`, if it can't: the import
    /// left out a name of that spelling in the sheet's scope (which hides the workbook's) or the
    /// workbook's, a workbook name reads a name of the sheet's the import left out, or the file
    /// defines none (`absent`).
    fn name_problem(&self, name: &str, sheet: usize, absent: &'static str) -> Option<Unsupported> {
        let spelling = name.to_ascii_lowercase();
        let left_out = |scope| self.unimported.contains(&(scope, spelling.clone()));
        if left_out(Some(sheet)) {
            return Some(Unsupported(LEFT_OUT));
        }
        match self.wb.name(name, sheet) {
            Some(def) if def.scope.is_none() && self.shadowed.get(&spelling).is_some_and(|sheets| sheets.contains(sheet)) => {
                Some(Unsupported(LEFT_OUT))
            }
            Some(_) => None,
            None => Some(Unsupported(if left_out(None) { LEFT_OUT } else { absent })),
        }
    }
}

/// What the import can make of the translated names.
struct NameUse {
    /// By position, whether each can be imported: it translated and uses, directly or through
    /// other such names, only names and functions that exist.
    usable: Vec<bool>,
    /// The imported workbook names (in lower case) each sheet can't use (see [`shadowed_names`]).
    shadowed: std::collections::HashMap<String, SheetSet>,
}

/// A set of sheets, by index.
#[derive(Default)]
struct SheetSet(Vec<u64>);

impl SheetSet {
    fn insert(&mut self, sheet: usize) {
        let word = sheet / 64;
        if self.0.len() <= word {
            self.0.resize(word + 1, 0);
        }
        if let Some(bits) = self.0.get_mut(word) {
            *bits |= 1 << (sheet % 64);
        }
    }

    fn contains(&self, sheet: usize) -> bool {
        self.0.get(sheet / 64).is_some_and(|bits| (bits >> (sheet % 64)) & 1 == 1)
    }
}

/// Which of the translated `names` can be imported: those that translated and use, directly or
/// through other such names, only names and functions that exist. A name is read wherever it is
/// used: a workbook name sees only the workbook's names, a sheet's name its own sheet's and the
/// workbook's (and see [`shadowed_names`] for a workbook name used on a sheet).
fn usable_names(names: &[(PendingName, Option<Expr>)], known: &dyn Fn(&str) -> bool, reasons: &mut Vec<&'static str>) -> NameUse {
    use std::collections::{HashMap, HashSet};
    // What each name uses, in lower case: names, and functions GridCraft doesn't have (a LAMBDA
    // name is called as one). LET/LAMBDA parameters (`_xlpm.`) are the expression's own.
    let uses: Vec<Vec<String>> = names
        .iter()
        .map(|(_, expr)| {
            let mut used = vec![];
            if let Some(expr) = expr {
                expr.walk(&mut |e| match e {
                    Expr::Call(name, _) if !known(name) => used.push(name.to_ascii_lowercase()),
                    Expr::Name(name) if !name.to_ascii_lowercase().starts_with("_xlpm.") => used.push(name.to_ascii_lowercase()),
                    _ => {}
                });
            }
            used
        })
        .collect();
    let mut usable: Vec<bool> = names.iter().map(|(_, expr)| expr.is_some()).collect();
    let spellings: Vec<String> = names.iter().map(|(n, _)| n.name.to_ascii_lowercase()).collect();
    // Who defines each name formulas can use (built-in names become settings), and who uses each.
    let mut defining: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, ((_, expr), spelling)) in names.iter().zip(&spellings).enumerate() {
        if expr.is_some() && !BUILT_IN_NAMES.contains(&spelling.as_str()) {
            defining.entry(spelling).or_default().push(i);
        }
    }
    let mut users: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, used) in uses.iter().enumerate() {
        for u in used {
            users.entry(u).or_default().push(i);
        }
    }
    // Every spelling a scope declares, usable or not: a sheet's own name hides the workbook's.
    let declared: HashSet<(Option<usize>, &str)> = names.iter().zip(&spellings).map(|((n, _), spelling)| (n.scope, spelling.as_str())).collect();
    let defined = |used: &str, scope: Option<usize>, usable: &[bool]| {
        let at = if scope.is_some() && declared.contains(&(scope, used)) { scope } else { None };
        defining
            .get(used)
            .is_some_and(|candidates| candidates.iter().any(|&j| usable.get(j) == Some(&true) && names.get(j).is_some_and(|(d, _)| d.scope == at)))
    };
    let mut queue: Vec<usize> = (0..names.len()).collect();
    while let Some(i) = queue.pop() {
        let (Some((name, _)), Some(used)) = (names.get(i), uses.get(i)) else { continue };
        if usable.get(i) != Some(&true) || used.iter().all(|u| defined(u, name.scope, &usable)) {
            continue;
        }
        if let Some(slot) = usable.get_mut(i) {
            *slot = false;
        }
        reasons.push(USES_LEFT_OUT);
        // Whoever used it may have lost a name.
        if let Some(dependents) = spellings.get(i).and_then(|spelling| users.get(spelling.as_str())) {
            queue.extend(dependents);
        }
    }
    let shadowed = shadowed_names(names, &spellings, &users, &mut usable, reasons);
    NameUse { usable, shadowed }
}

/// The imported workbook names each sheet can't use, by name in lower case. A name is read
/// wherever it is used (ODF 1.3 part 4, 5.11): the names a workbook name uses are looked up
/// from the sheet it is used on, where one that sheet declares is its own, even when the import
/// left it out, so the workbook name reads the workbook's there. A spelling a sheet declares
/// without a name the import keeps is gone on that sheet, and so is every workbook name using it
/// (directly or through other names), as well as the sheet's own names using any; the other
/// sheets still read the workbook's. A sheet's own name hides the workbook's of its spelling.
///
/// `spellings` are the `names` in lower case, `users` the names using each spelling, and the
/// sheets' own names found unusable are cleared in `usable`.
fn shadowed_names(
    names: &[(PendingName, Option<Expr>)],
    spellings: &[String],
    users: &std::collections::HashMap<&str, Vec<usize>>,
    usable: &mut [bool],
    reasons: &mut Vec<&'static str>,
) -> std::collections::HashMap<String, SheetSet> {
    use std::collections::{HashMap, HashSet};
    let at = |i: usize| spellings.get(i).map_or("", String::as_str);
    let mut own: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, (n, _)) in names.iter().enumerate() {
        if let Some(sheet) = n.scope {
            own.entry(sheet).or_default().push(i);
        }
    }
    let mut shadowed: HashMap<String, SheetSet> = HashMap::new();
    for (sheet, own) in own {
        let declared: HashSet<&str> = own.iter().map(|&i| at(i)).collect();
        let kept: HashSet<&str> = own.iter().filter(|&&i| usable.get(i) == Some(&true)).map(|&i| at(i)).collect();
        let mut gone: HashSet<&str> = declared.difference(&kept).copied().collect();
        let mut queue: Vec<&str> = gone.iter().copied().collect();
        while let Some(spelling) = queue.pop() {
            for &user in users.get(spelling).into_iter().flatten() {
                let (Some((n, _)), Some(slot)) = (names.get(user), usable.get_mut(user)) else { continue };
                let name = at(user);
                match n.scope {
                    // The sheet's own name, which can't be kept.
                    Some(s) if s == sheet && *slot => {
                        *slot = false;
                        reasons.push(USES_LEFT_OUT);
                    }
                    // A workbook name, unless the sheet has a name of that spelling to read instead.
                    None if *slot && !declared.contains(name) => {
                        shadowed.entry(name.to_string()).or_default().insert(sheet);
                    }
                    _ => continue,
                }
                if gone.insert(name) {
                    queue.push(name);
                }
            }
        }
    }
    shadowed
}

fn push_text(out: &mut String, text: &str) -> Result<(), IoError> {
    if out.len().saturating_add(text.len()) > MAX_CELL_TEXT {
        return Err(IoError::TooLarge("ODS cell text exceeds 1 MiB".into()));
    }
    out.push_str(text);
    Ok(())
}

/// Support unzoned ISO calendar dates and times, as serials of the date system `sys` (the
/// document's null date). A timezone, invalid date or out-of-range calendar year remains text;
/// never silently discard an offset or roll an invalid date.
fn parse_date(s: &str, sys: DateSystem) -> Option<(f64, &'static str)> {
    let (date, time) = s.split_once('T').map_or((s, None), |(d, t)| (d, Some(t)));
    if date.len() != 10 || date.get(4..5)? != "-" || date.get(7..8)? != "-" {
        return None;
    }
    let year: i32 = digits(date.get(..4)?)?.parse().ok()?;
    let month: u32 = digits(date.get(5..7)?)?.parse().ok()?;
    let day: u32 = digits(date.get(8..10)?)?.parse().ok()?;
    if !(1900..=9999).contains(&year) || !(1..=12).contains(&month) || day == 0 || day > days_in_month(year, month) {
        return None;
    }
    let day = serial_from_ymd(sys, year.into(), month.into(), day.into())?;
    let Some(time) = time else { return Some((day, "yyyy-mm-dd")) };
    let mut parts = time.split(':');
    let h = parts.next()?;
    let m = parts.next()?;
    let sec = parts.next()?;
    if parts.next().is_some() || h.len() != 2 || m.len() != 2 || sec.split('.').next()?.len() != 2 {
        return None;
    }
    let h: u32 = digits(h)?.parse().ok()?;
    let m: u32 = digits(m)?.parse().ok()?;
    let sec = decimal(sec)?;
    if h >= 24 || m >= 60 || sec >= 60.0 {
        return None;
    }
    Some((day + (h as f64 * 3600.0 + m as f64 * 60.0 + sec) / 86400.0, "yyyy-mm-dd hh:mm:ss.000"))
}

fn digits(s: &str) -> Option<&str> {
    (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())).then_some(s)
}
fn decimal(s: &str) -> Option<f64> {
    match s.split_once('.') {
        Some((a, b)) => {
            digits(a)?;
            digits(b)?;
        }
        None => {
            digits(s)?;
        }
    }
    s.parse::<f64>().ok().filter(|n| n.is_finite())
}

/// A bounded ISO day/time duration. Calendar years/months cannot be represented as days.
fn parse_duration(s: &str) -> Option<f64> {
    let (negative, s) = s.strip_prefix('-').map_or((false, s), |s| (true, s));
    let mut rest = s.strip_prefix('P')?;
    let mut total = 0.0;
    let mut any = false;
    if let Some((days, tail)) = rest.split_once('D') {
        total += digits(days)?.parse::<f64>().ok()? * 86400.0;
        rest = tail;
        any = true;
    }
    if let Some(mut time) = rest.strip_prefix('T') {
        let mut time_any = false;
        for (unit, factor) in [('H', 3600.0), ('M', 60.0), ('S', 1.0)] {
            if let Some((n, tail)) = time.split_once(unit) {
                let value = if unit == 'S' { decimal(n)? } else { digits(n)?.parse::<f64>().ok()? };
                total += value * factor;
                time = tail;
                time_any = true;
            }
        }
        if !time.is_empty() || !time_any {
            return None;
        }
        any = true;
    } else if !rest.is_empty() {
        return None;
    }
    let days = total / 86400.0;
    (any && days.is_finite() && days <= 2_958_465.0).then_some(if negative { -days } else { days })
}
