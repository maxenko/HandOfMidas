# Slice 7 — App-side Consumer Migration

**Goal.** Swap `midas-app` off the central `BrokerEvent::Tick` mediator handler onto per-consumer `SubscriptionHandle`s from the router. Delete the old match arm, `active_market_subs` set, `ensure_market_subscriptions`, and `apply_tick` tick-folding path.

This is the big-bang moment. Tests may temporarily break in the middle of this slice; at slice end everything must be green again.

## Scope

### A. Router instantiation (BR-1 — sync/async reconciliation; NB-4 / NB-5 Option-based placeholder)

iced `Application::new` is **synchronous** and returns `(Self, Task<Message>)`. The old plan sketch mixed `.await?` into `MidasApp::new`, which does not compile. BR-1 fix:

- **Sim** can be constructed synchronously (`MarketDataRouter::new` returns `Arc<Self>` synchronously per S5 §B / NM-4). Build the router directly in `new`.
- **IB** requires `.await` to connect. Store `self.router = None` as the placeholder; kick off `Task::perform(async { IbMarketData::new(cfg).await }, Message::RouterReady)`. On `Message::RouterReady(Ok((router, order)))`, assign `self.router = Some(router)`; iced re-diffs `subscription()` and spins up chart / watchlist / ticker subscriptions against the real router.

**NB-5: there is no `MarketDataRouter::new_disconnected()`.** The placeholder is `None`, full stop.

```rust
impl MidasApp {
    // Synchronous — iced contract.
    fn new(flags: Flags) -> (Self, Task<Message>) {
        let (router, order_client, boot_task) = match flags.broker_backend {
            BrokerBackend::Sim => {
                let source: Arc<dyn MarketDataSource> = Arc::new(SimMarketData::new(SimConfig::default()));
                let order: Arc<dyn OrderClient> = Arc::new(SimOrderClient::new(SimOrderConfig::default()));
                let router = MarketDataRouter::new(source.clone());   // Arc::new_cyclic under the hood (NM-4)
                (Some(router), order, Task::none())
            }
            BrokerBackend::LivePaper | BrokerBackend::Live => {
                // NB-5: placeholder is None; no phantom "disconnected router".
                let placeholder_order: Arc<dyn OrderClient> = Arc::new(DisconnectedOrderClient);
                let connect_cfg = flags.ib_config.clone();
                let task = Task::perform(
                    async move {
                        let source: Arc<dyn MarketDataSource> = Arc::new(IbMarketData::new(connect_cfg).await?);
                        let order: Arc<dyn OrderClient> = Arc::new(IbOrderClient::new(source.shared_client())?);
                        let router = MarketDataRouter::new(source);
                        Ok::<_, Error>((router, order))
                    },
                    Message::RouterReady,
                );
                (None, placeholder_order, task)
            }
        };
        // ...
    }

    // New message handler:
    // Message::RouterReady(Ok((router, order))) => {
    //     self.router = Some(router);
    //     self.order_client = order;
    //     // iced re-diffs subscription() on next update cycle; subscriptions spin up.
    // }
    // Message::RouterReady(Err(e)) => { /* show error, schedule retry */ }
}
```

`MidasApp` gains fields:
```rust
pub struct MidasApp {
    // ...existing fields except active_market_subs, which is deleted...
    pub(crate) router: Option<Arc<MarketDataRouter>>,   // NB-4: None while connecting
    pub(crate) order_client: Arc<dyn OrderClient>,
    // ...
}
```

Every subscription closure early-returns `Subscription::none()` while `router` is `None`:

```rust
let Some(router) = self.router.clone() else { return Subscription::none(); };
```

Delete:
- `pub(crate) active_market_subs: HashSet<SymbolKey>`.
- `pub(crate) broker_bridge: Option<Arc<BrokerBridge>>` (replace with `router` + `order_client` pair).
- Any field holding the old `BrokerHandle`.
- Any reference to `MarketDataRouter::new_disconnected()` (it does not exist — NB-5).

### B. Chart widget subscription

`midas-app/src/app/chart_subscription.rs` (new file).

NB-1 + NB-4 applied: early-return when router is `None`; use `handle.recv()` or `handle.into_stream()` — never read the private `rx`.

```rust
impl MidasApp {
    pub(crate) fn chart_subscriptions(&self) -> Subscription<Message> {
        // NB-4: no router yet → no subscriptions. iced re-diffs when Some(router).
        let Some(router) = self.router.clone() else { return Subscription::none(); };
        let mut out = vec![];
        for (chart_id, chart) in &self.charts {
            let Some(sym) = chart.bound_symbol.clone() else { continue; };
            let tf = chart.timeframe;
            let router = router.clone();
            let chart_id = *chart_id;
            out.push(iced::subscription::channel(
                ("chart-bars", chart_id, sym.clone(), tf),
                64,
                move |mut out| async move {
                    // NB-1: recv() via &mut self — handle owns rx + guard; dropping
                    // `handle` at end-of-closure drops the guard and DecRefs upstream.
                    let mut handle = match router.subscribe_bars(sym.clone(), tf).await {
                        Ok(h) => h,
                        Err(_) => return,
                    };
                    let mut pending: Vec<Bar> = Vec::with_capacity(8);
                    let mut interval = tokio::time::interval(Duration::from_millis(16));
                    loop {
                        tokio::select! {
                            r = handle.recv() => match r {
                                Ok(arc_bar) => pending.push((*arc_bar).clone()),
                                Err(RecvError::Lagged(_)) => {
                                    let _ = out.send(Message::ChartResync { chart_id }).await;
                                }
                                Err(RecvError::Closed) => break,
                            },
                            _ = interval.tick() => {
                                if !pending.is_empty() {
                                    let _ = out.send(Message::ChartBarBatch {
                                        chart_id,
                                        bars: std::mem::take(&mut pending),
                                    }).await;
                                }
                            }
                        }
                    }
                }
            ));
        }
        Subscription::batch(out)
    }
}
```

Same pattern for `floating_charts`.

New `Message` variants:
```rust
Message::ChartBarBatch { chart_id: ChartId, bars: Vec<Bar> }
Message::ChartResync { chart_id: ChartId }
Message::ChartResyncLoaded(Result<(ChartId, Vec<Bar>), MarketDataError>)   // NM-6
```

Handler for `ChartBarBatch`: iterate bars, for each update `chart.data` via `Arc::make_mut` + `apply_bar(bar)`. (Rename `apply_tick` to `apply_bar` on `CandleBuffer`; ticks no longer reach the chart directly.)

**NM-6: `ChartResync` is a two-step Task::perform pattern.** `history_then_live` is async; iced update handlers are sync. The handler kicks off the async load and receives the result via a follow-up message that actually mutates `chart.data`:

```rust
Message::ChartResync { chart_id } => {
    // M-29: throttle at most one resync per chart per 5 s.
    if !self.resync_throttle.allow(chart_id) { return Task::none(); }
    let Some(router) = self.router.clone() else { return Task::none(); };
    let Some(sym) = self.charts.get(&chart_id).and_then(|c| c.bound_symbol.clone()) else {
        return Task::none();
    };
    let tf = self.charts[&chart_id].timeframe;
    Task::perform(
        async move {
            let stream = router
                .history_then_live(sym.clone(), tf, Duration::from_secs(24 * 3600))
                .await?;
            // Collect the initial history batch (and a few live bars if the stream
            // emits them by the time collect() runs). The live tail continues via
            // the chart's normal `chart_subscriptions()` path; this resync only
            // rebuilds the historical prefix.
            let bars: Vec<Bar> = stream.take(1000).collect().await;
            Ok::<_, MarketDataError>((chart_id, bars))
        },
        Message::ChartResyncLoaded,
    )
}
Message::ChartResyncLoaded(Ok((chart_id, bars))) => {
    if let Some(chart) = self.charts.get_mut(&chart_id) {
        let mut buffer = CandleBuffer::with_capacity(bars.len());
        for bar in bars { buffer.apply_bar(&bar); }
        chart.data = Some(Arc::new(buffer));
    }
    Task::none()
}
Message::ChartResyncLoaded(Err(e)) => {
    tracing::warn!(?e, "chart resync failed");
    Task::none()
}
```

**M-29: throttle — at most one `ChartResync` per chart per 5 s.** A `HashMap<ChartId, Instant>` on `MidasApp` (wrapped as `resync_throttle.allow(chart_id)`) records the last resync time; if an incoming `ChartResync` arrives within 5 s of the last, it is dropped. Prevents resync storms from a flaky consumer DoS'ing IB pacing.

### C. Watchlist subscription

`midas-app/src/app/watchlist_subscription.rs` (new file).

```rust
impl MidasApp {
    pub(crate) fn watchlist_subscription(&self) -> Subscription<Message> {
        // NB-4: no router yet → no subscriptions.
        let Some(router) = self.router.clone() else { return Subscription::none(); };
        // M-7: sort the symbol set from day one so the subscription key is stable across re-renders.
        let mut symbols: Vec<SymbolKey> = self.watchlists
            .values()
            .flat_map(|wl| wl.tickers.iter().map(|t| SymbolKey::new(&t.symbol)))
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        symbols.sort();
        let key = ("watchlist-quotes", symbols.clone());
        iced::subscription::channel(key, 256, move |mut out| async move {
            let mut handles: Vec<_> = vec![];
            for sym in &symbols {
                let rx = router.last_quote(sym.clone()).await;
                handles.push((sym.clone(), rx));
            }
            let mut interval = tokio::time::interval(Duration::from_millis(50));
            loop {
                interval.tick().await;
                let mut batch: Vec<(SymbolKey, Quote)> = Vec::new();
                for (sym, rx) in &mut handles {
                    if rx.has_changed().unwrap_or(false) {
                        let q = rx.borrow_and_update().clone();
                        batch.push((sym.clone(), q));
                    }
                }
                if !batch.is_empty() {
                    let _ = out.send(Message::QuoteBatch(batch)).await;
                }
            }
        })
    }
}
```

Handler for `Message::QuoteBatch(batch)`: update `market_cache` entries for each `(symbol, quote)`.

### D. TickerState subscription

`midas-app/src/app/ticker_subscription.rs`.

```rust
impl MidasApp {
    pub(crate) fn ticker_subscription(&self) -> Subscription<Message> {
        // NB-4: no router yet → no subscriptions.
        let Some(router) = self.router.clone() else { return Subscription::none(); };
        let mut out = vec![];
        let active_symbols: HashSet<SymbolKey> = self.tickers.keys().cloned().collect();
        for sym in active_symbols {
            let router = router.clone();
            let sym2 = sym.clone();
            out.push(iced::subscription::channel(
                ("ticker-ticks", sym.clone()),
                128,
                move |mut out| async move {
                    // NB-1: handle owns rx + guard; recv() via &mut self.
                    let mut handle = router.subscribe_ticks(sym2.clone()).await;
                    let mut interval = tokio::time::interval(Duration::from_millis(33));
                    let mut last_price: Option<f64> = None;
                    loop {
                        tokio::select! {
                            r = handle.recv() => match r {
                                Ok(arc_tick) => {
                                    if let (TickType::Last, TickValue::Price(p)) = (&arc_tick.tick_type, &arc_tick.value) {
                                        last_price = Some(*p);
                                    }
                                }
                                Err(Lagged(_)) => continue,
                                Err(Closed) => break,
                            },
                            _ = interval.tick() => {
                                if let Some(p) = last_price.take() {
                                    let _ = out.send(Message::TickerLastPrice {
                                        symbol: sym2.clone(),
                                        last_price: p,
                                    }).await;
                                }
                            }
                        }
                    }
                }
            ));
        }
        Subscription::batch(out)
    }
}
```

Handler for `Message::TickerLastPrice`: dispatch `TickerMsg::UpdateMarketData` to the ticker's state machine.

### E. Farm status and connection subscription

Keep the existing `BrokerConnectionChanged` message path for status-bar display. Source it from `router.connection_state()` instead of the old broker-bridge watch.

Add a new `Message::FarmStatusChanged(FarmStatus)` wired to `router.farm_status()`. Used for logging and future status-bar granularity.

### F. Delete the old code

- `app/handlers.rs`: delete the `BrokerEvent::Tick` match arm entirely (~lines 3712-3805 per recent diff).
- `app/handlers.rs`: delete `ensure_market_subscriptions` method.
- `app/handlers.rs`: delete `drop_market_subscription` method.
- `app/ticker_wiring.rs::bind_chart_to_symbol`: delete the `self.ensure_market_subscriptions()` call. Chart subscription is driven by `chart_subscriptions()` inside iced subscription, which re-runs whenever `self.charts` keys change.
- `app.rs`: delete the `initial_conn_task` 250ms synthetic Ready hack. The router's connection watch handles state dispatch natively.
- `app/fixture.rs`: delete fixture's `ensure_market_subscriptions()` call.
- `broker_bridge.rs`: deprecate or delete `broker_event_stream` for Tick events; keep order-event path until S9.

### F.5. Positions + account panel migration (BR-14)

`account_panel/subscription.rs` and `main.rs` currently subscribe to `BrokerEventSource`; `positions_subscription` is a dedicated iced subscription. Plan deletes `BrokerBridge` in S9 but until now did not state how positions flow post-refactor. This sub-section is the explicit migration path.

- Replace `broker_bridge.position_events()` → `order_client.position_events()` broadcast.
- Replace `broker_bridge.account_events()` → `order_client.account_events()` broadcast.
- Rewrite `positions_subscription` as an `iced::subscription` over `order_client.position_events()`.
- Rewrite `account_panel/subscription.rs` as an `iced::subscription` over `order_client.account_events()` (plus the reconnect-recovery path: on `ConnectionState::Ready`, call `order_client.open_orders()` + `order_client.completed_orders()` to rebuild local state — M-21).

Test: open account panel in the sim fixture, place and fill an order via `SimOrderClient`, assert the account panel's positions row updates within 500 ms. Use `#[tokio::test(start_paused = true)]`.

### G. Dev harness migration (BR-15)

`dev_harness/dump.rs` serialises `active_market_subs`, `broker_bridge.is_some()`, `market_cache`. `app_sim_e2e.rs::live_prices_appear_in_watchlist_after_launch` injects `BrokerEvent::Tick` via `broker_inject.rs`. All of those paths are deleted in S7; replace them here.

- **State projection.** Replace the old `active_market_subs` / `broker_bridge` fields in the dump with:
  ```json
  "router_state": {
      "subscribed_symbols": ["AAPL","MSFT"],
      "active_aggregators": [["AAPL","M1"],["MSFT","M5"]],
      "symbol_debug": [ { "symbol":"AAPL", "tick_refcount":2, "rt_bar_refcount":1, "last_tick_ts":..., "publisher_alive":true } ]
  }
  ```
  Source: `router.debug_dump()` (M-28).
- **Inject path.** Replace `broker_inject::inject` with `DevloopCmd::InjectMarketEvent(MarketEvent)`. The command calls `router.source().inject_for_test(event)`. Only the sim source implements `inject_for_test`; IB source returns `Err(MarketDataError::Unsupported)`.
- **Test migration.** Port `app_sim_e2e.rs::live_prices_appear_in_watchlist_after_launch` to use `DevloopCmd::InjectMarketEvent(MarketEvent::Tick(...))`. Assert the watchlist row's last_price updates within 500 ms.

Also update `DumpState` JSON-schema docs under `desktop/win/crates/midas-devloop-proto/doc/` — OPEN if those docs exist; if not, leave a note to write them alongside this slice.

### G. Live candle fold — relocate

The chart's last candle is now reconstructed from the bar stream, not from ticks. `CandleBuffer::apply_tick` is deleted; `apply_bar(bar: &Bar)` is added:

```rust
impl CandleBuffer {
    pub fn apply_bar(&mut self, bar: &Bar) {
        // If the last candle's ts_open matches bar.ts_open → update in place.
        // Else → push new candle.
        // In both cases bump version.
        match self.timestamps.last().copied() {
            Some(ts) if ts == bar.ts_open.timestamp() * 1000 => {
                // Update last.
                *self.opens.last_mut().unwrap() = bar.o as f32;
                *self.highs.last_mut().unwrap() = bar.h as f32;
                *self.lows.last_mut().unwrap() = bar.l as f32;
                *self.closes.last_mut().unwrap() = bar.c as f32;
                *self.volumes.last_mut().unwrap() = bar.volume.min(u32::MAX as u64) as u32;
            }
            _ => {
                self.push(
                    bar.ts_open.timestamp() * 1000,
                    bar.o as f32, bar.h as f32, bar.l as f32, bar.c as f32,
                    bar.volume.min(u32::MAX as u64) as u32,
                );
            }
        }
        self.version.fetch_add(1, Ordering::Relaxed);
    }
}
```

## Tests

### Integration tests (in `midas-app/tests/`)

1. `sim_watchlist_price_moves` — start app with sim, add AAPL to watchlist, wait 2 s, assert watchlist row's last_price has changed at least 3 times.
2. `sim_chart_updates_on_ticks` — open chart on AAPL, wait 2 s, assert the chart's last candle close != the historical-load last candle close.
3. `sim_multiple_charts_share_subscription` (NB-6 Model A) — open two charts on AAPL, one 1m and one 5m. Instrument `SimMarketData::subscribe_realtime_bars` call count. Assert: exactly ONE upstream RT-bar sub (router fans out; both aggregators share the hub's RT-bar broadcast). Tick sub count should be 0 (no direct tick subscriptions from charts).
4. `sim_drop_all_consumers_unsubscribes` — open chart, close chart, wait 100 ms, assert sim's active subscription count is 0.
5. `sim_fixture_loaded_chart_receives_ticks` — load fixture, verify bound chart symbols receive rt-bar-driven bar updates within 2 s.
6. **BR-14 positions flow** — open account panel in sim fixture, place + fill a market order via `SimOrderClient`, assert positions row updates within 500 ms.
7. **BR-15 dev-harness inject** — via TCP harness, send `DevloopCmd::InjectMarketEvent(Tick(..))` for AAPL, assert watchlist row updates within 500 ms. (This replaces the old `BrokerEvent::Tick` injection in `app_sim_e2e.rs`.)
8. **BR-15 router state projection** — via TCP harness, send `DumpState`, assert returned JSON contains `router_state.subscribed_symbols` and `router_state.active_aggregators`.

### Manual test: run the app

`cargo run -p midas-app --features dev_harness` — verify:
- Watchlist prices drift.
- Chart last candle moves.
- Inline / floating charts update.
- No runaway tasks (check tokio-console or `ps`).

## Acceptance

- All existing desktop tests still pass (1440+).
- 5 new integration tests pass.
- `cargo clippy --workspace --all-targets -- -D warnings` (desktop workspace) clean.
- `cargo fmt --all`.
- Manual smoke test: sim prices move, chart moves.

## Risks

- **Biggest slice in the refactor.** High risk of missed call sites, especially in fixture paths, link groups, floating chart rebinding.
- **iced Subscription lifetime** — subscription keys must be stable across `view()` calls or iced will tear down and re-create on every re-render. Use `HashSet<SymbolKey>` as keys, not `Vec`.
- **Startup ordering** — `router.subscribe_bars` needs the router's aggregator registry, which depends on the source. Source must be constructed and `Arc`-wrapped before anything subscribes. Construct synchronously in `MidasApp::new`.
- **Dev harness fixture fidelity** — fixtures that expect a specific number of tick events may break. Update fixture expectations.
