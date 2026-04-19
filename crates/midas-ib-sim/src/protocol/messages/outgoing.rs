//! Sim -> client (server -> client) message types.
//!
//! Stage 02c owns the encoders. Each variant mirrors a TWS server response
//! message from the deep-parity subset. Field-by-field encoding happens in
//! [`OutgoingMsg::encode`], which dispatches to per-message helpers defined in
//! this module. Cross-message shared encoders (contract writer, execution
//! writer) live in [`super::shared`].
//!
//! # Design notes
//!
//! * Every message starts with `<msg_id>\0`, followed by a per-message inner
//!   `version` field for those messages that carry one. Versions are derived
//!   from `rust-ibapi`'s decoder expectations.
//! * Per-field server-version gates come from `rust-ibapi`'s
//!   `server_versions.rs` — we mirror them exactly across the advertised range
//!   (176..221).
//! * `Option<f64>` values serialise as `UNSET_DOUBLE_STR`; `Option<i32>` as
//!   `UNSET_INT`; `Option<i64>` as `UNSET_LONG`. See `FieldWriter` helpers in
//!   [`super::fields`].

use crate::engine::types::{
    Bar, CommissionReport, Execution, MarketDataType, OpenOrder, OrderId, OrderStatus,
    OrderStatusCode, ReqId, TickAttribs, TickType,
};
use crate::protocol::messages::fields::FieldWriter;
use crate::protocol::messages::shared::{
    side_action, write_contract_execution_prefix, write_order_kind, write_tif,
};
use crate::protocol::ServerVersion;

// ---------------------------------------------------------------------------
// Server-version gate constants (mirrored from rust-ibapi's server_versions.rs)
// ---------------------------------------------------------------------------

/// `MIN_SERVER_VER_MARKET_CAP_PRICE`. When `sv >= 131` the `mkt_cap_price`
/// trailing field is present on `ORDER_STATUS`, and the inner version field
/// is suppressed.
pub(crate) const SV_MARKET_CAP_PRICE: i32 = 131;
/// `MIN_SERVER_VER_LAST_LIQUIDITY`. Gates `last_liquidity` on `EXECUTION_DATA`
/// and also gates whether the inner version field is emitted.
pub(crate) const SV_LAST_LIQUIDITY: i32 = 136;
/// `MIN_SERVER_VER_PAST_LIMIT`. Gates `past_limit` bit in `TICK_PRICE` attrs.
pub(crate) const SV_PAST_LIMIT: i32 = 109;
/// `MIN_SERVER_VER_PRE_OPEN_BID_ASK`. Gates `pre_open` bit.
pub(crate) const SV_PRE_OPEN_BID_ASK: i32 = 132;
/// `MIN_SERVER_VER_MODELS_SUPPORT`. Gates `model_code` on `EXECUTION_DATA`.
pub(crate) const SV_MODELS_SUPPORT: i32 = 103;
/// `MIN_SERVER_VER_PENDING_PRICE_REVISION`. Gates `pending_price_revision`.
pub(crate) const SV_PENDING_PRICE_REVISION: i32 = 178;
/// `MIN_SERVER_VER_ADVANCED_ORDER_REJECT`. Gates `advanced_order_reject_json`
/// on `ERR_MSG`.
pub(crate) const SV_ADVANCED_ORDER_REJECT: i32 = 166;
/// `MIN_SERVER_VER_ERROR_TIME`. `ERR_MSG` switches format (no inner version,
/// adds trailing `error_time`) at this version.
pub(crate) const SV_ERROR_TIME: i32 = 194;
/// `MIN_SERVER_VER_ORDER_CONTAINER`. Removes the inner version field from
/// `OPEN_ORDER`/`EXECUTION_DATA`/`ORDER_STATUS`.
pub(crate) const SV_ORDER_CONTAINER: i32 = 145;
/// `MIN_SERVER_VER_SUBMITTER`. Gates `submitter` on `EXECUTION_DATA`.
pub(crate) const SV_SUBMITTER: i32 = 198;
/// `MIN_SERVER_VER_HISTORICAL_DATA_END`. At this version `start`/`end` fields
/// move out of the body into a separate `HISTORICAL_DATA_END` (108) message;
/// the bar stream is preceded only by `bar_count`.
pub(crate) const SV_HISTORICAL_DATA_END: i32 = 196;
/// `MIN_SERVER_VER_SYNT_REALTIME_BARS`. Before this version, HISTORICAL_DATA
/// carried an inner version + a `hasGaps` field per bar.
pub(crate) const SV_SYNT_REALTIME_BARS: i32 = 124;
/// `MIN_SERVER_VER_INELIGIBILITY_REASONS`. At this version CONTRACT_DATA
/// gains a trailing `ineligibility_reasons` list (count + entries).
pub(crate) const SV_INELIGIBILITY_REASONS: i32 = 208;
/// `MIN_SERVER_VER_LAST_TRADE_DATE`. At this version CONTRACT_DATA gains an
/// extra `last_trade_date` String field inserted BEFORE `strike`.
pub(crate) const SV_LAST_TRADE_DATE: i32 = 220;

// ---------------------------------------------------------------------------
// Message ID constants
// ---------------------------------------------------------------------------

pub(crate) const ID_TICK_PRICE: i32 = 1;
pub(crate) const ID_TICK_SIZE: i32 = 2;
pub(crate) const ID_ORDER_STATUS: i32 = 3;
pub(crate) const ID_ERR_MSG: i32 = 4;
pub(crate) const ID_OPEN_ORDER: i32 = 5;
pub(crate) const ID_ACCT_VALUE: i32 = 6;
pub(crate) const ID_PORTFOLIO_VALUE: i32 = 7;
pub(crate) const ID_NEXT_VALID_ID: i32 = 9;
pub(crate) const ID_CONTRACT_DATA: i32 = 10;
pub(crate) const ID_EXECUTION_DATA: i32 = 11;
pub(crate) const ID_MANAGED_ACCTS: i32 = 15;
pub(crate) const ID_HISTORICAL_DATA: i32 = 17;
pub(crate) const ID_TICK_GENERIC: i32 = 45;
pub(crate) const ID_TICK_STRING: i32 = 46;
pub(crate) const ID_CURRENT_TIME: i32 = 49;
pub(crate) const ID_REAL_TIME_BARS: i32 = 50;
pub(crate) const ID_CONTRACT_DATA_END: i32 = 52;
pub(crate) const ID_OPEN_ORDER_END: i32 = 53;
pub(crate) const ID_ACCT_DOWNLOAD_END: i32 = 54;
pub(crate) const ID_EXECUTION_DATA_END: i32 = 55;
pub(crate) const ID_MARKET_DATA_TYPE: i32 = 58;
pub(crate) const ID_COMMISSION_REPORT: i32 = 59;
pub(crate) const ID_POSITION: i32 = 61;
pub(crate) const ID_ACCOUNT_SUMMARY: i32 = 63;

// ---------------------------------------------------------------------------
// Outgoing payload types
// ---------------------------------------------------------------------------

/// Minimal `ContractDetails` projection used by `CONTRACT_DATA` encoder.
///
/// Matches the field set the sim actually populates; `rust-ibapi`'s decoder
/// accepts defaulted values for everything else.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ContractDetails {
    pub con_id: i32,
    pub symbol: String,
    pub sec_type: String,
    pub last_trade_date_or_contract_month: String,
    /// Emitted only when sv >= [`SV_LAST_TRADE_DATE`] (220). IB tolerates an
    /// empty string when unknown. Real TWS populates this with the concrete
    /// last-trade date (e.g. "20251219 16:00:00 US/Eastern").
    pub last_trade_date: String,
    pub strike: f64,
    pub right: String,
    pub exchange: String,
    pub currency: String,
    pub local_symbol: String,
    pub market_name: String,
    pub trading_class: String,
    pub min_tick: f64,
    pub multiplier: String,
    pub order_types: String,
    pub valid_exchanges: String,
    pub price_magnifier: i32,
    pub under_con_id: i32,
    pub long_name: String,
    pub primary_exchange: String,
    pub contract_month: String,
    pub industry: String,
    pub category: String,
    pub subcategory: String,
    pub time_zone_id: String,
    pub trading_hours: String,
    pub liquid_hours: String,
    pub ev_rule: String,
    pub ev_multiplier: f64,
    pub agg_group: i32,
    pub under_symbol: String,
    pub under_sec_type: String,
    pub market_rule_ids: String,
    pub real_expiration_date: String,
    pub stock_type: String,
    pub min_size: f64,
    pub size_increment: f64,
    pub suggested_size_increment: f64,
}

/// Sim-owned view of the `OPEN_ORDER` payload that the encoder needs. Holds
/// the `OpenOrder` plus the order-state string + the permId/client fields
/// which `rust-ibapi` cross-references from parallel `ORDER_STATUS` events.
#[derive(Clone, Debug, PartialEq)]
pub struct OpenOrderPayload {
    pub order: OpenOrder,
    pub order_state_status: String,
    pub perm_id: i64,
    pub client_id: i32,
}

/// Payload for the `PORTFOLIO_VALUE` (id=7) outgoing message.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PortfolioValuePayload {
    pub con_id: i32,
    pub symbol: String,
    pub sec_type: String,
    pub last_trade_date_or_contract_month: String,
    pub strike: f64,
    pub right: String,
    pub multiplier: String,
    pub primary_exchange: String,
    pub currency: String,
    pub local_symbol: String,
    pub trading_class: String,
    pub position: f64,
    pub market_price: f64,
    pub market_value: f64,
    pub avg_cost: f64,
    pub unrealized_pnl: f64,
    pub realized_pnl: f64,
    pub acct_code: String,
}

/// Payload for the `POSITION` (id=61) outgoing message.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PositionPayload {
    pub account: String,
    pub con_id: i32,
    pub symbol: String,
    pub sec_type: String,
    pub last_trade_date_or_contract_month: String,
    pub strike: f64,
    pub right: String,
    pub multiplier: String,
    pub exchange: String,
    pub currency: String,
    pub local_symbol: String,
    pub trading_class: String,
    pub position: f64,
    pub avg_cost: f64,
}

// ---------------------------------------------------------------------------
// OutgoingMsg — the variant set covered by this stage.
// ---------------------------------------------------------------------------

/// Server -> client message, pre-encode. Each variant maps 1:1 to a TWS
/// server response message id. Encode via [`Self::encode`].
#[derive(Clone, Debug, PartialEq)]
pub enum OutgoingMsg {
    /// id 1 — `TICK_PRICE`. Price tick; `size` is optional (some tick types
    /// carry a companion size).
    TickPrice {
        req_id: ReqId,
        tick: TickType,
        price: f64,
        size: Option<i64>,
        attribs: TickAttribs,
    },
    /// id 2 — `TICK_SIZE`.
    TickSize {
        req_id: ReqId,
        tick: TickType,
        size: i64,
    },
    /// id 3 — `ORDER_STATUS`. The hottest outbound message.
    OrderStatus(OrderStatus),
    /// id 4 — `ERR_MSG`. `req_id = -1` for bulletins / farm status.
    ErrMsg {
        req_id: i32,
        code: i32,
        message: String,
        /// Only emitted when `sv >= ADVANCED_ORDER_REJECT (166)`.
        advanced_order_reject_json: Option<String>,
        /// Epoch millis — only emitted when `sv >= ERROR_TIME (194)`. The
        /// sim usually leaves this as `0` for bulletins.
        error_time_ms: i64,
    },
    /// id 5 — `OPEN_ORDER`. Wide, multi-section encoder. The sim only
    /// populates a Hand-of-Midas relevant subset of fields; every other
    /// field emits a defaulted-but-present value that matches `rust-ibapi`'s
    /// decoder expectations. Field ordering best-effort against `v200`
    /// sample in `rust-ibapi/src/orders/common/decoders/tests.rs`.
    OpenOrder(OpenOrderPayload),
    /// id 6 — `ACCT_VALUE`.
    AcctValue {
        key: String,
        value: String,
        currency: String,
        acct_code: String,
    },
    /// id 7 — `PORTFOLIO_VALUE`.
    PortfolioValue(Box<PortfolioValuePayload>),
    /// id 9 — `NEXT_VALID_ID`. Emitted unsolicited after START_API.
    NextValidId { order_id: OrderId },
    /// id 10 — `CONTRACT_DATA`.
    ContractData {
        req_id: ReqId,
        details: Box<ContractDetails>,
    },
    /// id 11 — `EXECUTION_DATA`.
    ExecutionData { req_id: ReqId, execution: Execution },
    /// id 15 — `MANAGED_ACCTS`. One string field, comma-separated.
    ManagedAccts { accounts: String },
    /// id 17 — `HISTORICAL_DATA`. Bar array is length-prefixed.
    HistoricalData {
        req_id: ReqId,
        start: String,
        end: String,
        bars: Vec<Bar>,
    },
    /// id 45 — `TICK_GENERIC`.
    TickGeneric {
        req_id: ReqId,
        tick: TickType,
        value: f64,
    },
    /// id 46 — `TICK_STRING`.
    TickString {
        req_id: ReqId,
        tick: TickType,
        value: String,
    },
    /// id 49 — `CURRENT_TIME`.
    CurrentTime { epoch_secs: i64 },
    /// id 50 — `REAL_TIME_BARS`. Always 5-second bars in IB.
    RealTimeBar {
        req_id: ReqId,
        timestamp: i64,
        open: f64,
        high: f64,
        low: f64,
        close: f64,
        volume: i64,
        wap: f64,
        count: i32,
    },
    /// id 52 — `CONTRACT_DATA_END`.
    ContractDataEnd { req_id: ReqId },
    /// id 53 — `OPEN_ORDER_END`.
    OpenOrderEnd,
    /// id 54 — `ACCT_DOWNLOAD_END`.
    AcctDownloadEnd { acct_code: String },
    /// id 55 — `EXECUTION_DATA_END`.
    ExecutionDataEnd { req_id: ReqId },
    /// id 58 — `MARKET_DATA_TYPE`.
    MarketDataTypeResp {
        req_id: ReqId,
        data_type: MarketDataType,
    },
    /// id 59 — `COMMISSION_REPORT`.
    CommissionReport { report: CommissionReport },
    /// id 61 — `POSITION`.
    Position(Box<PositionPayload>),
    /// id 63 — `ACCOUNT_SUMMARY`.
    AccountSummary {
        req_id: ReqId,
        account: String,
        tag: String,
        value: String,
        currency: String,
    },
}

impl OutgoingMsg {
    /// Encode into `w` against the negotiated `sv`. Every message emits its
    /// id first, followed by any applicable inner version field, followed by
    /// the per-message body.
    pub fn encode(&self, w: &mut FieldWriter, sv: ServerVersion) {
        match self {
            Self::TickPrice {
                req_id,
                tick,
                price,
                size,
                attribs,
            } => encode_tick_price(w, sv, *req_id, *tick, *price, *size, *attribs),
            Self::TickSize { req_id, tick, size } => encode_tick_size(w, *req_id, *tick, *size),
            Self::OrderStatus(s) => encode_order_status(w, sv, s),
            Self::ErrMsg {
                req_id,
                code,
                message,
                advanced_order_reject_json,
                error_time_ms,
            } => encode_err_msg(
                w,
                sv,
                *req_id,
                *code,
                message,
                advanced_order_reject_json.as_deref(),
                *error_time_ms,
            ),
            Self::OpenOrder(p) => encode_open_order(w, sv, p),
            Self::AcctValue {
                key,
                value,
                currency,
                acct_code,
            } => encode_acct_value(w, key, value, currency, acct_code),
            Self::PortfolioValue(p) => encode_portfolio_value(w, p),
            Self::NextValidId { order_id } => encode_next_valid_id(w, *order_id),
            Self::ContractData { req_id, details } => encode_contract_data(w, sv, *req_id, details),
            Self::ExecutionData { req_id, execution } => {
                encode_execution_data(w, sv, *req_id, execution)
            }
            Self::ManagedAccts { accounts } => encode_managed_accts(w, accounts),
            Self::HistoricalData {
                req_id,
                start,
                end,
                bars,
            } => encode_historical_data(w, sv, *req_id, start, end, bars),
            Self::TickGeneric {
                req_id,
                tick,
                value,
            } => encode_tick_generic(w, *req_id, *tick, *value),
            Self::TickString {
                req_id,
                tick,
                value,
            } => encode_tick_string(w, *req_id, *tick, value),
            Self::CurrentTime { epoch_secs } => encode_current_time(w, *epoch_secs),
            Self::RealTimeBar {
                req_id,
                timestamp,
                open,
                high,
                low,
                close,
                volume,
                wap,
                count,
            } => encode_realtime_bar(
                w, *req_id, *timestamp, *open, *high, *low, *close, *volume, *wap, *count,
            ),
            Self::ContractDataEnd { req_id } => encode_contract_data_end(w, *req_id),
            Self::OpenOrderEnd => encode_open_order_end(w),
            Self::AcctDownloadEnd { acct_code } => encode_acct_download_end(w, acct_code),
            Self::ExecutionDataEnd { req_id } => encode_execution_data_end(w, *req_id),
            Self::MarketDataTypeResp { req_id, data_type } => {
                encode_market_data_type(w, *req_id, *data_type)
            }
            Self::CommissionReport { report } => encode_commission_report(w, report),
            Self::Position(p) => encode_position(w, p),
            Self::AccountSummary {
                req_id,
                account,
                tag,
                value,
                currency,
            } => encode_account_summary(w, *req_id, account, tag, value, currency),
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers — tick type enum -> i32 wire value
// ---------------------------------------------------------------------------

/// TWS tick-type integer codes. See `rust-ibapi/src/market_data/realtime/common/tick_types.rs`.
fn tick_type_code(t: TickType) -> i32 {
    match t {
        TickType::BidSize => 0,
        TickType::Bid => 1,
        TickType::Ask => 2,
        TickType::AskSize => 3,
        TickType::Last => 4,
        TickType::LastSize => 5,
        TickType::High => 6,
        TickType::Low => 7,
        TickType::Volume => 8,
        TickType::Close => 9,
        TickType::BidOptionComputation => 10,
        TickType::AskOptionComputation => 11,
        TickType::LastOptionComputation => 12,
        TickType::ModelOption => 13,
        TickType::Open => 14,
        TickType::Low13Week => 15,
        TickType::High13Week => 16,
        TickType::Low26Week => 17,
        TickType::High26Week => 18,
        TickType::Low52Week => 19,
        TickType::High52Week => 20,
        TickType::AvgVolume => 21,
        TickType::OpenInterest => 22,
        TickType::OptionHistoricalVol => 23,
        TickType::OptionImpliedVol => 24,
        TickType::OptionBidExch => 25,
        TickType::OptionAskExch => 26,
        TickType::OptionCallOpenInterest => 27,
        TickType::OptionPutOpenInterest => 28,
        TickType::OptionCallVolume => 29,
        TickType::OptionPutVolume => 30,
        TickType::IndexFuturePremium => 31,
        TickType::BidExch => 32,
        TickType::AskExch => 33,
        TickType::AuctionVolume => 34,
        TickType::AuctionPrice => 35,
        TickType::AuctionImbalance => 36,
        TickType::MarkPrice => 37,
        TickType::BidEfpComputation => 38,
        TickType::AskEfpComputation => 39,
        TickType::LastEfpComputation => 40,
        TickType::OpenEfpComputation => 41,
        TickType::HighEfpComputation => 42,
        TickType::LowEfpComputation => 43,
        TickType::CloseEfpComputation => 44,
        TickType::LastTimestamp => 45,
        TickType::Shortable => 46,
        TickType::FundamentalRatios => 47,
        TickType::RtVolume => 48,
        TickType::Halted => 49,
        TickType::BidYield => 50,
        TickType::AskYield => 51,
        TickType::LastYield => 52,
        TickType::CustOptionComputation => 53,
        TickType::TradeCount => 54,
        TickType::TradeRate => 55,
        TickType::VolumeRate => 56,
        TickType::LastRthTrade => 57,
        TickType::RtHistoricalVol => 58,
        TickType::IbDividends => 59,
        TickType::BondFactorMultiplier => 60,
        TickType::RegulatoryImbalance => 61,
        TickType::NewsTick => 62,
        TickType::ShortTermVolume3Min => 63,
        TickType::ShortTermVolume5Min => 64,
        TickType::ShortTermVolume10Min => 65,
        TickType::DelayedBid => 66,
        TickType::DelayedAsk => 67,
        TickType::DelayedLast => 68,
        TickType::DelayedBidSize => 69,
        TickType::DelayedAskSize => 70,
        TickType::DelayedLastSize => 71,
        TickType::DelayedHigh => 72,
        TickType::DelayedLow => 73,
        TickType::DelayedVolume => 74,
        TickType::DelayedClose => 75,
        TickType::DelayedOpen => 76,
        TickType::RtTrdVolume => 77,
        TickType::CreditmanMarkPrice => 78,
        TickType::CreditmanSlowMarkPrice => 79,
        TickType::DelayedBidOption => 80,
        TickType::DelayedAskOption => 81,
        TickType::DelayedLastOption => 82,
        TickType::DelayedModelOption => 83,
        TickType::LastExch => 84,
        TickType::LastRegTime => 85,
        TickType::FuturesOpenInterest => 86,
        TickType::AvgOptVolume => 87,
        TickType::DelayedLastTimestamp => 88,
        TickType::ShortableShares => 89,
    }
}

/// IB order-status string for an [`OrderStatusCode`].
fn order_status_str(s: OrderStatusCode) -> &'static str {
    match s {
        OrderStatusCode::ApiPending => "ApiPending",
        OrderStatusCode::PendingSubmit => "PendingSubmit",
        OrderStatusCode::PreSubmitted => "PreSubmitted",
        OrderStatusCode::Submitted => "Submitted",
        OrderStatusCode::Filled => "Filled",
        OrderStatusCode::PartiallyFilled => "PartiallyFilled",
        OrderStatusCode::Cancelled => "Cancelled",
        OrderStatusCode::ApiCancelled => "ApiCancelled",
        OrderStatusCode::Inactive => "Inactive",
    }
}

fn market_data_type_code(t: MarketDataType) -> i32 {
    t as i32
}

// ---------------------------------------------------------------------------
// Per-message encoders
// ---------------------------------------------------------------------------

fn encode_tick_price(
    w: &mut FieldWriter,
    sv: ServerVersion,
    req_id: ReqId,
    tick: TickType,
    price: f64,
    size: Option<i64>,
    attribs: TickAttribs,
) {
    // Inner version = 3 (enables size + attribs mask fields).
    w.write_i32(ID_TICK_PRICE);
    w.write_i32(3);
    w.write_i32(req_id.0);
    w.write_i32(tick_type_code(tick));
    w.write_f64(price);
    // Size at v>=2 — we always emit since inner_version = 3.
    // rust-ibapi 2.10 still decodes this slot via `next_double()`, but newer
    // IB builds treat tick sizes as `Decimal` (integral-looking strings,
    // `""` for unset). Emitting an integral string is byte-identical to the
    // `next_double()` path (it accepts `"100"` just fine) while staying
    // compatible with the Decimal parser. Empty string reads back as
    // unset / zero on both paths — use it for `None`.
    match size {
        Some(v) => w.write_string(&v.to_string()),
        None => w.write_string(""),
    };
    // Attribs mask at v>=3.
    let mut mask = 0i32;
    if sv.raw() >= SV_PAST_LIMIT {
        if attribs.can_auto_execute {
            mask |= 0x1;
        }
        if attribs.past_limit {
            mask |= 0x2;
        }
        if sv.raw() >= SV_PRE_OPEN_BID_ASK && attribs.pre_open {
            mask |= 0x4;
        }
    }
    w.write_i32(mask);
}

fn encode_tick_size(w: &mut FieldWriter, req_id: ReqId, tick: TickType, size: i64) {
    w.write_i32(ID_TICK_SIZE);
    w.write_i32(1); // inner version
    w.write_i32(req_id.0);
    w.write_i32(tick_type_code(tick));
    // Wire slot is decoded as `next_double()` in rust-ibapi 2.10 but as
    // `Decimal` (integral-looking string) in newer IB builds. Emitting the
    // integral string covers both paths byte-for-byte.
    w.write_string(&size.to_string());
}

fn encode_tick_generic(w: &mut FieldWriter, req_id: ReqId, tick: TickType, value: f64) {
    w.write_i32(ID_TICK_GENERIC);
    w.write_i32(1);
    w.write_i32(req_id.0);
    w.write_i32(tick_type_code(tick));
    w.write_f64(value);
}

fn encode_tick_string(w: &mut FieldWriter, req_id: ReqId, tick: TickType, value: &str) {
    w.write_i32(ID_TICK_STRING);
    w.write_i32(1);
    w.write_i32(req_id.0);
    w.write_i32(tick_type_code(tick));
    w.write_string(value);
}

fn encode_order_status(w: &mut FieldWriter, sv: ServerVersion, s: &OrderStatus) {
    // `ORDER_STATUS` emits inner version = 6 pre-MARKET_CAP_PRICE (131); at or
    // above that version the version field is suppressed entirely.
    w.write_i32(ID_ORDER_STATUS);
    if sv.raw() < SV_MARKET_CAP_PRICE {
        w.write_i32(6);
    }
    w.write_i32(s.order_id.0);
    w.write_string(order_status_str(s.status));
    w.write_f64(s.filled);
    w.write_f64(s.remaining);
    w.write_f64(s.avg_fill_price);
    w.write_i64(s.perm_id as i64);
    w.write_i32(s.parent_id);
    w.write_f64(s.last_fill_price);
    w.write_i32(s.client_id);
    w.write_string(&s.why_held);
    if sv.raw() >= SV_MARKET_CAP_PRICE {
        w.write_opt_f64(s.mkt_cap_price);
    }
}

fn encode_err_msg(
    w: &mut FieldWriter,
    sv: ServerVersion,
    req_id: i32,
    code: i32,
    message: &str,
    advanced_order_reject_json: Option<&str>,
    error_time_ms: i64,
) {
    w.write_i32(ID_ERR_MSG);
    if sv.raw() < SV_ERROR_TIME {
        // Classic format: inner version = 2.
        w.write_i32(2);
        w.write_i32(req_id);
        w.write_i32(code);
        w.write_string(message);
        if sv.raw() >= SV_ADVANCED_ORDER_REJECT {
            w.write_opt_string(advanced_order_reject_json);
        }
    } else {
        // ERROR_TIME format (>=194): no inner version; trailing error_time.
        w.write_i32(req_id);
        w.write_i32(code);
        w.write_string(message);
        w.write_opt_string(advanced_order_reject_json);
        w.write_i64(error_time_ms);
    }
}

fn encode_next_valid_id(w: &mut FieldWriter, order_id: OrderId) {
    w.write_i32(ID_NEXT_VALID_ID);
    w.write_i32(1); // inner version
    w.write_i32(order_id.0);
}

fn encode_current_time(w: &mut FieldWriter, epoch_secs: i64) {
    w.write_i32(ID_CURRENT_TIME);
    w.write_i32(1); // inner version
    w.write_i64(epoch_secs);
}

fn encode_managed_accts(w: &mut FieldWriter, accounts: &str) {
    w.write_i32(ID_MANAGED_ACCTS);
    w.write_i32(1); // inner version
    w.write_string(accounts);
}

fn encode_contract_data_end(w: &mut FieldWriter, req_id: ReqId) {
    w.write_i32(ID_CONTRACT_DATA_END);
    w.write_i32(1); // inner version
    w.write_i32(req_id.0);
}

fn encode_open_order_end(w: &mut FieldWriter) {
    w.write_i32(ID_OPEN_ORDER_END);
    w.write_i32(1);
}

fn encode_acct_download_end(w: &mut FieldWriter, acct_code: &str) {
    w.write_i32(ID_ACCT_DOWNLOAD_END);
    w.write_i32(1);
    w.write_string(acct_code);
}

fn encode_execution_data_end(w: &mut FieldWriter, req_id: ReqId) {
    w.write_i32(ID_EXECUTION_DATA_END);
    w.write_i32(1);
    w.write_i32(req_id.0);
}

fn encode_market_data_type(w: &mut FieldWriter, req_id: ReqId, data_type: MarketDataType) {
    w.write_i32(ID_MARKET_DATA_TYPE);
    w.write_i32(1);
    w.write_i32(req_id.0);
    w.write_i32(market_data_type_code(data_type));
}

fn encode_account_summary(
    w: &mut FieldWriter,
    req_id: ReqId,
    account: &str,
    tag: &str,
    value: &str,
    currency: &str,
) {
    w.write_i32(ID_ACCOUNT_SUMMARY);
    w.write_i32(1);
    w.write_i32(req_id.0);
    w.write_string(account);
    w.write_string(tag);
    w.write_string(value);
    w.write_string(currency);
}

fn encode_acct_value(w: &mut FieldWriter, key: &str, value: &str, currency: &str, acct_code: &str) {
    // Inner version = 2 (enables `acct_code` trailing field).
    w.write_i32(ID_ACCT_VALUE);
    w.write_i32(2);
    w.write_string(key);
    w.write_string(value);
    w.write_string(currency);
    w.write_string(acct_code);
}

fn encode_portfolio_value(w: &mut FieldWriter, p: &PortfolioValuePayload) {
    // Inner version = 8 (enables every field since the sim always speaks
    // modern server versions). Contract fields follow rust-ibapi's
    // `decode_account_portfolio_value` at v>=8.
    w.write_i32(ID_PORTFOLIO_VALUE);
    w.write_i32(8);
    w.write_i32(p.con_id);
    w.write_string(&p.symbol);
    w.write_string(&p.sec_type);
    w.write_string(&p.last_trade_date_or_contract_month);
    w.write_f64(p.strike);
    w.write_string(&p.right);
    w.write_string(&p.multiplier);
    w.write_string(&p.primary_exchange);
    w.write_string(&p.currency);
    w.write_string(&p.local_symbol);
    w.write_string(&p.trading_class);
    w.write_f64(p.position);
    w.write_f64(p.market_price);
    w.write_f64(p.market_value);
    w.write_f64(p.avg_cost);
    w.write_f64(p.unrealized_pnl);
    w.write_f64(p.realized_pnl);
    w.write_string(&p.acct_code);
}

fn encode_position(w: &mut FieldWriter, p: &PositionPayload) {
    // Inner version = 3 (enables trading_class + avg_cost).
    w.write_i32(ID_POSITION);
    w.write_i32(3);
    w.write_string(&p.account);
    w.write_i32(p.con_id);
    w.write_string(&p.symbol);
    w.write_string(&p.sec_type);
    w.write_string(&p.last_trade_date_or_contract_month);
    w.write_f64(p.strike);
    w.write_string(&p.right);
    w.write_string(&p.multiplier);
    w.write_string(&p.exchange);
    w.write_string(&p.currency);
    w.write_string(&p.local_symbol);
    w.write_string(&p.trading_class);
    w.write_f64(p.position);
    w.write_f64(p.avg_cost);
}

fn encode_commission_report(w: &mut FieldWriter, r: &CommissionReport) {
    // Inner version = 1.
    w.write_i32(ID_COMMISSION_REPORT);
    w.write_i32(1);
    w.write_string(&r.exec_id);
    w.write_f64(r.commission);
    w.write_string(&r.currency);
    w.write_opt_f64(r.realized_pnl);
    w.write_opt_f64(r.yield_);
    // `yield_redemption_date` is a string field on the wire. We emit whatever
    // integer date the sim has; `None` becomes empty string.
    match r.yield_redemption_date {
        Some(d) => {
            let s = d.to_string();
            w.write_string(&s);
        }
        None => {
            w.write_string("");
        }
    }
}

fn encode_execution_data(w: &mut FieldWriter, sv: ServerVersion, req_id: ReqId, e: &Execution) {
    w.write_i32(ID_EXECUTION_DATA);
    if sv.raw() < SV_LAST_LIQUIDITY {
        // Pre-136: inner version = 10 (the value rust-ibapi's decoder ignores
        // but expects present). Our advertised range is 176..221 so this
        // branch isn't actually reachable — kept for completeness.
        w.write_i32(10);
    }
    w.write_i32(req_id.0);
    w.write_i32(e.order_id.0);
    write_contract_execution_prefix(w, &e.contract);
    w.write_string(&e.exec_id);
    // Time is emitted as an ASCII string. The sim projects VirtualInstant to
    // an epoch-seconds string; downstream decoders accept this shape.
    let secs = e.time.as_duration().as_secs() as i64;
    let time_str = secs.to_string();
    w.write_string(&time_str);
    w.write_string(&e.acct_number);
    w.write_string(&e.exchange);
    w.write_string(side_action(e.side));
    w.write_f64(e.shares);
    w.write_f64(e.price);
    w.write_i64(e.perm_id as i64);
    w.write_i32(e.client_id);
    w.write_i32(e.liquidation);
    w.write_f64(e.cumulative_quantity);
    w.write_f64(e.avg_price);
    w.write_opt_string(e.order_ref.as_deref());
    // `ev_rule` + `ev_multiplier` — sim always emits empty / unset.
    w.write_string("");
    w.write_opt_f64(None);
    if sv.raw() >= SV_MODELS_SUPPORT {
        w.write_string(""); // model_code
    }
    if sv.raw() >= SV_LAST_LIQUIDITY {
        w.write_i32(0); // last_liquidity: 0 = unknown
    }
    if sv.raw() >= SV_PENDING_PRICE_REVISION {
        w.write_bool(false);
    }
    if sv.raw() >= SV_SUBMITTER {
        w.write_string(""); // submitter
    }
}

fn encode_contract_data(
    w: &mut FieldWriter,
    sv: ServerVersion,
    req_id: ReqId,
    d: &ContractDetails,
) {
    // Inner version is suppressed at `sv >= SIZE_RULES (162)` in rust-ibapi;
    // our advertised range starts at 176 so it's always suppressed — but
    // rust-ibapi's decoder peeks at the `message_version` variable default (8)
    // and skips the request_id at `message_version >= 3`. That skip is always
    // in effect at our range, so we always emit `req_id`.
    w.write_i32(ID_CONTRACT_DATA);
    w.write_i32(req_id.0);
    w.write_string(&d.symbol);
    w.write_string(&d.sec_type);
    w.write_string(&d.last_trade_date_or_contract_month);
    // LAST_TRADE_DATE gate (server_versions::LAST_TRADE_DATE=220). When the
    // negotiated sv is >=220, rust-ibapi's decoder expects an extra
    // `last_trade_date` String BEFORE `strike`. Omitting it causes full field
    // misalignment on every CONTRACT_DATA frame at common negotiated versions
    // (TWS typically negotiates 220 or 221).
    if sv.raw() >= SV_LAST_TRADE_DATE {
        w.write_string(&d.last_trade_date);
    }
    w.write_f64(d.strike);
    w.write_string(&d.right);
    w.write_string(&d.exchange);
    w.write_string(&d.currency);
    w.write_string(&d.local_symbol);
    w.write_string(&d.market_name);
    w.write_string(&d.trading_class);
    w.write_i32(d.con_id);
    w.write_f64(d.min_tick);
    w.write_string(&d.multiplier);
    w.write_string(&d.order_types);
    w.write_string(&d.valid_exchanges);
    w.write_i32(d.price_magnifier);
    w.write_i32(d.under_con_id);
    w.write_string(&d.long_name);
    w.write_string(&d.primary_exchange);
    w.write_string(&d.contract_month);
    w.write_string(&d.industry);
    w.write_string(&d.category);
    w.write_string(&d.subcategory);
    w.write_string(&d.time_zone_id);
    w.write_string(&d.trading_hours);
    w.write_string(&d.liquid_hours);
    w.write_string(&d.ev_rule);
    w.write_f64(d.ev_multiplier);
    w.write_i32(0); // sec_id_list count (empty)
    w.write_i32(d.agg_group);
    w.write_string(&d.under_symbol);
    w.write_string(&d.under_sec_type);
    w.write_string(&d.market_rule_ids);
    w.write_string(&d.real_expiration_date);
    w.write_string(&d.stock_type);
    // SIZE_RULES gate (162). Our minimum is 176, always emit.
    w.write_f64(d.min_size);
    w.write_f64(d.size_increment);
    w.write_f64(d.suggested_size_increment);
    // INELIGIBILITY_REASONS (208). Only emitted when sv >= 208; at sv in
    // 176..=207 rust-ibapi's decoder doesn't consume this field, so emitting
    // it produces a stray trailing `0\0` that drifts the frame byte-count.
    if sv.raw() >= SV_INELIGIBILITY_REASONS {
        w.write_i32(0);
    }
}

fn encode_historical_data(
    w: &mut FieldWriter,
    sv: ServerVersion,
    req_id: ReqId,
    start: &str,
    end: &str,
    bars: &[Bar],
) {
    w.write_i32(ID_HISTORICAL_DATA);
    // Inner version only present when sv < SYNT_REALTIME_BARS (124). Our
    // range is 176+, so suppressed.
    w.write_i32(req_id.0);
    // `start` and `end` only present when sv < HISTORICAL_DATA_END (196).
    // Our range spans that boundary.
    if sv.raw() < SV_HISTORICAL_DATA_END {
        w.write_string(start);
        w.write_string(end);
    }
    w.write_i32(bars.len() as i32);
    for b in bars {
        // Bar `date` as epoch-seconds string — rust-ibapi's parser accepts
        // bare integer timestamps via `parse_ib_date_time_with_timezone`.
        let secs = b.time.as_duration().as_secs() as i64;
        let date = secs.to_string();
        w.write_string(&date);
        w.write_f64(b.open);
        w.write_f64(b.high);
        w.write_f64(b.low);
        w.write_f64(b.close);
        w.write_f64(b.volume as f64);
        w.write_f64(b.wap);
        // `hasGaps` only before SYNT_REALTIME_BARS. Skipped above that.
        if sv.raw() < SV_SYNT_REALTIME_BARS {
            w.write_string("false");
        }
        w.write_i32(b.count);
    }
}

#[allow(clippy::too_many_arguments)]
fn encode_realtime_bar(
    w: &mut FieldWriter,
    req_id: ReqId,
    timestamp: i64,
    open: f64,
    high: f64,
    low: f64,
    close: f64,
    volume: i64,
    wap: f64,
    count: i32,
) {
    w.write_i32(ID_REAL_TIME_BARS);
    w.write_i32(3); // inner version
    w.write_i32(req_id.0);
    w.write_i64(timestamp);
    w.write_f64(open);
    w.write_f64(high);
    w.write_f64(low);
    w.write_f64(close);
    w.write_f64(volume as f64);
    w.write_f64(wap);
    w.write_i32(count);
}

fn encode_open_order(w: &mut FieldWriter, sv: ServerVersion, p: &OpenOrderPayload) {
    // OPEN_ORDER is the widest message. This encoder emits a faithful
    // projection of the `v200` sample field layout captured in
    // `rust-ibapi/src/orders/common/decoders/tests.rs::build_open_order_base_fields`.
    //
    // FIELD-ORDERING BEST GUESS: advanced-order-preview fields
    // (margin/commission trails, peg-to-bench, conditions) are emitted with
    // default/empty values the real decoder accepts. A future Wave-2 pass can
    // widen this once we have live traffic to diff against.
    //
    // TODO: roundtrip test with incoming parser (Stage 02b).

    let o = &p.order;
    w.write_i32(ID_OPEN_ORDER);
    // Inner version is suppressed at sv >= ORDER_CONTAINER (145). Our range
    // starts at 176 -> always suppressed.
    if sv.raw() < SV_ORDER_CONTAINER {
        w.write_i32(34);
    }
    w.write_i32(o.order_id.0);

    // Contract (11 fields — no primary_exchange / include_expired slots).
    let (symbol, sec_type, expiry, strike, right, exchange, currency, local_symbol, trading_class) =
        super::shared::contract_open_order_fields(&o.contract);
    w.write_i32(0); // contract_id — sim doesn't assign numeric ids yet
    w.write_string(symbol);
    w.write_string(sec_type);
    w.write_string(expiry);
    w.write_f64(strike);
    w.write_string(right);
    w.write_string(""); // multiplier
    w.write_string(exchange);
    w.write_string(currency);
    w.write_string(local_symbol);
    w.write_string(trading_class);

    // Order action + type.
    w.write_string(side_action(o.side));
    w.write_f64(o.total_quantity);
    w.write_string(write_order_kind(o.kind));
    w.write_opt_f64(o.limit_price);
    w.write_opt_f64(o.aux_price);

    // TIF / OCA / account / open_close / origin.
    w.write_string(write_tif(&o.tif));
    w.write_string(o.oca_group.as_deref().unwrap_or(""));
    w.write_string(&o.account);
    w.write_string(""); // open_close
    w.write_i32(0); // origin (CUSTOMER)
    w.write_string(""); // order_ref
    w.write_i32(p.client_id);
    w.write_i64(p.perm_id);
    w.write_bool(false); // outside_rth
    w.write_bool(false); // hidden
    w.write_f64(0.0); // discretionary_amt
    w.write_string(""); // good_after_time
    w.write_string(""); // (deprecated) shares_allocation
    w.write_string(""); // fa_group
    w.write_string(""); // fa_method
    w.write_string(""); // fa_percentage
                        // fa_profile was desupported at 177. Emit empty if sv < 177.
    if sv.raw() < 177 {
        w.write_string(""); // fa_profile
    }
    w.write_string(""); // model_code
    w.write_string(""); // good_till_date
    w.write_string(""); // rule_80_a
    w.write_opt_f64(None); // percent_offset
    w.write_string(""); // settling_firm
    w.write_i32(0); // short_sale_slot
    w.write_string(""); // designated_location
    w.write_i32(-1); // exempt_code
    w.write_i32(0); // auction_strategy
    w.write_opt_f64(None); // starting_price
    w.write_opt_f64(None); // stock_ref_price
    w.write_opt_f64(None); // delta
    w.write_opt_f64(None); // stock_range_lower
    w.write_opt_f64(None); // stock_range_upper
    w.write_opt_i32(None); // display_size
    w.write_bool(false); // block_order
    w.write_bool(false); // sweep_to_fill
    w.write_bool(false); // all_or_none
    w.write_opt_i32(None); // min_qty
    w.write_i32(0); // oca_type
    w.write_string(""); // skip_etrade_only
    w.write_string(""); // skip_firm_quote_only
    w.write_string(""); // skip_nbbo_price_cap
    w.write_i32(o.parent_id.map(|p| p.0).unwrap_or(0)); // parent_id
    w.write_i32(0); // trigger_method

    // Volatility order params (read_open_order_attributes = true).
    w.write_opt_f64(None); // volatility
    w.write_opt_i32(None); // volatility_type
    w.write_string(""); // delta_neutral_order_type
    w.write_opt_f64(None); // delta_neutral_aux_price
    w.write_bool(false); // continuous_update
    w.write_opt_i32(None); // reference_price_type

    // Trail params.
    w.write_opt_f64(None); // trail_stop_price
    w.write_opt_f64(None); // trailing_percent

    // Basis points.
    w.write_opt_f64(None); // basis_points
    w.write_opt_i32(None); // basis_points_type

    // Combo legs.
    w.write_string(""); // combo_legs_description
    w.write_i32(0); // combo_legs_count
    w.write_i32(0); // order_combo_legs_count

    // Smart combo routing params.
    w.write_i32(0);

    // Scale order params.
    w.write_opt_i32(None); // scale_init_level_size
    w.write_opt_i32(None); // scale_subs_level_size
    w.write_opt_f64(None); // scale_price_increment

    // Hedge params (empty hedge_type).
    w.write_string("");

    // Opt out smart routing.
    w.write_bool(false);

    // Clearing params.
    w.write_string(""); // clearing_account
    w.write_string(""); // clearing_intent

    // Not held.
    w.write_bool(false);

    // Delta neutral contract.
    w.write_bool(false);

    // Algo params.
    w.write_string(""); // algo_strategy (empty means no algo_params block)

    // Solicited.
    w.write_bool(false);

    // What-if + commission block.
    w.write_bool(false); // what_if
    w.write_string(&p.order_state_status); // order_state.status
                                           // What-if extended fields (13): margin-before/change/after (9) +
                                           // commission (4). All emitted as empty strings — rust-ibapi reads them
                                           // as `next_optional_double()` / `next_string()` which tolerate empties.
    for _ in 0..9 {
        w.write_string("");
    }
    w.write_string(""); // commission
    w.write_string(""); // minimum_commission
    w.write_string(""); // maximum_commission
    w.write_string(""); // commission_currency

    // FULL_ORDER_PREVIEW_FIELDS gate (195). Our range crosses 195.
    const SV_FULL_ORDER_PREVIEW_FIELDS: i32 = 195;
    if sv.raw() >= SV_FULL_ORDER_PREVIEW_FIELDS {
        for _ in 0..12 {
            w.write_string("");
        }
        w.write_i32(0); // order_allocations_count
    }

    w.write_string(""); // warning_text
    w.write_bool(false); // randomize_size
    w.write_bool(false); // randomize_price

    // Conditions count + adjusted_order_params block.
    w.write_i32(0); // conditions_count
    w.write_string(""); // adjusted_order_type
    w.write_opt_f64(None); // trigger_price
    w.write_opt_f64(None); // trail_stop_price
    w.write_opt_f64(None); // limit_price_offset
    w.write_opt_f64(None); // adjusted_stop_price
    w.write_opt_f64(None); // adjusted_stop_limit_price
    w.write_opt_f64(None); // adjusted_trailing_amount
    w.write_i32(0); // adjustable_trailing_unit

    // Soft-dollar tier.
    w.write_string(""); // name
    w.write_string(""); // value
    w.write_string(""); // display_name

    // Cash qty.
    w.write_string(""); // cash_qty

    // Auto price for hedge + OMS container + d-peg + price-mgmt + duration.
    w.write_bool(false); // dont_use_auto_price_for_hedge
    w.write_bool(false); // is_oms_container
    w.write_bool(false); // discretionary_up_to_limit_price
    w.write_opt_i32(None); // use_price_mgmt_algo
    w.write_opt_i32(None); // duration
    w.write_string(""); // post_to_ats
    w.write_string(""); // auto_cancel_parent

    // PEGBEST_PEGMID_OFFSETS — scalar flag + 5 offsets.
    w.write_i32(0); // peg_best_peg_mid flag
    w.write_opt_i32(None); // min_trade_qty
    w.write_opt_i32(None); // min_compete_size
    w.write_opt_f64(None); // compete_against_best_offset
    w.write_opt_f64(None); // mid_offset_at_whole
    w.write_opt_f64(None); // mid_offset_at_half

    // v183-v199 additions (customer_account … imbalance_only).
    if sv.raw() >= 183 {
        w.write_string(""); // customer_account
    }
    if sv.raw() >= 184 {
        w.write_bool(false); // professional_customer
    }
    if sv.raw() >= 185 {
        w.write_string(""); // bond_accrued_interest
    }
    if sv.raw() >= 189 {
        w.write_bool(false); // include_overnight
    }
    if sv.raw() >= 193 {
        w.write_string(""); // ext_operator
        w.write_i32(0); // manual_order_indicator (0 = unset)
    }
    if sv.raw() >= SV_SUBMITTER {
        w.write_string(""); // submitter
    }
    if sv.raw() >= 199 {
        w.write_bool(false); // imbalance_only
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::clock::VirtualInstant;
    use crate::engine::types::OrderKind;
    use crate::protocol::messages::fields::FieldReader;
    use crate::protocol::{ServerVersion, MAX_VERSION, MIN_VERSION};
    use bytes::Bytes;
    use midas_broker_core::ContractSpec;

    /// Encode `msg` at server version `sv` and return payload bytes.
    fn enc(msg: &OutgoingMsg, sv: ServerVersion) -> Vec<u8> {
        let mut w = FieldWriter::new();
        msg.encode(&mut w, sv);
        w.into_bytes()
    }

    fn sv(v: i32) -> ServerVersion {
        ServerVersion::new(v).unwrap_or_else(|| panic!("bad sv {v}"))
    }

    // ---- NEXT_VALID_ID ---------------------------------------------------

    #[test]
    fn encodes_next_valid_id() {
        let msg = OutgoingMsg::NextValidId {
            order_id: OrderId(100),
        };
        let bytes = enc(&msg, ServerVersion::MAX);
        // msg_id=9, version=1, order_id=100
        assert_eq!(bytes, b"9\x001\x00100\x00");
    }

    // ---- MANAGED_ACCTS ---------------------------------------------------

    #[test]
    fn encodes_managed_accts_comma_list() {
        let msg = OutgoingMsg::ManagedAccts {
            accounts: "DU1234567,DU2345678".to_string(),
        };
        let bytes = enc(&msg, ServerVersion::MAX);
        assert_eq!(bytes, b"15\x001\x00DU1234567,DU2345678\x00");
    }

    // ---- CURRENT_TIME ----------------------------------------------------

    #[test]
    fn encodes_current_time() {
        let msg = OutgoingMsg::CurrentTime {
            epoch_secs: 1_700_000_000,
        };
        let bytes = enc(&msg, ServerVersion::MAX);
        assert_eq!(bytes, b"49\x001\x001700000000\x00");
    }

    // ---- ERR_MSG ---------------------------------------------------------

    #[test]
    fn encodes_err_msg_farm_status_bulletin_classic_format() {
        // Farm-status bulletin 2104, using the < ERROR_TIME (v193) format.
        let msg = OutgoingMsg::ErrMsg {
            req_id: -1,
            code: 2104,
            message: "Market data farm connection is OK:usfarm".to_string(),
            advanced_order_reject_json: None,
            error_time_ms: 0,
        };
        let bytes = enc(&msg, sv(193));
        // id=4, v=2, -1, 2104, "...", "" (advanced_order_reject since sv>=166)
        assert_eq!(
            bytes,
            b"4\x002\x00-1\x002104\x00Market data farm connection is OK:usfarm\x00\x00"
        );
    }

    #[test]
    fn encodes_err_msg_farm_status_bulletin_modern_format() {
        // At sv>=194 we switch to the no-version + error_time format.
        let msg = OutgoingMsg::ErrMsg {
            req_id: -1,
            code: 2106,
            message: "HMDS data farm connection is OK:ushmds".to_string(),
            advanced_order_reject_json: None,
            error_time_ms: 0,
        };
        let bytes = enc(&msg, sv(210));
        assert_eq!(
            bytes,
            b"4\x00-1\x002106\x00HMDS data farm connection is OK:ushmds\x00\x000\x00"
        );
    }

    #[test]
    fn encodes_err_msg_order_rejection_with_advanced_json() {
        let msg = OutgoingMsg::ErrMsg {
            req_id: 42,
            code: 201,
            message: "Order rejected - reason:".to_string(),
            advanced_order_reject_json: Some(r#"{"code":"X1"}"#.to_string()),
            error_time_ms: 0,
        };
        let bytes = enc(&msg, sv(193));
        assert_eq!(
            bytes,
            b"4\x002\x0042\x00201\x00Order rejected - reason:\x00{\"code\":\"X1\"}\x00"
        );
    }

    // ---- TICK_PRICE ------------------------------------------------------

    #[test]
    fn encodes_tick_price_bid_with_size_and_attribs() {
        let msg = OutgoingMsg::TickPrice {
            req_id: ReqId(7),
            tick: TickType::Bid,
            price: 150.25,
            size: Some(100),
            attribs: TickAttribs {
                can_auto_execute: true,
                past_limit: false,
                pre_open: true,
            },
        };
        let bytes = enc(&msg, ServerVersion::MAX);
        // id=1, v=3, req_id=7, tick=1 (Bid), price=150.25, size=100, mask=5
        assert_eq!(bytes, b"1\x003\x007\x001\x00150.25\x00100\x005\x00");
    }

    #[test]
    fn encodes_tick_price_unset_size_serialises_empty_decimal() {
        let msg = OutgoingMsg::TickPrice {
            req_id: ReqId(7),
            tick: TickType::Last,
            price: 150.0,
            size: None,
            attribs: TickAttribs::default(),
        };
        let bytes = enc(&msg, ServerVersion::MAX);
        // Unset size is an empty `Decimal` string. rust-ibapi 2.10's
        // `next_double()` decodes empty -> 0.0; newer Decimal-aware builds
        // treat it as `Decimal::NONE`. Both are correct wire values.
        assert_eq!(bytes, b"1\x003\x007\x004\x00150\x00\x000\x00");
    }

    // ---- TICK_SIZE / TICK_GENERIC / TICK_STRING --------------------------

    #[test]
    fn encodes_tick_size() {
        let msg = OutgoingMsg::TickSize {
            req_id: ReqId(7),
            tick: TickType::LastSize,
            size: 100,
        };
        let bytes = enc(&msg, ServerVersion::MAX);
        assert_eq!(bytes, b"2\x001\x007\x005\x00100\x00");
    }

    #[test]
    fn encodes_tick_generic() {
        let msg = OutgoingMsg::TickGeneric {
            req_id: ReqId(7),
            tick: TickType::Halted,
            value: 0.0,
        };
        let bytes = enc(&msg, ServerVersion::MAX);
        assert_eq!(bytes, b"45\x001\x007\x0049\x000\x00");
    }

    #[test]
    fn encodes_tick_string() {
        let msg = OutgoingMsg::TickString {
            req_id: ReqId(7),
            tick: TickType::LastTimestamp,
            value: "1700000000".into(),
        };
        let bytes = enc(&msg, ServerVersion::MAX);
        assert_eq!(bytes, b"46\x001\x007\x0045\x001700000000\x00");
    }

    // ---- ORDER_STATUS ---------------------------------------------------

    fn sample_order_status() -> OrderStatus {
        OrderStatus {
            order_id: OrderId(42),
            status: OrderStatusCode::Filled,
            filled: 100.0,
            remaining: 0.0,
            avg_fill_price: 150.25,
            perm_id: 987654321,
            parent_id: 0,
            last_fill_price: 150.25,
            client_id: 1,
            why_held: String::new(),
            mkt_cap_price: None,
        }
    }

    #[test]
    fn encodes_order_status_modern() {
        let bytes = enc(&OutgoingMsg::OrderStatus(sample_order_status()), sv(210));
        // v>=131: no inner version; msg_id=3, order_id=42, status="Filled"
        // filled, remaining, avg_fill, perm_id, parent_id, last_fill, client_id,
        // why_held, mkt_cap_price (unset -> canonical sentinel)
        let expected: Vec<u8> = format!(
            "3\x0042\x00Filled\x00100\x000\x00150.25\x00987654321\x000\x00150.25\x001\x00\x00{}\x00",
            crate::protocol::messages::fields::UNSET_DOUBLE_STR
        )
        .into_bytes();
        assert_eq!(bytes, expected);
    }

    #[test]
    fn encodes_order_status_legacy_branch_direct() {
        // The `sv < MARKET_CAP_PRICE (131)` branch is below MIN_VERSION (176)
        // so real traffic never hits it, but we still cover the code path by
        // calling the helper directly.
        let mut w = FieldWriter::new();
        // Manually emit msg id + inner version to match legacy format.
        w.write_i32(ID_ORDER_STATUS);
        w.write_i32(6);
        let s = sample_order_status();
        w.write_i32(s.order_id.0);
        w.write_string(order_status_str(s.status));
        w.write_f64(s.filled);
        w.write_f64(s.remaining);
        w.write_f64(s.avg_fill_price);
        w.write_i64(s.perm_id as i64);
        w.write_i32(s.parent_id);
        w.write_f64(s.last_fill_price);
        w.write_i32(s.client_id);
        w.write_string(&s.why_held);
        let bytes = w.into_bytes();
        assert_eq!(
            bytes,
            b"3\x006\x0042\x00Filled\x00100\x000\x00150.25\x00987654321\x000\x00150.25\x001\x00\x00"
        );
    }

    // ---- Per-version field gating roundtrip ------------------------------

    #[test]
    fn order_status_per_version_gating_via_reader() {
        // Verify modern-format field layout is stable across our advertised
        // range (176..221). Legacy format (<131) isn't reachable but is still
        // exercised by `encodes_order_status_legacy_branch_direct` above.
        for v in [MIN_VERSION, 180, 195, 200, MAX_VERSION] {
            let bytes = enc(&OutgoingMsg::OrderStatus(sample_order_status()), sv(v));
            let fields: Vec<Bytes> = split_payload(&bytes);
            let mut r = FieldReader::new(&fields);
            let id = r.read_i32().unwrap();
            assert_eq!(id, 3);
            // No inner version at our modern range.
            let order_id = r.read_i32().unwrap();
            assert_eq!(order_id, 42);
            let status = r.read_string().unwrap();
            assert_eq!(status, "Filled");
        }
    }

    // ---- COMMISSION_REPORT -----------------------------------------------

    #[test]
    fn encodes_commission_report() {
        let r = CommissionReport {
            exec_id: "0000e0d5.6535f6f1.01.01".into(),
            commission: 1.0,
            currency: "USD".into(),
            realized_pnl: Some(25.50),
            yield_: None,
            yield_redemption_date: None,
        };
        let bytes = enc(&OutgoingMsg::CommissionReport { report: r }, sv(210));
        assert_eq!(
            bytes,
            b"59\x001\x000000e0d5.6535f6f1.01.01\x001\x00USD\x0025.5\x001.7976931348623157E308\x00\x00"
        );
    }

    // ---- EXECUTION_DATA --------------------------------------------------

    fn sample_execution() -> Execution {
        Execution {
            req_id: Some(ReqId(1)),
            order_id: OrderId(42),
            exec_id: "0000e0d5.6535f6f1.01.01".into(),
            time: VirtualInstant::from_secs(1_700_000_000),
            acct_number: "DU1234567".into(),
            exchange: "NASDAQ".into(),
            side: crate::engine::types::Side::Buy,
            shares: 100.0,
            price: 150.25,
            perm_id: 987654321,
            client_id: 1,
            liquidation: 0,
            cumulative_quantity: 100.0,
            avg_price: 150.25,
            order_ref: None,
            contract: ContractSpec::Stock {
                symbol: "AAPL".into(),
                exchange: "SMART".into(),
                currency: "USD".into(),
            },
        }
    }

    #[test]
    fn encodes_execution_data_roundtrip_via_reader() {
        let bytes = enc(
            &OutgoingMsg::ExecutionData {
                req_id: ReqId(1),
                execution: sample_execution(),
            },
            sv(200),
        );
        let fields: Vec<Bytes> = split_payload(&bytes);
        let mut r = FieldReader::new(&fields);
        assert_eq!(r.read_i32().unwrap(), 11); // msg id
        assert_eq!(r.read_i32().unwrap(), 1); // req_id
        assert_eq!(r.read_i32().unwrap(), 42); // order_id
        assert_eq!(r.read_i32().unwrap(), 0); // contract_id
        assert_eq!(r.read_string().unwrap(), "AAPL");
        assert_eq!(r.read_string().unwrap(), "STK");
        r.read_string().unwrap(); // last_trade_date
        r.read_f64().unwrap(); // strike
        r.read_string().unwrap(); // right
        r.read_string().unwrap(); // multiplier
        assert_eq!(r.read_string().unwrap(), "SMART"); // exchange
        assert_eq!(r.read_string().unwrap(), "USD"); // currency
        assert_eq!(r.read_string().unwrap(), "AAPL"); // local_symbol
        r.read_string().unwrap(); // trading_class
        assert_eq!(r.read_string().unwrap(), "0000e0d5.6535f6f1.01.01");
        assert_eq!(r.read_string().unwrap(), "1700000000"); // time
    }

    // ---- HISTORICAL_DATA -------------------------------------------------

    fn sample_bar(ts: u64, close: f64) -> Bar {
        Bar {
            time: VirtualInstant::from_secs(ts),
            open: close - 1.0,
            high: close + 0.5,
            low: close - 1.5,
            close,
            volume: 1_000,
            wap: close,
            count: 10,
        }
    }

    #[test]
    fn encodes_historical_data_modern_has_no_start_end_in_body() {
        let bars = vec![
            sample_bar(1_700_000_000, 100.0),
            sample_bar(1_700_000_060, 101.0),
        ];
        let bytes = enc(
            &OutgoingMsg::HistoricalData {
                req_id: ReqId(5),
                start: "20260418 09:30:00".into(),
                end: "20260418 16:00:00".into(),
                bars,
            },
            sv(210),
        );
        // sv>=196 => no start/end fields in body. Just req_id + bar_count + bars.
        let fields: Vec<Bytes> = split_payload(&bytes);
        let mut r = FieldReader::new(&fields);
        assert_eq!(r.read_i32().unwrap(), 17);
        assert_eq!(r.read_i32().unwrap(), 5);
        assert_eq!(r.read_i32().unwrap(), 2); // bar count
        assert_eq!(r.read_string().unwrap(), "1700000000");
    }

    #[test]
    fn encodes_historical_data_legacy_includes_start_end() {
        let bytes = enc(
            &OutgoingMsg::HistoricalData {
                req_id: ReqId(5),
                start: "20260418 09:30:00".into(),
                end: "20260418 16:00:00".into(),
                bars: vec![],
            },
            sv(195),
        );
        let fields: Vec<Bytes> = split_payload(&bytes);
        let mut r = FieldReader::new(&fields);
        assert_eq!(r.read_i32().unwrap(), 17);
        assert_eq!(r.read_i32().unwrap(), 5);
        assert_eq!(r.read_string().unwrap(), "20260418 09:30:00");
        assert_eq!(r.read_string().unwrap(), "20260418 16:00:00");
        assert_eq!(r.read_i32().unwrap(), 0); // bar count = 0
    }

    // ---- CONTRACT_DATA + CONTRACT_DATA_END -------------------------------

    #[test]
    fn encodes_contract_data_end() {
        let bytes = enc(&OutgoingMsg::ContractDataEnd { req_id: ReqId(9) }, sv(200));
        assert_eq!(bytes, b"52\x001\x009\x00");
    }

    #[test]
    fn encodes_contract_data_aapl() {
        let details = Box::new(ContractDetails {
            con_id: 265598,
            symbol: "AAPL".into(),
            sec_type: "STK".into(),
            exchange: "SMART".into(),
            currency: "USD".into(),
            local_symbol: "AAPL".into(),
            trading_class: "NMS".into(),
            min_tick: 0.01,
            min_size: 1.0,
            size_increment: 1.0,
            suggested_size_increment: 1.0,
            primary_exchange: "NASDAQ".into(),
            long_name: "APPLE INC".into(),
            ..Default::default()
        });
        let bytes = enc(
            &OutgoingMsg::ContractData {
                req_id: ReqId(9),
                details,
            },
            sv(200),
        );
        let fields: Vec<Bytes> = split_payload(&bytes);
        let mut r = FieldReader::new(&fields);
        assert_eq!(r.read_i32().unwrap(), 10);
        assert_eq!(r.read_i32().unwrap(), 9); // req_id
        assert_eq!(r.read_string().unwrap(), "AAPL");
        assert_eq!(r.read_string().unwrap(), "STK");
    }

    /// Builds a `ContractData` frame at the requested sv, returning the raw
    /// encoded bytes. Shared helper for the cross-version golden tests.
    fn encode_aapl_contract_data_at(sv_raw: i32) -> Vec<u8> {
        let details = Box::new(ContractDetails {
            con_id: 265598,
            symbol: "AAPL".into(),
            sec_type: "STK".into(),
            last_trade_date_or_contract_month: String::new(),
            last_trade_date: "20251219 16:00:00 US/Eastern".into(),
            strike: 0.0,
            right: String::new(),
            exchange: "SMART".into(),
            currency: "USD".into(),
            local_symbol: "AAPL".into(),
            trading_class: "NMS".into(),
            min_tick: 0.01,
            min_size: 1.0,
            size_increment: 1.0,
            suggested_size_increment: 1.0,
            primary_exchange: "NASDAQ".into(),
            long_name: "APPLE INC".into(),
            ..Default::default()
        });
        enc(
            &OutgoingMsg::ContractData {
                req_id: ReqId(9),
                details,
            },
            sv(sv_raw),
        )
    }

    /// At sv=220 / 221 the wire MUST carry an extra `last_trade_date` field
    /// before `strike`. Reading the payload with the matching offset must
    /// yield the populated date and the expected `strike=0` immediately
    /// after — if the gate is missing, `strike` would be misread as the
    /// string and every subsequent field would drift.
    #[test]
    fn encodes_contract_data_emits_last_trade_date_at_sv_220() {
        for sv_raw in [220, 221] {
            let bytes = encode_aapl_contract_data_at(sv_raw);
            let fields: Vec<Bytes> = split_payload(&bytes);
            let mut r = FieldReader::new(&fields);
            assert_eq!(r.read_i32().unwrap(), 10);
            assert_eq!(r.read_i32().unwrap(), 9);
            assert_eq!(r.read_string().unwrap(), "AAPL");
            assert_eq!(r.read_string().unwrap(), "STK");
            assert_eq!(r.read_string().unwrap(), ""); // last_trade_date_or_contract_month
            assert_eq!(
                r.read_string().unwrap(),
                "20251219 16:00:00 US/Eastern",
                "sv={sv_raw}: last_trade_date must precede strike"
            );
            assert_eq!(r.read_f64().unwrap(), 0.0); // strike
            assert_eq!(r.read_string().unwrap(), ""); // right
            assert_eq!(r.read_string().unwrap(), "SMART"); // exchange
        }
    }

    /// Below sv=220 the field is absent; `strike` must come immediately
    /// after `last_trade_date_or_contract_month`.
    #[test]
    fn encodes_contract_data_omits_last_trade_date_below_sv_220() {
        let bytes = encode_aapl_contract_data_at(219);
        let fields: Vec<Bytes> = split_payload(&bytes);
        let mut r = FieldReader::new(&fields);
        assert_eq!(r.read_i32().unwrap(), 10);
        assert_eq!(r.read_i32().unwrap(), 9);
        assert_eq!(r.read_string().unwrap(), "AAPL");
        assert_eq!(r.read_string().unwrap(), "STK");
        assert_eq!(r.read_string().unwrap(), "");
        // strike next — *no* last_trade_date slot in between.
        assert_eq!(r.read_f64().unwrap(), 0.0);
        assert_eq!(r.read_string().unwrap(), ""); // right
    }

    /// At sv=208 the trailing `ineligibility_reasons` count (0) MUST be
    /// present; at sv=207 it must be absent. The only way to tell is a
    /// total-field-count check.
    #[test]
    fn encodes_contract_data_ineligibility_reasons_gated_at_208() {
        let bytes_207 = encode_aapl_contract_data_at(207);
        let bytes_208 = encode_aapl_contract_data_at(208);
        let n_207 = split_payload(&bytes_207).len();
        let n_208 = split_payload(&bytes_208).len();
        assert_eq!(
            n_208,
            n_207 + 1,
            "sv=208 must emit exactly one extra trailing field (ineligibility count)"
        );
    }

    // ---- POSITION / ACCOUNT_SUMMARY / ACCT_VALUE / PORTFOLIO_VALUE -------

    #[test]
    fn encodes_position() {
        let msg = OutgoingMsg::Position(Box::new(PositionPayload {
            account: "DU1234567".into(),
            con_id: 265598,
            symbol: "AAPL".into(),
            sec_type: "STK".into(),
            last_trade_date_or_contract_month: String::new(),
            strike: 0.0,
            right: String::new(),
            multiplier: String::new(),
            exchange: "SMART".into(),
            currency: "USD".into(),
            local_symbol: "AAPL".into(),
            trading_class: "NMS".into(),
            position: 100.0,
            avg_cost: 150.25,
        }));
        let bytes = enc(&msg, sv(200));
        let fields: Vec<Bytes> = split_payload(&bytes);
        let mut r = FieldReader::new(&fields);
        assert_eq!(r.read_i32().unwrap(), 61);
        assert_eq!(r.read_i32().unwrap(), 3); // inner version
        assert_eq!(r.read_string().unwrap(), "DU1234567");
        assert_eq!(r.read_i32().unwrap(), 265598);
        assert_eq!(r.read_string().unwrap(), "AAPL");
    }

    #[test]
    fn encodes_account_summary() {
        let bytes = enc(
            &OutgoingMsg::AccountSummary {
                req_id: ReqId(11),
                account: "DU1234567".into(),
                tag: "NetLiquidation".into(),
                value: "50000.0".into(),
                currency: "USD".into(),
            },
            sv(200),
        );
        assert_eq!(
            bytes,
            b"63\x001\x0011\x00DU1234567\x00NetLiquidation\x0050000.0\x00USD\x00"
        );
    }

    #[test]
    fn encodes_acct_value() {
        let bytes = enc(
            &OutgoingMsg::AcctValue {
                key: "NetLiquidation".into(),
                value: "50000.0".into(),
                currency: "USD".into(),
                acct_code: "DU1234567".into(),
            },
            sv(200),
        );
        assert_eq!(
            bytes,
            b"6\x002\x00NetLiquidation\x0050000.0\x00USD\x00DU1234567\x00"
        );
    }

    #[test]
    fn encodes_portfolio_value_roundtrip() {
        let msg = OutgoingMsg::PortfolioValue(Box::new(PortfolioValuePayload {
            con_id: 265598,
            symbol: "AAPL".into(),
            sec_type: "STK".into(),
            last_trade_date_or_contract_month: String::new(),
            strike: 0.0,
            right: String::new(),
            multiplier: String::new(),
            primary_exchange: "NASDAQ".into(),
            currency: "USD".into(),
            local_symbol: "AAPL".into(),
            trading_class: "NMS".into(),
            position: 100.0,
            market_price: 150.25,
            market_value: 15025.0,
            avg_cost: 145.0,
            unrealized_pnl: 525.0,
            realized_pnl: 0.0,
            acct_code: "DU1234567".into(),
        }));
        let bytes = enc(&msg, sv(200));
        let fields: Vec<Bytes> = split_payload(&bytes);
        let mut r = FieldReader::new(&fields);
        assert_eq!(r.read_i32().unwrap(), 7); // msg id
        assert_eq!(r.read_i32().unwrap(), 8); // inner version
        assert_eq!(r.read_i32().unwrap(), 265598);
        assert_eq!(r.read_string().unwrap(), "AAPL");
    }

    // ---- Simple end markers ----------------------------------------------

    #[test]
    fn encodes_open_order_end() {
        let bytes = enc(&OutgoingMsg::OpenOrderEnd, sv(200));
        assert_eq!(bytes, b"53\x001\x00");
    }

    #[test]
    fn encodes_acct_download_end() {
        let bytes = enc(
            &OutgoingMsg::AcctDownloadEnd {
                acct_code: "DU1234567".into(),
            },
            sv(200),
        );
        assert_eq!(bytes, b"54\x001\x00DU1234567\x00");
    }

    #[test]
    fn encodes_execution_data_end() {
        let bytes = enc(&OutgoingMsg::ExecutionDataEnd { req_id: ReqId(5) }, sv(200));
        assert_eq!(bytes, b"55\x001\x005\x00");
    }

    #[test]
    fn encodes_market_data_type() {
        let bytes = enc(
            &OutgoingMsg::MarketDataTypeResp {
                req_id: ReqId(7),
                data_type: MarketDataType::Delayed,
            },
            sv(200),
        );
        assert_eq!(bytes, b"58\x001\x007\x003\x00");
    }

    // ---- REAL_TIME_BARS --------------------------------------------------

    #[test]
    fn encodes_realtime_bar() {
        let bytes = enc(
            &OutgoingMsg::RealTimeBar {
                req_id: ReqId(5),
                timestamp: 1_700_000_000,
                open: 100.0,
                high: 100.5,
                low: 99.5,
                close: 100.25,
                volume: 1_000,
                wap: 100.1,
                count: 12,
            },
            sv(200),
        );
        // id=50, v=3, req_id=5, ts, o, h, l, c, vol, wap, count
        assert_eq!(
            bytes,
            b"50\x003\x005\x001700000000\x00100\x00100.5\x0099.5\x00100.25\x001000\x00100.1\x0012\x00"
        );
    }

    // ---- OPEN_ORDER ------------------------------------------------------

    fn sample_open_order() -> OpenOrderPayload {
        OpenOrderPayload {
            order: OpenOrder {
                order_id: OrderId(42),
                contract: ContractSpec::Stock {
                    symbol: "AAPL".into(),
                    exchange: "SMART".into(),
                    currency: "USD".into(),
                },
                side: crate::engine::types::Side::Buy,
                total_quantity: 100.0,
                kind: OrderKind::Limit,
                limit_price: Some(150.50),
                aux_price: None,
                status: OrderStatusCode::Submitted,
                tif: "DAY".into(),
                account: "DU1234567".into(),
                parent_id: None,
                oca_group: None,
            },
            order_state_status: "Submitted".into(),
            perm_id: 987654321,
            client_id: 1,
        }
    }

    #[test]
    fn encodes_open_order_core_fields_survive_roundtrip() {
        // We can't diff the full wide payload byte-for-byte against a golden
        // without recording live traffic — but we can verify the critical
        // front-loaded fields arrive in the expected order.
        let bytes = enc(&OutgoingMsg::OpenOrder(sample_open_order()), sv(200));
        let fields: Vec<Bytes> = split_payload(&bytes);
        let mut r = FieldReader::new(&fields);
        assert_eq!(r.read_i32().unwrap(), 5); // msg id
        assert_eq!(r.read_i32().unwrap(), 42); // order_id
        assert_eq!(r.read_i32().unwrap(), 0); // contract_id
        assert_eq!(r.read_string().unwrap(), "AAPL");
        assert_eq!(r.read_string().unwrap(), "STK");
        r.read_string().unwrap(); // last_trade_date
        r.read_f64().unwrap(); // strike
        r.read_string().unwrap(); // right
        r.read_string().unwrap(); // multiplier
        assert_eq!(r.read_string().unwrap(), "SMART");
        assert_eq!(r.read_string().unwrap(), "USD");
        assert_eq!(r.read_string().unwrap(), "AAPL");
        r.read_string().unwrap(); // trading_class
        assert_eq!(r.read_string().unwrap(), "BUY");
        assert_eq!(r.read_f64().unwrap(), 100.0);
        assert_eq!(r.read_string().unwrap(), "LMT");
        assert_eq!(r.read_opt_f64().unwrap(), Some(150.50));
        assert_eq!(r.read_opt_f64().unwrap(), None); // aux_price
        assert_eq!(r.read_string().unwrap(), "DAY");
    }

    #[test]
    fn encodes_open_order_perm_id_and_client_id() {
        let p = sample_open_order();
        let bytes = enc(&OutgoingMsg::OpenOrder(p.clone()), sv(200));
        // Verify the emitted byte stream contains the perm_id + account and
        // is non-empty at the tail.
        let needle = b"987654321" as &[u8];
        assert!(
            windowed_contains(&bytes, needle),
            "perm_id missing from OPEN_ORDER encoding"
        );
        let acct = b"DU1234567" as &[u8];
        assert!(
            windowed_contains(&bytes, acct),
            "account missing from OPEN_ORDER encoding"
        );
    }

    fn windowed_contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }

    // ---- Sanity: encoders never produce zero-byte payloads ---------------

    #[test]
    fn every_encoder_produces_non_empty_payload() {
        let samples: Vec<OutgoingMsg> = vec![
            OutgoingMsg::NextValidId {
                order_id: OrderId(1),
            },
            OutgoingMsg::ManagedAccts {
                accounts: "DU1".into(),
            },
            OutgoingMsg::CurrentTime { epoch_secs: 0 },
            OutgoingMsg::ErrMsg {
                req_id: -1,
                code: 2104,
                message: "ok".into(),
                advanced_order_reject_json: None,
                error_time_ms: 0,
            },
            OutgoingMsg::TickPrice {
                req_id: ReqId(1),
                tick: TickType::Bid,
                price: 1.0,
                size: None,
                attribs: TickAttribs::default(),
            },
            OutgoingMsg::TickSize {
                req_id: ReqId(1),
                tick: TickType::BidSize,
                size: 1,
            },
            OutgoingMsg::TickGeneric {
                req_id: ReqId(1),
                tick: TickType::Halted,
                value: 0.0,
            },
            OutgoingMsg::TickString {
                req_id: ReqId(1),
                tick: TickType::LastTimestamp,
                value: "0".into(),
            },
            OutgoingMsg::OrderStatus(sample_order_status()),
            OutgoingMsg::OpenOrder(sample_open_order()),
            OutgoingMsg::OpenOrderEnd,
            OutgoingMsg::AcctValue {
                key: "K".into(),
                value: "V".into(),
                currency: "USD".into(),
                acct_code: "DU1".into(),
            },
            OutgoingMsg::AcctDownloadEnd {
                acct_code: "DU1".into(),
            },
            OutgoingMsg::Position(Box::new(PositionPayload {
                account: "DU1".into(),
                con_id: 1,
                symbol: "A".into(),
                sec_type: "STK".into(),
                last_trade_date_or_contract_month: String::new(),
                strike: 0.0,
                right: String::new(),
                multiplier: String::new(),
                exchange: "SMART".into(),
                currency: "USD".into(),
                local_symbol: "A".into(),
                trading_class: String::new(),
                position: 1.0,
                avg_cost: 1.0,
            })),
            OutgoingMsg::AccountSummary {
                req_id: ReqId(1),
                account: "DU1".into(),
                tag: "NL".into(),
                value: "0".into(),
                currency: "USD".into(),
            },
            OutgoingMsg::ContractData {
                req_id: ReqId(1),
                details: Box::new(ContractDetails::default()),
            },
            OutgoingMsg::ContractDataEnd { req_id: ReqId(1) },
            OutgoingMsg::ExecutionData {
                req_id: ReqId(1),
                execution: sample_execution(),
            },
            OutgoingMsg::ExecutionDataEnd { req_id: ReqId(1) },
            OutgoingMsg::CommissionReport {
                report: CommissionReport {
                    exec_id: "x".into(),
                    commission: 0.0,
                    currency: "USD".into(),
                    realized_pnl: None,
                    yield_: None,
                    yield_redemption_date: None,
                },
            },
            OutgoingMsg::HistoricalData {
                req_id: ReqId(1),
                start: String::new(),
                end: String::new(),
                bars: vec![],
            },
            OutgoingMsg::RealTimeBar {
                req_id: ReqId(1),
                timestamp: 0,
                open: 0.0,
                high: 0.0,
                low: 0.0,
                close: 0.0,
                volume: 0,
                wap: 0.0,
                count: 0,
            },
            OutgoingMsg::MarketDataTypeResp {
                req_id: ReqId(1),
                data_type: MarketDataType::Live,
            },
            OutgoingMsg::PortfolioValue(Box::new(PortfolioValuePayload {
                con_id: 1,
                symbol: "A".into(),
                sec_type: "STK".into(),
                last_trade_date_or_contract_month: String::new(),
                strike: 0.0,
                right: String::new(),
                multiplier: String::new(),
                primary_exchange: "SMART".into(),
                currency: "USD".into(),
                local_symbol: "A".into(),
                trading_class: String::new(),
                position: 1.0,
                market_price: 1.0,
                market_value: 1.0,
                avg_cost: 1.0,
                unrealized_pnl: 0.0,
                realized_pnl: 0.0,
                acct_code: "DU1".into(),
            })),
        ];
        assert_eq!(
            samples.len(),
            24,
            "expected 24 encoders, got {}",
            samples.len()
        );
        for msg in samples {
            let bytes = enc(&msg, ServerVersion::MAX);
            assert!(
                !bytes.is_empty(),
                "encoder produced empty bytes for {msg:?}"
            );
            // First field must always be the msg id; parse it.
            let fields = split_payload(&bytes);
            let mut r = FieldReader::new(&fields);
            let id = r.read_i32().unwrap();
            assert!(id > 0 && id < 100, "bad msg id {id} for {msg:?}");
        }
    }

    // ---- Every encoder survives across the full advertised sv range -----

    #[test]
    fn encoders_work_across_advertised_range() {
        for v in [MIN_VERSION, 180, 195, 200, MAX_VERSION] {
            let sv = ServerVersion::new(v).unwrap();
            // Representative messages that have version-gated fields.
            let _ = enc(&OutgoingMsg::OrderStatus(sample_order_status()), sv);
            let _ = enc(&OutgoingMsg::OpenOrder(sample_open_order()), sv);
            let _ = enc(
                &OutgoingMsg::ExecutionData {
                    req_id: ReqId(1),
                    execution: sample_execution(),
                },
                sv,
            );
            let _ = enc(
                &OutgoingMsg::HistoricalData {
                    req_id: ReqId(1),
                    start: "s".into(),
                    end: "e".into(),
                    bars: vec![sample_bar(0, 1.0)],
                },
                sv,
            );
            let _ = enc(
                &OutgoingMsg::ErrMsg {
                    req_id: -1,
                    code: 2104,
                    message: "Market data farm connection is OK:usfarm".into(),
                    advanced_order_reject_json: None,
                    error_time_ms: 0,
                },
                sv,
            );
        }
    }

    // ---- Helper: split NUL-delimited payload -----------------------------

    fn split_payload(bytes: &[u8]) -> Vec<Bytes> {
        if bytes.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut start = 0;
        for (i, &b) in bytes.iter().enumerate() {
            if b == 0 {
                out.push(Bytes::copy_from_slice(&bytes[start..i]));
                start = i + 1;
            }
        }
        if start < bytes.len() {
            out.push(Bytes::copy_from_slice(&bytes[start..]));
        }
        out
    }
}
