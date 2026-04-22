# Hand of Midas

Trading platform for Interactive Brokers. Windows desktop app with GPU-rendered charts and a Rust broker engine.

**Never commit automatically.** Only commit when explicitly asked.

**Use `gh` for all GitHub interactions** — PRs, issues, checks, releases, comments, reviews. Prefer `gh` over the web UI or raw `git` remote commands for anything that touches github.com.

## Dev harness (autonomous loops + UI screenshots)

`midas-app` ships an in-process TCP harness behind the `dev_harness` Cargo feature. It's the entry point for autonomous dev loops: Claude (or any client) drives the running app over newline-delimited JSON on `127.0.0.1:9898`.

```bash
cd desktop/win
cargo run -p midas-app --features dev_harness
cargo run -p midas-app --features dev_harness -- --fixture <name>   # boot from a saved fixture
```

Supported commands (see `desktop/win/crates/midas-devloop-proto/src/lib.rs`):
- `Ping`, `Shutdown`
- `LoadFixture` / `SnapshotFixture` — persist and replay app state
- `DumpState` — JSON projection of `MidasApp`, optional JSON-pointer path
- `Screenshot` — captures the main iced window to PNG, diffs against a reference, reports SSIM + diff fraction
- `WaitForEvent` / `WaitForIdle` — block on the event log or settle quiescence
- `InjectTickerMsg` / `InjectBrokerEvent` — drive domain mutations directly
- `Key`, `Scroll`, `OpenOrdersPanel`

Runtime artifacts land in `desktop/win/.devloop/`: `app.<port>.pid`, `events.jsonl`, `panic.txt`. Smoke scripts: `desktop/win/tools/devloop-smoke.sh`, `devloop-orders-journey.sh`.

Use the harness (not manual `cargo run` + eyeballing) to verify UI-visible changes in a dev loop: boot a fixture → inject/key → `WaitForIdle` → `Screenshot` → compare.

## Project Status

1000+ tests passing across two workspaces. Order entry with interactive bracket placement, GPU chart rendering, per-ticker state machine, and decorator-based annotation system are implemented. Phase 1 (IB paper trading connection) is next.

## Workspace Structure

Two independent Cargo workspaces share a single git repo:

```
HandOfMidas/
├── Cargo.toml                     # Root workspace: broker engine + market-data router
├── crates/
│   ├── midas-broker-core/         # Shared domain types (OrderId, SecurityType, market-data events)
│   ├── midas-broker/              # Trading engine — sim + IB backends behind MarketDataSource / OrderClient traits
│   ├── midas-market-data/         # Per-symbol router + bar aggregator registry + RAII subscription handles
│   ├── midas-ib-sim/              # In-process IB-gateway simulator for integration tests
│   └── mailbox_processor/         # Async actor pattern (request-reply channels)
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

### MarketDataRouter (streaming topology)
Market data flows through a single per-symbol fan-out router (`crates/midas-market-data`) that sits between the broker backend and UI consumers. The `MarketDataSource` trait abstracts the provider (sim or IB); the router holds `Arc<dyn MarketDataSource>` and never sees concrete types. For each active symbol the router spawns a `SymbolHub` with three broadcast lanes (ticks, realtime bars, quote watch) plus refcounted RAII subscription handles — drop a handle and the last-ref decrement cascades an upstream cancel. A lazily spawned `BarAggregator` actor registry fans (symbol, timeframe) aggregates off the shared tick stream.

```
midas-broker (SimMarketData | IbMarketData : MarketDataSource)
      ↓ Arc<dyn MarketDataSource>
midas-market-data::MarketDataRouter
   ├─ per-symbol SymbolHub (ticks / rt-bars / quote watch, refcounted)
   └─ BarAggregatorRegistry (lazy per (sym, tf) — subscribes once, fans out)
      ↓
midas-app consumers (chart / watchlist / ticker-state), each holding its
own SubscriptionHandle via a per-widget iced::Subscription::channel
with frame-rate coalescing
```

Order flow (place, cancel, modify, positions, account events) is on a separate `OrderClient` trait with its own broadcast channels; the sim and IB backends each implement both traits.

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

1. **No ibapi types in public API.** UI crate never imports ibapi; the `midas-market-data` router never imports `midas-broker` concrete types — it holds `Arc<dyn MarketDataSource>`.
2. **Split channel architecture.** Market data goes through the router's per-symbol fan-out; order events on `broadcast(8192)`; connection state on `watch`.
3. **Live-trading guard.** Config refuses port 4001 unless `allow_live = true`.
4. **Two-tier DB writes.** Critical (orders, fills) awaited; non-critical fire-and-forget.
5. **SecurityType enum over strings.** Use `SecurityType::Stock` not `"STK"`.
6. **`MarketDataSource` + `OrderClient` traits.** Sim and IB backends both implement these; the router and UI never see concrete backends.
7. **RAII subscription handles.** Every market-data consumer holds a `SubscriptionHandle`; drop cascades upstream cancellation via the router's refcount.
8. **All bracket mutations through TickerState.** No direct bracket modification outside `apply()`.
9. **Dependency flow is strictly downward.** `midas-broker-core` → `midas-broker` / `midas-market-data` → `midas-app`. No circular crate dependencies.

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
cargo run -p midas-app --features dev_harness   # run with devloop TCP harness on 127.0.0.1:9898
cargo build --workspace --release
```

## Documentation Map

| Topic | Where to look |
|---|---|
| midas-broker-core API | `crates/midas-broker-core/doc/api.md` |
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
