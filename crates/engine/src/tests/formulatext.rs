use std::sync::Arc;

use gridcraft_core::{CellRef, Value};
use gridcraft_model::{Cell, Formula};
use serde_json::json;

use super::{Session, s, v};

fn imported(session: &mut Session, at: &str, display: &str) {
    let mut f = Formula::new(display);
    f.display_text = Some(display.into());
    let wb = Arc::make_mut(&mut session.doc_mut().unwrap().wb);
    wb.sheet_mut(0).unwrap().set_cell(CellRef::parse(at).unwrap(), Cell::formula(f));
}

fn formula_bar(session: &mut Session, at: &str) -> Value {
    session.execute("cell.set", json!({"cell": "Z1", "input": format!("=FORMULATEXT({at})")})).unwrap();
    v(session, "Z1")
}

#[test]
fn formulatext_preserves_only_identical_reentry_and_keeps_edit_effects() {
    let mut session = s();
    imported(&mut session, "A1", "RAND() + 0");
    session.execute("formulas.calculateNow", json!({})).unwrap();
    assert_eq!(formula_bar(&mut session, "A1"), Value::text("=RAND() + 0"));
    let before = v(&session, "A1").as_f64().unwrap();
    let undo = session.doc().unwrap().undo.len();
    session.execute("cell.set", json!({"cell": "A1", "input": "=RAND() + 0"})).unwrap();
    assert_ne!(v(&session, "A1").as_f64().unwrap(), before);
    assert_eq!(session.doc().unwrap().undo.len(), undo + 1);
    assert_eq!(v(&session, "Z1"), Value::text("=RAND() + 0"));
    session.execute("cell.set", json!({"cell": "A1", "input": "=RAND()+ 0"})).unwrap();
    assert_eq!(v(&session, "Z1"), Value::text("=RAND()+0"));
    session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(v(&session, "Z1"), Value::text("=RAND() + 0"));
}

#[test]
fn formulatext_keeps_spelling_until_references_change() {
    let mut session = s();
    imported(&mut session, "B2", "A2 + 1");
    assert_eq!(formula_bar(&mut session, "B2"), Value::text("=A2 + 1"));
    session.execute("home.insertRows", json!({"rows": "3:3"})).unwrap();
    assert_eq!(formula_bar(&mut session, "B2"), Value::text("=A2 + 1"));
    session.execute("edit.cut", json!({"range": "B2"})).unwrap();
    session.execute("edit.paste", json!({"at": "D2"})).unwrap();
    assert_eq!(formula_bar(&mut session, "D2"), Value::text("=A2 + 1"));
    session.execute("home.insertRows", json!({"rows": "2:2"})).unwrap();
    assert_eq!(formula_bar(&mut session, "D3"), Value::text("=A3+1"));

    imported(&mut session, "B4", "A4 + 1");
    session.execute("cell.set", json!({"cell": "A4", "input": "2"})).unwrap();
    session.execute("edit.cut", json!({"range": "A4:B4"})).unwrap();
    session.execute("edit.paste", json!({"at": "C4"})).unwrap();
    assert_eq!(formula_bar(&mut session, "D4"), Value::text("=C4+1"));
}

#[test]
fn cell_width_uses_imported_base_until_a_width_is_set() {
    let mut session = s();
    let wb = Arc::make_mut(&mut session.doc_mut().unwrap().wb);
    let sheet = wb.sheet_mut(0).unwrap();
    sheet.default_col_width = 80.0;
    sheet.xlsx_base_col_width = Some(10);
    sheet.cols.entry(1).or_default().size = Some(84.0);
    for (at, column, want) in [("A1", "A", 10.0), ("B1", "B", 11.0), ("C1", "C", 10.0)] {
        session.execute("cell.set", json!({"cell": at, "input": format!("=CELL(\"width\",{column}2)")})).unwrap();
        assert_eq!(v(&session, at), Value::Number(want));
    }
    session.execute("home.columnWidth", json!({"cols": "A:A", "width": 89})).unwrap();
    session.execute("formulas.calculateNow", json!({})).unwrap();
    assert_eq!(v(&session, "A1"), Value::Number(12.0));
    assert_eq!(v(&session, "C1"), Value::Number(10.0));
    session.execute("home.defaultWidth", json!({"width": 103})).unwrap();
    session.execute("formulas.calculateNow", json!({})).unwrap();
    assert_eq!(v(&session, "A1"), Value::Number(12.0));
    assert_eq!(v(&session, "C1"), Value::Number(14.0));
}
