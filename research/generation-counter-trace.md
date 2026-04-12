# Generation Counter Bug Trace Report

## Summary
The generation counter implementation has a critical bug: **`load_generation` is NOT incremented in most code paths that call `load_chart_async()`, causing all messages to be discarded**.

## Step 1: ChartPanel Field Definition
**File**: `app.rs` line 138  
**Field**: `pub load_generation: u64`  
**Default Value**: Initialized to `0` in `make_empty_panel()` (line 1418)

## Step 2: Increment Location - PROBLEM FOUND
**File**: `app.rs` line 1618  
**Function**: `load_symbol_for_chart()`  
**Code**:
```rust
if let Some(chart) = self.charts.get_mut(&chart_id) {
    chart.load_generation = chart.load_generation.wrapping_add(1);  // Line 1618
    chart.load_state = LoadState::Loading;
    chart.chart_state.dirty.mark_data();
}
```
**Timing**: Increment happens BEFORE `load_chart_async()` is called (line 1629)

**CRITICAL ISSUE**: This increment ONLY happens in `load_symbol_for_chart()`. There are **6+ other call sites** that call `load_chart_async()` WITHOUT incrementing:
- Line 1760: `propagate_timeframe_change()` 
- Line 1977: `reload_all_charts()`
- Line 2989: (SymbolSubmitted handler)
- Line 4242: (Some other handler)
- Line 5236: (Some other handler)

## Step 3: Read Location in load_chart_with
**File**: `app.rs` lines 1848-1852  
**Code**:
```rust
let gen = self
    .charts
    .get(&chart_id)
    .map(|c| c.load_generation)
    .unwrap_or(0);  // Line 1852
```
**Order**: Reads `load_generation` AFTER it was already incremented (if increment happened). But the increment only happens in `load_symbol_for_chart()`, not in the other call paths.

## Step 4: DataLoaded Handler Guard Logic
**File**: `app.rs` lines 2128-2137  
**Code**:
```rust
Message::DataLoaded(chart_id, gen, result) => {
    // Discard stale loads — chart may have switched tickers since this load started.
    if let Some(chart) = self.charts.get(&chart_id) {
        if chart.load_generation != gen {  // Line 2131
            tracing::debug!(
                "discarding stale DataLoaded (gen {gen} != {})",
                chart.load_generation
            );
            return Task::none();  // DISCARD
        }
    }
```
**Logic**: `if chart.load_generation != gen { DISCARD }` - Discards when they DON'T match.  
**Status**: Logic is CORRECT, but the counters don't match because increments are missing.

## Step 5: Other Call Sites Without Increment
The following call sites call `load_chart_async()` WITHOUT incrementing `load_generation`:
1. **propagate_timeframe_change()** (line 1760) - reloads linked charts when timeframe changes
2. **reload_all_charts()** (line 1977) - reloads all charts (e.g., after provider switch)
3. **SymbolSubmitted handler** (line 2989) - inline symbol submission
4. **Unknown handler** (line 4242) - needs context verification
5. **Unknown handler** (line 5236) - needs context verification

**Result**: When these paths execute, `load_generation` stays at its old value (0 for startup, or the last value from `load_symbol_for_chart`). The async task captures that value. When `DataLoaded` arrives, it compares the captured gen against the current `load_generation` (which hasn't changed), but the guard says "if they don't match, discard". Since they DO match (both are stale), the data is applied.

## Step 6: Startup Restore Path
**File**: `app.rs` lines 1195, 1875-1882  
**Code**:
```rust
// Line 1195: Called from new()
load_tasks.push(app.load_chart_async_restore(*id, symbol, *tf));

// Lines 1875-1882
fn load_chart_async_restore(&self, chart_id: ChartId, symbol: &str, tf: Timeframe) -> Task<Message> {
    self.load_chart_with(chart_id, symbol, tf, Message::DataRestoredFromStartup)
}
```
**Status**: No increment happens before `load_chart_async_restore()`. The chart has `load_generation = 0` (default). The captured `gen` is also `0` (from `unwrap_or(0)` in `load_chart_with`). The guard at line 2242 checks `if chart.load_generation != gen` — `0 != 0` is false, so it passes. This path actually works correctly.

## Step 7: Chart Existence Check
**File**: `app.rs` line 1848-1852  
In `load_chart_with()`:
```rust
let gen = self
    .charts
    .get(&chart_id)
    .map(|c| c.load_generation)
    .unwrap_or(0);
```
**Issue**: If `self.charts.get(&chart_id)` returns `None` (invalid chart_id), `gen` defaults to `0`. The guard at line 2131 then does `if let Some(chart) = self.charts.get(&chart_id)` — if the chart STILL doesn't exist, the guard is skipped entirely and the data is applied anyway. This is a secondary issue.

## Root Cause
**The hypothesis is correct but incomplete**: The issue is not that the guard is inverted; it's that `load_generation` is NOT being incremented in 6+ code paths that call `load_chart_async()` or `load_chart_with()` directly.

Only `load_symbol_for_chart()` increments the counter. All other paths skip the increment, causing:
1. Stale `DataLoaded` messages from previous requests to be accepted (they match the old counter)
2. Current requests' data to be discarded if issued from these paths (they haven't incremented)
3. Pan/zoom broken because stale data from a previous ticker is applied

## Fix Required
Every call site that invokes `load_chart_async()` or `load_chart_with()` must increment `load_generation` first, OR the increment must be moved into `load_chart_async()` / `load_chart_with()` itself (though this requires `&mut self`).
