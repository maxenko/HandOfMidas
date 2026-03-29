//! Color constants and theme definitions for chart rendering.
//!
//! All colors are in **linear RGB** space (NOT sRGB). The GPU surface
//! format (typically `Bgra8UnormSrgb`) handles the linear-to-sRGB
//! conversion automatically.

/// Chart color theme containing all colors used by the rendering pipelines.
///
/// Colors are `[f32; 4]` in linear RGBA space. The alpha channel is always
/// present (1.0 for fully opaque).
#[derive(Clone, Debug)]
pub struct ChartTheme {
    /// Chart background color.
    pub background: [f32; 4],
    /// Bullish candle body color (close >= open).
    pub bull: [f32; 4],
    /// Bearish candle body color (close < open).
    pub bear: [f32; 4],
    /// Bullish volume bar color (semi-transparent).
    pub volume_bull: [f32; 4],
    /// Bearish volume bar color (semi-transparent).
    pub volume_bear: [f32; 4],
    /// Minor grid line color (semi-transparent).
    pub grid: [f32; 4],
    /// Major grid line color (slightly brighter).
    pub grid_major: [f32; 4],
    /// Text color for axis labels.
    pub text: [f32; 4],
    /// Crosshair line color.
    pub crosshair: [f32; 4],
}

/// Convert an sRGB `u8` triplet to linear `[f32; 4]` with the given alpha.
///
/// Uses the standard sRGB-to-linear conversion:
/// - If srgb <= 0.04045: linear = srgb / 12.92
/// - Else: linear = ((srgb + 0.055) / 1.055) ^ 2.4
#[inline]
pub fn srgb_to_linear(r: u8, g: u8, b: u8, a: f32) -> [f32; 4] {
    [
        srgb_component_to_linear(r),
        srgb_component_to_linear(g),
        srgb_component_to_linear(b),
        a,
    ]
}

/// Convert a single sRGB `u8` component to linear `f32`.
#[inline]
fn srgb_component_to_linear(value: u8) -> f32 {
    let s = value as f32 / 255.0;
    if s <= 0.04045 {
        s / 12.92
    } else {
        ((s + 0.055) / 1.055).powf(2.4)
    }
}

/// Convert a linear `f32` component back to sRGB `u8`.
#[inline]
pub fn linear_component_to_srgb(value: f32) -> u8 {
    let s = if value <= 0.0031308 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0 + 0.5).clamp(0.0, 255.0) as u8
}

/// Dark theme suitable for trading applications.
///
/// Modeled after professional charting platforms with a dark navy background.
/// All colors are in linear RGB space.
pub fn dark_theme() -> ChartTheme {
    ChartTheme {
        // Dark navy background: sRGB #1a1a2e
        background: srgb_to_linear(0x1a, 0x1a, 0x2e, 1.0),
        // Green bull candle: sRGB #26a69a (teal-green)
        bull: srgb_to_linear(0x26, 0xa6, 0x9a, 1.0),
        // Red bear candle: sRGB #ef5350
        bear: srgb_to_linear(0xef, 0x53, 0x50, 1.0),
        // Bull volume: same hue, semi-transparent
        volume_bull: srgb_to_linear(0x26, 0xa6, 0x9a, 0.30),
        // Bear volume: same hue, semi-transparent
        volume_bear: srgb_to_linear(0xef, 0x53, 0x50, 0.30),
        // Grid lines: subtle white
        grid: srgb_to_linear(0xff, 0xff, 0xff, 0.06),
        // Major grid lines: slightly more visible
        grid_major: srgb_to_linear(0xff, 0xff, 0xff, 0.12),
        // Axis label text: light gray
        text: srgb_to_linear(0xcc, 0xcc, 0xcc, 1.0),
        // Crosshair: semi-transparent white
        crosshair: srgb_to_linear(0xcc, 0xcc, 0xcc, 0.50),
    }
}

/// Light theme for bright-background charting.
///
/// All colors are in linear RGB space.
pub fn light_theme() -> ChartTheme {
    ChartTheme {
        // White background: sRGB #fafafa
        background: srgb_to_linear(0xfa, 0xfa, 0xfa, 1.0),
        // Green bull candle: sRGB #2e7d32
        bull: srgb_to_linear(0x2e, 0x7d, 0x32, 1.0),
        // Red bear candle: sRGB #c62828
        bear: srgb_to_linear(0xc6, 0x28, 0x28, 1.0),
        // Bull volume: semi-transparent
        volume_bull: srgb_to_linear(0x2e, 0x7d, 0x32, 0.20),
        // Bear volume: semi-transparent
        volume_bear: srgb_to_linear(0xc6, 0x28, 0x28, 0.20),
        // Grid lines: subtle black
        grid: srgb_to_linear(0x00, 0x00, 0x00, 0.06),
        // Major grid lines: slightly more visible
        grid_major: srgb_to_linear(0x00, 0x00, 0x00, 0.12),
        // Axis label text: dark gray
        text: srgb_to_linear(0x33, 0x33, 0x33, 1.0),
        // Crosshair: semi-transparent dark gray
        crosshair: srgb_to_linear(0x33, 0x33, 0x33, 0.50),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify that sRGB-to-linear conversion produces expected values.
    #[test]
    fn srgb_to_linear_black() {
        let c = srgb_to_linear(0, 0, 0, 1.0);
        assert_eq!(c[0], 0.0);
        assert_eq!(c[1], 0.0);
        assert_eq!(c[2], 0.0);
        assert_eq!(c[3], 1.0);
    }

    #[test]
    fn srgb_to_linear_white() {
        let c = srgb_to_linear(255, 255, 255, 1.0);
        assert!((c[0] - 1.0).abs() < 1e-4);
        assert!((c[1] - 1.0).abs() < 1e-4);
        assert!((c[2] - 1.0).abs() < 1e-4);
    }

    #[test]
    fn srgb_to_linear_midgray() {
        // sRGB 128 should be roughly 0.216 in linear
        let c = srgb_to_linear(128, 128, 128, 1.0);
        assert!((c[0] - 0.216).abs() < 0.01, "got {}", c[0]);
    }

    /// Round-trip: sRGB -> linear -> sRGB should preserve the value.
    #[test]
    fn srgb_round_trip() {
        for val in [0u8, 1, 50, 128, 200, 254, 255] {
            let linear = srgb_component_to_linear(val);
            let back = linear_component_to_srgb(linear);
            assert_eq!(
                back, val,
                "round-trip failed for sRGB {val}: linear={linear}, back={back}"
            );
        }
    }

    /// All theme colors must have components in [0, 1].
    #[test]
    fn dark_theme_colors_in_range() {
        let theme = dark_theme();
        for color in [
            theme.background,
            theme.bull,
            theme.bear,
            theme.volume_bull,
            theme.volume_bear,
            theme.grid,
            theme.grid_major,
            theme.text,
            theme.crosshair,
        ] {
            for (i, &c) in color.iter().enumerate() {
                assert!(
                    (0.0..=1.0).contains(&c),
                    "dark theme color component [{i}] = {c} out of range"
                );
            }
        }
    }

    #[test]
    fn light_theme_colors_in_range() {
        let theme = light_theme();
        for color in [
            theme.background,
            theme.bull,
            theme.bear,
            theme.volume_bull,
            theme.volume_bear,
            theme.grid,
            theme.grid_major,
            theme.text,
            theme.crosshair,
        ] {
            for (i, &c) in color.iter().enumerate() {
                assert!(
                    (0.0..=1.0).contains(&c),
                    "light theme color component [{i}] = {c} out of range"
                );
            }
        }
    }

    /// Verify alpha values are sensible.
    #[test]
    fn theme_alpha_values() {
        let theme = dark_theme();
        // Opaque colors should have alpha = 1.0
        assert_eq!(theme.background[3], 1.0);
        assert_eq!(theme.bull[3], 1.0);
        assert_eq!(theme.bear[3], 1.0);
        assert_eq!(theme.text[3], 1.0);

        // Semi-transparent colors should have alpha < 1.0
        assert!(theme.volume_bull[3] < 1.0);
        assert!(theme.volume_bear[3] < 1.0);
        assert!(theme.grid[3] < 1.0);
        assert!(theme.crosshair[3] < 1.0);
    }
}
