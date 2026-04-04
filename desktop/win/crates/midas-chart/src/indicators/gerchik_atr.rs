//! Gerchik ATR indicator -- chart-specific configuration and output.
//!
//! This module wraps the pure math from `midas_indicators::GerchikAtr`
//! with chart-specific config (period, thresholds, display colors) and
//! produces [`IndicatorOutput`] for the renderer.
//!
//! The existing [`crate::gerchik_atr`] module remains untouched for now;
//! this is the forward-looking replacement that uses the shared math crate
//! and integrates with the new indicator architecture.

use midas_core::CandleData;
use midas_indicators::atr::{true_range, GerchikAtr};

use super::IndicatorOutput;

/// One day in milliseconds.
const DAY_MS: i64 = 86_400_000;

/// Default ATR period (number of daily bars).
const DEFAULT_PERIOD: usize = 14;

/// Default upper paranormal coefficient.
const DEFAULT_UPPER_COEFF: f64 = 2.0;

/// Default lower paranormal coefficient.
const DEFAULT_LOWER_COEFF: f64 = 0.5;

/// Percentage threshold: below = green, at or above = red.
/// Re-exported from `midas_core::GATR_THRESHOLD_PCT` for custom config default.
const ATR_THRESHOLD_PCT: f32 = midas_core::GATR_THRESHOLD_PCT;

// ── Config ──────────────────────────────────────────────────────────

/// Chart-specific configuration for the Gerchik ATR indicator.
///
/// Stored alongside the general [`super::IndicatorConfig`] when
/// the chart needs custom ATR parameters (non-default period, etc.).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct GerchikAtrConfig {
    /// ATR smoothing period (number of daily bars).
    pub period: usize,
    /// Upper paranormal coefficient (candles with TR > coeff * raw_ATR excluded).
    pub upper_coeff: f64,
    /// Lower paranormal coefficient (candles with TR < coeff * raw_ATR excluded).
    pub lower_coeff: f64,
    /// Percentage threshold for green/red coloring.
    pub threshold_pct: f32,
}

impl Default for GerchikAtrConfig {
    fn default() -> Self {
        Self {
            period: DEFAULT_PERIOD,
            upper_coeff: DEFAULT_UPPER_COEFF,
            lower_coeff: DEFAULT_LOWER_COEFF,
            threshold_pct: ATR_THRESHOLD_PCT,
        }
    }
}

// ── Computation ─────────────────────────────────────────────────────

/// A synthetic daily bar aggregated from intraday candles.
#[derive(Clone, Debug)]
struct DailyBar {
    high: f64,
    low: f64,
    close: f64,
}

/// Aggregate intraday candles into daily bars by UTC calendar day.
///
/// Groups consecutive candles sharing the same `timestamp / DAY_MS`
/// into a single bar with the day's high, low, and last close.
fn aggregate_daily_bars(data: &dyn CandleData) -> Vec<DailyBar> {
    if data.is_empty() {
        return Vec::new();
    }

    let mut bars = Vec::new();
    let mut day_high = data.high(0) as f64;
    let mut day_low = data.low(0) as f64;
    let mut day_close = data.close(0) as f64;
    let mut current_day = data.timestamp(0).div_euclid(DAY_MS);

    for i in 1..data.len() {
        let day = data.timestamp(i).div_euclid(DAY_MS);
        if day != current_day {
            bars.push(DailyBar {
                high: day_high,
                low: day_low,
                close: day_close,
            });
            day_high = data.high(i) as f64;
            day_low = data.low(i) as f64;
            current_day = day;
        } else {
            day_high = day_high.max(data.high(i) as f64);
            day_low = day_low.min(data.low(i) as f64);
        }
        day_close = data.close(i) as f64;
    }
    bars.push(DailyBar {
        high: day_high,
        low: day_low,
        close: day_close,
    });

    bars
}

/// Compute the Gerchik ATR indicator from intraday candle data.
///
/// Returns `None` if:
/// - Data has fewer than 2 candles
/// - Candle duration >= 1 day (not an intraday chart)
/// - Not enough daily bars for ATR calculation (need at least 2)
///
/// This uses `midas_indicators::GerchikAtr` for the core math,
/// wrapping it with daily bar aggregation and chart display logic.
pub fn compute(
    data: &dyn CandleData,
    candle_duration_ms: f64,
    config: &GerchikAtrConfig,
) -> Option<IndicatorOutput> {
    // Only show on intraday charts.
    if candle_duration_ms >= DAY_MS as f64 || data.len() < 2 {
        return None;
    }

    let daily_bars = aggregate_daily_bars(data);
    if daily_bars.len() < 2 {
        return None;
    }

    // Build true range series from daily bars.
    let mut true_ranges = Vec::with_capacity(daily_bars.len() - 1);
    for i in 1..daily_bars.len() {
        let tr = true_range(
            daily_bars[i].high,
            daily_bars[i].low,
            Some(daily_bars[i - 1].close),
        );
        true_ranges.push(tr);
    }

    // Compute filtered ATR using the midas-indicators crate.
    let gerchik = GerchikAtr::with_coefficients(
        config.period,
        config.upper_coeff,
        config.lower_coeff,
    );
    let atr = gerchik.compute(&true_ranges)?;
    if atr <= 0.0 {
        return None;
    }

    // Current session range = last daily bar's (high - low).
    let last = daily_bars.last()?;
    let session_range = last.high - last.low;

    let pct = (session_range / atr * 100.0) as f32;
    let color = if pct >= config.threshold_pct {
        midas_core::GATR_COLOR_RED
    } else {
        midas_core::GATR_COLOR_GREEN
    };
    let text = format!("G.ATR {:.0}%", pct);

    Some(IndicatorOutput::text_badge(text, color))
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::ops::Range;

    /// Minimal test fixture implementing `CandleData`.
    struct TestCandles {
        timestamps: Vec<i64>,
        opens: Vec<f32>,
        highs: Vec<f32>,
        lows: Vec<f32>,
        closes: Vec<f32>,
        volumes: Vec<u32>,
    }

    impl CandleData for TestCandles {
        fn len(&self) -> usize {
            self.timestamps.len()
        }
        fn timestamp(&self, idx: usize) -> i64 {
            self.timestamps[idx]
        }
        fn open(&self, idx: usize) -> f32 {
            self.opens[idx]
        }
        fn high(&self, idx: usize) -> f32 {
            self.highs[idx]
        }
        fn low(&self, idx: usize) -> f32 {
            self.lows[idx]
        }
        fn close(&self, idx: usize) -> f32 {
            self.closes[idx]
        }
        fn volume(&self, idx: usize) -> u32 {
            self.volumes[idx]
        }
        fn price_range(&self, range: Range<usize>) -> (f32, f32) {
            let mut min = f32::MAX;
            let mut max = f32::MIN;
            for i in range {
                min = min.min(self.lows[i]);
                max = max.max(self.highs[i]);
            }
            (min, max)
        }
        fn find_index_by_time(&self, ts: i64) -> usize {
            match self.timestamps.binary_search(&ts) {
                Ok(idx) => idx,
                Err(idx) => idx.min(self.len().saturating_sub(1)),
            }
        }
    }

    /// Build 5-minute candles across multiple days.
    ///
    /// Each day has 4 candles at 09:30, 10:00, 10:30, 11:00 UTC.
    /// Day N has: high = 100 + N*2, low = 100 - N, close = 100 + N.
    fn multi_day_5m_data(num_days: usize) -> TestCandles {
        let five_min_ms: i64 = 300_000;
        let mut timestamps = Vec::new();
        let mut opens = Vec::new();
        let mut highs = Vec::new();
        let mut lows = Vec::new();
        let mut closes = Vec::new();
        let mut volumes = Vec::new();

        let base_day_ms: i64 = 1_705_276_800_000;
        let session_start_offset: i64 = 9 * 3_600_000 + 30 * 60_000;

        for day in 0..num_days {
            let day_base = base_day_ms + day as i64 * DAY_MS + session_start_offset;
            let day_f = day as f32;
            for candle in 0..4 {
                let ts = day_base + candle as i64 * five_min_ms;
                timestamps.push(ts);
                opens.push(100.0 + day_f);
                if candle == 0 {
                    highs.push(100.0 + day_f * 2.0);
                    lows.push(100.0 - day_f);
                } else {
                    highs.push(100.0 + day_f + 0.5);
                    lows.push(100.0 + day_f - 0.5);
                }
                closes.push(100.0 + day_f);
                volumes.push(1000);
            }
        }

        TestCandles {
            timestamps,
            opens,
            highs,
            lows,
            closes,
            volumes,
        }
    }

    fn default_cfg() -> GerchikAtrConfig {
        GerchikAtrConfig::default()
    }

    // ── aggregate_daily_bars ────────────────────────────────────────

    #[test]
    fn aggregate_empty_data() {
        let data = TestCandles {
            timestamps: vec![],
            opens: vec![],
            highs: vec![],
            lows: vec![],
            closes: vec![],
            volumes: vec![],
        };
        assert!(aggregate_daily_bars(&data).is_empty());
    }

    #[test]
    fn aggregate_single_candle() {
        let data = TestCandles {
            timestamps: vec![1_705_276_800_000],
            opens: vec![100.0],
            highs: vec![105.0],
            lows: vec![95.0],
            closes: vec![102.0],
            volumes: vec![1000],
        };
        let bars = aggregate_daily_bars(&data);
        assert_eq!(bars.len(), 1);
        assert!((bars[0].high - 105.0).abs() < f64::EPSILON);
        assert!((bars[0].low - 95.0).abs() < f64::EPSILON);
        assert!((bars[0].close - 102.0).abs() < f64::EPSILON);
    }

    #[test]
    fn aggregate_multi_day() {
        let data = multi_day_5m_data(3);
        let bars = aggregate_daily_bars(&data);
        assert_eq!(bars.len(), 3);

        // Day 0: high = max(100.0, 100.5, 100.5, 100.5) = 100.5
        assert!((bars[0].high - 100.5).abs() < 1e-6);
        // Day 1: high = 102.0 (from first candle with day_f * 2.0)
        assert!((bars[1].high - 102.0).abs() < 1e-6);
        assert!((bars[1].low - 99.0).abs() < 1e-6);
    }

    // ── compute ─────────────────────────────────────────────────────

    #[test]
    fn returns_none_for_empty_data() {
        let data = TestCandles {
            timestamps: vec![],
            opens: vec![],
            highs: vec![],
            lows: vec![],
            closes: vec![],
            volumes: vec![],
        };
        assert!(compute(&data, 300_000.0, &default_cfg()).is_none());
    }

    #[test]
    fn returns_none_for_daily_timeframe() {
        let data = multi_day_5m_data(20);
        assert!(compute(&data, DAY_MS as f64, &default_cfg()).is_none());
        assert!(compute(&data, DAY_MS as f64 * 7.0, &default_cfg()).is_none());
    }

    #[test]
    fn returns_none_for_single_day() {
        let data = multi_day_5m_data(1);
        assert!(compute(&data, 300_000.0, &default_cfg()).is_none());
    }

    #[test]
    fn produces_text_badge_for_multi_day_intraday() {
        let data = multi_day_5m_data(20);
        let output = compute(&data, 300_000.0, &default_cfg());
        assert!(output.is_some());
        match output.unwrap() {
            IndicatorOutput::TextBadge { text, color: _ } => {
                assert!(text.starts_with("G.ATR "));
                assert!(text.ends_with('%'));
                // No decimal point in the percentage.
                let pct_part = text.strip_prefix("G.ATR ").unwrap();
                assert!(!pct_part.contains('.'));
            }
            _ => panic!("expected TextBadge variant"),
        }
    }

    #[test]
    fn red_when_high_atr_usage() {
        let base = 1_705_276_800_000_i64;
        let five_min = 300_000_i64;
        let session_start = 9 * 3_600_000_i64;
        let mut timestamps = Vec::new();
        let mut opens = Vec::new();
        let mut highs = Vec::new();
        let mut lows = Vec::new();
        let mut closes = Vec::new();
        let mut volumes = Vec::new();

        // 15 days with tiny ranges.
        for day in 0..15 {
            for candle in 0..4 {
                let ts = base + day * DAY_MS + session_start + candle * five_min;
                timestamps.push(ts);
                opens.push(100.0);
                highs.push(100.5);
                lows.push(99.5);
                closes.push(100.0);
                volumes.push(1000);
            }
        }
        // Day 15: huge range.
        for candle in 0..4 {
            let ts = base + 15 * DAY_MS + session_start + candle * five_min;
            timestamps.push(ts);
            opens.push(100.0);
            if candle == 0 {
                highs.push(120.0);
                lows.push(80.0);
            } else {
                highs.push(101.0);
                lows.push(99.0);
            }
            closes.push(100.0);
            volumes.push(1000);
        }

        let data = TestCandles {
            timestamps,
            opens,
            highs,
            lows,
            closes,
            volumes,
        };

        let output = compute(&data, 300_000.0, &default_cfg()).unwrap();
        match output {
            IndicatorOutput::TextBadge { text: _, color } => {
                assert_eq!(color, midas_core::GATR_COLOR_RED, "should be red for high ATR usage");
            }
            _ => panic!("expected TextBadge variant"),
        }
    }

    #[test]
    fn custom_config_period() {
        let data = multi_day_5m_data(10);
        let mut cfg = default_cfg();
        cfg.period = 5;
        let output = compute(&data, 300_000.0, &cfg);
        assert!(output.is_some(), "should produce output with custom period");
    }

    #[test]
    fn custom_config_threshold() {
        let data = multi_day_5m_data(20);
        let mut cfg = default_cfg();
        // Very low threshold: everything should be red.
        cfg.threshold_pct = 0.1;
        let output = compute(&data, 300_000.0, &cfg).unwrap();
        match output {
            IndicatorOutput::TextBadge { text: _, color } => {
                assert_eq!(color, midas_core::GATR_COLOR_RED, "near-zero threshold should produce red");
            }
            _ => panic!("expected TextBadge variant"),
        }
    }

    #[test]
    fn h4_is_still_intraday() {
        let data = multi_day_5m_data(20);
        let output = compute(&data, 14_400_000.0, &default_cfg());
        assert!(output.is_some());
    }

    #[test]
    fn config_serde_roundtrip() {
        let cfg = GerchikAtrConfig {
            period: 21,
            upper_coeff: 3.0,
            lower_coeff: 0.25,
            threshold_pct: 80.0,
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: GerchikAtrConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.period, 21);
        assert!((back.upper_coeff - 3.0).abs() < f64::EPSILON);
        assert!((back.lower_coeff - 0.25).abs() < f64::EPSILON);
        assert!((back.threshold_pct - 80.0).abs() < f32::EPSILON);
    }

    #[test]
    fn default_config_values() {
        let cfg = GerchikAtrConfig::default();
        assert_eq!(cfg.period, 14);
        assert!((cfg.upper_coeff - 2.0).abs() < f64::EPSILON);
        assert!((cfg.lower_coeff - 0.5).abs() < f64::EPSILON);
        assert!((cfg.threshold_pct - 75.0).abs() < f32::EPSILON);
    }
}
