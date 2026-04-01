//! DataProvider trait implementation for the deterministic test data generator.
//!
//! [`TestProvider`] wraps [`TestDataProvider`] with interior mutability to
//! satisfy the `DataProvider` trait's `&self` requirement. The underlying
//! generator is fast (~1-5ms) and purely CPU-bound, so a `parking_lot::Mutex`
//! is used rather than an async-aware lock.

use async_trait::async_trait;
use midas_core::provider::{DataProvider, ProviderError};
use midas_core::{CandleBuffer, Timeframe};
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

#[async_trait]
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
        let mut guard = self.inner.lock();
        Ok(guard.get_candles(symbol, timeframe, days))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use midas_core::Timeframe;

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

        let a = p1.get_candles("AAPL", Timeframe::D1, 365).await.unwrap();
        let b = p2.get_candles("AAPL", Timeframe::D1, 365).await.unwrap();

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
        let aapl = provider.get_candles("AAPL", Timeframe::D1, 90).await.unwrap();
        let tsla = provider.get_candles("TSLA", Timeframe::D1, 90).await.unwrap();
        assert_ne!(aapl.opens[0], tsla.opens[0]);
    }

    #[tokio::test]
    async fn test_provider_multiple_timeframes() {
        let provider = TestProvider::new();
        for tf in [
            Timeframe::S30, Timeframe::M1, Timeframe::M5,
            Timeframe::H1, Timeframe::D1, Timeframe::W1,
        ] {
            let buf = provider.get_candles("MSFT", tf, 30).await.unwrap();
            assert!(!buf.is_empty(), "{tf} returned no data");
        }
    }

    #[tokio::test]
    async fn test_provider_matches_raw_provider() {
        let provider = TestProvider::new();
        let mut raw = TestDataProvider::new();

        let via_trait = provider.get_candles("AAPL", Timeframe::D1, 365).await.unwrap();
        let via_raw = raw.get_candles("AAPL", Timeframe::D1, 365);

        assert_eq!(via_trait.len(), via_raw.len());
        for i in 0..via_trait.len() {
            assert_eq!(via_trait.timestamps[i], via_raw.timestamps[i], "timestamp mismatch at index {i}");
            assert_eq!(via_trait.closes[i], via_raw.closes[i], "close mismatch at index {i}");
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
