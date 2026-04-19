//! Client → sim message parser.
//!
//! `IncomingMsg::parse(frame, server_version)` decodes a [`RawFrame`] into
//! one of 17 typed variants listed in the deep-parity subset from
//! `plan/ib-sim/02-protocol-layer.md`. Field layouts mirror
//! [`rust-ibapi`](https://github.com/wboayue/rust-ibapi) v1.2.2's text
//! encoders (the last pre-protobuf release) plus per-server-version gates
//! for fields added after v176.
//!
//! This module does **not** speak any response logic — engine stages are
//! responsible for turning an `IncomingMsg` into `EngineCmd`s and replies.
//! The parser's only job is byte-accurate field reconstruction.

use crate::protocol::framing::RawFrame;
use crate::protocol::messages::fields::FieldReader;
use crate::protocol::messages::helpers::{
    parse_contract_combo_legs, parse_contract_for_historical_data, parse_contract_for_market_data,
    parse_contract_for_place_order, parse_contract_for_realtime_bars,
    parse_contract_for_req_contract_data, parse_delta_neutral_contract, parse_execution_filter,
    parse_order_combo_legs, parse_tag_value_list, parse_tag_value_string,
};
use crate::protocol::messages::server_versions as sv;
use crate::protocol::messages::types::{
    ContractSpec, ExecutionFilter, MarketDataType, OrderSpec, TagValue,
};
use crate::protocol::{ProtocolError, ServerVersion};

// ---------------------------------------------------------------------------
// Message ID table.
// ---------------------------------------------------------------------------

/// Client-originated message IDs we parse. Other IDs are rejected with
/// [`ProtocolError::UnsupportedMsgId`].
pub mod msg_id {
    pub const REQ_MKT_DATA: i32 = 1;
    pub const CANCEL_MKT_DATA: i32 = 2;
    pub const PLACE_ORDER: i32 = 3;
    pub const CANCEL_ORDER: i32 = 4;
    pub const REQ_OPEN_ORDERS: i32 = 5;
    pub const REQ_ACCOUNT_DATA: i32 = 6;
    pub const REQ_EXECUTIONS: i32 = 7;
    pub const REQ_IDS: i32 = 8;
    pub const REQ_CONTRACT_DATA: i32 = 9;
    pub const REQ_HISTORICAL_DATA: i32 = 20;
    pub const REQ_CURRENT_TIME: i32 = 49;
    pub const REQ_REAL_TIME_BARS: i32 = 50;
    pub const REQ_GLOBAL_CANCEL: i32 = 58;
    pub const REQ_MARKET_DATA_TYPE: i32 = 59;
    pub const REQ_POSITIONS: i32 = 61;
    pub const REQ_ACCOUNT_SUMMARY: i32 = 62;
    pub const START_API: i32 = 71;
}

// ---------------------------------------------------------------------------
// IncomingMsg — the typed client→sim variant.
// ---------------------------------------------------------------------------

/// A decoded client→sim message. The sim's engine matches on this enum to
/// produce an `EngineCmd`.
#[derive(Clone, Debug, PartialEq)]
pub enum IncomingMsg {
    StartApi {
        client_id: i32,
        /// `optionalCapabilities` is only present when server version >
        /// `OPTIONAL_CAPABILITIES` (72). We model it as `Option<String>` with
        /// `None` == "not sent".
        optional_caps: Option<String>,
    },
    ReqCurrentTime,
    ReqIds {
        num_ids: i32,
    },
    ReqContractData {
        req_id: i32,
        contract: ContractSpec,
    },
    ReqMktData {
        req_id: i32,
        contract: ContractSpec,
        generic_ticks: String,
        snapshot: bool,
        regulatory_snapshot: bool,
        opts: Vec<TagValue>,
    },
    CancelMktData {
        req_id: i32,
    },
    PlaceOrder {
        order_id: i32,
        contract: ContractSpec,
        /// Boxed to keep `IncomingMsg`'s size down — `OrderSpec` is 2 KiB+
        /// and dominates the enum when inlined.
        order: Box<OrderSpec>,
    },
    CancelOrder {
        order_id: i32,
        /// Present only when server version >= `MANUAL_ORDER_TIME` (169).
        manual_order_cancel_time: Option<String>,
    },
    ReqOpenOrders,
    ReqAccountData {
        subscribe: bool,
        acct_code: String,
    },
    ReqExecutions {
        req_id: i32,
        filter: ExecutionFilter,
    },
    ReqHistoricalData {
        req_id: i32,
        contract: ContractSpec,
        end_date_time: String,
        duration: String,
        bar_size: String,
        what_to_show: String,
        use_rth: bool,
        format_date: i32,
        keep_up_to_date: bool,
        chart_opts: Vec<TagValue>,
    },
    ReqRealTimeBars {
        req_id: i32,
        contract: ContractSpec,
        bar_size: i32,
        what_to_show: String,
        use_rth: bool,
        opts: Vec<TagValue>,
    },
    ReqMarketDataType {
        data_type: MarketDataType,
    },
    ReqPositions,
    ReqAccountSummary {
        req_id: i32,
        group: String,
        tags: String,
    },
    ReqGlobalCancel,
}

impl IncomingMsg {
    /// Parse a raw frame into a typed `IncomingMsg`. Returns
    /// [`ProtocolError::UnsupportedMsgId`] for message IDs outside the
    /// 17-variant subset.
    pub fn parse(frame: RawFrame, sv_: ServerVersion) -> Result<Self, ProtocolError> {
        let mut r = FieldReader::new(&frame.fields);
        let msg_id = r.read_i32()?;
        match msg_id {
            msg_id::REQ_MKT_DATA => parse_req_mkt_data(&mut r, sv_),
            msg_id::CANCEL_MKT_DATA => parse_cancel_mkt_data(&mut r),
            msg_id::PLACE_ORDER => parse_place_order(&mut r, sv_),
            msg_id::CANCEL_ORDER => parse_cancel_order(&mut r, sv_),
            msg_id::REQ_OPEN_ORDERS => parse_req_open_orders(&mut r),
            msg_id::REQ_ACCOUNT_DATA => parse_req_account_data(&mut r),
            msg_id::REQ_EXECUTIONS => parse_req_executions(&mut r, sv_),
            msg_id::REQ_IDS => parse_req_ids(&mut r),
            msg_id::REQ_CONTRACT_DATA => parse_req_contract_data(&mut r, sv_),
            msg_id::REQ_HISTORICAL_DATA => parse_req_historical_data(&mut r, sv_),
            msg_id::REQ_CURRENT_TIME => parse_req_current_time(&mut r),
            msg_id::REQ_REAL_TIME_BARS => parse_req_real_time_bars(&mut r, sv_),
            msg_id::REQ_GLOBAL_CANCEL => parse_req_global_cancel(&mut r),
            msg_id::REQ_MARKET_DATA_TYPE => parse_req_market_data_type(&mut r),
            msg_id::REQ_POSITIONS => parse_req_positions(&mut r),
            msg_id::REQ_ACCOUNT_SUMMARY => parse_req_account_summary(&mut r),
            msg_id::START_API => parse_start_api(&mut r, sv_),
            other => Err(ProtocolError::UnsupportedMsgId(other)),
        }
    }
}

// ---------------------------------------------------------------------------
// Per-message parsers.
// ---------------------------------------------------------------------------

/// `71 | 2 | <client_id> | [optional_caps]`
fn parse_start_api(
    r: &mut FieldReader<'_>,
    sv_: ServerVersion,
) -> Result<IncomingMsg, ProtocolError> {
    let _inner_version = r.read_i32()?;
    let client_id = r.read_i32()?;
    let optional_caps = if sv_.raw() >= sv::OPTIONAL_CAPABILITIES {
        // If the field is present but empty, treat as Some("").
        Some(r.read_string()?)
    } else {
        None
    };
    Ok(IncomingMsg::StartApi {
        client_id,
        optional_caps,
    })
}

/// `49 | 1` — current-time request has no payload beyond the header.
fn parse_req_current_time(r: &mut FieldReader<'_>) -> Result<IncomingMsg, ProtocolError> {
    let _inner_version = r.read_i32()?;
    Ok(IncomingMsg::ReqCurrentTime)
}

/// `8 | 1 | <num_ids>` — num_ids is always 0 for this message.
fn parse_req_ids(r: &mut FieldReader<'_>) -> Result<IncomingMsg, ProtocolError> {
    let _inner_version = r.read_i32()?;
    let num_ids = r.read_i32()?;
    Ok(IncomingMsg::ReqIds { num_ids })
}

/// `9 | 8 | [req_id?] | <contract...>`
fn parse_req_contract_data(
    r: &mut FieldReader<'_>,
    sv_: ServerVersion,
) -> Result<IncomingMsg, ProtocolError> {
    let _inner_version = r.read_i32()?;
    let req_id = if sv_.raw() >= sv::CONTRACT_DATA_CHAIN {
        r.read_i32()?
    } else {
        0
    };
    let contract = parse_contract_for_req_contract_data(r, sv_)?;
    Ok(IncomingMsg::ReqContractData { req_id, contract })
}

/// `1 | 11 | <req_id> | <contract...> | [legs] | [delta_neutral]
///          | <generic_ticks> | <snapshot> | [regulatory_snapshot] | <opts>`
fn parse_req_mkt_data(
    r: &mut FieldReader<'_>,
    sv_: ServerVersion,
) -> Result<IncomingMsg, ProtocolError> {
    let _inner_version = r.read_i32()?;
    let req_id = r.read_i32()?;
    let mut contract = parse_contract_for_market_data(r, sv_)?;

    // BAG legs: presence determined by security_type == "BAG".
    if contract.security_type == "BAG" {
        contract.combo_legs = {
            let n = r.read_i32()?;
            if n < 0 {
                return Err(ProtocolError::Field(format!(
                    "negative BAG-leg count in REQ_MKT_DATA: {n}"
                )));
            }
            let mut legs = Vec::with_capacity(n as usize);
            for _ in 0..n {
                legs.push(crate::protocol::messages::types::ComboLeg {
                    contract_id: r.read_i32()?,
                    ratio: r.read_i32()?,
                    action: r.read_string()?,
                    exchange: r.read_string()?,
                    ..Default::default()
                });
            }
            legs
        };
    }

    // Always present: the delta-neutral triplet gate-bool. When bool is false
    // the following three fields are absent.
    contract.delta_neutral_contract = parse_delta_neutral_contract(r)?;

    let generic_ticks = r.read_string()?;
    let snapshot = r.read_bool()?;
    let regulatory_snapshot = if sv_.raw() >= sv::REQ_SMART_COMPONENTS {
        r.read_bool()?
    } else {
        false
    };
    // Trailing misc-opts field (empty by convention; single string field).
    let opts = parse_tag_value_string(r)?;

    Ok(IncomingMsg::ReqMktData {
        req_id,
        contract,
        generic_ticks,
        snapshot,
        regulatory_snapshot,
        opts,
    })
}

/// `2 | 2 | <req_id>` — cancel market data.
fn parse_cancel_mkt_data(r: &mut FieldReader<'_>) -> Result<IncomingMsg, ProtocolError> {
    let _inner_version = r.read_i32()?;
    let req_id = r.read_i32()?;
    Ok(IncomingMsg::CancelMktData { req_id })
}

/// `4 | 1 | <order_id> | [manual_order_cancel_time]`
fn parse_cancel_order(
    r: &mut FieldReader<'_>,
    sv_: ServerVersion,
) -> Result<IncomingMsg, ProtocolError> {
    let _inner_version = r.read_i32()?;
    let order_id = r.read_i32()?;
    let manual_order_cancel_time = if sv_.raw() >= sv::MANUAL_ORDER_TIME {
        Some(r.read_string()?)
    } else {
        None
    };
    Ok(IncomingMsg::CancelOrder {
        order_id,
        manual_order_cancel_time,
    })
}

/// `5 | 1` — request open orders.
fn parse_req_open_orders(r: &mut FieldReader<'_>) -> Result<IncomingMsg, ProtocolError> {
    let _inner_version = r.read_i32()?;
    Ok(IncomingMsg::ReqOpenOrders)
}

/// `6 | 2 | <subscribe> | [acct_code]`
fn parse_req_account_data(r: &mut FieldReader<'_>) -> Result<IncomingMsg, ProtocolError> {
    let _inner_version = r.read_i32()?;
    let subscribe = r.read_bool()?;
    // acct_code may be an empty field when server version < 9. Since we
    // advertise v176+, always read it.
    let acct_code = r.read_string()?;
    Ok(IncomingMsg::ReqAccountData {
        subscribe,
        acct_code,
    })
}

/// `7 | 3 | [req_id?] | <filter (7 fields)>`
fn parse_req_executions(
    r: &mut FieldReader<'_>,
    sv_: ServerVersion,
) -> Result<IncomingMsg, ProtocolError> {
    let _inner_version = r.read_i32()?;
    let req_id = if sv_.raw() >= sv::EXECUTION_DATA_CHAIN {
        r.read_i32()?
    } else {
        0
    };
    let filter = parse_execution_filter(r)?;
    Ok(IncomingMsg::ReqExecutions { req_id, filter })
}

/// `58 | 1` — global cancel.
fn parse_req_global_cancel(r: &mut FieldReader<'_>) -> Result<IncomingMsg, ProtocolError> {
    let _inner_version = r.read_i32()?;
    Ok(IncomingMsg::ReqGlobalCancel)
}

/// `59 | 1 | <type>` — switch market data type.
fn parse_req_market_data_type(r: &mut FieldReader<'_>) -> Result<IncomingMsg, ProtocolError> {
    let _inner_version = r.read_i32()?;
    let raw = r.read_i32()?;
    let data_type = MarketDataType::from_i32(raw).ok_or_else(|| {
        ProtocolError::Field(format!("unknown MarketDataType discriminant: {raw}"))
    })?;
    Ok(IncomingMsg::ReqMarketDataType { data_type })
}

/// `61 | 1` — subscribe to all positions.
fn parse_req_positions(r: &mut FieldReader<'_>) -> Result<IncomingMsg, ProtocolError> {
    let _inner_version = r.read_i32()?;
    Ok(IncomingMsg::ReqPositions)
}

/// `62 | 1 | <req_id> | <group> | <tags>`
fn parse_req_account_summary(r: &mut FieldReader<'_>) -> Result<IncomingMsg, ProtocolError> {
    let _inner_version = r.read_i32()?;
    let req_id = r.read_i32()?;
    let group = r.read_string()?;
    let tags = r.read_string()?;
    Ok(IncomingMsg::ReqAccountSummary {
        req_id,
        group,
        tags,
    })
}

/// `50 | 8 | <req_id> | <contract...> | <bar_size> | <what_to_show>
///          | <use_rth> | [opts]`
fn parse_req_real_time_bars(
    r: &mut FieldReader<'_>,
    sv_: ServerVersion,
) -> Result<IncomingMsg, ProtocolError> {
    let _inner_version = r.read_i32()?;
    let req_id = r.read_i32()?;
    let contract = parse_contract_for_realtime_bars(r, sv_)?;
    let bar_size = r.read_i32()?;
    let what_to_show = r.read_string()?;
    let use_rth = r.read_bool()?;
    let opts = if sv_.raw() >= sv::LINKING {
        parse_tag_value_string(r)?
    } else {
        Vec::new()
    };
    Ok(IncomingMsg::ReqRealTimeBars {
        req_id,
        contract,
        bar_size,
        what_to_show,
        use_rth,
        opts,
    })
}

/// `20 | [6?] | <req_id> | <contract...> | <end_date_time> | <bar_size>
///          | <duration> | <use_rth> | <what_to_show> | <format_date>
///          | [BAG legs] | <keep_up_to_date?> | [chart_opts]`
fn parse_req_historical_data(
    r: &mut FieldReader<'_>,
    sv_: ServerVersion,
) -> Result<IncomingMsg, ProtocolError> {
    let version = sv_.raw();

    if version < sv::SYNT_REALTIME_BARS {
        // Legacy header included an explicit inner-version field.
        let _inner_version = r.read_i32()?;
    }
    let req_id = r.read_i32()?;
    let mut contract = parse_contract_for_historical_data(r, sv_)?;

    let end_date_time = r.read_string()?;
    let bar_size = r.read_string()?;
    let duration = r.read_string()?;
    let use_rth = r.read_bool()?;
    let what_to_show = r.read_string()?;
    let format_date = r.read_i32()?;

    // BAG legs block — same 4-field shape as REQ_MKT_DATA.
    if contract.security_type == "BAG" {
        let n = r.read_i32()?;
        if n < 0 {
            return Err(ProtocolError::Field(format!(
                "negative BAG-leg count in REQ_HISTORICAL_DATA: {n}"
            )));
        }
        let mut legs = Vec::with_capacity(n as usize);
        for _ in 0..n {
            legs.push(crate::protocol::messages::types::ComboLeg {
                contract_id: r.read_i32()?,
                ratio: r.read_i32()?,
                action: r.read_string()?,
                exchange: r.read_string()?,
                ..Default::default()
            });
        }
        contract.combo_legs = legs;
    }

    let keep_up_to_date = if version >= sv::SYNT_REALTIME_BARS {
        r.read_bool()?
    } else {
        false
    };
    let chart_opts = if version >= sv::LINKING {
        parse_tag_value_string(r)?
    } else {
        Vec::new()
    };

    Ok(IncomingMsg::ReqHistoricalData {
        req_id,
        contract,
        end_date_time,
        duration,
        bar_size,
        what_to_show,
        use_rth,
        format_date,
        keep_up_to_date,
        chart_opts,
    })
}

// ---------------------------------------------------------------------------
// PLACE_ORDER — the largest parser in this module, mirrors
// `rust-ibapi` v1.2.2 `src/orders/encoders.rs::encode_place_order` exactly.
// ---------------------------------------------------------------------------

fn parse_place_order(
    r: &mut FieldReader<'_>,
    sv_: ServerVersion,
) -> Result<IncomingMsg, ProtocolError> {
    let version = sv_.raw();

    // Inner version is only present for server_version < ORDER_CONTAINER.
    if version < sv::ORDER_CONTAINER {
        let _inner_version = r.read_i32()?;
    }

    let order_id = r.read_i32()?;
    let mut contract = parse_contract_for_place_order(r, sv_)?;
    let mut o = OrderSpec {
        action: r.read_string()?,
        total_quantity: if version >= sv::FRACTIONAL_POSITIONS {
            r.read_f64()?
        } else {
            r.read_i32()? as f64
        },
        order_type: r.read_string()?,
        // Both `limit_price` and `aux_price` have unconditional `read_opt_f64`
        // behaviour at our supported range — MIN=176 is well above
        // `ORDER_COMBO_LEGS_PRICE (61)` and `TRAILING_PERCENT (62)`. The old
        // conditional arms were identity copies with a misleading comment; a
        // debug-assert guards the invariant instead.
        limit_price: {
            debug_assert!(version >= sv::ORDER_COMBO_LEGS_PRICE);
            r.read_opt_f64()?
        },
        aux_price: {
            debug_assert!(version >= sv::TRAILING_PERCENT);
            r.read_opt_f64()?
        },
        ..Default::default()
    };

    // Extended order fields.
    o.tif = r.read_string()?;
    o.oca_group = r.read_string()?;
    o.account = r.read_string()?;
    o.open_close = r.read_string()?;
    o.origin = r.read_i32()?;
    o.order_ref = r.read_string()?;
    o.transmit = r.read_bool()?;
    o.parent_id = r.read_i32()?;

    o.block_order = r.read_bool()?;
    o.sweep_to_fill = r.read_bool()?;
    o.display_size = r.read_i32()?;
    o.trigger_method = r.read_i32()?;
    o.outside_rth = r.read_bool()?;

    o.hidden = r.read_bool()?;

    // BAG combo legs (inside contract).
    if contract.security_type == "BAG" {
        contract.combo_legs = parse_contract_combo_legs(r, sv_)?;

        if version >= sv::ORDER_COMBO_LEGS_PRICE {
            o.order_combo_legs = parse_order_combo_legs(r)?;
        }

        if version >= sv::SMART_COMBO_ROUTING_PARAMS {
            o.smart_combo_routing_params = parse_tag_value_list(r)?;
        }
    }

    // Deprecated shares-allocation field (always empty).
    let _shares_allocation = r.read_string()?;

    o.discretionary_amt = r.read_f64()?;
    o.good_after_time = r.read_string()?;
    o.good_till_date = r.read_string()?;

    o.fa_group = r.read_string()?;
    o.fa_method = r.read_string()?;
    o.fa_percentage = r.read_string()?;
    if version < sv::FA_PROFILE_DESUPPORT {
        o.fa_profile = r.read_string()?;
    }

    if version >= sv::MODELS_SUPPORT {
        o.model_code = r.read_string()?;
    }

    o.short_sale_slot = r.read_i32()?;
    o.designated_location = r.read_string()?;

    if version >= sv::SSHORTX_OLD {
        o.exempt_code = r.read_i32()?;
    }

    o.oca_type = r.read_i32()?;
    o.rule_80_a = r.read_string()?;
    o.settling_firm = r.read_string()?;
    o.all_or_none = r.read_bool()?;
    o.min_qty = r.read_opt_i32()?;
    o.percent_offset = r.read_opt_f64()?;
    o.e_trade_only = r.read_bool()?;
    o.firm_quote_only = r.read_bool()?;
    o.nbbo_price_cap = r.read_opt_f64()?;
    o.auction_strategy = r.read_i32()?;
    o.starting_price = r.read_opt_f64()?;
    o.stock_ref_price = r.read_opt_f64()?;
    o.delta = r.read_opt_f64()?;
    o.stock_range_lower = r.read_opt_f64()?;
    o.stock_range_upper = r.read_opt_f64()?;

    o.override_percentage_constraints = r.read_bool()?;

    // Volatility orders.
    o.volatility = r.read_opt_f64()?;
    o.volatility_type = r.read_opt_i32()?;
    o.delta_neutral_order_type = r.read_string()?;
    o.delta_neutral_aux_price = r.read_opt_f64()?;

    let delta_neutral_present = !o.delta_neutral_order_type.is_empty();
    if version >= sv::DELTA_NEUTRAL_CONID && delta_neutral_present {
        o.delta_neutral_con_id = r.read_i32()?;
        o.delta_neutral_settling_firm = r.read_string()?;
        o.delta_neutral_clearing_account = r.read_string()?;
        o.delta_neutral_clearing_intent = r.read_string()?;
    }
    if version >= sv::DELTA_NEUTRAL_OPEN_CLOSE && delta_neutral_present {
        o.delta_neutral_open_close = r.read_string()?;
        o.delta_neutral_short_sale = r.read_bool()?;
        o.delta_neutral_short_sale_slot = r.read_i32()?;
        o.delta_neutral_designated_location = r.read_string()?;
    }

    o.continuous_update = r.read_bool()?;
    o.reference_price_type = r.read_opt_i32()?;

    o.trail_stop_price = r.read_opt_f64()?;
    if version >= sv::TRAILING_PERCENT {
        o.trailing_percent = r.read_opt_f64()?;
    }

    // Scale orders.
    if version >= sv::SCALE_ORDERS {
        if version >= sv::SCALE_ORDERS2 {
            o.scale_init_level_size = r.read_opt_i32()?;
            o.scale_subs_level_size = r.read_opt_i32()?;
        } else {
            // Pre-SCALE_ORDERS2: dummy empty string + init_level_size.
            let _dummy = r.read_string()?;
            o.scale_init_level_size = r.read_opt_i32()?;
        }
        o.scale_price_increment = r.read_opt_f64()?;
    }

    let scale_order = matches!(o.scale_price_increment, Some(v) if v > 0.0 && v != f64::MAX);
    if version >= sv::SCALE_ORDERS3 && scale_order {
        o.scale_price_adjust_value = r.read_opt_f64()?;
        o.scale_price_adjust_interval = r.read_opt_i32()?;
        o.scale_profit_offset = r.read_opt_f64()?;
        o.scale_auto_reset = r.read_bool()?;
        o.scale_init_position = r.read_opt_i32()?;
        o.scale_init_fill_qty = r.read_opt_i32()?;
        o.scale_random_percent = r.read_bool()?;
    }

    if version >= sv::SCALE_TABLE {
        o.scale_table = r.read_string()?;
        o.active_start_time = r.read_string()?;
        o.active_stop_time = r.read_string()?;
    }

    if version >= sv::HEDGE_ORDERS {
        o.hedge_type = r.read_string()?;
        if !o.hedge_type.is_empty() {
            o.hedge_param = r.read_string()?;
        }
    }

    if version >= sv::OPT_OUT_SMART_ROUTING {
        o.opt_out_smart_routing = r.read_bool()?;
    }

    if version >= 39 {
        // PTA_ORDERS gate.
        o.clearing_account = r.read_string()?;
        o.clearing_intent = r.read_string()?;
    }

    if version >= sv::NOT_HELD {
        o.not_held = r.read_bool()?;
    }

    if version >= sv::DELTA_NEUTRAL {
        contract.delta_neutral_contract = parse_delta_neutral_contract(r)?;
    }

    if version >= sv::ALGO_ORDERS {
        o.algo_strategy = r.read_string()?;
        if !o.algo_strategy.is_empty() {
            o.algo_params = parse_tag_value_list(r)?;
        }
    }

    if version >= sv::ALGO_ID {
        o.algo_id = r.read_string()?;
    }

    if version >= sv::WHAT_IF_ORDERS {
        o.what_if = r.read_bool()?;
    }

    if version >= sv::LINKING {
        o.order_misc_options = parse_tag_value_string(r)?;
    }

    if version >= sv::ORDER_SOLICITED {
        o.solicited = r.read_bool()?;
    }

    if version >= sv::RANDOMIZE_SIZE_AND_PRICE {
        o.randomize_size = r.read_bool()?;
        o.randomize_price = r.read_bool()?;
    }

    // Pegged-to-benchmark + conditions block.
    if version >= sv::PEGGED_TO_BENCHMARK {
        if o.order_type == "PEG BENCH" {
            o.reference_contract_id = r.read_i32()?;
            o.is_pegged_change_amount_decrease = r.read_bool()?;
            o.pegged_change_amount = r.read_opt_f64()?;
            o.reference_change_amount = r.read_opt_f64()?;
            o.reference_exchange = r.read_string()?;
        }
        let n_conditions = r.read_i32()?;
        if n_conditions < 0 {
            return Err(ProtocolError::Field(format!(
                "negative condition count: {n_conditions}"
            )));
        }
        if n_conditions > 0 {
            // Each condition is a pre-serialised string blob — the TWS
            // protocol nests a length-prefixed struct, but because we don't
            // decode the inner shape we read them as single fields. This is
            // sufficient for byte-roundtrip when the encoder mirrors it.
            for _ in 0..n_conditions {
                o.conditions.push(r.read_string()?);
            }
            o.conditions_ignore_rth = r.read_bool()?;
            o.conditions_cancel_order = r.read_bool()?;
        }

        o.adjusted_order_type = r.read_string()?;
        o.trigger_price = r.read_opt_f64()?;
        o.limit_price_offset = r.read_opt_f64()?;
        o.adjusted_stop_price = r.read_opt_f64()?;
        o.adjusted_stop_limit_price = r.read_opt_f64()?;
        o.adjusted_trailing_amount = r.read_opt_f64()?;
        o.adjustable_trailing_unit = r.read_i32()?;
    }

    if version >= sv::EXT_OPERATOR {
        o.ext_operator = r.read_string()?;
    }

    if version >= sv::SOFT_DOLLAR_TIER {
        o.soft_dollar_tier_name = r.read_string()?;
        o.soft_dollar_tier_value = r.read_string()?;
    }

    if version >= sv::CASH_QTY {
        o.cash_qty = r.read_opt_f64()?;
    }

    if version >= sv::DECISION_MAKER {
        o.mifid2_decision_maker = r.read_string()?;
        o.mifid2_decision_algo = r.read_string()?;
    }

    if version >= sv::MIFID_EXECUTION {
        o.mifid2_execution_trader = r.read_string()?;
        o.mifid2_execution_algo = r.read_string()?;
    }

    if version >= sv::AUTO_PRICE_FOR_HEDGE {
        o.dont_use_auto_price_for_hedge = r.read_bool()?;
    }

    if version >= sv::ORDER_CONTAINER {
        o.is_oms_container = r.read_bool()?;
    }

    if version >= sv::D_PEG_ORDERS {
        o.discretionary_up_to_limit_price = r.read_bool()?;
    }

    if version >= sv::PRICE_MGMT_ALGO {
        // IB transmits this as an Option<bool> via the sentinel: empty = None,
        // "0" = Some(false), "1" = Some(true). Map accordingly.
        let raw = r.read_string()?;
        o.use_price_mgmt_algo = match raw.as_str() {
            "" => None,
            "0" => Some(false),
            "1" => Some(true),
            _ => {
                return Err(ProtocolError::Field(format!(
                    "invalid use_price_mgmt_algo: {raw:?}"
                )))
            }
        };
    }

    if version >= sv::DURATION {
        o.duration = r.read_opt_i32()?;
    }

    if version >= sv::POST_TO_ATS {
        o.post_to_ats = r.read_opt_i32()?;
    }

    if version >= sv::AUTO_CANCEL_PARENT {
        o.auto_cancel_parent = r.read_bool()?;
    }

    if version >= sv::ADVANCED_ORDER_REJECT {
        o.advanced_error_override = r.read_string()?;
    }

    if version >= sv::MANUAL_ORDER_TIME {
        o.manual_order_time = r.read_string()?;
    }

    if version >= sv::PEGBEST_PEGMID_OFFSETS {
        if contract.exchange == "IBKRATS" {
            o.min_trade_qty = r.read_opt_i32()?;
        }
        let mut send_mid_offsets = false;
        if o.order_type == "PEG BEST" {
            o.min_compete_size = r.read_opt_i32()?;
            o.compete_against_best_offset = r.read_opt_f64()?;
            if matches!(o.compete_against_best_offset, Some(v) if v.is_infinite()) {
                send_mid_offsets = true;
            }
        } else if o.order_type == "PEG MID" {
            send_mid_offsets = true;
        }
        if send_mid_offsets {
            o.mid_offset_at_whole = r.read_opt_f64()?;
            o.mid_offset_at_half = r.read_opt_f64()?;
        }
    }

    Ok(IncomingMsg::PlaceOrder {
        order_id,
        contract,
        order: Box::new(o),
    })
}
