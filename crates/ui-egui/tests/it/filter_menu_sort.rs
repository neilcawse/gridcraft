//! Real filter-button and menu clicks over in-memory workbooks. No fixture files.

use crate::common::{self, Harness};
use egui::{Event, Key, Modifiers, PointerButton, Pos2, vec2};
use egui_kittest::kittest::Queryable;
use gridcraft_engine::Session;
use gridcraft_engine::core::{CellRef, RangeRef};
use gridcraft_engine::model::Sheet;
use gridcraft_ui_egui::grid;
use serde_json::json;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn sheet(h: &Harness) -> TestResult<&Sheet> {
    Ok(h.state().session.active().and_then(|d| d.wb.active()).ok_or("active sheet")?)
}

fn names(h: &Harness) -> TestResult<Vec<String>> {
    let sh = sheet(h)?;
    Ok((0..4).map(|row| sh.value(CellRef::new(row, 0)).display()).collect())
}

fn fixture(table: bool, selection: &str) -> TestResult<Harness> {
    let mut s = Session::new();
    s.new_workbook();
    s.execute("selection.set", json!({"range":selection}))?;
    s.execute("range.setValues", json!({"range":"A1", "values":[["Name", "Status"], ["Zoe", "Active"], ["Bob", "Inactive"], ["Amy", "Active"]]}))?;
    s.execute("range.setValues", json!({"range":"D1", "values":[[3, "three"], [1, "one"], [2, "two"]]}))?;
    if table {
        s.execute("range.setValues", json!({"range":"A1", "values":[["Name", "Score"], ["Zoe", 4], ["Bob", 1], ["Amy", 5]]}))?;
        s.execute("insert.table", json!({"range":"A1:B4", "header":true, "name":"Scores"}))?;
        s.execute("table.totalRow", json!({"table":"Scores", "on":true}))?;
        s.execute("range.setValues", json!({"range":"H1", "values":[["Other"], ["Y"], ["X"]]}))?;
        s.execute("data.filter", json!({"range":"H1:H3"}))?;
    } else {
        s.execute("data.filter", json!({"range":"A1:B4"}))?;
        s.execute("data.filterBy", json!({"column":"B", "values":["Active"]}))?;
    }
    Ok(common::harness_with(common::builder().with_size(vec2(1200.0, 800.0)), common::app(s), 4))
}

fn click(h: &mut Harness, pos: Pos2) {
    h.input_mut().events.push(Event::PointerMoved(pos));
    for pressed in [true, false] {
        h.input_mut().events.push(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
    }
    h.run_steps(4);
}

fn sort_from_button(h: &mut Harness, header: CellRef, label: &str) -> TestResult {
    let before = h.state().session.active().ok_or("active document")?.selection.clone();
    let sh = sheet(h)?;
    let geo = grid::Geo::new(sh, h.state().grid.rect.ok_or("grid rect")?, h.state().view().scroll);
    click(h, grid::filter_button_rect(&geo, sh, header).center());
    assert!(h.state().grid.filter_menu.is_some(), "header button opens its popup");
    assert_eq!(h.state().session.active().ok_or("active document")?.selection, before);
    let button = h.get_by_label(label).rect().center();
    click(h, button);
    assert!(h.state().grid.filter_menu.is_none(), "sort closes the popup");
    assert_eq!(h.state().session.active().ok_or("active document")?.selection, before);
    Ok(())
}

#[test]
fn all_text_filter_header_stays_put() -> TestResult {
    let mut h = fixture(false, "A1")?;
    assert!(sheet(&h)?.is_row_hidden(2));
    sort_from_button(&mut h, CellRef::new(0, 0), "↑  Sort Ascending")?;
    assert_eq!(names(&h)?, ["Name", "Amy", "Bob", "Zoe"], "the header and filtered-out Bob must stay in place");
    assert!(sheet(&h)?.is_row_hidden(2));
    assert_eq!(sheet(&h)?.value(CellRef::new(2, 1)).display(), "Inactive");
    Ok(())
}

#[test]
fn worksheet_filter_ignores_other_selected_block_and_sort_can_be_undone_and_redone() -> TestResult {
    let mut h = fixture(false, "D1:E3")?;
    let other_range = RangeRef::parse("D1:E3").ok_or("other range")?;
    let other = |h: &Harness| -> TestResult<_> {
        let sh = sheet(h)?;
        Ok(other_range.iter().map(|c| sh.value(c)).collect::<Vec<_>>())
    };
    let before = other(&h)?;
    let selection = h.state().session.active().ok_or("active document")?.selection.clone();
    sort_from_button(&mut h, CellRef::new(0, 0), "↑  Sort Ascending")?;
    assert_eq!(other(&h)?, before, "the unrelated selected block must not be sorted");
    assert_eq!(names(&h)?, ["Name", "Amy", "Bob", "Zoe"]);
    h.state_mut().run("edit.undo", json!({}))?;
    assert_eq!(names(&h)?, ["Name", "Zoe", "Bob", "Amy"]);
    assert!(sheet(&h)?.is_row_hidden(2));
    assert_eq!(h.state().session.active().ok_or("active document")?.selection, selection);
    h.state_mut().run("edit.redo", json!({}))?;
    assert_eq!(names(&h)?, ["Name", "Amy", "Bob", "Zoe"]);
    assert!(sheet(&h)?.is_row_hidden(2));
    assert_eq!(h.state().session.active().ok_or("active document")?.selection, selection);
    assert_eq!(other(&h)?, before);
    Ok(())
}

#[test]
fn table_filter_sorts_clicked_column_descending_from_outside_the_table() -> TestResult {
    let mut h = fixture(true, "D1:E3")?;
    sort_from_button(&mut h, CellRef::new(0, 1), "↓  Sort Descending")?;
    // Descending by name would put Zoe first. Sorting the totals row would move its sum (10).
    assert_eq!(names(&h)?, ["Name", "Amy", "Zoe", "Bob"], "sort the clicked table's data by Score");
    assert_eq!(sheet(&h)?.value(CellRef::new(4, 0)).display(), "Total");
    assert_eq!(sheet(&h)?.value(CellRef::new(4, 1)).as_f64(), Some(10.0));
    assert_eq!(sheet(&h)?.value(CellRef::new(0, 4)).display(), "three");
    assert_eq!(sheet(&h)?.value(CellRef::new(1, 7)).display(), "Y");
    Ok(())
}

fn harness(s: Session) -> Harness {
    common::harness_with(common::builder().with_size(vec2(1200.0, 800.0)), common::app(s), 4)
}

fn column_of(h: &Harness, sheet: usize, col: u32, rows: u32) -> TestResult<Vec<String>> {
    let sh = h.state().session.active().and_then(|d| d.wb.sheet(sheet)).ok_or("sheet")?;
    Ok((0..rows).map(|row| sh.value(CellRef::new(row, col)).display()).collect())
}

fn open_menu(h: &mut Harness, header: CellRef) -> TestResult {
    let sh = sheet(h)?;
    let geo = grid::Geo::new(sh, h.state().grid.rect.ok_or("grid rect")?, h.state().view().scroll);
    click(h, grid::filter_button_rect(&geo, sh, header).center());
    assert!(h.state().grid.filter_menu.is_some(), "header button opens its popup");
    Ok(())
}

/// Clicks Sort Ascending if the popup still shows it.
fn click_ascending_if_shown(h: &mut Harness) {
    if let Some(button) = h.query_by_label("↑  Sort Ascending").map(|b| b.rect().center()) {
        click(h, button);
    }
}

fn filter_on_two_sheets() -> TestResult<Harness> {
    let mut s = Session::new();
    s.new_workbook();
    s.execute("range.setValues", json!({"range":"A1", "values":[["N"], ["c"], ["a"], ["b"]]}))?;
    s.execute("data.filter", json!({"range":"A1:A4"}))?;
    s.execute("home.insertSheet", json!({}))?;
    s.execute("range.setValues", json!({"range":"A1", "values":[["M"], ["z"], ["x"], ["y"]]}))?;
    s.execute("data.filter", json!({"range":"A1:A4"}))?;
    s.execute("sheet.activate", json!({"sheet": 0}))?;
    Ok(harness(s))
}

#[test]
fn popup_left_open_across_a_sheet_switch_sorts_neither_sheet() -> TestResult {
    let mut h = filter_on_two_sheets()?;
    open_menu(&mut h, CellRef::new(0, 0))?;
    h.input_mut().events.push(Event::Text("a".into()));
    h.run_steps(2);
    let value = h.get_by_label("a").rect().center();
    click(&mut h, value);
    // Not a click (that would dismiss the popup): a control client switches sheets.
    h.state_mut().run("sheet.activate", json!({"sheet": 1}))?;
    h.run_steps(2);
    click_ascending_if_shown(&mut h);
    assert!(h.state().grid.filter_menu.is_none(), "the popup closes with the sheet it opened on");
    assert_eq!(column_of(&h, 0, 0, 4)?, ["N", "c", "a", "b"]);
    assert_eq!(column_of(&h, 1, 0, 4)?, ["M", "z", "x", "y"], "the newly shown sheet's filter is not sorted");
    open_menu(&mut h, CellRef::new(0, 0))?;
    assert!(h.query_by_label("(Select All)").is_some(), "the new popup has an empty search");
    assert!(h.query_by_label("x").is_some(), "the new popup lists values from its own sheet");
    Ok(())
}

#[test]
fn keyboard_popup_cannot_bind_to_a_sheet_activated_in_the_same_frame() -> TestResult {
    let mut h = filter_on_two_sheets()?;
    h.state_mut().run("selection.set", json!({"range":"A1"}))?;
    for (key, modifiers) in [(Key::ArrowDown, Modifiers::ALT), (Key::PageDown, Modifiers::COMMAND)] {
        for pressed in [true, false] {
            h.input_mut().events.push(Event::Key { key, physical_key: Some(key), pressed, repeat: false, modifiers });
        }
    }
    h.run_steps(2);
    let doc = h.state().session.active().ok_or("active document")?;
    assert_eq!(doc.wb.active_sheet(), 1, "the keyboard switched sheets before the popup drew");
    click_ascending_if_shown(&mut h);
    assert_eq!(column_of(&h, 1, 0, 4)?, ["M", "z", "x", "y"], "the popup must not sort the newly activated sheet");
    assert!(h.state().grid.filter_menu.is_none());
    Ok(())
}

#[test]
fn popup_whose_filter_was_removed_sorts_nothing() -> TestResult {
    let mut s = Session::new();
    s.new_workbook();
    s.execute("range.setValues", json!({"range":"A1", "values":[["N"], ["c"], ["a"], ["b"]]}))?;
    s.execute("data.filter", json!({"range":"A1:A4"}))?;
    let mut h = harness(s);
    open_menu(&mut h, CellRef::new(0, 0))?;
    h.state_mut().run("data.filter", json!({}))?;
    h.run_steps(2);
    assert!(sheet(&h)?.autofilter.is_none());
    click_ascending_if_shown(&mut h);
    assert_eq!(column_of(&h, 0, 0, 4)?, ["N", "c", "a", "b"], "no filter owns the header any more");
    Ok(())
}

#[test]
fn table_away_from_column_a_sorts_by_the_clicked_column_and_keeps_its_totals() -> TestResult {
    let mut s = Session::new();
    s.new_workbook();
    s.execute("range.setValues", json!({"range":"C3", "values":[["Name", "Score"], ["Zoe", 4], ["Bob", 1], ["Amy", 5]]}))?;
    s.execute("insert.table", json!({"range":"C3:D6", "header":true, "name":"Offset"}))?;
    s.execute("table.totalRow", json!({"table":"Offset", "on":true}))?;
    s.execute("selection.set", json!({"range":"A1"}))?;
    let mut h = harness(s);
    sort_from_button(&mut h, CellRef::new(2, 3), "↑  Sort Ascending")?;
    assert_eq!(column_of(&h, 0, 2, 8)?, ["", "", "Name", "Bob", "Zoe", "Amy", "Total", ""]);
    assert_eq!(column_of(&h, 0, 3, 8)?, ["", "", "Score", "1", "4", "5", "10", ""]);
    Ok(())
}
