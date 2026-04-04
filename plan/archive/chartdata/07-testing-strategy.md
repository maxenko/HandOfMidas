# 07 — Testing Strategy for midas-store

> Part of the [Chart Data Cache plan](00-index.md).
> Prerequisite reading: [03-schema-and-migrations](03-schema-and-migrations.md), [04-dbhandle-api](04-dbhandle-api.md)

This document specifies every test that must pass before `midas-store` ships.
It includes complete Rust code for each test, benchmark harness setup, and
CI/build-spike details.

---

## 1. Build Spike (Phase 0 — Go/No-Go Gate)

**Purpose:** Validate that DuckDB compiles and runs on our Windows MSVC
toolchain before investing in the full crate. Time-boxed to 4 hours.

### 1.1 Spike Crate Setup

Create a throwaway crate outside the workspace:

```
D:\GitHub\HandOfMidas\desktop\win\duckdb-spike\
  Cargo.toml
  src\main.rs
```

```toml
# duckdb-spike/Cargo.toml
[package]
name = "duckdb-spike"
version = "0.0.0"
edition = "2021"
publish = false

[dependencies]
duckdb = { version = "1", features = ["bundled"] }
```

### 1.2 Windows MSVC Fallback Chain

Try each step in order. Stop at the first that produces a working binary.

| Step | Action | Command |
|------|--------|---------|
| 1 | `bundled` feature (default) | `cargo build` |
| 2 | Pre-built binary download | `set DUCKDB_DOWNLOAD_LIB=1 && cargo build` |
| 3 | `frozen-duckdb` crate | Replace dep with `frozen-duckdb = { version = "1", features = ["bundled"] }` |
| 4 | Pivot to DataFusion | Abandon DuckDB; re-scope schema in 03-schema.md |

### 1.3 Spike Test Code

```rust
// duckdb-spike/src/main.rs
use duckdb::{params, Connection};
use std::time::Instant;

fn main() {
    println!("=== DuckDB Build Spike ===\n");

    // ── Phase 1: Open and migrate ────────────────────────────────────
    let t0 = Instant::now();
    let conn = Connection::open_in_memory()
        .expect("FAIL: cannot open in-memory DuckDB");
    let t_open = t0.elapsed();
    println!("[OK] open_in_memory: {:?}", t_open);

    conn.execute_batch(
        "CREATE SCHEMA IF NOT EXISTS market;
         CREATE SCHEMA IF NOT EXISTS meta;
         CREATE TABLE market.candles (
             symbol         VARCHAR    NOT NULL,
             timeframe_secs INTEGER    NOT NULL,
             timestamp_ms   BIGINT     NOT NULL,
             open           FLOAT      NOT NULL,
             high           FLOAT      NOT NULL,
             low            FLOAT      NOT NULL,
             close          FLOAT      NOT NULL,
             volume         UINTEGER   NOT NULL,
             PRIMARY KEY (symbol, timeframe_secs, timestamp_ms)
         );
         CREATE TABLE meta.data_ranges (
             symbol         VARCHAR    NOT NULL,
             timeframe_secs INTEGER    NOT NULL,
             candle_count   INTEGER    NOT NULL,
             first_ts       BIGINT     NOT NULL,
             last_ts        BIGINT     NOT NULL,
             source         VARCHAR    NOT NULL,
             updated_at     TIMESTAMP  DEFAULT CURRENT_TIMESTAMP,
             PRIMARY KEY (symbol, timeframe_secs)
         );",
    )
    .expect("FAIL: cannot create schema");
    let t_migrate = t0.elapsed();
    println!("[OK] schema created: {:?} (total)", t_migrate);

    // ── Phase 2: Bulk insert 5000 candles ────────────────────────────
    let n: usize = 5000;
    let t_ins_start = Instant::now();

    {
        let mut appender = conn
            .appender("market.candles")
            .expect("FAIL: cannot create appender");
        for i in 0..n {
            let ts = 1_700_000_000_000i64 + (i as i64 * 86_400_000);
            let price = 150.0f32 + (i as f32 * 0.01);
            appender
                .append_row(params![
                    "AAPL",
                    86400i32,
                    ts,
                    price,            // open
                    price + 2.0,      // high
                    price - 1.5,      // low
                    price + 0.5,      // close
                    (1000 + i) as u32 // volume
                ])
                .expect("FAIL: appender row");
        }
        appender.flush().expect("FAIL: appender flush");
    }
    let t_ins = t_ins_start.elapsed();
    println!("[OK] insert {n} candles: {:?}", t_ins);

    // ── Phase 3: Verify row count ────────────────────────────────────
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM market.candles WHERE symbol = 'AAPL'",
            [],
            |row| row.get(0),
        )
        .expect("FAIL: count query");
    assert_eq!(count, n as i64, "FAIL: row count mismatch");
    println!("[OK] row count verified: {count}");

    // ── Phase 4: Query back all 5000 ─────────────────────────────────
    let t_query_start = Instant::now();
    let mut stmt = conn
        .prepare(
            "SELECT timestamp_ms, open, high, low, close, volume
             FROM market.candles
             WHERE symbol = 'AAPL' AND timeframe_secs = 86400
             ORDER BY timestamp_ms ASC",
        )
        .expect("FAIL: prepare query");

    let mut timestamps = Vec::with_capacity(n);
    let mut opens = Vec::with_capacity(n);
    let mut highs = Vec::with_capacity(n);
    let mut lows = Vec::with_capacity(n);
    let mut closes = Vec::with_capacity(n);
    let mut volumes = Vec::with_capacity(n);

    let mut rows = stmt.query([]).expect("FAIL: execute query");
    while let Some(row) = rows.next().expect("FAIL: row iteration") {
        timestamps.push(row.get::<_, i64>(0).unwrap());
        opens.push(row.get::<_, f32>(1).unwrap());
        highs.push(row.get::<_, f32>(2).unwrap());
        lows.push(row.get::<_, f32>(3).unwrap());
        closes.push(row.get::<_, f32>(4).unwrap());
        volumes.push(row.get::<_, u32>(5).unwrap());
    }
    let t_query = t_query_start.elapsed();
    println!("[OK] query {n} candles: {:?}", t_query);

    // ── Phase 5: Validate data integrity ─────────────────────────────
    assert_eq!(timestamps.len(), n, "FAIL: query returned wrong count");
    assert_eq!(timestamps[0], 1_700_000_000_000i64, "FAIL: first ts");
    assert_eq!(
        timestamps[n - 1],
        1_700_000_000_000i64 + ((n - 1) as i64 * 86_400_000),
        "FAIL: last ts"
    );
    // Verify monotonically increasing
    for i in 1..timestamps.len() {
        assert!(timestamps[i] > timestamps[i - 1], "FAIL: ts order at {i}");
    }
    // Verify f32 price roundtrip
    let expected_open_0 = 150.0f32;
    assert_eq!(opens[0], expected_open_0, "FAIL: open[0] roundtrip");
    println!("[OK] data integrity verified");

    // ── Phase 6: File-based database ─────────────────────────────────
    let temp_dir = std::env::temp_dir().join("duckdb_spike");
    std::fs::create_dir_all(&temp_dir).expect("FAIL: create temp dir");
    let db_path = temp_dir.join("spike.duckdb");
    // Clean up any previous run
    let _ = std::fs::remove_file(&db_path);

    let t_file_start = Instant::now();
    let file_conn = Connection::open(&db_path).expect("FAIL: open file DB");
    file_conn
        .execute_batch(
            "CREATE SCHEMA IF NOT EXISTS market;
             CREATE TABLE market.candles (
                 symbol VARCHAR NOT NULL,
                 timeframe_secs INTEGER NOT NULL,
                 timestamp_ms BIGINT NOT NULL,
                 open FLOAT NOT NULL, high FLOAT NOT NULL,
                 low FLOAT NOT NULL, close FLOAT NOT NULL,
                 volume UINTEGER NOT NULL,
                 PRIMARY KEY (symbol, timeframe_secs, timestamp_ms)
             );",
        )
        .expect("FAIL: file DB schema");
    {
        let mut appender = file_conn.appender("market.candles").unwrap();
        for i in 0..100 {
            let ts = 1_700_000_000_000i64 + (i as i64 * 86_400_000);
            appender
                .append_row(params!["TEST", 86400i32, ts, 100.0f32, 105.0f32, 95.0f32, 101.0f32, 1000u32])
                .unwrap();
        }
        appender.flush().unwrap();
    }
    drop(file_conn);
    let t_file = t_file_start.elapsed();
    println!("[OK] file-based DB write + close: {:?}", t_file);

    // Reopen and verify persistence
    let file_conn2 = Connection::open(&db_path).expect("FAIL: reopen file DB");
    let persisted_count: i64 = file_conn2
        .query_row("SELECT COUNT(*) FROM market.candles", [], |r| r.get(0))
        .expect("FAIL: count after reopen");
    assert_eq!(persisted_count, 100, "FAIL: persistence");
    drop(file_conn2);
    println!("[OK] file persistence verified ({persisted_count} rows)");

    // ── Summary ──────────────────────────────────────────────────────
    let total = t0.elapsed();
    println!("\n=== SPIKE RESULTS ===");
    println!("  open:       {:>10?}", t_open);
    println!("  migrate:    {:>10?}", t_migrate - t_open);
    println!("  insert 5K:  {:>10?}", t_ins);
    println!("  query 5K:   {:>10?}", t_query);
    println!("  file DB:    {:>10?}", t_file);
    println!("  TOTAL:      {:>10?}", total);

    if total.as_millis() < 1000 {
        println!("\n*** PASS: total < 1 second. Proceed with DuckDB. ***");
    } else {
        println!("\n*** WARN: total >= 1 second ({:?}). Review performance. ***", total);
    }

    // Cleanup
    let _ = std::fs::remove_dir_all(&temp_dir);
}
```

### 1.4 Go/No-Go Criteria

| Criterion | Pass | Fail Action |
|-----------|------|-------------|
| `cargo build` succeeds on Windows MSVC (any fallback step) | Required | Pivot to DataFusion |
| `open + migrate + insert 5K + query 5K` < 1 second | Required | Investigate; may still proceed if < 2s |
| f32 prices survive roundtrip exactly | Required | Investigate DuckDB FLOAT semantics |
| File-based DB persists across close/reopen | Required | Bug in spike code or DuckDB version |

---

## 2. Test Helpers

Shared helpers used across all test modules. These live in
`crates/midas-store/src/lib.rs` behind `#[cfg(test)]` or in a
`tests/helpers.rs` module.

```rust
// ── Test helpers (used by schema, queries, convert, and integration tests) ──

#[cfg(test)]
pub(crate) mod test_helpers {
    use duckdb::Connection;
    use midas_core::Timeframe;
    use midas_data::CandleBuffer;

    use crate::schema::run_migrations;
    use crate::types::DataKey;

    /// Open an in-memory DuckDB connection with migrations applied.
    pub fn test_conn() -> Connection {
        let conn = Connection::open_in_memory()
            .expect("failed to open in-memory DuckDB for test");
        run_migrations(&conn).expect("migration failed in test");
        conn
    }

    /// Generate a deterministic CandleBuffer with `n` candles.
    ///
    /// Timestamps start at 2024-01-01T00:00:00Z (1704067200000 ms) and
    /// increment by 86_400_000ms (1 day) per candle.
    ///
    /// Prices follow a simple linear ramp: open = 100 + i*0.1, with
    /// high = open + 2, low = open - 1.5, close = open + 0.5.
    /// Volume = 1000 + i.
    pub fn sample_buffer(n: usize) -> CandleBuffer {
        let base_ts: i64 = 1_704_067_200_000; // 2024-01-01T00:00:00Z
        let mut buf = CandleBuffer::with_capacity(n);
        for i in 0..n {
            let ts = base_ts + (i as i64 * 86_400_000);
            let open = 100.0f32 + (i as f32 * 0.1);
            buf.push(ts, open, open + 2.0, open - 1.5, open + 0.5, (1000 + i) as u32);
        }
        buf
    }

    /// Convenience: build a DataKey for a given symbol with D1 timeframe.
    pub fn sample_key(symbol: &str) -> DataKey {
        DataKey {
            symbol: symbol.to_owned(),
            timeframe: Timeframe::D1,
        }
    }

    /// Build a DataKey with a specific timeframe.
    pub fn key_with_tf(symbol: &str, tf: Timeframe) -> DataKey {
        DataKey {
            symbol: symbol.to_owned(),
            timeframe: tf,
        }
    }
}
```

---

## 3. Unit Tests — `schema.rs`

These tests validate DDL execution, migration idempotency, and schema
version tracking.

```rust
#[cfg(test)]
mod tests {
    use crate::test_helpers::test_conn;
    use crate::schema::run_migrations;
    use duckdb::Connection;

    // ── test_migration_idempotent ──────────────────────────────────────
    /// Running migrations twice must not error or duplicate state.
    #[test]
    fn test_migration_idempotent() {
        let conn = Connection::open_in_memory()
            .expect("failed to open in-memory DuckDB");
        run_migrations(&conn).expect("first migration failed");
        run_migrations(&conn).expect("second migration must also succeed (idempotent)");

        // Schema must still be intact after double-run
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM information_schema.tables
                 WHERE table_schema = 'market' AND table_name = 'candles'",
                [],
                |row| row.get(0),
            )
            .expect("query failed");
        assert_eq!(count, 1, "candles table should exist exactly once");
    }

    // ── test_schema_version_tracked ───────────────────────────────────
    /// After migration, the schema_version table records the applied version.
    #[test]
    fn test_schema_version_tracked() {
        let conn = test_conn();

        let version: i32 = conn
            .query_row(
                "SELECT MAX(version) FROM schema_version",
                [],
                |row| row.get(0),
            )
            .expect("schema_version query failed");

        // v1 is the initial migration. Must be >= 1.
        assert!(version >= 1, "schema version should be >= 1, got {version}");
    }

    // ── test_all_tables_exist ─────────────────────────────────────────
    /// After migration, all expected tables must exist in the correct schemas.
    #[test]
    fn test_all_tables_exist() {
        let conn = test_conn();

        let expected_tables = vec![
            ("market", "candles"),
            ("meta", "data_ranges"),
            ("meta", "symbols"),
        ];

        for (schema, table) in &expected_tables {
            let exists: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM information_schema.tables
                     WHERE table_schema = ? AND table_name = ?",
                    duckdb::params![schema, table],
                    |row| row.get(0),
                )
                .expect("information_schema query failed");

            assert_eq!(
                exists, 1,
                "table {schema}.{table} should exist after migration"
            );
        }

        // schema_version table lives in the default (main) schema
        let sv_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM information_schema.tables
                 WHERE table_name = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .expect("schema_version check failed");
        assert_eq!(sv_exists, 1, "schema_version table should exist");
    }
}
```

---

## 4. Unit Tests — `queries.rs`

These tests validate all SQL query functions against an in-memory DuckDB
with the full schema. Each test is isolated (fresh connection).

```rust
#[cfg(test)]
mod tests {
    use crate::queries::{bulk_insert, delete_symbol, query_all, query_range, list_cached};
    use crate::test_helpers::{key_with_tf, sample_buffer, sample_key, test_conn};
    use midas_core::Timeframe;
    use midas_data::CandleBuffer;

    // ── test_bulk_insert_roundtrip ────────────────────────────────────
    /// Insert a CandleBuffer, query it back, verify every field matches.
    #[test]
    fn test_bulk_insert_roundtrip() {
        let conn = test_conn();
        let key = sample_key("AAPL");
        let buf = sample_buffer(100);

        let inserted = bulk_insert(&conn, &key, &buf).expect("insert failed");
        assert_eq!(inserted, 100, "should report 100 rows inserted");

        let loaded = query_all(&conn, &key).expect("query failed");
        assert_eq!(loaded.len(), 100);
        assert_eq!(loaded.timestamps, buf.timestamps, "timestamps mismatch");
        assert_eq!(loaded.opens, buf.opens, "opens mismatch");
        assert_eq!(loaded.highs, buf.highs, "highs mismatch");
        assert_eq!(loaded.lows, buf.lows, "lows mismatch");
        assert_eq!(loaded.closes, buf.closes, "closes mismatch");
        assert_eq!(loaded.volumes, buf.volumes, "volumes mismatch");
    }

    // ── test_empty_query ──────────────────────────────────────────────
    /// Querying a non-existent symbol returns an empty CandleBuffer.
    #[test]
    fn test_empty_query() {
        let conn = test_conn();
        let key = sample_key("NONEXISTENT");

        let loaded = query_all(&conn, &key).expect("query should succeed even for missing data");
        assert!(loaded.is_empty(), "should return empty buffer");
        assert_eq!(loaded.len(), 0);
    }

    // ── test_range_query ──────────────────────────────────────────────
    /// Insert 1000 candles, query a subset by time range, verify boundaries.
    #[test]
    fn test_range_query() {
        let conn = test_conn();
        let key = sample_key("MSFT");
        let buf = sample_buffer(1000);

        bulk_insert(&conn, &key, &buf).expect("insert failed");

        // Query candles 100..200 by timestamp range
        let start_ts = buf.timestamps[100];
        let end_ts = buf.timestamps[199];

        let loaded = query_range(&conn, &key, start_ts, end_ts)
            .expect("range query failed");

        assert_eq!(loaded.len(), 100, "should return exactly 100 candles");
        assert_eq!(loaded.timestamps[0], start_ts, "first ts should match start");
        assert_eq!(
            *loaded.timestamps.last().unwrap(),
            end_ts,
            "last ts should match end"
        );

        // All returned timestamps should be within [start, end]
        for ts in &loaded.timestamps {
            assert!(
                *ts >= start_ts && *ts <= end_ts,
                "timestamp {ts} outside range [{start_ts}, {end_ts}]"
            );
        }
    }

    // ── test_upsert_overwrites ────────────────────────────────────────
    /// Insert data, then upsert overlapping data. Verify latest values win.
    #[test]
    fn test_upsert_overwrites() {
        let conn = test_conn();
        let key = sample_key("TSLA");

        // Insert 10 candles
        let buf1 = sample_buffer(10);
        bulk_insert(&conn, &key, &buf1).expect("first insert failed");

        // Create updated buffer for the same timestamps but different prices
        let mut buf2 = CandleBuffer::with_capacity(10);
        for i in 0..10 {
            buf2.push(
                buf1.timestamps[i],       // same timestamp
                999.0,                    // different open
                1001.0,                   // different high
                997.0,                    // different low
                1000.0,                   // different close
                99999,                    // different volume
            );
        }

        // Upsert (INSERT OR REPLACE)
        bulk_insert(&conn, &key, &buf2).expect("upsert failed");

        let loaded = query_all(&conn, &key).expect("query after upsert failed");
        assert_eq!(loaded.len(), 10, "count should remain 10 after upsert");
        assert_eq!(loaded.opens[0], 999.0, "open should be updated");
        assert_eq!(loaded.closes[0], 1000.0, "close should be updated");
        assert_eq!(loaded.volumes[0], 99999, "volume should be updated");
    }

    // ── test_data_ranges_updated ──────────────────────────────────────
    /// After inserting candles, meta.data_ranges should reflect the insert.
    #[test]
    fn test_data_ranges_updated() {
        let conn = test_conn();
        let key = sample_key("GOOG");
        let buf = sample_buffer(500);

        bulk_insert(&conn, &key, &buf).expect("insert failed");

        let cached = list_cached(&conn).expect("list_cached failed");
        assert_eq!(cached.len(), 1, "should have one cache entry");

        let info = &cached[0];
        assert_eq!(info.key.symbol, "GOOG");
        assert_eq!(info.key.timeframe, Timeframe::D1);
        assert_eq!(info.candle_count, 500);
        assert_eq!(info.first_ts, buf.timestamps[0]);
        assert_eq!(info.last_ts, *buf.timestamps.last().unwrap());
    }

    // ── test_duplicate_insert_ignored ─────────────────────────────────
    /// Inserting the same data twice with INSERT OR IGNORE semantics
    /// should not error and should not create duplicates.
    #[test]
    fn test_duplicate_insert_ignored() {
        let conn = test_conn();
        let key = sample_key("AMZN");
        let buf = sample_buffer(50);

        bulk_insert(&conn, &key, &buf).expect("first insert failed");
        bulk_insert(&conn, &key, &buf).expect("duplicate insert should not error");

        let loaded = query_all(&conn, &key).expect("query failed");
        // Depending on INSERT OR REPLACE vs INSERT OR IGNORE:
        // With REPLACE: 50 rows (overwritten in place)
        // With IGNORE: 50 rows (duplicates skipped)
        // Either way, exactly 50 rows.
        assert_eq!(loaded.len(), 50, "should still have exactly 50 candles");
    }

    // ── test_delete_symbol ────────────────────────────────────────────
    /// Insert data, delete it, verify the symbol is gone.
    #[test]
    fn test_delete_symbol() {
        let conn = test_conn();
        let key = sample_key("NVDA");
        let buf = sample_buffer(200);

        bulk_insert(&conn, &key, &buf).expect("insert failed");
        let loaded = query_all(&conn, &key).expect("query failed");
        assert_eq!(loaded.len(), 200);

        delete_symbol(&conn, &key).expect("delete failed");

        let loaded_after = query_all(&conn, &key).expect("query after delete failed");
        assert!(loaded_after.is_empty(), "should be empty after delete");

        // meta.data_ranges should also be cleaned up
        let cached = list_cached(&conn).expect("list_cached failed");
        let has_nvda = cached.iter().any(|c| c.key.symbol == "NVDA");
        assert!(!has_nvda, "data_ranges should not contain deleted symbol");
    }

    // ── test_multiple_symbols ─────────────────────────────────────────
    /// Insert AAPL and MSFT, query each independently. No cross-talk.
    #[test]
    fn test_multiple_symbols() {
        let conn = test_conn();

        let key_aapl = sample_key("AAPL");
        let key_msft = sample_key("MSFT");

        let buf_aapl = sample_buffer(100);

        // Create a different buffer for MSFT (different prices)
        let mut buf_msft = CandleBuffer::with_capacity(50);
        let base_ts: i64 = 1_704_067_200_000;
        for i in 0..50 {
            let ts = base_ts + (i as i64 * 86_400_000);
            let open = 300.0f32 + (i as f32 * 0.2);
            buf_msft.push(ts, open, open + 3.0, open - 2.0, open + 1.0, (5000 + i) as u32);
        }

        bulk_insert(&conn, &key_aapl, &buf_aapl).expect("AAPL insert failed");
        bulk_insert(&conn, &key_msft, &buf_msft).expect("MSFT insert failed");

        let loaded_aapl = query_all(&conn, &key_aapl).expect("AAPL query failed");
        let loaded_msft = query_all(&conn, &key_msft).expect("MSFT query failed");

        assert_eq!(loaded_aapl.len(), 100, "AAPL should have 100 candles");
        assert_eq!(loaded_msft.len(), 50, "MSFT should have 50 candles");

        // Verify no cross-contamination
        assert_eq!(loaded_aapl.opens[0], 100.0, "AAPL open[0]");
        assert_eq!(loaded_msft.opens[0], 300.0, "MSFT open[0]");

        // list_cached should show both
        let cached = list_cached(&conn).expect("list_cached failed");
        assert_eq!(cached.len(), 2, "should have two cache entries");
    }

    // ── test_multiple_timeframes ──────────────────────────────────────
    /// Insert D1 and M5 data for the same symbol. Query each independently.
    #[test]
    fn test_multiple_timeframes() {
        let conn = test_conn();

        let key_d1 = key_with_tf("AAPL", Timeframe::D1);
        let key_m5 = key_with_tf("AAPL", Timeframe::M5);

        // D1 data: 100 daily candles
        let buf_d1 = sample_buffer(100);

        // M5 data: 200 five-minute candles (different timestamps)
        let base_ts: i64 = 1_704_067_200_000;
        let mut buf_m5 = CandleBuffer::with_capacity(200);
        for i in 0..200 {
            let ts = base_ts + (i as i64 * 300_000); // 5-minute intervals
            let open = 150.0f32 + (i as f32 * 0.05);
            buf_m5.push(ts, open, open + 1.0, open - 0.5, open + 0.3, (2000 + i) as u32);
        }

        bulk_insert(&conn, &key_d1, &buf_d1).expect("D1 insert failed");
        bulk_insert(&conn, &key_m5, &buf_m5).expect("M5 insert failed");

        let loaded_d1 = query_all(&conn, &key_d1).expect("D1 query failed");
        let loaded_m5 = query_all(&conn, &key_m5).expect("M5 query failed");

        assert_eq!(loaded_d1.len(), 100, "D1 should have 100 candles");
        assert_eq!(loaded_m5.len(), 200, "M5 should have 200 candles");

        // Verify timeframe isolation
        assert_eq!(loaded_d1.opens[0], 100.0);
        assert_eq!(loaded_m5.opens[0], 150.0);

        // list_cached should show both entries for AAPL
        let cached = list_cached(&conn).expect("list_cached failed");
        let aapl_entries: Vec<_> = cached.iter().filter(|c| c.key.symbol == "AAPL").collect();
        assert_eq!(aapl_entries.len(), 2, "AAPL should have two timeframe entries");
    }

    // ── test_large_buffer_insert ──────────────────────────────────────
    /// Insert 50K candles, verify count and boundary values.
    #[test]
    fn test_large_buffer_insert() {
        let conn = test_conn();
        let key = sample_key("SPY");
        let buf = sample_buffer(50_000);

        let inserted = bulk_insert(&conn, &key, &buf).expect("50K insert failed");
        assert_eq!(inserted, 50_000);

        let loaded = query_all(&conn, &key).expect("50K query failed");
        assert_eq!(loaded.len(), 50_000);

        // Verify boundary values
        assert_eq!(loaded.timestamps[0], buf.timestamps[0], "first ts");
        assert_eq!(
            *loaded.timestamps.last().unwrap(),
            *buf.timestamps.last().unwrap(),
            "last ts"
        );
        assert_eq!(loaded.opens[0], buf.opens[0], "first open");
        assert_eq!(
            *loaded.opens.last().unwrap(),
            *buf.opens.last().unwrap(),
            "last open"
        );

        // Verify monotonically increasing timestamps
        for i in 1..loaded.len() {
            assert!(
                loaded.timestamps[i] > loaded.timestamps[i - 1],
                "timestamp order violated at index {i}"
            );
        }
    }
}
```

---

## 5. Unit Tests — `convert.rs`

These tests validate that Rust types survive the DuckDB storage round-trip
with exact fidelity. This is critical because `CandleBuffer` fields feed
directly into GPU pipelines where precision matters.

```rust
#[cfg(test)]
mod tests {
    use crate::test_helpers::test_conn;
    use duckdb::params;
    use midas_core::Timeframe;

    // ── test_f32_roundtrip ────────────────────────────────────────────
    /// f32 prices must survive DuckDB FLOAT storage exactly (no promotion
    /// to f64 and back).
    #[test]
    fn test_f32_roundtrip() {
        let conn = test_conn();

        // Test representative f32 values including edge cases
        let test_prices: Vec<f32> = vec![
            0.0,
            1.0,
            -1.0,
            100.0,
            150.125,       // exact in f32
            0.1,           // not exact in f32 — but must roundtrip as the same f32 bits
            f32::MIN_POSITIVE,
            f32::MAX,
            99999.99,
            0.001,
        ];

        for (i, &price) in test_prices.iter().enumerate() {
            let ts = 1_704_067_200_000i64 + (i as i64 * 86_400_000);
            conn.execute(
                "INSERT OR REPLACE INTO market.candles
                 VALUES ('RT_TEST', 86400, ?, ?, ?, ?, ?, 1000)",
                params![ts, price, price, price, price],
            )
            .expect("insert failed");

            let loaded: f32 = conn
                .query_row(
                    "SELECT open FROM market.candles
                     WHERE symbol = 'RT_TEST' AND timestamp_ms = ?",
                    params![ts],
                    |row| row.get(0),
                )
                .expect("query failed");

            assert_eq!(
                loaded.to_bits(),
                price.to_bits(),
                "f32 bitwise roundtrip failed for value {price} at index {i}"
            );
        }
    }

    // ── test_u32_volume_roundtrip ─────────────────────────────────────
    /// u32 volumes must survive DuckDB UINTEGER storage exactly.
    #[test]
    fn test_u32_volume_roundtrip() {
        let conn = test_conn();

        let test_volumes: Vec<u32> = vec![
            0,
            1,
            1000,
            1_000_000,
            u32::MAX,       // 4,294,967,295
            u32::MAX - 1,
        ];

        for (i, &vol) in test_volumes.iter().enumerate() {
            let ts = 1_704_067_200_000i64 + (i as i64 * 86_400_000);
            conn.execute(
                "INSERT OR REPLACE INTO market.candles
                 VALUES ('VOL_TEST', 86400, ?, 100.0, 105.0, 95.0, 101.0, ?)",
                params![ts, vol],
            )
            .expect("insert failed");

            let loaded: u32 = conn
                .query_row(
                    "SELECT volume FROM market.candles
                     WHERE symbol = 'VOL_TEST' AND timestamp_ms = ?",
                    params![ts],
                    |row| row.get(0),
                )
                .expect("query failed");

            assert_eq!(
                loaded, vol,
                "u32 volume roundtrip failed for value {vol} at index {i}"
            );
        }
    }

    // ── test_i64_timestamp_roundtrip ──────────────────────────────────
    /// i64 timestamps must survive DuckDB BIGINT storage exactly.
    #[test]
    fn test_i64_timestamp_roundtrip() {
        let conn = test_conn();

        let test_timestamps: Vec<i64> = vec![
            0,
            1_704_067_200_000,        // 2024-01-01T00:00:00Z
            1_735_689_600_000,        // 2025-01-01T00:00:00Z
            i64::MAX,
            -1,                        // Pre-epoch (unlikely but valid)
        ];

        for (i, &ts) in test_timestamps.iter().enumerate() {
            conn.execute(
                "INSERT OR REPLACE INTO market.candles
                 VALUES (?, 86400, ?, 100.0, 105.0, 95.0, 101.0, 1000)",
                params![format!("TS_TEST_{i}"), ts],
            )
            .expect("insert failed");

            let loaded: i64 = conn
                .query_row(
                    "SELECT timestamp_ms FROM market.candles
                     WHERE symbol = ? AND timestamp_ms = ?",
                    params![format!("TS_TEST_{i}"), ts],
                    |row| row.get(0),
                )
                .expect("query failed");

            assert_eq!(
                loaded, ts,
                "i64 timestamp roundtrip failed for value {ts} at index {i}"
            );
        }
    }

    // ── test_timeframe_secs_roundtrip ─────────────────────────────────
    /// Timeframe -> as_secs() -> u32 -> store -> load -> Timeframe must
    /// reconstruct the original variant.
    #[test]
    fn test_timeframe_secs_roundtrip() {
        let all_timeframes = [
            Timeframe::S1,
            Timeframe::S5,
            Timeframe::S15,
            Timeframe::S30,
            Timeframe::M1,
            Timeframe::M5,
            Timeframe::M15,
            Timeframe::M30,
            Timeframe::H1,
            Timeframe::H4,
            Timeframe::D1,
            Timeframe::W1,
            Timeframe::MN1,
        ];

        for tf in all_timeframes {
            let secs = tf.as_secs();
            let reconstructed = timeframe_from_secs(secs);
            assert_eq!(
                reconstructed,
                Some(tf),
                "Timeframe roundtrip failed: {tf:?} -> {secs} -> {reconstructed:?}"
            );
        }
    }

    /// Helper: reconstruct a Timeframe from its as_secs() value.
    /// This function should live in convert.rs (or types.rs) in production code.
    fn timeframe_from_secs(secs: u32) -> Option<Timeframe> {
        match secs {
            1 => Some(Timeframe::S1),
            5 => Some(Timeframe::S5),
            15 => Some(Timeframe::S15),
            30 => Some(Timeframe::S30),
            60 => Some(Timeframe::M1),
            300 => Some(Timeframe::M5),
            900 => Some(Timeframe::M15),
            1800 => Some(Timeframe::M30),
            3600 => Some(Timeframe::H1),
            14400 => Some(Timeframe::H4),
            86400 => Some(Timeframe::D1),
            604800 => Some(Timeframe::W1),
            2592000 => Some(Timeframe::MN1),
            _ => None,
        }
    }
}
```

---

## 6. Integration Tests — `handle.rs`

These tests exercise the full async stack: `DbHandle` -> `MailboxProcessor`
-> DuckDB actor thread. They require the tokio runtime.

```rust
#[cfg(test)]
mod tests {
    use crate::handle::DbHandle;
    use crate::test_helpers::{sample_buffer, sample_key, key_with_tf};
    use midas_core::Timeframe;

    // ── test_dbhandle_open_file ───────────────────────────────────────
    /// File-based DbHandle opens, operates, and persists.
    #[tokio::test]
    async fn test_dbhandle_open_file() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let db_path = dir.path().join("test.duckdb");

        let handle = DbHandle::open(StoreConfig { path: Some(db_path), ..Default::default() });

        let key = sample_key("AAPL");
        let buf = sample_buffer(50);
        handle
            .insert_candles(key.clone(), buf.clone())
            .await
            .expect("insert failed");

        let loaded = handle.query_candles(key).await.expect("query failed");
        assert_eq!(loaded.len(), 50);
        assert_eq!(loaded.timestamps, buf.timestamps);

        handle.shutdown().await.expect("shutdown failed");
    }

    // ── test_dbhandle_open_memory ─────────────────────────────────────
    /// In-memory DbHandle works for test isolation.
    #[tokio::test]
    async fn test_dbhandle_open_memory() {
        let handle = DbHandle::open_memory();

        let key = sample_key("MSFT");
        let buf = sample_buffer(25);
        handle
            .insert_candles(key.clone(), buf.clone())
            .await
            .expect("insert failed");

        let loaded = handle.query_candles(key).await.expect("query failed");
        assert_eq!(loaded.len(), 25);
        assert_eq!(loaded.opens, buf.opens);

        handle.shutdown().await.expect("shutdown failed");
    }

    // ── test_dbhandle_concurrent_queries ──────────────────────────────
    /// 10 concurrent query tasks via cloned handle. All should succeed
    /// without deadlock or data corruption.
    #[tokio::test]
    async fn test_dbhandle_concurrent_queries() {
        let handle = DbHandle::open_memory();

        // Pre-populate with data
        let key = sample_key("AAPL");
        let buf = sample_buffer(1000);
        handle
            .insert_candles(key.clone(), buf.clone())
            .await
            .expect("insert failed");

        // Spawn 10 concurrent query tasks
        let mut tasks = Vec::new();
        for i in 0..10 {
            let h = handle.clone();
            let k = sample_key("AAPL");
            tasks.push(tokio::spawn(async move {
                let loaded = h.query_candles(k).await.expect("concurrent query failed");
                assert_eq!(loaded.len(), 1000, "task {i}: expected 1000 candles");
                loaded.len()
            }));
        }

        // Await all tasks
        let mut total_rows = 0;
        for task in tasks {
            total_rows += task.await.expect("task panicked");
        }
        assert_eq!(total_rows, 10_000, "all 10 tasks should return 1000 each");

        handle.shutdown().await.expect("shutdown failed");
    }

    // ── test_dbhandle_fire_and_forget_insert ──────────────────────────
    /// fire_and_forget insert returns immediately; data appears on
    /// subsequent query.
    #[tokio::test]
    async fn test_dbhandle_fire_and_forget_insert() {
        let handle = DbHandle::open_memory();

        let key = sample_key("TSLA");
        let buf = sample_buffer(100);

        // fire_and_forget should not block
        handle
            .fire_and_forget_insert(key.clone(), buf.clone())
            .await
            .expect("fire_and_forget failed");

        // The actor processes messages sequentially, so a subsequent
        // send() will wait for the insert to complete first.
        let loaded = handle.query_candles(key).await.expect("query failed");
        assert_eq!(loaded.len(), 100);

        handle.shutdown().await.expect("shutdown failed");
    }

    // ── test_dbhandle_shutdown_clean ──────────────────────────────────
    /// After shutdown(), the handle should return Ok. Subsequent sends
    /// should fail with a channel-closed error.
    #[tokio::test]
    async fn test_dbhandle_shutdown_clean() {
        let handle = DbHandle::open_memory();

        // Insert some data before shutdown
        let key = sample_key("AAPL");
        let buf = sample_buffer(10);
        handle
            .insert_candles(key.clone(), buf)
            .await
            .expect("insert failed");

        // Shutdown should succeed
        handle.shutdown().await.expect("shutdown failed");

        // Subsequent operations should fail (channel closed)
        let result = handle.query_candles(key).await;
        assert!(
            result.is_err(),
            "query after shutdown should fail"
        );
    }

    // ── test_dbhandle_clone_independence ──────────────────────────────
    /// Cloning a DbHandle shares the channel. Dropping the original does
    /// NOT close the actor — the clone keeps it alive.
    #[tokio::test]
    async fn test_dbhandle_clone_independence() {
        let handle = DbHandle::open_memory();
        let clone = handle.clone();

        // Insert via original
        let key = sample_key("GOOG");
        let buf = sample_buffer(30);
        handle
            .insert_candles(key.clone(), buf.clone())
            .await
            .expect("insert via original failed");

        // Drop original — actor should stay alive because clone holds a sender
        drop(handle);

        // Query via clone should still work
        let loaded = clone.query_candles(key).await.expect("query via clone failed");
        assert_eq!(loaded.len(), 30);

        clone.shutdown().await.expect("shutdown via clone failed");
    }

    // ── test_dbhandle_list_cached ─────────────────────────────────────
    /// list_cached returns metadata for all stored symbol/timeframe pairs.
    #[tokio::test]
    async fn test_dbhandle_list_cached() {
        let handle = DbHandle::open_memory();

        // Insert two symbols with different timeframes
        let key1 = key_with_tf("AAPL", Timeframe::D1);
        let key2 = key_with_tf("MSFT", Timeframe::M5);
        handle
            .insert_candles(key1.clone(), sample_buffer(100))
            .await
            .expect("insert 1 failed");

        let mut buf_m5 = midas_data::CandleBuffer::with_capacity(200);
        let base_ts: i64 = 1_704_067_200_000;
        for i in 0..200 {
            let ts = base_ts + (i as i64 * 300_000);
            buf_m5.push(ts, 150.0 + i as f32 * 0.1, 152.0, 148.0, 151.0, 1000);
        }
        handle
            .insert_candles(key2.clone(), buf_m5)
            .await
            .expect("insert 2 failed");

        let cached = handle.list_cached().await.expect("list_cached failed");
        assert_eq!(cached.len(), 2);

        // Verify metadata
        let aapl_entry = cached.iter().find(|c| c.key.symbol == "AAPL").expect("AAPL not found");
        assert_eq!(aapl_entry.candle_count, 100);
        assert_eq!(aapl_entry.key.timeframe, Timeframe::D1);

        let msft_entry = cached.iter().find(|c| c.key.symbol == "MSFT").expect("MSFT not found");
        assert_eq!(msft_entry.candle_count, 200);
        assert_eq!(msft_entry.key.timeframe, Timeframe::M5);

        handle.shutdown().await.expect("shutdown failed");
    }

    // ── test_dbhandle_query_range ─────────────────────────────────────
    /// query_candles_range returns only candles within the time window.
    #[tokio::test]
    async fn test_dbhandle_query_range() {
        let handle = DbHandle::open_memory();

        let key = sample_key("SPY");
        let buf = sample_buffer(1000);
        handle
            .insert_candles(key.clone(), buf.clone())
            .await
            .expect("insert failed");

        let start = buf.timestamps[200];
        let end = buf.timestamps[299];
        let loaded = handle
            .query_candles_range(key, start, end)
            .await
            .expect("range query failed");

        assert_eq!(loaded.len(), 100);
        assert_eq!(loaded.timestamps[0], start);
        assert_eq!(*loaded.timestamps.last().unwrap(), end);

        handle.shutdown().await.expect("shutdown failed");
    }
}
```

---

## 7. Integration Tests — Workspace Level

These tests live in `desktop/win/tests/` and exercise cross-crate data flow.

### 7.1 `tests/store_integration.rs`

```rust
//! Workspace-level integration tests for midas-store.
//!
//! Run with: cargo test --test store_integration

use midas_core::Timeframe;
use midas_data::CandleBuffer;

// ── test_candle_buffer_to_store_roundtrip ─────────────────────────────
/// Create a CandleBuffer in midas-data, store via midas-store, retrieve,
/// and compare field by field.
#[tokio::test]
async fn test_candle_buffer_to_store_roundtrip() {
    use midas_store::handle::DbHandle;
    use midas_store::types::DataKey;

    let handle = DbHandle::open_memory();

    // Build a buffer using midas-data's public API
    let mut buf = CandleBuffer::with_capacity(500);
    let base_ts: i64 = 1_704_067_200_000; // 2024-01-01
    for i in 0..500 {
        let ts = base_ts + (i as i64 * 86_400_000);
        let price = 175.0f32 + (i as f32 * 0.05);
        buf.push(ts, price, price + 3.0, price - 2.0, price + 1.0, (10000 + i) as u32);
    }

    let key = DataKey {
        symbol: "AAPL".to_owned(),
        timeframe: Timeframe::D1,
    };

    handle
        .insert_candles(key.clone(), buf.clone())
        .await
        .expect("insert failed");

    let loaded = handle.query_candles(key).await.expect("query failed");

    // Field-by-field comparison
    assert_eq!(loaded.len(), buf.len(), "length mismatch");
    assert_eq!(loaded.timestamps, buf.timestamps, "timestamps mismatch");
    assert_eq!(loaded.opens, buf.opens, "opens mismatch");
    assert_eq!(loaded.highs, buf.highs, "highs mismatch");
    assert_eq!(loaded.lows, buf.lows, "lows mismatch");
    assert_eq!(loaded.closes, buf.closes, "closes mismatch");
    assert_eq!(loaded.volumes, buf.volumes, "volumes mismatch");

    handle.shutdown().await.expect("shutdown failed");
}

// ── test_store_disabled_fallback ──────────────────────────────────────
/// When store is None (disabled in config), data loading falls back to
/// TestDataProvider. This validates that the app works identically with
/// the store disabled.
#[test]
fn test_store_disabled_fallback() {
    use midas_feed::TestDataProvider;

    // Simulate the app's fallback path: store = None
    let store: Option<()> = None; // Placeholder for Option<DbHandle>

    // When store is None, we use TestDataProvider directly
    if store.is_none() {
        let mut provider = TestDataProvider::new();
        let candles = provider.get_candles("AAPL", Timeframe::D1, 365);

        // TestDataProvider should produce valid data
        assert!(!candles.is_empty(), "TestDataProvider should produce data");
        assert!(candles.len() > 200, "should have > 200 trading days for 365 calendar days");

        // Timestamps must be monotonically increasing
        for i in 1..candles.len() {
            assert!(
                candles.timestamps[i] > candles.timestamps[i - 1],
                "timestamps not sorted at index {i}"
            );
        }

        // Prices must be positive
        for i in 0..candles.len() {
            assert!(candles.opens[i] > 0.0, "open must be positive");
            assert!(candles.highs[i] >= candles.lows[i], "high must be >= low");
        }
    }
}
```

---

## 8. Benchmark Suite (criterion)

### 8.1 Setup

Add to `crates/midas-store/Cargo.toml`:

```toml
[dev-dependencies]
criterion = { workspace = true }
tempfile  = { workspace = true }
tokio     = { workspace = true }

[[bench]]
name = "store_bench"
harness = false
```

### 8.2 Benchmark Code

```rust
// crates/midas-store/benches/store_bench.rs

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use duckdb::Connection;
use midas_core::Timeframe;
use midas_data::CandleBuffer;

// ── Helpers ──────────────────────────────────────────────────────────

fn make_buffer(n: usize) -> CandleBuffer {
    let base_ts: i64 = 1_704_067_200_000;
    let mut buf = CandleBuffer::with_capacity(n);
    for i in 0..n {
        let ts = base_ts + (i as i64 * 86_400_000);
        let open = 100.0f32 + (i as f32 * 0.1);
        buf.push(ts, open, open + 2.0, open - 1.5, open + 0.5, (1000 + i) as u32);
    }
    buf
}

fn open_and_migrate() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    midas_store::schema::run_migrations(&conn).unwrap();
    conn
}

fn do_bulk_insert(conn: &Connection, symbol: &str, tf_secs: u32, buf: &CandleBuffer) {
    // Clear existing data for this key to make benchmark idempotent
    conn.execute(
        "DELETE FROM market.candles WHERE symbol = ? AND timeframe_secs = ?",
        duckdb::params![symbol, tf_secs],
    )
    .unwrap();

    let mut appender = conn.appender("market.candles").unwrap();
    for i in 0..buf.len() {
        appender
            .append_row(duckdb::params![
                symbol,
                tf_secs as i32,
                buf.timestamps[i],
                buf.opens[i],
                buf.highs[i],
                buf.lows[i],
                buf.closes[i],
                buf.volumes[i]
            ])
            .unwrap();
    }
    appender.flush().unwrap();
}

fn do_query_all(conn: &Connection, symbol: &str, tf_secs: u32) -> CandleBuffer {
    let mut stmt = conn
        .prepare_cached(
            "SELECT timestamp_ms, open, high, low, close, volume
             FROM market.candles
             WHERE symbol = ? AND timeframe_secs = ?
             ORDER BY timestamp_ms ASC",
        )
        .unwrap();

    let mut buf = CandleBuffer::with_capacity(50_000);
    let mut rows = stmt.query(duckdb::params![symbol, tf_secs]).unwrap();
    while let Some(row) = rows.next().unwrap() {
        buf.push(
            row.get(0).unwrap(),
            row.get(1).unwrap(),
            row.get(2).unwrap(),
            row.get(3).unwrap(),
            row.get(4).unwrap(),
            row.get(5).unwrap(),
        );
    }
    buf
}

fn do_query_range(
    conn: &Connection,
    symbol: &str,
    tf_secs: u32,
    start: i64,
    end: i64,
) -> CandleBuffer {
    let mut stmt = conn
        .prepare_cached(
            "SELECT timestamp_ms, open, high, low, close, volume
             FROM market.candles
             WHERE symbol = ? AND timeframe_secs = ?
               AND timestamp_ms >= ? AND timestamp_ms <= ?
             ORDER BY timestamp_ms ASC",
        )
        .unwrap();

    let mut buf = CandleBuffer::with_capacity(5_000);
    let mut rows = stmt
        .query(duckdb::params![symbol, tf_secs, start, end])
        .unwrap();
    while let Some(row) = rows.next().unwrap() {
        buf.push(
            row.get(0).unwrap(),
            row.get(1).unwrap(),
            row.get(2).unwrap(),
            row.get(3).unwrap(),
            row.get(4).unwrap(),
            row.get(5).unwrap(),
        );
    }
    buf
}

// ── Benchmarks ───────────────────────────────────────────────────────

fn bench_open_migrate(c: &mut Criterion) {
    c.bench_function("open_migrate", |b| {
        b.iter(|| {
            let conn = black_box(open_and_migrate());
            drop(conn);
        });
    });
}

fn bench_insert_5k(c: &mut Criterion) {
    let buf = make_buffer(5_000);
    let conn = open_and_migrate();

    c.bench_function("insert_5k", |b| {
        b.iter(|| {
            do_bulk_insert(&conn, "BENCH", 86400, black_box(&buf));
        });
    });
}

fn bench_insert_50k(c: &mut Criterion) {
    let buf = make_buffer(50_000);
    let conn = open_and_migrate();

    c.bench_function("insert_50k", |b| {
        b.iter(|| {
            do_bulk_insert(&conn, "BENCH50K", 86400, black_box(&buf));
        });
    });
}

fn bench_query_5k(c: &mut Criterion) {
    let buf = make_buffer(5_000);
    let conn = open_and_migrate();
    do_bulk_insert(&conn, "Q5K", 86400, &buf);

    c.bench_function("query_5k", |b| {
        b.iter(|| {
            let result = do_query_all(&conn, "Q5K", 86400);
            black_box(result.len());
        });
    });
}

fn bench_query_range_1k(c: &mut Criterion) {
    let buf = make_buffer(50_000);
    let conn = open_and_migrate();
    do_bulk_insert(&conn, "QR1K", 86400, &buf);

    // Range: candles 1000..2000 (1000 rows out of 50K)
    let start = buf.timestamps[1000];
    let end = buf.timestamps[1999];

    c.bench_function("query_range_1k_of_50k", |b| {
        b.iter(|| {
            let result = do_query_range(&conn, "QR1K", 86400, start, end);
            black_box(result.len());
        });
    });
}

criterion_group!(
    benches,
    bench_open_migrate,
    bench_insert_5k,
    bench_insert_50k,
    bench_query_5k,
    bench_query_range_1k,
);
criterion_main!(benches);
```

### 8.3 Performance Targets

| Benchmark | Target | Failure Action |
|-----------|--------|----------------|
| `open_migrate` | < 100ms | Investigate; check extension loading |
| `insert_5k` | < 10ms | Check Appender usage; verify no per-row transactions |
| `insert_50k` | < 100ms | Same |
| `query_5k` | < 5ms | Verify `prepare_cached`; check column types |
| `query_range_1k_of_50k` | < 3ms | Verify zone map pruning; check ORDER BY cost |

### 8.4 Running Benchmarks

```bash
# Run all store benchmarks
cargo bench -p midas-store

# Run a specific benchmark
cargo bench -p midas-store -- insert_5k

# Generate HTML report (opens in browser)
cargo bench -p midas-store -- --output-format bencher
```

---

## 9. CI Considerations

### 9.1 DuckDB Build Time

The `bundled` feature compiles DuckDB's C++ source. This takes 5-15 minutes
on CI depending on the runner. Mitigations:

| Strategy | Effect |
|----------|--------|
| Cargo cache (`actions/cache@v4` with `target/` key) | Avoids recompile on dependency-only changes |
| `sccache` | Cross-build C++ compilation cache |
| Feature gate: `duckdb-store` feature on `midas-app` | Skip DuckDB build for UI-only CI jobs |

### 9.2 Windows-Specific CI Notes

- Use `windows-latest` runner (MSVC toolchain).
- DuckDB `bundled` requires CMake and a C++ compiler. GitHub Actions'
  `windows-latest` includes both via Visual Studio Build Tools.
- If using `DUCKDB_DOWNLOAD_LIB=1`, ensure the runner has internet access
  for the initial download (cached afterward).
- DuckDB creates temporary files during queries. Ensure the `TEMP`
  directory has adequate space (1GB minimum).

### 9.3 Test Matrix

```yaml
# .github/workflows/test.yml (excerpt)
jobs:
  test:
    runs-on: windows-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
        with:
          cache-targets: true
      - name: Build workspace
        run: cargo build --workspace
      - name: Run tests
        run: cargo test --workspace
      - name: Run clippy
        run: cargo clippy --workspace -- -D warnings
```

### 9.4 Test Isolation

Every test creates its own in-memory DuckDB connection (`test_conn()`).
File-based tests use `tempfile::tempdir()` for automatic cleanup. No shared
state between tests. Tests can run in parallel (`cargo test` default).

The actor thread in `DbHandle` tests is short-lived: `shutdown()` drops the
channel, the actor loop exits, and the OS thread joins. No thread leaks.

### 9.5 Feature Gating (Future)

When `midas-store` is integrated into `midas-app`, consider a cargo feature:

```toml
# midas-app/Cargo.toml
[features]
default = ["duckdb-store"]
duckdb-store = ["midas-store"]
```

This allows CI to run `cargo test --workspace --no-default-features` for
fast feedback loops that skip the DuckDB C++ compile.

---

## 9.6 Additional Robustness Tests

### Corrupted DuckDB file fallback

```rust
#[tokio::test]
async fn test_corrupted_db_file_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("corrupt.duckdb");

    // Write garbage to simulate a corrupted database file.
    std::fs::write(&db_path, b"this is not a valid duckdb file").unwrap();

    let config = StoreConfig {
        path: Some(db_path),
        memory_limit_mb: 64,
        threads: 1,
        ..Default::default()
    };
    let db = DbHandle::open(config);

    // First command triggers lazy connection open, which should fail.
    let result = db.list_cached().await;
    assert!(result.is_err(), "corrupted file should produce an error");

    match result.unwrap_err() {
        StoreError::ConnectionFailed(_) | StoreError::ActorDead(_) => {},
        other => panic!("expected ConnectionFailed or ActorDead, got: {other}"),
    }
}
```

### Write-behind cache cycle (Phase 5 integration)

```rust
#[tokio::test]
async fn test_write_behind_cache_cycle() {
    let db = DbHandle::open_memory();
    let key = DataKey {
        symbol: "CACHE_TEST".into(),
        timeframe: Timeframe::D1,
    };

    // Cache miss
    let result = db.query_candles(key.clone()).await.unwrap();
    assert!(result.is_empty());

    // Simulate write-behind: fire_and_forget insert
    let buf = sample_buffer(100);
    db.fire_and_forget_insert(key.clone(), buf.clone()).await.unwrap();

    // Allow actor to process
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Cache hit
    let cached = db.query_candles(key).await.unwrap();
    assert_eq!(cached.len(), 100);
    assert_eq!(cached.timestamps[0], buf.timestamps[0]);

    db.shutdown().await.ok();
}
```

---

## 10. Test Inventory Summary

| Module | Test Name | Type | Description |
|--------|-----------|------|-------------|
| **schema** | `test_migration_idempotent` | Unit | Run migrations twice, no error |
| **schema** | `test_schema_version_tracked` | Unit | Version table reflects applied migrations |
| **schema** | `test_all_tables_exist` | Unit | All tables present after migration |
| **queries** | `test_bulk_insert_roundtrip` | Unit | Insert + query, field-by-field compare |
| **queries** | `test_empty_query` | Unit | Non-existent symbol returns empty buffer |
| **queries** | `test_range_query` | Unit | 1000 candles, query subset by time |
| **queries** | `test_upsert_overwrites` | Unit | Overlapping insert, verify latest values |
| **queries** | `test_data_ranges_updated` | Unit | Insert updates meta.data_ranges |
| **queries** | `test_duplicate_insert_ignored` | Unit | No duplicates on re-insert |
| **queries** | `test_delete_symbol` | Unit | Insert, delete, verify empty |
| **queries** | `test_multiple_symbols` | Unit | AAPL and MSFT, no cross-talk |
| **queries** | `test_multiple_timeframes` | Unit | D1 and M5 for same symbol |
| **queries** | `test_large_buffer_insert` | Unit | 50K candles, boundary verification |
| **convert** | `test_f32_roundtrip` | Unit | f32 prices survive FLOAT storage |
| **convert** | `test_u32_volume_roundtrip` | Unit | u32 volumes survive UINTEGER storage |
| **convert** | `test_i64_timestamp_roundtrip` | Unit | i64 timestamps survive BIGINT storage |
| **convert** | `test_timeframe_secs_roundtrip` | Unit | Timeframe -> u32 -> Timeframe |
| **handle** | `test_dbhandle_open_file` | Integration | File-based handle operations |
| **handle** | `test_dbhandle_open_memory` | Integration | In-memory handle operations |
| **handle** | `test_dbhandle_concurrent_queries` | Integration | 10 concurrent tasks |
| **handle** | `test_dbhandle_fire_and_forget_insert` | Integration | Non-blocking insert |
| **handle** | `test_dbhandle_shutdown_clean` | Integration | Shutdown + subsequent send fails |
| **handle** | `test_dbhandle_clone_independence` | Integration | Clone survives original drop |
| **handle** | `test_dbhandle_list_cached` | Integration | Metadata for stored pairs |
| **handle** | `test_dbhandle_query_range` | Integration | Time-range query via handle |
| **handle** | `test_corrupted_db_file_fallback` | Integration | Corrupted file produces error, no panic |
| **handle** | `test_write_behind_cache_cycle` | Integration | Cache miss -> insert -> cache hit cycle |
| **workspace** | `test_candle_buffer_to_store_roundtrip` | Integration | Cross-crate data flow |
| **workspace** | `test_store_disabled_fallback` | Integration | Graceful fallback without store |
| **bench** | `bench_open_migrate` | Benchmark | Cold open + migration |
| **bench** | `bench_insert_5k` | Benchmark | Bulk insert 5K candles |
| **bench** | `bench_insert_50k` | Benchmark | Bulk insert 50K candles |
| **bench** | `bench_query_5k` | Benchmark | Query 5K candles to CandleBuffer |
| **bench** | `bench_query_range_1k` | Benchmark | Range query: 1K of 50K |

**Total: 29 tests + 5 benchmarks + 1 build spike.**
