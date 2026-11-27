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

#[test]
fn context_filter_by_value_targets_the_table_not_the_worksheet() -> TestResult {
    let mut h = sheet_filter_and_table()?;
    let depth = undo_depth(&h)?;
    filter_by_cell_value(&mut h, CellRef::new(8, 0))?;
    assert_eq!(sheet_criteria(&h)?, Some(vec![]), "the context action leaves the worksheet filter alone");
    assert_eq!(table_criteria(&h, "People")?, [(0, values(&["Dan"]))]);
    assert_eq!(hidden(&h)?, [10, 11]);
    assert_eq!(undo_depth(&h)?, depth + 1);
    Ok(())
}

fn filter_by_cell_value(h: &mut Harness, at: CellRef) -> TestResult {
    let sh = sheet(h)?;
    let geo = grid::Geo::new(sh, h.state().grid.rect.ok_or("grid rect")?, h.state().view().scroll);
    let pos = geo.cell_rect(sh, at).center();
    h.input_mut().events.push(Event::PointerMoved(pos));
    for pressed in [true, false] {
        h.input_mut().events.push(Event::PointerButton { pos, button: PointerButton::Secondary, pressed, modifiers: Modifiers::NONE });
    }
    h.run_steps(4);
    assert!(h.state().grid.context_menu.is_some(), "right-click opens the real cell menu");
    press(h, "Filter by Selected Cell's Value")
}

#[test]
fn context_filter_by_value_with_buttons_off_undoes_and_redoes_as_one_step() -> TestResult {
    // GridCraft's existing targeting and button behavior; this Excel path is unmeasured.
    for has_worksheet_filter in [true, false] {
        let mut s = Session::new();
        s.new_workbook();
        s.execute("range.setValues", json!({"range":"A1", "values":[["Name", "Region"], ["Ann", "West"], ["Ben", "West"], ["Cat", "East"]]}))?;
        s.execute("insert.table", json!({"range":"A1:B4", "header":true, "name":"People"}))?;
        s.execute("data.filter", json!({}))?;
        s.execute("insert.slicer", json!({"table":"People", "column":"Region", "at":"J2"}))?;
        s.execute("slicer.toggle", json!({"value":"West"}))?;
        if has_worksheet_filter {
            s.execute("selection.set", json!({"range":"F1"}))?;
            s.execute("data.filter", json!({"range":"A1:B4"}))?;
        }
        let mut h = harness(s);
        let state = |h: &Harness| -> TestResult<_> {
            let sh = sheet(h)?;
            let t = sh.tables.iter().find(|t| t.name == "People").ok_or("People")?;
            Ok((t.filter_button, t.filter.clone(), sh.autofilter.as_ref().map(|af| (af.range.a1(), af.criteria.clone())), hidden(h)?))
        };
        let before = (false, vec![(1, values(&["West"]))], has_worksheet_filter.then(|| ("A1:B4".into(), vec![])), vec![4]);
        let after = (!has_worksheet_filter, vec![(1, values(&["West"])), (0, values(&["Ann"]))], None, vec![3, 4]);
        assert_eq!(state(&h)?, before);
        let depth = undo_depth(&h)?;
        filter_by_cell_value(&mut h, CellRef::new(1, 0))?;
        assert_eq!(state(&h)?, after);
        h.state_mut().run("edit.undo", json!({}))?;
        assert_eq!(state(&h)?, before, "one undo must also restore the table's filter buttons");
        assert_eq!(undo_depth(&h)?, depth);
        h.state_mut().run("edit.redo", json!({}))?;
        assert_eq!(state(&h)?, after);
        assert_eq!(undo_depth(&h)?, depth + 1);
    }
    Ok(())
}

/// The worksheet AutoFilter's range and criteria, or `None` without one.
type SheetFilter = Option<(String, Vec<(u32, FilterCriterion)>)>;

fn autofilter(h: &Harness) -> TestResult<SheetFilter> {
    Ok(sheet(h)?.autofilter.as_ref().map(|af| (af.range.a1(), af.criteria.clone())))
}

fn undo_label(h: &Harness) -> TestResult<String> {
    Ok(h.state().session.active().ok_or("active document")?.undo.last().ok_or("no undo step")?.label.clone())
}

/// Name/Q in A1:B4: no table, no worksheet AutoFilter.
fn plain_range() -> TestResult<Session> {
    let mut s = Session::new();
    s.new_workbook();
    s.execute("range.setValues", json!({"range":"A1", "values":[["Name","Q"],["Ann",1],["Ben",2],["Cat",3]]}))?;
    Ok(s)
}

#[test]
fn context_filter_by_value_on_a_plain_range_undoes_and_redoes_as_one_step() -> TestResult {
    let mut h = harness(plain_range()?);
    let depth = undo_depth(&h)?;
    let after = Some(("A1:B4".to_string(), vec![(0, values(&["Ann"]))]));
    filter_by_cell_value(&mut h, CellRef::new(1, 0))?;
    assert_eq!(autofilter(&h)?, after, "the action creates the worksheet AutoFilter with the criterion");
    assert_eq!(hidden(&h)?, [3, 4]);
    assert_eq!(undo_depth(&h)?, depth + 1);
    // The undo list names the step as it does when the action is a single `data.filterBy`.
    let label = gridcraft_engine::find_command("data.filterBy").ok_or("data.filterBy")?.label;
    assert_eq!(undo_label(&h)?, label);
    h.state_mut().run("edit.undo", json!({}))?;
    assert_eq!(autofilter(&h)?, None, "one undo also removes the AutoFilter it created");
    assert_eq!(hidden(&h)?, Vec::<u32>::new());
    assert_eq!(undo_depth(&h)?, depth);
    h.state_mut().run("edit.redo", json!({}))?;
    assert_eq!(autofilter(&h)?, after);
    assert_eq!(hidden(&h)?, [3, 4]);
    assert_eq!(undo_label(&h)?, label);
    Ok(())
}

#[test]
fn context_filter_by_value_whose_criterion_fails_rolls_its_setup_back() -> TestResult {
    let mut s = plain_range()?;
    s.execute("range.setValues", json!({"range":"B2", "values":[[9]]}))?;
    s.execute("edit.undo", json!({}))?;
    // Column C is empty: the AutoFilter `data.filter` installs over the selection's data (A1:B4) leaves out the
    // active cell's column, and `data.filterBy` then refuses it.
    s.execute("selection.set", json!({"range":"A1:C4", "active":"C1"}))?;
    let mut h = harness(s);
    let depth = undo_depth(&h)?;
    filter_by_cell_value(&mut h, CellRef::new(0, 2))?;
    let message = h.state().message.as_ref().map(|(_, m)| m.clone()).ok_or("the failure is reported")?;
    assert!(message.contains("column outside the filter range"), "{message}");
    assert_eq!(autofilter(&h)?, None, "no AutoFilter is left behind without its criterion");
    assert_eq!(undo_depth(&h)?, depth, "a failed action adds no undo step");
    assert_eq!(h.state().session.active().ok_or("active document")?.redo.len(), 1, "and keeps the redo history");
    Ok(())
}

#[test]
fn popup_closes_when_the_worksheet_filter_range_grows() -> TestResult {
    let mut h = sheet_filter_and_table()?;
    open_menu(&mut h, CellRef::new(0, 0))?;
    press(&mut h, "Ben")?;
    h.state_mut().run("cell.set", json!({"cell":"A5", "input":"Gil"}))?;
    h.run_steps(2);
    assert_eq!(sheet(&h)?.autofilter.as_ref().ok_or("worksheet filter")?.range.a1(), "A1:B5");
    if h.query_by_label("Apply").is_some() {
        press(&mut h, "Apply")?;
    }
    assert_eq!(sheet_criteria(&h)?, Some(vec![]), "stale checkboxes must not filter the enlarged range");
    assert!(h.state().grid.filter_menu.is_none());
    Ok(())
}

#[test]
fn resized_worksheet_filter_rejects_pending_sort_clear_and_enter() -> TestResult {
    for action in ["↓  Sort Descending", "Clear Filter", "Enter"] {
        let mut h = sheet_filter_and_table()?;
        h.state_mut().run("data.filterBy", json!({"column":"A", "values":["Ann", "Cat"]}))?;
        h.run_steps(2);
        open_menu(&mut h, CellRef::new(0, 0))?;
        press(&mut h, "Ann")?;
        h.state_mut().run("cell.set", json!({"cell":"A5", "input":"Gil"}))?;
        let before = sheet(&h)?.autofilter.clone();
        let depth = undo_depth(&h)?;
        if action == "Enter" {
            h.input_mut().events.push(Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
            h.run_steps(2);
        } else {
            h.run_steps(2);
            if h.query_by_label(action).is_some() {
                press(&mut h, action)?;
            }
        }
        assert!(h.state().grid.filter_menu.is_none(), "{action}: range change closes the popup");
        assert_eq!(undo_depth(&h)?, depth, "{action}: no command acted");
        assert_eq!(sheet(&h)?.autofilter, before, "{action}: criteria are unchanged");
    }
    Ok(())
}

#[test]
fn worksheet_filter_recreated_at_the_same_range_between_frames_is_the_same_target() -> TestResult {
    // GridCraft choice: worksheet filters have no identity apart from sheet and exact range.
    let mut h = sheet_filter_and_table()?;
    open_menu(&mut h, CellRef::new(0, 0))?;
    press(&mut h, "Ben")?;
    h.state_mut().run("data.filter", json!({"range":"A1:B4"}))?;
    assert_eq!(sheet_criteria(&h)?, None);
    h.state_mut().run("data.filter", json!({"range":"A1:B4"}))?;
    h.run_steps(2);
    press(&mut h, "Apply")?;
    assert_eq!(sheet_criteria(&h)?, Some(vec![(0, values(&["Ann", "Cat"]))]));
    assert_eq!(hidden(&h)?, [3]);
    Ok(())
}

#[test]
fn popup_keeps_its_table_after_rename_but_closes_when_the_header_moves() -> TestResult {
    let mut h = sheet_filter_and_table()?;
    open_menu(&mut h, CellRef::new(7, 0))?;
    press(&mut h, "Eve")?;
    h.state_mut().run("table.rename", json!({"table":"People", "name":"Staff"}))?;
    h.run_steps(2);
    press(&mut h, "Apply")?;
    assert_eq!(table_criteria(&h, "Staff")?, [(0, values(&["Dan", "Fay"]))]);
    open_menu(&mut h, CellRef::new(7, 0))?;
    h.state_mut().run("table.resize", json!({"table":"Staff", "range":"B8:B12"}))?;
    h.run_steps(2);
    assert!(h.state().grid.filter_menu.is_none(), "the table no longer owns A8");
    assert_eq!(sheet_criteria(&h)?, Some(vec![]));
    Ok(())
}

#[test]
fn header_only_table_popup_has_no_data_values() -> TestResult {
    let mut s = Session::new();
    s.new_workbook();
    s.execute("cell.set", json!({"cell":"A1", "input":"Header"}))?;
    s.execute("insert.table", json!({"range":"A1", "header":true, "name":"EmptyTable"}))?;
    let mut h = harness(s);
    open_menu(&mut h, CellRef::new(0, 0))?;
    assert_eq!(sheet_criteria(&h)?, None, "opening the empty table's popup installs no filter");
    assert!(h.query_by_label("(Blanks)").is_none(), "a header-only table has no blank data row");
    press(&mut h, "Apply")?;
    assert_eq!(table_criteria(&h, "EmptyTable")?, []);
    assert_eq!(hidden(&h)?, Vec::<u32>::new());
    Ok(())
}

fn overlapping_worksheet_and_slicer_filters() -> TestResult<Harness> {
    // An explicit Data > Filter range can be set from a selection outside the table.
    let mut s = Session::new();
    s.new_workbook();
    s.execute("range.setValues", json!({"range":"A1", "values":[["Name"], ["Ann"], ["Ben"], ["Cat"]]}))?;
    s.execute("insert.table", json!({"range":"A1:A4", "header":true, "name":"People"}))?;
    s.execute("data.filter", json!({}))?;
    s.execute("insert.slicer", json!({"table":"People", "column":"Name", "at":"J2"}))?;
    s.execute("slicer.toggle", json!({"value":"Ann"}))?;
    s.execute("selection.set", json!({"range":"F1"}))?;
    s.execute("data.filter", json!({"range":"A1:A4"}))?;
    Ok(harness(s))
}

#[test]
fn worksheet_popup_applies_only_to_the_worksheet_filter() -> TestResult {
    let mut h = overlapping_worksheet_and_slicer_filters()?;
    open_menu(&mut h, CellRef::new(0, 0))?;
    // Ann and Ben start checked for the worksheet, though the table slicer hides Ben.
    press(&mut h, "Cat")?;
    press(&mut h, "Apply")?;
    assert_eq!(sheet_criteria(&h)?, Some(vec![(0, values(&["Ann", "Ben"]))]), "Apply must target only the worksheet filter");
    assert_eq!(table_criteria(&h, "People")?, [(0, values(&["Ann"]))]);
    assert_eq!(hidden(&h)?, [3, 4]);
    Ok(())
}

#[test]
fn worksheet_popup_clears_only_the_worksheet_filter() -> TestResult {
    let mut h = overlapping_worksheet_and_slicer_filters()?;
    open_menu(&mut h, CellRef::new(0, 0))?;
    press(&mut h, "Clear Filter")?;
    assert_eq!(table_criteria(&h, "People")?, [(0, values(&["Ann"]))], "worksheet Clear left the table's native filter alone");
    assert_eq!(sheet_criteria(&h)?, Some(vec![]));
    assert_eq!(hidden(&h)?, [3, 4]);
    Ok(())
}
