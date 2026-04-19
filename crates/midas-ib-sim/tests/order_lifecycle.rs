//! Stage-04 integration tests — the high-level order-lifecycle guarantees
//! from `plan/ib-sim/04-order-lifecycle.md`.

use std::sync::Arc;

use midas_broker_core::ContractSpec;
use midas_ib_sim::orders::accounts::symbol_key_for;
use midas_ib_sim::orders::{patterns, BasicOrderSimulator};
use midas_ib_sim::{
    MarketSnapshot, OrderEmission, OrderId, OrderKind, OrderSimulator, OrderStatus,
    OrderStatusCode, PlaceOrderReq, Side, VirtualClock, VirtualInstant,
};

fn stock(sym: &str) -> ContractSpec {
    ContractSpec::Stock {
        symbol: sym.into(),
        exchange: "SMART".into(),
        currency: "USD".into(),
    }
}

fn place_req(order_id: i32, kind: OrderKind, side: Side, qty: f64) -> PlaceOrderReq {
    PlaceOrderReq {
        order_id: OrderId(order_id),
        contract: stock("AAPL"),
        side,
        total_quantity: qty,
        kind,
        limit_price: match kind {
            OrderKind::Limit | OrderKind::StopLimit => Some(150.00),
            _ => None,
        },
        aux_price: match kind {
            OrderKind::Stop | OrderKind::StopLimit => Some(148.00),
            _ => None,
        },
        tif: "DAY".into(),
        account: "U1".into(),
        parent_id: None,
        oca_group: None,
        transmit: true,
    }
}

fn market_snap(mid: f64) -> MarketSnapshot {
    MarketSnapshot {
        symbol: symbol_key_for(&stock("AAPL")),
        mid,
        bid: mid - 0.01,
        ask: mid + 0.01,
        last: mid,
        volume: Some(100),
        ts: VirtualInstant::from_millis(1_000),
    }
}

#[test]
fn e2e_bracket_tp_wins_sl_cancels_via_oca() {
    let mut s = BasicOrderSimulator::with_seed(Arc::new(VirtualClock::new()), 7);

    let mut parent = place_req(1, OrderKind::Market, Side::Buy, 100.0);
    parent.oca_group = Some("bracket-1".into());
    let _ = s.place(parent);

    let mut tp = place_req(2, OrderKind::Limit, Side::Sell, 100.0);
    tp.limit_price = Some(155.00);
    tp.parent_id = Some(OrderId(1));
    tp.oca_group = Some("bracket-1".into());
    let _ = s.place(tp);

    let mut sl = place_req(3, OrderKind::Stop, Side::Sell, 100.0);
    sl.aux_price = Some(148.00);
    sl.parent_id = Some(OrderId(1));
    sl.oca_group = Some("bracket-1".into());
    let _ = s.place(sl);

    let _ = s.on_market_snapshot(&market_snap(150.00));
    assert_eq!(s.orders()[&OrderId(2)].status, OrderStatusCode::Submitted);
    assert_eq!(s.orders()[&OrderId(3)].status, OrderStatusCode::Submitted);

    let out = s.on_market_snapshot(&market_snap(155.20));
    assert_eq!(s.orders()[&OrderId(2)].status, OrderStatusCode::Filled);
    assert_eq!(s.orders()[&OrderId(3)].status, OrderStatusCode::Cancelled);
    assert!(out.iter().any(|e| matches!(
        e,
        OrderEmission::OrderStatus(OrderStatus {
            order_id: OrderId(3),
            status: OrderStatusCode::Cancelled,
            ..
        })
    )));
}

#[test]
fn e2e_bracket_sl_wins_tp_cancels_via_oca() {
    let mut s = BasicOrderSimulator::with_seed(Arc::new(VirtualClock::new()), 7);

    let mut parent = place_req(10, OrderKind::Market, Side::Buy, 100.0);
    parent.oca_group = Some("b-10".into());
    let _ = s.place(parent);

    let mut tp = place_req(11, OrderKind::Limit, Side::Sell, 100.0);
    tp.limit_price = Some(160.00);
    tp.parent_id = Some(OrderId(10));
    tp.oca_group = Some("b-10".into());
    let _ = s.place(tp);

    let mut sl = place_req(12, OrderKind::Stop, Side::Sell, 100.0);
    sl.aux_price = Some(148.00);
    sl.parent_id = Some(OrderId(10));
    sl.oca_group = Some("b-10".into());
    let _ = s.place(sl);

    let _ = s.on_market_snapshot(&market_snap(150.00));
    let _ = s.on_market_snapshot(&market_snap(147.50));
    assert_eq!(s.orders()[&OrderId(12)].status, OrderStatusCode::Filled);
    assert_eq!(s.orders()[&OrderId(11)].status, OrderStatusCode::Cancelled);
}

#[test]
fn e2e_determinism_three_runs_byte_identical() {
    fn run(seed: u64) -> Vec<OrderEmission> {
        let mut s = BasicOrderSimulator::with_seed(Arc::new(VirtualClock::new()), seed);
        let mut all = Vec::new();
        for i in 1..=10 {
            all.extend(s.place(place_req(i, OrderKind::Market, Side::Buy, 100.0)));
        }
        all.extend(s.on_market_snapshot(&market_snap(150.00)));
        all.extend(s.on_market_snapshot(&market_snap(150.10)));
        all
    }
    let a = run(99);
    let b = run(99);
    let c = run(99);
    assert_eq!(format!("{a:?}"), format!("{b:?}"));
    assert_eq!(format!("{b:?}"), format!("{c:?}"));
}

#[test]
fn e2e_pattern_b_market_order_emits_exec_before_status() {
    let mut found = None;
    for i in 1..500 {
        if matches!(
            patterns::select_pattern(7, OrderId(i), OrderKind::Market, 100.0, 1_000.0),
            patterns::PatternKind::B
        ) {
            found = Some(i);
            break;
        }
    }
    let oid = found.expect("must find a Pattern-B id");

    let mut s = BasicOrderSimulator::with_seed(Arc::new(VirtualClock::new()), 7);
    let _ = s.place(place_req(oid, OrderKind::Market, Side::Buy, 100.0));
    let out = s.on_market_snapshot(&market_snap(150.00));

    let exec_idx = out
        .iter()
        .position(|e| matches!(e, OrderEmission::Execution(_)));
    let filled_idx = out.iter().position(|e| {
        matches!(
            e,
            OrderEmission::OrderStatus(OrderStatus {
                status: OrderStatusCode::Filled,
                ..
            })
        )
    });
    assert!(
        exec_idx.unwrap() < filled_idx.unwrap(),
        "Pattern B must emit Execution before OrderStatus(Filled)"
    );
}

#[test]
fn e2e_account_snapshot_reflects_fills() {
    let mut s = BasicOrderSimulator::with_seed(Arc::new(VirtualClock::new()), 7);
    let _ = s.place(place_req(1, OrderKind::Market, Side::Buy, 100.0));
    let _ = s.on_market_snapshot(&market_snap(150.00));

    let snap = s.account().snapshot_positions();
    let positions = snap
        .iter()
        .filter(|e| matches!(e, OrderEmission::Position(_)))
        .count();
    assert_eq!(positions, 1);
    assert!(matches!(snap.last(), Some(OrderEmission::PositionEnd)));
}
