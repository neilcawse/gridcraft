//! The `postMessage` bridge to a same-origin parent page (`?host=parent`), for pages that embed
//! GridCraft in an `<iframe>` (such as a Nextcloud app). Messages are plain objects with a `type`:
//!
//! - `gridcraft:ready` (to the parent): `{ version }`, the app is listening; send a file now.
//! - `gridcraft:open` (from the parent): `{ name, bytes }`, bytes an `ArrayBuffer` or
//!   `Uint8Array`. It opens like a workbook picked with File › Open or dropped on the window.
//! - `gridcraft:opened` (to the parent): `{ name, docId }` once a sent file has opened: `docId`
//!   is the workbook it became, or `null` when it didn't open (or waits behind a password
//!   prompt, after which it counts as a workbook the user opened).
//! - `gridcraft:save` (to the parent): `{ name, bytes, docId, saveId }` (`bytes` a transferred
//!   `ArrayBuffer`) for every file GridCraft would otherwise download: Save and Save As (an .xlsx
//!   workbook) and Export as PDF. `docId` is the workbook it came from (`null` when none is
//!   open), the same number `gridcraft:opened` gave for a sent file, so the parent writes it
//!   back only to that file. `saveId` numbers the saves.
//! - `gridcraft:saved` (from the parent, with `?ack=1`): `{ saveId, name }`, the save is stored
//!   as `name`. Only then does the workbook count as saved, as it was when it was saved. Without
//!   `?ack=1` a workbook counts as saved once it is handed over.
//! - `gridcraft:dirty` (to the parent): `{ dirty }` whenever unsaved changes appear or go away.
//! - `gridcraft:request-save` (from the parent): save the active workbook as File › Save would,
//!   so a parent's own Save button or shortcut gets a `gridcraft:save` back.
//! - `gridcraft:failed` (to the parent): `{ error }` when the app couldn't start.
//!
//! Only the parent window on the page's own origin is heard or answered. In host mode the page
//! starts with an empty workbook instead of a sample, and the parent guards leaving the page.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Arc;

use gridcraft_engine::model::Workbook;
use gridcraft_ui_egui::{Inbox, Services, SheetApp};
use wasm_bindgen::JsCast as _;
use wasm_bindgen::JsValue;
use wasm_bindgen::closure::Closure;

/// Saving one of these hands the whole workbook to the parent, which stores it: the workbook
/// counts as saved under that name, as it would after Save on the desktop.
const WORKBOOK_EXTS: &[&str] = &["xlsx"];

/// The longest user name taken from `?author=`.
const MAX_AUTHOR_CHARS: usize = 200;

/// Files with their bytes, in the order they came.
type Files = Rc<RefCell<VecDeque<(String, Vec<u8>)>>>;

/// A sent file given to the app, and the workbooks open before it.
type Opening = (String, Vec<u64>);

/// A workbook handed to the parent and not yet acknowledged: the workbook it came from and its
/// content then.
struct Unacked {
    save_id: u64,
    doc: u64,
    content: Arc<Workbook>,
}

#[derive(Clone)]
pub struct Bridge {
    window: web_sys::Window,
    parent: web_sys::Window,
    origin: String,
    /// `?ack=1`: a workbook counts as saved when the parent says it stored it (`gridcraft:saved`),
    /// not when it is handed over.
    ack: bool,
    /// Files GridCraft saved since the last frame, posted after it with the workbook they came
    /// from.
    saves: Files,
    /// Files the parent sent that the app hasn't been given yet.
    sent: Files,
    /// The sent file the app was given last frame, and the workbooks open before it.
    opening: Rc<RefCell<Option<Opening>>>,
    /// Saves the parent stored since the last frame: `(saveId, name)`.
    stored: Rc<RefCell<Vec<(u64, String)>>>,
    unacked: Rc<RefCell<Vec<Unacked>>>,
    next_save_id: Rc<Cell<u64>>,
    /// The parent asked for a save (`gridcraft:request-save`) since the last frame.
    save_requested: Rc<Cell<bool>>,
    /// The unsaved-changes state last reported to the parent.
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
            saves: Rc::default(),
            sent: Rc::default(),
            opening: Rc::default(),
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
        self.listen(ctx.clone());
        let saves = Rc::clone(&self.saves);
        services.download = Some(Box::new(move |name: &str, bytes: &[u8]| {
            saves.borrow_mut().push_back((name.to_string(), bytes.to_vec()));
            ctx.request_repaint();
        }));
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

    /// After each frame: a save the parent asked for runs, saves go to the parent with the
    /// workbook they came from, stored saves mark their workbook saved, the parent hears which
    /// workbook the file it sent became, the next sent file goes to the app, and the parent hears
    /// when unsaved changes appear or go away.
    pub fn sync(&self, app: &mut SheetApp, inbox: &Inbox, ctx: &egui::Context) {
        if self.save_requested.take() && app.session.active().is_some() {
            app.run_or_alert("file.save", serde_json::json!({}));
        }
        self.post_saves(app);
        self.apply_stored(app);
        if let Some((name, before)) = self.opening.take() {
            let docs = app.session.documents();
            let doc = crate::doc_ids::opened_doc(&before, docs.iter().map(|d| (d.uid, d.title.as_str())), &name);
            self.send(&[("type", "gridcraft:opened".into()), ("name", name.as_str().into()), ("docId", id_value(doc))]);
        }
        if let Some((name, bytes)) = self.sent.borrow_mut().pop_front() {
            let before = app.session.documents().iter().map(|d| d.uid).collect();
            inbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push((name.clone(), bytes));
            *self.opening.borrow_mut() = Some((name, before));
            ctx.request_repaint();
        }
        let dirty = app.session.documents().iter().any(|d| d.is_dirty());
        if self.reported.get() != Some(dirty) && self.send(&[("type", "gridcraft:dirty".into()), ("dirty", dirty.into())]) {
            self.reported.set(Some(dirty));
        }
    }

    fn post_saves(&self, app: &mut SheetApp) {
        let saves = std::mem::take(&mut *self.saves.borrow_mut());
        for (path, bytes) in saves {
            let name = std::path::Path::new(&path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or(path);
            let save_id = self.next_save_id.get();
            self.next_save_id.set(save_id + 1);
            let doc = app.session.active().map(|d| d.uid);
            if let Err(e) = self.post_save(&name, &bytes, doc, save_id) {
                log::error!("couldn't hand {name} to the page: {e}");
                continue;
            }
            if !is_workbook(&name) {
                continue;
            }
            let Some(active) = app.session.active_mut() else { continue };
            if self.ack {
                self.unacked.borrow_mut().push(Unacked { save_id, doc: active.uid, content: Arc::clone(&active.wb) });
            } else {
                active.saved = Arc::clone(&active.wb);
                if active.path.is_none() {
                    active.title = name;
                }
            }
        }
    }

    /// A stored save marks its workbook saved as it was then (later edits stay unsaved) and
    /// names an untitled workbook after the file. Older saves of that workbook won't be stored
    /// after a newer one, and saves of closed workbooks are forgotten.
    fn apply_stored(&self, app: &mut SheetApp) {
        let stored = std::mem::take(&mut *self.stored.borrow_mut());
        let mut unacked = self.unacked.borrow_mut();
        for (save_id, name) in stored {
            let Some(at) = unacked.iter().position(|u| u.save_id == save_id) else { continue };
            let save = unacked.remove(at);
            unacked.retain(|u| u.doc != save.doc || u.save_id > save_id);
            let Some(doc) = app.session.document_mut(save.doc) else { continue };
            doc.saved = save.content;
            if doc.path.is_none() {
                doc.title = name;
            }
        }
        let open: Vec<u64> = app.session.documents().iter().map(|d| d.uid).collect();
        unacked.retain(|u| open.contains(&u.doc));
    }

    fn post_save(&self, name: &str, bytes: &[u8], doc: Option<u64>, save_id: u64) -> Result<(), String> {
        let buffer = js_sys::Uint8Array::from(bytes).buffer();
        let message = message(&[
            ("type", "gridcraft:save".into()),
            ("name", name.into()),
            ("bytes", buffer.clone().into()),
            ("docId", id_value(doc)),
            ("saveId", id_value(Some(save_id))),
        ])
        .ok_or_else(|| "couldn't build the message".to_string())?;
        self.parent
            .post_message_with_transfer(&message, &self.origin, &js_sys::Array::of1(&buffer))
            .map_err(|e| e.as_string().unwrap_or_else(|| format!("{e:?}")))
    }

    fn listen(&self, ctx: egui::Context) {
        let parent = self.parent.clone();
        let origin = self.origin.clone();
        let save_requested = Rc::clone(&self.save_requested);
        let sent = Rc::clone(&self.sent);
        let stored = Rc::clone(&self.stored);
        let on_message = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |event: web_sys::MessageEvent| {
            if event.origin() != origin || !event.source().is_some_and(|s| js_sys::Object::is(&s, &parent)) {
                return;
            }
            let data = event.data();
            match string(&data, "type").as_deref() {
                Some("gridcraft:open") => {
                    let name = string(&data, "name").filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "Book.xlsx".to_string());
                    let Some(bytes) = bytes(&data) else { return };
                    sent.borrow_mut().push_back((name, bytes));
                }
                Some("gridcraft:saved") => {
                    let Some(save_id) = number(&data, "saveId") else { return };
                    let Some(name) = string(&data, "name").filter(|n| !n.trim().is_empty()) else { return };
                    stored.borrow_mut().push((save_id, name));
                }
                Some("gridcraft:request-save") => save_requested.set(true),
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

fn is_workbook(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(_, ext)| WORKBOOK_EXTS.iter().any(|e| ext.eq_ignore_ascii_case(e)))
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
