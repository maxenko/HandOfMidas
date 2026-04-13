# Ticker Switch Bug Trace: A→B→A Sequence Analysis

## Bug Summary
After switching A→B→A, the chart displays B's candles with frozen candle graphics while the grid moves independently. This is a **data-symbol mismatch** where the wrong buffer's data renders against the new symbol's camera bounds.

## Root Cause
The `Message::DataLoaded` message carries **only** the `ChartId`, not the symbol. A stale DataLoaded from symbol B can arrive after the chart has been bound to symbol A, causing apply_candle_data to overwrite A's buffer with B's buffer.

---

## Step-by-Step Trace

### Step 1: User clicks ticker B in watchlist (while viewing A)

**File:** app.rs:3811 (Message::WatchlistTickerSelected)

1. Watchlist finds link targets matching its symbol_link mode
2. For each target chart: calls load_symbol_for_chart(id, "B")

**File:** app.rs:1601 (load_symbol_for_chart)

This function:
- Marks chart as LoadState::Loading
- Calls bind_chart_to_symbol(chart_id, SymbolKey("B"))
- Spawns async load via load_chart_async

**File:** app/ticker_wiring.rs:39 (bind_chart_to_symbol)

Only updates chart.symbol and chart.bound_symbol to "B". CRUCIALLY, does NOT clear or modify chart.data. The comment at lines 67-70 explicitly states:

"Do NOT set the camera here — the chart still has the previous ticker's candle buffer. Setting camera bounds for AMD while NVDA candles are rendered causes a visual desync (NVDA candles frozen at AMD's price/time range, grid moves under them)."

**State after bind:**
- chart.symbol = "B"
- chart.data = Some(Arc<A's CandleBuffer>) ← STILL STALE

**Async Task Spawned:** app.rs:1829-1848 (load_chart_with)

Creates a Task::perform closure that:
- Calls provider.get_candles("B", tf, days)
- On completion, calls move |result| Message::DataLoaded(chart_id, result)

CRITICAL: Only chart_id is embedded in the message, not "B".

---

### Step 2: User clicks ticker A (while B's data is loading)

Same sequence runs:
- load_symbol_for_chart(chart_id, "A") at app.rs:3831
- bind_chart_to_symbol(chart_id, SymbolKey("A")) at app/ticker_wiring.rs:39
- Spawns second async task for A's data

**State now:**
- chart.symbol = "A" (bound_symbol updated to A)
- chart.data = Some(Arc<A's CandleBuffer>) ← Still unchanged
- TWO pending async tasks:
  1. Message::DataLoaded(chart_id, B's buffer result)
  2. Message::DataLoaded(chart_id, A's buffer result)

The tasks are indistinguishable. No symbol in the message = no way to identify which belongs to which.

---

### Step 3: Stale DataLoaded for B arrives

**File:** app.rs:2112-2148 (Message::DataLoaded handler)

```
if let Some(chart) = self.charts.get_mut(&chart_id) {
    let sym = chart.symbol.clone();  // Reads "A" (CURRENT symbol)
    // ...
    Self::apply_candle_data(chart, buffer, true);  // buffer = B's data
}
```

The handler assumes buffer contains data for sym, but it's B's buffer. NO VERIFICATION occurs.

**File:** app.rs:1908-1944 (apply_candle_data)

Unconditionally:
- Sets chart.data = Some(Arc::clone(&buffer)) where buffer is B
- Calls chart.chart_state.dirty.mark_data()
- Resets camera to "last 200 candles" using B's price range and timestamps

**Mismatch State Achieved:**
- chart.symbol = "A"
- chart.data = Some(Arc<B's CandleBuffer>)
- chart.chart_state.camera = Camera computed from B's timestamps/closes

---

### Step 4: Rendering Frame

**File:** app/views.rs:845-914 (view_pane_body)

Snapshot construction at line 855-913:

```
let snapshot = ChartRenderSnapshot {
    symbol: chart.symbol.clone(),           // Reads "A"
    data: Some(Arc::clone(data)),          // Points to B's buffer
    camera: chart.chart_state.camera.clone(),  // Computed from B
    // ...
};
```

The snapshot MIXES:
- symbol = "A" (for title, OHLC display)
- data = B's candles (for rendering)
- camera = B's viewport (from apply_candle_data)

**GPU Render Result:**
- Title shows "A"
- Candles rendered are B's (from data buffer)
- Grid computed from camera (which matches B's buffer)

**Pan/Zoom Interaction:**
When user pans chart:
- Chart input handler (chart_widget.rs) updates chart.chart_state.camera
- New camera bounds computed for symbol A
- But chart.data still points to B's buffer
- Next frame: candles don't move (they're B's), grid moves (it's A's pan)

This creates the frozen candles effect.

---

## Critical Code Locations

### 1. Message Has No Symbol Guard
**File:** app.rs:331
```rust
DataLoaded(ChartId, Result<Arc<CandleBuffer>, String>),
```

Only carries ChartId. A stale task's result is indistinguishable from the current task's result.

### 2. No Verification in Handler
**File:** app.rs:2123-2134
```rust
if let Some(chart) = self.charts.get_mut(&chart_id) {
    let sym = chart.symbol.clone();
    // ASSUMPTION: buffer is for sym
    // NO VERIFICATION
    Self::apply_candle_data(chart, buffer, true);
}
```

### 3. apply_candle_data Overwrites Blindly
**File:** app.rs:1909
```rust
chart.data = Some(Arc::clone(&buffer));
```

No check that buffer matches the chart's current symbol.

### 4. Snapshot Captures Mismatched State
**File:** app/views.rs:856-857
```rust
symbol: chart.symbol.clone(),
data: Some(Arc::clone(data)),
```

Reads chart.symbol and chart.data in separate lines. If they become mismatched between reads (impossible here, but impossible to reason about concurrently), rendering breaks.

---

## Why the Arc Doesn't Help

**File:** app.rs:113
```rust
pub data: Option<Arc<CandleBuffer>>,
```

Yes, data is an Arc. But this doesn't prevent the symbol-data mismatch. The Arc just means multiple holders can reference the same buffer. The problem is that chart.symbol and chart.data point to different symbols' data.

---

## Fix Requirements

Message must carry symbol or generation:

Option A (Symbol):
```rust
DataLoaded(ChartId, SymbolKey, Result<Arc<CandleBuffer>, String>),
```

Then check:
```rust
if let Some(chart) = self.charts.get_mut(&chart_id) {
    let current_sym = chart.bound_symbol.clone().unwrap_or_default();
    if current_sym != loaded_sym {
        return Task::none();  // Discard stale load
    }
    Self::apply_candle_data(chart, buffer, true);
}
```

Option B (Generation Counter):
Track a load_generation on each chart, increment on every bind_chart_to_symbol, include in async task, check on arrival.

---

## Exact Bug Point

**app.rs:2124 reads chart.symbol AFTER the symbol has been changed, but applies data that was loaded for the OLD symbol.**

```
2124:  let sym = chart.symbol.clone();  // "A" (current)
2134:  Self::apply_candle_data(chart, buffer, true);  // buffer is B's
```

The buffer was fetched asynchronously for symbol B, but the chart's symbol has since moved to A. The handler has no way to detect this race condition because the Message carries no symbol.

