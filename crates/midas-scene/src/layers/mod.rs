//! Concrete [`SceneLayer`](crate::SceneLayer) implementations.
//!
//! Each file implements one visual concern; they are independently
//! testable and compose via [`ChartSceneBuilder`](crate::ChartSceneBuilder).

mod annotations;
mod candle;
mod crosshair;
mod grid;
mod holiday;
mod session_band;
mod session_separator;
mod volume;

pub use annotations::{
    DecoratorLayer, LevelLayer, LevelView, OrderBracketLayer, OrderBracketView, PriceLineLayer,
    PriceLineView, Side,
};
pub use candle::{CandleLayer, CandleStyle, SharedCandleSeries};
pub use crosshair::CrosshairLayer;
pub use grid::{GridLayer, GridStyle};
pub use holiday::HolidayMarkerLayer;
pub use session_band::{SessionBandLayer, SessionPalette};
pub use session_separator::{SeparatorStyle, SessionBoundary, SessionSeparatorLayer};
pub use volume::{VolumeLayer, VolumeStyle};
