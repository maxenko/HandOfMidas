use super::*;
use midas_core::Timeframe;
use personality::ticker_seed;

// -- Seed determinism -------------------------------------------------

#[test]
fn same_ticker_same_seed() {
    assert_eq!(ticker_seed("AAPL"), ticker_seed("AAPL"));
    assert_eq!(ticker_seed("TSLA"), ticker_seed("TSLA"));
}

#[test]
fn different_tickers_different_seeds() {
    assert_ne!(ticker_seed("AAPL"), ticker_seed("TSLA"));
    assert_ne!(ticker_seed("MSFT"), ticker_seed("GOOG"));
}

// -- Provider determinism ---------------------------------------------

#[test]
fn same_ticker_same_daily_data() {
    let mut p = TestDataProvider::new();
    let a: Vec<OhlcvBar> = p.daily_bars("AAPL").to_vec();

    let mut p2 = TestDataProvider::new();
    let b: Vec<OhlcvBar> = p2.daily_bars("AAPL").to_vec();

    assert_eq!(a.len(), b.len());
    for (x, y) in a.iter().zip(b.iter()) {
        assert_eq!(x.timestamp, y.timestamp);
        assert_eq!(x.open, y.open);
        assert_eq!(x.close, y.close);
    }
}

#[test]
fn different_tickers_different_data() {
    let mut p = TestDataProvider::new();
    let aapl = p.daily_bars("AAPL")[0].open;
    let tsla = p.daily_bars("TSLA")[0].open;
    assert_ne!(aapl, tsla);
}

// -- Daily bar quality ------------------------------------------------

#[test]
fn daily_ohlc_constraints() {
    let mut p = TestDataProvider::new();
    for bar in p.daily_bars("AAPL") {
        assert!(bar.high >= bar.open, "H < O: {bar:?}");
        assert!(bar.high >= bar.close, "H < C: {bar:?}");
        assert!(bar.low <= bar.open, "L > O: {bar:?}");
        assert!(bar.low <= bar.close, "L > C: {bar:?}");
        assert!(bar.low > 0.0, "L <= 0: {bar:?}");
        assert!(bar.volume > 0, "vol <= 0: {bar:?}");
    }
}

#[test]
fn daily_prices_stay_positive() {
    let mut p = TestDataProvider::new();
    // Test several tickers across all personality types
    for ticker in &["AAPL", "TSLA", "GME", "KO", "XOM", "NVDA", "AMZN"] {
        for bar in p.daily_bars(ticker) {
            assert!(bar.close > 0.0, "{ticker} close <= 0: {bar:?}");
            assert!(bar.low > 0.0, "{ticker} low <= 0: {bar:?}");
        }
    }
}

#[test]
fn daily_bars_skip_weekends() {
    let mut p = TestDataProvider::new();
    let bars = p.daily_bars("AAPL");
    for pair in bars.windows(2) {
        let gap = pair[1].timestamp - pair[0].timestamp;
        // Mon-Thu -> 86400, Fri -> 259200 (3 days)
        assert!(
            gap == 86400 || gap == 259200,
            "unexpected day gap: {gap}s between ts {} and {}",
            pair[0].timestamp,
            pair[1].timestamp,
        );
    }
}

#[test]
fn daily_data_covers_decade() {
    let mut p = TestDataProvider::new();
    let (start, end) = p.date_range("AAPL");
    let years = (end - start) as f64 / (365.25 * 86400.0);
    assert!(years > 9.0, "only {years:.1} years of data");
}

// -- Date range queries -----------------------------------------------

#[test]
fn date_range_filter_works() {
    let mut p = TestDataProvider::new();
    let (data_start, _) = p.date_range("AAPL");
    // Request 30 trading days from the start
    let end = data_start + 45 * 86400; // ~45 calendar days ~ 30 trading days
    let bars = p.bars("AAPL", Timeframe::D1, data_start, end);
    for bar in &bars {
        assert!(bar.timestamp >= data_start);
        assert!(bar.timestamp < end);
    }
    assert!(!bars.is_empty());
}

#[test]
fn bars_last_days_convenience() {
    let mut p = TestDataProvider::new();
    let bars = p.bars_last_days("AAPL", Timeframe::D1, 60);
    // ~60 calendar days ~ ~42 trading days
    assert!(bars.len() > 30 && bars.len() < 60);
}

#[test]
fn bars_last_months_convenience() {
    let mut p = TestDataProvider::new();
    let bars = p.bars_last_months("AAPL", Timeframe::D1, 3);
    // 3 months ~ 90 calendar days ~ ~63 trading days
    assert!(bars.len() > 50 && bars.len() < 80);
}

// -- Intraday quality -------------------------------------------------

#[test]
fn intraday_ohlc_constraints() {
    let mut p = TestDataProvider::new();
    let bars = p.bars_last_days("AAPL", Timeframe::S30, 2);
    assert!(!bars.is_empty(), "no intraday bars");
    for bar in &bars {
        assert!(bar.high >= bar.open, "H < O: {bar:?}");
        assert!(bar.high >= bar.close, "H < C: {bar:?}");
        assert!(bar.low <= bar.open, "L > O: {bar:?}");
        assert!(bar.low <= bar.close, "L > C: {bar:?}");
        assert!(bar.low > 0.0, "L <= 0: {bar:?}");
        assert!(bar.volume > 0, "vol <= 0: {bar:?}");
    }
}

#[test]
fn intraday_consistent_with_daily() {
    let mut p = TestDataProvider::new();
    let daily = p.daily_bars("MSFT").to_vec();
    // Pick a day in the middle
    let day = &daily[daily.len() / 2];
    let day_end = day.timestamp + 86400;
    let intraday = p.bars("MSFT", Timeframe::S30, day.timestamp, day_end);
    assert!(!intraday.is_empty());
    // First intraday open == daily open
    assert_eq!(
        intraday.first().unwrap().open,
        day.open,
        "first intraday open != daily open"
    );
    // Last intraday close == daily close
    assert_eq!(
        intraday.last().unwrap().close,
        day.close,
        "last intraday close != daily close"
    );
    // Max intraday high == daily high
    let max_high = intraday
        .iter()
        .map(|b| b.high)
        .fold(f64::NEG_INFINITY, f64::max);
    assert_eq!(max_high, day.high, "max intraday high != daily high");
    // Min intraday low == daily low
    let min_low = intraday.iter().map(|b| b.low).fold(f64::INFINITY, f64::min);
    assert_eq!(min_low, day.low, "min intraday low != daily low");
    // Sum of intraday volumes == daily volume
    let vol_sum: i64 = intraday.iter().map(|b| b.volume).sum();
    assert_eq!(vol_sum, day.volume, "intraday volume sum != daily volume");
}

/// Verify that aggregating M1 bars for every day matches the daily bar
/// exactly (open, high, low, close, volume) across multiple tickers.
#[test]
fn m1_aggregates_to_daily_for_all_tickers() {
    let mut p = TestDataProvider::new();
    for ticker in &["AAPL", "TSLA", "GME", "KO", "XOM", "NVDA", "AMZN"] {
        let daily = p.daily_bars(ticker).to_vec();
        // Test 20 evenly-spaced days across the dataset
        let step = daily.len() / 20;
        for idx in (0..daily.len()).step_by(step.max(1)).take(20) {
            let day = &daily[idx];
            let day_end = day.timestamp + 86400;
            let m1 = p.bars(ticker, Timeframe::M1, day.timestamp, day_end);
            assert!(!m1.is_empty(), "{ticker} day {idx}: no M1 bars");

            let agg_open = m1.first().unwrap().open;
            let agg_close = m1.last().unwrap().close;
            let agg_high = m1.iter().map(|b| b.high).fold(f64::NEG_INFINITY, f64::max);
            let agg_low = m1.iter().map(|b| b.low).fold(f64::INFINITY, f64::min);
            let agg_vol: i64 = m1.iter().map(|b| b.volume).sum();

            assert_eq!(agg_open, day.open, "{ticker} day {idx}: open mismatch");
            assert_eq!(agg_high, day.high, "{ticker} day {idx}: high mismatch");
            assert_eq!(agg_low, day.low, "{ticker} day {idx}: low mismatch");
            assert_eq!(agg_close, day.close, "{ticker} day {idx}: close mismatch");
            assert_eq!(agg_vol, day.volume, "{ticker} day {idx}: volume mismatch");
        }
    }
}

/// Verify consistency across timeframe chain: S30 -> M1 -> M5 -> H1 all
/// produce the same daily OHLCV when aggregated.
#[test]
fn timeframe_chain_consistent() {
    let mut p = TestDataProvider::new();
    let daily = p.daily_bars("AAPL").to_vec();
    let day = &daily[daily.len() / 2];
    let day_end = day.timestamp + 86400;

    for tf in [
        Timeframe::S30,
        Timeframe::M1,
        Timeframe::M5,
        Timeframe::M15,
        Timeframe::H1,
    ] {
        let bars = p.bars("AAPL", tf, day.timestamp, day_end);
        assert!(!bars.is_empty(), "{tf}: no bars");

        let agg_open = bars.first().unwrap().open;
        let agg_close = bars.last().unwrap().close;
        let agg_high = bars
            .iter()
            .map(|b| b.high)
            .fold(f64::NEG_INFINITY, f64::max);
        let agg_low = bars.iter().map(|b| b.low).fold(f64::INFINITY, f64::min);

        assert_eq!(agg_open, day.open, "{tf}: open mismatch");
        assert_eq!(agg_high, day.high, "{tf}: high mismatch");
        assert_eq!(agg_low, day.low, "{tf}: low mismatch");
        assert_eq!(agg_close, day.close, "{tf}: close mismatch");
    }
}

#[test]
fn intraday_deterministic() {
    let mut p1 = TestDataProvider::new();
    let mut p2 = TestDataProvider::new();
    let a = p1.bars_last_days("GOOG", Timeframe::M5, 5);
    let b = p2.bars_last_days("GOOG", Timeframe::M5, 5);
    assert_eq!(a.len(), b.len());
    for (x, y) in a.iter().zip(b.iter()) {
        assert_eq!(x.timestamp, y.timestamp);
        assert_eq!(x.open, y.open);
    }
}

// -- Multi-timeframe --------------------------------------------------

#[test]
fn multiple_timeframes_from_same_provider() {
    let mut p = TestDataProvider::new();
    let ticker = "NVDA";
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
        let bars = p.bars_last_days(ticker, tf, 30);
        assert!(!bars.is_empty(), "{tf} returned no bars");
    }
}

#[test]
fn coarser_timeframe_has_fewer_bars() {
    let mut p = TestDataProvider::new();
    let m1 = p.bars_last_days("AAPL", Timeframe::M1, 10);
    let m5 = p.bars_last_days("AAPL", Timeframe::M5, 10);
    let h1 = p.bars_last_days("AAPL", Timeframe::H1, 10);
    assert!(m1.len() > m5.len());
    assert!(m5.len() > h1.len());
}

#[test]
#[should_panic(expected = "finest resolution is S30")]
fn panics_on_sub_s30() {
    let mut p = TestDataProvider::new();
    p.bars_last_days("AAPL", Timeframe::S1, 1);
}

// -- Aggregation ------------------------------------------------------

#[test]
fn aggregate_preserves_ohlc() {
    let mut p = TestDataProvider::new();
    let s30 = p.bars_last_days("AAPL", Timeframe::S30, 5);
    let m5 = aggregate_bars(&s30, Timeframe::M5);
    for bar in &m5 {
        assert!(bar.high >= bar.open, "agg H < O: {bar:?}");
        assert!(bar.high >= bar.close, "agg H < C: {bar:?}");
        assert!(bar.low <= bar.open, "agg L > O: {bar:?}");
        assert!(bar.low <= bar.close, "agg L > C: {bar:?}");
        assert!(bar.volume > 0, "agg vol <= 0: {bar:?}");
    }
}

#[test]
fn aggregate_correctness() {
    // 10 source bars at known timestamps
    let source: Vec<OhlcvBar> = (0..10)
        .map(|i| OhlcvBar {
            timestamp: i * 60, // M1 spacing
            open: 100.0 + i as f64,
            high: 105.0 + i as f64,
            low: 95.0 + i as f64,
            close: 101.0 + i as f64,
            volume: 1000,
        })
        .collect();

    let m5 = aggregate_bars(&source, Timeframe::M5);
    assert_eq!(m5.len(), 2);

    // First bucket: bars 0..5
    assert_eq!(m5[0].open, source[0].open);
    assert_eq!(m5[0].close, source[4].close);
    let expected_high = source[..5]
        .iter()
        .map(|b| b.high)
        .fold(f64::NEG_INFINITY, f64::max);
    assert_eq!(m5[0].high, expected_high);
    let expected_low = source[..5]
        .iter()
        .map(|b| b.low)
        .fold(f64::INFINITY, f64::min);
    assert_eq!(m5[0].low, expected_low);
    assert_eq!(m5[0].volume, 5000);
}

#[test]
fn aggregate_empty() {
    assert!(aggregate_bars(&[], Timeframe::H1).is_empty());
}

// -- Realism checks ---------------------------------------------------

#[test]
fn growth_ticker_trends_significantly() {
    let mut p = TestDataProvider::new();
    // Try multiple tickers -- at least one should show >2x movement
    let mut found_big_move = false;
    for ticker in &["AAPL", "TSLA", "GME", "NVDA", "AMZN", "META", "NFLX"] {
        let bars = p.daily_bars(ticker);
        let start_price = bars.first().unwrap().close;
        let end_price = bars.last().unwrap().close;
        let max_price = bars.iter().map(|b| b.high).fold(0.0_f64, f64::max);
        let min_price = bars.iter().map(|b| b.low).fold(f64::INFINITY, f64::min);
        let total_range = max_price / min_price;
        if total_range > 2.0 || (end_price / start_price).abs() > 1.5 {
            found_big_move = true;
            break;
        }
    }
    assert!(found_big_move, "no ticker showed significant price travel");
}

#[test]
fn volume_correlates_with_moves() {
    let mut p = TestDataProvider::new();
    let bars = p.daily_bars("AAPL");

    // Split bars into "big move" (top 25%) and "small move" (bottom 25%)
    let mut returns: Vec<(f64, i64)> = bars
        .iter()
        .map(|b| ((b.close - b.open).abs() / b.open, b.volume))
        .collect();
    returns.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());

    let q1 = returns.len() / 4;
    let q3 = returns.len() * 3 / 4;

    let avg_vol_small: f64 = returns[..q1].iter().map(|r| r.1 as f64).sum::<f64>() / q1 as f64;
    let avg_vol_big: f64 =
        returns[q3..].iter().map(|r| r.1 as f64).sum::<f64>() / (returns.len() - q3) as f64;

    assert!(
        avg_vol_big > avg_vol_small,
        "big-move avg vol ({avg_vol_big:.0}) should exceed small-move ({avg_vol_small:.0})"
    );
}
