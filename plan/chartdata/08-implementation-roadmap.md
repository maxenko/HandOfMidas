# 08 — Implementation Roadmap for midas-store

> Part of the [Chart Data Cache plan](00-index.md).
> Prerequisite reading: all prior documents (01-06) and [07-testing-strategy](07-testing-strategy.md)

This document specifies the exact sequence of implementation work, broken
into phases with explicit gate criteria. Each phase lists every file
created or modified, every test added, dependencies on prior phases, risk
factors, and estimated complexity.

---

## Phase Overview

| Phase | Name | Complexity | Gate | Depends On |
|-------|------|-----------|------|------------|
| 0 | Build Spike | S | DuckDB builds, benchmarks pass | None |
| 1 | Crate Skeleton + Schema | M | `cargo test -p midas-store` passes (schema tests) |  Phase 0 |
| 2 | Query Layer | M | All query + convert tests pass | Phase 1 |
| 3 | Actor + Handle | M | Async integration tests pass | Phase 2 |
| 4 | App Integration | L | App starts with store enabled and disabled | Phase 3 |
| 5 | Write-Behind Cache | M | Restart loads from DuckDB cache | Phase 4 |
| 6 | Benchmarks + Optimization | S | Performance targets met | Phase 5 |

**Estimated total effort:** 5-8 working days for a developer familiar with
the codebase. Phase 0 is time-boxed to 4 hours regardless.

---

## Phase 0: Build Spike (Go/No-Go Gate)

**Goal:** Confirm DuckDB compiles and meets performance targets on Windows
MSVC before investing in the full crate.

**Time-box:** 4 hours. If not resolved in 4 hours, escalate to the
DataFusion pivot decision.

### Files Created

| File | Purpose |
|------|---------|
| `desktop/win/duckdb-spike/Cargo.toml` | Throwaway crate manifest |
| `desktop/win/duckdb-spike/src/main.rs` | Spike test code (see 07-testing-strategy.md Section 1.3) |

This crate is **outside the workspace** (not listed in `workspace.members`)
to avoid polluting the workspace dependency graph.

### Execution Steps

1. Create `duckdb-spike/` directory at `desktop/win/duckdb-spike/`
2. Write `Cargo.toml` with `duckdb = { version = "1", features = ["bundled"] }`
3. Write `src/main.rs` with the spike test code from 07-testing-strategy.md
4. Run `cargo build` inside `duckdb-spike/`
5. If build fails, follow the fallback chain:

| Step | Command | Notes |
|------|---------|-------|
| 1 (default) | `cargo build` | Uses `bundled` feature, compiles DuckDB C++ from source |
| 2 | `set DUCKDB_DOWNLOAD_LIB=1 && cargo build` | Downloads prebuilt DuckDB binary instead of compiling |
| 3 | Replace `duckdb` dep with `frozen-duckdb = { version = "1", features = ["bundled"] }` | Fork that pins a known-good version |
| 4 | **PIVOT**: Abandon DuckDB, evaluate DataFusion | Requires re-scoping 03-schema.md (DataFusion uses SQL but different DDL). DataFusion is pure Rust -- no C++ build risk but no built-in persistence (must add Parquet layer). |

6. Run `cargo run` and verify all phases pass
7. Record timing results

### Go/No-Go Criteria

| Criterion | Pass | Fail Action |
|-----------|------|-------------|
| `cargo build` succeeds (any fallback step) | Required | Pivot to DataFusion |
| `open + migrate + insert 5K + query 5K` < 1 second | Required | Investigate; may proceed if < 2s |
| f32 prices survive roundtrip exactly | Required | Investigate FLOAT semantics |
| File-based DB persists across close/reopen | Required | Bug in spike code |

### Decision Output

Document the outcome in a brief `duckdb-spike/RESULT.md`:
- Which build method worked (bundled / download / frozen / pivot)
- Exact timing results
- Any workarounds required
- Go / No-Go decision

### Cleanup

After the decision, the `duckdb-spike/` directory can be deleted. Its code
served its purpose. The production crate will be built fresh.

### Risk Factors

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| `bundled` fails on Windows MSVC (known issues #544, #413) | Medium | Blocks phase 0 | Fallback chain (steps 2-4) |
| C++ compile takes > 15 minutes | Medium | Time box pressure | Use `DUCKDB_DOWNLOAD_LIB=1` to skip compile |
| DuckDB Appender API unavailable in Rust crate version | Low | Blocks bulk insert perf | Fall back to prepared statements with explicit transaction |
| DataFusion pivot needed | Low | Re-scopes schema, adds 2-3 days | 03-schema.md notes DuckDB-specific SQL that needs revision |

**Complexity: S** (small, self-contained, time-boxed)

---

## Phase 1: Crate Skeleton + Schema

**Goal:** Create the `midas-store` crate with working schema migrations
and the `mailbox_processor` dependency in place.

**Depends on:** Phase 0 (Go decision)

### Files Created

| File | Purpose |
|------|---------|
| `crates/mailbox_processor/Cargo.toml` | Local copy of mailbox_processor |
| `crates/mailbox_processor/src/lib.rs` | Copied from `D:\GitHub\ControlPlugin\Shared\mailbox_processor\src\lib.rs` + `new_blocking()` added |
| `crates/midas-store/Cargo.toml` | Crate manifest |
| `crates/midas-store/src/lib.rs` | Module declarations, test_helpers, re-exports |
| `crates/midas-store/src/error.rs` | `StoreError` enum |
| `crates/midas-store/src/types.rs` | `DataKey`, `CacheInfo`, `StoreConfig` |
| `crates/midas-store/src/schema.rs` | DDL, `run_migrations()`, migration tracking |

### Files Modified

| File | Change |
|------|--------|
| `desktop/win/Cargo.toml` | Add `"crates/mailbox_processor"` and `"crates/midas-store"` to `[workspace] members` |

### Implementation Details

#### 1.1 Copy and Extend `mailbox_processor`

Copy `D:\GitHub\ControlPlugin\Shared\mailbox_processor\` to
`desktop/win/crates/mailbox_processor/`. Add `new_blocking()` constructor:

**DECISION: Use `blocking_recv()`** — the simpler form with no mini tokio
runtime. The DuckDB handler is purely synchronous C++ FFI.

```rust
pub fn new_blocking<State: 'static + Send>(
    buffer_size: BufferSize,
    initial_state: State,
    thread_name: &str,
    handler: impl Fn(Msg, State, Option<Sender<ReplyMsg>>) -> State + Send + 'static,
) -> Self {
    let (s, mut r) = mpsc::channel(buffer_size.unwrap_or(1_000));
    std::thread::Builder::new()
        .name(thread_name.to_owned())
        .spawn(move || {
            let mut state = initial_state;
            while let Some((msg, reply_channel)) = r.blocking_recv() {
                state = handler(msg, state, reply_channel);
            }
        })
        .expect("failed to spawn mailbox processor thread");
    MailboxProcessor { message_sender: s }
}
```

Use this simpler form. DuckDB's handler is entirely synchronous.

**Also add `#[derive(Clone)]` to `MailboxProcessor`:**

The existing crate only derives `Debug`. `DbHandle` requires `Clone`
(to share across chart panels), and it wraps `MailboxProcessor`. Since
`MailboxProcessor` wraps `Sender<...>` which is `Clone`, adding the derive
is safe and straightforward:

```rust
#[derive(Debug, Clone)]
pub struct MailboxProcessor<Msg, ReplyMsg> {
    message_sender: Sender<(Msg, Option<Sender<ReplyMsg>>)>,
}
```

This is a **required change**, not a risk — add it alongside `new_blocking()`.

#### 1.2 `error.rs`

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("DuckDB error: {0}")]
    DuckDb(#[from] duckdb::Error),

    #[error("migration failed: {0}")]
    Migration(String),

    #[error("actor channel closed")]
    ChannelClosed,

    #[error("invalid timeframe_secs: {0}")]
    InvalidTimeframe(u32),

    #[error("unexpected reply from actor")]
    UnexpectedReply,
}
```

#### 1.3 `types.rs`

```rust
use midas_core::Timeframe;

#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub struct DataKey {
    pub symbol: String,
    pub timeframe: Timeframe,
}

#[derive(Clone, Debug)]
pub struct CacheInfo {
    pub key: DataKey,
    pub candle_count: usize,
    pub first_ts: i64,
    pub last_ts: i64,
    pub source: String,
}

#[derive(Clone, Debug)]
pub struct StoreConfig {
    pub enabled: bool,
    pub path: String,
    pub memory_limit_mb: u32,
}

impl Default for StoreConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            path: "cache.duckdb".to_owned(),
            memory_limit_mb: 256,
        }
    }
}
```

#### 1.4 `schema.rs`

```rust
use duckdb::Connection;
use crate::error::StoreError;

const CURRENT_VERSION: i32 = 1;

pub fn run_migrations(conn: &Connection) -> Result<(), StoreError> {
    // Version table (idempotent)
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_version (
             version    INTEGER NOT NULL,
             applied_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
         );"
    ).map_err(|e| StoreError::Migration(e.to_string()))?;

    let current: Option<i32> = conn
        .query_row(
            "SELECT MAX(version) FROM schema_version",
            [],
            |row| row.get(0),
        )
        .ok()
        .flatten();

    let version = current.unwrap_or(0);

    if version < 1 {
        migrate_v1(conn)?;
    }

    Ok(())
}

fn migrate_v1(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        "CREATE SCHEMA IF NOT EXISTS market;
         CREATE SCHEMA IF NOT EXISTS meta;

         CREATE TABLE IF NOT EXISTS market.candles (
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

         CREATE TABLE IF NOT EXISTS meta.data_ranges (
             symbol         VARCHAR    NOT NULL,
             timeframe_secs INTEGER    NOT NULL,
             candle_count   INTEGER    NOT NULL,
             first_ts       BIGINT     NOT NULL,
             last_ts        BIGINT     NOT NULL,
             source         VARCHAR    NOT NULL,
             updated_at     TIMESTAMP  DEFAULT CURRENT_TIMESTAMP,
             PRIMARY KEY (symbol, timeframe_secs)
         );

         INSERT INTO schema_version (version) VALUES (1);"
    ).map_err(|e| StoreError::Migration(e.to_string()))?;

    Ok(())
}
```

### Tests Added

| Test | Module | Description |
|------|--------|-------------|
| `test_migration_idempotent` | schema | Run migrations twice |
| `test_schema_version_tracked` | schema | Version >= 1 after migration |
| `test_all_tables_exist` | schema | market.candles, meta.data_ranges present |

See 07-testing-strategy.md Section 3 for complete test code.

### Gate Criteria

```bash
cargo test -p midas-store          # All 3 schema tests pass
cargo test -p mailbox_processor    # Existing mailbox tests still pass
cargo clippy -p midas-store -- -D warnings  # No warnings
```

### Risk Factors

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| `mailbox_processor` copy diverges from upstream | Low | Technical debt | Document that this is a local fork; consider publishing as workspace crate |
| `new_blocking()` deadlocks with tokio | Low | Blocks phase 3 | Test with `#[tokio::test]`; use `blocking_recv()` (no tokio runtime on thread) |
| DuckDB CREATE SCHEMA syntax differs from spike | Low | Blocks migration | Validate exact SQL in Phase 0 spike |

**Complexity: M** (multiple files, mailbox_processor modification)

---

## Phase 2: Query Layer

**Goal:** Implement all SQL query functions and the CandleBuffer/DuckDB
conversion layer. This is the core data access code.

**Depends on:** Phase 1 (schema, types, error)

### Files Created

| File | Purpose |
|------|---------|
| `crates/midas-store/src/queries.rs` | `bulk_insert()`, `query_all()`, `query_range()`, `list_cached()`, `delete_symbol()` |
| `crates/midas-store/src/convert.rs` | `timeframe_from_secs()`, any future CandleBuffer conversion utilities |

### Files Modified

| File | Change |
|------|--------|
| `crates/midas-store/src/lib.rs` | Add `pub mod queries;` and `pub mod convert;` |

### Implementation Details

#### 2.1 `queries.rs`

```rust
use duckdb::{params, Connection};
use midas_data::CandleBuffer;

use crate::convert::timeframe_from_secs;
use crate::error::StoreError;
use crate::types::{CacheInfo, DataKey};

/// Bulk insert a CandleBuffer into market.candles using the Appender API.
///
/// Also updates meta.data_ranges with the inserted data's metadata.
/// Uses INSERT OR REPLACE semantics for upsert.
///
/// Returns the number of rows inserted.
pub fn bulk_insert(
    conn: &Connection,
    key: &DataKey,
    buf: &CandleBuffer,
) -> Result<usize, StoreError> {
    if buf.is_empty() {
        return Ok(0);
    }

    let tf_secs = key.timeframe.as_secs() as i32;

    // Delete existing data for this key (upsert via delete + re-insert).
    // The Appender API does not support INSERT OR REPLACE, so we must
    // clear first. For append-only time-series this is typically a no-op.
    conn.execute(
        "DELETE FROM market.candles WHERE symbol = ? AND timeframe_secs = ?",
        params![&key.symbol, tf_secs],
    )?;

    // Bulk insert via Appender (fastest path)
    {
        let mut appender = conn.appender("market.candles")?;
        for i in 0..buf.len() {
            appender.append_row(params![
                &key.symbol,
                tf_secs,
                buf.timestamps[i],
                buf.opens[i],
                buf.highs[i],
                buf.lows[i],
                buf.closes[i],
                buf.volumes[i],
            ])?;
        }
        appender.flush()?;
    }

    // Update metadata
    let first_ts = buf.timestamps[0];
    let last_ts = *buf.timestamps.last().unwrap();

    conn.execute(
        "INSERT OR REPLACE INTO meta.data_ranges
         (symbol, timeframe_secs, candle_count, first_ts, last_ts, source)
         VALUES (?, ?, ?, ?, ?, 'rust')",
        params![&key.symbol, tf_secs, buf.len() as i32, first_ts, last_ts],
    )?;

    Ok(buf.len())
}

/// Query all candles for a given DataKey, ordered by timestamp ascending.
pub fn query_all(conn: &Connection, key: &DataKey) -> Result<CandleBuffer, StoreError> {
    let tf_secs = key.timeframe.as_secs() as i32;

    let mut stmt = conn.prepare_cached(
        "SELECT timestamp_ms, open, high, low, close, volume
         FROM market.candles
         WHERE symbol = ? AND timeframe_secs = ?
         ORDER BY timestamp_ms ASC",
    )?;

    let mut buf = CandleBuffer::with_capacity(5000);
    let mut rows = stmt.query(params![&key.symbol, tf_secs])?;
    while let Some(row) = rows.next()? {
        buf.push(
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
            row.get(5)?,
        );
    }
    Ok(buf)
}

/// Query candles within a time range [start_ts, end_ts] inclusive.
pub fn query_range(
    conn: &Connection,
    key: &DataKey,
    start_ts: i64,
    end_ts: i64,
) -> Result<CandleBuffer, StoreError> {
    let tf_secs = key.timeframe.as_secs() as i32;

    let mut stmt = conn.prepare_cached(
        "SELECT timestamp_ms, open, high, low, close, volume
         FROM market.candles
         WHERE symbol = ? AND timeframe_secs = ?
           AND timestamp_ms >= ? AND timestamp_ms <= ?
         ORDER BY timestamp_ms ASC",
    )?;

    let mut buf = CandleBuffer::with_capacity(5000);
    let mut rows = stmt.query(params![&key.symbol, tf_secs, start_ts, end_ts])?;
    while let Some(row) = rows.next()? {
        buf.push(
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
            row.get(5)?,
        );
    }
    Ok(buf)
}

/// List all cached symbol/timeframe pairs with metadata.
pub fn list_cached(conn: &Connection) -> Result<Vec<CacheInfo>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT symbol, timeframe_secs, candle_count, first_ts, last_ts, source
         FROM meta.data_ranges
         ORDER BY symbol, timeframe_secs",
    )?;

    let mut result = Vec::new();
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let tf_secs: u32 = row.get::<_, i32>(1)? as u32;
        let timeframe = timeframe_from_secs(tf_secs)
            .ok_or(StoreError::InvalidTimeframe(tf_secs))?;

        result.push(CacheInfo {
            key: DataKey {
                symbol: row.get(0)?,
                timeframe,
            },
            candle_count: row.get::<_, i32>(2)? as usize,
            first_ts: row.get(3)?,
            last_ts: row.get(4)?,
            source: row.get(5)?,
        });
    }
    Ok(result)
}

/// Delete all data for a given symbol/timeframe from both candles and metadata.
pub fn delete_symbol(conn: &Connection, key: &DataKey) -> Result<(), StoreError> {
    let tf_secs = key.timeframe.as_secs() as i32;
    conn.execute(
        "DELETE FROM market.candles WHERE symbol = ? AND timeframe_secs = ?",
        params![&key.symbol, tf_secs],
    )?;
    conn.execute(
        "DELETE FROM meta.data_ranges WHERE symbol = ? AND timeframe_secs = ?",
        params![&key.symbol, tf_secs],
    )?;
    Ok(())
}
```

#### 2.2 `convert.rs`

```rust
use midas_core::Timeframe;

/// Reconstruct a Timeframe from its `as_secs()` value.
///
/// Returns `None` for unrecognized values. This is the inverse of
/// `Timeframe::as_secs()`.
pub fn timeframe_from_secs(secs: u32) -> Option<Timeframe> {
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
```

### Tests Added

| Test | Module | Description |
|------|--------|-------------|
| `test_bulk_insert_roundtrip` | queries | Insert + query, field-by-field |
| `test_empty_query` | queries | Non-existent symbol returns empty |
| `test_range_query` | queries | 1000 candles, query subset |
| `test_upsert_overwrites` | queries | Overlapping data, latest wins |
| `test_data_ranges_updated` | queries | Insert updates metadata |
| `test_duplicate_insert_ignored` | queries | No duplicates |
| `test_delete_symbol` | queries | Insert, delete, verify empty |
| `test_multiple_symbols` | queries | Two symbols, no cross-talk |
| `test_multiple_timeframes` | queries | D1 and M5, same symbol |
| `test_large_buffer_insert` | queries | 50K candles |
| `test_f32_roundtrip` | convert | f32 bitwise fidelity |
| `test_u32_volume_roundtrip` | convert | u32 exact roundtrip |
| `test_i64_timestamp_roundtrip` | convert | i64 exact roundtrip |
| `test_timeframe_secs_roundtrip` | convert | Timeframe -> u32 -> Timeframe |

See 07-testing-strategy.md Sections 4-5 for complete test code.

### Gate Criteria

```bash
cargo test -p midas-store          # All 17 tests pass (3 schema + 10 query + 4 convert)
cargo clippy -p midas-store -- -D warnings
```

### Risk Factors

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| DuckDB Appender API returns different error types than expected | Medium | Code refactoring | Validate in Phase 0 spike; wrap all duckdb errors via `#[from]` |
| `prepare_cached` not available in duckdb crate | Low | Minor perf impact | Fall back to `prepare()`; cache is an optimization not a requirement |
| f32 roundtrip fails for edge-case values (subnormals, NaN) | Low | Data corruption | Test with explicit edge cases; filter NaN/Inf at insert boundary |
| DELETE + re-INSERT upsert strategy too slow for large datasets | Medium | Perf regression | Acceptable for v1 (< 50K rows per symbol); optimize with merge in v2 |

**Complexity: M** (core data access, most tests)

---

## Phase 3: Actor + Handle

**Goal:** Wrap the synchronous query layer in an async actor via
`MailboxProcessor::new_blocking()`. Expose the `DbHandle` public API.

**Depends on:** Phase 2 (queries, convert)

### Files Created

| File | Purpose |
|------|---------|
| `crates/midas-store/src/actor.rs` | `DbCommand`, `DbReply` enums, actor handler closure |
| `crates/midas-store/src/handle.rs` | `DbHandle` struct wrapping `MailboxProcessor` |

### Files Modified

| File | Change |
|------|--------|
| `crates/midas-store/src/lib.rs` | Add `pub mod actor;` and `pub mod handle;` |
| `crates/midas-store/Cargo.toml` | Ensure `tempfile` in `[dev-dependencies]` |

### Implementation Details

#### 3.1 `actor.rs`

```rust
use midas_data::CandleBuffer;
use crate::error::StoreError;
use crate::types::{CacheInfo, DataKey};

/// Commands sent to the DuckDB actor thread.
pub(crate) enum DbCommand {
    InsertCandles { key: DataKey, buffer: CandleBuffer },
    QueryCandles { key: DataKey },
    QueryCandlesRange { key: DataKey, start: i64, end: i64 },
    ListCached,
    Shutdown,
}

/// Replies from the DuckDB actor thread.
pub(crate) enum DbReply {
    Inserted(Result<usize, StoreError>),
    Candles(Result<CandleBuffer, StoreError>),
    CacheList(Result<Vec<CacheInfo>, StoreError>),
    ShutdownAck,
}
```

#### 3.2 `handle.rs`

> **Note:** The canonical `DbHandle` implementation is in
> [04-dbhandle-api.md Section 3.3](04-dbhandle-api.md). The code below is
> a simplified illustration. Key differences from canonical:
> - `open()` is synchronous (`pub fn open(config: StoreConfig) -> Self`), not async
> - Connection failures use `DbReply::Error(...)`, not `expect()`/panic
> - Startup includes data_ranges reconciliation SQL

```rust
use std::path::Path;
use duckdb::Connection;
use mailbox_processor::{BufferSize, MailboxProcessor};
use tokio::sync::mpsc::Sender;
use midas_data::CandleBuffer;
use tracing::{info, error};

use crate::actor::{DbCommand, DbReply};
use crate::error::StoreError;
use crate::queries;
use crate::schema::run_migrations;
use crate::types::{CacheInfo, DataKey, StoreConfig};

#[derive(Clone)]
pub struct DbHandle {
    mb: MailboxProcessor<DbCommand, DbReply>,
}

impl DbHandle {
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let path = path.as_ref().to_owned();
        let mb = MailboxProcessor::new_blocking(
            BufferSize::Size(256),
            None::<Connection>,
            "duckdb-store",
            move |cmd, conn_state, reply_channel| {
                let conn = conn_state.unwrap_or_else(|| {
                    let c = Connection::open(&path).expect("DuckDB open failed");
                    configure_connection(&c);
                    run_migrations(&c).expect("DuckDB migration failed");
                    info!("DuckDB store opened: {}", path.display());
                    c
                });
                handle_command(cmd, &conn, reply_channel);
                Some(conn)
            },
        );
        Ok(Self { mb })
    }

    pub async fn open_memory() -> Result<Self, StoreError> {
        let mb = MailboxProcessor::new_blocking(
            BufferSize::Size(256),
            None::<Connection>,
            "duckdb-store-mem",
            |cmd, conn_state, reply_channel| {
                let conn = conn_state.unwrap_or_else(|| {
                    let c = Connection::open_in_memory().expect("DuckDB in-memory open failed");
                    run_migrations(&c).expect("DuckDB migration failed");
                    c
                });
                handle_command(cmd, &conn, reply_channel);
                Some(conn)
            },
        );
        Ok(Self { mb })
    }

    pub async fn insert_candles(
        &self, key: DataKey, buffer: CandleBuffer,
    ) -> Result<usize, StoreError> {
        let reply = self.mb.send(DbCommand::InsertCandles { key, buffer }).await
            .map_err(|_| StoreError::ChannelClosed)?;
        match reply {
            DbReply::Inserted(r) => r,
            _ => Err(StoreError::UnexpectedReply),
        }
    }

    pub async fn fire_and_forget_insert(
        &self, key: DataKey, buffer: CandleBuffer,
    ) -> Result<(), StoreError> {
        self.mb.fire_and_forget(DbCommand::InsertCandles { key, buffer }).await
            .map_err(|_| StoreError::ChannelClosed)
    }

    pub async fn query_candles(&self, key: DataKey) -> Result<CandleBuffer, StoreError> {
        let reply = self.mb.send(DbCommand::QueryCandles { key }).await
            .map_err(|_| StoreError::ChannelClosed)?;
        match reply {
            DbReply::Candles(r) => r,
            _ => Err(StoreError::UnexpectedReply),
        }
    }

    pub async fn query_candles_range(
        &self, key: DataKey, start: i64, end: i64,
    ) -> Result<CandleBuffer, StoreError> {
        let reply = self.mb.send(DbCommand::QueryCandlesRange { key, start, end }).await
            .map_err(|_| StoreError::ChannelClosed)?;
        match reply {
            DbReply::Candles(r) => r,
            _ => Err(StoreError::UnexpectedReply),
        }
    }

    pub async fn list_cached(&self) -> Result<Vec<CacheInfo>, StoreError> {
        let reply = self.mb.send(DbCommand::ListCached).await
            .map_err(|_| StoreError::ChannelClosed)?;
        match reply {
            DbReply::CacheList(r) => r,
            _ => Err(StoreError::UnexpectedReply),
        }
    }

    pub async fn shutdown(&self) -> Result<(), StoreError> {
        let reply = self.mb.send(DbCommand::Shutdown).await
            .map_err(|_| StoreError::ChannelClosed)?;
        match reply {
            DbReply::ShutdownAck => Ok(()),
            _ => Err(StoreError::UnexpectedReply),
        }
    }
}

fn configure_connection(conn: &Connection) {
    let _ = conn.execute_batch(
        "SET memory_limit = '256MB';
         SET threads = 2;
         SET enable_progress_bar = false;",
    );
}

fn handle_command(
    cmd: DbCommand,
    conn: &Connection,
    reply_channel: Option<Sender<DbReply>>,
) {
    let reply = match cmd {
        DbCommand::InsertCandles { key, buffer } => {
            DbReply::Inserted(queries::bulk_insert(conn, &key, &buffer))
        }
        DbCommand::QueryCandles { key } => {
            DbReply::Candles(queries::query_all(conn, &key))
        }
        DbCommand::QueryCandlesRange { key, start, end } => {
            DbReply::Candles(queries::query_range(conn, &key, start, end))
        }
        DbCommand::ListCached => {
            DbReply::CacheList(queries::list_cached(conn))
        }
        DbCommand::Shutdown => {
            DbReply::ShutdownAck
        }
    };

    if let Some(ch) = reply_channel {
        // blocking_send because we are on a std::thread, not a tokio task
        let _ = ch.blocking_send(reply);
    }
}
```

### Tests Added

| Test | Module | Description |
|------|--------|-------------|
| `test_dbhandle_open_file` | handle | File-based handle |
| `test_dbhandle_open_memory` | handle | In-memory handle |
| `test_dbhandle_concurrent_queries` | handle | 10 concurrent tasks |
| `test_dbhandle_fire_and_forget_insert` | handle | Non-blocking insert |
| `test_dbhandle_shutdown_clean` | handle | Clean shutdown |
| `test_dbhandle_clone_independence` | handle | Clone survives original drop |
| `test_dbhandle_list_cached` | handle | Metadata listing |
| `test_dbhandle_query_range` | handle | Range query via handle |

See 07-testing-strategy.md Section 6 for complete test code.

### Gate Criteria

```bash
cargo test -p midas-store          # All 25 tests pass (3 + 10 + 4 + 8)
cargo clippy -p midas-store -- -D warnings
```

### Risk Factors

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| `blocking_send()` on reply channel panics in test | Low | Test failure | Ensure `#[tokio::test]` multi-thread runtime; test reply channel capacity |
| `new_blocking()` handler closure lifetime issues | Medium | Compile error | Use `move` closures; ensure `path` is owned (`PathBuf`) |
| Actor thread does not shut down cleanly | Medium | Thread leak in tests | `Shutdown` command drops the connection; dropping all senders closes the channel; the `while let Some(...)` loop exits |
| `MailboxProcessor::Clone` not derived | N/A | N/A | **Resolved in Phase 1** — `#[derive(Clone)]` added alongside `new_blocking()` |

**Complexity: M** (actor pattern, async-sync bridge)

---

## Phase 4: App Integration

**Goal:** Wire `DbHandle` into `MidasApp` with graceful fallback. The app
must work identically whether the store is enabled or disabled.

**Depends on:** Phase 3 (DbHandle working)

### Files Modified

| File | Change |
|------|--------|
| `crates/midas-core/src/config.rs` | Add `StoreConfig` section to `AppConfig` |
| `crates/midas-app/Cargo.toml` | Add `midas-store = { path = "../midas-store" }` dependency |
| `crates/midas-app/src/app.rs` | Add `store: Option<DbHandle>` field to `MidasApp` |
| `crates/midas-app/src/app.rs` | Add `StoreConfig` loading in `MidasApp::new()` |
| `crates/midas-app/src/app.rs` | Modify `load_symbol_for_chart()` to try DuckDB first |
| `desktop/win/Cargo.toml` | Add `midas-store` to root `[dependencies]` if workspace tests need it |

### Implementation Details

#### 4.1 Config Extension

Add to `crates/midas-core/src/config.rs`:

```rust
/// DuckDB persistent cache configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreConfig {
    /// Whether the DuckDB cache is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// Path to the DuckDB file (relative to data directory).
    #[serde(default = "default_store_path")]
    pub path: String,
    /// Maximum memory DuckDB may use for query processing (MB).
    #[serde(default = "default_memory_limit")]
    pub memory_limit_mb: u32,
}

fn default_store_path() -> String { "cache.duckdb".to_owned() }
fn default_memory_limit() -> u32 { 256 }

impl Default for StoreConfig {
    fn default() -> Self {
        Self {
            enabled: true,  // DuckDB activates on first launch
            path: default_store_path(),
            memory_limit_mb: default_memory_limit(),
        }
    }
}
```

Add to `AppConfig`:

```rust
pub struct AppConfig {
    // ... existing fields ...
    #[serde(default)]
    pub store: StoreConfig,
}
```

#### 4.2 MidasApp Changes

```rust
pub struct MidasApp {
    // ... existing fields ...
    /// DuckDB persistent cache handle. None if disabled or failed to open.
    pub store: Option<midas_store::handle::DbHandle>,
}
```

In `MidasApp::new()`:

```rust
// DbHandle::open() is synchronous — spawns actor thread, no DB I/O yet.
// Connection opens lazily on first command. Errors surface via Task.
let store = if config.store.enabled {
    let data_dir = /* resolve data directory */;
    let db_path = data_dir.join(&config.store.path);
    tracing::info!("DuckDB store configured: {}", db_path.display());
    Some(DbHandle::open(StoreConfig {
        path: Some(db_path),
        memory_limit_mb: config.store.memory_limit_mb,
        ..Default::default()
    }))
} else {
    tracing::info!("DuckDB store disabled in config");
    None
};

// Surface connection errors via health-check startup task:
let store_health_task = if let Some(ref db) = store {
    let db = db.clone();
    Task::perform(
        async move {
            match db.list_cached().await {
                Ok(_) => Ok(()),
                Err(e) => Err(e.to_string()),
            }
        },
        Message::StoreReady,
    )
} else {
    Task::none()
};
```

#### 4.3 Graceful Fallback

The key invariant: **`store = None` must be indistinguishable from today's
behavior.** No code path should panic or degrade when the store is absent.

```rust
// In the symbol load handler:
if let Some(ref store) = self.store {
    // Try DuckDB first
    let store = store.clone();
    let key = DataKey { symbol: symbol.clone(), timeframe };
    Task::perform(
        async move { store.query_candles(key).await },
        move |result| match result {
            Ok(buf) if !buf.is_empty() => Message::DataLoaded(id, Ok(Arc::new(buf))),
            _ => Message::DataCacheMiss(id, /* key */),
        },
    )
} else {
    // Fallback: TestDataProvider (today's behavior)
    self.load_test_data_for_chart(id, &symbol, timeframe, true);
    Task::none()
}
```

### Tests Added

| Test | Location | Description |
|------|----------|-------------|
| `test_store_disabled_fallback` | `tests/store_integration.rs` | App works with store=None |

See 07-testing-strategy.md Section 7 for test code.

### Gate Criteria

| Criterion | How to Verify |
|-----------|---------------|
| App starts with `store.enabled = false` | `cargo run -p midas-app`, verify charts load via TestDataProvider |
| App starts with `store.enabled = true` | Set config, `cargo run`, verify DbHandle construction in logs |
| No regression | `cargo test --workspace` passes (all existing 128+ tests) |
| Config round-trip | Save/load config.toml with `[store]` section |

### Risk Factors

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| `DbHandle::open()` integration with iced startup | Low | Minor | **Resolved**: `open()` is synchronous. Health check via `Task::perform()`. |
| DuckDB file lock prevents app restart | Low | UX issue | DuckDB releases lock on `Connection::drop`; `shutdown()` ensures clean close |
| New `Message` variants break exhaustive match | Low | Compile error | Add `DataCacheMiss`, `DataLoadFailed` variants |
| `midas-store` compile time adds 5+ minutes to build | Medium | Developer friction | Feature-gate behind `duckdb-store` feature on `midas-app` |

**Complexity: L** (touches app state machine, config, message routing)

---

## Phase 5: Write-Behind Cache

**Goal:** After any data provider loads data, fire-and-forget write to
DuckDB. On subsequent app restarts, DuckDB cache hit eliminates the
provider call.

**Depends on:** Phase 4 (app integration with fallback)

### Files Modified

| File | Change |
|------|--------|
| `crates/midas-app/src/app.rs` | Add write-behind after TestDataProvider load |
| `crates/midas-app/src/app.rs` | Add `DataCacheMiss` message handler |

### Implementation Details

#### 5.1 Write-Behind Flow

```
User types "AAPL" -> PanelSymbolSubmitted
  -> Try DuckDB query (cache miss first time)
  -> DataCacheMiss(id, key)
  -> TestDataProvider.get_candles("AAPL", D1)
  -> DataLoaded(id, Arc<CandleBuffer>)
  -> Chart renders immediately
  -> fire_and_forget: store.insert_candles(key, buffer.as_ref().clone())
  -> DuckDB now has AAPL D1 data

Next app restart:
  -> PanelSymbolSubmitted
  -> DuckDB query (cache HIT)
  -> DataLoaded(id, Arc<CandleBuffer>)
  -> Chart renders from cache. TestDataProvider never called.
```

#### 5.2 Message Handlers

```rust
Message::DataCacheMiss(id, key) => {
    // Fallback to TestDataProvider
    let buf = self.test_data_provider.get_candles(
        &key.symbol, key.timeframe, 365,
    );
    let arc_buf = Arc::new(buf);
    self.charts.get_mut(&id).unwrap().data = Some(arc_buf.clone());
    self.charts.get_mut(&id).unwrap().load_state = LoadState::Loaded;

    // Write-behind: async insert to DuckDB
    if let Some(ref store) = self.store {
        let store = store.clone();
        let buffer = arc_buf.as_ref().clone();
        let _ = store.fire_and_forget_insert(key, buffer).await;
    }

    Task::none()
}
```

#### 5.3 Cache Hit Detection

On the query path, an empty `CandleBuffer` result means cache miss:

```rust
Ok(buf) if !buf.is_empty() => {
    tracing::debug!("DuckDB cache hit for {}/{}", key.symbol, key.timeframe);
    Message::DataLoaded(id, Ok(Arc::new(buf)))
}
Ok(_) | Err(_) => {
    tracing::debug!("DuckDB cache miss for {}/{}", key.symbol, key.timeframe);
    Message::DataCacheMiss(id, key)
}
```

### Tests Added

| Test | Location | Description |
|------|----------|-------------|
| `test_write_behind_cache_cycle` | handle | Cache miss -> load -> fire_and_forget insert -> query -> cache hit |

```rust
#[tokio::test]
async fn test_write_behind_cache_cycle() {
    let db = DbHandle::open_memory();
    let key = DataKey {
        symbol: "TEST".into(),
        timeframe: Timeframe::D1,
    };

    // 1. Cache miss: query returns empty
    let result = db.query_candles(key.clone()).await.unwrap();
    assert!(result.is_empty(), "should be empty on first query");

    // 2. Simulate data load from TestDataProvider
    let mut buf = sample_buffer(100);

    // 3. Fire-and-forget write-behind insert
    db.fire_and_forget_insert(key.clone(), buf.clone()).await.unwrap();

    // 4. Small delay for actor to process
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // 5. Cache hit: query returns data
    let cached = db.query_candles(key).await.unwrap();
    assert_eq!(cached.len(), 100, "should find 100 candles in cache");
    assert_eq!(cached.timestamps[0], buf.timestamps[0]);

    db.shutdown().await.ok();
}
```

### Gate Criteria

| Criterion | How to Verify |
|-----------|---------------|
| First run: chart loads from TestDataProvider | Logs show `cache miss` then `TestDataProvider` call |
| First run: DuckDB gets populated | Logs show `fire_and_forget insert_candles` |
| Second run: chart loads from DuckDB | Logs show `cache hit`, no `TestDataProvider` call |
| Data integrity | Chart looks identical between cache-miss and cache-hit loads |

### Risk Factors

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| `fire_and_forget` silently fails | Medium | Cache never populated | Log errors in actor handler; test with `send()` first to validate |
| `CandleBuffer::clone()` too expensive for 50K candles | Low | Minor latency (< 1ms for 50K) | Profile; could use `Arc<CandleBuffer>` directly but insert needs ownership |
| Stale cache data after external data source changes | Low (v1 uses TestDataProvider) | Wrong chart data | Add cache invalidation / TTL in future phase |
| App crash between TestDataProvider load and DuckDB write | Low | Data not cached, re-fetched next time | Acceptable; fire-and-forget is best-effort |

**Complexity: M** (touches app message flow, async coordination)

---

## Phase 6: Benchmarks + Optimization

**Goal:** Add criterion benchmarks, profile startup overhead, tune DuckDB
configuration. Validate all performance targets.

**Depends on:** Phase 5 (write-behind working)

### Files Created

| File | Purpose |
|------|---------|
| `crates/midas-store/benches/store_bench.rs` | Criterion benchmarks |

### Files Modified

| File | Change |
|------|--------|
| `crates/midas-store/Cargo.toml` | Add `[[bench]]` section and dev-dependencies |

### Implementation Details

See 07-testing-strategy.md Section 8 for complete benchmark code.

#### 6.1 DuckDB Configuration Tuning

Test the following configurations and measure startup overhead:

```sql
-- Conservative (low memory)
SET memory_limit = '64MB';
SET threads = 1;

-- Balanced (default)
SET memory_limit = '256MB';
SET threads = 2;

-- Aggressive (fast queries)
SET memory_limit = '512MB';
SET threads = 4;
```

For desktop use, the balanced configuration is recommended. `threads = 2`
avoids starving the GPU and UI threads.

#### 6.2 Startup Profiling

Use `tracing` span timers to measure:

```
[STARTUP] DbHandle::open        ... 45ms
[STARTUP]   Connection::open     ... 12ms
[STARTUP]   run_migrations       ... 8ms
[STARTUP]   configure            ... 2ms
[STARTUP]   actor thread spawn   ... 1ms
[STARTUP] query_candles (AAPL)   ... 3ms
[STARTUP] query_candles (MSFT)   ... 4ms
[STARTUP] Total DuckDB overhead  ... 52ms
```

### Performance Targets

| Metric | Target | Phase 0 Spike Result | Actual (fill in) |
|--------|--------|---------------------|-------------------|
| `open + migrate` | < 100ms | (measured in spike) | |
| `insert 5K` | < 10ms | (measured in spike) | |
| `query 5K` | < 5ms | (measured in spike) | |
| `query range 1K of 50K` | < 3ms | n/a | |
| Startup overhead (4 charts) | < 100ms | n/a | |
| Memory (DuckDB process) | < 256MB | n/a | |

### Gate Criteria

```bash
cargo bench -p midas-store                     # Benchmarks run and produce reports
cargo bench -p midas-store -- open_migrate     # < 100ms median
cargo bench -p midas-store -- insert_5k        # < 10ms median
cargo bench -p midas-store -- query_5k         # < 5ms median
```

### Risk Factors

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| Query performance worse than spike | Low | Failed targets | Investigate: schema vs in-memory, connection config, query plan |
| DuckDB memory exceeds 256MB cap | Low | OOM pressure | `SET memory_limit` enforces hard cap; DuckDB spills to disk |
| Criterion benchmark noise on Windows | Medium | Flaky results | Run benchmarks with `--warm-up-time 5` and `--measurement-time 10` |

**Complexity: S** (benchmarks, configuration tuning)

---

## Future Phases (Out of Scope for v1)

These phases are documented for planning purposes. They will be specified
in detail when their prerequisites are met.

### Phase 7: CSV Import Writes to DuckDB

**Prerequisite:** Phase 5 (write-behind cache)

When the user imports a CSV file via `midas-feed::import_csv()`, the
resulting `CandleBuffer` should be persisted to DuckDB in addition to
writing the `.midas` binary file.

- Modify `midas-app` CSV import handler to call `store.insert_candles()`
- No changes to `midas-feed` (crate stays DuckDB-free)
- Test: import CSV, restart app, chart loads from DuckDB cache

### Phase 8: IB Streaming Batch Flush

**Prerequisite:** IB API integration (broker crate Phase 1), Phase 5

Real-time IB tick data is aggregated into candles by `midas-feed`. Every
5 seconds (configurable via `flush_interval_secs`), the accumulated
candles are batch-inserted into DuckDB.

- Add `flush_interval_secs` to `StoreConfig`
- Add periodic flush timer in `MidasApp`
- Use `fire_and_forget_insert` for non-blocking writes
- Test: stream 100 ticks, verify DuckDB has aggregated candles

### Phase 9: Analytical Queries

**Prerequisite:** Phase 5 (DuckDB populated with data)

Enable cross-symbol and window-function queries:

- Volume scanner: "All symbols with volume > 1M today"
- ATR computation: 14-period ATR via SQL window function
- Correlation: cross-symbol price correlation matrix
- These queries run on-demand (user-triggered), not per-frame

### Phase 10: Read Pool for Parallel Queries

**Prerequisite:** Phase 6 benchmarks showing serialized reads bottleneck

If 20+ simultaneous chart loads saturate the single actor thread:

- Add `Connection::try_clone()` for read-only connections
- Gate with `Semaphore(8)` to limit concurrent readers
- Keep single writer (actor) for all mutations
- Hybrid: writes go through actor, reads bypass via pool

---

## Dependency Graph

```
Phase 0: Build Spike
    |
Phase 1: Crate Skeleton + Schema
    |
Phase 2: Query Layer
    |
Phase 3: Actor + Handle
    |
Phase 4: App Integration
    |
Phase 5: Write-Behind Cache
    |
Phase 6: Benchmarks + Optimization
    |
    +-- Phase 7: CSV Import (future)
    +-- Phase 8: IB Streaming (future, needs IB API)
    +-- Phase 9: Analytical Queries (future)
    +-- Phase 10: Read Pool (future, needs profiling data)
```

---

## File Inventory

Complete list of files created or modified across all v1 phases.

### New Files

| Phase | File | Purpose |
|-------|------|---------|
| 0 | `duckdb-spike/Cargo.toml` | Throwaway spike crate (deleted after) |
| 0 | `duckdb-spike/src/main.rs` | Spike test code (deleted after) |
| 1 | `crates/mailbox_processor/Cargo.toml` | Local mailbox_processor copy |
| 1 | `crates/mailbox_processor/src/lib.rs` | Mailbox + `new_blocking()` |
| 1 | `crates/midas-store/Cargo.toml` | Crate manifest |
| 1 | `crates/midas-store/src/lib.rs` | Module declarations, test helpers |
| 1 | `crates/midas-store/src/error.rs` | StoreError enum |
| 1 | `crates/midas-store/src/types.rs` | DataKey, CacheInfo, StoreConfig |
| 1 | `crates/midas-store/src/schema.rs` | DDL, migration system |
| 2 | `crates/midas-store/src/queries.rs` | SQL query functions |
| 2 | `crates/midas-store/src/convert.rs` | Timeframe/type conversions |
| 3 | `crates/midas-store/src/actor.rs` | DbCommand/DbReply enums |
| 3 | `crates/midas-store/src/handle.rs` | DbHandle async API |
| 6 | `crates/midas-store/benches/store_bench.rs` | Criterion benchmarks |

### Modified Files

| Phase | File | Change |
|-------|------|--------|
| 1 | `desktop/win/Cargo.toml` | Add workspace members |
| 4 | `crates/midas-core/src/config.rs` | Add `StoreConfig` |
| 4 | `crates/midas-app/Cargo.toml` | Add `midas-store` dependency |
| 4 | `crates/midas-app/src/app.rs` | Add `store` field, message handlers |
| 5 | `crates/midas-app/src/app.rs` | Write-behind logic |

### Test Count by Phase

| Phase | New Tests | Cumulative |
|-------|-----------|------------|
| 0 | 0 (manual spike) | 0 |
| 1 | 3 (schema) | 3 |
| 2 | 14 (queries + convert) | 17 |
| 3 | 8 (handle integration) | 25 |
| 4 | 2 (workspace integration) | 27 |
| 5 | 1 (`test_write_behind_cache_cycle`) | 28 |
| 6 | 5 benchmarks | 28 tests + 5 benches |

---

## Quick Reference: Commands by Phase

```bash
# Phase 0
cd desktop/win/duckdb-spike && cargo run

# Phase 1
cargo test -p mailbox_processor
cargo test -p midas-store

# Phase 2
cargo test -p midas-store

# Phase 3
cargo test -p midas-store

# Phase 4
cargo test --workspace
cargo run -p midas-app

# Phase 5
cargo run -p midas-app   # run twice, check logs

# Phase 6
cargo bench -p midas-store
```
