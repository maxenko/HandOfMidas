# 05 - Data Flow: Write-Behind Cache and Message Routing

## Overview

This document specifies the complete data loading flow when DuckDB is integrated
as the L2 cache layer. It covers the write-behind pattern, every loading
scenario (cache hit, miss, partial), the new `Message` variants, modified
`update()` handlers, startup batching, and future IB streaming integration.

**Prerequisite reads:**
- [02-actor-concurrency.md](02-actor-concurrency.md) -- `DbHandle` actor model
- [03-schema-and-migrations.md](03-schema-and-migrations.md) -- DuckDB schema
- [04-dbhandle-api.md](04-dbhandle-api.md) -- `DbHandle` public API

---

## 1. Write-Behind Cache Pattern

### Why Write-Behind, Not Write-Through

In a **write-through** cache, data is written to DuckDB *before* it is available
to the consumer. In a **write-behind** cache, the consumer receives data
immediately and DuckDB is populated asynchronously afterward.

Write-behind is the correct choice for this architecture because of three
hard constraints:

**Constraint 1: GPU demands contiguous f32/i64 arrays every frame.**
The renderer calls `CandleBuffer.timestamps[i]`, `CandleBuffer.opens[i]`, etc.
via the `CandleData` trait on every visible candle, every frame. These are
direct slice indexing operations into SoA `Vec`s. Any indirection through
DuckDB -- even a cached prepared statement -- would add 0.1-0.5ms per chart per
frame. With 20 charts at 60fps, that is 120-600ms/second of pure overhead.

**Constraint 2: DuckDB operations are blocking C++ FFI.**
`Connection::prepare()`, `Statement::query()`, `Appender::flush()` all call
into DuckDB's C++ engine. These cannot run on tokio's async worker pool without
risking thread starvation. They run on a dedicated OS thread via the
`MailboxProcessor::new_blocking()` actor.

**Constraint 3: Data must be available before the write completes.**
When a user types "AAPL" and presses Enter, the chart must render data within
one frame (16ms). A DuckDB insert of 5000 candles takes ~10ms. If the chart
waited for the write to complete, that is 10ms of blank screen with no
backpressure signal to the user.

### Write-Behind in Practice

```
User types "AAPL" + Enter
    |
    v
PanelSymbolSubmitted(chart_id)
    |
    v
[1] Query DuckDB: db.query_candles(key)     -- async, ~5ms
    |                                        -- chart shows "Loading..." state
    v
[2a] Cache HIT (non-empty CandleBuffer)?
    |-- YES --> DataLoaded(id, Ok(Arc<CandleBuffer>))
    |           Chart renders immediately. Done.
    |
    |-- NO ---> DataCacheMiss(id, key)
                |
                v
[3] Load from TestDataProvider/CSV/IB       -- sync for test data, async for IB
    |
    v
    CandleBuffer available
    |
    +---> chart.data = Some(Arc::new(buffer))    -- chart renders immediately
    |
    +---> fire_and_forget: db.insert_candles()   -- async, ~10ms, no reply needed
          |
          v
          DataCacheWritten(id, Ok(count))         -- optional, for logging only
```

The critical insight: the chart renders from `Arc<CandleBuffer>` in L1.
DuckDB is only consulted at load time (symbol change, timeframe change, app
startup). Once data is in L1, DuckDB is invisible to the rendering path.

### Write-Behind Guarantees

| Property | Guarantee |
|----------|-----------|
| **Durability** | Eventual. Data reaches DuckDB within 1 second of load. |
| **Ordering** | Guaranteed. Single actor thread processes inserts sequentially. |
| **Idempotency** | Yes. `INSERT OR REPLACE` handles duplicate timestamps. |
| **Loss window** | If app crashes before write-behind completes, data is re-fetched from L3 on next startup. No data corruption. |
| **Atomicity** | Per-insert. A partial write leaves valid rows; the next load re-fetches and fills gaps. |

---

## 2. Complete Data Loading Flows

### 2.1 Scenario: Cache Hit (Happy Path)

The symbol+timeframe combination exists in DuckDB with sufficient data.

**Steps:**

1. User types "AAPL" in chart panel's symbol input, presses Enter.
2. `Message::PanelSymbolSubmitted(chart_id)` fires.
3. `update()` reads the symbol from `chart.symbol_input`, uppercases it.
4. `update()` sets `chart.symbol = "AAPL"`, `chart.load_state = LoadState::Loading`.
5. `update()` checks `self.store`:
   - `Some(ref store)` -- DuckDB is available.
   - Constructs `DataKey { symbol: "AAPL", timeframe: Timeframe::D1 }`.
   - Returns `Task::perform(async { store.query_candles(key).await }, ...)`.
6. Tokio runtime executes the query. `DbHandle.send()` sends `DbCommand::QueryCandles`
   to the actor thread. Actor thread runs the SQL query (~5ms), materializes
   result to `CandleBuffer`, sends `DbReply::Candles(Ok(buffer))` back.
7. `Task::perform` callback receives `Ok(buffer)` where `buffer.len() > 0`.
   Maps to `Message::DataLoaded(chart_id, Ok(Arc::new(buffer)))`.
8. `update()` handles `DataLoaded`:
   - `chart.data = Some(arc_buffer)`.
   - `chart.load_state = LoadState::Loaded`.
   - `chart.chart_state.dirty.mark_data()`.
   - Resets camera to show last 200 candles.
   - Updates status bar: `"AAPL: 2520 candles at 1D (cached)"`.
9. Next frame renders the chart with data. Total latency: ~5-8ms.

### 2.2 Scenario: Cache Miss (First Load)

The symbol has never been loaded before. DuckDB returns empty.

**Steps:**

1. Steps 1-5 same as cache hit.
2. Actor thread query returns empty `CandleBuffer` (no rows match).
3. `Task::perform` callback receives `Ok(buffer)` where `buffer.is_empty()`.
   Maps to `Message::DataCacheMiss(chart_id, key)`.
4. `update()` handles `DataCacheMiss`:
   - Loads data from `TestDataProvider::get_candles()` (sync, ~2ms).
   - Installs buffer: `chart.data = Some(Arc::new(buffer))`.
   - `chart.load_state = LoadState::Loaded`.
   - Marks dirty, resets camera (same as existing `load_test_data_for_chart()`).
   - **Write-behind:** if `self.store` is `Some`, fires
     `db.fire_and_forget(DbCommand::InsertCandles { key, buffer: buffer_clone })`.
   - Optionally returns a `Task::perform` for the insert to get confirmation.
   - Updates status bar: `"AAPL: 2520 candles at 1D (fetched)"`.
5. Chart renders immediately from L1. DuckDB write completes ~10ms later.
6. On next app launch, the same symbol is a cache hit.

### 2.3 Scenario: Partial Cache (Future, IB Integration)

DuckDB has data up to 2025-12-31 but today is 2026-03-31. Need to fetch
3 months of missing data from IB and merge.

**Steps:**

1. Steps 1-5 same as cache hit.
2. Actor thread returns `CandleBuffer` with data ending at `2025-12-31 00:00:00 UTC`.
3. `Task::perform` callback checks staleness:
   - `buffer.timestamps.last()` = `1735689600000` (2025-12-31).
   - Expected last timestamp for D1: previous trading day's market close.
   - Gap detected: data is stale.
4. Maps to `Message::DataLoaded(chart_id, Ok(Arc::new(partial_buffer)))`.
   - **Immediately renders** what we have. User sees chart with data through Dec 2025.
5. A **second task** is spawned to fetch the gap from L3 (IB API):
   ```rust
   Task::perform(
       async move {
           let gap = ib_client.req_historical_data(
               &symbol, timeframe, stale_end_ts, now_ts,
           ).await;
           gap
       },
       move |result| Message::DataGapFilled(chart_id, key, result),
   )
   ```
6. `Message::DataGapFilled` handler:
   - Merges `gap_buffer` with existing `chart.data` (append, since timestamps
     are strictly after existing data).
   - Replaces `chart.data` with merged `Arc<CandleBuffer>`.
   - Marks dirty for re-render.
   - Fire-and-forget: insert gap data into DuckDB.
   - Fire-and-forget: update `meta.data_ranges` with new `last_ts`.

**Note:** Partial cache handling is a Phase 2 feature (IB integration). Phase 1
uses the simpler hit/miss binary: if data exists, use it; if not, fetch all
from TestDataProvider.

### 2.4 Scenario: DuckDB Unavailable (Graceful Fallback)

`self.store` is `None` (disabled in config, or failed to open).

**Steps:**

1. `Message::PanelSymbolSubmitted(chart_id)` fires.
2. `update()` checks `self.store` -- it is `None`.
3. Falls through directly to `load_test_data_for_chart()` (existing synchronous path).
4. Chart renders immediately. No DuckDB interaction at all.
5. This is **identical to the pre-DuckDB behavior**. Zero regression risk.

### 2.5 Scenario: DuckDB Query Error

The actor thread encounters an error (corrupted file, OOM, etc.).

**Steps:**

1. Steps 1-5 same as cache hit.
2. Actor thread returns `Err(StoreError::QueryFailed(...))`.
3. `Task::perform` callback receives `Err(e)`.
   Maps to `Message::DataCacheMiss(chart_id, key)` -- same as empty result.
4. `tracing::warn!("DuckDB query failed for {key:?}: {e}")`.
5. Falls through to TestDataProvider. User sees data. No visible error.
6. **No write-behind attempted** for this load (DuckDB may be in a bad state).

---

## 3. New Message Variants

Add these variants to the existing `Message` enum in `app.rs`:

```rust
pub enum Message {
    // ... existing variants unchanged ...

    // -- Data cache (DuckDB integration) --

    /// DuckDB returned empty or errored for this chart's symbol+timeframe.
    /// Triggers fallback to TestDataProvider/CSV/IB, then write-behind to DuckDB.
    DataCacheMiss(ChartId, DataKey),

    /// Write-behind insert to DuckDB completed. Used for logging/diagnostics only.
    /// The chart is already rendering from L1 -- this confirmation is non-critical.
    DataCacheWritten(ChartId, Result<usize, String>),

    // -- Future: partial cache / gap fill (Phase 2, IB integration) --
    // DataGapFilled(ChartId, DataKey, Result<CandleBuffer, String>),
}
```

### Why `DataCacheMiss` carries `DataKey`

The `DataCacheMiss` handler needs to know the symbol and timeframe to:
1. Load data from the fallback provider.
2. Construct the correct `DataKey` for the fire-and-forget write-behind.

Without `DataKey`, the handler would have to re-read the chart's state (which
may have changed if the user rapidly typed a new symbol between the query being
dispatched and the miss being received). Carrying `DataKey` in the message
makes the handler idempotent and race-free.

### Why `DataCacheWritten` carries `Result<usize, String>` not `Result<usize, StoreError>`

`StoreError` may not implement `Clone` (it wraps `duckdb::Error` which is not
`Clone`). The iced `Message` enum must be `Clone`. Converting to `String` at
the boundary is the standard iced pattern (matching the existing
`DataLoaded(ChartId, Result<Arc<CandleBuffer>, String>)` variant).

---

## 4. Modified update() Handlers

### 4.1 PanelSymbolSubmitted -- Check DuckDB First

```rust
Message::PanelSymbolSubmitted(chart_id) => {
    self.focus_chart(chart_id);
    let symbol = if let Some(chart) = self.charts.get(&chart_id) {
        chart.symbol_input.trim().to_uppercase()
    } else {
        return Task::none();
    };
    if symbol.is_empty() {
        return Task::none();
    }

    let tf = self
        .charts
        .get(&chart_id)
        .map(|c| c.timeframe)
        .unwrap_or(Timeframe::D1);

    // Update chart state immediately (symbol, loading indicator).
    if let Some(chart) = self.charts.get_mut(&chart_id) {
        chart.symbol = symbol.clone();
        chart.symbol_input = symbol.clone();
        chart.load_state = LoadState::Loading;
    }

    self.mark_config_dirty();

    // Try DuckDB cache first, fall through to direct load if store is None.
    if let Some(ref store) = self.store {
        let store = store.clone();
        let key = DataKey {
            symbol: symbol.clone(),
            timeframe: tf,
        };
        Task::perform(
            async move { store.query_candles(key.clone()).await },
            move |result| match result {
                Ok(buf) if !buf.is_empty() => {
                    Message::DataLoaded(chart_id, Ok(Arc::new(buf)))
                }
                Ok(_empty) => {
                    Message::DataCacheMiss(
                        chart_id,
                        DataKey { symbol, timeframe: tf },
                    )
                }
                Err(e) => {
                    tracing::warn!("DuckDB query error: {e}");
                    Message::DataCacheMiss(
                        chart_id,
                        DataKey { symbol, timeframe: tf },
                    )
                }
            },
        )
    } else {
        // No store available -- direct load (pre-DuckDB behavior).
        self.load_test_data_for_chart(chart_id, &symbol, tf, true);
        Task::none()
    }
}
```

### 4.2 DataLoaded -- Existing Handler, Now Activated

The existing `DataLoaded` handler is currently a no-op stub (all data loads
synchronously via TestDataProvider). With DuckDB, it becomes the primary
success path for cache hits:

```rust
Message::DataLoaded(chart_id, result) => {
    match result {
        Ok(buffer) => {
            if let Some(chart) = self.charts.get_mut(&chart_id) {
                let len = buffer.len();
                chart.data = Some(buffer.clone());
                chart.load_state = LoadState::Loaded;
                chart.chart_state.dirty.mark_data();

                if len > 0 {
                    // Set data bounds for scroll clamping.
                    if chart.chart_state.collapse_gaps {
                        chart.chart_state.data_time_start = 0.0;
                        chart.chart_state.data_time_end = len as f64;
                    } else {
                        let first_ts = buffer.timestamps[0] as f64;
                        let last_ts = buffer.timestamps[len - 1] as f64;
                        chart.chart_state.data_time_start = first_ts;
                        chart.chart_state.data_time_end = last_ts;
                    }

                    // Position camera to show last 200 candles.
                    let visible_count = 200.min(len);

                    if chart.chart_state.collapse_gaps {
                        let start_idx = (len - visible_count) as f64;
                        let end_idx = len as f64 + (visible_count as f64 * 0.05);
                        chart.chart_state.camera.time_start = start_idx;
                        chart.chart_state.camera.time_end = end_idx;
                    } else {
                        let last_ts = buffer.timestamps[len - 1] as f64;
                        let first_visible_ts =
                            buffer.timestamps[len - visible_count] as f64;
                        chart.chart_state.camera.time_start = first_visible_ts;
                        chart.chart_state.camera.time_end =
                            last_ts + (last_ts - first_visible_ts) * 0.05;
                    }

                    let range = (len - visible_count)..len;
                    let (low, high) = buffer.price_range(range);
                    let padding = (high - low) as f64 * 0.05;
                    chart.chart_state.camera.price_low = low as f64 - padding;
                    chart.chart_state.camera.price_high = high as f64 + padding;

                    chart.chart_state.dirty.mark_camera();
                }

                self.status_message = format!(
                    "{}: {} candles at {} (cached)",
                    chart.symbol, len, chart.timeframe.display_name()
                );
            }
        }
        Err(e) => {
            tracing::warn!("Data load error for chart {chart_id}: {e}");
            if let Some(chart) = self.charts.get_mut(&chart_id) {
                chart.load_state = LoadState::Error(e);
            }
        }
    }
    Task::none()
}
```

**Note:** The camera-reset logic in `DataLoaded` duplicates what is currently
in `load_test_data_for_chart()`. This should be extracted into a shared helper
method `install_candle_data(chart_id, buffer, reset_camera)` to avoid
duplication. The same helper serves both `DataLoaded` and `DataCacheMiss`.

```rust
impl MidasApp {
    /// Install a loaded CandleBuffer into a chart panel.
    ///
    /// Sets data, marks dirty, and optionally resets the camera to show
    /// the last 200 candles. Called from both DataLoaded (cache hit) and
    /// DataCacheMiss (fallback load) handlers.
    fn install_candle_data(
        &mut self,
        chart_id: ChartId,
        buffer: Arc<CandleBuffer>,
        reset_camera: bool,
        source_label: &str,
    ) {
        if let Some(chart) = self.charts.get_mut(&chart_id) {
            let len = buffer.len();
            chart.data = Some(buffer.clone());
            chart.load_state = LoadState::Loaded;
            chart.chart_state.dirty.mark_data();

            if len > 0 {
                // Set data bounds for scroll clamping.
                if chart.chart_state.collapse_gaps {
                    chart.chart_state.data_time_start = 0.0;
                    chart.chart_state.data_time_end = len as f64;
                } else {
                    let first_ts = buffer.timestamps[0] as f64;
                    let last_ts = buffer.timestamps[len - 1] as f64;
                    chart.chart_state.data_time_start = first_ts;
                    chart.chart_state.data_time_end = last_ts;
                }

                if reset_camera {
                    let visible_count = 200.min(len);

                    if chart.chart_state.collapse_gaps {
                        let start_idx = (len - visible_count) as f64;
                        let end_idx = len as f64 + (visible_count as f64 * 0.05);
                        chart.chart_state.camera.time_start = start_idx;
                        chart.chart_state.camera.time_end = end_idx;
                    } else {
                        let last_ts = buffer.timestamps[len - 1] as f64;
                        let first_visible_ts =
                            buffer.timestamps[len - visible_count] as f64;
                        chart.chart_state.camera.time_start = first_visible_ts;
                        chart.chart_state.camera.time_end =
                            last_ts + (last_ts - first_visible_ts) * 0.05;
                    }

                    let range = (len - visible_count)..len;
                    let (low, high) = buffer.price_range(range);
                    let padding = (high - low) as f64 * 0.05;
                    chart.chart_state.camera.price_low = low as f64 - padding;
                    chart.chart_state.camera.price_high = high as f64 + padding;

                    chart.chart_state.dirty.mark_camera();
                }
            }

            self.status_message = format!(
                "{}: {} candles at {} ({})",
                chart.symbol, len, chart.timeframe.display_name(), source_label,
            );
        }
    }
}
```

### 4.3 DataCacheMiss -- Fallback + Write-Behind

```rust
Message::DataCacheMiss(chart_id, key) => {
    // Load from L3 fallback (TestDataProvider for now, IB API in future).
    let days = match key.timeframe.as_secs() {
        s if s >= Timeframe::W1.as_secs() => 3650,
        s if s >= Timeframe::D1.as_secs() => 730,
        s if s >= Timeframe::H1.as_secs() => 90,
        s if s >= Timeframe::M15.as_secs() => 30,
        _ => 10,
    };

    let buffer = self.test_data.get_candles(&key.symbol, key.timeframe, days);
    let buffer = Arc::new(buffer);

    // Install into chart immediately -- user sees data this frame.
    self.install_candle_data(chart_id, buffer.clone(), true, "fetched");

    // Write-behind: persist to DuckDB asynchronously.
    if let Some(ref store) = self.store {
        let store = store.clone();
        let key_clone = key.clone();
        // Clone the inner CandleBuffer for the insert (Arc -> owned).
        let buffer_owned = (*buffer).clone();

        Task::perform(
            async move {
                store
                    .insert_candles(key_clone, buffer_owned)
                    .await
                    .map_err(|e| e.to_string())
            },
            move |result| Message::DataCacheWritten(chart_id, result),
        )
    } else {
        Task::none()
    }
}
```

### 4.4 DataCacheWritten -- Optional Logging

```rust
Message::DataCacheWritten(chart_id, result) => {
    match result {
        Ok(count) => {
            tracing::debug!(
                "Write-behind complete for chart {chart_id}: {count} candles persisted"
            );
        }
        Err(e) => {
            tracing::warn!(
                "Write-behind failed for chart {chart_id}: {e}"
            );
            // No user-visible error. The chart is already rendering from L1.
            // Data will be re-fetched from L3 on next app launch.
        }
    }
    Task::none()
}
```

### 4.5 PanelTimeframeSelected -- Timeframe Change with Cache

When the user changes the timeframe on an existing chart, the same
cache-first flow applies:

```rust
Message::PanelTimeframeSelected(chart_id, tf) => {
    self.focus_chart(chart_id);
    let symbol = self
        .charts
        .get(&chart_id)
        .map(|c| c.symbol.clone())
        .unwrap_or_default();

    if let Some(chart) = self.charts.get_mut(&chart_id) {
        chart.timeframe = tf;
        chart.chart_state.dirty.mark_camera();
    }

    self.mark_config_dirty();

    if symbol.is_empty() {
        return Task::none();
    }

    // Same cache-first flow as PanelSymbolSubmitted.
    if let Some(ref store) = self.store {
        let store = store.clone();
        let key = DataKey {
            symbol: symbol.clone(),
            timeframe: tf,
        };
        Task::perform(
            async move { store.query_candles(key.clone()).await },
            move |result| match result {
                Ok(buf) if !buf.is_empty() => {
                    Message::DataLoaded(chart_id, Ok(Arc::new(buf)))
                }
                _ => Message::DataCacheMiss(
                    chart_id,
                    DataKey { symbol, timeframe: tf },
                ),
            },
        )
    } else {
        self.load_test_data_for_chart(chart_id, &symbol, tf, true);
        Task::none()
    }
}
```

---

## 5. Startup Sequence

### Detailed Timeline

```
T+0ms:    main() starts.
          - Initialize tracing subscriber.
          - Call MidasApp::new().

T+2ms:    Load config.toml from AppData/Local/HandOfMidas/config.toml.
          - Deserialize StoreConfig from [store] section (or use defaults).
          - Deserialize chart configs, level configs, window geometry.

T+5ms:    Open main window via iced daemon.
          - window::open() returns (window::Id, Task).

T+10ms:   Attempt DbHandle::open() if store.enabled == true.
          - Spawns dedicated "duckdb-store" OS thread.
          - Thread opens Connection, runs schema migrations.
          - On success: self.store = Some(handle).
          - On failure: tracing::warn!(), self.store = None.

T+15ms:   Build MidasApp struct with charts from config.
          - Each chart has symbol + timeframe from config, data = None.
          - Cameras restored from saved positions.
          - LevelStore populated from config.

T+20ms:   Build startup data-load tasks (see Section 6 below).
          - One Task::perform per chart with a non-empty symbol.
          - All tasks batched via Task::batch().

T+25ms:   MidasApp::new() returns (app, Task::batch([open_task, load_tasks])).
          - iced runtime begins processing tasks concurrently.

T+30ms:   First frame rendered (charts show loading indicators or empty state).

T+35-     DuckDB queries complete asynchronously:
 90ms:    - Cache hits: DataLoaded messages arrive, charts render data.
          - Cache misses: DataCacheMiss messages arrive, fallback loads fire.

T+90-     Fallback loads complete (TestDataProvider is sync, ~2ms each).
 100ms:   - All charts now have data and are rendering.

T+100-    Write-behind inserts for cache misses complete.
 200ms:   - DuckDB now has data for next startup.

T+200ms:  Steady state. All charts rendered. DuckDB idle.
```

### First Launch (Empty DuckDB)

On first launch, DuckDB has no cached data. Every chart triggers
`DataCacheMiss`. The timeline looks like:

```
T+35ms:   20 DuckDB queries fire (all return empty).
T+45ms:   20 DataCacheMiss messages arrive.
T+47ms:   20 TestDataProvider.get_candles() calls (~2ms each, but sequential
          within update() -- iced processes messages one at a time).
T+87ms:   All 20 charts have data and are rendering.
T+87ms:   20 fire_and_forget insert_candles() tasks dispatched.
T+200ms:  All 20 inserts complete. DuckDB populated.
```

Total time to first rendered chart: ~47ms.
Total time to all charts rendered: ~87ms.
Total time to DuckDB fully populated: ~200ms.

### Second Launch (Warm DuckDB)

On second launch, all data is in DuckDB:

```
T+35ms:   20 DuckDB queries fire (all return data, ~5ms each).
T+40ms:   First DataLoaded arrives. First chart renders.
T+55ms:   All 20 DataLoaded messages processed. All charts rendered.
```

Total time to first rendered chart: ~40ms.
Total time to all charts rendered: ~55ms.
No TestDataProvider calls. No write-behind. Pure cache hit path.

---

## 6. Batching 20+ Chart Loads at Startup

### The Problem

On startup, `MidasApp::new()` restores N chart panels from config, each with
a symbol and timeframe. Currently, `load_test_data_for_chart()` is called
synchronously in a loop for each chart. With DuckDB, each load becomes an
async query.

### The Solution: Task::batch()

iced's `Task::batch()` accepts a `Vec<Task<Message>>` and runs them all
concurrently on the tokio runtime. Each task independently queries DuckDB
and produces a `Message::DataLoaded` or `Message::DataCacheMiss`.

```rust
impl MidasApp {
    /// Build startup tasks for loading data into all restored charts.
    ///
    /// When DuckDB is available, each chart gets an async cache query.
    /// When DuckDB is unavailable, data loads synchronously (pre-DuckDB behavior).
    fn build_startup_load_tasks(&mut self) -> Task<Message> {
        // Collect chart IDs and their data keys.
        let chart_keys: Vec<(ChartId, String, Timeframe)> = self
            .charts
            .iter()
            .filter(|(_, panel)| !panel.symbol.is_empty())
            .map(|(&id, panel)| (id, panel.symbol.clone(), panel.timeframe))
            .collect();

        if chart_keys.is_empty() {
            return Task::none();
        }

        match self.store {
            Some(ref store) => {
                // DuckDB available: batch async queries.
                let tasks: Vec<Task<Message>> = chart_keys
                    .into_iter()
                    .map(|(chart_id, symbol, tf)| {
                        let store = store.clone();
                        let key = DataKey {
                            symbol: symbol.clone(),
                            timeframe: tf,
                        };

                        Task::perform(
                            async move { store.query_candles(key.clone()).await },
                            move |result| match result {
                                Ok(buf) if !buf.is_empty() => {
                                    Message::DataLoaded(
                                        chart_id,
                                        Ok(Arc::new(buf)),
                                    )
                                }
                                _ => Message::DataCacheMiss(
                                    chart_id,
                                    DataKey {
                                        symbol,
                                        timeframe: tf,
                                    },
                                ),
                            },
                        )
                    })
                    .collect();

                Task::batch(tasks)
            }
            None => {
                // No store: load synchronously (existing behavior).
                for (id, symbol, tf) in chart_keys {
                    self.load_test_data_for_chart(id, &symbol, tf, false);
                }
                Task::none()
            }
        }
    }
}
```

### Modified MidasApp::new()

```rust
pub fn new() -> (Self, Task<Message>) {
    // ... existing config loading, window opening, chart restoration ...

    let mut app = Self {
        // ... existing fields ...
        store: None,  // initialized below
        test_data: TestDataProvider::new(),
    };

    // Open DuckDB store if enabled.
    if store_config.enabled {
        let store_path = resolve_store_path(&store_config);
        match DbHandle::open(&store_path) {
            Ok(handle) => {
                tracing::info!("DuckDB store opened at {}", store_path.display());
                app.store = Some(handle);
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to open DuckDB store at {}: {e}. \
                     Falling back to direct data loading.",
                    store_path.display()
                );
                // app.store remains None -- graceful fallback.
            }
        }
    }

    // Build data loading tasks (async DuckDB or sync TestDataProvider).
    let load_tasks = app.build_startup_load_tasks();

    // Combine window-open task with data-load tasks.
    let startup_tasks = Task::batch(vec![open_task, load_tasks]);

    (app, startup_tasks)
}
```

### Concurrency at the Actor Level

All 20 `Task::perform` tasks run concurrently on tokio. However, they all send
`DbCommand::QueryCandles` to the **same** `MailboxProcessor` actor thread.
The actor processes them sequentially (one query at a time, ~5ms each).

With 20 charts:
- Total serialized query time: ~100ms.
- But tokio tasks are suspended (not blocking threads) while waiting for replies.
- The actor thread is the bottleneck, not tokio.

This is acceptable for v1. If profiling shows this is too slow, the actor can
be upgraded to a read pool (see research doc `02-architecture.md`, "v1 vs
Future Read Pool").

---

## 7. Future: IB Streaming Integration

### Architecture

When IB API streaming is integrated, live market data arrives as ticks (price,
size, timestamp). These ticks are aggregated into candles in memory:

```
IB WebSocket --> midas-feed::Aggregator --> CandleBuffer.push() / .update_last()
                                              |
                                              +--> Chart re-renders (L1)
                                              |
                                              +--> Batch flush to DuckDB every 5s (L2)
```

### Forming Candle vs Closed Candle

At any given moment, the last candle in the buffer is the **forming candle**.
It is updated on every tick (`CandleBuffer::update_last()`). Only closed
candles (where the candle's time period has elapsed) are flushed to DuckDB.

```rust
/// State held by the tick aggregator for one symbol+timeframe.
struct AggregatorState {
    /// The current forming candle's data.
    forming: Option<FormingCandle>,
    /// Closed candles not yet flushed to DuckDB.
    pending_flush: Vec<ClosedCandle>,
    /// Last flush timestamp.
    last_flush: Instant,
}

struct FormingCandle {
    open_ts: i64,
    open: f32,
    high: f32,
    low: f32,
    close: f32,
    volume: u32,
}

struct ClosedCandle {
    timestamp_ms: i64,
    open: f32,
    high: f32,
    low: f32,
    close: f32,
    volume: u32,
}
```

### Timer-Based Batch Flush

A tokio interval timer fires every `flush_interval_secs` (default: 5 seconds).
On each tick, the aggregator:

1. Checks `pending_flush` for each active symbol.
2. If non-empty, constructs a `CandleBuffer` from the pending candles.
3. Calls `store.fire_and_forget(DbCommand::InsertCandles { key, buffer })`.
4. Clears `pending_flush`.

```rust
// In MidasApp or a dedicated StreamManager:
fn setup_flush_timer(store: DbHandle, flush_interval: Duration) -> Task<Message> {
    // iced does not have a built-in interval. Use Subscription::run
    // with a tokio::time::interval inside.
    //
    // Alternatively, use the existing Tick message (fires every second)
    // with a modular counter:
    //   if tick_count % flush_interval_secs == 0 { flush_pending() }
    Task::none() // placeholder -- actual implementation uses Subscription
}
```

### Why the Forming Candle Stays in Memory Only

1. **It changes every tick.** Writing to DuckDB on every tick (100-500/second)
   would overwhelm the actor thread.
2. **It is not a closed candle.** Its OHLCV values are provisional. Writing it
   would create a row that needs to be UPDATEd on every tick, which is
   anti-pattern for an append-only time-series store.
3. **Loss is acceptable.** If the app crashes mid-candle, the forming candle
   is lost. On restart, the candle is re-fetched from IB's historical data API
   which returns the final closed values.

---

## 8. Data Staleness Detection

### The Problem

DuckDB may have cached data for AAPL at D1 with `last_ts = 2026-03-28 00:00 UTC`
(Friday). Today is Monday 2026-03-31. The cache is stale: it is missing one
trading day.

### Detection Strategy

After a cache hit, compare `buffer.timestamps.last()` against the expected
last trading timestamp:

```rust
/// Check whether cached data is stale (missing recent trading sessions).
///
/// Returns true if the cache is likely missing data that should exist.
fn is_cache_stale(buffer: &CandleBuffer, timeframe: Timeframe) -> bool {
    let last_ts_ms = match buffer.timestamps.last() {
        Some(&ts) => ts,
        None => return true, // empty buffer is "stale"
    };

    let now_ms = chrono::Utc::now().timestamp_millis();

    // Staleness threshold: if the gap between last cached timestamp and
    // now exceeds 2x the timeframe period, the cache is probably stale.
    //
    // For D1: 2 * 86400 * 1000 = 172_800_000ms (2 days).
    //   This handles weekends (Fri data, checked on Mon = 2.x days gap).
    //   3-day weekends would trigger a stale check, which is correct.
    //
    // For M5: 2 * 300 * 1000 = 600_000ms (10 minutes).
    //   Any gap > 10 minutes triggers re-fetch.
    let threshold_ms = 2 * timeframe.as_secs() as i64 * 1000;

    // Only consider staleness during market hours (crude check).
    // A more sophisticated check would use a market calendar.
    let gap = now_ms - last_ts_ms;
    gap > threshold_ms
}
```

### Staleness Flow (Future)

Phase 2 introduces a staleness check after every cache hit:

```
DataLoaded(chart_id, Ok(buffer))
    |
    v
is_cache_stale(&buffer, timeframe)?
    |
    +-- NO:  Done. Cache is fresh.
    |
    +-- YES: Render what we have, then spawn a gap-fill task.
             Task::perform(
                 ib_client.req_historical_data(symbol, tf, last_ts, now),
                 |result| Message::DataGapFilled(chart_id, key, result),
             )
```

For Phase 1 (TestDataProvider only), staleness is not meaningful because
TestDataProvider generates deterministic data up to mid-2026. Staleness
detection is implemented but not acted upon until IB integration.

### meta.data_ranges Integration

The `meta.data_ranges` table stores the last known timestamp for each
symbol+timeframe. This enables O(1) staleness checks without scanning the
full `market.candles` table:

```sql
SELECT last_ts FROM meta.data_ranges
WHERE symbol = 'AAPL' AND timeframe_secs = 86400;
```

When `last_ts` < expected market close, the data is stale. The staleness
check can be done in the actor thread as part of `DbCommand::QueryCandles`,
returning a `CandleBuffer` plus a `stale: bool` flag.

---

## 9. Sequence Diagrams

### 9.1 Cache Hit

```
  User         MidasApp::update()      Tokio Runtime      DuckDB Actor Thread
   |                 |                      |                     |
   |  Enter "AAPL"   |                      |                     |
   |---------------->|                      |                     |
   |                 | PanelSymbolSubmitted  |                     |
   |                 |                      |                     |
   |                 | chart.load_state =   |                     |
   |                 |   Loading            |                     |
   |                 |                      |                     |
   |                 | Task::perform ------>|                     |
   |                 |   query_candles()    |                     |
   |                 |                      | mb.send(Query) ---->|
   |                 |                      |                     | SELECT ... FROM
   |                 |                      |                     | market.candles
   |                 |                      |                     | (~5ms)
   |                 |                      |<---- DbReply -------|
   |                 |                      |                     |
   |                 |<-- DataLoaded -------|                     |
   |                 |   Ok(Arc<CandleBuffer>)                   |
   |                 |                      |                     |
   |                 | chart.data = Some(buf)                    |
   |                 | chart.load_state =   |                     |
   |                 |   Loaded             |                     |
   |                 | dirty.mark_data()    |                     |
   |                 |                      |                     |
   |  Chart renders  |                      |                     |
   |<----------------|                      |                     |
   |                 |                      |                     |
```

### 9.2 Cache Miss with Write-Behind

```
  User         MidasApp::update()      Tokio Runtime      DuckDB Actor    TestDataProvider
   |                 |                      |                  |                 |
   |  Enter "AAPL"   |                      |                  |                 |
   |---------------->|                      |                  |                 |
   |                 | PanelSymbolSubmitted  |                  |                 |
   |                 |                      |                  |                 |
   |                 | Task::perform ------>|                  |                 |
   |                 |   query_candles()    |                  |                 |
   |                 |                      | mb.send(Query) ->|                 |
   |                 |                      |                  | SELECT ... (0 rows)
   |                 |                      |<--- DbReply -----|                 |
   |                 |                      |  (empty buffer)  |                 |
   |                 |<-- DataCacheMiss ----|                  |                 |
   |                 |   (chart_id, key)    |                  |                 |
   |                 |                      |                  |                 |
   |                 | get_candles() -------|------------------|---------------->|
   |                 |                      |                  |                 | Generate
   |                 |<--- CandleBuffer ----|------------------|-----------------|  (~2ms)
   |                 |                      |                  |                 |
   |                 | chart.data = Some(buf)                  |                 |
   |  Chart renders  |                      |                  |                 |
   |<----------------|                      |                  |                 |
   |                 |                      |                  |                 |
   |                 | Task::perform ------>|                  |                 |
   |                 |   insert_candles()   |                  |                 |
   |                 |                      | mb.send(Insert)->|                 |
   |                 |                      |                  | INSERT INTO ... |
   |                 |                      |                  |   (~10ms)       |
   |                 |                      |<-- DbReply ------|                 |
   |                 |<-- DataCacheWritten--|                  |                 |
   |                 |   Ok(2520)           |                  |                 |
   |                 | tracing::debug!()    |                  |                 |
   |                 |                      |                  |                 |
```

### 9.3 Startup Batch Load (20 Charts)

```
  MidasApp::new()        Tokio Runtime (20 tasks)         DuckDB Actor Thread
       |                          |                              |
       | Task::batch(20 tasks) -->|                              |
       |                          |                              |
       |                          | Task 1: query(AAPL/D1) ----->|
       |                          | Task 2: query(MSFT/D1) ----->| (queued)
       |                          | Task 3: query(TSLA/5m) ----->| (queued)
       |                          | ...                          |
       |                          | Task 20: query(SPY/H1) ----->| (queued)
       |                          |                              |
       |                          |                              | Process AAPL (~5ms)
       |                          |<---- DataLoaded(0, Ok) ------|
       |                          |                              | Process MSFT (~5ms)
       |                          |<---- DataCacheMiss(1) -------|
       |                          |                              | Process TSLA (~5ms)
       |                          |<---- DataLoaded(2, Ok) ------|
       |                          |                              | ...
       |                          |                              | Process SPY (~5ms)
       |                          |<---- DataLoaded(19, Ok) -----|
       |                          |                              |
       |<-- Messages processed ---|                              |
       |    sequentially by       |                              |
       |    iced event loop       |                              |
       |                          |                              |
       | Total: ~100ms for all 20 queries (serialized at actor)  |
       | But charts render as each DataLoaded arrives.           |
       |                                                         |
```

### 9.4 Graceful Fallback (No DuckDB)

```
  User         MidasApp::update()      TestDataProvider
   |                 |                       |
   |  Enter "AAPL"   |                       |
   |---------------->|                       |
   |                 | PanelSymbolSubmitted   |
   |                 |                       |
   |                 | self.store == None     |
   |                 |                       |
   |                 | get_candles() -------->|
   |                 |                       | Generate (~2ms)
   |                 |<--- CandleBuffer -----|
   |                 |                       |
   |                 | chart.data = Some(buf)|
   |  Chart renders  |                       |
   |<----------------|                       |
   |                 |                       |
   | (Identical to pre-DuckDB behavior.     |
   |  No async tasks, no messages, no DuckDB)|
```

---

## 10. Summary: Data Loading Decision Tree

```
PanelSymbolSubmitted(chart_id)
    |
    +-- symbol empty? --> Task::none()
    |
    +-- self.store is None?
    |       |
    |       YES --> load_test_data_for_chart() (sync, existing behavior)
    |
    +-- self.store is Some(store)
            |
            +-- Task::perform(store.query_candles(key))
                    |
                    +-- Ok(buf) && !buf.is_empty()
                    |       |
                    |       +-- DataLoaded(id, Ok(Arc(buf)))
                    |              |
                    |              +-- install_candle_data(reset_camera=true)
                    |              +-- status: "cached"
                    |
                    +-- Ok(empty) OR Err(_)
                            |
                            +-- DataCacheMiss(id, key)
                                   |
                                   +-- TestDataProvider.get_candles()
                                   +-- install_candle_data(reset_camera=true)
                                   +-- status: "fetched"
                                   +-- Task::perform(store.insert_candles())
                                          |
                                          +-- DataCacheWritten(id, Ok(count))
                                                 |
                                                 +-- tracing::debug!()
```
