//! Formulas whose cached value GridCraft could not be expected to reproduce, because it came from
//! outside what Excel computes from the file: functions that are not Excel's own (an add-in's, a
//! VBA or other user-defined function, Excel 4 macro functions), functions that read from outside
//! the file. The cached value is then the only record of the
//! answer, and Excel itself, calculating without the add-in or the source, would not give it.

use std::cell::RefCell;
use std::sync::LazyLock;

use gridcraft_engine::formula::SheetSel;

use super::{Expr, HashMap, HashSet, Workbook};

/// The functions Excel itself provides (`excel_functions.txt`).
static EXCEL_FUNCTIONS: LazyLock<HashSet<&'static str>> =
    LazyLock::new(|| include_str!("excel_functions.txt").lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).collect());

/// Excel functions that read from outside the workbook: services, data models, cubes, the web, a
/// Python runtime, the cloud.
const EXTERNAL_DATA: [&str; 9] = ["RTD", "WEBSERVICE", "STOCKHISTORY", "IMAGE", "PY", "DETECTLANGUAGE", "TRANSLATE", "COPILOT", "FIELDVALUE"];

/// Excel 4 macro functions that turn up in formulas and defined names; `GET.*`, `SET.*` and
/// `FORMULA.*` are the macro sheet's families.
const XLM: [&str; 7] = ["CALL", "REGISTER", "REGISTER.ID", "UNREGISTER", "EXEC", "EXECUTE", "EVALUATE"];

/// Why a formula has no oracle; the strings are the JSON keys of `noOracleReasons`.
pub(super) mod why {
    /// The file saved the formula without a value (a writer that doesn't calculate).
    pub const NO_CACHED_VALUE: &str = "noCachedValue";
    pub const ADDIN: &str = "addin";
    pub const XLM: &str = "xlm";
    pub const UDF: &str = "udf";
    pub const EXTERNAL_DATA: &str = "externalData";
    /// Reads a cell that has no oracle; the formula itself calls nothing foreign.
    pub const DOWNSTREAM: &str = "downstream";
    /// Reads a sheet the workbook doesn't have: Excel turns the references to a sheet it deletes
    /// into `#REF!` and so never saves one, so the package lost the sheet after the producer
    /// calculated the cache.
    pub const MISSING_SHEET: &str = "missingSheet";
}

/// What `check` can tell about the places a formula reaches outside what it can compute.
pub(super) struct Foreign<'a> {
    wb: &'a Workbook,
    /// The verdict on each function name met (it doesn't depend on where it is called).
    calls: RefCell<HashMap<String, Option<&'static str>>>,
}

impl<'a> Foreign<'a> {
    pub(super) fn new(wb: &'a Workbook) -> Foreign<'a> {
        Foreign { wb, calls: RefCell::new(HashMap::new()) }
    }

    /// Why `e` can't be graded, or `None`. `defined` is the upper-cased names the workbook defines,
    /// which a call may name (`LAMBDA`s).
    pub(super) fn reason(&self, e: &Expr, defined: &HashSet<String>) -> Option<&'static str> {
        let mut found: Option<&'static str> = None;
        let mut bound: Option<HashSet<String>> = None;
        e.walk(&mut |x| {
            if found.is_some() {
                return;
            }
            found = match x {
                Expr::Call(n, _) => self.call(n, defined, e, &mut bound),
                Expr::Ref(r) => self.missing_sheet(&r.sheet),
                _ => None,
            };
        });
        found
    }

    /// A call to function `n`, in the formula `whole`.
    fn call(&self, n: &str, defined: &HashSet<String>, whole: &Expr, bound: &mut Option<HashSet<String>>) -> Option<&'static str> {
        let known = self.calls.borrow().get(n).copied();
        let class = match known {
            Some(c) => c,
            None => {
                let c = classify(n, defined);
                self.calls.borrow_mut().insert(n.to_string(), c);
                c
            }
        };
        // A name no function is called by may still be a variable of a `LET` or `LAMBDA` of this
        // formula, called as the function it holds.
        if class == Some(why::UDF) && bound.get_or_insert_with(|| bound_names(whole)).contains(&n.to_ascii_uppercase()) {
            return None;
        }
        class
    }

    /// A reference to a sheet of this workbook that it doesn't have (LibreOffice's
    /// empty_ext_defined_name.xlsx keeps formulas over `Auftragsdaten!` and `'Zeit pro Spritze'!`,
    /// sheets its `docProps/app.xml` still lists, with the values Excel 16 cached for them).
    fn missing_sheet(&self, sel: &SheetSel) -> Option<&'static str> {
        let absent = |s: &str| !s.is_empty() && !s.starts_with('[') && !self.wb.sheets.iter().any(|sh| sh.name.eq_ignore_ascii_case(s));
        let missing = match sel {
            SheetSel::Current => false,
            SheetSel::Named(s) => absent(s),
            SheetSel::Span(a, b) => absent(a) || absent(b),
        };
        missing.then_some(why::MISSING_SHEET)
    }
}

/// What kind of function `n` is to Excel: `None` if Excel computes it (or it is defined by a name
/// of the workbook), else why Excel couldn't without something outside the file.
fn classify(n: &str, defined: &HashSet<String>) -> Option<&'static str> {
    let upper = n.to_ascii_uppercase();
    if upper.starts_with("_XLL.") {
        return Some(why::ADDIN);
    }
    if EXTERNAL_DATA.contains(&upper.as_str()) || upper.starts_with("CUBE") {
        return Some(why::EXTERNAL_DATA);
    }
    if defined.contains(&upper) {
        return None;
    }
    if XLM.contains(&upper.as_str()) || ["GET.", "SET.", "FORMULA."].iter().any(|p| upper.starts_with(p)) {
        return Some(why::XLM);
    }
    // `_xludf.NAME` marks a user-defined function, whatever else `NAME` is. When Excel has a
    // function of that name the file can be read as that function, so it is still graded.
    if let Some(bare) = upper.trim_start_matches('_').strip_prefix("XLUDF.") {
        return (!EXCEL_FUNCTIONS.contains(bare)).then_some(why::UDF);
    }
    if gridcraft_engine::calc::is_known_function(n) || EXCEL_FUNCTIONS.contains(upper.as_str()) {
        return None;
    }
    Some(why::UDF)
}

/// The upper-cased variables `LET` and `LAMBDA` bind in `e`.
fn bound_names(e: &Expr) -> HashSet<String> {
    let mut out = HashSet::new();
    e.walk(&mut |x| {
        let Expr::Call(n, args) = x else { return };
        let vars: Vec<&Expr> = match n.as_str() {
            // LET(name, value, [name, value…], result)
            "LET" => args.iter().enumerate().filter(|(i, _)| i % 2 == 0 && i + 1 < args.len()).map(|(_, a)| a).collect(),
            // LAMBDA([parameter…], body)
            "LAMBDA" => args.iter().take(args.len().saturating_sub(1)).collect(),
            _ => return,
        };
        for v in vars {
            if let Expr::Name(name) = v {
                out.insert(name.to_ascii_uppercase());
            }
        }
    });
    out
}
