//! Router-era [`MarketDataSource`] provider trait + stream handles.
//!
//! Lives in `midas-broker-core` so every downstream consumer (the
//! `midas-market-data` router, any future non-IB backend, and test
//! harnesses) can depend on the neutral provider surface without
//! reaching into the concrete `midas-broker` crate.
//!
//! Object-safe via `#[async_trait::async_trait]`, IB-faithful
//! semantics. The sim (`midas-broker/src/sim/market_data.rs`) and the
//! IB adapter (`midas-broker/src/ib/market_data.rs`) both implement it.
//! Returns [`TickStream`] / [`RealtimeBarStream`] / [`HistoricalStream`]
//! handle types that auto-cancel upstream on drop (BR-2).
//!
//! # Stream handles
//!
//! The three handle types carry `tokio::sync::broadcast::Receiver` /
//! `tokio::sync::mpsc::Receiver` and a `Drop`-fired cancel closure.
//! They are intentionally narrow — no clone, no pub fields — so the
//! refcount-on-drop invariant cannot be sidestepped by callers.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tokio::sync::{broadcast, mpsc, watch};

#[cfg(any(test, feature = "test_inject"))]
use crate::market_data::MarketEvent;
use crate::market_data::{
    Bar, ConnectionState, ContractDetails, FarmStatus, GenericTicks, IbDuration, MarketDataError,
    ReqId, SecurityType, SymbolKey, Tick, TickByTickKind, Timeframe, WhatToShow,
};

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Router-era provider trait (slice 2).
///
/// Object-safety is contractual (M-1): a contract test in
/// `midas-broker/tests/trait_object_safety.rs` asserts that
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
/// [`MarketEvent::Error`](crate::market_data::MarketEvent::Error)
/// (surfaced to the caller via [`TickStream::last_error`] per NM-5) and
/// then closes. Consumers observe
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
    /// [`TickStream::last_error`] (BR-11).
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
    /// [`HistoricalStreamEvent::Historical`] →
    /// [`HistoricalStreamEvent::End`] →
    /// [`HistoricalStreamEvent::Update`] `*` with an optional terminal
    /// [`HistoricalStreamEvent::Error`].
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
    /// `Ready` is the "safe to send orders" marker (connected AND
    /// `nextValidId` received AND the MKT data farm up — see M-23).
    /// Implementations MUST NOT transition to `Ready` until they have
    /// observed at least one `FarmStatus { code: MarketDataFarmOk,
    /// connected: true, .. }` on [`farm_status`](Self::farm_status) —
    /// otherwise stay in `Connected { .. }` and surface the timeout.
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

// ---------------------------------------------------------------------------
// Stream handles
// ---------------------------------------------------------------------------

/// Handle for a per-subscription tick fan-out.
///
/// `TickStream` is `!Clone` on purpose; clones would not increment the
/// router-side refcount and would destabilise the cleanup model.
/// Consumers who need a second view call [`TickStream::resubscribe`],
/// which adds a broadcast receiver but does NOT add a router guard —
/// the original handle still drives cancellation.
pub struct TickStream {
    req_id: ReqId,
    rx: broadcast::Receiver<Arc<Tick>>,
    last_error: Arc<OnceLock<MarketDataError>>,
    cancel: Option<Box<dyn FnOnce() + Send + Sync>>,
}

impl TickStream {
    /// Build a new stream handle.
    ///
    /// Intended for use by concrete [`MarketDataSource`]
    /// implementations (sim + IB adapter). Marked `#[doc(hidden)]` so
    /// it does not advertise itself as a public consumer API.
    #[doc(hidden)]
    pub fn new(
        req_id: ReqId,
        rx: broadcast::Receiver<Arc<Tick>>,
        last_error: Arc<OnceLock<MarketDataError>>,
        cancel: Box<dyn FnOnce() + Send + Sync>,
    ) -> Self {
        Self {
            req_id,
            rx,
            last_error,
            cancel: Some(cancel),
        }
    }

    /// Wire request id of this subscription.
    pub fn req_id(&self) -> ReqId {
        self.req_id
    }

    /// Await the next tick.
    ///
    /// Forwards to [`broadcast::Receiver::recv`]. On
    /// [`broadcast::error::RecvError::Closed`], [`Self::last_error`] may
    /// carry the permanent error that triggered the close (NM-5).
    pub async fn next(&mut self) -> Result<Arc<Tick>, broadcast::error::RecvError> {
        self.rx.recv().await
    }

    /// Create an independent broadcast receiver on the same fan-out.
    ///
    /// Does NOT add a router-side refcount. The original handle retains
    /// sole responsibility for calling the cancel closure on drop.
    pub fn resubscribe(&self) -> broadcast::Receiver<Arc<Tick>> {
        self.rx.resubscribe()
    }

    /// Permanent error that closed this stream, if any (NM-5).
    ///
    /// `None` means either the stream is still live OR it closed
    /// cleanly (consumer dropped its handle).
    pub fn last_error(&self) -> Option<&MarketDataError> {
        self.last_error.get()
    }
}

impl Drop for TickStream {
    fn drop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel();
        }
    }
}

/// Handle for a realtime-bar fan-out.
///
/// `!Clone` on purpose — [`Self::resubscribe`] exists for callers who
/// need another receiver without acquiring a second router guard.
pub struct RealtimeBarStream {
    req_id: ReqId,
    rx: broadcast::Receiver<Arc<Bar>>,
    last_error: Arc<OnceLock<MarketDataError>>,
    cancel: Option<Box<dyn FnOnce() + Send + Sync>>,
}

impl RealtimeBarStream {
    /// Build a new stream handle.
    ///
    /// Intended for use by concrete [`MarketDataSource`]
    /// implementations. Marked `#[doc(hidden)]` so it does not advertise
    /// itself as a public consumer API.
    #[doc(hidden)]
    pub fn new(
        req_id: ReqId,
        rx: broadcast::Receiver<Arc<Bar>>,
        last_error: Arc<OnceLock<MarketDataError>>,
        cancel: Box<dyn FnOnce() + Send + Sync>,
    ) -> Self {
        Self {
            req_id,
            rx,
            last_error,
            cancel: Some(cancel),
        }
    }

    /// Wire request id of this subscription.
    pub fn req_id(&self) -> ReqId {
        self.req_id
    }

    /// Await the next realtime bar.
    pub async fn next(&mut self) -> Result<Arc<Bar>, broadcast::error::RecvError> {
        self.rx.recv().await
    }

    /// Create an independent broadcast receiver on the same fan-out.
    pub fn resubscribe(&self) -> broadcast::Receiver<Arc<Bar>> {
        self.rx.resubscribe()
    }

    /// Permanent error that closed this stream, if any (NM-5).
    pub fn last_error(&self) -> Option<&MarketDataError> {
        self.last_error.get()
    }
}

impl Drop for RealtimeBarStream {
    fn drop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel();
        }
    }
}

/// Event yielded by [`HistoricalStream`].
#[derive(Debug)]
pub enum HistoricalStreamEvent {
    /// Initial bulk payload. Emitted exactly once, before any
    /// [`HistoricalStreamEvent::End`] or later events.
    Historical(Vec<Bar>),
    /// Seam marker. `last_ts` is the `t_server` boundary — the first
    /// live [`HistoricalStreamEvent::Update`] will have `ts_open >
    /// last_ts`.
    End {
        /// First bar timestamp in the bulk payload.
        first_ts: DateTime<Utc>,
        /// Last bar timestamp in the bulk payload (the seam boundary).
        last_ts: DateTime<Utc>,
    },
    /// Trailing live bar emitted while `keep_up_to_date = true`
    /// (rust-ibapi 2.10 "update" event).
    Update(Bar),
    /// Permanent error that terminated the stream.
    Error(MarketDataError),
}

/// Handle for a historical data stream.
///
/// `!Clone` on purpose. `Drop` invokes the cancel closure (BR-2) which
/// signals the upstream publisher to stop emitting further events.
pub struct HistoricalStream {
    req_id: ReqId,
    rx: mpsc::Receiver<HistoricalStreamEvent>,
    cancel: Option<Box<dyn FnOnce() + Send + Sync>>,
}

impl HistoricalStream {
    /// Build a new stream handle.
    ///
    /// Intended for use by concrete [`MarketDataSource`]
    /// implementations. Marked `#[doc(hidden)]` so it does not advertise
    /// itself as a public consumer API.
    #[doc(hidden)]
    pub fn new(
        req_id: ReqId,
        rx: mpsc::Receiver<HistoricalStreamEvent>,
        cancel: Box<dyn FnOnce() + Send + Sync>,
    ) -> Self {
        Self {
            req_id,
            rx,
            cancel: Some(cancel),
        }
    }

    /// Wire request id of this subscription.
    pub fn req_id(&self) -> ReqId {
        self.req_id
    }

    /// Await the next historical event.
    ///
    /// Returns `None` when the upstream publisher closes the channel.
    pub async fn next(&mut self) -> Option<HistoricalStreamEvent> {
        self.rx.recv().await
    }
}

impl Drop for HistoricalStream {
    fn drop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel();
        }
    }
}
