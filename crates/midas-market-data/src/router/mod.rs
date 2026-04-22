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

/// Local re-statement of the provider trait (imported from
/// `midas-broker::MarketDataSource`) so the router doesn't need a
/// compile-time dependency on `midas-broker`. In practice callers
/// pass `Arc<dyn midas_broker::MarketDataSource>` which trivially
/// implements this local alias thanks to the identical method set.
///
/// In S5 we simply re-use the `midas-broker` trait; this alias exists
/// to document intent. If a future refactor pushes the provider trait
/// into `midas-broker-core`, this alias retargets there.
pub use midas_broker::MarketDataSource as MarketDataSourceTrait;

/// Refcounted per-symbol market-data router.
///
/// Construct with [`MarketDataRouter::new`]. Cheap to clone-reach —
/// every method goes through a `&Arc<Self>` so concurrent subscribers
/// simply bump the refcount on the returned handle, not on the router
/// itself.
pub struct MarketDataRouter {
    state: Arc<RouterState>,
    control: mpsc::UnboundedSender<RouterMsg>,
}

impl MarketDataRouter {
    /// Build a new router backed by `source`. Spawns the control-plane
    /// actor eagerly so subscribe/unsubscribe work immediately.
    ///
    /// Construction uses [`Arc::new_cyclic`] (NM-4) so a `Weak<Self>`
    /// can be stashed in [`RouterState::weak_self`] for S6's aggregator
    /// registry.
    pub fn new(source: DynMarketDataSource) -> Arc<Self> {
        Arc::new_cyclic(|weak_self| {
            let (control_tx, control_rx) = mpsc::unbounded_channel();
            let state = Arc::new(RouterState {
                source,
                per_symbol: dashmap::DashMap::new(),
                contract_cache: dashmap::DashMap::new(),
                weak_self: weak_self.clone(),
            });
            let actor_state = state.clone();
            let actor_control = control_tx.clone();
            tokio::spawn(run_control_actor(control_rx, actor_state, actor_control));
            Self {
                state,
                control: control_tx,
            }
        })
    }

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
        self.control
            .send(RouterMsg::SubscribeTicks {
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
        self.control
            .send(RouterMsg::SubscribeRtBars {
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
        self.control
            .send(RouterMsg::OpenHubForWatch {
                symbol,
                reply: reply_tx,
            })
            .map_err(|_| MarketDataError::ShuttingDown)?;
        reply_rx.await.map_err(|_| MarketDataError::ShuttingDown)?
    }

    /// Subscribe to aggregated bars at `tf` for `symbol`.
    ///
    /// **OPEN(S6)**: the aggregator registry is not yet wired. In S5
    /// this method returns `Err(MarketDataError::Other(...))` for
    /// timeframes other than [`Timeframe::S5`]; `S5` passes through
    /// to [`Self::subscribe_rt_bars`] since IB's RT bar cadence is
    /// already 5 s. This keeps the public surface stable while S6
    /// adds the aggregator.
    pub async fn subscribe_bars(
        &self,
        symbol: SymbolKey,
        tf: Timeframe,
    ) -> Result<SubscriptionHandle<Bar>, MarketDataError> {
        if matches!(tf, Timeframe::S5) {
            return self.subscribe_rt_bars(symbol).await;
        }
        Err(MarketDataError::Other(
            "aggregator not yet implemented; tracked as OPEN(S6)".to_string(),
        ))
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
            .control
            .send(RouterMsg::DebugDump { reply: reply_tx })
            .is_err()
        {
            return Vec::new();
        }
        reply_rx.await.unwrap_or_default()
    }

    /// Accessor for the underlying provider. Pub-crate only — used by
    /// the seam utility; external callers shouldn't reach through.
    pub(crate) fn source(&self) -> &dyn MarketDataSourceTrait {
        &*self.state.source
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
        let _ = self.control.send(RouterMsg::Shutdown);
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
