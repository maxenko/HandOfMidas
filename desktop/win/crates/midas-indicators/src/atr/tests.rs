use super::*;

#[test]
fn true_range_no_gap() {
    // Simple bar with no gap: TR = high - low.
    assert!((true_range(110.0, 100.0, Some(105.0)) - 10.0).abs() < f64::EPSILON);
}

#[test]
fn true_range_gap_up() {
    // Gap up: high - prev_close > high - low.
    assert!((true_range(120.0, 115.0, Some(100.0)) - 20.0).abs() < f64::EPSILON);
}

#[test]
fn true_range_gap_down() {
    // Gap down: prev_close - low > high - low.
    assert!((true_range(85.0, 80.0, Some(100.0)) - 20.0).abs() < f64::EPSILON);
}

#[test]
fn true_range_first_bar() {
    assert!((true_range(110.0, 100.0, None) - 10.0).abs() < f64::EPSILON);
}

#[test]
fn wilders_atr_sma_phase() {
    // During the first `length` bars, ATR = SMA of True Range.
    let mut atr = WildersAtr::new(3);
    atr.update_tr(10.0);
    assert!((atr.value() - 10.0).abs() < f64::EPSILON);
    atr.update_tr(20.0);
    assert!((atr.value() - 15.0).abs() < f64::EPSILON); // (10+20)/2
    atr.update_tr(30.0);
    assert!((atr.value() - 20.0).abs() < f64::EPSILON); // (10+20+30)/3
    assert!(atr.is_ready());
}

#[test]
fn wilders_atr_ema_phase() {
    let mut atr = WildersAtr::new(3);
    // Fill SMA phase.
    atr.update_tr(10.0);
    atr.update_tr(20.0);
    atr.update_tr(30.0);
    assert!((atr.value() - 20.0).abs() < f64::EPSILON);

    // Next bar: RMA = 40 * (1/3) + 20 * (2/3) = 13.333 + 13.333 = 26.667
    atr.update_tr(40.0);
    let expected = 40.0 / 3.0 + 20.0 * 2.0 / 3.0;
    assert!(
        (atr.value() - expected).abs() < 1e-10,
        "got {}, expected {}",
        atr.value(),
        expected
    );
}

#[test]
fn wilders_atr_converges() {
    // Constant TR should converge to that value.
    let mut atr = WildersAtr::new(14);
    for _ in 0..1000 {
        atr.update_tr(5.0);
    }
    assert!(
        (atr.value() - 5.0).abs() < 1e-10,
        "should converge to 5.0, got {}",
        atr.value()
    );
}

#[test]
fn wilders_atr_as_percent() {
    let mut atr = WildersAtr::new(1);
    atr.update_tr(5.0);
    assert!((atr.as_percent(100.0) - 5.0).abs() < f64::EPSILON);
    assert!((atr.as_percent(200.0) - 2.5).abs() < f64::EPSILON);
}

#[test]
#[should_panic]
fn wilders_atr_zero_length_panics() {
    WildersAtr::new(0);
}

// ── Gerchik ATR tests ────────────────────────────────────────────

#[test]
fn gerchik_atr_uniform_candles() {
    // All candles identical — none filtered, result = raw ATR.
    let g = GerchikAtr::new(5);
    let trs = vec![10.0, 10.0, 10.0, 10.0, 10.0];
    assert!((g.compute(&trs).unwrap() - 10.0).abs() < f64::EPSILON);
}

#[test]
fn gerchik_atr_filters_large_candle() {
    // 4 normal candles at 20, one huge at 200.
    // Raw ATR = (20*4 + 200) / 5 = 56.
    // Upper threshold = 56 * 2 = 112. The 200 is excluded.
    // Lower threshold = 56 * 0.5 = 28. The 20s are below 28... also excluded.
    // Use values that survive: 4 at 50, one at 200.
    // Raw ATR = (50*4 + 200) / 5 = 80.
    // Upper = 160. 200 excluded.
    // Lower = 40. 50s pass.
    // Filtered ATR = 50.
    let g = GerchikAtr::new(5);
    let trs = vec![50.0, 50.0, 200.0, 50.0, 50.0];
    assert!((g.compute(&trs).unwrap() - 50.0).abs() < f64::EPSILON);
}

#[test]
fn gerchik_atr_filters_tiny_candle() {
    // 4 normal candles at 20, one tiny at 1.
    // Raw ATR = (20*4 + 1) / 5 = 16.2.
    // Lower threshold = 16.2 * 0.5 = 8.1. The 1.0 is excluded.
    // Filtered ATR = 20.
    let g = GerchikAtr::new(5);
    let trs = vec![20.0, 20.0, 1.0, 20.0, 20.0];
    assert!((g.compute(&trs).unwrap() - 20.0).abs() < f64::EPSILON);
}

#[test]
fn gerchik_atr_all_paranormal_falls_back() {
    // Custom thresholds so tight that everything is excluded.
    let g = GerchikAtr::with_coefficients(3, 0.01, 100.0);
    let trs = vec![10.0, 20.0, 30.0];
    // Falls back to raw ATR = 20.
    assert!((g.compute(&trs).unwrap() - 20.0).abs() < f64::EPSILON);
}

#[test]
fn gerchik_atr_uses_last_n_bars() {
    // Provide more data than length — only last 3 used.
    let g = GerchikAtr::new(3);
    let trs = vec![999.0, 999.0, 10.0, 10.0, 10.0];
    assert!((g.compute(&trs).unwrap() - 10.0).abs() < f64::EPSILON);
}

#[test]
fn gerchik_atr_empty_returns_none() {
    let g = GerchikAtr::new(5);
    assert!(g.compute(&[]).is_none());
}

#[test]
#[should_panic]
fn gerchik_atr_zero_length_panics() {
    GerchikAtr::new(0);
}
