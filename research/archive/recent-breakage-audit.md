# Chart Rendering Breakage Audit

## Summary
Found **1 critical bug** and verified the rest of the recent refactoring is sound. Chart rendering is broken due to incomplete camera restoration logic in DataLoaded handler.

## Critical Bug: Incomplete Saved Camera Restoration

**Location:** `desktop/win/crates/midas-app/src/app.rs`, lines 2188-2207 (DataLoaded handler)

**Issue:** When restoring a previously-saved camera position after ticker switch, the code only applies the camera if `needs_shift` is true. If historical data IS available (`needs_shift = false`), the saved camera is discarded entirely.

**Current Code (Broken):**
```rust
if let Some(saved) = ts.saved_camera() {
    let duration = saved.time_end - saved.time_start;
    let needs_shift = if saved.was_at_live_edge {
        true
    } else {
        !chart.has_candles_in_range(saved.time_start, saved.time_end)
    };
    if needs_shift {  // ← ONLY applies if true
        // Apply shifted camera
        chart.chart_state.camera.time_end = latest + margin;
        chart.chart_state.camera.time_start = ...;
    }
}
```

**Expected Code (from commit 984d1b1):**
```rust
if saved.was_at_live_edge {
    // Shift window to latest candle
    chart.chart_state.camera.time_end = latest + margin;
    chart.chart_state.camera.time_start = ...;
} else {
    // Restore historical range verbatim
    chart.chart_state.camera.time_start = saved.time_start;
    chart.chart_state.camera.time_end = saved.time_end;
}
chart.chart_state.camera.price_low = saved.price_low;
chart.chart_state.camera.price_high = saved.price_high;
chart.chart_state.dirty.mark_camera();
```

**Impact:** User switches from ticker A to B and back to A. If B's data fully covers A's previous viewport, A's saved camera is silently dropped. Chart renders with default "last 200 candles" view instead of user's saved position.

**Root Cause:** Commit 23d257b (Per-ticker camera state) added the deferred-restore logic but didn't fully port the live-edge/historical branching from the old bind_chart_to_symbol code.

---

## Verified: Stale-Data Guard (cb66a1f)

✓ Message pattern correctly updated from 2-arg to 3-arg in both variants
✓ Generation parameter captured in load_chart_with closure
✓ Both DataLoaded and DataRestoredFromStartup handlers check stale guard
✓ No code accidentally removed after guard was added

---

## Verified: Camera Per-Ticker (94641a2, 984d1b1)

✓ apply_candle_data now correctly called with reset_camera=true (always)
✓ Saved camera re-evaluation happens in same handler (no flash)
✓ camera_restored_pending flag cleared on first user pan
✓ save_camera_for_chart exported to ticker_wiring module and called in all pan/zoom handlers

---

## Verified: Ticker Wiring Extraction (125f1c3)

✓ All Message::ChartPan/ChartZoom/ChartZoomY handlers still present in app.rs
✓ Handler implementations intact and unchanged
✓ Message::Ticker delegation to handle_ticker_effects is correct
✓ No handlers dropped during module extraction

---

## Verified: Ticker Order Intent Deletion (a683764)

✓ No Message::OrderIntent references remain
✓ TickerState apply() correctly called via Message::Ticker
✓ All broker submission logic migrated to handle_ticker_effects
✓ Tests pass (244 passed, 0 failed)

---

## Verified: Chart Widget View

✓ Both floating and docked charts construct ChartRenderSnapshot with fresh data from chart.data
✓ All snapshot fields properly populated from chart state
✓ No stale data references

---

## Additional Observations

- cargo check: clean (0 errors)
- cargo test: 244 passed, 0 failed
- No TODO/todo!()/unreachable!() markers relevant to chart rendering
- All 3-arg Message patterns fully updated across codebase

