//! PivotTable visual styling: built-in Excel `PivotStyleLight1…28 / Medium1…28 / Dark1…28`
//! palettes and workbook-defined custom `<tableStyle>` DXF layering, resolved non-destructively
//! over cell styles for both imported unrefreshed and refreshed PivotTables.

use gridcraft_core::{CellRef, RangeRef};
use gridcraft_model::{
    BorderLine, BorderStyle, Borders, Color, Fill, PatternType, PivotColRole, PivotRowRole, PivotStyleElement, PivotStyleLayout, PivotStyleOptions,
    PivotTable, Sheet, Style, Underline, Workbook,
};

/// Resolves the effective style of a cell inside `pt` by layering the PivotTable's built-in or
/// custom style underneath the cell's own explicit style (explicit fills, font colors, borders
/// and font styles on the cell always win, matching Excel and [`crate::tables::table_cell_style`]).
pub fn pivot_cell_style(wb: &Workbook, sh: &Sheet, pt: &PivotTable, c: CellRef) -> Style {
    let own = wb.styles.get(sh.style_id(c)).clone();
    let Some(pt_style) = resolve_pivot_layer(pt, c) else {
        return own;
    };
    merge_under_own(own, &pt_style)
}

/// Computes only the PivotTable style layer (without the cell's own style) for cell `c`.
/// Returns `None` when the PivotTable has no style (`style` is empty and no `custom_style`),
/// or when `c` lies outside the PivotTable's styled areas (such as the blank row between page
/// filters and the body).
pub(crate) fn resolve_pivot_layer(pt: &PivotTable, c: CellRef) -> Option<Style> {
    if pt.style.is_empty() && pt.custom_style.0.is_none() {
        return None;
    }
    let fallback;
    let layout = match pt.style_grid.0.as_deref() {
        Some(l) => l,
        None => {
            fallback = fallback_layout(pt)?;
            &fallback
        }
    };
    let body = layout.body_range.or(pt.last_range)?;
    if let Some(custom) = pt.custom_style.0.as_deref() {
        resolve_custom_style(custom, pt, layout, body, c)
    } else {
        resolve_builtin_style(&pt.style, pt, layout, body, c)
    }
}

fn fallback_layout(pt: &PivotTable) -> Option<PivotStyleLayout> {
    let last = pt.last_range?;
    let body_start = CellRef::new(pt.anchor.row.max(last.start.row), pt.anchor.col.max(last.start.col));
    if body_start.row > last.end.row || body_start.col > last.end.col {
        return None;
    }
    let body = RangeRef::new(body_start, last.end);
    let page_rows = body_start.row.saturating_sub(last.start.row).saturating_sub(1);
    Some(PivotStyleLayout {
        body_range: Some(body),
        first_header_row: 0,
        first_data_row: u32::from(pt.show_headers),
        first_data_col: 1,
        page_rows,
        page_cols: 1,
        row_roles: vec![],
        row_subheading: vec![],
        col_roles: vec![],
        col_subheading: vec![],
    })
}

/// Classification of a single cell relative to a PivotTable's layout.
#[derive(Clone, Copy, Debug)]
struct CellRole {
    in_body: bool,
    page_label: bool,
    page_value: bool,
    is_header: bool,
    is_first_header_cell: bool,
    is_total_row: bool,
    is_blank_row: bool,
    subtotal_row: Option<u8>,
    row_subheading: Option<u8>,
    is_first_col: bool,
    is_total_col: bool,
    subtotal_col: Option<u8>,
    col_subheading: Option<u8>,
    /// True when the PivotTable body includes at least one data column (`first_data_col < body_cols`).
    has_data_cols: bool,
    /// 0-based row index inside the data area (for row stripe parity).
    data_row: u32,
    /// 0-based column index inside the body (for column stripe parity).
    rel_col: u32,
}

fn classify_cell(pt: &PivotTable, layout: &PivotStyleLayout, body: RangeRef, c: CellRef) -> Option<CellRole> {
    // Page-filter rows sit above the body separated by one blank row.
    if c.row < body.start.row {
        if layout.page_rows == 0 {
            return None;
        }
        let filter_end = body.start.row.saturating_sub(2);
        let filter_start = filter_end.saturating_sub(layout.page_rows.saturating_sub(1));
        if c.row < filter_start || c.row > filter_end || c.col < body.start.col {
            return None;
        }
        let rel_c = c.col - body.start.col;
        let max_page_cols = layout.page_cols.max(1).saturating_mul(2);
        if rel_c >= max_page_cols {
            return None;
        }
        return Some(CellRole {
            in_body: false,
            page_label: rel_c.is_multiple_of(2),
            page_value: !rel_c.is_multiple_of(2),
            is_header: false,
            is_first_header_cell: false,
            is_total_row: false,
            is_blank_row: false,
            subtotal_row: None,
            row_subheading: None,
            is_first_col: false,
            is_total_col: false,
            subtotal_col: None,
            col_subheading: None,
            has_data_cols: false,
            data_row: 0,
            rel_col: rel_c,
        });
    }
    if !body.contains(c) {
        return None;
    }

    let rel_r = c.row - body.start.row;
    let rel_c = c.col - body.start.col;
    let body_rows = body.end.row - body.start.row + 1;
    let body_cols = body.end.col - body.start.col + 1;

    let is_header = rel_r < layout.first_data_row;
    let data_row = rel_r.saturating_sub(layout.first_data_row);
    let (is_total_row, is_blank_row, subtotal_row, row_subheading) = if is_header {
        (false, false, None, None)
    } else if let Some(&role) = layout.row_roles.get(data_row as usize) {
        let sub = layout.row_subheading.get(data_row as usize).copied().flatten();
        match role {
            PivotRowRole::GrandTotal => (true, false, None, None),
            PivotRowRole::Blank => (false, true, None, None),
            PivotRowRole::Subtotal(d) => (false, false, Some(d), None),
            PivotRowRole::Data => (false, false, None, sub),
        }
    } else {
        let total = pt.grand_totals_cols && body_rows > layout.first_data_row && rel_r + 1 == body_rows;
        (total, false, None, None)
    };

    let has_data_cols = layout.first_data_col < body_cols;
    let row_label_cols = layout.first_data_col.max(1);
    let is_first_col = rel_c < row_label_cols;
    let is_first_header_cell = is_header && rel_r == 0 && rel_c == 0;
    let (is_total_col, subtotal_col, col_subheading) = if rel_c >= layout.first_data_col {
        let dc = (rel_c - layout.first_data_col) as usize;
        let csub = layout.col_subheading.get(dc).copied().flatten();
        if let Some(&role) = layout.col_roles.get(dc) {
            match role {
                PivotColRole::GrandTotal => (true, None, None),
                PivotColRole::Subtotal(d) => (false, Some(d), None),
                PivotColRole::Data => (false, None, csub),
                PivotColRole::Blank => (false, None, None),
            }
        } else {
            let total = pt.grand_totals_rows && body_cols > layout.first_data_col && rel_c + 1 == body_cols;
            (total, None, csub)
        }
    } else {
        (false, None, None)
    };

    Some(CellRole {
        in_body: true,
        page_label: false,
        page_value: false,
        is_header,
        is_first_header_cell,
        is_total_row,
        is_blank_row,
        subtotal_row,
        row_subheading,
        is_first_col,
        is_total_col,
        subtotal_col,
        col_subheading,
        has_data_cols,
        data_row,
        rel_col: rel_c,
    })
}

// ---------------------------------------------------------------- custom <tableStyle>

fn find_elem<'a>(elems: &'a [PivotStyleElement], kind: &str) -> Option<&'a PivotStyleElement> {
    elems.iter().find(|e| e.kind == kind)
}

fn stripe_kind(elems: &[PivotStyleElement], first: &'static str, second: &'static str, idx: u32) -> &'static str {
    let s1 = find_elem(elems, first).map_or(1, |e| e.size.max(1));
    let s2 = find_elem(elems, second).map_or(1, |e| e.size.max(1));
    let period = s1.saturating_add(s2).max(1);
    if idx % period < s1 { first } else { second }
}

fn subtotal_row_kind(depth: u8) -> &'static str {
    match depth {
        0 => "firstSubtotalRow",
        1 => "secondSubtotalRow",
        _ => "thirdSubtotalRow",
    }
}

fn subtotal_col_kind(depth: u8) -> &'static str {
    match depth {
        0 => "firstSubtotalColumn",
        1 => "secondSubtotalColumn",
        _ => "thirdSubtotalColumn",
    }
}

fn subheading_row_kind(depth: u8) -> &'static str {
    match depth {
        0 => "firstRowSubheading",
        1 => "secondRowSubheading",
        _ => "thirdRowSubheading",
    }
}

fn subheading_col_kind(depth: u8) -> &'static str {
    match depth {
        0 => "firstColumnSubheading",
        1 => "secondColumnSubheading",
        _ => "thirdColumnSubheading",
    }
}

fn resolve_custom_style(elems: &[PivotStyleElement], pt: &PivotTable, layout: &PivotStyleLayout, body: RangeRef, c: CellRef) -> Option<Style> {
    let role = classify_cell(pt, layout, body, c)?;
    let opts = pt.style_options;
    let mut out = Style::default();
    let mut applied = false;
    let mut apply = |kind: &str| {
        if let Some(el) = find_elem(elems, kind) {
            merge_dxf(&mut out, &el.style);
            applied = true;
        }
    };

    if !role.in_body {
        if role.page_label {
            apply("pageFieldLabels");
        } else if role.page_value {
            apply("pageFieldValues");
        }
        return applied.then_some(out);
    }

    apply("wholeTable");

    if role.is_blank_row {
        apply("blankRow");
        return applied.then_some(out);
    }

    if !role.is_header && !role.is_total_row {
        if opts.show_row_stripes && role.has_data_cols {
            apply(stripe_kind(elems, "firstRowStripe", "secondRowStripe", role.data_row));
        }
        if opts.show_col_stripes {
            apply(stripe_kind(elems, "firstColumnStripe", "secondColumnStripe", role.rel_col));
        }
        if opts.show_row_headers && role.is_first_col {
            apply("firstColumn");
        }
    }

    if !role.is_header {
        if opts.show_last_column && role.is_total_col {
            apply("lastColumn");
        }
        if let Some(d) = role.subtotal_col {
            apply(subtotal_col_kind(d));
        }
        if opts.show_row_headers
            && !role.is_total_row
            && let Some(d) = role.row_subheading
        {
            apply(subheading_row_kind(d));
        }
        if let Some(d) = role.subtotal_row {
            apply(subtotal_row_kind(d));
        }
    }

    if role.is_header && opts.show_col_headers {
        apply("headerRow");
        if let Some(d) = role.col_subheading {
            apply(subheading_col_kind(d));
        }
        if role.is_first_header_cell {
            apply("firstHeaderCell");
        }
    }
    if role.is_total_row {
        apply("totalRow");
    }

    applied.then_some(out)
}

/// Applies one DXF layer over an accumulated PivotTable style.
fn merge_dxf(dst: &mut Style, src: &Style) {
    if src.fill.pattern != PatternType::None {
        dst.fill = src.fill;
    }
    if src.font.color != Color::Auto {
        dst.font.color = src.font.color;
    }
    if src.font.bold {
        dst.font.bold = true;
    } else if src.dxf_stated.bold {
        dst.font.bold = false;
    }
    if src.font.italic {
        dst.font.italic = true;
    } else if src.dxf_stated.italic {
        dst.font.italic = false;
    }
    if src.font.strike {
        dst.font.strike = true;
    } else if src.dxf_stated.strike {
        dst.font.strike = false;
    }
    if src.font.underline != Underline::None {
        dst.font.underline = src.font.underline;
    } else if src.dxf_stated.underline {
        dst.font.underline = Underline::None;
    }
    for (d, s) in [
        (&mut dst.borders.top, src.borders.top),
        (&mut dst.borders.bottom, src.borders.bottom),
        (&mut dst.borders.left, src.borders.left),
        (&mut dst.borders.right, src.borders.right),
    ] {
        if !s.is_none() {
            *d = s;
        }
    }
}

/// Merges a resolved PivotTable style underneath a cell's own explicit style.
fn merge_under_own(mut own: Style, pt: &Style) -> Style {
    if own.fill.pattern == PatternType::None && pt.fill.pattern != PatternType::None {
        own.fill = pt.fill;
    }
    if own.font.color == Color::Auto && pt.font.color != Color::Auto {
        own.font.color = pt.font.color;
    }
    own.font.bold |= pt.font.bold;
    own.font.italic |= pt.font.italic;
    own.font.strike |= pt.font.strike;
    if own.font.underline == Underline::None && pt.font.underline != Underline::None {
        own.font.underline = pt.font.underline;
    }
    for (d, s) in [
        (&mut own.borders.top, pt.borders.top),
        (&mut own.borders.bottom, pt.borders.bottom),
        (&mut own.borders.left, pt.borders.left),
        (&mut own.borders.right, pt.borders.right),
    ] {
        if d.is_none() && !s.is_none() {
            *d = s;
        }
    }
    own
}

// ---------------------------------------------------------------- built-in PivotStyleLight/Medium/Dark

/// Parses a built-in PivotTable style name (e.g. `PivotStyleMedium9` → `("Medium", 9)`).
pub fn parse_pivot_style_name(name: &str) -> Option<(&'static str, u32)> {
    let rest = name.strip_prefix("PivotStyle").unwrap_or(name);
    for fam in ["Light", "Medium", "Dark"] {
        if let Some(num) = rest.strip_prefix(fam)
            && let Ok(n) = num.parse::<u32>()
            && n >= 1
        {
            return Some((fam, n));
        }
    }
    None
}

/// Palette for a built-in `PivotStyleLight1…28 / Medium1…28 / Dark1…28` style.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BuiltinPivotLook {
    pub whole_table: Option<Color>,
    pub header_fill: Option<Color>,
    pub header_font: Color,
    pub header_bottom: Option<Color>,
    pub band1: Option<Color>,
    pub band2: Option<Color>,
    pub body_font: Color,
    pub subtotal_fill: Option<Color>,
    pub subtotal_font: Color,
    pub subtotal_line: Option<Color>,
    pub total_fill: Option<Color>,
    pub total_font: Color,
    pub total_line: Option<Color>,
    pub outer_border: Option<Color>,
    pub inner_h: Option<Color>,
    pub inner_v: Option<Color>,
    pub filter_fill: Option<Color>,
}

/// Built-in palette for a PivotTable style name (`None` for unrecognized/custom names).
pub fn builtin_look(style: &str) -> Option<BuiltinPivotLook> {
    let (fam, n) = parse_pivot_style_name(style)?;
    let k = n.saturating_sub(1) % 7;
    let acc = if k == 0 { None } else { Some(3 + k as u8) };
    let a = |tint: i16, neutral: Color| acc.map_or(neutral, |a| Color::Theme(a, tint));
    let black = Color::Theme(1, 0);
    let white = Color::Theme(0, 0);
    let light_grey = Color::Theme(0, -150);
    let mid_grey = Color::Theme(0, -250);
    let dark_grey = Color::Theme(0, -350);
    let main = a(0, black);
    let pale = a(800, light_grey);
    let mid = a(600, mid_grey);
    let line = a(400, dark_grey);
    let group = (n.saturating_sub(1) / 7).min(3);

    let lk = match (fam, group) {
        // Light 1-7: clean header with top/bottom rules, subtle banded rows.
        ("Light", 0) => BuiltinPivotLook {
            whole_table: None,
            header_fill: None,
            header_font: black,
            header_bottom: Some(main),
            band1: Some(pale),
            band2: None,
            body_font: black,
            subtotal_fill: None,
            subtotal_font: black,
            subtotal_line: Some(line),
            total_fill: None,
            total_font: black,
            total_line: Some(main),
            outer_border: Some(main),
            inner_h: None,
            inner_v: None,
            filter_fill: Some(pale),
        },
        // Light 8-14: solid accent header, white bold text, horizontal grid lines.
        ("Light", 1) => BuiltinPivotLook {
            whole_table: None,
            header_fill: Some(main),
            header_font: white,
            header_bottom: Some(main),
            band1: Some(pale),
            band2: None,
            body_font: black,
            subtotal_fill: Some(pale),
            subtotal_font: black,
            subtotal_line: Some(line),
            total_fill: Some(pale),
            total_font: black,
            total_line: Some(main),
            outer_border: Some(main),
            inner_h: Some(line),
            inner_v: None,
            filter_fill: Some(pale),
        },
        // Light 15-21 (Light16 = Excel default): soft pale header, light stripe, tinted total.
        ("Light", 2) => BuiltinPivotLook {
            whole_table: None,
            header_fill: Some(pale),
            header_font: black,
            header_bottom: Some(line),
            band1: Some(a(800, Color::Theme(0, -50))),
            band2: None,
            body_font: black,
            subtotal_fill: Some(pale),
            subtotal_font: black,
            subtotal_line: Some(line),
            total_fill: Some(pale),
            total_font: black,
            total_line: Some(line),
            outer_border: None,
            inner_h: None,
            inner_v: None,
            filter_fill: Some(pale),
        },
        // Light 22-28: clean header with outer frame and vertical dividers.
        ("Light", _) => BuiltinPivotLook {
            whole_table: None,
            header_fill: None,
            header_font: black,
            header_bottom: Some(main),
            band1: Some(pale),
            band2: None,
            body_font: black,
            subtotal_fill: Some(pale),
            subtotal_font: black,
            subtotal_line: Some(line),
            total_fill: None,
            total_font: black,
            total_line: Some(main),
            outer_border: Some(main),
            inner_h: None,
            inner_v: Some(line),
            filter_fill: Some(pale),
        },
        // Medium 1-7: solid accent header, white text, light bands, framed outer border.
        ("Medium", 0) => BuiltinPivotLook {
            whole_table: None,
            header_fill: Some(main),
            header_font: white,
            header_bottom: Some(main),
            band1: Some(pale),
            band2: None,
            body_font: black,
            subtotal_fill: Some(pale),
            subtotal_font: black,
            subtotal_line: Some(line),
            total_fill: Some(pale),
            total_font: black,
            total_line: Some(main),
            outer_border: Some(line),
            inner_h: None,
            inner_v: None,
            filter_fill: Some(pale),
        },
        // Medium 8-14 (Medium9 = classic dark blue header + banded rows + bold accent total).
        ("Medium", 1) => BuiltinPivotLook {
            whole_table: None,
            header_fill: Some(main),
            header_font: white,
            header_bottom: Some(main),
            band1: Some(pale),
            band2: None,
            body_font: black,
            subtotal_fill: Some(mid),
            subtotal_font: black,
            subtotal_line: Some(line),
            total_fill: Some(main),
            total_font: white,
            total_line: Some(main),
            outer_border: None,
            inner_h: None,
            inner_v: None,
            filter_fill: Some(pale),
        },
        // Medium 15-21: solid accent header, neutral grey bands, bold accent total.
        ("Medium", 2) => BuiltinPivotLook {
            whole_table: None,
            header_fill: Some(main),
            header_font: white,
            header_bottom: Some(black),
            band1: Some(light_grey),
            band2: None,
            body_font: black,
            subtotal_fill: Some(mid_grey),
            subtotal_font: black,
            subtotal_line: Some(black),
            total_fill: Some(main),
            total_font: white,
            total_line: Some(black),
            outer_border: Some(black),
            inner_h: None,
            inner_v: None,
            filter_fill: Some(light_grey),
        },
        // Medium 22-28: pale header with grid lines, two-tone bands.
        ("Medium", _) => BuiltinPivotLook {
            whole_table: Some(pale),
            header_fill: Some(pale),
            header_font: black,
            header_bottom: Some(line),
            band1: Some(mid),
            band2: Some(pale),
            body_font: black,
            subtotal_fill: Some(a(400, dark_grey)),
            subtotal_font: black,
            subtotal_line: Some(line),
            total_fill: Some(mid),
            total_font: black,
            total_line: Some(line),
            outer_border: Some(line),
            inner_h: None,
            inner_v: Some(line),
            filter_fill: Some(pale),
        },
        // Dark 1-7: black header, dark accent body bands with white text, black total.
        ("Dark", 0) => BuiltinPivotLook {
            whole_table: Some(a(0, Color::Theme(1, 450))),
            header_fill: Some(black),
            header_font: white,
            header_bottom: Some(main),
            band1: Some(a(-250, Color::Theme(1, 250))),
            band2: Some(a(0, Color::Theme(1, 450))),
            body_font: white,
            subtotal_fill: Some(a(-500, black)),
            subtotal_font: white,
            subtotal_line: Some(white),
            total_fill: Some(black),
            total_font: white,
            total_line: Some(white),
            outer_border: None,
            inner_h: None,
            inner_v: None,
            filter_fill: Some(mid),
        },
        // Dark 8-14: solid accent header, mid/pale body bands, solid accent total.
        ("Dark", 1) => BuiltinPivotLook {
            whole_table: Some(pale),
            header_fill: Some(main),
            header_font: white,
            header_bottom: Some(white),
            band1: Some(mid),
            band2: Some(pale),
            body_font: black,
            subtotal_fill: Some(a(400, dark_grey)),
            subtotal_font: black,
            subtotal_line: Some(main),
            total_fill: Some(main),
            total_font: white,
            total_line: Some(main),
            outer_border: None,
            inner_h: None,
            inner_v: None,
            filter_fill: Some(mid),
        },
        // Dark 15-21: black header, mid/pale body bands, black total.
        ("Dark", 2) => BuiltinPivotLook {
            whole_table: Some(pale),
            header_fill: Some(black),
            header_font: white,
            header_bottom: Some(main),
            band1: Some(mid),
            band2: Some(pale),
            body_font: black,
            subtotal_fill: Some(a(400, dark_grey)),
            subtotal_font: black,
            subtotal_line: Some(black),
            total_fill: Some(black),
            total_font: white,
            total_line: Some(black),
            outer_border: None,
            inner_h: None,
            inner_v: None,
            filter_fill: Some(mid),
        },
        // Dark 22-28: solid accent header, rich dark bands, solid accent total.
        _ => BuiltinPivotLook {
            whole_table: Some(a(0, Color::Theme(1, 450))),
            header_fill: Some(main),
            header_font: white,
            header_bottom: Some(white),
            band1: Some(a(-250, Color::Theme(1, 250))),
            band2: Some(a(0, Color::Theme(1, 450))),
            body_font: white,
            subtotal_fill: Some(a(-500, black)),
            subtotal_font: white,
            subtotal_line: Some(white),
            total_fill: Some(main),
            total_font: white,
            total_line: Some(white),
            outer_border: None,
            inner_h: None,
            inner_v: None,
            filter_fill: Some(mid),
        },
    };
    Some(lk)
}

fn thin(c: Option<Color>) -> BorderLine {
    c.map(|color| BorderLine { style: BorderStyle::Thin, color }).unwrap_or_default()
}

fn resolve_builtin_style(name: &str, pt: &PivotTable, layout: &PivotStyleLayout, body: RangeRef, c: CellRef) -> Option<Style> {
    let role = classify_cell(pt, layout, body, c)?;
    let lk = builtin_look(name)?;
    let opts = pt.style_options;
    let mut s = Style::default();

    if !role.in_body {
        if let Some(fill) = lk.filter_fill {
            s.fill = Fill::solid(fill);
        }
        if role.page_label {
            s.font.bold = true;
        }
        return Some(s);
    }

    if role.is_blank_row {
        return Some(s);
    }

    apply_builtin_borders(&mut s.borders, &lk, body, c, &role, opts);
    apply_builtin_fill_and_font(&mut s, &lk, &role, opts);
    Some(s)
}

fn apply_builtin_borders(b: &mut Borders, lk: &BuiltinPivotLook, body: RangeRef, c: CellRef, role: &CellRole, opts: PivotStyleOptions) {
    if c.row == body.start.row {
        b.top = thin(lk.outer_border);
    }
    if c.row == body.end.row {
        b.bottom = thin(lk.outer_border);
    }
    if c.col == body.start.col {
        b.left = thin(lk.outer_border);
    }
    if c.col == body.end.col {
        b.right = thin(lk.outer_border);
    }
    if role.is_header && opts.show_col_headers {
        if let Some(hb) = lk.header_bottom {
            b.bottom = BorderLine { style: BorderStyle::Thin, color: hb };
        }
    } else if role.is_total_row {
        if let Some(tl) = lk.total_line {
            b.top = BorderLine { style: BorderStyle::Thin, color: tl };
            b.bottom = BorderLine { style: BorderStyle::Thin, color: tl };
        }
    } else if role.subtotal_row.is_some() {
        if let Some(sl) = lk.subtotal_line {
            b.top = BorderLine { style: BorderStyle::Thin, color: sl };
            b.bottom = BorderLine { style: BorderStyle::Thin, color: sl };
        }
    } else {
        if b.bottom.is_none() {
            b.bottom = thin(lk.inner_h);
        }
        if c.col < body.end.col && b.right.is_none() {
            b.right = thin(lk.inner_v);
        }
    }
}

fn apply_builtin_fill_and_font(s: &mut Style, lk: &BuiltinPivotLook, role: &CellRole, opts: PivotStyleOptions) {
    s.font.color = lk.body_font;
    if role.is_header && opts.show_col_headers {
        s.font.bold = true;
        s.font.color = lk.header_font;
        if let Some(f) = lk.header_fill {
            s.fill = Fill::solid(f);
        }
        return;
    }
    if role.is_total_row {
        s.font.bold = true;
        s.font.color = lk.total_font;
        if let Some(f) = lk.total_fill {
            s.fill = Fill::solid(f);
        }
        return;
    }
    if role.subtotal_row.is_some() {
        s.font.bold = true;
        s.font.color = lk.subtotal_font;
        if let Some(f) = lk.subtotal_fill {
            s.fill = Fill::solid(f);
        }
        return;
    }
    if let Some(f) = lk.whole_table {
        s.fill = Fill::solid(f);
    }
    if opts.show_row_stripes
        && role.has_data_cols
        && let Some(f) = if role.data_row.is_multiple_of(2) { lk.band1 } else { lk.band2 }
    {
        s.fill = Fill::solid(f);
    }
    if opts.show_col_stripes
        && let Some(f) = if role.rel_col.is_multiple_of(2) { lk.band1 } else { lk.band2 }
    {
        s.fill = Fill::solid(f);
    }
    if (opts.show_row_headers && (role.is_first_col || role.row_subheading.is_some()))
        || (opts.show_last_column && role.is_total_col)
        || role.subtotal_col.is_some()
    {
        s.font.bold = true;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use gridcraft_core::{CellRef, RangeRef, Value};
    use gridcraft_model::{
        BorderLine, BorderStyle, Cell, Color, Fill, PatternType, PivotColRole, PivotCustomStyle, PivotRowRole, PivotStyleElement, PivotStyleGrid,
        PivotStyleLayout, PivotStyleOptions, PivotTable, Style, Workbook,
    };

    use super::*;

    fn sample_workbook_with_pivot(pt: PivotTable) -> Workbook {
        let mut wb = Workbook::new();
        let sh = wb.sheet_mut(0).unwrap();
        for r in 0..=10 {
            for c in 0..=4 {
                sh.set_cell(CellRef::new(r, c), Cell::value(Value::text(format!("R{r}C{c}"))));
            }
        }
        sh.pivots.push(pt);
        wb
    }

    fn base_layout() -> PivotStyleLayout {
        // Page filter on row 0, blank on row 1, body A3:D9 (rows 2..=8, cols 0..=3):
        // row 2: header
        // row 3: data 0
        // row 4: data 1
        // row 5: subtotal 0 (data idx 2)
        // row 6: data 2 (data idx 3 -> odd!)
        // row 7: blank (data idx 4)
        // row 8: grand total (data idx 5)
        PivotStyleLayout {
            body_range: Some(RangeRef::new(CellRef::new(2, 0), CellRef::new(8, 3))),
            first_header_row: 0,
            first_data_row: 1,
            first_data_col: 1,
            page_rows: 1,
            page_cols: 1,
            row_roles: vec![
                PivotRowRole::Data,
                PivotRowRole::Data,
                PivotRowRole::Subtotal(0),
                PivotRowRole::Data,
                PivotRowRole::Blank,
                PivotRowRole::GrandTotal,
            ],
            row_subheading: vec![None; 6],
            col_roles: vec![PivotColRole::Data, PivotColRole::Subtotal(0), PivotColRole::GrandTotal],
            col_subheading: vec![None; 3],
        }
    }

    #[test]
    fn builtin_medium_light_dark_styles_apply_headers_stripes_subtotals_and_grand_totals() {
        for (name, expect_white_header, expect_dark_total) in
            [("PivotStyleLight16", false, false), ("PivotStyleMedium9", true, true), ("PivotStyleDark2", true, true)]
        {
            let pt = PivotTable {
                style: name.into(),
                style_options: PivotStyleOptions {
                    show_row_headers: true,
                    show_col_headers: true,
                    show_row_stripes: true,
                    show_col_stripes: false,
                    show_last_column: true,
                },
                anchor: CellRef::new(2, 0),
                last_range: Some(RangeRef::new(CellRef::new(0, 0), CellRef::new(8, 3))),
                style_grid: PivotStyleGrid(Some(Arc::new(base_layout()))),
                ..PivotTable::default()
            };
            let wb = sample_workbook_with_pivot(pt);
            let sh = wb.sheet(0).unwrap();
            let p = &sh.pivots[0];

            // Page filter label (row 0, col 0) vs blank row (row 1, col 0).
            let flt = pivot_cell_style(&wb, sh, p, CellRef::new(0, 0));
            assert!(flt.font.bold, "{name}: page filter label should be bold");
            assert_eq!(flt.fill.pattern, PatternType::Solid, "{name}: page filter should have fill");
            let blank_gap = pivot_cell_style(&wb, sh, p, CellRef::new(1, 0));
            assert_eq!(blank_gap.fill.pattern, PatternType::None, "{name}: blank separator row should stay unstyled");

            // Header row (row 2).
            let hdr = pivot_cell_style(&wb, sh, p, CellRef::new(2, 1));
            assert!(hdr.font.bold, "{name}: header bold");
            assert_eq!(hdr.fill.pattern, PatternType::Solid, "{name}: header solid fill");
            if expect_white_header {
                assert_eq!(hdr.font.color, Color::Theme(0, 0), "{name}: white header text");
            }

            // Row stripe parity: data row 0 (row 3) vs data row 1 (row 4).
            let r0 = pivot_cell_style(&wb, sh, p, CellRef::new(3, 1));
            let r1 = pivot_cell_style(&wb, sh, p, CellRef::new(4, 1));
            assert_eq!(r0.fill.pattern, PatternType::Solid, "{name}: even row stripe has fill");
            assert_ne!(r0.fill, r1.fill, "{name}: alternating row stripes differ");

            // Row header column (col 0) bold when show_row_headers=true; grand total column (col 3) bold.
            assert!(pivot_cell_style(&wb, sh, p, CellRef::new(3, 0)).font.bold);
            assert!(pivot_cell_style(&wb, sh, p, CellRef::new(3, 3)).font.bold);
            assert!(!r0.font.bold, "{name}: ordinary data cell not bold");

            // Subtotal row (row 5).
            let sub = pivot_cell_style(&wb, sh, p, CellRef::new(5, 1));
            assert!(sub.font.bold, "{name}: subtotal row bold");
            assert!(!sub.borders.top.is_none() && !sub.borders.bottom.is_none(), "{name}: subtotal borders");

            // Grand total row (row 8).
            let grand = pivot_cell_style(&wb, sh, p, CellRef::new(8, 1));
            assert!(grand.font.bold, "{name}: grand total bold");
            if expect_dark_total {
                assert_eq!(grand.font.color, Color::Theme(0, 0), "{name}: grand total white font");
            }
        }
    }

    #[test]
    fn empty_style_leaves_cells_unchanged_and_explicit_cell_styles_win() {
        let mut pt = PivotTable {
            style: String::new(),
            anchor: CellRef::new(2, 0),
            last_range: Some(RangeRef::new(CellRef::new(2, 0), CellRef::new(8, 3))),
            style_grid: PivotStyleGrid(Some(Arc::new(base_layout()))),
            ..PivotTable::default()
        };
        let mut wb = sample_workbook_with_pivot(pt.clone());
        let own_id = wb.styles.derive(gridcraft_model::StyleId::DEFAULT, |s| {
            s.fill = Fill::solid(Color::Rgb(0xFFE599));
            s.font.color = Color::Rgb(0xAA0000);
            s.font.size = 14.0;
            s.borders.bottom = BorderLine { style: BorderStyle::Medium, color: Color::Rgb(0x112233) };
        });
        wb.sheet_mut(0).unwrap().set_style(CellRef::new(2, 1), own_id);

        // Empty style name ("None"): returns own style verbatim.
        let sh = wb.sheet(0).unwrap();
        let s_none = pivot_cell_style(&wb, sh, &sh.pivots[0], CellRef::new(2, 1));
        assert_eq!(s_none, *wb.styles.get(own_id));

        // Styled PivotTable: explicit fill, font color, size, and bottom border on cell win.
        pt.style = "PivotStyleMedium9".into();
        wb.sheet_mut(0).unwrap().pivots[0] = pt;
        let sh = wb.sheet(0).unwrap();
        let s_styled = pivot_cell_style(&wb, sh, &sh.pivots[0], CellRef::new(2, 1));
        assert_eq!(s_styled.fill, Fill::solid(Color::Rgb(0xFFE599)));
        assert_eq!(s_styled.font.color, Color::Rgb(0xAA0000));
        assert_eq!(s_styled.font.size, 14.0);
        assert_eq!(s_styled.borders.bottom, BorderLine { style: BorderStyle::Medium, color: Color::Rgb(0x112233) });
        assert!(s_styled.font.bold, "header bold still merges when cell wasn't bold");
    }

    #[test]
    fn custom_table_style_layer_precedence_and_stripe_parity_across_subtotals() {
        let mut whole = Style::default();
        whole.font.color = Color::Theme(3, 0);

        let mut hdr = Style::default();
        hdr.font.bold = true;
        hdr.font.color = Color::Theme(0, 0);
        hdr.fill = Fill::solid(Color::Theme(2, 0));
        hdr.borders.bottom = BorderLine { style: BorderStyle::Thin, color: Color::Theme(4, 0) };

        let mut stripe1 = Style::default();
        stripe1.font.color = Color::Theme(1, 0);
        stripe1.dxf_stated.bold = true; // explicitly un-bold
        stripe1.fill = Fill::solid(Color::Theme(0, -50));

        let mut stripe2 = Style::default();
        stripe2.font.color = Color::Theme(1, 0);
        stripe2.fill = Fill::solid(Color::Theme(2, 800));

        let mut sub1 = Style::default();
        sub1.font.bold = true;
        sub1.font.color = Color::Theme(1, 0);
        sub1.fill = Fill::solid(Color::Theme(0, -150));
        sub1.borders.top = BorderLine { style: BorderStyle::Thin, color: Color::Theme(4, 0) };
        sub1.borders.bottom = BorderLine { style: BorderStyle::Thin, color: Color::Theme(4, 0) };

        let mut total = Style::default();
        total.font.bold = true;
        total.font.color = Color::Theme(1, 0);
        total.fill = Fill::solid(Color::Theme(0, -50));
        total.borders.top = BorderLine { style: BorderStyle::Thin, color: Color::Theme(2, 400) };
        total.borders.bottom = BorderLine { style: BorderStyle::Thin, color: Color::Theme(2, 400) };

        let elems = vec![
            PivotStyleElement { kind: "wholeTable".into(), size: 1, style: whole },
            PivotStyleElement { kind: "headerRow".into(), size: 1, style: hdr },
            PivotStyleElement { kind: "firstRowStripe".into(), size: 1, style: stripe1 },
            PivotStyleElement { kind: "secondRowStripe".into(), size: 1, style: stripe2 },
            PivotStyleElement { kind: "firstSubtotalRow".into(), size: 1, style: sub1 },
            PivotStyleElement { kind: "totalRow".into(), size: 1, style: total },
        ];

        let pt = PivotTable {
            style: "SyntheticCustom".into(),
            style_options: PivotStyleOptions {
                show_row_headers: false,
                show_col_headers: true,
                show_row_stripes: true,
                show_col_stripes: false,
                show_last_column: true,
            },
            custom_style: PivotCustomStyle(Some(Arc::new(elems))),
            anchor: CellRef::new(2, 0),
            last_range: Some(RangeRef::new(CellRef::new(0, 0), CellRef::new(8, 3))),
            style_grid: PivotStyleGrid(Some(Arc::new(base_layout()))),
            ..PivotTable::default()
        };
        let wb = sample_workbook_with_pivot(pt);
        let sh = wb.sheet(0).unwrap();
        let p = &sh.pivots[0];

        // Header gets white bold text and dark blue fill (Theme 2).
        let h = pivot_cell_style(&wb, sh, p, CellRef::new(2, 0));
        assert!(h.font.bold);
        assert_eq!(h.font.color, Color::Theme(0, 0));
        assert_eq!(h.fill, Fill::solid(Color::Theme(2, 0)));

        // Data row 0 (row 3) -> firstRowStripe, black text overrides wholeTable cyan.
        let d0 = pivot_cell_style(&wb, sh, p, CellRef::new(3, 0));
        assert!(!d0.font.bold);
        assert_eq!(d0.font.color, Color::Theme(1, 0));
        assert_eq!(d0.fill, Fill::solid(Color::Theme(0, -50)));

        // Data row 1 (row 4) -> secondRowStripe.
        let d1 = pivot_cell_style(&wb, sh, p, CellRef::new(4, 0));
        assert_eq!(d1.fill, Fill::solid(Color::Theme(2, 800)));

        // Subtotal row (row 5, data index 2) -> firstSubtotalRow overrides stripe.
        let s0 = pivot_cell_style(&wb, sh, p, CellRef::new(5, 0));
        assert!(s0.font.bold);
        assert_eq!(s0.fill, Fill::solid(Color::Theme(0, -150)));
        assert_eq!(s0.borders.top.style, BorderStyle::Thin);
        assert_eq!(s0.borders.bottom.style, BorderStyle::Thin);

        // Row 6 (data index 3, right after subtotal at index 2) -> odd -> secondRowStripe!
        let d3 = pivot_cell_style(&wb, sh, p, CellRef::new(6, 0));
        assert_eq!(d3.fill, Fill::solid(Color::Theme(2, 800)));

        // Grand total row (row 8) -> totalRow.
        let g = pivot_cell_style(&wb, sh, p, CellRef::new(8, 0));
        assert!(g.font.bold);
        assert_eq!(g.fill, Fill::solid(Color::Theme(0, -50)));
        assert_eq!(g.borders.top.color, Color::Theme(2, 400));
    }

    #[test]
    fn pdf_export_renders_styled_imported_pivot_table_fills() {
        let mut pt = PivotTable {
            style: "PivotStyleMedium9".into(),
            style_options: PivotStyleOptions {
                show_row_headers: false,
                show_col_headers: true,
                show_row_stripes: true,
                show_col_stripes: false,
                show_last_column: true,
            },
            anchor: CellRef::new(2, 0),
            last_range: Some(RangeRef::new(CellRef::new(2, 0), CellRef::new(8, 3))),
            style_grid: PivotStyleGrid(Some(Arc::new(base_layout()))),
            ..PivotTable::default()
        };
        pt.style_grid.0 = Some(Arc::new(PivotStyleLayout { page_rows: 0, ..base_layout() }));
        let wb = sample_workbook_with_pivot(pt);
        let lays = crate::cmd::print::layouts(&wb, &crate::cmd::print::PrintOptions::default());
        let pdf = crate::cmd::print::render_pdf(&wb, &lays, "Book", "").unwrap();
        let text = String::from_utf8_lossy(&pdf.bytes);
        // Header fill (Theme Accent 2 = default #ED7D31 -> 0.929 0.49 0.192 rg) and white text (1 1 1 rg)
        // must appear in the emitted PDF stream even though the cells themselves have default style.
        assert!(text.contains("1 1 1 rg"), "expected white header text color in PDF stream");
        assert!(text.contains(" re f"), "expected solid fill rectangles for pivot header/stripes in PDF stream");
    }

    #[test]
    fn row_stripes_are_omitted_when_pivot_has_no_data_columns() {
        let hdr = Style { fill: Fill::solid(Color::Theme(2, 0)), ..Style::default() };
        let stripe1 = Style { fill: Fill::solid(Color::Theme(0, -50)), ..Style::default() };
        let stripe2 = Style { fill: Fill::solid(Color::Theme(2, 800)), ..Style::default() };
        let total = Style { fill: Fill::solid(Color::Theme(0, -150)), ..Style::default() };
        let elems = vec![
            PivotStyleElement { kind: "headerRow".into(), size: 1, style: hdr },
            PivotStyleElement { kind: "firstRowStripe".into(), size: 1, style: stripe1 },
            PivotStyleElement { kind: "secondRowStripe".into(), size: 1, style: stripe2 },
            PivotStyleElement { kind: "totalRow".into(), size: 1, style: total },
        ];
        // 4 columns A..D, all row fields (first_data_col == 4 == body_cols).
        let row_only_layout = PivotStyleLayout {
            body_range: Some(RangeRef::new(CellRef::new(2, 0), CellRef::new(6, 3))),
            first_header_row: 0,
            first_data_row: 1,
            first_data_col: 4,
            page_rows: 0,
            page_cols: 1,
            row_roles: vec![PivotRowRole::Data, PivotRowRole::Data, PivotRowRole::Data, PivotRowRole::GrandTotal],
            row_subheading: vec![None; 4],
            col_roles: vec![],
            col_subheading: vec![],
        };
        let pt = PivotTable {
            style: "CustomRowOnly".into(),
            style_options: PivotStyleOptions {
                show_row_headers: false,
                show_col_headers: true,
                show_row_stripes: true,
                show_col_stripes: false,
                show_last_column: true,
            },
            custom_style: PivotCustomStyle(Some(Arc::new(elems))),
            anchor: CellRef::new(2, 0),
            last_range: Some(RangeRef::new(CellRef::new(2, 0), CellRef::new(6, 3))),
            style_grid: PivotStyleGrid(Some(Arc::new(row_only_layout))),
            ..PivotTable::default()
        };
        let wb = sample_workbook_with_pivot(pt);
        let sh = wb.sheet(0).unwrap();
        let p = &sh.pivots[0];
        // Header and total row still receive their fills; data rows stay plain white (no stripes).
        assert_eq!(pivot_cell_style(&wb, sh, p, CellRef::new(2, 0)).fill, Fill::solid(Color::Theme(2, 0)));
        assert_eq!(pivot_cell_style(&wb, sh, p, CellRef::new(3, 0)).fill.pattern, PatternType::None);
        assert_eq!(pivot_cell_style(&wb, sh, p, CellRef::new(4, 0)).fill.pattern, PatternType::None);
        assert_eq!(pivot_cell_style(&wb, sh, p, CellRef::new(6, 0)).fill, Fill::solid(Color::Theme(0, -150)));
    }

    #[test]
    fn builtin_stripes_respect_individual_flags_and_unknown_style_returns_none() {
        assert!(builtin_look("UnrecognizedStyleName").is_none());
        let pt = PivotTable {
            style: "PivotStyleMedium22".into(),
            style_options: PivotStyleOptions {
                show_row_headers: false,
                show_col_headers: true,
                show_row_stripes: false,
                show_col_stripes: false,
                show_last_column: false,
            },
            anchor: CellRef::new(2, 0),
            last_range: Some(RangeRef::new(CellRef::new(2, 0), CellRef::new(8, 3))),
            style_grid: PivotStyleGrid(Some(Arc::new(PivotStyleLayout { page_rows: 0, ..base_layout() }))),
            ..PivotTable::default()
        };
        let wb = sample_workbook_with_pivot(pt);
        let sh = wb.sheet(0).unwrap();
        let p = &sh.pivots[0];
        let lk = builtin_look("PivotStyleMedium22").unwrap();
        // Both stripe flags off: data rows get whole_table fill uniformly, not band1/band2 alternation.
        let d0 = pivot_cell_style(&wb, sh, p, CellRef::new(3, 1));
        let d1 = pivot_cell_style(&wb, sh, p, CellRef::new(4, 1));
        assert_eq!(d0.fill, Fill::solid(lk.whole_table.unwrap()));
        assert_eq!(d0.fill, d1.fill);

        // Empty custom style must NOT fall back to Light16.
        let empty_custom = PivotTable {
            style: "EmptyCustom".into(),
            custom_style: PivotCustomStyle(Some(Arc::new(vec![]))),
            anchor: CellRef::new(2, 0),
            last_range: Some(RangeRef::new(CellRef::new(2, 0), CellRef::new(8, 3))),
            style_grid: PivotStyleGrid(Some(Arc::new(PivotStyleLayout { page_rows: 0, ..base_layout() }))),
            ..PivotTable::default()
        };
        let wb2 = sample_workbook_with_pivot(empty_custom);
        let sh2 = wb2.sheet(0).unwrap();
        assert_eq!(pivot_cell_style(&wb2, sh2, &sh2.pivots[0], CellRef::new(3, 1)).fill.pattern, PatternType::None);
    }

    #[test]
    fn custom_style_dispatches_first_header_cell_col_subheading_and_col_stripes() {
        let hdr = Style { fill: Fill::solid(Color::Rgb(0x112233)), ..Style::default() };
        let first_hdr = Style { fill: Fill::solid(Color::Rgb(0x445566)), ..Style::default() };
        let col_sub = Style { fill: Fill::solid(Color::Rgb(0x778899)), ..Style::default() };
        let col_stripe = Style { fill: Fill::solid(Color::Rgb(0xAABBCC)), ..Style::default() };
        let elems = vec![
            PivotStyleElement { kind: "headerRow".into(), size: 1, style: hdr },
            PivotStyleElement { kind: "firstHeaderCell".into(), size: 1, style: first_hdr },
            PivotStyleElement { kind: "firstColumnSubheading".into(), size: 1, style: col_sub },
            PivotStyleElement { kind: "firstColumnStripe".into(), size: 1, style: col_stripe },
        ];
        let layout = PivotStyleLayout { page_rows: 0, col_subheading: vec![Some(0), None, None], ..base_layout() };
        let pt = PivotTable {
            style: "CustomFull".into(),
            style_options: PivotStyleOptions {
                show_row_headers: false,
                show_col_headers: true,
                show_row_stripes: false,
                show_col_stripes: true,
                show_last_column: false,
            },
            custom_style: PivotCustomStyle(Some(Arc::new(elems))),
            anchor: CellRef::new(2, 0),
            last_range: Some(RangeRef::new(CellRef::new(2, 0), CellRef::new(8, 3))),
            style_grid: PivotStyleGrid(Some(Arc::new(layout))),
            ..PivotTable::default()
        };
        let wb = sample_workbook_with_pivot(pt);
        let sh = wb.sheet(0).unwrap();
        let p = &sh.pivots[0];
        assert_eq!(pivot_cell_style(&wb, sh, p, CellRef::new(2, 0)).fill, Fill::solid(Color::Rgb(0x445566)));
        assert_eq!(pivot_cell_style(&wb, sh, p, CellRef::new(2, 1)).fill, Fill::solid(Color::Rgb(0x778899)));
        assert_eq!(pivot_cell_style(&wb, sh, p, CellRef::new(2, 2)).fill, Fill::solid(Color::Rgb(0x112233)));
        assert_eq!(pivot_cell_style(&wb, sh, p, CellRef::new(3, 0)).fill, Fill::solid(Color::Rgb(0xAABBCC)));
        assert_eq!(pivot_cell_style(&wb, sh, p, CellRef::new(3, 1)).fill.pattern, PatternType::None);
    }
}
