//! Rules the non-XLSX readers (`.xls`, XLSB, ODS) apply the way the XLSX reader does: what a file
//! says about its sheets never fails the import. A workbook with nothing to show still opens, and
//! names Excel forbids are repaired, each with a warning.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gridcraft_core::{CellRef, is_r1c1_name, valid_defined_name};
use gridcraft_model::{Sheet, Workbook};

use crate::ReadReport;

/// Said once when any sheet was renamed.
pub(crate) const RENAMED: &str = "Some sheet names were shortened or renamed to valid, unique sheet names.";

/// The characters a sheet name cannot hold.
fn forbidden(c: char) -> bool {
    matches!(c, ':' | '\\' | '/' | '?' | '*' | '[' | ']')
}

/// The name for the sheet a file calls `original`, as the `index`-th sheet of `wb` (the sheets
/// before it already in place): the characters Excel forbids are dropped, as are apostrophes at
/// either end, the rest is cut to 31 characters, an empty name becomes `Sheet<n>` (a name of spaces
/// only is one Excel takes and is kept), and a name
/// another sheet already has (or the reserved `History`) gets a ` (2)`, ` (3)`… suffix. The
/// result passes [`Workbook::check_sheet_name`], so the editor never meets an imported sheet
/// whose own name it refuses.
///
/// Excel refuses all of these when a person types a name, but files written by other programs
/// (or by Excel itself, long ago) carry them, and a reader that refuses the file for that is
/// refusing data it could show. An XLSX written from the result is one Excel opens.
pub(crate) fn readable_sheet_name(wb: &Workbook, original: &str, index: usize) -> String {
    let kept: String = original.chars().filter(|c| !forbidden(*c)).collect();
    let cut: String = kept.trim_matches('\'').chars().take(31).collect();
    // Cutting can leave the apostrophe that sat at the 31st character at the end.
    let mut names = crate::read::SheetNames::of(wb);
    let name = names.unique(cut.trim_end_matches('\''), index);
    if wb.check_sheet_name(&name, None).is_ok() {
        return name;
    }
    // What the editor still refuses is Excel's reserved `History`: stay recognisable.
    let name = names.unique(&format!("{name} (2)"), index);
    if wb.check_sheet_name(&name, None).is_ok() { name } else { wb.next_sheet_name() }
}

/// Whether a defined name reads as a cell reference (`MU1`, `TAX2012`) or as an R1C1 one (`R`,
/// `C`, `RC`, `R2C3`), which Excel doesn't accept as a name. Files written when the grid ended at
/// column IV carry names such as `MU1`; in today's 16,384-column grid they are cells.
pub(crate) fn reads_as_reference(name: &str) -> bool {
    CellRef::parse(name).is_some() || is_r1c1_name(name)
}

/// Said once when a reader renamed defined names Excel can't hold.
pub(crate) const NAMES_RENAMED: &str = "Some defined names Excel doesn't allow (a cell reference such as TAX2020, or a name with spaces, hyphens or a leading digit) were renamed to valid, unique names with a leading underscore or underscores for the characters Excel forbids; formulas that use them follow.";

/// The longest defined name Excel holds, in characters.
const MAX_NAME_CHARS: usize = 255;

/// The name for the defined name `original` when Excel can't hold it: each character Excel
/// forbids (a space, `-`, `/`…) becomes `_`, and a `_` goes in front of a name that doesn't start
/// with a letter, `_` or `\` (`1st`) or that reads as a reference ([`reads_as_reference`]: `MU1`
/// becomes `_MU1`, as the `.xls` and XLSB readers do for what Excel itself renames), and a name
/// that is too long is cut. The result passes [`valid_defined_name`]; making it unique is the
/// caller's, by putting more underscores in front.
pub(crate) fn excel_name(original: &str) -> String {
    let mut name: String = original.chars().map(|c| if c.is_alphanumeric() || matches!(c, '_' | '.' | '\\' | '?') { c } else { '_' }).collect();
    while !valid_defined_name(&name) {
        if name.chars().count() > MAX_NAME_CHARS {
            name = name.chars().take(MAX_NAME_CHARS).collect();
        } else {
            name.insert(0, '_');
        }
    }
    name
}

/// The names Excel holds for the defined names (`written`) that `needs_rename` says it can't hold
/// as they are, by the written name in lower case (names compare without regard to case): each
/// gets [`excel_name`], made different from every name the document keeps (the `written` ones
/// `needs_rename` passes) and from every other new one by more underscores in front. One map
/// serves a name's definition and every formula that uses it, so a sheet's name that hides the
/// workbook's of its spelling still does. An empty name is left out. The ODS reader renames
/// whatever [`valid_defined_name`] refuses, the XLSB reader what [`reads_as_reference`] (the names
/// Excel itself renames when it opens a file); both make the result unique here, so no reader
/// leaves two names of one spelling.
pub(crate) fn excel_names<'a>(written: impl Iterator<Item = &'a str> + Clone, needs_rename: impl Fn(&str) -> bool) -> HashMap<String, String> {
    let mut taken: HashSet<String> = written.clone().filter(|n| !n.is_empty() && !needs_rename(n)).map(str::to_lowercase).collect();
    // How many underscores each base has tried, so a crowd of names meeting at one base stays linear.
    let mut tried: HashMap<String, usize> = HashMap::new();
    let mut renamed = HashMap::new();
    for name in written.filter(|n| !n.is_empty() && needs_rename(n)) {
        let key = name.to_lowercase();
        if renamed.contains_key(&key) {
            continue;
        }
        let base = excel_name(name);
        let k = tried.entry(base.to_lowercase()).or_default();
        let new = loop {
            let digits = k.to_string();
            let candidate = if base.chars().count() + *k <= MAX_NAME_CHARS {
                format!("{}{base}", "_".repeat(*k))
            } else {
                // Past the length limit: the counter replaces the end of the name.
                format!("{}{digits}", base.chars().take(MAX_NAME_CHARS - digits.len()).collect::<String>())
            };
            *k += 1;
            if taken.insert(candidate.to_lowercase()) {
                break candidate;
            }
        };
        renamed.insert(key, new);
    }
    renamed
}

/// A workbook whose file holds no worksheet to show (a chart-only workbook, which Excel opens)
/// gets one empty sheet and a warning, as when an XLSX has none, instead of failing to open.
pub(crate) fn ensure_a_sheet(wb: &mut Workbook, report: &mut ReadReport) {
    if wb.sheets.is_empty() {
        report.warn("the workbook has no worksheets; added an empty sheet");
        wb.sheets.push(Arc::new(Sheet::new("Sheet1")));
    }
}
