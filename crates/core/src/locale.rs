//! The regional settings Excel reads text and writes numbers with: the order of an all-numeric
//! date, the decimal and thousands separators, the currency symbol, the month and day names, and
//! the letters a `TEXT` format code spells its date parts with.
//!
//! A workbook doesn't record the locale of the Excel that calculated it, so GridCraft calculates
//! as en-US Excel does unless told otherwise. The locale in effect is per thread: [`current`] is
//! [`EN_US`] until [`scoped`] runs a closure under another one. The text→number coercion
//! (`crate::parse`), `&` (`Value::to_text`) and number formatting read [`current`], so a
//! recalculation run inside [`scoped`] gives the answers an Excel with those settings would.
//! `gridcraft-cli check` uses it to tell a cache computed under another locale ([`CANDIDATES`])
//! from a calculation defect.
//!
//! Data: the Windows regional defaults of each language (public CLDR/Windows locale data), with
//! the month and day names Excel shows for `mmmm`, `mmm`, `dddd` and `ddd`. The only localized
//! format letter is Swedish `Å` for the year (`ÅÅÅÅ-MM-DD`), seen in Excel caches; other languages
//! keep the English letters until a cache shows otherwise.

use std::cell::Cell;

/// Field order of an all-numeric date such as `3/4/2026`. A leading four-digit field is always the
/// year (`2026-03-04`), and month names are unambiguous.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DateOrder {
    /// `m/d/y` (en-US): `3/4/2026` is 4 March.
    #[default]
    MonthDayYear,
    /// `d/m/y` (en-GB and most of the world): `3/4/2026` is 3 April.
    DayMonthYear,
    /// `y-m-d` (sv-SE): `26-03-04` is 4 March 2026.
    YearMonthDay,
}

/// One locale's settings. See the module docs.
#[derive(Debug, PartialEq, Eq)]
pub struct Locale {
    /// BCP 47 tag, `de-DE`.
    pub tag: &'static str,
    pub date_order: DateOrder,
    /// Separates the parts of a date besides `/` and `-`, which every locale reads: `.` in
    /// German (`16.02.24`), else `/`.
    pub date_separator: char,
    /// Decimal separator: `,` in `1,5`.
    pub decimal: char,
    /// Thousands separator: `.` in `1.234,5`.
    pub group: char,
    /// Currency symbol read before or after a number (`$5`, `23€`, `€100`).
    pub currency: &'static str,
    /// How a positive amount is written in the currency format (`DOLLAR`): `¤` stands for
    /// [`Self::currency`] and `#` for the digits, `¤#` is `$1.00` and `# ¤` is `1,00 €`.
    pub currency_positive: &'static str,
    /// How a negative amount is written, as [`Self::currency_positive`]: `(¤#)` is `($1.00)` and
    /// `-¤#` is `-£1.00`.
    pub currency_negative: &'static str,
    /// Full month names, January first.
    pub months: [&'static str; 12],
    /// Month names for `mmm`.
    pub months_short: [&'static str; 12],
    /// Full day names, Sunday first.
    pub days: [&'static str; 7],
    /// Day names for `ddd`, Sunday first.
    pub days_short: [&'static str; 7],
    /// The letter a `TEXT` format code writes the year with (`y`, Swedish `Å`), either case.
    pub year_code: char,
}

impl Locale {
    /// Whether this is the en-US default.
    pub fn is_default(&self) -> bool {
        self.tag == EN_US.tag
    }

    /// CELL's `info_type` spellings in this locale as `(spelling, English name)` pairs; empty where
    /// the English names are used. Excel reads the text in the language of its interface: a
    /// French Excel caches `$AV$3` for `CELL("adresse",AV3)` (poi StringContinueRecords.xls), where
    /// an English one gives `#VALUE!` (Excel 16.114).
    pub fn cell_info_names(&self) -> &'static [(&'static str, &'static str)] {
        match self.tag {
            "fr-FR" => &FR_CELL_INFO,
            _ => &[],
        }
    }
}

/// The `info_type` names of CELL that Microsoft's French page ("CELLULE", support.microsoft.com
/// fr-fr) lists, with the English ones they stand for.
const FR_CELL_INFO: [(&str, &str); 12] = [
    ("adresse", "address"),
    ("col", "col"),
    ("couleur", "color"),
    ("contenu", "contents"),
    ("nomfichier", "filename"),
    ("format", "format"),
    ("parentheses", "parentheses"),
    ("prefixe", "prefix"),
    ("protege", "protect"),
    ("ligne", "row"),
    ("type", "type"),
    ("largeur", "width"),
];

const ENGLISH_MONTHS: [&str; 12] =
    ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
const ENGLISH_MONTHS_SHORT: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const ENGLISH_DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
const ENGLISH_DAYS_SHORT: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/// United States English: GridCraft's default.
pub const EN_US: Locale = Locale {
    tag: "en-US",
    date_order: DateOrder::MonthDayYear,
    date_separator: '/',
    decimal: '.',
    group: ',',
    currency: "$",
    currency_positive: "¤#",
    currency_negative: "(¤#)",
    months: ENGLISH_MONTHS,
    months_short: ENGLISH_MONTHS_SHORT,
    days: ENGLISH_DAYS,
    days_short: ENGLISH_DAYS_SHORT,
    year_code: 'y',
};

pub const EN_GB: Locale = Locale { tag: "en-GB", date_order: DateOrder::DayMonthYear, currency: "£", currency_negative: "-¤#", ..EN_US };

/// Irish English: en-GB's dates and separators with the euro.
pub const EN_IE: Locale = Locale { tag: "en-IE", currency: "€", ..EN_GB };

pub const DE_DE: Locale = Locale {
    tag: "de-DE",
    date_order: DateOrder::DayMonthYear,
    date_separator: '.',
    decimal: ',',
    group: '.',
    currency: "€",
    currency_positive: "# ¤",
    currency_negative: "-# ¤",
    months: ["Januar", "Februar", "März", "April", "Mai", "Juni", "Juli", "August", "September", "Oktober", "November", "Dezember"],
    months_short: ["Jan", "Feb", "Mrz", "Apr", "Mai", "Jun", "Jul", "Aug", "Sep", "Okt", "Nov", "Dez"],
    days: ["Sonntag", "Montag", "Dienstag", "Mittwoch", "Donnerstag", "Freitag", "Samstag"],
    days_short: ["So", "Mo", "Di", "Mi", "Do", "Fr", "Sa"],
    year_code: 'y',
};

pub const FR_FR: Locale = Locale {
    tag: "fr-FR",
    date_order: DateOrder::DayMonthYear,
    date_separator: '/',
    decimal: ',',
    group: '\u{a0}',
    currency: "€",
    currency_positive: "# ¤",
    currency_negative: "-# ¤",
    months: ["janvier", "février", "mars", "avril", "mai", "juin", "juillet", "août", "septembre", "octobre", "novembre", "décembre"],
    months_short: ["janv.", "févr.", "mars", "avr.", "mai", "juin", "juil.", "août", "sept.", "oct.", "nov.", "déc."],
    days: ["dimanche", "lundi", "mardi", "mercredi", "jeudi", "vendredi", "samedi"],
    days_short: ["dim.", "lun.", "mar.", "mer.", "jeu.", "ven.", "sam."],
    year_code: 'y',
};

pub const ES_ES: Locale = Locale {
    tag: "es-ES",
    date_order: DateOrder::DayMonthYear,
    date_separator: '/',
    decimal: ',',
    group: '.',
    currency: "€",
    currency_positive: "# ¤",
    currency_negative: "-# ¤",
    months: ["enero", "febrero", "marzo", "abril", "mayo", "junio", "julio", "agosto", "septiembre", "octubre", "noviembre", "diciembre"],
    months_short: ["ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "sep", "oct", "nov", "dic"],
    days: ["domingo", "lunes", "martes", "miércoles", "jueves", "viernes", "sábado"],
    days_short: ["dom", "lun", "mar", "mié", "jue", "vie", "sáb"],
    year_code: 'y',
};

pub const IT_IT: Locale = Locale {
    tag: "it-IT",
    date_order: DateOrder::DayMonthYear,
    date_separator: '/',
    decimal: ',',
    group: '.',
    currency: "€",
    currency_positive: "¤ #",
    currency_negative: "-¤ #",
    months: ["gennaio", "febbraio", "marzo", "aprile", "maggio", "giugno", "luglio", "agosto", "settembre", "ottobre", "novembre", "dicembre"],
    months_short: ["gen", "feb", "mar", "apr", "mag", "giu", "lug", "ago", "set", "ott", "nov", "dic"],
    days: ["domenica", "lunedì", "martedì", "mercoledì", "giovedì", "venerdì", "sabato"],
    days_short: ["dom", "lun", "mar", "mer", "gio", "ven", "sab"],
    year_code: 'y',
};

pub const PT_BR: Locale = Locale {
    tag: "pt-BR",
    date_order: DateOrder::DayMonthYear,
    date_separator: '/',
    decimal: ',',
    group: '.',
    currency: "R$",
    currency_positive: "¤ #",
    currency_negative: "-¤ #",
    months: ["janeiro", "fevereiro", "março", "abril", "maio", "junho", "julho", "agosto", "setembro", "outubro", "novembro", "dezembro"],
    months_short: ["jan", "fev", "mar", "abr", "mai", "jun", "jul", "ago", "set", "out", "nov", "dez"],
    days: ["domingo", "segunda-feira", "terça-feira", "quarta-feira", "quinta-feira", "sexta-feira", "sábado"],
    days_short: ["dom", "seg", "ter", "qua", "qui", "sex", "sáb"],
    year_code: 'y',
};

pub const NL_NL: Locale = Locale {
    tag: "nl-NL",
    date_order: DateOrder::DayMonthYear,
    date_separator: '-',
    decimal: ',',
    group: '.',
    currency: "€",
    currency_positive: "¤ #",
    currency_negative: "¤ -#",
    months: ["januari", "februari", "maart", "april", "mei", "juni", "juli", "augustus", "september", "oktober", "november", "december"],
    months_short: ["jan", "feb", "mrt", "apr", "mei", "jun", "jul", "aug", "sep", "okt", "nov", "dec"],
    days: ["zondag", "maandag", "dinsdag", "woensdag", "donderdag", "vrijdag", "zaterdag"],
    days_short: ["zo", "ma", "di", "wo", "do", "vr", "za"],
    year_code: 'y',
};

pub const SV_SE: Locale = Locale {
    tag: "sv-SE",
    date_order: DateOrder::YearMonthDay,
    date_separator: '-',
    decimal: ',',
    group: '\u{a0}',
    currency: "kr",
    currency_positive: "# ¤",
    currency_negative: "-# ¤",
    months: ["januari", "februari", "mars", "april", "maj", "juni", "juli", "augusti", "september", "oktober", "november", "december"],
    months_short: ["jan", "feb", "mar", "apr", "maj", "jun", "jul", "aug", "sep", "okt", "nov", "dec"],
    days: ["söndag", "måndag", "tisdag", "onsdag", "torsdag", "fredag", "lördag"],
    days_short: ["sön", "mån", "tis", "ons", "tors", "fre", "lör"],
    year_code: 'Å',
};

pub const TR_TR: Locale = Locale {
    tag: "tr-TR",
    date_order: DateOrder::DayMonthYear,
    date_separator: '.',
    decimal: ',',
    group: '.',
    currency: "₺",
    currency_positive: "¤#",
    currency_negative: "-¤#",
    months: ["Ocak", "Şubat", "Mart", "Nisan", "Mayıs", "Haziran", "Temmuz", "Ağustos", "Eylül", "Ekim", "Kasım", "Aralık"],
    months_short: ["Oca", "Şub", "Mar", "Nis", "May", "Haz", "Tem", "Ağu", "Eyl", "Eki", "Kas", "Ara"],
    days: ["Pazar", "Pazartesi", "Salı", "Çarşamba", "Perşembe", "Cuma", "Cumartesi"],
    days_short: ["Paz", "Pzt", "Sal", "Çar", "Per", "Cum", "Cmt"],
    year_code: 'y',
};

/// The locales other than en-US that a cache may have been computed under, in the order they
/// are tried.
pub const CANDIDATES: [&Locale; 10] = [&EN_GB, &EN_IE, &DE_DE, &FR_FR, &ES_ES, &IT_IT, &PT_BR, &NL_NL, &SV_SE, &TR_TR];

thread_local! {
    static CURRENT: Cell<&'static Locale> = const { Cell::new(&EN_US) };
    static READS: Cell<u64> = const { Cell::new(0) };
}

/// The locale in effect on this thread: [`EN_US`] outside [`scoped`].
pub fn current() -> &'static Locale {
    let _ = READS.try_with(|n| n.set(n.get().wrapping_add(1)));
    CURRENT.try_with(Cell::get).unwrap_or(&EN_US)
}

/// How many times [`current`] has been asked on this thread. What ran between two equal readings
/// never looked at the locale, so it gave the result it gives under any other locale: the
/// calculation uses this to tell the formulas whose value can depend on the locale from those
/// that can't.
pub fn reads() -> u64 {
    READS.try_with(Cell::get).unwrap_or(0)
}

/// Runs `f` with `locale` in effect on this thread, then restores the previous one (also when `f`
/// unwinds).
pub fn scoped<R>(locale: &'static Locale, f: impl FnOnce() -> R) -> R {
    struct Restore(&'static Locale);
    impl Drop for Restore {
        fn drop(&mut self) {
            let _ = CURRENT.try_with(|c| c.set(self.0));
        }
    }
    let _restore = Restore(CURRENT.try_with(|c| c.replace(locale)).unwrap_or(&EN_US));
    f()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_en_us_and_scopes_nest_and_restore() {
        assert_eq!(current().tag, "en-US");
        assert!(current().is_default());
        let inner = scoped(&DE_DE, || {
            assert_eq!(current().tag, "de-DE");
            scoped(&SV_SE, || current().tag)
        });
        assert_eq!(inner, "sv-SE");
        assert_eq!(current().tag, "en-US");
        let unwound = std::panic::catch_unwind(|| scoped(&FR_FR, || std::panic::panic_any("unwind")));
        assert!(unwound.is_err());
        assert_eq!(current().tag, "en-US");
    }

    #[test]
    fn candidates_are_distinct_and_not_the_default() {
        for (i, a) in CANDIDATES.iter().enumerate() {
            assert!(!a.is_default(), "{}", a.tag);
            assert!(CANDIDATES.iter().skip(i + 1).all(|b| b.tag != a.tag), "{}", a.tag);
            assert!(a.decimal != a.group, "{}", a.tag);
        }
    }
}
