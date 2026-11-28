//! A PivotTable source bound to a defined name goes through save and reopen bound to the same
//! definition: the source each save writes, the cache records it holds, and the report a native
//! reopen and refresh gives. Sheet `Report` (the PivotTable's) holds K/V with 901, 902 in A1:B3,
//! sheet `Data` holds K/V with 1, 2 in A1:B3; the grand total tells them apart (1803 or 3).

use std::io::Read;
use std::sync::Arc;

use gridcraft_core::{CellRef, RangeRef, Value};
use gridcraft_model::{Cell, DefinedName, Formula, PivotField, PivotTable, PivotValue, Sheet, Table, TableColumn, TotalsFn, Workbook};
use serde_json::json;

use crate::io::{FileKind, open_bytes, save_bytes_reported};

const REPORT: usize = 0;
const FROM_DATA: f64 = 3.0;
const FROM_REPORT: f64 = 1803.0;

fn name(n: &str, scope: Option<usize>, formula: &str) -> DefinedName {
    DefinedName { name: n.into(), scope, formula: formula.into(), comment: String::new(), hidden: false, uid: Default::default(), legacy_grid: false }
}

fn set_rows(sh: &mut Sheet, v: [f64; 2]) {
    for (i, (k, n)) in [("a", v[0]), ("b", v[1])].into_iter().enumerate() {
        let r = i as u32 + 1;
        sh.set_value(CellRef::new(r, 0), Value::text(k));
        sh.set_value(CellRef::new(r, 1), Value::Number(n));
    }
    sh.set_value(CellRef::new(0, 0), Value::text("K"));
    sh.set_value(CellRef::new(0, 1), Value::text("V"));
}

/// The workbook with workbook name `rData` (`Data!$A$1:$B$3`) and a PivotTable on Report whose
/// source is `source`, bound when `bind` (as a file's would be), refreshed.
fn book(source: &str, bind: bool) -> Workbook {
    let mut wb = Workbook::new();
    let report = Arc::make_mut(&mut wb.sheets[0]);
    report.name = "Report".into();
    set_rows(report, [901.0, 902.0]);
    let mut data = Sheet::new("Data");
    set_rows(&mut data, [1.0, 2.0]);
    wb.sheets.push(Arc::new(data));
    wb.names.push(name("rData", None, "Data!$A$1:$B$3"));
    let mut pt = PivotTable {
        name: "P".into(),
        source: source.into(),
        anchor: CellRef::parse("E3").unwrap(),
        rows: vec![PivotField { source_col: "K".into(), ..Default::default() }],
        values: vec![PivotValue { source_col: "V".into(), name: "Sum of V".into(), ..Default::default() }],
        ..Default::default()
    };
    if bind {
        pt.binding = wb.bind_pivot_source(source, REPORT);
        assert!(pt.binding.0.is_some());
    }
    Arc::make_mut(&mut wb.sheets[REPORT]).pivots.push(pt);
    super::refresh(&mut wb, REPORT, 0, true).unwrap();
    wb
}

/// The report's grand total, refreshed in `wb` as it is.
fn total(wb: &mut Workbook) -> Result<f64, String> {
    total_at(wb, REPORT)
}

/// The grand total of the first PivotTable of sheet `si`, refreshed in `wb` as it is.
fn total_at(wb: &mut Workbook, si: usize) -> Result<f64, String> {
    let r = super::refresh(wb, si, 0, true)?;
    let sh = wb.sheet(si).unwrap();
    (r.start.row..=r.end.row)
        .find(|&row| sh.value(CellRef::new(row, r.start.col)).display() == "Grand Total")
        .and_then(|row| sh.value(CellRef::new(row, r.start.col + 1)).as_f64())
        .ok_or_else(|| "no grand total".into())
}

/// One save: the warnings, the written cache's `worksheetSource` and its records' numbers, and
/// the workbook a native reopen gives.
struct Saved {
    warnings: Vec<String>,
    source: Option<String>,
    records: Vec<String>,
    back: Workbook,
}

fn save(wb: &Workbook) -> Saved {
    let (bytes, warnings) = save_bytes_reported(wb, "t.xlsx", FileKind::Xlsx).unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
    // The one cache's parts, by root element: a kept cache keeps names of its own.
    let names: Vec<String> =
        zip.file_names().map(|n| n.unwrap().into_owned()).filter(|n| n.starts_with("xl/pivotCache/") && !n.contains("_rels")).collect();
    let parts: Vec<String> = names
        .iter()
        .map(|n| {
            let mut s = String::new();
            zip.by_name(n).unwrap().read_to_string(&mut s).unwrap();
            s
        })
        .collect();
    let text = |root: &str| parts.iter().find(|x| x.contains(&format!("<{root} "))).cloned();
    let source = text("pivotCacheDefinition").and_then(|x| {
        let i = x.find("<worksheetSource")?;
        Some(x[i..i + x[i..].find("/>")? + 2].to_string())
    });
    let records = text("pivotCacheRecords")
        .map(|x| x.split("<n v=\"").skip(1).filter_map(|s| s.split('"').next()).map(str::to_string).collect())
        .unwrap_or_default();
    let warnings = warnings.into_iter().filter(|w| w.contains("PivotTable")).collect();
    let (back, _) = open_bytes("t.xlsx", &bytes).unwrap();
    Saved { warnings, source, records, back }
}

/// `wb` saved and reopened, as a file with its parts kept is.
fn reopened(wb: &Workbook) -> Workbook {
    let s = save(wb);
    assert!(s.warnings.is_empty(), "{:?}", s.warnings);
    assert!(s.back.sheets[REPORT].preserved.as_ref().is_some_and(|p| !p.pivots.is_empty()), "kept as read");
    s.back
}

/// The PivotTable edited so its kept parts no longer describe it: rebuilt on save.
fn edited(mut wb: Workbook) -> Workbook {
    let pt = &mut Arc::make_mut(&mut wb.sheets[REPORT]).pivots[0];
    pt.enable_show_details = !pt.enable_show_details;
    wb
}

/// The source, records and refreshed total of `wb` saved and reopened; asserts what each says.
fn assert_saved(wb: &Workbook, source: &str, records: &[&str], warnings: usize, total_back: f64) -> Saved {
    let mut s = save(wb);
    assert_eq!(s.source.as_deref(), Some(source), "{:?}", s.warnings);
    assert_eq!(s.records, records);
    assert_eq!(s.warnings.len(), warnings, "{:?}", s.warnings);
    assert_eq!(total(&mut s.back), Ok(total_back), "after a native reopen and refresh");
    s
}

#[test]
fn a_shadow_on_the_host_sheet_is_saved_past_by_naming_the_sheet() {
    for rebuilt in [false, true] {
        let mut wb = reopened(&book("rData", true));
        // Report's own rData now hides the workbook's from a bare name on Report.
        wb.names.push(name("rData", Some(REPORT), "Report!$A$1:$B$3"));
        if rebuilt {
            wb = edited(wb);
        }
        assert_eq!(total(&mut wb), Ok(FROM_DATA), "in session: the definition it is bound to");
        let s = assert_saved(&wb, r#"<worksheetSource name="rData" sheet="Data"/>"#, &["1", "2"], 0, FROM_DATA);
        let pt = &s.back.sheets[REPORT].pivots[0];
        assert_eq!(pt.source, "Data!rData", "rebuilt: {rebuilt}");
    }
}

#[test]
fn a_shadow_on_the_refs_own_sheet_too_is_saved_as_the_range() {
    for rebuilt in [false, true] {
        let mut wb = reopened(&book("rData", true));
        wb.names.push(name("rData", Some(REPORT), "Report!$A$1:$B$3"));
        wb.names.push(name("rData", Some(1), "Report!$A$1:$B$3"));
        if rebuilt {
            wb = edited(wb);
        }
        assert_eq!(total(&mut wb), Ok(FROM_DATA));
        let s = assert_saved(&wb, r#"<worksheetSource ref="A1:B3" sheet="Data"/>"#, &["1", "2"], 1, FROM_DATA);
        assert!(s.warnings[0].contains("was saved as the range Data!A1:B3"), "{:?}", s.warnings);
    }
}

#[test]
fn a_qualified_workbook_name_follows_its_reference_to_another_sheet() {
    let wb = reopened(&book("Data!rData", true));
    assert_eq!(wb.sheets[REPORT].pivots[0].source, "Data!rData");
    for rebuilt in [false, true] {
        let mut wb = wb.clone();
        // Edited, not replaced: the same definition, its reference on Report now.
        wb.names.iter_mut().find(|n| n.name == "rData").unwrap().formula = "Report!$A$1:$B$3".into();
        if rebuilt {
            wb = edited(wb);
        }
        assert_eq!(total(&mut wb), Ok(FROM_REPORT));
        let records: &[&str] = if rebuilt { &["901", "902"] } else { &["1", "2"] };
        // `Data!rData` no longer reads back to it; the bare name, tried before the name with the
        // sheet its reference is on, does.
        assert_saved(&wb, r#"<worksheetSource name="rData"/>"#, records, 0, FROM_REPORT);
    }
}

#[test]
fn a_scope_edit_keeps_the_binding() {
    let mut s = crate::Session::new();
    s.new_workbook();
    {
        let mut wb = book("rData", false);
        wb.names.clear();
        wb.names.push(name("rData", Some(1), "Data!$A$1:$B$3"));
        let binding = wb.bind_pivot_source("Data!rData", REPORT);
        let pt = &mut Arc::make_mut(&mut wb.sheets[REPORT]).pivots[0];
        pt.source = "Data!rData".into();
        pt.binding = binding;
        *Arc::make_mut(&mut s.doc_mut().unwrap().wb) = reopened(&wb);
    }
    // Data's rData made the workbook's, the same name over the same cells.
    s.execute(
        "formulas.defineName",
        json!({"name": "rData", "refersTo": "=Data!$A$1:$B$3", "scope": "Workbook", "oldName": "rData", "oldScope": "Data"}),
    )
    .unwrap();
    let mut wb = (*s.doc().unwrap().wb).clone();
    assert_eq!(wb.names.iter().filter(|n| n.name == "rData").map(|n| n.scope).collect::<Vec<_>>(), [None]);
    assert_eq!(total(&mut wb), Ok(FROM_DATA));
    assert_saved(&wb, r#"<worksheetSource name="rData" sheet="Data"/>"#, &["1", "2"], 0, FROM_DATA);
}

#[test]
fn an_explicit_source_change_rebinds_and_undo_restores_the_binding() {
    let mut s = crate::Session::new();
    s.new_workbook();
    {
        let mut wb = reopened(&book("rData", true));
        wb.names.push(name("rData", Some(REPORT), "Report!$A$1:$B$3"));
        *Arc::make_mut(&mut s.doc_mut().unwrap().wb) = wb;
    }
    let current = |s: &crate::Session| (*s.doc().unwrap().wb).clone();
    s.execute("pivot.changeSource", json!({"sheet": "Report", "pivot": "P", "source": "Data!$A$1:$B$3"})).unwrap();
    assert_eq!(total(&mut current(&s)), Ok(FROM_DATA));
    // Back to `rData`, which on Report now names Report's own: that is what it stands for.
    s.execute("pivot.changeSource", json!({"sheet": "Report", "pivot": "P", "source": "rData"})).unwrap();
    assert_eq!(total(&mut current(&s)), Ok(FROM_REPORT));
    assert_saved(&current(&s), r#"<worksheetSource name="rData"/>"#, &["901", "902"], 0, FROM_REPORT);
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    let wb = current(&s);
    assert_eq!(wb.sheets[REPORT].pivots[0].source, "rData");
    assert_eq!(total(&mut wb.clone()), Ok(FROM_DATA), "the binding as read again");
    assert_saved(&wb, r#"<worksheetSource name="rData" sheet="Data"/>"#, &["1", "2"], 0, FROM_DATA);
}

#[test]
fn a_replacement_that_is_no_range_drops_the_pivot_with_one_warning() {
    let mut wb = reopened(&book("rData", true));
    wb.names.retain(|n| n.name != "rData");
    wb.names.push(name("rData", None, "42"));
    let s = save(&wb);
    assert_eq!(s.source, None, "no cache");
    assert_eq!(s.warnings.len(), 1, "{:?}", s.warnings);
    assert!(s.warnings[0].contains("was not saved") && s.warnings[0].contains("is not a range"), "{:?}", s.warnings);
    assert!(s.back.sheets[REPORT].pivots.is_empty());
}

#[test]
fn an_app_created_unbound_pivot_keeps_its_text_semantics() {
    let mut wb = book("rData", false);
    assert!(wb.sheets[REPORT].pivots[0].binding.0.is_none());
    assert_eq!(total(&mut wb), Ok(FROM_DATA));
    // A bare name typed in the app is whatever it names now: Report's own once there is one.
    wb.names.push(name("rData", Some(REPORT), "Report!$A$1:$B$3"));
    assert_eq!(total(&mut wb), Ok(FROM_REPORT));
    assert_saved(&wb, r#"<worksheetSource ref="A1:B3" sheet="Report"/>"#, &["901", "902"], 0, FROM_REPORT);
}

// ---------------------------------------------------------------- the package as written

/// A written PivotTable: its sheet, the cache part its relationship leads to, the `cacheId` it
/// names, and that cache's `worksheetSource` element.
#[derive(Debug)]
struct WrittenPivot {
    sheet: String,
    cache: String,
    cache_id: String,
    source: String,
}

/// The value of attribute `key` in the tag text `tag`.
fn attr(tag: &str, key: &str) -> Option<String> {
    let pat = format!(" {key}=\"");
    let i = tag.find(&pat)? + pat.len();
    Some(tag[i..i + tag[i..].find('"')?].to_string())
}

/// The text of each start tag `<el …>` in `xml`.
fn tags<'a>(xml: &'a str, el: &str) -> Vec<&'a str> {
    let open = format!("<{el} ");
    xml.match_indices(&open).filter_map(|(i, _)| Some(&xml[i..i + xml[i..].find('>')? + 1])).collect()
}

/// The package's parts as text.
fn parts(bytes: &[u8]) -> std::collections::HashMap<String, String> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let names: Vec<String> = zip.file_names().map(|n| n.unwrap().into_owned()).collect();
    names
        .into_iter()
        .map(|n| {
            let mut s = String::new();
            zip.by_name(&n).unwrap().read_to_string(&mut s).unwrap();
            (n, s)
        })
        .collect()
}

/// (relationship id, type, target part) of the relationships of part `owner`.
fn rels_of(parts: &std::collections::HashMap<String, String>, owner: &str) -> Vec<(String, String, String)> {
    let (dir, file) = owner.rsplit_once('/').unwrap_or(("", owner));
    let Some(xml) = parts.get(&format!("{dir}/_rels/{file}.rels")) else { return vec![] };
    tags(xml, "Relationship")
        .into_iter()
        .map(|t| {
            let target = attr(t, "Target").unwrap();
            let mut path: Vec<&str> = if target.starts_with('/') { vec![] } else { dir.split('/').collect() };
            for seg in target.trim_start_matches('/').split('/') {
                match seg {
                    ".." => drop(path.pop()),
                    "." => {}
                    s => path.push(s),
                }
            }
            (attr(t, "Id").unwrap(), attr(t, "Type").unwrap(), path.join("/"))
        })
        .collect()
}

/// Every written PivotTable, followed from its sheet through its own relationship to its cache.
/// Asserts that the `cacheId` it names is the workbook's entry for that very cache, that a cache
/// has one `cacheId` and its `x14:pivotCacheId` (when it has one) is that id.
fn written_pivots(bytes: &[u8]) -> Vec<WrittenPivot> {
    let parts = parts(bytes);
    let wb = &parts["xl/workbook.xml"];
    let wb_rels = rels_of(&parts, "xl/workbook.xml");
    let target = |rid: &str| wb_rels.iter().find(|r| r.0 == rid).map(|r| r.2.clone()).unwrap();
    let by_id: std::collections::HashMap<String, String> =
        tags(wb, "pivotCache").into_iter().map(|t| (attr(t, "cacheId").unwrap(), target(&attr(t, "r:id").unwrap()))).collect();
    let mut out = vec![];
    for sheet in tags(wb, "sheet") {
        let part = target(&attr(sheet, "r:id").unwrap());
        for (_, _, table) in rels_of(&parts, &part).into_iter().filter(|r| r.1.ends_with("/pivotTable")) {
            let cache_id = attr(tags(&parts[&table], "pivotTableDefinition")[0], "cacheId").unwrap();
            let cache = rels_of(&parts, &table).into_iter().find(|r| r.1.ends_with("/pivotCacheDefinition")).unwrap().2;
            assert_eq!(by_id.get(&cache_id), Some(&cache), "the cacheId of {table} names the cache it is related to");
            let def = &parts[&cache];
            for x14 in tags(def, "x14:pivotCacheDefinition") {
                assert_eq!(attr(x14, "pivotCacheId").as_ref(), Some(&cache_id), "{cache}");
            }
            let source = tags(def, "worksheetSource").first().map(|t| t.to_string()).unwrap_or_default();
            out.push(WrittenPivot { sheet: attr(sheet, "name").unwrap(), cache, cache_id, source });
        }
    }
    for a in &out {
        assert!(out.iter().all(|b| (a.cache == b.cache) == (a.cache_id == b.cache_id)), "one cacheId per cache: {out:?}");
    }
    out
}

/// `wb` saved: its PivotTables as written, every warning, and the workbook a native reopen gives.
fn save_package(wb: &Workbook) -> (Vec<WrittenPivot>, Vec<String>, Workbook) {
    let (bytes, warnings) = save_bytes_reported(wb, "t.xlsx", FileKind::Xlsx).unwrap();
    let (back, _) = open_bytes("t.xlsx", &bytes).unwrap();
    (written_pivots(&bytes), warnings, back)
}

const SECOND: usize = 2;
const FROM_THIRD: f64 = 203.0;

/// [`book`] with sheets Second (K/V with 501, 502) and Third (101, 102), and a PivotTable on
/// Second as on Report: both on the workbook's `rData`, bound as read and sharing one cache in
/// the file, kept.
fn two_hosts() -> Workbook {
    let mut wb = book("rData", true);
    for (n, v) in [("Second", [501.0, 502.0]), ("Third", [101.0, 102.0])] {
        let mut sh = Sheet::new(n);
        set_rows(&mut sh, v);
        wb.sheets.push(Arc::new(sh));
    }
    let mut pt = PivotTable { id: 2, name: "Q".into(), ..wb.sheets[REPORT].pivots[0].clone() };
    pt.binding = wb.bind_pivot_source("rData", SECOND);
    Arc::make_mut(&mut wb.sheets[SECOND]).pivots.push(pt);
    super::refresh(&mut wb, SECOND, 0, true).unwrap();
    let (written, warnings, back) = save_package(&wb);
    assert!(written.len() == 2 && written[0].cache == written[1].cache, "one cache: {written:?} {warnings:?}");
    for si in [REPORT, SECOND] {
        assert!(back.sheets[si].preserved.as_ref().is_some_and(|p| !p.pivots.is_empty()), "kept as read");
    }
    back
}

/// Every PivotTable of `wb` edited so its kept parts no longer describe it: rebuilt on save.
fn all_edited(mut wb: Workbook) -> Workbook {
    for sh in wb.sheets.iter_mut() {
        for pt in Arc::make_mut(sh).pivots.iter_mut() {
            pt.enable_show_details = !pt.enable_show_details;
        }
    }
    wb
}

/// The written PivotTable of sheet `si` of `wb`.
fn written_on<'a>(written: &'a [WrittenPivot], wb: &Workbook, si: usize) -> &'a WrittenPivot {
    written.iter().find(|p| p.sheet == wb.sheets[si].name).unwrap_or_else(|| panic!("{written:?}"))
}

#[test]
fn pivots_sharing_a_cache_get_one_each_once_a_shadow_on_one_host_needs_another_source() {
    for (shadowed, other) in [(REPORT, SECOND), (SECOND, REPORT)] {
        for rebuilt in [false, true] {
            let mut wb = two_hosts();
            let host = wb.sheets[shadowed].name.clone();
            wb.names.push(name("rData", Some(shadowed), &format!("{host}!$A$1:$B$3")));
            if rebuilt {
                wb = all_edited(wb);
            }
            for si in [shadowed, other] {
                assert_eq!(total_at(&mut wb, si), Ok(FROM_DATA), "in session");
            }
            let (written, warnings, mut back) = save_package(&wb);
            let case = format!("shadow on {host}, rebuilt: {rebuilt}: {written:?} {warnings:?}");
            assert!(!warnings.iter().any(|w| w.contains("PivotTable")), "{case}");
            let (s, o) = (written_on(&written, &wb, shadowed), written_on(&written, &wb, other));
            assert_eq!(s.source, r#"<worksheetSource name="rData" sheet="Data"/>"#, "{case}");
            assert_eq!(o.source, r#"<worksheetSource name="rData"/>"#, "{case}");
            assert!(s.cache != o.cache && s.cache_id != o.cache_id, "{case}");
            if !rebuilt {
                assert!(s.cache.contains("kept") && o.cache.contains("kept"), "both through the kept path: {case}");
            }
            for si in [shadowed, other] {
                assert_eq!(total_at(&mut back, si), Ok(FROM_DATA), "after a native reopen: {case}");
            }
        }
    }
}

#[test]
fn a_pivot_whose_shared_cache_can_only_say_a_range_gets_its_own_and_the_save_says_so_once() {
    for (shadowed, other) in [(REPORT, SECOND), (SECOND, REPORT)] {
        for rebuilt in [false, true] {
            let mut wb = two_hosts();
            let host = wb.sheets[shadowed].name.clone();
            wb.names.push(name("rData", Some(shadowed), &format!("{host}!$A$1:$B$3")));
            // Data's own rData hides the workbook's from `Data!rData` too.
            wb.names.push(name("rData", Some(1), "Third!$A$1:$B$3"));
            if rebuilt {
                wb = all_edited(wb);
            }
            let (written, warnings, mut back) = save_package(&wb);
            let case = format!("shadow on {host}, rebuilt: {rebuilt}: {written:?} {warnings:?}");
            let pivot_warnings: Vec<&String> = warnings.iter().filter(|w| w.contains("PivotTable")).collect();
            assert_eq!(pivot_warnings.len(), 1, "{case}");
            assert!(pivot_warnings[0].contains(&format!("on sheet \"{host}\"")) && pivot_warnings[0].contains("the range Data!A1:B3"), "{case}");
            let (s, o) = (written_on(&written, &wb, shadowed), written_on(&written, &wb, other));
            assert_eq!(s.source, r#"<worksheetSource ref="A1:B3" sheet="Data"/>"#, "{case}");
            assert_eq!(o.source, r#"<worksheetSource name="rData"/>"#, "{case}");
            assert!(s.cache != o.cache && s.cache_id != o.cache_id, "{case}");
            for si in [shadowed, other] {
                assert_eq!(total_at(&mut back, si), Ok(FROM_DATA), "after a native reopen: {case}");
            }
        }
    }
}

/// Table `name` over K/V in A1:B3.
fn kv_table(name: &str) -> Table {
    let col = |n: &str| TableColumn { name: n.into(), totals: TotalsFn::None, totals_label: None, formula: None, unique_name: None, xml: None };
    Table {
        id: 1,
        name: name.into(),
        range: RangeRef::parse("A1:B3").unwrap(),
        header_row: true,
        totals_row: false,
        columns: vec![col("K"), col("V")],
        style: String::new(),
        banded_rows: true,
        banded_cols: false,
        first_col: false,
        last_col: false,
        filter_button: true,
        filter: vec![],
        table_type: None,
        connection_id: None,
        uid: Default::default(),
    }
}

#[test]
fn a_table_renamed_on_saving_never_takes_the_name_a_source_is_saved_as() {
    // The workbook's `r_Data` (Data's 1, 2), and a table on Report over its 901, 902.
    let mut wb = book("rData", false);
    wb.names[0].name = "r_Data".into();
    Arc::make_mut(&mut wb.sheets[REPORT]).tables.push(kv_table("Tbl"));
    let binding = wb.bind_pivot_source("r_Data", REPORT);
    let pt = &mut Arc::make_mut(&mut wb.sheets[REPORT]).pivots[0];
    pt.source = "r_Data".into();
    pt.binding = binding;
    let wb = reopened(&wb);
    for rebuilt in [false, true] {
        let mut wb = wb.clone();
        // The invalid table name would sanitize to the existing workbook name without X17.
        let sheet = Arc::make_mut(&mut wb.sheets[REPORT]);
        sheet.tables[0].name = "r?Data".into();
        let table_formula = CellRef::new(0, 8);
        sheet.set_cell(table_formula, gridcraft_model::Cell::formula(gridcraft_model::Formula::new("SUM(r?Data[V])")));
        if rebuilt {
            wb = edited(wb);
        }
        assert_eq!(total(&mut wb), Ok(FROM_DATA));
        let (written, warnings, mut back) = save_package(&wb);
        let case = format!("rebuilt: {rebuilt}: {written:?} {warnings:?}");
        assert_eq!(written[0].source, r#"<worksheetSource name="r_Data"/>"#, "{case}");
        assert_eq!(warnings, [r#"Table "r?Data" on sheet "Report" was renamed to "r_Data_2"."#], "{case}");
        assert_eq!(back.sheets[REPORT].tables[0].name, "r_Data_2");
        assert!(matches!(back.pivot_source(&back.sheets[REPORT].pivots[0], REPORT).map(|s| s.source),
            Ok(gridcraft_model::PivotSource::Name { name, .. }) if name.name == "r_Data" && name.formula == "Data!$A$1:$B$3"));
        assert_eq!(back.table("r_Data_2"), Some((REPORT, 0)));
        assert_eq!(
            back.sheets[REPORT].cell(table_formula).and_then(|c| c.formula.as_ref()).map(|f| f.text().into_owned()).as_deref(),
            Some("SUM(r_Data_2[V])")
        );
        assert_eq!(total(&mut back), Ok(FROM_DATA), "after a native reopen: {case}");
    }
}

#[test]
fn a_missing_imported_source_cannot_acquire_a_sanitized_table() {
    let mut wb = book("rData", true);
    wb.names[0].name = "r_Data".into();
    let binding = wb.bind_pivot_source("r_Data", REPORT);
    let pt = &mut Arc::make_mut(&mut wb.sheets[REPORT]).pivots[0];
    pt.source = "r_Data".into();
    pt.binding = binding;
    let (bytes, _) = save_bytes_reported(&wb, "t.xlsx", FileKind::Xlsx).unwrap();
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(vec![]));
    for (part, mut xml) in parts(&bytes) {
        if part == "xl/workbook.xml" {
            xml = xml.replace(r#"<definedName name="r_Data""#, r#"<definedName name="different""#);
        }
        zip.start_file(part, zip::write::SimpleFileOptions::default()).unwrap();
        std::io::Write::write_all(&mut zip, xml.as_bytes()).unwrap();
    }
    let (wb, _) = open_bytes("t.xlsx", &zip.finish().unwrap().into_inner()).unwrap();
    assert!(wb.sheets[REPORT].pivots[0].binding.unresolved("r_Data"));
    for (rebuilt, table) in [(false, "r Data"), (true, "r Data"), (false, "r_Data"), (true, "r_Data")] {
        let mut candidate = if rebuilt { edited(wb.clone()) } else { wb.clone() };
        Arc::make_mut(&mut candidate.sheets[REPORT]).tables.push(kv_table(table));
        assert!(total(&mut candidate).is_err());
        let (written, warnings, back) = save_package(&candidate);
        assert!(written.is_empty(), "{written:?}");
        let drops: Vec<_> = warnings.iter().filter(|w| w.contains("PivotTable") && w.contains("was not saved")).collect();
        assert_eq!(drops.len(), 1, "{warnings:?}");
        assert!(drops[0].contains("was unresolved on import but would acquire a definition"), "{warnings:?}");
        assert_eq!(back.sheets[REPORT].tables[0].name, "r_Data");
        assert_eq!(back.sheets[REPORT].value(CellRef::parse("F6").unwrap()), wb.sheets[REPORT].value(CellRef::parse("F6").unwrap()));
    }
}

#[test]
fn omitted_source_and_dependency_use_the_native_range() {
    let base = reopened(&book("rData", true));
    for (rebuilt, dependency) in [(false, false), (false, true), (true, false), (true, true)] {
        let mut wb = if rebuilt { edited(base.clone()) } else { base.clone() };
        if dependency {
            wb.names[0].formula = "_xlchart.Input".into();
            wb.names.push(name("_xlchart.Input", None, "Data!$A$1:$B$3"));
        } else {
            wb.names[0].name = "_xlchart.Input".into();
        }
        assert_eq!(total(&mut wb), Ok(FROM_DATA));
        assert_saved(&wb, r#"<worksheetSource ref="A1:B3" sheet="Data"/>"#, &["1", "2"], 1, FROM_DATA);
    }
}

#[test]
fn failed_native_refresh_does_not_mask_an_omitted_formula_dependency() {
    let mut wb = reopened(&book("rData", true));
    wb.names[0].formula = "OFFSET(_xlchart.Input,0,0,3,2)".into();
    wb.names.push(name("_xlchart.Input", None, "Data!$A$1:$B$3"));
    assert!(total(&mut wb).is_err());
    let saved = save(&wb);
    assert!(saved.back.sheets[REPORT].pivots.is_empty());
    assert_eq!(saved.warnings.len(), 1, "{:?}", saved.warnings);
    assert!(saved.warnings[0].contains("cannot be represented by the saved definitions"), "{:?}", saved.warnings);
}

#[test]
fn a_source_change_keeps_x14_provenance_and_uses_the_new_data() {
    let mut wb = book("rData", true);
    Arc::make_mut(&mut wb.sheets[REPORT]).pivots[0].had_x14_cache_id.0 = true;
    let mut s = crate::Session::new();
    s.new_workbook();
    *Arc::make_mut(&mut s.doc_mut().unwrap().wb) = reopened(&wb);
    assert!(s.doc().unwrap().wb.sheets[REPORT].pivots[0].had_x14_cache_id.0);
    s.execute("pivot.changeSource", json!({"sheet": "Report", "pivot": "P", "source": "Report!$A$1:$B$3"})).unwrap();
    let result = assert_saved(&s.doc().unwrap().wb, r#"<worksheetSource ref="A1:B3" sheet="Report"/>"#, &["901", "902"], 0, FROM_REPORT);
    assert!(result.back.sheets[REPORT].pivots[0].had_x14_cache_id.0);
}

#[test]
fn generated_slicer_names_never_displace_a_pivot_source() {
    use gridcraft_model::{Anchor, AnchorMode, PivotSort, Slicer, SlicerCrossFilter, SlicerPivot, SlicerSource};
    let mut wb = book("rData", true);
    wb.names[0].name = "Slicer_K".into();
    let binding = wb.bind_pivot_source("Slicer_K", REPORT);
    let pt = &mut Arc::make_mut(&mut wb.sheets[REPORT]).pivots[0];
    pt.source = "Slicer_K".into();
    pt.binding = binding;
    let mut wb = reopened(&wb);
    let data = Arc::make_mut(&mut wb.sheets[1]);
    data.tables.push(kv_table("Slicer K2"));
    let report = Arc::make_mut(&mut wb.sheets[REPORT]);
    let mut sliced = report.pivots[0].clone();
    sliced.id += 100;
    sliced.name = "Sliced".into();
    sliced.anchor = CellRef::parse("I3").unwrap();
    sliced.source = "Report!$A$1:$B$3".into();
    sliced.binding = Default::default();
    report.pivots.push(sliced);
    report.slicers.push(Slicer {
        id: 1,
        name: "K".into(),
        caption: "K".into(),
        columns: 1,
        style: String::new(),
        row_height: None,
        imported_cache: Default::default(),
        anchor: Anchor { cell: CellRef::parse("M1").unwrap(), dx: 0.0, dy: 0.0, width: 180.0, height: 240.0, mode: AnchorMode::default() },
        source: SlicerSource::Pivot {
            pivots: vec![SlicerPivot { sheet: "Report".into(), name: "Sliced".into() }],
            field: "K".into(),
            sort: PivotSort::Asc,
            cross_filter: SlicerCrossFilter::ShowItemsWithDataAtTop,
            show_missing: true,
        },
    });
    for rebuilt in [false, true] {
        let candidate = if rebuilt { edited(wb.clone()) } else { wb.clone() };
        let (written, warnings, back) = save_package(&candidate);
        assert_eq!(written.len(), 2, "{written:?} {warnings:?}");
        assert_ne!(written[0].cache_id, written[1].cache_id);
        assert_eq!(back.sheets[REPORT].slicers.len(), 1);
        let (written, warnings, mut back) = save_package(&back);
        assert_eq!(written.len(), 2, "{written:?} {warnings:?}");
        assert_eq!(back.sheets[REPORT].slicers.len(), 1);
        assert_eq!(back.names.iter().filter(|n| n.name.starts_with("Slicer_K")).count(), 2);
        assert_eq!(back.name_in_scope("Slicer_K", None).unwrap().formula, "Data!$A$1:$B$3");
        assert_eq!(back.name_in_scope("Slicer_K3", None).unwrap().formula, "#N/A");
        assert_eq!(total(&mut back), Ok(FROM_DATA));
        back.sheets[REPORT] = Arc::new({
            let mut report = (*back.sheets[REPORT]).clone();
            report.pivots.remove(0);
            report
        });
        assert_eq!(total(&mut back), Ok(FROM_REPORT));
    }
}

#[test]
fn table_rename_follows_bound_pivots_and_does_not_capture_let_or_lambda_names() {
    let mut wb = book("rData", true);
    Arc::make_mut(&mut wb.sheets[1]).tables.push(kv_table("Sales"));
    let binding = wb.bind_pivot_source("Data!Sales", REPORT);
    let pt = &mut Arc::make_mut(&mut wb.sheets[REPORT]).pivots[0];
    pt.source = "Data!Sales".into();
    pt.binding = binding;
    let mut s = crate::Session::new();
    s.new_workbook();
    *Arc::make_mut(&mut s.doc_mut().unwrap().wb) = wb;
    let checks = [
        ("M1", "=LET(Sales,5,Sales+SUM(Sales[V]))", 8.0),
        ("M2", "=LET(Renamed,5,SUM(Sales)+Renamed)", 8.0),
        ("M3", "=LET(Sales,SUM(Sales),Sales+SUM(Data!Sales))", 6.0),
        ("M4", "=LAMBDA(Renamed,Renamed+SUM(Sales))(5)", 8.0),
        ("M5", "=ROWS(Data!Sales)", 2.0),
        ("M6", "=LET(Data!Sales,5,Data!Sales+SUM(Sales[V]))", 8.0),
        ("M7", "=LET(Data!Renamed,5,SUM(Data!Sales)+Data!Renamed)", 8.0),
        ("M8", "=LAMBDA(Data!Renamed,Data!Renamed+SUM(Data!Sales))(5)", 8.0),
    ];
    for (cell, input, _) in checks {
        s.execute("cell.set", json!({"sheet": "Report", "cell": cell, "input": input})).unwrap();
    }
    let assert_values = |s: &crate::Session| {
        for (cell, _, value) in checks {
            assert_eq!(s.doc().unwrap().wb.sheets[REPORT].value(CellRef::parse(cell).unwrap()), Value::Number(value), "{cell}");
        }
    };
    assert_values(&s);
    s.execute("table.rename", json!({"sheet": "Data", "table": "Sales", "name": "Renamed"})).unwrap();
    assert_values(&s);
    let formulas = &s.doc().unwrap().wb.sheets[REPORT];
    assert_eq!(formulas.cells.get(CellRef::parse("M5").unwrap()).unwrap().formula.as_ref().unwrap().text(), "ROWS(Data!Renamed)");
    let mut wb = (*s.doc().unwrap().wb).clone();
    assert_eq!(wb.sheets[REPORT].pivots[0].source, "Data!Renamed");
    assert_eq!(total(&mut wb), Ok(FROM_DATA));
}

#[test]
fn table_rename_keeps_builtin_calls_that_share_a_lambda_name() {
    let mut wb = book("rData", true);
    Arc::make_mut(&mut wb.sheets[1]).tables.push(kv_table("Sales"));
    wb.names.push(name("IsRef", None, "LAMBDA(x,FALSE)"));
    // A native file's cell tree parses the call as the built-in ISREF.
    let at = CellRef::parse("M1").unwrap();
    let f: Formula = serde_json::from_str(r#"{"text":"ISREF(Sales[V])","array":null}"#).unwrap();
    Arc::make_mut(&mut wb.sheets[REPORT]).set_cell(at, Cell::formula(f));
    let mut s = rename_session(wb);
    s.execute("formulas.calculateNow", json!({})).unwrap();
    assert_eq!(s.doc().unwrap().wb.sheets[REPORT].value(at), Value::Bool(true));
    s.execute("table.rename", json!({"sheet": "Data", "table": "Sales", "name": "Renamed"})).unwrap();
    let sh = &s.doc().unwrap().wb.sheets[REPORT];
    assert_eq!(sh.value(at), Value::Bool(true));
    assert_eq!(sh.cell(at).unwrap().formula.as_ref().unwrap().text(), "ISREF(Renamed[V])");
}

fn rename_session(wb: Workbook) -> crate::Session {
    let mut s = crate::Session::new();
    s.new_workbook();
    *Arc::make_mut(&mut s.doc_mut().unwrap().wb) = wb;
    s
}

fn check_table_rename(mut s: crate::Session, check: impl Fn(&Workbook, &str)) -> Workbook {
    check(&s.doc().unwrap().wb, "Sales");
    s.execute("table.rename", json!({"sheet": "Data", "table": "Sales", "name": "Renamed"})).unwrap();
    check(&s.doc().unwrap().wb, "Renamed");
    s.execute("edit.undo", json!({})).unwrap();
    check(&s.doc().unwrap().wb, "Sales");
    s.execute("edit.redo", json!({})).unwrap();
    check(&s.doc().unwrap().wb, "Renamed");
    (*s.doc().unwrap().wb).clone()
}

#[test]
fn table_rename_follows_defined_names_without_changing_shadowed_names() {
    let mut wb = book("rData", true);
    Arc::make_mut(&mut wb.sheets[1]).tables.push(kv_table("Sales"));
    wb.names.extend([
        name("Sales", Some(REPORT), "42"),
        name("Amounts", None, "Sales[V]"),
        name("DataRows", Some(1), "ROWS(Sales)"),
        name("LocalTotal", Some(REPORT), "Sales+SUM(Sales[V])"),
        name("CallerDependent", None, "SUM(Sales)"),
        name("LocalBinder", None, "LET(Sales,5,Sales+SUM(Sales[V]))"),
    ]);
    let mut s = rename_session(wb);
    for (sheet, at, f) in [("Report", "M1", "=SUM(Amounts)+LocalTotal+LocalBinder"), ("Data", "M1", "=DataRows")] {
        s.execute("cell.set", json!({"sheet": sheet, "cell": at, "input": f})).unwrap();
    }
    let check = |wb: &Workbook, table: &str| {
        assert_eq!(wb.name("Amounts", REPORT).unwrap().formula, format!("{table}[V]"));
        assert_eq!(wb.name("DataRows", 1).unwrap().formula, format!("ROWS({table})"));
        assert_eq!(wb.name("LocalTotal", REPORT).unwrap().formula, format!("Sales+SUM({table}[V])"));
        assert_eq!(wb.name("CallerDependent", REPORT).unwrap().formula, "SUM(Sales)");
        assert_eq!(wb.name("LocalBinder", REPORT).unwrap().formula, format!("LET(Sales,5,Sales+SUM({table}[V]))"));
        assert_eq!(wb.sheets[REPORT].value(CellRef::parse("M1").unwrap()), Value::Number(56.0));
        assert_eq!(wb.sheets[1].value(CellRef::parse("M1").unwrap()), Value::Number(2.0));
    };
    let wb = check_table_rename(s, check);
    check(&reopened(&wb), "Renamed");
}

#[test]
fn table_rename_follows_names_scoped_to_retained_sheets() {
    use gridcraft_model::{Preserved, PreservedName, PreservedPart, PreservedRel, PreservedSheet};

    let mut wb = book("rData", true);
    Arc::make_mut(&mut wb.sheets[1]).tables.push(kv_table("Sales"));
    let attrs = r#" hidden="1""#;
    let part = "xl/chartsheets/sheet1.xml";
    let content = br#"<chartsheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetViews><sheetView workbookViewId="0"/></sheetViews></chartsheet>"#;
    wb.preserved = Some(Arc::new(Preserved {
        rels: vec![PreservedRel {
            id: "rIdKept".into(),
            rel_type: "http://schemas.openxmlformats.org/officeDocument/2006/relationships/chartsheet".into(),
            target: part.into(),
            external: false,
        }],
        parts: vec![PreservedPart {
            name: part.into(),
            content_type: "application/vnd.openxmlformats-officedocument.spreadsheetml.chartsheet+xml".into(),
            data: Arc::from(content.as_slice()),
            rels: vec![],
            dxfs: vec![],
            num_fmts: vec![],
        }],
        sheets: vec![PreservedSheet {
            name: "KeptChart".into(),
            rel_id: "rIdKept".into(),
            before: 1,
            sheet_id: 4,
            names: vec![PreservedName { name: "KeptAmounts".into(), attrs: attrs.into(), formula: "Sales[V]".into() }],
            ..Default::default()
        }],
        ..Default::default()
    }));
    let check = |wb: &Workbook, table: &str| {
        let kept = wb.preserved.as_ref().unwrap();
        let name = &kept.sheets[0].names[0];
        assert_eq!(name.name, "KeptAmounts");
        assert_eq!(name.formula, format!("{table}[V]"));
        assert_eq!(name.attrs, attrs);
        assert_eq!(kept.parts.iter().find(|p| p.name == part).unwrap().data.as_ref(), content);
    };
    let wb = check_table_rename(rename_session(wb), check);
    check(&reopened(&wb), "Renamed");
}

#[test]
fn table_rename_follows_table_column_formulas_but_not_totals_labels() {
    let mut wb = book("rData", true);
    let table = &mut kv_table("Sales");
    table.columns[0].totals_label = Some("Sales[V]".into());
    table.columns[1].formula = Some("ROW(Sales[@K])".into());
    table.columns[1].totals = TotalsFn::Custom;
    table.columns[1].totals_label = Some("SUM(Sales[V])".into());
    Arc::make_mut(&mut wb.sheets[1]).tables.push(table.clone());
    let check = |wb: &Workbook, name: &str| {
        let columns = &wb.sheets[1].tables[0].columns;
        assert_eq!(columns[0].totals_label.as_deref(), Some("Sales[V]"));
        assert_eq!(columns[1].formula, Some(format!("ROW({name}[@K])")));
        assert_eq!(columns[1].totals_label, Some(format!("SUM({name}[V])")));
    };
    let wb = check_table_rename(rename_session(wb), check);
    check(&reopened(&wb), "Renamed");
}

#[test]
fn table_rename_follows_unbound_pivot_table_sources() {
    let mut wb = book("rData", false);
    Arc::make_mut(&mut wb.sheets[1]).tables.push(kv_table("Sales"));
    Arc::make_mut(&mut wb.sheets[REPORT]).pivots[0].source = "Sales".into();
    let check = |wb: &Workbook, table: &str| {
        assert_eq!(wb.sheets[REPORT].pivots[0].source, table);
        assert_eq!(total(&mut wb.clone()), Ok(FROM_DATA));
    };
    let wb = check_table_rename(rename_session(wb), check);
    check(&reopened(&wb), "Renamed");
}

#[test]
fn table_rename_follows_a_pivot_rebound_to_the_table_that_took_its_place() {
    let mut wb = book("rData", false);
    Arc::make_mut(&mut wb.sheets[1]).tables.push(kv_table("Sales"));
    let binding = wb.bind_pivot_source("Sales", REPORT);
    let pt = &mut Arc::make_mut(&mut wb.sheets[REPORT]).pivots[0];
    pt.source = "Sales".into();
    pt.binding = binding;
    // The bound table is gone; another of the same name took its place.
    Arc::make_mut(&mut wb.sheets[1]).tables[0].uid = gridcraft_model::DefinitionUid::fresh();
    let check = |wb: &Workbook, table: &str| {
        let pt = &wb.sheets[REPORT].pivots[0];
        assert_eq!(pt.source, table);
        assert_eq!(total(&mut wb.clone()), Ok(FROM_DATA));
        let found = wb.pivot_source(pt, REPORT).unwrap();
        assert_eq!(found.rebound, Some(("table \"Sales\"".into(), format!("table \"{table}\""))));
    };
    let wb = check_table_rename(rename_session(wb), check);
    let saved = save(&wb);
    assert_eq!(saved.warnings.len(), 1, "{:?}", saved.warnings);
    assert!(saved.warnings[0].contains("table \"Sales\" was deleted, and table \"Renamed\" took its place"), "{:?}", saved.warnings);
    let mut reopened = saved.back;
    assert_eq!(reopened.sheets[REPORT].pivots[0].source, "Renamed");
    assert_eq!(total(&mut reopened), Ok(FROM_DATA));
}

#[test]
fn a_bare_name_is_saved_before_a_range_so_the_binding_stays_live() {
    let wb = reopened(&book("Data!rData", true));
    for rebuilt in [false, true] {
        let mut wb = wb.clone();
        let mut third = Sheet::new("Third");
        set_rows(&mut third, [101.0, 102.0]);
        wb.sheets.push(Arc::new(third));
        // The same workbook name, its reference moved to Third, which has its own rData: neither
        // `Data!rData` nor `Third!rData` reads back to it, the bare name from Report does.
        wb.names.iter_mut().find(|n| n.name == "rData").unwrap().formula = "Third!$A$1:$B$3".into();
        wb.names.push(name("rData", Some(2), "Third!$A$1:$B$3"));
        if rebuilt {
            wb = edited(wb);
        }
        assert_eq!(total(&mut wb), Ok(FROM_THIRD));
        let (written, warnings, mut back) = save_package(&wb);
        let case = format!("rebuilt: {rebuilt}: {written:?} {warnings:?}");
        assert_eq!(written[0].source, r#"<worksheetSource name="rData"/>"#, "{case}");
        assert!(!warnings.iter().any(|w| w.contains("PivotTable")), "{case}");
        assert_eq!(total(&mut back), Ok(FROM_THIRD), "{case}");
        // Edited again after the reopen: the source follows the name, not a frozen range.
        back.names.iter_mut().find(|n| n.name == "rData" && n.scope.is_none()).unwrap().formula = "Data!$A$1:$B$3".into();
        assert_eq!(total(&mut back), Ok(FROM_DATA), "{case}");
    }
}

#[test]
fn an_explicit_source_change_takes_what_reading_a_file_takes() {
    let mut s = crate::Session::new();
    s.new_workbook();
    {
        let mut wb = book("rData", true);
        Arc::make_mut(&mut wb.sheets[REPORT]).tables.push(kv_table("Tbl"));
        *Arc::make_mut(&mut s.doc_mut().unwrap().wb) = wb;
    }
    let current = |s: &crate::Session| (*s.doc().unwrap().wb).clone();
    // A qualified table, then a qualified workbook name: both as `worksheetSource name sheet`.
    s.execute("pivot.changeSource", json!({"sheet": "Report", "pivot": "P", "source": "Report!Tbl"})).unwrap();
    assert_eq!(total(&mut current(&s)), Ok(FROM_REPORT));
    s.execute("pivot.changeSource", json!({"sheet": "Report", "pivot": "P", "source": "Data!rData"})).unwrap();
    assert_eq!(total(&mut current(&s)), Ok(FROM_DATA));
    assert_saved(&current(&s), r#"<worksheetSource name="rData" sheet="Data"/>"#, &["1", "2"], 0, FROM_DATA);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(current(&s).sheets[REPORT].pivots[0].source, "Report!Tbl");
    assert_eq!(total(&mut current(&s)), Ok(FROM_REPORT), "undo restores the table binding");
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(current(&s).sheets[REPORT].pivots[0].source, "rData");
    assert_eq!(total(&mut current(&s)), Ok(FROM_DATA));
}
