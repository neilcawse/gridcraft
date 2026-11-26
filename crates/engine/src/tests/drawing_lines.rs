//! Floating objects follow row heights and column widths by their anchor mode (ECMA-376 Part 1,
//! 20.5.3.2 `ST_EditAs`): on open, over the rows the open fitted, and after in-app resizes and
//! row fits.

use gridcraft_core::{CellRef, Value};
use gridcraft_model::{AnchorMode, DrawingRef, ObjectRect, StyleId};
use serde_json::json;

use super::{add_image, group_objects, s};
use crate::Session;

const EMU_PER_PX: f64 = 9525.0;

/// A column chart of A1:A4 at C2, 200 x 100 px, with anchor `mode`.
fn chart_session(mode: &str) -> Session {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [[1], [2], [3], [4]]})).unwrap();
    s.execute("selection.set", json!({"range": "A1:A4"})).unwrap();
    s.execute("insert.chart", json!({"type": "column", "at": "C2", "width": 200, "height": 100, "mode": mode})).unwrap();
    s
}

fn chart_rect(s: &Session) -> ObjectRect {
    let sh = s.doc().unwrap().wb.sheet(0).unwrap();
    sh.anchor_rect(&sh.charts[0].anchor)
}

fn assert_rect(got: ObjectRect, want: ObjectRect, what: &str) {
    let close = |a: f64, b: f64| (a - b).abs() < 1e-3;
    assert!(close(got.x, want.x) && close(got.y, want.y) && close(got.w, want.w) && close(got.h, want.h), "{what}: {got:?}, expected {want:?}");
}

/// The first two-cell anchor's opening tag and markers in a saved workbook's first drawing.
fn first_anchor_markers(bytes: &[u8]) -> String {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut xml = String::new();
    std::io::Read::read_to_string(&mut zip.by_name("xl/drawings/drawing1.xml").unwrap(), &mut xml).unwrap();
    let start = xml.find("<xdr:twoCellAnchor").expect("a two-cell anchor");
    let end = start + xml[start..].find("</xdr:to>").expect("a to marker") + "</xdr:to>".len();
    xml[start..end].to_string()
}

fn marker_number(markers: &str, marker: &str, field: &str) -> i64 {
    let at = markers.find(&format!("<xdr:{marker}>")).unwrap();
    let rest = &markers[at..];
    let open = format!("<xdr:{field}>");
    let value = &rest[rest.find(&open).unwrap() + open.len()..];
    value[..value.find('<').unwrap()].parse().unwrap()
}

/// A chart saved over rows whose 36 pt text the saved heights don't fit (none were fitted),
/// then opened: the open fits the row inside the chart, and the chart, whatever its `editAs`,
/// spans its saved markers over the fitted rows. Saving it unedited writes those markers back.
#[test]
fn open_places_a_chart_over_rows_the_open_fits() {
    for (mode, edit_as) in [("moveAndSize", ""), ("moveOnly", " editAs=\"oneCell\""), ("absolute", " editAs=\"absolute\"")] {
        let mut s = chart_session(mode);
        let wb = std::sync::Arc::make_mut(&mut s.doc_mut().unwrap().wb);
        let large = wb.styles.derive(StyleId::DEFAULT, |st| st.font.size = 36.0);
        let sh = wb.sheet_mut(0).unwrap();
        // Row 4, inside the chart's rows 2..6: text and style, without the fit an edit would run.
        sh.set_value(CellRef::new(3, 1), Value::text("tall"));
        sh.set_style(CellRef::new(3, 1), large);
        let saved_height = sh.row_height(3);
        let bytes = crate::io::save_bytes(wb, "chart.xlsx").unwrap();
        let markers = first_anchor_markers(&bytes);
        assert!(markers.starts_with(&format!("<xdr:twoCellAnchor{edit_as}>")), "{markers}");

        let (opened, _) = crate::io::open_bytes("chart.xlsx", &bytes).unwrap();
        let sh = opened.sheet(0).unwrap();
        assert!(sh.row_height(3) > saved_height + 1.0, "{mode}: the open fits the tall row");
        let px = |emu: i64| emu as f64 / EMU_PER_PX;
        let corner = |marker| {
            let x = sh.col_left(marker_number(&markers, marker, "col") as u32) + px(marker_number(&markers, marker, "colOff"));
            let y = sh.row_top(marker_number(&markers, marker, "row") as u32) + px(marker_number(&markers, marker, "rowOff"));
            (x, y)
        };
        let ((x1, y1), (x2, y2)) = (corner("from"), corner("to"));
        let chart = &sh.charts[0];
        assert_eq!(Some(chart.anchor.mode), crate::cmd::parse_anchor_mode(mode));
        assert_rect(sh.anchor_rect(&chart.anchor), ObjectRect { x: x1, y: y1, w: x2 - x1, h: y2 - y1 }, mode);

        let resaved = crate::io::save_bytes(&opened, "chart.xlsx").unwrap();
        assert_eq!(first_anchor_markers(&resaved), markers, "{mode}: unedited markers save unchanged");
    }
}

/// Excel's Format Shape ▸ Properties: resizing a row or column inside a chart sizes a "move and
/// size with cells" chart only; resizing one before it moves both kinds that move with cells;
/// "don't move or size with cells" stays where it is either way. Undo puts each back.
#[test]
fn resized_rows_and_columns_move_and_size_charts_by_anchor_mode() {
    for mode in ["moveAndSize", "moveOnly", "absolute"] {
        // (command, line key, line, inside the chart), lines as A1 ranges and indices.
        for (command, key, lines, line, inside) in [
            ("home.rowHeight", "rows", "4:4", 3, true),
            ("home.rowHeight", "rows", "1:1", 0, false),
            ("home.columnWidth", "cols", "D:D", 3, true),
            ("home.columnWidth", "cols", "A:A", 0, false),
        ] {
            let mut s = chart_session(mode);
            let before = chart_rect(&s);
            let rows = key == "rows";
            let size = |s: &Session| {
                let sh = s.doc().unwrap().wb.sheet(0).unwrap();
                f64::from(if rows { sh.row_height(line) } else { sh.col_width(line) })
            };
            let old = size(&s);
            let param = if rows { "height" } else { "width" };
            let mut params = serde_json::Map::new();
            params.insert(key.into(), json!(lines));
            params.insert(param.into(), json!(old + 30.0));
            s.execute(command, params.into()).unwrap();
            let grown = size(&s) - old;
            assert!(grown > 20.0, "{command} {lines} grows the line");
            let mut want = before;
            match (mode, inside) {
                ("moveAndSize", true) if rows => want.h += grown,
                ("moveAndSize", true) => want.w += grown,
                ("absolute", _) | (_, true) => {}
                (_, false) if rows => want.y += grown,
                (_, false) => want.x += grown,
            }
            assert_rect(chart_rect(&s), want, &format!("{mode} after {command} {lines}"));
            s.execute("edit.undo", json!({})).unwrap();
            assert_rect(chart_rect(&s), before, &format!("{mode} after undoing {command} {lines}"));
        }
    }
}

/// Hiding a row under a chart leaves the chart's size alone, as it did before charts followed
/// resized rows: only the size of a collapsed line, not where the chart's corner sat in it,
/// would be known when it is shown again.
#[test]
fn hiding_a_row_inside_a_chart_keeps_its_size() {
    let mut s = chart_session("moveAndSize");
    let before = chart_rect(&s);
    s.execute("home.rowHeight", json!({"rows": "4:4", "height": 0})).unwrap();
    assert_rect(chart_rect(&s), before, "hidden row");
}

/// Round-trip a synthetic chart whose end marker cannot be reconstructed by hit-testing
/// its drawn corner. The file explicitly says which row owns the offset.
fn chart_with_named_end_row(to_row: u32, to_off: i64, zero_rows: &[u32]) -> Session {
    use gridcraft_model::{DrawingFile, DrawingMarker, DrawingPlacement, LineInfo};
    let s = chart_session("moveAndSize");
    let mut wb = s.doc().unwrap().wb.as_ref().clone();
    let sh = wb.sheet_mut(0).unwrap();
    for &row in zero_rows {
        sh.rows.insert(row, LineInfo { size: Some(0.0), custom: true, ..LineInfo::default() });
    }
    let from = DrawingMarker { col: 2, row: 1, col_off: 0, row_off: 0 };
    let to = DrawingMarker { col: 5, row: to_row, col_off: 0, row_off: to_off };
    let placement = DrawingPlacement::TwoCell { from, to, edit_as: None };
    let anchor = placement.anchor(sh);
    sh.charts[0].anchor = anchor;
    sh.drawing_files.insert(DrawingRef::Chart(sh.charts[0].id), DrawingFile { anchor, placement });
    let bytes = crate::io::save_bytes(&wb, "markers.xlsx").unwrap();
    let mut session = Session::new();
    session.execute("file.open", json!({"name": "markers.xlsx", "base64": crate::io::base64_encode(&bytes)})).unwrap();
    session
}

#[test]
fn chart_resize_uses_the_named_end_row_even_with_an_out_of_cell_offset() {
    let mut s = chart_with_named_end_row(3, 50 * 9525, &[]);
    let before = chart_rect(&s);
    s.execute("home.rowHeight", json!({"rows": "4:4", "points": 40})).unwrap();
    assert_rect(chart_rect(&s), before, "end marker belongs to row 4, not one of the later rows the offset reaches");
}

#[test]
fn chart_resize_can_distinguish_coincident_markers_in_zero_height_rows() {
    let mut s = chart_with_named_end_row(3, 0, &[3, 4]);
    let before = chart_rect(&s);
    let sh = s.doc().unwrap().wb.sheet(0).unwrap();
    assert_eq!(sh.row_top(3), sh.row_top(5));
    assert!(!sh.is_row_hidden(3));
    s.execute("home.rowHeight", json!({"rows": "4:4", "points": 40})).unwrap();
    assert_rect(chart_rect(&s), before, "the marker stays at the start of row 4");
}

#[test]
fn resizing_a_row_keeps_an_absolute_chart_beyond_ten_million_pixels_fixed() {
    let mut s = chart_session("absolute");
    let id = s.doc().unwrap().wb.sheet(0).unwrap().charts[0].id;
    s.execute("object.move", json!({"kind": "chart", "id": id, "at": "C700001"})).unwrap();
    let before = chart_rect(&s);
    assert!(before.y > 1e7);
    s.execute("home.rowHeight", json!({"rows": "1:1", "points": 40})).unwrap();
    assert_rect(chart_rect(&s), before, "absolute position on a valid late worksheet row");
}

/// A row fitted to a larger font inside a "move and size with cells" chart grows the chart.
#[test]
fn a_row_fit_inside_a_chart_sizes_it() {
    let mut s = chart_session("moveAndSize");
    let before = chart_rect(&s);
    s.execute("cell.set", json!({"cell": "B4", "input": "tall"})).unwrap();
    let old = s.doc().unwrap().wb.sheet(0).unwrap().row_height(3);
    s.execute("home.fontSize", json!({"range": "B4", "size": 36})).unwrap();
    let grown = f64::from(s.doc().unwrap().wb.sheet(0).unwrap().row_height(3) - old);
    assert!(grown > 20.0);
    assert_rect(chart_rect(&s), ObjectRect { h: before.h + grown, ..before }, "fitted row");
}

/// A sort that leaves a chart's anchor as it was still placed it, so the sorted row heights
/// don't move it a second time. Rows 1 and 3 trade places (and heights) around the chart's top
/// row, so the chart rises with row 2; its bottom lies past the sorted rows, so it keeps its size.
#[test]
fn a_sort_that_keeps_a_charts_anchor_does_not_follow_the_sorted_rows_again() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [[3], [2], [1]]})).unwrap();
    s.execute("home.rowHeight", json!({"rows": "1:1", "points": 37.5})).unwrap();
    s.execute("selection.set", json!({"range": "A1:A3"})).unwrap();
    s.execute("insert.chart", json!({"type": "column", "at": "B2", "width": 200, "height": 100, "mode": "moveAndSize"})).unwrap();
    let (anchor, before) = (s.doc().unwrap().wb.sheet(0).unwrap().charts[0].anchor, chart_rect(&s));
    s.execute("data.sort", json!({"range": "A1:D3", "header": false, "keys": [{"column": "A"}]})).unwrap();
    let sh = s.doc().unwrap().wb.sheet(0).unwrap();
    assert_eq!(sh.charts[0].anchor, anchor, "the sort keeps the chart's anchor");
    let raised = f64::from(sh.row_height(2) - sh.row_height(0));
    assert!(raised > 20.0, "row 1's height went to row 3");
    assert_rect(chart_rect(&s), ObjectRect { y: before.y - raised, ..before }, "sorted once");
}

/// A chart the sort moves still follows the row fit after the sort: a cell's 36 pt text, never
/// fitted, sorts into the middle of the chart's new rows.
#[test]
fn a_chart_the_sort_moves_still_follows_the_row_fit_after_it() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [[4], [5], [6], [1], [2], [3]]})).unwrap();
    let rh = f64::from(s.doc().unwrap().wb.sheet(0).unwrap().row_height(0));
    s.execute("selection.set", json!({"range": "A1:A6"})).unwrap();
    s.execute("insert.chart", json!({"type": "column", "at": "B1", "width": 200, "height": 2.0 * rh + 10.0, "mode": "moveAndSize"})).unwrap();
    {
        let wb = std::sync::Arc::make_mut(&mut s.doc_mut().unwrap().wb);
        let large = wb.styles.derive(StyleId::DEFAULT, |st| st.font.size = 36.0);
        wb.sheet_mut(0).unwrap().set_style(CellRef::new(1, 0), large);
    }
    s.execute("data.sort", json!({"range": "A1:D6", "header": false, "keys": [{"column": "A"}]})).unwrap();
    let sh = s.doc().unwrap().wb.sheet(0).unwrap();
    assert!(f64::from(sh.row_height(4)) > rh + 20.0, "the 36 pt row fits after the sort");
    let (top, bottom) = (sh.row_top(3), sh.row_top(5) + 10.0);
    let r = chart_rect(&s);
    assert!((r.y - top).abs() < 1e-3 && (r.y + r.h - bottom).abs() < 1e-3, "chart {r:?} spans rows 4 to 6 over the fitted rows ({top}, {bottom})");
}

#[test]
fn a_sort_that_keeps_the_editable_anchor_does_not_reuse_stale_markers_during_fit() {
    let mut s = chart_with_named_end_row(3, 50 * 9525, &[]);
    s.execute("range.setValues", json!({"range": "A1", "values": [[3], [2], [1]]})).unwrap();
    s.execute("home.rowHeight", json!({"rows": "1:1", "points": 37.5})).unwrap();
    {
        let wb = std::sync::Arc::make_mut(&mut s.doc_mut().unwrap().wb);
        let large = wb.styles.derive(StyleId::DEFAULT, |st| st.font.size = 36.0);
        let sh = wb.sheet_mut(0).unwrap();
        sh.set_style(CellRef::new(2, 0), large);
        sh.rows.entry(2).or_default().size = Some(sh.default_row_height);
    }
    let before = chart_rect(&s);
    let sh = s.doc().unwrap().wb.sheet(0).unwrap();
    assert!(sh.drawing_files.contains_key(&DrawingRef::Chart(sh.charts[0].id)));
    s.execute("data.sort", json!({"range": "A1:D3", "header": false, "keys": [{"column": "A"}]})).unwrap();
    let sh = s.doc().unwrap().wb.sheet(0).unwrap();
    assert!(sh.row_height(0) > sh.default_row_height + 20.0, "the tall content sorted into row 1 fits there");
    assert_rect(chart_rect(&s), ObjectRect { y: sh.row_top(1), ..before }, "the fit above the chart only moves it");
}

/// Saved markers that still describe a sorted chart on the sorted grid carry into the row fit
/// after the sort. Rows 3 and 4 (all of standard height) trade places, carrying unfitted 36 pt
/// text into row 4. The end marker names row 4, its 50 px offset reaching past that row, so the
/// chart keeps its height when row 4 grows to fit (the drawn corner alone would put the end in
/// row 6 and grow the chart).
#[test]
fn a_sort_keeps_valid_named_markers_for_the_row_fit() {
    let mut s = chart_with_named_end_row(3, 50 * 9525, &[]);
    s.execute("range.setValues", json!({"range": "A3", "values": [[4], [3]]})).unwrap();
    {
        let wb = std::sync::Arc::make_mut(&mut s.doc_mut().unwrap().wb);
        let large = wb.styles.derive(StyleId::DEFAULT, |st| st.font.size = 36.0);
        wb.sheet_mut(0).unwrap().set_style(CellRef::new(2, 0), large);
    }
    let before = chart_rect(&s);
    let old = s.doc().unwrap().wb.sheet(0).unwrap().row_height(3);
    s.execute("data.sort", json!({"range": "A1:D4", "header": false, "keys": [{"column": "A"}]})).unwrap();
    assert!(s.doc().unwrap().wb.sheet(0).unwrap().row_height(3) > old + 20.0, "row 4 fits after the sort");
    assert_rect(chart_rect(&s), before, "the end marker stays 50 px into row 4");
}

/// Synthetic version of the Batch 4 grouped-shape probe: one group straddles A5:D10
/// (members starting in B1 and B6), and the other lies fully inside it (B8 and B9).
fn group_sort_session() -> Session {
    let mut s = s();
    s.execute(
        "range.setValues",
        json!({"range": "A5", "values": [[6, 16, 26, 36], [5, 15, 25, 35], [4, 14, 24, 34], [3, 13, 23, 33], [2, 12, 22, 32], [1, 11, 21, 31]]}),
    )
    .unwrap();
    let sh = s.doc().unwrap().wb.sheet(0).unwrap();
    let (row, width) = (sh.row_height(0), sh.col_left(4) - sh.col_left(1));
    for (cells, rows) in [(["B1", "B6"], 2.0), (["B8", "B9"], 1.0)] {
        let mut members = Vec::new();
        for at in cells {
            let shape = s
                .execute("insert.shape", json!({"kind": "rectangle", "at": at, "width": width, "height": rows * row, "mode": "moveAndSize"}))
                .unwrap();
            members.push(DrawingRef::Shape(shape["shape"].as_u64().unwrap() as u32));
        }
        let wb = std::sync::Arc::make_mut(&mut s.doc_mut().unwrap().wb);
        let id = wb.next_object_id();
        let sh = wb.sheet_mut(0).unwrap();
        let first = sh.drawing_rect(members[0]).unwrap();
        let last = sh.drawing_rect(members[1]).unwrap();
        let rect = ObjectRect { h: last.y + last.h - first.y, ..first };
        sh.groups.push(gridcraft_model::DrawingGroup {
            id,
            name: format!("Group {id}"),
            anchor: sh.rect_anchor(rect, AnchorMode::MoveAndSize),
            child_offset: (0, 0),
            child_extent: ((rect.w * EMU_PER_PX) as i64, (rect.h * EMU_PER_PX) as i64),
            flip: (false, false),
            rotation: 0,
            member_rotations: Vec::new(),
            members,
            file: None,
        });
    }
    let bytes = crate::io::save_bytes(&s.doc().unwrap().wb, "groups.xlsx").unwrap();
    s.execute("file.open", json!({"name": "groups.xlsx", "base64": crate::io::base64_encode(&bytes)})).unwrap();
    s
}

fn sort_group_rows(s: &mut Session) {
    s.execute("data.sort", json!({"range": "A5:D10", "header": false, "keys": [{"column": "A"}]})).unwrap();
}

fn assert_group_sort_rows(sh: &gridcraft_model::Sheet, sorted: bool) {
    for row in 4..10 {
        let key = if sorted { row - 3 } else { 10 - row };
        for col in 0..4 {
            assert_eq!(sh.value(CellRef::new(row, col)), Value::Number(f64::from(key + 10 * col)));
        }
    }
}

fn assert_group_boxes(sh: &gridcraft_model::Sheet, before: &gridcraft_model::Sheet, index: usize) {
    assert_eq!(sh.groups.len(), 2);
    let (group, old) = (&sh.groups[index], &before.groups[index]);
    assert_eq!(group.name, old.name);
    assert_eq!(group.anchor.mode, AnchorMode::MoveAndSize);
    assert_rect(sh.anchor_rect(&group.anchor), before.anchor_rect(&old.anchor), "stored group box");
    assert_rect(sh.drawing_rect(DrawingRef::Group(group.id)).unwrap(), before.drawing_rect(DrawingRef::Group(old.id)).unwrap(), "group box");
    assert_eq!(group.members.len(), old.members.len());
    for (&member, &old_member) in group.members.iter().zip(&old.members) {
        assert_rect(sh.drawing_rect(member).unwrap(), before.drawing_rect(old_member).unwrap(), "group member");
    }
}

#[test]
fn a_sort_keeps_group_boxes_and_members_in_place_through_undo_redo() {
    let mut s = group_sort_session();
    let before = s.doc().unwrap().wb.sheets[0].clone();
    assert_group_sort_rows(&before, false);
    sort_group_rows(&mut s);
    for (command, sorted) in [(None, true), (Some("edit.undo"), false), (Some("edit.redo"), true)] {
        if let Some(command) = command {
            s.execute(command, json!({})).unwrap();
        }
        let sh = s.doc().unwrap().wb.sheet(0).unwrap();
        assert_group_sort_rows(sh, sorted);
        for index in 0..2 {
            assert_group_boxes(sh, &before, index);
        }
    }
}

#[test]
fn a_sort_keeps_group_boxes_and_members_in_place_through_xlsx_save() {
    let mut s = group_sort_session();
    let before = s.doc().unwrap().wb.sheets[0].clone();
    sort_group_rows(&mut s);
    let bytes = crate::io::save_bytes(&s.doc().unwrap().wb, "groups.xlsx").unwrap();
    let (reopened, _) = crate::io::open_bytes("groups.xlsx", &bytes).unwrap();
    let sh = reopened.sheet(0).unwrap();
    assert_group_sort_rows(sh, true);
    for index in (0..2).rev() {
        assert_group_boxes(sh, &before, index);
        let (new_ref, old_ref) = (DrawingRef::Group(sh.groups[index].id), DrawingRef::Group(before.groups[index].id));
        assert_eq!(sh.drawing_files[&new_ref].placement, before.drawing_files[&old_ref].placement);
    }
}

#[test]
fn a_sort_keeps_groups_in_place_before_the_separate_row_fit() {
    let mut s = group_sort_session();
    s.execute("home.rowHeight", json!({"rows": "5:5", "points": 30})).unwrap();
    {
        let wb = std::sync::Arc::make_mut(&mut s.doc_mut().unwrap().wb);
        let large = wb.styles.derive(StyleId::DEFAULT, |st| st.font.size = 36.0);
        wb.sheet_mut(0).unwrap().set_style(CellRef::new(6, 0), large);
    }
    let before = s.doc().unwrap().wb.sheets[0].clone();
    let group_box = |sh: &gridcraft_model::Sheet, index: usize| sh.drawing_rect(DrawingRef::Group(sh.groups[index].id)).unwrap();
    sort_group_rows(&mut s);
    let sh = s.doc().unwrap().wb.sheet(0).unwrap();
    assert_eq!(sh.row_height(9), before.row_height(4), "row 5's height goes to row 10");
    let grown = f64::from(sh.row_height(7) - before.row_height(6));
    assert!(grown > 20.0, "the unfitted 36 pt cell sorts into row 8 and fits");
    let (straddle, inside) = (group_box(&before, 0), group_box(&before, 1));
    assert_rect(group_box(sh, 0), ObjectRect { h: straddle.h + grown, ..straddle }, "row 8 fits inside the straddling group");
    assert_rect(group_box(sh, 1), ObjectRect { y: inside.y + grown, ..inside }, "row 8 fits above the inside group");
}

/// Rows of different heights trade places under both groups: each member keeps its box on the
/// sheet, re-anchored to the rows now under it, through save/reopen and undo.
#[test]
fn a_sort_over_unequal_rows_keeps_groups_in_place_through_save_and_undo() {
    let mut s = group_sort_session();
    s.execute("home.rowHeight", json!({"rows": "5:5", "points": 30})).unwrap();
    s.execute("home.rowHeight", json!({"rows": "9:9", "points": 24.75})).unwrap();
    let before = s.doc().unwrap().wb.sheets[0].clone();
    sort_group_rows(&mut s);
    let sh = s.doc().unwrap().wb.sheet(0).unwrap();
    assert_ne!(sh.row_height(4), before.row_height(4), "the sort moved row heights");
    let bytes = crate::io::save_bytes(&s.doc().unwrap().wb, "groups.xlsx").unwrap();
    let (reopened, _) = crate::io::open_bytes("groups.xlsx", &bytes).unwrap();
    for index in 0..2 {
        assert_group_boxes(s.doc().unwrap().wb.sheet(0).unwrap(), &before, index);
        assert_group_boxes(reopened.sheet(0).unwrap(), &before, index);
    }
    s.execute("edit.undo", json!({})).unwrap();
    let sh = s.doc().unwrap().wb.sheet(0).unwrap();
    for (group, old) in sh.groups.iter().zip(&before.groups) {
        assert_eq!(group.anchor, old.anchor, "undo restores the group's anchor");
    }
}

#[test]
fn a_sort_fit_discards_file_markers_for_a_group_beside_the_block() {
    let mut s = group_sort_session();
    {
        let wb = std::sync::Arc::make_mut(&mut s.doc_mut().unwrap().wb);
        let large = wb.styles.derive(StyleId::DEFAULT, |st| st.font.size = 36.0);
        wb.sheet_mut(0).unwrap().set_style(CellRef::new(6, 0), large);
    }
    let before = s.doc().unwrap().wb.sheets[0].clone();
    let group = DrawingRef::Group(before.groups[1].id);
    assert!(before.drawing_files.contains_key(&group));
    let rect = before.drawing_rect(group).unwrap();
    // The groups start in column B, exactly on the excluded right edge of this sort.
    s.execute("data.sort", json!({"range": "A5:A10", "header": false, "keys": [{"column": "A"}]})).unwrap();
    let sh = s.doc().unwrap().wb.sheet(0).unwrap();
    let growth = f64::from(sh.row_height(7) - before.row_height(7));
    assert!(growth > 20.0);
    assert_rect(sh.drawing_rect(group).unwrap(), ObjectRect { h: rect.h + growth, ..rect }, "the group follows the row fit");
    assert!(!sh.drawing_files.contains_key(&group));
}

/// Only groups touching the sorted block keep their place on the sheet. A group beside it, or
/// below it once a hidden row inside it takes another height, follows its moved rows like the
/// ungrouped picture next to it.
#[test]
fn a_sort_leaves_groups_clear_of_the_block_to_follow_their_rows() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A5", "values": [[6], [5], [4], [3], [2], [1]]})).unwrap();
    let mut pictures = |cells: [&str; 3]| {
        let [a, b, lone] = cells.map(|cell| DrawingRef::Image(add_image(&mut s, cell, AnchorMode::MoveAndSize)));
        let wb = std::sync::Arc::make_mut(&mut s.doc_mut().unwrap().wb);
        let id = wb.next_object_id();
        let sh = wb.sheet_mut(0).unwrap();
        let (first, last) = (sh.drawing_rect(a).unwrap(), sh.drawing_rect(b).unwrap());
        let rect = ObjectRect { w: last.x + last.w - first.x, ..first };
        sh.groups.push(gridcraft_model::DrawingGroup {
            id,
            name: format!("Group {id}"),
            anchor: sh.rect_anchor(rect, AnchorMode::MoveAndSize),
            child_offset: (0, 0),
            child_extent: ((rect.w * EMU_PER_PX) as i64, (rect.h * EMU_PER_PX) as i64),
            flip: (false, false),
            rotation: 0,
            member_rotations: Vec::new(),
            members: vec![a, b],
            file: None,
        });
        [a, b, lone]
    };
    let beside = pictures(["H10", "J10", "L10"]);
    let below = pictures(["H15", "J15", "L15"]);
    {
        let wb = std::sync::Arc::make_mut(&mut s.doc_mut().unwrap().wb);
        let sh = wb.sheet_mut(0).unwrap();
        sh.rows.insert(5, gridcraft_model::LineInfo { size: Some(20.0), hidden: true, ..Default::default() });
        sh.rows.insert(8, gridcraft_model::LineInfo { size: Some(60.0), custom: true, ..Default::default() });
    }
    let before = s.doc().unwrap().wb.sheets[0].clone();
    s.execute("data.sort", json!({"range": "A5:A10", "header": false, "keys": [{"column": "A"}]})).unwrap();
    let sh = s.doc().unwrap().wb.sheet(0).unwrap();
    for [a, b, lone] in [beside, below] {
        let (was, now) = (before.drawing_rect(lone).unwrap(), sh.drawing_rect(lone).unwrap());
        assert!((was.y - now.y).abs() > 20.0, "the picture's rows moved: {was:?} -> {now:?}");
        for member in [a, b] {
            let (old, got) = (before.drawing_rect(member).unwrap(), sh.drawing_rect(member).unwrap());
            assert_rect(got, ObjectRect { y: now.y, h: now.h, ..old }, "grouped picture follows its rows");
        }
    }
}

/// The row fit after a deletion re-measures the chart against the remapped rows.
#[test]
fn a_row_fit_after_a_deletion_sizes_the_shifted_chart() {
    let mut s = chart_session("moveAndSize");
    // B5, inside the chart's rows 2..6, refers to A1: deleting row 1 rewrites it, and the
    // rewrite fits its row to the 36 pt font given it without a fit.
    s.execute("cell.set", json!({"cell": "B5", "input": "=A1"})).unwrap();
    {
        let wb = std::sync::Arc::make_mut(&mut s.doc_mut().unwrap().wb);
        let large = wb.styles.derive(StyleId::DEFAULT, |st| st.font.size = 36.0);
        wb.sheet_mut(0).unwrap().set_style(CellRef::new(4, 1), large);
    }
    let before = chart_rect(&s);
    let height = |s: &Session, row| f64::from(s.doc().unwrap().wb.sheet(0).unwrap().row_height(row));
    let (deleted, unfitted) = (height(&s, 0), height(&s, 4));
    s.execute("home.deleteRows", json!({"rows": "1:1"})).unwrap();
    assert_eq!(s.doc().unwrap().wb.sheet(0).unwrap().charts[0].anchor.cell.row, 0);
    let grown = height(&s, 3) - unfitted;
    assert!(grown > 20.0, "the rewritten formula's row fits");
    assert_rect(chart_rect(&s), ObjectRect { y: before.y - deleted, h: before.h + grown, ..before }, "after the deletion");
}

#[test]
fn a_row_fit_after_deletion_uses_the_remapped_named_end_row() {
    let mut s = chart_with_named_end_row(3, 50 * 9525, &[]);
    s.execute("cell.set", json!({"cell": "B4", "input": "=A1"})).unwrap();
    {
        let wb = std::sync::Arc::make_mut(&mut s.doc_mut().unwrap().wb);
        let large = wb.styles.derive(StyleId::DEFAULT, |st| st.font.size = 36.0);
        wb.sheet_mut(0).unwrap().set_style(CellRef::new(3, 1), large);
    }
    let height = |s: &Session, row| f64::from(s.doc().unwrap().wb.sheet(0).unwrap().row_height(row));
    let before = chart_rect(&s);
    let (deleted, unfitted) = (height(&s, 0), height(&s, 3));
    s.execute("home.deleteRows", json!({"rows": "1:1"})).unwrap();
    assert!(height(&s, 2) > unfitted + 20.0, "the rewritten formula's row fits");
    assert_rect(chart_rect(&s), ObjectRect { y: before.y - deleted, ..before }, "the fitted row starts at the named end marker");
}

/// Two 40 px pictures at B3 and B6 grouped with `mode`, the group's box around them, and row 4
/// (between them) grown by 30 px. Returns the group's and the members' boxes before and after.
fn resize_inside_group(mode: AnchorMode) -> ([ObjectRect; 3], [ObjectRect; 3], f64) {
    let mut s = s();
    let top = DrawingRef::Image(add_image(&mut s, "B3", mode));
    let bottom = DrawingRef::Image(add_image(&mut s, "B6", mode));
    let id = group_objects(&mut s, vec![top, bottom]);
    {
        let wb = std::sync::Arc::make_mut(&mut s.doc_mut().unwrap().wb);
        let sh = wb.sheet_mut(0).unwrap();
        let anchor = sh.group_anchor(id).unwrap();
        sh.groups.iter_mut().find(|g| g.id == id).unwrap().anchor = anchor;
    }
    let boxes = |s: &Session| {
        let sh = s.doc().unwrap().wb.sheet(0).unwrap();
        [sh.anchor_rect(&sh.drawing_group(id).unwrap().anchor), sh.drawing_rect(top).unwrap(), sh.drawing_rect(bottom).unwrap()]
    };
    let before = boxes(&s);
    let old = f64::from(s.doc().unwrap().wb.sheet(0).unwrap().row_height(3));
    s.execute("home.rowHeight", json!({"rows": "4:4", "height": old + 30.0})).unwrap();
    let grown = f64::from(s.doc().unwrap().wb.sheet(0).unwrap().row_height(3)) - old;
    assert!(grown > 20.0);
    (before, boxes(&s), grown)
}

/// A group sizes with its cells as one object: its members scale with it.
#[test]
fn a_resized_row_inside_a_group_sizes_the_group_and_its_members() {
    let ([before, ..], [after, t, b], grown) = resize_inside_group(AnchorMode::MoveAndSize);
    assert_rect(after, ObjectRect { h: before.h + grown, ..before }, "group");
    assert!((t.y - after.y).abs() < 1e-3 && ((b.y + b.h) - (after.y + after.h)).abs() < 1e-3, "members span the group: {t:?} {b:?} {after:?}");
    let scale = after.h / before.h;
    assert!((t.h - 40.0 * scale).abs() < 1e-3 && (b.h - 40.0 * scale).abs() < 1e-3, "members scale with the group");
}

/// A "move but don't size" group keeps its size over a grown row, and its members keep their
/// places in it, though each is anchored to a cell of its own.
#[test]
fn a_resized_row_inside_a_move_only_group_keeps_the_group_and_its_members() {
    let (before, after, _) = resize_inside_group(AnchorMode::MoveOnly);
    for (what, (got, want)) in ["group", "top member", "bottom member"].into_iter().zip(after.into_iter().zip(before)) {
        assert_rect(got, want, what);
    }
}
