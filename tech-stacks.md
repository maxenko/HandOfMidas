# Tech Stack Analysis: Custom High-Performance Stock Charting Platform

> Research compiled March 2026. Covers rendering engines, GUI frameworks, data architecture, and a final recommended stack for building a from-scratch charting solution rivaling TC2000's precision with full pipeline ownership.

---

## Table of Contents

- [Executive Summary](#executive-summary)
- [Requirements Recap](#requirements-recap)
- [Part 1: Rendering Engines](#part-1-rendering-engines)
  - [GPU Rendering Libraries (Rust)](#gpu-rendering-libraries-rust)
  - [2D Rendering Frameworks (Rust)](#2d-rendering-frameworks-rust)
  - [Skia (C++ with Rust Bindings)](#skia-c-with-rust-bindings)
  - [WebGL / WebGPU / Canvas (Web-Based)](#webgl--webgpu--canvas-web-based)
  - [Direct GPU (C++ / OpenGL / Vulkan / DirectX)](#direct-gpu-c--opengl--vulkan--directx)
  - [Rendering Engine Verdict](#rendering-engine-verdict)
- [Part 2: GUI / Application Frameworks](#part-2-gui--application-frameworks)
  - [Rust-Native GUI Frameworks](#rust-native-gui-frameworks)
  - [Hybrid Approaches (Tauri + WebView)](#hybrid-approaches-tauri--webview)
  - [GUI Framework Verdict](#gui-framework-verdict)
- [Part 3: Data Architecture](#part-3-data-architecture)
  - [Storage Layer](#storage-layer)
  - [Memory Layout for Rendering](#memory-layout-for-rendering)
  - [Multi-Resolution / Level of Detail](#multi-resolution--level-of-detail)
  - [Real-Time Streaming Architecture](#real-time-streaming-architecture)
  - [Multi-Chart Synchronization](#multi-chart-synchronization)
  - [Indicator Computation](#indicator-computation)
- [Part 4: Competitive Analysis](#part-4-competitive-analysis)
  - [What Makes TC2000 Crisp](#what-makes-tc2000-crisp)
  - [TradingView's Architecture](#tradingviews-architecture)
  - [Kraken Desktop (Rust/iced)](#kraken-desktop-rusticed)
- [Part 5: Language Alternatives](#part-5-language-alternatives)
- [Part 6: Final Recommended Stack](#part-6-final-recommended-stack)
  - [Primary Recommendation: Pure Rust Native](#primary-recommendation-pure-rust-native)
  - [Architecture Diagram](#architecture-diagram)
  - [Performance Budget](#performance-budget)
  - [Development Roadmap Estimate](#development-roadmap-estimate)
  - [Alternative Stacks Considered](#alternative-stacks-considered)

---

## Executive Summary

After extensive research across Rust-native, C++, and web-based approaches, the recommended stack is:

| Layer | Technology | Why |
|---|---|---|
| **Language** | Rust | Zero-cost abstractions, no GC pauses, SIMD support, memory safety, excellent GPU ecosystem |
| **GPU Abstraction** | wgpu (v29+) | Production-ready, cross-platform (Vulkan/Metal/DX12/WebGPU), used by Firefox & Zed |
| **2D Rendering** | Custom wgpu instanced shaders (candles) + Vello (overlays/text) | Maximum performance for chart primitives; Vello for antialiased paths and annotations |
| **GUI Shell** | iced | Elm architecture, production-proven at Kraken, `Shader` widget for custom GPU rendering |
| **Data Storage** | Memory-mapped custom binary files (hot) + QuestDB (durable) | O(1) time-range access, zero-copy reads |
| **Memory Layout** | Structure of Arrays (SoA) with f32 prices | 3-6x better cache utilization for rendering scans |
| **Streaming** | Triple-buffered lock-free (crossbeam) + tokio-tungstenite | Zero contention between ingest and render threads |
| **Indicators** | CPU + SIMD, incremental O(1) DAG-based updates | Sequential recurrences are not GPU-friendly |

**Key insight from research**: Rendering 50,000 candles at 60fps is **trivially easy** for any GPU approach. The hard problems are pixel-perfect crispness, multi-chart system architecture, smooth interaction, and efficient real-time data flow. TC2000's visual quality comes from pixel-level attention to detail (alignment, AA control, font hinting), not exotic rendering technology.

---

## Requirements Recap

| Requirement | Priority |
|---|---|
| Pixel-perfect crispness (TC2000-level) | Critical |
| Thousands of candles per chart at 60fps | Critical |
| Multiple charts simultaneously (20+) | Critical |
| Zoom, pan, scale — smooth at all levels | Critical |
| Horizontal levels (with flexibility for more) | Critical |
| Multiple real-time data streams | High |
| Full ownership of rendering pipeline | High |
| Customizable / extensible for future needs | High |
| Cross-platform (Windows primary, macOS/Linux secondary) | Medium |
| Not locked into someone else's charting solution | Critical |

---

## Part 1: Rendering Engines

### GPU Rendering Libraries (Rust)

| Library | Version | Backend | Maturity | Suitability |
|---|---|---|---|---|
| **wgpu** | v29.0.0 (Mar 2025) | Vulkan, Metal, DX12, WebGPU, WebGL2 | **Production** (Firefox, Zed) | **Excellent** — the standard choice |
| vulkano | v0.34 | Vulkan only | Mature but narrower | Overkill — Vulkan-only locks you out of macOS Metal |
| ash | v0.38 | Vulkan only (raw bindings) | Mature | Too low-level — raw Vulkan FFI, months of boilerplate |
| glow | v0.16 | OpenGL ES 3.0 | Mature | Legacy — no compute shaders, no modern GPU features |

**Verdict: wgpu is the clear winner.** It's the Rust ecosystem's standard GPU abstraction, used by Firefox's WebGPU implementation and Zed editor (120fps UI). It supports all major backends from a single API. Instanced rendering of 50,000 candles = 1-2 draw calls, trivial GPU workload.

**wgpu gotchas:**
- Breaking API changes every 2-3 months — pin your dependency version
- WebGL2 fallback has rough edges on some browsers
- Keep all GPU work on one thread (multi-threading has reported issues)
- WGSL shader tooling is less mature than GLSL/HLSL

### 2D Rendering Frameworks (Rust)

| Framework | Version | Rendering | GPU? | Performance | Charting Fit |
|---|---|---|---|---|---|
| **Vello** | v0.6.0 | GPU compute shaders (wgpu) | Yes — fully GPU | **Up to 100x faster than SkiaSharp** in GPU workloads; 120fps with tens of thousands of animated paths | **Very High** |
| lyon | v1.0.1 | Tessellation library (outputs triangles) | Depends on consumer | Good | Utility, not a renderer |
| femtovg | v0.9 | OpenGL ES | Yes (limited) | Good for moderate complexity | OpenGL-only limits it |
| tiny-skia | v0.11 | CPU rasterization (Skia-like API) | No | 20-100% slower than Skia on x86-64 | CPU-only, not for interactive charting |
| Piet | — | Abstraction layer | Backend-dependent | Varies | Maintenance mode, not recommended |

**Vello deep dive:**
Vello is the most technically ambitious 2D renderer in the Rust ecosystem. Its breakthrough: using **GPU compute shaders** for the entire rendering pipeline including path rasterization, via prefix-sum algorithms. Three implementations exist:
- **Vello GPU**: Fully GPU compute-based — highest performance
- **Vello CPU**: Competitive with Skia/Cairo in benchmarks
- **Vello Hybrid**: GPU compositing + SIMD CPU geometry

Vello is backed by the Linebender group (Google Fonts funding). It's used by Xilem, Servo (canvas rendering), and Bevy (via bevy_vello). It's in **alpha state** — API breakage between versions is expected — but the rendering quality and performance are exceptional.

**For financial charts specifically:** Vello excels at antialiased lines, fills, text (via Parley), and annotations. However, for the core candlestick rendering (thousands of axis-aligned rectangles), a custom wgpu instanced pipeline will outperform any general-purpose path renderer because you can skip the tessellation/rasterization overhead entirely.

**Recommendation: Use both.** Custom wgpu shaders for candlesticks/wicks/volume bars (the hot path) + Vello for indicators, annotations, overlays, and text (where path quality matters).

### Skia (C++ with Rust Bindings)

Skia is Google's 2D rendering library powering Chrome, Android, and Flutter. The `skia-safe` crate provides idiomatic Rust bindings.

**Strengths:**
- Battle-tested in Chrome rendering billions of web pages
- GPU-accelerated (Ganesh backend, being succeeded by Graphite)
- World-class text rendering (with HarfBuzz shaping)
- Production-grade anti-aliasing
- 50,000 candles at 60fps with GPU backend

**Weaknesses:**
- Large C++ dependency (~2GB build, though pre-built binaries available)
- Adds C++ toolchain complexity to a Rust project
- Slightly lower performance ceiling than custom wgpu (abstraction cost)
- Skia's new Graphite backend is still maturing

**Verdict:** Skia is arguably the safest, most proven choice for production-quality 2D rendering. If Vello's alpha status is too risky, Skia via `skia-safe` is the fallback. However, the C++ build dependency adds friction.

### WebGL / WebGPU / Canvas (Web-Based)

| Approach | Performance (50K candles) | Ownership | Precision | Notes |
|---|---|---|---|---|
| **SciChart.js** | 60fps (WASM + WebGL2) | Closed-source, $2-4K/dev/year | High | Best commercial option, but vendor lock-in |
| **PixiJS** | 60fps (WebGL batching) | Full ownership | Good | 2D WebGL engine; you build everything on top |
| **regl** | 60fps (functional WebGL) | Full ownership | Good | Minimal abstraction, maximum shader control |
| **Lightweight Charts (TradingView)** | ~10K candles OK | Open source (Apache 2.0) | Good | Canvas 2D only (CPU-bound), not WebGL |
| **Raw WebGL2** | 60fps | Full ownership | Maximum | Zero dependencies, maximum effort |
| **WebGPU** | 60fps + compute shaders | Full ownership | Maximum | Chrome/Edge + Safari production-ready; Firefox behind flag |

**Canvas 2D precision tricks:**
- Draw 1px lines at half-pixel offsets (x + 0.5) for crisp rendering
- Set canvas dimensions to `width * devicePixelRatio` for HiDPI
- Layer canvases: static grid, chart data, crosshair (avoid redrawing expensive layers)
- Use OffscreenCanvas + Web Workers to free main thread

**The hybrid Canvas + WebGL recommendation** (if going web-based):
```
Layer 0 (bottom): WebGL canvas — chart data (candles, lines, fills, indicators)
Layer 1 (top):    Canvas 2D    — text (axis labels, legends), gridlines, crosshair
```
This gives GPU performance for data + pixel-perfect text via the browser's native renderer.

**Verdict:** Web-based approaches are viable but add browser overhead (GC pauses, layout recalc, input latency). For TC2000-level crispness and multi-chart performance, native Rust rendering is superior.

### Direct GPU (C++ / OpenGL / Vulkan / DirectX)

| Approach | 50K Candles | Dev Effort | Quality | Cross-Platform |
|---|---|---|---|---|
| Raw OpenGL 4.x + instancing | 60fps (trivial — can do 500K+ quads) | Very High | Maximum control | All platforms |
| Direct2D + DirectWrite (Windows) | 60fps | Moderate | Excellent native Windows rendering | Windows only |
| Vulkan | 60fps | Extreme | Maximum | All except older macOS |
| DirectX 12 | 60fps | Very High | Maximum | Windows only |
| Qt + Custom QOpenGLWidget | 60fps | High | Excellent | All platforms |
| Dear ImGui + ImPlot | 60fps with LOD | Low | Good (not pixel-perfect by default) | All platforms |

**Qt Charts specifically** is not viable — it uses CPU-rasterized QPainter and drops below 15fps at 10,000 candles. Custom OpenGL in Qt works but adds the Qt dependency.

**ImGui + ImPlot** is great for prototyping (fastest path to a working chart) but the immediate-mode architecture means CPU-side geometry regeneration every frame, which bottlenecks at ~50K candles without LOD. Not pixel-perfect by default.

### Rendering Engine Verdict

**For a production charting platform that needs TC2000 crispness:**

1. **Best overall**: Custom wgpu instanced shaders (candles) + Vello (overlays/text) — maximum performance with high-quality path/text rendering
2. **Safest proven choice**: Skia via `skia-safe` — Chrome-level quality, battle-tested, but C++ build dependency
3. **Fastest to prototype**: egui with `Shape::Callback` for custom wgpu chart rendering
4. **If going web**: Tauri + Rust backend + WebGL2 (via regl or PixiJS) with Canvas 2D text overlay

---

## Part 2: GUI / Application Frameworks

### Rust-Native GUI Frameworks

| Framework | Stars | Architecture | Custom GPU Rendering | Maturity | Production Use |
|---|---|---|---|---|---|
| **iced** | 25K | Elm (MVU) | `Shader` widget — full wgpu access | **High** | **Kraken Desktop** (trading platform) |
| **egui** | 27K | Immediate mode | `Shape::Callback` — inject custom wgpu passes | **High** | Rerun, many tools |
| **Makepad** | ~5K | GPU-instanced, live DSL | Native — entire framework is GPU-first | **1.0** (thin docs) | Limited |
| **Slint** | 18K | Declarative, compiled | Limited custom rendering | High | Embedded/industrial |
| Dioxus | 24K | React-like | WebView-based | Medium | Web-focused |
| Xilem | ~3K | Elm-like, Vello-backed | Canvas widget (new) | **Alpha** | None |
| Floem | ~2K | Reactive, optional Vello | Custom rendering possible | Pre-1.0 | Lapce editor |

#### iced (Recommended)

iced uses an Elm-inspired architecture with a clear `Model → Message → Update → View` cycle. It's built on wgpu and winit.

**Why iced for charting:**
- **`Shader` widget**: Gives you a raw wgpu rendering surface within the iced layout. You write your own vertex/fragment shaders, manage your own buffers. The chart is a `Shader` widget; toolbars, watchlists, and panels are standard iced widgets.
- **Proven at scale**: Kraken (the crypto exchange) built their entire desktop trading platform in iced. This is the closest real-world validation for financial charting.
- **Structured state management**: The Elm architecture maps well to chart state (zoom level, visible range, selected symbol, indicator config).
- **Multi-window support**: iced supports multiple windows — useful for multi-monitor chart layouts.

#### egui (Strong Alternative)

egui is an immediate-mode GUI (re-renders every frame). It's simpler to get started with and has a larger community.

**Why egui could work:**
- **`Shape::Callback`**: Lets you inject a custom wgpu render pass into any egui area. Your chart rendering bypasses egui's CPU tessellator entirely.
- **Massive ecosystem**: Most examples, tutorials, and community support of any Rust GUI.
- **Fastest prototyping**: You can have a working chart UI in days.

**Why iced over egui:**
- Retained-mode is more efficient for complex layouts (egui re-tessellates UI every frame)
- Better structured state management for complex applications
- Kraken's production validation specifically for trading

#### Makepad (High Potential, Higher Risk)

Makepad 1.0 reached release in May 2025. Its architecture is **uniquely suited** for financial charting:

- **Instanced-array GPU drawing**: Widgets batch into single draw calls. Drawing 10,000 candles = one draw call with one instance array. Hover/update modifies the array directly without rebuilding the UI.
- **Custom shader support**: Any Shadertoy-compatible shader runs inside a widget. You could write a pixel-perfect candlestick shader.
- **Live styling**: UI changes reflect instantly without recompilation — superb for iterating on chart aesthetics.

**The risk**: Documentation is described as "a macro DSL with no documentation." The community is small. You'd be largely on your own for complex problems. The DSL is a custom language that differs from standard layout models.

### Hybrid Approaches (Tauri + WebView)

```
┌──────────────────────────────────────────┐
│  Tauri v2 Process                        │
│  ┌────────────────────────────────────┐  │
│  │  Rust Backend                      │  │
│  │  - Data fetching (WebSocket)       │  │
│  │  - Aggregation, indicators, LOD    │  │
│  └───────────┬────────────────────────┘  │
│              │ IPC (~0.1-0.5ms, binary)  │
│  ┌───────────▼────────────────────────┐  │
│  │  WebView (system browser engine)   │  │
│  │  - WebGL2/WebGPU chart rendering   │  │
│  │  - DOM for non-chart UI            │  │
│  └────────────────────────────────────┘  │
└──────────────────────────────────────────┘
```

**Performance**: WebGL rendering in WebView is GPU-accelerated, identical to a standalone browser. IPC overhead: ~0.1-0.5ms for small payloads, ~1-3ms for 2MB data transfer. Tauri uses ~30-80MB RAM vs Electron's ~150-300MB.

**Verdict**: Viable but adds IPC latency and browser overhead that native Rust avoids. Best if you want to ship fast and iterate with web technologies.

### GUI Framework Verdict

| Approach | Best For | Tradeoff |
|---|---|---|
| **iced + custom wgpu Shader widget** | Production trading platform | Steeper learning curve, slower to prototype |
| **egui + Shape::Callback** | Rapid prototyping, internal tools | Immediate-mode overhead for complex layouts |
| **Makepad** | Maximum GPU performance, live iteration | Poor docs, small community, high risk |
| **Tauri + WebGL** | Fast shipping, web-native devs | IPC overhead, browser limitations |

**Primary recommendation: iced** — production-validated for financial trading (Kraken), structured architecture, full wgpu access via Shader widget.

---

## Part 3: Data Architecture

### Storage Layer

#### Tiered Storage Architecture

| Tier | Purpose | Technology | Access Pattern |
|---|---|---|---|
| **L1 — In-memory** | Current session ticks + forming candles | Lock-free ring buffer (crossbeam) | Real-time write, render-thread read |
| **L2 — Memory-mapped files** | Pre-aggregated OHLCV per symbol/timeframe | Custom binary format with mmap | O(1) random access, renderer hot path |
| **L3 — Time-series DB** | Durable historical tick storage, ad-hoc queries | QuestDB (column-oriented, zero-GC) | Backfill, research, indicator backtesting |
| **L4 — Archive** | Long-term raw tick data | Compressed flat files / object storage | Rarely accessed |

#### Custom Binary File Format (L2 — Renderer Hot Path)

```
File: AAPL_1m.candles
Header (64 bytes):
  magic: u32            // Format identifier
  version: u16          // Format version
  symbol_id: u32        // Internal symbol ID
  timeframe_secs: u32   // Candle duration in seconds
  start_ts: i64         // First candle timestamp (epoch ms)
  end_ts: i64           // Last candle timestamp (epoch ms)
  candle_count: u64     // Number of candles
  flags: u16            // Compression, adjustment flags
  padding: [u8; 14]     // Align to 64 bytes

Body: [OhlcvCandle; candle_count]  // Packed array, 48 bytes each
```

**Why fixed stride matters**: Any timestamp → byte offset in O(1):
```
offset = header_size + ((target_ts - start_ts) / timeframe_secs) * candle_size
```

No index lookup, no B-tree traversal, no deserialization. The OS page cache handles caching via `mmap`.

**Storage cost** (all standard timeframes, one symbol, one year): ~6 MB. For 5,000 US equities: ~30 GB/year — fits on a single NVMe.

#### Time-Series Database Comparison

| Database | Query Speed | Write Speed | Best For |
|---|---|---|---|
| **kdb+/q** | Exceptional (industry gold standard) | Excellent | If budget allows ($$$, proprietary) |
| **QuestDB** | Excellent (designed for time-range scans) | Very high | **Best open-source for this use case** |
| **TimescaleDB** | Good (PostgreSQL maturity) | Good | If you need SQL JOINs with fundamentals |
| **InfluxDB** | Moderate | Good | DevOps metrics (overkill for OHLCV) |

### Memory Layout for Rendering

#### Structure of Arrays (SoA) — Recommended

```rust
struct CandlesSoA {
    timestamps: Vec<i64>,
    opens:  Vec<f32>,
    highs:  Vec<f32>,
    lows:   Vec<f32>,
    closes: Vec<f32>,
    volumes: Vec<u32>,
}
```

**Why SoA over AoS (Array of Structures):**

Charting operations access one or two fields across many candles:
- Drawing bodies: sequential scan of `opens[]` and `closes[]`
- Drawing wicks: sequential scan of `highs[]` and `lows[]`
- Computing SMA: sequential scan of `closes[]`

| Layout | Cache Utilization (SMA over 10K candles) | Performance |
|---|---|---|
| AoS | ~17% (strides across 24B of unneeded data per candle) | Baseline |
| **SoA** | **100% (contiguous f32 array)** | **3-6x faster** |

SoA also enables SIMD vectorization — AVX2 processes 8 candles per cycle for min/max/sum operations:

```rust
// With SoA + AVX2: find min low across visible candles
// Processes 8 f32 values per iteration
use std::simd::f32x8;
fn find_min(lows: &[f32]) -> f32 {
    lows.chunks_exact(8)
        .fold(f32x8::splat(f32::MAX), |acc, chunk| acc.simd_min(f32x8::from_slice(chunk)))
        .reduce_min()
}
```

### Multi-Resolution / Level of Detail

#### Pre-Aggregation Strategy

Pre-compute all standard timeframes from a 1-minute base:

```
Raw Ticks → 1s → 1m (base) → [5m, 15m, 1h, 4h, 1D, 1W, 1M]
```

| Timeframe | Candles/Year | Storage |
|---|---|---|
| 1m | ~98,280 | ~4.7 MB |
| 5m | ~19,656 | ~943 KB |
| 15m | ~6,552 | ~314 KB |
| 1h | ~1,638 | ~79 KB |
| 1D | ~252 | ~12 KB |

#### Dynamic Downsampling (Min-Max Bucketing)

When zoomed out with thousands of candles mapping to the same pixel columns:

```rust
fn downsample(candles: &[Candle], target_count: usize) -> Vec<Candle> {
    let bucket_size = candles.len() / target_count;
    candles.chunks(bucket_size).map(|bucket| Candle {
        timestamp: bucket[0].timestamp,
        open:  bucket[0].open,                              // first open
        high:  bucket.iter().map(|c| c.high).max(),         // true high
        low:   bucket.iter().map(|c| c.low).min(),          // true low
        close: bucket.last().unwrap().close,                // last close
        volume: bucket.iter().map(|c| c.volume).sum(),      // total volume
    }).collect()
}
```

This produces "super-candles" that preserve the price envelope — visually identical to pre-aggregated higher timeframes. For a 4K display, you never need more than ~4,000 candles in the render buffer.

**For line indicators** (moving averages, RSI): Use LTTB (Largest Triangle Three Buckets) — better at preserving visual shape for single-valued series. MinMaxLTTB variant is up to 100x faster than standard LTTB.

### Real-Time Streaming Architecture

#### Triple-Buffered Lock-Free Design

```
[Ingest Thread]              [Ready Buffer]           [Render Thread]
  writes to ──→ Write Buffer ──atomic swap──→ Ready ──atomic swap──→ Read Buffer
                                                                      reads from
```

- Ingest thread writes tick data and updates forming candles
- Atomic pointer swap when consistent state is ready
- Render thread swaps ready → read at frame start, reads exclusively from read buffer
- **Zero locks, zero contention** — only `AtomicPtr::swap` synchronization

In Rust, use the `triple_buffer` crate or implement with `crossbeam`.

#### Feed Handler Pipeline

```
WebSocket (tokio-tungstenite)
  → Deserialize (binary preferred over JSON — 5x smaller)
  → Tick Aggregator (updates forming candles at ALL timeframes simultaneously)
  → Append finalized candles to mmap'd files
  → Triple Buffer Swap
  → Emit events via crossbeam::channel
```

#### Event Types

```rust
enum MarketEvent {
    Tick { symbol: SymbolId, price: f32, size: u32, ts: i64 },
    CandleUpdate { symbol: SymbolId, timeframe: Timeframe, candle: Candle },
    CandleClosed { symbol: SymbolId, timeframe: Timeframe, candle: Candle },
}
```

### Multi-Chart Synchronization

#### Centralized TimeAxisController (Event-Driven)

```
┌──────────────────────────────────────┐
│        TimeAxisController            │
│  visible_range: (start_ts, end_ts)   │
│  on_pan(delta) → recalc → notify     │
│  on_zoom(center, factor) → notify    │
└──────┬───────────────────────────────┘
       │ emits TimeRangeChanged
       ├──→ Chart A (AAPL)  — fetches own data slice, computes own Y-axis, re-renders
       ├──→ Chart B (SPY)   — same
       └──→ Chart C (RSI)   — same
```

Each chart subscribes to time range changes. Notification = sub-microsecond (just sets a dirty flag and a byte offset into mmap'd data). No polling.

**Crosshair sync**: When user hovers on Chart A at time T, emit `CrosshairMoved(T)` → all charts draw vertical line at T and show their respective values.

### Indicator Computation

#### CPU + SIMD, Not GPU

Most technical indicators are **sequential recurrences** (each output depends on the previous):
```
EMA[i] = α * price[i] + (1 - α) * EMA[i-1]
```
This is inherently serial — GPUs cannot parallelize it. Stick with CPU + SIMD.

#### Incremental O(1) Updates

Never recompute from scratch. Each indicator maintains state and updates in O(1) per new candle:

```rust
trait IncrementalIndicator {
    fn initialize(&mut self, historical: &[f32]);   // O(n) once
    fn update(&mut self, new_value: f32) -> f32;     // O(1) per candle close
    fn update_last(&mut self, value: f32) -> f32;    // O(1) for forming candle (tentative)
}
```

**`update_last` pattern**: While a candle is forming, its close changes every tick. The indicator must update tentatively without advancing internal state. Maintain a snapshot at the last closed candle, recompute from snapshot for each forming-candle update.

#### Dependency DAG

Complex indicators form a directed acyclic graph:
```
Close → EMA(12) → MACD Line → Signal Line (EMA of MACD)
      → EMA(26) ↗            → MACD Histogram
```
Traverse in topological order on each candle close. Each node is O(1) → entire DAG update is microseconds.

---

## Part 4: Competitive Analysis

### What Makes TC2000 Crisp

TC2000 (Worden Brothers) is a Windows-first desktop app. The current versions (v20/v21) use web-based architecture (likely Chromium/Electron), but earlier versions were pure native.

**The crispness comes from implementation details, not exotic technology:**

1. **Pixel-aligned rendering**: Candle bodies and wicks snap to exact pixel boundaries. No sub-pixel blur.
2. **Controlled anti-aliasing**: 1px lines with gamma-correct AA, not blurry default AA.
3. **High-DPI awareness**: Renders at native resolution, not upscaled from lower-res buffer.
4. **Minimal overdraw**: Charts drawn in a single pass, minimal layering/compositing.
5. **Color precision**: Carefully chosen colors with good contrast on light and dark backgrounds.
6. **Font hinting**: Crisp small fonts using platform-native font renderer.

**Key takeaway**: You can achieve TC2000 quality with wgpu, Skia, Direct2D, or even Canvas. The secret is attention to pixel-level detail.

### TradingView's Architecture

- **Rendering**: HTML5 Canvas 2D (`CanvasRenderingContext2D`) — NOT WebGL
- **Open-source Lightweight Charts**: Pure Canvas 2D, handles ~200-500 visible bars well
- **Production platform optimizations**: Bitmap caching, incremental rendering, Web Workers, OffscreenCanvas
- Handles ~10,000 visible candles with aggressive viewport culling

**Why some traders prefer TC2000**: Canvas pixel snapping is less precise, browser GC causes frame drops, ~1-2 frames input latency vs native.

### Kraken Desktop (Rust/iced)

Kraken built their desktop trading platform entirely in Rust using iced. This is the **strongest production validation** for the iced + custom rendering approach for financial applications. They use iced for the application shell with custom rendering for data-dense areas.

---

## Part 5: Language Alternatives

| Language | Ecosystem | Performance | Risk |
|---|---|---|---|
| **Rust** | Excellent (wgpu, iced, Vello, egui, crossbeam, tokio) | Maximum | Steep learning curve |
| C++ | Most mature (Qt, Skia, OpenGL, ImGui) | Maximum | Memory safety issues, build complexity |
| Zig | Immature GUI ecosystem (Capy, Mach, DVUI) | Comparable to Rust | Pre-1.0 language, tiny community |
| C# / WPF | SciChart WPF (DirectX 11), mature Windows UI | Good | Windows-only, GC pauses |
| TypeScript (Tauri) | Web rendering (WebGL/WebGPU) + Rust backend | Good | Browser overhead, IPC latency |

**Verdict**: Rust is the right choice. The GPU ecosystem (wgpu, Vello), GUI frameworks (iced, egui), and data handling (crossbeam, tokio, mmap) are all best-in-class. The borrow checker eliminates entire categories of concurrency bugs in the streaming/rendering pipeline.

Zig was evaluated as a potential alternative due to simpler syntax and near-instant compile times, but the GUI/graphics ecosystem is years behind Rust.

---

## Part 6: Final Recommended Stack

### Primary Recommendation: Pure Rust Native

```
┌─────────────────────────────────────────────────────────────────────┐
│                        APPLICATION LAYER                            │
│                                                                     │
│  iced (Elm architecture)                                            │
│  ├── Standard widgets: Toolbars, Panels, Watchlists, Settings       │
│  ├── Shader widget: Custom wgpu rendering surface for each chart    │
│  └── Multi-window support for multi-monitor layouts                 │
│                                                                     │
│  Per Chart (inside Shader widget):                                  │
│  ├── Custom wgpu instanced pipeline: Candlesticks, volume bars      │
│  ├── Vello (on wgpu): Indicator lines, annotations, overlays        │
│  ├── wgpu text rendering (MSDF atlas or Vello/Parley): Axis labels  │
│  └── Horizontal levels, crosshair, selection: Custom wgpu drawing   │
│                                                                     │
│  TimeAxisController: Centralized zoom/pan/sync across all charts    │
│  IndicatorEngine: DAG of incremental O(1) indicators (CPU+SIMD)    │
└─────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────┐
│                        DATA LAYER                                   │
│                                                                     │
│  Ingest Thread (tokio runtime):                                     │
│  ├── tokio-tungstenite: WebSocket connections to data providers      │
│  ├── Binary deserialization (no JSON in hot path)                   │
│  ├── Tick Aggregator: Forms candles at all timeframes simultaneously │
│  ├── Appends finalized candles to mmap'd binary files              │
│  └── Triple Buffer Swap → render thread                            │
│                                                                     │
│  Storage:                                                           │
│  ├── L1: In-memory ring buffers (crossbeam, lock-free)             │
│  ├── L2: Memory-mapped custom binary files (SoA layout)            │
│  ├── L3: QuestDB (durable tick storage, backfill queries)          │
│  └── LOD: MinMaxLTTB downsampling for zoom-dependent resolution    │
└─────────────────────────────────────────────────────────────────────┘
```

### Architecture Diagram

```
                    ┌──────────────────────────┐
                    │   Market Data Providers   │
                    │   (Polygon, Databento,    │
                    │    IBKR, etc.)            │
                    └────────────┬─────────────┘
                                 │ WebSocket / Binary
                    ┌────────────▼─────────────┐
                    │   Ingest Thread (tokio)   │
                    │   ┌─────────────────────┐ │
                    │   │  Tick Aggregator     │ │
                    │   │  (all timeframes)    │ │
                    │   └──────────┬──────────┘ │
                    │              │             │
                    │   ┌──────────▼──────────┐ │
                    │   │  mmap'd binary files │ │
                    │   │  (append on close)   │ │
                    │   └─────────────────────┘ │
                    └────────────┬─────────────┘
                                 │ Triple Buffer (atomic swap)
                    ┌────────────▼─────────────┐
                    │    Application Core       │
                    │                           │
                    │  ┌───────────────────┐    │
                    │  │ TimeAxisController │    │
                    │  │ (sync all charts)  │    │
                    │  └─────┬─────────────┘    │
                    │        │ event-driven      │
                    │  ┌─────▼─────────────┐    │
                    │  │ IndicatorEngine    │    │
                    │  │ (DAG, incremental) │    │
                    │  └───────────────────┘    │
                    └────────────┬─────────────┘
                                 │
              ┌──────────────────┼──────────────────┐
              │                  │                   │
     ┌────────▼───────┐ ┌───────▼────────┐ ┌───────▼────────┐
     │  Chart Panel 1  │ │  Chart Panel 2  │ │  Chart Panel N  │
     │  ┌────────────┐ │ │  ┌────────────┐ │ │  ┌────────────┐ │
     │  │wgpu shader │ │ │  │wgpu shader │ │ │  │wgpu shader │ │
     │  │(candles)   │ │ │  │(candles)   │ │ │  │(candles)   │ │
     │  ├────────────┤ │ │  ├────────────┤ │ │  ├────────────┤ │
     │  │Vello       │ │ │  │Vello       │ │ │  │Vello       │ │
     │  │(indicators)│ │ │  │(indicators)│ │ │  │(indicators)│ │
     │  └────────────┘ │ │  └────────────┘ │ │  └────────────┘ │
     └────────────────┘ └────────────────┘ └────────────────┘
```

### Performance Budget (Per Frame at 60fps = 16.67ms)

| Phase | Time Budget | Notes |
|---|---|---|
| Triple buffer swap | <0.01ms | Atomic pointer swap |
| Data slice (mmap read) | <0.1ms | O(1) offset calculation + memcpy |
| LOD downsampling | 0-0.5ms | Only if zoom level changed |
| Indicator update (if new data) | 0-0.5ms | O(1) incremental, on separate thread |
| GPU buffer upload (dirty charts) | 0.1-0.5ms per chart | `bufferSubData` for incremental; full upload on viewport change |
| GPU render (instanced candles) | 0.2-0.5ms per chart | 5K candles + indicators = 1-2 draw calls |
| Compositor pass | 0.2ms | Draw all chart textures as screen quads |
| UI overlay (crosshairs, tooltips) | 0.3ms | |
| **Total (3 dirty charts of 20)** | **~2-3ms** | **14ms headroom** |
| **Total (all 20 charts updating)** | **~8-12ms** | **Still within budget** |

### Development Roadmap Estimate

| Phase | Scope | Effort |
|---|---|---|
| **1. Rendering foundation** | wgpu pipeline, instanced candle shader, basic zoom/pan | 4-6 weeks |
| **2. Data pipeline** | mmap binary files, WebSocket ingest, tick aggregation, triple buffer | 3-4 weeks |
| **3. Chart interaction** | Crosshair, horizontal levels, selections, pixel-perfect alignment | 3-4 weeks |
| **4. iced integration** | Application shell, multi-chart layout, panels, watchlist | 3-4 weeks |
| **5. Indicator framework** | Incremental engine, DAG, SMA/EMA/RSI/MACD/Bollinger | 3-5 weeks |
| **6. Multi-chart sync** | TimeAxisController, crosshair sync, shared time axis | 2-3 weeks |
| **7. Polish** | HiDPI, theming, keyboard shortcuts, persistence | 2-3 weeks |
| **Total to production alpha** | | **~5-7 months** |

### Alternative Stacks Considered

| Alternative | Pros | Cons | When to Choose |
|---|---|---|---|
| **egui + wgpu** | Faster prototyping, larger community | Immediate-mode overhead, less structured | If you want to ship a prototype fast |
| **Makepad** | GPU-first architecture, live shader editing | Poor docs, tiny community | If you're comfortable pioneering |
| **Skia (skia-safe) + winit** | Most proven 2D renderer, Chrome-level quality | C++ build dependency, less Rust-idiomatic | If Vello's alpha status is too risky |
| **Tauri + Rust + WebGL2** | Web rendering, fast iteration, smaller binary | IPC latency, browser overhead, not as crisp | If cross-platform web deployment matters |
| **C++ (Qt + OpenGL)** | Most mature ecosystem, decades of tooling | Memory safety, build complexity, no Rust benefits | If team has deep C++ expertise |

---

## Key Crate Dependencies

```toml
[dependencies]
# GPU & Rendering
wgpu = "29"
winit = "0.30"
vello = "0.6"

# GUI
iced = { version = "0.13", features = ["wgpu"] }

# Async & Streaming
tokio = { version = "1", features = ["full"] }
tokio-tungstenite = "0.24"
crossbeam = "0.8"
triple_buffer = "8"

# Data
memmap2 = "0.9"
bytemuck = "1"               # zero-copy cast between byte slices and typed data

# SIMD (nightly or via packed_simd2)
# std::simd (nightly) or use explicit std::arch intrinsics on stable

# Serialization (for non-hot-path)
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

---

## Bottom Line

Build in **Rust** with **wgpu** for GPU rendering, **iced** for the application shell, and **Vello** for high-quality 2D overlays. Store data in **memory-mapped binary files** with **SoA layout**. Use **triple-buffered lock-free** streaming from **tokio-tungstenite** WebSocket connections. Compute indicators on the **CPU with SIMD** using incremental O(1) updates.

This gives you:
- **Full ownership** of every pixel in the rendering pipeline
- **TC2000-level crispness** through pixel-aligned wgpu shaders
- **60fps with 20+ charts**, each showing thousands of candles
- **Sub-millisecond** data updates from real-time feeds
- **Zero vendor lock-in** — every component is open source
- **Memory safety** guaranteed by the Rust compiler
- **Cross-platform** via wgpu (Vulkan/Metal/DX12)
