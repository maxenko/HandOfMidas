//! `HistoricalBarsResult` → `Vec<Candle>` bulk conversion.
//!
//! Historical bulk fetches are always `Completeness::Completed` by
//! definition — IB's `historical_data` only returns closed bars (the
//! live tail comes through `historical_stream` / realtime). We force
//! `Completeness::Completed` here regardless of what upstream marked,
//! mirroring IB's semantics and keeping the `HistoryThenLive` seam
//! dedup deterministic.

use midas_bars::{Candle, Completeness, Ohlcv, Symbol};
use midas_broker_core::provider::HistoricalBarsResult;
use midas_calendar::{BarPeriod, BarWindow, ExchangeCalendar};

/// Convert a bulk [`HistoricalBarsResult`] payload into a
/// `Vec<Candle>`. All candles are tagged `Completeness::Completed`.
pub fn historical_bars_to_candles(
    result: &HistoricalBarsResult,
    symbol: Symbol,
    calendar: &'static dyn ExchangeCalendar,
    period: BarPeriod,
) -> Vec<Candle> {
    let mut out = Vec::with_capacity(result.bars.len());
    for bar in &result.bars {
        let session = calendar.classify(bar.ts_open);
        let window = BarWindow {
            open: bar.ts_open,
            close: bar.ts_close,
            session: session.clone(),
        };
        let ohlcv = Ohlcv::new(
            bar.o,
            bar.h,
            bar.l,
            bar.c,
            bar.volume,
            bar.trade_count,
            bar.wap,
        )
        .expect("historical bar OHLCV violates invariants");
        let candle = Candle::new(
            symbol,
            calendar,
            period,
            session,
            window,
            ohlcv,
            Completeness::Completed,
        )
        .expect("historical candle invariants should hold");
        out.push(candle);
    }
    out
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use midas_broker_core::market_data::{Bar, BarCompleteness, SymbolKey, Timeframe};
    use midas_calendar::{xnys, BarPeriod, SessionKind};

    use super::*;

    fn utc(y: i32, m: u32, d: u32, h: u32, min: u32) -> chrono::DateTime<chrono::Utc> {
        chrono::Utc.with_ymd_and_hms(y, m, d, h, min, 0).unwrap()
    }

    fn rth_bar(minute_offset: u32) -> Bar {
        let open = utc(2024, 1, 17, 15, 0) + chrono::Duration::minutes(minute_offset as i64);
        Bar {
            symbol: SymbolKey {
                contract_id: 265598,
                symbol: "AAPL".into(),
            },
            timeframe: Timeframe::M1,
            ts_open: open,
            ts_close: open + chrono::Duration::minutes(1),
            o: 100.0 + f64::from(minute_offset) * 0.1,
            h: 101.0 + f64::from(minute_offset) * 0.1,
            l: 99.5 + f64::from(minute_offset) * 0.1,
            c: 100.5 + f64::from(minute_offset) * 0.1,
            volume: 100 + u64::from(minute_offset),
            trade_count: 1,
            wap: None,
            completeness: BarCompleteness::Partial, // overwritten to Completed
        }
    }

    #[test]
    fn ten_bars_all_completed() {
        let cal = xnys();
        let sym = Symbol::new("AAPL", cal.id());
        let bars: Vec<Bar> = (0..10).map(rth_bar).collect();
        let result = HistoricalBarsResult {
            first_ts: bars.first().unwrap().ts_open,
            last_ts: bars.last().unwrap().ts_open,
            bars,
        };
        let candles = historical_bars_to_candles(&result, sym, cal, BarPeriod::m1());
        assert_eq!(candles.len(), 10);
        for c in &candles {
            assert_eq!(c.completeness, Completeness::Completed);
            assert_eq!(c.calendar, cal.id());
            assert_eq!(c.session.kind(), SessionKind::Regular);
        }
    }

    #[test]
    fn empty_input_produces_empty_output() {
        let cal = xnys();
        let sym = Symbol::new("AAPL", cal.id());
        let result = HistoricalBarsResult {
            bars: vec![],
            first_ts: utc(2024, 1, 17, 15, 0),
            last_ts: utc(2024, 1, 17, 15, 0),
        };
        let candles = historical_bars_to_candles(&result, sym, cal, BarPeriod::m1());
        assert!(candles.is_empty());
    }

    #[test]
    fn preserves_order() {
        let cal = xnys();
        let sym = Symbol::new("AAPL", cal.id());
        let bars: Vec<Bar> = (0..5).map(rth_bar).collect();
        let result = HistoricalBarsResult {
            first_ts: bars.first().unwrap().ts_open,
            last_ts: bars.last().unwrap().ts_open,
            bars,
        };
        let candles = historical_bars_to_candles(&result, sym, cal, BarPeriod::m1());
        for w in candles.windows(2) {
            assert!(w[1].window.open > w[0].window.open);
        }
    }
}
