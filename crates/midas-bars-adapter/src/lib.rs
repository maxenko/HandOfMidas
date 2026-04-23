//! # midas-bars-adapter
//!
//! Adapter bridging the legacy `midas-broker-core::MarketDataSource`
//! world (which emits `Bar` through `Arc`-broadcast stream handles) into
//! the session-aware `midas-stream::BarStream<Item = Candle>` world.
//!
//! This crate is Slice S6 of
//! `plan/session-aware-charts/00b-integration-strategy.md` — Phase B.
//!
//! Scope boundaries (intentional):
//! - Pure adapter. Reads the existing provider surface; never modifies
//!   `midas-broker-core`, `midas-broker`, or `midas-market-data`.
//! - Session tagging is performed at conversion time via
//!   `ExchangeCalendar::classify`; there is no session-aware
//!   aggregation here. That lives in S7.
//! - Supports `BarPeriod::Clock` via the legacy `Timeframe` mapping.
//!   `Session`/`Calendar` periods that have no `Timeframe` equivalent
//!   surface `AdapterError::NoTimeframeMapping`; the full history/live
//!   composite builder rejects those until the S7 aggregator lands.
//!
//! Public surface:
//! - [`timeframe_to_period`] / [`period_to_timeframe`] — bi-directional
//!   map between `Timeframe` and `BarPeriod`.
//! - [`bar_to_candle`] / [`historical_bars_to_candles`] — `Bar` →
//!   `Candle` conversion with calendar-supplied `Session`.
//! - [`SymbolResolver`] / [`StaticSymbolResolver`] /
//!   [`HeuristicSymbolResolver`] — ticker → `(Symbol, calendar, con_id)`
//!   lookup (R2-G-9).
//! - [`RealtimeBarAdapter`] — wraps a `RealtimeBarStream` as a
//!   `BarStream<Candle>` (non-seekable).
//! - [`build_history_then_live`] — composite builder that stitches a
//!   `HistoricalBarsResult` + a `RealtimeBarStream` into a
//!   `HistoryThenLive<FixtureBarStream, RealtimeBarAdapter>`.

#![forbid(unsafe_code)]

mod aggregator;
mod candle;
mod composite;
mod error;
mod historical;
mod period;
mod realtime;
mod resolver;
mod timeout;

pub use crate::aggregator::{
    subscribe_aggregated_bars, subscribe_aggregated_bars_with_timeout, AggregatorConfig,
    AggregatorError, AggregatorOutput, SessionedBarAggregator, SessionedBarStream,
    DEFAULT_PARTIAL_EMIT_RATE_HZ,
};
pub use crate::candle::bar_to_candle;
pub use crate::composite::{
    build_history_then_live, build_history_then_live_with_timeout, HistoryThenLiveAdapter,
};
pub use crate::error::AdapterError;
pub use crate::historical::historical_bars_to_candles;
pub use crate::period::{period_to_timeframe, timeframe_to_period};
pub use crate::realtime::RealtimeBarAdapter;
pub use crate::resolver::{
    HeuristicSymbolResolver, ResolveError, ResolvedSymbol, StaticSymbolResolver, SymbolResolver,
};
pub use crate::timeout::{BROKER_CALL_TIMEOUT, BROKER_CALL_TIMEOUT_SECS};
