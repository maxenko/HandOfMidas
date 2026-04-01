# Chart Data Cache — Implementation Plan

> **Crate:** `midas-store` | **Technology:** DuckDB (embedded, columnar) | **Role:** L2 persistent cache
>
> **Date:** 2026-03-31 | **Status:** Plan complete, ready for implementation

## Executive Summary

This plan specifies `midas-store`, a new workspace crate that adds a DuckDB-backed persistent cache for OHLCV chart data. The system is **generic and self-contained** — it initializes at application startup as `Option<DbHandle>` but is not consumed by any feature yet. All existing behavior (TestDataProvider, chart rendering, broker DB) remains unchanged.

### What It Does

- Persists `CandleBuffer` data to a columnar DuckDB database (`cache.duckdb`)
- Provides a `DbHandle` API (sync open, async operations) for insert/query/list
- Runs DuckDB operations on a dedicated OS thread via `MailboxProcessor::new_blocking()`
- Survives app restarts — cached data loads in ~5ms instead of regenerating
- Configurable via `[store]` section in `config.toml` with graceful fallback

### What It Does NOT Do

- Replace SQLite broker DB (orders, fills, positions stay in SQLite)
- Replace `.midas` binary files for hot-path GPU rendering
- Query DuckDB per-frame (only at load time / symbol change)
- Use DuckDB extensions (ICU, httpfs deferred to avoid startup latency)
- Use Arrow zero-copy (results always materialized to `CandleBuffer`)

## Three-Tier Data Architecture

```
Layer     | Technology              | Purpose                          | Latency
----------|-------------------------|----------------------------------|----------
L1 (Hot)  | CandleBuffer + mmap     | GPU rendering, live candles      | ~0us
L2 (Warm) | DuckDB (this plan)      | Analytical queries, cache        | 0.4-50ms
L3 (Cold) | IB API / CSV files      | Historical data fetch on demand  | 100ms-5s
Metadata  | SQLite (existing)       | Orders, fills, config, levels    | 0.05-1ms
```

## Documents

| # | Document | Lines | Key Content |
|---|----------|-------|-------------|
| [01](01-crate-architecture.md) | Crate Architecture | 1,340 | Module layout, dependency graph, Cargo.toml, feature gates, mailbox_processor integration |
| [02](02-actor-concurrency.md) | Actor Concurrency Model | 1,700 | `new_blocking()` implementation, DbCommand/DbReply enums, actor handler, connection lifecycle, backpressure, shutdown |
| [03](03-schema-and-migrations.md) | Schema & Migrations | 1,653 | Complete DDL, migration system, 7 query functions, bulk insert optimization, time bucket aggregation |
| [04](04-dbhandle-api.md) | DbHandle Public API | 1,555 | Public types, StoreError enum, async methods, clone semantics, timeout patterns, API evolution |
| [05](05-data-flow.md) | Data Flow | 1,140 | Write-behind cache pattern, 5 loading scenarios, Message variants, startup sequence, sequence diagrams |
| [06](06-config-and-startup.md) | Configuration & Startup | 1,229 | StoreConfig, config.toml integration, memory budget, graceful fallback, diagnostics |
| [07](07-testing-strategy.md) | Testing Strategy | 1,583 | Build spike, 27 unit/integration tests, 5 benchmarks, CI considerations |
| [08](08-implementation-roadmap.md) | Implementation Roadmap | 1,351 | 7 phases with gates, file lists, complexity ratings, risk mitigations |

**Total: 11,551 lines across 9 documents**

## Reading Order

1. Start with this index for the overview
2. **[08-implementation-roadmap.md](08-implementation-roadmap.md)** for the phased plan and decision gates
3. **[01-crate-architecture.md](01-crate-architecture.md)** for where everything lives
4. **[02-actor-concurrency.md](02-actor-concurrency.md)** for the threading model
5. **[03-schema-and-migrations.md](03-schema-and-migrations.md)** for the data layer
6. **[04-dbhandle-api.md](04-dbhandle-api.md)** for the public API contract
7. **[05-data-flow.md](05-data-flow.md)** for how data moves through the system
8. **[06-config-and-startup.md](06-config-and-startup.md)** for configuration and fallback behavior
9. **[07-testing-strategy.md](07-testing-strategy.md)** for test cases and benchmarks

## Implementation Phases (Summary)

| Phase | Scope | Gate | Complexity |
|-------|-------|------|------------|
| **0** | Build spike — DuckDB on Windows MSVC | open + migrate + insert/query 5K rows < 1s | S |
| **1** | Crate skeleton + schema + migrations | `cargo test -p midas-store` passes (3 tests) | M |
| **2** | Query layer + type conversions | 17 query/convert tests pass | M |
| **3** | Actor thread + DbHandle async API | 25 total tests pass | M |
| **4** | App integration — `Option<DbHandle>` in MidasApp | App starts with store enabled and disabled | L |
| **5** | Write-behind cache — persist after load | Second launch loads from DuckDB cache | M |
| **6** | Benchmarks + optimization | Startup overhead < 100ms, query 5K < 5ms | S |

**Estimated effort:** 5-8 working days

## New Crate Location

```
desktop/win/crates/midas-store/
├── Cargo.toml
├── src/
│   ├── lib.rs          # Public re-exports
│   ├── error.rs        # StoreError enum
│   ├── types.rs        # DataKey, CacheInfo, StoreConfig
│   ├── schema.rs       # DDL, migrations
│   ├── queries.rs      # SQL query functions
│   ├── convert.rs      # CandleBuffer <-> DuckDB conversions
│   ├── actor.rs        # DbCommand/DbReply, handler
│   └── handle.rs       # DbHandle async API
└── benches/
    └── store_bench.rs  # Criterion benchmarks
```

## Dependencies on External Crates

| Crate | Version | Purpose |
|-------|---------|---------|
| `duckdb` | `1` (bundled) | Embedded columnar database |
| `mailbox_processor` | local | Typed actor abstraction (copied from ControlPlugin) |
| `midas-core` | workspace | Timeframe, CandleData trait |
| `midas-data` | workspace | CandleBuffer |
| `tokio` | workspace | Async runtime |
| `thiserror` | workspace | Error derives |
| `tracing` | workspace | Structured logging |
| `chrono` | workspace | Timestamp utilities |

## Key Design Decisions

1. **Mailbox actor over Arc\<Mutex\>** — DuckDB Connection is `Send+!Sync`, Appender is `!Send+!Sync`. A dedicated OS thread avoids mutex contention and keeps blocking FFI off tokio's threadpool.

2. **Write-behind over write-through** — GPU rendering needs contiguous `f32`/`i64` arrays immediately. DuckDB writes happen asynchronously after data is already in use.

3. **Single actor thread (v1)** — Both reads and writes through one thread. Queries are infrequent (~5ms each, on symbol change only). Read pool deferred until profiling shows need.

4. **Always materialize to CandleBuffer** — DuckDB results are never exposed as `CandleData` directly. One-time ~1ms materialization at load, never on the render path.

5. **Graceful fallback** — If DuckDB fails to open or is disabled, `store = None` and all behavior is identical to pre-DuckDB. No user-visible errors.

6. **No new dependencies beyond DuckDB** — Reuses existing workspace crates (tokio, thiserror, tracing, chrono) and the mailbox_processor from ControlPlugin.

## Canonical References

When multiple documents describe the same concept, follow the document listed as canonical. Other documents provide overview-level illustrations but may simplify or diverge.

| Concept | Canonical Document | Other Mentions |
|---------|-------------------|----------------|
| Schema DDL (tables, columns, types) | [03-schema-and-migrations.md](03-schema-and-migrations.md) | 01 (simplified overview) |
| Module layout (which file owns which type) | [01-crate-architecture.md](01-crate-architecture.md) | 04 (simplified overview) |
| Public API types (StoreError, DataKey, CacheInfo) | [04-dbhandle-api.md](04-dbhandle-api.md) | 01 (illustrative) |
| StoreConfig (fields, defaults, serde) | [06-config-and-startup.md](06-config-and-startup.md) | 01, 04 (illustrative) |
| DbHandle methods and signatures | [04-dbhandle-api.md](04-dbhandle-api.md) | 08 (implementation-level) |
| Actor threading model | [02-actor-concurrency.md](02-actor-concurrency.md) | 04 (overview) |
| Connection initialization strategy | [02-actor-concurrency.md](02-actor-concurrency.md) | 04 (overview) |
| Implementation task list | [08-implementation-roadmap.md](08-implementation-roadmap.md) | all (context) |

**Key resolved decisions:**
- **`StoreConfig.enabled` defaults to `true`** — DuckDB activates on first launch. Existing configs without `[store]` get defaults via `#[serde(default)]`.
- **`DbHandle::open()` is synchronous** — it only creates the mpsc channel and spawns the actor thread. No async needed. The health-check ping is deferred to a startup `Task::perform()` in iced.
- **`new_blocking()` uses `blocking_recv()`** — the simpler form with no mini tokio runtime. The handler is purely synchronous.
- **Appender does NOT support INSERT OR IGNORE** — use DELETE-before-INSERT for upsert. See Doc 03 Section 10.

## Research Foundation

This plan is built on the [DuckDB integration research](../../research/duckdb/00-index.md) (7 documents covering ecosystem, architecture, schema, performance, integration, and codebase analysis).

## Risk Summary

| Risk | Impact | Mitigation |
|------|--------|------------|
| Windows MSVC build failure | Blocks entire feature | Phase 0 build spike with fallback chain |
| DuckDB binary size (+30-50MB) | Larger installer | Acceptable — app already links wgpu+iced (~80MB) |
| DuckDB memory default (80% RAM) | OOM on low-RAM machines | Explicitly cap at 128-256MB via SET |
| Actor thread bottleneck | Slow 20+ chart loads | Future read pool with `try_clone()` + `Semaphore(8)` |
