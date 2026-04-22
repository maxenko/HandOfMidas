// S7a: the registry skeleton is wired in this commit so the
// infrastructure compiles and is reachable from subsequent slices;
// the actual bind call-sites land in S7b (charts) and S7d (ticker
// states). Suppressing dead_code here keeps the S7a commit clean
// without hacking a feature flag onto every identifier.
#![allow(dead_code)]

//! Keyed registry of live router subscription handles.
//!
//! `Subscription::run_with` requires a `fn` pointer (not a closure),
//! so the per-instance `SubscriptionHandle<Bar>` / `SubscriptionHandle<Tick>`
//! returned from the router cannot be captured directly. Instead, the
//! app subscribe path (S7b: chart bind; S7d: ticker activate) parks
//! the handle in a static registry keyed by a hashable identifier,
//! and the builder function looks it up inside the closure.
//!
//! The handle lives behind an `Arc<Mutex<_>>` so the registry keeps
//! the guard alive for the lifetime of the keyed entry while each
//! subscription-stream invocation calls `resubscribe()` to obtain a
//! fresh `broadcast::Receiver`. Dropping the entry from the registry
//! drops the `SubscriptionHandle`, which in turn `DecRef`s upstream
//! through the router's RAII guard.

use std::sync::{Arc, LazyLock};

use dashmap::DashMap;
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

/// Global bar-subscription registry.
pub static CHART_REGISTRY: LazyLock<DashMap<ChartKey, Arc<BarEntry>>> = LazyLock::new(DashMap::new);

/// Global tick-subscription registry.
pub static TICKER_REGISTRY: LazyLock<DashMap<SymbolKey, Arc<TickEntry>>> =
    LazyLock::new(DashMap::new);

/// Install a chart bar handle into the registry. Replaces any
/// previous entry for the same key, dropping its guard.
pub fn install_chart_handle(key: ChartKey, handle: SubscriptionHandle<Bar>) {
    CHART_REGISTRY.insert(key, Arc::new(BarEntry::new(handle)));
}

/// Remove a chart bar handle — called from the chart-close and
/// symbol/tf switch paths. Dropping the entry `DecRef`s upstream.
pub fn remove_chart_handle(key: &ChartKey) {
    CHART_REGISTRY.remove(key);
}

/// Look up a chart bar handle (shared-ref to the entry, not a clone
/// of the inner handle — the entry owns the guard).
pub fn get_chart_handle(key: &ChartKey) -> Option<Arc<BarEntry>> {
    CHART_REGISTRY.get(key).map(|r| r.clone())
}

/// Install a ticker tick handle into the registry.
pub fn install_ticker_handle(sym: SymbolKey, handle: SubscriptionHandle<Tick>) {
    TICKER_REGISTRY.insert(sym, Arc::new(TickEntry::new(handle)));
}

/// Remove a ticker tick handle — called when the last chart /
/// ticker-state consumer for a symbol goes away.
pub fn remove_ticker_handle(sym: &SymbolKey) {
    TICKER_REGISTRY.remove(sym);
}

/// Look up a ticker tick handle.
pub fn get_ticker_handle(sym: &SymbolKey) -> Option<Arc<TickEntry>> {
    TICKER_REGISTRY.get(sym).map(|r| r.clone())
}

/// Global router pointer. Installed from `MidasApp::new` (Sim) or
/// after `Message::RouterReady` (IB) so the `fn`-pointer
/// subscription builders can resolve the router without capturing
/// it in the closure. `OnceLock` because the router is constructed
/// once per process and never replaced; subsequent install
/// attempts silently no-op.
static ROUTER: std::sync::OnceLock<Arc<midas_market_data::MarketDataRouter>> =
    std::sync::OnceLock::new();

pub fn install_router(router: Arc<midas_market_data::MarketDataRouter>) {
    let _ = ROUTER.set(router);
}

pub fn router() -> Option<Arc<midas_market_data::MarketDataRouter>> {
    ROUTER.get().cloned()
}
