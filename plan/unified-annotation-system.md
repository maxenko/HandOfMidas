# Feature: Unified Annotation System (Levels + Brackets)

## Overview

Converge the level and order bracket rendering/interaction pipelines into a single code path. Currently levels and brackets are rendered, hit-tested, and dragged via separate functions with duplicated logic — causing bugs when one path gets a fix the other doesn't (cursor change, hover highlight, screen-space clamping). This refactor routes all annotations through the same compute → hit-test → drag pipeline via `WidgetOutput`.

## Research Summary

### Codebase Analysis

**Current state:** Two parallel systems with ~70% structural overlap:

| Capability | Levels | Brackets |
|---|---|---|
| Rendering | `compute_levels()` → `LevelRender` (GPU struct) | `compute_bracket()` → `WidgetOutput` |
| Hit-testing | `hit_test_levels()` (interaction/mod.rs) | `hit_test_bracket_legs()` (interaction/mod.rs) |
| Cursor change | Direct price_to_y loop in mouse_interaction() | `bracket_leg_at_cursor()` helper |
| Drag mode | `LevelToolMode::Dragging` (separate state machine) | `InteractionMode::DraggingBracketLeg` |
| Hover highlight | None (levels don't highlight on hover) | `hovered_bracket_leg` → width +1.0 |
| Storage | `LevelStore` (legacy, per-ticker) | `AnnotationStore` |

**Bridge code:** `old_levels_to_annotations()` (`chart_widget.rs`) converts levels to `Annotation` wrappers for the interaction layer, but NOT for rendering.

**Shared already:** `LineStyle` enum, `HitZone`/`HitZoneKind` types (including `LevelLine` variant), `CursorIcon`, `Camera2D` transforms, `GridLineInstance` GPU primitives, `Annotation` wrapper with `Presence`.

**Key enum names** (verified against codebase):
- `AnnotationKind::Level(HorizontalLevel)` — NOT `HorizontalLevel` variant
- `LevelToolMode::Dragging { level_id, grab_offset }` — inside `LevelTool.mode` field
- `HitZoneKind::LevelLine` — already exists in `hit_test.rs`

### Best Practices & Idiomatic Approach

- **Enum dispatch** (`AnnotationKind` with `match`) is correct for this codebase — closed set of types, needs serde, needs exhaustiveness checking. Trait objects add vtable cost and lose compile-time completeness.
- **Single compute dispatch**: One `match` on `AnnotationKind` in `compute_widget_annotations()`. Every new variant gets a compiler error.
- **Shared line helper, not trait**: A shared `emit_price_line()` function called by both level and bracket compute. Composition over inheritance.
- **Keep lightweight price-to-y hit-testing**: `mouse_interaction()` and `update()` don't have access to cached hit zones (iced's `draw()` takes `&State`). The price-to-y approach is fast and correct — unify it into ONE function that handles all annotation types rather than two separate functions.

## Design Decisions

### Decision: Route levels through WidgetOutput (retire LevelRender)

**Context**: Levels use `LevelRender` (GPU struct), brackets use `WidgetOutput`. This split means levels can't participate in hover highlight, unified hit zones, or the widget compute pipeline.
**Recommendation**: Add `compute_level()` returning `WidgetOutput`. Keep `compute_levels()` temporarily during transition. Remove `LevelRender` path once visual parity is confirmed.
**Confidence**: high

### Decision: Unified hit-test via price-to-y (not cached hit zones)

**Context**: Hit zones are computed in `draw()` but needed in `update()` and `mouse_interaction()`. `draw()` takes `&State` (immutable), so caching is complex.
**Options**:
1. Cache hit zones in `RefCell` — adds complexity, lifetime issues
2. Recompute hit zones in `update()` — requires full `ComputeContext`, expensive
3. Keep price-to-y approach, unify into single function — fast, simple, already works

**Recommendation**: Option 3. Replace `hit_test_levels()` + `hit_test_bracket_legs()` + `bracket_leg_at_cursor()` with a single `hit_test_annotation()` that iterates annotations and checks `price_to_y` distance for all types. This matches the existing pattern but eliminates the duplication.
**Confidence**: high

### Decision: Unified drag mode

**Context**: `LevelToolMode::Dragging` and `InteractionMode::DraggingBracketLeg` are separate state paths for identical behavior.
**Recommendation**: Unify into `InteractionMode::DraggingAnnotation { annotation_id, element: HitZoneKind, grab_offset, clamp_ctx: Option<BracketClampCtx> }`. The `LevelToolMode::Dragging` variant is removed; `LevelTool` retains only `Idle`/`Placing`.
**Confidence**: high

### Decision: Defer LevelStore → AnnotationStore migration

**Context**: Full storage convergence requires data migration and config format changes.
**Recommendation**: Keep the `old_levels_to_annotations()` bridge. It works and costs nothing. Migrate in a follow-up track.
**Confidence**: high

## Implementation Plan

### Slice 1: Level compute via WidgetOutput + field rename

**Goal**: Levels produce `WidgetOutput` (lines, labels, hit zones, hover highlight) through the same pipeline as brackets. Rename `hovered_bracket_leg` to `hovered_annotation` globally.

**Depends on**: None

**Files to create or modify**:
- `desktop/win/crates/midas-chart/src/widget/level.rs` — add `compute_level()` function
- `desktop/win/crates/midas-chart/src/widget/order_bracket/mod.rs` — move `segmented_line()` to shared location or re-export; update `hovered_bracket_leg` references
- `desktop/win/crates/midas-chart/src/compute/mod.rs` — add `AnnotationKind::Level` dispatch in `compute_widget_annotations()`
- `desktop/win/crates/midas-chart/src/state/mod.rs` — rename `hovered_bracket_leg` → `hovered_annotation`
- `desktop/win/crates/midas-chart/src/input.rs` — rename field in `ChartInput`
- `desktop/win/crates/midas-chart/src/widget/compute.rs` — rename field in `ComputeContext`
- `desktop/win/crates/midas-app/src/chart_widget.rs` — update field references
- `desktop/win/crates/midas-chart/src/compute/tests.rs` — update field name
- `desktop/win/tests/integration_gate.rs` — update field name
- `desktop/win/crates/midas-chart/src/widget/order_bracket/tests.rs` — update field references

**Key implementation details**:

`compute_level()` in `widget/level.rs`:
- Takes `(level: &HorizontalLevel, annotation_id: AnnotationId, ctx: &ComputeContext, alpha: f32)`
- Produces: 1 segmented line (via `segmented_line()`), 1 hit zone (`HitZoneKind::LevelLine`), 1 price label
- Reads `ctx.hovered_annotation` to apply hover highlight (+1.0 width)
- Locked levels still render but emit hit zones with `cursor: CursorIcon::Crosshair` (no drag affordance)

In `compute_widget_annotations()`, add level dispatch:
```rust
match &ann.kind {
    AnnotationKind::Level(level) => {
        // Locked levels still render (labels, lines) but their hit zones
        // use CursorIcon::Crosshair (no drag affordance). Lock check is
        // in the hit-test/drag path, not here.
        let out = compute_level(level, ann.id, &ctx, alpha, ann.locked);
        merged.merge(out);
    }
    AnnotationKind::OrderBracket(bracket) => { /* existing */ }
    _ => {}
}
```

`segmented_line()` should be moved to `widget/line.rs` (new file) or `widget/level.rs` since both level and bracket compute need it.

**Testing**:
- `compute_level()` produces correct line count for Solid/Dashed/Dotted
- Hover highlight widens level line by +1.0 when `hovered_annotation` matches
- `LevelLine` hit zone is emitted at correct Y position with ±6px rect
- All existing tests pass with renamed `hovered_annotation` field

**Double-rendering gate**: Once `compute_widget_annotations()` handles `AnnotationKind::Level`, the old `compute_levels()` must stop emitting `LevelRender` for those same annotations. The simplest approach: remove the `AnnotationKind::Level` arm from `compute_levels()` so it returns an empty vec for levels. This immediately switches level rendering to the new `WidgetOutput` path. The old `scene.levels` → `GridLineInstance` conversion loop in `chart_widget.rs` (lines ~826-858) becomes a no-op (empty input) and can be cleaned up in Slice 4.

**Selection glow and drag ghost**: These are acceptable feature gaps during the transition period. `LevelRender` fields `is_selected` and `is_being_dragged`/`original_screen_y` have no equivalent in the initial `compute_level()`. They are assigned as pre-Slice 4 requirements (see Risks #2 and #3). During Slices 1-3, levels will lack selection glow and drag ghost — the old path no longer renders them since `compute_levels()` returns empty.

**Done when**: `cargo test --workspace` passes. Levels render via `WidgetOutput` with hit zones, hover highlight, correct line styles, and price labels. Old `compute_levels()` returns empty for `AnnotationKind::Level`.

---

### Slice 2: Unified hit-test function

**Goal**: Replace `hit_test_levels()`, `hit_test_bracket_legs()`, and `bracket_leg_at_cursor()` with a single function.

**Depends on**: Slice 1 (levels are now annotation-aware)

**Files to create or modify**:
- `desktop/win/crates/midas-chart/src/interaction/mod.rs` — add `hit_test_annotation()`, update `PendingDrag` handler and mouse press handler
- `desktop/win/crates/midas-app/src/chart_widget.rs` — replace `bracket_leg_at_cursor()` with unified `annotation_at_cursor()`

**Key implementation details**:

New function in `interaction/mod.rs`:
```rust
/// Unified hit-test for all annotation elements (levels, bracket legs).
/// Iterates annotations, computes price_to_y for each interactive element,
/// returns the closest within LEVEL_HIT_TOLERANCE_PX.
fn hit_test_annotation(
    annotations: &[Annotation],
    cursor_y: f32,
    camera: &Camera2D,
) -> Option<(AnnotationId, HitZoneKind, f64 /* grab_offset */, HitContext)>
```

Where `HitContext` carries optional bracket-specific data:
```rust
struct HitContext {
    entry_price: Option<f64>,
    side: Option<BracketSide>,
}
```

This function replaces:
- `hit_test_levels()` — levels produce `HitZoneKind::LevelLine` results
- `hit_test_bracket_legs()` — brackets produce `BracketTP`/`BracketSL`/`BracketEntry`/`BracketStopTrigger`

`PendingDrag` handler calls `hit_test_annotation()` once instead of two separate functions.

Similarly in `chart_widget.rs`, `annotation_at_cursor()` replaces both the level loop and `bracket_leg_at_cursor()` in `mouse_interaction()`.

**Testing**:
- Hit-test returns `LevelLine` for levels within tolerance
- Hit-test returns `BracketTP`/`BracketSL` for bracket legs
- Closest element wins when overlapping
- Locked annotations skipped
- Right-click still works (emits correct action for levels vs brackets)

**Done when**: One hit-test function handles all annotation types. Old `hit_test_levels()` and `hit_test_bracket_legs()` marked deprecated.

---

### Slice 3: Unified drag interaction mode

**Goal**: Replace `LevelToolMode::Dragging` and `InteractionMode::DraggingBracketLeg` with `InteractionMode::DraggingAnnotation`.

**Depends on**: Slice 2 (unified hit-test provides common result type)

**Files to create or modify**:
- `desktop/win/crates/midas-chart/src/state/mod.rs` — add `DraggingAnnotation`, deprecate `DraggingBracketLeg`
- `desktop/win/crates/midas-chart/src/interaction/mod.rs` — unified drag handler
- `desktop/win/crates/midas-chart/src/level_tool.rs` — remove `Dragging` from `LevelToolMode`
- `desktop/win/crates/midas-app/src/chart_widget.rs` — update mode checks in `mouse_interaction()`

**Key implementation details**:

```rust
InteractionMode::DraggingAnnotation {
    annotation_id: AnnotationId,
    element: HitZoneKind,
    grab_offset: f64,
    /// Bracket-specific clamping context. None for levels.
    clamp_ctx: Option<BracketClampCtx>,
}

struct BracketClampCtx {
    entry_price: f64,
    side: BracketSide,
}
```

Drag handler dispatches by `element`:
- `LevelLine` → no clamping, emit `ChartAction::DragLevel`
- `BracketTP`/`BracketSL`/`BracketEntry`/`BracketStopTrigger` → apply `clamp_bracket_leg_price()`, emit `ChartAction::DragBracketLeg`

`LevelToolMode` retains only `Idle` and `Placing`. The `Dragging` variant is removed.

**Testing**:
- Level drag through unified mode produces `DragLevel` action
- Bracket leg drag produces `DragBracketLeg` with clamping
- `mouse_interaction()` shows `ResizingVertically` during `DraggingAnnotation`
- Ghost level preview during level drag still works via `drag_price_override`

**Done when**: Both levels and brackets drag through `DraggingAnnotation`. `LevelToolMode::Dragging` removed. `DraggingBracketLeg` replaced.

---

### Slice 4: Remove deprecated code paths

**Goal**: Delete old level-specific rendering and duplicate hit-test functions.

**Depends on**: Slices 1-3 (all unified paths working and tested)

**Pre-Slice 4 requirements** (must be done before cleanup):
- Add selection glow rendering to `compute_level()` — read `selected_annotation: Option<AnnotationId>` from `ComputeContext` (new field), emit a wider semi-transparent highlight line when selected.
- Add drag ghost rendering to `compute_level()` — accept `drag_price_override` and render a faint line at the original position during drag.

**Files to create or modify**:
- `desktop/win/crates/midas-chart/src/compute/mod.rs` — remove `compute_levels()`, remove `LevelRender` from `ChartScene`
- `desktop/win/crates/midas-chart/src/instances.rs` — remove `LevelRender` struct
- `desktop/win/crates/midas-chart/src/scene.rs` — remove `levels: Vec<LevelRender>` field
- `desktop/win/crates/midas-chart/src/interaction/mod.rs` — remove `hit_test_levels()`, `hit_test_bracket_legs()`
- `desktop/win/crates/midas-app/src/chart_widget.rs` — remove `old_levels_to_annotations()` bridge, remove old `scene.levels` → `GridLineInstance` conversion loop (~lines 826-858)
- `desktop/win/crates/midas-app/src/app/views.rs` — rewrite `build_level_labels_overlay()` and `compute_level_renders()` to read from `WidgetOutput` labels instead of `LevelRender`

**Note**: `midas-render` has zero references to `LevelRender` — it does not need modification. The actual consumers of `LevelRender` are `chart_widget.rs` (GPU line conversion) and `views.rs` (iced label overlay).

**Testing**:
- Visual regression: levels render identically (line style, color, width, label position)
- `cargo test --workspace` passes
- `cargo clippy --workspace` clean, no dead code warnings
- No `LevelRender` references remain in codebase

**Done when**: All annotations flow through `WidgetOutput`. No separate render path for levels.

---

### Dependency Summary

```
Slice 1 (level compute + rename) ──────────────────┐
                                                     │
Slice 2 (unified hit-test) ─── depends on 1 ────────┤
                                                     ├─→ Slice 4 (cleanup)
Slice 3 (unified drag) ─── depends on 2 ────────────┤
```

Sequential execution recommended (Slices 2 and 3 touch overlapping files).

## Risks & Unknowns

1. **LevelRender consumers** (audited): `midas-render` has zero `LevelRender` references. The actual consumers are `chart_widget.rs` (converts `scene.levels` to `GridLineInstance` for GPU, ~lines 826-858) and `views.rs` (`build_level_labels_overlay()` and `compute_level_renders()` for iced text overlays). Both must be updated in Slice 4.

2. **Level selection rendering** (assigned: pre-Slice 4): `LevelRender` has `is_selected: bool`. The new `compute_level()` must render a selection indicator when `ChartState::selected_level` matches. Requires adding `selected_annotation: Option<AnnotationId>` to `ComputeContext`. Acceptable gap during Slices 1-3 transition.

3. **Level drag ghost line** (assigned: pre-Slice 4): `LevelRender` has `is_being_dragged` and `original_screen_y` for ghost line feedback. `compute_level()` needs a `drag_price_override` parameter (same pattern as `chart_widget.rs` level drag). Acceptable gap during Slices 1-3 transition.

4. **Double rendering during transition**: Both `compute_levels()` (old) and `compute_widget_annotations()` (new) will render levels during Slices 1-3. Must gate the old path with a flag or ensure only one emits GPU primitives.

5. **Level editor popup**: `editing_level_id` and level property editing (color, label, lock) must still work after levels render via WidgetOutput. This is unaffected since editing modifies the `HorizontalLevel` data, which `compute_level()` reads.

## Testing Strategy

- **Per-slice**: Each slice adds tests for new unified functions alongside existing tests.
- **Transition period**: During Slices 1-3, both paths exist. Manual visual comparison confirms parity.
- **Final regression**: Slice 4 runs full `cargo test --workspace` + `cargo clippy` + visual verification.

## Non-Goals / Out of Scope

- **LevelStore → AnnotationStore migration**: Storage convergence deferred. Bridge stays.
- **New annotation types**: TextNote, Marker, TrendLine not in this refactor.
- **Level creation tool changes**: `LevelTool` placement mode unchanged.
- **Annotation persistence on startup**: Loading saved annotations from disk is separate.
- **Level property editor UI**: Editor popup is app-layer, unaffected by render/interaction refactor.

## Review Notes

**`segmented_line()` relocation**: Currently in `order_bracket/mod.rs`. Must be moved to shared location (e.g., `widget/line.rs`) in Slice 1 so `compute_level()` can use it without depending on the bracket module.

**Naming consistency**: `hovered_bracket_leg` is renamed to `hovered_annotation` in Slice 1 (not deferred to later) because `compute_level()` needs the generic name immediately.

**DraggingAnnotation is unification, not simplification**: The handler still dispatches by `HitZoneKind` for clamping logic. The value is consistency (one state machine path, one code path to fix), not fewer lines of code.
