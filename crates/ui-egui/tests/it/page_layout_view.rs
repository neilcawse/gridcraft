//! Page Layout view through real pointer and keyboard input: cells are hit through the page
//! transform, and headers are edited in place, committed on a click elsewhere, and undoable. A
//! header still being typed is unsaved work: Save, the File tab and Close all enter it first.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use egui::{Event, Key, Modifiers, PointerButton, Pos2, pos2, vec2};
use egui_kittest::kittest::Queryable;
use gridcraft_engine::Session;
use gridcraft_engine::core::{CellRef, Value};
use gridcraft_engine::model::{SheetViewMode, Workbook};
use gridcraft_ui_egui::grid::Geo;
use gridcraft_ui_egui::{HostSave, Services, SheetApp};
use serde_json::json;

type Harness = egui_kittest::Harness<'static, SheetApp>;

fn fixture() -> Harness {
    fixture_with(false, None)
}

/// `rtl` mirrors the sheet; `setup` is extra `pageLayout.pageSetup` params.
fn fixture_with(rtl: bool, setup: Option<serde_json::Value>) -> Harness {
    harness_of(crate::common::app(page_layout_session(rtl, setup)))
}

/// Sixty numbers in column A and the header "Left" on the left, shown in Page Layout view.
fn page_layout_session(rtl: bool, setup: Option<serde_json::Value>) -> Session {
    let mut session = Session::new();
    session.new_workbook();
    if rtl {
        session.execute("view.rightToLeft", json!({"on": true})).unwrap();
    }
    if let Some(p) = setup {
        session.execute("cell.set", json!({"cell": "Z1", "input": "wide"})).unwrap();
        session.execute("pageLayout.pageSetup", p).unwrap();
    }
    for r in 1..=60 {
        session.execute("cell.set", json!({"cell": format!("A{r}"), "input": r.to_string()})).unwrap();
    }
    session.execute("pageLayout.headerFooter", json!({"header": "&LLeft"})).unwrap();
    session.execute("view.pageLayout", json!({})).unwrap();
    session
}

fn harness_of(app: SheetApp) -> Harness {
    crate::common::harness_with(
        crate::common::builder().with_size(vec2(1200.0, 900.0)).with_step_dt(1.0 / 60.0).with_os(egui::os::OperatingSystem::Mac),
        app,
        4,
    )
}

fn geo(h: &Harness) -> Geo {
    h.state().grid.geo.clone().unwrap()
}

fn click(h: &mut Harness, p: Pos2) {
    // Keep clicks apart from egui's double-click window.
    h.run_steps(40);
    h.input_mut().events.push(Event::PointerMoved(p));
    h.input_mut().events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
}

fn header(h: &Harness) -> String {
    h.state().session.doc().unwrap().wb.active().unwrap().print.header.clone()
}

/// Screen point of a layout point.
fn screen(g: &Geo, x: f64, y: f64) -> Pos2 {
    pos2(g.cells.left() + (x - g.scroll.x as f64) as f32 * g.z, g.cells.top() + (y - g.scroll.y as f64) as f32 * g.z)
}

#[test]
fn cells_are_hit_through_the_page_transform() {
    let mut h = fixture();
    let g = geo(&h);
    let pg = g.paper.clone().expect("Page Layout view lays out pages");
    let app = h.state();
    let sh = app.session.doc().unwrap().wb.active().unwrap();
    assert_eq!(sh.view, SheetViewMode::PageLayout);
    // Cells start inside the first page's margins, not at the grid's corner.
    let a1 = g.cell_rect(sh, CellRef::new(0, 0));
    let content = screen(&g, pg.page_x(0) + pg.margins[0], pg.page_y(0) + pg.margins[2]);
    assert!((a1.left() - content.x).abs() < 0.5 && (a1.top() - content.y).abs() < 0.5, "{a1:?} vs {content:?}");
    let c5 = g.cell_rect(sh, CellRef::parse("C5").unwrap()).center();
    assert_eq!(g.cell_at(sh, c5), CellRef::parse("C5").unwrap());
    click(&mut h, c5);
    assert_eq!(h.state().session.doc().unwrap().selection.active, CellRef::parse("C5").unwrap());
    // A click in the gap left of the page selects nothing new.
    let gap = screen(&g, pg.gap / 2.0, pg.page_y(0) + pg.margins[2] + 30.0);
    click(&mut h, gap);
    assert_eq!(h.state().session.doc().unwrap().selection.active, CellRef::parse("C5").unwrap());
}

#[test]
fn header_is_edited_in_place_committed_by_a_click_elsewhere_and_undoable() {
    let mut h = fixture();
    let g = geo(&h);
    let pg = g.paper.clone().unwrap();
    // The centre third of the first page's top margin.
    let (px, py, w, _) = pg.page_rect(0, 0);
    let centre = screen(&g, px + w / 2.0, py + pg.margins[2] / 2.0);
    click(&mut h, centre);
    {
        let app = h.state();
        let e = app.grid.hf_edit.as_ref().expect("clicking the top margin edits the header");
        assert_eq!((e.footer, e.section), (false, 1));
        assert_eq!(app.ui.ribbon_tab, "Header & Footer");
    }
    h.input_mut().events.push(Event::Text("Report ".into()));
    h.run_steps(2);
    gridcraft_ui_egui::paper_view::insert_field(h.state_mut(), "&[Page]");
    h.run_steps(2);
    assert_eq!(h.state().grid.hf_edit.as_ref().unwrap().texts[1], "Report &[Page]");
    assert_eq!(header(&h), "&LLeft", "nothing is written while editing");
    // Clicking a cell commits (one undo step) and selects the cell.
    let sh_geo = geo(&h);
    let b3 = {
        let app = h.state();
        let sh = app.session.doc().unwrap().wb.active().unwrap();
        sh_geo.cell_rect(sh, CellRef::parse("B3").unwrap()).center()
    };
    click(&mut h, b3);
    assert!(h.state().grid.hf_edit.is_none());
    assert_eq!(header(&h), "&LLeft&CReport &P");
    assert_eq!(h.state().session.doc().unwrap().selection.active, CellRef::parse("B3").unwrap());
    assert_ne!(h.state().ui.ribbon_tab, "Header & Footer");
    h.state_mut().run("edit.undo", json!({})).unwrap();
    assert_eq!(header(&h), "&LLeft");
    // Escape cancels an edit without writing.
    click(&mut h, centre);
    h.input_mut().events.push(Event::Text("x".into()));
    h.run_steps(2);
    h.input_mut().events.push(Event::Key { key: Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.run_steps(2);
    assert!(h.state().grid.hf_edit.is_none());
    assert_eq!(header(&h), "&LLeft");
}

#[test]
fn the_first_page_edits_its_own_header_when_different_first_page_is_on() {
    let mut h = fixture();
    h.state_mut().run("pageLayout.headerFooter", json!({"differentFirst": true, "firstHeader": "&CCover"})).unwrap();
    h.run_steps(3);
    // Insert › Header & Footer edits the first page's centre header, here the first-page one.
    gridcraft_ui_egui::paper_view::start_editing(h.state_mut());
    h.run_steps(3);
    {
        let e = h.state().grid.hf_edit.as_ref().expect("Insert › Header & Footer starts editing");
        assert_eq!((e.footer, e.section, e.kind), (false, 1, gridcraft_engine::model::HfPage::First));
        assert_eq!(e.texts[1], "Cover");
    }
    h.input_mut().events.push(Event::Text(" page".into()));
    h.run_steps(2);
    let g = geo(&h);
    let b3 = {
        let app = h.state();
        g.cell_rect(app.session.doc().unwrap().wb.active().unwrap(), CellRef::parse("B3").unwrap()).center()
    };
    click(&mut h, b3);
    let pr = h.state().session.doc().unwrap().wb.active().unwrap().print.clone();
    assert_eq!(pr.first_header, "&CCover page");
    assert_eq!(pr.header, "&LLeft", "the other pages keep their header");
}

#[test]
fn scale_with_document_and_align_with_page_margins_are_check_boxes_on_the_header_and_footer_tab() {
    let mut h = fixture();
    gridcraft_ui_egui::paper_view::start_editing(h.state_mut());
    h.run_steps(4);
    let options = |h: &Harness| {
        let pr = &h.state().session.doc().unwrap().wb.active().unwrap().print;
        (pr.hf_scale_with_doc, pr.hf_align_with_margins)
    };
    assert_eq!(options(&h), (true, true));
    h.get_by_label("Scale with Document").click();
    h.run_steps(3);
    assert_eq!(options(&h), (false, true));
    h.get_by_label("Align with Page Margins").click();
    h.run_steps(3);
    assert_eq!(options(&h), (false, false));
    assert!(h.state().grid.hf_edit.is_some(), "the header stays in edit");
    h.state_mut().run("edit.undo", json!({})).unwrap();
    assert_eq!(options(&h), (false, true));
}

#[test]
fn insert_header_and_footer_switches_to_page_layout_view() {
    let mut h = fixture();
    h.state_mut().run("view.normal", json!({})).unwrap();
    h.run_steps(3);
    gridcraft_ui_egui::paper_view::start_editing(h.state_mut());
    h.run_steps(4);
    assert_eq!(h.state().session.doc().unwrap().wb.active().unwrap().view, SheetViewMode::PageLayout);
    let e = h.state().grid.hf_edit.as_ref().expect("the header is being edited");
    assert_eq!((e.footer, e.section, e.texts[0].as_str()), (false, 1, "Left"));
}

fn margins(h: &Harness) -> [f32; 6] {
    h.state().session.doc().unwrap().wb.active().unwrap().print.margins
}

fn mouse(h: &mut Harness, ev: Event) {
    h.input_mut().events.push(ev);
    h.step();
}

#[test]
fn dragging_a_ruler_edge_previews_commits_on_release_as_one_undo_step_and_esc_cancels() {
    let mut h = fixture();
    let g = geo(&h);
    let pg = g.paper.clone().unwrap();
    let start_margins = margins(&h);
    let ruler_y = h.state().grid.rect.unwrap().top() + 9.0;
    let edge = pos2(screen(&g, pg.page_x(0) + pg.margins[0], 0.0).x, ruler_y);
    // One inch further in = 96 layout pixels at 100% print scale; use the screen scale to be exact.
    let per_inch = (72.0 / (pg.paper_pt.0 as f64 / pg.page_w)) as f32 * g.z;
    let target = pos2(edge.x + per_inch, ruler_y);
    h.run_steps(40);
    mouse(&mut h, Event::PointerMoved(edge));
    mouse(&mut h, Event::PointerButton { pos: edge, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    mouse(&mut h, Event::PointerMoved(pos2(edge.x + 5.0, ruler_y)));
    mouse(&mut h, Event::PointerMoved(target));
    h.run_steps(2);
    // Live preview: the page content moved, the document did not change yet.
    let live = geo(&h).paper.unwrap().margins[0];
    assert!(live > pg.margins[0] + 1.0, "{live} vs {}", pg.margins[0]);
    assert_eq!(margins(&h), start_margins);
    let want = (start_margins[0] + 1.0 * 1.0).min(10.0);
    mouse(&mut h, Event::PointerButton { pos: target, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
    let after = margins(&h);
    assert!((after[0] - want).abs() < 0.03, "{after:?} want left {want}");
    assert_eq!(after[1..], start_margins[1..]);
    // One undo step restores it.
    h.state_mut().run("edit.undo", json!({})).unwrap();
    h.run_steps(3);
    assert_eq!(margins(&h), start_margins);
    // Esc during a drag cancels: nothing is committed.
    let g = geo(&h);
    let pg = g.paper.clone().unwrap();
    let edge = pos2(screen(&g, pg.page_x(0) + pg.margins[0], 0.0).x, ruler_y);
    h.run_steps(40);
    mouse(&mut h, Event::PointerMoved(edge));
    mouse(&mut h, Event::PointerButton { pos: edge, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    mouse(&mut h, Event::PointerMoved(pos2(edge.x + per_inch, ruler_y)));
    mouse(&mut h, Event::Key { key: Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    mouse(&mut h, Event::PointerButton { pos: edge, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
    assert_eq!(margins(&h), start_margins);
}

fn press(h: &mut Harness, p: Pos2) {
    h.run_steps(40);
    mouse(h, Event::PointerMoved(p));
    mouse(h, Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
}

fn release(h: &mut Harness, p: Pos2) {
    mouse(h, Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
}

/// Screen pixels per inch of the page.
fn per_inch(g: &Geo) -> f32 {
    let pg = g.paper.clone().unwrap();
    (72.0 / (pg.paper_pt.0 as f64 / pg.page_w)) as f32 * g.z
}

#[test]
fn header_and_footer_margins_are_dragged_on_the_vertical_ruler() {
    let mut h = fixture();
    let g = geo(&h);
    let pg = g.paper.clone().unwrap();
    let start = margins(&h);
    let ruler_x = h.state().grid.rect.unwrap().left() + 9.0;
    // Header: distance from the top of the page, 0.2 in further down.
    let at = pos2(ruler_x, screen(&g, 0.0, pg.page_y(0) + pg.margins[4]).y);
    let to = pos2(ruler_x, at.y + 0.2 * per_inch(&g));
    press(&mut h, at);
    mouse(&mut h, Event::PointerMoved(pos2(ruler_x, at.y + 4.0)));
    mouse(&mut h, Event::PointerMoved(to));
    release(&mut h, to);
    let after = margins(&h);
    assert!((after[4] - (start[4] + 0.2)).abs() < 0.03, "{after:?} vs {start:?}");
    assert_eq!((after[..4].to_vec(), after[5]), (start[..4].to_vec(), start[5]));
    h.state_mut().run("edit.undo", json!({})).unwrap();
    h.run_steps(3);
    assert_eq!(margins(&h), start);
    // Footer: distance from the bottom, dragged up by 0.2 in grows it (zoom out so the page
    // bottom is on screen).
    h.state_mut().run("view.zoom", json!({"percent": 50})).unwrap();
    h.run_steps(4);
    let g = geo(&h);
    let pg = g.paper.clone().unwrap();
    let (_, py, _, ph) = pg.page_rect(0, 0);
    let at = pos2(ruler_x, screen(&g, 0.0, py + ph - pg.margins[5]).y);
    let to = pos2(ruler_x, at.y - 0.2 * per_inch(&g));
    press(&mut h, at);
    mouse(&mut h, Event::PointerMoved(pos2(ruler_x, at.y - 4.0)));
    mouse(&mut h, Event::PointerMoved(to));
    release(&mut h, to);
    assert!((margins(&h)[5] - (start[5] + 0.2)).abs() < 0.03, "{:?}", margins(&h));
}

#[test]
fn right_to_left_sheets_mirror_the_horizontal_ruler_drag() {
    let mut h = fixture_with(true, None);
    let g = geo(&h);
    assert!(g.rtl);
    let pg = g.paper.clone().unwrap();
    let start = margins(&h);
    let y = h.state().grid.rect.unwrap().top() + 9.0;
    // The left margin edge is drawn on the right of the mirrored page; dragging it towards the
    // page centre (screen-left) grows the left margin.
    let at = pos2(g.mirror_x(screen(&g, pg.page_x(0) + pg.margins[0], 0.0).x), y);
    let to = pos2(at.x - per_inch(&g), y);
    press(&mut h, at);
    mouse(&mut h, Event::PointerMoved(pos2(at.x - 4.0, y)));
    mouse(&mut h, Event::PointerMoved(to));
    release(&mut h, to);
    let after = margins(&h);
    assert!((after[0] - (start[0] + 1.0)).abs() < 0.03, "{after:?} vs {start:?}");
    assert_eq!(after[1], start[1]);
}

#[test]
fn preview_repaginates_so_fit_to_width_matches_the_commit() {
    let mut h = fixture_with(false, Some(json!({"fitWidth": 1, "fitHeight": 0})));
    let g = geo(&h);
    let pg = g.paper.clone().unwrap();
    let start = margins(&h);
    let y = h.state().grid.rect.unwrap().top() + 9.0;
    let at = pos2(screen(&g, pg.page_x(0) + pg.margins[0], 0.0).x, y);
    let to = pos2(at.x + 2.0 * per_inch(&g), y);
    press(&mut h, at);
    mouse(&mut h, Event::PointerMoved(pos2(at.x + 4.0, y)));
    mouse(&mut h, Event::PointerMoved(to));
    h.run_steps(2);
    let live_scale = geo(&h).paper.unwrap().scale;
    assert_eq!(margins(&h), start);
    assert!(live_scale < pg.scale, "live scale {live_scale} should shrink below {}", pg.scale);
    release(&mut h, to);
    h.run_steps(2);
    let committed = geo(&h).paper.unwrap().scale;
    assert!((committed - live_scale).abs() < 0.01, "committed {committed} vs preview {live_scale}");
}

#[test]
fn right_to_left_sheets_carry_the_vertical_ruler_on_the_right() {
    let mut h = fixture_with(true, None);
    let g = geo(&h);
    assert!(g.rtl);
    let pg = g.paper.clone().unwrap();
    let start = margins(&h);
    let outer = h.state().grid.rect.unwrap();
    // The grid leaves the ruler's width free on the right, not on the left.
    assert!((g.rect.right() - (outer.right() - gridcraft_ui_egui::paper_view::RULER)).abs() < 0.5, "{:?} {outer:?}", g.rect);
    assert!((g.rect.left() - outer.left()).abs() < 0.5);
    let x = outer.right() - 9.0;
    let at = pos2(x, screen(&g, 0.0, pg.page_y(0) + pg.margins[2]).y);
    let to = pos2(x, at.y + 0.25 * per_inch(&g));
    press(&mut h, at);
    mouse(&mut h, Event::PointerMoved(pos2(x, at.y + 4.0)));
    mouse(&mut h, Event::PointerMoved(to));
    release(&mut h, to);
    let after = margins(&h);
    assert!((after[2] - (start[2] + 0.25)).abs() < 0.03, "{after:?} vs {start:?}");
    // The left ruler strip is not a handle any more: nothing happens there.
    let g = geo(&h);
    let pg = g.paper.clone().unwrap();
    let at = pos2(outer.left() + 9.0, screen(&g, 0.0, pg.page_y(0) + pg.margins[3]).y);
    press(&mut h, at);
    mouse(&mut h, Event::PointerMoved(pos2(at.x, at.y - 30.0)));
    release(&mut h, pos2(at.x, at.y - 30.0));
    assert_eq!(margins(&h)[3], start[3]);
}

#[test]
fn header_distance_cannot_be_dragged_past_the_top_margin() {
    let mut h = fixture();
    let g = geo(&h);
    let pg = g.paper.clone().unwrap();
    let start = margins(&h);
    let ruler_x = h.state().grid.rect.unwrap().left() + 9.0;
    let at = pos2(ruler_x, screen(&g, 0.0, pg.page_y(0) + pg.margins[4]).y);
    let to = pos2(ruler_x, at.y + 3.0 * per_inch(&g));
    press(&mut h, at);
    mouse(&mut h, Event::PointerMoved(pos2(ruler_x, at.y + 4.0)));
    mouse(&mut h, Event::PointerMoved(to));
    release(&mut h, to);
    let after = margins(&h);
    assert!((after[4] - start[2]).abs() < 0.011, "header {} should stop at the top margin {}", after[4], start[2]);
    assert_eq!(after[2], start[2]);
}

// ---------------------------------------------------------------- a header being typed is unsaved work

/// The workbooks a page that keeps the files was handed, in order ([`Services::host_save`]).
type Handed = Rc<RefCell<Vec<Arc<Workbook>>>>;

/// The fixture in a page that keeps the files, as the extension is: a save hands the workbook over
/// and counts as done (no `?ack=1`). The workbook is saved, so only typing is unsaved work.
fn hosted() -> (Harness, Handed) {
    hosted_of(page_layout_session(false, None))
}

/// [`hosted`] for another session.
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
    (harness_of(app), handed)
}

/// Clicks the first page's top margin and types `text` into its centre header: a draft, not yet
/// in the workbook.
fn type_header(h: &mut Harness, text: &str) {
    type_header_over(h, text, "&LLeft");
}

/// [`type_header`] in a sheet whose header is `over` now.
fn type_header_over(h: &mut Harness, text: &str, over: &str) {
    let g = geo(h);
    let pg = g.paper.clone().unwrap();
    let (px, py, w, _) = pg.page_rect(0, 0);
    click(h, screen(&g, px + w / 2.0, py + pg.margins[2] / 2.0));
    h.input_mut().events.push(Event::Text(text.into()));
    h.run_steps(2);
    let e = h.state().grid.hf_edit.as_ref().expect("clicking the top margin edits the header");
    assert_eq!(e.texts[1], text, "setup: the header is typed");
    assert_eq!(header(h), over, "setup: and not in the workbook");
    assert!(h.state().has_unsaved_work(), "setup: the typed header would be lost");
    assert!(!h.state().session.active().unwrap().is_dirty(), "setup: it is not in the workbook yet");
}

/// Types `text` into `cell` of the sheet showing, in Normal view: a draft, not yet in the workbook.
fn type_cell(h: &mut Harness, cell: &str, text: &str) {
    h.state_mut().run("view.normal", json!({})).unwrap();
    h.state_mut().run("selection.set", json!({"cell": cell})).unwrap();
    h.run_steps(4);
    h.input_mut().events.push(Event::Text(text.into()));
    h.run_steps(3);
    assert_eq!(h.state().editor.as_ref().map(|e| e.text.as_str()), Some(text), "setup: {cell} is being typed");
}

/// Save, as the tab's button does; the workbook is handed to the page.
fn save(h: &mut Harness) {
    h.state_mut().run_or_alert("file.save", json!({}));
    h.run_steps(3);
}

fn button(h: &mut Harness, label: &str) {
    h.get_by_role_and_label(egui::accesskit::Role::Button, label).click();
    h.run_steps(3);
}

/// Save (the extension's button and Ctrl/Cmd+S, which ask the page for `file.save`) writes the
/// header that is being typed, and nothing else is left to lose.
#[test]
fn save_enters_the_header_being_typed_and_leaves_the_tab_clean() {
    let (mut h, handed) = hosted();
    type_header(&mut h, "Q3 report");

    h.state_mut().run_or_alert("file.save", json!({}));
    h.run_steps(3);
    let files = handed.borrow();
    assert_eq!(files.len(), 1, "the page was handed the file");
    assert_eq!(files[0].sheet(0).unwrap().print.header, "&LLeft&CQ3 report", "the file has what was typed");
    assert!(h.state().grid.hf_edit.is_none(), "the header is entered, not left open");
    assert!(!h.state().session.active().unwrap().is_dirty());
    assert!(!h.state().has_unsaved_work(), "the tab reports clean: nothing else is pending");
}

/// Save in the File view: the File tab enters the header, which the view hides along with the
/// sheet, and Save then hands the file over with it.
#[test]
fn the_file_tab_enters_the_header_being_typed_and_its_save_writes_it() {
    let (mut h, handed) = hosted();
    type_header(&mut h, "Q3 report");

    h.get_by_label("File").click();
    h.run_steps(3);
    assert!(h.state().backstage.is_some(), "the File view is open");
    assert!(h.state().grid.hf_edit.is_none(), "the sheet is hidden: the header is entered, not left open behind it");
    assert_eq!(header(&h), "&LLeft&CQ3 report");
    assert_ne!(h.state().ui.ribbon_tab, "Header & Footer", "the Header & Footer tab goes with the edit");

    button(&mut h, "Save");
    let files = handed.borrow();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].sheet(0).unwrap().print.header, "&LLeft&CQ3 report");
    assert!(!h.state().has_unsaved_work(), "nothing is left to lose");
}

/// Save, Cmd/Ctrl+S with the header section holding the keyboard: the editor answers the key (the
/// grid's shortcuts stand aside), as the text box does, and the file has what was typed.
#[test]
fn the_save_shortcut_in_the_header_being_typed_writes_it() {
    for (modifiers, command) in [(Modifiers::COMMAND, "Save"), (Modifiers::COMMAND | Modifiers::SHIFT, "Save As")] {
        let (mut h, handed) = hosted();
        type_header(&mut h, "Q3 report");
        let m = modifiers;
        h.input_mut().events.push(Event::Key { key: Key::S, physical_key: Some(Key::S), pressed: true, repeat: false, modifiers: m });
        h.input_mut().events.push(Event::Key { key: Key::S, physical_key: Some(Key::S), pressed: false, repeat: false, modifiers: m });
        h.run_steps(3);
        let files = handed.borrow();
        assert_eq!(files.len(), 1, "{command}: the page was handed the file");
        assert_eq!(files[0].sheet(0).unwrap().print.header, "&LLeft&CQ3 report", "{command}: the file has what was typed");
        assert!(h.state().grid.hf_edit.is_none(), "{command}: the header is entered");
        assert!(!h.state().has_unsaved_work(), "{command}: nothing is left to lose");
    }
}

/// File › Close with a header being typed: the header is entered, which leaves the workbook with
/// an unsaved change, so Close asks. Cancel keeps both; Don't Save closes the workbook.
#[test]
fn closing_with_a_header_being_typed_asks_to_save_it() {
    for route in ["File view", "command"] {
        let (mut h, _) = hosted();
        type_header(&mut h, "Q3 report");
        let typed_in = h.state().session.active().unwrap().uid;
        if route == "File view" {
            h.get_by_label("File").click();
            h.run_steps(3);
            button(&mut h, "Close");
        } else {
            h.state_mut().run_or_alert("file.close", json!({}));
            h.run_steps(3);
        }
        let open = |h: &Harness| h.state().session.documents().iter().any(|d| d.uid == typed_in);
        assert!(open(&h), "{route}: the workbook is still open");
        assert_eq!(h.state().dialog.as_ref().map(|d| d.name()), Some("saveChanges"), "{route}: Close asks");
        assert_eq!(header(&h), "&LLeft&CQ3 report", "{route}: the typed header is in the workbook the question is about");

        button(&mut h, "Cancel");
        assert!(h.state().dialog.is_none());
        assert!(open(&h), "{route}: Cancel keeps the workbook");
        assert_eq!(header(&h), "&LLeft&CQ3 report", "{route}: and the header");
        assert!(h.state().has_unsaved_work(), "{route}: still unsaved");

        h.state_mut().run_or_alert("file.close", json!({}));
        h.run_steps(3);
        button(&mut h, "Don't Save");
        assert!(!open(&h), "{route}: Don't Save closes the workbook");
        assert!(!h.state().has_unsaved_work(), "{route}: and nothing of it is left");
    }
}

/// The header typed in a workbook that closes goes with it: the workbooks made next, in the same
/// window, are untouched, in Page Layout view or not. No header edit is left behind to be entered
/// into them. (Closing the only workbook shows a blank one at once, and File › New another.)
#[test]
fn a_header_being_typed_in_a_closed_workbook_never_reaches_the_next_one() {
    let (mut h, _) = hosted();
    type_header(&mut h, "Q3 report");
    let typed_in = h.state().session.active().unwrap().uid;
    h.state_mut().run_or_alert("file.close", json!({"force": true, "document": true}));
    h.run_steps(3);
    assert!(h.state().grid.hf_edit.is_none(), "the edit closed with its window");
    h.state_mut().run_or_alert("file.new", json!({}));
    h.run_steps(4);
    assert!(h.state().grid.hf_edit.is_none());

    let docs: Vec<_> = h.state().session.documents().iter().map(|d| (d.uid, d.wb.sheet(0).unwrap().print.header.clone(), d.is_dirty())).collect();
    assert!(docs.len() >= 2 && docs.iter().all(|(uid, ..)| *uid != typed_in), "the closed workbook is gone and others took its place: {docs:?}");
    assert!(docs.iter().all(|(_, header, dirty)| header.is_empty() && !dirty), "the new workbooks have no header and no change: {docs:?}");
    assert!(!h.state().has_unsaved_work(), "nothing of the closed workbook is left to lose");
    for index in 0..docs.len() {
        h.state_mut().run("window.activate", json!({"index": index})).unwrap();
        h.state_mut().run("view.pageLayout", json!({})).unwrap();
        h.run_steps(4);
        assert_eq!(header(&h), "", "workbook {index}: also in Page Layout view");
    }
}

/// A header edit belongs to the workbook it was typed in: if the engine's active workbook is
/// another one when it is entered (a script switched windows between frames), it is not written
/// there, and what was typed is said, not lost unseen.
#[test]
fn a_header_edit_is_never_entered_into_another_workbook() {
    let (mut h, _) = hosted();
    type_header(&mut h, "Q3 report");
    let typed_in = h.state().session.active().unwrap().uid;
    h.state_mut().session.run("file.new", json!({})).unwrap();
    let other = h.state().session.active().unwrap().uid;
    assert_ne!(other, typed_in, "setup: another workbook is the active one");

    gridcraft_ui_egui::paper_view::commit(h.state_mut());
    let doc = |uid: u64| h.state().session.documents().iter().find(|d| d.uid == uid).unwrap().wb.sheet(0).unwrap().print.header.clone();
    assert_eq!(doc(other), "", "the other workbook has no header");
    assert!(h.state().grid.hf_edit.is_none(), "the edit is over");
    assert!(!h.state().session.documents().iter().find(|d| d.uid == other).unwrap().is_dirty(), "the other workbook is untouched");
    let said = h.state().message.as_ref().map(|(_, m)| m.clone()).expect("what was typed is said");
    assert!(said.contains("Q3 report") && said.contains("header"), "{said}");
}

/// Quit enters a header being typed in a window that is not showing, as it does a cell: the
/// workbook then has an unsaved change and Quit asks about it.
#[test]
fn quit_asks_about_a_header_being_typed_in_another_window() {
    let (mut h, _) = hosted();
    let first = h.state().session.active_window().unwrap();
    h.state_mut().run("view.newWindow", json!({})).unwrap();
    h.run_steps(4);
    assert_ne!(h.state().session.active_window(), Some(first), "setup: the second window is showing");
    type_header(&mut h, "Q3 report");
    // The first window comes forward without a command (which would enter the header first).
    h.state_mut().enter_window(first);
    h.run_steps(3);
    assert!(h.state().grid.hf_edit.is_none(), "setup: the header edit is the second window's");
    assert!(h.state().has_unsaved_work(), "the parked header would be lost");

    let ctx = h.ctx.clone();
    h.state_mut().request_quit(&ctx);
    h.run_steps(3);
    assert_eq!(h.state().dialog.as_ref().map(|d| d.name()), Some("saveChanges"), "Quit asks about the workbook");
    assert_eq!(header(&h), "&LLeft&CQ3 report", "the typed header is in the workbook");
}

// ---------------------------------------------------------------- a header being typed belongs to its sheet

/// Alpha, showing in Page Layout view, and Beta after it, a copy of it: sixty rows, a small table
/// to make a PivotTable of, a sum for Solver to maximise, and the header "Left" each, in a page that
/// keeps the files.
fn two_sheets() -> (Harness, Handed) {
    let mut session = page_layout_session(false, None);
    for (cell, input) in [("C1", "Region"), ("D1", "Sales"), ("C2", "North"), ("D2", "5"), ("C3", "South"), ("D3", "7"), ("E1", "=SUM(A1:A2)")] {
        session.execute("cell.set", json!({"cell": cell, "input": input})).unwrap();
    }
    session.execute("sheet.rename", json!({"name": "Alpha"})).unwrap();
    session.execute("sheet.move", json!({"sheet": 0, "to": 1, "copy": true})).unwrap();
    session.execute("sheet.rename", json!({"sheet": 1, "name": "Beta"})).unwrap();
    session.execute("sheet.activate", json!({"sheet": 0})).unwrap();
    hosted_of(session)
}

/// The sheets of `wb` in tab order, each with its header.
fn headers(wb: &Workbook) -> Vec<(String, String)> {
    wb.sheets.iter().map(|s| (s.name.clone(), s.print.header.clone())).collect()
}

/// What the tabs show, left to right: the sheet names.
fn tabs(h: &Harness) -> Vec<String> {
    h.state().session.doc().unwrap().wb.sheets.iter().map(|s| s.name.clone()).collect()
}

/// The last workbook the page was handed.
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

/// Drags with the primary button from `from` to `to`, in moves, as a hand does.
fn drag(h: &mut Harness, from: Pos2, to: Pos2) {
    press(h, from);
    for i in 1..=4 {
        mouse(h, Event::PointerMoved(from + (to - from) * (i as f32 / 4.0)));
    }
    release(h, to);
}

/// Reordering the tabs by dragging one: the header typed in the sheet whose tab is dragged goes
/// with that sheet, not with the one that takes its place among the tabs.
#[test]
fn dragging_the_tab_of_a_sheet_with_a_header_being_typed_keeps_the_header_in_that_sheet() {
    let (mut h, handed) = two_sheets();
    type_header(&mut h, "Only for Alpha");
    let (from, to) = (tab_centre(&h, "Alpha"), tab_centre(&h, "Beta"));
    drag(&mut h, from, to);
    assert_eq!(tabs(&h), ["Beta", "Alpha"], "setup: the tab was dragged past Beta's");

    h.state_mut().run_or_alert("file.save", json!({}));
    h.run_steps(3);
    let saved = last_saved(&handed);
    assert_eq!(
        headers(&saved),
        [("Beta".to_string(), "&LLeft".to_string()), ("Alpha".to_string(), "&LLeft&COnly for Alpha".to_string())],
        "the file has the header in the sheet it was typed in"
    );
    assert!(h.state().grid.hf_edit.is_none());
    assert!(!h.state().has_unsaved_work(), "the tab reports clean: nothing else is pending");
}

/// What happens to the tabs while a draft is open: the sheet the draft is typed in, the command and
/// its parameters. Some move, add, delete or rename sheets (the tab menu, the New sheet button,
/// Move or Copy…, Delete); some write their result on a new sheet before the active one; the last
/// only switch sheets, so they move nothing.
fn tab_changes() -> Vec<(&'static str, &'static str, serde_json::Value)> {
    let pivot = |sheet: &str| json!({"source": format!("{sheet}!C1:D3"), "rows": ["Region"], "values": ["Sales"]});
    let forecast = json!({"timeline": "A1:A60", "values": "A1:A60"});
    let model = json!({"objective": "E1", "variables": "A1:A2", "constraints": ["E1 <= 20"], "method": "simplex"});
    let solve = json!({"objective": "E1", "variables": "A1:A2", "constraints": ["E1 <= 20"], "method": "simplex", "reports": ["answer"]});
    vec![
        ("Alpha", "sheet.move", json!({"sheet": 0, "to": 1})),
        ("Beta", "sheet.move", json!({"sheet": 0, "to": 1})),
        ("Alpha", "home.insertSheet", json!({"before": 0})),
        ("Alpha", "home.insertSheet", json!({})),
        ("Beta", "home.deleteSheet", json!({"sheet": 0})),
        ("Alpha", "home.deleteSheet", json!({"sheet": 1})),
        ("Alpha", "insert.pivotTable", pivot("Alpha")),
        ("Beta", "insert.pivotTable", pivot("Beta")),
        ("Alpha", "data.forecastSheet", forecast.clone()),
        ("Beta", "data.forecastSheet", forecast),
        ("Alpha", "data.solver", solve.clone()),
        ("Beta", "data.solver", solve),
        ("Alpha", "data.solverReport", json!({"reports": ["answer"], "model": model.clone()})),
        ("Beta", "data.solverReport", json!({"reports": ["answer"], "model": model})),
        ("Alpha", "sheet.activate", json!({"sheet": 1})),
        ("Beta", "sheet.activate", json!({"sheet": 0})),
        ("Alpha", "sheet.next", json!({})),
        ("Beta", "sheet.previous", json!({})),
    ]
}

/// Makes `typed_in` the sheet showing.
fn show_sheet(h: &mut Harness, typed_in: &str) {
    if typed_in == "Beta" {
        h.state_mut().run("sheet.activate", json!({"sheet": 1})).unwrap();
        h.run_steps(4);
    }
}

/// Runs `command`, then Save, as the tab would; any reason the command was refused for.
fn run_then_save(h: &mut Harness, command: &str, params: &serde_json::Value) -> Option<String> {
    h.state_mut().run_or_alert(command, params.clone());
    h.run_steps(3);
    let refused = h.state().message.as_ref().map(|(_, m)| m.clone());
    save(h);
    refused
}

/// Every one of them: the header ends up in the sheet it was typed in, and the file has no other
/// change.
#[test]
fn a_header_being_typed_is_entered_into_its_own_sheet_however_the_tabs_change() {
    let mut wrong = Vec::new();
    for (typed_in, command, params) in tab_changes() {
        let (mut h, handed) = two_sheets();
        show_sheet(&mut h, typed_in);
        type_header(&mut h, "Typed");
        if let Some(refused) = run_then_save(&mut h, command, &params) {
            wrong.push(format!("typed in {typed_in}, then {command} {params}: refused, {refused}"));
        }

        let saved = headers(&last_saved(&handed));
        let want = |name: &str| match name {
            n if n == typed_in => "&LLeft&CTyped",
            "Alpha" | "Beta" => "&LLeft",
            _ => "",
        };
        if !saved.iter().any(|(name, _)| name == typed_in) || saved.iter().any(|(name, header)| header != want(name)) {
            wrong.push(format!("typed in {typed_in}, then {command} {params}: the file has {saved:?}"));
        }
        if h.state().has_unsaved_work() {
            wrong.push(format!("typed in {typed_in}, then {command} {params}: the tab is not clean after Save"));
        }
    }
    assert!(wrong.is_empty(), "a header went into another sheet, or was lost: {wrong:#?}");
}

/// The same with a cell being typed, the other draft of a sheet: it is entered into the sheet it
/// was typed in and nowhere else, whatever then changes the tabs.
#[test]
fn a_cell_being_typed_is_entered_into_its_own_sheet_however_the_tabs_change() {
    let h12 = CellRef::parse("H12").unwrap();
    let mut wrong = Vec::new();
    for (typed_in, command, params) in tab_changes() {
        let (mut h, handed) = two_sheets();
        show_sheet(&mut h, typed_in);
        type_cell(&mut h, "H12", "hello");
        if let Some(refused) = run_then_save(&mut h, command, &params) {
            wrong.push(format!("typed in {typed_in}, then {command} {params}: refused, {refused}"));
        }

        let saved = last_saved(&handed);
        let entered_in: Vec<&str> =
            saved.sheets.iter().filter(|s| s.cell(h12).is_some_and(|c| c.value == Value::text("hello"))).map(|s| s.name.as_str()).collect();
        if entered_in != [typed_in] {
            wrong.push(format!("typed in {typed_in}, then {command} {params}: the file has it in {entered_in:?}"));
        }
        if h.state().has_unsaved_work() {
            wrong.push(format!("typed in {typed_in}, then {command} {params}: the tab is not clean after Save"));
        }
    }
    assert!(wrong.is_empty(), "a cell went into another sheet, or was lost: {wrong:#?}");
}

/// Renaming the sheet through its tab's name field, with its header still being typed.
#[test]
fn renaming_the_sheet_of_a_header_being_typed_keeps_the_header_in_it() {
    let (mut h, handed) = two_sheets();
    type_header(&mut h, "Typed");
    gridcraft_ui_egui::tabs::begin_tab_rename(h.state_mut(), 0);
    h.run_steps(3);
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.run_steps(2);
    h.input_mut().events.push(Event::Text("Budget".into()));
    h.run_steps(3);
    h.input_mut().events.push(Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.input_mut().events.push(Event::Key { key: Key::Enter, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
    assert_eq!(tabs(&h), ["Budget", "Beta"], "setup: the sheet is renamed");

    h.state_mut().run_or_alert("file.save", json!({}));
    h.run_steps(3);
    assert_eq!(
        headers(&last_saved(&handed)),
        [("Budget".to_string(), "&LLeft&CTyped".to_string()), ("Beta".to_string(), "&LLeft".to_string())],
        "the renamed sheet has the header"
    );
    assert!(!h.state().has_unsaved_work());
}

/// A header edit belongs to the sheet it was typed in, not to the place that sheet had: if the tabs
/// change under it in a way that doesn't enter it first (a script, another window), it still goes
/// into its own sheet and never into whichever sheet now has the place.
#[test]
fn a_header_edit_is_never_entered_into_another_sheet() {
    let (mut h, _) = two_sheets();
    type_header(&mut h, "Typed");
    h.state_mut().session.run("sheet.move", json!({"sheet": 0, "to": 1})).unwrap();
    assert_eq!(tabs(&h), ["Beta", "Alpha"], "setup: Beta has Alpha's place");

    gridcraft_ui_egui::paper_view::commit(h.state_mut());
    assert_eq!(
        headers(&h.state().session.doc().unwrap().wb),
        [("Beta".to_string(), "&LLeft".to_string()), ("Alpha".to_string(), "&LLeft&CTyped".to_string())]
    );
}

/// The same drag with a cell being typed, the other draft of a sheet: it is entered into the sheet
/// it was typed in, not into the one that takes its place among the tabs.
#[test]
fn dragging_the_tab_of_a_sheet_with_a_cell_being_typed_enters_the_cell_in_that_sheet() {
    let (mut h, handed) = two_sheets();
    h.state_mut().run("view.normal", json!({})).unwrap();
    h.run_steps(4);
    h.input_mut().events.push(Event::Text("hello".into()));
    h.run_steps(3);
    assert_eq!(h.state().editor.as_ref().map(|e| e.text.as_str()), Some("hello"), "setup: A1 of Alpha is being typed");
    let (from, to) = (tab_centre(&h, "Alpha"), tab_centre(&h, "Beta"));
    drag(&mut h, from, to);
    assert_eq!(tabs(&h), ["Beta", "Alpha"], "setup: the tab was dragged past Beta's");

    h.state_mut().run_or_alert("file.save", json!({}));
    h.run_steps(3);
    let saved = last_saved(&handed);
    let a1 = |sheet: &str| saved.sheets.iter().find(|s| s.name == sheet).unwrap().cell(CellRef::parse("A1").unwrap()).map(|c| c.value.clone());
    assert_eq!(a1("Alpha"), Some(Value::text("hello")), "the file has what was typed in the sheet it was typed in");
    assert_eq!(a1("Beta"), Some(Value::Number(1.0)), "and Beta is as it was");
    assert!(!h.state().has_unsaved_work());
}

/// A header being typed in a window that is not showing keeps to its sheet too, when the tabs are
/// moved in another window of the workbook.
#[test]
fn a_header_typed_in_another_window_stays_with_its_sheet_when_the_tabs_move() {
    let (mut h, handed) = two_sheets();
    let first = h.state().session.active_window().unwrap();
    h.state_mut().run("view.newWindow", json!({})).unwrap();
    h.run_steps(4);
    let second = h.state().session.active_window().unwrap();
    assert_ne!(first, second, "setup: the second window is showing");
    type_header(&mut h, "Typed");
    // The first window comes forward without a command (which would enter the header first) and
    // moves Alpha's tab.
    h.state_mut().enter_window(first);
    h.run_steps(3);
    assert!(h.state().grid.hf_edit.is_none(), "setup: the header edit is the second window's");
    h.state_mut().run_or_alert("sheet.move", json!({"sheet": 0, "to": 1}));
    h.run_steps(3);
    assert_eq!(tabs(&h), ["Beta", "Alpha"], "setup: the tab moved");
    assert!(h.state().has_unsaved_work(), "the parked header would be lost");

    h.state_mut().enter_window(second);
    h.run_steps(3);
    h.state_mut().run_or_alert("file.save", json!({}));
    h.run_steps(3);
    assert_eq!(
        headers(&last_saved(&handed)),
        [("Beta".to_string(), "&LLeft".to_string()), ("Alpha".to_string(), "&LLeft&CTyped".to_string())],
        "the header is in the sheet it was typed in"
    );
    assert!(!h.state().has_unsaved_work());
}

// ---------------------------------------------------------------- Undo, Redo, Show Details and macros end a draft first

/// What the sheet named `sheet` of `wb` holds in `cell`.
fn value_in(wb: &Workbook, sheet: &str, cell: &str) -> Option<Value> {
    wb.sheets.iter().find(|s| s.name == sheet).and_then(|s| s.cell(CellRef::parse(cell).unwrap())).map(|c| c.value.clone())
}

/// Undo with a header being typed enters the header first, as a click elsewhere does, and takes
/// that back: not the rename before it, and not the header's sheet, which it used to leave under
/// the draft. Redo brings the header back.
#[test]
fn undo_enters_the_header_being_typed_first_and_takes_back_that_not_the_rename() {
    let (mut h, handed) = two_sheets();
    h.state_mut().run_or_alert("sheet.rename", json!({"name": "Budget"}));
    h.run_steps(3);
    assert_eq!(tabs(&h), ["Budget", "Beta"], "setup: the sheet is renamed");
    save(&mut h);
    type_header(&mut h, "Typed");

    button(&mut h, "Undo");
    assert!(h.state().grid.hf_edit.is_none(), "the header was entered");
    assert_eq!(tabs(&h), ["Budget", "Beta"], "and the Undo took that back, not the rename");
    assert_eq!(header(&h), "&LLeft");
    assert!(h.state().message.is_none(), "nothing is lost: Redo has it");

    button(&mut h, "Undo");
    assert_eq!(tabs(&h), ["Alpha", "Beta"], "the next Undo takes the rename back");
    button(&mut h, "Redo");
    button(&mut h, "Redo");
    assert_eq!(tabs(&h), ["Budget", "Beta"]);
    save(&mut h);
    assert_eq!(
        headers(&last_saved(&handed)),
        [("Budget".to_string(), "&LLeft&CTyped".to_string()), ("Beta".to_string(), "&LLeft".to_string())],
        "Redo brought the typed header back into the sheet it was typed in"
    );
    assert!(!h.state().has_unsaved_work());
}

/// The same when the Undo is of the Insert Sheet that made the sheet the header is typed in: the
/// sheet is not taken away under the draft (which dropped it); the header is entered, and that is
/// what Undo takes back. The next Undo takes the sheet away, and two Redos bring both back.
#[test]
fn undoing_an_insert_sheet_enters_the_header_typed_in_the_new_sheet_first() {
    let (mut h, handed) = two_sheets();
    h.state_mut().run_or_alert("home.insertSheet", json!({}));
    h.state_mut().run_or_alert("view.pageLayout", json!({}));
    h.run_steps(4);
    let new = tabs(&h)[1].clone();
    assert_eq!(h.state().session.doc().unwrap().wb.active().unwrap().name, new, "setup: the new sheet is showing");
    save(&mut h);
    type_header_over(&mut h, "Typed", "");

    button(&mut h, "Undo");
    assert!(h.state().grid.hf_edit.is_none(), "the header was entered");
    assert_eq!(tabs(&h), ["Alpha", new.as_str(), "Beta"], "the new sheet stays: the Undo took back the header");
    assert_eq!(header(&h), "");
    assert!(h.state().message.is_none());

    button(&mut h, "Undo");
    assert_eq!(tabs(&h), ["Alpha", "Beta"], "the next Undo takes the new sheet away");
    button(&mut h, "Redo");
    button(&mut h, "Redo");
    save(&mut h);
    assert_eq!(
        headers(&last_saved(&handed)),
        [("Alpha".to_string(), "&LLeft".to_string()), (new, "&CTyped".to_string()), ("Beta".to_string(), "&LLeft".to_string())],
        "the typed header is in the new sheet"
    );
    assert!(!h.state().has_unsaved_work());
}

/// The same with a cell being typed: it used to go into whichever sheet had the new sheet's place.
#[test]
fn undoing_an_insert_sheet_enters_the_cell_typed_in_the_new_sheet_first() {
    let (mut h, handed) = two_sheets();
    h.state_mut().run_or_alert("home.insertSheet", json!({}));
    h.run_steps(3);
    let new = tabs(&h)[1].clone();
    save(&mut h);
    type_cell(&mut h, "H12", "hello");

    button(&mut h, "Undo");
    assert!(h.state().editor.is_none(), "the cell was entered");
    assert_eq!(tabs(&h), ["Alpha", new.as_str(), "Beta"], "the new sheet stays: the Undo took back the cell");
    button(&mut h, "Undo");
    assert_eq!(tabs(&h), ["Alpha", "Beta"], "the next Undo takes the new sheet away");
    button(&mut h, "Redo");
    button(&mut h, "Redo");
    save(&mut h);
    let saved = last_saved(&handed);
    assert_eq!(value_in(&saved, &new, "H12"), Some(Value::text("hello")), "the cell is in the sheet it was typed in");
    assert_eq!(value_in(&saved, "Alpha", "H12"), None);
    assert_eq!(value_in(&saved, "Beta", "H12"), None);
    assert!(!h.state().has_unsaved_work());
}

/// Redo with a header being typed enters it, which ends what can be redone, as typing after an
/// Undo does; the rename is not redone under it, which dropped the header.
#[test]
fn redo_enters_the_header_being_typed_first_instead_of_renaming_its_sheet_under_it() {
    let (mut h, handed) = two_sheets();
    h.state_mut().run_or_alert("sheet.rename", json!({"name": "Budget"}));
    h.state_mut().run_or_alert("edit.undo", json!({}));
    h.run_steps(3);
    assert_eq!(tabs(&h), ["Alpha", "Beta"], "setup: the rename is undone, and could be redone");
    save(&mut h);
    type_header(&mut h, "Typed");

    button(&mut h, "Redo");
    assert!(h.state().grid.hf_edit.is_none(), "the header was entered");
    assert_eq!(tabs(&h), ["Alpha", "Beta"], "the rename was not redone under it");
    save(&mut h);
    assert_eq!(
        headers(&last_saved(&handed)),
        [("Alpha".to_string(), "&LLeft&CTyped".to_string()), ("Beta".to_string(), "&LLeft".to_string())],
        "the file has the header"
    );
    assert!(!h.state().has_unsaved_work());
}

/// The same with a cell: Redo of an Insert Sheet before the sheet the cell is typed in used to
/// put the cell into the new sheet.
#[test]
fn redo_enters_the_cell_being_typed_first_instead_of_inserting_a_sheet_before_it() {
    let (mut h, handed) = two_sheets();
    h.state_mut().run_or_alert("home.insertSheet", json!({"before": 0}));
    h.state_mut().run_or_alert("edit.undo", json!({}));
    h.run_steps(3);
    assert_eq!(tabs(&h), ["Alpha", "Beta"], "setup: the new sheet is undone, and could be redone");
    save(&mut h);
    type_cell(&mut h, "H12", "hello");

    button(&mut h, "Redo");
    assert!(h.state().editor.is_none(), "the cell was entered");
    assert_eq!(tabs(&h), ["Alpha", "Beta"], "the sheet was not redone before it");
    save(&mut h);
    let saved = last_saved(&handed);
    assert_eq!(value_in(&saved, "Alpha", "H12"), Some(Value::text("hello")), "the file has the cell in the sheet it was typed in");
    assert_eq!(value_in(&saved, "Beta", "H12"), None);
    assert!(!h.state().has_unsaved_work());
}

/// Opening the Undo list enters a header being typed, so that the list names the steps there
/// are: choosing the rename before it undoes both, as the list says.
#[test]
fn the_undo_list_names_the_steps_there_are_with_a_header_being_typed_entered() {
    let (mut h, _) = two_sheets();
    h.state_mut().run_or_alert("sheet.rename", json!({"name": "Budget"}));
    h.run_steps(3);
    save(&mut h);
    type_header(&mut h, "Typed");

    h.get_by_label("Undo list").click();
    h.run_steps(3);
    assert!(h.state().grid.hf_edit.is_none(), "opening the list entered the header");
    let labels: Vec<String> = h.state().session.doc().unwrap().undo.iter().rev().take(2).map(|e| e.label.clone()).collect();
    // Earlier steps are renames too: the entry nearest the top is the one for the rename before the header.
    let both = format!("Undo {}", labels[1]);
    h.query_all_by_label(&both).next().unwrap_or_else(|| panic!("the list has no {both}")).click();
    h.run_steps(3);
    assert_eq!(tabs(&h), ["Alpha", "Beta"], "{both}: the entry and the rename before it are taken back");
}

/// Show Details puts the records on a new sheet before the PivotTable's, so a draft typed in the
/// PivotTable's sheet is entered first, into that sheet (the cell used to land in the new one).
#[test]
fn show_details_enters_a_draft_in_the_pivot_sheet_before_its_new_sheet_goes_in_front_of_it() {
    let mut wrong = Vec::new();
    for draft in ["header", "cell"] {
        let (mut h, handed) = two_sheets();
        h.state_mut().run("insert.pivotTable", json!({"source": "Alpha!C1:D3", "rows": ["Region"], "values": ["Sales"]})).unwrap();
        h.run_steps(4);
        let (pivot, value) = {
            let sh = h.state().session.doc().unwrap().wb.active().unwrap().clone();
            let area = sh.pivots[0].last_range.expect("the report is laid out");
            (sh.name.clone(), CellRef::new(area.start.row + 1, area.start.col + 1).a1())
        };
        save(&mut h);
        if draft == "header" {
            h.state_mut().run("view.pageLayout", json!({})).unwrap();
            h.run_steps(4);
            type_header_over(&mut h, "Typed", "");
        } else {
            type_cell(&mut h, "H12", "hello");
        }
        if let Some(refused) = run_then_save(&mut h, "pivot.showDetails", &json!({"cell": value})) {
            wrong.push(format!("{draft}: refused, {refused}"));
        }

        let saved = last_saved(&handed);
        if saved.sheets.len() != 4 {
            wrong.push(format!("{draft}: the details sheet was not made: {:?}", saved.sheets.iter().map(|s| &s.name).collect::<Vec<_>>()));
        }
        let with_it: Vec<&str> = if draft == "header" {
            saved.sheets.iter().filter(|s| s.print.header.contains("Typed")).map(|s| s.name.as_str()).collect()
        } else {
            saved.sheets.iter().filter(|s| value_in(&saved, &s.name, "H12") == Some(Value::text("hello"))).map(|s| s.name.as_str()).collect()
        };
        if with_it != [pivot.as_str()] {
            wrong.push(format!("{draft}: typed in {pivot}, the file has it in {with_it:?}"));
        }
        if h.state().has_unsaved_work() {
            wrong.push(format!("{draft}: the tab is not clean after Save"));
        }
    }
    assert!(wrong.is_empty(), "a draft went into another sheet, or was lost: {wrong:#?}");
}

/// A macro or a script can rename a sheet or add one before it, so a draft typed in the sheet is
/// entered into it first, as for the commands that do so by name.
#[test]
fn a_draft_is_entered_into_its_own_sheet_before_a_macro_or_script_changes_the_tabs() {
    let mut wrong = Vec::new();
    for run in ["macro.run", "automate.runScript"] {
        for draft in ["header", "cell"] {
            let (mut h, handed) = two_sheets();
            let (command, params) =
                if draft == "header" { ("sheet.rename", json!({"name": "Budget"})) } else { ("home.insertSheet", json!({"before": 0})) };
            let step = json!({"command": command, "params": params});
            if run == "macro.run" {
                h.state_mut().run("macro.save", json!({"macro": {"name": "Steps", "steps": [step]}})).unwrap();
            } else {
                h.state_mut().run("automate.newScript", json!({"name": "Steps", "commands": [step]})).unwrap();
            }
            save(&mut h);
            if draft == "header" {
                type_header(&mut h, "Typed");
            } else {
                type_cell(&mut h, "H12", "hello");
            }
            if let Some(refused) = run_then_save(&mut h, run, &json!({"name": "Steps"})) {
                wrong.push(format!("{draft}, then {run}: refused, {refused}"));
            }

            let saved = last_saved(&handed);
            if draft == "header" {
                let want = [("Budget".to_string(), "&LLeft&CTyped".to_string()), ("Beta".to_string(), "&LLeft".to_string())];
                if headers(&saved) != want {
                    wrong.push(format!("{draft}, then {run}: the file has {:?}", headers(&saved)));
                }
            } else {
                let with_it: Vec<&str> =
                    saved.sheets.iter().filter(|s| value_in(&saved, &s.name, "H12") == Some(Value::text("hello"))).map(|s| s.name.as_str()).collect();
                if with_it != ["Alpha"] || saved.sheets.len() != 3 {
                    wrong.push(format!("{draft}, then {run}: the file has it in {with_it:?} of {} sheets", saved.sheets.len()));
                }
            }
            if h.state().has_unsaved_work() {
                wrong.push(format!("{draft}, then {run}: the tab is not clean after Save"));
            }
        }
    }
    assert!(wrong.is_empty(), "a draft went into another sheet, or was lost: {wrong:#?}");
}

/// If something that doesn't enter the header first (a script in another window) takes its sheet
/// away, the header can't be entered, and it is not dropped unseen: the message gives what was typed.
#[test]
fn a_header_whose_sheet_is_gone_is_said_not_dropped_unseen() {
    let (mut h, _) = two_sheets();
    type_header(&mut h, "Typed");
    h.state_mut().session.run("sheet.rename", json!({"sheet": 0, "name": "Budget"})).unwrap();
    assert!(h.state().message.is_none(), "setup: nothing has been said");

    gridcraft_ui_egui::paper_view::commit(h.state_mut());
    let said = h.state().message.as_ref().map(|(_, m)| m.clone()).expect("what was typed is said");
    assert!(said.contains("Typed") && said.contains("Alpha") && said.contains("header"), "{said}");
    assert!(h.state().grid.hf_edit.is_none());
}
