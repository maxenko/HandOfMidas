# 04 - Provider Registry

## Overview

This document specifies the `ProviderRegistry`, a centralized container for all
data providers and order brokers available to the application. The registry
tracks which provider is active, supports runtime switching, persists the active
selection to `config.toml`, and exposes a listing API for UI dropdowns.

**Prerequisite context:**
- `DataProvider` trait definition (midas-core)
- `CachingProvider` wrapping pattern (midas-store, planned)
- MidasApp iced architecture (`desktop/win/crates/midas-app/src/app.rs`)
- DuckDB store handle (`desktop/win/crates/midas-store/src/handle.rs`)

---

## 1. The DataProvider Trait

Before defining the registry, the trait it stores must be established. This
trait lives in `midas-core` (the leaf crate) so that every crate in the
workspace can depend on it without pulling in heavy provider implementations.

### Location

`desktop/win/crates/midas-core/src/provider.rs` -- new module, re-exported from
`midas-core::provider`.

### Definition

```rust
use std::sync::Arc;
use async_trait::async_trait;
use midas_core::CandleBuffer;
use crate::Timeframe;

/// Errors returned by data providers.
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("provider not connected")]
    NotConnected,
    #[error("unknown symbol: {symbol}")]
    UnknownSymbol { symbol: String },
    #[error("unsupported timeframe: {timeframe}")]
    UnsupportedTimeframe { timeframe: String },
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("internal error: {0}")]
    Internal(String),
    #[error("cache error: {0}")]
    Store(String),
}

/// A source of OHLCV candle data.
///
/// Implementations include:
/// - `TestProvider` (midas-feed) -- implements `DataProvider` from midas-core.
/// - `CachingProvider` (midas-store) -- wraps any `DataProvider` + DuckDB cache.
/// - Future: `IbDataProvider` (midas-broker) -- IB TWS historical data.
/// - Future: `PolygonProvider` -- Polygon.io REST API.
///
/// All methods are async to support network-based providers. Local providers
/// (TestProvider) return immediately from their async implementations.
#[async_trait]
pub trait DataProvider: Send + Sync {
    /// Human-readable name for UI display (e.g., "Test Data", "IB TWS").
    fn name(&self) -> &str;

    /// Whether the provider is currently connected to its data source.
    ///
    /// Local providers (TestProvider) always return `true`.
    /// Network providers return `false` until handshake completes.
    fn is_connected(&self) -> bool;

    /// Fetch candles for a symbol at a given timeframe.
    ///
    /// `days` is the number of calendar days of history to fetch.
    /// Providers may return fewer candles if the symbol has less history.
    async fn get_candles(
        &self,
        symbol: &str,
        timeframe: Timeframe,
        days: u32,
    ) -> Result<CandleBuffer, ProviderError>;
}
```

### Why midas-core

The trait must be in `midas-core` because:

1. `midas-store::CachingProvider` needs to implement `DataProvider` and wraps
   an `Arc<dyn DataProvider>`. Placing the trait in `midas-core` means no
   `midas-store -> midas-feed` dependency is needed.

2. `midas-app` needs the trait for `ProviderRegistry`. Placing it in the leaf
   crate means zero additional edges in the dependency graph.

3. `midas-core` already holds `Timeframe` and `CandleData`. `DataProvider` is
   a natural companion.

**Note on `CandleBuffer`:** The trait references `CandleBuffer`. To avoid a
circular dependency (`midas-core` -> `midas-data` -> `midas-core`), the
`CandleBuffer` struct definition (just the Vec fields and methods) is moved
to `midas-core`. LOD/mmap/binary functionality remains in `midas-data` as
extension traits/functions.

```
midas-core  (leaf: Timeframe, CandleData trait, CandleBuffer struct,
             DataProvider trait, OrderBroker trait, config)
    ^
midas-data  (CandleBuffer extensions: LOD, mmap, binary format)
    ^
midas-feed  (TestProvider, TestDataProvider, CSV import)
    ^
midas-store (CachingProvider, DuckDB cache, DbHandle)
    ^
midas-app   (ProviderRegistry, iced shell)
```

### Location

`desktop/win/crates/midas-core/src/provider.rs` -- trait and error type.
Re-exported from `midas_core::provider::{DataProvider, ProviderError}`.

---

## 2. ProviderRegistry Struct

### Location

`desktop/win/crates/midas-core/src/registry.rs` -- new module.

Wait -- this has the same circularity problem. `ProviderRegistry` holds
`Arc<dyn DataProvider>`, which references `CandleBuffer`. It cannot live in
`midas-core`.

**Final location:** `desktop/win/crates/midas-app/src/registry.rs`.

The registry is an application-level construct. It does not need to be shared
across library crates. It lives in `midas-app` alongside `MidasApp` and
uses `midas_core::provider::DataProvider` as the trait bound.

### Definition

```rust
use std::sync::Arc;
use midas_core::provider::DataProvider;

/// Central registry of all available data providers and order brokers.
///
/// Owned by `MidasApp` (single iced update thread). Providers are `Arc`'d
/// so they can be cloned into `Task::perform` closures for async operations.
///
/// The registry guarantees at least one data provider is always registered
/// (TestProvider is the built-in default). `active_data_idx` is always a
/// valid index into `data_providers`.
pub struct ProviderRegistry {
    /// All registered data providers, in registration order.
    data_providers: Vec<Arc<dyn DataProvider>>,

    /// Index into `data_providers` for the currently active provider.
    /// Invariant: `active_data_idx < data_providers.len()`.
    active_data_idx: usize,

    /// All registered order brokers, in registration order.
    /// Empty until IB integration is implemented.
    order_brokers: Vec<Arc<dyn OrderBroker>>,

    /// Index into `order_brokers` for the currently active broker.
    /// `None` when no broker is connected (paper/sim mode).
    active_broker_idx: Option<usize>,
}
```

### OrderBroker Trait (Stub)

The `OrderBroker` trait is not yet implemented but is included in the registry
struct to establish the dual-provider architecture from day one.

```rust
/// Placeholder for order execution providers (IB TWS, paper trading, etc.).
///
/// Will be defined in a future plan document when IB order integration begins.
/// For now, `order_brokers` is always empty and `active_broker_idx` is `None`.
pub trait OrderBroker: Send + Sync {
    fn name(&self) -> &str;
    fn is_connected(&self) -> bool;
}
```

This trait lives in `midas-core/src/provider.rs` alongside `DataProvider`.
Both traits are exported from `midas_core::provider`.

---

## 3. ProviderRegistry API

### Construction

```rust
impl ProviderRegistry {
    /// Create a new registry with no providers.
    ///
    /// At least one data provider must be registered before calling
    /// `active_data_provider()`. Use `register_data_provider()` immediately after
    /// construction.
    pub fn new() -> Self {
        Self {
            data_providers: Vec::new(),
            active_data_idx: 0,
            order_brokers: Vec::new(),
            active_broker_idx: None,
        }
    }
}
```

### Registration

```rust
impl ProviderRegistry {
    /// Register a data provider.
    ///
    /// The first registered provider becomes the active one by default.
    /// Subsequent registrations do not change the active provider.
    pub fn register_data_provider(&mut self, provider: Arc<dyn DataProvider>) {
        self.data_providers.push(provider);
    }

    /// Register an order broker.
    ///
    /// Does NOT set the active broker — that requires an explicit call to
    /// `set_active_broker()`. Order execution is opt-in.
    pub fn register_order_broker(&mut self, broker: Arc<dyn OrderBroker>) {
        self.order_brokers.push(broker);
    }
}
```

### Switching Active Provider

```rust
impl ProviderRegistry {
    /// Set the active data provider by index.
    ///
    /// Returns `true` if the index was valid and the active provider changed.
    /// Returns `false` if the index is out of range or already active.
    ///
    /// Callers should trigger a reload of all charts when this returns `true`.
    pub fn set_active_data(&mut self, idx: usize) -> bool {
        if idx >= self.data_providers.len() || idx == self.active_data_idx {
            return false;
        }
        self.active_data_idx = idx;
        true
    }

    /// Set the active order broker by index.
    ///
    /// Returns `true` if the index was valid and the active broker changed.
    /// Pass `None` to disconnect from all brokers (paper/sim mode).
    pub fn set_active_broker(&mut self, idx: Option<usize>) -> bool {
        if let Some(i) = idx {
            if i >= self.order_brokers.len() {
                return false;
            }
        }
        if idx == self.active_broker_idx {
            return false;
        }
        self.active_broker_idx = idx;
        true
    }
}
```

### Active Provider Access

```rust
impl ProviderRegistry {
    /// Get the currently active data provider, if any.
    pub fn active_data_provider(&self) -> Option<Arc<dyn DataProvider>> {
        self.data_providers.get(self.active_data_idx).cloned()
    }

    /// Get the currently active order broker, if any.
    pub fn active_broker(&self) -> Option<&Arc<dyn OrderBroker>> {
        self.active_broker_idx
            .map(|idx| &self.order_brokers[idx])
    }

    /// Get the index of the currently active data provider.
    pub fn active_data_index(&self) -> usize {
        self.active_data_idx
    }

    /// Get the index of the currently active broker, if any.
    pub fn active_broker_index(&self) -> Option<usize> {
        self.active_broker_idx
    }
}
```

### Provider Listing for UI

```rust
impl ProviderRegistry {
    /// List all registered data provider names.
    ///
    /// Returns names in registration order, matching the indices used by
    /// `set_active_data()`. Intended for populating a dropdown/picker in
    /// the toolbar.
    pub fn data_provider_names(&self) -> Vec<String> {
        self.data_providers.iter().map(|p| p.name().to_string()).collect()
    }

    /// List all registered order broker names, with "None" prepended.
    pub fn order_broker_names(&self) -> Vec<String> {
        let mut names = vec!["None".to_string()];
        for b in &self.order_brokers {
            names.push(b.name().to_string());
        }
        names
    }

    /// Number of registered data providers.
    pub fn data_provider_count(&self) -> usize {
        self.data_providers.len()
    }

    /// Number of registered order brokers.
    pub fn order_broker_count(&self) -> usize {
        self.order_brokers.len()
    }
}
```

### Provider by Index

```rust
impl ProviderRegistry {
    /// Get a data provider by index (for status checks, etc.).
    pub fn data_provider(&self, idx: usize) -> Option<&Arc<dyn DataProvider>> {
        self.data_providers.get(idx)
    }

    /// Get an order broker by index.
    pub fn order_broker(&self, idx: usize) -> Option<&Arc<dyn OrderBroker>> {
        self.order_brokers.get(idx)
    }

    /// Display name of the active data provider.
    pub fn active_data_provider_name(&self) -> String {
        self.data_providers
            .get(self.active_data_idx)
            .map(|p| p.name().to_string())
            .unwrap_or_else(|| "None".to_string())
    }

    /// Find a data provider's index by display name.
    pub fn find_data_provider_by_name(&self, name: &str) -> Option<usize> {
        self.data_providers
            .iter()
            .position(|p| p.name() == name)
    }

    /// Find a broker's index by display name.
    pub fn find_broker_by_name(&self, name: &str) -> Option<usize> {
        self.order_brokers
            .iter()
            .position(|b| b.name() == name)
    }
}
```

---

## 4. Registration at Startup

Registration happens in `MidasApp::new()` after config is loaded and before
charts are populated with data.

### Registration Sequence

```rust
// In MidasApp::new():

// 1. Create the base data provider (always available).
let test_provider = Arc::new(TestProvider::new());

// 2. Optionally wrap in CachingProvider if DuckDB store is enabled.
let data_provider: Arc<dyn DataProvider> = if config.store.enabled {
    let data_dir = config_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let db_path = data_dir.join(&config.store.path);
    let store_config = midas_store::StoreConfig {
        path: Some(db_path),
        memory_limit_mb: config.store.memory_limit_mb,
        threads: config.store.threads,
    };
    let db_handle = midas_store::DbHandle::open(store_config);
    tracing::info!("DuckDB store enabled, wrapping TestProvider in CachingProvider");
    Arc::new(CachingProvider::new(test_provider.clone(), db_handle))
} else {
    tracing::info!("DuckDB store disabled, using raw TestProvider");
    test_provider.clone()
};

// 3. Build the registry.
let mut registry = ProviderRegistry::new();
registry.register_data_provider(data_provider);

// Future: register additional providers.
// if let Some(ib_config) = config.ib.as_ref() {
//     let ib = Arc::new(IbDataProvider::new(ib_config));
//     registry.register_data_provider(ib);
// }

// 4. Restore active provider from config.
if let Some(saved_name) = config.providers.as_ref().and_then(|p| p.active_data.as_deref()) {
    let names = registry.data_provider_names();
    if let Some(idx) = names.iter().position(|n| n == saved_name) {
        registry.set_active_data(idx);
        tracing::info!("Restored active data provider: {saved_name}");
    } else {
        tracing::warn!(
            "Saved provider '{}' not found, using default '{}'",
            saved_name,
            names.first().map(|s| s.as_str()).unwrap_or("<none>")
        );
    }
}
```

### Registration Order Convention

Providers are registered in a fixed order determined by the startup sequence:

| Index | Provider | Available When |
|-------|----------|---------------|
| 0 | `CachingProvider(TestProvider)` or raw `TestProvider` | Always |
| 1 | `IbDataProvider` | Future: IB TWS config present |
| 2 | `PolygonProvider` | Future: Polygon API key present |

The index is stable within a single run. Between runs, indices may shift if
providers are added or removed from config. Config persistence uses **name**
matching, not index matching.

---

## 5. Provider Switching at Runtime

### Switching Flow

```
User selects "IB TWS" from toolbar dropdown
    |
    v
DataProviderSelected("IB TWS".to_string())
    |
    v
MidasApp::update() handler:
    |
    v
[1] resolve name → index, registry.set_active_data(idx)
    |-- returns false? --> no-op, provider didn't change
    |-- returns true?  -->
    |
    v
[2] Build reload tasks for all charts with data:
    for each chart_id where chart.data.is_some():
        let provider = registry.active_data_provider().unwrap();  // Arc clone
        let symbol = chart.symbol.clone();
        let tf = chart.timeframe;
        let days = days_for_timeframe(tf);
        chart.load_state = LoadState::Loading;
        tasks.push(Task::perform(
            async move { provider.get_candles(&symbol, tf, days).await },
            move |result| Message::DataLoaded(chart_id, result.map(Arc::new).map_err(|e| e.to_string()))
        ));
    |
    v
[3] status_message = format!("Switched to {}", registry.active_data_provider_name())
    |
    v
[4] Mark config dirty (persist new active provider name)
    |
    v
[5] Return Task::batch(tasks)
```

### What Happens During Switch

| Step | Timing | User Sees |
|------|--------|-----------|
| `set_active_data(1)` | Instant | Nothing yet |
| Charts set to `LoadState::Loading` | Same frame | Loading indicators appear |
| `Task::perform` spawned per chart | Same frame | Nothing (async begins) |
| Status message updated | Same frame | Status bar shows "Switched to IB TWS" |
| First `DataLoaded` arrives | ~5ms (test) or ~200ms (IB) | First chart renders |
| All `DataLoaded` arrive | ~50ms (test) or ~2s (IB) | All charts render |

### Edge Cases

**Rapid switching:** If the user switches providers while loads are in flight,
`DataLoaded` messages from the old provider will arrive after the switch.
This is harmless because `DataLoaded` carries the data directly (not a provider
reference). The chart gets the latest data regardless of which provider
produced it. If strict ordering is needed in the future, add a generation
counter to `DataLoaded`.

**Provider disconnected:** If the selected provider reports `is_connected() ==
false`, the switch still proceeds. `get_candles()` will return
`ProviderError::NotConnected`, which the chart handler converts to
`LoadState::Error`. The user sees an error message on each chart. They can
switch back to TestProvider to restore data.

**Same provider re-selected:** `set_active_data()` returns `false` if the
index is unchanged. No reload occurs.

---

## 6. Config Persistence

### New Config Section

Add a `[providers]` section to `config.toml`:

```toml
[providers]
# Name of the active data provider (matched against DataProvider::name()).
# If the named provider is not available at startup, falls back to the first
# registered provider.
active_data = "Test Data (cached)"

# Name of the active order broker. Empty string or absent = no broker.
# active_broker = "IB TWS Paper"
```

### Config Struct

Add to `desktop/win/crates/midas-core/src/config.rs`:

```rust
/// Configuration for provider selection, persisted across sessions.
///
/// Serialized as the `[providers]` section in `config.toml`. Existing
/// configs without this section get defaults via `#[serde(default)]`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// Name of the active data provider.
    ///
    /// Matched against `DataProvider::name()` at startup. If no match is
    /// found (provider removed, renamed, or config from a different machine),
    /// falls back to the first registered provider.
    ///
    /// Default: `None` (use first registered provider).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_data: Option<String>,

    /// Name of the active order broker.
    ///
    /// Default: `None` (no broker connected / paper mode).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_broker: Option<String>,
}
```

### AppConfig Addition

```rust
pub struct AppConfig {
    // ... existing fields ...

    /// Provider selection, persisted across sessions.
    #[serde(default)]
    pub providers: Option<ProviderConfig>,
}
```

Using `Option<ProviderConfig>` with `#[serde(default)]` ensures backward
compatibility: existing `config.toml` files without a `[providers]` section
deserialize successfully. The `Option` is `None`, which the startup code
treats as "use defaults."

### Save Logic

In the config build function (`MidasApp::build_config()`), add:

```rust
fn build_provider_config(&self) -> ProviderConfig {
    ProviderConfig {
        active_data: Some(self.providers.active_data_provider_name()),
        active_broker: self.providers.active_broker().map(|b| b.name().to_string()),
    }
}
```

### Restore Logic

Name-based matching at startup (shown in Section 4 above). The algorithm:

1. Read `config.providers.active_data` (or `None` if absent).
2. Iterate `registry.data_provider_names()` looking for a match.
3. If found, call `registry.set_active_data(idx)`.
4. If not found, log a warning and keep the default (index 0).

This handles:
- First launch (no `[providers]` section): uses default.
- Provider renamed: logs warning, uses default.
- Provider removed from build: logs warning, uses default.
- Normal restore: sets the correct provider silently.

---

## 7. Thread Safety Analysis

### Ownership Model

```
MidasApp (single iced update thread)
    |
    +-- ProviderRegistry (owned, mutable access in update())
         |
         +-- data_providers: Vec<Arc<dyn DataProvider>>
         |       |
         |       +-- Arc<TestProvider>       -- cloned into Task closures
         |       +-- Arc<CachingProvider>    -- cloned into Task closures
         |
         +-- order_brokers: Vec<Arc<dyn OrderBroker>>
                 |
                 +-- (empty for now)
```

### Thread Boundaries

| Context | Thread | Access Pattern |
|---------|--------|---------------|
| `MidasApp::update()` | iced main thread | `&mut self.providers` -- exclusive |
| `MidasApp::view()` | iced main thread | `&self.providers` -- shared |
| `Task::perform` closure | tokio runtime thread | `Arc<dyn DataProvider>` -- shared |
| `DataProvider::get_candles()` | tokio runtime thread | `&self` on the provider |

### Why Arc, Not Rc

`Task::perform` sends the closure to tokio's multi-threaded runtime. The
closure must be `Send + 'static`. `Arc<dyn DataProvider>` is `Send` (because
`DataProvider: Send + Sync`). `Rc` is not `Send` and would fail to compile.

### No Mutex on Registry

The registry itself does not need `Mutex` or `RwLock` because:

1. It is only mutated in `MidasApp::update()`, which runs on a single thread.
2. `view()` takes `&self`, which gives `&ProviderRegistry` -- read-only.
3. The `Arc`'d providers are cloned (cheap reference count bump) before being
   sent to `Task::perform`.
4. Individual providers handle their own internal synchronization (e.g.,
   `CachingProvider` uses `DbHandle` which has an internal mailbox channel).

### Send + Sync Bounds

The `DataProvider` trait requires `Send + Sync`, which means:

- `TestProvider` must be `Send + Sync`. It is -- it holds `HashMap<String,
  CandleBuffer>` (all `Send + Sync` types). The `StdRng` field is `Send`
  but not `Sync`, so `TestProvider` wraps it in interior mutability
  (`Mutex<StdRng>`) or pre-generates data in `new()`.

- `CachingProvider` must be `Send + Sync`. It holds `Arc<dyn DataProvider>`
  (Send + Sync by trait bound) and `DbHandle` (cloneable, channel-based,
  Send + Sync).

- Future `IbDataProvider` will use channels to communicate with the IB
  connection thread, making it naturally `Send + Sync`.

---

## 8. TestProvider Implementation

The `TestProvider` wraps the existing `TestDataProvider` to implement the
`DataProvider` trait. It lives in `midas-feed/src/test_provider.rs`. The trait
itself is defined in `midas-core/src/provider.rs`.

```rust
use parking_lot::Mutex;
use midas_core::CandleBuffer;
use crate::testdata::TestDataProvider;

/// Data provider backed by deterministic test data.
///
/// Always reports as connected. Generates realistic OHLCV data for any
/// ticker symbol instantly (deterministic RNG seeded from the symbol name).
pub struct TestProvider {
    /// Inner provider behind a `parking_lot::Mutex` for `Sync` compatibility.
    /// `TestDataProvider` uses `&mut self` for its `get_candles` method
    /// (it caches generated data internally). The Mutex makes the outer
    /// struct `Sync` while allowing interior mutation.
    inner: Mutex<TestDataProvider>,
}

impl TestProvider {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(TestDataProvider::new()),
        }
    }
}

#[async_trait::async_trait]
impl DataProvider for TestProvider {
    fn name(&self) -> &str {
        "Test Data"
    }

    fn is_connected(&self) -> bool {
        true
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

### Why `parking_lot::Mutex`

`TestDataProvider::get_candles()` takes `&mut self` because it caches generated
data in an internal `HashMap`. The `DataProvider` trait uses `&self` (required
for `Sync`). A `parking_lot::Mutex` bridges the gap. The lock is held for ~2ms
(time to generate or look up data), which is acceptable because:

1. Chart loads are sequential per chart (one `Task::perform` per symbol submit).
2. Even with 20 concurrent loads, `Mutex` contention is negligible compared
   to the data generation cost.
3. `tokio::sync::Mutex` is not needed here -- the lock is held for a
   CPU-bound operation, not across `.await` points.
4. `parking_lot::Mutex` has no poisoning and is faster on Windows.

---

## 9. CachingProvider Implementation

The `CachingProvider` wraps any `DataProvider` with a DuckDB cache layer.
It lives in `midas-store/src/caching.rs`.

```rust
use std::sync::Arc;
use async_trait::async_trait;
use midas_core::CandleBuffer;
use midas_core::provider::{DataProvider, ProviderError};
use midas_core::Timeframe;

use crate::handle::DbHandle;
use crate::types::DataKey;

/// Caching wrapper around any `DataProvider`.
///
/// On `get_candles()`:
/// 1. Check DuckDB cache first.
/// 2. If cache hit (non-empty result), return cached data.
/// 3. If cache miss, delegate to the inner provider.
/// 4. On success, fire-and-forget insert into DuckDB (write-behind).
/// 5. Return the fresh data immediately.
pub struct CachingProvider {
    /// The upstream data source.
    inner: Arc<dyn DataProvider>,
    /// DuckDB cache handle.
    db: DbHandle,
    /// Display name (derived from inner provider name).
    name: String,
}

impl CachingProvider {
    /// Create a new caching provider.
    ///
    /// The `name` is derived as `"{inner.name()} (Cached)"`.
    pub fn new(inner: Arc<dyn DataProvider>, db: DbHandle) -> Self {
        let name = format!("{} (Cached)", inner.name());
        Self { inner, db, name }
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
            symbol: symbol.to_uppercase(),
            timeframe,
        };

        // 1. Try cache first.
        match self.db.query_candles(key.clone()).await {
            Ok(cached) if !cached.is_empty() => {
                tracing::debug!(
                    symbol = %key.symbol,
                    tf = %timeframe,
                    count = cached.len(),
                    "cache hit"
                );
                return Ok(cached);
            }
            Ok(_) => {
                tracing::debug!(symbol = %key.symbol, tf = %timeframe, "cache miss (empty)");
            }
            Err(e) => {
                tracing::warn!(
                    symbol = %key.symbol,
                    tf = %timeframe,
                    error = %e,
                    "cache query failed, falling through to provider"
                );
            }
        }

        // 2. Cache miss — fetch from upstream.
        let buffer = self.inner.get_candles(symbol, timeframe, days).await?;

        // 3. Write-behind: cache the result asynchronously.
        if !buffer.is_empty() {
            let db = self.db.clone();
            let key = key.clone();
            let buffer_clone = buffer.clone();
            let source = self.inner.name().to_string();
            tokio::spawn(async move {
                if let Err(e) = db
                    .fire_and_forget_insert(key, buffer_clone, &source)
                    .await
                {
                    tracing::warn!(error = %e, "write-behind cache insert failed");
                }
            });
        }

        Ok(buffer)
    }
}
```

### Cache Semantics

| Scenario | Behavior |
|----------|----------|
| Cache hit (non-empty) | Return cached data, skip upstream |
| Cache miss (empty result) | Fetch from upstream, cache result |
| Cache query error | Log warning, fetch from upstream |
| Upstream error | Propagate error, do not cache |
| Upstream returns empty | Return empty, do not cache empty data |

### DuckDB Failure Isolation

If DuckDB fails permanently (connection error, disk full, etc.), the
`CachingProvider` degrades gracefully to passthrough mode:

1. Every `query_candles()` call returns an error.
2. The error is caught and logged.
3. The inner provider is called directly.
4. The `fire_and_forget_insert()` call also fails silently.
5. The user sees no difference from using the raw inner provider.

---

## 10. Complete Module Layout

After implementation, the provider-related files are:

```
desktop/win/crates/
├── midas-core/src/
│   ├── provider.rs         # DataProvider trait, OrderBroker trait, ProviderError
│   ├── candle_buffer.rs    # CandleBuffer struct (moved from midas-data)
│   ├── config.rs           # ProviderConfig struct added
│   └── lib.rs              # Re-exports: DataProvider, CandleBuffer, ProviderError
│
├── midas-feed/src/
│   ├── test_provider.rs    # TestProvider (DataProvider impl wrapping TestDataProvider)
│   ├── testdata.rs         # TestDataProvider (existing, unchanged)
│   └── lib.rs              # Re-exports: TestProvider
│
├── midas-store/src/
│   ├── caching_provider.rs # CachingProvider (DataProvider impl wrapping inner + DuckDB)
│   └── lib.rs              # Re-exports: CachingProvider, DbHandle, etc.
│
└── midas-app/src/
    ├── registry.rs          # ProviderRegistry struct
    └── app.rs               # Uses ProviderRegistry (see 05-app-integration.md)
```

### Dependency Additions

| Crate | New Dependency | Reason |
|-------|---------------|--------|
| `midas-core` | `async-trait` | `#[async_trait]` on `DataProvider` and `OrderBroker` |
| `midas-feed` | `async-trait` | `#[async_trait]` on `impl DataProvider for TestProvider` |
| `midas-store` | (none new) | Already depends on `midas-core` (which now has `DataProvider`) |
| `midas-app` | (none new) | Already depends on all crates |

---

## 11. Testing Strategy

### Unit Tests for ProviderRegistry

In `midas-app/src/registry.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// Minimal test provider for registry tests.
    struct MockProvider {
        name: &'static str,
        connected: bool,
    }

    #[async_trait::async_trait]
    impl DataProvider for MockProvider {
        fn name(&self) -> &str { self.name }
        fn is_connected(&self) -> bool { self.connected }
        async fn get_candles(
            &self, _: &str, _: Timeframe, _: u32,
        ) -> Result<CandleBuffer, ProviderError> {
            Ok(CandleBuffer::new())
        }
    }

    fn mock(name: &'static str) -> Arc<dyn DataProvider> {
        Arc::new(MockProvider { name, connected: true })
    }

    #[test]
    fn register_and_list() {
        let mut reg = ProviderRegistry::new();
        reg.register_data_provider(mock("A"));
        reg.register_data_provider(mock("B"));
        assert_eq!(reg.data_provider_names(), vec!["A", "B"]);
    }

    #[test]
    fn active_defaults_to_first() {
        let mut reg = ProviderRegistry::new();
        reg.register_data_provider(mock("First"));
        reg.register_data_provider(mock("Second"));
        assert_eq!(reg.active_data_provider().unwrap().name(), "First");
        assert_eq!(reg.active_data_index(), 0);
    }

    #[test]
    fn switch_active() {
        let mut reg = ProviderRegistry::new();
        reg.register_data_provider(mock("A"));
        reg.register_data_provider(mock("B"));

        assert!(reg.set_active_data(1));
        assert_eq!(reg.active_data_provider().unwrap().name(), "B");
        assert_eq!(reg.active_data_index(), 1);
    }

    #[test]
    fn switch_to_same_returns_false() {
        let mut reg = ProviderRegistry::new();
        reg.register_data_provider(mock("A"));
        assert!(!reg.set_active_data(0));
    }

    #[test]
    fn switch_out_of_range_returns_false() {
        let mut reg = ProviderRegistry::new();
        reg.register_data_provider(mock("A"));
        assert!(!reg.set_active_data(99));
        assert_eq!(reg.active_data_provider().unwrap().name(), "A");
    }

    #[test]
    fn restore_by_name() {
        let mut reg = ProviderRegistry::new();
        reg.register_data_provider(mock("Test Data"));
        reg.register_data_provider(mock("IB TWS"));

        let saved_name = "IB TWS";
        let names = reg.data_provider_names();
        if let Some(idx) = names.iter().position(|n| *n == saved_name) {
            reg.set_active_data(idx);
        }
        assert_eq!(reg.active_data_provider().unwrap().name(), "IB TWS");
    }

    #[test]
    fn restore_missing_name_keeps_default() {
        let mut reg = ProviderRegistry::new();
        reg.register_data_provider(mock("Test Data"));

        let saved_name = "Nonexistent";
        let names = reg.data_provider_names();
        if let Some(idx) = names.iter().position(|n| *n == saved_name) {
            reg.set_active_data(idx);
        }
        // Should still be the default.
        assert_eq!(reg.active_data_provider().unwrap().name(), "Test Data");
    }

    #[test]
    fn broker_none_by_default() {
        let reg = ProviderRegistry::new();
        assert!(reg.active_broker().is_none());
        assert_eq!(reg.order_broker_count(), 0);
    }

    #[test]
    fn provider_count() {
        let mut reg = ProviderRegistry::new();
        assert_eq!(reg.data_provider_count(), 0);
        reg.register_data_provider(mock("A"));
        assert_eq!(reg.data_provider_count(), 1);
    }
}
```

### Integration Tests

Integration tests for `TestProvider` and `CachingProvider` are covered in
their respective crate test modules. The key property to verify:

1. `TestProvider::get_candles()` returns the same data as
   `TestDataProvider::get_candles()` for the same inputs.

2. `CachingProvider` returns data from cache on second call (verify via log
   output or by measuring that no upstream call occurred).

3. `CachingProvider` with a broken `DbHandle` still returns data from the
   inner provider.

---

## 12. Future Extensions

### Provider Health Monitoring

Add a periodic health check that polls `is_connected()` on all providers
and emits `ProviderStatusChanged` messages:

```rust
// In subscription():
fn subscription(&self) -> Subscription<Message> {
    iced::time::every(Duration::from_secs(5)).map(|_| Message::ProviderHealthCheck)
}
```

### Provider-Specific Configuration

Each provider may need its own config section. The `ProviderConfig` struct
can be extended with provider-specific sub-sections:

```toml
[providers]
active_data = "IB TWS"

[providers.ib]
host = "127.0.0.1"
port = 7497
client_id = 1

[providers.polygon]
api_key = "..."
```

### Dynamic Provider Loading

Future providers could be loaded as dynamic libraries (`.dll` / `.so`) via
Rust's `libloading` crate. The registry's `register_data_provider()` API
is already compatible -- it accepts `Arc<dyn DataProvider>`, which can be
constructed from any implementation regardless of where the code lives.

### Multi-Provider Mode

A future `CompositeProvider` could route different symbols to different
providers (e.g., stocks from IB, crypto from a different source). The
registry would need a "routing mode" enum:

```rust
enum RoutingMode {
    /// All symbols go to the active provider.
    Single,
    /// Symbols are routed based on asset class or prefix.
    Composite(Vec<(SymbolPattern, usize)>),
}
```

This is out of scope for the initial implementation.
