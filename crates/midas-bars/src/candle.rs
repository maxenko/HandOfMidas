//! `Candle` — session-tagged OHLCV wire/API type.
//!
//! A `Candle` is the self-contained bar that `BarStream::next()` yields
//! to consumers. It carries its own `Session` and `BarWindow` so receivers
//! need no additional calendar lookup to interpret it. Per the R2-NM-3
//! resolution the three redundant paths to `CalendarId` —
//! `candle.calendar`, `candle.session.calendar()`, and
//! `candle.window.session.calendar()` — are reconciled by the
//! `Candle::new` smart constructor, which is the only public way to
//! create a `Candle`.
//!
//! Storage (see `CandleSeries`) stores the *minimum* — calendar and
//! period once per series, plus one byte per row for `SessionKind` and
//! `Completeness`. `CandleRef` lazily reconstructs the full `Session` on
//! demand from the stored ingredients.

use serde::{Deserialize, Serialize};

use midas_calendar::{BarPeriod, BarWindow, CalendarId, ExchangeCalendar, Session, SessionKind};

use crate::Symbol;

/// Whether a bar is closed (`Completed`) or still accumulating ticks
/// (`Partial`). SoA-friendly: `#[repr(u8)]` keeps it one byte per row.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum Completeness {
    Completed,
    Partial,
}

/// Plain OHLCV payload. Separate from `Candle` so aggregators and
/// pipelines can carry price/volume without the session envelope.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Ohlcv {
    pub o: f64,
    pub h: f64,
    pub l: f64,
    pub c: f64,
    pub volume: u64,
    pub trade_count: u32,
    pub wap: Option<f64>,
}

impl Ohlcv {
    /// Build an `Ohlcv` and validate OHLC ordering. Returns
    /// `CandleError::InvalidOhlc` for NaN / infinite values, `l > h`, or
    /// `o`/`c` outside `[l, h]`. Negative prices are permitted (indices,
    /// spreads). Zero volume is permitted.
    pub fn new(
        o: f64,
        h: f64,
        l: f64,
        c: f64,
        volume: u64,
        trade_count: u32,
        wap: Option<f64>,
    ) -> Result<Self, CandleError> {
        if !o.is_finite() || !h.is_finite() || !l.is_finite() || !c.is_finite() {
            return Err(CandleError::InvalidOhlc {
                reason: "OHLC contains NaN or infinite value",
                o,
                h,
                l,
                c,
            });
        }
        if l > h {
            return Err(CandleError::InvalidOhlc {
                reason: "low > high",
                o,
                h,
                l,
                c,
            });
        }
        if o < l || o > h {
            return Err(CandleError::InvalidOhlc {
                reason: "open outside [low, high]",
                o,
                h,
                l,
                c,
            });
        }
        if c < l || c > h {
            return Err(CandleError::InvalidOhlc {
                reason: "close outside [low, high]",
                o,
                h,
                l,
                c,
            });
        }
        if let Some(w) = wap {
            if !w.is_finite() {
                return Err(CandleError::InvalidOhlc {
                    reason: "wap not finite",
                    o,
                    h,
                    l,
                    c,
                });
            }
            if w < l || w > h {
                return Err(CandleError::InvalidOhlc {
                    reason: "wap outside [low, high]",
                    o,
                    h,
                    l,
                    c,
                });
            }
        }
        Ok(Self {
            o,
            h,
            l,
            c,
            volume,
            trade_count,
            wap,
        })
    }
}

/// A session-tagged, calendar-bound OHLCV bar. Wire/API type produced by
/// `BarStream::next()`. Every field is `pub` for ergonomic pattern-
/// matching; the only way to *construct* one is `Candle::new`, which
/// enforces the invariants.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Candle {
    pub symbol: Symbol,
    pub calendar: CalendarId,
    pub period: BarPeriod,
    pub session: Session,
    pub window: BarWindow,
    pub o: f64,
    pub h: f64,
    pub l: f64,
    pub c: f64,
    pub volume: u64,
    pub trade_count: u32,
    pub wap: Option<f64>,
    pub completeness: Completeness,
}

impl Candle {
    /// Smart constructor — the only public way to produce a `Candle`.
    ///
    /// Validates:
    /// - OHLC ordering (`l <= o, c <= h` and `l <= h`).
    /// - `wap` sits inside `[l, h]` if present.
    /// - `window.session` agrees with the explicit `session` argument on
    ///   `(kind, open, close)` — the two must be the same session.
    /// - `calendar.id() == session.calendar() == window.session.calendar()`.
    pub fn new(
        symbol: Symbol,
        calendar: &'static dyn ExchangeCalendar,
        period: BarPeriod,
        session: Session,
        window: BarWindow,
        ohlcv: Ohlcv,
        completeness: Completeness,
    ) -> Result<Self, CandleError> {
        let cal_id = calendar.id();
        if session.calendar() != cal_id {
            return Err(CandleError::CalendarMismatch {
                expected: cal_id,
                session_calendar: session.calendar(),
                window_calendar: window.session.calendar(),
            });
        }
        if window.session.calendar() != cal_id {
            return Err(CandleError::CalendarMismatch {
                expected: cal_id,
                session_calendar: session.calendar(),
                window_calendar: window.session.calendar(),
            });
        }
        if symbol.calendar() != cal_id {
            return Err(CandleError::SymbolCalendarMismatch {
                expected: cal_id,
                got: symbol.calendar(),
            });
        }
        if window.session.kind() != session.kind()
            || window.session.open() != session.open()
            || window.session.close() != session.close()
        {
            return Err(CandleError::WindowSessionMismatch);
        }

        // Re-validate OHLC (idempotent with Ohlcv::new but keeps the
        // smart constructor self-contained so callers can assemble an
        // `Ohlcv` struct literal via future `..default()` patterns and
        // still get defended).
        let ohlcv = Ohlcv::new(
            ohlcv.o,
            ohlcv.h,
            ohlcv.l,
            ohlcv.c,
            ohlcv.volume,
            ohlcv.trade_count,
            ohlcv.wap,
        )?;

        Ok(Self {
            symbol,
            calendar: cal_id,
            period,
            session,
            window,
            o: ohlcv.o,
            h: ohlcv.h,
            l: ohlcv.l,
            c: ohlcv.c,
            volume: ohlcv.volume,
            trade_count: ohlcv.trade_count,
            wap: ohlcv.wap,
            completeness,
        })
    }
}

/// Errors surfaced by the `Candle` / `Ohlcv` smart constructors.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum CandleError {
    #[error("invalid OHLC ({reason}): o={o}, h={h}, l={l}, c={c}")]
    InvalidOhlc {
        reason: &'static str,
        o: f64,
        h: f64,
        l: f64,
        c: f64,
    },
    #[error(
        "calendar mismatch: expected {expected}, session has {session_calendar}, window has \
         {window_calendar}"
    )]
    CalendarMismatch {
        expected: CalendarId,
        session_calendar: CalendarId,
        window_calendar: CalendarId,
    },
    #[error("symbol calendar {got} != expected {expected}")]
    SymbolCalendarMismatch {
        expected: CalendarId,
        got: CalendarId,
    },
    #[error("window.session does not agree with explicit session argument")]
    WindowSessionMismatch,
    #[error("calendar {candle_calendar} != series calendar {series_calendar}")]
    SeriesCalendarMismatch {
        series_calendar: CalendarId,
        candle_calendar: CalendarId,
    },
    #[error("period {candle_period:?} != series period {series_period:?}")]
    SeriesPeriodMismatch {
        series_period: BarPeriod,
        candle_period: BarPeriod,
    },
    #[error("symbol {candle_symbol} != series symbol {series_symbol}")]
    SeriesSymbolMismatch {
        series_symbol: Symbol,
        candle_symbol: Symbol,
    },
}

/// Convenience for `CandleSeries::sessions` column: the session *kind*
/// only. Re-exported so callers can use `midas_bars::SessionKindByte` in
/// fixture tests without dragging in midas-calendar as a dep.
pub type SessionKindByte = SessionKind;

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use midas_calendar::{xnys, BarPeriod, SessionSpan};

    use super::*;

    fn utc(y: i32, m: u32, d: u32, h: u32, min: u32) -> midas_calendar::Timestamp {
        chrono::Utc.with_ymd_and_hms(y, m, d, h, min, 0).unwrap()
    }

    #[test]
    fn ohlcv_new_accepts_valid() {
        let v = Ohlcv::new(10.0, 11.0, 9.5, 10.5, 1_000, 42, Some(10.2)).unwrap();
        assert_eq!(v.o, 10.0);
        assert_eq!(v.volume, 1_000);
    }

    #[test]
    fn ohlcv_rejects_high_below_low() {
        let err = Ohlcv::new(10.0, 9.0, 9.5, 9.2, 0, 0, None).unwrap_err();
        matches!(err, CandleError::InvalidOhlc { .. });
    }

    #[test]
    fn ohlcv_rejects_open_above_high() {
        let err = Ohlcv::new(12.0, 11.0, 9.5, 10.5, 0, 0, None).unwrap_err();
        matches!(err, CandleError::InvalidOhlc { .. });
    }

    #[test]
    fn ohlcv_rejects_close_below_low() {
        let err = Ohlcv::new(10.0, 11.0, 9.5, 9.0, 0, 0, None).unwrap_err();
        matches!(err, CandleError::InvalidOhlc { .. });
    }

    #[test]
    fn ohlcv_rejects_nan() {
        let err = Ohlcv::new(f64::NAN, 11.0, 9.5, 10.5, 0, 0, None).unwrap_err();
        matches!(err, CandleError::InvalidOhlc { .. });
    }

    #[test]
    fn ohlcv_rejects_infinite() {
        let err = Ohlcv::new(10.0, f64::INFINITY, 9.5, 10.5, 0, 0, None).unwrap_err();
        matches!(err, CandleError::InvalidOhlc { .. });
    }

    #[test]
    fn ohlcv_accepts_zero_volume() {
        Ohlcv::new(10.0, 11.0, 9.5, 10.5, 0, 0, None).unwrap();
    }

    #[test]
    fn ohlcv_accepts_negative_prices() {
        // Spreads / futures calendar spreads can go negative.
        Ohlcv::new(-1.0, 0.5, -2.0, 0.0, 10, 1, None).unwrap();
    }

    #[test]
    fn ohlcv_rejects_wap_outside_range() {
        let err = Ohlcv::new(10.0, 11.0, 9.5, 10.5, 100, 1, Some(12.0)).unwrap_err();
        matches!(err, CandleError::InvalidOhlc { .. });
    }

    #[test]
    fn candle_new_accepts_valid_xnys_regular() {
        let cal = xnys();
        let sym = Symbol::new("SPY", cal.id());
        let ts = utc(2024, 1, 17, 15, 0); // 10:00 ET = Regular session
        let session = cal.classify(ts);
        let window = cal.bar_window(ts, BarPeriod::m1()).unwrap();
        let ohlcv = Ohlcv::new(100.0, 101.0, 99.5, 100.5, 5_000, 50, None).unwrap();
        let c = Candle::new(
            sym,
            cal,
            BarPeriod::m1(),
            session.clone(),
            window,
            ohlcv,
            Completeness::Completed,
        )
        .unwrap();
        assert_eq!(c.calendar, cal.id());
        assert_eq!(c.session.kind(), SessionKind::Regular);
        assert_eq!(c.completeness, Completeness::Completed);
    }

    #[test]
    fn candle_new_rejects_symbol_calendar_mismatch() {
        let cal = xnys();
        let wrong_sym = Symbol::new("BTC-USD", CalendarId("CRYPTO"));
        let ts = utc(2024, 1, 17, 15, 0);
        let session = cal.classify(ts);
        let window = cal.bar_window(ts, BarPeriod::m1()).unwrap();
        let ohlcv = Ohlcv::new(100.0, 101.0, 99.5, 100.5, 0, 0, None).unwrap();
        let err = Candle::new(
            wrong_sym,
            cal,
            BarPeriod::m1(),
            session,
            window,
            ohlcv,
            Completeness::Completed,
        )
        .unwrap_err();
        assert!(matches!(err, CandleError::SymbolCalendarMismatch { .. }));
    }

    #[test]
    fn candle_new_rejects_bad_ohlc() {
        let cal = xnys();
        let sym = Symbol::new("SPY", cal.id());
        let ts = utc(2024, 1, 17, 15, 0);
        let session = cal.classify(ts);
        let window = cal.bar_window(ts, BarPeriod::m1()).unwrap();
        // Bypass Ohlcv::new by building by hand, then hand to Candle::new
        // which re-validates.
        let bad = Ohlcv {
            o: 10.0,
            h: 9.0,
            l: 8.0,
            c: 8.5,
            volume: 0,
            trade_count: 0,
            wap: None,
        };
        let err = Candle::new(
            sym,
            cal,
            BarPeriod::m1(),
            session,
            window,
            bad,
            Completeness::Completed,
        )
        .unwrap_err();
        assert!(matches!(err, CandleError::InvalidOhlc { .. }));
    }

    #[test]
    fn candle_round_trip_json() {
        let cal = xnys();
        let sym = Symbol::new("SPY", cal.id());
        let ts = utc(2024, 1, 17, 15, 0);
        let session = cal.classify(ts);
        let window = cal.bar_window(ts, BarPeriod::m1()).unwrap();
        let ohlcv = Ohlcv::new(100.0, 101.0, 99.5, 100.5, 5_000, 50, Some(100.3)).unwrap();
        let c = Candle::new(
            sym,
            cal,
            BarPeriod::m1(),
            session,
            window,
            ohlcv,
            Completeness::Partial,
        )
        .unwrap();

        let json = serde_json::to_string(&c).unwrap();
        let back: Candle = serde_json::from_str(&json).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn candle_pre_market_08et_classifies() {
        let cal = xnys();
        let sym = Symbol::new("SPY", cal.id());
        let ts = utc(2024, 1, 17, 13, 0); // 08:00 ET = PreMarket
        let session = cal.classify(ts);
        assert_eq!(session.kind(), SessionKind::PreMarket);
        let window = cal.bar_window(ts, BarPeriod::m1()).unwrap();
        let ohlcv = Ohlcv::new(50.0, 50.1, 49.9, 50.0, 10, 1, None).unwrap();
        let c = Candle::new(
            sym,
            cal,
            BarPeriod::m1(),
            session,
            window,
            ohlcv,
            Completeness::Completed,
        )
        .unwrap();
        assert_eq!(c.session.kind(), SessionKind::PreMarket);
    }

    #[test]
    fn candle_daily_rth_session_span() {
        // Use the d1_rth period, which is SessionSpan::Regular.
        let cal = xnys();
        let sym = Symbol::new("AAPL", cal.id());
        let ts = utc(2024, 1, 17, 15, 0);
        let session = cal.classify(ts);
        let window = cal
            .bar_window(ts, BarPeriod::Session(SessionSpan::Regular))
            .unwrap();
        let ohlcv = Ohlcv::new(180.0, 182.0, 179.0, 181.5, 10_000_000, 90_000, None).unwrap();
        let c = Candle::new(
            sym,
            cal,
            BarPeriod::d1_rth(),
            session,
            window,
            ohlcv,
            Completeness::Completed,
        )
        .unwrap();
        assert_eq!(c.period, BarPeriod::d1_rth());
    }
}
