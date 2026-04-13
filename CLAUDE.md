# Hand of Midas

Trading platform for Interactive Brokers. Windows desktop app with GPU-rendered charts and a Rust broker engine.

**Never commit automatically.** Only commit when explicitly asked.

## Project Status

1000+ tests passing across two workspaces. Order entry with interactive bracket placement, GPU chart rendering, per-ticker state machine, and decorator-based annotation system are implemented. Phase 1 (IB paper trading connection) is next.

## Workspace Structure

Two independent Cargo workspaces share a single git repo:

```
HandOfMidas/
├── Cargo.toml                     # Root workspace: broker engine
├── crates/
│   ├── midas-core/                # Shared domain types (OrderId, SecurityType, etc.)
│   └── midas-broker/              # Trading engine — wraps IB via rust-ibapi
├── desktop/win/                   # Desktop workspace (11 crates)
│   ├── Cargo.toml                 # Workspace root with shared dependency versions
│   └── crates/
│       ├── midas-core/            # App types, config, IDs, events, CandleData trait
│       ├── midas-data/            # SoA candle buffers, binary format, mmap, LOD
│       ├── midas-chart/           # Sans-IO chart core (zero GPU deps)
│       ├── midas-render/          # wgpu 27 GPU pipelines (candles, lines, badges)
│       ├── midas-feed/            # CSV import, DataProvider trait
│       ├── midas-indicators/      # Streaming technical analysis (ATR, Gerchik ATR)
│       ├── midas-ui/              # iced widget library (buttons, labels, tooltips)
│       ├── midas-grid/            # Headless grid/table widget for iced
│       ├── midas-store/           # DuckDB persistence layer (actor-based)
│       ├── midas-app/             # Binary entry point — iced shell, ties everything
│       └── mailbox_processor/     # Async actor pattern (request-reply channels)
├── plan/                          # Active design plans + plan/archive/
├── research/                      # Research docs + research/archive/
├── design/                        # Visual design assets (Affinity Designer)
└── .github/workflows/rust.yml     # CI: test + clippy both workspaces
```

## Key Architecture Patterns

### TickerState (single source of truth)
All per-ticker state lives in `TickerState` (`midas-app/src/ticker_state/`). Mutations go through `apply(TickerMsg) -> Vec<TickerEffect>` — fields are private, read via getters. Manages: order brackets, entry-type memory, GATR anchors, price levels, and camera position. Every UI surface renders from TickerState.

### Decorator system (chart annotations)
Composable widget pipeline built on `PriceLine` primitives (`midas-chart/src/widget/`). `DecoratorGroup` containers hold badges, buttons, and spacers with visibility rules (always / on-hover). Domain types like `OrderBracket` project into decorator trees at render time — domain model stays independent of visuals.

### Order brackets
Three-leg order annotations (entry + optional TP/SL) with interactive chart placement via `BracketTool` — a 3-click state machine with auto-directional constraint enforcement. Supports Market, Limit, Stop, and StopLimit entry types. Lives in TickerState, projected to `AnnotationStore` via effects.

### ChartViewStore (per-ticker viewport)
Session-scoped per-(symbol, timeframe) camera state (`midas-app/src/chart_view.rs`). Saves zoom levels on user interaction, restores on ticker switch, positions camera on data load.

### Sans-IO chart core
`midas-chart` has zero GPU or framework dependencies. All chart logic (state, interactions, zoom/pan, hit-testing, dirty flags) lives here. `midas-render` reads `ChartScene` to build GPU primitives.

## Architecture Rules

1. **No ibapi types in public API.** UI crate never imports ibapi.
2. **Split channel architecture.** Market data on `broadcast(4096)`, order events on `broadcast(8192)`, connection state on `watch`.
3. **Live-trading guard.** Config refuses port 4001 unless `allow_live = true`.
4. **Two-tier DB writes.** Critical (orders, fills) awaited; non-critical fire-and-forget.
5. **SecurityType enum over strings.** Use `SecurityType::Stock` not `"STK"`.
6. **MarketDataSource trait.** Test data and future IB both implement this.
7. **All bracket mutations through TickerState.** No direct bracket modification outside `apply()`.
8. **Dependency flow is strictly downward.** No circular crate dependencies.

## Build & Test

```bash
# Root workspace (broker engine)
cargo test --workspace
cargo clippy --workspace -- -D warnings

# Desktop workspace
cd desktop/win
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --all
cargo run -p midas-app
cargo build --workspace --release
```

## Documentation Map

| Topic | Where to look |
|---|---|
| midas-core API | `crates/midas-core/doc/api.md` |
| midas-broker API | `crates/midas-broker/doc/` (6 files) |
| Broker architecture | `plan/broker/01-architecture.md` |
| Order state machine | `plan/broker/02-order-management.md` |
| Data layer schema | `plan/broker/03-data-layer.md` |
| Events & commands | `plan/broker/04-market-data-and-events.md` |
| Widget system design | `plan/widget-system/00-index.md` |
| Decorator system | `plan/archive/decorator-system/00-index.md` |
| Ticker state machine | `desktop/win/plan/archive/ticker-state-machine/` |
| Ticker order state | `desktop/win/plan/archive/ticker-order-state/` |
| Grid component | `desktop/win/plan/grid-component/README.md` |
| Desktop UI architecture | `desktop/win/plan/archive/initial/00-index.md` |
| IB API reference | `research/provider-ib.md` |
