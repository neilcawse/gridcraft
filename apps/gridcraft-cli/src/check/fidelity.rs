use super::calc::formula_text;
use super::package::{REBUILT_KINDS, relationship_kinds, validate_package};
use super::{BTreeSet, Category, Cell, CellError, CellRef, Diffs, Options, RoundTrip, Sheet, Value, Workbook, number_text, quote_sheet};

// ---------------------------------------------------------------- round trip

/// Writes `wb` with GridCraft's XLSX writer, validates the package against itself and against
/// `source` (the file `wb` was read from), reads it back and compares.
#[cfg(test)]
pub fn round_trip(wb: &Workbook, source: &[u8], opts: &Options) -> RoundTrip {
    round_trip_owned(wb.clone(), source, opts)
}

/// Like [`round_trip`], consuming `wb` into a compact per-sheet snapshot and freeing all cell
/// B-trees, formula ASTs and preserved parts before reading back the written file.
pub(super) fn round_trip_owned(wb: Workbook, source: &[u8], opts: &Options) -> RoundTrip {
    let bytes = match gridcraft_engine::io::save_bytes(&wb, "roundtrip.xlsx") {
        Ok(b) => b,
        Err(e) => return RoundTrip { error: Some(format!("write failed: {e}")), categories: vec![] },
    };
    let package = validate_round_trip_package(source, &bytes, opts.max_report);
    let snap = WorkbookSnap::from_workbook(wb);
    let back = match gridcraft_engine::io::open_bytes("roundtrip.xlsx", &bytes) {
        Ok((b, _)) => b,
        Err(e) => return RoundTrip { error: Some(format!("reading the written file failed: {e}")), categories: vec![package.done()] },
    };
    drop(bytes);
    let mut categories = diff_snap_with_workbook(snap, back, opts);
    categories.push(package.done());
    RoundTrip { error: None, categories }
}

fn validate_round_trip_package(source: &[u8], bytes: &[u8], max_report: usize) -> Diffs {
    let mut package = Diffs::new("package", max_report);
    match validate_package(bytes) {
        Ok(issues) => issues.into_iter().for_each(|i| package.push(|| i)),
        Err(e) => package.push(|| e),
    }
    if let (Ok(before), Ok(after)) = (relationship_kinds(source, false), relationship_kinds(bytes, true)) {
        for (key, kind) in &before {
            if !after.contains_key(key) && !REBUILT_KINDS.contains(&key.as_str()) {
                package.push(|| format!("lost relationship {kind}"));
            }
        }
    }
    package
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

/// The sheet's PivotTables: those modelled, and those kept unchanged for saving (their source is
/// in another workbook, a data connection or the Data Model).
fn pivot_count(s: &Sheet) -> usize {
    let kept = s.preserved.as_deref().map_or(0, |p| p.rels.iter().filter(|r| !r.external && r.rel_type.ends_with("/pivotTable")).count());
    s.pivots.len() + kept
}

fn diff_count(d: &mut Diffs, sheet: &str, a: usize, b: usize) {
    if a != b {
        d.push(|| format!("{sheet}: {a} → {b}"));
    }
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
    cells: Vec<(CellRef, Value, gridcraft_engine::model::StyleId, Option<std::sync::Arc<gridcraft_engine::model::Formula>>)>,
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
            let pivots = pivot_count(&sh);
            let raw_name = sh.name.clone();
            let mut cells = Vec::with_capacity(sh.cells.len());
            for (c, cell) in sh.cells.iter() {
                cells.push((c, as_cached(cell.value.scalar()), cell.style, cell.formula.clone()));
            }
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
    let mut names = Diffs::new("names", m);
    diff_sets(&mut names, "workbook", &a.names, &workbook_name_set(&b));
    b.preserved = None;
    b.vba_project = None;
    let styles_a = a.styles;
    let styles_b = std::mem::take(&mut b.styles);
    let sheets_b = std::mem::take(&mut b.sheets);
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

    let mut pair_cache: std::collections::HashMap<(usize, usize, i32, i32), bool> = std::collections::HashMap::new();
    for (sa, sb_arc) in a.sheets.into_iter().zip(sheets_b) {
        let sb = std::sync::Arc::unwrap_or_clone(sb_arc);
        let name = quote_sheet(&sa.raw_name);
        for &(c, ref va, style_a, ref fa) in &sa.cells {
            let cb = sb.cell(c);
            let vb = as_cached(cb.map(|x| x.value.scalar()).unwrap_or_default());
            if *va != vb {
                values.push(|| format!("{name}!{}: {} → {}", c.a1(), value_show(va), value_show(&vb)));
            }
            let fb = cb.and_then(|x| x.formula.as_ref());
            if fa.is_some() || fb.is_some() {
                let same = match (fa, fb) {
                    (Some(a_f), Some(b_f)) if a_f.array == b_f.array => {
                        if let (Some((ba, dra, dca)), Some((bb, drb, dcb))) = (a_f.expr_shifted(), b_f.expr_shifted()) {
                            let key = (
                                std::sync::Arc::as_ptr(ba) as usize,
                                std::sync::Arc::as_ptr(bb) as usize,
                                dra.wrapping_sub(drb),
                                dca.wrapping_sub(dcb),
                            );
                            *pair_cache.entry(key).or_insert_with(|| a_f.text() == b_f.text())
                        } else {
                            a_f.text() == b_f.text()
                        }
                    }
                    _ => false,
                };
                if !same {
                    let fa_str = fa
                        .as_ref()
                        .map(|f| if f.array.is_some() { format!("{{={}}}", f.text()) } else { format!("={}", f.text()) })
                        .unwrap_or_default();
                    let fb_str = opt_formula(cb);
                    if fa_str != fb_str {
                        formulas.push(|| format!("{name}!{}: {fa_str} → {fb_str}", c.a1()));
                    }
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
                if va != vb {
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
        diff_count(&mut pivots, &name, sa.pivots, pivot_count(&sb));
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
