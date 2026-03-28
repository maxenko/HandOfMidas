//! midas-core: Shared types, IDs, events, and configuration.
//!
//! This is the leaf crate of the workspace. It has no internal dependencies.
//! Every other midas crate depends on this one, so keep it small and stable.

pub mod id;
pub mod timeframe;
pub mod candle_data;
pub mod config;

// ── Planned modules (uncomment as implemented) ──────────────────────
// pub mod events;       // MarketEvent, ChartEvent, UIEvent
// pub mod time_axis;    // TimeAxisController

/// Re-export common types at crate root for ergonomic imports.
/// Example: `use midas_core::{Timeframe, CandleData, AppConfig};`
pub use id::{ChartId, PaneId, SymbolId};
pub use timeframe::Timeframe;
pub use candle_data::CandleData;
pub use config::AppConfig;
