//! Protocol-level wire types.
//!
//! These are intermediate representations — flat structs that carry every
//! field the TWS wire protocol places in a given message, including those the
//! engine doesn't (yet) act on. The engine translates them into its own
//! richer domain types (`midas_broker_core::ContractSpec`, `PlaceOrderReq`,
//! …) via `From` / `TryFrom` conversions layered on top.
//!
//! Keeping the wire types separate from the engine types means:
//! 1. We can round-trip every byte without lossy enum-flattening
//!    (e.g. `trading_class`, `include_expired`, `primary_exchange` survive).
//! 2. The engine can add new variants to its domain enums without breaking
//!    the parsers.
//! 3. The protocol layer has a well-defined "raw-struct" boundary which
//!    keeps the parser straightforward.

use serde::{Deserialize, Serialize};

/// Wire-level contract descriptor. Mirrors the flat field set IB's protocol
/// serialises for every contract reference — stocks, options, futures,
/// forex, spreads. Field order on the wire is preserved as struct field
/// declaration order.
///
/// Not all fields are populated by every call site; empty strings and
/// `0.0` / `0` are legitimate "unspecified" markers (IB's own clients treat
/// them the same way).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ContractSpec {
    /// IB contract ID (0 when unset).
    pub contract_id: i32,
    pub symbol: String,
    pub security_type: String,
    pub last_trade_date_or_contract_month: String,
    pub strike: f64,
    /// Option right: `"C"`, `"P"`, or empty.
    pub right: String,
    pub multiplier: String,
    pub exchange: String,
    pub primary_exchange: String,
    pub currency: String,
    pub local_symbol: String,
    /// Only present on server version >= `TRADING_CLASS` (68).
    pub trading_class: String,
    pub include_expired: bool,
    pub security_id_type: String,
    pub security_id: String,
    /// Added at server version >= `BOND_ISSUERID` (176).
    pub issuer_id: String,
    /// Combo-leg payload for BAG security type.
    pub combo_legs: Vec<ComboLeg>,
    /// Optional delta-neutral attached contract (triplet contract-id / delta / price).
    pub delta_neutral_contract: Option<DeltaNeutralContract>,
}

/// Combo leg inside a BAG-security `ContractSpec`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ComboLeg {
    pub contract_id: i32,
    pub ratio: i32,
    pub action: String,
    pub exchange: String,
    /// Open/close indicator: 0 = Same, 1 = Open, 2 = Close, 3 = Unknown.
    pub open_close: i32,
    pub short_sale_slot: i32,
    pub designated_location: String,
    pub exempt_code: i32,
}

/// Order-specific combo leg (carries a per-leg price for ORDER_COMBO_LEGS_PRICE).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OrderComboLeg {
    pub price: Option<f64>,
}

/// Delta-neutral contract triplet sent alongside a regular contract.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DeltaNeutralContract {
    pub contract_id: i32,
    pub delta: f64,
    pub price: f64,
}

// ---------------------------------------------------------------------------
// TagValue — key=value pair used across options / misc-options fields.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TagValue {
    pub tag: String,
    pub value: String,
}

impl TagValue {
    pub fn new(tag: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            tag: tag.into(),
            value: value.into(),
        }
    }
}

// ---------------------------------------------------------------------------
// MarketDataType — mirror of the engine/types version, isolated at the
// protocol layer so parsers don't need a cross-crate dep.
// ---------------------------------------------------------------------------

#[derive(Copy, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[repr(i32)]
pub enum MarketDataType {
    Live = 1,
    Frozen = 2,
    Delayed = 3,
    DelayedFrozen = 4,
}

impl MarketDataType {
    pub fn from_i32(v: i32) -> Option<Self> {
        match v {
            1 => Some(Self::Live),
            2 => Some(Self::Frozen),
            3 => Some(Self::Delayed),
            4 => Some(Self::DelayedFrozen),
            _ => None,
        }
    }

    pub fn as_i32(self) -> i32 {
        self as i32
    }
}

// ---------------------------------------------------------------------------
// ExecutionFilter — `reqExecutions` argument bundle.
// ---------------------------------------------------------------------------

/// Filter passed to `reqExecutions` (msg id 7). Empty strings / 0 encode
/// "match anything" per IB's convention — we preserve them verbatim rather
/// than mapping to `Option` so encode/decode is lossless.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionFilter {
    pub client_id: i32,
    pub acct_code: String,
    /// IB's format: `"yyyymmdd-HH:MM:SS"` or `"yyyymmdd HH:MM:SS <TZ>"`.
    pub time: String,
    pub symbol: String,
    pub sec_type: String,
    pub exchange: String,
    pub side: String,
}

// ---------------------------------------------------------------------------
// OrderSpec — flat image of an IB `Order` struct, wire-level.
// ---------------------------------------------------------------------------

/// Every field the TWS `PLACE_ORDER` frame carries. Fields gated by server
/// version or inner message version default to their type's default when
/// absent on the wire, mirroring IB's client convention of "unsent = default".
///
/// This type is deliberately non-exhaustive-feeling (60+ fields). See
/// `rust-ibapi` v1.2.2 `src/orders/encoders.rs::encode_place_order` for the
/// canonical field order.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OrderSpec {
    // ---- primary ----
    /// `"BUY"` / `"SELL"` / `"SSHORT"` / `"SLONG"`.
    pub action: String,
    pub total_quantity: f64,
    pub order_type: String,
    pub limit_price: Option<f64>,
    pub aux_price: Option<f64>,

    // ---- extended ----
    pub tif: String,
    pub oca_group: String,
    pub account: String,
    pub open_close: String,
    pub origin: i32,
    pub order_ref: String,
    pub transmit: bool,
    pub parent_id: i32,
    pub block_order: bool,
    pub sweep_to_fill: bool,
    pub display_size: i32,
    pub trigger_method: i32,
    pub outside_rth: bool,
    pub hidden: bool,

    // ---- combo legs ----
    pub order_combo_legs: Vec<OrderComboLeg>,
    pub smart_combo_routing_params: Vec<TagValue>,

    // ---- misc ----
    pub discretionary_amt: f64,
    pub good_after_time: String,
    pub good_till_date: String,
    pub fa_group: String,
    pub fa_method: String,
    pub fa_percentage: String,
    /// Only present on server version < `FA_PROFILE_DESUPPORT` (177).
    pub fa_profile: String,
    pub model_code: String,
    pub short_sale_slot: i32,
    pub designated_location: String,
    pub exempt_code: i32,
    pub oca_type: i32,
    pub rule_80_a: String,
    pub settling_firm: String,
    pub all_or_none: bool,
    pub min_qty: Option<i32>,
    pub percent_offset: Option<f64>,
    /// Deprecated `e_trade_only` flag on wire (always false on write side).
    pub e_trade_only: bool,
    /// Deprecated `firm_quote_only` flag.
    pub firm_quote_only: bool,
    /// Deprecated `nbbo_price_cap` Option<f64>.
    pub nbbo_price_cap: Option<f64>,
    pub auction_strategy: i32,
    pub starting_price: Option<f64>,
    pub stock_ref_price: Option<f64>,
    pub delta: Option<f64>,
    pub stock_range_lower: Option<f64>,
    pub stock_range_upper: Option<f64>,
    pub override_percentage_constraints: bool,

    // ---- volatility orders ----
    pub volatility: Option<f64>,
    pub volatility_type: Option<i32>,
    pub delta_neutral_order_type: String,
    pub delta_neutral_aux_price: Option<f64>,
    // Delta-neutral extended block (only present when delta_neutral_order_type is non-empty
    // AND server_version >= DELTA_NEUTRAL_CONID).
    pub delta_neutral_con_id: i32,
    pub delta_neutral_settling_firm: String,
    pub delta_neutral_clearing_account: String,
    pub delta_neutral_clearing_intent: String,
    pub delta_neutral_open_close: String,
    pub delta_neutral_short_sale: bool,
    pub delta_neutral_short_sale_slot: i32,
    pub delta_neutral_designated_location: String,

    pub continuous_update: bool,
    pub reference_price_type: Option<i32>,
    pub trail_stop_price: Option<f64>,
    pub trailing_percent: Option<f64>,

    // ---- scale ----
    pub scale_init_level_size: Option<i32>,
    pub scale_subs_level_size: Option<i32>,
    pub scale_price_increment: Option<f64>,
    pub scale_price_adjust_value: Option<f64>,
    pub scale_price_adjust_interval: Option<i32>,
    pub scale_profit_offset: Option<f64>,
    pub scale_auto_reset: bool,
    pub scale_init_position: Option<i32>,
    pub scale_init_fill_qty: Option<i32>,
    pub scale_random_percent: bool,
    pub scale_table: String,
    pub active_start_time: String,
    pub active_stop_time: String,

    // ---- hedging ----
    pub hedge_type: String,
    pub hedge_param: String,
    pub opt_out_smart_routing: bool,
    pub clearing_account: String,
    pub clearing_intent: String,
    pub not_held: bool,

    // ---- algo ----
    pub algo_strategy: String,
    pub algo_params: Vec<TagValue>,
    pub algo_id: String,

    pub what_if: bool,
    pub order_misc_options: Vec<TagValue>,
    pub solicited: bool,

    pub randomize_size: bool,
    pub randomize_price: bool,

    // ---- pegged-to-benchmark ----
    pub reference_contract_id: i32,
    pub is_pegged_change_amount_decrease: bool,
    pub pegged_change_amount: Option<f64>,
    pub reference_change_amount: Option<f64>,
    pub reference_exchange: String,
    pub adjusted_order_type: String,
    pub trigger_price: Option<f64>,
    pub limit_price_offset: Option<f64>,
    pub adjusted_stop_price: Option<f64>,
    pub adjusted_stop_limit_price: Option<f64>,
    pub adjusted_trailing_amount: Option<f64>,
    pub adjustable_trailing_unit: i32,
    /// Per-condition raw string payload — each element is an already-serialised
    /// IB condition. Parsing individual conditions is a v2 concern.
    pub conditions: Vec<String>,
    pub conditions_ignore_rth: bool,
    pub conditions_cancel_order: bool,

    // ---- misc newer fields ----
    pub ext_operator: String,
    pub soft_dollar_tier_name: String,
    pub soft_dollar_tier_value: String,
    pub cash_qty: Option<f64>,
    pub mifid2_decision_maker: String,
    pub mifid2_decision_algo: String,
    pub mifid2_execution_trader: String,
    pub mifid2_execution_algo: String,
    pub dont_use_auto_price_for_hedge: bool,
    pub is_oms_container: bool,
    pub discretionary_up_to_limit_price: bool,
    pub use_price_mgmt_algo: Option<bool>,
    pub duration: Option<i32>,
    pub post_to_ats: Option<i32>,
    pub auto_cancel_parent: bool,
    pub advanced_error_override: String,
    pub manual_order_time: String,

    // ---- pegbest / pegmid ----
    pub min_trade_qty: Option<i32>,
    pub min_compete_size: Option<i32>,
    pub compete_against_best_offset: Option<f64>,
    pub mid_offset_at_whole: Option<f64>,
    pub mid_offset_at_half: Option<f64>,
}
