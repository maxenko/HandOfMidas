//! Gerchik ATR indicator — shows what percentage of the average daily range
//! has been consumed in the current intraday session.
//!
//! Aggregates intraday candles into synthetic daily bars, then delegates to
//! [`midas_core::gerchik_gatr_pct`] which implements the canonical algorithm:
//! skip today, walk previous 7 sessions, filter paranormal candles (TR > 2x
//! or < 0.5x of raw average), return today's (H-L) / filtered average * 100.
//!
//! Only produces output for intraday charts (candle duration < 1 day).
//!
//! # Display
//!
//! Renders as a subtle text badge in the top-right corner of the chart:
//! - **Green** when < 75% of ATR consumed (room for movement)
//! - **Red** when ≥ 75% of ATR consumed (range exhaustion)

use midas_core::CandleData;

/// One day in milliseconds.
const DAY_MS: i64 = 86_400_000;

/// Render data for the Gerchik ATR overlay.
///
/// Produced by [`compute_gerchik_atr`] and consumed by the view layer
/// to build a text badge in the top-right corner of the chart.
#[derive(Clone, Debug)]
pub struct GerchikAtrRender {
    /// ATR percentage consumed (0.0+, can exceed 100).
    pub pct: f32,
    /// Display text (e.g. "ATR 67%").
    pub text: String,
    /// RGBA color: green if below threshold, red if at/above.
    pub color: [f32; 4],
    /// Intraday candle index ranges that should remain bright during
    /// hover highlighting. Each tuple is `(start_idx, end_idx)` inclusive.
    /// Includes the 7 selected non-paranormal daily bars + today.
    /// Sorted ascending by start_idx, non-overlapping.
    pub bright_ranges: Vec<(usize, usize)>,
}

/// Compute the Gerchik ATR overlay from intraday candle data.
///
/// Returns `None` if:
/// - Data has fewer than 2 candles
/// - Candle duration ≥ 1 day (not an intraday chart)
/// - Not enough daily bars for ATR calculation (need at least 2)
///
/// **Note**: This is the intraday variant. The watchlist grid uses a daily
/// variant in `midas_app::market_cache` that computes directly from D1 bars.
/// The two may show different percentages for the same symbol.
pub fn compute_gerchik_atr(
    data: &dyn CandleData,
    candle_duration_ms: f64,
) -> Option<GerchikAtrRender> {
    // Only show on intraday charts.
    if candle_duration_ms >= DAY_MS as f64 || data.len() < 2 {
        return None;
    }

    // Aggregate intraday candles into synthetic daily bars.
    let daily_bars = aggregate_daily_bars(data);
    if daily_bars.len() < 3 {
        return None;
    }

    // Convert synthetic daily bars to f64 slices for the shared algorithm.
    let highs: Vec<f64> = daily_bars.iter().map(|b| b.high as f64).collect();
    let lows: Vec<f64> = daily_bars.iter().map(|b| b.low as f64).collect();
    let closes: Vec<f64> = daily_bars.iter().map(|b| b.close as f64).collect();

    // Use the detail variant to get both percentage and selected bar indices.
    let result = midas_core::gerchik_gatr_detail(&highs, &lows, &closes)?;
    let n = closes.len();
    let price_up = n >= 2 && closes[n - 1] >= closes[n - 2];
    let color = midas_core::gatr_color(price_up);
    let text = format!("G.ATR {:.0}%", result.pct);

    // Map selected daily bar indices → intraday candle index ranges.
    // Always include today (last daily bar).
    let mut bright_ranges: Vec<(usize, usize)> = result
        .selected_bars
        .iter()
        .map(|&bar_idx| (daily_bars[bar_idx].start_idx, daily_bars[bar_idx].end_idx))
        .collect();
    // Add today's range (last daily bar).
    let today = &daily_bars[daily_bars.len() - 1];
    bright_ranges.push((today.start_idx, today.end_idx));
    bright_ranges.sort_unstable_by_key(|r| r.0);

    Some(GerchikAtrRender {
        pct: result.pct,
        text,
        color,
        bright_ranges,
    })
}

/// A synthetic daily bar aggregated from intraday candles.
#[derive(Clone, Debug)]
struct DailyBar {
    high: f32,
    low: f32,
    close: f32,
    /// First intraday candle index (inclusive) in the source `CandleData`.
    start_idx: usize,
    /// Last intraday candle index (inclusive) in the source `CandleData`.
    end_idx: usize,
}

/// Aggregate intraday candles into daily bars by UTC calendar day.
///
/// Groups consecutive candles that share the same `timestamp / DAY_MS`
/// value into a single bar with the day's high, low, last close, and
/// the source candle index range.
fn aggregate_daily_bars(data: &dyn CandleData) -> Vec<DailyBar> {
    if data.is_empty() {
        return Vec::new();
    }

    let mut bars = Vec::new();
    let mut day_high = data.high(0);
    let mut day_low = data.low(0);
    let mut day_close = data.close(0);
    let mut current_day = data.timestamp(0).div_euclid(DAY_MS);
    let mut day_start_idx: usize = 0;

    for i in 1..data.len() {
        let day = data.timestamp(i).div_euclid(DAY_MS);
        if day != current_day {
            bars.push(DailyBar {
                high: day_high,
                low: day_low,
                close: day_close,
                start_idx: day_start_idx,
                end_idx: i - 1,
            });
            day_high = data.high(i);
            day_low = data.low(i);
            current_day = day;
            day_start_idx = i;
        } else {
            day_high = day_high.max(data.high(i));
            day_low = day_low.min(data.low(i));
        }
        day_close = data.close(i);
    }
    // Push the final (current) day.
    bars.push(DailyBar {
        high: day_high,
        low: day_low,
        close: day_close,
        start_idx: day_start_idx,
        end_idx: data.len() - 1,
    });

    bars
}

#[cfg(test)]
mod tests;
