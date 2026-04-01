# Concurrency Architecture: MailboxProcessor Pattern

## Why a Mailbox Processor?

DuckDB's `Connection` is `Send` but `!Sync` (wraps `RefCell`). While `Arc<Mutex<Connection>>` does compile and work (the broker crate uses this pattern for rusqlite), a channel-based actor is preferred because:
1. It avoids mutex contention when 20+ charts query concurrently
2. It keeps blocking C++ FFI calls off tokio's threadpool
3. `Appender` is `!Send` and must live on the same thread as `Connection`
4. The mailbox naturally enables write batching and clean async API

## Implementation: `mailbox_processor` Crate

We use our own `mailbox_processor` crate (from `ControlPlugin/Shared/mailbox_processor`), which provides a typed actor abstraction over tokio channels. Copy it into the workspace as a local dependency.

**Setup:** Copy `mailbox_processor/` to `desktop/win/crates/mailbox_processor/` and add it to the workspace members in `desktop/win/Cargo.toml`.

The crate provides two constructors:
- `MailboxProcessor::new()` — async handler on a tokio task (for I/O-bound work)
- `MailboxProcessor::new_blocking()` — sync handler on a dedicated OS thread (for blocking FFI)

**We use `new_blocking()`** because DuckDB operations are synchronous C++ FFI calls.

```
                        +----------------------------+
  iced UI thread ------>|                            |
                        |  MailboxProcessor          |---> std::thread "duckdb-store"
  tokio task A -------->|   .send() / .fire_and_forget()|  owns Connection
                        |                            |     processes sequentially
  tokio task B -------->|                            |     replies via channel
                        +----------------------------+
```

## Message Types

```rust
/// Commands sent to the DuckDB actor.
enum DbCommand {
    InsertCandles { key: DataKey, buffer: CandleBuffer },
    QueryCandles { key: DataKey, time_range: Option<(i64, i64)> },
    ListCached,
}

/// Replies from the DuckDB actor.
enum DbReply {
    Inserted(Result<usize, StoreError>),
    Candles(Result<CandleBuffer, StoreError>),
    CacheList(Result<Vec<CacheInfo>, StoreError>),
}
```

No manual oneshot channels — `MailboxProcessor` handles reply wiring internally.

## DbHandle (the public API)

See [05-integration.md](05-integration.md) for the canonical API surface. Internally, `DbHandle` wraps a `MailboxProcessor`:

```rust
pub struct DbHandle {
    mb: MailboxProcessor<DbCommand, DbReply>,
}

impl DbHandle {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let path = path.as_ref().to_owned();
        let mb = MailboxProcessor::new_blocking(
            BufferSize::Size(256),
            None, // state: Option<Connection>, initialized in handler
            "duckdb-store",
            move |cmd, conn_state, reply_channel| {
                // Lazily open connection on first message
                let conn = conn_state.unwrap_or_else(|| {
                    let c = Connection::open(&path).expect("DuckDB open");
                    run_migrations(&c).expect("DuckDB migrations");
                    c
                });

                match cmd {
                    DbCommand::InsertCandles { key, buffer } => {
                        let result = bulk_insert(&conn, &key, &buffer);
                        if let Some(ch) = reply_channel {
                            ch.blocking_send(DbReply::Inserted(result)).ok();
                        }
                    }
                    DbCommand::QueryCandles { key, time_range } => {
                        let result = query_candles_impl(&conn, &key, time_range);
                        if let Some(ch) = reply_channel {
                            ch.blocking_send(DbReply::Candles(result)).ok();
                        }
                    }
                    DbCommand::ListCached => {
                        let result = list_cached_impl(&conn);
                        if let Some(ch) = reply_channel {
                            ch.blocking_send(DbReply::CacheList(result)).ok();
                        }
                    }
                }
                Some(conn) // return state for next iteration
            },
        );
        Ok(DbHandle { mb })
    }

    pub async fn insert_candles(&self, key: DataKey, buf: CandleBuffer)
        -> Result<usize, StoreError>
    {
        match self.mb.send(DbCommand::InsertCandles { key, buffer: buf }).await
            .map_err(|e| StoreError::ActorDead(e.to_string()))?
        {
            DbReply::Inserted(r) => r,
            _ => unreachable!(),
        }
    }

    pub async fn query_candles(&self, key: DataKey)
        -> Result<CandleBuffer, StoreError>
    {
        match self.mb.send(DbCommand::QueryCandles { key, time_range: None }).await
            .map_err(|e| StoreError::ActorDead(e.to_string()))?
        {
            DbReply::Candles(r) => r,
            _ => unreachable!(),
        }
    }
}
```

### Why new_blocking() (not new())

DuckDB operations are synchronous C++ FFI calls (milliseconds to tens of milliseconds). `new_blocking()` runs the handler on a dedicated OS thread:
- Never competes with tokio's async worker pool
- Connection created on-thread, never moves
- Lives for app lifetime (no repeated spawn overhead)
- Named thread visible in debuggers: `"duckdb-store"`

### What MailboxProcessor gives us over hand-rolling

| Concern | Hand-rolled mpsc+oneshot | MailboxProcessor |
|---------|------------------------|------------------|
| Reply plumbing | Manual oneshot per call | Built-in via `send()` |
| Fire-and-forget | Manual `try_send` | Built-in `fire_and_forget()` |
| State threading | Manual `let mut state` in loop | `(Msg, State) -> State` functional pattern |
| Shutdown | Manual drop-sender dance | Implicit on drop |
| Boilerplate per query | Enum variant + handler arm + oneshot wiring | Enum variant + handler arm (no wiring) |

## Iced Integration

### Firing queries from update()

```rust
// In MidasApp::update() -- extends the existing PanelSymbolSubmitted handler:
// (DataCacheMiss, DataLoadFailed are new Message variants to add)
Message::PanelSymbolSubmitted(id) => {
    let db = self.db_handle.clone();
    let key = DataKey {
        symbol: self.charts[&id].symbol.clone(),
        timeframe: self.charts[&id].timeframe,
    };

    Task::perform(
        async move { db.query_candles(key).await },
        move |result| match result {
            Ok(buf) if !buf.is_empty() => Message::DataLoaded(id, Ok(Arc::new(buf))),
            _ => Message::DataCacheMiss(id),
        },
    )
}
```

### Batching 20+ chart loads

```rust
let tasks: Vec<Task<Message>> = chart_ids.iter().map(|&id| {
    let db = self.db_handle.clone();
    let key = DataKey {
        symbol: self.charts[&id].symbol.clone(),
        timeframe: self.charts[&id].timeframe,
    };
    Task::perform(
        async move { db.query_candles(key).await },
        move |r| match r {
            Ok(c) if !c.is_empty() => Message::DataLoaded(id, Ok(Arc::new(c))),
            _ => Message::DataCacheMiss(id),
        },
    )
}).collect();
Task::batch(tasks)
```

## Error Handling and Backpressure

| Concern | Approach |
|---------|----------|
| Mailbox full | `send().await` suspends caller (bounded channel, 256) |
| Fire-and-forget | `fire_and_forget()` drops silently if full |
| Query timeout | `tokio::time::timeout(Duration::from_secs(5), mb.send(...))` |
| Actor thread panic | `send()` returns `MailboxProcessorError` (channel closed) |
| Graceful shutdown | Drop `DbHandle` -> all senders drop -> thread exits |

## v1 vs Future Read Pool

**v1:** Both reads and writes go through the single `MailboxProcessor` actor. Queries are infrequent (on load/symbol change, not per-frame) and fast (~5ms each).

**Future optimization:** If profiling shows 20+ simultaneous chart loads bottlenecking, add a read pool: parallel `spawn_blocking` tasks with `Connection::try_clone()`, gated by `Semaphore(8)`. The `DbHandle` public API stays the same — only the internal dispatch changes.

## Sources

- [Actors with Tokio -- Alice Ryhl](https://ryhl.io/blog/actors-with-tokio/)
- [Tokio Tutorial: Channels](https://tokio.rs/tokio/tutorial/channels)
- [duckdb-rs threading issue #378](https://github.com/duckdb/duckdb-rs/issues/378)
- Internal: `mailbox_processor` crate (from ControlPlugin/Shared)
