//! Apply and Clear in a filter-header popup change only the filter that owns the clicked header
//! (X50): a table's own filter or the worksheet AutoFilter. Real button and menu clicks over
//! in-memory workbooks; no fixture files.

use crate::common::{self, Harness};
use egui::{Event, Key, Modifiers, PointerButton, Pos2, vec2};
use egui_kittest::kittest::Queryable;
use gridcraft_engine::Session;
use gridcraft_engine::core::CellRef;
use gridcraft_engine::model::{FilterCriterion, Sheet};
use gridcraft_ui_egui::grid;
use serde_json::json;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn harness(s: Session) -> Harness {
    common::harness_with(common::builder().with_size(vec2(1200.0, 800.0)), common::app(s), 4)
}

fn sheet(h: &Harness) -> TestResult<&Sheet> {
    Ok(h.state().session.active().and_then(|d| d.wb.active()).ok_or("active sheet")?)
}

fn undo_depth(h: &Harness) -> TestResult<usize> {
    Ok(h.state().session.active().ok_or("active document")?.undo.len())
}

/// 1-based numbers of the hidden rows among the first 20.
fn hidden(h: &Harness) -> TestResult<Vec<u32>> {
    let sh = sheet(h)?;
    Ok((0..20).filter(|&r| sh.is_row_hidden(r)).map(|r| r + 1).collect())
}

/// The worksheet AutoFilter's criteria, or `None` without one.
fn sheet_criteria(h: &Harness) -> TestResult<Option<Vec<(u32, FilterCriterion)>>> {
    Ok(sheet(h)?.autofilter.as_ref().map(|af| af.criteria.clone()))
}

fn table_criteria(h: &Harness, name: &str) -> TestResult<Vec<(u32, FilterCriterion)>> {
    Ok(sheet(h)?.tables.iter().find(|t| t.name == name).ok_or("table")?.filter.clone())
}

fn values(v: &[&str]) -> FilterCriterion {
    FilterCriterion::Values { values: v.iter().map(|s| s.to_string()).collect(), blanks: false }
}

fn click(h: &mut Harness, pos: Pos2) {
    h.input_mut().events.push(Event::PointerMoved(pos));
    for pressed in [true, false] {
        h.input_mut().events.push(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
    }
    h.run_steps(4);
}

fn open_menu(h: &mut Harness, header: CellRef) -> TestResult {
    let sh = sheet(h)?;
    let geo = grid::Geo::new(sh, h.state().grid.rect.ok_or("grid rect")?, h.state().view().scroll);
    click(h, grid::filter_button_rect(&geo, sh, header).center());
    assert!(h.state().grid.filter_menu.is_some(), "header button opens its popup");
    Ok(())
}

fn press(h: &mut Harness, label: &str) -> TestResult {
    let at = h.query_by_label(label).ok_or_else(|| format!("the popup shows no `{label}`"))?.rect().center();
    click(h, at);
    Ok(())
}

/// Opens the popup on `header`, which must list `listed` (its own filter's values), unchecks
/// `off` and applies.
fn filter_out(h: &mut Harness, header: CellRef, listed: &[&str], off: &str) -> TestResult {
    open_menu(h, header)?;
    for v in listed {
        assert!(h.query_by_label(v).is_some(), "the popup on {} lists its own filter's `{v}`", header.a1());
    }
    press(h, off)?;
    press(h, "Apply")?;
    assert!(h.state().grid.filter_menu.is_none(), "Apply closes the popup");
    Ok(())
}

/// A worksheet AutoFilter over A1:B4 and table `People` at A8:B12, whose totals row is 12.
/// Their columns overlap, so a table action routed to the worksheet filter acts on a real column.
fn sheet_filter_and_table() -> TestResult<Harness> {
    let mut s = Session::new();
    s.new_workbook();
    s.execute("range.setValues", json!({"range":"A1", "values":[["Name","Q"],["Ann",1],["Ben",2],["Cat",3]]}))?;
    s.execute("data.filter", json!({"range":"A1:B4"}))?;
    s.execute("range.setValues", json!({"range":"A8", "values":[["Who","N"],["Dan",1],["Eve",2],["Fay",3]]}))?;
    s.execute("insert.table", json!({"range":"A8:B11", "header":true, "name":"People"}))?;
    s.execute("table.totalRow", json!({"table":"People", "on":true}))?;
    s.execute("selection.set", json!({"range":"F1"}))?;
    Ok(harness(s))
}

/// Tables `Alpha` (A1:B4) and `Beta` (A8:B12, totals row 12) in the same columns, no worksheet AutoFilter.
fn two_tables() -> TestResult<Harness> {
    let mut s = Session::new();
    s.new_workbook();
    s.execute("range.setValues", json!({"range":"A1", "values":[["Name","Q"],["Ann",1],["Ben",2],["Cat",3]]}))?;
    s.execute("insert.table", json!({"range":"A1:B4", "header":true, "name":"Alpha"}))?;
    s.execute("range.setValues", json!({"range":"A8", "values":[["Who","N"],["Dan",1],["Eve",2],["Fay",3]]}))?;
    s.execute("insert.table", json!({"range":"A8:B11", "header":true, "name":"Beta"}))?;
    s.execute("table.totalRow", json!({"table":"Beta", "on":true}))?;
    s.execute("selection.set", json!({"range":"F1"}))?;
    Ok(harness(s))
}

#[test]
fn table_button_filters_only_its_table_beside_a_worksheet_filter() -> TestResult {
    let mut h = sheet_filter_and_table()?;
    filter_out(&mut h, CellRef::new(7, 0), &["Dan", "Eve", "Fay"], "Eve")?;
    assert_eq!(table_criteria(&h, "People")?, [(0, values(&["Dan", "Fay"]))], "the table's own filter holds the criterion");
    assert_eq!(sheet_criteria(&h)?, Some(vec![]), "the worksheet AutoFilter is untouched");
    assert_eq!(hidden(&h)?, [10], "only Eve's row is hidden; the worksheet rows and the totals row show");
    // The worksheet filter's own button still filters only the worksheet range.
    filter_out(&mut h, CellRef::new(0, 0), &["Ann", "Ben", "Cat"], "Ben")?;
    assert_eq!(sheet_criteria(&h)?, Some(vec![(0, values(&["Ann", "Cat"]))]));
    assert_eq!(table_criteria(&h, "People")?, [(0, values(&["Dan", "Fay"]))]);
    assert_eq!(hidden(&h)?, [3, 10]);
    Ok(())
}

#[test]
fn clear_from_a_button_clears_only_that_filter() -> TestResult {
    let mut h = sheet_filter_and_table()?;
    filter_out(&mut h, CellRef::new(0, 0), &["Ann", "Ben", "Cat"], "Ben")?;
    assert_eq!(hidden(&h)?, [3]);
    // Clear from the table's button, whose column A has no criterion: the worksheet's stays.
    open_menu(&mut h, CellRef::new(7, 0))?;
    press(&mut h, "Clear Filter")?;
    assert_eq!(sheet_criteria(&h)?, Some(vec![(0, values(&["Ann", "Cat"]))]), "Clear on the table must not clear the worksheet filter");
    assert_eq!(hidden(&h)?, [3]);
    // Both filtered: Clear from the worksheet's button leaves the table's criterion, and back.
    filter_out(&mut h, CellRef::new(7, 0), &["Dan", "Eve", "Fay"], "Eve")?;
    assert_eq!(hidden(&h)?, [3, 10]);
    open_menu(&mut h, CellRef::new(0, 0))?;
    press(&mut h, "Clear Filter")?;
    assert_eq!(sheet_criteria(&h)?, Some(vec![]));
    assert_eq!(table_criteria(&h, "People")?, [(0, values(&["Dan", "Fay"]))]);
    assert_eq!(hidden(&h)?, [10]);
    open_menu(&mut h, CellRef::new(7, 0))?;
    press(&mut h, "Clear Filter")?;
    assert_eq!(table_criteria(&h, "People")?, []);
    assert_eq!(hidden(&h)?, Vec::<u32>::new());
    Ok(())
}

#[test]
fn each_of_two_tables_filters_itself() -> TestResult {
    let mut h = two_tables()?;
    let depth = undo_depth(&h)?;
    filter_out(&mut h, CellRef::new(7, 0), &["Dan", "Eve", "Fay"], "Eve")?;
    assert_eq!(table_criteria(&h, "Beta")?, [(0, values(&["Dan", "Fay"]))]);
    assert_eq!(sheet_criteria(&h)?, None, "no worksheet AutoFilter stands in for the table");
    filter_out(&mut h, CellRef::new(0, 0), &["Ann", "Ben", "Cat"], "Ben")?;
    assert_eq!(table_criteria(&h, "Alpha")?, [(0, values(&["Ann", "Cat"]))], "Alpha's button filters Alpha, not Beta");
    assert_eq!(table_criteria(&h, "Beta")?, [(0, values(&["Dan", "Fay"]))]);
    assert_eq!(hidden(&h)?, [3, 10]);
    assert_eq!(undo_depth(&h)?, depth + 2, "each Apply is one undo step");
    Ok(())
}

#[test]
fn table_filter_leaves_its_totals_row_and_opening_changes_nothing() -> TestResult {
    let mut h = two_tables()?;
    let doc = |h: &Harness| -> TestResult<(usize, u64)> {
        let d = h.state().session.active().ok_or("active document")?;
        Ok((d.undo.len(), d.revision))
    };
    let before = doc(&h)?;
    open_menu(&mut h, CellRef::new(7, 1))?;
    h.input_mut().events.push(Event::Key {
        key: Key::Escape,
        physical_key: Some(Key::Escape),
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    });
    h.run_steps(3);
    assert!(h.state().grid.filter_menu.is_none());
    assert_eq!(sheet_criteria(&h)?, None, "opening a table's popup must not install a worksheet AutoFilter over the table");
    assert_eq!(doc(&h)?, before);
    // Filter Beta's N column to 2 and 3: the totals row (12) is outside the filter and shows.
    filter_out(&mut h, CellRef::new(7, 1), &["1", "2", "3"], "1")?;
    assert_eq!(table_criteria(&h, "Beta")?, [(1, values(&["2", "3"]))]);
    assert_eq!(hidden(&h)?, [9], "only Dan's row is hidden; the totals row shows");
    assert_eq!(sheet(&h)?.value(CellRef::new(11, 0)).display(), "Total");
    // Data > Filter on a plain range elsewhere now turns a worksheet filter on there.
    h.state_mut().run("range.setValues", json!({"range":"H1", "values":[["K"],["x"],["y"]]}))?;
    h.state_mut().run("selection.set", json!({"range":"H1"}))?;
    let r = h.state_mut().run("data.filter", json!({}))?;
    assert_eq!(r["filter"], json!(true), "{r}");
    assert_eq!(table_criteria(&h, "Beta")?, [(1, values(&["2", "3"]))]);
    Ok(())
}

#[test]
fn table_apply_undoes_and_redoes_as_one_step() -> TestResult {
    let mut h = two_tables()?;
    let depth = undo_depth(&h)?;
    filter_out(&mut h, CellRef::new(7, 0), &["Dan", "Eve", "Fay"], "Eve")?;
    assert_eq!(undo_depth(&h)?, depth + 1);
    h.state_mut().run("edit.undo", json!({}))?;
    h.run_steps(2);
    assert_eq!(sheet_criteria(&h)?, None, "undo leaves no worksheet AutoFilter behind");
    assert_eq!(table_criteria(&h, "Beta")?, []);
    assert_eq!(hidden(&h)?, Vec::<u32>::new());
    h.state_mut().run("edit.redo", json!({}))?;
    h.run_steps(2);
    assert_eq!(table_criteria(&h, "Beta")?, [(0, values(&["Dan", "Fay"]))]);
    assert_eq!(sheet_criteria(&h)?, None);
    assert_eq!(hidden(&h)?, [10]);
    // A second undo reaches the totals-row step, not a hidden filter install.
    h.state_mut().run("edit.undo", json!({}))?;
    h.state_mut().run("edit.undo", json!({}))?;
    h.run_steps(2);
    assert_eq!(sheet(&h)?.tables.iter().find(|t| t.name == "Beta").ok_or("Beta")?.range.a1(), "A8:B11");
    assert_eq!(sheet_criteria(&h)?, None);
    Ok(())
}

#[test]
fn popup_whose_worksheet_filter_was_removed_applies_nothing() -> TestResult {
    let mut s = Session::new();
    s.new_workbook();
    s.execute("range.setValues", json!({"range":"A1", "values":[["N"], ["c"], ["a"], ["b"]]}))?;
    s.execute("data.filter", json!({"range":"A1:A4"}))?;
    let mut h = harness(s);
    open_menu(&mut h, CellRef::new(0, 0))?;
    press(&mut h, "a")?;
    h.state_mut().run("data.filter", json!({}))?;
    h.run_steps(2);
    if h.query_by_label("Apply").is_some() {
        press(&mut h, "Apply")?;
    }
    assert!(h.state().grid.filter_menu.is_none(), "the popup closes with its filter");
    assert_eq!(sheet_criteria(&h)?, None, "Apply must not turn a new filter on");
    assert_eq!(hidden(&h)?, Vec::<u32>::new());
    Ok(())
}

#[test]
fn popup_whose_table_became_a_worksheet_filter_applies_nothing() -> TestResult {
    let mut h = two_tables()?;
    open_menu(&mut h, CellRef::new(7, 0))?;
    press(&mut h, "Eve")?;
    // A control client replaces Beta by a worksheet AutoFilter over the same header.
    h.state_mut().run("table.convertToRange", json!({"table":"Beta"}))?;
    h.state_mut().run("data.filter", json!({"range":"A8:B11"}))?;
    h.run_steps(2);
    if h.query_by_label("Apply").is_some() {
        press(&mut h, "Apply")?;
    }
    assert!(h.state().grid.filter_menu.is_none(), "the popup closes with the filter that owned it");
    assert_eq!(sheet(&h)?.autofilter.as_ref().map(|af| (af.range.a1(), af.criteria.len())), Some(("A8:B11".into(), 0)), "no filter changed or added");
    assert_eq!(hidden(&h)?, Vec::<u32>::new());
    Ok(())
}

#[test]
fn popup_whose_table_was_replaced_by_another_applies_nothing() -> TestResult {
    let mut h = sheet_filter_and_table()?;
    open_menu(&mut h, CellRef::new(7, 0))?;
    // Unchecks every listed value; a click on Apply would then hide every row of its filter.
    press(&mut h, "(Select All)")?;
    // A control client replaces People by another table over the same header.
    h.state_mut().run("table.convertToRange", json!({"table":"People"}))?;
    h.state_mut().run("insert.table", json!({"range":"A8:B11", "header":true, "name":"Others"}))?;
    h.run_steps(2);
    if h.query_by_label("Apply").is_some() {
        press(&mut h, "Apply")?;
    }
    assert!(h.state().grid.filter_menu.is_none(), "the popup closes with the table that owned it");
    assert_eq!(table_criteria(&h, "Others")?, []);
    assert_eq!(sheet_criteria(&h)?, Some(vec![]));
    assert_eq!(hidden(&h)?, Vec::<u32>::new());
    Ok(())
}
