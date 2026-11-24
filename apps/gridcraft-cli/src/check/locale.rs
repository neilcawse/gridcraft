//! Caches computed under another locale. GridCraft calculates as en-US Excel does, but the
//! producer may have run Excel with other regional settings: `INT("01/09/2020 15:02:40")` is 1
//! September in en-GB, `TEXT(A1,"mmmm")` is `enero` in Spanish, `TEXT(0.15,"0.0%")` is `15%` in
//! Italian (`.` groups thousands there), `ÅÅÅÅ` is the year in Swedish.
//!
//! When a trusted cache has mismatches, the workbook is recalculated afresh under each candidate
//! locale ([`CANDIDATES`]). A locale is evidence only when the whole cache is consistent with it:
//!
//! 1. every cell that matched under en-US still matches exactly under it, and
//! 2. it reproduces at least one en-US mismatch exactly (the same comparison, tolerance included).
//!
//! Of the locales that pass, the one that reproduces the most mismatches wins (ties: the order of
//! [`CANDIDATES`]). The mismatches it reproduces are counted as matched and as `localeDependent`,
//! and the file reports the locale's tag; every other mismatch stays one.

use super::{Calc, HashMap, HashSet, Key, Value, Workbook};

use super::calc::{pin_volatile, unpin_volatile, values_match};
use gridcraft_engine::core::locale::{CANDIDATES, Locale, scoped};

/// Among [`CANDIDATES`], the locale the cache was computed under by the rule in the module docs,
/// with the mismatching cells (`differs`) it reproduces. `same` are the cells that matched under
/// en-US, `saved` the cache, and `relocalized` gives the workbook recalculated under a locale.
pub(super) fn reproducing(
    differs: &[Key],
    same: &[Key],
    saved: &HashMap<Key, Value>,
    tolerance: f64,
    relocalized: &mut dyn FnMut(&'static Locale) -> Option<Workbook>,
) -> Option<(&'static str, HashSet<Key>)> {
    let mut best: Option<(&'static str, HashSet<Key>)> = None;
    for &loc in CANDIDATES.iter() {
        let Some(wb) = relocalized(loc) else { continue };
        let matches = |k: &Key| {
            let actual = wb.sheet(k.0).and_then(|s| s.cell(k.1)).map(|x| x.value.scalar()).unwrap_or_default();
            saved.get(k).is_some_and(|e| values_match(e, &actual, tolerance))
        };
        if !same.iter().all(matches) {
            continue;
        }
        let reproduced: HashSet<Key> = differs.iter().copied().filter(matches).collect();
        if !reproduced.is_empty() && best.as_ref().is_none_or(|(_, b)| reproduced.len() > b.len()) {
            best = Some((loc.tag, reproduced));
        }
    }
    best
}

/// The workbook as the file holds it (`opened`: the reader itself doesn't depend on the locale, so
/// it is read once for all the locales) recalculated under `loc`, with the volatile formulas
/// holding their saved values (`saved`) as in the en-US calculation ([`pin_volatile`]).
pub(super) fn recalculated_under(opened: &Workbook, saved: &HashMap<Key, Value>, loc: &'static Locale) -> Workbook {
    let mut wb = opened.clone();
    let pinned = pin_volatile(&mut wb, saved);
    scoped(loc, || Calc::new().recalc_all(&mut wb));
    unpin_volatile(&mut wb, pinned);
    wb
}
