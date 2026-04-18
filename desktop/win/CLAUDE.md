# CLAUDE.md — AI Assistant Context for Hand of Midas

## What is this project?

Hand of Midas is a native desktop stock charting application written in pure Rust.
It renders 20+ simultaneous charts at 60fps using custom wgpu 27 GPU pipelines inside
an iced 0.14 GUI shell. Target platform: Windows 11 (x86_64).

## Workspace structure

This is a Cargo workspace with 11 crates under `crates/`:

| Crate | Type | Purpose | Internal deps |
|---|---|---|---|
| `midas-core` | lib | Shared types, IDs, events, config schema, `CandleData` trait | none (leaf) |
| `midas-data` | lib | SoA candle buffers, binary format, mmap, LOD | midas-core |
| `midas-indicators` | lib | Streaming technical analysis (ATR, Gerchik ATR) | midas-core |
| `midas-chart` | lib | Sans-IO chart core: ChartState, ChartScene, Camera2D, interactions, decorators, brackets | midas-core, midas-data |
| `midas-render` | lib | wgpu GPU rendering pipelines (candles, lines, badges), reads ChartScene | midas-core, midas-data, midas-chart |
| `midas-feed` | lib | CSV import, data provider trait | midas-core, midas-data |
| `midas-ui` | lib | Custom iced widgets: buttons, labels, tooltips, button groups | midas-core |
| `midas-grid` | lib | Headless grid/table widget for iced | none |
| `midas-store` | lib | DuckDB persistence layer, actor-based async ops | midas-core, midas-data |
| `midas-app` | bin | iced application shell, TickerState machine, ties everything together | all above |
| `mailbox_processor` | lib | Async actor pattern with request-reply channels | none |

Dependency flows strictly downward. No circular dependencies.

## Build commands

```bash
# Build entire workspace
cargo build --workspace

# Run the application
cargo run -p midas-app

# Run with logging
RUST_LOG=midas=debug cargo run -p midas-app

# Run with dev harness (TCP socket on 127.0.0.1:9898 for Claude-driven
# fixtures, event-log tailing, screenshot capture, inject_ticker_msg).
# Boot from a saved fixture: add `-- --fixture <name>`.
# See plan/devloop-spec.md and tools/devloop-smoke.sh.
cargo run -p midas-app --features dev_harness

# Run all tests
cargo test --workspace

# Run clippy
cargo clippy --workspace -- -D warnings

# Format code
cargo fmt --all

# Release build (slow compile, fast binary)
cargo build --workspace --release

# Profiling build (release + debug symbols)
cargo build --workspace --profile release-debug
```

## Key conventions

### Code style
- Rust 2021 edition, stable toolchain
- `snake_case` for functions, variables, modules
- `PascalCase` for types, traits, enum variants
- `SCREAMING_SNAKE_CASE` for constants
- Max line width: 100 characters (rustfmt default)
- Prefer `thiserror` for library error types, `anyhow` only in midas-app
- All public items must have doc comments (`///`)
- No `unwrap()` in library crates — use `?` or explicit error handling
- `unwrap()` is acceptable only in tests and main.rs during early development

### Architecture rules
- `midas-chart` is the sans-IO chart core. It has zero GPU or framework dependencies. All chart logic (state, interactions, zoom/pan, auto-scale) lives here.
- `midas-chart` consumes data through the `CandleData` trait (defined in `midas-core`), not concrete types.
- `midas-render` reads `ChartScene` (produced by `midas-chart`) to build GPU primitives. It does NOT depend on iced.
- `midas-core` must remain small and stable. Changes recompile the entire workspace.
- GPU data structs must be `#[repr(C)]` and derive `bytemuck::Pod + Zeroable`.
- SoA (Structure of Arrays) layout for candle data, not AoS.
- Shaders are WGSL files in `crates/midas-render/shaders/`, included via `include_str!()`.
- Configuration is TOML (not JSON, not YAML).

### File locations
- Shaders: `crates/midas-render/shaders/*.wgsl`
- Runtime data: `data/` (gitignored)
- Binary candle files: `data/candles/<SYMBOL>/<timeframe>.candles`
- User config: `data/config.toml`
- Test fixtures: `tests/data/`
- Design documents: `plan/`

### Error handling pattern
```rust
// In library crates (midas-core, midas-data, midas-chart, midas-render, midas-feed):
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DataError {
    #[error("file not found: {path}")]
    FileNotFound { path: String },
    #[error("invalid binary format: {0}")]
    InvalidFormat(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

// In midas-app (binary crate):
use anyhow::{Context, Result};
fn load_data() -> Result<()> {
    let buf = midas_data::load("AAPL")
        .context("failed to load AAPL candle data")?;
    Ok(())
}
```

### GPU data struct pattern
```rust
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CandleInstance {
    pub x: f32,
    pub body_top: f32,
    pub body_bottom: f32,
    pub wick_top: f32,
    pub wick_bottom: f32,
    pub width: f32,
    pub color: [f32; 4],
}
```

## Performance targets

| Metric | Target |
|---|---|
| Frame time (1 chart, 5K candles) | < 4ms |
| Frame time (20 charts, 5K candles each) | < 14ms |
| Cold start to first chart | < 2 seconds |
| Memory (20 charts, 1yr daily each) | < 200 MB |
| Input-to-pixel latency | < 16ms (1 frame) |

## Tech stack versions

- Rust: stable (pinned via rust-toolchain.toml)
- wgpu: 27
- iced: 0.14 (with wgpu + tokio features)
- bytemuck: 1 (with derive feature)
- glam: 0.29 (with bytemuck feature)
- tokio: 1 (rt-multi-thread, macros, sync, time)
- serde: 1 (with derive feature)
- tracing: 0.1
