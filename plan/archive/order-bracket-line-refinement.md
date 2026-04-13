# Feature: Order Bracket Line Visual Refinement

## Overview

Refine order bracket line visuals: stop-loss lines become orange dotted, entry lines colored by side + order type (Long Stop = green, Long StopLimit = lime, Short Stop = red, Short StopLimit = pink-red), and add hover interaction feedback — bold highlight with foreground z-ordering and vertical-resize cursor when hovering movable lines. Currently, bracket lines all use the same green/red scheme, cursor doesn't change on hover, and there's no visual feedback for grabbability.

## Research Summary

### Codebase Analysis

**Color & Style Pipeline**: `OrderBracket::leg_style(role: LegRole)` at `order_bracket/mod.rs:192` dispatches on `LegRole` and `BracketStatus`. Colors come from constants at lines 170-176. `segmented_line()` at line 374 converts `LineStyle` (Solid/Dashed/Dotted) into `Vec<GridLineInstance>` rects for GPU rendering.

**Cursor Gap (critical bug)**: `CursorIcon` enum in `hit_test.rs` has `ResizeNS` and `Pointer`. Each bracket `HitZone` carries `cursor: CursorIcon::ResizeNS`. **But `mouse_interaction()` in `chart_widget.rs:519` never checks widget hit zones** — it only checks volume handles, timeline borders, and legacy levels. Bracket lines register correct cursor icons but the app layer never reads them.

**No Hover State**: `ChartState` has no hover tracking for bracket legs. `compute_bracket()` has no way to know which leg the cursor is over.

**Data Flow for Hover**: `draw()` has access to `state.chart_state` (local widget state) and constructs `ChartInput` from it. It already reads local state for `crosshair`, `timeline_border_ratio`, `volume_scale`, and `level_tool` — the hover field follows this established pattern: `state.chart_state` → `ChartInput` → `ComputeContext` → `compute_bracket()`.

**Entry Lines Not Draggable**: `compute_bracket()` emits hit zones for TP/SL (lines 479-514) but not entry. `InteractionMode::DraggingBracketLeg` supports only TP/SL via `LegRole`. Entry dragging is out of scope.

**Existing Tests**: `tests.rs` has ~30 `leg_style` tests. Tests that will break:
- `test_leg_style_cancelled_sl` (line 190) — asserts `LineStyle::Solid` for cancelled SL; will need to assert `Dotted` instead.
- `leg_style_entry_green_for_long` (line 301) — asserts green RGB for Long Market entry; still passes (Market/Limit colors unchanged).
- `leg_style_entry_red_for_short` (line 317) — asserts red RGB for Short Market entry; still passes.
- `test_bracket_zone_rects_active` (line 254) — only asserts alpha (0.06), not RGB; still passes.

Net: ~1 test directly broken by SL always-dotted (`test_leg_style_cancelled_sl`). Entry color tests still pass for default Market type. New tests needed for Stop/StopLimit entry colors and SL-always-dotted behavior.

### Best Practices & Idiomatic Approach

- Professional charting (TradingView, thinkorswim): 6-12px hit zones on 1-2px lines — codebase already uses ±6px.
- Hover highlight: boost `line_width` by +1.0 and set alpha to 1.0.
- Cursor affordance: `ResizeNS` for draggable price lines is standard.
- Z-ordering: render hovered bracket last so its lines draw on top.
- Sans-IO boundary: `CursorIcon` (chart enum) maps to `mouse::Interaction` (iced type) only in `chart_widget.rs`.

## Design Decisions

### Decision: Color scheme derivation

**Context**: Need distinct colors per leg role, side, and entry type.
**Recommendation**: Expand `leg_style()` match arms using new constants. Method already has `&self` access to `side` and `entry_type`. No API change needed.
**Confidence**: high

### Decision: SL always-dotted regardless of BracketStatus

**Context**: User wants SL lines to always be orange dotted. Currently, status dictates style (Draft = dashed, Pending = dotted, Active = solid).
**Recommendation**: For `LegRole::StopLoss`, force `LineStyle::Dotted` on all statuses. Status still modulates alpha and width. This breaks the status→style symmetry but is explicitly requested.
**Confidence**: high

### Decision: Cursor detection approach

**Context**: `mouse_interaction()` needs bracket leg proximity detection. It has `snapshot.bracket_annotations` and `camera` but not computed `WidgetOutput::hit_zones`.
**Recommendation**: Lightweight hit-test in `mouse_interaction()` using `camera.price_to_y()` + ±6px check. This mirrors the existing level-hover pattern at lines 565-574 of the same function.
**Confidence**: high

### Decision: Hover highlight data flow

**Context**: `compute_bracket()` runs in sans-IO `midas-chart` and needs to know which leg is hovered.
**Recommendation**: Add `hovered_bracket_leg: Option<(AnnotationId, HitZoneKind)>` to `ChartState`. In `update()`, set it on mouse-move. In `draw()`, read from `state.chart_state` (same as existing `crosshair`, `volume_scale` patterns) and pass through `ChartInput` → `ComputeContext` → `compute_bracket()`.
**Confidence**: high

## Implementation Plan

### Slice 1: Color Constants & leg_style() Update

**Goal**: SL = orange dotted (always), entry colored by (side, entry_type): Long Stop = green, Long StopLimit = lime, Short Stop = red, Short StopLimit = pink-red. TP stays green.

**Depends on**: None

**Files to create or modify**:
- `desktop/win/crates/midas-chart/src/widget/order_bracket/mod.rs` — new color constants, updated `leg_style()` match arms
- `desktop/win/crates/midas-chart/src/widget/order_bracket/tests.rs` — update existing color assertion tests, add new ones

**Key implementation details**:

New/modified color constants (linear RGBA):
```rust
/// Orange stop-loss line (all brackets).
const BRACKET_SL_COLOR: [f32; 4] = [1.0, 0.60, 0.0, 1.0];
/// Orange zone fill at 6% alpha (between entry and SL).
const BRACKET_SL_ZONE: [f32; 4] = [1.0, 0.60, 0.0, 0.06];
/// Green entry for Long Stop orders.
const BRACKET_LONG_STOP_COLOR: [f32; 4] = [0.20, 0.78, 0.35, 1.0];
/// Lime green entry for Long StopLimit orders.
const BRACKET_LONG_STOP_LIMIT_COLOR: [f32; 4] = [0.50, 0.90, 0.20, 1.0];
/// Red entry for Short Stop orders.
const BRACKET_SHORT_STOP_COLOR: [f32; 4] = [0.90, 0.25, 0.25, 1.0];
/// Pink-red entry for Short StopLimit orders.
const BRACKET_SHORT_STOP_LIMIT_COLOR: [f32; 4] = [0.90, 0.30, 0.50, 1.0];
```

`leg_style()` changes:
- **StopLoss**: Force `LineStyle::Dotted { dot_spacing: 4.0 }` on ALL statuses. Status still modulates alpha and width:
  ```rust
  LegRole::StopLoss => {
      let base_color = BRACKET_SL_COLOR;
      // SL is always dotted (user requirement). Width/alpha vary by status.
      let (width, alpha_mult) = match self.status {
          BracketStatus::Draft => (1.0, if self.saved { 0.65 } else { 0.50 }),
          BracketStatus::Pending => (1.0, 0.80),
          BracketStatus::PartialFill => (1.5, 0.90),
          BracketStatus::Active => (1.5, 1.0),
          BracketStatus::Closed => (1.0, 0.30),
          BracketStatus::Cancelled => (1.0, 0.20),
      };
      let style = LineStyle::Dotted { dot_spacing: 4.0 };
      // ... apply alpha_mult to color
  }
  ```
- **Entry**: Dispatch on `(self.side, self.entry_type)`:
  ```rust
  LegRole::Entry => match (self.side, self.entry_type) {
      (BracketSide::Long, EntryType::Stop) => BRACKET_LONG_STOP_COLOR,
      (BracketSide::Long, EntryType::StopLimit) => BRACKET_LONG_STOP_LIMIT_COLOR,
      (BracketSide::Long, _) => BRACKET_LONG_ENTRY_COLOR,
      (BracketSide::Short, EntryType::Stop) => BRACKET_SHORT_STOP_COLOR,
      (BracketSide::Short, EntryType::StopLimit) => BRACKET_SHORT_STOP_LIMIT_COLOR,
      (BracketSide::Short, _) => BRACKET_SHORT_ENTRY_COLOR,
  }
  ```
- **TakeProfit**: Unchanged (`BRACKET_TP_COLOR`).
- **Zone fills**: `BRACKET_SL_ZONE` changes to orange at 6% alpha `[1.0, 0.60, 0.0, 0.06]`. This automatically flows through `bracket_zone_rects()` (line 258 of `mod.rs`) which uses this constant. SL zone fills will render as orange instead of red for Active/PartialFill brackets.

**Testing**:
- Update `test_leg_style_cancelled_sl` (line 190) → assert `LineStyle::Dotted` instead of `LineStyle::Solid`, and assert orange RGB `[1.0, 0.60, 0.0, ...]`
- Verify `leg_style_entry_green_for_long` (line 301) still passes (Market entry, green unchanged)
- Verify `leg_style_entry_red_for_short` (line 317) still passes (Market entry, red unchanged)
- Add: `leg_style_entry_green_for_long_stop` — `(Long, Stop)` → green `[0.20, 0.78, 0.35, ...]`
- Add: `leg_style_entry_lime_for_long_stop_limit` — `(Long, StopLimit)` → lime `[0.50, 0.90, 0.20, ...]`
- Add: `leg_style_entry_red_for_short_stop` — `(Short, Stop)` → red `[0.90, 0.25, 0.25, ...]`
- Add: `leg_style_entry_pink_for_short_stop_limit` — `(Short, StopLimit)` → pink `[0.90, 0.30, 0.50, ...]`
- Add: `leg_style_sl_always_dotted_active` — verify SL is Dotted even when status = Active
- Add: `leg_style_sl_always_dotted_draft` — verify SL is Dotted for Draft (was previously Dashed)
- Add: `bracket_zone_rects_sl_orange` — verify SL zone fill is orange `[1.0, 0.60, 0.0, 0.06]`

**Done when**: `cargo test -p midas-chart` passes. SL lines are orange dotted regardless of status. SL zone fills are orange. Entry lines colored correctly by side + entry type.

---

### Slice 2: Cursor Change on Bracket Line Hover

**Goal**: OS cursor changes to vertical-resize when hovering a movable (Draft) bracket TP or SL line. Currently `mouse_interaction()` ignores bracket hit zones entirely.

**Depends on**: None (independent of Slice 1)

**Files to create or modify**:
- `desktop/win/crates/midas-app/src/chart_widget.rs` — add bracket leg proximity check in `mouse_interaction()`

**Key implementation details**:

**Step 1 — Extract a shared hit-test helper** as a private function in `chart_widget.rs`:
```rust
/// Find the bracket leg (if any) at the given cursor Y position.
/// Returns the annotation ID and hit zone kind for the nearest Draft bracket
/// TP or SL line within 6px tolerance. Returns `None` if no leg is near.
fn bracket_leg_at_cursor(
    annotations: &[Annotation],
    camera: &Camera2D,
    cursor_y: f32,
) -> Option<(AnnotationId, HitZoneKind)> {
    for ann in annotations {
        if let AnnotationKind::OrderBracket(ref bracket) = ann.kind {
            if bracket.status != BracketStatus::Draft {
                continue;
            }
            if let Some(ref tp) = bracket.take_profit {
                let tp_y = camera.price_to_y(tp.price);
                if (cursor_y - tp_y).abs() <= 6.0 {
                    return Some((ann.id, HitZoneKind::BracketTP));
                }
            }
            if let Some(ref sl) = bracket.stop_loss {
                let sl_y = camera.price_to_y(sl.price);
                if (cursor_y - sl_y).abs() <= 6.0 {
                    return Some((ann.id, HitZoneKind::BracketSL));
                }
            }
        }
    }
    None
}
```

This helper is reused in both `mouse_interaction()` (this slice) and `update()` (Slice 3), avoiding logic duplication. It mirrors the existing level-hover pattern (lines 565-574). Only checks TP and SL — entry lines are not draggable.

**Step 2 — Call the helper in `mouse_interaction()`**, after the level-hover check (line 574) and before the crosshair-active check (line 577):
```rust
// Check bracket TP/SL lines — show resize cursor on Draft brackets.
if bracket_leg_at_cursor(
    &self.snapshot.bracket_annotations,
    &cs.camera,
    pos.y,
).is_some() {
    return mouse::Interaction::ResizingVertically;
}
```

**Testing**:
- Unit test for `bracket_leg_at_cursor()`: given mock annotations with known prices and a camera, verify correct `(AnnotationId, HitZoneKind)` returned when cursor is within 6px of TP/SL, and `None` when outside or on non-Draft brackets.
- Manual: hover over Draft bracket TP/SL lines, verify cursor changes.

**Done when**: Cursor changes to vertical-resize when hovering any Draft bracket TP/SL line. Returns to default when moving away. `bracket_leg_at_cursor()` passes unit tests.

---

### Slice 3: Hover Highlight (Bold + Foreground)

**Goal**: Hovered movable bracket lines render bolder (wider) and on top of non-hovered lines.

**Depends on**: Slice 2 (reuses hover detection pattern)

**Files to create or modify**:
- `desktop/win/crates/midas-chart/src/state/mod.rs` — add `hovered_bracket_leg` field to `ChartState`
- `desktop/win/crates/midas-chart/src/input.rs` — add `hovered_bracket_leg` field to `ChartInput`
- `desktop/win/crates/midas-chart/src/widget/compute.rs` — add `hovered_bracket_leg` field to `ComputeContext`
- `desktop/win/crates/midas-chart/src/compute/mod.rs` — pass hover state through `compute_widget_annotations()`, implement two-pass z-ordering
- `desktop/win/crates/midas-chart/src/widget/order_bracket/mod.rs` — read hover state in `compute_bracket()`, boost width + alpha
- `desktop/win/crates/midas-app/src/chart_widget.rs` — set hover state in `update()` on mouse-move, pass through in `draw()` via `ChartInput`
- `desktop/win/crates/midas-chart/src/widget/order_bracket/tests.rs` — tests for hover highlight
- `desktop/win/crates/midas-chart/src/compute/tests.rs` — add `hovered_bracket_leg: None` to both `ChartInput` construction sites (lines 152 and 186)

**Key implementation details**:

**Step 1 — Add hover field to `ChartState`** (`state/mod.rs`):
```rust
use crate::widget::hit_test::HitZoneKind;
// ...
pub struct ChartState {
    // ... existing fields ...
    /// Currently hovered bracket leg for visual highlight.
    /// Set by the app layer on mouse-move; read by compute_bracket().
    pub hovered_bracket_leg: Option<(AnnotationId, HitZoneKind)>,
}
```
Initialize to `None` in `ChartState::new()`.

**Step 2 — Add hover field to `ChartInput`** (`input.rs`):
```rust
use crate::widget::AnnotationId;
use crate::widget::hit_test::HitZoneKind;
// ...
pub struct ChartInput<'a> {
    // ... existing fields ...
    /// Currently hovered bracket leg (for hover highlight in compute pass).
    pub hovered_bracket_leg: Option<(AnnotationId, HitZoneKind)>,
}
```

**Step 3 — Add hover field to `ComputeContext`** (`widget/compute.rs`):
```rust
use super::AnnotationId;
use super::hit_test::HitZoneKind;
// ...
pub struct ComputeContext<'a> {
    // ... existing fields ...
    /// Hovered bracket leg for highlight styling.
    pub hovered_bracket_leg: Option<(AnnotationId, HitZoneKind)>,
}
```

**Step 4 — Pass through in `compute_widget_annotations()`** (`compute/mod.rs:1120`):
```rust
let ctx = ComputeContext {
    // ... existing fields ...
    hovered_bracket_leg: input.hovered_bracket_leg,
};
```

**Step 5 — Two-pass z-ordering** in `compute_widget_annotations()`:
Replace the single loop (lines 1137-1147) with a two-pass approach:
```rust
let mut merged = WidgetOutput::default();
let hovered_aid = input.hovered_bracket_leg.map(|(aid, _)| aid);

// Pass 1: non-hovered brackets (render underneath).
for ann in annotations {
    if !ann.presence.is_visible() { continue; }
    if let AnnotationKind::OrderBracket(bracket) = &ann.kind {
        if Some(ann.id) == hovered_aid { continue; }
        let out = compute_bracket(bracket, ann.id, &ctx, ann.presence.alpha());
        merged.merge(out);
    }
}
// Pass 2: hovered bracket (render on top).
for ann in annotations {
    if !ann.presence.is_visible() { continue; }
    if let AnnotationKind::OrderBracket(bracket) = &ann.kind {
        if Some(ann.id) != hovered_aid { continue; }
        let out = compute_bracket(bracket, ann.id, &ctx, ann.presence.alpha());
        merged.merge(out);
    }
}
```

**Step 6 — Hover highlight in `compute_bracket()`** (`order_bracket/mod.rs`):
After each `leg_style()` call, check hover state and boost:
```rust
// TP line example:
let (tp_style, mut tp_width, tp_color) = bracket.leg_style(LegRole::TakeProfit);
let tp_hovered = ctx.hovered_bracket_leg
    .map(|(aid, kind)| aid == annotation_id && kind == HitZoneKind::BracketTP)
    .unwrap_or(false);
if tp_hovered {
    tp_width += 1.0;  // bold
}
// Same pattern for SL with HitZoneKind::BracketSL
```

**Step 7 — Set hover state in `update()`** (`chart_widget.rs`):

**Critical placement**: The hover detection must run **before** the `chart_events.is_empty()` early return at line 257. That early return fires for many non-mouse events (window focus, timer ticks), which would prevent hover state from being cleared when the cursor leaves. Place this block after modifier tracking (line 252) and before `translate_event()` (line 256):

```rust
// Update bracket hover state on every update() call, not just mouse events.
// Must be before the chart_events.is_empty() early return so hover clears
// even when non-mouse events fire.
{
    let found = if let Some(pos) = cursor.position_in(bounds) {
        bracket_leg_at_cursor(
            &self.snapshot.bracket_annotations,
            &chart_state.camera,
            pos.y,
        )
    } else {
        None // cursor left bounds — clear hover
    };
    chart_state.hovered_bracket_leg = found;
}
```

This reuses the `bracket_leg_at_cursor()` helper from Slice 2. Hover is cleared automatically when the cursor is outside bounds (returns `None`).

**Step 8 — Pass hover through `draw()`** (`chart_widget.rs`):
When constructing `ChartInput`:
```rust
let input = ChartInput {
    // ... existing fields ...
    hovered_bracket_leg: state
        .chart_state
        .as_ref()
        .and_then(|cs| cs.hovered_bracket_leg),
};
```
This follows the same pattern used for `crosshair`, `level_tool`, and `volume_scale`.

**Testing**:
- `compute_bracket_tp_hovered_wider`: verify TP line width is +1.0 when hovered
- `compute_bracket_sl_hovered_wider`: verify SL line width is +1.0 when hovered
- `compute_bracket_no_hover_normal_width`: verify normal width when not hovered
- `compute_bracket_hovered_renders_after_non_hovered`: verify two-pass ordering

**Done when**: Hovering a Draft bracket TP/SL line makes it visually bolder. Moving away reverts it. Hovered bracket lines render on top of non-hovered.

---

### Dependency Summary

```
Slice 1 (colors)     ─── independent ───┐
                                         ├─→ Can merge independently
Slice 2 (cursor)     ─── independent ───┤
                                         │
Slice 3 (highlight)  ─── depends on 2 ──┘
```

Slices 1 and 2 can be implemented in parallel. Slice 3 depends on Slice 2's `bracket_leg_at_cursor()` helper function.

## Risks & Unknowns

1. **Color tuning in linear space**: RGBA constants are linear, not sRGB. Orange/lime/pink shades may need visual iteration. Start with the proposed values, adjust by eye.

2. **SL always-dotted breaks status symmetry**: SL uses `Dotted` even for Draft (which was `Dashed`). Existing tests that assert Draft SL = Dashed will fail — these must be updated in Slice 1.

3. **One-frame hover lag**: `update()` sets hover state, `draw()` reads it same frame via local `chart_state`. This is the same pattern as crosshair and should have zero lag within a frame.

4. **Entry line not draggable**: Entry lines don't have hit zones or drag support. The cursor won't change on entry hover. Adding entry dragging would be a separate feature.

## Testing Strategy

- **Unit tests**: `leg_style()` color/style assertions for all `(side, entry_type, role)` combinations. `compute_bracket()` hover highlight width boost. Two-pass z-ordering verification.
- **Integration**: Manual cursor change verification (iced widget behavior).
- **Regression**: `cargo test --workspace` in `desktop/win/`.

## Non-Goals / Out of Scope

- **Submitted order line styling**: User noted submitted orders will use different annotations later.
- **Entry line dragging/hover**: Entry has no hit zone or drag support. Not adding it here.
- **Button hover cursor**: Bracket buttons (Submit, Save, etc.) already have `CursorIcon::Pointer` in hit zones but `mouse_interaction()` doesn't check them. Fixing button cursors is a separate task.
- **TP line color changes**: TP stays green.
- **Market/Limit entry colors**: Stay as-is (green Long, red Short). Only Stop and StopLimit get new colors.

## Review Notes

**ChartInput field additions**: Slice 3 adds `hovered_bracket_leg` to `ChartInput`. Call sites that construct `ChartInput` and must be updated:
- `chart_widget.rs:461` — the `draw()` method (primary)
- Any test helpers that construct `ChartInput` in `midas-chart` tests — search for `ChartInput {` across the test codebase

**Existing test breakage**: Slice 1 breaks 1 existing test directly: `test_leg_style_cancelled_sl` (line 190) which asserts `LineStyle::Solid` for cancelled SL — must change to `Dotted`. Entry color tests (`leg_style_entry_green_for_long`, `leg_style_entry_red_for_short`) still pass because they test Market entry type which keeps existing colors.

**Shared hit-test helper**: `bracket_leg_at_cursor()` is extracted in Slice 2 and reused by Slice 3's `update()` hover logic. This avoids duplicating the 15-line hit-test loop and makes the logic independently unit-testable.

**Alternative considered for hover state**: Caching `WidgetOutput::hit_zones` in widget state was considered but rejected — `draw()` takes `&State` (immutable), and the lightweight hit-test approach is simpler and follows the existing level-hover pattern.
