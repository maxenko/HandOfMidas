# DuckDB Rust Ecosystem

## The `duckdb` Crate

- **Latest version:** `1.10501.0` (wrapping DuckDB v1.5.1)
- **Repository:** [github.com/duckdb/duckdb-rs](https://github.com/duckdb/duckdb-rs) -- official, maintained by DuckDB team
- **License:** MIT
- **API style:** Mirrors `rusqlite`. If you know rusqlite, you know duckdb-rs.

### Connection Management

```rust
// File-based persistent database
let conn = Connection::open("trading.duckdb")?;

// In-memory (lost on close)
let conn = Connection::open_in_memory()?;

// With configuration
let config = Config::default()?
    .access_mode(AccessMode::ReadWrite)?
    .max_memory("2GB")?
    .threads(4)?;
let conn = Connection::open_with_flags("trading.duckdb", config)?;

// Additional connection to same DB (for read parallelism)
let conn2 = conn.try_clone()?;
```

### Thread Safety

**`Connection` is `Send` but NOT `Sync`.** Critical implications:

- Can **move** a Connection to another thread.
- Cannot share via `&Connection` across threads.
- Inner uses `RefCell`, so `Arc<Connection>` won't compile for shared access.
- `Appender` is **neither Send nor Sync** -- must live on same thread as Connection.
- `InterruptHandle` IS `Send + Sync` (wrapped in Arc) -- enables cross-thread query cancellation.

**Multi-thread patterns:**
1. `try_clone()` per thread -- create a new connection from the original
2. `Mutex<Connection>` -- serialized access
3. Channel-based mailbox -- funnel ops to a dedicated thread (recommended)
4. r2d2 pool -- `DuckdbConnectionManager` built into crate

### Prepared Statements

```rust
// Simple execution
conn.execute("INSERT INTO candles VALUES (?, ?, ?, ?, ?, ?)",
    params![ts, open, high, low, close, volume])?;

// Cached prepared statement (avoids re-preparation)
let mut stmt = conn.prepare_cached("SELECT * FROM candles WHERE symbol = ?")?;

// Single-row shortcut
let count: i64 = conn.query_row("SELECT COUNT(*) FROM candles", [], |row| row.get(0))?;
```

### Result Iteration

```rust
// Row-by-row mapping
let candles = stmt.query_map([], |row| {
    Ok(Candle {
        ts: row.get(0)?,
        open: row.get(1)?,
        // ...
    })
})?.collect::<Result<Vec<_>>>()?;

// Arrow columnar (zero-copy)
let batches: Vec<RecordBatch> = stmt.query_arrow([])?.collect();
```

### Appender API (Bulk Insert)

```rust
let mut appender = conn.appender("candles")?;
for candle in data {
    appender.append_row(params![candle.ts, candle.open, /*...*/])?;
}
appender.flush()?;  // Also called on drop
```

~63K-150K rows/sec for individual appends; significantly faster via Arrow RecordBatch.

### Extension Loading

```rust
// Runtime install (requires network on first call)
conn.execute_batch("INSTALL parquet; LOAD parquet;")?;

// ICU for timezone support -- NOT included in bundled builds
conn.execute_batch("INSTALL icu; LOAD icu;")?;
```

**Gotcha:** When using `bundled` feature, ICU extension is NOT included (crates.io 10MB limit). Timezone-aware operations fail without it.

## Alternative Bindings

| Crate | Description | Assessment |
|-------|-------------|------------|
| `duckdb` (duckdb-rs) | Official Rust client | **Use this one** |
| `async-duckdb` | Third-party async wrapper | Unnecessary if you use tokio::spawn_blocking |
| `frozen-duckdb` | Pre-compiled DuckDB binaries | Faster builds, less tested |
| `r2d2-duckdb` | Connection pooling | Built into duckdb crate with `r2d2` feature |

## Build on Windows

| Strategy | How | Notes |
|----------|-----|-------|
| `bundled` (recommended) | Compiles C++ from source | Requires MSVC Build Tools. **Known linker issues** |
| Download prebuilt | `DUCKDB_DOWNLOAD_LIB=1` env var | Fallback if bundled fails |
| System library | `DUCKDB_LIB_DIR` + `DUCKDB_INCLUDE_DIR` | Manual setup |
| vcpkg | Auto-detected for MSVC | Set `VCPKGRS_DYNAMIC=1` for dynamic linking |

**Risk:** Multiple open issues with Windows MSVC + `bundled`:
- [#544](https://github.com/duckdb/duckdb-rs/issues/544): link.exe failure (exit code 1120)
- [#413](https://github.com/duckdb/duckdb-rs/issues/413): libduckdb-sys build failure
- **Mitigation:** Test build on actual toolchain before committing.

```toml
# Recommended Cargo.toml
[dependencies]
duckdb = { version = "=1.10501.0", features = ["bundled"] }
```

## Embedded Mode Details

### Concurrency Model
- **Within process:** MVCC with optimistic concurrency. Multiple connections via `try_clone()`.
- **Cross-process:** Single writer only. Multiple read-only processes OK.
- `try_clone()` creates a new Connection to the same underlying Database instance.

### WAL and Checkpointing
- Changes written to WAL first, checkpointed to main file periodically.
- `CHECKPOINT` / `FORCE CHECKPOINT` available for manual control.
- Crash-safe: WAL ensures no corruption on unexpected shutdown.

### Memory Management
- **Default limit:** 80% of physical RAM (must override for desktop app!).
- Configurable: `SET memory_limit = '256MB';` or via `Config::default()?.max_memory("256MB")?`.
- Spills to disk when limit exceeded.
- Settings are NOT persistent -- must re-apply on each open.

### File Format Stability
- Backward compatible since v0.10 (current: v1.5.x).
- Not as battle-tested as SQLite's 20+ year format stability.
- For archival: export to Parquet (safest long-term format).

### Key Trading-Relevant Features

| Feature | Trading Use Case |
|---------|-----------------|
| Columnar storage | Scan only `close` column for MA calculation (6x less I/O) |
| Window functions | ATR, SMA, RSI, VWAP -- all in SQL |
| ASOF JOIN | Align trades with quotes, signals with executions |
| `time_bucket()` | Aggregate 1min candles to any timeframe |
| `FIRST()/LAST() ORDER BY` | Open/close prices in aggregation buckets |
| Parquet native | Import/export historical data |
| Zone maps | Skip row groups outside query time range |

## Binary Size Impact

| Component | Size |
|-----------|------|
| libduckdb (Windows x64) | ~30-55MB |
| SQLite (for comparison) | ~1-2MB |
| DuckDB Rust bundled | ~30-50MB added to binary |

Significant addition, but the project already links wgpu + iced (~80MB+ combined).
