//! Parsing what people type into a cell (and numeric text in formulas).
//!
//! Recognises numbers with thousands separators, percentages, currency, scientific notation,
//! fractions (`1 1/2`), dates (`3/4/2026`, `2026-03-04`, `4-Mar-2026`, `Mar 4, 2026`, `1JUN2021`),
//! times (`10:30`, `10:30:15 PM`) and date-times. Separators, the currency symbol, the date order
//! and the month names are the [`crate::locale::current`] locale's (en-US unless scoped). The date
//! and time grammar is in the `datetext` module.
//!
//! Spaces around the text are ignored for numbers, percentages, currency and fractions, but not
//! for dates and times: Excel keeps `" 2024-01-10 "` as text when coercing. `DATEVALUE` and
//! `TIMEVALUE` do trim it ([`parse_date_time_text`]).

mod datetext;

pub use datetext::current_year;

use crate::date::DateSystem;
use crate::locale::{self, Locale};
use crate::value::{CellError, Value};

/// A parsed entry: its value and the number format Excel would apply automatically.
#[derive(Clone, Debug, PartialEq)]
pub struct Parsed {
    pub value: Value,
    pub format: Option<&'static str>,
}

/// Interprets typed cell input (not formulas). Text that isn't a number, date, boolean or error
/// stays text. A leading apostrophe forces text.
pub fn parse_input(s: &str, sys: DateSystem) -> Parsed {
    if s.is_empty() {
        return Parsed { value: Value::Empty, format: None };
    }
    if let Some(rest) = s.strip_prefix('\'') {
        return Parsed { value: Value::text(rest), format: None };
    }
    let t = s.trim();
    if t.eq_ignore_ascii_case("TRUE") {
        return Parsed { value: Value::Bool(true), format: None };
    }
    if t.eq_ignore_ascii_case("FALSE") {
        return Parsed { value: Value::Bool(false), format: None };
    }
    if let Some(e) = CellError::parse(t) {
        return Parsed { value: Value::Error(e), format: None };
    }
    if let Some((n, f)) = parse_numeric(s, sys, locale::current()) {
        return Parsed { value: Value::number(n), format: f };
    }
    Parsed { value: Value::text(s), format: None }
}

/// Numeric text as arithmetic coerces it ("1,000", "50%", "$3", "1/2/2020", "10:00"). Spaces
/// around a number are ignored; a date or time has to fill the whole text.
pub fn parse_number_text(s: &str) -> Option<f64> {
    parse_number_text_in(s, DateSystem::D1900)
}

/// Like [`parse_number_text`], with date text read in the given workbook date system.
pub fn parse_number_text_in(s: &str, sys: DateSystem) -> Option<f64> {
    parse_numeric(s, sys, locale::current()).map(|(n, _)| n)
}

/// Date and/or time text as `DATEVALUE` and `TIMEVALUE` read it: unlike coercion, they ignore
/// spaces around the text. Returns the serial (the date plus the fraction of a day) and the number
/// format Excel would apply.
pub fn parse_date_time_text(s: &str, sys: DateSystem) -> Option<(f64, &'static str)> {
    datetext::parse_datetime(s.trim(), sys, locale::current())
}

/// Text that Excel's "Number Stored as Text" check flags: it would be a number if typed (signs,
/// thousands separators, currency, percent, exponent, a parenthesised negative). Dates, times and
/// fractions kept as text are not flagged, as in Excel.
pub fn number_stored_as_text(s: &str) -> Option<f64> {
    let t = s.trim();
    let loc = locale::current();
    // The same parentheses typed entry reads, accounting forms included: `$(5)`, `(5)%`.
    match parenthesized(t, loc) {
        Some(inner) if inner.strip_prefix(loc.currency).map_or(inner.as_str(), str::trim_start).starts_with(['-', '+', '(']) => None,
        Some(inner) => parse_plain(&inner, loc).map(|(n, _)| -n),
        None => parse_plain(t, loc).map(|(n, _)| n),
    }
}

fn parse_numeric(raw: &str, sys: DateSystem, loc: &Locale) -> Option<(f64, Option<&'static str>)> {
    let t = raw.trim();
    // Parenthesised negatives: (100) = -100, and the accounting forms `$(100)` and `(5)%`.
    if let Some(inner) = parenthesized(t, loc) {
        // Excel reads no sign and no nested parentheses inside: `(-5)`, `($-5)`, `((5))`.
        if inner.strip_prefix(loc.currency).map_or(inner.as_str(), str::trim_start).starts_with(['-', '+', '(']) {
            return None;
        }
        let r = parse_plain(&inner, loc).or_else(|| parse_fraction(&inner).map(|n| (n, Some("# ?/?"))));
        return r.map(|(n, f)| (-n, f));
    }
    parse_unparenthesised(raw, t, sys, loc)
}

/// `raw` is the text as given and `t` the same without spaces around it.
fn parse_unparenthesised(raw: &str, t: &str, sys: DateSystem, loc: &Locale) -> Option<(f64, Option<&'static str>)> {
    if t.is_empty() {
        return None;
    }
    if let Some(r) = parse_plain(t, loc) {
        return Some(r);
    }
    if let Some(r) = parse_fraction(t) {
        return Some((r, Some("# ?/?")));
    }
    // A date or time is read from the whole text: Excel doesn't coerce `" 2024-01-10 "`.
    if raw.len() == t.len() {
        return datetext::parse_datetime(t, sys, loc).map(|(n, f)| (n, Some(f)));
    }
    None
}

/// The number inside accounting parentheses, as plain text: `(5)` gives `5`. The currency symbol
/// may stand before them (`$(5)` reads as `($5)`) and a percent sign after them (`(5)%` reads as
/// `(5%)`), with spaces around either. The caller reads only a plain number or a fraction from it.
fn parenthesized(t: &str, loc: &Locale) -> Option<String> {
    let (currency, rest) = match t.strip_prefix(loc.currency) {
        Some(r) => (loc.currency, r.trim_start()),
        None => ("", t),
    };
    let rest = rest.strip_prefix('(')?;
    let (body, percent) = match rest.strip_suffix(')') {
        Some(b) => (b, ""),
        None => (rest.strip_suffix('%')?.trim_end().strip_suffix(')')?, "%"),
    };
    Some(format!("{currency}{}{percent}", body.trim()))
}

/// Signs, currency, thousands separators, percent, exponent, with the locale's separators and
/// currency symbol (en-US reads the symbol only in front, other locales also after the number).
fn parse_plain(t: &str, loc: &Locale) -> Option<(f64, Option<&'static str>)> {
    let mut s = t;
    let mut neg = false;
    let mut currency = false;
    let mut percent = false;
    if let Some(r) = s.strip_prefix('-') {
        neg = true;
        s = r.trim_start();
    } else if let Some(r) = s.strip_prefix('+') {
        s = r.trim_start();
    }
    if let Some(r) = s.strip_prefix(loc.currency) {
        currency = true;
        s = r.trim_start();
        if !neg && let Some(r2) = s.strip_prefix('-') {
            neg = true;
            s = r2;
        }
    } else if !loc.is_default()
        && let Some(r) = s.strip_suffix(loc.currency)
    {
        currency = true;
        s = r.trim_end();
    }
    if let Some(r) = s.strip_suffix('%') {
        percent = true;
        s = r.trim_end();
    }
    // Excel reads `$5` and `5%` but not `$5%`.
    if s.is_empty() || currency && percent {
        return None;
    }
    // A no-break-space thousands separator is typed as a space too.
    let is_group = |c: char| c == loc.group || (loc.group == '\u{a0}' && c == ' ');
    let has_sep = s.contains(is_group);
    let (int, frac) = s.split_once(loc.decimal).map_or((s, None), |(a, b)| (a, Some(b)));
    let mut digits: String = if has_sep {
        // Thousands separators must be in groups of three before the decimal point.
        let groups: Vec<&str> = int.split(is_group).collect();
        let first = groups.first()?;
        if first.is_empty() || first.len() > 3 || groups.iter().skip(1).any(|g| g.len() != 3) {
            return None;
        }
        groups.concat()
    } else {
        int.to_string()
    };
    if let Some(f) = frac {
        digits.push('.');
        digits.push_str(f);
    }
    let b = digits.as_bytes();
    // Only the locale's decimal separator, now `.`, may stand for the point.
    if loc.decimal != '.' && loc.group != '.' && int.contains('.')
        || !b.iter().all(|c| c.is_ascii_digit() || matches!(c, b'.' | b'e' | b'E' | b'+' | b'-'))
    {
        return None;
    }
    if !b.first().is_some_and(|c| c.is_ascii_digit() || *c == b'.') {
        return None;
    }
    let mut n: f64 = digits.parse().ok()?;
    if !n.is_finite() {
        return None;
    }
    let sci = digits.contains(['e', 'E']);
    if percent {
        n /= 100.0;
    }
    if neg {
        n = -n;
    }
    let fmt = if currency {
        Some(match (loc.is_default(), digits.contains('.')) {
            (true, true) => "\"$\"#,##0.00",
            (true, false) => "\"$\"#,##0",
            (false, true) => "#,##0.00",
            (false, false) => "#,##0",
        })
    } else if percent {
        Some(if digits.contains('.') { "0.00%" } else { "0%" })
    } else if sci {
        Some("0.00E+00")
    } else if has_sep {
        Some(if digits.contains('.') { "#,##0.00" } else { "#,##0" })
    } else {
        None
    };
    Some((n, fmt))
}

/// A plain ASCII number: digits, with an optional fraction after the `frac` separator when one is
/// given. Rejects `nan`, `inf`, signs and exponents that `f64::from_str` would accept, and digits
/// too many for a finite number (#2163).
fn plain_number(s: &str, frac: Option<char>) -> Option<f64> {
    let (int, dec) = match frac.and_then(|sep| s.split_once(sep)) {
        Some((i, d)) => (i, Some(d)),
        None => (s, None),
    };
    let digits = |x: &str| !x.is_empty() && x.bytes().all(|c| c.is_ascii_digit());
    if !digits(int) || dec.is_some_and(|d| !digits(d)) {
        return None;
    }
    let n: Option<f64> = match dec {
        Some(d) if frac != Some('.') => format!("{int}.{d}").parse().ok(),
        _ => s.parse().ok(),
    };
    n.filter(|x| x.is_finite())
}

/// `1/2` (only with a whole part, else it's a date), `1 1/2`, `0 3/4`.
fn parse_fraction(t: &str) -> Option<f64> {
    let (whole, frac) = t.split_once(' ')?;
    let neg = whole.starts_with('-');
    let w = plain_number(whole.trim_start_matches('-'), None)?;
    let (a, b) = frac.trim().split_once('/')?;
    let a = plain_number(a, None)?;
    let b = plain_number(b, None)?;
    if b == 0.0 || !w.is_finite() {
        return None;
    }
    let v = w + a / b;
    if !v.is_finite() {
        return None;
    }
    Some(if neg { -v } else { v })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Parsed {
        parse_input(s, DateSystem::D1900)
    }

    /// #2163: digits too many for a finite number don't make one (∞/∞ was NaN).
    #[test]
    fn overlong_digits_stay_text() {
        let big = "9".repeat(400);
        for s in [format!("1 {big}/{big}"), format!("1 {big}/2"), format!("1 1/{big}0"), format!("{big}:00"), format!("1:00:{big}.5")] {
            assert_eq!(p(&s).value, Value::text(s.as_str()), "{s}");
            assert_eq!(parse_number_text(&s), None, "{s}");
        }
        assert_eq!(parse_number_text("1 1/2"), Some(1.5));
    }

    #[test]
    fn overlong_time_fields_stay_text() {
        // #2168: all-digit field lengths are gated before allocation/derivation,
        // so hostile strings never spin the time-derivation machinery.
        let big = "9".repeat(500);
        for s in [format!("{big}:00"), format!("1:00:{big}"), format!("{big}:{big}:{big}"), format!("1:0{big}"), format!("00:00:{big}.1")] {
            assert_eq!(p(&s).value, Value::text(s.as_str()), "{s}");
            assert_eq!(parse_number_text(&s), None, "{s}");
        }
        // Within the gate, times still work.
        assert_eq!(p("01:02:03").value, Value::number(3.0 / 86_400.0 + 1.0 / 24.0 + 2.0 / 1440.0));
        assert_eq!(parse_number_text("13:14:15"), Some(0.5515625));
    }

    #[test]
    fn non_digit_time_and_fraction_parts_stay_text() {
        for s in ["1:nan", "nan:30", "1:30:NaN", "1 nan/2", "1 1/nan", "inf:00", "1e1:00", "+5:00"] {
            assert_eq!(p(s).value, Value::text(s), "{s}");
        }
    }

    #[test]
    fn signed_date_parts_stay_text() {
        for s in ["+3/4/2026", "3/+4/2026", "3/4/+2026", "1/1/2020 +5:00"] {
            assert_eq!(p(s).value, Value::text(s), "{s}");
        }
        assert_eq!(p("3/4/2026").value, Value::Number(46085.0));
    }

    #[test]
    fn month_name_short_year() {
        for s in ["Mar 4, 26", "Mar 4 26", "March 4 26", "4 March 26", "Mar 4, 2026"] {
            assert_eq!(p(s).value, Value::Number(46085.0), "{s}");
        }
        assert_eq!(p("Mar 4, 99").value, p("Mar 4, 1999").value);
        assert_eq!(p("Mar 4, 1776").value, Value::text("Mar 4, 1776"));
        assert_eq!(p("Feb 30, 26").value, Value::text("Feb 30, 26"));
    }

    #[test]
    fn numbers() {
        assert_eq!(p("42").value, Value::Number(42.0));
        assert_eq!(p("-3.5").value, Value::Number(-3.5));
        assert_eq!(p("1,234").format, Some("#,##0"));
        assert_eq!(p("1,234").value, Value::Number(1234.0));
        assert_eq!(p("1,23").value, Value::text("1,23"));
        assert_eq!(p("12%").value, Value::Number(0.12));
        assert_eq!(p("$1,200.50").value, Value::Number(1200.5));
        assert_eq!(p("$1,200.50").format, Some("\"$\"#,##0.00"));
        assert_eq!(p("(100)").value, Value::Number(-100.0));
        assert_eq!(p("1e3").value, Value::Number(1000.0));
        assert_eq!(p("1 1/2").value, Value::Number(1.5));
        assert_eq!(p(".5").value, Value::Number(0.5));
    }

    /// Observed in Excel 365 (en-US): typed entry, VALUE and `--"text"` agree.
    #[test]
    fn accounting_parentheses() {
        let accepted: [(&str, f64, Option<&str>); 22] = [
            ("$(5)", -5.0, Some("\"$\"#,##0")),
            ("$ (5)", -5.0, Some("\"$\"#,##0")),
            ("$( 5 )", -5.0, Some("\"$\"#,##0")),
            ("$ ( 1,234 )", -1234.0, Some("\"$\"#,##0")),
            ("$(1,234)", -1234.0, Some("\"$\"#,##0")),
            ("$(1,000.50)", -1000.5, Some("\"$\"#,##0.00")),
            ("$(1,234.)", -1234.0, Some("\"$\"#,##0.00")),
            ("$(.5)", -0.5, Some("\"$\"#,##0.00")),
            ("$(0)", 0.0, Some("\"$\"#,##0")),
            (" $(5) ", -5.0, Some("\"$\"#,##0")),
            ("($5)", -5.0, Some("\"$\"#,##0")),
            ("( $ 1,234 )", -1234.0, Some("\"$\"#,##0")),
            ("(5)", -5.0, None),
            ("( 5 )", -5.0, None),
            ("(1,234.5)", -1234.5, Some("#,##0.00")),
            ("(1e-3)", -0.001, Some("0.00E+00")),
            ("(5)%", -0.05, Some("0%")),
            ("( 5 ) %", -0.05, Some("0%")),
            ("(5%)", -0.05, Some("0%")),
            ("(5.)%", -0.05, Some("0.00%")),
            ("(1,234)%", -12.34, Some("0%")),
            ("(1 1/2)", -1.5, Some("# ?/?")),
        ];
        for (s, n, f) in accepted {
            assert_eq!(p(s), Parsed { value: Value::Number(n), format: f }, "{s}");
            assert_eq!(parse_number_text(s), Some(n), "{s}");
        }
        for s in [
            "$5%",
            "$(5%)",
            "$(5)%",
            "($5%)",
            "($5)%",
            "$ ( 5 ) %",
            "(5)%%",
            "(5%)%",
            "((5))",
            "(-5)",
            "(+5)",
            "($-5)",
            "($ -5)",
            "($+5)",
            "(-$5)",
            "$(-5)",
            "$ (-5)",
            "$(+5)",
            "-(5)",
            "-$(5)",
            "+$(5)",
            "$+(5)",
            "$$(5)",
            "$ $(5)",
            "$((5))",
            "($(5))",
            "(-1 1/2)",
            "()",
            "$()",
            "(5)$",
            "$(5)$",
            "(5)(5)",
            "$(5)0",
            "(5)e3",
            "(5)-",
            "(5-)",
            "$(5-)",
            "$(5",
            "$5)",
            "(5",
            "$(1,23)",
            "$(,5)",
            "(5e)",
            "(1/2)",
            "$(1/2)",
            "(3/4/2026)",
            "$(3/4/2026)",
            "(10:30)",
            "$(10:30)",
        ] {
            assert_eq!(p(s).value, Value::text(s), "{s}");
            assert_eq!(parse_number_text(s), None, "{s}");
        }
    }

    #[test]
    fn deeply_nested_parentheses_do_not_overflow() {
        // #2180: coercion used to recurse once per parenthesis and overflow the stack. Excel reads
        // one pair only, so nesting is text now, and it is rejected without recursing.
        let depth = 100_000;
        let s = format!("{}1{}", "(".repeat(depth), ")".repeat(depth));
        assert_eq!(parse_number_text(&s), None);
        assert_eq!(p(&s).value, Value::text(s.as_str()));
        let odd = format!("{}1{}", "(".repeat(depth + 1), ")".repeat(depth + 1));
        assert_eq!(parse_number_text(&odd), None);
        let unbalanced = format!("{}1{}", "(".repeat(depth), ")".repeat(depth - 1));
        assert_eq!(parse_number_text(&unbalanced), None);
        // A single pair still reads.
        assert_eq!(parse_number_text("(100)"), Some(-100.0));
        assert_eq!(parse_number_text("( $1,200.50 )"), Some(-1200.5));
        assert_eq!(parse_number_text("()"), None);
    }

    #[test]
    fn texts_and_specials() {
        assert_eq!(p("hello").value, Value::text("hello"));
        assert_eq!(p("'123").value, Value::text("123"));
        assert_eq!(p("true").value, Value::Bool(true));
        assert_eq!(p("#N/A").value, Value::Error(CellError::NA));
        assert_eq!(p("e").value, Value::text("e"));
        assert_eq!(p("-").value, Value::text("-"));
        assert_eq!(p("1.2.3.4").value, Value::text("1.2.3.4"));
    }

    #[test]
    fn dates_and_times() {
        assert_eq!(p("10/7/2026").value, Value::Number(46302.0));
        assert_eq!(p("2026-10-07").value, Value::Number(46302.0));
        assert_eq!(p("7-Oct-2026").value, Value::Number(46302.0));
        assert_eq!(p("Oct 7, 2026").value, Value::Number(46302.0));
        assert_eq!(p("2/30/2026").value, Value::text("2/30/2026"));
        assert_eq!(p("7/4/1776").value, Value::text("7/4/1776"));
        assert_eq!(p("1776-07-04").value, Value::text("1776-07-04"));
        assert_eq!(p("4-Jul-1776").value, Value::text("4-Jul-1776"));
        assert_eq!(p("12/31/1899").value, Value::text("12/31/1899"));
        assert_eq!(p("1/1/1900").value, Value::Number(1.0));
        assert_eq!(p("12:00").value, Value::Number(0.5));
        assert_eq!(p("6:00 PM").value, Value::Number(0.75));
        assert_eq!(p("12:00 AM").value, Value::Number(0.0));
        assert_eq!(p("10/7/2026 18:00").value, Value::Number(46302.75));
        assert_eq!(p("25:00").value, Value::Number(25.0 / 24.0));
        assert_eq!(p("25:00").format, Some("[h]:mm:ss"));
        assert_eq!(p("23:59").format, Some("h:mm"));
    }

    /// Excel-authored ironcalc fixtures (SIN/FLOOR/EXP over `=" 2024-01-10 "`, labelled "Non
    /// coerced text date") and an Excel-calculated workbook (`TEXT(" 4/7/13","mm/dd/yy")` stays text): spaces
    /// around a number are ignored, around a date or a time they make the text uncoercible.
    /// `DATEVALUE`/`TIMEVALUE` trim it themselves (DATEVALUE.xlsx B16/B53/B54, TIMEVALUE.xlsx B50).
    #[test]
    fn spaces_around_dates_and_times_block_coercion_but_not_date_time_text() {
        assert_eq!(parse_number_text("2024-01-10"), Some(45301.0));
        assert_eq!(parse_number_text(" 10 "), Some(10.0));
        assert_eq!(parse_number_text(" 1 1/2 "), Some(1.5));
        assert_eq!(parse_number_text(" 50% "), Some(0.5));
        assert_eq!(parse_number_text("4:35 PM"), parse_number_text("16:35"));
        for s in [" 2024-01-10 ", "2024-01-10 ", " 2024-01-10", " 4/7/13", "4/7/13 ", " Mar 4, 2026", " 4:35 PM ", "10:30 ", " 3/4/2026 10:30"] {
            assert_eq!(parse_number_text(s), None, "{s:?}");
            assert_eq!(p(s).value, Value::text(s), "{s:?}");
        }
        let sys = DateSystem::D1900;
        assert_eq!(parse_date_time_text(" 2024-01-10 ", sys), Some((45301.0, "yyyy-mm-dd")));
        assert_eq!(parse_date_time_text("2026-01-01 ", sys), Some((46023.0, "yyyy-mm-dd")));
        assert_eq!(parse_date_time_text(" 4:35 PM ", sys).map(|(n, _)| n), parse_number_text("4:35 PM"));
        assert_eq!(parse_date_time_text("7/7/2026 4:35 PM", sys).map(|(n, _)| n - n.floor()), Some(0.6909722222189885));
        // Plain numbers and words are neither dates nor times.
        for s in [" 10 ", "8", "10.0", "008", "abc", "", " "] {
            assert_eq!(parse_date_time_text(s, sys), None, "{s:?}");
        }
    }

    /// Excel's en-US recognizer has no `.` date separator (ironcalc DATEVALUE.xlsx B52).
    #[test]
    fn dots_do_not_separate_date_parts() {
        for s in ["2026.01.01", "1.5.2026", "3.4.26", "1.2.3.4"] {
            assert_eq!(p(s).value, Value::text(s), "{s}");
            assert_eq!(parse_number_text(s), None, "{s}");
        }
        assert_eq!(p("1.5").value, Value::Number(1.5));
    }

    /// Month names touching numbers (Excel-saved workbooks: `"1"&B1&B2`, `1&H$2`, `MONTH(D2&1)`).
    #[test]
    fn concatenated_month_names_coerce() {
        assert_eq!(parse_number_text("1JUN2021"), Some(44348.0));
        assert_eq!(parse_number_text("1-JAN2024"), Some(45292.0));
        assert_eq!(parse_number_text("May 2022"), parse_number_text("5/1/2022"));
        assert_eq!(parse_number_text("July1"), parse_number_text(&format!("7/1/{}", current_year())));
        assert_eq!(p("1Oct 21").format, Some("d-mmm-yy"));
        assert_eq!(p("May 2022").format, Some("mmm-yy"));
        assert_eq!(p("Oct 21").format, Some("d-mmm"));
    }

    #[test]
    fn fictitious_1900_02_29() {
        // Excel accepts 29 February 1900 (serial 60) and keeps 1 March 1900 at 61.
        assert_eq!(p("2/29/1900").value, Value::Number(60.0));
        assert_eq!(p("1900-02-29").value, Value::Number(60.0));
        assert_eq!(p("29-Feb-1900").value, Value::Number(60.0));
        assert_eq!(p("2/28/1900").value, Value::Number(59.0));
        assert_eq!(p("3/1/1900").value, Value::Number(61.0));
        assert_eq!(p("2/29/1901").value, Value::text("2/29/1901"));
        assert_eq!(p("2/30/1900").value, Value::text("2/30/1900"));
        // The 1904 system starts after it.
        assert_eq!(parse_input("2/29/1900", DateSystem::D1904).value, Value::text("2/29/1900"));
        assert_eq!(parse_input("2/29/1904", DateSystem::D1904).value, Value::Number(59.0));
    }

    #[test]
    fn numbers_stored_as_text() {
        for (text, n) in [
            ("123", 123.0),
            (" 42 ", 42.0),
            ("-1,000.5", -1000.5),
            ("$3", 3.0),
            ("50%", 0.5),
            ("1E3", 1000.0),
            ("(100)", -100.0),
            ("$(5)", -5.0),
            ("(5)%", -0.05),
        ] {
            assert_eq!(number_stored_as_text(text), Some(n), "{text}");
        }
        // Dates, times, fractions, words and blanks are not flagged.
        for text in ["3/4/2026", "10:30", "1 1/2", "abc", "12abc", "", "  ", "TRUE", "#N/A", "((1))", "(-5)", "$(5)%", "(1/2)"] {
            assert_eq!(number_stored_as_text(text), None, "{text}");
        }
    }

    /// Coercion reads the separators, currency and date order of the locale in effect.
    #[test]
    fn numbers_and_dates_in_another_locale() {
        use crate::locale::{DE_DE, EN_GB, ES_ES, FR_FR, scoped};
        let n = parse_number_text;
        scoped(&DE_DE, || {
            assert_eq!(n("1,5"), Some(1.5));
            assert_eq!(n("1.234,5"), Some(1234.5));
            assert_eq!(n("-1.234"), Some(-1234.0));
            assert_eq!(n("7,5%"), Some(0.075));
            assert_eq!(n("23€"), Some(23.0));
            assert_eq!(n("23 €"), Some(23.0));
            assert_eq!(n("€100"), Some(100.0));
            assert_eq!(n("$5"), None);
            assert_eq!(n("1,2.3"), None);
            assert_eq!(n("16.02.24"), Some(45338.0));
            assert_eq!(n("1.5"), parse_number_text_in("1/5", DateSystem::D1900));
        });
        scoped(&FR_FR, || {
            assert_eq!(n("1 234,5"), Some(1234.5));
            assert_eq!(n("1\u{a0}234"), Some(1234.0));
            assert_eq!(n("1.5"), None);
        });
        scoped(&ES_ES, || assert_eq!(n("10-12-1999"), Some(36504.0)));
        // en-IE: en-GB's dates and point, the euro before or after (ironcalc's fixtures' Excel).
        scoped(&crate::locale::EN_IE, || {
            assert_eq!((n("23€"), n(" €100 "), n("7.0e0"), n("123,456.67"), n("12,34")), (Some(23.0), Some(100.0), Some(7.0), Some(123456.67), None));
        });
        scoped(&EN_GB, || {
            assert_eq!(n("01/09/2020 15:02:40").map(f64::floor), Some(44075.0));
            assert_eq!(n("1,234.5"), Some(1234.5));
        });
        // en-US is unchanged.
        assert_eq!(n("1,5"), None);
        assert_eq!(n("23€"), None);
        assert_eq!(n("01/09/2020"), Some(43839.0));
        assert_eq!(Value::Number(1.5).to_text(), Ok("1.5".into()));
        assert_eq!(scoped(&DE_DE, || Value::Number(1.5).to_text()), Ok("1,5".into()));
    }
}
