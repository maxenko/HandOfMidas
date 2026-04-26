//! midas-core: Shared types, IDs, events, and configuration.
//!
//! This is the leaf crate of the workspace. It has no internal dependencies.
//! Every other midas crate depends on this one, so keep it small and stable.

use serde::{Deserialize, Serialize};

pub mod atr;
pub mod candle_buffer;
pub mod candle_data;
/// D7 of the chart-transition plan: `impl CandleData for
/// midas_bars::CandleSeries`. Thin cross-workspace adapter module;
/// the implementation is the single `impl` block, so the module
/// itself stays tiny.
pub mod candle_data_for_series;
pub mod config;
pub mod id;
pub mod link;
pub mod market_data;
pub mod provider;
pub mod symbol;
pub mod timeframe;

// ── Planned modules (uncomment as implemented) ──────────────────────
// pub mod events;       // MarketEvent, ChartEvent, UIEvent
// pub mod time_axis;    // TimeAxisController

pub use atr::{
    gatr_color, gerchik_gatr_detail, gerchik_gatr_pct, true_range, wilder_atr, GatrResult,
    ATR_PERIOD, GATR_COLOR_GREEN, GATR_COLOR_RED, GATR_LOOKBACK, GATR_THRESHOLD_PCT,
};
pub use candle_buffer::{CandleBuffer, CandleSlice};
pub use candle_data::CandleData;
pub use config::{AppConfig, BrokerBackend, BrokerConnectionConfig, ChartBackend};
/// Re-export common types at crate root for ergonomic imports.
/// Example: `use midas_core::{Timeframe, CandleData, AppConfig};`
pub use id::{AccountPanelId, ChartId, OrderBlotterId, OrderPanelId, PaneId, WatchlistId};
pub use link::{LinkColor, LinkMode};
pub use market_data::MarketSnapshot;
/// Re-export from `midas-bars` so downstream chart crates
/// (`midas-chart`, `midas-render`) can read `CandleData::session_kind`
/// without taking an explicit `midas-bars` dep. The trait method
/// already returns this type.
pub use midas_bars::SessionKindByte;
pub use provider::{ConnectionState, DataProvider, OrderBroker, ProviderError};
pub use symbol::SymbolKey;
pub use timeframe::Timeframe;

// ---------------------------------------------------------------------------
// SecurityType — IB security type for contracts
// ---------------------------------------------------------------------------

/// IB security type identifier. Replaces raw strings like "STK", "OPT", etc.
/// with a type-safe enum that serializes to the same IB API strings.
///
/// MIRROR OF: `crates/midas-core/src/lib.rs::SecurityType`
/// Changes must be kept in sync manually.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SecurityType {
    Stock,
    Option,
    Future,
    Forex,
}

impl std::fmt::Display for SecurityType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stock => f.write_str("STK"),
            Self::Option => f.write_str("OPT"),
            Self::Future => f.write_str("FUT"),
            Self::Forex => f.write_str("CASH"),
        }
    }
}

impl std::str::FromStr for SecurityType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "STK" => Ok(Self::Stock),
            "OPT" => Ok(Self::Option),
            "FUT" => Ok(Self::Future),
            "CASH" => Ok(Self::Forex),
            other => Err(format!("unknown SecurityType: {other}")),
        }
    }
}
