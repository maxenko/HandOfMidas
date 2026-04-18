//! Deterministic, realistic test market data generator.
//!
//! This is a self-contained reimplementation of the `midas-broker` test data
//! generator, adapted to produce [`CandleBuffer`] directly (f32 prices,
//! epoch-millisecond timestamps, u32 volumes). It avoids any dependency on
//! the broker crate or its `midas-core`, preventing type conflicts with the
//! desktop workspace's own `midas-core`.
//!
//! The algorithm generates ~10 years (2016-2026) of daily OHLCV bars per
//! ticker using regime-switching dynamics with GARCH(1,1) volatility
//! clustering, momentum, and overnight gaps. Intraday data is produced via
//! Brownian bridge from daily OHLC. Any ticker string is supported --- the
//! ticker name seeds the RNG deterministically (FNV-1a hash).
//!
//! # Usage
//!
//! ```
//! use midas_feed::testdata::TestDataProvider;
//! use midas_core::Timeframe;
//!
//! let mut provider = TestDataProvider::new();
//! let candles = provider.get_candles("AAPL", Timeframe::D1, 365);
//! assert!(candles.len() > 200);
//! ```

use std::collections::HashMap;

use midas_core::Timeframe;
use midas_data::CandleBuffer;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

// ══════════════════════════════════════════════════════════════════════════
// Constants
// ══════════════════════════════════════════════════════════════════════════

/// 2016-01-04 00:00 UTC (Monday) --- start of generated history.
const DATA_START: i64 = 1451865600;

/// ~10.5 years of trading days (Mon-Fri) to cover through mid-2026.
const TRADING_DAYS: usize = 2700;

/// Number of S30 bars per trading day (6.5 hours = 23400s / 30s).
const INTRADAY_BARS: usize = 780;

/// Market open offset from midnight UTC (14:30 UTC = 09:30 ET).
const MARKET_OPEN_OFFSET: i64 = 14 * 3600 + 30 * 60;

/// Seconds per intraday bar (S30).
const BAR_SECS: i64 = 30;

// ══════════════════════════════════════════════════════════════════════════
// Internal bar type (f64 precision, epoch seconds)
// ══════════════════════════════════════════════════════════════════════════

/// Internal OHLCV bar with f64 precision and epoch-second timestamps.
/// Converted to CandleBuffer format (f32, epoch-ms, u32 vol) on output.
#[derive(Clone, Debug)]
struct OhlcvBar {
    timestamp: i64,
    open: f64,
    high: f64,
    low: f64,
    close: f64,
    volume: i64,
}

// ══════════════════════════════════════════════════════════════════════════
// Regime & Personality
// ══════════════════════════════════════════════════════════════════════════

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Regime {
    Bull = 0,
    Bear = 1,
    Consolidation = 2,
    Crash = 3,
}

impl Regime {
    fn from_index(i: usize) -> Self {
        match i {
            0 => Self::Bull,
            1 => Self::Bear,
            2 => Self::Consolidation,
            _ => Self::Crash,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct RegimeParams {
    drift: f64,
    volatility: f64,
}

#[derive(Clone, Copy, Debug)]
struct StockPersonality {
    start_price: f64,
    regimes: [RegimeParams; 4],
    transitions: [[f64; 4]; 4],
    garch_alpha: f64,
    garch_beta: f64,
    momentum: f64,
    gap_sigma: f64,
    base_daily_volume: i64,
    vol_regime_mult: [f64; 4],
}

// ── Presets ──────────────────────────────────────────────────────────────

const GROWTH: StockPersonality = StockPersonality {
    start_price: 15.0,
    regimes: [
        RegimeParams {
            drift: 0.0012,
            volatility: 0.025,
        },
        RegimeParams {
            drift: -0.0006,
            volatility: 0.030,
        },
        RegimeParams {
            drift: 0.0001,
            volatility: 0.018,
        },
        RegimeParams {
            drift: -0.006,
            volatility: 0.055,
        },
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

// ── Seed & selection ────────────────────────────────────────────────────

/// Deterministic FNV-1a hash of a ticker symbol.
fn ticker_seed(ticker: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in ticker.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Derive a stock personality from a seed, with deterministic variation.
fn personality_for_seed(seed: u64) -> StockPersonality {
    let mut rng = StdRng::seed_from_u64(seed.wrapping_add(0xBEEF));
    let idx = (seed as usize) % PRESETS.len();
    let base = PRESETS[idx];

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

// ══════════════════════════════════════════════════════════════════════════
// Utilities
// ══════════════════════════════════════════════════════════════════════════

/// Box-Muller transform: two uniform -> one standard normal.
fn standard_normal(rng: &mut impl Rng) -> f64 {
    let u1: f64 = rng.gen_range(1e-10..1.0);
    let u2: f64 = rng.gen::<f64>();
    (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

// ══════════════════════════════════════════════════════════════════════════
// Daily generation
// ══════════════════════════════════════════════════════════════════════════

/// Generate ~10 years of daily OHLCV bars using regime-switching with
/// GARCH(1,1) volatility clustering, momentum, and overnight gaps.
fn generate_daily_bars(personality: &StockPersonality, seed: u64) -> Vec<OhlcvBar> {
    let mut rng = StdRng::seed_from_u64(seed);
    let mut bars = Vec::with_capacity(TRADING_DAYS);

    let mut price = personality.start_price;
    let mut regime = Regime::Consolidation;
    let mut prev_return: f64 = 0.0;
    let mut sigma = personality.regimes[Regime::Consolidation as usize].volatility;

    let mut ts = DATA_START;
    let mut weekday: u8 = 0; // 0=Mon..4=Fri

    for _ in 0..TRADING_DAYS {
        // 1. Regime transition
        let roll: f64 = rng.gen();
        let row = &personality.transitions[regime as usize];
        let mut cum = 0.0;
        for (j, &p) in row.iter().enumerate() {
            cum += p;
            if roll < cum {
                regime = Regime::from_index(j);
                break;
            }
        }

        let rp = personality.regimes[regime as usize];

        // 2. GARCH(1,1) volatility update
        let omega = rp.volatility.powi(2)
            * (1.0 - personality.garch_alpha - personality.garch_beta).max(0.01);
        sigma = (omega
            + personality.garch_alpha * prev_return.powi(2)
            + personality.garch_beta * sigma.powi(2))
        .sqrt()
        .clamp(rp.volatility * 0.3, rp.volatility * 3.0);

        // 3. Daily return with momentum, fat-tail noise, and a weak log-
        //    space pull toward `start_price`. Without the reversion term,
        //    the regime-switching + momentum + GARCH process is a pure
        //    random walk: over the ~2600-day horizon, unlucky seeds can
        //    drift to the `0.01` absolute floor and stay there, producing
        //    flat tails that the thumbnail sparkline can't render (y_min
        //    == y_max collapses the shader's normalisation span). The
        //    rate below gives a ~230-day half-life — gentle enough to
        //    preserve short-term volatility and multi-year trends, firm
        //    enough to keep prices in a viewable band over the full
        //    dataset.
        const MEAN_REVERSION_RATE: f64 = 0.003;
        let log_dev = (price / personality.start_price).ln();
        let reversion = -log_dev * MEAN_REVERSION_RATE;
        let z = standard_normal(&mut rng);
        let daily_return = rp.drift + reversion + personality.momentum * prev_return + sigma * z;

        // 4. Overnight gap
        let gap = standard_normal(&mut rng) * personality.gap_sigma;
        let open = (price * (1.0 + gap)).max(0.01);
        let close = (open * (1.0 + daily_return)).max(0.01);

        // 5. Wicks (half-normal, regime-aware)
        let body_high = open.max(close);
        let body_low = open.min(close);
        let wick_up = standard_normal(&mut rng).abs() * sigma * price * 0.5;
        let wick_dn = standard_normal(&mut rng).abs() * sigma * price * 0.5;
        let high = body_high + wick_up;
        let low = (body_low - wick_dn).max(0.01);

        // 6. Volume: regime-adjusted, return-correlated, lognormal noise
        let vol_mult = personality.vol_regime_mult[regime as usize];
        let vol_noise = (standard_normal(&mut rng) * 0.3).exp();
        let ret_adj = 1.0 + 2.0 * daily_return.abs() / sigma.max(0.001);
        let volume = (personality.base_daily_volume as f64 * vol_mult * vol_noise * ret_adj) as i64;

        bars.push(OhlcvBar {
            timestamp: ts,
            open: round2(open),
            high: round2(high),
            low: round2(low),
            close: round2(close),
            volume: volume.max(100),
        });

        prev_return = daily_return;
        price = close;

        // Next trading day (skip weekends)
        weekday += 1;
        if weekday >= 5 {
            weekday = 0;
            ts += 3 * 86400; // Fri -> Mon
        } else {
            ts += 86400;
        }
    }

    bars
}

// ══════════════════════════════════════════════════════════════════════════
// Intraday generation
// ══════════════════════════════════════════════════════════════════════════

/// Generate S30 (30-second) bars for a single trading day using a Brownian
/// bridge constrained by the daily OHLC.
fn generate_intraday_for_day(daily: &OhlcvBar, seed: u64, day_index: usize) -> Vec<OhlcvBar> {
    let mut rng = StdRng::seed_from_u64(seed ^ (day_index as u64).wrapping_mul(0x517CC1B727220A95));

    let n = INTRADAY_BARS;
    let range = daily.high - daily.low;

    // Degenerate case: zero-range daily bar -> flat intraday
    if range < 0.005 {
        let base_ts = daily.timestamp + MARKET_OPEN_OFFSET;
        let per_bar = (daily.volume / n as i64).max(1);
        let mut bars: Vec<OhlcvBar> = (0..n)
            .map(|i| OhlcvBar {
                timestamp: base_ts + i as i64 * BAR_SECS,
                open: daily.open,
                high: daily.high,
                low: daily.low,
                close: daily.close,
                volume: per_bar,
            })
            .collect();
        // Last bar absorbs rounding remainder
        let assigned: i64 = bars[..n - 1].iter().map(|b| b.volume).sum();
        bars[n - 1].volume = (daily.volume - assigned).max(1);
        return bars;
    }

    // 1. Generate Brownian bridge close prices (n+1 points: O -> ... -> C)
    let bridge_sigma = range / daily.open.max(0.01) / (n as f64).sqrt() * 1.5;
    let mut closes = Vec::with_capacity(n + 1);
    closes.push(daily.open);

    for i in 1..n {
        let remaining = (n - i) as f64;
        let drift = (daily.close - closes[i - 1]) / remaining;
        let noise = standard_normal(&mut rng) * bridge_sigma * daily.open.max(0.01);
        closes.push((closes[i - 1] + drift + noise).max(0.01));
    }
    closes.push(daily.close);

    // 2. Scale the path so it touches daily H and L
    scale_path_to_range(&mut closes, daily.high, daily.low);

    // 3. Build S30 bars from consecutive close prices
    let base_ts = daily.timestamp + MARKET_OPEN_OFFSET;
    let mut bars = Vec::with_capacity(n);

    for i in 0..n {
        let open = closes[i];
        let close = closes[i + 1];
        let body_h = open.max(close);
        let body_l = open.min(close);

        // Small intrabar wicks
        let wick = standard_normal(&mut rng).abs() * bridge_sigma * daily.open * 0.12;
        let high = body_h + wick;
        let low = (body_l - wick).max(0.01);

        // U-shaped volume: high at open (i~0) and close (i~n), low midday
        let t = i as f64 / n as f64;
        let u_shape = 0.5 + (2.0 * t - 1.0).powi(2); // 1.5 at edges, 0.5 at center
        let bar_vol = (daily.volume as f64 / n as f64 * u_shape * rng.gen_range(0.5..1.5)) as i64;

        bars.push(OhlcvBar {
            timestamp: base_ts + i as i64 * BAR_SECS,
            open: round2(open),
            high: round2(high),
            low: round2(low),
            close: round2(close),
            volume: bar_vol.max(1),
        });
    }

    // ── Post-process: enforce daily OHLCV consistency ────────────────
    // When aggregating all intraday bars for this day, the result must
    // exactly match the daily bar's open, high, low, close, and volume.

    // 1. Clamp all highs/lows to the daily range
    for bar in &mut bars {
        bar.high = bar.high.min(daily.high);
        bar.low = bar.low.max(daily.low);
        // Safety: ensure OHLC constraints still hold after clamping
        bar.high = bar.high.max(bar.open.max(bar.close));
        bar.low = bar.low.min(bar.open.min(bar.close));
    }

    // 2. Force exactly one bar to hit daily.high and one to hit daily.low
    let (hi_idx, _) = bars
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| {
            a.open
                .max(a.close)
                .partial_cmp(&b.open.max(b.close))
                .unwrap()
        })
        .unwrap();
    bars[hi_idx].high = daily.high;

    let (lo_idx, _) = bars
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| {
            a.open
                .min(a.close)
                .partial_cmp(&b.open.min(b.close))
                .unwrap()
        })
        .unwrap();
    bars[lo_idx].low = daily.low;

    // 3. Normalize volumes so they sum exactly to daily.volume
    let raw_sum: i64 = bars.iter().map(|b| b.volume).sum();
    if raw_sum > 0 && daily.volume > 0 {
        let scale = daily.volume as f64 / raw_sum as f64;
        let mut assigned: i64 = 0;
        for bar in &mut bars[..n - 1] {
            bar.volume = ((bar.volume as f64 * scale).round() as i64).max(1);
            assigned += bar.volume;
        }
        bars[n - 1].volume = (daily.volume - assigned).max(1);
    }

    bars
}

/// Adjust a price path so it reaches `target_high` and `target_low` while
/// preserving the first and last values (Open and Close).
fn scale_path_to_range(prices: &mut [f64], target_high: f64, target_low: f64) {
    if prices.len() < 3 {
        return;
    }

    let body_high = prices[0].max(prices[prices.len() - 1]);
    let body_low = prices[0].min(prices[prices.len() - 1]);

    // Find raw extremes of intermediate points
    let interior = &prices[1..prices.len() - 1];
    let raw_max = interior.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let raw_min = interior.iter().cloned().fold(f64::INFINITY, f64::min);

    // Scale overshoots above body -> target_high
    if raw_max > body_high && (raw_max - body_high).abs() > 1e-10 {
        let scale = (target_high - body_high) / (raw_max - body_high);
        let len = prices.len();
        for p in &mut prices[1..len - 1] {
            if *p > body_high {
                *p = body_high + (*p - body_high) * scale;
            }
        }
    }

    // Scale undershoots below body -> target_low
    if raw_min < body_low && (body_low - raw_min).abs() > 1e-10 {
        let scale = (body_low - target_low) / (body_low - raw_min);
        let len = prices.len();
        for p in &mut prices[1..len - 1] {
            if *p < body_low {
                *p = body_low - (body_low - *p) * scale;
            }
        }
    }

    // If the path never overshot the body, force one point to reach the extreme
    let max_after = prices[1..prices.len() - 1]
        .iter()
        .cloned()
        .fold(f64::NEG_INFINITY, f64::max);
    if max_after < target_high - 0.01 {
        let idx = prices.len() / 3;
        prices[idx] = target_high;
    }

    let min_after = prices[1..prices.len() - 1]
        .iter()
        .cloned()
        .fold(f64::INFINITY, f64::min);
    if min_after > target_low + 0.01 {
        let idx = prices.len() * 2 / 3;
        prices[idx] = target_low;
    }
}

// ══════════════════════════════════════════════════════════════════════════
// OHLCV Aggregation
// ══════════════════════════════════════════════════════════════════════════

/// Aggregate fine-grained bars into a coarser timeframe.
fn aggregate_bars(source: &[OhlcvBar], target_tf: Timeframe) -> Vec<OhlcvBar> {
    if source.is_empty() {
        return vec![];
    }

    let step_secs = target_tf.as_secs() as i64;
    let mut result = Vec::new();

    let mut bucket_ts = (source[0].timestamp / step_secs) * step_secs;
    let mut open = source[0].open;
    let mut high = source[0].high;
    let mut low = source[0].low;
    let mut close = source[0].close;
    let mut volume = source[0].volume;

    for bar in &source[1..] {
        let bar_bucket = (bar.timestamp / step_secs) * step_secs;
        if bar_bucket != bucket_ts {
            result.push(OhlcvBar {
                timestamp: bucket_ts,
                open,
                high,
                low,
                close,
                volume,
            });
            bucket_ts = bar_bucket;
            open = bar.open;
            high = bar.high;
            low = bar.low;
            close = bar.close;
            volume = bar.volume;
        } else {
            high = high.max(bar.high);
            low = low.min(bar.low);
            close = bar.close;
            volume += bar.volume;
        }
    }
    result.push(OhlcvBar {
        timestamp: bucket_ts,
        open,
        high,
        low,
        close,
        volume,
    });

    result
}

// ══════════════════════════════════════════════════════════════════════════
// Conversion: OhlcvBar -> CandleBuffer
// ══════════════════════════════════════════════════════════════════════════

/// Convert internal f64/epoch-second bars into a CandleBuffer
/// (f32 prices, epoch-millisecond timestamps, u32 volumes).
fn bars_to_candle_buffer(bars: &[OhlcvBar]) -> CandleBuffer {
    let mut buf = CandleBuffer::with_capacity(bars.len());
    for bar in bars {
        let ts_ms = bar.timestamp * 1000;
        let o = bar.open as f32;
        let h = bar.high as f32;
        let l = bar.low as f32;
        let c = bar.close as f32;
        let v = (bar.volume as u32).max(1);
        buf.push(ts_ms, o, h, l, c, v);
    }
    buf
}

// ══════════════════════════════════════════════════════════════════════════
// TestDataProvider
// ══════════════════════════════════════════════════════════════════════════

/// Cached per-ticker data.
struct TickerData {
    seed: u64,
    #[allow(dead_code)]
    personality: StockPersonality,
    daily_bars: Vec<OhlcvBar>,
    /// day timestamp -> S30 intraday bars (lazily generated).
    intraday_cache: HashMap<i64, Vec<OhlcvBar>>,
}

/// Provider of deterministic test market data.
///
/// Lazily generates ~10 years (2016-2026) of daily bars per ticker on first
/// access, then generates intraday S30 bars on demand via Brownian bridge
/// from the daily OHLC. Coarser timeframes aggregate from S30 or daily.
///
/// Any ticker string works. The same ticker always produces identical data.
pub struct TestDataProvider {
    tickers: HashMap<String, TickerData>,
}

impl TestDataProvider {
    /// Create a new empty provider.
    pub fn new() -> Self {
        Self {
            tickers: HashMap::new(),
        }
    }

    /// Get candle data for any ticker at any timeframe.
    ///
    /// Returns a [`CandleBuffer`] with `days` calendar days of data counted
    /// back from the end of the generated dataset (~mid-2026).
    ///
    /// Supports timeframes from S30 through MN1. Panics if a finer
    /// resolution than S30 is requested.
    pub fn get_candles(&mut self, ticker: &str, timeframe: Timeframe, days: u32) -> CandleBuffer {
        let tf_secs = timeframe.as_secs();
        assert!(
            tf_secs >= Timeframe::S30.as_secs(),
            "TestDataProvider finest resolution is S30; requested {timeframe}",
        );

        self.ensure_ticker(ticker);

        let (_, end) = self.date_range(ticker);
        let start = end - days as i64 * 86400;

        let bars = if tf_secs >= Timeframe::D1.as_secs() {
            self.bars_daily_or_coarser(ticker, timeframe, start, end)
        } else {
            self.bars_intraday(ticker, timeframe, start, end)
        };

        bars_to_candle_buffer(&bars)
    }

    /// Get candle data for a specific epoch-second range.
    ///
    /// Like [`get_candles`](Self::get_candles) but takes explicit
    /// `[start, end)` timestamps in UTC epoch seconds instead of a
    /// trailing day count.
    pub fn get_candles_range(
        &mut self,
        ticker: &str,
        timeframe: Timeframe,
        start: i64,
        end: i64,
    ) -> CandleBuffer {
        let tf_secs = timeframe.as_secs();
        assert!(
            tf_secs >= Timeframe::S30.as_secs(),
            "TestDataProvider finest resolution is S30; requested {timeframe}",
        );

        self.ensure_ticker(ticker);

        let bars = if tf_secs >= Timeframe::D1.as_secs() {
            self.bars_daily_or_coarser(ticker, timeframe, start, end)
        } else {
            self.bars_intraday(ticker, timeframe, start, end)
        };

        bars_to_candle_buffer(&bars)
    }

    /// Ensure a ticker's daily bars are generated.
    fn ensure_ticker(&mut self, ticker: &str) {
        if !self.tickers.contains_key(ticker) {
            let seed = ticker_seed(ticker);
            let personality = personality_for_seed(seed);
            let daily_bars = generate_daily_bars(&personality, seed);
            self.tickers.insert(
                ticker.to_string(),
                TickerData {
                    seed,
                    personality,
                    daily_bars,
                    intraday_cache: HashMap::new(),
                },
            );
        }
    }

    /// Timestamp range covered by the generated data.
    fn date_range(&self, ticker: &str) -> (i64, i64) {
        let bars = &self.tickers[ticker].daily_bars;
        (
            bars.first().expect("no daily bars").timestamp,
            bars.last().expect("no daily bars").timestamp + 86400,
        )
    }

    fn bars_daily_or_coarser(
        &self,
        ticker: &str,
        tf: Timeframe,
        start: i64,
        end: i64,
    ) -> Vec<OhlcvBar> {
        let data = &self.tickers[ticker];
        let filtered: Vec<OhlcvBar> = data
            .daily_bars
            .iter()
            .filter(|b| b.timestamp >= start && b.timestamp < end)
            .cloned()
            .collect();

        if tf == Timeframe::D1 {
            filtered
        } else {
            aggregate_bars(&filtered, tf)
        }
    }

    fn bars_intraday(
        &mut self,
        ticker: &str,
        tf: Timeframe,
        start: i64,
        end: i64,
    ) -> Vec<OhlcvBar> {
        let data = self.tickers.get(ticker).expect("ticker not initialized");

        let day_start = (start / 86400) * 86400;
        let day_end = ((end + 86399) / 86400) * 86400;

        let relevant_days: Vec<(usize, OhlcvBar)> = data
            .daily_bars
            .iter()
            .enumerate()
            .filter(|(_, b)| b.timestamp >= day_start && b.timestamp < day_end)
            .map(|(i, b)| (i, b.clone()))
            .collect();

        let seed = data.seed;
        let data = self
            .tickers
            .get_mut(ticker)
            .expect("ticker not initialized");

        let mut all_bars = Vec::new();
        for (day_idx, daily) in &relevant_days {
            let day_bars = data
                .intraday_cache
                .entry(daily.timestamp)
                .or_insert_with(|| generate_intraday_for_day(daily, seed, *day_idx));

            all_bars.extend(
                day_bars
                    .iter()
                    .filter(|b| b.timestamp >= start && b.timestamp < end)
                    .cloned(),
            );
        }

        if tf.as_secs() == Timeframe::S30.as_secs() {
            all_bars
        } else {
            aggregate_bars(&all_bars, tf)
        }
    }
}

impl Default for TestDataProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
