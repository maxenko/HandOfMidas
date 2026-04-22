# Hand of Midas

[![Rust](https://github.com/maxenko/HandOfMidas/actions/workflows/rust.yml/badge.svg)](https://github.com/maxenko/HandOfMidas/actions/workflows/rust.yml)

A personal, criteria-driven day-trading application for Interactive Brokers.

This software is built almost entirely around one person's trading style, preferences, and workflow. The charts look the way I want. The order entry works the way I think. The indicators are the ones I actually use. If you're looking for a general-purpose trading platform, this probably isn't it.

It's open source because the architecture, patterns, and GPU rendering pipeline may be useful as a foundation for your own trading application. Fork it, gut the parts you don't need, and build something that fits *your* style.

---

## Technical Overview

Hand of Midas is a native Windows desktop application written entirely in Rust. No Electron, no browser runtime, no garbage collector. The charting layer renders via custom wgpu GPU pipelines, the broker engine manages order lifecycle through a type-safe state machine, and the whole thing compiles to a single binary.

```
                      iced 0.14 (multi-window daemon)
                                |
                +---------------+---------------+
                |                               |
           midas-app                      midas-app
        (main window)                  (floating charts)
                |                               |
      +---------+---------+-------------+
      |         |         |             |
 midas-ui  midas-chart  midas-render   midas-devloop-proto
 (widgets) (sans-IO)    (wgpu 27)      (dev-harness IPC)
      |         |
 midas-grid  midas-data      midas-indicators
 (tables)  (SoA, mmap, LOD)  (ATR, Gerchik ATR)
                |
           midas-feed         midas-store         mailbox_processor
        (CSV, providers)      (DuckDB cache)      (actor channel)

                    midas-market-data
              (per-symbol router + RAII handles
               + bar aggregator registry)
                         |
                         v
                    midas-broker
                  (IB engine, rusqlite,
                   MarketDataSource + OrderClient
                   traits; sim + IB backends)
                         |
                  midas-broker-core
                (shared domain types)
```

The chart core (`midas-chart`) is a pure state machine with zero GPU or framework dependencies. It produces a `ChartScene` that the GPU renderer consumes. All chart logic -- zoom, pan, interactions, auto-scale, widget hit-testing, annotation decorators -- is fully unit-testable without a window or GPU context. **1,500+ tests** run across both workspaces in under a second.

---

## Stack

| Layer | Technology |
|-------|-----------|
| Language | Rust (stable, 2021 edition) |
| GPU | wgpu 27, custom WGSL shaders |
| GUI | iced 0.14 (multi-window daemon mode) |
| Async | tokio 1 |
| Broker | ibapi 2.10 (rust-ibapi) |
| Order DB | rusqlite (WAL mode) |
| Analytics DB | DuckDB 1.0 (candle cache) |
| Ticker state | redb 2 (per-ticker order intent) |
| Candle storage | Custom binary format, SoA layout, memmap2 |
| Math | glam 0.29 (SIMD) |

Two Cargo workspaces, 16 crates total (broker engine + market-data router + IB-gateway simulator + desktop app + shared domain types). Dependency flows strictly downward -- no cycles, no leaky abstractions. The broker's IB types never reach the UI layer; a dedicated `midas-broker-core` crate carries shared domain enums (`SecurityType`, `OrderAction`, `OptionRight`) so they can be consumed without pulling the `ibapi` transport. Market data flows through the `midas-market-data::MarketDataRouter` per-symbol fan-out, which holds `Arc<dyn MarketDataSource>` and never imports concrete backend types.

---

## Top Features

**GPU-rendered charting** -- Custom wgpu pipelines render candlesticks, volume bars, grid lines, badges, and price-line annotations. 20+ simultaneous charts at 60fps. Sub-frame input-to-pixel latency.

**Sans-IO chart architecture** -- All chart state, interactions, and computation live in a framework-agnostic core with zero GPU dependencies. The renderer is a pure consumer of computed scenes.

**Decorator annotation system** -- Composable widget pipeline built on `PriceLine` primitives. Badges, buttons, and hover zones attach to price levels and bracket legs. Domain types project into decorator trees at render time -- the domain model stays independent of visuals.

**Interactive order brackets** -- Market, Limit, Stop, and Stop-Limit entry with a 3-click bracket tool (entry + TP + SL). Auto-directional constraint enforcement, drag-to-modify legs, bidirectional panel-chart sync, GATR-based default offsets.

**TickerState machine** -- Per-ticker single source of truth. All bracket mutations, entry-type memory, GATR anchors, price levels, and session flags flow through `apply(TickerMsg) -> Vec<TickerEffect>`. Private fields, public getters, compiler-enforced invariants. The same shape (`SubMsg -> Vec<Effect>`) is now the project's canonical decomposition pattern for sub-controllers splitting out of the top-level app.

**View-model projection layer** -- Every major pane (Account, Watchlist, Chart, Order, Status bar, Toolbar) renders from a dedicated `*Vm` projection built once per frame by a `MidasApp::*_vm()` method. Views take a `&Vm`, not `&MidasApp` -- so the view layer doesn't reach across ~80 fields of top-level state, and VM builders are unit-testable without spinning up iced. Part of a multi-phase structural audit that also collapsed parallel message/state triples (4-tab column-resize → one `Message::ColumnResize` envelope; docked + floating chart maps → one iterator abstraction with `ChartHandle`) and retired duplicate per-symbol stores (`LevelStore` + `AnnotationStore` → single `AnnotationStore` with `AnnotationKind::Level`).

**Centralized annotations + symbols** -- One `AnnotationStore` keyed by `SymbolKey` holds every chart overlay (order brackets, price levels, future annotation kinds). `SymbolKey` is a `#[serde(transparent)]` newtype in `midas-core` that trims + uppercases on construction, so `"AAPL"`, `"aapl"`, and `" AAPL "` become the same key by the type system -- symbol-mismatch bugs are unrepresentable across module boundaries.

**Order bracket engine** -- OCA cancellation, validated state machine transitions across 11 order states, and a full-simulation test broker for development without an IB connection.

**Live-trading guard** -- Config refuses IB gateway port 4001 unless `allow_live = true`. You have to explicitly opt in to real money.

**Multi-panel workspace** -- Resizable pane grid, pop-out chart windows, color-coded symbol linking across panels, watchlists with drag-to-chart ticker binding, dockable order entry panel with per-ticker state persistence via redb.

**Per-ticker camera** -- Zoom and scroll position saved per (symbol, timeframe) pair and restored on ticker switch. No more losing your place.

**Custom indicator: Gerchik ATR** -- A personal ATR variant tuned to how I read volatility. Hover highlighting, per-bar detail, session-boundary re-anchoring, integrated into the chart widget system.

---

## Build & Run

Requires Rust stable toolchain on Windows.

```bash
cargo run -p midas-app                              # run the app
cargo run -p midas-app --features dev_harness       # run with devloop TCP harness (127.0.0.1:9898)
cargo test --workspace                              # broker workspace tests
cd desktop/win && cargo test --workspace            # desktop workspace tests
cargo build --workspace --release                   # release build
```

No IB connection needed to explore -- a built-in test data provider generates realistic multi-timeframe candle data with per-ticker personality, GARCH volatility, and deterministic seeding.

---

## Fair Warning

This application changes constantly as my trading style evolves. Features get added, removed, or reworked based on what I'm actually using in live sessions. It is not designed for generic day-trader use and never will be.

If you find it useful, it's most likely as:

- **A reference implementation** for Rust-based GPU charting, broker integration, or sans-IO architecture patterns.
- **A starting point** for building your own trading platform without reinventing the plumbing.
- **Context for AI agents** -- the codebase is structured and documented so that tools like Claude Code can navigate, extend, and refactor it efficiently. If you're using AI-assisted development for a trading project, the patterns here may save you significant time.

---

## License

MIT -- Max Enko, 2026
