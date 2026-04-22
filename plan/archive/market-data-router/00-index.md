# Market-Data Router Refactor — Plan Index

## Why

The current streaming pipeline is a mediator-pattern hub: one `broadcast::channel` out of the broker engine, one app-side subscription, one giant `BrokerEvent::Tick` match arm fans out to five hard-coded consumers. It works for a single-UI desktop app but deviates from how TWS, NinjaTrader, TradingView, MultiCharts, and NautilusTrader structure market-data routing. Adding a 6th consumer means editing the central handler. Subscriptions are set-union, not refcounted. Sim and real IB diverge at the wire (sim packs bid/ask/last/volume into one callback; live adapter actually calls `realtime_bars`, not `reqMktData`). Historical and streaming paths don't share an interface.

Target: world-class streaming topology that (1) makes sim and IB indistinguishable at the provider-trait boundary, (2) routes through one per-symbol fan-out hub with refcounted RAII subscriptions, (3) unifies historical+live into a single stream via server-timestamp seam, (4) lazily spawns per-(symbol, timeframe) aggregator actors, (5) delivers frame-rate-coalesced batches to iced consumers.

## Architecture at a glance

```
┌───────────────────────────────────────────────────────────────────┐
│   Provider layer (crates/midas-broker, new sub-crates)            │
│                                                                   │
│   trait MarketDataSource (ticks, realtime bars, historical)       │
│        ↓                                                          │
│   SimMarketData          │          IbMarketData                  │
│   - reqId-keyed subs     │          - wraps rust-ibapi            │
│   - per-tick-type events │          - realtime_bars Subscription  │
│   - farm-state events    │          - historical_stream (tail)    │
│   - historical_bars +    │          - historical_bars (one-shot)  │
│     historical_stream    │                                        │
│                                                                   │
│   trait OrderClient      (orders only, separated)                 │
│        ↓                                                          │
│   SimOrderClient         │          IbOrderClient                 │
└───────────────────────────────────────────────────────────────────┘
                       ↓ single broker engine
┌───────────────────────────────────────────────────────────────────┐
│   Routing layer (crates/midas-market-data)                        │
│                                                                   │
│   MarketDataRouter  (mailbox-processor control plane +            │
│                      DashMap hot publish path)                    │
│                                                                   │
│   For each active symbol:                                         │
│     broadcast::Sender<Arc<Tick>>          — tick fan-out          │
│     broadcast::Sender<Arc<Bar>>           — rt-bar fan-out (NB-6) │
│     watch::Sender<Quote>                   — last-quote (watch)   │
│     tick_refcount / watch_refcount /                              │
│       rt_bar_refcount    (NB-3)                                   │
│     publisher tasks own upstream streams  (NB-7)                  │
│                                                                   │
│   subscribe(symbol) -> SubscriptionHandle (into_stream/recv)       │
│     - first subscriber: upstream.subscribe_ticks(symbol)          │
│     - Drop on _guard: DecRef; actor aborts publisher on last drop  │
│                                                                   │
│   history_then_live(symbol, tf) -> impl Stream<MarketEvent>       │
│     - subscribe_ticks FIRST, buffer live                          │
│     - request_historical_bars with T_server                       │
│     - emit stream::iter(history).chain(tail.filter(ts>T_server))  │
└───────────────────────────────────────────────────────────────────┘
                       ↓
┌───────────────────────────────────────────────────────────────────┐
│   Aggregation layer (crates/midas-market-data::aggregator)        │
│                                                                   │
│   BarAggregatorRegistry  (same refcount pattern per (sym, tf))    │
│     lazy spawn: first subscribe_bars(sym, tf)                     │
│     owns one tick subscription + its own broadcast<Candle>        │
│     maintains current-incomplete-candle in the actor              │
│     emits on bar-close + on current-candle update tick            │
└───────────────────────────────────────────────────────────────────┘
                       ↓
┌───────────────────────────────────────────────────────────────────┐
│   Consumer layer (desktop/win/crates/midas-app)                   │
│                                                                   │
│   Chart widget:                                                   │
│     iced::Subscription::channel per visible chart                 │
│     owns a SubscriptionHandle to (sym, tf) aggregator             │
│     frame-rate coalesce → Message::ChartBarBatch                  │
│                                                                   │
│   Watchlist row:                                                  │
│     watch::Receiver<Quote> per symbol                             │
│     Subscription::channel batched 16ms → Message::QuoteBatch      │
│                                                                   │
│   TickerState:                                                    │
│     Separate Subscription::channel subscribing to the tick        │
│     stream for the active chart's symbol                          │
└───────────────────────────────────────────────────────────────────┘
```

No central `BrokerEvent::Tick` match arm. No `active_market_subs: HashSet`. No `Arc::make_mut` on a chart buffer inside a handler. Each consumer holds its own handle; lifetime = handle lifetime.

## Slices

| # | File | Status | Depends on | Parallelizable with |
|---|------|--------|------------|---------------------|
| S0 | [01a-slice-0-prep.md](01a-slice-0-prep.md) — prep: move `mailbox_processor` up, pin `ibapi`, iced 0.14 POC | pending | — | — |
| S1 | [02-slice-1-core-types.md](02-slice-1-core-types.md) — shared market-data types | pending | S0 | — |
| S2 | [03-slice-2-provider-traits.md](03-slice-2-provider-traits.md) — new `MarketDataSource` + `OrderClient` traits | pending | S1 | — |
| S3 | [04-slice-3-sim-backend.md](04-slice-3-sim-backend.md) — reqId-keyed sim with IB fidelity | pending | S2 | S4 |
| S4 | [05-slice-4-ib-backend.md](05-slice-4-ib-backend.md) — IB adapter against new traits | pending | S2 | S3 |
| S5 | [06-slice-5-router.md](06-slice-5-router.md) — `MarketDataRouter` + RAII handles + history seam | pending | S2, S3 (tests) | — |
| S6 | [07-slice-6-aggregator.md](07-slice-6-aggregator.md) — bar aggregator registry | pending | S5 | — |
| S7 | [08-slice-7-app-consumers.md](08-slice-7-app-consumers.md) — chart / watchlist / ticker-state migration + deletion of old central handler | pending | S5, S6 | — |
| S8 | [09-slice-8-iced-subscriptions.md](09-slice-8-iced-subscriptions.md) — per-chart `Subscription::channel` with frame coalescing | pending | S7 | — |
| S9 | [10-slice-9-cleanup.md](10-slice-9-cleanup.md) — delete dead paths, update CLAUDE.md docs | pending | S8 | — |

Parallelizable pairs: **S1 and S2 are NOT parallelizable** (S2 depends on S1's types). (S3 || S4) after S2 lands. Implementation of S8 can start in parallel with S7 once S7's public API is drafted (M-37).

Revised ordering: **S0 (prep) → S1 (core types) → S2 (traits) → S3 || S4 → S5 → S6 → S7a–e → S8 → S9**. S3 alone unblocks S5 (router tests use sim); S4 is needed for S9.

## Cross-cutting documents

- [01-architecture.md](01-architecture.md) — detailed design decisions and rationale
- [90-migration.md](90-migration.md) — how to execute the big-bang swap (S7) without leaving a broken intermediate state
- [91-testing.md](91-testing.md) — test matrix, sim-vs-IB behavioral equivalence tests, performance budgets

## Non-goals

- Replacing `iced` or `wgpu`.
- Replacing `rust-ibapi`.
- Replacing the order-entry / bracket state machine (lives in TickerState, orthogonal).
- Persisting ticks to the DuckDB store. The router passes through; persistence is a separate consumer pattern that can be added later.
- Multi-account streaming (single account for now; architecture supports expansion).

## Invariants preserved through the refactor

1. No `ibapi` types leak past the `midas-broker` crate boundary.
2. Split-channel architecture (market vs orders vs connection) survives; routing is layered on top, not replacing it.
3. `SecurityType` enum over strings.
4. Two-tier DB writes (critical awaited, non-critical fire-and-forget).
5. Live-trading guard (port 4001 refused unless `allow_live = true`).
6. Dependency flow stays strictly downward: midas-core → midas-broker → midas-market-data → app.

## Revision log

- **Round 1 applied** (see git history). Folded 24 blockers + 37 majors into the slice files; added S0 prep slice; reshaped `ReqId` to `i32`, split historical API, rewrote `SubscriptionHandle`/guard lifetime model, added pacing governor, positions + dev-harness migration sub-sections, and fidelity fixtures.
- **Round 2 applied** — integration-layer fixes (SubscriptionHandle API, router ownership, watch refcount, Model A RT-bar fan-out, Arc::new_cyclic, contract cache).
