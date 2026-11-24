//! Read-only access to the same glyph layout the desktop and web grids use.

use egui::{Context, Id, LayerId, Order, Painter, RawInput};
use gridcraft_engine::core::{CellRef, MAX_COLS, Value};
use gridcraft_engine::display::CellLook;
use gridcraft_engine::model::{HAlign, LineIndex, Sheet, Workbook};

use super::cell_paint::painted_halign;
use super::quadrant::{CellTextInput, wraps_across_column};
use super::theme;

/// Maps worksheet coordinates to text widths, without a viewport or a surface to paint.
pub struct SheetText<'a> {
    wb: &'a Workbook,
    sheet: &'a Sheet,
    index: usize,
    columns: LineIndex,
    z: f32,
}

/// Text submitted to the renderer before viewport/line clipping. Wrapped text retains its source
/// line breaks; the unwrapped path includes the actual overflow hashes and number-format repeats.
pub struct DisplayText {
    pub text: String,
    pub color: [u8; 3],
    pub horizontal: HAlign,
}

impl<'a> SheetText<'a> {
    pub fn new(wb: &'a Workbook, index: usize, sheet: &'a Sheet) -> Self {
        Self { wb, sheet, index, columns: LineIndex::new(&sheet.cols, sheet.default_col_width, MAX_COLS), z: super::geo::sheet_scale(sheet) }
    }

    pub fn cell(&self, frame: &TextFrame, c: CellRef, value: &Value, lk: &CellLook) -> Option<DisplayText> {
        let sh = self.sheet;
        let merge = sh.merge_at(c);
        if value.is_empty()
            || lk.conditional.as_ref().is_some_and(|cf| cf.hide_value)
            || sh.cell_pictures.contains_key(&c)
            || merge.is_some_and(|m| m.start != c)
            || (merge.is_none() && (sh.col_width(c.col) <= 0.0 || sh.row_height(c.row) <= 0.0))
        {
            return None;
        }
        let width = merge.map_or_else(|| sh.col_width(c.col), |m| (self.columns.start(m.end.col + 1) - self.columns.start(m.start.col)) as f32);
        let filter = sh.filter_button_space(c);
        let icon = if lk.conditional.as_ref().is_some_and(|cf| cf.icon.is_some()) { 15.0 } else { 0.0 };
        let width = ((width - filter).max(0.0) - icon) * self.z;
        let text = CellTextInput {
            wb: self.wb,
            si: self.index,
            sh,
            c,
            style: &lk.style,
            text_color: theme::color32(lk.text_color),
            value,
            width,
            z: self.z,
        }
        .prepare(&frame.painter)?;
        let [r, g, b, _] = text.color.to_array();
        let horizontal = painted_halign(text.halign, sh.right_to_left);
        let text = if wraps_across_column(lk.style.align) || (lk.style.align.wrap && lk.style.align.rotation.abs() == 90) {
            text.text
        } else {
            let (galley, _) = text.single_line(&frame.painter, lk.style.align, value, width, self.z);
            galley.job.text.clone()
        };
        Some(DisplayText { text, color: [r, g, b], horizontal })
    }
}

/// Headless font context. Start a fresh pass periodically during bulk reads so egui can evict
/// the previous pass's text layouts, just as it does when painting successive UI frames.
pub struct TextContext(Context);

impl TextContext {
    pub fn new(wb: &Workbook) -> Self {
        let mut names: Vec<_> = wb.styles.iter().map(|(_, s)| s.font.name.clone()).collect();
        names.extend([wb.theme.major_font.clone(), wb.theme.minor_font.clone()]);
        let ctx = Context::default();
        ctx.set_fonts(theme::font_definitions_with(crate::i18n::Language::En, &names));
        Self(ctx)
    }

    pub fn frame(&mut self) -> TextFrame<'_> {
        self.0.begin_pass(RawInput::default());
        let painter = self.0.layer_painter(LayerId::new(Order::Background, Id::unique("headless-grid-text")));
        TextFrame { context: self, painter }
    }
}

pub struct TextFrame<'a> {
    context: &'a mut TextContext,
    painter: Painter,
}

impl Drop for TextFrame<'_> {
    fn drop(&mut self) {
        self.context.0.end_pass().textures_delta.clear();
    }
}
