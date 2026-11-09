//! The interface state each window keeps for itself.
//!
//! [`SheetApp`]'s per-window fields (the cell draft, dialog, Backstage view, grid gestures…) hold
//! the state of one window: the engine's active window. Every other window's state is parked in a
//! [`WindowUi`] and swapped in when that window is activated or drawn, the way the engine swaps a
//! window's selection and sheet into its workbook. So a draft, a dialog or a message stays in the
//! window it belongs to.

use gridcraft_engine::core::CellRef;

use crate::{Notice, SheetApp, backstage, dialogs, editor, grid, keytips, text_box};

/// One window's parked interface state (see the module docs).
#[derive(Default)]
pub struct WindowUi {
    editor: Option<editor::EditState>,
    text_box_editor: Option<text_box::EditState>,
    dialog: Option<dialogs::Dialog>,
    backstage: Option<backstage::Backstage>,
    message: Option<(String, String)>,
    toast: Option<(String, f64)>,
    notice: Option<Notice>,
    grid: grid::GridState,
    name_box: Option<String>,
    keytips: keytips::KeyTips,
    selected_chart: Option<u32>,
    tab_run: Option<(u64, usize, u32, CellRef)>,
    open_switch_windows: bool,
    /// The ribbon tab this window shows. A contextual tab depends on this window's selection, so
    /// another window's frame must not reset it.
    ribbon_tab: String,
    /// The status bar's Ready/Enter/Edit, which follows the window's cell draft.
    mode: gridcraft_engine::Mode,
}

impl WindowUi {
    /// A new window's state. The last border, fill and font colour picked carry over, as they are
    /// the user's recent choices rather than the window's; so does the ribbon tab.
    fn fresh(like: &grid::GridState, ribbon_tab: &str) -> WindowUi {
        WindowUi {
            ribbon_tab: ribbon_tab.to_string(),
            grid: grid::GridState {
                last_border: like.last_border.clone(),
                last_fill: like.last_fill.clone(),
                last_font_color: like.last_font_color.clone(),
                ..Default::default()
            },
            ..Default::default()
        }
    }
}

impl SheetApp {
    /// Exchanges the per-window fields with `w`. Destructured in full, so a field added to
    /// [`WindowUi`] can't be left out of the swap.
    fn swap_window_ui(&mut self, w: &mut WindowUi) {
        let WindowUi {
            editor,
            text_box_editor,
            dialog,
            backstage,
            message,
            toast,
            notice,
            grid,
            name_box,
            keytips,
            selected_chart,
            tab_run,
            open_switch_windows,
            ribbon_tab,
            mode,
        } = w;
        std::mem::swap(&mut self.editor, editor);
        std::mem::swap(&mut self.text_box_editor, text_box_editor);
        std::mem::swap(&mut self.dialog, dialog);
        std::mem::swap(&mut self.backstage, backstage);
        std::mem::swap(&mut self.message, message);
        std::mem::swap(&mut self.toast, toast);
        std::mem::swap(&mut self.notice, notice);
        std::mem::swap(&mut self.grid, grid);
        std::mem::swap(&mut self.name_box, name_box);
        std::mem::swap(&mut self.keytips, keytips);
        std::mem::swap(&mut self.selected_chart, selected_chart);
        std::mem::swap(&mut self.tab_run, tab_run);
        std::mem::swap(&mut self.open_switch_windows, open_switch_windows);
        std::mem::swap(&mut self.ui.ribbon_tab, ribbon_tab);
        std::mem::swap(&mut self.session.mode, mode);
    }

    /// The window whose interface state is in the per-window fields.
    pub fn ui_window(&self) -> Option<u64> {
        self.ui_window
    }

    /// Brings the per-window fields in line with the engine's active window, after a command (or
    /// a script) switched windows: the previous window's state is parked, the active one's swapped
    /// in (a window never seen before starts fresh). A closed window's state is dropped.
    pub fn follow_active_window(&mut self) {
        let active = self.session.active_window();
        if active == self.ui_window {
            return;
        }
        let Some(previous) = self.ui_window else {
            // The first window: the fields are already its state.
            self.ui_window = active;
            return;
        };
        let mut outgoing = WindowUi::fresh(&self.grid, &self.ui.ribbon_tab);
        self.swap_window_ui(&mut outgoing);
        if self.session.window(previous).is_some() {
            self.parked.insert(previous, outgoing);
        }
        if let Some(mut incoming) = active.and_then(|w| self.parked.remove(&w)) {
            self.swap_window_ui(&mut incoming);
        }
        self.ui_window = active;
        let session = &self.session;
        self.parked.retain(|w, _| session.window(*w).is_some());
    }

    /// Activates window `id` (it becomes the most recently active) with its interface state.
    pub fn enter_window(&mut self, id: u64) {
        self.session.activate_window(id);
        self.follow_active_window();
    }

    /// Makes window `id` and its interface state current while it is drawn, without changing
    /// which window counts as most recently active.
    pub fn show_window(&mut self, id: u64) {
        self.session.show_window(id);
        self.follow_active_window();
    }

    /// Puts back a dialog that was taken out of window `home`'s fields while it ran. A command
    /// run from the dialog may have switched windows meanwhile; the dialog stays in the window it
    /// was opened in (dropped if that window closed).
    pub(crate) fn return_dialog(&mut self, home: Option<u64>, d: dialogs::Dialog) {
        if home == self.ui_window {
            self.dialog = Some(d);
        } else if let Some(w) = home.and_then(|h| self.parked.get_mut(&h))
            && w.dialog.is_none()
        {
            w.dialog = Some(d);
        }
    }

    /// Whether a window other than the current one has a cell or text-box draft open.
    pub fn parked_window_with_draft(&self) -> Option<u64> {
        self.parked.iter().filter(|(_, w)| w.editor.is_some() || w.text_box_editor.is_some()).map(|(id, _)| *id).min()
    }

    /// Unsaved work the user would lose if the app went away now (a browser tab closing): a
    /// workbook with unsaved changes, or a draft not yet entered in any window. Nothing is
    /// committed to find out.
    pub fn has_unsaved_work(&self) -> bool {
        self.session.documents().iter().any(|d| d.is_dirty()) || self.has_unsaved_drafts()
    }

    /// A draft in any window that entering would change something with: a cell or formula-bar
    /// edit with text, a text box or header/footer edit that changed, unsaved macro steps, or a
    /// comment reply typed and not sent (the app keeps it, so it counts whether or not the
    /// Comments pane is drawn, as long as the comment it answers is there).
    pub fn has_unsaved_drafts(&self) -> bool {
        let drafts = |editor: &Option<editor::EditState>, text_box: &Option<text_box::EditState>, grid: &grid::GridState| {
            editor.as_ref().is_some_and(editor::EditState::changes_cell)
                || text_box.as_ref().is_some_and(text_box::EditState::changed)
                || grid.hf_edit.as_ref().is_some_and(crate::paper_view::HfEdit::changed)
        };
        drafts(&self.editor, &self.text_box_editor, &self.grid)
            || self.parked.values().any(|w| drafts(&w.editor, &w.text_box_editor, &w.grid))
            || self.macros.editor.as_ref().is_some_and(|ed| ed.dirty)
            || self.reply_drafts.pending(&self.session)
    }
}
