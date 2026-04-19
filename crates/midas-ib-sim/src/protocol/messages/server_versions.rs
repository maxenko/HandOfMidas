//! Per-feature minimum server version constants.
//!
//! Mirror of the subset of [`rust-ibapi`](https://github.com/wboayue/rust-ibapi)'s
//! `server_versions.rs` that Stage 02b parsers need to gate fields with. We
//! only include the constants referenced in the 17 client→server messages we
//! parse; if a new gate is required, add it here rather than widening the
//! import surface.
//!
//! These values are intentionally plain `i32` constants rather than a typed
//! enum — they get compared against [`ServerVersion::raw`](crate::protocol::ServerVersion::raw)
//! on the hot parse path and the Option<&'static str>-free layout keeps call
//! sites readable (`sv >= server_versions::TRAILING_PERCENT`).
//!
//! Source of truth: `rust-ibapi` v1.2.2 and main, which mirror the official
//! IB/TWS client constants. See `plan/ib-sim/research/tws-wire-protocol.md`.
#![allow(dead_code)]

// Contract + market data --------------------------------------------------

pub const REAL_TIME_BARS: i32 = 34;
pub const SNAPSHOT_MKT_DATA: i32 = 35;
pub const WHAT_IF_ORDERS: i32 = 36;
pub const CONTRACT_CONID: i32 = 37;
pub const DELTA_NEUTRAL: i32 = 40;
pub const CONTRACT_DATA_CHAIN: i32 = 40;
pub const SCALE_ORDERS2: i32 = 40;
pub const ALGO_ORDERS: i32 = 41;
pub const EXECUTION_DATA_CHAIN: i32 = 42;
pub const NOT_HELD: i32 = 44;
pub const SEC_ID_TYPE: i32 = 45;
pub const PLACE_ORDER_CONID: i32 = 46;
pub const REQ_MKT_DATA_CONID: i32 = 47;
pub const SSHORT_COMBO_LEGS: i32 = 35;
pub const SSHORTX_OLD: i32 = 51;
pub const SSHORTX: i32 = 52;
pub const REQ_GLOBAL_CANCEL: i32 = 53;
pub const HEDGE_ORDERS: i32 = 54;
pub const REQ_MARKET_DATA_TYPE: i32 = 55;
pub const OPT_OUT_SMART_ROUTING: i32 = 56;
pub const SMART_COMBO_ROUTING_PARAMS: i32 = 57;
pub const DELTA_NEUTRAL_CONID: i32 = 58;
pub const SCALE_ORDERS3: i32 = 60;
pub const ORDER_COMBO_LEGS_PRICE: i32 = 61;
pub const TRAILING_PERCENT: i32 = 62;
pub const DELTA_NEUTRAL_OPEN_CLOSE: i32 = 66;
pub const POSITIONS: i32 = 67;
pub const ACCOUNT_SUMMARY: i32 = 67;
pub const TRADING_CLASS: i32 = 68;
pub const SCALE_TABLE: i32 = 69;
pub const LINKING: i32 = 70;
pub const ALGO_ID: i32 = 71;
pub const OPTIONAL_CAPABILITIES: i32 = 72;
pub const ORDER_SOLICITED: i32 = 73;
pub const PRIMARYEXCH: i32 = 75;
pub const RANDOMIZE_SIZE_AND_PRICE: i32 = 76;
pub const FRACTIONAL_POSITIONS: i32 = 101;
pub const PEGGED_TO_BENCHMARK: i32 = 102;
pub const MODELS_SUPPORT: i32 = 103;
pub const SYNT_REALTIME_BARS: i32 = 124;
pub const EXT_OPERATOR: i32 = 105;
pub const SOFT_DOLLAR_TIER: i32 = 114;
pub const CASH_QTY: i32 = 111;
pub const DECISION_MAKER: i32 = 138;
pub const MIFID_EXECUTION: i32 = 139;
pub const AUTO_PRICE_FOR_HEDGE: i32 = 141;
pub const ORDER_CONTAINER: i32 = 145;
pub const D_PEG_ORDERS: i32 = 148;
pub const MKT_DEPTH_PRIM_EXCHANGE: i32 = 149;
pub const PRICE_MGMT_ALGO: i32 = 151;
pub const DURATION: i32 = 158;
pub const POST_TO_ATS: i32 = 160;
pub const AUTO_CANCEL_PARENT: i32 = 162;
pub const ADVANCED_ORDER_REJECT: i32 = 166;
pub const MANUAL_ORDER_TIME: i32 = 169;
pub const PEGBEST_PEGMID_OFFSETS: i32 = 170;
pub const BOND_ISSUERID: i32 = 176;
pub const FA_PROFILE_DESUPPORT: i32 = 177;
pub const REQ_SMART_COMPONENTS: i32 = 145;
pub const SCALE_ORDERS: i32 = 35;
pub const INCLUDE_EXPIRED_IN_REQ_CONTRACT_DATA: i32 = 31;

/// Sentinel used by `PEGBEST_PEGMID_OFFSETS` to flag "send mid offsets".
/// Mirrors `rust-ibapi`'s private constant of the same name.
pub const COMPETE_AGAINST_BEST_OFFSET_UP_TO_MID: f64 = f64::INFINITY;
