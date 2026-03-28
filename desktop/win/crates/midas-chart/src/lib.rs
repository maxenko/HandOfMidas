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
pub mod dirty;
pub mod input;
pub mod instances;
pub mod interaction;
pub mod levels;
pub mod scene;
pub mod state;

// ── Re-exports ─────────────────────────────────────────────────────
pub use camera::Camera2D;
pub use compute::compute_chart_scene;
pub use dirty::{DirtyFlags, DirtyTracker};
pub use input::ChartInput;
pub use instances::{
    AxisLabel, CandleInstance, CrosshairRender, GridLine, GridLineInstance,
    LevelRender, OhlcvOverlay, SessionBoundary, VolumeInstance,
};
pub use interaction::{ChartAction, ChartEvent, Key, MouseButton, handle_event};
pub use levels::HorizontalLevel;
pub use scene::{ChartScene, SceneGenerations};
pub use state::{ChartState, InteractionMode, Momentum, YAnimation};

#[cfg(test)]
mod tests {
    #[test]
    fn it_compiles() {
        // Proves the midas-core + midas-data dependency links work.
        // Import a type from each dependency to verify linkage.
        let _ = std::mem::size_of::<midas_core::ChartId>();
    }
}
