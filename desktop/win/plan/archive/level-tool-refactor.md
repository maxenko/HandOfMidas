# Level Tool Refactor -- Self-Contained Component Design

**Status:** SOLID (post-evaluation, all findings addressed)
**Date:** 2026-03-28
**Scope:** `midas-chart`, `midas-app`

## 0. Goals and Non-Goals

### Goals

1. **Single source of truth** for level tool state: one `LevelTool` struct
   owns mode, alt-held, snapped price, and suspend/resume logic.
2. **Single snap implementation**: one `snap_to_ohlc()` method replaces
   duplicated functions in compute.rs and app.rs.
3. **Testable in isolation**: `LevelTool` can be unit-tested without
   constructing a full `ChartState` or iced widget tree.
4. **All existing behavior preserved** — verified by existing tests plus
   new `LevelTool` unit tests and manual regression (Section 10).

### Non-Goals

- **Trend lines or other drawing tools.** This refactor extracts the level
  tool only. A generic `DrawingTool` trait is premature until a second tool
  exists.
- **Undo/redo.** Level operations remain non-undoable. Undo is a separate
  feature that would sit above the tool layer.
- **Multi-tool abstraction / trait.** `LevelTool` is a concrete struct,
  not a trait implementor. Introducing a trait now would be YAGNI.
- **Persistence format changes.** `LevelConfig` TOML schema is unchanged.
- **Multi-chart level synchronization.** Levels are per-chart. Syncing
  a level across multiple charts of the same symbol is a separate feature.

## 0.1 Alternatives Considered

**Alternative A: Keep `PlacingLevel`/`DraggingLevel` on `InteractionMode`,
just unify snap.**
Pro: minimal churn. Con: doesn't solve the orthogonality problem
(PlacingLevel is not a camera mode — it must coexist with panning/scaling).
The widget sync hacks remain. Rejected.

**Alternative B: `DrawingTool` trait with `LevelTool` as first impl.**
Pro: establishes a pattern for future tools. Con: only one tool exists
today, so the trait would have exactly one implementor — classic YAGNI.
The trait boundary would also complicate the interaction between tool and
state machine. Deferred until a second tool is needed.

**Chosen: concrete `LevelTool` struct on `ChartState`.**
Best balance of simplicity, testability, and correctness for the current
single-tool case. Easy to extract a trait later if needed.

## 0.2 Known Behavioral Changes

The unified `snap_to_ohlc` uses the compute.rs algorithm (nearest ±1
candles, adaptive threshold 15–40px) for both placement and drag. The
previous app.rs drag snap searched ALL visible candles with a fixed 20px
threshold. This means dragging a level far from its origin candle will
no longer snap to distant candles it previously would have. This is
intentional — snapping to a candle 500px away on the X axis was
confusing UX.

The `DoubleClick` handler now snaps to OHLC in the interaction layer
(via `level_tool.snap_to_ohlc`). Previously, the raw cursor price was
sent to the app layer, which re-snapped it. After Phase 4 deletes the
app-layer snap, DoubleClick must snap in-line to avoid a precision
regression. Net effect: identical behavior (OHLC snap on double-click),
just performed in the interaction layer instead of the app layer.

The widget sync guard for level data is broadened from "skip during
drag only" to "skip while tool is active" (`!is_active()`). This
means external level changes (e.g., from config reload) won't sync
to the widget until placement or drag completes. Acceptable — the
user is actively interacting and won't notice a sub-second delay.

## 1. Motivation

The horizontal level tool's implementation is scattered across six files and
two crates. Placement mode, drag mode, OHLC snap logic, and alt-key state
are all threaded through the general-purpose interaction state machine,
the chart compute pipeline, the widget sync layer, and the app update loop.
This makes the level tool hard to reason about, hard to test in isolation,
and a source of subtle bugs (stale snapped prices, placement mode re-entry
races, duplicated snap logic that can drift out of sync).

This document proposes extracting all level-tool concerns into a single
`LevelTool` struct that owns its own mode, snap state, and event handling.

## 2. Current Functionality (preserved exactly)

All of the following behaviors are preserved unchanged by this refactor:

### 2.1 Level Placement

- **Activation:** `H` hotkey (interaction.rs line 764-768) or drawing panel
  button (`DrawingPanelCreateLevel` message, app.rs line 1201-1206).
- **Preview line:** Crosshair follows cursor Y. In placement mode the
  crosshair is visible without holding the mouse button (interaction.rs
  line 221-232).
- **OHLC snap:** The preview line's Y snaps to the nearest OHLC value of
  candles within +/-1 of the cursor's X position. Snap threshold is
  `max(candle_width_px, 15).min(40)` pixels (compute.rs line 1091).
  Search radius is the nearest candle index +/- 1 (compute.rs line 1083-1087).
- **Alt disables snap:** Holding Alt during placement stores
  `placing_alt_held = true` on ChartState (interaction.rs line 222), which
  causes compute.rs to skip the snap call (compute.rs line 230).
- **Click to place:** Left-click creates a level at the snapped price (or
  raw cursor price if Alt held). Uses `level_preview_snapped_price` from
  ChartState, which was written by the compute layer via Cell feedback
  (interaction.rs line 486-496, chart_widget.rs line 420, line 243).
- **Cancel:** Escape key (interaction.rs line 747-755) or right-click
  (handled by entering RightPanning, then widget sync re-enters PlacingLevel
  or not based on snapshot, chart_widget.rs line 171-185).
- **Default appearance:** New levels get color `[0.85, 0.85, 0.85, 0.8]`,
  line_width 1.0, no label, no icon, unlocked.

### 2.2 Level Dragging

- **Initiation:** Left-click near a non-locked level (within 6px hit
  tolerance), drag past 4px threshold. Resolved in `PendingDrag` ->
  `DraggingLevel` transition (interaction.rs line 268-278).
- **Grab offset:** The price difference between level and cursor at grab
  time is preserved so the level does not jump to the cursor (interaction.rs
  line 339).
- **OHLC snap during drag:** The app layer snaps the dragged price via
  `snap_price_to_ohlc` (app.rs line 942-948). Alt disables snap
  (reads `placing_alt_held` from chart_state).
- **Alt tracked during drag:** `placing_alt_held` is updated on each
  `MouseMoved` in `DraggingLevel` mode (interaction.rs line 337).
- **Cursor:** `ResizingVertically` cursor shown on hover over non-locked
  levels and during drag (chart_widget.rs line 443-449, 478-489).
- **Local application:** The widget applies `DragLevel` locally to avoid
  visual lag during the message round-trip (chart_widget.rs line 271-275).
- **Level sync guard:** During `DraggingLevel`, the widget does NOT
  overwrite levels from the snapshot (chart_widget.rs line 156-158).

### 2.3 Pan/Scale During Placement

- **Right-click pans:** While in `PlacingLevel`, right-click temporarily
  enters `RightPanning`. When panning ends, widget sync re-enters
  `PlacingLevel` on the next frame (interaction.rs line 498-503,
  chart_widget.rs line 171-175).
- **Middle-click scales:** Same pattern with `PendingScale` (interaction.rs
  line 505-513).

### 2.4 Level Editing

- **Right-click popup:** Right-click on a level emits `RightClickLevel`,
  opening the editor popup (interaction.rs line 577-578, app.rs line 236).
- **Price input:** Text input with mouse wheel and up/down arrow step
  buttons. Steps are 1c below $200, 5c at $200+ (levels.rs `price_step_for`,
  views.rs line 986-1028).
- **Label, color, thickness, icon, lock, delete:** All in the popup editor
  (views.rs line 962-1060+).

### 2.5 Level Labels Overlay

- **16pt text overlay** positioned at level's Y coordinate, icon char +
  label text, colored badge with dark background (views.rs line 903-954).

### 2.6 Level GPU Rendering

- Full-width `GridLineInstance` for each level.
- Selection glow (wider, brighter line for selected level).
- Drag ghost line (original position shown faintly during drag).
- Preview line during placement (the snapped crosshair horizontal line).
- Computed by `compute_levels()` in compute.rs.

### 2.7 Locked Levels

- Cannot be dragged (locked levels fall through to panning, interaction.rs
  line 272-286).
- Cannot be deleted (guarded in `DeleteSelectedLevel` handler, state.rs
  line 343-348).
- Lock toggled via editor popup.

### 2.8 Config Persistence

- Levels with label, icon, locked fields serialized to TOML via
  `LevelConfig` (midas-core config schema).

## 3. Problems with Current Design

### 3.1 Duplicated snap logic

Two independent OHLC snap implementations exist:

| Function | Location | Used by |
|---|---|---|
| `snap_crosshair_to_ohlc` | compute.rs:1040-1125 | Preview line snap during placement (compute layer) |
| `snap_price_to_ohlc` | app.rs:30-71 | Level creation price, drag price (app layer) |

They have different search strategies:
- `snap_crosshair_to_ohlc` searches nearest_idx +/- 1 candles with
  threshold `max(candle_width_px, 15).min(40)`.
- `snap_price_to_ohlc` searches ALL visible candles with a fixed
  threshold of 20px.

This means the preview line can snap to a price that differs from the
price actually assigned on click, or the drag snap can behave differently
from the placement snap.

### 3.2 Cell feedback loop

The snapped price flows through a `Cell<Option<f64>>` on `ChartWidgetState`:

1. `draw()` calls `compute_chart_scene()`, which computes
   `level_preview_snapped_price` and returns it in `ChartScene`.
2. `draw()` writes it to `state.snapped_price.set(...)` (chart_widget.rs:420).
3. `update()` reads it via `state.snapped_price.get()` and stores it on
   `chart_state.level_preview_snapped_price` (chart_widget.rs:243).
4. `handle_mouse_pressed` reads `level_preview_snapped_price` to determine
   the creation price (interaction.rs:486-488).

This is fragile: the snapped price may be stale (from the previous frame),
and the Cell crosses the `draw()`/`update()` boundary in a way that violates
the unidirectional data flow.

### 3.3 PlacingLevel entangled with InteractionMode

`PlacingLevel` is a variant of `InteractionMode` alongside `Panning`,
`HorizontalScaling`, `DraggingLevel`, etc. This creates awkward
special-casing throughout:

- `handle_mouse_moved` has an early return for `PlacingLevel` before the
  main match (interaction.rs:221-232), then an `unreachable!()` at the
  bottom (interaction.rs:463-466).
- `handle_mouse_pressed` has a separate early return for `PlacingLevel`
  (interaction.rs:483-513).
- `handle_mouse_released` has a special `PlacingLevel` arm that returns
  empty to prevent the mode from resetting to Idle (interaction.rs:705-708).
- `handle_key_pressed` has a PlacingLevel guard on Escape (interaction.rs:
  747-755).

The level tool is not really an "interaction mode" in the same sense as
Panning or Scaling -- it is a persistent tool that temporarily yields to
other modes and resumes.

### 3.4 Scattered alt_held and snapped_price state

Two fields on ChartState exist solely for the level tool:

- `placing_alt_held: bool` (state.rs:154)
- `level_preview_snapped_price: Option<f64>` (state.rs:158)

These have no meaning outside level tool contexts but pollute ChartState.
They must be manually cleared in multiple places:
- interaction.rs line 491-492 (on placement click)
- interaction.rs line 751 (on Escape)
- app.rs line 932-933 (on ChartCreateLevel)
- app.rs line 1008 (on ChartCancelPlacing)
- app.rs line 1416-1417 (on Escape key press)
- chart_widget.rs line 183 (on snapshot sync when placing is false)

Missing a single clear site causes stale state bugs.

### 3.5 Widget sync complexity

The chart_widget.rs `update()` function has a 15-line block (line 164-185)
dedicated to syncing placement mode between the app's snapshot and the
widget's local ChartState. It must handle:

- App says placement active, widget is Idle -> re-enter PlacingLevel
  (after pan/scale completes).
- App says placement inactive, widget is PlacingLevel -> force Idle
  (after CancelPlacing message processed).

This two-way sync is error-prone and hard to follow.

### 3.6 Interaction layer cannot access CandleData

The interaction layer (`handle_event`) only receives `&mut ChartState`.
It has no access to candle data for OHLC snapping. This is why:

- Snap during **placement** is hacked through the compute layer (which
  does have data access) and fed back via Cell.
- Snap during **drag** is done in the app layer's message handler (which
  has `chart.data`).
- The interaction layer itself places levels at the raw cursor price
  (interaction.rs:488) and trusts that the app layer will snap it later.

This split makes it impossible to show a snapped preview during drag in
the interaction layer.

## 4. Proposed Design: LevelTool

### 4.1 New file

`crates/midas-chart/src/level_tool.rs`

Registered in `lib.rs`:
```rust
pub mod level_tool;
pub use level_tool::LevelTool;
```

### 4.2 LevelToolMode enum

```rust
/// The level tool's internal state machine.
#[derive(Clone, Debug, PartialEq)]
pub enum LevelToolMode {
    /// Tool is not active. No preview, no drag.
    Idle,
    /// User activated the tool. Preview line follows cursor Y
    /// (snapped to OHLC unless Alt held). Single click places.
    Placing,
    /// User is dragging an existing level to a new price.
    Dragging {
        /// ID of the level being dragged.
        level_id: u64,
        /// Price offset between level and cursor at grab time
        /// (so the level doesn't jump to the cursor).
        grab_offset: f64,
    },
}
```

### 4.3 LevelTool struct

```rust
/// Self-contained horizontal level tool.
///
/// Owns all state for level placement, dragging, and OHLC snapping.
/// Lives as a field on `ChartState`. The interaction layer delegates
/// level-related event handling to this struct.
#[derive(Clone, Debug)]
pub struct LevelTool {
    /// Current tool mode.
    pub mode: LevelToolMode,
    /// Whether Alt is held (disables OHLC snap).
    pub alt_held: bool,
    /// OHLC-snapped price for the current preview/drag position.
    /// `None` if no snap was computed (no data, or Alt held).
    pub snapped_price: Option<f64>,
    /// Whether the tool was in `Placing` mode before a temporary
    /// pan/scale interruption. When the pan/scale ends and mode
    /// returns to `Idle`, this flag causes automatic re-entry
    /// to `Placing`.
    pub was_placing: bool,
}
```

Default:
```rust
impl Default for LevelTool {
    fn default() -> Self {
        Self {
            mode: LevelToolMode::Idle,
            alt_held: false,
            snapped_price: None,
            was_placing: false,
        }
    }
}
```

### 4.4 Core snap method

A single snap function lives on `LevelTool`, eliminating both
`snap_crosshair_to_ohlc` (compute.rs) and `snap_price_to_ohlc` (app.rs):

```rust
/// Maximum pixel distance (Y-axis) for OHLC snap.
const SNAP_THRESHOLD_MIN_PX: f32 = 15.0;
const SNAP_THRESHOLD_MAX_PX: f32 = 40.0;

impl LevelTool {
    /// Snap a raw price to the nearest OHLC value within threshold.
    ///
    /// Searches candles within +/- `search_radius` of the candle nearest
    /// to `cursor_x`. Returns the snapped price, or `raw_price` if no
    /// OHLC value is close enough.
    ///
    /// Also updates `self.snapped_price` as a side effect.
    pub fn snap_to_ohlc(
        &mut self,
        raw_price: f64,
        cursor_x: f32,
        camera: &Camera2D,
        data: &dyn CandleData,
        is_collapsed: bool,
    ) -> f64 {
        if self.alt_held || data.is_empty() {
            self.snapped_price = None;
            return raw_price;
        }

        let cursor_y = camera.price_to_y(raw_price);
        let len = data.len();

        // Find nearest candle index to cursor X.
        let nearest_idx = if is_collapsed {
            let idx_f = camera.x_to_time(cursor_x);
            (idx_f.round() as isize).clamp(0, len as isize - 1) as usize
        } else {
            let cursor_time = camera.x_to_time(cursor_x);
            data.find_index_by_time(cursor_time as i64)
        };

        // Search radius: nearest candle +/- 1.
        let search_start = nearest_idx.saturating_sub(1);
        let search_end = (nearest_idx + 2).min(len);

        // Adaptive snap threshold based on candle density.
        let visible_candles = if is_collapsed {
            (camera.time_end - camera.time_start).max(1.0)
        } else {
            let vis_start = data.find_index_by_time(camera.time_start as i64);
            let vis_end = (data.find_index_by_time(camera.time_end as i64) + 1)
                .min(len);
            (vis_end.saturating_sub(vis_start)).max(1) as f64
        };
        let candle_width_px = camera.viewport_width as f64 / visible_candles;
        let snap_threshold_px = (candle_width_px as f32)
            .max(SNAP_THRESHOLD_MIN_PX)
            .min(SNAP_THRESHOLD_MAX_PX);

        let mut best_price = raw_price;
        let mut best_dist = f32::MAX;

        for i in search_start..search_end {
            for &p in &[
                data.open(i) as f64,
                data.high(i) as f64,
                data.low(i) as f64,
                data.close(i) as f64,
            ] {
                let py = camera.price_to_y(p);
                let dist = (py - cursor_y).abs();
                if dist < best_dist {
                    best_dist = dist;
                    best_price = p;
                }
            }
        }

        if best_dist <= snap_threshold_px {
            self.snapped_price = Some(best_price);
            best_price
        } else {
            self.snapped_price = None;
            raw_price
        }
    }

    /// Clear all tool state, returning to Idle.
    pub fn cancel(&mut self) {
        self.mode = LevelToolMode::Idle;
        self.alt_held = false;
        self.snapped_price = None;
        self.was_placing = false;
    }

    /// Activate level placement mode.
    /// No-op if currently dragging (prevents H-key during active drag).
    pub fn activate(&mut self) {
        if self.is_dragging() {
            return;
        }
        self.mode = LevelToolMode::Placing;
        self.alt_held = false;
        self.snapped_price = None;
        self.was_placing = false;
    }

    /// Returns true if the tool is in Placing or Dragging mode.
    pub fn is_active(&self) -> bool {
        !matches!(self.mode, LevelToolMode::Idle)
    }

    /// Returns true if in Placing mode.
    pub fn is_placing(&self) -> bool {
        matches!(self.mode, LevelToolMode::Placing)
    }

    /// Returns true if in Dragging mode.
    pub fn is_dragging(&self) -> bool {
        matches!(self.mode, LevelToolMode::Dragging { .. })
    }

    /// Temporarily suspend Placing for a pan/scale operation.
    /// Sets `was_placing` so the tool can resume after.
    pub fn suspend_placing(&mut self) {
        if matches!(self.mode, LevelToolMode::Placing) {
            self.was_placing = true;
            self.mode = LevelToolMode::Idle;
        }
    }

    /// If the tool was suspended from Placing, resume it.
    /// Called when a pan/scale operation ends.
    pub fn try_resume_placing(&mut self) {
        if self.was_placing && matches!(self.mode, LevelToolMode::Idle) {
            self.mode = LevelToolMode::Placing;
            self.was_placing = false;
        }
    }
}
```

### 4.5 Updated handle_event signature

The `handle_event` function (and its private helpers) gains an optional
data parameter so the level tool can snap in-line:

```rust
/// Process a chart event and return zero or more actions.
///
/// `data` is needed for OHLC snap during level placement and drag.
/// Pass `None` if no candle data is loaded.
pub fn handle_event(
    state: &mut ChartState,
    event: ChartEvent,
    data: Option<&dyn CandleData>,
    is_collapsed: bool,
) -> Vec<ChartAction> { ... }
```

This is a breaking change to the public API of `midas-chart`. The call
site in chart_widget.rs already has `data` available (from
`self.snapshot.data`), so wiring it through is trivial.

### 4.6 LevelTool as a field on ChartState

```rust
// In state.rs:
pub struct ChartState {
    // ... existing fields ...

    /// Self-contained horizontal level tool.
    pub level_tool: LevelTool,

    // REMOVED:
    // pub placing_alt_held: bool,
    // pub level_preview_snapped_price: Option<f64>,
}
```

### 4.7 Updated ChartInput

```rust
// In input.rs:
pub struct ChartInput<'a> {
    // ... existing fields ...

    /// Reference to the level tool for placement preview state.
    pub level_tool: &'a LevelTool,

    // REMOVED:
    // pub placing_level: bool,
    // pub placing_alt_held: bool,
}
```

The compute layer reads `input.level_tool.is_placing()` and
`input.level_tool.alt_held` instead of the old booleans.

### 4.8 Remove PlacingLevel and DraggingLevel from InteractionMode

```rust
// In state.rs -- AFTER refactor:
pub enum InteractionMode {
    Idle,
    PendingDrag { start_x: f32, start_y: f32 },
    Panning,
    // DraggingLevel REMOVED -- now in LevelTool
    PendingScale { start_x: f32, start_y: f32 },
    HorizontalScaling { anchor_x: f32, last_x: f32 },
    VerticalScaling { anchor_y: f32, last_y: f32 },
    RightPanning,
    DraggingTimelineBorder { anchor_y: f32, start_ratio: f32 },
    DraggingVolumeScale { anchor_y: f32, start_scale: f32 },
    // PlacingLevel REMOVED -- now in LevelTool
}
```

The level tool's mode and the interaction state machine's mode are now
orthogonal. The level tool can be `Placing` while the interaction mode is
`RightPanning` (right-click pan during placement), which is exactly the
existing behavior but modeled explicitly rather than through widget sync
hacks.

### 4.8.1 State Transition Table

Legal `(InteractionMode, LevelToolMode)` combinations:

| InteractionMode | LevelTool::Idle | LevelTool::Placing | LevelTool::Dragging |
|---|---|---|---|
| **Idle** | Normal (default) | Active placement | Never (drag needs mouse) |
| **PendingDrag** | Normal click | Illegal* | Never |
| **Panning** | Normal pan | Suspended (was_placing=true) | Never |
| **RightPanning** | Normal pan | Suspended (was_placing=true) | Never |
| **PendingScale** | Normal | Suspended (was_placing=true) | Never |
| **H/V Scaling** | Normal | Suspended (was_placing=true) | Never |
| **DragTimeline** | Normal | Suspended | Never |
| **DragVolume** | Normal | Suspended | Never |

*PendingDrag + Placing is illegal: entering PendingDrag from Placing
means a left-click, which places the level and returns to Idle.

When `level_tool.mode == Dragging`:
- `interaction_mode` MUST be `Idle`. The drag is driven entirely by
  the level tool. Right-click during drag is not supported (drag ends
  on any release).

**Invariant**: `level_tool.is_dragging()` and `interaction_mode != Idle`
must never co-exist. This MUST be enforced with a `debug_assert!` at
the top of `handle_event_inner`:
```rust
debug_assert!(
    !(state.level_tool.is_dragging() && state.interaction_mode != InteractionMode::Idle),
    "invariant violated: LevelTool::Dragging requires InteractionMode::Idle"
);
```

**Dispatch rule**: `handle_mouse_moved` checks `level_tool.is_active()`
as an **early return** before the `match state.interaction_mode` block,
identical to the current PlacingLevel pattern. This ensures the level
tool handles mouse moves without falling through to the interaction
state machine.

**Guard**: `level_tool.activate()` is a no-op if `is_dragging()` is true.
This prevents H-key activation during an active drag.

### 4.9 Crosshair snapped price on ChartScene

`ChartScene.level_preview_snapped_price` is **removed**. The snapped price
now lives on `LevelTool.snapped_price`, which the interaction layer reads
directly when the user clicks to place. No Cell feedback loop needed.

## 5. Detailed Change Inventory

### 5.1 Files created

| File | Contents |
|---|---|
| `midas-chart/src/level_tool.rs` | `LevelTool`, `LevelToolMode`, snap logic, unit tests |

### 5.2 Files modified

| File | Changes |
|---|---|
| `midas-chart/src/lib.rs` | Add `pub mod level_tool; pub use level_tool::LevelTool;` |
| `midas-chart/src/state.rs` | Add `level_tool: LevelTool` field. Phase 2: mark `placing_alt_held`, `level_preview_snapped_price`, `PlacingLevel`, `DraggingLevel` as `#[deprecated]`. Phase 4: remove them. Update `apply_action` for `EnterPlacingLevel` / `CancelPlacing` to delegate to `level_tool`. |
| `midas-chart/src/interaction.rs` | Refactor `handle_event` signature, delegate level logic to `LevelTool`, remove `PlacingLevel`/`DraggingLevel` branches from state machine, remove `ChartAction::EnterPlacingLevel`/`CancelPlacing` (or keep them but implement via `level_tool`) |
| `midas-chart/src/input.rs` | Replace `placing_level: bool` + `placing_alt_held: bool` with `level_tool: &'a LevelTool` |
| `midas-chart/src/compute.rs` | Delete `snap_crosshair_to_ohlc` function (lines 1040-1125). Replace all `input.placing_level` / `input.placing_alt_held` reads with `input.level_tool.is_placing()` / `input.level_tool.alt_held`. Snap call replaced by reading `input.level_tool.snapped_price`. Remove `level_preview_snapped_price` from both `compute_normal_scene` and `compute_collapsed_scene`. |
| `midas-chart/src/scene.rs` | Remove `level_preview_snapped_price: Option<f64>` from `ChartScene` |
| `midas-app/src/chart_widget.rs` | Phase 3: update `ChartInput` construction to pass `level_tool` reference. Phase 4: remove `snapped_price: Cell<Option<f64>>` from `ChartWidgetState`, remove Cell write/read, remove placement sync block, update `handle_event` call site with new signature, replace `snapshot.placing_level` / `snapshot.placing_alt_held` with `snapshot.level_tool`, simplify level sync guard to `level_tool.is_active()`. |
| `midas-app/src/app.rs` | Delete `snap_price_to_ohlc` function (lines 30-71). Delete `OHLC_SNAP_THRESHOLD_PX` constant. `ChartCreateLevel` handler no longer snaps (already snapped). `ChartDragLevel` handler no longer snaps (already snapped). Simplify `ChartCancelPlacing` handler to call `chart.chart_state.level_tool.cancel()`. Simplify `DrawingPanelCreateLevel` to call `chart.chart_state.level_tool.activate()`. Simplify `handle_key_press` H/Escape handlers. |
| `midas-app/src/app/views.rs` | Replace `matches!(interaction_mode, PlacingLevel)` checks with `chart.chart_state.level_tool.is_placing()`. Update `ChartRenderSnapshot` construction. |

### 5.3 Functions deleted

| Function | File | Lines | Reason |
|---|---|---|---|
| `snap_crosshair_to_ohlc` | compute.rs | 1040-1125 | Replaced by `LevelTool::snap_to_ohlc` |
| `snap_price_to_ohlc` | app.rs | 30-71 | Replaced by `LevelTool::snap_to_ohlc` |

### 5.4 Fields deleted

| Field | Struct | File |
|---|---|---|
| `placing_alt_held: bool` | ChartState | state.rs:154 |
| `level_preview_snapped_price: Option<f64>` | ChartState | state.rs:158 |
| `level_preview_snapped_price: Option<f64>` | ChartScene | scene.rs:62 |
| `snapped_price: Cell<Option<f64>>` | ChartWidgetState | chart_widget.rs:122 |
| `placing_level: bool` | ChartInput | input.rs:56 |
| `placing_alt_held: bool` | ChartInput | input.rs:58 |
| `placing_level: bool` | ChartRenderSnapshot | chart_widget.rs:85 |
| `placing_alt_held: bool` | ChartRenderSnapshot | chart_widget.rs:87 |

### 5.5 Fields added

| Field | Struct | File |
|---|---|---|
| `level_tool: LevelTool` | ChartState | state.rs |
| `level_tool: &'a LevelTool` | ChartInput | input.rs |
| `level_tool: LevelTool` | ChartRenderSnapshot | chart_widget.rs |

### 5.6 Enum variants removed from InteractionMode

- `PlacingLevel` (state.rs:66)
- `DraggingLevel { level_id: u64, grab_offset: f64 }` (state.rs:34-39)

### 5.7 ChartAction changes

| Variant | Change |
|---|---|
| `EnterPlacingLevel` | Re-bodied in Phase 2 to delegate to `level_tool.activate()`. Removed entirely in Phase 4 (no code emits it after Phase 2 rewrites callers). |
| `CancelPlacing` | Kept, but the handler now delegates to `level_tool.cancel()`. |
| `DragLevel` | The `new_price` field now carries the already-snapped price (no post-snap in app layer). |
| `CreateLevel` | The `price` field now carries the already-snapped price. |

### 5.8 ChartEvent changes

| Variant | Change |
|---|---|
| `ActivateLevelTool` | Handler body changes from `state.interaction_mode = InteractionMode::PlacingLevel` to `state.level_tool.activate()`. Variant itself is kept. |

## 6. Interaction Flow After Refactor

### 6.1 Level placement flow

```
User presses H  (or clicks Level button)
  -> interaction.rs: state.level_tool.activate()
  -> LevelTool.mode = Placing

User moves mouse
  -> handle_mouse_moved: level_tool.is_placing() -> true
  -> level_tool.snap_to_ohlc(raw_price, cursor_x, camera, data, is_collapsed)
  -> Updates level_tool.snapped_price
  -> Emit SetCrosshair with snapped Y position
  -> Crosshair renders at snapped position

User left-clicks
  -> handle_mouse_pressed: level_tool.is_placing() -> true
  -> price = level_tool.snapped_price.unwrap_or(raw_price)
  -> level_tool.cancel()
  -> Emit CreateLevel { price }

User right-clicks during placement
  -> level_tool.suspend_placing()  (was_placing = true, mode = Idle)
  -> interaction_mode = RightPanning
  ...panning...
  -> Right mouse released -> interaction_mode = Idle
  -> level_tool.try_resume_placing()  (mode = Placing)

User presses Escape
  -> level_tool.cancel()
  -> Emit CancelPlacing + ClearCrosshair
```

### 6.2 Level drag flow

```
User left-presses near a level
  -> interaction_mode = PendingDrag { start_x, start_y }
  -> Emit SelectLevel { id: level_id }

Mouse moves past 4px threshold, hit_test_levels() matches
  -> level not locked
  -> state.level_tool.mode = Dragging { level_id, grab_offset }
  -> interaction_mode = Idle  (tool drives the drag, not interaction_mode)

Mouse moves during drag (early return in handle_mouse_moved)
  -> level_tool.is_active() check fires before match interaction_mode
  -> level_tool.alt_held = alt_held
  -> raw_price = camera.y_to_price(y) + grab_offset
  -> snapped = level_tool.snap_to_ohlc(raw_price, cursor_x, ...)
  -> Emit DragLevel { id: level_id, new_price: snapped }

Mouse released (left button)
  -> level_tool.mode = Idle
  -> No snap needed in app layer -- price is already snapped
  -> level_tool.try_resume_placing() (no-op since was_placing is false)
```

### 6.3 Crosshair snap during placement (compute layer)

After the refactor, the compute layer no longer performs OHLC snap for the
crosshair. Instead:

1. `handle_mouse_moved` calls `level_tool.snap_to_ohlc()` and gets the
   snapped price.
2. It computes the snapped Y: `camera.price_to_y(snapped_price)`.
3. It emits `SetCrosshair { x, y: snapped_y }` with the already-snapped Y.
4. The compute layer renders the crosshair at the position it receives --
   no additional snap logic needed.
5. `ChartScene.level_preview_snapped_price` is deleted.

This eliminates the Cell feedback loop entirely.

## 7. ChartRenderSnapshot Changes

```rust
// BEFORE:
pub struct ChartRenderSnapshot {
    // ...
    pub placing_level: bool,
    pub placing_alt_held: bool,
}

// AFTER:
pub struct ChartRenderSnapshot {
    // ...
    pub level_tool: LevelTool,  // Clone of the tool state
}
```

Snapshot construction in views.rs changes from:
```rust
placing_level: matches!(chart.chart_state.interaction_mode,
                        midas_chart::InteractionMode::PlacingLevel),
placing_alt_held: chart.chart_state.placing_alt_held,
```
to:
```rust
level_tool: chart.chart_state.level_tool.clone(),
```

## 8. Widget Sync Simplification

### 8.1 Before (chart_widget.rs update(), lines 156-185)

```rust
// Only sync levels from snapshot when NOT actively dragging a level.
if !matches!(chart_state.interaction_mode, InteractionMode::DraggingLevel { .. }) {
    chart_state.levels = self.snapshot.levels.clone();
}

// Sync placement mode between app and widget. [15 lines of logic]
if self.snapshot.placing_level {
    if matches!(chart_state.interaction_mode, InteractionMode::Idle) {
        chart_state.interaction_mode = InteractionMode::PlacingLevel;
    }
} else {
    if matches!(chart_state.interaction_mode, InteractionMode::PlacingLevel) {
        chart_state.interaction_mode = InteractionMode::Idle;
        chart_state.crosshair_pos = None;
        chart_state.placing_alt_held = false;
    }
}
```

### 8.2 After

```rust
// Sync levels from snapshot unless the tool is actively handling them.
if !chart_state.level_tool.is_active() {
    chart_state.levels = self.snapshot.levels.clone();
}

// Sync level tool from snapshot — but only when the tool is idle.
// During Placing or Dragging, the widget's local tool state is
// authoritative (it has the live snapped_price, alt_held, etc.).
// Overwriting from a stale snapshot would kill the active operation.
if !chart_state.level_tool.is_active() {
    chart_state.level_tool = self.snapshot.level_tool.clone();
}
```

The `was_placing` flag on `LevelTool` handles the resume-after-pan case
internally. When the widget's interaction mode returns to Idle after
a right-click pan, `level_tool.try_resume_placing()` is called in the
mouse release handler, which re-enters Placing if `was_placing` is set.

The Cell feedback (`snapped_price: Cell<Option<f64>>`) is deleted entirely.

## 9. Migration Phases

### Phase 1: Create LevelTool struct + snap function + tests

**Files touched:** `level_tool.rs` (new), `lib.rs`

1. Create `crates/midas-chart/src/level_tool.rs` with `LevelToolMode`,
   `LevelTool`, and `snap_to_ohlc`.
2. Register in `lib.rs`.
3. Write unit tests for:
   - `snap_to_ohlc` finds nearest OHLC within threshold.
   - `snap_to_ohlc` returns raw price when beyond threshold.
   - `snap_to_ohlc` returns raw price when `alt_held` is true.
   - `snap_to_ohlc` returns raw price when data is empty.
   - `activate()` / `cancel()` transitions.
   - `suspend_placing()` / `try_resume_placing()` round-trip.
   - `is_placing()`, `is_dragging()`, `is_active()` predicates.

**Test command:** `cargo test -p midas-chart level_tool`

This phase has zero impact on existing code. The new module exists but
is not wired in.

### Phase 2: Wire into interaction.rs (keep old stubs for compilation)

**Files touched:** `state.rs`, `interaction.rs`

**Key principle:** Do NOT remove `PlacingLevel`/`DraggingLevel` from
`InteractionMode`, and do NOT remove `placing_alt_held` or
`level_preview_snapped_price` from `ChartState` in this phase. Mark
them all `#[deprecated]` and keep them as dead code so that compute.rs,
chart_widget.rs, and app.rs (which still reference them) continue to
compile. They are removed in Phase 4.

Do NOT change `handle_event`'s public signature in this phase. Instead,
add an internal helper that accepts `data` and `is_collapsed`, and have
the public `handle_event` call it with `data = None` and
`is_collapsed = state.collapse_gaps`. The signature change happens in
Phase 4 when the call site in chart_widget.rs is also updated.

1. Add `level_tool: LevelTool` field to `ChartState::new()`.
2. Mark `placing_alt_held` and `level_preview_snapped_price` as
   `#[deprecated]` on `ChartState`. They are NOT removed yet — app code
   still reads them. Phase 4 removes them.
3. Add an internal helper with the new signature:
   ```rust
   fn handle_event_inner(
       state: &mut ChartState,
       event: ChartEvent,
       data: Option<&dyn CandleData>,
       is_collapsed: bool,
   ) -> Vec<ChartAction>
   ```
   The existing public `handle_event` delegates to it:
   ```rust
   pub fn handle_event(state: &mut ChartState, event: ChartEvent) -> Vec<ChartAction> {
       let is_collapsed = state.collapse_gaps;
       handle_event_inner(state, event, None, is_collapsed)
   }
   ```
   This preserves the existing call site in chart_widget.rs while allowing
   the interaction logic to use data for snapping. The public signature
   change (adding `data` and `is_collapsed` params) happens in Phase 4.
4. Refactor `handle_mouse_moved` (inside `handle_event_inner`):
   - Replace `InteractionMode::PlacingLevel` early-return check with
     `state.level_tool.is_placing()`. The early return pattern stays
     identical — `level_tool.is_active()` is checked BEFORE the
     `match state.interaction_mode` block.
   - Call `state.level_tool.snap_to_ohlc(raw_price, cursor_x, camera,
     data, is_collapsed)` inside the handler (if `data` is `Some`).
     Emit `SetCrosshair` with the snapped Y:
     `camera.price_to_y(snapped_price)`.
   - **Do NOT refactor the `DraggingLevel` branch in this phase.** Leave
     the existing `InteractionMode::DraggingLevel` handling unchanged.
     Drag still uses the old path (interaction layer emits unsnapped
     price, app layer re-snaps via `snap_price_to_ohlc`). The drag
     refactoring moves to Phase 4 where `handle_event` receives real
     `data` and `snap_to_ohlc` can actually snap. This avoids visual
     jitter from emitting unsnapped `DragLevel` prices during Phase 2.
5. Refactor `handle_mouse_pressed` (inside `handle_event_inner`):
   - Replace `InteractionMode::PlacingLevel` check with
     `state.level_tool.is_placing()`.
   - Read price on left-click with a bridging fallback to the old
     Cell-fed field (which compute.rs still writes during Phase 2):
     `level_tool.snapped_price
         .or(state.level_preview_snapped_price)
         .unwrap_or(raw_price)`
     This ensures snap-on-click works during Phase 2 even though
     `handle_event_inner` receives `data = None` (so `snap_to_ohlc`
     is not called). The `.or(state.level_preview_snapped_price)`
     fallback is removed in Phase 4 when the old field is deleted.
   - Call `state.level_tool.suspend_placing()` on right/middle-click.
   - **Do NOT touch the PendingDrag→level-hit transition in this phase.**
     It continues to set `InteractionMode::DraggingLevel { ... }` as
     before. The old `DraggingLevel` branch at interaction.rs line 337
     continues to write `placing_alt_held`, which the app layer reads
     for drag snap. Do not remove this write until Phase 4.
   - **Preserve `SelectLevel { id: level_id }` action** when entering drag.
6. Refactor `handle_mouse_released` (inside `handle_event_inner`):
   - **Do NOT touch the `DraggingLevel` mouse-up handler in this phase.**
     The existing left-release arm for `InteractionMode::DraggingLevel`
     remains unchanged. Drag migration is in Phase 4 step 3a.
   - Call `state.level_tool.try_resume_placing()` **inside** the
     right-button-release block (after setting mode to Idle, before the
     early return) AND inside the middle-button-release block (same
     pattern). These are the exact insertion points — placing the call
     after the early returns would never execute it.
     Concrete example for the right-button-release block:
     ```rust
     // Right mouse released:
     if matches!(state.interaction_mode, InteractionMode::RightPanning) {
         state.interaction_mode = InteractionMode::Idle;
         state.level_tool.try_resume_placing();  // <-- INSERT HERE
         return vec![];
     }
     ```
     Apply the same pattern for the middle-button-release block (after
     `state.interaction_mode = InteractionMode::Idle;`, before `return`).
   - Remove the `PlacingLevel` match arm from the left-release block.
7. Refactor `handle_key_pressed` (inside `handle_event_inner`):
   - Escape: call `state.level_tool.cancel()`.
   - H: call `state.level_tool.activate()`.
8. Update `apply_action` for `EnterPlacingLevel` -> `state.level_tool.activate()`,
   `CancelPlacing` -> `state.level_tool.cancel()`.
   Also update the `ChartEvent::ActivateLevelTool` handler in
   `handle_event_inner`: change its body from
   `state.interaction_mode = InteractionMode::PlacingLevel` to
   `state.level_tool.activate()`.
9. Mark `PlacingLevel` and `DraggingLevel` variants as `#[deprecated]`
   with a note: "Moved to LevelTool — will be removed in Phase 4."
   Keep them as match arms that are unreachable but compile.
10. Update all existing interaction tests, add new level-tool-specific tests.
11. Add an integration test that calls `handle_event_inner` directly with
    mock `CandleData` and `data = Some(...)` to validate the new snap path
    end-to-end before Phase 3/4 delete the old snap implementations:
    ```
    test handle_event_inner_placement_snap_with_data
    test handle_event_inner_placement_alt_held_with_data
    ```

**Test command:** `cargo test --workspace`

### Phase 3: Wire into compute.rs, scene.rs, and promote handle_event signature

**Files touched:** `input.rs`, `compute.rs`, `scene.rs`, `chart_widget.rs`, `interaction.rs`

**Note:** This phase also updates the `ChartInput` construction site in
`chart_widget.rs` `draw()` since `ChartInput` fields change. Critically,
the `handle_event` public signature is promoted here (moved forward from
Phase 4) so the interaction layer has real `data` access BEFORE the
compute-layer snap is deleted. Without this ordering, there would be a
window where no component performs OHLC snap during placement.

1. **Promote `handle_event_inner` to public `handle_event`** in
   interaction.rs. Delete the old wrapper. The signature is now:
   ```rust
   pub fn handle_event(
       state: &mut ChartState,
       event: ChartEvent,
       data: Option<&dyn CandleData>,
       is_collapsed: bool,
   ) -> Vec<ChartAction>
   ```
2. **Update `chart_widget.rs` `update()`** — change the `handle_event`
   call site to use the new public signature:
   `handle_event(chart_state, chart_event, data, is_collapsed)`.
   Pass `data` from `self.snapshot.data.as_ref().map(|d| d.as_ref()
   as &dyn CandleData)` and `is_collapsed` from
   `self.snapshot.collapse_gaps`. The interaction layer now receives
   real data and `snap_to_ohlc` works during placement.
3. Replace `placing_level` and `placing_alt_held` fields on `ChartInput`
   with `level_tool: &'a LevelTool`.
4. In `compute_normal_scene` and `compute_collapsed_scene`:
   - Remove the `snap_crosshair_to_ohlc` call blocks.
   - The crosshair Y is already snapped by the interaction layer
     (which now has data access via step 1-2).
   - If `level_tool.is_placing()`, read `level_tool.snapped_price` to
     adjust the crosshair's `priceline_label.text` and `priceline_label.screen_y`.
   - Remove `level_preview_snapped_price` from `ChartScene` construction.
5. Delete `snap_crosshair_to_ohlc` function from compute.rs.
6. Remove `level_preview_snapped_price` from `ChartScene` struct.
7. Remove the Cell write in `chart_widget.rs` `draw()` (line 420):
   `state.snapped_price.set(scene.level_preview_snapped_price)`.
   This line references the now-deleted `ChartScene` field. The Cell
   field itself and the Cell read (line 243) can remain until Phase 4
   since they reference `ChartState` (still has the deprecated field).
   Add `#[allow(deprecated)]` on the Cell read line to suppress the
   deprecation warning between Phase 3 and Phase 4.
8. Update the `ChartInput` construction in `chart_widget.rs` `draw()`:
   replace `placing_level: ...` and `placing_alt_held: ...` with
   `level_tool: &chart_state.level_tool` (reading from widget's live
   state, guarded by `is_active()` as in Section 8.2).
9. Update all compute tests and `integration_gate.rs` that construct
   `ChartInput` to pass `level_tool: &LevelTool::default()` instead
   of the two booleans.

**Test command:** `cargo test --workspace`

### Phase 4: Simplify app layer + complete drag refactoring

**Files touched:** `chart_widget.rs`, `app.rs`, `app/views.rs`, `state.rs`, `interaction.rs`

**Note:** This phase completes the drag refactoring that was deferred
from Phase 2 (the `DraggingLevel` branch on `InteractionMode` is migrated
to `LevelTool` here). The `handle_event` signature was already promoted
in Phase 3, so the interaction layer has real `data` for snap.

1. Delete `snap_price_to_ohlc` and `OHLC_SNAP_THRESHOLD_PX` from app.rs.
2. Simplify `Message::ChartCreateLevel` handler:
   - Remove snap call. The price from the interaction layer is already snapped.
   - Remove references to deprecated `placing_alt_held` /
     `level_preview_snapped_price`.
   - Call `chart.chart_state.level_tool.cancel()` (already done by
     interaction layer, but defensive).
3. Simplify `Message::ChartDragLevel` handler:
   - Remove snap call. Price is already snapped.
4. **Refactor drag path in interaction.rs** (deferred from Phase 2):
   - Replace the `InteractionMode::DraggingLevel` handling in
     `handle_mouse_moved` with `state.level_tool.is_dragging()` (in
     the early-return block, same pattern as placement).
   - Call `snap_to_ohlc` during drag and emit `DragLevel` with the
     snapped price. Works correctly because `handle_event` receives
     real `data` from the chart_widget.rs call site (Phase 3 step 2).
   - In `handle_mouse_pressed`, PendingDrag→level-hit transition: set
     `state.level_tool.mode = LevelToolMode::Dragging { ... }` and
     `state.interaction_mode = InteractionMode::Idle`.
     At this transition, also clear crosshair state:
     `state.crosshair_pos = None` and emit `ClearCrosshair`. Without
     this, a stale crosshair from the PendingDrag phase would remain
     visible during drag (the drag early-return block fires before
     the crosshair suppression code at interaction.rs lines 237-242).
   - In `handle_mouse_released`, when `level_tool.is_dragging()`: set
     `state.level_tool.mode = LevelToolMode::Idle` on left mouse-up.
5. **Add OHLC snap to `DoubleClick` handler** in interaction.rs. The
   current handler (line 172-175) creates a level at raw cursor price.
   Previously the app layer re-snapped via `snap_price_to_ohlc`
   (removed in step 1). Now snap in-line:
   ```rust
   ChartEvent::DoubleClick { x, y, .. } => {
       let raw_price = state.camera.y_to_price(y);
       let price = if let Some(data) = data {
           state.level_tool.snap_to_ohlc(
               raw_price, x, &state.camera, data, is_collapsed)
       } else {
           raw_price
       };
       vec![ChartAction::CreateLevel { price }]
   }
   ```
6. Simplify `Message::ChartCancelPlacing` handler:
   - Just call `chart.chart_state.level_tool.cancel()`.
7. Simplify `Message::DrawingPanelCreateLevel` handler:
   - Just call `chart.chart_state.level_tool.activate()`.
8. Simplify `handle_key_press` H/Escape handlers:
   - H: `chart.chart_state.level_tool.activate()`.
   - Escape: `chart.chart_state.level_tool.cancel()`.
9. Update `ChartRenderSnapshot` struct:
   - Replace `placing_level: bool` and `placing_alt_held: bool` with
     `level_tool: LevelTool`.
10. Update all snapshot construction sites (main pane, floating windows).
11. Update `ChartWidgetState`:
    - Remove `snapped_price: Cell<Option<f64>>`.
12. Update `chart_widget.rs` `update()`:
    - Remove Cell read (`chart_state.level_preview_snapped_price = ...`).
    - Remove 15-line placement sync block.
    - Replace with guarded sync (Section 8.2): only sync `level_tool`
      and `levels` from snapshot when `!level_tool.is_active()`.
    - Add `chart_state.level_tool.cancel()` to the viewport resize
      handler block (lines 198-204) alongside the existing
      `interaction_mode = Idle` reset. Without this, a resize during
      active placement would leave `level_tool.mode == Placing` with
      `crosshair_pos == None`.
13. Update `chart_widget.rs` `draw()`:
    - Cell write already removed in Phase 3 step 7.
    - Remove the `level_preview_snapped_price` fallback from the
      placement click handler (the `.or(state.level_preview_snapped_price)`
      bridging code added in Phase 2 step 5 — no longer needed since
      `handle_event` now receives `data` directly).
    - Update the levels source guard (line 395): replace
      `matches!(cs.interaction_mode, InteractionMode::DraggingLevel { .. })`
      with `cs.level_tool.is_dragging()`. Without this, the widget would
      use snapshot levels instead of live widget-local levels during drag,
      causing visible per-frame jitter.
14. Update `chart_widget.rs` `mouse_interaction()`:
    - Replace `InteractionMode::DraggingLevel` check with
      `level_tool.is_dragging()`.
15. Update `ChartPrimitive`:
    - Keep the field name `placing_level: bool`, just change the data
      source from `snap.placing_level` to `level_tool.is_placing()`.
16. Update views.rs:
    - Replace `matches!(interaction_mode, PlacingLevel)` with
      `chart.chart_state.level_tool.is_placing()`.
17. **Remove `#[deprecated]` items from `ChartState` and `InteractionMode`**
    (state.rs):
    - Remove `placing_alt_held` and `level_preview_snapped_price` fields.
    - Remove `PlacingLevel` and `DraggingLevel` enum variants.
    - Remove `EnterPlacingLevel` from `ChartAction` (no code emits it
      after Phase 2 rewrote all callers to use `level_tool.activate()`).
    - Remove any dead match arms kept for compilation.

**Test command:** `cargo test --workspace`

### Phase 5: Cleanup and final verification

1. All scattered fields should already be gone (Phase 4 removed the
   deprecated stubs from `ChartState` and `InteractionMode`).
2. `cargo clippy --workspace -- -D warnings` -- fix any dead code warnings.
3. `cargo test --workspace` -- all tests pass.
4. Verify no remaining references to the deleted fields/functions:
   - `grep -r "placing_alt_held" crates/`
   - `grep -r "level_preview_snapped_price" crates/`
   - `grep -r "snap_price_to_ohlc" crates/`
   - `grep -r "snap_crosshair_to_ohlc" crates/`
   - `grep -r "PlacingLevel" crates/` (should only appear in comments or
     the new `LevelToolMode::Placing`)
5. Manual testing:
   - H activates, preview line follows with OHLC snap.
   - Alt disables snap (line follows cursor exactly).
   - Click places at snapped price.
   - Escape cancels.
   - Right-click during placement pans, tool resumes.
   - Middle-click during placement scales, tool resumes.
   - Drag non-locked level with OHLC snap.
   - Drag with Alt disables snap.
   - Right-click level opens editor.
   - Locked level cannot be dragged.
   - Drawing panel button activates.
   - Double-click places at OHLC-snapped price.
   - Clear All removes all levels.
   - Config save/load round-trips correctly.
   - Viewport resize during placement cancels the tool cleanly.

## 10. Test Plan

### 10.1 New unit tests in level_tool.rs

```
test snap_to_ohlc_finds_nearest_ohlc
test snap_to_ohlc_beyond_threshold_returns_raw
test snap_to_ohlc_alt_held_returns_raw
test snap_to_ohlc_empty_data_returns_raw
test snap_to_ohlc_updates_snapped_price_field
test snap_to_ohlc_collapsed_mode
test snap_to_ohlc_ignores_distant_candles
test activate_sets_placing_mode
test cancel_clears_all_state
test suspend_and_resume_placing
test suspend_when_not_placing_is_noop
test resume_when_not_suspended_is_noop
test is_active_predicates
```

### 10.2 Updated interaction tests

All existing tests in `interaction.rs::tests` must pass. Key changes:
- `handle_event` calls gain `data` and `is_collapsed` parameters.
- Tests using `InteractionMode::PlacingLevel` switch to
  `state.level_tool.is_placing()`.
- Tests using `InteractionMode::DraggingLevel` switch to
  `state.level_tool.is_dragging()`.

### 10.3 Updated compute tests

All existing tests in `compute.rs::tests` must pass. Key changes:
- `ChartInput` construction uses `level_tool: &LevelTool::default()`
  instead of `placing_level: false, placing_alt_held: false`.

## 11. Risk Assessment

| Risk | Mitigation |
|---|---|
| Snap behavior changes subtly (different search radius or threshold) | The unified `snap_to_ohlc` uses the compute.rs algorithm (nearest +/-1, adaptive threshold), which is the more precise one. App.rs's all-visible-candle search was always a worse approximation. Net improvement. |
| `handle_event` API change breaks downstream | Only `midas-app` calls `handle_event`. Single call site, easy to update. |
| Orthogonal level_tool + interaction_mode creates new edge cases | The two state machines are strictly non-overlapping: level_tool handles level concerns, interaction_mode handles camera concerns. The `suspend_placing` / `try_resume_placing` pattern is explicit about the one point of contact. |
| Performance: snap_to_ohlc called on every mouse move during placement | Same cost as current compute-layer snap. Search radius is 3 candles (6 OHLC values). Negligible. |
| Level sync guard (skip overwrite during active tool) still needed | Yes, but the guard condition is cleaner: `!level_tool.is_active()` instead of `matches!(interaction_mode, DraggingLevel { .. })`. This broadens the guard to also block during `Placing` (not just `Dragging`), meaning external level changes won't sync until the tool returns to Idle. This is acceptable — the user is actively interacting and won't notice a one-operation delay. |
| Sibling plans reference deleted artifacts | `crosshair-tool-refactor.md` references `snap_crosshair_to_ohlc()` (deleted here). The annotations plan (`04-interaction.md`) references `DraggingLevel` on `InteractionMode` and follows the old entangled pattern. Update both plans after this refactor completes. The annotations plan should follow the orthogonal tool pattern established here. |

## 12. Lines of Code Estimate

| Category | Lines |
|---|---|
| `level_tool.rs` (struct + snap + tests) | ~250 |
| Net deletion from compute.rs | ~90 |
| Net deletion from app.rs | ~50 |
| Net deletion from chart_widget.rs | ~30 |
| Net deletion from state.rs | ~10 |
| Modifications across all files | ~150 |
| **Net change** | **~+70 lines** (mostly tests) |
