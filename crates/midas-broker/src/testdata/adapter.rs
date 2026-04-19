//! `MarketDataSource` implementation for `TestDataProvider`.

use midas_broker_core::SymbolKey;

use crate::error::BrokerError;
use crate::market_data::{HistoricalBarsResult, MarketDataSource};

use super::TestDataProvider;

impl MarketDataSource for TestDataProvider {
    fn historical_bars(
        &mut self,
        symbol: &str,
        con_id: i32,
        timeframe: midas_broker_core::Timeframe,
        start: i64,
        end: i64,
        request_id: u64,
    ) -> Result<HistoricalBarsResult, BrokerError> {
        // Guard: TestDataProvider panics below S30
        if timeframe.as_secs() < 30 {
            return Err(BrokerError::Config(
                "test data source minimum resolution is S30 (30 secs)".to_string(),
            ));
        }

        let bars = self.bars(symbol, timeframe, start, end);

        Ok(HistoricalBarsResult {
            symbol: SymbolKey {
                contract_id: con_id,
                symbol: symbol.to_string(),
            },
            request_id,
            bars,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use midas_broker_core::Timeframe;

    #[test]
    fn adapter_returns_bars() {
        let mut provider = TestDataProvider::new();
        let (start, end) = provider.date_range("AAPL");
        let result = provider
            .historical_bars("AAPL", 265598, Timeframe::D1, start, end, 1)
            .unwrap();
        assert!(!result.bars.is_empty());
        assert_eq!(result.symbol.symbol, "AAPL");
        assert_eq!(result.symbol.contract_id, 265598);
        assert_eq!(result.request_id, 1);
    }

    #[test]
    fn adapter_rejects_sub_s30() {
        let mut provider = TestDataProvider::new();
        let result = provider.historical_bars("AAPL", 1, Timeframe::S1, 0, i64::MAX, 1);
        assert!(result.is_err());
    }

    #[test]
    fn adapter_empty_for_out_of_range() {
        let mut provider = TestDataProvider::new();
        // Far future — no data
        let result = provider
            .historical_bars("AAPL", 1, Timeframe::D1, 2_000_000_000, 2_100_000_000, 1)
            .unwrap();
        assert!(result.bars.is_empty());
    }
}
