//! Excel's reported UsedRange and Last Cell (X25 measurements, Excel for Mac 16.115.2) against
//! the shared work extent that print, export and calculation keep using.
//!
//! Each case builds the final worksheet state of a measured probe in code. Where the probe's
//! outline getters returned nothing, the outline level and collapsed summary line after a hidden
//! group are inferred (see `docs/gaps.md` §25).

use std::sync::Arc;

use gridcraft_engine::Session;
use gridcraft_engine::core::{CellRef, RangeRef, Value};
use gridcraft_engine::model::style::{Style, StyleId};
use gridcraft_engine::model::{LineInfo, Sheet};
use serde_json::json;

fn at(a1: &str) -> CellRef {
    CellRef::parse(a1).unwrap()
}

fn col(letter: &str) -> u32 {
    at(&format!("{letter}1")).col
}

/// A session whose only sheet is built by `build`, given a style for styled cells and lines.
fn session(build: impl FnOnce(&mut Sheet, StyleId)) -> Session {
    let mut s = Session::new();
    s.new_workbook();
    let wb = Arc::make_mut(&mut s.doc_mut().unwrap().wb);
    let mut style = Style::default();
    style.font.bold = true;
    let st = wb.styles.intern(style);
    build(wb.sheet_mut(0).unwrap(), st);
    s
}

fn put(sh: &mut Sheet, cells: &[&str]) {
    for &c in cells {
        sh.set_value(at(c), Value::text(c));
    }
}

fn row(sh: &mut Sheet, n: u32, info: LineInfo) {
    sh.rows.insert(n - 1, info);
}

fn column(sh: &mut Sheet, letter: &str, info: LineInfo) {
    sh.cols.insert(col(letter), info);
}

fn sized(size: f32) -> LineInfo {
    LineInfo { size: Some(size), custom: true, ..Default::default() }
}

/// The fixture's destination rows 8:9 and columns H:I, the same on S and T: a green 12 pt row 8
/// at outline level 2, a hidden yellow 45 pt row 9, and the matching columns.
fn destination_lines(sh: &mut Sheet, st: StyleId) {
    row(sh, 8, LineInfo { style: Some(st), outline: 2, ..sized(16.) });
    row(sh, 9, LineInfo { style: Some(st), hidden: true, ..sized(60.) });
    column(sh, "H", LineInfo { style: Some(st), outline: 2, ..sized(40.) });
    column(sh, "I", LineInfo { style: Some(st), hidden: true, ..sized(285.) });
}

/// Sheet T before any paste: H1, I1, A8 and A9 plus the destination lines.
fn t_base(sh: &mut Sheet, st: StyleId) {
    put(sh, &["H1", "I1", "A8", "A9"]);
    destination_lines(sh, st);
}

/// Sheet S before any paste: hidden source rows 3:4 (the second at outline level 1, row 5 the
/// collapsed summary), hidden source columns C:D (E the summary), a bare row 7 and the
/// destination lines.
fn s_base(sh: &mut Sheet, st: StyleId) {
    put(sh, &["A3", "B3", "A4", "B4", "A5", "A8", "A9", "C1", "D1", "H1", "I1"]);
    row(sh, 3, LineInfo { style: Some(st), hidden: true, ..sized(40.) });
    row(sh, 4, LineInfo { hidden: true, outline: 1, ..sized(50.) });
    row(sh, 5, LineInfo { collapsed: true, ..Default::default() });
    row(sh, 7, LineInfo { saved_element: true, ..Default::default() });
    column(sh, "C", LineInfo { style: Some(st), hidden: true, ..sized(124.) });
    column(sh, "D", LineInfo { hidden: true, outline: 1, ..sized(166.) });
    column(sh, "E", LineInfo { collapsed: true, ..sized(89.) });
    destination_lines(sh, st);
}

/// Rows 3:4 of S pasted at row 8: their size, style and visibility, with the source outline level
/// and a collapsed summary row 10 (inferred), and the source cells.
fn rows_pasted_at_8(sh: &mut Sheet, st: StyleId) {
    put(sh, &["A8", "B8", "A9", "B9"]);
    row(sh, 8, LineInfo { style: Some(st), hidden: true, ..sized(40.) });
    row(sh, 9, LineInfo { hidden: true, outline: 1, ..sized(50.) });
    row(sh, 10, LineInfo { collapsed: true, ..Default::default() });
}

/// Columns C:D of S pasted at column H, likewise with a collapsed summary column J (inferred).
fn cols_pasted_at_h(sh: &mut Sheet, st: StyleId) {
    sh.set_value(at("H1"), Value::text("S-C1"));
    sh.set_style(at("H1"), st);
    sh.set_value(at("I1"), Value::text("S-D1"));
    column(sh, "H", LineInfo { style: Some(st), hidden: true, ..sized(124.) });
    column(sh, "I", LineInfo { hidden: true, outline: 1, ..sized(166.) });
    column(sh, "J", LineInfo { collapsed: true, ..Default::default() });
}

/// Sheet U of the UsedRange-only probes: numbers in C3:E5.
fn u_base(sh: &mut Sheet) {
    for r in ["3", "4", "5"] {
        for c in ["C", "D", "E"] {
            sh.set_value(at(&format!("{c}{r}")), Value::number(1.0));
        }
    }
}

type Build = fn(&mut Sheet, StyleId);

/// (probe ids, final state, Excel UsedRange, Excel Last Cell).
const CASES: &[(&str, Build, &str, &str)] = &[
    ("x25 fixture S and T before every action", |sh, st| t_base(sh, st), "A1:I9", "H8"),
    ("x25 S before every action", |sh, st| s_base(sh, st), "A1:I9", "H8"),
    (
        "x25-copy-rows-{to-t-a8,to-t-8-9,explicit-to-t-a8,explicit-to-t-8-9}, x25-cut-rows-to-t-a8: T",
        |sh, st| {
            t_base(sh, st);
            rows_pasted_at_8(sh, st);
        },
        "A1:I10",
        "H10",
    ),
    (
        "x25-cut-rows-to-s-a8: S",
        |sh, st| {
            s_base(sh, st);
            for c in ["A3", "B3", "A4", "B4"] {
                sh.cells.remove(at(c));
            }
            sh.rows.remove(&2);
            sh.rows.remove(&3);
            rows_pasted_at_8(sh, st);
        },
        "A1:I10",
        "H10",
    ),
    (
        "x25-paste-special-all-rows: S",
        |sh, st| {
            s_base(sh, st);
            rows_pasted_at_8(sh, st);
        },
        "A1:I10",
        "H10",
    ),
    (
        "x25-copy-cols-{to-t-h1,to-t-h-i,explicit-to-t-h1,explicit-to-t-h-i}, x25-cut-cols-to-t-h1: T",
        |sh, st| {
            t_base(sh, st);
            cols_pasted_at_h(sh, st);
        },
        "A1:J9",
        "J8",
    ),
    (
        "x25-cut-cols-to-s-h1: S",
        |sh, st| {
            s_base(sh, st);
            sh.cells.remove(at("C1"));
            sh.cells.remove(at("D1"));
            sh.cols.remove(&col("C"));
            sh.cols.remove(&col("D"));
            cols_pasted_at_h(sh, st);
        },
        "A1:J9",
        "J8",
    ),
    (
        "x25-insert-copied-rows: S (old rows 8:9 shifted to 10:11)",
        |sh, st| {
            put(sh, &["A3", "B3", "A4", "B4", "A5", "C1", "D1", "H1", "I1", "A10", "A11"]);
            row(sh, 3, LineInfo { style: Some(st), hidden: true, ..sized(40.) });
            row(sh, 4, LineInfo { hidden: true, outline: 1, ..sized(50.) });
            row(sh, 5, LineInfo { collapsed: true, ..Default::default() });
            row(sh, 7, LineInfo { saved_element: true, ..Default::default() });
            rows_pasted_at_8(sh, st);
            row(sh, 10, LineInfo { style: Some(st), outline: 2, collapsed: true, ..sized(16.) });
            row(sh, 11, LineInfo { style: Some(st), hidden: true, ..sized(60.) });
            column(sh, "H", LineInfo { style: Some(st), outline: 2, ..sized(40.) });
            column(sh, "I", LineInfo { style: Some(st), hidden: true, ..sized(285.) });
        },
        "A1:I11",
        "H10",
    ),
    (
        "x25-control-a3-b4: T",
        |sh, st| {
            t_base(sh, st);
            put(sh, &["A8", "B8", "A9", "B9"]);
        },
        "A1:I9",
        "H8",
    ),
    (
        "x25-default-row-over-custom: T (row 8 reset, A8 cleared)",
        |sh, st| {
            t_base(sh, st);
            sh.cells.remove(at("A8"));
            sh.rows.remove(&7);
        },
        "A1:I9",
        "H8",
    ),
    (
        "x25-default-col-over-custom: T (column H reset, H1 cleared)",
        |sh, st| {
            t_base(sh, st);
            sh.cells.remove(at("H1"));
            sh.cols.remove(&col("H"));
        },
        "A1:I9",
        "H8",
    ),
    ("x25-used-base", |sh, _| u_base(sh), "C3:E5", "E5"),
    (
        "x25-used-bare-row",
        |sh, _| {
            u_base(sh);
            row(sh, 40, LineInfo { saved_element: true, ..Default::default() });
        },
        "C3:E5",
        "E5",
    ),
    (
        "x25-used-height-row",
        |sh, _| {
            u_base(sh);
            row(sh, 40, sized(40.));
        },
        "C3:E40",
        "E40",
    ),
    (
        "x25-used-style-row",
        |sh, st| {
            u_base(sh);
            row(sh, 40, LineInfo { style: Some(st), ..Default::default() });
        },
        "C3:E40",
        "E40",
    ),
    (
        "x25-used-width-col",
        |sh, _| {
            u_base(sh);
            column(sh, "J", sized(145.));
        },
        "C3:E5",
        "E5",
    ),
    (
        "x25-used-style-col",
        |sh, st| {
            u_base(sh);
            column(sh, "J", LineInfo { style: Some(st), ..Default::default() });
        },
        "C3:J5",
        "J5",
    ),
];

#[test]
fn inspection_reports_excels_used_range_and_navigation_reaches_its_last_cell() {
    for &(case, build, used, last) in CASES {
        let mut s = session(build);
        let inspect = s.execute("document.inspect", json!({})).unwrap();
        assert_eq!(inspect["sheets"][0]["usedRange"], used, "{case}: usedRange");
        let stats = s.execute("review.workbookStatistics", json!({})).unwrap();
        assert_eq!(stats["sheet"]["endOfSheet"], last, "{case}: endOfSheet");
        s.execute("selection.set", json!({"cell": "B2"})).unwrap();
        s.execute("edit.goToSpecial", json!({"kind": "lastCell"})).unwrap();
        assert_eq!(s.active().unwrap().selection.active.a1(), last, "{case}: Go To Special Last Cell");
    }
}

#[test]
fn line_records_leave_the_shared_work_extent_print_and_csv_bounds_alone() {
    for &(case, build, used, _) in CASES {
        let s = session(build);
        let sh = s.active().unwrap().wb.sheet(0).unwrap();
        let shared = sh.used_range().unwrap();
        let reported = RangeRef::parse(used).unwrap();
        // The reported extent only ever adds line records to the stored cells.
        assert_eq!(shared.union(&reported), reported, "{case}: shared extent {} inside {used}", shared.a1());
        assert_eq!(gridcraft_engine::cmd::print::print_range(sh), Some(shared), "{case}: print range");
        let csv = gridcraft_xlsx::write_csv(sh, b',', |_| String::new()).unwrap();
        let lines = csv.split(|&b| b == b'\n').filter(|l| !l.is_empty()).count();
        assert_eq!(lines as u32, shared.end.row + 1, "{case}: CSV rows");
    }
    // A styled row far below the data grows only the reported extent.
    let s = session(|sh, st| {
        u_base(sh);
        row(sh, 40, LineInfo { style: Some(st), ..sized(40.) });
        column(sh, "J", LineInfo { style: Some(st), ..Default::default() });
    });
    let sh = s.active().unwrap().wb.sheet(0).unwrap();
    assert_eq!(sh.used_range().map(|r| r.a1()).as_deref(), Some("C3:E5"));
    assert_eq!(sh.reported_used_range().map(|r| r.a1()).as_deref(), Some("C3:J40"));
}

#[test]
fn last_cell_steps_back_over_every_trailing_hidden_line_but_not_past_the_first() {
    let s = session(|sh, _| {
        put(sh, &["B2", "D5"]);
        for r in 3..=5 {
            row(sh, r, LineInfo { hidden: true, ..Default::default() });
        }
        column(sh, "D", LineInfo { hidden: true, ..Default::default() });
        column(sh, "C", sized(0.));
    });
    assert_eq!(s.active().unwrap().wb.sheet(0).unwrap().last_cell().a1(), "B2");
    let all_hidden = session(|sh, _| {
        put(sh, &["C3"]);
        row(sh, 3, LineInfo { hidden: true, ..Default::default() });
    });
    assert_eq!(all_hidden.active().unwrap().wb.sheet(0).unwrap().last_cell().a1(), "C3");
}
