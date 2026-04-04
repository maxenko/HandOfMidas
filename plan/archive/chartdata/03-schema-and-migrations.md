# 03 -- Schema, Migrations, and Query Layer

> midas-store schema design, version-tracked migrations, and every SQL/Rust
> query function needed for the DuckDB chart data cache.
>
> Revision 0 -- 2026-03-31

---

## Table of Contents

1. [Complete DDL](#1-complete-ddl)
2. [Migration System](#2-migration-system)
3. [Migration v1 SQL](#3-migration-v1-sql)
4. [Future Migration Pattern](#4-future-migration-pattern)
5. [Query Functions](#5-query-functions)
6. [Prepared Statement Caching](#6-prepared-statement-caching)
7. [Row-to-CandleBuffer Materialization](#7-row-to-candlebuffer-materialization)
8. [Bulk Insert Optimization](#8-bulk-insert-optimization)
9. [Time Bucket Aggregation](#9-time-bucket-aggregation)
10. [Data Integrity](#10-data-integrity)

---

## 1. Complete DDL

### 1.1 Schema Namespaces

DuckDB supports `CREATE SCHEMA` for logical grouping. Three schemas partition
the database by concern:

```sql
-- market: time-series price data (candles, future: ticks)
CREATE SCHEMA IF NOT EXISTS market;

-- meta: cache inventory and symbol catalog (small, frequently updated)
CREATE SCHEMA IF NOT EXISTS meta;

-- cache: pre-computed derived data (future: indicator cache, aggregated bars)
CREATE SCHEMA IF NOT EXISTS cache;
```

**Why three schemas instead of one?**
- `DROP SCHEMA market CASCADE` wipes all price data without touching metadata.
- Logical grouping in DuckDB's `information_schema` queries.
- Future: different backup/export strategies per schema (e.g., Parquet export
  only for `market`).

### 1.2 Schema Version Table

Lives in the default (`main`) schema. Created first, before any other table.

```sql
-- Tracks applied migrations. Each row is one migration.
-- Created in the default schema (not inside market/meta/cache) so it
-- survives DROP SCHEMA operations on the data schemas.
CREATE TABLE IF NOT EXISTS schema_version (
    -- Migration version number. Monotonically increasing, starting at 1.
    -- The highest value present is the current schema version.
    version     INTEGER     NOT NULL PRIMARY KEY,

    -- UTC timestamp when this migration was applied.
    -- DuckDB CURRENT_TIMESTAMP returns UTC.
    applied_at  TIMESTAMP   NOT NULL DEFAULT CURRENT_TIMESTAMP,

    -- Human-readable description for debugging. Not parsed programmatically.
    description VARCHAR     NOT NULL DEFAULT ''
);
```

**Design decisions:**
- `PRIMARY KEY` on `version` prevents double-application.
- `applied_at` uses DuckDB's `TIMESTAMP` (microsecond precision, UTC).
- `description` is optional metadata for `SELECT * FROM schema_version` debugging.
- Lives in `main` schema so `DROP SCHEMA market CASCADE` cannot destroy
  migration history.

### 1.3 market.candles -- Primary OHLCV Storage

```sql
CREATE TABLE market.candles (
    -- Symbol ticker string. Matches ContractSpec::symbol() output.
    -- VARCHAR, not an integer FK, because symbol names are the primary
    -- lookup key throughout the codebase and IB API responses.
    symbol          VARCHAR     NOT NULL,

    -- Candle period in seconds. Maps directly to Timeframe::as_secs().
    -- INTEGER (not VARCHAR) to avoid string parsing on every query.
    -- Values: 1, 5, 15, 30, 60, 300, 900, 1800, 3600, 14400, 86400,
    --         604800, 2592000
    timeframe_secs  INTEGER     NOT NULL,

    -- Bar open time as epoch milliseconds. Matches CandleBuffer.timestamps
    -- (Vec<i64>). BIGINT, not TIMESTAMP, to avoid conversion overhead at
    -- the Rust boundary. Use epoch_ms()/make_timestamp() in SQL analytics
    -- when date functions are needed.
    timestamp_ms    BIGINT      NOT NULL,

    -- OHLCV prices as FLOAT (32-bit IEEE 754). Matches CandleBuffer's
    -- Vec<f32> exactly. No precision loss on roundtrip. Cast to DOUBLE
    -- in SQL when computing indicators that need higher precision.
    open            FLOAT       NOT NULL,
    high            FLOAT       NOT NULL,
    low             FLOAT       NOT NULL,
    close           FLOAT       NOT NULL,

    -- Trade volume. UINTEGER maps to u32 in Rust, matching CandleBuffer's
    -- Vec<u32>. Capped at u32::MAX (~4.29 billion) which covers all
    -- equity and futures volume.
    volume          UINTEGER    NOT NULL,

    -- Compound primary key enforces uniqueness per (symbol, timeframe, time).
    -- DuckDB stores this as an ART index. Zone maps on each column enable
    -- efficient range scans without secondary indexes.
    PRIMARY KEY (symbol, timeframe_secs, timestamp_ms)
);
```

**Why compound PK over separate indexes:**
- A single compound PK handles both uniqueness enforcement and the primary
  query pattern (`WHERE symbol = ? AND timeframe_secs = ? ORDER BY timestamp_ms`).
- DuckDB's zone maps (min/max per row group per column) automatically provide
  row-group pruning for range scans. No additional index needed.
- Adding a surrogate integer PK would waste space and provide no query benefit
  for time-series access patterns.

**Why no UNIQUE constraint separately:**
- The PK constraint *is* the uniqueness constraint. `INSERT OR REPLACE` and
  `INSERT OR IGNORE` both use the PK for conflict detection.

### 1.4 meta.data_ranges -- Cache Inventory

```sql
CREATE TABLE meta.data_ranges (
    -- Same (symbol, timeframe_secs) as candles table. This is the "index"
    -- that tells the application what data exists without scanning candles.
    symbol          VARCHAR     NOT NULL,
    timeframe_secs  INTEGER     NOT NULL,

    -- Number of candles currently stored for this key. Updated on every
    -- insert/delete. Used for UI display ("5,234 daily bars cached").
    candle_count    INTEGER     NOT NULL DEFAULT 0,

    -- Timestamp range of stored data (epoch ms). Used to determine cache
    -- hit/miss: if the requested range falls within [first_ts, last_ts],
    -- the data is available locally.
    first_ts        BIGINT      NOT NULL DEFAULT 0,
    last_ts         BIGINT      NOT NULL DEFAULT 0,

    -- Data source identifier. Helps with debugging and future cache
    -- invalidation policies.
    -- Values: 'csv', 'ib_historical', 'ib_stream', 'test', 'aggregated'
    source          VARCHAR     NOT NULL DEFAULT 'csv',

    -- Last time this entry was modified. UTC.
    updated_at      TIMESTAMP   NOT NULL DEFAULT CURRENT_TIMESTAMP,

    PRIMARY KEY (symbol, timeframe_secs)
);
```

**Why a separate metadata table instead of querying candles directly?**
- `SELECT COUNT(*), MIN(timestamp_ms), MAX(timestamp_ms) FROM market.candles
   WHERE symbol = ? AND timeframe_secs = ?` requires scanning an entire
  row group. With 500 symbols x 13 timeframes, startup inventory would scan
  the full table.
- `meta.data_ranges` makes inventory queries O(1) per key: `SELECT * FROM
  meta.data_ranges` returns all cached ranges instantly.
- Trade-off: must keep data_ranges in sync with candles. This is managed
  by `update_data_ranges()` called after every insert/delete.

### 1.5 meta.symbols -- Symbol Catalog

```sql
CREATE TABLE meta.symbols (
    -- Primary identifier. Matches ContractSpec::symbol() output.
    symbol      VARCHAR     PRIMARY KEY,

    -- Human-readable company/instrument name. Nullable because IB
    -- contract details may not be available at insert time.
    name        VARCHAR,

    -- SecurityType::Display form: 'STK', 'OPT', 'FUT', 'CASH'.
    -- Parse via SecurityType::from_str() on read. VARCHAR (not enum)
    -- because DuckDB has no Rust enum mapping, and the SecurityType
    -- enum in midas-core is the canonical type.
    sec_type    VARCHAR     NOT NULL DEFAULT 'STK',

    -- IB exchange routing. 'SMART' is IB's smart routing default.
    exchange    VARCHAR     NOT NULL DEFAULT 'SMART',

    -- ISO 4217 currency code.
    currency    VARCHAR     NOT NULL DEFAULT 'USD',

    -- IB contract ID. Nullable until resolved via IB API.
    -- Used by SymbolKey for efficient comparison on hot path.
    con_id      INTEGER,

    -- Minimum price increment. Nullable until resolved.
    min_tick    DOUBLE,

    -- Last time this row was updated. UTC.
    updated_at  TIMESTAMP   NOT NULL DEFAULT CURRENT_TIMESTAMP
);
```

> **v1 note:** `meta.symbols` is included in the v1 migration for forward
> compatibility with IB contract resolution. No queries, commands, or tests
> target this table in v1. It will be populated when IB API integration is
> implemented.

**Why store symbol metadata in DuckDB instead of SQLite?**
- The symbol catalog is read alongside candle data. Co-locating it avoids
  a cross-database join at query time.
- The broker's SQLite database may not be available (e.g., no IB connection).
- Write frequency is very low (once per new symbol), so DuckDB's higher
  per-transaction overhead is irrelevant.

---

## 2. Migration System

### 2.1 Design Principles

1. **Sequential version numbers.** Each migration has a `u32` version starting
   at 1. There are no gaps.
2. **Idempotent startup.** `run_migrations(conn)` is called on every app start.
   It checks `schema_version`, determines which migrations have been applied,
   and runs only the missing ones. Safe to call repeatedly.
3. **Forward-only.** No rollback support. If a migration fails, the application
   panics at startup with a clear error. This is acceptable for a single-user
   desktop app where the developer controls all migrations.
4. **Transaction per migration.** Each migration runs inside a transaction.
   If the SQL fails, the transaction rolls back and `schema_version` is not
   updated for that version.
5. **Compile-time SQL.** All migration SQL is embedded as `&'static str` in
   a const array. No file I/O, no runtime SQL loading.

### 2.2 Migration Struct and Registry

```rust
/// A single schema migration.
struct Migration {
    /// Sequential version number (1, 2, 3, ...).
    version: u32,
    /// Human-readable description for the schema_version table.
    description: &'static str,
    /// SQL to execute. May contain multiple statements separated by ';'.
    sql: &'static str,
}

/// All migrations in application order. Append new migrations to the end.
/// Never modify or remove existing entries.
const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        description: "initial schema: market.candles, meta.data_ranges, meta.symbols",
        sql: MIGRATION_V1_SQL,
    },
    // Future:
    // Migration {
    //     version: 2,
    //     description: "add market.ticks table",
    //     sql: MIGRATION_V2_SQL,
    // },
];
```

### 2.3 run_migrations() Implementation

```rust
use duckdb::{Connection, params};
use tracing::{info, warn};

/// Apply all pending migrations to the database.
///
/// Called once during `DbHandle::open()`. Safe to call on every startup:
/// - Creates `schema_version` table if it does not exist.
/// - Skips migrations already recorded in `schema_version`.
/// - Applies remaining migrations sequentially, each in its own transaction.
///
/// # Errors
///
/// Returns `StoreError::MigrationFailed` if any migration SQL fails to
/// execute. The database will contain all successfully-applied migrations
/// up to (but not including) the failed one.
///
/// # Panics
///
/// Does not panic. All errors are returned as `Result::Err`.
pub fn run_migrations(conn: &Connection) -> Result<(), StoreError> {
    // Step 1: Ensure schema_version table exists.
    // This is the bootstrap: the very first table, created outside the
    // migration system. IF NOT EXISTS makes it safe to run repeatedly.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_version (
            version     INTEGER     NOT NULL PRIMARY KEY,
            applied_at  TIMESTAMP   NOT NULL DEFAULT CURRENT_TIMESTAMP,
            description VARCHAR     NOT NULL DEFAULT ''
        );"
    ).map_err(|e| StoreError::MigrationFailed(format!(
        "failed to create schema_version table: {e}"
    )))?;

    // Step 2: Determine the highest applied version.
    let current_version: u32 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |row| row.get(0),
        )
        .map_err(|e| StoreError::MigrationFailed(format!(
            "failed to read current schema version: {e}"
        )))?;

    info!(current_version, total_migrations = MIGRATIONS.len(),
          "checking schema migrations");

    // Step 3: Apply each pending migration in order.
    for migration in MIGRATIONS {
        if migration.version <= current_version {
            continue; // Already applied.
        }

        info!(version = migration.version, desc = migration.description,
              "applying migration");

        // Begin transaction for this migration.
        conn.execute_batch("BEGIN TRANSACTION;")
            .map_err(|e| StoreError::MigrationFailed(format!(
                "failed to begin transaction for migration v{}: {e}",
                migration.version
            )))?;

        // Execute migration SQL.
        match conn.execute_batch(migration.sql) {
            Ok(()) => {
                // Record the migration in schema_version.
                conn.execute(
                    "INSERT INTO schema_version (version, description) VALUES (?, ?)",
                    params![migration.version, migration.description],
                ).map_err(|e| {
                    // Attempt rollback (best-effort).
                    let _ = conn.execute_batch("ROLLBACK;");
                    StoreError::MigrationFailed(format!(
                        "migration v{} succeeded but failed to record: {e}",
                        migration.version
                    ))
                })?;

                conn.execute_batch("COMMIT;")
                    .map_err(|e| StoreError::MigrationFailed(format!(
                        "failed to commit migration v{}: {e}",
                        migration.version
                    )))?;

                info!(version = migration.version, "migration applied successfully");
            }
            Err(e) => {
                // Rollback the failed migration.
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(StoreError::MigrationFailed(format!(
                    "migration v{} failed: {e}\nSQL:\n{}",
                    migration.version, migration.sql
                )));
            }
        }
    }

    Ok(())
}
```

**Important DuckDB-specific note:** DuckDB transactions are not nested.
`BEGIN` inside an active transaction is an error. Each migration must be
a self-contained `BEGIN ... COMMIT` unit. The `execute_batch` in the
migration SQL must NOT contain `BEGIN`/`COMMIT` statements.

---

## 3. Migration v1 SQL

The full initial migration, stored as a `const &str`:

```rust
const MIGRATION_V1_SQL: &str = r#"
-- ================================================================
-- SCHEMAS
-- ================================================================
CREATE SCHEMA IF NOT EXISTS market;
CREATE SCHEMA IF NOT EXISTS meta;
CREATE SCHEMA IF NOT EXISTS cache;

-- ================================================================
-- market.candles — Primary OHLCV storage
-- ================================================================
-- Compound PK: (symbol, timeframe_secs, timestamp_ms)
-- Timestamps as BIGINT (epoch ms) to match CandleBuffer.
-- Prices as FLOAT (f32) to match CandleBuffer.
-- Volume as UINTEGER (u32) to match CandleBuffer.
CREATE TABLE market.candles (
    symbol          VARCHAR     NOT NULL,
    timeframe_secs  INTEGER     NOT NULL,
    timestamp_ms    BIGINT      NOT NULL,
    open            FLOAT       NOT NULL,
    high            FLOAT       NOT NULL,
    low             FLOAT       NOT NULL,
    close           FLOAT       NOT NULL,
    volume          UINTEGER    NOT NULL,
    PRIMARY KEY (symbol, timeframe_secs, timestamp_ms)
);

-- ================================================================
-- meta.data_ranges — Cache inventory
-- ================================================================
-- One row per (symbol, timeframe) pair.
-- Avoids scanning market.candles for cache hit/miss checks.
CREATE TABLE meta.data_ranges (
    symbol          VARCHAR     NOT NULL,
    timeframe_secs  INTEGER     NOT NULL,
    candle_count    INTEGER     NOT NULL DEFAULT 0,
    first_ts        BIGINT      NOT NULL DEFAULT 0,
    last_ts         BIGINT      NOT NULL DEFAULT 0,
    source          VARCHAR     NOT NULL DEFAULT 'csv',
    updated_at      TIMESTAMP   NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (symbol, timeframe_secs)
);

-- ================================================================
-- meta.symbols — Symbol catalog
-- ================================================================
CREATE TABLE meta.symbols (
    symbol      VARCHAR     PRIMARY KEY,
    name        VARCHAR,
    sec_type    VARCHAR     NOT NULL DEFAULT 'STK',
    exchange    VARCHAR     NOT NULL DEFAULT 'SMART',
    currency    VARCHAR     NOT NULL DEFAULT 'USD',
    con_id      INTEGER,
    min_tick    DOUBLE,
    updated_at  TIMESTAMP   NOT NULL DEFAULT CURRENT_TIMESTAMP
);
"#;
```

Note: `CREATE SCHEMA IF NOT EXISTS` is idempotent, but within the migration
system this SQL only runs once (guarded by `schema_version`). The `IF NOT EXISTS`
is a safety net for manual recovery scenarios.

---

## 4. Future Migration Pattern

### 4.1 Adding a New Table (v2 example)

```rust
const MIGRATION_V2_SQL: &str = r#"
-- Add tick data storage for IB streaming.
CREATE TABLE market.ticks (
    symbol       VARCHAR     NOT NULL,
    timestamp_ms BIGINT      NOT NULL,
    price        DOUBLE      NOT NULL,
    size         UINTEGER    NOT NULL,
    exchange     VARCHAR,
    conditions   VARCHAR
);
-- No PK: ticks can share timestamps. Insert-only, never updated.
"#;
```

Then append to `MIGRATIONS`:

```rust
const MIGRATIONS: &[Migration] = &[
    Migration { version: 1, ... },
    Migration {
        version: 2,
        description: "add market.ticks table for streaming tick data",
        sql: MIGRATION_V2_SQL,
    },
];
```

### 4.2 Altering an Existing Table (v3 example)

```rust
const MIGRATION_V3_SQL: &str = r#"
-- Add 'adjusted' flag for split-adjusted prices.
ALTER TABLE market.candles ADD COLUMN adjusted BOOLEAN NOT NULL DEFAULT FALSE;

-- Add source tracking per candle row.
ALTER TABLE market.candles ADD COLUMN source VARCHAR NOT NULL DEFAULT 'csv';
"#;
```

### 4.3 Data Transform Migration (v4 example)

```rust
const MIGRATION_V4_SQL: &str = r#"
-- Migrate sec_type values from legacy names to IB codes.
UPDATE meta.symbols SET sec_type = 'STK' WHERE sec_type = 'STOCK';
UPDATE meta.symbols SET sec_type = 'OPT' WHERE sec_type = 'OPTION';
UPDATE meta.symbols SET sec_type = 'FUT' WHERE sec_type = 'FUTURE';
UPDATE meta.symbols SET sec_type = 'CASH' WHERE sec_type = 'FOREX';
"#;
```

### 4.4 Rules for Writing Migrations

1. **Never modify a shipped migration.** Once a version is released, its SQL
   is immutable. Fix forward with a new version.
2. **Never reorder versions.** Append only.
3. **Test migrations on a fresh database AND on a database at version N-1.**
   The CI test `test_migrations_fresh` opens in-memory, runs all migrations,
   verifies schema. The test `test_migrations_incremental` applies v1, inserts
   data, then applies v2+, verifies data survives.
4. **Keep migrations small.** One logical change per version. Makes rollback
   diagnosis easier.
5. **No `IF NOT EXISTS` inside migrations** (except for schemas). The migration
   system guarantees each migration runs exactly once. `IF NOT EXISTS` hides
   bugs where a table was created outside the migration system.

---

## 5. Query Functions

### 5.1 DataKey and CacheInfo Types

```rust
use midas_core::Timeframe;

/// Identifies a specific candle series in the cache.
#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub struct DataKey {
    pub symbol: String,
    pub timeframe: Timeframe,
}

impl DataKey {
    /// Returns the timeframe duration in seconds, for use in SQL queries.
    /// Maps directly to Timeframe::as_secs() but returns u32 for SQL params.
    pub fn timeframe_secs(&self) -> u32 {
        self.timeframe.as_secs() as u32
    }
}

/// Metadata about a cached candle series. Returned by `list_cached()`.
#[derive(Clone, Debug)]
pub struct CacheInfo {
    pub key: DataKey,
    pub candle_count: usize,
    pub first_ts: i64,
    pub last_ts: i64,
    pub source: String,
}
```

### 5.2 bulk_insert -- Appender-Based Fast Insert

```rust
use duckdb::{Connection, params};
use midas_data::CandleBuffer;

/// Insert candles into market.candles using DuckDB's Appender API.
///
/// The Appender bypasses SQL parsing entirely, writing directly to DuckDB's
/// internal columnar storage. This is 5-10x faster than prepared INSERT
/// statements for batch loads.
///
/// # Arguments
///
/// * `conn` - DuckDB connection. Must be on the same thread (Appender is !Send).
/// * `key` - Symbol and timeframe identifying this series.
/// * `buffer` - The candle data to insert. SoA layout is iterated by index.
///
/// # Returns
///
/// The number of rows inserted.
///
/// # Errors
///
/// Returns `StoreError::InsertFailed` if the Appender cannot be created
/// (e.g., table does not exist) or if any row fails to append.
///
/// # Duplicate Handling
///
/// Rows that violate the compound PK (duplicate symbol+timeframe+timestamp)
/// cause the Appender to error. Caller must ensure no duplicates, or use
/// `upsert_candles()` for overlapping data.
pub fn bulk_insert(
    conn: &Connection,
    key: &DataKey,
    buffer: &CandleBuffer,
) -> Result<usize, StoreError> {
    if buffer.is_empty() {
        return Ok(0);
    }

    let tf_secs = key.timeframe_secs();
    let mut appender = conn
        .appender("market", "candles")
        .map_err(|e| StoreError::InsertFailed(format!(
            "failed to create appender for market.candles: {e}"
        )))?;

    for i in 0..buffer.len() {
        appender
            .append_row(params![
                key.symbol.as_str(),
                tf_secs,
                buffer.timestamps[i],
                buffer.opens[i],
                buffer.highs[i],
                buffer.lows[i],
                buffer.closes[i],
                buffer.volumes[i],
            ])
            .map_err(|e| StoreError::InsertFailed(format!(
                "appender row {i} failed: {e}"
            )))?;
    }

    // flush() sends buffered rows to storage. Also called on drop,
    // but explicit flush surfaces errors instead of silently discarding.
    appender.flush().map_err(|e| StoreError::InsertFailed(format!(
        "appender flush failed: {e}"
    )))?;

    Ok(buffer.len())
}
```

**Note on `conn.appender()` signature:** The DuckDB Rust crate's `appender()`
method takes `(schema, table)` as separate arguments when the table is in a
non-default schema. If the crate version uses a single argument, use
`"market.candles"` as the table name string.

### 5.3 query_candles -- Full Range Scan

```rust
/// Query all candles for a given symbol and timeframe, ordered by time.
///
/// Results are materialized into a `CandleBuffer` (SoA layout) for direct
/// use by the chart renderer and indicator engine.
///
/// # Arguments
///
/// * `conn` - DuckDB connection.
/// * `key` - Symbol and timeframe to query.
///
/// # Returns
///
/// A `CandleBuffer` containing all candles for this key, ordered by
/// ascending timestamp. Returns an empty buffer if no data exists.
pub fn query_candles(
    conn: &Connection,
    key: &DataKey,
) -> Result<CandleBuffer, StoreError> {
    let mut stmt = conn
        .prepare_cached(
            "SELECT timestamp_ms, open, high, low, close, volume
             FROM market.candles
             WHERE symbol = ? AND timeframe_secs = ?
             ORDER BY timestamp_ms ASC"
        )
        .map_err(|e| StoreError::QueryFailed(format!("prepare failed: {e}")))?;

    let tf_secs = key.timeframe_secs();

    let rows = stmt
        .query_map(params![&key.symbol, tf_secs], |row| {
            Ok((
                row.get::<_, i64>(0)?,   // timestamp_ms
                row.get::<_, f32>(1)?,   // open
                row.get::<_, f32>(2)?,   // high
                row.get::<_, f32>(3)?,   // low
                row.get::<_, f32>(4)?,   // close
                row.get::<_, u32>(5)?,   // volume
            ))
        })
        .map_err(|e| StoreError::QueryFailed(format!("query failed: {e}")))?;

    // Pre-allocate for typical chart load. 5000 daily candles = ~20 years.
    let mut buffer = CandleBuffer::with_capacity(5000);

    for row_result in rows {
        let (ts, o, h, l, c, v) = row_result
            .map_err(|e| StoreError::QueryFailed(format!("row read failed: {e}")))?;
        buffer.push(ts, o, h, l, c, v);
    }

    Ok(buffer)
}
```

### 5.4 query_candles_range -- Time-Bounded Query

```rust
/// Query candles within a specific time range [start_ms, end_ms], inclusive.
///
/// Used for:
/// - Loading only the visible portion of a chart (lazy loading).
/// - Fetching a gap range to merge with existing in-memory data.
///
/// # Arguments
///
/// * `conn` - DuckDB connection.
/// * `key` - Symbol and timeframe.
/// * `start_ms` - Start of range, inclusive (epoch ms).
/// * `end_ms` - End of range, inclusive (epoch ms).
///
/// # Returns
///
/// Candles within the range, ordered by ascending timestamp. Empty buffer
/// if no data exists in the range.
pub fn query_candles_range(
    conn: &Connection,
    key: &DataKey,
    start_ms: i64,
    end_ms: i64,
) -> Result<CandleBuffer, StoreError> {
    let mut stmt = conn
        .prepare_cached(
            "SELECT timestamp_ms, open, high, low, close, volume
             FROM market.candles
             WHERE symbol = ? AND timeframe_secs = ?
               AND timestamp_ms >= ? AND timestamp_ms <= ?
             ORDER BY timestamp_ms ASC"
        )
        .map_err(|e| StoreError::QueryFailed(format!("prepare failed: {e}")))?;

    let tf_secs = key.timeframe_secs();

    let rows = stmt
        .query_map(params![&key.symbol, tf_secs, start_ms, end_ms], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, f32>(1)?,
                row.get::<_, f32>(2)?,
                row.get::<_, f32>(3)?,
                row.get::<_, f32>(4)?,
                row.get::<_, u32>(5)?,
            ))
        })
        .map_err(|e| StoreError::QueryFailed(format!("query failed: {e}")))?;

    // Estimate capacity: daily candles over 20 years ~= 5000.
    // For sub-minute timeframes the caller knows better, but 5000 is a
    // reasonable default that avoids excessive reallocation.
    let mut buffer = CandleBuffer::with_capacity(5000);

    for row_result in rows {
        let (ts, o, h, l, c, v) = row_result
            .map_err(|e| StoreError::QueryFailed(format!("row read failed: {e}")))?;
        buffer.push(ts, o, h, l, c, v);
    }

    Ok(buffer)
}
```

### 5.5 list_cached -- Cache Inventory

```rust
/// List all cached data series with their metadata.
///
/// Reads from `meta.data_ranges`, not from `market.candles`. This is an
/// O(N) scan over the small metadata table, not a full table scan.
///
/// # Returns
///
/// A vector of `CacheInfo` entries, one per (symbol, timeframe) pair that
/// has data in the cache.
pub fn list_cached(conn: &Connection) -> Result<Vec<CacheInfo>, StoreError> {
    let mut stmt = conn
        .prepare_cached(
            "SELECT symbol, timeframe_secs, candle_count, first_ts, last_ts, source
             FROM meta.data_ranges
             ORDER BY symbol ASC, timeframe_secs ASC"
        )
        .map_err(|e| StoreError::QueryFailed(format!("prepare failed: {e}")))?;

    let rows = stmt
        .query_map([], |row| {
            let symbol: String = row.get(0)?;
            let tf_secs: u32 = row.get(1)?;
            let candle_count: i32 = row.get(2)?;
            let first_ts: i64 = row.get(3)?;
            let last_ts: i64 = row.get(4)?;
            let source: String = row.get(5)?;
            Ok((symbol, tf_secs, candle_count, first_ts, last_ts, source))
        })
        .map_err(|e| StoreError::QueryFailed(format!("query failed: {e}")))?;

    let mut result = Vec::new();

    for row_result in rows {
        let (symbol, tf_secs, count, first, last, source) = row_result
            .map_err(|e| StoreError::QueryFailed(format!("row read failed: {e}")))?;

        // Convert timeframe_secs back to Timeframe enum.
        // Skip entries with unknown timeframe values (forward compatibility).
        let timeframe = match timeframe_from_secs(tf_secs) {
            Some(tf) => tf,
            None => {
                tracing::warn!(tf_secs, symbol, "unknown timeframe_secs in data_ranges, skipping");
                continue;
            }
        };

        result.push(CacheInfo {
            key: DataKey { symbol, timeframe },
            candle_count: count as usize,
            first_ts: first,
            last_ts: last,
            source,
        });
    }

    Ok(result)
}

/// Convert a timeframe duration in seconds to a Timeframe enum variant.
///
/// Returns None for values that do not map to a known variant.
fn timeframe_from_secs(secs: u32) -> Option<Timeframe> {
    match secs {
        1       => Some(Timeframe::S1),
        5       => Some(Timeframe::S5),
        15      => Some(Timeframe::S15),
        30      => Some(Timeframe::S30),
        60      => Some(Timeframe::M1),
        300     => Some(Timeframe::M5),
        900     => Some(Timeframe::M15),
        1800    => Some(Timeframe::M30),
        3600    => Some(Timeframe::H1),
        14400   => Some(Timeframe::H4),
        86400   => Some(Timeframe::D1),
        604800  => Some(Timeframe::W1),
        2592000 => Some(Timeframe::MN1),
        _       => None,
    }
}
```

**Note on `timeframe_from_secs`:** This function should ideally live on
`Timeframe` itself in `midas-core` (e.g., `Timeframe::from_secs(u32) -> Option<Self>`).
If adding it to midas-core is not desired in this phase, keep the private
helper in midas-store. When/if `Timeframe::from_secs` is added to midas-core,
delete this helper and call the canonical implementation.

### 5.6 upsert_candles -- Overlapping Data

```rust
/// Insert candles, replacing any existing rows with the same PK.
///
/// Used when re-importing data that overlaps with existing cached data.
/// For example, re-downloading the last 30 days of daily bars where some
/// bars were already cached.
///
/// Strategy: DELETE the overlapping time range, then bulk insert.
/// This is faster than row-by-row INSERT OR REPLACE for large overlaps
/// because the Appender API cannot handle conflict resolution.
///
/// # Arguments
///
/// * `conn` - DuckDB connection.
/// * `key` - Symbol and timeframe.
/// * `buffer` - New candle data. Must be time-sorted (CandleBuffer invariant).
///
/// # Returns
///
/// The number of rows inserted (after deletion of overlapping rows).
pub fn upsert_candles(
    conn: &Connection,
    key: &DataKey,
    buffer: &CandleBuffer,
) -> Result<usize, StoreError> {
    if buffer.is_empty() {
        return Ok(0);
    }

    let tf_secs = key.timeframe_secs();

    // Determine the time range of incoming data.
    let first_ts = buffer.timestamps[0];
    let last_ts = buffer.timestamps[buffer.len() - 1];

    // Delete existing rows in the overlapping range.
    conn.execute(
        "DELETE FROM market.candles
         WHERE symbol = ? AND timeframe_secs = ?
           AND timestamp_ms >= ? AND timestamp_ms <= ?",
        params![&key.symbol, tf_secs, first_ts, last_ts],
    ).map_err(|e| StoreError::InsertFailed(format!(
        "delete before upsert failed: {e}"
    )))?;

    // Insert the new data via Appender.
    bulk_insert(conn, key, buffer)
}
```

### 5.7 update_data_ranges -- Metadata Bookkeeping

```rust
/// Update the cache metadata after inserting or deleting candles.
///
/// This must be called after every `bulk_insert()`, `upsert_candles()`, or
/// `delete_symbol()` to keep `meta.data_ranges` in sync with `market.candles`.
///
/// # Arguments
///
/// * `conn` - DuckDB connection.
/// * `key` - Symbol and timeframe that was modified.
/// * `count` - Current total candle count for this key.
/// * `first_ts` - Timestamp of the earliest candle (epoch ms).
/// * `last_ts` - Timestamp of the latest candle (epoch ms).
/// * `source` - Data source identifier (e.g., "csv", "ib_historical").
pub fn update_data_ranges(
    conn: &Connection,
    key: &DataKey,
    count: usize,
    first_ts: i64,
    last_ts: i64,
    source: &str,
) -> Result<(), StoreError> {
    conn.execute(
        "INSERT OR REPLACE INTO meta.data_ranges
         (symbol, timeframe_secs, candle_count, first_ts, last_ts, source, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, CURRENT_TIMESTAMP)",
        params![
            &key.symbol,
            key.timeframe_secs(),
            count as i32,
            first_ts,
            last_ts,
            source,
        ],
    ).map_err(|e| StoreError::QueryFailed(format!(
        "update_data_ranges failed: {e}"
    )))?;

    Ok(())
}

/// Recalculate and update data_ranges from the actual candle data.
///
/// Useful after bulk operations where tracking incremental changes is
/// impractical. Performs a single aggregate query against market.candles.
pub fn refresh_data_ranges(
    conn: &Connection,
    key: &DataKey,
    source: &str,
) -> Result<(), StoreError> {
    let tf_secs = key.timeframe_secs();

    let (count, first_ts, last_ts): (i32, i64, i64) = conn
        .query_row(
            "SELECT COUNT(*)::INTEGER,
                    COALESCE(MIN(timestamp_ms), 0),
                    COALESCE(MAX(timestamp_ms), 0)
             FROM market.candles
             WHERE symbol = ? AND timeframe_secs = ?",
            params![&key.symbol, tf_secs],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|e| StoreError::QueryFailed(format!(
            "refresh_data_ranges aggregate query failed: {e}"
        )))?;

    if count == 0 {
        // No data remains; delete the metadata entry.
        conn.execute(
            "DELETE FROM meta.data_ranges WHERE symbol = ? AND timeframe_secs = ?",
            params![&key.symbol, tf_secs],
        ).map_err(|e| StoreError::QueryFailed(format!(
            "delete empty data_ranges entry failed: {e}"
        )))?;
    } else {
        update_data_ranges(conn, key, count as usize, first_ts, last_ts, source)?;
    }

    Ok(())
}
```

### 5.8 delete_symbol -- Cleanup

```rust
/// Delete all data for a given symbol across all timeframes.
///
/// Removes rows from:
/// - `market.candles` (all timeframes for this symbol)
/// - `meta.data_ranges` (all metadata entries for this symbol)
/// - `meta.symbols` (the symbol catalog entry)
///
/// # Returns
///
/// The total number of candle rows deleted from `market.candles`.
pub fn delete_symbol(
    conn: &Connection,
    symbol: &str,
) -> Result<usize, StoreError> {
    // Delete candles first (largest table).
    let candle_count: usize = conn
        .execute(
            "DELETE FROM market.candles WHERE symbol = ?",
            params![symbol],
        )
        .map_err(|e| StoreError::QueryFailed(format!(
            "delete candles for {symbol} failed: {e}"
        )))?;

    // Delete metadata.
    conn.execute(
        "DELETE FROM meta.data_ranges WHERE symbol = ?",
        params![symbol],
    ).map_err(|e| StoreError::QueryFailed(format!(
        "delete data_ranges for {symbol} failed: {e}"
    )))?;

    // Delete symbol catalog entry.
    conn.execute(
        "DELETE FROM meta.symbols WHERE symbol = ?",
        params![symbol],
    ).map_err(|e| StoreError::QueryFailed(format!(
        "delete symbol {symbol} from catalog failed: {e}"
    )))?;

    Ok(candle_count)
}
```

### 5.9 vacuum -- Compaction

```rust
/// Run DuckDB's CHECKPOINT to force WAL flush and file compaction.
///
/// DuckDB checkpoints automatically, but calling this explicitly:
/// - After large bulk imports to reclaim WAL space.
/// - Before application exit for clean shutdown.
/// - On user request (e.g., "compact database" menu item).
///
/// This is a blocking operation that may take 100ms-1s depending on
/// WAL size. Call from the actor thread only.
pub fn vacuum(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch("CHECKPOINT;")
        .map_err(|e| StoreError::QueryFailed(format!("checkpoint failed: {e}")))?;

    Ok(())
}
```

**Why CHECKPOINT and not VACUUM?** DuckDB does not have a `VACUUM` command
in the SQLite sense. `CHECKPOINT` flushes the WAL to the main file and
compacts. For full database compaction, use `CHECKPOINT` followed by
`PRAGMA database_size` to verify.

---

## 6. Prepared Statement Caching

### 6.1 How DuckDB Statement Caching Works

DuckDB's `prepare_cached()` (from the duckdb-rs crate) maintains an internal
`HashMap<String, PreparedStatement>` on the `Connection`. On first call with
a given SQL string, it:

1. Parses the SQL.
2. Plans the query.
3. Creates a `PreparedStatement` and stores it in the cache.
4. Returns a handle to the cached statement.

On subsequent calls with the same SQL string, it returns the cached statement
directly, skipping parse and plan. The cache lives for the lifetime of the
`Connection`.

### 6.2 What to Cache and What Not to Cache

**Cache (use `prepare_cached`):**
- `query_candles` -- called on every symbol load / chart switch.
- `query_candles_range` -- called for lazy loading and gap-fill.
- `list_cached` -- called at startup and on cache inventory requests.
- `update_data_ranges` -- called after every insert.

**Do not cache:**
- Migration SQL (`execute_batch`) -- runs once at startup, contains multiple
  statements.
- `DELETE FROM market.candles WHERE symbol = ?` -- infrequent cleanup operation.
- `CHECKPOINT` -- rare explicit operation.

### 6.3 Statement Cache Sizing

The duckdb-rs crate uses an unbounded `HashMap` for the statement cache.
With the ~6 queries we cache, this uses negligible memory (~1KB total for
statement metadata). No configuration needed.

### 6.4 Thread Safety of Cached Statements

Since `Connection` is `!Sync` and lives on a single actor thread, cached
statements are never accessed concurrently. No synchronization overhead.

---

## 7. Row-to-CandleBuffer Materialization

### 7.1 The Conversion Pattern

Every query materializes results into `CandleBuffer` before returning to
the caller. This is the canonical conversion code:

```rust
/// Materialize a DuckDB result set into a CandleBuffer.
///
/// The result set must have columns in this exact order:
///   0: timestamp_ms (BIGINT -> i64)
///   1: open (FLOAT -> f32)
///   2: high (FLOAT -> f32)
///   3: low (FLOAT -> f32)
///   4: close (FLOAT -> f32)
///   5: volume (UINTEGER -> u32)
///
/// # Pre-allocation
///
/// The `capacity_hint` parameter is used to pre-allocate the six internal
/// Vecs. For known-size results, pass the exact count. For unknown-size
/// results, pass a reasonable estimate (5000 for daily charts, 50000 for
/// intraday).
fn materialize_rows<P: duckdb::Params>(
    stmt: &mut duckdb::CachedStatement<'_>,
    params: P,
    capacity_hint: usize,
) -> Result<CandleBuffer, StoreError> {
    let rows = stmt
        .query_map(params, |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, f32>(1)?,
                row.get::<_, f32>(2)?,
                row.get::<_, f32>(3)?,
                row.get::<_, f32>(4)?,
                row.get::<_, u32>(5)?,
            ))
        })
        .map_err(|e| StoreError::QueryFailed(format!("query failed: {e}")))?;

    let mut buffer = CandleBuffer::with_capacity(capacity_hint);

    for row_result in rows {
        let (ts, o, h, l, c, v) = row_result
            .map_err(|e| StoreError::QueryFailed(format!("row read failed: {e}")))?;
        buffer.push(ts, o, h, l, c, v);
    }

    Ok(buffer)
}
```

### 7.2 Why Not Use Arrow Zero-Copy?

DuckDB supports `query_arrow()` which returns `Vec<RecordBatch>` in Apache
Arrow columnar format. This *could* avoid the row-by-row iteration overhead.
However, for v1:

1. **CandleBuffer is the canonical type.** The chart engine, indicator engine,
   and GPU pipeline all consume `CandleBuffer` (via `CandleData` trait). An
   Arrow RecordBatch does not implement `CandleData`.
2. **Materialization is a one-time cost.** For 5000 candles, row iteration
   takes ~1ms. This happens once per chart load, not per frame.
3. **Arrow adds a dependency.** The `arrow` crate is large. Adding it just
   for materialization is not justified.

If profiling shows materialization as a bottleneck (unlikely), the future path
is: `query_arrow()` -> extract columns as `&[f32]`/`&[i64]` slices -> build
`CandleBuffer` from slices without per-row branching.

### 7.3 SoA vs AoS Consideration

The `query_map` closure returns a tuple `(i64, f32, f32, f32, f32, u32)`.
This is an AoS-to-SoA conversion: DuckDB stores data in columnar format,
the Rust row iterator presents it as rows, and we push each field into its
respective Vec.

An alternative would be six separate queries (one per column), but this would
be 6x the query overhead for no benefit. The row iteration approach is the
correct trade-off: one query, one pass, one allocation per Vec.

---

## 8. Bulk Insert Optimization

### 8.1 Appender Lifecycle

```
bulk_insert() called
    |
    v
conn.appender("market", "candles")    -- Create Appender (binds to table)
    |
    v
for i in 0..buffer.len() {
    appender.append_row(params![...])  -- Buffer rows internally
}
    |
    v
appender.flush()                       -- Write buffered rows to storage
    |
    v
appender dropped                       -- Releases internal resources
```

**Critical rules:**
1. Create one Appender per batch. Do not reuse across batches -- the Appender
   holds a write lock on the table's row groups.
2. Call `flush()` explicitly before the Appender is dropped. The `Drop` impl
   also calls flush, but errors are silently discarded on drop.
3. The Appender is `!Send` and `!Sync`. It must be created and used on the
   same thread as the `Connection`. The mailbox actor thread satisfies this.

### 8.2 Performance Characteristics

| Buffer size | Appender time | Prepared INSERT time | Speedup |
|-------------|---------------|----------------------|---------|
| 100 rows    | ~0.2ms        | ~1ms                 | 5x      |
| 1,000 rows  | ~0.5ms        | ~5ms                 | 10x     |
| 5,000 rows  | ~2ms          | ~25ms                | 12x     |
| 50,000 rows | ~15ms         | ~200ms               | 13x     |

The Appender bypasses SQL parsing, parameter binding, and plan optimization
for each row. It writes directly into DuckDB's internal columnar buffers.

### 8.3 SoA-to-Appender Mapping

The CandleBuffer SoA layout requires indexed access into six parallel arrays:

```rust
// Each field is accessed by index into its own contiguous array.
// This is cache-efficient: for each row, we read one element from
// each of six arrays. The arrays are contiguous in memory, so
// prefetching works well even though we stride across six arrays.
for i in 0..buffer.len() {
    appender.append_row(params![
        symbol,                 // VARCHAR (same for all rows)
        tf_secs,                // INTEGER (same for all rows)
        buffer.timestamps[i],   // BIGINT
        buffer.opens[i],        // FLOAT
        buffer.highs[i],        // FLOAT
        buffer.lows[i],         // FLOAT
        buffer.closes[i],       // FLOAT
        buffer.volumes[i],      // UINTEGER
    ])?;
}
```

The `symbol` and `tf_secs` values are constant for the entire batch. DuckDB's
Appender does not special-case constant columns, so they are repeated per row.
This is a minor inefficiency (<1% overhead) not worth optimizing.

### 8.4 Batching Strategy for Streaming Data

For future IB streaming integration, candles arrive one at a time. Calling
`bulk_insert` for each candle would create/destroy an Appender per row, losing
all performance benefit.

The actor thread should batch incoming candles:

```rust
// In the actor handler, accumulate candles in a pending buffer.
// Flush to DuckDB on a timer (every flush_interval_secs) or when
// the buffer exceeds a threshold (e.g., 100 candles).
struct ActorState {
    conn: Connection,
    pending: HashMap<DataKey, CandleBuffer>,
    last_flush: std::time::Instant,
}

// On DbCommand::InsertCandles:
state.pending.entry(key).or_default().push(ts, o, h, l, c, v);

// On timer tick or buffer threshold:
for (key, buffer) in state.pending.drain() {
    bulk_insert(&state.conn, &key, &buffer)?;
    refresh_data_ranges(&state.conn, &key, "ib_stream")?;
}
state.last_flush = Instant::now();
```

This deferred batching is a v2 feature. For v1, `bulk_insert` is called
immediately with the full buffer from CSV import or IB historical request.

---

## 9. Time Bucket Aggregation

### 9.1 General Resampling Query

DuckDB's `FIRST()` and `LAST()` aggregate functions with `ORDER BY` clause
enable correct OHLCV resampling:

```sql
SELECT
    symbol,
    :target_tf_secs AS timeframe_secs,
    (timestamp_ms / :bucket_ms) * :bucket_ms AS timestamp_ms,
    FIRST(open ORDER BY timestamp_ms)   AS open,
    MAX(high)                           AS high,
    MIN(low)                            AS low,
    LAST(close ORDER BY timestamp_ms)   AS close,
    SUM(volume)::UINTEGER               AS volume
FROM market.candles
WHERE symbol = :symbol AND timeframe_secs = :source_tf_secs
GROUP BY symbol, (timestamp_ms / :bucket_ms) * :bucket_ms
ORDER BY timestamp_ms;
```

Where:
- `:bucket_ms = :target_tf_secs * 1000` (target period in milliseconds)
- `:source_tf_secs` is the source timeframe (e.g., 60 for 1min)
- `:target_tf_secs` is the target timeframe (e.g., 300 for 5min)

**Floor arithmetic:** `(timestamp_ms / bucket_ms) * bucket_ms` rounds each
timestamp down to its bucket boundary using integer division. This works
because `timestamp_ms` and `bucket_ms` are both BIGINT.

### 9.2 Specific Aggregation Examples

#### 1min to 5min

```sql
-- bucket_ms = 300 * 1000 = 300000
SELECT
    symbol,
    300 AS timeframe_secs,
    (timestamp_ms / 300000) * 300000 AS timestamp_ms,
    FIRST(open ORDER BY timestamp_ms) AS open,
    MAX(high) AS high,
    MIN(low) AS low,
    LAST(close ORDER BY timestamp_ms) AS close,
    SUM(volume)::UINTEGER AS volume
FROM market.candles
WHERE symbol = 'AAPL' AND timeframe_secs = 60
GROUP BY symbol, (timestamp_ms / 300000) * 300000
ORDER BY timestamp_ms;
```

#### 1min to 15min

```sql
-- bucket_ms = 900 * 1000 = 900000
SELECT
    symbol,
    900 AS timeframe_secs,
    (timestamp_ms / 900000) * 900000 AS timestamp_ms,
    FIRST(open ORDER BY timestamp_ms) AS open,
    MAX(high) AS high,
    MIN(low) AS low,
    LAST(close ORDER BY timestamp_ms) AS close,
    SUM(volume)::UINTEGER AS volume
FROM market.candles
WHERE symbol = 'AAPL' AND timeframe_secs = 60
GROUP BY symbol, (timestamp_ms / 900000) * 900000
ORDER BY timestamp_ms;
```

#### 1min to 1 hour

```sql
-- bucket_ms = 3600 * 1000 = 3600000
SELECT
    symbol,
    3600 AS timeframe_secs,
    (timestamp_ms / 3600000) * 3600000 AS timestamp_ms,
    FIRST(open ORDER BY timestamp_ms) AS open,
    MAX(high) AS high,
    MIN(low) AS low,
    LAST(close ORDER BY timestamp_ms) AS close,
    SUM(volume)::UINTEGER AS volume
FROM market.candles
WHERE symbol = 'AAPL' AND timeframe_secs = 60
GROUP BY symbol, (timestamp_ms / 3600000) * 3600000
ORDER BY timestamp_ms;
```

#### 1min to Daily

```sql
-- bucket_ms = 86400 * 1000 = 86400000
-- NOTE: For daily aggregation from intraday, floor arithmetic works for
-- a single timezone. If the data spans market hours in different timezones,
-- use a trading-session-aware bucket instead (requires ICU extension).
SELECT
    symbol,
    86400 AS timeframe_secs,
    (timestamp_ms / 86400000) * 86400000 AS timestamp_ms,
    FIRST(open ORDER BY timestamp_ms) AS open,
    MAX(high) AS high,
    MIN(low) AS low,
    LAST(close ORDER BY timestamp_ms) AS close,
    SUM(volume)::UINTEGER AS volume
FROM market.candles
WHERE symbol = 'AAPL' AND timeframe_secs = 60
GROUP BY symbol, (timestamp_ms / 86400000) * 86400000
ORDER BY timestamp_ms;
```

### 9.3 Rust Function for On-Demand Aggregation

```rust
/// Aggregate candles from a source timeframe to a target timeframe.
///
/// Uses DuckDB's FIRST()/LAST() ORDER BY for correct OHLCV resampling.
/// The result is materialized into a CandleBuffer ready for chart rendering.
///
/// # Arguments
///
/// * `conn` - DuckDB connection.
/// * `symbol` - Symbol to aggregate.
/// * `source_tf` - Source timeframe (must contain data in market.candles).
/// * `target_tf` - Target timeframe (must be a multiple of source_tf).
///
/// # Errors
///
/// Returns `StoreError::QueryFailed` if the query fails.
/// Returns `StoreError::InvalidTimeframe` if target is not a multiple of source.
pub fn aggregate_candles(
    conn: &Connection,
    symbol: &str,
    source_tf: Timeframe,
    target_tf: Timeframe,
) -> Result<CandleBuffer, StoreError> {
    let source_secs = source_tf.as_secs() as u32;
    let target_secs = target_tf.as_secs() as u32;

    if target_secs % source_secs != 0 || target_secs <= source_secs {
        return Err(StoreError::InvalidTimeframe(target_secs));
    }

    let bucket_ms = (target_secs as i64) * 1000;

    // This query is not cached because the bucket_ms varies.
    // An alternative is to use a parameterized bucket, but DuckDB does not
    // support parameterized expressions in GROUP BY. The SQL is constructed
    // with literal values, which is safe because bucket_ms is derived from
    // a u32 (no injection risk).
    let sql = format!(
        "SELECT
            ({bucket_ms_val} * (timestamp_ms / {bucket_ms_val})) AS ts,
            FIRST(open ORDER BY timestamp_ms) AS open,
            MAX(high) AS high,
            MIN(low) AS low,
            LAST(close ORDER BY timestamp_ms) AS close,
            SUM(volume)::UINTEGER AS volume
         FROM market.candles
         WHERE symbol = ? AND timeframe_secs = ?
         GROUP BY ({bucket_ms_val} * (timestamp_ms / {bucket_ms_val}))
         ORDER BY ts",
        bucket_ms_val = bucket_ms,
    );

    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| StoreError::QueryFailed(format!(
            "aggregate prepare failed: {e}"
        )))?;

    let rows = stmt
        .query_map(params![symbol, source_secs], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, f32>(1)?,
                row.get::<_, f32>(2)?,
                row.get::<_, f32>(3)?,
                row.get::<_, f32>(4)?,
                row.get::<_, u32>(5)?,
            ))
        })
        .map_err(|e| StoreError::QueryFailed(format!(
            "aggregate query failed: {e}"
        )))?;

    let mut buffer = CandleBuffer::with_capacity(5000);

    for row_result in rows {
        let (ts, o, h, l, c, v) = row_result
            .map_err(|e| StoreError::QueryFailed(format!(
                "aggregate row read failed: {e}"
            )))?;
        buffer.push(ts, o, h, l, c, v);
    }

    Ok(buffer)
}
```

### 9.4 Insert Aggregated Results Back into Cache

After aggregating, the result can be persisted to avoid re-computation:

```rust
// Aggregate 1min -> 5min and store the result.
let buf_5m = aggregate_candles(&conn, "AAPL", Timeframe::M1, Timeframe::M5)?;
upsert_candles(&conn, &DataKey { symbol: "AAPL".into(), timeframe: Timeframe::M5 }, &buf_5m)?;
refresh_data_ranges(&conn, &DataKey { symbol: "AAPL".into(), timeframe: Timeframe::M5 }, "aggregated")?;
```

---

## 10. Data Integrity

### 10.1 Compound PK Duplicate Prevention

The `PRIMARY KEY (symbol, timeframe_secs, timestamp_ms)` on `market.candles`
prevents two candles for the same symbol, timeframe, and timestamp from
coexisting. DuckDB enforces this constraint on every INSERT.

**Behavior with different INSERT variants:**

| Statement | Duplicate behavior | Use case |
|---|---|---|
| `INSERT INTO` | Error on duplicate PK | Default. Use for known-clean data. |
| `INSERT OR IGNORE INTO` | Silently skip duplicate rows | Append-only: skip candles already cached. |
| `INSERT OR REPLACE INTO` | Delete old row, insert new one | Correction: overwrite with updated data. |

The Appender API does not support `OR IGNORE` or `OR REPLACE` modifiers.
If incoming data may contain duplicates, use one of these strategies:

1. **Pre-filter in Rust.** Before calling `bulk_insert()`, query the existing
   time range and remove overlapping timestamps from the buffer.
2. **Use `upsert_candles()`.** The DELETE + INSERT approach handles overlaps.
3. **Staging table.** Insert into a temp table, then `INSERT OR IGNORE INTO
   market.candles SELECT * FROM staging`.

For v1, strategy 2 (`upsert_candles`) is the recommended approach for
overlapping data. Strategy 1 is suitable for append-only scenarios where
new data is known to be after existing data.

### 10.2 Transaction Boundaries

**Migrations:** Each migration runs in its own `BEGIN ... COMMIT` transaction.
If the migration SQL fails, the transaction is rolled back and the
`schema_version` entry is not created.

**Bulk insert + metadata update:** These should be atomic:

```rust
/// Insert candles and update metadata atomically.
///
/// If the insert succeeds but metadata update fails, the database is left
/// with candles but stale metadata. To prevent this, wrap both in a
/// transaction.
pub fn insert_candles_with_metadata(
    conn: &Connection,
    key: &DataKey,
    buffer: &CandleBuffer,
    source: &str,
) -> Result<usize, StoreError> {
    if buffer.is_empty() {
        return Ok(0);
    }

    // Note: DuckDB Appender cannot be used inside a transaction (it manages
    // its own transaction internally). So we insert first, then update
    // metadata. If metadata update fails, the candles are still there and
    // metadata will be corrected on next refresh_data_ranges() call.
    let count = bulk_insert(conn, key, buffer)?;
    refresh_data_ranges(conn, key, source)?;

    Ok(count)
}
```

**Important DuckDB Appender constraint:** The Appender manages its own
internal transaction. Calling `conn.execute("BEGIN")` before creating an
Appender may cause conflicts. The `bulk_insert` + `refresh_data_ranges`
sequence is not atomic, but this is acceptable because:
- The only consequence of a crash between insert and metadata update is
  stale `data_ranges` metadata.
- `refresh_data_ranges` recomputes from the source table, self-healing
  any inconsistency.
- A startup routine can call `refresh_data_ranges` for all known keys
  to reconcile any stale metadata.

### Startup Reconciliation

If the app crashes between the Appender flush and the `refresh_data_ranges()`
call, `meta.data_ranges` will have stale or missing entries. To self-heal,
the actor runs a reconciliation query during connection initialization
(after migration, before serving commands):

```sql
INSERT OR REPLACE INTO meta.data_ranges
    (symbol, timeframe_secs, candle_count, first_ts, last_ts, source)
SELECT symbol, timeframe_secs, COUNT(*), MIN(timestamp_ms),
       MAX(timestamp_ms), 'reconciled'
FROM market.candles
GROUP BY symbol, timeframe_secs;
```

This runs once per app startup. For typical datasets (< 500 symbol/timeframe
pairs), it completes in < 10ms. The `'reconciled'` source tag distinguishes
these entries from normal inserts for debugging purposes.

### 10.3 Crash Safety

DuckDB uses a WAL (Write-Ahead Log) for crash safety:
- Committed data survives process crashes.
- The WAL is replayed automatically on next open.
- Uncommitted data (in-flight Appender rows that were not flushed) is lost.
- The `.duckdb.wal` file is automatically cleaned up by checkpoint.

For this application:
- CSV imports: fully committed before returning to the caller. No data loss.
- IB streaming (future): the batching buffer in the actor thread holds unflushed
  candles. On crash, up to `flush_interval_secs` of streaming data is lost.
  This is acceptable because IB historical data can be re-requested.

### 10.4 Concurrent Access

The actor model serializes all database access through a single thread:
- No concurrent writers. No write-write conflicts.
- No concurrent readers. No read-write conflicts.
- No need for `WAL` mode configuration (DuckDB's default is already WAL-based).
- `Appender` `!Send` requirement is naturally satisfied.

If a future read pool is added (parallel `Connection::try_clone()` readers),
DuckDB's MVCC ensures readers see a consistent snapshot. Writes from the
actor thread are visible to readers after commit.

---

## Appendix A: Complete SQL Reference

Quick-reference of all SQL used by midas-store, organized by operation:

```
SCHEMA CREATION:
  CREATE SCHEMA IF NOT EXISTS market
  CREATE SCHEMA IF NOT EXISTS meta
  CREATE SCHEMA IF NOT EXISTS cache

TABLE CREATION (migration v1):
  CREATE TABLE market.candles (...)
  CREATE TABLE meta.data_ranges (...)
  CREATE TABLE meta.symbols (...)
  CREATE TABLE IF NOT EXISTS schema_version (...)

QUERIES (prepare_cached):
  SELECT timestamp_ms, open, high, low, close, volume FROM market.candles
    WHERE symbol = ? AND timeframe_secs = ? ORDER BY timestamp_ms ASC
  SELECT timestamp_ms, open, high, low, close, volume FROM market.candles
    WHERE symbol = ? AND timeframe_secs = ? AND timestamp_ms >= ? AND timestamp_ms <= ?
    ORDER BY timestamp_ms ASC
  SELECT symbol, timeframe_secs, candle_count, first_ts, last_ts, source
    FROM meta.data_ranges ORDER BY symbol ASC, timeframe_secs ASC

MUTATIONS:
  INSERT OR REPLACE INTO meta.data_ranges (...) VALUES (?, ?, ?, ?, ?, ?, CURRENT_TIMESTAMP)
  DELETE FROM market.candles WHERE symbol = ? AND timeframe_secs = ?
    AND timestamp_ms >= ? AND timestamp_ms <= ?
  DELETE FROM market.candles WHERE symbol = ?
  DELETE FROM meta.data_ranges WHERE symbol = ?
  DELETE FROM meta.data_ranges WHERE symbol = ? AND timeframe_secs = ?
  DELETE FROM meta.symbols WHERE symbol = ?

AGGREGATION:
  SELECT COUNT(*)::INTEGER, COALESCE(MIN(timestamp_ms), 0), COALESCE(MAX(timestamp_ms), 0)
    FROM market.candles WHERE symbol = ? AND timeframe_secs = ?

MAINTENANCE:
  CHECKPOINT

BULK INSERT (Appender API, not SQL):
  conn.appender("market", "candles") -> append_row(params![...]) -> flush()

MIGRATIONS:
  SELECT COALESCE(MAX(version), 0) FROM schema_version
  INSERT INTO schema_version (version, description) VALUES (?, ?)
```
