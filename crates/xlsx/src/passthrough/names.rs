//! The defined names scoped to kept sheets.
//!
//! A kept sheet is written back as it was, but its names are formulas over the workbook's own
//! sheets and cells (`Rate` = `Data!$A$1`), and the kept macro cells call them. They are rewritten
//! by the edits that move or rename what they name, as the workbook's own names are.

use gridcraft_formula::Expr;

use super::{Arc, Workbook};

/// Rewrites the formula of every defined name scoped to a kept sheet of `wb` through `rewrite`:
/// the edit that renamed a sheet, deleted it, or moved the cells a name refers to. A name that
/// `rewrite` leaves as it is stays exactly as the file spelled it, and so does one whose formula
/// can't be read.
pub fn rewrite_kept_names(wb: &mut Workbook, rewrite: impl Fn(Expr) -> Expr) {
    let Some(p) = wb.preserved.as_mut() else { return };
    let mut rewritten: Vec<(usize, usize, String)> = vec![];
    for (si, sheet) in p.sheets.iter().enumerate() {
        for (ni, name) in sheet.names.iter().enumerate() {
            // The names of the workbook are read and written through the same conversions.
            let Ok(expr) = gridcraft_formula::parse_name(&crate::fmla::from_file(name.formula.trim())) else { continue };
            let new = rewrite(expr.clone());
            if new != expr {
                rewritten.push((si, ni, crate::fmla::text_to_file(&gridcraft_formula::print(&new))));
            }
        }
    }
    if rewritten.is_empty() {
        return;
    }
    let p = Arc::make_mut(p);
    for (si, ni, formula) in rewritten {
        if let Some(name) = p.sheets.get_mut(si).and_then(|s| s.names.get_mut(ni)) {
            name.formula = formula;
        }
    }
}
