//! # midas-calendar
//!
//! Exchange calendars for the session-aware chart stack.
//!
//! The crate defines the `ExchangeCalendar` trait plus two concrete
//! `LazyLock` singletons:
//!
//! - [`XNYS`] — NYSE/NASDAQ equities (coverage 2000..2032).
//! - [`CRYPTO_SPOT`] — 24/7 UTC spot-crypto.
//!
//! Consumers take `&'static dyn ExchangeCalendar` (via [`xnys`] or
//! [`crypto_spot`]) — calendars are process-global, so `Arc<dyn>` is
//! explicitly rejected by the design.
//!
//! ## Invariants
//!
//! - `classify` is **infallible and saturating**. Out-of-range timestamps
//!   return `SessionKind::Closed`, never an error.
//! - `covers()` is half-open `[start, end)`.
//! - `sessions_between` writes into a caller-owned `SmallVec` for
//!   allocation-free render hot-paths.
//! - The `(calendar, period)` compatibility matrix is validated at
//!   `Chart::new` time via `validate_period`; the hot path trusts the
//!   validation.
//!
//! ## Example
//!
//! ```
//! use chrono::TimeZone;
//! use midas_calendar::{xnys, BarPeriod, SessionKind};
//!
//! let cal = xnys();
//! // 2024-01-17 09:30 ET = 14:30 UTC (EST).
//! let ts = chrono::Utc.with_ymd_and_hms(2024, 1, 17, 14, 30, 0).unwrap();
//! let session = cal.classify(ts);
//! assert_eq!(session.kind(), SessionKind::Regular);
//!
//! let bw = cal.bar_window(ts, BarPeriod::d1_rth()).unwrap();
//! assert_eq!(bw.session.kind(), SessionKind::Regular);
//! ```

mod crypto_spot;
mod exchange;
mod period;
mod types;
mod xnys;

pub use crate::crypto_spot::{CryptoSpotCalendar, CRYPTO_SPOT, CRYPTO_SPOT_ID};
pub use crate::exchange::ExchangeCalendar;
pub use crate::period::{BarPeriod, CalendarSpan, ClockInterval, SessionSpan};
pub use crate::types::{
    BarWindow, CalendarError, CalendarId, Session, SessionBuf, SessionKind, TimeAxisPolicy,
    Timestamp, TradingDay,
};
pub use crate::xnys::{XnysCalendar, XNYS, XNYS_ID};

/// Process-global XNYS calendar as `&'static dyn ExchangeCalendar`.
/// Prefer this over `&*XNYS` at call sites for readability.
pub fn xnys() -> &'static dyn ExchangeCalendar {
    &*XNYS
}

/// Process-global CryptoSpot calendar as `&'static dyn ExchangeCalendar`.
pub fn crypto_spot() -> &'static dyn ExchangeCalendar {
    &*CRYPTO_SPOT
}

// ---------------------------------------------------------------------------
// Crate-level smoke tests (crypto side)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod crypto_tests {
    use chrono::TimeZone;

    use super::*;

    #[test]
    fn crypto_classify_is_always_regular_in_coverage() {
        let cal = crypto_spot();
        let ts = chrono::Utc
            .with_ymd_and_hms(2024, 8, 17, 3, 14, 15)
            .unwrap();
        let s = cal.classify(ts);
        assert_eq!(s.kind(), SessionKind::Regular);
        assert_eq!(s.calendar(), CRYPTO_SPOT_ID);
    }

    #[test]
    fn crypto_time_axis_is_continuous() {
        assert_eq!(crypto_spot().time_axis_policy(), TimeAxisPolicy::Continuous);
    }

    #[test]
    fn crypto_validate_period_rejects_eth() {
        let p = BarPeriod::Session(SessionSpan::Eth);
        let err = crypto_spot().validate_period(p).unwrap_err();
        match err {
            CalendarError::UnsupportedPeriod { calendar, period } => {
                assert_eq!(calendar, CRYPTO_SPOT_ID);
                assert_eq!(period, p);
            }
            other => panic!("expected UnsupportedPeriod, got {other:?}"),
        }
    }

    #[test]
    fn crypto_bar_window_regular_session_is_24h_utc() {
        let cal = crypto_spot();
        let ts = chrono::Utc
            .with_ymd_and_hms(2024, 8, 17, 3, 14, 15)
            .unwrap();
        let bw = cal
            .bar_window(ts, BarPeriod::Session(SessionSpan::Regular))
            .unwrap();
        assert_eq!(
            bw.open,
            chrono::Utc.with_ymd_and_hms(2024, 8, 17, 0, 0, 0).unwrap()
        );
        assert_eq!(
            bw.close,
            chrono::Utc.with_ymd_and_hms(2024, 8, 18, 0, 0, 0).unwrap()
        );
    }

    #[test]
    fn crypto_bar_window_extended_aliases_regular() {
        let cal = crypto_spot();
        let ts = chrono::Utc
            .with_ymd_and_hms(2024, 8, 17, 3, 14, 15)
            .unwrap();
        let reg = cal
            .bar_window(ts, BarPeriod::Session(SessionSpan::Regular))
            .unwrap();
        let ext = cal
            .bar_window(ts, BarPeriod::Session(SessionSpan::Extended))
            .unwrap();
        assert_eq!(reg.open, ext.open);
        assert_eq!(reg.close, ext.close);
    }

    #[test]
    fn crypto_is_trading_every_day() {
        let cal = crypto_spot();
        assert!(cal.is_trading_day(chrono::NaiveDate::from_ymd_opt(2024, 12, 25).unwrap()));
        assert!(cal.is_trading_day(chrono::NaiveDate::from_ymd_opt(2024, 1, 1).unwrap()));
        assert!(cal.is_trading_day(chrono::NaiveDate::from_ymd_opt(2024, 1, 6).unwrap()));
    }

    #[test]
    fn crypto_sessions_between_one_week() {
        let cal = crypto_spot();
        let from = chrono::Utc.with_ymd_and_hms(2024, 8, 12, 0, 0, 0).unwrap();
        let to = chrono::Utc.with_ymd_and_hms(2024, 8, 19, 0, 0, 0).unwrap();
        let mut buf: smallvec::SmallVec<[_; 16]> = smallvec::SmallVec::new();
        let n = cal.sessions_between(from, to, &mut buf);
        assert_eq!(n, 7);
        assert!(buf.iter().all(|s| s.kind() == SessionKind::Regular));
    }
}

// ---------------------------------------------------------------------------
// Smart-constructor tests for BarPeriod
// ---------------------------------------------------------------------------

#[cfg(test)]
mod period_tests {
    use super::*;

    #[test]
    fn smart_constructors() {
        assert_eq!(BarPeriod::m1(), BarPeriod::Clock(ClockInterval::Minutes(1)));
        assert_eq!(BarPeriod::m5(), BarPeriod::Clock(ClockInterval::Minutes(5)));
        assert_eq!(BarPeriod::h1(), BarPeriod::Clock(ClockInterval::Hours(1)));
        assert_eq!(
            BarPeriod::d1_rth(),
            BarPeriod::Session(SessionSpan::Regular)
        );
        assert_eq!(
            BarPeriod::d1_eth(),
            BarPeriod::Session(SessionSpan::Extended)
        );
        assert_eq!(BarPeriod::w1(), BarPeriod::Calendar(CalendarSpan::Week));
        assert_eq!(BarPeriod::mn1(), BarPeriod::Calendar(CalendarSpan::Month));
    }
}
