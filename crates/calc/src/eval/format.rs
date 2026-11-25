use super::{CellError, Value};
use gridcraft_core::{CellRef, MAX_COLS, MAX_ROWS};

/// CELL("format") code for a number format.
pub(super) fn cell_format_code(code: &str) -> String {
    let c = code.to_ascii_lowercase();
    if c == "general" {
        return "G".into();
    }
    if c.contains('%') {
        return format!("P{}", decimals(&c));
    }
    if c.contains('e') && c.contains('0') {
        return format!("S{}", decimals(&c));
    }
    if c.contains('$') {
        return format!("C{}", decimals(&c));
    }
    if c.contains('y') || c.contains('d') {
        return "D1".into();
    }
    if c.contains('h') {
        return "D9".into();
    }
    if c.contains(',') {
        return format!(",{}", decimals(&c));
    }
    format!("F{}", decimals(&c))
}

fn decimals(c: &str) -> usize {
    c.split_once('.').map_or(0, |(_, d)| d.chars().take_while(|ch| *ch == '0').count())
}

pub(super) fn same_kind(a: &Value, b: &Value) -> bool {
    std::mem::discriminant(a) == std::mem::discriminant(b) || (a.is_empty() || b.is_empty())
}

/// One end of an R1C1 reference, as A1 text pieces: a cell, a whole row or a whole column.
enum R1C1Part {
    Cell(String),
    Row(String),
    Col(String),
}

/// Converts an R1C1 reference text to A1 (for INDIRECT(…, FALSE)). `Ok(None)` means the text is
/// not R1C1; `Err` means it is R1C1 but points past the sheet.
///
/// As in Excel: a relative offset wraps around the sheet's edge (`R[-6]C` in row 6 is row
/// 1048576, `R[1]C[1]` in XFD1048576 is A1) as long as it is shorter than the sheet; `R2`, `C2`,
/// `R`, `C[1]`, `R1:R2` and `C2:C3` are whole rows and columns.
pub fn r1c1_to_a1(s: &str, at: CellRef) -> Result<Option<String>, CellError> {
    let (sheet, body) = match s.rsplit_once('!') {
        Some((sh, b)) => (format!("{sh}!"), b),
        None => (String::new(), s),
    };
    // `None`: not R1C1. `Some(None)`: R1C1, but past the sheet.
    let num = |p: &str, base: u32, max: u32| -> Option<Option<(u32, bool)>> {
        let max = i64::from(max);
        if p.is_empty() {
            return Some(Some((base, false)));
        }
        if let Some(inner) = p.strip_prefix('[').and_then(|x| x.strip_suffix(']')) {
            let off: i64 = inner.parse().ok()?;
            return Some((off.unsigned_abs() < max.unsigned_abs()).then(|| ((i64::from(base) + off).rem_euclid(max) as u32, false)));
        }
        let v = p.parse::<u32>().ok()?.checked_sub(1).map(i64::from);
        Some(v.filter(|v| (0..max).contains(v)).map(|v| (v as u32, true)))
    };
    let dollar = |absolute: bool| if absolute { "$" } else { "" };
    let row = |(r, abs): (u32, bool)| format!("{}{}", dollar(abs), r + 1);
    let col = |(c, abs): (u32, bool)| format!("{}{}", dollar(abs), gridcraft_core::col_to_letters(c));
    let conv = |part: &str| -> Result<Option<R1C1Part>, CellError> {
        let u = part.to_ascii_uppercase();
        if let Some(rest) = u.strip_prefix('R') {
            let (rpart, cpart) = match rest.split_once('C') {
                Some((r, c)) => (r, Some(c)),
                None => (rest, None),
            };
            let Some(r) = num(rpart, at.row, MAX_ROWS) else { return Ok(None) };
            let Some(cpart) = cpart else { return r.map(|r| Some(R1C1Part::Row(row(r)))).ok_or(CellError::Ref) };
            let Some(c) = num(cpart, at.col, MAX_COLS) else { return Ok(None) };
            let (Some(r), Some(c)) = (r, c) else { return Err(CellError::Ref) };
            return Ok(Some(R1C1Part::Cell(format!("{}{}", col(c), row(r)))));
        }
        let Some(cpart) = u.strip_prefix('C') else { return Ok(None) };
        let Some(c) = num(cpart, at.col, MAX_COLS) else { return Ok(None) };
        c.map(|c| Some(R1C1Part::Col(col(c)))).ok_or(CellError::Ref)
    };
    let out = match body.split_once(':') {
        Some((a, b)) => match (conv(a)?, conv(b)?) {
            (Some(R1C1Part::Cell(a)), Some(R1C1Part::Cell(b)))
            | (Some(R1C1Part::Row(a)), Some(R1C1Part::Row(b)))
            | (Some(R1C1Part::Col(a)), Some(R1C1Part::Col(b))) => {
                format!("{a}:{b}")
            }
            _ => return Ok(None),
        },
        None => match conv(body)? {
            Some(R1C1Part::Cell(x)) => x,
            Some(R1C1Part::Row(x) | R1C1Part::Col(x)) => format!("{x}:{x}"),
            None => return Ok(None),
        },
    };
    Ok(Some(format!("{sheet}{out}")))
}
