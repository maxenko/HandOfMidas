# Data Loading Trace: AMD/ADD/ABC Show Same Chart Bug

## Executive Summary
The data loading architecture is **sound**. Symbol is correctly passed through every layer, and `apply_candle_data` properly swaps the data buffer. The bug is NOT in the core load path.

## Traced Flow

### 1. User Clicks Watchlist Ticker
**File:** `app.rs:3843`
```rust
Message::WatchlistTickerSelected(wl_id, symbol) => {
    // ... 
    for id in targets {
        tasks.push(self.load_symbol_for_chart(id, &symbol));
    }
}
```
✅ Symbol correctly passed to `load_symbol_for_chart`.

### 2. load_symbol_for_chart (Entry Point)
**File:** `app.rs:1605-1630`
- Line 1606: Trims and uppercases the symbol
- Line 1617-1620: Increments `chart.load_generation` and sets `LoadState::Loading`
- Line 1626-1627: Calls `bind_chart_to_symbol` to set `chart.symbol`
- Line 1629: Calls `load_chart_async` with the correct symbol

✅ **Symbol captured correctly.** Generation counter incremented BEFORE async task creation.

### 3. load_chart_async → load_chart_with
**File:** `app.rs:1869-1870`
```rust
fn load_chart_async(&self, chart_id: ChartId, symbol: &str, tf: Timeframe) -> Task<Message> {
    self.load_chart_with(chart_id, symbol, tf, Message::DataLoaded)
}
```

### 4. load_chart_with (Core Provider Call)
**File:** `app.rs:1834-1865`
```rust
fn load_chart_with<F>(..., symbol: &str, ...) -> Task<Message> {
    let gen = self.charts.get(&chart_id).map(|c| c.load_generation).unwrap_or(0);
    let symbol = symbol.to_uppercase();  // Line 1853
    Task::perform(
        async move { provider.get_candles(&symbol, tf, days).await },  // Line 1856
        move |result| make_msg(chart_id, gen, result.map(Arc::new).map_err(...))
    )
}
```
✅ **Symbol is captured by value (line 1853)** into the async closure. No reference issue. `get_candles(&symbol)` uses correct symbol.

### 5. DataLoaded Handler (Generation Guard)
**File:** `app.rs:2128-2138`
```rust
Message::DataLoaded(chart_id, gen, result) => {
    if let Some(chart) = self.charts.get(&chart_id) {
        if chart.load_generation != gen {  // Line 2131
            tracing::debug!("discarding stale DataLoaded (gen {gen} != {})", 
                            chart.load_generation);
            return Task::none();
        }
    }
    // ... continues to apply_candle_data
}
```
✅ **Generation guard logic is CORRECT.** The `gen` value is captured at task creation time (line 1850), so it should match unless the user clicked another ticker.

### 6. apply_candle_data Called
**File:** `app.rs:2160`
```rust
Self::apply_candle_data(chart, buffer, true);
```
✅ **Called unconditionally after generation guard passes.**

### 7. apply_candle_data Implementation
**File:** `app.rs:1924-1945`
```rust
fn apply_candle_data(chart: &mut ChartPanel, buffer: Arc<CandleBuffer>, reset_camera: bool) {
    chart.data = Some(Arc::clone(&buffer));  // Line 1925: SWAPPED
    chart.load_state = LoadState::Loaded;
    chart.chart_state.dirty.mark_data();
    // ... camera reset logic
}
```
✅ **Data is properly swapped.** `chart.data` is set to the new buffer.

## Diagnosis

The code path is **architecturally correct**. No stale symbol capture, no generation mismatch bug, proper data buffer swap.

### If all three tickers show SAME data:
1. **Provider returns same data for all symbols** — Check the provider implementation
2. **Chart.symbol binding fails** — The `bind_chart_to_symbol` call (line 1627) sets `chart.symbol`. Verify this is being used when rendering/retrieving data
3. **Rendering uses stale pointer** — After `apply_candle_data` swaps `chart.data`, verify the rendering code actually reads from `chart.data`, not a cached copy
4. **Generation guard blocks ALL loads** — If every DataLoaded is discarded, `gen` captured at line 1850 would always mismatch `chart.load_generation`. Trace whether a rapid multi-click increments generation faster than loads complete

## Critical Questions to Debug
1. Add logging to `load_symbol_for_chart` line 1606: print `symbol` before `to_uppercase()`
2. Add logging to `load_chart_with` line 1856: print the symbol being passed to `provider.get_candles()`
3. Add logging to `DataLoaded` handler lines 2131-2135: log `gen` vs `chart.load_generation` for each message
4. In rendering code: verify it reads `chart.data` not a previous value

