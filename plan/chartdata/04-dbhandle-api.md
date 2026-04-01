# 04 -- DbHandle Public API

> Complete API specification for `midas-store::DbHandle` -- the async handle
> to the DuckDB actor thread. Every public type, method, error variant, and
> usage pattern needed for implementation.
>
> Revision 0 -- 2026-03-31

---

## Table of Contents

1. [Public API Surface](#1-public-api-surface)
2. [StoreError Enum](#2-storeerror-enum)
3. [DbHandle Methods](#3-dbhandle-methods)
4. [Clone Semantics](#4-clone-semantics)
5. [Timeout Wrapper](#5-timeout-wrapper)
6. [Type Conversion Helpers](#6-type-conversion-helpers)
7. [Thread Safety Guarantees](#7-thread-safety-guarantees)
8. [API Evolution](#8-api-evolution)

---

## 1. Public API Surface

### 1.1 Module Structure

> **Note:** The canonical module layout is in [01-crate-architecture.md](01-crate-architecture.md).
> This document uses a simplified view for API exposition.

```
midas-store/
  src/
    lib.rs           // Re-exports public API
    error.rs         // StoreError enum
    types.rs         // DataKey, CacheInfo, StoreConfig (canonical: 06-config-and-startup.md)
    schema.rs        // MIGRATION_V1_SQL, run_migrations()
    queries.rs       // bulk_insert, query_candles, etc.
    convert.rs       // CandleBuffer <-> DuckDB conversions
    actor.rs         // DbCommand, DbReply, handler
    handle.rs        // DbHandle wrapping MailboxProcessor
```

### 1.2 Public Exports (lib.rs)

```rust
//! midas-store: DuckDB-backed persistent chart data cache.
//!
//! Provides [`DbHandle`], an async-safe handle to a DuckDB database running
//! on a dedicated actor thread. All database operations are serialized through
//! a mailbox channel, keeping blocking C++ FFI calls off the tokio runtime.
//!
//! # Usage
//!
//! ```rust
//! use midas_store::{DbHandle, DataKey, StoreConfig};
//! use midas_core::Timeframe;
//! use midas_data::CandleBuffer;
//!
//! # async fn example() -> Result<(), midas_store::StoreError> {
//! let config = StoreConfig::default();
//! let db = DbHandle::open(config).await?;
//!
//! // Insert candles
//! let key = DataKey::new("AAPL", Timeframe::D1);
//! let mut buf = CandleBuffer::with_capacity(1);
//! buf.push(1_700_000_000_000, 150.0, 155.0, 148.0, 153.0, 50_000);
//! db.insert_candles(key.clone(), buf).await?;
//!
//! // Query candles
//! let data = db.query_candles(key).await?;
//! assert_eq!(data.len(), 1);
//!
//! db.shutdown().await?;
//! # Ok(())
//! # }
//! ```

mod config;
mod error;
mod handle;
mod queries;
mod schema;

pub use config::StoreConfig;
pub use error::StoreError;
pub use handle::{DbHandle, CacheInfo, DataKey};
```

### 1.3 DataKey

```rust
use midas_core::Timeframe;

/// Identifies a specific candle series in the cache.
///
/// A DataKey uniquely identifies a set of candles by their symbol ticker
/// and bar timeframe. This is the compound key used for all cache operations.
///
/// # Examples
///
/// ```rust
/// use midas_store::DataKey;
/// use midas_core::Timeframe;
///
/// let key = DataKey::new("AAPL", Timeframe::D1);
/// assert_eq!(key.symbol, "AAPL");
/// assert_eq!(key.timeframe_secs(), 86400);
///
/// // Clone is cheap (clones one String and copies one enum).
/// let key2 = key.clone();
/// assert_eq!(key, key2);
/// ```
#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub struct DataKey {
    /// Symbol ticker string. Matches `ContractSpec::symbol()` output.
    /// Examples: "AAPL", "MSFT", "EUR.USD", "ES"
    pub symbol: String,

    /// Bar timeframe. Stored as the enum variant, not as seconds.
    /// Convert to seconds for SQL via `timeframe_secs()`.
    pub timeframe: Timeframe,
}

impl DataKey {
    /// Create a new DataKey.
    ///
    /// # Arguments
    ///
    /// * `symbol` - The ticker symbol string.
    /// * `timeframe` - The candle timeframe.
    pub fn new(symbol: impl Into<String>, timeframe: Timeframe) -> Self {
        Self {
            symbol: symbol.into(),
            timeframe,
        }
    }

    /// Returns the timeframe duration in seconds as u32.
    ///
    /// Maps directly to `Timeframe::as_secs()` which returns `u32`.
    /// All Timeframe variants fit within u32 (max is MN1 = 2,592,000).
    /// Used as the SQL parameter for the `timeframe_secs` column.
    #[inline]
    pub fn timeframe_secs(&self) -> u32 {
        self.timeframe.as_secs() as u32
    }
}

impl std::fmt::Display for DataKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.symbol, self.timeframe)
    }
}
```

### 1.4 CacheInfo

```rust
/// Metadata about a cached candle series.
///
/// Returned by [`DbHandle::list_cached()`]. One entry per (symbol, timeframe)
/// pair that has data stored in the cache. This metadata comes from the
/// `meta.data_ranges` table, not from scanning `market.candles`.
///
/// # Examples
///
/// ```rust
/// use midas_store::CacheInfo;
///
/// // List what's in the cache (typically at startup or for UI display):
/// let entries: Vec<CacheInfo> = db.list_cached().await?;
/// for entry in &entries {
///     println!("{}: {} candles, {} to {}",
///         entry.key, entry.candle_count, entry.first_ts, entry.last_ts);
/// }
/// ```
#[derive(Clone, Debug)]
pub struct CacheInfo {
    /// The symbol and timeframe this entry describes.
    pub key: DataKey,

    /// Total number of candles stored for this key.
    pub candle_count: usize,

    /// Timestamp (epoch ms) of the earliest candle in the cache.
    pub first_ts: i64,

    /// Timestamp (epoch ms) of the latest candle in the cache.
    pub last_ts: i64,

    /// Data source identifier. Values: "csv", "ib_historical", "ib_stream",
    /// "test", "aggregated".
    pub source: String,
}

impl CacheInfo {
    /// Returns true if the cached data fully covers the requested time range.
    ///
    /// A range [start_ms, end_ms] is covered if the cache spans from at or
    /// before `start_ms` to at or after `end_ms`.
    pub fn covers_range(&self, start_ms: i64, end_ms: i64) -> bool {
        self.first_ts <= start_ms && self.last_ts >= end_ms
    }
}
```

### 1.5 StoreConfig

```rust
use std::path::PathBuf;

/// Configuration for the DuckDB data store.
///
/// Loaded from the `[store]` section of `config.toml`. All fields have
/// sensible defaults for desktop usage.
///
/// # Examples
///
/// ```rust
/// use midas_store::StoreConfig;
///
/// // Default config: enabled, file-based, 256MB memory, 2 threads.
/// let config = StoreConfig::default();
///
/// // Disabled (graceful fallback to TestDataProvider):
/// let config = StoreConfig { enabled: false, ..Default::default() };
///
/// // In-memory for tests:
/// let config = StoreConfig::memory();
/// ```
#[derive(Clone, Debug)]
pub struct StoreConfig {
    /// Whether the store is enabled. When false, `DbHandle::open()` returns
    /// `Ok` with a no-op handle, or the caller skips the open entirely.
    pub enabled: bool,

    /// Path to the DuckDB database file. Relative paths are resolved
    /// against the application data directory.
    ///
    /// Set to `None` for in-memory mode (tests, benchmarks).
    pub path: Option<PathBuf>,

    /// Maximum memory DuckDB may use for query processing, in megabytes.
    ///
    /// DuckDB defaults to 80% of system RAM, which is far too aggressive
    /// for a desktop app sharing resources with wgpu, iced, and the OS.
    /// This cap is applied via `SET memory_limit = '<N>MB'` on connection open.
    ///
    /// Recommended: 64-256 MB.
    pub memory_limit_mb: u32,

    /// Number of DuckDB worker threads for query parallelism.
    ///
    /// Set low (1-2) to avoid starving the GPU rendering and UI threads.
    /// Applied via `SET threads = <N>` on connection open.
    pub threads: u8,

    /// Interval in seconds between flush cycles for streaming data batching.
    ///
    /// In v1, this is unused (all inserts are immediate). In v2 with IB
    /// streaming, the actor thread accumulates incoming candles and flushes
    /// to DuckDB on this interval.
    pub flush_interval_secs: u32,
}

impl Default for StoreConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            path: Some(PathBuf::from("cache.duckdb")),
            memory_limit_mb: 256,
            threads: 2,
            flush_interval_secs: 5,
        }
    }
}

impl StoreConfig {
    /// Configuration for an in-memory database. Used in tests.
    pub fn memory() -> Self {
        Self {
            enabled: true,
            path: None,
            memory_limit_mb: 64,
            threads: 1,
            flush_interval_secs: 5,
        }
    }
}
```

---

## 2. StoreError Enum

```rust
/// Errors produced by `midas-store` operations.
///
/// All variants carry a human-readable message describing the failure.
/// The message includes DuckDB error details when available, but does NOT
/// include SQL statements (which could contain user data in interpolated
/// queries -- although we use parameterized queries exclusively).
///
/// # Error Handling Strategy
///
/// - Library consumers (midas-app) match on the variant to decide behavior:
///   - `ConnectionFailed` / `MigrationFailed`: fatal at startup, disable store.
///   - `QueryFailed` / `InsertFailed`: log and fall back to alternative data source.
///   - `ActorDead`: the actor thread has panicked or been dropped. Restart or disable.
///   - `Timeout`: retry or fall back.
///   - `InvalidTimeframe`: programming error, log and skip.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// DuckDB connection could not be opened.
    ///
    /// Causes: file permissions, corrupt database, incompatible DuckDB version,
    /// disk full.
    ///
    /// Recovery: disable store, fall back to TestDataProvider.
    #[error("connection failed: {0}")]
    ConnectionFailed(String),

    /// Schema migration failed during startup.
    ///
    /// Causes: SQL syntax error in migration, table already exists with
    /// incompatible schema (manual modification), disk full during DDL.
    ///
    /// Recovery: disable store, log full error for developer diagnosis.
    /// The database file may need manual repair or deletion.
    #[error("migration failed: {0}")]
    MigrationFailed(String),

    /// A query (SELECT) failed to execute.
    ///
    /// Causes: table dropped externally, column type mismatch, out of memory
    /// during query processing.
    ///
    /// Recovery: return empty CandleBuffer, fall back to data source.
    #[error("query failed: {0}")]
    QueryFailed(String),

    /// A bulk insert (Appender) or mutation (DELETE/UPDATE) failed.
    ///
    /// Causes: PK violation (duplicate candles), disk full, Appender creation
    /// failure (table not found).
    ///
    /// Recovery: log the error. Data remains in memory (CandleBuffer) and
    /// can be retried. The chart is not affected (write-behind pattern).
    #[error("insert failed: {0}")]
    InsertFailed(String),

    /// The mailbox channel to the actor thread is closed.
    ///
    /// Causes: actor thread panicked, DbHandle was shut down, all senders
    /// were dropped.
    ///
    /// Recovery: create a new DbHandle (re-open). Or disable store for the
    /// remainder of the session.
    #[error("actor dead: {0}")]
    ActorDead(String),

    /// A query exceeded its deadline.
    ///
    /// Causes: DuckDB processing a large aggregation, disk I/O stall,
    /// actor thread blocked on a long-running previous operation.
    ///
    /// Recovery: return empty result, let the caller retry or fall back.
    #[error("query timed out")]
    Timeout,

    /// A `timeframe_secs` value does not map to any known Timeframe variant.
    ///
    /// Causes: database contains data from a future version of the app with
    /// new timeframe variants, or manual data insertion with incorrect values.
    ///
    /// Recovery: skip the entry (forward compatibility).
    #[error("invalid timeframe: {0} seconds does not map to a known Timeframe variant")]
    InvalidTimeframe(u32),
}
```

### 2.1 Error Conversion Impls

```rust
/// Convert MailboxProcessorError to StoreError::ActorDead.
///
/// MailboxProcessorError is produced when the channel to the actor thread
/// is closed (actor panicked or was dropped).
impl From<mailbox_processor::MailboxProcessorError> for StoreError {
    fn from(e: mailbox_processor::MailboxProcessorError) -> Self {
        StoreError::ActorDead(e.to_string())
    }
}

/// Convert tokio::time::error::Elapsed to StoreError::Timeout.
impl From<tokio::time::error::Elapsed> for StoreError {
    fn from(_: tokio::time::error::Elapsed) -> Self {
        StoreError::Timeout
    }
}
```

---

## 3. DbHandle Methods

### 3.1 Internal Types (not public)

```rust
use midas_core::Timeframe;
use midas_data::CandleBuffer;
use mailbox_processor::{MailboxProcessor, BufferSize};
use tokio::sync::mpsc::Sender;

/// Commands sent to the DuckDB actor thread.
///
/// Each variant corresponds to one public DbHandle method. The actor
/// handler matches on these to dispatch to the appropriate query function.
///
/// Not public: callers use DbHandle's async methods instead.
enum DbCommand {
    /// Insert candles into market.candles via Appender.
    /// Sent by: `DbHandle::insert_candles()`
    InsertCandles {
        key: DataKey,
        buffer: CandleBuffer,
        source: String,
    },

    /// Query all candles for a key (full range scan).
    /// Sent by: `DbHandle::query_candles()`
    QueryCandles {
        key: DataKey,
    },

    /// Query candles within a time range.
    /// Sent by: `DbHandle::query_candles_range()`
    QueryCandlesRange {
        key: DataKey,
        start_ms: i64,
        end_ms: i64,
    },

    /// List all cached data series.
    /// Sent by: `DbHandle::list_cached()`
    ListCached,

    /// Graceful shutdown: checkpoint and close.
    /// Sent by: `DbHandle::shutdown()`
    Shutdown,
}

/// Replies from the DuckDB actor thread.
///
/// Each variant wraps a `Result` because database operations can fail.
/// The DbHandle methods unwrap these into the appropriate return type.
enum DbReply {
    /// Response to InsertCandles.
    Inserted(Result<usize, StoreError>),

    /// Response to QueryCandles and QueryCandlesRange.
    Candles(Result<CandleBuffer, StoreError>),

    /// Response to ListCached.
    CacheList(Result<Vec<CacheInfo>, StoreError>),

    /// Response to Shutdown.
    ShutdownComplete(Result<(), StoreError>),

    /// Connection or migration failure. Sent when the actor thread
    /// cannot open the database, regardless of which command triggered
    /// the open attempt. All DbHandle methods must check for this variant.
    Error(StoreError),
}
```

### 3.2 DbHandle Struct

```rust
/// Async-safe handle to the DuckDB data store.
///
/// `DbHandle` wraps a `MailboxProcessor` that owns a DuckDB `Connection`
/// on a dedicated OS thread. All database operations are serialized through
/// the mailbox channel, ensuring:
///
/// - Blocking C++ FFI calls never run on tokio worker threads.
/// - The `Connection` (which is `!Sync`) is never shared across threads.
/// - The `Appender` (which is `!Send`) is created and used on the same thread.
///
/// # Clone
///
/// `DbHandle` is cheaply cloneable. Each clone shares the same underlying
/// mailbox channel. Multiple chart panels, the data manager, and the app
/// shell can each hold their own clone. See [Clone Semantics](#4-clone-semantics).
///
/// # Shutdown
///
/// Call [`shutdown()`](DbHandle::shutdown) for a graceful shutdown that
/// checkpoints the database. If the `DbHandle` is simply dropped, the
/// actor thread exits when all senders are dropped, but no checkpoint
/// is performed.
pub struct DbHandle {
    /// The mailbox processor wrapping the DuckDB connection.
    /// MailboxProcessor<DbCommand, DbReply> internally holds a
    /// tokio::sync::mpsc::Sender, which is Send + Sync.
    mb: MailboxProcessor<DbCommand, DbReply>,
}
```

### 3.3 DbHandle::open()

```rust
impl DbHandle {
    /// Open a persistent DuckDB database and run migrations.
    ///
    /// Creates the database file if it does not exist. Applies any pending
    /// schema migrations (idempotent -- safe to call on every startup).
    /// Spawns a dedicated OS thread named `"duckdb-store"` for all database
    /// operations.
    ///
    /// # Arguments
    ///
    /// * `config` - Store configuration (path, memory limit, threads).
    ///
    /// # Errors
    ///
    /// Returns `StoreError::ConnectionFailed` if DuckDB cannot open the file.
    /// Returns `StoreError::MigrationFailed` if schema migrations fail.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use midas_store::{DbHandle, StoreConfig};
    /// use std::path::PathBuf;
    ///
    /// # async fn example() -> Result<(), midas_store::StoreError> {
    /// let config = StoreConfig {
    ///     path: Some(PathBuf::from("data/cache.duckdb")),
    ///     memory_limit_mb: 128,
    ///     threads: 2,
    ///     ..Default::default()
    /// };
    /// let db = DbHandle::open(config).await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Implementation Detail
    ///
    /// - Sends: (no command -- connection is opened lazily on first message,
    ///   OR eagerly in the thread spawn closure)
    /// - Receives: (no reply -- open is synchronous in the constructor)
    ///
    /// The connection is opened eagerly in the `new_blocking` closure.
    /// If opening fails, the actor thread panics, and subsequent `send()`
    /// calls return `StoreError::ActorDead`. To surface connection errors
    /// at `open()` time, the constructor sends a no-op "ping" command and
    /// awaits the reply.
    /// Open a persistent DuckDB database.
    ///
    /// This is a **synchronous** function. It only creates the mpsc channel
    /// and spawns the actor thread — no database I/O happens here. The
    /// connection is opened lazily on the first command sent to the actor.
    ///
    /// To surface connection/migration errors at startup, send a health-check
    /// command (e.g., `list_cached()`) via an iced `Task::perform()` after
    /// construction. See [05-data-flow.md](05-data-flow.md) for the startup
    /// sequence.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use midas_store::{DbHandle, StoreConfig};
    ///
    /// let config = StoreConfig::default();
    /// let db = DbHandle::open(config);
    ///
    /// // In iced, surface errors via a startup task:
    /// // Task::perform(async move { db.list_cached().await }, Message::StoreReady)
    /// ```
    pub fn open(config: StoreConfig) -> Self {
        let path = config.path.clone();
        let memory_limit_mb = config.memory_limit_mb;
        let threads = config.threads;

        let mb = MailboxProcessor::new_blocking(
            BufferSize::Size(256),
            None::<duckdb::Connection>, // State: Option<Connection>
            "duckdb-store",
            move |cmd, conn_state, reply_channel| {
                // Lazily open connection on first command.
                let conn = match conn_state {
                    Some(c) => c,
                    None => {
                        let c = match &path {
                            Some(p) => duckdb::Connection::open(p),
                            None => duckdb::Connection::open_in_memory(),
                        };

                        let c = match c {
                            Ok(c) => c,
                            Err(e) => {
                                // Cannot open DB. Reply with Error variant
                                // regardless of which command triggered the open.
                                if let Some(ch) = reply_channel {
                                    let _ = ch.blocking_send(
                                        DbReply::Error(StoreError::ConnectionFailed(
                                            e.to_string(),
                                        )),
                                    );
                                }
                                return None;
                            }
                        };

                        // Apply memory and thread limits.
                        let _ = c.execute_batch(&format!(
                            "SET memory_limit = '{}MB'; \
                             SET threads = {}; \
                             SET enable_progress_bar = false; \
                             SET enable_object_cache = true;",
                            memory_limit_mb, threads,
                        ));

                        // Run migrations.
                        if let Err(e) = crate::schema::run_migrations(&c) {
                            if let Some(ch) = reply_channel {
                                let _ = ch.blocking_send(DbReply::Error(e));
                            }
                            return None;
                        }

                        // Reconcile data_ranges metadata (self-healing after crash).
                        let _ = c.execute_batch(
                            "INSERT OR REPLACE INTO meta.data_ranges
                                 (symbol, timeframe_secs, candle_count, first_ts, last_ts, source)
                             SELECT symbol, timeframe_secs, COUNT(*), MIN(timestamp_ms),
                                    MAX(timestamp_ms), 'reconciled'
                             FROM market.candles
                             GROUP BY symbol, timeframe_secs"
                        );

                        c
                    }
                };

                // Dispatch command.
                match cmd {
                    DbCommand::InsertCandles { key, buffer, source } => {
                        let result = crate::queries::insert_candles_with_metadata(
                            &conn, &key, &buffer, &source,
                        );
                        if let Some(ch) = reply_channel {
                            let _ = ch.blocking_send(DbReply::Inserted(result));
                        }
                    }

                    DbCommand::QueryCandles { key } => {
                        let result = crate::queries::query_candles(&conn, &key);
                        if let Some(ch) = reply_channel {
                            let _ = ch.blocking_send(DbReply::Candles(result));
                        }
                    }

                    DbCommand::QueryCandlesRange { key, start_ms, end_ms } => {
                        let result = crate::queries::query_candles_range(
                            &conn, &key, start_ms, end_ms,
                        );
                        if let Some(ch) = reply_channel {
                            let _ = ch.blocking_send(DbReply::Candles(result));
                        }
                    }

                    DbCommand::ListCached => {
                        let result = crate::queries::list_cached(&conn);
                        if let Some(ch) = reply_channel {
                            let _ = ch.blocking_send(DbReply::CacheList(result));
                        }
                    }

                    DbCommand::Shutdown => {
                        // Checkpoint (not vacuum — vacuum can be slow).
                        let _ = conn.execute_batch("CHECKPOINT");
                        if let Some(ch) = reply_channel {
                            let _ = ch.blocking_send(
                                DbReply::ShutdownComplete(Ok(())),
                            );
                        }
                    }
                }

                Some(conn) // Return connection for next iteration.
            },
        );

        DbHandle { mb }
    }
```

### 3.4 DbHandle::open_memory()

```rust
    /// Open an in-memory DuckDB database for testing.
    ///
    /// The database is lost when the DbHandle is dropped. Migrations are
    /// applied. Memory limit is set to 64MB.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use midas_store::DbHandle;
    ///
    /// # async fn example() -> Result<(), midas_store::StoreError> {
    /// let db = DbHandle::open_memory().await?;
    /// // ... run tests ...
    /// db.shutdown().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn open_memory() -> Self {
        Self::open(StoreConfig::memory())
    }
```

### 3.5 DbHandle::insert_candles()

```rust
    /// Insert candles into the persistent cache.
    ///
    /// Uses DuckDB's Appender API for bulk insert, then updates the
    /// `meta.data_ranges` metadata table. The operation is atomic with
    /// respect to the metadata update (if the insert succeeds but metadata
    /// update fails, `refresh_data_ranges` will self-heal on next call).
    ///
    /// # Arguments
    ///
    /// * `key` - Symbol and timeframe identifying the series.
    /// * `buffer` - Candle data to insert. Ownership is transferred to the
    ///   actor thread to avoid copying.
    ///
    /// # Returns
    ///
    /// The number of rows inserted. Returns 0 for an empty buffer (no-op).
    ///
    /// # Errors
    ///
    /// - `StoreError::InsertFailed` - Appender creation failed, PK violation
    ///   (duplicate candles), or disk full.
    /// - `StoreError::ActorDead` - Actor thread has exited.
    ///
    /// # Duplicate Handling
    ///
    /// This method does NOT handle overlapping data. If the buffer contains
    /// timestamps that already exist in the cache, the insert will fail with
    /// a PK violation. Use `upsert_candles()` (future API) for overlapping
    /// data, or ensure the caller deduplicates before calling.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use midas_store::{DbHandle, DataKey};
    /// use midas_core::Timeframe;
    /// use midas_data::CandleBuffer;
    ///
    /// # async fn example(db: &DbHandle) -> Result<(), midas_store::StoreError> {
    /// let key = DataKey::new("AAPL", Timeframe::D1);
    /// let mut buf = CandleBuffer::with_capacity(2);
    /// buf.push(1_700_000_000_000, 150.0, 155.0, 148.0, 153.0, 50_000);
    /// buf.push(1_700_086_400_000, 153.0, 158.0, 151.0, 156.0, 45_000);
    ///
    /// let inserted = db.insert_candles(key, buf).await?;
    /// assert_eq!(inserted, 2);
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Implementation
    ///
    /// - Sends: `DbCommand::InsertCandles { key, buffer, source: "csv" }`
    /// - Expects: `DbReply::Inserted(Result<usize, StoreError>)`
    pub async fn insert_candles(
        &self,
        key: DataKey,
        buffer: CandleBuffer,
    ) -> Result<usize, StoreError> {
        self.insert_candles_with_source(key, buffer, "csv").await
    }

    /// Insert candles with a specified data source tag.
    ///
    /// Like `insert_candles()`, but allows specifying the source identifier
    /// stored in `meta.data_ranges`. Source values: "csv", "ib_historical",
    /// "ib_stream", "test", "aggregated".
    pub async fn insert_candles_with_source(
        &self,
        key: DataKey,
        buffer: CandleBuffer,
        source: &str,
    ) -> Result<usize, StoreError> {
        let source = source.to_owned();

        match self
            .mb
            .send(DbCommand::InsertCandles { key, buffer, source })
            .await?
        {
            DbReply::Inserted(result) => result,
            DbReply::Error(e) => Err(e),
            other => unreachable!(
                "insert_candles: unexpected reply variant: {:?}",
                std::mem::discriminant(&other)
            ),
        }
    }
```

### 3.6 DbHandle::query_candles()

```rust
    /// Query all cached candles for a symbol and timeframe.
    ///
    /// Returns candles ordered by ascending timestamp, materialized into a
    /// `CandleBuffer` ready for chart rendering. Returns an empty buffer
    /// (not an error) if no data exists for this key.
    ///
    /// # Arguments
    ///
    /// * `key` - Symbol and timeframe to query.
    ///
    /// # Returns
    ///
    /// A `CandleBuffer` with all cached candles for this key. The buffer's
    /// `timestamps` are monotonically increasing (guaranteed by the database
    /// ORDER BY and CandleBuffer's push invariant).
    ///
    /// # Errors
    ///
    /// - `StoreError::QueryFailed` - SQL execution error.
    /// - `StoreError::ActorDead` - Actor thread has exited.
    ///
    /// # Performance
    ///
    /// Typical latency: ~5ms for 5,000 daily candles (20 years of data).
    /// This is called once per chart load, not per frame.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use midas_store::{DbHandle, DataKey};
    /// use midas_core::Timeframe;
    ///
    /// # async fn example(db: &DbHandle) -> Result<(), midas_store::StoreError> {
    /// let key = DataKey::new("AAPL", Timeframe::D1);
    /// let candles = db.query_candles(key).await?;
    ///
    /// if candles.is_empty() {
    ///     // Cache miss: load from alternative source (CSV, IB API).
    /// } else {
    ///     // Cache hit: use directly for chart rendering.
    ///     let arc_buf = std::sync::Arc::new(candles);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Implementation
    ///
    /// - Sends: `DbCommand::QueryCandles { key }`
    /// - Expects: `DbReply::Candles(Result<CandleBuffer, StoreError>)`
    pub async fn query_candles(
        &self,
        key: DataKey,
    ) -> Result<CandleBuffer, StoreError> {
        match self.mb.send(DbCommand::QueryCandles { key }).await? {
            DbReply::Candles(result) => result,
            DbReply::Error(e) => Err(e),
            other => unreachable!(
                "query_candles: unexpected reply variant: {:?}",
                std::mem::discriminant(&other)
            ),
        }
    }
```

### 3.7 DbHandle::query_candles_range()

```rust
    /// Query cached candles within a specific time range.
    ///
    /// Returns candles where `timestamp_ms >= start_ms AND timestamp_ms <= end_ms`,
    /// ordered by ascending timestamp. Used for:
    ///
    /// - Loading only the visible portion of a chart (lazy loading).
    /// - Fetching a specific gap range to merge with existing in-memory data.
    /// - Checking if a time range is already cached before requesting from IB.
    ///
    /// # Arguments
    ///
    /// * `key` - Symbol and timeframe to query.
    /// * `start_ms` - Start of the time range, inclusive (epoch milliseconds).
    /// * `end_ms` - End of the time range, inclusive (epoch milliseconds).
    ///
    /// # Returns
    ///
    /// A `CandleBuffer` with candles in the requested range. Empty buffer if
    /// no data exists in the range.
    ///
    /// # Errors
    ///
    /// - `StoreError::QueryFailed` - SQL execution error.
    /// - `StoreError::ActorDead` - Actor thread has exited.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use midas_store::{DbHandle, DataKey};
    /// use midas_core::Timeframe;
    ///
    /// # async fn example(db: &DbHandle) -> Result<(), midas_store::StoreError> {
    /// let key = DataKey::new("AAPL", Timeframe::D1);
    ///
    /// // Load last 30 days only:
    /// let now_ms = chrono::Utc::now().timestamp_millis();
    /// let thirty_days_ms = 30 * 86_400 * 1_000;
    /// let candles = db.query_candles_range(
    ///     key,
    ///     now_ms - thirty_days_ms,
    ///     now_ms,
    /// ).await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Implementation
    ///
    /// - Sends: `DbCommand::QueryCandlesRange { key, start_ms, end_ms }`
    /// - Expects: `DbReply::Candles(Result<CandleBuffer, StoreError>)`
    pub async fn query_candles_range(
        &self,
        key: DataKey,
        start_ms: i64,
        end_ms: i64,
    ) -> Result<CandleBuffer, StoreError> {
        match self
            .mb
            .send(DbCommand::QueryCandlesRange {
                key,
                start_ms,
                end_ms,
            })
            .await?
        {
            DbReply::Candles(result) => result,
            DbReply::Error(e) => Err(e),
            other => unreachable!(
                "query_candles_range: unexpected reply variant: {:?}",
                std::mem::discriminant(&other)
            ),
        }
    }
```

### 3.8 DbHandle::list_cached()

```rust
    /// List all cached data series with their metadata.
    ///
    /// Reads from `meta.data_ranges` (small table, fast scan). Used at
    /// startup to determine which symbols have cached data, and for UI
    /// display of cache contents.
    ///
    /// # Returns
    ///
    /// A vector of `CacheInfo` entries, one per (symbol, timeframe) pair.
    /// Entries with unknown `timeframe_secs` values (from a future app
    /// version) are silently skipped.
    ///
    /// # Errors
    ///
    /// - `StoreError::QueryFailed` - SQL execution error.
    /// - `StoreError::ActorDead` - Actor thread has exited.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use midas_store::DbHandle;
    ///
    /// # async fn example(db: &DbHandle) -> Result<(), midas_store::StoreError> {
    /// let cached = db.list_cached().await?;
    /// for entry in &cached {
    ///     println!("{} {} bars: {} to {}",
    ///         entry.key.symbol,
    ///         entry.candle_count,
    ///         entry.first_ts,
    ///         entry.last_ts,
    ///     );
    /// }
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Implementation
    ///
    /// - Sends: `DbCommand::ListCached`
    /// - Expects: `DbReply::CacheList(Result<Vec<CacheInfo>, StoreError>)`
    pub async fn list_cached(&self) -> Result<Vec<CacheInfo>, StoreError> {
        match self.mb.send(DbCommand::ListCached).await? {
            DbReply::CacheList(result) => result,
            DbReply::Error(e) => Err(e),
            other => unreachable!(
                "list_cached: unexpected reply variant: {:?}",
                std::mem::discriminant(&other)
            ),
        }
    }
```

### 3.9 DbHandle::shutdown()

```rust
    /// Graceful shutdown: checkpoint the database and signal the actor to stop.
    ///
    /// Sends a `CHECKPOINT` command to flush the WAL to the main database
    /// file, then returns. The actor thread continues to process any queued
    /// commands and exits when all `DbHandle` clones are dropped.
    ///
    /// If shutdown is not called, the actor thread exits when all senders
    /// are dropped, but no explicit checkpoint occurs. DuckDB's WAL will
    /// be replayed on next open, so no data is lost -- but the `.wal` file
    /// remains on disk until the next checkpoint.
    ///
    /// # Errors
    ///
    /// - `StoreError::QueryFailed` - Checkpoint failed (unlikely).
    /// - `StoreError::ActorDead` - Actor thread already exited.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use midas_store::DbHandle;
    ///
    /// # async fn example(db: DbHandle) -> Result<(), midas_store::StoreError> {
    /// // Before app exit:
    /// db.shutdown().await?;
    /// // db can still be used after shutdown (checkpoint is just a flush).
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Implementation
    ///
    /// - Sends: `DbCommand::Shutdown`
    /// - Expects: `DbReply::ShutdownComplete(Result<(), StoreError>)`
    pub async fn shutdown(&self) -> Result<(), StoreError> {
        match self.mb.send(DbCommand::Shutdown).await? {
            DbReply::ShutdownComplete(result) => result,
            DbReply::Error(e) => Err(e),
            other => unreachable!(
                "shutdown: unexpected reply variant: {:?}",
                std::mem::discriminant(&other)
            ),
        }
    }
}
```

---

## 4. Clone Semantics

### 4.1 How Clone Works

`DbHandle` contains a single field: `mb: MailboxProcessor<DbCommand, DbReply>`.
The `MailboxProcessor` internally holds a `tokio::sync::mpsc::Sender`. Cloning
the `Sender` increments an `Arc` reference count -- no heap allocation, no
channel duplication.

```rust
impl Clone for DbHandle {
    fn clone(&self) -> Self {
        DbHandle {
            mb: self.mb.clone(), // Clones the internal Sender (Arc increment)
        }
    }
}
```

If `MailboxProcessor` does not derive `Clone`, implement it manually by
exposing the `Sender`:

```rust
// Alternative if MailboxProcessor does not impl Clone:
// Store the Sender directly instead of the MailboxProcessor.
pub struct DbHandle {
    sender: tokio::sync::mpsc::Sender<(DbCommand, Option<tokio::sync::mpsc::Sender<DbReply>>)>,
}

impl Clone for DbHandle {
    fn clone(&self) -> Self {
        DbHandle {
            sender: self.sender.clone(),
        }
    }
}
```

### 4.2 Usage Pattern

Multiple chart panels hold independent clones of the same `DbHandle`.
Each clone sends commands through the same channel to the single actor thread.

```rust
// In MidasApp::new():
let db = DbHandle::open(config).await?;

// Each chart panel gets its own clone:
for panel in &mut self.chart_panels {
    panel.db = db.clone();
}

// The app shell keeps the original:
self.store = Some(db);
```

### 4.3 Drop Behavior

When the last clone of `DbHandle` is dropped, all `Sender` halves of the
channel are dropped. The actor thread's `Receiver::recv()` returns `None`,
exiting the message loop. The `Connection` is dropped on the actor thread,
closing the database file.

No explicit shutdown is required for correctness. However, calling
`shutdown()` before the final drop ensures a clean `CHECKPOINT` is performed.

---

## 5. Timeout Wrapper

### 5.1 The Pattern

All `DbHandle` methods are async but do not have built-in timeouts. The
caller should wrap calls with `tokio::time::timeout` when latency guarantees
are needed (e.g., during startup where a hung database should not block the
UI indefinitely).

```rust
use std::time::Duration;
use tokio::time::timeout;

/// Query candles with a timeout.
///
/// Returns `StoreError::Timeout` if the query does not complete within
/// the specified duration.
pub async fn query_with_timeout(
    db: &DbHandle,
    key: DataKey,
    deadline: Duration,
) -> Result<CandleBuffer, StoreError> {
    timeout(deadline, db.query_candles(key))
        .await
        .map_err(|_| StoreError::Timeout)?
}
```

### 5.2 Recommended Timeouts

| Operation | Recommended timeout | Rationale |
|---|---|---|
| `query_candles` (cold start) | 5 seconds | First query may trigger lazy connection open + migration |
| `query_candles` (warm) | 2 seconds | Connection is open, query should be <50ms |
| `insert_candles` | 10 seconds | Large bulk inserts (50K+ rows) may take time |
| `list_cached` | 2 seconds | Small table scan |
| `shutdown` | 5 seconds | Checkpoint on large database |

### 5.3 Usage in MidasApp

```rust
// In the startup sequence:
let db = DbHandle::open(config).await?;

// Load each chart with a timeout:
for (id, panel) in &self.charts {
    let key = DataKey::new(&panel.symbol, panel.timeframe);
    let db_clone = db.clone();

    Task::perform(
        async move {
            timeout(Duration::from_secs(5), db_clone.query_candles(key.clone()))
                .await
                .unwrap_or_else(|_| {
                    tracing::warn!(%key, "query timed out, falling back");
                    Ok(CandleBuffer::new())
                })
        },
        move |result| match result {
            Ok(buf) if !buf.is_empty() => Message::DataLoaded(id, Ok(Arc::new(buf))),
            _ => Message::DataCacheMiss(id),
        },
    )
}
```

### 5.4 Why Not Built-In Timeouts

Timeouts are the caller's concern, not the library's:

1. **Different callers need different timeouts.** Startup loads tolerate higher
   latency than interactive chart switches.
2. **`tokio::time::timeout` composes cleanly.** No need to plumb timeout
   configuration through every method signature.
3. **The From<Elapsed> impl on StoreError** makes the pattern ergonomic
   with `?`.

---

## 6. Type Conversion Helpers

### 6.1 DataKey::timeframe_secs()

Already defined in the DataKey struct (section 1.3). Maps `Timeframe` enum
to the `INTEGER` value stored in the `timeframe_secs` column:

```rust
impl DataKey {
    #[inline]
    pub fn timeframe_secs(&self) -> u32 {
        self.timeframe.as_secs() as u32
    }
}
```

### 6.2 timeframe_from_secs()

Converts the `INTEGER` column value back to a `Timeframe` enum. Used when
reading from `meta.data_ranges` and `market.candles`.

This is a private helper in `midas-store`. If/when `Timeframe::from_secs()`
is added to `midas-core`, this helper should be removed in favor of the
canonical implementation.

```rust
/// Convert a duration in seconds to a Timeframe enum variant.
///
/// Returns `None` for values that do not map to any known variant.
/// Used for forward compatibility: if the database contains data from a
/// future version of the app with new timeframe variants, those entries
/// are skipped rather than causing an error.
fn timeframe_from_secs(secs: u32) -> Option<Timeframe> {
    use midas_core::Timeframe;
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

### 6.3 Adding Timeframe::from_secs() to midas-core (Recommended)

When convenient, add this to `midas-core::Timeframe`:

```rust
impl Timeframe {
    /// Convert a duration in seconds to a Timeframe variant.
    ///
    /// Returns `None` if the value does not match any variant.
    pub fn from_secs(secs: u64) -> Option<Self> {
        match secs {
            1       => Some(Self::S1),
            5       => Some(Self::S5),
            15      => Some(Self::S15),
            30      => Some(Self::S30),
            60      => Some(Self::M1),
            300     => Some(Self::M5),
            900     => Some(Self::M15),
            1800    => Some(Self::M30),
            3600    => Some(Self::H1),
            14400   => Some(Self::H4),
            86400   => Some(Self::D1),
            604800  => Some(Self::W1),
            2592000 => Some(Self::MN1),
            _       => None,
        }
    }
}
```

Then in `midas-store`, replace `timeframe_from_secs(secs)` with
`Timeframe::from_secs(secs as u64)`.

---

## 7. Thread Safety Guarantees

### 7.1 DbHandle is Send + Sync

`DbHandle` wraps `MailboxProcessor<DbCommand, DbReply>`, which internally
holds a `tokio::sync::mpsc::Sender<(DbCommand, Option<Sender<DbReply>>)>`.

`Sender<T>` is `Send + Sync` when `T: Send`. Therefore `DbHandle` is
`Send + Sync` if `DbCommand` and `DbReply` are `Send`.

```rust
// DbCommand is Send because all fields are Send:
//   - DataKey: String + Timeframe (Copy) -> Send
//   - CandleBuffer: Vec<i64> + Vec<f32> + Vec<u32> -> Send
//   - String -> Send
// DbReply is Send because all fields are Send:
//   - CandleBuffer -> Send
//   - Vec<CacheInfo> -> Send
//   - StoreError -> Send (all variants contain String or u32)
//   - usize -> Send

// Compile-time assertions:
const _: () = {
    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}
    fn check() {
        assert_send::<DbHandle>();
        assert_sync::<DbHandle>();
    }
};
```

### 7.2 What This Enables

Because `DbHandle` is `Send + Sync`:

1. **Clone and move to any thread.** iced's `Task::perform` moves closures
   to tokio's thread pool. The closure can capture a `DbHandle` clone.

2. **Store in `Arc`.** Although cloning is already cheap, `Arc<DbHandle>`
   works if needed for shared ownership patterns.

3. **Store as a field in `MidasApp`.** The iced application struct must be
   `Send` (for multi-threaded runtime). `DbHandle` satisfies this.

4. **Pass to async tasks.** Any `tokio::spawn` task can hold a `DbHandle`.

### 7.3 What Remains on the Actor Thread

These types are NOT `Send` or NOT `Sync`, and live exclusively on the
`"duckdb-store"` thread:

| Type | Send | Sync | Why |
|---|---|---|---|
| `duckdb::Connection` | Send | **!Sync** | Internal `RefCell` |
| `duckdb::Appender` | **!Send** | **!Sync** | Tied to connection's write state |
| `duckdb::CachedStatement` | **!Send** | **!Sync** | Borrows from connection |

The mailbox actor model ensures these types never leave the actor thread.
The `Connection` is created on the actor thread and never moved. The
`Appender` and `CachedStatement` are created on-demand within the handler
closure, used, and dropped before the handler returns.

---

## 8. API Evolution

### 8.1 Planned v2 Methods

These methods will be added when their corresponding features are implemented.
The `DbCommand` and `DbReply` enums will be extended with new variants.
The `DbHandle` API is additive-only; no existing methods will change signature.

#### upsert_candles

```rust
/// Insert candles, replacing any existing rows with the same PK.
///
/// Used when re-importing data that overlaps with existing cached data.
/// Internally performs DELETE of the overlapping range, then bulk INSERT.
pub async fn upsert_candles(
    &self,
    key: DataKey,
    buffer: CandleBuffer,
) -> Result<usize, StoreError>;
```

When: needed for IB historical data re-downloads where the last N bars
overlap with existing cache.

#### aggregate_candles

```rust
/// Aggregate candles from a source timeframe to a target timeframe.
///
/// Uses DuckDB SQL with FIRST()/LAST() ORDER BY for OHLCV resampling.
/// Does NOT store the result; returns a CandleBuffer for the caller to
/// decide whether to display or persist.
pub async fn aggregate_candles(
    &self,
    symbol: String,
    source_tf: Timeframe,
    target_tf: Timeframe,
) -> Result<CandleBuffer, StoreError>;
```

When: needed when the user switches timeframes and only lower-resolution
data is available (e.g., has 1min data, wants 5min view).

#### delete_symbol

```rust
/// Delete all cached data for a symbol across all timeframes.
///
/// Removes candles, metadata, and catalog entry.
pub async fn delete_symbol(
    &self,
    symbol: String,
) -> Result<usize, StoreError>;
```

When: needed for UI "clear cache for symbol" action or data cleanup.

#### compute_indicator (v3)

```rust
/// Compute an indicator using DuckDB window functions.
///
/// Returns a vector of (timestamp_ms, value) pairs.
/// Supported indicators: SMA, EMA (approximated), ATR (SMA-based).
pub async fn compute_indicator(
    &self,
    key: DataKey,
    indicator: &str,  // e.g., "SMA_200", "ATR_14"
) -> Result<Vec<(i64, f64)>, StoreError>;
```

When: useful for cross-symbol screening ("show all stocks where SMA50 > SMA200").
Per-chart indicators will remain in the Rust indicator engine for performance.

#### attach_sqlite (v3)

```rust
/// Attach the broker's SQLite database for cross-domain queries.
///
/// Enables queries like "show P&L against current price" by joining
/// DuckDB candle data with SQLite position data.
pub async fn attach_sqlite(
    &self,
    path: std::path::PathBuf,
) -> Result<(), StoreError>;
```

When: needed when the broker crate is integrated and cross-domain queries
are desirable (e.g., overlay position entry price on chart).

#### vacuum (v2)

```rust
/// Explicitly checkpoint and compact the database.
///
/// Currently called internally by `shutdown()`. Expose publicly when
/// a UI action or scheduled task needs to trigger compaction.
pub async fn vacuum(&self) -> Result<(), StoreError>;
```

When: needed for a "compact database" menu item or periodic maintenance.

### 8.2 Extension Strategy

Adding a new operation to `DbHandle` follows this checklist:

1. **Add a variant to `DbCommand`** with the necessary parameters.
2. **Add a variant to `DbReply`** wrapping the result type.
3. **Implement the query function** in `queries.rs` (pure sync, takes `&Connection`).
4. **Add a match arm** in the actor handler that dispatches to the query function.
5. **Add an async method** to `DbHandle` that sends the command and unwraps the reply.
6. **Add a unit test** using `DbHandle::open_memory()`.

No changes to the `MailboxProcessor`, the actor thread lifecycle, or existing
methods. The pattern is strictly additive.

### 8.3 Breaking Changes Policy

This is an internal crate (not published to crates.io). Breaking changes
to the `DbHandle` API are acceptable when justified, but should be avoided
by:

- Adding methods rather than modifying existing ones.
- Using builder patterns for complex parameter sets (avoid methods with
  more than 4 parameters).
- Keeping `DataKey`, `CacheInfo`, and `StoreConfig` as simple structs
  with public fields (no getters/setters).

---

## Appendix A: Complete Type Summary

| Type | Kind | Public | Send | Sync | Clone |
|---|---|---|---|---|---|
| `DbHandle` | struct | yes | yes | yes | yes (cheap) |
| `DataKey` | struct | yes | yes | yes | yes |
| `CacheInfo` | struct | yes | yes | yes | yes |
| `StoreConfig` | struct | yes | yes | yes | yes |
| `StoreError` | enum | yes | yes | yes | no (thiserror) |
| `DbCommand` | enum | no | yes | -- | no |
| `DbReply` | enum | no | yes | -- | no |
| `Migration` | struct | no | -- | -- | no |

## Appendix B: Dependency Graph

```
midas-core          (Timeframe, ContractSpec, CandleData trait)
     |
midas-data          (CandleBuffer, CandleSlice)
     |
midas-store         (DbHandle, DataKey, CacheInfo, StoreConfig, StoreError)
     |               depends on: midas-core, midas-data, mailbox_processor,
     |                           duckdb, tokio, thiserror, tracing
     |
midas-app           (MidasApp holds Option<DbHandle>)
```

## Appendix C: Full Integration Example

```rust
use midas_store::{DbHandle, DataKey, StoreConfig, StoreError};
use midas_core::Timeframe;
use midas_data::CandleBuffer;
use std::sync::Arc;
use std::time::Duration;

async fn startup() -> Result<(), StoreError> {
    // 1. Open database (runs migrations).
    let config = StoreConfig::default();
    let db = DbHandle::open(config).await?;

    // 2. Check what's cached.
    let cached = db.list_cached().await?;
    tracing::info!("{} series cached", cached.len());

    // 3. Load chart data.
    let key = DataKey::new("AAPL", Timeframe::D1);

    let candles = tokio::time::timeout(
        Duration::from_secs(5),
        db.query_candles(key.clone()),
    )
    .await
    .unwrap_or_else(|_| {
        tracing::warn!("query timed out");
        Ok(CandleBuffer::new())
    })?;

    if candles.is_empty() {
        // Cache miss: load from CSV or IB.
        let mut buf = CandleBuffer::with_capacity(5000);
        // ... populate buf from data source ...

        // Write-behind: store in cache for next startup.
        let db_clone = db.clone();
        let key_clone = key.clone();
        tokio::spawn(async move {
            if let Err(e) = db_clone.insert_candles(key_clone, buf).await {
                tracing::error!("cache write failed: {e}");
            }
        });
    } else {
        // Cache hit: use directly.
        let arc_buf = Arc::new(candles);
        // ... pass to chart panel ...
    }

    // 4. Graceful shutdown.
    db.shutdown().await?;

    Ok(())
}
```
