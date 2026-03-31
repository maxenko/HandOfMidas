# Synthesis: Widget Architecture for Hand of Midas

> Analysis and concrete recommendations distilled from all research, 2026-03-30

---

## 1. Core Principles (What Every Library Agrees On)

After surveying TradingView, SciChart, ECharts, Plotly, D3, egui, iced, Bevy, GPUI, Makepad, and six professional trading platforms, the following principles emerged with near-universal agreement:

1. **Separate data from rendering.** Every successful architecture has a boundary between data (what to show) and presentation (how to show it). SciChart's `DataSeries` vs `RenderableSeries`, iced's `Primitive` as serialization boundary, Bevy's Extract phase, GPUI's flat scene list. The data side must be testable without GPU.

2. **Own copies of data.** TradingView, ECharts, Qt Charts, and iced all copy on ingestion. The rendering pipeline should never borrow from application state. Owned snapshots eliminate lifetime complexity.

3. **Layer-based rendering order.** QCustomPlot (background -> grid -> main -> axes -> legend -> overlay), GPUI (shadows -> quads -> paths -> underlines -> sprites), SciChart (BelowSeries -> Series -> AboveSeries -> AboveChart). Fixed pipeline execution order is simpler and more predictable than dynamic z-sorting.

4. **Interaction as a separable concern.** SciChart's ChartModifiers, D3's behaviors, egui's response system. Interaction logic should be decomposable into self-contained units, even if currently implemented monolithically.

5. **Charts are not games.** Bevy's ECS, render graphs, tile-based invalidation -- all over-engineered for charts. Charts have 5-10 visual element types, not thousands of entities. The Extract pattern is valuable; the rest is not.

6. **Direct GPU is correct for v1.** No render abstraction layer. wgpu IS the abstraction. Adding another layer (Vello, Skia) buys nothing when targeting a single renderer. Midas already demonstrates 5M candlesticks at 100+ FPS with instanced drawing.

---

## 2. The Presence Enum Pattern: Active / Ghost / Hidden

Derived from Bevy's three-tier visibility, Unity's CanvasGroup opacity propagation, and the cross-chart sync requirement for "visible but not editable" overlays.

```rust
/// Controls how an overlay participates in rendering and interaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Presence {
    /// Fully rendered, fully interactive. Normal state.
    /// Hit-testing includes this overlay. Drag/edit allowed.
    Active,

    /// Rendered at reduced opacity, NOT interactive.
    /// Hit-testing excludes this overlay. Cannot be dragged or edited.
    /// Use cases:
    ///   - Synced drawings from other charts (visible, not editable here)
    ///   - Preview ghost when a tool is active (showing placement)
    ///   - Locked annotations the user has explicitly locked
    Ghost,

    /// Not rendered, not interactive. Zero GPU cost.
    /// Still stored in the data model. Can be toggled back to Active.
    Hidden,
}

impl Presence {
    /// Alpha multiplier for rendering. Active = 1.0, Ghost = 0.4, Hidden = 0.0
    pub fn alpha(&self) -> f32 {
        match self {
            Presence::Active => 1.0,
            Presence::Ghost => 0.4,
            Presence::Hidden => 0.0,
        }
    }

    /// Whether this overlay participates in hit-testing.
    pub fn is_interactive(&self) -> bool {
        matches!(self, Presence::Active)
    }

    /// Whether this overlay should be rendered at all.
    pub fn is_visible(&self) -> bool {
        !matches!(self, Presence::Hidden)
    }
}
```

This three-state model is simpler than Bevy's three-tier system (which adds propagation) because chart overlays have a flat hierarchy. No parent-child opacity cascading needed.

---

## 3. Widget Storage: Per-Ticker, Not Per-Chart

Every professional platform converges on per-symbol storage for price-level annotations. The research validates `LevelStore`'s design and recommends extending it to all annotation types.

```rust
/// Centralized annotation storage, owned by MidasApp.
/// All charts read from this -- no per-chart copies, no sync.
pub struct AnnotationStore {
    /// Annotations keyed by symbol. Each symbol has one collection.
    by_symbol: HashMap<String, SymbolAnnotations>,
    /// Generation counter per symbol for O(1) dirty checking.
    generations: HashMap<String, u64>,
}

pub struct SymbolAnnotations {
    annotations: Vec<Annotation>,
    next_id: u64,
}

pub struct Annotation {
    pub id: AnnotationId,
    pub kind: AnnotationKind,
    pub presence: Presence,
    /// None = visible on all timeframes.
    /// Some = visible only on listed timeframes.
    pub visible_timeframes: Option<Vec<Timeframe>>,
    pub created_at: i64,
    pub modified_at: i64,
}
```

**Why not per-chart**: A horizontal level at $185.50 on AAPL is $185.50 regardless of timeframe or chart panel. Per-chart storage creates duplication, requires sync logic, and risks inconsistency (NinjaTrader's clone model).

**Why not per-workspace**: Annotations should survive workspace layout changes. When the user rearranges charts, their drawn levels should not disappear.

**Timeframe filtering**: The `visible_timeframes` field handles the "I only want this trendline on the daily chart" case without per-chart duplication. `None` means "show everywhere" (the common case for horizontal levels).

---

## 4. Rendering Pipeline: Compute -> Scene -> GPU

The research converges on a three-phase rendering pipeline that Midas has independently discovered:

```
Phase 1: COMPUTE (midas-chart, sans-IO)
  ChartState + AnnotationStore + Camera2D
           |
           v
  compute_chart_scene() -- pure function
           |
           v
  ChartScene { candles, volumes, grid, levels, annotations, crosshair, ... }

Phase 2: SCENE UPLOAD (midas-render)
  ChartScene -- the serialization boundary
           |
           v
  Dirty-flag check: scene.generations vs tracker
           |
           v
  Conditional GPU buffer writes (queue.write_buffer)

Phase 3: GPU DISPATCH (midas-render)
  Fixed pipeline execution order:
    1. Background    (clear color)
    2. Grid          (GridPipeline -- horizontal/vertical lines)
    3. Candles       (CandlePipeline -- instanced OHLC)
    4. Volume        (VolumePipeline -- instanced bars)
    5. Annotation fills  (GridPipeline -- zone fills, behind lines)
    6. Annotation lines  (GridPipeline -- level lines, brackets)
    7. Annotation markers (GridPipeline -- points, badges)
    8. Indicators    (GerchikAtr badge via iced overlay)
    9. Axes          (iced text overlay)
   10. Crosshair     (GridPipeline -- horizontal/vertical lines)
   11. Tooltips      (iced text overlay)
```

This matches:
- iced's `Program::draw() -> Primitive -> prepare() -> render()`
- Bevy's `Extract -> Prepare -> Queue -> Render`
- GPUI's `Layout -> Prepaint -> Paint`
- Vello's `Scene encoding -> compute shader stages`

The key invariant: **Phase 1 has zero GPU dependencies.** It can be unit-tested, fuzzed, and benchmarked without a window or GPU device. Phases 2 and 3 are mechanical -- they upload and dispatch.

---

## 5. Interaction Separation: Tools vs Visual Elements

The research reveals a clean separation between **tools** (interaction state machines) and **visual elements** (rendered overlays):

### Tools: Interaction State Machines

Tools handle user input and produce actions. They are transient modal states.

```rust
pub struct ChartState {
    // Tools -- each is a self-contained state machine
    pub crosshair: CrosshairTool,
    pub level_tool: LevelTool,
    pub bracket_tool: BracketTool,   // future
    pub marker_tool: MarkerTool,     // future

    // State that tools read from
    pub camera: Camera2D,
    pub interaction: InteractionMode,
}
```

Each tool:
- Owns its internal mode enum (Idle, Placing, Dragging, etc.)
- Exposes predicates: `is_active()`, `is_placing()`, `is_dragging()`
- Has `cancel()` for reset and `suspend_placing()` / `try_resume_placing()` for interruption
- Produces `ChartAction` values that the app shell processes

This is SciChart's ChartModifier pattern expressed as Rust structs rather than trait objects. It works because the tool set is small and closed.

### Visual Elements: Data + Render

Visual elements are pure data that produce render primitives. They are not interactive -- tools make them interactive.

```rust
pub struct HorizontalLevel {
    pub price: f64,
    pub color: [f32; 4],
    pub line_width: f32,
    pub label: Option<String>,
    pub presence: Presence,
}
```

The level does not know how to be dragged. The `LevelTool` knows how to drag a level by modifying its price. This separation means:
- Levels can exist without any tool being active
- Multiple tools can interact with levels differently
- Adding a new tool does not require changing level data structures

### Current vs Future Architecture

The monolithic `handle_event()` function (~500 lines) is manageable for ~10 interaction types. When it exceeds 1000 lines or 15 interaction modes, refactor to the composable modifier pattern:

```rust
pub trait ChartModifier: Send + Sync {
    fn on_event(&mut self, event: &ChartEvent, state: &ChartState) -> ModifierResult;
    fn priority(&self) -> u32 { 100 }
    fn is_active(&self) -> bool { true }
}

pub enum ModifierResult {
    Handled(Vec<ChartAction>),   // consumed the event, stop propagation
    Continue(Vec<ChartAction>),  // produced actions, continue propagation
    Ignored,                     // not relevant to this modifier
}
```

---

## 6. Enum Dispatch for Closed Set + Future Extensibility

### The Recommendation

Use `AnnotationKind` enum for all chart overlays. The research is unambiguous: for a closed, known set of types (<20 variants) with tight iteration loops, enum dispatch is 10-12x faster than trait objects and provides compile-time exhaustiveness checking.

```rust
#[derive(Debug, Clone)]
pub enum AnnotationKind {
    Level(HorizontalLevel),
    Bracket(OrderBracket),
    Note(TextNote),
    Marker(MarkerAnnotation),
    // Add new built-in variants here as needed.
    // When the count exceeds ~15, consider:
    // Custom(Box<dyn CustomAnnotation>),
}

impl AnnotationKind {
    /// Hit-test this annotation against a screen coordinate.
    pub fn hit_test(&self, px: f32, py: f32, camera: &Camera2D) -> Option<HitZone> {
        match self {
            AnnotationKind::Level(l) => l.hit_test(px, py, camera),
            AnnotationKind::Bracket(b) => b.hit_test(px, py, camera),
            AnnotationKind::Note(n) => n.hit_test(px, py, camera),
            AnnotationKind::Marker(m) => m.hit_test(px, py, camera),
        }
    }

    /// Compute render primitives for this annotation.
    pub fn to_render(&self, camera: &Camera2D, viewport: (u32, u32)) -> AnnotationRender {
        match self {
            AnnotationKind::Level(l) => l.to_render(camera, viewport),
            AnnotationKind::Bracket(b) => b.to_render(camera, viewport),
            AnnotationKind::Note(n) => n.to_render(camera, viewport),
            AnnotationKind::Marker(m) => m.to_render(camera, viewport),
        }
    }
}
```

### Extensibility Escape Hatch

When (if ever) a plugin system is needed, add a `Custom(Box<dyn CustomAnnotation>)` variant. This pays the vtable cost only for user-defined types while preserving enum dispatch performance for built-in types.

### Why NOT Trait Objects Now

- No downstream extensibility needed (this is a proprietary trading app, not a library)
- Hit-testing iterates all overlays every frame -- cache locality matters
- Exhaustive `match` catches missing variants at compile time
- The variant count is small (4-8, growing slowly)

---

## 7. Concrete Rust Struct/Enum Sketches

### The Full Widget System

```rust
// === Data Layer (midas-chart, sans-IO) ===

/// Unique identifier for an annotation within a symbol's collection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AnnotationId(u64);

/// A single annotation: kind + metadata.
#[derive(Debug, Clone)]
pub struct Annotation {
    pub id: AnnotationId,
    pub kind: AnnotationKind,
    pub presence: Presence,
    pub visible_timeframes: Option<Vec<Timeframe>>,
    pub created_at: i64,
    pub modified_at: i64,
}

/// Closed enum of all annotation types.
#[derive(Debug, Clone)]
pub enum AnnotationKind {
    Level(HorizontalLevel),
    Bracket(OrderBracket),
    Note(TextNote),
    Marker(MarkerAnnotation),
}

/// Three-state visibility/interactivity control.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Presence {
    Active,
    Ghost,
    Hidden,
}

/// A horizontal price level.
#[derive(Debug, Clone)]
pub struct HorizontalLevel {
    pub price: f64,
    pub color: [f32; 4],
    pub line_width: f32,
    pub label: Option<String>,
}

/// An order bracket (entry + stop loss + take profit).
#[derive(Debug, Clone)]
pub struct OrderBracket {
    pub entry_price: f64,
    pub stop_loss: Option<f64>,
    pub take_profit: Option<f64>,
    pub quantity: f64,
    pub side: OrderSide,
}

/// A text note anchored at (time, price).
#[derive(Debug, Clone)]
pub struct TextNote {
    pub anchor_time: i64,
    pub anchor_price: f64,
    pub text: String,
    pub background_color: [f32; 4],
}

/// A point marker at (time, price).
#[derive(Debug, Clone)]
pub struct MarkerAnnotation {
    pub time: i64,
    pub price: f64,
    pub marker_type: MarkerType,
    pub color: [f32; 4],
}

// === Render Output (still in midas-chart, sans-IO) ===

/// Render data for a single annotation, produced by compute_chart_scene().
#[derive(Debug, Clone)]
pub enum AnnotationRender {
    Lines(Vec<GridLineInstance>),
    Fills(Vec<GridLineInstance>),
    LinesAndFills {
        lines: Vec<GridLineInstance>,
        fills: Vec<GridLineInstance>,
    },
    Badge {
        position: [f32; 2],
        text: String,
        color: [f32; 4],
    },
}

// === Storage (midas-app) ===

/// Centralized per-symbol annotation storage.
pub struct AnnotationStore {
    by_symbol: HashMap<String, SymbolAnnotations>,
    generations: HashMap<String, u64>,
}

impl AnnotationStore {
    /// Get all visible annotations for a symbol at a given timeframe.
    pub fn visible_annotations(
        &self,
        symbol: &str,
        timeframe: Timeframe,
    ) -> impl Iterator<Item = &Annotation> {
        self.by_symbol
            .get(symbol)
            .into_iter()
            .flat_map(|sa| sa.annotations.iter())
            .filter(move |a| {
                a.presence.is_visible()
                    && a.visible_timeframes
                        .as_ref()
                        .map_or(true, |tfs| tfs.contains(&timeframe))
            })
    }

    /// Get generation counter for dirty checking.
    pub fn generation(&self, symbol: &str) -> u64 {
        self.generations.get(symbol).copied().unwrap_or(0)
    }

    /// Insert an annotation, increment generation.
    pub fn insert(&mut self, symbol: &str, kind: AnnotationKind) -> AnnotationId {
        let sa = self.by_symbol.entry(symbol.to_string()).or_default();
        let id = sa.next_id();
        sa.annotations.push(Annotation {
            id,
            kind,
            presence: Presence::Active,
            visible_timeframes: None,
            created_at: now_ms(),
            modified_at: now_ms(),
        });
        *self.generations.entry(symbol.to_string()).or_insert(0) += 1;
        id
    }
}

// === Scene Integration (midas-chart) ===

pub struct ChartScene {
    // Existing fields
    pub candles: Vec<CandleInstance>,
    pub volumes: Vec<VolumeInstance>,
    pub grid_instances: Vec<GridLineInstance>,
    pub axis_labels: Vec<AxisLabel>,
    pub crosshair: Option<CrosshairRender>,

    // Annotation render data (new)
    pub annotation_fills: Vec<GridLineInstance>,
    pub annotation_lines: Vec<GridLineInstance>,
    pub annotation_markers: Vec<GridLineInstance>,
    pub annotation_badges: Vec<BadgeRender>,

    // Indicator overlays
    pub gerchik_atr: Option<GerchikAtrRender>,

    // Generation tracking
    pub generations: SceneGenerations,
}
```

### How It All Fits Together

```
User clicks "Add Level" tool
  -> LevelTool enters Placing mode
  -> Mouse click at price $185.50
  -> ChartAction::CreateLevel { price: 185.50 }
  -> MidasApp inserts into AnnotationStore for current symbol
  -> AnnotationStore.generation("AAPL") increments
  -> Next frame: all AAPL charts detect generation change
  -> compute_chart_scene() reads AnnotationStore, produces GridLineInstances
  -> Renderer uploads to GPU, draws at layer 6/7
```

No sync logic. No cloning. No event storms. One source of truth, N readers.

---

## 8. What NOT To Build (Premature Abstractions)

Based on the research, explicitly defer these until their trigger conditions are met:

| Abstraction | Trigger to Add | Current Status |
|---|---|---|
| `Box<dyn Overlay>` trait objects | >15 annotation types or plugin API needed | 4 types, closed set |
| Composable `ChartModifier` trait | >10 interaction modes or >1000 lines in handle_event | ~10 modes, ~500 lines |
| `Indicator` trait | >10 indicators with common interface | 1 indicator (GerchikAtr) |
| Render abstraction layer | SVG/PDF export needed, or Vello reaches 1.0 | Direct wgpu is correct |
| Scene graph | Complex parent-child overlay relationships | Flat overlay list |
| New GPU instance types | `GridLineInstance` cannot express the visual | Covers all current needs |
| Plugin/extension system | User-defined custom overlays | Not needed for proprietary app |

The research is clear: every library that added abstractions prematurely paid a cost in complexity. TradingView added plugins in v4/v5. ECharts added registerable series in v6. SciChart started with its modifier system because it needed it from day one. Match the abstraction level to the current complexity, not future hypothetical complexity.
