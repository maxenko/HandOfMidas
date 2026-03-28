//! The `CandleData` trait abstracts over candle data sources.
//!
//! It lives in `midas-core` (the leaf crate) so that `midas-chart` can program
//! against it without depending on `midas-data`'s concrete `CandleBuffer` type.
//!
//! This enables:
//! - **Sans-IO chart logic**: `midas-chart` accepts `&dyn CandleData` or
//!   generics bounded by `CandleData`, keeping it free of storage dependencies.
//! - **Testing**: test fixtures can implement `CandleData` with hard-coded data.
//! - **Future streaming**: a real-time adapter wrapping a ring buffer or database
//!   cursor can implement `CandleData` without converting to `CandleBuffer`.

use std::ops::Range;

/// Trait abstracting over candle data sources.
///
/// Implemented by `CandleBuffer` (in `midas-data`), and potentially by
/// streaming adapters, database cursors, or test fixtures.
///
/// # Object Safety
///
/// This trait is object-safe: all methods take `&self` and return sized types,
/// so it can be used as `&dyn CandleData`.
pub trait CandleData {
    /// Total number of candles in the data source.
    fn len(&self) -> usize;

    /// Whether the data source contains zero candles.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Epoch-millisecond timestamp of the candle at `idx`.
    fn timestamp(&self, idx: usize) -> i64;

    /// Opening price of the candle at `idx`.
    fn open(&self, idx: usize) -> f32;

    /// Highest price of the candle at `idx`.
    fn high(&self, idx: usize) -> f32;

    /// Lowest price of the candle at `idx`.
    fn low(&self, idx: usize) -> f32;

    /// Closing price of the candle at `idx`.
    fn close(&self, idx: usize) -> f32;

    /// Trade volume for the candle at `idx`.
    fn volume(&self, idx: usize) -> u32;

    /// Min and max prices (low, high) across the given index range.
    ///
    /// Returns `(min_low, max_high)` for all candles in `range`.
    fn price_range(&self, range: Range<usize>) -> (f32, f32);

    /// Find the index of the candle whose timestamp is closest to `ts`
    /// (epoch milliseconds).
    ///
    /// If `ts` is before all data, returns 0.
    /// If `ts` is after all data, returns `len() - 1` (or 0 if empty).
    fn find_index_by_time(&self, ts: i64) -> usize;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal test fixture implementing `CandleData` for unit testing.
    struct TestCandles {
        timestamps: Vec<i64>,
        opens: Vec<f32>,
        highs: Vec<f32>,
        lows: Vec<f32>,
        closes: Vec<f32>,
        volumes: Vec<u32>,
    }

    impl TestCandles {
        fn sample() -> Self {
            Self {
                timestamps: vec![1000, 2000, 3000, 4000, 5000],
                opens:      vec![100.0, 101.0, 102.0, 103.0, 104.0],
                highs:      vec![105.0, 106.0, 107.0, 108.0, 109.0],
                lows:       vec![95.0,  96.0,  97.0,  98.0,  99.0],
                closes:     vec![101.0, 102.0, 103.0, 104.0, 105.0],
                volumes:    vec![1000, 2000, 3000, 4000, 5000],
            }
        }
    }

    impl CandleData for TestCandles {
        fn len(&self) -> usize {
            self.timestamps.len()
        }

        fn timestamp(&self, idx: usize) -> i64 {
            self.timestamps[idx]
        }

        fn open(&self, idx: usize) -> f32 {
            self.opens[idx]
        }

        fn high(&self, idx: usize) -> f32 {
            self.highs[idx]
        }

        fn low(&self, idx: usize) -> f32 {
            self.lows[idx]
        }

        fn close(&self, idx: usize) -> f32 {
            self.closes[idx]
        }

        fn volume(&self, idx: usize) -> u32 {
            self.volumes[idx]
        }

        fn price_range(&self, range: Range<usize>) -> (f32, f32) {
            let mut min = f32::MAX;
            let mut max = f32::MIN;
            for i in range {
                min = min.min(self.lows[i]);
                max = max.max(self.highs[i]);
            }
            (min, max)
        }

        fn find_index_by_time(&self, ts: i64) -> usize {
            match self.timestamps.binary_search(&ts) {
                Ok(idx) => idx,
                Err(idx) => idx.min(self.len().saturating_sub(1)),
            }
        }
    }

    #[test]
    fn trait_is_object_safe() {
        let candles = TestCandles::sample();
        let dyn_ref: &dyn CandleData = &candles;
        assert_eq!(dyn_ref.len(), 5);
        assert!(!dyn_ref.is_empty());
    }

    #[test]
    fn is_empty_default_impl() {
        let candles = TestCandles {
            timestamps: vec![],
            opens: vec![],
            highs: vec![],
            lows: vec![],
            closes: vec![],
            volumes: vec![],
        };
        assert!(candles.is_empty());
        assert_eq!(candles.len(), 0);
    }

    #[test]
    fn field_accessors() {
        let candles = TestCandles::sample();
        assert_eq!(candles.timestamp(0), 1000);
        assert_eq!(candles.open(2), 102.0);
        assert_eq!(candles.high(4), 109.0);
        assert_eq!(candles.low(0), 95.0);
        assert_eq!(candles.close(3), 104.0);
        assert_eq!(candles.volume(1), 2000);
    }

    #[test]
    fn price_range_subset() {
        let candles = TestCandles::sample();
        let (min, max) = candles.price_range(1..4);
        assert_eq!(min, 96.0);  // min of lows[1..4]
        assert_eq!(max, 108.0); // max of highs[1..4]
    }

    #[test]
    fn find_index_exact_match() {
        let candles = TestCandles::sample();
        assert_eq!(candles.find_index_by_time(3000), 2);
    }

    #[test]
    fn find_index_between_candles() {
        let candles = TestCandles::sample();
        // 2500 is between timestamp 2000 (idx 1) and 3000 (idx 2)
        // binary_search returns Err(2), clamped to 2
        assert_eq!(candles.find_index_by_time(2500), 2);
    }

    #[test]
    fn find_index_before_all() {
        let candles = TestCandles::sample();
        // 500 is before all timestamps
        assert_eq!(candles.find_index_by_time(500), 0);
    }

    #[test]
    fn find_index_after_all() {
        let candles = TestCandles::sample();
        // 9999 is after all timestamps
        assert_eq!(candles.find_index_by_time(9999), 4);
    }

    #[test]
    fn dyn_dispatch_works() {
        fn compute_avg_close(data: &dyn CandleData) -> f32 {
            if data.is_empty() {
                return 0.0;
            }
            let sum: f32 = (0..data.len()).map(|i| data.close(i)).sum();
            sum / data.len() as f32
        }

        let candles = TestCandles::sample();
        let avg = compute_avg_close(&candles);
        // closes: 101, 102, 103, 104, 105 -> avg = 103.0
        assert!((avg - 103.0).abs() < f32::EPSILON);
    }
}
