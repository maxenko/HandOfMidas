//! Synthetic fill model. Pessimistic default — market must actually cross.
//!
//! See `plan/ib-sim/04-order-lifecycle.md` §"Fill model".

use rand::Rng;

use crate::engine::clock::VirtualInstant;
use crate::engine::types::{OrderId, OrderKind, Side};
use crate::orders::determinism::{rng_for, DrawKind};
use crate::orders::state_machine::OrderRecord;

#[derive(Clone, Debug, PartialEq)]
pub struct Fill {
    pub order_id: OrderId,
    pub price: f64,
    pub shares: f64,
    pub ts: VirtualInstant,
    pub side: Side,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SlippageKind {
    None,
    FixedBps(f64),
}

impl Default for SlippageKind {
    fn default() -> Self {
        SlippageKind::FixedBps(1.0)
    }
}

pub fn maybe_fill(
    order: &mut OrderRecord,
    mid: f64,
    bid: f64,
    ask: f64,
    now: VirtualInstant,
    base_seed: u64,
    slippage: SlippageKind,
) -> Option<Fill> {
    let fill_price = match order.kind {
        OrderKind::Market => cross_spread(order.side, bid, ask),
        OrderKind::Limit => match order.side {
            Side::Buy if ask <= order.limit_price? => order.limit_price?.min(ask),
            Side::Sell if bid >= order.limit_price? => order.limit_price?.max(bid),
            _ => return None,
        },
        OrderKind::Stop => {
            if !order.stop_triggered {
                if stop_triggered(order.side, order.aux_price?, mid) {
                    order.stop_triggered = true;
                } else {
                    return None;
                }
            }
            cross_spread(order.side, bid, ask)
        }
        OrderKind::StopLimit => {
            if !order.stop_triggered {
                if stop_triggered(order.side, order.aux_price?, mid) {
                    order.stop_triggered = true;
                } else {
                    return None;
                }
            }
            match order.side {
                Side::Buy if ask <= order.limit_price? => order.limit_price?.min(ask),
                Side::Sell if bid >= order.limit_price? => order.limit_price?.max(bid),
                _ => return None,
            }
        }
    };

    let final_price = match order.kind {
        OrderKind::Market | OrderKind::Stop => {
            apply_slippage(fill_price, order.side, base_seed, order.order_id, slippage)
        }
        OrderKind::Limit | OrderKind::StopLimit => fill_price,
    };

    Some(Fill {
        order_id: order.order_id,
        price: round_to_cent(final_price),
        shares: order.remaining_qty,
        ts: now,
        side: order.side,
    })
}

#[inline]
fn cross_spread(side: Side, bid: f64, ask: f64) -> f64 {
    match side {
        Side::Buy => ask,
        Side::Sell => bid,
    }
}

#[inline]
pub fn stop_triggered(side: Side, stop_price: f64, mid: f64) -> bool {
    match side {
        Side::Buy => mid >= stop_price,
        Side::Sell => mid <= stop_price,
    }
}

fn apply_slippage(
    price: f64,
    side: Side,
    base_seed: u64,
    order_id: OrderId,
    slippage: SlippageKind,
) -> f64 {
    match slippage {
        SlippageKind::None => price,
        SlippageKind::FixedBps(bps) => {
            let mut rng = rng_for(base_seed, order_id, DrawKind::Slippage, 0);
            let _: u64 = rng.gen();
            let delta = price * (bps / 10_000.0);
            match side {
                Side::Buy => price + delta,
                Side::Sell => price - delta,
            }
        }
    }
}

#[inline]
fn round_to_cent(p: f64) -> f64 {
    (p * 100.0).round() / 100.0
}

pub fn partial_chunks(
    total: f64,
    base_seed: u64,
    order_id: OrderId,
    partial_threshold: f64,
) -> Vec<f64> {
    if total <= partial_threshold {
        return vec![total];
    }
    let mut rng = rng_for(base_seed, order_id, DrawKind::PartialChunking, 0);
    let n: u32 = 2 + (rng.gen::<u32>() % 3);
    let mut chunks = Vec::with_capacity(n as usize);
    let mut remaining = total;
    for i in 0..n {
        if i + 1 == n {
            chunks.push(remaining);
        } else {
            let frac = 0.2 + rng.gen::<f64>() * 0.3;
            let raw = (remaining * frac / 100.0).round() * 100.0;
            let raw = raw.max(100.0).min(remaining - 100.0);
            chunks.push(raw);
            remaining -= raw;
        }
    }
    chunks
}

#[cfg(test)]
mod tests {
    use midas_broker_core::ContractSpec;

    use super::*;
    use crate::engine::types::{OrderStatusCode, PlaceOrderReq};

    fn rec(
        kind: OrderKind,
        side: Side,
        limit: Option<f64>,
        stop: Option<f64>,
        qty: f64,
    ) -> OrderRecord {
        let req = PlaceOrderReq {
            order_id: OrderId(1),
            contract: ContractSpec::Stock {
                symbol: "AAPL".into(),
                exchange: "SMART".into(),
                currency: "USD".into(),
            },
            side,
            total_quantity: qty,
            kind,
            limit_price: limit,
            aux_price: stop,
            tif: "DAY".into(),
            account: "U1".into(),
            parent_id: None,
            oca_group: None,
            transmit: true,
        };
        let mut r = OrderRecord::from_place_req(&req, VirtualInstant::ZERO);
        r.status = OrderStatusCode::Submitted;
        r
    }

    #[test]
    fn market_buy_fills_at_ask() {
        let mut o = rec(OrderKind::Market, Side::Buy, None, None, 100.0);
        let f = maybe_fill(
            &mut o,
            100.0,
            99.98,
            100.02,
            VirtualInstant::ZERO,
            0,
            SlippageKind::None,
        )
        .unwrap();
        assert_eq!(f.price, 100.02);
        assert_eq!(f.shares, 100.0);
    }

    #[test]
    fn market_sell_fills_at_bid() {
        let mut o = rec(OrderKind::Market, Side::Sell, None, None, 100.0);
        let f = maybe_fill(
            &mut o,
            100.0,
            99.98,
            100.02,
            VirtualInstant::ZERO,
            0,
            SlippageKind::None,
        )
        .unwrap();
        assert_eq!(f.price, 99.98);
    }

    #[test]
    fn limit_buy_does_not_fill_when_above() {
        let mut o = rec(OrderKind::Limit, Side::Buy, Some(99.50), None, 100.0);
        assert!(maybe_fill(
            &mut o,
            100.0,
            99.98,
            100.02,
            VirtualInstant::ZERO,
            0,
            SlippageKind::None
        )
        .is_none());
    }

    #[test]
    fn limit_buy_fills_when_market_drops() {
        let mut o = rec(OrderKind::Limit, Side::Buy, Some(100.00), None, 100.0);
        let f = maybe_fill(
            &mut o,
            99.99,
            99.98,
            100.00,
            VirtualInstant::ZERO,
            0,
            SlippageKind::None,
        )
        .unwrap();
        assert_eq!(f.price, 100.00);
    }

    #[test]
    fn stop_buy_triggers_and_fills() {
        let mut o = rec(OrderKind::Stop, Side::Buy, None, Some(100.50), 100.0);
        assert!(maybe_fill(
            &mut o,
            100.00,
            99.98,
            100.02,
            VirtualInstant::ZERO,
            0,
            SlippageKind::None
        )
        .is_none());
        assert!(!o.stop_triggered);
        let f = maybe_fill(
            &mut o,
            100.60,
            100.58,
            100.62,
            VirtualInstant::ZERO,
            0,
            SlippageKind::None,
        )
        .unwrap();
        assert!(o.stop_triggered);
        assert_eq!(f.price, 100.62);
    }

    #[test]
    fn stop_limit_triggers_but_requires_cross() {
        let mut o = rec(
            OrderKind::StopLimit,
            Side::Buy,
            Some(100.55),
            Some(100.50),
            100.0,
        );
        assert!(maybe_fill(
            &mut o,
            100.60,
            100.58,
            100.62,
            VirtualInstant::ZERO,
            0,
            SlippageKind::None
        )
        .is_none());
        assert!(o.stop_triggered);
        let f = maybe_fill(
            &mut o,
            100.55,
            100.50,
            100.55,
            VirtualInstant::ZERO,
            0,
            SlippageKind::None,
        )
        .unwrap();
        assert_eq!(f.price, 100.55);
    }

    #[test]
    fn partial_chunks_sum_to_total() {
        let chunks = partial_chunks(5_000.0, 1, OrderId(1), 1_000.0);
        assert!(chunks.len() >= 2);
        let sum: f64 = chunks.iter().sum();
        assert!((sum - 5_000.0).abs() < 1e-6);
    }

    #[test]
    fn partial_chunks_deterministic() {
        let a = partial_chunks(10_000.0, 42, OrderId(7), 1_000.0);
        let b = partial_chunks(10_000.0, 42, OrderId(7), 1_000.0);
        assert_eq!(a, b);
    }

    #[test]
    fn small_order_is_single_fill() {
        let chunks = partial_chunks(100.0, 0, OrderId(1), 1_000.0);
        assert_eq!(chunks, vec![100.0]);
    }

    #[test]
    fn slippage_bps_moves_price_against_buyer() {
        let mut o = rec(OrderKind::Market, Side::Buy, None, None, 100.0);
        let f = maybe_fill(
            &mut o,
            100.0,
            99.98,
            100.00,
            VirtualInstant::ZERO,
            0,
            SlippageKind::FixedBps(1.0),
        )
        .unwrap();
        assert_eq!(f.price, 100.01);
    }
}
