//! Render-adjacent label and overlay metadata for the chart.
//!
//! These types are NOT `Pod` (they contain `String` / `Option`), but they
//! travel alongside the GPU instance data in `crate::instances`: the
//! renderer consumes them to compose axis ticks, crosshair lenses, and
//! the TC2000-style OHLCV overlay.

// ── Axis labels ────────────────────────────────────────────────────

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

// ── Crosshair render data ──────────────────────────────────────────

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
    /// Price lens.
    pub priceline_lens: AxisLabel,
    /// Timeline lens.
    pub timeline_lens: AxisLabel,
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
