# Hand of Midas

Trading platform for Interactive Brokers. Windows desktop app with GPU-rendered charts and a Rust broker engine.

## Project Status

**Phase 0 (Foundation) is complete.** 128 unit tests passing. Phase 1 (IB API integration) is next.

No git history yet — all code is uncommitted on master.

## Workspace Structure

```
HandOfMidas/
├── Cargo.toml                     # Workspace root (resolver = "2")
├── crates/
│   ├── midas-core/                # Shared types — zero IB dependency
│   │   ├── doc/                   # API documentation
│   │   └── src/lib.rs
│   └── midas-broker/              # Trading engine — wraps IB via rust-ibapi
│       ├── doc/                   # API documentation (multiple files)
│       ├── migrations/
│       └── src/
├── broker/plan/                   # Architecture docs (5 documents, ~7k lines)
├── desktop/win/plan/              # UI architecture docs (8 documents, ~18k lines)
└── *.md                           # Research docs (providers, tech stacks)
```

Future crates (not yet created): `midas-feed`, `midas-data`, `midas-render`, `midas-chart`, `midas-indicators`, `midas-app`.

## Key Architecture Rules

1. **No ibapi types leak through public API.** UI crate never imports ibapi.
2. **Split channel architecture.** Market data on `broadcast(4096)`, order events on `broadcast(8192)`, connection state on `watch`.
3. **Live-trading guard.** Config refuses port 4001 unless `allow_live = true`.
4. **Two-tier DB writes.** Critical (orders, fills) awaited; non-critical fire-and-forget.
5. **SecurityType enum over strings.** Use `SecurityType::Stock` not `"STK"`.
6. **MarketDataSource trait.** Test data and future IB both implement this — consumer code is identical.

## Build & Test

```bash
cargo test              # 128 tests across both crates
cargo build --release
cargo clippy
```

## Documentation Map

| Topic | Where to look |
|---|---|
| midas-core API | `crates/midas-core/doc/api.md` |
| midas-broker API | `crates/midas-broker/doc/` (multiple files) |
| Architecture overview | `broker/plan/01-architecture.md` |
| Order state machine | `broker/plan/02-order-management.md` (canonical) |
| SQLite schema | `broker/plan/03-data-layer.md` (canonical) |
| Events & commands | `broker/plan/04-market-data-and-events.md` |
| Implementation roadmap | `broker/plan/05-implementation-roadmap.md` |
| Desktop UI architecture | `desktop/win/plan/initial/00-index.md` |
| IB API reference | `provider-ib.md` |
