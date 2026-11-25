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
