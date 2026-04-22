# Slice 2 — Provider Traits

**Goal.** Define the new `MarketDataSource` and `OrderClient` traits in `midas-broker` with IB-faithful semantics. Expose `Subscription<T>`-style handles. Keep the existing `BrokerClient` trait and old types in place (don't delete; slice 9 removes them). This slice lands only the new contract.

## Scope

### A. `MarketDataSource` trait

`crates/midas-broker/src/market_data_source.rs` (new; or rename existing `market_data.rs`).

```rust
use async_trait::async_trait;
use tokio::sync::{broadcast, watch};
use midas_broker_core::market_data::*;

/// M-1: object-safe via `async_trait`. A contract test confirms
/// `let _: Arc<dyn MarketDataSource> = Arc::new(SimMarketData::new(...));` boxes cleanly.
#[async_trait]
pub trait MarketDataSource: Send + Sync {
    /// Sampled ticks (reqMktData). IB samples ~250 ms; sim emits at most one
    /// Last + one BidAsk-set per sample window (BR-11).
    async fn subscribe_ticks(
        &self,
        symbol: &SymbolKey,
        con_id: i32,
        generic_ticks: GenericTicks,   // BR-10: carries 233/293/etc.
    ) -> Result<TickStream, MarketDataError>;

    /// Tick-by-tick unsampled (reqTickByTickData). Subject to IB's 5-symbol / 15 s cap.
    async fn subscribe_tick_by_tick(
        &self,
        symbol: &SymbolKey,
        con_id: i32,
        kind: TickByTickKind,
    ) -> Result<TickStream, MarketDataError>;

    async fn subscribe_realtime_bars(
        &self,
        symbol: &SymbolKey,
        con_id: i32,
        what_to_show: WhatToShow,
    ) -> Result<RealtimeBarStream, MarketDataError>;

    /// BR-9: one-shot historical. Maps to rust-ibapi 2.10 `historical_data(end_date).await`.
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

    /// BR-9: streaming historical (no end_date). Maps to rust-ibapi 2.10
    /// `historical_data_streaming`. Stream emits Historical(Vec<Bar>) → End → Update loop.
    async fn historical_stream(
        &self,
        symbol: &SymbolKey,
        con_id: i32,
        duration: IbDuration,
        bar_size: Timeframe,
        what_to_show: WhatToShow,
        use_rth: bool,
    ) -> Result<HistoricalStream, MarketDataError>;

    /// M-34: contract resolution required for anything other than SMART-routed US stocks.
    async fn resolve_contract(
        &self,
        symbol: &SymbolKey,
        sec_type: SecurityType,
        exchange: &str,
    ) -> Result<ContractDetails, MarketDataError>;

    fn farm_status(&self) -> broadcast::Receiver<FarmStatus>;
    fn connection_state(&self) -> watch::Receiver<ConnectionState>;

    /// Snapshot the provider's identity (for logs, diagnostics).
    fn name(&self) -> &str;

    /// BR-15: test-only inject path (dev-harness migration).
    #[cfg(any(test, feature = "test_inject"))]
    fn inject_for_test(&self, _event: MarketEvent) { /* default: no-op; sim overrides */ }
}

/// BR-9 result for one-shot historical.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoricalBarsResult {
    pub bars: Vec<Bar>,
    pub first_ts: DateTime<Utc>,
    pub last_ts: DateTime<Utc>,   // the seam point `t_server`
}
```

**M-3 (error semantics).** `subscribe_*` returns `Ok(stream)` even if IB later rejects the permission; the stream then emits `MarketEvent::Error { code: NoMarketDataPermission, .. }` and closes. Document this on the trait doc-comment and assert in the contract test that a mock source which injects a late `Error` then `Close` behaves identically under either backend.

### B. Stream handle types

BR-2: `cancel` is `Option<Box<dyn FnOnce() + Send + Sync>>` so `Drop::drop` (which takes `&mut self`) can `.take()` and invoke it.

NM-5: `TickStream` carries an `Arc<OnceLock<MarketDataError>>` shared with its publisher. On a permanent upstream error (IB error code, wire disconnect mid-stream), the publisher sets `last_error` before dropping the broadcast. Consumers that see `Err(RecvError::Closed)` can call `last_error()` to distinguish "normal close" (None) from "closed due to X" (Some(err)).

```rust
pub struct TickStream {
    req_id: ReqId,
    rx: broadcast::Receiver<Arc<Tick>>,
    last_error: Arc<OnceLock<MarketDataError>>,   // NM-5: error-latch shared with publisher
    cancel: Option<Box<dyn FnOnce() + Send + Sync>>,   // BR-2
}

impl TickStream {
    pub fn req_id(&self) -> ReqId { self.req_id }
    pub async fn next(&mut self) -> Result<Arc<Tick>, TickStreamError> { ... }
    pub fn resubscribe(&self) -> broadcast::Receiver<Arc<Tick>> { ... }

    /// NM-5: if the stream closed due to a permanent error, returns it.
    /// None means either the stream is still live OR it closed cleanly.
    pub fn last_error(&self) -> Option<&MarketDataError> {
        self.last_error.get()
    }
}

impl Drop for TickStream {
    fn drop(&mut self) {
        if let Some(cancel) = self.cancel.take() {   // BR-2: .take() on Option
            cancel();  // sends unsubscribe upstream
        }
    }
}

pub struct RealtimeBarStream {
    req_id: ReqId,
    rx: broadcast::Receiver<Arc<Bar>>,
    cancel: Option<Box<dyn FnOnce() + Send + Sync>>,   // BR-2
}

impl Drop for RealtimeBarStream {
    fn drop(&mut self) {
        if let Some(cancel) = self.cancel.take() { cancel(); }
    }
}

pub struct HistoricalStream {
    req_id: ReqId,
    rx: mpsc::Receiver<HistoricalStreamEvent>,
    cancel: Option<Box<dyn FnOnce() + Send + Sync>>,   // BR-2
}

impl Drop for HistoricalStream {
    fn drop(&mut self) {
        if let Some(cancel) = self.cancel.take() { cancel(); }
    }
}

// M-16: single canonical shape, aligned with rust-ibapi 2.10.
// No per-bar `Bar(Bar)` variant; initial batch is one `Historical(Vec<Bar>)`.
pub enum HistoricalStreamEvent {
    Historical(Vec<Bar>),
    End {
        first_ts: DateTime<Utc>,
        last_ts: DateTime<Utc>,
    },
    Update(Bar),   // emitted repeatedly while subscription is live
    Error(MarketDataError),
}
```

Drop auto-cancels via the stored closure. The closure captures an mpsc sender into the backend's control plane (sim or IB), not a reference to the backend — keeps handles `'static`.

### C. `OrderClient` trait

`crates/midas-broker/src/order_client.rs` (new).

```rust
#[async_trait]
pub trait OrderClient: Send + Sync {
    /// M-12: async because IB's next valid id comes in via `nextValidId` watch.
    /// Sim returns atomic counter. IB awaits `nextValidId`, returns on first arrival.
    async fn next_order_id(&self) -> Result<i32, OrderError>;

    async fn place_order(&self, spec: OrderSpec) -> Result<PlaceOrderResult, OrderError>;

    /// BR-13: rust-ibapi 2.10 requires `manual_order_cancel_time`; returns a stream
    /// of CancelOrderEvent (Submitted → Cancelled | Error). Consumers await the stream.
    async fn cancel_order(
        &self,
        ib_order_id: i32,
        manual_cancel_time: Option<DateTime<Utc>>,
    ) -> Result<CancelOrderStream, OrderError>;

    async fn modify_order(&self, ib_order_id: i32, spec: OrderModify) -> Result<(), OrderError>;

    /// M-21: reconnect-recovery hooks. App calls both after a reconnect to rebuild
    /// local order state before accepting new order intent.
    async fn open_orders(&self) -> Result<Vec<OpenOrder>, OrderError>;
    async fn completed_orders(&self) -> Result<Vec<CompletedOrder>, OrderError>;

    fn order_events(&self) -> broadcast::Receiver<OrderEvent>;
    fn position_events(&self) -> broadcast::Receiver<PositionUpdate>;
    fn account_events(&self) -> broadcast::Receiver<AccountEvent>;

    fn name(&self) -> &str;
}

pub struct CancelOrderStream {
    rx: mpsc::Receiver<CancelOrderEvent>,
    cancel: Option<Box<dyn FnOnce() + Send + Sync>>,   // BR-2
}

pub enum CancelOrderEvent {
    Submitted { ib_order_id: i32 },
    Cancelled { ib_order_id: i32 },
    Error { ib_order_id: i32, code: ErrorCode, message: String },
}

/// M-18: OrderSpec completeness — parity with IB order form.
pub struct OrderSpec {
    pub ib_order_id: i32,
    pub symbol: SymbolKey,
    pub con_id: i32,
    pub action: OrderAction,              // Buy | Sell
    pub order_type: OrderType,            // Market | Limit | Stop | StopLimit | MarketOnClose | ...
    pub quantity: f64,
    pub limit_price: Option<f64>,
    pub stop_price: Option<f64>,
    pub parent_id: Option<i32>,
    pub transmit: bool,
    pub tif: Tif,                         // Day | Gtc | Opg | Ioc | Fok | Dtc
    pub outside_rth: bool,
    pub oca_group: Option<String>,
    pub oca_type: Option<OcaType>,        // CancelWithBlock | ReduceWithBlock | ReduceNonBlock
    pub conditions: Vec<OrderCondition>,
    pub algo_strategy: Option<AlgoStrategy>,
    pub algo_params: Vec<(String, String)>,
    pub good_after_time: Option<DateTime<Utc>>,
    pub good_till_date: Option<DateTime<Utc>>,
    pub display_size: Option<u32>,
    pub hidden: bool,
    pub trigger_method: TriggerMethod,    // Default | DoubleBidAsk | Last | DoubleLast | BidAsk | LastOrBidAsk | MidPoint
    pub discretionary_amt: Option<f64>,
    pub sweep_to_fill: bool,
}

pub enum OcaType { CancelWithBlock, ReduceWithBlock, ReduceNonBlock }
pub enum OrderCondition { Price { .. }, Time { .. }, Margin { .. }, Execution { .. }, Volume { .. }, PercentChange { .. } }
pub enum AlgoStrategy { Vwap, Twap, ArrivalPx, DarkIce, PctOfVol, Adaptive, Custom(String) }
pub enum TriggerMethod { Default, DoubleBidAsk, Last, DoubleLast, BidAsk, LastOrBidAsk, MidPoint }

// M-19: split fill and commission into separate events; correlate via exec_id.
pub enum OrderEvent {
    Submitted { ib_order_id: i32 },
    StatusChanged { ib_order_id: i32, status: OrderStatus, filled: f64, remaining: f64, avg_fill_price: f64 },
    ExecutionDetails { ib_order_id: i32, exec_id: String, shares: f64, price: f64, side: OrderAction, ts: DateTime<Utc> },
    Commission { exec_id: String, commission: f64, realized_pnl: Option<f64>, yield_redemption_date: Option<DateTime<Utc>> },
    Rejected { ib_order_id: i32, reason: String },
    Cancelled { ib_order_id: i32 },
}

#[derive(Debug, thiserror::Error)]
pub enum OrderError {
    #[error("broker not connected")]
    Disconnected,
    #[error("invalid order spec: {0}")]
    InvalidSpec(String),
    #[error("rejected: {0}")]
    Rejected(String),
    #[error("order not found: {0}")]
    NotFound(i32),
    #[error("{0}")]
    Other(String),
}
```

### D. Keep old trait + types behind deprecation

Don't delete `BrokerClient`, `BrokerCallback`, `poll_callbacks`, etc. yet. Add `#[deprecated(since = "router-refactor", note = "Use MarketDataSource + OrderClient")]` markers on the public items but don't enforce `deny(deprecated)` yet.

This lets S3 implement NEW traits in the sim while S7 app-side migration proceeds. The old engine+handle path keeps running until S9.

### E. Engine wiring

Don't modify `BrokerEngine` yet. The new traits are free-standing. The router (slice 5) instantiates them directly by calling `SimMarketData::new(...)` or `IbMarketData::new(...)` — NOT through `start_broker_engine`.

Eventually (slice 9), `start_broker_engine` returns a pair `(Arc<dyn OrderClient>, Arc<dyn MarketDataSource>)` instead of the current `BrokerHandle`. But that's slice 9.

## Tests

- Mock `MarketDataSource` impl in `crates/midas-broker/tests/market_data_source_contract.rs` that exercises `subscribe_ticks` + drop-auto-cancel + `farm_status` + `connection_state`.
- M-1: object-safety contract test — `let _: Arc<dyn MarketDataSource> = Arc::new(MockSource::new());` and `let _: Arc<dyn OrderClient> = Arc::new(MockOrderClient::new());` both compile and pass through a helper that takes `&Arc<dyn _>`.
- M-3: "late error" test — mock source accepts `subscribe_ticks`, then injects `MarketEvent::Error { code: NoMarketDataPermission, .. }` after 20 ms and closes the stream. Consumer observes `Ok(handle)` then `Err(RecvError::Closed)` after the error event.
- `OrderClient` contract test: place_order idempotency, cancel_order returns error on unknown id.
- Compile-only: `impl MarketDataSource for MockSource {}` — confirm trait shape compiles.

## Acceptance

- `cargo build -p midas-broker` passes with new modules.
- `cargo test -p midas-broker` passes (existing tests untouched + new contract tests).
- `cargo clippy -p midas-broker -- -D warnings` clean.
- Existing `TestBroker` and `IbClient` still compile against the old `BrokerClient` trait. Nothing moved yet.

## Risks

- `async_trait` object-safety: ensure the trait is boxable as `Arc<dyn MarketDataSource>`. Test by creating such an Arc in the contract test.
- `Drop` closure boxing: `Box<dyn FnOnce() + Send + Sync>` may require `FnOnce` → `FnMut` tweaks since `Drop::drop` is `&mut self`, not by-value. Use `Option<Box<dyn FnOnce()>>` with `.take()` in Drop.
- `broadcast::Receiver::recv().await` borrows `&mut self` — `TickStream::next` needs `&mut self`. Don't put `rx` behind immutable borrows.
