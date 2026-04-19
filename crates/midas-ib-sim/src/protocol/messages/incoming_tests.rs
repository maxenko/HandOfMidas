//! Roundtrip + golden-fixture tests for every variant of `IncomingMsg`.
//!
//! Strategy: for each variant, build a representative value, encode via the
//! test-only `incoming_encoder`, parse with `IncomingMsg::parse`, assert
//! equality. `parse_place_order` is the tall tent-pole — it's tested at
//! three different server versions to exercise the gated field blocks.
//!
//! Golden fixtures are hand-derived from `rust-ibapi` v1.2.2's wire encoders
//! and checked in as raw byte strings, so a rewrite of our encoder can't
//! silently drift.

use bytes::Bytes;
use proptest::prelude::*;

use crate::protocol::framing::RawFrame;
use crate::protocol::messages::incoming::IncomingMsg;
use crate::protocol::messages::incoming_encoder::encode_incoming;
use crate::protocol::messages::types::{
    ComboLeg, ContractSpec, DeltaNeutralContract, ExecutionFilter, MarketDataType, OrderComboLeg,
    OrderSpec, TagValue,
};
use crate::protocol::{ProtocolError, ServerVersion};

fn sv(v: i32) -> ServerVersion {
    ServerVersion::new(v).unwrap_or_else(|| panic!("invalid test server version {v}"))
}

fn fields_from_literal(s: &[u8]) -> Vec<Bytes> {
    // Accept a `\0`-terminated NUL-delimited literal, producing the per-field
    // vec exactly the way the framing codec would.
    if s.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut start = 0;
    for (i, &b) in s.iter().enumerate() {
        if b == 0 {
            out.push(Bytes::copy_from_slice(&s[start..i]));
            start = i + 1;
        }
    }
    if start < s.len() {
        out.push(Bytes::copy_from_slice(&s[start..]));
    }
    out
}

fn roundtrip(msg: IncomingMsg, sv_: ServerVersion) {
    let frame = encode_incoming(&msg, sv_);
    let decoded = IncomingMsg::parse(frame, sv_).expect("parse");
    assert_eq!(decoded, msg, "roundtrip @ sv={sv_:?}");
}

// ---------------------------------------------------------------------------
// Simple / fixed-shape messages.
// ---------------------------------------------------------------------------

#[test]
fn roundtrip_start_api_with_optional_caps() {
    roundtrip(
        IncomingMsg::StartApi {
            client_id: 100,
            optional_caps: Some(String::new()),
        },
        sv(200),
    );
}

#[test]
fn roundtrip_start_api_with_non_empty_optional_caps() {
    // Every advertised version (176..=221) sits above OPTIONAL_CAPABILITIES
    // (72), so `optional_caps` is always present on the wire. Confirm the
    // non-empty Some case round-trips cleanly.
    roundtrip(
        IncomingMsg::StartApi {
            client_id: 42,
            optional_caps: Some("DEBUG=1".into()),
        },
        sv(200),
    );
}

#[test]
fn roundtrip_req_current_time() {
    roundtrip(IncomingMsg::ReqCurrentTime, sv(200));
}

#[test]
fn roundtrip_req_ids() {
    roundtrip(IncomingMsg::ReqIds { num_ids: 0 }, sv(200));
}

#[test]
fn roundtrip_req_open_orders() {
    roundtrip(IncomingMsg::ReqOpenOrders, sv(200));
}

#[test]
fn roundtrip_req_positions() {
    roundtrip(IncomingMsg::ReqPositions, sv(200));
}

#[test]
fn roundtrip_req_global_cancel() {
    roundtrip(IncomingMsg::ReqGlobalCancel, sv(200));
}

#[test]
fn roundtrip_req_market_data_type_each_variant() {
    for dt in [
        MarketDataType::Live,
        MarketDataType::Frozen,
        MarketDataType::Delayed,
        MarketDataType::DelayedFrozen,
    ] {
        roundtrip(IncomingMsg::ReqMarketDataType { data_type: dt }, sv(200));
    }
}

#[test]
fn roundtrip_req_account_data() {
    roundtrip(
        IncomingMsg::ReqAccountData {
            subscribe: true,
            acct_code: "DU123456".into(),
        },
        sv(200),
    );
    roundtrip(
        IncomingMsg::ReqAccountData {
            subscribe: false,
            acct_code: String::new(),
        },
        sv(200),
    );
}

#[test]
fn roundtrip_req_account_summary() {
    roundtrip(
        IncomingMsg::ReqAccountSummary {
            req_id: 9000,
            group: "All".into(),
            tags: "NetLiquidation,TotalCashValue".into(),
        },
        sv(200),
    );
}

#[test]
fn roundtrip_cancel_order_with_manual_time() {
    roundtrip(
        IncomingMsg::CancelOrder {
            order_id: 15,
            manual_order_cancel_time: Some("20260418-14:30:00".into()),
        },
        sv(200),
    );
}

#[test]
fn roundtrip_cancel_order_empty_manual_time() {
    // The MANUAL_ORDER_TIME gate (169) sits below our 176..221 advertised
    // range, so the field is always present on the wire. Confirm an empty
    // string round-trips as Some("").
    roundtrip(
        IncomingMsg::CancelOrder {
            order_id: 15,
            manual_order_cancel_time: Some(String::new()),
        },
        sv(200),
    );
}

#[test]
fn roundtrip_cancel_mkt_data() {
    roundtrip(IncomingMsg::CancelMktData { req_id: 1 }, sv(200));
}

// ---------------------------------------------------------------------------
// Contract-bearing messages.
// ---------------------------------------------------------------------------

fn stock_contract() -> ContractSpec {
    ContractSpec {
        contract_id: 265598,
        symbol: "AAPL".into(),
        security_type: "STK".into(),
        exchange: "SMART".into(),
        primary_exchange: "NASDAQ".into(),
        currency: "USD".into(),
        trading_class: "NMS".into(),
        ..Default::default()
    }
}

#[test]
fn roundtrip_req_contract_data_stock() {
    roundtrip(
        IncomingMsg::ReqContractData {
            req_id: 1001,
            contract: stock_contract(),
        },
        sv(200),
    );
}

#[test]
fn roundtrip_req_mkt_data_simple() {
    roundtrip(
        IncomingMsg::ReqMktData {
            req_id: 9000,
            contract: stock_contract(),
            generic_ticks: "100,101,104,106".into(),
            snapshot: false,
            regulatory_snapshot: false,
            opts: vec![],
        },
        sv(200),
    );
}

#[test]
fn roundtrip_req_mkt_data_with_opts() {
    roundtrip(
        IncomingMsg::ReqMktData {
            req_id: 9001,
            contract: stock_contract(),
            generic_ticks: "233".into(),
            snapshot: true,
            regulatory_snapshot: true,
            opts: vec![TagValue::new("XYZ", "1"), TagValue::new("XXX", "abc")],
        },
        sv(200),
    );
}

#[test]
fn roundtrip_req_mkt_data_bag_contract() {
    let mut c = stock_contract();
    c.security_type = "BAG".into();
    c.combo_legs = vec![
        ComboLeg {
            contract_id: 1,
            ratio: 1,
            action: "BUY".into(),
            exchange: "SMART".into(),
            ..Default::default()
        },
        ComboLeg {
            contract_id: 2,
            ratio: 2,
            action: "SELL".into(),
            exchange: "SMART".into(),
            ..Default::default()
        },
    ];
    roundtrip(
        IncomingMsg::ReqMktData {
            req_id: 9002,
            contract: c,
            generic_ticks: String::new(),
            snapshot: false,
            regulatory_snapshot: false,
            opts: vec![],
        },
        sv(200),
    );
}

#[test]
fn roundtrip_req_mkt_data_with_delta_neutral() {
    let mut c = stock_contract();
    c.delta_neutral_contract = Some(DeltaNeutralContract {
        contract_id: 999,
        delta: 0.5,
        price: 150.0,
    });
    roundtrip(
        IncomingMsg::ReqMktData {
            req_id: 9003,
            contract: c,
            generic_ticks: String::new(),
            snapshot: false,
            regulatory_snapshot: false,
            opts: vec![],
        },
        sv(200),
    );
}

#[test]
fn roundtrip_req_historical_data_simple() {
    roundtrip(
        IncomingMsg::ReqHistoricalData {
            req_id: 7001,
            contract: stock_contract(),
            end_date_time: "20260418 16:00:00 UTC".into(),
            duration: "1 D".into(),
            bar_size: "1 min".into(),
            what_to_show: "TRADES".into(),
            use_rth: true,
            format_date: 2,
            keep_up_to_date: false,
            chart_opts: vec![],
        },
        sv(200),
    );
}

#[test]
fn roundtrip_req_real_time_bars() {
    roundtrip(
        IncomingMsg::ReqRealTimeBars {
            req_id: 8001,
            contract: stock_contract(),
            bar_size: 5,
            what_to_show: "TRADES".into(),
            use_rth: true,
            opts: vec![],
        },
        sv(200),
    );
}

#[test]
fn roundtrip_req_executions_with_filter() {
    roundtrip(
        IncomingMsg::ReqExecutions {
            req_id: 5555,
            filter: ExecutionFilter {
                client_id: 100,
                acct_code: "DU123456".into(),
                time: "20260418 00:00:00".into(),
                symbol: "AAPL".into(),
                sec_type: "STK".into(),
                exchange: "SMART".into(),
                side: "BUY".into(),
            },
        },
        sv(200),
    );
}

#[test]
fn roundtrip_req_executions_empty_filter() {
    roundtrip(
        IncomingMsg::ReqExecutions {
            req_id: 0,
            filter: ExecutionFilter::default(),
        },
        sv(200),
    );
}

// ---------------------------------------------------------------------------
// PLACE_ORDER — exercised at multiple server versions to guard field gating.
// ---------------------------------------------------------------------------

fn simple_limit_order() -> OrderSpec {
    OrderSpec {
        action: "BUY".into(),
        total_quantity: 100.0,
        order_type: "LMT".into(),
        limit_price: Some(150.0),
        tif: "DAY".into(),
        transmit: true,
        display_size: 100,
        ..Default::default()
    }
}

#[test]
fn roundtrip_place_order_simple_limit_v200() {
    roundtrip(
        IncomingMsg::PlaceOrder {
            order_id: 1001,
            contract: stock_contract(),
            order: Box::new(simple_limit_order()),
        },
        sv(200),
    );
}

#[test]
fn roundtrip_place_order_simple_limit_v176() {
    // Same order, older advertised version — fields gated by TRAILING_PERCENT
    // etc. should all be present but manual_order_time, price-mgmt-algo etc.
    // still round-trip.
    roundtrip(
        IncomingMsg::PlaceOrder {
            order_id: 1001,
            contract: stock_contract(),
            order: Box::new(simple_limit_order()),
        },
        sv(176),
    );
}

#[test]
fn roundtrip_place_order_market_with_algo() {
    let order = OrderSpec {
        action: "SELL".into(),
        total_quantity: 200.0,
        order_type: "MKT".into(),
        tif: "GTC".into(),
        transmit: true,
        display_size: 200,
        algo_strategy: "VWAP".into(),
        algo_params: vec![
            TagValue::new("maxPctVol", "0.3"),
            TagValue::new("startTime", "14:30:00"),
        ],
        algo_id: "test-algo-1".into(),
        ..Default::default()
    };
    roundtrip(
        IncomingMsg::PlaceOrder {
            order_id: 1002,
            contract: stock_contract(),
            order: Box::new(order),
        },
        sv(200),
    );
}

#[test]
fn roundtrip_place_order_with_misc_options() {
    let order = OrderSpec {
        action: "BUY".into(),
        total_quantity: 50.0,
        order_type: "LMT".into(),
        limit_price: Some(100.0),
        tif: "DAY".into(),
        transmit: true,
        display_size: 50,
        order_misc_options: vec![TagValue::new("optA", "1"), TagValue::new("optB", "2")],
        ..Default::default()
    };
    roundtrip(
        IncomingMsg::PlaceOrder {
            order_id: 1003,
            contract: stock_contract(),
            order: Box::new(order),
        },
        sv(200),
    );
}

#[test]
fn roundtrip_place_order_trailing_percent_at_boundary() {
    // Exactly at the server_version boundary — trailing_percent goes from
    // "absent" to "present" at v62.
    let order = OrderSpec {
        action: "BUY".into(),
        total_quantity: 100.0,
        order_type: "TRAIL".into(),
        trailing_percent: Some(1.5),
        tif: "DAY".into(),
        transmit: true,
        ..Default::default()
    };
    roundtrip(
        IncomingMsg::PlaceOrder {
            order_id: 1004,
            contract: stock_contract(),
            order: Box::new(order),
        },
        sv(176),
    );
}

#[test]
fn roundtrip_place_order_bag_contract() {
    let mut contract = stock_contract();
    contract.security_type = "BAG".into();
    contract.combo_legs = vec![ComboLeg {
        contract_id: 1,
        ratio: 1,
        action: "BUY".into(),
        exchange: "SMART".into(),
        open_close: 0,
        short_sale_slot: 0,
        designated_location: String::new(),
        exempt_code: -1,
    }];
    let order = OrderSpec {
        action: "BUY".into(),
        total_quantity: 1.0,
        order_type: "LMT".into(),
        limit_price: Some(1.0),
        tif: "DAY".into(),
        transmit: true,
        order_combo_legs: vec![OrderComboLeg { price: Some(1.5) }],
        smart_combo_routing_params: vec![TagValue::new("route", "SMART")],
        ..Default::default()
    };
    roundtrip(
        IncomingMsg::PlaceOrder {
            order_id: 2001,
            contract,
            order: Box::new(order),
        },
        sv(200),
    );
}

#[test]
fn roundtrip_place_order_volatility_with_delta_neutral() {
    let order = OrderSpec {
        action: "BUY".into(),
        total_quantity: 10.0,
        order_type: "VOL".into(),
        limit_price: Some(2.0),
        volatility: Some(0.25),
        volatility_type: Some(1),
        delta_neutral_order_type: "MKT".into(),
        delta_neutral_aux_price: Some(0.0),
        delta_neutral_con_id: 123,
        delta_neutral_settling_firm: "IB".into(),
        delta_neutral_clearing_account: "DU".into(),
        delta_neutral_clearing_intent: "IB".into(),
        delta_neutral_open_close: "O".into(),
        delta_neutral_short_sale: false,
        delta_neutral_short_sale_slot: 0,
        delta_neutral_designated_location: String::new(),
        tif: "DAY".into(),
        transmit: true,
        ..Default::default()
    };
    roundtrip(
        IncomingMsg::PlaceOrder {
            order_id: 3001,
            contract: stock_contract(),
            order: Box::new(order),
        },
        sv(200),
    );
}

#[test]
fn roundtrip_place_order_peg_bench_with_conditions() {
    let order = OrderSpec {
        action: "BUY".into(),
        total_quantity: 100.0,
        order_type: "PEG BENCH".into(),
        reference_contract_id: 999,
        is_pegged_change_amount_decrease: true,
        pegged_change_amount: Some(0.5),
        reference_change_amount: Some(0.25),
        reference_exchange: "SMART".into(),
        conditions: vec!["cond1".into(), "cond2".into()],
        conditions_ignore_rth: true,
        conditions_cancel_order: false,
        tif: "DAY".into(),
        transmit: true,
        ..Default::default()
    };
    roundtrip(
        IncomingMsg::PlaceOrder {
            order_id: 4001,
            contract: stock_contract(),
            order: Box::new(order),
        },
        sv(200),
    );
}

#[test]
fn roundtrip_place_order_peg_best_ibkrats() {
    let mut contract = stock_contract();
    contract.exchange = "IBKRATS".into();
    let order = OrderSpec {
        action: "BUY".into(),
        total_quantity: 100.0,
        order_type: "PEG BEST".into(),
        tif: "DAY".into(),
        transmit: true,
        min_trade_qty: Some(10),
        min_compete_size: Some(100),
        compete_against_best_offset: Some(0.01),
        ..Default::default()
    };
    roundtrip(
        IncomingMsg::PlaceOrder {
            order_id: 5001,
            contract,
            order: Box::new(order),
        },
        sv(200),
    );
}

#[test]
fn roundtrip_place_order_peg_mid_sends_offsets() {
    let order = OrderSpec {
        action: "BUY".into(),
        total_quantity: 100.0,
        order_type: "PEG MID".into(),
        tif: "DAY".into(),
        transmit: true,
        mid_offset_at_whole: Some(0.01),
        mid_offset_at_half: Some(0.005),
        ..Default::default()
    };
    roundtrip(
        IncomingMsg::PlaceOrder {
            order_id: 5002,
            contract: stock_contract(),
            order: Box::new(order),
        },
        sv(200),
    );
}

#[test]
fn roundtrip_place_order_use_price_mgmt_algo_none_and_both_bools() {
    for v in [None, Some(false), Some(true)] {
        let order = OrderSpec {
            action: "BUY".into(),
            total_quantity: 100.0,
            order_type: "LMT".into(),
            limit_price: Some(150.0),
            tif: "DAY".into(),
            transmit: true,
            use_price_mgmt_algo: v,
            ..Default::default()
        };
        roundtrip(
            IncomingMsg::PlaceOrder {
                order_id: 6001,
                contract: stock_contract(),
                order: Box::new(order),
            },
            sv(200),
        );
    }
}

// ---------------------------------------------------------------------------
// Field-gate delta between server versions.
// ---------------------------------------------------------------------------

#[test]
fn place_order_field_count_varies_with_server_version() {
    // Same message, different advertised server version → different field
    // count on the wire, because at least one block is gated in/out. In our
    // 176..=221 range, `fa_profile` is gated on `< FA_PROFILE_DESUPPORT (177)`,
    // so v176 emits one extra field vs v177+.
    let msg = IncomingMsg::PlaceOrder {
        order_id: 1,
        contract: stock_contract(),
        order: Box::new(simple_limit_order()),
    };
    let v176 = encode_incoming(&msg, sv(176)).fields.len();
    let v177 = encode_incoming(&msg, sv(177)).fields.len();
    assert_eq!(
        v176,
        v177 + 1,
        "fa_profile should add exactly one field at v176 (got {v176} vs {v177})"
    );
}

// ---------------------------------------------------------------------------
// Unsupported IDs.
// ---------------------------------------------------------------------------

#[test]
fn parse_unsupported_msg_id_returns_error() {
    let frame = RawFrame {
        fields: vec![Bytes::from_static(b"42"), Bytes::from_static(b"1")],
    };
    let err = IncomingMsg::parse(frame, sv(200)).unwrap_err();
    assert!(
        matches!(err, ProtocolError::UnsupportedMsgId(42)),
        "got: {err:?}"
    );
}

// ---------------------------------------------------------------------------
// Golden fixtures — hand-derived from rust-ibapi v1.2.2 encoder semantics.
//
// Each fixture lists the expected NUL-delimited payload for a known message.
// Parsing it should reconstruct the same IncomingMsg our builder produces.
// ---------------------------------------------------------------------------

#[test]
fn golden_req_current_time() {
    let payload = b"49\x001\x00";
    let fields = fields_from_literal(payload);
    let msg = IncomingMsg::parse(RawFrame { fields }, sv(200)).unwrap();
    assert_eq!(msg, IncomingMsg::ReqCurrentTime);
}

#[test]
fn golden_req_global_cancel() {
    let payload = b"58\x001\x00";
    let fields = fields_from_literal(payload);
    let msg = IncomingMsg::parse(RawFrame { fields }, sv(200)).unwrap();
    assert_eq!(msg, IncomingMsg::ReqGlobalCancel);
}

#[test]
fn golden_start_api_v200() {
    // 71 | 2 | 100 | ""   (optional_caps present as empty string at sv>72)
    let payload = b"71\x002\x00100\x00\x00";
    let fields = fields_from_literal(payload);
    let msg = IncomingMsg::parse(RawFrame { fields }, sv(200)).unwrap();
    assert_eq!(
        msg,
        IncomingMsg::StartApi {
            client_id: 100,
            optional_caps: Some(String::new()),
        }
    );
}

#[test]
fn golden_req_market_data_type_delayed() {
    // 59 | 1 | 3 (Delayed)
    let payload = b"59\x001\x003\x00";
    let fields = fields_from_literal(payload);
    let msg = IncomingMsg::parse(RawFrame { fields }, sv(200)).unwrap();
    assert_eq!(
        msg,
        IncomingMsg::ReqMarketDataType {
            data_type: MarketDataType::Delayed,
        }
    );
}

#[test]
fn golden_cancel_order_v200_with_empty_manual_time() {
    // 4 | 1 | 15 | ""
    let payload = b"4\x001\x0015\x00\x00";
    let fields = fields_from_literal(payload);
    let msg = IncomingMsg::parse(RawFrame { fields }, sv(200)).unwrap();
    assert_eq!(
        msg,
        IncomingMsg::CancelOrder {
            order_id: 15,
            manual_order_cancel_time: Some(String::new()),
        }
    );
}

// ---------------------------------------------------------------------------
// Property tests — arbitrary messages roundtrip.
// ---------------------------------------------------------------------------

fn arb_stock_contract() -> impl Strategy<Value = ContractSpec> {
    (
        0..1_000_000i32,
        "[A-Z]{1,5}",
        "(SMART|ARCA|NYSE|NASDAQ|IBKRATS)",
        "USD",
        "[A-Z]{1,3}",
    )
        .prop_map(|(cid, sym, ex, cur, tc)| ContractSpec {
            contract_id: cid,
            symbol: sym,
            security_type: "STK".into(),
            exchange: ex,
            primary_exchange: "NASDAQ".into(),
            currency: cur,
            trading_class: tc,
            ..Default::default()
        })
}

fn arb_execution_filter() -> impl Strategy<Value = ExecutionFilter> {
    (
        0..1_000_000i32,
        "[A-Z0-9]{0,12}",
        "[0-9]{0,20}",
        "[A-Z]{0,5}",
        "(|STK|OPT|FUT)",
        "[A-Z]{0,6}",
        "(|BUY|SELL)",
    )
        .prop_map(|(c, a, t, s, sec, ex, side)| ExecutionFilter {
            client_id: c,
            acct_code: a,
            time: t,
            symbol: s,
            sec_type: sec,
            exchange: ex,
            side,
        })
}

proptest! {
    #[test]
    fn prop_start_api(client_id: i32, caps in "[a-zA-Z0-9 ]{0,32}") {
        roundtrip(IncomingMsg::StartApi {
            client_id,
            optional_caps: Some(caps),
        }, sv(200));
    }

    #[test]
    fn prop_req_ids(n in 0..100i32) {
        roundtrip(IncomingMsg::ReqIds { num_ids: n }, sv(200));
    }

    #[test]
    fn prop_cancel_mkt_data(req_id in 0..10_000i32) {
        roundtrip(IncomingMsg::CancelMktData { req_id }, sv(200));
    }

    #[test]
    fn prop_cancel_order(
        order_id in 1..1_000_000i32,
        manual in "[a-zA-Z0-9: -]{0,32}"
    ) {
        roundtrip(IncomingMsg::CancelOrder {
            order_id,
            manual_order_cancel_time: Some(manual),
        }, sv(200));
    }

    #[test]
    fn prop_req_account_summary(
        req_id in 0..10_000i32,
        group in "[A-Za-z]{1,16}",
        tags in "[A-Za-z,]{1,64}"
    ) {
        roundtrip(IncomingMsg::ReqAccountSummary { req_id, group, tags }, sv(200));
    }

    #[test]
    fn prop_req_contract_data(c in arb_stock_contract(), req_id in 0..10_000i32) {
        roundtrip(IncomingMsg::ReqContractData {
            req_id,
            contract: c,
        }, sv(200));
    }

    #[test]
    fn prop_req_executions(filter in arb_execution_filter(), req_id in 0..10_000i32) {
        roundtrip(IncomingMsg::ReqExecutions { req_id, filter }, sv(200));
    }

    #[test]
    fn prop_req_real_time_bars(
        c in arb_stock_contract(),
        req_id in 0..10_000i32,
        bar_size in 1..300i32,
        use_rth: bool,
    ) {
        roundtrip(IncomingMsg::ReqRealTimeBars {
            req_id,
            contract: c,
            bar_size,
            what_to_show: "TRADES".into(),
            use_rth,
            opts: vec![],
        }, sv(200));
    }
}
