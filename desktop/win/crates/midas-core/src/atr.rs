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

/// Compute Gerchik G.ATR percentage from daily bars.
///
/// Algorithm:
/// 1. Skip today (last bar).
/// 2. Walk the previous [`GATR_LOOKBACK`] (7) trading sessions.
/// 3. Compute raw average TR from those sessions.
/// 4. Filter paranormal candles (TR > 2x or < 0.5x of the raw average).
/// 5. Average the surviving TRs.
/// 6. Return today's (H-L) / filtered_avg * 100.
///
/// `highs`, `lows`, `closes` must have the same length and represent
/// daily bars ordered oldest-first. The last element is "today".
///
/// Returns `None` if not enough data (need at least `GATR_LOOKBACK + 1` bars,
/// i.e. 7 history bars + 1 today bar = 8 minimum).
pub fn gerchik_gatr_pct(highs: &[f64], lows: &[f64], closes: &[f64]) -> Option<f32> {
    let len = highs.len().min(lows.len()).min(closes.len());
    // Need at least GATR_LOOKBACK prior bars + 1 for today.
    if len < GATR_LOOKBACK + 1 {
        return None;
    }

    // Today is the last bar — we measure its range but exclude it from ATR.
    let today_range = highs[len - 1] - lows[len - 1];

    // Walk the previous GATR_LOOKBACK sessions (indices len-2 down to len-1-GATR_LOOKBACK).
    let history_end = len - 1; // exclusive — skip today
    let history_start = history_end - GATR_LOOKBACK;

    // Compute true ranges for the lookback window.
    let mut trs = Vec::with_capacity(GATR_LOOKBACK);
    for i in history_start..history_end {
        let tr = if i > 0 {
            true_range(highs[i], lows[i], closes[i - 1])
        } else {
            highs[i] - lows[i]
        };
        trs.push(tr);
    }

    if trs.is_empty() {
        return None;
    }

    // Step 1: raw average.
    let raw_avg = trs.iter().sum::<f64>() / trs.len() as f64;
    if raw_avg <= f64::EPSILON {
        return None;
    }

    // Step 2: filter paranormal candles.
    let upper = raw_avg * GATR_PARANORMAL_UPPER;
    let lower = raw_avg * GATR_PARANORMAL_LOWER;
    let mut sum = 0.0;
    let mut count = 0u32;
    for &tr in &trs {
        if tr >= lower && tr <= upper {
            sum += tr;
            count += 1;
        }
    }

    // Step 3: average survivors (fall back to raw if all paranormal).
    let filtered_avg = if count > 0 {
        sum / count as f64
    } else {
        raw_avg
    };

    Some((today_range / filtered_avg * 100.0) as f32)
}

/// Determine the G.ATR color based on percentage consumed.
pub fn gatr_color(pct: f32) -> [f32; 4] {
    if pct >= GATR_THRESHOLD_PCT {
        GATR_COLOR_RED
    } else {
        GATR_COLOR_GREEN
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wilder_atr_empty_returns_none() {
        assert!(wilder_atr(&[], 14).is_none());
    }

    #[test]
    fn wilder_atr_single_element() {
        let result = wilder_atr(&[5.0], 14);
        assert!(result.is_some());
        assert!((result.unwrap() - 5.0).abs() < f64::EPSILON);
    }

    #[test]
    fn wilder_atr_uniform_values() {
        let trs = vec![10.0; 20];
        let result = wilder_atr(&trs, 14).unwrap();
        assert!(
            (result - 10.0).abs() < 1e-10,
            "uniform TR=10 should produce ATR=10, got {result}"
        );
    }

    #[test]
    fn wilder_atr_zero_period_returns_none() {
        assert!(wilder_atr(&[1.0, 2.0], 0).is_none());
    }

    #[test]
    fn true_range_basic() {
        // No gap: TR = high - low.
        let tr = true_range(110.0, 100.0, 105.0);
        assert!((tr - 10.0).abs() < f64::EPSILON);
    }

    #[test]
    fn true_range_gap_up() {
        // Gap up: high - prev_close > high - low.
        let tr = true_range(120.0, 115.0, 100.0);
        assert!((tr - 20.0).abs() < f64::EPSILON);
    }

    #[test]
    fn true_range_gap_down() {
        // Gap down: prev_close - low > high - low.
        let tr = true_range(85.0, 80.0, 100.0);
        assert!((tr - 20.0).abs() < f64::EPSILON);
    }

    #[test]
    fn gatr_color_below_threshold_is_green() {
        assert_eq!(gatr_color(50.0), GATR_COLOR_GREEN);
    }

    #[test]
    fn gatr_color_at_threshold_is_red() {
        assert_eq!(gatr_color(75.0), GATR_COLOR_RED);
    }

    #[test]
    fn gatr_color_above_threshold_is_red() {
        assert_eq!(gatr_color(100.0), GATR_COLOR_RED);
    }

    // ── gerchik_gatr_pct ────────────────────────────────────────

    #[test]
    fn gatr_pct_too_few_bars() {
        // Need GATR_LOOKBACK + 1 = 8 bars minimum.
        let h = vec![110.0; 7];
        let l = vec![90.0; 7];
        let c = vec![100.0; 7];
        assert!(gerchik_gatr_pct(&h, &l, &c).is_none());
    }

    #[test]
    fn gatr_pct_uniform_range() {
        // 8 bars: 7 history + 1 today, all with range 20.
        let h = vec![110.0; 8];
        let l = vec![90.0; 8];
        let c = vec![100.0; 8];
        let pct = gerchik_gatr_pct(&h, &l, &c).unwrap();
        // Today range = 20, avg TR = 20, pct = 100%.
        assert!((pct - 100.0).abs() < 1.0, "expected ~100%, got {pct}");
    }

    #[test]
    fn gatr_pct_skips_today() {
        // 7 history bars with range 20, today with range 10.
        let mut h = vec![110.0; 7];
        h.push(105.0); // today high
        let mut l = vec![90.0; 7];
        l.push(95.0); // today low
        let c = vec![100.0; 8];
        let pct = gerchik_gatr_pct(&h, &l, &c).unwrap();
        // Today range = 10, avg TR ≈ 20, pct ≈ 50%.
        assert!((pct - 50.0).abs() < 5.0, "expected ~50%, got {pct}");
    }

    #[test]
    fn gatr_pct_filters_paranormal() {
        // 7 history bars: 6 normal (range 20) + 1 paranormal (range 200).
        // Raw avg ≈ (20*6 + 200) / 7 ≈ 45.7. Upper = 91.4, Lower = 22.9.
        // The 200 is excluded (>91.4). The 20s are excluded (<22.9).
        // All filtered → falls back to raw avg ≈ 45.7.
        //
        // Use ranges that survive filtering: 6 normal at 40, 1 huge at 200.
        // Raw avg = (40*6 + 200) / 7 ≈ 62.9. Upper = 125.7, Lower = 31.4.
        // 40s pass (>31.4, <125.7). 200 excluded. Filtered avg = 40.
        let mut h = Vec::new();
        let mut l = Vec::new();
        let c = vec![100.0; 9]; // 8 + 1 extra for prev_close lookback

        // Bar 0 (seed for prev_close).
        h.push(120.0);
        l.push(80.0);

        // Bars 1-6: normal range 40.
        for _ in 0..6 {
            h.push(120.0);
            l.push(80.0);
        }
        // Bar 7: paranormal range 200.
        h.push(200.0);
        l.push(0.0);
        // Today: range 40.
        h.push(120.0);
        l.push(80.0);

        let pct = gerchik_gatr_pct(&h, &l, &c).unwrap();
        // Today range = 40, filtered avg = 40, pct ≈ 100%.
        assert!((pct - 100.0).abs() < 5.0, "expected ~100%, got {pct}");
    }
}
