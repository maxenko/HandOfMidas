# Design Decisions

These are the consequential choices made in this plan. See [00-index.md](00-index.md) for the overall plan and [03-data-model.md](03-data-model.md) for the resulting type surface.

Each decision records the problem, the alternatives considered, the chosen path, and a confidence rating. Type definitions and slice-level wiring live in the sibling files — this document is the "why," not the "what."

Where a decision directly constrains a sibling file (for example, Decision 5 constraining the rendering pipeline, or Decision 8 constraining the hit-test data model), the sibling file is cross-linked inline. Reading this document top-to-bottom should give a reviewer enough context to understand every non-obvious choice; the sibling files exist to answer "show me the code."

The eight decisions fall into three clusters:

- **Geometry and styling** (Decisions 1–2): `LineStyle::Pattern` and `PriceLine` collapse the duplicated line-drawing vocabulary and establish a single primitive for every decorated horizontal line.
- **Decorator model and rendering** (Decisions 3–5): `DecoratorGroup` as a flex container, `Badge` as a multi-segment composite with optional per-segment shapes, and an SDF pipeline that rasterizes every non-rectangular shape the catalogue needs.
- **Interaction, state, and hit testing** (Decisions 6–8): a sans-IO `DecoratorAction` enum routed through `ChartAction`, cross-frame hover persistence that recomputes hit zones in `update()` each mouse event, and a generalized `HitZoneKind::Decorator` variant that replaces five bracket-specific button variants without giving up the `Copy` derive on `HitZoneKind`.

One of the eight decisions is marked medium confidence: Decision 7 (hover persistence), because the frame-ordering invariant still needs a dedicated regression test. The remaining seven are high confidence — the alternatives were considered and the chosen path is the obvious one once the constraints are laid out.

---

### Decision: `LineStyle::Pattern(SmallVec<[f32; 6]>)` replaces `Dashed`/`Dotted`

**Context**: The current `LineStyle` enum at `widget/level.rs:42` has fixed 2-phase variants (`Dashed { dash_len, gap_len }`, `Dotted { dot_spacing }`). The user explicitly asked for SVG-style patterns such as `[1, 2]` so that sparse dots and custom rhythms (dash-dot, dash-dot-dot) can be expressed without adding new enum variants. The decorator system is the first consumer that needs the richer vocabulary, and several existing call sites in `order_bracket::leg_style()` at `order_bracket/mod.rs:207` will also benefit.

**Options**:

1. Keep the existing variants and add more (`DashDot`, `DashDotDot`, `SparseDotted`). Easy today, but doesn't scale — the user will ask for another shape next month, and each new variant adds a branch in the shader and the pattern-matching sites.
2. Add a `LineStyle::Custom { dashes: Vec<f32> }` alongside the existing variants. Produces two ways to say the same thing and every consumer has to handle both paths. Discoverability suffers: new contributors won't know whether `Dashed` or `Custom` is the canonical form.
3. Replace all existing run-length variants with a single `Pattern` variant that covers every case through a shared cyclically-walked dash list. One code path in the shader, one serialization format, one preset module.

**Recommendation**: Option 3.

`LineStyle` becomes essentially `Solid | Pattern(SmallVec<[f32; 6]>)`, where the `SmallVec` holds alternating on/off run lengths in logical pixels, walked cyclically (SVG convention).

- `[1, 3]` is a dot pattern.
- `[6, 3, 1, 3]` is dash-dot.
- `[1, 2]` — the example from the user ask — is a 1px-on, 2px-off sparse dotted rhythm.

Preset constructors keep call sites readable and encode the canonical recipes once:

- `LineStyle::dotted()` → `[1.0, 3.0]`
- `LineStyle::sparse_dotted()` → `[1.0, 6.0]`
- `LineStyle::dashed()` → `[6.0, 3.0]`
- `LineStyle::dashed_long()` → `[10.0, 4.0]`
- `LineStyle::dash_dot()` → `[6.0, 3.0, 1.0, 3.0]`
- `LineStyle::dash_dot_dot()` → `[6.0, 3.0, 1.0, 3.0, 1.0, 3.0]`

`SmallVec<[f32; 6]>` keeps every preset heap-free (the longest is six entries) while still permitting arbitrary-length user-defined patterns. The SDF line pipeline walks the pattern modulo its total length when rasterizing each segment — see [04-rendering.md](04-rendering.md) for the shader details.

Migration story: every existing `LineStyle::Dashed { dash_len, gap_len }` call site rewrites to `LineStyle::Pattern(smallvec![dash_len, gap_len])`, and every `LineStyle::Dotted { dot_spacing }` rewrites to `LineStyle::Pattern(smallvec![1.0, dot_spacing])`.

The `leg_style()` function at `order_bracket/mod.rs:207` collapses from two match arms to preset constructor calls (`LineStyle::dashed()`, `LineStyle::dotted()`) without any behavior change. Slice 1 of [06-implementation.md](06-implementation.md) does this rewrite mechanically before any decorator code is added, so the decorator rollout starts from a clean styling baseline.

The alternative of keeping a backwards-compatible two-phase variant alongside `Pattern` was considered briefly and rejected: it leaves dead code in the shader (two dash-evaluation paths), two serialization formats to version, and the ambiguity of "which representation should new code use?" The rewrite is small enough that a hard cutover is simpler than a transition period.

**Confidence**: high

---

### Decision: Introduce `PriceLine` as the geometric primitive

**Context**: The decorator system needs a single concept to hang off of — "a horizontal line at a specific price, with a stroke and an extent." Three near-duplicate structs carry this concept today: `HorizontalLevel` in `widget/level.rs`, the fields inside `BracketLeg`, and an orphan `widget::level::HorizontalLevel`. Each re-invents price + extent + stroke in slightly different shapes. The decorator system is the forcing function to collapse them, because every decorated line type needs to speak the same layout vocabulary.

**Options**:

1. Keep `HorizontalLevel` and `BracketLeg` as-is and have each own a `Vec<DecoratorGroup>` independently. Preserves the duplication forever and forces `compute_decorator_group()` to special-case two container types — every change has to be made in two places.
2. Introduce a neutral `PriceLine { price, extent, stroke }` primitive and have both domain types compose it. Shared layout code operates on `PriceLine` regardless of whether it came from a level or a bracket leg.
3. Promote `HorizontalLevel` itself to the primitive and have `BracketLeg` embed a `HorizontalLevel`. Overloads an already-meaningful domain name with a more generic one, confuses persisted data semantics: a `HorizontalLevel` in storage would suddenly mean two different things depending on context.

**Recommendation**: Option 2.

The new primitive lives at `midas-chart/src/widget/price_line.rs` and owns three fields — `price: f64`, `extent: LineExtent`, `stroke: LineStroke`.

`LineStroke` in turn wraps color, width, and the `LineStyle` from the previous decision. `LineExtent` becomes an enum (`FullWidth`, `RightFrom { timestamp }`, `Between { start, end }`) so that brackets with a time origin and levels with a full-width reach share one model.

Domain types become thin composition wrappers: `HorizontalLevel` retains `id`, `locked`, and `decorators` but delegates all geometry to an inner `line: PriceLine`; `BracketLeg` likewise holds `line: PriceLine`, `role`, `decorators`, and projected PnL fields. The second orphan `widget::level::HorizontalLevel` disappears entirely — its responsibilities are absorbed into `PriceLine` plus decorators. Full type definitions are in [03-data-model.md](03-data-model.md).

Note that `Annotation.locked` continues to live on the wrapper at `widget/mod.rs:146`, not on `HorizontalLevel` or `BracketLeg`. `PriceLine` knows nothing about lock state — lock is a user-intent bit that belongs to the annotation, not to the geometry. The compute path reads `annotation.locked` and adjusts the hit-zone cursor accordingly; `PriceLine` itself is pure geometry and stroke. The locked bit is passed into `compute_level()` as an explicit argument from the `Annotation` wrapper at the call site in `compute_widget_annotations()`.

**Confidence**: high

---

### Decision: `DecoratorGroup` is a flex container, not a single badge

**Context**: The user's screenshots show that a single logical "tag" on a price line is in fact *several* visual elements: a dedicated `[X]` close button, a main multi-segment badge with a triangular left point, and — on some legs — a vertically stacked pair of `▲`/`▼` quick-action buttons on the far right. A single `Badge` struct cannot express this composition; packaging the close button and stack into the badge would hard-code a layout and make hover reveal impossible to target per element.

**Options**:

1. Single `Badge` with a hard-coded "close button on left, stack on right" layout. Inflexible — any new shape breaks the schema and every consumer has to know the layout by convention. Hover reveal on an individual close button becomes impossible to target because the button isn't a first-class entity.
2. Flex container with a direction axis, a gap, and a list of child items. Covers everything in the screenshots (Row for the whole tag, nested Column for the stack) with a single concept. Each item is independently hit-testable, hover-gateable, and actionable.
3. Full constraint / grid layout engine. Massive over-engineering for groups that max out at six items and never need cross-axis alignment. Would introduce a solver where a one-pass loop does the job.

**Recommendation**: Option 2.

A `DecoratorGroup` is a flex-laid container anchored to a point on its parent `PriceLine`, with four pieces of data:

- **Direction** (`FlexDirection::Row | Column`) — controls the main axis along which items are laid out.
- **Gap** — logical pixels between siblings along the main axis.
- **Items** — an ordered list of `DecoratorItem`s, each of which wraps a `Badge`, a `Button`, a nested `DecoratorGroup` (the stack case), or a `Spacer`.
- **Anchor** (`DecoratorAnchor::LeftEdge | RightEdge | AtTimestamp(i64) | AtScreenX(f32)`) — where on the parent line the group pins itself before flex layout runs.

This is a minimum viable flex model: items have intrinsic sizes, layout is one pass along the `direction` axis with `gap` between siblings, no shrink, no grow, no align-self. If decorators ever need cross-axis alignment we add it incrementally. The nested-group case (the `▲`/`▼` stack) is just a `Column` group sitting inside a `Row` group — no special handling, the layout recursion is already correct.

Every item also carries a `visibility: Visibility` and an optional `action: DecoratorAction`. `Visibility::Always` is for permanent decorations such as the main TP price badge; `Visibility::OnHover` is for the close button and the quick-action stack that should appear only when the user's pointer is near the line. Hover-gated visibility is what makes Decision 7 necessary — see that decision for how the gating flag is kept alive across cursor movements onto newly-revealed items.

Each `DecoratorGroup` carries a stable `group_id: u16` that is unique within its parent annotation. The id is the hover-persistence key in Decision 7 and the hit-zone identifier in Decision 8; it is assigned at construction time and never mutated, so serialization and persistence are trivial.

See [05-interaction.md](05-interaction.md) for the single-pass layout algorithm, the anchor-resolution rules, and the hit-zone emission that pairs with it.

**Confidence**: high

---

### Decision: `Badge` carries multiple `BadgeSegment`s with optional per-segment shape

**Context**: The take-profit tag screenshot shows one visual badge with several colored compartments — green "T", black circle around "2", green "100%", darker-green "46.40" — all sharing one outer pointed-left outline but with independent backgrounds. A `Badge` with a single text field would force consumers to stitch multiple Badges together per tag, which breaks the unified outline, complicates hit testing, and scatters style state across sibling items that the user perceives as one thing.

**Options**:

1. Treat each compartment as a separate `Badge` inside the flex row. Simple, but loses the unified outer outline — the triangular left point only applies to the leftmost badge and the siblings look visually disjoint. Also forces hit testing to stitch neighbouring badges together when the user hovers the "edge" between two compartments, producing fiddly boundary cases.
2. `Badge` owns a `Vec<BadgeSegment>` and renders one outer outline (with optional dividers between segments). Text, color, and optional per-segment shape are stored per segment, so a circle-within-a-pill is expressible without special cases.

**Recommendation**: Option 2.

A `Badge` carries a shared outer shape, fill, border, height, and padding; plus a `segments: SmallVec<[BadgeSegment; 3]>` list. Each `BadgeSegment` carries its own text, text color, font size, optional minimum width (for column alignment across multiple rows), and — crucially — an optional `fill_override` and `shape_override` so a segment can paint its own background inside the parent outline.

The "black circle around 2" in the TP screenshot is exactly one segment with `shape_override: Some(BadgeShape::Circle)` and `fill_override: Some([0, 0, 0, 1])`; no dedicated code path, no special case in the renderer, no hit-test quirk.

Each segment may also carry its own `DecoratorAction`, enabling per-segment click handling (click "P" to cycle entry type, click the quantity segment to edit). The segment list is rendered in main-axis order, and hit testing walks it linearly — see [05-interaction.md](05-interaction.md) for the full segment geometry and hit-testing rules. The concrete field list and the `BadgeShape` enum are in [03-data-model.md](03-data-model.md).

The key insight here is that per-segment `shape_override` and `fill_override` let a badge contain a *chart* of mini-badges without any new infrastructure. A circle drawn over a pill, a nested rounded rect inside a pointed-left tag, a monochrome indicator dot inside an otherwise colored status pill — all of these are one `BadgeSegment` with two optional fields set. The SDF pipeline in Decision 5 treats each segment as its own sub-instance, so the rendering cost is proportional to total segment count, not segment nesting depth.

Per-segment `min_width` exists for one specific reason: aligning quantity columns across multiple stacked legs of a bracket (entry / TP / SL badges should have their quantity segment line up visually). Without a minimum width, proportionally-sized text in the quantity segment produces ragged left edges on the subsequent segments.

**Confidence**: high

---

### Decision: SDF GPU pipeline over geometry decomposition

**Context**: The decorator system introduces a family of non-axis-aligned shapes: `PointLeft`, `PointRight`, `DoublePoint`, `Chevron`, `Pill`, `Rounded`, and `Circle`. The current GPU path draws only axis-aligned quads via `GridLineInstance` in `instances.rs:76`; there is no existing pipeline that can rasterize a rounded rectangle, let alone a pointed-left badge, without aliasing artefacts on diagonal edges.

**Options**:

1. **Geometry decomposition**: break each shape into axis-aligned rectangles and triangles on the CPU, build a `TriangleInstance` pipeline. Works for `PointLeft` (one rect plus one triangle), but produces ugly results for `Rounded`, `Pill`, and `Circle` — they would need many small tessellated rectangles, and the diagonal seams alias badly without an MSAA target. Every new shape means new decomposition code on the CPU side.
2. **SDF pipeline**: one instance per badge, a single fragment shader evaluates a signed distance field per fragment based on a `shape_id` and a `shape_param`, with analytical antialiasing via `fwidth()`. Clean edges, scales to any 2D shape that has a known SDF (rect, rounded rect, pill, circle, triangle point), and every badge in the frame draws in one instanced call. New shapes are additions to one `switch` block in the fragment shader.
3. **Textured quads with pre-baked shape atlases**: works but is restrictive — fixed shape sizes force mipmap chains, aspect ratios get distorted when the `point_width` is small relative to the total rect, and adding a new shape means re-baking the atlas. Text rendering is a separate atlas entirely, so the badge shape would need yet another layer.

**Recommendation**: Option 2.

SDFs are the correct long-term answer and unlock every shape for the cost of writing the pipeline once. For context, Inigo Quilez's SDF reference ([iquilezles.org/articles/distfunctions2d](https://iquilezles.org/articles/distfunctions2d/)) lists analytical distance functions for every 2D primitive the decorator catalogue needs — rectangle, rounded rectangle, pill, circle, triangle — so the shader is assembled from textbook components, not invented.

Concretely: a new `BadgeInstance` type in `midas-chart/src/instances.rs` (sans-IO boundary, zero GPU deps — it's `Pod`/`Zeroable` but not tied to wgpu), declared in Slice 2 alongside the other decorator types.

`BadgeInstance` carries the screen-space rect, fill and border colors, a `shape_id` enum discriminant as `u32`, a `shape_param` (radius for `Rounded`, point_width for `PointLeft`, etc.), a border thickness, and padding to the 16-byte vertex-attribute alignment wgpu requires.

A new `BadgePipeline` in `midas-render/src/pipelines/badge.rs` (Slice 4) plus a `badge.wgsl` shader owns the rasterization. The fragment shader is roughly forty lines of SDF math, with border rendering expressed as `sdf < 0 ? fill : (sdf < thickness ? border : transparent)` — one conditional on a scalar distance, antialiased via `fwidth()` at the boundary.

Draw order is preserved: grid → volume → volume_profile → candle_wicks → candle_bodies → **badges** → crosshair. Badges insert after candle bodies and before the crosshair so decorators occlude price bars but remain under the hover cursor. The two `ChartScene` types — the IR in `midas-chart/src/scene.rs:20` and the borrowed render-side struct in `midas-render/src/renderer.rs:20` — both gain a `badges` field in Slice 2 / Slice 4 respectively. `ChartRenderer::new()` at `renderer.rs:51` gains the `BadgePipeline` construction call.

A note on the sans-IO boundary: `BadgeInstance` itself is declared in `midas-chart/src/instances.rs`, which has zero GPU dependencies. It's a `bytemuck::Pod`-and-`Zeroable` struct, nothing more. The `midas-render` crate consumes that same struct in its `BadgePipeline` without any translation layer — the CPU-side compute path builds `Vec<BadgeInstance>` in the sans-IO crate, and the render crate hands the slice straight to `wgpu::Queue::write_buffer`. This pattern matches how `GridLineInstance`, `CandleInstance`, and the existing per-primitive instance types already work, so it does not invent a new architectural convention; it extends an existing one.

The alternative of keeping the GPU struct private to `midas-render` and defining a mirror sans-IO type in `midas-chart` was rejected: it doubles the maintenance surface and introduces a translation step that has historically leaked alignment bugs in other crates. One struct, two crates, zero translation.

See [04-rendering.md](04-rendering.md) for the SDF shader walkthrough and the draw-order verification tests.

**Confidence**: high

---

### Decision: Sans-IO action routing via `DecoratorAction` + `ChartAction::DecoratorClick`

**Context**: Decorator buttons need to trigger side effects — close an annotation, attach a take-profit, submit an order bracket, cycle an entry type. But `midas-chart` is a sans-IO crate by architecture: it cannot dispatch broker commands, and its public types must remain serializable so annotations can be persisted and round-tripped. Any routing scheme that stores closures or reaches into the app layer violates the boundary.

**Options**:

1. Store callbacks (`Box<dyn Fn(...)>`) on decorators. Breaks serialization, threads lifetimes through sans-IO types, and couples chart internals to app state. A `DecoratorGroup` could no longer be stored, cloned, or sent across threads without surgery.
2. Emit action variants as data via an enum (`DecoratorAction`) that is surfaced through a new `ChartAction::DecoratorClick` variant. The app layer matches on the action and maps to broker commands or UI events. Data-only, exhaustive at compile time, matches the existing `ChartAction` pattern.
3. String-ID actions with a runtime lookup table in the app layer. Loses compile-time exhaustiveness — a typo in an action ID is a runtime no-op, and the compiler can't tell you which IDs are still in use after a refactor.

**Recommendation**: Option 2.

A `DecoratorAction` enum lives at `midas-chart/src/widget/decorator/action.rs` and covers the fixed vocabulary:

- `CloseAnnotation` — delete the parent annotation.
- `CreateTakeProfit` / `CreateStopLoss` — attach a new bracket leg.
- `CycleEntryType` — step through `Limit` / `Stop` / `Market` for an entry.
- `EditQuantity` / `EditPrice` — open an inline text editor.
- `ToggleLocked` — flip `Annotation.locked` at `widget/mod.rs:146`.
- `Submit` / `Save` — transmit a draft bracket or persist a level.
- `Custom(u32)` — the escape hatch for app-defined actions (see below).

A new `ChartAction::DecoratorClick { annotation_id, group_id, action }` variant is added to `interaction/mod.rs:61`. The app layer matches on this in `chart_widget.rs` and maps each `(AnnotationKind, DecoratorAction)` pair to the appropriate broker command or UI message — for instance, `(AnnotationKind::OrderBracket, DecoratorAction::CreateStopLoss)` becomes `BrokerMessage::AttachStopLoss { bracket_id, price }`. The mapping is explicit, per-annotation-kind, and lives entirely outside the chart crate. `DecoratorAction` derives `Copy`, which matters for the next two decisions.

The `Custom(u32)` escape hatch is intentional: it exists so that app-layer experiments and future indicator-specific actions can be routed through the same pipeline without first negotiating an enum variant into `midas-chart`. The `u32` payload is a namespace the app layer owns; collisions are its problem, not the chart crate's. When a custom action stabilizes into something worth reusing, it graduates into a named variant in a follow-up change.

One property worth noting: because `DecoratorAction` is both `Copy` and serializable, the click path does not need any heap allocation. A click walks from mouse event → hit zone lookup → `DecoratorClick` emission → app-layer match, passing the action enum by value the whole way. This is what keeps Decision 8's `HitZoneKind` migration clean.

See [05-interaction.md](05-interaction.md) for the full action → command mapping table and the click-routing walkthrough.

**Confidence**: high

---

### Decision: Hover persistence via `hovered_decorator_groups` — recompute-in-`update()` is the default

**Context**: When a user hovers a price line, the `OnHover` decorator items become visible (close button, quick-action stack). If the user then moves the cursor off the line itself but *onto* one of the newly-revealed buttons, those buttons must stay alive long enough to be clicked. Without persistence they disappear the moment the pointer leaves the line's narrow hit zone — a frame before the user's cursor reaches the button, making the buttons effectively unclickable. The naive "cursor is over the line" rule isn't sufficient.

A second, subtler constraint: iced 0.14 calls `canvas::Program::draw()` not just for mouse events but also for theme changes, viewport resizes, animation frames, and window redraws. Any scheme that relies on `draw()`-time invariants being preserved across frames must tolerate `draw()` being called for reasons that have nothing to do with input state.

**Options**:

1. Buttons are always visible (never reveal on hover). Rejected — the user explicitly wants the chart to stay uncluttered until a line is hovered.
2. Fatten the parent line's hit zone to also cover the decorator area. Leaks decorator layout into the parent line's hit test, brittle when decorator count changes, and means the line drag handle overlaps button click targets.
3. Persist hover state across frames via a field on `ChartState`, keyed by `(annotation_id, group_id)`. On every mouse-move event in `update()`, **recompute the decorator hit zones from scratch** using a minimal camera-transform + layout pass, then test the cursor against both the parent line and every visible decorator item. A group stays in the persisted set as long as the cursor is over its line or any of its items.
4. Persist hover state plus cache last-frame hit zones in a `RefCell<Vec<HitZone>>` populated during `draw()`. `update()` reads the cache instead of recomputing.

**Recommendation**: Option 3 is the default.

A new field `hovered_decorator_groups: SmallVec<[(AnnotationId, u16); 2]>` goes on `ChartState` at `state/mod.rs:126`. Note that `ChartState` currently has only `hovered_annotation`; `selected_annotation` and `drag_ghost` live on `ChartInput` and `ComputeContext` respectively, not on `ChartState`. The new field joins `hovered_annotation` as the second piece of hover-state state owned by `ChartState` — both are driven from `update()` and read by `draw()` via the normal snapshot path.

On every mouse-move event, `chart_widget.rs::update()` recomputes the visible decorator items for every currently-persisted or newly-entered group — one camera-transform pass plus one flex-layout pass per group — and updates the set. The recomputed group list is passed through `ChartInput` → `ComputeContext` → `compute_decorator_group()`, which reads it to decide which `OnHover` items to emit during the next `draw()`.

**Option 4 (`RefCell` cache) is a documented fallback only**, used only if a measurement shows that recomputing hit zones on every mouse move is too expensive.

The reason Option 4 is *not* the default is the iced lifecycle. Because `canvas::Program::draw()` fires on theme changes, viewport resizes, and animation frames — not just on input events — a `RefCell`-based cache cannot enforce its transition invariants across those extraneous draws. The cache may be stale on the `update()` that follows a non-mouse `draw()`, and there is no single place in the iced program lifecycle to reliably invalidate it.

Recomputing in `update()` sidesteps the entire class of frame-ordering bugs: there is no cache, so there is nothing to invalidate, and the hit zones the interaction layer tests against are always derived from the same camera and layout that the *next* draw will use.

For the scale that decorators actually run at — up to roughly six items per group, with a handful of groups visible at any time — the recompute cost is one camera transform (a pair of `f64` multiplications) plus one linear flex pass. The budget is expected to be comfortably below the mouse-move frame budget, but the assumption is retired early: Slice 2.5 of [06-implementation.md](06-implementation.md) runs a standalone synthetic benchmark (2400 items across a worst-case chart grid) before any interaction code commits to the default, and Slice 5 adds a live benchmark gate.

Mental model: `ChartState::hovered_decorator_groups` is the set of groups whose `OnHover` items are currently visible.

On every mouse-move event, the app layer walks that set, recomputes each visible group's hit zones from the current camera and layout, and answers the question "is the cursor over the parent line *or* over any item?" — a group is retained if yes, dropped if no. Newly-entered lines are added to the set when their line hit zone is first crossed.

The set is `SmallVec<[(AnnotationId, u16); 2]>` because two simultaneously-hovered groups is the realistic ceiling (entry plus one leg during bracket editing); larger counts spill to heap without incident.

Why not fatten the line hit zone instead (Option 2)? Because the decorator layout can change per frame — quantity edits, price drags, and PnL updates all mutate badge content, which mutates widths. A fattened line hit zone would need to track those width changes to avoid falsely claiming the cursor "is still on the line" when it has actually moved off both the line and the now-narrower decorators.

The persistence approach is strictly simpler: it hit-tests what is actually visible, not an approximation of what might be visible. And because the hit test runs against freshly-recomputed zones on every mouse move, there is no drift between what the renderer drew and what the interaction layer thinks is clickable.

**Confidence**: medium. The data flow is clean, but the frame-ordering invariant — specifically, that `update()` always runs between two consecutive `draw()` calls that matter for hover state — still needs a dedicated regression test. That test is listed in [07-risks-testing.md](07-risks-testing.md).

---

### Decision: Generalize the 5 bracket-button `HitZoneKind` variants into `HitZoneKind::Decorator`

**Context**: `HitZoneKind` at `desktop/win/crates/midas-chart/src/widget/hit_test.rs:46` — declared with `#[derive(Clone, Copy, Debug, PartialEq, Eq)]` — already contains five purpose-specific button variants: `BracketSubmit`, `BracketSave`, `BracketToggleSL`, `BracketCancel`, `BracketCancelSL`. The decorator system creates a general mechanism for exactly this concept (a button hit zone that carries an action), so the bracket-specific variants become legacy noise the moment decorators land. They should be replaced.

The replacement variant must carry enough information to route a click: which decorator group was hit, which item within the group (including nested items for the stack-in-row case), and which `DecoratorAction` the click maps to. The obvious data structure for "nested item path" is `SmallVec<[u8; 4]>` — a small inline vector of child indices. **That is the wrong answer here**, because `SmallVec` is not `Copy`, and dropping `Copy` from `HitZoneKind` would cascade through the entire hit-test and hover infrastructure.

**Options**:

1. Keep the existing bracket variants and add `HitZoneKind::Decorator` alongside. Duplication forever; two code paths that mean the same thing. Every future button type gets a choice: new hit-zone variant or decorator? The choice is fake, but it's a thing consumers would have to reason about.
2. Replace the five bracket-button variants with a single `HitZoneKind::Decorator` variant that carries `group_id`, an item path, and a `DecoratorAction`. Migration happens in the slice plan: decorators replace the bracket buttons first, then the old variants are deleted.

**Recommendation**: Option 2.

Migrate the bracket buttons to decorators in the decorator rollout slice, then delete the five `Bracket*` button variants in the cleanup slice.

The critical implementation constraint is that `HitZoneKind` must preserve `#[derive(Clone, Copy, Debug, PartialEq, Eq)]`, which means the item path cannot be a `SmallVec`. Instead, use a fixed-size `[u8; 4]` plus a `u8` length prefix:

```rust
pub enum HitZoneKind {
    LevelLine,
    BracketEntry, BracketTP, BracketSL, BracketStopTrigger, BracketZone,
    MarkerIcon, NoteBody, VolumeProfileBar,
    /// Click on a decorator item. `item_path` is a fixed breadcrumb with
    /// a `len` prefix: [2, 0, 0, 0] with len=2 means group item 2 -> stack child 0.
    /// Using a `[u8; 4] + u8` (5 bytes) instead of `SmallVec` keeps the enum Copy.
    /// Four levels of nesting is well beyond anything the decorator model uses.
    Decorator {
        group_id: u16,
        item_path: [u8; 4],
        item_path_len: u8,
        action: DecoratorAction,
    },
}
```

The reason `SmallVec<[u8; 4]>` is rejected even though it would be ergonomic: `SmallVec` is not `Copy`, so adopting it would force `HitZoneKind` to drop its `Copy` derive.

That cascade is large and everywhere:

- `HitZone` itself would lose `Copy` (it embeds `HitZoneKind`).
- Every `(AnnotationId, HitZoneKind)` pair stored in `ComputeContext::hovered_annotation`, `ChartInput::hovered_annotation`, and `ChartState::hovered_annotation` would need `.clone()` instead of by-value copies.
- Every `match hit.kind { ... }` site — of which there are many, in compute, hit-test, drag, and draw code — would need either `.clone()` or explicit borrow patterns.
- The bracket interaction state machine currently pattern-matches on `HitZoneKind` by value in several places; each of those would gain either a clone or a borrow, and the resulting diff would dwarf the decorator feature itself.

For four levels of nesting — already far beyond anything the decorator model uses in practice — a fixed `[u8; 4] + u8` is five bytes, stays in the `Copy` club, and requires zero churn on the rest of the hit-test surface.

The drawback is the compile-time ceiling on nesting depth, which is the correct trade. If a future decorator model truly needs five-plus levels, the fix is to bump the array size, not to reintroduce a heap-backed path.

The remaining non-button variants — `LevelLine`, `BracketEntry`, `BracketTP`, `BracketSL`, `BracketStopTrigger`, `BracketZone` — stay as-is. They represent **line hits for drag**, not button clicks, and the decorator system is orthogonal to line dragging. Keeping them separate from `HitZoneKind::Decorator` also means the drag-interaction state machine in `chart_widget.rs` does not have to filter decorator hits out of its line-hit search: dragging code pattern-matches on `LevelLine | Bracket{Entry,TP,SL,StopTrigger,Zone}` exactly as today.

The `item_path_len` field is the one piece of cleverness in the replacement variant. It exists because a zero-initialized `[u8; 4]` is indistinguishable from "one level of nesting, child index 0" — the zero byte is a valid child index at root. A separate `len` prefix disambiguates: `item_path = [2, 0, 0, 0], item_path_len = 1` means "group item 2," while `item_path = [2, 0, 0, 0], item_path_len = 2` means "group item 2 → stack child 0." For derived `PartialEq`/`Hash` to be sound, two paths with equal `item_path_len` but different trailing garbage in `item_path` must never compare equal by accident — so all construction goes through the private `ItemPath` newtype defined in [03-data-model.md](03-data-model.md), which asserts `len <= 4` and zeroes the unused tail. Pattern-match sites read the slice via `&item_path[..item_path_len as usize]`.

**Alternatives considered for the payload type**:

- **`SmallVec<[u8; 4]>`**: the most ergonomic shape but the one that breaks `Copy` (see cascade above). Rejected.
- **`arrayvec::ArrayVec<u8, 4>` / `tinyvec::ArrayVec<[u8; 4]>`**: both crates provide fixed-capacity inline vectors that *are* `Copy` when the element type is `Copy` (via `impl<T: Copy, const CAP: usize> Copy for ArrayVec<T, CAP>`). They handle the length byte, iteration, serde, and `Debug` uniformly. ICU4X's `tinystr` is a precedent for exactly this pattern. They are the idiomatic ecosystem answer to "`Copy`-preserving short variable-length payload" — the hand-rolled `[u8; 4] + u8` we ship is structurally identical to what these crates give you, just hand-written. The trade we accept: no new workspace dependency, one local `ItemPath` newtype instead. If either crate enters the workspace for another reason, the decorator system should graduate to it.
- **Bit-packed `u32`** (4 nibbles): more compact but opaque in `{:?}` output and hostile to interactive debugging. Rejected.
- **Interned path into an external lookup table**: appropriate for large payloads. Overkill at 4 bytes. Rejected.

See [03-data-model.md](03-data-model.md) for the full `HitZoneKind` definition and [06-implementation.md](06-implementation.md) for the bracket-button → decorator migration sequencing.

A final note: the replacement is a net simplification of `HitZoneKind`. Five variants disappear, one new variant replaces them, and the new variant encodes strictly more information (which group, which item, which action) than the old variants did (which bracket button, nothing else). The end state is smaller and more expressive simultaneously — an unusual win.

**Confidence**: high
