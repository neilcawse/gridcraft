//! OpenDocument data styles (`number:number-style`, `number:date-style`, …) as Excel number
//! format codes. Format reference: ODF 1.3 part 3, section 16.29 (data styles) and 19.3xx (their
//! attributes); Excel's format code syntax: Microsoft's "Number format codes" documentation.
//! What Excel can't express (eras, quarters, week numbers, alternative calendars) is left out
//! of the code; the rest of the style still applies.

/// The family of a data style: which element defined it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DataKind {
    Number,
    Currency,
    Percentage,
    Date,
    Time,
    Boolean,
    Text,
}

/// One child of a data style, in document order.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Part {
    /// `number:text`: literal text.
    Text(String),
    /// `number:number`.
    Number(Digits),
    /// `number:scientific-number`.
    Scientific {
        digits: Digits,
        min_exponent: u32,
        interval: u32,
        forced_sign: bool,
    },
    /// `number:fraction`.
    Fraction {
        min_integer: Option<u32>,
        numerator: u32,
        denominator: u32,
        denominator_value: Option<u32>,
        grouping: bool,
    },
    /// `number:currency-symbol`.
    Currency(String),
    /// `number:fill-character`.
    Fill(String),
    /// `number:text-content`: the cell's text.
    TextContent,
    Day {
        long: bool,
    },
    Month {
        long: bool,
        textual: bool,
    },
    Year {
        long: bool,
    },
    DayOfWeek {
        long: bool,
    },
    Hours {
        long: bool,
    },
    Minutes {
        long: bool,
    },
    Seconds {
        long: bool,
        decimals: u32,
    },
    AmPm,
    /// `number:era`, `number:quarter`, `number:week-of-year`, `number:boolean`: no Excel code.
    Unsupported,
}

/// The digits of a `number:number` (or the mantissa of a `number:scientific-number`).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Digits {
    /// `number:decimal-places`; absent means as many as needed (General).
    pub decimals: Option<u32>,
    /// `number:min-decimal-places` (`loext:min-decimal-places`): the decimals always shown.
    pub min_decimals: Option<u32>,
    pub min_integer: u32,
    pub grouping: bool,
    /// `number:display-factor`: 1000 per thousands scaling comma.
    pub factor: f64,
    /// `number:embedded-text`: text at a digit position (counted leftwards from the decimal point).
    pub embedded: Vec<(u32, String)>,
}

/// A data style: its parts, text colour and conditional maps.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DataStyle {
    pub kind: DataKind,
    pub parts: Vec<Part>,
    /// `style:text-properties fo:color`, as Excel's colour name or `ColorN`.
    pub color: Option<&'static str>,
    /// `style:map`: a condition on `value()` and the data style it applies.
    pub maps: Vec<(String, String)>,
    /// `number:truncate-on-overflow="false"` on a time style: elapsed hours.
    pub elapsed: bool,
}

/// Most digit placeholders written for one count attribute; larger values are hostile.
const MAX_DIGITS: u32 = 30;
/// Excel allows two conditional sections before the last.
const MAX_CONDITIONS: usize = 2;

impl DataStyle {
    pub(crate) fn new(kind: DataKind) -> Self {
        Self { kind, parts: vec![], color: None, maps: vec![], elapsed: false }
    }

    /// This style's own section (no maps), or `None` where it is Excel's General.
    fn section(&self) -> Option<String> {
        if self.kind == DataKind::Boolean {
            return None;
        }
        let mut out = String::new();
        if let Some(color) = self.color {
            out.push('[');
            out.push_str(color);
            out.push(']');
        }
        let date = matches!(self.kind, DataKind::Date | DataKind::Time);
        let mut elapsed = self.elapsed;
        let mut general = true;
        for part in &self.parts {
            match part {
                Part::Text(t) => literal(&mut out, t, self.kind),
                Part::Number(d) => {
                    let code = number(d);
                    general &= code == "General";
                    out.push_str(&code);
                }
                Part::Scientific { digits, min_exponent, interval, forced_sign } => {
                    general = false;
                    out.push_str(&scientific(digits, *min_exponent, *interval, *forced_sign));
                }
                Part::Fraction { min_integer, numerator, denominator, denominator_value, grouping } => {
                    general = false;
                    out.push_str(&fraction(*min_integer, *numerator, *denominator, *denominator_value, *grouping));
                }
                Part::Currency(symbol) => {
                    general = false;
                    currency(&mut out, symbol);
                }
                Part::Fill(c) => {
                    if let Some(ch) = c.chars().next() {
                        general = false;
                        out.push('*');
                        out.push(ch);
                    }
                }
                Part::TextContent => {
                    general = false;
                    out.push('@');
                }
                Part::Unsupported => {}
                _ if date => {
                    general = false;
                    date_part(&mut out, part, &mut elapsed);
                }
                _ => {}
            }
        }
        // A section that is General with nothing around it is General itself; a text style
        // without its content is still text.
        if out == "General" && general {
            return None;
        }
        if self.kind == DataKind::Text && !out.contains('@') && out.is_empty() {
            return Some("@".into());
        }
        Some(out)
    }
}

/// The Excel number format code of the data style `name`, whose maps name other data styles
/// looked up through `find`. `None` is General.
///
/// A text style's own section is Excel's fourth, text, section: numbers are formatted by its
/// maps only (ODF 1.3 part 3 §16.3, §19.472), so the sign maps become the three numeric sections
/// before it.
pub(crate) fn format_code<'a>(style: &'a DataStyle, find: impl Fn(&str) -> Option<&'a DataStyle>) -> Option<String> {
    let main = style.section();
    let mut mapped: Vec<(Condition, Option<String>)> = vec![];
    for (cond, target) in &style.maps {
        let Some((c, s)) = Condition::parse(cond).and_then(|c| Some((c, find(target)?.section()))) else { continue };
        // Maps are tried in order (§19.472): one every number meets ends the list, and the
        // main style formats no number at all.
        let all = c.all_finite();
        mapped.push((c, s));
        if all || mapped.len() == 3 {
            break;
        }
    }
    if mapped.is_empty() {
        return main;
    }
    let text = |s: &Option<String>| s.clone().unwrap_or_else(|| "General".into());
    let is_text = style.kind == DataKind::Text;
    let main = text(&main);
    // A text section after the three numeric ones; Excel shows text as itself without one.
    let with_text = |numeric: [String; 3]| {
        let [p, n, z] = numeric;
        if is_text && main != "@" {
            format!("{p};{n};{z};{main}")
        } else if is_text {
            format!("{p};{n};{z};@")
        } else {
            format!("{p};{n};{z}")
        }
    };
    // The standard shapes are Excel's own sections: positive-and-zero;negative, and
    // positive;negative;zero (the mapped sections show the magnitude, as Excel's do).
    let find_op = |op: &str, n: f64| mapped.iter().find(|(c, _)| c.op == op && c.value == n).map(|(_, s)| text(s));
    if let Some((c, s)) = mapped.last()
        && c.all_finite()
    {
        // Every number reaches the last map; any before it are conditions Excel also checks
        // first, in at most two conditional sections.
        let s = text(s);
        if mapped.len() == 1 {
            return Some(if is_text && main != "@" { with_text([s.clone(), negated(&s), s]) } else { s });
        }
        let mut sections: Vec<String> = mapped
            .iter()
            .take(mapped.len() - 1)
            .take(MAX_CONDITIONS)
            .map(|(c, s)| format!("[{}{}]{}", c.op, number_text(c.value), text(s)))
            .collect();
        // The text style's own section formats text (§16.29.26, §16.29.28), so it follows the
        // numeric ones. After one condition a last section holding `@` is the text section of a
        // three-section code, and numbers go through the first two as they do without it (live
        // Excel 16.114: `[>5]"big";0.00;"Name: "@` shows 1.234 as 1.23 and -3 as -3.00). Text
        // without `@` can only be the fourth, after a negative section that adds the minus
        // sign Excel leaves out of its own (there `…;0.00;General;"Name: "@` shows -3 as 3.00).
        let text_section = is_text && main != "@";
        if text_section && sections.len() == 1 && !style.parts.iter().any(|p| matches!(p, Part::TextContent)) {
            sections.push(negated(&s));
        }
        sections.push(s);
        if text_section {
            sections.push(main);
        }
        return Some(sections.join(";"));
    }
    if mapped.len() == 1 && mapped[0].0.op == ">=" && mapped[0].0.value == 0.0 {
        return Some(format!("{};{main}", text(&mapped[0].1)));
    }
    if let (Some(positive), Some(negative)) = (find_op(">", 0.0), find_op("<", 0.0)) {
        match (mapped.len(), find_op("=", 0.0)) {
            (3, Some(zero)) => return Some(with_text([positive, negative, zero])),
            // Zero is the main style's: a text style's number shows as itself.
            (2, _) if is_text => return Some(with_text([positive, negative, "General".into()])),
            (2, _) => return Some(format!("{positive};{negative};{main}")),
            _ => {}
        }
    }
    let mut out = String::new();
    for (c, s) in mapped.iter().take(MAX_CONDITIONS) {
        out.push_str(&format!("[{}{}]{};", c.op, number_text(c.value), text(s)));
    }
    out.push_str(&main);
    Some(out)
}

/// `section` as an explicit negative section: Excel adds no minus sign to one, so it is written
/// after the section's leading `[…]` colour or condition.
fn negated(section: &str) -> String {
    let mut at = 0;
    while section[at..].starts_with('[')
        && let Some(end) = section[at..].find(']')
    {
        at += end + 1;
    }
    format!("{}-{}", &section[..at], &section[at..])
}

/// A `style:condition` on the cell value: `value()>=0`.
struct Condition {
    op: &'static str,
    value: f64,
}

impl Condition {
    fn parse(s: &str) -> Option<Self> {
        let rest = s.trim().strip_prefix("value()")?.trim_start();
        let (op, rest) = [(">=", ">="), ("<=", "<="), ("!=", "<>"), ("<>", "<>"), (">", ">"), ("<", "<"), ("==", "="), ("=", "=")]
            .iter()
            .find_map(|(w, op)| rest.strip_prefix(w).map(|r| (*op, r)))?;
        let value = rest.trim().parse::<f64>().ok().filter(|v| v.is_finite())?;
        Some(Self { op, value })
    }

    /// Whether every finite number (every cell value) meets the condition.
    fn all_finite(&self) -> bool {
        match self.op {
            "<=" => self.value == f64::MAX,
            ">=" => self.value == f64::MIN,
            _ => false,
        }
    }
}

/// A condition's threshold, in the shorter of its plain and scientific spellings (both round
/// trip): `1E+300`, not 301 digits, so a short condition stays a short code.
fn number_text(v: f64) -> String {
    let plain = format!("{v}");
    let sci = format!("{v:E}");
    let sci = match sci.split_once('E') {
        Some((m, e)) if !e.starts_with('-') => format!("{m}E+{e}"),
        _ => sci,
    };
    if sci.len() < plain.len() { sci } else { plain }
}

/// Literal text: characters Excel shows as themselves stay bare, the rest is quoted. A
/// percentage style's `%` is the scaling percent sign.
fn literal(out: &mut String, text: &str, kind: DataKind) {
    let mut quoted = false;
    for ch in text.chars() {
        let bare = match ch {
            ' ' | '-' | '+' | '(' | ')' | ':' => true,
            '/' | ',' => matches!(kind, DataKind::Date | DataKind::Time),
            '%' => kind == DataKind::Percentage,
            _ => false,
        };
        if bare || ch == '"' {
            if quoted {
                out.push('"');
                quoted = false;
            }
            if ch == '"' {
                out.push('\\');
            }
            out.push(ch);
        } else {
            if !quoted {
                out.push('"');
                quoted = true;
            }
            out.push(ch);
        }
    }
    if quoted {
        out.push('"');
    }
}

/// `[$€]`, or a bare `$`.
fn currency(out: &mut String, symbol: &str) {
    if symbol.is_empty() {
        return;
    }
    if symbol == "$" {
        out.push('$');
    } else if symbol.contains([']', '-', '"']) {
        literal(out, symbol, DataKind::Currency);
    } else {
        out.push_str("[$");
        out.push_str(symbol);
        out.push(']');
    }
}

fn capped(n: u32) -> usize {
    n.min(MAX_DIGITS) as usize
}

/// The integer placeholders: `min` zeros, `#` before them (one at least), a grouping comma, and
/// embedded texts at their positions.
fn integer(min: u32, grouping: bool, embedded: &[(u32, String)]) -> String {
    let min = capped(min);
    // Text at position k has k digits to its right, so it follows placeholder k (counted from
    // the right, from 0): the placeholders reach one past the outermost text (§19.358).
    let outermost = embedded.iter().map(|(p, _)| capped(*p) + 1).max().unwrap_or(0);
    let mut width = min.max(outermost).max(1);
    if grouping {
        width = width.max(4);
    }
    // Placeholders right to left, so position k is the k-th digit left of the point.
    let mut cells: Vec<String> = (0..width).map(|i| if i < min { "0".into() } else { "#".into() }).collect();
    if grouping && embedded.is_empty() {
        // `#,##0`: a comma between placeholders turns on Excel's thousands separator.
        if let Some(cell) = cells.get_mut(3) {
            cell.push(',');
        }
    }
    let mut out = String::new();
    for (i, cell) in cells.iter().enumerate().rev() {
        out.push_str(cell);
        for (_, text) in embedded.iter().filter(|(p, _)| capped(*p) == i) {
            literal(&mut out, text, DataKind::Number);
        }
    }
    out
}

fn decimals(d: &Digits, default: u32) -> String {
    let places = capped(d.decimals.unwrap_or(default));
    let shown = capped(d.min_decimals.unwrap_or(places as u32)).min(places);
    if places == 0 {
        return String::new();
    }
    format!(".{}{}", "0".repeat(shown), "#".repeat(places - shown))
}

fn number(d: &Digits) -> String {
    if d.decimals.is_none() && d.embedded.is_empty() && d.min_integer <= 1 && (d.factor == 1.0 || d.factor == 0.0) {
        return "General".into();
    }
    let mut out = integer(d.min_integer, d.grouping, &d.embedded);
    out.push_str(&decimals(d, 0));
    // Each trailing comma divides by 1000.
    let mut factor = d.factor;
    while factor >= 1000.0 && (factor / 1000.0).fract() == 0.0 {
        out.push(',');
        factor /= 1000.0;
    }
    out
}

fn scientific(d: &Digits, min_exponent: u32, interval: u32, forced_sign: bool) -> String {
    let min = capped(d.min_integer);
    let mut out = if interval > 1 {
        let width = capped(interval).max(min).max(1);
        format!("{}{}", "#".repeat(width - min.max(1)), "0".repeat(min.max(1)))
    } else if min == 0 {
        "#".into()
    } else {
        "0".repeat(min)
    };
    out.push_str(&decimals(d, 2));
    out.push_str(if forced_sign { "E+" } else { "E-" });
    out.push_str(&"0".repeat(capped(min_exponent).max(1)));
    out
}

fn fraction(min_integer: Option<u32>, numerator: u32, denominator: u32, denominator_value: Option<u32>, grouping: bool) -> String {
    let mut out = String::new();
    if let Some(min) = min_integer {
        out.push_str(&integer(min, grouping, &[]));
        out.push(' ');
    }
    out.push_str(&"?".repeat(capped(numerator).max(1)));
    out.push('/');
    match denominator_value {
        Some(v) if v > 0 => out.push_str(&v.to_string()),
        _ => out.push_str(&"?".repeat(capped(denominator).max(1))),
    }
    out
}

fn date_part(out: &mut String, part: &Part, elapsed: &mut bool) {
    // An elapsed time puts its first (largest) unit in brackets.
    let mut unit = |out: &mut String, code: &str| {
        if std::mem::take(elapsed) {
            out.push('[');
            out.push_str(code);
            out.push(']');
        } else {
            out.push_str(code);
        }
    };
    match part {
        Part::Day { long } => out.push_str(if *long { "dd" } else { "d" }),
        Part::Month { long, textual } => out.push_str(match (textual, long) {
            (true, true) => "mmmm",
            (true, false) => "mmm",
            (false, true) => "mm",
            (false, false) => "m",
        }),
        Part::Year { long } => out.push_str(if *long { "yyyy" } else { "yy" }),
        Part::DayOfWeek { long } => out.push_str(if *long { "dddd" } else { "ddd" }),
        Part::Hours { long } => unit(out, if *long { "hh" } else { "h" }),
        Part::Minutes { long } => unit(out, if *long { "mm" } else { "m" }),
        Part::Seconds { long, decimals } => {
            unit(out, if *long { "ss" } else { "s" });
            if *decimals > 0 {
                out.push('.');
                out.push_str(&"0".repeat(capped(*decimals)));
            }
        }
        Part::AmPm => out.push_str("AM/PM"),
        _ => {}
    }
}

/// Excel's colour names in format codes, then its 56-colour palette (`[ColorN]`) for an
/// exact match; other colours have no code.
pub(crate) fn color_name(hex: &str) -> Option<&'static str> {
    const NAMES: [(u32, &str); 8] = [
        (0x000000, "Black"),
        (0x0000FF, "Blue"),
        (0x00FFFF, "Cyan"),
        (0x00FF00, "Green"),
        (0xFF00FF, "Magenta"),
        (0xFF0000, "Red"),
        (0xFFFFFF, "White"),
        (0xFFFF00, "Yellow"),
    ];
    const PALETTE: [&str; 56] = [
        "Color1", "Color2", "Color3", "Color4", "Color5", "Color6", "Color7", "Color8", "Color9", "Color10", "Color11", "Color12", "Color13",
        "Color14", "Color15", "Color16", "Color17", "Color18", "Color19", "Color20", "Color21", "Color22", "Color23", "Color24", "Color25",
        "Color26", "Color27", "Color28", "Color29", "Color30", "Color31", "Color32", "Color33", "Color34", "Color35", "Color36", "Color37",
        "Color38", "Color39", "Color40", "Color41", "Color42", "Color43", "Color44", "Color45", "Color46", "Color47", "Color48", "Color49",
        "Color50", "Color51", "Color52", "Color53", "Color54", "Color55", "Color56",
    ];
    let h = hex.trim().strip_prefix('#')?;
    if h.len() != 6 {
        return None;
    }
    let rgb = u32::from_str_radix(h, 16).ok()?;
    if let Some((_, name)) = NAMES.iter().find(|(c, _)| *c == rgb) {
        return Some(name);
    }
    // `ColorN` is entry N + 7 of the legacy palette (N = 1..=56).
    let slot = gridcraft_core::color::INDEXED.iter().enumerate().skip(8).find(|(_, c)| **c == rgb).map(|(i, _)| i)?;
    PALETTE.get(slot - 8).copied()
}
