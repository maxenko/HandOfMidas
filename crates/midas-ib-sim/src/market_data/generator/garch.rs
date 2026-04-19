//! GARCH(1,1) volatility process.
//!
//! Defined on a **fixed sampling grid** (1 second by default). See the
//! Stage-03 plan — `plan/ib-sim/03-market-data-engine.md` §"Time-basis
//! discipline" — for why the grid is decoupled from tick arrivals.
//!
//! Canonical parameters (Andersen et al. 2003 for equities):
//!
//! ```text
//! σ²_t = ω + α · r²_{t-1} + β · σ²_{t-1}
//! ω = 1e-6   α = 0.08   β = 0.90    (persistence α+β = 0.98)
//! ```

/// One GARCH grid-step interval (1 second).
pub const GARCH_GRID_INTERVAL_SECS: f64 = 1.0;

/// Default canonical parameters for a liquid US equity, calibrated for the
/// *1-second* grid (not daily as in Andersen et al. 2003).
///
/// Per-second unconditional σ ≈ sqrt(ω/(1-α-β)) ≈ 2e-5 (0.2 bp/s), which
/// aggregates to ~0.06% per minute — slightly calmer than realized intraday
/// SPY volatility but essential for keeping the Roll-bounce signal visible
/// above mid-price noise. Stylized-fact tests still pass with this level.
pub const DEFAULT_OMEGA: f64 = 8e-12;
pub const DEFAULT_ALPHA: f64 = 0.25;
pub const DEFAULT_BETA: f64 = 0.72;

/// Per-symbol GARCH(1,1) state. Holds σ²_t and the last grid-step return.
///
/// `variance` is the *per-grid-step* variance, i.e. the variance of a
/// 1-second log-return. Callers that want a per-tick sigma for non-unit
/// `dt` should scale by `sqrt(dt)`.
#[derive(Clone, Debug)]
pub struct GarchState {
    pub variance: f64,
    pub prev_return: f64,
    pub omega: f64,
    pub alpha: f64,
    pub beta: f64,
}

impl Default for GarchState {
    fn default() -> Self {
        Self::canonical()
    }
}

impl GarchState {
    /// Canonical equity GARCH(1,1). Persistence α+β = 0.98.
    pub fn canonical() -> Self {
        let alpha = DEFAULT_ALPHA;
        let beta = DEFAULT_BETA;
        let omega = DEFAULT_OMEGA;
        Self {
            // Seed variance at the long-run unconditional level so the
            // process starts stationary: σ² = ω / (1 − α − β).
            variance: omega / (1.0 - alpha - beta),
            prev_return: 0.0,
            omega,
            alpha,
            beta,
        }
    }

    /// Build from explicit parameters.
    pub fn with_params(omega: f64, alpha: f64, beta: f64) -> Self {
        debug_assert!(omega > 0.0, "GARCH ω must be positive");
        debug_assert!(alpha >= 0.0, "GARCH α must be non-negative");
        debug_assert!(beta >= 0.0, "GARCH β must be non-negative");
        debug_assert!(alpha + beta < 1.0, "GARCH α+β must be <1 for stationarity");
        let variance = omega / (1.0 - alpha - beta);
        Self {
            variance,
            prev_return: 0.0,
            omega,
            alpha,
            beta,
        }
    }

    /// Persistence α+β.
    pub fn persistence(&self) -> f64 {
        self.alpha + self.beta
    }

    /// Long-run unconditional variance σ² = ω / (1 − α − β).
    pub fn unconditional_variance(&self) -> f64 {
        self.omega / (1.0 - self.alpha - self.beta)
    }

    /// Advance one grid-step:
    ///
    ///   σ²_t      = ω + α · r²_{t-1} + β · σ²_{t-1}
    ///   r_t       = σ_t · ε
    ///   prev_ret ← r_t
    ///
    /// Returns the freshly-sampled grid-level return r_t so callers can
    /// estimate `α+β` from a sequence of returns during validation.
    pub fn step(&mut self, innovation: f64) -> f64 {
        // σ²_t = ω + α · r²_{t-1} + β · σ²_{t-1}
        self.variance = self.omega
            + self.alpha * self.prev_return * self.prev_return
            + self.beta * self.variance;
        // Guard against pathological negatives (shouldn't happen with valid params).
        if self.variance < 0.0 || !self.variance.is_finite() {
            self.variance = self.omega;
        }
        // Cap at 100× the unconditional variance so extreme sequences of
        // fat-tailed shocks don't send σ → ∞ and corrupt downstream prices.
        let cap = self.unconditional_variance() * 100.0;
        if self.variance > cap {
            self.variance = cap;
        }
        let r = self.variance.sqrt() * innovation;
        self.prev_return = r;
        r
    }

    /// Current σ (per grid-step).
    pub fn sigma(&self) -> f64 {
        self.variance.sqrt()
    }
}

// ---------------------------------------------------------------------------
// Autocorrelation helpers — used by stylized-fact tests and the
// λ_base-independence validator. Keeps the model-tuning feedback loop inside
// the crate instead of shelling out to Python/R.
// ---------------------------------------------------------------------------

/// Sample Pearson autocorrelation of `xs` at lag `lag`. Returns `None` if
/// the sample is too small or has zero variance.
pub fn autocorrelation(xs: &[f64], lag: usize) -> Option<f64> {
    if xs.len() <= lag + 2 {
        return None;
    }
    let mean = xs.iter().sum::<f64>() / xs.len() as f64;
    let var = xs.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / xs.len() as f64;
    if var <= 0.0 {
        return None;
    }
    let n = xs.len() - lag;
    let mut cov = 0.0;
    for i in 0..n {
        cov += (xs[i] - mean) * (xs[i + lag] - mean);
    }
    cov /= n as f64;
    Some(cov / var)
}

/// Estimate `α+β` from a GARCH(1,1) sample via the ARMA(1,1) representation
/// of `r²_t`. For GARCH(1,1):
///
/// ```text
/// ρ(k) = [α(1 − αβ − β²)] · (α+β)^{k−1} / [1 − 2αβ − β²]     for k ≥ 1
/// ```
///
/// so `ρ(k+1) / ρ(k) = α+β`. We use the ratio of lag-2 to lag-1
/// autocorrelations of `r²`, which is a consistent (though noisy) estimator.
/// Adequate for the ±0.03 persistence-drift test.
pub fn estimate_persistence(returns: &[f64]) -> Option<f64> {
    let sq: Vec<f64> = returns.iter().map(|r| r * r).collect();
    let rho1 = autocorrelation(&sq, 1)?;
    let rho2 = autocorrelation(&sq, 2)?;
    if rho1.abs() < 1e-6 {
        return None;
    }
    let est = rho2 / rho1;
    if !est.is_finite() {
        return None;
    }
    Some(est.clamp(-1.5, 1.5))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{rngs::SmallRng, SeedableRng};
    use rand_distr::Distribution;

    #[test]
    fn canonical_has_expected_persistence() {
        let g = GarchState::canonical();
        assert!(
            (g.persistence() - (DEFAULT_ALPHA + DEFAULT_BETA)).abs() < 1e-12,
            "persistence {} doesn't match α+β",
            g.persistence()
        );
        // Sanity: defaults are in the stationary regime, and α+β < 1.
        assert!(g.persistence() < 1.0 && g.persistence() > 0.9);
    }

    #[test]
    fn unconditional_variance_matches_seed() {
        let g = GarchState::canonical();
        assert!((g.variance - g.unconditional_variance()).abs() < 1e-18);
    }

    #[test]
    fn step_advances_variance_on_shock() {
        let mut g = GarchState::with_params(1e-6, 0.1, 0.85);
        let v0 = g.variance;
        // Big innovation → |r_t| large → next variance should rise.
        g.step(5.0);
        g.step(0.0);
        assert!(
            g.variance > v0,
            "variance must rise after big shock: {} vs {}",
            g.variance,
            v0
        );
    }

    #[test]
    fn lag1_autocorrelation_of_squares_positive() {
        // GARCH(1,1) by construction produces positively-autocorrelated
        // squared returns (volatility clustering). We check both Normal
        // and Student-t(4) innovations to make sure the property holds
        // under the fat-tailed regime the sim uses.
        use rand_distr::{StandardNormal, StudentT};
        let mut rng = SmallRng::seed_from_u64(42);
        let mut g = GarchState::canonical();
        let n = 50_000;
        let mut returns = Vec::with_capacity(n);
        for _ in 0..n {
            let eps: f64 = StandardNormal.sample(&mut rng);
            returns.push(g.step(eps));
        }
        let sq: Vec<f64> = returns.iter().map(|r| r * r).collect();
        let rho1 = autocorrelation(&sq, 1).expect("ACF defined");
        println!("canonical GARCH ρ(1) under Normal ε: {rho1}");
        assert!(
            rho1 > 0.03,
            "lag-1 ACF of squared returns too low for GARCH(Normal): {rho1}"
        );

        // Student-t(4) winsorised at ±20 — the sim's actual innovation.
        let mut rng2 = SmallRng::seed_from_u64(7);
        let t = StudentT::new(4.0).unwrap();
        let mut g2 = GarchState::canonical();
        let mut returns2 = Vec::with_capacity(n);
        for _ in 0..n {
            let e: f64 = t.sample(&mut rng2);
            returns2.push(g2.step(e.clamp(-20.0, 20.0)));
        }
        let sq2: Vec<f64> = returns2.iter().map(|r| r * r).collect();
        let rho1_t = autocorrelation(&sq2, 1).expect("ACF defined");
        println!("canonical GARCH ρ(1) under winsorised-t(4) ε: {rho1_t}");
        assert!(
            rho1_t > 0.01,
            "lag-1 ACF of squared returns too low for GARCH(t4): {rho1_t}"
        );
    }
}
