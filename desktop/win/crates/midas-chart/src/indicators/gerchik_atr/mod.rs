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
/// into a single bar with the day's high, low, last close, and
/// the source candle index range.
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
/// This uses `midas_core::gerchik_gatr_pct` for the core math,
/// wrapping it with daily bar aggregation and chart display logic.
pub fn compute(
    data: &dyn CandleData,
    candle_duration_ms: f64,
    _config: &GerchikAtrConfig,
) -> Option<IndicatorOutput> {
    // Only show on intraday charts.
    if candle_duration_ms >= DAY_MS as f64 || data.len() < 2 {
        return None;
    }

    let daily_bars = aggregate_daily_bars(data);
    if daily_bars.len() < 2 {
        return None;
    }

    // Build f64 slices for the canonical Gerchik algorithm.
    let highs: Vec<f64> = daily_bars.iter().map(|b| b.high).collect();
    let lows: Vec<f64> = daily_bars.iter().map(|b| b.low).collect();
    let closes: Vec<f64> = daily_bars.iter().map(|b| b.close).collect();

    let pct = midas_core::gerchik_gatr_pct(&highs, &lows, &closes)?;
    let n = closes.len();
    let price_up = n >= 2 && closes[n - 1] >= closes[n - 2];
    let color = midas_core::gatr_color(price_up);
    let text = format!("G.ATR {:.0}%", pct);

    Some(IndicatorOutput::text_badge(text, color))
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests;
