# Feature: Stale DataLoaded Guard via Generation Counter

## Overview

Fix the ticker-switch race condition where a stale `DataLoaded` message overwrites the chart's candle buffer with data from a previously-requested ticker. Add a generation counter to `ChartPanel` that increments on every load request; carry it in the `DataLoaded` message; discard stale arrivals.

## The Bug

`DataLoaded(ChartId, Result<Arc<CandleBuffer>, String>)` carries only `chart_id`. When the user switches A→B→A rapidly, three async loads are in flight for the same chart_id. If B's response arrives after A's second response, the handler applies B's candle buffer to a chart now bound to A — causing "frozen candles from the wrong ticker."

## Design

### Generation counter on ChartPanel

```rust
// On ChartPanel:
pub load_generation: u64,  // default 0
```

Incremented in `load_symbol_for_chart` before the async load fires:
```rust
chart.load_generation += 1;
```

### Carry generation in DataLoaded

```rust
// Before:
DataLoaded(ChartId, Result<Arc<CandleBuffer>, String>),

// After:
DataLoaded(ChartId, u64, Result<Arc<CandleBuffer>, String>),
//                   ^^^ load_generation at time of request
```

The closure in `load_chart_with` captures the generation:
```rust
let gen = chart.load_generation;
Task::perform(
    async move { provider.get_candles(&symbol, tf, days).await },
    move |result| make_msg(chart_id, gen, result.map(Arc::new).map_err(|e| e.to_string())),
)
```

### Guard in handler

```rust
Message::DataLoaded(chart_id, gen, result) => {
    if let Some(chart) = self.charts.get(&chart_id) {
        if chart.load_generation != gen {
            // Stale load — chart requested newer data since this load started.
            return Task::none();
        }
    }
    // ... proceed with apply_candle_data
}
```

### Also apply to DataRestoredFromStartup

`DataRestoredFromStartup` uses the same `load_chart_with` function. It gets the generation counter automatically. Its handler gets the same guard.

## Files to Modify

- `desktop/win/crates/midas-app/src/app.rs`:
  - Add `load_generation: u64` to `ChartPanel` struct (default 0)
  - Change `Message::DataLoaded(ChartId, Result<...>)` → `Message::DataLoaded(ChartId, u64, Result<...>)`
  - Change `Message::DataRestoredFromStartup(ChartId, Result<...>)` → same pattern
  - Add stale-guard check at the top of both handlers
  - Increment `chart.load_generation` in `load_symbol_for_chart`

- `desktop/win/crates/midas-app/src/app.rs` (`load_chart_with`):
  - Capture `chart.load_generation` before the async task
  - Pass it through the `make_msg` closure
  - Update the `make_msg` type signature: `FnOnce(ChartId, u64, Result<...>) -> Message`

## Tests

- `stale_data_loaded_discarded` — set load_generation=5, fire DataLoaded with gen=3 → discarded, chart.data unchanged
- `current_data_loaded_applied` — set load_generation=5, fire DataLoaded with gen=5 → applied, chart.data updated

## Done When

Switch AMD → NVDA → AMD rapidly. The chart never shows NVDA candles on an AMD-bound chart. Pan/zoom always works. No frozen candles.

## Non-Goals

- Cancelling in-flight async tasks (iced doesn't support task cancellation cleanly)
- Changing the data provider or caching layer
- Refactoring the chart rendering pipeline (the pipeline itself is correct per the research)
