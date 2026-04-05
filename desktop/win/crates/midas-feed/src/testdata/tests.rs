use super::*;

#[test]
fn deterministic_output() {
    let mut p1 = TestDataProvider::new();
    let mut p2 = TestDataProvider::new();
    let a = p1.get_candles("AAPL", Timeframe::D1, 60);
    let b = p2.get_candles("AAPL", Timeframe::D1, 60);
    assert_eq!(a.len(), b.len());
    for i in 0..a.len() {
        assert_eq!(a.timestamps[i], b.timestamps[i]);
        assert_eq!(a.opens[i], b.opens[i]);
        assert_eq!(a.closes[i], b.closes[i]);
    }
}

#[test]
fn different_tickers_produce_different_data() {
    let mut p = TestDataProvider::new();
    let aapl = p.get_candles("AAPL", Timeframe::D1, 30);
    let tsla = p.get_candles("TSLA", Timeframe::D1, 30);
    assert_ne!(aapl.opens[0], tsla.opens[0]);
}

#[test]
fn any_ticker_works() {
    let mut p = TestDataProvider::new();
    for ticker in &["AAPL", "TSLA", "XYZ", "FOO123", "BTC", "MIDAS"] {
        let buf = p.get_candles(ticker, Timeframe::D1, 30);
        assert!(buf.len() > 15, "{ticker} has too few bars: {}", buf.len());
    }
}

#[test]
fn timestamps_are_epoch_milliseconds() {
    let mut p = TestDataProvider::new();
    let buf = p.get_candles("MSFT", Timeframe::D1, 10);
    assert!(!buf.is_empty());
    // Epoch ms for 2016+ should be > 1.4 trillion
    assert!(buf.timestamps[0] > 1_400_000_000_000);
}

#[test]
fn timestamps_monotonically_increasing() {
    let mut p = TestDataProvider::new();
    let buf = p.get_candles("GOOG", Timeframe::D1, 365);
    for i in 1..buf.len() {
        assert!(
            buf.timestamps[i] > buf.timestamps[i - 1],
            "not monotonic at index {i}"
        );
    }
}

#[test]
fn ohlc_constraints_hold() {
    let mut p = TestDataProvider::new();
    let buf = p.get_candles("NVDA", Timeframe::D1, 365);
    for i in 0..buf.len() {
        let h = buf.highs[i];
        let l = buf.lows[i];
        let o = buf.opens[i];
        let c = buf.closes[i];
        assert!(h >= o, "H < O at {i}");
        assert!(h >= c, "H < C at {i}");
        assert!(l <= o, "L > O at {i}");
        assert!(l <= c, "L > C at {i}");
        assert!(l > 0.0, "L <= 0 at {i}");
        assert!(buf.volumes[i] > 0, "vol <= 0 at {i}");
    }
}

#[test]
fn intraday_works() {
    let mut p = TestDataProvider::new();
    let buf = p.get_candles("AAPL", Timeframe::M5, 5);
    // 5 calendar days should produce intraday bars
    assert!(buf.len() > 10, "too few M5 bars: {}", buf.len());
}

#[test]
fn multiple_timeframes() {
    let mut p = TestDataProvider::new();
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
        let buf = p.get_candles("AMZN", tf, 30);
        assert!(!buf.is_empty(), "{tf} returned no bars");
    }
}

#[test]
fn coarser_has_fewer_bars() {
    let mut p = TestDataProvider::new();
    let m1 = p.get_candles("AAPL", Timeframe::M1, 10);
    let m5 = p.get_candles("AAPL", Timeframe::M5, 10);
    let h1 = p.get_candles("AAPL", Timeframe::H1, 10);
    assert!(m1.len() > m5.len());
    assert!(m5.len() > h1.len());
}

#[test]
fn prices_stay_positive() {
    let mut p = TestDataProvider::new();
    for ticker in &["AAPL", "TSLA", "GME", "KO", "XOM", "NVDA", "AMZN"] {
        let buf = p.get_candles(ticker, Timeframe::D1, 3650);
        for i in 0..buf.len() {
            assert!(buf.closes[i] > 0.0, "{ticker} close <= 0 at {i}");
            assert!(buf.lows[i] > 0.0, "{ticker} low <= 0 at {i}");
        }
    }
}

#[test]
fn daily_bars_skip_weekends() {
    let mut p = TestDataProvider::new();
    let buf = p.get_candles("AAPL", Timeframe::D1, 365);
    for i in 1..buf.len() {
        let gap_ms = buf.timestamps[i] - buf.timestamps[i - 1];
        let gap_s = gap_ms / 1000;
        assert!(
            gap_s == 86400 || gap_s == 259200,
            "unexpected gap: {gap_s}s at index {i}"
        );
    }
}

#[test]
#[should_panic(expected = "finest resolution is S30")]
fn panics_on_sub_s30() {
    let mut p = TestDataProvider::new();
    p.get_candles("AAPL", Timeframe::S1, 1);
}

/// Verify that M1 bars for each day aggregate exactly to the daily bar:
/// same open, high, low, close, and volume.
#[test]
fn m1_aggregates_to_daily_for_all_tickers() {
    let mut p = TestDataProvider::new();
    for ticker in &["AAPL", "TSLA", "GME", "KO", "XOM", "NVDA", "AMZN"] {
        let daily = p.get_candles(ticker, Timeframe::D1, 3650);
        // Test 20 evenly-spaced days
        let step = daily.len() / 20;
        for d in (0..daily.len()).step_by(step.max(1)).take(20) {
            let day_ts_ms = daily.timestamps[d];
            let day_open = daily.opens[d];
            let day_high = daily.highs[d];
            let day_low = daily.lows[d];
            let day_close = daily.closes[d];
            let day_vol = daily.volumes[d];

            // Get all M1 bars for this day (need epoch-second range for
            // the internal provider). Day timestamp in ms → start/end in
            // seconds passed through get_candles_range.
            let day_start_s = day_ts_ms / 1000;
            let day_end_s = day_start_s + 86400;

            // Use internal bars method via ensure_ticker + bars_intraday
            // We access through get_candles with a 1-day window centered
            // on this day. Since get_candles counts backwards from dataset
            // end, we use the raw bars_intraday path instead.
            let m1 = p.get_candles_range(ticker, Timeframe::M1, day_start_s, day_end_s);
            assert!(!m1.is_empty(), "{ticker} day {d}: no M1 bars");

            let agg_open = m1.opens[0];
            let agg_close = m1.closes[m1.len() - 1];
            let agg_high = m1.highs.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
            let agg_low = m1.lows.iter().cloned().fold(f32::INFINITY, f32::min);
            let agg_vol: u32 = m1.volumes.iter().sum();

            assert_eq!(agg_open, day_open, "{ticker} day {d}: open mismatch");
            assert_eq!(agg_high, day_high, "{ticker} day {d}: high mismatch");
            assert_eq!(agg_low, day_low, "{ticker} day {d}: low mismatch");
            assert_eq!(agg_close, day_close, "{ticker} day {d}: close mismatch");
            assert_eq!(agg_vol, day_vol, "{ticker} day {d}: volume mismatch");
        }
    }
}
