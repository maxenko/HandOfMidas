//! Router control-plane state.
//!
//! [`RouterState`] carries the upstream provider + the per-symbol hub
//! map + the contract cache. [`SymbolHub`] is the per-symbol fan-out
//! record the publisher tasks capture at spawn (BR-18) — each hub
//! owns one tick broadcast, one quote watch, and an optional RT-bar
//! broadcast, plus three independent refcounts (NB-3) and the two
//! publisher [`JoinHandle`]s (NB-7).
//!
//! The hot publish path never touches the DashMap on this struct; the
//! publisher holds `Arc<SymbolHub>` directly. The DashMap is only
//! consulted by the control actor on subscribe/unsubscribe.

use std::sync::atomic::{AtomicI64, AtomicU32};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use dashmap::DashMap;
use midas_broker_core::market_data::{Bar, ContractDetails, Quote, SymbolKey, Tick};
use tokio::sync::{broadcast, watch};
use tokio::task::JoinHandle;

use crate::aggregator::BarAggregatorRegistry;
use crate::router::MarketDataRouter;

use super::DynMarketDataSource;

/// Default capacity of the per-symbol tick broadcast.
///
/// 4096 keeps the ring bounded but large enough to absorb ~16 s of
/// top-of-book activity on a liquid symbol (250 ticks/s) before a
/// lagging consumer starts to see `RecvError::Lagged`.
pub(crate) const TICKS_CAP: usize = 4096;

/// Default capacity of the per-symbol realtime-bar broadcast.
///
/// IB's RT bars fire once every 5 s; 256 slots is ~21 min of buffer.
pub(crate) const RT_BARS_CAP: usize = 256;

/// Number of consecutive "zero receivers" publisher ticks required
/// before auto-exit fires (M-4).
pub(crate) const PUBLISHER_AUTO_EXIT_STREAK: u32 = 16;

/// Router-wide state shared with the control actor + publisher tasks.
///
/// Publishers only ever hold `Arc<SymbolHub>` directly; they never
/// follow the `Arc<RouterState>.per_symbol` DashMap on the hot path.
pub(crate) struct RouterState {
    /// Upstream provider (sim or IB). Boxed trait object so the router
    /// is backend-agnostic.
    pub(crate) source: DynMarketDataSource,
    /// Live hubs, keyed by symbol. The actor is the sole mutator.
    pub(crate) per_symbol: DashMap<SymbolKey, Arc<SymbolHub>>,
    /// Memoised `resolve_contract` responses (NM-1). Separate from
    /// `per_symbol` because a hub can be torn down and later recreated
    /// without re-resolving the contract.
    pub(crate) contract_cache: DashMap<SymbolKey, ContractDetails>,
    /// Self-weak, captured via `Arc::new_cyclic` (NM-4). S6's
    /// aggregator registry upgrades this to reach back into the router
    /// without holding a strong ref.
    #[allow(dead_code)]
    pub(crate) weak_self: Weak<MarketDataRouter>,
    /// Lazy per-`(symbol, timeframe)` aggregator registry (S6).
    ///
    /// Delegate target for [`MarketDataRouter::subscribe_bars`]. Owns a
    /// `Weak<MarketDataRouter>` so the router can still be dropped even
    /// while aggregator tasks are alive.
    pub(crate) aggregator_registry: Arc<BarAggregatorRegistry>,
}

/// Per-symbol fan-out record.
///
/// All refcounts are `AtomicU32`; the actor is the only writer, but
/// the publisher task reads `receiver_count()` on both the broadcast
/// and the watch to decide when to auto-exit (M-4 + NB-3).
pub(crate) struct SymbolHub {
    /// Symbol this hub services — stamped at construction for debug
    /// output.
    pub(crate) symbol: SymbolKey,

    /// Tick fan-out (`Arc<Tick>` to keep the ring compact).
    pub(crate) ticks_tx: broadcast::Sender<Arc<Tick>>,
    /// Count of live `SubscriptionHandle<Tick>` consumers.
    pub(crate) tick_refcount: AtomicU32,

    /// Count of live [`QuoteHandle`] consumers.
    ///
    /// Watch-only consumers (watchlist cells) do NOT hold a broadcast
    /// receiver, so the publisher must also consider this refcount
    /// when deciding whether to auto-exit.
    ///
    /// [`QuoteHandle`]: super::handle::QuoteHandle
    pub(crate) watch_refcount: AtomicU32,

    /// Coalesced last bid/ask/last for the symbol.
    pub(crate) last_quote_tx: watch::Sender<Quote>,

    /// Realtime-bar fan-out. Lazily initialised on the first
    /// `subscribe_rt_bars` call (NB-6 Model A); tick-only hubs leave
    /// this empty.
    pub(crate) rt_bars_tx: OnceLock<broadcast::Sender<Arc<Bar>>>,
    /// Count of live `SubscriptionHandle<Bar>` consumers.
    pub(crate) rt_bar_refcount: AtomicU32,

    /// Publisher-task join handle for the tick stream (NB-7).
    ///
    /// Set when the publisher is spawned; cleared + `abort()`-ed by
    /// the actor on the last `DecTickRef + DecWatchRef` pair. Aborting
    /// drops the `TickStream` the task owned, which cancels upstream.
    pub(crate) tick_publisher_task: Mutex<Option<JoinHandle<()>>>,
    /// Publisher-task join handle for the RT-bar stream (NB-7).
    pub(crate) rt_bar_publisher_task: Mutex<Option<JoinHandle<()>>>,

    /// Timestamp (unix-ms) of the most recent tick published through
    /// `ticks_tx`. Observability hook for `debug_dump` (M-28).
    pub(crate) last_tick_ts: AtomicI64,
}

impl SymbolHub {
    /// Build a new hub with fresh broadcast + watch channels and all
    /// refcounts zeroed.
    pub(crate) fn new(symbol: SymbolKey) -> Arc<Self> {
        let (ticks_tx, _) = broadcast::channel(TICKS_CAP);
        let (last_quote_tx, _) = watch::channel(Quote::default());
        Arc::new(Self {
            symbol,
            ticks_tx,
            tick_refcount: AtomicU32::new(0),
            watch_refcount: AtomicU32::new(0),
            last_quote_tx,
            rt_bars_tx: OnceLock::new(),
            rt_bar_refcount: AtomicU32::new(0),
            tick_publisher_task: Mutex::new(None),
            rt_bar_publisher_task: Mutex::new(None),
            last_tick_ts: AtomicI64::new(0),
        })
    }

    /// Ensure the RT-bar broadcast is initialised and return its
    /// sender (cloned, cheap). Idempotent.
    pub(crate) fn ensure_rt_bars_tx(&self) -> broadcast::Sender<Arc<Bar>> {
        self.rt_bars_tx
            .get_or_init(|| {
                let (tx, _) = broadcast::channel(RT_BARS_CAP);
                tx
            })
            .clone()
    }
}
