//! OpenFormula ([ODF 1.3 part 4]) to Excel formula text, for the ODS import.
//!
//! The two syntaxes share operators, literals and most function names. They differ in what this
//! module rewrites: references are bracketed and sheet-qualified with a dot (`[.A1]`,
//! `[$Sheet1.$A$1:.$B$2]`, `['My sheet'.A:.A]`), arguments are separated by `;`, inline arrays
//! use `;` between columns and `|` between rows, the reference union is `~` and the intersection
//! `!`, and functions newer than the standard carry a vendor prefix (`COM.MICROSOFT.XLOOKUP`,
//! `LEGACY.CHIDIST`). Anything else (references into other files, application-specific
//! functions, functions whose OpenFormula meaning differs from Excel's of the same name) is
//! refused, and the import keeps the cell's saved result.
//!
//! [ODF 1.3 part 4]: https://docs.oasis-open.org/office/OpenDocument/v1.3/os/part4-formula/OpenDocument-v1.3-os-part4-formula.html

use gridcraft_core::{CellRef, MAX_COLS, MAX_ROWS, RangeRef, col_to_letters as col_name};

/// Why a formula can't be imported as a formula.
#[derive(Debug, PartialEq)]
pub(crate) struct Unsupported(pub &'static str);

/// Formula text read at most (Excel's limit is 8,192 characters).
const MAX_LEN: usize = 64 * 1024;
/// Parenthesis nesting at most, as in the formula parser.
const MAX_DEPTH: usize = 256;

/// Where references are seen from. A cell formula's references are its own coordinates. A named
/// expression's relative parts are relative to its base cell (ODF 1.3 part 3, 9.4.13), and the
/// model keeps a name's relative parts as seen from A1, so they are moved by the base cell.
#[derive(Clone, Copy, Default)]
pub(crate) struct Base {
    pub row: u32,
    pub col: u32,
}

/// The document a formula is translated in: the sheet names references use, mapped to the
/// imported sheets' names (a name Excel doesn't allow was repaired on import), and its named
/// expressions.
pub(crate) trait Sheets {
    fn sheet(&self, written: &str) -> String;
    /// Whether any sheet was imported under another name than the document gives it.
    fn repaired(&self) -> bool;
    /// Whether any named expression of the document spells `name` (lower case) in its name or
    /// expression. A defined name is evaluated where it is used, so a LET the translation adds
    /// must not bind a name one of them could read.
    fn names_mention(&self, name: &str) -> bool;
    /// The name Excel holds for the document's named expression `written`, when the document
    /// defines one by that spelling (any case) that Excel can't hold under it (`TAX2020` is a cell
    /// in today's grid, `1st` starts with a digit): the import renamed it. `None` for every other
    /// spelling, including a valid name, which keeps its own.
    fn excel_name(&self, written: &str) -> Option<String>;
    /// The named expression INDIRECT's literal text names, resolved as LibreOffice's INDIRECT does
    /// (`ScRangeStringConverter::GetRangeDataFromString`): `qualifier` is the sheet the text
    /// qualifies the name with (quotes removed, any case), and only that sheet's own names count;
    /// without one, the names of the sheet the formula is on, then the workbook's. `Ok(None)` when
    /// the document defines no such name, so the text reads as cell references; an error when it
    /// does but the import can't follow (the name was left out, or which sheet's name it is can't
    /// be told).
    fn indirect_name(&self, qualifier: Option<&str>, written: &str) -> Result<Option<IndirectName>, Unsupported>;
}

/// A named expression INDIRECT's text names (see [`Sheets::indirect_name`]).
pub(crate) struct IndirectName {
    /// The name as Excel holds it: renamed when Excel can't hold the written one.
    pub name: String,
    /// Whether the import renamed it.
    pub renamed: bool,
    /// The imported name of the sheet the text qualifies it with, if it does.
    pub sheet: Option<String>,
}

/// Translates a `table:formula` value (`of:=…`; `oooc:=…` from OpenOffice.org 2, which writes
/// the same syntax; `msoxl:=…` is Excel syntax already). Returns the formula without `=`.
/// OpenFormula's `=` is optional, and `==` marks a formula recalculated on every change
/// (ODF 1.3 part 4 section 5.2: `Formula ::= Intro? Expression`, `Intro ::= '=' ForceRecalc?`).
pub(crate) fn cell_formula(src: &str, sheets: &dyn Sheets) -> Result<String, Unsupported> {
    let (ns, body) = match src.split_once(':') {
        Some((ns, body)) if !ns.is_empty() && ns.bytes().all(|b| b.is_ascii_alphanumeric()) => (ns, body),
        _ => ("of", src),
    };
    let body = match body.strip_prefix('=') {
        Some(rest) => rest.strip_prefix('=').unwrap_or(rest),
        None if ns == "msoxl" => return Err(Unsupported("malformed formula")),
        None => body,
    };
    match ns {
        "of" | "oooc" => translate(body, sheets, None),
        // Excel's own syntax is imported as written, which can't follow a rename (a name that reads
        // as a cell would read the cell): a formula using a renamed name keeps its saved result.
        "msoxl" if body.split(|c: char| !(c.is_alphanumeric() || matches!(c, '_' | '.' | '\\' | '?'))).any(|t| sheets.excel_name(t).is_some()) => {
            Err(Unsupported("names Excel doesn't allow, written in Excel's syntax"))
        }
        "msoxl" => Ok(body.to_string()),
        _ => Err(Unsupported("formula syntax of another application")),
    }
}

/// Translates an OpenFormula expression (a named expression's `table:expression`, a named
/// range's address in brackets) whose relative parts count from `base`.
pub(crate) fn translate(src: &str, sheets: &dyn Sheets, base: Option<Base>) -> Result<String, Unsupported> {
    translate_with(src, sheets, base, false)
}

/// [`translate`], keeping the source's whitespace when `keep_space`.
fn translate_with(src: &str, sheets: &dyn Sheets, base: Option<Base>, keep_space: bool) -> Result<String, Unsupported> {
    if src.len() > MAX_LEN {
        return Err(Unsupported("formula too long"));
    }
    let toks = lex(src, sheets, base, keep_space)?;
    let mut p = Parser { toks, i: 0, depth: 0, doc: sheets };
    let parts = p.seq(None)?;
    match parts.as_slice() {
        [one] if !one.text.is_empty() => Ok(one.arg()),
        _ => Err(Unsupported("malformed formula")),
    }
}

#[derive(Debug, PartialEq)]
enum Tok {
    /// Text copied as it is: numbers, strings, operators, references and errors already in
    /// Excel's syntax.
    Raw(String),
    /// Whitespace, kept only when translating a formula as written.
    Space(String),
    Ident(String),
    Open,
    Close,
    Sep,
    LBrace,
    RBrace,
    Pipe,
    Union,
    Intersect,
}

const ERRORS: [&str; 8] = ["#NULL!", "#DIV/0!", "#VALUE!", "#REF!", "#NAME?", "#NUM!", "#N/A", "#GETTING_DATA"];

const PRODUCER_ERROR: &str = "formula is an error constant of the producing application";

/// Whether `s` starts with one of LibreOffice's numbered errors, `#ERR502!`.
fn producer_error(s: &str) -> bool {
    let b = s.as_bytes();
    let digits = b.get(4..).map_or(0, |d| d.iter().take_while(|c| c.is_ascii_digit()).count());
    b.get(..4).is_some_and(|p| p.eq_ignore_ascii_case(b"#ERR")) && digits > 0 && b.get(4 + digits) == Some(&b'!')
}

fn lex(src: &str, sheets: &dyn Sheets, base: Option<Base>, keep_space: bool) -> Result<Vec<Tok>, Unsupported> {
    let mut out = Vec::new();
    let mut rest = src;
    while let Some(c) = rest.chars().next() {
        let (tok, len) = match c {
            ' ' | '\t' | '\r' | '\n' | '\u{a0}' => {
                if keep_space {
                    out.push(Tok::Space(c.to_string()));
                }
                rest = &rest[c.len_utf8()..];
                continue;
            }
            '"' => {
                let mut end = 1;
                loop {
                    let close = rest.get(end..).and_then(|s| s.find('"')).ok_or(Unsupported("unterminated string"))?;
                    end += close + 1;
                    if rest.as_bytes().get(end) == Some(&b'"') {
                        end += 1;
                    } else {
                        break;
                    }
                }
                (Tok::Raw(rest[..end].to_string()), end)
            }
            '[' => {
                let end = bracket_end(rest).ok_or(Unsupported("unterminated reference"))?;
                (Tok::Raw(reference(&rest[1..end], sheets, base)?), end + 1)
            }
            '#' => {
                // LibreOffice writes its own errors as `#ERR502!` (OpenFormula's Error syntax);
                // Excel has no such error.
                if producer_error(rest) {
                    return Err(Unsupported(PRODUCER_ERROR));
                }
                // Compared as bytes: the text after a short error (`#N/A+"é"`) may end inside a
                // character where a longer candidate would.
                let e = ERRORS
                    .iter()
                    .find(|e| rest.as_bytes().get(..e.len()).is_some_and(|p| p.eq_ignore_ascii_case(e.as_bytes())))
                    .ok_or(Unsupported("unknown error constant"))?;
                (Tok::Raw((*e).to_string()), e.len())
            }
            '0'..='9' | '.' => {
                let b = rest.as_bytes();
                let mut end = b.iter().take_while(|b| b.is_ascii_digit() || **b == b'.').count();
                if matches!(b.get(end), Some(b'e' | b'E')) {
                    let sign = usize::from(matches!(b.get(end + 1), Some(b'+' | b'-')));
                    let digits = b.get(end + 1 + sign..).map_or(0, |d| d.iter().take_while(|b| b.is_ascii_digit()).count());
                    if digits > 0 {
                        end += 1 + sign + digits;
                    }
                }
                (Tok::Raw(rest[..end].to_string()), end)
            }
            c if c.is_alphabetic() || c == '_' || c == '\\' => {
                let end = rest.find(|c: char| !(c.is_alphanumeric() || matches!(c, '_' | '.' | '\\'))).unwrap_or(rest.len());
                (Tok::Ident(rest[..end].to_string()), end)
            }
            '(' => (Tok::Open, 1),
            ')' => (Tok::Close, 1),
            ';' => (Tok::Sep, 1),
            '{' => (Tok::LBrace, 1),
            '}' => (Tok::RBrace, 1),
            '|' => (Tok::Pipe, 1),
            '~' => (Tok::Union, 1),
            '!' => (Tok::Intersect, 1),
            // `Err:502` (and its translations, `Chyba:504`) is how LibreOffice shows its own errors.
            ':' if matches!(out.last(), Some(Tok::Ident(_))) && rest[1..].starts_with(|c: char| c.is_ascii_digit()) => {
                return Err(Unsupported(PRODUCER_ERROR));
            }
            // The range operator between references (`INDIRECT(…):INDIRECT(…)`), as in Excel.
            ':' => (Tok::Raw(":".into()), 1),
            '+' | '-' | '*' | '/' | '^' | '&' | '=' | '<' | '>' | '%' => (Tok::Raw(c.to_string()), 1),
            _ => return Err(Unsupported("unknown formula character")),
        };
        out.push(tok);
        rest = &rest[len..];
    }
    Ok(out)
}

/// The index of the `]` closing the reference `s` starts with, skipping quoted sheet names.
fn bracket_end(s: &str) -> Option<usize> {
    let mut quoted = false;
    for (i, c) in s.char_indices().skip(1) {
        match c {
            '\'' => quoted = !quoted,
            ']' if !quoted => return Some(i),
            _ => {}
        }
    }
    None
}

/// One end of a reference: a cell, a whole column or a whole row, with `$` marks.
#[derive(Clone, Copy)]
enum End {
    Cell { row: u32, row_abs: bool, col: u32, col_abs: bool },
    Col { col: u32, abs: bool },
    Row { row: u32, abs: bool },
}

impl End {
    fn moved(self, base: Option<Base>) -> End {
        let Some(b) = base else { return self };
        let back = |v: u32, by: u32, size: u32| (v + size - by % size) % size;
        match self {
            End::Cell { row, row_abs, col, col_abs } => End::Cell {
                row: if row_abs { row } else { back(row, b.row, MAX_ROWS) },
                row_abs,
                col: if col_abs { col } else { back(col, b.col, MAX_COLS) },
                col_abs,
            },
            End::Col { col, abs } => End::Col { col: if abs { col } else { back(col, b.col, MAX_COLS) }, abs },
            End::Row { row, abs } => End::Row { row: if abs { row } else { back(row, b.row, MAX_ROWS) }, abs },
        }
    }

    fn text(self) -> String {
        let d = |abs: bool| if abs { "$" } else { "" };
        match self {
            End::Cell { row, row_abs, col, col_abs } => format!("{}{}{}{}", d(col_abs), col_name(col), d(row_abs), row + 1),
            End::Col { col, abs } => format!("{}{}", d(abs), col_name(col)),
            End::Row { row, abs } => format!("{}{}", d(abs), row + 1),
        }
    }
}

/// A sheet name as written: `'quoted ''name'''` or a bare name, after an optional `$`.
fn sheet_name(s: &str) -> Result<(Option<String>, &str), Unsupported> {
    let s = s.strip_prefix('$').unwrap_or(s);
    if let Some(q) = s.strip_prefix('\'') {
        let mut name = String::new();
        let mut rest = q;
        loop {
            let at = rest.find('\'').ok_or(Unsupported("unterminated sheet name"))?;
            name.push_str(&rest[..at]);
            rest = &rest[at + 1..];
            if let Some(r) = rest.strip_prefix('\'') {
                name.push('\'');
                rest = r;
            } else {
                break;
            }
        }
        return Ok((Some(name), rest));
    }
    match s.find('.') {
        Some(0) => Ok((None, s)),
        Some(at) => Ok((Some(s[..at].to_string()), &s[at..])),
        None => Err(Unsupported("reference without a sheet separator")),
    }
}

/// `$A$1`, `A`, `$12`.
fn end(s: &str) -> Result<End, Unsupported> {
    const BAD: Unsupported = Unsupported("invalid cell address");
    let (col_abs, s) = s.strip_prefix('$').map_or((false, s), |r| (true, r));
    let letters = s.bytes().take_while(u8::is_ascii_alphabetic).count();
    let (letters_s, rest) = s.split_at(letters);
    let (row_abs, digits) = rest.strip_prefix('$').map_or((false, rest), |r| (true, r));
    let col = if letters == 0 {
        None
    } else {
        let n = letters_s.bytes().try_fold(0u32, |n, b| n.checked_mul(26)?.checked_add(u32::from(b.to_ascii_uppercase() - b'A' + 1))).ok_or(BAD)?;
        Some(n.checked_sub(1).filter(|c| *c < MAX_COLS).ok_or(BAD)?)
    };
    let row = if digits.is_empty() {
        None
    } else {
        if !digits.bytes().all(|b| b.is_ascii_digit()) {
            return Err(BAD);
        }
        let n: u32 = digits.parse().map_err(|_| BAD)?;
        Some(n.checked_sub(1).filter(|r| *r < MAX_ROWS).ok_or(BAD)?)
    };
    match (col, row) {
        (Some(col), Some(row)) => Ok(End::Cell { row, row_abs, col, col_abs }),
        (Some(col), None) if !row_abs => Ok(End::Col { col, abs: col_abs }),
        // Without a column, a leading `$` marks the row.
        (None, Some(row)) if !(col_abs && row_abs) => Ok(End::Row { row, abs: col_abs || row_abs }),
        _ => Err(BAD),
    }
}

/// Whether a reference's inside is broken: `#REF!` as the reference (ODF 1.3 part 4, 5.8:
/// `ReferenceError`) or in place of a sheet or address (`[.F1:.#REF!]`, `[$#REF!.A1]`), never
/// text inside a quoted sheet name (`['Budget#REF!'.A1]`). A doubled quote inside a quoted name
/// leaves an empty unquoted piece.
fn broken(s: &str) -> bool {
    s.split('\'').step_by(2).any(|unquoted| unquoted.contains("#REF!"))
}

/// The inside of a bracketed reference to Excel's syntax (ODF 1.3 part 4, 5.8).
fn reference(s: &str, sheets: &dyn Sheets, base: Option<Base>) -> Result<String, Unsupported> {
    if broken(s) {
        return Ok("#REF!".into());
    }
    let (first_sheet, rest) = sheet_name(s)?;
    if rest.starts_with('#') || first_sheet.as_deref().is_some_and(|n| n.contains("://")) {
        return Err(Unsupported("references to other files"));
    }
    let rest = rest.strip_prefix('.').ok_or(Unsupported("reference without a sheet separator"))?;
    let (a, second) = match rest.split_once(':') {
        Some((a, b)) => (a, Some(b)),
        None => (rest, None),
    };
    let a = end(a)?.moved(base);
    let (last_sheet, b) = match second {
        Some(b) => {
            let (sheet, b) = sheet_name(b)?;
            let b = b.strip_prefix('.').ok_or(Unsupported("reference without a sheet separator"))?;
            (sheet, Some(end(b)?.moved(base)))
        }
        None => (None, None),
    };
    let address = match (a, b) {
        (End::Cell { .. }, None) => a.text(),
        (End::Cell { .. }, Some(b @ End::Cell { .. }))
        | (End::Col { .. }, Some(b @ End::Col { .. }))
        | (End::Row { .. }, Some(b @ End::Row { .. })) => {
            format!("{}:{}", a.text(), b.text())
        }
        _ => return Err(Unsupported("invalid range address")),
    };
    let quote = |n: &str| n.replace('\'', "''");
    Ok(match (first_sheet, last_sheet) {
        (None, None) => address,
        (Some(f), None) => format!("'{}'!{address}", quote(&sheets.sheet(&f))),
        (Some(f), Some(l)) if f == l => format!("'{}'!{address}", quote(&sheets.sheet(&f))),
        (Some(f), Some(l)) => format!("'{}:{}'!{address}", quote(&sheets.sheet(&f)), quote(&sheets.sheet(&l))),
        (None, Some(_)) => return Err(Unsupported("3-D reference without a first sheet")),
    })
}

/// A named expression's `table:base-cell-address` (`$Sheet1.$B$2`).
pub(crate) fn base_cell(s: &str) -> Option<Base> {
    let (_, rest) = sheet_name(s).ok()?;
    match end(rest.strip_prefix('.')?).ok()? {
        End::Cell { row, col, .. } => Some(Base { row, col }),
        _ => None,
    }
}

/// A cell range address on one sheet as an attribute writes it (`Sheet2.M1:Sheet2.O7`,
/// `'My sheet'.$A$1:.$C$9`): the sheet's name as written and the range.
pub(crate) fn range_address(s: &str) -> Option<(String, RangeRef)> {
    let (sheet, rest) = sheet_name(s).ok()?;
    let (a, b) = rest.strip_prefix('.')?.split_once(':')?;
    let (second, b) = sheet_name(b).ok()?;
    if second.is_some_and(|n| Some(&n) != sheet.as_ref()) {
        return None;
    }
    match (end(a).ok()?, end(b.strip_prefix('.')?).ok()?) {
        (End::Cell { row: r0, col: c0, .. }, End::Cell { row: r1, col: c1, .. }) => {
            Some((sheet?, RangeRef::new(CellRef::new(r0.min(r1), c0.min(c1)), CellRef::new(r0.max(r1), c0.max(c1)))))
        }
        _ => None,
    }
}

/// An Excel call for an OpenFormula one (ODF 1.3 part 4, 6), or why there is none: the name
/// without its vendor prefix, and the arguments of functions whose OpenFormula signature differs
/// from Excel's function of that name rearranged to the Excel function with the same meaning.
fn call(name: &str, mut args: Vec<String>, doc: &dyn Sheets) -> Result<String, Unsupported> {
    let upper = name.to_ascii_uppercase();
    let n = args.len();
    // Functions Excel added after OpenFormula 1.2, and the Excel spellings OpenFormula keeps for
    // compatibility (LEGACY.FDIST is Excel's right-tailed FDIST).
    let excel =
        if let Some(n) = upper.strip_prefix("COM.MICROSOFT.").or_else(|| upper.strip_prefix("LEGACY.")).or_else(|| upper.strip_prefix("_XLFN.")) {
            n.to_string()
        } else {
            let cumulative = |args: &mut Vec<String>, at: usize| {
                if args.len() == at {
                    args.push("TRUE".into());
                }
            };
            match upper.as_str() {
                "FORMULA" => "FORMULATEXT".into(),
                // Digits default to 0; Excel requires them.
                "ROUND" | "ROUNDUP" | "ROUNDDOWN" if n == 1 => {
                    args.push("0".into());
                    upper
                }
                // FLOOR/CEILING(N; [Significance]; [Mode]): towards -∞/+∞ to a multiple of
                // Significance (default ±1), or with a non-zero Mode the magnitude towards/away from
                // zero: Excel's FLOOR.MATH/CEILING.MATH, which take the significance's magnitude.
                // Excel's own FLOOR/CEILING are written COM.MICROSOFT.FLOOR/CEILING.
                "FLOOR" | "CEILING" => {
                    if args.get(1).is_some_and(String::is_empty) {
                        args[1] = "1".into();
                    }
                    format!("{upper}.MATH")
                }
                // MROUND(A; B): the multiple of B nearest A, the greater one when two are as near,
                // whatever the signs (ODF 1.3 part 4, 6.17.4). Excel's MROUND rounds a tie away from
                // zero and is #NUM! for opposite signs, so this is A + |B|/2 floored to a multiple
                // of |B| (0 when B is 0, as FLOOR.MATH and Excel's MROUND give). LET evaluates B
                // once, under a name A doesn't mention (A is read in the LET's scope), nor any
                // named expression A could read through (a name is evaluated where it is used).
                "MROUND" if n == 2 => {
                    let a = args[0].to_ascii_lowercase();
                    let b = (0..)
                        .map(|i| format!("mround_b{}", "_".repeat(i)))
                        .find(|b| !a.contains(b.as_str()) && !doc.names_mention(b))
                        .unwrap_or_default();
                    return Ok(format!("LET(_xlpm.{b},ABS({}),FLOOR.MATH(({})+_xlpm.{b}/2,_xlpm.{b}))", args[1], args[0]));
                }
                // INDIRECT(Ref [; A1 = TRUE]): OpenFormula reference text names its sheet with a
                // dot (`Sheet2.A1`) or with Excel's `!` (ODF 1.3 part 4, 6.14.7); Excel reads only `!`.
                "INDIRECT" if n <= 2 => {
                    let a1 = match args.get(1).map(|a| a.trim().to_ascii_uppercase()) {
                        None => Some(true),
                        // An empty A1 is its default.
                        Some(a) if a.is_empty() => {
                            args[1] = "TRUE".into();
                            Some(true)
                        }
                        Some(a) if a == "TRUE" || a == "1" => Some(true),
                        Some(a) if a == "FALSE" || a == "0" => Some(false),
                        Some(_) => None,
                    };
                    if let Some(r) = args.first_mut() {
                        *r = indirect_text(r, a1, doc)?;
                    }
                    upper
                }
                // BETADIST(x; α; β [; a = 0 [; b = 1 [; Cumulative = TRUE]]]).
                "BETADIST" if n == 6 => {
                    let cum = args.remove(5);
                    args.insert(3, cum);
                    "BETA.DIST".into()
                }
                // LOGNORMDIST(X [; μ = 0 [; σ = 1 [; Cumulative = TRUE]]]).
                "LOGNORMDIST" if n == 4 => "LOGNORM.DIST".into(),
                "LOGNORMDIST" => {
                    for default in ["0", "1"].iter().skip(n.saturating_sub(1)) {
                        args.push((*default).into());
                    }
                    upper
                }
                // HYPGEOMDIST(X; T; M; N [; Cumulative = FALSE]).
                "HYPGEOMDIST" if n == 5 => "HYPGEOM.DIST".into(),
                // Cumulative defaults to TRUE; Excel requires it.
                "POISSON" | "EXPONDIST" => {
                    cumulative(&mut args, 2);
                    upper
                }
                "NORMDIST" | "GAMMADIST" => {
                    cumulative(&mut args, 3);
                    upper
                }
                // Left-tailed with an optional Cumulative; Excel's FDIST/FINV are right-tailed
                // (LEGACY.FDIST/LEGACY.FINV in OpenFormula).
                "FDIST" => {
                    cumulative(&mut args, 3);
                    "F.DIST".into()
                }
                "FINV" => "F.INV".into(),
                "CHISQDIST" => {
                    cumulative(&mut args, 2);
                    "CHISQ.DIST".into()
                }
                "CHISQINV" => "CHISQ.INV".into(),
                "MULTIPLE.OPERATIONS" => return Err(Unsupported("what-if tables (MULTIPLE.OPERATIONS)")),
                // Vendor prefixes are reverse domain names; dotted names without one (`ERROR.TYPE`,
                // `BINOM.DIST.RANGE`) are standard functions.
                n if n.starts_with("ORG.") || n.starts_with("COM.") || n.starts_with("NET.") => {
                    return Err(Unsupported("functions of the producing application"));
                }
                _ => upper,
            }
        };
    Ok(format!("{excel}({})", args.join(",")))
}

const COMPUTED_INDIRECT: Unsupported = Unsupported("INDIRECT reference text the formula computes, which may name its sheet the OpenFormula way");
const REPAIRED_INDIRECT: Unsupported = Unsupported("INDIRECT reference text naming a sheet whose name was repaired on import");
const RENAMED_INDIRECT: Unsupported =
    Unsupported("INDIRECT text naming a name Excel doesn't allow, read in R1C1 style or a style the formula computes");

/// INDIRECT's reference argument `arg` (Excel syntax) for Excel's INDIRECT, `a1` its style when
/// the formula says. Literal text with OpenFormula's dot between sheet and cell (`"Sheet2.A1"`,
/// `"'My sheet'.A1:B2"`) becomes Excel's (`"'Sheet2'!A1"`), and a sheet named either way is named
/// as imported (a name Excel doesn't allow was repaired); text without a sheet (`"B3"`, a name)
/// means the same to both. Text the formula computes could be either, except ADDRESS's: each
/// evaluator's ADDRESS writes what its INDIRECT reads (part 4, 6.14.2), for the sheet it is
/// given as imported.
fn indirect_text(arg: &str, a1: Option<bool>, doc: &dyn Sheets) -> Result<String, Unsupported> {
    let trimmed = arg.trim();
    let Some(text) = string_literal(trimmed) else {
        return match call_args(trimmed, "ADDRESS") {
            Some(args) if args.get(4).is_none_or(|sheet| address_sheet_kept(sheet, doc)) => Ok(arg.to_string()),
            Some(_) => Err(REPAIRED_INDIRECT),
            None => Err(COMPUTED_INDIRECT),
        };
    };
    // A name first, as LibreOffice's INDIRECT tries one before any cell (tdf#151046), resolved the
    // way it does (see [`indirect_name_text`]): a name only another sheet has leaves
    // `INDIRECT("Q1")` reading the cell. A name the import renamed is named so in the text
    // (`INDIRECT("TAX2020")` reads the name the document defines, which Excel holds as
    // `_TAX2020`); in R1C1 style, or a style the formula computes, the text can't be rewritten.
    let styles: &[bool] = match a1 {
        Some(true) => &[true],
        Some(false) => &[false],
        None => &[true, false],
    };
    for &style in styles {
        let Some(named) = indirect_name_text(&text, style, doc)? else { continue };
        if a1 != Some(true) && named.renamed {
            return Err(RENAMED_INDIRECT);
        }
        if a1 == Some(true) {
            return Ok(named.excel.map_or_else(|| arg.to_string(), |excel| text_literal(&excel)));
        }
    }
    if let Some((sheet, rest)) = excel_sheet(&text) {
        let kept = doc.sheet(&sheet);
        return Ok(if kept == sheet { arg.to_string() } else { text_literal(&format!("'{}'!{rest}", kept.replace('\'', "''"))) });
    }
    if find_unquoted(&text, '.').is_none() {
        return Ok(arg.to_string());
    }
    if a1 != Some(true) {
        return Err(Unsupported("INDIRECT reference text with an OpenFormula sheet separator, read in R1C1 style or a style the formula computes"));
    }
    // The second end may leave out its sheet (`Sheet2.A1:B2`), as a bracketed reference doesn't.
    let text = match find_unquoted(&text, ':') {
        Some(at) if find_unquoted(&text[at + 1..], '.').is_none() => format!("{}:.{}", &text[..at], &text[at + 1..]),
        _ => text,
    };
    let excel = reference(&text, doc, None).map_err(|_| Unsupported("INDIRECT reference text that isn't a reference GridCraft reads"))?;
    Ok(text_literal(&excel))
}

/// The Excel-syntax string literal of `text`.
fn text_literal(text: &str) -> String {
    format!("\"{}\"", text.replace('"', "\"\""))
}

/// Whether ADDRESS's sheet argument `arg` (Excel syntax) names its sheet as imported: a literal
/// no repair renamed, or text the formula computes when no sheet was repaired.
fn address_sheet_kept(arg: &str, doc: &dyn Sheets) -> bool {
    let arg = arg.trim();
    arg.is_empty() || string_literal(arg).map_or(!doc.repaired(), |name| doc.sheet(&name) == name)
}

/// The sheet Excel's `!` names in reference text (unquoted), and the text after the `!`.
fn excel_sheet(text: &str) -> Option<(String, &str)> {
    let at = find_unquoted(text, '!')?;
    Some((unquote_head(&text[..at]), &text[at + 1..]))
}

/// The sheet name a reference text's head (before its `.` or `!`) writes: its quotes taken off.
fn unquote_head(head: &str) -> String {
    head.strip_prefix('\'').and_then(|h| h.strip_suffix('\'')).map_or_else(|| head.to_string(), |h| h.replace("''", "'"))
}

/// A named expression INDIRECT's literal `text` names, and how Excel's INDIRECT must be given it.
struct NamedText {
    /// Whether the import renamed the name: R1C1-style text naming it can't be followed.
    renamed: bool,
    /// The text Excel's INDIRECT reads for the name (`'Sheet2'!_Q1`), `None` when `text` is that.
    excel: Option<String>,
}

/// The named expression INDIRECT's literal `text` names, found as LibreOffice's INDIRECT finds one
/// before it tries any cell (`ScRangeStringConverter::GetRangeDataFromString`, tdf#151046): text
/// with a sheet before its first unquoted `.` (OpenFormula) or, failing that, `!` (Excel; the `.`
/// is not looked for when the text is read in R1C1 style, `a1` false) names that sheet's own name
/// of the part after it, exactly as written (a `$` or padding is no part of a name); text without
/// one names the formula's own sheet's name, then the workbook's. A text that names no name of
/// the document reads as cell references.
fn indirect_name_text(text: &str, a1: bool, doc: &dyn Sheets) -> Result<Option<NamedText>, Unsupported> {
    let at = if a1 { find_unquoted(text, '.') } else { None }.or_else(|| find_unquoted(text, '!'));
    let (qualifier, written) = match at {
        // Nothing before the separator: not a name.
        Some(0) => return Ok(None),
        Some(at) => (Some(unquote_head(&text[..at])), &text[at + 1..]),
        None => (None, text),
    };
    let Some(name) = doc.indirect_name(qualifier.as_deref(), written)? else {
        // LibreOffice finds no name in padded text (`" Q1"`, and no cell: `#REF!`), where Excel's
        // INDIRECT trims it: the saved result stays when the trimmed text would name one.
        let trimmed = written.trim();
        if trimmed.len() != written.len() && doc.indirect_name(qualifier.as_deref(), trimmed)?.is_some() {
            return Err(Unsupported("INDIRECT text naming a name with spaces around it"));
        }
        return Ok(None);
    };
    let dotted = at.is_some_and(|at| text[at..].starts_with('.'));
    let spelled = match &name.sheet {
        Some(sheet) => format!("'{}'!{}", sheet.replace('\'', "''"), name.name),
        None => name.name.clone(),
    };
    let changed = name.renamed || dotted || name.sheet.as_deref() != qualifier.as_deref();
    Ok(Some(NamedText { renamed: name.renamed, excel: changed.then_some(spelled) }))
}

/// The text of the string literal `s` is, if it is one whole literal.
fn string_literal(s: &str) -> Option<String> {
    let inner = s.strip_prefix('"')?.strip_suffix('"')?;
    let mut text = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '"' && chars.next() != Some('"') {
            return None;
        }
        text.push(c);
    }
    Some(text)
}

/// Where `c` first is in `s` outside a quoted sheet name.
fn find_unquoted(s: &str, c: char) -> Option<usize> {
    let mut quoted = false;
    s.char_indices()
        .find(|&(_, x)| {
            if x == '\'' {
                quoted = !quoted;
            }
            !quoted && x == c
        })
        .map(|(i, _)| i)
}

/// The arguments of the Excel-syntax `s` if it is one call of `name`, its parentheses closing at
/// the end.
fn call_args<'a>(s: &'a str, name: &str) -> Option<Vec<&'a str>> {
    let rest = s.get(..name.len()).filter(|p| p.eq_ignore_ascii_case(name)).and_then(|_| s[name.len()..].strip_prefix('('))?;
    let (mut depth, mut text, mut sheet, mut start) = (1usize, false, false, 0usize);
    let mut args = vec![];
    for (i, c) in rest.char_indices() {
        match c {
            '"' if !sheet => text = !text,
            '\'' if !text => sheet = !sheet,
            '(' | '{' if !text && !sheet => depth += 1,
            ')' | '}' if !text && !sheet => {
                depth -= 1;
                if depth == 0 {
                    args.push(&rest[start..i]);
                    return (i + 1 == rest.len()).then_some(args);
                }
            }
            ',' if depth == 1 && !text && !sheet => {
                args.push(&rest[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    None
}

/// How the document's functions and comparisons match text: the host properties of
/// `table:calculation-settings` (ODF 1.3 part 3, 9.4.1 and table 16 in 19.646; part 4, 3.4),
/// with the defaults that table gives when the document says nothing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Matching {
    /// `table:use-regular-expressions` (HOST-USE-REGULAR-EXPRESSIONS), default true.
    pub regex: bool,
    /// `table:use-wildcards` (HOST-USE-WILDCARDS), default false.
    pub wildcards: bool,
    /// `table:search-criteria-must-apply-to-whole-cell`, default true.
    pub whole_cell: bool,
    /// `table:case-sensitive` (HOST-CASE-SENSITIVE), default true: text comparisons (`=`, `<>`,
    /// `<`, `<=`, `>`, `>=`) and the criteria and lookups that use them tell case apart, where
    /// Excel's never do.
    pub case_sensitive: bool,
}

impl Default for Matching {
    fn default() -> Self {
        Matching { regex: true, wildcards: false, whole_cell: true, case_sensitive: true }
    }
}

/// What a function's argument is matched as.
#[derive(Clone, Copy)]
enum Matched {
    /// A criterion (ODF 1.3 part 4, 4.11.8): an optional comparator, then a number or text.
    Criterion,
    /// A lookup value, matched as a text or number.
    Lookup,
    /// SEARCH's text to find (the whole-cell property is about cells, not this).
    Search,
    /// D-functions' criteria range: its cells are criteria, which the formula doesn't show.
    Range,
}

pub(crate) const MATCHING: &str = "criteria the document matches with regular expressions, without wildcards or against part of a cell, unlike Excel";
pub(crate) const CASE: &str = "text comparisons the document makes case-sensitive, unlike Excel";

impl Matching {
    /// Excel's matching: wildcards, no regular expressions, criteria against the whole cell,
    /// case ignored.
    fn is_excel(self) -> bool {
        self.wildcards && !self.regex && self.whole_cell && !self.case_sensitive
    }

    /// Why `expr` can mean something else under these settings than in Excel, if it can: it
    /// compares text that may differ in case only (part 4, 6.4.7 to 6.4.9) or calls a function
    /// that matches an argument differently from the Excel function of its name (COUNTIF 6.13.9,
    /// COUNTIFS, SUMIF(S), AVERAGEIF(S), the lookups MATCH, VLOOKUP, HLOOKUP and LOOKUP, SEARCH
    /// 6.20.20 and the database functions 6.9 depend on them). An argument whose meaning is the
    /// same under both (a number, or text without the characters either reads specially) is
    /// fine; any other expression could be such text when calculated.
    pub(crate) fn differs(self, expr: &gridcraft_formula::Expr) -> Option<&'static str> {
        if self.is_excel() {
            return None;
        }
        let ignoring_case = Matching { case_sensitive: false, ..self };
        if ignoring_case.calls_differ(expr) {
            Some(MATCHING)
        } else if self.case_sensitive && (self.calls_differ(expr) || compares_cased_text(expr)) {
            Some(CASE)
        } else {
            None
        }
    }

    fn calls_differ(self, expr: &gridcraft_formula::Expr) -> bool {
        if self.is_excel() {
            return false;
        }
        let mut differs = false;
        expr.walk(&mut |e| {
            if let gridcraft_formula::Expr::Call(name, args) = e {
                differs |= args.iter().enumerate().any(|(i, a)| matched(name, i).is_some_and(|how| !self.same(a, how)));
            }
        });
        differs
    }

    fn same(self, arg: &gridcraft_formula::Expr, how: Matched) -> bool {
        use gridcraft_formula::{Expr, UnOp};
        match arg {
            Expr::Number(_) | Expr::Bool(_) | Expr::Missing => true,
            Expr::Unary(UnOp::Neg | UnOp::Plus, e) | Expr::Paren(e) => self.same(e, how),
            Expr::Text(t) => self.same_text(t, how),
            _ => false,
        }
    }

    fn same_text(self, text: &str, how: Matched) -> bool {
        let pattern = match how {
            Matched::Range => return false,
            // SEARCH ignores case whatever the setting (part 4, 6.20.20).
            Matched::Search => return self.same_pattern(text),
            Matched::Lookup if self.case_sensitive && has_case(text) => return false,
            Matched::Lookup => text,
            Matched::Criterion => {
                let op = ["<=", ">=", "<>", "<", ">", "="].into_iter().find(|op| text.starts_with(op));
                let rest = &text[op.map_or(0, str::len)..];
                // An empty value or a number: no text compared.
                if rest.is_empty() || rest.trim().parse::<f64>().is_ok() {
                    return true;
                }
                if self.case_sensitive && has_case(rest) {
                    return false;
                }
                // An ordered comparison: no text matching.
                if matches!(op, Some("<=" | ">=" | "<" | ">")) {
                    return true;
                }
                rest
            }
        };
        self.whole_cell && self.same_pattern(pattern)
    }

    /// Whether text to match means the same as Excel's wildcard pattern: no character either
    /// side reads specially (`*`, `?` and `~` for Excel; a regular expression's metacharacters).
    fn same_pattern(self, text: &str) -> bool {
        if self.wildcards && !self.regex {
            return true;
        }
        let special: &[char] =
            if self.regex { &['*', '?', '~', '.', '^', '$', '+', '(', ')', '[', ']', '{', '}', '|', '\\'] } else { &['*', '?', '~'] };
        !text.contains(special)
    }
}

/// Whether `text` has a letter with another case.
fn has_case(text: &str) -> bool {
    text.chars().any(|c| c.to_lowercase().ne(c.to_uppercase()))
}

/// Whether `expr` compares two operands that may both be text with letters, where case-sensitive
/// comparison (ODF 1.3 part 4, 6.4.7 to 6.4.9) can differ from Excel's. A number or logical
/// operand compares no text; text without letters of either case equals the same text in any
/// case, and the empty text orders before every other.
fn compares_cased_text(expr: &gridcraft_formula::Expr) -> bool {
    use gridcraft_formula::{BinOp, Expr};
    let mut found = false;
    expr.walk(&mut |e| {
        if let Expr::Binary(op @ (BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge), a, b) = e {
            let caseless = |x: &Expr| {
                not_text(x)
                    || match bare(x) {
                        Expr::Text(t) if matches!(op, BinOp::Eq | BinOp::Ne) => !has_case(t),
                        Expr::Text(t) => t.is_empty(),
                        _ => false,
                    }
            };
            found |= !caseless(a) && !caseless(b);
        }
    });
    found
}

/// `e` without its parentheses.
fn bare(mut e: &gridcraft_formula::Expr) -> &gridcraft_formula::Expr {
    while let gridcraft_formula::Expr::Paren(inner) = e {
        e = inner;
    }
    e
}

/// Whether `e` is never text: a number, logical or error, arithmetic, or a call of a function
/// that returns only numbers, logicals or errors.
fn not_text(e: &gridcraft_formula::Expr) -> bool {
    use gridcraft_formula::{BinOp, Expr, UnOp};
    match bare(e) {
        Expr::Number(_) | Expr::Bool(_) | Expr::Error(_) | Expr::Unary(UnOp::Neg | UnOp::Percent, _) => true,
        // A unary plus gives its operand back, text included.
        Expr::Unary(UnOp::Plus, x) => not_text(x),
        Expr::Binary(op, ..) => matches!(
            op,
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Pow | BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge
        ),
        Expr::Call(name, _) => matches!(
            name.as_str(),
            "ABS"
                | "ACOS"
                | "AND"
                | "ASIN"
                | "ATAN"
                | "ATAN2"
                | "AVERAGE"
                | "AVERAGEIF"
                | "AVERAGEIFS"
                | "CEILING"
                | "CEILING.MATH"
                | "CODE"
                | "COLUMN"
                | "COLUMNS"
                | "COMBIN"
                | "COS"
                | "COUNT"
                | "COUNTA"
                | "COUNTBLANK"
                | "COUNTIF"
                | "COUNTIFS"
                | "DATE"
                | "DATEVALUE"
                | "DAY"
                | "DAYS"
                | "DEGREES"
                | "EDATE"
                | "EOMONTH"
                | "EVEN"
                | "EXACT"
                | "EXP"
                | "FACT"
                | "FALSE"
                | "FIND"
                | "FLOOR"
                | "FLOOR.MATH"
                | "HOUR"
                | "INT"
                | "ISBLANK"
                | "ISERR"
                | "ISERROR"
                | "ISEVEN"
                | "ISLOGICAL"
                | "ISNA"
                | "ISNONTEXT"
                | "ISNUMBER"
                | "ISODD"
                | "ISREF"
                | "ISTEXT"
                | "LARGE"
                | "LEN"
                | "LN"
                | "LOG"
                | "LOG10"
                | "MATCH"
                | "MAX"
                | "MAXIFS"
                | "MEDIAN"
                | "MIN"
                | "MINIFS"
                | "MINUTE"
                | "MOD"
                | "MONTH"
                | "N"
                | "NOT"
                | "NOW"
                | "ODD"
                | "OR"
                | "PI"
                | "POWER"
                | "PRODUCT"
                | "RADIANS"
                | "RAND"
                | "RANDBETWEEN"
                | "RANK"
                | "ROUND"
                | "ROUNDDOWN"
                | "ROUNDUP"
                | "ROW"
                | "ROWS"
                | "SEARCH"
                | "SECOND"
                | "SIGN"
                | "SIN"
                | "SMALL"
                | "SQRT"
                | "SUM"
                | "SUMIF"
                | "SUMIFS"
                | "SUMPRODUCT"
                | "SUMSQ"
                | "TAN"
                | "TIME"
                | "TIMEVALUE"
                | "TODAY"
                | "TRUE"
                | "TRUNC"
                | "VALUE"
                | "WEEKDAY"
                | "WEEKNUM"
                | "XOR"
                | "YEAR"
        ),
        _ => false,
    }
}

/// How the `i`th argument of the Excel function `name` is matched, if it is.
fn matched(name: &str, i: usize) -> Option<Matched> {
    Some(match name {
        "COUNTIF" | "SUMIF" | "AVERAGEIF" if i == 1 => Matched::Criterion,
        "COUNTIFS" if i % 2 == 1 => Matched::Criterion,
        "SUMIFS" | "AVERAGEIFS" | "MAXIFS" | "MINIFS" if i >= 2 && i.is_multiple_of(2) => Matched::Criterion,
        "MATCH" | "VLOOKUP" | "HLOOKUP" | "LOOKUP" if i == 0 => Matched::Lookup,
        "SEARCH" | "SEARCHB" if i == 0 => Matched::Search,
        "DAVERAGE" | "DCOUNT" | "DCOUNTA" | "DGET" | "DMAX" | "DMIN" | "DPRODUCT" | "DSTDEV" | "DSTDEVP" | "DSUM" | "DVAR" | "DVARP" if i == 2 => {
            Matched::Range
        }
        _ => return None,
    })
}

struct Part {
    text: String,
    union: bool,
}

impl Part {
    /// The part as an argument: a union is parenthesized, where Excel's `,` would otherwise
    /// separate arguments.
    fn arg(&self) -> String {
        if self.union { format!("({})", self.text) } else { self.text.clone() }
    }
}

struct Parser<'a> {
    toks: Vec<Tok>,
    i: usize,
    depth: usize,
    doc: &'a dyn Sheets,
}

impl Parser<'_> {
    fn next(&mut self) -> Option<&Tok> {
        let t = self.toks.get(self.i);
        self.i += 1;
        t
    }

    /// Reads up to the closer (consumed) or the end, returning the `;`-separated parts.
    fn seq(&mut self, close: Option<&Tok>) -> Result<Vec<Part>, Unsupported> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(Unsupported("formula nested too deeply"));
        }
        let mut parts = vec![Part { text: String::new(), union: false }];
        loop {
            let i = self.i;
            let Some(t) = self.next() else {
                if close.is_some() {
                    return Err(Unsupported("unbalanced parentheses"));
                }
                break;
            };
            if Some(t) == close {
                break;
            }
            let mut text = String::new();
            match t {
                Tok::Raw(s) | Tok::Space(s) => text.push_str(s),
                Tok::Sep if close.is_some() => {
                    parts.push(Part { text: String::new(), union: false });
                    continue;
                }
                Tok::Union => {
                    if let Some(p) = parts.last_mut() {
                        p.union = true;
                    }
                    text.push(',');
                }
                Tok::Intersect => text.push(' '),
                Tok::Ident(name) => {
                    let name = name.clone();
                    if self.toks.get(i + 1) == Some(&Tok::Open) {
                        self.i += 1;
                        let args = self.seq(Some(&Tok::Close))?;
                        let args: Vec<String> =
                            if args.len() == 1 && args[0].text.is_empty() { vec![] } else { args.iter().map(Part::arg).collect() };
                        text = call(&name, args, self.doc)?;
                    } else {
                        // A name is spelled as the import renamed it. An identifier the file
                        // doesn't define is left alone: LibreOffice reads a bare `A13` as that cell.
                        text = self.doc.excel_name(&name).unwrap_or(name);
                    }
                }
                Tok::Open => {
                    let inner = self.seq(Some(&Tok::Close))?;
                    let [one] = inner.as_slice() else { return Err(Unsupported("malformed parentheses")) };
                    text = format!("({})", one.text);
                }
                Tok::LBrace => text = self.array()?,
                _ => return Err(Unsupported("malformed formula")),
            }
            if let Some(p) = parts.last_mut() {
                p.text.push_str(&text);
            }
        }
        self.depth -= 1;
        Ok(parts)
    }

    /// An inline array after `{`: `;` separates columns and `|` rows.
    fn array(&mut self) -> Result<String, Unsupported> {
        let mut text = String::from("{");
        loop {
            match self.next() {
                Some(Tok::RBrace) => break,
                Some(Tok::Sep) => text.push(','),
                Some(Tok::Pipe) => text.push(';'),
                Some(Tok::Raw(s) | Tok::Space(s)) => text.push_str(s),
                Some(Tok::Ident(s)) => text.push_str(s),
                _ => return Err(Unsupported("malformed inline array")),
            }
        }
        text.push('}');
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Same;
    impl Sheets for Same {
        fn sheet(&self, written: &str) -> String {
            if written == "Bad/Name" { "Bad_Name".into() } else { written.into() }
        }
        fn repaired(&self) -> bool {
            true
        }
        fn names_mention(&self, _: &str) -> bool {
            false
        }
        fn excel_name(&self, _: &str) -> Option<String> {
            None
        }
        fn indirect_name(&self, _: Option<&str>, _: &str) -> Result<Option<IndirectName>, Unsupported> {
            Ok(None)
        }
    }

    fn of(s: &str) -> Result<String, Unsupported> {
        cell_formula(s, &Same)
    }

    #[test]
    fn references_separators_and_arrays_become_excel_syntax() {
        let cases = [
            ("of:=[.A1]*2", "A1*2"),
            ("of:=SUM([.$A$1:.B$2]; 3)", "SUM($A$1:B$2,3)"),
            ("of:=[$Sheet1.$C$4]/12*[.B7]", "'Sheet1'!$C$4/12*B7"),
            ("oooc:=SUM([.C7:.C367])", "SUM(C7:C367)"),
            ("of:=SUM(['It''s'.A1:.B2])", "SUM('It''s'!A1:B2)"),
            ("of:=SUM([bug.G11:bug.G251])", "SUM('bug'!G11:G251)"),
            ("of:=SHEETS([Sheet1.A1:Sheet3.G12])", "SHEETS('Sheet1:Sheet3'!A1:G12)"),
            ("of:=SUM([.C:.C])+SUM([.$2:.3])", "SUM(C:C)+SUM($2:3)"),
            ("of:=[$Bad/Name.A1]", "'Bad_Name'!A1"),
            ("of:=SUM([.A1]~[.B2])", "SUM((A1,B2))"),
            ("of:=INDEX([.A1:.B3]~[.D1:.E3];1;1;2)", "INDEX((A1:B3,D1:E3),1,1,2)"),
            ("of:=SUM([.A1:.C3]![.B2:.D4])", "SUM(A1:C3 B2:D4)"),
            ("of:=SUM({1;2|3;4})", "SUM({1,2;3,4})"),
            ("of:=COMPLEX({2;3};{3;7};\"j\")", "COMPLEX({2,3},{3,7},\"j\")"),
            ("of:=IF([.A1]=\"a;b\";\"x\"\"y\";1.5E+3)", "IF(A1=\"a;b\",\"x\"\"y\",1.5E+3)"),
            ("of:=[.E26]-[#REF!]+[.F1:.#REF!]", "E26-#REF!+#REF!"),
            ("of:=COM.MICROSOFT.XLOOKUP(1;[.A1:.A3];[.B1:.B3])", "XLOOKUP(1,A1:A3,B1:B3)"),
            ("of:=LEGACY.CHIDIST(3;2)+ERROR.TYPE(#N/A)+BINOM.DIST.RANGE(3;0.5;1)", "CHIDIST(3,2)+ERROR.TYPE(#N/A)+BINOM.DIST.RANGE(3,0.5,1)"),
            ("of:=FORMULA([.A1])", "FORMULATEXT(A1)"),
            ("of:=PI()+TRUE()-IF(;1)", "PI()+TRUE()-IF(,1)"),
            ("of:=-(1+2)%", "-(1+2)%"),
            ("msoxl:=SUM(A1:B2)", "SUM(A1:B2)"),
            // OpenFormula signatures mapped to the Excel function with the same meaning.
            ("of:=ROUND([.A1])+ROUNDDOWN(2.5;1)", "ROUND(A1,0)+ROUNDDOWN(2.5,1)"),
            ("of:=FLOOR(-2.5;-2)+CEILING(7;;1)+COM.MICROSOFT.FLOOR(2.5;2)", "FLOOR.MATH(-2.5,-2)+CEILING.MATH(7,1,1)+FLOOR(2.5,2)"),
            ("of:=BETADIST(0.5;2;3;0;1;FALSE())+BETADIST(0.5;2;3)", "BETA.DIST(0.5,2,3,FALSE(),0,1)+BETADIST(0.5,2,3)"),
            ("of:=LOGNORMDIST(2)+LOGNORMDIST(2;0;1;FALSE())", "LOGNORMDIST(2,0,1)+LOGNORM.DIST(2,0,1,FALSE())"),
            ("of:=HYPGEOMDIST(1;2;3;10;TRUE())+POISSON(2;3)+NORMDIST(1;0;1)", "HYPGEOM.DIST(1,2,3,10,TRUE())+POISSON(2,3,TRUE)+NORMDIST(1,0,1,TRUE)"),
            (
                "of:=FDIST(1;2;3)+FINV(0.5;2;3)+LEGACY.FDIST(1;2;3)+CHISQDIST(1;2)",
                "F.DIST(1,2,3,TRUE)+F.INV(0.5,2,3)+FDIST(1,2,3)+CHISQ.DIST(1,2,TRUE)",
            ),
            ("=1+1", "1+1"),
            // The `=` is optional, and `==` forces recalculation (ODF 1.3 part 4, 5.2).
            ("of:#N/A", "#N/A"),
            ("of:TRUE()", "TRUE()"),
            ("of:==NOW()", "NOW()"),
            // The range operator joins references functions return.
            ("of:=SUM(INDIRECT(\"A1\"):INDIRECT(\"B2\"))", "SUM(INDIRECT(\"A1\"):INDIRECT(\"B2\"))"),
            ("of:=SUM([.$O6]:CHOOSE(2;[.$O6];[.$P6]))", "SUM($O6:CHOOSE(2,$O6,$P6))"),
            // Text right after an error constant, where a longer error's length ends inside a character.
            ("of:=#N/A+\"é\"", "#N/A+\"é\""),
            ("of:=#N/A&\"日本\"", "#N/A&\"日本\""),
            // `#REF!` inside a quoted sheet name is part of the name, not a broken reference.
            ("of:=SUM(['Budget#REF!'.A1])+['It''s #REF!'.B2]", "SUM('Budget#REF!'!A1)+'It''s #REF!'!B2"),
            ("of:=[$#REF!.A1]+['ok'.A1:.#REF!]", "#REF!+#REF!"),
            // OpenFormula's MROUND: ties to the greater multiple, any signs; B once, by a name A lacks.
            ("of:=MROUND([.A1];-2)", "LET(_xlpm.mround_b,ABS(-2),FLOOR.MATH((A1)+_xlpm.mround_b/2,_xlpm.mround_b))"),
            ("of:=MROUND(mround_b;2)", "LET(_xlpm.mround_b_,ABS(2),FLOOR.MATH((mround_b)+_xlpm.mround_b_/2,_xlpm.mround_b_))"),
            // INDIRECT's text with OpenFormula's sheet separator becomes Excel's (part 4, 6.14.7).
            ("of:=INDIRECT(\"Sheet2.A1\")", "INDIRECT(\"'Sheet2'!A1\")"),
            ("of:=INDIRECT(\"$'It''s'.$A$1:B2\";1)", "INDIRECT(\"'It''s'!$A$1:B2\",1)"),
            ("of:=INDIRECT(\"Bad/Name.A1:Bad/Name.C3\";)", "INDIRECT(\"'Bad_Name'!A1:C3\",TRUE)"),
            (
                "of:=INDIRECT(\"B3\")+INDIRECT(\"Sheet2!B3\")+INDIRECT(\"'a.b'!B3\")",
                "INDIRECT(\"B3\")+INDIRECT(\"Sheet2!B3\")+INDIRECT(\"'a.b'!B3\")",
            ),
            ("of:=INDIRECT(\"R1C1\";0)", "INDIRECT(\"R1C1\",0)"),
            ("of:=INDIRECT(ADDRESS(1;2;1;1;\"Sheet2\"))", "INDIRECT(ADDRESS(1,2,1,1,\"Sheet2\"))"),
            // A sheet named either way is named as imported (`Bad/Name` was repaired).
            ("of:=INDIRECT(\"'Bad/Name'!A1\")", "INDIRECT(\"'Bad_Name'!A1\")"),
        ];
        for (src, want) in cases {
            assert_eq!(of(src), Ok(want.to_string()), "{src}");
        }
    }

    /// The LET MROUND adds binds a name no named expression of the document spells: a name is
    /// evaluated where it is used, so `alias` = `mround_b` would read the binding.
    #[test]
    fn mround_binding_avoids_the_documents_names() {
        struct Named;
        impl Sheets for Named {
            fn sheet(&self, written: &str) -> String {
                written.into()
            }
            fn repaired(&self) -> bool {
                false
            }
            fn names_mention(&self, name: &str) -> bool {
                "alias\nmround_b\nmround_b\n100".contains(name)
            }
            fn excel_name(&self, _: &str) -> Option<String> {
                None
            }
            fn indirect_name(&self, _: Option<&str>, _: &str) -> Result<Option<IndirectName>, Unsupported> {
                Ok(None)
            }
        }
        assert_eq!(
            cell_formula("of:=MROUND(alias;2)", &Named),
            Ok("LET(_xlpm.mround_b_,ABS(2),FLOOR.MATH((alias)+_xlpm.mround_b_/2,_xlpm.mround_b_))".into())
        );
    }

    /// ADDRESS writes the sheet it is given the way INDIRECT reads it: the pair means the same in
    /// Excel for a sheet named as imported, a computed one when no sheet was repaired included.
    #[test]
    fn indirect_of_address_follows_the_sheet_names_as_imported() {
        struct Plain;
        impl Sheets for Plain {
            fn sheet(&self, written: &str) -> String {
                written.into()
            }
            fn repaired(&self) -> bool {
                false
            }
            fn names_mention(&self, _: &str) -> bool {
                false
            }
            fn excel_name(&self, _: &str) -> Option<String> {
                None
            }
            fn indirect_name(&self, _: Option<&str>, _: &str) -> Result<Option<IndirectName>, Unsupported> {
                Ok(None)
            }
        }
        let computed = "of:=INDIRECT(ADDRESS(1;2;1;1;[.A1]))";
        assert_eq!(cell_formula(computed, &Plain), Ok("INDIRECT(ADDRESS(1,2,1,1,A1))".into()));
        assert!(of(computed).is_err());
    }

    /// Under case-sensitive matching (ODF 1.3 part 4, 3.4; the default) a comparison of operands
    /// that may be text with letters can differ from Excel's, which ignores case; one that
    /// compares no such text can't. A unary plus returns its operand, text included (6.4.15).
    #[test]
    fn case_sensitive_comparisons_are_told_from_those_case_cant_change() {
        let sensitive = Matching { regex: false, wildcards: true, whole_cell: true, case_sensitive: true };
        let insensitive = Matching { case_sensitive: false, ..sensitive };
        let parse = |src: &str| gridcraft_formula::parse(src).unwrap_or_else(|e| panic!("{src}: {e:?}"));
        for src in ["A1=\"a\"", "A1<>B1", "\"a\"=\"A\"", "A1<\"b\"", "+A1=\"x\"", "IF(A1=B1,1,2)", "LEFT(A1)=\"a\"", "A1&\"x\"=B1"] {
            assert_eq!(sensitive.differs(&parse(src)), Some(CASE), "{src}");
            assert_eq!(insensitive.differs(&parse(src)), None, "{src}");
        }
        for src in ["A1=1", "A1=\"\"", "A1>\"\"", "A1=\"123\"", "LEN(A1)=1", "A1+1>B1", "(A1=1)=TRUE", "-A1<B1", "+(A1+1)=2", "SUM(A1:A3)>=B1"] {
            assert_eq!(sensitive.differs(&parse(src)), None, "{src}");
        }
    }

    #[test]
    fn unsupported_syntax_keeps_the_saved_result() {
        for src in [
            "of:#ERR502!",
            "of:=LOOKUP(0;['lookup_source.fods'#$Data.A3:.A8])",
            "of:=['file:///x.ods'#$Data.A1]",
            "of:=ORG.LIBREOFFICE.ROUNDSIG(1;2)",
            "of:=ORG.OPENOFFICE.WEEKS([.A1];[.A2];0)",
            "of:=ORG.OPENOFFICE.EASTERSUNDAY(2020)",
            "of:=MULTIPLE.OPERATIONS([.A1];[.B1];[.C1])",
            "of:=SUM([.A1]",
            "of:=[.A1",
            "of:=\"open",
            "of:=[.XFE1]",
            "of:=[.A0]",
            "of:=[.A1:.B]",
            "of:=#ERR!",
            "of:Err:502",
            "of:=err:504+1",
            "of:=#err504!",
            "of:=1?2",
            "xx:=1",
            // INDIRECT text the formula computes, or a dot read in R1C1 style.
            "of:=INDIRECT([.A1])",
            "of:=INDIRECT(\"Sheet\"&\"2.A1\")",
            "of:=INDIRECT(\"Sheet2.R1C1\";0)",
            "of:=INDIRECT(\"Sheet2.A1\";[.B1])",
            "of:=INDIRECT(\"my.name\")",
            // ADDRESS for a sheet repaired on import, or one the formula computes where one was.
            "of:=INDIRECT(ADDRESS(1;2;1;1;\"Bad/Name\"))",
            "of:=INDIRECT(ADDRESS(1;2;1;1;[.A1]))",
        ] {
            assert!(of(src).is_err(), "{src}");
        }
        let deep = format!("of:={}1{}", "(".repeat(300), ")".repeat(300));
        assert!(of(&deep).is_err());
    }

    #[test]
    fn named_expressions_move_relative_parts_by_their_base_cell() {
        let base = Some(Base { row: 1, col: 1 });
        assert_eq!(translate("[$Sheet1.$A$1:.$B$5]", &Same, base), Ok("'Sheet1'!$A$1:$B$5".into()));
        // One row up and one column left of the base B2: from A1 that is the last row and column.
        assert_eq!(translate("[.A1]", &Same, base), Ok("XFD1048576".into()));
        assert_eq!(translate("[.C3]+[.$C3]", &Same, base), Ok("B2+$C2".into()));
        assert_eq!(translate("COM.MICROSOFT.LAMBDA(_xlpm.x; _xlpm.x+2)", &Same, None), Ok("LAMBDA(_xlpm.x,_xlpm.x+2)".into()));
    }
}
