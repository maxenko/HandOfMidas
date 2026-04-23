//! Calendar-scoped bar periods.
//!
//! A `BarPeriod` encodes WHAT kind of calendar semantics a bar needs.
//! Clock intervals are UTC-epoch-modular (same meaning on any calendar).
//! Session-scoped periods depend on the calendar's reckoning of regular /
//! extended / electronic sessions. Calendar-scoped periods use the
//! calendar's rollup (ISO week, month, quarter, year).
//!
//! Validity of a (calendar, period) pairing is enforced at
//! `ExchangeCalendar::validate_period` time — see `00a-ideal-design.md`
//! §"Calendar × Period compatibility matrix".

/// Discriminated union of bar periods. Construction goes through the
/// smart constructors below; no arbitrary `(ClockInterval, SessionScope)`
/// combinations are representable.
///
/// `#[non_exhaustive]` so that downstream `match` sites keep a fallback
/// arm — adding a new period variant here does NOT break consumer
/// crates that already match on the existing variants.
#[non_exhaustive]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum BarPeriod {
    /// Clock-interval bar. Calendar-agnostic in window math (aggregators
    /// must still close a bar at session boundaries, but the *window*
    /// computation is modular over UTC epoch seconds).
    Clock(ClockInterval),

    /// Session-scoped bar. One bar per session span on the owning
    /// calendar — e.g. `Session(Regular)` on XNYS is 09:30–16:00 ET,
    /// on CryptoSpot is 00:00–24:00 UTC.
    Session(SessionSpan),

    /// Calendar-scoped bar: ISO week, month, quarter, year.
    Calendar(CalendarSpan),
}

/// `#[non_exhaustive]` so adding a new clock family (e.g. `Days(u32)`)
/// never becomes a semver-breaking change for downstream `match`
/// consumers.
#[non_exhaustive]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ClockInterval {
    Seconds(u32),
    Minutes(u32),
    Hours(u32),
}

/// `#[non_exhaustive]` so adding a new session span (e.g. a custom FX
/// overlay tag) is a minor-version change, not a break.
#[non_exhaustive]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum SessionSpan {
    /// Regular-hours session (RTH on equities).
    Regular,
    /// Pre + regular + post as one bar (ETH on equities). On calendars
    /// with no distinct extended-hours session (CryptoSpot) this aliases
    /// `Regular`.
    Extended,
    /// Futures electronic session. Reserved for calendars with a true
    /// electronic/pit split (XCME). Rejected by CryptoSpot and XNYS.
    Eth,
}

/// `#[non_exhaustive]` so adding a new calendar span (e.g. `Decade`,
/// `FiscalQuarter`) is a minor-version change for downstream callers.
#[non_exhaustive]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum CalendarSpan {
    Week,
    Month,
    Quarter,
    Year,
}

impl BarPeriod {
    #[inline]
    pub fn m1() -> Self {
        Self::Clock(ClockInterval::Minutes(1))
    }

    #[inline]
    pub fn m5() -> Self {
        Self::Clock(ClockInterval::Minutes(5))
    }

    #[inline]
    pub fn h1() -> Self {
        Self::Clock(ClockInterval::Hours(1))
    }

    /// Daily, regular-hours (e.g. XNYS 09:30–16:00 ET).
    #[inline]
    pub fn d1_rth() -> Self {
        Self::Session(SessionSpan::Regular)
    }

    /// Daily, extended-hours (e.g. XNYS 04:00–20:00 ET).
    #[inline]
    pub fn d1_eth() -> Self {
        Self::Session(SessionSpan::Extended)
    }

    #[inline]
    pub fn w1() -> Self {
        Self::Calendar(CalendarSpan::Week)
    }

    #[inline]
    pub fn mn1() -> Self {
        Self::Calendar(CalendarSpan::Month)
    }
}
