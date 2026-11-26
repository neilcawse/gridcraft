//! Form controls (buttons, scroll bars, spin buttons, drop-down lists, check boxes) in xlsx, from
//! the published ECMA-376, MS-XLSX and MS-OI29500 formats. A control is written three ways, as
//! spreadsheet applications expect:
//!
//! - a control properties part (`xl/ctrlProps/ctrlPropN.xml`, `formControlPr`) with its type,
//!   linked cell, limits and value (a drop-down: its source range, chosen item and rows; a check
//!   box: its state and shading);
//! - a `<control>` in the sheet's `<controls>`, inside `mc:AlternateContent` (it needs the 2010
//!   extensions), naming the properties part, the VML shape and the anchor;
//! - a legacy VML shape with `x:ClientData`, in the sheet's VML drawing (shared with notes).
//!
//! Excel also writes a hidden DrawingML shape for each control (`cNvPr hidden="1"` with an
//! `a14:compatExt spid` naming the VML shape) for applications that don't know controls. The
//! drawing reader drops that stand-in for every control read here ([`ReadControls::modelled`]),
//! so it doesn't also become a shape, and the writer writes one for each control
//! (`drawing::write_drawing`), so the sheet keeps its drawing. The stand-ins of controls left to
//! pass-through are read as shapes, as before.
//!
//! Reading prefers the properties part and falls back to the VML. Values are clamped to the
//! 0..30000 whole numbers sliders and spin buttons take, and a drop-down's chosen item and rows
//! to the same range. Controls of other kinds (list boxes, option buttons, group boxes, labels,
//! ActiveX controls), drop-downs with their own item list or a macro, and drop-downs and check
//! boxes with settings the model doesn't hold (another look or a rich-text caption) are left
//! to pass-through (`passthrough.rs`), which keeps them unchanged; a sheet can
//! hold both, in one `<controls>` element and one VML drawing. A check box's macro is kept as the
//! file wrote it, and never run.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use gridcraft_core::CellRef;
use gridcraft_model::{
    Anchor, AnchorMode, CONTROL_MAX, CaptionMarkup, CheckSheetAutoFlags, CheckState, CheckVmlLook, ControlAction, ControlKind, DROP_LINES_DEFAULT,
    DropDisplay, DropStyle, FormControl, MacroInput, Sheet,
};

use crate::package::Rel;
use crate::read::Ctx;
use crate::tables::EMU_PER_PX;
use crate::write::{Out, Rels};
use crate::xml::{El, esc, esc_attr};

pub const CT_CTRL_PROP: &str = "application/vnd.ms-excel.controlproperties+xml";
/// The worksheet relationship to a control properties part (MS-XLSX), as Excel writes it.
pub const REL_CTRL_PROP: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/ctrlProp";
const NS_X14: &str = "http://schemas.microsoft.com/office/spreadsheetml/2009/9/main";
const NS_MC: &str = "http://schemas.openxmlformats.org/markup-compatibility/2006";
const NS_XDR: &str = "http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing";

/// The most controls read from one sheet.
const MAX_CONTROLS: usize = 10_000;

/// The control's type as the properties part spells it (`objectType`).
fn object_type(k: ControlKind) -> &'static str {
    match k {
        ControlKind::Button => "Button",
        ControlKind::Slider => "Scroll",
        ControlKind::Spin => "Spin",
        ControlKind::Drop => "Drop",
        ControlKind::Check => "CheckBox",
    }
}

/// The control's type as the VML spells it (`x:ClientData ObjectType`).
fn vml_object_type(k: ControlKind) -> &'static str {
    if k == ControlKind::Check { "Checkbox" } else { object_type(k) }
}

fn kind_of(t: &str) -> Option<ControlKind> {
    match t.trim().to_ascii_lowercase().as_str() {
        "button" => Some(ControlKind::Button),
        "scroll" => Some(ControlKind::Slider),
        "spin" => Some(ControlKind::Spin),
        "drop" => Some(ControlKind::Drop),
        "checkbox" => Some(ControlKind::Check),
        _ => None,
    }
}

/// A check box's `checked` (`Unchecked`, `Checked`, `Mixed`).
fn check_state_of(t: &str) -> Option<CheckState> {
    match t.trim().to_ascii_lowercase().as_str() {
        "unchecked" => Some(CheckState::Unchecked),
        "checked" => Some(CheckState::Checked),
        "mixed" => Some(CheckState::Mixed),
        _ => None,
    }
}

/// A check box's VML `x:Checked` (0 unchecked, 1 checked, 2 mixed).
fn vml_check_state_of(t: &str) -> Option<CheckState> {
    match t.trim() {
        "0" => Some(CheckState::Unchecked),
        "1" => Some(CheckState::Checked),
        "2" => Some(CheckState::Mixed),
        _ => None,
    }
}

/// A `dropStyle` (`combo`, `comboedit`, `simple`; the VML spells them `Combo`, `ComboEdit`,
/// `Simple`).
fn drop_style_of(t: &str) -> Option<DropStyle> {
    match t.trim().to_ascii_lowercase().as_str() {
        "combo" => Some(DropStyle::Combo),
        "comboedit" => Some(DropStyle::ComboEdit),
        "simple" => Some(DropStyle::Simple),
        _ => None,
    }
}

/// A drop style as the properties part spells it, and as the VML does.
fn drop_style_names(s: DropStyle) -> (&'static str, &'static str) {
    match s {
        DropStyle::Combo => ("combo", "Combo"),
        DropStyle::ComboEdit => ("comboedit", "ComboEdit"),
        DropStyle::Simple => ("simple", "Simple"),
    }
}

/// The macro a control runs, as the file writes it: a button's as Excel writes it, `[0]!Name`
/// (this workbook); a check box's as it was read.
fn macro_ref(c: &FormControl) -> Option<Cow<'_, str>> {
    if c.kind == ControlKind::Check {
        return c.kept_macro.as_deref().map(Cow::Borrowed);
    }
    match &c.action {
        Some(ControlAction::Macro { name, .. }) if !name.trim().is_empty() => Some(Cow::Owned(format!("[0]!{}", name.trim()))),
        _ => None,
    }
}

/// The macro name in `[0]!Name`, `Book1.xlsm!Name` or `Name`.
fn macro_name(text: &str) -> Option<String> {
    let name = text.rsplit_once('!').map_or(text, |(_, n)| n).trim().trim_matches('\'');
    (!name.is_empty() && name.chars().count() <= gridcraft_model::MAX_MACRO_NAME).then(|| name.to_string())
}

/// The cell under a sheet position and the offset into it: (cell, dx, dy).
fn cell_at(sheet: &Sheet, x: f64, y: f64) -> (CellRef, f64, f64) {
    let (col, row) = (sheet.col_at(x), sheet.row_at(y));
    (CellRef::new(row, col), x - sheet.col_left(col), y - sheet.row_top(row))
}

/// The two corners of a control: ((from, dx, dy), (to, dx, dy)), offsets in sheet pixels.
fn corners(sheet: &Sheet, a: &Anchor) -> ((CellRef, f64, f64), (CellRef, f64, f64)) {
    let x1 = sheet.col_left(a.cell.col) + a.dx.max(0.0) as f64;
    let y1 = sheet.row_top(a.cell.row) + a.dy.max(0.0) as f64;
    let from = (a.cell, a.dx.max(0.0) as f64, a.dy.max(0.0) as f64);
    (from, cell_at(sheet, x1 + a.width.max(0.0) as f64, y1 + a.height.max(0.0) as f64))
}

/// The sheet's controls written out: the `<control>` entries of the worksheet's `<controls>`
/// element (each in its own `mc:AlternateContent`, with the namespaces it needs, so it can also
/// join a kept `<controls>` element) and the VML shapes for its drawing.
pub struct Written {
    pub entries: String,
    pub vml: String,
}

/// The `<controls>` element for `entries` written by [`write`].
pub fn controls_element(entries: &str) -> String {
    format!(
        "<mc:AlternateContent xmlns:mc=\"{NS_MC}\"><mc:Choice xmlns:x14=\"{NS_X14}\" Requires=\"x14\"><controls>{entries}</controls></mc:Choice></mc:AlternateContent>"
    )
}

/// Writes the properties parts and relationships of the sheet's controls. `shape_id(i)` is
/// control `i`'s VML shape id.
pub fn write(out: &mut Out, rels: &mut Rels, sheet: &Sheet, shape_id: impl Fn(usize) -> u64) -> Written {
    let mut xml = String::new();
    let mut vml = String::new();
    for (i, c) in sheet.controls.iter().enumerate() {
        // Kept properties parts of another sheet may hold the next name.
        let n = loop {
            out.ctrl_props = out.ctrl_props.saturating_add(1);
            if out.kept.reserve(&format!("xl/ctrlProps/ctrlProp{}.xml", out.ctrl_props)) || out.ctrl_props == u32::MAX {
                break out.ctrl_props;
            }
        };
        let part = format!("xl/ctrlProps/ctrlProp{n}.xml");
        out.part(&part, Some(CT_CTRL_PROP), ctrl_prop(c).into_bytes());
        let rid = rels.add_uri(REL_CTRL_PROP, &format!("../ctrlProps/ctrlProp{n}.xml"));
        let id = shape_id(i);
        let ((from, fdx, fdy), (to, tdx, tdy)) = corners(sheet, &c.anchor);
        let emu = |v: f64| (v.max(0.0) * EMU_PER_PX).round() as i64;
        let marker = |tag: &str, cell: CellRef, dx: f64, dy: f64| {
            format!(
                "<{tag}><xdr:col>{}</xdr:col><xdr:colOff>{}</xdr:colOff><xdr:row>{}</xdr:row><xdr:rowOff>{}</xdr:rowOff></{tag}>",
                cell.col,
                emu(dx),
                cell.row,
                emu(dy)
            )
        };
        // A drop-down uses explicit line properties. A check box keeps its worksheet flags;
        // only when generating an entry from scratch do its flags follow the VML.
        let mut pr = String::from(match c.kind {
            ControlKind::Drop => " defaultSize=\"0\" autoLine=\"0\" autoPict=\"0\"",
            _ => " defaultSize=\"0\"",
        });
        if c.kind == ControlKind::Check {
            let flags = c.check_sheet_auto_flags.unwrap_or_else(|| {
                let look = c.check_vml_look.as_ref();
                CheckSheetAutoFlags { fill_off: look.is_none_or(|l| l.auto_fill_off), line_off: look.is_none_or(|l| l.auto_line_off) }
            });
            if flags.fill_off {
                pr.push_str(" autoFill=\"0\"");
            }
            if flags.line_off {
                pr.push_str(" autoLine=\"0\"");
            }
        }
        if c.kind != ControlKind::Drop {
            pr.push_str(" autoPict=\"0\"");
        }
        if !c.print {
            pr.push_str(" print=\"0\"");
        }
        if !c.locked {
            pr.push_str(" locked=\"0\"");
        }
        if let Some(m) = macro_ref(c) {
            let _ = write!(pr, " macro=\"{}\"", esc_attr(&m));
        }
        let (move_cells, size_cells) = match c.anchor.mode {
            AnchorMode::MoveAndSize => (true, true),
            AnchorMode::MoveOnly => (true, false),
            AnchorMode::Absolute => (false, false),
        };
        let name = esc_attr(&c.display_name());
        let _ = write!(
            xml,
            "<mc:AlternateContent xmlns:mc=\"{NS_MC}\"><mc:Choice xmlns:x14=\"{NS_X14}\" Requires=\"x14\"><control shapeId=\"{id}\" r:id=\"{rid}\" name=\"{name}\"><controlPr{pr}><anchor{}{} xmlns:xdr=\"{NS_XDR}\">{}{}</anchor></controlPr></control></mc:Choice><mc:Fallback><control shapeId=\"{id}\" r:id=\"{rid}\" name=\"{name}\"/></mc:Fallback></mc:AlternateContent>",
            if move_cells { " moveWithCells=\"1\"" } else { "" },
            if size_cells { " sizeWithCells=\"1\"" } else { "" },
            marker("from", from, fdx, fdy),
            marker("to", to, tdx, tdy),
        );
        vml.push_str(&vml_shape(sheet, c, id, i));
    }
    Written { entries: xml, vml }
}

/// The control properties part.
fn ctrl_prop(c: &FormControl) -> String {
    let mut s = format!("{}<formControlPr xmlns=\"{NS_X14}\" objectType=\"{}\"", crate::write::XML_DECL, object_type(c.kind));
    match c.kind {
        ControlKind::Button => {
            s.push_str(" lockText=\"1\"");
        }
        ControlKind::Slider | ControlKind::Spin => {
            s.push_str(" dx=\"22\"");
            if let Some(l) = &c.link {
                let _ = write!(s, " fmlaLink=\"{}\"", esc_attr(l));
            }
            if c.kind == ControlKind::Slider && c.horizontal {
                s.push_str(" horiz=\"1\"");
            }
            let _ = write!(s, " inc=\"{}\" max=\"{}\" min=\"{}\"", c.step, c.max, c.min);
            if c.kind == ControlKind::Slider {
                let _ = write!(s, " page=\"{}\"", c.page);
            }
            let _ = write!(s, " val=\"{}\"", c.value);
        }
        ControlKind::Drop => {
            let _ = write!(s, " dropLines=\"{}\" dropStyle=\"{}\"", c.drop_lines, drop_style_names(c.drop_style).0);
            if let Some(l) = &c.link {
                let _ = write!(s, " fmlaLink=\"{}\"", esc_attr(l));
            }
            if let Some(r) = &c.list_range {
                let _ = write!(s, " fmlaRange=\"{}\"", esc_attr(r));
            }
            if c.selected > 0 {
                let _ = write!(s, " sel=\"{}\"", c.selected);
            }
            let d = c.drop_display;
            for (name, value) in [("noThreeD", d.no_three_d), ("noThreeD2", d.no_three_d2)] {
                if let Some(value) = value {
                    let _ = write!(s, " {name}=\"{}\"", u8::from(value));
                }
            }
            for (name, value) in [("dx", d.scroll_width), ("val", d.scroll_offset)] {
                if let Some(value) = value {
                    let _ = write!(s, " {name}=\"{value}\"");
                }
            }
        }
        ControlKind::Check => {
            if c.checked != CheckState::Unchecked {
                let state = if c.checked == CheckState::Checked { "Checked" } else { "Mixed" };
                let _ = write!(s, " checked=\"{state}\"");
            }
            if let Some(l) = &c.link {
                let _ = write!(s, " fmlaLink=\"{}\"", esc_attr(l));
            }
            if c.check_lock_text.or(c.check_vml_lock_text).unwrap_or(true) {
                s.push_str(" lockText=\"1\"");
            }
            if let Some(flat) = c.no_three_d {
                let _ = write!(s, " noThreeD=\"{}\"", u8::from(flat));
            }
        }
    }
    s.push_str("/>");
    s
}

/// A control's legacy VML shape.
fn vml_shape(sheet: &Sheet, c: &FormControl, id: u64, z: usize) -> String {
    let ((from, fdx, fdy), (to, tdx, tdy)) = corners(sheet, &c.anchor);
    let pt = |px: f64| px * 0.75;
    let x = sheet.col_left(from.col) + fdx;
    let y = sheet.row_top(from.row) + fdy;
    let mut data = format!(
        "<x:ClientData ObjectType=\"{}\">{}{}<x:Anchor>{}, {}, {}, {}, {}, {}, {}, {}</x:Anchor>",
        vml_object_type(c.kind),
        // These two elements mean the object does *not* move (size) with its cells.
        if c.anchor.mode == AnchorMode::Absolute { "<x:MoveWithCells/>" } else { "" },
        if c.anchor.mode == AnchorMode::MoveAndSize { "" } else { "<x:SizeWithCells/>" },
        from.col,
        fdx.round() as i64,
        from.row,
        fdy.round() as i64,
        to.col,
        tdx.round() as i64,
        to.row,
        tdy.round() as i64
    );
    if !c.print {
        data.push_str("<x:PrintObject>False</x:PrintObject>");
    }
    if !c.locked {
        data.push_str("<x:Locked>False</x:Locked>");
    }
    let look = if c.kind == ControlKind::Check { c.check_vml_look.as_ref() } else { None };
    if look.is_none_or(|l| l.auto_fill_off) {
        data.push_str("<x:AutoFill>False</x:AutoFill>");
    }
    match c.kind {
        ControlKind::Button => {
            if let Some(m) = macro_ref(c) {
                let _ = write!(data, "<x:FmlaMacro>{}</x:FmlaMacro>", esc(&m));
            }
            data.push_str("<x:TextHAlign>Center</x:TextHAlign><x:TextVAlign>Center</x:TextVAlign>");
        }
        ControlKind::Slider | ControlKind::Spin => {
            if let Some(l) = &c.link {
                let _ = write!(data, "<x:FmlaLink>{}</x:FmlaLink>", esc(l));
            }
            let _ = write!(data, "<x:Val>{}</x:Val><x:Min>{}</x:Min><x:Max>{}</x:Max><x:Inc>{}</x:Inc>", c.value, c.min, c.max, c.step);
            if c.kind == ControlKind::Slider {
                let _ = write!(data, "<x:Page>{}</x:Page>", c.page);
                if c.horizontal {
                    data.push_str("<x:Horiz/>");
                }
            }
            data.push_str("<x:Dx>22</x:Dx>");
        }
        ControlKind::Drop => {
            data.push_str("<x:AutoLine>False</x:AutoLine>");
            if let Some(l) = &c.link {
                let _ = write!(data, "<x:FmlaLink>{}</x:FmlaLink>", esc(l));
            }
            let d = c.drop_vml_display;
            if let Some(val) = d.scroll_offset {
                let _ = write!(data, "<x:Val>{val}</x:Val>");
            }
            let _ = write!(data, "<x:Min>0</x:Min><x:Max>0</x:Max><x:Inc>1</x:Inc><x:Page>{}</x:Page>", c.drop_lines);
            if let Some(dx) = d.scroll_width {
                let _ = write!(data, "<x:Dx>{dx}</x:Dx>");
            }
            push_vml_flag(&mut data, "NoThreeD", d.no_three_d);
            push_vml_flag(&mut data, "NoThreeD2", d.no_three_d2);
            if let Some(r) = &c.list_range {
                let _ = write!(data, "<x:FmlaRange>{}</x:FmlaRange>", esc(r));
            }
            if c.selected > 0 {
                let _ = write!(data, "<x:Sel>{}</x:Sel>", c.selected);
            }
            let _ = write!(
                data,
                "<x:SelType>Single</x:SelType><x:LCT>Normal</x:LCT><x:DropStyle>{}</x:DropStyle><x:DropLines>{}</x:DropLines>",
                drop_style_names(c.drop_style).1,
                c.drop_lines
            );
        }
        ControlKind::Check => {
            if look.is_none_or(|l| l.auto_line_off) {
                data.push_str("<x:AutoLine>False</x:AutoLine>");
            }
            if look.is_none_or(|l| l.text_v_center) {
                data.push_str("<x:TextVAlign>Center</x:TextVAlign>");
            }
            if let Some(m) = macro_ref(c) {
                let _ = write!(data, "<x:FmlaMacro>{}</x:FmlaMacro>", esc(&m));
            }
            if let Some(l) = &c.link {
                let _ = write!(data, "<x:FmlaLink>{}</x:FmlaLink>", esc(l));
            }
            if c.checked != CheckState::Unchecked {
                let _ = write!(data, "<x:Checked>{}</x:Checked>", c.checked.number());
            }
            push_vml_flag(&mut data, "LockText", c.check_vml_lock_text);
            push_vml_flag(&mut data, "NoThreeD", c.no_three_d);
        }
    }
    data.push_str("</x:ClientData>");
    let text = match c.kind {
        ControlKind::Button => {
            format!(
                "<v:textbox style=\"mso-direction-alt:auto\" o:singleclick=\"f\"><div style=\"text-align:center\">{}</div></v:textbox>",
                esc(&c.text)
            )
        }
        ControlKind::Check => check_caption(c),
        _ => String::new(),
    };
    let mut shape = String::from("<v:shape");
    if c.kind == ControlKind::Check && !c.name.trim().is_empty() && shape_number(&c.name).is_none() {
        let _ = write!(shape, " o:spid=\"_x0000_s{id}\" id=\"{}\"", esc_attr(&c.name));
    } else {
        let _ = write!(shape, " id=\"_x0000_s{id}\"");
    }
    // A check box from a file keeps the paint, fill, lock and path settings it had; others get
    // GridCraft's own (no fill or line, rotation locked).
    let (paint, parts): (Cow<'_, str>, Cow<'_, str>) = match look {
        Some(l) => {
            let mut paint = String::new();
            push_vml_attrs(&mut paint, &l.paint, &VML_PAINT);
            let mut parts = String::new();
            for (name, attrs, names) in [("v:fill", &l.fill, &VML_FILL[..]), ("o:lock", &l.lock, &VML_LOCK[..]), ("v:path", &l.path, &VML_PATH[..])] {
                if let Some(attrs) = attrs {
                    let _ = write!(parts, "<{name}");
                    push_vml_attrs(&mut parts, attrs, names);
                    parts.push_str("/>");
                }
            }
            (paint.into(), parts.into())
        }
        None if c.kind == ControlKind::Button => (" o:button=\"t\"".into(), "<o:lock v:ext=\"edit\" rotation=\"t\"/>".into()),
        None => (" stroked=\"f\" filled=\"f\"".into(), "<o:lock v:ext=\"edit\" rotation=\"t\"/>".into()),
    };
    let _ = write!(
        shape,
        " type=\"#_x0000_t201\" style=\"position:absolute;margin-left:{:.2}pt;margin-top:{:.2}pt;width:{:.2}pt;height:{:.2}pt;z-index:{}{}\"{paint} o:insetmode=\"auto\">{parts}{text}{data}</v:shape>",
        pt(x),
        pt(y),
        pt(c.anchor.width.max(0.0) as f64),
        pt(c.anchor.height.max(0.0) as f64),
        z + 1,
        if c.kind == ControlKind::Check && c.check_vml_wrap_tight { ";mso-wrap-style:tight" } else { "" },
    );
    shape
}

/// A VML flag element as the file had it: `<x:Name/>` when on, `<x:Name>False</x:Name>` when
/// written off, nothing when left out.
fn push_vml_flag(data: &mut String, name: &str, value: Option<bool>) {
    match value {
        Some(true) => {
            let _ = write!(data, "<x:{name}/>");
        }
        Some(false) => {
            let _ = write!(data, "<x:{name}>False</x:{name}>");
        }
        None => {}
    }
}

fn caption_font_attr(name: &str) -> bool {
    matches!(name, "face" | "size" | "color")
}

/// A check box's caption text box: the caption in the `<div>` (and `<font>`) the file set it in,
/// else in a left-aligned `<div>`, as Excel writes a new check box's. Nothing without a caption.
fn check_caption(c: &FormControl) -> String {
    if c.text.is_empty() && c.caption_markup.is_none() {
        return String::new();
    }
    let mut s = String::from("<v:textbox style=\"mso-direction-alt:auto\" o:singleclick=\"f\"><div");
    let markup = c.caption_markup.as_ref();
    match markup.map_or(Some("text-align:left"), |m| m.div_style.as_deref()) {
        Some(style) => {
            let _ = write!(s, " style=\"{}\">", esc_attr(style));
        }
        None => s.push('>'),
    }
    let font = markup.and_then(|m| m.font.as_ref());
    if let Some(attrs) = font {
        s.push_str("<font");
        for (k, v) in attrs.iter().filter(|(k, _)| caption_font_attr(k)) {
            let _ = write!(s, " {k}=\"{}\"", esc_attr(v));
        }
        s.push('>');
    }
    s.push_str(&esc(&c.text));
    if font.is_some() {
        s.push_str("</font>");
    }
    s.push_str("</div></v:textbox>");
    s
}

/// The VML shape type form controls use.
pub const SHAPETYPE: &str = "<v:shapetype id=\"_x0000_t201\" coordsize=\"21600,21600\" o:spt=\"201\" path=\"m,l,21600r21600,l21600,xe\"><v:stroke joinstyle=\"miter\"/><v:path shadowok=\"f\" o:extrusionok=\"f\" strokeok=\"f\" fillok=\"f\" o:connecttype=\"rect\"/><o:lock v:ext=\"edit\" shapetype=\"t\"/></v:shapetype>";

// ---------------------------------------------------------------- reading

/// What a control's properties say, from `formControlPr` or VML `x:ClientData`.
#[derive(Clone, Default)]
struct Props {
    kind: Option<ControlKind>,
    link: Option<String>,
    min: Option<i64>,
    max: Option<i64>,
    inc: Option<i64>,
    page: Option<i64>,
    val: Option<i64>,
    horiz: Option<bool>,
    macro_: Option<String>,
    text: String,
    print: Option<bool>,
    locked: Option<bool>,
    /// A drop-down's source range (`fmlaRange`), chosen item (`sel`), rows and style.
    range: Option<String>,
    sel: Option<i64>,
    drop_lines: Option<i64>,
    drop_style: Option<DropStyle>,
    /// Shading (`noThreeD`, `noThreeD2`) and a drop-down's scroll settings, from each part.
    display: DropDisplay,
    vml_display: DropDisplay,
    /// A check box's state (`checked`, `x:Checked`) and how its caption is set.
    checked: Option<CheckState>,
    markup: Option<CaptionMarkup>,
    check_lock_text: Option<bool>,
    check_vml_lock_text: Option<bool>,
    check_vml_wrap_tight: bool,
    check_vml_look: Option<CheckVmlLook>,
    check_sheet_auto_flags: Option<CheckSheetAutoFlags>,
    check_ctrl_prop_opaque: bool,
    /// Payload that the drop-down or check box model cannot faithfully rewrite.
    opaque: bool,
}

/// Leave a drop-down or check box with unsupported payload intact rather than rewriting it from
/// the model. A check box's macro is kept as read; a drop-down's isn't modelled.
fn beyond_model(p: &Props, kind: ControlKind) -> bool {
    let long = |r: Option<&str>| r.is_some_and(|r| r.chars().count() > 300);
    match kind {
        ControlKind::Drop => {
            p.opaque || p.macro_.as_deref().is_some_and(|m| !m.trim().is_empty()) || long(p.range.as_deref()) || long(p.link.as_deref())
        }
        ControlKind::Check => p.opaque || long(p.macro_.as_deref()) || long(p.link.as_deref()),
        _ => false,
    }
}

/// Properties-part payload beyond the drop-down model.
fn drop_ctrl_prop_payload(e: &El) -> bool {
    let int = |k: &str| e.attr_f64(k).map(|v| v.round() as i64);
    let uint = |k: &str| e.attr(k).and_then(|v| v.trim().parse::<u32>().ok());
    e.children.iter().any(|c| c.name != "itemLst" || !c.children.is_empty())
        || e.attrs.iter().any(|(k, v)| match k.as_str() {
            "objectType" | "fmlaLink" | "fmlaRange" | "sel" | "dropLines" => false,
            "dropStyle" => drop_style_of(v).is_none(),
            "dx" | "val" => uint(k).is_none(),
            "min" => int(k) != Some(0),
            "inc" => int(k) != Some(1),
            "noThreeD" | "noThreeD2" => e.attr_bool(k).is_none(),
            "colored" | "justLastX" => e.attr_bool(k) != Some(false),
            "seltype" => !v.eq_ignore_ascii_case("single"),
            "textHAlign" => !v.eq_ignore_ascii_case("left"),
            "textVAlign" => !v.eq_ignore_ascii_case("top"),
            _ => true,
        })
}

/// Properties-part payload beyond the check box model (state, link, shading and caption lock).
fn check_ctrl_prop_payload(e: &El) -> bool {
    !e.children.is_empty()
        || !e.text.trim().is_empty()
        || e.attrs.iter().any(|(k, v)| match k.as_str() {
            "objectType" | "fmlaLink" => false,
            "checked" => check_state_of(v).is_none(),
            "noThreeD" => e.attr_bool(k).is_none(),
            "lockText" => e.attr_bool(k).is_none(),
            _ => true,
        })
}

fn props_from_ctrl_prop(e: &El) -> Props {
    let int = |k: &str| e.attr_f64(k).map(|v| v.round() as i64);
    let uint = |k: &str| e.attr(k).and_then(|v| v.trim().parse::<u32>().ok());
    let kind = e.attr("objectType").and_then(kind_of);
    Props {
        kind,
        link: e.attr("fmlaLink").map(str::to_string),
        min: int("min"),
        max: int("max"),
        inc: int("inc"),
        page: int("page"),
        val: int("val"),
        horiz: e.attr_bool("horiz"),
        macro_: None,
        text: String::new(),
        print: None,
        locked: None,
        range: e.attr("fmlaRange").map(str::to_string),
        sel: int("sel"),
        drop_lines: int("dropLines"),
        drop_style: e.attr("dropStyle").and_then(drop_style_of),
        display: DropDisplay {
            no_three_d: e.attr_bool("noThreeD"),
            no_three_d2: e.attr_bool("noThreeD2"),
            scroll_width: uint("dx"),
            scroll_offset: uint("val"),
        },
        vml_display: DropDisplay::default(),
        checked: e.attr("checked").and_then(check_state_of),
        markup: None,
        check_lock_text: Some(e.attr_bool("lockText").unwrap_or(false)),
        check_vml_lock_text: None,
        check_vml_wrap_tight: false,
        check_vml_look: None,
        check_sheet_auto_flags: None,
        check_ctrl_prop_opaque: matches!(kind, Some(ControlKind::Check) | None)
            && (check_ctrl_prop_payload(e) || e.attr("objectType").is_some_and(|_| kind.is_none())),
        opaque: match kind {
            Some(ControlKind::Drop) => drop_ctrl_prop_payload(e),
            _ => false,
        },
    }
}

fn props_from_vml(shape: &El) -> Option<Props> {
    let data = shape.child("ClientData")?;
    let kind = data.attr("ObjectType").and_then(kind_of)?;
    let text_of = |n: &str| data.child(n).map(|c| c.text.trim().to_string());
    let int = |n: &str| text_of(n).and_then(|t| t.parse::<f64>().ok()).filter(|v| v.is_finite()).map(|v| v.round() as i64);
    let truth = |n: &str| {
        data.child(n).and_then(|c| if kind == ControlKind::Check { vml_drop_bool(c) } else { Some(!c.text.trim().eq_ignore_ascii_case("false")) })
    };
    let (text, markup, opaque) = match kind {
        ControlKind::Check => match check_caption_of(shape) {
            Some((text, markup)) => (text, markup, check_vml_payload(shape, data)),
            None => (String::new(), None, true),
        },
        _ => {
            let text = shape.child("textbox").map(|t| all_text(t, 0)).unwrap_or_default().trim().to_string();
            let opaque = kind == ControlKind::Drop && (drop_vml_payload(shape, data) || !text.is_empty());
            (text, None, opaque)
        }
    };
    Some(Props {
        kind: Some(kind),
        link: text_of("FmlaLink").filter(|t| !t.is_empty()),
        min: int("Min"),
        max: int("Max"),
        inc: int("Inc"),
        page: int("Page"),
        val: int("Val"),
        horiz: data.child("Horiz").map(|_| true),
        macro_: if kind == ControlKind::Check { data.child("FmlaMacro").map(|e| e.text.clone()) } else { text_of("FmlaMacro") },
        text,
        print: truth("PrintObject"),
        locked: truth("Locked"),
        range: text_of("FmlaRange").filter(|t| !t.is_empty()),
        sel: int("Sel"),
        drop_lines: int("DropLines"),
        drop_style: text_of("DropStyle").as_deref().and_then(drop_style_of),
        display: DropDisplay::default(),
        vml_display: DropDisplay {
            no_three_d: data.child("NoThreeD").and_then(vml_drop_bool),
            no_three_d2: data.child("NoThreeD2").and_then(vml_drop_bool),
            scroll_width: data.child("Dx").and_then(vml_drop_uint),
            scroll_offset: data.child("Val").and_then(vml_drop_uint),
        },
        checked: data.child("Checked").and_then(|c| vml_check_state_of(&c.text)),
        markup,
        check_vml_lock_text: if kind == ControlKind::Check { data.child("LockText").and_then(vml_drop_bool) } else { None },
        check_vml_wrap_tight: kind == ControlKind::Check
            && shape.attr("style").is_some_and(|s| s.split(';').filter_map(vml_style_entry).any(|p| p == ("mso-wrap-style", "tight"))),
        check_vml_look: (kind == ControlKind::Check).then(|| check_vml_look_of(shape, data)),
        opaque,
        ..Props::default()
    })
}

/// A check box shape's look ([`CheckVmlLook`]): what [`check_vml_payload`] lets an editable check
/// box hold, kept so it is written back as read.
fn check_vml_look_of(shape: &El, data: &El) -> CheckVmlLook {
    let attrs = |name: &str| shape.child(name).map(|e| e.attrs.clone());
    let says = |name: &str, v: &str| data.child(name).is_some_and(|c| c.text.trim().eq_ignore_ascii_case(v));
    CheckVmlLook {
        paint: shape.attrs.iter().filter(|(k, _)| VML_PAINT.iter().any(|(n, _)| n == k)).cloned().collect(),
        fill: attrs("fill"),
        lock: attrs("lock"),
        path: attrs("path"),
        auto_fill_off: says("AutoFill", "false"),
        auto_line_off: says("AutoLine", "false"),
        text_v_center: says("TextVAlign", "center"),
    }
}

/// The attributes a kept [`CheckVmlLook`] writes, by element: (name as read, name as written).
const VML_PAINT: [(&str, &str); 5] =
    [("filled", "filled"), ("fillcolor", "fillcolor"), ("stroked", "stroked"), ("strokecolor", "strokecolor"), ("strokeweight", "strokeweight")];
const VML_FILL: [(&str, &str); 2] = [("color2", "color2"), ("detectmouseclick", "o:detectmouseclick")];
const VML_LOCK: [(&str, &str); 4] = [("ext", "v:ext"), ("rotation", "rotation"), ("text", "text"), ("aspectratio", "aspectratio")];
const VML_PATH: [(&str, &str); 3] = [("shadowok", "shadowok"), ("strokeok", "strokeok"), ("fillok", "fillok")];

/// Writes `attrs` that `names` knows, as ` name="value"`.
fn push_vml_attrs(out: &mut String, attrs: &[(String, String)], names: &[(&str, &str)]) {
    for (k, v) in attrs {
        if let Some((_, q)) = names.iter().find(|(n, _)| n == k) {
            let _ = write!(out, " {q}=\"{}\"", esc_attr(v));
        }
    }
}

/// VML payload beyond the check box model. The look Excel gives a check box (no fill or line of
/// its own, the window colours, its `v:fill`, `o:lock` and `v:path` settings) is kept as read
/// ([`CheckVmlLook`]) and written back; anything else about the shape, and client data other than
/// its anchor, flags, link, macro, state and shading, is kept by leaving the check box to
/// pass-through.
fn check_vml_payload(shape: &El, data: &El) -> bool {
    shape.children.len() > 5
        || data.children.len() > 13
        || has_repeated_children(shape)
        || has_repeated_children(data)
        || shape.attrs.iter().any(|(k, v)| match k.as_str() {
            "id" | "spid" => false,
            "style" => v.split(';').map(str::trim).filter(|s| !s.is_empty()).any(|s| match vml_style_entry(s) {
                Some(("position", "absolute")) => false,
                Some(("margin-left" | "margin-top" | "width" | "height", v)) => {
                    !v.strip_suffix("pt").or_else(|| v.strip_suffix("in")).unwrap_or(v).parse::<f64>().is_ok_and(f64::is_finite)
                }
                Some(("z-index", v)) => v.parse::<i64>().is_err(),
                Some(("mso-wrap-style", "tight")) => false,
                _ => true,
            }),
            "insetmode" => v != "auto",
            "stroked" | "filled" => v != "f",
            "type" => v != "#_x0000_t201",
            "strokecolor" => !matches!(v.as_str(), "black [0]" | "black [64]" | "windowText [64]" | "windowText"),
            "strokeweight" => shape.attr("stroked") != Some("f") || v != "3e-5mm",
            "fillcolor" => !matches!(v.as_str(), "window [65]" | "windowText [64]") && !(shape.attr("filled") == Some("f") && v == "white [9]"),
            _ => true,
        })
        || shape.children.iter().any(|c| match c.name.as_str() {
            "ClientData" | "textbox" => false,
            _ if !c.children.is_empty() || !c.text.trim().is_empty() => true,
            "lock" => c
                .attrs
                .iter()
                .any(|(k, v)| !matches!((k.as_str(), v.as_str()), ("ext", "edit") | ("rotation" | "text", "t") | ("aspectratio", "t" | "f"))),
            "fill" => c.attrs.iter().any(|(k, v)| !matches!((k.as_str(), v.as_str()), ("color2", "window [65]") | ("detectmouseclick", "t"))),
            "path" => c.attrs.iter().any(|(k, v)| !matches!((k.as_str(), v.as_str()), ("shadowok" | "strokeok" | "fillok", "t"))),
            _ => true,
        })
        || data.attrs.iter().any(|(k, _)| k != "ObjectType")
        || data.children.iter().any(|c| {
            !c.attrs.is_empty()
                || !c.children.is_empty()
                || match c.name.as_str() {
                    "Anchor" => vml_anchor_numbers(&c.text).is_none(),
                    "FmlaLink" | "FmlaMacro" => false,
                    "AutoFill" | "AutoLine" => !c.text.trim().eq_ignore_ascii_case("false"),
                    "TextVAlign" => !c.text.trim().eq_ignore_ascii_case("center"),
                    "Checked" => vml_check_state_of(&c.text).is_none(),
                    "NoThreeD" | "MoveWithCells" | "SizeWithCells" | "Locked" | "PrintObject" | "LockText" => vml_drop_bool(c).is_none(),
                    _ => true,
                }
        })
}

fn vml_style_entry(s: &str) -> Option<(&str, &str)> {
    s.split_once(':').map(|(k, v)| (k.trim(), v.trim()))
}

fn has_repeated_children(e: &El) -> bool {
    // Only inspect known, bounded sets of children. A duplicate means the writer would drop one.
    e.children.iter().enumerate().any(|(i, c)| e.children[..i].iter().any(|prior| prior.name == c.name))
}

fn check_sheet_control_payload(ctl: &El, pr: Option<&El>) -> bool {
    ctl.attrs.iter().any(|(k, _)| !matches!(k.as_str(), "shapeId" | "id" | "name"))
        || !ctl.text.trim().is_empty()
        || ctl.children.len() > usize::from(pr.is_some())
        || pr.is_some_and(|pr| {
            !pr.text.trim().is_empty()
                || pr.children.len() > 1
                || pr.child("anchor").is_some_and(|a| {
                    !a.text.trim().is_empty()
                        || a.attrs.iter().any(|(k, _)| !matches!(k.as_str(), "moveWithCells" | "sizeWithCells") || a.attr_bool(k).is_none())
                        || a.children.len() != 2
                        || ["from", "to"].into_iter().any(|name| {
                            a.child(name).is_none_or(|m| {
                                !m.attrs.is_empty()
                                    || !m.text.trim().is_empty()
                                    || m.children.len() != 4
                                    || ["col", "colOff", "row", "rowOff"].into_iter().any(|name| {
                                        m.child(name)
                                            .is_none_or(|e| !e.attrs.is_empty() || !e.children.is_empty() || e.text.trim().parse::<i64>().is_err())
                                    })
                            })
                        })
                })
        })
}

/// A check box's caption and how its VML text box sets it: one `<div>` (with at most a `style`)
/// holding the text, or one `<font>` (face, size, colour) holding it. `None` for anything else
/// (several runs, embedded line-break elements, other text box settings, or an oversized caption).
fn check_caption_of(shape: &El) -> Option<(String, Option<CaptionMarkup>)> {
    let blank = |e: &El| e.text.trim().is_empty();
    let Some(tb) = shape.child("textbox") else { return Some((String::new(), None)) };
    let plain_box = tb.attrs.iter().all(|(k, v)| matches!((k.as_str(), v.as_str()), ("style", "mso-direction-alt:auto") | ("singleclick", "f")));
    if !plain_box || !blank(tb) {
        return None;
    }
    let div = match tb.children.as_slice() {
        [] => return Some((String::new(), None)),
        [d] if d.name == "div" && d.attrs.iter().all(|(k, _)| k == "style") => d,
        _ => return None,
    };
    let (holder, font) = match div.children.as_slice() {
        [] => (div, None),
        [f] if f.name == "font" && f.children.is_empty() && blank(div) && f.attrs.iter().all(|(k, _)| caption_font_attr(k)) => {
            (f, Some(f.attrs.clone()))
        }
        _ => return None,
    };
    let text = &holder.text;
    if text.chars().count() > 1_024 {
        return None;
    }
    Some((text.to_string(), Some(CaptionMarkup { div_style: div.attr("style").map(str::to_string), font })))
}

fn vml_drop_bool(e: &El) -> Option<bool> {
    let text = e.text.trim();
    if text.is_empty() || text.eq_ignore_ascii_case("true") || text == "1" || text.eq_ignore_ascii_case("t") {
        Some(true)
    } else if text.eq_ignore_ascii_case("false") || text == "0" || text.eq_ignore_ascii_case("f") {
        Some(false)
    } else {
        None
    }
}

fn vml_drop_uint(e: &El) -> Option<u32> {
    e.text.trim().parse().ok()
}

fn drop_vml_payload(shape: &El, data: &El) -> bool {
    shape.attrs.iter().any(|(k, v)| match k.as_str() {
        "id" | "spid" | "style" | "stroked" | "filled" | "insetmode" => false,
        "type" => v != "#_x0000_t201",
        "strokecolor" => !matches!(v.as_str(), "black [0]" | "black [64]" | "windowText [64]" | "windowText"),
        "fillcolor" => v != "windowText [64]",
        "strokeweight" => v != "3e-5mm",
        _ => true,
    }) || shape.children.iter().any(|c| match c.name.as_str() {
        "ClientData" => false,
        "lock" => c.attrs.iter().any(|(k, v)| !matches!((k.as_str(), v.as_str()), ("ext", "edit") | ("rotation" | "text", "t"))),
        "fill" => c.attrs.iter().any(|(k, v)| !matches!((k.as_str(), v.as_str()), ("color2", "window [65]"))),
        _ => true,
    }) || data.attrs.iter().any(|(k, _)| k != "ObjectType")
        || data.children.iter().any(|c| match c.name.as_str() {
            "Anchor" | "MoveWithCells" | "SizeWithCells" | "FmlaLink" | "FmlaRange" | "DropLines" | "Sel" | "Locked" | "PrintObject"
            | "FmlaMacro" | "Max" | "Page" => false,
            "DropStyle" => drop_style_of(&c.text).is_none(),
            "AutoFill" | "AutoLine" => !c.text.trim().eq_ignore_ascii_case("false"),
            "Val" | "Dx" => vml_drop_uint(c).is_none(),
            "NoThreeD" | "NoThreeD2" => vml_drop_bool(c).is_none(),
            "Min" => c.text.trim() != "0",
            "Inc" => c.text.trim() != "1",
            "SelType" => !c.text.trim().eq_ignore_ascii_case("single"),
            "LCT" => !c.text.trim().eq_ignore_ascii_case("normal"),
            _ => true,
        })
}

/// The text inside `e` and its descendants (a VML text box holds `<div>` and `<font>`), up to a
/// caption's length; `<br>` is a line break.
fn all_text(e: &El, depth: usize) -> String {
    let mut s = e.text.clone();
    if depth < 16 {
        for k in &e.children {
            if k.name == "br" {
                s.push('\n');
            }
            s.push_str(&all_text(k, depth + 1));
            if s.len() > 4_096 {
                break;
            }
        }
    }
    s
}

/// A control from what the file says, its numbers clamped to whole numbers 0..30000 that pass
/// the model's rules (a hostile or odd file can't give a control it couldn't have).
fn build(p: Props, kind: ControlKind, anchor: Anchor, name: Option<&str>, id: u32) -> FormControl {
    let clamp = |v: Option<i64>, d: u32| v.map_or(d, |v| v.clamp(0, CONTROL_MAX as i64) as u32);
    let button_macro = if kind == ControlKind::Button { p.macro_.as_deref().and_then(macro_name) } else { None };
    let mut c = FormControl { id, kind, anchor, ..FormControl::default() };
    c.min = clamp(p.min, 0);
    c.max = clamp(p.max, 100);
    if c.min >= c.max {
        if c.min >= CONTROL_MAX {
            c.min = CONTROL_MAX - 1;
        }
        c.max = c.min + 1;
    }
    c.step = clamp(p.inc, 1).clamp(1, c.max - c.min);
    c.page = clamp(p.page, 10).max(c.step);
    c.value = c.snap(clamp(p.val, c.min) as f64);
    // Only sliders have an orientation; `horiz` is left out for a vertical one.
    c.horizontal = kind != ControlKind::Slider || p.horiz.unwrap_or(false);
    let reference = |r: Option<String>| r.map(|l| l.trim().trim_start_matches('=').to_string()).filter(|l| !l.is_empty() && l.chars().count() <= 300);
    c.link = reference(p.link);
    if kind == ControlKind::Drop {
        c.list_range = reference(p.range);
        c.selected = clamp(p.sel, 0);
        c.drop_lines = clamp(p.drop_lines, DROP_LINES_DEFAULT).max(1);
        c.drop_style = p.drop_style.unwrap_or_default();
        c.drop_display = p.display;
        c.drop_vml_display = p.vml_display;
    }
    if kind == ControlKind::Check {
        c.checked = p.checked.unwrap_or_default();
        c.no_three_d = p.display.no_three_d.or(p.vml_display.no_three_d);
        c.check_lock_text = p.check_lock_text;
        c.check_vml_lock_text = p.check_vml_lock_text;
        c.check_vml_wrap_tight = p.check_vml_wrap_tight;
        c.check_vml_look = p.check_vml_look;
        c.check_sheet_auto_flags = p.check_sheet_auto_flags;
        c.kept_macro = p.macro_;
        c.caption_markup = p.markup;
    }
    c.text = if kind == ControlKind::Check { p.text } else { p.text.chars().take(1_024).collect() };
    if let Some(n) = name {
        c.name = if kind == ControlKind::Check { n.to_string() } else { n.chars().take(255).collect() };
    }
    c.print = p.print.unwrap_or(true);
    c.locked = p.locked.unwrap_or(true);
    if let Some(m) = button_macro {
        c.action = Some(ControlAction::Macro { name: m, input: MacroInput::Selection, output: None });
    }
    c
}

/// The `<control>` elements one child of a `<controls>` element holds: the child itself, or those
/// inside its `mc:AlternateContent` (the Choice, else the Fallback).
fn controls_of_child(k: &El) -> Vec<&El> {
    match k.name.as_str() {
        "control" => vec![k],
        "AlternateContent" => k
            .child("Choice")
            .filter(|c| c.child("control").is_some())
            .or_else(|| k.child("Fallback"))
            .map(|inner| inner.kids("control").collect())
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// The `<control>` elements under a `<controls>` element, looking through `mc:AlternateContent`
/// (the Choice, else the Fallback).
fn controls_in(e: &El) -> Vec<&El> {
    e.children.iter().flat_map(controls_of_child).collect()
}

/// The `<controls>` element of a worksheet child (`controls`, or one inside AlternateContent).
pub fn controls_of(e: &El) -> Option<&El> {
    match e.name.as_str() {
        "controls" => Some(e),
        "AlternateContent" => e.child("Choice").and_then(|c| c.child("controls")).or_else(|| e.child("Fallback").and_then(|c| c.child("controls"))),
        _ => None,
    }
}

/// The shape number of a VML shape id (`_x0000_s1025` → 1025) or a `shapeId` attribute.
pub(crate) fn shape_number(id: &str) -> Option<u64> {
    id.trim().trim_start_matches("_x0000_s").parse().ok()
}

/// A VML shape's number: from its `id` (`_x0000_s1025`), else from `o:spid`, where producers that
/// give the shape a named id (`id="Button 1"`) keep it.
pub(crate) fn number_of(shape: &El) -> Option<u64> {
    shape.attr("id").and_then(shape_number).or_else(|| shape.attr("spid").and_then(shape_number))
}

fn vml_name(shape: &El) -> Option<&str> {
    shape.attr("id").filter(|id| shape_number(id).is_none() && !id.trim().is_empty())
}

/// What [`read`] took from a sheet's form controls, for pass-through.
#[derive(Debug, Default)]
pub struct ReadControls {
    /// The VML shape numbers of the controls read into the model: GridCraft writes those, so they
    /// are left out of the kept content.
    pub modelled: HashSet<u64>,
    /// Sheet controls, not necessarily modeled. The name confirms proxies without compatExt.
    pub known: HashMap<u64, Option<String>>,
    pub drawing: Option<crate::drawing::KeptProxies>,
}

/// Whether a VML shape's client data is a note's: its `ObjectType` is `Note` in any case and with
/// spaces around it. Only that attribute counts, not text elsewhere in the shape (a caption).
pub(crate) fn is_note_data(data: &El) -> bool {
    data.attr("ObjectType").is_some_and(|t| t.trim().eq_ignore_ascii_case("note"))
}

/// Reads a sheet's buttons, scroll bars and spin buttons: each `<control>` with its properties
/// part, else its VML shape, and VML shapes the sheet doesn't list (files that only have VML).
/// The shapes come from the VML drawing with relationship id `vml_rid` (`<legacyDrawing r:id>`).
/// Returns the shape numbers of the controls it read, so pass-through keeps only the others.
pub fn read(cx: &mut Ctx<'_>, controls: Option<&El>, vml_rid: Option<&str>, rels: &[Rel], sheet: &mut Sheet) -> ReadControls {
    let mut read = HashSet::new();
    let vml = match vml_rid.and_then(|id| rels.iter().find(|r| r.id == id && r.kind == "vmlDrawing")) {
        Some(r) => cx.pkg.read_xml(&r.target).ok().flatten(),
        None => None,
    };
    let shapes: Vec<&El> = vml.as_ref().map(|v| v.kids("shape").collect()).unwrap_or_default();
    // Looked up once per listed control: indexes keep a sheet of many controls linear.
    let mut by_number: HashMap<u64, &El> = HashMap::with_capacity(shapes.len());
    for s in &shapes {
        if let Some(n) = number_of(s) {
            by_number.entry(n).or_insert(s);
        }
    }
    let shape_of = |n: u64| by_number.get(&n).copied();
    // The relationships likewise: one per control, looked up by id (the first of a repeated id wins,
    // as a scan would find it).
    let mut rel_by_id: HashMap<&str, &Rel> = HashMap::with_capacity(rels.len());
    for r in rels {
        rel_by_id.entry(r.id.as_str()).or_insert(r);
    }
    // Each properties part is read and parsed once, however many controls name it: every read is
    // charged to the package's budget, so a file whose controls all point at one part would
    // otherwise spend it on that part alone. A part that fails to read, or isn't a
    // `formControlPr`, is remembered as nothing and not read again.
    let mut props_by_part: HashMap<&str, Option<Props>> = HashMap::new();
    // Every shape the sheet lists, read here or not.
    let mut listed: HashSet<u64> = HashSet::new();
    let mut known = HashMap::new();
    for ctl in controls.map(controls_in).unwrap_or_default() {
        let Some(n) = ctl.attr("shapeId").and_then(shape_number) else { continue };
        listed.insert(n);
        known.insert(n, ctl.attr("name").map(str::to_string));
        if sheet.controls.len() >= MAX_CONTROLS {
            continue;
        }
        let shape = shape_of(n);
        let vml_props = shape.and_then(props_from_vml);
        let prop_part = ctl.attr("id").and_then(|id| rel_by_id.get(id)).map(|r| r.target.as_str());
        let file_props = match prop_part {
            Some(t) => props_by_part
                .entry(t)
                .or_insert_with(|| cx.pkg.read_xml(t).ok().flatten().filter(|e| e.name == "formControlPr").map(|e| props_from_ctrl_prop(&e)))
                .clone(),
            None => None,
        };
        let pr = ctl.child("controlPr");
        let mut p = match (file_props, vml_props) {
            (Some(mut f), v) => {
                // The properties part has no caption; the VML text box does.
                if let Some(v) = v {
                    f.text = v.text;
                    f.print = f.print.or(v.print);
                    f.locked = f.locked.or(v.locked);
                    f.macro_ = f.macro_.or(v.macro_);
                    f.range = f.range.or(v.range);
                    match f.kind.or(v.kind) {
                        Some(ControlKind::Drop) => {
                            f.kind = f.kind.or(v.kind);
                            f.link = f.link.or(v.link);
                            f.sel = f.sel.or(v.sel);
                            f.drop_lines = f.drop_lines.or(v.drop_lines);
                            f.drop_style = f.drop_style.or(v.drop_style);
                            f.vml_display = v.vml_display;
                            f.opaque |= f.kind != v.kind;
                        }
                        Some(ControlKind::Check) => {
                            f.kind = f.kind.or(v.kind);
                            f.link = f.link.or(v.link);
                            f.checked = f.checked.or(v.checked);
                            f.vml_display = v.vml_display;
                            f.markup = v.markup;
                            f.check_vml_lock_text = v.check_vml_lock_text;
                            f.check_vml_wrap_tight = v.check_vml_wrap_tight;
                            f.check_vml_look = v.check_vml_look;
                            f.opaque |= f.kind != v.kind;
                        }
                        _ => {}
                    }
                    f.opaque |= v.opaque;
                }
                f
            }
            (None, Some(v)) => v,
            (None, None) => continue,
        };
        if let Some(pr) = pr {
            if let Some(m) = pr.attr("macro").filter(|m| p.kind == Some(ControlKind::Check) || !m.trim().is_empty()) {
                p.macro_ = Some(m.to_string());
            }
            p.print = pr.attr_bool("print").or(p.print);
            p.locked = pr.attr_bool("locked").or(p.locked);
            p.opaque |= pr.children.iter().any(|c| c.name != "anchor")
                || pr.attrs.iter().any(|(k, _)| match k.as_str() {
                    "macro" => false,
                    "print" | "locked" => p.kind == Some(ControlKind::Check) && pr.attr_bool(k).is_none(),
                    "defaultSize" | "autoFill" | "autoLine" | "autoPict" | "disabled" => pr.attr_bool(k) != Some(false),
                    _ => true,
                });
        }
        if p.kind == Some(ControlKind::Check) {
            // Keep the worksheet flags independent of the VML, which can omit different flags.
            // Default sizing and automatic pictures remain opaque (both default to true).
            let off = |k: &str| pr.and_then(|pr| pr.attr_bool(k)) == Some(false);
            p.check_sheet_auto_flags = pr.map(|_| CheckSheetAutoFlags { fill_off: off("autoFill"), line_off: off("autoLine") });
            p.opaque |= p.check_ctrl_prop_opaque
                || !off("defaultSize")
                || !off("autoPict")
                || (ctl.attr("id").is_some() && p.check_lock_text.is_none())
                || check_sheet_control_payload(ctl, pr)
                || shape.is_some_and(|s| s.child("ClientData").and_then(|d| d.attr("ObjectType")).and_then(kind_of) != Some(ControlKind::Check));
        }
        // Other kinds (a list box, an option button), and drop-downs and check boxes the model
        // can't hold, stay with pass-through.
        let Some(kind) = p.kind.filter(|&k| !beyond_model(&p, k)) else { continue };
        let anchor =
            pr.and_then(|pr| pr.child("anchor")).and_then(|a| anchor_from_markers(a, sheet)).or_else(|| shape.and_then(|s| vml_anchor(s, sheet)));
        let Some(anchor) = anchor else { continue };
        let id = cx.next_object_id();
        let name = ctl.attr("name").or_else(|| if kind == ControlKind::Check { shape.and_then(vml_name) } else { None });
        sheet.controls.push(build(p, kind, anchor, name, id));
        read.insert(n);
    }
    // Controls known only from the VML. The ones not read (an option button, or past the limit)
    // stay in the kept drawing. A shape's name is its `id` when that isn't the `_x0000_s` number
    // (the number is then in `o:spid`).
    for s in &shapes {
        let n = number_of(s);
        if n.is_some_and(|n| listed.contains(&n)) {
            continue;
        }
        let kind = s.child("ClientData").and_then(|d| d.attr("ObjectType")).and_then(kind_of);
        if let Some(n) = n.filter(|_| matches!(kind, Some(ControlKind::Drop | ControlKind::Check))) {
            known.insert(n, None);
        }
        let _ = n.filter(|_| sheet.controls.len() < MAX_CONTROLS).and_then(|n| {
            let p = props_from_vml(s)?;
            let kind = p.kind.filter(|&k| !beyond_model(&p, k))?;
            let anchor = vml_anchor(s, sheet)?;
            let id = cx.next_object_id();
            let name = vml_name(s);
            sheet.controls.push(build(p, kind, anchor, name, id));
            known.insert(n, name.map(str::to_string));
            read.insert(n);
            Some(())
        });
    }
    ReadControls { modelled: read, known, drawing: None }
}

fn anchor_from_markers(a: &El, sheet: &Sheet) -> Option<Anchor> {
    let (cell, dx, dy) = crate::drawing::marker(a.child("from")?);
    let (to, tdx, tdy) = a.child("to").map(crate::drawing::marker).unwrap_or((cell, dx + 64.0, dy + 20.0));
    let x1 = sheet.col_left(cell.col) + dx as f64;
    let y1 = sheet.row_top(cell.row) + dy as f64;
    let x2 = sheet.col_left(to.col) + tdx as f64;
    let y2 = sheet.row_top(to.row) + tdy as f64;
    let mode = match (a.flag("moveWithCells", false), a.flag("sizeWithCells", false)) {
        (_, true) => AnchorMode::MoveAndSize,
        (true, false) => AnchorMode::MoveOnly,
        _ => AnchorMode::Absolute,
    };
    Some(Anchor { cell, dx, dy, width: (x2 - x1).clamp(1.0, 10_000.0) as f32, height: (y2 - y1).clamp(1.0, 10_000.0) as f32, mode })
}

/// A VML `x:Anchor` (`LeftColumn, LeftOffset, TopRow, TopOffset, RightColumn, RightOffset,
/// BottomRow, BottomOffset`, offsets in pixels).
fn vml_anchor(shape: &El, sheet: &Sheet) -> Option<Anchor> {
    let data = shape.child("ClientData")?;
    let is_check = data.attr("ObjectType").and_then(kind_of) == Some(ControlKind::Check);
    let [c1, dx1, r1, dy1, c2, dx2, r2, dy2] = vml_anchor_numbers(&data.child("Anchor")?.text)?;
    let col = |c: i64| c.clamp(0, gridcraft_core::MAX_COLS as i64 - 1) as u32;
    let row = |r: i64| r.clamp(0, gridcraft_core::MAX_ROWS as i64 - 1) as u32;
    let off = |v: i64| v.clamp(0, 10_000) as f64;
    let cell = CellRef::new(row(r1), col(c1));
    let x1 = sheet.col_left(cell.col) + off(dx1);
    let y1 = sheet.row_top(cell.row) + off(dy1);
    let x2 = sheet.col_left(col(c2)) + off(dx2);
    let y2 = sheet.row_top(row(r2)) + off(dy2);
    // Excel reverses the VML flag values (MS-OI29500 §19.4.2.42 and §19.4.2.59).
    let moves = |name| data.child(name).is_none_or(|c| is_check && vml_drop_bool(c) == Some(false));
    let mode = match (moves("MoveWithCells"), moves("SizeWithCells")) {
        (_, true) => AnchorMode::MoveAndSize,
        (true, false) => AnchorMode::MoveOnly,
        _ => AnchorMode::Absolute,
    };
    Some(Anchor {
        cell,
        dx: off(dx1) as f32,
        dy: off(dy1) as f32,
        width: (x2 - x1).clamp(1.0, 10_000.0) as f32,
        height: (y2 - y1).clamp(1.0, 10_000.0) as f32,
        mode,
    })
}

fn vml_anchor_numbers(text: &str) -> Option<[i64; 8]> {
    let mut nums = text.split(',');
    let mut anchor = [0; 8];
    for n in &mut anchor {
        *n = nums.next()?.trim().parse().ok()?;
    }
    nums.next().is_none().then_some(anchor)
}
