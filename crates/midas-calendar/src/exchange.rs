//! The `ExchangeCalendar` trait.
//!
//! Every session-aware producer speaks this trait; every consumer holds
//! a `&'static dyn ExchangeCalendar`. Calendars are process-global
//! singletons behind `LazyLock` (see `xnys`, `crypto_spot` modules).
//! Per the ideal-design document:
//!
//! - `classify()` is INFALLIBLE and saturating. Out-of-range timestamps
//!   return `SessionKind::Closed`.
//! - `covers()` is HALF-OPEN `[start, end)`.
//! - `sessions_between` writes into a caller-owned `SmallVec` so the
//!   render hot-path allocates zero.

use std::ops::Range;

use chrono::NaiveDate;
use chrono_tz::Tz;

use crate::types::{
    BarWindow, CalendarError, CalendarId, Session, SessionBuf, SessionKind, Timestamp, TradingDay,
};
use crate::BarPeriod;
use crate::TimeAxisPolicy;

/// Calendar for one exchange. See crate docs for ownership and
/// thread-safety contracts.
pub trait ExchangeCalendar: Send + Sync + 'static {
    /// MIC-ish identifier.
    fn id(&self) -> CalendarId;

    /// IANA timezone used for rendering and for naive-local ↔ UTC
    /// conversion inside calendar internals.
    fn tz(&self) -> Tz;

    /// Half-open `[start, end)`. A date equal to `covers().end` is OUT.
    fn covers(&self) -> Range<NaiveDate>;

    /// Whether the time axis should collapse closed time.
    fn time_axis_policy(&self) -> TimeAxisPolicy;

    /// Day-level view. Fails with `OutOfRange` if `date` sits outside
    /// `covers()`.
    fn trading_day(&self, date: NaiveDate) -> Result<TradingDay, CalendarError>;

    /// Fast boolean "is anything at all scheduled on this date." Returns
    /// `false` for weekends, holidays, and out-of-coverage dates.
    fn is_trading_day(&self, date: NaiveDate) -> bool;

    /// Point-in-time session classification. INFALLIBLE. Returns a
    /// `Session` with `kind == SessionKind::Closed` for holidays,
    /// weekends, overnight gaps on equities/futures calendars, or any
    /// timestamp outside `covers()`.
    fn classify(&self, ts: Timestamp) -> Session;

    /// Bar window for `(ts, period)`. Clock intervals align to UTC
    /// epoch; session-scoped periods resolve through the calendar.
    /// Returns `UnsupportedPeriod` on invalid pairings — although in
    /// practice `validate_period` catches these at chart construction.
    fn bar_window(&self, ts: Timestamp, period: BarPeriod) -> Result<BarWindow, CalendarError>;

    /// Called once at `Chart::new` time. Fails fast on nonsensical
    /// (calendar, period) pairings — e.g.
    /// `(CryptoSpot, Session(Eth))` — so downstream hot-path callers can
    /// treat `bar_window` errors as unreachable.
    fn validate_period(&self, period: BarPeriod) -> Result<(), CalendarError>;

    /// Fill `out` with every session intersecting `[from, to)`. The
    /// caller owns the buffer (reuse across frames to avoid allocation).
    /// Returns the number of sessions written.
    ///
    /// The buffer type is aliased to [`SessionBuf`] so the inline
    /// capacity (currently 16) lives in one place; callers pass any
    /// `&mut SessionBuf`.
    fn sessions_between(&self, from: Timestamp, to: Timestamp, out: &mut SessionBuf) -> usize;

    /// Next session-open of `kind` strictly after `ts` (or equal, if
    /// `ts` is itself an open).
    fn next_open(&self, ts: Timestamp, kind: SessionKind) -> Option<Timestamp>;

    /// Most recent session-close of `kind` at or before `ts`.
    fn prev_close(&self, ts: Timestamp, kind: SessionKind) -> Option<Timestamp>;
}
