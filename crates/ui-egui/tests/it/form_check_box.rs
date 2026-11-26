//! A form check box through real pointer and keyboard input: a click checks or clears it and
//! writes TRUE or FALSE to its linked cell as one undo step, a mixed one is checked by a click,
//! and Space toggles the focused check box without starting a cell edit.

use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect, vec2};
use gridcraft_engine::Session;
use gridcraft_engine::core::{CellRef, Value};
use gridcraft_engine::model::{CheckState, FormControl, ObjKind, ObjRef};
use gridcraft_ui_egui::{SheetApp, chartview, grid::Geo};
use serde_json::json;

type Harness = egui_kittest::Harness<'static, SheetApp>;

fn fixture() -> (Harness, u32) {
    let mut s = Session::new();
    s.new_workbook();
    s.execute("cell.set", json!({"cell": "L1", "input": "=IF(K1,\"yes\",\"no\")"})).unwrap();
    let r = s.execute("insert.checkBoxControl", json!({"at": "E3", "width": 120, "height": 20, "text": "Paid", "link": "K1"})).unwrap();
    let id = r["control"].as_u64().unwrap() as u32;
    s.execute("selection.set", json!({"cell": "A20"})).unwrap();
    let mut h = egui_kittest::Harness::builder()
        .with_size(vec2(1400.0, 900.0))
        .with_step_dt(1.0 / 60.0)
        .with_os(egui::os::OperatingSystem::Mac)
        .build_ui_state(
            |ui, app: &mut SheetApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            SheetApp::new(s, Default::default()),
        );
    h.run_steps(4);
    (h, id)
}

fn control(h: &Harness, id: u32) -> FormControl {
    h.state().session.doc().unwrap().wb.active().unwrap().controls.iter().find(|c| c.id == id).cloned().unwrap()
}

fn rect(h: &Harness, id: u32) -> Rect {
    let app = h.state();
    let sh = app.session.doc().unwrap().wb.active().unwrap();
    chartview::object_rect(&Geo::new(sh, app.grid.rect.unwrap(), app.view().scroll), sh, ObjRef::new(ObjKind::Control, id)).unwrap()
}

fn cell(h: &Harness, a: &str) -> Value {
    h.state().session.doc().unwrap().wb.active().unwrap().value(CellRef::parse(a).unwrap())
}

fn undo_len(h: &Harness) -> usize {
    h.state().session.doc().unwrap().undo.len()
}

fn click(h: &mut Harness, p: Pos2) {
    h.run_steps(30);
    for pressed in [true, false] {
        h.input_mut().events.push(Event::PointerMoved(p));
        h.input_mut().events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        h.step();
    }
    h.run_steps(2);
}

/// Space as a real keyboard sends it: the key and the text it types.
fn space(h: &mut Harness) {
    h.input_mut().events.push(Event::Key { key: Key::Space, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.input_mut().events.push(Event::Text(" ".into()));
    h.input_mut().events.push(Event::Key { key: Key::Space, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE });
    h.run_steps(2);
}

#[test]
fn a_click_toggles_the_check_box_and_its_linked_cell_as_one_undo_step() {
    let (mut h, id) = fixture();
    let r = rect(&h, id);
    let before = undo_len(&h);
    click(&mut h, r.center());
    assert_eq!(h.state().selected_object, None, "working a check box doesn't select it");
    assert_eq!((control(&h, id).checked, cell(&h, "K1"), cell(&h, "L1")), (CheckState::Checked, Value::Bool(true), Value::text("yes")));
    assert_eq!(undo_len(&h), before + 1, "one undo step");
    click(&mut h, r.center());
    assert_eq!((control(&h, id).checked, cell(&h, "K1"), cell(&h, "L1")), (CheckState::Unchecked, Value::Bool(false), Value::text("no")));
    h.state_mut().run("edit.undo", json!({})).unwrap();
    h.run_steps(2);
    assert_eq!((control(&h, id).checked, cell(&h, "K1")), (CheckState::Checked, Value::Bool(true)));
    // A mixed check box is checked by a click.
    h.state_mut().run("control.setValue", json!({"id": id, "value": "mixed"})).unwrap();
    click(&mut h, r.center());
    assert_eq!((control(&h, id).checked, cell(&h, "K1")), (CheckState::Checked, Value::Bool(true)));
}

#[test]
fn a_check_box_paints_and_toggles_at_minimum_zoom() {
    let (mut h, id) = fixture();
    h.state_mut().run("view.zoom", json!({"percent": 10})).unwrap();
    h.run_steps(2);
    let p = rect(&h, id).center();
    click(&mut h, p);
    assert_eq!(cell(&h, "K1"), Value::Bool(true));
}

#[test]
fn a_batched_check_box_click_toggles_in_its_own_frame() {
    let (mut h, id) = fixture();
    let pos = rect(&h, id).center();
    h.input_mut().events.push(Event::PointerMoved(pos));
    h.step();
    for pressed in [true, false] {
        h.input_mut().events.push(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
    }
    h.step();
    assert_eq!(cell(&h, "K1"), Value::Bool(true));
}

#[test]
fn space_toggles_the_focused_check_box_without_editing_a_cell() {
    let (mut h, id) = fixture();
    let r = rect(&h, id);
    click(&mut h, r.center());
    assert_eq!((h.state().grid.focused_control, control(&h, id).checked), (Some(id), CheckState::Checked));
    let before = undo_len(&h);
    space(&mut h);
    assert_eq!((control(&h, id).checked, cell(&h, "K1")), (CheckState::Unchecked, Value::Bool(false)));
    assert_eq!(undo_len(&h), before + 1, "one undo step");
    assert!(h.state().editor.is_none(), "Space didn't start a cell edit");
    assert_eq!(cell(&h, "A20"), Value::Empty, "nor typed into the active cell");
    assert_eq!(h.state().session.doc().unwrap().selection.active.a1(), "A20", "the cell cursor didn't move");
}
