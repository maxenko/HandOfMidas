# Feature: Per-Ticker Camera State

## Overview

Remember the viewport camera position (time range + price range) for each ticker so that switching away and back restores exactly where the user left off. Uses the existing `TickerState` per-ticker persistence — four scalar `f64` fields, saved on pan/zoom, restored on ticker bind.

## Research Summary

### Codebase Analysis

**Camera2D** (`midas-chart/src/camera/mod.rs:19-35`): `time_start`, `time_end` (x position + scale), `price_low`, `price_high` (y position + scale), plus `viewport_width`, `viewport_height`, `dpi_scale` (screen-dependent, NOT per-ticker).

**Current behavior**: camera is NOT reset or restored on symbol switch. `bind_chart_to_symbol` in `app/ticker_wiring.rs` sets the symbol and creates brackets but never touches the camera. The chart keeps whatever camera bounds it had from the previous ticker — which is usually wrong (a $14 stock using a $150 price window from the last ticker).

**Pan/zoom path**: user input → `ChartAction::Pan/Zoom/ZoomY` → `ChartState::apply_action()` mutates `camera` fields → dirty flags set → next render uses new camera.

**TickerState pattern**: private fields + getter + `#[serde(default)]` + TickerMsg variant + apply() handler returning effects. Camera fields follow the same pattern as `last_price`, `gatr_abs`, `bracket_mode`.

**Crate boundary**: Camera2D is in midas-chart (sans-IO); TickerState is in midas-app. Store camera as 4 scalar f64s on TickerState, not as Camera2D (avoids cross-crate serde dependency). Convert at the boundary.

## Design Decisions

### D1: Store 4 scalars, not Camera2D

**Context**: Camera2D doesn't derive Serialize/Deserialize. Adding it would couple midas-chart to serde. Viewport dimensions and DPI scale are screen-dependent, not ticker-dependent.

**Recommendation**: Store `camera_time_start: Option<f64>`, `camera_time_end: Option<f64>`, `camera_price_low: Option<f64>`, `camera_price_high: Option<f64>`, and `camera_was_at_live_edge: bool` on TickerState. The four f64s are `Option` so a fresh ticker (never viewed) has `None` and falls through to the existing auto-scale behavior. `camera_was_at_live_edge` defaults to `true`. Convert to/from Camera2D at the boundary in `bind_chart_to_symbol`.

**Confidence**: high.

### D2: Save on gesture end, not per-frame

**Context**: At 60 Hz, saving every mouse-move frame would dispatch 60 `Message::Ticker(SaveCameraState)` per second — each going through the reentry guard, match, and effect handler. Only the last position matters.

**Recommendation**: Fire `SaveCameraState` once per user action: on mouse-up after a pan drag, on each scroll-wheel tick (already a single event), and when the AutoScaleY animation completes. This gives 1 dispatch per gesture instead of 60 per second. The existing `PersistDirty` effect + 75ms debounce flush handles the redb write naturally.

**Confidence**: high.

### D3: Restore in bind_chart_to_symbol, before data load

**Context**: `bind_chart_to_symbol` is the single chokepoint for every ticker activation. Restoring the camera here means every activation path (watchlist click, symbol link, startup) gets the saved camera.

**Recommendation**: After `bind_chart_to_symbol` sets the symbol and before bracket creation, read the TickerState's saved camera. If present, set `chart.chart_state.camera.time_start/end/price_low/high` from the saved values. If `None`, don't touch the camera (auto-scale will handle it when data arrives).

**Confidence**: high.

### D4: Smart restore — live edge detection

**Context**: If the user was watching live price action (most recent candle visible) and comes back a day later, restoring the exact saved time window would show stale history. But if the user was deliberately examining a historical region, restoring should not jump them to the present.

**Recommendation**: On save, compute `was_at_live_edge: bool` — true if the most recent candle's timestamp falls within `[time_start, time_end]`. On restore:
- If `was_at_live_edge == true`: preserve the zoom level (`duration = time_end - time_start`) but shift the window so the latest available candle is near the right edge: `time_end = latest_candle_time + small_margin`, `time_start = time_end - duration`. The user sees current data at the same zoom.
- If `was_at_live_edge == false`: restore `time_start` and `time_end` verbatim. The user was examining history — don't move them.
- Y axis (`price_low`, `price_high`): always restore as-is; AutoScaleY will adjust to fit visible candles naturally.

The "latest candle time" for the restore is read from the chart's loaded data at the moment `bind_chart_to_symbol` runs. If no data has loaded yet (fresh startup), defer the restore to `MarketSnapshotLoaded` (same pattern as bracket creation deferral).

**Confidence**: high.

### D5: Graceful fallback when historical view has no data

**Context**: If `was_at_live_edge == false` (historical view), the camera restores to the exact saved `time_start`/`time_end`. But that time range might contain no data — the data may have been pruned, the provider may not cover that period, or the chart simply doesn't have candles loaded for that region. The user sees an empty viewport with no candles, which is confusing.

**Recommendation**: After restoring a historical camera position (verbatim restore path), validate that the restored time window actually overlaps with loaded data. Check: does any candle in the chart's data fall within `[time_start, time_end]`? If not (empty viewport), fall back to the live-edge behavior — shift the window to show the last available data at the same zoom level. This gives the user a populated chart instead of a blank one, while still preserving their zoom level.

The validation runs in two places:
1. **In `bind_chart_to_symbol`**: immediately after restore, check overlap with loaded data. If no data is loaded yet, set `camera_restored_pending = true` and let `MarketSnapshotLoaded` re-evaluate (same as the live-edge deferral).
2. **In `MarketSnapshotLoaded`**: after data arrives, if `camera_restored_pending` and the restored historical window has zero overlap with the loaded data, shift to last available candle.

The rule is: "show the user something useful, never a blank chart." If the historical region has data, honor it. If it doesn't, gracefully jump to what's available.

**Confidence**: high.

## Implementation Plan

### Slice 1: Add camera fields to TickerState + save + restore

**Goal**: Single slice — add the fields, the save path, the restore path, and tests. This is a small, contained change following the established TickerState pattern.

**Depends on**: None.

**Files to modify**:

- `desktop/win/crates/midas-app/src/ticker_state/mod.rs` — add 5 private fields:
  ```rust
  #[serde(default)]
  camera_time_start: Option<f64>,
  #[serde(default)]
  camera_time_end: Option<f64>,
  #[serde(default)]
  camera_price_low: Option<f64>,
  #[serde(default)]
  camera_price_high: Option<f64>,
  #[serde(default = "default_true")]
  camera_was_at_live_edge: bool,
  ```
  Add getter: `saved_camera() -> Option<SavedCamera>` where:
  ```rust
  pub struct SavedCamera {
      pub time_start: f64,
      pub time_end: f64,
      pub price_low: f64,
      pub price_high: f64,
      pub was_at_live_edge: bool,
  }
  ```
  Returns `Some(SavedCamera { ... })` if all 4 f64s are `Some`, `None` if any is missing. `was_at_live_edge` is always available (defaults `true`).

- `desktop/win/crates/midas-app/src/ticker_state/apply.rs` — add TickerMsg variant:
  ```rust
  SaveCameraState {
      time_start: f64,
      time_end: f64,
      price_low: f64,
      price_high: f64,
      was_at_live_edge: bool,
  },
  ```
  Handler in `apply()` dispatcher → new `fn apply_save_camera(...)`:
  ```rust
  self.camera_time_start = Some(time_start);
  self.camera_time_end = Some(time_end);
  self.camera_price_low = Some(price_low);
  self.camera_price_high = Some(price_high);
  self.camera_was_at_live_edge = was_at_live_edge;
  self.generation += 1;
  vec![TickerEffect::PersistDirty]
  ```
  The caller computes `was_at_live_edge` by checking if the latest candle's timestamp is within `[time_start, time_end]` at the moment of the gesture end.

- `desktop/win/crates/midas-app/src/app.rs` (ChartPanel struct) — add `camera_restored_pending: bool` field to `ChartPanel`. Default `false`. Set to `true` in `bind_chart_to_symbol` when a saved camera is restored. Cleared to `false` in `MarketSnapshotLoaded` after the re-restore runs, and in the `Message::ChartPan` handler on the first user pan (so a manual pan after bind doesn't get overwritten by the deferred restore). Add two helper methods on `ChartPanel`:
  - `fn latest_candle_time(&self) -> Option<f64>` — reads the last timestamp from the chart's loaded candle data (`self.data.as_ref().and_then(|d| d.last_timestamp())`). Returns `None` if no data is loaded.
  - `fn has_candles_in_range(&self, start: f64, end: f64) -> bool` — returns true if any candle timestamp in the loaded data falls within `[start, end]`. Used by D5's empty-history fallback.

- `desktop/win/crates/midas-app/src/app/ticker_wiring.rs` — in `bind_chart_to_symbol`, after setting the symbol and before bracket creation, apply the smart restore:
  ```rust
  // Restore per-ticker camera if saved.
  if let Some(ts) = self.tickers.get(&symbol) {
      if let Some(saved) = ts.saved_camera() {
          if let Some(chart) = self.charts.get_mut(&chart_id) {
              if saved.was_at_live_edge {
                  // User was watching live price action. Shift the time
                  // window so the latest candle is near the right edge,
                  // preserving the zoom level (same duration).
                  let duration = saved.time_end - saved.time_start;
                  let latest = chart.latest_candle_time()
                      .unwrap_or(saved.time_end);
                  let margin = duration * 0.02; // 2% breathing room
                  chart.chart_state.camera.time_end = latest + margin;
                  chart.chart_state.camera.time_start = chart.chart_state.camera.time_end - duration;
              } else {
                  // User was examining history. Restore verbatim.
                  chart.chart_state.camera.time_start = saved.time_start;
                  chart.chart_state.camera.time_end = saved.time_end;
              }
              chart.chart_state.camera.price_low = saved.price_low;
              chart.chart_state.camera.price_high = saved.price_high;
              chart.chart_state.dirty.mark_camera();
          }
      }
  }
  ```
  `chart.latest_candle_time()` reads the last timestamp from the chart's loaded candle data. If data hasn't loaded yet (fresh startup), the `unwrap_or(saved.time_end)` fallback uses the saved value as a temporary position — the `MarketSnapshotLoaded` handler re-applies the live-edge shift once real data arrives (see below).

- `desktop/win/crates/midas-app/src/app.rs` — **two additions**:

  **A. Re-apply live-edge camera shift in `MarketSnapshotLoaded`**: when fresh candle data arrives for a symbol, check if the bound TickerState has `was_at_live_edge == true` AND the chart's camera hasn't been manually panned since bind (track via a `camera_restored_pending: bool` flag on ChartPanel, set true in bind_chart_to_symbol, cleared on first user pan). If both conditions hold, re-compute the live-edge shift with the now-available `latest_candle_time()`:
  ```rust
  // In MarketSnapshotLoaded, after data is loaded:
  if let Some(chart) = self.charts.get_mut(&chart_id) {
      if chart.camera_restored_pending {
          if let Some(ts) = self.tickers.get(&sym_key) {
              if let Some(saved) = ts.saved_camera() {
                  let duration = saved.time_end - saved.time_start;
                  let needs_shift = if saved.was_at_live_edge {
                      // Live edge: always shift to latest.
                      true
                  } else {
                      // Historical: check if the restored window has
                      // any data. If empty, fall back to live edge.
                      !chart.has_candles_in_range(
                          saved.time_start, saved.time_end,
                      )
                  };
                  if needs_shift {
                      if let Some(latest) = chart.latest_candle_time() {
                          let margin = duration * 0.02;
                          chart.chart_state.camera.time_end = latest + margin;
                          chart.chart_state.camera.time_start =
                              chart.chart_state.camera.time_end - duration;
                          chart.chart_state.dirty.mark_camera();
                      }
                  }
              }
              chart.camera_restored_pending = false;
          }
      }
  }
  ```
  This handles two cases:
  1. **Startup timing gap** (live edge): `bind_chart_to_symbol` runs before data arrives and uses a stale fallback; `MarketSnapshotLoaded` arrives with real data and shifts to the latest candle.
  2. **Empty historical view** (D5): the saved time window pointed to a region with no loaded data (pruned, unavailable, or different timeframe). Rather than showing a blank chart, the fallback shifts to the last available data at the same zoom level. `has_candles_in_range(start, end)` checks if any candle timestamp falls within the range — if false, the shift fires.

  The `camera_restored_pending` flag ensures this runs exactly once per bind (not on every data update).

  **B. Save camera state on gesture end**, not per-frame. Find the mouse-up / drag-end handler that fires after a pan or zoom gesture completes (search for `DragEnd`, `MouseUp`, `Released`, or the message that clears the interaction state after panning). Fire `SaveCameraState` there — one dispatch per gesture, not 60/sec during the drag. The camera values come from `chart.chart_state.camera` at the moment of release:
  ```rust
  // Save camera to TickerState on gesture end (not per-frame).
  if let Some(sym) = chart.bound_symbol.as_ref() {
      let cam = &chart.chart_state.camera;
      // Determine if the most recent candle is visible in the viewport.
      let latest = chart.latest_candle_time().unwrap_or(0.0);
      let at_live_edge = latest > 0.0
          && latest >= cam.time_start
          && latest <= cam.time_end;
      let _ = self.update(Message::Ticker(
          sym.clone(),
          TickerMsg::SaveCameraState {
              time_start: cam.time_start,
              time_end: cam.time_end,
              price_low: cam.price_low,
              price_high: cam.price_high,
              was_at_live_edge: at_live_edge,
          },
      ));
  }
  ```
  Also fire after: `Message::ChartAutoScaleY` animation end, scroll-wheel zoom (which is a single event, not a gesture), and keyboard pan/zoom if those exist. The key rule: save once per user action, not once per frame.

- `desktop/win/crates/midas-app/src/ticker_state/tests.rs` — tests:
  - `save_camera_state_persists` — fire SaveCameraState with was_at_live_edge=true, assert saved_camera() returns the values + PersistDirty effect
  - `save_camera_preserves_live_edge_flag` — fire with was_at_live_edge=false, assert saved_camera().was_at_live_edge == false
  - `save_camera_serde_roundtrip` — serialize/deserialize TickerState with camera fields including was_at_live_edge, assert roundtrip
  - `saved_camera_returns_none_when_unset` — fresh TickerState, assert saved_camera() is None
  - `saved_camera_returns_none_on_missing_field_backward_compat` — deserialize old blob without camera fields, assert None (covered by `#[serde(default)]`); was_at_live_edge defaults to true
  - `restore_at_live_edge_shifts_to_latest_candle` — saved camera with was_at_live_edge=true, latest candle is 1 day later. Assert restored time_end is near latest candle, duration preserved.
  - `restore_at_history_uses_saved_verbatim` — saved camera with was_at_live_edge=false, data exists in the saved range. Assert restored time_start and time_end match saved values exactly.
  - `restore_at_history_empty_falls_back_to_latest` — saved camera with was_at_live_edge=false, NO data in the saved time range. Assert restored window shifts to latest candle (same duration, anchored at latest data). Verifies D5 graceful fallback.

**Done when**: switch to AAPL, zoom to a specific time range, switch to IBM, switch back to AAPL → the chart restores the exact zoom/scroll position. Restart the app → same thing (persisted via redb).

## Risks & Unknowns

- **Auto-scale conflict**: when data loads after camera restore, `AutoScaleY` might override the restored `price_low`/`price_high`. Mitigation: the restore sets the camera BEFORE data loads (in `bind_chart_to_symbol`, which runs before `MarketSnapshotLoaded`). If auto-scale runs after data arrives, it only adjusts Y — the X (time) range stays from the restore. This is acceptable because the user's time-range zoom is preserved, and Y auto-adjusts to fit the visible candles. If the user explicitly set a Y zoom (via ZoomY), they'd need to re-zoom after data load. Note: this is the same behavior as today — AutoScaleY always runs on new data.
- **Momentum animations**: if the user switches tickers while a pan momentum animation is running, the in-flight animation will overwrite the restored camera. Mitigation: `bind_chart_to_symbol` could cancel pending animations. Low priority — rare edge case.

## Testing Strategy

- Unit tests on TickerState (save/restore/roundtrip)
- Manual test: zoom AAPL to a specific region → switch to IBM → switch back → verify position
- Manual test: restart app → verify AAPL's camera is where you left it

## Non-Goals / Out of Scope

- Saving the camera for the auto-scale Y animation target (only the raw camera bounds are saved)
- Per-timeframe camera state (one camera per ticker, regardless of timeframe)
- Momentum animation cancellation on ticker switch (can be a follow-up)
