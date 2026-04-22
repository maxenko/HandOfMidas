# Slice 6 — Bar Aggregator Registry

**Goal.** Add `BarAggregatorRegistry` inside `midas-market-data` that lazily spawns per-`(symbol, timeframe)` aggregator tasks, each consuming a tick `SubscriptionHandle` from the router and producing a `broadcast::Sender<Arc<Bar>>` for consumers.

## Scope

`crates/midas-market-data/src/aggregator/mod.rs`, `aggregator/task.rs`, `aggregator/registry.rs`.

### A. Registry (BR-5 — fully async, no `block_on`)

No mailbox-processor actor. The registry uses a plain async method protected by a `tokio::sync::Mutex<HashMap<Key, Arc<AggregatorEntry>>>`. Serial init is per-key (actually global across the registry, which is fine — aggregator init is cold and rare). For first-request-per-key idempotency, wrap entries in a `tokio::sync::OnceCell` stored inside the map.

```rust
pub struct BarAggregatorRegistry {
    aggregators: tokio::sync::Mutex<HashMap<(SymbolKey, Timeframe), Arc<AggregatorEntry>>>,
    router: Weak<MarketDataRouter>,  // BR-6: no unwrap; upgrade-or-early-return
    weak_self: Weak<BarAggregatorRegistry>,
}

struct AggregatorEntry {
    bars_tx: broadcast::Sender<Arc<Bar>>,   // cap 256
    refcount: AtomicU32,
    last_bar: tokio::sync::RwLock<Option<Bar>>,   // M-28 / snapshot resync
    _task: JoinHandle<()>,
}
```

No `AggMsg` / actor; DecRef is a direct async `remove_if_zero` call from the guard's Drop. For Drop-safety in a sync context, the guard fires a fire-and-forget `tokio::spawn` of the async removal (acceptable — removal is always OK to delay).

### B. Subscribe flow (BR-5, BR-6, BR-12, BR-22, NB-6 Model A)

The aggregator subscribes to RT bars **through the router** (`router.subscribe_rt_bars`), not directly against the source. Model A fan-out (NB-6): one upstream IB RT-bar request per symbol, shared by every aggregator on that symbol regardless of timeframe. The router's per-symbol hub owns the broadcast and publisher; the aggregator only holds a `SubscriptionHandle<Bar>` and folds its stream into the target timeframe.

```rust
pub async fn subscribe(
    &self,
    symbol: SymbolKey,
    tf: Timeframe,
) -> Result<SubscriptionHandle<Bar>, MarketDataError> {
    // BR-22: reject timeframes that can't be derived from 5s RT bars with UTC alignment.
    if matches!(tf, Timeframe::D1 | Timeframe::W1 | Timeframe::M1Monthly | Timeframe::H4) {
        return Err(MarketDataError::UnsupportedTimeframe(tf));
    }

    // BR-6: Weak upgrade; no unwrap.
    let router = self.router.upgrade().ok_or(MarketDataError::ShuttingDown)?;
    let key = (symbol.clone(), tf);

    let mut map = self.aggregators.lock().await;
    let entry = if let Some(e) = map.get(&key) {
        e.clone()
    } else {
        // NB-6 Model A: aggregator goes through the router's refcounted
        // hub so two aggregators on the same symbol share ONE upstream
        // IB subscription. BR-12: source is RT-bars, not raw ticks.
        let rt_handle = router
            .subscribe_rt_bars(symbol.clone())
            .await?;
        let (bars_tx, _) = broadcast::channel(256);
        let entry = Arc::new(AggregatorEntry {
            bars_tx: bars_tx.clone(),
            refcount: AtomicU32::new(0),
            last_bar: tokio::sync::RwLock::new(None),
            _task: tokio::spawn(run_aggregator(rt_handle, tf, bars_tx)),
        });
        map.insert(key.clone(), entry.clone());
        entry
    };
    entry.refcount.fetch_add(1, Ordering::Relaxed);
    let rx = entry.bars_tx.subscribe();
    drop(map);

    Ok(SubscriptionHandle {
        rx,
        _guard: Box::new(BarSubGuard {
            key,
            registry: self.weak_self.clone(),
        }),
    })
}

// Guard drop: fire-and-forget async removal.
impl Drop for BarSubGuard {
    fn drop(&mut self) {
        let key = self.key.clone();
        let registry = self.registry.clone();
        tokio::spawn(async move {
            if let Some(reg) = registry.upgrade() {
                let mut map = reg.aggregators.lock().await;
                if let Some(entry) = map.get(&key) {
                    let prev = entry.refcount.fetch_sub(1, Ordering::Relaxed);
                    if prev == 1 {
                        map.remove(&key);  // drops entry → aborts task → cancels RT-bar sub
                    }
                }
            }
        });
    }
}
```

### C. Aggregator task (BR-12, M-5, M-6, M-11, M-26, M-36, NB-6)

Source is `SubscriptionHandle<Bar>` from `router.subscribe_rt_bars` (5 s Trades, router-fanned-out per NB-6 Model A). The aggregator folds 5 s windows into the target timeframe. No tick-accumulation (BR-12). Partial emits are coalesced at 100 ms (M-26); completed bars emit immediately.

```rust
async fn run_aggregator(
    mut rt_handle: SubscriptionHandle<Bar>,   // NB-6: from router, not source
    tf: Timeframe,
    bars_tx: broadcast::Sender<Arc<Bar>>,
) {
    let mut current: Option<Bar> = None;
    let mut coalesce = tokio::time::interval(Duration::from_millis(100));   // M-26
    coalesce.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut dirty = false;
    let mut zero_receivers_streak: u32 = 0;    // M-4

    loop {
        tokio::select! {
            r = rt_handle.recv() => {
                // M-5: clean match, no double-unwrap.
                let rt_bar_arc = match r {
                    Ok(b) => b,
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(lag = n, "aggregator lagged; invalidating current bar");
                        current = None;   // M-11: Lagged drops the current bar; next tick in next window opens fresh.
                        dirty = false;
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => return,
                };
                let rt_bar = &*rt_bar_arc;

                let window_open = align_to_window(rt_bar.ts_open, tf)?;   // M-6: guard inside
                match current.as_mut() {
                    Some(bar) if bar.ts_open == window_open => {
                        bar.c = rt_bar.c;
                        bar.h = bar.h.max(rt_bar.h);
                        bar.l = bar.l.min(rt_bar.l);
                        bar.volume = bar.volume.saturating_add(rt_bar.volume);
                        bar.completeness = BarCompleteness::Partial;   // M-36: no ticks_folded
                        dirty = true;
                    }
                    _ => {
                        // Close out previous bar.
                        if let Some(mut prev) = current.take() {
                            prev.completeness = BarCompleteness::Completed;
                            let _ = bars_tx.send(Arc::new(prev));
                        }
                        current = Some(Bar {
                            symbol: rt_bar.symbol.clone(),
                            timeframe: tf,
                            ts_open: window_open,
                            ts_close: window_open + tf.to_duration(),
                            o: rt_bar.o, h: rt_bar.h, l: rt_bar.l, c: rt_bar.c,
                            volume: rt_bar.volume,
                            trade_count: rt_bar.trade_count,
                            wap: rt_bar.wap,
                            completeness: BarCompleteness::Partial,
                        });
                        dirty = true;
                    }
                }
            }
            _ = coalesce.tick(), if dirty => {
                if let Some(bar) = &current {
                    let _ = bars_tx.send(Arc::new(bar.clone()));
                }
                dirty = false;
                // M-4 auto-exit on idle.
                if bars_tx.receiver_count() == 0 {
                    zero_receivers_streak = zero_receivers_streak.saturating_add(1);
                    if zero_receivers_streak >= 16 { return; }
                } else {
                    zero_receivers_streak = 0;
                }
            }
        }
    }
}

// M-6: reject zero-duration timeframes; return an Err the caller can convert into a bar-stream Error.
fn align_to_window(ts: DateTime<Utc>, tf: Timeframe) -> Result<DateTime<Utc>, MarketDataError> {
    let secs = tf.to_duration().as_secs() as i64;
    if secs <= 0 {
        return Err(MarketDataError::UnsupportedTimeframe(tf));
    }
    let epoch = ts.timestamp();
    Ok(DateTime::from_timestamp(epoch - (epoch % secs), 0).unwrap())
}
```

### D. Snapshot access

Consumers who get `Lagged` need a way to resync. Expose:

```rust
impl BarAggregatorRegistry {
    pub async fn last_bar(&self, symbol: &SymbolKey, tf: Timeframe) -> Option<Bar> {
        let map = self.aggregators.lock().await;
        if let Some(entry) = map.get(&(symbol.clone(), tf)) {
            return entry.last_bar.read().await.clone();
        }
        None
    }
}
```

Implementation: the aggregator task updates `entry.last_bar` (RwLock) on every publish. `last_bar` reads through the RwLock directly — no actor hop.

## Tests

`crates/midas-market-data/tests/aggregator_behavior.rs`. All timing-sensitive tests use `#[tokio::test(start_paused = true)]` (BR-20).

1. `aggregates_rt_bars_into_single_bar` — feed 12 rt-bars (60 s of 5 s bars) to `(AAPL, M1)` aggregator. Receive one Completed at t0+60s plus coalesced Partial emits at 100 ms cadence.
2. `closes_bar_on_window_rollover` — feed rt-bars at t0 and t0+61s. Observe Completed(t0) then Partial(t0+60s).
3. `multiple_consumers_share_aggregator` — subscribe twice to `(AAPL, M1)`. Feed 100 rt-bars. Assert: both consumers see the same bar sequence AND the sim received exactly one `subscribe_realtime_bars` call.
4. `different_timeframes_share_rt_sub` (NB-6 Model A) — subscribe to `(AAPL, M1)` and `(AAPL, M5)`. Instrument the sim's `subscribe_realtime_bars` call counter. Assert: exactly ONE upstream call (router fans out via `subscribe_rt_bars`; the per-symbol hub's RT-bar broadcast feeds both aggregators). Each aggregator still runs its own folding task with an independent `SubscriptionHandle<Bar>`.
5. `last_drop_aborts_task` — subscribe, drop, advance 200 ms, subscribe again. Should spin up a fresh aggregator task.
6. `last_bar_returns_current_partial` — mid-window, call `last_bar`, receive the current partial bar with up-to-date o/h/l/c.
7. `lagged_invalidates_current_bar` (M-11) — force a Lagged at mid-window; next rt-bar opens a fresh bar; prior partial is dropped and NOT emitted as Completed.
8. `unsupported_timeframe_rejected` (BR-22) — `subscribe(AAPL, Timeframe::D1)` returns `Err(UnsupportedTimeframe)`. Same for `W1`, `H4`, monthly.
9. `partial_emits_are_coalesced` (M-26) — feed 5 rt-bars within the same window in 20 ms; consumer receives exactly one coalesced partial emit within 100 ms, not five per-bar emits.

## Acceptance

- All 6 tests pass.
- `cargo test -p midas-market-data` green.
- `cargo clippy -p midas-market-data -- -D warnings`.
- Hot publish path still meets budget (re-run bench from S5).

## Risks

- **Weak<MarketDataRouter> cycle** (BR-6) — router owns registry; registry references router via `Weak`. Upgrade-or-early-return; never `.unwrap()`. Initialize router with `Arc::new_cyclic` so the weak ref is valid at construction time.
- **BR-22 scope line**: D1, W1, monthly, H4, and any RTH-aligned timeframe are explicitly out of scope. D1 comes from `historical_bars` (server-computed). Re-opening this scope requires a calendar dependency.
- **Lagged on upstream realtime bars** (M-11): invalidating `current` means the prior bar's OHLC is lost. That's acceptable — Lagged already implies dropped data — but document in the aggregator task doc-comment.
