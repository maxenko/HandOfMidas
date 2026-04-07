//! Shared ATR (Average True Range) computation using Wilder's smoothing.

/// Default ATR period (number of bars for Wilder's smoothing).
pub const ATR_PERIOD: usize = 14;

/// Number of prior trading sessions to walk for Gerchik ATR.
pub const GATR_LOOKBACK: usize = 7;

/// Paranormal upper threshold: candles with TR > raw_avg * this are excluded.
pub const GATR_PARANORMAL_UPPER: f64 = 2.0;

/// Paranormal lower threshold: candles with TR < raw_avg * this are excluded.
pub const GATR_PARANORMAL_LOWER: f64 = 0.5;

/// Gerchik ATR threshold: below = green (room to move), at/above = red (range exhaustion).
pub const GATR_THRESHOLD_PCT: f32 = 75.0;

/// Green color for ATR below threshold (low alpha -- watermark style).
pub const GATR_COLOR_GREEN: [f32; 4] = [0.2, 0.8, 0.3, 0.18];

/// Red color for ATR at/above threshold (low alpha -- watermark style).
pub const GATR_COLOR_RED: [f32; 4] = [0.9, 0.25, 0.2, 0.18];

/// Compute ATR from a slice of True Range values using Wilder's smoothing.
///
/// Seeds with the simple average of the first `period` values, then
/// applies exponential smoothing: `atr = atr * (1 - 1/period) + tr / period`.
///
/// Returns `None` if `true_ranges` is empty.
pub fn wilder_atr(true_ranges: &[f64], period: usize) -> Option<f64> {
    if true_ranges.is_empty() {
        return None;
    }
    let period = period.min(true_ranges.len());
    if period == 0 {
        return None;
    }
    let mut atr: f64 = true_ranges[..period].iter().sum::<f64>() / period as f64;
    let alpha = 1.0 / period as f64;
    for &tr in &true_ranges[period..] {
        atr = atr * (1.0 - alpha) + tr * alpha;
    }
    Some(atr)
}

/// Compute True Range for a single bar given current high/low and previous close.
pub fn true_range(high: f64, low: f64, prev_close: f64) -> f64 {
    (high - low)
        .max((high - prev_close).abs())
        .max((low - prev_close).abs())
}

/// Detailed result from the Gerchik G.ATR computation.
///
/// Contains both the percentage and the indices of the daily bars
/// that were selected (non-paranormal) for the average.
#[derive(Clone, Debug)]
pub struct GatrResult {
    /// G.ATR percentage (today's TR / filtered average × 100).
    pub pct: f32,
    /// Indices into the input `highs`/`lows`/`closes` arrays for the
    /// non-paranormal history bars used in the average.
    /// Does NOT include today (the last bar) — today is always bright.
    pub selected_bars: Vec<usize>,
    /// Absolute average true range (filtered average of non-paranormal bars).
    /// When all bars are paranormal, falls back to the raw average.
    pub avg_atr: f64,
}

/// Compute Gerchik G.ATR with full detail (percentage + selected bar indices).
///
/// Algorithm:
/// 1. Compute True Range for today (last bar) and all prior bars.
///    TR includes overnight gaps: `max(H-L, |H-prevClose|, |L-prevClose|)`.
/// 2. Define paranormal thresholds from the raw average of history TRs:
///    TR > 2× raw avg or TR < 0.5× raw avg.
/// 3. Walk backwards from yesterday, skipping paranormal candles, until
///    [`GATR_LOOKBACK`] (7) non-paranormal sessions are collected.
/// 4. Average those 7 TRs.
/// 5. Return today's TR / average × 100, plus the indices of the selected bars.
///
/// `highs`, `lows`, `closes` must have the same length and represent
/// daily bars ordered oldest-first. The last element is "today"
/// (the most recent calendar day in the loaded data).
///
/// Returns `None` if there are fewer than 2 bars or no history TRs.
/// When all history bars are paranormal, returns `Some` with `pct`
/// based on the raw average and empty `selected_bars`.
pub fn gerchik_gatr_detail(highs: &[f64], lows: &[f64], closes: &[f64]) -> Option<GatrResult> {
    let len = highs.len().min(lows.len()).min(closes.len());
    if len < 2 {
        return None;
    }

    // Today's range: simple H-L (not True Range). Gaps from yesterday's
    // close don't count toward "traveled" — only actual intraday movement.
    let today_range = highs[len - 1] - lows[len - 1];

    // Compute TR for every bar before today (indices 1..len-1, using prev close).
    let history_end = len - 1;
    let mut all_trs: Vec<f64> = Vec::with_capacity(history_end);
    for i in 1..history_end {
        all_trs.push(true_range(highs[i], lows[i], closes[i - 1]));
    }
    if all_trs.is_empty() {
        return None;
    }

    // Raw average over all history — used only for paranormal classification.
    let raw_avg = all_trs.iter().sum::<f64>() / all_trs.len() as f64;
    if raw_avg <= f64::EPSILON {
        return None;
    }

    // Walk backwards, collecting non-paranormal TRs until we have 7.
    let upper = raw_avg * GATR_PARANORMAL_UPPER;
    let lower = raw_avg * GATR_PARANORMAL_LOWER;
    let mut sum = 0.0;
    let mut selected_bars = Vec::with_capacity(GATR_LOOKBACK);
    for (j, &tr) in all_trs.iter().enumerate().rev() {
        if tr >= lower && tr <= upper {
            sum += tr;
            // all_trs[j] was computed from bar at index j+1 in the input arrays.
            selected_bars.push(j + 1);
            if selected_bars.len() == GATR_LOOKBACK {
                break;
            }
        }
    }

    let (pct, avg_atr) = if selected_bars.is_empty() {
        ((today_range / raw_avg * 100.0) as f32, raw_avg)
    } else {
        let avg = sum / selected_bars.len() as f64;
        ((today_range / avg * 100.0) as f32, avg)
    };

    // Reverse so indices are in ascending order.
    selected_bars.reverse();

    Some(GatrResult {
        pct,
        selected_bars,
        avg_atr,
    })
}

/// Compute Gerchik G.ATR percentage from daily bars.
///
/// Thin wrapper around [`gerchik_gatr_detail`] that discards the
/// selected bar indices. Use `gerchik_gatr_detail` when you need
/// to know which bars were selected (e.g., for hover highlighting).
pub fn gerchik_gatr_pct(highs: &[f64], lows: &[f64], closes: &[f64]) -> Option<f32> {
    gerchik_gatr_detail(highs, lows, closes).map(|r| r.pct)
}

/// Determine the G.ATR color based on today's price direction.
/// Green if price is up from previous close, red if down.
pub fn gatr_color(price_up: bool) -> [f32; 4] {
    if price_up {
        GATR_COLOR_GREEN
    } else {
        GATR_COLOR_RED
    }
}

#[cfg(test)]
mod tests;
