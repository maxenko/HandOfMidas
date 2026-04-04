# Global Level Placement Mode

## Problem

Level placement is currently per-chart: clicking the Level button activates
placement on the focused chart only. The preview line and click-to-place only
work within that single chart panel. The user cannot roam between charts to
place levels on different tickers without re-activating the tool each time.

## Desired Behavior

1. **Global activation.** Clicking the Level button (or pressing H) activates
   placement mode across ALL visible charts — pane grid and floating windows.
   Every chart's button highlights blue.

2. **Roaming preview.** The preview horizontal line appears in whichever chart
   the cursor is currently over. When the cursor leaves that chart, the preview
   disappears. Other charts show no preview while the cursor isn't over them.

3. **Cross-symbol placement.** The user can place a level in any chart. The
   level is stored under the correct ticker in `LevelStore`. After placement,
   the global mode deactivates (one-shot).

4. **Cancellation.** Escape or re-clicking the button deactivates placement
   globally.

## Design

### Source of truth: `MidasApp.level_placing: bool`

A single boolean on `MidasApp` replaces per-chart `level_tool.activate()`
calls for placement mode control. The per-chart `LevelTool` still handles
local concerns (preview_price, snapped_price, OHLC snap, drag state) — only
the "am I in placement mode?" question becomes global.

### Data flow

```
User clicks Level button / presses H
    ↓
MidasApp toggles self.level_placing
    ↓
Next frame: all chart snapshots carry level_placing = true
    ↓
Each chart widget syncs: if level_placing && !local_placing → activate()
    ↓
Mouse events only arrive at the chart under the cursor
    ↓
That chart's handle_suppressed_move() sets preview_price → preview line renders
    ↓
Other charts' handle_suppressed_move() with !in_bounds → preview_price = None → no line
    ↓
User left-clicks → CreateLevel for that chart's ticker
    ↓
MidasApp sets level_placing = false
    ↓
Next frame: all chart widgets sync: !level_placing && local_placing → cancel()
```

### Why per-chart LevelTool is still needed

- `preview_price` is cursor-position-dependent (chart-local coordinates)
- `snapped_price` depends on each chart's data and camera
- OHLC snap computation uses chart-local pixel distances
- Dragging is always per-chart (a drag is on a specific level in a specific chart)
- `suspend_placing` / `try_resume_placing` for pan/scale interrupts are per-widget

### Preview clearing when cursor leaves a chart

iced's shader widgets receive ALL mouse events regardless of cursor position.
Each widget converts to local coordinates and checks bounds. When `!in_bounds`
during Placing mode, `handle_suppressed_move()` currently does NOT clear
`preview_price` — the value persists from the last in-bounds move, causing a
stale preview line.

**Fix:** Add `else { state.level_tool.preview_price = None; }` in the
`!in_bounds` branch of the Placing path in `handle_suppressed_move()`.

## Files to modify (4 files)

### 1. `midas-app/src/app.rs` — Global state + message handlers

**Add field:**
```rust
pub struct MidasApp {
    /// Whether level placement mode is globally active.
    pub level_placing: bool,
    // ...
}
```
Initialize to `false` in constructor.

**Modify handlers:**

| Message | Current | New |
|---------|---------|-----|
| `DrawingPanelCreateLevel(id)` | Toggle per-chart `level_tool` | Toggle `self.level_placing` |
| `ChartCreateLevel(id, price)` | Creates level, cancels per-chart tool | Also set `self.level_placing = false` |
| `ChartCancelPlacing(id)` | Cancel per-chart tool | Set `self.level_placing = false` |
| H hotkey | Per-chart `level_tool.activate()` | Toggle `self.level_placing` |
| Escape key | Per-chart `level_tool.cancel()` | Set `self.level_placing = false` |

Remove direct per-chart `level_tool.activate()` and `level_tool.cancel()` calls
from these handlers — the widget sync loop handles activation/deactivation.

### 2. `midas-app/src/app/views.rs` — Button highlight + snapshot

**Drawing panel button:** Pass `self.level_placing` instead of per-chart
`chart.chart_state.level_tool.is_placing()`:

```rust
// Before:
let is_placing = chart.chart_state.level_tool.is_placing();
let drawing_panel = build_drawing_panel(chart_id, is_placing);

// After:
let drawing_panel = build_drawing_panel(chart_id, self.level_placing);
```

Both in `view_floating_chart` and `view_pane_body`.

**Snapshot:** Add `level_placing` field:
```rust
ChartRenderSnapshot {
    level_placing: self.level_placing,
    // ...
}
```

### 3. `midas-app/src/chart_widget.rs` — Snapshot field + sync logic

**Add to snapshot struct:**
```rust
pub struct ChartRenderSnapshot {
    pub level_placing: bool,
    // ...
}
```

**Replace level_tool sync block** (lines 158–167):

```rust
// Old:
if !chart_state.level_tool.is_active() && !state.tool_cancelled_this_frame {
    chart_state.level_tool = self.snapshot.level_tool.clone();
}
if !self.snapshot.level_tool.is_active() {
    state.tool_cancelled_this_frame = false;
}

// New:
// Sync global placement state → per-widget level tool.
// Don't interfere with dragging (drag is always local).
if !chart_state.level_tool.is_dragging() {
    if self.snapshot.level_placing && !chart_state.level_tool.is_placing()
        && !state.tool_cancelled_this_frame
    {
        chart_state.level_tool.activate();
    } else if !self.snapshot.level_placing && chart_state.level_tool.is_placing() {
        chart_state.level_tool.cancel();
    }
}
// Clear cancel-guard once the app has caught up.
if !self.snapshot.level_placing {
    state.tool_cancelled_this_frame = false;
}
```

**tool_cancelled_this_frame rationale:** When the user clicks to place a level
in chart A, the widget emits `CancelPlacing`. But the snapshot for the *next*
frame may still have `level_placing = true` (the message hasn't round-tripped
yet). Without the guard, the widget would immediately re-enter Placing mode.
The flag prevents this until the snapshot catches up.

### 4. `midas-chart/src/interaction.rs` — Clear preview on cursor exit

In `handle_suppressed_move()`, Placing mode branch (~line 870):

```rust
// Before:
if in_bounds {
    // ... compute preview_price
}

// After:
if in_bounds {
    // ... compute preview_price
} else {
    state.level_tool.preview_price = None;
}
```

This ensures the preview line disappears when the cursor moves to another chart.

## Edge cases

| Scenario | Behavior |
|----------|----------|
| Click Level button while already placing | Toggles off (`level_placing = false`) |
| Right-click to pan during placement | Per-chart `suspend_placing()` / `try_resume_placing()` still works; global flag stays true |
| Middle-click to scale during placement | Same as right-click — local suspend/resume |
| Place level, then move to another chart | `level_placing` set false after placement. New chart's tool syncs to idle. |
| Resize window during placement | Widget cancels local tool (existing behavior); global `level_placing` stays true, widget re-syncs on next frame |
| Drag existing level while NOT placing | Dragging is separate from placement — `is_dragging()` guard prevents sync interference |
| H key while dragging | `activate()` is a no-op during drag (existing guard in LevelTool) |

## Verification

```bash
cargo check -p midas-app
cargo clippy --workspace
cargo test --workspace
cargo fmt --all
```

Manual test:
1. Open 2+ panes with different symbols
2. Click Level button on any chart → all buttons highlight blue
3. Hover chart A → preview line visible on A only
4. Move to chart B → preview disappears from A, appears on B
5. Click to place → level created for B's ticker, all buttons deactivate
6. Verify H hotkey and Escape work globally
