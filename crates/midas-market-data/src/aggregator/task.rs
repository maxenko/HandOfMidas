//! Per-`(symbol, timeframe)` aggregator task + window-alignment helper.
//!
//! The aggregator folds upstream 5 s RT bars (fanned out by the router
//! per NB-6 Model A) into the target timeframe. Partial emits are
//! coalesced at 100 ms (M-26); completed bars emit immediately at the
//! window boundary. The task owns its `SubscriptionHandle<Bar>` outright
//! — when the registry drops the [`AggregatorEntry`] the `JoinHandle`
//! aborts this task, which drops the upstream handle, which `DecRef`s
//! the router's RT-bar hub.
//!
//! No tick accumulation (BR-12): the aggregator never folds raw ticks —
//! volume would drift versus IB's own 5 s bars.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use midas_broker_core::market_data::{Bar, BarCompleteness, MarketDataError, Timeframe};
use tokio::sync::broadcast;
use tokio::sync::RwLock;

use crate::router::SubscriptionHandle;

/// Coalesce cadence for partial-bar emits (M-26).
const COALESCE_MS: u64 = 100;

/// Consecutive zero-receiver coalesce ticks required before the
/// aggregator task exits (M-4).
const ZERO_STREAK_AUTO_EXIT: u32 = 16;

/// Aggregator main loop.
///
/// * Reads `Arc<Bar>` values from `rt_handle` (shared RT-bar fan-out).
/// * Folds into the target `tf`, emitting `Completed` at window rollover
///   and `Partial` on the 100 ms coalesce tick.
/// * Keeps `last_bar_slot` fresh so [`BarAggregatorRegistry::last_bar`]
///   can serve snapshot resync after `Lagged` (M-11).
/// * Auto-exits after [`ZERO_STREAK_AUTO_EXIT`] consecutive zero-
///   receiver coalesce ticks (M-4).
///
/// [`BarAggregatorRegistry::last_bar`]: super::registry::BarAggregatorRegistry::last_bar
pub(crate) async fn run_aggregator(
    mut rt_handle: SubscriptionHandle<Bar>,
    tf: Timeframe,
    bars_tx: broadcast::Sender<Arc<Bar>>,
    last_bar_slot: Arc<RwLock<Option<Bar>>>,
) {
    let mut current: Option<Bar> = None;
    let mut coalesce = tokio::time::interval(Duration::from_millis(COALESCE_MS));
    coalesce.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // First tick fires immediately — consume it so our first partial
    // emit lands one coalesce window after the first rt-bar.
    coalesce.tick().await;
    let mut dirty = false;
    let mut zero_streak: u32 = 0;

    loop {
        tokio::select! {
            r = rt_handle.recv() => {
                let rt_bar = match r {
                    Ok(b) => b,
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        // M-11: lagged drops the current partial; the
                        // next rt-bar in the next window opens a fresh
                        // bar. Prior partial is NOT emitted as
                        // Completed — OHLC would be wrong.
                        tracing::warn!(
                            lag = n,
                            tf = ?tf,
                            "aggregator upstream lagged; invalidating current bar"
                        );
                        current = None;
                        dirty = false;
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        tracing::debug!(tf = ?tf, "aggregator upstream closed");
                        return;
                    }
                };

                let window_open = match align_to_window(rt_bar.ts_open, tf) {
                    Ok(t) => t,
                    Err(e) => {
                        tracing::error!(
                            tf = ?tf,
                            error = ?e,
                            "align_to_window failed; aborting aggregator"
                        );
                        return;
                    }
                };

                match current.as_mut() {
                    Some(bar) if bar.ts_open == window_open => {
                        // Fold into the open window.
                        bar.c = rt_bar.c;
                        if rt_bar.h > bar.h { bar.h = rt_bar.h; }
                        if rt_bar.l < bar.l { bar.l = rt_bar.l; }
                        bar.volume = bar.volume.saturating_add(rt_bar.volume);
                        bar.trade_count = bar.trade_count.saturating_add(rt_bar.trade_count);
                        // wap is the last upstream's; without per-bar
                        // weighting we prefer "most recent" over a
                        // naive arithmetic mean.
                        bar.wap = rt_bar.wap;
                        bar.completeness = BarCompleteness::Partial; // M-36
                        dirty = true;
                    }
                    _ => {
                        // Rollover: close previous, open new.
                        if let Some(mut prev) = current.take() {
                            prev.completeness = BarCompleteness::Completed;
                            let arc = Arc::new(prev.clone());
                            let _ = bars_tx.send(arc);
                            *last_bar_slot.write().await = Some(prev);
                        }
                        current = Some(Bar {
                            symbol: rt_bar.symbol.clone(),
                            timeframe: tf,
                            ts_open: window_open,
                            ts_close: window_open + tf_to_duration(tf),
                            o: rt_bar.o,
                            h: rt_bar.h,
                            l: rt_bar.l,
                            c: rt_bar.c,
                            volume: rt_bar.volume,
                            trade_count: rt_bar.trade_count,
                            wap: rt_bar.wap,
                            completeness: BarCompleteness::Partial,
                        });
                        dirty = true;
                    }
                }
            }
            _ = coalesce.tick(), if dirty => {
                if let Some(bar) = &current {
                    let arc = Arc::new(bar.clone());
                    let _ = bars_tx.send(arc);
                    *last_bar_slot.write().await = Some(bar.clone());
                }
                dirty = false;

                // M-4 auto-exit on idle.
                if bars_tx.receiver_count() == 0 {
                    zero_streak = zero_streak.saturating_add(1);
                    if zero_streak >= ZERO_STREAK_AUTO_EXIT {
                        tracing::debug!(
                            tf = ?tf,
                            "aggregator auto-exit (no receivers)"
                        );
                        return;
                    }
                } else {
                    zero_streak = 0;
                }
            }
        }
    }
}

/// Round `ts` down to the start of the `tf` window (UTC-aligned).
///
/// Rejects zero-duration timeframes (M-6) so the caller can convert the
/// error into a bar-stream failure instead of panicking on `% 0`.
pub(crate) fn align_to_window(
    ts: DateTime<Utc>,
    tf: Timeframe,
) -> Result<DateTime<Utc>, MarketDataError> {
    let secs = tf_to_duration(tf).as_secs() as i64;
    if secs <= 0 {
        return Err(MarketDataError::UnsupportedTimeframe(tf));
    }
    let epoch = ts.timestamp();
    let aligned = epoch - epoch.rem_euclid(secs);
    Ok(DateTime::from_timestamp(aligned, 0).unwrap_or(ts))
}

/// Canonical duration for every aggregator-supported timeframe.
///
/// Mirrors [`Timeframe::as_secs`] for the subset the aggregator
/// handles. Unsupported variants (`S1`, `H4`, `D1`, `W1`, `MN1`) map to
/// their `as_secs` value so the `align_to_window` error path fires
/// uniformly — but the registry already rejects those at subscribe
/// time, so this arm is defensive.
pub(crate) fn tf_to_duration(tf: Timeframe) -> Duration {
    Duration::from_secs(tf.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn align_to_window_m1_rounds_down() {
        // 1_700_000_000 % 60 == 20 → aligned at 1_699_999_980.
        let ts = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let aligned = align_to_window(ts, Timeframe::M1).unwrap();
        let secs = aligned.timestamp();
        assert_eq!(secs % 60, 0);
        assert!(secs <= 1_700_000_000 && secs > 1_700_000_000 - 60);
    }

    #[test]
    fn align_to_window_h1_rounds_down() {
        let ts = DateTime::from_timestamp(1_700_003_601, 0).unwrap();
        let aligned = align_to_window(ts, Timeframe::H1).unwrap();
        let secs = aligned.timestamp();
        assert_eq!(secs % 3600, 0);
        assert!(secs <= 1_700_003_601 && secs > 1_700_003_601 - 3600);
    }

    #[test]
    fn align_to_window_m5_rounds_down() {
        // M5 == 300 s. 1_700_000_000 % 300 == 200 → aligned at 1_699_999_800.
        let ts = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let aligned = align_to_window(ts, Timeframe::M5).unwrap();
        let secs = aligned.timestamp();
        assert_eq!(secs % 300, 0);
        assert!(secs <= 1_700_000_000 && secs > 1_700_000_000 - 300);
    }

    #[test]
    fn tf_to_duration_matches_as_secs() {
        for tf in [
            Timeframe::S5,
            Timeframe::S15,
            Timeframe::S30,
            Timeframe::M1,
            Timeframe::M5,
            Timeframe::M15,
            Timeframe::M30,
            Timeframe::H1,
        ] {
            assert_eq!(tf_to_duration(tf).as_secs(), tf.as_secs());
        }
    }
}
