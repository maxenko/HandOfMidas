# Architecture Reference

## Layers and ownership

The refactor creates a new crate, `midas-market-data`, that sits between `midas-broker` (provider) and `midas-app` (consumers). No existing crate is deleted; the new crate replaces the ad-hoc routing inside `midas-app`. Crate placement:

- **Root workspace** (`Cargo.toml` at repo root):
  - `midas-broker-core` (existing) — shared types used across boundaries.
  - `midas-broker` (existing) — order engine + IB adapter + sim adapter.
  - `midas-market-data` (new) — router + aggregator + shared streaming types. Depends on `midas-broker-core` only; does NOT depend on `midas-broker`. The router holds a trait object, not a concrete broker.
- **Desktop workspace** (`desktop/win/Cargo.toml`):
  - `midas-app` depends on `midas-broker` + `midas-market-data`. The broker engine is instantiated by the app, then the router is constructed around the broker's `MarketDataSource` trait object.

Dependency edges:

```
 midas-broker-core  (types)
       ↑
 midas-broker       (trait MarketDataSource, trait OrderClient, impls)
                          ↑
                     midas-market-data  (router, aggregator, handles)
                          ↑
                     midas-app
```

`midas-market-data` does not depend on `midas-broker`'s concrete types; it reaches upstream via the trait objects boxed inside `MarketDataRouter::new(source: Arc<dyn MarketDataSource>)`.

### Startup and shutdown ordering (M-9)

1. `MarketDataSource` concrete impl (sim or IB) is constructed first; its internal tasks (tick emitter, error watcher, ibapi client) spawn here.
2. `Arc<dyn MarketDataSource>` is passed to `MarketDataRouter::new(source)`. The router spawns its control-plane actor, holding the `JoinHandle` in its `RouterState`.
3. The app stores `Arc<MarketDataRouter>` and `Arc<dyn OrderClient>` on `MidasApp`.
4. **Shutdown**: when `Arc<MarketDataRouter>` drops (last ref gone), router's own `Drop` sends `RouterMsg::Shutdown` to the control actor and awaits the `JoinHandle` with a 1 s timeout, then aborts. Each `SymbolHub`'s `TickStream`/`RealtimeBarStream` are dropped in the actor's shutdown path, cascading upstream cancels. The source's own background tasks are cancelled by the source's own `Drop`.
5. Handle drop during shutdown: `TickSubGuard::drop` sends `DecRef`; if the control channel is already closed (send returns `Err`), that's fine — the upstream is already being torn down.

## Core types

```rust
// midas-broker-core::market_data
pub struct Tick {
    pub symbol: SymbolKey,
    pub req_id: ReqId,
    pub kind: TickKind,           // Price | Size | Params | String | Generic
    pub tick_type: TickType,      // Bid | Ask | Last | BidSize | AskSize | LastSize | Volume | High | Low | Close | Open | HaltedState | ...
    pub value: TickValue,          // f64 | i64 | String | bool
    pub attrs: TickAttributes,     // can_auto_execute, past_limit, pre_open
    pub ts: DateTime<Utc>,         // event time
}

pub struct Bar {
    pub symbol: SymbolKey,
    pub timeframe: Timeframe,
    pub ts_open: DateTime<Utc>,
    pub ts_close: DateTime<Utc>,   // end of bar window
    pub o: f64, pub h: f64, pub l: f64, pub c: f64,
    pub volume: u64,
    pub completeness: BarCompleteness,  // Completed | Partial { ticks_folded: u32 }
}

// M-16: single canonical historical event shape; no per-bar `Bar` variant inside historical.
pub enum MarketEvent {
    Tick(Tick),
    Bar(Bar),                      // emitted by aggregator; not by raw source
    FarmStatus(FarmStatus),         // 2104 / 2106 / 2108 / 1100 / 1101 / 1102 / 2103 / 2105 / 2158
    ConnectionState(ConnectionState),
    OrderingReady { next_order_id: i32 },   // M-14: next-valid-id is its own event, not a FarmCode
    SubscriptionAccepted { req_id: ReqId, symbol: SymbolKey, stream: StreamKind },
    SubscriptionEnded { req_id: ReqId, reason: EndReason },
    Historical(Vec<Bar>),
    HistoricalDataEnd { req_id: ReqId, first_ts: DateTime<Utc>, last_ts: DateTime<Utc> },
    HistoricalUpdate(Bar),         // rust-ibapi 2.10 "update" event while keep_up_to_date
    Error { req_id: Option<ReqId>, code: ErrorCode, message: String },
}
```

The public handle exposed to consumers is `MarketEvent`-agnostic where possible — chart widgets subscribe to `impl Stream<Item = Bar>` via the aggregator, watchlist widgets subscribe to `watch::Receiver<Quote>`, and raw-tick consumers get `broadcast::Receiver<Arc<Tick>>`. The unified `MarketEvent` enum exists only for internal router plumbing and for consumers who want everything (e.g. a future audit-log).

## Provider traits (S2)

```rust
#[async_trait]
pub trait MarketDataSource: Send + Sync {
    /// Subscribe to level-1 sampled tick stream (reqMktData).
    /// IB samples at ~250 ms; sim emits at most one Last + one BidAsk set per
    /// sample window regardless of internal drift steps (BR-11).
    /// Drop of the handle auto-cancels upstream.
    async fn subscribe_ticks(
        &self,
        symbol: &SymbolKey,
        con_id: i32,
        generic_ticks: GenericTicks,   // BR-10: carries tick-233 (RT Volume), 293 (Trade Count), ...
    ) -> Result<TickStream, MarketDataError>;

    /// Subscribe to unsampled tick-by-tick stream (reqTickByTickData).
    /// IB caps 5 symbols / 15s identical-throttle; errors bubble back as MarketEvent::Error (BR-11).
    async fn subscribe_tick_by_tick(
        &self,
        symbol: &SymbolKey,
        con_id: i32,
        kind: TickByTickKind,   // Last | AllLast | BidAsk | MidPoint
    ) -> Result<TickStream, MarketDataError>;

    /// Subscribe to realtime 5-second bars (IB reqRealTimeBars).
    /// Separate from subscribe_ticks because IB treats them as separate wire requests.
    async fn subscribe_realtime_bars(
        &self,
        symbol: &SymbolKey,
        con_id: i32,
        what_to_show: WhatToShow,
    ) -> Result<RealtimeBarStream, MarketDataError>;

    /// Fetch historical bars one-shot (BR-9). Maps to rust-ibapi 2.10 `historical_data`.
    /// Returns all bars immediately; no live tail.
    async fn historical_bars(
        &self,
        symbol: &SymbolKey,
        con_id: i32,
        end: DateTime<Utc>,
        duration: IbDuration,            // M-2
        bar_size: Timeframe,
        what_to_show: WhatToShow,
        use_rth: bool,
    ) -> Result<HistoricalBarsResult, MarketDataError>;

    /// Fetch historical bars with live tail (BR-9). Maps to rust-ibapi 2.10
    /// `historical_data_streaming`. Stream emits Historical(bars) → End{first_ts,last_ts} → Update(bar) repeatedly.
    async fn historical_stream(
        &self,
        symbol: &SymbolKey,
        con_id: i32,
        duration: IbDuration,
        bar_size: Timeframe,
        what_to_show: WhatToShow,
        use_rth: bool,
    ) -> Result<HistoricalStream, MarketDataError>;

    /// Resolve a symbol to a fully-qualified contract (M-34). Without this,
    /// the router only handles SMART-routed US stocks.
    async fn resolve_contract(
        &self,
        symbol: &SymbolKey,
        sec_type: SecurityType,
        exchange: &str,
    ) -> Result<ContractDetails, MarketDataError>;

    /// Farm-state and connection-lifecycle subscription.
    /// Shared across all consumers via a single broadcast.
    fn farm_status(&self) -> broadcast::Receiver<FarmStatus>;

    /// Connection state watch.
    fn connection_state(&self) -> watch::Receiver<ConnectionState>;

    /// Test-only inject path (S7 dev-harness — BR-15). Real IB source returns
    /// `Err(MarketDataError::Unsupported)`; sim source pushes event into its hubs.
    #[cfg(any(test, feature = "test_inject"))]
    fn inject_for_test(&self, event: MarketEvent);
}

pub trait OrderClient: Send + Sync {
    /// M-12: async because IB's next valid id comes in via `nextValidId` watch.
    async fn next_order_id(&self) -> Result<i32, OrderError>;
    async fn place_order(&self, spec: OrderSpec) -> Result<PlaceOrderResult, OrderError>;
    /// BR-13: rust-ibapi 2.10 requires manual_order_cancel_time; returns a stream of CancelOrderEvent.
    async fn cancel_order(
        &self,
        ib_order_id: i32,
        manual_cancel_time: Option<DateTime<Utc>>,
    ) -> Result<CancelOrderStream, OrderError>;
    async fn modify_order(&self, ib_order_id: i32, spec: OrderModify) -> Result<(), OrderError>;
    /// M-21: recover local order state after reconnect.
    async fn open_orders(&self) -> Result<Vec<OpenOrder>, OrderError>;
    async fn completed_orders(&self) -> Result<Vec<CompletedOrder>, OrderError>;
    fn order_events(&self) -> broadcast::Receiver<OrderEvent>;
    fn position_events(&self) -> broadcast::Receiver<PositionUpdate>;
    fn account_events(&self) -> broadcast::Receiver<AccountEvent>;
}
```

Order and market-data traits are split because they have different lifecycle, different wire channels, and different test needs. A single backend (sim or IB) implements both.

`TickStream`, `RealtimeBarStream`, `HistoricalStream` are `Subscription<T>`-style handles (modeled after rust-ibapi): own an inner `Receiver`, implement `Drop` to send upstream cancel. Built on tokio `broadcast::Receiver` internally but expose a narrow async API (`.next().await`, `.error()`, `.req_id()`).

## Router (S5)

```rust
// midas-market-data::router
pub struct MarketDataRouter {
    control: mpsc::UnboundedSender<RouterMsg>,
    // Hot publish path bypasses the actor; control plane only.
    state: Arc<RouterState>,
}

struct RouterState {
    per_symbol: DashMap<SymbolKey, Arc<SymbolHub>>,
    contract_cache: DashMap<SymbolKey, ContractDetails>,   // NM-1: memoised resolve_contract results
}

// NB-7: publisher task is the SOLE owner of TickStream / RealtimeBarStream.
// Hub does not hold upstream subs; the actor aborts the publisher task on
// final DecRef, which drops the upstream and cancels via its Drop closure.
// NB-3: `watch_refcount` keeps the publisher alive while watchlist-only
// consumers hold `watch::Receiver<Quote>` (no broadcast receiver).
struct SymbolHub {
    ticks_tx: broadcast::Sender<Arc<Tick>>,   // cap 4096, Arc<Tick> to keep ring small
    last_quote_tx: watch::Sender<Quote>,
    rt_bars_tx: OnceLock<broadcast::Sender<Arc<Bar>>>,  // NB-6 Model A: router fans out RT bars
    tick_refcount: AtomicU32,
    watch_refcount: AtomicU32,   // NB-3
    rt_bar_refcount: AtomicU32,
    tick_publisher_task: Mutex<Option<JoinHandle<()>>>,     // NB-7: actor-managed lifecycle
    rt_bar_publisher_task: Mutex<Option<JoinHandle<()>>>,
    last_tick_ts: AtomicI64,   // for debug_dump (M-28)
}

enum RouterMsg {
    Subscribe { symbol, reply: oneshot::Sender<SubscriptionHandle<Tick>> },  // BR-4: actor builds handle
    IncRef { symbol, stream_kind: StreamKind },
    DecRef { symbol, stream_kind: StreamKind },
    Shutdown,
    // Upstream decoder publishes directly; does not route through this mpsc.
}

impl MarketDataRouter {
    pub fn new(source: Arc<dyn MarketDataSource>) -> Self { ... }

    /// Subscribe to ticks for a symbol. First subscriber triggers upstream.
    /// Handle's Drop sends DecRef; last drop triggers upstream.unsubscribe.
    /// BR-4: the actor constructs the SubscriptionHandle (rx + guard paired) and
    /// sends it back via oneshot. Caller cannot observe a refcount-without-guard state.
    /// NM-3: returns Result — source failure on first-subscribe flows back here
    /// without leaving a half-registered hub.
    pub async fn subscribe_ticks(&self, symbol: SymbolKey)
        -> Result<SubscriptionHandle<Tick>, MarketDataError> { ... }

    /// Per-symbol last-quote snapshot. Coalesced; suitable for watchlist cells.
    /// NB-3: bumps `watch_refcount` on the hub. If no hub exists, the actor
    /// lazy-opens the tick publisher (to populate the watch) without issuing
    /// any broadcast receivers. The returned receiver is wrapped in a small
    /// `QuoteHandle` that decrements `watch_refcount` on drop; the hub's
    /// publisher keeps running as long as either broadcast OR watch refcount
    /// is positive.
    pub async fn last_quote(&self, symbol: SymbolKey) -> QuoteHandle { ... }

    /// Bar-granularity subscription. Delegates to BarAggregatorRegistry.
    /// BR-22: returns Err(UnsupportedTimeframe) for D1/W1/M1/H4/RTH-aligned; those
    /// callers must use historical_bars (server-computed).
    pub async fn subscribe_bars(
        &self,
        symbol: SymbolKey,
        tf: Timeframe,
    ) -> Result<SubscriptionHandle<Bar>, MarketDataError> { ... }

    /// Unified history + live. Subscribes to ticks first, buffers live, fetches
    /// history, filters overlap by T_server, returns a single chained stream.
    pub async fn history_then_live(
        &self,
        symbol: SymbolKey,
        tf: Timeframe,
        lookback: IbDuration,    // M-2
    ) -> Result<impl Stream<Item = Bar>, MarketDataError> { ... }

    pub fn farm_status(&self) -> broadcast::Receiver<FarmStatus> { ... }
    pub fn connection_state(&self) -> watch::Receiver<ConnectionState> { ... }

    /// M-28: observability snapshot used by dev_harness DumpState.
    pub fn debug_dump(&self) -> Vec<SymbolDebugInfo> { ... }
}

/// BR-3: receiver is private; guard is a Box (non-clonable, since clones do NOT
/// IncRef and would destabilise the refcount). Handle is explicitly `!Clone`.
/// NB-1: `into_stream` folds rx+guard into a single Stream so the guard
/// drop tracks stream drop — preferred consumer API.
pub struct SubscriptionHandle<T> {
    rx: broadcast::Receiver<Arc<T>>,
    _guard: Box<dyn Guard>,
}

impl<T: Clone + Send + Sync + 'static> SubscriptionHandle<T> {
    pub async fn recv(&mut self) -> Result<Arc<T>, RecvError> { self.rx.recv().await }
    pub fn into_parts(self) -> (broadcast::Receiver<Arc<T>>, Box<dyn Guard>) { (self.rx, self._guard) }
    pub fn into_stream(self) -> impl Stream<Item = Arc<T>> {
        // Internally binds (rx, guard) as stream state so dropping the
        // stream drops the guard → DecRef.
        // (Full body shown in 06-slice-5-router.md §D.)
    }
}

pub trait Guard: Send + Sync {}

struct TickSubGuard {
    symbol: SymbolKey,
    stream_kind: StreamKind,   // Tick | RealtimeBar | Bar(tf)
    router: mpsc::UnboundedSender<RouterMsg>,
}

impl Drop for TickSubGuard {
    fn drop(&mut self) {
        let _ = self.router.send(RouterMsg::DecRef {
            symbol: self.symbol.clone(),
            stream_kind: self.stream_kind,
        });
    }
}
impl Guard for TickSubGuard {}
```

### Hot vs cold paths

- **Hot (upstream decode → consumer fan-out)**: the upstream `TickStream` publisher task **owns an `Arc<SymbolHub>` captured at spawn time** (BR-18) AND is the **sole owner of the upstream `TickStream` itself** (NB-7). It never consults the DashMap during a tick — the shard-lock churn during subscribe/unsubscribe would stall publishers. On each tick: `hub.ticks_tx.send(Arc::new(tick))` + inline `update_last_quote(&hub.last_quote_tx, ...)`. Zero map lookups on the hot path.
- **Cold (subscribe/unsubscribe/refcount)**: goes through the control actor (`RouterMsg`) so mutations to `per_symbol` serialize. DashMap is consulted only here; the actor inserts a new `Arc<SymbolHub>`, then spawns a publisher task with a cloned `Arc<SymbolHub>` so the hot path never touches the map again. On final DecRef the actor calls `hub.tick_publisher_task.lock().take().map(JoinHandle::abort)` — aborting drops the publisher, which drops its `TickStream`, which cancels upstream via the Drop closure (NB-7).
- **Publisher auto-exit** (M-4 + NB-3): publisher task checks `hub.ticks_tx.receiver_count() == 0` AND `hub.last_quote_tx.receiver_count() == 0` after each send — watchlist-only consumers hold the watch but not the broadcast, so both must be idle before auto-exit. After N (default 16) consecutive zero cycles, the task exits and cascades via its `TickStream` Drop (upstream cancel). Aggregator / RT-bar publishers do the same against `rt_bars_tx`.

### History+live seam

BR-7 rewrite: no spawned buffering task (the old shape leaked the spawned task + upstream sub if the caller dropped the returned stream). NB-2 fix: use `SubscriptionHandle::into_stream()` so rx AND guard move into the returned stream together — dropping the stream drops the guard, which DecRefs upstream.

```rust
pub async fn history_then_live(&self, symbol, tf, lookback)
    -> Result<impl Stream<Item = Bar>, MarketDataError>
{
    // 1. Subscribe to aggregator bars FIRST. into_stream() binds rx+guard.
    let live_handle = self.subscribe_bars(symbol.clone(), tf).await?;
    let live_stream = live_handle.into_stream().map(|arc_bar| (*arc_bar).clone());

    // 2. Fetch history to completion synchronously.
    let end = Utc::now();
    let hist = self.source.historical_bars(   // BR-9: one-shot API
        &symbol, 0, end, lookback, tf, WhatToShow::Trades, true,
    ).await?;
    let HistoricalBarsResult { bars: hist_bars, last_ts: t_server, .. } = hist;

    // 3. M-35: filter live by `ts_open > t_server` (not ts_close — avoids boundary dup).
    let filtered_tail = live_stream.filter(move |bar| {
        futures::future::ready(bar.ts_open > t_server)
    });

    Ok(stream::iter(hist_bars).chain(filtered_tail))
}
```

The `t_server` is whatever the provider reports as "history covers up to [start, T_server)". For IB, that's `historicalDataEnd.end_date`. For sim, we synthesize one from a test-configurable `last_ts` (BR-21) at the moment historical generation completes. This seam is the same utility function for both; consumers never implement it.

## Aggregator (S6)

BR-5: the registry is fully async; there is no `block_on`. First-request-for-key init is serialised by a per-key `OnceCell`-style guard over `tokio::sync::Mutex<HashMap<Key, Arc<AggregatorEntry>>>`. BR-6: `Weak<MarketDataRouter>` is never `.unwrap()`-ed; on upgrade failure the registry returns early (router is being torn down). BR-12 + NB-6 Model A: the aggregator source is **`router.subscribe_rt_bars(sym)`** (which returns `SubscriptionHandle<Bar>` wrapping a refcounted fan-out of IB's 5s bars per `WhatToShow::Trades`), not a tick-folding path. Tick-based aggregation miscounts volume vs. IB's own 5s bars; sharing upstream via the router avoids duplicate IB subscriptions when two aggregators (e.g. `(AAPL, M1)` and `(AAPL, M5)`) need the same symbol.

**Note:** the code sketches below are indicative; the authoritative type shape and method names are in [`07-slice-6-aggregator.md`](07-slice-6-aggregator.md) §B/§C.

```rust
// midas-market-data::aggregator
pub struct BarAggregatorRegistry {
    aggregators: tokio::sync::Mutex<HashMap<(SymbolKey, Timeframe), Arc<AggregatorEntry>>>,
    router: Weak<MarketDataRouter>,  // BR-6: don't unwrap
}

struct AggregatorEntry {
    bars_tx: broadcast::Sender<Arc<Bar>>,
    refcount: AtomicU32,
    last_bar: RwLock<Option<Bar>>,   // M-28 / snapshot resync
    _task: JoinHandle<()>,
}

impl BarAggregatorRegistry {
    pub async fn subscribe(&self, symbol: SymbolKey, tf: Timeframe)
        -> Result<SubscriptionHandle<Bar>, MarketDataError>
    {
        // BR-22: reject unsupported timeframes upfront.
        if matches!(tf, Timeframe::D1 | Timeframe::W1 | Timeframe::M1Monthly | Timeframe::H4) {
            return Err(MarketDataError::UnsupportedTimeframe(tf));
        }
        let router = self.router.upgrade().ok_or(MarketDataError::ShuttingDown)?;  // BR-6
        let key = (symbol.clone(), tf);
        let mut map = self.aggregators.lock().await;
        let entry = if let Some(e) = map.get(&key) { e.clone() } else {
            // BR-12 + NB-6 Model A: aggregator subscribes via the router's
            // refcounted fan-out, not directly to the source. Two aggregators
            // on the same symbol (different tfs) share one upstream.
            let rt_handle = router.subscribe_rt_bars(symbol.clone()).await?;
            let (bars_tx, _) = broadcast::channel(256);
            let entry = Arc::new(AggregatorEntry {
                bars_tx: bars_tx.clone(),
                refcount: AtomicU32::new(0),
                last_bar: RwLock::new(None),
                _task: tokio::spawn(run_aggregator(rt_handle, tf, bars_tx)),
            });
            map.insert(key.clone(), entry.clone());
            entry
        };
        entry.refcount.fetch_add(1, Ordering::Relaxed);
        let rx = entry.bars_tx.subscribe();
        drop(map);  // release before handing handle back
        Ok(SubscriptionHandle {
            rx,
            _guard: Box::new(AggGuard { key, registry: Arc::downgrade(&self.weak_self()) }),
        })
    }
}

async fn run_aggregator(
    mut rt_handle: SubscriptionHandle<Bar>,   // NB-6 Model A: shared fan-out from router
    tf: Timeframe,
    bars_tx: broadcast::Sender<Arc<Bar>>,
) {
    let mut current: Option<Bar> = None;
    let mut coalesce = tokio::time::interval(Duration::from_millis(100));  // M-26
    let mut dirty = false;
    loop {
        tokio::select! {
            r = rt_sub.next() => match r {
                Ok(rt_bar) => {
                    // Fold each 5s bar into the target timeframe.
                    let window = align_to_window(rt_bar.ts_open, tf);
                    match current.as_mut() {
                        Some(bar) if bar.ts_open == window => {
                            bar.c = rt_bar.c;
                            bar.h = bar.h.max(rt_bar.h);
                            bar.l = bar.l.min(rt_bar.l);
                            bar.volume += rt_bar.volume;
                            dirty = true;   // M-26: coalesce partial emits
                        }
                        _ => {
                            if let Some(mut prev) = current.take() {
                                prev.completeness = BarCompleteness::Completed;
                                let _ = bars_tx.send(Arc::new(prev));   // bar-close emits immediately
                            }
                            current = Some(Bar::new_starting_at(window, tf, rt_bar));
                            dirty = true;
                        }
                    }
                }
                Err(Lagged(_)) => { current = None; dirty = false; }   // M-5/M-11
                Err(Closed) => return,
            },
            _ = coalesce.tick(), if dirty => {
                if let Some(bar) = &current {
                    let _ = bars_tx.send(Arc::new(bar.clone()));
                    dirty = false;
                }
            }
        }
    }
}
```

One aggregator task per `(symbol, timeframe)`, lazily spawned, refcounted via RAII. Consumers on the same `(sym, tf)` share the same `broadcast::Sender<Arc<Bar>>`. Consumers on different `tf` for the same symbol share the same upstream **realtime-bar** subscription but run independent aggregators.

## App-side consumers (S7 / S8)

Every consumer obtains its own `SubscriptionHandle` and owns its own lifetime. No central match arm. Three canonical consumers:

### Chart widget (iced Subscription per visible chart)

```rust
impl MidasApp {
    fn subscription(&self) -> Subscription<Message> {
        // NB-4: placeholder is None while connecting; no subscriptions issued yet.
        let Some(router) = self.router.clone() else { return Subscription::none(); };
        let mut subs = vec![];
        for chart in self.charts.values().filter(|c| c.is_visible()) {
            let sym = chart.bound_symbol.clone().unwrap();
            let tf = chart.timeframe;
            let router = router.clone();
            subs.push(iced::subscription::channel(
                ("chart-bars", chart.id, sym.clone(), tf), 64,
                move |mut out| async move {
                    // NB-1: recv() via &mut handle; handle owns rx + guard so
                    // dropping handle at closure end DecRefs upstream.
                    let mut handle = match router.subscribe_bars(sym, tf).await {
                        Ok(h) => h, Err(_) => return,
                    };
                    let mut pending: Vec<Bar> = Vec::new();
                    let mut interval = tokio::time::interval(Duration::from_millis(16));
                    loop {
                        tokio::select! {
                            r = handle.recv() => match r {
                                Ok(bar) => pending.push((*bar).clone()),
                                Err(Lagged(_)) => { let _ = out.send(Message::ChartResync(chart.id)).await; }
                                Err(Closed) => break,
                            },
                            _ = interval.tick() => if !pending.is_empty() {
                                let _ = out.send(Message::ChartBarBatch(chart.id, std::mem::take(&mut pending))).await;
                            }
                        }
                    }
                }
            ));
        }
        Subscription::batch(subs)
    }
}
```

- Subscription is keyed on `(chart_id, symbol, timeframe)` — iced diffs this and tears down when the chart closes or the symbol/tf changes.
- Handle's Drop propagates through iced's subscription lifecycle.
- Frame-coalesce inside the task: at most one `Message::ChartBarBatch` per 16 ms per chart.
- `Lagged` → `ChartResync`: consumer re-reads latest snapshot from aggregator's `last_bar()` accessor.

### Watchlist row (per-symbol watch receiver)

```rust
fn watchlist_subscription(&self) -> Subscription<Message> {
    let router = self.router.clone();
    let symbols: Vec<SymbolKey> = /* union of watchlist symbols */;
    iced::subscription::channel(("watchlist-quotes", symbols.len()), 256,
        move |mut out| async move {
            let handles: Vec<_> = join_all(symbols.iter().map(|s| router.last_quote(s.clone()))).await;
            // Multiplex `changed()` across all watches into one batched message.
            let mut interval = tokio::time::interval(Duration::from_millis(50));
            loop {
                interval.tick().await;
                let snapshot: Vec<(SymbolKey, Quote)> = handles.iter()
                    .zip(&symbols)
                    .filter_map(|(h, s)| /* drain if changed */)
                    .collect();
                if !snapshot.is_empty() {
                    let _ = out.send(Message::QuoteBatch(snapshot)).await;
                }
            }
        }
    )
}
```

### TickerState tick consumer

TickerState needs ticks for its `UpdateMarketData` transition. It subscribes once per active chart symbol — the iced subscription is keyed on the set of symbols that have at least one TickerState consumer. Identical coalescing pattern.

## Legacy elimination

After S7+S8 land:

- Delete `MidasApp::active_market_subs` field.
- Delete `MidasApp::ensure_market_subscriptions` method.
- Delete the `BrokerEvent::Tick` match arm in `handlers.rs` (the five hard-coded side effects).
- Delete the `BrokerEvent` → `Message::BrokerEventReceived` translation for Tick events; router owns that dispatch.
- `broker_event_stream` keeps order + connection events but Tick/BarUpdated/BarClosed routes through router.
- Delete `chart.data: Option<Arc<CandleBuffer>>` mutation from the tick path. The chart widget holds an aggregator subscription; its bar state is reconstructed from the stream.

## Invariants for correctness

1. **Single logical subscription per (symbol, stream kind)** from router upstream. Never two concurrent tick subs for the same symbol from the router, even with N consumers.
2. **RAII refcount** — the ONLY way to decrement is dropping the handle. No manual `unsubscribe()` API exists publicly.
3. **No gap or duplicate at history/live seam** — the seam util guarantees `last_historical.ts_close < first_live.ts_close`. The router filters live events with `ts > t_server` as the guarantee.
4. **Farm-state dispatch is separate** — farm events don't flow through per-symbol hubs; they go on their own broadcast that all consumers can subscribe to.
5. **Order of delivery is preserved per symbol** — within one symbol's broadcast, ticks arrive in wire order. The router does not reorder.

## Performance budget

- Router hot publish path: ≤ 200 ns per tick (DashMap read + broadcast::send + watch::send). Benchmark as part of S5.
- Router subscribe call: ≤ 100 µs p99 (one mpsc message to actor, one oneshot reply).
- Aggregator per-tick work: ≤ 500 ns (bar update) / ≤ 5 µs (bar close + send).
- iced Subscription batch rate: 16 ms cadence, max 3 `Message` per chart per batch.
- Memory: 4096-cap broadcast per symbol × 1 KB per `Arc<Tick>` × N symbols. For 100 symbols worst-case: 400 MB if all rings saturated, but rings only grow to receiver lag; realistic footprint is tens of MB.
