//! Configuration for the sim market-data + order backends.
//!
//! Each backend (market-data, orders) has its own config struct so
//! tests and callers can tune one side without reaching into the
//! other. [`SimConfig`] wraps both for convenience.

use chrono::{DateTime, Utc};

/// Per-slice config for the sim backend.
#[derive(Debug, Clone, Default)]
pub struct SimConfig {
    /// Market-data simulation knobs.
    pub market_data: SimMarketDataConfig,
    /// Order-client simulation knobs.
    pub orders: SimOrderConfig,
}

/// Tunables for [`SimMarketData`](crate::sim::SimMarketData).
///
/// All duration fields are in milliseconds. Defaults are the
/// IB-faithful values called out in the slice 3 plan
/// (`plan/market-data-router/04-slice-3-sim-backend.md`).
#[derive(Debug, Clone)]
pub struct SimMarketDataConfig {
    /// Tick sample window (BR-11). Default 250 ms.
    pub tick_cadence_ms: u64,
    /// Peak per-tick drift in basis points (uniform random in
    /// `[-drift_bps, +drift_bps]`). Default 1 bp — at the 250 ms tick
    /// cadence that's ≈ 1.6 % std-dev per minute (ceiling), within
    /// realistic intraday volatility for liquid US equities. The
    /// previous default of 10 bps produced a 16 % per-minute std-dev
    /// drift that visibly out-paced the historical-bar price action
    /// on every chart load.
    pub tick_drift_bps: f64,
    /// Emit the initial burst on subscribe. Default `true`.
    pub burst_enabled: bool,
    /// Delay before first burst emits. Default 50 ms.
    pub burst_delay_ms: u64,
    /// Delay after TCP-connect before farm-up events fire. Default
    /// 100 ms (NM-2).
    pub farm_up_delay_ms: u64,
    /// Grace period after a subscription is cancelled during which
    /// late ticks are still allowed through (M-24 / M-25). Default
    /// 200 ms.
    pub late_tick_window_ms: u64,
    /// Realtime-bar aggregation window (5s on IB). Default 5000 ms.
    pub realtime_bar_size_ms: u64,
    /// Override for the historical `last_ts` seam (BR-21). When set,
    /// [`MarketDataSource::historical_bars`](crate::MarketDataSource::historical_bars)
    /// uses this instead of `Utc::now()` so history/live seam tests
    /// are deterministic.
    pub historical_last_ts: Option<DateTime<Utc>>,
    /// Seed IB server version reported via `ConnectionState::Connected`.
    pub server_version: i32,
    /// Next-valid-id seed reported via
    /// [`MarketEvent::OrderingReady`](midas_broker_core::market_data::MarketEvent::OrderingReady).
    pub next_order_id_seed: i32,
    /// RNG seed; `None` seeds from wall clock.
    pub rng_seed: Option<u64>,
    /// Default bid/ask spread in dollars. Default `$0.01`.
    pub default_spread: f64,
}

impl Default for SimMarketDataConfig {
    fn default() -> Self {
        Self {
            tick_cadence_ms: 250,
            tick_drift_bps: 1.0,
            burst_enabled: true,
            burst_delay_ms: 50,
            farm_up_delay_ms: 100,
            late_tick_window_ms: 200,
            realtime_bar_size_ms: 5_000,
            historical_last_ts: None,
            server_version: 176,
            next_order_id_seed: 1_000,
            rng_seed: None,
            default_spread: 0.01,
        }
    }
}

/// Tunables for [`SimOrderClient`](crate::sim::SimOrderClient).
///
/// Mirrors the knobs that used to live on the retired `TestBrokerConfig` —
/// the new client reuses the same simulation semantics.
#[derive(Debug, Clone)]
pub struct SimOrderConfig {
    /// Fill timing: `"instant"` (default) or `"delayed"` /
    /// `"price_triggered"`.
    pub fill_timing: String,
    /// Starting cash.
    pub initial_cash: f64,
    /// Commission per share.
    pub commission_per_share: f64,
    /// Minimum order size that triggers the partial-fill tranches.
    pub partial_fill_threshold: f64,
    /// Number of tranches to split a large fill into. 1 = no partial
    /// fills.
    pub partial_fill_tranches: u32,
    /// Deterministic rejection rate — every Nth order where
    /// `N = round(1.0 / rejection_rate)` is rejected.
    pub rejection_rate: f64,
    /// Next-valid-id seed.
    pub next_order_id_seed: i32,
    /// Default bid/ask spread in dollars.
    pub default_spread: f64,
}

impl Default for SimOrderConfig {
    fn default() -> Self {
        Self {
            fill_timing: "instant".to_string(),
            initial_cash: 100_000.0,
            commission_per_share: 0.005,
            partial_fill_threshold: 0.0,
            partial_fill_tranches: 1,
            rejection_rate: 0.0,
            next_order_id_seed: 1_000,
            default_spread: 0.01,
        }
    }
}
