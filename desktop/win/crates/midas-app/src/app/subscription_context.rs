//! Consolidated subscription context (audit P1 refactor 3).
//!
//! The per-consumer subscription machinery used to live in four
//! independent module-scoped mutable globals:
//!
//! * `subscription_registry::CHART_REGISTRY` — per-chart bar handles
//! * `subscription_registry::TICKER_REGISTRY` — per-symbol tick handles
//! * `watchlist_subscription::WATCHLIST_REGISTRY` — per-symbol quote
//!   handles
//! * `subscription_registry::ROUTER` — `Arc<MarketDataRouter>`
//!
//! Four globals across three files, each with its own set of
//! `install_* / get_* / remove_*` helpers. That is connascence of
//! name across ~8 call sites: callers had to know which module owned
//! which static. Parallel test runs could not observe independent
//! registries, and a process shutdown had no single "drop everything
//! subscription-related" entry point.
//!
//! [`SubscriptionContext`] consolidates the four statics into one
//! struct held behind a single [`OnceLock`]. The original helper
//! surface in `subscription_registry` and `watchlist_subscription`
//! becomes thin shims over [`SubscriptionContext::current`] — so no
//! call site churn today, but the pattern is now trivially
//! extendable (single place to add new subscription registries,
//! single place to drop state at shutdown).

use std::sync::{Arc, RwLock};

use dashmap::DashMap;
use midas_broker_core::SymbolKey;
use midas_market_data::MarketDataRouter;

use super::subscription_registry::{BarEntry, ChartKey, TickEntry};
use super::watchlist_subscription::QuoteEntry;

/// Process-scoped subscription hub — one per app instance.
///
/// Holds every registry keyed by its subscription kind plus the
/// shared `Arc<MarketDataRouter>`. Installed by
/// `MidasApp::new` (sim path) or after `Message::RouterReady` (IB
/// path); looked up by every `fn`-pointer stream builder to resolve
/// the router without capturing it in the closure.
pub struct SubscriptionContext {
    pub router: Arc<MarketDataRouter>,
    pub charts: DashMap<ChartKey, Arc<BarEntry>>,
    pub tickers: DashMap<SymbolKey, Arc<TickEntry>>,
    pub watchlist: DashMap<SymbolKey, Arc<QuoteEntry>>,
}

impl SubscriptionContext {
    /// Build a fresh context wrapping `router` with empty registries.
    pub fn new(router: Arc<MarketDataRouter>) -> Arc<Self> {
        Arc::new(Self {
            router,
            charts: DashMap::new(),
            tickers: DashMap::new(),
            watchlist: DashMap::new(),
        })
    }
}

// `RwLock<Option<_>>` not `OnceLock<_>`: the production lifecycle is
// still "install once", but test code needs to swap the context so
// each `#[test]` fn can stand up its own router + tokio runtime
// without leaking task handles into sibling tests.
static CTX: RwLock<Option<Arc<SubscriptionContext>>> = RwLock::new(None);

/// Install the process-scoped context. First production call wins
/// (mirrors the old `ROUTER`/`LazyLock<DashMap>` semantics); any
/// subsequent production install is a silent no-op. Test code uses
/// [`install_for_test`] / [`clear_for_test`] to swap instead.
pub fn install(router: Arc<MarketDataRouter>) {
    let mut guard = CTX.write().expect("subscription context RwLock poisoned");
    if guard.is_some() {
        return;
    }
    *guard = Some(SubscriptionContext::new(router));
}

/// Fetch the current process-scoped context, or `None` if the router
/// hasn't been installed yet (subscription builders race the
/// `RouterReady` handshake on IB boot).
pub fn current() -> Option<Arc<SubscriptionContext>> {
    CTX.read()
        .expect("subscription context RwLock poisoned")
        .clone()
}

/// Convenience: the current router, if any. Shorter than
/// `current().map(|c| c.router.clone())`.
pub fn router() -> Option<Arc<MarketDataRouter>> {
    current().map(|c| c.router.clone())
}

/// Test-only: replace the process-scoped context with a fresh one
/// around `router`, dropping any previously-installed context. Gated
/// to `#[cfg(test)]` so production code can't accidentally swap the
/// context mid-flight.
#[cfg(test)]
pub fn install_for_test(router: Arc<MarketDataRouter>) {
    let mut guard = CTX.write().expect("subscription context RwLock poisoned");
    *guard = Some(SubscriptionContext::new(router));
}

/// Test-only: clear the process-scoped context. Pairs with
/// [`install_for_test`] in test teardown.
#[cfg(test)]
pub fn clear_for_test() {
    *CTX.write().expect("subscription context RwLock poisoned") = None;
}
