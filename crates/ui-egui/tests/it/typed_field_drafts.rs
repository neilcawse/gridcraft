//! Text typed into an in-place field and not yet entered (Enter, a click elsewhere) is unsaved work:
//! the sheet tab's name field, the Table Design name box, the Format Chart title, axis title and
//! size boxes, Options › Formulas and the Info properties. Save, the File commands, Close and Quit
//! enter it first, into the workbook and sheet it was typed in, and the file has what was typed.
//!
//! Everything is driven through real input (the tab's context menu, typing, ⌘S), with the page
//! keeping the files as the browser extension does: ⌘S there is a `file.save` request.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use egui::accesskit::Role;
use egui::{Event, Key, Modifiers, PointerButton, Pos2, vec2};
use egui_kittest::kittest::Queryable as _;
use gridcraft_engine::Session;
use gridcraft_engine::model::Workbook;
use gridcraft_ui_egui::{HostSave, Services, SheetApp};
use serde_json::json;

type Harness = egui_kittest::Harness<'static, SheetApp>;

/// The workbooks a page that keeps the files was handed, in order ([`Services::host_save`]).
type Handed = Rc<RefCell<Vec<Arc<Workbook>>>>;

/// A harness around `session` in a page that keeps the files: a save hands the workbook over and
/// counts as done. The workbook starts saved, so only typing is unsaved work.
fn hosted_of(session: Session) -> (Harness, Handed) {
    let handed = Handed::default();
    let sink = Rc::clone(&handed);
    let services = Services {
        host_save: Some(Box::new(move |save: &HostSave<'_>| {
            sink.borrow_mut().push(Arc::clone(&save.content));
            Ok(())
        })),
        ..Default::default()
    };
    let mut app = crate::common::prepare(SheetApp::new(session, services));
    let d = app.session.active_mut().unwrap();
    d.saved = Arc::clone(&d.wb);
    let h = crate::common::harness_with(
        crate::common::builder().with_size(vec2(1200.0, 800.0)).with_step_dt(1.0 / 60.0).with_os(egui::os::OperatingSystem::Mac),
        app,
        4,
    );
    (h, handed)
}

/// Alpha and Beta, Alpha showing, with numbers and a small table to build a PivotTable from.
fn two_sheets() -> (Harness, Handed) {
    let mut s = Session::new();
    s.new_workbook();
    for (cell, input) in [("C1", "Region"), ("D1", "Sales"), ("C2", "North"), ("D2", "5"), ("C3", "South"), ("D3", "7"), ("E1", "=SUM(A1:A2)")] {
        s.execute("cell.set", json!({"cell": cell, "input": input})).unwrap();
    }
    s.execute("range.setValues", json!({"range": "A1", "values": (1..=60).map(|i| vec![i]).collect::<Vec<_>>()})).unwrap();
    s.execute("sheet.rename", json!({"name": "Alpha"})).unwrap();
    s.execute("home.insertSheet", json!({"name": "Beta"})).unwrap();
    s.execute("sheet.activate", json!({"sheet": 0})).unwrap();
    hosted_of(s)
}

fn names(wb: &Workbook) -> Vec<String> {
    wb.sheets.iter().map(|s| s.name.clone()).collect()
}

fn tabs(h: &Harness) -> Vec<String> {
    names(&h.state().session.doc().unwrap().wb)
}

fn last_saved(handed: &Handed) -> Arc<Workbook> {
    handed.borrow().last().cloned().expect("the page was handed the file")
}

/// Where the tab named `name` was painted on the strip.
fn tab_centre(h: &Harness, name: &str) -> Pos2 {
    h.output()
        .shapes
        .iter()
        .find_map(|s| match &s.shape {
            egui::Shape::Text(t) if t.galley.job.text == name => Some(t.visual_bounding_rect().center()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("the {name} tab was not painted"))
}

fn key(h: &mut Harness, key: Key, modifiers: Modifiers) {
    for pressed in [true, false] {
        h.input_mut().events.push(Event::Key { key, physical_key: Some(key), pressed, repeat: false, modifiers });
    }
}

/// ⌘S, as the keyboard sends it.
fn cmd_s(h: &mut Harness) {
    key(h, Key::S, Modifiers::COMMAND);
    h.run_steps(3);
}

/// Types `text` over what the focused field holds, without Enter.
fn type_over(h: &mut Harness, text: &str) {
    key(h, Key::A, Modifiers::COMMAND);
    h.run_steps(2);
    if text.is_empty() {
        key(h, Key::Backspace, Modifiers::NONE);
    } else {
        for c in text.chars() {
            h.input_mut().events.push(Event::Text(c.to_string()));
            h.run_steps(2);
        }
    }
    h.run_steps(2);
}

/// Right-click Rename on the tab of `sheet`, then types `text` over its name: a draft, not yet in
/// the workbook.
fn rename_typed(h: &mut Harness, sheet: &str, text: &str) {
    let at = tab_centre(h, sheet);
    h.input_mut().events.extend([
        Event::PointerMoved(at),
        Event::PointerButton { pos: at, button: PointerButton::Secondary, pressed: true, modifiers: Modifiers::NONE },
    ]);
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: at, button: PointerButton::Secondary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(20);
    h.get_by_role_and_label(Role::Button, "Rename").click();
    h.run_steps(4);
    type_over(h, text);
    assert!(h.state().grid.renaming_tab.is_some(), "setup: the name field is open");
    assert!(h.state().has_unsaved_work(), "setup: the typed name would be lost");
    assert!(!h.state().session.active().unwrap().is_dirty(), "setup: it is not in the workbook yet");
    assert!(tabs(h).iter().any(|n| n == sheet), "setup: the sheet still has its name");
}

fn button(h: &mut Harness, label: &str) {
    h.get_by_role_and_label(Role::Button, label).click();
    h.run_steps(3);
}

// ---------------------------------------------------------------- the sheet tab's name field

/// ⌘S while the name is being typed (the field has the keyboard, so the grid's shortcut stands
/// aside) saves the file with the new name.
#[test]
fn the_save_shortcut_while_typing_a_sheet_name_saves_the_new_name() {
    let (mut h, handed) = two_sheets();
    rename_typed(&mut h, "Alpha", "Budget");
    cmd_s(&mut h);
    assert_eq!(handed.borrow().len(), 1, "the page was handed the file");
    assert_eq!(names(&last_saved(&handed)), ["Budget", "Beta"], "the file has the name that was typed");
    assert!(h.state().grid.renaming_tab.is_none(), "the name is entered");
    assert_eq!(tabs(&h), ["Budget", "Beta"]);
    assert!(!h.state().has_unsaved_work(), "nothing is left to lose");
}

/// The extension sends ⌘S to the page as a save request: the same.
#[test]
fn a_save_request_while_typing_a_sheet_name_saves_the_new_name() {
    let (mut h, handed) = two_sheets();
    rename_typed(&mut h, "Alpha", "Budget");
    h.state_mut().run_or_alert("file.save", json!({}));
    h.run_steps(3);
    assert_eq!(names(&last_saved(&handed)), ["Budget", "Beta"]);
    assert!(!h.state().has_unsaved_work());
}

/// Close with a name being typed asks, as it does for any unsaved change; Cancel keeps both.
#[test]
fn closing_while_typing_a_sheet_name_asks_to_save_it() {
    let (mut h, _) = two_sheets();
    rename_typed(&mut h, "Alpha", "Budget");
    let typed_in = h.state().session.active().unwrap().uid;
    h.state_mut().run_or_alert("file.close", json!({}));
    h.run_steps(3);
    let open = |h: &Harness| h.state().session.documents().iter().any(|d| d.uid == typed_in);
    assert!(open(&h), "the workbook is still open");
    assert_eq!(h.state().dialog.as_ref().map(|d| d.name()), Some("saveChanges"), "Close asks");
    assert_eq!(tabs(&h), ["Budget", "Beta"], "the typed name is in the workbook the question is about");

    button(&mut h, "Cancel");
    assert!(open(&h), "Cancel keeps the workbook");
    assert_eq!(tabs(&h), ["Budget", "Beta"], "and the name");
    assert!(h.state().has_unsaved_work());
}

/// Closing the window or quitting asks too.
#[test]
fn quitting_while_typing_a_sheet_name_asks() {
    let (mut h, _) = two_sheets();
    rename_typed(&mut h, "Alpha", "Budget");
    let ctx = h.ctx.clone();
    h.state_mut().request_quit(&ctx);
    h.run_steps(3);
    assert_eq!(h.state().dialog.as_ref().map(|d| d.name()), Some("saveChanges"), "Quit asks about the workbook");
    assert_eq!(tabs(&h), ["Budget", "Beta"]);
}

/// A name the workbook refuses (blank, taken, with a character sheets can't have) is not saved
/// and not dropped: Save says why, writes nothing, and the typed name stays in the field.
#[test]
fn a_sheet_name_the_workbook_refuses_is_kept_and_reported_not_saved() {
    for (typed, why) in [("", "blank"), ("Beta", "taken"), ("beta", "taken in another case"), ("a/b", "a character sheets can't have")] {
        let (mut h, handed) = two_sheets();
        rename_typed(&mut h, "Alpha", typed);
        cmd_s(&mut h);
        assert!(handed.borrow().is_empty(), "{why}: nothing was saved");
        assert!(h.state().grid.renaming_tab.is_some(), "{why}: the typed name is kept");
        assert!(h.state().message.is_some(), "{why}: the reason is shown");
        assert_eq!(tabs(&h), ["Alpha", "Beta"], "{why}: no wrong name was entered");
        assert!(h.state().has_unsaved_work(), "{why}: still unsaved");
    }
}

/// The same through the Close question's Save and the File tab.
#[test]
fn the_close_questions_save_enters_the_sheet_name_being_typed() {
    let (mut h, handed) = two_sheets();
    rename_typed(&mut h, "Alpha", "Budget");
    h.state_mut().run_or_alert("file.close", json!({}));
    h.run_steps(3);
    button(&mut h, "Save");
    assert_eq!(names(&last_saved(&handed)), ["Budget", "Beta"], "the file has the name");
}

/// The draft is for the sheet it was typed in, not for the place the sheet had among the tabs:
/// when the tabs move under it without entering it first (a script), it goes into its own sheet.
#[test]
fn a_sheet_name_being_typed_stays_with_its_sheet_when_the_tabs_move() {
    let (mut h, handed) = two_sheets();
    rename_typed(&mut h, "Alpha", "Budget");
    h.state_mut().session.run("sheet.move", json!({"sheet": 0, "to": 1})).unwrap();
    assert_eq!(tabs(&h), ["Beta", "Alpha"], "setup: Alpha is second now");
    h.run_steps(3);
    h.state_mut().run_or_alert("file.save", json!({}));
    h.run_steps(3);
    assert_eq!(names(&last_saved(&handed)), ["Beta", "Budget"], "the name went to the sheet it was typed in");
    assert!(!h.state().has_unsaved_work());
}

/// Every command that changes the tabs enters the name first, into its own sheet.
#[test]
fn a_sheet_name_being_typed_is_entered_into_its_own_sheet_however_the_tabs_change() {
    let pivot = json!({"source": "Alpha!C1:D3", "rows": ["Region"], "values": ["Sales"]});
    let forecast = json!({"timeline": "A1:A60", "values": "A1:A60"});
    let solve = json!({"objective": "E1", "variables": "A1:A2", "constraints": ["E1 <= 20"], "method": "simplex", "reports": ["answer"]});
    let commands = [
        ("sheet.move", json!({"sheet": 0, "to": 1})),
        ("sheet.move", json!({"sheet": 1, "to": 0})),
        ("home.insertSheet", json!({"before": 0})),
        ("home.insertSheet", json!({})),
        ("home.deleteSheet", json!({"sheet": 1})),
        ("insert.pivotTable", pivot),
        ("data.forecastSheet", forecast),
        ("data.solver", solve),
        ("sheet.activate", json!({"sheet": 1})),
    ];
    let mut wrong = Vec::new();
    for (command, params) in commands {
        let (mut h, handed) = two_sheets();
        rename_typed(&mut h, "Alpha", "Typed");
        h.state_mut().run_or_alert(command, params.clone());
        h.run_steps(3);
        h.state_mut().run_or_alert("file.save", json!({}));
        h.run_steps(3);
        let saved = names(&last_saved(&handed));
        let beta = if command == "home.deleteSheet" { 0 } else { 1 };
        if saved.iter().filter(|n| *n == "Typed").count() != 1
            || saved.iter().any(|n| n == "Alpha")
            || saved.iter().filter(|n| *n == "Beta").count() != beta
        {
            wrong.push(format!("{command} {params}: the file has {saved:?}"));
        }
        if h.state().has_unsaved_work() {
            wrong.push(format!("{command} {params}: still unsaved after Save"));
        }
    }
    assert!(wrong.is_empty(), "a name went into another sheet, or was lost: {wrong:#?}");
}

/// Dragging another tab past the one being renamed: the name goes to the sheet it was typed in.
#[test]
fn dragging_a_tab_past_the_sheet_being_renamed_keeps_the_name_with_its_sheet() {
    let (mut h, handed) = two_sheets();
    // The tab being renamed paints no label, so where it is is noted first.
    let alpha = tab_centre(&h, "Alpha");
    rename_typed(&mut h, "Alpha", "Budget");
    let (from, to) = (tab_centre(&h, "Beta"), alpha);
    h.run_steps(40);
    h.input_mut().events.push(Event::PointerMoved(from));
    h.input_mut().events.push(Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    for i in 1..=4 {
        h.input_mut().events.push(Event::PointerMoved(from + (to - from) * (i as f32 / 4.0)));
        h.step();
    }
    h.input_mut().events.push(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(4);
    h.state_mut().run_or_alert("file.save", json!({}));
    h.run_steps(3);
    let saved = names(&last_saved(&handed));
    assert!(saved.iter().any(|n| n == "Budget") && saved.iter().any(|n| n == "Beta") && saved.len() == 2, "the file has {saved:?}");
    assert!(!h.state().has_unsaved_work());
}

/// File › New (a command) enters the name into the workbook it was typed in; the new workbook is untouched.
#[test]
fn a_sheet_name_being_typed_is_entered_before_another_workbook_comes_forward() {
    let (mut h, _) = two_sheets();
    rename_typed(&mut h, "Alpha", "Budget");
    let typed_in = h.state().session.active().unwrap().uid;
    h.state_mut().run_or_alert("file.new", json!({}));
    h.run_steps(4);
    let docs: Vec<(u64, Vec<String>)> = h.state().session.documents().iter().map(|d| (d.uid, names(&d.wb))).collect();
    assert_eq!(docs.iter().find(|(uid, _)| *uid == typed_in).map(|d| d.1.clone()), Some(vec!["Budget".into(), "Beta".into()]));
    assert!(docs.iter().filter(|(uid, _)| *uid != typed_in).all(|(_, n)| n.iter().all(|s| s != "Budget")), "{docs:?}");
    assert!(h.state().grid.renaming_tab.is_none(), "the field is closed");
}

/// A script that makes another workbook the active one doesn't take the draft with it: Enter then
/// does not rename a sheet of that workbook, and what was typed is said, not lost unseen.
#[test]
fn a_sheet_name_being_typed_never_lands_in_another_workbook() {
    let (mut h, _) = two_sheets();
    rename_typed(&mut h, "Alpha", "Budget");
    let typed_in = h.state().session.active().unwrap().uid;
    h.state_mut().session.run("file.new", json!({})).unwrap();
    let other = h.state().session.active().unwrap().uid;
    assert_ne!(other, typed_in, "setup: another workbook is the active one");
    let before = names(&h.state().session.active().unwrap().wb);
    h.run_steps(3);
    key(&mut h, Key::Enter, Modifiers::NONE);
    h.run_steps(4);
    let doc = |uid: u64| names(&h.state().session.documents().iter().find(|d| d.uid == uid).unwrap().wb);
    assert_eq!(doc(other), before, "the other workbook's sheets keep their names");
    assert_eq!(doc(typed_in), ["Alpha", "Beta"], "and the draft is not entered there by a mistake");
    assert!(!h.state().session.documents().iter().find(|d| d.uid == other).unwrap().is_dirty(), "the other workbook is untouched");
}

/// A name typed in a window that is not showing counts: Quit enters it and asks.
#[test]
fn quit_asks_about_a_sheet_name_being_typed_in_another_window() {
    let (mut h, _) = two_sheets();
    let first = h.state().session.active_window().unwrap();
    h.state_mut().run("view.newWindow", json!({})).unwrap();
    h.run_steps(4);
    assert_ne!(h.state().session.active_window(), Some(first), "setup: the second window is showing");
    rename_typed(&mut h, "Alpha", "Budget");
    h.state_mut().enter_window(first);
    h.run_steps(3);
    assert!(h.state().grid.renaming_tab.is_none(), "setup: the name field is the second window's");
    assert!(h.state().has_unsaved_work(), "the parked name would be lost");
    let ctx = h.ctx.clone();
    h.state_mut().request_quit(&ctx);
    h.run_steps(3);
    assert_eq!(h.state().dialog.as_ref().map(|d| d.name()), Some("saveChanges"), "Quit asks about the workbook");
    assert_eq!(tabs(&h), ["Budget", "Beta"], "the typed name is in the workbook");
}

/// Undo with a name being typed enters it first, as a click elsewhere does, and takes back that,
/// not the rename before it.
#[test]
fn undo_enters_the_sheet_name_being_typed_first() {
    let (mut h, handed) = two_sheets();
    h.state_mut().run_or_alert("sheet.rename", json!({"name": "Budget"}));
    h.run_steps(3);
    let d = h.state_mut().session.active_mut().unwrap();
    d.saved = Arc::clone(&d.wb);
    rename_typed(&mut h, "Budget", "Typed");
    h.state_mut().run_or_alert("edit.undo", json!({}));
    h.run_steps(3);
    assert_eq!(tabs(&h), ["Budget", "Beta"], "Undo took back the typed name, not the rename before it");
    assert!(h.state().grid.renaming_tab.is_none());
    h.state_mut().run_or_alert("edit.redo", json!({}));
    h.run_steps(3);
    assert_eq!(tabs(&h), ["Typed", "Beta"], "Redo brings it back");
    h.state_mut().run_or_alert("file.save", json!({}));
    h.run_steps(3);
    assert_eq!(names(&last_saved(&handed)), ["Typed", "Beta"]);
}

// ---------------------------------------------------------------- fields over workbook values

/// A table A1:B3 with a header on a blank sheet, selected in a cell of it.
fn table_session() -> Session {
    let mut s = Session::new();
    s.new_workbook();
    s.execute("range.setValues", json!({"range": "A1", "values": [["Name", "Qty"], ["a", 1], ["b", 2]]})).unwrap();
    s.execute("insert.table", json!({"range": "A1:B3", "header": true})).unwrap();
    s.execute("selection.set", json!({"range": "A2"})).unwrap();
    s
}

/// The text fields at the pane's side of the window (the formula bar and Name Box are further left).
fn pane_fields(h: &Harness) -> Vec<egui::Rect> {
    h.get_all_by_role(Role::TextInput).map(|n| n.rect()).filter(|r| r.left() > 850.0).collect()
}

/// Clicks `field`, replaces its text with `text`, without Enter.
fn type_into(h: &mut Harness, field: egui::Rect, text: &str) {
    h.input_mut().events.push(Event::PointerMoved(field.center()));
    h.run_steps(1);
    h.get_all_by_role(Role::TextInput).find(|n| n.rect() == field).unwrap().click();
    h.run_steps(2);
    type_over(h, text);
}

/// Saves two ways, as the keyboard and as the extension's request do, in fresh pages built by
/// `setup`, which types into a field; `read` gets the value from the saved workbook.
fn saves_what_was_typed<T: PartialEq + std::fmt::Debug>(setup: impl Fn() -> Harness2, want: T, read: impl Fn(&Workbook) -> T) {
    for route in ["⌘S", "save request"] {
        let (mut h, handed) = setup();
        assert!(h.state().has_unsaved_work(), "{route}: the typed text would be lost");
        assert!(!h.state().session.active().unwrap().is_dirty(), "{route}: it is not in the workbook yet");
        if route == "⌘S" {
            cmd_s(&mut h);
        } else {
            h.state_mut().run_or_alert("file.save", json!({}));
            h.run_steps(3);
        }
        assert_eq!(handed.borrow().len(), 1, "{route}: the page was handed the file");
        assert_eq!(read(&last_saved(&handed)), want, "{route}: the file has what was typed");
        assert!(!h.state().has_unsaved_work(), "{route}: nothing is left to lose");
    }
}

type Harness2 = (Harness, Handed);

#[test]
fn a_table_name_being_typed_is_saved_by_save() {
    saves_what_was_typed(
        || {
            let (mut h, handed) = hosted_of(table_session());
            h.state_mut().ui.ribbon_tab = "Table Design".into();
            h.run_steps(2);
            let label = h.get_by_label("Table Name:").rect();
            let below = |r: &egui::Rect| r.top() >= label.bottom() - 2.0 && (r.left() - label.left()).abs() < 20.0;
            let field = h.get_all_by_role(Role::TextInput).map(|n| n.rect()).find(below).unwrap();
            type_into(&mut h, field, "Sales");
            (h, handed)
        },
        "Sales".to_string(),
        |wb| wb.sheet(0).unwrap().tables[0].name.clone(),
    );
}

/// A table name the workbook refuses is kept in the field and reported; nothing is saved.
#[test]
fn a_table_name_the_workbook_refuses_is_kept_and_reported_not_saved() {
    let (mut h, handed) = hosted_of(table_session());
    h.state_mut().ui.ribbon_tab = "Table Design".into();
    h.run_steps(2);
    let label = h.get_by_label("Table Name:").rect();
    let below = |r: &egui::Rect| r.top() >= label.bottom() - 2.0 && (r.left() - label.left()).abs() < 20.0;
    let field = h.get_all_by_role(Role::TextInput).map(|n| n.rect()).find(below).unwrap();
    type_into(&mut h, field, "A-B");
    cmd_s(&mut h);
    assert!(handed.borrow().is_empty(), "nothing was saved");
    assert!(h.state().message.is_some(), "the reason is shown");
    assert_eq!(h.state().session.active().unwrap().wb.sheet(0).unwrap().tables[0].name, "Table1", "no wrong name was entered");
    assert!(h.state().has_unsaved_work(), "the typed name is still unsaved work");
}

/// A column chart on A1:B3 at D3 (300 × 200).
fn chart_session() -> (Session, u32) {
    let mut s = Session::new();
    s.new_workbook();
    s.execute("range.setValues", json!({"range": "A1", "values": [["Name", "Qty"], ["a", 1], ["b", 2]]})).unwrap();
    s.execute("selection.set", json!({"range": "A1:B3"})).unwrap();
    let chart = s.execute("insert.chart", json!({"type": "column", "at": "D3", "width": 300, "height": 200})).unwrap();
    (s, chart["chart"].as_u64().unwrap() as u32)
}

/// The Format Chart pane for chart `id`, with `section` expanded.
fn format_chart(h: &mut Harness, id: u32, section: &str) {
    h.state_mut().selected_chart = Some(id);
    h.state_mut().grid.pane = Some("formatChart".into());
    h.run_steps(2);
    h.get_by_role_and_label(Role::DisclosureTriangle, section).click();
    h.run_steps(2);
}

fn chart_of(wb: &Workbook) -> &gridcraft_engine::model::Chart {
    &wb.sheet(0).unwrap().charts[0]
}

#[test]
fn a_chart_title_being_typed_is_saved_by_save() {
    saves_what_was_typed(
        || {
            let (s, id) = chart_session();
            let (mut h, handed) = hosted_of(s);
            format_chart(&mut h, id, "Chart Title");
            let field = pane_fields(&h)[0];
            type_into(&mut h, field, "Sales");
            (h, handed)
        },
        Some("Sales".to_string()),
        |wb| chart_of(wb).title.clone(),
    );
}

#[test]
fn a_horizontal_axis_title_being_typed_is_saved_by_save() {
    saves_what_was_typed(
        || {
            let (s, id) = chart_session();
            let (mut h, handed) = hosted_of(s);
            format_chart(&mut h, id, "Elements");
            let field = pane_fields(&h)[0];
            type_into(&mut h, field, "Region");
            (h, handed)
        },
        Some("Region".to_string()),
        |wb| chart_of(wb).x_title.clone(),
    );
}

#[test]
fn a_vertical_axis_title_being_typed_is_saved_by_save() {
    saves_what_was_typed(
        || {
            let (s, id) = chart_session();
            let (mut h, handed) = hosted_of(s);
            format_chart(&mut h, id, "Elements");
            let field = pane_fields(&h)[1];
            type_into(&mut h, field, "Units");
            (h, handed)
        },
        Some("Units".to_string()),
        |wb| chart_of(wb).y_title.clone(),
    );
}

#[test]
fn a_chart_width_being_typed_is_saved_by_save() {
    saves_what_was_typed(
        || {
            let (s, id) = chart_session();
            let (mut h, handed) = hosted_of(s);
            format_chart(&mut h, id, "Size");
            let field = h.get_all_by_role(Role::SpinButton).next().unwrap().rect();
            h.input_mut().events.push(Event::PointerMoved(field.center()));
            for pressed in [true, false] {
                h.input_mut().events.push(Event::PointerButton {
                    pos: field.center(),
                    button: PointerButton::Primary,
                    pressed,
                    modifiers: Modifiers::NONE,
                });
                h.run_steps(2);
            }
            for digit in ["6", "4", "0"] {
                h.input_mut().events.push(Event::Text(digit.into()));
                h.run_steps(2);
            }
            (h, handed)
        },
        (640.0, 200.0),
        |wb| {
            let a = chart_of(wb).anchor;
            (a.width, a.height)
        },
    );
}

/// Alpha and Beta, each with a chart (ids are the workbook's, so they differ), Alpha showing in a
/// page that keeps the files; Alpha's chart's id.
fn two_charts() -> (Harness, Handed, u32) {
    let (mut s, id) = chart_session();
    s.execute("sheet.rename", json!({"name": "Alpha"})).unwrap();
    s.execute("home.insertSheet", json!({"name": "Beta"})).unwrap();
    s.execute("range.setValues", json!({"range": "A1", "values": [["Name", "Qty"], ["a", 1], ["b", 2]]})).unwrap();
    s.execute("selection.set", json!({"range": "A1:B3"})).unwrap();
    s.execute("insert.chart", json!({"type": "column", "at": "D3", "width": 300, "height": 200})).unwrap();
    s.execute("sheet.activate", json!({"sheet": 0})).unwrap();
    let (mut h, handed) = hosted_of(s);
    format_chart(&mut h, id, "Chart Title");
    let field = pane_fields(&h)[0];
    type_into(&mut h, field, "Only for Alpha");
    (h, handed, id)
}

/// Clicking another sheet's tab with a chart title being typed enters it into the chart it was typed for.
#[test]
fn a_chart_title_being_typed_is_entered_into_its_own_sheet_before_another_sheet_comes_forward() {
    let (mut h, handed, _) = two_charts();
    let beta_title = h.state().session.doc().unwrap().wb.sheet(1).unwrap().charts[0].title.clone();
    assert!(beta_title.is_some(), "setup: Beta's chart has a title of its own to lose");
    h.state_mut().run_or_alert("sheet.activate", json!({"sheet": 1}));
    h.run_steps(3);
    h.state_mut().run_or_alert("file.save", json!({}));
    h.run_steps(3);
    let saved = last_saved(&handed);
    let title = |i: usize| saved.sheet(i).unwrap().charts[0].title.clone();
    // Beta's chart keeps the title it was given when it was inserted.
    assert_eq!((title(0), title(1)), (Some("Only for Alpha".to_string()), beta_title), "the title is on the chart it was typed for");
    assert!(!h.state().has_unsaved_work());
}

/// A script that switches sheets under the draft doesn't make it land on the other sheet's chart:
/// it is not entered there, and what was typed is said.
#[test]
fn a_chart_title_being_typed_never_lands_on_another_sheets_chart() {
    let (mut h, handed, _) = two_charts();
    h.state_mut().session.run("sheet.activate", json!({"sheet": 1})).unwrap();
    h.run_steps(3);
    h.state_mut().run_or_alert("file.save", json!({}));
    h.run_steps(3);
    let saved = last_saved(&handed);
    assert!(saved.sheets.iter().all(|s| s.charts[0].title.as_deref() != Some("Only for Alpha")), "the title is on no chart");
    let said = h.state().message.as_ref().map(|(_, m)| m.clone());
    assert!(said.is_some_and(|m| m.contains("Only for Alpha")), "what was typed is said");
}

// ---------------------------------------------------------------- File › Info and Options

#[test]
fn a_document_property_being_typed_is_saved_by_save() {
    saves_what_was_typed(
        || {
            let (mut h, handed) = hosted_of(crate::support::blank_session());
            h.state_mut().run("ui.backstage", json!({"page": "info"})).unwrap();
            h.run_steps(3);
            let field = h.get_by_role_and_label(Role::TextInput, "Title").rect();
            type_into(&mut h, field, "Quarterly");
            (h, handed)
        },
        "Quarterly".to_string(),
        |wb| wb.props.title.clone(),
    );
}

#[test]
fn a_maximum_iterations_value_being_typed_is_saved_by_save() {
    saves_what_was_typed(
        || {
            let mut s = crate::support::blank_session();
            s.execute("formulas.calculationOptions", json!({"iterative": true})).unwrap();
            let (mut h, handed) = hosted_of(s);
            h.set_size(vec2(1200.0, 1400.0));
            h.state_mut().run("ui.backstage", json!({"page": "options"})).unwrap();
            h.run_steps(3);
            let field = h.get_by_role_and_label(Role::TextInput, "Maximum Iterations:").rect();
            type_into(&mut h, field, "777");
            (h, handed)
        },
        777,
        |wb| wb.calc.max_iterations,
    );
}

#[test]
fn a_maximum_change_value_being_typed_is_saved_by_save() {
    saves_what_was_typed(
        || {
            let mut s = crate::support::blank_session();
            s.execute("formulas.calculationOptions", json!({"iterative": true})).unwrap();
            let (mut h, handed) = hosted_of(s);
            h.set_size(vec2(1200.0, 1400.0));
            h.state_mut().run("ui.backstage", json!({"page": "options"})).unwrap();
            h.run_steps(3);
            let field = h.get_by_role_and_label(Role::TextInput, "Maximum Change:").rect();
            type_into(&mut h, field, "0.5");
            (h, handed)
        },
        0.5,
        |wb| wb.calc.max_change,
    );
}
