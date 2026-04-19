//! Central types frozen at Stage 01.
//!
//! Every enum variant / struct field here is declared exactly once, at scaffold
//! time, so that Wave 2 stages can be implemented concurrently without central-
//! file merge conflicts. Wave 2 agents fill in `todo!()` bodies, never touch
//! the shapes in this file.
//!
//! If a variant turns out to be insufficient during Wave 2, add it to a
//! stage-local `*Ext` enum (see `plan/ib-sim/01-architecture.md` §"Extension-
//! enum pattern") and fold it into the base enum as a single Stage-01
//! amendment PR at the end of the wave.

use std::time::Duration;

use midas_broker_core::{ContractSpec, SymbolKey};
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use crate::engine::clock::VirtualInstant;
use crate::scenario::Scenario;

// ---------------------------------------------------------------------------
// Identifiers
// ---------------------------------------------------------------------------

/// Per-connection identifier assigned by the sim at accept time.
#[derive(
    Copy, Clone, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize,
)]
pub struct SessionId(pub u64);

/// IB request identifier supplied by the client on `reqMktData`, `reqHistoricalData`, etc.
#[derive(
    Copy, Clone, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize,
)]
pub struct ReqId(pub i32);

/// IB order identifier — i32 to match the wire protocol.
#[derive(
    Copy, Clone, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize,
)]
pub struct OrderId(pub i32);

// ---------------------------------------------------------------------------
// Subscription keys
// ---------------------------------------------------------------------------

/// Uniquely identifies a market-data subscription across the sim.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct SubKey {
    pub session: SessionId,
    pub req_id: ReqId,
    pub symbol: SymbolKey,
}

/// Type of market-data subscription — maps 1:1 to an IB `reqMktData` variant.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum SubMode {
    StreamingL1 {
        snapshot: bool,
        regulatory_snapshot: bool,
    },
    TickByTick {
        kind: TickByTickKind,
    },
    RealtimeBars5s,
    Historical(HistoricalReq),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum TickByTickKind {
    Last,
    AllLast,
    BidAsk,
    MidPoint,
}

// ---------------------------------------------------------------------------
// Market-data value types
// ---------------------------------------------------------------------------

/// IB tick type — subset we actually emit. Full mapping lives in the protocol layer.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub enum TickType {
    BidSize,
    Bid,
    Ask,
    AskSize,
    Last,
    LastSize,
    High,
    Low,
    Volume,
    Close,
    BidOptionComputation,
    AskOptionComputation,
    LastOptionComputation,
    ModelOption,
    Open,
    Low13Week,
    High13Week,
    Low26Week,
    High26Week,
    Low52Week,
    High52Week,
    AvgVolume,
    OpenInterest,
    OptionHistoricalVol,
    OptionImpliedVol,
    OptionBidExch,
    OptionAskExch,
    OptionCallOpenInterest,
    OptionPutOpenInterest,
    OptionCallVolume,
    OptionPutVolume,
    IndexFuturePremium,
    BidExch,
    AskExch,
    AuctionVolume,
    AuctionPrice,
    AuctionImbalance,
    MarkPrice,
    BidEfpComputation,
    AskEfpComputation,
    LastEfpComputation,
    OpenEfpComputation,
    HighEfpComputation,
    LowEfpComputation,
    CloseEfpComputation,
    LastTimestamp,
    Shortable,
    FundamentalRatios,
    RtVolume,
    Halted,
    BidYield,
    AskYield,
    LastYield,
    CustOptionComputation,
    TradeCount,
    TradeRate,
    VolumeRate,
    LastRthTrade,
    RtHistoricalVol,
    IbDividends,
    BondFactorMultiplier,
    RegulatoryImbalance,
    NewsTick,
    ShortTermVolume3Min,
    ShortTermVolume5Min,
    ShortTermVolume10Min,
    DelayedBid,
    DelayedAsk,
    DelayedLast,
    DelayedBidSize,
    DelayedAskSize,
    DelayedLastSize,
    DelayedHigh,
    DelayedLow,
    DelayedVolume,
    DelayedClose,
    DelayedOpen,
    RtTrdVolume,
    CreditmanMarkPrice,
    CreditmanSlowMarkPrice,
    DelayedBidOption,
    DelayedAskOption,
    DelayedLastOption,
    DelayedModelOption,
    LastExch,
    LastRegTime,
    FuturesOpenInterest,
    AvgOptVolume,
    DelayedLastTimestamp,
    ShortableShares,
}

/// Boolean/bit-flag attributes that accompany TickPrice.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct TickAttribs {
    pub can_auto_execute: bool,
    pub past_limit: bool,
    pub pre_open: bool,
}

/// A 5-second realtime bar (IB `reqRealTimeBars`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bar5s {
    pub time: VirtualInstant,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: i64,
    pub wap: f64,
    pub count: i32,
}

/// A single historical bar (IB `reqHistoricalData`). The bar-size is carried by
/// the request; this struct is just the OHLCVC tuple.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bar {
    pub time: VirtualInstant,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: i64,
    pub wap: f64,
    pub count: i32,
}

/// Historical-data request payload.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HistoricalReq {
    pub contract: ContractSpec,
    /// IB's end-date-time string, e.g. "20260418 16:00:00 US/Eastern" or "" for "now".
    pub end_date_time: String,
    /// IB duration string, e.g. "1 D", "2 W", "3 M".
    pub duration: String,
    /// IB bar-size string, e.g. "1 min", "5 secs", "1 day".
    pub bar_size: String,
    /// IB what-to-show, e.g. "TRADES", "BID_ASK", "MIDPOINT".
    pub what_to_show: String,
    /// 1 = regular trading hours only, 0 = include extended hours.
    pub use_rth: bool,
    /// 1 = epoch seconds, 2 = formatted date.
    pub format_date: i32,
    pub keep_up_to_date: bool,
}

/// Real-time bars request payload (always 5-second bars in IB).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RealTimeBarsReq {
    pub contract: ContractSpec,
    pub bar_size: i32,
    pub what_to_show: String,
    pub use_rth: bool,
}

/// Contract-details request payload.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ContractDetailsReq {
    pub contract: ContractSpec,
}

/// Filter passed to `reqExecutions`.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExecutionFilter {
    pub client_id: Option<i32>,
    pub acct_code: Option<String>,
    pub time: Option<String>,
    pub symbol: Option<String>,
    pub sec_type: Option<String>,
    pub exchange: Option<String>,
    pub side: Option<String>,
}

/// IB `reqMarketDataType` argument.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum MarketDataType {
    Live = 1,
    Frozen = 2,
    Delayed = 3,
    DelayedFrozen = 4,
}

// ---------------------------------------------------------------------------
// Order types
// ---------------------------------------------------------------------------

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub enum Side {
    Buy,
    Sell,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub enum OrderKind {
    Market,
    Limit,
    Stop,
    StopLimit,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub enum OrderStatusCode {
    ApiPending,
    PendingSubmit,
    PreSubmitted,
    Submitted,
    Filled,
    PartiallyFilled,
    Cancelled,
    ApiCancelled,
    Inactive,
}

/// Client-submitted `PLACE_ORDER` payload — engine-facing projection.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlaceOrderReq {
    pub order_id: OrderId,
    pub contract: ContractSpec,
    pub side: Side,
    pub total_quantity: f64,
    pub kind: OrderKind,
    pub limit_price: Option<f64>,
    pub aux_price: Option<f64>,
    pub tif: String,
    pub account: String,
    pub parent_id: Option<OrderId>,
    pub oca_group: Option<String>,
    pub transmit: bool,
}

/// Sim-side projection of the IB `openOrder` callback.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OpenOrder {
    pub order_id: OrderId,
    pub contract: ContractSpec,
    pub side: Side,
    pub total_quantity: f64,
    pub kind: OrderKind,
    pub limit_price: Option<f64>,
    pub aux_price: Option<f64>,
    pub status: OrderStatusCode,
    pub tif: String,
    pub account: String,
    pub parent_id: Option<OrderId>,
    pub oca_group: Option<String>,
}

/// Sim-side projection of the IB `orderStatus` callback.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OrderStatus {
    pub order_id: OrderId,
    pub status: OrderStatusCode,
    pub filled: f64,
    pub remaining: f64,
    pub avg_fill_price: f64,
    pub perm_id: i32,
    pub parent_id: i32,
    pub last_fill_price: f64,
    pub client_id: i32,
    pub why_held: String,
    /// `None` when there's no market-cap-price cap on the order. Emitted as
    /// the canonical `UNSET_DOUBLE` sentinel on the wire, which rust-ibapi
    /// decodes back to `None`. A literal `Some(0.0)` means "explicit zero",
    /// not "absent" — don't collapse the two.
    pub mkt_cap_price: Option<f64>,
}

/// Sim-side projection of the IB `execDetails` callback.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Execution {
    pub req_id: Option<ReqId>,
    pub order_id: OrderId,
    pub exec_id: String,
    pub time: VirtualInstant,
    pub acct_number: String,
    pub exchange: String,
    pub side: Side,
    pub shares: f64,
    pub price: f64,
    pub perm_id: i32,
    pub client_id: i32,
    pub liquidation: i32,
    pub cumulative_quantity: f64,
    pub avg_price: f64,
    pub order_ref: Option<String>,
    pub contract: ContractSpec,
}

/// Sim-side projection of the IB `commissionReport` callback.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CommissionReport {
    pub exec_id: String,
    pub commission: f64,
    pub currency: String,
    pub realized_pnl: Option<f64>,
    pub yield_: Option<f64>,
    pub yield_redemption_date: Option<i32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PositionUpdate {
    pub account: String,
    pub contract: ContractSpec,
    pub position: f64,
    pub avg_cost: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PortfolioValueUpdate {
    pub contract: ContractSpec,
    pub position: f64,
    pub market_price: f64,
    pub market_value: f64,
    pub average_cost: f64,
    pub unrealized_pnl: f64,
    pub realized_pnl: f64,
    pub account: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AcctValueUpdate {
    pub key: String,
    pub value: String,
    pub currency: String,
    pub account: String,
}

// ---------------------------------------------------------------------------
// Emission enums — the central push contracts.
// ---------------------------------------------------------------------------

/// Market-data events the engine emits outbound (and projects internally to
/// `MarketSnapshot` for the order simulator).
#[derive(Clone, Debug, PartialEq)]
pub enum MarketEmission {
    TickPrice {
        key: SubKey,
        tick: TickType,
        price: f64,
        size: Option<i64>,
        attribs: TickAttribs,
    },
    TickSize {
        key: SubKey,
        tick: TickType,
        size: i64,
    },
    TickString {
        key: SubKey,
        tick: TickType,
        value: String,
    },
    TickGeneric {
        key: SubKey,
        tick: TickType,
        value: f64,
    },
    Bar {
        key: SubKey,
        bar: Bar5s,
    },
    HistoricalBatch {
        key: SubKey,
        bars: Vec<Bar>,
        is_complete: bool,
    },
}

/// Order events the engine emits outbound.
#[derive(Clone, Debug, PartialEq)]
pub enum OrderEmission {
    OpenOrder(OpenOrder),
    OrderStatus(OrderStatus),
    Execution(Execution),
    Commission(CommissionReport),
    Reject {
        order_id: OrderId,
        code: i32,
        message: String,
    },
    Position(PositionUpdate),
    PortfolioValue(PortfolioValueUpdate),
    AcctValue(AcctValueUpdate),
    AcctDownloadEnd(String),
    PositionEnd,
}

/// Engine-internal projection from a `MarketEmission` to a snapshot the order
/// simulator reads on each mid-price update.
#[derive(Clone, Debug, PartialEq)]
pub struct MarketSnapshot {
    pub symbol: SymbolKey,
    pub mid: f64,
    pub bid: f64,
    pub ask: f64,
    /// Set if the update carried a trade.
    pub last: f64,
    pub volume: Option<i64>,
    pub ts: VirtualInstant,
}

// ---------------------------------------------------------------------------
// Quirk violation — central enum (Stage 05 fills in logic).
// ---------------------------------------------------------------------------

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum ViolationAction {
    /// Emit the error then close the session after a brief delay (mirrors real IB).
    DisconnectAfterError,
    /// Reject the originating request; session continues.
    RejectRequest,
    /// Log the violation, keep serving — used for informational pacing warnings.
    WarnOnly,
}

/// All the ways the sim observes a client tripping IB's quirk limits. The
/// protocol layer translates each variant to the corresponding `ErrMsg` frame.
#[derive(Clone, Debug, PartialEq)]
pub enum QuirkViolation {
    RateLimit {
        code: i32,
        message: String,
        action: ViolationAction,
    },
    LineLimit {
        code: i32,
        message: String,
        action: ViolationAction,
    },
    HistoricalPacing {
        code: i32,
        message: String,
        action: ViolationAction,
    },
    TickByTickLimit {
        code: i32,
        message: String,
        action: ViolationAction,
    },
    DuplicateOrderId {
        code: i32,
        message: String,
        order_id: OrderId,
    },
    UnknownContract {
        code: i32,
        message: String,
        req_id: ReqId,
    },
    MarketDataNotSubscribed {
        code: i32,
        message: String,
        req_id: ReqId,
    },
}

// ---------------------------------------------------------------------------
// EngineCmd — the central input surface of the engine actor.
// ---------------------------------------------------------------------------

/// Every command the engine actor can receive. Wave 2 stages must not add
/// variants here without amending Stage 01; use a stage-local `EngineCmdExt`
/// during development and fold at wave end.
#[derive(Debug)]
pub enum EngineCmd {
    // ---- from sessions (client → sim over TWS wire) ----
    StartApi {
        session: SessionId,
        client_id: i32,
    },
    PlaceOrder {
        session: SessionId,
        req: PlaceOrderReq,
    },
    CancelOrder {
        session: SessionId,
        order_id: OrderId,
    },
    SubscribeMarketData {
        session: SessionId,
        req_id: ReqId,
        contract: ContractSpec,
        mode: SubMode,
    },
    UnsubscribeMarketData {
        session: SessionId,
        req_id: ReqId,
    },
    ReqContractData {
        session: SessionId,
        req_id: ReqId,
        contract: ContractSpec,
    },
    ReqHistoricalData {
        session: SessionId,
        req_id: ReqId,
        req: HistoricalReq,
    },
    ReqRealTimeBars {
        session: SessionId,
        req_id: ReqId,
        req: RealTimeBarsReq,
    },
    ReqPositions {
        session: SessionId,
    },
    ReqAccountSummary {
        session: SessionId,
        req_id: ReqId,
        group: String,
        tags: String,
    },
    ReqAccountData {
        session: SessionId,
        subscribe: bool,
        acct_code: String,
    },
    ReqExecutions {
        session: SessionId,
        req_id: ReqId,
        filter: ExecutionFilter,
    },
    ReqGlobalCancel {
        session: SessionId,
    },
    ReqCurrentTime {
        session: SessionId,
    },
    ReqIds {
        session: SessionId,
        num_ids: i32,
    },
    ReqMarketDataType {
        session: SessionId,
        data_type: MarketDataType,
    },

    // ---- from control plane (fault injection + scenarios) ----
    InjectDisconnect {
        session: SessionId,
        reason: String,
    },
    InjectLag {
        session: SessionId,
        duration: Duration,
    },
    InjectPacingViolation {
        session: SessionId,
    },
    InjectFarmOutage {
        code: i32,
        farms: Vec<String>,
    },
    InjectFarmRestore {
        code: i32,
        farms: Vec<String>,
    },
    InjectPriceJump {
        symbol: SymbolKey,
        magnitude_pct: f64,
    },
    InjectGap {
        symbol: SymbolKey,
        from: f64,
        to: f64,
    },
    InjectHalt {
        symbol: SymbolKey,
        duration: Duration,
    },
    InjectBurst {
        symbols: Vec<SymbolKey>,
        multiplier: f64,
        duration: Duration,
    },
    InjectDailyRestart,
    LoadScenario(Scenario),
    DumpState {
        reply: oneshot::Sender<EngineSnapshot>,
    },

    // ---- from scheduler ----
    Tick(VirtualInstant),
}

// ---------------------------------------------------------------------------
// EngineEvent — observability / read-only projection of what the engine did.
// ---------------------------------------------------------------------------

/// Events the engine publishes on its `broadcast` channel for observability
/// (control plane, metrics, scenario assertions). Never routed to clients.
#[derive(Clone, Debug, PartialEq)]
pub enum EngineEvent {
    Connected {
        session: SessionId,
        client_id: i32,
    },
    Disconnected {
        session: SessionId,
        reason: String,
    },
    HandshakeCompleted {
        session: SessionId,
        version: i32,
    },
    MarketDataSubscribed {
        session: SessionId,
        req_id: ReqId,
        symbol: SymbolKey,
    },
    MarketDataUnsubscribed {
        session: SessionId,
        req_id: ReqId,
    },
    OrderPlaced {
        session: SessionId,
        order_id: OrderId,
    },
    OrderCancelled {
        session: SessionId,
        order_id: OrderId,
    },
    FillObserved {
        order_id: OrderId,
        price: f64,
        shares: f64,
    },
    QuirkTriggered {
        session: SessionId,
        violation: QuirkViolation,
    },
    ScenarioStepExecuted {
        scenario: String,
        step: u32,
    },
    FarmStatusChanged {
        code: i32,
        farms: Vec<String>,
    },
    SchedulerDrained,
}

// ---------------------------------------------------------------------------
// EngineSnapshot — the `/control/dump` payload.
// ---------------------------------------------------------------------------

/// Read-only snapshot of the engine's visible state. Rendered as JSON by the
/// control plane for debugging. Additive-only — fields are `Option` /
/// collection-typed so the JSON shape grows without breaking consumers.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct EngineSnapshot {
    pub now: Option<VirtualInstant>,
    pub sessions: Vec<SessionSummary>,
    pub open_orders: Vec<OpenOrderSummary>,
    pub active_subscriptions: Vec<SubscriptionSummary>,
    pub scheduler_queue_depth: usize,
    pub quirks: QuirkCounters,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SessionSummary {
    pub session: SessionId,
    pub client_id: i32,
    pub peer: String,
    pub connected_at: Option<VirtualInstant>,
    pub msgs_in: u64,
    pub msgs_out: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct OpenOrderSummary {
    pub order_id: OrderId,
    pub symbol: Option<SymbolKey>,
    pub side: Option<Side>,
    pub kind: Option<OrderKind>,
    pub status: Option<OrderStatusCode>,
    pub remaining: f64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SubscriptionSummary {
    pub session: SessionId,
    pub req_id: ReqId,
    pub symbol: Option<SymbolKey>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct QuirkCounters {
    pub rate_limit_triggers: u64,
    pub line_limit_triggers: u64,
    pub historical_pacing_triggers: u64,
    pub tick_by_tick_triggers: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_snapshot_default_round_trip() {
        // JSON-round-trip a default snapshot so schema changes stay visible.
        let snap = EngineSnapshot::default();
        let s = serde_json::to_string(&snap).expect("serialize default snapshot");
        let back: EngineSnapshot = serde_json::from_str(&s).expect("deserialize default snapshot");
        assert_eq!(back.scheduler_queue_depth, 0);
    }

    #[test]
    fn market_emission_variants_compile() {
        // Smoke: every MarketEmission variant can be constructed. Catches
        // accidental signature drift during Wave 2 edits.
        let key = SubKey {
            session: SessionId(1),
            req_id: ReqId(1),
            symbol: SymbolKey {
                contract_id: 1,
                symbol: "AAPL".into(),
            },
        };
        let _p = MarketEmission::TickPrice {
            key: key.clone(),
            tick: TickType::Last,
            price: 100.0,
            size: Some(1),
            attribs: TickAttribs::default(),
        };
        let _s = MarketEmission::TickSize {
            key: key.clone(),
            tick: TickType::LastSize,
            size: 1,
        };
        let _str = MarketEmission::TickString {
            key: key.clone(),
            tick: TickType::LastTimestamp,
            value: "1".into(),
        };
        let _g = MarketEmission::TickGeneric {
            key: key.clone(),
            tick: TickType::Halted,
            value: 0.0,
        };
    }
}
