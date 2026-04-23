//! Golden-fixture + spot-check tests for `XnysCalendar`.
//!
//! The golden fixture cross-checks our rule-based holiday set against
//! `nyse-holiday-cal` (library coverage 2020..=2028) for every day in
//! that range. Any divergence is a bug in one of the two.

use chrono::{Datelike, Duration, NaiveDate};

use super::{XnysCalendar, XNYS, XNYS_ID};
use crate::exchange::ExchangeCalendar;
use crate::period::{BarPeriod, ClockInterval, SessionSpan};
use crate::types::{CalendarError, SessionKind};

// ---------------------------------------------------------------------------
// Golden fixture: cross-check against nyse-holiday-cal
// ---------------------------------------------------------------------------

/// Iterate every day in [2020-01-01, 2029-01-01) and assert that
/// `XnysCalendar::is_trading_day` matches `nyse_holiday_cal::is_busday`.
/// The reference library covers 2020..=2028 (inclusive); we bound our
/// test to that range. Our crate's own coverage extends to 2031 but is
/// exercised elsewhere with spot checks.
///
/// A small allowlist handles ad-hoc closures published AFTER the
/// reference library's last release (v0.2.5, 2023-ish). These are cases
/// where we are intentionally more complete than the library:
/// - 2025-01-09: Jimmy Carter day of mourning (announced Dec 2024).
///   The reference library was frozen before this; our rule table
///   includes it per the session-aware-charts spec.
#[test]
fn golden_fixture_matches_nyse_holiday_cal() {
    use nyse_holiday_cal::HolidayCal;

    // Dates where our rule table correctly diverges from the reference
    // library (we know about closures the library doesn't yet).
    const KNOWN_DIVERGENCES: &[(i32, u32, u32, &str)] = &[(
        2025,
        1,
        9,
        "Jimmy Carter day of mourning (post-library release)",
    )];

    let start = NaiveDate::from_ymd_opt(2020, 1, 1).unwrap();
    let end = NaiveDate::from_ymd_opt(2029, 1, 1).unwrap();

    let mut d = start;
    let mut checked = 0usize;
    let mut divergences = 0usize;
    while d < end {
        let ours = XNYS.is_trading_day(d);
        let theirs = d.is_busday().expect("in library range");
        let key = (d.year(), d.month(), d.day());
        let known = KNOWN_DIVERGENCES
            .iter()
            .find(|(y, m, dd, _)| (*y, *m, *dd) == key);
        if ours != theirs {
            divergences += 1;
            assert!(
                known.is_some(),
                "{d} UNEXPECTED disagreement: ours={ours} reference={theirs}",
            );
        }
        d += Duration::days(1);
        checked += 1;
    }
    assert!(checked > 3000, "sanity: iterated {checked} days");
    assert_eq!(
        divergences,
        KNOWN_DIVERGENCES.len(),
        "every allow-listed divergence must actually fire",
    );
}

// ---------------------------------------------------------------------------
// Infallibility and saturation
// ---------------------------------------------------------------------------

#[test]
fn classify_is_infallible_out_of_range() {
    // 1999 is before coverage.
    let ts = chrono::Utc
        .with_ymd_and_hms(1999, 6, 15, 14, 30, 0)
        .unwrap();
    let s = XNYS.classify(ts);
    assert_eq!(s.kind(), SessionKind::Closed);
    assert_eq!(s.calendar(), XNYS_ID);
}

#[test]
fn classify_is_infallible_out_of_range_future() {
    // 2040 is well after coverage end (2032).
    let ts = chrono::Utc.with_ymd_and_hms(2040, 1, 1, 14, 30, 0).unwrap();
    assert_eq!(XNYS.classify(ts).kind(), SessionKind::Closed);
}

#[test]
fn classify_weekend_is_closed() {
    // 2024-01-06 Saturday, 14:30 UTC (09:30 ET).
    let ts = chrono::Utc.with_ymd_and_hms(2024, 1, 6, 14, 30, 0).unwrap();
    assert_eq!(XNYS.classify(ts).kind(), SessionKind::Closed);
}

#[test]
fn covers_is_half_open() {
    let c = XNYS.covers();
    assert_eq!(c.start, NaiveDate::from_ymd_opt(2000, 1, 1).unwrap());
    assert_eq!(c.end, NaiveDate::from_ymd_opt(2032, 1, 1).unwrap());
    // End is NOT in coverage.
    assert!(XNYS
        .trading_day(NaiveDate::from_ymd_opt(2032, 1, 1).unwrap())
        .is_err());
    // Last day in coverage IS reachable.
    let _ = XNYS
        .trading_day(NaiveDate::from_ymd_opt(2031, 12, 31).unwrap())
        .unwrap();
}

#[test]
fn validate_period_rejects_eth() {
    let p = BarPeriod::Session(SessionSpan::Eth);
    match XNYS.validate_period(p) {
        Err(CalendarError::UnsupportedPeriod { calendar, period }) => {
            assert_eq!(calendar, XNYS_ID);
            assert_eq!(period, p);
        }
        other => panic!("expected UnsupportedPeriod, got {other:?}"),
    }
}

#[test]
fn trading_day_out_of_range_returns_err() {
    let d = NaiveDate::from_ymd_opt(1999, 12, 31).unwrap();
    assert!(matches!(
        XNYS.trading_day(d),
        Err(CalendarError::OutOfRange(_))
    ));
}

// ---------------------------------------------------------------------------
// Session classification hour-sweep
// ---------------------------------------------------------------------------

#[test]
fn classify_regular_day_hour_sweep() {
    // 2024-01-17 (Wednesday) — a normal winter trading day.
    // Winter EST = UTC-5. ET 04:00 = 09:00 UTC. ET 09:30 = 14:30 UTC.
    // ET 16:00 = 21:00 UTC. ET 20:00 = 01:00 UTC (next day).
    let date = NaiveDate::from_ymd_opt(2024, 1, 17).unwrap();
    assert!(XNYS.is_trading_day(date));

    // 02:00 ET = 07:00 UTC → Closed (overnight).
    let t = chrono::Utc.with_ymd_and_hms(2024, 1, 17, 7, 0, 0).unwrap();
    assert_eq!(XNYS.classify(t).kind(), SessionKind::Closed);

    // 04:00 ET = 09:00 UTC → PreMarket.
    let t = chrono::Utc.with_ymd_and_hms(2024, 1, 17, 9, 0, 0).unwrap();
    assert_eq!(XNYS.classify(t).kind(), SessionKind::PreMarket);

    // 09:30 ET = 14:30 UTC → Regular.
    let t = chrono::Utc
        .with_ymd_and_hms(2024, 1, 17, 14, 30, 0)
        .unwrap();
    assert_eq!(XNYS.classify(t).kind(), SessionKind::Regular);

    // 15:59 ET = 20:59 UTC → Regular.
    let t = chrono::Utc
        .with_ymd_and_hms(2024, 1, 17, 20, 59, 0)
        .unwrap();
    assert_eq!(XNYS.classify(t).kind(), SessionKind::Regular);

    // 16:00 ET = 21:00 UTC → PostMarket (half-open: close exclusive).
    let t = chrono::Utc.with_ymd_and_hms(2024, 1, 17, 21, 0, 0).unwrap();
    assert_eq!(XNYS.classify(t).kind(), SessionKind::PostMarket);

    // 19:59 ET = 00:59 UTC next day → PostMarket.
    let t = chrono::Utc.with_ymd_and_hms(2024, 1, 18, 0, 59, 0).unwrap();
    assert_eq!(XNYS.classify(t).kind(), SessionKind::PostMarket);

    // 20:00 ET = 01:00 UTC next day → Closed.
    let t = chrono::Utc.with_ymd_and_hms(2024, 1, 18, 1, 0, 0).unwrap();
    assert_eq!(XNYS.classify(t).kind(), SessionKind::Closed);
}

// ---------------------------------------------------------------------------
// Specific date spot-checks
// ---------------------------------------------------------------------------

#[test]
fn black_friday_2023_early_close_13_et() {
    // 2023-11-24 (Fri after Thanksgiving). Winter EST → close 13:00 ET
    // = 18:00 UTC.
    let date = NaiveDate::from_ymd_opt(2023, 11, 24).unwrap();
    let td = XNYS.trading_day(date).unwrap();
    assert!(td.is_early_close);
    let reg = td
        .sessions
        .iter()
        .find(|s| s.kind() == SessionKind::Regular)
        .unwrap();
    assert_eq!(
        reg.close(),
        chrono::Utc
            .with_ymd_and_hms(2023, 11, 24, 18, 0, 0)
            .unwrap()
    );
    assert_eq!(
        reg.open(),
        chrono::Utc
            .with_ymd_and_hms(2023, 11, 24, 14, 30, 0)
            .unwrap()
    );
}

#[test]
fn juneteenth_pre_2022_is_trading_day() {
    // 2021-06-18 (Fri) — day before Saturday June 19 2021. Pre-2022
    // rule = no observance; must be a regular trading day.
    let d = NaiveDate::from_ymd_opt(2021, 6, 18).unwrap();
    assert!(XNYS.is_trading_day(d));
    assert!(XNYS.trading_day(d).unwrap().holiday_name.is_none());
}

#[test]
fn juneteenth_2022_onward_is_holiday() {
    // 2022-06-19 is Sunday → observed Monday 2022-06-20.
    let obs = NaiveDate::from_ymd_opt(2022, 6, 20).unwrap();
    assert!(!XNYS.is_trading_day(obs));
    let td = XNYS.trading_day(obs).unwrap();
    assert!(td.is_holiday);
    assert_eq!(td.holiday_name, Some("Juneteenth"));
}

#[test]
fn juneteenth_2024_weekday_is_holiday() {
    // 2024-06-19 is Wed — observed on the date itself.
    let d = NaiveDate::from_ymd_opt(2024, 6, 19).unwrap();
    assert!(!XNYS.is_trading_day(d));
    assert_eq!(
        XNYS.trading_day(d).unwrap().holiday_name,
        Some("Juneteenth")
    );
}

#[test]
fn reagan_funeral_2004_06_11_closed() {
    let d = NaiveDate::from_ymd_opt(2004, 6, 11).unwrap();
    assert!(!XNYS.is_trading_day(d));
    let td = XNYS.trading_day(d).unwrap();
    assert!(td.is_holiday);
    assert_eq!(td.holiday_name, Some("Ronald Reagan state funeral"));
}

#[test]
fn day_before_good_friday_2023_is_regular_trading_day() {
    // 2023-04-06 (Thu). NYSE does NOT early-close the day before Good
    // Friday; that's a SIFMA bond-market convention only.
    let d = NaiveDate::from_ymd_opt(2023, 4, 6).unwrap();
    assert!(XNYS.is_trading_day(d));
    let td = XNYS.trading_day(d).unwrap();
    assert!(!td.is_early_close);
    let reg = td
        .sessions
        .iter()
        .find(|s| s.kind() == SessionKind::Regular)
        .unwrap();
    assert_eq!(
        reg.close(),
        chrono::Utc.with_ymd_and_hms(2023, 4, 6, 20, 0, 0).unwrap(),
        "regular close at 20:00 UTC (16:00 ET EDT)",
    );
}

#[test]
fn good_friday_2023_closed() {
    let d = NaiveDate::from_ymd_opt(2023, 4, 7).unwrap();
    assert!(!XNYS.is_trading_day(d));
    assert_eq!(
        XNYS.trading_day(d).unwrap().holiday_name,
        Some("Good Friday")
    );
}

// ---------------------------------------------------------------------------
// bar_window
// ---------------------------------------------------------------------------

#[test]
fn bar_window_regular_session_winter() {
    // 2024-01-17 (Wed) regular session. 09:30 ET = 14:30 UTC EST, 16:00
    // ET = 21:00 UTC.
    let ts = chrono::Utc
        .with_ymd_and_hms(2024, 1, 17, 14, 30, 0)
        .unwrap();
    let bw = XNYS
        .bar_window(ts, BarPeriod::Session(SessionSpan::Regular))
        .unwrap();
    assert_eq!(
        bw.open,
        chrono::Utc
            .with_ymd_and_hms(2024, 1, 17, 14, 30, 0)
            .unwrap()
    );
    assert_eq!(
        bw.close,
        chrono::Utc.with_ymd_and_hms(2024, 1, 17, 21, 0, 0).unwrap()
    );
    assert_eq!(bw.session.kind(), SessionKind::Regular);
}

#[test]
fn bar_window_regular_session_early_close_day() {
    // Black Friday 2023 (13:00 ET close = 18:00 UTC EST).
    let ts = chrono::Utc
        .with_ymd_and_hms(2023, 11, 24, 15, 0, 0)
        .unwrap();
    let bw = XNYS
        .bar_window(ts, BarPeriod::Session(SessionSpan::Regular))
        .unwrap();
    assert_eq!(
        bw.open,
        chrono::Utc
            .with_ymd_and_hms(2023, 11, 24, 14, 30, 0)
            .unwrap()
    );
    assert_eq!(
        bw.close,
        chrono::Utc
            .with_ymd_and_hms(2023, 11, 24, 18, 0, 0)
            .unwrap()
    );
}

#[test]
fn bar_window_extended_session() {
    // 2024-01-17 ETH = 04:00 ET = 09:00 UTC → 20:00 ET = 01:00 UTC next.
    let ts = chrono::Utc
        .with_ymd_and_hms(2024, 1, 17, 14, 30, 0)
        .unwrap();
    let bw = XNYS
        .bar_window(ts, BarPeriod::Session(SessionSpan::Extended))
        .unwrap();
    assert_eq!(
        bw.open,
        chrono::Utc.with_ymd_and_hms(2024, 1, 17, 9, 0, 0).unwrap()
    );
    assert_eq!(
        bw.close,
        chrono::Utc.with_ymd_and_hms(2024, 1, 18, 1, 0, 0).unwrap()
    );
}

#[test]
fn bar_window_clock_m1_aligns_to_minute() {
    // ts = 14:30:37 UTC → M1 window = 14:30:00..14:31:00.
    let ts = chrono::Utc
        .with_ymd_and_hms(2024, 1, 17, 14, 30, 37)
        .unwrap();
    let bw = XNYS
        .bar_window(ts, BarPeriod::Clock(ClockInterval::Minutes(1)))
        .unwrap();
    assert_eq!(
        bw.open,
        chrono::Utc
            .with_ymd_and_hms(2024, 1, 17, 14, 30, 0)
            .unwrap()
    );
    assert_eq!(
        bw.close,
        chrono::Utc
            .with_ymd_and_hms(2024, 1, 17, 14, 31, 0)
            .unwrap()
    );
}

// ---------------------------------------------------------------------------
// sessions_between
// ---------------------------------------------------------------------------

#[test]
fn sessions_between_one_week_has_3x5_sessions() {
    // Mon 2024-01-15 is MLK day (holiday). Tue..Fri = 4 trading days *
    // 3 sessions each (pre+reg+post) = 12 sessions.
    let from = chrono::Utc.with_ymd_and_hms(2024, 1, 15, 0, 0, 0).unwrap();
    let to = chrono::Utc.with_ymd_and_hms(2024, 1, 20, 0, 0, 0).unwrap();
    let mut buf: smallvec::SmallVec<[_; 16]> = smallvec::SmallVec::new();
    let n = XNYS.sessions_between(from, to, &mut buf);
    assert_eq!(n, 12);
    assert_eq!(buf.len(), 12);
}

#[test]
fn sessions_between_reuses_buffer() {
    let from = chrono::Utc.with_ymd_and_hms(2024, 1, 16, 0, 0, 0).unwrap();
    let to = chrono::Utc.with_ymd_and_hms(2024, 1, 18, 0, 0, 0).unwrap();
    let mut buf: smallvec::SmallVec<[_; 16]> = smallvec::SmallVec::new();
    buf.push(crate::types::Session::new(
        XNYS_ID,
        SessionKind::Closed,
        from,
        from,
    ));
    let n = XNYS.sessions_between(from, to, &mut buf);
    // Expect exactly 6 (2 days * 3 sessions). Old contents must be
    // cleared.
    assert_eq!(n, 6);
    assert!(buf.iter().all(|s| s.kind() != SessionKind::Closed));
}

// ---------------------------------------------------------------------------
// Navigation
// ---------------------------------------------------------------------------

#[test]
fn next_open_skips_holiday() {
    // Fri 2024-01-12 close → next Regular open should be Tue 2024-01-16
    // (Mon is MLK).
    let close = chrono::Utc.with_ymd_and_hms(2024, 1, 12, 21, 0, 0).unwrap();
    let next = XNYS.next_open(close, SessionKind::Regular).unwrap();
    assert_eq!(
        next,
        chrono::Utc
            .with_ymd_and_hms(2024, 1, 16, 14, 30, 0)
            .unwrap()
    );
}

#[test]
fn prev_close_returns_most_recent_regular_close() {
    let ts = chrono::Utc.with_ymd_and_hms(2024, 1, 17, 12, 0, 0).unwrap();
    let prev = XNYS.prev_close(ts, SessionKind::Regular).unwrap();
    assert_eq!(
        prev,
        chrono::Utc.with_ymd_and_hms(2024, 1, 16, 21, 0, 0).unwrap()
    );
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

use chrono::TimeZone;

#[test]
fn xnys_singleton_via_helper() {
    let c: &'static dyn ExchangeCalendar = crate::xnys();
    assert_eq!(c.id(), XNYS_ID);
    // Same object as the static.
    let p: *const XnysCalendar = &*XNYS as *const _;
    let q: *const XnysCalendar = c as *const dyn ExchangeCalendar as *const XnysCalendar;
    assert_eq!(p, q);
}

// ---------------------------------------------------------------------------
// Ad-hoc closure coverage
// ---------------------------------------------------------------------------

#[test]
fn sandy_closed_both_days() {
    let d1 = NaiveDate::from_ymd_opt(2012, 10, 29).unwrap();
    let d2 = NaiveDate::from_ymd_opt(2012, 10, 30).unwrap();
    assert!(!XNYS.is_trading_day(d1));
    assert!(!XNYS.is_trading_day(d2));
}

#[test]
fn nine_eleven_closed_four_days() {
    for day in 11..=14 {
        let d = NaiveDate::from_ymd_opt(2001, 9, day).unwrap();
        assert!(
            !XNYS.is_trading_day(d),
            "2001-09-{day} should be closed (9/11 attacks)"
        );
    }
}

#[test]
fn carter_mourning_2025_01_09_closed() {
    let d = NaiveDate::from_ymd_opt(2025, 1, 9).unwrap();
    assert!(!XNYS.is_trading_day(d));
    assert_eq!(
        XNYS.trading_day(d).unwrap().holiday_name,
        Some("Jimmy Carter day of mourning")
    );
}

#[test]
fn thanksgiving_day_after_2024_11_29_early_close() {
    // Thanksgiving 2024 = Nov 28 (Thu). Day after = Nov 29 (Fri).
    let d = NaiveDate::from_ymd_opt(2024, 11, 29).unwrap();
    assert!(XNYS.is_trading_day(d));
    let td = XNYS.trading_day(d).unwrap();
    assert!(td.is_early_close);
}

#[test]
fn dst_transition_march_and_november() {
    // 2024 DST forward = Sun Mar 10; DST back = Sun Nov 3.
    // Mon Mar 11 2024 trades; 09:30 ET EDT = 13:30 UTC.
    let d = NaiveDate::from_ymd_opt(2024, 3, 11).unwrap();
    let td = XNYS.trading_day(d).unwrap();
    let reg = td
        .sessions
        .iter()
        .find(|s| s.kind() == SessionKind::Regular)
        .unwrap();
    assert_eq!(
        reg.open(),
        chrono::Utc
            .with_ymd_and_hms(2024, 3, 11, 13, 30, 0)
            .unwrap()
    );

    // Mon Nov 4 2024; 09:30 ET EST = 14:30 UTC.
    let d = NaiveDate::from_ymd_opt(2024, 11, 4).unwrap();
    let td = XNYS.trading_day(d).unwrap();
    let reg = td
        .sessions
        .iter()
        .find(|s| s.kind() == SessionKind::Regular)
        .unwrap();
    assert_eq!(
        reg.open(),
        chrono::Utc
            .with_ymd_and_hms(2024, 11, 4, 14, 30, 0)
            .unwrap()
    );
}

#[test]
fn calendar_independent_of_iso_week_year_rollover() {
    // Sanity: years where ISO week crosses year boundaries don't panic.
    // 2024-12-31 is a Tuesday, ISO week 1 of 2025.
    let d = NaiveDate::from_ymd_opt(2024, 12, 31).unwrap();
    assert_eq!(d.iso_week().year(), 2025);
    assert!(XNYS.is_trading_day(d));
}
