# 06 -- Implementation Roadmap

> Phased plan for building the Hand of Midas widget system.
> Migrates existing code (levels, G.ATR, volume profile) into a unified
> annotation/indicator architecture, then extends with order brackets,
> advanced indicators, and polish features.
>
> Cross-references: Core types in `01-core-architecture.md`, storage in
> `02-storage-and-sync.md`, rendering in `03-rendering-pipeline.md`,
> interaction in `04-interaction-system.md`, widget specs in
> `05-widget-catalog.md`, patterns in `07-design-patterns.md`.
>
> **Path convention**: Throughout this document, `midas-chart` refers to
> `desktop/win/crates/midas-chart`, and `midas-app` refers to
> `desktop/win/crates/midas-app`.

---

## 1. Implementation Principles

### 1.1 Migrate Before Extending

Existing features (levels, G.ATR, volume profile) migrate first. Each
migration validates the architecture against battle-tested code. Only after
the migration proves the design do we build net-new widgets.

### 1.2 One Widget at a Time

Each migration is a complete vertical slice: types, store, compute,
hit-testing, rendering, persistence, and tests. The sequence:

1. HorizontalLevel (simplest annotation, most integration points)
2. GerchikAtr (simplest indicator, pure computation)
3. VolumeProfile (indicator with GPU output)
4. OrderBracket (compound annotation, multi-step tool)

### 1.3 Test at Every Layer

The Compute and Scene phases are testable without a GPU context. Every
phase ships with unit tests for compute output, hit zone geometry, state
machine transitions, and persistence round-trips.

### 1.4 Backward-Compatible Migration

Old code remains alongside new until the new path is validated. The
removal of deprecated files is always a separate, reviewable commit that
follows a full green test run.

### 1.5 No Speculative Abstractions

No trait objects, no ChartModifier trait, no render abstraction layer, no
plugin system. Build the simplest thing that works. See Section 13 for the
explicit "what not to build" list with reconsideration triggers.

---

## 2. Phase 1A: Foundation -- Core Types + AnnotationStore

**Goal**: Define the widget type system and AnnotationStore without modifying
any existing code. All 128 tests pass. New code compiles alongside old code.

### 2.1 Tasks

1. **Create module hierarchy** -- `midas-chart/src/widget/` with
   `mod.rs`, `compute.rs`, `level.rs`, `hit_test.rs`, `theme.rs`.

2. **Define core types** -- `AnnotationId(u64)`, `Presence` enum,
   `Annotation` wrapper (id, kind, presence, visible_timeframes, locked,
   created_at, modified_at), `AnnotationKind::Level(HorizontalLevel)`,
   `HorizontalLevel` (price, color, line_width, style, label, extend, icon),
   `LineStyle`, `LevelExtend`. Types per `01-core-architecture.md`.

3. **Define compute interface** -- `ComputeContext` (camera, viewport, theme,
   snap_fn), `WidgetOutput` (fills, lines, markers, labels, hit_zones),
   `HitZone` (annotation_id, bounding rect). These live in `widget/compute.rs`.

4. **Implement AnnotationStore** -- Per-symbol flat `Vec<Annotation>`, monotonic
   `next_id`, generation counter bumped on every mutation. CRUD: `add()`,
   `remove()`, `update()` (closure-based), `get()`, `get_visible()`.
   Owned by `MidasApp`. Replaces `LevelStore` from `midas-app/src/level_store.rs`.

5. **Unit tests for new code** (~8 tests) -- AnnotationStore CRUD, generation
   counter bumps, `get_visible()` filtering, serde round-trip for
   `Annotation` and `HorizontalLevel`.

### 2.2 Success Criteria (1A)

- All 128+ existing tests pass unchanged (old code untouched)
- New types compile; AnnotationStore CRUD tests pass
- New code is additive only -- no existing file modified
- Can be merged independently as a foundation commit

### 2.3 Estimated Scope (1A)

~5-6 new files, ~0-1 modified files (lib.rs re-exports only), ~8 new tests.

---

## 2B. Phase 1B: Level Migration

**Goal**: Migrate levels from `LevelStore` / `Vec<HorizontalLevel>` to
AnnotationStore. Zero user-visible behavior change.

**Depends on**: Phase 1A.

### 2B.1 Tasks

1. **Migrate HorizontalLevel** -- Map `id -> AnnotationId(id)`,
   `price/color/line_width/label/icon/locked` into `HorizontalLevel`
   fields, defaults: `style=Solid`, `extend=FullWidth`, `presence=Active`.

2. **Migrate LevelTool** -- `Dragging.level_id` becomes `AnnotationId`.
   Placement creates `Annotation { kind: Level(..) }` instead of
   `HorizontalLevel`. Drag updates price via `store.update(symbol, id, ..)`.
   Snap logic unchanged. Make `LevelTool.mode` private (add predicates).

3. **Update compute_chart_scene() -- signature** -- Add `annotations: &[Annotation]`
   and `timeframe: Timeframe` to `ChartInput`. Pre-filtered by visibility in
   the app layer via `AnnotationStore::get_visible()`.

4. **Update compute_chart_scene() -- widget compute path** -- Add
   `compute_widget_outputs()` call that iterates annotations, dispatches to
   `compute_level()` for Level variants. Merge WidgetOutput into ChartScene
   fields (`annotation_fills`, `annotation_lines`, `annotation_labels`).
   Wire into both normal-mode and collapsed-gaps-mode compute paths.

5. **Update DirtyFlags** -- Add `annotations: u64` counter. Wire
   `mark_annotations()`. Cascade from `mark_camera()` and `mark_theme()`. Add
   `needs_annotation_rebuild()` to `DirtyTracker`. Deprecate `levels` field.

6. **Migrate LevelStore persistence** -- `MidasApp` holds `AnnotationStore`.
   Levels persist to config.toml using Annotation types (same storage
   location, new data model). Full JSON file persistence deferred to Phase 6.

7. **Update handle_event() -- hit testing** -- Add `hit_test_annotations()`
   returning `Option<(AnnotationId, HitZone)>`. Wire into mouse-press path
   for annotation selection.

8. **Update handle_event() -- drag and delete** -- Wire drag (with grab
   offset), delete key, escape-to-deselect, crosshair suppression during
   drag. `ChartAction::CreateLevel/DragLevel/SelectLevel/DeleteSelectedLevel`
   use `AnnotationId`. Note: `interaction.rs` is ~2,400 lines — keep changes
   minimal, touch only the level-related code paths.

9. **All existing tests pass** -- Gate: every test in the workspace passes
   without behavioral changes to test assertions.

### 2B.2 Success Criteria (1B)

- All 128+ existing tests pass unchanged
- Levels render identically (pixel-perfect)
- Level tool (placement, snap, drag, delete, edit) works identically
- Multiple charts for the same symbol share levels automatically
- Persistence survives close/reopen cycle
- No frame-time regression

### 2B.3 Risk Mitigation

- Keep old `levels.rs` and `level_store.rs` alongside new code; diff
  `ChartScene.levels` output between paths before removing old code
- Dedicated test for generation counter propagation through dirty system
- ChartInput signature change in a single compiler-enforced commit

### 2B.4 Estimated Scope (1B)

~2-3 new files, ~12-15 modified files (including two >2,000-line files:
`compute.rs` and `interaction.rs`), ~2 deprecated files, ~7 new tests.

---

## 3. Phase 2: Indicator Architecture + G.ATR Migration

**Goal**: Define indicators as a separate category from annotations (data-derived,
per-chart, not persisted in AnnotationStore). Migrate GerchikAtr to prove it.

**Note on existing `midas-indicators` crate**: The codebase already has
`desktop/win/crates/midas-indicators/` containing pure math formulas
(`WildersAtr`, `GerchikAtr` accumulators). The new `midas-chart/src/indicators/`
module wraps these with chart-specific config, output formatting, and
`IndicatorOutput` production. The pure math stays in `midas-indicators`;
chart integration lives in `midas-chart`. Add `midas-indicators` as a
dependency in `midas-chart/Cargo.toml` if not already present.

### 3.1 Indicators vs. Annotations

| Property | Annotation | Indicator |
|---|---|---|
| Created by | User interaction | Computation from data |
| Storage | AnnotationStore (per-symbol) | IndicatorConfig (per-chart) |
| Shared across charts | Yes (same symbol) | No (each chart independent) |
| Persistence | Annotation JSON files | config.toml per-chart section |
| Hit-testable | Yes | No (display only) |

### 3.2 Tasks

1. **Add `midas-indicators` dependency** -- Add
   `midas-indicators = { path = "../midas-indicators" }` to
   `midas-chart/Cargo.toml`.

2. **Create module hierarchy** -- `midas-chart/src/indicators/` with
   `mod.rs`, `gerchik_atr.rs`.

3. **Define indicator types** -- `IndicatorKind` enum (GerchikAtr,
   VolumeProfile, future variants), `IndicatorConfig` (kind, enabled,
   per-indicator settings), `IndicatorOutput` enum (TextBadge, Instances).

4. **Add per-chart config** -- `ChartState` gains
   `indicator_configs: Vec<IndicatorConfig>`. Persisted in config.toml.

5. **Migrate GerchikAtr** -- Move `gerchik_atr.rs` to `indicators/`.
   Wrap with config check. `GerchikAtrRender` becomes an
   `IndicatorOutput::TextBadge`. Re-export from `indicators/mod.rs`.

6. **Add show/hide toggle** -- `IndicatorConfig.enabled` flag. Keyboard
   shortcut or toolbar button. Persisted per-chart.

7. **All G.ATR tests pass** -- 12 existing tests, unchanged assertions.

### 3.3 Success Criteria

- G.ATR renders identically to current behavior
- G.ATR toggleable per chart, state persists across restarts
- Architecture supports future indicators without structural changes

### 3.4 Estimated Scope

~2-3 new files, ~5-6 modified, ~1 deprecated, ~5 new tests.

---

## 4. Phase 3: Volume Profile Enhancement

**Goal**: Migrate VP into indicator architecture. Add POC line, Value Area
highlighting, hover tooltips, and computation caching.

### 4.1 Tasks

1. **Migrate volume_profile.rs** -- Move to `indicators/volume_profile.rs`.
   Wrap with `IndicatorConfig` check. Replace `ChartState.show_volume_profile`
   with config lookup.

2. **VolumeProfileConfig** -- `num_bins`, `period` (Visible/Session/FixedBars),
   `show_poc`, `show_value_area`, `value_area_pct` (default 70%),
   `max_width_fraction` (default 0.25).

3. **POC line** -- Full-width dashed horizontal line at the highest-volume
   price bin. Muted gold color. Uses `GridLineInstance` segments.

4. **Value Area** -- Compute the price range containing N% of total volume
   centered on POC. Render as two boundary lines + optional low-alpha fill.

5. **Hover tooltip** -- When cursor over a VP bar, show iced overlay tooltip
   with price range, buy/sell/total volume, percentage of total.

6. **Computation caching** -- Cache `VolumeProfile` in ChartState. Invalidate
   on camera time range change, data change, or config change.

7. **Tests** -- 14 existing tests pass. New tests for VA computation, POC
   line output, cache invalidation, config round-trip (~8 new tests).

### 4.2 Success Criteria

- VP renders correctly from new location
- POC line and Value Area visible when enabled
- Hover tooltip works
- Cached computation avoids recompute (measurable in benchmarks)

### 4.3 Estimated Scope

~1 new file (migrated + enhanced), ~5-6 modified, ~1 deprecated, ~8 new tests.

---

## 5. Phase 4: Order Bracket System

**Goal**: Users can draw entry/TP/SL brackets, see risk/reward zones, and prepare
orders for broker submission. Full design in `05-widget-catalog.md` Section 3
and `04-interaction-system.md` Section 4.

Split into two sub-phases because the interaction layer (4B) is comparable
in complexity to the entire existing LevelTool, and should only be built
after the data model and rendering are validated (4A).

### 5.1 Dependencies

Phase 1 required (AnnotationStore). Broker integration NOT required -- Draft
mode is fully functional without a broker connection.

### 5.2 Phase 4A: Data Model, Compute, and Rendering

1. **Define data model** -- `OrderBracket`, `BracketLeg`, `BracketSide`,
   `BracketStatus` per `05-widget-catalog.md` Section 3. Add
   `AnnotationKind::OrderBracket(OrderBracket)` variant.

2. **Bracket compute** -- 1-3 horizontal lines + 0-2 zone fill rects +
   price label badges + R:R ratio text. All `GridLineInstance` output.
   Visual states per `BracketStatus` (dashed/dotted/solid/dimmed).

3. **R:R calculation** -- `risk_reward(bracket) -> Option<f64>`. Displayed
   as "R:R 2.5:1" near entry label.

4. **Tests** (~10) -- Data model serde, compute output, zone fill geometry,
   R:R calculation, status visual mapping.

**4A Success Criteria**: Brackets can be created programmatically, render
correctly, and serialize round-trip. No interaction needed yet.

### 5.3 Phase 4B: BracketTool and Interaction

5. **BracketTool state machine** -- `DrawingBracket { side, phase }` mode.
   Three-click sequence: entry -> TP -> SL. OHLC snap on each click.
   Preview rendering at each step (ghost lines + zone preview).
   Escape cancels at any step.

6. **Per-leg hit-testing** -- Each leg has an independent `HitZone`.
   Click selects bracket + highlights leg. Drag moves only that leg.

7. **DraggingBracketLeg** -- New interaction mode. Constraint enforcement:
   Long TP > entry > SL, Short SL > entry > TP. OHLC snap available.

8. **Keyboard shortcuts** -- B: activate bracket tool. Escape: cancel/deselect.
   Delete: delete selected. Tab during drawing: toggle Long/Short.

9. **Tests** (~10) -- Drawing state machine, cancel paths, constraint
   enforcement, per-leg hit-test, keyboard shortcuts.

**4B Success Criteria**: Draw Long/Short brackets via 3-click sequence.
Each leg draggable with constraint enforcement. R:R updates on drag.
Zone fills render. Draft mode fully functional without broker.
Brackets persist and sync across same-symbol charts.

### 5.4 Estimated Scope

Phase 4A: ~2 new files, ~4 modified, ~10 new tests.
Phase 4B: ~2 new files, ~4-6 modified, ~10 new tests.

---

## 6. Phase 5: Advanced Widgets

Independent items, buildable in any order after their dependencies are met.

### 6.0 LinePipeline Spike (prerequisite for 6.1)

**Goal**: Validate the diagonal line rendering approach before building
dependent widgets. This is GPU pipeline engineering, not widget development.

- Define `LineInstance` GPU struct: `{ p0, p1, width, color }` (32 bytes, Pod)
- Implement WGSL shader: expand line to quad via perpendicular offsets
- Create `LinePipeline` in `midas-render/src/pipelines/line.rs`
- Render a test diagonal line in a chart to validate correctness
- **Go/no-go gate**: If the spike produces correct output, proceed to 6.1.
  If perpendicular expansion has precision issues at shallow angles,
  consider fragment-shader-based dashing as an alternative approach.

### 6.1 Moving Average Indicator

**Requires**: LinePipeline from 6.0.

- `MovingAverageConfig`: kind (SMA/EMA/WMA), period, color, width
- Output: `Vec<LineInstance>` connecting successive MA values
- Multiple MAs per chart, each with own config

### 6.2 Velocity/Momentum Indicator

- Rate-of-change per candle, mapped to color gradient (red/green)
- Output: one `GridLineInstance` per candle, renders behind candle bodies
- Configurable lookback period and color gradient

### 6.3 Text Notes and Markers

- `AnnotationKind::TextNote(TextNote)` -- click to place, type text, Enter to confirm
- `AnnotationKind::Marker(MarkerAnnotation)` -- click to place icon at price/time
- Hit-test: bounding box for notes, circular radius for markers
- Double-click to edit note text. Drag to reposition both types.

### 6.4 Estimated Scope

~5-8 new files, ~8-12 modified, ~25 new tests.

---

## 7. Phase 6: Annotation Persistence -- JSON Files

**Goal**: Extract annotations from config.toml into dedicated per-symbol JSON
files. Full format in `02-storage-and-sync.md` Section 4.

### 7.1 Tasks

1. **File format** -- `{ version, symbol, next_id, annotations: [...] }`.
   One file per symbol: `data/annotations/AAPL.json`.

2. **Forward-compatible deserialization** -- Use a two-pass approach:
   deserialize the annotations array as `Vec<serde_json::Value>`, then
   attempt to deserialize each element into `Annotation`. Entries that
   fail (unknown `AnnotationKind` variants from a later phase) are
   silently skipped with a warning log. This ensures JSON files written
   by a later phase (with `OrderBracket`, `TextNote`, etc.) can be
   loaded by earlier code without crashing. Note: `#[serde(other)]`
   cannot be used here because it only works for unit variants, and
   `AnnotationKind` has data-carrying variants. Write a test proving
   that a JSON file containing an unknown `AnnotationKind` variant
   deserializes without error (unknown entries are skipped, known
   entries are preserved).

3. **Save/load** -- Atomic writes (`.tmp` + rename). Debounced (500ms after
   last mutation). Corrupt files renamed to `.corrupt.bak`, empty store
   returned, no crash. For `BracketStatus` transitions (Draft→Pending,
   Pending→Active), flush immediately rather than debouncing, since
   these represent financial intent.

4. **One-time migration** -- Read levels from config.toml, write as
   annotations to JSON, remove from config.toml.

### 7.2 Success Criteria

- Annotations survive close/reopen. Deleting JSON starts empty (no crash).
- Migration from config.toml works once. Atomic writes prevent corruption.
- Unknown annotation variants in JSON do not cause load failure.

### 7.3 Estimated Scope

~2 new files, ~3-4 modified, ~10 new tests.

---

## 8. Phase 7: Order Bridge -- Submit to Broker

**Goal**: Brackets submit as IB bracket orders. Fill events update bracket
status and create fill markers. Bridge design in `02-storage-and-sync.md`
Section 5 and `04-interaction-system.md` Section 4.

### 8.1 Dependencies

Phase 4 + Phase 6 + midas-broker Phase 1 (IB API, LocalOrder, BrokerCommand/Event).

### 8.2 Tasks

1. **OrderAnnotationLink** -- Maps `AnnotationId` to broker order IDs.
   Lives in midas-app (chart crate does not know about orders).

2. **Submit action** -- UI button on bracket label. Creates
   `BrokerCommand::PlaceBracketOrder`. Updates status to Pending.

3. **Fill event wiring** -- Subscribe to `BrokerEvent::OrderFilled/Cancelled`.
   Update `BracketStatus`. Auto-create fill `MarkerAnnotation`.

4. **Live modification** -- Drag leg while Active triggers confirmation
   dialog, then `BrokerCommand::ModifyOrder`.

5. **Tests** (~12) -- Submit, fill status update, cancel, fill marker
   creation, modification, rejection of closed bracket modification.
   All use `MockBrokerAdapter`.

### 8.3 Success Criteria

- Paper trading: draw bracket, submit, see fills, cancel, see status changes.
- Fill markers at correct price/time. Survives restart.

### 8.4 Estimated Scope

~2-3 new files, ~4-5 modified, ~12 new tests.

---

## 9. Phase 8: Polish and Advanced Features

Independent items, prioritized by user impact.

| Feature | Description |
|---|---|
| **Undo/Redo** | Action log with inverse ops. Stack depth 50. Ctrl+Z / Ctrl+Y. |
| **Annotation Templates** | Save/load presets (e.g., "My trading levels"). JSON files in `data/templates/`. |
| **Link Groups** | Color-coded chart grouping for symbol routing. Symbol change propagates to group. |
| **Import/Export** | Export to JSON/CSV. Import from JSON. Future: TradingView import. |
| **Multi-Select** | Shift+click, box select, bulk delete/move/color. |

### 9.1 Estimated Scope

~5-8 new files, ~8-12 modified, ~20 new tests.

---

## 10. Testing Strategy

### 10.1 Unit Tests (Per Widget, in midas-chart)

| Category | Validates |
|---|---|
| Compute output | Camera + data + annotations -> correct GridLineInstances |
| Hit zone accuracy | Click position -> correct AnnotationId and zone |
| State machine | Tool mode transitions, edge cases, cancel paths |
| Constraints | Bracket leg ordering, level lock prevention |
| Edge cases | Empty data, extreme zoom, zero-width viewport, NaN |
| Serialization | Serde round-trip for all annotation/indicator types |

### 10.2 Integration Tests

| Category | Validates |
|---|---|
| AnnotationStore CRUD | Insert, update, delete, generation tracking |
| Cross-chart sync | Create annotation, verify visible on all same-symbol charts |
| Persistence round-trip | Save to JSON, load, verify equality |
| Migration | config.toml levels -> annotation JSON files |
| Tool -> Store | Tool creates annotation -> store receives -> dirty flag set |

### 10.3 Regression Gate

After each phase: `cargo test --workspace`. No phase merges until all
tests pass. Baseline: 128 tests. Count only grows.

### 10.4 Property-Based Tests (Phase 5+)

- Arbitrary annotation positions: verify no NaN, no infinite coords
- Fuzz hit-testing with random click positions: no panics
- Random bracket leg drags: constraint invariants hold

### 10.5 Performance Benchmarks

- 100 levels, single chart: compute < 0.5ms
- 50 brackets, single chart: compute < 1ms
- 20 charts, 50 annotations each: total compute < 10ms

---

## 11. File Change Map

### 11.1 New Files

```
Phase 1A: widget/{mod,compute,level,hit_test,theme}.rs
          midas-app/src/annotation_store.rs
Phase 1B: (modifies existing files only, no new files)
Phase 2:  indicators/{mod,gerchik_atr}.rs
Phase 3:  indicators/volume_profile.rs
Phase 4A: widget/order_bracket.rs
Phase 4B: widget/bracket_tool.rs
Phase 5:  midas-render/src/pipelines/line.rs, shaders/line.wgsl
          indicators/{moving_average,velocity}.rs
          widget/{text_note,marker}.rs
Phase 6:  midas-app/src/{annotation_persistence,annotation_migration}.rs
Phase 7:  midas-app/src/{order_bridge,bracket_ui}.rs
Phase 8:  widget/undo.rs, midas-app/src/{templates,link_groups}.rs
```

### 11.2 Modified Files (Cumulative)

```
midas-chart:  lib.rs, compute.rs, scene.rs, state.rs, dirty.rs,
              input.rs, instances.rs, interaction.rs
midas-app:    app.rs, chart_widget.rs, views.rs, persistence.rs
midas-core:   config.rs
midas-render: renderer.rs, pipelines/mod.rs (Phase 5 only)
```

### 11.3 Deprecated After Migration

```
Phase 1B: levels.rs, level_store.rs
Phase 2: gerchik_atr.rs
Phase 3: volume_profile.rs
```

---

## 12. Dependency Graph

```
Phase 1A: Core Types + AnnotationStore (new code only, no existing files touched)
  |
  +---> Phase 1B: Level Migration (migrate existing levels into AnnotationStore)
  |       |
  |       +---> Phase 4A: Bracket Data Model + Rendering
  |       |       |
  |       |       +---> Phase 4B: BracketTool + Interaction
  |       |
  |       +---> Phase 5: Advanced Widgets (items are independent)
  |       |       |
  |       |       +---> 5.0 LinePipeline Spike (go/no-go gate)
  |       |               |
  |       |               +---> 5.1 Moving Average (depends on spike)
  |       |
  |       +---> Phase 6 Task 4: One-time migration (depends on 1B merged)
  |
  +---> Phase 6 Tasks 1-3: File format, save/load, debounce (depends on 1A only)
  |
  +---> Phase 2: Indicator Architecture + G.ATR (independent of 1B)
  |       |
  |       +---> Phase 3: Volume Profile Enhancement
  |
  +===> INTEGRATION GATE (before Phase 7):
  |       All of Phases 1A, 1B, 4A/4B, 6 must be merged.
  |       `cargo test --workspace` must pass.
  |       Manual verification: bracket persistence round-trip.
  |
  +---> Phase 7: Order Bridge (requires integration gate + broker)
  |
  +---> Phase 8: Polish (depends on all prior phases)

Parallelizable after Phase 1A:
  Phase 2 can start immediately (no dependency on 1B -- indicators
  use a separate IndicatorKind enum in ChartState, not AnnotationStore).
  Phase 6 Tasks 1-3 (file format, save/load, debounce) can start -- they
  only need AnnotationKind types, not the level migration.

Parallelizable after Phase 1B:
  Phase 4A, Phase 5 can proceed simultaneously.
  Phase 6 Task 4 (one-time migration from config.toml) requires 1B merged.
  Phase 4B can start as soon as 4A is complete.
  Phase 5 items are independent of each other (except 5.1 depends on 5.0).

Merge order for parallel phases touching compute.rs / interaction.rs:
  Phases 1B, 2, 4A, and 4B all modify these two large files. To minimize
  merge conflicts, merge in this order:
  1. Phase 1B first (adds annotation dispatch loop to compute.rs)
  2. Phase 2 next (adds indicator dispatch block -- smallest change)
  3. Phase 4A (adds bracket match arm to existing dispatch)
  4. Phase 4B (adds bracket interaction to handle_event)
  Each subsequent merge appends match arms to the dispatch established
  by Phase 1B, rather than restructuring the function.

Forward compatibility: Each phase that adds a new AnnotationKind variant
MUST add a persistence round-trip test for that variant, even if Phase 6
has not started yet (the test uses in-memory serde, not file I/O).
```

**Critical path**: Phase 1A → Phase 1B → Phase 4A → Phase 4B → Integration Gate → Phase 7.

---

## 13. What NOT to Build

| Feature | Why Skip | Reconsider When |
|---|---|---|
| Trait object dispatch | Enum is 10-12x faster, closed set | >15 annotation types or plugin API |
| ChartModifier trait | handle_event() ~2,400 lines but mostly existing pan/zoom | >15 interaction modes or widget code exceeds 500 lines within handle_event |
| Render abstraction | Direct wgpu is correct | SVG/PDF export needed |
| Scene graph | Flat Vec is correct for <500 items | >1000 annotations or spatial query hotspot |
| Plugin system | Proprietary app | User-defined widgets needed |
| Multi-select | Low priority | User demand |
| Trend lines/Fibonacci | Requires LinePipeline (Phase 5) | After MA proves the pipeline |
| Real-time P&L on brackets | Requires live data feed | After midas-feed streaming |

---

## 14. Migration Safety Checklist

Before removing any deprecated file, verify:

- [ ] `cargo test --workspace` -- all green
- [ ] `cargo clippy --workspace -- -D warnings` -- clean
- [ ] No dead code warnings on new path
- [ ] All re-exports updated in lib.rs
- [ ] ChartInput signature matches all call sites
- [ ] Config persistence round-trip (save, quit, reopen, verify)
- [ ] Level tool works (place, drag, delete, edit, snap)
- [ ] Volume profile renders (toggle on, verify histogram)
- [ ] G.ATR renders (intraday data, verify badge)
- [ ] Cross-chart sync (2 charts same symbol, place level, verify both)
- [ ] Performance gate: 20 charts open, frame time < 14ms
- [ ] Binary size delta < 50KB
