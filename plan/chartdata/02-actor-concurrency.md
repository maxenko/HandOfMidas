# 02 -- Actor Concurrency Model

> Dedicated-thread actor for DuckDB operations in midas-store.
>
> Status: DESIGN SPECIFICATION
> Date: 2026-03-31
>
> **Companion document**: `01-crate-architecture.md` covers module layout,
> dependency graph, and public API surface.
>
> **Prerequisite reading**: The `mailbox_processor` crate source at
> `D:\GitHub\ControlPlugin\Shared\mailbox_processor\src\lib.rs`.

---

## 1. Why a Mailbox Actor

### The threading constraint

DuckDB's Rust bindings impose three constraints that make a naive
`tokio::spawn_blocking` approach inadequate:

| Type | `Send` | `Sync` | Implication |
|------|--------|--------|-------------|
| `duckdb::Connection` | Yes | **No** | Can move to a thread but cannot be shared via `&` across threads. `Arc<Mutex<Connection>>` technically works but holding a `Mutex` guard across a blocking FFI call starves other waiters. |
| `duckdb::Appender` | **No** | **No** | Must be created and consumed on the same thread. Cannot be sent to another thread at all. |
| `duckdb::InterruptHandle` | Yes | Yes | Can be cloned and shared freely. Used for cancellation from any thread. |

### Why not `spawn_blocking`?

```rust
// BAD: Each call gets a random threadpool thread.
// The Connection must move with it -- impossible to reuse.
tokio::task::spawn_blocking(move || {
    let conn = Connection::open("candles.duckdb")?; // Opens every time!
    conn.execute("INSERT ...", params)?;
    Ok(())
})
```

Problems:

1. **Connection per call.** `spawn_blocking` gives you a random thread from the
   blocking pool. You cannot guarantee the same thread runs consecutive calls,
   so the `Connection` cannot be reused. Opening a DuckDB connection takes
   ~5-20ms (C++ initialization, WAL recovery).

2. **Appender is !Send.** The `Appender` type (DuckDB's bulk insert path) is
   `!Send`. It cannot be sent to `spawn_blocking` at all. You would have to
   fall back to row-by-row `INSERT` statements, which is 10-50x slower for
   bulk imports.

3. **No serialization.** Multiple concurrent `spawn_blocking` calls can open
   multiple connections to the same database file, causing WAL contention and
   potential lock errors.

### Why a dedicated thread with a mailbox

```rust
// GOOD: Single thread owns the Connection for its entire lifetime.
// Messages arrive via mpsc channel. Appender is created and consumed
// on this same thread.
std::thread::Builder::new()
    .name("midas-store-db".into())
    .spawn(move || {
        let conn = Connection::open("candles.duckdb").unwrap();
        loop {
            match rx.blocking_recv() {
                Some((msg, reply_tx)) => handle(msg, &conn, reply_tx),
                None => break, // All senders dropped
            }
        }
    })
```

Benefits:

1. **Single connection, full lifetime.** The `Connection` lives on the dedicated
   thread from open to close. No repeated open/close overhead.

2. **Appender works.** Since `Appender` is `!Send` but we never move it off
   the thread, it works perfectly. Created per-batch, flushed, dropped -- all
   on the same thread.

3. **Natural serialization.** The mpsc channel serializes all commands. No
   locking, no contention, no deadlocks.

4. **Tokio isolation.** The dedicated thread is *not* part of the tokio
   threadpool. Blocking FFI calls cannot starve async tasks, the iced UI loop,
   or GPU frame submissions.

5. **Backpressure.** Bounded channel (capacity 256) provides natural
   backpressure. If the DB thread falls behind, senders wait.

---

## 2. `new_blocking()` Implementation

This section specifies the exact code to add to the `mailbox_processor` crate.

### 2.1 New constructor

Add this method to `impl<Msg, ReplyMsg> MailboxProcessor<Msg, ReplyMsg>`:

```rust
/// Create a mailbox processor that runs its handler on a dedicated OS thread.
///
/// Unlike [`new()`] which spawns a tokio task (async handler on the
/// tokio threadpool), this spawns a `std::thread` with a synchronous
/// handler and uses `blocking_recv()` for the message loop.
///
/// # Use Case
///
/// Blocking FFI (DuckDB, SQLite, etc.) that must not run on the tokio
/// threadpool. The handler function is `Fn` (not `async Fn`), so it
/// can call blocking APIs directly.
///
/// # Thread Naming
///
/// The thread is named `thread_name` for debuggability in profilers
/// and `ps` output.
///
/// # Shutdown
///
/// The thread exits when all `MailboxProcessor` clones (senders) are
/// dropped. The `blocking_recv()` call returns `None`, the loop exits,
/// and the thread terminates.
///
/// # Panics
///
/// Panics if the OS thread cannot be spawned (e.g., resource exhaustion).
pub fn new_blocking<State: 'static + Send>(
    buffer_size: BufferSize,
    initial_state: State,
    thread_name: &str,
    handler: impl Fn(Msg, State, Option<Sender<ReplyMsg>>) -> State + Send + 'static,
) -> Self
where
    Msg: Send + 'static,
    ReplyMsg: Send + 'static,
{
    let (tx, mut rx) = mpsc::channel(buffer_size.unwrap_or(1_000));

    let name = thread_name.to_string();
    std::thread::Builder::new()
        .name(name.clone())
        .spawn(move || {
            let mut state = initial_state;
            while let Some((msg, reply_channel)) = rx.blocking_recv() {
                state = handler(msg, state, reply_channel);
            }
            // All senders dropped -- thread exits naturally.
        })
        .unwrap_or_else(|e| panic!("failed to spawn thread '{name}': {e}"));

    MailboxProcessor { message_sender: tx }
}
```

### 2.2 Synchronous reply helper

Add a free function alongside the existing `reply_if_present()`:

```rust
/// Synchronous version of [`reply_if_present`] for use in blocking handlers.
///
/// Attempts to send `value` on the reply channel. If the receiver has been
/// dropped (caller timed out or cancelled), the send silently fails.
///
/// # Blocking
///
/// This uses `blocking_send()` which blocks the current thread until the
/// receiver reads the value. Since the reply channel has capacity 1 and
/// the receiver is `send().await` on the caller side, this completes
/// almost instantly.
pub fn reply_sync<T: Send>(
    reply_channel: Option<Sender<T>>,
    value: T,
) {
    if let Some(channel) = reply_channel {
        // blocking_send on a capacity-1 channel. The receiver is
        // awaiting on the other side, so this returns immediately
        // unless the receiver was dropped.
        let _ = channel.blocking_send(value);
    }
}
```

### 2.3 Key design decisions in `new_blocking()`

**Why `blocking_recv()` and not a `futures::block_on()` loop?**

`blocking_recv()` is a method on `tokio::sync::mpsc::Receiver` that blocks the
current OS thread without requiring a tokio runtime context. It is the correct
primitive for a non-async thread reading from a tokio mpsc channel. Using
`futures::block_on(rx.recv())` would work but adds an unnecessary dependency
and creates a mini-runtime per call.

**Why `Fn` and not `FnMut`?**

The handler is `Fn` (not `FnMut`) to match the existing `new()` constructor's
signature. State mutation flows through the return value (`-> State`), not
through mutable captures. This makes the handler stateless in its closure
environment, which is cleaner and easier to reason about.

**Why `unwrap_or_else(panic)` on thread spawn?**

Thread creation failure is an unrecoverable system error (out of OS threads or
memory). Returning a `Result` would force every caller to handle a condition
they cannot meaningfully recover from. The panic message includes the thread
name for diagnostics.

**Why not `JoinHandle` stored?**

The `JoinHandle` from `std::thread::spawn` is intentionally not stored in the
`MailboxProcessor`. The thread's lifetime is governed by the channel: when all
sender clones are dropped, `blocking_recv()` returns `None` and the thread
exits. Storing and joining the handle would require the `MailboxProcessor` to
be non-`Clone`, which breaks the existing API contract.

If the caller needs to wait for the thread to fully exit (e.g., for graceful
shutdown with a WAL checkpoint), they should send a `Shutdown` command and
await the reply *before* dropping the last sender. This is exactly what
`DbHandle::shutdown()` does.

### 2.4 Updated mailbox_processor/Cargo.toml

```toml
[package]
name = "mailbox_processor"
version = "0.2.0"
edition = "2021"

[dependencies]
tokio = { version = "1", features = ["sync", "rt"] }
futures = "0.3"  # Used only by async constructors

[dev-dependencies]
tokio = { version = "1", features = ["rt-multi-thread", "macros", "sync"] }
```

### 2.5 Clone semantics (unchanged)

The existing `MailboxProcessor` does not derive `Clone`, but it should. The
inner `Sender` is `Clone`, and cloning gives a second sender handle to the
same actor -- this is the standard pattern for sharing an actor across
multiple consumers.

Add to the existing impl:

```rust
impl<Msg: Send, ReplyMsg: Send> Clone for MailboxProcessor<Msg, ReplyMsg> {
    fn clone(&self) -> Self {
        MailboxProcessor {
            message_sender: self.message_sender.clone(),
        }
    }
}
```

This is essential for `DbHandle` to be `Clone` (multiple chart panes sharing
one store handle).

---

## 3. DbCommand and DbReply Enums

### 3.1 DbCommand

Every message the `DbHandle` can send to the actor thread.

```rust
/// Commands sent from [`DbHandle`] to the DuckDB actor thread.
///
/// All variants carry owned data (no borrows) because they cross a thread
/// boundary via the mpsc channel.
pub(crate) enum DbCommand {
    /// Insert candles, skipping rows with duplicate timestamps.
    ///
    /// Uses the DuckDB Appender API for bulk throughput.
    /// Expects reply: `DbReply::Inserted(Result<u64, StoreError>)`.
    Insert {
        key: DataKey,
        candles: CandleBuffer,
    },

    /// Upsert candles: insert new rows, replace existing rows on
    /// primary key conflict `(symbol, timeframe, ts)`.
    ///
    /// Uses row-by-row `INSERT OR REPLACE` (Appender does not support
    /// conflict resolution).
    /// Expects reply: `DbReply::Inserted(Result<u64, StoreError>)`.
    Upsert {
        key: DataKey,
        candles: CandleBuffer,
    },

    /// Query candles for a key, optionally filtered by time range.
    ///
    /// Expects reply: `DbReply::Candles(Result<CandleBuffer, StoreError>)`.
    Query {
        key: DataKey,
        range: Option<TimeRange>,
    },

    /// List all cached datasets with metadata (count, time range).
    ///
    /// Expects reply: `DbReply::CacheList(Result<Vec<CacheInfo>, StoreError>)`.
    ListCached,

    /// Delete candles for a key, optionally filtered by time range.
    ///
    /// Expects reply: `DbReply::Deleted(Result<u64, StoreError>)`.
    Delete {
        key: DataKey,
        range: Option<TimeRange>,
    },

    /// Force a WAL checkpoint to flush pending writes to disk.
    ///
    /// Expects reply: `DbReply::Done(Result<(), StoreError>)`.
    Checkpoint,

    /// Vacuum the database to reclaim disk space from deleted rows.
    ///
    /// Expects reply: `DbReply::Done(Result<(), StoreError>)`.
    Vacuum,

    /// Graceful shutdown: checkpoint WAL, close connection.
    ///
    /// Expects reply: `DbReply::Done(Result<(), StoreError>)`.
    /// After sending this reply, the actor thread exits.
    Shutdown,
}
```

### 3.2 DbReply

Every possible response from the actor thread.

```rust
/// Replies sent from the DuckDB actor thread back to [`DbHandle`] callers.
///
/// Each variant wraps a `Result` so that database errors can flow back
/// to the caller through the reply channel.
pub(crate) enum DbReply {
    /// Response to `Insert` or `Upsert`. Payload is rows affected.
    Inserted(Result<u64, StoreError>),

    /// Response to `Query`. Payload is the requested candle data.
    Candles(Result<CandleBuffer, StoreError>),

    /// Response to `ListCached`. Payload is metadata for all cached datasets.
    CacheList(Result<Vec<CacheInfo>, StoreError>),

    /// Response to `Delete`. Payload is rows deleted.
    Deleted(Result<u64, StoreError>),

    /// Response to `Checkpoint`, `Vacuum`, or `Shutdown`.
    Done(Result<(), StoreError>),
}
```

### 3.3 Message flow table

| DbHandle method | Sends | Reply channel | Expects |
|-----------------|-------|---------------|---------|
| `insert()` | `Insert { key, candles }` | `Some` | `Inserted(Ok(n))` |
| `insert_fire_and_forget()` | `Insert { key, candles }` | `None` | (none) |
| `upsert()` | `Upsert { key, candles }` | `Some` | `Inserted(Ok(n))` |
| `query()` | `Query { key, range }` | `Some` | `Candles(Ok(buf))` |
| `list_cached()` | `ListCached` | `Some` | `CacheList(Ok(vec))` |
| `delete()` | `Delete { key, range }` | `Some` | `Deleted(Ok(n))` |
| `checkpoint()` | `Checkpoint` | `Some` | `Done(Ok(()))` |
| `vacuum()` | `Vacuum` | `Some` | `Done(Ok(()))` |
| `shutdown()` | `Shutdown` | `Some` | `Done(Ok(()))` |

---

## 4. Actor Handler Function

### 4.1 Actor state

The actor state is a simple struct holding the DuckDB connection and
configuration. The connection is initialized lazily on the first message.

```rust
/// State held by the DuckDB actor thread across message processing.
///
/// This struct is `Send` because `Connection` is `Send`. It is moved
/// into the thread closure and never shared.
pub(crate) struct DbActorState {
    /// DuckDB connection. `None` until the first message triggers
    /// lazy initialization.
    conn: Option<duckdb::Connection>,

    /// Configuration for opening the connection.
    config: StoreConfig,

    /// Whether a shutdown has been requested.
    shutting_down: bool,
}

impl DbActorState {
    pub(crate) fn new(config: StoreConfig) -> Self {
        Self {
            conn: None,
            config,
            shutting_down: false,
        }
    }
}
```

### 4.2 Lazy connection initialization

The connection is opened on the first message rather than during construction.
This has two benefits:

1. **Error reporting.** Connection errors are reported through the reply channel
   as `StoreError`, rather than panicking during `DbHandle::open()`.
2. **Fast startup.** `DbHandle::open()` returns immediately. The ~10-20ms
   DuckDB initialization happens on the first actual database operation, which
   is typically a fire-and-forget insert that the UI does not wait for.

```rust
/// Ensure the connection is open, initializing it if necessary.
///
/// Returns a mutable reference to the connection, or an error if
/// initialization fails.
fn ensure_connection(state: &mut DbActorState) -> Result<&duckdb::Connection, StoreError> {
    if state.conn.is_none() {
        let conn = open_and_configure(&state.config)?;
        state.conn = Some(conn);
    }
    // SAFETY: We just set it to Some if it was None.
    Ok(state.conn.as_ref().unwrap())
}

/// Open a DuckDB connection and apply configuration + migrations.
fn open_and_configure(config: &StoreConfig) -> Result<duckdb::Connection, StoreError> {
    // Ensure parent directory exists
    if let Some(parent) = config.db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let conn = duckdb::Connection::open(&config.db_path)
        .map_err(|e| StoreError::OpenFailed {
            path: config.db_path.clone(),
            source: e,
        })?;

    // Configure DuckDB engine parameters
    conn.execute_batch(&format!(
        "SET memory_limit = '{}B';
         SET threads = {};",
        config.memory_limit_bytes,
        config.threads,
    ))?;

    if let Some(ref temp_dir) = config.temp_directory {
        std::fs::create_dir_all(temp_dir)?;
        conn.execute_batch(&format!(
            "SET temp_directory = '{}';",
            temp_dir.display(),
        ))?;
    }

    tracing::info!(
        path = %config.db_path.display(),
        memory_limit_mb = config.memory_limit_bytes / (1024 * 1024),
        threads = config.threads,
        "DuckDB connection opened"
    );

    // Run schema migrations
    crate::schema::migrate(&conn)?;

    Ok(conn)
}
```

### 4.3 Handler function

The core handler that processes each message on the dedicated thread.

```rust
use tokio::sync::mpsc::Sender;
use mailbox_processor::reply_sync;

/// Process a single DbCommand on the actor thread.
///
/// This function is the handler passed to `MailboxProcessor::new_blocking()`.
/// It is called once per message in the receive loop.
///
/// # State Flow
///
/// The handler takes ownership of `state`, may mutate it (e.g., to initialize
/// the connection), and returns the updated state for the next message.
pub(crate) fn handle_db_command(
    cmd: DbCommand,
    mut state: DbActorState,
    reply_tx: Option<Sender<DbReply>>,
) -> DbActorState {
    if state.shutting_down {
        // After shutdown, reject all commands
        reply_sync(reply_tx, DbReply::Done(Err(StoreError::ActorShutdown)));
        return state;
    }

    match cmd {
        DbCommand::Insert { key, candles } => {
            let result = ensure_connection(&mut state)
                .and_then(|conn| crate::queries::insert_candles(conn, &key, &candles));

            match (&result, &reply_tx) {
                (Err(e), None) => {
                    // Fire-and-forget: log the error
                    tracing::warn!(
                        symbol = %key.symbol(),
                        timeframe = %key.timeframe(),
                        error = %e,
                        "fire-and-forget insert failed"
                    );
                }
                _ => {}
            }

            reply_sync(reply_tx, DbReply::Inserted(result));
        }

        DbCommand::Upsert { key, candles } => {
            let result = ensure_connection(&mut state)
                .and_then(|conn| crate::queries::upsert_candles(conn, &key, &candles));
            reply_sync(reply_tx, DbReply::Inserted(result));
        }

        DbCommand::Query { key, range } => {
            let result = ensure_connection(&mut state)
                .and_then(|conn| crate::queries::query_candles(conn, &key, range));
            reply_sync(reply_tx, DbReply::Candles(result));
        }

        DbCommand::ListCached => {
            let result = ensure_connection(&mut state)
                .and_then(|conn| crate::queries::list_cached(conn));
            reply_sync(reply_tx, DbReply::CacheList(result));
        }

        DbCommand::Delete { key, range } => {
            let result = ensure_connection(&mut state)
                .and_then(|conn| crate::queries::delete_candles(conn, &key, range));
            reply_sync(reply_tx, DbReply::Deleted(result));
        }

        DbCommand::Checkpoint => {
            let result = ensure_connection(&mut state)
                .and_then(|conn| crate::queries::checkpoint(conn));
            reply_sync(reply_tx, DbReply::Done(result));
        }

        DbCommand::Vacuum => {
            let result = ensure_connection(&mut state)
                .and_then(|conn| crate::queries::vacuum(conn));
            reply_sync(reply_tx, DbReply::Done(result));
        }

        DbCommand::Shutdown => {
            tracing::info!("store actor: shutdown requested");
            state.shutting_down = true;

            // Checkpoint before closing
            let result = if let Some(ref conn) = state.conn {
                crate::queries::checkpoint(conn).map(|_| {
                    tracing::info!("store actor: WAL checkpoint completed");
                })
            } else {
                Ok(()) // Never opened, nothing to checkpoint
            };

            // Drop the connection
            state.conn = None;

            reply_sync(reply_tx, DbReply::Done(result));

            // The actor will reject all subsequent messages.
            // When all senders drop, the blocking_recv() loop exits
            // and the thread terminates.
        }
    }

    state
}
```

### 4.4 Wiring it all together: `DbHandle::open()`

The `open()` function creates the actor and returns a handle.

```rust
impl DbHandle {
    /// Open (or create) a DuckDB store at the configured path.
    ///
    /// This returns immediately. The DuckDB connection is opened lazily
    /// on the first database operation (insert, query, etc.).
    ///
    /// # Thread
    ///
    /// Spawns a dedicated OS thread named `"midas-store-db"` for all
    /// database operations. This thread is NOT part of the tokio
    /// threadpool.
    ///
    /// # Shutdown
    ///
    /// The thread exits when:
    /// 1. `shutdown()` is called explicitly (preferred -- does WAL checkpoint), OR
    /// 2. All `DbHandle` clones are dropped (senders close, thread exits)
    /// Synchronous constructor. Spawns the actor thread and returns immediately.
    /// Connection opens lazily on the first command.
    /// See 04-dbhandle-api.md Section 3.3 for canonical implementation.
    pub fn open(config: StoreConfig) -> Self {
        let channel_capacity = config.channel_capacity;

        let mailbox = MailboxProcessor::<DbCommand, DbReply>::new_blocking(
            BufferSize::Size(channel_capacity),
            DbActorState::new(config),
            "midas-store-db",
            handle_db_command,
        );

        DbHandle { inner: mailbox }
    }
}
```

---

## 5. Connection Lifecycle

### 5.1 Lifecycle phases

```
Phase 1: CREATED
  DbHandle::open() called
  Thread spawned, waiting on channel
  Connection: None
  Duration: instant

Phase 2: IDLE (waiting for first message)
  Thread blocked on rx.blocking_recv()
  Connection: None
  Duration: until first insert/query

Phase 3: INITIALIZING (first message arrives)
  ensure_connection() called
  Connection::open() -- ~10-20ms (C++ init, WAL recovery)
  Schema migration -- ~5-10ms (first run only)
  SET memory_limit, threads, temp_directory
  Connection: Some(conn)

Phase 4: SERVING
  Normal message processing
  Connection: Some(conn), reused across all messages
  Duration: application lifetime

Phase 5: SHUTTING DOWN (Shutdown command received)
  CHECKPOINT -- flush WAL to main database file
  Connection dropped (Close())
  state.shutting_down = true
  Rejects subsequent messages with ActorShutdown error
  Thread continues until senders drop

Phase 6: TERMINATED
  All senders dropped
  blocking_recv() returns None
  Thread exits
```

### 5.2 Configuration parameters

Applied during Phase 3 (initialization):

```sql
-- Memory limit: 256 MB default. Conservative because DuckDB is the L2 cache,
-- not the primary working set. The L1 CandleBuffer in midas-data holds the
-- hot data in native Rust Vecs.
SET memory_limit = '268435456B';

-- Threads: 2 default. The store handles simple range scans and bulk inserts.
-- Complex analytical queries (joins, aggregations) are not part of the
-- workload. 2 threads is sufficient.
SET threads = 2;

-- Temp directory: for spill files if a query exceeds memory_limit.
-- Defaults to system temp + "midas-store-tmp/".
SET temp_directory = 'C:\Users\max\AppData\Local\Temp\midas-store-tmp';
```

### 5.3 WAL behavior

DuckDB uses a Write-Ahead Log (WAL) by default. Key behaviors:

- Writes go to the WAL first, then to the main file on checkpoint.
- `CHECKPOINT` forces a WAL flush. Called on graceful shutdown.
- If the application crashes, WAL recovery happens automatically on next open
  (during Phase 3). This may take a few hundred milliseconds depending on
  WAL size.
- The WAL file is `candles.duckdb.wal` next to the main database file.

---

## 6. Appender Usage

### 6.1 Why Appender

DuckDB's `Appender` is the high-performance bulk insert path. Compared to
row-by-row `INSERT` statements:

| Method | 10K rows | 100K rows | 1M rows |
|--------|----------|-----------|---------|
| Row-by-row INSERT | ~200ms | ~2s | ~20s |
| Appender | ~5ms | ~30ms | ~200ms |

For initial CSV imports (which commonly have 10K-50K candles), the Appender
provides a 40x speedup.

### 6.2 Appender constraints

```rust
// duckdb::Appender is:
//   - !Send: cannot be moved to another thread
//   - !Sync: cannot be shared by reference across threads
//
// This is fine because we create it on the actor thread,
// use it on the actor thread, and drop it on the actor thread.
// It never crosses a thread boundary.
```

### 6.3 Per-batch Appender pattern

The Appender is created fresh for each `Insert` command and dropped after
`flush()`. It is NOT held across messages.

```rust
pub(crate) fn insert_candles(
    conn: &Connection,
    key: &DataKey,
    candles: &CandleBuffer,
) -> Result<usize, StoreError> {
    if candles.is_empty() {
        return Ok(0);
    }

    let tf_secs = key.timeframe().as_secs() as i32;

    // Delete existing data — Appender does NOT support conflict resolution.
    conn.execute(
        "DELETE FROM market.candles WHERE symbol = ? AND timeframe_secs = ?",
        params![key.symbol(), tf_secs],
    )?;

    // Bulk insert via Appender. Created and dropped within this call
    // because Appender is !Send + !Sync.
    {
        let mut appender = conn.appender("market.candles")?;
        for i in 0..candles.len() {
            appender.append_row(params![
                key.symbol(),
                tf_secs,
                candles.timestamps[i],
                candles.opens[i],   // f32 -> FLOAT directly
                candles.highs[i],
                candles.lows[i],
                candles.closes[i],
                candles.volumes[i], // u32 -> UINTEGER
            ])?;
        }
        appender.flush()?;
    }

    Ok(candles.len())
}
```

### 6.4 Appender vs INSERT OR REPLACE

The Appender does not support `INSERT OR REPLACE` conflict resolution.
DuckDB's Appender appends rows directly to the table's column segments,
bypassing the conflict detection logic.

For **upsert** operations, we fall back to row-by-row `INSERT OR REPLACE`:

```rust
pub(crate) fn upsert_candles(
    conn: &Connection,
    key: &DataKey,
    candles: &CandleBuffer,
) -> Result<usize, StoreError> {
    if candles.is_empty() {
        return Ok(0);
    }

    let tf_secs = key.timeframe().as_secs() as i32;

    let mut stmt = conn.prepare(
        "INSERT OR REPLACE INTO market.candles
             (symbol, timeframe_secs, timestamp_ms, open, high, low, close, volume)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
    )?;

    for i in 0..candles.len() {
        stmt.execute(params![
            key.symbol(),
            tf_secs,
            candles.timestamps[i],
            candles.opens[i],   // f32 directly
            candles.highs[i],
            candles.lows[i],
            candles.closes[i],
            candles.volumes[i],
        ])?;
    }

    Ok(candles.len())
}
```

### 6.5 When to use Insert vs Upsert

| Scenario | Method | Why |
|----------|--------|-----|
| First CSV import | `Insert` | No existing data, Appender is fast |
| Re-import same CSV | `Insert` | Duplicates silently skipped by PK constraint |
| Import with corrected data | `Upsert` | Existing rows need to be replaced |
| Append new data (later dates) | `Insert` | No overlap, Appender is fast |
| Backfill gaps in existing data | `Upsert` | Some rows may overlap boundary candles |

---

## 7. Error Propagation

### 7.1 Reply channel path (send)

When the caller uses `DbHandle::insert()` (await-for-reply):

```rust
// In DbHandle:
pub async fn insert(&self, key: &DataKey, candles: &CandleBuffer) -> Result<u64, StoreError> {
    let reply = self.inner.send(DbCommand::Insert {
        key: key.clone(),
        candles: candles.clone(),
    }).await.map_err(|_| StoreError::ActorShutdown)?;

    match reply {
        DbReply::Inserted(result) => result,
        _ => Err(StoreError::NoReply), // Protocol violation -- should never happen
    }
}
```

Error flow:

```
DuckDB error (in queries.rs)
    |
    v
Result<u64, StoreError> (in handler)
    |
    v
DbReply::Inserted(Err(StoreError::DuckDb(...)))
    |
    v
reply_sync(Some(tx), DbReply::Inserted(Err(...)))
    |   (blocking_send on capacity-1 channel)
    v
DbHandle::insert() receives DbReply::Inserted(Err(...))
    |
    v
Returns Err(StoreError::DuckDb(...)) to caller
```

### 7.2 Fire-and-forget path

When the caller uses `DbHandle::insert_fire_and_forget()`:

```rust
// In DbHandle:
pub fn insert_fire_and_forget(&self, key: DataKey, candles: CandleBuffer) {
    let inner = self.inner.clone();
    tokio::spawn(async move {
        let _ = inner.fire_and_forget(DbCommand::Insert { key, candles }).await;
    });
}
```

Error flow:

```
DuckDB error (in queries.rs)
    |
    v
Result<u64, StoreError> (in handler)
    |
    v
reply_tx is None (fire_and_forget)
    |
    v
Handler logs via tracing::warn!
    |
    v
Error is silently swallowed (no caller to report to)
```

The `tracing::warn!` in the handler ensures that fire-and-forget errors are
visible in the application log, even though no caller is waiting for them.

### 7.3 Channel closed (actor shutdown)

If the actor thread has exited (or the channel is full and the sender is
dropped), the `send()` call on the mpsc channel returns an error:

```rust
// In DbHandle:
let reply = self.inner.send(cmd).await
    .map_err(|_| StoreError::ActorShutdown)?;
//             ^^ MailboxProcessorError -> StoreError::ActorShutdown
```

The caller receives `StoreError::ActorShutdown` and can decide how to handle
it (typically by logging a warning and continuing without persistence).

### 7.4 Error type mapping

| Source | StoreError variant | When |
|--------|-------------------|------|
| `duckdb::Error` | `DuckDb(e)` | Query failure, constraint violation, etc. |
| `duckdb::Error` during open | `OpenFailed { path, source }` | File permissions, corrupt DB, etc. |
| Migration SQL failure | `MigrationFailed { version, message }` | Schema upgrade error |
| Channel closed | `ActorShutdown` | Actor thread exited |
| No reply received | `NoReply` | Bug: handler didn't send a reply |
| Invalid time range | `InvalidTimeRange { start, end }` | Caller passed start >= end |
| `std::io::Error` | `Io(e)` | Directory creation, temp file errors |
| Row conversion | `Conversion(msg)` | Data integrity issue |

---

## 8. Backpressure

### 8.1 Bounded channel

The mpsc channel has a bounded capacity (default: 256 messages). This provides
natural backpressure:

```rust
MailboxProcessor::new_blocking(
    BufferSize::Size(256),  // <-- bounded capacity
    // ...
);
```

### 8.2 Behavior when full

| Method | Channel full behavior |
|--------|---------------------|
| `DbHandle::insert()` (send) | `send().await` suspends the caller's async task until space is available. The caller's `.await` point yields to the tokio runtime. No thread is blocked. |
| `DbHandle::insert_fire_and_forget()` | `fire_and_forget().await` also suspends. Since this is called from a spawned task, the spawned task suspends but the caller has already returned. |
| `DbHandle::query()` (send) | Same as `insert()` -- suspends until space. The UI may show a loading state while waiting. |

### 8.3 Capacity sizing rationale

256 messages is generous for the expected workload:

- A typical session imports 1-5 CSV files (1-5 Insert messages).
- Chart startup queries 1 dataset per chart pane (typically 1-6 Query messages).
- Background operations (vacuum, checkpoint) are rare (1 per session).

Even under heavy load (importing 20 CSV files simultaneously), the queue has
ample capacity. The bounded channel is primarily a safety valve against
unbounded memory growth, not a real backpressure concern.

### 8.4 Monitoring

The channel does not expose its current fill level. If monitoring is needed
in the future, the actor handler can track throughput:

```rust
// Future enhancement: track message processing rate
let start = std::time::Instant::now();
// ... process message ...
let elapsed = start.elapsed();
tracing::trace!(
    command = %cmd_name,
    elapsed_us = elapsed.as_micros(),
    "message processed"
);
```

---

## 9. Graceful Shutdown

### 9.1 Explicit shutdown (preferred)

The recommended shutdown sequence:

```rust
// In midas-app, during application close:
async fn shutdown(&mut self) {
    if let Some(ref db) = self.db_handle {
        match db.shutdown().await {
            Ok(()) => tracing::info!("store: graceful shutdown complete"),
            Err(e) => tracing::warn!(error = %e, "store: shutdown error"),
        }
    }
}
```

What happens inside:

```
1. DbHandle::shutdown() sends DbCommand::Shutdown with reply channel
2. Actor handler receives Shutdown
3. Handler calls CHECKPOINT on the connection
4. Handler drops the Connection (closes the DuckDB database)
5. Handler sets shutting_down = true
6. Handler sends DbReply::Done(Ok(())) through reply channel
7. DbHandle::shutdown() receives Ok(())
8. Application drops the DbHandle
9. All senders are dropped
10. blocking_recv() returns None
11. Thread exits
```

### 9.2 Drop without explicit shutdown (fallback)

If the application crashes or the `DbHandle` is dropped without calling
`shutdown()`:

```
1. All DbHandle clones are dropped
2. All Sender clones are dropped
3. blocking_recv() returns None
4. Thread exits
5. DbActorState is dropped (runs destructors)
6. Connection is dropped (DuckDB Close())
7. DuckDB performs automatic WAL checkpoint on close
```

DuckDB's internal shutdown logic includes a WAL checkpoint, so data loss is
unlikely even without explicit shutdown. However, explicit shutdown is preferred
because:

- It confirms the checkpoint succeeded (error handling).
- It produces a log entry for operational visibility.
- It ensures the WAL file is cleaned up (no stale `.wal` file on next startup).

### 9.3 Post-shutdown behavior

After `Shutdown` is processed, the actor rejects all subsequent messages:

```rust
if state.shutting_down {
    reply_sync(reply_tx, DbReply::Done(Err(StoreError::ActorShutdown)));
    return state;
}
```

This means if other parts of the application send messages after shutdown
(e.g., a background task trying to persist data), they receive
`StoreError::ActorShutdown` immediately rather than hanging.

### 9.4 Thread join

The `MailboxProcessor` does not store the `JoinHandle`, so there is no way to
`join()` the thread and wait for it to exit. This is by design -- the channel
close mechanism is sufficient for the shutdown sequence:

1. `shutdown()` awaits the `Shutdown` reply (confirms checkpoint completed).
2. After the reply, the connection is already closed.
3. The thread will exit on the next `blocking_recv()` call (returns `None`).
4. Thread exit happens asynchronously but harmlessly -- there are no resources
   left to clean up.

If deterministic thread join is ever needed (e.g., for test teardown), the
`new_blocking()` constructor can be extended to return a `(MailboxProcessor, JoinHandle)` pair. This is not needed for the initial implementation.

---

## 10. Future: Read Pool

### 10.1 Problem statement

The single-threaded actor serializes all operations. For a single chart
loading one dataset, this is fine. But consider the startup scenario for a
20-chart workspace:

```
Chart 1: Query(AAPL, D1) -> ~10ms
Chart 2: Query(MSFT, D1) -> ~10ms
...
Chart 20: Query(TSLA, H4) -> ~10ms

Total: ~200ms serial, but could be ~10ms parallel
```

### 10.2 DuckDB's `try_clone()` solution

`duckdb::Connection::try_clone()` creates a new connection handle to the same
database file. Multiple cloned connections can execute queries concurrently
because DuckDB's engine is internally thread-safe for reads.

```rust
let conn = Connection::open("candles.duckdb")?;
let conn2 = conn.try_clone()?;  // Second handle, same DB
// conn and conn2 can query concurrently from different threads
```

### 10.3 Read pool architecture

```
                    DbHandle
                       |
                   [mpsc channel]
                       |
              DbActor (write thread)
                  |          |
            [writes]    [read dispatch]
                  |          |
                  |     +----+----+----+
                  |     |    |    |    |
                  |   conn1 conn2 ... connN  (cloned connections)
                  |     |    |    |    |
                  |     +----+----+----+
                  |          |
                  |    [tokio::spawn_blocking pool]
                  |    [Semaphore(8) limits concurrency]
                  |          |
                  +----------+
                       |
                  [reply channels]
```

### 10.4 Implementation sketch

```rust
use tokio::sync::Semaphore;
use std::sync::Arc;

pub(crate) struct DbActorState {
    conn: Option<duckdb::Connection>,
    config: StoreConfig,
    shutting_down: bool,
    // Future: read pool
    read_semaphore: Arc<Semaphore>,
    read_connections: Vec<duckdb::Connection>,
}

impl DbActorState {
    fn init_read_pool(&mut self) -> Result<(), StoreError> {
        let conn = self.conn.as_ref().ok_or(StoreError::ActorShutdown)?;
        let pool_size = 8; // Configurable

        for _ in 0..pool_size {
            let cloned = conn.try_clone()
                .map_err(StoreError::DuckDb)?;
            self.read_connections.push(cloned);
        }

        self.read_semaphore = Arc::new(Semaphore::new(pool_size));
        Ok(())
    }
}
```

For read queries, the actor would dispatch to `spawn_blocking` with a cloned
connection and a semaphore permit:

```rust
DbCommand::Query { key, range } => {
    if let Some(conn) = state.read_connections.pop() {
        let sem = state.read_semaphore.clone();
        let reply_tx = reply_tx; // Move into spawned task

        // Dispatch to blocking threadpool
        tokio::task::spawn_blocking(move || {
            let _permit = sem.blocking_acquire().unwrap();
            let result = crate::queries::query_candles(&conn, &key, range);
            if let Some(tx) = reply_tx {
                let _ = tx.blocking_send(DbReply::Candles(result));
            }
            // Return connection to pool... (needs channel back to actor)
        });
    }
}
```

### 10.5 Why defer the read pool

The read pool adds significant complexity:

1. **Connection return.** Read connections must be returned to the pool after
   each query. This requires a separate mpsc channel from the blocking tasks
   back to the actor, or an `Arc<Mutex<Vec<Connection>>>` pool.
2. **Mixed reads/writes.** Writes must still be serialized on the main actor
   thread. The actor must distinguish between read and write commands.
3. **Error handling.** A failed read connection (e.g., corrupted clone) must
   be replaced, not returned to the pool.
4. **Testing.** The pool adds concurrent behavior that is harder to test
   deterministically.

For the initial implementation, the serial actor is sufficient. The 200ms
startup for 20 charts is acceptable. The read pool can be added later without
changing the public `DbHandle` API -- it is purely an internal optimization.

### 10.6 API stability

The `DbHandle` public API is designed so that the read pool can be added
without breaking changes:

```rust
// This API works with both serial actor and read pool:
let buf = handle.query(&key, None).await?;
```

The caller does not know or care whether the query ran on the actor thread
or on a pooled connection. The `Result<CandleBuffer, StoreError>` contract
is the same either way.

---

## Appendix A: Complete DbHandle Implementation

This is the full implementation of all `DbHandle` methods, ready for
copy-paste into `handle.rs`.

```rust
use mailbox_processor::{BufferSize, MailboxProcessor};
use midas_data::CandleBuffer;

use crate::actor::{handle_db_command, DbActorState, DbCommand, DbReply};
use crate::types::{CacheInfo, DataKey, StoreConfig, TimeRange};
use crate::StoreError;

/// Async handle to the DuckDB store actor.
///
/// Cheap to clone (clones the underlying mpsc sender).
/// Dropping all clones causes the actor thread to exit gracefully.
#[derive(Clone)]
pub struct DbHandle {
    inner: MailboxProcessor<DbCommand, DbReply>,
}

impl DbHandle {
    /// Open (or create) a DuckDB store with the given configuration.
    ///
    /// Synchronous constructor. Spawns the actor thread and returns immediately.
    /// Connection opens lazily on the first command.
    /// See 04-dbhandle-api.md Section 3.3 for canonical implementation.
    pub fn open(config: StoreConfig) -> Self {
        let channel_capacity = config.channel_capacity;

        let mailbox = MailboxProcessor::<DbCommand, DbReply>::new_blocking(
            BufferSize::Size(channel_capacity),
            DbActorState::new(config),
            "midas-store-db",
            handle_db_command,
        );

        DbHandle { inner: mailbox }
    }

    /// Insert candles (bulk append, duplicates skipped).
    pub async fn insert(
        &self,
        key: &DataKey,
        candles: &CandleBuffer,
    ) -> Result<u64, StoreError> {
        let reply = self
            .inner
            .send(DbCommand::Insert {
                key: key.clone(),
                candles: candles.clone(),
            })
            .await
            .map_err(|_| StoreError::ActorShutdown)?;

        match reply {
            DbReply::Inserted(result) => result,
            other => Err(StoreError::NoReply),
        }
    }

    /// Insert candles without waiting for completion.
    ///
    /// Errors are logged via `tracing::warn!` on the actor thread.
    pub fn insert_fire_and_forget(&self, key: DataKey, candles: CandleBuffer) {
        let inner = self.inner.clone();
        tokio::spawn(async move {
            let _ = inner
                .fire_and_forget(DbCommand::Insert { key, candles })
                .await;
        });
    }

    /// Upsert candles (insert or replace on primary key conflict).
    pub async fn upsert(
        &self,
        key: &DataKey,
        candles: &CandleBuffer,
    ) -> Result<u64, StoreError> {
        let reply = self
            .inner
            .send(DbCommand::Upsert {
                key: key.clone(),
                candles: candles.clone(),
            })
            .await
            .map_err(|_| StoreError::ActorShutdown)?;

        match reply {
            DbReply::Inserted(result) => result,
            _ => Err(StoreError::NoReply),
        }
    }

    /// Query candles. Pass `None` for full dataset, `Some(range)` for
    /// time-windowed query.
    pub async fn query(
        &self,
        key: &DataKey,
        range: Option<TimeRange>,
    ) -> Result<CandleBuffer, StoreError> {
        let reply = self
            .inner
            .send(DbCommand::Query {
                key: key.clone(),
                range,
            })
            .await
            .map_err(|_| StoreError::ActorShutdown)?;

        match reply {
            DbReply::Candles(result) => result,
            _ => Err(StoreError::NoReply),
        }
    }

    /// List all cached datasets with metadata.
    pub async fn list_cached(&self) -> Result<Vec<CacheInfo>, StoreError> {
        let reply = self
            .inner
            .send(DbCommand::ListCached)
            .await
            .map_err(|_| StoreError::ActorShutdown)?;

        match reply {
            DbReply::CacheList(result) => result,
            _ => Err(StoreError::NoReply),
        }
    }

    /// Delete candles for a key, optionally within a time range.
    pub async fn delete(
        &self,
        key: &DataKey,
        range: Option<TimeRange>,
    ) -> Result<u64, StoreError> {
        let reply = self
            .inner
            .send(DbCommand::Delete {
                key: key.clone(),
                range,
            })
            .await
            .map_err(|_| StoreError::ActorShutdown)?;

        match reply {
            DbReply::Deleted(result) => result,
            _ => Err(StoreError::NoReply),
        }
    }

    /// Force a WAL checkpoint.
    pub async fn checkpoint(&self) -> Result<(), StoreError> {
        let reply = self
            .inner
            .send(DbCommand::Checkpoint)
            .await
            .map_err(|_| StoreError::ActorShutdown)?;

        match reply {
            DbReply::Done(result) => result,
            _ => Err(StoreError::NoReply),
        }
    }

    /// Vacuum the database to reclaim disk space.
    pub async fn vacuum(&self) -> Result<(), StoreError> {
        let reply = self
            .inner
            .send(DbCommand::Vacuum)
            .await
            .map_err(|_| StoreError::ActorShutdown)?;

        match reply {
            DbReply::Done(result) => result,
            _ => Err(StoreError::NoReply),
        }
    }

    /// Graceful shutdown: checkpoint WAL and close connection.
    pub async fn shutdown(&self) -> Result<(), StoreError> {
        let reply = self
            .inner
            .send(DbCommand::Shutdown)
            .await
            .map_err(|_| StoreError::ActorShutdown)?;

        match reply {
            DbReply::Done(result) => result,
            _ => Err(StoreError::NoReply),
        }
    }
}
```

---

## Appendix B: Thread Safety Analysis

### Why DbHandle is Send + Sync + Clone

```rust
// DbHandle contains:
struct DbHandle {
    inner: MailboxProcessor<DbCommand, DbReply>,
}

// MailboxProcessor contains:
struct MailboxProcessor<Msg, ReplyMsg> {
    message_sender: tokio::sync::mpsc::Sender<(Msg, Option<Sender<ReplyMsg>>)>,
}

// tokio::sync::mpsc::Sender<T> is Send + Sync + Clone when T: Send.
// (Msg, Option<Sender<ReplyMsg>>): Send when Msg: Send and ReplyMsg: Send.
// DbCommand: Send (all fields are owned, sendable types).
// DbReply: Send (CandleBuffer: Send, Vec<CacheInfo>: Send, StoreError: Send).
//
// Therefore: DbHandle is Send + Sync + Clone.   QED.
```

### Why DbActorState is NOT Sync (and doesn't need to be)

```rust
// DbActorState contains:
struct DbActorState {
    conn: Option<duckdb::Connection>,  // Send + !Sync
    config: StoreConfig,               // Send + Sync
    shutting_down: bool,               // Send + Sync
}

// duckdb::Connection is Send + !Sync.
// Therefore DbActorState is Send + !Sync.
//
// This is fine because DbActorState is:
//   1. Moved into the thread closure (Send required, satisfied).
//   2. Owned exclusively by the thread (Sync not required).
//   3. Never shared by reference across threads.
```

### Why Appender works despite being !Send + !Sync

```rust
// The Appender is created inside insert_candles(), which runs on the
// actor thread. The Appender borrows the Connection (&conn), does its
// work, and is dropped -- all within a single function call on a single
// thread. It never needs to be Send or Sync.
//
// fn insert_candles(conn: &Connection, ...) {
//     let mut appender = conn.appender("candles")?;  // !Send + !Sync
//     // ... append rows ...
//     appender.flush()?;
//     // appender dropped here, on the same thread it was created
// }
```

---

## Appendix C: Testing the Actor

### Unit testing the handler

The handler function is a pure function with signature
`(DbCommand, DbActorState, Option<Sender<DbReply>>) -> DbActorState`.
It can be tested directly without spawning threads:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    fn test_config(path: &std::path::Path) -> StoreConfig {
        StoreConfig {
            db_path: path.join("test.duckdb"),
            memory_limit_bytes: 64 * 1024 * 1024,
            threads: 1,
            temp_directory: None,
            channel_capacity: 16,
        }
    }

    #[test]
    fn handler_insert_and_query() {
        let dir = tempfile::tempdir().unwrap();
        let config = test_config(dir.path());
        let mut state = DbActorState::new(config);

        // Insert
        let mut buf = CandleBuffer::new();
        buf.push(1000, 100.0, 105.0, 95.0, 101.0, 1000);
        buf.push(2000, 101.0, 106.0, 96.0, 102.0, 2000);

        let key = DataKey::new("AAPL", Timeframe::D1);

        let (tx, mut rx) = mpsc::channel(1);
        state = handle_db_command(
            DbCommand::Insert { key: key.clone(), candles: buf },
            state,
            Some(tx),
        );

        let reply = rx.blocking_recv().unwrap();
        match reply {
            DbReply::Inserted(Ok(n)) => assert_eq!(n, 2),
            other => panic!("unexpected reply: {other:?}"),
        }

        // Query
        let (tx, mut rx) = mpsc::channel(1);
        state = handle_db_command(
            DbCommand::Query { key: key.clone(), range: None },
            state,
            Some(tx),
        );

        let reply = rx.blocking_recv().unwrap();
        match reply {
            DbReply::Candles(Ok(buf)) => {
                assert_eq!(buf.len(), 2);
                assert_eq!(buf.timestamps[0], 1000);
                assert_eq!(buf.timestamps[1], 2000);
                assert_eq!(buf.opens[0], 100.0);
            }
            other => panic!("unexpected reply: {other:?}"),
        }
    }

    #[test]
    fn handler_shutdown_rejects_subsequent() {
        let dir = tempfile::tempdir().unwrap();
        let config = test_config(dir.path());
        let mut state = DbActorState::new(config);

        // Shutdown
        let (tx, mut rx) = mpsc::channel(1);
        state = handle_db_command(DbCommand::Shutdown, state, Some(tx));
        let reply = rx.blocking_recv().unwrap();
        assert!(matches!(reply, DbReply::Done(Ok(()))));

        // Subsequent command should be rejected
        let (tx, mut rx) = mpsc::channel(1);
        state = handle_db_command(DbCommand::ListCached, state, Some(tx));
        let reply = rx.blocking_recv().unwrap();
        assert!(matches!(reply, DbReply::Done(Err(StoreError::ActorShutdown))));
    }

    #[test]
    fn handler_fire_and_forget_logs_error() {
        // Pass None as reply channel, verify no panic
        let dir = tempfile::tempdir().unwrap();
        let config = test_config(dir.path());
        let state = DbActorState::new(config);

        let buf = CandleBuffer::new();
        let key = DataKey::new("TEST", Timeframe::M1);

        // This should not panic even with empty buffer
        let _state = handle_db_command(
            DbCommand::Insert { key, candles: buf },
            state,
            None, // fire and forget
        );
    }
}
```

### Integration testing with DbHandle

```rust
#[cfg(test)]
mod integration_tests {
    use super::*;

    #[tokio::test]
    async fn roundtrip_insert_query() {
        let dir = tempfile::tempdir().unwrap();
        let config = StoreConfig {
            db_path: dir.path().join("test.duckdb"),
            ..StoreConfig::default()
        };

        let handle = DbHandle::open(config);

        // Build test data
        let mut buf = CandleBuffer::new();
        for i in 0..100u64 {
            buf.push(
                (i * 60_000) as i64, // 1-minute intervals
                100.0 + i as f32,
                105.0 + i as f32,
                95.0 + i as f32,
                101.0 + i as f32,
                (1000 + i) as u32,
            );
        }

        let key = DataKey::new("AAPL", Timeframe::M1);

        // Insert
        let inserted = handle.insert(&key, &buf).await.unwrap();
        assert_eq!(inserted, 100);

        // Query all
        let result = handle.query(&key, None).await.unwrap();
        assert_eq!(result.len(), 100);
        assert_eq!(result.timestamps[0], 0);
        assert_eq!(result.timestamps[99], 99 * 60_000);

        // Query range
        let range = TimeRange::new(10 * 60_000, 20 * 60_000).unwrap();
        let result = handle.query(&key, Some(range)).await.unwrap();
        assert_eq!(result.len(), 10); // [10, 20) = 10 candles

        // List cached
        let cached = handle.list_cached().await.unwrap();
        assert_eq!(cached.len(), 1);
        assert_eq!(cached[0].key.symbol(), "AAPL");
        assert_eq!(cached[0].count, 100);

        // Shutdown
        handle.shutdown().await.unwrap();
    }
}
```

---

## Appendix D: Performance Expectations

| Operation | Expected latency | Notes |
|-----------|-----------------|-------|
| `DbHandle::open()` | <1ms | Thread spawn only, lazy connection |
| First message (connection init) | 10-30ms | DuckDB C++ init + migration |
| `insert()` 10K candles | 5-15ms | Appender bulk path |
| `insert()` 100K candles | 30-80ms | Appender bulk path |
| `query()` 10K candles | 3-8ms | Columnar scan + SoA conversion |
| `query()` 100K candles | 15-40ms | Columnar scan + SoA conversion |
| `query()` with time range (1K of 100K) | 1-3ms | Index-accelerated scan |
| `list_cached()` (10 datasets) | <1ms | Metadata aggregation |
| `upsert()` 1K candles | 20-50ms | Row-by-row INSERT OR REPLACE |
| `checkpoint()` | 5-50ms | Depends on WAL size |
| `vacuum()` | 50-500ms | Depends on DB size |
| `shutdown()` | 10-60ms | Checkpoint + close |

These are estimates for a modern NVMe SSD (Samsung 990 Pro class). The primary
optimization target is `query()` latency, since it is on the startup critical
path. Insert latency is less critical because inserts are typically
fire-and-forget.
