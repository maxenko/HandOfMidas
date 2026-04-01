# Integration Architecture

## Crate Placement: New `midas-store`

### Decision

Create a new `midas-store` crate. Do not add DuckDB to `midas-data` or `midas-feed`.

### Rationale

- `midas-data` is lightweight (bytemuck, memmap2, chrono). Adding DuckDB's C++ build dependency would leak compile cost to `midas-chart`, `midas-feed`, and `midas-render` transitively.
- `midas-feed` handles data *ingestion* (CSV, IB). Persistence is a separate concern.
- A dedicated crate provides clean separation and allows feature-gating DuckDB out entirely.

### Updated Dependency Graph

```
midas-core                       (leaf: types, traits, IDs)
     |
midas-data                       (SoA buffers, binary format, LOD)
     |
+----+----+----------+
|         |          |
midas-chart  midas-store  midas-indicators  (chart logic / DuckDB / indicators)
|         |         |
|    midas-feed     |     (CSV import, IB streaming -- NO midas-store dep)
|         |         |
midas-render  midas-ui    (GPU pipelines / UI widgets)
|         |         |
+----+----+---------+
     |
midas-app                       (iced shell, ties everything together)
```

Note: `midas-feed` does NOT depend on `midas-store`. Write-through to DuckDB happens at the `MidasApp` level after receiving a `CandleBuffer` from the feed, preserving crate independence.

### Setup

1. Copy `D:\GitHub\ControlPlugin\Shared\mailbox_processor\` to `desktop/win/crates/mailbox_processor/`
2. Add `new_blocking()` constructor to the local copy (see [02-architecture.md](02-architecture.md) for why). This runs the handler on a dedicated `std::thread` instead of a tokio task — required for DuckDB's blocking C++ FFI:
   ```rust
   /// Sync handler on a dedicated OS thread. For blocking FFI / !Sync resources.
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
3. Add `"crates/mailbox_processor"` to `[workspace] members` in `desktop/win/Cargo.toml`
4. Create `desktop/win/crates/midas-store/`

### Cargo.toml

```toml
[package]
name = "midas-store"
version = "0.1.0"
edition = "2021"
publish = false

[dependencies]
midas-core         = { path = "../midas-core" }
midas-data         = { path = "../midas-data" }
mailbox_processor  = { path = "../mailbox_processor" }
duckdb             = { version = "1", features = ["bundled"] }
tokio              = { workspace = true }
thiserror          = { workspace = true }
tracing            = { workspace = true }
chrono             = { workspace = true }
```

## DbHandle API

### Public Types

```rust
/// Opaque handle to the DuckDB actor.
/// Internally wraps a MailboxProcessor<DbCommand, DbReply> via new_blocking().
/// Clone is cheap (clones the internal channel sender).
pub struct DbHandle {
    mb: MailboxProcessor<DbCommand, DbReply>,
}

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
}
```

### Async Methods

```rust
impl DbHandle {
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, StoreError>;
    pub async fn open_memory() -> Result<Self, StoreError>;  // for tests
    pub async fn insert_candles(&self, key: DataKey, buffer: CandleBuffer) -> Result<usize, StoreError>;
    pub async fn query_candles(&self, key: DataKey) -> Result<CandleBuffer, StoreError>;
    pub async fn query_candles_range(&self, key: DataKey, start: i64, end: i64) -> Result<CandleBuffer, StoreError>;
    pub async fn list_cached(&self) -> Result<Vec<CacheInfo>, StoreError>;
    pub async fn shutdown(&self) -> Result<(), StoreError>;
}
```

No blocking variant needed. The codebase is fully async (iced + tokio). If blocking access is ever needed, `tokio::task::block_in_place` handles it.

**v1 simplification:** This `DbHandle` routes both reads and writes through a single actor thread. The architecture doc (02-architecture.md) describes a "write actor + read pool" hybrid for parallel reads. For v1, the single-actor model is sufficient -- queries are infrequent (on load/symbol change, not per-frame) and fast (~5ms each). If profiling shows serialized reads bottlenecking 20+ simultaneous chart loads, add a read pool: parallel `spawn_blocking` tasks with `Connection::try_clone()`, gated by `Semaphore(8)`.

## Data Flow: Write-Behind Cache

### Why write-behind wins

The GPU rendering path needs contiguous `f32`/`i64` arrays every frame. Querying DuckDB per frame adds 0.1-0.5ms per chart; with 20 charts that's 2-10ms -- eating into the 14ms frame budget.

### Flow Diagram

```
COLD START (cache hit):
  DuckDB --query--> CandleBuffer --Arc--> Charts --> GPU

COLD START (cache miss):
  CSV/TestData --> CandleBuffer --+--Arc--> Charts --> GPU
                                  |
                                  +--async--> DuckDB (write-behind)

FUTURE (IB streaming):
  IB ticks --> aggregate --> CandleBuffer.push() --+--> Charts (live)
                                                   |
                                                   +--> DuckDB (batch flush every 5s)
```

### Startup Sequence

```
T+0ms:    App launches
T+10ms:   Load config.toml
T+50ms:   DbHandle::open("data/cache.duckdb") -- migrations, spawn actor
T+70ms:   For each chart: db.query_candles(key)
T+90ms:   Cache hit: chart.data = Arc::new(result)
          Cache miss: TestDataProvider -> chart.data, fire-and-forget write to DuckDB
T+200ms:  First frame rendered
```

Total DuckDB overhead: ~40ms. Well within 2-second budget.

### Always Materialize to CandleBuffer

DuckDB results should NOT implement `CandleData` directly. Always materialize:

```rust
fn query_to_buffer(conn: &Connection, key: &DataKey) -> Result<CandleBuffer, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT timestamp_ms, open, high, low, close, volume
         FROM market.candles
         WHERE symbol = ? AND timeframe_secs = ?
         ORDER BY timestamp_ms ASC"
    )?;
    let mut buf = CandleBuffer::with_capacity(5000);
    let mut rows = stmt.query(params![key.symbol, key.timeframe.as_secs()])?;
    while let Some(row) = rows.next()? {
        buf.push(row.get(0)?, row.get(1)?, row.get(2)?,
                 row.get(3)?, row.get(4)?, row.get(5)?);
    }
    Ok(buf)
}
```

Reasons:
- `CandleData` requires O(1) indexed access; result sets don't naturally support this
- SoA layout critical for SIMD and GPU upload
- 5000 candles = ~180KB, negligible overhead
- One-time cost at load, not per-frame

## MidasApp Integration

### New Field

```rust
pub struct MidasApp {
    // ... existing fields ...
    store: Option<DbHandle>,   // None if disabled or failed to open
}
```

### Symbol Load Handler

The existing `Message::PanelSymbolSubmitted(ChartId)` handler calls `load_symbol_for_chart()`.
New `Message` variants to add: `DataCacheMiss(ChartId, DataKey)`, `DataLoadFailed(ChartId, String)`.

```rust
// Modified load_symbol_for_chart():
Message::PanelSymbolSubmitted(id) => {
    let symbol = self.charts[&id].symbol.clone();
    if let Some(ref store) = self.store {
        let store = store.clone();
        let key = DataKey { symbol, timeframe: self.charts[&id].timeframe };
        Task::perform(
            async move { store.query_candles(key).await },
            move |result| match result {
                Ok(buf) if !buf.is_empty() => Message::DataLoaded(id, Ok(Arc::new(buf))),
                _ => Message::DataCacheMiss(id, key),
            },
        )
    } else {
        self.load_test_data_for_chart(id, &symbol, self.charts[&id].timeframe, true);
        Task::none()
    }
}
```

## Configuration

### config.toml

```toml
[store]
enabled = true
path = "cache.duckdb"        # relative to data directory
memory_limit_mb = 256
flush_interval_secs = 5       # for streaming data batching
```

Note: Data retention/cleanup is deferred to a future iteration. The database will grow unbounded until a purging mechanism is designed (scheduled cleanup job, SQL, compaction impact analysis). For typical desktop usage with <500 symbols, the database stays well under 1GB for years of daily data.

### Graceful Fallback

When `store.enabled = false` or store fails to open:
- `MidasApp.store = None`
- All data loading falls back to `TestDataProvider`
- No error shown to user (silent fallback with `tracing::warn`)

## Testing Strategy

### Unit Tests (midas-store, sync)

```rust
fn test_conn() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    run_migrations(&conn).unwrap();
    conn
}

#[test]
fn roundtrip() {
    let conn = test_conn();
    let key = DataKey { symbol: "AAPL".into(), timeframe: Timeframe::D1 };
    let mut buf = CandleBuffer::new();
    buf.push(1000, 100.0, 105.0, 95.0, 101.0, 1000);
    bulk_insert(&conn, &key, &buf).unwrap();
    let loaded = query_all(&conn, &key).unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded.timestamps, buf.timestamps);
}
```

### Async Integration Tests

```rust
#[tokio::test]
async fn dbhandle_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let handle = DbHandle::open(dir.path().join("test.duckdb")).await.unwrap();
    let key = DataKey { symbol: "AAPL".into(), timeframe: Timeframe::D1 };
    let mut buf = CandleBuffer::new();
    buf.push(1000, 100.0, 105.0, 95.0, 101.0, 1000);
    handle.insert_candles(key.clone(), buf).await.unwrap();
    let loaded = handle.query_candles(key).await.unwrap();
    assert_eq!(loaded.len(), 1);
    handle.shutdown().await.unwrap();
}
```

### Chart Tests Unaffected

`midas-chart` tests consume `&dyn CandleData`. They never see `DbHandle`. The store is invisible to the chart layer by design.

## Performance Expectations

| Operation | Time |
|-----------|------|
| Open + migrate (cold) | ~50ms |
| Query 5000 candles | ~5ms |
| Bulk insert 5000 candles | ~10ms |
| Materialize to CandleBuffer | ~1ms |
| Full startup overhead (4 charts) | ~40ms |

All well within the 2-second cold start budget. DuckDB is never on the per-frame rendering path.
