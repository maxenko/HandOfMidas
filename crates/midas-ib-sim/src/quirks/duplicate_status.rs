//! T2 quirk — occasionally emit duplicate `OrderStatus` messages.
//!
//! Some real-IB paths emit the same `OrderStatus(Submitted, filled=0,
//! remaining=100)` twice. Behind feature flag
//! `fills.duplicate_order_status_rate = 0.05` (5% of status changes).
//!
//! # Determinism
//!
//! Callers pass in a `&mut impl Rng` so tests can seed with a fixed `StdRng`
//! and get reproducible outputs. The duplicator itself holds no RNG state.

use rand::Rng;

/// Stateless policy — `duplicate(rate, rng)` returns `true` with probability
/// `rate`. Clamps `rate` to `[0, 1]` so bad config can't over-duplicate.
#[derive(Copy, Clone, Debug)]
pub struct DuplicateOrderStatus {
    rate: f64,
}

impl DuplicateOrderStatus {
    pub fn new(rate: f64) -> Self {
        let rate = if rate.is_nan() {
            0.0
        } else {
            rate.clamp(0.0, 1.0)
        };
        Self { rate }
    }

    /// Current rate, clamped to [0, 1].
    pub fn rate(&self) -> f64 {
        self.rate
    }

    /// Consult: should the next `OrderStatus` emission be duplicated?
    pub fn should_duplicate(&self, rng: &mut impl Rng) -> bool {
        if self.rate <= 0.0 {
            return false;
        }
        if self.rate >= 1.0 {
            return true;
        }
        rng.gen::<f64>() < self.rate
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::StdRng;
    use rand::SeedableRng;

    #[test]
    fn rate_zero_never_duplicates() {
        let policy = DuplicateOrderStatus::new(0.0);
        let mut rng = StdRng::seed_from_u64(1);
        for _ in 0..1_000 {
            assert!(!policy.should_duplicate(&mut rng));
        }
    }

    #[test]
    fn rate_one_always_duplicates() {
        let policy = DuplicateOrderStatus::new(1.0);
        let mut rng = StdRng::seed_from_u64(1);
        for _ in 0..1_000 {
            assert!(policy.should_duplicate(&mut rng));
        }
    }

    #[test]
    fn rate_negative_clamps_to_zero() {
        assert_eq!(DuplicateOrderStatus::new(-0.5).rate(), 0.0);
    }

    #[test]
    fn rate_above_one_clamps() {
        assert_eq!(DuplicateOrderStatus::new(2.0).rate(), 1.0);
    }

    #[test]
    fn rate_nan_treated_as_zero() {
        assert_eq!(DuplicateOrderStatus::new(f64::NAN).rate(), 0.0);
    }

    #[test]
    fn five_percent_rate_lands_within_tolerance() {
        // Deterministic RNG — running this 10_000 times against a fixed seed
        // must produce a proportion close to 0.05. Tolerance generous enough
        // that any future RNG change doesn't trip the test spuriously.
        let policy = DuplicateOrderStatus::new(0.05);
        let mut rng = StdRng::seed_from_u64(0xDEAD_BEEF);
        let n = 10_000;
        let hits = (0..n).filter(|_| policy.should_duplicate(&mut rng)).count();
        let observed = hits as f64 / n as f64;
        assert!(
            (0.035..=0.065).contains(&observed),
            "observed rate {observed} outside 3.5-6.5% band for 5% target"
        );
    }
}
