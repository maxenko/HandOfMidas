# Current Data Layer Analysis

Analysis of the existing Hand of Midas data architecture to understand what DuckDB integrates with.

## CandleData Trait (midas-core)

**Location:** `crates/midas-core/src/candle_data.rs`

Object-safe trait for polymorphic candle data access:

```rust
pub trait CandleData {
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool;            // default: len() == 0
    fn timestamp(&self, idx: usize) -> i64;
    fn open(&self, idx: usize) -> f32;
    fn high(&self, idx: usize) -> f32;
    fn low(&self, idx: usize) -> f32;
    fn close(&self, idx: usize) -> f32;
    fn volume(&self, idx: usize) -> u32;
    fn price_range(&self, range: Range<usize>) -> (f32, f32);  // (min_low, max_high)
    fn find_index_by_time(&self, ts: i64) -> usize;            // binary search
}
```

Note: The trait has no `Send + Sync` supertraits. `CandleBuffer` is naturally `Send + Sync` (composed of `Vec` types), so cross-thread sharing via `Arc<CandleBuffer>` works without trait-level bounds.

Key design: stateless indexed accessors, no iterators. Enables zero-copy views via `CandleSlice`.

**Implemented by:** `CandleBuffer`, `CandleSlice`, test fixtures.

## CandleBuffer (midas-data)

**Location:** `crates/midas-data/src/candle.rs`

SoA (Structure-of-Arrays) layout:

```rust
pub struct CandleBuffer {
    pub timestamps: Vec<i64>,    // epoch ms, strictly monotonic
    pub opens:      Vec<f32>,
    pub highs:      Vec<f32>,
    pub lows:       Vec<f32>,
    pub closes:     Vec<f32>,
    pub volumes:    Vec<u32>,
}
```

**Invariants:**
- All six `Vec`s have identical length
- `timestamps` strictly monotonically increasing (debug assert in `push()`)

**Key methods:**
- `push(ts, o, h, l, c, v)` -- append, amortized O(1)
- `slice(range) -> CandleSlice` -- zero-copy borrow, O(1)
- `price_range(range) -> (f32, f32)` -- SIMD auto-vectorized by LLVM
- `find_index_by_time(ts) -> usize` -- O(log n) via `partition_point`
- `update_last(...)` -- replace last candle (real-time forming candle)

**SoA advantages for this project:**
- Cache-friendly contiguous `f32` arrays for indicator loops
- SIMD-vectorizable tight loops (AVX2 on x86_64)
- Direct `bytemuck` cast for GPU instance buffer upload

## Binary Format (midas-data)

**Location:** `crates/midas-data/src/binary.rs`

```
.midas file layout:
+------------------+  128 bytes
|   MidasHeader    |  magic, version, symbol_id, timeframe_secs,
|                  |  start_ts, end_ts, candle_count
+------------------+
|   CandleRecord 0 |  32 bytes: ts(i64), O(f32), H(f32), L(f32), C(f32), V(u32), pad(u32)
|   CandleRecord 1 |
|   ...            |
+------------------+
```

Supported via memmap2 (`MmapCandleFile::open()`). Currently implemented but not used in the desktop app's active data path.

## Data Loading Flow

```
User types symbol in chart
    |
    v
PanelSymbolSubmitted(ChartId, symbol)
    |
    v
load_symbol_for_chart() -> TestDataProvider::get_candles()
    |
    v
Arc<CandleBuffer> stored in ChartPanel.data
    |
    v
chart_state.dirty.mark_data() -> triggers redraw
    |
    v
Renderer consumes via &dyn CandleData
```

**TestDataProvider** (currently the only data source):
- Generates deterministic OHLCV from FNV-1a hash of symbol name
- ~10 years of history per symbol (2016-2026)
- Regime-switching dynamics (Bull/Bear/Consolidation/Crash)
- GARCH(1,1) volatility clustering, overnight gaps
- Cached: `HashMap<(symbol, timeframe), CandleBuffer>`

**CSV import** (`midas-feed::csv`):
- Auto-detect columns from header
- 4 timestamp formats (epoch ms, epoch s, YYYY-MM-DD, YYYY-MM-DDTHH:MM:SS)
- Validates: no negative prices, finite values, no future timestamps
- Sorts ascending, checks for duplicate timestamps
- Implemented and tested, but not wired into the UI flow yet

## Current Data Storage

| Component | Storage | Scope |
|-----------|---------|-------|
| TestDataProvider | HashMap in-memory | Per-symbol-timeframe, app lifetime |
| CandleBuffer | Arc in ChartPanel | Shared across renderer/indicators/UI |
| Binary .midas | memmap2 | Implemented, not used in active flow |
| Broker DB | SQLite (WAL mode) | Orders, fills, positions (separate crate) |

**No persistent OHLCV cache exists today.** Every app restart regenerates test data or reimports CSVs.

## Broker SQLite (midas-broker)

**Location:** `crates/midas-broker/src/db.rs`

- `rusqlite = 0.32` with `bundled` feature
- WAL mode, NORMAL sync, foreign keys, 5s busy timeout
- Tables: orders, order_audit, fills, positions, account_values, contracts
- Access: `Arc<Mutex<Connection>>` + `spawn_blocking()`

This is transactional data. DuckDB should NOT replace it. But DuckDB can ATTACH it read-only for cross-domain analytics.

## What DuckDB Replaces

| Subsystem | Current | DuckDB Replacement |
|-----------|---------|-------------------|
| Data source | TestDataProvider (in-memory) | Query OHLCV from DuckDB |
| Persistent storage | None for OHLCV | `cache.duckdb` file |
| Analytical queries | Hand-written Rust | SQL window functions |
| Cross-symbol scans | Not possible | Single columnar scan |
| Cache lifetime | Single app session | Survives restarts |

## What DuckDB Does NOT Replace

| Subsystem | Stays As-Is | Reason |
|-----------|-------------|--------|
| Hot-path rendering | CandleBuffer + mmap | Zero-copy, zero-overhead |
| Forming candle | CandleBuffer.update_last() | Per-tick updates too fast for DB |
| CandleData trait | &dyn CandleData | Sans-IO boundary, untouched |
| Broker data | SQLite | Transactional, point lookups |
| Config | TOML files | Human-readable |

## Integration Points

- **Entry:** `MidasApp::load_symbol_for_chart()` and `load_test_data_for_chart()` (the `DataProvider` trait is planned but not yet implemented -- currently commented out in `midas-feed/src/lib.rs`)
- **Message flow:** `Message::PanelSymbolSubmitted(ChartId)` triggers symbol loading. New variants needed: `DataCacheMiss`, `DataLoadFailed`
- **Output:** Must produce `Arc<CandleBuffer>` for `ChartPanel.data`
- **Async:** `Message::DataLoaded(ChartId, Result<Arc<CandleBuffer>, String>)` already defined
- **Config:** Extend `AppConfig` with `[store]` section
- **Trait boundary preserved:** `midas-chart` sees only `&dyn CandleData`, never DuckDB types
