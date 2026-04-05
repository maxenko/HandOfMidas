//! Stock personality definitions and ticker-seeded selection.
//!
//! Each synthetic ticker is assigned a personality that controls regime-switching
//! parameters, volatility clustering, gap behavior, and volume patterns.

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

// ── Regime ────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Regime {
    Bull = 0,
    Bear = 1,
    Consolidation = 2,
    Crash = 3,
}

impl Regime {
    pub fn from_index(i: usize) -> Self {
        match i {
            0 => Self::Bull,
            1 => Self::Bear,
            2 => Self::Consolidation,
            _ => Self::Crash,
        }
    }
}

// ── Regime parameters ─────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug)]
pub struct RegimeParams {
    /// Expected daily return (e.g. 0.0012 ≈ +30% annualized).
    pub drift: f64,
    /// Daily volatility (std dev of daily returns).
    pub volatility: f64,
}

// ── Stock personality ─────────────────────────────────────────────────────

/// Full parameter set defining how a synthetic stock behaves over time.
#[derive(Clone, Copy, Debug)]
pub struct StockPersonality {
    pub start_price: f64,
    /// \[Bull, Bear, Consolidation, Crash\] regime parameters.
    pub regimes: [RegimeParams; 4],
    /// 4x4 Markov transition matrix (rows sum to 1.0).
    pub transitions: [[f64; 4]; 4],
    /// GARCH(1,1) alpha — weight of previous squared return.
    pub garch_alpha: f64,
    /// GARCH(1,1) beta — weight of previous variance.
    pub garch_beta: f64,
    /// Return autocorrelation (0.0–0.15). Creates trending behavior.
    pub momentum: f64,
    /// Overnight gap std dev as fraction of price.
    pub gap_sigma: f64,
    /// Average daily share volume.
    pub base_daily_volume: i64,
    /// Volume multiplier per regime \[Bull, Bear, Consol, Crash\].
    pub vol_regime_mult: [f64; 4],
}

// ── Presets ────────────────────────────────────────────────────────────────

/// Growth stock (TSLA-like): low start, aggressive uptrend, high volatility.
const GROWTH: StockPersonality = StockPersonality {
    start_price: 15.0,
    regimes: [
        RegimeParams {
            drift: 0.0012,
            volatility: 0.025,
        }, // bull
        RegimeParams {
            drift: -0.0006,
            volatility: 0.030,
        }, // bear
        RegimeParams {
            drift: 0.0001,
            volatility: 0.018,
        }, // consol
        RegimeParams {
            drift: -0.006,
            volatility: 0.055,
        }, // crash
    ],
    transitions: [
        [0.970, 0.010, 0.015, 0.005],
        [0.015, 0.955, 0.025, 0.005],
        [0.025, 0.015, 0.955, 0.005],
        [0.050, 0.100, 0.050, 0.800],
    ],
    garch_alpha: 0.08,
    garch_beta: 0.88,
    momentum: 0.10,
    gap_sigma: 0.005,
    base_daily_volume: 15_000_000,
    vol_regime_mult: [1.0, 1.2, 0.7, 2.5],
};

/// Blue chip (AAPL-like): moderate drift, low volatility, long stable trends.
const BLUE_CHIP: StockPersonality = StockPersonality {
    start_price: 120.0,
    regimes: [
        RegimeParams {
            drift: 0.0004,
            volatility: 0.012,
        },
        RegimeParams {
            drift: -0.0002,
            volatility: 0.016,
        },
        RegimeParams {
            drift: 0.0001,
            volatility: 0.008,
        },
        RegimeParams {
            drift: -0.004,
            volatility: 0.035,
        },
    ],
    transitions: [
        [0.980, 0.005, 0.012, 0.003],
        [0.010, 0.965, 0.020, 0.005],
        [0.015, 0.010, 0.972, 0.003],
        [0.040, 0.100, 0.060, 0.800],
    ],
    garch_alpha: 0.05,
    garch_beta: 0.90,
    momentum: 0.05,
    gap_sigma: 0.002,
    base_daily_volume: 50_000_000,
    vol_regime_mult: [1.0, 1.1, 0.8, 2.0],
};

/// Volatile meme stock (GME-like): wild swings, fast regime changes.
const VOLATILE: StockPersonality = StockPersonality {
    start_price: 8.0,
    regimes: [
        RegimeParams {
            drift: 0.002,
            volatility: 0.040,
        },
        RegimeParams {
            drift: -0.001,
            volatility: 0.045,
        },
        RegimeParams {
            drift: 0.0,
            volatility: 0.028,
        },
        RegimeParams {
            drift: -0.010,
            volatility: 0.070,
        },
    ],
    transitions: [
        [0.940, 0.020, 0.030, 0.010],
        [0.020, 0.935, 0.035, 0.010],
        [0.030, 0.025, 0.935, 0.010],
        [0.060, 0.100, 0.040, 0.800],
    ],
    garch_alpha: 0.10,
    garch_beta: 0.85,
    momentum: 0.12,
    gap_sigma: 0.010,
    base_daily_volume: 5_000_000,
    vol_regime_mult: [1.0, 1.3, 0.6, 3.0],
};

/// Steady dividend stock (KO-like): low vol, long consolidation periods.
const STEADY: StockPersonality = StockPersonality {
    start_price: 55.0,
    regimes: [
        RegimeParams {
            drift: 0.0003,
            volatility: 0.007,
        },
        RegimeParams {
            drift: -0.0002,
            volatility: 0.010,
        },
        RegimeParams {
            drift: 0.0001,
            volatility: 0.005,
        },
        RegimeParams {
            drift: -0.003,
            volatility: 0.025,
        },
    ],
    transitions: [
        [0.975, 0.005, 0.018, 0.002],
        [0.008, 0.970, 0.020, 0.002],
        [0.010, 0.008, 0.980, 0.002],
        [0.030, 0.080, 0.070, 0.820],
    ],
    garch_alpha: 0.04,
    garch_beta: 0.92,
    momentum: 0.03,
    gap_sigma: 0.001,
    base_daily_volume: 10_000_000,
    vol_regime_mult: [1.0, 1.1, 0.9, 1.8],
};

/// Cyclical stock (XOM-like): balanced bull/bear cycles, medium vol.
const CYCLICAL: StockPersonality = StockPersonality {
    start_price: 70.0,
    regimes: [
        RegimeParams {
            drift: 0.0005,
            volatility: 0.015,
        },
        RegimeParams {
            drift: -0.0004,
            volatility: 0.020,
        },
        RegimeParams {
            drift: 0.0,
            volatility: 0.012,
        },
        RegimeParams {
            drift: -0.005,
            volatility: 0.040,
        },
    ],
    transitions: [
        [0.960, 0.015, 0.020, 0.005],
        [0.015, 0.960, 0.020, 0.005],
        [0.020, 0.020, 0.955, 0.005],
        [0.040, 0.100, 0.060, 0.800],
    ],
    garch_alpha: 0.06,
    garch_beta: 0.88,
    momentum: 0.07,
    gap_sigma: 0.003,
    base_daily_volume: 20_000_000,
    vol_regime_mult: [1.0, 1.2, 0.8, 2.2],
};

const PRESETS: [StockPersonality; 5] = [GROWTH, BLUE_CHIP, VOLATILE, STEADY, CYCLICAL];

// ── Seed & selection ──────────────────────────────────────────────────────

/// Deterministic hash of a ticker symbol. Same ticker always produces the same seed.
pub fn ticker_seed(ticker: &str) -> u64 {
    // FNV-1a
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in ticker.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Derive a stock personality from a seed. Picks a base preset and adds
/// deterministic random variation so each ticker is unique.
pub fn personality_for_seed(seed: u64) -> StockPersonality {
    let mut rng = StdRng::seed_from_u64(seed.wrapping_add(0xBEEF));
    let idx = (seed as usize) % PRESETS.len();
    let base = PRESETS[idx];

    // Vary start price within a range appropriate to the preset
    let price_factor: f64 = rng.gen_range(0.5..2.5);

    StockPersonality {
        start_price: (base.start_price * price_factor * 100.0).round() / 100.0,
        regimes: [
            vary_regime(&mut rng, base.regimes[0]),
            vary_regime(&mut rng, base.regimes[1]),
            vary_regime(&mut rng, base.regimes[2]),
            vary_regime(&mut rng, base.regimes[3]),
        ],
        garch_alpha: base.garch_alpha * rng.gen_range(0.8..1.2),
        garch_beta: base.garch_beta * rng.gen_range(0.95..1.05),
        momentum: base.momentum * rng.gen_range(0.7..1.3),
        gap_sigma: base.gap_sigma * rng.gen_range(0.8..1.2),
        base_daily_volume: (base.base_daily_volume as f64 * rng.gen_range(0.4..2.0)) as i64,
        ..base
    }
}

fn vary_regime(rng: &mut StdRng, base: RegimeParams) -> RegimeParams {
    RegimeParams {
        drift: base.drift * rng.gen_range(0.8..1.2),
        volatility: base.volatility * rng.gen_range(0.85..1.15),
    }
}
