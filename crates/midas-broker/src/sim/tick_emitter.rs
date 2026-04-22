//! Background tick-emitter loop for [`SimMarketData`].
//!
//! The loop wakes every `tick_cadence_ms` via `tokio::time::interval`
//! (BR-20: paused-time-friendly), advances every registered symbol's
//! price via xorshift drift, and emits **at most one** `Last` event
//! plus one paired `Bid` and one paired `Ask` per window per
//! subscription. This mirrors IB's `reqMktData` sampling (~250 ms
//! aggregation) and keeps test bookkeeping simple: a test that knows
//! the cadence knows the exact upper/lower bound on tick count.
//!
//! Ordering guarantees:
//! * New ticks arrive in wire order per subscription.
//! * Every sub owned by the map is either "live" (`cancelled == false`)
//!   or "draining" (`cancelled == true`, `draining_until_ns > now`).
//!   Both states get publishes; entries past the drain window are
//!   swept by the loop itself so in-flight receivers still observe
//!   [`RecvError::Closed`] promptly.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use dashmap::DashMap;
use midas_broker_core::market_data::{
    ReqId, SymbolKey, Tick, TickAttributes, TickKind, TickType, TickValue,
};
use parking_lot::Mutex;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use crate::sim::market_data::{SimSubscription, SymbolSimState};
use crate::sim::rng::Xorshift64;

/// Owner-held handle on the tick-loop.
///
/// The loop itself is an infinite `interval`-driven task; dropping the
/// [`SimMarketData`](crate::sim::SimMarketData) that owns this handle
/// causes the join handle to be dropped, which aborts the task.
pub(crate) struct TickLoopHandle {
    pub(crate) wake: Arc<Notify>,
    #[allow(dead_code)]
    pub(crate) join: JoinHandle<()>,
}

/// Spawn the tick emitter loop.
///
/// Arguments:
/// * `cadence` — the wake interval (default 250 ms).
/// * `late_window` — grace period for cancelled subs (M-24).
/// * `drift_bps` — peak uniform drift per window.
/// * `spread` — bid/ask spread in dollars.
/// * `subscriptions` — the sim's per-reqId subscription map.
/// * `symbol_state` — per-symbol price / volume state.
/// * `rng` — shared xorshift RNG.
pub(crate) fn run_tick_loop(
    cadence: Duration,
    late_window: Duration,
    drift_bps: f64,
    spread: f64,
    subscriptions: Arc<DashMap<ReqId, Arc<SimSubscription>>>,
    symbol_state: Arc<DashMap<SymbolKey, SymbolSimState>>,
    rng: Arc<Mutex<Xorshift64>>,
) -> TickLoopHandle {
    let wake = Arc::new(Notify::new());
    let join = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(cadence);
        // Skip missed ticks so a long `tokio::time::advance(Nx cadence)`
        // in tests fires the emitter N times, not once (the default
        // `Burst` behaviour) and not only once at the end — every
        // missed window still produces one emission.
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // Consume the immediate first tick so the loop's work fires on
        // the first full cadence, not at t=0 — avoids racing the
        // initial-burst task.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            tick_once(
                drift_bps,
                spread,
                late_window,
                &subscriptions,
                &symbol_state,
                &rng,
            );
        }
    });
    TickLoopHandle { wake, join }
}

/// Single emission pass. Public-in-crate for unit tests.
fn tick_once(
    drift_bps: f64,
    spread: f64,
    late_window: Duration,
    subscriptions: &DashMap<ReqId, Arc<SimSubscription>>,
    symbol_state: &DashMap<SymbolKey, SymbolSimState>,
    rng: &Mutex<Xorshift64>,
) {
    // 1. Sweep: drop cancelled subs whose drain window has elapsed.
    let now_ms = super::market_data_helpers::now_ms();
    let dead: Vec<ReqId> = subscriptions
        .iter()
        .filter_map(|entry| {
            let sub = entry.value();
            if sub.cancelled.load(Ordering::SeqCst) {
                let until = sub.draining_until_ns.load(Ordering::SeqCst);
                if until == 0 || now_ms.saturating_sub(until) >= late_window.as_millis() as u64 {
                    Some(sub.req_id)
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect();
    for req_id in dead {
        subscriptions.remove(&req_id);
    }

    // 2. Advance each symbol's price by a single xorshift step.
    let mut per_symbol_updates: Vec<(SymbolKey, f64, i64)> = Vec::new();
    for mut entry in symbol_state.iter_mut() {
        let sym = entry.key().clone();
        let state = entry.value_mut();
        let prev = state.market_price;
        let new_price = if drift_bps > 0.0 {
            let mut rng = rng.lock();
            let unit = rng.next_unit();
            let drift = (drift_bps / 10_000.0) * unit * prev;
            (prev + drift).max(0.01)
        } else {
            prev
        };
        state.market_price = new_price;
        state.volume_accum = state.volume_accum.saturating_add(100);
        per_symbol_updates.push((sym, new_price, state.volume_accum));
    }

    if per_symbol_updates.is_empty() {
        return;
    }

    // 3. Fan out: one Last + paired Bid + paired Ask per sub per
    //    window. Skip subs with zero receivers (M-4).
    let ts = Utc::now();
    let half_spread = spread / 2.0;

    for entry in subscriptions.iter() {
        let sub = entry.value();
        let Some(tx) = sub.tx.lock().clone() else {
            continue;
        };
        // Cancelled subs inside the drain window still receive ticks
        // (M-24 / M-25) — `receiver_count == 0` is the terminal check.
        if tx.receiver_count() == 0 {
            continue;
        }
        for (sym, new_price, vol) in per_symbol_updates.iter() {
            if *sym != sub.symbol {
                continue;
            }
            let mk = |kind: TickKind, tick_type: TickType, value: TickValue| {
                Arc::new(Tick {
                    symbol: sym.clone(),
                    req_id: sub.req_id,
                    kind,
                    tick_type,
                    value,
                    attrs: TickAttributes::default(),
                    ts,
                })
            };

            // Atomic Last + LastSize (M-17).
            let _ = tx.send(mk(
                TickKind::PriceSize,
                TickType::Last,
                TickValue::PriceSize {
                    price: *new_price,
                    size: 100,
                },
            ));

            // Paired Bid snapshot.
            let _ = tx.send(mk(
                TickKind::PriceSize,
                TickType::Bid,
                TickValue::PriceSize {
                    price: new_price - half_spread,
                    size: 100,
                },
            ));

            // Paired Ask snapshot.
            let _ = tx.send(mk(
                TickKind::PriceSize,
                TickType::Ask,
                TickValue::PriceSize {
                    price: new_price + half_spread,
                    size: 100,
                },
            ));

            // Volume delta.
            let _ = tx.send(mk(TickKind::Size, TickType::Volume, TickValue::Size(*vol)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::market_data::SubKind;
    use std::sync::atomic::AtomicBool;
    use std::sync::OnceLock;
    use tokio::sync::broadcast;

    fn make_sub(
        req_id: ReqId,
        sym: SymbolKey,
    ) -> (Arc<SimSubscription>, broadcast::Receiver<Arc<Tick>>) {
        let (tx, rx) = broadcast::channel(256);
        let sub = Arc::new(SimSubscription {
            req_id,
            symbol: sym,
            con_id: 0,
            kind: SubKind::Tick,
            tx: Mutex::new(Some(tx)),
            cancelled: AtomicBool::new(false),
            draining_until_ns: std::sync::atomic::AtomicU64::new(0),
            last_error: Arc::new(OnceLock::new()),
        });
        (sub, rx)
    }

    #[test]
    fn tick_once_emits_for_live_sub() {
        let subs = Arc::new(DashMap::new());
        let states = Arc::new(DashMap::new());
        let rng = Arc::new(Mutex::new(Xorshift64::new(1)));
        let sym = SymbolKey {
            contract_id: 1,
            symbol: "AAPL".into(),
        };
        states.insert(sym.clone(), SymbolSimState::new(100.0));
        let (sub, mut rx) = make_sub(ReqId(1), sym);
        subs.insert(ReqId(1), sub);

        tick_once(10.0, 0.01, Duration::from_millis(200), &subs, &states, &rng);
        // Expect at least 4 events: Last, Bid, Ask, Volume.
        let mut count = 0;
        while rx.try_recv().is_ok() {
            count += 1;
        }
        assert!(count >= 4, "expected ≥4 events, got {count}");
    }
}
