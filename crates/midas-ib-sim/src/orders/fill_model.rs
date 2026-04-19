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

/// Split `total` into 2–4 per-fill chunks for Pattern-B partial fills.
///
/// Invariants (enforced by the loop + asserts + proptest):
/// * every chunk is `>= MIN_CHUNK` (round-lot minimum, 100 shares);
/// * `chunks.iter().sum::<f64>() == total` (no drift, no loss);
/// * at most `2 + rng % 3` chunks, but may stop early when the remainder
///   falls below `2 * MIN_CHUNK` (i.e. nothing left to split further).
///
/// The earlier version used `raw.max(100.0).min(remaining - 100.0)`, which
/// fed a negative upper bound to `.min(...)` whenever `remaining < 200`.
/// That produced zero-sized / negative chunks and violated the sum
/// invariant. The clamp below never inverts.
pub fn partial_chunks(
    total: f64,
    base_seed: u64,
    order_id: OrderId,
    partial_threshold: f64,
) -> Vec<f64> {
    /// Minimum round-lot size. Any chunk we emit is guaranteed ≥ this.
    const MIN_CHUNK: f64 = 100.0;

    if total <= partial_threshold {
        return vec![total];
    }
    let mut rng = rng_for(base_seed, order_id, DrawKind::PartialChunking, 0);
    let n: u32 = 2 + (rng.gen::<u32>() % 3);
    let mut chunks = Vec::with_capacity(n as usize);
    let mut remaining = total;
    for i in 0..n {
        // Last slot — or the remainder can't split further without dipping
        // below MIN_CHUNK. Either way, ship the residual and stop.
        if i + 1 == n || remaining <= 2.0 * MIN_CHUNK {
            chunks.push(remaining);
            break;
        }
        // Propose a round-lot-aligned fraction between 20% and 50%, then
        // clamp into `[MIN_CHUNK, remaining - MIN_CHUNK]`. `clamp` panics if
        // the interval inverts, so we floor the ceiling at MIN_CHUNK to
        // guarantee `lo <= hi` in every reachable state.
        let frac = 0.2 + rng.gen::<f64>() * 0.3;
        let raw = (remaining * frac / MIN_CHUNK).round() * MIN_CHUNK;
        let ceiling = (remaining - MIN_CHUNK).max(MIN_CHUNK);
        let chunk = raw.clamp(MIN_CHUNK, ceiling);
        chunks.push(chunk);
        remaining -= chunk;
    }
    // Safety belt: every chunk must be at least MIN_CHUNK, and they must sum
    // to the original total (modulo f64 rounding noise ≤ 1e-6).
    debug_assert!(
        chunks.iter().all(|c| *c >= MIN_CHUNK - 1e-9),
        "partial_chunks emitted a sub-MIN_CHUNK chunk: {chunks:?}"
    );
    debug_assert!(
        (chunks.iter().sum::<f64>() - total).abs() < 1e-6,
        "partial_chunks sum drift: {chunks:?} vs total {total}"
    );
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

    // Regression: the old `raw.max(100).min(remaining - 100)` drove `raw`
    // negative when remaining was just above the partial threshold. Cover
    // the boundary where the bug lived (threshold → 2*threshold).
    #[test]
    fn partial_chunks_boundary_above_threshold_are_nonnegative() {
        for total in [101.0, 150.0, 199.0, 200.0, 201.0, 250.0] {
            let chunks = partial_chunks(total, 1, OrderId(1), 100.0);
            assert!(!chunks.is_empty(), "empty chunks for total={total}");
            for c in &chunks {
                assert!(*c >= 100.0 - 1e-9, "chunk {c} below MIN_CHUNK for {total}");
            }
            let sum: f64 = chunks.iter().sum();
            assert!(
                (sum - total).abs() < 1e-6,
                "sum drift for total={total}: chunks={chunks:?}",
            );
        }
    }

    proptest::proptest! {
        /// Every chunk is at least MIN_CHUNK and the chunks sum to `total`,
        /// across any (total, seed, threshold) combination.
        #[test]
        fn partial_chunks_invariants(
            total in 100.0f64..100_000.0,
            seed in 0u64..u64::MAX,
            order_id in 1i32..10_000,
            threshold in 100.0f64..5_000.0,
        ) {
            let chunks = partial_chunks(total, seed, OrderId(order_id), threshold);
            proptest::prop_assert!(!chunks.is_empty());
            for c in &chunks {
                proptest::prop_assert!(
                    *c >= 100.0 - 1e-9,
                    "sub-MIN chunk {c} in {chunks:?} (total={total})",
                );
            }
            let sum: f64 = chunks.iter().sum();
            proptest::prop_assert!(
                (sum - total).abs() < 1e-6,
                "sum drift {sum} vs {total}: chunks={chunks:?}",
            );
        }
    }
}
