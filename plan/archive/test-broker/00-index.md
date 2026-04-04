# Test Broker — Implementation Plan

> A full-fidelity test broker that simulates Interactive Brokers' order execution,
> market data, and account management without a live IB connection. Enables
> end-to-end testing of the entire trading pipeline.
>
> Status: PLANNING
> Date: 2026-04-02
> Documents: 4
>
> **Reference**: rust-ibapi v2.10 API surface and IB TWS/Gateway behavior.
> The test broker mirrors IB's async stream model, order state machine,
> bracket mechanics, and market data subscription lifecycle.
>
> **Scope**: Replace the current `TestBrokerClient` (accept-only stub) with
> a simulation engine that produces realistic status callbacks, fills,
> market data ticks, and account updates.

---

## Documents

| # | Document | Description |
|---|----------|-------------|
| 01 | [Architecture](01-architecture.md) | Core design, trait hierarchy, simulation engine, channel model |
| 02 | [Order Simulation](02-order-simulation.md) | Order lifecycle, fill engine, bracket mechanics, edge cases |
| 03 | [Market Data & Account](03-market-data-account.md) | Tick generation, bar streaming, positions, account values |
| 04 | [Implementation Roadmap](04-implementation-roadmap.md) | Phased delivery, test matrix, acceptance criteria |

---

## Why a Test Broker?

The current `TestBrokerClient` accepts all orders and records them but:
- Never fires fill callbacks (orders stay in PendingSubmit forever)
- No bracket lifecycle simulation (parent fill → child activation → OCA)
- No market data generation (ticks, streaming bars)
- No account/position updates on fills
- No connection state simulation (connect/disconnect/reconnect)
- No error injection (rejections, partial fills, network failures)

Every upcoming feature depends on realistic broker simulation:

| Feature | Needs From Test Broker |
|---|---|
| Market order brackets | Parent fill → child activation → TP/SL fills |
| Limit order brackets | Price-triggered fills, partial fills |
| Position tracking | Fill → position update → P&L calculation |
| Chart bracket visualization | Status callbacks driving annotation updates |
| Order panel feedback | Submission → confirmation → fill toast |
| Reconnection handling | Disconnect → reconcile → resume |
| Strategy backtesting | Deterministic fill simulation at historical prices |

---

## Design Decisions

| Decision | Choice | Rationale |
|---|---|---|
| Simulation model | Event-driven with configurable timing | Instant fills for unit tests, delayed for UI testing |
| Fill price model | Last price ± configurable slippage | Realistic without requiring a full order book |
| Market data source | Compose with existing `TestDataProvider` | Reuse 10 years of deterministic OHLCV data |
| Trait boundary | Extend existing `BrokerClient` trait | Backward compatible, engine doesn't change |
| Bracket OCA | Engine-side implicit (parent-child link) | Matches IB's native bracket behavior |
| Threading model | Single async task with timer-driven callbacks | Matches engine's `tokio::select!` loop |
| Determinism | Seeded RNG per symbol for fill prices | Reproducible test runs |

---

## Non-Goals

| Non-Goal | Why |
|---|---|
| Full order book simulation | Unnecessary for bracket testing; adds massive complexity |
| Multi-account support | Single account sufficient for v1 |
| Options/futures Greeks | Stock-only for now; futures straightforward to add later |
| Real-time P&L streaming | Tick-level P&L deferred until `midas-feed` streaming exists |
| FIX protocol simulation | We only need the rust-ibapi trait surface, not wire protocol |
