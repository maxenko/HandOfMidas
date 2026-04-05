# Hand of Midas

A native Windows desktop trading platform for Interactive Brokers. GPU-rendered charts, a Rust broker engine, and a sans-IO architecture that runs 20+ simultaneous charts at 60fps.

Built entirely in Rust. No Electron. No browser. No GC pauses.

---

## Why This Exists

Retail trading platforms are either web apps with input lag, or legacy C++ monsters with plugin APIs from 2005. Hand of Midas is a ground-up rewrite of what a modern trading desktop should be: GPU-accelerated charting with sub-frame latency, a type-safe broker engine that won't let you fat-finger a live order, and a codebase that compiles in under 30 seconds.

---

## Architecture

```
                          iced 0.14 (multi-window daemon)
                                    |
                    +---------------+---------------+
                    |                               |
               midas-app                      midas-app
            (main window)                  (floating charts)
                    |                               |
          +---------+---------+                     |
          |         |         |                     |
     midas-ui  midas-chart  midas-render      midas-render
     (widgets) (sans-IO)    (wgpu 27)         (wgpu 27)
                    |
               midas-data
            (SoA, mmap, LOD)
                    |
               midas-feed ------> midas-broker
            (CSV, providers)      (IB engine)
                                       |
                                   ibapi 2.10
                                  (paper/live)
```

**Key design decisions:**

- **Sans-IO chart core** — `midas-chart` has zero GPU or framework dependencies. All chart logic (state, zoom/pan, interactions, auto-scale) lives here as a pure state machine. Produces a `ChartScene` that the GPU renderer consumes. Fully testable without a window.

- **Split channel model** — Market data on `broadcast(4096)`, order events on `broadcast(8192)`, connection state on `watch`. Consumer code is identical for test data and live IB feeds.

- **No ibapi leakage** — The broker crate wraps Interactive Brokers behind `BrokerClient` and `MarketDataSource` traits. The UI never imports `ibapi`. Ever.

- **Live-trading guard** — Config refuses IB gateway port 4001 unless `allow_live = true`. You have to opt in to real money.

---

## Tech Stack

| Layer | Technology | Version |
|-------|-----------|---------|
| Language | Rust (stable, 2021 edition) | pinned via `rust-toolchain.toml` |
| GPU | wgpu | 27 |
| GUI | iced (multi-window daemon) | 0.14 |
| Async | tokio | 1 |
| Broker | ibapi (rust-ibapi) | 2.10 |
| Order DB | rusqlite (WAL mode) | 0.32 |
| Analytics DB | DuckDB | 1.0 |
| Candle storage | Custom binary + memmap2 | SoA layout |
| Math | glam (SIMD) | 0.29 |
| Shaders | WGSL | — |

---

## Workspace Structure

Two Cargo workspaces. Dependency flows strictly downward.

```
HandOfMidas/
├── crates/                         # Broker workspace
│   ├── midas-core/                 # Shared types (SecurityType, ContractSpec)
│   └── midas-broker/               # Trading engine, order state machine, SQLite
│
├── desktop/win/crates/             # Desktop workspace
│   ├── midas-core/                 # App types, config schema, broker bridge
│   ├── midas-data/                 # SoA candle buffers, binary format, mmap
│   ├── midas-chart/                # Sans-IO chart core (zero GPU deps)
│   ├── midas-render/               # wgpu GPU pipelines, WGSL shaders
│   ├── midas-feed/                 # CSV import, data provider trait
│   ├── midas-ui/                   # iced widget library
│   ├── midas-app/                  # Binary entry point, application shell
│   ├── midas-store/                # DuckDB persistence layer
│   └── mailbox_processor/          # Async actor pattern
│
├── plan/                           # Active plans + plan/archive/
│   ├── broker/                     # Broker architecture docs (6 files)
│   ├── widget-system/              # Chart widget system design
│   ├── TODO.md                     # Current state and next steps
│   └── archive/                    # Implemented plans
├── desktop/win/plan/               # UI-specific plans + archive/
│   └── grid-component/             # Trading-grade grid widget design
└── research/                       # Technology research + archive/
```

---

## Features

### Implemented

**Charting**
- GPU-rendered candlestick charts via custom wgpu pipelines
- 20+ simultaneous charts at 60fps
- Zoom, pan, auto-scale with momentum animation
- Grid lines, date/price labels, volume profile
- Horizontal level tool with per-symbol persistence
- Crosshair with coordinate labels
- Multi-window support (pop-out charts)
- Symbol linking across charts

**Broker Engine**
- Order status state machine (11 states, validated transitions)
- Market order brackets (entry + TP + SL) with OCA cancellation
- Full-simulation test broker (fills, stops, partial fills, positions)
- SQLite persistence with audit trail and idempotent fill insertion
- `BrokerClient` trait — test and IB implementations share the same interface

**Desktop App**
- Multi-panel workspace with resizable pane grid
- Watchlist panels with drag-to-chart ticker linking
- Order entry panel with validation and risk/reward display
- Configuration persistence (TOML)
- Dark theme throughout

### Planned

| Phase | Focus |
|-------|-------|
| 1 | IB paper trading connection, live market data streaming |
| 2 | Account/position tracking, real-time bar updates |
| 3 | Trading-grade grid widget, order blotter, advanced watchlist |
| 4 | Annotations (trend lines, fibonacci), historical replay |

---

## Performance Targets

| Metric | Target |
|--------|--------|
| Frame time (1 chart, 5K candles) | < 4ms |
| Frame time (20 charts, 5K each) | < 14ms |
| Cold start to first chart | < 2s |
| Memory (20 charts, 1yr daily) | < 200 MB |
| Input-to-pixel latency | < 16ms (1 frame) |

---

## Build & Run

Requires Rust stable toolchain on Windows.

```bash
# Run the application
cargo run -p midas-app

# Run with debug logging
RUST_LOG=midas=debug cargo run -p midas-app

# Run all tests (both workspaces)
cargo test --workspace
cd desktop/win && cargo test --workspace

# Release build
cargo build --workspace --release

# Lint
cargo clippy --workspace -- -D warnings
```

---

## Test Data

No IB connection needed to explore the app. The built-in `TestDataProvider` generates realistic multi-timeframe candle data with:

- Per-ticker personality (AAPL trades differently than TSLA)
- GARCH volatility modeling
- Brownian bridge intraday interpolation
- Regime switching (trending, mean-reverting, consolidating)
- Deterministic seeding for reproducible test runs

---

## Project Status

Foundation is complete. 700+ tests passing across both workspaces. The broker engine handles the full order lifecycle with a simulation test broker. The charting layer renders production-quality candlestick charts with GPU acceleration. Next milestone is connecting to Interactive Brokers paper trading.

---

## License

MIT — Max Enko, 2026
