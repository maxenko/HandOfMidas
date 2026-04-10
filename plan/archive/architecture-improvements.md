# Architecture Improvements — Chart, Level, Crosshair

## Assessment Summary

Overall grade: A-. Sans-IO boundary discipline is flawless, dirty generation
counters are elegant, type design is excellent. The items below are polish,
not structural rework.

## 1. Remove deprecated crosshair field bridges

**Priority**: Medium
**Files**: `midas-chart/src/state.rs`, `midas-chart/src/interaction.rs`,
`midas-app/src/chart_widget.rs`, `midas-app/src/app.rs`

`crosshair_pos` and `left_mouse_down` on `ChartState` are `#[deprecated]`
but still actively synced. Actual scope:

- `interaction.rs`: 15 `#[allow(deprecated)]` pragmas
- `state.rs`: 5 `#[allow(deprecated)]` pragmas
- `app.rs`: 4 `#[allow(deprecated)]` syncing `crosshair_pos`
- `chart_widget.rs`: `ChartRenderSnapshot.crosshair_pos` field + 3 usages

`CrosshairTool` already owns this state — the bridges add maintenance
friction and no value.

**Action**: Remove both deprecated fields from `ChartState`. Remove
`crosshair_pos` from `ChartRenderSnapshot`. Eliminate all 24+
`#[allow(deprecated)]` usages. Update callers in both midas-chart and
midas-app to use `CrosshairTool` methods exclusively.

## 2. Unify normal/collapsed code paths

**Priority**: Medium
**Files**: `midas-chart/src/compute.rs`

Crosshair, grid labels, and visible range detection each have two
near-identical implementations for normal vs collapsed-gaps mode.
The candle/volume builders already solved this with closures
(`x_for_candle(idx)`). Apply the same pattern to unify:

- `compute_crosshair()` / `compute_collapsed_crosshair()`
- `compute_timeline_ticks()` / `compute_collapsed_timeline_ticks()`
- `visible_candle_range()` / `visible_candle_range_collapsed()`

**Approach**: Extract an `XMapping` closure or small struct that
encapsulates "cursor X -> candle index" and "candle index -> screen X"
for both modes. Pass it into shared logic.

**Key design challenge**: The collapsed path uses `vis_start`/`vis_end`
offsets and an `index_to_x` closure (see `compute_collapsed_crosshair`
at compute.rs:718). The normal path uses `camera.x_to_time()` directly.
The unified abstraction must handle the global-to-local index mapping
that only the collapsed path needs. Reference `build_candle_instances()`
which already solves the analogous problem with an `x_for_candle(idx)`
closure parameter.

## 3. Add interaction sequence tests

**Priority**: Medium
**Files**: `midas-chart/src/interaction.rs` (test module)

185 unit tests cover individual state machines well, but no tests
exercise realistic event sequences:

- Press -> drag across level -> release -> verify level moved + crosshair hidden
- Place level tool -> start timeline drag -> release -> verify tool resumes
- Right-click pan -> zoom -> double-click to place level
- Crosshair active -> cursor leaves bounds -> re-enters -> verify restore

**Approach**: Add a `mod integration_tests` section with helpers that
chain `handle_event()` calls and assert intermediate + final state.

## 4. Clean up RightClickLevel action

**Priority**: Low
**Files**: `midas-chart/src/interaction.rs`, `midas-app/src/app.rs`

`ChartAction::RightClickLevel { id, x, y }` carries screen coordinates —
a UI concern, not domain logic. This is the one spot where the event/action
boundary leaks implementation details.

**Approach**: Change to `ChartAction::EditLevel { id }` and let the app
layer compute popup position from the level's `screen_y` and cursor pos
at view time.

## 5. Deduplicate crosshair label computation

**Priority**: Low
**Files**: `midas-chart/src/compute.rs`

`build_crosshair_data()` (GPU pipeline) and `compute_crosshair_labels()`
(view overlay) both independently compute price/time labels from cursor
position. The duplication exists because `ChartScene` is consumed inside
the shader pipeline and doesn't reach the view layer.

**Approach**: Either extract shared helper both call, or accept the
duplication as a cost of the clean sans-IO/overlay split. Not worth
restructuring data flow for this alone.

## Reference

- **Flowsurface** (github.com/flowsurface-rs/flowsurface): closest iced
  charting app in the ecosystem, worth studying for comparison
- **Halloy** (github.com/squidowl/halloy): 90K+ line iced app, reference
  for large-app structure with pane_grid and multi-window
