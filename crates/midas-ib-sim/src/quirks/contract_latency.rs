//! T2 quirk — jittered latency on `reqContractDetails` responses.
//!
//! Real IB takes 50-200ms to respond to a contract-qualification request.
//! Behind `contract_latency_ms = [50, 200]` in [`QuirksConfig`](super::config).
//! When the range is `[0, 0]` the quirk short-circuits and the engine emits
//! `ContractData`/`ContractDataEnd` immediately.
//!
//! # Determinism
//!
//! The `next_delay_ms` helper takes a `&mut impl Rng`; tests seed with
//! `StdRng::seed_from_u64`. Uniform distribution between the bounds.

use std::time::Duration;

use rand::Rng;

use crate::quirks::config::ContractLatencyConfig;

/// Sample a single contract-details response delay from a config.
pub fn next_delay(cfg: &ContractLatencyConfig, rng: &mut impl Rng) -> Duration {
    if !cfg.is_enabled() {
        return Duration::ZERO;
    }
    let (lo, hi) = cfg.as_range_ms();
    if hi == lo {
        return Duration::from_millis(lo);
    }
    // Uniform [lo, hi] inclusive on both ends.
    Duration::from_millis(rng.gen_range(lo..=hi))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::StdRng;
    use rand::SeedableRng;

    #[test]
    fn disabled_config_returns_zero() {
        let cfg = ContractLatencyConfig::default();
        let mut rng = StdRng::seed_from_u64(1);
        for _ in 0..100 {
            assert_eq!(next_delay(&cfg, &mut rng), Duration::ZERO);
        }
    }

    #[test]
    fn enabled_range_is_inclusive_on_both_ends() {
        let cfg = ContractLatencyConfig {
            min_ms: 50,
            max_ms: 200,
        };
        let mut rng = StdRng::seed_from_u64(42);
        let mut observed_min = u64::MAX;
        let mut observed_max = 0u64;
        for _ in 0..10_000 {
            let d = next_delay(&cfg, &mut rng);
            let ms = d.as_millis() as u64;
            assert!((50..=200).contains(&ms), "out-of-range: {ms}");
            observed_min = observed_min.min(ms);
            observed_max = observed_max.max(ms);
        }
        assert!(
            observed_min <= 55,
            "min never approached lower bound: {observed_min}"
        );
        assert!(
            observed_max >= 195,
            "max never approached upper bound: {observed_max}"
        );
    }

    #[test]
    fn degenerate_range_returns_that_value() {
        let cfg = ContractLatencyConfig {
            min_ms: 123,
            max_ms: 123,
        };
        let mut rng = StdRng::seed_from_u64(1);
        for _ in 0..10 {
            assert_eq!(next_delay(&cfg, &mut rng), Duration::from_millis(123));
        }
    }
}
