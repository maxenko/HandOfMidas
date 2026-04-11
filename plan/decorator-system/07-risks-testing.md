# Risks, Testing & Non-Goals

This file collects the cross-cutting concerns that don't fit cleanly into a single slice. Implementation detail lives in [06-implementation.md](06-implementation.md).

It is organised in three parts:

- **Risks & Mitigations** — nine identified risks with per-risk mitigation, ordered from highest uncertainty (shader work) to lowest (byte-size auditing).
- **Testing Strategy** — per-slice, integration, visual, and performance bars.
- **Non-Goals / Out of Scope** — eleven explicit exclusions, each with a one-paragraph rationale.
- **Review Notes** — pre-answers to "why X instead of Y" questions a reviewer will ask during PR review.

Cross-references:

- Design rationale: [02-design-decisions.md](02-design-decisions.md)
- Data-model invariants referenced in risks: [03-data-model.md](03-data-model.md)
- Interaction model referenced in Risks 3 and 6: [05-interaction.md](05-interaction.md)
- Slice ordering and per-slice tests: [06-implementation.md](06-implementation.md)

## Risks & Mitigations

The risks below are ordered from highest uncertainty to lowest. The first three are "novel technical work" where the outcome isn't knowable without prototyping. The remaining six are "known issues with known mitigations" — recorded here so reviewers can confirm each has been thought through rather than overlooked.

### 1. SDF shader complexity for `PointLeft` / `Chevron`

Rectangles, rounded rects, pills, and circles are textbook SDFs — every shader tutorial has the formulas, and wgpu has no special issues with them. The pointed and chevron shapes require combining a rect SDF with a triangle SDF via `min()`, and the triangle SDF (iq's standard formula on Inigo Quilez's site) has edge cases at the tip where anti-aliasing breaks down if the smoothing width approaches the tip radius.

The specific failure modes to watch for:

- **Jagged tip** — at 1× DPI, the point edge becomes visibly stair-stepped because `smoothstep()` has no sub-pixel data to interpolate
- **Fringing** — where the triangle and rect SDFs meet via `min()`, a half-pixel seam can appear if the two SDFs disagree on distance units
- **Rounded tip artefact** — `smoothstep()` applied to a pointed SDF rounds the tip visibly, losing the crisp corner the design calls for

The risk is not that we can't draw a pointed rectangle — it's that the visual quality at 1× DPI might be unacceptable compared to discrete geometry. On a retina display the problems vanish; on a 1920×1080 monitor they may not.

**Mitigation**: the spike is now **Slice 0** — a standalone go/no-go gate that runs before any other decorator work. It prototypes the shader in isolation and validates that `PointLeft` and `Chevron` render crisply at both 1× and 2× DPI. The slice produces a throw-away binary that renders one instance of each shape on a blank wgpu surface, nothing more. Done criteria: visual review at 1×, 1.25×, 1.5×, and 2× DPI passes.

If the spike fails, the project falls back to **geometry decomposition** (explicit triangle + rect geometry per shape), documented as the reject path in Decision 5 of [02-design-decisions.md](02-design-decisions.md). Six of the nine subsequent slices (1, 2, 3, 7, 8a, 8b, 9) remain valuable as pure refactor even without SDF — the core primitive split, layout engine, interaction model, and decorator migration are all shape-agnostic. Only Slices 4, 5, and 6 depend directly on the spike result, and even those can run with a rect-only `DecoratorShape` set.

In practice this means: **the decorator system has no single point of failure**. The worst case is a six-week refactor that leaves the project in a cleaner state with discrete geometry instead of SDF, and a future agent can revisit the shader work as an incremental upgrade. The decorator system is decoupled enough from the shape-rendering mechanism that shape improvements are self-contained.

### 2. Text measurement inside badges

Slice 3 uses a `font_size * 0.6 * char_count` heuristic for text width during layout. This is fine for fixed-digit numbers (prices, quantities) but wrong for proportional-width labels where "MMMM" is dramatically wider than "iiii". The `0.6` factor is a common approximation for the average character width of a sans-serif font at typical body sizes, but it's a rough one.

Concretely, the failure mode is: a badge containing proportional text lays out with too much trailing whitespace (if text is narrower than the heuristic expected) or clips slightly (if text is wider). Neither is catastrophic — the badge still renders, the text still reads — but the visual polish suffers.

**Mitigation**: the heuristic is good enough for the screenshots — all badge text is digits and uppercase letters that approximate monospace widths. If a real label later appears wrong-sized in practice, swap in iced's `Text::measure()` or a glyph-metrics crate (`fontdue`) in Slice 7 during the real-label migration. The measurement call-site is a single function in the layout engine (`measure_text_width()` in `widget/decorator/layout.rs`), so the swap is contained to one file and doesn't ripple through the decorator constructors.

The reason this isn't addressed up-front: measuring via iced's real text system requires a `Text::Font` handle, which the compute pipeline doesn't currently hold. Threading it through adds plumbing to every `compute_*()` function for a benefit (tighter label widths) that may never be exercised. The heuristic is cheap; real measurement is not.

### 3. Frame-ordering on hover persistence

The old plan framed this as a one-frame lag risk: the set used in frame N was computed from frame N-1's hit zones, and a decorator group that expanded and contracted within a single frame could flicker. **It is no longer.**

The new recompute-in-`update()` default (see Decision 7 in [02-design-decisions.md](02-design-decisions.md) and the interaction walkthrough in [05-interaction.md](05-interaction.md)) eliminates the cross-frame dependency. Hover state is recomputed against the **current frame's cursor** and the **current frame's hit zones**, synchronously, on every mouse-move event. No frame-N / frame-N-1 mismatch is possible because there is no cached set spanning frames.

**How the recompute works**: on every `update()` call driven by a `Message::MouseMoved`, the chart program runs a lightweight version of `compute_decorator_hit_zones()` over the current visible annotations, walks the zones from deepest to shallowest, and picks the hit whose bounds contain the cursor. The result is written to `ChartState::hovered_annotation` before the next `draw()` call observes it. Order of operations: message in → recompute → state write → draw. No cache, no lag.

**Residual risk**: if a future measurement shows the recompute is too expensive and forces the fallback `RefCell` cache path, the iced 0.14 `Program::draw()` lifecycle reintroduces the one-frame-lag and cache-ordering risk. `draw()` in iced 0.14 can fire on:

- theme change (user switches light/dark)
- viewport resize (window drag, devtools open)
- animation frames (if `Subscription::animation_frames()` is active)
- actual input events

Only the last one advances the "hover clock" if we cache, so a theme swap or viewport resize could render a frame with a stale hover set.

**Mitigation**: benchmark before falling back. The Slice 5 hover recompute benchmark (see Testing Strategy below) defends the default path. If the benchmark shows < 50 µs per event, the fallback never kicks in and this residual risk stays hypothetical.

### 4. One-pass layout, no shrinking

If a badge's measured text width exceeds the viewport width, the layout overflows off-screen. Badges always grow to fit their contents and never shrink to fit the viewport or collide with neighbouring badges. The layout engine is single-pass: it walks the decorator tree once, accumulating width and placing items, with no second pass to redistribute space.

**Mitigation**: `WidgetOutput` fills are clipped by the render pipeline to the chart bounds, so visually the overflow is invisible — it just means some badge content is off-screen to the right or bottom. If a decorator needs to shrink-to-fit, add `min_width` / `max_width` per segment in a follow-up. The layout engine in Slice 2 is a pure function so this is a non-breaking addition.

Accepting this risk means: we are explicitly choosing **"overflow silently"** over **"shrink and degrade"**. For a trading UI where badge text is price data, showing "1234" clipped to "123" is worse than showing "1234" extending past the chart edge — the second is obviously wrong (visibly off-screen), the first is dangerously wrong (plausible but incorrect number).

A related concern is **badge collision with the crosshair or other chart elements**. The draw order contract in [03-data-model.md](03-data-model.md) places badges after candle bodies and before the crosshair, so the crosshair draws on top of the badge when they overlap. This is intentional — the crosshair is interactive and must always be visible — but it means the badge text can be temporarily occluded during cursor movement. This is accepted behaviour, not a bug.

### 5. Persisted config migration

Slice 7 changes the on-disk `HorizontalLevel` layout. A user's existing `config.toml` must continue to load without data loss, without user intervention, and without a manual migration step. This is non-negotiable: any migration strategy that requires the user to run a command, edit a file, or accept a prompt is a failed design.

**Mitigation**: Slice 7 uses a **manual `Deserialize` impl** with a v1-compat fallback struct. The impl attempts the v2 shape first; on failure, it tries the v1 shape and converts. This is explicitly **not** one of these alternatives:

- **Not `#[serde(untagged)]` enum**: untagged enums silently pick the wrong branch on ambiguous input. If v1 and v2 share a subset of fields (which they will, since `price` and `id` don't change), serde can deserialise a v2 file as v1 or vice versa without erroring. The resulting data corruption is silent.
- **Not `#[serde(default)]` on individual fields**: too coarse. `serde(default)` works when you're adding new fields to an existing struct. It does not work when field semantics change (e.g., `style: LineStyle` going from a string enum to a struct).

Both writer sites are named: `level_store/mod.rs:19` and `annotation_persistence.rs`. A fixture file (`desktop/win/tests/fixtures/config_v1_pre_decorator.toml`) is checked in and loaded by the migration test, which asserts round-trip equivalence: load v1 → convert to v2 → re-serialise → compare structure.

The `midas-store` DuckDB verification was resolved during plan writing: annotations are **not** persisted in `midas-store` (grepping the crate for `HorizontalLevel`, `BracketLeg`, `LineStyle`, `Level`, and `Annotation` returned no schema-level hits; the store holds only candle data). The migration is therefore local to `midas-app`, touching only `level_store/mod.rs:19` and `annotation_persistence.rs`. See Slice 7's config-migration section in [06-implementation.md](06-implementation.md).

### 6. Hit-zone `item_path` ambiguity across frames

If a decorator group is re-ordered between frames — e.g., an item disappears due to visibility rules, or hover state adds an item at index 0 and shifts everything else down — the `item_path` of remaining items could shift. A click landed against frame N-1's path numbering would target the wrong item in frame N. The concrete hazard: a user clicks a button, the click event fires with `item_path = [0, 1, 2, 0]`, but by the time the handler runs, the item at that path is a different item because the group composition changed.

**Mitigation**: `item_path` is only used for disambiguating clicks within a **single frame** — it is not persisted and not stable across frames. As long as one frame's hit-zone emission is consistent with the same frame's draw (guaranteed by the draw-order contract in [03-data-model.md](03-data-model.md)), click routing is correct. The one-frame recompute model from Risk 3 covers mouse events within the same frame.

`item_path` is `[u8; 4]` + `u8` length tag (per Decision 8 in [02-design-decisions.md](02-design-decisions.md)) — max 4 levels of nesting. This is more than enough for every decorator layout in the screenshots. The deepest is:

1. `BracketLeg` (annotation)
2. Hover-reveal group (decorator group)
3. Button row (container segment)
4. Individual button (leaf item)

Four levels, filled exactly. If a future decorator needed a five-level hierarchy — e.g., a nested accordion inside a hover-reveal — the path format needs enlarging. See Risk 9 for the upgrade path.

### 7. Unknown — `compute_bracket()` behavior that won't cleanly map to decorators

The existing `compute_bracket()` may have behaviour (selection glow, drag preview, partial-fill highlighting) that won't cleanly map to the decorator model. The decorator model assumes a badge is a passive visual carrier; if `compute_bracket()` has state-aware rendering (e.g., "animate the fill while an order is pending"), that doesn't fit.

This is marked "unknown" because the current `compute_bracket()` was written incrementally and its full behaviour set isn't documented in one place. The Slice 8a-i work begins with a read-through of the function to enumerate what it actually does, which will either confirm the mapping is clean or expose the misfit.

**Mitigation**: Slice 8a-i starts with a snapshot test of the current `compute_bracket()` output and refactors the data model until the snapshot matches primitive-count parity via the rendering shim. Slice 8a-ii then swaps the shim for `compute_decorator_group()` and asserts a new snapshot that reflects the decorator shape. Either step surfaces any behaviour that can't be expressed as decorators, and the snapshot is checked in as the baseline. Any such behaviour gets flagged during that process and handled either as:

- **(a)** a new `DecoratorShape` variant — if the behaviour is a shape the existing SDF pipeline can't draw (e.g., a horizontal capsule with a notch), add it to the enum and ship a shader tweak.
- **(b)** direct line-primitive emission in `compute_price_line_geometry()` alongside the decorator pass — if the behaviour is a non-badge visual (e.g., a glow halo around the line itself), emit it as a regular `WidgetOutput` line and leave decorators alone.
- **(c)** a new `ItemContent` variant — if the behaviour is a text-ish or icon-ish content that wasn't anticipated, extend `ItemContent` with a new variant. This is cheap because `ItemContent` is a sealed enum under our control.

All three escape hatches are cheap. The refactor is not gated on perfect decorator coverage; it's gated on primitive-count parity with the baseline. If primitive counts match, the visual is equivalent and the refactor ships.

### 8. `BadgeInstance` byte size

64 bytes is larger than `CandleInstance` (48 bytes) and `GridLineInstance` (32 bytes). Worth noting, not worrying. The extra bytes are spent on per-instance shape params (corner radius, point offset, rotation) that the other primitives don't need — they're the cost of the unified shader approach.

**Mitigation**: at 2000 decorators per frame (wildly pessimistic — see the performance target in Testing Strategy for a realistic 100-decorator upper bound), `BadgeInstance` costs 128 KB per frame of upload bandwidth. That's dwarfed by candle instance traffic for any non-trivial chart (a 5000-candle visible range is ~240 KB of `CandleInstance` uploads per frame). No action needed beyond documenting the trade-off in the struct's doc comment.

If the decorator system ever exceeds 10k decorators per frame, revisit — but 10k decorators on one chart implies a UX problem, not a perf problem. The user has no reason to ever see 10k anything on a single chart view; anything beyond the hundreds indicates a runaway emission bug, not a legitimate scene.

Note: the 64-byte figure is a placeholder pending the actual Slice 4 shader definition. The final size depends on how many SDF parameters fit into the instance vs. being uniforms. If the final struct is smaller (say 48 bytes), this risk becomes even more academic. If larger (say 80 bytes), the 128 KB figure scales linearly — still negligible.

### 9. `HitZoneKind` Copy cascade

The new `HitZoneKind::Decorator` variant uses `[u8; 4]` + `u8` instead of `SmallVec<[u8; 4]>` to preserve `#[derive(Copy)]` on the enum.

**Why it matters**: dropping `Copy` from `HitZoneKind` would cascade through:

- `HitZone` (holds a `HitZoneKind`)
- Every `(AnnotationId, HitZoneKind)` pair in `ComputeContext::hovered_annotation`
- `ChartInput::hovered_annotation`
- `ChartState::hovered_annotation`
- Every site that destructures `hit.kind` via `match` without explicit cloning
- Any function signature that takes `HitZoneKind` by value

The ripple is wider than the decorator-system surface area justifies. Counting rough call sites, dropping `Copy` means touching 20+ files for a change that doesn't buy us anything the fixed-array solution can't provide.

**The constraint**: if a future decorator needs deeper than 4 levels of nesting, the `[u8; 4]` cap becomes a hard limit. The current design is comfortable — the deepest screenshot layout is 4 levels — but a ribbon or multi-row decorator tree could push past it.

**Mitigation**: the cap is documented in Decision 8 of [02-design-decisions.md](02-design-decisions.md) and in the `HitZoneKind::Decorator` variant's doc comment. If breached, the fix is one of:

- **(a)** Enlarge the array to `[u8; 8]`. Cost: 4 extra bytes per hit zone, or ~16 KB extra per frame at 2000 hit zones. Preserves `Copy`. No API change.
- **(b)** Bit-pack a `u32` path. 8 nibbles, up to 15 children per level, 8 levels deep. 4 bytes instead of 5. Preserves `Copy`. Needs a helper to pack and unpack.

Both preserve `Copy`. The decision in 02 is not a dead end — it's a default with a clear upgrade path.

## Testing Strategy

The test pyramid for the decorator system looks like this, from most to least frequent:

**Per-slice unit tests** — each slice adds targeted tests for its new functions and types. Existing test files gain new cases rather than being rewritten. Slice boundaries in [06-implementation.md](06-implementation.md) list the specific test names per slice. Target: every new public function has at least one unit test; every new enum variant has at least one match-exhaustiveness test. The unit tests live next to the code they cover (inside `#[cfg(test)] mod tests` blocks or, where the project has been separated, in sibling `_tests.rs` files — follow the convention of the file being edited).

**Visual parity tests** — Slices 7, 8a-i, and 8a-ii each include a parity test that snapshots the `WidgetOutput` primitive counts (fills, lines, labels, hit zones, badges) for a representative annotation. Slice 7 asserts "identical to pre-refactor" for levels; Slice 8a-i asserts the same for brackets via the data-model rewrite + rendering shim; Slice 8a-ii asserts "identical to the new decorator shape" after the visual migration. Any divergence fails the test. Parity is counted, not pixel-compared — see the Review Notes section below for why this is the right bar.

The snapshot format is plain text: one line per primitive type with a count. Example:

```
fills: 4
lines: 2
labels: 2
hit_zones: 3
badges: 0
```

Before the refactor, the snapshot is generated from the current compute function and checked in as a `.snap` file alongside the test. After the refactor, the test runs the new compute function and asserts the same counts. If the refactor intentionally changes counts (e.g., a hit zone is removed because decorators don't need it), the snapshot is updated in the same commit as the refactor and the diff is reviewed.

**Integration tests** — `desktop/win/tests/integration_gate.rs` gains new cases for decorator click → `ChartAction::DecoratorClick` → side-effect round-trips. Cases cover:

- Price-badge click — expected: no-op (passive decorator, no `DecoratorAction`)
- Hover-reveal group click on TP — expected: `ChartAction::DecoratorClick(CreateTakeProfit)`
- Hover-reveal group click on SL — expected: `ChartAction::DecoratorClick(CreateStopLoss)`
- Click outside any decorator — expected: fall through to line-drag start or chart-pan
- Click on a decorator belonging to a hidden annotation — expected: ignored
- Click on a decorator while dragging another annotation — expected: ignored (drag mode takes precedence)
- Hover enter a group, move within the group, hover leave — expected: group stays expanded throughout, collapses on leave

**Manual visual verification** — after Slices 0, 4, 7, 8a-i, and 8a-ii, a human runs the app with test data and confirms the rendering matches the designs. This is a gating step, not a nice-to-have. Slice 4's done criteria include a dev-only `--show-decorator-showcase` feature flag that spawns one badge of every shape (rect, rounded-rect, pill, circle, `PointLeft`, `Chevron`) on a dummy chart. The showcase is the visual canary that the SDF shader and layout engine are working end-to-end before any real annotation migrates.

The checks at each gating slice are:

- **After Slice 0**: spike binary renders pointed and chevron shapes cleanly at 1× and 2× DPI. Go/no-go decision recorded in the slice commit message.
- **After Slice 4**: showcase flag spawns one badge per shape; all shapes render at the expected size and colour; SDF edges are smooth; no flicker.
- **After Slice 7**: existing levels render identically to the pre-refactor state. Visual parity checked by loading a config with several levels and comparing side-by-side screenshots.
- **After Slice 8a-i**: existing brackets render pixel-identical to pre-refactor via the new data model. Primitive-count parity snapshot is green; no visual diff expected.
- **After Slice 8a-ii**: brackets render according to the new design. Hover reveal works, click routing works via the legacy path, no visual regression on existing bracket features.

**Regression suite** — full `cargo test --workspace` (both root and `desktop/win/`) plus `cargo clippy --workspace -- -D warnings` passes after every slice. No slice lands yellow. The slice is not done until the regression suite is green and no new warnings appear.

The commands run in both workspaces:

```bash
cargo test --workspace                              # root workspace
cd desktop/win && cargo test --workspace            # desktop workspace
cargo clippy --workspace -- -D warnings             # both, separately
```

Any new warning surfaced by clippy during a slice must be fixed before the slice is considered done. The project is clippy-clean today and the decorator work will not be the reason it stops being.

**Property tests (optional)** — `segmented_line()` is a good quickcheck candidate. Any random `[f32; N]` pattern walked over any random length should produce non-overlapping rects whose total "on" length matches the analytic expectation:

```
sum(on_segments) * floor(length / period) + partial_tail
```

Opt-in via a `proptest` feature — not required to merge but nice to have for the stroke pattern helper. Property tests are particularly valuable here because `segmented_line()` has a lot of fiddly off-by-one conditions (last segment truncated, first segment aligned to origin, pattern wrap-around) that unit tests tend to miss.

Candidate invariants for `segmented_line()`:

- Sum of "on" rect widths equals the analytic expectation within float epsilon
- No two rects overlap in x
- All rects lie within `[0, total_length]`
- Pattern with all-zero "off" segments produces exactly one rect of full length
- Pattern with all-zero "on" segments produces zero rects

**Performance target — decorator upload + draw stays under 100 µs per frame at 60 Hz with 100 visible decorators**. Measured via the existing render benchmark if one exists; otherwise eyeballed during the Slice 4 manual visual check. 100 decorators is a generous upper bound — realistic scenes will have 10-30 decorators visible at any time:

- One price badge + one label badge per level × 5-10 levels = 10-20 decorators
- One bracket hover-reveal group when hovered = 0-4 decorators
- Future additions (markers, text notes) unlikely to exceed 10 simultaneously

The 100-decorator cap gives headroom for future annotation kinds (`TextNote`, `Marker`, ribbons) without forcing a second perf pass. If a real chart exceeds 100 decorators, something else has gone wrong with annotation design.

The 100 µs figure is 1/166th of a 60 Hz frame (16.67 ms), leaving plenty of headroom for candle rendering, grid, crosshair, and iced overlay. If the decorator system starts eating more than 1% of the frame budget, the slice that introduced the regression must own fixing it before moving on.

**Hover recompute benchmark** — in Slice 5, measure `compute_decorator_hit_zones()` cost on mouse-move. Target **< 50 µs per event**. If exceeded, the `RefCell` fallback path from Decision 7 in [02-design-decisions.md](02-design-decisions.md) kicks in, and Risk 3's residual lag becomes real. The benchmark name and location are stated in [06-implementation.md](06-implementation.md) Slice 5. The benchmark should use a realistic scene (20-30 decorators, mix of passive badges and hover groups) rather than a synthetic worst case.

**Test scope boundaries**. The following are explicitly **not** covered by the test strategy here:

- Cross-platform rendering differences (Windows-only for now)
- Multi-monitor DPI transitions (out of scope for decorator v1)
- Theme colour contrast auditing (separate accessibility pass)
- Fuzzing of `item_path` deserialisation (not persisted, not needed)

The regression suite catches everything that touches existing tests. Anything new is covered by slice-specific tests listed in [06-implementation.md](06-implementation.md). If a class of bug slips through, the pattern to follow is: write a failing test first, then fix — same as the rest of the codebase.

## Non-Goals / Out of Scope

The following are explicit exclusions. Each has a one-paragraph rationale — not because they're unimportant, but because they expand scope past the vertical slice the decorator system is trying to land.

**GPU text rendering inside badges**. Text stays in the iced overlay layer. Decorator badges lay out around pre-measured text but do not rasterize glyphs on the GPU. Adding GPU text is a separate project that touches the whole render pipeline, not just decorators — it requires a text atlas, glyph caching, shader changes, and interaction with iced's text system. Deferring it keeps the decorator slice tractable.

**Animation / transitions**. Hover buttons pop in and out instantaneously. No fade, no slide, no easing. If animation is wanted, add a `Presence`-style alpha field to `DecoratorItem` in a follow-up — the struct has room for it and the renderer already handles per-instance alpha via `BadgeInstance`. Animation requires an `animation_frames()` subscription and an interpolation clock, both of which are cross-cutting concerns better added once for the whole app.

**Keyboard navigation of decorator items**. Buttons respond to mouse clicks only. Tab-to-focus and enter-to-click are deferred. The interaction layer has no keyboard-focus concept for annotations today, and adding one is a systemic change beyond decorator scope. A keyboard-nav pass would need to define focus order, visual focus indicator, and keyboard-to-`DecoratorAction` dispatch — a separate vertical slice.

**Tooltips on decorator hover**. No tooltips on badge hover. If needed later, add a `Tooltip` variant to `ItemContent` and route it through the iced overlay layer. The hover state tracking from Slice 5 provides the hook, but no overlay plumbing is included here. Tooltips also introduce a dwell-timer concept that doesn't exist in the current chart state machine.

**Drag-and-drop of decorators**. Decorators are static per-frame. The line itself is draggable (via `HitZoneKind::LevelLine`, `BracketTP`, `BracketSL`, etc.), but individual badges are click-only. Draggable decorators would require a parent-relative drag-origin concept that doesn't exist yet, plus a collision model for where a dragged badge can be dropped.

**Per-chart theming of decorator colors**. Decorator fills are hard-coded at construction time in the `to_decorators()` pattern. Theme-awareness can come later by passing `&Theme` into the constructor functions — but that means every caller site needs a theme handle, which is a wider change touching `compute_level()`, `compute_bracket()`, and every annotation-compute path. Deferred until a real theming need appears.

**`LevelExtent::Between` interaction with decorator anchors**. If a level has a bounded `Between { start, end }` extent, decorators at `RightEdge` should probably anchor to `end` instead of the viewport right edge. This is an edge case — `Between` is rarely used — and is deferred until a real use case hits it. The anchor system in Slice 2 has room for a `ContextRelativeEnd` anchor kind if needed. **Interim behavior**: until the anchor kind is added, `DecoratorAnchor::RightEdge` resolves to the viewport's right edge regardless of the parent line's extent, so a `Between`-extent line will have its right-edge decorators visually detached from the line's actual right endpoint. This is acceptable because `Between` extents aren't used in the initial level or bracket layouts — no user-visible issue at v1.

**Migrating `TextNote` and `Marker` annotation kinds to decorators**. Those variants exist in `AnnotationKind` but are not yet dispatched in `compute_widget_annotations()`. The decorator system makes their future implementation trivial (a single `to_decorators()` impl each), but adding them is a separate feature. The decorator slice proves the pattern against levels and brackets; extending it to other annotation kinds is a follow-up.

**Cross-annotation decorator alignment (auto-stacking)**. If two brackets sit at similar prices, their right-edge badges will overlap. No auto-stacking. A future "annotation layout pass" could de-overlap by running a global collision step over all emitted badges — not in scope here. Auto-stacking is a hard problem (non-local layout, jitter on small-price-change animations) and deferring it avoids over-engineering the v1.

**Broker order round-trip for quick-create TP/SL**. Slice 6 hooks up the `DecoratorAction::CreateTakeProfit` / `CreateStopLoss` dispatch to chart-state mutation, but the actual broker submission flow (IB paper trading) is out of scope and lives behind the `BrokerBridge` abstraction. Decorator click → chart state → broker command is a separate vertical slice tracked in Phase 1 of the broker work.

**Undo / redo for decorator actions**. Creating a TP/SL via a hover-reveal button mutates chart state but does not push an undo entry. Undo is not yet implemented for any annotation action in the chart, and the decorator system does not try to be the first. When a project-wide undo stack lands, decorator actions will plug into it via the existing action dispatch — no decorator-specific work needed.

## Review Notes

These are pre-answers to the "why X instead of Y" questions a reviewer will ask during PR review. They mirror the shape of decision rationale but stay at the review-talking-points level — full rationale lives in [02-design-decisions.md](02-design-decisions.md).

**Why a new top-level primitive instead of extending `HorizontalLevel`**. The persisted `HorizontalLevel` carries lifecycle concerns (id, locked, created-at-style metadata). The renderer-side `HorizontalLevel` carries visual concerns (extent, style). `BracketLeg` carries order-role concerns. Pulling out `PriceLine` as the shared geometry primitive keeps each concern in its own type without dragging lifecycle or role fields into places that don't need them. Extending `HorizontalLevel` would have pulled bracket-only fields into a type that levels don't need.

**Why SDF instead of decomposition**. Decomposition works for any individual shape but scales badly across a shape set. Adding a new shape with decomposition means new geometry plus new pipeline work. Adding a new shape with SDF means ~10 lines of WGSL. The user will ask for chevrons, half-pills, ribbons, and who-knows-what — SDF is the pattern that stays cheap as the shape catalog grows. The Slice 0 spike is the insurance policy: if SDF turns out to be a dead end for `PointLeft` / `Chevron`, decomposition is the documented fallback.

**Why flex layout instead of absolute positioning**. Absolute positioning would require every decorator constructor to know viewport width and compute screen coordinates. Flex lets constructors stay pure data — position is resolved at compute time by the layout engine. This is critical for the `to_decorators()` pattern on `HorizontalLevel` and `OrderBracket`, which runs per-frame in `compute_*()` without access to rendering context. Constructors are straightforward Rust data literals instead of coordinate math.

**Why `SmallVec<[f32; 6]>` for `LineStyle::Pattern`** (and note: this is for `LineStyle`, not for `HitZoneKind::Decorator::item_path` — the latter uses fixed `[u8; 4]` because it's inside a `Copy` enum). All six preset patterns fit inline: the longest is `dash_dot_dot` with 6 entries. `smallvec` avoids allocation for every stroke while still allowing arbitrary user-defined patterns for custom line styles. The `6` matches the longest preset exactly — if a longer pattern shows up it'll heap-allocate, which is fine for an edge case that probably won't exist.

**Why `u16` for `group_id`**. 65k decorator groups per annotation is absurdly generous. `u8` (256) is arguably enough but feels cramped. `u16` gives room to number groups non-densely (e.g., group 0 = price badge, group 10 = label badge, group 20 = lock badge) for readability without worrying about overflow. The 2-byte cost over `u8` is immaterial — `DecoratorGroup` isn't stored in GPU instances.

**Visual parity vs pixel parity**. Slices 7 and 8a-i aim for **visual parity** — the user perceives the rendering as unchanged. They do **not** aim for pixel parity — a one-pixel shift in badge position is acceptable if the layout engine reaches the right result by a different path. Parity tests count primitive types, not exact coordinates. Pixel parity would over-constrain the refactor and block legitimate improvements (e.g., rounding to nearest device pixel). Slice 8a-ii intentionally changes the visual, so its parity test asserts a different snapshot — the new decorator shape, not the old one.

**Where Slice 8a-ii's decorator constructors live**. A new file `widget/order_bracket/decorators.rs` rather than inside `mod.rs`. `mod.rs` is already large (~800 lines) and adding three 60-line constructor functions would push it over. The new file is unit-testable in isolation and keeps the decorator model discoverable for future annotation kinds — anyone adding a new bracket decorator knows exactly where to look.

**Migration order (levels first, brackets second) is deliberate**. Levels have simpler decoration needs (just a price badge and a label badge) and zero interactive hover buttons. Proving the machinery against levels first means Slice 8a-ii's bracket migration can focus on the novel screenshot-specific parts (hover-reveal groups, nested stacks, multi-segment badges with sub-shapes) without also debugging the core compute engine. If the machinery is broken, levels will expose it cheaply; brackets would bury the root cause under interactive complexity.

**`compute_price_line_geometry()` vs `compute_decorator_group()`**. These stay as two separate functions even though both are called from `compute_level()` and `compute_bracket()`. The geometry function handles line-level concerns (selection glow, drag ghost, the line itself, the line hit zone for dragging) that are **not decorators** and shouldn't be bolted onto the decorator model. Keeping them separate keeps each function small and testable, and preserves the distinction between "the line" and "things attached to the line".

**Why `item_path: [u8; 4]` + `u8` length and not `SmallVec<[u8; 4]>`**. `HitZoneKind` is `#[derive(Copy)]`. Dropping `Copy` would cascade to `HitZone`, every `(AnnotationId, HitZoneKind)` pair in `ComputeContext::hovered_annotation`, `ChartInput::hovered_annotation`, `ChartState::hovered_annotation`, and every site that matches on `hit.kind` without explicit cloning. The fixed-array solution is 5 bytes per `Decorator` variant and preserves the existing invariant. Risk 9 above covers the 4-level-nesting cap and its upgrade path; the short version is that the cap is comfortable today and enlarging it is a one-line change if we ever hit it.

**What changes for users without the decorator payoff**. Slices 1 through 6 and Slices 7, 8a-i land infrastructure and pure refactors that are invisible in the running app. **Slice 8a-ii is the first moment the user sees a difference** — that's when the new tag designs from the screenshots replace the current bracket rendering. Prioritise getting to Slice 8a-ii; Slices 8b and 9 can always come later.

With Slice 4's dev-only `--show-decorator-showcase` feature flag, the team has a visual canary from Slice 4 onward, so "is the decorator system alive?" is answerable before any real annotation migrates. This matters for confidence pacing: it means a developer can demo progress after Slice 4 (showcase) and again after Slice 8a-ii (bracket redesign), with Slices 5–8a-i as "trust us, the tests pass" phases in between.

**Why the split of old Slice 8 into 8a-i, 8a-ii, and 8b**. The original plan had one large "migrate brackets" slice that combined the bracket-compute refactor with the new decorator content AND the interactive cutover. Splitting into three creates safer merge points: 8a-i lands the data model with pixel-identical output via a rendering shim (easy revert if snapshot tests drift); 8a-ii swaps the rendering path to decorators with no interactive change; 8b migrates the interaction path and deletes legacy hit-zone variants. Each step is independently reviewable and independently revertable. The split is a risk-reduction move; it does not change total work, but it does let a reviewer understand each PR in isolation.

**Why no decorator-specific feature flag in production**. The showcase flag is `--show-decorator-showcase` and is dev-only. There is no runtime toggle between "old bracket rendering" and "new bracket rendering" for end users. The reason: the temporary rendering shim in Slice 8a-i and the coexisting legacy hit zones during 8a-ii already introduce one transitional dimension of complexity; adding a runtime toggle on top of that would double the test surface without user benefit. The refactor ships as a hard cut — shim is deleted in 8a-ii, legacy hit zones deleted in 8b. If a regression lands, the fix is a hotfix slice, not a runtime flag flip.
