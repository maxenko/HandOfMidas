//! IB-faithful [`MarketDataSource`] sim backend.
//!
//! `SimMarketData` implements every method on
//! [`MarketDataSource`](crate::MarketDataSource) with wire-accurate
//! event sequences:
//!
//! * eager connect → `Connecting` → `Connected` → farm-up →
//!   `OrderingReady` → `Ready` (NM-2).
//! * initial burst of `Bid` / `Ask` / `Last` / `BidSize` / `AskSize` /
//!   `LastSize` / `Volume` / `High` / `Low` / `Close` / `Open` /
//!   `TickParams` on every `subscribe_ticks` call.
//! * tick-loop emitting one `Last` + one paired `Bid` + one paired
//!   `Ask` per `tick_cadence_ms` window regardless of internal RNG
//!   drift steps (BR-11).
//! * deferred-removal cancel path with `late_tick_window_ms` grace
//!   (M-24 / M-25).
//! * explicit `simulate_connection_lost(FarmCode)` that distinguishes
//!   1100 / 1101 / 1102 per M-20.
//!
//! All timers use `tokio::time::interval` + `tokio::time::sleep` so
//! `tokio::time::pause()` drives the emitter deterministically in
//! tests (BR-20).

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use dashmap::DashMap;
#[cfg(any(test, feature = "test_inject"))]
use midas_broker_core::market_data::MarketEvent;
use midas_broker_core::market_data::{
    Bar, BarCompleteness, ConnectionState, ContractDetails, EndReason, FarmCode, FarmStatus,
    GenericTicks, IbDuration, MarketDataError, ReqId, SecurityType, SymbolKey, Tick,
    TickAttributes, TickByTickKind, TickKind, TickType, TickValue, Timeframe, WhatToShow,
};
use midas_broker_core::OhlcvBar;
use parking_lot::Mutex;
use tokio::sync::{broadcast, mpsc, watch, Notify};
use tokio::task::JoinHandle;

use crate::market_data_source::{HistoricalBarsResult, MarketDataSource};
use crate::sim::config::SimMarketDataConfig;
use crate::sim::rng::Xorshift64;
use crate::sim::tick_emitter::{run_tick_loop, TickLoopHandle};
use crate::stream::{HistoricalStream, HistoricalStreamEvent, RealtimeBarStream, TickStream};
use crate::testdata::TestDataProvider;

/// Per-subscription internal state held by the sim.
///
/// `cancelled` flips on handle-drop; the tick emitter reads it before
/// publishing. `draining_until_ns` gates the M-24 late-tick window: a
/// cancelled sub still accepts publishes until this instant, then is
/// GCed by the emitter sweep.
pub(crate) struct SimSubscription {
    pub(crate) req_id: ReqId,
    pub(crate) symbol: SymbolKey,
    #[allow(dead_code)]
    pub(crate) con_id: i32,
    /// Reserved for S4 isomorphism / ib_sim_isomorphism tests — the
    /// tick-by-tick path reuses the same fan-out but may need the
    /// flavour at emission time.
    #[allow(dead_code)]
    pub(crate) kind: SubKind,
    /// Optional so forced teardown (farm drop, disconnect) can `.take()`
    /// the sender out from under the emitter, closing the broadcast
    /// even if another `Arc<SimSubscription>` is still in flight.
    pub(crate) tx: Mutex<Option<broadcast::Sender<Arc<Tick>>>>,
    pub(crate) cancelled: AtomicBool,
    /// Milliseconds-since-epoch sentinel for the M-24 drain window.
    /// `0` means "not yet cancelled".
    pub(crate) draining_until_ns: std::sync::atomic::AtomicU64,
    /// Kept alive per NM-5 so future permanent errors can be surfaced
    /// through [`TickStream::last_error`](crate::stream::TickStream::last_error).
    #[allow(dead_code)]
    pub(crate) last_error: Arc<OnceLock<MarketDataError>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SubKind {
    Tick,
    TickByTick,
}

/// Per-symbol state shared across all subscriptions on that symbol.
pub(crate) struct SymbolSimState {
    pub(crate) market_price: f64,
    pub(crate) volume_accum: i64,
    /// Reserved — future `subscribe_ticks` will gate the initial-burst
    /// task off this so multi-sub symbols don't double-emit.
    #[allow(dead_code)]
    pub(crate) first_burst_done: AtomicBool,
}

impl SymbolSimState {
    pub(crate) fn new(initial_price: f64) -> Self {
        Self {
            market_price: initial_price,
            volume_accum: 0,
            first_burst_done: AtomicBool::new(false),
        }
    }
}

/// Subscription snapshot for a realtime-bar feed.
pub(crate) struct RtBarSubscription {
    pub(crate) req_id: ReqId,
    #[allow(dead_code)]
    pub(crate) symbol: SymbolKey,
    pub(crate) tx: broadcast::Sender<Arc<Bar>>,
    pub(crate) cancelled: AtomicBool,
    #[allow(dead_code)]
    pub(crate) last_error: Arc<OnceLock<MarketDataError>>,
    #[allow(dead_code)]
    pub(crate) what_to_show: WhatToShow,
}

/// Historical-stream subscription snapshot.
pub(crate) struct HistSubscription {
    #[allow(dead_code)]
    pub(crate) req_id: ReqId,
    #[allow(dead_code)]
    pub(crate) symbol: SymbolKey,
    #[allow(dead_code)]
    pub(crate) cancel: Arc<AtomicBool>,
}

/// IB-faithful sim implementation of
/// [`MarketDataSource`](crate::MarketDataSource).
///
/// Construction spawns a background connect-sequence task — calling
/// [`MarketDataSource::connection_state`] right after `new` returns
/// will show the state machine marching from `Disconnected` through
/// `Ready` without an explicit `connect()` call (NM-2).
pub struct SimMarketData {
    config: SimMarketDataConfig,
    pub(crate) subscriptions: Arc<DashMap<ReqId, Arc<SimSubscription>>>,
    pub(crate) rt_bar_subs: Arc<DashMap<ReqId, Arc<RtBarSubscription>>>,
    pub(crate) hist_subs: Arc<DashMap<ReqId, Arc<HistSubscription>>>,
    pub(crate) symbol_state: Arc<DashMap<SymbolKey, SymbolSimState>>,
    farm_status_tx: broadcast::Sender<FarmStatus>,
    conn_state_tx: watch::Sender<ConnectionState>,
    ordering_ready_tx: broadcast::Sender<MarketEventLite>,
    req_id_counter: Arc<AtomicI32>,
    /// RNG kept on the struct so tests can reseed via a future
    /// `SimMarketData::reseed` helper; currently passed to the
    /// emitter loop at construction only.
    #[allow(dead_code)]
    pub(crate) rng: Arc<Mutex<Xorshift64>>,
    /// TestDataProvider seeded for historical bar generation. Held
    /// inside a mutex because the underlying API takes `&mut self`.
    pub(crate) data_provider: Arc<Mutex<TestDataProvider>>,
    /// Background-task handles kept alive for the life of the sim.
    #[allow(dead_code)]
    tasks: Arc<Mutex<Vec<JoinHandle<()>>>>,
    tick_loop: TickLoopHandle,
    /// Reconnect-attempt counter (used by the 1100 path).
    reconnect_attempt: Arc<AtomicU32>,
    /// Tracks the most-recent farm loss so a subsequent reconnect can
    /// decide whether to prepend `ConnectionRestoredDataLost`.
    last_loss: Arc<Mutex<Option<FarmCode>>>,
    /// Test-introspection counters + failure injection. Always live
    /// (cheap atomics) so the `pub` observability methods that read
    /// them don't need feature gating. Routers/apps that don't touch
    /// these methods pay only the single extra Arc clone at
    /// construction time.
    test_counters: Arc<SimTestCounters>,
}

/// Test-only introspection counters and one-shot error injection hook.
///
/// Held behind `Arc` so the router behaviour tests can share the
/// counters across tasks.
#[derive(Default)]
pub(crate) struct SimTestCounters {
    /// Cumulative successful `subscribe_ticks` calls.
    pub(crate) tick_subscribe_calls: AtomicU64,
    /// Cumulative successful `subscribe_realtime_bars` calls.
    pub(crate) rt_bar_subscribe_calls: AtomicU64,
    /// Cumulative `resolve_contract` calls.
    pub(crate) resolve_contract_calls: AtomicU64,
    /// One-shot next-call error for `subscribe_ticks`. Cleared on
    /// use — the arm-then-fire pattern matches the NM-3 rollback
    /// test.
    pub(crate) next_subscribe_error: Mutex<Option<MarketDataError>>,
    /// One-shot hang primitive: when armed, the next
    /// `subscribe_ticks` call parks on the paired `Notify` until the
    /// test flushes it. Used by the router wedge test to prove the
    /// control actor's per-handler timeout returns `Err(..)` instead
    /// of wedging the mpsc forever.
    pub(crate) next_subscribe_hang: Mutex<Option<Arc<Notify>>>,
    /// Most recent `use_rth` value the sim observed on
    /// [`MarketDataSource::historical_bars`]. `None` until the first
    /// call lands. Drives the router-level propagation test for the
    /// ETH-shading `S4-router` slice — the router exposes a
    /// `use_rth` parameter that must reach the underlying source
    /// unaltered.
    pub(crate) last_historical_use_rth: Mutex<Option<bool>>,
}

/// Minimal local mirror of the `MarketEvent::OrderingReady` variant so
/// tests that listen for it have a broadcast they can subscribe to.
///
/// The router-era unified [`MarketEvent`](midas_broker_core::market_data::MarketEvent)
/// enum already carries `OrderingReady { next_order_id }`, but
/// re-using it would require a second broadcast for the one event —
/// this lightweight mirror keeps the type alongside [`SimMarketData`]
/// while slices 5/6 converge consumers onto the unified enum.
#[derive(Debug, Clone, Copy)]
pub struct OrderingReadyEvent {
    /// Next valid IB order id seeded from
    /// [`SimMarketDataConfig::next_order_id_seed`].
    pub next_order_id: i32,
}

/// Back-compat alias; crate-internal callers still use the original
/// name.
pub(crate) type MarketEventLite = OrderingReadyEvent;

impl SimMarketData {
    /// Build a new sim backend with `config`.
    ///
    /// Spawns a background task that walks
    /// [`ConnectionState`] → `Connecting` → `Connected` → (after
    /// `farm_up_delay_ms`) farm-up events → `OrderingReady` → `Ready`.
    /// The tick-emitter loop is also spawned here and runs until the
    /// sim is dropped.
    pub fn new(config: SimMarketDataConfig) -> Arc<Self> {
        let (farm_status_tx, _) = broadcast::channel(64);
        let (conn_state_tx, _) = watch::channel(ConnectionState::Disconnected);
        let (ordering_ready_tx, _) = broadcast::channel(16);

        let rng = Arc::new(Mutex::new(match config.rng_seed {
            Some(seed) => Xorshift64::new(seed),
            None => Xorshift64::from_entropy(),
        }));

        let subscriptions: Arc<DashMap<ReqId, Arc<SimSubscription>>> = Arc::new(DashMap::new());
        let rt_bar_subs: Arc<DashMap<ReqId, Arc<RtBarSubscription>>> = Arc::new(DashMap::new());
        let hist_subs: Arc<DashMap<ReqId, Arc<HistSubscription>>> = Arc::new(DashMap::new());
        let symbol_state: Arc<DashMap<SymbolKey, SymbolSimState>> = Arc::new(DashMap::new());
        let data_provider = Arc::new(Mutex::new(TestDataProvider::new()));
        let req_id_counter = Arc::new(AtomicI32::new(1));
        let tasks: Arc<Mutex<Vec<JoinHandle<()>>>> = Arc::new(Mutex::new(Vec::new()));

        // Spawn the tick-emitter loop.
        let tick_loop = run_tick_loop(
            Duration::from_millis(config.tick_cadence_ms),
            Duration::from_millis(config.late_tick_window_ms),
            config.tick_drift_bps,
            config.default_spread,
            subscriptions.clone(),
            symbol_state.clone(),
            rng.clone(),
        );

        let this = Arc::new(Self {
            config: config.clone(),
            subscriptions,
            rt_bar_subs,
            hist_subs,
            symbol_state,
            farm_status_tx: farm_status_tx.clone(),
            conn_state_tx: conn_state_tx.clone(),
            ordering_ready_tx: ordering_ready_tx.clone(),
            req_id_counter,
            rng,
            data_provider,
            tasks: tasks.clone(),
            tick_loop,
            reconnect_attempt: Arc::new(AtomicU32::new(0)),
            last_loss: Arc::new(Mutex::new(None)),
            test_counters: Arc::new(SimTestCounters::default()),
        });

        // Eagerly drive the connect sequence (NM-2).
        let this_clone = this.clone();
        let handle = tokio::spawn(async move {
            this_clone.run_connect_sequence().await;
        });
        push_task_inner(&tasks, handle);

        this
    }

    /// Drive the `Disconnected` → `Connecting` → `Connected` → farm-up
    /// → `OrderingReady` → `Ready` sequence.
    async fn run_connect_sequence(&self) {
        // Announce Connecting immediately.
        let _ = self.conn_state_tx.send(ConnectionState::Connecting);

        // Short handshake delay (10 ms — plenty for tests, imperceptible for humans).
        tokio::time::sleep(Duration::from_millis(10)).await;
        let _ = self.conn_state_tx.send(ConnectionState::Connected {
            server_version: self.config.server_version,
        });

        // Wait for farms to come up.
        tokio::time::sleep(Duration::from_millis(self.config.farm_up_delay_ms)).await;

        let _ = self.farm_status_tx.send(FarmStatus {
            code: FarmCode::MarketDataFarmOk,
            connected: true,
            detail: "Market data farm connection is OK:sim".to_string(),
        });
        let _ = self.farm_status_tx.send(FarmStatus {
            code: FarmCode::HistoricalDataFarmOk,
            connected: true,
            detail: "HMDS data farm connection is OK:sim".to_string(),
        });
        let _ = self.farm_status_tx.send(FarmStatus {
            code: FarmCode::SecDefFarmOk,
            connected: true,
            detail: "Sec-def data farm connection is OK:sim".to_string(),
        });

        // OrderingReady (M-14) — NOT a FarmCode.
        let _ = self.ordering_ready_tx.send(MarketEventLite {
            next_order_id: self.config.next_order_id_seed,
        });

        // Finally: Ready.
        let _ = self.conn_state_tx.send(ConnectionState::Ready);
    }

    /// Public observability hook — broadcast of the local
    /// `OrderingReady` event. Separate channel from
    /// [`FarmStatus`](midas_broker_core::market_data::FarmStatus) so
    /// consumers never have to treat `OrderingReady` as a farm code.
    pub fn ordering_ready(&self) -> broadcast::Receiver<OrderingReadyEvent> {
        self.ordering_ready_tx.subscribe()
    }

    /// Get (or synthesise) the base price for `symbol`, AND re-anchor
    /// the live `market_price` to [`TestDataProvider`]'s most recent
    /// synthetic close.
    ///
    /// The historical fetch path (`historical_bars`) and the live-tick
    /// path (`tick_emitter` random walk on `market_price`) live in two
    /// independent price universes: the former is deterministic,
    /// generated by `TestDataProvider`; the latter is a random walk
    /// initialised at the daily close and free-running thereafter. If
    /// the chart fetches historical (last close = X) and then
    /// subscribes when `market_price` has drifted to X', the very
    /// first live tick spikes the chart's last bar from X to X'.
    ///
    /// The fix: on every subscribe, re-seed `market_price` to the
    /// TestDataProvider's most recent intraday close (falling back to
    /// the daily close when intraday data isn't available). This is a
    /// SIM detail — existing subscribers may see a small
    /// discontinuity if their stream had drifted, but for a sim that
    /// is the lesser evil compared to the consistent chart-load spike
    /// the prior behaviour produced.
    ///
    /// Returns the daily close, which is the correct value for the
    /// IB-API `TickType::Close` (prev-day-reference) tick — that
    /// semantic is unchanged.
    fn seed_price_for(&self, symbol: &SymbolKey) -> f64 {
        let now_secs = Utc::now().timestamp();
        let (intraday_close, daily_close) = {
            let mut dp = self.data_provider.lock();
            // 24-hour lookback comfortably covers the most recent
            // session even on holidays / pre-market opens. Intraday
            // bars are M1 — the finest resolution `TestDataProvider`
            // returns above S30.
            let intraday = dp.bars(&symbol.symbol, Timeframe::M1, now_secs - 86400, now_secs);
            let intraday_close = intraday.last().map(|b| b.close);
            let daily_close = dp
                .daily_bars(&symbol.symbol)
                .last()
                .map(|b| b.close)
                .unwrap_or(100.0);
            (intraday_close, daily_close)
        };
        let anchor = intraday_close.unwrap_or(daily_close);
        self.symbol_state
            .entry(symbol.clone())
            .and_modify(|s| s.market_price = anchor)
            .or_insert_with(|| {
                let mut state = SymbolSimState::new(daily_close);
                state.market_price = anchor;
                state
            });
        daily_close
    }

    /// Allocate a fresh wire-request id (monotonic `AtomicI32`).
    fn next_req_id(&self) -> ReqId {
        ReqId::next(&self.req_id_counter)
    }

    /// Spawn the initial-burst task for a fresh subscription.
    fn spawn_initial_burst(&self, sub: Arc<SimSubscription>) {
        let base_price = self.seed_price_for(&sub.symbol);
        // Read the CURRENT drifted price for Bid/Ask/Last/High/Low/Open —
        // those tick types describe live session state, not the prior
        // day's reference. Using `base_price` (the cached daily close)
        // for them produced visible chart spikes whenever the chart
        // subscribed mid-drift: the Quote.last → `update_last_price`
        // pipeline stretched the in-progress candle's high to the
        // stale daily-close value. Fall back to `base_price` only if
        // the symbol state is missing (newly seeded; current == base).
        let current_price = self
            .symbol_state
            .get(&sub.symbol)
            .map(|s| s.market_price)
            .unwrap_or(base_price);
        let spread = self.config.default_spread;
        let delay = Duration::from_millis(self.config.burst_delay_ms);
        let enabled = self.config.burst_enabled;

        let handle = tokio::spawn(async move {
            if !enabled {
                return;
            }
            tokio::time::sleep(delay).await;
            if sub.cancelled.load(Ordering::SeqCst) {
                return;
            }
            let ts = Utc::now();
            let mk = |kind: TickKind, tick_type: TickType, value: TickValue| {
                Arc::new(Tick {
                    symbol: sub.symbol.clone(),
                    req_id: sub.req_id,
                    kind,
                    tick_type,
                    value,
                    attrs: TickAttributes::default(),
                    ts,
                })
            };

            let half = spread / 2.0;

            // IB-faithful initial burst: bid/ask pair, last/last-size,
            // then the one-shot OHLC snapshot and params. Bid/Ask/Last/
            // High/Low/Open derive from `current_price` so the chart's
            // first Quote reflects the live session, not yesterday's
            // close. `Close` keeps `base_price` — IB-API tick-id 9
            // *is* the previous-day close (used as the change% anchor),
            // so the original semantics there are correct.
            let ticks = vec![
                mk(
                    TickKind::Price,
                    TickType::Bid,
                    TickValue::Price(current_price - half),
                ),
                mk(TickKind::Size, TickType::BidSize, TickValue::Size(100)),
                mk(
                    TickKind::Price,
                    TickType::Ask,
                    TickValue::Price(current_price + half),
                ),
                mk(TickKind::Size, TickType::AskSize, TickValue::Size(100)),
                mk(
                    TickKind::Price,
                    TickType::Last,
                    TickValue::Price(current_price),
                ),
                mk(TickKind::Size, TickType::LastSize, TickValue::Size(100)),
                mk(TickKind::Size, TickType::Volume, TickValue::Size(0)),
                mk(
                    TickKind::Price,
                    TickType::High,
                    TickValue::Price(current_price),
                ),
                mk(
                    TickKind::Price,
                    TickType::Low,
                    TickValue::Price(current_price),
                ),
                mk(
                    TickKind::Price,
                    TickType::Close,
                    TickValue::Price(base_price),
                ),
                mk(
                    TickKind::Price,
                    TickType::Open,
                    TickValue::Price(current_price),
                ),
                // TickParams: represented as a Params-kind tick carrying a
                // Text value that tests key off of. Real IB uses a separate
                // callback family but the router-era MarketEvent shape folds
                // it into a single Tick event.
                mk(
                    TickKind::Params,
                    TickType::Bid,
                    TickValue::Text("min_tick=0.01;bbo=SMART;snapshot_permissions=0".to_string()),
                ),
            ];
            for tick in ticks {
                if let Some(tx) = sub.tx.lock().as_ref() {
                    let _ = tx.send(tick);
                }
            }
            sub.cancelled
                .compare_exchange(false, false, Ordering::SeqCst, Ordering::SeqCst)
                .ok();
            // Ensure the tick-loop does not re-emit the first burst.
            // (The symbol state already gates this per-symbol.)
        });
        self.push_task(handle);
    }

    /// Push a background task handle onto `self.tasks` while pruning
    /// finished handles inline. Without this the vec grows unboundedly
    /// for long-lived sims — every subscribe + drop cycle adds a
    /// `JoinHandle<()>` but nothing trims completed ones. A single
    /// `retain` per push keeps the cost O(live-tasks) per subscribe
    /// call and bounds the vec size to roughly the number of
    /// currently-running tasks.
    fn push_task(&self, handle: JoinHandle<()>) {
        push_task_inner(&self.tasks, handle);
    }

    /// Build a cancel closure for a tick subscription. Handle-drop
    /// fires this, which flips `cancelled` and opens the drain window.
    fn build_tick_cancel_closure(
        &self,
        sub: Arc<SimSubscription>,
    ) -> Box<dyn FnOnce() + Send + Sync> {
        let late_window = Duration::from_millis(self.config.late_tick_window_ms);
        let subscriptions = self.subscriptions.clone();
        Box::new(move || {
            // Flip cancelled immediately so the tick-loop skips this sub
            // for NEW publishes — in-flight publishes inside the current
            // window still go through.
            sub.cancelled.store(true, Ordering::SeqCst);
            // Compute the drain-until instant in unix-ns. Used by the
            // tick-loop's GC sweep.
            let until_ms = now_ms() + late_window.as_millis() as u64;
            sub.draining_until_ns.store(until_ms, Ordering::SeqCst);
            // Emit a final SubscriptionEnded sentinel by closing the
            // broadcast (consumers observe RecvError::Closed). The GC
            // sweep in the tick-loop removes the entry after the
            // drain window; we schedule a detached task to do the
            // removal so the closure returns promptly.
            let req_id = sub.req_id;
            let subs = subscriptions.clone();
            // Gate the spawn on an active tokio runtime. This closure
            // is often invoked from a stream handle's Drop; if the
            // handle is dropped outside a tokio context (sim teardown
            // in a `#[test]` rather than `#[tokio::test]`, or during
            // runtime shutdown) `tokio::spawn` would panic. Without a
            // runtime the sim is being torn down anyway — we just
            // remove the sub entry synchronously so the DashMap stays
            // clean.
            match tokio::runtime::Handle::try_current() {
                Ok(handle) => {
                    handle.spawn(async move {
                        tokio::time::sleep(late_window).await;
                        subs.remove(&req_id);
                    });
                }
                Err(_) => {
                    subs.remove(&req_id);
                }
            }
        })
    }

    /// Public observability: return the count of live subscriptions.
    /// Used by the test helper and dev_harness snapshots.
    pub fn live_subscription_count(&self) -> usize {
        self.subscriptions
            .iter()
            .filter(|e| !e.cancelled.load(Ordering::SeqCst))
            .count()
    }

    /// Count of live tick subscriptions for `symbol`. Used by
    /// downstream router tests (S5) to assert the fan-out invariant
    /// "one upstream per symbol regardless of consumer count".
    pub fn live_subscription_count_for(&self, symbol: &SymbolKey) -> usize {
        self.subscriptions
            .iter()
            .filter(|e| !e.cancelled.load(Ordering::SeqCst) && e.symbol == *symbol)
            .count()
    }

    /// Count of live realtime-bar subscriptions for `symbol`. Mirrors
    /// [`Self::live_subscription_count_for`] on the RT-bar fan-out.
    pub fn live_rt_bar_subscription_count_for(&self, symbol: &SymbolKey) -> usize {
        self.rt_bar_subs
            .iter()
            .filter(|e| !e.cancelled.load(Ordering::SeqCst) && e.symbol == *symbol)
            .count()
    }

    /// Cumulative number of successful `subscribe_ticks` calls. Used
    /// by the router behaviour tests to assert the "single upstream
    /// per first-subscribe" invariant.
    pub fn tick_subscribe_call_count(&self) -> u64 {
        self.test_counters
            .tick_subscribe_calls
            .load(Ordering::SeqCst)
    }

    /// Cumulative number of successful `subscribe_realtime_bars`
    /// calls. Mirror of [`Self::tick_subscribe_call_count`] for the
    /// RT-bar path.
    pub fn rt_bar_subscribe_call_count(&self) -> u64 {
        self.test_counters
            .rt_bar_subscribe_calls
            .load(Ordering::SeqCst)
    }

    /// Cumulative number of `resolve_contract` calls. Used to verify
    /// the router's contract-cache memoisation (NM-1).
    pub fn resolve_contract_call_count(&self) -> u64 {
        self.test_counters
            .resolve_contract_calls
            .load(Ordering::SeqCst)
    }

    /// Most recent `use_rth` value the sim observed on
    /// [`MarketDataSource::historical_bars`]. Returns `None` until
    /// the first call lands. Used by the router-level propagation
    /// test for the ETH-shading `S4-router` slice.
    pub fn last_historical_use_rth(&self) -> Option<bool> {
        *self.test_counters.last_historical_use_rth.lock()
    }

    /// Force the next `subscribe_ticks` call to return the configured
    /// error — once-shot. Consumed on use. Used by the router test
    /// for NM-3 source-failure rollback.
    pub fn arm_next_subscribe_ticks_error(&self, err: MarketDataError) {
        *self.test_counters.next_subscribe_error.lock() = Some(err);
    }

    /// Arm the next `subscribe_ticks` call to hang until the returned
    /// [`Notify`] is signalled. Consumed on use. Used by the router
    /// wedge-guard test to prove `ROUTER_ACTOR_OP_TIMEOUT` closes
    /// out the handler with `Err(..)` even when the upstream never
    /// yields. Callers drive `notify.notify_one()` to unblock the
    /// hung task after the observation they care about.
    pub fn arm_next_subscribe_ticks_hang(&self) -> Arc<Notify> {
        let notify = Arc::new(Notify::new());
        *self.test_counters.next_subscribe_hang.lock() = Some(notify.clone());
        notify
    }

    /// M-20: simulate a farm-level connection event.
    ///
    /// * [`FarmCode::ConnectionRestoredDataLost`] (1101): emit
    ///   `SubscriptionEnded { reason: FarmDropped }` on every live tick
    ///   sub (by closing its broadcast); consumers must re-subscribe.
    /// * [`FarmCode::ConnectionRestoredDataKept`] (1102): log-only. No
    ///   subs dropped.
    /// * [`FarmCode::ConnectionLost`] (1100): emit the 1100 farm event,
    ///   drop all subs (same as 1101), and transition
    ///   [`ConnectionState`] to `Reconnecting { attempt: n }`.
    ///
    /// Other farm codes are accepted for completeness but trigger only
    /// the farm-status broadcast.
    pub fn simulate_connection_lost(&self, code: FarmCode) {
        let detail = match code {
            FarmCode::ConnectionLost => "Connectivity between IB and TWS has been lost.",
            FarmCode::ConnectionRestoredDataLost => {
                "Connectivity between IB and TWS has been restored - data lost."
            }
            FarmCode::ConnectionRestoredDataKept => {
                "Connectivity between IB and TWS has been restored - data maintained."
            }
            _ => "Sim: farm event",
        };
        let connected = matches!(
            code,
            FarmCode::ConnectionRestoredDataKept | FarmCode::ConnectionRestoredDataLost
        );
        let _ = self.farm_status_tx.send(FarmStatus {
            code,
            connected,
            detail: detail.to_string(),
        });

        match code {
            FarmCode::ConnectionRestoredDataLost => {
                self.drop_all_subs_with_reason(EndReason::FarmDropped);
                *self.last_loss.lock() = Some(code);
            }
            FarmCode::ConnectionLost => {
                let attempt = self.reconnect_attempt.fetch_add(1, Ordering::SeqCst) + 1;
                self.drop_all_subs_with_reason(EndReason::Disconnected);
                let _ = self
                    .conn_state_tx
                    .send(ConnectionState::Reconnecting { attempt });
                *self.last_loss.lock() = Some(code);
            }
            FarmCode::ConnectionRestoredDataKept => {
                // log-only (no subs dropped)
            }
            _ => {}
        }
    }

    /// Simulate a fresh reconnect. Re-runs the farm-up sequence; if
    /// the prior loss was 1100/1101, prepends `ConnectionRestoredDataLost`.
    pub fn simulate_reconnect(&self) {
        let prior_loss = self.last_loss.lock().take();
        if matches!(
            prior_loss,
            Some(FarmCode::ConnectionLost) | Some(FarmCode::ConnectionRestoredDataLost)
        ) {
            let _ = self.farm_status_tx.send(FarmStatus {
                code: FarmCode::ConnectionRestoredDataLost,
                connected: true,
                detail: "Sim: reconnected; subscription data lost".to_string(),
            });
        }
        let _ = self.conn_state_tx.send(ConnectionState::Connected {
            server_version: self.config.server_version,
        });
        let _ = self.farm_status_tx.send(FarmStatus {
            code: FarmCode::MarketDataFarmOk,
            connected: true,
            detail: "Market data farm connection is OK:sim".to_string(),
        });
        let _ = self.farm_status_tx.send(FarmStatus {
            code: FarmCode::HistoricalDataFarmOk,
            connected: true,
            detail: "HMDS data farm connection is OK:sim".to_string(),
        });
        let _ = self.farm_status_tx.send(FarmStatus {
            code: FarmCode::SecDefFarmOk,
            connected: true,
            detail: "Sec-def data farm connection is OK:sim".to_string(),
        });
        let _ = self.conn_state_tx.send(ConnectionState::Ready);
    }

    fn drop_all_subs_with_reason(&self, _reason: EndReason) {
        // `_reason` is carried for dev_harness replay in a later slice.
        //
        // Snapshot req_ids before mutating the map so the iterator
        // isn't invalidated by the remove calls.
        let ids: Vec<ReqId> = self.subscriptions.iter().map(|e| e.req_id).collect();
        for req_id in ids {
            if let Some((_, sub)) = self.subscriptions.remove(&req_id) {
                sub.cancelled.store(true, Ordering::SeqCst);
                // Force the broadcast closed: `.take()` the sender out
                // even if other `Arc<SimSubscription>` refs are still
                // in flight (e.g. a detached initial-burst task). The
                // taken `Sender` drops here, closing the channel;
                // every consumer observes `RecvError::Closed` on the
                // next `recv()`.
                drop(sub.tx.lock().take());
            }
        }
        let rt_ids: Vec<ReqId> = self.rt_bar_subs.iter().map(|e| e.req_id).collect();
        for req_id in rt_ids {
            if let Some((_, sub)) = self.rt_bar_subs.remove(&req_id) {
                sub.cancelled.store(true, Ordering::SeqCst);
            }
        }
    }

    /// Resolve the per-call `use_rth` request against the sim's
    /// `synthetic_includes_eth` capability flag. ETH bars enter the
    /// response only when both align — capability ON and the call
    /// asked for non-RTH data. See `SimMarketDataConfig::synthetic_includes_eth`
    /// for the rationale on splitting the flag in two.
    fn should_include_eth(&self, use_rth: bool) -> bool {
        self.config.synthetic_includes_eth && !use_rth
    }

    /// Build the historical bars for a symbol using the seeded
    /// [`TestDataProvider`]. Helper shared by `historical_bars` and
    /// `historical_stream`.
    ///
    /// `include_eth` selects the RTH-only vs. RTH+ETH (pre + post)
    /// generation path. The caller is expected to combine the
    /// per-call `use_rth` flag with `SimMarketDataConfig::synthetic_includes_eth`
    /// before deciding — see [`Self::should_include_eth`].
    fn synthesize_historical(
        &self,
        symbol: &SymbolKey,
        end: DateTime<Utc>,
        duration: IbDuration,
        bar_size: Timeframe,
        include_eth: bool,
    ) -> (Vec<Bar>, DateTime<Utc>, DateTime<Utc>) {
        let end_secs = end.timestamp();
        let lookback_secs: i64 = match duration {
            IbDuration::Seconds(n) => n as i64,
            IbDuration::Days(n) => n as i64 * 86_400,
            IbDuration::Weeks(n) => n as i64 * 7 * 86_400,
            IbDuration::Months(n) => n as i64 * 30 * 86_400,
            IbDuration::Years(n) => n as i64 * 365 * 86_400,
        };
        let start_secs = end_secs.saturating_sub(lookback_secs);

        // TestDataProvider only handles S30 and coarser; fall back to S30
        // for finer requests.
        let effective_tf = if bar_size.as_secs() < Timeframe::S30.as_secs() {
            Timeframe::S30
        } else {
            bar_size
        };

        let ohlcv = {
            let mut dp = self.data_provider.lock();
            dp.bars_with_eth(
                &symbol.symbol,
                effective_tf,
                start_secs,
                end_secs,
                include_eth,
            )
        };

        let bars: Vec<Bar> = ohlcv
            .iter()
            .map(|b: &OhlcvBar| ohlcv_to_bar(b, symbol.clone(), bar_size))
            .collect();

        // Seam boundary: explicit config override (BR-21) wins, else
        // now() at the moment we synthesise.
        let last_ts = self
            .config
            .historical_last_ts
            .unwrap_or_else(|| bars.last().map(|b| b.ts_close).unwrap_or(end));
        let first_ts = bars.first().map(|b| b.ts_open).unwrap_or(end);
        (bars, first_ts, last_ts)
    }
}

/// Convert an [`OhlcvBar`] (epoch-second) to a router-level [`Bar`].
fn ohlcv_to_bar(b: &OhlcvBar, symbol: SymbolKey, tf: Timeframe) -> Bar {
    let ts_open = DateTime::<Utc>::from_timestamp(b.timestamp, 0).unwrap_or_else(Utc::now);
    let ts_close = ts_open + chrono::Duration::seconds(tf.as_secs() as i64);
    Bar {
        symbol,
        timeframe: tf,
        ts_open,
        ts_close,
        o: b.open,
        h: b.high,
        l: b.low,
        c: b.close,
        volume: b.volume.max(0) as u64,
        trade_count: 0,
        wap: None,
        completeness: BarCompleteness::Completed,
    }
}

/// Shared retain-then-push helper for the background-task vec.
///
/// Called from every site that spawns a long-running sim task (ctor
/// connect-sequence, initial-burst, rt-bar emitter, historical-stream
/// update tail). Pruning `is_finished` handles on the way in keeps
/// the vec bounded to the number of live tasks — the pre-fix path
/// simply pushed, so the vec grew forever for any long-lived sim.
fn push_task_inner(tasks: &Arc<Mutex<Vec<JoinHandle<()>>>>, handle: JoinHandle<()>) {
    let mut ts = tasks.lock();
    ts.retain(|h| !h.is_finished());
    ts.push(handle);
}

// Wall-clock helper lives in `market_data_helpers::now_ms`; both this
// module and `tick_emitter` reach for it there to avoid drift.
use crate::sim::market_data_helpers::now_ms;

#[async_trait]
impl MarketDataSource for SimMarketData {
    async fn subscribe_ticks(
        &self,
        symbol: &SymbolKey,
        con_id: i32,
        _generic_ticks: GenericTicks,
    ) -> Result<TickStream, MarketDataError> {
        // One-shot error injection (used by NM-3 rollback test).
        if let Some(err) = self.test_counters.next_subscribe_error.lock().take() {
            return Err(err);
        }
        // One-shot hang injection (used by the router wedge test).
        // Park on the paired `Notify` before we allocate any state so
        // the observation site is indistinguishable from a genuinely
        // stuck upstream. Scope the lock tightly so the parking_lot
        // guard isn't held across the await (it's !Send).
        let hang_slot = self.test_counters.next_subscribe_hang.lock().take();
        if let Some(hang) = hang_slot {
            hang.notified().await;
        }
        let req_id = self.next_req_id();
        let (tx, rx) = broadcast::channel::<Arc<Tick>>(4096);
        let last_error = Arc::new(OnceLock::new());
        let sub = Arc::new(SimSubscription {
            req_id,
            symbol: symbol.clone(),
            con_id,
            kind: SubKind::Tick,
            tx: Mutex::new(Some(tx.clone())),
            cancelled: AtomicBool::new(false),
            draining_until_ns: std::sync::atomic::AtomicU64::new(0),
            last_error: last_error.clone(),
        });
        self.seed_price_for(symbol);
        self.subscriptions.insert(req_id, sub.clone());
        self.spawn_initial_burst(sub.clone());
        // Nudge the emitter loop so its interval re-aligns with the new sub.
        self.tick_loop.wake.notify_one();
        let cancel = self.build_tick_cancel_closure(sub);
        self.test_counters
            .tick_subscribe_calls
            .fetch_add(1, Ordering::SeqCst);
        Ok(TickStream::new(req_id, rx, last_error, cancel))
    }

    async fn subscribe_tick_by_tick(
        &self,
        symbol: &SymbolKey,
        con_id: i32,
        _kind: TickByTickKind,
    ) -> Result<TickStream, MarketDataError> {
        // For sim fidelity we reuse the same emission path — the kind
        // distinction matters on the IB wire, not in the simulated
        // event sequence. Mark the sub as TickByTick so future tests
        // can filter.
        let req_id = self.next_req_id();
        let (tx, rx) = broadcast::channel::<Arc<Tick>>(4096);
        let last_error = Arc::new(OnceLock::new());
        let sub = Arc::new(SimSubscription {
            req_id,
            symbol: symbol.clone(),
            con_id,
            kind: SubKind::TickByTick,
            tx: Mutex::new(Some(tx.clone())),
            cancelled: AtomicBool::new(false),
            draining_until_ns: std::sync::atomic::AtomicU64::new(0),
            last_error: last_error.clone(),
        });
        self.seed_price_for(symbol);
        self.subscriptions.insert(req_id, sub.clone());
        self.spawn_initial_burst(sub.clone());
        self.tick_loop.wake.notify_one();
        let cancel = self.build_tick_cancel_closure(sub);
        Ok(TickStream::new(req_id, rx, last_error, cancel))
    }

    async fn subscribe_realtime_bars(
        &self,
        symbol: &SymbolKey,
        _con_id: i32,
        what_to_show: WhatToShow,
    ) -> Result<RealtimeBarStream, MarketDataError> {
        let req_id = self.next_req_id();
        let (tx, rx) = broadcast::channel::<Arc<Bar>>(256);
        let last_error = Arc::new(OnceLock::new());
        let sub = Arc::new(RtBarSubscription {
            req_id,
            symbol: symbol.clone(),
            tx: tx.clone(),
            cancelled: AtomicBool::new(false),
            last_error: last_error.clone(),
            what_to_show,
        });
        self.seed_price_for(symbol);
        self.rt_bar_subs.insert(req_id, sub.clone());

        let bar_window = Duration::from_millis(self.config.realtime_bar_size_ms);
        let sub_for_task = sub.clone();
        let symbol_state = self.symbol_state.clone();
        let sym_key = symbol.clone();
        let tf = Timeframe::S30; // closest 30s canonical; sim emits on `realtime_bar_size_ms` cadence
        let handle = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(bar_window);
            // First tick fires immediately — consume it so the first
            // bar lands one window after subscribe (IB-faithful).
            ticker.tick().await;
            loop {
                ticker.tick().await;
                if sub_for_task.cancelled.load(Ordering::SeqCst) {
                    break;
                }
                let state = match symbol_state.get(&sym_key) {
                    Some(s) => s,
                    None => continue,
                };
                let c = state.market_price;
                // Tight synthetic OHLC around the current market price;
                // enough fidelity for seam + cadence tests without
                // double-tracking tick min/max per window.
                let bar = Bar {
                    symbol: sym_key.clone(),
                    timeframe: tf,
                    ts_open: Utc::now()
                        - chrono::Duration::milliseconds(bar_window.as_millis() as i64),
                    ts_close: Utc::now(),
                    o: c,
                    h: c,
                    l: c,
                    c,
                    volume: 100,
                    trade_count: 1,
                    wap: Some(c),
                    completeness: BarCompleteness::Completed,
                };
                let _ = sub_for_task.tx.send(Arc::new(bar));
            }
        });
        self.push_task(handle);

        let rt_subs = self.rt_bar_subs.clone();
        let cancel = Box::new(move || {
            sub.cancelled.store(true, Ordering::SeqCst);
            rt_subs.remove(&req_id);
        });
        self.test_counters
            .rt_bar_subscribe_calls
            .fetch_add(1, Ordering::SeqCst);
        Ok(RealtimeBarStream::new(req_id, rx, last_error, cancel))
    }

    async fn historical_bars(
        &self,
        symbol: &SymbolKey,
        _con_id: i32,
        end: DateTime<Utc>,
        duration: IbDuration,
        bar_size: Timeframe,
        _what_to_show: WhatToShow,
        use_rth: bool,
    ) -> Result<HistoricalBarsResult, MarketDataError> {
        *self.test_counters.last_historical_use_rth.lock() = Some(use_rth);
        let include_eth = self.should_include_eth(use_rth);
        let (bars, first_ts, last_ts) =
            self.synthesize_historical(symbol, end, duration, bar_size, include_eth);
        Ok(HistoricalBarsResult {
            bars,
            first_ts,
            last_ts,
        })
    }

    async fn historical_stream(
        &self,
        symbol: &SymbolKey,
        _con_id: i32,
        duration: IbDuration,
        bar_size: Timeframe,
        _what_to_show: WhatToShow,
        use_rth: bool,
    ) -> Result<HistoricalStream, MarketDataError> {
        let req_id = self.next_req_id();
        let (tx, rx) = mpsc::channel::<HistoricalStreamEvent>(64);

        let end = Utc::now();
        let include_eth = self.should_include_eth(use_rth);
        let (bars, first_ts, last_ts) =
            self.synthesize_historical(symbol, end, duration, bar_size, include_eth);

        let cancel_flag = Arc::new(AtomicBool::new(false));
        let hist_sub = Arc::new(HistSubscription {
            req_id,
            symbol: symbol.clone(),
            cancel: cancel_flag.clone(),
        });
        self.hist_subs.insert(req_id, hist_sub);
        let hist_subs = self.hist_subs.clone();

        // Emit Historical + End synchronously (no await between them).
        let send_failed = tx
            .send(HistoricalStreamEvent::Historical(bars.clone()))
            .await
            .is_err()
            || tx
                .send(HistoricalStreamEvent::End { first_ts, last_ts })
                .await
                .is_err();
        if send_failed {
            hist_subs.remove(&req_id);
        }

        // Spawn update-tail task.
        let tx_task = tx;
        let cancel_for_task = cancel_flag.clone();
        let sym = symbol.clone();
        let symbol_state = self.symbol_state.clone();
        let tf = bar_size;
        let update_cadence = Duration::from_secs(tf.as_secs().max(1));
        let handle = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(update_cadence);
            // First tick fires immediately — consume it so updates
            // land at full cadence.
            ticker.tick().await;
            loop {
                ticker.tick().await;
                if cancel_for_task.load(Ordering::SeqCst) {
                    break;
                }
                let price = symbol_state
                    .get(&sym)
                    .map(|s| s.market_price)
                    .unwrap_or(100.0);
                let ts_close = Utc::now();
                let ts_open = ts_close - chrono::Duration::seconds(tf.as_secs() as i64);
                let bar = Bar {
                    symbol: sym.clone(),
                    timeframe: tf,
                    ts_open,
                    ts_close,
                    o: price,
                    h: price,
                    l: price,
                    c: price,
                    volume: 1,
                    trade_count: 1,
                    wap: Some(price),
                    completeness: BarCompleteness::Partial,
                };
                if tx_task
                    .send(HistoricalStreamEvent::Update(bar))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });
        self.push_task(handle);

        let hist_subs = self.hist_subs.clone();
        let cancel_closure = Box::new(move || {
            cancel_flag.store(true, Ordering::SeqCst);
            hist_subs.remove(&req_id);
        });
        Ok(HistoricalStream::new(req_id, rx, cancel_closure))
    }

    async fn resolve_contract(
        &self,
        symbol: &SymbolKey,
        sec_type: SecurityType,
        exchange: &str,
    ) -> Result<ContractDetails, MarketDataError> {
        self.test_counters
            .resolve_contract_calls
            .fetch_add(1, Ordering::SeqCst);
        // Sim is permissive: every symbol resolves to a SMART-routed
        // stock quoted in USD with a 1-cent min tick.
        Ok(ContractDetails {
            contract_id: symbol.contract_id,
            symbol: symbol.symbol.clone(),
            sec_type,
            exchange: exchange.to_string(),
            primary_exchange: Some("NASDAQ".to_string()),
            currency: "USD".to_string(),
            long_name: Some(format!("{} (sim)", symbol.symbol)),
            min_tick: 0.01,
            multiplier: None,
            trading_class: None,
        })
    }

    fn farm_status(&self) -> broadcast::Receiver<FarmStatus> {
        self.farm_status_tx.subscribe()
    }

    fn connection_state(&self) -> watch::Receiver<ConnectionState> {
        self.conn_state_tx.subscribe()
    }

    fn name(&self) -> &str {
        "sim"
    }

    #[cfg(any(test, feature = "test_inject"))]
    fn inject_for_test(&self, event: MarketEvent) {
        // **OPEN**: wire this into the dev_harness replay path in S7.
        // For now: push `Tick` events into matching subscriptions and
        // `FarmStatus` into the farm broadcast. Other variants are
        // logged-and-ignored so callers don't blow up.
        match event {
            MarketEvent::Tick(t) => {
                // Route by (symbol, req_id): if req_id matches an
                // existing sub, fan to that one; else fan to every sub
                // on that symbol (broadcast semantics).
                let arc = Arc::new(t.clone());
                let targeted = self
                    .subscriptions
                    .get(&t.req_id)
                    .and_then(|sub| sub.tx.lock().as_ref().map(|tx| tx.send(arc.clone())))
                    .is_some();
                if !targeted {
                    for entry in self.subscriptions.iter() {
                        if entry.symbol != t.symbol {
                            continue;
                        }
                        if let Some(tx) = entry.tx.lock().as_ref() {
                            let _ = tx.send(arc.clone());
                        }
                    }
                }
            }
            MarketEvent::FarmStatus(fs) => {
                let _ = self.farm_status_tx.send(fs);
            }
            MarketEvent::ConnectionState(cs) => {
                let _ = self.conn_state_tx.send(cs);
            }
            MarketEvent::Bar(b) => {
                // Route bars by symbol to every matching rt-bar
                // subscription. Used by aggregator tests to inject
                // deterministic 5 s bars without waiting on the sim's
                // interval-based emitter.
                for entry in self.rt_bar_subs.iter() {
                    if entry.symbol == b.symbol {
                        let _ = entry.tx.send(Arc::new(b.clone()));
                    }
                }
            }
            _ => {
                // Silently drop; dev_harness owns richer replay.
            }
        }
    }
}

// ─── Helpers used only from tests / tick emitter ────────────────────────

/// Metadata helper for the tick emitter — pairs a subscription's
/// [`ReqId`] with its [`Notify`] waker.
#[allow(dead_code)]
pub(crate) struct SubscriptionMeta {
    pub(crate) req_id: ReqId,
    pub(crate) wake: Arc<Notify>,
}

#[cfg(test)]
#[allow(dead_code)]
pub(crate) fn new_test_sim_with(config: SimMarketDataConfig) -> Arc<SimMarketData> {
    SimMarketData::new(config)
}

#[cfg(test)]
mod internal_tests {
    use super::*;

    #[test]
    fn ohlcv_to_bar_copies_fields() {
        let sym = SymbolKey {
            contract_id: 1,
            symbol: "AAPL".into(),
        };
        let ohlcv = OhlcvBar {
            timestamp: 1_700_000_000,
            open: 100.0,
            high: 101.0,
            low: 99.0,
            close: 100.5,
            volume: 1_234,
        };
        let bar = ohlcv_to_bar(&ohlcv, sym.clone(), Timeframe::M1);
        assert_eq!(bar.symbol, sym);
        assert_eq!(bar.o, 100.0);
        assert_eq!(bar.volume, 1_234);
        assert_eq!(bar.ts_close - bar.ts_open, chrono::Duration::seconds(60));
    }

    /// Dropping a tick cancel closure outside a tokio runtime must
    /// not panic. Pre-fix the inner `tokio::spawn` unconditionally
    /// tried to schedule the deferred removal; post-fix we fall
    /// back to a synchronous `DashMap::remove` when no runtime is
    /// active.
    #[test]
    fn tick_cancel_closure_outside_tokio_does_not_panic() {
        // Build a minimal sub + map without a runtime.
        let req_id = ReqId(42);
        let subs: Arc<DashMap<ReqId, Arc<SimSubscription>>> = Arc::new(DashMap::new());
        let sub = Arc::new(SimSubscription {
            req_id,
            symbol: SymbolKey {
                contract_id: 1,
                symbol: "AAPL".into(),
            },
            con_id: 1,
            kind: SubKind::Tick,
            tx: Mutex::new(None),
            cancelled: AtomicBool::new(false),
            draining_until_ns: std::sync::atomic::AtomicU64::new(0),
            last_error: Arc::new(OnceLock::new()),
        });
        subs.insert(req_id, sub.clone());

        // Reproduce the closure shape inline (exercises the same
        // `try_current` fallback path the real closure uses).
        let late_window = Duration::from_millis(50);
        let subs_cl = subs.clone();
        let sub_cl = sub.clone();
        let closure: Box<dyn FnOnce() + Send + Sync> = Box::new(move || {
            sub_cl.cancelled.store(true, Ordering::SeqCst);
            let req_id = sub_cl.req_id;
            let subs = subs_cl.clone();
            match tokio::runtime::Handle::try_current() {
                Ok(h) => {
                    h.spawn(async move {
                        tokio::time::sleep(late_window).await;
                        subs.remove(&req_id);
                    });
                }
                Err(_) => {
                    subs.remove(&req_id);
                }
            }
        });
        // Invoke the closure from a plain `#[test]` (no runtime).
        // Pre-fix this would panic; post-fix the sub is removed
        // synchronously.
        closure();
        assert!(subs.get(&req_id).is_none(), "sub should be removed sync");
    }

    /// 100 subscribe/drop cycles must not leave 100 handles in
    /// `self.tasks`. With the pre-fix `push` only path the vec grew
    /// unboundedly; with `push_task_inner` the retain trims every
    /// finished `JoinHandle` before the next push.
    #[tokio::test]
    async fn push_task_prunes_finished_handles() {
        let tasks: Arc<Mutex<Vec<JoinHandle<()>>>> = Arc::new(Mutex::new(Vec::new()));
        // Spawn 100 tasks that finish immediately.
        for _ in 0..100 {
            let handle = tokio::spawn(async {
                // Immediately returns — the JoinHandle is reaped on
                // the next push_task_inner call.
            });
            push_task_inner(&tasks, handle);
            // Yield so the task actually finishes before the next
            // push_task_inner runs and has a chance to prune.
            tokio::task::yield_now().await;
        }
        // After the loop the vec size is bounded — the exact count
        // depends on scheduling but must be << 100.
        let len = tasks.lock().len();
        assert!(
            len <= 20,
            "tasks vec must not grow unboundedly; got {len} handles after 100 cycles"
        );
    }
}
