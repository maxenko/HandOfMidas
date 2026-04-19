//! Roll (1984) bid-ask bounce.
//!
//! The efficient (unobservable) mid-price `m_t` evolves as a martingale;
//! the observed last-trade price jumps between `m_t − s/2` (buyer-initiated
//! sell at the bid) and `m_t + s/2` (seller-initiated buy at the ask), where
//! `s` is the full spread.
//!
//! Roll's spread estimator uses this property:
//!
//! ```text
//! s = 2 · sqrt(-Cov(Δp_t, Δp_{t+1}))
//! ```
//!
//! Validation test #6 asserts the estimator recovers the configured
//! half-spread within 10%.

use crate::engine::types::Side;

/// Apply the Roll bounce to the mid-price: `last = mid ± half_spread` with
/// `+` for buy-initiated trades and `−` for sell-initiated.
pub fn observed_price(mid: f64, half_spread: f64, side: Side) -> f64 {
    match side {
        Side::Buy => mid + half_spread,
        Side::Sell => mid - half_spread,
    }
}

/// Roll's implicit-spread estimator over an ordered sequence of trade prices.
///
/// Computes `2 · sqrt(-Cov(Δp_t, Δp_{t+1}))` where the covariance is taken
/// over the first-differences of `prices`. Returns `None` if the covariance
/// is non-negative (estimator undefined) or the sample is too small.
pub fn roll_spread_estimator(prices: &[f64]) -> Option<f64> {
    if prices.len() < 8 {
        return None;
    }
    let dp: Vec<f64> = prices.windows(2).map(|w| w[1] - w[0]).collect();
    let n = dp.len() - 1;
    if n < 4 {
        return None;
    }
    let mean_x = dp[..n].iter().sum::<f64>() / n as f64;
    let mean_y = dp[1..].iter().sum::<f64>() / n as f64;
    let mut cov = 0.0;
    for i in 0..n {
        cov += (dp[i] - mean_x) * (dp[i + 1] - mean_y);
    }
    cov /= n as f64;
    if cov >= 0.0 {
        return None;
    }
    Some(2.0 * (-cov).sqrt())
}

/// Sample a random buy/sell side. Optional `bias` in [−1, +1] shifts the
/// probability of a buy above/below 0.5. A negative `prev_momentum_hint`
/// captures Roll's empirical observation that consecutive ticks in the
/// same direction are rarer than iid.
pub fn sample_side(u: f64, bias: f64) -> Side {
    let p_buy = (0.5 + bias).clamp(0.01, 0.99);
    if u < p_buy {
        Side::Buy
    } else {
        Side::Sell
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{rngs::SmallRng, Rng, SeedableRng};

    #[test]
    fn observed_buy_is_above_mid() {
        let p = observed_price(100.0, 0.01, Side::Buy);
        assert!((p - 100.01).abs() < 1e-9);
    }

    #[test]
    fn observed_sell_is_below_mid() {
        let p = observed_price(100.0, 0.01, Side::Sell);
        assert!((p - 99.99).abs() < 1e-9);
    }

    #[test]
    fn roll_estimator_recovers_known_spread() {
        // Martingale mid + bounce with deterministic half-spread = 0.05.
        // Generate 20 000 trades with random 50/50 sides. Expect Roll's
        // estimator to recover s = 0.10 within 10%.
        let mut rng = SmallRng::seed_from_u64(7);
        let half = 0.05;
        let mut mid = 100.0;
        let mut prices = Vec::with_capacity(20_000);
        for _ in 0..20_000 {
            // Martingale mid (tiny Gaussian step so mid changes don't dominate bounce).
            let inc = rng.gen_range(-0.001..0.001);
            mid += inc;
            let side = if rng.gen::<f64>() < 0.5 {
                Side::Buy
            } else {
                Side::Sell
            };
            prices.push(observed_price(mid, half, side));
        }
        let est = roll_spread_estimator(&prices).expect("covariance must be negative");
        let est_half = est / 2.0;
        assert!(
            (est_half - half).abs() / half < 0.1,
            "Roll estimator {} deviates >10% from configured half-spread {}",
            est_half,
            half
        );
    }
}
