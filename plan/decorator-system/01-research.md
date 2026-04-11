# Research Summary

This file audits the existing codebase surface the decorator system touches. See [00-index.md](00-index.md) for the overall plan.

## Codebase Analysis

### Three near-duplicate "line at a price" types

Every annotation that draws a horizontal stroke today re-implements the same skeleton — a price, a stroke, a label, and a hand-picked subset of decorations. The three structs are:

| Field | `levels::HorizontalLevel` (persist) at `midas-chart/src/levels.rs:103` | `widget::level::HorizontalLevel` (render) at `midas-chart/src/widget/level.rs:20` | `BracketLeg` (render) at `midas-chart/src/widget/order_bracket/mod.rs:78` |
|---|---|---|---|
| `price: f64` | yes | yes | yes |
| color | `[f32;4]` | `[f32;4]` | `Option<[f32;4]>` (theme fallback) |
| `line_width: f32` | yes | yes | yes |
| `style` | — | `LineStyle` (Solid/Dashed/Dotted) | `LineStyle` |
| horizontal extent | full width only | `LevelExtend::{FullWidth, RightFrom, Between}` | `timestamp: Option<i64>` — `None` = full-width, `Some(t)` = right-from timestamp `t` |
| `label: Option<String>` | yes | yes | yes |
| `icon: LevelIcon` | yes | yes | — |
| `projected_pnl` / `pct` | — | — | yes |
| `locked: bool` | yes (persist-side) | — (lives on `Annotation` wrapper) | — |
| `id: u64` | yes (persist-side) | — | — |

The shape is: **geometry (price + extent + stroke) + 0..N decorations**. Each struct inlines a different hand-picked subset, and the renderers (`compute_level()` at `widget/level.rs:156` and `compute_bracket()` in `widget/order_bracket/mod.rs`) each re-implement label emission, icon compositing, hover highlight, and selection glow.

### Shared line helper already exists

`segmented_line()` at `widget/level.rs:90` is called by both the level and bracket compute paths. Geometry is already shared — this refactor finishes the job for decorations.

### `LineStyle` is rigid

Current definition at `widget/level.rs:42`:

```rust
pub enum LineStyle {
    #[default] Solid,
    Dashed { dash_len: f32, gap_len: f32 },
    Dotted { dot_spacing: f32 },
}
```

This can only express fixed two-phase rhythms. A `[1, 2, 1, 2]` sparse dot or `[6, 3, 1, 3]` dash-dot requires a new enum variant each time. SVG `stroke-dasharray` (a single `Vec<f32>` walked cyclically) is the standard solution. See [Design Decisions](02-design-decisions.md) for the `LineStyle::Pattern` replacement.

### Data-flow plumbing audit

The unified-annotation refactor already wired most of what the decorator system needs — but the three fields are NOT all in the same place, and it matters for Slice 5 (hover persistence):

- **Only `hovered_annotation: Option<(AnnotationId, HitZoneKind)>` is on `ChartState`** — at `midas-chart/src/state/mod.rs:182`.
- **`selected_annotation` and `drag_ghost` are on `ChartInput` and `ComputeContext` ONLY** — at `midas-chart/src/input.rs:18` and `midas-chart/src/widget/compute.rs:20`. They are sourced from app-side state owned outside `ChartState` and injected per-frame via `ChartInput`.
- Two-pass dispatch lives in `compute_widget_annotations()` at `compute/mod.rs:1077` — non-hovered pass, then hovered-on-top pass.

This asymmetry means the new `hovered_decorator_groups` field for cross-frame hover persistence must live on `ChartState` alongside `hovered_annotation`, not piggyback on the `ChartInput` pathway used for `selected_annotation` / `drag_ghost`. See [Interaction](05-interaction.md).

### `AnnotationKind` dispatch is partial today

`AnnotationKind` at `widget/mod.rs:124` has four variants: `Level(HorizontalLevel)`, `OrderBracket(Box<OrderBracket>)`, `TextNote(TextNote)`, `Marker(MarkerAnnotation)`. **Only `Level` and `OrderBracket` are dispatched in `compute_widget_annotations()` at `compute/mod.rs:1077` today**; `TextNote` and `Marker` exist as enum variants with no compute path (they fall through a `_ => {}` arm). The decorator system targets the two dispatched variants; growing `TextNote` / `Marker` is out of scope.

### `Annotation` wrapper owns `locked`

`Annotation` at `widget/mod.rs:146` has fields `id, kind, presence, visible_timeframes, locked: bool, created_at, modified_at`. The `locked` flag lives on the wrapper, **NOT** on the inner `HorizontalLevel`. This matters because `compute_level()` at `widget/level.rs:156` takes 5 args — `(level, annotation_id, ctx, alpha, locked: bool)` — where the `locked` argument is plumbed in by the call site in `compute/mod.rs` from the wrapper. The decorator system inherits this pattern: per-annotation decorator layout receives `locked` from the wrapper, not from the inner geometry.

### `HitZoneKind` Copy constraint

`HitZoneKind` at `widget/hit_test.rs:46` is declared `#[derive(Clone, Copy, Debug, PartialEq, Eq)]`. Variants (14 total): `LevelLine`, `BracketEntry`, `BracketTP`, `BracketSL`, `BracketStopTrigger`, `BracketZone`, `MarkerIcon`, `NoteBody`, `VolumeProfileBar`, `BracketSubmit`, `BracketSave`, `BracketToggleSL`, `BracketCancel`, `BracketCancelSL`.

**The `Copy` derive is load-bearing.** `HitZoneKind` flows through `hovered_annotation: Option<(AnnotationId, HitZoneKind)>` on `ChartState`, `ChartInput`, and `ComputeContext`, and every code site that destructures that tuple assumes copy semantics. Introducing a `HitZoneKind::Decorator { group_id, item_index, ... }` variant MUST preserve `Copy` — meaning its payload has to be `Copy` (plain integers / small enums, no `String`, no `Vec`, no `Box`). If `Copy` is dropped, the cascade touches every `(AnnotationId, HitZoneKind)` use site. This is a hard constraint on the decorator button payload design in [Data Model](03-data-model.md).

`HitZone` itself at `widget/hit_test.rs:82` is `{ annotation_id, rect: [f32;4], kind: HitZoneKind, cursor: CursorIcon }`. The last five `HitZoneKind` variants (`BracketSubmit`, `BracketSave`, `BracketToggleSL`, `BracketCancel`, `BracketCancelSL`) are already purpose-specific button hit zones — the hit-zone system already supports "clickable decorator element." Generalizing those five into one `HitZoneKind::Decorator` variant is consolidation, not invention.

### `WidgetOutput` is the emission target

At `widget/compute.rs:72`:

```rust
pub struct WidgetOutput {
    pub fills: Vec<GridLineInstance>,
    pub lines: Vec<GridLineInstance>,
    pub markers: Vec<GridLineInstance>,
    pub labels: Vec<WidgetLabel>,
    pub hit_zones: Vec<HitZone>,
}
```

The decorator system adds exactly one field: `badges: Vec<BadgeInstance>`. Everything else flows through existing lists. The `merge()` method at `widget/compute.rs:110` is extended to concatenate badge vectors too.

### No GPU text pipeline

Text today goes through iced's native `text()` + `container()` widgets in `build_crosshair_label_overlay()` at `midas-app/src/app/views.rs:2986`. This is a hard constraint: the decorator system's SDF pipeline draws **only shapes** — text is still emitted as `WidgetLabel`s that iced renders in an overlay pass on top. The implication: text inside a badge cannot be clipped by the badge shape — the badge must be sized to fit the text, not the other way around. In practice this is fine because decorators measure their text and lay out accordingly. See [Rendering](04-rendering.md).

### GPU pipeline layout

`ChartRenderer` at `midas-render/src/renderer.rs:41` holds one field per pipeline (`candle_pipeline`, `volume_pipeline`, `grid_pipeline`, `volume_profile_pipeline`, `crosshair_pipeline`). `ChartRenderer::new()` at `renderer.rs:51` initializes them. `ChartRenderer::render()` at `renderer.rs:77-144` iterates per-frame, uploads instance buffers conditionally on dirty flags, and calls `pipeline.draw()` in back-to-front order:

```
grid → volume → volume_profile → candle_wicks → candle_bodies → crosshair
```

**Badges will slot in AFTER `candle_bodies` and BEFORE `crosshair`.** That keeps badges above the price data (visible) but below the crosshair overlay (which must always win for UX reasons). Adding the pipeline is three touch points: one new struct field, one new init call in `new()`, one new upload + draw call in `render()`.

### Two `ChartScene` types — CRITICAL structural fact

There are **two separate `ChartScene` types** and any decorator field added to the IR must be added to both:

1. **Owned IR** at `midas-chart/src/scene.rs:20` — `pub struct ChartScene { ... }`. Produced by `midas-chart` compute, owns its instance `Vec`s, is the canonical framework-agnostic representation.
2. **Borrowed render-side** at `midas-render/src/renderer.rs:20` — `pub struct ChartScene<'a> { ... }`. Holds `&'a [CandleInstance]`, `&'a [GridLineInstance]`, etc. Consumed by `ChartRenderer::render()`.

The widget layer (`chart_widget.rs`) imports both under different names — `use midas_chart::scene::ChartScene` and `use midas_render::renderer::ChartScene as RenderScene` — and copies/borrows fields across the boundary per-frame. A `badges: Vec<BadgeInstance>` field on the owned IR must be paired with a matching `badges: &'a [BadgeInstance]` on the borrowed render-side struct, plus the bridging code in `chart_widget.rs`. Missing either half breaks the build or silently drops data.

### Instance-struct conventions

All GPU instance structs are `#[repr(C)] + bytemuck::Pod + bytemuck::Zeroable`. `GridLineInstance` at `midas-chart/src/instances.rs:76` is 32 bytes — `[f32;4] rect + [f32;4] color`. Adding a `BadgeInstance` that's larger is fine but the pattern expects the same derive stack.

### Persistence surfaces

Levels are persisted via **two** surfaces in `midas-app`:

- `midas-app/src/level_store/mod.rs:19` — legacy per-ticker level store (the `old_levels_to_annotations()` bridge feeds from here).
- `midas-app/src/annotation_persistence.rs` — the newer annotation persistence path.

**Verified during planning**: `HorizontalLevel` / `BracketLeg` are **not** persisted in the `midas-store` DuckDB crate. Grepping `desktop/win/crates/midas-store/` for `HorizontalLevel`, `BracketLeg`, `LineStyle`, `Level`, and `Annotation` returned no schema-level references; `midas-store` stores only candle data. The migration is therefore local to `midas-app` — the two writers that need updating are `midas-app/src/level_store/mod.rs:19` (TOML via `LevelConfig`) and `midas-app/src/annotation_persistence.rs` (JSON). See Slice 7 in [06-implementation.md](06-implementation.md).

### No naming conflicts

A grep for `Decorator`, `Badge`, `Tag` in the annotation/widget domain returned only `TextBadge` at `midas-chart/src/indicators/mod.rs:80` — an indicator summary label, unrelated to annotations. The names `Decorator`, `DecoratorGroup`, `Badge`, `Button`, `BadgeShape` are free.

## Best Practices & Idiomatic Approach

- **SVG `stroke-dasharray` convention** is the standard for pattern-based dash lines. A single `Vec<f32>` walked cyclically expresses every rhythm — solid (`[]`), dotted (`[1, 3]`), dashed (`[6, 3]`), dash-dot (`[6, 3, 1, 3]`), and arbitrary combinations. Shipping with this from day one avoids follow-up `Dashed2` / `DottedSparse` / `DashDot` variants.
- **SDF shapes in a single fragment shader** is how TradingView, Grafana, and every modern GPU charting library handles badges, pills, and rounded rects. One pipeline, one draw call per frame, shape selected by a per-instance `shape_id`. Anti-aliasing is free via the distance-to-edge derivative. See Íñigo Quílez's 2D distance functions reference (iquilezles.org) for the primitive library.
- **Flex-style layout** (row/column + gap + items) is the minimum viable layout model that handles the screenshots without inventing a custom coordinate system. A full constraint solver is overkill — all decorator groups are ≤6 items with intrinsic sizes.
- **Sans-IO action enum** preserves the crate boundary: `midas-chart` emits `DecoratorAction::{CloseAnnotation, CreateTakeProfit, ...}` as data, and `midas-app` maps them to broker commands or UI state changes. No IO type leaks into `midas-chart`. This mirrors how `ChartAction::DragBracketLeg` is already handled at `interaction/mod.rs:61`.
- **Hover state persisted across frames** is how iced, web browsers, and native UI kits all handle "buttons that stay expanded when the cursor leaves the parent line." The parent annotation remembers which of its decorator groups is currently expanded and keeps emitting OnHover items until the pointer clears both the parent line AND every expanded item. Without persistence, the first pixel of leaving the line collapses the button row and the cursor lands on nothing.
- **Two-pass compute** matches the existing pattern in `compute_widget_annotations()` at `compute/mod.rs:1077` — non-hovered annotations first, hovered annotations on top. Decorators extend this to a two-pass-per-annotation model: `Always` items first, `OnHover` items appended if the group is currently expanded.
