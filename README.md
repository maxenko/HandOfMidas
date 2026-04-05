# Hand of Midas

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

The chart core (`midas-chart`) is a pure state machine with zero GPU or framework dependencies. It produces a `ChartScene` that the GPU renderer consumes. This means all chart logic -- zoom, pan, interactions, auto-scale, widget hit-testing -- is fully unit-testable without a window or GPU context. 1,000+ tests run across both workspaces in under a second.

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
| Analytics DB | DuckDB 1.0 |
| Candle storage | Custom binary format, SoA layout, memmap2 |
| Math | glam 0.29 (SIMD) |

Two Cargo workspaces, 12 crates total. Dependency flows strictly downward -- no cycles, no leaky abstractions. The broker's IB types never reach the UI layer.

---

## Top Features

**GPU-rendered charting** -- Custom wgpu pipelines render candlesticks, volume profiles, grid lines, and annotations. 20+ simultaneous charts at 60fps. Sub-frame input-to-pixel latency.

**Sans-IO chart architecture** -- All chart state, interactions, and computation live in a framework-agnostic core with zero GPU dependencies. The renderer is a pure consumer of computed scenes.

**Order bracket engine** -- Market order brackets (entry + take-profit + stop-loss) with OCA cancellation, validated state machine transitions across 11 order states, and a full-simulation test broker for development without an IB connection.

**Live-trading guard** -- Config refuses IB gateway port 4001 unless `allow_live = true`. You have to explicitly opt in to real money.

**Multi-panel workspace** -- Resizable pane grid, pop-out chart windows, symbol linking across panels, watchlists with drag-to-chart ticker binding, dockable order entry.

**Custom indicator: Gerchik ATR** -- A personal ATR variant tuned to how I read volatility. Hover highlighting, per-bar detail, integrated into the chart widget system.

---

## Build & Run

Requires Rust stable toolchain on Windows.

```bash
cargo run -p midas-app                              # run the app
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
