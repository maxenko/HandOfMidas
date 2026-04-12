# Pan/Zoom Interaction Pipeline Trace

## Problem Statement
After loading tickers, pan and zoom don't work — candles don't respond to mouse interaction. Brackets load correctly, confirming data IS reaching the widget.

## Interaction Pipeline (Complete, No Breaks Found)

### 1. Mouse Events → ChartEvent ✓
- **File:** `crates/midas-app/src/chart_widget.rs:1348-1431` (`translate_mouse_event`)
- Mouse events (CursorMoved, ButtonPressed, ButtonReleased, WheelScrolled) are correctly translated to ChartEvents
- Position handling is correct: unclamped for drags, clamped for button presses (as intended)
- **Status:** Working

### 2. ChartEvent → ChartAction ✓
- **File:** `crates/midas-chart/src/interaction/mod.rs:446-518` (`handle_event`)
- `handle_event` dispatches to `handle_mouse_moved`, `handle_mouse_pressed`, `handle_mouse_released`, `handle_mouse_wheel`
- Pan/Zoom actions are generated:
  - Pan: Line 654 (`InteractionMode::Panning`)
  - Zoom horizontal: Line 697 (`InteractionMode::HorizontalScaling`)
  - Zoom vertical: Line 712 (`InteractionMode::VerticalScaling`)
- **Status:** Working

### 3. ChartAction → Message ✓
- **File:** `crates/midas-app/src/chart_widget.rs:1456-1477` (`action_to_message`)
- Converts ChartAction::Pan → Message::ChartPan (line 1464)
- Converts ChartAction::Zoom → Message::ChartZoom (line 1465)
- Converts ChartAction::ZoomY → Message::ChartZoomY (line 1471)
- **Status:** Working

### 4. Message Handlers → Camera Update + Dirty Flags ✓
- **File:** `crates/midas-app/src/app.rs:2511-2558`
- **ChartPan (2511-2524):** Calls `apply_action(&ChartAction::Pan)` which calls `mark_camera()` ✓
- **ChartZoom (2526-2541):** Directly updates camera AND calls `mark_camera()` at line 2536 ✓
- **ChartZoomY (2543-2558):** Directly updates camera AND calls `mark_camera()` at line 2553 ✓
- **Status:** Working

### 5. Dirty Flags → Renderer ✓
- **File:** `crates/midas-chart/src/dirty/mod.rs`
- Dirty flags are generation counters, not cleared
- `apply_action` and message handlers increment these counters
- DirtyTracker uses `needs_camera_update()` to check if camera generation changed
- **Status:** Working (generation-based system is sound)

### 6. Snapshot Creation → Widget ✓
- **File:** `crates/midas-app/src/app/views.rs:169-227`
- Snapshot created each frame with `dirty: chart.chart_state.dirty.clone()` (line 173)
- After pan/zoom message, dirty flags are already incremented, so next snapshot sees them
- **Status:** Working

### 7. Widget Event Routing ✓
- **File:** `crates/midas-app/src/chart_widget.rs:439-723` (`update` method)
- Events are correctly captured and routed
- `mouse_interaction` (line 915) always returns Interaction (never None), so iced will route events to widget
- **Status:** Working

### 8. Data Loading Dirty Flags ✓
- **File:** `crates/midas-app/src/app.rs:1924-1961` (`apply_candle_data`)
- Calls `chart.chart_state.dirty.mark_data()` at line 1927 ✓
- Calls `chart.chart_state.dirty.mark_camera()` at line 1960 ✓
- **Status:** Working (dirty flags ARE set after DataLoaded)

## Critical Finding: NO BREAKS DETECTED

The entire pipeline from mouse event to camera update is wired correctly:
1. Events reach the widget ✓
2. Events translate to actions ✓
3. Actions convert to messages ✓
4. Messages update camera AND mark dirty flags ✓
5. Dirty flags flow to snapshot ✓
6. Snapshot is created fresh each frame ✓

## Potential Hidden Issues to Investigate

Since no breaks are in the pipeline code itself, the problem likely lies in:

1. **Widget State Not Persisting Between Frames:**
   - In `update()` line 448-449, widget state is recreated from snapshot
   - After DataLoaded, snapshot's ChartState data might differ from widget's local state
   - Need to verify: After DataLoaded, is widget's `ChartState` properly synced to new data?

2. **Data Being Loaded But snapshot.data Not Updated:**
   - Snapshot is created in `views.rs` from `chart.data`
   - If DataLoaded doesn't properly set `chart.data`, snapshot.data stays None or stale
   - **Verification needed:** Check if `apply_candle_data` at line 1925 (`chart.data = Some(...)`) is actually persisting between frames

3. **Viewport Mismatch After Load:**
   - In `update()` line 454-457, camera viewport is synced from widget bounds
   - In `apply_candle_data`, camera is modified but viewport dimensions may be stale at that moment
   - If viewport is 0x0 during data load, interactions won't work (division by zero or clamping)

4. **camera_restored_pending Flag Blocking Interaction:**
   - Line 2183: If `camera_restored_pending` is true and no saved camera exists, code might leave camera in broken state
   - After DataLoaded and camera restore, verify camera ranges are valid

## Recommended Debugging Steps

1. Add logging to `apply_candle_data`: print camera viewport before/after
2. Add logging to snapshot creation in views.rs: print dirty.candles, dirty.camera values
3. Verify in update() that `pos_in_bounds` is not always None after DataLoaded
4. Check GPU renderer: verify candle instances are actually uploaded (DirtyTracker.acknowledge called)

**Code flow is correct. Problem is likely in state persistence, viewport handling, or GPU renderer acknowledgment.**
