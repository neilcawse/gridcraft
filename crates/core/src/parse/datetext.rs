//! Date and time recognition in text, shared by typed cell input, `VALUE`/`DATEVALUE`/`TIMEVALUE`
//! and the arithmetic coercion of text (`"1-Jan-2024"+1`), in a [`Locale`]: its date order, month
//! names, decimal separator for seconds and, where it is `.`, its date separator.
//!
//! The reader works on tokens, not on separators. A date is two or three *items*, each a run of
//! digits or a run of letters, with `/`, `-`, `,` or spaces between them, or nothing at all
//! between a digit run and a letter run (`1JUN2021`, `July1`, `1-JAN2024`, `1Oct 21`):
//!
//! * all digits: `m/d/y`, `m-d-y`, `y-m-d`, `m/d` (this year), `m/yyyy` (in the locale's order);
//!   the separators must agree;
//! * one month name (a name or a prefix of at least three letters) and two numbers: day, month and
//!   year in the orders `d mmm y`, `mmm d y` and `y mmm d`;
//! * one month name and one number `N`: the day of this year when `N` is a valid day of that month
//!   (`Oct 21`, `July1`), else the year, on day 1 (`May 2022`, `Feb 30`, `January0`).
//!
//! Two-digit years use Excel's 1930..=2029 window. A date may be followed by a time after a space.
//! Times take spaces around the colons (`4 : 35`) and seconds up to 60 (`23:59:60` is `24:00`).
//! A `.` separates date parts only in a locale that writes dates with it (`16.02.24` in de-DE),
//! so in en-US `2026.01.01` is text.
//!
//! Callers decide about surrounding spaces: the readers here need the text without any, because
//! Excel coerces `" 2024-01-10 "` only in `DATEVALUE` and `TIMEVALUE`, which trim it themselves.

use super::plain_number;
use crate::date::{DateSystem, days_in_month_in, serial_from_ymd, time_fraction};
use crate::locale::{DateOrder, Locale};

/// Longest text a date is looked for in: the longest accepted form (`September 30, 2026`) is far
/// shorter, so anything longer is rejected before it is tokenized.
const MAX_DATE_LEN: usize = 32;
/// Longest time (`10:30:15.123456789012 PM`), and so the longest tail tried after a date.
const MAX_TIME_LEN: usize = 36;
/// Longest date followed by a time.
const MAX_DATETIME_LEN: usize = MAX_DATE_LEN + 1 + MAX_TIME_LEN;

/// Separator between two items.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sep {
    Space,
    Comma,
    Dash,
    Slash,
    Dot,
}

#[derive(Clone, Copy)]
struct Item<'a> {
    text: &'a str,
    /// A run of letters; otherwise a run of digits.
    word: bool,
}

/// The items of a date, at most three. `seps[i]` sits between item `i` and `i + 1` (`None` when a
/// run of digits touches a run of letters).
struct Items<'a> {
    items: [Item<'a>; 3],
    seps: [Option<Sep>; 2],
    len: usize,
}

impl<'a> Items<'a> {
    fn text(&self, i: usize) -> Option<&'a str> {
        self.items.get(i).filter(|_| i < self.len).map(|x| x.text)
    }

    /// The separators between the items, when every one is `/`, every one is `-` or every one
    /// is `.`.
    fn uniform_numeric_separator(&self) -> Option<Sep> {
        let used = self.seps.get(..self.len.saturating_sub(1))?;
        let first = (*used.first()?)?;
        (matches!(first, Sep::Dash | Sep::Slash | Sep::Dot) && used.iter().all(|s| *s == Some(first))).then_some(first)
    }
}

/// A run of separator characters starting at `start`: one `/`, `-` or `.` (`.` only when `dot` is
/// set) with optional spaces on either side (`2026 - 01 - 01`, as Excel's `DATEVALUE` reads it), a
/// `,` with spaces after it, or spaces. Returns it and the index after it.
fn separator_run(b: &[u8], start: usize, dot: bool) -> Option<(Sep, usize)> {
    let skip_spaces = |mut i: usize| {
        while b.get(i) == Some(&b' ') {
            i += 1;
        }
        i
    };
    let after_spaces = skip_spaces(start);
    let punct = match b.get(after_spaces) {
        Some(b'-') => Some(Sep::Dash),
        Some(b'/') => Some(Sep::Slash),
        Some(b'.') if dot => Some(Sep::Dot),
        Some(b',') if after_spaces == start => Some(Sep::Comma),
        _ => None,
    };
    match punct {
        Some(sep) => Some((sep, skip_spaces(after_spaces + 1))),
        None if after_spaces > start => Some((Sep::Space, after_spaces)),
        None => None,
    }
}

/// Splits `t` into at most three digit-run / letter-run items and the separators between them.
/// Anything else (another character, a leading, trailing or doubled separator, a fourth item) is
/// not a date. `.` separates only in a locale that writes dates with it. Letters are any
/// alphabetic characters (`März`, `février`); an abbreviation the locale writes with a period
/// (`janv.` in fr-FR) keeps it.
fn split_items<'a>(t: &'a str, loc: &Locale) -> Option<Items<'a>> {
    let dot = loc.date_separator == '.';
    let b = t.as_bytes();
    let mut out = Items { items: [Item { text: "", word: false }; 3], seps: [None; 2], len: 0 };
    let mut sep: Option<Sep> = None;
    let mut i = 0;
    while let Some(&c) = b.get(i) {
        if matches!(c, b' ' | b',' | b'-' | b'/') || (dot && c == b'.') {
            if out.len == 0 || sep.is_some() {
                return None;
            }
            let (s, next) = separator_run(b, i, dot)?;
            sep = Some(s);
            i = next;
            continue;
        }
        let start = i;
        let word = t.get(i..)?.chars().next().is_some_and(char::is_alphabetic);
        if word {
            i += t.get(i..)?.chars().take_while(|x| x.is_alphabetic()).map(char::len_utf8).sum::<usize>();
            if b.get(i) == Some(&b'.') && t.get(start..=i).is_some_and(|w| loc.months_short.iter().any(|m| same_letters(m, w))) {
                i += 1;
            }
        } else if c.is_ascii_digit() {
            while b.get(i).is_some_and(u8::is_ascii_digit) {
                i += 1;
            }
        } else {
            return None;
        }
        if let Some(prev) = out.len.checked_sub(1) {
            *out.seps.get_mut(prev)? = sep.take();
        }
        *out.items.get_mut(out.len)? = Item { text: t.get(start..i)?, word };
        out.len += 1;
    }
    (sep.is_none() && out.len >= 2).then_some(out)
}

/// Whether `a` and `b` are the same letters, case-insensitively (`MÄRZ` and `März`).
fn same_letters(a: &str, b: &str) -> bool {
    a.chars().flat_map(char::to_lowercase).eq(b.chars().flat_map(char::to_lowercase))
}

/// 1-12 for one of the locale's month names, its abbreviations (`Mrz` in de-DE, `mrt` in nl-NL,
/// `févr.` in fr-FR) or a prefix of a name at least three letters long, case-insensitive.
fn month_from_name(s: &str, loc: &Locale) -> Option<i64> {
    let n = s.chars().count();
    if n < 3 {
        return None;
    }
    let prefix = |m: &&str| m.chars().count() >= n && s.chars().flat_map(char::to_lowercase).eq(m.chars().take(n).flat_map(char::to_lowercase));
    let at = loc.months_short.iter().position(|m| same_letters(m, s)).or_else(|| loc.months.iter().position(prefix))?;
    i64::try_from(at + 1).ok()
}

/// The value of a run of at most four digits.
fn small(s: &str) -> Option<i64> {
    if s.len() > 4 { None } else { s.parse().ok() }
}

/// A one- or two-digit day or month.
fn day_or_month(s: &str) -> Option<i64> {
    if s.len() > 2 { None } else { small(s) }
}

/// A year: two digits use the 1930..=2029 window, four digits stand for themselves.
fn year(s: &str) -> Option<i64> {
    let y = small(s)?;
    match s.len() {
        1 | 2 => Some(if y < 30 { 2000 + y } else { 1900 + y }),
        4 => Some(y),
        _ => None,
    }
}

/// Serial of a valid date. Dates before 1900 stay text (`DATE()` would add 1900 to such a year).
fn valid_ymd(sys: DateSystem, y: i64, m: i64, d: i64) -> Option<f64> {
    if !(1..=12).contains(&m) || d < 1 || y < 1900 {
        return None;
    }
    let max = i64::from(days_in_month_in(sys, i32::try_from(y).ok()?, u32::try_from(m).ok()?));
    if d > max {
        return None;
    }
    serial_from_ymd(sys, y, m, d)
}

/// The year used for dates typed without one, from the system clock (browser clock on wasm).
/// Falls back to 2026 only when no clock is available.
pub fn current_year() -> i64 {
    if let Some(ms) = crate::date::unix_now_ms() {
        let days = (ms / 86_400_000.0).floor() as i64;
        return i64::from(crate::date::civil_from_days(days).0);
    }
    2026
}

/// `y-m-d` / `m/d/y` / `d/m/y` / `yy-m-d` in the locale's order, or the two-field `m/d` (`d/m`)
/// and `m/yyyy`.
fn numeric_date(it: &Items<'_>, sys: DateSystem, order: DateOrder) -> Option<(f64, &'static str)> {
    it.uniform_numeric_separator()?;
    let (a, b) = (it.text(0)?, it.text(1)?);
    let month_day = |m: &str, d: &str| match order {
        DateOrder::MonthDayYear | DateOrder::YearMonthDay => (day_or_month(m), day_or_month(d)),
        DateOrder::DayMonthYear => (day_or_month(d), day_or_month(m)),
    };
    match it.text(2) {
        Some(c) if a.len() == 4 => valid_ymd(sys, small(a)?, day_or_month(b)?, day_or_month(c)?).map(|s| (s, "yyyy-mm-dd")),
        Some(c) if order == DateOrder::YearMonthDay => valid_ymd(sys, year(a)?, day_or_month(b)?, day_or_month(c)?).map(|s| (s, "yyyy-mm-dd")),
        Some(c) => {
            let (m, d) = month_day(a, b);
            valid_ymd(sys, year(c)?, m?, d?).map(|s| (s, "m/d/yyyy"))
        }
        // `m/yyyy` is the first of that month; `m/d` is that day of this year.
        None if b.len() == 4 => valid_ymd(sys, small(b)?, day_or_month(a)?, 1).map(|s| (s, "mmm-yy")),
        None => {
            let (m, d) = month_day(a, b);
            valid_ymd(sys, current_year(), m?, d?).map(|s| (s, "d-mmm"))
        }
    }
}

/// A month name next to one number: this year's day when the number is a valid day of the month,
/// else the year, on day 1.
fn month_and_number(sys: DateSystem, month: i64, number: &str) -> Option<(f64, &'static str)> {
    if number.len() <= 2 {
        let this_year = current_year();
        let n = small(number)?;
        let days = i64::from(days_in_month_in(sys, i32::try_from(this_year).ok()?, u32::try_from(month).ok()?));
        if (1..=days).contains(&n) {
            return valid_ymd(sys, this_year, month, n).map(|s| (s, "d-mmm"));
        }
    }
    valid_ymd(sys, year(number)?, month, 1).map(|s| (s, "mmm-yy"))
}

/// A date with one month name: `d mmm y`, `mmm d y`, `y mmm d`, `mmm N`, `N mmm`.
fn month_name_date(it: &Items<'_>, sys: DateSystem, loc: &Locale) -> Option<(f64, &'static str)> {
    let at = it.items.iter().take(it.len).position(|x| x.word)?;
    let month = month_from_name(it.text(at)?, loc)?;
    let fmt = "d-mmm-yy";
    match (it.len, at) {
        (2, _) => month_and_number(sys, month, it.text(1 - at)?),
        (3, 0) => valid_ymd(sys, year(it.text(2)?)?, month, day_or_month(it.text(1)?)?).map(|s| (s, fmt)),
        (3, 1) => {
            let (a, c) = (it.text(0)?, it.text(2)?);
            let (y, d) = if a.len() == 4 { (small(a)?, day_or_month(c)?) } else { (year(c)?, day_or_month(a)?) };
            valid_ymd(sys, y, month, d).map(|s| (s, fmt))
        }
        _ => None,
    }
}

/// A date as typed, with the number format Excel applies to it.
pub(super) fn parse_date(t: &str, sys: DateSystem, loc: &Locale) -> Option<(f64, &'static str)> {
    if t.len() > MAX_DATE_LEN {
        return None;
    }
    let items = split_items(t, loc)?;
    let words = items.items.iter().take(items.len).filter(|x| x.word).count();
    match words {
        0 => numeric_date(&items, sys, loc.date_order),
        1 => month_name_date(&items, sys, loc),
        _ => None,
    }
}

/// `4 pm` / `4:35 Am` / `4:35p`: the text without its meridiem, and whether that was PM.
fn split_meridiem(t: &str) -> (&str, Option<bool>) {
    let b = t.as_bytes();
    let letter = |i: usize| b.get(i).map(u8::to_ascii_lowercase);
    let n = b.len();
    let (cut, p) = match (n.checked_sub(2).and_then(letter), n.checked_sub(1).and_then(letter)) {
        (Some(x @ (b'a' | b'p')), Some(b'm')) => (n - 2, x),
        (_, Some(x @ (b'a' | b'p'))) => (n - 1, x),
        _ => return (t, None),
    };
    (t.get(..cut).unwrap_or(t), Some(p == b'p'))
}

/// A time as typed: `h:mm`, `h:mm:ss`, `h:mm:ss.000` (the locale's decimal separator), `h AM`,
/// with spaces around the colons. Minutes stop at 59 and seconds at 60, which carries into the
/// next minute. Returns the fraction of a day, whether seconds were given and whether there was a
/// meridiem.
pub(super) fn parse_time(t: &str, loc: &Locale) -> Option<(f64, bool, bool)> {
    if t.len() > MAX_TIME_LEN {
        return None;
    }
    let (body, ampm) = split_meridiem(t);
    let mut parts = body.trim_end().split(':');
    let (h, m, s) = (parts.next(), parts.next(), parts.next());
    if parts.next().is_some() || (m.is_none() && ampm.is_none()) {
        return None;
    }
    // Two fields with a fraction of a second are minutes and seconds: `58:59.736`.
    if s.is_none()
        && ampm.is_none()
        && let Some(sec) = m.filter(|p| p.contains(loc.decimal))
    {
        let minute = plain_number(h?.trim_end(), None)?;
        let second = plain_number(sec.trim(), Some(loc.decimal))?;
        return (minute < 60.0 && second < 60.0).then(|| (time_fraction(0.0, minute, second), true, false));
    }
    let mut hour = plain_number(h?.trim_end(), None)?;
    let minute = m.map_or(Some(0.0), |p| plain_number(p.trim(), None))?;
    let second = s.map_or(Some(0.0), |p| plain_number(p.trim(), Some(loc.decimal)))?;
    if minute >= 60.0 || second > 60.0 {
        return None;
    }
    match ampm {
        Some(pm) => {
            if hour > 12.0 {
                return None;
            }
            if hour == 12.0 {
                hour = 0.0;
            }
            if pm {
                hour += 12.0;
            }
        }
        None if hour >= 10000.0 => return None,
        None => {}
    }
    Some((time_fraction(hour, minute, second), s.is_some(), ampm.is_some()))
}

/// The number format Excel applies to a time typed in this form.
fn time_format(fraction: f64, secs: bool, ampm: bool) -> &'static str {
    match (secs, ampm) {
        // 24 hours or more: elapsed hours, so 25:00 shows as 25:00:00, not 1:00.
        _ if fraction >= 1.0 => "[h]:mm:ss",
        (false, false) => "h:mm",
        (true, false) => "h:mm:ss",
        (false, true) => "h:mm AM/PM",
        (true, true) => "h:mm:ss AM/PM",
    }
}

/// A date, a time, or a date and a time after a space (`3/4/2026 10:30`), with the number format
/// Excel applies. The text must not have spaces around it.
pub(super) fn parse_datetime(t: &str, sys: DateSystem, loc: &Locale) -> Option<(f64, &'static str)> {
    if t.len() > MAX_DATETIME_LEN {
        return None;
    }
    if let Some((fraction, secs, ampm)) = parse_time(t, loc) {
        return Some((fraction, time_format(fraction, secs, ampm)));
    }
    if let Some(date) = parse_date(t, sys, loc) {
        return Some(date);
    }
    // Only a tail that is short and starts with a digit can be a time, so the number of tries
    // stays bounded however many spaces a long text has.
    for (i, _) in t.match_indices(' ') {
        let Some(tail) = t.get(i + 1..).filter(|b| b.len() <= MAX_TIME_LEN && b.starts_with(|c: char| c.is_ascii_digit())) else { continue };
        let Some((fraction, _, _)) = parse_time(tail, loc) else { continue };
        // The browser's `3/4/2026, 10:30:15 AM` has a comma after the date.
        let head = t.get(..i).map(|a| a.strip_suffix(',').unwrap_or(a));
        if let Some((date, _)) = head.and_then(|a| parse_date(a, sys, loc)) {
            return Some((date + fraction, "m/d/yyyy h:mm"));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::locale::{DE_DE, EN_GB, EN_US, ES_ES, FR_FR, NL_NL, SV_SE, TR_TR};

    const SYS: DateSystem = DateSystem::D1900;

    fn date(t: &str) -> Option<f64> {
        parse_date(t, SYS, &EN_US).map(|(n, _)| n)
    }

    fn ymd(y: i64, m: i64, d: i64) -> Option<f64> {
        serial_from_ymd(SYS, y, m, d)
    }

    #[test]
    fn month_names_and_numbers_in_any_token_order() {
        // Plan App. D class A witnesses: no separator at a digit/letter boundary, mixed separators.
        assert_eq!(date("1JUN2021"), Some(44348.0));
        assert_eq!(date("1-JAN2024"), Some(45292.0));
        assert_eq!(date("1Jan2016"), ymd(2016, 1, 1));
        assert_eq!(date("1Oct 21"), ymd(2021, 10, 1));
        assert_eq!(date("4-Mar-2026"), Some(46085.0));
        assert_eq!(date("4 Mar 2026"), Some(46085.0));
        assert_eq!(date("Mar4,2026"), Some(46085.0));
        assert_eq!(date("Mar-4-2026"), Some(46085.0));
        assert_eq!(date("2026-Mar-04"), Some(46085.0));
        assert_eq!(date("2026 Mar 4"), Some(46085.0));
        assert_eq!(date("Sept 4 2026"), ymd(2026, 9, 4));
    }

    #[test]
    fn month_and_one_number_is_a_day_this_year_or_else_a_year() {
        let y = current_year();
        assert_eq!(date("Oct 21"), ymd(y, 10, 21));
        assert_eq!(date("July1"), ymd(y, 7, 1));
        assert_eq!(date("01-Sep"), ymd(y, 9, 1));
        assert_eq!(date("Jan 22"), ymd(y, 1, 22));
        assert_eq!(date("5 May"), ymd(y, 5, 5));
        // Not a day of that month: the number is the year, on day 1.
        assert_eq!(date("May 2022"), ymd(2022, 5, 1));
        assert_eq!(date("January0"), ymd(2000, 1, 1));
        assert_eq!(date("Feb 30"), ymd(1930, 2, 1));
        assert_eq!(date("Apr 31"), ymd(1931, 4, 1));
        assert_eq!(date("Mar 32"), ymd(1932, 3, 1));
        assert_eq!(date("2022-May"), ymd(2022, 5, 1));
        // Years before 1900 and three- or five-digit numbers are not dates.
        assert_eq!(date("May 1776"), None);
        assert_eq!(date("May 202"), None);
        assert_eq!(date("May 20226"), None);
    }

    #[test]
    fn numeric_dates() {
        assert_eq!(date("3/4/2026"), Some(46085.0));
        assert_eq!(date("3-4-2026"), Some(46085.0));
        assert_eq!(date("03/04/26"), Some(46085.0));
        assert_eq!(date("2026-03-04"), Some(46085.0));
        assert_eq!(date("2026/3/4"), Some(46085.0));
        assert_eq!(date("3/4/99"), ymd(1999, 3, 4));
        assert_eq!(date("3/4/29"), ymd(2029, 3, 4));
        assert_eq!(date("3/4/30"), ymd(1930, 3, 4));
        assert_eq!(date("3/2026"), ymd(2026, 3, 1));
        assert_eq!(date("3/4"), ymd(current_year(), 3, 4));
        // Spaces around a separator (Excel 16 caches `DATEVALUE("2026 - 01 - 01")` as 46023).
        assert_eq!(date("2026 - 01 - 01"), Some(46023.0));
        assert_eq!(date("3 / 4 / 2026"), Some(46085.0));
        assert_eq!(date("3 -4/2026"), None);
        assert_eq!(date("4 Mar , 2026"), None);
    }

    #[test]
    fn malformed_numeric_dates() {
        // Separators must agree, and only `/` and `-` are separators in en-US.
        assert_eq!(date("3/4-2026"), None);
        assert_eq!(date("3 4 2026"), None);
        assert_eq!(date("2026.01.01"), None);
        assert_eq!(date("1.5.2026"), None);
        assert_eq!(date("3.4.26"), None);
        // Out-of-range parts, the 1900 floor and the 9999 ceiling.
        assert_eq!(date("13/4/2026"), None);
        assert_eq!(date("2026-13-01"), None);
        assert_eq!(date("2026-01-32"), None);
        assert_eq!(date("2026-04-31"), None);
        assert_eq!(date("2100-02-29"), None);
        assert_eq!(date("1789-05-05"), None);
        assert_eq!(date("10000-01-01"), None);
        assert_eq!(date("9999-12-31"), Some(2_958_465.0));
        assert_eq!(date("1900-02-29"), Some(60.0));
        // Stray characters and signs.
        assert_eq!(date("-2026-01-01"), None);
        assert_eq!(date("\"2026-01-12\""), None);
        assert_eq!(date("2026-01-01x"), None);
        assert_eq!(date("3/4/2026/"), None);
        assert_eq!(date("/3/4/2026"), None);
        assert_eq!(date("3//4/2026"), None);
        assert_eq!(date("3/4/2026,"), None);
    }

    #[test]
    fn day_first_order_comes_with_the_locale() {
        let dmy = |t: &str| parse_date(t, SYS, &EN_GB).map(|(n, _)| n);
        assert_eq!(dmy("3/4/2026"), ymd(2026, 4, 3));
        assert_eq!(dmy("15/08/2004"), ymd(2004, 8, 15));
        assert_eq!(dmy("03/02/24"), ymd(2024, 2, 3));
        assert_eq!(dmy("2026-03-04"), Some(46085.0));
        assert_eq!(dmy("4 Mar 2026"), Some(46085.0));
        assert_eq!(dmy("3/4"), ymd(current_year(), 4, 3));
        assert_eq!(dmy("4/2026"), ymd(2026, 4, 1));
        assert_eq!(dmy("16.02.24"), None);
        assert_eq!(date("15/08/2004"), None);
    }

    #[test]
    fn other_locales_dates() {
        let de = |t: &str| parse_date(t, SYS, &DE_DE).map(|(n, _)| n);
        assert_eq!(de("16.02.24"), ymd(2024, 2, 16));
        assert_eq!(de("16/02/2024"), ymd(2024, 2, 16));
        assert_eq!(de("16.02/2024"), None);
        // Month names in any letters, and the abbreviations TEXT's `mmm` writes in the locale.
        assert_eq!(de("4 März 2026"), ymd(2026, 3, 4));
        assert_eq!(de("4 MÄRZ 2026"), ymd(2026, 3, 4));
        assert_eq!(de("4-Mrz-2026"), ymd(2026, 3, 4));
        assert_eq!(de("4-Mär-2026"), ymd(2026, 3, 4));
        let nl = |t: &str| parse_date(t, SYS, &NL_NL).map(|(n, _)| n);
        assert_eq!(nl("4-mrt-2026"), ymd(2026, 3, 4));
        assert_eq!(nl("4 maart 2026"), ymd(2026, 3, 4));
        let fr = |t: &str| parse_date(t, SYS, &FR_FR).map(|(n, _)| n);
        assert_eq!(fr("4 février 2026"), ymd(2026, 2, 4));
        assert_eq!(fr("4-févr.-2026"), ymd(2026, 2, 4));
        assert_eq!(fr("4-déc.-2026"), ymd(2026, 12, 4));
        assert_eq!(fr("4-fév.-2026"), None, "a period only after the locale's own abbreviation");
        assert_eq!(date("4-Jan.-2026"), None);
        assert_eq!(de("4 Mai 2026"), ymd(2026, 5, 4));
        // Month names are the locale's: an English one is no date in Spanish.
        let es = |t: &str| parse_date(t, SYS, &ES_ES).map(|(n, _)| n);
        assert_eq!(es("1 julio 2026"), ymd(2026, 7, 1));
        assert_eq!(es("July 1, 2026"), None);
        assert_eq!(es("10-12-1999"), ymd(1999, 12, 10));
        let sv = |t: &str| parse_date(t, SYS, &SV_SE).map(|(n, _)| n);
        assert_eq!(sv("26-03-04"), ymd(2026, 3, 4));
        assert_eq!(sv("2026-03-04"), ymd(2026, 3, 4));
    }

    #[test]
    fn seconds_take_the_locale_decimal_separator() {
        let t = |s: &str, loc: &Locale| parse_time(s, loc).map(|(f, _, _)| f);
        assert_eq!(t("10:30:15,5", &DE_DE), Some(time_fraction(10.0, 30.0, 15.5)));
        assert_eq!(t("10:30:15.5", &DE_DE), None);
        assert_eq!(t("10:30:15.5", &EN_US), Some(time_fraction(10.0, 30.0, 15.5)));
        // Minutes and seconds when the second field has a fraction.
        assert_eq!(t("58:59.736", &EN_US), Some(time_fraction(0.0, 58.0, 59.736)));
        assert_eq!(t("58:59,736", &TR_TR), Some(time_fraction(0.0, 58.0, 59.736)));
        assert_eq!(t("60:00.5", &EN_US), None);
    }

    #[test]
    fn words_that_are_not_months_are_not_dates() {
        for t in ["Alice", "Marx 4", "Ma 4", "Mar Apr", "4 Mar Apr", "Mar 4 Apr", "Mar 4 26 1", "4 26 Mar", "x4", "4x", "Mar4th"] {
            assert_eq!(date(t), None, "{t}");
        }
    }

    #[test]
    fn times() {
        let t = |s: &str| parse_time(s, &EN_US).map(|(f, _, _)| f);
        assert_eq!(t("4:35 AM"), Some(time_fraction(4.0, 35.0, 0.0)));
        assert_eq!(t("4:35 Am"), t("4:35 AM"));
        assert_eq!(t("4:35a"), t("4:35 AM"));
        assert_eq!(t("4 pm"), Some(16.0 / 24.0));
        assert_eq!(t("4:5 pm"), Some((16.0 * 60.0 + 5.0) / 1440.0));
        assert_eq!(t("12:00 AM"), Some(0.0));
        assert_eq!(t("12:00 PM"), Some(0.5));
        assert_eq!(t("4:35"), t("04:35"));
        // Spaces around the colons.
        assert_eq!(t("4 : 35"), t("4:35"));
        assert_eq!(t("4 :35 : 20 pm"), t("4:35:20 pm"));
        // Seconds carry over: 23:59:60 is 24:00.
        assert_eq!(t("23:59:60"), Some(1.0));
        assert_eq!(t("24:00"), Some(1.0));
        assert_eq!(t("0:0:86400"), None);
        assert_eq!(t("0:0:61"), None);
        assert_eq!(t("0:60"), None);
        assert_eq!(t("0:00:01"), Some(1.0 / 86400.0));
        // Not times.
        for s in ["13:00 PM", "4:35 A.M.", "4:35 PMx", ":35", "4::35", "1600", "4.35", "-4:35 AM", "4", "1:2:3:4", "4:35 P M", "4:35 p.m."] {
            assert_eq!(t(s), None, "{s}");
        }
    }

    #[test]
    fn date_and_time_together() {
        let dt = |s: &str| parse_datetime(s, SYS, &EN_US).map(|(n, _)| n);
        assert_eq!(dt("2026-07-16 4:35 PM"), Some(46219.0 + (16.0 * 60.0 + 35.0) / 1440.0));
        assert_eq!(dt("2026-01-01 00:00:00"), Some(46023.0));
        assert_eq!(dt("3/4/2026 10:30"), Some(46085.0 + 10.5 / 24.0));
        assert_eq!(dt("3/4/2026, 10:30:15 AM"), Some(46085.0 + (10.0 * 3600.0 + 30.0 * 60.0 + 15.0) / 86400.0));
        assert_eq!(dt("Mar 4, 2026 6 PM"), Some(46085.75));
        assert_eq!(dt("3/4/2026 +5:00"), None);
        assert_eq!(dt("3/4/2026  10:30"), None);
        assert_eq!(dt("10:30 3/4/2026"), None);
        assert_eq!(parse_datetime("2026-01-01 23:59:59", SYS, &EN_US).map(|(_, f)| f), Some("m/d/yyyy h:mm"));
    }

    #[test]
    fn long_text_is_rejected_without_work() {
        let long = "1 ".repeat(5000);
        assert_eq!(parse_datetime(&long, SYS, &EN_US), None);
        let spaced = format!("{}3/4/2026 10:30", "a ".repeat(5000));
        assert_eq!(parse_datetime(&spaced, SYS, &EN_US), None);
        // Just inside the length gate, many spaces still end quickly with no date.
        assert_eq!(parse_datetime(&"1 ".repeat(34), SYS, &EN_US), None);
    }
}
