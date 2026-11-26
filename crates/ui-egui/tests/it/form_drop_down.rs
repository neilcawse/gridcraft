//! A form drop-down through real pointer and keyboard input: a click opens its list, choosing an
//! item writes the item's position to the linked cell as one undo step, a click elsewhere or
//! Escape closes the list, and the arrow keys choose the item above or below.

use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect, vec2};
use egui_kittest::kittest::Queryable;
use gridcraft_engine::Session;
use gridcraft_engine::core::{CellRef, Value};
use gridcraft_engine::model::{FormControl, ObjKind, ObjRef};
use gridcraft_ui_egui::{SheetApp, chartview, grid::Geo};
use serde_json::json;

type Harness = egui_kittest::Harness<'static, SheetApp>;

fn fixture() -> (Harness, u32) {
    let mut s = Session::new();
    s.new_workbook();
    s.execute("range.setValues", json!({"range": "J1:J4", "values": [["Red"], ["Green"], ["Blue"], ["Gold"]]})).unwrap();
    s.execute("cell.set", json!({"cell": "L1", "input": "=K1*10"})).unwrap();
    let r = s.execute("insert.dropDown", json!({"at": "E3", "width": 160, "height": 20, "listRange": "J1:J4", "link": "K1"})).unwrap();
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

fn key(h: &mut Harness, key: Key) {
    h.input_mut().events.push(Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.input_mut().events.push(Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE });
    h.run_steps(2);
}

fn open(h: &Harness) -> Option<u32> {
    h.state().grid.drop_open
}

#[test]
fn choosing_from_the_open_list_writes_the_linked_cell_as_one_undo_step() {
    let (mut h, id) = fixture();
    let r = rect(&h, id);
    click(&mut h, r.center());
    assert_eq!(open(&h), Some(id), "a click opens the list");
    assert_eq!(h.state().selected_object, None, "working a control doesn't select it");
    let before = undo_len(&h);
    h.get_by_label("Blue").click();
    h.run_steps(3);
    assert_eq!(open(&h), None, "choosing closes the list");
    assert_eq!((control(&h, id).selected, cell(&h, "K1"), cell(&h, "L1")), (3, Value::Number(3.0), Value::Number(30.0)));
    assert_eq!(undo_len(&h), before + 1, "one undo step");
    assert_eq!(h.state().grid.focused_control, Some(id), "picking an item returns focus to the drop-down");
    key(&mut h, Key::ArrowUp);
    assert_eq!(control(&h, id).selected, 2, "keyboard focus works immediately after a pointer pick");
    h.state_mut().run("edit.undo", json!({})).unwrap();
    h.run_steps(2);
    h.state_mut().run("edit.undo", json!({})).unwrap();
    h.run_steps(2);
    assert_eq!((control(&h, id).selected, cell(&h, "K1")), (0, Value::Empty));

    // A click on the sheet away from the list closes it and chooses nothing.
    click(&mut h, r.center());
    assert_eq!(open(&h), Some(id));
    click(&mut h, Pos2::new(r.right() + 400.0, r.bottom() + 300.0));
    assert_eq!(open(&h), None);
    assert_eq!(control(&h, id).selected, 0);
}

#[test]
fn the_keyboard_chooses_items_and_escape_closes_the_list_first() {
    let (mut h, id) = fixture();
    let r = rect(&h, id);
    click(&mut h, r.center());
    assert_eq!(h.state().grid.focused_control, Some(id));
    // Escape closes the list but keeps the keyboard; the arrows then choose.
    key(&mut h, Key::Escape);
    assert_eq!((open(&h), h.state().grid.focused_control), (None, Some(id)));
    key(&mut h, Key::ArrowDown);
    key(&mut h, Key::ArrowDown);
    assert_eq!((control(&h, id).selected, cell(&h, "K1")), (2, Value::Number(2.0)));
    key(&mut h, Key::End);
    assert_eq!(control(&h, id).selected, 4);
    key(&mut h, Key::ArrowDown);
    assert_eq!(control(&h, id).selected, 4, "it stops at the last item");
    key(&mut h, Key::Home);
    key(&mut h, Key::ArrowUp);
    assert_eq!((control(&h, id).selected, cell(&h, "K1")), (1, Value::Number(1.0)), "and at the first");
    // Enter opens the list; Escape closes it, then lets go of the control.
    key(&mut h, Key::Enter);
    assert_eq!(open(&h), Some(id));
    key(&mut h, Key::Escape);
    key(&mut h, Key::Escape);
    assert_eq!((open(&h), h.state().grid.focused_control), (None, None));
    assert_eq!(h.state().session.doc().unwrap().selection.active.a1(), "A20", "the cell cursor didn't move meanwhile");
}

#[test]
fn a_click_batched_in_one_frame_opens_the_list_immediately() {
    let (mut h, id) = fixture();
    let p = rect(&h, id).center();
    h.run_steps(30);
    h.input_mut().events.push(Event::PointerMoved(p));
    for pressed in [true, false] {
        h.input_mut().events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
    }
    h.step();
    assert_eq!(open(&h), Some(id));
    h.input_mut().events.push(Event::PointerMoved(Pos2::new(p.x + 300.0, p.y)));
    h.step();
    assert_eq!(open(&h), Some(id), "the completed click does not start a second gesture");
}

#[test]
fn a_batched_release_over_a_drop_down_does_not_open_it_after_an_outside_press() {
    let (mut h, id) = fixture();
    let r = rect(&h, id);
    let outside = Pos2::new(r.right() + 1.0, r.center().y);
    let inside = Pos2::new(r.right() - 1.0, r.center().y);
    h.run_steps(30);
    h.input_mut().events.push(Event::PointerMoved(inside));
    for (pos, pressed) in [(outside, true), (inside, false)] {
        h.input_mut().events.push(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
    }
    h.step();
    assert_eq!(open(&h), None);
    assert_eq!(h.state().grid.focused_control, None);
}
