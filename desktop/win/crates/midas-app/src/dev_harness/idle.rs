//! `wait_for_idle` support: tracks when the UI thread last processed
//! an input-origin or state-mutating message.
//!
//! The iced update path calls [`IdleTracker::mark`] at the top of
//! [`crate::app::MidasApp::update`] for every non-tick-rate message.
//! `wait_for_idle` in the listener polls against this tracker.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

/// How long the UI must be quiet before `wait_for_idle` considers the
/// app idle. Three frames at 60 fps is ~50 ms, which matches the plan.
const IDLE_THRESHOLD: Duration = Duration::from_millis(50);

static IDLE_TRACKER: OnceLock<Arc<IdleTracker>> = OnceLock::new();

pub fn init_global(tracker: Arc<IdleTracker>) -> bool {
    IDLE_TRACKER.set(tracker).is_ok()
}

pub fn try_global() -> Option<Arc<IdleTracker>> {
    IDLE_TRACKER.get().cloned()
}

/// Last-mark timestamp in nanoseconds since an arbitrary epoch
/// (`Instant` is not monotonic-comparable across types, so we stash
/// `elapsed_since_epoch` instead).
pub struct IdleTracker {
    epoch: Instant,
    last_mark_ns: AtomicI64,
}

impl Default for IdleTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl IdleTracker {
    pub fn new() -> Self {
        Self {
            epoch: Instant::now(),
            last_mark_ns: AtomicI64::new(0),
        }
    }

    /// Record that the UI thread just processed a state-mutating message.
    /// Cheap, non-blocking — safe to call from the hot path.
    pub fn mark(&self) {
        let ns = self.epoch.elapsed().as_nanos() as i64;
        self.last_mark_ns.store(ns, Ordering::Release);
    }

    /// Seconds since the last mark.
    pub fn since(&self) -> Duration {
        let last = self.last_mark_ns.load(Ordering::Acquire);
        let now = self.epoch.elapsed().as_nanos() as i64;
        Duration::from_nanos((now - last).max(0) as u64)
    }

    /// Poll until `since() >= IDLE_THRESHOLD` or `timeout` elapses.
    /// Returns `true` if idle was reached.
    pub async fn wait_until_idle(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            let quiet = self.since();
            if quiet >= IDLE_THRESHOLD {
                return true;
            }
            let need = IDLE_THRESHOLD - quiet;
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            tokio::time::sleep(need.min(remaining)).await;
        }
    }
}
