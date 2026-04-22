//! Slice 2: new [`MarketDataSource`] provider trait.
//!
//! This is the router-era trait — object-safe via
//! `#[async_trait::async_trait]`, IB-faithful semantics. Both the sim
//! (slice 3) and the IB adapter (slice 4) will implement it. Returns
//! [`TickStream`] / [`RealtimeBarStream`] / [`HistoricalStream`] handle
//! types that auto-cancel upstream on drop (BR-2).
//!
//! The pre-existing historical-only trait in
//! [`crate::market_data`](crate::market_data) (also called
//! `MarketDataSource` inside that module) is retained untouched and
//! stays in use by the legacy `BrokerEngine` path until it is
//! deleted in slice 9.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use midas_broker_core::market_data::Bar;
#[cfg(any(test, feature = "test_inject"))]
use midas_broker_core::market_data::MarketEvent;
use midas_broker_core::market_data::{
    ConnectionState, ContractDetails, FarmStatus, GenericTicks, IbDuration, MarketDataError,
    SecurityType, SymbolKey, TickByTickKind, Timeframe, WhatToShow,
};
use tokio::sync::{broadcast, watch};

use crate::stream::{HistoricalStream, RealtimeBarStream, TickStream};

/// Router-era provider trait (slice 2).
///
/// Object-safety is contractual (M-1): a contract test in
/// `tests/trait_object_safety.rs` asserts that
/// `Arc<dyn MarketDataSource>` type-checks. All methods are
/// `#[async_trait]`-desugared to `Pin<Box<dyn Future + Send>>` so
/// trait objects remain boxable.
///
/// ## Error semantics (M-3)
///
/// The `subscribe_*` methods return `Ok(stream)` even if the provider
/// later rejects the subscription (e.g.
/// [`MarketDataError::NoPermission`], IB error code 354). In that case
/// the stream emits a terminal
/// [`MarketEvent::Error`](midas_broker_core::market_data::MarketEvent::Error)
/// (surfaced to the caller via
/// [`TickStream::last_error`](crate::stream::TickStream::last_error)
/// per NM-5) and then closes. Consumers observe
/// [`tokio::sync::broadcast::error::RecvError::Closed`] and can inspect
/// `last_error()` for the classified cause.
#[async_trait]
#[allow(
    clippy::too_many_arguments,
    reason = "historical_bars / historical_stream mirror IB's own parameter set"
)]
pub trait MarketDataSource: Send + Sync {
    /// Subscribe to sampled L1 ticks (IB `reqMktData`).
    ///
    /// IB samples at roughly 250 ms; the sim emits at most one `Last`
    /// and one bid/ask set per sample window regardless of internal
    /// drift (BR-11). `generic_ticks` (BR-10) carries IB's generic tick
    /// list — e.g. 233 (RT Volume), 293 (Trade Count).
    async fn subscribe_ticks(
        &self,
        symbol: &SymbolKey,
        con_id: i32,
        generic_ticks: GenericTicks,
    ) -> Result<TickStream, MarketDataError>;

    /// Subscribe to unsampled tick-by-tick data (IB
    /// `reqTickByTickData`).
    ///
    /// IB caps concurrent tick-by-tick subscriptions at 5 symbols with
    /// a 15 s identical-throttle; violations bubble back via
    /// [`TickStream::last_error`](crate::stream::TickStream::last_error)
    /// (BR-11).
    async fn subscribe_tick_by_tick(
        &self,
        symbol: &SymbolKey,
        con_id: i32,
        kind: TickByTickKind,
    ) -> Result<TickStream, MarketDataError>;

    /// Subscribe to 5-second realtime bars (IB `reqRealTimeBars`).
    ///
    /// Separate from [`Self::subscribe_ticks`] because IB treats them
    /// as distinct wire requests. Higher-timeframe candles are the
    /// aggregator's concern.
    async fn subscribe_realtime_bars(
        &self,
        symbol: &SymbolKey,
        con_id: i32,
        what_to_show: WhatToShow,
    ) -> Result<RealtimeBarStream, MarketDataError>;

    /// Fetch historical bars one-shot (BR-9).
    ///
    /// Maps to rust-ibapi 2.10 `historical_data(end_date).await`.
    /// Returns all bars immediately plus the seam boundary
    /// ([`HistoricalBarsResult::last_ts`], the `t_server` value). No
    /// live tail — callers who need continuation use
    /// [`Self::historical_stream`].
    async fn historical_bars(
        &self,
        symbol: &SymbolKey,
        con_id: i32,
        end: DateTime<Utc>,
        duration: IbDuration,
        bar_size: Timeframe,
        what_to_show: WhatToShow,
        use_rth: bool,
    ) -> Result<HistoricalBarsResult, MarketDataError>;

    /// Fetch historical bars with live tail (BR-9).
    ///
    /// Maps to rust-ibapi 2.10 `historical_data_streaming`. Emits
    /// [`HistoricalStreamEvent::Historical`](crate::stream::HistoricalStreamEvent::Historical)
    /// → [`HistoricalStreamEvent::End`](crate::stream::HistoricalStreamEvent::End)
    /// → [`HistoricalStreamEvent::Update`](crate::stream::HistoricalStreamEvent::Update) `*`
    /// with an optional terminal
    /// [`HistoricalStreamEvent::Error`](crate::stream::HistoricalStreamEvent::Error).
    async fn historical_stream(
        &self,
        symbol: &SymbolKey,
        con_id: i32,
        duration: IbDuration,
        bar_size: Timeframe,
        what_to_show: WhatToShow,
        use_rth: bool,
    ) -> Result<HistoricalStream, MarketDataError>;

    /// Resolve a symbol to a fully-qualified contract (M-34).
    ///
    /// Without contract resolution the router only handles SMART-routed
    /// US stocks; every other instrument type needs a round-trip to IB.
    async fn resolve_contract(
        &self,
        symbol: &SymbolKey,
        sec_type: SecurityType,
        exchange: &str,
    ) -> Result<ContractDetails, MarketDataError>;

    /// Farm / connection-status broadcast.
    ///
    /// Shared across all consumers through a single broadcast channel
    /// (cap chosen by the implementation); each caller gets an
    /// independent receiver.
    fn farm_status(&self) -> broadcast::Receiver<FarmStatus>;

    /// Connection state watch.
    ///
    /// `Ready` is the "safe to send orders" marker (connected AND all
    /// farms up AND `nextValidId` received — see M-23).
    fn connection_state(&self) -> watch::Receiver<ConnectionState>;

    /// Stable identity for logs and diagnostics (e.g. `"sim"`, `"ib"`).
    fn name(&self) -> &str;

    /// Test-only event injection path (BR-15).
    ///
    /// Real IB sources leave this as the default no-op; sim sources
    /// override and push the event into their hubs. Gated on the
    /// `test_inject` Cargo feature plus `cfg(test)` so production
    /// builds never see it.
    #[cfg(any(test, feature = "test_inject"))]
    fn inject_for_test(&self, _event: MarketEvent) {}
}

/// Result of [`MarketDataSource::historical_bars`] (BR-9).
///
/// `last_ts` is the `t_server` boundary used by the router's
/// `history_then_live` seam (slice 5): the first live bar will have
/// `ts_open > last_ts`.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoricalBarsResult {
    /// Historical bars, oldest first.
    pub bars: Vec<Bar>,
    /// First bar timestamp in `bars`.
    pub first_ts: DateTime<Utc>,
    /// Last bar timestamp in `bars` — the `t_server` seam boundary.
    pub last_ts: DateTime<Utc>,
}

/// Convenience alias for a boxed, shareable [`MarketDataSource`].
///
/// Exposed so callers can pass one around as a trait object without
/// re-typing `Arc<dyn MarketDataSource>` at every call site.
pub type DynMarketDataSource = Arc<dyn MarketDataSource>;
