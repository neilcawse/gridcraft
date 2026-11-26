//! Form drop-downs: the list follows its source cells, choosing an item writes its position to
//! the linked cell as one undo step and survives a save, and structural edits and renames move
//! the references.

use gridcraft_core::{CellRef, Value};
use gridcraft_model::{ControlKind, FormControl, Workbook};
use serde_json::{Value as Json, json};

use super::{s, v};
use crate::cmd::controls::{drop_items, drop_len, drop_shown};
use crate::{Session, Trust};

fn id(r: &Json) -> u32 {
    r["control"].as_u64().and_then(|n| u32::try_from(n).ok()).unwrap()
}

fn control(s: &Session, id: u32) -> FormControl {
    s.doc().unwrap().wb.sheet(0).unwrap().controls.iter().find(|c| c.id == id).cloned().unwrap()
}

/// What control `id` on the first sheet lists, all of it.
fn items(s: &Session, id: u32) -> Vec<String> {
    let wb = &s.doc().unwrap().wb;
    drop_items(wb, 0, &control(s, id), 0, u32::MAX)
}

fn shown(s: &Session, id: u32) -> String {
    drop_shown(&s.doc().unwrap().wb, 0, &control(s, id))
}

fn undo_len(s: &Session) -> usize {
    s.doc().unwrap().undo.len()
}

/// Saved as xlsx and opened again as a new document.
fn reopen(s: &mut Session) {
    let bytes = gridcraft_xlsx::write_xlsx(&s.doc().unwrap().wb).unwrap();
    let (wb, _) = gridcraft_xlsx::read_xlsx(&bytes).unwrap();
    s.add_document(crate::DocState::new(wb, None, "Reopened".into(), Trust::FromFile));
}

fn colours(s: &mut Session) {
    s.execute("range.setValues", json!({"range": "A1:A4", "values": [["Red"], ["Green"], ["Blue"], [0.25]]})).unwrap();
    s.execute("home.formatCells", json!({"range": "A4", "style": {"num_fmt": "0%"}})).unwrap();
}

#[test]
fn choosing_an_item_writes_its_position_as_one_undo_step_and_survives_a_save() {
    let mut s = s();
    colours(&mut s);
    s.execute("cell.set", json!({"cell": "F1", "input": "=E1*10"})).unwrap();
    let c = id(&s.execute("insert.dropDown", json!({"at": "C2", "listRange": "a1:a4", "link": "e1", "dropLines": 3})).unwrap());
    let ctl = control(&s, c);
    assert_eq!(
        (ctl.kind, ctl.list_range.as_deref(), ctl.link.as_deref(), ctl.drop_lines, ctl.selected),
        (ControlKind::Drop, Some("$A$1:$A$4"), Some("$E$1"), 3, 0)
    );
    // Items show as their cells do (through the number format).
    assert_eq!(items(&s, c), ["Red", "Green", "Blue", "25%"]);
    let listed = s.execute("control.list", json!({})).unwrap();
    assert_eq!((listed[0]["kind"].clone(), listed[0]["items"].clone()), (json!("dropDown"), json!(["Red", "Green", "Blue", "25%"])));
    assert_eq!(shown(&s, c), "", "nothing chosen yet");

    let before = undo_len(&s);
    let r = s.execute("control.setValue", json!({"id": c, "value": 2})).unwrap();
    assert_eq!(r["value"], json!(2));
    assert_eq!((control(&s, c).selected, v(&s, "E1"), v(&s, "F1")), (2, Value::Number(2.0), Value::Number(20.0)));
    assert_eq!(shown(&s, c), "Green");
    assert_eq!(undo_len(&s), before + 1, "the choice and the linked cell are one undo step");
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!((control(&s, c).selected, v(&s, "E1")), (0, Value::Empty));
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!((control(&s, c).selected, v(&s, "E1")), (2, Value::Number(2.0)));
    // Items that aren't in the list are refused and change nothing.
    for bad in [json!(5), json!(-1), json!(1.5), json!("x")] {
        assert!(s.execute("control.setValue", json!({"id": c, "value": bad.clone()})).is_err(), "{bad}");
        assert_eq!(control(&s, c).selected, 2, "{bad}");
    }

    // Editing the source cells changes the list; editing the linked cell changes the choice.
    s.execute("cell.set", json!({"cell": "A2", "input": "Lime"})).unwrap();
    assert_eq!(shown(&s, c), "Lime");
    s.execute("cell.set", json!({"cell": "E1", "input": "3"})).unwrap();
    assert_eq!((control(&s, c).selected, shown(&s, c)), (3, "Blue".to_string()));

    // Saved and opened again: the same choice in the control and in its cell; choosing again
    // there and saving keeps the new choice.
    reopen(&mut s);
    let c2 = s.doc().unwrap().wb.sheet(0).unwrap().controls[0].id;
    assert_eq!((control(&s, c2).selected, v(&s, "E1"), shown(&s, c2)), (3, Value::Number(3.0), "Blue".to_string()));
    s.execute("control.setValue", json!({"id": c2, "value": 4})).unwrap();
    reopen(&mut s);
    let c3 = s.doc().unwrap().wb.sheet(0).unwrap().controls[0].id;
    assert_eq!((control(&s, c3).selected, v(&s, "E1"), shown(&s, c3)), (4, Value::Number(4.0), "25%".to_string()));
    assert_eq!(control(&s, c3).drop_lines, 3);
}

#[test]
fn format_changes_the_list_and_rows() {
    let mut s = s();
    colours(&mut s);
    let c = id(&s.execute("insert.dropDown", json!({"listRange": "A1:A2", "link": "E1", "value": 2})).unwrap());
    assert_eq!(v(&s, "E1"), Value::Number(2.0), "a given value is written");
    let r = s.execute("control.format", json!({"id": c, "listRange": "A1:A4", "dropLines": 12, "value": 4})).unwrap();
    assert_eq!((r["listRange"].clone(), r["dropLines"].clone(), r["selected"].clone()), (json!("$A$1:$A$4"), json!(12), json!(4)));
    assert_eq!(v(&s, "E1"), Value::Number(4.0));
    for (p, needle) in [
        (json!({"id": c, "listRange": "nonsense"}), "isn't a range"),
        (json!({"id": c, "listRange": "Nowhere!A1:A3"}), "isn't a range"),
        (json!({"id": c, "dropLines": 0}), "at least 1"),
        (json!({"id": c, "value": 9}), "from 1 to 4"),
    ] {
        let e = s.execute("control.format", p.clone()).unwrap_err().to_string();
        assert!(e.contains(needle), "{p}: {e}");
    }
    s.execute("control.format", json!({"id": c, "listRange": ""})).unwrap();
    assert_eq!((control(&s, c).list_range, items(&s, c).len()), (None, 0));
}

#[test]
fn changing_the_input_range_clamps_only_present_scroll_offsets() {
    let mut s = s();
    let mut wb = Workbook::new();
    let display = gridcraft_model::DropDisplay { no_three_d: Some(true), no_three_d2: Some(false), scroll_width: Some(35), scroll_offset: Some(4) };
    let vml = gridcraft_model::DropDisplay { no_three_d: Some(false), no_three_d2: Some(true), scroll_width: Some(45), scroll_offset: Some(3) };
    let sh = wb.sheet_mut(0).unwrap();
    for (id, drop_display, drop_vml_display) in [(1, display, vml), (2, Default::default(), Default::default())] {
        sh.controls.push(FormControl {
            id,
            kind: ControlKind::Drop,
            list_range: Some("$A$1:$A$5".into()),
            drop_display,
            drop_vml_display,
            ..Default::default()
        });
    }
    s.add_document(crate::DocState::new(wb, None, "Drop settings".into(), Trust::FromFile));
    reopen(&mut s);
    let settings = |s: &Session, id| {
        let c = control(s, id);
        (c.drop_display, c.drop_vml_display)
    };
    s.execute("control.format", json!({"id": 1, "name": "Renamed", "listRange": "A1:A5"})).unwrap();
    assert_eq!(settings(&s, 1), (display, vml));
    s.execute("control.format", json!({"id": 1, "listRange": "A1:A4"})).unwrap();
    assert_eq!(settings(&s, 1), (gridcraft_model::DropDisplay { scroll_offset: Some(3), ..display }, vml));
    s.execute("control.format", json!({"id": 1, "listRange": ""})).unwrap();
    s.execute("control.format", json!({"id": 2, "listRange": "A1:A2"})).unwrap();
    reopen(&mut s);
    assert_eq!(
        settings(&s, 1),
        (gridcraft_model::DropDisplay { scroll_offset: Some(0), ..display }, gridcraft_model::DropDisplay { scroll_offset: Some(0), ..vml },)
    );
    assert_eq!(settings(&s, 2), (Default::default(), Default::default()));
}

/// Structural edits keep saved scroll positions inside the list only when they shorten it (rows
/// deleted inside it, its sheet deleted); an unrelated insert or a rename leaves them as read,
/// even one the file saved past the end of its list.
#[test]
fn structural_edits_clamp_scroll_offsets_only_of_shortened_lists() {
    use gridcraft_model::DropDisplay;
    let mut s = s();
    let mut wb = Workbook::new();
    wb.sheets.push(std::sync::Arc::new(gridcraft_model::Sheet::new("Lists")));
    let offset = |n| DropDisplay { scroll_offset: Some(n), scroll_width: Some(35), ..Default::default() };
    let sh = wb.sheet_mut(0).unwrap();
    for (id, list_range, at) in [(1, "$A$1:$A$9", 8), (2, "$C$1:$C$3", 7), (3, "Lists!$A$1:$A$9", 8), (4, "=Missing!$A$1:$A$9", 6)] {
        sh.controls.push(FormControl {
            id,
            kind: ControlKind::Drop,
            list_range: Some(list_range.into()),
            drop_display: offset(at),
            drop_vml_display: offset(at),
            ..Default::default()
        });
    }
    s.add_document(crate::DocState::new(wb, None, "Offsets".into(), Trust::FromFile));
    let offsets = |s: &Session, id| {
        let c = control(s, id);
        (c.drop_display.scroll_offset, c.drop_vml_display.scroll_offset, c.drop_display.scroll_width)
    };
    // A row inserted above every list, and a rename: no list is shorter, so nothing is clamped
    // (control 2 was saved past the end of its three items and stays so).
    s.execute("home.insertRows", json!({"rows": "1:1", "sheet": 0})).unwrap();
    assert_eq!(control(&s, 4).list_range.as_deref(), Some("Missing!$A$1:$A$9"));
    assert_eq!(offsets(&s, 4), (Some(6), Some(6), Some(35)));
    s.execute("sheet.rename", json!({"sheet": 1, "name": "Other"})).unwrap();
    assert_eq!(
        (offsets(&s, 1), offsets(&s, 2), offsets(&s, 3)),
        ((Some(8), Some(8), Some(35)), (Some(7), Some(7), Some(35)), (Some(8), Some(8), Some(35)))
    );
    // Four rows deleted inside the first list leave five items: the offsets end at the last one.
    s.execute("home.deleteRows", json!({"rows": "3:6", "sheet": 0})).unwrap();
    assert_eq!(control(&s, 1).list_range.as_deref(), Some("$A$2:$A$6"));
    assert_eq!(offsets(&s, 1), (Some(4), Some(4), Some(35)));
    // The third list's sheet deleted: it names nothing, so its offsets go to 0.
    s.execute("home.deleteSheet", json!({"sheet": 1})).unwrap();
    assert_eq!(offsets(&s, 3), (Some(0), Some(0), Some(35)));
    // Undo brings back the sheet, the list and the offsets exactly; redo clamps them again.
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!((control(&s, 3).list_range.as_deref(), offsets(&s, 3)), (Some("Other!$A$1:$A$9"), (Some(8), Some(8), Some(35))));
    s.execute("edit.redo", json!({})).unwrap();
    assert!(control(&s, 3).list_range.as_deref().is_some_and(|r| r.contains("#REF!")), "{:?}", control(&s, 3).list_range);
    assert_eq!(offsets(&s, 3), (Some(0), Some(0), Some(35)));
    // Saved and read back, the clamped offsets are what the file holds.
    reopen(&mut s);
    assert_eq!((offsets(&s, 1), offsets(&s, 3)), ((Some(4), Some(4), Some(35)), (Some(0), Some(0), Some(35))));
    assert_eq!(offsets(&s, 4), (Some(6), Some(6), Some(35)));
}

/// Deleting a sheet rewrites the links of the objects on the other sheets only: the deleted sheet,
/// shared with the undo copy, is not copied on the way out.
#[test]
fn deleting_a_sheet_does_not_copy_it_to_rewrite_its_own_links() {
    let mut wb = Workbook::new();
    wb.sheets.push(std::sync::Arc::new(gridcraft_model::Sheet::new("Doomed")));
    let lists = |at: &str| FormControl { kind: ControlKind::Drop, list_range: Some(at.into()), link: Some("$B$1".into()), ..Default::default() };
    wb.sheet_mut(0).unwrap().controls.push(FormControl { id: 1, ..lists("Doomed!$A$1:$A$9") });
    wb.sheet_mut(1).unwrap().controls.push(FormControl { id: 2, ..lists("Sheet1!$A$1:$A$9") });
    let order: Vec<String> = wb.sheets.iter().map(|s| s.name.clone()).collect();
    let mut edited = wb.clone();
    crate::cmd::edit::rewrite_cell_links(&mut edited, Some(1), |_, e| gridcraft_formula::adjust::delete_sheet(e, "Doomed", &order));
    assert!(std::sync::Arc::ptr_eq(&wb.sheets[1], &edited.sheets[1]), "the deleted sheet was copied");
    assert!(edited.sheets[0].controls[0].list_range.as_deref().is_some_and(|r| r.contains("#REF!")));
}

#[test]
fn references_follow_structural_edits_and_renames() {
    let mut s = s();
    s.execute("home.insertSheet", json!({"name": "Lists"})).unwrap();
    s.execute("range.setValues", json!({"range": "B2:B4", "values": [["one"], ["two"], ["three"]]})).unwrap();
    s.execute("sheet.activate", json!({"sheet": "Sheet1"})).unwrap();
    colours(&mut s);
    let remote = id(&s.execute("insert.dropDown", json!({"at": "H2", "listRange": "Lists!B2:B4", "link": "B3"})).unwrap());
    let local = id(&s.execute("insert.dropDown", json!({"at": "H6", "listRange": "A1:A4", "link": "C1", "value": 3})).unwrap());
    let refs = |s: &Session, id: u32| {
        let c = control(s, id);
        (c.list_range.unwrap_or_default(), c.link.unwrap_or_default())
    };
    assert_eq!(refs(&s, remote), ("Lists!$B$2:$B$4".into(), "$B$3".into()));

    // Rows inserted above on the control's sheet move the local list and both links.
    s.execute("home.insertRows", json!({"rows": "1:1", "sheet": 0})).unwrap();
    assert_eq!(refs(&s, local), ("$A$2:$A$5".into(), "$C$2".into()));
    assert_eq!(refs(&s, remote), ("Lists!$B$2:$B$4".into(), "$B$4".into()), "the list on Lists stays");
    assert_eq!((items(&s, local), shown(&s, local)), (vec!["Red".to_string(), "Green".into(), "Blue".into(), "25%".into()], "Blue".to_string()));
    // Columns inserted on Lists move the remote list; then the sheet is renamed.
    let lists = s.doc().unwrap().wb.sheet_index("Lists").unwrap();
    s.execute("home.insertColumns", json!({"cols": "A:A", "sheet": lists})).unwrap();
    assert_eq!(refs(&s, remote).0, "Lists!$C$2:$C$4");
    s.execute("sheet.rename", json!({"sheet": lists, "name": "My Lists"})).unwrap();
    assert_eq!(refs(&s, remote).0, "'My Lists'!$C$2:$C$4");
    assert_eq!(items(&s, remote), ["one", "two", "three"]);
    // A row inserted inside the list grows it.
    s.execute("home.insertRows", json!({"rows": "3:3", "sheet": 0})).unwrap();
    assert_eq!(refs(&s, local).0, "$A$2:$A$6");
    assert_eq!(items(&s, local).len(), 5);
    // Deleting the list's cells leaves a reference to nothing: an empty list, no panic.
    s.execute("home.deleteColumns", json!({"cols": "C:C", "sheet": lists})).unwrap();
    assert!(refs(&s, remote).0.contains("#REF!"), "{:?}", refs(&s, remote));
    assert_eq!((items(&s, remote).len(), drop_len(&s.doc().unwrap().wb, 0, &control(&s, remote))), (0, 0));
    assert!(s.execute("control.setValue", json!({"id": remote, "value": 1})).is_err());
    // Undo brings the references back, step by step.
    for _ in 0..3 {
        s.execute("edit.undo", json!({})).unwrap();
    }
    assert_eq!(refs(&s, remote).0, "Lists!$C$2:$C$4");
    assert_eq!(refs(&s, local).0, "$A$2:$A$5");
    // Saved and opened again, the moved references hold.
    reopen(&mut s);
    let back: Vec<(String, String)> = s
        .doc()
        .unwrap()
        .wb
        .sheet(0)
        .unwrap()
        .controls
        .iter()
        .map(|c| (c.list_range.clone().unwrap_or_default(), c.link.clone().unwrap_or_default()))
        .collect();
    assert_eq!(back, [("Lists!$C$2:$C$4".to_string(), "$B$4".to_string()), ("$A$2:$A$5".into(), "$C$2".into())]);
}

/// A drop-down with a source range that names nothing, covers a whole sheet or another
/// workbook's sheet, and a chosen item far past its list: listing, showing and choosing never
/// panic and read only the cells asked for.
#[test]
fn hostile_drop_down_state_never_panics() {
    let mut s = s();
    colours(&mut s);
    let mut wb = Workbook::new();
    let ranges = ["#REF!", "A1:XFD1048576", "'No Sheet'!A1:A3", "", "A:A", "[2]Other!A1:A3", "=A1:A4", "A4:A1", "1:1"];
    if let Some(sh) = wb.sheet_mut(0) {
        sh.set_value(CellRef::new(0, 0), Value::text("x"));
        for (i, r) in ranges.iter().enumerate() {
            let c = FormControl {
                id: i as u32 + 1,
                kind: ControlKind::Drop,
                list_range: Some((*r).to_string()),
                selected: 30_000,
                link: Some("$Z$1".into()),
                ..Default::default()
            };
            sh.controls.push(c);
        }
    }
    s.add_document(crate::DocState::new(wb, None, "Hostile".into(), Trust::FromFile));
    let listed = s.execute("control.list", json!({})).unwrap();
    for (i, c) in listed.as_array().unwrap().iter().enumerate() {
        assert!(c["items"].as_array().is_some_and(|a| a.len() <= 1_000), "{i}: the listing is capped");
    }
    for i in 1..=ranges.len() as u32 {
        let c = control(&s, i);
        let wb = &s.doc().unwrap().wb;
        assert!(drop_len(wb, 0, &c) <= gridcraft_model::CONTROL_MAX);
        let _ = (drop_shown(wb, 0, &c), drop_items(wb, 0, &c, u32::MAX - 1, u32::MAX));
        let _ = s.execute("control.setValue", json!({"id": i, "value": 1e300}));
        let _ = s.execute("control.setValue", json!({"id": i, "value": 1}));
    }
    // An edit runs the linked-cell sync over every one of them.
    s.execute("cell.set", json!({"cell": "Z1", "input": "99999999"})).unwrap();
    assert!(s.doc().unwrap().wb.sheet(0).unwrap().controls.iter().all(|c| c.selected <= gridcraft_model::CONTROL_MAX));
    assert!(s.execute("insert.dropDown", json!({"listRange": 5})).is_err());
    assert!(s.execute("insert.dropDown", json!({"listRange": "A1:A3", "value": 4})).is_err());
}
