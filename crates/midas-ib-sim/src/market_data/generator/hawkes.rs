//! Hawkes-lite arrival process (exponentially-decaying excitement).
//!
//! We model tick arrivals as a non-homogeneous Poisson process whose
//! intensity has a self-excitation term:
//!
//! ```text
//! λ(t) = λ_base · U(t) · (1 + excitement(t))
//! excitement(t) = Σ_{t_i < t}  exp(-ln 2 · (t − t_i) / half_life)
//! ```
//!
//! Half-life defaults to ~2 seconds (Bacry et al. 2015 for equities).
//!
//! `sample_next_arrival` uses the inverse-CDF method on the piecewise
//! instantaneous intensity — fine for our purposes because we re-sample
//! after every tick (excitement is reset forwards by the caller).

use std::time::Duration;

/// Hawkes kernel half-life (~2 seconds for US equities).
pub const HAWKES_HALF_LIFE: Duration = Duration::from_secs(2);

/// Decay an existing excitement value forward by `dt` seconds under the
/// configured half-life. Exported as a free function so the generator can
/// call it without threading a `HawkesLite` instance through every method.
pub fn decay_excitement(excitement: f64, dt_secs: f64, half_life: Duration) -> f64 {
    if dt_secs <= 0.0 {
        return excitement;
    }
    let hl = half_life.as_secs_f64();
    if hl <= 0.0 {
        return 0.0;
    }
    // exp(-ln 2 · dt / half_life)
    excitement * (-std::f64::consts::LN_2 * dt_secs / hl).exp()
}

/// Sample the waiting time to the next arrival given an *instantaneous*
/// intensity. We linearise the intensity over the (small) waiting window —
/// adequate for validation tests because the caller re-samples every tick.
///
/// `u` is a uniform(0,1) draw; returns Δt in seconds.
pub fn sample_next_arrival(intensity: f64, u: f64) -> f64 {
    // Guard against λ → 0 (no arrivals in foreseeable future); return a
    // large but finite sentinel so the generator can still make progress.
    let lambda = intensity.max(1e-9);
    let u = u.clamp(1e-12, 1.0 - 1e-12);
    -u.ln() / lambda
}

/// Simple container for the Hawkes parameters. `HawkesLite` is only really
/// a bundle of knobs; the actual state (last excitement + last-tick time)
/// lives in `SymbolState`.
#[derive(Clone, Debug)]
pub struct HawkesLite {
    pub baseline_rate: f64,
    pub excitement: f64,
    pub half_life: Duration,
}

impl Default for HawkesLite {
    fn default() -> Self {
        Self {
            baseline_rate: 1.0,
            excitement: 0.0,
            half_life: HAWKES_HALF_LIFE,
        }
    }
}

impl HawkesLite {
    /// Decay excitement forward by `dt_secs`.
    pub fn decay(&mut self, dt_secs: f64) {
        self.excitement = decay_excitement(self.excitement, dt_secs, self.half_life);
    }

    /// Register a self-excitation event at "now" — adds one unit of excitement.
    pub fn kick(&mut self) {
        self.excitement += 1.0;
    }

    /// Instantaneous intensity at the current state, given an external
    /// multiplier (e.g. intraday U-shape).
    pub fn intensity(&self, multiplier: f64) -> f64 {
        self.baseline_rate * multiplier * (1.0 + self.excitement)
    }

    /// Sample the waiting time to the next arrival.
    pub fn sample_wait(&self, multiplier: f64, u: f64) -> f64 {
        sample_next_arrival(self.intensity(multiplier), u)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decay_halves_over_half_life() {
        let hl = Duration::from_secs(2);
        let d = decay_excitement(1.0, 2.0, hl);
        assert!(
            (d - 0.5).abs() < 1e-9,
            "expected 0.5 after one half-life, got {d}"
        );
        let d2 = decay_excitement(1.0, 4.0, hl);
        assert!((d2 - 0.25).abs() < 1e-9);
    }

    #[test]
    fn decay_with_zero_dt_is_identity() {
        assert_eq!(decay_excitement(3.5, 0.0, HAWKES_HALF_LIFE), 3.5);
    }

    #[test]
    fn sample_next_arrival_is_finite_near_zero_intensity() {
        // Should not panic / infinity out when λ ≈ 0.
        let w = sample_next_arrival(0.0, 0.5);
        assert!(w.is_finite());
    }

    #[test]
    fn exponential_sample_mean_matches_intensity() {
        // With λ = 2.0 and many draws, mean waiting time ≈ 0.5 s.
        let lambda = 2.0;
        let n = 50_000;
        let mut total = 0.0;
        for i in 1..=n {
            let u = i as f64 / (n as f64 + 1.0);
            total += sample_next_arrival(lambda, u);
        }
        let mean = total / n as f64;
        assert!(
            (mean - 0.5).abs() < 0.02,
            "empirical mean {} far from 1/λ = 0.5",
            mean
        );
    }
}
