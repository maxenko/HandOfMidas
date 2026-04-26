// S7a: the registry skeleton is wired in this commit so the
// infrastructure compiles and is reachable from subsequent slices;
// the actual bind call-sites land in S7b (charts) and S7d (ticker
// states). Suppressing dead_code here keeps the S7a commit clean
// without hacking a feature flag onto every identifier.
//
// Audit P1 refactor 3: the four previously-per-module statics
// (CHART_REGISTRY, TICKER_REGISTRY, WATCHLIST_REGISTRY, ROUTER)
// moved behind a single `SubscriptionContext`. The free functions
// below remain as thin shims so call sites don't churn.
#![allow(dead_code)]

//! Keyed registry of live router subscription handles.
//!
//! `Subscription::run_with` requires a `fn` pointer (not a closure),
//! so the per-instance `SubscriptionHandle<Bar>` / `SubscriptionHandle<Tick>`
//! returned from the router cannot be captured directly. Instead, the
//! app subscribe path (S7b: chart bind; S7d: ticker activate) parks
//! the handle in the process-scoped [`SubscriptionContext`] keyed by
//! a hashable identifier, and the builder function looks it up inside
//! the closure.
//!
//! The handle lives behind an `Arc<Mutex<_>>` so the context keeps
//! the guard alive for the lifetime of the keyed entry while each
//! subscription-stream invocation calls `resubscribe()` to obtain a
//! fresh `broadcast::Receiver`. Dropping the entry from the registry
//! drops the `SubscriptionHandle`, which in turn `DecRef`s upstream
//! through the router's RAII guard.

use std::sync::Arc;

use midas_broker_core::market_data::{Bar, Tick};
use midas_broker_core::{SymbolKey, Timeframe};
use midas_core::ChartId;
use midas_market_data::SubscriptionHandle;
use tokio::sync::broadcast;
use tokio::sync::Mutex;

/// Composite key for a chart's bar subscription.
#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub struct ChartKey {
    pub symbol: SymbolKey,
    pub timeframe: Timeframe,
    pub chart_id: ChartId,
}

/// Registry entry — owns the `SubscriptionHandle` guard and exposes
/// `resubscribe()` to hand out fresh broadcast receivers.
pub struct BarEntry {
    inner: Mutex<SubscriptionHandle<Bar>>,
}

impl BarEntry {
    pub fn new(handle: SubscriptionHandle<Bar>) -> Self {
        Self {
            inner: Mutex::new(handle),
        }
    }

    /// Hand out a fresh broadcast receiver that shares the registry
    /// entry's guard. Safe as long as `self` outlives the receiver —
    /// the registry guarantees that.
    pub async fn resubscribe(&self) -> broadcast::Receiver<Arc<Bar>> {
        let guard = self.inner.lock().await;
        guard.resubscribe()
    }
}

/// Registry entry for a ticker-tick subscription.
pub struct TickEntry {
    inner: Mutex<SubscriptionHandle<Tick>>,
}

impl TickEntry {
    pub fn new(handle: SubscriptionHandle<Tick>) -> Self {
        Self {
            inner: Mutex::new(handle),
        }
    }

    pub async fn resubscribe(&self) -> broadcast::Receiver<Arc<Tick>> {
        let guard = self.inner.lock().await;
        guard.resubscribe()
    }
}

/// Install a chart bar handle into the context. Replaces any
/// previous entry for the same key, dropping its guard.
pub fn install_chart_handle(key: ChartKey, handle: SubscriptionHandle<Bar>) {
    // Slice F1: floating-chart synthetic ids set bit 31 to disambiguate
    // them from real `ChartId`s. With floating_charts retired, no caller
    // should ever produce such an id; this assert traps regressions
    // before they can leak into the registry.
    debug_assert!(
        key.chart_id.0 & (1u32 << 31) == 0,
        "ChartId with bit 31 set installed into CHART_REGISTRY (floating-chart synthetic-id pattern)"
    );
    if let Some(ctx) = super::subscription_context::current() {
        ctx.charts.insert(key, Arc::new(BarEntry::new(handle)));
    }
}

/// Remove a chart bar handle — called from the chart-close and
/// symbol/tf switch paths. Dropping the entry `DecRef`s upstream.
pub fn remove_chart_handle(key: &ChartKey) {
    if let Some(ctx) = super::subscription_context::current() {
        ctx.charts.remove(key);
    }
}

/// Remove every chart bar handle bound to `chart_id`, regardless of
/// symbol / timeframe. Used by the chart-close paths, which don't
/// know the exact `(sym, tf)` pairing(s) the chart accumulated over
/// its lifetime. Each removed entry drops its `SubscriptionHandle`
/// which `DecRef`s upstream through the router's RAII guard.
pub fn remove_chart_handles_for_chart(chart_id: ChartId) {
    if let Some(ctx) = super::subscription_context::current() {
        ctx.charts.retain(|k, _| k.chart_id != chart_id);
    }
}

/// Look up a chart bar handle (shared-ref to the entry, not a clone
/// of the inner handle — the entry owns the guard).
pub fn get_chart_handle(key: &ChartKey) -> Option<Arc<BarEntry>> {
    super::subscription_context::current().and_then(|ctx| ctx.charts.get(key).map(|r| r.clone()))
}

/// Install a ticker tick handle into the context.
pub fn install_ticker_handle(sym: SymbolKey, handle: SubscriptionHandle<Tick>) {
    if let Some(ctx) = super::subscription_context::current() {
        ctx.tickers.insert(sym, Arc::new(TickEntry::new(handle)));
    }
}

/// Remove a ticker tick handle — called when the last chart /
/// ticker-state consumer for a symbol goes away.
pub fn remove_ticker_handle(sym: &SymbolKey) {
    if let Some(ctx) = super::subscription_context::current() {
        ctx.tickers.remove(sym);
    }
}

/// Look up a ticker tick handle.
pub fn get_ticker_handle(sym: &SymbolKey) -> Option<Arc<TickEntry>> {
    super::subscription_context::current().and_then(|ctx| ctx.tickers.get(sym).map(|r| r.clone()))
}

/// Install the process-scoped router inside a fresh
/// [`SubscriptionContext`]. First call wins.
pub fn install_router(router: Arc<midas_market_data::MarketDataRouter>) {
    super::subscription_context::install(router);
}

/// Accessor for the process-scoped router, if a context has been
/// installed. Defers to [`super::subscription_context::router`].
pub fn router() -> Option<Arc<midas_market_data::MarketDataRouter>> {
    super::subscription_context::router()
}

#[cfg(test)]
mod tests {
    //! Unit coverage for the chart-registry cleanup helpers.
    //!
    //! The full `MidasApp` bind-then-switch regression is expensive
    //! to stand up (requires a `SimMarketData` router and an iced
    //! runtime), so we isolate the cleanup contract here: a chart
    //! that rebinds from one `(symbol, timeframe)` to another must
    //! leave exactly one registry entry behind, not two.
    use midas_broker_core::Timeframe;
    use midas_market_data::{MarketDataRouter, SubscriptionHandle};
    use std::sync::Arc;
    use tokio::runtime::Runtime;

    use super::*;

    /// Brew a real `SubscriptionHandle<Bar>` off a `SimMarketData`
    /// router so the guard's `DecRef` path survives the test —
    /// synthesising a fake `Guard` would let the test pass even if
    /// the DecRef wiring was broken upstream.
    async fn make_bar_handle(
        router: &Arc<MarketDataRouter>,
        sym: &str,
        tf: Timeframe,
    ) -> SubscriptionHandle<midas_broker_core::market_data::Bar> {
        let key = midas_broker_core::SymbolKey {
            contract_id: 0,
            symbol: sym.to_string(),
        };
        router
            .subscribe_bars(key, tf)
            .await
            .expect("subscribe_bars")
    }

    fn build_test_router() -> Arc<MarketDataRouter> {
        use midas_broker::sim::{SimMarketData, SimMarketDataConfig};
        let sim = SimMarketData::new(SimMarketDataConfig {
            farm_up_delay_ms: 1,
            burst_enabled: false,
            tick_drift_bps: 0.0,
            tick_cadence_ms: 60_000,
            ..SimMarketDataConfig::default()
        });
        let src: Arc<dyn midas_broker::MarketDataSource> = sim.clone();
        MarketDataRouter::new(src)
    }

    /// Build a fresh router and install it into the process-scoped
    /// context for the duration of this test. The caller holds
    /// [`REGISTRY_TEST_LOCK`] for serialisation and calls
    /// `clear_for_test` in teardown.
    fn install_test_ctx() -> Arc<MarketDataRouter> {
        let router = build_test_router();
        super::super::subscription_context::install_for_test(router.clone());
        router
    }

    /// Serialise the two registry tests. Each test installs a fresh
    /// `SubscriptionContext` + tokio `Runtime`; running them
    /// concurrently would leave one test's router attached to the
    /// other's tokio runtime, which is killed on `Runtime::drop`.
    /// The mutex makes the install→use→clear sequence atomic.
    static REGISTRY_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn remove_chart_handles_for_chart_evicts_every_entry_for_that_chart() {
        let _guard = REGISTRY_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let rt = Runtime::new().expect("tokio rt");
        rt.block_on(async {
            let router = install_test_ctx();
            let chart_id = ChartId::new(9_001);
            let other_id = ChartId::new(9_002);
            let aapl = midas_broker_core::SymbolKey {
                contract_id: 0,
                symbol: "AAPL_REG_T1".to_string(),
            };
            let msft = midas_broker_core::SymbolKey {
                contract_id: 0,
                symbol: "MSFT_REG_T1".to_string(),
            };

            // Two entries for `chart_id` (different symbols), one
            // entry for `other_id` that must survive.
            install_chart_handle(
                ChartKey {
                    symbol: aapl.clone(),
                    timeframe: Timeframe::M1,
                    chart_id,
                },
                make_bar_handle(&router, "AAPL_REG_T1", Timeframe::M1).await,
            );
            install_chart_handle(
                ChartKey {
                    symbol: msft.clone(),
                    timeframe: Timeframe::M1,
                    chart_id,
                },
                make_bar_handle(&router, "MSFT_REG_T1", Timeframe::M1).await,
            );
            install_chart_handle(
                ChartKey {
                    symbol: aapl.clone(),
                    timeframe: Timeframe::M1,
                    chart_id: other_id,
                },
                make_bar_handle(&router, "AAPL_REG_T1", Timeframe::M1).await,
            );

            remove_chart_handles_for_chart(chart_id);

            assert!(get_chart_handle(&ChartKey {
                symbol: aapl.clone(),
                timeframe: Timeframe::M1,
                chart_id,
            })
            .is_none());
            assert!(get_chart_handle(&ChartKey {
                symbol: msft.clone(),
                timeframe: Timeframe::M1,
                chart_id,
            })
            .is_none());
            assert!(get_chart_handle(&ChartKey {
                symbol: aapl.clone(),
                timeframe: Timeframe::M1,
                chart_id: other_id,
            })
            .is_some());

            // Cleanup so later tests that share the global
            // registry don't observe leftover entries.
            remove_chart_handles_for_chart(other_id);
        });
        super::super::subscription_context::clear_for_test();
    }

    #[test]
    fn bind_switch_leaves_one_entry_not_two() {
        // Direct cover for Bug 1: a chart that rebinds from AAPL@M1
        // to MSFT@M1 must leave exactly one CHART_REGISTRY entry,
        // not two. We emulate `bind_chart_to_symbol` by
        // install-then-evict-then-install.
        let _guard = REGISTRY_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let rt = Runtime::new().expect("tokio rt");
        rt.block_on(async {
            let router = install_test_ctx();
            let chart_id = ChartId::new(9_101);
            let aapl = midas_broker_core::SymbolKey {
                contract_id: 0,
                symbol: "AAPL_BIND_T1".to_string(),
            };
            let msft = midas_broker_core::SymbolKey {
                contract_id: 0,
                symbol: "MSFT_BIND_T1".to_string(),
            };

            install_chart_handle(
                ChartKey {
                    symbol: aapl.clone(),
                    timeframe: Timeframe::M1,
                    chart_id,
                },
                make_bar_handle(&router, "AAPL_BIND_T1", Timeframe::M1).await,
            );
            // Switching to MSFT: the bind path evicts the old
            // (AAPL, M1, chart_id) entry, then installs the new
            // (MSFT, M1, chart_id) entry.
            remove_chart_handle(&ChartKey {
                symbol: aapl.clone(),
                timeframe: Timeframe::M1,
                chart_id,
            });
            install_chart_handle(
                ChartKey {
                    symbol: msft.clone(),
                    timeframe: Timeframe::M1,
                    chart_id,
                },
                make_bar_handle(&router, "MSFT_BIND_T1", Timeframe::M1).await,
            );

            let ctx = super::super::subscription_context::current().expect("ctx installed");
            let live = ctx
                .charts
                .iter()
                .filter(|r| r.key().chart_id == chart_id)
                .count();
            assert_eq!(
                live, 1,
                "after symbol switch exactly one registry entry must remain"
            );

            remove_chart_handles_for_chart(chart_id);
        });
        super::super::subscription_context::clear_for_test();
    }
}
