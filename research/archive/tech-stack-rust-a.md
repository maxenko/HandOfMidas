# Implementation Plan: Hand of Midas — Pure Rust Charting Platform

> Stack A: Rust + wgpu + iced + Vello | Codename: **Midas**
> Based on tech-stacks.md research (March 2026)

---

## Table of Contents

- [Project Overview](#project-overview)
- [Project Structure](#project-structure)
- [Dependency Map](#dependency-map)
- [Phase 0: Scaffold & Proof of Life](#phase-0-scaffold--proof-of-life)
- [Phase 1: GPU Rendering Foundation](#phase-1-gpu-rendering-foundation)
- [Phase 2: Data Layer](#phase-2-data-layer)
- [Phase 3: Chart Interaction](#phase-3-chart-interaction)
- [Phase 4: iced Application Shell](#phase-4-iced-application-shell)
- [Phase 5: Indicator Engine](#phase-5-indicator-engine)
- [Phase 6: Multi-Chart & Sync](#phase-6-multi-chart--sync)
- [Phase 7: Real-Time Streaming](#phase-7-real-time-streaming)
- [Phase 8: Polish & Production Hardening](#phase-8-polish--production-hardening)
- [Appendix A: WGSL Shader Specifications](#appendix-a-wgsl-shader-specifications)
- [Appendix B: Binary File Format Specification](#appendix-b-binary-file-format-specification)
- [Appendix C: Risk Register](#appendix-c-risk-register)

---

## Project Overview

### What We're Building

A native desktop stock charting application with:
- TC2000-level pixel-perfect crispness
- 20+ simultaneous charts at 60fps
- Thousands of candles per chart with smooth zoom/pan/scale
- Horizontal price levels (extensible to any drawing tool)
- Real-time streaming from multiple data providers
- Full ownership of every pixel in the rendering pipeline

### Non-Goals (for v1)

- Web deployment (native desktop only)
- Mobile support
- Trading / order execution
- Level 2 / order book visualization
- Replay / backtesting UI
- Social features / sharing

### Success Criteria

| Metric | Target |
|---|---|
| Frame time (single chart, 5K candles) | < 4ms (250+ fps theoretical) |
| Frame time (20 charts, 5K candles each) | < 14ms (60fps sustained) |
| Cold start to first chart rendered | < 2 seconds |
| Memory (20 charts, 1 year daily data each) | < 200 MB |
| Zoom/pan input-to-pixel latency | < 16ms (1 frame) |
| Pixel alignment | All candle edges on exact pixel boundaries |
| Data update latency (WebSocket tick → pixel) | < 3 frames (~50ms) |

---

## Project Structure

```
midas/
├── Cargo.toml                    # Workspace root
├── CLAUDE.md                     # AI assistant context
├── README.md
│
├── crates/
│   ├── midas-app/                # Binary crate — iced application shell
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── main.rs           # Entry point, iced::Application
│   │       ├── app.rs            # App state, Message enum, update/view
│   │       ├── theme.rs          # Colors, fonts, spacing constants
│   │       ├── views/
│   │       │   ├── mod.rs
│   │       │   ├── workspace.rs  # Multi-chart layout grid
│   │       │   ├── toolbar.rs    # Top toolbar (symbol search, timeframe selector)
│   │       │   ├── sidebar.rs    # Watchlist panel
│   │       │   └── statusbar.rs  # Connection status, clock
│   │       └── widgets/
│   │           ├── mod.rs
│   │           └── chart_widget.rs  # iced Shader widget wrapper
│   │
│   ├── midas-render/             # Chart GPU renderer (wgpu)
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── renderer.rs       # ChartRenderer — owns wgpu pipeline
│   │       ├── pipelines/
│   │       │   ├── mod.rs
│   │       │   ├── candle.rs     # Instanced candlestick pipeline
│   │       │   ├── volume.rs     # Instanced volume bar pipeline
│   │       │   ├── line.rs       # Line strip pipeline (indicators, grid)
│   │       │   ├── hline.rs      # Horizontal level pipeline
│   │       │   ├── crosshair.rs  # Crosshair overlay pipeline
│   │       │   └── rect.rs       # General rectangle pipeline (selections, highlights)
│   │       ├── shaders/
│   │       │   ├── candle.wgsl
│   │       │   ├── volume.wgsl
│   │       │   ├── line.wgsl
│   │       │   ├── hline.wgsl
│   │       │   ├── crosshair.wgsl
│   │       │   └── rect.wgsl
│   │       ├── text/
│   │       │   ├── mod.rs
│   │       │   ├── atlas.rs      # MSDF glyph atlas generator
│   │       │   ├── layout.rs     # Axis label layout & positioning
│   │       │   └── msdf.wgsl     # Text rendering shader
│   │       ├── camera.rs         # 2D orthographic projection, zoom/pan state
│   │       ├── viewport.rs       # Pixel dimensions, DPI, coordinate transforms
│   │       └── color.rs          # Color constants, bull/bear palettes
│   │
│   ├── midas-data/               # Data storage, loading, binary format
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── candle.rs         # OHLCV struct, SoA CandleBuffer
│   │       ├── binary.rs         # Binary file format read/write/mmap
│   │       ├── symbol.rs         # SymbolId, symbol registry
│   │       ├── timeframe.rs      # Timeframe enum, boundary calculations
│   │       ├── lod.rs            # Level-of-detail downsampling (MinMax, LTTB)
│   │       └── cache.rs          # In-memory LRU cache for loaded data
│   │
│   ├── midas-feed/               # Market data ingest
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── provider.rs       # DataProvider trait
│   │       ├── polygon.rs        # Polygon.io WebSocket + REST
│   │       ├── csv.rs            # CSV file import (for dev/testing)
│   │       ├── aggregator.rs     # Tick → candle aggregation (all timeframes)
│   │       └── replay.rs         # Historical data replay for testing
│   │
│   ├── midas-indicators/         # Technical indicator engine
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── engine.rs         # IndicatorEngine — DAG executor
│   │       ├── traits.rs         # IncrementalIndicator trait
│   │       ├── registry.rs       # Indicator registry (name → constructor)
│   │       ├── overlays/         # Price-overlay indicators
│   │       │   ├── mod.rs
│   │       │   ├── sma.rs
│   │       │   ├── ema.rs
│   │       │   ├── bollinger.rs
│   │       │   └── vwap.rs
│   │       └── oscillators/      # Sub-chart indicators
│   │           ├── mod.rs
│   │           ├── rsi.rs
│   │           └── macd.rs
│   │
│   └── midas-core/               # Shared types, events, coordination
│       ├── Cargo.toml
│       └── src/
│           ├── lib.rs
│           ├── events.rs         # MarketEvent, ChartEvent, UIEvent enums
│           ├── time_axis.rs      # TimeAxisController
│           ├── chart_state.rs    # Per-chart state (symbol, timeframe, viewport, indicators)
│           ├── config.rs         # App config (serde, persistence)
│           └── id.rs             # ChartId, SymbolId, IndicatorId types
│
├── data/                         # Local data directory (gitignored)
│   ├── candles/                  # mmap'd binary candle files
│   │   ├── AAPL/
│   │   │   ├── 1m.candles
│   │   │   ├── 5m.candles
│   │   │   └── 1d.candles
│   │   └── SPY/
│   └── config.toml               # User configuration
│
└── tests/
    ├── rendering/                # Visual regression tests (screenshot comparison)
    ├── data/                     # Sample CSV/binary data for testing
    └── benchmarks/               # Criterion benchmarks
        ├── render_bench.rs
        ├── lod_bench.rs
        └── indicator_bench.rs
```

---

## Dependency Map

### Crate Dependency Graph

```
midas-app
├── midas-render    (GPU rendering)
├── midas-core      (shared types, events, coordination)
├── midas-data      (storage, SoA buffers)
├── midas-feed      (market data ingest)
└── midas-indicators (indicator computation)

midas-render
├── midas-core
└── midas-data      (reads CandleBuffer for GPU upload)

midas-feed
├── midas-core
└── midas-data      (writes to binary files and buffers)

midas-indicators
├── midas-core
└── midas-data      (reads CandleBuffer for computation)

midas-data
└── midas-core      (shared types only)

midas-core
└── (no internal deps — leaf crate)
```

### External Dependencies

```toml
[workspace.dependencies]
# GPU
wgpu = "29"
bytemuck = { version = "1", features = ["derive"] }

# GUI
iced = { version = "0.13", features = ["wgpu", "multi-window", "tokio"] }

# 2D rendering (overlays, text — Phase 5+)
# vello = "0.6"         # Deferred — evaluate after core pipeline works
# parley = "0.4"        # Text layout (used by Vello)

# Async
tokio = { version = "1", features = ["full"] }
tokio-tungstenite = "0.24"

# Concurrency
crossbeam = "0.8"
triple_buffer = "8"
parking_lot = "0.12"   # Faster Mutex/RwLock for non-hot-path

# Data
memmap2 = "0.9"
byteorder = "1"

# Serialization
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.8"

# Logging
tracing = "0.1"
tracing-subscriber = "0.3"

# Math
glam = "0.29"          # Vec2/Vec4/Mat4 for GPU math (no_std, SIMD)

# Fonts (text rendering)
ab_glyph = "0.2"       # Font parsing + rasterization for MSDF atlas
etagere = "0.2"         # Shelf-packing atlas allocator

# Utils
thiserror = "2"
anyhow = "1"
chrono = { version = "0.4", features = ["serde"] }
```

---

## Phase 0: Scaffold & Proof of Life

> **Goal**: Cargo workspace compiles, iced window opens, wgpu surface clears to background color.
> **Effort**: 2-3 days
> **Validates**: Toolchain, wgpu + iced integration, build times

### Tasks

#### 0.1 — Initialize Workspace

```
cargo init --name midas midas/
```

Create `Cargo.toml` workspace with all crate stubs. Each crate starts with `lib.rs` or `main.rs` containing just enough to compile.

**Acceptance**: `cargo build --workspace` succeeds. `cargo clippy --workspace` clean.

#### 0.2 — iced Window with wgpu Clear

In `midas-app/src/main.rs`:
- Create an `iced::Application` with a single window
- Title: "Midas"
- Background: dark charcoal (`#1a1a2e`)
- Window size: 1280x800

**Acceptance**: Window opens, shows solid dark background, resizes smoothly, closes cleanly.

#### 0.3 — iced Shader Widget Smoke Test

In `midas-render/`:
- Create a minimal `ChartRenderer` struct that implements iced's `Shader` trait (or the appropriate wgpu primitive trait)
- On each frame, clear to a slightly different color than the app background (proves the custom render pass is running)
- Place this widget in the center of the iced layout

**Acceptance**: A rectangle in the window renders a different background color via custom wgpu code. Resizing the window resizes the render surface.

#### 0.4 — Tracing & Dev Tooling

- Set up `tracing-subscriber` with `RUST_LOG` env filtering
- Add frame-time logging (print ms per frame to trace output)
- Add `.cargo/config.toml` with `target-cpu=native` for SIMD and optimized dev builds:
  ```toml
  [target.'cfg(target_os = "windows")']
  rustflags = ["-C", "target-cpu=native"]

  [profile.dev]
  opt-level = 1          # Faster dev builds for GPU code

  [profile.dev.package."*"]
  opt-level = 2          # Optimize dependencies fully
  ```

**Acceptance**: Frame timing appears in console. Dev build compiles in < 30s after initial.

---

## Phase 1: GPU Rendering Foundation

> **Goal**: Render 5,000 candlesticks from hardcoded data using instanced wgpu pipeline. Pixel-perfect, 60fps.
> **Effort**: 3-4 weeks
> **Depends on**: Phase 0

### Tasks

#### 1.1 — Camera & Coordinate System

`midas-render/src/camera.rs`:

```rust
pub struct Camera2D {
    /// Visible time range (x-axis) in epoch milliseconds
    pub time_start: f64,
    pub time_end: f64,

    /// Visible price range (y-axis)
    pub price_low: f64,
    pub price_high: f64,

    /// Pixel dimensions of the render surface
    pub viewport_width: u32,
    pub viewport_height: u32,

    /// Device pixel ratio (for HiDPI)
    pub dpi_scale: f32,
}
```

Methods:
- `time_to_x(timestamp) -> f32` — maps timestamp to pixel X
- `price_to_y(price) -> f32` — maps price to pixel Y (inverted: high prices = low Y)
- `x_to_time(pixel_x) -> f64` — inverse for mouse interaction
- `y_to_price(pixel_y) -> f64` — inverse for mouse interaction
- `pixels_per_candle() -> f32` — derived from time range and candle width
- `visible_candle_count() -> usize`
- `projection_matrix() -> glam::Mat4` — orthographic projection for the GPU uniform

**Pixel alignment rule**: All coordinate transforms must `round()` to physical pixel boundaries. A candle body at DPI 2.0 snaps to half-CSS-pixels (which are full physical pixels).

**Acceptance**: Unit tests verify round-trip `time_to_x(x_to_time(x)) == x` within 1 physical pixel. Projection matrix produces correct NDC coordinates for known inputs.

#### 1.2 — Candle Instance Data Layout

`midas-render/src/pipelines/candle.rs`:

Define the GPU instance struct (must match WGSL struct):

```rust
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CandleInstance {
    /// Pixel-space position and dimensions
    pub x: f32,           // Center X of candle (pixel)
    pub body_top: f32,    // Top of body (pixel Y — min of open/close)
    pub body_bottom: f32, // Bottom of body (pixel Y — max of open/close)
    pub wick_top: f32,    // High price (pixel Y)
    pub wick_bottom: f32, // Low price (pixel Y)
    pub width: f32,       // Candle body width (pixels)
    pub color: [f32; 4],  // RGBA (green for bull, red for bear)
}
```

Total: 32 bytes per instance. For 5,000 candles = 160 KB. Trivial GPU upload.

**Acceptance**: `CandleInstance` is `Pod + Zeroable`. `std::mem::size_of::<CandleInstance>()` == 32. Alignment verified.

#### 1.3 — Candlestick WGSL Shader

`midas-render/src/shaders/candle.wgsl`:

The shader renders each candle as two primitives from a single instanced draw call:
1. **Body**: Filled rectangle from `body_top` to `body_bottom`
2. **Wick**: 1px-wide vertical line from `wick_top` to `wick_bottom`

Approach: Use a unit quad (6 vertices forming 2 triangles) as the base geometry. The vertex shader reads per-instance data and transforms the quad to the correct screen position.

For the wick: A second draw call (or same shader with a mode flag) renders a thin rectangle of width 1 physical pixel.

Alternative (single pass): Emit a single rectangle per instance that covers the full wick height, then in the fragment shader, use the fragment's Y position to determine whether to draw body color or wick color. This reduces to 1 draw call total.

```wgsl
// Uniform: orthographic projection
struct Uniforms {
    projection: mat4x4<f32>,
}
@group(0) @binding(0) var<uniform> uniforms: Uniforms;

// Per-instance data
struct CandleInstance {
    @location(1) x: f32,
    @location(2) body_top: f32,
    @location(3) body_bottom: f32,
    @location(4) wick_top: f32,
    @location(5) wick_bottom: f32,
    @location(6) width: f32,
    @location(7) color: vec4<f32>,
}

// Vertex: unit quad [0,1]x[0,1] expanded to candle dimensions
struct VertexInput {
    @location(0) position: vec2<f32>,  // (0,0), (1,0), (1,1), (0,1)
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) local_y: f32,         // 0..1 within the wick range
    @location(2) body_top_norm: f32,   // body_top mapped to 0..1 in wick range
    @location(3) body_bottom_norm: f32,
}

@vertex
fn vs_main(vert: VertexInput, inst: CandleInstance) -> VertexOutput {
    // Expand unit quad to cover full wick height, candle width
    let wick_height = inst.wick_bottom - inst.wick_top;
    let px = inst.x - inst.width * 0.5 + vert.position.x * inst.width;
    let py = inst.wick_top + vert.position.y * wick_height;

    var out: VertexOutput;
    out.clip_position = uniforms.projection * vec4<f32>(px, py, 0.0, 1.0);
    out.color = inst.color;
    out.local_y = vert.position.y;
    out.body_top_norm = (inst.body_top - inst.wick_top) / wick_height;
    out.body_bottom_norm = (inst.body_bottom - inst.wick_top) / wick_height;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Inside body range: full color
    if in.local_y >= in.body_top_norm && in.local_y <= in.body_bottom_norm {
        return in.color;
    }
    // Outside body but inside wick: draw 1px wick
    // Check if fragment is within wick_width of center
    // (handled by making wick geometry 1px wide — see below)
    return in.color;
}
```

**Note**: The single-pass approach above is a starting point. In practice, we'll likely use two sub-draws:
1. Wick: Instanced thin rectangles (1-2px wide, full wick height)
2. Body: Instanced rectangles (candle width, body height)

Draw wicks first, bodies on top. 2 draw calls for all candles.

**Acceptance**: 5,000 colored rectangles render at correct positions. Green for bullish (close > open), red for bearish. Wicks visible as thin lines through bodies.

#### 1.4 — wgpu Pipeline Setup

`midas-render/src/pipelines/candle.rs`:

```rust
pub struct CandlePipeline {
    render_pipeline: wgpu::RenderPipeline,
    vertex_buffer: wgpu::Buffer,        // Unit quad (6 vertices)
    instance_buffer: wgpu::Buffer,      // CandleInstance array
    instance_count: u32,
    uniform_buffer: wgpu::Buffer,       // Projection matrix
    uniform_bind_group: wgpu::BindGroup,
}
```

Methods:
- `new(device, format) -> Self` — creates pipeline, compiles shader, allocates buffers
- `update_instances(queue, candles: &[CandleInstance])` — uploads instance data to GPU
- `update_uniforms(queue, projection: Mat4)` — uploads projection matrix
- `draw(render_pass)` — issues the instanced draw call

Buffer strategy:
- Allocate instance buffer with capacity for 10,000 candles initially
- Grow by 2x when exceeded (rare — only on extreme zoom-out)
- Use `queue.write_buffer()` for full updates
- Use partial writes for appending new candles (future optimization)

**Acceptance**: Pipeline compiles without validation errors. Can draw 10,000 instances.

#### 1.5 — Volume Bar Pipeline

`midas-render/src/pipelines/volume.rs`:

Nearly identical to candle pipeline but simpler — just filled rectangles at the bottom of the chart. Each bar has position, height (proportional to volume), and color (bull/bear).

```rust
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct VolumeInstance {
    pub x: f32,
    pub y_top: f32,      // Top of volume bar
    pub y_bottom: f32,    // Bottom of chart area (constant)
    pub width: f32,
    pub color: [f32; 4],  // Semi-transparent bull/bear color
}
```

Volume bars render with ~30% opacity so price data remains clearly visible.

**Acceptance**: Volume bars align with candles. Taller bars for higher volume. Semi-transparent.

#### 1.6 — Grid Lines Pipeline

`midas-render/src/pipelines/line.rs`:

Renders horizontal price grid lines and vertical time grid lines as thin rectangles (1 physical pixel wide/tall).

```rust
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct LineInstance {
    pub start: [f32; 2],  // Start point (pixel)
    pub end: [f32; 2],    // End point (pixel)
    pub width: f32,        // Line width in pixels
    pub color: [f32; 4],
}
```

Grid colors: very faint (`#ffffff10` on dark background). Major grid lines slightly brighter.

**Acceptance**: Grid lines visible at correct price intervals. Exactly 1 physical pixel wide (no blur). Grid adapts to zoom level (more lines when zoomed in, fewer when zoomed out).

#### 1.7 — ChartRenderer Orchestrator

`midas-render/src/renderer.rs`:

```rust
pub struct ChartRenderer {
    candle_pipeline: CandlePipeline,
    volume_pipeline: VolumePipeline,
    line_pipeline: LinePipeline,
    camera: Camera2D,
    is_dirty: bool,
}

impl ChartRenderer {
    pub fn set_data(&mut self, candles: &CandleBuffer) { ... }
    pub fn set_camera(&mut self, camera: Camera2D) { ... }
    pub fn render(&mut self, device: &wgpu::Device, queue: &wgpu::Queue,
                  target: &wgpu::TextureView, viewport: Viewport) { ... }
}
```

Render order:
1. Clear to background color
2. Draw grid lines
3. Draw volume bars (semi-transparent)
4. Draw candle wicks
5. Draw candle bodies (on top of wicks)
6. (Later: indicators, horizontal levels, crosshair, text)

**Acceptance**: Full chart rendering with candles + volume + grid from hardcoded data. 60fps with 5,000 candles. Screenshot matches expected layout.

#### 1.8 — Pixel-Perfect Alignment Validation

The most critical task in Phase 1. Implement and verify:

1. **Physical pixel snapping**: All candle edges round to physical pixel boundaries
   ```rust
   fn snap_to_pixel(value: f32, dpi_scale: f32) -> f32 {
       (value * dpi_scale).round() / dpi_scale
   }
   ```

2. **Consistent candle widths**: All candles must have identical pixel width. Compute width once and reuse (don't derive per-candle — floating point drift causes 1px jitter).

3. **Wick centering**: Wick always centered on candle body. For even-width candles, wick straddles two pixels — use floor for even widths.

4. **DPI awareness**: Test at 1.0x, 1.5x, 2.0x DPI scales. Candles must be crisp at all scales.

5. **Anti-aliasing control**: Disable MSAA for chart rendering. Use shader-based AA only where needed (indicator lines). Candle bodies and wicks should be hard-edged.

**Acceptance**: Screenshot comparison at multiple DPI scales. No blurry edges. No 1px jitter between adjacent candles. Zoom in to pixel level and verify alignment.

---

## Phase 2: Data Layer

> **Goal**: Load OHLCV data from CSV, convert to binary format, mmap for rendering. SoA buffer feeds GPU.
> **Effort**: 2-3 weeks
> **Depends on**: Phase 0 (for types), can parallel with Phase 1

### Tasks

#### 2.1 — Core Types

`midas-core/src/id.rs`:
```rust
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub struct SymbolId(pub u32);

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub struct ChartId(pub u32);
```

`midas-data/src/timeframe.rs`:
```rust
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, serde::Serialize, serde::Deserialize)]
pub enum Timeframe {
    S1, S5, S15, S30,
    M1, M5, M15, M30,
    H1, H4,
    D1, W1, MN1,
}

impl Timeframe {
    pub fn as_secs(&self) -> u32 { ... }
    pub fn floor_timestamp(&self, ts: i64) -> i64 { ... }  // Align to boundary
    pub fn next_boundary(&self, ts: i64) -> i64 { ... }
}
```

**Acceptance**: `floor_timestamp` correctly aligns to 5m/15m/1h/etc boundaries including DST and market hours.

#### 2.2 — SoA CandleBuffer

`midas-data/src/candle.rs`:

```rust
/// Structure of Arrays layout for cache-friendly access
pub struct CandleBuffer {
    pub timestamps: Vec<i64>,   // Epoch milliseconds
    pub opens:  Vec<f32>,
    pub highs:  Vec<f32>,
    pub lows:   Vec<f32>,
    pub closes: Vec<f32>,
    pub volumes: Vec<u32>,
}

impl CandleBuffer {
    pub fn new() -> Self { ... }
    pub fn with_capacity(n: usize) -> Self { ... }
    pub fn len(&self) -> usize { ... }
    pub fn push(&mut self, ts: i64, o: f32, h: f32, l: f32, c: f32, v: u32) { ... }
    pub fn slice(&self, start: usize, end: usize) -> CandleSlice<'_> { ... }
    pub fn find_index_by_time(&self, ts: i64) -> usize { ... }  // Binary search

    // For rendering: extract visible range as GPU-ready instance data
    pub fn to_candle_instances(&self, range: Range<usize>, camera: &Camera2D) -> Vec<CandleInstance> { ... }

    // Price range for auto-scaling Y axis (SIMD-friendly)
    pub fn price_range(&self, range: Range<usize>) -> (f32, f32) { ... }
}

/// Zero-copy slice view into a CandleBuffer
pub struct CandleSlice<'a> {
    pub timestamps: &'a [i64],
    pub opens:  &'a [f32],
    pub highs:  &'a [f32],
    pub lows:   &'a [f32],
    pub closes: &'a [f32],
    pub volumes: &'a [u32],
}
```

**Acceptance**: `price_range` over 100K candles completes in < 50us. `to_candle_instances` for 5K candles < 200us.

#### 2.3 — CSV Import

`midas-feed/src/csv.rs`:

Parse standard OHLCV CSV files (Yahoo Finance, Polygon, etc.) into `CandleBuffer`. Support formats:
- `Date,Open,High,Low,Close,Volume`
- `timestamp,open,high,low,close,volume` (epoch)
- Auto-detect header and delimiter

Include sample CSV files in `tests/data/` for testing.

**Acceptance**: Load 10 years of daily AAPL data from CSV. Verify candle count, price ranges.

#### 2.4 — Binary File Format

`midas-data/src/binary.rs`:

Implement the binary format from tech-stacks.md:

```rust
#[repr(C, packed)]
pub struct BinaryHeader {
    pub magic: u32,           // 0x4D494441 ("MIDA")
    pub version: u16,         // 1
    pub symbol_id: u32,
    pub timeframe_secs: u32,
    pub start_ts: i64,
    pub end_ts: i64,
    pub candle_count: u64,
    pub flags: u16,           // bit 0: has gaps (weekends stored as sentinel)
    pub _padding: [u8; 14],   // Align header to 64 bytes
}

#[repr(C, packed)]
pub struct BinaryCandle {
    pub timestamp: i64,
    pub open: f32,
    pub high: f32,
    pub low: f32,
    pub close: f32,
    pub volume: u32,
}
// Size: 28 bytes per candle
```

Methods:
- `write_binary(path, symbol_id, timeframe, candles: &CandleBuffer) -> Result<()>`
- `read_binary(path) -> Result<CandleBuffer>` — full file read
- `mmap_binary(path) -> Result<MmapCandleFile>` — memory-mapped zero-copy access
- `MmapCandleFile::slice(start_ts, end_ts) -> CandleSlice` — O(1) time-range access

**Acceptance**: Round-trip test: write → mmap read → compare. O(1) slice access verified with timing. Files readable across runs (persistence).

#### 2.5 — Level of Detail (Downsampling)

`midas-data/src/lod.rs`:

```rust
/// Min-max downsampling for candles — preserves price envelope
pub fn downsample_minmax(candles: &CandleSlice, target_count: usize) -> CandleBuffer { ... }

/// LTTB downsampling for line data (indicators)
pub fn downsample_lttb(values: &[f32], timestamps: &[i64], target_count: usize) -> Vec<(i64, f32)> { ... }
```

Auto-LOD selection:
```rust
pub fn select_lod_level(total_candles: usize, viewport_width: u32) -> usize {
    // Target: ~2 candles per pixel maximum
    let max_useful = viewport_width as usize * 2;
    if total_candles <= max_useful { return 1; }  // No downsampling
    (total_candles + max_useful - 1) / max_useful  // Bucket size
}
```

**Acceptance**: Benchmark: downsample 100K candles to 4K in < 1ms. Visual comparison: downsampled chart looks identical to original at the same zoom level.

#### 2.6 — Data Manager

`midas-data/src/cache.rs`:

```rust
pub struct DataManager {
    data_dir: PathBuf,
    loaded: HashMap<(SymbolId, Timeframe), Arc<CandleBuffer>>,
}

impl DataManager {
    pub fn load(&mut self, symbol: SymbolId, tf: Timeframe) -> Result<Arc<CandleBuffer>> { ... }
    pub fn import_csv(&mut self, path: &Path, symbol: SymbolId, tf: Timeframe) -> Result<()> { ... }
    pub fn get_visible(&self, symbol: SymbolId, tf: Timeframe,
                       time_range: (i64, i64), viewport_width: u32) -> CandleBuffer { ... }
}
```

`get_visible` handles: slice to time range → LOD downsample if needed → return.

**Acceptance**: Loading a mmap'd binary file and slicing is < 1ms.

---

## Phase 3: Chart Interaction

> **Goal**: Smooth zoom/pan with mouse/trackpad. Crosshair with price/time readout. Horizontal levels.
> **Effort**: 2-3 weeks
> **Depends on**: Phase 1, Phase 2

### Tasks

#### 3.1 — Pan (Mouse Drag / Scroll)

- Left-click drag: Pan chart (move visible time/price range)
- Mouse wheel: Scroll horizontally through time (shift + wheel = vertical)
- Trackpad: Two-finger scroll pans in both axes

Implementation:
```rust
// In camera update logic:
pub fn pan(&mut self, dx_pixels: f32, dy_pixels: f32) {
    let dt = dx_pixels * self.time_per_pixel();
    let dp = dy_pixels * self.price_per_pixel();
    self.time_start -= dt as f64;
    self.time_end -= dt as f64;
    self.price_low += dp as f64;
    self.price_high += dp as f64;
}
```

**Acceptance**: Pan is smooth (no jitter), 60fps maintained. Pan inertia feels natural. Panning through 10 years of data is instant (LOD kicks in).

#### 3.2 — Zoom (Scroll Wheel / Pinch)

- Ctrl + mouse wheel: Zoom in/out centered on cursor position
- Trackpad pinch: Zoom
- Zoom X-axis and Y-axis independently (default: zoom time axis only, Y auto-scales)

```rust
pub fn zoom(&mut self, center_x: f32, factor: f32) {
    let center_time = self.x_to_time(center_x);
    let left_dt = (center_time - self.time_start) * factor as f64;
    let right_dt = (self.time_end - center_time) * factor as f64;
    self.time_start = center_time - left_dt;
    self.time_end = center_time + right_dt;
    // Auto-scale Y to visible data
    self.auto_scale_y();
}
```

Auto-scale Y: Compute min(low) and max(high) of visible candles, add 5% padding.

**Acceptance**: Zoom feels smooth and centered. Zoom all the way out (20 years) → LOD keeps 60fps. Zoom all the way in (individual candles fill screen) → no precision loss.

#### 3.3 — Y-Axis Auto-Scaling

On every viewport change (pan, zoom, resize, new data):
1. Find visible candle range: `binary_search(timestamps, time_start..time_end)`
2. Compute `min(lows[range])`, `max(highs[range])` (SIMD-friendly scan)
3. Add padding: `price_range * 0.05` top and bottom
4. Animate the transition smoothly (lerp over 3-5 frames)

Animated scaling prevents jarring jumps when panning:
```rust
pub fn auto_scale_y_animated(&mut self, target_low: f64, target_high: f64) {
    self.target_price_low = target_low;
    self.target_price_high = target_high;
    self.animating = true;
}

pub fn tick_animation(&mut self, dt: f32) {
    let t = (dt * 8.0).min(1.0);  // Smooth lerp, ~8x per second
    self.price_low += (self.target_price_low - self.price_low) * t as f64;
    self.price_high += (self.target_price_high - self.price_high) * t as f64;
}
```

**Acceptance**: Y-axis adapts to visible data. Smooth animation. No jitter during pan.

#### 3.4 — Crosshair

When mouse is over the chart area:
- Vertical dashed line at cursor X (snaps to nearest candle center)
- Horizontal dashed line at cursor Y
- Price label on Y-axis at cursor position
- Date/time label on X-axis at cursor position
- OHLCV tooltip near cursor showing the candle under the cursor

Render via the crosshair pipeline (thin rectangles + text).

**Acceptance**: Crosshair follows mouse with zero perceived lag. Snaps to candle centers. OHLCV data is correct.

#### 3.5 — Horizontal Price Levels

The primary drawing tool. User clicks on the chart to place a horizontal line at a specific price.

```rust
pub struct HorizontalLevel {
    pub id: u64,
    pub price: f64,
    pub color: [f32; 4],
    pub line_width: f32,
    pub line_style: LineStyle,  // Solid, Dashed, Dotted
    pub label: Option<String>,
    pub draggable: bool,
}
```

Interaction:
- Double-click on chart: Create new horizontal level at clicked price
- Click on existing level: Select it (highlight)
- Drag selected level: Move it to new price (snap to tick)
- Right-click level: Context menu (delete, change color, change style)
- Price label on Y-axis for each level

Render as full-width horizontal lines via the `hline` pipeline.

**Acceptance**: Levels persist across sessions. Dragging is smooth. Multiple levels render without performance impact.

#### 3.6 — Y-Axis & X-Axis Rendering

Reserve pixel regions for axes:
- Right edge: Y-axis (price labels) — 80px wide
- Bottom edge: X-axis (date/time labels) — 30px tall

Axis labels:
- Y-axis: Price labels at grid line positions, formatted to appropriate decimal places
- X-axis: Date/time labels at grid line positions, format adapts to timeframe (HH:MM for intraday, MM/DD for daily, etc.)

Text rendering (Phase 1 approach — simple):
- Pre-render digits 0-9, period, colon, slash, space to a texture atlas at startup
- Draw axis labels as textured quads from the atlas
- (Phase 5+: Replace with Vello/Parley for full Unicode support)

**Acceptance**: Labels are crisp and readable at all zoom levels. No overlapping labels (adaptive spacing). Correct formatting per timeframe.

---

## Phase 4: iced Application Shell

> **Goal**: Multi-chart layout, toolbar, watchlist, symbol switching.
> **Effort**: 2-3 weeks
> **Depends on**: Phase 1-3 (working single chart)

### Tasks

#### 4.1 — Application State Architecture

`midas-app/src/app.rs`:

```rust
pub struct MidasApp {
    /// All chart panels
    charts: Vec<ChartPanel>,

    /// Workspace layout (grid arrangement)
    layout: WorkspaceLayout,

    /// Shared data manager
    data_manager: DataManager,

    /// Indicator engine (shared across charts)
    indicator_engine: IndicatorEngine,

    /// Active symbol for toolbar
    active_chart: Option<ChartId>,

    /// Theme
    theme: MidasTheme,
}

pub enum Message {
    // Chart events
    ChartZoom(ChartId, f32, f32),    // (center_x, factor)
    ChartPan(ChartId, f32, f32),     // (dx, dy)
    ChartHover(ChartId, f32, f32),   // (x, y)
    ChartClick(ChartId, f32, f32),

    // Data events
    SymbolChanged(ChartId, String),
    TimeframeChanged(ChartId, Timeframe),
    DataLoaded(ChartId, Arc<CandleBuffer>),

    // Layout
    LayoutChanged(WorkspaceLayout),

    // Toolbar
    SymbolSearchInput(String),
    SymbolSearchSubmit,

    // Tick
    Tick,  // 60fps animation tick
}
```

**Acceptance**: iced update/view cycle works. State is correctly propagated.

#### 4.2 — Workspace Layout

`midas-app/src/views/workspace.rs`:

Support layout presets:
- 1 chart (full screen)
- 2 charts (side by side)
- 2 charts (stacked)
- 4 charts (2x2 grid)
- 6 charts (3x2)
- 8 charts (4x2)
- Custom: drag borders to resize panels

Each cell contains a `ChartWidget` (the iced `Shader` widget from `chart_widget.rs`).

```rust
pub enum WorkspaceLayout {
    Single,
    SplitH,    // 2 side by side
    SplitV,    // 2 stacked
    Grid2x2,
    Grid3x2,
    Grid4x2,
    Custom(Vec<LayoutCell>),
}

pub struct LayoutCell {
    pub chart_id: ChartId,
    pub x: f32, pub y: f32,       // 0..1 relative position
    pub w: f32, pub h: f32,       // 0..1 relative size
}
```

**Acceptance**: Switch between layouts smoothly. Each chart renders independently. Resizing window redistributes space proportionally.

#### 4.3 — Toolbar

`midas-app/src/views/toolbar.rs`:

Top bar containing:
- Symbol search box (type ticker → load data)
- Timeframe buttons: 1m, 5m, 15m, 1H, 4H, D, W
- Layout selector dropdown
- (Future: indicator add button, drawing tools)

**Acceptance**: Type "AAPL" → loads AAPL data into active chart. Click "5m" → switches timeframe.

#### 4.4 — Sidebar / Watchlist

`midas-app/src/views/sidebar.rs`:

Left panel (collapsible) showing:
- List of watched symbols with last price and % change
- Click symbol → loads into active chart
- Right-click → open in new chart panel

**Acceptance**: Watchlist persists across sessions. Click loads data correctly.

#### 4.5 — Chart Widget (iced ↔ wgpu Bridge)

`midas-app/src/widgets/chart_widget.rs`:

This is the critical integration point. Implement iced's `Shader` widget trait to bridge iced's layout/event system with our custom `ChartRenderer`.

```rust
pub struct ChartWidget {
    chart_id: ChartId,
}

// Implement iced::widget::shader::Program for ChartWidget
// - prepare(): convert chart state to GPU-ready data
// - render(): invoke ChartRenderer::render with wgpu encoder
// - mouse_interaction(): return appropriate cursor icon
// - update(): handle mouse/keyboard events, produce Messages
```

**Acceptance**: Chart renders correctly inside iced layout. Events (mouse, keyboard) reach the chart. Multiple charts render independently in the grid.

#### 4.6 — Config Persistence

`midas-core/src/config.rs`:

```rust
#[derive(Serialize, Deserialize)]
pub struct AppConfig {
    pub layout: WorkspaceLayout,
    pub charts: Vec<ChartConfig>,
    pub watchlist: Vec<String>,
    pub theme: ThemeConfig,
    pub data_dir: PathBuf,
}

#[derive(Serialize, Deserialize)]
pub struct ChartConfig {
    pub symbol: String,
    pub timeframe: Timeframe,
    pub indicators: Vec<IndicatorConfig>,
    pub horizontal_levels: Vec<HorizontalLevel>,
}
```

Save to `data/config.toml` on every state change (debounced, 1 save per second max).

**Acceptance**: Close and reopen app → same layout, charts, symbols, indicators, levels.

---

## Phase 5: Indicator Engine

> **Goal**: Incremental indicator computation with DAG. SMA, EMA, RSI, MACD, Bollinger Bands rendering.
> **Effort**: 3-4 weeks
> **Depends on**: Phase 2 (CandleBuffer), Phase 1 (line rendering)

### Tasks

#### 5.1 — IncrementalIndicator Trait

`midas-indicators/src/traits.rs`:

```rust
pub trait IncrementalIndicator: Send + Sync {
    /// Human-readable name
    fn name(&self) -> &str;

    /// Number of output series (e.g., MACD has 3: line, signal, histogram)
    fn output_count(&self) -> usize;

    /// Initialize from historical data. Returns outputs for each candle.
    fn initialize(&mut self, closes: &[f32]) -> Vec<Vec<f32>>;

    /// O(1) update on new closed candle. Returns output values.
    fn update(&mut self, close: f32) -> Vec<f32>;

    /// O(1) tentative update for forming candle (does not advance state).
    fn peek(&self, close: f32) -> Vec<f32>;

    /// Rendering hint: overlay on price chart or separate sub-chart?
    fn display_mode(&self) -> DisplayMode;

    /// Line colors for each output series
    fn colors(&self) -> Vec<[f32; 4]>;
}

pub enum DisplayMode {
    Overlay,     // Drawn on the price chart (SMA, EMA, Bollinger)
    SubChart,    // Drawn in a separate panel below (RSI, MACD)
}
```

**Acceptance**: Trait compiles. Mock implementation passes.

#### 5.2 — Core Indicators

Implement each as a struct implementing `IncrementalIndicator`:

| Indicator | Outputs | Display | State | Notes |
|---|---|---|---|---|
| **SMA(period)** | 1 line | Overlay | Ring buffer of last `period` closes | |
| **EMA(period)** | 1 line | Overlay | Previous EMA value | α = 2/(period+1) |
| **Bollinger(period, std_dev)** | 3 lines (upper, middle, lower) | Overlay | SMA state + running variance | |
| **VWAP** | 1 line | Overlay | Cumulative price*volume, cumulative volume | Resets daily |
| **RSI(period)** | 1 line (0-100) | SubChart | Avg gain, avg loss (Wilder smoothing) | |
| **MACD(fast, slow, signal)** | 3 (MACD line, signal, histogram) | SubChart | Two EMAs + signal EMA | |

Each indicator must implement `peek()` for forming candle tentative values without mutating state.

**Acceptance**: Unit tests verify output matches a known-good reference (e.g., TradingView's values for AAPL daily). Accuracy within f32 precision.

#### 5.3 — Indicator Engine (DAG Executor)

`midas-indicators/src/engine.rs`:

```rust
pub struct IndicatorEngine {
    /// Per-chart indicator configurations
    charts: HashMap<ChartId, Vec<Box<dyn IncrementalIndicator>>>,

    /// Computed output series per chart per indicator
    outputs: HashMap<ChartId, Vec<IndicatorOutput>>,
}

pub struct IndicatorOutput {
    pub indicator_name: String,
    pub series: Vec<Vec<f32>>,   // One Vec<f32> per output series
    pub colors: Vec<[f32; 4]>,
    pub display_mode: DisplayMode,
}

impl IndicatorEngine {
    pub fn add_indicator(&mut self, chart: ChartId, indicator: Box<dyn IncrementalIndicator>) { ... }
    pub fn remove_indicator(&mut self, chart: ChartId, name: &str) { ... }
    pub fn initialize(&mut self, chart: ChartId, data: &CandleBuffer) { ... }
    pub fn on_candle_closed(&mut self, chart: ChartId, close: f32) { ... }
    pub fn on_candle_update(&mut self, chart: ChartId, close: f32) { ... }
    pub fn get_outputs(&self, chart: ChartId) -> &[IndicatorOutput] { ... }
}
```

For Phase 5, indicators are independent (no cross-indicator dependencies). The DAG for dependent indicators (MACD's internal EMAs) is handled internally within each indicator struct.

**Acceptance**: Add SMA(20) + EMA(50) + RSI(14) to a chart. Update with new candle → all indicators update in < 10us.

#### 5.4 — Indicator Line Rendering

Extend the line pipeline to render indicator output series as colored polylines.

For overlay indicators: Transform indicator values through the price camera (same Y-axis as candles).
For sub-chart indicators: Render in a separate viewport region below the main chart (fixed Y range, e.g., 0-100 for RSI).

Sub-chart layout:
```
┌───────────────────────────────┐
│                               │
│     Main Chart (candles +     │  70% of height
│     overlay indicators)       │
│                               │
├───────────────────────────────┤
│  RSI (0-100)                  │  15% of height
├───────────────────────────────┤
│  MACD                         │  15% of height
└───────────────────────────────┘
```

**Acceptance**: SMA/EMA lines overlay on candles correctly. RSI renders in sub-chart with 30/70 reference lines. MACD histogram renders as bars.

#### 5.5 — Indicator Add/Remove UI

Toolbar button opens indicator selector:
- Search/filter by name
- Click to add to active chart
- Configurable parameters (period, etc.)
- Click indicator legend in chart to toggle visibility or remove

**Acceptance**: User can add/remove indicators. Config persists.

---

## Phase 6: Multi-Chart & Sync

> **Goal**: Synchronized time axis, crosshair sync, independent Y-axes.
> **Effort**: 2-3 weeks
> **Depends on**: Phase 4 (multi-chart layout)

### Tasks

#### 6.1 — TimeAxisController

`midas-core/src/time_axis.rs`:

```rust
pub struct TimeAxisController {
    pub time_start: f64,
    pub time_end: f64,
    pub linked_charts: Vec<ChartId>,
    pub sync_enabled: bool,
}

impl TimeAxisController {
    pub fn pan(&mut self, dt: f64) { ... }
    pub fn zoom(&mut self, center_time: f64, factor: f64) { ... }

    /// Called when any linked chart pans/zooms. Updates all others.
    pub fn on_chart_viewport_changed(&mut self, source: ChartId, new_start: f64, new_end: f64)
        -> Vec<(ChartId, f64, f64)>  // Charts to update with new time ranges
    { ... }
}
```

**Acceptance**: Pan chart A → chart B-N pan to same time range. Zoom chart A → all charts zoom.

#### 6.2 — Crosshair Sync

When mouse hovers on any chart at time T:
1. Source chart draws full crosshair (vertical + horizontal + tooltip)
2. All other linked charts draw only a vertical line at time T (no horizontal — different Y axes)
3. Each chart's Y-axis shows the price at time T for its symbol

**Acceptance**: Hover on AAPL chart → all charts show vertical line at same time.

#### 6.3 — Independent Y-Axis Scaling

Each chart auto-scales its Y-axis independently based on its own visible data. Only the X-axis (time) is synchronized.

Already implemented in Phase 3 auto-scaling — just verify it works correctly with synchronized time axes.

#### 6.4 — Link/Unlink Charts

Allow individual charts to be unlinked from the TimeAxisController:
- Linked icon in chart header (click to toggle)
- Unlinked charts pan/zoom independently
- Default: all charts linked

**Acceptance**: Unlink one chart → it pans independently while others stay synced.

---

## Phase 7: Real-Time Streaming

> **Goal**: Live data from Polygon.io WebSocket. Candles update in real-time.
> **Effort**: 3-4 weeks
> **Depends on**: Phase 2, Phase 4

### Tasks

#### 7.1 — DataProvider Trait

`midas-feed/src/provider.rs`:

```rust
#[async_trait]
pub trait DataProvider: Send + Sync {
    /// Fetch historical candles for backfill
    async fn fetch_history(&self, symbol: &str, tf: Timeframe,
                           start: i64, end: i64) -> Result<CandleBuffer>;

    /// Subscribe to real-time updates. Returns a receiver channel.
    async fn subscribe(&self, symbol: &str) -> Result<mpsc::Receiver<MarketEvent>>;

    /// Unsubscribe
    async fn unsubscribe(&self, symbol: &str) -> Result<()>;
}
```

#### 7.2 — Polygon.io Provider

`midas-feed/src/polygon.rs`:

- REST client for historical data: `GET /v2/aggs/ticker/{ticker}/range/{multiplier}/{timespan}/{from}/{to}`
- WebSocket client: `wss://socket.polygon.io/stocks`
- Subscribe to `AM.*` (minute aggregates) and `T.*` (trades) channels
- Handle authentication, reconnection, heartbeats

```rust
pub struct PolygonProvider {
    api_key: String,
    ws_connection: Option<WebSocketConnection>,
    http_client: reqwest::Client,
}
```

**Acceptance**: Connect to Polygon, subscribe to AAPL. Receive real-time minute bars during market hours. Historical backfill loads correctly.

#### 7.3 — Tick Aggregator

`midas-feed/src/aggregator.rs`:

Converts raw ticks/trades into forming candles at all active timeframes:

```rust
pub struct TickAggregator {
    forming_candles: HashMap<(SymbolId, Timeframe), FormingCandle>,
}

pub struct FormingCandle {
    pub open_time: i64,
    pub close_time: i64,
    pub open: f32,
    pub high: f32,
    pub low: f32,
    pub close: f32,
    pub volume: u32,
}

impl TickAggregator {
    /// Process a new tick. Returns any candles that closed.
    pub fn on_tick(&mut self, symbol: SymbolId, price: f32, volume: u32, ts: i64)
        -> Vec<(Timeframe, CandleEvent)>
    {
        // For each active timeframe:
        // 1. Check if tick crosses candle boundary → close current, open new
        // 2. Update forming candle (update high/low/close/volume)
        // 3. Return CandleClosed events for any closed candles
    }
}

pub enum CandleEvent {
    Updated(FormingCandle),
    Closed(FormingCandle),
}
```

**Acceptance**: Feed 1 day of AAPL ticks → produces correct 1m, 5m, 15m, 1H, D candles. Compare with Polygon's pre-aggregated bars.

#### 7.4 — Triple Buffer Integration

Connect the feed thread to the render thread via triple buffering:

```
[Feed Thread]                    [Render Thread]
  tokio runtime                    iced event loop
  ├── WebSocket recv               ├── Reads from triple buffer
  ├── TickAggregator               ├── Checks dirty flags
  ├── Update CandleBuffer          └── Re-renders dirty charts
  └── Triple buffer swap
```

Use `crossbeam::channel` for one-shot events (CandleClosed) and `triple_buffer` for continuous state.

**Acceptance**: Real-time candles appear with < 50ms latency from tick to pixel. No frame drops during streaming.

#### 7.5 — Connection Management

- Auto-reconnect on disconnect (exponential backoff)
- Connection status displayed in status bar (Connected / Reconnecting / Error)
- Graceful degradation: if feed disconnects, chart continues to display last known data
- Support multiple simultaneous subscriptions (one per displayed symbol)

**Acceptance**: Kill network → "Reconnecting..." appears → restore network → auto-reconnects and resumes.

---

## Phase 8: Polish & Production Hardening

> **Goal**: HiDPI, theming, keyboard shortcuts, performance optimization, error handling.
> **Effort**: 2-3 weeks
> **Depends on**: Phase 1-7

### Tasks

#### 8.1 — HiDPI Support

- Detect `scale_factor` from winit
- Render all GPU content at physical pixel resolution
- All coordinate snapping uses physical pixels
- Test at 1.0x (1080p), 1.5x (common Windows), 2.0x (Retina/4K)
- Handle monitor change (drag window between monitors with different DPI)

#### 8.2 — Theming

`midas-app/src/theme.rs`:

```rust
pub struct MidasTheme {
    pub background: Color,        // Chart background
    pub grid: Color,              // Grid lines
    pub text: Color,              // Axis labels, tooltips
    pub bull: Color,              // Bullish candles
    pub bear: Color,              // Bearish candles
    pub volume_bull: Color,       // Volume bars (semi-transparent)
    pub volume_bear: Color,
    pub crosshair: Color,
    pub level_default: Color,     // Horizontal levels
    pub axis_bg: Color,           // Axis background
}
```

Ship with 2 themes: Dark (default) and Light. User-configurable via config.

#### 8.3 — Keyboard Shortcuts

| Shortcut | Action |
|---|---|
| `+` / `-` | Zoom in / out |
| `Left` / `Right` | Pan left / right |
| `Home` | Jump to latest data |
| `End` | Jump to oldest data |
| `1` through `9` | Quick timeframe switch (1=1m, 2=5m, 3=15m, 4=1H, 5=4H, 6=D, 7=W) |
| `Ctrl+F` | Symbol search focus |
| `Ctrl+1` through `Ctrl+4` | Layout presets |
| `Delete` | Remove selected level |
| `Escape` | Deselect / cancel |

#### 8.4 — Performance Optimization Pass

- Profile with Tracy or `perf` to identify bottlenecks
- Ensure GPU buffer updates are minimal (dirty flagging per chart)
- Verify LOD kicks in correctly at all zoom levels
- Stress test: 20 charts x 5,000 candles, measure frame time
- Memory audit: verify no leaks (mmap files closed on chart close)

#### 8.5 — Error Handling

- All `unwrap()` / `expect()` in non-panic paths replaced with proper error handling
- Data load failures show user-friendly message in chart area
- Network errors handled gracefully (retry + status bar)
- Invalid data (NaN prices, zero timestamps) filtered and logged

#### 8.6 — Logging & Diagnostics

- Frame time overlay (toggle with `F11`)
- GPU memory usage display
- WebSocket message rate display
- Write structured logs to `data/midas.log` (rolling, max 10MB)

---

## Appendix A: WGSL Shader Specifications

### Candle Shader (`candle.wgsl`)

Two-pass approach (cleaner than single-pass):

**Pass 1 — Wicks**: Instanced draw of thin rectangles (1px physical width, full wick height).
**Pass 2 — Bodies**: Instanced draw of candle body rectangles.

Both passes share the same instance buffer but use different vertex expansion logic.

### Line Shader (`line.wgsl`)

For indicator polylines. Renders line segments as screen-space quads with configurable width. Each segment is defined by start and end points; the vertex shader computes perpendicular offset for line width.

For dashed lines: Pass a `dash_offset` uniform and discard fragments based on distance along the line.

### Text Shader (`msdf.wgsl`)

Multi-channel signed distance field text rendering:
- Sample MSDF texture with 3 channels (R, G, B encode distance fields)
- Compute median of 3 channels
- Apply smoothstep for anti-aliasing
- Threshold for glyph edge

This gives resolution-independent crisp text at any size.

---

## Appendix B: Binary File Format Specification

### File Extension: `.candles`

### Header (64 bytes, offset 0)

| Offset | Size | Type | Field | Description |
|---|---|---|---|---|
| 0 | 4 | u32 | magic | `0x4D494441` ("MIDA") |
| 4 | 2 | u16 | version | Format version (currently 1) |
| 6 | 4 | u32 | symbol_id | Internal symbol identifier |
| 10 | 4 | u32 | timeframe_secs | Candle duration in seconds |
| 14 | 8 | i64 | start_ts | First candle timestamp (epoch ms) |
| 22 | 8 | i64 | end_ts | Last candle timestamp (epoch ms) |
| 30 | 8 | u64 | candle_count | Number of candles in body |
| 38 | 2 | u16 | flags | Bit 0: dense (includes gaps as sentinels) |
| 40 | 24 | [u8] | _reserved | Zero-filled, future use |

### Body (offset 64, repeating 28-byte records)

| Offset | Size | Type | Field |
|---|---|---|---|
| 0 | 8 | i64 | timestamp (epoch ms) |
| 8 | 4 | f32 | open |
| 12 | 4 | f32 | high |
| 16 | 4 | f32 | low |
| 20 | 4 | f32 | close |
| 24 | 4 | u32 | volume |

### Sentinel Values (for gaps in dense mode)

A candle with `volume == 0` and `open == high == low == close == previous_close` indicates a non-trading period. The renderer skips these candles.

### O(1) Random Access (dense mode only)

```
record_offset = 64 + ((target_ts - start_ts) / (timeframe_secs * 1000)) * 28
```

---

## Appendix C: Risk Register

| Risk | Impact | Likelihood | Mitigation |
|---|---|---|---|
| **iced Shader widget API changes** | High — core integration point | Medium (iced pre-1.0) | Pin iced version. Abstract behind our own trait. Monitor iced releases. |
| **Vello alpha instability** | Medium — affects overlays/text | Medium | Defer Vello to Phase 5+. Use MSDF text atlas as primary text renderer. Vello is optional enhancement. |
| **wgpu breaking changes** | Medium — rebuild pipelines | Medium (every 2-3 months) | Pin version. Update between phases, not mid-phase. |
| **Polygon.io API changes/downtime** | Medium — no live data | Low | DataProvider trait abstracts provider. CSV fallback for dev. Add Databento/IBKR as alternatives. |
| **Performance bottleneck in iced** | High — affects all charts | Low (Kraken validated) | Profile early. If iced is the bottleneck, bypass it for chart rendering (own window). |
| **HiDPI edge cases (mixed monitors)** | Low — visual glitch | Medium (Windows) | Test on multi-monitor early. Handle `ScaleFactorChanged` events. |
| **Data format evolution** | Medium — break saved files | Medium | Version field in header. Write migration code when format changes. |

---

## Phase Dependency Graph

```
Phase 0 (Scaffold)
  │
  ├──→ Phase 1 (GPU Rendering) ──→ Phase 3 (Interaction) ──→ Phase 6 (Multi-Chart)
  │                                       │
  │                                       └──→ Phase 8 (Polish)
  │
  ├──→ Phase 2 (Data Layer) ──→ Phase 5 (Indicators)
  │         │
  │         └──→ Phase 7 (Streaming)
  │
  └──→ Phase 4 (iced Shell) ──→ Phase 6 (Multi-Chart)
                │
                └──→ Phase 8 (Polish)
```

**Parallel work opportunities:**
- Phase 1 and Phase 2 can be developed in parallel
- Phase 4 can start as soon as Phase 0 is done (uses placeholder chart)
- Phase 5 only needs Phase 2's `CandleBuffer` and Phase 1's line pipeline
- Phase 7 only needs Phase 2's data types and storage

**Critical path**: Phase 0 → Phase 1 → Phase 3 → Phase 4 → Phase 6 → Phase 8

---

## Milestone Checkpoints

| Milestone | Phase | Demo |
|---|---|---|
| **M0: Window** | 0 | iced window opens with wgpu clear |
| **M1: Static Chart** | 1+2 | 5K candles rendered from CSV, pixel-perfect |
| **M2: Interactive Chart** | 3 | Zoom, pan, crosshair, horizontal levels |
| **M3: Multi-Chart App** | 4 | 4 charts in grid, toolbar, symbol switching |
| **M4: Indicators** | 5 | SMA, EMA, RSI, MACD rendering correctly |
| **M5: Synced Charts** | 6 | 8+ synced charts, crosshair sync |
| **M6: Live Data** | 7 | Real-time streaming from Polygon.io |
| **M7: Production Alpha** | 8 | Polish, HiDPI, themes, keyboard shortcuts |
