//! NYSE (XNYS) equity-market calendar.
//!
//! - Coverage: 2000-01-01 .. 2032-01-01 (half-open).
//! - Timezone: `America/New_York`.
//! - Sessions:
//!   - Pre-market: 04:00–09:30 ET. (ECN/ARCA convention used by TV,
//!     Bloomberg, IBKR TWS. NYSE floor formally accepts orders from
//!     06:30 ET; documenting here to avoid surprise.)
//!   - Regular:   09:30–16:00 ET (13:00 ET on early-close days).
//!   - Post-market: 16:00–20:00 ET (17:00 ET on early-close days).
//! - Holidays and ad-hoc closures: see `holidays.rs`.

pub(crate) mod holidays;
#[cfg(test)]
mod tests;

use std::ops::Range;
use std::sync::LazyLock;

use chrono::{Datelike, Duration, NaiveDate, NaiveTime, TimeZone, Timelike};
use chrono_tz::Tz;
use smallvec::SmallVec;

use crate::exchange::ExchangeCalendar;
use crate::period::{BarPeriod, CalendarSpan, ClockInterval, SessionSpan};
use crate::types::{
    BarWindow, CalendarError, CalendarId, Session, SessionBuf, SessionKind, Timestamp, TradingDay,
};
use crate::TimeAxisPolicy;

/// `CalendarId` for NYSE-style equities (used by NYSE-listed + NASDAQ in
/// practice; a separate `CRYPTO` calendar serves 24/7 markets).
pub const XNYS_ID: CalendarId = CalendarId("XNYS");

const COVERAGE_START_YEAR: i32 = 2000;
/// End year is EXCLUSIVE — `covers()` is half-open, so the end date
/// itself is OUT of coverage.
const COVERAGE_END_YEAR: i32 = 2032;

const PRE_OPEN_HM: (u32, u32) = (4, 0);
const REG_OPEN_HM: (u32, u32) = (9, 30);
const REG_CLOSE_HM: (u32, u32) = (16, 0);
const POST_CLOSE_HM: (u32, u32) = (20, 0);

/// NYSE (XNYS) equity-market calendar. Stateless; all session math is
/// per-call. Constructed once as `XNYS` and referenced as
/// `&'static dyn ExchangeCalendar`.
pub struct XnysCalendar {
    tz: Tz,
}

impl XnysCalendar {
    const fn new() -> Self {
        Self {
            tz: chrono_tz::America::New_York,
        }
    }

    /// Helper: convert an ET-local `(date, (h, m))` to a UTC timestamp,
    /// handling DST transitions deterministically. Uses "latest" for
    /// DST-fold ambiguity (preferring post-transition) and the first
    /// valid instant for DST-gap ambiguity; XNYS sessions never start
    /// inside a DST transition window, so these branches are defensive.
    fn et_to_utc(&self, date: NaiveDate, hm: (u32, u32)) -> Timestamp {
        let t = NaiveTime::from_hms_opt(hm.0, hm.1, 0).expect("valid HH:MM");
        let naive = date.and_time(t);
        match self.tz.from_local_datetime(&naive) {
            chrono::LocalResult::Single(dt) => dt.with_timezone(&chrono::Utc),
            chrono::LocalResult::Ambiguous(_earlier, later) => later.with_timezone(&chrono::Utc),
            chrono::LocalResult::None => {
                // DST-gap: skip forward an hour.
                let shifted = naive + Duration::hours(1);
                self.tz
                    .from_local_datetime(&shifted)
                    .single()
                    .expect("DST-gap fallback resolves uniquely")
                    .with_timezone(&chrono::Utc)
            }
        }
    }

    /// Date in ET that `ts` falls under.
    fn et_date(&self, ts: Timestamp) -> NaiveDate {
        ts.with_timezone(&self.tz).date_naive()
    }

    /// Build the `TradingDay` for `date`, assuming `date` is in coverage.
    /// Returns `None` if `date` is a weekend or holiday (no sessions).
    fn build_trading_day(&self, date: NaiveDate) -> TradingDay {
        let is_weekend = matches!(date.weekday(), chrono::Weekday::Sat | chrono::Weekday::Sun);
        let holiday_name = holidays::holiday_name(date);
        let is_holiday = holiday_name.is_some();

        if is_weekend || is_holiday {
            return TradingDay {
                date,
                sessions: SmallVec::new(),
                is_early_close: false,
                is_holiday,
                holiday_name,
            };
        }

        let early = holidays::early_close_minute(date);
        let is_early_close = early.is_some();

        let reg_close_hm = early.unwrap_or(REG_CLOSE_HM);
        // Post-market close shifts in lockstep with regular close on
        // early-close days: 17:00 ET instead of 20:00 ET.
        let post_close_hm = if is_early_close {
            (17, 0)
        } else {
            POST_CLOSE_HM
        };

        let pre_open = self.et_to_utc(date, PRE_OPEN_HM);
        let reg_open = self.et_to_utc(date, REG_OPEN_HM);
        let reg_close = self.et_to_utc(date, reg_close_hm);
        let post_close = self.et_to_utc(date, post_close_hm);

        let mut sessions: SmallVec<[Session; 4]> = SmallVec::new();
        sessions.push(Session::new(
            XNYS_ID,
            SessionKind::PreMarket,
            pre_open,
            reg_open,
        ));
        sessions.push(Session::new(
            XNYS_ID,
            SessionKind::Regular,
            reg_open,
            reg_close,
        ));
        sessions.push(Session::new(
            XNYS_ID,
            SessionKind::PostMarket,
            reg_close,
            post_close,
        ));

        TradingDay {
            date,
            sessions,
            is_early_close,
            is_holiday: false,
            holiday_name: None,
        }
    }

    /// Is `date` within `covers()`?
    fn in_coverage(&self, date: NaiveDate) -> bool {
        let start = NaiveDate::from_ymd_opt(COVERAGE_START_YEAR, 1, 1).unwrap();
        let end = NaiveDate::from_ymd_opt(COVERAGE_END_YEAR, 1, 1).unwrap();
        date >= start && date < end
    }

    /// First trading day on/after `date`. Returns `None` if no trading
    /// day is reachable within coverage (iterates bounded).
    fn next_trading_day_on_or_after(&self, date: NaiveDate) -> Option<NaiveDate> {
        let mut d = date;
        let end = NaiveDate::from_ymd_opt(COVERAGE_END_YEAR, 1, 1).unwrap();
        while d < end {
            if self.is_trading_day(d) {
                return Some(d);
            }
            d += Duration::days(1);
        }
        None
    }

    /// Last trading day on/before `date`.
    fn prev_trading_day_on_or_before(&self, date: NaiveDate) -> Option<NaiveDate> {
        let start = NaiveDate::from_ymd_opt(COVERAGE_START_YEAR, 1, 1).unwrap();
        let mut d = date;
        while d >= start {
            if self.is_trading_day(d) {
                return Some(d);
            }
            d -= Duration::days(1);
        }
        None
    }

    /// Closed-session stub for out-of-coverage / weekend / holiday.
    fn closed_session(&self, ts: Timestamp) -> Session {
        // Synthesize a 1-minute "closed" window around `ts` — it carries
        // no meaningful bounds since the session is Closed, but keeps
        // the Session type total.
        Session::new(XNYS_ID, SessionKind::Closed, ts, ts)
    }
}

// ---------------------------------------------------------------------------
// Trait impl
// ---------------------------------------------------------------------------

impl ExchangeCalendar for XnysCalendar {
    fn id(&self) -> CalendarId {
        XNYS_ID
    }

    fn tz(&self) -> Tz {
        self.tz
    }

    fn covers(&self) -> Range<NaiveDate> {
        let start = NaiveDate::from_ymd_opt(COVERAGE_START_YEAR, 1, 1).unwrap();
        let end = NaiveDate::from_ymd_opt(COVERAGE_END_YEAR, 1, 1).unwrap();
        start..end
    }

    fn time_axis_policy(&self) -> TimeAxisPolicy {
        TimeAxisPolicy::CompressedSessionBoundaries
    }

    fn trading_day(&self, date: NaiveDate) -> Result<TradingDay, CalendarError> {
        if !self.in_coverage(date) {
            return Err(CalendarError::OutOfRange(date));
        }
        Ok(self.build_trading_day(date))
    }

    fn is_trading_day(&self, date: NaiveDate) -> bool {
        if !self.in_coverage(date) {
            return false;
        }
        if matches!(date.weekday(), chrono::Weekday::Sat | chrono::Weekday::Sun) {
            return false;
        }
        holidays::holiday_name(date).is_none()
    }

    fn classify(&self, ts: Timestamp) -> Session {
        let et_date = self.et_date(ts);
        if !self.in_coverage(et_date) {
            return self.closed_session(ts);
        }
        let td = self.build_trading_day(et_date);
        for s in &td.sessions {
            if s.contains(ts) {
                return s.clone();
            }
        }
        // Check the PREVIOUS trading day's post-market in case `ts`
        // falls just after midnight ET — though XNYS post-market ends
        // at 20:00 ET so this is defensive.
        self.closed_session(ts)
    }

    fn bar_window(&self, ts: Timestamp, period: BarPeriod) -> Result<BarWindow, CalendarError> {
        self.validate_period(period)?;
        match period {
            BarPeriod::Clock(ci) => clock_bar_window(self, ts, ci),
            BarPeriod::Session(span) => session_bar_window(self, ts, span),
            BarPeriod::Calendar(span) => calendar_bar_window(self, ts, span),
        }
    }

    fn validate_period(&self, period: BarPeriod) -> Result<(), CalendarError> {
        match period {
            // XNYS has no distinct electronic session.
            BarPeriod::Session(SessionSpan::Eth) => Err(CalendarError::UnsupportedPeriod {
                calendar: XNYS_ID,
                period,
            }),
            _ => Ok(()),
        }
    }

    fn sessions_between(&self, from: Timestamp, to: Timestamp, out: &mut SessionBuf) -> usize {
        out.clear();
        if to <= from {
            return 0;
        }
        let from_date = self.et_date(from);
        let to_date = self.et_date(to);
        // Clamp to coverage.
        let start = NaiveDate::from_ymd_opt(COVERAGE_START_YEAR, 1, 1).unwrap();
        let end = NaiveDate::from_ymd_opt(COVERAGE_END_YEAR, 1, 1).unwrap();
        let start_date = from_date.max(start);
        let end_date = (to_date + Duration::days(1)).min(end);

        let mut d = start_date;
        while d < end_date {
            let td = self.build_trading_day(d);
            for s in td.sessions {
                if s.close() > from && s.open() < to {
                    out.push(s);
                }
            }
            d += Duration::days(1);
        }
        out.len()
    }

    fn next_open(&self, ts: Timestamp, kind: SessionKind) -> Option<Timestamp> {
        let et_date = self.et_date(ts);
        let mut d = if self.in_coverage(et_date) {
            et_date
        } else {
            NaiveDate::from_ymd_opt(COVERAGE_START_YEAR, 1, 1).unwrap()
        };
        let end = NaiveDate::from_ymd_opt(COVERAGE_END_YEAR, 1, 1).unwrap();
        while d < end {
            if self.is_trading_day(d) {
                let td = self.build_trading_day(d);
                for s in td.sessions {
                    if s.kind() == kind && s.open() > ts {
                        return Some(s.open());
                    }
                }
            }
            d += Duration::days(1);
        }
        None
    }

    fn prev_close(&self, ts: Timestamp, kind: SessionKind) -> Option<Timestamp> {
        let et_date = self.et_date(ts);
        let start = NaiveDate::from_ymd_opt(COVERAGE_START_YEAR, 1, 1).unwrap();
        let mut d = if self.in_coverage(et_date) {
            et_date
        } else {
            NaiveDate::from_ymd_opt(COVERAGE_END_YEAR, 12, 31).unwrap()
        };
        while d >= start {
            if self.is_trading_day(d) {
                let td = self.build_trading_day(d);
                for s in td.sessions.iter().rev() {
                    if s.kind() == kind && s.close() <= ts {
                        return Some(s.close());
                    }
                }
            }
            d -= Duration::days(1);
        }
        None
    }
}

// ---------------------------------------------------------------------------
// bar_window helpers
// ---------------------------------------------------------------------------

/// Clock-interval window: UTC-epoch modular. The session is the one
/// `ts` falls inside (for session-boundary awareness elsewhere in the
/// stack).
fn clock_bar_window(
    cal: &XnysCalendar,
    ts: Timestamp,
    ci: ClockInterval,
) -> Result<BarWindow, CalendarError> {
    let secs_per_bar: i64 = match ci {
        ClockInterval::Seconds(s) => s as i64,
        ClockInterval::Minutes(m) => (m as i64) * 60,
        ClockInterval::Hours(h) => (h as i64) * 3600,
    };
    if secs_per_bar <= 0 {
        return Err(CalendarError::UnsupportedPeriod {
            calendar: XNYS_ID,
            period: BarPeriod::Clock(ci),
        });
    }
    let ts_secs = ts.timestamp();
    let bar_start_secs = ts_secs - ts_secs.rem_euclid(secs_per_bar);
    let open = chrono::DateTime::<chrono::Utc>::from_timestamp(bar_start_secs, 0)
        .ok_or(CalendarError::OutOfRange(cal.et_date(ts)))?;
    let close = chrono::DateTime::<chrono::Utc>::from_timestamp(bar_start_secs + secs_per_bar, 0)
        .ok_or(CalendarError::OutOfRange(cal.et_date(ts)))?;
    let session = cal.classify(ts);
    Ok(BarWindow {
        open,
        close,
        session,
    })
}

/// Session-span window: Regular → RTH; Extended → pre+RTH+post as one bar.
fn session_bar_window(
    cal: &XnysCalendar,
    ts: Timestamp,
    span: SessionSpan,
) -> Result<BarWindow, CalendarError> {
    let et_date = cal.et_date(ts);
    // Find the trading day for `ts` (or roll forward if on a
    // weekend/holiday; this matches the "next bar" convention).
    let date = cal
        .next_trading_day_on_or_after(et_date)
        .ok_or(CalendarError::OutOfRange(et_date))?;
    let td = cal.build_trading_day(date);
    match span {
        SessionSpan::Regular => {
            // RTH session = middle entry (PreMarket, Regular, PostMarket).
            let reg = td
                .sessions
                .iter()
                .find(|s| s.kind() == SessionKind::Regular)
                .ok_or(CalendarError::OutOfRange(date))?;
            Ok(BarWindow {
                open: reg.open(),
                close: reg.close(),
                session: reg.clone(),
            })
        }
        SessionSpan::Extended => {
            // ETH = [04:00 ET, 20:00 ET] (or 17:00 on early-close).
            let pre = td.sessions.first().ok_or(CalendarError::OutOfRange(date))?;
            let post = td.sessions.last().ok_or(CalendarError::OutOfRange(date))?;
            // Re-tag as Regular-session for the bar (one "extended" bar
            // covers the whole day).
            let reg = td
                .sessions
                .iter()
                .find(|s| s.kind() == SessionKind::Regular)
                .ok_or(CalendarError::OutOfRange(date))?;
            Ok(BarWindow {
                open: pre.open(),
                close: post.close(),
                session: reg.clone(),
            })
        }
        SessionSpan::Eth => Err(CalendarError::UnsupportedPeriod {
            calendar: XNYS_ID,
            period: BarPeriod::Session(span),
        }),
    }
}

/// Calendar-span window: ISO week / month / quarter / year anchored to
/// trading days. For a Week bar, `[first trading day of ISO week RTH open,
/// last trading day of ISO week RTH close]`. Month/quarter/year analogous.
fn calendar_bar_window(
    cal: &XnysCalendar,
    ts: Timestamp,
    span: CalendarSpan,
) -> Result<BarWindow, CalendarError> {
    use chrono::Datelike;
    let et_date = cal.et_date(ts);

    let (start, end_excl) = match span {
        CalendarSpan::Week => {
            // ISO week: Monday .. next Monday.
            let iso_week = et_date.iso_week();
            let monday =
                NaiveDate::from_isoywd_opt(iso_week.year(), iso_week.week(), chrono::Weekday::Mon)
                    .ok_or(CalendarError::OutOfRange(et_date))?;
            let next_monday = monday + Duration::days(7);
            (monday, next_monday)
        }
        CalendarSpan::Month => {
            let first = NaiveDate::from_ymd_opt(et_date.year(), et_date.month(), 1)
                .ok_or(CalendarError::OutOfRange(et_date))?;
            let (ny, nm) = if first.month() == 12 {
                (first.year() + 1, 1)
            } else {
                (first.year(), first.month() + 1)
            };
            let next =
                NaiveDate::from_ymd_opt(ny, nm, 1).ok_or(CalendarError::OutOfRange(et_date))?;
            (first, next)
        }
        CalendarSpan::Quarter => {
            let q = (et_date.month() - 1) / 3;
            let qstart_month = q * 3 + 1;
            let first = NaiveDate::from_ymd_opt(et_date.year(), qstart_month, 1)
                .ok_or(CalendarError::OutOfRange(et_date))?;
            let (ny, nm) = if qstart_month + 3 > 12 {
                (first.year() + 1, qstart_month + 3 - 12)
            } else {
                (first.year(), qstart_month + 3)
            };
            let next =
                NaiveDate::from_ymd_opt(ny, nm, 1).ok_or(CalendarError::OutOfRange(et_date))?;
            (first, next)
        }
        CalendarSpan::Year => {
            let first = NaiveDate::from_ymd_opt(et_date.year(), 1, 1)
                .ok_or(CalendarError::OutOfRange(et_date))?;
            let next = NaiveDate::from_ymd_opt(et_date.year() + 1, 1, 1)
                .ok_or(CalendarError::OutOfRange(et_date))?;
            (first, next)
        }
    };

    let first_trading = cal
        .next_trading_day_on_or_after(start)
        .filter(|d| *d < end_excl)
        .ok_or(CalendarError::OutOfRange(start))?;
    let last_trading = cal
        .prev_trading_day_on_or_before(end_excl - Duration::days(1))
        .filter(|d| *d >= start)
        .ok_or(CalendarError::OutOfRange(end_excl - Duration::days(1)))?;

    let first_td = cal.build_trading_day(first_trading);
    let last_td = cal.build_trading_day(last_trading);
    let first_reg = first_td
        .sessions
        .iter()
        .find(|s| s.kind() == SessionKind::Regular)
        .ok_or(CalendarError::OutOfRange(first_trading))?;
    let last_reg = last_td
        .sessions
        .iter()
        .find(|s| s.kind() == SessionKind::Regular)
        .ok_or(CalendarError::OutOfRange(last_trading))?;

    Ok(BarWindow {
        open: first_reg.open(),
        close: last_reg.close(),
        session: first_reg.clone(),
    })
}

/// Defensive unused-import guard — `Timelike` is kept in scope in case
/// future window math needs sub-minute resolution.
#[allow(dead_code)]
fn _timelike_ref(t: &impl Timelike) -> u32 {
    t.hour()
}

// ---------------------------------------------------------------------------
// LazyLock singleton
// ---------------------------------------------------------------------------

/// Process-global XNYS calendar singleton. Consumers take
/// `&'static dyn ExchangeCalendar` via `xnys()`.
pub static XNYS: LazyLock<XnysCalendar> = LazyLock::new(XnysCalendar::new);
