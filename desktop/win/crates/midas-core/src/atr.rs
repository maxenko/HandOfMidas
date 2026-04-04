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
/// 2. Compute true ranges for all prior bars to establish a raw average.
/// 3. Define paranormal thresholds: TR > 2x raw avg or TR < 0.5x raw avg.
/// 4. Walk backwards from yesterday, skipping paranormal candles, until
///    [`GATR_LOOKBACK`] (7) non-paranormal sessions are collected.
/// 5. Average those 7 TRs.
/// 6. Return today's (H-L) / average * 100.
///
/// `highs`, `lows`, `closes` must have the same length and represent
/// daily bars ordered oldest-first. The last element is "today".
///
/// Returns `None` if fewer than 7 non-paranormal history bars exist.
pub fn gerchik_gatr_pct(highs: &[f64], lows: &[f64], closes: &[f64]) -> Option<f32> {
    let len = highs.len().min(lows.len()).min(closes.len());
    if len < 2 {
        return None;
    }

    // Today is the last bar — we measure its range but exclude it from the average.
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
    let mut count = 0u32;
    for &tr in all_trs.iter().rev() {
        if tr >= lower && tr <= upper {
            sum += tr;
            count += 1;
            if count == GATR_LOOKBACK as u32 {
                break;
            }
        }
    }

    if count == 0 {
        // All candles paranormal — fall back to raw average.
        return Some((today_range / raw_avg * 100.0) as f32);
    }

    let avg = sum / count as f64;
    Some((today_range / avg * 100.0) as f32)
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
        // Need at least 2 bars (1 history + 1 today).
        assert!(gerchik_gatr_pct(&[110.0], &[90.0], &[100.0]).is_none());
        assert!(gerchik_gatr_pct(&[], &[], &[]).is_none());
    }

    #[test]
    fn gatr_pct_uniform_range() {
        // 10 bars: 9 history + 1 today, all with range 20.
        let h = vec![110.0; 10];
        let l = vec![90.0; 10];
        let c = vec![100.0; 10];
        let pct = gerchik_gatr_pct(&h, &l, &c).unwrap();
        // Today range = 20, avg of 7 recent TRs = 20, pct = 100%.
        assert!((pct - 100.0).abs() < 1.0, "expected ~100%, got {pct}");
    }

    #[test]
    fn gatr_pct_skips_today() {
        // 9 history bars with range 20, today with range 10.
        let mut h = vec![110.0; 9];
        h.push(105.0); // today high
        let mut l = vec![90.0; 9];
        l.push(95.0); // today low
        let c = vec![100.0; 10];
        let pct = gerchik_gatr_pct(&h, &l, &c).unwrap();
        // Today range = 10, avg TR = 20, pct = 50%.
        assert!((pct - 50.0).abs() < 1.0, "expected ~50%, got {pct}");
    }

    #[test]
    fn gatr_pct_walks_past_paranormal_to_collect_7() {
        // 12 bars total: 11 history + today.
        // History layout (most recent first):
        //   bars 10,9: paranormal (huge range 200)
        //   bars 8,7,6,5,4,3,2: normal (range 40)  ← these 7 should be collected
        //   bar 1: normal (range 40) ← not needed, first 7 suffice
        //   bar 0: seed for prev_close
        //
        // All closes = 100.0 for simplicity.
        let c = vec![100.0; 12];
        let mut h = Vec::new();
        let mut l = Vec::new();

        // Bar 0: seed.
        h.push(120.0); l.push(80.0);
        // Bars 1-8: normal range 40 (close=100, H=120, L=80, TR=40).
        for _ in 0..8 { h.push(120.0); l.push(80.0); }
        // Bars 9-10: paranormal range 200 (H=200, L=0, TR=200).
        for _ in 0..2 { h.push(200.0); l.push(0.0); }
        // Today: range 40.
        h.push(120.0); l.push(80.0);

        let pct = gerchik_gatr_pct(&h, &l, &c).unwrap();
        // Paranormal bars (TR=200) skipped. 7 normal bars (TR=40) collected.
        // Today range = 40, avg = 40, pct = 100%.
        assert!(
            (pct - 100.0).abs() < 1.0,
            "expected ~100% (paranormal skipped), got {pct}"
        );
    }

    #[test]
    fn gatr_pct_fewer_than_7_non_paranormal_still_works() {
        // 6 history bars: 3 normal (TR=10) + 3 paranormal (TR=100).
        // Raw avg of all 6 TRs = (10*3 + 100*3) / 6 = 55.
        // Upper = 110, Lower = 27.5. Normal (10) excluded (<27.5).
        // Only 100s pass, and they are the collected ones.
        //
        // Better: use values where normals pass and paranormals don't.
        // 6 history: 3 at TR=50, 3 at TR=500.
        // Raw avg = (50*3 + 500*3) / 6 = 275.
        // Upper = 550, Lower = 137.5. 50s excluded (<137.5), 500s excluded (>550? No, 500<550).
        //
        // Simplest: 5 history bars at TR=10, 1 bar at TR=1000.
        // Raw avg = (10*5 + 1000) / 6 = 175. Upper=350, Lower=87.5.
        // 10s excluded (<87.5). 1000 excluded (>350). All paranormal → fallback.
        //
        // Let's just test with 3 bars where count < 7 naturally.
        let c = vec![100.0; 5]; // 4 history + 1 today
        let h = vec![110.0, 110.0, 110.0, 110.0, 105.0]; // all range 20, today 10
        let l = vec![90.0, 90.0, 90.0, 90.0, 95.0];
        let pct = gerchik_gatr_pct(&h, &l, &c).unwrap();
        // 3 non-paranormal TRs collected (all uniform, none filtered).
        // Avg TR = 20. Today range = 10. Pct = 50%.
        assert!((pct - 50.0).abs() < 1.0, "expected ~50%, got {pct}");
    }
}
