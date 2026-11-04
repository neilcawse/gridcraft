//! Web inside a page that keeps the files (`?host=parent`, [`Services::host_save`]): each file
//! goes to the page with the workbook it was made from, whatever happens next; Save As and the
//! exports say they are new files; a page that confirms its saves keeps a closing workbook open
//! until it has; drafts not yet entered count as unsaved work.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gridcraft_engine::Session;
use gridcraft_engine::model::Workbook;
use gridcraft_ui_egui::{HostSave, SaveAnswer, Services, SheetApp};
use serde_json::json;

/// What the page got: name, workbook, Save As, whether it saves the workbook, and the content.
struct Got {
    name: String,
    document: u64,
    save_as: bool,
    marks_saved: bool,
    content: Arc<Workbook>,
}

type Log = Rc<RefCell<Vec<Got>>>;

/// An app in a host page, with `confirms` as `?ack=1`, and the files handed to the page.
fn host_app(confirms: bool) -> (SheetApp, Log) {
    let log = Log::default();
    let sink = Rc::clone(&log);
    let services = Services {
        host_save: Some(Box::new(move |s: &HostSave<'_>| {
            sink.borrow_mut().push(Got {
                name: s.name.to_string(),
                document: s.document,
                save_as: s.save_as,
                marks_saved: s.marks_saved,
                content: Arc::clone(&s.content),
            });
            Ok(())
        })),
        host_confirms_saves: confirms,
        ..Default::default()
    };
    let mut session = Session::new();
    session.new_workbook();
    (SheetApp::new(session, services), log)
}

fn xlsx(app: &mut SheetApp) -> Vec<u8> {
    let r = app.run("file.saveBytes", json!({"format": "xlsx"})).unwrap();
    gridcraft_engine::io::base64_decode(r["base64"].as_str().unwrap()).unwrap()
}

/// Two unsaved workbooks, the second active: their uids.
fn two_edited(app: &mut SheetApp) -> (u64, u64) {
    app.run("cell.set", json!({"cell": "A1", "input": "Alpha"})).unwrap();
    let first = app.session.active().unwrap().uid;
    app.run("file.new", json!({})).unwrap();
    app.run("cell.set", json!({"cell": "A1", "input": "Bravo"})).unwrap();
    let second = app.session.active().unwrap().uid;
    assert_ne!(first, second);
    (first, second)
}

fn close_and_save(app: &mut SheetApp) {
    app.run("file.close", json!({})).unwrap();
    assert_eq!(app.dialog.as_ref().map(|d| d.name()), Some("saveChanges"));
    app.dialog = None;
    assert!(app.answer_save_changes(SaveAnswer::Save, false, &egui::Context::default()));
}

fn dirty(app: &SheetApp, uid: u64) -> bool {
    app.session.documents().iter().find(|d| d.uid == uid).unwrap().is_dirty()
}

#[test]
fn close_then_save_hands_the_file_with_the_closing_workbook() {
    let (mut app, log) = host_app(false);
    let (first, second) = two_edited(&mut app);
    close_and_save(&mut app);
    let got = log.borrow();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].document, second, "the file is the closing workbook's, not the one active after it closed");
    assert!(!got[0].save_as && got[0].marks_saved);
    drop(got);
    // Handed over is saved without `?ack=1`: the workbook closed, the other one is untouched.
    assert_eq!(app.session.documents().iter().map(|d| d.uid).collect::<Vec<_>>(), vec![first]);
    assert!(dirty(&app, first), "the workbook left open was not saved by the other one's save");
}

#[test]
fn a_host_that_confirms_saves_keeps_the_closing_workbook_until_it_has() {
    let (mut app, log) = host_app(true);
    let (first, second) = two_edited(&mut app);
    let content = Arc::clone(&app.session.active().unwrap().wb);
    close_and_save(&mut app);
    assert_eq!(log.borrow().len(), 1);
    assert_eq!(log.borrow()[0].document, second);
    assert!(Arc::ptr_eq(&log.borrow()[0].content, &content), "the content the file was made from");
    assert_eq!(app.session.documents().len(), 2, "nothing closes before the page has stored the file");
    assert_eq!(app.session.active().unwrap().uid, second);
    assert!(dirty(&app, second) && dirty(&app, first));
    assert!(app.toast.as_ref().is_some_and(|(t, _)| t.contains("Waiting for the save")), "{:?}", app.toast);

    // The page stored it (the bridge marks the workbook saved as it was): Close closes now.
    let d = app.session.document_mut(second).unwrap();
    d.saved = Arc::clone(&content);
    app.run("file.close", json!({})).unwrap();
    assert_eq!(app.session.documents().iter().map(|d| d.uid).collect::<Vec<_>>(), vec![first]);
}

#[test]
fn later_edits_are_not_in_the_handed_over_content() {
    let (mut app, log) = host_app(true);
    app.run("cell.set", json!({"cell": "A1", "input": "1"})).unwrap();
    app.run("file.save", json!({})).unwrap();
    app.run("cell.set", json!({"cell": "A1", "input": "2"})).unwrap();
    let d = app.session.active().unwrap();
    assert!(!Arc::ptr_eq(&log.borrow()[0].content, &d.wb), "storing the save leaves the later edit unsaved");
    assert!(d.is_dirty(), "with `?ack=1` handing over doesn't save");
}

#[test]
fn save_is_not_save_as_but_save_as_and_exports_are() {
    let (mut app, log) = host_app(false);
    app.run("cell.set", json!({"cell": "A1", "input": "1"})).unwrap();
    app.run("file.save", json!({})).unwrap();
    app.run("file.saveAs", json!({})).unwrap();
    assert!(gridcraft_ui_egui::backstage::export_as(&mut app, "csv"));
    app.run("file.exportPdf", json!({})).unwrap();
    let got = log.borrow();
    let flags: Vec<(&str, bool, bool)> = got.iter().map(|g| (g.name.rsplit('.').next().unwrap(), g.save_as, g.marks_saved)).collect();
    assert_eq!(flags, vec![("xlsx", false, true), ("xlsx", true, true), ("csv", true, false), ("pdf", true, false)]);
    let uid = app.session.active().unwrap().uid;
    assert!(got.iter().all(|g| g.document == uid));
    drop(got);
    assert!(!app.session.active().unwrap().is_dirty(), "without `?ack=1` a handed-over workbook is saved");
}

#[test]
fn a_host_that_refuses_the_file_leaves_the_workbook_unsaved() {
    let mut session = Session::new();
    session.new_workbook();
    let services = Services { host_save: Some(Box::new(|_: &HostSave<'_>| Err("the page is gone".into()))), ..Default::default() };
    let mut app = SheetApp::new(session, services);
    app.run("cell.set", json!({"cell": "A1", "input": "1"})).unwrap();
    app.run("file.save", json!({})).unwrap();
    assert!(app.session.active().unwrap().is_dirty());
    assert!(app.message.as_ref().is_some_and(|(_, m)| m.contains("the page is gone")), "{:?}", app.message);
}

#[test]
fn a_sent_file_reports_whether_saves_may_replace_it() {
    let (mut app, _) = host_app(true);
    app.run("cell.set", json!({"cell": "A1", "input": "1"})).unwrap();
    let bytes = xlsx(&mut app);
    let sent = app.open_sent("Report.xlsx", &bytes).unwrap();
    assert_eq!(sent.doc, app.session.active().unwrap().uid);
    assert!(sent.in_place);
    assert!(app.open_sent("Broken.xlsx", b"not a workbook").is_none(), "a file that didn't open is no workbook");
}

#[test]
fn a_draft_not_yet_entered_is_unsaved_work() {
    let (mut app, _) = host_app(true);
    assert!(!app.has_unsaved_work());
    // Typing in a cell, not yet entered.
    app.begin_edit(Some("typed".into()), false);
    assert!(app.has_unsaved_work(), "a typed cell draft");
    assert!(!app.session.active().unwrap().is_dirty(), "nothing was committed to find out");
    app.cancel_edit();
    // F2 on a cell, unchanged, then changed; the formula bar is the same draft.
    app.run("cell.set", json!({"cell": "A1", "input": "kept"})).unwrap();
    let saved = Arc::clone(&app.session.active().unwrap().wb);
    app.session.active_mut().unwrap().saved = saved;
    app.begin_edit(None, true);
    assert!(!app.has_unsaved_work(), "an untouched edit loses nothing");
    app.editor.as_mut().unwrap().text.push('!');
    assert!(app.has_unsaved_work(), "a changed formula-bar draft");
    app.cancel_edit();
    // A text box being edited.
    let id = app.run("insert.textBox", json!({"at": "B2", "text": "Box"})).unwrap()["shape"].as_u64().unwrap() as u32;
    let saved = Arc::clone(&app.session.active().unwrap().wb);
    app.session.active_mut().unwrap().saved = saved;
    assert!(app.begin_text_box_edit(id));
    assert!(!app.has_unsaved_work());
    app.text_box_editor.as_mut().unwrap().text.push_str(" edited");
    assert!(app.has_unsaved_work(), "a changed text box draft");
}
