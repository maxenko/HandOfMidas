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

use std::sync::{Arc, OnceLock};

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

static CTX: OnceLock<Arc<SubscriptionContext>> = OnceLock::new();

/// Install the process-scoped context. First call wins (mirrors the
/// old `ROUTER`/`LazyLock<DashMap>` semantics); subsequent calls are
/// silent no-ops.
pub fn install(router: Arc<MarketDataRouter>) {
    let _ = CTX.set(SubscriptionContext::new(router));
}

/// Fetch the current process-scoped context, or `None` if the router
/// hasn't been installed yet (subscription builders race the
/// `RouterReady` handshake on IB boot).
pub fn current() -> Option<Arc<SubscriptionContext>> {
    CTX.get().cloned()
}

/// Convenience: the current router, if any. Shorter than
/// `current().map(|c| c.router.clone())`.
pub fn router() -> Option<Arc<MarketDataRouter>> {
    CTX.get().map(|c| c.router.clone())
}
