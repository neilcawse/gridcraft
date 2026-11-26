//! Legacy form check boxes: listed from a file, checked and cleared through `control.setValue`
//! with the linked cell following the documented rule (TRUE, FALSE, #N/A) as one undo step, kept
//! off a protected sheet's locked cells, moved by structural edits and renames, and saved again
//! as the same check boxes.

use gridcraft_core::{CellError, Value};
use gridcraft_model::{CheckState, FormControl};
use gridcraft_xlsx::testing::{CHECK_BOX_NAMES, legacy_check_boxes};
use serde_json::{Value as Json, json};

use super::{s, v};
use crate::Session;

fn open(s: &mut Session, bytes: &[u8]) -> Json {
    s.execute("file.open", json!({"name": "checks.xlsx", "base64": crate::io::base64_encode(bytes)})).unwrap();
    s.execute("control.list", json!({})).unwrap()
}

fn listed_id(list: &Json, name: &str) -> u32 {
    let c = list.as_array().unwrap().iter().find(|c| c["name"] == json!(name)).unwrap_or_else(|| panic!("{name}: {list}"));
    u32::try_from(c["id"].as_u64().unwrap()).unwrap()
}

fn control(s: &Session, id: u32) -> FormControl {
    s.doc().unwrap().wb.active().unwrap().controls.iter().find(|c| c.id == id).cloned().unwrap()
}

fn reopen(s: &mut Session) -> Json {
    let r = s.execute("file.saveBytes", json!({"format": "xlsx"})).unwrap();
    let b64 = r["base64"].as_str().unwrap().to_string();
    s.execute("file.open", json!({"name": "again.xlsx", "base64": b64})).unwrap();
    s.execute("control.list", json!({})).unwrap()
}

/// (name, kind, state, value, text, link) of each listed control.
fn summary(list: &Json) -> Vec<(String, String, String, u64, String, String)> {
    list.as_array()
        .unwrap()
        .iter()
        .map(|c| {
            let t = |k: &str| c[k].as_str().unwrap_or_default().to_string();
            (t("name"), t("kind"), t("state"), c["value"].as_u64().unwrap_or(99), t("text"), t("link"))
        })
        .collect()
}

#[test]
fn a_files_check_boxes_list_toggle_undo_and_save() {
    for x14 in [false, true] {
        let mut s = s();
        let list = open(&mut s, &legacy_check_boxes(x14));
        let want = vec![
            (CHECK_BOX_NAMES[0].to_string(), "checkBox".to_string(), "checked".to_string(), 1, "Ship it".to_string(), "$B$2".to_string()),
            (CHECK_BOX_NAMES[1].into(), "checkBox".into(), "unchecked".into(), 0, "Second".into(), String::new()),
            (CHECK_BOX_NAMES[2].into(), "checkBox".into(), "mixed".into(), 2, "Maybe".into(), String::new()),
        ];
        assert_eq!(summary(&list), want, "x14: {x14}");
        assert_eq!((list[0]["threeD"].clone(), list[1]["threeD"].clone()), (json!(false), json!(true)));
        assert_eq!(list[0]["at"], json!("D2"));
        let first = listed_id(&list, CHECK_BOX_NAMES[0]);

        // Cleared: the linked cell says FALSE; mixed: #N/A; checked: TRUE. Each is one undo step.
        let undo = s.doc().unwrap().undo.len();
        let r = s.execute("control.setValue", json!({"id": first, "value": false})).unwrap();
        assert_eq!(r, json!({"value": 0, "state": "unchecked"}));
        assert_eq!((control(&s, first).checked, v(&s, "B2")), (CheckState::Unchecked, Value::Bool(false)));
        assert_eq!(s.doc().unwrap().undo.len(), undo + 1, "the state and the cell are one undo step");
        s.execute("control.setValue", json!({"id": first, "value": "mixed"})).unwrap();
        assert_eq!((control(&s, first).checked, v(&s, "B2")), (CheckState::Mixed, Value::Error(CellError::NA)));
        s.execute("control.setValue", json!({"id": first, "value": 1})).unwrap();
        assert_eq!((control(&s, first).checked, v(&s, "B2")), (CheckState::Checked, Value::Bool(true)));
        for _ in 0..2 {
            s.execute("edit.undo", json!({})).unwrap();
        }
        assert_eq!((control(&s, first).checked, v(&s, "B2")), (CheckState::Unchecked, Value::Bool(false)));
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!((control(&s, first).checked, v(&s, "B2")), (CheckState::Checked, Value::Bool(true)), "undo restores the file's state");
        for bad in [json!(3), json!(0.5), json!("perhaps"), json!([1])] {
            assert!(s.execute("control.setValue", json!({"id": first, "value": bad.clone()})).is_err(), "{bad}");
        }
        // Editing the linked cell moves the check box.
        s.execute("cell.set", json!({"cell": "B2", "input": false})).unwrap();
        assert_eq!(control(&s, first).checked, CheckState::Unchecked);
        s.execute("cell.set", json!({"cell": "B2", "input": "#N/A"})).unwrap();
        assert_eq!(control(&s, first).checked, CheckState::Mixed);
        s.execute("cell.set", json!({"cell": "B2", "input": true})).unwrap();
        assert_eq!(control(&s, first).checked, CheckState::Checked);

        // A second check box without a link changes only itself.
        let second = listed_id(&list, CHECK_BOX_NAMES[1]);
        s.execute("control.setValue", json!({"id": second, "value": "checked"})).unwrap();
        // Saved and opened again: the same names, states, links and captions.
        let back = reopen(&mut s);
        let mut want = want;
        want[1].2 = "checked".into();
        want[1].3 = 1;
        assert_eq!(summary(&back), want, "x14: {x14}, after a save");
        assert_eq!(v(&s, "B2"), Value::Bool(true));
    }
}

#[test]
fn a_locked_linked_cell_on_a_protected_sheet_refuses_the_change() {
    let mut s = s();
    let list = open(&mut s, &legacy_check_boxes(false));
    let first = listed_id(&list, CHECK_BOX_NAMES[0]);
    let second = listed_id(&list, CHECK_BOX_NAMES[1]);
    s.execute("review.protectSheet", json!({})).unwrap();
    // Its linked cell is locked: nothing changes, as for a slider whose cell is locked.
    assert!(s.execute("control.setValue", json!({"id": first, "value": false})).is_err());
    assert_eq!((control(&s, first).checked, v(&s, "B2")), (CheckState::Checked, Value::Bool(true)));
    // Without a link, a check box still works on a protected sheet, as other controls do; but it
    // is locked, so it can't be formatted.
    s.execute("control.setValue", json!({"id": second, "value": true})).unwrap();
    assert_eq!(control(&s, second).checked, CheckState::Checked);
    let e = s.execute("control.format", json!({"id": second, "text": "x"})).unwrap_err().to_string();
    assert!(e.contains("protected"), "{e}");
}

#[test]
fn links_follow_structural_edits_and_renames() {
    let mut s = s();
    open(&mut s, &legacy_check_boxes(false));
    s.execute("home.insertSheet", json!({"name": "Flags"})).unwrap();
    s.execute("sheet.activate", json!({"sheet": "Data"})).unwrap();
    let list = s.execute("control.list", json!({})).unwrap();
    let first = listed_id(&list, CHECK_BOX_NAMES[0]);
    let second = listed_id(&list, CHECK_BOX_NAMES[1]);
    s.execute("control.format", json!({"id": second, "link": "Flags!C3"})).unwrap();
    let link = |s: &Session, id| control(s, id).link.unwrap_or_default();
    assert_eq!(link(&s, second), "Flags!$C$3");
    s.execute("home.insertRows", json!({"rows": "1:1", "sheet": 0})).unwrap();
    assert_eq!(link(&s, first), "$B$3");
    assert_eq!(v(&s, "B3"), Value::Bool(true));
    s.execute("home.insertColumns", json!({"cols": "A:A", "sheet": 0})).unwrap();
    assert_eq!(link(&s, first), "$C$3");
    let flags = s.doc().unwrap().wb.sheet_index("Flags").unwrap();
    s.execute("home.deleteRows", json!({"rows": "1:1", "sheet": flags})).unwrap();
    s.execute("sheet.rename", json!({"sheet": flags, "name": "My Flags"})).unwrap();
    assert_eq!(link(&s, second), "'My Flags'!$C$2");
    // The moved link is the one written: clearing writes FALSE to the new cell.
    s.execute("control.setValue", json!({"id": first, "value": false})).unwrap();
    assert_eq!(v(&s, "C3"), Value::Bool(false));
    s.execute("control.setValue", json!({"id": second, "value": "mixed"})).unwrap();
    let wb = &s.doc().unwrap().wb;
    let at = gridcraft_core::CellRef::parse("C2").unwrap();
    assert_eq!(wb.sheet(wb.sheet_index("My Flags").unwrap()).unwrap().value(at), Value::Error(CellError::NA));
    // Undo takes back the two changes and the rename.
    for _ in 0..3 {
        s.execute("edit.undo", json!({})).unwrap();
    }
    assert_eq!((link(&s, first), link(&s, second)), ("$C$3".to_string(), "Flags!$C$2".to_string()));
    // Saved and opened again, the moved links hold.
    let back = reopen(&mut s);
    let links: Vec<&str> = back.as_array().unwrap().iter().map(|c| c["link"].as_str().unwrap_or_default()).collect();
    assert_eq!(links, ["$C$3", "Flags!$C$2", ""]);
}

#[test]
fn insert_and_format_a_check_box() {
    let mut s = s();
    let r = s.execute("insert.checkBoxControl", json!({"at": "C2", "link": "e1", "value": "checked"})).unwrap();
    let id = u32::try_from(r["control"].as_u64().unwrap()).unwrap();
    let c = control(&s, id);
    assert_eq!((c.checked, c.link.as_deref(), c.text.clone()), (CheckState::Checked, Some("$E$1"), format!("Check Box {id}")));
    assert_eq!(v(&s, "E1"), Value::Bool(true));
    let r = s.execute("control.format", json!({"id": id, "text": "Paid", "value": 2, "threeD": false})).unwrap();
    assert_eq!((r["state"].clone(), r["text"].clone(), r["threeD"].clone()), (json!("mixed"), json!("Paid"), json!(false)));
    assert_eq!(v(&s, "E1"), Value::Error(CellError::NA));
    // A file's left-out shading stays left out until it is turned off.
    let r = s.execute("insert.checkBoxControl", json!({})).unwrap();
    let plain = u32::try_from(r["control"].as_u64().unwrap()).unwrap();
    s.execute("control.format", json!({"id": plain, "threeD": true})).unwrap();
    assert_eq!(control(&s, plain).no_three_d, None);
    let back = reopen(&mut s);
    assert_eq!((back[0]["state"].clone(), back[0]["text"].clone(), back[0]["threeD"].clone()), (json!("mixed"), json!("Paid"), json!(false)));
    assert!(s.execute("control.press", json!({"id": back[0]["id"]})).unwrap_err().to_string().contains("setValue"));
}
