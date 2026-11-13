use super::calc::formula_text;
use super::package::validate_package;
use super::rels::{lost_relationships, relationship_tree};
use super::{
    BTreeMap, BTreeSet, Category, Cell, CellError, CellRef, Diffs, HashMap, Options, RoundTrip, Sheet, Value, Workbook, number_text, quote_sheet,
};

// ---------------------------------------------------------------- round trip

/// Writes `wb` as an `.xlsx` with GridCraft's XLSX writer, validates the package against itself and
/// against `source` (the file `wb` was read from), reads it back and compares. [`round_trip_owned`]
/// takes the name to save under.
#[cfg(test)]
pub fn round_trip(wb: &Workbook, source: &[u8], opts: &Options) -> RoundTrip {
    round_trip_owned(wb.clone(), source, "roundtrip.xlsx", opts)
}

/// The name the round trip saves under: the checked file's own extension when it names an XLSX
/// package kind (`.xlsm` stays `.xlsm`, so the VBA project is expected to come back), else `.xlsx`.
pub(super) fn output_name(path: &str) -> &'static str {
    let ext = std::path::Path::new(path).extension().and_then(std::ffi::OsStr::to_str).unwrap_or_default().to_ascii_lowercase();
    match ext.as_str() {
        "xlsm" => "roundtrip.xlsm",
        "xltx" => "roundtrip.xltx",
        "xltm" => "roundtrip.xltm",
        _ => "roundtrip.xlsx",
    }
}

/// Like [`round_trip`], consuming `wb` into a compact per-sheet snapshot and freeing all cell
/// B-trees, formula ASTs and preserved parts before reading back the written file.
pub(super) fn round_trip_owned(wb: Workbook, source: &[u8], name: &str, opts: &Options) -> RoundTrip {
    let bytes = match gridcraft_engine::io::save_bytes(&wb, name) {
        Ok(b) => b,
        Err(e) => return RoundTrip { error: Some(format!("write failed: {e}")), categories: vec![] },
    };
    let package = validate_round_trip_package(source, &bytes, opts);
    let snap = WorkbookSnap::from_workbook(wb);
    let back = match gridcraft_engine::io::open_bytes(name, &bytes) {
        Ok((b, _)) => b,
        Err(e) => return RoundTrip { error: Some(format!("reading the written file failed: {e}")), categories: vec![package.done()] },
    };
    drop(bytes);
    let mut categories = diff_snap_with_workbook(snap, back, opts);
    categories.push(package.done());
    RoundTrip { error: None, categories }
}

fn validate_round_trip_package(source: &[u8], bytes: &[u8], opts: &Options) -> Diffs {
    let mut package = Diffs::new("package", opts.max_report);
    match validate_package(bytes) {
        Ok(issues) => issues.into_iter().for_each(|i| package.push(|| i)),
        Err(e) => package.push(|| e),
    }
    // Relationships compare only between two SpreadsheetML packages: an XLSB's binary parts and
    // an XLS or ODS file have no counterpart in the XLSX written from them part by part.
    if spreadsheetml_package(source) {
        match (relationship_tree(source, opts.relationship_limits), relationship_tree(bytes, opts.relationship_limits)) {
            (Ok(before), Ok(after)) => {
                for lost in lost_relationships(&before, &after) {
                    package.push(|| lost);
                }
            }
            // A check that stopped at a limit listed only the relationships it reached, so it
            // can neither find a loss nor rule one out: that is a difference, not a pass.
            (before, after) => {
                for (whose, tree) in [("the checked file", before), ("the written file", after)] {
                    if let Err(why) = tree {
                        package.push(|| format!("relationships of {whose} not compared: {why}"));
                    }
                }
            }
        }
    }
    package
}

/// Whether `bytes` is a package whose workbook part is SpreadsheetML XML (XLSX and its macro and
/// template kinds), not XLSB's binary `workbook.bin`.
fn spreadsheetml_package(bytes: &[u8]) -> bool {
    let Some(mut zip) = super::package::zip_package(bytes) else { return false };
    let names = super::package::zip_names(&mut zip);
    super::package::workbook_entry(&mut zip, &names).is_some_and(|w| names.contains(&w) && !w.to_ascii_lowercase().ends_with(".bin"))
}

pub(super) fn value_show(v: &Value) -> String {
    match v {
        Value::Empty => "(blank)".to_string(),
        Value::Text(t) => serde_json::to_string(t.as_ref()).unwrap_or_default(),
        Value::Number(n) => number_text(*n),
        Value::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_string(),
        Value::Error(e) => e.as_str().to_string(),
        Value::Array(_) => v.scalar().display(),
    }
}

fn opt_formula(c: Option<&Cell>) -> String {
    c.filter(|c| c.formula.is_some()).map(formula_text).unwrap_or_default()
}

/// Per-sheet comparison of two lists, reporting what one side has and the other lacks.
fn diff_sets(d: &mut Diffs, sheet: &str, a: &BTreeSet<String>, b: &BTreeSet<String>) {
    for x in a.difference(b) {
        d.push(|| format!("{sheet}: lost {x}"));
    }
    for x in b.difference(a) {
        d.push(|| format!("{sheet}: gained {x}"));
    }
}

/// A sheet's conditional-format rules in precedence order, as comparable text. The priority number
/// itself is left out, so renumbering without changing the order isn't a difference.
fn cond_format_list(s: &Sheet) -> Vec<String> {
    let mut cfs: Vec<_> = s.cond_formats.iter().collect();
    cfs.sort_by_key(|c| c.priority);
    cfs.iter().map(|c| format!("{:?} {:?} stopIfTrue={}", c.ranges, c.rule, c.stop_if_true)).collect()
}

fn diff_count(d: &mut Diffs, sheet: &str, a: usize, b: usize) {
    if a != b {
        d.push(|| format!("{sheet}: {a} → {b}"));
    }
}

/// The header cells of a sheet's tables with the column name each holds. Excel requires the header
/// cell of a table column to hold that column's name as text, so a column whose header cell is
/// blank in the file comes back with its name in the cell. The writer makes that repair on
/// purpose; it isn't a difference (see [`is_header_repair`]).
fn table_headers(sheet: &Sheet) -> BTreeMap<CellRef, &str> {
    let mut out = BTreeMap::new();
    for t in sheet.tables.iter().filter(|t| t.header_row) {
        for (i, col) in t.columns.iter().enumerate() {
            if let Some(c) = u32::try_from(i).ok().and_then(|i| t.range.start.col.checked_add(i)) {
                out.insert(CellRef::new(t.range.start.row, c), col.name.as_str());
            }
        }
    }
    out
}

/// Whether the cell at `c`, blank before (`before`) and holding `after` now, is a table header the
/// writer filled with its column name.
fn is_header_repair(headers: &BTreeMap<CellRef, &str>, c: CellRef, before: &Value, after: &Value) -> bool {
    matches!((before, after), (Value::Empty, Value::Text(t)) if headers.get(&c).is_some_and(|name| *name == t.as_ref()))
}

/// Which sheet of the written workbook each sheet of the source came back as: the one with the
/// same name (sheet names compare without regard to case, as in Excel), then, among the sheets
/// left on both sides, the next in order, so that a renamed sheet still meets itself. A sheet that
/// is gone has no partner and doesn't shift the partners of the sheets after it.
fn pair_sheets(a: &[&str], b: &[&str]) -> Vec<Option<usize>> {
    let mut by_name: HashMap<String, usize> = HashMap::new();
    for (j, name) in b.iter().enumerate() {
        by_name.entry(name.to_lowercase()).or_insert(j);
    }
    let mut pairs: Vec<Option<usize>> = a.iter().map(|name| by_name.remove(&name.to_lowercase())).collect();
    let paired: BTreeSet<usize> = pairs.iter().flatten().copied().collect();
    let mut left = (0..b.len()).filter(|j| !paired.contains(j));
    for slot in pairs.iter_mut().filter(|p| p.is_none()) {
        *slot = left.next();
    }
    pairs
}

/// A value as the file format can hold it: `#SPILL!`/`#CALC!` are cached as `#VALUE!` and an
/// unresolved circular reference as 0 (what Excel writes), so those aren't round-trip losses.
fn as_cached(v: Value) -> Value {
    match v {
        Value::Error(CellError::Spill | CellError::Calc) => Value::Error(CellError::Value),
        Value::Error(CellError::Circ) => Value::Number(0.0),
        v => v,
    }
}

/// Feature-by-feature comparison of a workbook (`a`) with what reading it back gave (`b`). Sheets
/// are paired by position.
#[cfg(test)]
pub fn diff_workbooks(a: &Workbook, b: &Workbook, opts: &Options) -> Vec<Category> {
    diff_snap_with_workbook(WorkbookSnap::from_workbook(a.clone()), b.clone(), opts)
}

struct SheetSnap {
    raw_name: String,
    cells: Vec<(CellRef, Value, gridcraft_engine::model::StyleId, Option<Box<str>>)>,
    merges: BTreeSet<String>,
    tables: BTreeSet<String>,
    cond_formats: Vec<String>,
    validations: usize,
    charts: Vec<String>,
    comments: usize,
    hyperlinks: usize,
    pivots: usize,
}

struct WorkbookSnap {
    styles: gridcraft_engine::model::StyleTable,
    names: BTreeSet<String>,
    sheets: Vec<SheetSnap>,
}

fn workbook_name_set(wb: &Workbook) -> BTreeSet<String> {
    wb.names
        .iter()
        .map(|n| {
            let scope = n.scope.map(|i| format!("{}!", quote_sheet(wb.sheet(i).map(|s| s.name.as_str()).unwrap_or("?")))).unwrap_or_default();
            format!("{scope}{}={}", n.name, n.formula)
        })
        .collect()
}

fn sheet_merge_set(s: &Sheet) -> BTreeSet<String> {
    s.merges.iter().map(|r| r.a1()).collect()
}

fn sheet_table_set(s: &Sheet) -> BTreeSet<String> {
    s.tables
        .iter()
        .map(|t| format!("{} {} [{}]", t.name, t.range.a1(), t.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", ")))
        .collect()
}

fn sheet_chart_list(s: &Sheet) -> Vec<String> {
    s.charts.iter().map(|c| format!("{:?} ({} series)", c.kind, c.series.len())).collect()
}

impl WorkbookSnap {
    fn from_workbook(mut wb: Workbook) -> Self {
        let names = workbook_name_set(&wb);
        let styles = std::mem::take(&mut wb.styles);
        let raw_sheets = std::mem::take(&mut wb.sheets);
        drop(wb);
        let mut sheets = Vec::with_capacity(raw_sheets.len());
        for sh_arc in raw_sheets {
            let sh = std::sync::Arc::unwrap_or_clone(sh_arc);
            let merges = sheet_merge_set(&sh);
            let tables = sheet_table_set(&sh);
            let cond_formats = cond_format_list(&sh);
            let validations = sh.validations.len();
            let charts = sheet_chart_list(&sh);
            let comments = sh.comments.len();
            let hyperlinks = sh.hyperlinks.len();
            let pivots = sh.pivots.len();
            let raw_name = sh.name.clone();
            let mut cells = Vec::with_capacity(sh.cells.len());
            for (c, cell) in sh.cells.iter() {
                let f = cell.formula.is_some().then(|| formula_text(cell).into_boxed_str());
                cells.push((c, as_cached(cell.value.scalar()), cell.style, f));
            }
            drop(sh);
            sheets.push(SheetSnap { raw_name, cells, merges, tables, cond_formats, validations, charts, comments, hyperlinks, pivots });
        }
        Self { styles, names, sheets }
    }
}

fn diff_snap_with_workbook(a: WorkbookSnap, mut b: Workbook, opts: &Options) -> Vec<Category> {
    let m = opts.max_report;
    let mut sheets = Diffs::new("sheets", m);
    let (an, bn): (Vec<&str>, Vec<&str>) =
        (a.sheets.iter().map(|s| s.raw_name.as_str()).collect(), b.sheets.iter().map(|s| s.name.as_str()).collect());
    if an != bn {
        sheets.push(|| format!("{an:?} → {bn:?}"));
    }
    let pairs = pair_sheets(&an, &bn);
    let mut names = Diffs::new("names", m);
    diff_sets(&mut names, "workbook", &a.names, &workbook_name_set(&b));
    b.preserved = None;
    b.vba_project = None;
    let styles_a = a.styles;
    let styles_b = std::mem::take(&mut b.styles);
    let mut sheets_b: Vec<Option<std::sync::Arc<Sheet>>> = std::mem::take(&mut b.sheets).into_iter().map(Some).collect();
    drop(b);

    let mut values = Diffs::new("values", m);
    let mut formulas = Diffs::new("formulas", m);
    let mut num_fmts = Diffs::new("numFmts", m);
    let mut merges = Diffs::new("merges", m);
    let mut tables = Diffs::new("tables", m);
    let mut cond_formats = Diffs::new("condFormats", m);
    let mut validations = Diffs::new("validations", m);
    let mut charts = Diffs::new("charts", m);
    let mut comments = Diffs::new("comments", m);
    let mut hyperlinks = Diffs::new("hyperlinks", m);
    let mut pivots = Diffs::new("pivots", m);

    for (sa, partner) in a.sheets.into_iter().zip(pairs) {
        let Some(sb_arc) = partner.and_then(|j| sheets_b.get_mut(j)).and_then(Option::take) else { continue };
        let sb = std::sync::Arc::unwrap_or_clone(sb_arc);
        let headers = table_headers(&sb);
        let name = quote_sheet(&sa.raw_name);
        for &(c, ref va, style_a, ref fa) in &sa.cells {
            let cb = sb.cell(c);
            let vb = as_cached(cb.map(|x| x.value.scalar()).unwrap_or_default());
            if *va != vb && !is_header_repair(&headers, c, va, &vb) {
                values.push(|| format!("{name}!{}: {} → {}", c.a1(), value_show(va), value_show(&vb)));
            }
            let fa_str = fa.as_deref().unwrap_or("");
            if !fa_str.is_empty() || cb.is_some_and(|x| x.formula.is_some()) {
                let fb = opt_formula(cb);
                if fa_str != fb {
                    formulas.push(|| format!("{name}!{}: {fa_str} → {fb}", c.a1()));
                }
            }
            let na = styles_a.get(style_a).num_fmt.as_str();
            let nb = styles_b.get(cb.map(|x| x.style).unwrap_or_default()).num_fmt.as_str();
            if na != nb {
                num_fmts.push(|| format!("{name}!{}: {na} → {nb}", c.a1()));
            }
        }
        for (c, cb) in sb.cells.iter() {
            if sa.cells.binary_search_by_key(&c, |(pos, _, _, _)| *pos).is_err() {
                let va = Value::Empty;
                let vb = as_cached(cb.value.scalar());
                if va != vb && !is_header_repair(&headers, c, &va, &vb) {
                    values.push(|| format!("{name}!{}: {} → {}", c.a1(), value_show(&va), value_show(&vb)));
                }
                if cb.formula.is_some() {
                    let fb = opt_formula(Some(cb));
                    if !fb.is_empty() {
                        formulas.push(|| format!("{name}!{}:  → {fb}", c.a1()));
                    }
                }
                let na = styles_a.get(Default::default()).num_fmt.as_str();
                let nb = styles_b.get(cb.style).num_fmt.as_str();
                if na != nb {
                    num_fmts.push(|| format!("{name}!{}: {na} → {nb}", c.a1()));
                }
            }
        }
        diff_sets(&mut merges, &name, &sa.merges, &sheet_merge_set(&sb));
        diff_sets(&mut tables, &name, &sa.tables, &sheet_table_set(&sb));
        let cfb = cond_format_list(&sb);
        if sa.cond_formats.len() != cfb.len() {
            cond_formats.push(|| format!("{name}: {} → {}", sa.cond_formats.len(), cfb.len()));
        } else if let Some((x, y)) = sa.cond_formats.iter().zip(&cfb).find(|(x, y)| x != y) {
            cond_formats.push(|| format!("{name}: {x} → {y}"));
        }
        diff_count(&mut validations, &name, sa.validations, sb.validations.len());
        let cb_charts = sheet_chart_list(&sb);
        if sa.charts != cb_charts {
            charts.push(|| format!("{name}: {:?} → {:?}", sa.charts, cb_charts));
        }
        diff_count(&mut comments, &name, sa.comments, sb.comments.len());
        diff_count(&mut hyperlinks, &name, sa.hyperlinks, sb.hyperlinks.len());
        diff_count(&mut pivots, &name, sa.pivots, sb.pivots.len());
    }
    vec![
        sheets.done(),
        values.done(),
        formulas.done(),
        num_fmts.done(),
        merges.done(),
        names.done(),
        tables.done(),
        cond_formats.done(),
        validations.done(),
        charts.done(),
        comments.done(),
        hyperlinks.done(),
        pivots.done(),
    ]
}
