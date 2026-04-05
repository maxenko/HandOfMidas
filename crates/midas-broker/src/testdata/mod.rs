//! Deterministic, realistic test market data — per-ticker, multi-timeframe.
//!
//! Generates ~10 years of coherent OHLCV data for any ticker symbol. The
//! ticker name seeds the RNG so the same ticker always produces identical
//! bars. Each ticker gets a unique "personality" (growth, blue-chip,
//! volatile, etc.) with regime-switching dynamics, GARCH volatility
//! clustering, overnight gaps, and volume–price correlation.
//!
//! # Usage
//!
//! ```ignore
//! use midas_broker::testdata::TestDataProvider;
//! use midas_core::Timeframe;
//!
//! let mut provider = TestDataProvider::new();
//!
//! // Daily bars for the last 6 months
//! let daily = provider.bars_last_months("AAPL", Timeframe::D1, 6);
//!
//! // 5-minute bars for a specific date range (epoch seconds)
//! let intraday = provider.bars("TSLA", Timeframe::M5, 1704067200, 1704153600);
//!
//! // Same ticker always gives the same data
//! let a = provider.bars_last_days("MSFT", Timeframe::H1, 30);
//! let b = provider.bars_last_days("MSFT", Timeframe::H1, 30);
//! assert_eq!(a.len(), b.len());
//! ```

mod adapter;
mod generate;
pub mod personality;

use std::collections::HashMap;

use midas_core::{OhlcvBar, Timeframe};

use generate::{generate_daily_bars, generate_intraday_for_day};
use personality::{personality_for_seed, ticker_seed, StockPersonality};

// ══════════════════════════════════════════════════════════════════════════
// TestDataProvider
// ══════════════════════════════════════════════════════════════════════════

/// Provider of deterministic test market data.
///
/// Lazily generates ~10 years (2016–2026) of daily bars per ticker on first
/// access, then generates intraday S30 bars on demand via Brownian bridge
/// from the daily OHLC. Coarser timeframes aggregate from S30 or daily.
///
/// The API mirrors what a real historical-data provider would expose: give me
/// bars for a ticker, at a timeframe, within a date range.
pub struct TestDataProvider {
    tickers: HashMap<String, TickerData>,
}

struct TickerData {
    seed: u64,
    #[allow(dead_code)]
    personality: StockPersonality,
    daily_bars: Vec<OhlcvBar>,
    /// day timestamp → S30 intraday bars (lazily generated).
    intraday_cache: HashMap<i64, Vec<OhlcvBar>>,
}

impl TestDataProvider {
    pub fn new() -> Self {
        Self {
            tickers: HashMap::new(),
        }
    }

    /// Ensure a ticker's daily bars are generated.
    fn ensure_ticker(&mut self, ticker: &str) {
        if !self.tickers.contains_key(ticker) {
            let seed = ticker_seed(ticker);
            let personality = personality_for_seed(seed);
            let daily_bars = generate_daily_bars(&personality, seed);
            self.tickers.insert(
                ticker.to_string(),
                TickerData {
                    seed,
                    personality,
                    daily_bars,
                    intraday_cache: HashMap::new(),
                },
            );
        }
    }

    /// Get OHLCV bars for `ticker` at `timeframe` within `[start, end)`.
    ///
    /// Timestamps are UTC epoch seconds. Returns bars whose timestamp falls
    /// within the range. Data is deterministic — same arguments always
    /// produce identical output.
    ///
    /// Finest supported intraday resolution is S30 (30 seconds). Requesting
    /// S1/S5/S15 panics.
    pub fn bars(&mut self, ticker: &str, tf: Timeframe, start: i64, end: i64) -> Vec<OhlcvBar> {
        assert!(
            tf.as_secs() >= Timeframe::S30.as_secs(),
            "TestDataProvider finest resolution is S30; requested {}",
            tf,
        );

        self.ensure_ticker(ticker);

        if tf.as_secs() >= Timeframe::D1.as_secs() {
            self.bars_daily_or_coarser(ticker, tf, start, end)
        } else {
            self.bars_intraday(ticker, tf, start, end)
        }
    }

    /// All daily bars for a ticker (~2,575 bars covering 2016–2026).
    pub fn daily_bars(&mut self, ticker: &str) -> &[OhlcvBar] {
        self.ensure_ticker(ticker);
        &self.tickers[ticker].daily_bars
    }

    /// Timestamp range covered by the generated data: `(first_bar_ts, day_after_last_bar_ts)`.
    pub fn date_range(&mut self, ticker: &str) -> (i64, i64) {
        self.ensure_ticker(ticker);
        let bars = &self.tickers[ticker].daily_bars;
        (
            bars.first().unwrap().timestamp,
            bars.last().unwrap().timestamp + 86400,
        )
    }

    /// Convenience: bars for the last `days` calendar days from end of data.
    pub fn bars_last_days(&mut self, ticker: &str, tf: Timeframe, days: u32) -> Vec<OhlcvBar> {
        let (_, end) = self.date_range(ticker);
        let start = end - days as i64 * 86400;
        self.bars(ticker, tf, start, end)
    }

    /// Convenience: bars for the last `months` months (30 days each).
    pub fn bars_last_months(&mut self, ticker: &str, tf: Timeframe, months: u32) -> Vec<OhlcvBar> {
        self.bars_last_days(ticker, tf, months * 30)
    }

    // ── Internal ──────────────────────────────────────────────────────

    fn bars_daily_or_coarser(
        &self,
        ticker: &str,
        tf: Timeframe,
        start: i64,
        end: i64,
    ) -> Vec<OhlcvBar> {
        let data = &self.tickers[ticker];
        let filtered: Vec<OhlcvBar> = data
            .daily_bars
            .iter()
            .filter(|b| b.timestamp >= start && b.timestamp < end)
            .cloned()
            .collect();

        if tf == Timeframe::D1 {
            filtered
        } else {
            aggregate_bars(&filtered, tf)
        }
    }

    fn bars_intraday(
        &mut self,
        ticker: &str,
        tf: Timeframe,
        start: i64,
        end: i64,
    ) -> Vec<OhlcvBar> {
        let data = self.tickers.get(ticker).unwrap();

        // Find daily bars that could contain intraday bars in [start, end).
        // Daily bar timestamp is at 00:00 UTC; intraday starts at 14:30 UTC same day.
        let day_start = (start / 86400) * 86400;
        let day_end = ((end + 86399) / 86400) * 86400;

        let relevant_days: Vec<(usize, OhlcvBar)> = data
            .daily_bars
            .iter()
            .enumerate()
            .filter(|(_, b)| b.timestamp >= day_start && b.timestamp < day_end)
            .map(|(i, b)| (i, b.clone()))
            .collect();

        let seed = data.seed;
        let data = self.tickers.get_mut(ticker).unwrap();

        let mut all_bars = Vec::new();
        for (day_idx, daily) in &relevant_days {
            let day_bars = data
                .intraday_cache
                .entry(daily.timestamp)
                .or_insert_with(|| generate_intraday_for_day(daily, seed, *day_idx));

            all_bars.extend(
                day_bars
                    .iter()
                    .filter(|b| b.timestamp >= start && b.timestamp < end)
                    .cloned(),
            );
        }

        if tf.as_secs() == Timeframe::S30.as_secs() {
            all_bars
        } else {
            aggregate_bars(&all_bars, tf)
        }
    }
}

impl Default for TestDataProvider {
    fn default() -> Self {
        Self::new()
    }
}

// ══════════════════════════════════════════════════════════════════════════
// OHLCV Aggregation
// ══════════════════════════════════════════════════════════════════════════

/// Aggregate fine-grained bars into a coarser timeframe.
///
/// OHLCV rules: first open, max high, min low, last close, sum volume.
/// Bars are bucketed by flooring each timestamp to the target timeframe
/// boundary.
pub fn aggregate_bars(source: &[OhlcvBar], target_tf: Timeframe) -> Vec<OhlcvBar> {
    if source.is_empty() {
        return vec![];
    }

    let step_secs = target_tf.as_secs() as i64;
    let mut result = Vec::new();

    let mut bucket_ts = (source[0].timestamp / step_secs) * step_secs;
    let mut open = source[0].open;
    let mut high = source[0].high;
    let mut low = source[0].low;
    let mut close = source[0].close;
    let mut volume = source[0].volume;

    for bar in &source[1..] {
        let bar_bucket = (bar.timestamp / step_secs) * step_secs;
        if bar_bucket != bucket_ts {
            result.push(OhlcvBar {
                timestamp: bucket_ts,
                open,
                high,
                low,
                close,
                volume,
            });
            bucket_ts = bar_bucket;
            open = bar.open;
            high = bar.high;
            low = bar.low;
            close = bar.close;
            volume = bar.volume;
        } else {
            high = high.max(bar.high);
            low = low.min(bar.low);
            close = bar.close;
            volume += bar.volume;
        }
    }
    result.push(OhlcvBar {
        timestamp: bucket_ts,
        open,
        high,
        low,
        close,
        volume,
    });

    result
}

#[cfg(test)]
mod tests;
