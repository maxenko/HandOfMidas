//! ChartInput -- clean input contract for chart scene computation.
//!
//! `ChartInput` defines the exact data needed to produce a renderable
//! [`ChartScene`](crate::scene::ChartScene). By making the input explicit
//! we gain testability, decoupling, and clarity.

use crate::camera::Camera2D;
use crate::dirty::DirtyFlags;
use crate::level_tool::LevelTool;
use crate::levels::HorizontalLevel;
use midas_core::CandleData;

/// Clean input contract for chart scene computation.
///
/// Replaces the old pattern of passing `&MidasApp` (the entire application
/// state). Construct one of these in a test without any iced/wgpu context.
pub struct ChartInput<'a> {
    /// Symbol name for display (e.g. "AAPL").
    pub symbol: &'a str,
    /// Candle data source (trait object for flexibility).
    pub data: &'a dyn CandleData,
    /// Camera defining the visible time/price window.
    pub camera: &'a Camera2D,
    /// Viewport width in logical pixels.
    pub viewport_width: u32,
    /// Viewport height in logical pixels.
    pub viewport_height: u32,
    /// Display DPI scale factor.
    pub dpi_scale: f32,
    /// Background color (RGBA, linear space).
    pub background_color: [f32; 4],
    /// Bull (up) candle color.
    pub bull_color: [f32; 4],
    /// Bear (down) candle color.
    pub bear_color: [f32; 4],
    /// Bull volume bar color.
    pub volume_bull_color: [f32; 4],
    /// Bear volume bar color.
    pub volume_bear_color: [f32; 4],
    /// Grid line color.
    pub grid_color: [f32; 4],
    /// Crosshair position in chart pixel coords (`None` if inactive).
    pub crosshair: Option<(f32, f32)>,
    /// Horizontal price levels to render.
    pub levels: &'a [HorizontalLevel],
    /// Whether session gaps are collapsed (index-based X positioning).
    pub collapse_gaps: bool,
    /// Fraction of viewport height at which the timeline border line sits (0.0–1.0).
    pub timeline_border_ratio: f32,
    /// Volume bar height multiplier (1.0 = default).
    pub volume_scale: f32,
    /// Whether to compute and render the Volume Profile overlay.
    pub show_volume_profile: bool,
    /// Current dirty flags for generation tracking.
    pub dirty: &'a DirtyFlags,
    /// Level tool state for placement/snapping queries.
    pub level_tool: &'a LevelTool,
}
