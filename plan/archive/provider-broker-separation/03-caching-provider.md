# 03 -- CachingProvider: Transparent DuckDB Cache Layer

> A `DataProvider` decorator that intercepts `get_candles()` calls,
> checks DuckDB first, and falls back to the wrapped inner provider
> on cache miss.
>
> Status: DESIGN SPECIFICATION
> Date: 2026-03-31
>
> **Crate**: `midas-store` (`desktop/win/crates/midas-store/`)
> **File**: `src/caching_provider.rs` (new)
>
> **Prerequisites**:
> - `DataProvider` trait and `ProviderError` in `midas-core`
>   ([01-trait-design.md](01-trait-design.md))
> - `TestProvider` in `midas-feed`
>   ([02-test-provider.md](02-test-provider.md))
> - `DbHandle` in `midas-store` (already implemented)

---

## 1. Problem Statement

Today, caching is manual. `MidasApp` calls `TestDataProvider::get_candles()`
directly, then separately calls `store.fire_and_forget_insert()` to persist
the data. On subsequent launches, it would need to check DuckDB first, then
fall back to the provider. This two-step logic is scattered across
`load_test_data_for_chart()` and `load_symbol_for_chart()`.

### Goal

Make caching automatic and invisible. Any `DataProvider` wrapped in
`CachingProvider` gets free DuckDB persistence without the consumer knowing
or caring.

### Provider Chain Architecture

```
MidasApp sees: Arc<dyn DataProvider>
                    |
                    v
            CachingProvider (name = "Test Data (Cached)")
                    |
        +-----------+-----------+
        |                       |
   DuckDB cache           Inner provider
   (DbHandle)             (TestProvider)

  get_candles("AAPL", D1, 200):
    1. Check DuckDB for AAPL/D1
    2. Cache hit?  -> return cached data
    3. Cache miss? -> call inner.get_candles()
    4. Fire-and-forget insert to DuckDB
    5. Return data to caller
```

The consumer calls `provider.get_candles()` and gets data. It never knows
whether the data came from DuckDB or from the inner provider. The name
exposed via `name()` is `"{inner.name()} (Cached)"` -- the "(Cached)" suffix
enables config persistence to distinguish the cached version. Only the
cached version is registered in the dropdown (not both raw and cached).

---

## 2. Where It Lives

### Why `midas-store`

`CachingProvider` wraps a `DbHandle`, which is defined in `midas-store`. The
dependency direction is already correct:

```
midas-core   (DataProvider trait, Timeframe, ProviderError, CandleBuffer)
    ^
    |
midas-data   (CandleBuffer extensions: LOD, mmap, binary)
    ^
    |
midas-store  (DbHandle, DataKey, CachingProvider)
    |
midas-feed   (TestProvider, TestDataProvider)
```

Wait -- this is wrong. `CachingProvider` wraps `Arc<dyn DataProvider>`, and
`TestProvider` lives in `midas-feed`. But `CachingProvider` does NOT depend
on `midas-feed`. It depends only on:

- `midas-core` (for `DataProvider`, `Timeframe`, `ProviderError`)
- `midas-data` (for `CandleBuffer`)
- `midas-store` itself (for `DbHandle`, `DataKey`)

The `Arc<dyn DataProvider>` is a trait object -- `CachingProvider` does not
know or care that the inner provider is a `TestProvider`. This is the entire
point of the trait abstraction.

So `CachingProvider` lives in `midas-store` with no new dependencies:

```
midas-store/Cargo.toml already has:
  midas-core = { path = "../midas-core" }
  midas-data = { path = "../midas-data" }
```

No circular dependencies. No new crate dependencies.

### Why NOT `midas-feed`

`midas-feed` does not depend on `midas-store`. Adding `midas-store` as a
dependency of `midas-feed` would create unnecessary coupling and pull DuckDB
into the feed crate. The feed crate should remain focused on data sourcing
(CSV, test generation, future IB API), not caching.

---

## 3. Cache Key

`midas-store` already defines `DataKey`:

```rust
// midas-store/src/types.rs (existing)
#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub struct DataKey {
    pub symbol: String,
    pub timeframe: Timeframe,
}
```

This is exactly the right cache key for `CachingProvider`. A `get_candles()`
call for `("AAPL", D1, 200)` maps to:

```rust
DataKey {
    symbol: "AAPL".to_string(),
    timeframe: Timeframe::D1,
}
```

The `days` parameter is NOT part of the cache key. The cache stores all
available data for a symbol/timeframe pair. The provider returns all cached
data or fetches fresh data, and the consumer truncates as needed.

---

## 4. Complete Implementation

### 4.1 New File: `midas-store/src/caching_provider.rs`

```rust
//! Transparent DuckDB caching layer for any [`DataProvider`].
//!
//! [`CachingProvider`] wraps an inner `DataProvider` and a [`DbHandle`],
//! intercepting `get_candles()` to check the DuckDB cache first. On cache
//! miss, it delegates to the inner provider and asynchronously persists the
//! result for future calls.
//!
//! The caching layer appends "(Cached)" to the inner provider's name for
//! config persistence. Consumers cannot distinguish cached from fresh data
//! at the data level -- only the display name differs.

use std::sync::Arc;

use async_trait::async_trait;
use midas_core::provider::{DataProvider, ProviderError};
use midas_core::Timeframe;
use midas_core::CandleBuffer;

use crate::handle::DbHandle;
use crate::types::DataKey;

/// A [`DataProvider`] decorator that adds transparent DuckDB caching.
///
/// # Cache Strategy
///
/// - **Read-through**: `get_candles()` checks DuckDB first. On hit, returns
///   cached data immediately without calling the inner provider.
/// - **Write-behind**: On cache miss, the inner provider is called, data is
///   returned to the caller immediately, and a fire-and-forget insert writes
///   the data to DuckDB asynchronously.
/// - **Stale-while-revalidate (future)**: Currently, cached data is returned
///   unconditionally. TTL-based invalidation is deferred to a future version.
///
/// # Fallback Behavior
///
/// If the DuckDB query fails (connection lost, corruption, etc.), the error
/// is logged via `tracing::warn` and the request falls through to the inner
/// provider. The cache layer never causes a request to fail -- it only adds
/// speed when healthy.
///
/// # Thread Safety
///
/// `CachingProvider` is `Send + Sync`. Both `Arc<dyn DataProvider>` and
/// `DbHandle` are `Clone + Send + Sync`.
pub struct CachingProvider {
    /// The upstream data source (e.g., TestProvider, future IbProvider).
    inner: Arc<dyn DataProvider>,
    /// DuckDB persistent cache handle.
    store: DbHandle,
    /// Display name, auto-derived as `"{inner.name()} (Cached)"`.
    name: String,
}

impl CachingProvider {
    /// Create a new `CachingProvider` wrapping the given provider and store.
    ///
    /// # Arguments
    ///
    /// * `inner` - The upstream data provider to cache results from.
    /// * `store` - A `DbHandle` for DuckDB read/write operations.
    pub fn new(inner: Arc<dyn DataProvider>, store: DbHandle) -> Self {
        let name = format!("{} (Cached)", inner.name());
        Self { inner, store, name }
    }

    /// Build the `DataKey` for a cache lookup.
    fn cache_key(symbol: &str, timeframe: Timeframe) -> DataKey {
        DataKey {
            symbol: symbol.to_uppercase(),
            timeframe,
        }
    }
}

#[async_trait::async_trait]
impl DataProvider for CachingProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn is_connected(&self) -> bool {
        // The caching layer does not affect connectivity semantics.
        // If the inner provider is connected, so is the caching wrapper.
        self.inner.is_connected()
    }

    async fn get_candles(
        &self,
        symbol: &str,
        timeframe: Timeframe,
        days: u32,
    ) -> Result<CandleBuffer, ProviderError> {
        let key = Self::cache_key(symbol, timeframe);

        // ── Step 1: Check DuckDB cache ────────────────────────────────
        match self.store.query_candles(key.clone()).await {
            Ok(cached) if !cached.is_empty() => {
                tracing::debug!(
                    symbol = symbol,
                    timeframe = %timeframe,
                    cached_len = cached.len(),
                    "cache hit"
                );
                return Ok(cached);
            }
            Ok(_empty) => {
                tracing::debug!(
                    symbol = symbol,
                    timeframe = %timeframe,
                    "cache miss (empty result)"
                );
            }
            Err(e) => {
                // DuckDB query failed -- log and fall through to inner provider.
                // The cache layer must NEVER cause a request to fail.
                tracing::warn!(
                    symbol = symbol,
                    timeframe = %timeframe,
                    error = %e,
                    "cache query failed, falling through to inner provider"
                );
            }
        }

        // ── Step 2: Fetch from inner provider ─────────────────────────
        let buffer = self.inner.get_candles(symbol, timeframe, days).await?;

        // ── Step 3: Fire-and-forget persist to DuckDB ─────────────────
        if !buffer.is_empty() {
            let source = self.inner.name().to_string();
            if let Err(e) = self
                .store
                .fire_and_forget_insert(key, buffer.clone(), &source)
                .await
            {
                // Write failure is non-fatal. Data is already in memory
                // and will be returned to the caller.
                tracing::warn!(
                    symbol = symbol,
                    timeframe = %timeframe,
                    error = %e,
                    "cache write failed (non-fatal)"
                );
            }
        }

        // ── Step 4: Return fresh data ─────────────────────────────────
        Ok(buffer)
    }
}
```

### 4.2 Updated `midas-store/src/lib.rs`

```rust
//! midas-store: DuckDB-backed persistent cache for historical candle data.
//!
//! All DuckDB operations run on a dedicated OS thread via a mailbox actor.
//! The public [`DbHandle`] communicates with this thread through async
//! message passing, keeping blocking C++ FFI calls off the tokio threadpool.

mod actor;
mod convert;
mod error;
mod handle;
mod queries;
mod schema;
mod types;

pub mod caching_provider;

pub use caching_provider::CachingProvider;
pub use error::StoreError;
pub use handle::DbHandle;
pub use types::{CacheInfo, DataKey, StoreConfig};
```

### 4.3 Updated `midas-store/Cargo.toml`

No changes needed. `midas-store` already depends on `midas-core` and
`midas-data`, which is all `CachingProvider` requires. The `DataProvider`
trait and `ProviderError` come from `midas-core`.

```toml
# Existing Cargo.toml -- NO CHANGES REQUIRED
[dependencies]
midas-core = { path = "../midas-core" }
midas-data = { path = "../midas-data" }
mailbox_processor = { path = "../mailbox_processor" }
duckdb = { version = "1", features = ["bundled"] }
tokio = { workspace = true }
thiserror = { workspace = true }
tracing = { workspace = true }
```

---

## 5. Cache Behavior

### 5.1 Cache Hit (Second Launch)

```
Timeline:

  First launch:
    get_candles("AAPL", D1, 200)
      -> DuckDB query: empty result (miss)
      -> inner.get_candles() -> 200 candles from TestProvider
      -> fire_and_forget_insert to DuckDB
      -> return 200 candles to caller

  Second launch:
    get_candles("AAPL", D1, 200)
      -> DuckDB query: 200 candles (hit!)
      -> return cached data immediately
      -> inner provider never called
```

### 5.2 Cache Miss (New Symbol)

```
  get_candles("TSLA", D1, 200)
    -> DuckDB query: empty (never cached)
    -> inner.get_candles("TSLA", D1, 200) -> 200 candles
    -> fire_and_forget_insert to DuckDB
    -> return 200 candles
```

### 5.3 DuckDB Failure (Graceful Fallback)

```
  get_candles("AAPL", D1, 200)
    -> DuckDB query: Err(ChannelClosed) -- actor died
    -> tracing::warn("cache query failed...")
    -> inner.get_candles("AAPL", D1, 200) -> 200 candles
    -> fire_and_forget_insert: Err (also fails) -> tracing::warn
    -> return 200 candles
    -> App works fine, just no caching
```

### 5.4 Inner Provider Failure

```
  get_candles("AAPL", D1, 200)
    -> DuckDB query: empty (miss)
    -> inner.get_candles() -> Err(ProviderError::...)
    -> return Err(ProviderError::...) to caller
    -> No DuckDB write attempted
```

### 5.5 Empty Data

```
  get_candles("???", D1, 200)
    -> DuckDB query: empty (miss)
    -> inner.get_candles() -> Ok(empty CandleBuffer)
    -> buffer.is_empty() == true, skip DuckDB write
    -> return Ok(empty CandleBuffer)
```

---

## 6. Stale Data Policy

### Current (v1): Always Return Cache

If DuckDB has data for a symbol/timeframe pair, it is returned regardless of
age. This is correct for the current use case:

- `TestDataProvider` is deterministic. The same ticker always produces
  identical data. Cached data never becomes "stale" because the source data
  is immutable.
- On first real data provider (IB API), stale data is still useful as a fast
  first paint. The chart shows cached data immediately while fresh data is
  fetched in the background.

### Future (v2): TTL-Based Invalidation

When live data providers are added, the cache will need invalidation:

```rust
/// Future: CachingProvider with TTL support.
pub struct CachingProvider {
    inner: Arc<dyn DataProvider>,
    store: DbHandle,
    ttl: Option<Duration>,  // None = never expire (v1 behavior)
}
```

Invalidation strategy (deferred):
- Store `inserted_at` timestamp in `data_ranges` table (already exists in
  schema as `updated_at`).
- On cache hit, check `now - updated_at > ttl`. If stale, treat as miss.
- Consider timeframe-dependent TTL: D1 data stale after 24h, M5 data stale
  after 1h.

This is NOT implemented in v1. Documenting here to show the extension point
is clean.

---

## 7. Days vs. Full Cache

### The Days Mismatch

`get_candles()` takes a `days` parameter, but the DuckDB cache stores ALL
data that was ever fetched for a symbol/timeframe pair. This means:

- **First call**: `get_candles("AAPL", D1, 730)` fetches ~2 years of candles
  from the inner provider and stores them in DuckDB.
- **Second call**: `get_candles("AAPL", D1, 90)` returns ALL cached
  candles from DuckDB, not just 90 days.

This is intentional. The cache serves as a persistent data store, not a
request-level cache. Benefits:

1. If the user later requests 300 candles after previously requesting 200,
   the cache can serve the 200 immediately (partial hit) rather than missing
   entirely.
2. The chart renderer already handles variable-length data. Getting more
   candles than requested is harmless -- the renderer only displays what fits
   in the viewport.

### Future Enhancement: Range-Aware Caching

For production data providers, the cache could be range-aware:

```rust
// Future: check if cached range covers the request
let cached = store.query_candles(key).await?;
if !cached.is_empty() {
    return Ok(cached);  // sufficient data cached
}
// Miss: fetch from provider
```

This is deferred. For test data (deterministic, always the same), returning
the full cache is correct behavior.

---

## 8. Provider Chain Construction

### At App Startup

```rust
// In MidasApp::new() or equivalent setup code:

use std::sync::Arc;
use midas_feed::TestProvider;
use midas_store::{CachingProvider, DbHandle, StoreConfig};

// 1. Create the inner data provider.
let test_provider = Arc::new(TestProvider::new());

// 2. Create the DuckDB store.
let store = DbHandle::open(StoreConfig::default());

// 3. Wrap with caching layer.
let provider: Arc<dyn DataProvider> = Arc::new(
    CachingProvider::new(test_provider, store.clone())
);

// 4. App only sees Arc<dyn DataProvider>.
let app = MidasApp {
    provider,
    store: Some(store),  // retained for shutdown, diagnostics
    // ...
};
```

### Without DuckDB (Fallback)

```rust
// If DuckDB is disabled or failed to open:
let provider: Arc<dyn DataProvider> = Arc::new(TestProvider::new());

let app = MidasApp {
    provider,
    store: None,
    // ...
};
```

The app code that calls `provider.get_candles()` is identical in both cases.

### Composability

The decorator pattern allows arbitrary stacking:

```rust
// Future: logging + caching + rate limiting
let inner = Arc::new(IbProvider::new(connection));
let cached = Arc::new(CachingProvider::new(inner, store));
let logged = Arc::new(LoggingProvider::new(cached));    // future
let limited = Arc::new(RateLimitedProvider::new(logged)); // future
let provider: Arc<dyn DataProvider> = limited;
```

Each layer implements `DataProvider` and wraps the next. This is a
well-established pattern (Tower middleware, HTTP middleware chains).

---

## 9. Tests

### 9.1 Unit Tests in `caching_provider.rs`

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use midas_core::Timeframe;
    use midas_core::CandleBuffer;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    // ── Mock DataProvider for testing ─────────────────────────────────

    /// A mock DataProvider that counts calls and returns deterministic data.
    struct MockProvider {
        call_count: AtomicU32,
        name: &'static str,
    }

    impl MockProvider {
        fn new() -> Self {
            Self {
                call_count: AtomicU32::new(0),
                name: "Mock",
            }
        }

        fn calls(&self) -> u32 {
            self.call_count.load(Ordering::Relaxed)
        }
    }

    impl DataProvider for MockProvider {
        fn name(&self) -> &str {
            self.name
        }

        fn is_connected(&self) -> bool {
            true
        }

        async fn get_candles(
            &self,
            _symbol: &str,
            _timeframe: Timeframe,
            days: u32,
        ) -> Result<CandleBuffer, ProviderError> {
            self.call_count.fetch_add(1, Ordering::Relaxed);

            let mut buf = CandleBuffer::with_capacity(days as usize);
            for i in 0..days {
                let ts = 1_700_000_000_000i64 + (i as i64 * 86_400_000);
                let price = 150.0 + (i as f32 * 0.5);
                buf.push(ts, price, price + 2.0, price - 1.0, price + 0.5, 1000 + i);
            }
            Ok(buf)
        }
    }

    /// A mock DataProvider that always fails.
    struct FailingProvider;

    impl DataProvider for FailingProvider {
        fn name(&self) -> &str {
            "Failing"
        }

        fn is_connected(&self) -> bool {
            false
        }

        async fn get_candles(
            &self,
            _symbol: &str,
            _timeframe: Timeframe,
            _days: u32,
        ) -> Result<CandleBuffer, ProviderError> {
            Err(ProviderError::NotConnected)
        }
    }

    // ── Helper ────────────────────────────────────────────────────────

    fn make_caching_provider(inner: Arc<dyn DataProvider>) -> CachingProvider {
        let store = DbHandle::open_memory();
        CachingProvider::new(inner, store)
    }

    // ── Tests ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn name_appends_cached_suffix() {
        let inner = Arc::new(MockProvider::new());
        let cp = make_caching_provider(inner);
        assert_eq!(cp.name(), "Mock (Cached)");
    }

    #[tokio::test]
    async fn is_connected_delegates_to_inner() {
        let connected = Arc::new(MockProvider::new());
        let cp = make_caching_provider(connected);
        assert!(cp.is_connected());

        let disconnected: Arc<dyn DataProvider> = Arc::new(FailingProvider);
        let cp2 = make_caching_provider(disconnected);
        assert!(!cp2.is_connected());
    }

    #[tokio::test]
    async fn first_call_is_cache_miss() {
        let inner = Arc::new(MockProvider::new());
        let cp = make_caching_provider(Arc::clone(&inner) as Arc<dyn DataProvider>);

        let buf = cp
            .get_candles("AAPL", Timeframe::D1, 50)
            .await
            .unwrap();

        assert_eq!(buf.len(), 50);
        assert_eq!(inner.calls(), 1, "inner provider should be called once");
    }

    #[tokio::test]
    async fn second_call_is_cache_hit() {
        let inner = Arc::new(MockProvider::new());
        let store = DbHandle::open_memory();
        let cp = CachingProvider::new(
            Arc::clone(&inner) as Arc<dyn DataProvider>,
            store,
        );

        // First call: cache miss, calls inner.
        let buf1 = cp
            .get_candles("AAPL", Timeframe::D1, 50)
            .await
            .unwrap();
        assert_eq!(inner.calls(), 1);

        // Allow fire-and-forget insert to complete.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Second call: cache hit, inner NOT called.
        let buf2 = cp
            .get_candles("AAPL", Timeframe::D1, 50)
            .await
            .unwrap();
        assert_eq!(inner.calls(), 1, "inner should not be called on cache hit");

        // Data should be identical.
        assert_eq!(buf1.len(), buf2.len());
        assert_eq!(buf1.timestamps[0], buf2.timestamps[0]);
        assert_eq!(buf1.closes[0], buf2.closes[0]);
    }

    #[tokio::test]
    async fn different_symbols_are_separate_cache_entries() {
        let inner = Arc::new(MockProvider::new());
        let store = DbHandle::open_memory();
        let cp = CachingProvider::new(
            Arc::clone(&inner) as Arc<dyn DataProvider>,
            store,
        );

        cp.get_candles("AAPL", Timeframe::D1, 50).await.unwrap();
        cp.get_candles("TSLA", Timeframe::D1, 50).await.unwrap();

        assert_eq!(inner.calls(), 2, "each symbol triggers a separate fetch");
    }

    #[tokio::test]
    async fn different_timeframes_are_separate_cache_entries() {
        let inner = Arc::new(MockProvider::new());
        let store = DbHandle::open_memory();
        let cp = CachingProvider::new(
            Arc::clone(&inner) as Arc<dyn DataProvider>,
            store,
        );

        cp.get_candles("AAPL", Timeframe::D1, 50).await.unwrap();
        cp.get_candles("AAPL", Timeframe::H1, 50).await.unwrap();

        assert_eq!(inner.calls(), 2, "each timeframe triggers a separate fetch");
    }

    #[tokio::test]
    async fn inner_error_propagates() {
        let failing: Arc<dyn DataProvider> = Arc::new(FailingProvider);
        let cp = make_caching_provider(failing);

        let result = cp.get_candles("AAPL", Timeframe::D1, 50).await;
        assert!(result.is_err(), "inner provider error should propagate");
    }

    #[tokio::test]
    async fn empty_result_not_cached() {
        let inner = Arc::new(MockProvider::new());
        let store = DbHandle::open_memory();
        let cp = CachingProvider::new(
            Arc::clone(&inner) as Arc<dyn DataProvider>,
            store.clone(),
        );

        // Request 0 candles -> empty buffer -> should NOT be cached.
        let buf = cp.get_candles("AAPL", Timeframe::D1, 0).await.unwrap();
        assert!(buf.is_empty());

        // Verify nothing was written to DuckDB.
        let key = CachingProvider::cache_key("AAPL", Timeframe::D1);
        let cached = store.query_candles(key).await.unwrap();
        assert!(cached.is_empty());
    }

    #[tokio::test]
    async fn symbol_case_normalized() {
        let inner = Arc::new(MockProvider::new());
        let store = DbHandle::open_memory();
        let cp = CachingProvider::new(
            Arc::clone(&inner) as Arc<dyn DataProvider>,
            store,
        );

        cp.get_candles("aapl", Timeframe::D1, 50).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // "AAPL" should hit cache because cache_key normalizes to uppercase.
        cp.get_candles("AAPL", Timeframe::D1, 50).await.unwrap();
        assert_eq!(
            inner.calls(), 1,
            "case-insensitive cache lookup should hit"
        );
    }

    #[tokio::test]
    async fn concurrent_access_works() {
        let inner = Arc::new(MockProvider::new());
        let store = DbHandle::open_memory();
        let cp = Arc::new(CachingProvider::new(
            Arc::clone(&inner) as Arc<dyn DataProvider>,
            store,
        ));

        let mut handles = Vec::new();
        for i in 0..5 {
            let provider = Arc::clone(&cp);
            let symbol = format!("SYM{i}");
            handles.push(tokio::spawn(async move {
                provider
                    .get_candles(&symbol, Timeframe::D1, 20)
                    .await
                    .unwrap()
                    .len()
            }));
        }

        for handle in handles {
            let len = handle.await.unwrap();
            assert_eq!(len, 20);
        }

        assert_eq!(inner.calls(), 5);
    }
}
```

### 9.2 Integration Test with Real TestProvider

This test belongs in `midas-store/tests/` or as a workspace-level integration
test, since it requires both `midas-feed` and `midas-store`:

```rust
// tests/caching_integration.rs (workspace root level)
use std::sync::Arc;
use midas_core::Timeframe;
use midas_core::provider::DataProvider;
use midas_feed::TestProvider;
use midas_store::{CachingProvider, DbHandle, StoreConfig};

#[tokio::test]
async fn caching_provider_with_test_data_roundtrip() {
    let test_provider = Arc::new(TestProvider::new());
    let store = DbHandle::open(StoreConfig::memory());
    let provider = CachingProvider::new(
        test_provider as Arc<dyn DataProvider>,
        store.clone(),
    );

    // First call: cache miss, generates from TestProvider.
    let buf1 = provider.get_candles("AAPL", Timeframe::D1, 200).await.unwrap();
    assert!(!buf1.is_empty());

    // Wait for fire-and-forget write.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Second call: cache hit, loads from DuckDB.
    let buf2 = provider.get_candles("AAPL", Timeframe::D1, 200).await.unwrap();
    assert_eq!(buf1.len(), buf2.len());

    // Data should be identical (same source, same cache).
    for i in 0..buf1.len() {
        assert_eq!(buf1.timestamps[i], buf2.timestamps[i]);
        assert_eq!(buf1.opens[i], buf2.opens[i]);
        assert_eq!(buf1.closes[i], buf2.closes[i]);
    }

    store.shutdown().await.unwrap();
}
```

### 9.3 Test Coverage Summary

| Test | What It Verifies |
|------|------------------|
| `name_appends_cached_suffix` | `name()` returns `"{inner.name()} (Cached)"` |
| `is_connected_delegates_to_inner` | `is_connected()` reflects inner state |
| `first_call_is_cache_miss` | First call fetches from inner provider |
| `second_call_is_cache_hit` | Second call returns DuckDB data, skips inner |
| `different_symbols_are_separate_cache_entries` | Symbol isolation in cache |
| `different_timeframes_are_separate_cache_entries` | Timeframe isolation |
| `inner_error_propagates` | Provider errors pass through unchanged |
| `empty_result_not_cached` | Empty buffers are not persisted |
| `symbol_case_normalized` | Cache keys are case-insensitive |
| `concurrent_access_works` | Multiple concurrent requests succeed |
| `caching_provider_with_test_data_roundtrip` | End-to-end with real provider |

---

## 10. Error Handling

### 10.1 ProviderError

The `DataProvider` trait returns `Result<CandleBuffer, ProviderError>`.
`CachingProvider` introduces no new error variants. All errors come from
either the inner provider (returned as-is) or DuckDB (logged and swallowed).

### 10.2 DuckDB Errors Are Never Propagated

This is a critical design decision. The cache is an optimization, not a
requirement. If DuckDB fails:

- **Query failure**: Logged via `tracing::warn`, falls through to inner
  provider. The user sees correct data, just slower.
- **Insert failure**: Logged via `tracing::warn`, data is already in memory
  and returned. Next request will try to populate the cache again.
- **Channel closed (actor crashed)**: Same as query failure -- fall through.

The cache layer adheres to the principle: **the presence or absence of caching
must not change the observable behavior of the system** (excluding
performance).

### 10.3 Inner Provider Errors DO Propagate

If the inner provider returns `Err(ProviderError::...)`, the `CachingProvider`
returns that error to the caller. The cache cannot help -- it had no data
(that is why the inner provider was called in the first place).

---

## 11. Performance Characteristics

| Operation | Latency | Notes |
|-----------|---------|-------|
| Cache hit (DuckDB query) | ~1-5ms | Depends on candle count |
| Cache miss (inner + write-behind) | ~5-10ms | TestProvider ~1-5ms + fire-and-forget |
| Fire-and-forget DuckDB insert | ~5-20ms | Runs asynchronously, does not block caller |
| Cache hit on second launch | ~5ms | DuckDB cold query |

The overhead of checking DuckDB on every `get_candles()` call is ~1-5ms. For
the typical use case (loading data on symbol change or app startup), this is
negligible compared to the chart rendering cost.

---

## 12. File Summary

| File | Action | Description |
|------|--------|-------------|
| `midas-store/src/caching_provider.rs` | **New** | `CachingProvider` struct, `DataProvider` impl, 10 unit tests |
| `midas-store/src/lib.rs` | **Modify** | Add `pub mod caching_provider;` and `pub use caching_provider::CachingProvider;` |
| `midas-store/Cargo.toml` | **Unchanged** | No new dependencies |
| `tests/caching_integration.rs` | **New** | Integration test with real TestProvider + DuckDB |

---

## 13. Design Decisions Summary

### 13.1 Decorator Over Composition

`CachingProvider` implements `DataProvider` and wraps another `DataProvider`.
This is the decorator pattern. The alternative (having `MidasApp` manage
cache logic inline) was rejected because:

- It couples caching knowledge to the app layer.
- It must be duplicated for every place that calls `get_candles()`.
- It cannot be tested in isolation.

### 13.2 `fire_and_forget_insert` Over `insert_candles`

Write-behind (fire-and-forget) is used instead of awaited insert because:

- The caller already has the data in memory. It does not need to wait for
  DuckDB to persist it.
- Awaiting the insert would add 5-20ms latency to every cache miss.
- `DbHandle::fire_and_forget_insert()` already exists and is tested.

### 13.3 `query_candles` (Full) Over `query_candles_range`

The cache uses `query_candles()` (returns all data for a key) rather than
`query_candles_range()` (returns data in a time window). This is simpler
and correct for v1:

- Test data is static -- there is no concept of "recent data only."
- The chart renderer handles variable-length data; extra candles are harmless.
- Range queries add complexity (what time range to request? how to handle
  partial hits?) that is deferred to v2.

### 13.4 Symbol Case Normalization

`cache_key()` converts the symbol to uppercase. This ensures that
`get_candles("aapl", ...)` and `get_candles("AAPL", ...)` hit the same cache
entry. The `TestDataProvider` is case-sensitive (different seeds for different
strings), so we normalize before the cache lookup AND before passing to the
inner provider to maintain consistency.

Note: the inner provider receives the original (non-normalized) symbol from
the `get_candles()` parameter. If the inner provider is also case-sensitive,
a second normalization should happen there. `TestProvider` in doc 02 does not
normalize internally -- the `MidasApp` already uppercases symbols before
calling `get_candles()`. This is acceptable for v1 but should be revisited
when adding production providers.

---

## 14. Future Extensions

| Extension | Complexity | When |
|-----------|-----------|------|
| TTL-based cache invalidation | M | When live data providers are added |
| Range-aware partial cache hits | L | When fetching incremental data updates |
| Cache eviction (LRU by symbol) | S | When DuckDB database size becomes a concern |
| Cache warming at startup | S | When loading many charts from config |
| Cache statistics (hit rate) | S | When performance monitoring is added |

None of these require changes to the `DataProvider` trait. They are all
internal to `CachingProvider`.
