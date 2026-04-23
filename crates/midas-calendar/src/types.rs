//! Shared value types for the session-aware calendar stack.
//!
//! All types are cheap to clone / `Copy` where possible. `Session` is the
//! one exception — it carries an `Option<Cow<'static, str>>` label so
//! future user-defined session overlays can ship an owned `String` without
//! an API break. Static variants emit `Cow::Borrowed` and incur zero
//! allocation.

use std::borrow::Cow;

use chrono::NaiveDate;
use smallvec::SmallVec;

use crate::BarPeriod;

/// UTC wall-clock. The ONLY stored timestamp representation across the
/// session-aware chart stack. Exchange-tz conversion is a lens applied at
/// the edges (axis labels, calendar-internal computation).
pub type Timestamp = chrono::DateTime<chrono::Utc>;

/// MIC-ish identifier. Intentionally a newtype over `&'static str` so
/// construction is const-fn and identity comparisons are pointer-equal.
/// Examples: `CalendarId("XNYS")`, `CalendarId("CRYPTO")`.
#[derive(Copy, Clone, Debug, Hash, Eq, PartialEq)]
pub struct CalendarId(pub &'static str);

impl std::fmt::Display for CalendarId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

impl serde::Serialize for CalendarId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.0)
    }
}

impl<'de> serde::Deserialize<'de> for CalendarId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // CalendarId is `&'static str`. On deserialize, the string must
        // live for the rest of the process; we `Box::leak` it, which is
        // acceptable because calendar identifiers are a tiny, bounded set
        // (XNYS, CRYPTO, …) encountered at most during fixture replay /
        // wire decode. Deduplicate against known static ids first so
        // repeated decode of the same id doesn't leak each time.
        let s: String = <String as serde::Deserialize>::deserialize(deserializer)?;
        Ok(CalendarId::from_str_leak(&s))
    }
}

impl CalendarId {
    /// Intern `s` into a `&'static str` deterministically — known calendar
    /// ids (XNYS, CRYPTO) return the canonical pointer; unknown ids fall
    /// back to `Box::leak`. Used by `Deserialize` and by tests that build
    /// calendar ids from runtime strings.
    pub fn from_str_leak(s: &str) -> Self {
        match s {
            "XNYS" => CalendarId("XNYS"),
            "CRYPTO" => CalendarId("CRYPTO"),
            other => {
                let leaked: &'static str = Box::leak(other.to_owned().into_boxed_str());
                CalendarId(leaked)
            }
        }
    }
}

/// Kind of a session. Non-exhaustive so future asset classes (e.g. CME
/// maintenance windows, FX rollover breaks) can add variants without a
/// breaking change.
#[non_exhaustive]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[repr(u8)]
pub enum SessionKind {
    Regular,
    PreMarket,
    PostMarket,
    Break,
    Overnight,
    /// Sentinel for "no session covers this instant" — returned by
    /// `ExchangeCalendar::classify` for weekends, holidays, and any time
    /// outside the calendar's coverage range. Never errors.
    Closed,
}

/// A concrete session on a specific calendar. Produced only by calendar
/// methods; there is no public constructor. The `label` is a
/// `Cow<'static, str>` so static session names ("NY Regular") cost nothing
/// and future owned overlay labels work without a type change.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Session {
    calendar: CalendarId,
    kind: SessionKind,
    label: Option<Cow<'static, str>>,
    open: Timestamp,
    close: Timestamp,
}

impl Session {
    /// Crate-private constructor. Calendar impls are the only code
    /// permitted to synthesize a `Session`.
    pub(crate) fn new(
        calendar: CalendarId,
        kind: SessionKind,
        open: Timestamp,
        close: Timestamp,
    ) -> Self {
        Self {
            calendar,
            kind,
            label: None,
            open,
            close,
        }
    }

    /// Crate-private constructor with an explicit label. Retained for
    /// future user-defined session overlays (e.g. Tokyo/London/NY FX
    /// overlays); not exercised by the current XNYS / CryptoSpot impls.
    #[allow(dead_code)]
    pub(crate) fn with_label(
        calendar: CalendarId,
        kind: SessionKind,
        open: Timestamp,
        close: Timestamp,
        label: Cow<'static, str>,
    ) -> Self {
        Self {
            calendar,
            kind,
            label: Some(label),
            open,
            close,
        }
    }

    #[inline]
    pub fn calendar(&self) -> CalendarId {
        self.calendar
    }

    #[inline]
    pub fn kind(&self) -> SessionKind {
        self.kind
    }

    #[inline]
    pub fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }

    #[inline]
    pub fn open(&self) -> Timestamp {
        self.open
    }

    #[inline]
    pub fn close(&self) -> Timestamp {
        self.close
    }

    /// Half-open containment: `[open, close)`.
    #[inline]
    pub fn contains(&self, ts: Timestamp) -> bool {
        ts >= self.open && ts < self.close
    }
}

/// Day-level view. `sessions` is ordered by `open` and contains every
/// non-overlapping session for the date (pre-market, regular, post-market
/// for equities; a single 24h slot for crypto; multiple for futures with
/// maintenance breaks).
#[derive(Clone, Debug)]
pub struct TradingDay {
    pub date: NaiveDate,
    pub sessions: SmallVec<[Session; 4]>,
    pub is_early_close: bool,
    pub is_holiday: bool,
    pub holiday_name: Option<&'static str>,
}

/// Shared inline-session buffer type. Hoists the `[Session; 16]` inline
/// capacity into one place so changing the inline hint is a single-line
/// edit rather than a shotgun change across every consumer of
/// [`ExchangeCalendar::sessions_between`] and friends.
///
/// 16 inline elements absorb a typical one-week-or-less viewport on
/// equities (up to ~3 sessions/day × 5 trading days = 15) without
/// heap-allocating on the render hot path. Crypto viewports use at most
/// one entry per day; futures with maintenance-break sessions may use
/// two; multi-week overlays spill to the heap.
pub type SessionBuf = SmallVec<[Session; 16]>;

/// Window a bar sits within, together with the session it belongs to.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BarWindow {
    pub open: Timestamp,
    pub close: Timestamp,
    pub session: Session,
}

/// Policy driving time-axis compression.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TimeAxisPolicy {
    /// Crypto-style: every UTC pixel maps to a real second.
    Continuous,
    /// Equities/futures/FX: collapse closed time so consecutive sessions
    /// visually butt against one another.
    CompressedSessionBoundaries,
}

/// Errors surfaced by fallible calendar methods. `classify` never returns
/// `CalendarError`; it is infallible and saturating by design.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CalendarError {
    #[error("{0} out of calendar coverage")]
    OutOfRange(NaiveDate),
    #[error("unsupported period for {calendar}: {period:?}")]
    UnsupportedPeriod {
        calendar: CalendarId,
        period: BarPeriod,
    },
}
