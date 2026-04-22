//! Request-id newtypes.
//!
//! Two wire-distinct counters live here:
//!
//! * [`ReqId`] is an `i32` wrapper — that is exactly what IB sends on the
//!   wire (see BR-8). Keeping it wire-accurate avoids silent truncation
//!   when the broker adapter hands values to `rust-ibapi`.
//! * [`RouterSubId`] is a `u64` wrapper for router-internal bookkeeping.
//!   It never crosses an IB boundary, so the wider range is free and
//!   guarantees monotonicity for the entire process lifetime.
//!
//! Both types expose `next(&AtomicX)` helpers so the owning subsystem
//! can mint IDs via a plain `fetch_add`.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};

/// IB-wire request identifier.
///
/// IB uses a signed 32-bit integer on the wire. Callers should seed the
/// atomic counter to `1` (IB rejects `0` in many places) and never mint
/// a negative value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct ReqId(pub i32);

impl ReqId {
    /// Atomically mint the next ID from `counter`.
    ///
    /// Uses `Ordering::Relaxed` — this counter has no happens-before
    /// relationship with other state; downstream consumers only need a
    /// unique value.
    pub fn next(counter: &AtomicI32) -> Self {
        Self(counter.fetch_add(1, Ordering::Relaxed))
    }
}

impl fmt::Display for ReqId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Router-internal subscription identifier.
///
/// Separate from [`ReqId`] so router plumbing (handle registries, guard
/// debug output, etc.) cannot be confused with anything that touches the
/// IB wire. 64 bits is over-provisioned on purpose — it's cheap and
/// removes any concern about wrap-around over long-running processes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct RouterSubId(pub u64);

impl RouterSubId {
    /// Atomically mint the next router sub-id from `counter`.
    pub fn next(counter: &AtomicU64) -> Self {
        Self(counter.fetch_add(1, Ordering::Relaxed))
    }
}

impl fmt::Display for RouterSubId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn req_id_display() {
        assert_eq!(ReqId(42).to_string(), "42");
    }

    #[test]
    fn router_sub_id_display() {
        assert_eq!(RouterSubId(7).to_string(), "7");
    }

    #[test]
    fn req_id_next_is_monotonic_single_threaded() {
        let counter = AtomicI32::new(1);
        let a = ReqId::next(&counter);
        let b = ReqId::next(&counter);
        let c = ReqId::next(&counter);
        assert_eq!(a.0, 1);
        assert_eq!(b.0, 2);
        assert_eq!(c.0, 3);
    }

    #[test]
    fn req_id_next_is_unique_under_concurrent_fetch_add() {
        let counter = Arc::new(AtomicI32::new(1));
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let counter = counter.clone();
                thread::spawn(move || (0..1024).map(|_| ReqId::next(&counter)).collect::<Vec<_>>())
            })
            .collect();
        let mut all: Vec<ReqId> = handles
            .into_iter()
            .flat_map(|h| h.join().unwrap())
            .collect();
        let total = all.len();
        all.sort();
        all.dedup();
        // Every mint must be unique.
        assert_eq!(all.len(), total);
    }

    #[test]
    fn router_sub_id_next_is_unique_under_concurrent_fetch_add() {
        let counter = Arc::new(AtomicU64::new(0));
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let counter = counter.clone();
                thread::spawn(move || {
                    (0..1024)
                        .map(|_| RouterSubId::next(&counter))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        let mut all: Vec<RouterSubId> = handles
            .into_iter()
            .flat_map(|h| h.join().unwrap())
            .collect();
        let total = all.len();
        all.sort();
        all.dedup();
        assert_eq!(all.len(), total);
    }

    #[test]
    fn serde_roundtrip() {
        let r = ReqId(123);
        let s = serde_json::to_string(&r).unwrap();
        let back: ReqId = serde_json::from_str(&s).unwrap();
        assert_eq!(r, back);

        let r = RouterSubId(456);
        let s = serde_json::to_string(&r).unwrap();
        let back: RouterSubId = serde_json::from_str(&s).unwrap();
        assert_eq!(r, back);
    }
}
