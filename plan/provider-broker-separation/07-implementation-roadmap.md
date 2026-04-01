# 07 -- Implementation Roadmap

> Parent: [00-index.md](./00-index.md)
> Depends on: all prior design documents (01-06)

## Overview

Six phases, ordered by dependency. Each phase has a clear gate -- a
set of commands and observable behaviors that must pass before proceeding
to the next phase. Sizes are estimated in T-shirt units:
**S** = a few hours, **M** = half a day to a day.

Phases 1-3 are independent of the running application. Phase 4 is the
integration seam where the app switches from direct `TestDataProvider`
calls to trait-based provider access. Phase 5 adds UI controls. Phase 6
adds config persistence.

---

## Phase 1: Trait Definitions (S)

### Goal

Define the `DataProvider` and `OrderBroker` traits, plus supporting types,
in `midas-core`. Adds trait definitions and moves `CandleBuffer` to `midas-core` to enable trait return types.

### Files Created

| File | Contents |
|---|---|
| `midas-core/src/provider.rs` | `DataProvider` trait, `OrderBroker` trait, `ProviderError`, `ConnectionState` |
| `midas-core/src/candle_buffer.rs` | `CandleBuffer` struct + methods moved from `midas-data` |

### Files Modified

| File | Change |
|---|---|
| `midas-core/src/lib.rs` | Add `pub mod provider;` and `pub mod candle_buffer;`, re-export `CandleBuffer` |
| `midas-core/Cargo.toml` | Add `async-trait = "0.1"` dependency |
| `Cargo.toml` | Add `async-trait = "0.1"` to `[workspace.dependencies]` |
| `midas-data/src/candle.rs` | Remove `CandleBuffer` struct definition, keep `CandleSlice` (re-import `CandleBuffer` from `midas-core`) |
| `midas-data/src/lib.rs` | Re-export `midas_core::CandleBuffer` for backward compatibility |
| `midas-data/Cargo.toml` | Already depends on `midas-core` -- no change needed |

### Type Definitions

```rust
// provider.rs

use async_trait::async_trait;
use thiserror::Error;

use crate::Timeframe;

/// Errors that can occur during data provider or broker operations.
#[derive(Debug, Error)]
pub enum ProviderError {
    /// The provider is not connected and cannot serve the request.
    #[error("provider not connected")]
    NotConnected,
    /// The requested symbol is not recognized or not available.
    #[error("unknown symbol: {symbol}")]
    UnknownSymbol { symbol: String },
    /// The requested timeframe is not supported by this provider.
    #[error("unsupported timeframe: {timeframe}")]
    UnsupportedTimeframe { timeframe: String },
    /// A network or I/O error occurred.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// An internal error with a free-form message.
    #[error("internal error: {0}")]
    Internal(String),
    /// Wraps a store (caching) error transparently.
    #[error("cache error: {0}")]
    Store(String),
}

/// Connection lifecycle states for providers that maintain persistent
/// connections (IB Gateway, WebSocket feeds, etc.).
///
/// Mirrors the existing `ConnectionState` in `midas-broker` but lives
/// in `midas-core` so the UI crate can reference it without depending
/// on `midas-broker`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionState {
    /// No connection attempt has been made or the connection is closed.
    Disconnected,
    /// A connection attempt is in progress.
    Connecting,
    /// Connected to the remote server.
    Connected { server_version: i32 },
    /// Connected and fully initialized (ready for requests).
    Ready,
    /// Connection lost; automatic reconnection in progress.
    Reconnecting { attempt: u32 },
}

/// Uniform interface for historical candle data retrieval.
///
/// Implementors: `TestProvider`, `CachingProvider`, future `IbDataProvider`,
/// future `PolygonDataProvider`.
///
/// The `&self` signature enables sharing behind `Arc<dyn DataProvider>`.
/// Providers needing mutable state use interior mutability.
#[async_trait]
pub trait DataProvider: Send + Sync {
    /// Human-readable name for UI display.
    ///
    /// Borrowed from the implementor -- no allocation on each call.
    fn name(&self) -> &str;

    /// Whether the provider is currently able to serve requests.
    ///
    /// For local/test providers this always returns `true`.
    /// For network providers, returns `true` only when connected.
    fn is_connected(&self) -> bool;

    /// Retrieve historical candle data.
    ///
    /// # Arguments
    /// - `symbol`: Ticker symbol (e.g. "AAPL").
    /// - `timeframe`: Bar duration.
    /// - `days`: Number of calendar days of history to retrieve.
    ///
    /// Returns a `CandleBuffer` (SoA format) on success.
    async fn get_candles(
        &self,
        symbol: &str,
        timeframe: Timeframe,
        days: u32,
    ) -> Result<CandleBuffer, ProviderError>;
}

/// Uniform interface for order execution.
///
/// Trait-only for now -- no concrete implementation is built in this plan.
/// Future implementor: `IbOrderBroker` in `midas-broker`.
#[async_trait]
pub trait OrderBroker: Send + Sync {
    /// Human-readable name for UI display.
    fn name(&self) -> &str;

    /// Whether the broker is currently connected.
    fn is_connected(&self) -> bool;

    /// Current connection state.
    fn connection_state(&self) -> ConnectionState;
}
```

### Tests Added

```rust
// In provider.rs #[cfg(test)] mod tests

#[test]
fn data_provider_is_object_safe() {
    // Compile-time proof that DataProvider can be used as dyn trait.
    fn _assert_object_safe(_: &dyn DataProvider) {}
}

#[test]
fn order_broker_is_object_safe() {
    fn _assert_object_safe(_: &dyn OrderBroker) {}
}

#[test]
fn provider_error_display() {
    let err = ProviderError::UnknownSymbol {
        symbol: "XYZ".into(),
    };
    assert!(err.to_string().contains("XYZ"));
}

#[test]
fn connection_state_eq() {
    assert_eq!(ConnectionState::Disconnected, ConnectionState::Disconnected);
    assert_ne!(ConnectionState::Connecting, ConnectionState::Disconnected);
}
```

### Gate Criteria

```bash
cargo build -p midas-core        # compiles with new types
cargo test -p midas-core         # all existing + new tests pass
cargo clippy -p midas-core       # no warnings
```

### Dependencies on Prior Phases

None. This is the first phase.

### Risk Factors

- **`async-trait` version compatibility.** The workspace currently does not
  use `async-trait`. Adding it is low-risk -- it is a proc-macro with no
  transitive runtime dependencies. Pin to `0.1` (latest stable).
- **`CandleBuffer` ownership.** The `DataProvider` trait returns
  `CandleBuffer`. **Resolution:** Move `CandleBuffer` struct definition
  (just the Vec fields and methods) to `midas-core`. LOD/mmap/binary
  stays in `midas-data` as extension traits/functions). This lets `DataProvider`
  live in `midas-core/src/provider.rs` with zero circular deps.

#### Circular Dependency Resolution

The workspace dependency graph is:

```
midas-core (leaf) <-- midas-data <-- midas-feed
```

`midas-core` cannot depend on `midas-data` without creating a cycle.
The resolution is: **move `CandleBuffer` struct definition to `midas-core`**.
Only the struct with its Vec fields and basic methods moves; LOD/mmap/binary
functionality remains in `midas-data` as extension traits/functions.

This lets BOTH traits live in `midas-core`:

| File | Contents |
|---|---|
| `midas-core/src/provider.rs` | `DataProvider` trait, `OrderBroker` trait, `ProviderError`, `ConnectionState` |

No split between `midas-core` and `midas-feed` is needed. `CachingProvider`
lives in `midas-store` (which already depends on `midas-core`). No
`midas-store -> midas-feed` dependency.

---

## Phase 2: TestProvider Implementation (S)

### Goal

Wrap the existing `TestDataProvider` (in `midas-feed`) behind the new
`DataProvider` trait. The wrapper uses `parking_lot::Mutex` for interior
mutability so the async `&self` signature works with `Arc<dyn DataProvider>`.
`parking_lot::Mutex` is preferred over `std::sync::Mutex` because it has no
poisoning and is faster on Windows.

### Files Created

| File | Contents |
|---|---|
| `desktop/win/crates/midas-feed/src/test_provider.rs` | `TestProvider` struct implementing `DataProvider` |

### Files Modified

| File | Change |
|---|---|
| `desktop/win/crates/midas-feed/src/lib.rs` | Add `pub mod test_provider;` and `pub use test_provider::TestProvider;` |

### Implementation

```rust
// test_provider.rs

use parking_lot::Mutex;

use async_trait::async_trait;
use midas_core::provider::{DataProvider, ProviderError};
use midas_core::Timeframe;
use midas_core::CandleBuffer;

use crate::testdata::TestDataProvider;

/// Wraps the synchronous `TestDataProvider` behind the async `DataProvider`
/// trait. Uses `parking_lot::Mutex` for interior mutability since
/// `TestDataProvider` requires `&mut self` for its cache.
pub struct TestProvider {
    inner: Mutex<TestDataProvider>,
}

impl TestProvider {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(TestDataProvider::new()),
        }
    }
}

impl Default for TestProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl DataProvider for TestProvider {
    fn name(&self) -> &str {
        "Test Data"
    }

    fn is_connected(&self) -> bool {
        true // Test data is always available.
    }

    async fn get_candles(
        &self,
        symbol: &str,
        timeframe: Timeframe,
        days: u32,
    ) -> Result<CandleBuffer, ProviderError> {
        let mut inner = self.inner.lock();
        Ok(inner.get_candles(symbol, timeframe, days))
    }
}
```

### Tests Added

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_provider_returns_data() {
        let provider = TestProvider::new();
        let buf = provider.get_candles("AAPL", Timeframe::D1, 730).await.unwrap();
        assert!(!buf.is_empty());
    }

    #[tokio::test]
    async fn test_provider_is_always_connected() {
        let provider = TestProvider::new();
        assert!(provider.is_connected());
    }

    #[tokio::test]
    async fn test_provider_matches_raw_test_data() {
        // Verify the trait wrapper produces identical data to raw calls.
        let provider = TestProvider::new();
        let mut raw = TestDataProvider::new();

        let via_trait = provider.get_candles("MSFT", Timeframe::D1, 90).await.unwrap();
        let via_raw = raw.get_candles("MSFT", Timeframe::D1, 90);

        assert_eq!(via_trait.len(), via_raw.len());
        // First and last candle timestamps must match.
        assert_eq!(via_trait.timestamps[0], via_raw.timestamps[0]);
        let n = via_trait.len();
        assert_eq!(via_trait.timestamps[n - 1], via_raw.timestamps[n - 1]);
    }

    #[tokio::test]
    async fn test_provider_usable_as_dyn() {
        let provider: Arc<dyn DataProvider> = Arc::new(TestProvider::new());
        let buf = provider.get_candles("GOOG", Timeframe::H1, 7).await.unwrap();
        assert!(!buf.is_empty());
    }

    #[test]
    fn test_provider_name() {
        let provider = TestProvider::new();
        assert_eq!(provider.name(), "Test Data");
    }
}
```

### Gate Criteria

```bash
cargo test -p midas-feed         # all existing + new tests pass
cargo clippy -p midas-feed       # no warnings
```

Specific gate: `Arc<dyn DataProvider>` pointing to `TestProvider` compiles
and produces data identical to `TestDataProvider::get_candles()`.

### Dependencies on Prior Phases

Phase 1 (trait definitions in `midas-core`).

### Risk Factors

- **`Mutex` lock contention.** The `TestDataProvider` lock is held only
  during synchronous data generation (sub-millisecond for cached results,
  ~50ms for first-time generation of a new ticker). Since chart loads are
  sequential in practice (user submits one symbol at a time), contention
  is negligible. `parking_lot::Mutex` is used (not `tokio::sync::Mutex`)
  because the critical section is CPU-bound and short.
- **No lock poisoning.** `parking_lot::Mutex` does not support poisoning,
  so there is no `PoisonError` to handle. This is the desired behavior.

---

## Phase 3: CachingProvider (S)

### Goal

Create a `CachingProvider` that wraps any `DataProvider` and a `DbHandle`,
implementing the `DataProvider` trait itself. On `get_candles()`:

1. Query the DuckDB cache via `DbHandle::query_candles()`.
2. **Cache hit** (non-empty result): return the cached `CandleBuffer`.
3. **Cache miss** (empty or error): delegate to the inner provider.
4. On inner success: write the result to DuckDB via `fire_and_forget_insert()`.
5. Return the `CandleBuffer` to the caller.

### Files Created

| File | Contents |
|---|---|
| `desktop/win/crates/midas-store/src/caching_provider.rs` | `CachingProvider` struct implementing `DataProvider` |

### Files Modified

| File | Change |
|---|---|
| `desktop/win/crates/midas-store/src/lib.rs` | Add `pub mod caching_provider;` and `pub use caching_provider::CachingProvider;` |
| `desktop/win/crates/midas-store/Cargo.toml` | Add `async-trait` dependency (no `midas-feed` dep needed) |
| `midas-store/Cargo.toml` | Add `midas-feed = { path = "../midas-feed" }` under `[dev-dependencies]` (test-only, not production) |

> **Note on crate placement:** `CachingProvider` lives in `midas-store`
> rather than `midas-app` because it composes `DbHandle` (from `midas-store`)
> with `DataProvider` (from `midas-core`). The `midas-store` crate already
> depends on `midas-core` (which now contains both the `DataProvider` trait
> and `CandleBuffer`). No `midas-store -> midas-feed` dependency is needed.

### Implementation

```rust
// caching_provider.rs

use std::sync::Arc;

use async_trait::async_trait;
use midas_core::provider::{DataProvider, ProviderError};
use midas_core::Timeframe;
use midas_core::CandleBuffer;

use crate::handle::DbHandle;
use crate::types::DataKey;

/// Transparent caching wrapper around any `DataProvider`.
///
/// Checks DuckDB for cached data before delegating to the inner provider.
/// Cache writes use fire-and-forget to avoid blocking the caller.
pub struct CachingProvider {
    inner: Arc<dyn DataProvider>,
    store: DbHandle,
    /// Display name, auto-derived as `"{inner.name()} (Cached)"`.
    name: String,
}

impl CachingProvider {
    pub fn new(
        inner: Arc<dyn DataProvider>,
        store: DbHandle,
    ) -> Self {
        let name = format!("{} (Cached)", inner.name());
        Self {
            inner,
            store,
            name,
        }
    }
}

#[async_trait]
impl DataProvider for CachingProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn is_connected(&self) -> bool {
        self.inner.is_connected()
    }

    async fn get_candles(
        &self,
        symbol: &str,
        timeframe: Timeframe,
        days: u32,
    ) -> Result<CandleBuffer, ProviderError> {
        let key = DataKey {
            symbol: symbol.to_string(),
            timeframe,
        };

        // 1. Try cache.
        match self.store.query_candles(key.clone()).await {
            Ok(buf) if !buf.is_empty() => {
                tracing::debug!(
                    "Cache HIT for {symbol} {timeframe}: {} candles",
                    buf.len()
                );
                return Ok(buf);
            }
            Ok(_) => {
                tracing::debug!("Cache MISS for {symbol} {timeframe} (empty)");
            }
            Err(e) => {
                tracing::warn!("Cache query failed for {symbol} {timeframe}: {e}");
                // Fall through to inner provider.
            }
        }

        // 2. Delegate to inner provider.
        let buffer = self.inner.get_candles(symbol, timeframe, days).await?;

        // 3. Write-behind cache (fire and forget).
        if !buffer.is_empty() {
            let source = self.inner.name().to_string();
            if let Err(e) = self
                .store
                .fire_and_forget_insert(key, buffer.clone(), &source)
                .await
            {
                tracing::warn!("Cache write failed for {symbol} {timeframe}: {e}");
            }
        }

        Ok(buffer)
    }
}
```

### Tests Added

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use midas_feed::TestProvider;

    #[tokio::test]
    async fn caching_provider_returns_data() {
        let inner = Arc::new(TestProvider::new());
        let store = DbHandle::open_memory();
        let cached = CachingProvider::new(inner, store);

        let buf = cached.get_candles("AAPL", Timeframe::D1, 730).await.unwrap();
        assert!(!buf.is_empty());
    }

    #[tokio::test]
    async fn caching_provider_serves_from_cache_on_second_call() {
        let inner = Arc::new(TestProvider::new());
        let store = DbHandle::open_memory();
        let cached = CachingProvider::new(
            Arc::clone(&inner) as Arc<dyn DataProvider>,
            store.clone(),
        );

        // First call -- cache miss, populates cache.
        let buf1 = cached.get_candles("TSLA", Timeframe::D1, 30).await.unwrap();
        assert!(!buf1.is_empty());

        // Allow fire-and-forget write to complete.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Second call -- should serve from cache.
        // Verify by checking the store directly.
        let key = DataKey {
            symbol: "TSLA".to_string(),
            timeframe: Timeframe::D1,
        };
        let cached_buf = store.query_candles(key).await.unwrap();
        assert_eq!(cached_buf.len(), buf1.len());
    }

    #[tokio::test]
    async fn caching_provider_delegates_is_connected() {
        let inner = Arc::new(TestProvider::new());
        let store = DbHandle::open_memory();
        let cached = CachingProvider::new(inner, store);
        assert!(cached.is_connected()); // TestProvider is always connected.
    }

    #[tokio::test]
    async fn caching_provider_name() {
        let inner = Arc::new(TestProvider::new());
        let store = DbHandle::open_memory();
        let cached = CachingProvider::new(inner, store);
        let name = cached.name();
        assert_eq!(name, "Test Data (Cached)");
    }
}
```

### Gate Criteria

```bash
cargo test -p midas-store        # all existing + new tests pass
cargo clippy -p midas-store      # no warnings
```

Specific gate: First `get_candles()` call returns data. DuckDB contains
the data after the call (verified by `query_candles` in test).

### Dependencies on Prior Phases

- Phase 1 (trait definitions)
- Phase 2 (TestProvider -- used in tests; also validates the trait is usable)

### Risk Factors

- **`CandleBuffer::clone()` cost.** The `clone()` in the write-behind path
  copies all SoA arrays. For 2500 daily candles (~50KB), this is sub-millisecond.
  For 100K intraday candles (~2MB), it could take 1-2ms. This is acceptable
  for a fire-and-forget background operation. If profiling reveals issues,
  the clone can be replaced with `Arc<CandleBuffer>` sharing.
- **Race condition on concurrent cache misses.** Two charts loading the same
  symbol concurrently could both miss the cache and both insert. DuckDB
  handles duplicate inserts gracefully (the schema uses INSERT OR REPLACE).
  No data corruption occurs; at worst, a few milliseconds of redundant work.
- **No new crate dependencies.** `CachingProvider` depends only on
  `midas-core` (for `DataProvider` trait and `CandleBuffer`) and `midas-store`
  itself (for `DbHandle`). No `midas-store -> midas-feed` edge is needed.

---

## Phase 4: ProviderRegistry + App Integration (M)

### Goal

Build a `ProviderRegistry` that holds multiple named data providers and
an optional order broker. Replace the `test_data: TestDataProvider` and
`store: Option<DbHandle>` fields in `MidasApp` with a single
`providers: ProviderRegistry` field. Rewrite `load_test_data_for_chart()`
to call `provider.get_candles()` via async Task dispatch.

### Files Created

| File | Contents |
|---|---|
| `desktop/win/crates/midas-core/src/registry.rs` | `ProviderRegistry` struct |

> **Note:** Both `DataProvider` and `OrderBroker` traits are now in
> `midas-core`. However, `ProviderRegistry` is an application-level
> orchestration struct, not reusable library code.
>
> **Resolution:** Place `ProviderRegistry` in `midas-app`.

**Updated file plan:**

| File | Contents |
|---|---|
| `desktop/win/crates/midas-app/src/registry.rs` | `ProviderRegistry` struct |

### Files Modified

| File | Change |
|---|---|
| `desktop/win/crates/midas-app/src/app.rs` | Replace `test_data` and `store` fields with `providers: ProviderRegistry`. Rewrite `load_test_data_for_chart()` -> `load_chart_data()`. Update `MidasApp::new()` to build registry. Update `PanelSymbolSubmitted` and `PanelTimeframeSelected` handlers. |
| `desktop/win/crates/midas-app/src/app/persistence.rs` | Update config build to use `providers.active_data_provider_name()` instead of hard-coded values. |
| `desktop/win/crates/midas-app/src/main.rs` | Add `mod registry;` |

### ProviderRegistry Implementation

```rust
// registry.rs

use std::sync::Arc;
use midas_core::provider::{DataProvider, OrderBroker};

/// Manages available data providers and order brokers.
///
/// Holds a list of registered providers and tracks which one is active.
/// Provider switching triggers chart reloads in the app layer.
pub struct ProviderRegistry {
    /// Registered data providers, in display order.
    data_providers: Vec<Arc<dyn DataProvider>>,
    /// Index of the currently active data provider.
    active_data_idx: usize,
    /// Registered order brokers, in display order.
    order_brokers: Vec<Arc<dyn OrderBroker>>,
    /// Index of the currently active order broker, or None for "None".
    active_broker_idx: Option<usize>,
}

impl ProviderRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            data_providers: Vec::new(),
            active_data_idx: 0,
            order_brokers: Vec::new(),
            active_broker_idx: None,
        }
    }

    /// Register a data provider. First registered becomes the default active.
    pub fn register_data_provider(&mut self, provider: Arc<dyn DataProvider>) {
        self.data_providers.push(provider);
    }

    /// Register an order broker.
    pub fn register_order_broker(&mut self, broker: Arc<dyn OrderBroker>) {
        self.order_brokers.push(broker);
    }

    /// Names of all registered data providers (for pick_list options).
    pub fn data_provider_names(&self) -> Vec<String> {
        self.data_providers
            .iter()
            .map(|p| p.name().to_string())
            .collect()
    }

    /// Names of all registered order brokers, with "None" prepended.
    pub fn order_broker_names(&self) -> Vec<String> {
        let mut names = vec!["None".to_string()];
        for b in &self.order_brokers {
            names.push(b.name().to_string());
        }
        names
    }

    /// Get the currently active data provider, if any.
    pub fn active_data_provider(&self) -> Option<Arc<dyn DataProvider>> {
        self.data_providers.get(self.active_data_idx).cloned()
    }

    /// Display name of the active data provider.
    pub fn active_data_provider_name(&self) -> String {
        self.data_providers
            .get(self.active_data_idx)
            .map(|p| p.name().to_string())
            .unwrap_or_else(|| "None".to_string())
    }

    /// Get the currently active order broker, if any.
    pub fn active_broker(&self) -> Option<Arc<dyn OrderBroker>> {
        self.active_broker_idx
            .and_then(|idx| self.order_brokers.get(idx).cloned())
    }

    /// Find a data provider's index by display name.
    pub fn find_data_provider_index(&self, name: &str) -> Option<usize> {
        self.data_providers
            .iter()
            .position(|p| p.name() == name)
    }

    /// Find a broker's index by display name.
    pub fn find_broker_index(&self, name: &str) -> Option<usize> {
        self.order_brokers
            .iter()
            .position(|b| b.name() == name)
    }
}
```

> **Resolved:** `data_provider_names()` and `order_broker_names()` return
> `Vec<String>`. Since `name()` returns `&str` borrowed from the provider,
> returning borrows from the registry would require lifetime gymnastics.
> `Vec<String>` is simple and correct. The heap allocation of a few short
> strings per frame is negligible.

### App Data Loading Rewrite

The synchronous `load_test_data_for_chart()` is replaced with an async
flow using `Task::perform`:

```rust
/// Load candle data for a chart via the active data provider.
///
/// Dispatches an async task that calls `provider.get_candles()` and
/// returns the result via `Message::DataLoaded`.
fn load_chart_data(
    &mut self,
    chart_id: ChartId,
    symbol: &str,
    tf: Timeframe,
    reset_camera: bool,
) -> Task<Message> {
    let provider = match self.providers.active_data_provider() {
        Some(p) => p,
        None => {
            self.status_message = "No data provider available".to_string();
            return Task::none();
        }
    };

    // Mark chart as loading.
    if let Some(chart) = self.charts.get_mut(&chart_id) {
        chart.load_state = LoadState::Loading;
    }

    let symbol = symbol.to_string();
    let days = Self::days_for_timeframe(tf);

    Task::perform(
        async move {
            let result = provider.get_candles(&symbol, tf, days).await;
            (chart_id, result.map(Arc::new).map_err(|e| e.to_string()), reset_camera)
        },
        |(chart_id, result, reset_camera)| {
            // Wrap in a batch so we can carry reset_camera.
            // Or: extend DataLoaded to carry the flag.
            Message::DataLoaded(chart_id, result)
        },
    )
}

/// Determine how many calendar days of data to request based on timeframe.
fn days_for_timeframe(tf: Timeframe) -> u32 {
    match tf.as_secs() {
        s if s >= Timeframe::W1.as_secs() => 3650, // ~10 years
        s if s >= Timeframe::D1.as_secs() => 730,   // ~2 years
        s if s >= Timeframe::H1.as_secs() => 90,    // ~3 months
        s if s >= Timeframe::M15.as_secs() => 30,   // ~1 month
        _ => 10,                                     // <=M5: ~10 days
    }
}
```

### MidasApp::new() Changes

```rust
// In MidasApp::new():

// -- Before --
let test_data = TestDataProvider::new();
let store = if config.store.enabled { ... } else { None };

// -- After --
let test_provider = Arc::new(TestProvider::new());
let mut registry = ProviderRegistry::new();

// Register ONE provider per logical source. Caching is transparent.
let data_provider: Arc<dyn DataProvider> = if config.store.enabled {
    let store = DbHandle::open(store_config);
    Arc::new(CachingProvider::new(
        Arc::clone(&test_provider) as Arc<dyn DataProvider>,
        store,
    ))
} else {
    test_provider
};
registry.register_data_provider(data_provider);

// Restore active provider from config (Phase 6), or default to first.
if let Some(saved_name) = &config.providers.as_ref().and_then(|p| p.active_data.as_ref()) {
    if let Some(idx) = registry.find_data_provider_index(saved_name) {
        registry.set_active_data(idx);
    }
}
```

### Tests Added

Tests for `ProviderRegistry` are unit tests in `registry.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use midas_feed::TestProvider;

    #[test]
    fn registry_starts_empty() {
        let reg = ProviderRegistry::new();
        assert!(reg.active_data_provider().is_none());
        assert_eq!(reg.data_provider_names().len(), 0);
    }

    #[test]
    fn registry_register_and_lookup() {
        let mut reg = ProviderRegistry::new();
        reg.register_data_provider(Arc::new(TestProvider::new()));
        assert_eq!(reg.data_provider_names(), vec!["Test Data".to_string()]);
        assert!(reg.active_data_provider().is_some());
    }

    #[test]
    fn registry_find_by_name() {
        let mut reg = ProviderRegistry::new();
        reg.register_data_provider(Arc::new(TestProvider::new()));
        assert_eq!(reg.find_data_provider_index("Test Data"), Some(0));
        assert_eq!(reg.find_data_provider_index("Unknown"), None);
    }

    #[test]
    fn registry_broker_none_by_default() {
        let reg = ProviderRegistry::new();
        assert!(reg.active_broker().is_none());
        assert_eq!(reg.order_broker_names(), vec!["None".to_string()]);
    }
}
```

### Gate Criteria

```bash
cargo test --workspace           # all tests pass
cargo run -p midas-app           # app launches
```

Functional gate:
1. App starts and displays charts -- identical behavior to before.
2. Charts load data from `TestProvider` through the registry.
3. If `store.enabled = true` in config, caching is transparent (one entry
   in the registry, name shows inner provider name).

### Dependencies on Prior Phases

- Phase 1 (trait definitions)
- Phase 2 (TestProvider)
- Phase 3 (CachingProvider -- needed for the cached variant)

### Risk Factors

- **Breaking change scope.** This phase modifies `MidasApp` fields and the
  data loading path -- the most complex change in the plan. Mitigation:
  keep the `DataLoaded` message variant unchanged. The only difference is
  that data now arrives asynchronously via `Task::perform` instead of being
  generated synchronously in `update()`. The `DataLoaded` handler already
  exists and handles `Result<Arc<CandleBuffer>, String>`.
- **Synchronous -> async transition.** The current `load_test_data_for_chart`
  is synchronous and sets chart fields immediately. The async version sets
  `LoadState::Loading` first, then processes the result when `DataLoaded`
  arrives. This means there is a brief period where the chart shows
  "Loading..." instead of data. For TestProvider (sub-millisecond generation),
  this flash is imperceptible. For future network providers, the loading
  state is essential UX.
- **`reset_camera` flag threading.** The current code passes `reset_camera`
  to the synchronous loader. In the async version, this flag needs to travel
  through the task and back via the message. Options: (a) extend `DataLoaded`
  to carry the flag, or (b) store a `pending_camera_reset: HashSet<ChartId>`
  in `MidasApp`. Option (a) is cleaner -- change `DataLoaded` to
  `DataLoaded(ChartId, Result<Arc<CandleBuffer>, String>, bool)`.

---

## Phase 5: UI Controls (M)

### Goal

Add two `pick_list` dropdowns to the toolbar and enhance the status bar
with a connection indicator dot and provider name. Full specification is
in [06-ui-controls.md](./06-ui-controls.md).

### Files Modified

| File | Change |
|---|---|
| `desktop/win/crates/midas-app/src/app.rs` | Add `DataProviderSelected(String)` and `OrderBrokerSelected(String)` to `Message`. Add update handlers. Add `reload_all_charts()`. |
| `desktop/win/crates/midas-app/src/app/views.rs` | Modify `view_toolbar()`: add `Space::new().width(Fill)` + two `pick_list` widgets. Modify `view_status_bar()`: add connection dot + provider name. Add `connection_indicator()` helper. Add `dark_pick_list_style()` and `dark_pick_list_menu_style()` functions. |

### New Imports in views.rs

```rust
use iced::widget::pick_list;
```

### Tests Added

UI tests in iced are primarily visual/manual. The testable logic is:

```rust
#[test]
fn connection_indicator_test_provider() {
    // When TestProvider is active and no broker is set:
    // dot should be green (connected).
    // This test validates the logic, not the rendering.
    let (dot, color) = connection_indicator_logic(
        true,  // data connected
        None,  // no broker
    );
    assert_eq!(dot, "\u{25CF}");
    assert_eq!(color, theme::STATUS_OK);
}

#[test]
fn connection_indicator_disconnected() {
    let (dot, color) = connection_indicator_logic(
        false, // data not connected
        None,  // no broker
    );
    assert_eq!(dot, "\u{25CB}");
    assert_eq!(color, theme::TEXT_MUTED);
}
```

### Gate Criteria

```bash
cargo build -p midas-app         # compiles with new pick_list imports
cargo run -p midas-app           # app launches
```

Functional gate:
1. Toolbar shows two dropdowns on the right side.
2. Data dropdown lists `"Test Data"` (one entry per logical provider; caching
   is transparent, not a separate dropdown entry).
3. Broker dropdown lists `"None"`.
4. Selecting a different data provider reloads all charts.
5. Status bar shows green dot + provider name on the left.
6. Switching providers updates the status bar provider name.

### Dependencies on Prior Phases

- Phase 4 (ProviderRegistry in MidasApp, `reload_all_charts()`)

### Risk Factors

- **iced 0.14 `pick_list` API surface.** The exact styling API may differ
  slightly from what is documented here (iced 0.14 is relatively new and
  the styling API has evolved). Mitigation: test the style closure signature
  against the actual iced 0.14 source. The core `pick_list(options, selected,
  on_change)` API is stable.
- **`pick_list` dark theme dropdown overlay.** The menu (dropdown list) that
  appears when the pick_list is clicked may use the system theme by default.
  Custom menu styling may require `.menu_style()` or equivalent. If the
  exact API is not available, the dropdown will work but may not match the
  dark theme until a follow-up fix.
- **Layout compression.** With two dropdowns added, the toolbar may feel
  crowded at narrow window widths. The `Space::new().width(Fill)` spacer
  handles this gracefully -- the dropdowns push against the left-side buttons
  but never overlap (iced clips overflowing content). A future enhancement
  could hide labels at narrow widths.

---

## Phase 6: Config Persistence (S)

### Goal

Save the active data provider name and active broker name to `config.toml`
so they persist across app restarts. Use a new `[providers]` section.

### Config Schema Addition

```toml
[providers]
active_data = "Test Data (Cached)"
active_broker = "None"
```

### Files Modified

| File | Change |
|---|---|
| `desktop/win/crates/midas-core/src/config.rs` | Add `ProviderConfig` struct. Add `pub providers: Option<ProviderConfig>` field to `AppConfig`. |
| `desktop/win/crates/midas-app/src/app/persistence.rs` | Save `providers.active_data` and `providers.active_broker` during config build. |
| `desktop/win/crates/midas-app/src/app.rs` | In `MidasApp::new()`, restore active provider via `set_active_data()` from `config.providers.active_data`. |

### Config Struct

```rust
// In config.rs:

/// Provider selection configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// Display name of the active data provider.
    /// Defaults to "Test Data" if absent or unrecognized.
    pub active_data: Option<String>,
    /// Display name of the active order broker.
    /// Defaults to "None" if absent or unrecognized.
    pub active_broker: Option<String>,
}
```

### Backward Compatibility

The `providers` field in `AppConfig` is `Option<ProviderConfig>` with
`#[serde(default)]`. When loading a config file that was saved before this
change:

- `config.providers` is `None`.
- The app defaults to `active_data_idx = 0` ("Test Data").
- This is the same behavior as before -- no regression.

### Restore Logic in MidasApp::new()

```rust
// After building the registry:
if let Some(ref pc) = config.providers {
    if let Some(ref name) = pc.active_data {
        if let Some(idx) = registry.find_data_provider_index(name) {
            registry.set_active_data(idx);
        } else {
            tracing::warn!("Saved provider '{name}' not found, using default");
        }
    }
    if let Some(ref name) = pc.active_broker {
        if name != "None" {
            if let Some(idx) = registry.find_broker_index(name) {
                registry.set_active_broker(Some(idx));
            }
        }
    }
}
```

### Save Logic in persistence.rs

```rust
// In build_config():
let provider_config = ProviderConfig {
    active_data: Some(self.providers.active_data_provider_name()),
    active_broker: self.providers.active_broker()
        .map(|b| b.name().to_string())
        .or_else(|| Some("None".to_string())),
};
// ... include in AppConfig { providers: Some(provider_config), ... }
```

### Tests Added

```rust
#[test]
fn provider_config_defaults_when_missing() {
    let toml_str = r#"
        [window]
        width = 1200
        height = 800

        [theme]
        name = "dark"
    "#;
    let config: AppConfig = toml::from_str(toml_str).unwrap();
    assert!(config.providers.is_none());
}

#[test]
fn provider_config_round_trip() {
    let config = AppConfig {
        providers: Some(ProviderConfig {
            active_data: Some("Test Data (Cached)".to_string()),
            active_broker: Some("None".to_string()),
        }),
        ..AppConfig::default()
    };
    let toml_str = toml::to_string(&config).unwrap();
    let restored: AppConfig = toml::from_str(&toml_str).unwrap();
    assert_eq!(
        restored.providers.unwrap().active_data.unwrap(),
        "Test Data (Cached)"
    );
}
```

### Gate Criteria

```bash
cargo test -p midas-core         # config round-trip tests pass
cargo test --workspace           # all tests pass
cargo run -p midas-app           # app launches
```

Functional gate:
1. Switch provider to "Test Data (Cached)" via dropdown.
2. Close and restart the app.
3. Dropdown shows "Test Data (Cached)" selected.
4. Status bar shows "Test Data (Cached)" as the active provider.

### Dependencies on Prior Phases

- Phase 4 (ProviderRegistry with `active_data_provider_name()`)
- Phase 5 (UI controls to trigger switching -- but config can be tested
  independently by calling `set_active_data()`)

### Risk Factors

- **Provider name change breaks saved config.** If a provider's `name()`
  changes between releases (e.g., "Test Data" -> "Synthetic Data"), the
  saved config will not match. The fallback (default to index 0) handles
  this gracefully. Using `name()` for persistence is simple and
  human-readable in the TOML file.
- **Config file format compatibility.** The new `[providers]` section is
  additive. Old config files (without the section) load correctly via
  `#[serde(default)]`. New config files loaded by old app versions will
  have the `[providers]` section silently ignored by serde (unknown fields
  are skipped with `#[serde(default)]` on the parent struct).

---

## Summary Table

| Phase | Size | Description | Creates | Modifies | Gate |
|---|---|---|---|---|---|
| 1 | S | Trait definitions + CandleBuffer move | `midas-core/src/provider.rs`, `midas-core/src/candle_buffer.rs` | `midas-core/src/lib.rs`, `midas-data/src/candle.rs`, `midas-data/src/lib.rs`, `Cargo.toml` files | `cargo build -p midas-core` |
| 2 | S | TestProvider wrapper | `midas-feed/src/test_provider.rs` | `midas-feed/src/lib.rs` | `cargo test -p midas-feed` |
| 3 | S | CachingProvider | `midas-store/src/caching_provider.rs` | `midas-store/{src/lib.rs, Cargo.toml}` | `cargo test -p midas-store` |
| 4 | M | Registry + app integration | `midas-app/src/registry.rs` | `midas-app/src/app.rs`, `persistence.rs` | App launches, charts load via provider |
| 5 | M | UI controls | -- | `midas-app/src/app.rs`, `views.rs` | Dropdowns visible, switching works |
| 6 | S | Config persistence | -- | `midas-core/src/config.rs`, `persistence.rs`, `app.rs` | Provider survives restart |

### Dependency Graph

```
Phase 1 ─── Phase 2 ─── Phase 3 ─── Phase 4 ─── Phase 5 ─── Phase 6
  (traits)    (test)     (cache)     (registry)   (UI)       (config)
```

All phases are strictly sequential. No phase can begin until its predecessor's
gate passes. Estimated total: 2-3 focused days.

---

## Rollback Points

Each phase is a logical commit boundary. If a phase introduces a regression:

- **Phase 1:** Revert `provider.rs` additions. No existing code was modified
  (except adding a module declaration).
- **Phase 2:** Revert `test_provider.rs`. The existing `TestDataProvider`
  is untouched.
- **Phase 3:** Revert `caching_provider.rs`. The existing `DbHandle` is
  untouched.
- **Phase 4:** This is the highest-risk phase. Rollback means restoring
  `test_data` and `store` fields in `MidasApp` and reverting
  `load_test_data_for_chart()`. Keep the old code as dead code (behind
  `#[allow(dead_code)]`) until Phase 4's gate passes, then delete.
- **Phase 5:** Revert view changes. The app works without dropdowns.
- **Phase 6:** Revert config changes. Provider defaults to "Test Data" on
  every launch.
