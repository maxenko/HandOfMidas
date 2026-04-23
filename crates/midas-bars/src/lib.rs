//! # midas-bars
//!
//! Session-aware candle types and SoA storage for the
//! session-aware-charts stack. Slice S2 of
//! `plan/session-aware-charts/00-index.md`.
//!
//! ## Public surface
//!
//! - [`Symbol`] — bar-level identity: `(ticker, CalendarId)` pair, `Copy`.
//! - [`Candle`] — the WIRE/API type producers emit and consumers receive.
//!   Session and window are mandatory; smart constructor reconciles the
//!   three redundant paths to `CalendarId` (R2-NM-3).
//! - [`Ohlcv`] — plain OHLCV payload with its own smart constructor.
//! - [`Completeness`] — `Completed | Partial`.
//! - [`CandleSeries`] — SoA storage: one series per
//!   `(symbol, calendar, period)` tuple. Replaces the legacy
//!   `CandleBuffer`. Stores the MINIMUM; [`CandleRef`] reconstructs the
//!   full `Session` lazily via `ExchangeCalendar::classify`.
//! - [`CandleRef`] — read-side view of a series row.
//!
//! All calendar types — [`Timestamp`], [`CalendarId`], [`Session`],
//! [`SessionKind`], [`BarWindow`], [`TimeAxisPolicy`], [`BarPeriod`],
//! [`ClockInterval`], [`SessionSpan`], [`CalendarSpan`], [`TradingDay`],
//! [`CalendarError`], and the [`ExchangeCalendar`] trait — are
//! re-exported from `midas-calendar`. This crate intentionally adds no
//! new calendar types.
//!
//! ## Example
//!
//! ```
//! use midas_bars::{Candle, CandleSeries, Completeness, Ohlcv, Symbol};
//! use midas_calendar::{xnys, BarPeriod, CalendarId};
//! use chrono::TimeZone;
//!
//! let cal = xnys();
//! let sym = Symbol::new("SPY", cal.id());
//! let mut series = CandleSeries::new(cal.id(), BarPeriod::m1(), sym);
//!
//! // 14:30 UTC = 09:30 ET = Regular session open.
//! let ts = chrono::Utc.with_ymd_and_hms(2024, 1, 17, 14, 30, 0).unwrap();
//! let session = cal.classify(ts);
//! let window = cal.bar_window(ts, BarPeriod::m1()).unwrap();
//! let ohlcv = Ohlcv::new(100.0, 101.0, 99.5, 100.5, 5_000, 50, None).unwrap();
//!
//! let c = Candle::new(
//!     sym, cal, BarPeriod::m1(), session, window, ohlcv,
//!     Completeness::Completed,
//! ).unwrap();
//! series.push(c);
//!
//! let row = series.at(0).unwrap();
//! assert_eq!(row.session_kind(), midas_calendar::SessionKind::Regular);
//! ```

mod candle;
mod series;
mod symbol;

pub use crate::candle::{Candle, CandleError, Completeness, Ohlcv, SessionKindByte};
pub use crate::series::{CandleRef, CandleSeries};
pub use crate::symbol::Symbol;

// Re-exports from midas-calendar. S2 explicitly does NOT redefine any
// calendar-layer type — the re-exports let downstream slices take a
// single dependency on `midas-bars` for the full session-aware surface
// without juggling two crate roots.
pub use midas_calendar::{
    xnys, BarPeriod, BarWindow, CalendarError, CalendarId, CalendarSpan, ClockInterval,
    ExchangeCalendar, Session, SessionKind, SessionSpan, TimeAxisPolicy, Timestamp, TradingDay,
};
