//! The Comments pane: one Show Comments button, and a pane that keeps its width (#710). A reply
//! typed and not sent is unsaved work: closing or quitting asks about it, workbook by workbook.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use egui::{Event, Key, Modifiers, PointerButton, Pos2, pos2, vec2};
use egui_kittest::kittest::Queryable;
use gridcraft_engine::Session;
use gridcraft_engine::model::Workbook;
use gridcraft_ui_egui::{HostSave, Services, SheetApp};
use serde_json::json;

type Harness = egui_kittest::Harness<'static, SheetApp>;

const WIDTH: f32 = 1200.0;

fn harness() -> Harness {
    let mut session = Session::new();
    session.new_workbook();
    harness_of(crate::common::app(session))
}

fn harness_of(app: SheetApp) -> Harness {
    let mut h = crate::common::builder().with_step_dt(1.0 / 60.0).with_size(vec2(WIDTH, 800.0)).build_ui_state(
        |ui, app: &mut SheetApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    h
}

fn grid_right(h: &Harness) -> f32 {
    h.state().grid.rect.unwrap().right()
}

/// A comment whose author line and reply row are wide enough to push the pane if they don't wrap.
fn open_pane_on_long_comment(h: &mut Harness) {
    let text = "A long comment that should wrap inside the pane instead of pushing it wider. ".repeat(6);
    let author = "Someone With A Rather Long Display Name, Finance Department, Head Office";
    h.state_mut().run("review.newComment", json!({"cell": "B2", "text": text, "author": author})).unwrap();
    h.state_mut().run("review.showComments", json!({"on": true})).unwrap();
    h.run_steps(3);
}

fn drag(h: &mut Harness, from: Pos2, to: Pos2) {
    let press = |pos, pressed| Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
    h.input_mut().events.extend([Event::PointerMoved(from), press(from, true)]);
    h.step();
    for i in 1..=6 {
        let t = i as f32 / 6.0;
        h.input_mut().events.push(Event::PointerMoved(from + (to - from) * t));
        h.step();
    }
    h.input_mut().events.push(press(to, false));
    h.run_steps(3);
}

#[test]
fn the_pane_does_not_grow_on_its_own_and_keeps_a_manual_resize() {
    let mut h = harness();
    // Between the grid and the right edge of the window, before any pane is open.
    let gutter = WIDTH - grid_right(&h);
    open_pane_on_long_comment(&mut h);
    let opened = grid_right(&h);
    assert!(WIDTH - opened < WIDTH / 2.0, "the pane opens at its default width, got {}", WIDTH - opened);
    h.run_steps(60);
    assert_eq!(grid_right(&h), opened, "the pane must not widen itself frame after frame");

    // Drag the pane's left edge 150px to the left: the new width stays.
    let edge = pos2(opened + gutter, 450.0);
    drag(&mut h, edge, edge - vec2(150.0, 0.0));
    let resized = grid_right(&h);
    // The grab point is a few pixels off the exact edge, so allow some slack on the 150px.
    assert!(opened - resized > 120.0, "the drag widens the pane, grid edge {opened} -> {resized}");
    h.run_steps(60);
    assert_eq!(grid_right(&h), resized, "a manual resize must hold");

    // Dragged almost to the window's left edge, the pane stops at half the window.
    let edge = pos2(resized + gutter, 450.0);
    drag(&mut h, edge, pos2(60.0, 450.0));
    assert!(grid_right(&h) > WIDTH / 2.0 - gutter - 30.0, "the pane is capped at half the window, grid edge {}", grid_right(&h));
}

/// How many separate ribbon buttons show `tip` when hovered, scanning the big-button row.
fn buttons_with_tip(h: &mut Harness, tip: &str) -> usize {
    h.ctx.all_styles_mut(|s| s.interaction.tooltip_delay = 0.0);
    let (mut count, mut inside) = (0, false);
    let mut x = 2.0;
    while x < WIDTH {
        h.input_mut().events.push(Event::PointerMoved(pos2(x, 120.0)));
        h.run_steps(6);
        let shown = h.query_all_by_value(tip).next().is_some();
        if shown && !inside {
            count += 1;
        }
        inside = shown;
        x += 8.0;
    }
    count
}

#[test]
fn review_has_a_single_show_comments_button() {
    let mut h = harness();
    h.state_mut().ui.ribbon_tab = "Review".into();
    h.run_steps(2);
    // The scan has to find a known single button, or a count of one below would prove nothing.
    assert_eq!(buttons_with_tip(&mut h, "New Comment"), 1);
    assert_eq!(buttons_with_tip(&mut h, "Comments pane"), 1);
}

/// The Comments pane's reply field, by its hint. A field keeps its hint while it holds text,
/// though only an empty one exposes it as its placeholder.
fn reply_field(h: &Harness) -> egui_kittest::Node<'_> {
    use egui_kittest::kittest::NodeT;
    h.query_all_by_role(egui::accesskit::Role::TextInput)
        .find(|n| n.accesskit_node().data().placeholder() == Some("Reply…"))
        .expect("the reply field is drawn")
}

/// A reply typed under a comment and not sent is unsaved work whether or not the pane is drawn:
/// the app keeps it (the pane shows it again), and sending it ends the draft.
#[test]
fn a_reply_not_sent_is_unsaved_work_with_the_pane_hidden() {
    use egui_kittest::kittest::NodeT;
    let mut h = harness();
    h.state_mut().run("review.newComment", json!({"cell": "B2", "text": "Check the figures"})).unwrap();
    h.state_mut().run("review.showComments", json!({"on": true})).unwrap();
    let saved = std::sync::Arc::clone(&h.state().session.active().unwrap().wb);
    h.state_mut().session.active_mut().unwrap().saved = saved;
    h.run_steps(3);
    assert!(!h.state().has_unsaved_work());

    reply_field(&h).click();
    h.run_steps(1);
    h.event(Event::Text("Will do".into()));
    h.run_steps(2);
    assert!(h.state().has_unsaved_work(), "the typed reply would be lost");
    assert!(!h.state().session.active().unwrap().is_dirty(), "it is not in the workbook");

    // With the pane hidden nothing draws the reply, but it is still there to come back.
    h.state_mut().run("review.showComments", json!({"on": false})).unwrap();
    h.run_steps(3);
    assert!(
        h.query_all_by_role(egui::accesskit::Role::TextInput).all(|n| n.accesskit_node().data().placeholder() != Some("Reply…")),
        "the pane is not drawn"
    );
    assert!(h.state().has_unsaved_work(), "hiding the pane doesn't drop the reply");
    h.state_mut().run("review.showComments", json!({"on": true})).unwrap();
    h.run_steps(3);
    assert_eq!(reply_field(&h).value().as_deref(), Some("Will do"), "the pane shows the reply again");

    // Sending it puts the reply in the workbook and ends the draft.
    h.get_by_label("Send").click();
    h.run_steps(3);
    let replies = h.state().session.active().unwrap().wb.active().unwrap().comments.values().next().unwrap().replies.len();
    assert_eq!(replies, 1);
    let saved = std::sync::Arc::clone(&h.state().session.active().unwrap().wb);
    h.state_mut().session.active_mut().unwrap().saved = saved;
    assert!(!h.state().has_unsaved_work(), "nothing is left to lose");
}

/// The active workbook as saved, so only typing that is not in it yet is unsaved work.
fn mark_saved(app: &mut SheetApp) {
    let saved = std::sync::Arc::clone(&app.session.active().unwrap().wb);
    app.session.active_mut().unwrap().saved = saved;
}

/// Sheets Alpha and Beta (in that order), each with a comment at B2; Alpha is shown, saved, with
/// the Comments pane open.
fn alpha_and_beta(h: &mut Harness) {
    let state = h.state_mut();
    state.run("sheet.rename", json!({"name": "Alpha"})).unwrap();
    state.run("review.newComment", json!({"cell": "B2", "text": "Comment on Alpha"})).unwrap();
    state.run("sheet.move", json!({"sheet": 0, "to": 1, "copy": true})).unwrap();
    state.run("sheet.rename", json!({"sheet": 1, "name": "Beta"})).unwrap();
    state.run("sheet.activate", json!({"sheet": 0})).unwrap();
    state.run("review.showComments", json!({"on": true})).unwrap();
    mark_saved(state);
    h.run_steps(3);
}

/// The names of the sheets, in tab order.
fn sheet_names(h: &Harness) -> Vec<String> {
    h.state().session.active().unwrap().wb.sheets.iter().map(|s| s.name.clone()).collect()
}

/// What the reply field of the shown sheet holds.
fn reply_text(h: &Harness) -> String {
    reply_field(h).value().unwrap_or_default()
}

/// The replies under the comment at B2 of sheet `name`.
fn replies_on(h: &Harness, name: &str) -> Vec<String> {
    let sheet = h.state().session.active().unwrap().wb.sheets.iter().find(|s| s.name == name).unwrap().clone();
    sheet.comments.values().flat_map(|c| c.replies.iter().map(|(_, text)| text.clone())).collect()
}

/// A reply typed on Alpha stays Alpha's when the sheets are reordered while the pane is hidden,
/// and when the move is undone or redone: it is never shown (or sent) under Beta's comment.
#[test]
fn a_reply_not_sent_stays_with_its_sheet_when_the_sheets_are_reordered() {
    let mut h = harness();
    alpha_and_beta(&mut h);
    reply_field(&h).click();
    h.run_steps(1);
    h.event(Event::Text("Only for Alpha".into()));
    h.run_steps(2);
    h.state_mut().run("review.showComments", json!({"on": false})).unwrap();
    h.run_steps(3);

    h.state_mut().run("sheet.move", json!({"sheet": 0, "to": 1})).unwrap();
    mark_saved(h.state_mut());
    h.run_steps(3);
    assert_eq!(sheet_names(&h), ["Beta", "Alpha"]);
    assert!(h.state().has_unsaved_work(), "Alpha's reply is still waiting, though Alpha is not the first sheet any more");

    // Beta is first now, and has no reply.
    h.state_mut().run("sheet.activate", json!({"sheet": 0})).unwrap();
    h.state_mut().run("review.showComments", json!({"on": true})).unwrap();
    h.run_steps(3);
    assert_eq!(reply_text(&h), "", "Beta's comment has no reply typed under it");
    assert!(h.state().has_unsaved_work(), "Alpha's reply still counts while Beta is shown");

    // Undoing the move puts Alpha first again; redoing it puts it last. Either way the reply is Alpha's.
    h.state_mut().run("edit.undo", json!({})).unwrap();
    h.run_steps(3);
    assert_eq!(sheet_names(&h), ["Alpha", "Beta"]);
    h.state_mut().run("sheet.activate", json!({"sheet": 1})).unwrap();
    h.run_steps(3);
    assert_eq!(reply_text(&h), "", "Beta, now last, still has none");
    h.state_mut().run("edit.redo", json!({})).unwrap();
    h.run_steps(3);
    assert_eq!(sheet_names(&h), ["Beta", "Alpha"]);

    // Alpha has it, and Send puts it under Alpha's comment only.
    h.state_mut().run("sheet.activate", json!({"sheet": 1})).unwrap();
    h.run_steps(3);
    assert_eq!(reply_text(&h), "Only for Alpha");
    h.get_by_label("Send").click();
    h.run_steps(3);
    assert_eq!(replies_on(&h, "Alpha"), ["Only for Alpha"]);
    assert!(replies_on(&h, "Beta").is_empty());
    mark_saved(h.state_mut());
    assert!(!h.state().has_unsaved_work(), "nothing is left to lose");
}

/// Deleting the sheet a reply was typed on takes the reply with it: the workbook is left saved
/// and clean, and the sheet that took its place in the tab order shows nothing.
#[test]
fn deleting_the_sheet_takes_its_unsent_reply_with_it() {
    let mut h = harness();
    alpha_and_beta(&mut h);
    // Type the reply on Beta, the last sheet, whose place Alpha never takes.
    h.state_mut().run("sheet.activate", json!({"sheet": 1})).unwrap();
    h.run_steps(3);
    reply_field(&h).click();
    h.run_steps(1);
    h.event(Event::Text("Orphaned reply on Beta".into()));
    h.run_steps(2);
    assert!(h.state().has_unsaved_work());
    h.state_mut().run("review.showComments", json!({"on": false})).unwrap();
    h.run_steps(3);

    h.state_mut().run("home.deleteSheet", json!({"sheet": 1})).unwrap();
    mark_saved(h.state_mut());
    h.run_steps(3);
    assert_eq!(sheet_names(&h), ["Alpha"]);
    assert!(!h.state().has_unsaved_work(), "a reply for a sheet that is gone is nothing to lose");
    h.state_mut().run("review.showComments", json!({"on": true})).unwrap();
    h.run_steps(3);
    assert_eq!(reply_text(&h), "", "Alpha's comment has no reply");
    assert!(!h.state().has_unsaved_work(), "and showing Alpha's pane doesn't bring a flag back");

    // The same for the first sheet: Beta takes its place, and takes nothing from it.
    let mut h = harness();
    alpha_and_beta(&mut h);
    reply_field(&h).click();
    h.run_steps(1);
    h.event(Event::Text("Orphaned reply on Alpha".into()));
    h.run_steps(2);
    h.state_mut().run("review.showComments", json!({"on": false})).unwrap();
    h.run_steps(3);
    h.state_mut().run("home.deleteSheet", json!({"sheet": 0})).unwrap();
    mark_saved(h.state_mut());
    h.run_steps(3);
    assert_eq!(sheet_names(&h), ["Beta"]);
    assert!(!h.state().has_unsaved_work(), "Alpha's reply went with Alpha");
    h.state_mut().run("review.showComments", json!({"on": true})).unwrap();
    h.run_steps(3);
    assert_eq!(reply_text(&h), "", "Beta's comment doesn't show Alpha's reply");
    h.get_by_label("Send").click();
    h.run_steps(3);
    assert!(replies_on(&h, "Beta").is_empty(), "nothing was sent to Beta's comment");
}

/// The reply field drawn under the thread whose comment says `thread`: the nearest one below its
/// text. The pane lists the threads in cell order, which an insertion above doesn't change, so a
/// thread is told apart by what it says and not by its place.
fn reply_field_under<'a>(h: &'a Harness, thread: &str) -> egui_kittest::Node<'a> {
    use egui_kittest::kittest::NodeT;
    let below = h.get_by_label(thread).rect().bottom();
    h.query_all_by_role(egui::accesskit::Role::TextInput)
        .filter(|n| n.accesskit_node().data().placeholder() == Some("Reply…") && n.rect().top() >= below)
        .min_by(|a, b| a.rect().top().total_cmp(&b.rect().top()))
        .unwrap_or_else(|| panic!("a reply field is drawn under {thread:?}"))
}

/// What the reply field under `thread` holds.
fn reply_under(h: &Harness, thread: &str) -> String {
    reply_field_under(h, thread).value().unwrap_or_default()
}

/// Types `text` into the reply field under `thread`.
fn type_reply_under(h: &mut Harness, thread: &str, text: &str) {
    reply_field_under(h, thread).click();
    h.run_steps(1);
    h.event(Event::Text(text.into()));
    h.run_steps(2);
}

/// Clicks the Send button in the row of the reply field under `thread`.
fn send_under(h: &mut Harness, thread: &str) {
    let y = reply_field_under(h, thread).rect().center().y;
    h.get_all_by_label("Send")
        .min_by(|a, b| (a.rect().center().y - y).abs().total_cmp(&(b.rect().center().y - y).abs()))
        .expect("a Send button is drawn")
        .click();
    h.run_steps(3);
}

/// The replies under the comment at `cell` of the shown sheet.
fn replies_at(h: &Harness, cell: &str) -> Vec<String> {
    let at = gridcraft_engine::core::CellRef::parse(cell).unwrap();
    let sheet = h.state().session.active().unwrap().wb.active().unwrap();
    sheet.comments.get(&at).map(|c| c.replies.iter().map(|(_, text)| text.clone()).collect()).unwrap_or_default()
}

/// Sheet Alpha with the comments "First thread" at `first` and "Second thread" at `second`, saved,
/// with the Comments pane open.
fn two_threads(h: &mut Harness, first: &str, second: &str) {
    let state = h.state_mut();
    state.run("sheet.rename", json!({"name": "Alpha"})).unwrap();
    state.run("review.newComment", json!({"cell": first, "text": "First thread"})).unwrap();
    state.run("review.newComment", json!({"cell": second, "text": "Second thread"})).unwrap();
    state.run("review.showComments", json!({"on": true})).unwrap();
    mark_saved(state);
    h.run_steps(3);
}

/// Runs `command`, leaves the workbook saved so that only the reply can be unsaved work, and draws.
fn edit_and_settle(h: &mut Harness, command: &str, params: serde_json::Value) {
    h.state_mut().run(command, params).unwrap();
    mark_saved(h.state_mut());
    h.run_steps(3);
}

/// The reviewer's repro (AI-9882, M1). A reply is typed under the second thread and the pane is
/// hidden. `command` then moves the second thread from `second` to `second_after` and the first from
/// `first` to `first_after`, so that the first now sits at the address the reply was typed at. With
/// the pane back, the reply is under the thread it was typed under and the other thread has none,
/// also after undo and redo; Send posts to the thread that moved and to no other.
fn the_reply_stays_with_its_thread(first: &str, second: &str, command: &str, params: serde_json::Value, first_after: &str, second_after: &str) {
    let mut h = harness();
    two_threads(&mut h, first, second);
    type_reply_under(&mut h, "Second thread", "Will do");
    assert!(h.state().has_unsaved_work(), "the typed reply would be lost");
    h.state_mut().run("review.showComments", json!({"on": false})).unwrap();
    h.run_steps(3);
    assert!(h.state().has_unsaved_work(), "hiding the pane doesn't drop the reply");

    edit_and_settle(&mut h, command, params);
    assert!(h.state().has_unsaved_work(), "{command}: the reply is unsent work, and the workbook is saved");
    h.state_mut().run("review.showComments", json!({"on": true})).unwrap();
    h.run_steps(3);
    assert_eq!(reply_under(&h, "Second thread"), "Will do", "{command}: the reply is under the thread it was typed under");
    assert_eq!(reply_under(&h, "First thread"), "", "{command}: the thread now at {second} has no reply typed under it");
    assert!(h.state().has_unsaved_work(), "{command}: showing the pane doesn't change that");

    // Undo puts both threads back, redo moves them again: the reply stays with its thread.
    edit_and_settle(&mut h, "edit.undo", json!({}));
    assert_eq!(reply_under(&h, "Second thread"), "Will do", "{command}, undone");
    assert_eq!(reply_under(&h, "First thread"), "", "{command}, undone");
    assert!(h.state().has_unsaved_work(), "{command}, undone");
    edit_and_settle(&mut h, "edit.redo", json!({}));
    assert_eq!(reply_under(&h, "Second thread"), "Will do", "{command}, redone");
    assert_eq!(reply_under(&h, "First thread"), "", "{command}, redone");
    assert!(h.state().has_unsaved_work(), "{command}, redone");

    // Sending a blank field posts nothing; sending the reply posts it to the thread it is for.
    send_under(&mut h, "First thread");
    assert!(replies_at(&h, first_after).is_empty() && replies_at(&h, second_after).is_empty(), "{command}: an empty field sends nothing");
    send_under(&mut h, "Second thread");
    assert_eq!(replies_at(&h, second_after), ["Will do"], "{command}: the reply is in the thread it was typed under");
    assert!(replies_at(&h, first_after).is_empty(), "{command}: the thread now at {second} got none");
    mark_saved(h.state_mut());
    assert!(!h.state().has_unsaved_work(), "{command}: sending it leaves nothing to lose");
    assert_eq!(reply_under(&h, "Second thread"), "", "{command}: and the field is empty again");
}

#[test]
fn a_reply_not_sent_stays_with_its_thread_when_a_row_is_inserted_above() {
    the_reply_stays_with_its_thread("B1", "B2", "home.insertRows", json!({"rows": "1:1"}), "B2", "B3");
}

#[test]
fn a_reply_not_sent_stays_with_its_thread_when_a_column_is_inserted_to_the_left() {
    the_reply_stays_with_its_thread("A2", "B2", "home.insertColumns", json!({"cols": "A:A"}), "B2", "C2");
}

#[test]
fn a_reply_not_sent_stays_with_its_thread_when_cells_are_inserted_shifting_down() {
    the_reply_stays_with_its_thread("B1", "B2", "home.insertCells", json!({"range": "B1:B1", "shift": "down"}), "B2", "B3");
}

/// A comment written over the one a reply was typed under is another comment, though it sits at the
/// same cell and says the same: it has no reply, and the old reply is no longer unsaved work. The
/// same when the comment is edited in place (it is the same comment, but no longer what the reply
/// answers).
#[test]
fn a_comment_written_over_another_does_not_inherit_its_unsent_reply() {
    for how in ["delete and write again", "edit in place"] {
        let mut h = harness();
        h.state_mut().run("review.newComment", json!({"cell": "B2", "text": "Check the figures"})).unwrap();
        h.state_mut().run("review.showComments", json!({"on": true})).unwrap();
        mark_saved(h.state_mut());
        h.run_steps(3);
        type_reply_under(&mut h, "Check the figures", "Will do");
        h.state_mut().run("review.showComments", json!({"on": false})).unwrap();
        h.run_steps(3);
        assert!(h.state().has_unsaved_work(), "{how}: setup: the reply is pending");

        let thread = if how == "edit in place" {
            edit_and_settle(&mut h, "review.newComment", json!({"cell": "B2", "text": "Check the other figures"}));
            "Check the other figures"
        } else {
            h.state_mut().run("review.deleteComment", json!({"cell": "B2"})).unwrap();
            edit_and_settle(&mut h, "review.newComment", json!({"cell": "B2", "text": "Check the figures"}));
            "Check the figures"
        };
        assert!(!h.state().has_unsaved_work(), "{how}: the reply was for a comment that is not there any more");
        h.state_mut().run("review.showComments", json!({"on": true})).unwrap();
        h.run_steps(3);
        assert_eq!(reply_under(&h, thread), "", "{how}: the comment at B2 has no reply");
        send_under(&mut h, thread);
        assert!(replies_at(&h, "B2").is_empty(), "{how}: nothing was sent to it");
        assert!(!h.state().has_unsaved_work(), "{how}");
    }
}

// ---------------------------------------------------------------- closing a workbook with a reply not sent

// A reply is in no file and no Save puts it there (only Send does), and the engine can't see it:
// it asks about unsaved changes. So the app asks, for the workbook alone, whenever one closes.

/// The workbooks a page that keeps the files was handed, in order ([`Services::host_save`]).
type Handed = Rc<RefCell<Vec<Arc<Workbook>>>>;

/// The app in a page that keeps the files, as the extension is: a save hands the workbook over and
/// counts as done.
fn hosted() -> (Harness, Handed) {
    let handed = Handed::default();
    let sink = Rc::clone(&handed);
    let services = Services {
        host_save: Some(Box::new(move |save: &HostSave<'_>| {
            sink.borrow_mut().push(Arc::clone(&save.content));
            Ok(())
        })),
        ..Default::default()
    };
    let mut session = Session::new();
    session.new_workbook();
    (harness_of(crate::common::prepare(SheetApp::new(session, services))), handed)
}

/// A comment `thread` at B2 of the shown workbook and `reply` typed under it and not sent, with
/// the workbook saved and the pane hidden: the reply is all there is to lose.
fn type_unsent_reply(h: &mut Harness, thread: &str, reply: &str) {
    h.state_mut().run("review.newComment", json!({"cell": "B2", "text": thread})).unwrap();
    h.state_mut().run("review.showComments", json!({"on": true})).unwrap();
    mark_saved(h.state_mut());
    h.run_steps(3);
    type_reply_under(h, thread, reply);
    h.state_mut().run("review.showComments", json!({"on": false})).unwrap();
    h.run_steps(3);
}

fn active_uid(h: &Harness) -> u64 {
    h.state().session.active().unwrap().uid
}

fn is_open(h: &Harness, uid: u64) -> bool {
    h.state().session.documents().iter().any(|d| d.uid == uid)
}

fn question(h: &Harness) -> Option<&str> {
    h.state().dialog.as_ref().map(|d| d.name())
}

fn button(h: &mut Harness, label: &str) {
    h.get_by_role_and_label(egui::accesskit::Role::Button, label).click();
    h.run_steps(3);
}

fn press(h: &mut Harness, key: Key, modifiers: Modifiers) {
    for pressed in [true, false] {
        h.input_mut().events.push(Event::Key { key, physical_key: Some(key), pressed, repeat: false, modifiers });
    }
    h.run_steps(3);
}

/// Asks to close the shown workbook the way a user does: File › Close in the File view, Cmd/Ctrl+W
/// on the grid, or the commands behind them.
fn close(h: &mut Harness, route: &str) {
    match route {
        "File view" => {
            h.get_by_label("File").click();
            h.run_steps(3);
            button(h, "Close");
        }
        "Cmd+W" => press(h, Key::W, Modifiers::COMMAND),
        command => {
            h.state_mut().run_or_alert(command, json!({}));
            h.run_steps(3);
        }
    }
}

/// Closing a saved workbook with a reply typed under a comment and not sent asks first, however
/// the close is asked for, though no file would change. There is nothing to save, so the question
/// has no Save; Cancel keeps the workbook and the reply, Don't Save closes the workbook and the
/// reply goes with it.
#[test]
fn closing_a_workbook_with_a_reply_not_sent_asks_first() {
    for route in ["file.close", "window.close", "File view", "Cmd+W"] {
        let mut h = harness();
        type_unsent_reply(&mut h, "Check the figures", "Will do");
        let uid = active_uid(&h);
        assert!(!h.state().session.active().unwrap().is_dirty(), "{route}: setup: the workbook is saved");
        assert!(h.state().document_has_unsaved_work(uid), "{route}: setup: the reply would be lost");

        close(&mut h, route);
        assert_eq!(question(&h), Some("saveChanges"), "{route}: Close asks about the reply");
        assert!(is_open(&h, uid), "{route}: the workbook is still open");
        assert!(
            h.query_by_role_and_label(egui::accesskit::Role::Button, "Save").is_none(),
            "{route}: a question about a reply alone has no Save, which would put nothing in the file"
        );
        h.get_by_label_contains("isn't sent");

        button(&mut h, "Cancel");
        assert!(question(&h).is_none());
        assert!(is_open(&h, uid), "{route}: Cancel keeps the workbook");
        assert!(h.state().has_unsaved_work(), "{route}: and the reply");
        h.state_mut().run("review.showComments", json!({"on": true})).unwrap();
        h.run_steps(3);
        assert_eq!(reply_under(&h, "Check the figures"), "Will do", "{route}: the pane still has it");
        h.state_mut().run("review.showComments", json!({"on": false})).unwrap();
        h.run_steps(3);

        close(&mut h, "file.close");
        button(&mut h, "Don't Save");
        assert!(!is_open(&h, uid), "{route}: Don't Save closes the workbook");
        assert!(!h.state().has_unsaved_work(), "{route}: and the reply went with it");
    }
}

/// What closing asks about is the workbook it closes: its own reply, never another workbook's.
#[test]
fn closing_asks_about_the_replies_of_that_workbook_only() {
    // Book1 has a reply. Book2, made next and shown, has none: it closes at once, Book1 keeps its reply.
    let mut h = harness();
    type_unsent_reply(&mut h, "Thread in Book1", "Reply in Book1");
    let first = active_uid(&h);
    h.state_mut().run("file.new", json!({})).unwrap();
    h.run_steps(3);
    let second = active_uid(&h);
    assert_ne!(first, second, "setup: Book2 is the shown workbook");
    assert!(h.state().has_unsaved_work() && !h.state().document_has_unsaved_work(second), "setup: only Book1 has a reply");

    close(&mut h, "file.close");
    assert!(question(&h).is_none(), "Book2 has nothing to lose: Book1's reply is no part of closing it");
    assert!(!is_open(&h, second) && is_open(&h, first));
    assert_eq!(active_uid(&h), first, "Book1 comes forward");
    assert!(h.state().document_has_unsaved_work(first), "and still has its reply");

    // Both have a reply. Cancel keeps both; Don't Save closes the one asked about and the other keeps its reply.
    let mut h = harness();
    type_unsent_reply(&mut h, "Thread in Book1", "Reply in Book1");
    let first = active_uid(&h);
    h.state_mut().run("file.new", json!({})).unwrap();
    h.run_steps(3);
    type_unsent_reply(&mut h, "Thread in Book2", "Reply in Book2");
    let second = active_uid(&h);
    assert!(h.state().document_has_unsaved_work(first) && h.state().document_has_unsaved_work(second));

    close(&mut h, "file.close");
    assert_eq!(question(&h), Some("saveChanges"));
    assert_eq!(active_uid(&h), second, "the question is about the shown workbook");
    button(&mut h, "Cancel");
    assert!(is_open(&h, first) && is_open(&h, second), "Cancel keeps both workbooks");
    assert!(h.state().document_has_unsaved_work(first) && h.state().document_has_unsaved_work(second), "and both replies");

    close(&mut h, "file.close");
    button(&mut h, "Don't Save");
    assert!(!is_open(&h, second) && is_open(&h, first), "Don't Save closes Book2 alone");
    assert_eq!(active_uid(&h), first);
    assert!(h.state().document_has_unsaved_work(first), "Book1 keeps its reply: it was not the one thrown away");
    h.state_mut().run("review.showComments", json!({"on": true})).unwrap();
    h.run_steps(3);
    assert_eq!(reply_under(&h, "Thread in Book1"), "Reply in Book1");
}

/// A window of a workbook that another window still shows closes without a question: nothing is
/// lost with it. The last window of the workbook asks.
#[test]
fn closing_one_of_two_windows_of_a_workbook_with_a_reply_not_sent_asks_nothing() {
    let mut h = harness();
    type_unsent_reply(&mut h, "Check the figures", "Will do");
    let uid = active_uid(&h);
    h.state_mut().run("view.newWindow", json!({})).unwrap();
    h.run_steps(3);
    assert_eq!(h.state().session.windows().len(), 2, "setup: two windows show the workbook");

    close(&mut h, "window.close");
    assert!(question(&h).is_none(), "the workbook stays open in the other window");
    assert_eq!(h.state().session.windows().len(), 1);
    assert!(is_open(&h, uid) && h.state().document_has_unsaved_work(uid), "and the reply is kept");

    close(&mut h, "window.close");
    assert_eq!(question(&h), Some("saveChanges"), "the last window asks");
    assert!(is_open(&h, uid));
    button(&mut h, "Don't Save");
    assert!(!is_open(&h, uid) && !h.state().has_unsaved_work());
}

/// A workbook with unsaved changes and a reply: the question offers Save and says the reply is not
/// part of it. Save writes the changes but leaves the workbook open with a message, as the reply
/// would be lost with it; Don't Save then closes it for good.
#[test]
fn save_writes_the_changes_but_keeps_a_workbook_open_while_a_reply_waits() {
    let (mut h, handed) = hosted();
    type_unsent_reply(&mut h, "Check the figures", "Will do");
    let uid = active_uid(&h);
    h.state_mut().run("cell.set", json!({"cell": "D4", "input": "42"})).unwrap();
    h.run_steps(2);
    assert!(h.state().session.active().unwrap().is_dirty(), "setup: the workbook has a change too");

    close(&mut h, "file.close");
    assert_eq!(question(&h), Some("saveChanges"));
    h.get_by_label_contains("saving doesn't send it");
    button(&mut h, "Save");
    {
        let files = handed.borrow();
        assert_eq!(files.len(), 1, "the changes were saved");
        let sent = files[0].sheet(0).unwrap().comments.values().next().unwrap().replies.len();
        assert_eq!(sent, 0, "the file has the comment and no reply: a reply isn't sent by saving");
    }
    assert!(is_open(&h, uid), "the workbook stays open: closing it would lose the reply");
    assert!(question(&h).is_none());
    assert!(!h.state().session.active().unwrap().is_dirty(), "the changes are saved");
    assert!(h.state().has_unsaved_work(), "the reply is still there");
    let (_, message) = h.state().message.clone().expect("it says why the workbook was not closed");
    assert!(message.contains("reply") && message.contains("Don't Save"), "{message}");
    h.state_mut().message = None;

    close(&mut h, "file.close");
    assert_eq!(question(&h), Some("saveChanges"), "closing again asks about the reply alone");
    assert!(h.query_by_role_and_label(egui::accesskit::Role::Button, "Save").is_none());
    button(&mut h, "Don't Save");
    assert!(!is_open(&h, uid) && !h.state().has_unsaved_work());
    assert_eq!(handed.borrow().len(), 1, "nothing more was written");
}

/// The window's close button (or Alt+F4, or the OS quitting the app): steps until the request has
/// been handled and returns the viewport commands the app answered with.
fn request_window_close(h: &mut Harness) -> Vec<egui::ViewportCommand> {
    h.input_mut().viewports.entry(egui::ViewportId::ROOT).or_default().events.push(egui::ViewportEvent::Close);
    step_commands(h)
}

fn step_commands(h: &mut Harness) -> Vec<egui::ViewportCommand> {
    let mut out = Vec::new();
    for _ in 0..3 {
        h.step();
        out.extend(h.output().viewport_output.get(&egui::ViewportId::ROOT).map(|v| v.commands.clone()).unwrap_or_default());
    }
    out
}

fn click_commands(h: &mut Harness, label: &str) -> Vec<egui::ViewportCommand> {
    h.get_by_role_and_label(egui::accesskit::Role::Button, label).click();
    step_commands(h)
}

/// Closing the window, or quitting, with a reply not sent asks about its workbook and holds the
/// app open until it is answered: Cancel keeps it, Don't Save lets the app close.
#[test]
fn closing_the_window_or_quitting_asks_about_a_reply_not_sent() {
    for how in ["close button", "quit"] {
        let mut h = harness();
        type_unsent_reply(&mut h, "Check the figures", "Will do");
        let ask = |h: &mut Harness| {
            if how == "quit" {
                let ctx = h.ctx.clone();
                h.state_mut().request_quit(&ctx);
                step_commands(h)
            } else {
                request_window_close(h)
            }
        };

        let sent = ask(&mut h);
        assert!(!sent.contains(&egui::ViewportCommand::Close), "{how}: the reply holds the app open: {sent:?}");
        assert_eq!(question(&h), Some("saveChanges"), "{how}: it asks");
        assert!(h.query_by_role_and_label(egui::accesskit::Role::Button, "Save").is_none(), "{how}: nothing to save");
        let sent = click_commands(&mut h, "Cancel");
        assert!(!sent.contains(&egui::ViewportCommand::Close), "{how}: Cancel keeps the app open");
        assert!(h.state().has_unsaved_work(), "{how}: and the reply");

        ask(&mut h);
        let sent = click_commands(&mut h, "Don't Save");
        assert!(sent.contains(&egui::ViewportCommand::Close), "{how}: Don't Save lets the app close: {sent:?}");
        assert!(!h.state().has_unsaved_work(), "{how}: the reply is gone");
    }
}
