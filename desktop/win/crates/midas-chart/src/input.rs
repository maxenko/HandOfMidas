//! ChartInput -- clean input contract for chart scene computation.
//!
//! `ChartInput` defines the exact data needed to produce a renderable
//! [`ChartScene`](crate::scene::ChartScene). By making the input explicit
//! we gain testability, decoupling, and clarity.

use crate::camera::Camera2D;
use crate::dirty::DirtyFlags;
use crate::level_tool::LevelTool;
use crate::widget::hit_test::HitZoneKind;
use crate::widget::{Annotation, AnnotationId};
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
    /// Annotations to render (levels, brackets, etc.).
    pub annotations: &'a [Annotation],
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
    /// G.ATR hover highlight: intraday candle index ranges that should
    /// remain bright. Empty slice = no dimming (hover inactive).
    /// Each tuple is `(start_idx, end_idx)` inclusive, sorted ascending.
    pub gatr_bright_ranges: &'a [(usize, usize)],
    /// Currently hovered annotation element (for hover highlight in compute pass).
    pub hovered_annotation: Option<(AnnotationId, HitZoneKind)>,
    /// Decorator groups that are currently expanded. Tuples of
    /// `(annotation_id, group_id)`. Threaded through
    /// [`crate::widget::compute::ComputeContext`] so decorator compute
    /// can emit hover-gated items whose parent line is no longer under
    /// the cursor (first-frame hover edge case — see
    /// `plan/decorator-system/05-interaction.md`).
    pub hovered_decorator_groups: &'a [(AnnotationId, u16)],
    /// Currently selected annotation (for selection glow in compute pass).
    pub selected_annotation: Option<AnnotationId>,
    /// Drag ghost: annotation being dragged and its original (pre-drag) price.
    pub drag_ghost: Option<(AnnotationId, f64)>,
    /// Whether this chart's symbol has `TickerOrderIntent.pinned` set.
    /// Threaded through to [`crate::widget::compute::ComputeContext::pinned`]
    /// so the bracket `pin_toggle_group()` decorator can render its
    /// active vs outlined visual. The app populates this from its
    /// `order_intent_handle.snapshot(symbol)`; sans-IO tests default to
    /// `false`.
    pub pinned: bool,
    /// Whether the ETH (extended trading hours) session-band overlay
    /// is enabled. Drives the `compute_session_bands` pass, which
    /// emits filled rectangles into the existing grid-line bucket
    /// behind the candles.
    ///
    /// `false` short-circuits the pass to a single bool check; set
    /// from `chart.show_extended_hours_bands` in
    /// [`midas_core::config::ChartConfig`].
    pub show_extended_hours_bands: bool,
    /// Bar duration in milliseconds, used by the band pass to compute
    /// the right edge of the trailing bar in each run
    /// (`data.timestamp(last) + bar_duration_ms`). Sourced from
    /// `ChartPanel.timeframe.as_secs() * 1000`. Falls back to the
    /// `compute` module's interpolated estimate when unknown.
    pub bar_duration_ms: i64,
    /// Tint for pre-market bands (RGBA, linear). Defaults to
    /// `LEGACY_BAND_PRE` in `midas-chart` (warm brown, TradingView-
    /// style) but per-chart themes may override.
    pub pre_market_band_color: [f32; 4],
    /// Tint for post-market bands (RGBA, linear). Defaults to
    /// `LEGACY_BAND_POST` (cool blue).
    pub post_market_band_color: [f32; 4],
}
