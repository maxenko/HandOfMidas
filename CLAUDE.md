# Hand of Midas

Trading platform for Interactive Brokers. Windows desktop app with GPU-rendered charts and a Rust broker engine.

**Never commit automatically.** Only commit when explicitly asked.

## Project Status

700+ tests passing across two workspaces. Market order brackets, test broker simulation, and chart rendering are implemented. Phase 1 (IB paper trading connection) is next.

## Workspace Structure

```
HandOfMidas/
├── Cargo.toml                     # Root workspace (resolver = "2")
├── crates/
│   ├── midas-core/                # Shared types — zero IB dependency
│   └── midas-broker/              # Trading engine — wraps IB via rust-ibapi
├── desktop/win/                   # Desktop workspace (10 crates)
│   └── crates/
│       ├── midas-core/            # App-specific types, config, broker bridge
│       ├── midas-data/            # SoA candle buffers, binary format, mmap
│       ├── midas-chart/           # Sans-IO chart core (zero GPU deps)
│       ├── midas-render/          # wgpu 27 GPU pipelines
│       ├── midas-feed/            # CSV import, data providers
│       ├── midas-ui/              # iced widget library
│       ├── midas-app/             # Binary entry point
│       ├── midas-store/           # DuckDB persistence
│       ├── midas-indicators/      # Technical analysis (placeholder)
│       └── mailbox_processor/     # Async actor pattern
├── plan/                          # Active plans + plan/archive/
├── research/                      # Research docs + research/archive/
└── README.md
```

## Key Architecture Rules

1. **No ibapi types leak through public API.** UI crate never imports ibapi.
2. **Split channel architecture.** Market data on `broadcast(4096)`, order events on `broadcast(8192)`, connection state on `watch`.
3. **Live-trading guard.** Config refuses port 4001 unless `allow_live = true`.
4. **Two-tier DB writes.** Critical (orders, fills) awaited; non-critical fire-and-forget.
5. **SecurityType enum over strings.** Use `SecurityType::Stock` not `"STK"`.
6. **MarketDataSource trait.** Test data and future IB both implement this — consumer code is identical.

## Build & Test

```bash
cargo test --workspace                              # broker workspace
cd desktop/win && cargo test --workspace            # desktop workspace
cargo build --release
cargo clippy --workspace -- -D warnings
```

## Documentation Map

| Topic | Where to look |
|---|---|
| midas-core API | `crates/midas-core/doc/api.md` |
| midas-broker API | `crates/midas-broker/doc/` (8 files) |
| Broker architecture | `plan/broker/01-architecture.md` |
| Order state machine | `plan/broker/02-order-management.md` (canonical) |
| SQLite schema | `plan/broker/03-data-layer.md` (canonical) |
| Events & commands | `plan/broker/04-market-data-and-events.md` |
| Desktop UI architecture | `desktop/win/plan/archive/initial/00-index.md` |
| IB API reference | `research/provider-ib.md` |
| Grid component design | `desktop/win/plan/grid-component/README.md` |
| Widget system design | `plan/widget-system/00-index.md` |
