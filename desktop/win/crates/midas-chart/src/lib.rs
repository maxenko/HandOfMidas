//! midas-chart: Sans-IO chart core.
//!
//! Depends on: midas-core, midas-data
//!
//! This crate contains all chart logic with zero GPU or framework dependencies:
//! - ChartState: the pure state machine for a single chart
//! - ChartScene: intermediate representation (output of chart logic, input to renderer)
//! - ChartInput: clean input contract (replaces passing &MidasApp)
//! - ChartEvent / ChartAction: interaction state machine
//! - Camera2D / coordinate transforms
//! - DirtyFlags / DirtyTracker (canonical definition)
//! - Horizontal levels, crosshair state
//! - Zoom, pan, auto-scale, momentum logic
//!
//! The renderer (midas-render) reads ChartScene to build GPU primitives.
//! The app shell (midas-app) feeds ChartInput and collects ChartScene.

// ── Implemented modules ────────────────────────────────────────────
pub mod camera;
pub mod compute;
pub mod date_labels;
pub mod dirty;
pub mod grid;
pub mod input;
pub mod instances;
pub mod interaction;
pub mod level_tool;
pub mod levels;
pub mod scene;
pub mod state;
pub mod volume_profile;

// ── Re-exports ─────────────────────────────────────────────────────
pub use camera::Camera2D;
pub use compute::VOLUME_AREA_FRACTION;
pub use compute::{compute_chart_scene, compute_y_labels, estimate_candle_duration, format_price};
pub use date_labels::{DateLabel, Tier as DateLabelTier};
pub use dirty::{DirtyFlags, DirtyTracker};
pub use input::ChartInput;
pub use instances::{
    AxisLabel, CandleInstance, CrosshairRender, GridLine, GridLineInstance, LevelRender,
    OhlcvOverlay, SessionBoundary, VolumeInstance,
};
pub use interaction::{
    handle_event, timeline_border_y, volume_handle_y, ChartAction, ChartEvent, Key, MouseButton,
};
pub use level_tool::{LevelTool, LevelToolMode};
pub use levels::{HorizontalLevel, LevelIcon, price_step_for};
pub use scene::{ChartScene, SceneGenerations};
pub use state::{ChartState, InteractionMode, Momentum, YAnimation};
pub use volume_profile::{VolumeProfile, VolumeProfileBin};

#[cfg(test)]
mod tests {
    #[test]
    fn it_compiles() {
        // Proves the midas-core + midas-data dependency links work.
        // Import a type from each dependency to verify linkage.
        let _ = std::mem::size_of::<midas_core::ChartId>();
    }
}
