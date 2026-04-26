//! [`MarketDataRouter`] — refcounted per-symbol fan-out hub.
//!
//! The router sits between the provider layer (`midas-broker`) and
//! app-side consumers. One tick upstream from the provider is fanned
//! out to every subscribed consumer via a per-symbol broadcast; a
//! separate per-symbol `watch::Sender<Quote>` coalesces the same ticks
//! into a "last known quote" snapshot suitable for watchlist cells.
//!
//! Every consumer holds its own RAII [`SubscriptionHandle`]; dropping
//! the handle (or the stream it was folded into) sends a `DecRef` to
//! the router's control actor, which tears the upstream subscription
//! down when the last consumer leaves (BR-3 / NB-1).
//!
//! See `plan/market-data-router/06-slice-5-router.md` for the full
//! design and `plan/market-data-router/01-architecture.md` for the
//! hot/cold-path contract.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use futures::Stream;
use midas_broker_core::market_data::{
    Bar, ConnectionState, ContractDetails, FarmStatus, IbDuration, MarketDataError, SymbolKey,
    Tick, Timeframe,
};
use tokio::sync::{broadcast, mpsc, oneshot, watch};

pub mod actor;
pub mod contract_cache;
pub mod handle;
pub mod history_seam;
pub mod publisher;
pub mod state;

pub use actor::SymbolDebugInfo;
pub use handle::{Guard, GuardedStream, QuoteHandle, SubscriptionHandle};

use actor::{run_control_actor, RouterMsg};
use state::RouterState;

/// Object-safe alias for an `Arc<dyn MarketDataSource>`.
///
/// Re-exported from the router module so downstream callers don't need
/// a direct dependency on `midas-broker` to hold a boxed source — they
/// can work against this alias instead.
pub type DynMarketDataSource = Arc<dyn MarketDataSourceTrait>;

/// Re-export of the provider trait from `midas-broker-core::provider`.
///
/// Post-audit-P1 the trait lives in the neutral core crate so the
/// router no longer depends on `midas-broker`. The alias name is kept
/// to minimise churn at existing call sites inside `midas-market-data`.
pub use midas_broker_core::provider::MarketDataSource as MarketDataSourceTrait;

/// Refcounted per-symbol market-data router.
///
/// Construct with [`MarketDataRouter::new`]. Cheap to clone-reach —
/// every method goes through a `&Arc<Self>` so concurrent subscribers
/// simply bump the refcount on the returned handle, not on the router
/// itself.
pub struct MarketDataRouter {
    state: Arc<RouterState>,
    control: mpsc::UnboundedSender<RouterMsg>,
    /// Shared backlog counter (mirror of `state.backlog`). Hoisted to
    /// the top-level so the `send_control` helper can bump it without
    /// re-reaching into the `Arc<RouterState>`.
    backlog: Arc<AtomicUsize>,
}

impl MarketDataRouter {
    /// Build a new router backed by `source`. Spawns the control-plane
    /// actor eagerly so subscribe/unsubscribe work immediately.
    ///
    /// Construction uses [`Arc::new_cyclic`] (NM-4) so a `Weak<Self>`
    /// can be stashed in [`RouterState::weak_self`] and handed to the
    /// [`BarAggregatorRegistry`] for S6 without forming a reference
    /// cycle.
    ///
    /// [`BarAggregatorRegistry`]: crate::aggregator::BarAggregatorRegistry
    pub fn new(source: DynMarketDataSource) -> Arc<Self> {
        Arc::new_cyclic(|weak_self| {
            let (control_tx, control_rx) = mpsc::unbounded_channel();
            // The registry itself wraps a fresh Arc via its own
            // Arc::new_cyclic so its weak_self is populated before any
            // subscribe call runs; passing in the router's weak is
            // safe because `Arc::new_cyclic` guarantees `weak_self` is
            // already a valid `Weak<Self>` here.
            let registry = crate::aggregator::BarAggregatorRegistry::new(weak_self.clone());
            let backlog = Arc::new(AtomicUsize::new(0));
            let backlog_warned = Arc::new(AtomicBool::new(false));
            let state = Arc::new(RouterState {
                source,
                per_symbol: dashmap::DashMap::new(),
                contract_cache: dashmap::DashMap::new(),
                weak_self: weak_self.clone(),
                aggregator_registry: registry,
                backlog: Arc::clone(&backlog),
                backlog_warned: Arc::clone(&backlog_warned),
                previously_disconnected: parking_lot::Mutex::new(std::collections::HashSet::new()),
            });
            let actor_state = state.clone();
            let actor_control = control_tx.clone();
            tokio::spawn(run_control_actor(control_rx, actor_state, actor_control));
            Self {
                state,
                control: control_tx,
                backlog,
            }
        })
    }

    /// Send a message onto the control mpsc and bump the backlog
    /// counter. Returns the same `Result` shape as
    /// [`mpsc::UnboundedSender::send`] — `Err(SendError<RouterMsg>)` if
    /// the actor has already exited. Callers that need to distinguish
    /// between a normal shutdown and a successful enqueue should keep
    /// using this one helper.
    fn send_control(&self, msg: RouterMsg) -> Result<(), mpsc::error::SendError<RouterMsg>> {
        bump_backlog(&self.backlog, &self.state.backlog_warned);
        match self.control.send(msg) {
            Ok(()) => Ok(()),
            Err(e) => {
                // Undo the optimistic bump so a stale-Router drop
                // doesn't leave the counter permanently skewed above
                // the warn threshold on the next live router.
                self.backlog.fetch_sub(1, Ordering::Relaxed);
                Err(e)
            }
        }
    }
}

/// Bump the shared `backlog` counter and log once if we cross
/// [`actor::ROUTER_BACKLOG_WARN`]. Logged at `warn` level, sticky for
/// the lifetime of the router (no flapping). Exposed at module scope
/// so the per-guard Drop helpers can share the logic.
pub(crate) fn bump_backlog(backlog: &Arc<AtomicUsize>, warned: &Arc<AtomicBool>) {
    let depth = backlog.fetch_add(1, Ordering::Relaxed) + 1;
    if depth >= actor::ROUTER_BACKLOG_WARN && !warned.swap(true, Ordering::Relaxed) {
        tracing::warn!(
            backlog = depth,
            threshold = actor::ROUTER_BACKLOG_WARN,
            "router control-plane backlog crossed warn threshold; upstream may be hung"
        );
    }
}

impl MarketDataRouter {
    /// Subscribe to the per-symbol tick fan-out.
    ///
    /// First subscriber per symbol triggers `source.subscribe_ticks`;
    /// subsequent subscribers share the broadcast. The returned handle
    /// DecRefs on drop — when the last one drops, the publisher task
    /// is aborted and the upstream is cancelled (NB-7).
    pub async fn subscribe_ticks(
        &self,
        symbol: SymbolKey,
    ) -> Result<SubscriptionHandle<Tick>, MarketDataError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.send_control(RouterMsg::SubscribeTicks {
            symbol,
            reply: reply_tx,
        })
        .map_err(|_| MarketDataError::ShuttingDown)?;
        reply_rx.await.map_err(|_| MarketDataError::ShuttingDown)?
    }

    /// Subscribe to the per-symbol realtime-bar fan-out (NB-6 Model A).
    ///
    /// Multiple aggregators (different timeframes on the same symbol)
    /// share ONE upstream `subscribe_realtime_bars` request through
    /// this fan-out.
    pub async fn subscribe_rt_bars(
        &self,
        symbol: SymbolKey,
    ) -> Result<SubscriptionHandle<Bar>, MarketDataError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.send_control(RouterMsg::SubscribeRtBars {
            symbol,
            reply: reply_tx,
        })
        .map_err(|_| MarketDataError::ShuttingDown)?;
        reply_rx.await.map_err(|_| MarketDataError::ShuttingDown)?
    }

    /// Lazy-open a quote watch for the symbol (NB-3).
    ///
    /// If no hub exists for the symbol yet, a tick publisher is
    /// spawned so the watch actually receives updates — but no
    /// broadcast receiver is issued. The returned [`QuoteHandle`]
    /// holds a [`WatchGuard`] that `DecWatchRef`s on drop.
    ///
    /// [`WatchGuard`]: handle::WatchGuard
    pub async fn last_quote(&self, symbol: SymbolKey) -> Result<QuoteHandle, MarketDataError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.send_control(RouterMsg::OpenHubForWatch {
            symbol,
            reply: reply_tx,
        })
        .map_err(|_| MarketDataError::ShuttingDown)?;
        reply_rx.await.map_err(|_| MarketDataError::ShuttingDown)?
    }

    /// Subscribe to aggregated bars at `tf` for `symbol`.
    ///
    /// Delegates to [`BarAggregatorRegistry::subscribe`] (S6). The
    /// registry rejects unsupported timeframes (`S1`, `H4`, `D1`, `W1`,
    /// `MN1`) with [`MarketDataError::UnsupportedTimeframe`]. Supported
    /// set: `S5`, `S15`, `S30`, `M1`, `M5`, `M15`, `M30`, `H1`.
    ///
    /// All consumers on the same `(symbol, tf)` share ONE aggregator
    /// task and ONE broadcast; two different `tf` values on the same
    /// `symbol` share ONE upstream RT-bar subscription (NB-6 Model A).
    ///
    /// [`BarAggregatorRegistry::subscribe`]: crate::aggregator::BarAggregatorRegistry::subscribe
    pub async fn subscribe_bars(
        &self,
        symbol: SymbolKey,
        tf: Timeframe,
    ) -> Result<SubscriptionHandle<Bar>, MarketDataError> {
        self.state.aggregator_registry.subscribe(symbol, tf).await
    }

    /// Snapshot accessor into the aggregator registry.
    ///
    /// Thin passthrough to
    /// [`BarAggregatorRegistry::last_bar`](crate::aggregator::BarAggregatorRegistry::last_bar).
    /// Used by `ChartResync` after a consumer observes `Lagged`.
    pub async fn last_bar(&self, symbol: &SymbolKey, tf: Timeframe) -> Option<Bar> {
        self.state.aggregator_registry.last_bar(symbol, tf).await
    }

    /// Accessor for the aggregator registry.
    ///
    /// Hidden from public docs; used by behaviour tests and future
    /// dev-harness probes (S7) to inspect aggregator state without
    /// racing the subscribe/drop plumbing.
    #[doc(hidden)]
    pub fn aggregator_registry(&self) -> &std::sync::Arc<crate::aggregator::BarAggregatorRegistry> {
        &self.state.aggregator_registry
    }

    /// History + live seam (BR-7 rewrite, NB-2).
    ///
    /// Returns a single chained stream: all historical bars first,
    /// then live RT bars filtered by `ts_open > t_server` so no
    /// boundary duplicate slips through (M-35).
    ///
    /// The returned stream owns the live handle's guard via
    /// [`SubscriptionHandle::into_stream`]; dropping the stream
    /// `DecRef`s upstream.
    pub async fn history_then_live(
        &self,
        symbol: SymbolKey,
        tf: Timeframe,
        duration: IbDuration,
    ) -> Result<impl Stream<Item = Bar> + Send + 'static, MarketDataError> {
        history_seam::history_then_live_impl(self, symbol, tf, duration).await
    }

    /// One-shot historical fetch through the router's own provider.
    ///
    /// Resolves `con_id` via the router's contract cache, then calls
    /// [`MarketDataSource::historical_bars`] with `end = Utc::now()`
    /// and `WhatToShow::Trades`. Use this when callers need just the
    /// historical tail (no live chain) — e.g. the desktop chart's
    /// initial data load that unifies with the watchlist's sim
    /// source instead of a disjoint synthetic generator.
    ///
    /// `use_rth` selects regular-trading-hours-only (`true`) vs.
    /// extended-hours-included (`false`). On the IB backend this maps
    /// to the `useRTH` flag of `reqHistoricalData`, which controls
    /// whether the response includes 04:00–09:30 ET pre-market and
    /// 16:00–20:00 ET post-market bars. Per
    /// `plan/session-aware-charts/eth-shading.md` §D the desktop
    /// chart load passes `!show_extended_hours` so the user knob
    /// directly drives the request, while the watchlist snapshot
    /// load keeps `true` (we only ever want a stable last-RTH-close
    /// for the row).
    ///
    /// Callers that need history + live should prefer
    /// [`Self::history_then_live`].
    pub async fn historical_bars(
        &self,
        symbol: SymbolKey,
        tf: Timeframe,
        duration: IbDuration,
        use_rth: bool,
    ) -> Result<midas_broker_core::provider::HistoricalBarsResult, MarketDataError> {
        let con_id = self.resolve_or_cached(&symbol).await?.contract_id;
        let end = chrono::Utc::now();
        self.source()
            .historical_bars(
                &symbol,
                con_id,
                end,
                duration,
                tf,
                midas_broker_core::market_data::WhatToShow::Trades,
                use_rth,
            )
            .await
    }

    /// Clone of the upstream farm-status broadcast.
    pub fn farm_status(&self) -> broadcast::Receiver<FarmStatus> {
        self.state.source.farm_status()
    }

    /// Clone of the upstream connection-state watch.
    pub fn connection_state(&self) -> watch::Receiver<ConnectionState> {
        self.state.source.connection_state()
    }

    /// Observability snapshot (M-28).
    ///
    /// Collects per-symbol refcounts, publisher liveness, and
    /// last-tick timestamps. Used by `dev_harness::DumpState` and by
    /// the behavior tests.
    pub async fn debug_dump(&self) -> Vec<SymbolDebugInfo> {
        let (reply_tx, reply_rx) = oneshot::channel();
        if self
            .send_control(RouterMsg::DebugDump { reply: reply_tx })
            .is_err()
        {
            return Vec::new();
        }
        reply_rx.await.unwrap_or_default()
    }

    /// Current pending-message depth on the control mpsc. Exposed
    /// crate-wide so behaviour tests can assert the counter drains to
    /// zero once all subscribes/unsubscribes have settled, and so
    /// operators / `dev_harness` probes can surface the value without
    /// reaching into the internals.
    #[doc(hidden)]
    pub fn control_backlog(&self) -> usize {
        self.backlog.load(Ordering::Relaxed)
    }

    /// Accessor for the underlying provider. Pub-crate only — used by
    /// the seam utility; external callers shouldn't reach through.
    pub(crate) fn source(&self) -> &dyn MarketDataSourceTrait {
        &*self.state.source
    }

    /// Test-only accessor for the underlying provider (S8d).
    ///
    /// Returns the same `Arc<dyn MarketDataSource>` the router holds
    /// internally so callers can reach the provider's
    /// `inject_for_test(event)` method. Gated on the `test_inject`
    /// Cargo feature (plus `cfg(test)`) so production builds of the
    /// crate never see it — the feature is forwarded to
    /// `midas-broker/test_inject` by Cargo so the provider-side
    /// `inject_for_test` stays compiled in.
    ///
    /// Real IB sources' `inject_for_test` is a no-op; only
    /// `SimMarketData` routes events through its hubs.
    #[cfg(any(test, feature = "test_inject"))]
    pub fn source_for_test(&self) -> DynMarketDataSource {
        Arc::clone(&self.state.source)
    }

    /// Memoised `resolve_contract`. Pub-crate only — used by the
    /// seam utility.
    pub(crate) async fn resolve_or_cached(
        &self,
        sym: &SymbolKey,
    ) -> Result<ContractDetails, MarketDataError> {
        contract_cache::resolve_or_cached(&self.state, sym).await
    }
}

impl Drop for MarketDataRouter {
    fn drop(&mut self) {
        // Fire-and-forget shutdown. Actor may already be draining.
        // Bump the backlog so the actor's post-recv decrement stays
        // balanced — otherwise the counter could wrap on a borderline
        // shutdown.
        bump_backlog(&self.backlog, &self.state.backlog_warned);
        if self.control.send(RouterMsg::Shutdown).is_err() {
            self.backlog.fetch_sub(1, Ordering::Relaxed);
        }
    }
}

impl std::fmt::Debug for MarketDataRouter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MarketDataRouter")
            .field("source", &self.state.source.name())
            .field("live_symbols", &self.state.per_symbol.len())
            .finish()
    }
}

// Document: the router is not itself cloneable — consumers hold
// `Arc<MarketDataRouter>` and share. `SubscriptionHandle<T>: !Clone`
// is enforced by deliberate omission of any `Clone` impl.
