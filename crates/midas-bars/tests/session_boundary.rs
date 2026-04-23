//! Integration test: push 100 M1 candles across a session boundary on
//! XNYS. Verifies that `CandleSeries` stores one `SessionKind` byte per
//! row and that `CandleRef::session` reconstructs the full `Session`
//! correctly at the PreMarket → Regular transition.

use chrono::{Duration, TimeZone, Utc};

use midas_bars::{Candle, CandleSeries, Completeness, Ohlcv, Symbol};
use midas_calendar::{xnys, BarPeriod, SessionKind};

fn mk_candle(ts: chrono::DateTime<Utc>, price: f64) -> Candle {
    let cal = xnys();
    let sym = Symbol::new("SPY", cal.id());
    let session = cal.classify(ts);
    let window = cal.bar_window(ts, BarPeriod::m1()).unwrap();
    let ohlcv = Ohlcv::new(price, price + 0.05, price - 0.05, price, 200, 4, None).unwrap();
    Candle::new(
        sym,
        cal,
        BarPeriod::m1(),
        session,
        window,
        ohlcv,
        Completeness::Completed,
    )
    .unwrap()
}

#[test]
fn push_100_m1_candles_across_premarket_regular_boundary() {
    let cal = xnys();
    let sym = Symbol::new("SPY", cal.id());
    let mut series = CandleSeries::new(cal.id(), BarPeriod::m1(), sym);

    // 08:00 ET = 13:00 UTC on 2024-01-17 (EST, UTC-5).
    let start = Utc.with_ymd_and_hms(2024, 1, 17, 13, 0, 0).unwrap();

    // Push 100 consecutive M1 bars. The PreMarket → Regular transition
    // happens at minute 90 (09:30 ET).
    for i in 0..100 {
        let ts = start + Duration::minutes(i);
        series.push(mk_candle(ts, 100.0 + (i as f64) * 0.01));
    }
    assert_eq!(series.len(), 100);
    assert_eq!(series.version(), 100);

    // Indices 0..90 are PreMarket, 90..100 are Regular.
    for i in 0..90 {
        let r = series.at(i).unwrap();
        assert_eq!(
            r.session_kind(),
            SessionKind::PreMarket,
            "row {i} should be PreMarket (minute {} from 08:00 ET)",
            i
        );
        assert_eq!(r.session(cal).kind(), SessionKind::PreMarket);
    }
    for i in 90..100 {
        let r = series.at(i).unwrap();
        assert_eq!(
            r.session_kind(),
            SessionKind::Regular,
            "row {i} should be Regular (minute {} from 08:00 ET)",
            i
        );
        assert_eq!(r.session(cal).kind(), SessionKind::Regular);
    }
}

#[test]
fn session_boundary_index_is_exactly_90() {
    // Regression guard: the transition lives at row 90, not 89 or 91.
    let cal = xnys();
    let sym = Symbol::new("SPY", cal.id());
    let mut series = CandleSeries::new(cal.id(), BarPeriod::m1(), sym);
    let start = Utc.with_ymd_and_hms(2024, 1, 17, 13, 0, 0).unwrap();

    for i in 0..100 {
        let ts = start + Duration::minutes(i);
        series.push(mk_candle(ts, 100.0));
    }

    let kinds: Vec<SessionKind> = series.iter().map(|r| r.session_kind()).collect();
    let first_regular = kinds
        .iter()
        .position(|k| *k == SessionKind::Regular)
        .expect("at least one regular bar");
    assert_eq!(
        first_regular, 90,
        "first Regular bar must be at row 90 (09:30 ET = minute 90 from 08:00 ET)"
    );
}

#[test]
fn ts_close_lines_up_across_session_boundary() {
    // Ensures bar_window routing through the calendar gives the correct
    // close timestamp on both sides of 09:30 ET.
    let cal = xnys();
    let sym = Symbol::new("SPY", cal.id());
    let mut series = CandleSeries::new(cal.id(), BarPeriod::m1(), sym);
    let start = Utc.with_ymd_and_hms(2024, 1, 17, 13, 0, 0).unwrap();

    for i in 0..100 {
        let ts = start + Duration::minutes(i);
        series.push(mk_candle(ts, 100.0));
    }

    for i in 0..100 {
        let r = series.at(i).unwrap();
        let expected_close = r.ts_open() + Duration::minutes(1);
        assert_eq!(
            r.ts_close(cal),
            expected_close,
            "row {i}: m1 bar close must be 1 minute after open",
        );
    }
}
