# Provider/Broker Separation -- Master Index

## Executive Summary

Hand of Midas currently hard-wires data acquisition to a single `TestDataProvider` instance owned by `MidasApp`. The app calls `self.test_data.get_candles()` directly -- there is no trait boundary, no provider switching, and no caching layer between the data source and the chart panels. Meanwhile, `midas-broker` defines its own `MarketDataSource` trait for historical bars, tightly coupled to IB concepts like `con_id` and `request_id`.

This plan introduces two new trait abstractions:

1. **`DataProvider`** (async, in `midas-core`) -- a uniform interface for historical candle retrieval. `TestDataProvider`, a future `IbDataProvider`, and a future `PolygonDataProvider` all implement this trait. The app holds `Arc<dyn DataProvider>` and never knows which concrete provider is behind it.

2. **`OrderBroker`** (async, in `midas-core`) -- a uniform interface for order execution. The app holds `Option<Arc<dyn OrderBroker>>` because order execution is not always available (e.g., when using Polygon for data only). Only the trait definition ships now; no concrete implementation is built in this plan.

A `CachingProvider` wrapper composes any `DataProvider` with `midas-store`'s DuckDB backend, providing transparent read-through caching. A `ProviderRegistry` manages available providers and enables runtime switching via a toolbar dropdown.

### Why Now

- Charts already render via `&dyn CandleData` and hold `Option<Arc<CandleBuffer>>` -- the rendering path is provider-agnostic. The bottleneck is the acquisition path.
- DuckDB caching (`midas-store`) is operational but not wired into the data loading flow. Every app restart regenerates synthetic data from scratch.
- The IB integration (Phase 1) is next. Without this abstraction layer, IB data fetching would be another hard-wired call site, making future providers (Polygon, Alpaca, CSV replay) increasingly painful to add.
- Separating data from orders is a prerequisite for the common pattern of "Polygon for data, IB for orders."

## Non-Goals

These items are explicitly out of scope for this plan:

- **Wiring IB live data.** The `IbDataProvider` struct is defined as a future target. No IB API calls are made.
- **Implementing Polygon.** `PolygonDataProvider` is mentioned for design validation only. No Polygon SDK integration.
- **Order execution UI.** The `OrderBroker` trait is defined but no order panel, order entry form, or position display is built.
- **Real-time streaming.** The `DataProvider` trait covers historical data only. A future `subscribe()` method is noted in comments but not implemented.
- **Multi-provider blending.** No aggregation of data from multiple providers simultaneously (e.g., IB for equities, Polygon for crypto).

## Document Map

| # | Document | Purpose |
|---|---|---|
| 00 | `00-index.md` | This file. Executive summary, architecture, phase overview. |
| 01 | `01-trait-design.md` | `DataProvider`, `OrderBroker`, `ProviderError` trait and type definitions with full Rust code. |
| 02 | `02-test-provider.md` | Wrap existing `TestDataProvider` behind `DataProvider`. Migration path for `midas-feed`. |
| 03 | `03-caching-provider.md` | `CachingProvider<P: DataProvider>` -- DuckDB-backed transparent cache wrapping any provider. |
| 04 | `04-provider-registry.md` | `ProviderRegistry` -- runtime provider collection, active provider switching, serialization to config. |
| 05 | `05-app-integration.md` | `MidasApp` changes: replace `test_data` field with `Arc<dyn DataProvider>`, new `Message` variants, async data loading rewrite. |
| 06 | `06-ui-controls.md` | Toolbar provider dropdown (`pick_list`), enhanced status bar with provider name and connection indicator. |
| 07 | `07-implementation-roadmap.md` | Phased implementation plan with gates, testing strategy, and rollback points. |

## Architecture Diagram

```
                                    MidasApp
                                       |
                    +------------------+------------------+
                    |                                     |
           Arc<dyn DataProvider>              Option<Arc<dyn OrderBroker>>
                    |                                     |
              ProviderRegistry                     (future: IbOrderBroker)
              /       |       \
             /        |        \
     "test"        "ib"       "polygon"
        |             |            |
  CachingProvider  CachingProvider  CachingProvider
        |             |            |
  TestDataProvider IbDataProvider  PolygonDataProvider
                      |
                   (future)

    Each CachingProvider wraps:
    +-----------------------+
    |   CachingProvider<P>  |
    |   +---------------+   |       +------------------+
    |   | inner: P      |---+------>| DbHandle         |
    |   | store: DbHandle|  |       | (midas-store)    |
    |   +---------------+   |       | DuckDB on thread |
    +-----------------------+       +------------------+
                                           |
                                      cache.duckdb

    Data flow for get_candles("AAPL", D1, 365):

    1. CachingProvider checks DbHandle for cached data
    2. Cache HIT  --> return CandleBuffer from DuckDB
    3. Cache MISS --> delegate to inner DataProvider
    4. Inner returns CandleBuffer
    5. CachingProvider writes to DbHandle (fire-and-forget)
    6. Return CandleBuffer to caller
```

### Crate Dependency Graph (After)

```
midas-core          (leaf -- CandleBuffer, DataProvider, OrderBroker, ProviderError)
    ^
    |
midas-data          (CandleBuffer extensions: LOD, mmap, binary)
    ^
    |
midas-feed          (TestDataProvider, TestProvider: impl DataProvider)
    ^
    |
midas-store         (DbHandle, CachingProvider: impl DataProvider)
    ^
    |
midas-app           (ProviderRegistry, MidasApp)
```

Key constraint: `DataProvider` and `OrderBroker` traits live in `midas-core` so that any crate can implement them without circular dependencies. This is possible because `CandleBuffer` (struct + methods only) is moved to `midas-core`; LOD/mmap/binary functionality remains in `midas-data` as extension traits/functions. `CachingProvider` lives in `midas-store/src/caching_provider.rs` (no `midas-store -> midas-feed` dependency needed).

## Implementation Phases Summary

### Phase 1: Trait Definitions (doc 01)

Define `DataProvider`, `OrderBroker`, and `ProviderError` in `midas-core`. Move `CandleBuffer` to `midas-core`. Add `async-trait` dependency.

**Gate:** `cargo test --workspace` passes. New types compile. Trait is object-safe (verified by test).

### Phase 2: TestDataProvider Adapter (doc 02)

Implement `DataProvider` for `TestDataProvider` in `midas-feed`. The existing `get_candles(&mut self, ...)` method is adapted to the async `&self` signature using interior mutability (`Mutex<HashMap>` for the cache).

**Gate:** `TestDataProvider` can be used as `Arc<dyn DataProvider>`. Existing tests still pass.

### Phase 3: CachingProvider (doc 03)

Build `CachingProvider` that wraps any `DataProvider` and a `DbHandle`. Implements `DataProvider` itself. Read-through cache with fire-and-forget writes.

**Gate:** Integration test: `CachingProvider<TestDataProvider>` returns data. Second call serves from cache (verified by mock or timing).

### Phase 4: ProviderRegistry (doc 04)

Build `ProviderRegistry` to hold multiple named providers, track the active one, and support switching. Serializes active provider ID to `AppConfig`.

**Gate:** Registry can register providers, switch active, and return `Arc<dyn DataProvider>`.

### Phase 5: App Integration (doc 05)

Replace `test_data: TestDataProvider` in `MidasApp` with `registry: ProviderRegistry`. Rewrite `load_chart_data()` to use async `get_candles`. Add new `Message` variants. Update `PanelSymbolSubmitted` and `PanelTimeframeSelected` handlers.

**Gate:** App launches. Charts display data from `TestDataProvider` through `CachingProvider` via the registry. Identical user experience to current behavior.

### Phase 6: UI Controls (doc 06)

Add provider dropdown to toolbar using `pick_list`. Enhance status bar with provider name and connection dot (green/gray). Switching provider in dropdown triggers data reload for all charts.

**Gate:** Provider dropdown appears. Selecting a provider updates status bar. Charts reload.

## Key Design Decisions

### 1. Two traits, not one

Data feeds and order execution have fundamentally different lifecycles. You might use Polygon for data (no connection needed beyond an API key) while routing orders through IB (requires TWS Gateway, connection state machine, real-time heartbeats). Combining them into a single `Provider` trait would force every data-only provider to stub out order methods, and would prevent the app from holding `Option<Arc<dyn OrderBroker>>` to represent "no order routing configured."

### 2. Traits in midas-core, not midas-feed

`midas-core` is the leaf crate with zero internal dependencies. Placing the traits there means any crate (`midas-feed`, `midas-broker`, a future `midas-polygon`) can implement them without introducing circular dependencies. If the traits lived in `midas-feed`, then `midas-broker` would need to depend on `midas-feed` to implement `DataProvider` -- which is wrong architecturally.

### 3. async-trait over native async fn in trait

Rust 1.75+ supports `async fn` in traits, but only for `dyn`-safe traits when using `#[trait_variant::make]` or boxing manually. The `async-trait` crate provides a battle-tested, ergonomic solution that works with `Arc<dyn DataProvider>` out of the box. Since iced 0.14 already depends on tokio and the workspace uses async throughout, the overhead of one heap allocation per `get_candles` call is negligible compared to the I/O cost.

### 4. &self not &mut self

Multiple chart panels may call `get_candles()` concurrently (e.g., when switching layout presets reloads all charts). The `&self` signature enables sharing the provider behind `Arc<dyn DataProvider>` without external synchronization. Providers that need mutable state (like `TestDataProvider`'s internal cache) use interior mutability (`parking_lot::Mutex` -- no poisoning, faster on Windows).

### 5. CachingProvider as a wrapper, not a trait method

Caching is a cross-cutting concern that should not leak into the `DataProvider` trait. By implementing caching as a composable wrapper (`CachingProvider<P: DataProvider>` itself implements `DataProvider`), any provider can be cached or not cached at the caller's discretion. This also means tests can skip the cache layer entirely.

### 6. CandleBuffer as the universal output type

`CandleBuffer` (SoA, f32 prices, epoch-ms timestamps, u32 volumes) is already the type that charts consume. The `DataProvider` trait returns `CandleBuffer` directly, avoiding any conversion step between provider output and chart input. This is consistent with how `CandleData` trait works -- `CandleBuffer` implements `CandleData`, and charts render via `&dyn CandleData`.

### 7. Provider switching reloads all charts

When the user switches providers via the toolbar dropdown, all chart panels reload their data from the new provider. This is intentional -- stale data from a previous provider would be confusing. The reload uses the same `Message::DataLoaded` flow as initial chart loading, keeping the code path unified.

### 8. OrderBroker is trait-only for now

The `OrderBroker` trait is defined to validate the design (ensure data and order concerns separate cleanly) but no concrete implementation is built. This avoids premature abstraction while establishing the API contract that `midas-broker`'s future `IbOrderBroker` will implement.

---

## Resolved Decisions

- **Trait placement**: `DataProvider` and `OrderBroker` traits live in `midas-core/src/provider.rs`. This is possible because `CandleBuffer` is moved to `midas-core` (just the struct + methods; LOD/mmap/binary remain in `midas-data` as extension traits/functions).
- **Parameter semantics**: `get_candles` takes `days: u32` (calendar days), matching existing `TestDataProvider` API.
- **CachingProvider location**: Lives in `midas-store/src/caching_provider.rs`. No `midas-store -> midas-feed` dependency.
- **Mutex choice**: `parking_lot::Mutex` (no poisoning, faster on Windows).
- **CachingProvider::new()**: 2-arg `(inner, db_handle)`, name auto-derived as `"{inner.name()} (Cached)"`.
- **Provider registration**: One entry per logical provider. Caching is transparent -- not a separate dropdown entry.
- **`data_provider_names()`**: Returns `Vec<String>` (not `Vec<&str>`).
- **DataProvider trait surface**: 3 methods only: `fn name(&self) -> &str`, `fn is_connected(&self) -> bool`, `async fn get_candles(&self, symbol: &str, timeframe: Timeframe, days: u32) -> Result<CandleBuffer, ProviderError>`. No `id()`, no `info()` method. Name is `&str` borrowed from the implementor.
- **ProviderError enum**: Canonical definition is in `07-implementation-roadmap.md` Phase 1. Variants: `NotConnected`, `UnknownSymbol { symbol }`, `UnsupportedTimeframe { timeframe }`, `Io(#[from] std::io::Error)`, `Internal(String)`, `Store(String)`.
- **CachingProvider name**: Returns `"{inner.name()} (Cached)"` suffix for config persistence. Only the cached version is registered in the dropdown (not both raw and cached).
- **ProviderRegistry API**: Doc 07 (implementation roadmap) is canonical. Method: `active_data_provider() -> Option<Arc<dyn DataProvider>>`. `DataProviderSelected(String)` message carries provider name, not index. `active_data_idx` is private.
