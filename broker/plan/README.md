# `midas-broker` — Implementation Plan

> Trading engine crate for Hand of Midas, wrapping Interactive Brokers via `rust-ibapi`.
> March 2026 — Draft v1, subject to refinement.

## Plan Documents

| # | Document | Covers |
|---|---|---|
| 01 | [Architecture](01-architecture.md) | Non-goals, crate structure, async model, split channels, connection lifecycle, two-tier DB writes, config, crate boundaries |
| 02 | [Order Management](02-order-management.md) | **Canonical** OrderState enum + IB mapping, state machine, activate/deactivate, brackets, OCA, modification, reconciliation, tags/groups |
| 03 | [Data Layer](03-data-layer.md) | **Canonical** SQLite schema (6 tables + binary candle file cache), Rust types, rusqlite access pattern, migrations, sync strategy, backup/recovery |
| 04 | [Market Data & Events](04-market-data-and-events.md) | Subscriptions, caching, BrokerEvent/BrokerCommand enums, split channel architecture, iced integration |
| 05 | [Implementation Roadmap](05-implementation-roadmap.md) | 8 phases (week-by-week), risk register, dependencies, testing strategy |

### Authoritative Sources

Types and schemas are defined in multiple documents for context, but **only one is canonical**:

| Concern | Canonical Document | Others reference it |
|---|---|---|
| `OrderState` enum + IB status mapping | 02-order-management.md §1.4-1.5 | 03, 04, 05 |
| SQLite schema (DDL) | 03-data-layer.md §1 | 01, 02, 05 (Appendix A) |
| DB access pattern (rusqlite) | 01-architecture.md §5 + 03-data-layer.md §3 | 02 (Appendix C) |
| Channel architecture | 01-architecture.md §2 + 04-market-data-and-events.md §4 | — |

## Key Decisions

- **Foundation crate:** `rust-ibapi` by wboayue (MIT, 290 stars, actively maintained). Exact version determined in Phase 0.
- **Architecture:** Single process, shared tokio runtime with iced desktop app
- **Persistence:** SQLite via rusqlite (WAL mode). Two-tier write policy: critical writes awaited, non-critical fire-and-forget.
- **Communication:** Split channels — `broadcast` for market data (lossy), `mpsc` for order events (lossless), `watch` for connection state
- **No ibapi types leak** through the public API — UI crate never imports ibapi directly
- **Live-trading guard:** `allow_live = false` by default; engine refuses port 4001 unless explicitly enabled

## Reference Documents

These files in the project root provide context referenced by the plan:

- [`provider-ib.md`](../../provider-ib.md) — Complete IBKR API reference (order management, market data, gotchas)
- [`tech-stack-rust-a.md`](../../tech-stack-rust-a.md) — Charting platform architecture (binary candle format, GPU rendering)
- [`providers.md`](../../providers.md) — Market data provider research

## Workspace Structure

```
crates/
  midas-core/       # Shared types (ContractSpec, SecurityType, SymbolKey, Timeframe, OhlcvBar)
  midas-broker/     # THIS CRATE — order management + IB connection
  midas-feed/       # Market data streaming (future)
  midas-data/       # Historical data storage (future)
  midas-render/     # GPU chart rendering (future)
  midas-indicators/ # Indicator computation (future)
  midas-app/        # iced desktop application (future)
```

## Status

- [x] Research complete (providers.md, provider-ib.md)
- [x] Architecture planned
- [x] Plan evaluation + refinement (cross-document consistency, split channels, two-tier writes)
- [x] Phase 0: Foundation — midas-core types, engine skeleton, BrokerConfig + live-trading guard, SQLite schema (6 tables), OrderStatus state machine (11 states, 32 tests), BrokerEvent/BrokerCommand enums, realistic per-ticker test data provider (regime switching + GARCH + Brownian bridge), 115 unit tests total
- [ ] Evaluate rust-ibapi (connect to paper trading)
- [ ] Phase 1: Order Basics
- [ ] Phase 2: Market Data
- [ ] Phase 3: Account & Positions
- [ ] Phase 4: Advanced Orders (brackets, OCA, Adaptive algo, trailing stops — no conditionals)
- [ ] Phase 5: iced Integration
- [ ] Phase 6: Resilience
- [ ] Phase 7: Polish

### Phase 0 Deliverables (complete)

| Deliverable | Location |
|---|---|
| Core types (ContractSpec, SecurityType, SymbolKey, Timeframe, OhlcvBar) | `midas-core/src/lib.rs` |
| BrokerEngine skeleton with select! loop | `midas-broker/src/engine.rs` |
| BrokerConfig with live-trading guard | `midas-broker/src/config.rs` |
| SQLite schema + migrations | `midas-broker/migrations/001_initial.sql` |
| OrderStatus state machine (11 states) | `midas-broker/src/orders/state.rs` |
| Order types (LocalOrder, OrderAction, OrderKind, TimeInForce) | `midas-broker/src/orders/types.rs` |
| BrokerEvent (22 variants) + BrokerCommand (17 variants) | `midas-broker/src/events.rs`, `commands.rs` |
| ConnectionState (5 states) | `midas-broker/src/connection.rs` |
| BrokerError (thiserror, 9 variants) | `midas-broker/src/error.rs` |
| Persistence layer (order CRUD, audit, fills) | `midas-broker/src/persist/order_repo.rs` |
| TestDataProvider (per-ticker, regime-switching, multi-TF) | `midas-broker/src/testdata/` |
