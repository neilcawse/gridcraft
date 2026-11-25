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
    /// Any saved row height switches a valid custom sheet default to autofit for
    /// automatic rows containing cells or serializable row metadata.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub fit_automatic_rows: bool,
}

impl SavedSheetGeometry {
    /// The positive finite custom sheet height, before Mac point snapping.
    pub fn effective_custom_row_height_pt(self) -> Option<f64> {
        self.default_row_height.filter(|h| self.custom_height == Some(true) && h.is_finite() && *h > 0.0)
    }
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
        let measured: &[(u8, u8)] = if font.name.eq_ignore_ascii_case("Calibri") {
            &[(6, 3), (10, 5), (11, 6), (12, 6), (14, 7), (16, 8), (18, 9), (20, 10), (24, 12), (26, 13), (34, 17)]
        } else if font.name.eq_ignore_ascii_case("Arial") {
            &[(6, 3), (8, 4), (10, 6), (11, 6), (12, 7), (14, 8), (16, 9), (18, 10), (20, 11), (24, 13), (32, 18)]
        } else if font.name.eq_ignore_ascii_case("Times New Roman") {
            &[(10, 5), (12, 6)]
        } else if font.name.eq_ignore_ascii_case("Aptos Narrow") {
            &[(11, 6), (12, 6), (16, 8)]
        } else if font.name.eq_ignore_ascii_case("MS PGothic") {
            &[(11, 6), (12, 6), (32, 16)]
        } else {
            &[]
        };
        let size = font.size;
        let i = measured.partition_point(|&(s, _)| f32::from(s) < size);
        // Unmeasured sizes are estimates, not measured glyph advances: linear between the family's
        // neighbouring measurements, else its nearest measured digit-to-size ratio. Families
        // without measurements use an em fraction.
        let estimate = match (i.checked_sub(1).and_then(|j| measured.get(j)), measured.get(i)) {
            (_, Some(&(s, d))) if f32::from(s) == size => f32::from(d),
            (Some(&(s0, d0)), Some(&(s1, d1))) => {
                f32::from(d0) + (size - f32::from(s0)) * (f32::from(d1) - f32::from(d0)) / (f32::from(s1) - f32::from(s0))
            }
            (Some(&(s, d)), None) | (None, Some(&(s, d))) => size * f32::from(d) / f32::from(s),
            (None, None) => size * 0.56,
        };
        Self::Mac { digit_pt: (estimate.round() as u8).max(1) }
    }

    /// Layout pixels for a Column Width dialog value in characters: W3 inverted, in whole points.
    pub fn mac_chars_px(digit_pt: u8, chars: f64) -> f64 {
        let (d, p) = mac_digit_and_padding(digit_pt);
        let points = if chars < 1.0 { chars * (d + p) } else { chars * d + p };
        points.round() * (4.0 / 3.0)
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
                || {
                    let (d, p) = mac_digit_and_padding(digit_pt);
                    ((f64::from(base.unwrap_or(if has_format { 8 } else { 10 }).min(255)) * d + p) * (4.0 / 3.0)) as f32
                },
                |w| self.stored_width_px(w),
            ),
        }
    }

    pub fn width_chars(self, px: f64) -> f64 {
        match self {
            Self::Legacy => points_to_chars(px),
            Self::Mac { digit_pt } => {
                let mut pt = px * 0.75;
                let whole = pt.round();
                // Imported whole-point dimensions pass through f32 layout pixels. Undo that
                // representation error, without snapping fractional editor-supplied widths.
                if f64::from((whole * (4.0 / 3.0)) as f32) == px {
                    pt = whole;
                }
                let (d, p) = mac_digit_and_padding(digit_pt);
                let hundredths = if pt < d + p { pt * 100.0 / (d + p) } else { (pt - p) * 100.0 / d };
                hundredths.round() / 100.0
            }
        }
    }

    pub fn digit_and_padding_px(self) -> (f64, f64) {
        match self {
            Self::Legacy => (7.0, 5.0),
            Self::Mac { digit_pt } => {
                let (d, p) = mac_digit_and_padding(digit_pt);
                (d * (4.0 / 3.0), p * (4.0 / 3.0))
            }
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

fn mac_digit_and_padding(digit_pt: u8) -> (f64, f64) {
    let d = digit_pt.max(1);
    (f64::from(d), f64::from(2 * d.div_ceil(4) + 1))
}

/// Measured automatic height in points, interpolated between measured sizes. Outside the
/// measured family/size range this is an estimate, not a font-rendering metric.
pub fn mac_line_height_pt(font: &Font) -> f32 {
    // Each table lists exact readings only: the dense runs cover integer sizes up to 40,
    // with additional samples above 40 and some exact small or fractional sizes.
    const CALIBRI: &[u8] = &[15, 16, 17, 19, 20, 21, 23, 24, 25, 26, 28, 29, 30, 31, 33, 34, 35, 37, 38, 39, 40, 42, 43, 44, 45, 47, 48, 49, 50, 52];
    const APTOS_NARROW: &[u8] =
        &[15, 16, 18, 19, 20, 22, 23, 24, 25, 27, 28, 29, 30, 32, 33, 34, 36, 37, 38, 39, 41, 42, 43, 45, 46, 47, 48, 50, 51, 52];
    const ARIAL: &[u8] = &[14, 16, 17, 18, 19, 20, 22, 23, 24, 25, 26, 28, 29, 30, 31, 33, 34, 35, 36, 37, 39, 40, 41, 42, 43, 45, 46, 47, 48, 49];
    const TIMES: &[u8] = &[14, 16, 17, 18, 19, 20, 22, 23, 24, 25, 27, 28, 29, 30, 31, 33, 34, 35, 36, 37, 39, 40, 41, 42, 44, 45, 46, 47, 48, 50];
    const PGOTHIC: &[u8] = &[14, 15, 16, 17, 18, 19, 20, 22, 23, 24, 25, 26, 27, 28, 30, 31, 32, 33, 34, 35, 36, 38, 39, 40, 41, 42, 43, 44, 45, 47];
    let name = &font.name;
    let (low, dense, high): (&[(f32, u8)], &[u8], [u8; 3]) = if name.eq_ignore_ascii_case("Arial") {
        (&[(6., 10), (8., 11), (9., 12), (10., 13), (10.5, 14), (11., 14), (11.5, 16), (12., 16)], ARIAL, [59, 74, 88])
    } else if name.eq_ignore_ascii_case("Times New Roman") {
        (&[(10., 13), (10.5, 14), (11., 14), (11.5, 16), (12., 16)], TIMES, [59, 74, 89])
    } else if name.eq_ignore_ascii_case("Aptos Narrow") {
        (&[(11., 15), (12., 16)], APTOS_NARROW, [63, 78, 93])
    } else if name.eq_ignore_ascii_case("MS PGothic") {
        (&[(11., 14), (12., 15)], PGOTHIC, [56, 69, 83])
    } else {
        (&[(6., 9), (8., 11), (9., 12), (10., 14), (10.5, 15), (11., 15), (11.5, 16), (12., 16)], CALIBRI, [62, 77, 92])
    };
    let size = font.size;
    if (12.0..40.0).contains(&size) {
        let i = size as usize - 11;
        let h = f32::from(dense[i]);
        return (h + size.fract() * (f32::from(dense[i + 1]) - h)).round();
    }
    let (a, h, b, k) = if size < 12.0 {
        let i = low.partition_point(|&(s, _)| s < size).max(1);
        let (a, h) = low[i - 1];
        let (b, k) = low[i];
        (a, h, b, k)
    } else if size <= 40.0 {
        (39., dense[28], 40., dense[29])
    } else if size <= 48.0 {
        (40., dense[29], 48., high[0])
    } else if size <= 60.0 {
        (48., high[0], 60., high[1])
    } else {
        (60., high[1], 72., high[2])
    };
    (f32::from(h) + (size - a) * (f32::from(k) - f32::from(h)) / (b - a)).round().clamp(1.0, 409.0)
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
    fn padding_steps_and_character_ties_follow_excel_mac() {
        for (digit_pt, padding) in [(4, 3), (5, 5), (8, 5), (10, 7), (12, 7), (13, 9), (16, 9), (17, 11), (18, 11)] {
            let g = Geometry::Mac { digit_pt };
            let points = f64::from(8 * u16::from(digit_pt) + padding);
            assert_eq!(g.inherited_width_px(true, None, None), (points * (4.0 / 3.0)) as f32);
            assert_eq!(g.digit_and_padding_px().1, f64::from(padding) * (4.0 / 3.0));
            assert_eq!(g.width_chars(f64::from(g.inherited_width_px(true, None, None))), 8.0);
        }
        let g = Geometry::Mac { digit_pt: 8 };
        assert_eq!(g.width_chars(f64::from(g.stored_width_px(8.25))), 7.63);
        assert_eq!(g.width_chars((66.0 - 0.001) * (4.0 / 3.0)), 7.62);
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
            ("Calibri", 6., 3, 9.),
            ("Calibri", 26., 13, 34.),
            ("Calibri", 34., 17, 44.),
            ("Arial", 6., 3, 10.),
            ("Arial", 8., 4, 11.),
            ("Arial", 14., 8, 18.),
            ("Arial", 32., 18, 40.),
            ("Times New Roman", 10., 5, 13.),
            ("Times New Roman", 21., 11, 27.),
            ("Times New Roman", 35., 18, 44.),
            ("Times New Roman", 40., 20, 50.),
            ("Times New Roman", 72., 36, 89.),
            ("Aptos Narrow", 16., 8, 22.),
            ("MS PGothic", 32., 16, 38.),
            ("Calibri", 16., 8, 21.),
            ("Calibri", 20., 10, 26.),
        ] {
            let font = Font { name: name.into(), size, ..Font::default() };
            assert_eq!(Geometry::mac(&font), Geometry::Mac { digit_pt });
            assert_eq!(mac_line_height_pt(&font), height);
        }
    }

    #[test]
    fn unmeasured_sizes_follow_their_family_measurements() {
        // Estimates, not measurements: Calibri digits measure half the size from 10 to 34 pt.
        for (name, size, digit_pt) in [
            ("Calibri", 22., 11),
            ("Calibri", 28., 14),
            ("Calibri", 40., 20),
            ("Aptos Narrow", 14., 7),
            ("Aptos Narrow", 20., 10),
            ("MS PGothic", 20., 10),
            ("Arial", 9., 5),
            ("Verdana", 10., 6),
        ] {
            let font = Font { name: name.into(), size, ..Font::default() };
            assert_eq!(Geometry::mac(&font), Geometry::Mac { digit_pt }, "{name} {size}");
        }
    }

    #[test]
    fn character_input_inverts_stepped_padding() {
        // W3 inverted: 8 characters read back as 8 at every padding step.
        for (digit_pt, padding) in [(3, 3), (4, 3), (6, 5), (9, 7), (12, 7), (13, 9), (17, 11)] {
            let px = Geometry::mac_chars_px(digit_pt, 8.0);
            assert_eq!(px, f64::from(8 * u16::from(digit_pt) + padding) * (4.0 / 3.0), "{digit_pt}");
            assert_eq!(Geometry::Mac { digit_pt }.width_chars(f64::from(px as f32)), 8.0);
        }
        assert_eq!(Geometry::mac_chars_px(4, 0.5), 4.0 * (4.0 / 3.0));
    }
}
