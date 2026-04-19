//! Daily and intraday bar generation using regime-switching random walk
//! with GARCH volatility clustering and Brownian bridge intraday paths.

use midas_broker_core::OhlcvBar;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

use super::personality::{Regime, StockPersonality};

// ── Constants ─────────────────────────────────────────────────────────────

/// 2016-01-04 00:00 UTC (Monday) — start of generated history.
pub const DATA_START: i64 = 1451865600;

/// ~10.5 years of trading days (Mon–Fri) to cover through mid-2026.
pub const TRADING_DAYS: usize = 2700;

/// Number of S30 bars per trading day (6.5 hours = 23400s / 30s).
const INTRADAY_BARS: usize = 780;

/// Market open offset from midnight UTC (14:30 UTC = 09:30 ET).
const MARKET_OPEN_OFFSET: i64 = 14 * 3600 + 30 * 60;

/// Seconds per intraday bar (S30).
const BAR_SECS: i64 = 30;

// ── Utilities ─────────────────────────────────────────────────────────────

/// Box-Muller transform: two uniform → one standard normal.
fn standard_normal(rng: &mut impl Rng) -> f64 {
    let u1: f64 = rng.gen_range(1e-10..1.0);
    let u2: f64 = rng.gen::<f64>();
    (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
}

pub fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

// ── Daily generation ──────────────────────────────────────────────────────

/// Generate ~10 years of daily OHLCV bars using regime-switching with
/// GARCH(1,1) volatility clustering, momentum, and overnight gaps.
pub fn generate_daily_bars(personality: &StockPersonality, seed: u64) -> Vec<OhlcvBar> {
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

        // 3. Daily return with momentum and fat-tail noise
        let z = standard_normal(&mut rng);
        let daily_return = rp.drift + personality.momentum * prev_return + sigma * z;

        // 4. Overnight gap
        let gap = standard_normal(&mut rng) * personality.gap_sigma;
        let open = (price * (1.0 + gap)).max(0.01);
        let close = (open * (1.0 + daily_return)).max(0.01);

        // 5. Wicks: lognormal distribution with directional bias
        //    Research: daily body is ~45-60% of range, wicks are heavy-tailed
        let body_high = open.max(close);
        let body_low = open.min(close);
        let wick_scale = sigma * price;
        let upper_mult = (standard_normal(&mut rng) * 0.6 - 0.2).exp();
        let lower_mult = (standard_normal(&mut rng) * 0.6 - 0.2).exp();
        let (wick_up, wick_dn) = if close >= open {
            // Bullish: longer lower wick (buy-the-dip)
            (upper_mult * wick_scale * 0.4, lower_mult * wick_scale * 0.6)
        } else {
            // Bearish: longer upper wick (sell-the-rally)
            (upper_mult * wick_scale * 0.6, lower_mult * wick_scale * 0.4)
        };
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
            ts += 3 * 86400; // Fri → Mon
        } else {
            ts += 86400;
        }
    }

    bars
}

// ── Intraday generation ───────────────────────────────────────────────────

/// Generate a Brownian bridge of close prices for one trading day, scaled to
/// touch the daily High and Low while preserving Open and Close at the endpoints.
fn generate_bridge_closes(rng: &mut impl Rng, daily: &OhlcvBar, n: usize) -> Vec<f64> {
    let range = daily.high - daily.low;
    let price = daily.open.max(0.01);
    // Floor so even small-body days have meaningful intra-day movement
    let effective_range = range.max(price * 0.006);
    let bridge_sigma = effective_range / price / (n as f64).sqrt() * 1.5;

    let mut closes = Vec::with_capacity(n + 1);
    closes.push(daily.open);
    for i in 1..n {
        let remaining = (n - i) as f64;
        let drift = (daily.close - closes[i - 1]) / remaining;
        let noise = standard_normal(rng) * bridge_sigma * price;
        closes.push((closes[i - 1] + drift + noise).max(0.01));
    }
    closes.push(daily.close);

    scale_path_to_range(&mut closes, daily.high, daily.low);
    closes
}

/// Generate S30 (30-second) bars for a single trading day using a Brownian
/// bridge for the close-to-close path, with independent per-bar microstructure
/// for realistic wicks.
///
/// Each bar gets:
/// - Body from the Brownian bridge (determines price path)
/// - Independent lognormal wicks (wide variance, heavy right tail)
/// - Directional bias (bullish → longer lower wick)
/// - U-shaped intraday volatility (higher at market open/close)
/// - Volatility clustering (big bars tend to follow big bars)
/// - U-shaped volume profile
///
/// Target statistics (from empirical research):
/// - ~80% of bars have both wicks visible
/// - Body is ~35-45% of range (median) at S30/M1
/// - Wick sizes are lognormally distributed
pub fn generate_intraday_for_day(daily: &OhlcvBar, seed: u64, day_index: usize) -> Vec<OhlcvBar> {
    let mut rng = StdRng::seed_from_u64(seed ^ (day_index as u64).wrapping_mul(0x517CC1B727220A95));

    let n = INTRADAY_BARS;
    let range = daily.high - daily.low;
    let price = daily.open.max(0.01);

    // Degenerate case: zero-range daily bar
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

    let closes = generate_bridge_closes(&mut rng, daily, n);

    // ── Per-bar microstructure parameters ─────────────────────────────
    let base_ts = daily.timestamp + MARKET_OPEN_OFFSET;

    // Base per-bar range: daily range distributed with √n scaling
    let effective_range = range.max(price * 0.006);
    let base_bar_range = effective_range / (n as f64).sqrt() * 0.5;
    // Minimum bar range: 2 basis points of price
    let min_bar_range = price * 0.0002;

    let mut prev_bar_range = base_bar_range;
    let mut bars = Vec::with_capacity(n);

    // ── 3. Build bars with independent wick microstructure ────────────
    for i in 0..n {
        let bar_open = closes[i];
        let bar_close = closes[i + 1];
        let body = (bar_close - bar_open).abs();
        let body_high = bar_open.max(bar_close);
        let body_low = bar_open.min(bar_close);

        // U-shaped intraday volatility: 2-3x at open/close, 0.6x midday
        let t = i as f64 / n as f64;
        let vol_u = 0.6 + 0.8 * (2.0 * t - 1.0).powi(2);

        // Volatility clustering: big previous bar → bigger current bar
        let cluster = 1.0 + 0.25 * (prev_bar_range / base_bar_range - 1.0).clamp(-0.5, 2.0);

        // Wick base: max of body and locally-adjusted bar range
        let wick_base = (body.max(base_bar_range * vol_u * cluster)).max(min_bar_range);

        // Lognormal wick sizes: exp(N(0,1) * 0.8 - 0.3)
        //   10th pct: 0.25x   50th: 0.74x   90th: 2.34x   99th: 5.5x
        //   Creates wide variance: tiny wicks, normal wicks, occasional very long ones
        let upper_raw = (standard_normal(&mut rng) * 0.8 - 0.3).exp() * wick_base;
        let lower_raw = (standard_normal(&mut rng) * 0.8 - 0.3).exp() * wick_base;

        // Directional bias: bullish → longer lower wick, bearish → longer upper
        let (upper_wick, lower_wick) = if bar_close >= bar_open {
            (upper_raw * 0.75, lower_raw * 1.25)
        } else {
            (upper_raw * 1.25, lower_raw * 0.75)
        };

        let high = round2(body_high + upper_wick);
        let low = round2((body_low - lower_wick).max(0.01));

        // Track range for volatility clustering
        prev_bar_range = (high - low).max(min_bar_range);

        // U-shaped volume: high at market open/close, low midday
        let u_vol = 0.5 + (2.0 * t - 1.0).powi(2);
        let bar_vol = (daily.volume as f64 / n as f64 * u_vol * rng.gen_range(0.5..1.5)) as i64;

        bars.push(OhlcvBar {
            timestamp: base_ts + i as i64 * BAR_SECS,
            open: round2(bar_open),
            high,
            low,
            close: round2(bar_close),
            volume: bar_vol.max(1),
        });
    }

    // ── Post-process: enforce daily OHLCV consistency ────────────────
    // When aggregating all intraday bars for this day, the result must
    // exactly match the daily bar's open, high, low, close, and volume.
    //
    // Open/Close: already guaranteed by the Brownian bridge endpoints.
    // High/Low: clamp per-bar wicks to daily range, then force extremes.
    // Volume: normalize so the sum equals daily volume exactly.

    // 1. Clamp all highs/lows to the daily range
    for bar in &mut bars {
        bar.high = bar.high.min(daily.high);
        bar.low = bar.low.max(daily.low);
        // Safety: ensure OHLC constraints still hold after clamping
        bar.high = bar.high.max(bar.open.max(bar.close));
        bar.low = bar.low.min(bar.open.min(bar.close));
    }

    // 2. Force exactly one bar to hit daily.high and one to hit daily.low.
    //    Pick the bar whose body is closest to the extreme.
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
        // Last bar gets the remainder to avoid rounding drift
        bars[n - 1].volume = (daily.volume - assigned).max(1);
    }

    bars
}

/// Adjust a price path so it reaches `target_high` and `target_low` while
/// preserving the first and last values (Open and Close).
///
/// Points above the body (max of O,C) are scaled outward toward `target_high`.
/// Points below the body (min of O,C) are scaled outward toward `target_low`.
fn scale_path_to_range(prices: &mut [f64], target_high: f64, target_low: f64) {
    if prices.len() < 3 {
        return;
    }
    let len = prices.len();
    let body_high = prices[0].max(prices[len - 1]);
    let body_low = prices[0].min(prices[len - 1]);

    // Scale overshoots above body → target_high, or force a point if none exist
    let raw_max = prices[1..len - 1]
        .iter()
        .cloned()
        .fold(f64::NEG_INFINITY, f64::max);
    if raw_max > body_high + 1e-10 {
        let scale = (target_high - body_high) / (raw_max - body_high);
        for p in &mut prices[1..len - 1] {
            if *p > body_high {
                *p = body_high + (*p - body_high) * scale;
            }
        }
    } else {
        prices[len / 3] = target_high;
    }

    // Scale undershoots below body → target_low, or force a point if none exist
    let raw_min = prices[1..len - 1]
        .iter()
        .cloned()
        .fold(f64::INFINITY, f64::min);
    if body_low - raw_min > 1e-10 {
        let scale = (body_low - target_low) / (body_low - raw_min);
        for p in &mut prices[1..len - 1] {
            if *p < body_low {
                *p = body_low - (body_low - *p) * scale;
            }
        }
    } else {
        prices[len * 2 / 3] = target_low;
    }
}
