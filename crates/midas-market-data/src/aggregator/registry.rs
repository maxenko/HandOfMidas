//! [`BarAggregatorRegistry`] — lazily-spawned per-`(symbol, timeframe)`
//! aggregator tasks with refcounted subscribe/drop.
//!
//! Sits inside [`MarketDataRouter`] alongside the per-symbol hub map.
//! Each entry owns one aggregator task, its `JoinHandle`, a
//! `broadcast::Sender<Arc<Bar>>` for fan-out, and a `last_bar` slot for
//! snapshot resync after [`broadcast::error::RecvError::Lagged`].
//!
//! Supported timeframes: `S5`, `S15`, `S30`, `M1`, `M5`, `M15`, `M30`,
//! `H1`. `S1` is rejected because upstream RT bars arrive on a 5 s
//! cadence so S1 would always produce a single-tick "bar" — meaningless
//! (use tick-by-tick instead). `H4` / `D1` / `W1` / `MN1` are rejected
//! because they carry session-boundary / calendar semantics the 5 s
//! fold cannot express correctly (BR-22).
//!
//! [`MarketDataRouter`]: crate::router::MarketDataRouter

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Weak};

use midas_broker_core::market_data::{Bar, MarketDataError, SymbolKey, Timeframe};
use tokio::sync::{broadcast, Mutex, RwLock};
use tokio::task::JoinHandle;

use crate::router::{Guard, MarketDataRouter, SubscriptionHandle};

use super::task::run_aggregator;

/// Default capacity of the aggregator's output broadcast.
///
/// 256 is ~25 s of buffer at the 100 ms partial-emit cadence (M-26),
/// which is comfortably above the expected frame rate of a chart
/// consumer (16 ms / 60 fps). Consumers that fall behind see
/// `Lagged` and must resync via [`BarAggregatorRegistry::last_bar`].
const BARS_CAP: usize = 256;

/// Refcounted per-`(symbol, tf)` aggregator record.
pub(crate) struct AggregatorEntry {
    /// Fan-out sender for aggregated `Arc<Bar>` emits.
    bars_tx: broadcast::Sender<Arc<Bar>>,
    /// Live `SubscriptionHandle<Bar>` count. Incremented by
    /// [`BarAggregatorRegistry::subscribe`], decremented by
    /// [`AggGuard::drop`].
    refcount: AtomicU32,
    /// Most-recent bar (partial or completed) the aggregator emitted.
    /// Kept in an `RwLock` so [`BarAggregatorRegistry::last_bar`] can
    /// serve snapshot reads without stealing from the task.
    last_bar: Arc<RwLock<Option<Bar>>>,
    /// Aggregator task `JoinHandle`. Aborted in [`Drop`] so removing
    /// the entry from the registry map cancels the task, which in
    /// turn drops the upstream `SubscriptionHandle<Bar>` and `DecRef`s
    /// the router's RT-bar hub.
    task: JoinHandle<()>,
}

impl Drop for AggregatorEntry {
    fn drop(&mut self) {
        // Tokio's `Drop for JoinHandle` does NOT abort the task; we
        // must explicitly cancel so the aggregator stops folding bars
        // and releases its upstream handle. The drop of the upstream
        // `SubscriptionHandle<Bar>` inside the cancelled task sends
        // `DecRtBarRef` to the router's control actor.
        self.task.abort();
    }
}

/// Lazily-spawned per-`(symbol, tf)` aggregator registry.
///
/// Shared by every consumer of [`MarketDataRouter::subscribe_bars`].
/// Construct with [`BarAggregatorRegistry::new`] (uses
/// [`Arc::new_cyclic`] to stash `weak_self` for the RAII guard).
pub struct BarAggregatorRegistry {
    /// Live aggregators, keyed by `(symbol, timeframe)`. Serialised
    /// by a single `tokio::sync::Mutex` — aggregator bring-up is cold
    /// and rare, so coarse-grained locking is fine (BR-5).
    aggregators: Mutex<HashMap<(SymbolKey, Timeframe), Arc<AggregatorEntry>>>,
    /// Weak back-reference to the router, so the aggregator can reach
    /// `subscribe_rt_bars` without forming a cycle (BR-6). Upgrade
    /// returns `None` on router teardown — never `.unwrap()`-ed.
    router: Weak<MarketDataRouter>,
    /// Self-weak, captured via [`Arc::new_cyclic`] so [`AggGuard`] can
    /// reach back into the registry on drop without a strong reference.
    weak_self: Weak<BarAggregatorRegistry>,
}

impl BarAggregatorRegistry {
    /// Build a new registry whose aggregators subscribe upstream via
    /// `router`. The registry is wrapped in an `Arc` via
    /// [`Arc::new_cyclic`] so `weak_self` is populated before any
    /// method runs.
    pub fn new(router: Weak<MarketDataRouter>) -> Arc<Self> {
        Arc::new_cyclic(|weak_self| Self {
            aggregators: Mutex::new(HashMap::new()),
            router,
            weak_self: weak_self.clone(),
        })
    }

    /// Subscribe to aggregated bars at `tf` for `symbol`.
    ///
    /// First subscriber per `(symbol, tf)` lazily:
    ///
    /// 1. Rejects unsupported timeframes (BR-22) without touching the
    ///    map.
    /// 2. Upgrades `self.router`; on failure returns
    ///    [`MarketDataError::ShuttingDown`] (BR-6).
    /// 3. Subscribes to the router's refcounted RT-bar fan-out for
    ///    `symbol` (BR-12 + NB-6 Model A).
    /// 4. Spawns an aggregator task that folds the 5 s stream into `tf`.
    ///
    /// Subsequent subscribers on the same key share the broadcast and
    /// simply bump the refcount. The returned
    /// [`SubscriptionHandle<Bar>`] carries an [`AggGuard`] that
    /// `DecRef`s on drop.
    pub async fn subscribe(
        &self,
        symbol: SymbolKey,
        tf: Timeframe,
    ) -> Result<SubscriptionHandle<Bar>, MarketDataError> {
        // BR-22: reject RTH-aligned / calendar-scale / sub-5 s
        // timeframes upfront.
        if is_unsupported_tf(tf) {
            return Err(MarketDataError::UnsupportedTimeframe(tf));
        }

        // BR-6: weak upgrade, no unwrap.
        let router = self.router.upgrade().ok_or(MarketDataError::ShuttingDown)?;

        let key = (symbol.clone(), tf);
        let mut map = self.aggregators.lock().await;
        let entry = if let Some(existing) = map.get(&key) {
            existing.clone()
        } else {
            // NB-6 Model A: subscribe via the router's RT-bar fan-out,
            // not the source directly. Two aggregators on the same
            // symbol (different tfs) share ONE upstream.
            let rt_handle = router.subscribe_rt_bars(symbol.clone()).await?;
            let (bars_tx, _) = broadcast::channel(BARS_CAP);
            let last_bar_slot: Arc<RwLock<Option<Bar>>> = Arc::new(RwLock::new(None));
            let task = tokio::spawn(run_aggregator(
                rt_handle,
                tf,
                bars_tx.clone(),
                last_bar_slot.clone(),
            ));
            let entry = Arc::new(AggregatorEntry {
                bars_tx,
                refcount: AtomicU32::new(0),
                last_bar: last_bar_slot,
                task,
            });
            map.insert(key.clone(), entry.clone());
            entry
        };

        entry.refcount.fetch_add(1, Ordering::Relaxed);
        let rx = entry.bars_tx.subscribe();
        // Release the map before constructing the handle so consumers
        // never see the map locked while the actor is pumping.
        drop(map);

        let guard: Box<dyn Guard> = Box::new(AggGuard {
            key,
            registry: self.weak_self.clone(),
        });
        Ok(SubscriptionHandle::new(rx, guard))
    }

    /// Snapshot accessor for `(symbol, tf)`'s most recent emitted bar.
    ///
    /// Returns `None` if no aggregator exists for the key or the
    /// aggregator has not emitted anything yet. Used by the
    /// `ChartResync` path (S7) after a consumer observes `Lagged`.
    pub async fn last_bar(&self, symbol: &SymbolKey, tf: Timeframe) -> Option<Bar> {
        let map = self.aggregators.lock().await;
        if let Some(entry) = map.get(&(symbol.clone(), tf)) {
            return entry.last_bar.read().await.clone();
        }
        None
    }

    /// Test hook: does an aggregator exist for `(symbol, tf)`?
    ///
    /// Hidden from public docs; used by behaviour tests to observe
    /// lazy spawn / last-drop cleanup without synthetic sleeps.
    #[doc(hidden)]
    pub async fn has_aggregator(&self, symbol: &SymbolKey, tf: Timeframe) -> bool {
        let map = self.aggregators.lock().await;
        map.contains_key(&(symbol.clone(), tf))
    }

    /// Test hook: number of currently-registered aggregators.
    #[doc(hidden)]
    pub async fn registered_count(&self) -> usize {
        self.aggregators.lock().await.len()
    }
}

/// RAII guard on an aggregator subscription.
///
/// Drop triggers a fire-and-forget `tokio::spawn` that decrements the
/// entry's refcount and, if it hits zero, removes the entry from the
/// registry map. Dropping the map entry drops the `JoinHandle`, which
/// aborts the aggregator task, which drops its upstream
/// [`SubscriptionHandle<Bar>`], which `DecRef`s the router's RT-bar hub.
pub(crate) struct AggGuard {
    key: (SymbolKey, Timeframe),
    registry: Weak<BarAggregatorRegistry>,
}

impl Guard for AggGuard {}

impl Drop for AggGuard {
    fn drop(&mut self) {
        let key = self.key.clone();
        let weak = self.registry.clone();
        // Fire-and-forget: guard drop is synchronous, but the removal
        // must take the async `Mutex`. Acceptable to delay — all that
        // happens is the aggregator lives a touch longer.
        tokio::spawn(async move {
            let Some(reg) = weak.upgrade() else {
                return;
            };
            let mut map = reg.aggregators.lock().await;
            let Some(entry) = map.get(&key) else {
                return;
            };
            let prev = entry.refcount.fetch_sub(1, Ordering::Relaxed);
            if prev == 0 {
                // Underflow — log and carry on.
                tracing::warn!(
                    key = ?key,
                    "aggregator refcount underflow on drop"
                );
                entry.refcount.store(0, Ordering::Relaxed);
                return;
            }
            if prev == 1 {
                // Dropping the entry aborts the task and cascades
                // DecRef upstream.
                map.remove(&key);
            }
        });
    }
}

/// BR-22: which timeframes the aggregator refuses to synthesise from 5 s
/// RT bars.
///
/// Rejected set:
///
/// * `S1` — shorter than the upstream cadence.
/// * `H4` — RTH-aligned; would need a trading-calendar dep.
/// * `D1`, `W1`, `MN1` — session / calendar semantics; `historical_bars`
///   is the correct path for those.
///
/// Supported: `S5`, `S15`, `S30`, `M1`, `M5`, `M15`, `M30`, `H1`.
fn is_unsupported_tf(tf: Timeframe) -> bool {
    matches!(
        tf,
        Timeframe::S1 | Timeframe::H4 | Timeframe::D1 | Timeframe::W1 | Timeframe::MN1
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_timeframes_accept() {
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
            assert!(!is_unsupported_tf(tf), "{tf:?} should be supported");
        }
    }

    #[test]
    fn unsupported_timeframes_reject() {
        for tf in [
            Timeframe::S1,
            Timeframe::H4,
            Timeframe::D1,
            Timeframe::W1,
            Timeframe::MN1,
        ] {
            assert!(is_unsupported_tf(tf), "{tf:?} should be rejected");
        }
    }
}
