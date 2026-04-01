# Widget System Architecture Plan

> Comprehensive design for the Hand of Midas chart widget system.
> Covers all visual elements drawn on charts: levels, order brackets,
> volume profiles, indicators, annotations, and future extensions.
>
> Status: DESIGN SPECIFICATION
> Date: 2026-03-30
> Total: ~12,000 lines across 7 documents
>
> **Path convention**: Throughout this plan, `midas-chart` refers to
> `desktop/win/crates/midas-chart`, `midas-app` refers to
> `desktop/win/crates/midas-app`, and `midas-render` refers to
> `desktop/win/crates/midas-render`.
>
> **Canonical types**: `01-core-architecture.md` is the single authority
> for all shared type definitions (`AnnotationKind`, `Annotation`,
> `Presence`, `WidgetOutput`, `WidgetLabel`, `HitZone`, `HitZoneKind`,
> `ComputeContext`). Other documents reference these types but do not
> redefine them.

---

## Goals

1. **Unified widget architecture** -- All visual chart elements (levels, brackets, indicators, notes, markers) share a common compute→scene→GPU pipeline
2. **Migrate existing code** -- HorizontalLevel, G.ATR, and Volume Profile migrate to the new system with zero behavior change and all 128+ tests passing
3. **Order bracket system** -- Users can draw entry/TP/SL brackets with zone fills, R:R display, and prepare orders for broker submission (Draft mode without broker)
4. **Per-symbol annotation sharing** -- Annotations created on one chart automatically appear on all charts displaying the same symbol
5. **Extensibility without abstraction tax** -- New widget types can be added by following a documented 19-step checklist, with no trait objects, no plugin system, and no render abstraction layer

## Non-Goals

These are deliberately excluded from scope. Each has a trigger for reconsideration.

| Non-Goal | Why Excluded | Reconsider When |
|----------|-------------|-----------------|
| Multi-select / box select | Low priority, adds interaction complexity | User demand after v1 ships |
| Undo/redo | Significant infrastructure (action log + inverses) | Phase 8 (polish), not before |
| Plugin / extension API | Proprietary app, closed set of widget types | >15 annotation types or user-defined widgets needed |
| Trendlines / Fibonacci | Requires LineInstance GPU pipeline (diagonal lines) | After Moving Average proves the LinePipeline (Phase 5) |
| Real-time P&L on brackets | Requires live streaming data feed | After midas-feed streaming is built |
| Composable ChartModifier | handle_event() is ~2,400 lines but manageable | Widget-specific code within handle_event exceeds 500 lines |
| GPU text rendering (MSDF) | iced overlay works for <100 labels | Label count exceeds 100 per chart or text rendering is a profiled bottleneck |
| Accessibility (screen reader, high-contrast) | Desktop trading app, mouse-driven workflow | Regulatory requirement or user demand |

---

## Executive Summary

The widget system provides a unified architecture for all visual elements
on Hand of Midas charts. It replaces the current ad-hoc per-component
approach (separate levels, crosshair, G.ATR implementations) with a
cohesive system built on five proven principles:

1. **Sans-IO boundary** -- Widget logic lives in `midas-chart` with zero GPU/framework deps
2. **Enum dispatch** -- `AnnotationKind` enum (10-12x faster than trait objects)
3. **Per-symbol storage** -- `AnnotationStore` keyed by symbol, not chart (validated by TradingView, Bloomberg, ThinkOrSwim)
4. **Retained data + immediate composition** -- Annotations persist; render primitives recomputed each frame
5. **Generation counter dirty tracking** -- O(1) change detection, no boolean reset issues

### Core Architecture At a Glance

```
User Action
    |
    v
Drawing Tool (state machine)      Indicator Config (per-chart)
    |                                      |
    v                                      v
AnnotationStore (per-symbol)       compute_indicator()
    |                                      |
    v                                      v
compute_widget_scene()  <--  ComputeContext (camera, data, theme)
    |
    v
WidgetScene (fills + lines + markers + labels + hit_zones)
    |
    v
ChartScene (merged with candles, volume, grid)
    |
    v
GPU Pipeline (reuses GridLineInstance, fixed layer order)
    |
    v
Pixels
```

### Widget Categories

| Category | Storage | Examples | Interactive |
|----------|---------|----------|-------------|
| **Annotation** | AnnotationStore (per-symbol) | Levels, Order Brackets, Notes, Markers | Yes |
| **Indicator** | ChartState config (per-chart) | G.ATR, Volume Profile, Moving Avg, Velocity | Hover only |

### Target Use Cases

- **Horizontal Levels** -- Price levels drawn across all timeframes (migration of existing)
- **G.ATR Indicator** -- ATR consumption badge (migration of existing)
- **Order Entry Brackets** -- Complex multi-leg order visualization with in-chart UI
- **Volume Profile** -- Session/visible-range volume distribution (migration of existing)
- **Moving Averages** -- SMA/EMA/WMA line overlays (future, needs LineInstance pipeline)
- **Velocity/Momentum** -- Historical colored background layer (future)
- **Text Notes, Markers, Trendlines** -- Future annotation types

---

## Document Map

### [01 -- Core Architecture](01-core-architecture.md) (2,856 lines)

The foundational type system and compute pipeline.

- **AnnotationKind enum** -- 4 annotation variants (Level, OrderBracket, TextNote, Marker) + Custom escape hatch. Indicators (G.ATR, Volume Profile) are a separate `IndicatorKind` enum stored per-chart, not in AnnotationStore.
- **Presence enum** -- Active / Ghost / Hidden (from Bevy's three-tier visibility)
- **Annotation wrapper** -- ID, kind, presence, timeframe visibility, lock state
- **ComputeContext** -- Camera, CandleData, viewport, theme, snap closure
- **WidgetOutput** -- Fills (layer 6) + lines (layer 7) + markers (layer 8) + labels + hit zones
- **ChartScene integration** -- How widget output merges into existing pipeline
- **Dirty flag integration** -- New generation counters and cascading rules
- **Module structure** -- `midas-chart/src/widget/` file layout

### [02 -- Storage and Sync](02-storage-and-sync.md) (1,613 lines)

Per-symbol annotation storage and cross-chart synchronization.

- **AnnotationStore** -- `HashMap<SymbolKey, SymbolAnnotations>` with generation counters
- **SymbolKey** -- Normalized uppercase newtype with zero-alloc lookups
- **CRUD API** -- Closure-based update (prevents generation-bump-forgetting)
- **Cross-chart sync** -- Generation counter polling, no event system needed
- **Annotation categories** -- Price-only vs time-anchored vs server-owned
- **Persistence** -- Per-symbol JSON files, debounced atomic writes, schema versioning
- **Order bracket integration** -- BrokerEvent → read-only bracket annotations
- **Migration from LevelStore** -- Per-chart HashMap → per-symbol AnnotationStore

### [03 -- Rendering Pipeline](03-rendering-pipeline.md) (882 lines)

How widgets get from data to pixels through the three-phase pipeline.

- **Phase 1: Compute** -- `compute_widget_scene()` pure function, presence-aware alpha
- **Phase 2: Scene Upload** -- Four dirty tiers, `GrowableBuffer` strategy, conditional uploads
- **Phase 3: GPU Dispatch** -- 12-layer render order, pipeline reuse via GridLineInstance
- **Render primitive vocabulary** -- GridLineInstance covers 90% of needs; future MarkerInstance, LineInstance
- **Text/label rendering** -- iced overlay (v1), MSDF/SDF atlas (future)
- **Performance budget** -- ~300 instances typical, <100 us compute per chart

### [04 -- Interaction System](04-interaction-system.md) (1,940 lines)

How users create, select, drag, edit, and delete widgets.

- **Extended event flow** -- Input → ChartEvent → handle_event() → ChartAction → AnnotationStore mutation
- **Tool vs widget distinction** -- Tools create widgets; widgets are passive data
- **Selection model** -- Per-chart (view-specific), not per-annotation
- **HitZone system** -- Precomputed from last compute phase, reverse-iteration priority
- **Grab offset pattern** -- Prevents jump-to-cursor on drag
- **Drawing tools** -- LevelTool (migrate), BracketTool (new, multi-step state machine)
- **Order bracket interaction** -- Per-leg hit zones, drag constraints, in-chart iced overlay
- **Cursor management** -- Priority-based cursor icon system
- **Future: ChartModifier** -- Migration path when complexity exceeds thresholds

### [05 -- Widget Catalog](05-widget-catalog.md) (2,784 lines)

Complete specification for every concrete widget type.

- **HorizontalLevel** -- Data model, render output (1 line + label + hit zone), migration plan
- **OrderBracket** -- Multi-leg data model, zone fills, R:R display, per-status visuals, in-chart UI, broker connection
- **G.ATR** -- Config model, label-only output, annotation vs indicator distinction
- **VolumeProfile** -- Config (period/bins/POC/VA), ~50-200 GridLineInstances, computation caching
- **MovingAverage** -- SMA/EMA/WMA, step-line workaround for v1, proper LineInstance in v2
- **Velocity/Momentum** -- Historical colored background bars, color gradient mapping
- **TextNote** -- Anchored text with background, iced overlay rendering
- **MarkerAnnotation** -- Shape enum (circle/diamond/triangle/flag), trade fill markers
- **Trendline** -- Data model only (blocked on LineInstance pipeline)
- **Catalog summary table** -- Priority, storage, GPU primitives, instance budget per type

### [06 -- Implementation Roadmap](06-implementation-roadmap.md) (535 lines)

Phased build plan with dependencies, success criteria, and file change maps.

- **Phase 1A** -- Foundation (core types, AnnotationStore -- new code only, mergeable independently)
- **Phase 1B** -- Level migration (migrate existing levels into AnnotationStore)
- **Phase 2** -- Indicator architecture + G.ATR migration (acknowledges existing midas-indicators crate)
- **Phase 3** -- Volume Profile enhancement (POC, Value Area, caching)
- **Phase 4A** -- Order Bracket data model, compute, and rendering
- **Phase 4B** -- BracketTool state machine and interaction
- **Phase 5** -- Advanced widgets (moving average, velocity, notes, markers)
- **Phase 6** -- Persistence (per-symbol JSON, debounced writes, forward-compatible schema)
- **Phase 7** -- Order Bridge (BrokerEvent → AnnotationStore)
- **Phase 8** -- Polish (undo/redo, templates, link groups, import/export)
- **Testing strategy** -- Unit, integration, regression, property-based, performance
- **File change map** -- New files, modified files, deprecated files per phase
- **Integration gate** -- All parallel phases merged + regression before Phase 7
- **Critical path** -- Phase 1A → 1B → Phase 4A → Phase 4B → Integration Gate → Phase 7

### [07 -- Design Patterns](07-design-patterns.md) (1,440 lines)

Cookbook and reference for implementing any new widget.

- **Seven-layer architecture** -- Data Type → State Machine → Interaction → Compute → Render Types → Scene → View Overlay
- **Grab offset pattern** -- Complete code for preventing drag jump
- **Snap closure pattern** -- Unified compute for timestamp-space and index-space
- **Generation counter pattern** -- Step-by-step for adding new dirty tracking
- **Tool activation pattern** -- Single active tool, CursorClaim priority
- **Crosshair suppression** -- Decision table: suppress vs force_hide vs preview
- **Testing patterns** -- Templates for compute, hit test, state machine, serde, GPU size assertions
- **19-step checklist** -- Complete procedure for adding any new widget
- **Anti-patterns** -- 10 things to avoid with wrong/right code examples

---

## Research Foundation

This plan synthesizes findings from 7 research documents (~7,000 lines):

| Research | Key Contribution |
|----------|-----------------|
| `research/widget-architecture/01-tradingview.md` | Per-symbol storage, plugin model, zOrder |
| `research/widget-architecture/02-professional-charting.md` | SciChart modifiers, Highcharts navigator |
| `research/widget-architecture/03-game-engine-patterns.md` | Bevy visibility, iced Shader, GPUI flat scene |
| `research/widget-architecture/04-rust-dispatch.md` | Enum 10-12x faster, sans-IO validation |
| `research/widget-architecture/05-cross-chart-sync.md` | 6-platform sync analysis, recursion prevention |
| `research/widget-architecture/06-synthesis.md` | Concrete Rust sketches, what NOT to build |
| `desktop/win/plan/rust-widget-patterns-research.md` | Trait design patterns, lifecycle model |
| `plan/cross-chart-sync-research.md` | Per-symbol storage consensus, link groups |

---

## Key Architectural Decisions

| Decision | Choice | Alternative Rejected | Why |
|----------|--------|---------------------|-----|
| Dispatch | Enum (`AnnotationKind`) | `Box<dyn Widget>` | 10-12x faster, compile-time exhaustiveness, <20 types |
| Storage | Per-symbol | Per-chart | All pro platforms agree; eliminates sync complexity |
| Visibility | Presence enum (3-state) | Boolean visible | Ghost state for cross-chart preview, locked items |
| Dirty tracking | Generation counters (u64) | Boolean flags | iced Primitive takes `&self` (immutable), can't clear booleans |
| Render primitive | GridLineInstance reuse | New types per widget | 32-byte rect covers 90% of visual needs |
| Compute model | Immediate recompute | Cached scene graph | <500 annotations, compute cheaper than cache management |
| Tool architecture | Monolithic handle_event | Composable ChartModifier | ~2,400 lines total, but widget code will be <500 lines within it |
| Text rendering | iced overlay (v1) | GPU MSDF atlas | Simpler, <100 labels, GPU text added when needed |

---

## What NOT to Build (Premature Abstractions)

| Abstraction | Current State | Trigger to Add |
|-------------|--------------|----------------|
| Trait object dispatch | 4 annotation enum variants | >15 types or plugin API |
| Composable ChartModifier | ~2,400 lines but mostly pan/zoom | Widget code within handle_event exceeds 500 lines |
| Indicator trait | 2 indicators | >10 indicators |
| Render abstraction layer | Direct wgpu | SVG/PDF export needed |
| Scene graph | Flat list | Complex parent-child needed |
| Plugin system | Proprietary app | User-defined widget extensions |
| New GPU instance types | GridLineInstance works | Diagonal lines or SDF markers |

---

## Quick Start

**To understand the system:** Read documents 01 → 03 → 05

**To implement a widget:** Read document 07 (patterns cookbook), follow the 19-step checklist

**To plan a sprint:** Read document 06 (roadmap), start with Phase 1A

**To understand a specific widget:** Read document 05 (catalog), find your widget section
