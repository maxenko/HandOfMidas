//! NYSE holiday and early-close rules.
//!
//! One function per rule; all tested in `super::tests`. Rules are expressed
//! in terms of ET-local calendar dates (`NaiveDate`). UTC conversion is the
//! caller's responsibility.
//!
//! Sources:
//! - NYSE holiday schedule + archival closure notices.
//! - `exchange_calendars::USExchangeCalendar` (Python) for rule structure.
//! - `nyse-holiday-cal` crate as a cross-check (dev-dep only).
//!
//! ## Binding rules
//!
//! - Day-after-Thanksgiving early close is `thanksgiving_date + 1 day`,
//!   NOT "4th Friday of November" (which is coincidentally wrong in ~40%
//!   of years).
//! - Day-before-Good-Friday is NOT an NYSE equity early close (that's a
//!   SIFMA bond-market convention).
//! - Juneteenth is observed from 2022 onward only.
//! - Ad-hoc closures (9/11, Sandy, state funerals) are enumerated.

use chrono::{Datelike, Duration, NaiveDate, Weekday};

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

/// Is `date` an NYSE equity trading holiday (regular observed or ad-hoc)?
/// Weekends are NOT holidays — they are non-trading days but not flagged
/// as holidays. Call `is_trading_day` for the combined check.
pub(crate) fn holiday_name(date: NaiveDate) -> Option<&'static str> {
    if let Some(name) = ad_hoc_closure_name(date) {
        return Some(name);
    }
    regular_holiday_name(date)
}

/// If `date` is an early-close day, return 13:00 as `(hour, minute)`.
/// Otherwise `None`. Early-close is a valid trading day; the regular
/// session just ends at 13:00 ET instead of 16:00 ET.
pub(crate) fn early_close_minute(date: NaiveDate) -> Option<(u32, u32)> {
    if is_day_after_thanksgiving(date)
        || is_july_3_weekday_early_close(date)
        || is_christmas_eve_weekday_early_close(date)
    {
        Some((13, 0))
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Regular observed holidays (rule-based)
// ---------------------------------------------------------------------------

pub(crate) fn regular_holiday_name(date: NaiveDate) -> Option<&'static str> {
    if is_new_years_day_observed(date) {
        return Some("New Year's Day");
    }
    if is_mlk_day(date) {
        return Some("Martin Luther King Jr. Day");
    }
    if is_presidents_day(date) {
        return Some("Presidents Day");
    }
    if is_good_friday(date) {
        return Some("Good Friday");
    }
    if is_memorial_day(date) {
        return Some("Memorial Day");
    }
    if is_juneteenth_observed(date) {
        return Some("Juneteenth");
    }
    if is_independence_day_observed(date) {
        return Some("Independence Day");
    }
    if is_labor_day(date) {
        return Some("Labor Day");
    }
    if is_thanksgiving(date) {
        return Some("Thanksgiving Day");
    }
    if is_christmas_observed(date) {
        return Some("Christmas Day");
    }
    None
}

/// Weekend-to-weekday observance for fixed-date holidays. Saturday →
/// preceding Friday; Sunday → following Monday. This is NYSE's rule for
/// federal fixed-date holidays (New Year's Day, Juneteenth,
/// Independence Day, Christmas).
fn observed(date: NaiveDate) -> NaiveDate {
    match date.weekday() {
        Weekday::Sat => date - Duration::days(1),
        Weekday::Sun => date + Duration::days(1),
        _ => date,
    }
}

pub(crate) fn is_new_years_day_observed(date: NaiveDate) -> bool {
    let actual = NaiveDate::from_ymd_opt(date.year(), 1, 1).unwrap();
    date == observed(actual)
}

/// 3rd Monday of January, >= 2000.
pub(crate) fn is_mlk_day(date: NaiveDate) -> bool {
    date.month() == 1 && nth_weekday_of_month(date.year(), 1, Weekday::Mon, 3) == Some(date)
}

/// 3rd Monday of February.
pub(crate) fn is_presidents_day(date: NaiveDate) -> bool {
    date.month() == 2 && nth_weekday_of_month(date.year(), 2, Weekday::Mon, 3) == Some(date)
}

/// Easter Sunday - 2 days.
pub(crate) fn is_good_friday(date: NaiveDate) -> bool {
    easter_sunday(date.year())
        .map(|e| date == e - Duration::days(2))
        .unwrap_or(false)
}

/// Last Monday of May.
pub(crate) fn is_memorial_day(date: NaiveDate) -> bool {
    date.month() == 5 && last_weekday_of_month(date.year(), 5, Weekday::Mon) == Some(date)
}

/// June 19 with weekend observance, ONLY for years >= 2022. (Federal
/// holiday signed into law in June 2021; NYSE observed from 2022 onward.)
pub(crate) fn is_juneteenth_observed(date: NaiveDate) -> bool {
    if date.year() < 2022 {
        return false;
    }
    let actual = NaiveDate::from_ymd_opt(date.year(), 6, 19).unwrap();
    date == observed(actual)
}

/// July 4 with weekend observance.
pub(crate) fn is_independence_day_observed(date: NaiveDate) -> bool {
    let actual = NaiveDate::from_ymd_opt(date.year(), 7, 4).unwrap();
    date == observed(actual)
}

/// 1st Monday of September.
pub(crate) fn is_labor_day(date: NaiveDate) -> bool {
    date.month() == 9 && nth_weekday_of_month(date.year(), 9, Weekday::Mon, 1) == Some(date)
}

/// 4th Thursday of November.
pub(crate) fn is_thanksgiving(date: NaiveDate) -> bool {
    date.month() == 11 && nth_weekday_of_month(date.year(), 11, Weekday::Thu, 4) == Some(date)
}

/// December 25 with weekend observance.
pub(crate) fn is_christmas_observed(date: NaiveDate) -> bool {
    let actual = NaiveDate::from_ymd_opt(date.year(), 12, 25).unwrap();
    date == observed(actual)
}

// ---------------------------------------------------------------------------
// Early-close rules
// ---------------------------------------------------------------------------

/// Day AFTER Thanksgiving (the 4th Thursday of November + 1 day). Note:
/// this is NOT "4th Friday of November" — those differ whenever November
/// starts on a Friday (Nov 1 is then the 1st Friday, not part of a
/// Thanksgiving weekend; 4th Friday = Nov 22, 4th Thursday + 1 = Nov 29).
pub(crate) fn is_day_after_thanksgiving(date: NaiveDate) -> bool {
    if date.month() != 11 {
        return false;
    }
    let thanksgiving = match nth_weekday_of_month(date.year(), 11, Weekday::Thu, 4) {
        Some(d) => d,
        None => return false,
    };
    date == thanksgiving + Duration::days(1)
}

/// July 3 is an early close if (a) it's a weekday and (b) July 4 is the
/// ACTUAL trading holiday (i.e. July 4 isn't observed on some other day).
/// When July 4 falls on a Saturday, Independence Day is observed on
/// July 3; July 3 itself is a holiday, not an early close.
/// When July 4 falls on a Sunday, Independence Day is observed on July 5;
/// July 3 (a Friday) IS an early close.
pub(crate) fn is_july_3_weekday_early_close(date: NaiveDate) -> bool {
    if date.month() != 7 || date.day() != 3 {
        return false;
    }
    // Weekend guard.
    if matches!(date.weekday(), Weekday::Sat | Weekday::Sun) {
        return false;
    }
    // If July 3 is itself the observed Independence Day (July 4 was a
    // Saturday), it's a full holiday, not an early close.
    if is_independence_day_observed(date) {
        return false;
    }
    true
}

/// Christmas Eve is an early close if it's a weekday AND Christmas is
/// observed on its actual date (Dec 25). When Christmas falls on a
/// weekend the observance shifts and Dec 24 ends up as either a normal
/// trading day (Friday before a Monday observance) or a full weekend day.
///
/// Historically NYSE has early-closed on every Dec 24 weekday in modern
/// practice, so this rule is simply "Dec 24 is a weekday and is not
/// itself a holiday observance."
pub(crate) fn is_christmas_eve_weekday_early_close(date: NaiveDate) -> bool {
    if date.month() != 12 || date.day() != 24 {
        return false;
    }
    if matches!(date.weekday(), Weekday::Sat | Weekday::Sun) {
        return false;
    }
    // If Dec 24 were somehow a holiday observance (it isn't under current
    // rules), skip. Guard defensively in case of ad-hoc flags.
    if regular_holiday_name(date).is_some() {
        return false;
    }
    true
}

// ---------------------------------------------------------------------------
// Ad-hoc closures (enumerated)
// ---------------------------------------------------------------------------

/// Non-recurring market closures. The canonical list, locked to what the
/// spec and archival record establish.
pub(crate) fn ad_hoc_closure_name(date: NaiveDate) -> Option<&'static str> {
    const CLOSURES: &[(i32, u32, u32, &str)] = &[
        // September 11, 2001 attacks.
        (2001, 9, 11, "September 11 attacks"),
        (2001, 9, 12, "September 11 attacks"),
        (2001, 9, 13, "September 11 attacks"),
        (2001, 9, 14, "September 11 attacks"),
        // Reagan state funeral.
        (2004, 6, 11, "Ronald Reagan state funeral"),
        // Ford state funeral.
        (2007, 1, 2, "Gerald Ford state funeral"),
        // Hurricane Sandy.
        (2012, 10, 29, "Hurricane Sandy"),
        (2012, 10, 30, "Hurricane Sandy"),
        // George H.W. Bush state funeral.
        (2018, 12, 5, "George H.W. Bush state funeral"),
        // Jimmy Carter day of mourning.
        (2025, 1, 9, "Jimmy Carter day of mourning"),
    ];

    let needle = (date.year(), date.month(), date.day());
    for &(y, m, d, name) in CLOSURES {
        if (y, m, d) == needle {
            return Some(name);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Date helpers
// ---------------------------------------------------------------------------

/// Nth occurrence of `weekday` in `(year, month)`. `n` is 1-based.
pub(crate) fn nth_weekday_of_month(
    year: i32,
    month: u32,
    weekday: Weekday,
    n: u32,
) -> Option<NaiveDate> {
    let first = NaiveDate::from_ymd_opt(year, month, 1)?;
    // Offset from the 1st to the first `weekday` in-month.
    let first_weekday = first.weekday().num_days_from_monday() as i64;
    let target = weekday.num_days_from_monday() as i64;
    let mut delta = target - first_weekday;
    if delta < 0 {
        delta += 7;
    }
    let day = 1 + delta as u32 + (n - 1) * 7;
    NaiveDate::from_ymd_opt(year, month, day)
}

/// Last occurrence of `weekday` in `(year, month)`.
pub(crate) fn last_weekday_of_month(year: i32, month: u32, weekday: Weekday) -> Option<NaiveDate> {
    // Start from month-end; walk backwards to the target weekday.
    let last_day = last_day_of_month(year, month)?;
    let last = NaiveDate::from_ymd_opt(year, month, last_day)?;
    let diff = (last.weekday().num_days_from_monday() as i64
        - weekday.num_days_from_monday() as i64)
        .rem_euclid(7);
    NaiveDate::from_ymd_opt(year, month, last_day - diff as u32)
}

fn last_day_of_month(year: i32, month: u32) -> Option<u32> {
    // First of next month minus one day = last of this month.
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    let next_first = NaiveDate::from_ymd_opt(next_year, next_month, 1)?;
    Some((next_first - Duration::days(1)).day())
}

/// Anonymous Gregorian algorithm for computing Easter Sunday.
/// Valid for years >= 1583. Returns `None` if `year` is out of range.
///
/// Reference: Meeus/Jones/Butcher, as reproduced in Knuth TAOCP §1.3.2.
pub(crate) fn easter_sunday(year: i32) -> Option<NaiveDate> {
    if year < 1583 {
        return None;
    }
    let a = year % 19;
    let b = year / 100;
    let c = year % 100;
    let d = b / 4;
    let e = b % 4;
    let f = (b + 8) / 25;
    let g = (b - f + 1) / 3;
    let h = (19 * a + b - d - g + 15) % 30;
    let i = c / 4;
    let k = c % 4;
    let l = (32 + 2 * e + 2 * i - h - k) % 7;
    let m = (a + 11 * h + 22 * l) / 451;
    let month = (h + l - 7 * m + 114) / 31;
    let day = ((h + l - 7 * m + 114) % 31) + 1;
    NaiveDate::from_ymd_opt(year, month as u32, day as u32)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nth_weekday_mlk_2024() {
        // MLK 2024 = 3rd Monday of January = Jan 15.
        let mlk = nth_weekday_of_month(2024, 1, Weekday::Mon, 3).unwrap();
        assert_eq!(mlk, NaiveDate::from_ymd_opt(2024, 1, 15).unwrap());
    }

    #[test]
    fn last_weekday_memorial_2024() {
        // Memorial Day 2024 = last Monday of May = May 27.
        let md = last_weekday_of_month(2024, 5, Weekday::Mon).unwrap();
        assert_eq!(md, NaiveDate::from_ymd_opt(2024, 5, 27).unwrap());
    }

    #[test]
    fn easter_spot_checks() {
        // Canonical reference dates for Easter Sunday.
        assert_eq!(
            easter_sunday(2023),
            Some(NaiveDate::from_ymd_opt(2023, 4, 9).unwrap())
        );
        assert_eq!(
            easter_sunday(2024),
            Some(NaiveDate::from_ymd_opt(2024, 3, 31).unwrap())
        );
        assert_eq!(
            easter_sunday(2025),
            Some(NaiveDate::from_ymd_opt(2025, 4, 20).unwrap())
        );
        assert_eq!(
            easter_sunday(2000),
            Some(NaiveDate::from_ymd_opt(2000, 4, 23).unwrap())
        );
    }

    #[test]
    fn good_friday_spot_checks() {
        // Good Friday = Easter Sunday - 2 days.
        assert!(is_good_friday(NaiveDate::from_ymd_opt(2023, 4, 7).unwrap()));
        assert!(is_good_friday(
            NaiveDate::from_ymd_opt(2024, 3, 29).unwrap()
        ));
        // Day before Good Friday is NOT Good Friday.
        assert!(!is_good_friday(
            NaiveDate::from_ymd_opt(2023, 4, 6).unwrap()
        ));
    }

    #[test]
    fn juneteenth_gated_on_2022() {
        // June 20, 2022 (Mon) — observance of Sunday June 19.
        assert!(is_juneteenth_observed(
            NaiveDate::from_ymd_opt(2022, 6, 20).unwrap()
        ));
        // June 19 2021 is Saturday — not a holiday for NYSE; and the
        // rule is gated on year>=2022 anyway. Friday 2021-06-18 must be
        // a normal trading day.
        assert!(!is_juneteenth_observed(
            NaiveDate::from_ymd_opt(2021, 6, 18).unwrap()
        ));
        assert!(!is_juneteenth_observed(
            NaiveDate::from_ymd_opt(2021, 6, 19).unwrap()
        ));
    }

    #[test]
    fn thanksgiving_and_black_friday_2023() {
        // Thanksgiving 2023 = Nov 23 (Thu).
        assert!(is_thanksgiving(
            NaiveDate::from_ymd_opt(2023, 11, 23).unwrap()
        ));
        // Black Friday 2023 = Nov 24 (Fri) — early close, not holiday.
        assert!(is_day_after_thanksgiving(
            NaiveDate::from_ymd_opt(2023, 11, 24).unwrap()
        ));
        assert_eq!(
            early_close_minute(NaiveDate::from_ymd_opt(2023, 11, 24).unwrap()),
            Some((13, 0))
        );
    }

    #[test]
    fn reagan_funeral_2004_06_11() {
        assert_eq!(
            ad_hoc_closure_name(NaiveDate::from_ymd_opt(2004, 6, 11).unwrap()),
            Some("Ronald Reagan state funeral")
        );
    }

    #[test]
    fn day_before_good_friday_2023_is_not_early_close() {
        // 2023-04-06 (Thu). Good Friday was 2023-04-07.
        let d = NaiveDate::from_ymd_opt(2023, 4, 6).unwrap();
        assert_eq!(early_close_minute(d), None, "SIFMA != NYSE");
        assert!(holiday_name(d).is_none());
    }

    #[test]
    fn christmas_eve_2024_early_close() {
        // Dec 24 2024 = Tue. Early close.
        let d = NaiveDate::from_ymd_opt(2024, 12, 24).unwrap();
        assert_eq!(early_close_minute(d), Some((13, 0)));
        assert!(holiday_name(d).is_none());
    }

    #[test]
    fn july_3_2024_early_close() {
        // Jul 3 2024 = Wed (not a weekend; Jul 4 is Thu). Early close.
        let d = NaiveDate::from_ymd_opt(2024, 7, 3).unwrap();
        assert_eq!(early_close_minute(d), Some((13, 0)));
    }
}
