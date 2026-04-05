//! Live Interactive Brokers historical data adapter implementing [`MarketDataSource`].
//!
//! Wraps `ibapi::Client` to fetch historical OHLCV bars from IB's data farms.

use std::sync::Arc;

use midas_core::{OhlcvBar, SymbolKey, Timeframe};
use tokio::runtime::Handle;

use crate::error::BrokerError;
use crate::market_data::{HistoricalBarsResult, MarketDataSource};

/// Live IB historical data source.
///
/// Fetches historical bars from IB's data farms via `ibapi::Client`.
/// Pacing rules (max 60 requests / 10 minutes) must be enforced by the
/// engine; this adapter does not throttle.
pub struct IbDataSource {
    /// Shared reference to the connected ibapi client.
    client: Arc<ibapi::Client>,
    /// Tokio runtime handle for sync→async bridge.
    rt: Handle,
}

impl IbDataSource {
    /// Create a data source wrapping an already-connected ibapi client.
    pub fn new(client: Arc<ibapi::Client>) -> Self {
        Self {
            client,
            rt: Handle::current(),
        }
    }
}

impl MarketDataSource for IbDataSource {
    fn historical_bars(
        &mut self,
        symbol: &str,
        con_id: i32,
        timeframe: Timeframe,
        start: i64,
        end: i64,
        request_id: u64,
    ) -> Result<HistoricalBarsResult, BrokerError> {
        let bar_size = timeframe_to_ib_bar_size(timeframe);
        let duration = seconds_to_ib_duration(end - start);

        let contract = ibapi::contracts::Contract::stock(symbol).build();
        let client = Arc::clone(&self.client);

        let bars = self.rt.block_on(async {
            client
                .historical_data(
                    &contract,
                    None, // end_date: None means "now"
                    duration,
                    bar_size,
                    Some(ibapi::market_data::historical::WhatToShow::Trades),
                    ibapi::market_data::TradingHours::Regular,
                )
                .await
                .map_err(|e| BrokerError::IbApi {
                    code: -1,
                    message: format!("historical_data request failed: {e}"),
                })
        })?;

        let ohlcv_bars: Vec<OhlcvBar> = bars
            .bars
            .iter()
            .map(|bar| OhlcvBar {
                timestamp: bar.date.unix_timestamp(),
                open: bar.open,
                high: bar.high,
                low: bar.low,
                close: bar.close,
                volume: bar.volume as i64,
            })
            .collect();

        Ok(HistoricalBarsResult {
            symbol: SymbolKey {
                contract_id: con_id,
                symbol: symbol.to_string(),
            },
            request_id,
            bars: ohlcv_bars,
        })
    }
}

/// Convert a [`Timeframe`] to an IB [`BarSize`](ibapi::market_data::historical::BarSize).
fn timeframe_to_ib_bar_size(tf: Timeframe) -> ibapi::market_data::historical::BarSize {
    use ibapi::market_data::historical::BarSize;
    match tf {
        Timeframe::S1 => BarSize::Sec,
        Timeframe::S5 => BarSize::Sec5,
        Timeframe::S15 => BarSize::Sec15,
        Timeframe::S30 => BarSize::Sec30,
        Timeframe::M1 => BarSize::Min,
        Timeframe::M5 => BarSize::Min5,
        Timeframe::M15 => BarSize::Min15,
        Timeframe::M30 => BarSize::Min30,
        Timeframe::H1 => BarSize::Hour,
        Timeframe::H4 => BarSize::Hour4,
        Timeframe::D1 => BarSize::Day,
        Timeframe::W1 => BarSize::Week,
        Timeframe::MN1 => BarSize::Month,
    }
}

/// Convert a duration in seconds to an IB [`Duration`](ibapi::market_data::historical::Duration).
fn seconds_to_ib_duration(secs: i64) -> ibapi::market_data::historical::Duration {
    use ibapi::market_data::historical::Duration;
    if secs >= 365 * 86400 {
        Duration::years((secs / (365 * 86400)) as i32)
    } else if secs >= 30 * 86400 {
        Duration::months((secs / (30 * 86400)) as i32)
    } else if secs >= 7 * 86400 {
        Duration::weeks((secs / (7 * 86400)) as i32)
    } else if secs >= 86400 {
        Duration::days((secs / 86400) as i32)
    } else {
        Duration::seconds(secs as i32)
    }
}
