//! Test-only encoder that inverts [`IncomingMsg::parse`].
//!
//! Stage 02b ships the parser only; the production outbound encoder lives
//! in Stage 02c. This module exists so that the 02b test suite can build
//! roundtrip fixtures: `encode(msg) -> bytes -> parse(bytes) == msg`.
//!
//! **Not a complete client.** The encoder mirrors the parser field-for-field
//! at a given `ServerVersion`; it has no notion of "the rust-ibapi client
//! would have omitted this" nor does it optimise empty fields away.

#![cfg(test)]

use crate::protocol::framing::RawFrame;
use crate::protocol::messages::fields::FieldWriter;
use crate::protocol::messages::helpers;
use crate::protocol::messages::incoming::{msg_id, IncomingMsg};
use crate::protocol::messages::server_versions as sv;
use crate::protocol::messages::types::{
    ComboLeg, ContractSpec, DeltaNeutralContract, ExecutionFilter, OrderComboLeg, OrderSpec,
    TagValue,
};
use crate::protocol::ServerVersion;

// ---------------------------------------------------------------------------
// Helpers — shared writers.
// ---------------------------------------------------------------------------

fn write_tag_value_list(w: &mut FieldWriter, tvs: &[TagValue]) {
    w.write_i32(tvs.len() as i32);
    for tv in tvs {
        w.write_string(&tv.tag);
        w.write_string(&tv.value);
    }
}

fn write_tag_value_string(w: &mut FieldWriter, tvs: &[TagValue]) {
    if tvs.is_empty() {
        w.write_string("");
        return;
    }
    let mut s = String::new();
    for tv in tvs {
        s.push_str(&tv.tag);
        s.push('=');
        s.push_str(&tv.value);
        s.push(';');
    }
    w.write_string(&s);
}

fn write_execution_filter(w: &mut FieldWriter, f: &ExecutionFilter) {
    w.write_i32(f.client_id);
    w.write_string(&f.acct_code);
    w.write_string(&f.time);
    w.write_string(&f.symbol);
    w.write_string(&f.sec_type);
    w.write_string(&f.exchange);
    w.write_string(&f.side);
}

fn write_combo_legs_for_place_order(w: &mut FieldWriter, legs: &[ComboLeg], sv_: ServerVersion) {
    w.write_i32(legs.len() as i32);
    for leg in legs {
        w.write_i32(leg.contract_id);
        w.write_i32(leg.ratio);
        w.write_string(&leg.action);
        w.write_string(&leg.exchange);
        w.write_i32(leg.open_close);
        if sv_.raw() >= sv::SSHORT_COMBO_LEGS {
            w.write_i32(leg.short_sale_slot);
            w.write_string(&leg.designated_location);
        }
        if sv_.raw() >= sv::SSHORTX_OLD {
            w.write_i32(leg.exempt_code);
        }
    }
}

fn write_order_combo_legs(w: &mut FieldWriter, legs: &[OrderComboLeg]) {
    w.write_i32(legs.len() as i32);
    for leg in legs {
        w.write_opt_f64(leg.price);
    }
}

fn write_delta_neutral(w: &mut FieldWriter, dn: &Option<DeltaNeutralContract>) {
    match dn {
        Some(x) => {
            w.write_bool(true);
            w.write_i32(x.contract_id);
            w.write_f64(x.delta);
            w.write_f64(x.price);
        }
        None => {
            w.write_bool(false);
        }
    }
}

fn write_mkt_data_bag_legs(w: &mut FieldWriter, legs: &[ComboLeg]) {
    w.write_i32(legs.len() as i32);
    for leg in legs {
        w.write_i32(leg.contract_id);
        w.write_i32(leg.ratio);
        w.write_string(&leg.action);
        w.write_string(&leg.exchange);
    }
}

// ---------------------------------------------------------------------------
// Contract writers — mirror each parse_contract_for_X exactly.
// ---------------------------------------------------------------------------

fn write_contract_for_place_order(w: &mut FieldWriter, c: &ContractSpec, sv_: ServerVersion) {
    let version = sv_.raw();
    if version >= sv::PLACE_ORDER_CONID {
        w.write_i32(c.contract_id);
    }
    w.write_string(&c.symbol);
    w.write_string(&c.security_type);
    w.write_string(&c.last_trade_date_or_contract_month);
    w.write_f64(c.strike);
    w.write_string(&c.right);
    w.write_string(&c.multiplier);
    w.write_string(&c.exchange);
    w.write_string(&c.primary_exchange);
    w.write_string(&c.currency);
    w.write_string(&c.local_symbol);
    if version >= sv::TRADING_CLASS {
        w.write_string(&c.trading_class);
    }
    if version >= sv::SEC_ID_TYPE {
        w.write_string(&c.security_id_type);
        w.write_string(&c.security_id);
    }
}

fn write_contract_for_req_contract_data(w: &mut FieldWriter, c: &ContractSpec, sv_: ServerVersion) {
    let version = sv_.raw();
    if version >= sv::CONTRACT_CONID {
        w.write_i32(c.contract_id);
    }
    w.write_string(&c.symbol);
    w.write_string(&c.security_type);
    w.write_string(&c.last_trade_date_or_contract_month);
    w.write_f64(c.strike);
    w.write_string(&c.right);
    if version >= 15 {
        w.write_string(&c.multiplier);
    }
    if version >= sv::PRIMARYEXCH {
        w.write_string(&c.exchange);
        w.write_string(&c.primary_exchange);
    } else if version >= sv::LINKING {
        // Encode fused "EXCHANGE:PRIMARY" when appropriate.
        if !c.primary_exchange.is_empty() && (c.exchange == "BEST" || c.exchange == "SMART") {
            w.write_string(&format!("{}:{}", c.exchange, c.primary_exchange));
        } else {
            w.write_string(&c.exchange);
        }
    }
    w.write_string(&c.currency);
    w.write_string(&c.local_symbol);
    if version >= sv::TRADING_CLASS {
        w.write_string(&c.trading_class);
    }
    if version >= sv::INCLUDE_EXPIRED_IN_REQ_CONTRACT_DATA {
        w.write_bool(c.include_expired);
    }
    if version >= sv::SEC_ID_TYPE {
        w.write_string(&c.security_id_type);
        w.write_string(&c.security_id);
    }
    if version >= sv::BOND_ISSUERID {
        w.write_string(&c.issuer_id);
    }
}

fn write_contract_for_market_data(w: &mut FieldWriter, c: &ContractSpec, sv_: ServerVersion) {
    let version = sv_.raw();
    w.write_i32(c.contract_id);
    w.write_string(&c.symbol);
    w.write_string(&c.security_type);
    w.write_string(&c.last_trade_date_or_contract_month);
    w.write_f64(c.strike);
    w.write_string(&c.right);
    w.write_string(&c.multiplier);
    w.write_string(&c.exchange);
    w.write_string(&c.primary_exchange);
    w.write_string(&c.currency);
    w.write_string(&c.local_symbol);
    if version >= sv::TRADING_CLASS {
        w.write_string(&c.trading_class);
    }
}

fn write_contract_for_historical_data(w: &mut FieldWriter, c: &ContractSpec, sv_: ServerVersion) {
    let version = sv_.raw();
    if version >= sv::TRADING_CLASS {
        w.write_i32(c.contract_id);
    }
    w.write_string(&c.symbol);
    w.write_string(&c.security_type);
    w.write_string(&c.last_trade_date_or_contract_month);
    w.write_f64(c.strike);
    w.write_string(&c.right);
    w.write_string(&c.multiplier);
    w.write_string(&c.exchange);
    w.write_string(&c.primary_exchange);
    w.write_string(&c.currency);
    w.write_string(&c.local_symbol);
    if version >= sv::TRADING_CLASS {
        w.write_string(&c.trading_class);
    }
    w.write_bool(c.include_expired);
}

fn write_contract_for_realtime_bars(w: &mut FieldWriter, c: &ContractSpec, sv_: ServerVersion) {
    let version = sv_.raw();
    if version >= sv::TRADING_CLASS {
        w.write_i32(c.contract_id);
    }
    w.write_string(&c.symbol);
    w.write_string(&c.security_type);
    w.write_string(&c.last_trade_date_or_contract_month);
    w.write_f64(c.strike);
    w.write_string(&c.right);
    w.write_string(&c.multiplier);
    w.write_string(&c.exchange);
    w.write_string(&c.primary_exchange);
    w.write_string(&c.currency);
    w.write_string(&c.local_symbol);
    if version >= sv::TRADING_CLASS {
        w.write_string(&c.trading_class);
    }
}

// ---------------------------------------------------------------------------
// Public entry point for tests.
// ---------------------------------------------------------------------------

pub(crate) fn encode_incoming(msg: &IncomingMsg, sv_: ServerVersion) -> RawFrame {
    let mut w = FieldWriter::new();
    match msg {
        IncomingMsg::StartApi {
            client_id,
            optional_caps,
        } => {
            w.write_i32(msg_id::START_API);
            w.write_i32(2);
            w.write_i32(*client_id);
            if sv_.raw() > sv::OPTIONAL_CAPABILITIES {
                w.write_string(optional_caps.as_deref().unwrap_or(""));
            }
        }
        IncomingMsg::ReqCurrentTime => {
            w.write_i32(msg_id::REQ_CURRENT_TIME);
            w.write_i32(1);
        }
        IncomingMsg::ReqIds { num_ids } => {
            w.write_i32(msg_id::REQ_IDS);
            w.write_i32(1);
            w.write_i32(*num_ids);
        }
        IncomingMsg::ReqContractData { req_id, contract } => {
            w.write_i32(msg_id::REQ_CONTRACT_DATA);
            w.write_i32(8);
            if sv_.raw() >= sv::CONTRACT_DATA_CHAIN {
                w.write_i32(*req_id);
            }
            write_contract_for_req_contract_data(&mut w, contract, sv_);
        }
        IncomingMsg::ReqMktData {
            req_id,
            contract,
            generic_ticks,
            snapshot,
            regulatory_snapshot,
            opts,
        } => {
            w.write_i32(msg_id::REQ_MKT_DATA);
            w.write_i32(11);
            w.write_i32(*req_id);
            write_contract_for_market_data(&mut w, contract, sv_);
            if contract.security_type == "BAG" {
                write_mkt_data_bag_legs(&mut w, &contract.combo_legs);
            }
            write_delta_neutral(&mut w, &contract.delta_neutral_contract);
            w.write_string(generic_ticks);
            w.write_bool(*snapshot);
            if sv_.raw() >= sv::REQ_SMART_COMPONENTS {
                w.write_bool(*regulatory_snapshot);
            }
            write_tag_value_string(&mut w, opts);
        }
        IncomingMsg::CancelMktData { req_id } => {
            w.write_i32(msg_id::CANCEL_MKT_DATA);
            w.write_i32(2);
            w.write_i32(*req_id);
        }
        IncomingMsg::PlaceOrder {
            order_id,
            contract,
            order,
        } => {
            write_place_order(&mut w, *order_id, contract, order.as_ref(), sv_);
        }
        IncomingMsg::CancelOrder {
            order_id,
            manual_order_cancel_time,
        } => {
            w.write_i32(msg_id::CANCEL_ORDER);
            w.write_i32(1);
            w.write_i32(*order_id);
            if sv_.raw() >= sv::MANUAL_ORDER_TIME {
                w.write_string(manual_order_cancel_time.as_deref().unwrap_or(""));
            }
        }
        IncomingMsg::ReqOpenOrders => {
            w.write_i32(msg_id::REQ_OPEN_ORDERS);
            w.write_i32(1);
        }
        IncomingMsg::ReqAccountData {
            subscribe,
            acct_code,
        } => {
            w.write_i32(msg_id::REQ_ACCOUNT_DATA);
            w.write_i32(2);
            w.write_bool(*subscribe);
            w.write_string(acct_code);
        }
        IncomingMsg::ReqExecutions { req_id, filter } => {
            w.write_i32(msg_id::REQ_EXECUTIONS);
            w.write_i32(3);
            if sv_.raw() >= sv::EXECUTION_DATA_CHAIN {
                w.write_i32(*req_id);
            }
            write_execution_filter(&mut w, filter);
        }
        IncomingMsg::ReqHistoricalData {
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
        } => {
            w.write_i32(msg_id::REQ_HISTORICAL_DATA);
            if sv_.raw() < sv::SYNT_REALTIME_BARS {
                w.write_i32(6);
            }
            w.write_i32(*req_id);
            write_contract_for_historical_data(&mut w, contract, sv_);
            w.write_string(end_date_time);
            w.write_string(bar_size);
            w.write_string(duration);
            w.write_bool(*use_rth);
            w.write_string(what_to_show);
            w.write_i32(*format_date);
            if contract.security_type == "BAG" {
                write_mkt_data_bag_legs(&mut w, &contract.combo_legs);
            }
            if sv_.raw() >= sv::SYNT_REALTIME_BARS {
                w.write_bool(*keep_up_to_date);
            }
            if sv_.raw() >= sv::LINKING {
                write_tag_value_string(&mut w, chart_opts);
            }
        }
        IncomingMsg::ReqRealTimeBars {
            req_id,
            contract,
            bar_size,
            what_to_show,
            use_rth,
            opts,
        } => {
            w.write_i32(msg_id::REQ_REAL_TIME_BARS);
            w.write_i32(8);
            w.write_i32(*req_id);
            write_contract_for_realtime_bars(&mut w, contract, sv_);
            w.write_i32(*bar_size);
            w.write_string(what_to_show);
            w.write_bool(*use_rth);
            if sv_.raw() >= sv::LINKING {
                write_tag_value_string(&mut w, opts);
            }
        }
        IncomingMsg::ReqMarketDataType { data_type } => {
            w.write_i32(msg_id::REQ_MARKET_DATA_TYPE);
            w.write_i32(1);
            w.write_i32(data_type.as_i32());
        }
        IncomingMsg::ReqPositions => {
            w.write_i32(msg_id::REQ_POSITIONS);
            w.write_i32(1);
        }
        IncomingMsg::ReqAccountSummary {
            req_id,
            group,
            tags,
        } => {
            w.write_i32(msg_id::REQ_ACCOUNT_SUMMARY);
            w.write_i32(1);
            w.write_i32(*req_id);
            w.write_string(group);
            w.write_string(tags);
        }
        IncomingMsg::ReqGlobalCancel => {
            w.write_i32(msg_id::REQ_GLOBAL_CANCEL);
            w.write_i32(1);
        }
    }
    let fields = w.into_fields();
    RawFrame { fields }
}

// ---------------------------------------------------------------------------
// PLACE_ORDER — mirror of parse_place_order.
// ---------------------------------------------------------------------------

fn write_place_order(
    w: &mut FieldWriter,
    order_id: i32,
    contract: &ContractSpec,
    o: &OrderSpec,
    sv_: ServerVersion,
) {
    let version = sv_.raw();
    w.write_i32(msg_id::PLACE_ORDER);
    if version < sv::ORDER_CONTAINER {
        w.write_i32(45);
    }
    w.write_i32(order_id);
    write_contract_for_place_order(w, contract, sv_);

    w.write_string(&o.action);
    if version >= sv::FRACTIONAL_POSITIONS {
        w.write_f64(o.total_quantity);
    } else {
        w.write_i32(o.total_quantity as i32);
    }
    w.write_string(&o.order_type);
    w.write_opt_f64(o.limit_price);
    w.write_opt_f64(o.aux_price);

    w.write_string(&o.tif);
    w.write_string(&o.oca_group);
    w.write_string(&o.account);
    w.write_string(&o.open_close);
    w.write_i32(o.origin);
    w.write_string(&o.order_ref);
    w.write_bool(o.transmit);
    w.write_i32(o.parent_id);

    w.write_bool(o.block_order);
    w.write_bool(o.sweep_to_fill);
    w.write_i32(o.display_size);
    w.write_i32(o.trigger_method);
    w.write_bool(o.outside_rth);
    w.write_bool(o.hidden);

    if contract.security_type == "BAG" {
        write_combo_legs_for_place_order(w, &contract.combo_legs, sv_);
        if version >= sv::ORDER_COMBO_LEGS_PRICE {
            write_order_combo_legs(w, &o.order_combo_legs);
        }
        if version >= sv::SMART_COMBO_ROUTING_PARAMS {
            write_tag_value_list(w, &o.smart_combo_routing_params);
        }
    }

    // deprecated shares_allocation
    w.write_string("");

    w.write_f64(o.discretionary_amt);
    w.write_string(&o.good_after_time);
    w.write_string(&o.good_till_date);

    w.write_string(&o.fa_group);
    w.write_string(&o.fa_method);
    w.write_string(&o.fa_percentage);
    if version < sv::FA_PROFILE_DESUPPORT {
        w.write_string(&o.fa_profile);
    }
    if version >= sv::MODELS_SUPPORT {
        w.write_string(&o.model_code);
    }

    w.write_i32(o.short_sale_slot);
    w.write_string(&o.designated_location);
    if version >= sv::SSHORTX_OLD {
        w.write_i32(o.exempt_code);
    }

    w.write_i32(o.oca_type);
    w.write_string(&o.rule_80_a);
    w.write_string(&o.settling_firm);
    w.write_bool(o.all_or_none);
    w.write_opt_i32(o.min_qty);
    w.write_opt_f64(o.percent_offset);
    w.write_bool(o.e_trade_only);
    w.write_bool(o.firm_quote_only);
    w.write_opt_f64(o.nbbo_price_cap);
    w.write_i32(o.auction_strategy);
    w.write_opt_f64(o.starting_price);
    w.write_opt_f64(o.stock_ref_price);
    w.write_opt_f64(o.delta);
    w.write_opt_f64(o.stock_range_lower);
    w.write_opt_f64(o.stock_range_upper);

    w.write_bool(o.override_percentage_constraints);

    w.write_opt_f64(o.volatility);
    w.write_opt_i32(o.volatility_type);
    w.write_string(&o.delta_neutral_order_type);
    w.write_opt_f64(o.delta_neutral_aux_price);

    let delta_neutral_present = !o.delta_neutral_order_type.is_empty();
    if version >= sv::DELTA_NEUTRAL_CONID && delta_neutral_present {
        w.write_i32(o.delta_neutral_con_id);
        w.write_string(&o.delta_neutral_settling_firm);
        w.write_string(&o.delta_neutral_clearing_account);
        w.write_string(&o.delta_neutral_clearing_intent);
    }
    if version >= sv::DELTA_NEUTRAL_OPEN_CLOSE && delta_neutral_present {
        w.write_string(&o.delta_neutral_open_close);
        w.write_bool(o.delta_neutral_short_sale);
        w.write_i32(o.delta_neutral_short_sale_slot);
        w.write_string(&o.delta_neutral_designated_location);
    }

    w.write_bool(o.continuous_update);
    w.write_opt_i32(o.reference_price_type);

    w.write_opt_f64(o.trail_stop_price);
    if version >= sv::TRAILING_PERCENT {
        w.write_opt_f64(o.trailing_percent);
    }

    if version >= sv::SCALE_ORDERS {
        if version >= sv::SCALE_ORDERS2 {
            w.write_opt_i32(o.scale_init_level_size);
            w.write_opt_i32(o.scale_subs_level_size);
        } else {
            w.write_string("");
            w.write_opt_i32(o.scale_init_level_size);
        }
        w.write_opt_f64(o.scale_price_increment);
    }

    let scale_order = matches!(o.scale_price_increment, Some(v) if v > 0.0 && v != f64::MAX);
    if version >= sv::SCALE_ORDERS3 && scale_order {
        w.write_opt_f64(o.scale_price_adjust_value);
        w.write_opt_i32(o.scale_price_adjust_interval);
        w.write_opt_f64(o.scale_profit_offset);
        w.write_bool(o.scale_auto_reset);
        w.write_opt_i32(o.scale_init_position);
        w.write_opt_i32(o.scale_init_fill_qty);
        w.write_bool(o.scale_random_percent);
    }

    if version >= sv::SCALE_TABLE {
        w.write_string(&o.scale_table);
        w.write_string(&o.active_start_time);
        w.write_string(&o.active_stop_time);
    }

    if version >= sv::HEDGE_ORDERS {
        w.write_string(&o.hedge_type);
        if !o.hedge_type.is_empty() {
            w.write_string(&o.hedge_param);
        }
    }

    if version >= sv::OPT_OUT_SMART_ROUTING {
        w.write_bool(o.opt_out_smart_routing);
    }

    if version >= 39 {
        w.write_string(&o.clearing_account);
        w.write_string(&o.clearing_intent);
    }

    if version >= sv::NOT_HELD {
        w.write_bool(o.not_held);
    }

    if version >= sv::DELTA_NEUTRAL {
        write_delta_neutral(w, &contract.delta_neutral_contract);
    }

    if version >= sv::ALGO_ORDERS {
        w.write_string(&o.algo_strategy);
        if !o.algo_strategy.is_empty() {
            write_tag_value_list(w, &o.algo_params);
        }
    }
    if version >= sv::ALGO_ID {
        w.write_string(&o.algo_id);
    }
    if version >= sv::WHAT_IF_ORDERS {
        w.write_bool(o.what_if);
    }
    if version >= sv::LINKING {
        write_tag_value_string(w, &o.order_misc_options);
    }
    if version >= sv::ORDER_SOLICITED {
        w.write_bool(o.solicited);
    }
    if version >= sv::RANDOMIZE_SIZE_AND_PRICE {
        w.write_bool(o.randomize_size);
        w.write_bool(o.randomize_price);
    }

    if version >= sv::PEGGED_TO_BENCHMARK {
        if o.order_type == "PEG BENCH" {
            w.write_i32(o.reference_contract_id);
            w.write_bool(o.is_pegged_change_amount_decrease);
            w.write_opt_f64(o.pegged_change_amount);
            w.write_opt_f64(o.reference_change_amount);
            w.write_string(&o.reference_exchange);
        }
        w.write_i32(o.conditions.len() as i32);
        if !o.conditions.is_empty() {
            for cond in &o.conditions {
                w.write_string(cond);
            }
            w.write_bool(o.conditions_ignore_rth);
            w.write_bool(o.conditions_cancel_order);
        }
        w.write_string(&o.adjusted_order_type);
        w.write_opt_f64(o.trigger_price);
        w.write_opt_f64(o.limit_price_offset);
        w.write_opt_f64(o.adjusted_stop_price);
        w.write_opt_f64(o.adjusted_stop_limit_price);
        w.write_opt_f64(o.adjusted_trailing_amount);
        w.write_i32(o.adjustable_trailing_unit);
    }

    if version >= sv::EXT_OPERATOR {
        w.write_string(&o.ext_operator);
    }
    if version >= sv::SOFT_DOLLAR_TIER {
        w.write_string(&o.soft_dollar_tier_name);
        w.write_string(&o.soft_dollar_tier_value);
    }
    if version >= sv::CASH_QTY {
        w.write_opt_f64(o.cash_qty);
    }
    if version >= sv::DECISION_MAKER {
        w.write_string(&o.mifid2_decision_maker);
        w.write_string(&o.mifid2_decision_algo);
    }
    if version >= sv::MIFID_EXECUTION {
        w.write_string(&o.mifid2_execution_trader);
        w.write_string(&o.mifid2_execution_algo);
    }
    if version >= sv::AUTO_PRICE_FOR_HEDGE {
        w.write_bool(o.dont_use_auto_price_for_hedge);
    }
    if version >= sv::ORDER_CONTAINER {
        w.write_bool(o.is_oms_container);
    }
    if version >= sv::D_PEG_ORDERS {
        w.write_bool(o.discretionary_up_to_limit_price);
    }
    if version >= sv::PRICE_MGMT_ALGO {
        let s = match o.use_price_mgmt_algo {
            None => "",
            Some(false) => "0",
            Some(true) => "1",
        };
        w.write_string(s);
    }
    if version >= sv::DURATION {
        w.write_opt_i32(o.duration);
    }
    if version >= sv::POST_TO_ATS {
        w.write_opt_i32(o.post_to_ats);
    }
    if version >= sv::AUTO_CANCEL_PARENT {
        w.write_bool(o.auto_cancel_parent);
    }
    if version >= sv::ADVANCED_ORDER_REJECT {
        w.write_string(&o.advanced_error_override);
    }
    if version >= sv::MANUAL_ORDER_TIME {
        w.write_string(&o.manual_order_time);
    }
    if version >= sv::PEGBEST_PEGMID_OFFSETS {
        if contract.exchange == "IBKRATS" {
            w.write_opt_i32(o.min_trade_qty);
        }
        let mut send_mid_offsets = false;
        if o.order_type == "PEG BEST" {
            w.write_opt_i32(o.min_compete_size);
            w.write_opt_f64(o.compete_against_best_offset);
            if matches!(o.compete_against_best_offset, Some(v) if v.is_infinite()) {
                send_mid_offsets = true;
            }
        } else if o.order_type == "PEG MID" {
            send_mid_offsets = true;
        }
        if send_mid_offsets {
            w.write_opt_f64(o.mid_offset_at_whole);
            w.write_opt_f64(o.mid_offset_at_half);
        }
    }

    let _ = helpers::parse_contract_for_place_order; // silence dead_import on helpers import
}
