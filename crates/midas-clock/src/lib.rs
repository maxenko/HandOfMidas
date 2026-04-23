//! First-class `Clock` abstraction.
//!
//! Every `chrono::Utc::now()` and `std::time::Instant::now()` call in the
//! session-aware chart stack routes through this trait. Tests inject a
//! [`MockClock`]; prod injects a [`SystemClock`].
//!
//! ## Why both wall and monotonic?
//!
//! Bar timestamps and calendar classification need wall-clock UTC
//! (`chrono::DateTime<chrono::Utc>`). Timeouts, pacing, toast expirations,
//! and idle detection need monotonic time (`std::time::Instant`) so they
//! survive wall-clock adjustments (NTP step, DST). The trait exposes both
//! on one object so consumers don't juggle two injection points.
//!
//! ## `MockClock` + `tokio::time::pause()`
//!
//! `tokio::time::pause()` freezes tokio's internal timers but does NOT
//! affect `chrono::Utc::now()` nor `std::time::Instant::now()`. A test
//! that uses either of those directly breaks under paused time. The fix
//! is: every call goes through a `Clock` trait object, and in tests
//! that trait object is a [`MockClock`] whose [`MockClock::advance_by`]
//! also calls [`tokio::time::advance`] so tokio timers advance in lockstep.
//!
//! ## Ownership
//!
//! Clocks are runtime-swappable for tests and per-test isolation, so
//! consumers take `Arc<dyn Clock>` at construction. Calendars, by contrast,
//! are process-global singletons stored as `&'static dyn ExchangeCalendar`.

use std::sync::Arc;
use std::time::Instant;

#[cfg(feature = "mock_clock")]
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
#[cfg(feature = "mock_clock")]
use std::time::Duration;

pub type Timestamp = chrono::DateTime<chrono::Utc>;

/// Wall-clock + monotonic abstraction. Every time-dependent module in the
/// session-aware chart stack takes `Arc<dyn Clock>` at construction.
pub trait Clock: Send + Sync + 'static {
    /// Wall-clock (UTC). Replaces `chrono::Utc::now()`.
    fn now(&self) -> Timestamp;

    /// Monotonic. Replaces `std::time::Instant::now()`. Guaranteed
    /// non-decreasing across calls on the same `Clock` instance.
    fn now_monotonic(&self) -> Instant;
}

/// Convenience `impl Clock for Arc<dyn Clock>` so consumers can hold either
/// a concrete clock or a type-erased one without a wrapping layer.
impl Clock for Arc<dyn Clock> {
    fn now(&self) -> Timestamp {
        (**self).now()
    }
    fn now_monotonic(&self) -> Instant {
        (**self).now_monotonic()
    }
}

// ---------------------------------------------------------------------------
// SystemClock — production default.
// ---------------------------------------------------------------------------

/// Thin wrapper over `chrono::Utc::now()` + `std::time::Instant::now()`.
#[derive(Copy, Clone, Debug, Default)]
pub struct SystemClock;

impl SystemClock {
    pub fn new() -> Self {
        Self
    }

    pub fn shared() -> Arc<dyn Clock> {
        Arc::new(Self)
    }
}

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        chrono::Utc::now()
    }

    fn now_monotonic(&self) -> Instant {
        Instant::now()
    }
}

// ---------------------------------------------------------------------------
// MockClock — test-only.
// ---------------------------------------------------------------------------

/// Deterministic clock for tests. Wall time is epoch-nanoseconds (`i64`)
/// to match the full resolution of `chrono::DateTime<chrono::Utc>`.
/// Monotonic time is nanoseconds since `MockClock::new()`.
///
/// Use [`advance_by`] / [`advance_to`] to move the clock. The
/// [`advance_by`] method is `async` and calls [`tokio::time::advance`]
/// internally, so tests that run under `#[tokio::test(start_paused = true)]`
/// see wall-clock, monotonic, and tokio-timer time advance in lockstep.
///
/// Tests that don't drive tokio timers can use the synchronous
/// [`advance_by_sync`] / [`advance_to_sync`] variants.
///
/// [`advance_by`]: MockClock::advance_by
/// [`advance_to`]: MockClock::advance_to
/// [`advance_by_sync`]: MockClock::advance_by_sync
/// [`advance_to_sync`]: MockClock::advance_to_sync
#[cfg(feature = "mock_clock")]
#[derive(Debug)]
pub struct MockClock {
    wall_ns: AtomicI64, // epoch-nanos
    mono_ns: AtomicU64, // nanos since `base_instant`
    base_instant: Instant,
}

#[cfg(feature = "mock_clock")]
impl MockClock {
    /// Construct a new `MockClock` anchored at `start` (wall) and at the
    /// current `Instant::now()` (monotonic).
    pub fn new(start: Timestamp) -> Self {
        let wall_ns = start
            .timestamp_nanos_opt()
            .expect("MockClock start timestamp out of i64 range");
        Self {
            wall_ns: AtomicI64::new(wall_ns),
            mono_ns: AtomicU64::new(0),
            base_instant: Instant::now(),
        }
    }

    /// Convenience: wrap in `Arc<dyn Clock>` for downstream consumers.
    pub fn shared(start: Timestamp) -> Arc<Self> {
        Arc::new(Self::new(start))
    }

    /// Synchronous wall+monotonic advance. Does NOT touch `tokio::time`.
    /// Use when tests don't rely on `tokio::time::sleep`/`Interval`.
    ///
    /// Bug-hunt M1: `dur.as_nanos()` is a `u128`; the pre-fix code
    /// used `as u64` which silently truncates for absurdly-large
    /// durations. The monotonic arm now matches the wall-clock arm's
    /// `try_from` pattern — a duration that overflows the storage
    /// type panics with a clear message rather than wrapping.
    pub fn advance_by_sync(&self, dur: Duration) {
        let ns = i64::try_from(dur.as_nanos()).expect("advance_by duration exceeds i64 nanos");
        let ns_u64 = u64::try_from(dur.as_nanos()).expect("advance_by duration exceeds u64 nanos");
        self.wall_ns.fetch_add(ns, Ordering::SeqCst);
        self.mono_ns.fetch_add(ns_u64, Ordering::SeqCst);
    }

    /// Synchronous absolute set. Panics if `to` is before the current wall.
    pub fn advance_to_sync(&self, to: Timestamp) {
        let target = to
            .timestamp_nanos_opt()
            .expect("MockClock advance_to target out of i64 range");
        let prev = self.wall_ns.load(Ordering::SeqCst);
        assert!(
            target >= prev,
            "MockClock::advance_to cannot rewind: prev={prev}ns target={target}ns"
        );
        let dur = Duration::from_nanos((target - prev) as u64);
        self.advance_by_sync(dur);
    }

    /// Async advance: updates wall + monotonic AND calls
    /// `tokio::time::advance(dur)` so tokio timers see the same elapsed
    /// time. Requires the test to be under `#[tokio::test(start_paused = true)]`
    /// or to have called `tokio::time::pause()` before use.
    pub async fn advance_by(&self, dur: Duration) {
        self.advance_by_sync(dur);
        tokio::time::advance(dur).await;
    }

    /// Async absolute set. See [`advance_to_sync`] for the sync variant.
    ///
    /// [`advance_to_sync`]: MockClock::advance_to_sync
    pub async fn advance_to(&self, to: Timestamp) {
        let target = to
            .timestamp_nanos_opt()
            .expect("MockClock advance_to target out of i64 range");
        let prev = self.wall_ns.load(Ordering::SeqCst);
        assert!(
            target >= prev,
            "MockClock::advance_to cannot rewind: prev={prev}ns target={target}ns"
        );
        let dur = Duration::from_nanos((target - prev) as u64);
        self.advance_by(dur).await;
    }
}

#[cfg(feature = "mock_clock")]
impl Clock for MockClock {
    fn now(&self) -> Timestamp {
        let ns = self.wall_ns.load(Ordering::SeqCst);
        chrono::DateTime::<chrono::Utc>::from_timestamp_nanos(ns)
    }

    fn now_monotonic(&self) -> Instant {
        let ns = self.mono_ns.load(Ordering::SeqCst);
        self.base_instant + Duration::from_nanos(ns)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_clock_now_is_monotonic_across_calls() {
        let c = SystemClock::new();
        let a = c.now_monotonic();
        let b = c.now_monotonic();
        assert!(b >= a, "monotonic clock must not go backwards");
    }

    #[test]
    fn system_clock_wall_returns_current_utc() {
        let c = SystemClock::new();
        let before = chrono::Utc::now();
        let got = c.now();
        let after = chrono::Utc::now();
        assert!(got >= before);
        assert!(got <= after);
    }

    #[cfg(feature = "mock_clock")]
    #[test]
    fn mock_clock_wall_starts_at_anchor() {
        let anchor = chrono::DateTime::<chrono::Utc>::from_timestamp(1_700_000_000, 0).unwrap();
        let c = MockClock::new(anchor);
        assert_eq!(c.now(), anchor);
    }

    #[cfg(feature = "mock_clock")]
    #[test]
    fn mock_clock_advance_by_sync_moves_both_clocks() {
        let anchor = chrono::DateTime::<chrono::Utc>::from_timestamp(1_700_000_000, 0).unwrap();
        let c = MockClock::new(anchor);
        let mono_before = c.now_monotonic();
        c.advance_by_sync(Duration::from_secs(5));
        let mono_after = c.now_monotonic();
        assert_eq!(c.now(), anchor + chrono::Duration::seconds(5));
        assert_eq!(
            mono_after.duration_since(mono_before),
            Duration::from_secs(5)
        );
    }

    #[cfg(feature = "mock_clock")]
    #[test]
    fn mock_clock_advance_to_sync_moves_both_clocks() {
        let anchor = chrono::DateTime::<chrono::Utc>::from_timestamp(1_700_000_000, 0).unwrap();
        let c = MockClock::new(anchor);
        let target = anchor + chrono::Duration::seconds(42);
        c.advance_to_sync(target);
        assert_eq!(c.now(), target);
    }

    #[cfg(feature = "mock_clock")]
    #[test]
    #[should_panic(expected = "cannot rewind")]
    fn mock_clock_advance_to_sync_rejects_backwards() {
        let anchor = chrono::DateTime::<chrono::Utc>::from_timestamp(1_700_000_000, 0).unwrap();
        let c = MockClock::new(anchor);
        c.advance_to_sync(anchor - chrono::Duration::seconds(1));
    }

    #[cfg(feature = "mock_clock")]
    #[tokio::test(start_paused = true)]
    async fn mock_clock_advance_by_drives_tokio_timers() {
        let anchor = chrono::DateTime::<chrono::Utc>::from_timestamp(1_700_000_000, 0).unwrap();
        let c = MockClock::new(anchor);

        let start = tokio::time::Instant::now();
        // Spawn a tokio timer; it should fire when MockClock::advance_by
        // propagates to tokio::time.
        let handle = tokio::spawn(async {
            tokio::time::sleep(Duration::from_secs(3)).await;
        });

        c.advance_by(Duration::from_secs(3)).await;
        handle.await.unwrap();
        let elapsed = tokio::time::Instant::now().duration_since(start);
        assert!(
            elapsed >= Duration::from_secs(3),
            "tokio timer did not advance"
        );
        assert_eq!(c.now(), anchor + chrono::Duration::seconds(3));
    }

    #[cfg(feature = "mock_clock")]
    #[test]
    fn mock_clock_now_monotonic_is_stable_without_advance() {
        let anchor = chrono::DateTime::<chrono::Utc>::from_timestamp(1_700_000_000, 0).unwrap();
        let c = MockClock::new(anchor);
        let a = c.now_monotonic();
        let b = c.now_monotonic();
        assert_eq!(
            a, b,
            "MockClock monotonic must not advance without advance_by"
        );
    }

    #[cfg(feature = "mock_clock")]
    #[test]
    fn mock_clock_as_arc_dyn_clock() {
        // Verify the `Arc<dyn Clock>` impl via the convenience blanket.
        let anchor = chrono::DateTime::<chrono::Utc>::from_timestamp(1_700_000_000, 0).unwrap();
        let c: Arc<dyn Clock> = Arc::new(MockClock::new(anchor));
        let _ = c.now();
        let _ = c.now_monotonic();
    }
}
