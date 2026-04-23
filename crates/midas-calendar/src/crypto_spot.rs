//! Crypto-spot 24/7 UTC calendar.
//!
//! Always open. `TimeAxisPolicy::Continuous`. Rejects
//! `BarPeriod::Session(SessionSpan::Eth)` — there's no electronic-vs-pit
//! distinction on a 24h market.
//!
//! Coverage is intentionally wide (1970-01-01 .. 2100-01-01) to keep the
//! crate trivial to use with any timestamp.

use std::ops::Range;
use std::sync::LazyLock;

use chrono::{Duration, NaiveDate, NaiveTime, TimeZone};
use chrono_tz::Tz;
use smallvec::SmallVec;

use crate::exchange::ExchangeCalendar;
use crate::period::{BarPeriod, CalendarSpan, ClockInterval, SessionSpan};
use crate::types::{
    BarWindow, CalendarError, CalendarId, Session, SessionBuf, SessionKind, Timestamp, TradingDay,
};
use crate::TimeAxisPolicy;

pub const CRYPTO_SPOT_ID: CalendarId = CalendarId("CRYPTO");

const COVERAGE_START_YEAR: i32 = 1970;
const COVERAGE_END_YEAR: i32 = 2100;

/// Crypto-spot 24/7 calendar. Sessions are always `SessionKind::Regular`
/// and cover 00:00–24:00 UTC on the calendar date.
pub struct CryptoSpotCalendar {
    tz: Tz,
}

impl CryptoSpotCalendar {
    const fn new() -> Self {
        Self { tz: chrono_tz::UTC }
    }

    fn utc_date(&self, ts: Timestamp) -> NaiveDate {
        ts.date_naive()
    }

    /// Build the 24h UTC session for `date`.
    fn build_day(&self, date: NaiveDate) -> TradingDay {
        let open_naive = date.and_time(NaiveTime::from_hms_opt(0, 0, 0).unwrap());
        let open = self
            .tz
            .from_utc_datetime(&open_naive)
            .with_timezone(&chrono::Utc);
        let close = open + Duration::days(1);

        let session = Session::new(CRYPTO_SPOT_ID, SessionKind::Regular, open, close);
        let mut sessions = SmallVec::new();
        sessions.push(session);
        TradingDay {
            date,
            sessions,
            is_early_close: false,
            is_holiday: false,
            holiday_name: None,
        }
    }

    fn in_coverage(&self, date: NaiveDate) -> bool {
        let start = NaiveDate::from_ymd_opt(COVERAGE_START_YEAR, 1, 1).unwrap();
        let end = NaiveDate::from_ymd_opt(COVERAGE_END_YEAR, 1, 1).unwrap();
        date >= start && date < end
    }

    fn closed_session(&self, ts: Timestamp) -> Session {
        Session::new(CRYPTO_SPOT_ID, SessionKind::Closed, ts, ts)
    }
}

impl ExchangeCalendar for CryptoSpotCalendar {
    fn id(&self) -> CalendarId {
        CRYPTO_SPOT_ID
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
        TimeAxisPolicy::Continuous
    }

    fn trading_day(&self, date: NaiveDate) -> Result<TradingDay, CalendarError> {
        if !self.in_coverage(date) {
            return Err(CalendarError::OutOfRange(date));
        }
        Ok(self.build_day(date))
    }

    fn is_trading_day(&self, date: NaiveDate) -> bool {
        self.in_coverage(date)
    }

    fn classify(&self, ts: Timestamp) -> Session {
        let date = self.utc_date(ts);
        if !self.in_coverage(date) {
            return self.closed_session(ts);
        }
        let td = self.build_day(date);
        td.sessions.into_iter().next().expect("24h session exists")
    }

    fn bar_window(&self, ts: Timestamp, period: BarPeriod) -> Result<BarWindow, CalendarError> {
        self.validate_period(period)?;
        match period {
            BarPeriod::Clock(ci) => clock_bar_window(self, ts, ci),
            BarPeriod::Session(_span) => {
                // Regular OR Extended (Extended aliases Regular on crypto).
                let date = self.utc_date(ts);
                if !self.in_coverage(date) {
                    return Err(CalendarError::OutOfRange(date));
                }
                let td = self.build_day(date);
                let s = td.sessions.into_iter().next().expect("24h session");
                Ok(BarWindow {
                    open: s.open(),
                    close: s.close(),
                    session: s,
                })
            }
            BarPeriod::Calendar(span) => calendar_bar_window(self, ts, span),
        }
    }

    fn validate_period(&self, period: BarPeriod) -> Result<(), CalendarError> {
        match period {
            BarPeriod::Session(SessionSpan::Eth) => Err(CalendarError::UnsupportedPeriod {
                calendar: CRYPTO_SPOT_ID,
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
        let start = NaiveDate::from_ymd_opt(COVERAGE_START_YEAR, 1, 1).unwrap();
        let end = NaiveDate::from_ymd_opt(COVERAGE_END_YEAR, 1, 1).unwrap();

        let mut d = self.utc_date(from).max(start);
        let end_date = (self.utc_date(to) + Duration::days(1)).min(end);
        while d < end_date {
            let td = self.build_day(d);
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
        if kind != SessionKind::Regular {
            return None;
        }
        let date = self.utc_date(ts);
        let mut d = if self.in_coverage(date) {
            date
        } else {
            NaiveDate::from_ymd_opt(COVERAGE_START_YEAR, 1, 1).unwrap()
        };
        let end = NaiveDate::from_ymd_opt(COVERAGE_END_YEAR, 1, 1).unwrap();
        while d < end {
            let td = self.build_day(d);
            if let Some(s) = td.sessions.first() {
                if s.open() > ts {
                    return Some(s.open());
                }
            }
            d += Duration::days(1);
        }
        None
    }

    fn prev_close(&self, ts: Timestamp, kind: SessionKind) -> Option<Timestamp> {
        if kind != SessionKind::Regular {
            return None;
        }
        let start = NaiveDate::from_ymd_opt(COVERAGE_START_YEAR, 1, 1).unwrap();
        let date = self.utc_date(ts);
        let mut d = if self.in_coverage(date) {
            date
        } else {
            NaiveDate::from_ymd_opt(COVERAGE_END_YEAR, 12, 31).unwrap()
        };
        while d >= start {
            let td = self.build_day(d);
            if let Some(s) = td.sessions.first() {
                if s.close() <= ts {
                    return Some(s.close());
                }
            }
            d -= Duration::days(1);
        }
        None
    }
}

fn clock_bar_window(
    cal: &CryptoSpotCalendar,
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
            calendar: CRYPTO_SPOT_ID,
            period: BarPeriod::Clock(ci),
        });
    }
    let ts_secs = ts.timestamp();
    let bar_start_secs = ts_secs - ts_secs.rem_euclid(secs_per_bar);
    let open = chrono::DateTime::<chrono::Utc>::from_timestamp(bar_start_secs, 0)
        .ok_or(CalendarError::OutOfRange(cal.utc_date(ts)))?;
    let close = chrono::DateTime::<chrono::Utc>::from_timestamp(bar_start_secs + secs_per_bar, 0)
        .ok_or(CalendarError::OutOfRange(cal.utc_date(ts)))?;
    let session = cal.classify(ts);
    Ok(BarWindow {
        open,
        close,
        session,
    })
}

fn calendar_bar_window(
    cal: &CryptoSpotCalendar,
    ts: Timestamp,
    span: CalendarSpan,
) -> Result<BarWindow, CalendarError> {
    use chrono::Datelike;
    let date = cal.utc_date(ts);
    let (start, end_excl) = match span {
        CalendarSpan::Week => {
            let iso_week = date.iso_week();
            let monday =
                NaiveDate::from_isoywd_opt(iso_week.year(), iso_week.week(), chrono::Weekday::Mon)
                    .ok_or(CalendarError::OutOfRange(date))?;
            (monday, monday + Duration::days(7))
        }
        CalendarSpan::Month => {
            let first = NaiveDate::from_ymd_opt(date.year(), date.month(), 1)
                .ok_or(CalendarError::OutOfRange(date))?;
            let (ny, nm) = if first.month() == 12 {
                (first.year() + 1, 1)
            } else {
                (first.year(), first.month() + 1)
            };
            let next = NaiveDate::from_ymd_opt(ny, nm, 1).ok_or(CalendarError::OutOfRange(date))?;
            (first, next)
        }
        CalendarSpan::Quarter => {
            let q = (date.month() - 1) / 3;
            let qstart_month = q * 3 + 1;
            let first = NaiveDate::from_ymd_opt(date.year(), qstart_month, 1)
                .ok_or(CalendarError::OutOfRange(date))?;
            let (ny, nm) = if qstart_month + 3 > 12 {
                (first.year() + 1, qstart_month + 3 - 12)
            } else {
                (first.year(), qstart_month + 3)
            };
            let next = NaiveDate::from_ymd_opt(ny, nm, 1).ok_or(CalendarError::OutOfRange(date))?;
            (first, next)
        }
        CalendarSpan::Year => {
            let first = NaiveDate::from_ymd_opt(date.year(), 1, 1)
                .ok_or(CalendarError::OutOfRange(date))?;
            let next = NaiveDate::from_ymd_opt(date.year() + 1, 1, 1)
                .ok_or(CalendarError::OutOfRange(date))?;
            (first, next)
        }
    };
    let start_td = cal.build_day(start);
    let end_td = cal.build_day(end_excl - Duration::days(1));
    let open = start_td.sessions.first().unwrap().open();
    let close = end_td.sessions.last().unwrap().close();
    let session = start_td.sessions.into_iter().next().unwrap();
    Ok(BarWindow {
        open,
        close,
        session,
    })
}

/// Process-global CryptoSpot calendar singleton.
pub static CRYPTO_SPOT: LazyLock<CryptoSpotCalendar> = LazyLock::new(CryptoSpotCalendar::new);
