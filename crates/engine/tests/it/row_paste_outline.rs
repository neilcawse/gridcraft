//! Whole-line Copy/Cut carries outline levels and collapsed-summary markers (X44), as measured in
//! Excel for Mac 16.115.2 by the synthetic action probes `x33-*` and the Batch 5 re-runs of the
//! `x25-*` cases. Each table below is the probe's object-model readback, row by row.

use std::sync::Arc;

use gridcraft_engine::Session;
use gridcraft_engine::core::{CellRef, Value};
use gridcraft_engine::model::style::{Style, StyleId};
use gridcraft_engine::model::{LineInfo, Sheet};
use serde_json::{Value as Json, json};

fn at(a1: &str) -> CellRef {
    CellRef::parse(a1).unwrap()
}

fn col(letter: &str) -> u32 {
    at(&format!("{letter}1")).col
}

fn sheet<'a>(s: &'a Session, name: &str) -> &'a Sheet {
    let wb = &s.doc().unwrap().wb;
    wb.sheet(wb.sheet_index(name).unwrap()).unwrap()
}

/// Excel's row readback: (OutlineLevel, Hidden, RowHeight in points, ShowDetail). OutlineLevel
/// counts from 1 where the file's `outlineLevel` counts from 0; GridCraft models ShowDetail
/// false with a collapsed-summary marker.
fn row_readback(sh: &Sheet, row: u32) -> (u8, bool, f32, bool) {
    let info = sh.rows.get(&(row - 1)).copied().unwrap_or_default();
    (info.outline + 1, info.hidden, sh.row_height(row - 1) * 0.75, !info.collapsed)
}

/// The column readback without width: (OutlineLevel, Hidden, zero width, ShowDetail).
fn col_readback(sh: &Sheet, letter: &str) -> (u8, bool, bool, bool) {
    let info = sh.cols.get(&col(letter)).copied().unwrap_or_default();
    (info.outline + 1, info.hidden, sh.col_width(col(letter)) == 0., !info.collapsed)
}

fn extent(s: &mut Session, name: &str) -> (Json, Json) {
    s.execute("sheet.activate", json!({"sheet": name})).unwrap();
    let inspect = s.execute("document.inspect", json!({})).unwrap();
    let index = s.doc().unwrap().wb.sheet_index(name).unwrap();
    let stats = s.execute("review.workbookStatistics", json!({})).unwrap();
    (inspect["sheets"][index]["usedRange"].clone(), stats["sheet"]["endOfSheet"].clone())
}

fn reopen(s: &mut Session) -> Session {
    let saved = s.execute("file.saveBytes", json!({"format": "xlsx"})).unwrap();
    let mut reopened = Session::new();
    reopened.execute("file.open", json!({"name": "outline.xlsx", "base64": saved["base64"]})).unwrap();
    reopened
}

// ------------------------------------------------------------------ x33

/// The `x33-*` fixture: sheet S, rows 3:5 at outline level 2 and hidden, collapsed under summary
/// row 6; ungrouped rows 20:23; rows 30:33 at level 2 and expanded under summary row 34.
fn x33_fixture() -> Session {
    let mut s = Session::new();
    s.new_workbook();
    s.execute("sheet.rename", json!({"name": "S"})).unwrap();
    let wb = Arc::make_mut(&mut s.doc_mut().unwrap().wb);
    let sh = wb.sheet_mut(0).unwrap();
    for (cell, text) in [("A3", "S-A3"), ("A4", "S-A4"), ("A5", "S-A5"), ("A6", "S-A6 summary"), ("A34", "D-A34 summary")] {
        sh.set_value(at(cell), Value::text(text));
    }
    for row in (20..=23).chain(30..=33) {
        sh.set_value(CellRef::new(row - 1, 0), Value::text(format!("D-A{row}")));
    }
    for row in 3..=5 {
        sh.rows.insert(row - 1, LineInfo { outline: 1, hidden: true, ..Default::default() });
    }
    sh.rows.insert(5, LineInfo { collapsed: true, ..Default::default() });
    for row in 30..=33 {
        sh.rows.insert(row - 1, LineInfo { outline: 1, ..Default::default() });
    }
    s
}

type RowReadback = (u32, u8, bool, f32, bool);

const X33_BEFORE: &[RowReadback] = &[
    (2, 1, false, 15., true),
    (3, 2, true, 0., true),
    (4, 2, true, 0., true),
    (5, 2, true, 0., true),
    (6, 1, false, 15., false),
    (7, 1, false, 15., true),
    (19, 1, false, 15., true),
    (20, 1, false, 15., true),
    (21, 1, false, 15., true),
    (22, 1, false, 15., true),
    (23, 1, false, 15., true),
    (24, 1, false, 15., true),
    (29, 1, false, 15., true),
    (30, 2, false, 15., true),
    (31, 2, false, 15., true),
    (32, 2, false, 15., true),
    (33, 2, false, 15., true),
    (34, 1, false, 15., true),
    (35, 1, false, 15., true),
];

/// Excel's after-readbacks: rows 2-7, 19-24 and 29-35 of each probe.
const X33_CASES: &[(&str, &str, &str, &[RowReadback], &str)] = &[
    (
        "x33-copy-rows-to-flat",
        "edit.copy",
        "20:23",
        &[
            (2, 1, false, 15., true),
            (3, 2, true, 0., true),
            (4, 2, true, 0., true),
            (5, 2, true, 0., true),
            (6, 1, false, 15., false),
            (7, 1, false, 15., true),
            (19, 1, false, 15., true),
            (20, 2, true, 0., true),
            (21, 2, true, 0., true),
            (22, 2, true, 0., true),
            (23, 1, false, 15., false),
            (24, 1, false, 15., true),
            (29, 1, false, 15., true),
            (30, 2, false, 15., true),
            (31, 2, false, 15., true),
            (32, 2, false, 15., true),
            (33, 2, false, 15., true),
            (34, 1, false, 15., true),
            (35, 1, false, 15., true),
        ],
        "A3:A34",
    ),
    (
        "x33-copy-rows-to-group",
        "edit.copy",
        "30:33",
        &[
            (2, 1, false, 15., true),
            (3, 2, true, 0., true),
            (4, 2, true, 0., true),
            (5, 2, true, 0., true),
            (6, 1, false, 15., false),
            (7, 1, false, 15., true),
            (19, 1, false, 15., true),
            (20, 1, false, 15., true),
            (21, 1, false, 15., true),
            (22, 1, false, 15., true),
            (23, 1, false, 15., true),
            (24, 1, false, 15., true),
            (29, 1, false, 15., true),
            (30, 2, true, 0., true),
            (31, 2, true, 0., true),
            (32, 2, true, 0., true),
            (33, 1, false, 15., false),
            (34, 1, false, 15., true),
            (35, 1, false, 15., true),
        ],
        "A3:A34",
    ),
    (
        "x33-cut-rows-to-flat",
        "edit.cut",
        "20:23",
        &[
            (2, 1, false, 15., true),
            (3, 1, false, 15., true),
            (4, 1, false, 15., true),
            (5, 1, false, 15., true),
            (6, 1, false, 15., true),
            (7, 1, false, 15., true),
            (19, 1, false, 15., true),
            (20, 2, true, 0., true),
            (21, 2, true, 0., true),
            (22, 2, true, 0., true),
            (23, 1, false, 15., false),
            (24, 1, false, 15., true),
            (29, 1, false, 15., true),
            (30, 2, false, 15., true),
            (31, 2, false, 15., true),
            (32, 2, false, 15., true),
            (33, 2, false, 15., true),
            (34, 1, false, 15., true),
            (35, 1, false, 15., true),
        ],
        "A20:A34",
    ),
    (
        "x33-cut-rows-to-group",
        "edit.cut",
        "30:33",
        &[
            (2, 1, false, 15., true),
            (3, 1, false, 15., true),
            (4, 1, false, 15., true),
            (5, 1, false, 15., true),
            (6, 1, false, 15., true),
            (7, 1, false, 15., true),
            (19, 1, false, 15., true),
            (20, 1, false, 15., true),
            (21, 1, false, 15., true),
            (22, 1, false, 15., true),
            (23, 1, false, 15., true),
            (24, 1, false, 15., true),
            (29, 1, false, 15., true),
            (30, 2, true, 0., true),
            (31, 2, true, 0., true),
            (32, 2, true, 0., true),
            (33, 1, false, 15., false),
            (34, 1, false, 15., true),
            (35, 1, false, 15., true),
        ],
        "A20:A34",
    ),
];

fn assert_rows(s: &Session, expected: &[RowReadback], context: &str) {
    let sh = sheet(s, "S");
    for &(row, level, hidden, points, show) in expected {
        assert_eq!(row_readback(sh, row), (level, hidden, points, show), "{context}: row {row} (level, hidden, height, ShowDetail)");
    }
}

fn assert_cells(s: &Session, case: &str, cut: bool) {
    let dest = if case.ends_with("flat") { 20 } else { 30 };
    let sh = sheet(s, "S");
    for (i, text) in ["S-A3", "S-A4", "S-A5", "S-A6 summary"].into_iter().enumerate() {
        let i = i as u32;
        assert_eq!(sh.value(CellRef::new(dest - 1 + i, 0)), Value::text(text), "{case}: A{}", dest + i);
        let source = if cut { Value::Empty } else { Value::text(text) };
        assert_eq!(sh.value(CellRef::new(2 + i, 0)), source, "{case}: A{}", 3 + i);
    }
}

#[test]
fn x33_whole_row_copy_and_cut_carry_outline_levels_and_the_collapsed_summary() {
    for &(case, op, dest, after, used) in X33_CASES {
        let mut s = x33_fixture();
        let cut = op == "edit.cut";
        assert_rows(&s, X33_BEFORE, &format!("{case} before"));
        let before = sheet(&s, "S").rows.clone();
        s.execute(op, json!({"range": "3:6"})).unwrap();
        s.execute("selection.set", json!({"range": dest})).unwrap();
        s.execute("edit.paste", json!({})).unwrap();
        assert_rows(&s, after, case);
        assert_cells(&s, case, cut);
        assert_eq!(extent(&mut s, "S"), (json!(used), json!("A34")), "{case}: UsedRange / Last Cell");
        let pasted = sheet(&s, "S").rows.clone();

        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(sheet(&s, "S").rows, before, "{case}: undo restores every row record");
        assert_rows(&s, X33_BEFORE, &format!("{case} undone"));
        assert_eq!(sheet(&s, "S").value(at("A3")), Value::text("S-A3"), "{case}: undo restores A3");
        s.execute("edit.redo", json!({})).unwrap();
        assert_eq!(sheet(&s, "S").rows, pasted, "{case}: redo restores every pasted row record");
        assert_rows(&s, after, &format!("{case} redone"));

        let mut reopened = reopen(&mut s);
        assert_rows(&reopened, after, &format!("{case} saved and reopened"));
        assert_cells(&reopened, case, cut);
        assert_eq!(extent(&mut reopened, "S"), (json!(used), json!("A34")), "{case} reopened: UsedRange / Last Cell");
    }
}

// ------------------------------------------------------------------ x25

/// The `x25-*` fixture's sheets S and T. S: rows 3 (styled, hidden) and 4 (hidden, outline level
/// 2) collapsed under summary row 5, a bare row 7, columns C:D likewise under summary column E,
/// and the destination lines; T: only the destination lines (row 8 / column H at outline level 3,
/// row 9 / column I hidden).
fn x25_fixture() -> Session {
    let mut s = Session::new();
    s.new_workbook();
    s.execute("sheet.rename", json!({"name": "S"})).unwrap();
    s.execute("home.insertSheet", json!({"name": "T"})).unwrap();
    let wb = Arc::make_mut(&mut s.doc_mut().unwrap().wb);
    let mut bold = Style::default();
    bold.font.bold = true;
    let st: StyleId = wb.styles.intern(bold);
    let sized = |size: f32| LineInfo { size: Some(size), custom: true, ..Default::default() };
    let destination = |sh: &mut Sheet| {
        sh.rows.insert(7, LineInfo { style: Some(st), outline: 2, ..sized(16.) });
        sh.rows.insert(8, LineInfo { style: Some(st), hidden: true, ..sized(60.) });
        sh.cols.insert(col("H"), LineInfo { style: Some(st), outline: 2, ..sized(40.) });
        sh.cols.insert(col("I"), LineInfo { style: Some(st), hidden: true, ..sized(285.) });
    };
    let s_index = wb.sheet_index("S").unwrap();
    let t_index = wb.sheet_index("T").unwrap();
    let sh = wb.sheet_mut(s_index).unwrap();
    for c in ["A3", "B3", "A4", "B4", "A5", "A8", "A9", "C1", "D1", "H1", "I1"] {
        sh.set_value(at(c), Value::text(c));
    }
    sh.rows.insert(2, LineInfo { style: Some(st), hidden: true, ..sized(40.) });
    sh.rows.insert(3, LineInfo { hidden: true, outline: 1, ..sized(50.) });
    sh.rows.insert(4, LineInfo { collapsed: true, ..Default::default() });
    sh.rows.insert(6, LineInfo { saved_element: true, ..Default::default() });
    sh.cols.insert(col("C"), LineInfo { style: Some(st), hidden: true, ..sized(124.) });
    sh.cols.insert(col("D"), LineInfo { hidden: true, outline: 1, ..sized(166.) });
    sh.cols.insert(col("E"), LineInfo { collapsed: true, ..sized(89.) });
    destination(sh);
    let sh = wb.sheet_mut(t_index).unwrap();
    for c in ["H1", "I1", "A8", "A9"] {
        sh.set_value(at(c), Value::text(c));
    }
    destination(sh);
    s
}

/// Rows 8:10 (or columns H:J) after a source with a hidden level-1 line then a hidden level-2
/// line lands there: Excel reads ShowDetail false on the line after the pasted group.
const PASTED_GROUP: [(u8, bool, bool, bool); 3] = [(1, true, true, true), (2, true, true, true), (1, false, false, false)];

fn rows_8_to_10(sh: &Sheet) -> [(u8, bool, bool, bool); 3] {
    [8, 9, 10].map(|r| {
        let (level, hidden, points, show) = row_readback(sh, r);
        (level, hidden, points == 0., show)
    })
}

fn cols_h_to_j(sh: &Sheet) -> [(u8, bool, bool, bool); 3] {
    ["H", "I", "J"].map(|c| col_readback(sh, c))
}

#[test]
fn x25_whole_line_copy_and_cut_carry_outline_and_reach_excels_extent() {
    // (probe, command, source range, destination sheet, destination cell)
    let cases = [
        ("x25-copy-rows-to-t-a8", "edit.copy", "3:4", "T", "A8"),
        ("x25-cut-rows-to-t-a8", "edit.cut", "3:4", "T", "A8"),
        ("x25-cut-rows-to-s-a8", "edit.cut", "3:4", "S", "A8"),
        ("x25-copy-cols-to-t-h1", "edit.copy", "C:D", "T", "H1"),
        ("x25-cut-cols-to-t-h1", "edit.cut", "C:D", "T", "H1"),
        ("x25-cut-cols-to-s-h1", "edit.cut", "C:D", "S", "H1"),
    ];
    for (case, op, range, dest, cell) in cases {
        let mut s = x25_fixture();
        let rows = range == "3:4";
        // Track both column maps to check fields hidden by the effective width readback.
        let columns = |s: &Session| [sheet(s, "S").cols.clone(), sheet(s, "T").cols.clone()];
        let before = (!rows && op == "edit.cut" && dest == "T").then(|| columns(&s));
        s.execute(op, json!({"sheet": "S", "range": range})).unwrap();
        s.execute("sheet.activate", json!({"sheet": dest})).unwrap();
        s.execute("edit.paste", json!({"at": cell})).unwrap();
        let d = sheet(&s, dest);
        assert_eq!(if rows { rows_8_to_10(d) } else { cols_h_to_j(d) }, PASTED_GROUP, "{case}: destination lines");
        let cells = if rows { [("A3", "A8"), ("A4", "A9")] } else { [("C1", "H1"), ("D1", "I1")] };
        let source = sheet(&s, "S");
        for (from, to) in cells {
            // The fixture gives each original cell its address as a unique value.
            let expected = Value::text(from);
            assert_eq!(d.value(at(to)), expected, "{case}: destination {to}");
            assert_eq!(source.value(at(from)), if op == "edit.cut" { Value::Empty } else { expected }, "{case}: source {from}");
        }
        let expected = if rows { ("A1:I10", "H10") } else { ("A1:J9", "J8") };
        assert_eq!(extent(&mut s, dest), (json!(expected.0), json!(expected.1)), "{case}: destination UsedRange / Last Cell");
        let src = sheet(&s, "S");
        if op == "edit.cut" {
            // The cut source lines return to level 1, visible, default size; the summary line
            // outside the cut keeps ShowDetail false.
            if rows {
                assert_eq!([3, 4, 5].map(|r| row_readback(src, r)), [(1, false, 15., true), (1, false, 15., true), (1, false, 15., false)], "{case}");
            } else {
                assert_eq!(
                    ["C", "D", "E"].map(|c| col_readback(src, c)),
                    [(1, false, false, true), (1, false, false, true), (1, false, false, false)],
                    "{case}"
                );
            }
            if dest == "T" {
                assert_eq!(extent(&mut s, "S"), (json!("A1:I9"), json!("H8")), "{case}: source UsedRange / Last Cell");
            }
        }
        if let Some(before) = before {
            let pasted = columns(&s);
            s.execute("edit.undo", json!({})).unwrap();
            assert_eq!(columns(&s), before, "{case}: undo restores both column maps");
            s.execute("edit.redo", json!({})).unwrap();
            assert_eq!(columns(&s), pasted, "{case}: redo restores both column maps");
        }
        let mut reopened = reopen(&mut s);
        let d = sheet(&reopened, dest);
        assert_eq!(if rows { rows_8_to_10(d) } else { cols_h_to_j(d) }, PASTED_GROUP, "{case} reopened");
        assert_eq!(extent(&mut reopened, dest), (json!(expected.0), json!(expected.1)), "{case} reopened: UsedRange / Last Cell");
    }
}

#[test]
fn x25_absent_source_line_resets_the_destination_outline_level() {
    for (case, range, dest, rows) in [("x25-default-row-over-custom", "6:6", "8:8", true), ("x25-default-col-over-custom", "F:F", "H:H", false)] {
        let mut s = x25_fixture();
        s.execute("edit.copy", json!({"sheet": "S", "range": range})).unwrap();
        s.execute("sheet.activate", json!({"sheet": "T"})).unwrap();
        s.execute("selection.set", json!({"range": dest})).unwrap();
        s.execute("edit.paste", json!({})).unwrap();
        let t = sheet(&s, "T");
        if rows {
            assert_eq!(row_readback(t, 8), (1, false, 15., true), "{case}");
        } else {
            assert_eq!(col_readback(t, "H"), (1, false, false, true), "{case}");
        }
        assert_eq!(extent(&mut s, "T"), (json!("A1:I9"), json!("H8")), "{case}: UsedRange / Last Cell");
    }
}

#[test]
fn x25_paste_special_all_and_insert_copied_rows_carry_outline() {
    let mut s = x25_fixture();
    s.execute("edit.copy", json!({"sheet": "S", "range": "3:4"})).unwrap();
    s.execute("edit.pasteSpecial", json!({"what": "all", "sheet": "S", "at": "A8"})).unwrap();
    assert_eq!(rows_8_to_10(sheet(&s, "S")), PASTED_GROUP, "x25-paste-special-all-rows");
    assert_eq!(extent(&mut s, "S"), (json!("A1:I10"), json!("H10")), "x25-paste-special-all-rows: UsedRange / Last Cell");

    let mut s = x25_fixture();
    s.execute("edit.copy", json!({"sheet": "S", "range": "3:4"})).unwrap();
    s.execute("sheet.activate", json!({"sheet": "S"})).unwrap();
    s.execute("home.insertCopiedCells", json!({"at": "A8", "shift": "down"})).unwrap();
    let sh = sheet(&s, "S");
    // The old rows 8:9 (level 3 and hidden) now sit at 10:11; the group 9:10 is not all hidden.
    let readback = [8, 9, 10, 11].map(|r| {
        let (level, hidden, points, show) = row_readback(sh, r);
        (level, hidden, points == 0., show)
    });
    assert_eq!(readback, [(1, true, true, true), (2, true, true, true), (3, false, false, true), (1, true, true, true)], "x25-insert-copied-rows");
    assert_eq!(extent(&mut s, "S"), (json!("A1:I11"), json!("H10")), "x25-insert-copied-rows: UsedRange / Last Cell");
}

#[test]
fn whole_row_paste_marking_a_summary_needs_row_formatting_on_a_protected_sheet() {
    use gridcraft_engine::model::SheetProtection;

    // GridCraft policy, not measured in Excel: a marker-only write still needs Format Rows.
    // The destination rows already match the source, so only the summary marker would change.
    for format_rows in [false, true] {
        let mut s = Session::new();
        s.new_workbook();
        s.execute("home.insertSheet", json!({"name": "Locked"})).unwrap();
        let wb = Arc::make_mut(&mut s.doc_mut().unwrap().wb);
        let lines = [LineInfo { hidden: true, ..Default::default() }, LineInfo { hidden: true, outline: 1, ..Default::default() }];
        let source = wb.sheet_index("Sheet1").unwrap();
        let locked = wb.sheet_index("Locked").unwrap();
        let sh = wb.sheet_mut(source).unwrap();
        sh.rows.insert(2, lines[0]);
        sh.rows.insert(3, lines[1]);
        let sh = wb.sheet_mut(locked).unwrap();
        sh.rows.insert(7, lines[0]);
        sh.rows.insert(8, lines[1]);
        sh.protection = Some(SheetProtection { format_rows, ..Default::default() });
        let before = sh.rows.clone();
        s.execute("edit.copy", json!({"sheet": "Sheet1", "range": "3:4"})).unwrap();
        s.execute("sheet.activate", json!({"sheet": "Locked"})).unwrap();
        let pasted = s.execute("edit.paste", json!({"at": "A8"}));
        let sh = sheet(&s, "Locked");
        if format_rows {
            pasted.unwrap();
            assert_eq!(row_readback(sh, 10), (1, false, 15., false));
        } else {
            let err = pasted.unwrap_err().to_string();
            assert!(err.contains("Formatting rows"), "{err}");
            assert_eq!(sh.rows, before);
        }
    }
}

#[test]
fn adjacent_whole_row_cut_marks_its_summary_after_resetting_the_source() {
    // GridCraft policy, not measured in Excel: the summary may land on a just-vacated row.
    let mut s = Session::new();
    s.new_workbook();
    let sh = Arc::make_mut(&mut s.doc_mut().unwrap().wb).sheet_mut(0).unwrap();
    let detail = LineInfo { outline: 1, hidden: true, size: Some(32.), custom: true, ..Default::default() };
    sh.rows.insert(2, detail);
    sh.rows.insert(3, detail);
    sh.set_value(at("A3"), Value::text("first detail"));

    s.execute("edit.cut", json!({"range": "3:4"})).unwrap();
    s.execute("edit.paste", json!({"at": "A1"})).unwrap();
    let sh = sheet(&s, "Sheet1");
    assert_eq!(sh.rows[&0], detail);
    assert_eq!(sh.rows[&1], detail);
    assert_eq!(sh.rows[&2], LineInfo { collapsed: true, ..Default::default() });
    assert!(!sh.rows.contains_key(&3));
    assert_eq!(sh.value(at("A1")), Value::text("first detail"));
    assert_eq!(sh.value(at("A3")), Value::Empty);
}

/// The summary is determined by the deepest contiguous group touching the paste, even when
/// it extends beyond the pasted selection. These are GridCraft rules, not Excel probes.
#[test]
fn whole_line_paste_marks_a_summary_only_for_a_hidden_group_at_its_boundary() {
    for (rows, before) in [(true, false), (true, true), (false, false), (false, true)] {
        for (neighbor_depth, gap, summary_depth, collapsed) in [(1, false, 1, true), (2, false, 1, false), (2, true, 1, true), (1, false, 2, false)] {
            let mut s = Session::new();
            s.new_workbook();
            let sh = Arc::make_mut(&mut s.doc_mut().unwrap().wb).sheet_mut(0).unwrap();
            sh.summary_below = !before;
            sh.summary_right = !before;
            let lines = if rows { &mut sh.rows } else { &mut sh.cols };
            let (summary, inside) = if before { (4, 6 + u32::from(gap)) } else { (6, 4 - u32::from(gap)) };
            lines.insert(1, LineInfo { outline: 2, hidden: true, ..Default::default() });
            lines.insert(inside, LineInfo { outline: neighbor_depth, ..Default::default() });
            lines.insert(summary, LineInfo { outline: summary_depth, ..Default::default() });
            s.execute("edit.copy", json!({"range": if rows { "2:2" } else { "B:B" }})).unwrap();
            s.execute("edit.paste", json!({"at": if rows { "A6" } else { "F1" }})).unwrap();
            let sh = s.doc().unwrap().wb.sheet(0).unwrap();
            let lines = if rows { &sh.rows } else { &sh.cols };
            assert_eq!(
                lines[&summary].collapsed, collapsed,
                "rows={rows}, before={before}, gap={gap}, inside={neighbor_depth}, summary={summary_depth}"
            );
        }
    }
}

/// `x25-paste-all-hidden-{both,dest,source}`: rows 3:4 (30 pt, 38 pt) pasted over rows 8:9
/// (22 pt, 45 pt), with row 4 and/or row 9 hidden. Excel's UsedRange stays `A3:B9`; its Last
/// Cell skips a hidden row 9, so it moves with the row 9 visibility the paste leaves.
#[test]
fn x25_paste_all_hidden_rows_move_last_cell_with_the_pasted_visibility() {
    // (probe, row 4 hidden, row 9 hidden, Last Cell before, Last Cell after)
    let cases = [("both", true, true, "B8", "B8"), ("dest", false, true, "B8", "B9"), ("source", true, false, "B9", "B8")];
    for (case, source_hidden, dest_hidden, before, after) in cases {
        for paste in ["edit.paste", "edit.pasteSpecial"] {
            let mut s = Session::new();
            s.new_workbook();
            s.execute("sheet.rename", json!({"name": "S"})).unwrap();
            let wb = Arc::make_mut(&mut s.doc_mut().unwrap().wb);
            let sh = wb.sheet_mut(0).unwrap();
            for (row, points, hidden) in [(3, 30., false), (4, 38., source_hidden), (8, 22., false), (9, 45., dest_hidden)] {
                sh.set_value(CellRef::new(row - 1, 0), Value::text(format!("A{row}")));
                sh.set_value(CellRef::new(row - 1, 1), Value::text(format!("B{row}")));
                sh.rows.insert(row - 1, LineInfo { size: Some(points * 4. / 3.), custom: true, hidden, ..Default::default() });
            }
            let context = format!("x25-paste-all-hidden-{case} via {paste}");
            assert_eq!(extent(&mut s, "S"), (json!("A3:B9"), json!(before)), "{context}: before");
            s.execute("edit.copy", json!({"range": "3:4"})).unwrap();
            let params = if paste == "edit.paste" { json!({"at": "A8"}) } else { json!({"what": "all", "at": "A8"}) };
            s.execute(paste, params).unwrap();
            let sh = sheet(&s, "S");
            assert_eq!(
                [8, 9].map(|r| row_readback(sh, r)),
                [(1, false, 30., true), (1, source_hidden, if source_hidden { 0. } else { 38. }, true)],
                "{context}"
            );
            assert_eq!(extent(&mut s, "S"), (json!("A3:B9"), json!(after)), "{context}: after");
            s.execute("selection.set", json!({"cell": "A1"})).unwrap();
            s.execute("edit.goToSpecial", json!({"kind": "lastCell"})).unwrap();
            assert_eq!(s.active().unwrap().selection.active.a1(), after, "{context}: Go To Special Last Cell");
        }
    }
}

/// The pre-review summary rule over every outline group, kept here as the oracle for the
/// boundary-local scan in `LineAxis::mark_collapsed_summary`.
fn group_rule(lines: &std::collections::BTreeMap<u32, LineInfo>, before: bool, first: u32, last: u32, end: u32) -> Option<u32> {
    gridcraft_engine::outline::groups_with(lines, before)
        .into_iter()
        .find(|g| g.collapsed && g.has_summary() && if before { g.start == first } else { g.end == last })
        .map(|g| g.summary())
        .filter(|&i| i < end && !lines.get(&i).is_some_and(|l| l.collapsed))
}

/// Generated small sheets (levels 0-9, gaps, hidden mixes, both summary directions, both sheet
/// edges): the marker the paste sets equals what the group rule gives on the pasted lines.
#[test]
fn boundary_summary_marker_matches_the_group_rule_on_generated_sheets() {
    use gridcraft_engine::core::{MAX_COLS, MAX_ROWS, col_to_letters};

    let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = |n: u64| {
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        (seed >> 33) % n
    };
    let mut marked = 0;
    for iteration in 0..1500 {
        let (rows, before) = (next(2) == 0, next(2) == 0);
        let width = 1 + next(3) as u32;
        let end = if rows { MAX_ROWS } else { MAX_COLS };
        let (first, last) = match next(6) {
            0 => (0, width - 1),
            1 => (end - width, end - 1),
            _ => (8, 7 + width),
        };
        let source = 100;
        let mut s = Session::new();
        s.new_workbook();
        let sh = Arc::make_mut(&mut s.doc_mut().unwrap().wb).sheet_mut(0).unwrap();
        sh.summary_below = !before;
        sh.summary_right = !before;
        let lines = if rows { &mut sh.rows } else { &mut sh.cols };
        let neighbourhood = first.saturating_sub(6)..=last.saturating_add(6).min(end - 1);
        for i in (source..source + width).chain(neighbourhood) {
            if next(4) != 0 {
                // Deep levels make "at or above the boundary's level" cases frequent.
                let outline = [0, 1, 1, 2, 2, 3, 7, 9][next(8) as usize];
                lines.insert(i, LineInfo { outline, hidden: next(3) != 0, collapsed: next(5) == 0, ..Default::default() });
            }
        }
        let original = lines.clone();
        let name = |i: u32| if rows { (i + 1).to_string() } else { col_to_letters(i) };
        let anchor = if rows { CellRef::new(first, 0) } else { CellRef::new(0, first) };
        s.execute("edit.copy", json!({"range": format!("{}:{}", name(source), name(source + width - 1))})).unwrap();
        s.execute("edit.paste", json!({"at": anchor.a1()})).unwrap();
        let sh = s.doc().unwrap().wb.sheet(0).unwrap();
        let after = if rows { &sh.rows } else { &sh.cols };
        let candidate = if before { first.checked_sub(1) } else { Some(last + 1).filter(|&i| i < end) };
        // The pasted lines as they stand before any marker: the candidate keeps its old record.
        let mut unmarked = after.clone();
        if let Some(i) = candidate {
            match original.get(&i) {
                Some(&line) => unmarked.insert(i, line),
                None => unmarked.remove(&i),
            };
        }
        let expected = group_rule(&unmarked, before, first, last, end);
        let context = format!("#{iteration}: rows={rows} before={before} paste={first}..={last} lines={original:?}");
        for (&i, line) in after.range(..first).chain(after.range(last.saturating_add(1)..)) {
            let mut old = original.get(&i).copied().unwrap_or_default();
            if expected == Some(i) {
                old.collapsed = true;
            }
            assert_eq!(*line, old, "{context}: line {i}");
        }
        if let Some(i) = expected {
            assert!(after.get(&i).is_some_and(|l| l.collapsed), "{context}: marker on {i}");
            marked += 1;
        } else if let Some(i) = candidate {
            assert_eq!(after.get(&i).copied(), original.get(&i).copied(), "{context}: no marker on {i}");
        }
    }
    assert!(marked > 50, "the generator should reach the marking branch often, got {marked}");
}
