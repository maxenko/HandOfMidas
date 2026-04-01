# 01 -- Widget System Core Architecture

> Foundational architecture document for the Hand of Midas chart widget system.
> Covers type system, compute pipeline, scene integration, and dirty tracking.
>
> Status: DESIGN SPECIFICATION (code sketches are authoritative designs, not final implementations)
> Date: 2026-03-30
> Depends on: midas-chart sans-IO core, ChartScene pipeline, Camera2D, DirtyFlags

---

## Table of Contents

1. [Design Principles](#1-design-principles)
2. [Core Type System](#2-core-type-system)
3. [ComputeContext](#3-computecontext)
4. [WidgetOutput](#4-widgetoutput)
5. [Integration with ChartScene](#5-integration-with-chartscene)
6. [Dirty Flag Integration](#6-dirty-flag-integration)
7. [Module Structure](#7-module-structure)
8. [Per-Symbol Storage Model](#8-per-symbol-storage-model)
9. [Error Handling and Edge Cases](#9-error-handling-and-edge-cases)
10. [Extension Points and Escape Hatches](#10-extension-points-and-escape-hatches)
11. [Migration Path from Existing Code](#11-migration-path-from-existing-code)
12. [Performance Budget](#12-performance-budget)

---

## 1. Design Principles

Five principles govern the widget system. Each is grounded in research
findings from TradingView, SciChart, Bloomberg, ThinkOrSwim, Bevy, egui,
iced, and GPUI, validated against the existing midas-chart codebase.


### 1.1 Sans-IO Boundary

The chart core (`midas-chart`) has zero GPU and zero framework dependencies.
Every type in the widget system is a plain Rust struct or enum containing
only numeric data, strings, and other plain Rust types. No `wgpu::Buffer`,
no `iced::Element`, no `Arc<Mutex<_>>`.

This is not a new constraint -- it is the existing architecture's central
invariant. `compute_chart_scene()` is a pure function:

```
ChartInput  -->  compute_chart_scene()  -->  ChartScene
```

Widgets extend this pipeline. They produce `WidgetOutput` (plain data)
which feeds into `ChartScene`. The renderer reads `ChartScene` and manages
GPU resources. The widget system never touches GPU resources directly.

**Research basis**: iced's `Program -> Primitive -> Pipeline` three-trait
separation validates this architecture. Bevy's Extract phase is the same
concept. The existing midas-chart codebase already implements this correctly
(as noted in the research: "Your current `ChartScene` IS the Primitive
concept"). The widget system must not compromise this boundary.

**Concrete rule**: No type in `midas-chart/src/widget/` may import from
`wgpu`, `iced`, `midas-render`, or any other framework crate. The only
external dependency permitted is `midas-core` (for `CandleData`, `Timeframe`,
`ChartId`).


### 1.2 Enum Dispatch for Closed Annotation Set

The set of widget types is **closed** -- known at compile time, controlled
by the same workspace. This makes enum dispatch the correct choice over
trait objects.

**Performance basis**: The `enum_dispatch` crate benchmarks show enum
dispatch is 10-12x faster than `Vec<Box<dyn Trait>>` for tight iteration
loops. The reasons are:

1. **Cache locality**: `Vec<AnnotationKind>` stores values contiguously.
   `Vec<Box<dyn Trait>>` stores fat pointers (16 bytes each) pointing to
   heap-scattered objects -- two levels of indirection.

2. **Vtable elimination**: Enum match compiles to a jump table or branch
   cascade. The compiler can inline variant methods. Trait objects require
   a vtable load + indirect jump, which blocks inlining.

3. **Branch prediction**: CPU branch predictors learn enum tag patterns.
   Vtable dispatch is essentially an unpredictable indirect call.

**Practical justification**: The widget system will have 6-8 built-in
variants. Hit-testing iterates all annotations per mouse event. Compute
iterates all annotations per frame. These are tight loops where the 10x
difference matters -- not in absolute terms (the annotation count is small),
but in cache behavior when interleaved with other per-frame work.

**The hybrid escape hatch**: A `Custom(Box<dyn CustomAnnotation>)` variant
is documented but NOT implemented until a real need arises. Adding it later
is a one-line enum extension plus a match arm. See Section 10.

**Exhaustive match**: The compiler enforces that every `match` on
`AnnotationKind` handles all variants. When a new widget type is added,
the compiler identifies every call site that needs updating. This is a
significant safety advantage over trait objects, where forgetting to handle
a new type compiles silently.


### 1.3 Per-Symbol Storage Model

Annotations are stored **per-symbol**, not per-chart. A horizontal level
at $185.50 on AAPL is meaningful regardless of which chart panel or
timeframe displays it.

**Research basis**: Every major platform agrees on this:

| Platform | Storage Model |
|---|---|
| TradingView | Per-symbol (separate storage, server-side) |
| ThinkOrSwim | Per-symbol (drawing sets, auto-sync across charts) |
| Bloomberg | Per-security group |
| NinjaTrader 8 | Per-instrument (global flag) |
| Sierra Chart | Per-chart with copy references |

The dominant pattern is per-symbol. This matches the existing `LevelStore`
design already chosen for Hand of Midas (`HashMap<String, Vec<HorizontalLevel>>`
keyed by symbol).

**Architectural consequence**: The widget system defines annotation *types*
and *compute logic*. Storage lives in the app layer (`midas-app`) as an
`AnnotationStore` keyed by symbol. Charts receive `&[Annotation]` slices
through `ChartInput`, never owning the annotations directly. Multiple
charts showing the same symbol share one canonical set of annotations.
Edits on any chart propagate instantly to every other chart showing that
symbol because they all read from the same source.

**Timeframe visibility**: Some annotations are only relevant on specific
timeframes (e.g., a 5-minute bracket is noise on a daily chart). The
`visible_timeframes` field on `Annotation` handles this without duplicating
data. An annotation with `visible_timeframes: Some(vec![M5, M15])` renders
only on 5-minute and 15-minute charts. An annotation with
`visible_timeframes: None` renders on all timeframes (the default).


### 1.4 Retained Data + Immediate Composition

Annotations are **retained data** -- they persist across frames in the
`AnnotationStore`. But their visual representation is **immediately
composed** each frame by the compute pipeline.

This is NOT a retained-mode scene graph. There is no persistent
`AnnotationNode` tree that gets incrementally updated. Each frame:

1. Read the current annotations from the store.
2. For each visible annotation, call its compute function.
3. Collect the `WidgetOutput` render primitives.
4. Flatten into `ChartScene` buffers.

**Research basis**: This matches the pattern validated across all examined
platforms. TradingView retains annotation data but re-renders from scratch
each frame. Bevy retains component data but re-extracts render data each
frame. egui is fully immediate-mode. The consensus is: **retain data,
immediately compose render primitives**.

**Why not incremental updates**: The annotation count is small (budget:
max 500 per chart, typical: 5-20). Computing `WidgetOutput` for 500
annotations is ~100 coordinate transforms, producing ~2000 `GridLineInstance`
values -- well under 0.5ms. Incremental caching would add complexity
(invalidation, partial updates, stale detection) for negligible performance
gain. The dirty flag system gates whether compute happens at all; within a
compute pass, iterating all annotations is the right approach.


### 1.5 Generation Counter Dirty Tracking

The widget system uses the same generation-counter pattern as the existing
`DirtyFlags`. A `u64` counter is incremented whenever annotations change.
The renderer's `DirtyTracker` compares its last-seen generation to decide
whether to re-upload GPU buffers.

**Why not boolean flags**: Boolean dirty flags have the "who clears it"
problem -- if two consumers read the flag, the first consumer clears it
and the second never sees the change. Generation counters are monotonically
increasing; each consumer tracks its own last-seen generation independently.
This is the established pattern in midas-chart (`DirtyFlags` + `DirtyTracker`).

**Cascading rules**: Annotation changes cascade to force re-computation
of hit zones and labels. Camera changes also force annotation re-computation
(because screen positions change). Theme changes force annotation color
recomputation. See Section 6 for the complete cascade table.

---

## 2. Core Type System

This section defines every type in the widget system. All types live in
`midas-chart` and have zero GPU or framework dependencies. Types that need
persistence derive `Serialize + Deserialize`.


### 2.1 AnnotationId

```rust
/// Monotonically increasing identifier for annotations.
///
/// Scoped to the `AnnotationStore` that owns it. Not globally unique
/// across stores -- two stores for different symbols may each have an
/// `AnnotationId(1)`. Within a store, IDs are never reused.
///
/// # Size
///
/// 8 bytes. Cheap to copy, hash, and compare.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AnnotationId(pub u64);

impl AnnotationId {
    /// The null/sentinel value. No valid annotation has this ID.
    /// Used for "no selection" checks without wrapping in Option.
    pub const NONE: Self = Self(0);

    /// Whether this is a valid (non-sentinel) ID.
    pub fn is_valid(self) -> bool {
        self.0 != 0
    }
}

impl fmt::Display for AnnotationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ann#{}", self.0)
    }
}
```

**Design decision**: IDs start at 1 (not 0) so that `AnnotationId(0)` can
serve as a sentinel value. This avoids `Option<AnnotationId>` in contexts
where the sentinel pattern is clearer (e.g., selection state). When
`Option<AnnotationId>` is more appropriate (e.g., hit-test results), use
`Option` directly.

**Persistence**: On load, the store sets its next-ID counter to
`max(existing_ids) + 1`. This prevents ID collisions after restart.


### 2.2 AnnotationKind Enum

```rust
/// The specific widget type of an annotation.
///
/// This is a closed enum -- all variants are known at compile time.
/// Adding a new variant requires modifying this enum and every `match`
/// arm that dispatches on it. The compiler enforces exhaustiveness.
///
/// # Variant sizing
///
/// Each variant should be kept under ~312 bytes so the enum (which is
/// sized to its largest variant + tag) doesn't waste memory.
/// `OrderBracket` is the largest variant at ~280-300 bytes (3 legs with
/// Option<String> labels). Add a compile-time assertion:
/// `static_assert!(std::mem::size_of::<AnnotationKind>() <= 312);`
/// If a future variant exceeds this, heap-allocate its data via
/// `Box<LargeVariantData>` within the variant struct.
///
/// # Annotations vs Indicators
///
/// This enum covers only **annotations** -- user-placed, per-symbol objects
/// stored in `AnnotationStore`. **Indicators** (G.ATR, Volume Profile,
/// Moving Averages) are a separate category: data-computed, per-chart,
/// stored in `ChartState.indicator_configs`. See `IndicatorKind` in
/// `midas-chart/src/indicators/mod.rs` (Phase 2).
///
/// # Extension
///
/// If a `Custom(Box<dyn CustomAnnotation>)` escape hatch is needed,
/// add it as the last variant. See Section 10 of the architecture doc.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AnnotationKind {
    /// Horizontal line at a price. The most common annotation type.
    /// Replaces the existing `HorizontalLevel` struct.
    Level(HorizontalLevel),

    /// Entry + optional TP/SL bracket. Maps to broker orders at the
    /// app layer. The chart crate sees these as pure visual geometry.
    OrderBracket(OrderBracket),

    /// Text note anchored to a price/time point on the chart.
    TextNote(TextNote),

    /// Icon or stamp at a specific price/time. Used for fill markers,
    /// signal flags, alerts, bookmarks.
    Marker(MarkerAnnotation),

    // ── Escape hatch (documented, not implemented) ──────────────
    // Custom(Box<dyn CustomAnnotation>),
    //
    // Add this variant when downstream code needs to define new
    // annotation types without modifying this enum. The trait object
    // pays a vtable cost only when used. See Section 10.
}
```

**Variant details**: Each variant wraps a dedicated struct defined in its
own module file. The structs are detailed below.


#### 2.2.1 HorizontalLevel

Migration target for the existing `levels.rs::HorizontalLevel`. The
struct gains line style and extend mode capabilities.

```rust
/// A horizontal line at a specific price.
///
/// The most common annotation type. Represents support/resistance levels,
/// moving average values, or any price of interest.
///
/// # Size budget: ~96 bytes (within 200-byte variant target)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HorizontalLevel {
    /// Price at which the horizontal line is drawn.
    pub price: f64,
    /// RGBA color of the line (linear space, NOT sRGB).
    pub color: [f32; 4],
    /// Line width in logical pixels. Typical: 1.0-3.0.
    pub line_width: f32,
    /// Line rendering style (solid, dashed, dotted).
    pub style: LineStyle,
    /// Optional text label displayed next to the price on the Y axis.
    /// Examples: "Support", "200 SMA", "Entry".
    pub label: Option<String>,
    /// How far the line extends horizontally.
    pub extend: LevelExtend,
    /// Icon displayed next to the label.
    pub icon: LevelIcon,
}

/// Line rendering style.
///
/// Dashed and dotted lines are rendered as multiple short `GridLineInstance`
/// segments. The GPU pipeline is unchanged -- it still draws axis-aligned
/// rectangles. The segmentation happens in the compute phase.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub enum LineStyle {
    /// Continuous line.
    #[default]
    Solid,
    /// Alternating dash/gap segments.
    Dashed {
        /// Length of each dash segment in logical pixels.
        dash_len: f32,
        /// Length of each gap between dashes in logical pixels.
        gap_len: f32,
    },
    /// Regularly spaced dots.
    Dotted {
        /// Spacing between dot centers in logical pixels.
        dot_spacing: f32,
    },
}

/// How far a level line extends horizontally across the chart.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub enum LevelExtend {
    /// Spans the entire visible chart width. Most common.
    #[default]
    FullWidth,
    /// Starts at a specific time, extends infinitely to the right.
    /// Used for bracket legs placed at a specific bar.
    RightFrom {
        /// Epoch milliseconds at which the line starts.
        timestamp: i64,
    },
    /// Bounded segment between two timestamps.
    Between {
        /// Start timestamp (epoch ms).
        start: i64,
        /// End timestamp (epoch ms).
        end: i64,
    },
}
```

**Migration from existing HorizontalLevel**: The new struct is a superset
of the old one. Mapping is mechanical:

```rust
// Old: levels.rs::HorizontalLevel
// New: widget::level::HorizontalLevel
fn migrate_level(old: crate::levels::HorizontalLevel) -> Annotation {
    Annotation {
        id: AnnotationId(old.id),
        kind: AnnotationKind::Level(HorizontalLevel {
            price: old.price,
            color: old.color,
            line_width: old.line_width,
            style: LineStyle::Solid,
            label: old.label,
            extend: LevelExtend::FullWidth,
            icon: old.icon,
        }),
        presence: Presence::Active,
        visible_timeframes: None,
        locked: old.locked,
        created_at: 0,
        modified_at: 0,
    }
}
```


#### 2.2.2 OrderBracket

A compound annotation representing a trade idea: entry, optional TP, optional
SL. The chart crate sees these as pure visual geometry. The app layer maps
them to broker orders.

```rust
/// An order bracket: entry line + optional take-profit and stop-loss.
///
/// The chart crate uses `BracketStatus` for visual styling only.
/// The app layer in midas-app maps brackets to `LocalOrder` instances
/// in midas-broker and keeps the status in sync.
///
/// # Size budget: ~280-300 bytes (largest AnnotationKind variant)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OrderBracket {
    /// The entry price line. Always present.
    pub entry: BracketLeg,
    /// Take-profit target. None if user hasn't set one yet.
    pub take_profit: Option<BracketLeg>,
    /// Stop-loss level. None if user hasn't set one yet.
    pub stop_loss: Option<BracketLeg>,
    /// Trade direction. Determines which side TP/SL go on.
    pub side: BracketSide,
    /// Visual status. The chart crate uses this for styling only.
    /// The app layer is responsible for keeping it in sync with
    /// broker order state.
    pub status: BracketStatus,
    /// Display quantity (informational label, not order routing).
    pub quantity: Option<f64>,
}

/// A single leg of an order bracket.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BracketLeg {
    /// Price level for this leg.
    pub price: f64,
    /// Optional time anchor. None = full-width ray from left edge.
    /// Some(ts) = ray starting at timestamp, extending right.
    pub timestamp: Option<i64>,
    /// Override color. If None, derived from BracketSide + leg role.
    pub color: Option<[f32; 4]>,
    /// Line style for this leg.
    pub style: LineStyle,
    /// Line thickness in logical pixels.
    pub line_width: f32,
    /// Text shown next to the price label.
    /// Examples: "Entry 185.50", "TP +2.5%", "SL -1.2%".
    pub label: Option<String>,
}

/// Trade direction for a bracket.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BracketSide {
    /// Long position: entry below TP, above SL.
    Long,
    /// Short position: entry above TP, below SL.
    Short,
}

/// Visual status of a bracket. Drives line style and opacity.
///
/// This enum is meaningful only for rendering. The chart crate does
/// not enforce state transitions -- the app layer does that based on
/// broker events.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum BracketStatus {
    /// Being drawn on chart, not yet actionable. Dashed lines.
    #[default]
    Draft,
    /// Submitted to broker, awaiting entry fill. Dotted lines.
    Pending,
    /// Entry partially filled. Dotted + solid mix.
    PartialFill,
    /// Entry filled, TP/SL orders live at broker. Solid lines.
    Active,
    /// TP or SL triggered, position closed. Dimmed solid lines.
    Closed,
    /// User or broker cancelled. Dimmed solid lines.
    Cancelled,
}

impl OrderBracket {
    /// Compute risk:reward ratio. Returns None if TP or SL is missing,
    /// or if risk is effectively zero.
    pub fn risk_reward(&self) -> Option<f64> {
        let tp = self.take_profit.as_ref()?;
        let sl = self.stop_loss.as_ref()?;
        let risk = (self.entry.price - sl.price).abs();
        let reward = (tp.price - self.entry.price).abs();
        if risk < f64::EPSILON {
            return None;
        }
        Some(reward / risk)
    }
}
```


#### 2.2.3 VolumeProfileConfig (Indicator, NOT AnnotationKind)

> **Note**: VolumeProfileConfig and IndicatorOverlay (below) are
> **indicator types**, stored per-chart in `IndicatorConfig`, NOT in
> AnnotationStore. They are defined here for completeness alongside
> all widget types but live in `midas-chart/src/indicators/`. See
> Phase 2 in `06-implementation-roadmap.md` for the indicator architecture.

Configuration for a volume profile overlay. The actual histogram data is
computed from candle data during the compute phase -- only the parameters
are stored.

```rust
/// Configuration for a volume-at-price histogram overlay.
///
/// The compute function reads candle data within the specified time range
/// and builds a horizontal histogram showing volume at each price level.
/// This is distinct from the existing global volume profile toggle
/// (`show_volume_profile`) -- this is a user-placed, time-bounded profile.
///
/// # Size budget: ~64 bytes
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VolumeProfileConfig {
    /// Start of the time range (epoch ms). Inclusive.
    pub time_start: i64,
    /// End of the time range (epoch ms). Inclusive.
    pub time_end: i64,
    /// Number of price bins. More bins = finer granularity.
    /// Clamped to [10, 500] during compute.
    pub num_bins: u32,
    /// Color for volume bars.
    pub color: [f32; 4],
    /// Opacity multiplier for the histogram bars.
    pub opacity: f32,
    /// Whether to highlight the Point of Control (highest-volume bin).
    pub show_poc: bool,
    /// Whether to show Value Area (70% of volume) boundaries.
    pub show_value_area: bool,
}
```


#### 2.2.4 IndicatorOverlay

Configuration for a computed indicator overlay. The indicator reads candle
data and produces render primitives during the compute phase.

```rust
/// An indicator overlay that computes visual output from candle data.
///
/// Indicators differ from static annotations (levels, notes) in that
/// their visual output depends on the underlying candle data, not just
/// on user-specified parameters. They are re-computed when data changes.
///
/// # Built-in indicators
///
/// - `GerchikAtr`: ATR percentage consumption badge (existing, migrated)
/// - More indicators will be added (SMA, EMA, Bollinger Bands, etc.)
///
/// # Size budget: ~128 bytes (parameters + enum tag)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IndicatorOverlay {
    /// Which indicator algorithm to run.
    pub indicator: IndicatorType,
    /// RGBA color for the indicator's primary visual element.
    pub color: [f32; 4],
    /// Line width for line-type indicators.
    pub line_width: f32,
    /// Whether to show the indicator's value label.
    pub show_label: bool,
}

/// Indicator algorithm selection with parameters.
///
/// Each variant carries algorithm-specific parameters. Adding a new
/// indicator means adding a variant here and a compute function in
/// the indicator module.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum IndicatorType {
    /// Gerchik ATR percentage consumption.
    /// Migrated from `gerchik_atr.rs`.
    GerchikAtr {
        /// ATR smoothing period (number of synthetic daily bars).
        period: usize,
        /// Percentage threshold for green/red coloring.
        threshold_pct: f32,
    },

    /// Simple Moving Average.
    Sma {
        /// Lookback period in candles.
        period: usize,
    },

    /// Exponential Moving Average.
    Ema {
        /// Lookback period in candles.
        period: usize,
    },

    // Future indicators added as variants here.
    // Each gets a compute function in widget/indicator.rs.
}
```


#### 2.2.5 TextNote

```rust
/// A text note anchored to a price/time point on the chart.
///
/// Rendered as a colored rectangle background with text on top.
/// The background is a GPU `GridLineInstance`; the text is rendered
/// by the iced overlay layer (same mechanism as axis labels).
///
/// # Size budget: ~96 bytes (excluding heap-allocated String)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TextNote {
    /// Anchor price (Y position).
    pub price: f64,
    /// Anchor timestamp (X position), epoch milliseconds.
    pub timestamp: i64,
    /// The note text content. Rendered by iced, not GPU.
    pub text: String,
    /// Background color for the note rectangle.
    pub background_color: [f32; 4],
    /// Text color.
    pub text_color: [f32; 4],
    /// Font size in logical pixels.
    pub font_size: f32,
    /// Maximum width in logical pixels for word wrapping.
    /// None = single line, no wrapping.
    pub max_width: Option<f32>,
}
```


#### 2.2.6 MarkerAnnotation

```rust
/// An icon or stamp at a specific price/time on the chart.
///
/// Used for: fill markers, buy/sell signals, alerts, bookmarks,
/// important events. Rendered as small colored shapes via the GPU
/// pipeline (approximated with `GridLineInstance` rects initially,
/// upgradeable to SDF-based markers later).
///
/// # Size budget: ~64 bytes
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MarkerAnnotation {
    /// Anchor price (Y position).
    pub price: f64,
    /// Anchor timestamp (X position), epoch milliseconds.
    pub timestamp: i64,
    /// Which icon shape to render.
    pub icon: MarkerIcon,
    /// Icon color.
    pub color: [f32; 4],
    /// Icon diameter in logical pixels. Typical: 6.0-16.0.
    pub size: f32,
    /// Tooltip text shown on hover. None = no tooltip.
    pub tooltip: Option<String>,
}

/// Available marker icon shapes.
///
/// Initially rendered as colored rectangles or stacked scanlines.
/// Can be upgraded to SDF-based shapes via a MarkerPipeline later
/// without changing the data model.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MarkerIcon {
    /// Filled circle. Used for fill events and generic markers.
    Circle,
    /// Upward-pointing triangle. Used for buy signals.
    TriangleUp,
    /// Downward-pointing triangle. Used for sell signals.
    TriangleDown,
    /// Diamond shape. Used for alerts.
    Diamond,
    /// X mark. Used for stop/cancel events.
    Cross,
    /// Flag shape. Used for important events.
    Flag,
    /// Star shape. Used for bookmarks.
    Star,
}
```


### 2.3 Presence Enum

Adapted from Bevy's three-tier visibility system. Bevy uses
`Visibility` (requested), `InheritedVisibility` (propagated), and
`ViewVisibility` (computed). For chart annotations, a simpler three-state
enum covers all needs: active, ghost, hidden.

```rust
/// Three-tier visibility state for annotations.
///
/// Adapted from Bevy's visibility system. Determines whether an
/// annotation is rendered, interactive, or completely dormant.
///
/// # Ghost mode
///
/// Ghost annotations are rendered at reduced opacity (~0.4 alpha)
/// and are NOT interactive (cannot be selected, dragged, or hit-tested).
/// Use cases:
/// - Cross-chart sync: levels from one chart appear as ghosts on
///   charts with different timeframes
/// - Locked annotations: visually present but not interactable
/// - Historical brackets: closed/cancelled orders shown faintly
///
/// # Hidden mode
///
/// Hidden annotations are not rendered at all. They remain in storage
/// and can be restored to Active/Ghost. Zero GPU cost.
/// Use cases:
/// - User toggled visibility off
/// - Timeframe filter excluded this annotation
/// - Temporarily hidden during a batch operation
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Presence {
    /// Fully rendered, interactive, hit-testable.
    #[default]
    Active,
    /// Rendered at reduced opacity. NOT interactive or hit-testable.
    Ghost,
    /// Not rendered at all. Zero GPU cost. Still in storage.
    Hidden,
}

impl Presence {
    /// Alpha multiplier for this presence state.
    ///
    /// Applied to the annotation's color during compute.
    /// Active = full alpha (1.0), Ghost = dim alpha (0.4),
    /// Hidden = zero (but Hidden annotations skip compute entirely).
    pub fn alpha(&self) -> f32 {
        match self {
            Presence::Active => 1.0,
            Presence::Ghost => 0.4,
            Presence::Hidden => 0.0,
        }
    }

    /// Whether this annotation should respond to mouse events.
    ///
    /// Only Active annotations are interactive. Ghost annotations are
    /// visible but pass-through for mouse events. Hidden annotations
    /// don't exist visually.
    pub fn is_interactive(&self) -> bool {
        matches!(self, Presence::Active)
    }

    /// Whether this annotation should be rendered.
    ///
    /// Both Active and Ghost annotations are visible (Ghost at reduced
    /// opacity). Hidden annotations are not rendered.
    pub fn is_visible(&self) -> bool {
        !matches!(self, Presence::Hidden)
    }

    /// Whether this annotation should be included in hit-testing.
    ///
    /// Same as `is_interactive()` -- only Active annotations participate
    /// in hit-testing. Ghost annotations are visual-only.
    pub fn is_hit_testable(&self) -> bool {
        self.is_interactive()
    }

    /// Transition to the next visibility state in the cycle:
    /// Active -> Ghost -> Hidden -> Active
    ///
    /// Used by the "toggle visibility" keyboard shortcut (H key).
    pub fn cycle(&self) -> Self {
        match self {
            Presence::Active => Presence::Ghost,
            Presence::Ghost => Presence::Hidden,
            Presence::Hidden => Presence::Active,
        }
    }
}
```

**Design decision**: The `Ghost` state is not just "low alpha". It is a
semantic state that affects interaction behavior. A ghost annotation is
visible for context but cannot be accidentally moved or selected. This
distinction matters for cross-chart sync (levels from other timeframes)
and for historical brackets (closed orders that should remain visible as
reference points).


### 2.4 Annotation Wrapper

The top-level struct that wraps every annotation type with shared metadata.

```rust
/// A chart annotation with metadata.
///
/// Every drawable element on a chart is an `Annotation`. The `kind`
/// field determines the specific widget type; the wrapper provides
/// shared metadata (ID, presence, timestamps, lock state).
///
/// # Ownership
///
/// Annotations are owned by `AnnotationStore` (in the app layer).
/// Charts receive `&[Annotation]` slices through `ChartInput`.
/// Annotations are never owned by individual charts.
///
/// # Persistence
///
/// All fields derive `Serialize + Deserialize`. Annotations persist
/// as JSON files, one per symbol. See `02-storage-and-sync.md` Section 4
/// for the file format.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Annotation {
    /// Unique identifier within the owning store.
    pub id: AnnotationId,

    /// The specific widget type and its data.
    pub kind: AnnotationKind,

    /// Visibility and interactivity state.
    pub presence: Presence,

    /// Optional timeframe filter. If `Some`, this annotation is only
    /// rendered on charts with a matching timeframe. If `None`, rendered
    /// on all timeframes.
    ///
    /// Examples:
    /// - `None` -- a support level visible on all charts (most common)
    /// - `Some(vec![M5, M15])` -- an intraday bracket, hidden on daily
    /// - `Some(vec![D1])` -- a daily-only indicator
    pub visible_timeframes: Option<Vec<Timeframe>>,

    /// Whether this annotation is locked against drag/delete.
    ///
    /// Locked annotations can still be selected (to view properties)
    /// but cannot be moved or deleted. Useful for important levels
    /// that should not be accidentally disturbed.
    pub locked: bool,

    /// Creation timestamp (epoch milliseconds).
    /// Set once when the annotation is created. Never changes.
    pub created_at: i64,

    /// Last modification timestamp (epoch milliseconds).
    /// Updated on any data change (price drag, style edit, etc.).
    /// NOT updated for presence/lock changes (those are view state,
    /// not data changes).
    pub modified_at: i64,
}

impl Annotation {
    /// Whether this annotation should be rendered on a chart with
    /// the given timeframe.
    ///
    /// Returns `true` if:
    /// - `visible_timeframes` is `None` (visible on all timeframes), OR
    /// - `visible_timeframes` contains `tf`
    ///
    /// Also checks `presence` -- Hidden annotations return `false`
    /// regardless of timeframe.
    pub fn should_render_on(&self, tf: Timeframe) -> bool {
        if !self.presence.is_visible() {
            return false;
        }
        match &self.visible_timeframes {
            None => true,
            Some(tfs) => tfs.contains(&tf),
        }
    }

    /// Whether this annotation should be included in hit-testing
    /// on a chart with the given timeframe.
    ///
    /// Must be visible on the timeframe AND have Active presence
    /// AND not be locked.
    pub fn is_interactive_on(&self, tf: Timeframe) -> bool {
        self.should_render_on(tf) && self.presence.is_interactive()
    }

    /// Whether this annotation can be dragged (moved).
    ///
    /// Must be interactive AND not locked.
    pub fn is_draggable_on(&self, tf: Timeframe) -> bool {
        self.is_interactive_on(tf) && !self.locked
    }
}
```

**Why `external_id` is absent**: An earlier (superseded) annotation plan
included `external_id: Option<String>` for
mapping annotations to broker orders. This field belongs in the app layer's
bridge struct, not in the chart-core annotation. The chart crate should not
know about broker order IDs. The app layer maintains a separate
`OrderAnnotationLink` map that bridges `AnnotationId` to broker order UUIDs.
This keeps the sans-IO boundary clean.

**Why `tags: Vec<String>` is absent**: Tags are a UI filtering concern
that belongs in the app layer or in a separate metadata map. Including them
in the core annotation struct would increase the struct size for a feature
that the compute pipeline never reads. If tags prove necessary for core
logic (unlikely), they can be added later without breaking changes.


### 2.5 Widget Compute Interface

This is the compute interface that each widget kind implements. It is NOT
a trait -- it is a set of free functions dispatched via `match` on
`AnnotationKind`.

```rust
/// Compute render primitives for a single annotation.
///
/// This is the central dispatch function. It matches on `AnnotationKind`
/// and delegates to the appropriate per-variant compute function.
///
/// # Why not a trait
///
/// A `WidgetCompute` trait would be object-safe and could work with
/// `dyn WidgetCompute`. But:
///
/// 1. We already have the enum. Adding a trait creates two dispatch
///    mechanisms for the same purpose.
/// 2. The enum match gives exhaustiveness checking. A new variant
///    without a compute function is a compile error.
/// 3. The compiler can inline through enum match. It cannot inline
///    through `dyn WidgetCompute`.
/// 4. There is exactly one call site for this dispatch (inside
///    `compute_widget_outputs()`). A trait adds abstraction for
///    a single consumer.
///
/// If a `Custom(Box<dyn CustomAnnotation>)` escape hatch is added,
/// the Custom variant's match arm delegates to the trait object's
/// method. All other variants use direct dispatch.
pub fn compute_annotation(
    annotation: &Annotation,
    ctx: &ComputeContext<'_>,
) -> Option<WidgetOutput> {
    if !annotation.presence.is_visible() {
        return None;
    }

    let alpha = annotation.presence.alpha();

    let mut output = match &annotation.kind {
        AnnotationKind::Level(level) => compute_level(level, ctx),
        AnnotationKind::OrderBracket(bracket) => compute_bracket(bracket, ctx),
        AnnotationKind::TextNote(note) => compute_text_note(note, ctx),
        AnnotationKind::Marker(marker) => compute_marker(marker, ctx),
        // AnnotationKind::Custom(custom) => custom.compute(ctx),
    };

    // Apply presence alpha to all render primitives.
    if alpha < 1.0 {
        output.apply_alpha(alpha);
    }

    // Tag all hit zones with this annotation's ID.
    for hit_zone in &mut output.hit_zones {
        hit_zone.annotation_id = annotation.id;
    }

    Some(output)
}

/// Hit-test a single annotation at a screen coordinate.
///
/// Returns the hit zone description if the point intersects any
/// interactive area of this annotation. Returns `None` if:
/// - The annotation is not interactive (Ghost or Hidden)
/// - The annotation is locked (can be selected but not dragged)
/// - The point does not intersect any part of the annotation
pub fn hit_test_annotation(
    annotation: &Annotation,
    point: Point,
    ctx: &ComputeContext<'_>,
) -> Option<HitResult> {
    if !annotation.presence.is_hit_testable() {
        return None;
    }

    match &annotation.kind {
        AnnotationKind::Level(level) => hit_test_level(level, point, ctx),
        AnnotationKind::OrderBracket(bracket) => hit_test_bracket(bracket, point, ctx),
        AnnotationKind::TextNote(note) => hit_test_text_note(note, point, ctx),
        AnnotationKind::Marker(marker) => hit_test_marker(marker, point, ctx),
    }
}

/// Compute the bounding box of an annotation in screen coordinates.
///
/// Returns `None` for annotations that span the full viewport
/// (FullWidth levels, indicators) since their "bounding box" is
/// the entire chart area and is not useful for culling.
pub fn bounding_box_annotation(
    annotation: &Annotation,
    ctx: &ComputeContext<'_>,
) -> Option<BoundingBox> {
    match &annotation.kind {
        AnnotationKind::Level(level) => bounding_box_level(level, ctx),
        AnnotationKind::OrderBracket(bracket) => bounding_box_bracket(bracket, ctx),
        AnnotationKind::TextNote(note) => bounding_box_text_note(note, ctx),
        AnnotationKind::Marker(marker) => bounding_box_marker(marker, ctx),
    }
}
```

**Why match dispatch, not a trait**: The research explicitly recommends
enum dispatch for closed variant sets under 20 types. The `WidgetCompute`
trait sketch in the task description is the conceptual interface that each
variant must satisfy. The implementation is match-arm dispatch, not trait
object dispatch. The distinction matters: enum match gives exhaustiveness
checking (compile error if a new variant lacks a compute function), allows
compiler inlining, and avoids the cache-hostile indirection of vtable calls.

This approach also avoids the common mistake of defining a trait and then
implementing it on the enum (which is just a slower `match` with extra
boilerplate). The free-function approach is idiomatic Rust for closed
dispatch.


### 2.6 HitResult and HitZone

```rust
/// A point in screen coordinates (logical pixels).
#[derive(Clone, Copy, Debug)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

/// Result of a successful hit-test.
///
/// Tells the interaction layer which annotation was hit and which
/// specific sub-element was clicked, so it knows what drag behavior
/// to use.
#[derive(Clone, Debug)]
pub struct HitResult {
    /// Which annotation was hit.
    pub annotation_id: AnnotationId,
    /// Which part of the annotation was hit.
    pub zone: HitZoneKind,
    /// Screen distance from the hit point to the nearest edge of the
    /// hit zone. Used for priority (closer = higher priority when
    /// multiple annotations overlap).
    pub distance: f32,
}

/// Which part of an annotation was hit.
///
/// The interaction layer uses this to determine drag behavior:
/// - `LevelLine` -> vertical drag (price only)
/// - `BracketEntry` -> vertical drag (moves entire bracket)
/// - `BracketTP` / `BracketSL` -> vertical drag (moves single leg)
/// - `BracketZone` -> select only (no drag)
/// - `MarkerIcon` -> select only
/// - `NoteBody` -> 2D drag (price + time)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HitZoneKind {
    /// A level's horizontal line.
    LevelLine,
    /// A bracket's entry line.
    BracketEntry,
    /// A bracket's take-profit line.
    BracketTP,
    /// A bracket's stop-loss line.
    BracketSL,
    /// A bracket's zone fill (between entry and TP or SL).
    BracketZone,
    /// A marker's icon area.
    MarkerIcon,
    /// A text note's bounding box.
    NoteBody,
    /// A volume profile's histogram area.
    VolumeProfileBar,
}

/// An interactive area registered by a widget during compute.
///
/// Collected into `WidgetOutput::hit_zones` and used for hit-testing
/// without re-computing the widget's geometry.
#[derive(Clone, Debug)]
pub struct HitZone {
    /// Which annotation owns this hit zone.
    pub annotation_id: AnnotationId,
    /// Screen-space bounding rectangle: [left, top, right, bottom].
    pub rect: [f32; 4],
    /// What kind of element this hit zone represents.
    pub kind: HitZoneKind,
    /// Cursor icon to show when hovering this zone.
    pub cursor: CursorIcon,
}

/// Screen-space bounding box: [left, top, right, bottom] in logical pixels.
#[derive(Clone, Copy, Debug)]
pub struct BoundingBox {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl BoundingBox {
    /// Whether a point is inside this bounding box.
    pub fn contains(&self, point: Point) -> bool {
        point.x >= self.left
            && point.x <= self.right
            && point.y >= self.top
            && point.y <= self.bottom
    }

    /// Expand the bounding box by `margin` pixels in each direction.
    pub fn expand(&self, margin: f32) -> Self {
        Self {
            left: self.left - margin,
            top: self.top - margin,
            right: self.right + margin,
            bottom: self.bottom + margin,
        }
    }
}
```

---

## 3. ComputeContext

The `ComputeContext` bundles everything a widget's compute function needs
to transform data-space annotations into screen-space render primitives.
It is a borrowed struct with a short lifetime -- created at the start of
each compute pass and dropped at the end.

```rust
/// Context passed to every widget compute function.
///
/// Contains everything needed to transform data-space annotation
/// coordinates into screen-space render primitives. Borrowed for
/// the duration of the compute pass -- never stored.
///
/// # Lifetime
///
/// All references borrow from the `ChartInput` that drives the current
/// frame's computation. The context is created at the start of
/// `compute_widget_outputs()` and dropped when that function returns.
pub struct ComputeContext<'a> {
    /// Camera defining the visible time/price window.
    /// Used for `price_to_y()`, `time_to_x()`, and viewport queries.
    pub camera: &'a Camera2D,

    /// Candle data source for indicators and volume profile computation.
    /// Trait object so the compute function works with any data source
    /// (live data, test fixtures, replayed data).
    pub data: &'a dyn CandleData,

    /// Viewport dimensions in logical pixels.
    pub viewport: Viewport,

    /// Current theme colors for default annotation styling.
    pub theme: &'a Theme,

    /// OHLC snap function: given a screen Y coordinate, returns the
    /// nearest OHLC snap target as `(snapped_screen_y, candle_index)`.
    ///
    /// Used by interactive annotations (levels, bracket legs) for
    /// price snapping during placement and drag. The snap function
    /// is provided by the level tool / interaction layer.
    ///
    /// Returns `None` if no snap target is within threshold distance.
    pub snap_fn: &'a dyn Fn(f32) -> Option<(f32, usize)>,

    /// Estimated candle duration in milliseconds.
    /// Used to convert between time-space and candle-index-space
    /// for annotations anchored to specific bars.
    pub candle_duration_ms: f64,

    /// Whether gaps are collapsed (index-based X positioning).
    /// When true, X coordinates are computed from candle indices,
    /// not timestamps. Annotations anchored to timestamps must
    /// be converted to index-space for rendering.
    pub collapse_gaps: bool,

    /// Separator Y position between price area and volume area.
    /// Annotations should not render below this line (they would
    /// overlap the volume bars).
    pub separator_y: f32,

    /// DPI scale factor for physical pixel calculations.
    /// Line widths specified in logical pixels are multiplied by
    /// this factor for physical pixel rendering.
    pub dpi_scale: f32,
}

/// Viewport dimensions.
#[derive(Clone, Copy, Debug)]
pub struct Viewport {
    /// Width in logical pixels.
    pub width: u32,
    /// Height in logical pixels.
    pub height: u32,
}

/// Theme colors used by default annotation styling.
///
/// Annotations can override these with per-annotation colors.
/// These are the fallback defaults when an annotation doesn't
/// specify its own color.
#[derive(Clone, Debug)]
pub struct Theme {
    /// Default level line color.
    pub level_color: [f32; 4],
    /// Default long bracket color (green-ish).
    pub bracket_long_color: [f32; 4],
    /// Default short bracket color (red-ish).
    pub bracket_short_color: [f32; 4],
    /// Default take-profit color.
    pub bracket_tp_color: [f32; 4],
    /// Default stop-loss color.
    pub bracket_sl_color: [f32; 4],
    /// Default bracket zone fill alpha.
    pub bracket_zone_alpha: f32,
    /// Default note background color.
    pub note_bg_color: [f32; 4],
    /// Default note text color.
    pub note_text_color: [f32; 4],
    /// Default marker color.
    pub marker_color: [f32; 4],
    /// Selection highlight color (glow around selected annotations).
    pub selection_color: [f32; 4],
    /// Selection highlight extra thickness in logical pixels.
    pub selection_thickness: f32,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            level_color: [0.0, 0.7, 1.0, 0.9],
            bracket_long_color: [0.15, 0.65, 0.35, 0.9],
            bracket_short_color: [0.65, 0.15, 0.15, 0.9],
            bracket_tp_color: [0.15, 0.65, 0.35, 0.5],
            bracket_sl_color: [0.65, 0.15, 0.15, 0.5],
            bracket_zone_alpha: 0.06,
            note_bg_color: [0.15, 0.15, 0.2, 0.85],
            note_text_color: [0.9, 0.9, 0.9, 1.0],
            marker_color: [0.8, 0.8, 0.2, 0.9],
            selection_color: [1.0, 1.0, 1.0, 0.4],
            selection_thickness: 2.0,
        }
    }
}
```

**Why `snap_fn` is a closure, not a method**: The snap function depends
on per-chart state (which candles are visible, current camera, OHLC values)
that varies across charts. Passing it as a closure allows each chart's
compute pass to capture its own context without the ComputeContext needing
to own all that data. This matches the existing `LevelTool::snap_to_ohlc()`
pattern.

**Why `Theme` is a struct, not the existing color fields**: The existing
`ChartInput` passes colors as individual fields (`bull_color`, `bear_color`,
`grid_color`). For widget-specific colors (bracket TP/SL, selection glow),
a dedicated theme struct is cleaner than adding 10+ individual color fields
to `ChartInput`. The `Theme` can be constructed from the existing color
fields plus widget-specific defaults.

---

## 4. WidgetOutput

The render primitives a widget produces. This is the serialization boundary
between widget logic and the renderer.

```rust
/// Render primitives produced by a single widget's compute function.
///
/// Contains all the GPU-ready geometry and metadata needed to render
/// the widget. Organized by rendering layer so the caller can sort
/// primitives into the correct draw order.
///
/// # Layer mapping
///
/// | Field | GPU Layer | Draw Order |
/// |---|---|---|
/// | `fills` | Layer 6 | Behind annotation lines |
/// | `lines` | Layer 7 | On top of fills |
/// | `markers` | Layer 8 | On top of lines |
/// | `labels` | Layer 10 | iced overlay (above all GPU) |
/// | `hit_zones` | N/A | Not rendered, used for interaction |
///
/// # Reuse of GridLineInstance
///
/// 90%+ of widget rendering needs are axis-aligned colored rectangles:
/// lines, fills, backgrounds, selection highlights. `GridLineInstance`
/// (32 bytes: `[f32; 4]` rect + `[f32; 4]` color) covers all of these.
/// The existing `GridPipeline` shader already renders these. No new
/// shaders are needed for the initial widget system.
///
/// The remaining 10% (marker shapes, indicator curves) will eventually
/// need dedicated pipelines (SDF markers, line-strip pipeline). Those
/// are future additions that don't affect the WidgetOutput data model --
/// new fields can be added alongside `fills`/`lines`/`markers`.
#[derive(Clone, Debug, Default)]
pub struct WidgetOutput {
    /// Background fills rendered at Layer 4 (behind candles).
    ///
    /// Used for: bracket zone fills (semi-transparent rectangles between
    /// TP and SL), note backgrounds, volume profile histogram bars.
    ///
    /// Each `GridLineInstance` specifies a rectangle and a color.
    /// Low-alpha fills create subtle shaded regions behind other elements.
    pub fills: Vec<GridLineInstance>,

    /// Lines and borders rendered at Layer 7.
    ///
    /// Used for: level lines, bracket leg lines, selection highlights,
    /// note borders.
    ///
    /// For dashed lines, this contains multiple short segments (one
    /// `GridLineInstance` per dash). At typical dash/gap ratios (8px/4px),
    /// a 1920px viewport produces ~160 segments per dashed line -- about
    /// 5 KB, negligible.
    pub lines: Vec<GridLineInstance>,

    /// Markers and point elements rendered at Layer 8.
    ///
    /// Used for: fill markers, signal icons, bracket drag handles,
    /// POC indicators.
    ///
    /// Initially rendered as small colored rectangles. Can be upgraded
    /// to SDF-based shapes later without changing this data model.
    pub markers: Vec<GridLineInstance>,

    /// Text labels rendered by the iced overlay at Layer 10.
    ///
    /// Used for: price badges on Y axis, note text, R:R ratio labels,
    /// indicator value labels.
    ///
    /// Text cannot be rendered by the GPU pipeline (no glyph atlas).
    /// Labels are passed to the iced overlay layer, which renders them
    /// as `iced::widget::Text` elements positioned absolutely.
    pub labels: Vec<WidgetLabel>,

    /// Interactive hit zones for mouse event handling.
    ///
    /// NOT rendered. These are screen-space rectangles that define
    /// clickable/draggable areas. The interaction layer queries these
    /// during hit-testing.
    ///
    /// Hit zones are computed during the same pass as render primitives
    /// to ensure they are always in sync with the visual output.
    pub hit_zones: Vec<HitZone>,
}

impl WidgetOutput {
    /// Create an empty output with no render primitives.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Apply an alpha multiplier to all render primitives.
    ///
    /// Used to implement `Presence::Ghost` (0.4 alpha).
    /// Multiplies the alpha channel of every color in fills, lines,
    /// and markers. Labels are also dimmed.
    pub fn apply_alpha(&mut self, alpha: f32) {
        for instance in self.fills.iter_mut()
            .chain(self.lines.iter_mut())
            .chain(self.markers.iter_mut())
        {
            instance.color[3] *= alpha;
        }
        for label in &mut self.labels {
            label.bg_color[3] *= alpha;
            label.text_color[3] *= alpha;
        }
    }

    /// Merge another WidgetOutput into this one.
    ///
    /// Used when a compound widget (like OrderBracket) builds its
    /// output from sub-components.
    pub fn merge(&mut self, other: WidgetOutput) {
        self.fills.extend(other.fills);
        self.lines.extend(other.lines);
        self.markers.extend(other.markers);
        self.labels.extend(other.labels);
        self.hit_zones.extend(other.hit_zones);
    }

    /// Total number of GPU instances across all layers.
    ///
    /// Used for performance monitoring and budget enforcement.
    pub fn instance_count(&self) -> usize {
        self.fills.len() + self.lines.len() + self.markers.len()
    }
}

/// A text label positioned in screen space, rendered by the iced overlay.
///
/// Distinct from `AxisLabel` (used for price/time axis labels in the
/// existing codebase). `WidgetLabel` is used exclusively for annotation
/// and indicator labels.
#[derive(Clone, Debug)]
pub struct WidgetLabel {
    /// Text content to display.
    pub text: String,
    /// Screen-space X position in logical pixels.
    pub screen_x: f32,
    /// Screen-space Y position in logical pixels.
    pub screen_y: f32,
    /// Background color (RGBA). Transparent for no background.
    pub bg_color: [f32; 4],
    /// Text color (RGBA).
    pub text_color: [f32; 4],
    /// Font size in logical pixels. Default: 11.0.
    pub font_size: f32,
    /// Anchor point for positioning.
    pub anchor: LabelAnchor,
}

#[derive(Clone, Copy, Debug, Default)]
pub enum LabelAnchor {
    /// Position is top-left corner of the label.
    #[default]
    TopLeft,
    /// Position is horizontal center, vertical center.
    Center,
    /// Position is left edge, vertically centered.
    Left,
    /// Position is right edge, vertically centered.
    Right,
}
```

### 4.1 Per-Variant Compute Function Sketches

Each `AnnotationKind` variant has a dedicated compute function. These
are design-specification sketches showing the structure, not final
production code.


#### compute_level

```rust
/// Compute render primitives for a horizontal level.
fn compute_level(level: &HorizontalLevel, ctx: &ComputeContext<'_>) -> WidgetOutput {
    let mut output = WidgetOutput::empty();

    let y = ctx.camera.price_to_y(level.price);

    // Skip if the level is outside the visible price range.
    // Add a small margin so levels at the edge still render.
    let margin = level.line_width * 2.0;
    if y < -margin || y > ctx.viewport.height as f32 + margin {
        return output;
    }

    // Determine X range based on extend mode.
    let (x_start, x_end) = match &level.extend {
        LevelExtend::FullWidth => (0.0, ctx.viewport.width as f32),
        LevelExtend::RightFrom { timestamp } => {
            let x = ctx.camera.time_to_x(*timestamp as f64);
            (x.max(0.0), ctx.viewport.width as f32)
        }
        LevelExtend::Between { start, end } => {
            let x0 = ctx.camera.time_to_x(*start as f64);
            let x1 = ctx.camera.time_to_x(*end as f64);
            (x0.max(0.0), x1.min(ctx.viewport.width as f32))
        }
    };

    // Generate line segments based on style.
    let half_width = level.line_width * 0.5;
    match &level.style {
        LineStyle::Solid => {
            output.lines.push(GridLineInstance {
                rect: [x_start, y - half_width, x_end, y + half_width],
                color: level.color,
            });
        }
        LineStyle::Dashed { dash_len, gap_len } => {
            let mut x = x_start;
            while x < x_end {
                let seg_end = (x + dash_len).min(x_end);
                output.lines.push(GridLineInstance {
                    rect: [x, y - half_width, seg_end, y + half_width],
                    color: level.color,
                });
                x += dash_len + gap_len;
            }
        }
        LineStyle::Dotted { dot_spacing } => {
            let dot_size = level.line_width * 1.5;
            let mut x = x_start;
            while x < x_end {
                output.markers.push(GridLineInstance {
                    rect: [
                        x - dot_size * 0.5,
                        y - dot_size * 0.5,
                        x + dot_size * 0.5,
                        y + dot_size * 0.5,
                    ],
                    color: level.color,
                });
                x += dot_spacing;
            }
        }
    }

    // Hit zone: full-width rectangle with vertical tolerance.
    let hit_tolerance = 6.0; // pixels
    output.hit_zones.push(HitZone {
        annotation_id: AnnotationId::NONE, // filled in by caller
        rect: [x_start, y - hit_tolerance, x_end, y + hit_tolerance],
        kind: HitZoneKind::LevelLine,
    });

    // Price label on Y axis.
    if let Some(label_text) = &level.label {
        output.labels.push(WidgetLabel {
            text: label_text.clone(),
            screen_x: ctx.viewport.width as f32 - 80.0, // right-aligned
            screen_y: y,
            bg_color: level.color,
            text_color: [1.0, 1.0, 1.0, 1.0],
            font_size: 11.0,
            anchor: LabelAnchor::Right,
        });
    }

    output
}
```


#### compute_bracket

```rust
/// Compute render primitives for an order bracket.
fn compute_bracket(bracket: &OrderBracket, ctx: &ComputeContext<'_>) -> WidgetOutput {
    let mut output = WidgetOutput::empty();

    let entry_y = ctx.camera.price_to_y(bracket.entry.price);

    // Determine line style based on bracket status.
    let effective_style = match bracket.status {
        BracketStatus::Draft => LineStyle::Dashed { dash_len: 8.0, gap_len: 4.0 },
        BracketStatus::Pending => LineStyle::Dotted { dot_spacing: 6.0 },
        BracketStatus::PartialFill => LineStyle::Dashed { dash_len: 12.0, gap_len: 3.0 },
        BracketStatus::Active => LineStyle::Solid,
        BracketStatus::Closed | BracketStatus::Cancelled => LineStyle::Solid,
    };

    // Alpha dimming for closed/cancelled brackets.
    let status_alpha: f32 = match bracket.status {
        BracketStatus::Closed | BracketStatus::Cancelled => 0.3,
        _ => 1.0,
    };

    // Determine colors based on side.
    let (entry_color, tp_color, sl_color) = match bracket.side {
        BracketSide::Long => (
            ctx.theme.bracket_long_color,
            ctx.theme.bracket_tp_color,
            ctx.theme.bracket_sl_color,
        ),
        BracketSide::Short => (
            ctx.theme.bracket_short_color,
            ctx.theme.bracket_sl_color,
            ctx.theme.bracket_tp_color,
        ),
    };

    // Entry line.
    let entry_leg_output = compute_bracket_leg(
        &bracket.entry,
        entry_color,
        &effective_style,
        status_alpha,
        HitZoneKind::BracketEntry,
        ctx,
    );
    output.merge(entry_leg_output);

    // Take-profit line + zone fill.
    if let Some(tp) = &bracket.take_profit {
        let tp_y = ctx.camera.price_to_y(tp.price);
        let tp_leg_output = compute_bracket_leg(
            tp,
            tp.color.unwrap_or(tp_color),
            &effective_style,
            status_alpha,
            HitZoneKind::BracketTP,
            ctx,
        );
        output.merge(tp_leg_output);

        // Zone fill between entry and TP.
        let (top, bottom) = if entry_y < tp_y {
            (entry_y, tp_y)
        } else {
            (tp_y, entry_y)
        };
        let mut zone_color = tp_color;
        zone_color[3] = ctx.theme.bracket_zone_alpha * status_alpha;
        output.fills.push(GridLineInstance {
            rect: [0.0, top, ctx.viewport.width as f32, bottom],
            color: zone_color,
        });
    }

    // Stop-loss line + zone fill.
    if let Some(sl) = &bracket.stop_loss {
        let sl_y = ctx.camera.price_to_y(sl.price);
        let sl_leg_output = compute_bracket_leg(
            sl,
            sl.color.unwrap_or(sl_color),
            &effective_style,
            status_alpha,
            HitZoneKind::BracketSL,
            ctx,
        );
        output.merge(sl_leg_output);

        // Zone fill between entry and SL.
        let (top, bottom) = if entry_y < sl_y {
            (entry_y, sl_y)
        } else {
            (sl_y, entry_y)
        };
        let mut zone_color = sl_color;
        zone_color[3] = ctx.theme.bracket_zone_alpha * status_alpha;
        output.fills.push(GridLineInstance {
            rect: [0.0, top, ctx.viewport.width as f32, bottom],
            color: zone_color,
        });
    }

    // R:R ratio label.
    if let Some(rr) = bracket.risk_reward() {
        output.labels.push(WidgetLabel {
            text: format!("R:R {:.1}:1", rr),
            screen_x: 10.0, // left-aligned
            screen_y: entry_y - 14.0, // above entry line
            bg_color: [0.0, 0.0, 0.0, 0.0], // transparent background
            text_color: entry_color,
            font_size: 11.0,
            anchor: LabelAnchor::Left,
        });
    }

    output
}

/// Compute render primitives for a single bracket leg.
fn compute_bracket_leg(
    leg: &BracketLeg,
    color: [f32; 4],
    style: &LineStyle,
    status_alpha: f32,
    hit_zone_kind: HitZoneKind,
    ctx: &ComputeContext<'_>,
) -> WidgetOutput {
    let mut output = WidgetOutput::empty();
    let y = ctx.camera.price_to_y(leg.price);

    let x_start = match leg.timestamp {
        Some(ts) => ctx.camera.time_to_x(ts as f64).max(0.0),
        None => 0.0,
    };
    let x_end = ctx.viewport.width as f32;

    let mut effective_color = color;
    effective_color[3] *= status_alpha;

    let half_width = leg.line_width * 0.5;

    // Line segments (style-dependent).
    match style {
        LineStyle::Solid => {
            output.lines.push(GridLineInstance {
                rect: [x_start, y - half_width, x_end, y + half_width],
                color: effective_color,
            });
        }
        LineStyle::Dashed { dash_len, gap_len } => {
            let mut x = x_start;
            while x < x_end {
                let seg_end = (x + dash_len).min(x_end);
                output.lines.push(GridLineInstance {
                    rect: [x, y - half_width, seg_end, y + half_width],
                    color: effective_color,
                });
                x += dash_len + gap_len;
            }
        }
        LineStyle::Dotted { dot_spacing } => {
            let dot_size = leg.line_width * 1.5;
            let mut x = x_start;
            while x < x_end {
                output.markers.push(GridLineInstance {
                    rect: [
                        x - dot_size * 0.5,
                        y - dot_size * 0.5,
                        x + dot_size * 0.5,
                        y + dot_size * 0.5,
                    ],
                    color: effective_color,
                });
                x += dot_spacing;
            }
        }
    }

    // Hit zone.
    let hit_tolerance = 6.0;
    output.hit_zones.push(HitZone {
        annotation_id: AnnotationId::NONE,
        rect: [x_start, y - hit_tolerance, x_end, y + hit_tolerance],
        kind: hit_zone_kind,
    });

    // Price label badge.
    if let Some(label_text) = &leg.label {
        output.labels.push(WidgetLabel {
            text: label_text.clone(),
            screen_x: ctx.viewport.width as f32 - 80.0,
            screen_y: y,
            bg_color: effective_color,
            text_color: [1.0, 1.0, 1.0, 1.0],
            font_size: 11.0,
            anchor: LabelAnchor::Right,
        });
    }

    output
}
```


#### compute_marker

```rust
/// Compute render primitives for a marker annotation.
fn compute_marker(marker: &MarkerAnnotation, ctx: &ComputeContext<'_>) -> WidgetOutput {
    let mut output = WidgetOutput::empty();

    let x = ctx.camera.time_to_x(marker.timestamp as f64);
    let y = ctx.camera.price_to_y(marker.price);

    // Skip if off-screen.
    let half = marker.size * 0.5;
    if x < -half || x > ctx.viewport.width as f32 + half
        || y < -half || y > ctx.viewport.height as f32 + half
    {
        return output;
    }

    // Render as a colored rectangle for now.
    // Future: SDF-based shapes via MarkerPipeline.
    output.markers.push(GridLineInstance {
        rect: [x - half, y - half, x + half, y + half],
        color: marker.color,
    });

    // Hit zone: circular area approximated as a square.
    let hit_radius = (marker.size * 0.5).max(8.0); // minimum 8px for clickability
    output.hit_zones.push(HitZone {
        annotation_id: AnnotationId::NONE,
        rect: [x - hit_radius, y - hit_radius, x + hit_radius, y + hit_radius],
        kind: HitZoneKind::MarkerIcon,
    });

    // Tooltip label (shown on hover, not always visible).
    if let Some(tooltip) = &marker.tooltip {
        output.labels.push(WidgetLabel {
            text: tooltip.clone(),
            screen_x: x + half + 4.0,
            screen_y: y - 8.0,
            bg_color: [0.1, 0.1, 0.15, 0.9],
            text_color: [0.9, 0.9, 0.9, 1.0],
            font_size: 10.0,
            anchor: LabelAnchor::Left,
        });
    }

    output
}
```


#### compute_text_note

```rust
/// Compute render primitives for a text note.
fn compute_text_note(note: &TextNote, ctx: &ComputeContext<'_>) -> WidgetOutput {
    let mut output = WidgetOutput::empty();

    let x = ctx.camera.time_to_x(note.timestamp as f64);
    let y = ctx.camera.price_to_y(note.price);

    // Estimate text dimensions.
    // Real text measurement happens in the iced overlay layer.
    // Here we estimate based on character count and font size.
    let char_width = note.font_size * 0.6;
    let max_width = note.max_width.unwrap_or(note.text.len() as f32 * char_width);
    let text_width = (note.text.len() as f32 * char_width).min(max_width);
    let line_count = if note.max_width.is_some() {
        ((note.text.len() as f32 * char_width) / max_width).ceil() as f32
    } else {
        1.0
    };
    let text_height = line_count * note.font_size * 1.3; // 1.3 line height

    let padding = 6.0;
    let bg_left = x - padding;
    let bg_top = y - padding;
    let bg_right = x + text_width + padding;
    let bg_bottom = y + text_height + padding;

    // Background rectangle.
    output.fills.push(GridLineInstance {
        rect: [bg_left, bg_top, bg_right, bg_bottom],
        color: note.background_color,
    });

    // Text label (rendered by iced overlay).
    output.labels.push(WidgetLabel {
        text: note.text.clone(),
        screen_x: x,
        screen_y: y,
        bg_color: [0.0; 4], // transparent (background already drawn)
        text_color: note.text_color,
        font_size: note.font_size,
        anchor: LabelAnchor::TopLeft,
    });

    // Hit zone: the background rectangle.
    output.hit_zones.push(HitZone {
        annotation_id: AnnotationId::NONE,
        rect: [bg_left, bg_top, bg_right, bg_bottom],
        kind: HitZoneKind::NoteBody,
    });

    output
}
```

---

## 5. Integration with ChartScene

The widget system feeds into the existing `ChartScene` pipeline. This
section describes the integration points.


### 5.1 Compute Pipeline Extension

The existing `compute_chart_scene()` function gains a widget computation
step. The step runs after candle/volume/grid computation and before
crosshair computation.

```rust
/// Extended compute_chart_scene with widget support.
///
/// The widget computation step is a pure function that reads annotations
/// from ChartInput and produces WidgetOutput primitives that merge into
/// ChartScene.
pub fn compute_chart_scene(input: &ChartInput<'_>) -> ChartScene {
    // ... existing computation (candles, volumes, grid, VP) ...

    // ── Widget computation (new) ─────────────────────────────────
    let widget_buffers = compute_widget_outputs(input);

    // ... existing computation (crosshair, date labels) ...

    ChartScene {
        // ... existing fields ...

        // New fields for widget rendering:
        annotation_fills: widget_buffers.fills,
        annotation_lines: widget_buffers.lines,
        annotation_markers: widget_buffers.markers,
        annotation_labels: widget_buffers.labels,
        annotation_hit_zones: widget_buffers.hit_zones,

        // ... existing fields ...
    }
}

/// Compute all widget outputs for the current frame.
///
/// Iterates all visible annotations for the current symbol and timeframe,
/// computes their render primitives, and flattens the results into
/// three GPU buffers (fills, lines, markers) plus label and hit-zone
/// collections.
fn compute_widget_outputs(input: &ChartInput<'_>) -> WidgetBuffers {
    let ctx = ComputeContext {
        camera: input.camera,
        data: input.data,
        viewport: Viewport {
            width: input.viewport_width,
            height: input.viewport_height,
        },
        theme: &input.widget_theme,
        snap_fn: &input.snap_fn,
        candle_duration_ms: estimate_candle_duration(input.data),
        collapse_gaps: input.collapse_gaps,
        separator_y: compute_separator_y(input),
        dpi_scale: input.dpi_scale,
    };

    let mut buffers = WidgetBuffers::default();

    for annotation in input.annotations {
        // Timeframe filter.
        if !annotation.should_render_on(input.timeframe) {
            continue;
        }

        if let Some(output) = compute_annotation(annotation, &ctx) {
            buffers.fills.extend(output.fills);
            buffers.lines.extend(output.lines);
            buffers.markers.extend(output.markers);
            buffers.labels.extend(output.labels);
            buffers.hit_zones.extend(output.hit_zones);
        }
    }

    buffers
}

/// Flattened widget render data for GPU upload.
#[derive(Default)]
struct WidgetBuffers {
    fills: Vec<GridLineInstance>,
    lines: Vec<GridLineInstance>,
    markers: Vec<GridLineInstance>,
    labels: Vec<WidgetLabel>,
    hit_zones: Vec<HitZone>,
}
```


### 5.2 ChartScene Extensions

New fields on `ChartScene` for widget rendering. These are added alongside
the existing fields, not replacing them.

```rust
pub struct ChartScene {
    // ── Existing fields (unchanged) ─────────────────────────────
    pub projection: glam::Mat4,
    pub viewport_width: u32,
    pub viewport_height: u32,
    pub background_color: [f32; 4],
    pub candles: Option<Vec<CandleInstance>>,
    pub candle_count: usize,
    pub volumes: Option<Vec<VolumeInstance>>,
    pub volume_count: usize,
    pub grid_instances: Vec<GridLineInstance>,
    pub x_labels: Vec<AxisLabel>,     // existing type, unchanged
    pub y_labels: Vec<AxisLabel>,     // existing type, unchanged
    pub levels: Vec<LevelRender>,
    pub crosshair: Option<CrosshairRender>,
    pub level_preview_y: Option<f32>,
    pub separator_y: f32,
    pub date_labels: Vec<DateLabel>,
    pub volume_profile_instances: Vec<GridLineInstance>,
    pub generations: SceneGenerations,

    // ── New widget fields ───────────────────────────────────────

    /// Annotation background fills: bracket zones, note backgrounds.
    /// Rendered at Layer 4 (behind candles, so fills don't obscure price data).
    pub annotation_fills: Vec<GridLineInstance>,

    /// Annotation lines: level lines, bracket legs, selection highlights.
    /// Rendered at Layer 7 (on top of fills, behind markers).
    pub annotation_lines: Vec<GridLineInstance>,

    /// Annotation markers: icons, drag handles, dots.
    /// Rendered at Layer 8 (on top of lines, behind crosshair).
    pub annotation_markers: Vec<GridLineInstance>,

    /// Annotation text labels: price badges, R:R ratios, note text.
    /// Rendered at Layer 10 (iced overlay, above all GPU content).
    pub annotation_labels: Vec<WidgetLabel>,

    /// Hit zones for annotation interaction.
    /// NOT rendered. Used by the interaction layer for hit-testing.
    pub annotation_hit_zones: Vec<HitZone>,
}
```

**Why three separate fields instead of `Vec<AnnotationRender>`**: The
earlier annotations plan (`annotations/05-rendering.md`) used an enum
`AnnotationRender` with variants that get flattened into three buffers
before GPU upload. This intermediate step adds allocation and iteration
overhead for no benefit. By writing directly to three flat buffers during
compute, we skip the intermediate allocation. The renderer just uploads
the three buffers -- no flattening step needed.


### 5.3 SceneGenerations Extension

```rust
#[derive(Clone, Debug, Default)]
pub struct SceneGenerations {
    // ── Existing fields (unchanged) ─────────────────────────────
    pub candles: u64,
    pub camera: u64,
    pub grid: u64,
    pub levels: u64,
    pub crosshair: u64,
    pub theme: u64,

    // ── New ─────────────────────────────────────────────────────
    /// Annotation data generation counter.
    pub annotations: u64,
}
```


### 5.4 ChartInput Extensions

```rust
pub struct ChartInput<'a> {
    // ── Existing fields (unchanged) ─────────────────────────────
    pub symbol: &'a str,
    pub data: &'a dyn CandleData,
    pub camera: &'a Camera2D,
    pub viewport_width: u32,
    pub viewport_height: u32,
    pub dpi_scale: f32,
    pub background_color: [f32; 4],
    pub bull_color: [f32; 4],
    pub bear_color: [f32; 4],
    pub volume_bull_color: [f32; 4],
    pub volume_bear_color: [f32; 4],
    pub grid_color: [f32; 4],
    pub crosshair: Option<(f32, f32)>,
    pub levels: &'a [HorizontalLevel],
    pub collapse_gaps: bool,
    pub timeline_border_ratio: f32,
    pub volume_scale: f32,
    pub show_volume_profile: bool,
    pub dirty: &'a DirtyFlags,
    pub level_tool: &'a LevelTool,

    // ── New fields ──────────────────────────────────────────────

    /// Annotations for the current symbol. Pre-filtered by the app
    /// layer to include only annotations relevant to this chart's
    /// symbol. Timeframe filtering happens during compute.
    pub annotations: &'a [Annotation],

    /// Widget theme colors for default annotation styling.
    pub widget_theme: Theme,

    /// Current chart timeframe. Used for annotation timeframe filtering.
    pub timeframe: Timeframe,

    /// OHLC snap function for interactive annotations.
    pub snap_fn: &'a dyn Fn(f32) -> Option<(f32, usize)>,
}
```


### 5.5 Renderer Integration

The renderer (`midas-render`) gains three new `GridPipeline` instances for
annotation sub-layers. Each pipeline gets its own instance buffer and draws
at the correct z-order.

```rust
/// In midas-render::ChartRenderer:
pub struct ChartRenderer {
    // ── Existing pipelines (unchanged) ──────────────────────────
    candle_pipeline: CandlePipeline,
    volume_pipeline: VolumePipeline,
    grid_pipeline: GridPipeline,
    volume_profile_pipeline: GridPipeline,
    crosshair_pipeline: GridPipeline,

    // ── New annotation pipelines (all reuse GridPipeline) ───────
    /// Layer 4: zone fills, note backgrounds (behind candles).
    annotation_fill_pipeline: GridPipeline,
    /// Layer 7: level lines, bracket legs, selection highlights.
    annotation_line_pipeline: GridPipeline,
    /// Layer 8: markers, drag handles, dots.
    annotation_marker_pipeline: GridPipeline,
}
```

Draw order during `render_draw_calls()`:

```
Layer 1: grid_pipeline.draw()              -- grid lines
Layer 2: volume_pipeline.draw()            -- volume bars
Layer 3: volume_profile_pipeline.draw()    -- VP histogram
Layer 4: annotation_fill_pipeline.draw()   -- annotation fills    [NEW]
         (zone fills render BEHIND candles so they don't obscure price data)
Layer 5: candle_pipeline.draw() (wicks)    -- candle wicks
Layer 6: candle_pipeline.draw() (bodies)   -- candle bodies
Layer 7: annotation_line_pipeline.draw()   -- annotation lines    [NEW]
Layer 8: annotation_marker_pipeline.draw() -- annotation markers  [NEW]
Layer 9: crosshair_pipeline.draw()         -- crosshair lines
Layer 10: (iced overlay)                   -- all text labels
```

No new shaders are needed. All three annotation pipelines are instances
of the existing `GridPipeline`, which renders axis-aligned colored
rectangles -- exactly what annotations need.

---

## 6. Dirty Flag Integration

### 6.1 New Generation Counter

Add `annotations: u64` to `DirtyFlags`:

```rust
pub struct DirtyFlags {
    // ── Existing (unchanged) ────────────────────────────────────
    pub camera: u64,
    pub candles: u64,
    pub indicators: u64,
    pub crosshair: u64,
    pub levels: u64,
    pub grid: u64,
    pub theme: u64,

    // ── New ─────────────────────────────────────────────────────
    /// Annotation data changed (added/moved/deleted/style changed).
    pub annotations: u64,
}
```


### 6.2 Mark Functions

```rust
impl DirtyFlags {
    // ── Existing (updated) ──────────────────────────────────────
    // mark_data(), mark_indicators(), mark_crosshair(),
    // mark_levels(), mark_theme()

    /// Updated mark_camera to cascade to annotations.
    /// Annotation screen positions depend on camera.price_to_y(),
    /// so camera changes must trigger annotation recomputation.
    pub fn mark_camera(&mut self) {
        self.camera += 1;
        self.candles += 1;
        self.grid += 1;
        self.annotations += 1;  // NEW: annotation Y positions depend on camera
    }

    // ── New ─────────────────────────────────────────────────────

    /// An annotation was created, modified, or deleted.
    pub fn mark_annotations(&mut self) {
        self.annotations += 1;
    }

    /// Updated mark_all to include annotations.
    pub fn mark_all(&mut self) {
        self.camera += 1;
        self.candles += 1;
        self.indicators += 1;
        self.crosshair += 1;
        self.levels += 1;
        self.grid += 1;
        self.theme += 1;
        self.annotations += 1;  // NEW
    }

    /// Updated mark_theme to cascade to annotations.
    /// Theme change affects annotation colors (since annotations
    /// may use theme defaults).
    pub fn mark_theme(&mut self) {
        self.theme += 1;
        self.candles += 1;
        self.indicators += 1;
        self.levels += 1;
        self.grid += 1;
        self.annotations += 1;  // NEW: theme colors affect annotations
    }
}
```


### 6.3 DirtyTracker Extension

```rust
impl DirtyTracker {
    // ── Existing (unchanged) ────────────────────────────────────
    // needs_camera_update(), needs_candle_rebuild(), etc.

    // ── New ─────────────────────────────────────────────────────

    /// Returns `true` if annotation data has changed since last ack.
    pub fn needs_annotation_rebuild(&self, current: &DirtyFlags) -> bool {
        self.last_seen.annotations != current.annotations
    }

    /// Updated any_dirty to include annotations.
    pub fn any_dirty(&self, current: &DirtyFlags) -> bool {
        self.needs_camera_update(current)
            || self.needs_candle_rebuild(current)
            || self.needs_indicator_rebuild(current)
            || self.needs_crosshair_update(current)
            || self.needs_level_rebuild(current)
            || self.needs_grid_rebuild(current)
            || self.needs_theme_rebuild(current)
            || self.needs_annotation_rebuild(current)  // NEW
    }
}
```


### 6.4 Cascade Rules

| Event | Flags incremented | Rationale |
|---|---|---|
| Annotation created/deleted/modified | `annotations` | Direct annotation data change |
| Annotation price/time changed (drag) | `annotations` | Annotation position changed |
| Camera moved (pan/zoom/resize) | `camera`, `candles`, `grid`, `annotations` | Screen positions change; annotation Y positions depend on `camera.price_to_y()`, so annotation compute must re-run. |
| Theme changed | `theme`, `candles`, `indicators`, `levels`, `grid`, `annotations` | Theme colors affect annotation default colors |
| Data changed | `candles`, `indicators` | Annotations don't directly depend on candle data (except indicators), but indicator annotations will re-compute. Level/bracket positions don't change when data changes. |

**Important**: Annotation pipelines use **always-upload** initially (same
strategy as crosshair and VP pipelines). The instance count is small
(budget: ~2000 instances, ~64 KB). The dirty-flag check gates whether
re-computation happens in `compute_widget_outputs()`, but the GPU upload
always writes the current buffer. This avoids the timing issues that
affected grid pipeline optimization.

When annotation instance counts grow large enough to warrant conditional
upload, gate behind `needs_annotation_rebuild()`. But start with
always-upload for correctness and simplicity.

---

## 7. Module Structure

```
midas-chart/src/
├── widget/
│   ├── mod.rs              # Re-exports, AnnotationId, AnnotationKind,
│   │                       # Annotation, Presence, Point, BoundingBox
│   │
│   ├── compute.rs          # ComputeContext, WidgetOutput, WidgetBuffers,
│   │                       # compute_annotation() dispatch, compute_widget_outputs(),
│   │                       # hit_test_annotation() dispatch
│   │
│   ├── level.rs            # HorizontalLevel, LineStyle, LevelExtend,
│   │                       # compute_level(), hit_test_level(), bounding_box_level()
│   │                       # (migrated from levels.rs)
│   │
│   ├── order_bracket.rs    # OrderBracket, BracketLeg, BracketSide, BracketStatus,
│   │                       # compute_bracket(), compute_bracket_leg(),
│   │                       # hit_test_bracket(), bounding_box_bracket()
│   │
│   ├── text_note.rs        # TextNote, compute_text_note(),
│   │                       # hit_test_text_note(), bounding_box_text_note()
│   │
│   ├── marker.rs           # MarkerAnnotation, MarkerIcon, compute_marker(),
│   │                       # hit_test_marker(), bounding_box_marker()
│   │
│   ├── hit_test.rs         # HitResult, HitZone, HitZoneKind,
│   │                       # hit_test_all_annotations() (top-level dispatcher)
│   │
│   └── theme.rs            # Theme struct and defaults
│
├── indicators/
│   ├── mod.rs              # IndicatorKind, IndicatorConfig, IndicatorOutput
│   ├── gerchik_atr.rs      # GerchikAtrConfig, compute_gerchik_atr()
│   │                       # (migrated from gerchik_atr.rs in Phase 2)
│   └── volume_profile.rs   # VolumeProfileConfig, compute_volume_profile()
│                           # (migrated from volume_profile.rs in Phase 3)
│
├── levels.rs               # RETAINED during migration (deprecated, re-exports
│                           # from widget/level.rs). Removed after Phase 1.
│
├── gerchik_atr.rs          # RETAINED during migration (deprecated, re-exports
│                           # from indicators/gerchik_atr.rs). Removed after Phase 2.
│
├── compute.rs              # Gains compute_widget_outputs() call
├── input.rs                # Gains annotations, widget_theme, timeframe fields
├── scene.rs                # Gains annotation_fills/lines/markers/labels/hit_zones
├── dirty.rs                # Gains annotations: u64
├── state.rs                # ChartState gains selected_annotation: Option<AnnotationId>
└── interaction.rs          # Gains DrawingBracket, DraggingBracketLeg,
                            # DraggingNote, PlacingMarker modes
```


### 7.1 Module Visibility Rules

```rust
// midas-chart/src/widget/mod.rs

// Public types (exported through midas-chart public API):
pub use self::compute::{ComputeContext, Viewport, WidgetOutput, compute_annotation};
pub use self::hit_test::{HitResult, HitZone, HitZoneKind};
pub use self::level::{HorizontalLevel, LevelExtend, LineStyle};
pub use self::marker::{MarkerAnnotation, MarkerIcon};
pub use self::order_bracket::{
    BracketLeg, BracketSide, BracketStatus, OrderBracket,
};
pub use self::text_note::TextNote;
pub use self::volume_profile::VolumeProfileConfig;
pub use self::indicator::{IndicatorOverlay, IndicatorType};
pub use self::theme::Theme;

// These are the core types:
mod compute;
mod hit_test;
mod indicator;
mod level;
mod marker;
mod order_bracket;
mod text_note;
mod theme;
mod volume_profile;

// AnnotationId, AnnotationKind, Annotation, Presence, Point, BoundingBox
// are defined directly in mod.rs (they are small and central).
```

**Why types like AnnotationId are in mod.rs**: These are imported by every
sub-module. Placing them in mod.rs avoids circular imports and makes the
dependency direction clear: `mod.rs` defines core types, sub-modules import
and use them.


### 7.2 Crate Boundary: What Goes Where

| Type/Function | Crate | Reason |
|---|---|---|
| `Annotation`, `AnnotationKind`, `Presence` | `midas-chart` | Core data types, framework-agnostic |
| `ComputeContext`, `WidgetOutput` | `midas-chart` | Part of the compute pipeline |
| `compute_annotation()`, `hit_test_annotation()` | `midas-chart` | Pure functions, need `Camera2D` |
| `HorizontalLevel`, `OrderBracket`, etc. | `midas-chart` | Variant data types |
| `AnnotationStore` (CRUD, ID gen) | `midas-app` | Storage/persistence is I/O |
| `OrderAnnotationLink` (broker bridge) | `midas-app` | Needs both chart and broker |
| JSON save/load | `midas-app` | I/O belongs in app shell |
| `annotation_fill_pipeline` etc. | `midas-render` | GPU resource management |
| `Timeframe` (used in `visible_timeframes`) | `midas-core` | Shared enum |

**Future crate extraction**: If `midas-chart/src/widget/` exceeds ~2000
lines or ~10 types, extract it to `midas-widget`. This is a mechanical
operation: `cargo new midas-widget`, move files, add `pub use` re-exports
in `midas-chart`. The public API does not change.

---

## 8. Per-Symbol Storage Model

The widget system defines annotation *types* and *compute logic*. Storage
and persistence are the app layer's responsibility.


### 8.1 AnnotationStore (App Layer)

The `AnnotationStore` is a simple wrapper around a `HashMap` keyed by symbol.
It lives in `midas-app`, not `midas-chart`.

```rust
/// Per-symbol annotation storage.
///
/// Owns all annotations for all symbols. Charts query by symbol to
/// get `&[Annotation]` slices for compute_chart_scene().
///
/// Lives in midas-app (the application shell). NOT in midas-chart
/// (which is sans-IO and doesn't own data).
pub struct AnnotationStore {
    /// Annotations indexed by symbol (e.g., "AAPL" -> Vec<Annotation>).
    symbols: HashMap<String, Vec<Annotation>>,
    /// Next ID counter. Monotonically increasing across all symbols.
    /// Global uniqueness prevents confusion when annotations are
    /// moved between symbols (unlikely but possible).
    next_id: u64,
    /// Whether any data has changed since last save.
    dirty: bool,
}

impl AnnotationStore {
    /// Create an empty store.
    pub fn new() -> Self {
        Self {
            symbols: HashMap::new(),
            next_id: 1, // Start at 1, not 0 (AnnotationId(0) is sentinel)
            dirty: false,
        }
    }

    /// Get all annotations for a symbol. Returns an empty slice if
    /// the symbol has no annotations.
    pub fn get(&self, symbol: &str) -> &[Annotation] {
        self.symbols.get(symbol).map_or(&[], |v| v.as_slice())
    }

    /// Insert a new annotation for a symbol. Returns the assigned ID.
    pub fn insert(&mut self, symbol: &str, mut annotation: Annotation) -> AnnotationId {
        let id = AnnotationId(self.next_id);
        self.next_id += 1;
        annotation.id = id;
        self.symbols
            .entry(symbol.to_string())
            .or_default()
            .push(annotation);
        self.dirty = true;
        id
    }

    /// Remove an annotation by ID. Returns the removed annotation if found.
    pub fn remove(&mut self, symbol: &str, id: AnnotationId) -> Option<Annotation> {
        let annotations = self.symbols.get_mut(symbol)?;
        let pos = annotations.iter().position(|a| a.id == id)?;
        self.dirty = true;
        Some(annotations.remove(pos))
    }

    /// Get a mutable reference to a specific annotation.
    pub fn get_mut(&mut self, symbol: &str, id: AnnotationId) -> Option<&mut Annotation> {
        let annotations = self.symbols.get_mut(symbol)?;
        self.dirty = true;
        annotations.iter_mut().find(|a| a.id == id)
    }

    /// Whether there are unsaved changes.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Mark as saved (clear dirty flag).
    pub fn mark_saved(&mut self) {
        self.dirty = false;
    }

    /// Restore the next-ID counter after loading from persistence.
    /// Sets it to max(all existing IDs) + 1.
    pub fn recalculate_next_id(&mut self) {
        let max_id = self
            .symbols
            .values()
            .flat_map(|v| v.iter())
            .map(|a| a.id.0)
            .max()
            .unwrap_or(0);
        self.next_id = max_id + 1;
    }
}
```


### 8.2 Cross-Chart Propagation

When a chart modifies an annotation (drag, delete, create), the change is
made directly in the `AnnotationStore`. All other charts showing the same
symbol see the change on the next frame because they all read from the
same store via `store.get(symbol)`.

No message passing, no event bus, no eventual consistency. The store is a
single mutable reference owned by `MidasApp`. The iced update loop is
single-threaded, so mutation and reading never conflict.

```
Chart A (AAPL, 5m):  &annotations = store.get("AAPL")
Chart B (AAPL, 1D):  &annotations = store.get("AAPL")
Chart C (MSFT, 5m):  &annotations = store.get("MSFT")

User drags level on Chart A:
  → store.get_mut("AAPL", id).price = new_price
  → dirty.mark_annotations()
  → Next frame: Chart B reads the updated price. Done.
```


### 8.3 Timeframe-Aware Rendering

Charts render annotations filtered by their own timeframe:

```rust
// In compute_widget_outputs():
for annotation in input.annotations {
    if !annotation.should_render_on(input.timeframe) {
        continue; // skip annotations not visible on this timeframe
    }
    // ... compute ...
}
```

An annotation with `visible_timeframes: Some(vec![M5, M15])` renders on
5-minute and 15-minute charts but is skipped on daily charts. An annotation
with `visible_timeframes: None` (the default) renders on all charts.

---

## 9. Error Handling and Edge Cases

### 9.1 Empty Data

When `data.len() == 0`, compute functions must not panic. Indicator
compute functions return `WidgetOutput::empty()`. Levels and brackets
still render (they depend on price, not data). Markers anchored to
specific timestamps may be off-screen but should not crash.

### 9.2 Extreme Zoom

At extreme zoom levels, a single candle may span thousands of pixels.
Dashed line generation must cap the number of segments to prevent
allocation blowup:

```rust
const MAX_DASH_SEGMENTS: usize = 1000;

// In dashed line generation:
let mut x = x_start;
let mut segment_count = 0;
while x < x_end && segment_count < MAX_DASH_SEGMENTS {
    // ... generate segment ...
    segment_count += 1;
}
```

### 9.3 Annotation at Price Range Boundary

A level at exactly `price_high` or `price_low` should still render. The
margin check (`y < -margin || y > height + margin`) handles this by
allowing a small buffer zone beyond the viewport.

### 9.4 Zero-Width Line

`line_width: 0.0` should not be allowed. Enforce a minimum of 0.5 logical
pixels in the compute function. This prevents invisible-but-hit-testable
annotations.

### 9.5 NaN/Infinity Prices

Annotations with `NaN` or `infinity` prices must be filtered out during
compute. The `Camera2D::price_to_y()` function already handles these
gracefully (returns 0.0 for degenerate ranges), but explicit filtering
prevents nonsensical render data:

```rust
fn is_valid_price(price: f64) -> bool {
    price.is_finite() && price.abs() < 1e15
}
```

### 9.6 Concurrent Modification

The `AnnotationStore` is accessed through `&mut` references in the iced
single-threaded update loop. There is no concurrent modification risk.
If multi-threaded access becomes necessary in the future (e.g., background
data loading), wrap the store in `RwLock`. But this is explicitly NOT
needed now and should NOT be added preemptively.

---

## 10. Extension Points and Escape Hatches

### 10.1 Custom Annotation Escape Hatch

If downstream code needs to define annotation types not in `AnnotationKind`,
add a `Custom` variant:

```rust
// NOTE: This example shows the escape hatch concept. In practice,
// VolumeProfile and Indicator are NOT in AnnotationKind -- they use
// a separate IndicatorKind enum stored per-chart (see Phase 2).
pub enum AnnotationKind {
    Level(HorizontalLevel),
    OrderBracket(OrderBracket),
    TextNote(TextNote),
    Marker(MarkerAnnotation),
    // Escape hatch (not implemented until >15 types):
    Custom(Box<dyn CustomAnnotation>),
}

/// Trait for user-defined annotation types.
///
/// Pays the vtable cost only when used. Built-in variants still
/// use direct enum dispatch.
pub trait CustomAnnotation: Send + Sync + fmt::Debug {
    fn compute(&self, ctx: &ComputeContext<'_>) -> WidgetOutput;
    fn hit_test(&self, point: Point, ctx: &ComputeContext<'_>) -> Option<HitResult>;
    fn bounding_box(&self, ctx: &ComputeContext<'_>) -> Option<BoundingBox>;
    fn clone_box(&self) -> Box<dyn CustomAnnotation>;
}
```

**When to add this**: When there is a concrete need for a custom annotation
type that does not fit any existing variant. Not before. Adding it now
would require implementing `Serialize`/`Deserialize` for `dyn CustomAnnotation`
(non-trivial), `Clone` via `clone_box()` (boilerplate-heavy), and testing
the vtable dispatch path. All cost, zero benefit until a real use case
exists.


### 10.2 New Built-In Variant

Adding a new built-in variant (e.g., `TrendLine`, `FibonacciRetracement`,
`Rectangle`) requires:

1. Define the data struct in a new file (e.g., `widget/trend_line.rs`).
2. Add the variant to `AnnotationKind`.
3. Add `compute_`, `hit_test_`, and `bounding_box_` functions.
4. Add match arms in `compute_annotation()` and `hit_test_annotation()`.
5. Add to `mod.rs` re-exports.

The compiler enforces steps 4-5 via exhaustive match. Steps 1-3 are
mechanical. This is the "adding a new type is harder but adding a new
operation is easy" trade-off of the enum approach, and it is the correct
trade-off for a system with 6-10 types and 3+ operations.


### 10.3 New GPU Pipeline

If a new widget type needs rendering beyond axis-aligned rectangles (e.g.,
diagonal lines, curves, SDF shapes), add a new pipeline to `midas-render`
and a new buffer field to `WidgetOutput`:

```rust
pub struct WidgetOutput {
    pub fills: Vec<GridLineInstance>,
    pub lines: Vec<GridLineInstance>,
    pub markers: Vec<GridLineInstance>,
    pub labels: Vec<WidgetLabel>,
    pub hit_zones: Vec<HitZone>,
    // Future:
    // pub line_strips: Vec<LineStripInstance>,  // for indicator curves
    // pub sdf_markers: Vec<SdfMarkerInstance>,  // for shaped markers
}
```

The `WidgetOutput` struct is additive -- new fields don't break existing
code. The renderer ignores empty buffers.


### 10.4 Plugin System

A full plugin system (dynamic loading of `.dll`/`.so` annotation types) is
out of scope. If ever needed, it would use the `Custom(Box<dyn CustomAnnotation>)`
variant with a plugin registry that maps type names to factory functions.
This is a large feature with security, stability, and versioning implications
that should not be designed speculatively.

---

## 11. Migration Path from Existing Code

### 11.1 Phase 1: Create widget/ Module, Migrate Levels

**Goal**: `HorizontalLevel` moves from `levels.rs` to `widget/level.rs`.
Zero behavior change. All existing tests pass.

**Steps**:

1. Create `widget/mod.rs` with `AnnotationId`, `AnnotationKind`, `Annotation`,
   `Presence`, and re-exports.
2. Move `HorizontalLevel`, `LevelIcon`, `LineStyle`, `LevelExtend` to
   `widget/level.rs`.
3. Create `widget/compute.rs` with `ComputeContext`, `WidgetOutput`,
   and `compute_level()`.
4. Leave `levels.rs` as a thin re-export module (deprecated) so that
   existing imports don't break.
5. Create stub files for other variants (`order_bracket.rs`, `text_note.rs`,
   `marker.rs`, `indicator.rs`, `volume_profile.rs`) with types but no
   compute functions.
6. Update `DirtyFlags` and `DirtyTracker` with the `annotations` counter.
7. Update `ChartScene` with annotation buffer fields.
8. Update `ChartInput` with `annotations`, `widget_theme`, `timeframe` fields.
9. Update `compute_chart_scene()` to call `compute_widget_outputs()`.

**Verification**: All existing tests pass. Levels render identically.
The `widget/` module compiles. New tests cover `AnnotationId`, `Presence`,
and `compute_level()`.


### 11.2 Phase 2: Migrate Gerchik ATR to Indicator Architecture

**Goal**: The Gerchik ATR indicator moves from a standalone compute function
to the separate indicator architecture (`indicators/` module, per-chart config).
Indicators are NOT `AnnotationKind` variants — they are a distinct category.
See `06-implementation-roadmap.md` Phase 2 for the full plan.

**Steps**:

1. Create `indicators/` module with `IndicatorKind` enum and `IndicatorConfig`.
2. For `IndicatorKind::GerchikAtr`, delegate to the existing
   `compute_gerchik_atr()` logic.
3. Wrap the output in `WidgetOutput` (the Gerchik ATR produces a text
   label, no GPU geometry).
4. Leave `gerchik_atr.rs` as a deprecated re-export.

**Verification**: Gerchik ATR badge still appears in the top-right corner.
Tests pass.


### 11.3 Phase 3+: New Widget Types

Phases 3+ add OrderBracket, TextNote, Marker, and VolumeProfile compute
functions. Each phase is independently shippable. See the
`annotations/08-implementation-order.md` document for the detailed phased
rollout.

---

## 12. Performance Budget

### 12.1 Instance Budgets

| Metric | Budget | Rationale |
|---|---|---|
| Max annotations per symbol | 500 | Generous for manual trading |
| Max GridLineInstances from annotations | 2,000 | 500 annotations x ~4 instances avg |
| GPU upload per frame (annotation buffers) | ~64 KB | 2,000 x 32 bytes = 64 KB |
| Max dashed-line segments per line | 1,000 | Extreme zoom safety cap |
| Max labels per frame | 100 | iced overlay performance limit |

### 12.2 Compute Time Budget

| Operation | Budget | Note |
|---|---|---|
| `compute_widget_outputs()` (all annotations) | < 0.5ms | Simple coordinate transforms |
| `hit_test_all_annotations()` (per mouse event) | < 0.1ms | Early-exit on first hit |
| GPU buffer upload (3 annotation pipelines) | < 0.2ms | 64 KB is trivial for modern GPUs |

### 12.3 Memory Budget

| Item | Per-Annotation | For 500 Annotations |
|---|---|---|
| `Annotation` struct (stack) | ~256 bytes | ~128 KB |
| Heap allocations (strings, labels) | ~64 bytes avg | ~32 KB |
| `WidgetOutput` (per-frame, transient) | varies | ~128 KB peak |
| GPU instance buffers (3 pipelines) | N/A | ~192 KB (3 x 64 KB) |

**Total memory footprint**: ~480 KB for 500 annotations. Negligible against
the 200 MB budget for 20 charts.


### 12.4 Scaling to 20 Charts

With 20 charts open (the target scenario), widget computation happens
independently per chart. 20 x 0.5ms = 10ms if fully sequential. In practice:

- Most charts share annotations (per-symbol, not per-chart), so the same
  annotations are computed multiple times with different cameras. This is
  correct (different zoom/pan = different screen positions).
- The 0.5ms budget is conservative. Typical annotation counts (5-20) per
  chart complete in < 0.05ms.
- GPU uploads are trivially parallel across charts (each chart has its
  own pipeline instances).

---

## Appendix A: Design Decision Log

This appendix records the key design decisions, the alternatives considered,
and the rationale for each choice.

### A.1 Enum Dispatch vs Trait Objects

**Decision**: Enum dispatch for all built-in annotation types.

**Alternatives**:
- Trait objects (`Vec<Box<dyn Widget>>`): 10-12x slower in tight loops,
  no exhaustive match, heap-scattered data.
- Generic type parameter: Cannot have heterogeneous collections.
- Visitor pattern: Same exhaustiveness as enum match, more boilerplate.

**Rationale**: The set of annotation types is closed (6-8 types), controlled
by the same workspace, and iterated in tight loops (hit-testing, compute).
Enum dispatch is the clear winner. Escape hatch via `Custom(Box<dyn ...>)`
is documented but not implemented.

### A.2 Per-Symbol vs Per-Chart vs Per-Workspace Storage

**Decision**: Per-symbol storage.

**Alternatives**:
- Per-chart: Duplicate annotations across charts showing the same symbol.
  Sync becomes a problem.
- Per-workspace: A single flat list of all annotations. Requires symbol
  field on each annotation and filtering on every access.
- Per-(symbol, timeframe): More granular but harder to manage.

**Rationale**: Every major trading platform (TradingView, ThinkOrSwim,
Bloomberg, NinjaTrader) stores annotations per-symbol. A support level
at $185 is meaningful regardless of the chart showing it. The existing
`LevelStore` already uses this pattern.

### A.3 Three-Phase Pipeline (Compute -> Scene -> GPU) vs Two-Phase

**Decision**: Three-phase: annotation data -> WidgetOutput -> GPU buffers.

**Alternatives**:
- Two-phase (annotation data -> GPU buffers directly): Faster, but couples
  compute logic to GPU data layout.
- Four-phase (with explicit diff/patch step): Over-engineered for our
  annotation count.

**Rationale**: The three-phase pipeline matches the existing architecture
(`ChartState` -> `ChartScene` -> `midas-render`). `WidgetOutput` is the
serialization boundary, analogous to `ChartScene`. This maintains the
sans-IO property of `midas-chart`.

### A.4 Presence Enum vs Boolean Visible Flag

**Decision**: Three-state `Presence` enum (Active/Ghost/Hidden).

**Alternatives**:
- Boolean `visible: bool`: Only two states, no Ghost mode.
- Opacity float `alpha: f32`: Continuous, but doesn't capture the
  interactive/non-interactive distinction.
- Bevy's three-tier (`Visibility`, `InheritedVisibility`, `ViewVisibility`):
  Over-complex for non-hierarchical annotations.

**Rationale**: Ghost mode is essential for cross-chart sync (levels from
other timeframes appear dimmed and non-interactive) and for historical
brackets (closed orders visible as reference). A boolean loses this
semantic distinction. The Presence enum captures both visibility AND
interactivity in a single field, which is how the compute pipeline
actually queries it: "should I render?" (`is_visible()`) and "should I
hit-test?" (`is_interactive()`).

### A.5 WidgetOutput vs AnnotationRender Enum

**Decision**: Direct flat buffers in `WidgetOutput` instead of an
intermediate `AnnotationRender` enum.

**Alternatives**:
- `AnnotationRender` enum (from annotations/05-rendering.md): Each
  annotation produces one `AnnotationRender` variant, then a flatten step
  sorts into three GPU buffers.
- Tagged instances: Each `GridLineInstance` carries a layer tag, sorted
  before upload.

**Rationale**: The intermediate enum adds an allocation and iteration step
that produces exactly the same three buffers. Writing directly to flat
buffers during compute eliminates this overhead. The layer assignment is
determined at compute time (fills vs lines vs markers), not at render time.
There is no need for an intermediate representation.

### A.6 Free Functions vs Trait Methods for Compute Dispatch

**Decision**: Free functions dispatched via `match` in a central function.

**Alternatives**:
- Trait `WidgetCompute` implemented per variant struct: Creates two
  dispatch mechanisms (enum + trait), adds boilerplate.
- Trait `WidgetCompute` implemented on `AnnotationKind` enum directly:
  Puts all compute logic in one giant `impl` block, losing the per-file
  organization benefit.
- Methods on each variant struct: `HorizontalLevel::compute()`,
  `OrderBracket::compute()`, etc.: Reasonable, but the `match` dispatcher
  is needed anyway to route from `AnnotationKind`.

**Rationale**: The free-function pattern (`compute_level()`,
`compute_bracket()`, etc.) keeps each variant's logic in its own file,
avoids trait boilerplate, and works naturally with the enum dispatch. The
central `compute_annotation()` function is the only call site, so there is
no polymorphism benefit from a trait. The compiler's exhaustive match
ensures no variant is forgotten.

---

## Appendix B: Relationship to Existing Plan Documents

> **Note**: The `annotations/*.md` paths below refer to prior-art documents
> in `desktop/win/plan/annotations/` which this plan **supersedes**. They
> are cited for historical context only — do not use them as implementation
> references. The authoritative specifications are in `plan/widget-system/`.

This document supersedes and consolidates the following plan documents
for the core architecture layer:

| Document | Relationship |
|---|---|
| `annotations/01-architecture.md` | Superseded for architecture. Module layout and dependency graph updated here. |
| `annotations/02-core-types.md` | Superseded for core types. Types expanded and refined here (Presence, WidgetOutput, ComputeContext). |
| `annotations/05-rendering.md` | Rendering layer (Layer 6/7/8) preserved. `AnnotationRender` enum replaced by flat buffers. |
| `annotations/08-implementation-order.md` | Implementation phases preserved as Section 11. |
| `rust-widget-patterns-research.md` | Research conclusions applied throughout. |
| `cross-chart-sync-research.md` | Per-symbol storage model validated in Section 1.3 and Section 8. |
| `per-ticker-level-store.md` | Compatible. LevelStore pattern extended to AnnotationStore. |

Documents NOT superseded (still canonical):

| Document | Scope |
|---|---|
| `annotations/03-order-brackets.md` | OrderBracket data model, visual design, R:R computation |
| `annotations/04-interaction.md` | Interaction modes, drawing flow, keyboard shortcuts |
| `annotations/06-persistence.md` | JSON file format, save/load strategy |
| `annotations/07-order-bridge.md` | Annotation-to-broker mapping |

---

## Appendix C: Glossary

| Term | Definition |
|---|---|
| **Annotation** | Any user-created or system-created visual element on a chart. The umbrella term for levels, brackets, notes, markers, indicators, and VP overlays. |
| **AnnotationKind** | The enum that specifies which type of annotation it is. Each variant wraps a dedicated data struct. |
| **Presence** | Three-state visibility/interactivity flag: Active, Ghost, Hidden. |
| **WidgetOutput** | The render primitives a single annotation produces during compute. Contains fills, lines, markers, labels, and hit zones. |
| **ComputeContext** | The borrowed context passed to every widget compute function. Contains camera, data, theme, viewport, and snap function. |
| **GridLineInstance** | 32-byte GPU instance struct (`[f32; 4]` rect + `[f32; 4]` color). The universal render primitive for axis-aligned colored rectangles. Covers 90%+ of annotation rendering needs. |
| **ChartScene** | The framework-agnostic intermediate representation produced by `compute_chart_scene()`. The serialization boundary between chart logic and GPU rendering. |
| **Sans-IO** | Architecture pattern where core logic has zero I/O (no GPU, no filesystem, no network). Enables unit testing without GPU context. |
| **Generation counter** | A `u64` that is incremented on each change. Readers compare their last-seen value to the current value to detect changes. Solves the "who clears the boolean flag" problem. |
| **Per-symbol storage** | Annotations are stored in a `HashMap<String, Vec<Annotation>>` keyed by stock symbol. All charts showing the same symbol share one set of annotations. |
| **Enum dispatch** | Calling different functions based on which variant of an enum is active, using `match`. 10-12x faster than trait object dispatch for tight iteration loops. |
