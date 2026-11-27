//! The copy/cut marquee stays on the sheet it was copied from when sheets are inserted or deleted
//! before it, and goes away with a deleted source sheet. Keeping the marquee on the source tab is
//! inferred from Excel's copy mode marking the copied cells, not measured; whether a sheet insert
//! should end copy mode is unmeasured and not changed here.

use egui::Shape;
use gridcraft_engine::Session;
use gridcraft_ui_egui::theme::Tokens;
use serde_json::json;

use crate::common::Harness;

/// A workbook with `sheets`, `B2:C3` copied (or cut) on Sheet2.
fn copied_on_sheet2(sheets: &[&str], cut: bool) -> Harness {
    let mut session = Session::new();
    session.new_workbook();
    for (at, name) in sheets.iter().enumerate().skip(1) {
        session.run("home.insertSheet", json!({"name": name, "before": at})).unwrap();
    }
    session.run("sheet.activate", json!({"sheet": "Sheet2"})).unwrap();
    session.run("range.setValues", json!({"range": "B2", "values": [[1, 2], [3, 4]]})).unwrap();
    session.run("selection.set", json!({"range": "B2:C3"})).unwrap();
    session.run(if cut { "edit.cut" } else { "edit.copy" }, json!({})).unwrap();
    let mut h = crate::common::harness_with(crate::common::builder().with_size(egui::vec2(1200.0, 800.0)), crate::common::app(session), 4);
    assert_eq!(h.state().session.active().unwrap().wb.sheets.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), sheets);
    assert!(ants(&h) > 0, "the marquee shows on the source sheet");
    h
}

fn run(h: &mut Harness, id: &str, params: serde_json::Value) {
    h.state_mut().run(id, params).unwrap();
    h.run_steps(4);
}

/// Shows sheet `name` and counts the marquee's dashes painted on it.
fn ants_on(h: &mut Harness, name: &str) -> usize {
    run(h, "sheet.activate", json!({"sheet": name}));
    assert!(h.state().session.clipboard.is_some(), "the copy is still on the clipboard");
    ants(h)
}

/// The marquee's dashes: 2-point line segments in the selection border colour, at most 4 points
/// long. The selected headers' underlines share the stroke but span a whole row or column header.
fn ants(h: &Harness) -> usize {
    let border = Tokens::get(&h.ctx).sel_border;
    h.output()
        .shapes
        .iter()
        .filter(|s| {
            matches!(&s.shape, Shape::LineSegment { points: [a, b], stroke }
                if stroke.color == border && stroke.width == 2.0 && a.distance(*b) <= 4.5)
        })
        .count()
}

#[test]
fn marquee_stays_on_the_source_after_a_sheet_is_inserted_before_it() {
    for cut in [false, true] {
        let mut h = copied_on_sheet2(&["Sheet1", "Sheet2"], cut);
        // The new sheet takes the source's old index 1; Sheet2 moves to 2.
        run(&mut h, "home.insertSheet", json!({"name": "New", "before": 1}));
        assert_eq!(ants_on(&mut h, "New"), 0, "cut {cut}: no marquee on the new sheet at the old index");
        assert!(ants_on(&mut h, "Sheet2") > 0, "cut {cut}: the marquee follows the source sheet");
    }
}

#[test]
fn marquee_stays_on_the_source_after_an_earlier_sheet_is_deleted() {
    for cut in [false, true] {
        let mut h = copied_on_sheet2(&["Sheet1", "Sheet2", "Sheet3"], cut);
        // Sheet2 moves to index 0 and Sheet3 takes its old index 1.
        run(&mut h, "home.deleteSheet", json!({"sheet": "Sheet1"}));
        assert_eq!(ants_on(&mut h, "Sheet3"), 0, "cut {cut}: no marquee on the sheet now at the old index");
        assert!(ants_on(&mut h, "Sheet2") > 0, "cut {cut}: the marquee follows the source sheet");
    }
}

#[test]
fn deleting_the_source_sheet_leaves_no_marquee() {
    for cut in [false, true] {
        let mut h = copied_on_sheet2(&["Sheet1", "Sheet2", "Sheet3"], cut);
        run(&mut h, "home.deleteSheet", json!({"sheet": "Sheet2"}));
        assert_eq!(ants_on(&mut h, "Sheet3"), 0, "cut {cut}: no marquee on the sheet now at the old index");
        assert_eq!(ants_on(&mut h, "Sheet1"), 0, "cut {cut}: no marquee on another sheet");
    }
}
