//! Xorshift64 RNG helper for the sim backend.
//!
//! Ported from `test_broker/mod.rs` so the new `sim/*` path has no
//! dependency on the legacy module. The algorithm is the classic
//! Marsaglia three-shift variant (`<< 13`, `>> 7`, `<< 17`). It cannot
//! advance from a zero seed, so callers must guarantee a non-zero seed
//! or use [`Xorshift64::from_entropy`] which substitutes a fixed
//! non-zero constant on the degenerate-clock path.

use std::time::{SystemTime, UNIX_EPOCH};

/// Deterministic xorshift64 generator.
///
/// Single-field state so the RNG is trivially `Clone` / `Copy` /
/// `Send` — callers that need interior mutability wrap it in a
/// `parking_lot::Mutex` or `tokio::sync::Mutex`.
#[derive(Debug, Clone, Copy)]
pub struct Xorshift64 {
    state: u64,
}

impl Xorshift64 {
    /// Build a new generator seeded from `state`.
    ///
    /// If `state == 0`, the generator falls back to the golden-ratio
    /// non-zero constant `0x9E3779B97F4A7C15` to avoid the
    /// "xorshift cannot leave zero" degenerate case.
    pub fn new(state: u64) -> Self {
        Self {
            state: if state == 0 {
                0x9E37_79B9_7F4A_7C15
            } else {
                state
            },
        }
    }

    /// Seed from the wall clock, falling back to a fixed non-zero
    /// constant if the system time reads as the Unix epoch.
    pub fn from_entropy() -> Self {
        let seed_ns = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        Self::new(seed_ns)
    }

    /// Advance one step and return the new state.
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    /// Draw a `f64` in `[-1.0, 1.0]`.
    pub fn next_unit(&mut self) -> f64 {
        let r = self.next_u64() as i64;
        r as f64 / i64::MAX as f64
    }

    /// Draw a `f64` in `[0.0, 1.0)`.
    pub fn next_unit_pos(&mut self) -> f64 {
        // Keep the sign bit off so we never negate the result.
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_seed_becomes_nonzero() {
        let mut rng = Xorshift64::new(0);
        assert_ne!(rng.next_u64(), 0);
    }

    #[test]
    fn same_seed_same_stream() {
        let mut a = Xorshift64::new(42);
        let mut b = Xorshift64::new(42);
        for _ in 0..1024 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn next_unit_stays_in_range() {
        let mut rng = Xorshift64::new(0xCAFEBABE);
        for _ in 0..10_000 {
            let v = rng.next_unit();
            assert!((-1.0..=1.0).contains(&v));
        }
    }

    #[test]
    fn next_unit_pos_stays_in_range() {
        let mut rng = Xorshift64::new(0xDEADBEEF);
        for _ in 0..10_000 {
            let v = rng.next_unit_pos();
            assert!((0.0..1.0).contains(&v));
        }
    }
}
