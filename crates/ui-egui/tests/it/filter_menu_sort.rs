//! Real filter-button and menu clicks over in-memory workbooks. No fixture files.

use crate::common::{self, Harness};
use egui::{Event, Modifiers, PointerButton, Pos2, vec2};
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
