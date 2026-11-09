//! The Comments pane: one Show Comments button, and a pane that keeps its width (#710).

use egui::{Event, Modifiers, PointerButton, Pos2, pos2, vec2};
use egui_kittest::kittest::Queryable;
use gridcraft_engine::Session;
use gridcraft_ui_egui::SheetApp;
use serde_json::json;

type Harness = egui_kittest::Harness<'static, SheetApp>;

const WIDTH: f32 = 1200.0;

fn harness() -> Harness {
    let mut session = Session::new();
    session.new_workbook();
    let mut h = crate::common::builder().with_step_dt(1.0 / 60.0).with_size(vec2(WIDTH, 800.0)).build_ui_state(
        |ui, app: &mut SheetApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        crate::common::app(session),
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
