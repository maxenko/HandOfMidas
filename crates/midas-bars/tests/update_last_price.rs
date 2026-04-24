//! Slice 2c of chart-transition: `CandleSeries::update_last_price`
//! regression suite. Mirrors the `fad1878` `CandleBuffer::
//! update_last_price` test suite against the new-stack series.
//!
//! Covers:
//! - Empty series is a no-op.
//! - New high extends the last candle's high.
//! - New low extends the last candle's low.
//! - In-range price only moves close (high/low untouched).
//! - Only the LAST candle is touched — earlier rows stay stable.
//! - Version bumps after every successful fold.
//! - Non-finite inputs (NaN / ±Inf) are rejected — no column or
//!   version change.

use chrono::TimeZone;
use midas_bars::{BarPeriod, Candle, CandleSeries, Completeness, Ohlcv, Symbol};
use midas_calendar::{xnys, Timestamp};

fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> Timestamp {
    chrono::Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
}

fn mk_candle(ts: Timestamp, o: f64, h: f64, l: f64, c: f64) -> Candle {
    let cal = xnys();
    let sym = Symbol::new("SPY", cal.id());
    let session = cal.classify(ts);
    let window = cal.bar_window(ts, BarPeriod::m1()).unwrap();
    let ohlcv = Ohlcv::new(o, h, l, c, 1_000, 1, None).unwrap();
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

fn fresh_series() -> CandleSeries {
    let cal = xnys();
    CandleSeries::new(cal.id(), BarPeriod::m1(), Symbol::new("SPY", cal.id()))
}

#[test]
fn update_last_price_on_empty_is_noop() {
    let mut s = fresh_series();
    s.update_last_price(100.0);
    assert!(s.is_empty());
    assert_eq!(s.version(), 0);
}

#[test]
fn update_last_price_moves_close_and_extends_high() {
    let mut s = fresh_series();
    let ts = utc(2024, 1, 17, 14, 30);
    s.push(mk_candle(ts, 100.0, 101.0, 99.0, 100.5));
    s.update_last_price(102.0);
    let row = s.at(0).unwrap();
    assert_eq!(row.close(), 102.0);
    assert_eq!(row.high(), 102.0, "extended from 101 → 102");
    assert_eq!(row.low(), 99.0, "low unchanged");
    assert_eq!(row.open(), 100.0, "open unchanged");
    assert_eq!(row.volume(), 1_000, "volume unchanged");
}

#[test]
fn update_last_price_extends_low() {
    let mut s = fresh_series();
    let ts = utc(2024, 1, 17, 14, 30);
    s.push(mk_candle(ts, 100.0, 101.0, 99.0, 100.5));
    s.update_last_price(98.0);
    let row = s.at(0).unwrap();
    assert_eq!(row.close(), 98.0);
    assert_eq!(row.low(), 98.0);
    assert_eq!(row.high(), 101.0, "high unchanged");
}

#[test]
fn update_last_price_inside_range_only_moves_close() {
    let mut s = fresh_series();
    let ts = utc(2024, 1, 17, 14, 30);
    s.push(mk_candle(ts, 100.0, 101.0, 99.0, 100.5));
    s.update_last_price(100.2);
    let row = s.at(0).unwrap();
    // Close is stored as f32 → tolerate sub-ULP drift on the round
    // trip.
    assert!((row.close() - 100.2).abs() < 1e-4, "close ≈ 100.2");
    assert_eq!(row.high(), 101.0, "high unchanged");
    assert_eq!(row.low(), 99.0, "low unchanged");
}

#[test]
fn update_last_price_bumps_version() {
    let mut s = fresh_series();
    let ts = utc(2024, 1, 17, 14, 30);
    s.push(mk_candle(ts, 100.0, 101.0, 99.0, 100.5));
    let v0 = s.version();
    s.update_last_price(100.2);
    assert_eq!(s.version(), v0 + 1);
}

#[test]
fn update_last_price_rejects_non_finite() {
    let mut s = fresh_series();
    let ts = utc(2024, 1, 17, 14, 30);
    s.push(mk_candle(ts, 100.0, 101.0, 99.0, 100.5));
    let v0 = s.version();
    s.update_last_price(f64::NAN);
    s.update_last_price(f64::INFINITY);
    s.update_last_price(f64::NEG_INFINITY);
    let row = s.at(0).unwrap();
    assert_eq!(row.close(), 100.5, "close unchanged");
    assert_eq!(s.version(), v0, "version must NOT bump on non-finite");
}

#[test]
fn update_last_price_only_touches_last_candle() {
    let mut s = fresh_series();
    let ts0 = utc(2024, 1, 17, 14, 30);
    let ts1 = utc(2024, 1, 17, 14, 31);
    s.push(mk_candle(ts0, 100.0, 101.0, 99.0, 100.5));
    s.push(mk_candle(ts1, 100.5, 102.0, 100.0, 101.5));
    s.update_last_price(105.0);
    // First candle untouched.
    let row0 = s.at(0).unwrap();
    assert_eq!(row0.close(), 100.5);
    assert_eq!(row0.high(), 101.0);
    // Second candle picked up the fold.
    let row1 = s.at(1).unwrap();
    assert_eq!(row1.close(), 105.0);
    assert_eq!(row1.high(), 105.0);
}
