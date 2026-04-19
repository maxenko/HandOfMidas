//! 50 msg/sec per-session rate limiter.
//!
//! Token-bucket model: 50 tokens capacity, 50 tokens/sec refill (continuous).
//! Matches real IB behavior — bursts up to the bucket capacity are fine, but
//! sustained traffic above 50/sec exhausts the bucket, triggers error 100, and
//! the protocol layer disconnects the session ~50ms later.
//!
//! # Determinism
//!
//! All time reads go through the [`Clock`] trait, so `VirtualClock` drives the
//! limiter in tests. Refill is computed as `elapsed_secs * refill_rate` — a
//! pure function of the clock, never `Instant::now()`.
//!
//! # Concurrency
//!
//! Not thread-safe on its own. Lives inside the engine actor, which is
//! single-threaded by design, so no interior mutex is needed.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::engine::clock::{Clock, VirtualInstant};
use crate::engine::types::{QuirkViolation, SessionId, ViolationAction};
use crate::quirks::error_codes;

/// Default bucket capacity (real IB's documented ceiling).
pub const DEFAULT_BUCKET_CAPACITY: u32 = 50;

/// Default refill rate — 50 tokens per second. Stored as tokens/sec because
/// the bucket does fractional accounting in `f64` land.
pub const DEFAULT_REFILL_PER_SEC: f64 = 50.0;

/// A single session's token-bucket state. Exposed so `EngineSnapshot` can
/// project violation counters without cloning the whole limiter.
#[derive(Clone, Debug)]
pub struct PerSessionRate {
    /// Tokens currently available. Stored as `f64` for sub-tick refill.
    tokens: f64,
    /// Virtual time of the last refill tick.
    last_refill: VirtualInstant,
    /// Count of violations observed for this session.
    pub violation_count: u32,
}

impl PerSessionRate {
    fn new(capacity: f64, now: VirtualInstant) -> Self {
        Self {
            tokens: capacity,
            last_refill: now,
            violation_count: 0,
        }
    }

    /// For observability / tests.
    pub fn tokens(&self) -> f64 {
        self.tokens
    }
}

/// Per-session msg-rate limiter.
#[derive(Clone)]
pub struct MsgRateLimiter {
    clock: Arc<dyn Clock>,
    capacity: f64,
    refill_per_sec: f64,
    sessions: BTreeMap<SessionId, PerSessionRate>,
}

impl MsgRateLimiter {
    /// Construct with the plan's defaults (50 capacity, 50/sec refill).
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self::with_params(clock, DEFAULT_BUCKET_CAPACITY, DEFAULT_REFILL_PER_SEC)
    }

    /// Construct with custom capacity / refill — used by config and tests.
    pub fn with_params(clock: Arc<dyn Clock>, capacity: u32, refill_per_sec: f64) -> Self {
        assert!(capacity > 0, "MsgRateLimiter capacity must be > 0");
        assert!(
            refill_per_sec.is_finite() && refill_per_sec > 0.0,
            "MsgRateLimiter refill_per_sec must be finite and > 0 (got {refill_per_sec})"
        );
        Self {
            clock,
            capacity: capacity as f64,
            refill_per_sec,
            sessions: BTreeMap::new(),
        }
    }

    /// Try to consume one token for `session`. On success the caller may
    /// proceed; on violation the error payload already carries the canonical
    /// code (100) and `DisconnectAfterError` action.
    ///
    /// On violation the bucket is *not* refunded. Real IB tears the socket
    /// down after the error, so tracking post-violation debits is noise.
    pub fn check(&mut self, session: SessionId) -> Result<(), QuirkViolation> {
        let now = self.clock.now();
        let capacity = self.capacity;
        let refill_per_sec = self.refill_per_sec;
        let per = self
            .sessions
            .entry(session)
            .or_insert_with(|| PerSessionRate::new(capacity, now));
        refill(per, now, capacity, refill_per_sec);

        if per.tokens < 1.0 {
            per.violation_count += 1;
            return Err(QuirkViolation::RateLimit {
                code: error_codes::MSG_RATE_EXCEEDED,
                message: error_codes::message(error_codes::MSG_RATE_EXCEEDED).to_string(),
                action: ViolationAction::DisconnectAfterError,
            });
        }
        per.tokens -= 1.0;
        Ok(())
    }

    /// Drop the per-session record on disconnect so memory doesn't grow
    /// without bound over the lifetime of the engine.
    pub fn forget_session(&mut self, session: SessionId) {
        self.sessions.remove(&session);
    }

    /// Read-only access for tests / `EngineSnapshot`.
    pub fn session(&self, session: SessionId) -> Option<&PerSessionRate> {
        self.sessions.get(&session)
    }

    /// Total violations observed across every session. Used in the counter
    /// block of `EngineSnapshot`.
    pub fn total_violations(&self) -> u64 {
        self.sessions
            .values()
            .map(|s| s.violation_count as u64)
            .sum()
    }
}

/// Refill `per` up to `capacity` based on elapsed virtual time.
fn refill(per: &mut PerSessionRate, now: VirtualInstant, capacity: f64, refill_per_sec: f64) {
    // Clock may technically advance backwards if the caller swapped anchors;
    // saturating_sub protects against it. `VirtualClock::advance` never
    // rewinds, so this is belt-and-braces.
    let elapsed = now.saturating_sub(per.last_refill).as_secs_f64();
    if elapsed <= 0.0 {
        return;
    }
    per.tokens = (per.tokens + elapsed * refill_per_sec).min(capacity);
    per.last_refill = now;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::clock::VirtualClock;

    fn mk(clock: &Arc<VirtualClock>) -> MsgRateLimiter {
        MsgRateLimiter::new(clock.clone() as Arc<dyn Clock>)
    }

    #[test]
    fn full_bucket_admits_fifty_back_to_back() {
        let clock = Arc::new(VirtualClock::new());
        let mut lim = mk(&clock);
        for _ in 0..50 {
            assert!(lim.check(SessionId(1)).is_ok());
        }
    }

    #[test]
    fn fifty_first_message_trips_error_100() {
        let clock = Arc::new(VirtualClock::new());
        let mut lim = mk(&clock);
        for _ in 0..50 {
            assert!(lim.check(SessionId(1)).is_ok());
        }
        let err = lim.check(SessionId(1)).unwrap_err();
        match err {
            QuirkViolation::RateLimit { code, action, .. } => {
                assert_eq!(code, error_codes::MSG_RATE_EXCEEDED);
                assert_eq!(action, ViolationAction::DisconnectAfterError);
            }
            other => panic!("expected RateLimit, got {other:?}"),
        }
        assert_eq!(lim.session(SessionId(1)).unwrap().violation_count, 1);
    }

    #[test]
    fn bucket_refills_over_a_full_second() {
        let clock = Arc::new(VirtualClock::new());
        let mut lim = mk(&clock);
        // Drain the bucket.
        for _ in 0..50 {
            assert!(lim.check(SessionId(1)).is_ok());
        }
        assert!(lim.check(SessionId(1)).is_err());
        // After 1 full virtual second we have a full bucket again.
        clock.advance(VirtualInstant::from_millis(1_000));
        for _ in 0..50 {
            assert!(
                lim.check(SessionId(1)).is_ok(),
                "refilled bucket must admit 50"
            );
        }
        assert!(
            lim.check(SessionId(1)).is_err(),
            "51st after refill must trip"
        );
    }

    #[test]
    fn partial_refill_is_proportional() {
        let clock = Arc::new(VirtualClock::new());
        let mut lim = mk(&clock);
        for _ in 0..50 {
            lim.check(SessionId(1)).unwrap();
        }
        // After 100ms we should have exactly 5 tokens (50/sec * 0.1s).
        clock.advance(VirtualInstant::from_millis(100));
        for _ in 0..5 {
            assert!(lim.check(SessionId(1)).is_ok());
        }
        assert!(
            lim.check(SessionId(1)).is_err(),
            "only 5 tokens should have refilled in 100ms"
        );
    }

    #[test]
    fn sessions_are_independent() {
        let clock = Arc::new(VirtualClock::new());
        let mut lim = mk(&clock);
        for _ in 0..50 {
            lim.check(SessionId(1)).unwrap();
        }
        // Session 2 has its own untouched bucket.
        for _ in 0..50 {
            assert!(lim.check(SessionId(2)).is_ok());
        }
        assert!(lim.check(SessionId(1)).is_err());
        assert!(lim.check(SessionId(2)).is_err());
    }

    #[test]
    fn forget_session_resets_counters() {
        let clock = Arc::new(VirtualClock::new());
        let mut lim = mk(&clock);
        for _ in 0..50 {
            lim.check(SessionId(1)).unwrap();
        }
        assert!(lim.check(SessionId(1)).is_err());
        lim.forget_session(SessionId(1));
        // Fresh session record -> full bucket.
        for _ in 0..50 {
            assert!(lim.check(SessionId(1)).is_ok());
        }
    }

    #[test]
    fn refill_never_exceeds_capacity() {
        let clock = Arc::new(VirtualClock::new());
        let mut lim = mk(&clock);
        // Take one token, then let 10 seconds pass — capacity is still 50.
        lim.check(SessionId(1)).unwrap();
        clock.advance(VirtualInstant::from_secs(10));
        for _ in 0..50 {
            assert!(lim.check(SessionId(1)).is_ok());
        }
        assert!(lim.check(SessionId(1)).is_err());
    }

    #[test]
    #[should_panic(expected = "capacity must be > 0")]
    fn rejects_zero_capacity() {
        let clock = Arc::new(VirtualClock::new()) as Arc<dyn Clock>;
        let _ = MsgRateLimiter::with_params(clock, 0, 50.0);
    }

    #[test]
    #[should_panic(expected = "refill_per_sec must be finite and > 0")]
    fn rejects_nan_refill() {
        let clock = Arc::new(VirtualClock::new()) as Arc<dyn Clock>;
        let _ = MsgRateLimiter::with_params(clock, 50, f64::NAN);
    }

    #[test]
    fn total_violations_sums_across_sessions() {
        let clock = Arc::new(VirtualClock::new());
        let mut lim = mk(&clock);
        for _ in 0..51 {
            let _ = lim.check(SessionId(1));
        }
        for _ in 0..52 {
            let _ = lim.check(SessionId(2));
        }
        // Two violations on session 2, one on session 1.
        assert_eq!(lim.total_violations(), 3);
        assert_eq!(lim.session(SessionId(1)).unwrap().violation_count, 1);
        assert_eq!(lim.session(SessionId(2)).unwrap().violation_count, 2);
    }

    #[test]
    fn backwards_time_is_a_noop() {
        // Scenario: an `AcceleratedClock` with anchor drift could theoretically
        // return earlier `now` than `last_refill`. We must not panic or
        // overflow — `saturating_sub` clamps to zero, refill short-circuits.
        let clock = Arc::new(VirtualClock::new());
        let mut lim = mk(&clock);
        clock.advance(VirtualInstant::from_secs(5));
        lim.check(SessionId(1)).unwrap(); // warms last_refill
                                          // A clock rewind is impossible on VirtualClock, so simulate by
                                          // manually poking `last_refill` forward; passing `now` is now "behind".
        let per = lim.sessions.get_mut(&SessionId(1)).unwrap();
        per.last_refill = VirtualInstant::from_secs(100);
        // No panic; still admits since bucket still has 49 tokens.
        assert!(lim.check(SessionId(1)).is_ok());
    }
}
