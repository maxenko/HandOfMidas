//! Per-symbol publisher tasks (NB-7 sole upstream owners).
//!
//! Each publisher is spawned by the control actor on first-subscribe
//! and is the unique owner of the upstream `TickStream` /
//! `RealtimeBarStream` it was handed. When the actor aborts the task
//! on the last `DecRef`, the upstream handle is dropped and its own
//! Drop closure cancels the wire subscription.
//!
//! The hot path deliberately never touches the `RouterState` DashMap
//! (BR-18) — it holds only `Arc<SymbolHub>`, captured at spawn time.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use midas_broker_core::market_data::{EndReason, Quote, Tick, TickKind, TickType, TickValue};
use midas_broker_core::provider::{RealtimeBarStream, TickStream};
use tokio::sync::broadcast::error::RecvError;

use super::actor::RouterMsg;
use super::handle::GuardCtrl;
use super::state::{SymbolHub, PUBLISHER_AUTO_EXIT_STREAK};

/// Notify the control actor that the publisher's upstream is fully
/// closed (slice B2).
///
/// Fire-and-forget through [`GuardCtrl::send_msg`] so the backlog
/// counter stays balanced with the actor's post-recv decrement (every
/// other control-plane message goes through `bump_backlog` /
/// `send_control`; this one must too, or the counter wraps).
fn notify_upstream_closed(ctrl: &GuardCtrl, hub: &SymbolHub, reason: EndReason) {
    ctrl.send_msg(RouterMsg::UpstreamClosed {
        symbol: hub.symbol.clone(),
        reason,
    });
}

/// Tick publisher.
///
/// Owns `upstream` outright; dropping this task (via abort) cancels
/// the wire subscription. Auto-exits after
/// [`PUBLISHER_AUTO_EXIT_STREAK`] consecutive "no broadcast AND no
/// watch receivers" iterations (M-4 + NB-3).
///
/// On a full upstream close (`RecvError::Closed`) the publisher
/// notifies the control actor with [`RouterMsg::UpstreamClosed`]
/// (slice B2) before returning, so the actor can flip the per-hub
/// end-reason watch, tear the hub down, and emit the structured warn
/// log.
pub(crate) async fn run_tick_publisher(
    hub: Arc<SymbolHub>,
    mut upstream: TickStream,
    ctrl: GuardCtrl,
) {
    let mut zero_streak: u32 = 0;
    loop {
        match upstream.next().await {
            Ok(tick) => {
                hub.last_tick_ts
                    .store(tick.ts.timestamp_millis(), Ordering::Relaxed);

                // Fan out to broadcast consumers. `send` returns Err
                // when there are no receivers, which we treat the
                // same as "receiver count 0" below — don't log, don't
                // bail.
                let _ = hub.ticks_tx.send(tick.clone());

                // Update the coalesced watch on price ticks only.
                // Size-only ticks carry no price information — the
                // `(Size, LastSize)` arm in `update_last_quote`
                // never matched anyway, and the previous branch
                // that blindly refreshed `next.ts` guaranteed a
                // no-op spam at the tick cadence. Keep the watch
                // strictly price-driven.
                if matches!(tick.kind, TickKind::Price | TickKind::PriceSize) {
                    update_last_quote(&hub.last_quote_tx, &tick);
                }

                // Auto-exit: both broadcast AND watch idle.
                let bcast = hub.ticks_tx.receiver_count() > 0;
                let wtch = hub.last_quote_tx.receiver_count() > 0;
                if !bcast && !wtch {
                    zero_streak = zero_streak.saturating_add(1);
                    if zero_streak >= PUBLISHER_AUTO_EXIT_STREAK {
                        tracing::debug!(
                            symbol = %hub.symbol,
                            "tick publisher auto-exit (no receivers)"
                        );
                        return;
                    }
                } else {
                    zero_streak = 0;
                }
            }
            Err(RecvError::Lagged(n)) => {
                tracing::warn!(
                    symbol = %hub.symbol,
                    dropped = n,
                    "tick publisher upstream lagged"
                );
                continue;
            }
            Err(RecvError::Closed) => {
                tracing::debug!(symbol = %hub.symbol, "tick publisher upstream closed");
                // Slice B2: notify the actor so it can tear the hub
                // down cleanly (publish EndReason on the per-hub
                // watch + remove from per_symbol). Dropping `upstream`
                // here also cascades the cancel closure; consumers of
                // `hub.ticks_tx` will observe `Closed` on next
                // `recv()` once the hub Arc finally drops.
                notify_upstream_closed(&ctrl, &hub, EndReason::Disconnected);
                return;
            }
        }
    }
}

/// Realtime-bar publisher.
///
/// Mirrors [`run_tick_publisher`] but against `hub.rt_bars_tx`.
/// Auto-exit considers only the broadcast receiver count — there is
/// no "watch counterpart" for RT bars. On full upstream close the
/// publisher notifies the actor via [`RouterMsg::UpstreamClosed`]
/// (slice B2).
pub(crate) async fn run_rt_bar_publisher(
    hub: Arc<SymbolHub>,
    mut upstream: RealtimeBarStream,
    ctrl: GuardCtrl,
) {
    let mut zero_streak: u32 = 0;
    loop {
        match upstream.next().await {
            Ok(bar) => {
                let Some(tx) = hub.rt_bars_tx.get() else {
                    // Unreachable under normal construction — the actor
                    // populates `rt_bars_tx` before spawning us — but
                    // bail gracefully if an invariant inverts.
                    tracing::error!(
                        symbol = %hub.symbol,
                        "rt-bar publisher found no rt_bars_tx; exiting"
                    );
                    return;
                };
                let _ = tx.send(bar);

                let active = tx.receiver_count() > 0;
                if !active {
                    zero_streak = zero_streak.saturating_add(1);
                    if zero_streak >= PUBLISHER_AUTO_EXIT_STREAK {
                        tracing::debug!(
                            symbol = %hub.symbol,
                            "rt-bar publisher auto-exit (no receivers)"
                        );
                        return;
                    }
                } else {
                    zero_streak = 0;
                }
            }
            Err(RecvError::Lagged(n)) => {
                tracing::warn!(
                    symbol = %hub.symbol,
                    dropped = n,
                    "rt-bar publisher upstream lagged"
                );
                continue;
            }
            Err(RecvError::Closed) => {
                tracing::debug!(symbol = %hub.symbol, "rt-bar publisher upstream closed");
                // Slice B2: notify the actor (see tick publisher).
                notify_upstream_closed(&ctrl, &hub, EndReason::Disconnected);
                return;
            }
        }
    }
}

/// Coalesce a single tick into the last-quote watch.
///
/// Writes only when a price field actually changes, to avoid
/// unnecessary `watch::Sender::send` wakeups. The timestamp is
/// advanced only inside the branches that mutate a price so
/// size-only ticks (which carry no price information anyway) don't
/// spam every watch consumer at the tick cadence — that was Bug 6:
/// the previous `next.ts = tick.ts` refresh fired unconditionally,
/// guaranteeing `next != current` for every tick and waking all
/// watchers even when nothing semantically changed.
fn update_last_quote(tx: &tokio::sync::watch::Sender<Quote>, tick: &Tick) {
    let current = tx.borrow().clone();
    let mut next = current.clone();

    match (tick.tick_type, &tick.value) {
        (TickType::Bid, TickValue::Price(p)) => {
            next.bid = Some(*p);
            next.ts = tick.ts;
        }
        (TickType::Ask, TickValue::Price(p)) => {
            next.ask = Some(*p);
            next.ts = tick.ts;
        }
        (TickType::Last, TickValue::Price(p)) => {
            next.last = Some(*p);
            next.ts = tick.ts;
        }
        (TickType::Last, TickValue::PriceSize { price, .. }) => {
            next.last = Some(*price);
            next.ts = tick.ts;
        }
        // `PriceSize` atomic ticks most commonly carry the last
        // trade; the `tick_type` discriminates bid/ask/last.
        (TickType::Bid, TickValue::PriceSize { price, .. }) => {
            next.bid = Some(*price);
            next.ts = tick.ts;
        }
        (TickType::Ask, TickValue::PriceSize { price, .. }) => {
            next.ask = Some(*price);
            next.ts = tick.ts;
        }
        _ => {}
    }

    if next != current {
        let _ = tx.send(next);
    }
}
