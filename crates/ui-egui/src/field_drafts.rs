//! Text typed into a field over a workbook value and not yet entered: the Table Design table name,
//! the Format Chart title, axis titles and size, the File › Info properties and the Options ›
//! Formulas iteration settings.
//!
//! Such a field enters its text when it loses focus (Enter or a click elsewhere), so what was typed
//! is unsaved work until then. The app keeps it, not egui's temporary data, in the window's
//! [`GridState`](crate::grid::GridState): it counts as unsaved work
//! ([`SheetApp::has_unsaved_work`](crate::SheetApp::has_unsaved_work)), and
//! [`SheetApp::settle_drafts`](crate::SheetApp::settle_drafts) enters it before Save, the File
//! commands, Close, Quit and a window switch, as it does a cell being typed.
//!
//! A draft is bound to what it is typed for, never to a place: its workbook by uid, and its table
//! by name, or its chart by id *and* the name of its sheet (a chart id is only unique within a
//! sheet). Entering never writes to another workbook, sheet or chart: a draft whose target is not
//! the one showing is said, with what was typed, not entered elsewhere. A value the workbook
//! refuses is reported, and Save does not go ahead with a wrong one.

use egui::{Id, Response, Ui};
use serde_json::{Value as Json, json};

use crate::{SheetApp, clean_error};

/// What a field is typed for.
#[derive(Clone, Debug, PartialEq)]
pub enum Field {
    /// A table, by its name.
    TableName(String),
    /// The chart `chart` of the sheet named `sheet`.
    ChartTitle {
        sheet: String,
        chart: u32,
    },
    ChartXTitle {
        sheet: String,
        chart: u32,
    },
    ChartYTitle {
        sheet: String,
        chart: u32,
    },
    ChartSize {
        sheet: String,
        chart: u32,
    },
    /// A document property (a `file.properties` key).
    Property(&'static str),
    MaxIterations,
    MaxChange,
}

impl Field {
    fn what(&self) -> &'static str {
        match self {
            Field::TableName(_) => "table name",
            Field::ChartTitle { .. } => "chart title",
            Field::ChartXTitle { .. } => "horizontal axis title",
            Field::ChartYTitle { .. } => "vertical axis title",
            Field::ChartSize { .. } => "chart size",
            Field::Property(_) => "document property",
            Field::MaxIterations => "Maximum Iterations",
            Field::MaxChange => "Maximum Change",
        }
    }

    /// The sheet name and chart id of a chart field.
    fn chart(&self) -> Option<(&str, u32)> {
        match self {
            Field::ChartTitle { sheet, chart }
            | Field::ChartXTitle { sheet, chart }
            | Field::ChartYTitle { sheet, chart }
            | Field::ChartSize { sheet, chart } => Some((sheet, *chart)),
            _ => None,
        }
    }
}

/// What was typed: text, or the chart's width and height.
#[derive(Clone, Debug, PartialEq)]
pub enum Typed {
    Text(String),
    Size(f32, f32),
}

impl std::fmt::Display for Typed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Typed::Text(t) => f.write_str(t),
            Typed::Size(w, h) => write!(f, "{w} × {h}"),
        }
    }
}

/// A value typed into a field and not yet entered.
#[derive(Clone, Debug)]
pub struct FieldDraft {
    /// The workbook (`DocState::uid`) it is typed in.
    doc: u64,
    field: Field,
    typed: Typed,
    /// What the workbook held when typing began.
    original: Typed,
}

impl FieldDraft {
    /// The user changed the value since typing began: unentered work.
    pub fn changed(&self) -> bool {
        self.typed != self.original
    }

    fn is(&self, doc: u64, field: &Field) -> bool {
        self.doc == doc && self.field == *field
    }

    /// Its workbook is one of the open ones `open`.
    pub(crate) fn is_in(&self, open: &[u64]) -> bool {
        open.contains(&self.doc)
    }
}

/// A one-line text field for a workbook value, which callers reload every frame. The text being
/// typed is kept as the window's draft until the field loses focus, then entered (Escape cancels
/// the edit). Save pressed in the field saves the file with what was typed.
pub(crate) fn text_field(app: &mut SheetApp, ui: &mut Ui, id: Id, field: Field, current: &str, width: Option<f32>) -> Response {
    let width = width.unwrap_or(ui.spacing().text_edit_width);
    let doc = app.session.active().map(|d| d.uid);
    let shown = doc.and_then(|doc| {
        app.grid.field_draft.as_ref().filter(|d| d.is(doc, &field)).and_then(|d| match &d.typed {
            Typed::Text(t) => Some(t.clone()),
            Typed::Size(..) => None,
        })
    });
    let mut text = shown.unwrap_or_else(|| current.to_string());
    let r = ui.add(egui::TextEdit::singleline(&mut text).id(id).desired_width(width));
    let Some(doc) = doc else { return r };
    let save = r.has_focus().then(|| crate::keymap::take_save_shortcut(ui)).flatten();
    if r.has_focus() || r.lost_focus() {
        hold(app, doc, &field, Typed::Text(text), Typed::Text(current.to_string()));
    }
    // While a message is up (a refused value from Save) the field lets go of the keyboard, which is not an Enter.
    if r.lost_focus() && app.message.is_none() {
        finish(app, ui, doc, &field);
    }
    if let Some(command) = save {
        app.run_or_alert(command, json!({}));
    }
    r
}

/// The Format Chart Size section's Width and Height boxes for chart `chart` of the sheet named
/// `sheet`, which hold `current`. The value being typed or dragged is kept between frames; it is
/// entered when the drag ends or a box loses focus.
pub(crate) fn size_fields(app: &mut SheetApp, ui: &mut Ui, sheet: &str, chart: u32, current: (f32, f32)) {
    let Some(doc) = app.session.active().map(|d| d.uid) else { return };
    let field = Field::ChartSize { sheet: sheet.to_string(), chart };
    let (mut w, mut h) = match app.grid.field_draft.as_ref().filter(|d| d.is(doc, &field)).map(|d| &d.typed) {
        Some(Typed::Size(w, h)) => (*w, *h),
        _ => current,
    };
    let a = ui.add(egui::DragValue::new(&mut w).prefix("Width ").range(40.0..=4000.0));
    let b = ui.add(egui::DragValue::new(&mut h).prefix("Height ").range(40.0..=4000.0));
    let editing = a.dragged() || a.has_focus() || b.dragged() || b.has_focus();
    let done = a.drag_stopped() || a.lost_focus() || b.drag_stopped() || b.lost_focus();
    let save = (a.has_focus() || b.has_focus()).then(|| crate::keymap::take_save_shortcut(ui)).flatten();
    if editing || done {
        hold(app, doc, &field, Typed::Size(w, h), Typed::Size(current.0, current.1));
    }
    if done && app.message.is_none() {
        finish(app, ui, doc, &field);
    }
    if let Some(command) = save {
        app.run_or_alert(command, json!({}));
    }
}

/// Keeps `typed` as the draft of `field`. Another field's draft is entered first.
fn hold(app: &mut SheetApp, doc: u64, field: &Field, typed: Typed, original: Typed) {
    if let Some(d) = app.grid.field_draft.as_mut().filter(|d| d.is(doc, field)) {
        d.typed = typed;
        return;
    }
    if let Some(other) = app.grid.field_draft.take() {
        let _ = enter(app, other, false);
    }
    app.grid.field_draft = Some(FieldDraft { doc, field: field.clone(), typed, original });
}

/// The field lost focus: Enter or a click elsewhere enters what was typed, Escape drops it. A
/// value the workbook refuses is reported and the field goes back to the workbook's value.
fn finish(app: &mut SheetApp, ui: &Ui, doc: u64, field: &Field) {
    if !app.grid.field_draft.as_ref().is_some_and(|d| d.is(doc, field)) {
        return;
    }
    let Some(draft) = app.grid.field_draft.take() else { return };
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        return;
    }
    if enter(app, draft, false).is_err() {
        // The Enter that ended the edit must not also dismiss the message it raised.
        ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
    }
}

/// Enters the window's draft, as Enter would, before a command that writes the file, switches
/// workbooks or closes one (see [`SheetApp::settle_drafts`]). A value the workbook refuses stays
/// open with the reason shown, and the command doesn't run.
pub(crate) fn commit(app: &mut SheetApp) -> Result<(), String> {
    let Some(draft) = app.grid.field_draft.take() else { return Ok(()) };
    enter(app, draft, true)
}

/// Writes the draft into its workbook. On a refusal the reason is shown, and the draft is put
/// back when `keep` (a command is waiting for it) instead of dropped.
fn enter(app: &mut SheetApp, draft: FieldDraft, keep: bool) -> Result<(), String> {
    if !draft.changed() {
        return Ok(());
    }
    match apply(app, &draft) {
        Ok(()) => Ok(()),
        Err(e) => {
            app.message = Some(("GridCraft".into(), e.clone()));
            if keep {
                app.grid.field_draft = Some(draft);
            }
            Err(e)
        }
    }
}

fn apply(app: &mut SheetApp, d: &FieldDraft) -> Result<(), String> {
    // A draft goes with its workbook when that closes, as the field it was typed in does.
    if !app.session.documents().iter().any(|x| x.uid == d.doc) {
        return Ok(());
    }
    let Some(doc) = app.session.active().filter(|x| x.uid == d.doc) else {
        return lost(app, d, "that workbook is not the one showing");
    };
    if let Some((sheet, chart)) = d.field.chart() {
        let there = doc.wb.active().filter(|s| s.name.eq_ignore_ascii_case(sheet)).is_some_and(|s| s.charts.iter().any(|c| c.id == chart));
        if !there {
            return lost(app, d, "that chart is not on the sheet showing");
        }
    }
    let params = match (&d.field, &d.typed) {
        (Field::TableName(table), Typed::Text(name)) => {
            if doc.wb.table(table).is_none() {
                return lost(app, d, "that table is gone");
            }
            ("table.rename", json!({"table": table, "name": name}))
        }
        (Field::ChartTitle { chart, .. }, Typed::Text(t)) => ("chart.set", json!({"chart": chart, "title": t})),
        (Field::ChartXTitle { chart, .. }, Typed::Text(t)) => ("chart.set", json!({"chart": chart, "xTitle": null_if_empty(t)})),
        (Field::ChartYTitle { chart, .. }, Typed::Text(t)) => ("chart.set", json!({"chart": chart, "yTitle": null_if_empty(t)})),
        (Field::ChartSize { chart, .. }, Typed::Size(w, h)) => ("chart.set", json!({"chart": chart, "width": w, "height": h})),
        (Field::Property(key), Typed::Text(t)) => ("file.properties", json!({ *key: t })),
        // These apply only while iterative calculation is on: a value left in the field when it is
        // switched off is dropped.
        (Field::MaxIterations, Typed::Text(t)) => {
            if !doc.wb.calc.iterative {
                return Ok(());
            }
            let Some(n) = crate::backstage::options::parse_max_iterations(t) else {
                return Err(app.ui.language.tr("Maximum Iterations must be a whole number from 1 to 32767.").into());
            };
            if n == doc.wb.calc.max_iterations {
                return Ok(());
            }
            ("formulas.calculationOptions", json!({"maxIterations": n}))
        }
        (Field::MaxChange, Typed::Text(t)) => {
            if !doc.wb.calc.iterative {
                return Ok(());
            }
            let Some(change) = crate::backstage::options::parse_max_change(t, app.ui.language.formula_locale()) else {
                return Err(app.ui.language.tr("Maximum Change must be a number of 0 or more.").into());
            };
            if change == doc.wb.calc.max_change {
                return Ok(());
            }
            ("formulas.calculationOptions", json!({"maxChange": change}))
        }
        _ => return Ok(()),
    };
    app.run(params.0, params.1).map(|_: Json| ()).map_err(|e| clean_error(&e))
}

fn null_if_empty(t: &str) -> Json {
    if t.is_empty() { Json::Null } else { json!(t) }
}

/// A draft whose target is not there to enter it into is not entered anywhere else; what was typed
/// is said, not lost unseen.
fn lost(app: &mut SheetApp, d: &FieldDraft, why: &str) -> Result<(), String> {
    app.message = Some(("GridCraft".into(), format!("The {} you typed ({}) was not entered: {why}.", d.field.what(), d.typed)));
    Ok(())
}
