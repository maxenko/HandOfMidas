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
pub fn gerchik_gatr_detail(
    highs: &[f64],
    lows: &[f64],
    closes: &[f64],
) -> Option<GatrResult> {
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

    let pct = if selected_bars.is_empty() {
        (today_range / raw_avg * 100.0) as f32
    } else {
        let avg = sum / selected_bars.len() as f64;
        (today_range / avg * 100.0) as f32
    };

    // Reverse so indices are in ascending order.
    selected_bars.reverse();

    Some(GatrResult { pct, selected_bars })
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
    fn gatr_color_up_is_green() {
        assert_eq!(gatr_color(true), GATR_COLOR_GREEN);
    }

    #[test]
    fn gatr_color_down_is_red() {
        assert_eq!(gatr_color(false), GATR_COLOR_RED);
    }

    // ── gerchik_gatr_pct ────────────────────────────────────────

    #[test]
    fn gatr_pct_too_few_bars() {
        // Need at least 2 bars (1 history + 1 today).
        assert!(gerchik_gatr_pct(&[110.0], &[90.0], &[100.0]).is_none());
        assert!(gerchik_gatr_pct(&[], &[], &[]).is_none());
    }

    #[test]
    fn gatr_pct_uniform_no_gaps() {
        // 10 bars, all with same H/L/C → no gaps, TR = H-L = 20 for all.
        let h = vec![110.0; 10];
        let l = vec![90.0; 10];
        let c = vec![100.0; 10];
        let pct = gerchik_gatr_pct(&h, &l, &c).unwrap();
        // Today TR = 20, avg of 7 recent TRs = 20, pct = 100%.
        assert!((pct - 100.0).abs() < 1.0, "expected ~100%, got {pct}");
    }

    #[test]
    fn gatr_pct_skips_today() {
        // 9 history bars with TR 20, today with TR 10.
        let mut h = vec![110.0; 9];
        h.push(105.0); // today high
        let mut l = vec![90.0; 9];
        l.push(95.0); // today low
        let c = vec![100.0; 10]; // no gaps
        let pct = gerchik_gatr_pct(&h, &l, &c).unwrap();
        // Today TR = 10, avg TR = 20, pct = 50%.
        assert!((pct - 50.0).abs() < 1.0, "expected ~50%, got {pct}");
    }

    #[test]
    fn gatr_pct_gap_up_does_not_inflate_today() {
        // Yesterday closed at 100. Today gaps up: H=115, L=108.
        // H-L = 7 (today's actual range). Gap is 8 points but doesn't
        // count toward today's traveled range.
        // History: 8 bars, all TR=15, no gaps.
        let mut h = vec![115.0; 8];
        h.push(115.0); // today
        let mut l = vec![100.0; 8];
        l.push(108.0); // today
        let mut c = vec![100.0; 8];
        c.push(112.0); // today's close
        // Today H-L = 7, avg TR = 15, pct = 7/15*100 ≈ 46.7%.
        let pct = gerchik_gatr_pct(&h, &l, &c).unwrap();
        let expected = 7.0 / 15.0 * 100.0;
        assert!(
            (pct - expected as f32).abs() < 1.0,
            "expected ~{expected:.0}%, got {pct}"
        );
    }

    #[test]
    fn gatr_pct_walks_past_paranormal_to_collect_7() {
        // 12 bars total: 11 history + today.
        // History layout (most recent first):
        //   bars 10,9: paranormal (huge range 200)
        //   bars 1-8: normal (TR=40) — 8 bars, 7 most recent collected
        //   bar 0: seed for prev_close
        let c = vec![100.0; 12];
        let mut h = Vec::new();
        let mut l = Vec::new();

        // Bar 0: seed.
        h.push(120.0); l.push(80.0);
        // Bars 1-8: normal TR=40 (close=100, H=120, L=80).
        for _ in 0..8 { h.push(120.0); l.push(80.0); }
        // Bars 9-10: paranormal TR=200 (H=200, L=0).
        for _ in 0..2 { h.push(200.0); l.push(0.0); }
        // Today: TR=40.
        h.push(120.0); l.push(80.0);

        let pct = gerchik_gatr_pct(&h, &l, &c).unwrap();
        // Paranormal bars (TR=200) skipped. 7 normal bars (TR=40) collected.
        // Today TR = 40, avg = 40, pct = 100%.
        assert!(
            (pct - 100.0).abs() < 1.0,
            "expected ~100% (paranormal skipped), got {pct}"
        );
    }

    #[test]
    fn gatr_pct_fewer_than_7_non_paranormal_still_works() {
        // 4 history bars + 1 today.
        let c = vec![100.0; 5];
        let h = vec![110.0, 110.0, 110.0, 110.0, 105.0]; // today H-L=10
        let l = vec![90.0, 90.0, 90.0, 90.0, 95.0];
        let pct = gerchik_gatr_pct(&h, &l, &c).unwrap();
        // 3 non-paranormal TRs collected (bars 1-3, bar 0 is seed).
        // Avg TR = 20. Today TR = 10. Pct = 50%.
        assert!((pct - 50.0).abs() < 1.0, "expected ~50%, got {pct}");
    }

    // ── gerchik_gatr_detail ────────────────────────────────────

    #[test]
    fn gatr_detail_returns_7_most_recent_uniform() {
        // 10 bars: 9 history + 1 today. All uniform, no gaps.
        let h = vec![110.0; 10];
        let l = vec![90.0; 10];
        let c = vec![100.0; 10];
        let result = gerchik_gatr_detail(&h, &l, &c).unwrap();
        assert!((result.pct - 100.0).abs() < 1.0);
        // 8 TRs from bars 1-8. All pass filter. 7 most recent = bars 2-8.
        assert_eq!(result.selected_bars.len(), 7);
        assert_eq!(result.selected_bars, vec![2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn gatr_detail_skips_paranormal_in_selected() {
        // 12 bars: 11 history + today.
        // Bars 9-10 are paranormal — should NOT appear in selected_bars.
        let c = vec![100.0; 12];
        let mut h = Vec::new();
        let mut l = Vec::new();
        h.push(120.0); l.push(80.0); // bar 0: seed
        for _ in 0..8 { h.push(120.0); l.push(80.0); } // bars 1-8: normal
        for _ in 0..2 { h.push(200.0); l.push(0.0); }  // bars 9-10: paranormal
        h.push(120.0); l.push(80.0); // today

        let result = gerchik_gatr_detail(&h, &l, &c).unwrap();
        assert_eq!(result.selected_bars.len(), 7);
        // Should be bars 2-8 (7 most recent non-paranormal).
        assert_eq!(result.selected_bars, vec![2, 3, 4, 5, 6, 7, 8]);
        // Paranormal indices 9,10 must not appear.
        assert!(!result.selected_bars.contains(&9));
        assert!(!result.selected_bars.contains(&10));
    }

    #[test]
    fn gatr_detail_fewer_than_7() {
        // 4 history bars + 1 today. Only 3 TRs (bars 1-3).
        let c = vec![100.0; 5];
        let h = vec![110.0, 110.0, 110.0, 110.0, 105.0];
        let l = vec![90.0, 90.0, 90.0, 90.0, 95.0];
        let result = gerchik_gatr_detail(&h, &l, &c).unwrap();
        assert_eq!(result.selected_bars.len(), 3);
        assert_eq!(result.selected_bars, vec![1, 2, 3]);
    }

    #[test]
    fn gatr_detail_indices_ascending() {
        // Verify selected_bars is sorted ascending.
        let h = vec![110.0; 10];
        let l = vec![90.0; 10];
        let c = vec![100.0; 10];
        let result = gerchik_gatr_detail(&h, &l, &c).unwrap();
        for w in result.selected_bars.windows(2) {
            assert!(w[0] < w[1], "selected_bars must be ascending");
        }
    }
}
