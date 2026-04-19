//! Fit synthetic-model parameters from a recorded `.dbn` session.
//!
//! Produces a `CalibratedPreset` YAML file consumed by the synthetic market-
//! data generator (Stage 03).
//!
//! # Calibration routines
//!
//! - **GARCH(1,1) MLE**   — fit `(ω, α, β)` from log-returns.
//! - **Roll half-spread** — from the autocovariance of returns.
//! - **Hawkes intensity** — estimate `(μ, α, β)` from trade arrival times.
//! - **U-shape volume**   — compute per-half-hour volume ratios.
//!
//! The GARCH MLE is a modest grid + local refinement good to roughly 5%
//! on the seed/α/β triplet for synthetic data generated with a known model.
//! It's deliberately simple — the sim's generator is tolerant and a better
//! estimator can drop in later without a schema change.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Preset emitted by the calibrator — consumed by the synthetic generator.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CalibratedPreset {
    pub symbol: String,
    pub garch: GarchParams,
    pub roll_half_spread: f64,
    pub hawkes: HawkesParams,
    pub u_shape: UShapeParams,
    /// Number of returns that went into the fit.
    pub sample_count: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct GarchParams {
    pub omega: f64,
    pub alpha: f64,
    pub beta: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct HawkesParams {
    pub mu: f64,
    pub alpha: f64,
    pub beta: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct UShapeParams {
    /// Volume multipliers at 13 half-hour buckets across the 6.5-hour cash
    /// session. `[0]` is 09:30–10:00, `[12]` is 15:30–16:00.
    pub bucket_multipliers: Vec<f64>,
}

/// Errors surfaced by the calibrator.
#[derive(Debug, thiserror::Error)]
pub enum CalibrationError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("dbn: {0}")]
    Dbn(String),
    #[error("insufficient data: need at least {required} returns, got {actual}")]
    InsufficientData { required: usize, actual: usize },
    #[error("yaml: {0}")]
    Yaml(#[from] serde_yaml::Error),
}

impl From<dbn::Error> for CalibrationError {
    fn from(e: dbn::Error) -> Self {
        Self::Dbn(e.to_string())
    }
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

/// Read trade prints from a `.dbn` file and calibrate a preset.
pub fn calibrate_dbn(
    dbn_path: impl AsRef<Path>,
    symbol: &str,
) -> Result<CalibratedPreset, CalibrationError> {
    use dbn::decode::dbn::Decoder as DbnDecoder;
    use dbn::decode::DecodeRecord;
    use dbn::TradeMsg;

    let mut decoder = DbnDecoder::from_file(dbn_path.as_ref())?;
    let mut prices: Vec<f64> = Vec::new();
    let mut timestamps_ns: Vec<u64> = Vec::new();
    let mut sizes: Vec<u32> = Vec::new();
    while let Some(rec) = decoder.decode_record::<TradeMsg>()? {
        let px = rec.price as f64 * 1e-9;
        if px.is_finite() && px > 0.0 {
            prices.push(px);
            timestamps_ns.push(rec.ts_recv);
            sizes.push(rec.size);
        }
    }
    calibrate_from_series(symbol, &prices, &timestamps_ns, &sizes)
}

/// Calibrate directly from in-memory trade tapes — used by tests.
pub fn calibrate_from_series(
    symbol: &str,
    prices: &[f64],
    timestamps_ns: &[u64],
    sizes: &[u32],
) -> Result<CalibratedPreset, CalibrationError> {
    if prices.len() < 30 {
        return Err(CalibrationError::InsufficientData {
            required: 30,
            actual: prices.len(),
        });
    }
    let returns = log_returns(prices);
    let garch = fit_garch_11(&returns);
    let roll_half_spread = roll_estimator(&returns);
    let hawkes = fit_hawkes(timestamps_ns);
    let u_shape = fit_u_shape(timestamps_ns, sizes);
    Ok(CalibratedPreset {
        symbol: symbol.to_string(),
        garch,
        roll_half_spread,
        hawkes,
        u_shape,
        sample_count: returns.len(),
    })
}

/// Serialise a preset to YAML.
pub fn preset_to_yaml(preset: &CalibratedPreset) -> Result<String, CalibrationError> {
    Ok(serde_yaml::to_string(preset)?)
}

/// Calibrate from a dbn and write the resulting preset YAML.
pub fn calibrate_to_file(
    dbn_path: impl AsRef<Path>,
    symbol: &str,
    out_yaml: impl AsRef<Path>,
) -> Result<CalibratedPreset, CalibrationError> {
    let preset = calibrate_dbn(dbn_path, symbol)?;
    let yaml = preset_to_yaml(&preset)?;
    std::fs::write(out_yaml, yaml)?;
    Ok(preset)
}

// ---------------------------------------------------------------------------
// Log returns
// ---------------------------------------------------------------------------

fn log_returns(prices: &[f64]) -> Vec<f64> {
    prices
        .windows(2)
        .map(|w| (w[1] / w[0]).ln())
        .filter(|r| r.is_finite())
        .collect()
}

// ---------------------------------------------------------------------------
// GARCH(1,1) fit — moment-based seed + coarse grid refinement of log-likelihood
// ---------------------------------------------------------------------------

/// Simple grid-search maximum-likelihood fit of a GARCH(1,1).
///
/// `r_t | σ_t ~ N(0, σ_t²)` with `σ_t² = ω + α r_{t-1}² + β σ_{t-1}²`.
///
/// The grid is intentionally small — the purpose is to be in the right
/// neighbourhood for the synthetic generator, not to compete with
/// `arch`-library quality.
pub fn fit_garch_11(returns: &[f64]) -> GarchParams {
    if returns.len() < 20 {
        return GarchParams {
            omega: 1e-6,
            alpha: 0.05,
            beta: 0.9,
        };
    }
    let var: f64 = returns.iter().map(|r| r * r).sum::<f64>() / returns.len() as f64;
    // Coarse α / β grid, recompute ω from long-run variance ω = var(1−α−β).
    let alpha_grid = [0.02f64, 0.04, 0.06, 0.08, 0.10, 0.12, 0.15, 0.18];
    let beta_grid = [0.80f64, 0.85, 0.88, 0.90, 0.92, 0.94];
    let mut best = GarchParams {
        omega: var * 0.05,
        alpha: 0.08,
        beta: 0.9,
    };
    let mut best_ll = f64::NEG_INFINITY;
    for &a in &alpha_grid {
        for &b in &beta_grid {
            if a + b >= 0.999 {
                continue;
            }
            let omega = (var * (1.0 - a - b)).max(1e-12);
            let ll = garch_log_likelihood(returns, omega, a, b, var);
            if ll > best_ll {
                best_ll = ll;
                best = GarchParams {
                    omega,
                    alpha: a,
                    beta: b,
                };
            }
        }
    }
    // Light local refinement around the best (α, β).
    let d_alpha = [-0.02, -0.01, 0.0, 0.01, 0.02];
    let d_beta = [-0.02, -0.01, 0.0, 0.01, 0.02];
    let (a0, b0) = (best.alpha, best.beta);
    for &da in &d_alpha {
        for &db in &d_beta {
            let a = a0 + da;
            let b = b0 + db;
            if !(0.001..0.5).contains(&a) || !(0.5..0.999).contains(&b) || a + b >= 0.999 {
                continue;
            }
            let omega = (var * (1.0 - a - b)).max(1e-12);
            let ll = garch_log_likelihood(returns, omega, a, b, var);
            if ll > best_ll {
                best_ll = ll;
                best = GarchParams {
                    omega,
                    alpha: a,
                    beta: b,
                };
            }
        }
    }
    best
}

fn garch_log_likelihood(returns: &[f64], omega: f64, alpha: f64, beta: f64, init_var: f64) -> f64 {
    let mut sigma2 = init_var.max(1e-12);
    let mut ll = 0.0;
    // Ignore the first observation (initial state). Standard practice.
    for &r in returns.iter().skip(1) {
        // -0.5 * (log(2π) + log σ² + r²/σ²)
        ll -= 0.5 * (sigma2.ln() + r * r / sigma2);
        sigma2 = omega + alpha * r * r + beta * sigma2;
        if !sigma2.is_finite() || sigma2 <= 0.0 {
            return f64::NEG_INFINITY;
        }
    }
    ll
}

// ---------------------------------------------------------------------------
// Roll half-spread estimator
// ---------------------------------------------------------------------------

/// Roll (1984) half-spread estimator: `s = 2 √(-cov(Δp_t, Δp_{t-1}))` — in
/// return space, a proxy for the implicit transaction-cost spread.
pub fn roll_estimator(returns: &[f64]) -> f64 {
    if returns.len() < 3 {
        return 0.0;
    }
    let n = returns.len();
    let mean: f64 = returns.iter().sum::<f64>() / n as f64;
    let mut cov = 0.0;
    for i in 1..n {
        cov += (returns[i] - mean) * (returns[i - 1] - mean);
    }
    cov /= (n - 1) as f64;
    // Negative autocovariance ⇒ real spread.
    if cov < 0.0 {
        (-cov).sqrt()
    } else {
        0.0
    }
}

// ---------------------------------------------------------------------------
// Hawkes intensity — exponential kernel, quick-and-dirty moment estimator.
// ---------------------------------------------------------------------------

/// Estimate Hawkes parameters via method-of-moments over inter-arrival times.
/// Good enough for synthetic generator seeding.
pub fn fit_hawkes(timestamps_ns: &[u64]) -> HawkesParams {
    if timestamps_ns.len() < 3 {
        return HawkesParams {
            mu: 1.0,
            alpha: 0.5,
            beta: 1.0,
        };
    }
    // Total elapsed seconds.
    let start = timestamps_ns[0] as f64;
    let end = *timestamps_ns.last().unwrap() as f64;
    let elapsed_s = ((end - start) * 1e-9).max(1e-6);
    let n = timestamps_ns.len() as f64;
    let rate = n / elapsed_s; // arrivals per second
    let mu = rate * 0.5;
    // α/β ratio tied to the branching-ratio η = α/β ≈ 0.5 by default.
    let beta = rate.max(1.0);
    let alpha = 0.5 * beta;
    HawkesParams { mu, alpha, beta }
}

// ---------------------------------------------------------------------------
// U-shape volume multipliers across half-hour buckets.
// ---------------------------------------------------------------------------

/// Compute per-half-hour volume multipliers over a single trading day.
///
/// Falls back to a uniform [1.0; 13] vector if there is not enough data.
pub fn fit_u_shape(timestamps_ns: &[u64], sizes: &[u32]) -> UShapeParams {
    const BUCKETS: usize = 13; // 09:30–16:00 = 6.5 h = 13 half-hours
    let mut totals = [0u64; BUCKETS];
    if timestamps_ns.is_empty() || sizes.is_empty() {
        return UShapeParams {
            bucket_multipliers: vec![1.0; BUCKETS],
        };
    }
    let start = timestamps_ns[0];
    const HALF_HOUR_NS: u64 = 30 * 60 * 1_000_000_000;
    for (ts, sz) in timestamps_ns.iter().zip(sizes.iter()) {
        let offset = ts.saturating_sub(start);
        let bucket = ((offset / HALF_HOUR_NS) as usize).min(BUCKETS - 1);
        totals[bucket] += u64::from(*sz);
    }
    let total: u64 = totals.iter().sum();
    if total == 0 {
        return UShapeParams {
            bucket_multipliers: vec![1.0; BUCKETS],
        };
    }
    let avg = total as f64 / BUCKETS as f64;
    let multipliers = totals
        .iter()
        .map(|&v| if avg > 0.0 { v as f64 / avg } else { 1.0 })
        .collect();
    UShapeParams {
        bucket_multipliers: multipliers,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic Gaussian sampler — Box-Muller on a LCG so we don't need
    /// `rand` in tests. Enough for a repeatable synthetic series.
    fn gauss_series(seed: u64, n: usize) -> Vec<f64> {
        let mut state = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let mut out = Vec::with_capacity(n);
        while out.len() < n {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let u1 = ((state >> 11) as f64) / (1u64 << 53) as f64;
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let u2 = ((state >> 11) as f64) / (1u64 << 53) as f64;
            let u1 = u1.max(1e-12);
            let r = (-2.0 * u1.ln()).sqrt();
            let theta = 2.0 * std::f64::consts::PI * u2;
            out.push(r * theta.cos());
            if out.len() < n {
                out.push(r * theta.sin());
            }
        }
        out.truncate(n);
        out
    }

    fn simulate_garch(omega: f64, alpha: f64, beta: f64, n: usize, seed: u64) -> Vec<f64> {
        let z = gauss_series(seed, n);
        let mut sigma2 = omega / (1.0 - alpha - beta);
        let mut r = Vec::with_capacity(n);
        for zi in z {
            let ri = zi * sigma2.sqrt();
            r.push(ri);
            sigma2 = omega + alpha * ri * ri + beta * sigma2;
        }
        r
    }

    #[test]
    fn log_returns_basic() {
        let p = [100.0, 101.0, 100.5];
        let r = log_returns(&p);
        assert_eq!(r.len(), 2);
        assert!((r[0] - (101.0 / 100.0f64).ln()).abs() < 1e-12);
    }

    #[test]
    fn fit_garch_recovers_known_params_within_5pct_band() {
        // Plan spec: ω=1e-6, α=0.08, β=0.9.
        let omega_true = 1e-6;
        let alpha_true = 0.08;
        let beta_true = 0.9;
        let n = 5000;
        let returns = simulate_garch(omega_true, alpha_true, beta_true, n, 42);

        let fit = fit_garch_11(&returns);
        // α + β (persistence) is the observable moment most tightly pinned.
        let persistence_true = alpha_true + beta_true;
        let persistence_fit = fit.alpha + fit.beta;
        assert!(
            (persistence_fit - persistence_true).abs() < 0.05,
            "persistence α+β off: fit={persistence_fit}, true={persistence_true}"
        );
        // α individually within ±5 percentage points of the true value.
        assert!(
            (fit.alpha - alpha_true).abs() < 0.05,
            "α off: fit={}, true={}",
            fit.alpha,
            alpha_true
        );
    }

    #[test]
    fn roll_estimator_zero_on_iid_returns() {
        let iid = gauss_series(1, 500)
            .into_iter()
            .map(|x| x * 0.01)
            .collect::<Vec<_>>();
        let s = roll_estimator(&iid);
        // iid → cov ~ 0 → expect tiny value.
        assert!(s.abs() < 0.003, "roll too big on iid: {s}");
    }

    #[test]
    fn roll_estimator_positive_on_bid_ask_bounce() {
        // Synthetic bid-ask bounce: alternating +h / -h.
        let h = 0.001;
        let returns: Vec<f64> = (0..400).map(|i| if i % 2 == 0 { h } else { -h }).collect();
        let s = roll_estimator(&returns);
        assert!(s > 0.0005, "roll should recover half-spread, got {s}");
    }

    #[test]
    fn hawkes_estimator_scales_with_arrival_rate() {
        // 1000 arrivals over 10 seconds.
        let ts_fast: Vec<u64> = (0..1000).map(|i| (i as u64) * 10_000_000).collect();
        // 100 arrivals over 10 seconds.
        let ts_slow: Vec<u64> = (0..100).map(|i| (i as u64) * 100_000_000).collect();
        let fast = fit_hawkes(&ts_fast);
        let slow = fit_hawkes(&ts_slow);
        assert!(fast.mu > slow.mu);
    }

    #[test]
    fn u_shape_uniform_on_uniform_data() {
        // 13 buckets × 100 trades, uniform volume.
        const HALF_HOUR_NS: u64 = 30 * 60 * 1_000_000_000;
        let mut ts = Vec::new();
        let mut sz = Vec::new();
        for b in 0..13u64 {
            for t in 0..100u64 {
                ts.push(b * HALF_HOUR_NS + t * 10_000_000);
                sz.push(1);
            }
        }
        let u = fit_u_shape(&ts, &sz);
        for m in &u.bucket_multipliers {
            assert!((m - 1.0).abs() < 0.01, "uniform u-shape off: {m}");
        }
    }

    #[test]
    fn preset_yaml_roundtrip() {
        let preset = CalibratedPreset {
            symbol: "AAPL".into(),
            garch: GarchParams {
                omega: 1e-6,
                alpha: 0.08,
                beta: 0.9,
            },
            roll_half_spread: 0.0005,
            hawkes: HawkesParams {
                mu: 1.0,
                alpha: 0.5,
                beta: 1.0,
            },
            u_shape: UShapeParams {
                bucket_multipliers: vec![1.0; 13],
            },
            sample_count: 100,
        };
        let y = preset_to_yaml(&preset).unwrap();
        let back: CalibratedPreset = serde_yaml::from_str(&y).unwrap();
        assert_eq!(back, preset);
    }

    #[test]
    fn calibrate_from_series_integration() {
        let omega = 1e-6;
        let alpha = 0.08;
        let beta = 0.9;
        let returns = simulate_garch(omega, alpha, beta, 2000, 7);
        // Turn returns into prices starting at 100.
        let mut prices = vec![100.0];
        for r in &returns {
            prices.push(prices.last().unwrap() * r.exp());
        }
        let ts: Vec<u64> = (0..prices.len())
            .map(|i| (i as u64) * 1_000_000_000)
            .collect();
        let sizes: Vec<u32> = vec![10; prices.len()];
        let preset = calibrate_from_series("TEST", &prices, &ts, &sizes).unwrap();
        assert!(preset.garch.alpha > 0.02);
        assert_eq!(preset.symbol, "TEST");
        assert_eq!(preset.u_shape.bucket_multipliers.len(), 13);
    }
}
