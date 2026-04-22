# Slice 5 — MarketDataRouter

**Goal.** Introduce the `midas-market-data` crate with `MarketDataRouter`, RAII `SubscriptionHandle<T>`, per-symbol `broadcast` + `watch` fan-out, refcounted upstream subscriptions, and the `history_then_live` seam utility.

## Scope

### A. New crate

`crates/midas-market-data/` in the root workspace.

`Cargo.toml`:
```toml
[package]
name = "midas-market-data"
version = "0.1.0"
edition = "2021"

[dependencies]
midas-broker-core = { path = "../midas-broker-core" }
tokio = { version = "1", features = ["rt-multi-thread", "macros", "sync", "time"] }
tokio-stream = "0.1"
async-trait = "0.1"
thiserror = "1"
futures = "0.3"
dashmap = "5"
mailbox_processor = { path = "../mailbox_processor" }   # moved in S0 prep (BR-16)
tracing = "0.1"
chrono = "0.4"

[dev-dependencies]
midas-broker = { path = "../midas-broker" }
tokio = { version = "1", features = ["test-util"] }
```

**S0 prep (BR-16) already moved `mailbox_processor` from `desktop/win/crates/mailbox_processor` to `crates/mailbox_processor` at the root workspace.** That commit landed before S1; this slice just depends on the new path.

### B. Router struct

`src/router.rs`:

```rust
pub struct MarketDataRouter {
    state: Arc<RouterState>,
    control: mpsc::UnboundedSender<RouterMsg>,
}

struct RouterState {
    source: Arc<dyn MarketDataSource>,
    per_symbol: DashMap<SymbolKey, Arc<SymbolHub>>,
    aggregator_registry: Arc<BarAggregatorRegistry>,  // S6
    contract_cache: DashMap<SymbolKey, ContractDetails>,   // NM-1: memoised resolve_contract results
}

// NB-7: publisher task is the SOLE TickStream / RealtimeBarStream owner.
// The hub no longer stores `upstream_tick` / `upstream_rt_bars`; the actor
// tracks liveness via the `*_publisher_task` JoinHandle and aborts on final
// DecRef. Aborting the task drops its TickStream / RealtimeBarStream, whose
// Drop closure cancels upstream.
struct SymbolHub {
    ticks_tx: broadcast::Sender<Arc<Tick>>,          // cap 4096
    last_quote_tx: watch::Sender<Quote>,
    rt_bars_tx: OnceLock<broadcast::Sender<Arc<Bar>>>,   // NB-6 Model A: router fans out RT bars
    tick_refcount: AtomicU32,
    watch_refcount: AtomicU32,   // NB-3: last_quote consumers tracked separately
    rt_bar_refcount: AtomicU32,
    tick_publisher_task: Mutex<Option<JoinHandle<()>>>,     // NB-7: actor aborts on final DecRef
    rt_bar_publisher_task: Mutex<Option<JoinHandle<()>>>,   // NB-7
    last_tick_ts: AtomicI64,   // M-28
}

// BR-4: actor reply carries the already-constructed SubscriptionHandle, not a raw receiver.
enum RouterMsg {
    SubscribeTicks { symbol: SymbolKey, reply: oneshot::Sender<Result<SubscriptionHandle<Tick>, MarketDataError>> },
    SubscribeRtBars { symbol: SymbolKey, reply: oneshot::Sender<Result<SubscriptionHandle<Bar>, MarketDataError>> },   // NB-6
    OpenHubForWatch { symbol: SymbolKey, reply: oneshot::Sender<watch::Receiver<Quote>> },  // NB-3: lazy-open for last_quote consumers
    DecTickRef { symbol: SymbolKey },
    DecRtBarRef { symbol: SymbolKey },
    DecWatchRef { symbol: SymbolKey },   // NB-3
    FarmDropAll { cause: FarmCode },   // M-20
    DebugDump { reply: oneshot::Sender<Vec<SymbolDebugInfo>> },   // M-28
    Shutdown,
}
```

`MarketDataRouter::new(source)` spawns the control actor. The actor owns the DashMap mutations — control-plane only. **Hot publish path does NOT consult the DashMap (BR-18)**: the publisher task owns its `Arc<SymbolHub>` directly, captured at spawn time.

**NM-4: explicit `Arc::new_cyclic` construction.** The aggregator registry holds a `Weak<MarketDataRouter>` (BR-6); the router must be wired through `new_cyclic` so the weak pointer is live at construction.

```rust
impl MarketDataRouter {
    pub fn new(source: Arc<dyn MarketDataSource>) -> Arc<Self> {
        Arc::new_cyclic(|weak_self| {
            let aggregator_registry = Arc::new(BarAggregatorRegistry::new(weak_self.clone()));
            let (control_tx, control_rx) = mpsc::unbounded_channel();
            let state = Arc::new(RouterState {
                source,
                per_symbol: DashMap::new(),
                aggregator_registry,
                contract_cache: DashMap::new(),
            });
            tokio::spawn(run_control_actor(control_rx, state.clone()));
            MarketDataRouter { state, control: control_tx }
        })
    }

    /// NB-6 Model A: subscribe to realtime 5s bars for a symbol, fanned out
    /// through the router's per-symbol hub. First subscriber opens the
    /// upstream `source.subscribe_realtime_bars`; subsequent subscribers share
    /// the same broadcast. Aggregators at different timeframes on the same
    /// symbol therefore share ONE upstream IB request.
    pub async fn subscribe_rt_bars(&self, symbol: SymbolKey)
        -> Result<SubscriptionHandle<Bar>, MarketDataError>
    {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.control.send(RouterMsg::SubscribeRtBars { symbol, reply: reply_tx })
            .map_err(|_| MarketDataError::ShuttingDown)?;
        reply_rx.await.map_err(|_| MarketDataError::ShuttingDown)?
    }

    /// NM-1: resolve `con_id` (and cache the ContractDetails) before any
    /// first-subscribe path that passes con_id to the source. Default
    /// `(SecurityType::Stock, "SMART")` suffices for US equities; future
    /// options/futures slice will add a contract-builder surface.
    async fn resolve_or_cached(&self, sym: &SymbolKey)
        -> Result<ContractDetails, MarketDataError>
    {
        if let Some(c) = self.state.contract_cache.get(sym) {
            return Ok(c.clone());
        }
        let c = self.state.source
            .resolve_contract(sym, SecurityType::Stock, "SMART")
            .await?;
        self.state.contract_cache.insert(sym.clone(), c.clone());
        Ok(c)
    }
}
```

### C. Subscribe / refcount flow (BR-4 — actor constructs handle; NM-3 rollback order; NB-7 publisher ownership)

Handle construction lives **in the actor**, not the caller. The actor sends back a fully-formed `SubscriptionHandle` (receiver + guard paired) via `oneshot`. If the caller's future is dropped after the actor replied but before the oneshot is observed, the oneshot drop drops the handle which invokes `TickSubGuard::drop` which sends `DecTickRef` — DecRef matches IncRef exactly.

**NM-3: source-failure rollback.** On first-subscribe-per-symbol, the actor calls the source FIRST. If it errors, the actor replies `Err(...)` via the oneshot and inserts NOTHING into `per_symbol`. The hub is only built and inserted after the upstream call returns Ok. No half-registered state survives a source failure.

**First subscribe to symbol X (ticks)**:
1. `router.subscribe_ticks(X)` sends `SubscribeTicks { X, reply }` on `control` and awaits the oneshot.
2. Actor receives. Looks up `per_symbol[X]`:
   - **If absent**:
     1. `contract_details = router.resolve_or_cached(&X).await?` (NM-1).
        On Err → `reply.send(Err(e))`, no insertion. Return.
     2. `upstream = source.subscribe_ticks(&X, contract_details.con_id, generic_ticks).await`.
        On Err → `reply.send(Err(e))`, no insertion. Return.
     3. Build `Arc<SymbolHub>` with fresh broadcast channels, `tick_refcount = 1`, empty publisher-task slots.
     4. Spawn `run_tick_publisher(hub.clone(), upstream)` — the spawned task is the SOLE owner of `TickStream` (NB-7). Store its `JoinHandle` in `hub.tick_publisher_task`.
     5. Insert `hub` into `per_symbol` map.
   - **If present**: `tick_refcount += 1`; no upstream call, no publisher spawn.
3. Build `SubscriptionHandle { rx: hub.ticks_tx.subscribe(), _guard: Box::new(TickSubGuard { symbol: X, router: control_clone }) }` (BR-3: private rx, Boxed guard, `!Clone`).
4. `reply.send(Ok(handle))`.

Same flow applies for `SubscribeRtBars` (NB-6 Model A) using `hub.rt_bars_tx` + `hub.rt_bar_refcount` + `hub.rt_bar_publisher_task` + `source.subscribe_realtime_bars(&X, con_id, WhatToShow::Trades)`. `rt_bars_tx` is lazily `OnceLock`-set on first RT-bar subscribe; if a tick hub already exists for X, RT-bar init reuses that hub and just populates the RT fields.

**Lazy hub-open for watchlist (NB-3)**: `router.last_quote(X)` sends `RouterMsg::OpenHubForWatch { X, reply }`. The actor:
  1. If no hub exists for X, runs the first-subscribe path above for ticks (so publisher is spawned and will populate `last_quote_tx`). `tick_refcount` stays at 0 because no broadcast handle is issued.
  2. `hub.watch_refcount += 1`.
  3. Replies with `hub.last_quote_tx.subscribe()`.

A `WatchGuard` returned to the caller alongside the `watch::Receiver` (wrapped in a small `QuoteHandle` — exact shape is implementer judgment; it must `DecWatchRef` on drop and otherwise deref to `watch::Receiver<Quote>`) sends `DecWatchRef` on drop.

**Second subscribe to same symbol (ticks)**:
1. `SubscribeTicks { X, reply }`.
2. Actor finds existing hub, `tick_refcount += 1`.
3. Builds a new `SubscriptionHandle` (new receiver on the same broadcast + fresh guard) and replies.
4. No upstream call.

**Drop handle**:
1. `TickSubGuard::drop` fires `DecTickRef { X }` on `control`.
2. Actor decrements. If `tick_refcount == 0` AND `watch_refcount == 0` (NB-3):
   - `hub.tick_publisher_task.lock().take().map(|h| h.abort())` — aborting the task drops the `TickStream` it owns, whose Drop closure cancels upstream. Non-blocking; no `block_on` (NB-7).
   - If `rt_bar_refcount == 0` as well: abort `rt_bar_publisher_task` and remove hub from `per_symbol`. If RT bars are still live, keep the hub entry — the rt-bar publisher still updates `last_tick_ts` via its own path (or leaves it stale; acceptable).

The `control` channel is unbounded because drops need to be non-blocking. Under extreme drop rate (app shutdown), the backlog is bounded by the number of handles — finite.

### D. `SubscriptionHandle<T>` (BR-3, NB-1)

Receiver is **private**; consumers call `recv()`, `into_stream()`, or `into_parts()`. Guard is a **`Box<dyn Guard>`** — NOT an `Arc`, because clones of the guard would not IncRef and would break refcounting. `SubscriptionHandle<T>` is explicitly `!Clone` (no `#[derive(Clone)]`).

```rust
pub struct SubscriptionHandle<T> {
    rx: broadcast::Receiver<Arc<T>>,      // private (NB-1)
    _guard: Box<dyn Guard>,               // non-clonable
}

// No #[derive(Clone)]; explicitly not cloneable.
impl<T: Clone + Send + Sync + 'static> SubscriptionHandle<T> {
    /// Borrow-based receive. Handle retains ownership of both rx and guard,
    /// so refcounting stays intact across recv() calls. (NB-1)
    pub async fn recv(&mut self) -> Result<Arc<T>, broadcast::error::RecvError> {
        self.rx.recv().await
    }

    /// Consume into (receiver, guard). Caller is responsible for holding the
    /// guard alive alongside the receiver — dropping the guard immediately
    /// decrements upstream refcount. Prefer `into_stream()` unless the guard
    /// must live inside a separate task-local slot. (NB-1)
    pub fn into_parts(self) -> (broadcast::Receiver<Arc<T>>, Box<dyn Guard>) {
        (self.rx, self._guard)
    }

    /// Consume into a Stream that internally owns both rx and guard. Dropping
    /// the returned stream drops the guard, which cascades the DecRef. This is
    /// the preferred consumer API — it makes the "guard lives as long as the
    /// stream" invariant structural. (NB-1, NB-2)
    pub fn into_stream(self) -> impl futures::Stream<Item = Arc<T>> {
        let (rx, guard) = self.into_parts();
        // The closure captures `guard`; dropping the stream drops the closure state.
        let stream = tokio_stream::wrappers::BroadcastStream::new(rx)
            .filter_map(|r| async move { r.ok() });
        // Attach the guard to the stream's lifetime via a simple wrapper.
        futures::stream::unfold((stream, guard), |(mut s, g)| async move {
            match futures::StreamExt::next(&mut s).await {
                Some(item) => Some((item, (s, g))),
                None => None,   // stream ended; (s, g) dropped → guard drops → DecRef
            }
        })
    }

    /// Re-subscribe gives a fresh receiver on the same broadcast channel but
    /// does NOT increment the refcount; its use is only safe while the
    /// original handle (which holds the guard) is still alive.
    pub fn resubscribe(&self) -> broadcast::Receiver<Arc<T>> { self.rx.resubscribe() }
}

pub trait Guard: Send + Sync {}

struct TickSubGuard {
    symbol: SymbolKey,
    router: mpsc::UnboundedSender<RouterMsg>,
}

impl Drop for TickSubGuard {
    fn drop(&mut self) {
        let _ = self.router.send(RouterMsg::DecTickRef { symbol: self.symbol.clone() });
    }
}
impl Guard for TickSubGuard {}
```

Type erasure via `Box<dyn Guard>` keeps the handle struct `T`-generic without propagating the guard type parameter. Generic over `T = Tick | Bar`.

### E. Publisher task (BR-18, NB-3 watch-aware auto-exit, NB-7 sole TickStream owner)

The publisher owns its `Arc<SymbolHub>` directly. **It is also the sole owner of `TickStream` (NB-7)** — dropping the task (via `JoinHandle::abort` from the actor on last DecRef) drops the TickStream, which cancels upstream via its Drop closure. The hub no longer holds `upstream_tick`. The publisher never reads the DashMap — zero shard-lock contention on the hot path.

```rust
async fn run_tick_publisher(
    hub: Arc<SymbolHub>,       // BR-18: captured at spawn; not re-looked-up per tick
    mut upstream: TickStream,  // NB-7: sole owner; dropped when task is aborted
) {
    let mut zero_receivers_streak: u32 = 0;   // M-4: auto-exit after idle
    while let Ok(tick) = upstream.next().await {
        let arc_tick = tick;  // already Arc<Tick>
        hub.last_tick_ts.store(arc_tick.ts.timestamp_millis(), Ordering::Relaxed);
        // Fan out to broadcast.
        let _ = hub.ticks_tx.send(arc_tick.clone());
        // Update last-quote watch (only for Price / PriceSize events).
        if matches!(arc_tick.kind, TickKind::Price | TickKind::PriceSize) {
            update_last_quote(&hub.last_quote_tx, &arc_tick);
        }
        // M-4 + NB-3: auto-exit requires BOTH broadcast and watch receivers to
        // be idle. Watchlist-only consumers subscribe via `last_quote` which
        // returns a `watch::Receiver<Quote>` — no broadcast receiver is issued,
        // so checking `ticks_tx.receiver_count()` alone would auto-exit while
        // watchlist is actively reading. `watch::Sender::receiver_count()`
        // requires tokio >= 1.29 (see 01a-slice-0-prep.md).
        let broadcast_active = hub.ticks_tx.receiver_count() > 0;
        let watch_active = hub.last_quote_tx.receiver_count() > 0;
        if !broadcast_active && !watch_active {
            zero_receivers_streak = zero_receivers_streak.saturating_add(1);
            if zero_receivers_streak >= 16 { return; }
        } else {
            zero_receivers_streak = 0;
        }
    }
    // Upstream closed; drop cascades; consumers see RecvError::Closed.
}
```

`update_last_quote` merges bid/ask/last into the `Quote` struct and calls `tx.send(new_quote)` only if it differs from the last value (to avoid unnecessary wakeups). M-27: spans on `subscribe_ticks`, last-drop DecRef, Lagged, history seam, farm-status, pacing queue/reject decisions.

The RT-bar publisher (`run_rt_bar_publisher`) mirrors this shape against `hub.rt_bars_tx` and `hub.rt_bar_refcount`; auto-exit checks only `rt_bars_tx.receiver_count()` (there is no watch counterpart for RT bars).

### F. History + live seam (BR-7 rewrite, NB-2)

No spawned buffering task. The live subscription's rx AND guard are folded into the returned stream via `into_stream()`, so the guard lives as long as the stream — dropping the returned stream cascades the upstream cancel. **Do NOT read `live_handle.rx` directly; that partially-moves the handle and drops the guard on the next scope exit before any consumer polls** (original bug NB-2 fix).

```rust
pub async fn history_then_live(
    &self,
    symbol: SymbolKey,
    tf: Timeframe,
    lookback: IbDuration,   // M-2
) -> Result<impl Stream<Item = Bar>, MarketDataError> {
    // 1. Subscribe to aggregator bars FIRST. `into_stream()` binds guard+rx
    //    into the returned stream; dropping the stream drops the guard and
    //    DecRefs upstream. (NB-1, NB-2)
    let live_handle = self.subscribe_bars(symbol.clone(), tf).await?;
    let live_stream = live_handle.into_stream().map(|arc_bar| (*arc_bar).clone());

    // 2. Fetch history one-shot (BR-9).
    let end = Utc::now();
    let hist = self.state.source.historical_bars(
        &symbol, 0, end, lookback, tf, WhatToShow::Trades, true,
    ).await?;
    let HistoricalBarsResult { bars: hist_bars, last_ts: t_server, .. } = hist;

    // 3. M-35: filter live by `ts_open > t_server`, not `ts_close`, to avoid the
    //    boundary-duplicate case (a bar whose open == t_server is already in hist).
    let filtered_tail = live_stream.filter(move |bar| {
        let keep = bar.ts_open > t_server;
        futures::future::ready(keep)
    });

    Ok(stream::iter(hist_bars).chain(filtered_tail))
}
```

Note: the returned stream owns the live handle's guard via `into_stream()`; there is no way to partially-move and drop the guard early.

### G. Tests

`crates/midas-market-data/tests/router_behavior.rs` (uses `SimMarketData` from slice 3). BR-20: timing tests use `#[tokio::test(start_paused = true)]` + `tokio::time::advance`.

1. `single_subscribe_opens_one_upstream` — subscribe twice to AAPL, assert the sim received exactly one `subscribe_ticks` call.
2. `last_drop_cancels_upstream` — subscribe once, drop, assert sim received one `cancel`.
3. `multiple_drops_decrement_refcount` — subscribe 3 times, drop 2, assert no cancel. Drop the 3rd, assert cancel.
4. `lagged_consumer_does_not_block_producer` — a consumer that doesn't drain gets `RecvError::Lagged`; the producer continues unimpeded.
5. `last_quote_watch_coalesces` — fire 1000 ticks, a `watch::Receiver` consumer reads at 50 ms intervals and never blocks. (BR-20 paused-time.)
6. `history_then_live_deterministic_seam` (BR-21) — configure sim with `historical_last_ts = Some(t0)`. Assert: bars with `ts_open <= t0` are served from history; bars with `ts_open > t0` are served from live; no duplicates, no gaps, exact sequence.
7. `history_then_live_filters_boundary` (M-35) — live stream produces a bar with `ts_open == t_server`; consumer observes the copy from history, NOT from live.
8. `publisher_task_aborts_on_shutdown` — drop all handles, then drop router, assert upstream subscription is cancelled.
9. `handle_is_not_clone` — compile-test (doc test or `impls` crate) asserting `SubscriptionHandle<Tick>: !Clone`.
10. `router_dropped_with_live_handles` (M-10) — create handle, drop router, handle's `recv` yields `Closed`, drop handle, no panic.
11. `concurrent_subscribe_plus_disconnect` (M-8) — fire `subscribe_ticks` and `simulate_connection_lost(1101)` concurrently; assert handle's next recv yields `Err(Closed)` within 500 ms and no upstream leak.
12. `publisher_hot_path_never_touches_dashmap` (BR-18) — instrument the DashMap with a read-counter; publish 10k ticks; assert counter == 0.

Performance test (`cargo bench`-compatible):

13. `bench_hot_publish_path` — publish 1M `Arc<Tick>` through the router to 10 consumers per symbol, measure ns/publish. Budget: ≤ 500 ns median. Not a CI gate; manual run.

## Acceptance

- All 8 behavior tests pass.
- `cargo test -p midas-market-data` green.
- `cargo clippy -p midas-market-data -- -D warnings` clean.
- `cargo fmt --all`.
- No regression in root or desktop workspace.

## Risks

- **DashMap write-lock churn on subscribe/unsubscribe** is acceptable because the hot path does not touch it (BR-18). Control-plane contention is bounded by subscription rate.
- **mailbox_processor move** — handled in S0 prep (BR-16); this slice assumes the move is already done.
- **Drop guard during router shutdown** — if the router's control channel is dropped before handles are dropped, the guard's `send` fails silently. That's fine; the upstream is already being torn down.
- **OPEN**: `router.debug_dump()` (M-28) exact shape / freshness semantics — spec'd but concrete struct fields (`SymbolDebugInfo`) are up to the implementer; must include at least `symbol, tick_refcount, rt_bar_refcount, last_tick_ts, publisher_alive`.
