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

pub mod personality;
mod adapter;
mod generate;

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

// ══════════════════════════════════════════════════════════════════════════
// Tests
// ══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use midas_core::Timeframe;
    use personality::ticker_seed;

    // ── Seed determinism ──────────────────────────────────────────────

    #[test]
    fn same_ticker_same_seed() {
        assert_eq!(ticker_seed("AAPL"), ticker_seed("AAPL"));
        assert_eq!(ticker_seed("TSLA"), ticker_seed("TSLA"));
    }

    #[test]
    fn different_tickers_different_seeds() {
        assert_ne!(ticker_seed("AAPL"), ticker_seed("TSLA"));
        assert_ne!(ticker_seed("MSFT"), ticker_seed("GOOG"));
    }

    // ── Provider determinism ──────────────────────────────────────────

    #[test]
    fn same_ticker_same_daily_data() {
        let mut p = TestDataProvider::new();
        let a: Vec<OhlcvBar> = p.daily_bars("AAPL").to_vec();

        let mut p2 = TestDataProvider::new();
        let b: Vec<OhlcvBar> = p2.daily_bars("AAPL").to_vec();

        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(b.iter()) {
            assert_eq!(x.timestamp, y.timestamp);
            assert_eq!(x.open, y.open);
            assert_eq!(x.close, y.close);
        }
    }

    #[test]
    fn different_tickers_different_data() {
        let mut p = TestDataProvider::new();
        let aapl = p.daily_bars("AAPL")[0].open;
        let tsla = p.daily_bars("TSLA")[0].open;
        assert_ne!(aapl, tsla);
    }

    // ── Daily bar quality ─────────────────────────────────────────────

    #[test]
    fn daily_ohlc_constraints() {
        let mut p = TestDataProvider::new();
        for bar in p.daily_bars("AAPL") {
            assert!(bar.high >= bar.open, "H < O: {bar:?}");
            assert!(bar.high >= bar.close, "H < C: {bar:?}");
            assert!(bar.low <= bar.open, "L > O: {bar:?}");
            assert!(bar.low <= bar.close, "L > C: {bar:?}");
            assert!(bar.low > 0.0, "L <= 0: {bar:?}");
            assert!(bar.volume > 0, "vol <= 0: {bar:?}");
        }
    }

    #[test]
    fn daily_prices_stay_positive() {
        let mut p = TestDataProvider::new();
        // Test several tickers across all personality types
        for ticker in &["AAPL", "TSLA", "GME", "KO", "XOM", "NVDA", "AMZN"] {
            for bar in p.daily_bars(ticker) {
                assert!(bar.close > 0.0, "{ticker} close <= 0: {bar:?}");
                assert!(bar.low > 0.0, "{ticker} low <= 0: {bar:?}");
            }
        }
    }

    #[test]
    fn daily_bars_skip_weekends() {
        let mut p = TestDataProvider::new();
        let bars = p.daily_bars("AAPL");
        for pair in bars.windows(2) {
            let gap = pair[1].timestamp - pair[0].timestamp;
            // Mon-Thu → 86400, Fri → 259200 (3 days)
            assert!(
                gap == 86400 || gap == 259200,
                "unexpected day gap: {gap}s between ts {} and {}",
                pair[0].timestamp,
                pair[1].timestamp,
            );
        }
    }

    #[test]
    fn daily_data_covers_decade() {
        let mut p = TestDataProvider::new();
        let (start, end) = p.date_range("AAPL");
        let years = (end - start) as f64 / (365.25 * 86400.0);
        assert!(years > 9.0, "only {years:.1} years of data");
    }

    // ── Date range queries ────────────────────────────────────────────

    #[test]
    fn date_range_filter_works() {
        let mut p = TestDataProvider::new();
        let (data_start, _) = p.date_range("AAPL");
        // Request 30 trading days from the start
        let end = data_start + 45 * 86400; // ~45 calendar days ≈ 30 trading days
        let bars = p.bars("AAPL", Timeframe::D1, data_start, end);
        for bar in &bars {
            assert!(bar.timestamp >= data_start);
            assert!(bar.timestamp < end);
        }
        assert!(!bars.is_empty());
    }

    #[test]
    fn bars_last_days_convenience() {
        let mut p = TestDataProvider::new();
        let bars = p.bars_last_days("AAPL", Timeframe::D1, 60);
        // ~60 calendar days ≈ ~42 trading days
        assert!(bars.len() > 30 && bars.len() < 60);
    }

    #[test]
    fn bars_last_months_convenience() {
        let mut p = TestDataProvider::new();
        let bars = p.bars_last_months("AAPL", Timeframe::D1, 3);
        // 3 months ≈ 90 calendar days ≈ ~63 trading days
        assert!(bars.len() > 50 && bars.len() < 80);
    }

    // ── Intraday quality ──────────────────────────────────────────────

    #[test]
    fn intraday_ohlc_constraints() {
        let mut p = TestDataProvider::new();
        let bars = p.bars_last_days("AAPL", Timeframe::S30, 2);
        assert!(!bars.is_empty(), "no intraday bars");
        for bar in &bars {
            assert!(bar.high >= bar.open, "H < O: {bar:?}");
            assert!(bar.high >= bar.close, "H < C: {bar:?}");
            assert!(bar.low <= bar.open, "L > O: {bar:?}");
            assert!(bar.low <= bar.close, "L > C: {bar:?}");
            assert!(bar.low > 0.0, "L <= 0: {bar:?}");
            assert!(bar.volume > 0, "vol <= 0: {bar:?}");
        }
    }

    #[test]
    fn intraday_consistent_with_daily() {
        let mut p = TestDataProvider::new();
        let daily = p.daily_bars("MSFT").to_vec();
        // Pick a day in the middle
        let day = &daily[daily.len() / 2];
        let day_end = day.timestamp + 86400;
        let intraday = p.bars("MSFT", Timeframe::S30, day.timestamp, day_end);
        assert!(!intraday.is_empty());
        // First intraday open == daily open
        assert_eq!(
            intraday.first().unwrap().open, day.open,
            "first intraday open != daily open"
        );
        // Last intraday close == daily close
        assert_eq!(
            intraday.last().unwrap().close, day.close,
            "last intraday close != daily close"
        );
    }

    #[test]
    fn intraday_deterministic() {
        let mut p1 = TestDataProvider::new();
        let mut p2 = TestDataProvider::new();
        let a = p1.bars_last_days("GOOG", Timeframe::M5, 5);
        let b = p2.bars_last_days("GOOG", Timeframe::M5, 5);
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(b.iter()) {
            assert_eq!(x.timestamp, y.timestamp);
            assert_eq!(x.open, y.open);
        }
    }

    // ── Multi-timeframe ───────────────────────────────────────────────

    #[test]
    fn multiple_timeframes_from_same_provider() {
        let mut p = TestDataProvider::new();
        let ticker = "NVDA";
        for tf in [
            Timeframe::S30,
            Timeframe::M1,
            Timeframe::M5,
            Timeframe::M15,
            Timeframe::M30,
            Timeframe::H1,
            Timeframe::H4,
            Timeframe::D1,
            Timeframe::W1,
        ] {
            let bars = p.bars_last_days(ticker, tf, 30);
            assert!(!bars.is_empty(), "{tf} returned no bars");
        }
    }

    #[test]
    fn coarser_timeframe_has_fewer_bars() {
        let mut p = TestDataProvider::new();
        let m1 = p.bars_last_days("AAPL", Timeframe::M1, 10);
        let m5 = p.bars_last_days("AAPL", Timeframe::M5, 10);
        let h1 = p.bars_last_days("AAPL", Timeframe::H1, 10);
        assert!(m1.len() > m5.len());
        assert!(m5.len() > h1.len());
    }

    #[test]
    #[should_panic(expected = "finest resolution is S30")]
    fn panics_on_sub_s30() {
        let mut p = TestDataProvider::new();
        p.bars_last_days("AAPL", Timeframe::S1, 1);
    }

    // ── Aggregation ───────────────────────────────────────────────────

    #[test]
    fn aggregate_preserves_ohlc() {
        let mut p = TestDataProvider::new();
        let s30 = p.bars_last_days("AAPL", Timeframe::S30, 5);
        let m5 = aggregate_bars(&s30, Timeframe::M5);
        for bar in &m5 {
            assert!(bar.high >= bar.open, "agg H < O: {bar:?}");
            assert!(bar.high >= bar.close, "agg H < C: {bar:?}");
            assert!(bar.low <= bar.open, "agg L > O: {bar:?}");
            assert!(bar.low <= bar.close, "agg L > C: {bar:?}");
            assert!(bar.volume > 0, "agg vol <= 0: {bar:?}");
        }
    }

    #[test]
    fn aggregate_correctness() {
        // 10 source bars at known timestamps
        let source: Vec<OhlcvBar> = (0..10)
            .map(|i| OhlcvBar {
                timestamp: i * 60, // M1 spacing
                open: 100.0 + i as f64,
                high: 105.0 + i as f64,
                low: 95.0 + i as f64,
                close: 101.0 + i as f64,
                volume: 1000,
            })
            .collect();

        let m5 = aggregate_bars(&source, Timeframe::M5);
        assert_eq!(m5.len(), 2);

        // First bucket: bars 0..5
        assert_eq!(m5[0].open, source[0].open);
        assert_eq!(m5[0].close, source[4].close);
        let expected_high = source[..5]
            .iter()
            .map(|b| b.high)
            .fold(f64::NEG_INFINITY, f64::max);
        assert_eq!(m5[0].high, expected_high);
        let expected_low = source[..5]
            .iter()
            .map(|b| b.low)
            .fold(f64::INFINITY, f64::min);
        assert_eq!(m5[0].low, expected_low);
        assert_eq!(m5[0].volume, 5000);
    }

    #[test]
    fn aggregate_empty() {
        assert!(aggregate_bars(&[], Timeframe::H1).is_empty());
    }

    // ── Realism checks ───────────────────────────────────────────────

    #[test]
    fn growth_ticker_trends_significantly() {
        let mut p = TestDataProvider::new();
        // Try multiple tickers — at least one should show >2x movement
        let mut found_big_move = false;
        for ticker in &["AAPL", "TSLA", "GME", "NVDA", "AMZN", "META", "NFLX"] {
            let bars = p.daily_bars(ticker);
            let start_price = bars.first().unwrap().close;
            let end_price = bars.last().unwrap().close;
            let max_price = bars.iter().map(|b| b.high).fold(0.0_f64, f64::max);
            let min_price = bars.iter().map(|b| b.low).fold(f64::INFINITY, f64::min);
            let total_range = max_price / min_price;
            if total_range > 2.0 || (end_price / start_price).abs() > 1.5 {
                found_big_move = true;
                break;
            }
        }
        assert!(found_big_move, "no ticker showed significant price travel");
    }

    #[test]
    fn volume_correlates_with_moves() {
        let mut p = TestDataProvider::new();
        let bars = p.daily_bars("AAPL");

        // Split bars into "big move" (top 25%) and "small move" (bottom 25%)
        let mut returns: Vec<(f64, i64)> = bars
            .iter()
            .map(|b| ((b.close - b.open).abs() / b.open, b.volume))
            .collect();
        returns.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());

        let q1 = returns.len() / 4;
        let q3 = returns.len() * 3 / 4;

        let avg_vol_small: f64 =
            returns[..q1].iter().map(|r| r.1 as f64).sum::<f64>() / q1 as f64;
        let avg_vol_big: f64 =
            returns[q3..].iter().map(|r| r.1 as f64).sum::<f64>() / (returns.len() - q3) as f64;

        assert!(
            avg_vol_big > avg_vol_small,
            "big-move avg vol ({avg_vol_big:.0}) should exceed small-move ({avg_vol_small:.0})"
        );
    }
}
