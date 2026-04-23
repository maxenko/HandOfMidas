//! `Bar` → `Candle` conversion.
//!
//! Session is supplied by the calendar at the provided `ts_open`. The
//! resulting `Candle` carries the pair-reconciled calendar/session
//! identity enforced by `Candle::new`.
//!
//! `period` is passed in by the caller because the legacy
//! `Bar::timeframe` → `BarPeriod` mapping is potentially ambiguous
//! (the same `Timeframe::D1` can mean `Session(Regular)` or —
//! historically — a wall-clock rollup on crypto). Adapter callers pin
//! the period explicitly when building a stream, so we honour that
//! choice rather than re-deriving it here.

use midas_bars::{Candle, Completeness, Ohlcv, Symbol};
use midas_broker_core::market_data::{Bar, BarCompleteness};
use midas_calendar::{BarPeriod, BarWindow, ExchangeCalendar};

/// Convert a provider [`Bar`] into a session-aware [`Candle`].
///
/// Panics only under two conditions that indicate misuse rather than
/// runtime data:
/// 1. The OHLC fields violate `Candle::new`'s invariants. Providers are
///    expected to emit consistent bars (`l <= o, c <= h`). A genuinely
///    malformed upstream bar is not expected on the hot path — guard at
///    the provider layer if that assumption weakens.
/// 2. The `symbol.calendar() != calendar.id()` — a caller bug.
///
/// Use `historical_bars_to_candles` to convert an entire bulk payload;
/// it applies this per-bar.
pub fn bar_to_candle(
    bar: &Bar,
    symbol: Symbol,
    calendar: &'static dyn ExchangeCalendar,
    period: BarPeriod,
) -> Candle {
    let session = calendar.classify(bar.ts_open);
    let window = BarWindow {
        open: bar.ts_open,
        close: bar.ts_close,
        session: session.clone(),
    };

    let completeness = match bar.completeness {
        BarCompleteness::Completed => Completeness::Completed,
        BarCompleteness::Partial => Completeness::Partial,
    };

    // Build the OHLCV via the smart constructor. Provider-side bars are
    // expected to satisfy OHLC ordering already; if they don't, we
    // surface a panic here rather than silently fabricating one. The
    // integration test harness exercises the happy path; bugs in the
    // sim/IB layer should fail loudly at the adapter seam.
    let ohlcv = Ohlcv::new(
        bar.o,
        bar.h,
        bar.l,
        bar.c,
        bar.volume,
        bar.trade_count,
        bar.wap,
    )
    .expect("bar OHLCV violates invariants");

    Candle::new(
        symbol,
        calendar,
        period,
        session,
        window,
        ohlcv,
        completeness,
    )
    .expect("candle invariants should hold given calendar.classify supplied session")
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use midas_broker_core::{SymbolKey, Timeframe};
    use midas_calendar::{crypto_spot, xnys, BarPeriod, SessionKind};

    use super::*;

    fn utc(y: i32, m: u32, d: u32, h: u32, min: u32) -> chrono::DateTime<chrono::Utc> {
        chrono::Utc.with_ymd_and_hms(y, m, d, h, min, 0).unwrap()
    }

    fn sample_aapl_bar(ts_open: chrono::DateTime<chrono::Utc>) -> Bar {
        Bar {
            symbol: SymbolKey {
                contract_id: 265598,
                symbol: "AAPL".into(),
            },
            timeframe: Timeframe::M1,
            ts_open,
            ts_close: ts_open + chrono::Duration::minutes(1),
            o: 100.0,
            h: 101.25,
            l: 99.75,
            c: 100.5,
            volume: 12_345,
            trade_count: 42,
            wap: Some(100.37),
            completeness: BarCompleteness::Completed,
        }
    }

    fn sample_btc_bar(ts_open: chrono::DateTime<chrono::Utc>) -> Bar {
        Bar {
            symbol: SymbolKey {
                contract_id: 1_000_000_001,
                symbol: "BTC-USD".into(),
            },
            timeframe: Timeframe::M1,
            ts_open,
            ts_close: ts_open + chrono::Duration::minutes(1),
            o: 50_000.0,
            h: 50_100.0,
            l: 49_950.0,
            c: 50_050.0,
            volume: 1_500,
            trade_count: 20,
            wap: None,
            completeness: BarCompleteness::Completed,
        }
    }

    #[test]
    fn aapl_m1_bar_in_rth_classifies_regular() {
        let cal = xnys();
        let sym = Symbol::new("AAPL", cal.id());
        // 2024-01-17 15:00 UTC = 10:00 ET — comfortably inside RTH.
        let bar = sample_aapl_bar(utc(2024, 1, 17, 15, 0));
        let c = bar_to_candle(&bar, sym, cal, BarPeriod::m1());
        assert_eq!(c.session.kind(), SessionKind::Regular);
        assert_eq!(c.calendar, cal.id());
        assert_eq!(c.period, BarPeriod::m1());
        assert_eq!(c.o, 100.0);
        assert_eq!(c.h, 101.25);
        assert_eq!(c.l, 99.75);
        assert_eq!(c.c, 100.5);
        assert_eq!(c.volume, 12_345);
        assert_eq!(c.trade_count, 42);
        assert_eq!(c.wap, Some(100.37));
        assert_eq!(c.completeness, Completeness::Completed);
    }

    #[test]
    fn aapl_m1_bar_at_08et_classifies_premarket() {
        let cal = xnys();
        let sym = Symbol::new("AAPL", cal.id());
        // 2024-01-17 13:00 UTC = 08:00 ET — inside XNYS pre-market.
        let bar = sample_aapl_bar(utc(2024, 1, 17, 13, 0));
        let c = bar_to_candle(&bar, sym, cal, BarPeriod::m1());
        assert_eq!(c.session.kind(), SessionKind::PreMarket);
    }

    #[test]
    fn btc_m1_bar_is_regular_all_day() {
        let cal = crypto_spot();
        let sym = Symbol::new("BTC-USD", cal.id());
        // Pick a random UTC hour; crypto is always Regular in coverage.
        let bar = sample_btc_bar(utc(2024, 8, 17, 3, 14));
        let c = bar_to_candle(&bar, sym, cal, BarPeriod::m1());
        assert_eq!(c.session.kind(), SessionKind::Regular);
        assert_eq!(c.calendar, cal.id());
    }

    #[test]
    fn completeness_roundtrips() {
        let cal = xnys();
        let sym = Symbol::new("AAPL", cal.id());

        let mut completed = sample_aapl_bar(utc(2024, 1, 17, 15, 0));
        completed.completeness = BarCompleteness::Completed;
        let c = bar_to_candle(&completed, sym, cal, BarPeriod::m1());
        assert_eq!(c.completeness, Completeness::Completed);

        let mut partial = sample_aapl_bar(utc(2024, 1, 17, 15, 0));
        partial.completeness = BarCompleteness::Partial;
        let c = bar_to_candle(&partial, sym, cal, BarPeriod::m1());
        assert_eq!(c.completeness, Completeness::Partial);
    }

    #[test]
    fn wap_none_and_some_roundtrip() {
        let cal = xnys();
        let sym = Symbol::new("AAPL", cal.id());

        let mut with_wap = sample_aapl_bar(utc(2024, 1, 17, 15, 0));
        with_wap.wap = Some(100.30);
        let c = bar_to_candle(&with_wap, sym, cal, BarPeriod::m1());
        assert_eq!(c.wap, Some(100.30));

        let mut no_wap = sample_aapl_bar(utc(2024, 1, 17, 15, 0));
        no_wap.wap = None;
        let c = bar_to_candle(&no_wap, sym, cal, BarPeriod::m1());
        assert_eq!(c.wap, None);
    }

    #[test]
    fn window_open_close_preserved() {
        let cal = xnys();
        let sym = Symbol::new("AAPL", cal.id());
        let open = utc(2024, 1, 17, 15, 0);
        let bar = sample_aapl_bar(open);
        let c = bar_to_candle(&bar, sym, cal, BarPeriod::m1());
        assert_eq!(c.window.open, open);
        assert_eq!(c.window.close, open + chrono::Duration::minutes(1));
    }
}
