//! OpenDocument styles as model styles: cell styles (`style:style style:family="table-cell"`,
//! their parents and the document's `style:default-style`), the data styles they name, font
//! faces, and the column, row and table styles of the sheet structure.
//! Format reference: ODF 1.3 part 3, chapters 16 (styles), 17 (formatting properties) and 20
//! (formatting attributes): <https://docs.oasis-open.org/office/OpenDocument/v1.3/os/part3-schema/OpenDocument-v1.3-os-part3-schema.html>.

use std::collections::HashMap;

use gridcraft_model::style::Protection;
use gridcraft_model::{BorderLine, BorderStyle, Color, Fill, HAlign, NumFmt, Style, StyleId, StyleTable, Underline, VAlign, VertAlign};
use quick_xml::NsReader;
use quick_xml::XmlVersion;
use quick_xml::events::BytesStart;
use quick_xml::name::{QName, ResolveResult};

use crate::IoError;
use crate::ods_numfmt::{DataKind, DataStyle, Digits, Part, color_name, format_code};

const STYLE: &[u8] = b"urn:oasis:names:tc:opendocument:xmlns:style:1.0";
const FO: &[u8] = b"urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0";
const NUMBER: &[u8] = b"urn:oasis:names:tc:opendocument:xmlns:datastyle:1.0";
const SVG: &[u8] = b"urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0";
const LOEXT: &[u8] = b"urn:org:documentfoundation:names:experimental:office:xmlns:loext:1.0";
const TABLE: &[u8] = b"urn:oasis:names:tc:opendocument:xmlns:table:1.0";
/// OpenOffice's table extensions (`tableooo:tab-color`).
const TABLE_OOO: &[u8] = b"http://openoffice.org/2009/table";

/// Styles of one family kept per document; more is refused as hostile.
const MAX_STYLES: usize = 65_536;
/// Children of one data style.
const MAX_PARTS: usize = 256;
/// Literal text of one data style part.
const MAX_PART_TEXT: usize = 1024;
/// Parent links followed from one style (ODF allows any depth; real chains are short).
const MAX_INHERITANCE: usize = 32;
/// The widest column Excel shows (255 characters of a 7-pixel digit plus padding), in pixels.
pub(crate) const MAX_COL_PX: f32 = 1790.0;
/// The tallest row Excel shows (409.5 points), in pixels.
pub(crate) const MAX_ROW_PX: f32 = 546.0;

/// Where a style is defined: common (named) styles are in `office:styles`, automatic ones in
/// the `office:automatic-styles` of the content. The automatic styles of `styles.xml` (`Part`)
/// format page layouts and headers, never cells, except its data styles, which the common
/// styles beside them may use (ODF 1.3 part 3 §3.15.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Origin {
    Common,
    Automatic,
    Part,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ns {
    Style,
    Fo,
    Number,
    Svg,
    LoExt,
    Table,
    TableOoo,
    Other,
}

fn ns_of(ns: &ResolveResult<'_>) -> Ns {
    let ResolveResult::Bound(n) = ns else { return Ns::Other };
    match n.as_ref() {
        STYLE => Ns::Style,
        FO => Ns::Fo,
        NUMBER => Ns::Number,
        SVG => Ns::Svg,
        LOEXT => Ns::LoExt,
        TABLE => Ns::Table,
        TABLE_OOO => Ns::TableOoo,
        _ => Ns::Other,
    }
}

/// OpenFormula, the default namespace of a `style:condition` (ODF 1.3 part 3 §19.472).
const OF: &[u8] = b"urn:oasis:names:tc:opendocument:xmlns:of:1.2";

/// A `style:condition` without its namespace prefix, when the prefix is bound to OpenFormula or
/// there is none (ODF 1.3 part 3 §19.472); a condition in another namespace is not one GridCraft
/// recognises, and its map is ignored.
fn openformula_condition<'c>(reader: &mut NsReader<&[u8]>, condition: &'c str) -> Option<&'c str> {
    let Some((prefix, rest)) = condition.split_once(':').filter(|(p, _)| is_ncname(p.trim_start())) else {
        return Some(condition);
    };
    let qname = format!("{}:c", prefix.trim_start());
    let (ns, _) = reader.resolver_mut().resolve_attribute(QName(qname.as_bytes()));
    matches!(ns, ResolveResult::Bound(n) if n.as_ref() == OF).then_some(rest)
}

/// An XML name without a colon (simplified to letters, digits, `_`, `-` and `.`), as a
/// namespace prefix is.
fn is_ncname(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_alphabetic() || c == '_') && chars.all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// An element's attributes in the namespaces styles use, read once.
struct Attrs(Vec<(Ns, Vec<u8>, String)>);

impl Attrs {
    fn read(reader: &mut NsReader<&[u8]>, e: &BytesStart<'_>) -> Result<Self, IoError> {
        let mut out = vec![];
        for a in e.attributes() {
            let a = a.map_err(|e| IoError::Xml(e.to_string()))?;
            let (resolved, local) = reader.resolver_mut().resolve_attribute(a.key);
            let ns = ns_of(&resolved);
            if ns == Ns::Other {
                continue;
            }
            let value = a.normalized_value(XmlVersion::Explicit1_0).map_err(|e| IoError::Xml(e.to_string()))?;
            if value.len() > MAX_PART_TEXT * 64 {
                return Err(IoError::TooLarge("ODS style attribute exceeds 64 KiB".into()));
            }
            out.push((ns, local.as_ref().to_vec(), value.into_owned()));
        }
        Ok(Self(out))
    }
    fn get(&self, ns: Ns, name: &str) -> Option<&str> {
        self.0.iter().rev().find(|(n, l, _)| *n == ns && l == name.as_bytes()).map(|(_, _, v)| v.as_str())
    }
    fn flag(&self, ns: Ns, name: &str) -> Option<bool> {
        self.get(ns, name).map(|v| v.trim() == "true")
    }
    fn count(&self, ns: Ns, name: &str) -> Option<u32> {
        self.get(ns, name).and_then(|v| v.trim().parse::<u32>().ok())
    }
}

/// A length (`2.258cm`, `0.889in`, `10pt`…) in points.
pub(crate) fn length_pt(s: &str) -> Option<f64> {
    let s = s.trim();
    let split = s.find(|c: char| c.is_ascii_alphabetic()).unwrap_or(s.len());
    let (n, unit) = s.split_at(split);
    let n = n.trim().parse::<f64>().ok().filter(|n| n.is_finite())?;
    let per = match unit {
        "pt" => 1.0,
        "in" => 72.0,
        "cm" => 72.0 / 2.54,
        "mm" => 72.0 / 25.4,
        "pc" => 12.0,
        "px" => 0.75,
        _ => return None,
    };
    Some(n * per)
}

/// Points as pixels at 96 dpi.
pub(crate) fn pt_to_px(pt: f64) -> f32 {
    (pt * 96.0 / 72.0) as f32
}

/// The formatting a cell style states; `None` inherits from its parent.
#[derive(Clone, Debug, Default)]
struct CellDef {
    parent: Option<String>,
    data_style: Option<String>,
    font_name: Option<String>,
    size: Option<FontSize>,
    bold: Option<bool>,
    italic: Option<bool>,
    underline: Option<Underline>,
    strike: Option<bool>,
    color: Option<Color>,
    vert: Option<VertAlign>,
    fill: Option<Fill>,
    left: Option<BorderLine>,
    right: Option<BorderLine>,
    top: Option<BorderLine>,
    bottom: Option<BorderLine>,
    diag_up: Option<BorderLine>,
    diag_down: Option<BorderLine>,
    /// `style:text-align-source`: `value-type` (true, Excel's General) or `fix`.
    align_by_type: Option<bool>,
    /// `fo:text-align` of the paragraph properties, used when the source is `fix`.
    text_align: Option<HAlign>,
    /// `style:repeat-content`: the content fills the cell (Excel's Fill).
    repeat: Option<bool>,
    v: Option<VAlign>,
    wrap: Option<bool>,
    shrink: Option<bool>,
    rotation: Option<i16>,
    protection: Option<Protection>,
}

impl CellDef {
    /// The style's properties over `s`, but for the font size: a percentage is of the parent
    /// style's, which only the chain the style is in knows ([`chain_size`]).
    fn apply(&self, s: &mut Style) {
        let font = &mut s.font;
        if let Some(name) = &self.font_name {
            font.name.clone_from(name);
            // The face is the document's, not the theme's body font.
            font.minor = false;
        }
        set(&mut font.bold, self.bold);
        set(&mut font.italic, self.italic);
        set(&mut font.underline, self.underline);
        set(&mut font.strike, self.strike);
        set(&mut font.color, self.color);
        set(&mut font.vert, self.vert);
        set(&mut s.fill, self.fill);
        let b = &mut s.borders;
        set(&mut b.left, self.left);
        set(&mut b.right, self.right);
        set(&mut b.top, self.top);
        set(&mut b.bottom, self.bottom);
        set(&mut b.diag_up, self.diag_up);
        set(&mut b.diag_down, self.diag_down);
        let a = &mut s.align;
        set(&mut a.v, self.v);
        set(&mut a.wrap, self.wrap);
        set(&mut a.shrink, self.shrink);
        set(&mut a.rotation, self.rotation);
        set(&mut s.protection, self.protection);
    }

    fn table_cell_properties(&mut self, a: &Attrs) {
        if let Some(bg) = a.get(Ns::Fo, "background-color") {
            self.fill = match bg.trim() {
                "transparent" => Some(Fill::default()),
                hex => Color::from_hex(hex).map(Fill::solid).or(self.fill),
            };
        }
        let all = a.get(Ns::Fo, "border").and_then(border);
        let side = |name: &str| a.get(Ns::Fo, name).and_then(border).or(all);
        self.left = side("border-left").or(self.left);
        self.right = side("border-right").or(self.right);
        self.top = side("border-top").or(self.top);
        self.bottom = side("border-bottom").or(self.bottom);
        self.diag_up = a.get(Ns::Style, "diagonal-bl-tr").and_then(border).or(self.diag_up);
        self.diag_down = a.get(Ns::Style, "diagonal-tl-br").and_then(border).or(self.diag_down);
        match a.get(Ns::Fo, "wrap-option").map(str::trim) {
            Some("wrap") => self.wrap = Some(true),
            Some("no-wrap") => self.wrap = Some(false),
            _ => {}
        }
        self.v = match a.get(Ns::Style, "vertical-align").map(str::trim) {
            Some("top") => Some(VAlign::Top),
            Some("middle") => Some(VAlign::Center),
            Some("bottom" | "automatic") => Some(VAlign::Bottom),
            _ => self.v,
        };
        if let Some(shrink) = a.flag(Ns::Style, "shrink-to-fit") {
            self.shrink = Some(shrink);
        }
        if a.get(Ns::Style, "direction").map(str::trim) == Some("ttb") {
            self.rotation = Some(255);
        } else if let Some(angle) = a.get(Ns::Style, "rotation-angle").and_then(rotation) {
            self.rotation = Some(angle);
        }
        // The alignment's source and the paragraph alignment are separate properties, each
        // inherited on its own (ODF 1.3 part 3 §16.2); [`Styles::build`] combines them.
        match a.get(Ns::Style, "text-align-source").map(str::trim) {
            Some("value-type") => self.align_by_type = Some(true),
            Some("fix") => self.align_by_type = Some(false),
            _ => {}
        }
        if let Some(repeat) = a.flag(Ns::Style, "repeat-content") {
            self.repeat = Some(repeat);
        }
        if let Some(protect) = a.get(Ns::Style, "cell-protect") {
            let words: Vec<&str> = protect.split_whitespace().collect();
            let has = |w: &str| words.contains(&w);
            self.protection = Some(Protection {
                locked: has("protected") || has("hidden-and-protected"),
                hidden: has("formula-hidden") || has("hidden-and-protected"),
            });
        }
    }

    fn paragraph_properties(&mut self, a: &Attrs) {
        let h = match a.get(Ns::Fo, "text-align").map(str::trim) {
            Some("start" | "left") => Some(HAlign::Left),
            Some("center") => Some(HAlign::Center),
            Some("end" | "right") => Some(HAlign::Right),
            Some("justify") => Some(HAlign::Justify),
            _ => None,
        };
        self.text_align = h.or(self.text_align);
    }

    fn text_properties(&mut self, a: &Attrs, fonts: &HashMap<String, String>) {
        if let Some(name) = a.get(Ns::Style, "font-name") {
            self.font_name = Some(fonts.get(name).cloned().unwrap_or_else(|| name.to_string()));
        } else if let Some(family) = a.get(Ns::Fo, "font-family") {
            self.font_name = Some(unquote(family));
        }
        if let Some(size) = a.get(Ns::Fo, "font-size") {
            let size = size.trim();
            // A percentage scales the parent style's size (ODF 1.3 part 3 §20.190).
            let parsed = match size.strip_suffix('%') {
                Some(p) => p.trim().parse::<f32>().ok().filter(|p| p.is_finite() && *p > 0.0).map(|p| FontSize::Scale(p / 100.0)),
                None => length_pt(size).filter(|s| *s > 0.0 && *s <= f64::from(MAX_FONT_PT)).map(|s| FontSize::Points(s as f32)),
            };
            if parsed.is_some() {
                self.size = parsed;
            }
        }
        if let Some(w) = a.get(Ns::Fo, "font-weight") {
            self.bold = Some(w.trim() == "bold" || w.trim().parse::<u32>().is_ok_and(|n| n >= 600));
        }
        if let Some(st) = a.get(Ns::Fo, "font-style") {
            self.italic = Some(matches!(st.trim(), "italic" | "oblique"));
        }
        if let Some(u) = a.get(Ns::Style, "text-underline-style") {
            self.underline = Some(match u.trim() {
                "none" => Underline::None,
                _ if a.get(Ns::Style, "text-underline-type").map(str::trim) == Some("double") => Underline::Double,
                _ => Underline::Single,
            });
        }
        if let Some(s) = a.get(Ns::Style, "text-line-through-style") {
            self.strike = Some(s.trim() != "none" && a.get(Ns::Style, "text-line-through-type").map(str::trim) != Some("none"));
        }
        if a.flag(Ns::Style, "use-window-font-color") == Some(true) {
            self.color = Some(Color::Auto);
        } else if let Some(c) = a.get(Ns::Fo, "color").and_then(Color::from_hex) {
            self.color = Some(c);
        }
        if let Some(p) = a.get(Ns::Style, "text-position") {
            let first = p.split_whitespace().next().unwrap_or_default();
            self.vert = Some(match first {
                "super" => VertAlign::Superscript,
                "sub" => VertAlign::Subscript,
                n => match n.trim_end_matches('%').parse::<f64>() {
                    Ok(v) if v > 0.0 => VertAlign::Superscript,
                    Ok(v) if v < 0.0 => VertAlign::Subscript,
                    _ => VertAlign::Baseline,
                },
            });
        }
    }
}

/// The largest font Excel shows, in points.
const MAX_FONT_PT: f32 = 409.0;

/// `fo:font-size`: a size, or a percentage of the parent style's.
#[derive(Clone, Copy, Debug)]
enum FontSize {
    Points(f32),
    Scale(f32),
}

impl FontSize {
    /// The size in points when the parent style's is `parent`.
    fn of(self, parent: f32) -> f32 {
        match self {
            FontSize::Points(pt) => pt,
            FontSize::Scale(f) => (parent * f).clamp(1.0, MAX_FONT_PT),
        }
    }
}

/// The font size the styles of `chain` (nearest first) give, if any sets one. A percentage is
/// "based on the font height of the parent style rather than … the attributes neighborhood"
/// (ODF 1.3 part 3 §20.190): the farthest style's is of `root`, the size its parent gives, and
/// each nearer style's of the size of its own parent. The default cell styles of a cell's row and
/// column are chains of their own, not parents of the cell's style, so they never scale it.
fn chain_size(chain: &[(Origin, &CellDef)], root: f32) -> Option<f32> {
    chain.iter().rev().filter_map(|(_, def)| def.size).fold(None, |size, s| Some(s.of(size.unwrap_or(root))))
}

fn set<T>(slot: &mut T, value: Option<T>) {
    if let Some(v) = value {
        *slot = v;
    }
}

fn unquote(s: &str) -> String {
    s.trim().trim_matches(|c| c == '\'' || c == '"').to_string()
}

/// `style:rotation-angle` (degrees by default; `deg`, `rad`, `grad`) as Excel's rotation: a
/// quarter turn either way. Angles between them have no Excel equivalent.
fn rotation(s: &str) -> Option<i16> {
    let s = s.trim();
    let (n, per) = if let Some(n) = s.strip_suffix("deg") {
        (n, 1.0)
    } else if let Some(n) = s.strip_suffix("grad") {
        (n, 0.9)
    } else if let Some(n) = s.strip_suffix("rad") {
        (n, 180.0 / std::f64::consts::PI)
    } else {
        (s, 1.0)
    };
    let degrees = (n.trim().parse::<f64>().ok().filter(|v| v.is_finite())? * per).rem_euclid(360.0).round();
    match degrees {
        d if d <= 90.0 => Some(d as i16),
        d if d >= 270.0 => Some((d - 360.0) as i16),
        _ => None,
    }
}

/// A border (`0.74pt solid #000000`): width, line style and colour in any order.
fn border(s: &str) -> Option<BorderLine> {
    let mut width = 0.75;
    let mut kind = "solid";
    let mut color = Color::Auto;
    for word in s.split_whitespace() {
        match word {
            "none" | "hidden" => return Some(BorderLine::default()),
            "thin" => width = 0.75,
            "medium" => width = 1.75,
            "thick" => width = 2.5,
            w if w.starts_with('#') => color = Color::from_hex(w).unwrap_or(Color::Auto),
            w if w.starts_with(|c: char| c.is_ascii_digit() || c == '.') => width = length_pt(w)?,
            w => kind = w,
        }
    }
    if width <= 0.0 {
        return Some(BorderLine::default());
    }
    let wide = width >= 1.0;
    let style = match kind {
        "dotted" => BorderStyle::Dotted,
        "dashed" | "fine-dashed" | "dash" if wide => BorderStyle::MediumDashed,
        "dashed" | "fine-dashed" | "dash" => BorderStyle::Dashed,
        "dash-dot" if wide => BorderStyle::MediumDashDot,
        "dash-dot" => BorderStyle::DashDot,
        "dash-dot-dot" if wide => BorderStyle::MediumDashDotDot,
        "dash-dot-dot" => BorderStyle::DashDotDot,
        "double" | "double-thin" => BorderStyle::Double,
        _ if width >= 2.0 => BorderStyle::Thick,
        _ if wide => BorderStyle::Medium,
        _ => BorderStyle::Thin,
    };
    Some(BorderLine { style, color })
}

/// A column style: its width in pixels.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ColumnStyle {
    pub width: Option<f32>,
}

/// A row style: its height in pixels, and whether the height follows the content.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct RowStyle {
    pub height: Option<f32>,
    pub optimal: bool,
}

/// A table style: whether the sheet is shown, and its tab colour.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TableStyle {
    pub display: bool,
    pub tab_color: Option<Color>,
}

/// The properties a column, row or table style states; `None` inherits from its parent, then
/// from the family's default style (ODF 1.3 part 3 §16.2, §16.4).
trait Inherit: Clone + Default {
    fn inherit(&mut self, parent: &Self);
}

#[derive(Clone, Copy, Debug, Default)]
struct ColumnDef {
    width: Option<f32>,
}

#[derive(Clone, Copy, Debug, Default)]
struct RowDef {
    height: Option<f32>,
    optimal: Option<bool>,
}

#[derive(Clone, Copy, Debug, Default)]
struct TableDef {
    display: Option<bool>,
    tab_color: Option<Color>,
}

impl Inherit for ColumnDef {
    fn inherit(&mut self, p: &Self) {
        self.width = self.width.or(p.width);
    }
}

impl Inherit for RowDef {
    fn inherit(&mut self, p: &Self) {
        self.height = self.height.or(p.height);
        self.optimal = self.optimal.or(p.optimal);
    }
}

impl Inherit for TableDef {
    fn inherit(&mut self, p: &Self) {
        self.display = self.display.or(p.display);
        self.tab_color = self.tab_color.or(p.tab_color);
    }
}

/// The styles of one of those families: automatic ones by name, the common ones they and each
/// other may inherit from, and the family's default style.
struct Family<T> {
    automatic: HashMap<String, (Option<String>, T)>,
    common: HashMap<String, (Option<String>, T)>,
    default: T,
}

impl<T: Inherit> Family<T> {
    fn new() -> Self {
        Self { automatic: HashMap::new(), common: HashMap::new(), default: T::default() }
    }

    fn len(&self) -> usize {
        self.automatic.len() + self.common.len()
    }

    fn insert(&mut self, origin: Origin, name: String, parent: Option<String>, def: T) {
        let map = if origin == Origin::Automatic { &mut self.automatic } else { &mut self.common };
        map.insert(name, (parent, def));
    }

    /// The style `name` (an automatic one first) with what it inherits; a parent is always a
    /// common style.
    fn resolve(&self, name: &str) -> Option<T> {
        let (parent, def) = self.automatic.get(name).or_else(|| self.common.get(name))?;
        let mut out = def.clone();
        let mut parent = parent.as_deref();
        for _ in 0..MAX_INHERITANCE {
            let Some((next, def)) = parent.and_then(|p| self.common.get(p)) else { break };
            out.inherit(def);
            parent = next.as_deref();
        }
        out.inherit(&self.default);
        Some(out)
    }
}

enum Open {
    Cell { origin: Origin, name: String, def: CellDef },
    Default(CellDef),
    Column { origin: Origin, name: String, parent: Option<String>, def: ColumnDef },
    Row { origin: Origin, name: String, parent: Option<String>, def: RowDef },
    Table { origin: Origin, name: String, parent: Option<String>, def: TableDef },
    ColumnDefault(ColumnDef),
    RowDefault(RowDef),
    TableDefault(TableDef),
    Data { origin: Origin, name: String, style: DataStyle },
    Skip,
}

/// The styles of a document, collected as their elements are read and resolved on use.
pub(crate) struct Styles {
    fonts: HashMap<String, String>,
    cells: HashMap<(Origin, String), CellDef>,
    default: CellDef,
    data: HashMap<(Origin, String), DataStyle>,
    columns: Family<ColumnDef>,
    rows: Family<RowDef>,
    tables: Family<TableDef>,
    open: Option<(usize, Open)>,
    /// The depth of the data-style element whose text is being read, and whether it is a
    /// number's embedded text.
    text: Option<(usize, bool)>,
    /// Cell style names by their [`Styles::key`] (key 0 is no style).
    names: Vec<String>,
    keys: HashMap<String, u32>,
    /// The model style of each (cell, row default, column default) cell style keys, and whether
    /// it aligns by value type.
    resolved: HashMap<[u32; 3], (StyleId, bool)>,
    /// A flat document: its one file holds the common and automatic styles together.
    pub flat: bool,
}

impl Styles {
    pub(crate) fn new() -> Self {
        Self {
            fonts: HashMap::new(),
            cells: HashMap::new(),
            default: CellDef::default(),
            data: HashMap::new(),
            columns: Family::new(),
            rows: Family::new(),
            tables: Family::new(),
            open: None,
            text: None,
            names: vec![],
            keys: HashMap::new(),
            resolved: HashMap::new(),
            flat: false,
        }
    }

    fn total(&self) -> usize {
        self.cells.len() + self.data.len() + self.columns.len() + self.rows.len() + self.tables.len() + self.fonts.len()
    }

    /// An element at `depth` inside a style section of `origin`.
    pub(crate) fn start(&mut self, reader: &mut NsReader<&[u8]>, e: &BytesStart<'_>, depth: usize, origin: Origin) -> Result<(), IoError> {
        let (ns, name) = reader.resolver_mut().resolve_element(e.name());
        let ns = ns_of(&ns);
        let local = name.as_ref().to_vec();
        let Some((open_depth, _)) = &self.open else {
            return self.top(reader, e, depth, origin, ns, &local);
        };
        let child = depth == open_depth + 1;
        let a = Attrs::read(reader, e)?;
        match (&mut self.open, ns, local.as_slice()) {
            (Some((_, Open::Cell { def, .. } | Open::Default(def))), Ns::Style, b"table-cell-properties") if child => def.table_cell_properties(&a),
            (Some((_, Open::Cell { def, .. } | Open::Default(def))), Ns::Style, b"paragraph-properties") if child => def.paragraph_properties(&a),
            (Some((_, Open::Cell { def, .. } | Open::Default(def))), Ns::Style, b"text-properties") if child => def.text_properties(&a, &self.fonts),
            (Some((_, Open::Column { def: c, .. } | Open::ColumnDefault(c))), Ns::Style, b"table-column-properties") if child => {
                if let Some(w) = a.get(Ns::Style, "column-width").and_then(length_pt) {
                    c.width = Some(pt_to_px(w).clamp(0.0, MAX_COL_PX));
                }
            }
            (Some((_, Open::Row { def: r, .. } | Open::RowDefault(r))), Ns::Style, b"table-row-properties") if child => {
                if let Some(h) = a.get(Ns::Style, "row-height").and_then(length_pt) {
                    r.height = Some(pt_to_px(h).clamp(0.0, MAX_ROW_PX));
                }
                if let Some(optimal) = a.flag(Ns::Style, "use-optimal-row-height") {
                    r.optimal = Some(optimal);
                }
            }
            (Some((_, Open::Table { def: t, .. } | Open::TableDefault(t))), Ns::Style, b"table-properties") if child => {
                if let Some(display) = a.flag(Ns::Table, "display") {
                    t.display = Some(display);
                }
                if let Some(c) = a.get(Ns::TableOoo, "tab-color").and_then(Color::from_hex) {
                    t.tab_color = Some(c);
                }
            }
            (Some((_, Open::Data { style, .. })), Ns::Style, b"map") if child && style.maps.len() < 8 => {
                let cond = a.get(Ns::Style, "condition").and_then(|c| openformula_condition(reader, c));
                if let (Some(cond), Some(target)) = (cond, a.get(Ns::Style, "apply-style-name")) {
                    style.maps.push((cond.to_string(), target.to_string()));
                }
            }
            (Some((_, Open::Data { style, .. })), Ns::Style, b"text-properties") if child => {
                style.color = a.get(Ns::Fo, "color").and_then(color_name);
            }
            // `number:embedded-text` inside a `number:number`: text at a digit position.
            (Some((d, Open::Data { style, .. })), Ns::Number, b"embedded-text") if depth == *d + 2 => {
                if let Some(Part::Number(digits)) = style.parts.last_mut()
                    && digits.embedded.len() < 16
                {
                    digits.embedded.push((a.count(Ns::Number, "position").unwrap_or(0), String::new()));
                    self.text = Some((depth, true));
                }
            }
            (Some((_, Open::Data { style, .. })), _, _) if child => {
                if style.parts.len() < MAX_PARTS
                    && let Some(part) = data_part(ns, &local, &a)
                {
                    style.parts.push(part);
                    if matches!(local.as_slice(), b"text" | b"currency-symbol" | b"fill-character") {
                        self.text = Some((depth, false));
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// A style element directly in a style section.
    fn top(&mut self, reader: &mut NsReader<&[u8]>, e: &BytesStart<'_>, depth: usize, origin: Origin, ns: Ns, local: &[u8]) -> Result<(), IoError> {
        // Of the automatic styles of styles.xml, only the data styles concern cells.
        if origin == Origin::Part && ns != Ns::Number {
            self.open = Some((depth, Open::Skip));
            return Ok(());
        }
        let a = Attrs::read(reader, e)?;
        let name = a.get(Ns::Style, "name").unwrap_or_default().to_string();
        let open = match (ns, local) {
            (Ns::Style, b"font-face") => {
                if let Some(family) = a.get(Ns::Svg, "font-family") {
                    self.fonts.insert(name, unquote(family));
                }
                Open::Skip
            }
            (Ns::Style, b"default-style") => match a.get(Ns::Style, "family").unwrap_or_default() {
                "table-cell" => Open::Default(CellDef { data_style: a.get(Ns::Style, "data-style-name").map(str::to_string), ..CellDef::default() }),
                "table-column" => Open::ColumnDefault(ColumnDef::default()),
                "table-row" => Open::RowDefault(RowDef::default()),
                "table" => Open::TableDefault(TableDef::default()),
                _ => Open::Skip,
            },
            (Ns::Style, b"style") => {
                let parent = a.get(Ns::Style, "parent-style-name").map(str::to_string);
                match a.get(Ns::Style, "family").unwrap_or_default() {
                    "table-cell" => Open::Cell {
                        origin,
                        name,
                        def: CellDef { parent, data_style: a.get(Ns::Style, "data-style-name").map(str::to_string), ..CellDef::default() },
                    },
                    "table-column" => Open::Column { origin, name, parent, def: ColumnDef::default() },
                    "table-row" => Open::Row { origin, name, parent, def: RowDef::default() },
                    "table" => Open::Table { origin, name, parent, def: TableDef::default() },
                    _ => Open::Skip,
                }
            }
            (Ns::Number, kind) => match data_kind(kind) {
                Some(kind) => {
                    let mut style = DataStyle::new(kind);
                    style.elapsed = kind == DataKind::Time && a.flag(Ns::Number, "truncate-on-overflow") == Some(false);
                    Open::Data { origin, name, style }
                }
                None => Open::Skip,
            },
            _ => Open::Skip,
        };
        self.open = Some((depth, open));
        Ok(())
    }

    /// Text inside the open element.
    pub(crate) fn text(&mut self, text: &str) {
        let Some((_, embedded)) = self.text else { return };
        let Some((_, Open::Data { style, .. })) = &mut self.open else { return };
        let target = match style.parts.last_mut() {
            Some(Part::Number(d)) if embedded => d.embedded.last_mut().map(|(_, t)| t),
            Some(Part::Text(t) | Part::Currency(t) | Part::Fill(t)) if !embedded => Some(t),
            _ => None,
        };
        if let Some(t) = target
            && t.len() + text.len() <= MAX_PART_TEXT
        {
            t.push_str(text);
        }
    }

    /// The end of the element at `depth` inside a style section.
    pub(crate) fn end(&mut self, depth: usize) -> Result<(), IoError> {
        if self.text.is_some_and(|(d, _)| d == depth) {
            self.text = None;
        }
        if !self.open.as_ref().is_some_and(|(d, _)| *d == depth) {
            return Ok(());
        }
        let Some((_, open)) = self.open.take() else { return Ok(()) };
        if self.total() >= MAX_STYLES {
            return Err(IoError::TooLarge("ODS defines more than 65536 styles".into()));
        }
        match open {
            Open::Cell { origin, name, def } => {
                self.cells.insert((origin, name), def);
            }
            Open::Default(def) => self.default = def,
            Open::Column { origin, name, parent, def } => self.columns.insert(origin, name, parent, def),
            Open::Row { origin, name, parent, def } => self.rows.insert(origin, name, parent, def),
            Open::Table { origin, name, parent, def } => self.tables.insert(origin, name, parent, def),
            Open::ColumnDefault(def) => self.columns.default = def,
            Open::RowDefault(def) => self.rows.default = def,
            Open::TableDefault(def) => self.tables.default = def,
            Open::Data { origin, name, style } => {
                self.data.insert((origin, name), style);
            }
            Open::Skip => {}
        }
        Ok(())
    }

    pub(crate) fn column(&self, name: &str) -> Option<ColumnStyle> {
        self.columns.resolve(name).map(|c| ColumnStyle { width: c.width })
    }
    pub(crate) fn row(&self, name: &str) -> Option<RowStyle> {
        self.rows.resolve(name).map(|r| RowStyle { height: r.height, optimal: r.optimal.unwrap_or(false) })
    }
    pub(crate) fn table(&self, name: &str) -> Option<TableStyle> {
        self.tables.resolve(name).map(|t| TableStyle { display: t.display.unwrap_or(true), tab_color: t.tab_color })
    }

    /// A cell style by name: the content's automatic styles first, then the common ones.
    fn cell_def(&self, name: &str) -> Option<(Origin, &CellDef)> {
        [Origin::Automatic, Origin::Common].into_iter().find_map(|o| self.cells.get(&(o, name.to_string())).map(|d| (o, d)))
    }

    /// The chain from `name` up through its parents, nearest first.
    fn chain(&self, name: &str) -> Vec<(Origin, &CellDef)> {
        let mut out: Vec<(Origin, &CellDef)> = vec![];
        let mut next = self.cell_def(name);
        while let Some((origin, def)) = next {
            if out.len() >= MAX_INHERITANCE {
                break;
            }
            out.push((origin, def));
            // A parent is always a common style.
            next = def.parent.as_deref().and_then(|p| self.cells.get(&(Origin::Common, p.to_string()))).map(|d| (Origin::Common, d));
        }
        out
    }

    /// The data style `name` named from a style of `origin`, and where it is defined: a style
    /// names a data style of its own file, automatic or common (ODF 1.3 part 3 §16.1), so the
    /// content's automatic styles name the content's or common ones, and common styles (and
    /// styles.xml's automatic data styles) those of styles.xml (of the one file, in a flat
    /// document).
    fn data_style(&self, name: &str, origin: Origin) -> Option<(Origin, &DataStyle)> {
        let automatic = if origin == Origin::Automatic || self.flat { Origin::Automatic } else { Origin::Part };
        [automatic, Origin::Common].into_iter().find_map(|o| self.data.get(&(o, name.to_string())).map(|s| (o, s)))
    }

    /// The number format code of the data style `name` named from a style of `origin`. Its maps
    /// name data styles from where it is defined, not from where the cell style is (§19.472).
    fn format(&self, name: &str, origin: Origin) -> Option<String> {
        let (defined, style) = self.data_style(name, origin)?;
        format_code(style, |n| self.data_style(n, defined).map(|(_, s)| s))
    }

    /// The style of a cell with the style `chains`, each a style and its parents nearest first:
    /// the cell's own, then its row's default and its column's. A property one doesn't set comes
    /// from the next, then from the document's default style (ODF 1.3 part 3 §16.2).
    fn build(&self, chains: &[Vec<(Origin, &CellDef)>]) -> Style {
        let mut style = Style::default();
        self.default.apply(&mut style);
        if let Some(size) = self.default.size {
            style.font.size = size.of(style.font.size);
        }
        let base = style.font.size;
        for (_, def) in chains.iter().rev().flat_map(|chain| chain.iter().rev()) {
            def.apply(&mut style);
        }
        if let Some(size) = chains.iter().find_map(|chain| chain_size(chain, base)) {
            style.font.size = size;
        }
        // The horizontal alignment once each of its properties is resolved through the chain
        // (nearest style first, the default style last): by value type (Excel's General)
        // outranks filling, which outranks the paragraph's own alignment.
        let defs = || chains.iter().flatten().map(|(_, d)| *d).chain(std::iter::once(&self.default));
        let by_type = defs().find_map(|d| d.align_by_type);
        let repeat = defs().find_map(|d| d.repeat);
        let text_align = defs().find_map(|d| d.text_align);
        style.align.h = match (by_type, repeat, text_align) {
            (Some(true), _, _) => HAlign::General,
            (_, Some(true), _) => HAlign::Fill,
            (_, _, Some(h)) => h,
            _ => style.align.h,
        };
        let data = chains.iter().flatten().find_map(|(o, d)| d.data_style.as_deref().map(|n| (n, *o)));
        let data = data.or_else(|| self.default.data_style.as_deref().map(|n| (n, Origin::Common)));
        if let Some(code) = data.and_then(|(name, origin)| self.format(name, origin)) {
            style.num_fmt = NumFmt::new(&code);
        }
        style
    }

    /// The workbook's Normal style: the document's default cell style under its `Default`
    /// common style, which cells without a style of their own use.
    pub(crate) fn normal(&self) -> Style {
        self.build(&[self.chain_of_common("Default")])
    }

    fn chain_of_common(&self, name: &str) -> Vec<(Origin, &CellDef)> {
        if self.cells.contains_key(&(Origin::Common, name.to_string())) { self.chain(name) } else { vec![] }
    }

    /// The key of the cell style `name`, for [`Styles::cascade`]: 0 for a name the document
    /// doesn't define, so the keys are bounded by the styles the document defines.
    pub(crate) fn key(&mut self, name: &str) -> u32 {
        if let Some(k) = self.keys.get(name) {
            return *k;
        }
        if self.cell_def(name).is_none() {
            return 0;
        }
        self.names.push(name.to_string());
        let k = self.names.len() as u32;
        self.keys.insert(name.to_string(), k);
        k
    }

    /// The model style of a cell with the cell style `keys[0]` in a row whose default cell style
    /// is `keys[1]` and a column whose default cell style is `keys[2]` (0: none): a property the
    /// cell style and its parents don't set comes from the row's default style and its parents,
    /// then the column's, then the document's default style (ODF 1.3 part 3 §16.2). Unknown
    /// names and no styles at all are the Normal style.
    pub(crate) fn cascade(&mut self, keys: [u32; 3], table: &mut StyleTable) -> StyleId {
        self.cascade_aligned(keys, table).0
    }

    /// [`Styles::cascade`], and whether the horizontal alignment is by value type: Excel's
    /// General but for logical and error values, which ODF aligns right as it does numbers, not
    /// centred (ODF 1.3 part 3 §20.364).
    pub(crate) fn cascade_aligned(&mut self, keys: [u32; 3], table: &mut StyleTable) -> (StyleId, bool) {
        if let Some(resolved) = self.resolved.get(&keys) {
            return *resolved;
        }
        let chains: Vec<Vec<(Origin, &CellDef)>> =
            keys.iter().filter_map(|k| k.checked_sub(1).and_then(|i| self.names.get(i as usize))).map(|n| self.chain(n)).collect();
        let id = if chains.is_empty() { StyleId::DEFAULT } else { table.intern(self.build(&chains)) };
        // No styles at all is the Normal style's.
        let normal = chains.is_empty().then(|| self.chain_of_common("Default"));
        let defs = chains.iter().flatten().chain(normal.iter().flatten()).map(|(_, d)| *d);
        let by_type = defs.chain(std::iter::once(&self.default)).find_map(|d| d.align_by_type) == Some(true);
        if self.resolved.len() < MAX_STYLES {
            self.resolved.insert(keys, (id, by_type));
        }
        (id, by_type)
    }
}

fn data_kind(local: &[u8]) -> Option<DataKind> {
    Some(match local {
        b"number-style" => DataKind::Number,
        b"currency-style" => DataKind::Currency,
        b"percentage-style" => DataKind::Percentage,
        b"date-style" => DataKind::Date,
        b"time-style" => DataKind::Time,
        b"boolean-style" => DataKind::Boolean,
        b"text-style" => DataKind::Text,
        _ => return None,
    })
}

fn digits(a: &Attrs) -> Digits {
    let min_decimals = a.count(Ns::Number, "min-decimal-places").or_else(|| a.count(Ns::LoExt, "min-decimal-places"));
    Digits {
        decimals: a.count(Ns::Number, "decimal-places"),
        min_decimals,
        min_integer: a.count(Ns::Number, "min-integer-digits").unwrap_or(0),
        grouping: a.flag(Ns::Number, "grouping").unwrap_or(false),
        factor: a.get(Ns::Number, "display-factor").and_then(|f| f.trim().parse::<f64>().ok()).filter(|f| f.is_finite()).unwrap_or(1.0),
        embedded: vec![],
    }
}

fn long(a: &Attrs) -> bool {
    a.get(Ns::Number, "style").map(str::trim) == Some("long")
}

/// One child of a data style, or `None` for what isn't a part of its code.
fn data_part(ns: Ns, local: &[u8], a: &Attrs) -> Option<Part> {
    if !matches!(ns, Ns::Number | Ns::LoExt) {
        return None;
    }
    Some(match local {
        b"text" => Part::Text(String::new()),
        b"number" => Part::Number(digits(a)),
        b"scientific-number" => Part::Scientific {
            digits: digits(a),
            min_exponent: a.count(Ns::Number, "min-exponent-digits").unwrap_or(1),
            interval: a.count(Ns::Number, "exponent-interval").or_else(|| a.count(Ns::LoExt, "exponent-interval")).unwrap_or(1),
            forced_sign: a.flag(Ns::Number, "forced-exponent-sign").or_else(|| a.flag(Ns::LoExt, "forced-exponent-sign")).unwrap_or(true),
        },
        b"fraction" => Part::Fraction {
            min_integer: a.count(Ns::Number, "min-integer-digits"),
            numerator: a.count(Ns::Number, "min-numerator-digits").max(a.count(Ns::LoExt, "max-numerator-digits")).unwrap_or(1),
            denominator: a.count(Ns::Number, "min-denominator-digits").unwrap_or(1).max(
                a.count(Ns::LoExt, "max-denominator-value")
                    .or_else(|| a.count(Ns::Number, "max-denominator-value"))
                    .map_or(0, |v| v.to_string().len() as u32),
            ),
            denominator_value: a.count(Ns::Number, "denominator-value"),
            grouping: a.flag(Ns::Number, "grouping").unwrap_or(false),
        },
        b"currency-symbol" => Part::Currency(String::new()),
        b"fill-character" => Part::Fill(String::new()),
        b"text-content" => Part::TextContent,
        b"day" => Part::Day { long: long(a) },
        b"month" => Part::Month { long: long(a), textual: a.flag(Ns::Number, "textual").unwrap_or(false) },
        b"year" => Part::Year { long: long(a) },
        b"day-of-week" => Part::DayOfWeek { long: long(a) },
        b"hours" => Part::Hours { long: long(a) },
        b"minutes" => Part::Minutes { long: long(a) },
        b"seconds" => Part::Seconds { long: long(a), decimals: a.count(Ns::Number, "decimal-places").unwrap_or(0) },
        b"am-pm" => Part::AmPm,
        b"era" | b"quarter" | b"week-of-year" | b"boolean" => Part::Unsupported,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths_convert_to_points() {
        assert_eq!(length_pt("1in"), Some(72.0));
        assert_eq!(length_pt("10pt"), Some(10.0));
        assert!((length_pt("2.54cm").unwrap_or_default() - 72.0).abs() < 1e-9);
        assert_eq!(length_pt("3em"), None);
        assert_eq!(length_pt("x"), None);
    }

    #[test]
    fn borders_map_width_and_line_style() {
        let b = |s: &str| border(s).map(|l| l.style);
        assert_eq!(b("0.74pt solid #000000"), Some(BorderStyle::Thin));
        assert_eq!(b("0.06pt solid #000000"), Some(BorderStyle::Thin));
        assert_eq!(b("1.75pt solid #000000"), Some(BorderStyle::Medium));
        assert_eq!(b("2.5pt solid #000000"), Some(BorderStyle::Thick));
        assert_eq!(b("0.74pt dashed #000000"), Some(BorderStyle::Dashed));
        assert_eq!(b("1.1pt double #000000"), Some(BorderStyle::Double));
        assert_eq!(b("none"), Some(BorderStyle::None));
        assert_eq!(border("0.74pt solid #ff0000").map(|l| l.color), Some(Color::Rgb(0xFF0000)));
    }

    #[test]
    fn rotations_fold_to_excel_range() {
        assert_eq!(rotation("90"), Some(90));
        assert_eq!(rotation("315deg"), Some(-45));
        assert_eq!(rotation("180"), None);
        assert_eq!(rotation("100grad"), Some(90));
    }
}
