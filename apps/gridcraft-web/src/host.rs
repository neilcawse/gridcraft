//! The `postMessage` bridge to a same-origin parent page (`?host=parent`), for pages that embed
//! GridCraft in an `<iframe>` (such as a Nextcloud app). Messages are plain objects with a `type`:
//!
//! - `gridcraft:ready` (to the parent): `{ version }`, the app is listening; send a file now.
//! - `gridcraft:open` (from the parent): `{ name, bytes }`, bytes an `ArrayBuffer` or
//!   `Uint8Array`. It opens like a workbook picked with File › Open or dropped on the window.
//! - `gridcraft:opened` (to the parent): `{ name, docId }` once a sent file has opened: `docId`
//!   is the workbook it became, or `null` when saving it must not replace the sent file: it
//!   didn't open, waits behind a password prompt (after which it counts as a workbook the user
//!   opened), was imported (.ods, .xls, .xlsb) or decrypted, which Save never writes over the
//!   file they came from ([`SentFile::in_place`]).
//! - `gridcraft:save` (to the parent): `{ name, bytes, docId, saveId, saveAs, copy }` (`bytes` a
//!   transferred `ArrayBuffer`) for every file GridCraft would otherwise download: Save and Save
//!   As (the workbook, in its own file's format: .xlsx, .xlsm, .csv or .tsv; Save As .xlsx or
//!   .xlsm) and the exports (PDF, CSV, web page). `docId` is the workbook it came
//!   from, taken when the bytes were made (a Save Changes prompt that then closes it can't give
//!   the file to another workbook), the same number `gridcraft:opened` gave for a sent file, so
//!   the parent writes it back only to that file. `saveId` numbers the saves. `saveAs` is true
//!   when the user asked for a new name or place (Save As, Download, an export), false for Save.
//!   `copy` is true for an export: a separate file, never the workbook's own, whatever its
//!   format (a CSV export of an .xlsx workbook must not become the workbook's file).
//! - `gridcraft:saved` (from the parent, with `?ack=1`): `{ saveId, name }`, the save is stored
//!   as `name`. Only then does the workbook count as saved, as it was when it was saved, and a
//!   Save Changes prompt's Save leaves it open until then. Without `?ack=1` a workbook counts as
//!   saved once it is handed over.
//! - `gridcraft:discard` (from the parent, with `?ack=1`): `{ saveId }`, that save will never be
//!   stored (the picker was cancelled, the write failed, a newer save replaced it): the workbook
//!   stays unsaved.
//! - `gridcraft:dirty` (to the parent): `{ dirty }` whenever unsaved work appears or goes away:
//!   a workbook with unsaved changes or a draft not yet entered (a cell being typed in, a text
//!   box or header being edited).
//! - `gridcraft:request-save` (from the parent): `{ saveAs? }` save the active workbook as File ›
//!   Save would, or with `saveAs: true` as File › Save As would (a whole-workbook .xlsx, .xlsm for
//!   a macro-enabled one, instead of a .csv/.tsv), so a parent's own Save and Save As buttons and
//!   shortcuts get a `gridcraft:save` back.
//! - `gridcraft:failed` (to the parent): `{ error }` when the app couldn't start.
//!
//! Only the parent window on the page's own origin is heard or answered. In host mode the page
//! starts with an empty workbook instead of a sample, and the parent guards leaving the page.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use gridcraft_engine::model::Workbook;
use gridcraft_ui_egui::{HostSave, SentFile, Services, SheetApp};
use wasm_bindgen::JsCast as _;
use wasm_bindgen::JsValue;
use wasm_bindgen::closure::Closure;

use crate::pending_saves::PendingSaves;

/// The longest user name taken from `?author=`.
const MAX_AUTHOR_CHARS: usize = 200;

/// Files with their bytes, in the order they came.
type Files = Rc<RefCell<Vec<(String, Vec<u8>)>>>;

#[derive(Clone)]
pub struct Bridge {
    window: web_sys::Window,
    parent: web_sys::Window,
    origin: String,
    /// `?ack=1`: a workbook counts as saved when the parent says it stored it (`gridcraft:saved`),
    /// not when it is handed over.
    ack: bool,
    /// Files the parent sent that the app hasn't opened yet.
    sent: Files,
    /// Saves the parent stored since the last frame: `(saveId, name)`.
    stored: Rc<RefCell<Vec<(u64, String)>>>,
    /// Workbook saves handed over and not yet stored or discarded (`?ack=1`).
    unacked: Rc<RefCell<PendingSaves<Arc<Workbook>>>>,
    next_save_id: Rc<Cell<u64>>,
    /// The parent asked for a save (`gridcraft:request-save`) since the last frame: `Some(true)`
    /// when any of those asks was a Save As.
    save_requested: Rc<Cell<Option<bool>>>,
    /// The unsaved-work state last reported to the parent.
    reported: Rc<Cell<Option<bool>>>,
}

impl Bridge {
    /// `None` when the page isn't framed (there is no parent to talk to).
    pub fn new(ack: bool) -> Option<Self> {
        let window = web_sys::window()?;
        let parent = window.parent().ok().flatten()?;
        if js_sys::Object::is(&parent, &window) {
            return None;
        }
        let origin = window.location().origin().ok()?;
        Some(Self {
            window,
            parent,
            origin,
            ack,
            sent: Rc::default(),
            stored: Rc::default(),
            unacked: Rc::default(),
            next_save_id: Rc::new(Cell::new(1)),
            save_requested: Rc::default(),
            reported: Rc::default(),
        })
    }

    /// Send saved and exported files to the parent instead of downloading them, and hear files
    /// from it.
    pub fn connect(&self, services: &mut Services, ctx: egui::Context) {
        self.listen(ctx);
        let bridge = self.clone();
        services.host_save = Some(Box::new(move |save: &HostSave<'_>| bridge.post_save(save)));
        services.host_confirms_saves = self.ack;
    }

    /// `?author=`: the host's name for the user signs new notes and comments.
    pub fn set_author(app: &mut SheetApp, author: &str) {
        let author: String = author.trim().chars().take(MAX_AUTHOR_CHARS).collect();
        if !author.is_empty() {
            app.session.prefs.user_name = author;
        }
    }

    pub fn post_ready(&self) {
        self.send(&[("type", "gridcraft:ready".into()), ("version", env!("CARGO_PKG_VERSION").into())]);
    }

    pub fn post_failed(&self, error: &str) {
        self.send(&[("type", "gridcraft:failed".into()), ("error", error.into())]);
    }

    /// After each frame: a save the parent asked for runs, stored saves mark their workbook saved,
    /// files the parent sent open and the parent hears which workbook each became, and the parent
    /// hears when unsaved work appears or goes away.
    pub fn sync(&self, app: &mut SheetApp, ctx: &egui::Context) {
        if let Some(save_as) = self.save_requested.take()
            && let Some(active) = app.session.active().map(|d| d.uid)
        {
            // A Save of a workbook whose Save As still waits for the host (the user hasn't chosen
            // where yet) is that Save As again with the newer content: a plain Save would make a
            // .csv/.tsv that replaces the waiting whole-workbook file.
            let save_as = save_as || self.unacked.borrow().save_as_waiting(active);
            app.run_or_alert(if save_as { "file.saveAs" } else { "file.save" }, serde_json::json!({}));
        }
        self.apply_stored(app);
        let sent = std::mem::take(&mut *self.sent.borrow_mut());
        for (name, bytes) in sent {
            let doc = app.open_sent(&name, &bytes).filter(|f: &SentFile| f.in_place).map(|f| f.doc);
            self.send(&[("type", "gridcraft:opened".into()), ("name", name.as_str().into()), ("docId", id_value(doc))]);
            ctx.request_repaint();
        }
        let dirty = app.has_unsaved_work();
        if self.reported.get() != Some(dirty) && self.send(&[("type", "gridcraft:dirty".into()), ("dirty", dirty.into())]) {
            self.reported.set(Some(dirty));
        }
    }

    /// [`Services::host_save`]: posts the file with the workbook it came from; with `?ack=1` a
    /// workbook save waits for `gridcraft:saved` with that content.
    fn post_save(&self, save: &HostSave<'_>) -> Result<(), String> {
        let name = std::path::Path::new(save.name).file_name().map_or_else(|| save.name.to_string(), |n| n.to_string_lossy().to_string());
        let save_id = self.next_save_id.get();
        self.next_save_id.set(save_id + 1);
        let buffer = js_sys::Uint8Array::from(save.bytes).buffer();
        let message = message(&[
            ("type", "gridcraft:save".into()),
            ("name", name.as_str().into()),
            ("bytes", buffer.clone().into()),
            ("docId", id_value(Some(save.document))),
            ("saveId", id_value(Some(save_id))),
            ("saveAs", save.save_as.into()),
            ("copy", (!save.marks_saved).into()),
        ])
        .ok_or_else(|| "couldn't build the message".to_string())?;
        self.parent
            .post_message_with_transfer(&message, &self.origin, &js_sys::Array::of1(&buffer))
            .map_err(|e| e.as_string().unwrap_or_else(|| format!("{e:?}")))?;
        if self.ack && save.marks_saved {
            self.unacked.borrow_mut().push(save_id, save.document, save.save_as, Arc::clone(&save.content));
        }
        Ok(())
    }

    /// A stored save marks its workbook saved as it was then (later edits stay unsaved) and
    /// names an untitled workbook after the file. Older saves of that workbook won't be stored
    /// after a newer one, and saves of closed workbooks are forgotten.
    fn apply_stored(&self, app: &mut SheetApp) {
        let stored = std::mem::take(&mut *self.stored.borrow_mut());
        let mut unacked = self.unacked.borrow_mut();
        for (save_id, name) in stored {
            let Some((doc, content)) = unacked.stored(save_id) else { continue };
            let Some(doc) = app.session.document_mut(doc) else { continue };
            doc.saved = content;
            if doc.path.is_none() {
                doc.title = name;
            }
        }
        let open: Vec<u64> = app.session.documents().iter().map(|d| d.uid).collect();
        unacked.keep_open(&open);
    }

    fn listen(&self, ctx: egui::Context) {
        let parent = self.parent.clone();
        let origin = self.origin.clone();
        let save_requested = Rc::clone(&self.save_requested);
        let sent = Rc::clone(&self.sent);
        let stored = Rc::clone(&self.stored);
        let unacked = Rc::clone(&self.unacked);
        let on_message = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |event: web_sys::MessageEvent| {
            if event.origin() != origin || !event.source().is_some_and(|s| js_sys::Object::is(&s, &parent)) {
                return;
            }
            let data = event.data();
            match string(&data, "type").as_deref() {
                Some("gridcraft:open") => {
                    let name = string(&data, "name").filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "Book.xlsx".to_string());
                    let Some(bytes) = bytes(&data) else { return };
                    sent.borrow_mut().push((name, bytes));
                }
                Some("gridcraft:saved") => {
                    let Some(save_id) = number(&data, "saveId") else { return };
                    let Some(name) = string(&data, "name").filter(|n| !n.trim().is_empty()) else { return };
                    stored.borrow_mut().push((save_id, name));
                }
                Some("gridcraft:discard") => {
                    let Some(save_id) = number(&data, "saveId") else { return };
                    unacked.borrow_mut().discard(save_id);
                    return;
                }
                Some("gridcraft:request-save") => {
                    let save_as = js_sys::Reflect::get(&data, &JsValue::from_str("saveAs")).is_ok_and(|v| v.as_bool() == Some(true));
                    save_requested.set(Some(save_as || save_requested.get() == Some(true)));
                }
                _ => return,
            }
            ctx.request_repaint();
        });
        if self.window.add_event_listener_with_callback("message", on_message.as_ref().unchecked_ref()).is_ok() {
            // The listener lives as long as the page.
            on_message.forget();
        }
    }

    /// Post a message to the parent; false if it couldn't be sent.
    fn send(&self, fields: &[(&str, JsValue)]) -> bool {
        message(fields).is_some_and(|m| self.parent.post_message(&m, &self.origin).is_ok())
    }
}

/// A workbook id or save number as a JS number (ids stay far below 2^53), `null` for none.
fn id_value(id: Option<u64>) -> JsValue {
    id.map_or(JsValue::NULL, |id| JsValue::from_f64(id as f64))
}

fn message(fields: &[(&str, JsValue)]) -> Option<js_sys::Object> {
    let m = js_sys::Object::new();
    for (key, value) in fields {
        js_sys::Reflect::set(&m, &JsValue::from_str(key), value).ok()?;
    }
    Some(m)
}

fn string(data: &JsValue, key: &str) -> Option<String> {
    js_sys::Reflect::get(data, &JsValue::from_str(key)).ok()?.as_string()
}

/// A whole, non-negative number field.
fn number(data: &JsValue, key: &str) -> Option<u64> {
    let n = js_sys::Reflect::get(data, &JsValue::from_str(key)).ok()?.as_f64()?;
    (n >= 0.0 && n.fract() == 0.0 && n < 9_007_199_254_740_992.0).then_some(n as u64)
}

/// `bytes` as an `ArrayBuffer` or `Uint8Array`.
fn bytes(data: &JsValue) -> Option<Vec<u8>> {
    let bytes = js_sys::Reflect::get(data, &JsValue::from_str("bytes")).ok()?;
    if let Some(array) = bytes.dyn_ref::<js_sys::Uint8Array>() {
        Some(array.to_vec())
    } else if bytes.is_instance_of::<js_sys::ArrayBuffer>() {
        Some(js_sys::Uint8Array::new(&bytes).to_vec())
    } else {
        None
    }
}
