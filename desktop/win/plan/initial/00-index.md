# Hand of Midas — Initial Implementation Plan

> Multi-Chart Stock Charting Platform | Rust + wgpu 27 + iced 0.14 | Windows Desktop
> Total plan: ~18,000 lines across 7 detailed specification documents

---

## Plan Files

| # | File | Lines | Scope |
|---|---|---|---|
| 1 | [scaffold-plan.md](scaffold-plan.md) | ~1,450 | Cargo workspace, 6-crate structure (incl. midas-chart), build config, CLAUDE.md, .gitignore, dev workflow |
| 2 | [window-management-and-chart-layout.md](window-management-and-chart-layout.md) | 2,418 | Binary split tree layout, drag-and-drop docking, resize, tab groups, presets, serialization |
| 3 | [gpu-rendering-architecture.md](gpu-rendering-architecture.md) | 2,995 | wgpu pipelines, WGSL shaders, instanced candles, MSDF text, pixel-perfect alignment, dirty flagging |
| 4 | [chart-interaction-system.md](chart-interaction-system.md) | 2,604 | Zoom/pan mechanics, crosshair, horizontal levels, momentum, animation, multi-chart sync |
| 5 | [data-architecture.md](data-architecture.md) | ~2,260 | Binary .midas format, mmap access, SoA buffers, CandleData trait, LOD downsampling, CSV import, timeframes |
| 6 | [iced-application-shell.md](iced-application-shell.md) | 3,165 | iced Application state, Message enum, Shader widget bridge, toolbar, subscriptions, theme |
| 7 | [testing-and-validation.md](testing-and-validation.md) | 3,132 | Headless wgpu rendering, screenshot comparison, pixel-perfect tests, benchmarks, CI |

---

## Goals

- Single-window Windows desktop application that renders 20+ simultaneous stock charts at 60 fps from local CSV data.
- TC2000-level pixel-perfect crispness for all chart elements (candles, wicks, grid lines, axis labels).
- Flexible grid layout with drag-and-drop docking, split/close, and tab groups.
- Full zoom/pan with momentum, crosshair overlay, and user-drawn horizontal levels.
- Complete ownership of every rendered pixel — no third-party charting library black boxes.

## Non-Goals (explicitly excluded for v1)

- macOS/Linux support
- Web deployment (WASM)
- Mobile (iOS/Android)
- Multi-window or multi-monitor
- Trading or order execution
- Level 2 / order book display
- Replay / backtesting UI
- Plugin system
- Custom scripting language
- Social or sharing features
- Real-time streaming data (the architecture is designed for future streaming, but v1 loads from local CSV files only)

---

## Context and Motivation

Existing charting platforms (TradingView, TC2000, thinkorswim) lock users into their rendering pipelines and charting paradigms. Customization stops at what their APIs expose, and pixel-level control over chart visualization is not possible. When you need to render a non-standard indicator, adjust sub-pixel alignment of candle wicks, or experiment with novel visual encodings for market data, you hit a wall.

The goal of Hand of Midas is full ownership of every pixel. The target user is an active trader and developer who wants programmatic control over chart visualization — someone who would rather write a custom wgpu shader for a volume profile than accept a canned widget. By building on Rust + wgpu + iced, we get native performance, GPU-accelerated rendering, and a modern widget toolkit without sacrificing low-level access to the graphics pipeline.

---

## Architecture Summary

```
┌─────────────────────────────────────────────────────────┐
│  midas-app (iced Application)                           │
│  ├── Toolbar: symbol search, timeframe, layout presets  │
│  ├── Workspace: binary split tree of chart panels       │
│  │   ├── Chart Panel 1 (iced Shader widget)             │
│  │   │   └── ChartRenderer (custom wgpu pipelines)      │
│  │   ├── Chart Panel 2                                  │
│  │   └── Chart Panel N                                  │
│  └── Status Bar: connection, clock                      │
├─────────────────────────────────────────────────────────┤
│  midas-render (GPU)           │  midas-chart (sans-IO)  │
│  ├── Candle pipeline          │  ├── ChartState         │
│  ├── Volume pipeline          │  ├── ChartScene (IR)    │
│  ├── Grid pipeline            │  ├── ChartInput         │
│  ├── HLine pipeline           │  ├── ChartEvent/Action  │
│  ├── Crosshair pipeline       │  ├── Camera2D           │
│  └── MSDF text pipeline       │  ├── DirtyFlags         │
│                               │  ├── HLevels, crosshair │
│                               │  └── Zoom/pan/momentum  │
├───────────────────────────────┤─────────────────────────┤
│  midas-core (shared)          │  midas-feed             │
│  ├── CandleData trait         │  ├── CSV import         │
│  ├── IDs, Timeframe           │  └── (future: WS feed)  │
│  ├── Events, Config           │                         │
│  └── Layout tree              │                         │
├───────────────────────────────┤                         │
│  midas-data                   │                         │
│  ├── Binary .midas files      │                         │
│  ├── Mmap access              │                         │
│  ├── SoA CandleBuffer         │                         │
│  └── LOD downsampling         │                         │
└─────────────────────────────────────────────────────────┘
```

## Crate Dependency Graph

```
midas-core          (leaf — IDs, Timeframe, CandleData trait, config, shared types)
    ↑
midas-data          (depends on midas-core — storage, binary format, SoA buffers)
    ↑
midas-chart         (depends on midas-core, midas-data — sans-IO chart logic + ChartScene)
    ↑
midas-render        (depends on midas-core, midas-data, midas-chart — wgpu GPU pipelines)
midas-feed          (depends on midas-core, midas-data — CSV import)
    ↑
midas-app           (depends on all above — iced application shell)
```

## Key Architectural Decisions

| Decision | Choice | Rationale |
|---|---|---|
| Layout model | Binary split tree | Local mutations, O(1) split/close, same model as tmux/VS Code |
| Candle rendering | Two-pass instanced wgpu | Wicks then bodies, shared instance buffer, 1-2 draw calls total |
| Text rendering | MSDF font atlas | Resolution-independent, crisp at any DPI, single texture |
| Grid lines | 1px filled rectangles | GPU line rasterization is hardware-dependent; rects are reliable |
| Data on disk | Custom .midas binary, 32-byte records | O(1) random access via mmap, padded for cache alignment |
| Memory layout | Structure of Arrays (SoA) | 3-6x cache utilization for rendering scans, SIMD-friendly |
| LOD | On-the-fly MinMax bucketing | Avoids pyramid invalidation on real-time appends |
| Dirty flagging | Generation counters in canonical `DirtyFlags` (midas-chart) | Eliminates "who clears the flag" timing bugs; single source of truth |
| Colors | Linear RGB internally, sRGB output | Physically correct alpha blending |
| Y-axis scaling | Animated exponential ease-out | Smooth transitions, ~250ms to 90% convergence |
| Config persistence | TOML (app config) + JSON (layout tree) | TOML for flat config, JSON for recursive tree |
| Sans-IO chart core | `midas-chart` crate | All chart logic (state, interactions, zoom/pan, auto-scale, momentum) is isolated from GPU and framework code. Produces a `ChartScene` IR consumed by the renderer. Testable without GPU. |
| CandleData trait | Trait in `midas-core`, impl in `midas-data` | Abstracts over candle data sources so `midas-chart` can program against an interface rather than a concrete `CandleBuffer`. Enables test fixtures, streaming adapters, and database cursors. |
| Interaction | 5-state state machine | Clean disambiguation of click/drag/pan/level-drag/placement |
| Testing | Headless wgpu offscreen rendering | AI-verifiable PNG output, CI-compatible |
| iced integration | Pipeline associated type (iced 0.14) | Shared GPU resources across all Shader widget instances |
| Draw mode switching | Small uniform buffer (not push constants) | Broader hardware compatibility; avoids PUSH_CONSTANTS feature requirement |

### Vello Decision Record

Vello was evaluated (see tech-stacks.md) but deferred for v1 because:

1. **Alpha stability risk.** Vello is pre-1.0 and its API surface is still changing. Pinning to an alpha release risks breakage on updates and limits access to bug fixes.
2. **Uncertain iced integration.** There is no established pattern for embedding Vello's rendering pipeline inside iced's Shader widget. Building that bridge would be speculative engineering.
3. **Complex C++ build dependency.** Comparisons with Skia-based alternatives introduced heavyweight C++ build chains; Vello avoids Skia but adds its own shader compilation complexity.

The MSDF text atlas provides crisp axis labels at any DPI. Custom line and rectangle pipelines give full control over candle and grid rendering. If indicator line rendering or text annotation needs grow beyond what the custom pipelines handle efficiently, Vello integration is the documented fallback path.

## Implementation Order

```
Phase 0: Scaffold (scaffold-plan.md)
    ↓
Phase 0.5: Core Interface Definitions (midas-core, midas-data, midas-chart)
    Define: CandleData trait, CandleBuffer, CandleSlice, Camera2D, DirtyFlags, ChartId, PaneId
    Both Phase 1 and Phase 2 code against these agreed interfaces.

    Phase 0.5 deliverables (in midas-core, midas-data, and midas-chart):
      - CandleData trait (in midas-core): len(), is_empty(), timestamp(), open(), high(), low(),
        close(), volume(), price_range(), find_index_by_time()
        This is the abstraction boundary between data storage and chart logic.
      - CandleBuffer (in midas-data): struct fields (SoA vecs), push(), len(), slice(),
        price_range(), find_index_by_time(). Implements CandleData trait.
      - CandleSlice: lifetime-borrowing view into CandleBuffer with the same field accessors
      - Camera2D (in midas-chart): struct fields (time_start/end, price_low/high, viewport dims,
        dpi_scale), time_to_x(), price_to_y(), x_to_time(), y_to_price(), projection_matrix()
      - DirtyFlags + DirtyTracker (canonical def in midas-chart): full implementation with
        generation counters and mark_*/needs_* methods
      - ChartId, PaneId, SymbolId: newtype wrappers with Copy, Clone, Eq, Hash
      - Timeframe enum with as_secs(), floor_timestamp()
      Method bodies may be todo!() stubs EXCEPT: DirtyFlags (fully implemented), CandleData
      trait definition, and type derives.
      Done when: cargo test --workspace passes with all types importable from downstream crates.

    Phase 0.5 also includes:
      - iced 0.14 Shader API spike: Build minimal Shader widget with colored triangle.
        Confirm exact Primitive and Pipeline trait signatures. Document actual method names
        and parameter types. Block Phase 1 GPU work until signatures are validated.
    ↓
   ┌────────────────────────────────────────────────┐
   │                                                │
Phase 1: GPU Rendering                        Phase 2: Data
(gpu-rendering-architecture.md)               (data-architecture.md)
   │                                                │
   └──────────────┬─────────────────┬───────────────┘
                  ↓                 ↓
Phase 2.5: Integration Gate
    Load AAPL.csv → write .midas binary → mmap read → CandleBuffer → headless render → verify PNG
    Must pass before Phase 3 begins.
    ↓
Phase 3: Interaction (chart-interaction-system.md)
    Depends on BOTH Phase 1 (renderer) AND Phase 2 (data for visible range)
    ↓                                          ↓
Phase 4a: iced Shell Skeleton                Phase 4b: Shader Widget Bridge
(Sections 1-4, 6-12 of                      (Section 5 of iced-application-shell.md:
 iced-application-shell.md:                   Shader widget bridge, Pipeline integration
 app state, messages, toolbar,                — requires Phase 1 + Phase 3.
 config, subscriptions, theme,                Connects interaction state machine
 keyboard — uses todo!() for                  to iced messages.)
 Shader widget. Parallel with Phase 3.)
(iced-application-shell.md)                  (iced-application-shell.md)
    ↓                                          ↓
   └──────────────┬────────────────────────────┘
                  ↓
Phase 5: Layout System (window-management-and-chart-layout.md)
    ↓
Phase 6: Polish & Integration
    ↓
Throughout: Testing (testing-and-validation.md)
```

## Open Questions

1. **iced Shader widget storage** — RESOLVED. iced 0.14's `Pipeline` associated type is shared across all widget instances. A single `Pipeline` struct holds GPU resources (pipelines, bind group layouts, shared textures) and is reused by every `Shader` widget. Per-chart state (instance buffers, uniforms) is managed in the `Primitive` returned by each widget.

2. **iced pane_grid vs custom binary split tree** — `pane_grid::Configuration` does support tree construction, making it viable for the basic split layout. However, the custom binary split tree model is still needed for tab groups (multiple charts stacked in a single pane with a tab bar), which `pane_grid` does not natively support. Decision: start with `pane_grid` for the initial split layout, layer tab group logic on top.

3. **wgpu version alignment** — RESOLVED. Use wgpu 27 to match iced 0.14's internal wgpu dependency. Run `cargo tree -d | grep wgpu` after first build to confirm no duplicates.

4. **MSDF atlas embedding**: Compile-time `include_bytes!` vs runtime font loading — remains open. `include_bytes!` is simpler and eliminates a runtime failure mode, but increases binary size by ~2-4 MB depending on glyph coverage. Runtime loading is more flexible for user-customizable fonts. Decide during Phase 1 implementation.
