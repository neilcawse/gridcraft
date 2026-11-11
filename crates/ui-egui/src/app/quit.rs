use super::{Json, SaveAnswer, SheetApp, json, now_ms};

impl SheetApp {
    /// Holds back a window close (the close button, Alt+F4, the OS quitting the app) while a
    /// workbook has unsaved changes, and asks about them first.
    pub(super) fn guard_window_close(&mut self, ctx: &egui::Context) {
        if self.native_windows || self.allow_close || !ctx.input(|i| i.viewport().close_requested()) {
            return;
        }
        // A cell, header or text box still being edited is part of the workbook, as for File › Close.
        if self.settle_drafts().is_err() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            return;
        }
        // The macro step editor asks about its own unsaved steps and debug run first; once it has
        // closed, the window close goes on.
        if self.dialog.as_ref().is_some_and(|d| d.name == "macroEditor") {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            crate::macro_editor::request_close(self, true);
            return;
        }
        if !self.session.documents().iter().any(|d| self.document_has_unsaved_work(d.uid)) {
            return;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        match self.dialog.as_mut() {
            // Already asking about a workbook (File › Close): answering it now goes on to quit.
            Some(d) if d.name() == "saveChanges" => {
                d.values.insert("quit".into(), json!(true));
            }
            _ => self.continue_quit(ctx),
        }
    }

    /// Asks about the next workbook with unsaved work (changes, or a reply typed and not sent)
    /// before quitting, in its window, or closes the app once none is left.
    pub(crate) fn continue_quit(&mut self, ctx: &egui::Context) {
        let unsaved = self.session.documents().iter().map(|d| d.uid).find(|uid| self.document_has_unsaved_work(*uid));
        match unsaved.and_then(|uid| self.session.doc_window(uid)) {
            Some(window) => {
                // Its window comes forward (the native window too: see `doc_windows`).
                self.enter_window(window);
                let title = self.session.active().map(|d| d.display_title()).unwrap_or_default();
                self.open_dialog("saveChanges", json!({"title": title, "quit": true}));
                self.bring_forward = true;
            }
            None => {
                self.allow_close = true;
                ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close);
            }
        }
    }

    /// Asks the Save Changes question for a workbook that is about to close and has a reply typed
    /// under a comment and not sent, but no unsaved changes: the engine asks only about changes
    /// (`file.close`, `window.close`), and it can't see replies, which the app keeps. Returns what
    /// the engine would for a workbook that asks, or `None` when the command is not one that
    /// closes the workbook, or this workbook closes without a question (`force`, another window
    /// still shows it, nothing to lose, or unsaved changes the engine asks about itself).
    pub(super) fn ask_before_closing(&mut self, id: &str, p: &Json) -> Option<Json> {
        if p.get("force").and_then(Json::as_bool).unwrap_or(false) {
            return None;
        }
        let window = if id == "window.close" {
            self.session.target_window(gridcraft_engine::Target::from_json(p).ok()?).ok()?
        } else {
            self.session.active_window()?
        };
        let doc = self.session.window(window)?.doc_uid;
        // A window closes alone while another one shows the workbook, unless it is `file.close` of the whole workbook.
        let whole = id == "file.close" && p.get("document").and_then(Json::as_bool).unwrap_or(false);
        let alone = self.session.windows().iter().all(|w| w.doc_uid != doc || w.id == window);
        if !(alone || whole)
            || !self.reply_drafts.pending_in(&self.session, doc)
            || self.session.documents().iter().any(|d| d.uid == doc && d.is_dirty())
        {
            return None;
        }
        // The question is about the active workbook, in its window.
        self.enter_window(window);
        let title = self.session.active().map(|d| d.display_title()).unwrap_or_default();
        self.open_dialog("saveChanges", json!({"title": title}));
        Some(json!({"closed": false, "dirty": true}))
    }

    /// Answers the Save Changes question about the active workbook (its buttons). While quitting,
    /// the next workbook with unsaved work is asked about once this one is answered; Cancel stops
    /// the quit. Returns whether the question is settled: Save keeps it open when the cell being
    /// edited can't be entered.
    ///
    /// A reply typed under a comment is not in the file, and no Save can put it there (only
    /// sending it does): Don't Save throws it away with the workbook, Cancel keeps it, and Save
    /// saves the changes but leaves the workbook open, saying why, while a reply waits.
    pub fn answer_save_changes(&mut self, answer: SaveAnswer, quitting: bool, ctx: &egui::Context) -> bool {
        match answer {
            SaveAnswer::Cancel => {}
            SaveAnswer::DontSave => {
                if let Some(uid) = self.session.active().map(|d| d.uid) {
                    self.reply_drafts.discard(uid);
                }
                self.run_or_alert("file.close", json!({"force": true, "document": true}));
                if quitting {
                    self.continue_quit(ctx);
                }
            }
            SaveAnswer::Save => {
                // File operations include everything still being typed (the cell, a header, a sheet name…), as File › Save does.
                if self.settle_drafts().is_err() {
                    return false;
                }
                let sent = self.host_saves_sent;
                let (uid, title) = self.session.active().map(|d| (d.uid, d.display_title())).unzip();
                self.run_or_alert("file.save", json!({}));
                if self.session.active().is_some_and(|x| !x.is_dirty()) {
                    if uid.is_some_and(|uid| self.reply_drafts.pending_in(&self.session, uid)) {
                        self.message = Some((
                            "GridCraft".into(),
                            format!(
                                "Saved. The reply you typed in {} isn't sent, and saving doesn't send it, so the workbook stays open: send the reply, or close the workbook with Don't Save to discard it.",
                                title.unwrap_or_default()
                            ),
                        ));
                    } else {
                        self.run_or_alert("file.close", json!({"force": true, "document": true}));
                        if quitting {
                            self.continue_quit(ctx);
                        }
                    }
                } else if self.host_saves_sent != sent {
                    // A host page that confirms its saves hasn't stored the file yet: the workbook
                    // stays open, unsaved, until it has; Close again then closes it.
                    self.toast = Some(("Waiting for the save to finish…".into(), now_ms()));
                }
            }
        }
        true
    }

    /// Quit (the macOS app menu, ⌘Q): commits every window's cell draft as Enter would, then asks
    /// about each unsaved workbook in its window; Cancel on any of them stops the quit. A draft its
    /// workbook refuses stops the quit too, with its window in front and the reason shown.
    pub fn request_quit(&mut self, ctx: &egui::Context) {
        while let Some(window) = self.parked_window_with_draft() {
            self.enter_window(window);
            if self.settle_drafts().is_err() {
                return;
            }
        }
        if self.settle_drafts().is_err() {
            return;
        }
        if let Some(d) = self.dialog.as_mut().filter(|d| d.name() == "saveChanges") {
            // Already asking about a workbook (File › Close): answering it goes on to quit.
            d.values.insert("quit".into(), json!(true));
            return;
        }
        self.continue_quit(ctx);
    }
}
