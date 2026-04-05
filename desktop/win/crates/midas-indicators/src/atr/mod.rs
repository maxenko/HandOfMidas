//! Average True Range variants.
//!
//! All ATR implementations share the same True Range formula but differ
//! in their smoothing method.

// ── True Range ───────────────────────────────────────────────────────

/// Compute True Range for a single bar.
///
/// `prev_close` is the previous bar's close. On the first bar (no
/// previous close), pass `None` and TR = high - low.
pub fn true_range(high: f64, low: f64, prev_close: Option<f64>) -> f64 {
    match prev_close {
        Some(pc) => {
            let hl = high - low;
            let hc = (high - pc).abs();
            let lc = (low - pc).abs();
            hl.max(hc).max(lc)
        }
        None => high - low,
    }
}

// ── Wilder's ATR ─────────────────────────────────────────────────────

/// Wilder's ATR (Wilder's smoothed moving average of True Range).
///
/// Smoothing: `RMA[i] = TR * (1/length) + RMA[i-1] * (1 - 1/length)`
/// First `length` bars use SMA for initialization.
///
/// This is the standard ATR used by most charting platforms (TradingView's
/// `ta.atr()`, Pine's `ta.rma()`).
#[derive(Clone, Debug)]
pub struct WildersAtr {
    length: usize,
    alpha: f64,
    rma: f64,
    sum: f64,
    count: usize,
}

impl WildersAtr {
    /// Create a new Wilder's ATR with the given period.
    ///
    /// # Panics
    /// Panics if `length` is 0.
    pub fn new(length: usize) -> Self {
        assert!(length > 0, "ATR length must be > 0");
        Self {
            length,
            alpha: 1.0 / length as f64,
            rma: 0.0,
            sum: 0.0,
            count: 0,
        }
    }

    /// Feed one bar's OHLC data. Returns the current ATR value.
    ///
    /// `prev_close` should be the previous bar's close, or `None` for
    /// the first bar.
    pub fn update(&mut self, high: f64, low: f64, prev_close: Option<f64>) -> f64 {
        let tr = true_range(high, low, prev_close);
        self.update_tr(tr)
    }

    /// Feed a pre-computed True Range value. Returns the current ATR.
    pub fn update_tr(&mut self, tr: f64) -> f64 {
        self.count += 1;
        if self.count <= self.length {
            // SMA initialization phase.
            self.sum += tr;
            self.rma = self.sum / self.count as f64;
        } else {
            // Wilder's smoothing (exponential with alpha = 1/length).
            self.rma = tr * self.alpha + self.rma * (1.0 - self.alpha);
        }
        self.rma
    }

    /// Current ATR value (NaN-free: returns 0.0 before first update).
    pub fn value(&self) -> f64 {
        self.rma
    }

    /// Whether the ATR has received enough bars for a stable reading.
    pub fn is_ready(&self) -> bool {
        self.count >= self.length
    }

    /// The smoothing period.
    pub fn length(&self) -> usize {
        self.length
    }

    /// Number of bars processed so far.
    pub fn count(&self) -> usize {
        self.count
    }

    /// Current ATR as a percentage of the given price.
    pub fn as_percent(&self, price: f64) -> f64 {
        if price.abs() > f64::EPSILON {
            (self.rma / price) * 100.0
        } else {
            0.0
        }
    }
}

// ── Gerchik ATR ──────────────────────────────────────────────────────

/// Gerchik's ATR: filters "paranormal" candles before averaging.
///
/// Algorithm:
/// 1. Compute raw ATR (simple average of TR) over a lookback window.
/// 2. Classify each candle as paranormal if its TR exceeds
///    `upper_coeff * raw_ATR` or falls below `lower_coeff * raw_ATR`.
/// 3. Average only the non-paranormal candles' true ranges.
///
/// This is a batch computation over a sliding window, not a streaming
/// accumulator like [`WildersAtr`]. Call [`compute`](GerchikAtr::compute)
/// with a slice of (high, low, prev_close) tuples.
///
/// Reference: <https://www.tradingview.com/script/zqFsHRft-ATR-Gerchik/>
#[derive(Clone, Debug)]
pub struct GerchikAtr {
    /// Lookback window size.
    length: usize,
    /// Candles with TR > upper_coeff * raw_ATR are excluded.
    upper_coeff: f64,
    /// Candles with TR < lower_coeff * raw_ATR are excluded.
    lower_coeff: f64,
}

impl GerchikAtr {
    /// Create with default thresholds (upper: 2.0, lower: 0.5).
    pub fn new(length: usize) -> Self {
        Self::with_coefficients(length, 2.0, 0.5)
    }

    /// Create with custom paranormal thresholds.
    ///
    /// - `upper_coeff`: exclude candles with TR > upper_coeff * raw_ATR (default 2.0)
    /// - `lower_coeff`: exclude candles with TR < lower_coeff * raw_ATR (default 0.5)
    pub fn with_coefficients(length: usize, upper_coeff: f64, lower_coeff: f64) -> Self {
        assert!(length > 0, "ATR length must be > 0");
        Self {
            length,
            upper_coeff,
            lower_coeff,
        }
    }

    /// The lookback window size.
    pub fn length(&self) -> usize {
        self.length
    }

    /// Compute Gerchik ATR from a slice of true range values.
    ///
    /// `true_ranges` should contain the most recent `length` (or more)
    /// TR values, ordered oldest-first. Only the last `length` values
    /// are used.
    ///
    /// Returns `None` if no non-paranormal candles remain after filtering.
    pub fn compute(&self, true_ranges: &[f64]) -> Option<f64> {
        let start = true_ranges.len().saturating_sub(self.length);
        let window = &true_ranges[start..];
        if window.is_empty() {
            return None;
        }

        // Step 1: raw ATR (simple average over the window).
        let raw_atr = window.iter().sum::<f64>() / window.len() as f64;
        if raw_atr.abs() < f64::EPSILON {
            return Some(0.0);
        }

        // Step 2: filter paranormal candles.
        let upper = raw_atr * self.upper_coeff;
        let lower = raw_atr * self.lower_coeff;
        let mut sum = 0.0;
        let mut count = 0u32;
        for &tr in window {
            if tr <= upper && tr >= lower {
                sum += tr;
                count += 1;
            }
        }

        // Step 3: average the survivors.
        if count > 0 {
            Some(sum / count as f64)
        } else {
            // All candles were paranormal — fall back to raw ATR.
            Some(raw_atr)
        }
    }

    /// Convenience: compute from OHLC slices (high, low, close), walking
    /// backward from the last bar.
    ///
    /// Each slice must have the same length. Uses true range (with gaps).
    pub fn compute_from_ohlc(&self, high: &[f64], low: &[f64], close: &[f64]) -> Option<f64> {
        let len = high.len().min(low.len()).min(close.len());
        if len == 0 {
            return None;
        }
        let start = len.saturating_sub(self.length + 1);
        let mut trs = Vec::with_capacity(self.length);
        for i in (start + 1)..len {
            trs.push(true_range(high[i], low[i], Some(close[i - 1])));
        }
        self.compute(&trs)
    }
}

#[cfg(test)]
mod tests;
