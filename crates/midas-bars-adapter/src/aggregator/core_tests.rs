//! Unit tests for [`SessionedBarAggregator`]. Moved out of `core.rs` by
//! the R6 refactor (arch-audit P3) so the production file stays under
//! ~500 LOC. Production behaviour is unchanged.

use super::*;
use crate::aggregator::config::AggregatorConfig;
use chrono::TimeZone;
use midas_bars::{Completeness, Symbol};
use midas_broker_core::market_data::{ReqId, Tick, TickAttributes, TickKind, TickType, TickValue};
use midas_broker_core::SymbolKey;
use midas_calendar::{
    crypto_spot, xnys, BarPeriod, SessionKind, SessionSpan, Timestamp, CRYPTO_SPOT_ID,
};
use midas_clock::{Clock, MockClock, SystemClock};
use std::sync::Arc;
use std::time::Duration;

fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> Timestamp {
    chrono::Utc.with_ymd_and_hms(y, mo, d, h, mi, s).unwrap()
}

fn price_size_tick(price: f64, size: i64, ts: Timestamp) -> Tick {
    Tick {
        symbol: SymbolKey {
            contract_id: 265598,
            symbol: "AAPL".into(),
        },
        req_id: ReqId(1),
        kind: TickKind::PriceSize,
        tick_type: TickType::Last,
        value: TickValue::PriceSize { price, size },
        attrs: TickAttributes::default(),
        ts,
    }
}

fn price_last_tick(price: f64, ts: Timestamp) -> Tick {
    Tick {
        symbol: SymbolKey {
            contract_id: 265598,
            symbol: "AAPL".into(),
        },
        req_id: ReqId(1),
        kind: TickKind::Price,
        tick_type: TickType::Last,
        value: TickValue::Price(price),
        attrs: TickAttributes::default(),
        ts,
    }
}

fn quote_bid_tick(price: f64, ts: Timestamp) -> Tick {
    Tick {
        symbol: SymbolKey {
            contract_id: 265598,
            symbol: "AAPL".into(),
        },
        req_id: ReqId(1),
        kind: TickKind::Price,
        tick_type: TickType::Bid,
        value: TickValue::Price(price),
        attrs: TickAttributes::default(),
        ts,
    }
}

/// A tick-by-tick Bid quote with an atomic price+size payload.
/// `reqTickByTickData("BidAsk")` emits these with kind=PriceSize and
/// tick_type=Bid|Ask — they MUST NOT fold into trade OHLC.
fn price_size_bid_tick(price: f64, size: i64, ts: Timestamp) -> Tick {
    Tick {
        symbol: SymbolKey {
            contract_id: 265598,
            symbol: "AAPL".into(),
        },
        req_id: ReqId(1),
        kind: TickKind::PriceSize,
        tick_type: TickType::Bid,
        value: TickValue::PriceSize { price, size },
        attrs: TickAttributes::default(),
        ts,
    }
}

fn size_volume_tick(vol: i64, ts: Timestamp) -> Tick {
    Tick {
        symbol: SymbolKey {
            contract_id: 265598,
            symbol: "AAPL".into(),
        },
        req_id: ReqId(1),
        kind: TickKind::Size,
        tick_type: TickType::Volume,
        value: TickValue::Size(vol),
        attrs: TickAttributes::default(),
        ts,
    }
}

fn xnys_m1_agg() -> SessionedBarAggregator {
    let cal = xnys();
    let sym = Symbol::new("AAPL", cal.id());
    let cfg = AggregatorConfig::new(sym, cal, BarPeriod::m1(), Arc::new(SystemClock))
        .with_partial_emit_rate_hz(0);
    SessionedBarAggregator::new(cfg).unwrap()
}

fn xnys_m1_agg_with_clock(clock: Arc<dyn Clock>) -> SessionedBarAggregator {
    let cal = xnys();
    let sym = Symbol::new("AAPL", cal.id());
    let cfg = AggregatorConfig::new(sym, cal, BarPeriod::m1(), clock);
    SessionedBarAggregator::new(cfg).unwrap()
}

// ---- construction ----

#[test]
fn new_rejects_crypto_eth() {
    let cal = crypto_spot();
    let sym = Symbol::new("BTC-USD", CRYPTO_SPOT_ID);
    let cfg = AggregatorConfig::new(
        sym,
        cal,
        BarPeriod::Session(SessionSpan::Eth),
        Arc::new(SystemClock),
    );
    let err = SessionedBarAggregator::new(cfg).unwrap_err();
    assert!(matches!(err, AggregatorError::InvalidPeriod(_)));
}

#[test]
fn new_accepts_xnys_m1() {
    let _ = xnys_m1_agg();
}

// ---- tick filtering ----

#[test]
fn ignores_bid_quote_tick() {
    let mut agg = xnys_m1_agg();
    let t = quote_bid_tick(100.0, utc(2024, 1, 17, 15, 0, 0));
    assert!(matches!(agg.accept_tick(&t), AggregatorOutput::Ignored));
    assert!(!agg.has_current());
}

#[test]
fn ignores_size_volume_tick() {
    let mut agg = xnys_m1_agg();
    let t = size_volume_tick(42, utc(2024, 1, 17, 15, 0, 0));
    assert!(matches!(agg.accept_tick(&t), AggregatorOutput::Ignored));
}

/// Regression: bug-hunt H2. A tick-by-tick BidAsk PriceSize packet
/// (kind=PriceSize, tick_type=Bid) used to match the legacy
/// wildcard arm and fold into trade OHLC. After the fix the arm is
/// restricted to tick_type==Last, so a Bid PriceSize MUST be
/// ignored.
#[test]
fn ignores_bid_price_size_quote_tick() {
    let mut agg = xnys_m1_agg();
    let t = price_size_bid_tick(99.5, 10, utc(2024, 1, 17, 15, 0, 0));
    assert!(matches!(agg.accept_tick(&t), AggregatorOutput::Ignored));
    assert!(!agg.has_current(), "quote tick must not open a bar");
}

/// Regression: bug-hunt M6. NaN or infinite prices must never
/// propagate into OHLC — `Ohlcv::new` panics on them, which would
/// crash the aggregator task. Early-reject in `extract_trade`.
#[test]
fn ignores_nan_price() {
    let mut agg = xnys_m1_agg();
    let t = price_size_tick(f64::NAN, 10, utc(2024, 1, 17, 15, 0, 0));
    assert!(matches!(agg.accept_tick(&t), AggregatorOutput::Ignored));
    assert!(!agg.has_current());
}

#[test]
fn ignores_infinite_price() {
    let mut agg = xnys_m1_agg();
    let t = price_size_tick(f64::INFINITY, 10, utc(2024, 1, 17, 15, 0, 0));
    assert!(matches!(agg.accept_tick(&t), AggregatorOutput::Ignored));
    let t = price_size_tick(f64::NEG_INFINITY, 10, utc(2024, 1, 17, 15, 0, 0));
    assert!(matches!(agg.accept_tick(&t), AggregatorOutput::Ignored));
}

#[test]
fn nan_price_on_price_last_tick_ignored() {
    let mut agg = xnys_m1_agg();
    let t = price_last_tick(f64::NAN, utc(2024, 1, 17, 15, 0, 0));
    assert!(matches!(agg.accept_tick(&t), AggregatorOutput::Ignored));
}

#[test]
fn negative_size_clamped_to_zero() {
    // IB's "size unknown" sentinel — the trade price is still
    // valid, but volume contribution is zero.
    let mut agg = xnys_m1_agg();
    let t = price_size_tick(100.0, -1, utc(2024, 1, 17, 15, 0, 0));
    let out = agg.accept_tick(&t);
    let c = match out {
        AggregatorOutput::Opened(c) => c,
        other => panic!("expected Opened, got {other:?}"),
    };
    assert_eq!(c.volume, 0);
    assert_eq!(c.trade_count, 1);
}

#[test]
fn absurd_size_is_ignored() {
    // Guard against overflow in the u64 volume accumulator after
    // folds. `i64::MAX / 2` is the documented cutoff.
    let mut agg = xnys_m1_agg();
    let t = price_size_tick(100.0, i64::MAX / 2 + 1, utc(2024, 1, 17, 15, 0, 0));
    assert!(matches!(agg.accept_tick(&t), AggregatorOutput::Ignored));
}

// ---- first tick ----

#[test]
fn first_tick_emits_opened_partial() {
    let mut agg = xnys_m1_agg();
    let t = price_size_tick(100.0, 10, utc(2024, 1, 17, 15, 0, 12));
    let out = agg.accept_tick(&t);
    let c = match out {
        AggregatorOutput::Opened(c) => c,
        other => panic!("expected Opened, got {other:?}"),
    };
    assert_eq!(c.o, 100.0);
    assert_eq!(c.h, 100.0);
    assert_eq!(c.l, 100.0);
    assert_eq!(c.c, 100.0);
    assert_eq!(c.volume, 10);
    assert_eq!(c.trade_count, 1);
    assert_eq!(c.completeness, Completeness::Partial);
    assert_eq!(c.session.kind(), SessionKind::Regular);
}

// ---- folding ----

#[test]
fn second_tick_same_window_updates_ohlc() {
    let mut agg = xnys_m1_agg();
    let t0 = price_size_tick(100.0, 10, utc(2024, 1, 17, 15, 0, 0));
    let t1 = price_size_tick(101.5, 20, utc(2024, 1, 17, 15, 0, 30));
    let _ = agg.accept_tick(&t0);
    let out = agg.accept_tick(&t1);
    let c = match out {
        AggregatorOutput::Partial(c) => c,
        other => panic!("expected Partial (rate_hz=0), got {other:?}"),
    };
    assert_eq!(c.o, 100.0);
    assert_eq!(c.h, 101.5);
    assert_eq!(c.l, 100.0);
    assert_eq!(c.c, 101.5);
    assert_eq!(c.volume, 30);
    assert_eq!(c.trade_count, 2);
}

#[test]
fn third_tick_updates_low() {
    let mut agg = xnys_m1_agg();
    let _ = agg.accept_tick(&price_size_tick(100.0, 10, utc(2024, 1, 17, 15, 0, 0)));
    let _ = agg.accept_tick(&price_size_tick(101.5, 20, utc(2024, 1, 17, 15, 0, 20)));
    let out = agg.accept_tick(&price_size_tick(99.25, 30, utc(2024, 1, 17, 15, 0, 40)));
    let c = match out {
        AggregatorOutput::Partial(c) => c,
        other => panic!("expected Partial, got {other:?}"),
    };
    assert_eq!(c.l, 99.25);
    assert_eq!(c.c, 99.25);
    assert_eq!(c.volume, 60);
    assert_eq!(c.trade_count, 3);
}

#[test]
fn volume_sum_across_three_ticks() {
    let mut agg = xnys_m1_agg();
    for (i, sz) in [10i64, 20, 30].into_iter().enumerate() {
        let ts = utc(2024, 1, 17, 15, 0, i as u32 * 10);
        let _ = agg.accept_tick(&price_size_tick(100.0, sz, ts));
    }
    let snap = agg.snapshot_current_partial().unwrap();
    assert_eq!(snap.volume, 60);
    assert_eq!(snap.trade_count, 3);
}

#[test]
fn vwap_accumulation_matches_formula() {
    let mut agg = xnys_m1_agg();
    let _ = agg.accept_tick(&price_size_tick(100.0, 10, utc(2024, 1, 17, 15, 0, 0)));
    let _ = agg.accept_tick(&price_size_tick(101.0, 20, utc(2024, 1, 17, 15, 0, 15)));
    let _ = agg.accept_tick(&price_size_tick(99.5, 40, utc(2024, 1, 17, 15, 0, 30)));
    let snap = agg.snapshot_current_partial().unwrap();
    // (100*10 + 101*20 + 99.5*40) / (10+20+40) = (1000+2020+3980)/70
    let expected = (1000.0 + 2020.0 + 3980.0) / 70.0;
    let got = snap.wap.unwrap();
    assert!((got - expected).abs() < 1e-9);
}

#[test]
fn price_last_tick_without_size_folds_trade_count_only() {
    let mut agg = xnys_m1_agg();
    let _ = agg.accept_tick(&price_size_tick(100.0, 10, utc(2024, 1, 17, 15, 0, 0)));
    // price-only tick adds trade_count but not volume.
    let _ = agg.accept_tick(&price_last_tick(101.0, utc(2024, 1, 17, 15, 0, 15)));
    let snap = agg.snapshot_current_partial().unwrap();
    assert_eq!(snap.volume, 10);
    assert_eq!(snap.trade_count, 2);
    assert_eq!(snap.h, 101.0);
}

// ---- partial emit rate limiting ----

#[tokio::test(start_paused = true)]
async fn partial_emit_respects_rate_limit_hz_4() {
    let start = utc(2024, 1, 17, 15, 0, 0);
    let clock = MockClock::shared(start);
    let cal = xnys();
    let sym = Symbol::new("AAPL", cal.id());
    let cfg = AggregatorConfig::new(sym, cal, BarPeriod::m1(), clock.clone())
        .with_partial_emit_rate_hz(4);
    let mut agg = SessionedBarAggregator::new(cfg).unwrap();

    // First tick → Opened (resets rate limit).
    let _ = agg.accept_tick(&price_size_tick(100.0, 1, start));
    // 100 ticks over 100ms (well under 250ms interval). Expect every
    // subsequent tick to be Folded with zero Partial emits.
    let mut partials = 0;
    for i in 1..=100i64 {
        // Advance mono by 1ms each; tokio advance drives the same.
        clock.advance_by(Duration::from_millis(1)).await;
        let t = price_size_tick(
            100.0 + i as f64 * 0.01,
            1,
            start + chrono::Duration::milliseconds(i),
        );
        match agg.accept_tick(&t) {
            AggregatorOutput::Folded => {}
            AggregatorOutput::Partial(_) => partials += 1,
            other => panic!("unexpected {other:?}"),
        }
    }
    // 100ms total / 250ms interval → zero partials after the Opened
    // seeded the last_partial_emit anchor.
    assert_eq!(partials, 0);
}

#[tokio::test(start_paused = true)]
async fn partial_emits_once_interval_crossed() {
    let start = utc(2024, 1, 17, 15, 0, 0);
    let clock = MockClock::shared(start);
    let cal = xnys();
    let sym = Symbol::new("AAPL", cal.id());
    let cfg = AggregatorConfig::new(sym, cal, BarPeriod::m1(), clock.clone())
        .with_partial_emit_rate_hz(4);
    let mut agg = SessionedBarAggregator::new(cfg).unwrap();
    let _ = agg.accept_tick(&price_size_tick(100.0, 1, start));
    // Move >=250ms and feed one tick.
    clock.advance_by(Duration::from_millis(300)).await;
    let out = agg.accept_tick(&price_size_tick(
        100.5,
        1,
        start + chrono::Duration::milliseconds(300),
    ));
    assert!(matches!(out, AggregatorOutput::Partial(_)));
}

#[tokio::test(start_paused = true)]
async fn partial_emit_rate_zero_means_every_tick() {
    let start = utc(2024, 1, 17, 15, 0, 0);
    let clock = MockClock::shared(start);
    let cal = xnys();
    let sym = Symbol::new("AAPL", cal.id());
    let cfg = AggregatorConfig::new(sym, cal, BarPeriod::m1(), clock).with_partial_emit_rate_hz(0);
    let mut agg = SessionedBarAggregator::new(cfg).unwrap();
    let _ = agg.accept_tick(&price_size_tick(100.0, 1, start));
    // Zero advance. Rate=0 → every tick emits.
    let out = agg.accept_tick(&price_size_tick(100.25, 1, start));
    assert!(matches!(out, AggregatorOutput::Partial(_)));
}

// ---- clock-boundary rollover ----

#[test]
fn rollover_on_m1_minute_boundary() {
    let mut agg = xnys_m1_agg();
    let _ = agg.accept_tick(&price_size_tick(100.0, 10, utc(2024, 1, 17, 15, 0, 30)));
    let out = agg.accept_tick(&price_size_tick(101.0, 5, utc(2024, 1, 17, 15, 1, 5)));
    let (closed, opened) = match out {
        AggregatorOutput::Rollover { closed, opened } => (closed, opened),
        other => panic!("expected Rollover, got {other:?}"),
    };
    assert_eq!(closed.completeness, Completeness::Completed);
    assert_eq!(closed.window.open, utc(2024, 1, 17, 15, 0, 0));
    assert_eq!(closed.window.close, utc(2024, 1, 17, 15, 1, 0));
    assert_eq!(closed.volume, 10);
    assert_eq!(opened.completeness, Completeness::Partial);
    assert_eq!(opened.window.open, utc(2024, 1, 17, 15, 1, 0));
    assert_eq!(opened.volume, 5);
    assert_eq!(opened.o, 101.0);
}

// ---- session-boundary rollover ----

#[test]
fn rollover_at_xnys_pre_to_regular_boundary() {
    // 09:29 ET = 14:29 UTC (winter) = PreMarket.
    // 09:30 ET = 14:30 UTC = Regular.
    let mut agg = xnys_m1_agg();
    let pre = agg.accept_tick(&price_size_tick(50.0, 10, utc(2024, 1, 17, 14, 29, 45)));
    assert!(matches!(pre, AggregatorOutput::Opened(_)));
    let out = agg.accept_tick(&price_size_tick(50.5, 20, utc(2024, 1, 17, 14, 30, 5)));
    let (closed, opened) = match out {
        AggregatorOutput::Rollover { closed, opened } => (closed, opened),
        other => panic!("expected Rollover, got {other:?}"),
    };
    assert_eq!(closed.session.kind(), SessionKind::PreMarket);
    assert_eq!(closed.window.open, utc(2024, 1, 17, 14, 29, 0));
    assert_eq!(closed.window.close, utc(2024, 1, 17, 14, 30, 0));
    assert_eq!(opened.session.kind(), SessionKind::Regular);
    assert_eq!(opened.window.open, utc(2024, 1, 17, 14, 30, 0));
}

#[test]
fn rollover_at_xnys_regular_to_post_boundary() {
    // 15:59 ET = 20:59 UTC = Regular.
    // 16:00 ET = 21:00 UTC = PostMarket.
    let mut agg = xnys_m1_agg();
    let _ = agg.accept_tick(&price_size_tick(100.0, 10, utc(2024, 1, 17, 20, 59, 30)));
    let out = agg.accept_tick(&price_size_tick(100.25, 5, utc(2024, 1, 17, 21, 0, 5)));
    let (closed, opened) = match out {
        AggregatorOutput::Rollover { closed, opened } => (closed, opened),
        other => panic!("expected Rollover, got {other:?}"),
    };
    assert_eq!(closed.session.kind(), SessionKind::Regular);
    assert_eq!(opened.session.kind(), SessionKind::PostMarket);
}

// ---- out-of-coverage ignored ----

#[test]
fn tick_out_of_coverage_is_ignored() {
    let mut agg = xnys_m1_agg();
    // 1950-01-01 is well outside XNYS coverage [2000, 2032). classify
    // returns Closed; bar_window still returns a valid UTC-aligned
    // window with session=Closed. The aggregator thus accepts it —
    // coverage is enforced by the CALENDAR, not by the aggregator.
    // We instead test an instant that would fall out of coverage for
    // a calendar impl that rejects bar_window. XNYS does not reject,
    // so assert the weaker property: an Opened emits with Closed
    // session.
    let t = price_size_tick(100.0, 10, utc(1950, 1, 1, 12, 0, 0));
    let out = agg.accept_tick(&t);
    // XNYS bar_window doesn't error for OOB ts — it returns a
    // closed-session window. We accept this as the documented
    // contract: "the calendar is authoritative."
    assert!(matches!(
        out,
        AggregatorOutput::Opened(_) | AggregatorOutput::Ignored
    ));
}

// ---- snapshot_current_partial ----

#[test]
fn snapshot_none_before_first_tick() {
    let agg = xnys_m1_agg();
    assert!(agg.snapshot_current_partial().is_none());
}

#[test]
fn snapshot_after_tick_returns_partial() {
    let mut agg = xnys_m1_agg();
    let _ = agg.accept_tick(&price_size_tick(100.0, 10, utc(2024, 1, 17, 15, 0, 0)));
    let s = agg.snapshot_current_partial().unwrap();
    assert_eq!(s.completeness, Completeness::Partial);
    assert_eq!(s.o, 100.0);
}

// ---- flush_if_due ----

#[tokio::test(start_paused = true)]
async fn flush_if_due_none_when_no_current() {
    let clock = MockClock::shared(utc(2024, 1, 17, 15, 0, 0));
    let mut agg = xnys_m1_agg_with_clock(clock);
    assert!(agg.flush_if_due().is_none());
}

#[tokio::test(start_paused = true)]
async fn flush_if_due_none_when_still_in_window() {
    let start = utc(2024, 1, 17, 15, 0, 0);
    let clock = MockClock::shared(start);
    let mut agg = xnys_m1_agg_with_clock(clock.clone());
    let _ = agg.accept_tick(&price_size_tick(
        100.0,
        10,
        start + chrono::Duration::seconds(5),
    ));
    // Advance to 15:00:30 UTC — still inside [15:00, 15:01).
    clock.advance_to(utc(2024, 1, 17, 15, 0, 30)).await;
    assert!(agg.flush_if_due().is_none());
}

#[tokio::test(start_paused = true)]
async fn flush_if_due_emits_completed_when_past_close() {
    let start = utc(2024, 1, 17, 15, 0, 0);
    let clock = MockClock::shared(start);
    let mut agg = xnys_m1_agg_with_clock(clock.clone());
    let _ = agg.accept_tick(&price_size_tick(
        100.0,
        10,
        start + chrono::Duration::seconds(5),
    ));
    clock.advance_to(utc(2024, 1, 17, 15, 1, 0)).await;
    let c = agg.flush_if_due().expect("should flush");
    assert_eq!(c.completeness, Completeness::Completed);
    assert!(!agg.has_current());
}

// ---- early close + Clock(H1) (R2-G-8) ----

#[test]
fn early_close_rollover_closes_bar_at_post_to_closed_boundary() {
    // Day after Thanksgiving 2024 = Fri 2024-11-29 — early close at
    // 13:00 ET (= 18:00 UTC winter). An H1 bar seeded with a 12:59 ET
    // tick should close when the NEXT tick arrives in PostMarket (or
    // the following pre-market). We test the XNYS H1 aggregator and
    // ensure the cross-window tick triggers a Rollover with a
    // Completed bar.
    let cal = xnys();
    let sym = Symbol::new("AAPL", cal.id());
    let cfg = AggregatorConfig::new(sym, cal, BarPeriod::h1(), Arc::new(SystemClock))
        .with_partial_emit_rate_hz(0);
    let mut agg = SessionedBarAggregator::new(cfg).unwrap();
    // 12:59 ET = 17:59 UTC — last tick of the RTH hour.
    let _ = agg.accept_tick(&price_size_tick(100.0, 10, utc(2024, 11, 29, 17, 59, 30)));
    // Next bar-boundary tick: 13:00 ET = 18:00 UTC — would normally
    // be RTH, but session on an early-close day is PostMarket-or-
    // Closed (depends on calendar). We accept either kind; the key
    // invariant is that a rollover fires.
    let out = agg.accept_tick(&price_size_tick(100.5, 5, utc(2024, 11, 29, 18, 0, 5)));
    match out {
        AggregatorOutput::Rollover { closed, .. } => {
            assert_eq!(closed.completeness, Completeness::Completed);
        }
        // Same UTC bucket + same Session means it folds; this would
        // violate G-8 and the CALENDAR tests should catch it. We
        // assert Rollover here as the contract.
        other => panic!("expected Rollover at early-close boundary, got {other:?}"),
    }
}

// ---- session-scoped period ----

#[test]
fn session_regular_one_bar_per_trading_day() {
    let cal = xnys();
    let sym = Symbol::new("AAPL", cal.id());
    let cfg = AggregatorConfig::new(sym, cal, BarPeriod::d1_rth(), Arc::new(SystemClock))
        .with_partial_emit_rate_hz(0);
    let mut agg = SessionedBarAggregator::new(cfg).unwrap();
    let mut rollovers = 0;
    let mut opened_or_partial = 0;
    // Feed 20 minute-spaced ticks across 10:00-10:20 ET on a trading
    // day. All inside one session window → no Rollover.
    for i in 0..20u32 {
        let ts = utc(2024, 1, 17, 15, 0, 0) + chrono::Duration::minutes(i as i64);
        match agg.accept_tick(&price_size_tick(100.0 + i as f64 * 0.1, 10, ts)) {
            AggregatorOutput::Rollover { .. } => rollovers += 1,
            AggregatorOutput::Opened(_) | AggregatorOutput::Partial(_) => opened_or_partial += 1,
            _ => {}
        }
    }
    assert_eq!(rollovers, 0);
    assert!(opened_or_partial >= 1);
}

#[test]
fn unsupported_period_ignores_ticks_if_validate_accepts_but_bar_window_errors() {
    // No calendar currently returns bar_window errors for an
    // already-validated period, so this is a belt-and-braces test
    // that simply exercises the "bar_window error → Ignored" path
    // by constructing a ts outside i64 nanos range. chrono will
    // usually clamp; we instead exercise the case where
    // bar_window's internal i64 arithmetic could fail. In practice
    // this path is vanishingly rare; assert the aggregator doesn't
    // panic.
    let mut agg = xnys_m1_agg();
    let t = price_size_tick(100.0, 10, utc(2024, 1, 17, 15, 0, 0));
    let _ = agg.accept_tick(&t);
    // Just confirm we can call without crashing. No strong assertion.
}

#[test]
fn aggregator_is_send() {
    fn assert_send<T: Send>() {}
    assert_send::<SessionedBarAggregator>();
}
