# Data Model

This file is the reference for every new type introduced by the decorator system. The design rationale lives in [02-design-decisions.md](02-design-decisions.md); the implementation order lives in [06-implementation.md](06-implementation.md).

All data types (everything except `BadgeInstance` and `HitZoneKind`) derive `Clone + Debug + PartialEq + Serialize + Deserialize`. `HitZoneKind` must additionally preserve `Copy` — see [Hit zones](#hit-zones-hitzonekinddecorator-variant). `BadgeInstance` is `#[repr(C)] + Copy + Clone + Debug + bytemuck::Pod + bytemuck::Zeroable` — no serde, no `PartialEq`.

Decorator data lives under `midas-chart/src/widget/decorator/` (new module, Slice 2). `PriceLine` lives in `midas-chart/src/widget/price_line.rs` (also new). `BadgeInstance` lives in `midas-chart/src/instances.rs` next to the existing `GridLineInstance` at line 76.

---

## Core primitive: `PriceLine`

**Module**: `midas-chart/src/widget/price_line.rs`

`PriceLine` is the shared geometry of "a horizontal line at a given price." Every horizontal-price annotation — levels, bracket legs, alert lines, future additions — composes one. Decorators are attached by the wrapping domain type, not stored on the `PriceLine` itself, so the geometric primitive stays independent of its visual accessories.

```rust
// midas-chart/src/widget/price_line.rs
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PriceLine {
    pub price: f64,
    pub extent: LineExtent,
    pub stroke: LineStroke,
}
```

---

## Line stroke: `LineStyle`, `LineStroke`

**Module**: `midas-chart/src/widget/price_line.rs` (stroke), `midas-chart/src/widget/level.rs` (style, already existing — modified in Slice 1)

`LineStroke` bundles color + width + dash pattern. `LineStyle::Pattern` replaces the old fixed `Dashed`/`Dotted` variants with an SVG-style `[dash, gap, dash, gap, ...]` list. Empty pattern means solid.

```rust
// midas-chart/src/widget/price_line.rs
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LineStroke {
    pub color: [f32; 4],
    pub width: f32,
    pub style: LineStyle,
}

// midas-chart/src/widget/level.rs (replaces current Solid/Dashed/Dotted enum at line 42)
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum LineStyle {
    #[default]
    Solid,
    Pattern(SmallVec<[f32; 6]>),
}

impl LineStyle {
    pub fn dashed(dash: f32, gap: f32) -> Self {
        Self::Pattern(smallvec![dash, gap])
    }
    pub fn dotted(spacing: f32) -> Self {
        Self::Pattern(smallvec![1.0, spacing])
    }
    pub fn is_solid(&self) -> bool {
        matches!(self, Self::Solid) || matches!(self, Self::Pattern(p) if p.is_empty())
    }
}
```

---

## Line extent: `LineExtent`

**Module**: `midas-chart/src/widget/price_line.rs`

Where on the time axis the line is drawn. `FullWidth` is the default and matches current level behavior; the half-open and closed variants exist for future per-bracket bounds and time-limited alerts.

```rust
// midas-chart/src/widget/price_line.rs
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub enum LineExtent {
    #[default]
    FullWidth,
    RightFrom { timestamp: i64 },
    Between { start: i64, end: i64 },
}
```

---

## Decorator group: `DecoratorGroup`, `DecoratorItem`, `Visibility`

**Module**: `midas-chart/src/widget/decorator/group.rs`

A `DecoratorGroup` is a flex-laid container anchored to a point on a parent `PriceLine`. `group_id` is a domain-scoped identifier (unique within one annotation's decorator set, **not** globally unique) — it's the stable key used by the hover-persistence system and the click-routing layer to refer to one group without needing a pointer.

```rust
// midas-chart/src/widget/decorator/group.rs
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DecoratorGroup {
    pub group_id: u16,
    pub anchor: DecoratorAnchor,
    pub direction: FlexDirection,
    pub gap: f32,
    pub items: SmallVec<[DecoratorItem; 4]>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DecoratorItem {
    pub visibility: Visibility,
    pub action: Option<DecoratorAction>,
    pub content: ItemContent,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub enum Visibility {
    /// Always rendered.
    #[default]
    Always,
    /// Visible when the parent PriceLine is hovered.
    OnLineHover,
    /// Visible when the parent PriceLine OR any item in this group is
    /// hovered. Used for buttons that must stay alive after the pointer
    /// leaves the line and moves onto them.
    OnGroupHover,
}
```

---

## Anchors and direction: `DecoratorAnchor`, `FlexDirection`

**Module**: `midas-chart/src/widget/decorator/group.rs`

Anchors resolve against either the viewport edges (`LeftEdge`/`RightEdge`), camera time (`AtTimestamp`), or raw screen space (`AtScreenX`). The `y` component always comes from `camera.price_to_y(parent_line.price)` — anchors only control the `x` axis.

```rust
// midas-chart/src/widget/decorator/group.rs
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum DecoratorAnchor {
    LeftEdge,
    RightEdge,
    AtTimestamp(i64),
    AtScreenX(f32),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum FlexDirection {
    Row,
    Column,
}
```

When a `DecoratorGroup` is used as a nested `ItemContent::Stack`, its `anchor` field is **ignored** — the nested group's anchor is derived from the parent item's position during layout.

---

## Item content: `ItemContent`

**Module**: `midas-chart/src/widget/decorator/group.rs`

One of four shapes a decorator item can take. `Stack` is boxed to break the recursive size cycle (`DecoratorGroup` → `DecoratorItem` → `ItemContent::Stack(Box<DecoratorGroup>)`).

```rust
// midas-chart/src/widget/decorator/group.rs
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ItemContent {
    Badge(Badge),
    Button(Button),
    Stack(Box<DecoratorGroup>),
    Spacer(f32),
}
```

---

## Badges: `Badge`, `BadgeSegment`, `BadgeShape`, `BadgeBorder`

**Module**: `midas-chart/src/widget/decorator/badge.rs`

A `Badge` is one outlined shape containing one or more `BadgeSegment`s laid out left-to-right inside. Each segment has its own text and can override the parent `shape`/`fill` (used for the "black circle around 2" in the TP tag — one segment with `shape_override: Some(Circle)` and `fill_override: Some([0,0,0,1])`). `divider_color` draws a thin vertical line between adjacent segments.

```rust
// midas-chart/src/widget/decorator/badge.rs
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Badge {
    pub shape: BadgeShape,
    pub fill: [f32; 4],
    pub border: Option<BadgeBorder>,
    pub height: f32,
    pub padding: f32,
    pub segments: SmallVec<[BadgeSegment; 3]>,
    pub divider_color: Option<[f32; 4]>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BadgeSegment {
    pub text: String,
    pub text_color: [f32; 4],
    pub font_size: f32,
    pub min_width: Option<f32>,
    pub fill_override: Option<[f32; 4]>,
    pub shape_override: Option<BadgeShape>,
    pub action: Option<DecoratorAction>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum BadgeShape {
    Rect,
    Rounded { radius: f32 },
    Pill,
    PointLeft { point_width: f32 },
    PointRight { point_width: f32 },
    DoublePoint { point_width: f32 },
    Chevron { point_width: f32 },
    Circle,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BadgeBorder {
    pub color: [f32; 4],
    pub thickness: f32,
}
```

`BadgeShape` and `BadgeBorder` are `Copy` — their variants are plain `f32`s and they get passed into the GPU-instance emission loop by value. A `BadgeSegment` carrying its own `action` emits a hit zone covering just that segment's sub-rect (not the whole badge); see [Hit zones](#hit-zones-hitzonekinddecorator-variant) for the `item_path` encoding.

---

## Buttons: `Button`

**Module**: `midas-chart/src/widget/decorator/button.rs`

Buttons are simpler than badges: one shape, one glyph, fixed size. `hover_fill` is the fill color when the pointer is directly over this button; `None` means no hover state change.

```rust
// midas-chart/src/widget/decorator/button.rs
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Button {
    pub shape: BadgeShape,
    pub fill: [f32; 4],
    pub hover_fill: Option<[f32; 4]>,
    pub glyph: char,
    pub glyph_color: [f32; 4],
    pub glyph_size: f32,
    pub size: [f32; 2],
    pub border: Option<BadgeBorder>,
}
```

---

## Actions: `DecoratorAction`

**Module**: `midas-chart/src/widget/decorator/action.rs`

The sans-IO action vocabulary. Clicks on decorator items produce one of these variants inside a `ChartAction::DecoratorClick` (see [05-interaction.md](05-interaction.md) for the dispatch layer). The enum is `Copy` so that it fits inside `HitZoneKind::Decorator` without breaking that type's `Copy` derive.

```rust
// midas-chart/src/widget/decorator/action.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DecoratorAction {
    CloseAnnotation,
    CreateTakeProfit,
    CreateStopLoss,
    CycleEntryType,
    EditQuantity,
    EditPrice,
    ToggleLocked,
    Submit,
    Save,
    Custom(u32),
}
```

The `Custom(u32)` variant is reserved for future domain-specific actions that don't justify a named variant yet; the mapping from `u32` to meaning is owned by whichever annotation kind emitted it.

---

## GPU instance: `BadgeInstance`

**Module**: `midas-chart/src/instances.rs` (added alongside the existing `GridLineInstance` at line 76)

`BadgeInstance` is the per-instance vertex attribute consumed by the SDF badge pipeline. It lives in the **data model** rather than the rendering file because it's declared in `midas-chart` (sans-IO, no GPU deps); `midas-render` only *consumes* the slice through `ChartScene.badges`. Slice 2 lands the struct; Slice 4 wires it into `WidgetOutput`, `ChartScene`, and the `midas-render` pipeline.

64 bytes, 16-byte aligned. Layout is stable with the WGSL fragment shader — do not reorder without updating `badge.wgsl`.

```rust
// midas-chart/src/instances.rs
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct BadgeInstance {
    pub rect: [f32; 4],        // screen-space bounding box [x0, y0, x1, y1]
    pub fill: [f32; 4],        // linear RGBA
    pub border: [f32; 4],      // linear RGBA, alpha=0 means no border
    pub shape_id: u32,
    pub shape_param: f32,      // radius / point_width / unused per shape
    pub border_thickness: f32, // logical pixels, 0 = no border
    pub _pad: f32,
}
```

**`shape_id` mapping** (stable contract with `midas-render/shaders/badge.wgsl` — adding a new shape means appending a discriminant, never reordering):

| `shape_id` | `BadgeShape` variant            | `shape_param` meaning           |
|-----------:|---------------------------------|---------------------------------|
| 0          | `Rect`                          | unused                          |
| 1          | `Rounded { radius }`            | `radius`                        |
| 2          | `Pill`                          | unused (derived: `min(w,h)/2`)  |
| 3          | `PointLeft { point_width }`     | `point_width`                   |
| 4          | `PointRight { point_width }`    | `point_width`                   |
| 5          | `DoublePoint { point_width }`   | `point_width`                   |
| 6          | `Chevron { point_width }`       | `point_width`                   |
| 7          | `Circle`                        | unused (derived: `min(w,h)/2`)  |

A compile-time test (`badge_instance_shape_id_matches_enum`) enforces this table via `#[repr(u8)]` discriminants plus a `const`-asserted mapping — reordering or dropping a variant causes a build break, not a silent shader mismatch.

---

## Hit zones: `HitZoneKind::Decorator` variant

**Module**: `midas-chart/src/widget/hit_test.rs` (modified in Slice 3)

The decorator-system contract with the hit-test/interaction layer. Every visible decorator item with `action: Some(_)` emits one `HitZone` whose `kind` is the `Decorator` variant below.

### `Copy` must be preserved

`HitZoneKind` is currently `#[derive(Clone, Copy, Debug, PartialEq, Eq)]` at `hit_test.rs:46`. The `Copy` derive is **load-bearing**: dropping it cascades through the entire hover/hit-test surface. `HitZone` is `Copy`, and every `(AnnotationId, HitZoneKind)` pair stored on `ComputeContext::hovered_annotation`, `ChartInput::hovered_annotation`, and `ChartState::hovered_annotation` is copied by value inside pattern matches in `interaction/mod.rs`, `compute/mod.rs`, and `chart_widget.rs`. A heap-allocating field (like `SmallVec<[u8; 4]>`) forces a `Clone` migration across all those sites and erases the zero-cost destructure pattern — rejected.

### The variant

`item_path` is a fixed-capacity breadcrumb wrapped in a private `ItemPath` newtype. The newtype enforces the invariant that the unused tail of the fixed array is zeroed — **critical** because `HitZoneKind` derives `PartialEq` and `Hash`, and two paths with equal length but differing garbage bytes past the length byte would compare unequal, silently breaking click dedup and hover-state lookup. Four bytes covers the deepest realistic nesting (`group → stack → segment` = 3 levels), with one byte of headroom.

```rust
// midas-chart/src/widget/hit_test.rs

/// Fixed-capacity breadcrumb into a nested decorator layout. Construction
/// zeroes the unused tail, so derived `PartialEq`/`Hash` are sound regardless
/// of how the caller passed the path in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ItemPath {
    bytes: [u8; 4],
    len: u8,
}

impl ItemPath {
    /// Construct from a slice. Panics in debug if `path.len() > 4`.
    pub fn new(path: &[u8]) -> Self {
        debug_assert!(path.len() <= 4, "ItemPath max depth is 4");
        let mut bytes = [0u8; 4];
        let len = path.len().min(4);
        bytes[..len].copy_from_slice(&path[..len]);
        Self { bytes, len: len as u8 }
    }

    pub fn as_slice(&self) -> &[u8] { &self.bytes[..self.len as usize] }
    pub fn len(&self) -> usize      { self.len as usize }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HitZoneKind {
    LevelLine,
    BracketEntry,
    BracketTP,
    BracketSL,
    BracketStopTrigger,
    BracketZone,
    MarkerIcon,
    NoteBody,
    VolumeProfileBar,
    /// Click on a decorator item. `item_path` is a fixed-capacity breadcrumb
    /// (max depth 4) wrapped in `ItemPath` to guarantee zero-tail invariant.
    Decorator {
        group_id: u16,
        item_path: ItemPath,
        action: DecoratorAction,
    },
}
```

Pattern-match sites read the slice via `item_path.as_slice()`. The `ItemPath` newtype is `Copy` (because `[u8; 4]` and `u8` are both `Copy`), so `HitZoneKind` stays `Copy` and the entire hover/hit-test cascade is preserved.

**Path encoding**:

| Target                               | Encoded slice                                       | `len` |
|--------------------------------------|-----------------------------------------------------|------:|
| Top-level item                       | `[item_idx]`                                        | 1     |
| Segment inside a top-level badge     | `[item_idx, segment_idx]`                           | 2     |
| Item inside a nested `Stack`         | `[stack_item_idx, child_item_idx]`                  | 2     |
| Segment inside a nested stack child  | `[stack_item_idx, child_item_idx, segment_idx]`     | 3     |

The five existing bracket-button variants (`BracketSubmit`, `BracketSave`, `BracketToggleSL`, `BracketCancel`, `BracketCancelSL`) are **kept in place through Slice 8a-ii** (both the data-model migration and the visual decorator emissions coexist with the legacy button hit zones) and then deleted in **Slice 8b** once the interactive cutover routes through `ChartAction::DecoratorClick`. Slice 3 only adds `Decorator`, it does not remove anything.

---

## Domain-type composition

After the migration slices (7 for levels, 8a for brackets), the two existing domain types compose `PriceLine` + decorators. Their old shapes are described in [01-research.md](01-research.md); the shapes below are the **post-migration** state.

### `HorizontalLevel` (Slice 7 end state)

**Module**: `midas-chart/src/levels.rs`

```rust
// midas-chart/src/levels.rs
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HorizontalLevel {
    pub id: u64,
    pub line: PriceLine,
    pub label: Option<String>,
    pub icon: LevelIcon,
}

impl HorizontalLevel {
    pub fn to_decorators(&self, locked: bool) -> Vec<DecoratorGroup> { /* ... */ }
}
```

The renderer-side `widget::level::HorizontalLevel` is **deleted entirely** in Slice 7; only the persisted `levels::HorizontalLevel` above remains. The old `color`, `line_width`, `style` fields move inside `line: PriceLine` (as `line.stroke.{color, width, style}`). `label` and `icon` stay on the domain struct as source data and are converted into decorators by `to_decorators()`.

`locked: bool` is **not** a field on `HorizontalLevel` — it lives on the `Annotation` wrapper at `widget/mod.rs:146`. `to_decorators()` takes `locked` as an explicit parameter so the level compute entry point can forward the wrapper's lock state, mirroring how the annotation layer already treats lock as a presence-level attribute rather than a per-kind field.

`compute_level()` keeps its current five-argument signature `(level, annotation_id, ctx, alpha, locked)` — the `locked` parameter is preserved — but the body becomes a thin wrapper that calls `compute_price_line_geometry()` for the line itself and `compute_decorator_group()` once per group returned by `level.to_decorators(locked)`.

### `BracketLeg` (Slice 8a-i end state)

**Module**: `midas-chart/src/widget/order_bracket/mod.rs`

```rust
// midas-chart/src/widget/order_bracket/mod.rs
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BracketLeg {
    pub line: PriceLine,
    pub role: LegRole,
    pub projected_pnl: Option<f64>,
    pub projected_pnl_pct: Option<f64>,
}
```

The old `color`, `line_width`, `style`, and `label` fields on `BracketLeg` are gone — color and stroke live inside `line: PriceLine`, and labels are now computed at decorator-build time by the constructor functions in `widget/order_bracket/decorators.rs` (`entry_decorator_group()`, `tp_decorator_group()`, `sl_decorator_group()`). Projected P&L stays on the leg as source data because it's wire-data that changes per tick and is read by multiple downstream consumers (decorators, tooltips, the trade-size widget).

`OrderBracket` as a whole continues to own a `Vec<BracketLeg>` plus the cross-leg state (side, entry type, quantity, draft/saved flags). Only the per-leg shape changes.
