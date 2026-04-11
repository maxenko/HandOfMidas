# Implementation Plan

Slice-by-slice execution. Types are defined in [03-data-model.md](03-data-model.md); rendering detail in [04-rendering.md](04-rendering.md); interaction detail in [05-interaction.md](05-interaction.md); risks and testing in [07-risks-testing.md](07-risks-testing.md).

## Dependency graph

The old plan was a strict 1→2→3→...→9 chain. This version adds Slice 0 (SDF spike) and
Slice 2.5 (hover-recompute benchmark) as standalone gates, runs Slices 0 and 1 in
parallel (zero file overlap — one is a throwaway example, the other is in
`midas-chart`), runs Slices 3 and 4 in parallel (different crates), runs Slices 6
and 7 in parallel (levels have no action buttons so they don't block on Slice 6),
allows Slice 8a-i to be soft-ordered after Slice 6 (no hard compile-time dep — see
note below), and splits the old Slice 8 into 8a-i (data-model + shim), 8a-ii (visual
emissions), and 8b (interactive cutover + 5-variant deletion).

```
Slice 0  (SDF shader spike — standalone go/no-go gate)
   │                                               ┐
   │   (go: proceed with SDF for Slice 4;          │
   │    no-go: fall back to geometry decomposition,│  Slice 1 may run on a second
   │    budget +1-2 days of re-planning)           │  engineer in parallel with Slice 0.
   ↓                                               │
Slice 1  (LineStyle::Pattern) ─────────────────────┘
   │
   ↓
Slice 2  (PriceLine + decorator types + BadgeInstance)   ← BadgeInstance lives HERE
   │
   ↓
Slice 2.5 (hover-recompute benchmark — standalone retirement of Decision 7 cost assumption)
   │
   ├─→ Slice 3 (compute engine — emits BadgeInstance into WidgetOutput.badges) ─┐
   │                                                                              │
   └─→ Slice 4 (SDF badge GPU pipeline + both ChartScene types wired)            │
                                                                                  │
                          Slices 3 and 4 run in parallel, different crates        │
                                                                                  ↓
                                                              ┌───────────────────┤
                                                              │                   │
                                                              ↓                   ↓
                             Slice 5 (hover two-pass + update() recompute)
                                              │
                                              ↓
              ┌───────────── Slice 6 (DecoratorAction routing) ─────────────┐
              │                                                               │ soft ordering
              │   parallel with Slice 7 (hard dep only on Slice 5)           │ — 8a can run
              │                                                               │ concurrently
              └───────────── Slice 7 (HorizontalLevel migration) ────────────┘
                                              │
                                              ↓
                             Slice 8a-i  (BracketLeg data-model rewrite, rendering via shim)
                                              │
                                              ↓
                             Slice 8a-ii (Visual decorator emissions, legacy hit zones alive)
                                              │
                                              ↓
                             Slice 8b   (Bracket button migration + 5-variant deletion)
                                              │
                                              ↓
                             Slice 9    (cleanup)
```

**Why Slices 0 and 1 can run in parallel**: Slice 0 is a throwaway single-file WGSL
spike in `midas-render/examples/`; Slice 1 touches `widget/level.rs` and
`widget/order_bracket/mod.rs` in `midas-chart`. Zero file overlap, zero merge-conflict
risk. On a two-engineer team, Engineer A runs the spike while Engineer B ships Slice 1
on the same day.

**Why Slice 3 and Slice 4 can run in parallel**: Slice 3 lives in `midas-chart`, which
is sans-IO with zero GPU dependencies per `desktop/win/CLAUDE.md`. It only needs the
`BadgeInstance` *type* from Slice 2 — not a working pipeline. Slice 4 lives in
`midas-render` and builds the GPU pipeline, wiring badges into `ChartScene`. Both
slices touch different crates. **Integration ownership**: Slice 4 explicitly owns the
edit that makes `compute_decorator_group()` emit real `BadgeInstance` entries into
`WidgetOutput.badges` instead of Slice 3's bounding-rect `GridLineInstance`
placeholders — see Slice 4's Files list and the
`compute_decorator_group_emits_badge_instance_for_point_left` regression test.

**Why Slice 5 depends only on Slice 3 (not 4)**: the hover state machine operates on
`HitZone`s and `WidgetOutput`, which are sans-IO concepts. Whether the emitted shapes
render as real SDF badges (Slice 4 shipped) or bounding-rect placeholders (Slice 4 not
yet shipped) is irrelevant to the state machine. Slice 5 is still blocked on Slice 4
for *visual* verification, but the code can land on top of Slice 3.

**Why Slice 7 does not depend on Slice 6 (and how the price badge stays inert until
Slice 6 lands)**: levels have no hover-revealed action buttons. Their decorators are
all `Visibility::Always`. The right-edge price badge is emitted with **`action: None`
during Slice 7** — clicks on it fall through to the existing `HitZoneKind::LevelLine`
hit zone at `widget/level.rs:219`, which still handles drag-to-edit exactly as today.
No runtime regression, no ambiguous hit-test priority. A follow-up one-line edit in
Slice 6 (or a later slice) flips the price badge action to
`Some(DecoratorAction::EditPrice)` once `ChartAction::DecoratorClick` routing exists,
turning the click into an explicit inline-edit affordance rather than a drag-start. The
plan deliberately pays the one-line follow-up cost to preserve the 6 ‖ 7 parallel
window.

**Why Slice 8a-i is soft-ordered after Slice 6 (not a hard dep)**: Slice 8a-i's code
does not reference `ChartAction::DecoratorClick`. The `DecoratorAction` enum comes
from Slice 2; `compute_decorator_group()` comes from Slice 3; hover comes from Slice 5.
The **hard** dependency is Slice 7 (which proves the migration pattern on the simpler
level case), not Slice 6. Slice 8a-i compiles and runs whether or not Slice 6 has
landed. Treating 8a-i as soft-ordered after Slice 6 means the execution schedule can
pick whichever ordering minimises engineer idle time — they're swappable. 8a-ii and
8b still require the action-routing plumbing to have landed for the interactive
cutover, so the soft ordering only applies to 8a-i.

## Slices

---

### Slice 0: SDF shader spike (go / no-go gate)

**Goal**: Prototype the 8 badge SDF shapes in a throwaway WGSL binary to validate
triangle-tip antialiasing on `PointLeft` / `PointRight` / `DoublePoint` / `Chevron`
before committing to the full SDF pipeline.

**Depends on**: none (standalone — runs before any other slice).

**Size**: S (≈ 1 engineer-day).

**Files to create or modify**:
- `desktop/win/crates/midas-render/examples/badge_sdf_spike.rs` — **new, throwaway**. A
  standalone wgpu example that opens a window, draws a grid of all 8 shapes at
  four heights (10px, 16px, 20px, 28px), and writes a screenshot to
  `plan/decorator-system/screenshots/sdf_spike.png`.
- `desktop/win/crates/midas-render/shaders/badge_spike.wgsl` — **new, throwaway**. A
  self-contained copy of the planned `badge.wgsl` fragment shader with all 8 SDF
  functions. This file is thrown away once the real `badge.wgsl` lands in Slice 4;
  the spike exists only to de-risk the decision.
- `plan/decorator-system/screenshots/` — **new directory** for the screenshot output.

**Key implementation details**:

Render a 4-column × 8-row grid against a dark background (rows = shapes, columns =
heights). At 10px, 16px, 20px, 28px, the triangle tips of `PointLeft`, `PointRight`,
`DoublePoint`, and `Chevron` are the most likely to alias. Visual inspection is the
gate.

Use iq's 2D SDF formulas (<https://iquilezles.org/articles/distfunctions2d/>) as the
starting point. The tricky ones are the pointed shapes, which need a union of a rect
SDF with a triangle SDF via `min()`. The standard analytic triangle SDF uses barycentric
projection onto the three edges — this is where tip aliasing shows up if the `fwidth(d)`
antialiasing factor is wrong.

Shader skeleton:

```wgsl
// Inside fs_main:
let aa = fwidth(d);
let fill_alpha = 1.0 - smoothstep(-aa, aa, d);
return in.fill * fill_alpha;
```

The spike does NOT need to wire into `ChartScene`, `ChartRenderer`, or the real
pipeline structure. It's a single-file `cargo run --example badge_sdf_spike`.

**Pass criterion**: tip aliasing is acceptable at all heights ≥ 16px. Badges look
smooth and the triangle points are sharp without obvious stair-stepping.

**Fail criterion → fallback**: Decision 5 in [02-design-decisions.md](02-design-decisions.md) documents "geometry
decomposition" as the rejected alternative. If Slice 0 fails, the project falls back
to that approach — decompose `PointLeft` into one rect + one triangle via a new
`TriangleInstance` primitive drawn by a second pipeline. Slice 4 is then rewritten as:

- `BadgeInstance` stays the same for rect/rounded/pill/circle (still SDF).
- `TriangleInstance` (new, 32 bytes: `rect[4] + color[4]`) handles just the triangles.
- Pointed shapes emit TWO primitives instead of one (rect + triangle), joined at the
  seam where both are filled the same color.

**Re-planning budget on no-go**: if Slice 0 fails, budget **1–2 additional engineer-days**
for Slice 4 re-planning before Slice 4 work can begin. The re-plan touches: (1) Slice 4's
Files list gains `midas-render/src/pipelines/triangle.rs` and
`midas-render/shaders/triangle.wgsl`; (2) the `shape_id` mapping in
[03-data-model.md](03-data-model.md) is revised so pointed variants are decomposed at
`compute_decorator_group()` emission time rather than at shader time; (3) a second
`triangles: Vec<TriangleInstance>` field lands on both `ChartScene` types; (4) the
`BadgePipeline::draw()` call is followed by a `TrianglePipeline::draw()` in
`ChartRenderer::render()`. The total engineering cost of Slice 4 increases by roughly
one day due to the extra pipeline plumbing.

Document the go/no-go outcome as a note at the top of Slice 4 before Slice 4 begins.

**Testing**: none — this is a visual spike. The "test" is human eyeball plus the saved
screenshot for review.

**Done when**:
- `cargo run -p midas-render --example badge_sdf_spike` runs standalone, opens a window
  rendering the 4×8 shape grid.
- `plan/decorator-system/screenshots/sdf_spike.png` is committed to the plan directory.
- The team has explicitly ruled "go" (proceed with SDF for Slice 4) or "no-go" (switch
  to geometry decomposition and rewrite Slice 4).
- The spike code is left in place as a reference or deleted — implementer's choice, but
  Slice 9 grep-checks that no `badge_sdf_spike` references leak into production code.

---

### Slice 1: `LineStyle::Pattern` replacement

**Goal**: Replace `LineStyle::{Dashed, Dotted}` with `LineStyle::Pattern(SmallVec<[f32; 6]>)`.
Rewrite `segmented_line()` as a pattern-walking loop. Self-contained, no dependency on
any later slice.

**Depends on**: nothing (can run in parallel with Slice 0).

**Size**: S.

**Files to create or modify**:
- `desktop/win/crates/midas-chart/src/widget/level.rs` — replace `LineStyle` enum (line 42),
  rewrite `segmented_line()` (line 90), update `compute_level()` call site (line 195).
- `desktop/win/crates/midas-chart/src/widget/order_bracket/mod.rs` — update `leg_style()`
  at line 207 to return `LineStyle::Pattern` instead of `Dashed`/`Dotted`.
- `desktop/win/crates/midas-chart/src/widget/order_bracket/tests.rs` — update tests that
  assert specific `LineStyle` variants (notably `test_leg_style_cancelled_sl` and the
  SL-always-dotted tests from the bracket-line-refinement plan).
- `desktop/win/Cargo.toml` — add `smallvec = { version = "1", features = ["serde", "const_generics"] }`
  to workspace dependencies if not already present.

**Key implementation details**:

New enum:

```rust
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum LineStyle {
    #[default]
    Solid,
    /// SVG-style dash pattern: alternating on/off run lengths in logical pixels.
    /// The pattern is walked cyclically, starting with "on". An empty pattern
    /// is equivalent to `Solid`.
    Pattern(SmallVec<[f32; 6]>),
}

impl LineStyle {
    pub fn dotted() -> Self         { Self::Pattern(smallvec![1.0, 3.0]) }
    pub fn sparse_dotted() -> Self  { Self::Pattern(smallvec![1.0, 6.0]) }
    pub fn dashed() -> Self         { Self::Pattern(smallvec![6.0, 3.0]) }
    pub fn dashed_long() -> Self    { Self::Pattern(smallvec![10.0, 4.0]) }
    pub fn dash_dot() -> Self       { Self::Pattern(smallvec![6.0, 3.0, 1.0, 3.0]) }
    pub fn dash_dot_dot() -> Self   { Self::Pattern(smallvec![6.0, 3.0, 1.0, 3.0, 1.0, 3.0]) }
}
```

Rewritten `segmented_line()`:

```rust
pub fn segmented_line(
    x0: f32, x1: f32, y: f32, height: f32,
    color: [f32; 4], style: &LineStyle,
) -> Vec<GridLineInstance> {
    match style {
        LineStyle::Solid => vec![GridLineInstance {
            rect: [x0, y, x1, y + height], color,
        }],
        LineStyle::Pattern(pattern) if pattern.is_empty() => vec![GridLineInstance {
            rect: [x0, y, x1, y + height], color,
        }],
        LineStyle::Pattern(pattern) => {
            let mut segments = Vec::new();
            let mut cursor = x0;
            let mut idx = 0;
            let mut is_on = true;
            while cursor < x1 {
                let run = pattern[idx % pattern.len()];
                let end = (cursor + run).min(x1);
                if is_on && end > cursor {
                    segments.push(GridLineInstance {
                        rect: [cursor, y, end, y + height], color,
                    });
                }
                cursor = end;
                idx += 1;
                is_on = !is_on;
            }
            segments
        }
    }
}
```

`leg_style()` dot-spacing-4 becomes `LineStyle::Pattern(smallvec![1.0, 3.0])`;
`Dashed { dash_len: D, gap_len: G }` becomes `LineStyle::Pattern(smallvec![D, G])`.
All call sites migrate to presets.

**Testing**:
- `segmented_line_solid_produces_one_segment`
- `segmented_line_empty_pattern_is_solid`
- `segmented_line_dotted_produces_expected_count` — verify `[1, 3]` over 100px
  produces ~25 segments.
- `segmented_line_dash_dot_alternates_run_lengths` — verify output rects have widths
  matching the pattern rhythm.
- `segmented_line_pattern_wraps_cyclically` — verify a `[a, b, c]` pattern (3 entries,
  odd count) correctly alternates on/off phases across cycles.
- `segmented_line_zero_width_run_skipped` — defensive: a `[0, 3]` pattern must not
  emit zero-width rects.
- `leg_style_sl_uses_pattern` — updated test verifying bracket SL uses
  `Pattern(smallvec![1.0, 3.0])`.
- `leg_style_draft_tp_uses_pattern` — updated test for Draft TP dashed pattern.

**Done when**: `cargo test -p midas-chart` passes. All old `Dashed`/`Dotted` references
are gone from the workspace (grep `LineStyle::Dashed` and `LineStyle::Dotted` returns
zero hits). Visual parity confirmed by running the app and checking existing bracket
lines render identically.

---

### Slice 2: `PriceLine` primitive + decorator types + `BadgeInstance`

**Goal**: Introduce the `PriceLine` primitive and the complete decorator data model as
plain types. Also land `BadgeInstance` (the GPU struct), so both Slices 3 and 4 can
start work in parallel off the back of this slice. No renderer or compute changes yet —
the types exist but nothing consumes them. Zero behavioral change in the running app.

**Depends on**: Slice 1 (`LineStroke` owns the new `LineStyle`).

**Size**: M.

**Files to create or modify**:
- `desktop/win/crates/midas-chart/src/widget/price_line.rs` — **new file**. Defines
  `PriceLine`, `LineStroke`, `LineExtent`.
- `desktop/win/crates/midas-chart/src/widget/decorator/mod.rs` — **new module**. Public
  re-exports.
- `desktop/win/crates/midas-chart/src/widget/decorator/group.rs` — **new file**.
  `DecoratorGroup`, `DecoratorItem`, `DecoratorAnchor`, `FlexDirection`, `Visibility`,
  `ItemContent`.
- `desktop/win/crates/midas-chart/src/widget/decorator/badge.rs` — **new file**.
  `Badge`, `BadgeSegment`, `BadgeShape`, `BadgeBorder`.
- `desktop/win/crates/midas-chart/src/widget/decorator/button.rs` — **new file**.
  `Button`.
- `desktop/win/crates/midas-chart/src/widget/decorator/action.rs` — **new file**.
  `DecoratorAction` enum.
- `desktop/win/crates/midas-chart/src/widget/mod.rs` — add `pub mod price_line;` and
  `pub mod decorator;` declarations.
- `desktop/win/crates/midas-chart/src/instances.rs` — **append `BadgeInstance` struct at
  end of file** (this is the H5 move — BadgeInstance used to live in Slice 4 but
  migrated here so Slices 3 and 4 can run in parallel).

**Key implementation details**:

Full type surface — all structs are `Clone + Debug + PartialEq + Serialize + Deserialize`
unless otherwise noted. See [03-data-model.md](03-data-model.md) for the canonical
definitions; the snippets below are the landing shape for this slice.

```rust
// price_line.rs
pub struct PriceLine {
    pub price: f64,
    pub extent: LineExtent,
    pub stroke: LineStroke,
}

pub struct LineStroke {
    pub color: [f32; 4],
    pub width: f32,
    pub style: LineStyle,
}

pub enum LineExtent {
    #[default] FullWidth,
    RightFrom { timestamp: i64 },
    Between { start: i64, end: i64 },
}

// decorator/group.rs
pub struct DecoratorGroup {
    pub group_id: u16,
    pub anchor: DecoratorAnchor,
    pub direction: FlexDirection,
    pub gap: f32,
    pub items: SmallVec<[DecoratorItem; 4]>,
}

pub enum DecoratorAnchor {
    LeftEdge,
    RightEdge,
    AtTimestamp(i64),
    AtScreenX(f32),
}

pub enum FlexDirection { Row, Column }

pub struct DecoratorItem {
    pub visibility: Visibility,
    pub action: Option<DecoratorAction>,
    pub content: ItemContent,
}

pub enum Visibility {
    #[default] Always,
    OnLineHover,
    OnGroupHover,
}

pub enum ItemContent {
    Badge(Badge),
    Button(Button),
    Stack(Box<DecoratorGroup>),
    Spacer(f32),
}
```

Badge, Button, and action types are listed in [03-data-model.md](03-data-model.md).

**`BadgeInstance`** — the GPU struct. Append to `instances.rs`:

```rust
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct BadgeInstance {
    pub rect: [f32; 4],            // bounding box in screen space
    pub fill: [f32; 4],            // linear RGBA
    pub border: [f32; 4],          // linear RGBA (alpha=0 → no border)
    pub shape_id: u32,             // see BadgeShape discriminant mapping below
    pub shape_param: f32,          // radius / point_width / unused
    pub border_thickness: f32,     // logical pixels
    pub _pad: f32,
}
```

`shape_id` mapping (stable, referenced from WGSL in Slice 4):

```
0 = Rect        4 = PointRight
1 = Rounded     5 = DoublePoint
2 = Pill        6 = Chevron
3 = PointLeft   7 = Circle
```

Add a compile-time assertion that the `BadgeShape` enum discriminant order matches the
`shape_id` mapping, so rearranging the enum can't silently corrupt the shader switch:

```rust
const _: () = {
    assert!(BadgeShape::Rect as u32 == 0);
    assert!(BadgeShape::Rounded { radius: 0.0 } as u32 == 1);
    // ... etc
};
```

(If the const-assert pattern doesn't work for non-unit variants, fall back to a
`#[test]` that round-trips each variant through `shape_id`.)

**Testing**:
- Serde round-trip for each top-level type: `PriceLine`, `DecoratorGroup`, `Badge`,
  `Button`, `DecoratorAction`.
- `decorator_group_nested_stack_serialises` — verify the boxed `Stack` variant round-
  trips.
- `line_style_pattern_serialises` — verify `SmallVec` serde works with the workspace's
  smallvec features.
- `badge_instance_pod_roundtrip` — bytemuck cast.
- `badge_instance_shape_id_matches_enum` — const or runtime assertion that the
  `BadgeShape` discriminant order matches the shader mapping.

**Done when**: `cargo test --workspace` passes. `cargo clippy --workspace -- -D warnings`
clean. All new types are public, documented, and serialize/deserialize cleanly.
`BadgeInstance` is visible from both `midas-chart` and `midas-render` via the
`midas-chart` dependency. Zero behavioral change in the running app.

---

### Slice 2.5: Hover-recompute benchmark (retires Decision 7 cost assumption)

**Goal**: Run a standalone synthetic benchmark that measures
`compute_decorator_group()`'s flex-layout + hit-zone emission cost at a worst-case
decorator density, before any interaction code (Slice 5 onward) commits to the
recompute-in-`update()` default from Decision 7 in [02-design-decisions.md](02-design-decisions.md).

**Depends on**: Slice 2 (types exist). This slice can run in parallel with Slices 3 and
4; it does not block them and is not blocked by them. The goal is to retire a confidence
assumption three slices earlier than it would otherwise land.

**Size**: S (≈ half an engineer-day).

**Files to create or modify**:
- `desktop/win/crates/midas-chart/benches/decorator_layout.rs` — **new, criterion
  benchmark**. Synthesizes a worst-case scene (see scenario below) and measures
  `compute_decorator_group()` per call.
- `desktop/win/crates/midas-chart/Cargo.toml` — add `criterion = "0.5"` as a dev-
  dependency and a `[[bench]]` entry if not already present.

**Key implementation details**:

Worst-case scenario: **20 visible charts × 10 annotations per chart × 2 groups per
annotation × 6 items per group ≈ 2400 decorator items**. For each call, construct a
stub `ComputeContext` with a synthetic `Camera2D`, iterate the 400 groups, and call
`compute_decorator_group()` in sequence. Measure total time for the full pass.

The benchmark assertion is **< 50 µs per mouse-move event** (the mouse-move recompute
budget — see the performance target in [07-risks-testing.md](07-risks-testing.md)).
On a modern desktop CPU this should be sub-microsecond per group and the full pass
should sit comfortably inside the budget.

This is an intentionally pessimistic worst case: real sessions show 10–30 visible
decorators, not 2400. If the worst-case pass fits in budget, the realistic path fits
by a wide margin.

**Pass criterion**: the benchmark's reported median time for the 400-group pass is
under 50 µs. If it exceeds this budget, Decision 7's "recompute-in-`update()` as the
default" is reconsidered — either the flex-layout engine gets a fast path, or the
fallback `canvas::Cache` approach from Decision 7 is promoted to the default. Document
the outcome as a note at the top of Slice 5 before Slice 5 begins.

**Testing**:
- The benchmark itself is the test. Criterion's statistical machinery establishes the
  median and the confidence interval; we assert on the median.
- One sanity unit test: `decorator_layout_bench_harness_builds_2400_items` — verify the
  scenario-construction code produces the expected item count and is not accidentally
  measuring a much smaller set.

**Done when**:
- `cargo bench -p midas-chart --bench decorator_layout` runs and reports a median in
  the sub-50-µs range at the 400-group / 2400-item scale.
- The decision outcome (default preserved vs. fallback promoted) is recorded as a note
  at the top of Slice 5 in this file.
- The benchmark lives in the repo and is re-run whenever `compute_decorator_group()` or
  the layout engine is touched in later slices, catching regressions.

---

### Slice 3: Decorator compute + flex layout engine (axis-aligned placeholders)

**Goal**: Implement `compute_decorator_group()` — the function that takes a `PriceLine`
plus a `DecoratorGroup` and a `&ComputeContext` and produces a `WidgetOutput`. Layout
is flex along the group's direction; shapes emit `GridLineInstance` fills for the
bounding box (all non-axis-aligned shapes render as bounding-box `Rect` placeholders
in this slice). Text emits `WidgetLabel`s. Each visible item with `action: Some(_)`
emits a `HitZone`.

**Depends on**: Slice 2 (types exist, including `BadgeInstance`).

**Size**: L.

**Runs in parallel with**: Slice 4. Slice 3 lives entirely in `midas-chart` and never
touches GPU code; Slice 4 lives in `midas-render` and never calls
`compute_decorator_group()`. Merge-conflict surface is minimal.

**Files to create or modify**:
- `desktop/win/crates/midas-chart/src/widget/decorator/compute.rs` — **new file**. The
  layout + emission engine.
- `desktop/win/crates/midas-chart/src/widget/decorator/layout.rs` — **new file**.
  Measure + position helpers.
- `desktop/win/crates/midas-chart/src/widget/decorator/mod.rs` — export
  `compute_decorator_group()`.
- `desktop/win/crates/midas-chart/src/widget/hit_test.rs` — add the
  `HitZoneKind::Decorator` variant at line 46.

**Key implementation details**:

Entry function:

```rust
/// Compute render primitives for a decorator group anchored to a PriceLine.
/// Returns a WidgetOutput containing fills/labels/hit_zones. In Slice 3, badges
/// with non-axis-aligned shapes are emitted as bounding-box rects; Slice 4
/// replaces those with BadgeInstance entries (the Slice 4 code path lives in
/// the same emission function, gated behind a shape check).
pub fn compute_decorator_group(
    group: &DecoratorGroup,
    line: &PriceLine,
    annotation_id: AnnotationId,
    ctx: &ComputeContext<'_>,
    alpha: f32,
) -> WidgetOutput
```

**Anchor resolution**: `DecoratorAnchor::LeftEdge` → `x = 0`; `RightEdge` →
`x = viewport.width`; `AtTimestamp(t)` → `camera.time_to_x(t)`; `AtScreenX(x)` → `x`.
Y comes from `camera.price_to_y(line.price)`.

**Measurement pass**: walk items, compute each item's `(width, height)` without
positioning. Text width is measured via a stub `measure_text(text, font_size) -> f32`
helper (Slice 3 uses a heuristic `font_size * 0.6 * char_count`; iced's real
measurement hooks in during migration in Slice 7).

**Layout pass**: starting at the anchor, place items along `direction` with `gap`
between siblings. For `Row + RightEdge` (the screenshot case), items lay out right-to-
left — the rightmost item is placed at `anchor_x - item.width`, the next at
`anchor_x - item.width - gap - prev.width`, etc.

**Emission pass**: each positioned item emits primitives:

- **`Badge`** → one or more `GridLineInstance` fills for the bounding rect (Slice 3
  placeholder — Slice 4 replaces with `BadgeInstance`), plus one `WidgetLabel` per
  segment, plus one divider `GridLineInstance` between adjacent segments if
  `divider_color` is set.
- **`Button`** → one bounding rect fill + one `WidgetLabel` for the glyph.
- **`Stack`** → recurse into `compute_decorator_group()` with `direction: Column` and
  anchor derived from the stack's position in the parent.
- **`Spacer`** → no primitive, just consumes `width` in the layout pass.

**`HitZoneKind::Decorator` variant**:

C1 fix — `HitZoneKind` at `hit_test.rs:46` currently derives `Copy`. That derive MUST
be preserved. The decorator variant therefore uses **fixed-size arrays, not `SmallVec`**:

```rust
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum HitZoneKind {
    // ... existing variants
    Decorator {
        group_id: u16,
        item_path: [u8; 4],      // breadcrumb path into nested Stacks
        item_path_len: u8,       // how many bytes of item_path are valid
        action: DecoratorAction, // DecoratorAction must also be Copy
    },
}
```

`DecoratorAction` in [03-data-model.md](03-data-model.md) is a plain-enum with `u32`
payload on `Custom` only; it derives `Copy` without issue.

Four bytes of `item_path` is enough: group → item → (optional) stack-child → (optional)
segment. Anything deeper than four levels is a design smell.

**Hit zones**: every visible item with `action: Some(_)` emits:

```rust
HitZone {
    annotation_id,
    rect: [x0, y0, x1, y1],
    kind: HitZoneKind::Decorator {
        group_id: group.group_id,
        item_path: [item_idx as u8, 0, 0, 0],
        item_path_len: 1,
        action,
    },
    cursor: CursorIcon::Pointer,
}
```

Nested `Stack` items set `item_path = [group_idx, stack_child_idx, 0, 0]`,
`item_path_len = 2`.

**Segment-level hit zones**: a `BadgeSegment` with its own `action` emits a hit zone
covering just the segment's sub-rect (not the whole badge). `item_path` carries an
additional byte for the segment index:
`item_path = [item_idx, segment_idx, 0, 0], item_path_len = 2`.

**Visibility filtering in Slice 3**: only `Visibility::Always` items are emitted.
`OnLineHover` and `OnGroupHover` items are **skipped** in this slice — they come in
Slice 5.

**Testing**:
- `decorator_row_lays_out_right_to_left_at_right_edge` — two badges at `RightEdge`
  produce rects where the second is left of the first.
- `decorator_gap_adds_spacing_between_items`.
- `decorator_column_stacks_vertically`.
- `decorator_spacer_consumes_width_emits_no_primitives`.
- `decorator_badge_emits_segment_labels`.
- `decorator_badge_divider_emits_vertical_rect_between_segments`.
- `decorator_button_emits_one_fill_one_label_one_hit_zone`.
- `decorator_segment_with_action_emits_own_hit_zone`.
- `decorator_nested_stack_layout` — group with a `Stack` child places the stack
  correctly.
- `decorator_item_path_breadcrumb_for_nested_stack_child`.
- `decorator_on_hover_items_skipped_in_slice_3`.
- `decorator_at_timestamp_anchor_uses_camera_time_to_x`.
- `hit_zone_kind_is_copy` — compile-time: `fn assert_copy<T: Copy>() {}; assert_copy::<HitZoneKind>()`.

**Done when**: `cargo test -p midas-chart` passes. `compute_decorator_group()` produces
correct `WidgetOutput` for row/column layouts, handles nested stacks, emits per-item
and per-segment hit zones, and skips `OnHover` items. `HitZoneKind` still derives
`Copy`. No visual change in the running app yet (nothing calls this function).

---

### Slice 4: SDF badge GPU pipeline + `ChartScene` wiring

**Goal**: Stand up the SDF badge pipeline in `midas-render` so that once Slice 3's
compute engine starts emitting `BadgeInstance`s (which it can do immediately, because
the type lives in Slice 2), the render side is ready to draw them. The slice ends with
a dev-only flag that spawns a showcase annotation covering every shape variant, so
Slices 5/6/7 have a visual canary.

**Depends on**: Slice 2 only. Does NOT depend on Slice 3 — the two run in parallel.

**Size**: L.

**Runs in parallel with**: Slice 3.

**Pre-slice note**: If Slice 0 went "no-go", re-read its fallback section before
starting. The file list and shader layout below assume the SDF path. The geometry-
decomposition fallback replaces `badge.wgsl` with two smaller pipelines and splits
`BadgeInstance` across both — Slice 4 is then structurally larger (two pipeline files)
but individually simpler (no triangle SDF).

**Files to create or modify**:
- `desktop/win/crates/midas-chart/src/widget/compute.rs` — add
  `badges: Vec<BadgeInstance>` to `WidgetOutput`, update `merge()` to merge badge
  vectors.
- `desktop/win/crates/midas-chart/src/widget/decorator/compute.rs` — **integration
  ownership** (see "Slice 3 ‖ Slice 4 integration" note below). Replace Slice 3's
  placeholder `GridLineInstance` emission for non-axis-aligned shapes with
  `BadgeInstance` emission into `WidgetOutput.badges`. Axis-aligned `Rect` shapes keep
  emitting through `WidgetOutput.fills` as before (they cost less via the existing grid
  pipeline). This is the file that neither Slice 3 nor the old Slice 4 owned; explicit
  ownership of this edit is critical to avoid the "decorators stay rectangular forever"
  silent failure.
- `desktop/win/crates/midas-chart/src/scene.rs` — add `badges: Vec<BadgeInstance>`
  field to the **owned `ChartScene`** at line 20 (C2 fix part 1).
- `desktop/win/crates/midas-render/src/renderer.rs` — add `badges: &'a [BadgeInstance]`
  field to the **borrowed `ChartScene`** at line 20 (C2 fix part 2).
- `desktop/win/crates/midas-app/src/chart_widget.rs` — update the alias line / scene
  conversion that bridges `use midas_chart::scene::ChartScene` (owned) and
  `use midas_render::renderer::ChartScene as RenderScene` (borrowed). The borrowed
  `RenderScene` is built by borrowing each vec from the owned `ChartScene`, including
  the new `badges` field.
- `desktop/win/crates/midas-chart/src/compute/mod.rs` — copy `WidgetOutput::badges`
  into `ChartScene::badges` in the frame-assembly function.
- `desktop/win/crates/midas-render/src/pipelines/badge.rs` — **new file**.
  `BadgePipeline` struct with `new()`, `update_instances()`, `draw()`, following the
  shape of `GridPipeline` at `midas-render/src/pipelines/grid.rs` (the template).
- `desktop/win/crates/midas-render/src/pipelines/mod.rs` — add `pub mod badge;`.
- `desktop/win/crates/midas-render/shaders/badge.wgsl` — **new file**. Vertex +
  fragment shaders, lifted from the Slice 0 spike once the spike passes.
- `desktop/win/crates/midas-render/src/renderer.rs` — `ChartRenderer` struct at
  line 41; `new()` at line 51; `render()` updated to upload and draw badges (L1 fix —
  these are the verified line numbers).

**Slice 3 ‖ Slice 4 integration note**: Slice 3 lands
`compute_decorator_group()` with placeholder `GridLineInstance` emission for every
shape, so the compute engine is usable even before the SDF pipeline exists. Slice 4 is
then responsible for **two** integration edits that the earlier plan draft left
unowned: (1) wiring the `BadgeInstance` type through both `ChartScene`s and the render
pipeline (the C2 fix above), and (2) **flipping
`compute_decorator_group()`'s emission** from placeholder rects to real `BadgeInstance`
entries for every shape whose `shape_id != 0` (`Rect`). After Slice 4 merges, any
non-`Rect` decorator must emit one `BadgeInstance` and zero placeholder fills.
Forgetting this second edit was the silent failure mode flagged by the second
plan-eval pass — the decorator showcase masked it because the showcase writes directly
into `ChartScene.badges`, bypassing `compute_decorator_group()` entirely.

**Key implementation details**:

**C2 fix — both `ChartScene`s need the `badges` field**. There are two `ChartScene`
types in this codebase (this is important enough to repeat here because the old plan
missed it):

1. **Owned IR**: `desktop/win/crates/midas-chart/src/scene.rs:20`.
   `pub struct ChartScene { ... badges: Vec<BadgeInstance> }`.
2. **Borrowed view**: `desktop/win/crates/midas-render/src/renderer.rs:20`.
   `pub struct ChartScene<'a> { ... badges: &'a [BadgeInstance] }`.

The app-layer bridge uses both types via aliased imports:

```rust
// in midas-app/src/chart_widget.rs
use midas_chart::scene::ChartScene;              // owned
use midas_render::renderer::ChartScene as RenderScene;  // borrowed
```

Slice 4 must update **both** struct definitions and **both** construction sites. A
test (`scene_borrow_includes_badges`) verifies that the `RenderScene` built from an
owned `ChartScene` shares the `badges` slice.

**M1 fix — z-order is after `candle_bodies`, before `crosshair`**. The old plan put
badges "between grid and volume", which is wrong. Correct draw order:

```
grid → volume → volume_profile → candle_wicks → candle_bodies → BADGES → crosshair
```

Badges sit on top of candles so that the main price badge isn't hidden behind a green
body, but under the crosshair so that lens labels still win z-fighting.

**`BadgeInstance`** was landed in Slice 2; see that slice for the struct layout and
`shape_id` mapping. This slice only consumes it.

**WGSL fragment shader core** (`badge.wgsl`, ~60 lines total, copied from the Slice 0
spike):

```wgsl
struct BadgeVertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) local_uv: vec2<f32>,  // -1..1 inside the instance's rect
    @location(1) size: vec2<f32>,      // rect width/height in logical px
    @location(2) fill: vec4<f32>,
    @location(3) border: vec4<f32>,
    @location(4) shape_data: vec4<f32>, // (shape_id, shape_param, border_thickness, 0)
}

fn sdf_rect(p: vec2<f32>, size: vec2<f32>) -> f32 {
    let d = abs(p) - size * 0.5;
    return length(max(d, vec2<f32>(0.0))) + min(max(d.x, d.y), 0.0);
}

fn sdf_rounded(p: vec2<f32>, size: vec2<f32>, r: f32) -> f32 {
    let d = abs(p) - (size * 0.5 - vec2<f32>(r));
    return length(max(d, vec2<f32>(0.0))) + min(max(d.x, d.y), 0.0) - r;
}

fn sdf_circle(p: vec2<f32>, size: vec2<f32>) -> f32 {
    return length(p) - min(size.x, size.y) * 0.5;
}

// sdf_point_left, sdf_point_right, sdf_double_point, sdf_chevron:
// see badge_spike.wgsl from Slice 0.

@fragment fn fs_main(in: BadgeVertexOut) -> @location(0) vec4<f32> {
    let p = in.local_uv * in.size * 0.5;
    let shape_id = u32(in.shape_data.x);
    let param = in.shape_data.y;
    let border_thickness = in.shape_data.z;

    var d: f32 = 0.0;
    switch shape_id {
        case 0u: { d = sdf_rect(p, in.size); }
        case 1u: { d = sdf_rounded(p, in.size, param); }
        case 2u: { d = sdf_rounded(p, in.size, min(in.size.x, in.size.y) * 0.5); }
        case 3u: { d = sdf_point_left(p, in.size, param); }
        case 4u: { d = sdf_point_right(p, in.size, param); }
        case 5u: { d = sdf_double_point(p, in.size, param); }
        case 6u: { d = sdf_chevron(p, in.size, param); }
        case 7u: { d = sdf_circle(p, in.size); }
        default: { d = sdf_rect(p, in.size); }
    }

    let aa = fwidth(d);
    let fill_alpha = 1.0 - smoothstep(-aa, aa, d);
    if (border_thickness > 0.0 && in.border.a > 0.0) {
        let border_alpha = smoothstep(-border_thickness - aa, -border_thickness + aa, d)
                         * (1.0 - smoothstep(-aa, aa, d));
        let fill = in.fill * fill_alpha * (1.0 - border_alpha);
        let edge = in.border * border_alpha;
        return fill + edge;
    }
    return in.fill * fill_alpha;
}
```

**Pipeline plumbing**: follow `GridPipeline` at `midas-render/src/pipelines/grid.rs`
as the template — same unit-quad vertex layout, same camera bind group, just a bigger
per-instance vertex attribute struct matching `BadgeInstance`.

**`ChartRenderer` update — L1 verified line numbers**:
- `ChartRenderer` struct at `renderer.rs:41` — add `badge_pipeline: BadgePipeline`.
- `ChartRenderer::new()` at `renderer.rs:51` — initialize `badge_pipeline` alongside
  the other pipelines.
- `render()` — insert the upload + draw block **after `candle_bodies`, before
  `crosshair`**:

```rust
// Upload + draw badges (after candle_bodies, before crosshair).
if !scene.badges.is_empty() {
    self.badge_pipeline.update_instances(&self.device, &self.queue, scene.badges);
    self.badge_pipeline.draw(&mut render_pass);
}
```

**L4 fix — decorator showcase feature flag**. Slice 4's Done criteria include a
dev-only hook that spawns a synthetic annotation with one badge of every shape, so
that Slices 5/6/7 have a visual canary and humans can eyeball the pipeline before the
real migrations land. Two acceptable implementations:

1. **Cargo feature**: add `decorator-showcase` feature to `midas-app/Cargo.toml`. When
   enabled, `main.rs` calls `spawn_decorator_showcase(&mut annotation_store)` once at
   startup, which inserts a single annotation whose render path emits one decorator
   group containing eight badges (one per `BadgeShape`), laid out in a row at a known
   price.
2. **Env var**: `MIDAS_DECORATOR_SHOWCASE=1` read in `main.rs` at startup, same
   effect. This avoids a Cargo feature flag and recompilation.

Implementer's choice; env var is simpler. The showcase code lives in
`midas-app/src/decorator_showcase.rs` (new file, ~30 lines) and is behind a
`#[cfg(debug_assertions)]` so it never ships in release builds.

**Testing**:
- `badge_instance_pod_roundtrip` (already landed in Slice 2).
- `scene_borrow_includes_badges` — verify the borrowed `RenderScene` shares the
  owned `ChartScene::badges` slice.
- `chart_scene_default_badges_empty` — the frame-assembly default is no badges.
- `render_draw_order_badges_after_candle_bodies_before_crosshair` — structural test on
  the `render()` function body (or a comment-anchored assertion if structural tests
  are impractical).
- **`compute_decorator_group_emits_badge_instance_for_point_left`** — the
  Slice 3 ‖ Slice 4 integration-gap regression test. Construct a `DecoratorGroup` with
  a single `Badge { shape: BadgeShape::PointLeft { point_width: 8.0 }, .. }`, run
  `compute_decorator_group()`, assert that the resulting `WidgetOutput` contains
  **exactly one** `BadgeInstance` in `badges` with `shape_id == 3` (PointLeft), and
  **zero** placeholder fills for that badge's bounding rect in `fills`. Add matching
  assertions for `Rounded`, `Pill`, `Circle`, and `Chevron`. `Rect` still emits through
  `fills` — that's the one exception.
- **`compute_decorator_group_rect_shape_still_uses_fills`** — positive control for the
  `Rect` exception. One rect badge, zero entries in `badges`, one entry in `fills`.
- **Visual test** (manual): run the app with the showcase flag set and eyeball the
  eight badges.

**Done when**:
- Running the app with the decorator showcase flag shows all eight shapes with real
  SDF rendering (not bounding-rect placeholders).
- `cargo test --workspace` passes.
- `cargo clippy --workspace -- -D warnings` clean.
- Frame budget unchanged (one extra draw call per frame is negligible).
- The `badges` field is wired through BOTH `ChartScene` types, Slice 3 can land its
  compute path against the same field, and the showcase annotation is visible.

---

### Slice 5: Two-pass hover compute + `update()`-based recompute

**Goal**: Make `OnLineHover` and `OnGroupHover` items actually appear when the user
hovers. Persist the expanded-group state across frames so buttons stay alive when the
pointer moves from the line onto them.

**Depends on**: Slice 3 only. Whether real SDF badges (Slice 4) or bounding-rect
placeholders are drawn is irrelevant to the hover state machine — Slice 5 operates on
`HitZone`s and `WidgetOutput`, both of which are sans-IO.

**Size**: M.

**Files to create or modify**:
- `desktop/win/crates/midas-chart/src/state/mod.rs` — add
  `hovered_decorator_groups: SmallVec<[(AnnotationId, u16); 2]>` to `ChartState` at
  line 126. Initialize empty in `ChartState::new()`.
- `desktop/win/crates/midas-chart/src/input.rs` — add
  `hovered_decorator_groups: &'a [(AnnotationId, u16)]` to `ChartInput` at line 18.
- `desktop/win/crates/midas-chart/src/widget/compute.rs` — add the same field to
  `ComputeContext` at line 20.
- `desktop/win/crates/midas-chart/src/compute/mod.rs` — pass through in
  `compute_widget_annotations()` at line 1077.
- `desktop/win/crates/midas-chart/src/widget/decorator/compute.rs` — two-pass emission:
  first `Always`, then `OnLineHover` / `OnGroupHover` conditionally.
- `desktop/win/crates/midas-app/src/chart_widget.rs` — extend `update()` to compute the
  new hover set on every mouse-move; extend `draw()` to pass the set through
  `ChartInput`.

**Key implementation details**:

**Two-pass emission** inside `compute_decorator_group()`:

```rust
let line_hovered = ctx.hovered_annotation
    .map(|aid| aid == annotation_id)
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
    // ... emit as before
}
```

**H3 fix — recompute-in-`update()` is the default, `RefCell` cache is documented fallback
only**. The authoritative design is in [05-interaction.md](05-interaction.md) under
"Hover set update strategy". The short version:

On every `update()` mouse-move event, `chart_widget.rs` does a cheap recompute of
decorator hit zones for the currently-visible annotations. Because `price_to_y()` is
O(1), the flex layout pass is linear in the number of items (typically < 20 per chart),
and the measurement heuristic is a single multiply, the recompute costs low
microseconds and runs ONCE per mouse-move event (not per frame). The hover set is then
updated from the recompute result.

Pseudocode:

```rust
// chart_widget.rs::update(), on mouse-move:
let decorator_hit_zones = recompute_decorator_hit_zones(
    &self.annotation_store,
    &chart_state,         // read-only
    &viewport,
);
let mut new_groups: SmallVec<[(AnnotationId, u16); 2]> = smallvec![];
if let Some((hovered_aid, _)) = chart_state.hovered_annotation {
    for gid in groups_for_annotation(hovered_aid, &self.annotation_store) {
        new_groups.push((hovered_aid, gid));
    }
}
for hit in &decorator_hit_zones {
    if let HitZoneKind::Decorator { group_id, .. } = hit.kind {
        let [x0, y0, x1, y1] = hit.rect;
        if pos.x >= x0 && pos.x <= x1 && pos.y >= y0 && pos.y <= y1 {
            let entry = (hit.annotation_id, group_id);
            if !new_groups.contains(&entry) {
                new_groups.push(entry);
            }
        }
    }
}
chart_state.hovered_decorator_groups = new_groups;
```

Because the recompute happens in `update()` (which takes `&mut State`), there is no
lifetime-with-`draw()` problem and no need for a `RefCell` cache of the previous
frame's hit zones. The old plan's `RefCell<Vec<HitZone>>` caching scheme is kept in
[05-interaction.md](05-interaction.md) as a **documented fallback** if profiling shows
the recompute is too expensive in some pathological case.

**L2 fix — first-frame hover edge case**. The canonical walkthrough is in
[05-interaction.md](05-interaction.md) under "First-frame hover edge case (walkthrough)"
— read it there and implement the algorithm exactly as stated. The short version:
`hovered_annotation`'s hit-test considers the line's hit zone **or** any expanded
decorator group's hit zones belonging to the same annotation, so when the cursor
crosses from the line onto a freshly-revealed button, the annotation stays hovered via
the decorator group path even though the cursor is no longer over the line itself.
This one rule closes the first-frame gap that a naive "cursor-over-line" check would
open. Slice 5's implementation must match 05-interaction.md's steps 1–5 verbatim; the
implementation shall not invent a parallel mechanism.

**Testing**:
- `decorator_on_line_hover_emitted_when_parent_hovered` — mock `hovered_annotation`,
  verify items appear.
- `decorator_on_group_hover_persists_when_group_expanded` — mock
  `hovered_decorator_groups`, verify items stay visible even when `hovered_annotation`
  is `None`.
- `decorator_hover_set_update_adds_groups_for_hovered_line`.
- `decorator_hover_set_update_keeps_groups_with_cursor_over_item`.
- `decorator_hover_set_update_drops_groups_when_cursor_leaves_both_line_and_items`.
- `decorator_first_frame_hover_no_flicker` — regression test for the L2 edge case.

**Done when**: Running the app, hovering a test bracket line reveals the hover-only
buttons, moving the cursor onto one of the buttons keeps them visible, and moving off
of both the line and all buttons hides them again. Clicking the decorator showcase
annotation (still emitted from Slice 4) shows the hover-only items correctly. All
tests pass.

---

### Slice 6: `DecoratorAction` routing through `ChartAction::DecoratorClick`

**Goal**: Wire up clicks. A click on a decorator item produces a
`ChartAction::DecoratorClick { annotation_id, group_id, item_path, action }` that the
app layer matches on.

**Depends on**: Slice 5 (hit zones with `DecoratorAction` are present, hover state
machine is live so OnGroupHover buttons are actually hittable).

**Size**: S.

**Files to create or modify**:
- `desktop/win/crates/midas-chart/src/interaction/mod.rs` — add
  `ChartAction::DecoratorClick` variant at line 61; extend the mouse-press handler to
  emit it when a click lands in a `HitZoneKind::Decorator` hit zone.
- `desktop/win/crates/midas-app/src/chart_widget.rs` — match on the new variant in the
  action dispatch loop; map each `DecoratorAction` to the appropriate side effect
  (broker command, UI state change, or `ChartEngineCommand`).

**Key implementation details**:

New `ChartAction` variant:

```rust
DecoratorClick {
    annotation_id: AnnotationId,
    group_id: u16,
    item_path: [u8; 4],
    item_path_len: u8,
    action: DecoratorAction,
}
```

Mouse-press handling in `interaction/mod.rs` — extend the existing hit-test loop (the
one that already handles `BracketSubmit` / `BracketCancel` / etc. — those arms stay
alive in this slice; they're removed in Slice 8b) to also match `HitZoneKind::Decorator`:

```rust
match hit.kind {
    HitZoneKind::Decorator { group_id, item_path, item_path_len, action } => {
        return Some(ChartAction::DecoratorClick {
            annotation_id: hit.annotation_id,
            group_id,
            item_path,
            item_path_len,
            action,
        });
    }
    // ... existing arms (BracketSubmit, BracketCancel, etc. — NOT removed yet)
}
```

App-layer dispatch in `chart_widget.rs` — one match arm per `DecoratorAction` variant:

```rust
ChartAction::DecoratorClick { annotation_id, group_id: _, item_path: _, item_path_len: _, action } => {
    match action {
        DecoratorAction::CloseAnnotation => {
            self.annotation_store.remove(annotation_id);
        }
        DecoratorAction::CreateTakeProfit => {
            if let Some(bracket) = self.annotation_store.get_bracket_mut(annotation_id) {
                attach_default_tp(bracket);
            }
        }
        DecoratorAction::CreateStopLoss => {
            if let Some(bracket) = self.annotation_store.get_bracket_mut(annotation_id) {
                attach_default_sl(bracket);
            }
        }
        DecoratorAction::CycleEntryType => { /* ... */ }
        DecoratorAction::EditQuantity => { /* open popup */ }
        DecoratorAction::EditPrice => { /* enter drag mode */ }
        DecoratorAction::Submit => { /* forward to broker bridge */ }
        DecoratorAction::Save => { /* mark as saved */ }
        DecoratorAction::ToggleLocked => { /* flip Annotation.locked */ }
        DecoratorAction::Custom(_) => { /* reserved */ }
    }
}
```

**Testing**:
- `mouse_press_on_decorator_emits_click_action` — mock a hit zone, verify
  `ChartAction::DecoratorClick` is produced.
- `decorator_click_close_annotation_removes_from_store` (app-layer).
- `decorator_click_create_tp_attaches_default_tp_to_bracket` (app-layer).
- End-to-end: click the `[X]` button on the decorator showcase annotation from Slice 4
  → annotation disappears from the store.

**Done when**: The decorator showcase annotation's action-bearing items (`[X]`, `▲`,
`▼`, etc.) are clickable and trigger the correct side effects. The legacy bracket
buttons (`BracketSubmit` / `Save` / `ToggleSL` / `Cancel` / `CancelSL`) are still
alive and unchanged — Slice 8b is where they get deleted. `cargo test --workspace`
passes.

**Slice 7 follow-up (one-line edit)**: if Slice 7 has already shipped when Slice 6
lands, Slice 6 also flips the `HorizontalLevel::to_decorators()` price-badge
`action` field from `None` to `Some(DecoratorAction::EditPrice)`. This is a
one-line edit in `midas-chart/src/levels.rs`, gated behind the Slice 7 milestone —
after it, clicking a level's right-edge price badge emits
`ChartAction::DecoratorClick { action: EditPrice }` and enters inline-edit mode
via the app-layer handler, instead of falling through to the line-drag path. If
Slice 6 and Slice 7 ship in parallel, whichever ships second owns this flip.

---

### Slice 7: Migrate `HorizontalLevel` onto `PriceLine` + decorators

**Goal**: Delete the duplication. `HorizontalLevel` (persisted) is rewritten to compose
`PriceLine` + decorators. The `widget::level::HorizontalLevel` (the second, renderer-
side one) is deleted entirely. `compute_level()` becomes a thin wrapper that constructs
a `PriceLine` and a `Vec<DecoratorGroup>` from the level's data and calls
`compute_decorator_group()` once per group.

**Depends on**: Slices 4 + 5 (shapes render correctly and hover works). Does NOT
depend on Slice 6. Levels have no hover-revealed buttons; their decorators are all
`Visibility::Always`. The right-edge price badge is emitted with **`action: None`** in
this slice — clicks on it fall through to the existing `HitZoneKind::LevelLine` hit
zone at `widget/level.rs:219`, which still handles drag-to-edit exactly as today, so
there is no interactive regression. A follow-up edit inside Slice 6 (one line) flips
the price badge's `action` to `Some(DecoratorAction::EditPrice)` after
`ChartAction::DecoratorClick` routing lands. Runs concurrently with Slice 6.

**Size**: L.

**Files to create or modify**:
- `desktop/win/crates/midas-chart/src/levels.rs` — rewrite `HorizontalLevel` to compose
  `PriceLine`. Add a `to_decorators()` method that builds the standard level decorator
  set (right-edge price badge, optional left-edge label/icon badge, optional lock
  badge).
- `desktop/win/crates/midas-chart/src/widget/level.rs` — delete the second
  `HorizontalLevel`, `LevelExtend`, and the old `compute_level()` body. Rewrite
  `compute_level()` to call `compute_decorator_group()`. `segmented_line()` stays
  (still used for line geometry).
- `desktop/win/crates/midas-chart/src/compute/mod.rs` — update `AnnotationKind::Level`
  dispatch (currently at line 1119 in the non-hovered pass and line 1140 in the
  hovered pass — L1 verified line numbers) to use the new compute path. The `locked`
  argument is sourced from `ann.locked` at this call site (see M2 note below).
- `desktop/win/crates/midas-app/src/level_store/mod.rs` — update to work with the new
  `HorizontalLevel` shape; this file IS one of the two serialization writers (see M3
  note below).
- `desktop/win/crates/midas-app/src/annotation_persistence.rs` — the OTHER writer.
  Levels that arrive inside an `Annotation` wrapper are written through this path too
  (pretty-printed JSON, via `serde_json`). Both writers must agree on the on-disk
  shape after this slice.
- `desktop/win/tests/fixtures/config_v1_pre_decorator.toml` — **new fixture**. A real
  pre-refactor `config.toml` snapshot checked into the repo, used by the migration
  test.

**Key implementation details**:

**M8 fix — `midas-store` verification finding**:

> **Verified**: `HorizontalLevel`/`Annotation`/`BracketLeg`/`LineStyle` are NOT
> persisted in `midas-store` DuckDB. Grepping
> `desktop/win/crates/midas-store/` for `HorizontalLevel`, `BracketLeg`, `LineStyle`,
> `Level`, `Annotation`, `annotation`, `bracket`, `level` returns zero hits. Inspection
> of `midas-store/src/schema.rs` shows no annotation/level/bracket tables. Annotations
> are persisted **only at the app layer**, via JSON files written by
> `midas-app/src/annotation_persistence.rs` (`AnnotationFile { version: u32, symbol:
> String, annotations: Vec<Annotation> }`, stored at `data/annotations/<SYMBOL>.json`,
> atomic tmp-then-rename) and via `midas-app/src/level_store/mod.rs` (`LevelStore`,
> in-memory `HashMap<String, Vec<HorizontalLevel>>` written out as part of TOML
> `config.toml` via `midas-core::config::LevelConfig`). **Only TOML config migration
> and JSON annotation migration are needed — no DuckDB migration step.**

**M2 fix — `locked` stays on the `Annotation` wrapper**: `Annotation.locked` lives on
the wrapper struct at `widget/mod.rs:146`, NOT on the inner `HorizontalLevel`. The new
`HorizontalLevel` does NOT duplicate a `locked` field. The signature of `compute_level()`
stays 5-arg:

```rust
pub fn compute_level(
    level: &HorizontalLevel,
    annotation_id: AnnotationId,
    ctx: &ComputeContext<'_>,
    alpha: f32,
    locked: bool,
) -> WidgetOutput
```

The `locked: bool` comes from `ann.locked` at the call site in `compute/mod.rs` (the
non-hovered pass around line 1119 and the hovered pass around line 1140):

```rust
AnnotationKind::Level(level) => {
    compute_level(level, ann.id, ctx, alpha, ann.locked)
}
```

`to_decorators()` takes `locked` as an argument too, so it can conditionally emit the
lock-badge group:

```rust
impl HorizontalLevel {
    pub fn to_decorators(&self, locked: bool) -> Vec<DecoratorGroup> { ... }
}
```

New `HorizontalLevel`:

```rust
// levels.rs
pub struct HorizontalLevel {
    pub id: u64,
    pub line: PriceLine,
    pub label: Option<String>,
    pub icon: LevelIcon,
    // NO `locked` field — it lives on Annotation.
}

impl HorizontalLevel {
    pub fn to_decorators(&self, locked: bool) -> Vec<DecoratorGroup> {
        let mut groups = Vec::new();
        // Group 0: right-edge price badge.
        groups.push(DecoratorGroup {
            group_id: 0,
            anchor: DecoratorAnchor::RightEdge,
            direction: FlexDirection::Row,
            gap: 0.0,
            items: smallvec![
                DecoratorItem {
                    visibility: Visibility::Always,
                    // action: None in Slice 7 — clicks fall through to the existing
                    // LevelLine drag hit zone at widget/level.rs:219, so no
                    // regression while Slice 6 is not yet shipped. Slice 6's
                    // follow-up edit flips this to Some(DecoratorAction::EditPrice)
                    // once ChartAction::DecoratorClick routing is in place.
                    action: None,
                    content: ItemContent::Badge(Badge {
                        shape: BadgeShape::Rect,
                        fill: [0.12, 0.12, 0.15, 0.85],
                        border: None,
                        height: 18.0,
                        padding: 6.0,
                        segments: smallvec![BadgeSegment {
                            text: format!("{:.2}", self.line.price),
                            text_color: self.line.stroke.color,
                            font_size: 11.0,
                            min_width: None,
                            fill_override: None,
                            shape_override: None,
                            action: None,
                        }],
                        divider_color: None,
                    }),
                },
            ],
        });
        // Group 1: left-edge label/icon badge (only if label or icon present).
        if self.label.is_some() || self.icon != LevelIcon::None {
            // ... build and push
        }
        // Group 2: lock badge (only if locked — sourced from Annotation wrapper).
        if locked {
            // ... build and push
        }
        groups
    }
}
```

New `compute_level()`:

```rust
pub fn compute_level(
    level: &HorizontalLevel,
    annotation_id: AnnotationId,
    ctx: &ComputeContext<'_>,
    alpha: f32,
    locked: bool,
) -> WidgetOutput {
    let mut out = WidgetOutput::default();
    // 1. Render the line geometry itself.
    out.merge(compute_price_line_geometry(&level.line, annotation_id, ctx, alpha));
    // 2. Render each decorator group.
    for group in level.to_decorators(locked) {
        out.merge(compute_decorator_group(&group, &level.line, annotation_id, ctx, alpha));
    }
    out
}
```

`compute_price_line_geometry()` is a new small helper that handles just the line
primitives: the `segmented_line` call, selection glow, drag ghost, and the line's
own hit zone. Everything else (labels, icons, lock) lives in decorators.

**M3 fix — concrete config migration via manual `Deserialize` with a v1 fallback
struct (NOT untagged enum)**. Both `midas-app/src/annotation_persistence.rs` (JSON,
inside `AnnotationFile`) and `midas-app/src/level_store/mod.rs` (TOML, inside
`LevelConfig`) are writers that need to agree on the on-disk shape. Implementation:

```rust
// In midas-chart/src/levels.rs, next to HorizontalLevel.

#[derive(Deserialize)]
struct HorizontalLevelV1 {
    // Legacy flat shape: color, line_width, label, icon, locked at top level.
    id: u64,
    price: f64,
    color: [f32; 4],
    line_width: f32,
    style: LineStyle,
    label: Option<String>,
    icon: LevelIcon,
    // note: v1 has `locked` here; v2 moves it to the Annotation wrapper,
    // so the migration drops it. A one-time log line warns if any level
    // had locked=true — those callers need to flip Annotation.locked manually.
}

impl<'de> Deserialize<'de> for HorizontalLevel {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        // Buffer once, try v2 first, then v1.
        let content = serde::__private::de::Content::deserialize(d)?;
        if let Ok(v2) = HorizontalLevelV2::deserialize(
            serde::__private::de::ContentRefDeserializer::<D::Error>::new(&content)
        ) {
            return Ok(v2.into());
        }
        if let Ok(v1) = HorizontalLevelV1::deserialize(
            serde::__private::de::ContentRefDeserializer::<D::Error>::new(&content)
        ) {
            return Ok(v1.into());
        }
        Err(de::Error::custom("HorizontalLevel: neither v2 nor v1 shape matched"))
    }
}

// Sketch (implementer's choice — an explicit #[serde(untagged)] wrapper enum with
// v2-first-then-v1 works as well; the important part is "try v2 first, fall back to
// v1", not the specific serde mechanism).
```

**Both** writer call sites are updated in this slice:

1. `midas-app/src/level_store/mod.rs:19` — `LevelStore` owns the primary in-memory
   copy. Any path that serializes `LevelStore` via `midas-core::config::LevelConfig`
   walks through the `Serialize` impl of the new `HorizontalLevel` and writes v2
   shape.
2. `midas-app/src/annotation_persistence.rs` — the `AnnotationFile` JSON path. Because
   `Annotation` contains a `HorizontalLevel` via `AnnotationKind::Level(level)`, the
   JSON path also writes v2 shape via `serde_json::to_string_pretty`. No explicit
   migration on write; the `Deserialize` impl handles load-time migration.

A fixture test loads the real pre-refactor config:

```rust
#[test]
fn level_store_loads_v1_config_toml_fixture() {
    let path = Path::new("../../tests/fixtures/config_v1_pre_decorator.toml");
    let text = std::fs::read_to_string(path).unwrap();
    let cfg: LevelConfig = toml::from_str(&text).unwrap();
    let levels = cfg.levels_for("AAPL");
    assert!(!levels.is_empty());
    assert_eq!(levels[0].line.price, 189.42);  // value from the fixture
    assert!(matches!(levels[0].line.stroke.style, LineStyle::Solid));
}
```

Similarly for JSON:

```rust
#[test]
fn annotation_persistence_loads_v1_json_fixture() {
    let json = include_str!("../../../tests/fixtures/annotations_v1_pre_decorator.json");
    let file: AnnotationFile = serde_json::from_str(json).unwrap();
    assert!(!file.annotations.is_empty());
    // assert the first level parsed correctly
}
```

**Testing**:
- `horizontal_level_to_decorators_right_badge_shows_price`.
- `horizontal_level_to_decorators_left_badge_shows_label`.
- `horizontal_level_to_decorators_icon_only_no_label_group` — verify icon alone still
  produces a group.
- `horizontal_level_to_decorators_locked_emits_lock_badge` — verify the `locked`
  parameter threads through.
- `horizontal_level_config_v1_migrates_to_new_shape` — serde round-trip against the
  `HorizontalLevelV1` fallback struct.
- `level_store_loads_v1_config_toml_fixture` — full load of the checked-in fixture.
- `annotation_persistence_loads_v1_json_fixture` — full load of a JSON fixture.
- `compute_level_visual_parity_with_pre_refactor` — render a level with known params,
  compare output primitive counts to a snapshot.
- `compute_level_hit_zones_include_line_plus_decorators`.
- `compute_level_signature_takes_locked_bool` — compile-time check that the 5-arg
  signature is preserved.

**Done when**: Levels render visually identical to before, hit zones still work, the
v1 TOML fixture and v1 JSON fixture both load through the new `Deserialize` impl,
and `grep widget::level::HorizontalLevel` returns zero hits for the struct definition
(the module file still exists but the struct is gone; `LineStyle` and `segmented_line`
stay).

---

### Slice 8a-i: `BracketLeg` data-model rewrite (rendering via shim)

**Goal**: Rewrite `BracketLeg` to own a `PriceLine`. Delete the top-level
`color`/`line_width`/`style`/`label` fields. Update `leg_style()` at line 207 to return
a `LineStroke` instead of raw style bytes, and apply it when constructing each leg's
`PriceLine`. `compute_bracket()` continues to render pixel-identical to today via a
**thin shim** that reconstructs the old `WidgetOutput` emission from `PriceLine.stroke`
— no decorator constructors yet, no `compute_decorator_group()` calls, no visual
change at all. This slice is a pure data-model migration: 0 visual diff, 0 behavioral
diff, 0 test semantics diff.

Splitting the old monolithic Slice 8 into 8a-i (data-model) and 8a-ii (visual)
produces two independently-reviewable PRs and surfaces pure data-model regressions
before the visual migration begins. 8a-i is revertable in isolation if snapshot
comparisons drift.

**Depends on**: Slice 7 (pattern proven on the simpler level case). **Soft-ordered
after Slice 6**: Slice 8a-i does not import or reference `ChartAction::DecoratorClick`,
so it can compile and run without Slice 6 having landed. On a two-engineer schedule
it runs in parallel with Slice 6 — Engineer B takes 8a-i while Engineer A finishes
Slice 6, closing one of the idle windows.

**Size**: M.

**Files to create or modify**:
- `desktop/win/crates/midas-chart/src/widget/order_bracket/mod.rs` — rewrite
  `BracketLeg` to own a `PriceLine`. Delete the `color`/`line_width`/`style`/`label`
  top-level fields. Rewrite `leg_style()` at line 207 to return `LineStroke` (the
  `(status, role) → LineStroke` mapping moves into this helper; the returned
  `LineStroke` is stamped onto `leg.line.stroke` before emission). Update
  `compute_bracket()`'s body so every read of the deleted fields routes through
  `leg.line.stroke.{color,width,style}` or the decorator-builder functions (not yet
  present in 8a-i — added in 8a-ii).
- `desktop/win/crates/midas-chart/src/widget/order_bracket/shim.rs` — **new, temporary
  file**. A small helper that reconstructs the exact legacy `WidgetOutput` a bracket
  leg used to produce, given a `BracketLeg` of the new shape. `compute_bracket()`
  calls this shim during 8a-i. The file is deleted in 8a-ii once
  `compute_decorator_group()` takes over the visual emission.
- `desktop/win/crates/midas-chart/src/widget/order_bracket/tests.rs` — update the ~30
  existing tests for the new field layout. Most assertions rewrite `leg.color` →
  `leg.line.stroke.color`, `leg.line_width` → `leg.line.stroke.width`, and so on.
  Test SEMANTICS stay identical: every assertion still passes against identical
  rendered output.

**Key implementation details**:

**M9 fix — `midas-store` verification for `BracketLeg`**:

> **Verified**: `BracketLeg` and `OrderBracket` are NOT persisted in `midas-store`
> DuckDB. Grepping `desktop/win/crates/midas-store/` for `BracketLeg`, `OrderBracket`,
> `bracket` returns zero hits. Inspection of `schema.rs` shows tables only for
> candles/market data. Bracket annotations live entirely in-memory in the
> `AnnotationStore` on the app layer; when persisted at all, they go through
> `midas-app/src/annotation_persistence.rs` as JSON alongside other annotations. **No
> DuckDB migration step is needed for brackets. The JSON fallback-Deserialize pattern
> from Slice 7 extends naturally to `BracketLeg` via an analogous `BracketLegV1`
> struct** — add one to `order_bracket/mod.rs` if existing bracket JSON files need to
> survive the refactor. In practice, Draft brackets are ephemeral and Active brackets
> are keyed to live broker order IDs, so the migration risk is low.

New `BracketLeg`:

```rust
pub struct BracketLeg {
    pub line: PriceLine,
    pub role: LegRole,
    pub projected_pnl: Option<f64>,
    pub projected_pnl_pct: Option<f64>,
    // no `color`, `line_width`, `style`, or `label` — those now live on `line.stroke`
    // or are computed at decorator-build time (8a-ii).
}
```

**Rendering shim**: `compute_bracket()` inside 8a-i calls
`shim::emit_legacy_bracket_leg(&leg, ctx, alpha)` for each visible leg. The shim reads
`leg.line.stroke` and produces exactly the same `WidgetOutput` that the pre-refactor
`compute_bracket()` produced. This gives the team a clean "pure data model" PR with
zero visual risk, and makes the 8a-ii switch to `compute_decorator_group()` a
visual-only change that can be reviewed as such.

**Testing**:
- All ~30 existing bracket tests pass against the new field layout with zero semantic
  changes.
- `bracket_leg_data_model_v1_roundtrip` — fixture test: load a checked-in pre-refactor
  bracket JSON (if brackets are persisted — optional per the M9 finding) and verify
  round-trip via the same manual-`Deserialize` pattern Slice 7 uses.
- `compute_bracket_visual_parity_with_pre_refactor` — the critical snapshot test:
  compare the `WidgetOutput` primitive counts (fills, lines, labels, hit zones) before
  and after the data-model rewrite. Must match exactly. This is the 8a-i gate.
- `bracket_leg_v1_json_migration` — if brackets are persisted, load the checked-in
  `annotations_v1_pre_decorator.json` fixture through the fallback `Deserialize` and
  verify the in-memory shape after migration matches the expected new shape.

**Done when**: `cargo test --workspace` passes with no snapshot-test diffs. The
rendered output of a Draft bracket is **pixel-identical** to pre-refactor, confirmed
by the visual parity snapshot test. `cargo clippy --workspace -- -D warnings` clean.
The `shim.rs` file exists and is the only caller of the old emission logic.

---

### Slice 8a-ii: Visual decorator emissions (the screenshot payoff)

**Goal**: Build `entry_decorator_group()`, `tp_decorator_group()`,
`sl_decorator_group()` helpers. Route `compute_bracket()` through
`compute_decorator_group()` for the visual layer. Delete `shim.rs`. After this slice,
Draft brackets render with the exact tag designs from the screenshots — pointed-left
main badge with `P/5000/45.01` segments, close button, hover-reveal quick-create
stack.

**IMPORTANT**: Legacy bracket button hit zones (`HitZoneKind::BracketSubmit`, `Save`,
`ToggleSL`, `Cancel`, `CancelSL`) **stay alive in this slice**. The legacy
interaction path remains the source of truth for clicks; the decorator emissions are
purely visual. This prevents any interactive regression during the visual migration.
The hit-zone variant deletion and interactive cutover happen in Slice 8b.

**Depends on**: Slice 8a-i (data-model landed). Soft-ordered after Slice 6 (same
rationale as 8a-i: no `ChartAction::DecoratorClick` references yet).

**Size**: M.

**Files to create or modify**:
- `desktop/win/crates/midas-chart/src/widget/order_bracket/decorators.rs` — **new
  file**. Constructor functions for the standard bracket decorator sets:
  `entry_decorator_group()`, `tp_decorator_group()`, `sl_decorator_group()`.
- `desktop/win/crates/midas-chart/src/widget/order_bracket/mod.rs` — rewrite
  `compute_bracket()` to call `compute_decorator_group()` once per group (entry + TP +
  SL) instead of the shim. Remove the `shim::emit_legacy_bracket_leg()` call sites.
- `desktop/win/crates/midas-chart/src/widget/order_bracket/shim.rs` — **deleted**. No
  longer called.
- `desktop/win/crates/midas-chart/src/widget/order_bracket/tests.rs` — update
  assertions from "leg produces line + two labels" to "leg produces line + one
  decorator group with N items" where appropriate. Many tests stay identical because
  they operate at the `WidgetOutput` primitive-count level.

**Key implementation details**:

**Visual parity goal**: the screenshot tag designs appear on screen. Specifically:

- **Entry (Limit, Long)**: pointed-left tag with three segments `[P | 5000 | 45.01]`,
  hover-reveals a close button on the left and a `▲/▼` stack on the right.
- **TP**: pointed-left tag with `[T | ②(circle) | 12.3% | 47.50]`, no hover buttons.
- **SL**: pointed-left tag with `[S | $150 | 44.00]`, orange fill, dotted-pattern line.

**Entry decorator group constructor** (matches screenshot 2 — this is where the
screenshot payoff lives):

```rust
// order_bracket/decorators.rs

fn entry_decorator_group(bracket: &OrderBracket) -> DecoratorGroup {
    let color_main = entry_base_color(bracket.side, bracket.entry_type);
    let color_light = lighten(color_main, 0.3);
    let color_dark = darken(color_main, 0.3);
    DecoratorGroup {
        group_id: 0,
        anchor: DecoratorAnchor::RightEdge,
        direction: FlexDirection::Row,
        gap: 3.0,
        items: smallvec![
            // Hover-only close button at the far left of the group.
            DecoratorItem {
                visibility: Visibility::OnGroupHover,
                action: Some(DecoratorAction::CloseAnnotation),
                content: ItemContent::Button(Button {
                    shape: BadgeShape::Rounded { radius: 2.0 },
                    fill: color_main,
                    hover_fill: Some(color_light),
                    glyph: 'X',
                    glyph_color: [1.0, 1.0, 1.0, 1.0],
                    glyph_size: 12.0,
                    size: [18.0, 18.0],
                    border: None,
                }),
            },
            // Main pointed-left tag.
            DecoratorItem {
                visibility: Visibility::Always,
                action: None,
                content: ItemContent::Badge(Badge {
                    shape: BadgeShape::PointLeft { point_width: 8.0 },
                    fill: color_main,
                    border: None,
                    height: 20.0,
                    padding: 6.0,
                    segments: smallvec![
                        BadgeSegment {
                            text: entry_type_glyph(bracket.entry_type).to_string(),
                            text_color: [1.0; 4],
                            font_size: 11.0,
                            min_width: Some(14.0),
                            fill_override: Some(color_light),
                            shape_override: None,
                            action: Some(DecoratorAction::CycleEntryType),
                        },
                        BadgeSegment {
                            text: format_quantity(bracket.quantity),
                            text_color: [1.0; 4],
                            font_size: 11.0,
                            min_width: Some(44.0),
                            fill_override: None,
                            shape_override: None,
                            action: Some(DecoratorAction::EditQuantity),
                        },
                        BadgeSegment {
                            text: format!("{:.2}", bracket.entry.line.price),
                            text_color: [1.0; 4],
                            font_size: 11.0,
                            min_width: None,
                            fill_override: Some(color_dark),
                            shape_override: None,
                            action: Some(DecoratorAction::EditPrice),
                        },
                    ],
                    divider_color: Some(color_dark),
                }),
            },
            // Hover-only vertical stack of quick-create buttons on the right.
            DecoratorItem {
                visibility: Visibility::OnGroupHover,
                action: None,
                content: ItemContent::Stack(Box::new(DecoratorGroup {
                    group_id: 1,  // sub-group within the entry
                    anchor: DecoratorAnchor::RightEdge,  // ignored when nested
                    direction: FlexDirection::Column,
                    gap: 1.0,
                    items: smallvec![
                        DecoratorItem {
                            visibility: Visibility::Always,
                            action: Some(DecoratorAction::CreateTakeProfit),
                            content: ItemContent::Button(Button {
                                shape: BadgeShape::Rect,
                                fill: [0.15, 0.85, 0.85, 1.0],
                                hover_fill: Some([0.25, 0.95, 0.95, 1.0]),
                                glyph: '▲',
                                glyph_color: [1.0; 4],
                                glyph_size: 10.0,
                                size: [14.0, 10.0],
                                border: None,
                            }),
                        },
                        DecoratorItem {
                            visibility: Visibility::Always,
                            action: Some(DecoratorAction::CreateStopLoss),
                            content: ItemContent::Button(Button {
                                shape: BadgeShape::Rect,
                                fill: [0.15, 0.85, 0.85, 1.0],
                                hover_fill: Some([0.25, 0.95, 0.95, 1.0]),
                                glyph: '▼',
                                glyph_color: [1.0; 4],
                                glyph_size: 10.0,
                                size: [14.0, 10.0],
                                border: None,
                            }),
                        },
                    ],
                })),
            },
        ],
    }
}
```

**TP decorator group** matches screenshot 1: same `PointLeft` shape, segments =
`[role_glyph, position_count_circle, percent, price]`, no hover buttons. The "2" count
circle is a `BadgeSegment` with `shape_override: Some(BadgeShape::Circle)` and
`fill_override: Some([0, 0, 0, 1])`.

**SL decorator group**: same `PointLeft` shape, segments =
`[role_glyph, risk_amount, price]`, orange fill.

**Bracket status → stroke mapping**: stays on `OrderBracket`. When compute builds
decorators each frame, it resolves `(status, role) → LineStroke` via the reshaped
`leg_style()` helper and stamps it onto `PriceLine.stroke` before calling
`compute_decorator_group()`. This keeps status-driven visual changes (Draft/Pending/
Active opacity + width) working without leaking `BracketStatus` into `PriceLine`.

**Legacy hit zones still emit**: `compute_bracket()` also continues to emit
`HitZoneKind::BracketSubmit` / `Save` / `ToggleSL` / `Cancel` / `CancelSL` hit zones
for the legacy button rects. These are invisible (no fills emitted for them — the
decorators take over the visuals) but remain clickable. This is the single biggest
contract of Slice 8a-ii: the old interaction path still works. Slice 8b tears it
down.

**Testing**:
- `entry_decorator_group_limit_long_has_three_segments`.
- `entry_decorator_group_hover_close_button_is_on_group_hover_only`.
- `entry_decorator_group_tp_sl_stack_is_on_group_hover_only`.
- `tp_decorator_group_position_count_is_circle_segment`.
- `sl_decorator_group_uses_orange_fill`.
- `bracket_status_active_resolves_to_solid_stroke_on_price_line`.
- `bracket_status_draft_resolves_to_dashed_stroke_on_price_line`.
- `compute_bracket_decorator_parity_snapshot` — snapshot of the new
  `WidgetOutput` shape (post-decorator), compared against a checked-in reference.
  Distinct from the 8a-i parity test: 8a-i asserts "identical to pre-refactor",
  8a-ii asserts "identical to the v2 decorator shape," so regressions in either
  direction are caught.
- `compute_bracket_still_emits_legacy_hit_zones` — regression: legacy
  `HitZoneKind::Bracket*` hit zones remain present, so bracket clicks still route
  through the old path during Slice 8a-ii.

**Done when**: Running the app with a Draft bracket shows the exact tag designs from
the screenshots. Hovering the entry line reveals the close + quick-create buttons.
**All existing bracket clicks still work via the legacy path** — Submit, Save, toggle
SL, Cancel, Cancel SL all do what they did before. `cargo test --workspace` passes.
No `HitZoneKind::Bracket*` variant has been deleted yet — that's Slice 8b.
`widget/order_bracket/shim.rs` is deleted.

---

### Slice 8b: Bracket button migration + 5-variant deletion

**Goal**: Switch bracket buttons from the legacy `HitZoneKind::{BracketSubmit,
BracketSave, BracketToggleSL, BracketCancel, BracketCancelSL}` variants to decorator-
emitted `HitZoneKind::Decorator` hit zones. Delete the five legacy variants from
`hit_test.rs`. Remove the match arms in `interaction/mod.rs` and `chart_widget.rs`
that handle them. Verify `ChartAction::DecoratorClick` dispatch from Slice 6 covers
every side effect the old variants produced.

**Depends on**: Slice 8a-ii (decorator emissions are already shipping, just not
clickable yet in a bracket context).

**Size**: M.

**Files to create or modify**:
- `desktop/win/crates/midas-chart/src/widget/order_bracket/decorators.rs` — extend the
  entry/TP/SL decorator constructors so that the action-bearing items have
  `action: Some(DecoratorAction::Submit)` / `Save` / `CloseAnnotation` / etc. as
  appropriate. (Most of this already landed in Slice 8a; Slice 8b fills in any gaps
  and adds the `Submit` button, which was previously a separate legacy hit zone.)
- `desktop/win/crates/midas-chart/src/widget/order_bracket/mod.rs` — remove the legacy
  `HitZone` emission calls from `compute_bracket()`. The decorator emissions are now
  the only bracket hit zones.
- `desktop/win/crates/midas-chart/src/widget/hit_test.rs` — **delete** the five
  legacy variants from `HitZoneKind` at line 46: `BracketSubmit`, `BracketSave`,
  `BracketToggleSL`, `BracketCancel`, `BracketCancelSL`.
- `desktop/win/crates/midas-chart/src/interaction/mod.rs` — remove the now-dead match
  arms for those five variants.
- `desktop/win/crates/midas-app/src/chart_widget.rs` — remove the now-dead match arms
  in the `ChartAction` dispatch loop. Verify every side effect (broker submit, save
  annotation to disk, toggle locked, cancel draft, cancel SL leg) has an equivalent
  `DecoratorAction` landing arm from Slice 6.

**Key implementation details**:

**Side-effect coverage table** — the cutover requires every legacy variant to have a
matching `DecoratorAction` handler. Build this table first, cross-check against Slice
6's app-layer dispatch, and fix gaps in Slice 6 before starting 8b:

| Legacy `HitZoneKind` | New `DecoratorAction` | Side effect |
|---|---|---|
| `BracketSubmit` | `Submit` | Forward bracket to broker bridge |
| `BracketSave` | `Save` | Persist annotation to disk |
| `BracketToggleSL` | `ToggleLocked` or a new variant | Flip SL attached/detached |
| `BracketCancel` | `CloseAnnotation` | Remove bracket from store |
| `BracketCancelSL` | A new variant, e.g. `RemoveStopLoss` | Detach SL leg only |

If `ToggleLocked` and `CloseAnnotation` don't cleanly cover `BracketToggleSL` and
`BracketCancelSL`, add two new `DecoratorAction` variants (`ToggleStopLoss`,
`RemoveStopLoss`) in this slice. That's a one-line addition to the enum in
`widget/decorator/action.rs` plus a new arm in the app dispatch.

**Grep verification** at the end of the slice:

```sh
rg 'HitZoneKind::BracketSubmit|BracketSave|BracketToggleSL|BracketCancel|BracketCancelSL'
# must return zero hits
```

All bracket interactions now route through `ChartAction::DecoratorClick`.

**Testing**:
- `bracket_submit_click_routes_through_decorator_click` — mouse-press on the submit
  button, assert `ChartAction::DecoratorClick { action: DecoratorAction::Submit, .. }`
  is produced.
- `bracket_cancel_click_routes_through_decorator_click`.
- `bracket_cancel_sl_click_routes_through_decorator_click`.
- `bracket_toggle_sl_click_routes_through_decorator_click`.
- `bracket_save_click_routes_through_decorator_click`.
- Regression: `bracket_submit_still_forwards_to_broker_bridge` — end-to-end.
- Regression: `bracket_cancel_still_removes_from_store`.
- Compile-time: `hit_zone_kind_has_no_bracket_variants` — a test that tries to
  construct the deleted variants and would therefore fail to compile if any
  accidentally came back (left as a comment, since you can't test "this doesn't
  compile" from `cargo test`; the rg check above is the real gate).

**Done when**:
- `grep HitZoneKind::BracketSubmit` returns zero hits.
- `grep HitZoneKind::BracketSave` returns zero hits.
- `grep HitZoneKind::BracketToggleSL` returns zero hits.
- `grep HitZoneKind::BracketCancel` returns zero hits.
- `grep HitZoneKind::BracketCancelSL` returns zero hits.
- All bracket interactions route through `ChartAction::DecoratorClick`.
- Every side effect the old variants produced (broker submit, persist to disk, toggle
  SL, cancel draft, cancel SL leg) has a matching `DecoratorAction` arm in
  `chart_widget.rs`.
- Manual click-through of a Draft bracket: create → edit → submit → cancel all still
  work.
- `cargo test --workspace` passes.

---

### Slice 9: Cleanup, documentation, tombstone

**Goal**: Final deprecation pass. Delete dead code, update doc comments, verify zero
clippy warnings, update the docs map in `CLAUDE.md`, archive the plan directory.

**Depends on**: Slice 8b (both migrations landed, all legacy variants gone).

**Size**: S.

**Files to create or modify**:
- `desktop/win/crates/midas-chart/src/widget/level.rs` — at this point should contain
  only `segmented_line()` (still used by `compute_price_line_geometry()`) and the
  `LineStyle` re-export. Everything else has moved. Either leave as-is with a
  tombstone doc comment, or rename to `widget/line_segment.rs` to reflect the reduced
  scope. Implementer's choice — the rename is noise in git log, so leaving it in
  place with a doc comment is preferred unless the reduced file feels misleading.
- `desktop/win/crates/midas-chart/src/levels.rs` — final doc comment sweep.
- `desktop/win/crates/midas-chart/src/widget/hit_test.rs` — final cleanup, verify no
  dead variants remain.
- `desktop/win/crates/midas-chart/src/widget/order_bracket/mod.rs` — remove any
  remaining dead helpers.
- `CLAUDE.md` (root) — update the "Documentation Map" table to point at the archived
  plan location.
- `desktop/win/CLAUDE.md` — same update.
- `plan/decorator-system/` → `plan/archive/decorator-system/` — move the entire plan
  directory once all slices ship. The archive move is the last action in the slice.

**Dead-code grep checklist** (every line below must return zero hits):

```sh
rg 'LineStyle::Dashed'
rg 'LineStyle::Dotted'
rg 'widget::level::HorizontalLevel'
rg '\bLevelExtend\b'                 # moved to LineExtent
rg 'HitZoneKind::BracketSubmit'
rg 'HitZoneKind::BracketSave'
rg 'HitZoneKind::BracketToggleSL'
rg 'HitZoneKind::BracketCancel\b'
rg 'HitZoneKind::BracketCancelSL'
rg 'BracketLeg\s*\{[^}]*\bcolor\s*:'     # BracketLeg.color as top-level field
rg 'BracketLeg\s*\{[^}]*\bline_width\s*:'
rg 'BracketLeg\s*\{[^}]*\bstyle\s*:'
rg 'BracketLeg\s*\{[^}]*\blabel\s*:'
rg 'OrderBracket::leg_style'              # deleted or repurposed
rg 'badge_sdf_spike'                      # leftover from Slice 0 if not cleaned up
```

**Clippy pass**: `cargo clippy --workspace -- -D warnings` clean. Consider
temporarily enabling `-W dead_code` to catch stragglers.

**CLAUDE.md doc map entry** — add a row pointing to the archived plan:

```
| Decorator system | plan/archive/decorator-system/00-index.md |
```

**Testing**:
- `cargo test --workspace` — all passing.
- `cargo clippy --workspace -- -D warnings` — clean.
- `cargo clippy --workspace -- -W dead_code -D warnings` — clean (temporary check).
- Visual regression manual check: level rendering, bracket rendering, hover buttons,
  dash patterns, all match the designs.
- All grep checks from the checklist above return zero hits.

**Done when**:
- The deprecated-code grep list is empty.
- `cargo test --workspace` and `cargo clippy --workspace -- -D warnings` both green.
- The plan directory is at `plan/archive/decorator-system/`.
- Both `CLAUDE.md` files link to the archived plan.

---

## Execution order and parallelism

Concrete parallelism recommendation. Slice 0 and Slice 1 run concurrently on day 0
(different files, zero conflict risk). Slice 2.5 retires the hover-recompute cost
assumption before Slice 5 commits to it. Slice 8a is split into 8a-i (data-model) and
8a-ii (visual) so the PRs stay reviewable.

| When (days) | Engineer A | Engineer B | Notes |
|---|---|---|---|
| 0–1     | **Slice 0** (SDF shader spike) | **Slice 1** (`LineStyle::Pattern`) | Parallel window 0. Zero file overlap: Slice 0 lives in `midas-render/examples/`, Slice 1 in `midas-chart/src/widget/level.rs` + `order_bracket/mod.rs`. |
| 1–3     | **Slice 2** (`PriceLine` + types + `BadgeInstance`) | idle / test-fixture prep | B prepares the Slice 2.5 benchmark harness and the Slice 7 fixture file. ~1.5d unused capacity. |
| 3–3.5   | **Slice 2.5** (hover-recompute benchmark) | continued prep | 0.5d. A or B can own it. Outcome gates Slice 5's default. |
| 3.5–7.5 | **Slice 3** (compute engine, `midas-chart`) | **Slice 4** (SDF pipeline, `midas-render`) | Parallel window 1. Different crates, minimal merge-conflict risk. Slice 4 owns the `compute_decorator_group()` emission flip at the end. |
| 7.5–9.5 | **Slice 5** (hover two-pass + recompute) | Slice 7 test-fixture prep | ~2d of B capacity. B checks in `config_v1_pre_decorator.toml` and `annotations_v1_pre_decorator.json`. |
| 9.5–10.5 | **Slice 6** (`DecoratorAction` routing) | **Slice 7** (`HorizontalLevel` migration, days 9.5–13.5) | Parallel window 2. Slice 6 is S, Slice 7 is L. A finishes 6 at day 10.5; at that point A can review Slice 7's PR, author Slice 6's one-line follow-up for the level price badge, and prep Slice 8a-i test fixtures. |
| 10.5–13.5 | Review / 8a-i prep | **Slice 7** (continues) | A can't start 8a-i yet (hard dep on Slice 7). Useful-but-not-utilized work: write the 8a-i snapshot test harness, draft the `shim.rs` skeleton. ~3d. |
| 13.5–15.5 | **Slice 8a-i** (`BracketLeg` data-model + shim) | 8a-ii / 8b test-fixture prep | M. A owns the data-model rewrite; 8a-ii visuals come right after. |
| 15.5–17.5 | **Slice 8a-ii** (visual decorator emissions) | idle / cross-review | M. Screenshot-payoff slice. |
| 17.5–19.5 | **Slice 8b** (button migration + 5-variant deletion) | idle | M. Interactive cutover. |
| 19.5–20.5 | **Slice 9** (cleanup, archive) | idle | S. |

**Critical path length** (sequential chain): Slice 0 → 2 → 2.5 → (3 ‖ 4) → 5 → 7 →
8a-i → 8a-ii → 8b → 9. That's 10 logical steps on the critical path, with the parallel
window 3 ‖ 4 collapsing into one step. **~20.5 elapsed days** assuming S=1d, M=2d,
L=4d.

**Engineer B utilization**: Slice 1 (1d) + Slice 4 (4d) + Slice 7 (4d) + prep/fixture
work (~3d) = **~12d utilized out of 20.5d elapsed**, or ~8.5d unused capacity spread
across three windows. If Engineer B has another project to rotate to during the unused
windows, the effective cost is closer to 25 engineer-days across both people. If B is
dedicated to this plan only, expect ~8.5 days of slack time that can absorb review,
fixture work, and schedule jitter — not 8 days of pure idle.

Compared to a strict single-engineer serial execution (total ≈ 25.5 engineer-days, no
parallelism), the two-engineer schedule saves ~5 calendar days by overlapping Slice 4
with Slice 3 and Slice 7 with Slice 6, at the cost of 8.5 days of partial utilization.
Two-engineer staffing is worth it only if the 5-day calendar saving matters more than
the utilization hit, OR if Engineer B has a rotating backlog to fill the unused
capacity.

**Reviewable checkpoints** — the user-visible moments that warrant a human review
even if tests are green:

1. After **Slice 1**: dash patterns render correctly.
2. After **Slice 2.5**: hover-recompute cost is measured; Decision 7's default is
   either confirmed or the fallback is promoted. Documented as a one-paragraph note at
   the top of Slice 5.
3. After **Slice 4**: the decorator showcase annotation shows all eight shapes with
   real SDF rendering.
4. After **Slice 5**: hover-reveal works on the showcase annotation.
5. After **Slice 7**: levels render visually identical to pre-refactor, v1 configs
   load.
6. After **Slice 8a-i**: bracket rendering is pixel-identical to pre-refactor via the
   new data model (snapshot parity test green). No visual diff.
7. After **Slice 8a-ii**: bracket tags match the screenshots, legacy clicks still
   work.
8. After **Slice 8b**: bracket interactions all routed through decorators, no legacy
   hit-zone variants remain.

Each of these is a natural PR boundary.
