//! Excel worksheet dimensions. Layout uses 96 units per inch on every platform.

use serde::{Deserialize, Serialize};

use crate::{DEFAULT_COL_WIDTH, Font, points_to_chars};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Geometry {
    #[default]
    Legacy,
    /// Maximum digit width in whole typographic points, as used by Excel for Mac.
    Mac { digit_pt: u8 },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SavedSheetGeometry {
    pub has_format: bool,
    pub base_col_width: Option<u32>,
    pub default_col_width: Option<f64>,
    pub default_row_height: Option<f64>,
    pub custom_height: Option<bool>,
    pub original_width_px: f32,
    pub original_height_px: f32,
}

impl Geometry {
    pub(crate) fn is_legacy(&self) -> bool {
        matches!(self, Self::Legacy)
    }

    /// The worksheet import profile. Other hosts retain the existing 96 dpi conversion.
    pub fn host(font: &Font) -> Self {
        if cfg!(target_os = "macos") { Self::mac(font) } else { Self::Legacy }
    }

    pub fn mac(font: &Font) -> Self {
        const MEASURED: &[(&str, f32, u8)] = &[
            ("Calibri", 11.0, 6),
            ("Aptos Narrow", 11.0, 6),
            ("Arial", 10.0, 6),
            ("Arial", 12.0, 7),
            ("Times New Roman", 12.0, 6),
            ("MS PGothic", 11.0, 6),
        ];
        let digit_pt = MEASURED.iter().find_map(|&(name, size, digit)| (font.size == size && font.name.eq_ignore_ascii_case(name)).then_some(digit));
        Self::Mac {
            digit_pt: digit_pt.unwrap_or_else(|| {
                // Unmeasured families and sizes use an estimate, not a measured glyph advance.
                let em = if font.name.eq_ignore_ascii_case("Times New Roman") { 0.50 } else { 0.56 };
                ((font.size * em).round() as u8).max(1)
            }),
        }
    }

    pub fn stored_width_px(self, width: f64) -> f32 {
        if !width.is_finite() || width <= 0.0 {
            return 0.0;
        }
        let (digit, scale) = match self {
            Self::Legacy => (7.0, 1.0),
            Self::Mac { digit_pt } => (f64::from(digit_pt.max(1)), 4.0 / 3.0),
        };
        ((((256.0 * width.min(255.0) + (128.0 / digit).trunc()) / 256.0) * digit).trunc() * scale) as f32
    }

    pub fn inherited_width_px(self, has_format: bool, base: Option<u32>, width: Option<f64>) -> f32 {
        match self {
            Self::Legacy => {
                if let Some(w) = width {
                    let px = self.stored_width_px(w);
                    if px > 0.0 { px } else { DEFAULT_COL_WIDTH }
                } else if let Some(b) = base {
                    (((f64::from(b.min(255)) * 7.0 + 5.0) / 8.0).ceil() * 8.0) as f32
                } else {
                    DEFAULT_COL_WIDTH
                }
            }
            Self::Mac { digit_pt } => width.map_or_else(
                || ((f64::from(base.unwrap_or(if has_format { 8 } else { 10 }).min(255)) * f64::from(digit_pt.max(1)) + 5.0) * (4.0 / 3.0)) as f32,
                |w| self.stored_width_px(w),
            ),
        }
    }

    pub fn width_chars(self, px: f64) -> f64 {
        match self {
            Self::Legacy => points_to_chars(px),
            Self::Mac { digit_pt } => {
                let pt = px * 0.75;
                let d = f64::from(digit_pt.max(1));
                let chars = if pt < d + 5.0 { pt / (d + 5.0) } else { (pt - 5.0) / d };
                (chars * 100.0).round() / 100.0
            }
        }
    }

    pub fn digit_and_padding_px(self) -> (f64, f64) {
        match self {
            Self::Legacy => (7.0, 5.0),
            Self::Mac { digit_pt } => (f64::from(digit_pt.max(1)) * 4.0 / 3.0, 20.0 / 3.0),
        }
    }

    pub fn custom_height_px(self, points: f64) -> f32 {
        let points = if points.is_finite() { points.clamp(0.0, 409.5) } else { 0.0 };
        let points = match self {
            Self::Legacy => points,
            Self::Mac { .. } => points.floor(),
        };
        (points * 4.0 / 3.0) as f32
    }
}

/// Measured automatic height in points, interpolated between measured sizes. Outside the
/// measured family/size range this is an estimate, not a font-rendering metric.
pub fn mac_line_height_pt(font: &Font) -> f32 {
    const CALIBRI: &[(f32, f32)] = &[(11., 15.), (12., 16.), (14., 19.), (18., 24.), (24., 31.), (30., 39.), (36., 47.)];
    const APTOS_NARROW: &[(f32, f32)] = &[(11., 15.), (12., 16.), (14., 19.), (18., 24.), (24., 32.), (30., 39.), (36., 47.)];
    const ARIAL: &[(f32, f32)] = &[(10., 13.), (11., 14.), (12., 16.), (14., 18.), (18., 23.), (24., 30.), (30., 37.), (36., 45.)];
    const PGOTHIC: &[(f32, f32)] = &[(11., 14.), (12., 15.), (14., 17.), (18., 22.), (24., 28.), (30., 35.), (36., 42.)];
    let name = &font.name;
    let table = if name.eq_ignore_ascii_case("Arial") || name.eq_ignore_ascii_case("Times New Roman") {
        ARIAL
    } else if name.eq_ignore_ascii_case("Aptos Narrow") {
        APTOS_NARROW
    } else if name.eq_ignore_ascii_case("MS PGothic") {
        PGOTHIC
    } else {
        CALIBRI
    };
    let i = table.partition_point(|&(size, _)| size < font.size).clamp(1, table.len() - 1);
    let (a, h) = table[i - 1];
    let (b, k) = table[i];
    (h + (font.size - a) * (k - h) / (b - a)).round().clamp(1.0, 409.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_widths_follow_excel_mac_truncation() {
        // W1–W3: sampled explicit widths, hidden-width boundary and character conversion.
        let inputs = [
            0.0625, 0.125, 0.25, 0.5, 0.75, 0.875, 0.99609375, 1., 1.125, 1.25, 1.5, 2., 4.125, 8.25, 8.296875, 8.30078125, 8.375, 8.4296875, 8.5,
            10.625, 17.671875, 30., 100., 255.,
        ];
        for (digit_pt, expected) in [
            (6, [0, 1, 1, 3, 4, 5, 6, 6, 7, 7, 9, 12, 25, 49, 50, 50, 50, 51, 51, 64, 106, 180, 600, 1530]),
            (7, [0, 1, 2, 3, 5, 6, 7, 7, 8, 9, 10, 14, 29, 58, 58, 58, 59, 59, 59, 74, 124, 210, 700, 1785]),
        ] {
            let geometry = Geometry::Mac { digit_pt };
            for (input, points) in inputs.into_iter().zip(expected) {
                assert_eq!(geometry.stored_width_px(input), (points as f64 * 4.0 / 3.0) as f32, "{digit_pt}, {input}");
            }
        }
        let geometry = Geometry::Mac { digit_pt: 6 };
        assert_eq!(geometry.width_chars(12.0), 0.82);
        assert_eq!(geometry.width_chars(68.0), 7.67);
    }

    #[test]
    fn inherited_width_uses_attribute_presence_and_precedence() {
        // W4: absent element, missing attributes, base width and explicit default precedence.
        let g = Geometry::Mac { digit_pt: 6 };
        for (format, base, width, points) in [
            (false, None, None, 65.),
            (true, None, None, 53.),
            (true, Some(4), None, 29.),
            (true, Some(4), Some(13.25), 79.),
            (true, None, Some(4.), 24.),
        ] {
            assert_eq!((g.inherited_width_px(format, base, width) * 0.75).round(), points);
        }
        assert_eq!(Geometry::Legacy.inherited_width_px(false, None, None), 64.);
    }

    #[test]
    fn row_height_uses_profile_and_calibrated_family() {
        // H1, H3, H7: custom-height flooring, Normal baselines and measured font metrics.
        let g = Geometry::Mac { digit_pt: 6 };
        for (raw, points) in [(5., 5.), (10.25, 10.), (15.49, 15.), (15.5, 15.), (15.51, 15.), (16.5, 16.), (21.75, 21.), (30.75, 30.)] {
            assert_eq!((g.custom_height_px(raw) * 0.75).round(), points);
        }
        assert_eq!(Geometry::Legacy.custom_height_px(15.75), 21.);
        for (name, size, digit_pt, height) in [
            ("Calibri", 11., 6, 15.),
            ("Aptos Narrow", 11., 6, 15.),
            ("Arial", 10., 6, 13.),
            ("Arial", 12., 7, 16.),
            ("Times New Roman", 12., 6, 16.),
            ("MS PGothic", 11., 6, 14.),
        ] {
            let font = Font { name: name.into(), size, ..Font::default() };
            assert_eq!(Geometry::mac(&font), Geometry::Mac { digit_pt });
            assert_eq!(mac_line_height_pt(&font), height);
        }
    }
}
