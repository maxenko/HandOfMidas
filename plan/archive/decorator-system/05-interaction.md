# Interaction & Hover Model

This file covers how decorator items respond to the cursor: the two-pass compute for hover-revealed items, the frame-to-frame persistence rule for `OnGroupHover`, and how clicks route through `ChartAction` to the app layer. Types are in [03-data-model.md](03-data-model.md); the implementation slice is Slice 5 + Slice 6 in [06-implementation.md](06-implementation.md).

Rendering of the shapes themselves — rects, labels, outlines — is in [04-rendering.md](04-rendering.md). This file treats the shapes as abstract: the interaction model is about *which* items get emitted in a given frame and *where* their clicks go, not what they look like.

---

## Visibility rules

A `DecoratorItem` carries a `Visibility` enum (defined in [03-data-model.md](03-data-model.md)). Three variants, with strict semantics:

- **`Visibility::Always`** — emitted every frame, regardless of cursor state. Price badges, label badges, and persistent markers use this.
- **`Visibility::OnLineHover`** — visible only while the parent `PriceLine` is hovered. "Hovered" means the pointer is over the line's own hit zone (the hover-padded rect the line emits as its `HitZoneKind::LevelLine` or `HitZoneKind::BracketLine`).
- **`Visibility::OnGroupHover`** — visible while the parent line is hovered **OR** while the pointer is over any currently-visible item in the same `DecoratorGroup`.

### Why `OnGroupHover` exists as a separate variant

Consider a bracket with a close button `[X]` that should appear only on hover. If the button were tagged `Visibility::OnLineHover`, this sequence would fail:

1. User hovers the line → button appears.
2. User moves the cursor from the line toward the button, leaving the line's hit zone.
3. `hovered_annotation` clears → button disappears → click misses.

`OnGroupHover` widens the "stay expanded" region to include the items themselves. As long as the cursor is over *either* the line *or* any already-visible decorator in the group, the group stays expanded and the button stays clickable. This is a deliberate two-region hit zone and it is the single trick that makes hover-reveal UI usable without a hover-delay timer.

`OnLineHover` still exists for items that should strictly follow the line — typically informational items (hover tooltips, read-only badges) that should *not* persist past the cursor moving onto them, because persisting them would visually block the region the user is moving into.

### Concrete mapping to bracket decorators

To make the distinction concrete, here is how the bracket decorator set (defined by the constructor functions in Slice 8a-ii of [06-implementation.md](06-implementation.md)) assigns visibility:

| Decorator | Visibility | Rationale |
|---|---|---|
| Price badge (right edge) | `Always` | Always needs to be readable; never a click target. |
| Label badge (left edge) | `Always` | Identifies the bracket at a glance. |
| Quantity pill | `Always` | Critical info; click target for editing, so needs persistence. |
| Close button `[X]` | `OnGroupHover` | Hidden by default, revealed on hover, must survive cursor moving onto it. |
| Create-TP button `▲` | `OnGroupHover` | Same as close. |
| Create-SL button `▼` | `OnGroupHover` | Same as close. |
| Drag affordance | `OnLineHover` | Cosmetic — disappears cleanly when cursor leaves the line. |

Every hover-reveal click target uses `OnGroupHover`. The only `OnLineHover` case in the initial decorator set is a non-interactive drag-affordance hint.

---

## Two-pass compute inside `compute_decorator_group()`

Each call to `compute_decorator_group()` (Slice 3 in [06-implementation.md](06-implementation.md)) resolves `Visibility` against the hover state carried on `ComputeContext`. The logic is a single linear pass gated by two booleans:

```rust
let line_hovered = ctx.hovered_annotation
    .map(|(aid, _)| aid == annotation_id)
    .unwrap_or(false);
let group_expanded = ctx.hovered_decorator_groups
    .iter()
    .any(|&(aid, gid)| aid == annotation_id && gid == group.group_id);

for (idx, item) in group.items.iter().enumerate() {
    let should_emit = match item.visibility {
        Visibility::Always => true,
        Visibility::OnLineHover => line_hovered,
        Visibility::OnGroupHover => line_hovered || group_expanded,
    };
    if !should_emit { continue; }
    // emit rects, labels, hit zones (see 04-rendering.md)
}
```

The "two-pass" framing is nominal — it's really one pass with a predicate. The name comes from the fact that rendering conceptually happens in two waves per frame: first `Always` items establish the permanent shape of the group, then `OnLineHover` / `OnGroupHover` items layer on top when conditions are met. A single linear iteration is enough because items in a group are ordered left-to-right (or top-to-bottom) and the visibility predicate is per-item, not cross-item.

Skipped items are **not** emitted as rects, **not** emitted as hit zones, and **not** counted toward the group's width for anchoring purposes — when hover-reveal items appear, the group grows; when they disappear, it shrinks. This is intentional: the group's anchor point (see `DecoratorAnchor` in [03-data-model.md](03-data-model.md)) stays fixed, and the layout expands/contracts away from it.

### What "don't emit" means in practice

A skipped item contributes zero footprint to the group. If the group is right-anchored, the visible items pack toward the anchor and the group's left edge moves rightward when hover-revealed items disappear. This is what produces the "drawer slides open" feel on hover. The alternative — reserving space for hidden items — was rejected because it creates dead zones the user cannot interact with and wastes horizontal real estate in the common case where the group is collapsed.

The consequence for hit-testing: a skipped item has **no hit zone** in the frame it was skipped. The cursor cannot land on an invisible decorator and "keep it alive" via step 2 of the recompute loop (below). Only items actually emitted in the previous frame's compute can preserve group expansion. This is correct: if an item isn't drawn, there is nothing for the cursor to be "over".

---

## Hover state ownership

The chart needs a new piece of per-widget state to track which groups are currently expanded:

- **New field** on `ChartState` at `midas-chart/src/state/mod.rs:126`:
  ```rust
  pub hovered_decorator_groups: SmallVec<[(AnnotationId, u16); 2]>,
  ```
- Initialized empty in `ChartState::new()`.
- Threaded through `ChartInput` at `input.rs:18` and `ComputeContext` at `widget/compute.rs:20` by adding:
  ```rust
  hovered_decorator_groups: &'a [(AnnotationId, u16)],
  ```
- Read by `compute_decorator_group()` via the `group_expanded` check shown above.
- Written by `chart_widget.rs::update()` on every mouse-move event (see the update loop below).

`SmallVec<[_; 2]>` is sized for the realistic case: most of the time zero or one groups are expanded at once. The 2-slot inline buffer covers the edge case of two adjacent lines where a hover motion briefly overlaps both. A heap spill is possible in theory (user hovers over a dense cluster of 5+ stacked bracket lines), but it's a non-issue: the allocation is one `Vec<(u64, u16)>`, freed when hover clears, and happens at most once per user interaction burst.

The tuple is `(AnnotationId, u16)` rather than a richer struct because those two fields are the complete composite key — the `AnnotationId` identifies the parent annotation, and `group_id` disambiguates among the (typically 1-3) groups attached to that annotation. No item index, no path — group-level expansion, not item-level.

### Correction note on state placement

The existing `hovered_annotation` field lives on `ChartState`; `selected_annotation` and `drag_ghost` do **NOT** — they're sourced from app-side state owned outside `ChartState` and flow in only via `ChartInput` / `ComputeContext`. `hovered_decorator_groups` follows the `ChartState` pattern (not the `selected_annotation` / `drag_ghost` pattern) because it's owned by the chart widget's local state, not the app store.

The reason for the split: `hovered_annotation` and `hovered_decorator_groups` are both pure functions of the cursor and the current frame's hit zones — nothing outside the chart widget needs to write to them, nothing outside needs to observe them directly. `selected_annotation` and `drag_ghost`, in contrast, are cross-cutting: other parts of the app (command palette, keyboard shortcuts, the annotation store) need to read and mutate them. Those live app-side; hover state stays widget-local.

---

## Update loop — recompute default

**This is the most important section.** The default approach is to **recompute decorator hit zones in `update()` on every mouse-move event**, not to cache them from the previous frame's draw.

```rust
// In chart_widget.rs::update(), on mouse move:
let mut new_groups: SmallVec<[(AnnotationId, u16); 2]> = smallvec![];
if let Some(pos) = cursor.position_in(bounds) {
    // 1. If a line is hovered, every OnLineHover / OnGroupHover group on that
    //    annotation is expanded.
    if let Some((hovered_aid, _)) = chart_state.hovered_annotation {
        for group in decorator_groups_for_annotation(hovered_aid, &self.snapshot) {
            new_groups.push((hovered_aid, group.group_id));
        }
    }
    // 2. Recompute hit zones for every currently-expanded group (including any
    //    newly expanded in step 1) and check each against the cursor. Any group
    //    with an item under the cursor stays expanded.
    for (aid, gid) in &chart_state.hovered_decorator_groups {
        let zones = recompute_decorator_hit_zones(*aid, *gid, &ctx);
        if zones.iter().any(|hz| rect_contains(hz.rect, pos)) {
            if !new_groups.contains(&(*aid, *gid)) {
                new_groups.push((*aid, *gid));
            }
        }
    }
}
chart_state.hovered_decorator_groups = new_groups;
```

### Why recompute instead of cache

`compute_decorator_group()` is cheap. A group has at most ~6 items, intrinsic sizes are known up-front (text metrics come from an already-loaded font atlas), and the layout is a single linear pass — there is no constraint solving, no reflow cascade. Running it once per mouse-move is negligible: at 120 Hz mouse polling with ~4 visible lines that each have ~2 groups, we're talking about running a dozen trivial computations on each mouse-move event. Well under a microsecond on modern hardware.

The **frame-ordering invariant** is trivial with this approach: the cursor check in step 2 uses the current frame's cursor coordinates against hit zones that were just computed from the current frame's context. There is no cross-frame dependency, no "is this hit zone stale" question, no one-frame lag.

Contrast with the cache-based approach, where `update()` at frame N reads hit zones written by `draw()` at frame N-1. That introduces three fragile assumptions: (1) `draw()` always ran at frame N-1 (false — see the iced draw-lifecycle note below), (2) nothing that would invalidate coordinates happened between N-1's draw and N's update (false — any zoom or pan event between the two runs `update()` without a preceding mouse-move `draw()`), and (3) the cache invalidation logic catches every case (unfalsifiable in the absence of extensive tests). Recomputing in `update()` sidesteps all three.

### Fallback: `canvas::Cache` (preferred) or `RefCell` cache (last resort)

A cache on the widget wrapper can hold last-frame decorator hit zones **if** a measurement shows the recompute is expensive. This is **NOT the default** — it is an escape hatch reserved for the case where profiling shows the per-mouse-move recompute in a hot path. The realistic expectation is that we never need it (see the Slice 2.5 standalone benchmark in [06-implementation.md](06-implementation.md), which retires this assumption before any interaction code commits).

If the fallback is ever needed, **prefer iced's own `canvas::Cache`** (`iced_widget::canvas::Cache`) over a hand-rolled `RefCell<Vec<HitZone>>`. The framework already models the invalidation points: you call `cache.clear()` explicitly at the points where state actually changes (zoom, pan, data update, resize), and the cache handles the draw-side bookkeeping. This is a lower-risk shape than a hand-rolled `RefCell` because the framework's lifecycle invariants are the ones we're trying to respect.

A hand-rolled `RefCell<Vec<HitZone>>` remains a legal last-resort option, but it inherits a fragile invariant: iced 0.14's `Program::draw()` is called for reasons other than mouse events — theme changes, viewport resizes, animation frames scheduled by other widgets, window focus changes. The cache's transition invariants across those extraneous draws are hard to enforce: you have to be sure that every code path that invalidates hit zone positions clears the cache, and you have to be sure that `update()` reading stale zones on the frame after an invalidation doesn't produce a visual glitch. The recompute-in-`update()` default avoids this entire category of bug.

---

## First-frame hover edge case (walkthrough)

Walk through this specific scenario, because it's the one that breaks naive hover schemes:

> **Frame N-1**: the user hovers a `PriceLine`. `compute_decorator_group()` emits the `OnGroupHover` close button with a hit zone at rect R. `hovered_annotation` is set to `(aid, HitZoneKind::LevelLine)`. `hovered_decorator_groups` contains `(aid, group_id)`.
>
> **Frame N**: the user moves the cursor by 10 pixels, and the new position is inside R (the close button). What happens?
>
> 1. `update()` runs first for the mouse-move event. It reads `chart_state.hovered_annotation` — still set from last frame's draw. Step 1 of the recompute loop finds decorator groups for the hovered annotation and keeps the group in `new_groups`.
> 2. Step 2 of the recompute loop re-checks the currently-expanded group by recomputing its hit zones against the fresh cursor position. The cursor is now inside R, so the group stays in `new_groups` *and* the hit-test at the per-item level marks the close button's hit zone as the active one.
> 3. `update()` also hit-tests the line itself for the `hovered_annotation` update. The cursor has moved 10px off the line, so *naively* `hovered_annotation` should be cleared — but if we clear it now, step 1 of the next frame's recompute wouldn't find the group via the hovered-line path.
> 4. **Resolution**: the hit-test for `hovered_annotation` uses the line's hit zone **OR** any expanded decorator group's hit zones belonging to the same annotation. The line is no longer hovered, but the group IS — so `hovered_annotation` stays set, pointing at the same `AnnotationId` but with the kind updated from `HitZoneKind::LevelLine` to `HitZoneKind::Decorator { group_id, item_path, .. }` for the close button.
> 5. `draw()` runs after `update()` and renders the group expanded (because `hovered_decorator_groups` still contains it) with the close button in its hover-highlighted visual state.
>
> There is no one-frame lag because the recompute happens in `update()` against fresh cursor state, and there is no gap in `hovered_annotation` because its hit-test considers the expanded group's zones as a fallback source of hover-truth for the annotation.

The key invariant this walkthrough enforces: **an annotation is "hovered" if the cursor is over its line OR over any currently-expanded decorator item belonging to it**. Encoding this OR into the `hovered_annotation` hit-test — rather than trying to coordinate two independent hover flags across frames — is what eliminates the class of one-frame-gap bugs that plague hover-reveal UIs.

---

## Action routing

A click on a decorator produces a `ChartAction::DecoratorClick` that the app layer matches on.

### New `ChartAction` variant

Added to the `ChartAction` enum in `midas-chart/src/interaction/mod.rs:61`:

```rust
DecoratorClick {
    annotation_id: AnnotationId,
    group_id: u16,
    action: DecoratorAction,
}
```

The variant carries enough context for the app layer to find the target annotation, identify which group the click belonged to (useful for disambiguation when an annotation has multiple groups with overlapping action vocabularies), and know what the user asked for via `DecoratorAction` (defined in [03-data-model.md](03-data-model.md)).

### Mouse-press handling

Extending the existing hit-test match in `interaction/mod.rs` (the one that already handles `BracketSubmit`, `BracketCancel`, and similar) to cover `HitZoneKind::Decorator`:

```rust
match hit.kind {
    HitZoneKind::Decorator { group_id, action, .. } => {
        return Some(ChartAction::DecoratorClick {
            annotation_id: hit.annotation_id,
            group_id,
            action,
        });
    }
    // ... existing arms
}
```

The `item_path` / `item_path_len` fields of `HitZoneKind::Decorator` are not consumed here — they exist for the rendering side (hit-zone-to-visual mapping for hover highlighting) and are not needed for action dispatch. See [04-rendering.md](04-rendering.md) for how `item_path` is used.

### App-layer dispatch

In `midas-app/src/chart_widget.rs`, the action handling loop gets one match arm per `DecoratorAction` variant:

```rust
ChartAction::DecoratorClick { annotation_id, action, .. } => match action {
    DecoratorAction::CloseAnnotation => self.annotation_store.remove(annotation_id),
    DecoratorAction::CreateTakeProfit => attach_default_tp(
        self.annotation_store.get_bracket_mut(annotation_id),
    ),
    DecoratorAction::CreateStopLoss => attach_default_sl(
        self.annotation_store.get_bracket_mut(annotation_id),
    ),
    DecoratorAction::CycleEntryType => { /* ... */ }
    DecoratorAction::EditQuantity => { /* open popup */ }
    DecoratorAction::EditPrice => { /* enter drag mode */ }
    DecoratorAction::Submit => { /* forward to broker bridge */ }
    DecoratorAction::Save => { /* mark as saved */ }
    DecoratorAction::ToggleLocked => { /* flip Annotation.locked */ }
    DecoratorAction::Custom(_) => { /* reserved */ }
}
```

Each arm is a small, local side effect: mutate the annotation store, open a popup, enter a drag mode, or forward a command to the broker bridge. The `group_id` is discarded in most arms (named `..`) because for the current annotation types the action alone is unambiguous; it's retained in the variant for future annotation types where the same `DecoratorAction` might mean different things in different groups.

### Why dispatch lives in the app layer

`midas-chart` is sans-IO. It doesn't own the annotation store, doesn't know about the broker bridge, and cannot open popups. Its job is to translate a click into a structured intent — `DecoratorClick { annotation_id, group_id, action }` — and hand that intent to the caller. The app layer is the only place with enough context to decide what `EditQuantity` actually means (which popup, anchored where, with what validation) or what `Submit` routes to (which broker account, which order template).

This is the same pattern already used for `BracketSubmit` / `BracketCancel` from the order-bracket refinement plan: chart emits a minimal action variant, app does the work. Adding `DecoratorClick` extends the pattern uniformly; there is no new layering or ownership concept to learn.

### Error handling

A `DecoratorClick` arrives with an `annotation_id` that was live at hit-test time. By the time the app's match arm runs, the annotation may have been removed by a concurrent event (e.g. a broker fill collapsed the bracket). Each arm handles missing-annotation gracefully — `get_bracket_mut` returns `Option`, the arm short-circuits, nothing crashes. There are no "action for nonexistent annotation" error dialogs because this is a normal race, not a bug.

---

## Hit-zone breadcrumbs

`HitZoneKind::Decorator` carries an `item_path` encoded as `[u8; 4]` plus a `u8` length byte — not a `SmallVec`. The reason is Decision 8 in [02-design-decisions.md](02-design-decisions.md): `HitZoneKind` is `#[derive(Copy)]` at `widget/hit_test.rs:46`, and a `SmallVec` field would break `Copy`. Fixed-size array + length byte preserves `Copy` while giving us enough addressing depth for every realistic decorator layout.

### Encoding

An `item_path` of `[2, 0, 0, 0]` with `item_path_len = 2` means: **group item index 2 → stack child 0**. The first byte identifies which item in the `DecoratorGroup.items` slice was hit; subsequent bytes drill into composite items (a stack of sub-segments, a split button) when the hit zone is more fine-grained than the top-level item.

The max depth of 4 is comfortable headroom. The deepest layout in the plan is `group → stack → segment`, which is 3 levels; the fourth byte exists as slack for future composites without reopening the `HitZoneKind` derive discussion.

### Consumers

- `interaction/mod.rs` mouse-press handler — reads `action` (already in the hit zone) and forwards to the app layer; does not consume `item_path`.
- Rendering hover-highlight logic in [04-rendering.md](04-rendering.md) — uses `item_path` to map "cursor is hovering this hit zone" back to "highlight this specific sub-rect of the group visual".
- Debug overlays — render `item_path` as a breadcrumb string for diagnosability when the hit zone inspector is enabled.

### Why a fixed array beats alternatives

Three alternatives were considered and rejected:

1. **`u16` encoded path** (pack 4 nibbles or 5 tritbits): compact but opaque, hostile to debugging, and the bit-packing is fragile to refactor.
2. **`SmallVec<[u8; 4]>`**: natural fit but breaks `Copy` on `HitZoneKind`, which cascades — every function that returns or destructures `HitZone` would need to clone or borrow, across a lot of code.
3. **Box / heap path**: violates the "hit zones are cheap value types" invariant the hit-test layer rests on.

The `[u8; 4] + u8` encoding is the only option that preserves `Copy`, stays readable in `{:?}` output, and has no allocation. The length byte is strictly required because a zero-padded array can't distinguish `[0]` (item 0) from `[0, 0]` (item 0's first child).

---

## Summary of invariants

1. **Two-pass emission inside `compute_decorator_group()`**: `Always` first, then `OnLineHover` / `OnGroupHover` gated by `line_hovered || group_expanded`.
2. **`hovered_decorator_groups` is owned by `ChartState`**, not by app-side state, because it's a pure function of the widget's own cursor + hit zones.
3. **`update()` recomputes decorator hit zones on every mouse-move** as the default. `RefCell` caching is a fallback, not the baseline.
4. **`hovered_annotation` hit-testing considers expanded decorator groups** as a fallback source of annotation-level hover truth — this is the single rule that closes the first-frame gap when the cursor moves from line to button.
5. **Clicks on decorators produce `ChartAction::DecoratorClick`** with `(annotation_id, group_id, action)`. The app layer dispatches one match arm per `DecoratorAction` variant.
6. **`item_path` uses `[u8; 4] + u8` length**, not `SmallVec`, to keep `HitZoneKind: Copy`.

All of these are testable in isolation. See the testing bullets in Slice 5 and Slice 6 of [06-implementation.md](06-implementation.md).

### Test hooks worth naming

Five unit tests, plus one end-to-end, cover the interaction model:

- `decorator_on_line_hover_emitted_when_parent_hovered` — construct a `ComputeContext` with `hovered_annotation = Some((aid, _))` and an empty `hovered_decorator_groups`, run `compute_decorator_group()`, assert the `OnLineHover` items were emitted.
- `decorator_on_group_hover_persists_when_group_expanded` — the inverse: `hovered_annotation = None`, `hovered_decorator_groups = [(aid, gid)]`, assert the `OnGroupHover` items are still emitted.
- `decorator_hover_set_update_keeps_groups_with_cursor_over_item` — simulate the first-frame walkthrough above, assert `hovered_decorator_groups` survives the transition.
- `decorator_hover_set_update_drops_groups_when_cursor_leaves_both_line_and_items` — cursor moves into dead space, assert `hovered_decorator_groups` is empty on the next frame.
- `mouse_press_on_decorator_emits_click_action` — inject a `HitZoneKind::Decorator` into the hit-test results, assert `ChartAction::DecoratorClick { .. }` is produced with the right `(annotation_id, group_id, action)`.
- **End-to-end**: launch the app, click the `[X]` on a test bracket, assert the bracket disappears from the store.

These are the minimum to prove the interaction model works; the slice bullets in [06-implementation.md](06-implementation.md) list a few more that cover edge cases specific to the compute pass.
