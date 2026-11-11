//! Renaming a sheet tab: the name field must let go of the keyboard when the user is done (#145).

use egui::{Event, Key, Modifiers, PointerButton};
use gridcraft_engine::Session;
use gridcraft_engine::core::CellRef;
use gridcraft_ui_egui::{SheetApp, grid::Geo};

fn harness() -> egui_kittest::Harness<'static, SheetApp> {
    let mut s = Session::new();
    s.new_workbook();
    let mut h = crate::common::builder().with_size(egui::vec2(1200.0, 800.0)).build_ui_state(
        |ui, app: &mut SheetApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        crate::common::app(s),
    );
    h.run_steps(4);
    h
}

/// Opens the first tab's name field and types `new_name` over its name, as a person does.
fn start_rename(h: &mut egui_kittest::Harness<'static, SheetApp>, new_name: &str) {
    gridcraft_ui_egui::tabs::begin_tab_rename(h.state_mut(), 0);
    h.run_steps(3);
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.run_steps(2);
    h.input_mut().events.push(Event::Text(new_name.to_string()));
    h.run_steps(3);
}

fn sheet_name(h: &egui_kittest::Harness<'static, SheetApp>) -> String {
    h.state().session.active().unwrap().wb.sheets[0].name.clone()
}

#[test]
fn enter_saves_the_sheet_name_and_closes_the_field() {
    let mut h = harness();
    start_rename(&mut h, "Budget");
    h.input_mut().events.push(Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.input_mut().events.push(Event::Key { key: Key::Enter, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
    assert!(h.state().grid.renaming_tab.is_none(), "Enter must close the rename field");
    assert_eq!(sheet_name(&h), "Budget");
}

#[test]
fn clicking_a_cell_saves_the_sheet_name_and_closes_the_field() {
    let mut h = harness();
    start_rename(&mut h, "Ledger");
    let app = h.state();
    let sheet = app.session.active().unwrap().wb.active().unwrap();
    let geo = Geo::new(sheet, app.grid.rect.unwrap(), app.view().scroll);
    let pos = geo.cell_rect(sheet, CellRef::parse("B2").unwrap()).center();
    h.input_mut().events.extend([
        Event::PointerMoved(pos),
        Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE },
        Event::PointerButton { pos, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE },
    ]);
    h.run_steps(3);
    assert!(h.state().grid.renaming_tab.is_none(), "clicking elsewhere must close the rename field");
    assert_eq!(sheet_name(&h), "Ledger");
}
