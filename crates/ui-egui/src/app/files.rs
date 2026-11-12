use super::{Json, Notice, NoticeKind, OpenWindow, SheetApp, clean_error, json};
use crate::SentFile;

impl SheetApp {
    pub fn open_path(&mut self, path: &str) {
        match self.run("file.open", json!({"path": path})) {
            Ok(r) => self.after_open(Some(path), &r),
            // A password-protected workbook: ask for the password; the dialog opens it.
            Err(e) if e.contains(gridcraft_engine::io::PASSWORD_REQUIRED) => self.open_dialog("openPassword", json!({"path": path})),
            Err(e) => self.message = Some(("GridCraft".into(), clean_error(&e))),
        }
        self.after_engine();
    }

    /// What follows a successful `file.open`: the workbook shows (not the Backstage), the Recent
    /// list (for a file on disk) and the open warnings notice. Shared by File › Open, the
    /// password dialog and the control channel.
    pub fn after_open(&mut self, path: Option<&str>, r: &Json) {
        self.backstage = None;
        if let Some(path) = path {
            self.ui.recent.retain(|p| p != path);
            self.ui.recent.insert(0, path.to_string());
            self.ui.recent.truncate(20);
        }
        self.show_open_warnings(r);
    }

    /// Turns the `warnings` a `file.open` result carries into an info-bar notice, choosing the
    /// most important one (a preserved VBA project outranks, say, a skipped optional part).
    /// Kept deliberately generic: any open-time warning can reach the user this way.
    pub fn show_open_warnings(&mut self, r: &Json) {
        // The opened workbook's window shows them, however it was opened (a script may have run
        // `file.open` on the session directly).
        self.follow_active_window();
        let Some(list) = r.get("warnings").and_then(Json::as_array) else {
            self.notice = None;
            return;
        };
        let msgs: Vec<&str> = list.iter().filter_map(Json::as_str).collect();
        if msgs.is_empty() {
            // A clean open clears any notice left over from the previous document.
            self.notice = None;
            return;
        }
        let vba: Vec<&str> = msgs.iter().copied().filter(|m| m.to_lowercase().contains("macro") || m.contains("VBA")).collect();
        if !vba.is_empty() {
            // The reader says "dropped" when the VBA project was missing or unreadable; never promise to keep it then.
            let dropped = vba.iter().any(|m| m.contains("dropped"));
            let text = if dropped {
                "This workbook's VBA macros could not be loaded and were dropped, so they will not be in the saved file. Keep the original file."
            } else {
                "This workbook contains VBA macros. GridCraft keeps them when you save as .xlsm, but does not run them."
            };
            self.notice = Some(Notice { text: text.into(), kind: NoticeKind::Warning, action: Some(("What this means".into(), "macros".into())) });
        } else {
            let text = if msgs.len() == 1 {
                msgs[0].to_string()
            } else {
                format!("{} items in this file were handled differently. First: {}", msgs.len(), msgs[0])
            };
            self.notice = Some(Notice { text, kind: NoticeKind::Info, action: None });
        }
    }

    /// The open windows in order, for the Switch Windows lists.
    pub fn open_windows(&self) -> Vec<OpenWindow> {
        let active = self.session.active_window();
        self.session
            .windows()
            .iter()
            .filter_map(|w| {
                let d = self.session.documents().iter().find(|d| d.uid == w.doc_uid)?;
                let title = self.session.window_title(w.id)?;
                Some(OpenWindow { window: w.id, doc: d.uid, title, dirty: d.is_dirty(), active: active == Some(w.id) })
            })
            .collect()
    }

    /// Opens files that arrived asynchronously (web file picker, dropped files).
    pub(super) fn poll_async_opens(&mut self) {
        let arrived: Vec<(String, Vec<u8>)> = self
            .services
            .inbox
            .as_ref()
            .map(|i| std::mem::take(&mut *i.lock().unwrap_or_else(std::sync::PoisonError::into_inner)))
            .unwrap_or_default();
        for (name, bytes) in arrived {
            self.open_bytes(&name, &bytes);
        }
    }

    /// Web: opens a file the host page sent (`?host=parent`), as a dropped file opens. Returns
    /// the workbook it became, or `None` when it didn't open (refused, or waiting behind the
    /// password prompt, after which it counts as a workbook the user opened).
    pub fn open_sent(&mut self, name: &str, bytes: &[u8]) -> Option<SentFile> {
        let r = self.open_bytes(name, bytes)?;
        let doc = r.get("doc").and_then(Json::as_u64)?;
        Some(SentFile { doc, in_place: r.get("inPlace").and_then(Json::as_bool) == Some(true) })
    }

    /// Opens `bytes` named `name` as File › Open would; the `file.open` result once it opened.
    fn open_bytes(&mut self, name: &str, bytes: &[u8]) -> Option<Json> {
        let b64 = gridcraft_engine::io::base64_encode(bytes);
        let opened = match self.run("file.open", json!({"name": name, "base64": &b64})) {
            // A file picked from File › Open (web) arrives here: show it, not the Backstage.
            Ok(r) => {
                self.backstage = None;
                self.show_open_warnings(&r);
                Some(r)
            }
            Err(e) if e.contains(gridcraft_engine::io::PASSWORD_REQUIRED) => {
                self.backstage = None;
                self.open_dialog("openPassword", json!({"name": name, "base64": b64}));
                None
            }
            Err(e) => {
                self.message = Some(("GridCraft".into(), clean_error(&e)));
                None
            }
        };
        self.after_engine();
        opened
    }

    /// Web: hands a file the user saved or exported to them. A host page that keeps the files
    /// ([`crate::Services::host_save`]) gets it with the active workbook, which the bytes were
    /// just made from, and its content now; otherwise the browser downloads it. `save_as`: the
    /// user asked for a new name or place; `marks_saved`: the file is the whole workbook, so
    /// storing it saves the workbook (at once, unless the host confirms its saves). Returns
    /// whether the file was handed over; a host that couldn't take it is reported.
    pub(crate) fn deliver_file(&mut self, name: &str, bytes: &[u8], save_as: bool, marks_saved: bool) -> bool {
        let Some(host) = &self.services.host_save else {
            let Some(download) = &self.services.download else { return false };
            download(name, bytes);
            return true;
        };
        let Some(d) = self.session.active() else { return false };
        let (document, content) = (d.uid, std::sync::Arc::clone(&d.wb));
        let handed = host(&crate::HostSave { name, bytes, save_as, marks_saved, document, content: std::sync::Arc::clone(&content) });
        if let Err(e) = handed {
            self.alert(format!("Couldn't save {name}: {e}"));
            return false;
        }
        if marks_saved {
            if self.services.host_confirms_saves {
                self.host_saves_sent += 1;
            } else if let Some(d) = self.session.document_mut(document) {
                d.saved = content;
                if d.path.is_none() {
                    d.title = name.to_string();
                }
            }
        }
        true
    }

    /// Saving the workbook to its own .csv/.tsv file (`name`) kept only the active sheet's values:
    /// the info bar says so, once per workbook.
    pub(super) fn tell_csv_loss(&mut self, name: &str) {
        let Some(uid) = self.session.active().map(|d| d.uid) else { return };
        if self.csv_loss_told.insert(uid) {
            self.notice = Some(Notice {
                text: format!(
                    "Saved {name} as CSV: it keeps the values of the active sheet only. Other sheets, formulas and formatting are not in the file; use File › Save As for an .xlsx that keeps them."
                ),
                kind: NoticeKind::Warning,
                action: None,
            });
        }
    }

    /// AutoSave: save workbooks that have a file shortly after they change. This decision lives in
    /// `logic` so a hidden or logic-only frame still saves; the monotonic timestamp check keeps a
    /// forced hidden UI pass from saving the same change twice.
    pub(super) fn autosave(&mut self, ctx: &egui::Context) {
        let dirty_xlsx = self.session.active().is_some_and(|d| d.is_dirty() && d.path.as_deref().is_some_and(|p| p.ends_with(".xlsx")));
        let now = crate::monotonic_ms();
        let autosave_due =
            self.ui.autosave && self.editor.is_none() && self.text_box_editor.is_none() && now - self.grid.last_autosave > 2000.0 && dirty_xlsx;
        if autosave_due {
            self.grid.last_autosave = now;
            self.store_view_state();
            let _ = self.session.run("file.save", json!({}));
        } else if self.ui.autosave && dirty_xlsx {
            // Keep checking even when no input arrives while the window is hidden.
            ctx.request_repaint_after(std::time::Duration::from_secs(2));
        }
    }

    /// Flip AutoSave (title-bar switch / Options ▸ Save). Persisted with the other UI
    /// preferences, so it stays as the user left it across restarts (#1901).
    pub fn toggle_autosave(&mut self) {
        self.ui.autosave = !self.ui.autosave;
    }
}
