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
    h.push(120.0);
    l.push(80.0);
    // Bars 1-8: normal TR=40 (close=100, H=120, L=80).
    for _ in 0..8 {
        h.push(120.0);
        l.push(80.0);
    }
    // Bars 9-10: paranormal TR=200 (H=200, L=0).
    for _ in 0..2 {
        h.push(200.0);
        l.push(0.0);
    }
    // Today: TR=40.
    h.push(120.0);
    l.push(80.0);

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
    h.push(120.0);
    l.push(80.0); // bar 0: seed
    for _ in 0..8 {
        h.push(120.0);
        l.push(80.0);
    } // bars 1-8: normal
    for _ in 0..2 {
        h.push(200.0);
        l.push(0.0);
    } // bars 9-10: paranormal
    h.push(120.0);
    l.push(80.0); // today

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
fn gatr_detail_avg_atr_uniform() {
    // 10 bars: 9 history + 1 today. All uniform TR=20, no gaps.
    let h = vec![110.0; 10];
    let l = vec![90.0; 10];
    let c = vec![100.0; 10];
    let result = gerchik_gatr_detail(&h, &l, &c).unwrap();
    assert!(
        (result.avg_atr - 20.0).abs() < 1e-6,
        "expected avg_atr ≈ 20.0, got {}",
        result.avg_atr
    );
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
