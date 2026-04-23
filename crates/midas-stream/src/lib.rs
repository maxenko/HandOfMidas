//! # midas-stream
//!
//! `BarStream` abstraction for the session-aware chart stack. Slice S3
//! of `plan/session-aware-charts/00-index.md`.
//!
//! ## Public surface
//!
//! - [`BarStream`] — minimum trait: `meta()` + `next()` + `snapshot()`.
//!   Live, history, fixture, and replay streams all implement it.
//! - [`SeekableBarStream`] — opt-in sub-trait for time-travelling streams
//!   (history, fixture). Live streams intentionally do NOT implement it
//!   (R2-NB-4).
//! - [`BarStreamMeta`] — symbol + calendar (`&'static dyn`) + period pinned
//!   at subscribe time.
//! - [`TimeRange`] — half-open `[from, to)` UTC range with a smart
//!   constructor.
//! - [`StreamError`] — unified error vocabulary.
//! - [`FixtureBarStream`] — in-memory vec replay, seekable.
//! - [`ChannelBarStream`] — thin `mpsc::Receiver<Candle>` wrapper, not
//!   seekable.
//! - [`HistoryThenLive`] — chains a seekable history stream into a live
//!   stream with seam dedup. Exposes `try_seek` instead of
//!   `SeekableBarStream` because seekability is dynamic.
//! - [`Filtered`] + [`FilterPolicy`] + [`EhFilter`] + [`SessionKindFilter`]
//!   — SessionKind-aware predicate combinator.
//! - [`Resampled`] — stub for a future resampling combinator (S7).
//!
//! Calendar is pinned at stream construction via `&'static dyn`. No
//! per-tick calendar lookup anywhere on the hot path.

#![forbid(unsafe_code)]

mod channel;
mod filter;
mod fixture;
mod history_then_live;
mod resample;

#[cfg(test)]
mod combinator_tests;

use async_trait::async_trait;

pub use midas_bars::{Candle, Symbol};
pub use midas_calendar::{
    BarPeriod, CalendarId, ExchangeCalendar, Session, SessionKind, Timestamp,
};

pub use crate::channel::ChannelBarStream;
pub use crate::filter::{EhFilter, FilterPolicy, Filtered, SessionKindFilter};
pub use crate::fixture::FixtureBarStream;
pub use crate::history_then_live::HistoryThenLive;
pub use crate::resample::Resampled;

// ---------------------------------------------------------------------------
// TimeRange
// ---------------------------------------------------------------------------

/// Half-open UTC range `[from, to)` used by `BarStream::snapshot`.
///
/// Construction is via the smart [`TimeRange::new`] which rejects
/// backward ranges. There is intentionally no public field-struct
/// constructor — an invariant-violating `TimeRange` must not be
/// representable.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TimeRange {
    from: Timestamp,
    to: Timestamp,
}

impl TimeRange {
    /// Build a [`TimeRange`]. Returns `None` if `from > to`. An empty
    /// range (`from == to`) is permitted — it simply matches no bars.
    #[inline]
    pub fn new(from: Timestamp, to: Timestamp) -> Option<Self> {
        if from > to {
            None
        } else {
            Some(Self { from, to })
        }
    }

    /// Build a [`TimeRange`] panicking on invalid input. Prefer
    /// [`TimeRange::new`] in production code; this is a convenience for
    /// fixtures and tests.
    #[inline]
    pub fn new_or_panic(from: Timestamp, to: Timestamp) -> Self {
        Self::new(from, to).expect("TimeRange::new_or_panic: from > to")
    }

    #[inline]
    pub fn from(&self) -> Timestamp {
        self.from
    }

    #[inline]
    pub fn to(&self) -> Timestamp {
        self.to
    }

    /// Half-open containment: `from <= ts < to`.
    #[inline]
    pub fn contains(&self, ts: Timestamp) -> bool {
        ts >= self.from && ts < self.to
    }

    /// `true` when `from == to`.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.from == self.to
    }
}

// ---------------------------------------------------------------------------
// StreamError
// ---------------------------------------------------------------------------

/// Error surface for the `BarStream` trait family.
///
/// `CoverageExceeded` is validated at construction (where the calendar is
/// known) rather than bubbled from `next()` — see `FixtureBarStream::new`.
#[derive(Debug, thiserror::Error)]
pub enum StreamError {
    /// Stream has been closed; no further candles will arrive.
    #[error("stream closed")]
    Closed,

    /// Stream does not support seek (live broadcast, for example).
    #[error("not seekable (live stream)")]
    NotSeekable,

    /// Target timestamp sits outside the stream's known range.
    #[error("timestamp {0} outside stream range")]
    OutOfRange(Timestamp),

    /// Upstream / provider emitted an error; opaque string payload.
    #[error("upstream error: {0}")]
    Upstream(String),

    /// Requested range exceeds the calendar's coverage window.
    /// Raised at construction by streams that validate coverage.
    #[error("range {range:?} exceeds coverage of calendar {calendar}")]
    CoverageExceeded {
        calendar: CalendarId,
        range: TimeRange,
    },
}

impl PartialEq for StreamError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Closed, Self::Closed) => true,
            (Self::NotSeekable, Self::NotSeekable) => true,
            (Self::OutOfRange(a), Self::OutOfRange(b)) => a == b,
            (Self::Upstream(a), Self::Upstream(b)) => a == b,
            (
                Self::CoverageExceeded {
                    calendar: ac,
                    range: ar,
                },
                Self::CoverageExceeded {
                    calendar: bc,
                    range: br,
                },
            ) => ac == bc && ar == br,
            _ => false,
        }
    }
}

// ---------------------------------------------------------------------------
// BarStreamMeta
// ---------------------------------------------------------------------------

/// Metadata pinned at stream construction. Calendar is `&'static dyn`
/// (R2-Nm-1) so every `next()` consumer has direct, allocation-free
/// access without a registry lookup.
#[derive(Clone)]
pub struct BarStreamMeta {
    pub symbol: Symbol,
    pub calendar: &'static dyn ExchangeCalendar,
    pub period: BarPeriod,
}

impl BarStreamMeta {
    #[inline]
    pub fn new(symbol: Symbol, calendar: &'static dyn ExchangeCalendar, period: BarPeriod) -> Self {
        Self {
            symbol,
            calendar,
            period,
        }
    }
}

impl std::fmt::Debug for BarStreamMeta {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BarStreamMeta")
            .field("symbol", &self.symbol)
            .field("calendar", &self.calendar.id())
            .field("period", &self.period)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// BarStream + SeekableBarStream
// ---------------------------------------------------------------------------

/// One stream of [`Candle`]s. Implementations decide whether to source
/// from cold storage, a live fan-out, a sim, a file, or an in-memory
/// fixture. `Send` is required; `Sync` is intentionally not — the
/// `snapshot` and `next` methods take `&mut self`, so streams are
/// single-consumer.
#[async_trait]
pub trait BarStream: Send {
    fn meta(&self) -> &BarStreamMeta;

    /// Pull the next candle, awaiting if none is yet available. Returns
    /// `None` when the stream has ended (EOF for history, disconnect
    /// for live).
    async fn next(&mut self) -> Option<Candle>;

    /// Return every candle whose `window.open` falls in `[range.from,
    /// range.to)`. Does NOT advance the stream cursor for replay-style
    /// streams. Live / non-seekable streams may return
    /// [`StreamError::NotSeekable`].
    async fn snapshot(&mut self, range: TimeRange) -> Result<Vec<Candle>, StreamError>;
}

/// Opt-in sub-trait for streams that support historical replay. Live
/// streams MUST NOT implement this — `HistoryThenLive` exposes a
/// `try_seek` method instead of implementing this trait because
/// seekability is dynamic.
#[async_trait]
pub trait SeekableBarStream: BarStream {
    /// Move the stream cursor so that the next call to `next()` yields
    /// the first candle whose `window.open >= to`. Target timestamps
    /// before the first candle are clamped to the start; target
    /// timestamps after the last candle leave the cursor at EOF.
    async fn seek(&mut self, to: Timestamp) -> Result<(), StreamError>;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn utc(y: i32, m: u32, d: u32, h: u32, min: u32) -> Timestamp {
        chrono::Utc.with_ymd_and_hms(y, m, d, h, min, 0).unwrap()
    }

    #[test]
    fn time_range_new_accepts_forward() {
        let a = utc(2024, 1, 17, 14, 30);
        let b = utc(2024, 1, 17, 20, 0);
        let r = TimeRange::new(a, b).unwrap();
        assert_eq!(r.from(), a);
        assert_eq!(r.to(), b);
    }

    #[test]
    fn time_range_new_rejects_backward() {
        let a = utc(2024, 1, 17, 14, 30);
        let b = utc(2024, 1, 17, 10, 0);
        assert!(TimeRange::new(a, b).is_none());
    }

    #[test]
    fn time_range_new_accepts_empty() {
        let a = utc(2024, 1, 17, 14, 30);
        let r = TimeRange::new(a, a).unwrap();
        assert!(r.is_empty());
    }

    #[test]
    fn time_range_contains_half_open() {
        let a = utc(2024, 1, 17, 14, 30);
        let b = utc(2024, 1, 17, 15, 0);
        let r = TimeRange::new(a, b).unwrap();
        assert!(r.contains(a));
        assert!(r.contains(utc(2024, 1, 17, 14, 45)));
        assert!(!r.contains(b));
    }

    #[test]
    fn stream_error_display_and_eq() {
        let e1 = StreamError::Closed;
        let e2 = StreamError::Closed;
        assert_eq!(e1, e2);
        assert_eq!(e1.to_string(), "stream closed");
        assert_ne!(
            StreamError::Upstream("a".into()),
            StreamError::Upstream("b".into())
        );
    }

    #[test]
    fn bar_stream_meta_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<BarStreamMeta>();
    }
}
