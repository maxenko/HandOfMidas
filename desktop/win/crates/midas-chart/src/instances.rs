//! GPU-layout instance types for chart rendering.
//!
//! These are pure data structs with `#[repr(C)]` and `bytemuck::Pod` derives.
//! They have **no wgpu dependency**. `midas-render` imports them from this
//! module to build GPU vertex/instance buffers.
//!
//! All pixel coordinates are in logical pixels (pre-DPI-scaling). The
//! projection matrix handles the mapping to NDC.

use bytemuck::{Pod, Zeroable};

// ── Candlestick instances ──────────────────────────────────────────

/// GPU instance data for a single candlestick.
///
/// Used by both wick pass and body pass -- the vertex shader reads different
/// fields depending on the `draw_mode` uniform.
///
/// Size: 48 bytes per instance (12 floats). Aligned to 16 bytes naturally.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct CandleInstance {
    /// Center X of this candle in pixel coordinates.
    pub x: f32,
    /// Top of body (pixel Y -- smaller value = higher on screen).
    pub body_top: f32,
    /// Bottom of body (pixel Y -- larger value = lower on screen).
    pub body_bottom: f32,
    /// Top of wick = high price in pixel Y.
    pub wick_top: f32,
    /// Bottom of wick = low price in pixel Y.
    pub wick_bottom: f32,
    /// Body width in pixels (same for all candles in a frame).
    pub width: f32,
    /// Wick width in physical pixels (always 1.0 after DPI adjustment).
    pub wick_width: f32,
    /// Dim factor: 0.0 = full brightness, 1.0 = dimmed to 30% brightness.
    /// Used for G.ATR hover highlighting.
    pub dim: f32,
    /// RGBA color (linear space, NOT sRGB).
    pub color: [f32; 4],
}

// ── Volume bar instances ───────────────────────────────────────────

/// GPU instance data for a single volume bar.
///
/// Drawn as a filled rectangle at the bottom of the chart area.
/// Size: 32 bytes per instance (8 floats).
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct VolumeInstance {
    /// Center X of the bar in pixel coordinates (same as candle x).
    pub x: f32,
    /// Top of bar in pixel Y (higher volume = lower Y = higher on screen).
    pub y_top: f32,
    /// Bottom of bar in pixel Y (constant: bottom of volume area).
    pub y_bottom: f32,
    /// Bar width in pixels.
    pub width: f32,
    /// RGBA color with alpha for semi-transparency.
    pub color: [f32; 4],
}

// ── Grid line instances ────────────────────────────────────────────

/// GPU instance data for a single axis-aligned grid line.
///
/// Horizontal lines have constant Y and span the full chart width.
/// Vertical lines have constant X and span the full chart height.
/// Each line is rendered as a filled rectangle exactly 1 physical pixel wide.
///
/// Size: 32 bytes per instance (8 floats).
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct GridLineInstance {
    /// Rectangle bounds in pixel coordinates: `[left, top, right, bottom]`.
    pub rect: [f32; 4],
    /// RGBA color (low alpha for subtle grid).
    pub color: [f32; 4],
}

// ── Session boundary marker ──────────────────────────────────────────

/// A session boundary marker -- a faint vertical line between trading sessions.
///
/// Produced when `collapse_gaps` is enabled and there is a time gap between
/// consecutive candles that exceeds the expected candle duration (e.g.,
/// overnight gaps, weekend gaps).
#[derive(Clone, Debug)]
pub struct SessionBoundary {
    /// X position in logical pixels (midpoint between the two adjacent candles).
    pub x: f32,
    /// RGBA color (typically very faint, e.g. `[0.3, 0.3, 0.4, 0.3]`).
    pub color: [f32; 4],
}

// ── High-level scene types (non-Pod) ───────────────────────────────
//
// These are used by the chart logic layer (ChartScene) and are consumed
// by both the grid/label builder and the renderer. They are NOT Pod because
// they contain String and bool fields.

/// A logical grid line with its position and label text.
///
/// Produced by the grid computation functions and consumed by both
/// the label renderer and the grid line instance builder.
#[derive(Clone, Debug)]
pub struct GridLine {
    /// Screen position: Y for horizontal (price) grid lines,
    /// X for vertical (time) grid lines, in logical pixels.
    pub position: f32,
    /// Text to display on the axis (formatted price or time string).
    pub label: String,
    /// Major lines are brighter / thicker than minor lines.
    pub is_major: bool,
}

/// A label displayed on the X or Y axis.
///
/// Used for crosshair axis labels and axis tick labels.
#[derive(Clone, Debug)]
pub struct AxisLabel {
    /// The label text content.
    pub text: String,
    /// Screen X position in logical pixels.
    pub screen_x: f32,
    /// Screen Y position in logical pixels.
    pub screen_y: f32,
    /// Background color (RGBA) for the label badge.
    pub bg_color: [f32; 4],
    /// Text color (RGBA).
    pub text_color: [f32; 4],
}

/// Render data for a single horizontal price level.
///
/// Produced by the chart state and consumed by the level rendering
/// pipeline. Contains both the geometric data and display state.
#[derive(Clone, Debug)]
pub struct LevelRender {
    /// Annotation ID of this level (for interaction targeting).
    pub id: crate::widget::AnnotationId,
    /// Price value of this level.
    pub price: f64,
    /// Screen Y position in logical pixels.
    pub screen_y: f32,
    /// Line color (RGBA).
    pub color: [f32; 4],
    /// Line width in logical pixels.
    pub line_width: f32,
    /// Whether this level is currently selected.
    pub is_selected: bool,
    /// Whether this level is being dragged.
    pub is_being_dragged: bool,
    /// Ghost line position during drag (original Y before drag started).
    pub original_screen_y: Option<f32>,
    /// Price formatted to tick size.
    pub label_text: String,
    /// User-defined label text (displayed on chart).
    pub label: Option<String>,
    /// Icon displayed next to the label.
    pub icon: crate::levels::LevelIcon,
    /// Whether this level is locked (prevents drag/delete).
    pub locked: bool,
}

/// Render data for the crosshair overlay.
///
/// Produced by the chart state when the mouse is over the chart
/// content area. Consumed by the crosshair rendering pipeline.
#[derive(Clone, Debug)]
pub struct CrosshairRender {
    /// Vertical line X position (snapped to candle center), in logical pixels.
    pub vertical_x: f32,
    /// Horizontal line Y position (at cursor), in logical pixels.
    pub horizontal_y: f32,
    /// Price label on the Y axis.
    pub price_label: AxisLabel,
    /// Time label on the X axis.
    pub time_label: AxisLabel,
    /// Line color (typically semi-transparent white or gray).
    pub line_color: [f32; 4],
    /// OHLCV data for the candle under the crosshair (TC2000-style data overlay).
    pub ohlcv_overlay: Option<OhlcvOverlay>,
}

/// OHLCV data overlay displayed in the top-left corner of the chart
/// when the crosshair is active (TC2000 "data box" style).
#[derive(Clone, Debug)]
pub struct OhlcvOverlay {
    /// Symbol name (e.g. "AAPL").
    pub symbol: String,
    /// Formatted date/time string (e.g. "Fri 3/27/26 02:40:00 PM").
    pub datetime: String,
    /// Open price.
    pub open: f32,
    /// High price.
    pub high: f32,
    /// Low price.
    pub low: f32,
    /// Close price.
    pub close: f32,
    /// Volume.
    pub volume: u32,
    /// Whether this candle is bullish (close >= open).
    pub is_bullish: bool,
    /// Price change from previous candle close (if available).
    pub change: Option<f32>,
    /// Percentage change from previous candle close (if available).
    pub change_pct: Option<f32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem;

    #[test]
    fn candle_instance_size_is_48_bytes() {
        assert_eq!(
            mem::size_of::<CandleInstance>(),
            48,
            "CandleInstance must be exactly 48 bytes for GPU layout"
        );
    }

    #[test]
    fn volume_instance_size_is_32_bytes() {
        assert_eq!(
            mem::size_of::<VolumeInstance>(),
            32,
            "VolumeInstance must be exactly 32 bytes for GPU layout"
        );
    }

    #[test]
    fn grid_line_instance_size_is_32_bytes() {
        assert_eq!(
            mem::size_of::<GridLineInstance>(),
            32,
            "GridLineInstance must be exactly 32 bytes for GPU layout"
        );
    }

    #[test]
    fn candle_instance_is_pod() {
        // Verify that bytemuck can cast a CandleInstance to bytes.
        let instance = CandleInstance {
            x: 100.0,
            body_top: 50.0,
            body_bottom: 60.0,
            wick_top: 45.0,
            wick_bottom: 65.0,
            width: 8.0,
            wick_width: 1.0,
            dim: 0.0,
            color: [0.0, 1.0, 0.0, 1.0],
        };
        let bytes: &[u8] = bytemuck::bytes_of(&instance);
        assert_eq!(bytes.len(), 48);
    }

    #[test]
    fn volume_instance_is_pod() {
        let instance = VolumeInstance {
            x: 100.0,
            y_top: 800.0,
            y_bottom: 1080.0,
            width: 8.0,
            color: [0.2, 0.8, 0.3, 0.3],
        };
        let bytes: &[u8] = bytemuck::bytes_of(&instance);
        assert_eq!(bytes.len(), 32);
    }

    #[test]
    fn grid_line_instance_is_pod() {
        let instance = GridLineInstance {
            rect: [0.0, 500.0, 1920.0, 500.667],
            color: [1.0, 1.0, 1.0, 0.1],
        };
        let bytes: &[u8] = bytemuck::bytes_of(&instance);
        assert_eq!(bytes.len(), 32);
    }

    #[test]
    fn candle_instance_zeroed_is_valid() {
        let instance = CandleInstance::zeroed();
        assert_eq!(instance.x, 0.0);
        assert_eq!(instance.color, [0.0; 4]);
    }

    #[test]
    fn volume_instance_zeroed_is_valid() {
        let instance = VolumeInstance::zeroed();
        assert_eq!(instance.x, 0.0);
        assert_eq!(instance.color, [0.0; 4]);
    }

    #[test]
    fn candle_instance_alignment() {
        // Verify that CandleInstance has at least 4-byte alignment
        // (f32 natural alignment).
        assert!(mem::align_of::<CandleInstance>() >= 4);
    }

    #[test]
    fn candle_instance_slice_cast() {
        // Verify that a Vec<CandleInstance> can be cast to &[u8] via
        // bytemuck for GPU buffer upload.
        let instances = vec![
            CandleInstance {
                x: 10.0,
                body_top: 20.0,
                body_bottom: 30.0,
                wick_top: 15.0,
                wick_bottom: 35.0,
                width: 6.0,
                wick_width: 1.0,
                dim: 0.0,
                color: [1.0, 0.0, 0.0, 1.0],
            },
            CandleInstance {
                x: 20.0,
                body_top: 25.0,
                body_bottom: 35.0,
                wick_top: 20.0,
                wick_bottom: 40.0,
                width: 6.0,
                wick_width: 1.0,
                dim: 0.0,
                color: [0.0, 1.0, 0.0, 1.0],
            },
        ];
        let bytes: &[u8] = bytemuck::cast_slice(&instances);
        assert_eq!(bytes.len(), 96); // 2 * 48
    }
}
