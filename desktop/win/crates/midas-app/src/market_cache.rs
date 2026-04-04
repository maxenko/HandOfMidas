//! In-memory cache of market data snapshots keyed by uppercase symbol.
//!
//! Populated asynchronously from the active DataProvider. The watchlist
//! reads from this cache instead of scraping chart candle buffers.

use std::collections::HashMap;

use midas_core::MarketSnapshot;

/// In-memory market data cache.
#[derive(Debug, Default)]
pub struct MarketDataCache {
    snapshots: HashMap<String, MarketSnapshot>,
}

impl MarketDataCache {
    /// Get a snapshot for a symbol.
    pub fn get(&self, symbol: &str) -> Option<&MarketSnapshot> {
        self.snapshots.get(symbol)
    }

    /// Get a mutable snapshot for a symbol (e.g., to merge GATR from intraday data).
    pub fn get_mut(&mut self, symbol: &str) -> Option<&mut MarketSnapshot> {
        self.snapshots.get_mut(symbol)
    }

    /// Insert or update a snapshot.
    pub fn insert(&mut self, symbol: String, snapshot: MarketSnapshot) {
        self.snapshots.insert(symbol, snapshot);
    }

    /// Remove a snapshot.
    pub fn remove(&mut self, symbol: &str) {
        self.snapshots.remove(symbol);
    }

    /// Iterate over all cached symbols.
    pub fn symbols(&self) -> impl Iterator<Item = &String> {
        self.snapshots.keys()
    }
}

/// Compute a [`MarketSnapshot`] from daily candle data.
///
/// - **Price / Change%**: from the last two daily bars.
/// - **G.ATR**: 14-period ATR computed from all bars *except* the last day.
///   The percentage is the last day's (high - low) / ATR * 100, showing
///   how much of the average daily range has been traveled today.
///
/// Requires at least 3 daily bars (2 for ATR seed + 1 for today's range).
pub fn snapshot_from_candles(buffer: &midas_core::CandleBuffer) -> MarketSnapshot {
    let len = buffer.len();
    if len == 0 {
        return MarketSnapshot::default();
    }
    let last_close = buffer.closes[len - 1] as f64;
    let prev_close = if len >= 2 {
        Some(buffer.closes[len - 2] as f64)
    } else {
        None
    };
    let change_pct = prev_close.map(|prev| {
        if prev != 0.0 {
            ((last_close - prev) / prev) * 100.0
        } else {
            0.0
        }
    });

    let gatr_pct = compute_daily_gatr(buffer);

    MarketSnapshot {
        last_price: Some(last_close),
        prev_close,
        change_pct,
        gatr_pct,
    }
}

/// Compute Gerchik ATR percentage from daily candle data.
///
/// Delegates to [`midas_core::gerchik_gatr_pct`] which implements the
/// canonical algorithm: skip today, walk previous 7 sessions, filter
/// paranormal candles (TR > 2x or < 0.5x of raw average), then show
/// today's (H-L) as a percentage of the filtered average.
fn compute_daily_gatr(buffer: &midas_core::CandleBuffer) -> Option<f32> {
    let len = buffer.len();
    if len < 3 {
        return None; // Need at least 2 history bars + today
    }
    let highs: Vec<f64> = buffer.highs.iter().map(|&h| h as f64).collect();
    let lows: Vec<f64> = buffer.lows.iter().map(|&l| l as f64).collect();
    let closes: Vec<f64> = buffer.closes.iter().map(|&c| c as f64).collect();

    // Debug: log the last few bars so we can verify the calculation.
    let start = len.saturating_sub(10);
    for i in start..len {
        let label = if i == len - 1 { "TODAY" } else { "     " };
        tracing::debug!(
            "G.ATR {label} bar[{i}]: H={:.2} L={:.2} C={:.2} range={:.2}",
            highs[i], lows[i], closes[i], highs[i] - lows[i],
        );
    }

    let result = midas_core::gerchik_gatr_pct(&highs, &lows, &closes);
    if let Some(pct) = result {
        tracing::debug!("G.ATR result: {pct:.1}% (from {len} D1 bars)");
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use midas_core::CandleBuffer;

    #[test]
    fn snapshot_from_empty_buffer() {
        let buf = CandleBuffer::new();
        let snap = snapshot_from_candles(&buf);
        assert!(snap.last_price.is_none());
        assert!(snap.prev_close.is_none());
        assert!(snap.change_pct.is_none());
        assert!(snap.gatr_pct.is_none());
    }

    #[test]
    fn snapshot_from_single_candle() {
        let mut buf = CandleBuffer::new();
        buf.push(1_000, 100.0, 105.0, 95.0, 102.0, 500);
        let snap = snapshot_from_candles(&buf);
        assert_eq!(snap.last_price, Some(102.0));
        assert!(snap.prev_close.is_none());
        assert!(snap.change_pct.is_none());
    }

    #[test]
    fn snapshot_from_two_candles() {
        let mut buf = CandleBuffer::new();
        buf.push(1_000, 100.0, 105.0, 95.0, 100.0, 500);
        buf.push(2_000, 100.0, 110.0, 98.0, 105.0, 600);
        let snap = snapshot_from_candles(&buf);
        assert_eq!(snap.last_price, Some(105.0));
        assert_eq!(snap.prev_close, Some(100.0));
        // change_pct = ((105 - 100) / 100) * 100 = 5.0
        let pct = snap.change_pct.expect("change_pct should be Some");
        assert!((pct - 5.0).abs() < 1e-9, "expected 5.0, got {pct}");
        // Two candles is not enough for GATR (need at least 3).
        assert!(snap.gatr_pct.is_none());
    }

    #[test]
    fn snapshot_gatr_from_daily_bars() {
        // Build 9 daily bars (7 lookback + 1 "today" needs 8 minimum,
        // but we need an extra bar for true range prev_close).
        let mut buf = CandleBuffer::new();
        // 8 history bars with uniform range, then 1 "today" bar.
        for i in 0..8 {
            let base = 100.0 + i as f32;
            buf.push(
                (i + 1) as i64 * 1000,
                base,
                base + 10.0,
                base - 10.0,
                base + 5.0,
                500,
            );
        }
        // "Today" bar with same range (20). Filtered ATR should be ~20. Pct ≈ 100%.
        buf.push(9_000, 108.0, 118.0, 98.0, 113.0, 500);
        let snap = snapshot_from_candles(&buf);
        let gatr_pct = snap.gatr_pct.expect("gatr_pct should be Some with 9 bars");
        assert!(
            (gatr_pct - 100.0).abs() < 5.0,
            "expected ~100%, got {gatr_pct}"
        );
    }

    #[test]
    fn snapshot_gatr_needs_at_least_3_bars() {
        let mut buf = CandleBuffer::new();
        buf.push(1_000, 100.0, 110.0, 90.0, 105.0, 500);
        buf.push(2_000, 105.0, 115.0, 95.0, 110.0, 500);
        let snap = snapshot_from_candles(&buf);
        assert!(snap.gatr_pct.is_none(), "2 bars should not produce GATR");
    }

    #[test]
    fn cache_get_insert_remove_roundtrip() {
        let mut cache = MarketDataCache::default();

        // Initially empty.
        assert!(cache.get("AAPL").is_none());

        // Insert a snapshot.
        let snap = MarketSnapshot {
            last_price: Some(150.0),
            prev_close: Some(148.0),
            change_pct: Some(1.35),
            gatr_pct: None,
        };
        cache.insert("AAPL".to_string(), snap);

        // Retrieve it.
        let retrieved = cache.get("AAPL").expect("should find AAPL");
        assert_eq!(retrieved.last_price, Some(150.0));
        assert_eq!(retrieved.change_pct, Some(1.35));

        // Verify symbols iterator.
        let syms: Vec<&String> = cache.symbols().collect();
        assert_eq!(syms.len(), 1);
        assert_eq!(syms[0], "AAPL");

        // Remove and verify it's gone.
        cache.remove("AAPL");
        assert!(cache.get("AAPL").is_none());
        assert_eq!(cache.symbols().count(), 0);
    }
}
