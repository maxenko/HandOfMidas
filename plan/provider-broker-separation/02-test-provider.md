# 02 -- TestProvider: DataProvider Wrapper for TestDataProvider

> Wrapping the existing deterministic test data generator behind the
> `DataProvider` trait for polymorphic data access.
>
> Status: DESIGN SPECIFICATION
> Date: 2026-03-31
>
> **Crate**: `midas-feed` (`desktop/win/crates/midas-feed/`)
> **File**: `src/test_provider.rs` (new) + `src/testdata.rs` (existing, unchanged)
>
> **Prerequisite**: `DataProvider` trait and `ProviderError` must exist in
> `midas-core` (defined in [01-provider-trait.md](01-provider-trait.md)).

---

## 1. Problem Statement

The existing `TestDataProvider` in `midas-feed/src/testdata.rs` is consumed
directly by `MidasApp`:

```rust
// midas-app/src/app.rs (current)
pub struct MidasApp {
    test_data: TestDataProvider,  // concrete type, tightly coupled
    // ...
}
```

`MidasApp::load_test_data_for_chart()` calls `self.test_data.get_candles()`
directly, and `load_symbol_for_chart()` delegates to it. This means:

1. **No polymorphism** -- switching to a different data source requires changing
   `MidasApp` internals.
2. **`&mut self` requirement** -- `TestDataProvider::get_candles()` takes
   `&mut self` because it lazily generates and caches data in a `HashMap`.
   The `DataProvider` trait takes `&self` (for `Arc` sharing).
3. **No parameter mismatch** -- `TestDataProvider::get_candles()` takes a
   `days: u32` parameter (calendar days), and the `DataProvider` trait also
   takes `days: u32` (calendar days). The parameter semantics match directly,
   so no conversion logic is needed.

### Goal

Wrap `TestDataProvider` in a new `TestProvider` struct that implements the
`DataProvider` trait, so that consumers see a uniform interface regardless of
whether data comes from test generation, CSV files, or (future) IB API.

---

## 2. Interior Mutability Strategy

### Why `parking_lot::Mutex`

The `DataProvider` trait requires `&self` for `Arc<dyn DataProvider>` sharing.
`TestDataProvider` needs `&mut self` for its internal `HashMap` cache. We need
interior mutability.

Options considered:

| Approach | Pros | Cons |
|----------|------|------|
| `std::sync::Mutex` | Standard library | Poisoning, slower on Windows |
| `parking_lot::Mutex` | No poisoning, faster, smaller | External dep |
| `tokio::sync::Mutex` | Async-aware, holds across `.await` | Overkill here |
| `RwLock` | Multiple readers | `get_candles` always writes (cache) |

**Decision: `parking_lot::Mutex`.**

Rationale:
- `TestDataProvider::get_candles()` completes in ~1-5ms (CPU-bound data
  generation). It never blocks on I/O. It never crosses an `.await` point.
- `parking_lot::Mutex` is already a workspace dependency.
- No poisoning means we never need to handle `PoisonError`.
- The lock is held for the duration of `get_candles()`, which is short enough
  that contention is negligible even with 20+ charts loading concurrently.

### Why NOT `tokio::sync::Mutex`

`TestDataProvider::get_candles()` is a synchronous, CPU-bound function (~1ms
for daily data, ~5ms for intraday with Brownian bridge generation). It does
not need to hold the lock across `.await` points. Using `tokio::sync::Mutex`
would add unnecessary overhead and complexity. `parking_lot::Mutex` is the
right tool for short, synchronous critical sections.

---

## 3. Parameter Semantics

### Direct Pass-Through

The `DataProvider` trait takes `days: u32` (calendar days), matching the
existing `TestDataProvider::get_candles()` signature exactly. No conversion
logic is needed -- the `TestProvider` wrapper passes `days` straight through
to the inner `TestDataProvider`.

The `days_for_timeframe()` helper in `MidasApp` (which maps timeframes to
appropriate calendar day counts) remains in the app layer where it belongs --
it is a UI concern, not a provider concern.

---

## 4. Complete Implementation

### 4.1 New File: `midas-feed/src/test_provider.rs`

```rust
//! DataProvider trait implementation for the deterministic test data generator.
//!
//! [`TestProvider`] wraps [`TestDataProvider`] with interior mutability to
//! satisfy the `DataProvider` trait's `&self` requirement. The underlying
//! generator is fast (~1-5ms) and purely CPU-bound, so a `parking_lot::Mutex`
//! is used rather than an async-aware lock.

use async_trait::async_trait;
use midas_core::Timeframe;
use midas_core::provider::{DataProvider, ProviderError};
use midas_core::CandleBuffer;
use parking_lot::Mutex;

use crate::testdata::TestDataProvider;

/// DataProvider wrapper around [`TestDataProvider`].
///
/// Always connected, always available. Produces deterministic data for any
/// ticker string. The same ticker always produces identical data across
/// runs (seeded by FNV-1a hash of the ticker name).
///
/// # Thread Safety
///
/// `TestProvider` is `Send + Sync`. The inner `TestDataProvider` is protected
/// by a `parking_lot::Mutex`. Lock contention is minimal because data
/// generation completes in ~1-5ms.
pub struct TestProvider {
    inner: Mutex<TestDataProvider>,
}

impl TestProvider {
    /// Create a new `TestProvider` with an empty cache.
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

#[async_trait::async_trait]
impl DataProvider for TestProvider {
    fn name(&self) -> &str {
        "Test Data"
    }

    fn is_connected(&self) -> bool {
        true // always available, no network dependency
    }

    async fn get_candles(
        &self,
        symbol: &str,
        timeframe: Timeframe,
        days: u32,
    ) -> Result<CandleBuffer, ProviderError> {
        // Lock is held only for the duration of get_candles() (~1-5ms).
        // The `days` parameter is passed straight through -- the trait uses
        // calendar days, matching TestDataProvider's existing API.
        let mut guard = self.inner.lock();
        Ok(guard.get_candles(symbol, timeframe, days))
    }
}
```

### 4.2 Updated `midas-feed/src/lib.rs`

```rust
//! midas-feed: Data import and market data ingest.
//!
//! Depends on: midas-core, midas-data
//!
//! Currently supports CSV import, deterministic test data generation,
//! and the TestProvider (DataProvider trait wrapper).

pub mod csv;
pub mod error;
pub mod test_provider;
pub mod testdata;

pub use csv::import_csv;
pub use error::CsvError;
pub use test_provider::TestProvider;
pub use testdata::TestDataProvider;
```

### 4.3 Updated `midas-feed/Cargo.toml`

The only new dependency is `parking_lot`, which is already a workspace
dependency:

```toml
[dependencies]
midas-core = { path = "../midas-core" }
midas-data = { path = "../midas-data" }

serde     = { workspace = true }
tokio     = { workspace = true }
chrono    = { workspace = true }
thiserror = { workspace = true }
anyhow    = { workspace = true }
tracing   = { workspace = true }
parking_lot = { workspace = true }

# CSV parsing
csv = "1"

# Deterministic RNG for test data generation
rand = "0.8"
```

---

## 5. Behavior Preservation

### 5.1 No Changes to TestDataProvider

The existing `testdata.rs` file is **completely unchanged**. `TestProvider`
wraps it; it does not modify it. All 12 existing tests in `testdata.rs`
continue to pass without modification.

### 5.2 Data Identity

For the same `(symbol, timeframe, days)` inputs, `TestProvider` produces
data identical to calling `TestDataProvider::get_candles()` directly. The
`days` parameter passes straight through with no conversion. The only
difference is the `&self` vs `&mut self` signature (interior mutability via
`parking_lot::Mutex`).

### 5.3 Migration Path in MidasApp

Before (current):

```rust
pub struct MidasApp {
    test_data: TestDataProvider,
    // ...
}

// In load_test_data_for_chart:
let days = match tf.as_secs() { /* ... */ };
let buffer = self.test_data.get_candles(symbol, tf, days);
```

After (with DataProvider trait):

```rust
pub struct MidasApp {
    provider: Arc<dyn DataProvider>,
    // ...
}

// In load_data_for_chart (renamed):
let days = Self::days_for_timeframe(tf);
let buffer = self.provider.get_candles(symbol, tf, days).await?;
```

The `days_for_timeframe()` helper stays in the app layer. The provider
receives `days` directly, matching `TestDataProvider`'s existing API.

---

## 6. Tests

### 6.1 Unit Tests in `test_provider.rs`

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use midas_core::Timeframe;

    // ── TestProvider via DataProvider trait ────────────────────────────

    #[tokio::test]
    async fn test_provider_name() {
        let provider = TestProvider::new();
        assert_eq!(provider.name(), "Test Data");
    }

    #[tokio::test]
    async fn test_provider_always_connected() {
        let provider = TestProvider::new();
        assert!(provider.is_connected());
    }

    #[tokio::test]
    async fn test_provider_returns_data() {
        let provider = TestProvider::new();
        let buf = provider
            .get_candles("AAPL", Timeframe::D1, 730)
            .await
            .unwrap();
        assert!(!buf.is_empty());
        assert!(buf.len() >= 100, "should return a reasonable amount of data");
    }

    #[tokio::test]
    async fn test_provider_deterministic() {
        let p1 = TestProvider::new();
        let p2 = TestProvider::new();

        let a = p1
            .get_candles("AAPL", Timeframe::D1, 365)
            .await
            .unwrap();
        let b = p2
            .get_candles("AAPL", Timeframe::D1, 365)
            .await
            .unwrap();

        assert_eq!(a.len(), b.len());
        for i in 0..a.len() {
            assert_eq!(a.timestamps[i], b.timestamps[i]);
            assert_eq!(a.opens[i], b.opens[i]);
            assert_eq!(a.closes[i], b.closes[i]);
        }
    }

    #[tokio::test]
    async fn test_provider_different_tickers() {
        let provider = TestProvider::new();
        let aapl = provider
            .get_candles("AAPL", Timeframe::D1, 90)
            .await
            .unwrap();
        let tsla = provider
            .get_candles("TSLA", Timeframe::D1, 90)
            .await
            .unwrap();
        assert_ne!(aapl.opens[0], tsla.opens[0]);
    }

    #[tokio::test]
    async fn test_provider_multiple_timeframes() {
        let provider = TestProvider::new();
        for tf in [
            Timeframe::S30,
            Timeframe::M1,
            Timeframe::M5,
            Timeframe::H1,
            Timeframe::D1,
            Timeframe::W1,
        ] {
            let buf = provider.get_candles("MSFT", tf, 30).await.unwrap();
            assert!(!buf.is_empty(), "{tf} returned no data");
        }
    }

    #[tokio::test]
    async fn test_provider_matches_raw_provider() {
        // Verify that TestProvider produces the same closing prices as
        // direct TestDataProvider usage (for the same days parameter).
        let provider = TestProvider::new();
        let mut raw = TestDataProvider::new();

        let tf = Timeframe::D1;
        let days = 365u32;

        let via_trait = provider.get_candles("AAPL", tf, days).await.unwrap();
        let via_raw = raw.get_candles("AAPL", tf, days);

        assert_eq!(via_trait.len(), via_raw.len());
        for i in 0..via_trait.len() {
            assert_eq!(
                via_trait.timestamps[i], via_raw.timestamps[i],
                "timestamp mismatch at index {i}"
            );
            assert_eq!(
                via_trait.closes[i], via_raw.closes[i],
                "close mismatch at index {i}"
            );
        }
    }

    #[tokio::test]
    async fn test_provider_arc_sharing() {
        use std::sync::Arc;

        let provider: Arc<dyn DataProvider> = Arc::new(TestProvider::new());

        let mut handles = Vec::new();
        for _ in 0..5 {
            let p = Arc::clone(&provider);
            handles.push(tokio::spawn(async move {
                p.get_candles("AAPL", Timeframe::D1, 90).await.unwrap()
            }));
        }

        for handle in handles {
            let buf = handle.await.unwrap();
            assert!(buf.len() > 0);
        }
    }
}
```

### 6.2 Test Coverage Summary

| Test | What It Verifies |
|------|------------------|
| `test_provider_name` | Name returns "Test Data" |
| `test_provider_always_connected` | `is_connected()` always true |
| `test_provider_returns_data` | `get_candles` returns non-empty result |
| `test_provider_deterministic` | Two independent providers produce identical data |
| `test_provider_different_tickers` | Different tickers produce different data |
| `test_provider_multiple_timeframes` | All supported timeframes produce data |
| `test_provider_matches_raw_provider` | Trait wrapper produces same data as raw provider |
| `test_provider_arc_sharing` | Works correctly behind `Arc<dyn DataProvider>` |

---

## 7. File Summary

| File | Action | Description |
|------|--------|-------------|
| `midas-feed/src/test_provider.rs` | **New** | `TestProvider` struct, `DataProvider` impl, 8 tests |
| `midas-feed/src/lib.rs` | **Modify** | Add `pub mod test_provider;` and `pub use test_provider::TestProvider;` |
| `midas-feed/Cargo.toml` | **Modify** | Add `parking_lot = { workspace = true }` |
| `midas-feed/src/testdata.rs` | **Unchanged** | No modifications whatsoever |

---

## 8. Design Decisions

### 8.1 Why `TestProvider` Is Separate from `TestDataProvider`

`TestDataProvider` is the data generation engine. It has a clean, synchronous
API that takes `&mut self` and `days: u32`. Changing its API would break 12
existing tests and alter a well-tested module.

`TestProvider` is a thin adapter layer (< 50 lines) that handles:
- Interior mutability (`parking_lot::Mutex`)
- Trait conformance (`DataProvider`)

The `days` parameter passes straight through with no conversion needed.

This separation follows the adapter pattern: the adapter owns the adaptee and
translates its interface without modifying it.

### 8.2 Why `days` in the Trait

The `DataProvider` trait uses `days: u32` (calendar days) because:

1. **Matches existing API** -- `TestDataProvider::get_candles()` already takes
   `days: u32`. Using the same semantics eliminates conversion logic entirely.
2. **Natural for the caller** -- the `days_for_timeframe()` helper in MidasApp
   already computes calendar days based on timeframe. The caller says "give me
   730 days of daily data" which is intuitive.
3. **Provider-independent** -- calendar days are meaningful for every data
   source. IB API, Polygon, and CSV files can all interpret "N calendar days
   of history" appropriately.

---

## 9. Edge Cases

| Case | Behavior |
|------|----------|
| `days = 0` | Returns empty `CandleBuffer` (no data requested) |
| `days > available data` | Returns all available data (less than full range) |
| Empty symbol `""` | `TestDataProvider` generates data for the empty string seed |
| Sub-S30 timeframe | `TestDataProvider` panics (assert). `TestProvider` does not guard against this -- the panic is preserved as a programming error |
| Concurrent access | `parking_lot::Mutex` serializes access; second caller blocks ~1-5ms |

---

## 10. Dependency Graph

```
midas-core (defines DataProvider trait, ProviderError, Timeframe, CandleBuffer)
    ^
    |
midas-data (CandleBuffer extension traits: LOD, mmap, binary)
    ^
    |
midas-feed (defines TestProvider, TestDataProvider)
    |
    +-- parking_lot (Mutex for interior mutability)
```

No new crate dependencies are introduced. `parking_lot` is already in the
workspace dependency table and is used by other crates.
