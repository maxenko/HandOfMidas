//! ChartScene -- framework-agnostic intermediate representation.
//!
//! `ChartScene` is the central data contract between chart logic and GPU
//! rendering. It describes **everything** a single chart frame needs to draw,
//! without referencing any iced or wgpu types. It is a plain Rust struct
//! containing only numeric data (positions, colors, matrices).
//!
//! Produced by [`compute_chart_scene()`](crate::compute::compute_chart_scene)
//! and consumed by `midas-render`.

use crate::date_labels::DateLabel;
use crate::instances::{
    AxisLabel, CandleInstance, CrosshairRender, GridLineInstance, LevelRender, VolumeInstance,
};

/// The output of chart logic -- a complete description of what to render.
///
/// Framework-agnostic: no iced, no wgpu types. Just data.
pub struct ChartScene {
    /// Orthographic projection matrix for this frame.
    pub projection: glam::Mat4,
    /// Viewport width in logical pixels.
    pub viewport_width: u32,
    /// Viewport height in logical pixels.
    pub viewport_height: u32,
    /// Background color (RGBA, linear space).
    pub background_color: [f32; 4],

    /// Candle rendering data (`Some` when candle instances were rebuilt).
    pub candles: Option<Vec<CandleInstance>>,
    /// Number of visible candles (may differ from `candles.len()` if candles is `None`).
    pub candle_count: usize,

    /// Volume rendering data (`Some` when volume instances were rebuilt).
    pub volumes: Option<Vec<VolumeInstance>>,
    /// Number of visible volume bars.
    pub volume_count: usize,

    /// GPU-ready grid line instances: horizontal price lines, separator,
    /// vertical time lines, and session boundaries — all in one buffer.
    pub grid_instances: Vec<GridLineInstance>,
    /// X-axis (time) labels.
    pub x_labels: Vec<AxisLabel>,
    /// Y-axis (price) labels.
    pub y_labels: Vec<AxisLabel>,

    /// Horizontal price levels.
    pub levels: Vec<LevelRender>,
    /// Crosshair overlay (if active).
    pub crosshair: Option<CrosshairRender>,

    /// Y position of the separator line between price and volume areas.
    pub separator_y: f32,

    /// Date labels for the time axis (TC2000-style adaptive formatting).
    pub date_labels: Vec<DateLabel>,

    /// Volume Profile horizontal histogram instances (empty if VP disabled).
    pub volume_profile_instances: Vec<GridLineInstance>,

    /// Dirty generation counters -- renderer compares to decide what to upload.
    pub generations: SceneGenerations,
}

/// Generation counters snapshot from [`DirtyFlags`](crate::DirtyFlags).
///
/// The renderer compares these against its [`DirtyTracker`](crate::DirtyTracker)
/// to decide which GPU buffers need re-uploading.
#[derive(Clone, Debug, Default)]
pub struct SceneGenerations {
    /// Candle data generation.
    pub candles: u64,
    /// Camera (viewport/zoom/pan) generation.
    pub camera: u64,
    /// Grid lines / date lines generation.
    pub grid: u64,
    /// Horizontal levels generation.
    pub levels: u64,
    /// Crosshair generation.
    pub crosshair: u64,
    /// Theme/colors generation.
    pub theme: u64,
}
