# 01 -- midas-store Crate Architecture

> DuckDB-backed persistent chart data cache for Hand of Midas.
>
> Status: DESIGN SPECIFICATION
> Date: 2026-03-31
>
> **Path convention**: `midas-store` refers to `desktop/win/crates/midas-store/`.
> `mailbox_processor` refers to the external crate at
> `D:\GitHub\ControlPlugin\Shared\mailbox_processor\`.
>
> **Companion document**: `02-actor-concurrency.md` covers the actor model,
> threading, and DuckDB connection lifecycle in detail.

---

## 1. Crate Purpose

### What midas-store IS

midas-store is the **L2 analytical cache layer** -- a DuckDB-backed persistent
store for historical OHLCV candle data. It sits between the in-memory L1 cache
(`CandleBuffer` in `midas-data`) and the upstream data sources (`midas-feed`).

Responsibilities:

- **Persistent caching** -- Write candle data to DuckDB after import from CSV
  or future data providers. On next app launch, data loads from DuckDB instead
  of re-importing.
- **Time-range queries** -- Retrieve candles for a given symbol+timeframe within
  a time window. DuckDB's columnar engine handles range scans efficiently.
- **Cache inventory** -- Answer "what data do we have?" queries: which symbols,
  which timeframes, what time range is cached, how many candles.
- **Append and upsert** -- Efficiently append new candles or upsert overlapping
  ranges (e.g., a re-import with corrected data, or backfill gaps).
- **Maintenance** -- Vacuum, checkpoint WAL, report database size.

### What midas-store is NOT

- **Not a data provider.** It does not fetch data from the network or parse CSV.
  That remains `midas-feed`'s responsibility.
- **Not the in-memory working set.** The charting pipeline reads from
  `CandleBuffer` (L1). midas-store populates L1 on startup or on-demand, but
  the hot rendering path never touches DuckDB.
- **Not a streaming sink.** Real-time tick data goes to `CandleBuffer` directly.
  midas-store receives completed candles for persistence after the fact.
- **Not the annotation store.** Levels, brackets, and widget annotations are
  stored in the per-symbol JSON files managed by `midas-chart`'s
  `AnnotationStore`.

### Data flow position

```
CSV File / Network Provider
        |
        v
  midas-feed  (import/parse)
        |
        | CandleBuffer
        v
  midas-app   (orchestrator)
        |
   +---------+----------+
   |                     |
   v                     v
midas-store            midas-chart
(L2 persist)           (L1 in-memory)
   |                     |
   | on startup:         | every frame:
   | load -> CandleBuffer| read CandleData
   |                     |
   +----------+----------+
              |
              v
         midas-render (GPU)
```

The key principle: **midas-feed does NOT depend on midas-store.** The
write-through happens at the `midas-app` orchestration level. When `midas-app`
imports data through `midas-feed`, it feeds the resulting `CandleBuffer` to both
`midas-chart` (for display) and `midas-store` (for persistence).

---

## 2. Module Layout

```
crates/midas-store/
  Cargo.toml
  src/
    lib.rs              Public re-exports, crate-level doc
    error.rs            StoreError enum (thiserror)
    types.rs            DataKey, CacheInfo, StoreConfig, TimeRange
    schema.rs           SQL DDL strings, migration functions
    queries.rs          Prepared query functions (insert, query, list, upsert)
    convert.rs          CandleBuffer <-> DuckDB row conversions
    actor.rs            DbCommand/DbReply enums, actor handler function
    handle.rs           DbHandle public API wrapping mailbox_processor
```

### 2.1 `lib.rs` -- Public re-exports

The crate root re-exports the public API surface. All internal modules are
`pub(crate)` except the types that consumers need.

```rust
//! midas-store: DuckDB-backed persistent cache for historical candle data.
//!
//! Depends on: midas-core, midas-data
//!
//! # Architecture
//!
//! All DuckDB operations run on a dedicated OS thread via a mailbox actor.
//! The public [`DbHandle`] communicates with this thread through async
//! message passing. This keeps blocking C++ FFI calls off the tokio
//! threadpool entirely.
//!
//! # Usage
//!
//! ```no_run
//! use midas_store::{DbHandle, StoreConfig, DataKey};
//! use midas_core::Timeframe;
//!
//! # async fn example() -> Result<(), midas_store::StoreError> {
//! let handle = DbHandle::open(StoreConfig::default()).await?;
//!
//! let key = DataKey::new("AAPL", Timeframe::D1);
//! let candles = handle.query(&key, None).await?;
//! # Ok(())
//! # }
//! ```

mod actor;
mod convert;
mod error;
mod handle;
mod queries;
mod schema;
mod types;

pub use error::StoreError;
pub use handle::DbHandle;
pub use types::{CacheInfo, DataKey, StoreConfig, TimeRange};
```

### 2.2 `error.rs` -- StoreError enum

Follows the project convention of `thiserror` in library crates.

```rust
//! Error types for the midas-store crate.

use std::path::PathBuf;

/// Errors produced by midas-store operations.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// DuckDB returned an error (query, connection, etc.).
    #[error("DuckDB error: {0}")]
    DuckDb(#[from] duckdb::Error),

    /// The database file could not be opened or created.
    #[error("failed to open database at {path}: {source}")]
    OpenFailed {
        path: PathBuf,
        source: duckdb::Error,
    },

    /// Schema migration failed.
    #[error("migration failed (version {version}): {message}")]
    MigrationFailed {
        version: u32,
        message: String,
    },

    /// A query returned no results when results were expected.
    #[error("no data found for {symbol} {timeframe}")]
    NotFound {
        symbol: String,
        timeframe: String,
    },

    /// The actor thread has exited or the channel is closed.
    #[error("store actor is shut down")]
    ActorShutdown,

    /// A reply was expected but the actor did not send one.
    #[error("no reply from store actor (possible bug: did you mean fire_and_forget?)")]
    NoReply,

    /// Timestamp range is invalid (start >= end).
    #[error("invalid time range: start ({start}) >= end ({end})")]
    InvalidTimeRange {
        start: i64,
        end: i64,
    },

    /// Data conversion error (e.g., row count mismatch).
    #[error("conversion error: {0}")]
    Conversion(String),

    /// I/O error (e.g., temp directory creation).
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}
```

### 2.3 `types.rs` -- DataKey, CacheInfo, StoreConfig, TimeRange

Pure data types with no DuckDB dependency. These are the public vocabulary types
that consumers interact with.

```rust
//! Public vocabulary types for midas-store.

use midas_core::Timeframe;
use std::path::PathBuf;

/// Composite key identifying a cached dataset: symbol + timeframe.
///
/// The symbol is stored as an owned, uppercase-normalized `String`.
/// This is the primary lookup key for all store operations.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DataKey {
    /// Uppercase symbol identifier (e.g., "AAPL", "MSFT").
    symbol: String,
    /// Candle timeframe.
    timeframe: Timeframe,
}

impl DataKey {
    /// Create a new `DataKey`, normalizing the symbol to uppercase.
    pub fn new(symbol: &str, timeframe: Timeframe) -> Self {
        Self {
            symbol: symbol.to_uppercase(),
            timeframe,
        }
    }

    /// The uppercase symbol string.
    pub fn symbol(&self) -> &str {
        &self.symbol
    }

    /// The timeframe.
    pub fn timeframe(&self) -> Timeframe {
        self.timeframe
    }
}

impl std::fmt::Display for DataKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.symbol, self.timeframe)
    }
}

/// Time range for filtered queries. Both bounds are epoch milliseconds,
/// inclusive on start, exclusive on end: `[start, end)`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TimeRange {
    /// Start timestamp (inclusive), epoch milliseconds.
    pub start: i64,
    /// End timestamp (exclusive), epoch milliseconds.
    pub end: i64,
}

impl TimeRange {
    /// Create a new time range. Returns `None` if `start >= end`.
    pub fn new(start: i64, end: i64) -> Option<Self> {
        if start < end {
            Some(Self { start, end })
        } else {
            None
        }
    }
}

/// Metadata about a cached dataset (symbol + timeframe).
#[derive(Clone, Debug)]
pub struct CacheInfo {
    /// The data key this info describes.
    pub key: DataKey,
    /// Number of candles stored.
    pub count: u64,
    /// Earliest timestamp in the dataset (epoch ms).
    pub first_ts: i64,
    /// Latest timestamp in the dataset (epoch ms).
    pub last_ts: i64,
    /// Total disk bytes used by this dataset (approximate).
    pub size_bytes: u64,
}

/// Configuration for opening a DuckDB store.
#[derive(Clone, Debug)]
pub struct StoreConfig {
    /// Path to the DuckDB database file.
    /// Defaults to `data/store/candles.duckdb` relative to the app data dir.
    pub db_path: PathBuf,

    /// Maximum memory DuckDB may use (bytes).
    /// Default: 256 MB. DuckDB is a secondary store, not the primary
    /// working set, so a conservative limit is appropriate.
    pub memory_limit_bytes: u64,

    /// Number of DuckDB internal threads.
    /// Default: 2. The store is not CPU-bound; most queries are simple
    /// range scans that complete in microseconds.
    pub threads: u32,

    /// Temporary directory for DuckDB spill files.
    /// Default: system temp dir + "midas-store-tmp".
    pub temp_directory: Option<PathBuf>,

    /// Mailbox channel capacity (bounded backpressure).
    /// Default: 256.
    pub channel_capacity: usize,
}

impl Default for StoreConfig {
    fn default() -> Self {
        Self {
            db_path: PathBuf::from("data/store/candles.duckdb"),
            memory_limit_bytes: 256 * 1024 * 1024, // 256 MB
            threads: 2,
            temp_directory: None,
            channel_capacity: 256,
        }
    }
}
```

### 2.4 `schema.rs` -- SQL DDL and migrations

Contains all SQL as `const &str` constants. The migration system is a simple
version-number table (`schema_version`) with forward-only migrations.

```rust
//! DuckDB schema definitions and migration logic.
//!
//! # Schema Design
//!
//! A single table `candles` stores all OHLCV data, partitioned by
//! `(symbol, timeframe)` composite key. DuckDB's columnar engine
//! naturally compresses the f32 columns and handles range scans
//! efficiently without explicit partitioning.
//!
//! # Migration Strategy
//!
//! Forward-only, numbered migrations. The `schema_version` table
//! tracks the current version. On open, `migrate()` applies any
//! pending migrations sequentially.

use duckdb::Connection;

/// Current schema version. Increment when adding a new migration.
pub(crate) const CURRENT_VERSION: u32 = 1;

/// DDL for the schema version tracking table.
const CREATE_SCHEMA_VERSION: &str = "
    CREATE TABLE IF NOT EXISTS schema_version (
        version  INTEGER NOT NULL,
        applied  TIMESTAMP DEFAULT current_timestamp
    );
";

// ---------------------------------------------------------------
// CANONICAL SCHEMA: See 03-schema-and-migrations.md for the full DDL.
//
// The v1 migration creates:
//   - market.candles (symbol, timeframe_secs, timestamp_ms, OHLCV)
//   - meta.data_ranges (symbol, timeframe_secs, count, range, source)
//   - meta.symbols (forward-looking, unused in v1)
//   - schema_version (migration tracking, in main schema)
//
// Key differences from any simplified snippets elsewhere:
//   - Uses `timeframe_secs INTEGER` (not VARCHAR)
//   - Uses DuckDB schema namespaces (market, meta, cache)
//   - Compound PK: (symbol, timeframe_secs, timestamp_ms)
// ---------------------------------------------------------------

/// Migration v1 SQL. See 03-schema-and-migrations.md Section 3 for
/// the complete, annotated version with design rationale.
const MIGRATION_V1: &str = include_str!("../sql/v1.sql");
// In practice, embed inline or use a const &str. The canonical
// SQL is specified in 03-schema-and-migrations.md.

/// Read the current schema version from the database.
/// Returns 0 if the schema_version table does not exist.
pub(crate) fn current_version(conn: &Connection) -> Result<u32, duckdb::Error> {
    // Check if schema_version table exists
    let exists: bool = conn.query_row(
        "SELECT count(*) > 0 FROM information_schema.tables
         WHERE table_name = 'schema_version'",
        [],
        |row| row.get(0),
    )?;

    if !exists {
        return Ok(0);
    }

    let version: u32 = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_version",
        [],
        |row| row.get(0),
    )?;

    Ok(version)
}

/// Apply all pending migrations up to `CURRENT_VERSION`.
///
/// Each migration runs in a transaction. If any migration fails,
/// that transaction is rolled back and the error is returned.
pub(crate) fn migrate(conn: &Connection) -> Result<(), crate::StoreError> {
    let current = current_version(conn)
        .map_err(|e| crate::StoreError::MigrationFailed {
            version: 0,
            message: format!("failed to read schema version: {e}"),
        })?;

    if current >= CURRENT_VERSION {
        tracing::debug!(
            current_version = current,
            target_version = CURRENT_VERSION,
            "schema is up to date"
        );
        return Ok(());
    }

    tracing::info!(
        from_version = current,
        to_version = CURRENT_VERSION,
        "applying schema migrations"
    );

    for version in (current + 1)..=CURRENT_VERSION {
        apply_migration(conn, version)?;
    }

    Ok(())
}

/// Apply a single migration by version number.
fn apply_migration(conn: &Connection, version: u32) -> Result<(), crate::StoreError> {
    match version {
        1 => {
            conn.execute_batch(CREATE_SCHEMA_VERSION)
                .map_err(|e| migration_err(1, e))?;
            conn.execute_batch(CREATE_CANDLES_V1)
                .map_err(|e| migration_err(1, e))?;
            conn.execute_batch(CREATE_CANDLES_INDEX_V1)
                .map_err(|e| migration_err(1, e))?;
            conn.execute(
                "INSERT INTO schema_version (version) VALUES (?)",
                [version],
            ).map_err(|e| migration_err(1, e))?;
            tracing::info!(version, "migration applied");
        }
        _ => {
            return Err(crate::StoreError::MigrationFailed {
                version,
                message: format!("unknown migration version {version}"),
            });
        }
    }
    Ok(())
}

fn migration_err(version: u32, e: duckdb::Error) -> crate::StoreError {
    crate::StoreError::MigrationFailed {
        version,
        message: e.to_string(),
    }
}
```

### 2.5 `queries.rs` -- Prepared query functions

All SQL lives in this module (alongside `schema.rs` for DDL). Functions accept
a `&Connection` and return typed results. They are called by the actor handler
on the dedicated DB thread.

```rust
//! Query functions for the candles table.
//!
//! All functions accept a `&duckdb::Connection` and operate synchronously.
//! They are called from the actor's dedicated OS thread, never from async code.

use duckdb::{params, Connection};
use midas_core::Timeframe;

use crate::convert;
use crate::types::{CacheInfo, DataKey, TimeRange};
use crate::StoreError;
use midas_data::CandleBuffer;

/// Insert a `CandleBuffer` into `market.candles` using the Appender API
/// for maximum throughput.
///
/// Uses DELETE-before-INSERT to handle overlapping data. The DuckDB
/// Appender does NOT support `INSERT OR IGNORE` or conflict resolution —
/// it will error on primary key violations. The delete-first pattern
/// ensures a clean insert.
///
/// Also updates `meta.data_ranges` with the inserted data's metadata.
///
/// See 03-schema-and-migrations.md Section 8 for the canonical
/// bulk insert specification.
pub(crate) fn insert_candles(
    conn: &Connection,
    key: &DataKey,
    candles: &CandleBuffer,
) -> Result<u64, StoreError> {
    if candles.is_empty() {
        return Ok(0);
    }

    let tf_secs = key.timeframe().as_secs() as i32;

    // Delete existing data for this key. Appender cannot handle PK conflicts.
    conn.execute(
        "DELETE FROM market.candles WHERE symbol = ? AND timeframe_secs = ?",
        params![key.symbol(), tf_secs],
    )?;

    // Bulk insert via Appender (fastest path).
    // Appender is !Send+!Sync — created and dropped within this call.
    {
        let mut appender = conn.appender("market.candles")?;
        for i in 0..candles.len() {
            appender.append_row(params![
                key.symbol(),
                tf_secs,
                candles.timestamps[i],
                candles.opens[i],   // f32 -> FLOAT, no cast needed
                candles.highs[i],
                candles.lows[i],
                candles.closes[i],
                candles.volumes[i], // u32 -> UINTEGER
            ])?;
        }
        appender.flush()?;
    }

    // Update metadata.
    let first_ts = candles.timestamps[0];
    let last_ts = *candles.timestamps.last().unwrap();
    conn.execute(
        "INSERT OR REPLACE INTO meta.data_ranges
         (symbol, timeframe_secs, candle_count, first_ts, last_ts, source)
         VALUES (?, ?, ?, ?, ?, 'rust')",
        params![key.symbol(), tf_secs, candles.len() as i32, first_ts, last_ts],
    )?;

    Ok(candles.len() as u64)
}

/// Query all candles for a given key, optionally filtered by time range.
///
/// Returns candles sorted by timestamp ascending.
pub(crate) fn query_candles(
    conn: &Connection,
    key: &DataKey,
    range: Option<TimeRange>,
) -> Result<CandleBuffer, StoreError> {
    let symbol = key.symbol();
    let tf_secs = key.timeframe().as_secs() as i32;

    let (sql, params_vec): (&str, Vec<Box<dyn duckdb::types::ToSql>>) = match range {
        Some(r) => (
            "SELECT timestamp_ms, open, high, low, close, volume
             FROM market.candles
             WHERE symbol = ? AND timeframe_secs = ? AND timestamp_ms >= ? AND timestamp_ms < ?
             ORDER BY timestamp_ms ASC",
            vec![
                Box::new(symbol.to_string()),
                Box::new(tf_secs),
                Box::new(r.start),
                Box::new(r.end),
            ],
        ),
        None => (
            "SELECT timestamp_ms, open, high, low, close, volume
             FROM market.candles
             WHERE symbol = ? AND timeframe_secs = ?
             ORDER BY timestamp_ms ASC",
            vec![
                Box::new(symbol.to_string()),
                Box::new(tf_secs),
            ],
        ),
    };

    let mut stmt = conn.prepare(sql)?;
    let params_refs: Vec<&dyn duckdb::types::ToSql> =
        params_vec.iter().map(|p| p.as_ref()).collect();

    let rows = stmt.query_map(params_refs.as_slice(), |row| {
        Ok(convert::CandleRow {
            ts: row.get(0)?,
            open: row.get(1)?,
            high: row.get(2)?,
            low: row.get(3)?,
            close: row.get(4)?,
            volume: row.get(5)?,
        })
    })?;

    convert::rows_to_buffer(rows)
}

/// List all cached datasets with metadata from `meta.data_ranges`.
///
/// This is O(1) per entry (reads the metadata table, not the candles table).
/// See 03-schema-and-migrations.md Section 5 for the canonical query.
pub(crate) fn list_cached(conn: &Connection) -> Result<Vec<CacheInfo>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT symbol, timeframe_secs, candle_count, first_ts, last_ts, source
         FROM meta.data_ranges
         ORDER BY symbol, timeframe_secs"
    )?;

    let rows = stmt.query_map([], |row| {
        let symbol: String = row.get(0)?;
        let tf_secs: i32 = row.get(1)?;
        let count: i64 = row.get(2)?;
        let first_ts: i64 = row.get(3)?;
        let last_ts: i64 = row.get(4)?;
        let source: String = row.get(5)?;

        Ok((symbol, tf_secs, count, first_ts, last_ts, source))
    })?;

    let mut results = Vec::new();
    for row in rows {
        let (symbol, tf_secs, count, first_ts, last_ts, _source) = row?;
        let tf_secs_u32 = tf_secs as u32;
        if let Some(tf) = crate::convert::timeframe_from_secs(tf_secs_u32) {
            results.push(CacheInfo {
                key: DataKey::new(&symbol, tf),
                count: count as u64,
                first_ts,
                last_ts,
                size_bytes: 0,
            });
        } else {
            tracing::warn!(
                timeframe_secs = tf_secs,
                "skipping cached dataset with unknown timeframe"
            );
        }
    }

    Ok(results)
}

/// Upsert candles: insert new rows, update existing rows with matching
/// `(symbol, timeframe, ts)` primary key.
///
/// Uses DuckDB's `INSERT OR REPLACE` semantics.
pub(crate) fn upsert_candles(
    conn: &Connection,
    key: &DataKey,
    candles: &CandleBuffer,
) -> Result<u64, StoreError> {
    if candles.is_empty() {
        return Ok(0);
    }

    let symbol = key.symbol();
    let tf_secs = key.timeframe().as_secs() as i32;

    let mut stmt = conn.prepare(
        "INSERT OR REPLACE INTO market.candles
             (symbol, timeframe_secs, timestamp_ms, open, high, low, close, volume)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
    )?;

    for i in 0..candles.len() {
        stmt.execute(params![
            symbol,
            tf_secs,
            candles.timestamps[i],
            candles.opens[i],   // f32 directly, no f64 cast
            candles.highs[i],
            candles.lows[i],
            candles.closes[i],
            candles.volumes[i],
        ])?;
    }

    Ok(candles.len() as u64)
}

/// Delete all candles for a given key within an optional time range.
pub(crate) fn delete_candles(
    conn: &Connection,
    key: &DataKey,
    range: Option<TimeRange>,
) -> Result<u64, StoreError> {
    let symbol = key.symbol();
    let tf_secs = key.timeframe().as_secs() as i32;

    let affected = match range {
        Some(r) => conn.execute(
            "DELETE FROM market.candles
             WHERE symbol = ? AND timeframe_secs = ? AND timestamp_ms >= ? AND timestamp_ms < ?",
            params![symbol, tf_secs, r.start, r.end],
        )?,
        None => conn.execute(
            "DELETE FROM market.candles WHERE symbol = ? AND timeframe_secs = ?",
            params![symbol, tf_secs],
        )?,
    };

    Ok(affected as u64)
}

/// Force a WAL checkpoint.
pub(crate) fn checkpoint(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch("CHECKPOINT")?;
    Ok(())
}

/// Vacuum the database to reclaim space.
pub(crate) fn vacuum(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch("VACUUM")?;
    Ok(())
}
```

### 2.6 `convert.rs` -- CandleBuffer to/from DuckDB rows

Conversion layer between the SoA `CandleBuffer` and DuckDB's row-oriented
query results.

```rust
//! Conversion between CandleBuffer and DuckDB row representations.

use duckdb::Rows;
use midas_data::CandleBuffer;

use crate::StoreError;

/// Intermediate row struct for query result mapping.
pub(crate) struct CandleRow {
    pub ts: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: u32,
}

/// Convert an iterator of `CandleRow` results into a `CandleBuffer`.
///
/// The f64 values from DuckDB FLOAT columns are narrowed to f32.
/// This is safe because we only ever store f32-precision data (prices
/// and volumes are single-precision in the trading domain).
pub(crate) fn rows_to_buffer(
    rows: impl Iterator<Item = Result<CandleRow, duckdb::Error>>,
) -> Result<CandleBuffer, StoreError> {
    let mut buf = CandleBuffer::new();

    for row_result in rows {
        let row = row_result?;
        buf.push(
            row.ts,
            row.open as f32,
            row.high as f32,
            row.low as f32,
            row.close as f32,
            row.volume,
        );
    }

    Ok(buf)
}

/// Convert a `CandleBuffer` into a `Vec<CandleRow>` for row-oriented operations.
///
/// Primarily used for upsert paths where the Appender API is not suitable.
pub(crate) fn buffer_to_rows(buf: &CandleBuffer) -> Vec<CandleRow> {
    (0..buf.len())
        .map(|i| CandleRow {
            ts: buf.timestamps[i],
            open: buf.opens[i] as f64,
            high: buf.highs[i] as f64,
            low: buf.lows[i] as f64,
            close: buf.closes[i] as f64,
            volume: buf.volumes[i],
        })
        .collect()
}
```

### 2.7 `actor.rs` -- DbCommand, DbReply, and handler

The actor module defines the message protocol and the synchronous handler
function that runs on the dedicated DB thread. Full details in
`02-actor-concurrency.md`.

```rust
//! Actor message types and handler for the DuckDB store.
//!
//! See 02-actor-concurrency.md for the full design.

use midas_data::CandleBuffer;
use std::path::PathBuf;

use crate::types::{CacheInfo, DataKey, StoreConfig, TimeRange};
use crate::StoreError;

/// Commands sent from DbHandle to the actor thread.
pub(crate) enum DbCommand {
    /// Insert candles (append, skip duplicates).
    Insert {
        key: DataKey,
        candles: CandleBuffer,
    },
    /// Upsert candles (insert or replace on conflict).
    Upsert {
        key: DataKey,
        candles: CandleBuffer,
    },
    /// Query candles for a key, optionally within a time range.
    Query {
        key: DataKey,
        range: Option<TimeRange>,
    },
    /// List all cached datasets with metadata.
    ListCached,
    /// Delete candles for a key, optionally within a time range.
    Delete {
        key: DataKey,
        range: Option<TimeRange>,
    },
    /// Force a WAL checkpoint.
    Checkpoint,
    /// Vacuum the database.
    Vacuum,
    /// Graceful shutdown: checkpoint and close.
    Shutdown,
}

/// Replies sent from the actor thread back to DbHandle callers.
pub(crate) enum DbReply {
    /// Insert/upsert completed. Payload is rows affected.
    Inserted(Result<u64, StoreError>),
    /// Query result.
    Candles(Result<CandleBuffer, StoreError>),
    /// Cache inventory.
    CacheList(Result<Vec<CacheInfo>, StoreError>),
    /// Delete completed. Payload is rows deleted.
    Deleted(Result<u64, StoreError>),
    /// Maintenance operation completed.
    Done(Result<(), StoreError>),
}
```

### 2.8 `handle.rs` -- DbHandle public API

The public API surface. `DbHandle` wraps the `MailboxProcessor` and provides
ergonomic async methods. It is `Clone + Send + Sync` (cheap sender clone).

```rust
//! Public API handle for the DuckDB store.
//!
//! `DbHandle` is the only public type consumers interact with (besides
//! config and error types). All operations are async and non-blocking
//! from the caller's perspective.

use midas_data::CandleBuffer;

use crate::actor::{DbCommand, DbReply};
use crate::types::{CacheInfo, DataKey, StoreConfig, TimeRange};
use crate::StoreError;

/// Async handle to the DuckDB store actor.
///
/// Cheap to clone (clones the underlying mpsc sender).
/// Dropping all clones causes the actor thread to exit gracefully.
///
/// # Thread Safety
///
/// `DbHandle` is `Send + Sync + Clone`. Multiple chart panes can
/// share a single handle to issue concurrent queries. The actor
/// serializes all database operations on its dedicated thread.
#[derive(Clone)]
pub struct DbHandle {
    // Inner mailbox processor -- details in actor.rs
    inner: mailbox_processor::MailboxProcessor<DbCommand, DbReply>,
}

impl DbHandle {
    /// Open (or create) a DuckDB store at the configured path.
    ///
    /// This spawns a dedicated OS thread for all database operations.
    /// The connection is opened lazily on the first message.
    pub async fn open(config: StoreConfig) -> Result<Self, StoreError> {
        // Implementation in 02-actor-concurrency.md
        todo!()
    }

    /// Insert candles into the store. Duplicate timestamps are skipped.
    ///
    /// Returns the number of rows inserted.
    pub async fn insert(
        &self,
        key: &DataKey,
        candles: &CandleBuffer,
    ) -> Result<u64, StoreError> {
        // Send Insert command, await Inserted reply
        todo!()
    }

    /// Insert candles without waiting for completion.
    ///
    /// Errors are logged via `tracing::warn!` but not returned.
    /// Use for fire-and-forget persistence after data import.
    pub fn insert_fire_and_forget(
        &self,
        key: DataKey,
        candles: CandleBuffer,
    ) {
        // fire_and_forget Insert command
        todo!()
    }

    /// Upsert candles (insert or replace existing).
    ///
    /// Returns the number of rows affected.
    pub async fn upsert(
        &self,
        key: &DataKey,
        candles: &CandleBuffer,
    ) -> Result<u64, StoreError> {
        todo!()
    }

    /// Query all candles for a key.
    ///
    /// Pass `None` for range to get the full dataset.
    /// Pass `Some(TimeRange)` to filter by time window.
    pub async fn query(
        &self,
        key: &DataKey,
        range: Option<TimeRange>,
    ) -> Result<CandleBuffer, StoreError> {
        todo!()
    }

    /// List all cached datasets with metadata.
    pub async fn list_cached(&self) -> Result<Vec<CacheInfo>, StoreError> {
        todo!()
    }

    /// Delete candles for a key, optionally within a time range.
    pub async fn delete(
        &self,
        key: &DataKey,
        range: Option<TimeRange>,
    ) -> Result<u64, StoreError> {
        todo!()
    }

    /// Force a WAL checkpoint.
    pub async fn checkpoint(&self) -> Result<(), StoreError> {
        todo!()
    }

    /// Vacuum the database to reclaim disk space.
    pub async fn vacuum(&self) -> Result<(), StoreError> {
        todo!()
    }

    /// Graceful shutdown: checkpoint WAL, close connection, wait for
    /// the actor thread to exit.
    pub async fn shutdown(&self) -> Result<(), StoreError> {
        todo!()
    }
}
```

---

## 3. Dependency Graph

### Workspace crate DAG after adding midas-store

```
                    midas-core (leaf)
                   /      |       \
                  /       |        \
           midas-data  midas-indicators  ...
              / |
             /  |
    midas-chart  midas-store (NEW)
        |            |
        |     midas-feed (NO dependency on midas-store)
        |          /
        v         v
      midas-render
            |
            v
         midas-ui
            |
            v
        midas-app  ----depends-on----> midas-store
                   ----depends-on----> midas-feed
```

### Critical design constraint: midas-feed does NOT depend on midas-store

The import pipeline works like this:

```
// In midas-app (the orchestrator):

// Step 1: Import CSV through midas-feed
let buffer: CandleBuffer = midas_feed::import_csv(&path)?;

// Step 2: Feed to chart for display (midas-chart)
chart_state.load_candles("AAPL", Timeframe::D1, buffer.clone());

// Step 3: Persist to store (midas-store) -- fire and forget
let key = DataKey::new("AAPL", Timeframe::D1);
db_handle.insert_fire_and_forget(key, buffer);
```

This keeps `midas-feed` pure (parse-only, no side effects) and `midas-store`
pure (persist-only, no parsing). The orchestration responsibility lives in
`midas-app`.

### midas-store direct dependencies

```
midas-store
  ├── midas-core    (Timeframe, CandleData trait)
  ├── midas-data    (CandleBuffer)
  ├── mailbox_processor (actor pattern)
  ├── duckdb        (database)
  ├── thiserror     (error types)
  └── tracing       (logging)
```

---

## 4. Cargo.toml

```toml
[package]
name = "midas-store"
version = "0.1.0"
edition = "2021"
publish = false

[dependencies]
# Workspace crates
midas-core = { path = "../midas-core" }
midas-data = { path = "../midas-data" }

# Actor pattern -- local path dependency
mailbox_processor = { path = "../../../../ControlPlugin/Shared/mailbox_processor" }

# Database
duckdb = { version = "1.1", features = ["bundled"] }

# Error handling
thiserror = { workspace = true }

# Logging
tracing = { workspace = true }

# Async runtime (for channel types used by mailbox_processor)
tokio = { workspace = true }

[dev-dependencies]
tempfile  = { workspace = true }
tokio     = { version = "1", features = ["rt-multi-thread", "macros", "sync", "time", "test-util"] }
```

### Notes on duckdb version and features

- `duckdb = "1.1"` targets the latest stable duckdb-rs crate (v1.1.x maps to
  DuckDB engine v1.1.x). The user specified v1.10501.0 which is the
  `duckdb-sys` version -- the `duckdb` crate version is `1.1.x`.
- `features = ["bundled"]` compiles the DuckDB C++ engine from source. This
  avoids requiring a system DuckDB installation and ensures version consistency.
  The bundled build adds ~2 minutes to a clean compile but is cached thereafter.
- No `"json"`, `"parquet"`, or `"httpfs"` extensions are needed. Raw OHLCV
  storage uses native types only.

### Adding to workspace Cargo.toml

Add to the workspace members list:

```toml
[workspace]
members = [
    "crates/midas-core",
    "crates/midas-data",
    "crates/midas-indicators",
    "crates/midas-chart",
    "crates/midas-render",
    "crates/midas-feed",
    "crates/midas-ui",
    "crates/midas-store",   # <-- NEW
    "crates/midas-app",
]
```

And add to midas-app's dependencies:

```toml
# In crates/midas-app/Cargo.toml
[dependencies]
midas-store  = { path = "../midas-store" }
```

---

## 5. mailbox_processor Integration

### Current state

The `mailbox_processor` crate at `D:\GitHub\ControlPlugin\Shared\mailbox_processor\`
provides:

- `MailboxProcessor::new()` -- spawns a `tokio::task` with an async handler.
- `MailboxProcessor::with_async_handler()` -- identical to `new()`, different name.
- `send()` -- async, returns `ReplyMsg`.
- `fire_and_forget()` -- async, one-way.

Both constructors run the handler as a `tokio::task::spawn()`, which means the
handler runs on the tokio threadpool.

### The problem

DuckDB operations are **blocking C++ FFI calls**. They must not run on the tokio
threadpool because:

1. `duckdb::Connection` is `Send` but `!Sync`. It cannot be shared across tokio
   tasks without a `Mutex`, and holding a `Mutex` across `.await` is unsound.
2. DuckDB's `Appender` is `!Send + !Sync`. It cannot cross thread boundaries at
   all. It must be created and consumed on the same thread.
3. Blocking FFI calls starve the tokio runtime. A 50ms bulk insert blocks an
   entire worker thread, causing latency spikes in the UI event loop.

### Solution: `new_blocking()` constructor

Add a new constructor to `mailbox_processor` that spawns a dedicated
`std::thread` instead of a `tokio::task`. The handler function is synchronous
(`Fn` not `async Fn`), and the receive loop uses `blocking_recv()` instead of
`.await`.

The full implementation is specified in `02-actor-concurrency.md`, section 2.

### Workspace integration for mailbox_processor

The mailbox_processor crate is currently referenced by absolute path. For the
Hand of Midas workspace, it should be added as a path dependency in the
workspace Cargo.toml (but NOT as a workspace member, since it lives outside
the workspace directory):

```toml
# In desktop/win/Cargo.toml [workspace.dependencies] section:
# NOT added here -- mailbox_processor is referenced directly by midas-store's
# Cargo.toml via relative path. It is not a workspace member.
```

The relative path from `crates/midas-store/Cargo.toml` to the mailbox_processor
is `../../../../ControlPlugin/Shared/mailbox_processor`. This is a cross-repo
path dependency, which is fine for a development workspace but would need to
become a git dependency or published crate for CI/distribution.

### Changes required to mailbox_processor

1. Add `new_blocking()` constructor (see `02-actor-concurrency.md` for full code).
2. Add `reply_sync()` helper function (sync equivalent of `reply_if_present()`).
3. Keep existing `new()` and `with_async_handler()` unchanged.
4. Remove the `futures` dependency from the blocking path (it is only needed
   for the async constructors).

Updated `mailbox_processor/Cargo.toml`:

```toml
[package]
name = "mailbox_processor"
version = "0.2.0"
edition = "2021"

[dependencies]
tokio = { version = "1", features = ["sync", "rt"] }
futures = "0.3"  # Only used by async constructors
```

No new dependencies are needed. The blocking constructor uses only `std::thread`
and `tokio::sync::mpsc` (the mpsc receiver's `blocking_recv()` is already
available in tokio 1.x).

---

## 6. Feature Gates

### Optional `duckdb` feature

The midas-store crate compiles with DuckDB by default. An optional
`no-duckdb` configuration is NOT provided via Cargo features because:

1. DuckDB is the entire purpose of this crate. A "midas-store without DuckDB"
   is an empty shell.
2. Feature-gating every function with `#[cfg(feature = "duckdb")]` adds
   complexity for no real benefit.
3. If a build environment cannot compile DuckDB (e.g., CI without C++ toolchain),
   the solution is to exclude `midas-store` from the workspace members list, not
   to compile it without its core dependency.

Instead, the feature gate exists at the **workspace level**: `midas-app` depends
on `midas-store` as an optional dependency:

```toml
# In crates/midas-app/Cargo.toml
[dependencies]
midas-store = { path = "../midas-store", optional = true }

[features]
default = ["persistent-cache"]
persistent-cache = ["dep:midas-store"]
```

This allows building `midas-app` without DuckDB:

```bash
cargo build -p midas-app --no-default-features
```

The app code uses conditional compilation:

```rust
// In midas-app/src/app.rs

#[cfg(feature = "persistent-cache")]
use midas_store::{DbHandle, StoreConfig, DataKey};

pub struct App {
    // ...
    #[cfg(feature = "persistent-cache")]
    db_handle: Option<DbHandle>,
}

impl App {
    async fn on_csv_imported(&mut self, symbol: &str, tf: Timeframe, buffer: CandleBuffer) {
        // Always feed to chart
        self.chart_state.load_candles(symbol, tf, buffer.clone());

        // Optionally persist
        #[cfg(feature = "persistent-cache")]
        if let Some(ref db) = self.db_handle {
            let key = DataKey::new(symbol, tf);
            db.insert_fire_and_forget(key, buffer);
        }
    }
}
```

---

## 7. Re-export Strategy

### Public API surface (`pub`)

These types are usable by consumers of the crate (primarily `midas-app`):

| Type | Module | Re-exported at root |
|------|--------|---------------------|
| `DbHandle` | `handle.rs` | Yes |
| `StoreError` | `error.rs` | Yes |
| `DataKey` | `types.rs` | Yes |
| `TimeRange` | `types.rs` | Yes |
| `CacheInfo` | `types.rs` | Yes |
| `StoreConfig` | `types.rs` | Yes |

### Crate-internal (`pub(crate)`)

These types are used across modules within midas-store but are not exposed
to consumers:

| Type | Module | Visibility |
|------|--------|------------|
| `DbCommand` | `actor.rs` | `pub(crate)` |
| `DbReply` | `actor.rs` | `pub(crate)` |
| `CandleRow` | `convert.rs` | `pub(crate)` |
| `migrate()` | `schema.rs` | `pub(crate)` |
| `current_version()` | `schema.rs` | `pub(crate)` |
| `insert_candles()` | `queries.rs` | `pub(crate)` |
| `query_candles()` | `queries.rs` | `pub(crate)` |
| `list_cached()` | `queries.rs` | `pub(crate)` |
| `upsert_candles()` | `queries.rs` | `pub(crate)` |
| `delete_candles()` | `queries.rs` | `pub(crate)` |
| `checkpoint()` | `queries.rs` | `pub(crate)` |
| `vacuum()` | `queries.rs` | `pub(crate)` |
| `rows_to_buffer()` | `convert.rs` | `pub(crate)` |
| `buffer_to_rows()` | `convert.rs` | `pub(crate)` |
| `CURRENT_VERSION` | `schema.rs` | `pub(crate)` |

### Private (module-level)

Helper functions, SQL string constants, and internal implementation details
are plain `fn` (private to their module).

### Consumer usage pattern

```rust
// In midas-app -- the full public API surface:
use midas_store::{DbHandle, StoreConfig, DataKey, TimeRange, CacheInfo, StoreError};
```

Six types. That is the entire public API. Everything else is an implementation
detail hidden behind the `DbHandle` facade.

---

## Appendix A: File Checklist

Files to create for the `midas-store` crate:

```
desktop/win/crates/midas-store/
  Cargo.toml
  src/
    lib.rs
    error.rs
    types.rs
    schema.rs
    queries.rs
    convert.rs
    actor.rs
    handle.rs
```

Files to modify:

```
desktop/win/Cargo.toml                    # Add midas-store to workspace members
desktop/win/crates/midas-app/Cargo.toml   # Add midas-store dependency
ControlPlugin/Shared/mailbox_processor/src/lib.rs  # Add new_blocking()
```

## Appendix B: Naming Rationale

The crate is named `midas-store` (not `midas-db`, `midas-cache`, or
`midas-duckdb`) because:

- `store` clearly communicates "persistent storage" without implying a
  specific database engine. If DuckDB is ever replaced (e.g., with SQLite
  for smaller deployments), the crate name still fits.
- `db` is too generic and conflicts with common abbreviation usage.
- `cache` implies volatility -- this is persistent storage that survives
  restarts.
- `duckdb` leaks the implementation detail into the public name.
