//! Deterministic clock abstraction — Stage 08 trait definition + `RealClock`.
//!
//! Three implementors exist in the design:
//! - `RealClock` — thin wrapper over `tokio::time::Instant`; production default.
//! - `VirtualClock` — test-only; time only advances via `advance()`.
//! - `AcceleratedClock` — wraps `RealClock` with a time multiplier (demos).
//!
//! Stage 01 ships `RealClock` and the trait definition. Wave 2 (Stage 08) fills
//! in `VirtualClock` / `AcceleratedClock` and `EventScheduler`.

use std::cmp::Ordering;
use std::fmt;
use std::sync::Arc;
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
///
/// Wave 2 adds `VirtualClock` (Stage 08) against the same trait so engine
/// code is agnostic to mode.
#[async_trait]
pub trait Clock: Send + Sync + 'static {
    fn now(&self) -> VirtualInstant;
    async fn sleep_until(&self, deadline: VirtualInstant);
    async fn sleep(&self, duration: Duration);
    fn mode(&self) -> ClockMode;
}

// ---------------------------------------------------------------------------
// RealClock — the production default. Implemented in Stage 01 (small + stable).
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
// VirtualClock stub — Stage 08 fills in.
// ---------------------------------------------------------------------------

/// Test-only clock. Time only advances when `advance()` is called. Stage 08
/// fills in the waker / priority-queue internals; Stage 01 ships the
/// type signature + trait impl stubs so other stages can reference the type.
#[derive(Clone, Debug, Default)]
pub struct VirtualClock {
    _priv: (),
}

impl VirtualClock {
    pub fn new() -> Self {
        Self { _priv: () }
    }

    pub fn shared() -> Arc<Self> {
        Arc::new(Self::new())
    }

    /// Advance virtual time to `until`, firing every waiter whose deadline is
    /// ≤ `until`. Stage 08 implements.
    pub fn advance(&self, _until: VirtualInstant) {
        todo!("Stage 08 — VirtualClock::advance")
    }

    /// Fire exactly the next-due waiter. Stage 08 implements.
    pub fn advance_to_next_event(&self) {
        todo!("Stage 08 — VirtualClock::advance_to_next_event")
    }
}

#[async_trait]
impl Clock for VirtualClock {
    fn now(&self) -> VirtualInstant {
        todo!("Stage 08 — VirtualClock::now")
    }

    async fn sleep_until(&self, _deadline: VirtualInstant) {
        todo!("Stage 08 — VirtualClock::sleep_until")
    }

    async fn sleep(&self, _duration: Duration) {
        todo!("Stage 08 — VirtualClock::sleep")
    }

    fn mode(&self) -> ClockMode {
        ClockMode::Virtual
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
