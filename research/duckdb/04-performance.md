# Performance, Benchmarks, and Alternatives

## DuckDB vs SQLite

### Query Performance

| Query Type | DuckDB | SQLite | Winner |
|---|---|---|---|
| Aggregation (SUM/AVG over 1M rows) | ~50ms | ~1-3s | DuckDB (20-50x) |
| Window functions (moving avg, ATR) | Sub-second at 2M rows | Very slow | DuckDB (10-100x) |
| Range scan (5000 candles by timestamp) | ~0.4ms | ~0.1-0.5ms (indexed) | Tied |
| Point lookup (single row by PK) | ~1-2ms | ~0.05-0.1ms | SQLite (10-20x) |
| Cross-symbol scan (all symbols, one column) | Milliseconds | Seconds | DuckDB (10-100x) |

### Insert Performance

| Method | DuckDB | SQLite |
|---|---|---|
| Bulk (Appender API / batch) | ~500K-1.2M rows/sec | ~50-100K rows/sec |
| Row-by-row streaming | ~63-150K rows/sec | ~10-50K rows/sec |
| Transaction overhead | Higher per-txn | Very low (~50us/commit) |

Both handle IB's ~4 ticks/sec per symbol trivially.

### Memory Usage

| Aspect | DuckDB | SQLite |
|---|---|---|
| Default limit | 80% of RAM (**must override!**) | Page cache, grows as needed |
| Configurable cap | `SET memory_limit = 'XXX'` | `PRAGMA cache_size` |
| Baseline overhead | ~20-50MB | ~1-5MB |
| Spill-to-disk | Yes | No |
| Recommended for this app | 64-256MB | ~32MB |

### Startup Time

| Metric | DuckDB | SQLite |
|---|---|---|
| Cold open (small DB) | ~5-50ms | ~1-5ms |
| Extension loading (each) | ~500-1000ms | N/A |
| Without extensions | ~5-50ms | ~1-5ms |

DuckDB without extensions: well within the 2-second cold start budget.

### Binary Size

| Component | Size |
|-----------|------|
| DuckDB (bundled) | ~30-50MB |
| SQLite (bundled) | ~1-2MB |

### Verdict: What Goes Where

| Data | Storage | Reason |
|------|---------|--------|
| Hot candle data (GPU rendering) | `.midas` binary + mmap | Zero-copy, zero-overhead |
| Historical candle cache (L2) | DuckDB | Fast bulk load, analytical queries |
| Cross-symbol analytics | DuckDB | Columnar scan |
| Indicator pre-computation | DuckDB | Window functions |
| Orders, fills, positions | SQLite (existing) | Transactional, point lookups |
| User settings, watchlists | SQLite | Simple key-value |

## DuckDB Memory Configuration for Desktop

```sql
SET memory_limit = '256MB';           -- Cap for query processing
SET temp_directory = 'data/duckdb_temp';
SET max_temp_directory_size = '1GB';
SET threads = 2;                       -- Don't starve GPU/UI threads
SET enable_progress_bar = false;
```

**Budget analysis (200MB total target):**
- 20 mmap'd binary files: ~3.2MB (OS-managed)
- 20 SoA CandleBuffers: ~2.8MB
- GPU buffers: ~20-40MB
- iced UI framework: ~30-50MB
- DuckDB: 64-256MB configurable
- Headroom needed

Conservative setting: 64MB. Generous: 256MB (if other components stay lean).

## Query Latency for Trading Patterns

| Pattern | Expected Latency | Notes |
|---------|-----------------|-------|
| Range scan: last 5000 candles | ~0.4ms | Zone map pruning |
| Same via mmap'd binary | ~0us | Pointer arithmetic |
| Daily ATR from 1min candles | ~5-20ms | 100K rows, window fn |
| 20-period moving average | ~1-5ms | 5K rows |
| Cross-symbol volume spike | ~10-50ms | 500 symbols |
| Cross-symbol 52-week highs | ~50-200ms | 500 symbols x 252 rows |

## Comparison with Alternatives

### Apache DataFusion

| Factor | DuckDB | DataFusion |
|--------|--------|------------|
| Language | C++ (FFI) | **Pure Rust** |
| Zero-copy from SoA | Requires data copy | Arrow RecordBatch from slices |
| Binary size | 30-50MB | 15-25MB |
| Compile time (bundled) | 10+ min (C++) | 3-8 min (Rust) |
| Compile time (prebuilt) | ~10 sec | 3-8 min |
| Memory overhead | Buffer pool + engine | Stateless, on-demand |
| Persistence | Built-in .duckdb file | **None** (export to Parquet) |
| SQL completeness | More complete | Sufficient |
| ClickBench performance | Fast | Fastest single-node Parquet engine |

**DataFusion is the strongest alternative.** Pure Rust, no FFI, Arrow-native. The existing `CandleBuffer` SoA layout maps almost directly to Arrow arrays. DataFusion lacks built-in persistence (must manage Parquet files), but the app already has `.midas` files.

### Polars

| Factor | Assessment |
|--------|------------|
| Performance | Competitive with DuckDB. 7.7x faster CSV reading. |
| API | DataFrame/LazyFrame (not SQL). Different paradigm. |
| Persistence | None built-in. Serialize to Parquet. |
| Binary size | ~10-20MB |
| Verdict | Good for transform pipelines. No SQL, no persistence. Better as complement than replacement. |

### Others

| Alternative | Verdict |
|-------------|---------|
| redb (Rust KV) | Too simple. No analytics. Good for metadata only. |
| sled | **Abandoned.** Do not use. |
| RocksDB | Overkill. No SQL. Complex tuning. Server-oriented. |

### Summary Matrix

| | DuckDB | DataFusion | Polars | SQLite | redb |
|---|---|---|---|---|---|
| Analytical queries | Excellent | Excellent | Good | Poor | N/A |
| Point lookups | Poor | N/A | N/A | Excellent | Good |
| Persistence | Yes | No | No | Yes | Yes |
| Pure Rust | No | **Yes** | **Yes** | No | **Yes** |
| Binary size | 30-50MB | 15-25MB | 10-20MB | 1-2MB | <1MB |
| SQL support | Full | Full | No | Full | No |
| Desktop suitability | Good | **Excellent** | Good | **Excellent** | Good |

## Hybrid Architecture Recommendation

### Option A: DuckDB (simpler, batteries-included)

```
midas-data: .midas files + mmap (L1)
midas-store: DuckDB .duckdb file (L2, analytical cache)
midas-broker: SQLite (metadata, orders)

Flow: IB API -> buffer -> Appender -> DuckDB -> CandleBuffer -> GPU
```

### Option B: DataFusion (Rust-native, zero-copy)

```
midas-data: .midas files + mmap (L1)
midas-store: Parquet files (L2) + DataFusion (query engine)
midas-broker: SQLite (metadata, orders)

Flow: IB API -> buffer -> CandleBuffer -> .midas + Parquet
     Query: DataFusion SQL -> Arrow RecordBatch -> result
```

### Recommendation

**Start with DuckDB (Option A)** for these reasons:
- Built-in persistence (no manual Parquet file management)
- Richer SQL (ASOF JOIN, `time_bucket`, ICU timezone support)
- `frozen-duckdb` crate gives near-instant compile times
- Simpler mental model (one `.duckdb` file vs directory of Parquet files)

**Revisit DataFusion** if:
- Windows build issues prove persistent
- Binary size becomes a problem (DuckDB adds 30-50MB)
- Zero-copy Arrow integration becomes important for performance

### What NOT to Change

Regardless of choice:
- **Keep `.midas` binary files + mmap** for GPU rendering hot path
- **Keep SQLite** for transactional broker data
- **Keep SoA `CandleBuffer`** as primary in-memory format
