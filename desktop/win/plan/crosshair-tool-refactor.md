# Crosshair Tool Refactor

Design plan for extracting the crosshair into a self-contained component
within `midas-chart`.

---

## 1. Current Crosshair Functionality

The crosshair is a full-viewport overlay that shows the user's cursor
position on the chart. It provides:

- **Visibility rule.** Only visible when the left mouse button is held
  down. Suppressed during `DraggingVolumeScale`, `DraggingTimelineBorder`,
  and `LevelTool::Dragging` interactions.
- **Vertical line.** Snaps to the center X of the nearest candle (via
  `data.find_index_by_time` in normal mode, or index-space rounding in
  collapsed mode).
- **Horizontal line.** Follows the cursor Y exactly (pixel-snapped).
- **Price label.** Displayed on the Y axis at the cursor's price.
- **Time label.** Displayed on the X axis at the snapped candle's
  timestamp.
- **OHLCV data overlay.** Symbol, datetime, OHLC values, volume, change
  and change percentage for the candle under the cursor (TC2000-style
  data box in the top-left corner).
- **Level placement preview.** When `level_tool.is_placing()` the
  crosshair's horizontal line is repurposed as a preview line. When
  Alt is not held, the Y position snaps to the nearest OHLC value of
  nearby candles (via `LevelTool::snap_to_ohlc()` in the interaction
  layer).
- **GPU rendering.** The `CrosshairRender` struct is converted into two
  `GridLineInstance` rectangles (one vertical, one horizontal) inside
  `prepare()`, then drawn in a separate overlay pass.

### Where the code lives today

| Concern | File | Specific locations |
|---|---|---|
| State storage | `state.rs` | `ChartState.crosshair_pos: Option<(f32, f32)>`, `ChartState.left_mouse_down: bool` |
| Visibility logic | `interaction.rs` | `handle_mouse_moved`: checks `left_mouse_down && !dragging_handle && in_bounds` |
| Show on press | `interaction.rs` | `handle_mouse_pressed` (Left): sets `left_mouse_down = true`, emits `SetCrosshair` |
| Hide on release | `interaction.rs` | `handle_mouse_released` (Left): sets `left_mouse_down = false`, emits `ClearCrosshair` |
| Level placing override | `interaction.rs` | Early return in `handle_mouse_moved` when `level_tool.is_placing()` — crosshair visible without left mouse |
| Level drag suppression | `interaction.rs` | `crosshair_pos = None` at PendingDrag→`LevelTool::Dragging` transition; `ClearCrosshair` emitted from drag early-return |
| Cancel clears | `interaction.rs` | `handle_key_pressed(Escape)` calls `level_tool.cancel()`, clears `crosshair_pos` |
| State mutation | `state.rs` | `apply_action(SetCrosshair)` and `apply_action(ClearCrosshair)` |
| Normal-mode compute | `compute.rs` | `compute_crosshair()` (~60 lines): cursor-to-time, snap to candle, build labels + OHLCV |
| Collapsed-mode compute | `compute.rs` | `compute_collapsed_crosshair()` (~60 lines): index-space snap, same label + OHLCV build |
| OHLC snap for levels | `compute.rs` | Inline in scene builders: reads `input.level_tool.snapped_price` to adjust `crosshair.horizontal_y` and price label |
| Scene output | `scene.rs` | `ChartScene.crosshair: Option<CrosshairRender>` |
| Input contract | `input.rs` | `ChartInput.crosshair: Option<(f32, f32)>`, `ChartInput.level_tool: &LevelTool` |
| Render data struct | `instances.rs` | `CrosshairRender`, `OhlcvOverlay`, `AxisLabel` |
| Widget snapshot sync | `chart_widget.rs` | `ChartRenderSnapshot.crosshair_pos`, `ChartRenderSnapshot.level_tool: LevelTool` |
| GPU instance build | `chart_widget.rs` | `crosshair_to_instances()`: two `GridLineInstance` rects |
| GPU upload | `chart_widget.rs` | `prepare()`: `resources.crosshair_lines`, preview line when `level_tool.is_placing()` |
| Dirty tracking | `dirty.rs` | `DirtyFlags.crosshair: u64` generation counter |

---

## 2. Problems

### 2.1 Scattered state

`crosshair_pos` and `left_mouse_down` are separate fields on `ChartState`
with no grouping. Their invariants (crosshair visible iff left mouse down
AND not dragging a handle AND in bounds) are enforced by convention across
multiple call sites rather than by a single authoritative method.

### 2.2 Duplicated visibility logic

The "should the crosshair be visible?" decision is made in at least four
places:

1. `handle_mouse_moved` in `interaction.rs` -- checks
   `left_mouse_down && !dragging_handle && in_bounds`.
2. `handle_mouse_pressed` in `interaction.rs` -- unconditionally sets
   crosshair on left press (after volume handle / timeline border
   early-returns).
3. `handle_mouse_released` in `interaction.rs` -- unconditionally
   clears crosshair on left release.
4. `level_tool.is_placing()` early return in `handle_mouse_moved` --
   overrides the normal visibility rule (visible without left mouse).

A bug in any one of these sites can leave the crosshair stuck visible or
invisible. There is no single `should_render()` predicate.

### 2.3 SetCrosshair/ClearCrosshair actions emitted from many places

`SetCrosshair` is emitted from:
- `handle_mouse_moved` (normal tracking)
- `handle_mouse_moved` (level placing tracking)
- `handle_mouse_pressed` (on left press)

`ClearCrosshair` is emitted from:
- `handle_mouse_moved` (when cursor leaves bounds or left is released)
- `handle_mouse_released` (always on left release)
- `handle_key_pressed(Escape)` (cancel placement and general escape)
- `handle_mouse_pressed` (level placing left-click placement --
  clears after creating level)

This scatter makes it difficult to reason about crosshair lifecycle.

### 2.4 Tight coupling with level tool

The level tool refactor (completed) extracted level state into
`LevelTool` and moved OHLC snap into the interaction layer. This
partially decoupled the crosshair from level concerns, but the
crosshair is still reused for the placement preview line via ad-hoc
conditionals:

- `handle_mouse_moved` has a special early return when
  `level_tool.is_placing()` that sets `crosshair_pos` directly on
  `ChartState` and emits `SetCrosshair` regardless of `left_mouse_down`.
- The compute layer reads `input.level_tool.snapped_price` and mutates
  the already-built `CrosshairRender` to adjust `horizontal_y` and the
  price label inline in `compute_normal_scene`/`compute_collapsed_scene`.
- `prepare()` in `chart_widget.rs` checks `self.placing_level` (a bool
  derived from `level_tool.is_placing()`) and adds extra
  `GridLineInstance` rects (blue preview line + glow).

The crosshair has no concept of "modes" -- it simply happens to be
reused by the level tool through ad-hoc conditionals.

### 2.5 Large, duplicated compute functions

`compute_crosshair()` and `compute_collapsed_crosshair()` are ~90 lines
each with nearly identical structure:

1. Unpack cursor position (or return `None`)
2. Find nearest candle index
3. Compute snap X
4. Build price label
5. Build time label
6. Build OHLCV overlay

The only difference is step 2 (timestamp lookup vs. index-space
rounding). The label and OHLCV construction is copy-pasted.

### 2.6 Widget-layer complexity

`chart_widget.rs` has crosshair concerns woven throughout:

- `ChartRenderSnapshot` carries `crosshair_pos`
- `update()` syncs `crosshair_pos` from snapshot, syncs `level_tool`
  when tool is idle
- `draw()` reads crosshair from widget state *or* snapshot
  (`cs.crosshair_pos.or(snap.crosshair_pos)`)
- `prepare()` converts `CrosshairRender` to GPU instances, then
  conditionally adds preview-line instances for placement mode
- `ChartGpuResources` caches `crosshair_lines: Vec<GridLineInstance>`

---

## 3. Proposed Design: CrosshairTool

### 3.1 New file

```
midas-chart/src/crosshair_tool.rs
```

Added to `midas-chart/src/lib.rs` as `pub mod crosshair_tool;`.

### 3.2 CrosshairMode enum

```rust
/// The crosshair's operational mode.
#[derive(Clone, Debug, PartialEq)]
pub enum CrosshairMode {
    /// Crosshair is hidden (default state, no mouse button held).
    Hidden,
    /// Crosshair tracks the cursor while left mouse is held.
    /// This is the standard charting crosshair behavior.
    Tracking,
    /// Crosshair provides a preview line for an external tool
    /// (e.g., level placement). Visible regardless of mouse button.
    /// The tool is responsible for entering/exiting this mode.
    Preview,
}
```

### 3.3 CrosshairTool struct

```rust
/// Self-contained crosshair component.
///
/// Owns all crosshair state and provides a clean API for show/hide/update.
/// Lives as a field on `ChartState`.
#[derive(Clone, Debug)]
pub struct CrosshairTool {
    /// Current operational mode.
    mode: CrosshairMode,
    /// Cursor position in chart-local pixels (always tracked, even when hidden).
    cursor_pos: Option<(f32, f32)>,
    /// Whether the left mouse button is currently held.
    left_mouse_down: bool,
}
```

### 3.4 Public API

```rust
impl CrosshairTool {
    /// Create a new CrosshairTool in Hidden mode.
    pub fn new() -> Self;

    /// Current mode.
    pub fn mode(&self) -> &CrosshairMode;

    /// The cursor position that should be used for rendering,
    /// or `None` if the crosshair should not be rendered.
    /// This is the single source of truth for crosshair visibility.
    pub fn render_pos(&self) -> Option<(f32, f32)>;

    /// Whether the crosshair should be rendered this frame.
    /// Equivalent to `self.render_pos().is_some()`.
    pub fn should_render(&self) -> bool;

    // ── Mutation methods (called by interaction layer) ──

    /// Record that the left mouse button was pressed at (x, y).
    /// Transitions from Hidden to Tracking if not in Preview mode.
    /// Returns true if the crosshair became visible.
    pub fn on_left_press(&mut self, x: f32, y: f32) -> bool;

    /// Record that the left mouse button was released.
    /// Transitions from Tracking to Hidden.
    /// Returns true if the crosshair became hidden.
    pub fn on_left_release(&mut self) -> bool;

    /// Update cursor position. Called on every mouse move.
    /// Only updates the visible position if the crosshair is active.
    /// `in_bounds` indicates whether the cursor is within the chart area.
    pub fn on_mouse_move(&mut self, x: f32, y: f32, in_bounds: bool);

    /// Enter preview mode (for level tool). Crosshair becomes visible
    /// at the given position regardless of mouse button state.
    pub fn enter_preview(&mut self, x: f32, y: f32);

    /// Exit preview mode. Returns to Hidden or Tracking based on
    /// whether the left mouse button is held.
    pub fn exit_preview(&mut self);

    /// Force-hide the crosshair (e.g., Escape key, viewport resize).
    /// Resets to Hidden mode and clears left_mouse_down.
    pub fn force_hide(&mut self);

    /// Whether the left mouse button is currently held.
    /// Exposed so interaction.rs can check without a separate field.
    pub fn left_mouse_down(&self) -> bool;

    /// Whether we are in Preview mode (for level tool queries).
    pub fn is_preview(&self) -> bool;

    /// Re-enter preview mode using the last known cursor position.
    /// Called after `level_tool.try_resume_placing()` restores placing
    /// mode — the crosshair should return to Preview at the stored
    /// position without the caller needing to track coordinates.
    /// No-op if `cursor_pos` is `None`.
    pub fn resume_preview(&mut self);

    /// Unconditionally set the crosshair position and make it visible.
    /// Used by `apply_action(SetCrosshair)` for backward compatibility
    /// with external callers that set crosshair via the action system
    /// rather than through `on_left_press` / `on_mouse_move`.
    /// Transitions to Tracking if currently Hidden.
    pub fn set_pos(&mut self, x: f32, y: f32);
}
```

### 3.5 Visibility invariant (single source of truth)

The `render_pos()` method encapsulates the entire visibility rule:

```rust
pub fn render_pos(&self) -> Option<(f32, f32)> {
    match self.mode {
        CrosshairMode::Hidden => None,
        CrosshairMode::Tracking | CrosshairMode::Preview => self.cursor_pos,
    }
}
```

No other code needs to check `left_mouse_down && !dragging_handle &&
in_bounds`. Those checks are done inside the `on_*` methods:

- `on_left_press` sets mode to `Tracking` (unless already `Preview`).
- `on_left_release` sets mode to `Hidden` (unless in `Preview`).
- `on_mouse_move` updates `cursor_pos`. When `!in_bounds` in
  `Tracking` mode, sets `cursor_pos = None` but keeps mode as
  `Tracking` — so re-entering bounds immediately restores visibility
  without needing another `on_left_press`. In `Preview` mode,
  `!in_bounds` is ignored (preview stays visible at last position).

The `dragging_handle` suppression is handled by the interaction layer:
it simply does not call `on_left_press` when starting a volume
scale / timeline border / level drag. This is already the existing
pattern (early returns in `handle_mouse_pressed`).

### 3.6 Integration with ChartState

```rust
pub struct ChartState {
    // ... existing fields ...

    /// Self-contained crosshair component.
    pub crosshair: CrosshairTool,

    // REMOVED:
    // pub crosshair_pos: Option<(f32, f32)>,
    // pub left_mouse_down: bool,
}
```

`ChartState::apply_action` for `SetCrosshair` and `ClearCrosshair`
delegates to the tool:

```rust
ChartAction::SetCrosshair { x, y } => {
    self.crosshair.set_pos(x, y);
    self.dirty.mark_crosshair();
}
ChartAction::ClearCrosshair => {
    self.crosshair.force_hide();
    self.dirty.mark_crosshair();
}
```

Eventually, `SetCrosshair`/`ClearCrosshair` actions may be removed
entirely if the crosshair tool handles all transitions internally.
That is a Phase 4 cleanup -- for the initial refactor, keeping the
actions preserves backward compatibility.

### 3.7 Compute function consolidation

The two compute functions share ~80% of their logic. Extract the common
parts:

```rust
/// Build the labels and OHLCV overlay for a crosshair at the given
/// candle index and snap positions.
fn build_crosshair_data(
    data: &dyn CandleData,
    camera: &Camera2D,
    data_idx: usize,
    snap_x: f32,
    snap_y: f32,
    snap_ts: i64,
    symbol: &str,
) -> CrosshairRender;
```

The two entry points become thin wrappers that resolve the candle
index and snap X differently:

```rust
/// Compute crosshair (normal timestamp mode).
fn compute_crosshair(
    pos: Option<(f32, f32)>,
    data: &dyn CandleData,
    camera: &Camera2D,
    symbol: &str,
) -> Option<CrosshairRender> {
    let (cx, cy) = pos?;
    if data.is_empty() { return None; }
    let cursor_time = camera.x_to_time(cx);
    let nearest_idx = data.find_index_by_time(cursor_time as i64);
    let snap_ts = data.timestamp(nearest_idx);
    let snap_x = camera.snap_to_pixel(camera.time_to_x(snap_ts as f64));
    let snap_y = camera.snap_to_pixel(cy);
    Some(build_crosshair_data(data, camera, nearest_idx, snap_x, snap_y, snap_ts, symbol))
}

/// Compute crosshair (collapsed index mode).
fn compute_collapsed_crosshair(
    pos: Option<(f32, f32)>,
    data: &dyn CandleData,
    camera: &Camera2D,
    vis_start: usize,
    vis_end: usize,
    symbol: &str,
    index_to_x: &dyn Fn(usize) -> f32,
) -> Option<CrosshairRender> {
    let (cx, cy) = pos?;
    // ... index-space snap logic (unchanged) ...
    Some(build_crosshair_data(data, camera, data_idx, snap_x, snap_y, snap_ts, symbol))
}
```

This eliminates ~60 lines of duplicated label/OHLCV construction.

### 3.8 ChartInput change

```rust
pub struct ChartInput<'a> {
    // CHANGED: use render_pos() output from CrosshairTool
    pub crosshair: Option<(f32, f32)>,
    // No structural change needed -- the field already carries
    // Option<(f32, f32)>. The caller just reads it from
    // state.crosshair.render_pos() instead of state.crosshair_pos.
}
```

### 3.9 Level tool interaction

The level tool uses a clean API instead of ad-hoc conditionals:

```rust
// In handle_mouse_moved, level placing branch:
if state.level_tool.is_placing() {
    state.level_tool.alt_held = alt_held;
    if in_bounds {
        state.crosshair.enter_preview(x, y);
        // snap_to_ohlc updates level_tool.snapped_price
        if let Some(data) = data {
            let snapped = state.level_tool.snap_to_ohlc(
                state.camera.y_to_price(y), x, &state.camera, data, is_collapsed);
            let snapped_y = state.camera.price_to_y(snapped);
            state.crosshair.enter_preview(x, snapped_y);
            return vec![ChartAction::SetCrosshair { x, y: snapped_y }];
        }
        return vec![ChartAction::SetCrosshair { x, y }];
    }
    return Vec::new();
}

// In handle_mouse_pressed, level placing left-click:
state.crosshair.force_hide();

// In handle_key_pressed, Escape:
state.level_tool.cancel();
state.crosshair.force_hide();

// At PendingDrag→LevelTool::Dragging transition:
state.crosshair.force_hide();
```

The `enter_preview` / `force_hide` methods make the intent explicit.
The crosshair tool knows it is in preview mode and adjusts its
visibility rule accordingly (visible without left mouse button).
`force_hide()` is used instead of `exit_preview()` at cancel/placement
because the crosshair should always hide (not transition to Tracking
even if the left mouse happens to be down).

---

## 4. Migration Steps

### Phase 1: Create CrosshairTool struct (no behavioral changes)

**Files changed:** `crosshair_tool.rs` (new), `lib.rs`

1. Create `midas-chart/src/crosshair_tool.rs` with `CrosshairMode`,
   `CrosshairTool`, and all public methods.
2. Add `pub mod crosshair_tool;` to `midas-chart/src/lib.rs`.
3. Write unit tests for `CrosshairTool` in isolation:
   - `new()` starts in `Hidden` mode
   - `on_left_press` transitions `Hidden -> Tracking`
   - `on_left_release` transitions `Tracking -> Hidden`
   - `on_mouse_move` updates position only when active
   - `enter_preview` / `exit_preview` mode transitions
   - `force_hide` resets everything
   - `render_pos` returns `None` when `Hidden`
   - Preview mode stays visible after `on_left_release`
   - `on_left_press` does not override `Preview` mode

**Tests:** All existing tests pass (no integration yet). New unit tests
for CrosshairTool.

### Phase 2: Move crosshair state into CrosshairTool and update interaction.rs

**Files changed:** `state.rs`, `crosshair_tool.rs`, `interaction.rs`

**Note:** Phases 2 and 3 are merged into a single phase to avoid
dual-ownership of `left_mouse_down`. If we added the tool separately
but kept interaction.rs writing `state.left_mouse_down` directly, the
deprecated getter (delegating to the tool) would read stale values.
Migrating all write sites atomically with adding the tool prevents this.

1. Add `pub crosshair: CrosshairTool` field to `ChartState`.
2. In `ChartState::new()`, initialize with `CrosshairTool::new()`.
3. **Immediately migrate all interaction.rs write sites** (this was
   previously Phase 3 — merged here for correctness):
   - Replace `state.left_mouse_down = true` with
     `state.crosshair.on_left_press(x, y)`.
   - Replace `state.left_mouse_down = false` with
     `state.crosshair.on_left_release()`.
   - Replace `state.crosshair_pos = Some(...)` with
     `state.crosshair.on_mouse_move(x, y, in_bounds)`.
   - Replace `state.crosshair_pos = None` with `force_hide()` or
     other appropriate tool method.
   - Replace `state.left_mouse_down` reads with
     `state.crosshair.left_mouse_down()`.
   - Replace the `level_tool.is_placing()` early return's direct state
     manipulation with `state.crosshair.enter_preview(x, y)`.
   - Replace escape/cancel's crosshair clearing with
     `state.crosshair.force_hide()`.
   - At PendingDrag→`LevelTool::Dragging` transition, call
     `state.crosshair.force_hide()`.
   - Remove the `dragging_handle` visibility check — suppression is
     handled by the tool's mode (see verification below).
4. Mark `crosshair_pos` and `left_mouse_down` as `#[deprecated]` on
   `ChartState`. They remain as fields for app-layer compatibility
   (removed in Phase 4).
5. Update `apply_action(SetCrosshair)` and `apply_action(ClearCrosshair)`
   to call `CrosshairTool` methods.
6. Update `apply_action(CancelPlacing)` to call `force_hide()` (not
   `exit_preview()` — cancel should always hide, even if left mouse
   is held).

**Tests:** All existing tests pass. The deprecated accessors keep
downstream code working.

**Verification:** The `dragging_handle` suppression works because:
- Volume handle press: early return before `on_left_press` is called
  → mode stays `Hidden`.
- Timeline border press: same pattern.
- Level drag: enters `PendingDrag` first (which does call
  `on_left_press` → mode becomes `Tracking`). When mouse moves past
  threshold near a level, `crosshair.force_hide()` is called at the
  PendingDrag→`LevelTool::Dragging` transition. On left release,
  `on_left_release()` keeps it hidden. The `level_tool.is_dragging()`
  early-return in `handle_mouse_moved` also prevents any crosshair
  updates during the drag.

**Tests:** All existing interaction tests pass with updated assertions.

### Phase 3: Consolidate compute functions

**Files changed:** `compute.rs`

1. Extract `build_crosshair_data()` helper containing the shared
   label + OHLCV construction logic.
2. Simplify `compute_crosshair()` to: resolve candle index and snap
   position, then call `build_crosshair_data()`.
3. Simplify `compute_collapsed_crosshair()` to: resolve index via
   index-space, then call `build_crosshair_data()`.
4. Change both functions to accept `Option<(f32, f32)>` from
   `CrosshairTool::render_pos()` (no signature change needed, this
   is already the parameter type).
5. Verify that the inline `level_tool.snapped_price` adjustment
   (which adjusts `crosshair.horizontal_y` and the price label in
   `compute_normal_scene`/`compute_collapsed_scene`) still works
   correctly with the consolidated `build_crosshair_data()` output.
   The adjustment reads `input.level_tool.snapped_price` and mutates
   the `CrosshairRender` after construction — this pattern is
   unchanged by the compute consolidation.

**Tests:** All existing crosshair compute tests pass. Add a test for
`build_crosshair_data()` directly.

### Phase 4: Simplify widget layer and remove deprecated aliases

**Files changed:** `chart_widget.rs`, `state.rs`, `app.rs`

1. Remove the deprecated `crosshair_pos` field from `ChartState`
   (replaced by `crosshair.render_pos()`).
2. Remove the deprecated `left_mouse_down` field from `ChartState`
   (replaced by `crosshair.left_mouse_down()`).
3. Update `ChartRenderSnapshot` to read `crosshair_pos` from
   `state.crosshair.render_pos()`.
4. Update `update()` in `ChartProgram` to sync the `CrosshairTool`
   rather than individual fields:
   ```rust
   // Replace:
   //   chart_state.crosshair_pos = None;
   //   chart_state.left_mouse_down = false;
   // With:
   chart_state.crosshair.force_hide();
   ```
5. Update `draw()` to read crosshair from `CrosshairTool`:
   ```rust
   crosshair: state
       .chart_state
       .as_ref()
       .and_then(|cs| cs.crosshair.render_pos())
       .or(snap.crosshair_pos),
   ```
6. Optionally move `crosshair_to_instances()` into `CrosshairTool`
   (or into `midas-chart` as a method on `CrosshairRender`), since it
   is pure geometry that does not need `wgpu` types.
7. Consider removing `SetCrosshair`/`ClearCrosshair` from `ChartAction`
   if all state transitions are handled internally by `CrosshairTool`
   methods called from `interaction.rs` (Phase 2). This would
   eliminate the "action emitted from many places" problem. The dirty flag
   can be marked directly:
   ```rust
   // In handle_mouse_moved:
   let was_visible = state.crosshair.should_render();
   state.crosshair.on_mouse_move(x, y, in_bounds);
   let is_visible = state.crosshair.should_render();
   if was_visible != is_visible || is_visible {
       state.dirty.mark_crosshair();
   }
   ```
   **Required before action removal:** The app layer directly writes
   `crosshair_pos` at these sites, which must be migrated first:
   - `app.rs:~794` — viewport resize: `chart.chart_state.crosshair_pos = None`
     → `chart.chart_state.crosshair.force_hide()`
   - `app.rs:~853` — crosshair message handler: sets `crosshair_pos`
     to `Some` or `None` → use `set_pos(x, y)` or `force_hide()`
   - `app.rs:~1338` — global Escape handler: `crosshair_pos = None`
     → `chart.chart_state.crosshair.force_hide()`
   - `chart_widget.rs:~177` — widget viewport resize: `crosshair_pos = None`,
     `left_mouse_down = false` → `chart_state.crosshair.force_hide()`

**Tests:** All existing tests updated to use new API. Remove tests that
tested the deprecated fields directly.

---

## 5. Risks and Mitigations

### 5.1 Level placing / Preview mode edge cases

**Risk:** The level tool temporarily suspends placing for right-click
panning or middle-click scaling via `level_tool.suspend_placing()`.
When the pan/scale ends, `level_tool.try_resume_placing()` is called
in `handle_mouse_released`, which re-enters `LevelToolMode::Placing`.
The crosshair tool needs to handle this correctly — it should return
to `Preview` mode when placing resumes.

**Mitigation:** After calling `level_tool.try_resume_placing()`, check
if placement was restored and re-enter preview mode:
```rust
state.level_tool.try_resume_placing();
if state.level_tool.is_placing() {
    state.crosshair.resume_preview();
}
```
`resume_preview()` re-enters Preview mode using the internally stored
`cursor_pos`, keeping the field private. No external position tracking
needed.

### 5.2 Widget state sync complexity

**Risk:** The `update()` method in `chart_widget.rs` syncs state between
the app's canonical `ChartState` and the widget's local copy. Adding
`CrosshairTool` sync could introduce subtle ordering bugs.

**Mitigation:** `CrosshairTool` is a value type (implements `Clone`).
Sync it the same way other fields are synced -- overwrite from snapshot
at the start of `update()`, then let interaction events mutate the local
copy. The tool's internal state machine handles transitions correctly
regardless of when it is cloned.

### 5.3 Dirty flag tracking

**Risk:** If `SetCrosshair`/`ClearCrosshair` actions are removed (Phase
4), the dirty flag must still be marked when crosshair visibility or
position changes.

**Mitigation:** The interaction layer marks `dirty.crosshair` after
calling any `CrosshairTool` mutation method that changes visibility or
position. The tool's methods return `bool` indicating whether state
changed, making this straightforward.

### 5.4 Test coverage for suppression

**Risk:** The level drag suppression relies on `force_hide()` being
called at the PendingDrag→`LevelTool::Dragging` transition. Missing it
would show a crosshair during level drags.

**Mitigation:** Add explicit integration tests:
- Start left press (crosshair visible via `on_left_press`)
- Move past drag threshold near a level (enters `LevelTool::Dragging`)
- Assert crosshair is not visible (`force_hide()` was called)
- Release mouse (exits drag, `on_left_release()`)
- Assert crosshair is not visible (left released → `Hidden` mode)

---

## 6. File Impact Summary

| File | Change type | Phase |
|---|---|---|
| `crosshair_tool.rs` | New file | 1 |
| `lib.rs` | Add `pub mod crosshair_tool` | 1 |
| `state.rs` | Add `crosshair: CrosshairTool` field, deprecate old fields | 2 |
| `interaction.rs` | Use `CrosshairTool` methods instead of direct field access | 2 |
| `compute.rs` | Extract `build_crosshair_data()`, simplify two compute fns | 3 |
| `chart_widget.rs` | Sync `CrosshairTool`, simplify crosshair reads | 4 |
| `input.rs` | No structural change (reads from `render_pos()`) | 4 |
| `scene.rs` | No change | -- |
| `instances.rs` | No change | -- |
| `dirty.rs` | No change | -- |

---

## 7. Testing Strategy

### Unit tests (Phase 1)

Standalone tests for `CrosshairTool` state machine:

- Mode transitions: `Hidden -> Tracking -> Hidden`
- Mode transitions: `Hidden -> Preview -> Hidden`
- Mode transitions: `Tracking -> Preview -> Tracking`
- `render_pos()` returns `None` in `Hidden`
- `render_pos()` returns position in `Tracking` and `Preview`
- `force_hide()` from any mode
- `on_mouse_move` with `in_bounds = false` hides in Tracking
- `on_mouse_move` with `in_bounds = false` does NOT hide in Preview
- `on_left_release` does NOT exit Preview mode

### Integration tests (Phase 2)

End-to-end event sequences through `handle_event`:

- Left press -> move -> release: crosshair visible during move
- Left press -> move past threshold -> panning: crosshair visible
- Left press near volume handle -> move: crosshair NOT visible
- Left press near timeline border -> move: crosshair NOT visible
- Left press near level -> drag (`LevelTool::Dragging`): crosshair NOT visible
- Level placing (`level_tool.is_placing()`) -> move: crosshair visible (preview)
- Level placing -> right-click pan -> release -> crosshair restored (resume)

### Compute tests (Phase 3)

- `build_crosshair_data()` produces correct labels and OHLCV
- Normal and collapsed modes produce identical OHLCV for same candle
- Inline `level_tool.snapped_price` adjustment works with consolidated output

---

## 8. Lines of Code Estimate

| Phase | New/changed lines | Net LOC delta |
|---|---|---|
| Phase 1 | ~150 (struct + tests) | +150 |
| Phase 2 | ~80 changed, ~10 new | +10 |
| Phase 3 | ~60 new helper, ~120 simplified | -60 |
| Phase 4 | ~40 changed, ~20 removed | -20 |
| **Total** | | **+80** |

The net increase is modest because the compute function consolidation
(Phase 3) and deprecated field removal (Phase 4) offset the new struct.
