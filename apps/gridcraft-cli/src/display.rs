use std::io::{self, BufWriter, Write};

use gridcraft_engine::cf::{CfCache, CfLook};
use gridcraft_engine::core::{CellRef, MAX_COLS, MAX_ROWS, RangeRef, Value, col_to_letters};
use gridcraft_engine::display::{CellLook, cell_look};
use gridcraft_engine::model::{
    Alignment, Anchor, AnchorMode, BorderLine, BorderStyle, Color, Font, HAlign, LineIndex, Sheet, StyleId, Theme, Underline, Workbook,
    points_to_chars,
};
use gridcraft_ui_egui::grid::display::{DisplayText, SheetText, TextContext};
use serde::Serialize;
use serde_json::json;

pub(super) fn run(args: &[String]) -> Result<(), String> {
    let Some((path, flags)) = args.split_first() else {
        return Err("usage: gridcraft-cli display <file> --json [--sheet NAME] [--range A1:Z99]".into());
    };
    let (mut json, mut sheet, mut range) = (false, None, None);
    let mut flags = flags.iter();
    while let Some(flag) = flags.next() {
        match flag.as_str() {
            "--json" if !json => json = true,
            "--sheet" if sheet.is_none() => sheet = Some(flags.next().filter(|s| !s.starts_with("--")).ok_or("--sheet requires a name")?.as_str()),
            "--range" if range.is_none() => {
                let arg = flags.next().ok_or("--range requires an A1 range")?;
                range = Some(RangeRef::parse(arg).ok_or_else(|| format!("invalid display range: {arg}"))?);
            }
            _ => return Err(format!("unexpected display argument: {flag}")),
        }
    }
    if !json {
        return Err("display requires --json".into());
    }
    let session = super::open(path)?;
    let wb = &session.doc().map_err(|err| err.to_string())?.wb;
    let index = sheet.map(|name| wb.sheets.iter().position(|sh| sh.name == name).ok_or_else(|| format!("sheet not found: {name}"))).transpose()?;
    let mut out = BufWriter::new(io::stdout().lock());
    dump(&mut out, wb, index, range).and_then(|()| out.flush()).map_err(|err| err.to_string())
}

// Merge two already ordered streams, coalescing coordinate collisions (e.g. styled spill cells).
fn union<T: Ord + Copy>(a: impl Iterator<Item = T>, b: impl Iterator<Item = T>) -> impl Iterator<Item = T> {
    let (mut a, mut b) = (a.peekable(), b.peekable());
    std::iter::from_fn(move || match (a.peek(), b.peek()) {
        (Some(x), Some(y)) if x == y => {
            b.next();
            a.next()
        }
        (Some(x), Some(y)) if x > y => b.next(),
        (None, _) => b.next(),
        _ => a.next(),
    })
}

fn coordinates(sh: &Sheet, r: RangeRef) -> impl Iterator<Item = CellRef> + '_ {
    let stored = sh.cells.iter_range(r).map(|(c, _)| c);
    // Jump over unrelated spills on the same row, rather than scanning the entire span of keys
    // between the range's corners when only a narrow column selection was requested.
    let mut at = r.start;
    let spills = std::iter::from_fn(move || {
        loop {
            let (&c, _) = sh.spill.range(at..).next()?;
            if c.row > r.end.row {
                return None;
            }
            if c.col < r.start.col {
                at = CellRef::new(c.row, r.start.col);
            } else if c.col > r.end.col {
                at = CellRef::new(c.row + 1, r.start.col);
            } else {
                at = CellRef::new(c.row, c.col + 1);
                return Some(c);
            }
        }
    });
    union(stored, spills)
}

fn dump(out: &mut impl Write, wb: &Workbook, index: Option<usize>, selection: Option<RangeRef>) -> io::Result<()> {
    let mut context = TextContext::new(wb);
    let normal =
        &wb.cell_styles.iter().find(|(name, _)| name.eq_ignore_ascii_case("Normal")).map_or(wb.styles.get(StyleId::DEFAULT), |(_, s)| s).font;
    write!(out, "{{\"version\":1,\"normal_font\":")?;
    serde_json::to_writer(&mut *out, &FontRecord::new(normal, &wb.theme))?;
    write!(out, ",\"sheets\":[")?;
    let mut first = true;
    for (si, sh) in wb.sheets.iter().enumerate().filter(|(si, _)| index.is_none_or(|i| i == *si)) {
        comma(out, &mut first)?;
        dump_sheet(out, wb, si, sh, selection, &mut context)?;
    }
    writeln!(out, "]}}")
}

fn dump_sheet(out: &mut impl Write, wb: &Workbook, si: usize, sh: &Sheet, selection: Option<RangeRef>, context: &mut TextContext) -> io::Result<()> {
    let r = selection.unwrap_or_else(|| RangeRef::new(CellRef::new(0, 0), CellRef::new(MAX_ROWS - 1, MAX_COLS - 1)));
    let (fr, fc) = sh.freeze.unwrap_or_default();
    write!(out, "{{\"name\":")?;
    serde_json::to_writer(&mut *out, &sh.name)?;
    write!(out, ",\"view\":")?;
    serde_json::to_writer(
        &mut *out,
        &json!({
            "index": si, "visibility": sh.visibility, "mode": sh.view, "zoom_percent": sh.zoom, "gridlines": sh.show_gridlines,
            "right_to_left": sh.right_to_left, "show_zeros": sh.show_zeros, "show_formulas": sh.show_formulas,
            "tab_color": sh.tab_color.and_then(|c| c.resolve(&wb.theme)), "freeze": {"rows": fr, "columns": fc},
            "range": selection.map(|r| r.a1())
        }),
    )?;
    write!(out, ",\"standard_dimensions\":")?;
    serde_json::to_writer(
        &mut *out,
        &json!({
            "width_pt": f64::from(sh.default_col_width) * PT, "width_chars": points_to_chars(f64::from(sh.default_col_width)),
            "height_pt": f64::from(sh.default_row_height) * PT, "digit_width_px": 7, "padding_px": 5,
        }),
    )?;
    let merges = sh.merges.iter().filter(|m| m.intersects(&r));
    write!(out, ",\"merges\":")?;
    array(out, merges.map(RangeRef::a1))?;
    write!(out, ",\"drawings\":")?;
    drawings(out, sh, selection)?;
    write!(out, ",\"cells\":[")?;
    let mut cells = coordinates(sh, r).peekable();
    let layout = SheetText::new(wb, si, sh);
    let mut cf = CfCache::new();
    let mut first = true;
    let mut used_cols = vec![false; (r.end.col - r.start.col + 1) as usize];
    while cells.peek().is_some() {
        let frame = context.frame();
        for c in cells.by_ref().take(512) {
            used_cols[(c.col - r.start.col) as usize] = true;
            let raw = sh.value_ref(c).unwrap_or(&Value::Empty);
            let look = cell_look(wb, si, sh, c, &mut cf);
            let text = layout.cell(&frame, c, raw, &look);
            comma(out, &mut first)?;
            serde_json::to_writer(&mut *out, &CellRecord::new(c, raw, &look, text, &wb.theme))?;
        }
    }
    write!(out, "],\"columns\":")?;
    for &col in sh.cols.range(r.start.col..=r.end.col).map(|(k, _)| k) {
        used_cols[(col - r.start.col) as usize] = true;
    }
    array(
        out,
        used_cols.into_iter().enumerate().filter(|(_, used)| *used).map(|(i, _)| {
            let col = r.start.col + i as u32;
            let size = sh.cols.get(&col).and_then(|l| l.size);
            let px = f64::from(sh.col_width(col));
            Column {
                column: col_to_letters(col),
                width_pt: px * PT,
                width_chars: points_to_chars(px),
                hidden: sh.cols.get(&col).is_some_and(|l| l.hidden),
                custom: size.is_some(),
                configured_width_pt: f64::from(size.unwrap_or(sh.default_col_width)) * PT,
            }
        }),
    )?;
    write!(out, ",\"rows\":")?;
    let mut previous_row = None;
    let used_rows = coordinates(sh, r).filter_map(|c| (previous_row.replace(c.row) != Some(c.row)).then_some(c.row));
    let rows = union(used_rows, sh.rows.range(r.start.row..=r.end.row).map(|(&row, _)| row));
    array(
        out,
        rows.map(|row| {
            let line = sh.rows.get(&row);
            Row {
                row: row + 1,
                height_pt: f64::from(sh.row_height(row)) * PT,
                hidden: line.is_some_and(|l| l.hidden),
                custom: line.is_some_and(|l| l.custom),
                configured_height_pt: f64::from(line.and_then(|l| l.size).unwrap_or(sh.default_row_height)) * PT,
            }
        }),
    )?;
    write!(out, "}}")
}

const PT: f64 = 72.0 / 96.0;

fn comma(out: &mut impl Write, first: &mut bool) -> io::Result<()> {
    if !std::mem::replace(first, false) {
        write!(out, ",")?;
    }
    Ok(())
}

fn array(out: &mut impl Write, items: impl IntoIterator<Item = impl Serialize>) -> io::Result<()> {
    write!(out, "[")?;
    let mut first = true;
    for item in items {
        comma(out, &mut first)?;
        serde_json::to_writer(&mut *out, &item)?;
    }
    write!(out, "]")
}

#[derive(Serialize)]
struct Column {
    column: String,
    width_pt: f64,
    width_chars: f64,
    hidden: bool,
    custom: bool,
    configured_width_pt: f64,
}

#[derive(Serialize)]
struct Row {
    row: u32,
    height_pt: f64,
    hidden: bool,
    custom: bool,
    configured_height_pt: f64,
}

#[derive(Serialize)]
struct CellRecord<'a> {
    a1: String,
    text: String,
    raw: &'a Value,
    number_format: &'a str,
    font: FontRecord<'a>,
    fill: Option<[u8; 3]>,
    conditional: Option<Conditional<'a>>,
    borders: Borders,
    alignment: Alignment,
    painted_horizontal: Option<HAlign>,
}

impl<'a> CellRecord<'a> {
    fn new(c: CellRef, raw: &'a Value, look: &'a CellLook, text: Option<DisplayText>, theme: &Theme) -> Self {
        let s = &look.style;
        let mut font = FontRecord::new(&s.font, theme);
        let (text, painted_horizontal) = match text {
            Some(t) => {
                font.color = t.color;
                (t.text, Some(t.horizontal))
            }
            None => (String::new(), None),
        };
        let b = s.borders;
        Self {
            a1: c.a1(),
            text,
            raw,
            number_format: s.num_fmt.as_str(),
            font,
            fill: look.fill,
            conditional: look.conditional.as_ref().map(Conditional::from),
            borders: Borders {
                left: border(b.left, theme),
                right: border(b.right, theme),
                top: border(b.top, theme),
                bottom: border(b.bottom, theme),
                diagonal_up: border(b.diag_up, theme),
                diagonal_down: border(b.diag_down, theme),
            },
            alignment: s.align,
            painted_horizontal,
        }
    }
}

#[derive(Serialize)]
struct FontRecord<'a> {
    name: &'a str,
    size_pt: f32,
    bold: bool,
    italic: bool,
    underline: Underline,
    strike: bool,
    color: [u8; 3],
}

impl<'a> FontRecord<'a> {
    fn new(f: &'a Font, theme: &Theme) -> Self {
        Self { name: &f.name, size_pt: f.size, bold: f.bold, italic: f.italic, underline: f.underline, strike: f.strike, color: rgb(f.color, theme) }
    }
}

fn rgb(color: Color, theme: &Theme) -> [u8; 3] {
    color.resolve(theme).unwrap_or([0, 0, 0])
}

#[derive(Serialize)]
struct Borders {
    left: Option<Border>,
    right: Option<Border>,
    top: Option<Border>,
    bottom: Option<Border>,
    diagonal_up: Option<Border>,
    diagonal_down: Option<Border>,
}

#[derive(Serialize)]
struct Border {
    style: BorderStyle,
    color: [u8; 3],
}

fn border(line: BorderLine, theme: &Theme) -> Option<Border> {
    (!line.is_none()).then(|| Border { style: line.style, color: rgb(line.color, theme) })
}

#[derive(Serialize)]
struct Conditional<'a> {
    differential_style: bool,
    scale_fill: Option<[u8; 3]>,
    bar: Option<Bar>,
    icon: Option<Icon<'a>>,
    hide_value: bool,
}

impl<'a> From<&'a CfLook> for Conditional<'a> {
    fn from(cf: &'a CfLook) -> Self {
        Self {
            differential_style: cf.style.is_some(),
            scale_fill: cf.scale_fill,
            bar: cf.bar.map(|(fraction, color, gradient)| Bar { fraction, color, gradient }),
            icon: cf.icon.as_ref().map(|(set, index)| Icon { set, index: *index }),
            hide_value: cf.hide_value,
        }
    }
}

#[derive(Serialize)]
struct Bar {
    fraction: f32,
    color: [u8; 3],
    gradient: bool,
}

#[derive(Serialize)]
struct Icon<'a> {
    set: &'a str,
    index: usize,
}

#[derive(Serialize)]
struct Drawing<'a> {
    kind: &'static str,
    id: Option<u32>,
    title: Option<&'a str>,
    anchor: AnchorRecord,
}

#[derive(Serialize)]
struct AnchorRecord {
    cell: String,
    dx_pt: f64,
    dy_pt: f64,
    width_pt: f64,
    height_pt: f64,
    mode: AnchorMode,
}

fn drawings(out: &mut impl Write, sh: &Sheet, selection: Option<RangeRef>) -> io::Result<()> {
    let cols = LineIndex::new(&sh.cols, sh.default_col_width, MAX_COLS);
    let rows = LineIndex::new(&sh.rows, sh.default_row_height, MAX_ROWS);
    let box_of = |c: CellRef| (cols.start(c.col), rows.start(c.row));
    let bounds = selection.map(|r| (box_of(r.start), box_of(CellRef::new(r.end.row + 1, r.end.col + 1))));
    write!(out, "[")?;
    let mut first = true;
    let mut emit = |kind, id, title, a: Anchor| {
        let (x, y) = box_of(a.cell);
        let x = x + f64::from(a.dx);
        let y = y + f64::from(a.dy);
        if bounds.is_some_and(|((l, t), (r, b))| x >= r || y >= b || x + f64::from(a.width) <= l || y + f64::from(a.height) <= t) {
            return Ok(());
        }
        let anchor = AnchorRecord {
            cell: a.cell.a1(),
            dx_pt: f64::from(a.dx) * PT,
            dy_pt: f64::from(a.dy) * PT,
            width_pt: f64::from(a.width) * PT,
            height_pt: f64::from(a.height) * PT,
            mode: a.mode,
        };
        comma(out, &mut first)?;
        serde_json::to_writer(&mut *out, &Drawing { kind, id, title, anchor }).map_err(io::Error::from)
    };
    for obj in &sh.charts {
        emit("chart", Some(obj.id), obj.title.as_deref(), obj.anchor)?;
    }
    for obj in &sh.images {
        emit("image", Some(obj.id), Some(obj.alt.as_str()), obj.anchor)?;
    }
    for obj in &sh.shapes {
        emit("shape", Some(obj.id), Some(obj.text.as_str()), obj.anchor)?;
    }
    for obj in &sh.slicers {
        emit("slicer", Some(obj.id), Some(obj.caption.as_str()), obj.anchor)?;
    }
    for obj in &sh.controls {
        emit("control", Some(obj.id), None, obj.anchor)?;
    }
    for obj in &sh.groups {
        if let Some(a) = sh.group_anchor(obj.id) {
            emit("group", Some(obj.id), None, a)?;
        }
    }
    write!(out, "]")
}

#[cfg(test)]
mod tests {
    use super::*;
    use gridcraft_engine::model::{Cell, CfOperator, CfRule, CfValueKind, CondFormat, FormControl, LineInfo, NumFmt, Style};
    use serde_json::Value as Json;
    use std::sync::Arc;

    fn address(s: &str) -> CellRef {
        CellRef::parse(s).unwrap()
    }
    fn range(s: &str) -> RangeRef {
        RangeRef::parse(s).unwrap()
    }

    fn render(wb: &Workbook, selection: Option<&str>) -> Json {
        let mut out = Vec::new();
        dump(&mut out, wb, None, selection.map(range)).unwrap();
        serde_json::from_slice(&out).unwrap()
    }

    #[test]
    fn display_text_and_effective_styles() {
        let mut wb = Workbook::new();
        let mut values = Vec::new();
        for (col, code, value) in [
            ("A1", "yyyy-mm-dd", 45_293.0),
            ("B1", "$#,##0.00", 1234.5),
            ("C1", "[Red]0.00\" km\"", 2.5),
            ("D1", "0*.\"x\"", 2.0),
            ("E1", "yyyy-mm-dd", 45_293.0),
            ("F1", "General", 8.0),
            ("G1", "General", 4.0),
            ("G2", "General", 8.0),
            ("H1", "General", 4.0),
            ("H2", "General", 8.0),
            ("I1", "General", 7.0),
        ] {
            let style = wb.styles.derive(StyleId::DEFAULT, |s| s.num_fmt = NumFmt::new(code));
            values.push((address(col), Cell::styled(Value::Number(value), style)));
        }
        let sh = Arc::make_mut(&mut wb.sheets[0]);
        sh.default_col_width = 140.0;
        sh.cols.insert(4, LineInfo { size: Some(24.0), ..LineInfo::default() });
        for (c, v) in values {
            sh.set_cell(c, v);
        }
        let mut style = Style::default();
        style.font.bold = true;
        style.font.italic = true;
        style.font.underline = Underline::Single;
        style.font.color = Color::rgb(15, 16, 17);
        style.borders.bottom = BorderLine::thin();
        style.num_fmt = NumFmt::new("0.0");
        for (r, rule) in [
            ("F1", CfRule::CellIs { op: CfOperator::Greater, a: "5".into(), b: None, style: Box::new(style) }),
            ("G1:G2", CfRule::ColorScale { stops: vec![(CfValueKind::Min, Color::rgb(255, 0, 0)), (CfValueKind::Max, Color::rgb(0, 255, 0))] }),
            (
                "H1:H2",
                CfRule::DataBar {
                    min: CfValueKind::Number(0.0),
                    max: CfValueKind::Max,
                    color: Color::rgb(0, 0, 255),
                    gradient: true,
                    show_value: false,
                },
            ),
            (
                "I1",
                CfRule::IconSet {
                    set: "3Arrows".into(),
                    thresholds: vec![CfValueKind::Number(0.0), CfValueKind::Number(3.0), CfValueKind::Number(6.0)],
                    reverse: false,
                    show_value: false,
                },
            ),
        ] {
            sh.cond_formats.push(CondFormat { ranges: vec![range(r)], rule, priority: 1, stop_if_true: false });
        }
        let result = render(&wb, Some("A1:I1"));
        let cells = result["sheets"][0]["cells"].as_array().unwrap();
        assert_eq!(cells.len(), 9);
        assert_eq!(cells[0]["text"], "2024-01-02");
        assert_eq!(cells[1]["text"], "$1,234.50");
        assert_eq!(cells[2]["text"], "2.50 km");
        assert_eq!(cells[2]["font"]["color"], json!([255, 0, 0]));
        let fill = cells[3]["text"].as_str().unwrap();
        assert!(fill.starts_with("2..") && fill.ends_with('x'), "{fill:?}");
        let hash = cells[4]["text"].as_str().unwrap();
        assert!(hash.len() > 1 && hash.bytes().all(|b| b == b'#'), "{hash:?}");
        assert_eq!(cells[5]["text"], "8.0");
        assert_eq!(cells[5]["font"]["bold"], true);
        assert_eq!(cells[5]["font"]["italic"], true);
        assert_eq!(cells[5]["font"]["underline"], "Single");
        assert_eq!(cells[5]["font"]["color"], json!([15, 16, 17]));
        assert_eq!(cells[5]["borders"]["bottom"]["style"], "Thin");
        assert_eq!(cells[6]["fill"], json!([255, 0, 0]));
        assert_eq!(cells[7]["conditional"]["bar"], json!({"fraction": 0.55, "color": [0, 0, 255], "gradient": true}));
        assert_eq!(cells[7]["text"], "");
        assert_eq!(cells[7]["raw"], json!({"t": "Number", "v": 4.0}));
        assert_eq!(cells[8]["conditional"]["icon"], json!({"set": "3Arrows", "index": 2}));
    }

    #[test]
    fn display_sparse_cells_geometry_and_drawing_intersection() {
        let mut wb = Workbook::new();
        let sh = Arc::make_mut(&mut wb.sheets[0]);
        sh.default_col_width = 75.0;
        sh.default_row_height = 24.0;
        sh.cols.insert(1, LineInfo { size: Some(128.0), hidden: true, ..LineInfo::default() });
        sh.rows.insert(1, LineInfo { size: Some(32.0), custom: true, hidden: true, ..LineInfo::default() });
        sh.freeze = Some((2, 1));
        sh.zoom = 125;
        sh.show_gridlines = false;
        sh.tab_color = Some(Color::rgb(40, 80, 160));
        sh.merges.push(range("A1:C1"));
        sh.set_cell(address("A1"), Cell::value(Value::text("anchor")));
        sh.set_cell(address("B1"), Cell::value(Value::text("covered")));
        sh.set_cell(address("C2"), Cell::value(Value::text("hidden")));
        sh.set_cell(address("XFD1048576"), Cell::value(Value::Bool(true)));
        sh.spill.insert(address("C3"), Value::Bool(false));
        sh.controls.push(FormControl {
            id: 7,
            anchor: Anchor { cell: address("A1"), dx: 10.0, dy: 4.0, width: 100.0, height: 20.0, mode: AnchorMode::default() },
            ..FormControl::default()
        });

        let all = render(&wb, None);
        assert_eq!(all["sheets"][0]["cells"].as_array().unwrap().len(), 5);
        let result = render(&wb, Some("B1:C3"));
        let sh = &result["sheets"][0];
        assert_eq!(sh["merges"], json!(["A1:C1"]));
        let cells = sh["cells"].as_array().unwrap();
        assert_eq!(cells.iter().map(|c| c["a1"].as_str().unwrap()).collect::<Vec<_>>(), ["B1", "C2", "C3"]);
        assert_eq!(cells[0]["text"], "");
        assert_eq!(cells[1]["text"], "");
        assert_eq!(cells[2]["text"], "FALSE");
        assert_eq!(sh["view"]["freeze"], json!({"columns": 1, "rows": 2}));
        assert_eq!(sh["view"]["zoom_percent"], 125);
        assert_eq!(sh["view"]["gridlines"], false);
        assert_eq!(sh["view"]["tab_color"], json!([40, 80, 160]));
        assert_eq!(
            sh["columns"][0],
            json!({"column":"B", "width_pt":0.0, "width_chars":0.0, "configured_width_pt":96.0, "hidden":true, "custom":true})
        );
        assert_eq!(sh["rows"][1], json!({"row":2,"height_pt":0.0,"configured_height_pt":24.0,"hidden":true,"custom":true}));
        assert_eq!(sh["standard_dimensions"]["height_pt"], 18.0);
        assert_eq!(sh["standard_dimensions"]["width_chars"], 10.0);
        assert_eq!(sh["drawings"][0]["anchor"]["cell"], "A1");
        assert_eq!(sh["drawings"][0]["anchor"]["width_pt"], 75.0);
    }

    #[test]
    fn display_v1_cell_schema() {
        let mut wb = Workbook::new();
        let style = wb.styles.derive(StyleId::DEFAULT, |s| {
            s.font.name = "Example Font".into();
            s.font.size = 9.0;
        });
        Arc::make_mut(&mut wb.sheets[0]).set_cell(address("A1"), Cell::styled(Value::text(" example "), style));
        let result = render(&wb, None);
        assert_eq!(result["version"], 1);
        assert_eq!(
            result["sheets"][0]["cells"][0],
            json!({
                "a1": "A1", "raw": {"t":"Text", "v":" example "}, "text": " example ", "number_format": "General",
                "font": {"name":"Example Font", "size_pt":9.0, "bold":false, "italic":false, "underline":"None", "strike":false, "color":[0,0,0]},
                "fill": null, "conditional": null,
                "borders": {"left":null,"right":null,"top":null,"bottom":null,"diagonal_up":null,"diagonal_down":null},
                "alignment": {"h":"General", "v":"Bottom", "wrap":false, "shrink":false, "indent":0, "rotation":0, "reading_order":null},
                "painted_horizontal": "Left"
            })
        );
    }
}
