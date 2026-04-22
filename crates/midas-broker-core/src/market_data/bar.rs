//! Bar (candle) types used across the provider/router/aggregator seam.
//!
//! Upstream IB `historical_data` and `realtime_bars` both materialise as
//! [`Bar`] at the router boundary. The router (and aggregator) compose
//! these into per-timeframe candles for consumers.
//!
//! `Timeframe` is re-exported from the crate root — it already exists as
//! the canonical workspace definition. `TODO(S1b)`: unify with
//! `desktop/win/crates/midas-core::Timeframe`, whose richer boundary /
//! suffix helpers will migrate here at that point.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::SymbolKey;

// Canonical workspace `Timeframe` lives at the crate root; re-export so
// `market_data::Timeframe` works for downstream callers.
pub use crate::Timeframe;

/// A single OHLCV bar carrying router-visible context.
///
/// Shape mirrors what IB emits on `realtime_bars` / `historical_data`,
/// plus a [`BarCompleteness`] marker so consumers can tell "bar closed"
/// from "current bar, not final yet".
///
/// `wap` (volume-weighted average price) is optional because not every
/// `WhatToShow` variant carries it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bar {
    /// Symbol this bar belongs to.
    pub symbol: SymbolKey,
    /// Timeframe of this bar.
    pub timeframe: Timeframe,
    /// Inclusive start of the bar window (UTC).
    pub ts_open: DateTime<Utc>,
    /// Exclusive end of the bar window (UTC).
    pub ts_close: DateTime<Utc>,
    /// Open price.
    pub o: f64,
    /// High price.
    pub h: f64,
    /// Low price.
    pub l: f64,
    /// Close price.
    pub c: f64,
    /// Traded volume over the bar window.
    pub volume: u64,
    /// Number of trades folded into the bar (IB `count`).
    pub trade_count: u32,
    /// Volume-weighted average price, when the upstream provides it.
    pub wap: Option<f64>,
    /// Whether this bar is final or still accumulating.
    pub completeness: BarCompleteness,
}

/// Whether a [`Bar`] has finalised.
///
/// Per M-36, the earlier `Partial { ticks_folded: u32 }` shape was
/// sim-only and leaked into the public API. The router exposes a plain
/// two-state enum; sim-internal tick counts are retained inside the sim
/// crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BarCompleteness {
    /// Bar window has closed; `o`/`h`/`l`/`c`/`volume` are final.
    Completed,
    /// Bar is still the "current" bar for its timeframe.
    Partial,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn sample_bar() -> Bar {
        Bar {
            symbol: SymbolKey {
                contract_id: 265598,
                symbol: "AAPL".into(),
            },
            timeframe: Timeframe::M1,
            ts_open: Utc.with_ymd_and_hms(2026, 1, 2, 14, 30, 0).unwrap(),
            ts_close: Utc.with_ymd_and_hms(2026, 1, 2, 14, 31, 0).unwrap(),
            o: 100.0,
            h: 101.25,
            l: 99.75,
            c: 100.5,
            volume: 12_345,
            trade_count: 42,
            wap: Some(100.37),
            completeness: BarCompleteness::Completed,
        }
    }

    #[test]
    fn bar_serde_roundtrip() {
        let bar = sample_bar();
        let json = serde_json::to_string(&bar).unwrap();
        let back: Bar = serde_json::from_str(&json).unwrap();
        assert_eq!(bar, back);
    }

    #[test]
    fn bar_completeness_serde_roundtrip() {
        for bc in [BarCompleteness::Completed, BarCompleteness::Partial] {
            let json = serde_json::to_string(&bc).unwrap();
            let back: BarCompleteness = serde_json::from_str(&json).unwrap();
            assert_eq!(bc, back);
        }
    }

    #[test]
    fn bar_debug_does_not_panic_on_nan() {
        let mut bar = sample_bar();
        bar.o = f64::NAN;
        bar.h = f64::INFINITY;
        bar.l = f64::NEG_INFINITY;
        let _ = format!("{bar:?}");
    }
}
