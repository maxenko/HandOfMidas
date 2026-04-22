//! Legacy pre-router `MarketDataSource` trait — historical-bars only.
//!
//! **OPEN (post-refactor):** this trait, [`HistoricalBarsResult`], and
//! the [`TestDataProvider`](crate::testdata::TestDataProvider) impl in
//! `testdata/adapter.rs` have no surviving consumers after slice 10g
//! deleted the standalone `ib_data_source.rs` adapter. The new
//! [`MarketDataSource`](crate::market_data_source::MarketDataSource)
//! trait (streaming, async, driven by the router) owns every live
//! code path. Deletion is a mechanical follow-up — left in place
//! this commit because the file lives outside the slice's explicit
//! deletion list (ib_client, ib_data_source) and the task plan
//! reserves that scope.

use midas_broker_core::{OhlcvBar, SymbolKey, Timeframe};

use crate::error::BrokerError;

/// Result from a historical data request.
pub struct HistoricalBarsResult {
    pub symbol: SymbolKey,
    pub request_id: u64,
    pub bars: Vec<OhlcvBar>,
}

/// Abstraction over market data providers.
///
/// The broker engine holds a `Box<dyn MarketDataSource>` and dispatches
/// `RequestHistoricalData` commands through it. Both `TestDataProvider`
/// and the future IB adapter implement this trait.
pub trait MarketDataSource: Send {
    /// Fetch historical bars for the given symbol and time range.
    ///
    /// `timeframe` and `(start, end)` are already parsed from IB-style
    /// duration/bar_size strings by the caller.
    fn historical_bars(
        &mut self,
        symbol: &str,
        con_id: i32,
        timeframe: Timeframe,
        start: i64,
        end: i64,
        request_id: u64,
    ) -> Result<HistoricalBarsResult, BrokerError>;
}
