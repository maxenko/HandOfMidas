//! Concrete [`SceneLayer`](crate::SceneLayer) implementations.
//!
//! Each file implements one visual concern; they are independently
//! testable and compose via [`ChartSceneBuilder`](crate::ChartSceneBuilder).

mod annotations;
pub(crate) mod candle;
mod crosshair;
mod grid;
mod holiday;
// Slices 6 + 7 of the chart-transition plan live in parallel-owned
// modules. Slice 6 inlines its own math (no cross-workspace dep); the
// module is safe to declare in the root workspace.
mod indicator;
mod session_band;
mod session_separator;
mod volume;
mod volume_profile;

pub use annotations::{
    DecoratorLayer, LevelLayer, LevelView, OrderBracketLayer, OrderBracketView, PriceLineLayer,
    PriceLineView, Side,
};
pub use candle::{BrightIndices, CandleLayer, CandleStyle, SharedCandleSeries};
pub use crosshair::CrosshairLayer;
pub use grid::{GridLayer, GridStyle};
pub use holiday::HolidayMarkerLayer;
pub use indicator::{AtrLayer, AtrStyle, GerchikAtrLayer, GerchikStyle};
pub use session_band::{SessionBandLayer, SessionPalette};
pub use session_separator::{SeparatorStyle, SessionBoundary, SessionSeparatorLayer};
pub use volume::{VolumeLayer, VolumeStyle};
pub use volume_profile::{bin_count_for_viewport, VolumeProfileLayer, VolumeProfileStyle};
