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

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn true_range_no_gap() {
        // Simple bar with no gap: TR = high - low.
        assert!((true_range(110.0, 100.0, Some(105.0)) - 10.0).abs() < f64::EPSILON);
    }

    #[test]
    fn true_range_gap_up() {
        // Gap up: high - prev_close > high - low.
        assert!((true_range(120.0, 115.0, Some(100.0)) - 20.0).abs() < f64::EPSILON);
    }

    #[test]
    fn true_range_gap_down() {
        // Gap down: prev_close - low > high - low.
        assert!((true_range(85.0, 80.0, Some(100.0)) - 20.0).abs() < f64::EPSILON);
    }

    #[test]
    fn true_range_first_bar() {
        assert!((true_range(110.0, 100.0, None) - 10.0).abs() < f64::EPSILON);
    }

    #[test]
    fn wilders_atr_sma_phase() {
        // During the first `length` bars, ATR = SMA of True Range.
        let mut atr = WildersAtr::new(3);
        atr.update_tr(10.0);
        assert!((atr.value() - 10.0).abs() < f64::EPSILON);
        atr.update_tr(20.0);
        assert!((atr.value() - 15.0).abs() < f64::EPSILON); // (10+20)/2
        atr.update_tr(30.0);
        assert!((atr.value() - 20.0).abs() < f64::EPSILON); // (10+20+30)/3
        assert!(atr.is_ready());
    }

    #[test]
    fn wilders_atr_ema_phase() {
        let mut atr = WildersAtr::new(3);
        // Fill SMA phase.
        atr.update_tr(10.0);
        atr.update_tr(20.0);
        atr.update_tr(30.0);
        assert!((atr.value() - 20.0).abs() < f64::EPSILON);

        // Next bar: RMA = 40 * (1/3) + 20 * (2/3) = 13.333 + 13.333 = 26.667
        atr.update_tr(40.0);
        let expected = 40.0 / 3.0 + 20.0 * 2.0 / 3.0;
        assert!(
            (atr.value() - expected).abs() < 1e-10,
            "got {}, expected {}",
            atr.value(),
            expected
        );
    }

    #[test]
    fn wilders_atr_converges() {
        // Constant TR should converge to that value.
        let mut atr = WildersAtr::new(14);
        for _ in 0..1000 {
            atr.update_tr(5.0);
        }
        assert!(
            (atr.value() - 5.0).abs() < 1e-10,
            "should converge to 5.0, got {}",
            atr.value()
        );
    }

    #[test]
    fn wilders_atr_as_percent() {
        let mut atr = WildersAtr::new(1);
        atr.update_tr(5.0);
        assert!((atr.as_percent(100.0) - 5.0).abs() < f64::EPSILON);
        assert!((atr.as_percent(200.0) - 2.5).abs() < f64::EPSILON);
    }

    #[test]
    #[should_panic]
    fn wilders_atr_zero_length_panics() {
        WildersAtr::new(0);
    }

    // ── Gerchik ATR tests ────────────────────────────────────────────

    #[test]
    fn gerchik_atr_uniform_candles() {
        // All candles identical — none filtered, result = raw ATR.
        let g = GerchikAtr::new(5);
        let trs = vec![10.0, 10.0, 10.0, 10.0, 10.0];
        assert!((g.compute(&trs).unwrap() - 10.0).abs() < f64::EPSILON);
    }

    #[test]
    fn gerchik_atr_filters_large_candle() {
        // 4 normal candles at 20, one huge at 200.
        // Raw ATR = (20*4 + 200) / 5 = 56.
        // Upper threshold = 56 * 2 = 112. The 200 is excluded.
        // Lower threshold = 56 * 0.5 = 28. The 20s are below 28... also excluded.
        // Use values that survive: 4 at 50, one at 200.
        // Raw ATR = (50*4 + 200) / 5 = 80.
        // Upper = 160. 200 excluded.
        // Lower = 40. 50s pass.
        // Filtered ATR = 50.
        let g = GerchikAtr::new(5);
        let trs = vec![50.0, 50.0, 200.0, 50.0, 50.0];
        assert!((g.compute(&trs).unwrap() - 50.0).abs() < f64::EPSILON);
    }

    #[test]
    fn gerchik_atr_filters_tiny_candle() {
        // 4 normal candles at 20, one tiny at 1.
        // Raw ATR = (20*4 + 1) / 5 = 16.2.
        // Lower threshold = 16.2 * 0.5 = 8.1. The 1.0 is excluded.
        // Filtered ATR = 20.
        let g = GerchikAtr::new(5);
        let trs = vec![20.0, 20.0, 1.0, 20.0, 20.0];
        assert!((g.compute(&trs).unwrap() - 20.0).abs() < f64::EPSILON);
    }

    #[test]
    fn gerchik_atr_all_paranormal_falls_back() {
        // Custom thresholds so tight that everything is excluded.
        let g = GerchikAtr::with_coefficients(3, 0.01, 100.0);
        let trs = vec![10.0, 20.0, 30.0];
        // Falls back to raw ATR = 20.
        assert!((g.compute(&trs).unwrap() - 20.0).abs() < f64::EPSILON);
    }

    #[test]
    fn gerchik_atr_uses_last_n_bars() {
        // Provide more data than length — only last 3 used.
        let g = GerchikAtr::new(3);
        let trs = vec![999.0, 999.0, 10.0, 10.0, 10.0];
        assert!((g.compute(&trs).unwrap() - 10.0).abs() < f64::EPSILON);
    }

    #[test]
    fn gerchik_atr_empty_returns_none() {
        let g = GerchikAtr::new(5);
        assert!(g.compute(&[]).is_none());
    }

    #[test]
    #[should_panic]
    fn gerchik_atr_zero_length_panics() {
        GerchikAtr::new(0);
    }
}
