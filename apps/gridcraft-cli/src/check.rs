//! `check`: measures calculation correctness and XLSX fidelity on any set of workbooks.
//!
//! **Calc oracle.** A workbook saved by Excel (or another calculating producer) carries the
//! producer's own result next to every formula (`<v>` beside `<f>`). The file is read keeping
//! those cached values, recalculated from scratch by GridCraft, and every formula cell's new value
//! is compared with the cached one. Formulas without a cached value (files from writers that don't
//! calculate) count as *no oracle*; volatile formulas (`RAND`, `NOW`…) and the formulas that read
//! them are skipped. Only a cache the producer vouches for gates: the workbook's `<calcPr>` says
//! whether the values are the result of a completed calculation (see [`CacheTrust`]), and the
//! comparison against any other cache is reported in a bucket of its own.
//!
//! **Round trip.** The recalculated workbook is written with GridCraft's XLSX writer, the package is
//! validated (content types cover every part, every internal relationship target exists, no kind of
//! relationship the source had is missing) and read back, and the two workbooks are compared feature
//! by feature.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::{Cursor, Read as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use gridcraft_engine::calc::{Calc, Key};
use gridcraft_engine::cmd::inspect::value_json;
use gridcraft_engine::core::{CellError, CellRef, Value};
use gridcraft_engine::formula::{Expr, quote_sheet};
use gridcraft_engine::model::{Cell, Sheet, Workbook};
use serde_json::{Value as Json, json};

use super::{Args, USAGE, number_text, pretty};

mod calc;
mod fidelity;
mod foreign;
mod legacy;
mod locale;
mod noise;
mod package;
mod producer;
mod rels;
mod report;
mod source;
mod taint;
mod xls_meta;

#[cfg(test)]
use calc::{bool_cached_as_number, values_match};
use calc::{oracle, pin_volatile, snapshot_formula_values, unpin_volatile};
#[cfg(test)]
use fidelity::diff_workbooks;
#[cfg(test)]
pub use fidelity::round_trip;
use fidelity::{output_name, round_trip_owned};
use package::calc_props;
#[cfg(test)]
use package::validate_package;
pub use producer::{Producer, ProducerKind};
use report::print_human;
pub use report::report_json;

/// Version tag of the `--json` output; bumped when a field changes meaning or disappears.
/// `gridcraft-check/2`: `mismatched` counts only cells whose producer cache is trusted; cells
/// compared against a cache that is not (see [`CacheTrust`]) are reported apart.
pub const SCHEMA: &str = "gridcraft-check/2";
/// Directory recursion limit (symlink loops end here).
const MAX_DEPTH: usize = 32;
/// Most files collected from the inputs.
const MAX_FILES: usize = 100_000;

#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// Relative tolerance for numbers (absolute below magnitude 1).
    pub tolerance: f64,
    /// Most examples listed per file and category.
    pub max_report: usize,
    /// `--cells`: list every formula cell with the bucket it was counted in (JSON `calc.cells`).
    pub cells: bool,
    /// What comparing the relationships of one package may spend before it reports that it did not
    /// finish. The command line doesn't change them; the tests lower them.
    pub relationship_limits: rels::Limits,
}

impl Default for Options {
    fn default() -> Self {
        Options { tolerance: 1e-9, max_report: 20, cells: false, relationship_limits: rels::Limits::default() }
    }
}

// ---------------------------------------------------------------- reports

/// Per-function counts: formula cells checked that call the function, how many of them mismatch,
/// and how many mismatch while none of their precedents do (*primary*: the likely culprit).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FnStat {
    pub checked: usize,
    pub mismatched: usize,
    pub primary: usize,
}

impl FnStat {
    fn add(&mut self, o: &FnStat) {
        self.checked += o.checked;
        self.mismatched += o.mismatched;
        self.primary += o.primary;
    }
}

/// How a workbook asks to be calculated (`calcMode` of `<calcPr>`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CalcMode {
    #[default]
    Auto,
    AutoNoTable,
    Manual,
}

impl CalcMode {
    pub fn as_str(self) -> &'static str {
        match self {
            CalcMode::Auto => "auto",
            CalcMode::AutoNoTable => "autoNoTable",
            CalcMode::Manual => "manual",
        }
    }
}

/// The calculation settings a producer saved in the workbook part's `<calcPr>`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CalcProps {
    pub mode: CalcMode,
    /// `calcCompleted`: whether the producer finished calculating before it saved. `None` when
    /// the attribute is absent, which the format reads as completed.
    pub completed: Option<bool>,
    pub full_calc_on_load: bool,
    pub iterate: bool,
    pub calc_id: Option<u32>,
}

/// How far the values a producer cached next to its formulas can be taken for the results of a
/// completed calculation, from what the file itself says (see [`CacheTrust::of`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CacheTrust {
    /// The producer says nothing against its cache: a mismatch is GridCraft's, and gates.
    #[default]
    Trusted,
    /// The producer asked for a full recalculation on load (`fullCalcOnLoad`): what it cached is
    /// whatever it had (libraries that don't calculate store 0 or nothing), not the result of a
    /// completed calculation. Compared, but not gating.
    FullCalcOnLoad,
    /// The producer says it did not finish calculating before it saved (`calcCompleted="0"`, in
    /// practice manual-mode workbooks saved with changes pending): the cached values were never
    /// brought up to date. Compared, but not gating.
    StaleCache,
    /// The file names a product other than Excel as its producer (LibreOffice, Google Sheets, WPS,
    /// a library…): the cached values are that product's results, and where its semantics differ
    /// from Excel's (an empty cell read as `""`, dates joined to text as dates…) they say nothing
    /// about what Excel would show. Compared, but not gating.
    ForeignProducer,
    /// One cell of a trusted manual-calculation workbook whose cache is inconsistent with the
    /// file's own cached inputs: its formula, IEEE 754 arithmetic on numbers only, evaluated on the
    /// values the file saved for every cell it reads, gives GridCraft's result and not the cache. Excel in manual mode only
    /// recalculates cells it marked dirty, so such a value survives a save; a full recalculation
    /// in Excel replaces it. Never a file's trust ([`CacheTrust::of`] doesn't return it): only a
    /// [`Mismatch::class`]. Listed, not gating (see [`calc`]'s precedent-local oracle).
    StaleCell,
    /// One cell of a trusted cache that an older Excel calculated with what it had: a function
    /// Excel 2003 or Excel 2010 made more accurate, within the old algorithm's accuracy, or `#NAME?` for a
    /// function the calculating version lacked; and the cells whose only mismatching inputs are
    /// such cells. Excel's answer for its own version, not the current one's. Only a
    /// [`Mismatch::class`] (see [`legacy`]). Listed, not gating.
    LegacyExcelVersion,
    /// One cell of a trusted cache on a circular reference, or computed from one, in a workbook
    /// with iterative calculation off. Microsoft ("Remove or allow a circular reference"): with
    /// `=D1+D2+D3` in D3 "Excel can't finish the calculation because D3 depends on D3"; what the
    /// loop's cells and their readers cached is what they held when Excel gave up, which depends
    /// on the order it took them in. Only a [`Mismatch::class`] (see [`calc`]'s loop oracle).
    /// Listed, not gating.
    CircularReference,
}

impl CacheTrust {
    /// A producer that says it never finished calculating is stale whatever else it says; one
    /// that asks for a full recalculation on load has no completed calculation behind its cache;
    /// a product that isn't Excel gives results that need not be Excel's, calculated or not.
    /// Manual mode alone is trusted: Excel recalculates before saving unless told not to, and
    /// `calcCompleted` is where it records that it did not.
    pub fn of(p: &CalcProps, producer: &Producer) -> CacheTrust {
        if p.completed == Some(false) {
            CacheTrust::StaleCache
        } else if p.full_calc_on_load {
            CacheTrust::FullCalcOnLoad
        } else if producer.kind.is_foreign() {
            CacheTrust::ForeignProducer
        } else {
            CacheTrust::Trusted
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            CacheTrust::Trusted => "trusted",
            CacheTrust::FullCalcOnLoad => "fullCalcOnLoad",
            CacheTrust::StaleCache => "staleCache",
            CacheTrust::ForeignProducer => "foreignProducer",
            CacheTrust::StaleCell => "staleCell",
            CacheTrust::LegacyExcelVersion => "legacyExcelVersion",
            CacheTrust::CircularReference => "circularReference",
        }
    }

    /// Why the cache of a file isn't trusted, for the human report.
    pub fn reason(self) -> &'static str {
        match self {
            CacheTrust::Trusted => "the producer calculated before it saved",
            CacheTrust::FullCalcOnLoad => {
                "the producer asked for a full recalculation on load, so its cached values are not the result of a completed calculation"
            }
            CacheTrust::StaleCache => {
                "the producer did not finish calculating before it saved (calcCompleted=\"0\", or an XLS Uncalced record): its cached values were not brought up to date"
            }
            CacheTrust::ForeignProducer => {
                "a product other than Excel wrote the file, so its cached values are that product's results, which need not be Excel's"
            }
            CacheTrust::StaleCell => {
                "in manual calculation mode the cache of this cell disagrees with the values the file saved for the cells it reads"
            }
            CacheTrust::LegacyExcelVersion => legacy::reason(),
            CacheTrust::CircularReference => {
                "the cell is on a circular reference, or reads one, and iterative calculation is off: Excel can't finish such a calculation, so the cache holds what the cell had when Excel stopped, which depends on the order it calculated the loop in"
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Mismatch {
    /// `Sheet1!B2`.
    pub cell: String,
    /// `=SUM(A1:A3)`, `{=…}` for a legacy array formula.
    pub formula: String,
    pub expected: Value,
    pub actual: Value,
    /// `numeric`, `text`, `bool`, `error`, `blank` (GridCraft produced no value) or `type`.
    pub kind: &'static str,
    pub functions: Vec<String>,
    pub primary: bool,
    /// The trust in the cache this mismatch was found against; anything but `Trusted` doesn't gate.
    pub class: CacheTrust,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CalcReport {
    pub formula_cells: usize,
    /// Cells compared against a trusted cache: `checked = matched + mismatched + stale_cell +
    /// legacy_excel_version + circular_reference`.
    pub checked: usize,
    pub matched: usize,
    /// Mismatches against a trusted cache: the ones that gate.
    pub mismatched: usize,
    /// Mismatches under en-US that the cache's locale ([`CalcReport::locale`]) reproduces: counted
    /// in `matched` too.
    pub locale_dependent: usize,
    /// The locale the trusted cache was computed under, when re-evaluating under it reproduces
    /// mismatches and keeps every match (see [`locale`]); `None` for en-US.
    pub locale: Option<&'static str>,
    /// Formulas with nothing to compare against, per `no_oracle_reasons`: saved without a value, or
    /// built on something Excel couldn't compute from the file alone.
    pub no_oracle: usize,
    /// `no_oracle` by why: `noCachedValue`, and for [`Source::Foreign`](taint::Source) the formula's
    /// own reason (`addin`, `xlm`, `udf`, `externalData`) or `downstream` for one that
    /// only reads such a cell.
    pub no_oracle_reasons: BTreeMap<&'static str, usize>,
    /// What the file says about its cache; anything but `Trusted` moves the comparisons of this
    /// file from `checked` to the `untrusted…` counts.
    pub cache_trust: CacheTrust,
    /// The `<calcPr>` the trust was read from.
    pub calc_props: CalcProps,
    /// Cells compared against an untrusted cache that agree with GridCraft.
    pub untrusted_matched: usize,
    /// Cells compared against an untrusted cache that differ from GridCraft: still listed, not gating.
    pub untrusted_mismatched: usize,
    /// Of the trusted comparisons, mismatches whose cache is inconsistent with the file's own
    /// cached inputs in a manual-calculation workbook ([`CacheTrust::StaleCell`]): listed, not
    /// counted in `mismatched`, not gating.
    pub stale_cell: usize,
    /// Of the trusted comparisons, mismatches that the version of Excel which calculated the cache
    /// explains ([`CacheTrust::LegacyExcelVersion`]): listed, not counted in `mismatched`, not gating.
    pub legacy_excel_version: usize,
    /// Of the trusted comparisons, mismatches on or downstream of a circular reference with
    /// iterative calculation off ([`CacheTrust::CircularReference`]): listed, not counted in
    /// `mismatched`, not gating.
    pub circular_reference: usize,
    /// Formulas whose value depends on the clock, a random generator or the environment, directly
    /// or through anything they read, defined names included: not compared.
    pub volatile: usize,
    /// Of `volatile`, the formulas that reach the volatile value through a defined name (the name
    /// itself calls a volatile function, or refers to a cell that depends on one).
    pub volatile_via_names: usize,
    /// Matched cells whose producer saved GridCraft's TRUE or FALSE as the number 1 or 0 (no
    /// `t="b"`, as Business Central's Excel exports do); counted in `matched` (or
    /// `untrusted_matched`) too.
    pub bool_as_number: usize,
    /// Matched cells of a Google Sheets workbook whose cache is an error as Google Sheets writes it,
    /// `#N/A` or `#N/A:lookupNotFound:…` text (the code is what the cell shows); counted in
    /// `matched` (or `untrusted_matched`) too.
    pub error_detail_cache: usize,
    /// Matched text cells of a workbook a writer other than Excel saved whose cache is GridCraft's
    /// text without the whitespace around it, written without `xml:space="preserve"`; counted in
    /// `matched` (or `untrusted_matched`) too.
    pub whitespace_cache: usize,
    /// Matched numbers within an iterative solver's accuracy ([`SOLVER_TOLERANCE`]) carried through
    /// the formula (see `calc::settle_undecided`); counted in `matched` too.
    pub solver_tolerance: usize,
    /// Matched numbers within the rounding the formula's operations propagate from what it reads
    /// (see [`noise`]); counted in `matched` too.
    pub cancellation_residue: usize,
    /// Matched numbers on an iterative loop that Excel's stopping rule explains (see [`noise`]);
    /// counted in `matched` too.
    pub iteration_change: usize,
    /// The first `max_report` mismatches (of the file's trusted or untrusted comparison), primary
    /// ones first.
    pub mismatches: Vec<Mismatch>,
    pub functions: BTreeMap<String, FnStat>,
    /// Mismatch counts by `expected type → actual type` (an error counts as its code), e.g.
    /// `#VALUE! → #SPILL!`: one signature often covers a single root cause across functions.
    pub signatures: BTreeMap<String, usize>,
    /// The same for the mismatches against an untrusted cache.
    pub untrusted_signatures: BTreeMap<String, usize>,
    /// With `--cells`, every formula cell (`Sheet1!B2`) and its bucket: `matched`,
    /// `matched_alt:<grade>`, `calc_mismatch`, `stale_cell`, `untrusted_match:<trust>`,
    /// `untrusted_mismatch:<trust>`, `no_oracle:<why>` or `volatile`.
    pub cells: Vec<(String, String)>,
}

/// One round-trip comparison: how many differences, and the first few.
#[derive(Clone, Debug, PartialEq)]
pub struct Category {
    pub name: &'static str,
    pub diffs: usize,
    pub examples: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RoundTrip {
    /// Writing or reading back failed (the categories are then empty).
    pub error: Option<String>,
    pub categories: Vec<Category>,
}

impl RoundTrip {
    pub fn differing(&self) -> Vec<&'static str> {
        self.categories.iter().filter(|c| c.diffs > 0).map(|c| c.name).collect()
    }
    fn is_clean(&self) -> bool {
        self.error.is_none() && self.categories.iter().all(|c| c.diffs == 0)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FileReport {
    pub path: String,
    /// Opening the file or discovering the requested inputs failed: its message.
    pub error: Option<String>,
    /// Import warnings, in the order the importer produced them.
    pub warnings: Vec<String>,
    /// The application the package says wrote it (see [`producer`]).
    pub producer: Producer,
    pub calc: CalcReport,
    pub round_trip: RoundTrip,
}

impl FileReport {
    fn is_clean(&self) -> bool {
        self.error.is_none() && self.calc.mismatched == 0 && self.round_trip.is_clean()
    }
}

/// Collects the first `max` examples while counting all differences.
struct Diffs {
    name: &'static str,
    count: usize,
    max: usize,
    examples: Vec<String>,
}

impl Diffs {
    fn new(name: &'static str, max: usize) -> Diffs {
        Diffs { name, count: 0, max, examples: vec![] }
    }
    fn push(&mut self, f: impl FnOnce() -> String) {
        self.count = self.count.saturating_add(1);
        if self.examples.len() < self.max {
            self.examples.push(f());
        }
    }
    fn done(self) -> Category {
        Category { name: self.name, diffs: self.count, examples: self.examples }
    }
}

// ---------------------------------------------------------------- entry point

/// `check <file-or-dir>... [--json] [--tolerance X] [--max-report N]`. Exit 0 when every file opens,
/// every checked formula matches and every round trip is clean; 1 otherwise; 2 on a usage error.
pub fn main(args: &[String]) -> ExitCode {
    let usage = |e: String| {
        eprintln!("gridcraft-cli: {e}\n{USAGE}");
        ExitCode::from(2)
    };
    let a = match Args::parse(args, &["tolerance", "max-report"], &["json", "cells"]) {
        Ok(a) => a,
        Err(e) => return usage(e),
    };
    if a.positional.is_empty() {
        return usage("missing <file-or-dir>".into());
    }
    let mut opts = Options::default();
    if let Some(t) = a.opt("tolerance") {
        match t.parse::<f64>() {
            Ok(t) if t.is_finite() && t >= 0.0 => opts.tolerance = t,
            _ => return usage(format!("--tolerance needs a number ≥ 0, not `{t}`")),
        }
    }
    if let Some(n) = a.opt("max-report") {
        match n.parse::<usize>() {
            Ok(n) => opts.max_report = n,
            Err(_) => return usage(format!("--max-report needs a whole number, not `{n}`")),
        }
    }
    opts.cells = a.has("cells");
    let Discovery { files, failures: mut reports } = collect_inputs(&a.positional);
    reports.extend(files.iter().map(|p| {
        let path = p.to_string_lossy().into_owned();
        match gridcraft_engine::io::read_file(&path) {
            Ok(bytes) => check_bytes(&path, &bytes, &opts),
            Err(e) => FileReport { path, error: Some(e.to_string()), ..Default::default() },
        }
    }));
    if a.has("json") {
        outln!("{}", pretty(&report_json(&reports, &opts)));
    } else {
        print_human(&reports, &opts);
    }
    if !files.is_empty() && reports.iter().all(FileReport::is_clean) { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

#[derive(Default)]
struct Discovery {
    files: Vec<PathBuf>,
    failures: Vec<FileReport>,
}

impl Discovery {
    fn fail(&mut self, path: &Path, error: String) {
        self.failures.push(FileReport { path: path.to_string_lossy().into_owned(), error: Some(error), ..Default::default() });
    }

    /// False means the file budget is exhausted: the remaining scan is incomplete.
    fn add_file(&mut self, path: PathBuf) -> bool {
        if self.files.len() >= MAX_FILES {
            self.fail(&path, format!("file limit of {MAX_FILES} reached; this and remaining inputs were not checked"));
            return false;
        }
        self.files.push(path);
        true
    }
}

/// Files named on the command line are checked whatever their extension; directories contribute
/// their `.xlsx`/`.xlsm` files, recursively, in name order. Hidden files and Excel lock files
/// (`~$…`) are skipped.
fn collect_inputs(inputs: &[String]) -> Discovery {
    let mut out = Discovery::default();
    let mut seen = std::collections::HashSet::new();
    for i in inputs {
        let p = PathBuf::from(i);
        if p.is_dir() {
            if !walk(&p, 0, &mut out, &mut seen) {
                break;
            }
        } else if !out.add_file(p) {
            break;
        }
    }
    if out.files.is_empty()
        && out.failures.is_empty()
        && let Some(input) = inputs.first()
    {
        out.fail(Path::new(input), "no workbook files were found in the requested inputs".into());
    }
    out
}

/// The workbook formats a directory scan checks: every spreadsheet format GridCraft reads with
/// its formulas and cached values (not text formats, which cache nothing).
const CHECKED_EXTENSIONS: [&str; 6] = ["xlsx", "xlsm", "xls", "xlsb", "ods", "fods"];

/// Returns false only when the global file budget stops traversal.
fn walk(dir: &Path, depth: usize, out: &mut Discovery, seen: &mut std::collections::HashSet<u64>) -> bool {
    // A directory is visited once by identity (dev+inode on unix): several links to the same
    // directory are aliases, not exponential branches of the scan. Other platforms keep the
    // depth cap only.
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        if let Ok(m) = std::fs::metadata(dir)
            && let Some(id) = m.dev().checked_mul(1 << 60).and_then(|d| d.checked_add(m.ino()))
            && !seen.insert(id)
        {
            return true;
        }
    }
    if depth > MAX_DEPTH {
        out.fail(dir, format!("directory depth limit of {MAX_DEPTH} exceeded; this subtree was not checked"));
        return true;
    }
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) => {
            out.fail(dir, format!("cannot read directory: {e}"));
            return true;
        }
    };
    let mut entries = Vec::new();
    for entry in rd {
        match entry {
            Ok(entry) => entries.push(entry.path()),
            Err(e) => out.fail(dir, format!("cannot read directory entry: {e}")),
        }
    }
    entries.sort();
    for p in entries {
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if name.starts_with('.') || name.starts_with("~$") {
            continue;
        }
        let metadata = match p.metadata() {
            Ok(metadata) => metadata,
            Err(e) => {
                out.fail(&p, format!("cannot inspect input: {e}"));
                continue;
            }
        };
        if metadata.is_dir() {
            if !walk(&p, depth + 1, out, seen) {
                return false;
            }
        } else if p.extension().and_then(std::ffi::OsStr::to_str).is_some_and(|e| CHECKED_EXTENSIONS.iter().any(|x| e.eq_ignore_ascii_case(x)))
            && !out.add_file(p)
        {
            return false;
        }
    }
    true
}

/// Checks one file's bytes. Never fails: problems are recorded in the report. A panic escaping
/// GridCraft's reader, calculator or writer is caught and reported as an internal error, so one
/// bad file doesn't end a corpus run.
pub fn check_bytes(path: &str, bytes: &[u8], opts: &Options) -> FileReport {
    let guarded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check_inner(path, bytes, opts)));
    guarded.unwrap_or_else(|p| {
        let msg = p.downcast_ref::<&str>().map(ToString::to_string).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_default();
        FileReport { path: path.to_string(), error: Some(format!("internal error while checking: {msg}")), ..Default::default() }
    })
}

fn check_inner(path: &str, bytes: &[u8], opts: &Options) -> FileReport {
    let xls = xls_meta::read(bytes);
    let producer = xls.as_ref().map_or_else(|| producer::detect(bytes), |x| x.producer.clone());
    let mut report = FileReport { path: path.to_string(), producer, ..Default::default() };
    let (mut wb, warnings) = match gridcraft_engine::io::open_bytes(path, bytes) {
        Ok(x) => x,
        Err(e) => {
            report.error = Some(e.to_string());
            return report;
        }
    };
    report.warnings = warnings;
    let saved = snapshot_formula_values(&wb);
    let pinned = pin_volatile(&mut wb, &saved);
    let mut calc = Calc::new();
    calc.recalc_all(&mut wb);
    let pinned = unpin_volatile(&mut wb, pinned);
    let mut props = calc_props(bytes);
    // An XLS says it saved with formulas pending recalculation in an `Uncalced` record.
    if xls.is_some_and(|x| x.uncalced) {
        props.completed = Some(false);
    }
    // Manual mode as the file's `<calcPr>` says, or as the reader found it in a file without one
    // (an XLS `CALCMODE` record, an XLSB or ODS setting).
    let manual = props.mode == CalcMode::Manual || wb.calc.mode == gridcraft_engine::model::CalcMode::Manual;
    let cache = calc::CacheFacts {
        trust: CacheTrust::of(&props, &report.producer),
        manual,
        excel_version: legacy::excel_version(&report.producer, &props),
        producer: report.producer.kind,
    };
    // The locale pass starts every locale from the workbook as read: read once, and only when a
    // mismatch needs the pass.
    let mut opened: Option<Option<Workbook>> = None;
    let mut relocalized = |loc| {
        let base = opened.get_or_insert_with(|| gridcraft_engine::io::open_bytes(path, bytes).ok().map(|(wb, _)| wb));
        base.as_ref().map(|base| locale::recalculated_under(base, &saved, loc))
    };
    report.calc = oracle(&saved, &wb, &calc, cache, &pinned, opts, bytes, &mut relocalized);
    drop(opened);
    report.calc.calc_props = props;
    drop(saved);
    report.round_trip = round_trip_owned(wb, bytes, output_name(path), opts);
    report
}

#[cfg(test)]
mod tests;
