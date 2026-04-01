# DuckDB Integration Research

Research into integrating DuckDB as a persistent cache and analytical query layer for Hand of Midas.

**Date:** 2026-03-31
**Status:** Research complete, ready for decision

## Executive Summary

DuckDB is a strong fit for Hand of Midas as an L2 analytical cache layer. Its columnar storage, window functions, and ASOF JOINs are tailor-made for trading data workloads. The recommended architecture is a **write-behind cache** where `CandleBuffer` remains the primary in-memory format for GPU rendering, and DuckDB persists data asynchronously through a **dedicated actor thread** with a mailbox channel.

**Key decision point:** DuckDB vs Apache DataFusion. DuckDB offers built-in persistence and richer SQL; DataFusion is pure Rust with zero-copy Arrow interop. See [04-performance.md](04-performance.md) for the full comparison.

**Risk:** Windows MSVC build issues with the `bundled` feature are documented. A build spike should be the first implementation step.

## Non-Goals

- **No replacement of SQLite broker DB.** Orders, fills, positions stay in SQLite (transactional workload).
- **No replacement of `.midas` binary files** for hot-path GPU rendering. DuckDB is L2 cache only.
- **No per-frame DuckDB queries.** DuckDB is queried at load time, never in the render loop.
- **No multi-process access.** Single desktop app, single DuckDB writer process.
- **No DuckDB extensions in v1.** ICU, httpfs, etc. are deferred to avoid startup latency and build complexity.
- **No Arrow zero-copy path in v1.** Results always materialized to `CandleBuffer`. Arrow deferred until profiling shows need.

## Documents

| File | Topic | Key Takeaway |
|------|-------|--------------|
| [01-ecosystem.md](01-ecosystem.md) | DuckDB Rust crate, API, build | v1.10501.0, rusqlite-style API, `Connection` is `Send` but `!Sync` |
| [02-architecture.md](02-architecture.md) | Concurrency, mailbox pattern, iced integration | Write actor + read pool hybrid; zero new deps beyond tokio |
| [03-schema.md](03-schema.md) | Table design, queries, bulk ops | Single `candles` table with compound PK; Appender for bulk; ASOF JOIN for alignment |
| [04-performance.md](04-performance.md) | Benchmarks, comparisons, alternatives | DuckDB 10-50x faster than SQLite for analytics; DataFusion is the pure-Rust alternative |
| [05-integration.md](05-integration.md) | Crate placement, API, data flow, startup | New `midas-store` crate; `DbHandle` async API; write-behind cache pattern |
| [06-codebase-analysis.md](06-codebase-analysis.md) | Current data layer analysis | CandleData trait, CandleBuffer, no persistent OHLCV store today |

## Three-Tier Data Architecture

```
Layer     | Technology              | Purpose                          | Latency
----------|-------------------------|----------------------------------|----------
L1 (Hot)  | CandleBuffer + mmap     | GPU rendering, live candles      | ~0us
L2 (Warm) | DuckDB                  | Analytical queries, cache        | 0.4-50ms
L3 (Cold) | IB API / CSV files      | Historical data fetch on demand  | 100ms-5s
Metadata  | SQLite (existing)       | Orders, fills, config, levels    | 0.05-1ms
```

## Recommended Next Steps

1. **Build spike** (go/no-go gate, time-box: 4 hours):
   - Try `duckdb = { version = "1", features = ["bundled"] }` in a throwaway crate.
   - If `bundled` fails: try `DUCKDB_DOWNLOAD_LIB=1` env var (prebuilt binary).
   - If that fails: try `frozen-duckdb` crate.
   - If all fail: pivot to DataFusion spike (note: schema in 03-schema.md uses DuckDB-specific SQL that would need revision).
   - While spiking: benchmark open + migrate + insert 5K rows + query back to validate performance estimates.
   - **Done when:** DuckDB opens, creates schema, inserts 5K rows, and queries them back in under 1 second on Windows MSVC.
2. **Create `midas-store` crate** at `desktop/win/crates/midas-store/` (add to `[workspace] members` in `desktop/win/Cargo.toml`). First, copy `D:\GitHub\ControlPlugin\Shared\mailbox_processor\` into `desktop/win/crates/mailbox_processor/` and add it to workspace members:
   - 2a: Crate skeleton + schema + raw connection roundtrip test passing.
   - 2b: Actor thread + `DbHandle` async roundtrip test passing.
3. **Wire into `midas-app`** as `Option<DbHandle>` with graceful fallback.
   - **Done when:** App starts with `store.enabled = false` and all charts load via TestDataProvider (no regression). App starts with `store.enabled = true` and `DbHandle` is constructed successfully.
4. **Migrate TestDataProvider** to write-behind DuckDB on first load. Write-behind happens at the `MidasApp` level (after receiving buffer from TestDataProvider, fire-and-forget `insert_candles`). This keeps `midas-feed` free of DuckDB dependency.
   - **Done when:** App restart finds DuckDB pre-populated; chart loads from cache without calling TestDataProvider.
5. **Add analytical queries** (cross-symbol scans, indicator pre-computation). Depends on step 4 populating DuckDB with data. Can be developed independently with a test harness that bulk-loads sample data into in-memory DuckDB.
   - **Done when:** Cross-symbol volume scan returns correct results against bulk-loaded test data.
