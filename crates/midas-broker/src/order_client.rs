//! Slice 2: new [`OrderClient`] trait and supporting types.
//!
//! Companion to [`MarketDataSource`](crate::MarketDataSource). Same
//! object-safe shape via `#[async_trait]`, IB-faithful order semantics
//! per M-18 / M-19 / M-21. The sim (slice 3) and the IB adapter
//! (slice 4) both implement this; the legacy
//! [`BrokerClient`](crate::BrokerClient) stays in place behind a
//! `#[deprecated]` marker until slice 9 removes it.
//!
//! Some types are deliberately new (e.g. [`OrderType`]) rather than
//! reusing the older [`OrderKind`](crate::OrderKind) so the router-era
//! IB surface can carry variants the legacy engine never modeled
//! (`MarketOnClose`, `MarketIfTouched`, `LimitIfTouched`, …).

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use midas_broker_core::market_data::{ErrorCode, SymbolKey};
use tokio::sync::{broadcast, mpsc};

use crate::client::PlaceOrderResult;
use crate::orders::state::OrderStatus;
use crate::orders::types::OrderAction;

// ───────────────────────────────────────────────────────────────────────────
// Re-used types
// ───────────────────────────────────────────────────────────────────────────

// Historical notes:
//
// * [`OrderAction`] (`Buy` / `Sell`) already exists and is the canonical
//   spelling throughout the codebase. We expose it here verbatim plus a
//   matching [`OrderSide`] alias so the router-era vocabulary stays
//   consistent with the plan document's naming.
// * [`OrderStatus`] already exists; the new
//   [`OrderEvent::StatusChanged`] variant carries it directly.

/// Alias for [`OrderAction`] so the router-era vocabulary can speak
/// of an order `side` without renaming the canonical enum.
pub type OrderSide = OrderAction;

// ───────────────────────────────────────────────────────────────────────────
// Order placement spec (M-18)
// ───────────────────────────────────────────────────────────────────────────

/// IB order type (M-18).
///
/// Superset of [`OrderKind`](crate::OrderKind) — the router-era surface
/// needs variants the legacy `BrokerClient` path never modeled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OrderType {
    /// Market order (`MKT`).
    Market,
    /// Limit order (`LMT`).
    Limit,
    /// Stop / stop-market order (`STP`).
    Stop,
    /// Stop-limit order (`STP LMT`).
    StopLimit,
    /// Trailing stop (`TRAIL`).
    TrailingStop,
    /// Market-on-close (`MOC`).
    MarketOnClose,
    /// Limit-on-close (`LOC`).
    LimitOnClose,
    /// Market-if-touched (`MIT`).
    MarketIfTouched,
    /// Limit-if-touched (`LIT`).
    LimitIfTouched,
    /// Pegged-to-market (`PEG MKT`).
    PegToMarket,
    /// Pegged-to-midpoint (`PEG MID`).
    PegToMidpoint,
    /// Relative / pegged-to-primary (`REL`).
    Relative,
    /// Volatility order (`VOL`).
    Volatility,
}

/// IB time-in-force qualifier (M-18).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tif {
    /// Day order (expires at session close).
    Day,
    /// Good-till-cancelled.
    Gtc,
    /// At-the-open.
    Opg,
    /// Immediate-or-cancel.
    Ioc,
    /// Fill-or-kill.
    Fok,
    /// Day-till-cancelled.
    Dtc,
    /// Good-till-date (paired with `good_till_date`).
    Gtd,
}

/// IB one-cancels-all group behaviour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OcaType {
    /// `1` — cancel with block: when one fills, block the others and
    /// cancel them.
    CancelWithBlock,
    /// `2` — reduce with block: reduce remaining and block.
    ReduceWithBlock,
    /// `3` — reduce without block.
    ReduceNonBlock,
}

/// IB conditional-order predicate (M-18).
///
/// Non-exhaustive so new IB condition kinds can be added without
/// breaking consumers that match on known variants.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum OrderCondition {
    /// Price threshold on a specific contract.
    Price {
        /// Contract id whose price drives the predicate.
        con_id: i32,
        /// Exchange the price is observed on.
        exchange: String,
        /// Operator direction: `true` = "is greater-than-or-equal".
        is_more: bool,
        /// Trigger price.
        price: f64,
        /// `true` → AND with previous; `false` → OR.
        is_conjunction: bool,
    },
    /// Wall-clock time threshold.
    Time {
        /// Moment at which the predicate becomes satisfied.
        time: DateTime<Utc>,
        /// `true` → predicate fires at or after `time`.
        is_more: bool,
        /// `true` → AND with previous; `false` → OR.
        is_conjunction: bool,
    },
    /// Margin-cushion threshold (percent).
    Margin {
        /// Cushion percent.
        percent: f64,
        /// `true` → fires when actual cushion is at or above
        /// `percent`.
        is_more: bool,
        /// `true` → AND with previous; `false` → OR.
        is_conjunction: bool,
    },
    /// Execution event on another contract.
    Execution {
        /// Symbol that must trade.
        symbol: String,
        /// IB security type string.
        sec_type: String,
        /// Exchange.
        exchange: String,
        /// `true` → AND with previous; `false` → OR.
        is_conjunction: bool,
    },
    /// Volume threshold on a specific contract.
    Volume {
        /// Contract id.
        con_id: i32,
        /// Exchange.
        exchange: String,
        /// `true` → fires at or above `volume`.
        is_more: bool,
        /// Volume threshold.
        volume: i64,
        /// `true` → AND with previous; `false` → OR.
        is_conjunction: bool,
    },
    /// Percent-change threshold on a specific contract.
    PercentChange {
        /// Contract id.
        con_id: i32,
        /// Exchange.
        exchange: String,
        /// `true` → fires at or above `change_percent`.
        is_more: bool,
        /// Percent change threshold.
        change_percent: f64,
        /// `true` → AND with previous; `false` → OR.
        is_conjunction: bool,
    },
}

/// IB algo strategy name (M-18).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AlgoStrategy {
    /// Volume-weighted average price.
    Vwap,
    /// Time-weighted average price.
    Twap,
    /// Arrival price.
    ArrivalPx,
    /// Dark ice / iceberg.
    DarkIce,
    /// Percent-of-volume.
    PctOfVol,
    /// Adaptive algo.
    Adaptive,
    /// Anything else — carried by IB name.
    Custom(String),
}

/// IB stop-trigger method (M-18).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TriggerMethod {
    /// Default (IB `0`).
    Default,
    /// Double-bid-ask (IB `1`).
    DoubleBidAsk,
    /// Last (IB `2`).
    Last,
    /// Double-last (IB `3`).
    DoubleLast,
    /// Bid-ask (IB `4`).
    BidAsk,
    /// Last or bid-ask (IB `7`).
    LastOrBidAsk,
    /// Midpoint (IB `8`).
    MidPoint,
}

/// Full order specification (M-18).
///
/// Parity with IB's order form — every knob IB exposes on `placeOrder`
/// has a home here. Callers who don't need a field leave it at its
/// default-ish value; the adapter translates to the IB wire format.
#[derive(Debug, Clone)]
pub struct OrderSpec {
    /// IB order id (pre-allocated via [`OrderClient::next_order_id`]).
    pub ib_order_id: i32,
    /// Symbol.
    pub symbol: SymbolKey,
    /// Contract id.
    pub con_id: i32,
    /// Direction.
    pub action: OrderAction,
    /// Type.
    pub order_type: OrderType,
    /// Size.
    pub quantity: f64,
    /// Limit price (Limit / StopLimit / LimitOnClose / LimitIfTouched).
    pub limit_price: Option<f64>,
    /// Stop / trigger price (Stop / StopLimit / MarketIfTouched).
    pub stop_price: Option<f64>,
    /// Parent id for bracket legs.
    pub parent_id: Option<i32>,
    /// Transmit flag — IB holds children until the last child transmits.
    pub transmit: bool,
    /// Time-in-force.
    pub tif: Tif,
    /// Allow fills outside regular trading hours.
    pub outside_rth: bool,
    /// OCA (one-cancels-all) group label.
    pub oca_group: Option<String>,
    /// OCA behaviour.
    pub oca_type: Option<OcaType>,
    /// Conditional-order predicates.
    pub conditions: Vec<OrderCondition>,
    /// Algo strategy.
    pub algo_strategy: Option<AlgoStrategy>,
    /// Algo-specific parameters (key/value pairs on the wire).
    pub algo_params: Vec<(String, String)>,
    /// `goodAfterTime` qualifier.
    pub good_after_time: Option<DateTime<Utc>>,
    /// `goodTillDate` qualifier (paired with [`Tif::Gtd`]).
    pub good_till_date: Option<DateTime<Utc>>,
    /// Iceberg display size.
    pub display_size: Option<i64>,
    /// Hide order from public book.
    pub hidden: bool,
    /// Stop-trigger method.
    pub trigger_method: TriggerMethod,
    /// Discretionary price cushion.
    pub discretionary_amt: Option<f64>,
    /// Sweep-to-fill flag.
    pub sweep_to_fill: bool,
}

/// Parameters accepted by
/// [`OrderClient::modify_order`](OrderClient::modify_order) (M-18).
#[derive(Debug, Clone, Default)]
pub struct OrderModify {
    /// New quantity, if changing.
    pub quantity: Option<f64>,
    /// New limit price, if changing.
    pub limit_price: Option<f64>,
    /// New stop / trigger price, if changing.
    pub stop_price: Option<f64>,
    /// New TIF, if changing.
    pub tif: Option<Tif>,
    /// New outside-RTH flag, if changing.
    pub outside_rth: Option<bool>,
}

// ───────────────────────────────────────────────────────────────────────────
// Order lifecycle events (M-19)
// ───────────────────────────────────────────────────────────────────────────

/// Events emitted on [`OrderClient::order_events`].
///
/// Fills and commissions are split deliberately (M-19) so the adapter
/// can emit `ExecutionDetails` the moment IB acknowledges the fill and
/// `Commission` later when `commissionReport` lands. Consumers
/// correlate via `exec_id`.
#[derive(Debug, Clone)]
pub enum OrderEvent {
    /// Order was accepted by the broker.
    Submitted {
        /// IB order id.
        ib_order_id: i32,
    },
    /// Status changed.
    StatusChanged {
        /// IB order id.
        ib_order_id: i32,
        /// New canonical status.
        status: OrderStatus,
        /// Filled quantity.
        filled: f64,
        /// Remaining quantity.
        remaining: f64,
        /// Average fill price so far.
        avg_fill_price: f64,
    },
    /// A single execution (fill tranche).
    ExecutionDetails {
        /// IB order id.
        ib_order_id: i32,
        /// IB execution id (unique per fill).
        exec_id: String,
        /// Filled quantity in this tranche.
        shares: f64,
        /// Price of this tranche.
        price: f64,
    },
    /// Commission report, correlated to a prior `ExecutionDetails` via
    /// `exec_id`.
    Commission {
        /// IB execution id.
        exec_id: String,
        /// Commission charged.
        commission: f64,
        /// Realised P&L for closing fills.
        realized_pnl: Option<f64>,
        /// Commission currency.
        currency: String,
    },
    /// Order rejected.
    Rejected {
        /// IB order id.
        ib_order_id: i32,
        /// Free-form reason from IB.
        reason: String,
    },
    /// Order cancelled.
    Cancelled {
        /// IB order id.
        ib_order_id: i32,
    },
}

/// Position update snapshot (emitted on
/// [`OrderClient::position_events`]).
#[derive(Debug, Clone)]
pub struct PositionUpdate {
    /// Account id.
    pub account: String,
    /// Symbol.
    pub symbol: String,
    /// IB contract id.
    pub con_id: i32,
    /// Net quantity (positive = long).
    pub quantity: f64,
    /// Average cost basis.
    pub avg_cost: f64,
}

/// Account update event (emitted on
/// [`OrderClient::account_events`]).
#[derive(Debug, Clone)]
pub enum AccountEvent {
    /// IB `updateAccountValue` row.
    Value {
        /// Account id.
        account: String,
        /// Key (e.g. `"CashBalance"`, `"BuyingPower"`).
        key: String,
        /// Raw string value.
        value: String,
        /// Currency or `"BASE"`.
        currency: String,
    },
    /// IB P&L event.
    Pnl {
        /// Account id.
        account: String,
        /// Daily P&L.
        daily_pnl: f64,
        /// Unrealised P&L.
        unrealized_pnl: f64,
        /// Realised P&L.
        realized_pnl: f64,
    },
    /// IB `accountDownloadEnd` marker.
    DownloadEnd {
        /// Account id.
        account: String,
    },
}

// ───────────────────────────────────────────────────────────────────────────
// Cancel-order streaming (BR-13)
// ───────────────────────────────────────────────────────────────────────────

/// Stream of events emitted by [`OrderClient::cancel_order`].
///
/// rust-ibapi 2.10's cancel surface is a stream because IB's response
/// can come back in multiple messages (`Submitted` ack, then a terminal
/// `Cancelled` or an `Error`).
pub struct CancelOrderStream {
    rx: mpsc::Receiver<CancelOrderEvent>,
    cancel: Option<Box<dyn FnOnce() + Send + Sync>>,
}

impl CancelOrderStream {
    /// Build a new handle.
    ///
    /// Intended for use by concrete [`OrderClient`] implementations.
    #[doc(hidden)]
    pub fn new(
        rx: mpsc::Receiver<CancelOrderEvent>,
        cancel: Box<dyn FnOnce() + Send + Sync>,
    ) -> Self {
        Self {
            rx,
            cancel: Some(cancel),
        }
    }

    /// Await the next cancel-lifecycle event. `None` when closed.
    pub async fn next(&mut self) -> Option<CancelOrderEvent> {
        self.rx.recv().await
    }
}

impl Drop for CancelOrderStream {
    fn drop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel();
        }
    }
}

/// Event yielded by [`CancelOrderStream`] (BR-13).
#[derive(Debug, Clone)]
pub enum CancelOrderEvent {
    /// Cancel request accepted by the broker.
    Submitted {
        /// IB order id being cancelled.
        ib_order_id: i32,
    },
    /// Order was cancelled.
    Cancelled {
        /// IB order id.
        ib_order_id: i32,
    },
    /// Cancel failed.
    Error {
        /// IB order id.
        ib_order_id: i32,
        /// Classified code.
        code: ErrorCode,
        /// Raw IB text.
        message: String,
    },
}

// ───────────────────────────────────────────────────────────────────────────
// Reconnect-recovery snapshots (M-21)
// ───────────────────────────────────────────────────────────────────────────

/// Snapshot of an order returned by
/// [`OrderClient::open_orders`] (M-21).
#[derive(Debug, Clone)]
pub struct OpenOrder {
    /// IB order id.
    pub ib_order_id: i32,
    /// IB permanent id (stable across reconnects).
    pub perm_id: Option<i64>,
    /// Symbol.
    pub symbol: SymbolKey,
    /// Direction.
    pub action: OrderAction,
    /// Type.
    pub order_type: OrderType,
    /// Total ordered quantity.
    pub quantity: f64,
    /// Limit price.
    pub limit_price: Option<f64>,
    /// Stop price.
    pub stop_price: Option<f64>,
    /// Time-in-force.
    pub tif: Tif,
    /// Canonical status.
    pub status: OrderStatus,
    /// Filled quantity.
    pub filled: f64,
    /// Remaining quantity.
    pub remaining: f64,
    /// Average fill price.
    pub avg_fill_price: Option<f64>,
    /// Parent order id for bracket legs.
    pub parent_id: Option<i32>,
}

/// Snapshot of a completed order returned by
/// [`OrderClient::completed_orders`] (M-21).
#[derive(Debug, Clone)]
pub struct CompletedOrder {
    /// IB order id.
    pub ib_order_id: i32,
    /// IB permanent id.
    pub perm_id: Option<i64>,
    /// Symbol.
    pub symbol: SymbolKey,
    /// Direction.
    pub action: OrderAction,
    /// Type.
    pub order_type: OrderType,
    /// Total ordered quantity.
    pub quantity: f64,
    /// Filled quantity (may differ from `quantity` on partial fills /
    /// cancels).
    pub filled: f64,
    /// Average fill price.
    pub avg_fill_price: Option<f64>,
    /// Terminal status.
    pub status: OrderStatus,
    /// Completion timestamp as reported by IB.
    pub completed_at: Option<DateTime<Utc>>,
}

// ───────────────────────────────────────────────────────────────────────────
// OrderError
// ───────────────────────────────────────────────────────────────────────────

/// Error returned from [`OrderClient`] methods.
#[derive(Debug, thiserror::Error)]
pub enum OrderError {
    /// Broker is not connected.
    #[error("broker not connected")]
    Disconnected,
    /// Order spec failed local validation.
    #[error("invalid order spec: {0}")]
    InvalidSpec(String),
    /// Broker rejected the order.
    #[error("rejected: {0}")]
    Rejected(String),
    /// No such order known to the broker (cancel / modify target).
    #[error("order not found: {0}")]
    NotFound(i32),
    /// Anything else.
    #[error("{0}")]
    Other(String),
}

// ───────────────────────────────────────────────────────────────────────────
// OrderClient trait
// ───────────────────────────────────────────────────────────────────────────

/// Router-era order-placement trait (slice 2).
///
/// Same object-safety discipline as
/// [`MarketDataSource`](crate::MarketDataSource). The IB adapter and
/// the sim both implement this; the app-side router composes an
/// `Arc<dyn OrderClient>` next to its `Arc<dyn MarketDataSource>`.
#[async_trait]
pub trait OrderClient: Send + Sync {
    /// Reserve the next IB order id (M-12).
    ///
    /// IB publishes valid ids via the `nextValidId` callback; the
    /// adapter awaits that watch before returning. Sim implementations
    /// back this with an atomic counter.
    async fn next_order_id(&self) -> Result<i32, OrderError>;

    /// Place a new order.
    ///
    /// For bracket children, set
    /// [`OrderSpec::parent_id`] and
    /// [`OrderSpec::transmit`] per IB's parent-transmits-last rule.
    async fn place_order(&self, spec: OrderSpec) -> Result<PlaceOrderResult, OrderError>;

    /// Request cancellation of an order (BR-13).
    ///
    /// rust-ibapi 2.10 requires a `manual_order_cancel_time` on manual
    /// cancels; pass `None` to let IB fill in the current server time.
    /// The returned stream yields
    /// [`CancelOrderEvent::Submitted`](CancelOrderEvent::Submitted) →
    /// [`CancelOrderEvent::Cancelled`](CancelOrderEvent::Cancelled) or
    /// [`CancelOrderEvent::Error`](CancelOrderEvent::Error).
    async fn cancel_order(
        &self,
        ib_order_id: i32,
        manual_cancel_time: Option<DateTime<Utc>>,
    ) -> Result<CancelOrderStream, OrderError>;

    /// Modify a working order.
    async fn modify_order(&self, ib_order_id: i32, spec: OrderModify) -> Result<(), OrderError>;

    /// Fetch all currently-open orders (M-21).
    ///
    /// Called after a reconnect to rebuild local order state before
    /// accepting fresh order intent from the UI.
    async fn open_orders(&self) -> Result<Vec<OpenOrder>, OrderError>;

    /// Fetch recently-completed orders (M-21).
    async fn completed_orders(&self) -> Result<Vec<CompletedOrder>, OrderError>;

    /// Subscribe to order lifecycle events (M-19).
    fn order_events(&self) -> broadcast::Receiver<OrderEvent>;

    /// Subscribe to position updates.
    fn position_events(&self) -> broadcast::Receiver<PositionUpdate>;

    /// Subscribe to account / P&L events.
    fn account_events(&self) -> broadcast::Receiver<AccountEvent>;

    /// Stable identity for logs and diagnostics.
    fn name(&self) -> &str;
}
