//! Deterministic clock abstraction — Stage 08.
//!
//! Three implementors live behind the [`Clock`] trait:
//! - [`RealClock`]   — thin wrapper over `tokio::time::Instant`; production
//!   default. Use in the shipped `midas-ib-sim-server`, dev-loops, and
//!   demos that run alongside a human-interactive UI.
//! - [`VirtualClock`] — test-only. Time only advances when [`VirtualClock::advance`]
//!   (or [`VirtualClock::advance_to_next_event`]) is called. Use in every
//!   unit / integration test that schedules future events.
//! - [`AcceleratedClock`] — wraps a [`RealClock`] with a constant multiplier.
//!   Useful for demos (`"replay a 9:30–10:00 session in 3 minutes"`) and for
//!   running long scenarios in bounded real time.
//!
//! Engine code only sees the trait, so the same handler bodies run under
//! every clock mode without `#[cfg]` branches.
//!
//! ### Wall-clock mapping
//!
//! [`VirtualInstant`] is measured as a [`Duration`] since session start.
//! For scenarios anchored to calendar time, pair it with a [`SessionAnchor`]
//! (see bottom of this module) or call [`VirtualInstant::as_wall_time`]
//! directly.
//!
//! ### `tokio::time::pause` interop
//!
//! [`VirtualClock`] is its own time source — it does **not** drive
//! `tokio::time::sleep`, `tokio::time::Interval`, or anything else Tokio
//! brings. Tests that need both virtual clock control **and** Tokio-time
//! interop (e.g., awaiting a `tokio::time::timeout` while advancing our
//! clock) should combine [`VirtualClock`] with
//! `#[tokio::test(start_paused = true)]` and call `tokio::time::advance`
//! alongside [`VirtualClock::advance`]. Stage 08 does not provide a
//! wrapper that keeps the two clocks automatically in sync — that's
//! deferred to Stage 09 if integration tests need it.

use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::time::Instant;

// ---------------------------------------------------------------------------
// VirtualInstant — time since session start.
// ---------------------------------------------------------------------------

/// A point in virtual time, measured as a `Duration` since the session anchor
/// (sim startup under `RealClock`, test-harness `t=0` under `VirtualClock`).
///
/// Opaque wrapper on purpose: prevents accidental mixing with
/// `std::time::Instant` (wall clock) in the sim's hot path.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VirtualInstant(pub(crate) Duration);

impl VirtualInstant {
    pub const ZERO: Self = VirtualInstant(Duration::ZERO);

    pub fn from_millis(ms: u64) -> Self {
        Self(Duration::from_millis(ms))
    }

    pub fn from_secs(secs: u64) -> Self {
        Self(Duration::from_secs(secs))
    }

    pub fn from_duration(d: Duration) -> Self {
        Self(d)
    }

    pub fn as_duration(self) -> Duration {
        self.0
    }

    pub fn saturating_sub(self, other: Self) -> Duration {
        self.0.saturating_sub(other.0)
    }

    pub fn saturating_add(self, d: Duration) -> Self {
        Self(self.0.saturating_add(d))
    }

    /// Map this virtual instant onto wall-clock time given a session anchor.
    pub fn as_wall_time(
        self,
        epoch: chrono::DateTime<chrono::Utc>,
    ) -> chrono::DateTime<chrono::Utc> {
        epoch + chrono::Duration::from_std(self.0).unwrap_or(chrono::Duration::zero())
    }
}

impl PartialOrd for VirtualInstant {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for VirtualInstant {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.cmp(&other.0)
    }
}

impl fmt::Display for VirtualInstant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.0)
    }
}

// ---------------------------------------------------------------------------
// ClockMode + Clock trait
// ---------------------------------------------------------------------------

#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ClockMode {
    Real,
    Accelerated(f64),
    Virtual,
}

/// Time abstraction used by every time-dependent module in the sim.
#[async_trait]
pub trait Clock: Send + Sync + 'static {
    fn now(&self) -> VirtualInstant;
    async fn sleep_until(&self, deadline: VirtualInstant);
    async fn sleep(&self, duration: Duration);
    fn mode(&self) -> ClockMode;
}

// ---------------------------------------------------------------------------
// RealClock — the production default.
// ---------------------------------------------------------------------------

/// Wall-clock implementation. Anchored at `RealClock::new()` so `now()` returns
/// elapsed time since startup (matching `VirtualInstant`'s "since session start"
/// semantics).
#[derive(Clone, Debug)]
pub struct RealClock {
    anchor: Instant,
}

impl RealClock {
    pub fn new() -> Self {
        Self {
            anchor: Instant::now(),
        }
    }

    pub fn shared() -> Arc<Self> {
        Arc::new(Self::new())
    }

    /// Expose the wall-clock anchor. Used by `AcceleratedClock` for its
    /// `sleep_until` translation.
    pub(crate) fn anchor(&self) -> Instant {
        self.anchor
    }
}

impl Default for RealClock {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Clock for RealClock {
    fn now(&self) -> VirtualInstant {
        VirtualInstant(self.anchor.elapsed())
    }

    async fn sleep_until(&self, deadline: VirtualInstant) {
        // Map virtual -> wall: anchor + deadline.
        let wall_deadline = self.anchor + deadline.0;
        tokio::time::sleep_until(wall_deadline).await;
    }

    async fn sleep(&self, duration: Duration) {
        tokio::time::sleep(duration).await;
    }

    fn mode(&self) -> ClockMode {
        ClockMode::Real
    }
}

// ---------------------------------------------------------------------------
// AcceleratedClock — RealClock * multiplier.
// ---------------------------------------------------------------------------

/// Wraps a [`RealClock`] with a time multiplier. A `multiplier` of `10.0`
/// means virtual time advances 10× faster than wall time: a 30-minute session
/// finishes in 3 wall-clock minutes.
///
/// Useful for demos and for bounded-time replay of long scenarios.
///
/// `multiplier` must be strictly positive; constructors panic otherwise.
#[derive(Clone, Debug)]
pub struct AcceleratedClock {
    base: RealClock,
    multiplier: f64,
}

impl AcceleratedClock {
    /// Create an [`AcceleratedClock`] anchored to `base` with the given
    /// time multiplier.
    ///
    /// # Panics
    ///
    /// Panics if `multiplier` is not finite or not strictly positive.
    pub fn new(base: RealClock, multiplier: f64) -> Self {
        assert!(
            multiplier.is_finite() && multiplier > 0.0,
            "AcceleratedClock multiplier must be finite and > 0 (got {multiplier})"
        );
        Self { base, multiplier }
    }

    /// Convenience: fresh [`RealClock`] + multiplier.
    pub fn with_multiplier(multiplier: f64) -> Self {
        Self::new(RealClock::new(), multiplier)
    }

    pub fn shared(base: RealClock, multiplier: f64) -> Arc<Self> {
        Arc::new(Self::new(base, multiplier))
    }

    pub fn multiplier(&self) -> f64 {
        self.multiplier
    }
}

#[async_trait]
impl Clock for AcceleratedClock {
    fn now(&self) -> VirtualInstant {
        let real_elapsed = self.base.now().0;
        VirtualInstant(real_elapsed.mul_f64(self.multiplier))
    }

    async fn sleep_until(&self, deadline: VirtualInstant) {
        // virtual_deadline / multiplier = wall deadline offset.
        let wall_offset = deadline.0.as_secs_f64() / self.multiplier;
        let wall_deadline = self.base.anchor() + Duration::from_secs_f64(wall_offset.max(0.0));
        tokio::time::sleep_until(wall_deadline).await;
    }

    async fn sleep(&self, duration: Duration) {
        // A `duration` in virtual time is `duration / multiplier` in wall time.
        let wall = Duration::from_secs_f64(duration.as_secs_f64() / self.multiplier);
        tokio::time::sleep(wall).await;
    }

    fn mode(&self) -> ClockMode {
        ClockMode::Accelerated(self.multiplier)
    }
}

// ---------------------------------------------------------------------------
// VirtualClock — deterministic, test-only.
// ---------------------------------------------------------------------------

/// A waiter parked in [`VirtualClock`]. Deadline is the primary key; `seq`
/// is a monotonic tie-breaker so that waiters registered at the same
/// virtual instant fire in the order they were registered. Mirrors the
/// `(deadline, seq)` ordering in [`crate::engine::scheduler::EventScheduler`]
/// and so preserves determinism across all components of the sim.
struct Waiter {
    deadline: VirtualInstant,
    seq: u64,
    waker: tokio::sync::oneshot::Sender<()>,
}

impl PartialEq for Waiter {
    fn eq(&self, other: &Self) -> bool {
        self.deadline == other.deadline && self.seq == other.seq
    }
}

impl Eq for Waiter {}

impl PartialOrd for Waiter {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Waiter {
    fn cmp(&self, other: &Self) -> Ordering {
        // Earlier deadline wins; `seq` tie-breaks in insertion order.
        self.deadline
            .cmp(&other.deadline)
            .then_with(|| self.seq.cmp(&other.seq))
    }
}

struct VirtualClockState {
    now: VirtualInstant,
    waiters: BinaryHeap<std::cmp::Reverse<Waiter>>,
    next_seq: u64,
}

/// Deterministic, test-only clock. Time only advances when
/// [`VirtualClock::advance`] (or [`VirtualClock::advance_to_next_event`])
/// is called by the test driver.
///
/// Shared across tasks via `Arc<VirtualClock>` or, equivalently, by `Clone`
/// (the type is a thin `Arc` wrapper).
#[derive(Clone)]
pub struct VirtualClock {
    state: Arc<Mutex<VirtualClockState>>,
}

impl fmt::Debug for VirtualClock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.state.lock().expect("VirtualClock mutex poisoned");
        f.debug_struct("VirtualClock")
            .field("now", &state.now)
            .field("waiters", &state.waiters.len())
            .finish()
    }
}

impl Default for VirtualClock {
    fn default() -> Self {
        Self::new()
    }
}

impl VirtualClock {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(VirtualClockState {
                now: VirtualInstant::ZERO,
                waiters: BinaryHeap::new(),
                next_seq: 0,
            })),
        }
    }

    /// Convenience: wrap in an `Arc` for `Arc<dyn Clock>` storage.
    pub fn shared() -> Arc<Self> {
        Arc::new(Self::new())
    }

    /// How many tasks are currently parked in [`Self::sleep_until`].
    /// Useful in tests to wait for all spawn-then-sleep tasks to register
    /// before advancing the clock.
    pub fn waiter_count(&self) -> usize {
        self.state
            .lock()
            .expect("VirtualClock mutex poisoned")
            .waiters
            .len()
    }

    /// Advance virtual time to `until`, firing every waiter whose deadline
    /// is ≤ `until` in deadline order (with `seq` tie-break). Waiters that
    /// fire observe `now` equal to their own `deadline` (not `until`);
    /// `now` is pinned to `max(now, until)` at the end of the call.
    ///
    /// **Cancel-safety**: synchronous; never awaits.
    ///
    /// **No dropped wakers**: every waiter popped under the lock has its
    /// one-shot `send` invoked before the lock is released, so concurrent
    /// `advance` callers never race to steal a waiter.
    pub fn advance(&self, until: VirtualInstant) {
        let mut state = self.state.lock().expect("VirtualClock mutex poisoned");
        while let Some(std::cmp::Reverse(top)) = state.waiters.peek() {
            if top.deadline > until {
                break;
            }
            let std::cmp::Reverse(waiter) = state.waiters.pop().expect("peeked, must pop");
            state.now = waiter.deadline;
            // `send` only fails if the receiver was dropped — that's the
            // sleeper's task being cancelled, which is fine; we silently
            // discard the signal.
            let _ = waiter.waker.send(());
        }
        if state.now < until {
            state.now = until;
        }
    }

    /// Fire exactly the next-due waiter (if any), advancing `now` to its
    /// deadline. Useful for step-by-step debugging and deterministic
    /// event-by-event tests.
    pub fn advance_to_next_event(&self) {
        let mut state = self.state.lock().expect("VirtualClock mutex poisoned");
        if let Some(std::cmp::Reverse(waiter)) = state.waiters.pop() {
            state.now = waiter.deadline;
            let _ = waiter.waker.send(());
        }
    }
}

#[async_trait]
impl Clock for VirtualClock {
    fn now(&self) -> VirtualInstant {
        self.state.lock().expect("VirtualClock mutex poisoned").now
    }

    async fn sleep_until(&self, deadline: VirtualInstant) {
        let rx = {
            let mut state = self.state.lock().expect("VirtualClock mutex poisoned");
            if state.now >= deadline {
                return;
            }
            let (tx, rx) = tokio::sync::oneshot::channel();
            let seq = state.next_seq;
            state.next_seq += 1;
            state.waiters.push(std::cmp::Reverse(Waiter {
                deadline,
                seq,
                waker: tx,
            }));
            rx
        };
        // `advance` sends `()`; a `RecvError` means the clock was dropped
        // mid-sleep — treat as a spurious wake and return.
        let _ = rx.await;
    }

    async fn sleep(&self, duration: Duration) {
        let deadline = {
            let state = self.state.lock().expect("VirtualClock mutex poisoned");
            VirtualInstant(state.now.0.saturating_add(duration))
        };
        self.sleep_until(deadline).await;
    }

    fn mode(&self) -> ClockMode {
        ClockMode::Virtual
    }
}

// ---------------------------------------------------------------------------
// SessionAnchor — maps VirtualInstant <-> wall-clock UTC.
// ---------------------------------------------------------------------------

/// Maps virtual time onto calendar time via a session-start anchor.
///
/// Example — opening-bell anchor for a US-equities regular-trading-hours
/// session:
///
/// ```
/// use chrono::TimeZone;
/// use midas_ib_sim::engine::clock::{SessionAnchor, VirtualInstant};
///
/// let anchor = SessionAnchor::new(
///     chrono::Utc.with_ymd_and_hms(2026, 4, 20, 13, 30, 0).unwrap(),
/// );
/// let open_plus_one_sec = anchor.to_wall(VirtualInstant::from_millis(1_000));
/// assert_eq!(
///     open_plus_one_sec.timestamp_millis(),
///     anchor.start_wall_time().timestamp_millis() + 1_000,
/// );
/// ```
#[derive(Copy, Clone, Debug)]
pub struct SessionAnchor {
    start_wall_time: chrono::DateTime<chrono::Utc>,
}

impl SessionAnchor {
    pub fn new(start_wall_time: chrono::DateTime<chrono::Utc>) -> Self {
        Self { start_wall_time }
    }

    pub fn start_wall_time(&self) -> chrono::DateTime<chrono::Utc> {
        self.start_wall_time
    }

    /// Map `vi` onto wall-clock UTC. Saturates silently if the resulting
    /// `chrono::Duration` overflows (unreachable for realistic sessions).
    pub fn to_wall(&self, vi: VirtualInstant) -> chrono::DateTime<chrono::Utc> {
        self.start_wall_time + chrono::Duration::from_std(vi.0).unwrap_or(chrono::Duration::zero())
    }

    /// Map `dt` back to virtual time, returning `None` if `dt` precedes
    /// the session start.
    pub fn from_wall(&self, dt: chrono::DateTime<chrono::Utc>) -> Option<VirtualInstant> {
        let delta = (dt - self.start_wall_time).to_std().ok()?;
        Some(VirtualInstant(delta))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    // -----------------------------------------------------------------------
    // VirtualInstant + misc
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn real_clock_sleep_advances_now() {
        let clock = RealClock::new();
        let t0 = clock.now();
        clock.sleep(Duration::from_millis(5)).await;
        let t1 = clock.now();
        assert!(
            t1 > t0,
            "RealClock::now must be monotonic across a sleep: {t0:?} -> {t1:?}"
        );
    }

    #[test]
    fn virtual_instant_ordering() {
        let a = VirtualInstant::from_millis(10);
        let b = VirtualInstant::from_millis(20);
        assert!(a < b);
        assert_eq!(b.saturating_sub(a), Duration::from_millis(10));
    }

    #[test]
    fn virtual_instant_saturating_sub_floor() {
        let a = VirtualInstant::from_millis(10);
        let b = VirtualInstant::from_millis(20);
        // Reverse order must saturate to zero, never panic.
        assert_eq!(a.saturating_sub(b), Duration::ZERO);
    }

    #[test]
    fn virtual_instant_from_millis_round_trip() {
        let vi = VirtualInstant::from_millis(1_234);
        assert_eq!(vi.as_duration(), Duration::from_millis(1_234));
    }

    #[test]
    fn virtual_instant_as_wall_time_adds_offset() {
        let epoch = chrono::DateTime::<chrono::Utc>::from_timestamp(1_000_000_000, 0).unwrap();
        let vi = VirtualInstant::from_millis(1_500);
        let wall = vi.as_wall_time(epoch);
        assert_eq!(wall.timestamp_millis(), epoch.timestamp_millis() + 1_500);
    }

    // -----------------------------------------------------------------------
    // SessionAnchor
    // -----------------------------------------------------------------------

    #[test]
    fn session_anchor_round_trip() {
        let start = chrono::DateTime::<chrono::Utc>::from_timestamp(1_700_000_000, 0).unwrap();
        let anchor = SessionAnchor::new(start);
        let vi = VirtualInstant::from_secs(42);
        let wall = anchor.to_wall(vi);
        assert_eq!(anchor.from_wall(wall), Some(vi));
    }

    #[test]
    fn session_anchor_from_wall_rejects_pre_session() {
        let start = chrono::DateTime::<chrono::Utc>::from_timestamp(1_700_000_000, 0).unwrap();
        let anchor = SessionAnchor::new(start);
        let pre = start - chrono::Duration::seconds(1);
        assert!(anchor.from_wall(pre).is_none());
    }

    // -----------------------------------------------------------------------
    // AcceleratedClock
    // -----------------------------------------------------------------------

    #[tokio::test(start_paused = true)]
    async fn accelerated_clock_scales_now() {
        let clock = AcceleratedClock::with_multiplier(10.0);
        assert_eq!(clock.mode(), ClockMode::Accelerated(10.0));
        // Advance wall time by 100ms — virtual time should advance by 1s.
        tokio::time::advance(Duration::from_millis(100)).await;
        let vi = clock.now();
        // Allow tiny slack; tokio::time::advance is exact under start_paused.
        assert!(
            vi.as_duration() >= Duration::from_millis(990)
                && vi.as_duration() <= Duration::from_millis(1_010),
            "expected ~1s virtual for 100ms wall @ 10×; got {vi:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn accelerated_clock_sleep_scales_wall_time() {
        let clock = AcceleratedClock::with_multiplier(4.0);
        let start_virtual = clock.now();
        // Sleep 4s of virtual time => 1s of wall time.
        clock.sleep(Duration::from_secs(4)).await;
        let elapsed_virtual = clock.now().as_duration() - start_virtual.as_duration();
        assert!(
            elapsed_virtual >= Duration::from_millis(3_900),
            "accelerated sleep should advance virtual time: {elapsed_virtual:?}"
        );
    }

    #[test]
    #[should_panic(expected = "must be finite and > 0")]
    fn accelerated_clock_rejects_zero_multiplier() {
        let _ = AcceleratedClock::with_multiplier(0.0);
    }

    #[test]
    #[should_panic(expected = "must be finite and > 0")]
    fn accelerated_clock_rejects_negative_multiplier() {
        let _ = AcceleratedClock::with_multiplier(-1.0);
    }

    #[test]
    #[should_panic(expected = "must be finite and > 0")]
    fn accelerated_clock_rejects_nan_multiplier() {
        let _ = AcceleratedClock::with_multiplier(f64::NAN);
    }

    // -----------------------------------------------------------------------
    // VirtualClock — core semantics
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn virtual_clock_starts_at_zero() {
        let clock = VirtualClock::new();
        assert_eq!(clock.now(), VirtualInstant::ZERO);
        assert_eq!(clock.mode(), ClockMode::Virtual);
        assert_eq!(clock.waiter_count(), 0);
    }

    #[tokio::test]
    async fn virtual_clock_sleep_returns_immediately_if_past_deadline() {
        let clock = VirtualClock::new();
        clock.advance(VirtualInstant::from_millis(100));
        // Deadline already in the past — must return without parking.
        clock.sleep_until(VirtualInstant::from_millis(50)).await;
        assert_eq!(clock.waiter_count(), 0);
    }

    #[tokio::test]
    async fn virtual_clock_sleep_until_wakes_on_advance() {
        let clock = Arc::new(VirtualClock::new());
        let fired = Arc::new(AtomicUsize::new(0));
        let fired_clone = Arc::clone(&fired);
        let clock_clone = Arc::clone(&clock);
        let handle = tokio::spawn(async move {
            clock_clone
                .sleep_until(VirtualInstant::from_millis(100))
                .await;
            fired_clone.fetch_add(1, AtomicOrdering::SeqCst);
        });

        // Wait for the spawned task to register its waiter.
        while clock.waiter_count() == 0 {
            tokio::task::yield_now().await;
        }
        assert_eq!(fired.load(AtomicOrdering::SeqCst), 0);

        clock.advance(VirtualInstant::from_millis(100));
        handle.await.unwrap();
        assert_eq!(fired.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(clock.now(), VirtualInstant::from_millis(100));
    }

    #[tokio::test]
    async fn virtual_clock_advance_fires_multiple_waiters_in_deadline_order() {
        let clock = Arc::new(VirtualClock::new());
        let order = Arc::new(Mutex::new(Vec::<u64>::new()));

        let deadlines_ms: &[u64] = &[300, 100, 200, 50];
        let mut handles = Vec::new();
        for &ms in deadlines_ms {
            let clock_clone = Arc::clone(&clock);
            let order_clone = Arc::clone(&order);
            handles.push(tokio::spawn(async move {
                clock_clone
                    .sleep_until(VirtualInstant::from_millis(ms))
                    .await;
                order_clone.lock().unwrap().push(ms);
            }));
        }

        // Wait for all 4 waiters to register.
        while clock.waiter_count() < deadlines_ms.len() {
            tokio::task::yield_now().await;
        }

        clock.advance(VirtualInstant::from_millis(1_000));
        for h in handles {
            h.await.unwrap();
        }

        assert_eq!(
            *order.lock().unwrap(),
            vec![50, 100, 200, 300],
            "waiters must fire in deadline order"
        );
        assert_eq!(clock.now(), VirtualInstant::from_millis(1_000));
    }

    #[tokio::test]
    async fn virtual_clock_advance_to_next_event_fires_only_one() {
        let clock = Arc::new(VirtualClock::new());
        let fired = Arc::new(AtomicUsize::new(0));

        let mut handles = Vec::new();
        for &ms in &[50u64, 100, 150] {
            let clock_clone = Arc::clone(&clock);
            let fired_clone = Arc::clone(&fired);
            handles.push(tokio::spawn(async move {
                clock_clone
                    .sleep_until(VirtualInstant::from_millis(ms))
                    .await;
                fired_clone.fetch_add(1, AtomicOrdering::SeqCst);
            }));
        }

        while clock.waiter_count() < 3 {
            tokio::task::yield_now().await;
        }

        clock.advance_to_next_event();
        handles.remove(0).await.unwrap();
        assert_eq!(fired.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(clock.now(), VirtualInstant::from_millis(50));
        assert_eq!(clock.waiter_count(), 2);

        // Clean up remaining waiters.
        clock.advance(VirtualInstant::from_millis(200));
        for h in handles {
            h.await.unwrap();
        }
    }

    #[tokio::test]
    async fn virtual_clock_tie_break_by_registration_order() {
        // Two waiters with identical deadlines must fire in the order they
        // were registered (FIFO via `seq`).
        let clock = Arc::new(VirtualClock::new());
        let order = Arc::new(Mutex::new(Vec::<&'static str>::new()));
        let t = VirtualInstant::from_millis(100);

        // Register "a" first, then "b".
        let clock_a = Arc::clone(&clock);
        let order_a = Arc::clone(&order);
        let h_a = tokio::spawn(async move {
            clock_a.sleep_until(t).await;
            order_a.lock().unwrap().push("a");
        });

        // Deterministic ordering: wait for "a" to be fully registered before
        // spawning "b".
        while clock.waiter_count() < 1 {
            tokio::task::yield_now().await;
        }

        let clock_b = Arc::clone(&clock);
        let order_b = Arc::clone(&order);
        let h_b = tokio::spawn(async move {
            clock_b.sleep_until(t).await;
            order_b.lock().unwrap().push("b");
        });

        while clock.waiter_count() < 2 {
            tokio::task::yield_now().await;
        }

        clock.advance(t);
        h_a.await.unwrap();
        h_b.await.unwrap();
        assert_eq!(*order.lock().unwrap(), vec!["a", "b"]);
    }

    #[tokio::test]
    async fn virtual_clock_sleep_relative_matches_sleep_until() {
        let clock = Arc::new(VirtualClock::new());
        let fired = Arc::new(AtomicUsize::new(0));

        let clock_clone = Arc::clone(&clock);
        let fired_clone = Arc::clone(&fired);
        let handle = tokio::spawn(async move {
            clock_clone.sleep(Duration::from_millis(250)).await;
            fired_clone.fetch_add(1, AtomicOrdering::SeqCst);
        });

        while clock.waiter_count() == 0 {
            tokio::task::yield_now().await;
        }

        // Not yet due.
        clock.advance(VirtualInstant::from_millis(200));
        assert_eq!(fired.load(AtomicOrdering::SeqCst), 0);
        // Now due.
        clock.advance(VirtualInstant::from_millis(250));
        handle.await.unwrap();
        assert_eq!(fired.load(AtomicOrdering::SeqCst), 1);
    }

    #[tokio::test]
    async fn virtual_clock_advance_without_waiters_still_bumps_now() {
        let clock = VirtualClock::new();
        clock.advance(VirtualInstant::from_millis(500));
        assert_eq!(clock.now(), VirtualInstant::from_millis(500));
        // Going "backwards" must not rewind `now`.
        clock.advance(VirtualInstant::from_millis(100));
        assert_eq!(clock.now(), VirtualInstant::from_millis(500));
    }
}
