//! SoA (Structure of Arrays) candle storage for cache-friendly data access.
//!
//! [`CandleBuffer`] is the primary in-memory representation that the renderer
//! and indicator engine read from. It stores each OHLCV field in a separate
//! contiguous `Vec`, giving 8x better cache utilization for single-field scans
//! compared to AoS (Array of Structs) layout and enabling SIMD auto-vectorization.
//!
//! [`CandleSlice`] is a zero-copy borrowed view into a `CandleBuffer` (or a
//! sub-range thereof). It borrows slices of each field with no allocation.
//!
//! Both types implement [`CandleData`] from `midas-core`.

use std::ops::Range;

use midas_core::CandleData;

// ─── CandleBuffer ──────────────────────────────────────────────────────

/// Structure-of-Arrays candle buffer. Cache-friendly for rendering and
/// indicator computation. Each `Vec` has the same length.
///
/// # Invariants
///
/// - All six `Vec`s always have the same length.
/// - `timestamps` is monotonically increasing (enforced by `debug_assert!`
///   in [`push`](CandleBuffer::push)).
#[derive(Clone, Debug, Default)]
pub struct CandleBuffer {
    /// Epoch milliseconds, monotonically increasing.
    pub timestamps: Vec<i64>,
    /// Opening prices.
    pub opens: Vec<f32>,
    /// Highest prices.
    pub highs: Vec<f32>,
    /// Lowest prices.
    pub lows: Vec<f32>,
    /// Closing prices.
    pub closes: Vec<f32>,
    /// Trade volumes (capped at `u32::MAX` for equities).
    pub volumes: Vec<u32>,
}

impl CandleBuffer {
    /// Create a new empty `CandleBuffer`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a new `CandleBuffer` with pre-allocated capacity for `n` candles.
    pub fn with_capacity(n: usize) -> Self {
        Self {
            timestamps: Vec::with_capacity(n),
            opens: Vec::with_capacity(n),
            highs: Vec::with_capacity(n),
            lows: Vec::with_capacity(n),
            closes: Vec::with_capacity(n),
            volumes: Vec::with_capacity(n),
        }
    }

    /// Number of candles in the buffer.
    #[inline]
    pub fn len(&self) -> usize {
        self.timestamps.len()
    }

    /// Whether the buffer contains zero candles.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.timestamps.is_empty()
    }

    /// Append one candle to the buffer.
    ///
    /// # Panics (debug only)
    ///
    /// Panics in debug builds if `ts` is not strictly greater than the last
    /// timestamp, violating the monotonically-increasing invariant.
    pub fn push(&mut self, ts: i64, o: f32, h: f32, l: f32, c: f32, v: u32) {
        debug_assert!(
            self.timestamps.last().is_none_or(|&prev| ts > prev),
            "timestamps must be monotonically increasing: tried to push {ts} \
             after last = {}",
            self.timestamps.last().copied().unwrap_or(0),
        );
        self.timestamps.push(ts);
        self.opens.push(o);
        self.highs.push(h);
        self.lows.push(l);
        self.closes.push(c);
        self.volumes.push(v);
    }

    /// Borrow a sub-range as a [`CandleSlice`]. No allocation, no copy.
    ///
    /// # Panics
    ///
    /// Panics if `range` is out of bounds.
    pub fn slice(&self, range: Range<usize>) -> CandleSlice<'_> {
        CandleSlice {
            timestamps: &self.timestamps[range.clone()],
            opens: &self.opens[range.clone()],
            highs: &self.highs[range.clone()],
            lows: &self.lows[range.clone()],
            closes: &self.closes[range.clone()],
            volumes: &self.volumes[range],
        }
    }

    /// Return the `(min_low, max_high)` price range over a given index range.
    ///
    /// This is a hot path called every frame for Y-axis auto-scaling.
    /// The tight loops over contiguous `f32` arrays are auto-vectorized by LLVM
    /// with AVX2 on x86_64.
    ///
    /// # Panics
    ///
    /// Panics if `range` is out of bounds or empty.
    pub fn price_range(&self, range: Range<usize>) -> (f32, f32) {
        let highs = &self.highs[range.clone()];
        let lows = &self.lows[range];

        let mut min_low = f32::MAX;
        let mut max_high = f32::MIN;

        for &h in highs {
            if h > max_high {
                max_high = h;
            }
        }
        for &l in lows {
            if l < min_low {
                min_low = l;
            }
        }

        (min_low, max_high)
    }

    /// Binary search for the index of the first candle with timestamp >= `target_ts`.
    ///
    /// - If `target_ts` is before all data, returns `0`.
    /// - If `target_ts` is after all data, returns `len() - 1` (or `0` if empty).
    pub fn find_index_by_time(&self, target_ts: i64) -> usize {
        if self.is_empty() {
            return 0;
        }
        let idx = self.timestamps.partition_point(|&ts| ts < target_ts);
        idx.min(self.len() - 1)
    }

    /// Binary search for the index of the first candle with timestamp >= `target_ts`.
    ///
    /// Returns `len()` if all timestamps are less than `target_ts`.
    pub fn find_index_ge(&self, target_ts: i64) -> usize {
        self.timestamps.partition_point(|&ts| ts < target_ts)
    }

    /// Binary search for the index of the first candle with timestamp > `target_ts`.
    ///
    /// Returns `len()` if all timestamps are <= `target_ts`.
    pub fn find_index_gt(&self, target_ts: i64) -> usize {
        self.timestamps.partition_point(|&ts| ts <= target_ts)
    }

    /// Find the visible candle index range for a given time window.
    ///
    /// Returns a `Range<usize>` spanning all candles whose timestamps fall
    /// within `[start_ts, end_ts]`.
    pub fn visible_range(&self, start_ts: i64, end_ts: i64) -> Range<usize> {
        let lo = self.find_index_ge(start_ts);
        let hi = self.find_index_gt(end_ts);
        lo..hi
    }

    /// Replace the last candle (for forming candle updates in real-time mode).
    ///
    /// Does nothing if the buffer is empty.
    pub fn update_last(&mut self, ts: i64, o: f32, h: f32, l: f32, c: f32, v: u32) {
        if let Some(last) = self.timestamps.last_mut() {
            *last = ts;
            *self.opens.last_mut().expect("opens out of sync") = o;
            *self.highs.last_mut().expect("highs out of sync") = h;
            *self.lows.last_mut().expect("lows out of sync") = l;
            *self.closes.last_mut().expect("closes out of sync") = c;
            *self.volumes.last_mut().expect("volumes out of sync") = v;
        }
    }
}

impl CandleData for CandleBuffer {
    fn len(&self) -> usize {
        self.len()
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
        self.price_range(range)
    }

    fn find_index_by_time(&self, ts: i64) -> usize {
        self.find_index_by_time(ts)
    }
}

// ─── CandleSlice ───────────────────────────────────────────────────────

/// Borrowed zero-copy view into a [`CandleBuffer`] or a sub-range thereof.
///
/// No allocation, no copy. The lifetime `'a` is tied to the source buffer.
#[derive(Copy, Clone, Debug)]
pub struct CandleSlice<'a> {
    /// Epoch-millisecond timestamps.
    pub timestamps: &'a [i64],
    /// Opening prices.
    pub opens: &'a [f32],
    /// Highest prices.
    pub highs: &'a [f32],
    /// Lowest prices.
    pub lows: &'a [f32],
    /// Closing prices.
    pub closes: &'a [f32],
    /// Trade volumes.
    pub volumes: &'a [u32],
}

impl<'a> CandleSlice<'a> {
    /// Number of candles in the slice.
    #[inline]
    pub fn len(&self) -> usize {
        self.timestamps.len()
    }

    /// Whether the slice contains zero candles.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.timestamps.is_empty()
    }

    /// Return the `(min_low, max_high)` price range over a given index range
    /// (relative to this slice).
    ///
    /// # Panics
    ///
    /// Panics if `range` is out of bounds or empty.
    pub fn price_range(&self, range: Range<usize>) -> (f32, f32) {
        let highs = &self.highs[range.clone()];
        let lows = &self.lows[range];

        let mut min_low = f32::MAX;
        let mut max_high = f32::MIN;

        for &h in highs {
            if h > max_high {
                max_high = h;
            }
        }
        for &l in lows {
            if l < min_low {
                min_low = l;
            }
        }

        (min_low, max_high)
    }

    /// Binary search for the index of the first candle with timestamp >= `target_ts`.
    ///
    /// - If `target_ts` is before all data, returns `0`.
    /// - If `target_ts` is after all data, returns `len() - 1` (or `0` if empty).
    pub fn find_index_by_time(&self, target_ts: i64) -> usize {
        if self.is_empty() {
            return 0;
        }
        let idx = self.timestamps.partition_point(|&ts| ts < target_ts);
        idx.min(self.len() - 1)
    }

    /// Borrow a further sub-range of this slice. No allocation, no copy.
    ///
    /// # Panics
    ///
    /// Panics if `range` is out of bounds.
    pub fn slice(&self, range: Range<usize>) -> CandleSlice<'a> {
        CandleSlice {
            timestamps: &self.timestamps[range.clone()],
            opens: &self.opens[range.clone()],
            highs: &self.highs[range.clone()],
            lows: &self.lows[range.clone()],
            closes: &self.closes[range.clone()],
            volumes: &self.volumes[range],
        }
    }
}

impl CandleData for CandleSlice<'_> {
    fn len(&self) -> usize {
        self.len()
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
        self.price_range(range)
    }

    fn find_index_by_time(&self, ts: i64) -> usize {
        self.find_index_by_time(ts)
    }
}

// ─── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a sample CandleBuffer with 5 candles for testing.
    fn sample_buffer() -> CandleBuffer {
        let mut buf = CandleBuffer::with_capacity(5);
        buf.push(1000, 100.0, 105.0, 95.0, 101.0, 1000);
        buf.push(2000, 101.0, 106.0, 96.0, 102.0, 2000);
        buf.push(3000, 102.0, 107.0, 97.0, 103.0, 3000);
        buf.push(4000, 103.0, 108.0, 98.0, 104.0, 4000);
        buf.push(5000, 104.0, 109.0, 99.0, 105.0, 5000);
        buf
    }

    // ── Construction and basic accessors ───────────────────────────────

    #[test]
    fn new_buffer_is_empty() {
        let buf = CandleBuffer::new();
        assert!(buf.is_empty());
        assert_eq!(buf.len(), 0);
    }

    #[test]
    fn with_capacity_is_empty() {
        let buf = CandleBuffer::with_capacity(100);
        assert!(buf.is_empty());
        assert_eq!(buf.len(), 0);
    }

    #[test]
    fn default_is_empty() {
        let buf = CandleBuffer::default();
        assert!(buf.is_empty());
        assert_eq!(buf.len(), 0);
    }

    #[test]
    fn push_increases_len() {
        let buf = sample_buffer();
        assert_eq!(buf.len(), 5);
        assert!(!buf.is_empty());
    }

    #[test]
    fn field_access_after_push() {
        let buf = sample_buffer();
        assert_eq!(buf.timestamps[0], 1000);
        assert_eq!(buf.opens[2], 102.0);
        assert_eq!(buf.highs[4], 109.0);
        assert_eq!(buf.lows[0], 95.0);
        assert_eq!(buf.closes[3], 104.0);
        assert_eq!(buf.volumes[1], 2000);
    }

    // ── price_range ────────────────────────────────────────────────────

    #[test]
    fn price_range_full() {
        let buf = sample_buffer();
        let (min, max) = buf.price_range(0..5);
        assert_eq!(min, 95.0);
        assert_eq!(max, 109.0);
    }

    #[test]
    fn price_range_subset() {
        let buf = sample_buffer();
        let (min, max) = buf.price_range(1..4);
        // lows[1..4] = [96, 97, 98], min = 96
        // highs[1..4] = [106, 107, 108], max = 108
        assert_eq!(min, 96.0);
        assert_eq!(max, 108.0);
    }

    #[test]
    fn price_range_single_candle() {
        let buf = sample_buffer();
        let (min, max) = buf.price_range(2..3);
        assert_eq!(min, 97.0);
        assert_eq!(max, 107.0);
    }

    // ── find_index_by_time ─────────────────────────────────────────────

    #[test]
    fn find_index_exact_match() {
        let buf = sample_buffer();
        assert_eq!(buf.find_index_by_time(3000), 2);
    }

    #[test]
    fn find_index_between_candles() {
        let buf = sample_buffer();
        // 2500 is between 2000 (idx 1) and 3000 (idx 2)
        // partition_point returns 2 (first ts >= 2500)
        assert_eq!(buf.find_index_by_time(2500), 2);
    }

    #[test]
    fn find_index_before_all() {
        let buf = sample_buffer();
        assert_eq!(buf.find_index_by_time(500), 0);
    }

    #[test]
    fn find_index_after_all() {
        let buf = sample_buffer();
        // After all timestamps: clamped to len()-1 = 4
        assert_eq!(buf.find_index_by_time(9999), 4);
    }

    #[test]
    fn find_index_empty_buffer() {
        let buf = CandleBuffer::new();
        assert_eq!(buf.find_index_by_time(1000), 0);
    }

    // ── find_index_ge / find_index_gt ──────────────────────────────────

    #[test]
    fn find_index_ge_exact() {
        let buf = sample_buffer();
        assert_eq!(buf.find_index_ge(3000), 2);
    }

    #[test]
    fn find_index_ge_between() {
        let buf = sample_buffer();
        assert_eq!(buf.find_index_ge(2500), 2);
    }

    #[test]
    fn find_index_ge_after_all() {
        let buf = sample_buffer();
        assert_eq!(buf.find_index_ge(9999), 5); // returns len()
    }

    #[test]
    fn find_index_gt_exact() {
        let buf = sample_buffer();
        // First index with timestamp > 3000 is idx 3 (ts=4000)
        assert_eq!(buf.find_index_gt(3000), 3);
    }

    // ── visible_range ──────────────────────────────────────────────────

    #[test]
    fn visible_range_subset() {
        let buf = sample_buffer();
        let range = buf.visible_range(2000, 4000);
        // ge(2000) = 1, gt(4000) = 4
        assert_eq!(range, 1..4);
    }

    #[test]
    fn visible_range_all() {
        let buf = sample_buffer();
        let range = buf.visible_range(0, 99999);
        assert_eq!(range, 0..5);
    }

    // ── update_last ────────────────────────────────────────────────────

    #[test]
    fn update_last_modifies_values() {
        let mut buf = sample_buffer();
        buf.update_last(5000, 110.0, 115.0, 105.0, 112.0, 9999);
        assert_eq!(buf.timestamps[4], 5000);
        assert_eq!(buf.opens[4], 110.0);
        assert_eq!(buf.highs[4], 115.0);
        assert_eq!(buf.lows[4], 105.0);
        assert_eq!(buf.closes[4], 112.0);
        assert_eq!(buf.volumes[4], 9999);
    }

    #[test]
    fn update_last_on_empty_does_nothing() {
        let mut buf = CandleBuffer::new();
        buf.update_last(1000, 1.0, 2.0, 0.5, 1.5, 100);
        assert!(buf.is_empty());
    }

    // ── CandleSlice ────────────────────────────────────────────────────

    #[test]
    fn slice_borrows_correctly() {
        let buf = sample_buffer();
        let sl = buf.slice(1..4);
        assert_eq!(sl.len(), 3);
        assert!(!sl.is_empty());
        assert_eq!(sl.timestamps, &[2000, 3000, 4000]);
        assert_eq!(sl.opens, &[101.0, 102.0, 103.0]);
        assert_eq!(sl.highs, &[106.0, 107.0, 108.0]);
        assert_eq!(sl.lows, &[96.0, 97.0, 98.0]);
        assert_eq!(sl.closes, &[102.0, 103.0, 104.0]);
        assert_eq!(sl.volumes, &[2000, 3000, 4000]);
    }

    #[test]
    fn slice_full_range() {
        let buf = sample_buffer();
        let sl = buf.slice(0..5);
        assert_eq!(sl.len(), 5);
    }

    #[test]
    fn slice_empty_range() {
        let buf = sample_buffer();
        let sl = buf.slice(2..2);
        assert!(sl.is_empty());
        assert_eq!(sl.len(), 0);
    }

    #[test]
    fn slice_price_range() {
        let buf = sample_buffer();
        let sl = buf.slice(1..4);
        let (min, max) = sl.price_range(0..3);
        assert_eq!(min, 96.0);
        assert_eq!(max, 108.0);
    }

    #[test]
    fn slice_find_index_by_time() {
        let buf = sample_buffer();
        let sl = buf.slice(1..4); // timestamps: [2000, 3000, 4000]
        assert_eq!(sl.find_index_by_time(3000), 1); // exact match at local idx 1
        assert_eq!(sl.find_index_by_time(2500), 1); // between 2000 and 3000
        assert_eq!(sl.find_index_by_time(1000), 0); // before all
        assert_eq!(sl.find_index_by_time(9999), 2); // after all, clamped
    }

    #[test]
    fn slice_of_slice() {
        let buf = sample_buffer();
        let sl = buf.slice(0..5);
        let sub = sl.slice(1..3);
        assert_eq!(sub.len(), 2);
        assert_eq!(sub.timestamps, &[2000, 3000]);
    }

    #[test]
    fn slice_is_copy() {
        let buf = sample_buffer();
        let sl = buf.slice(0..3);
        let sl2 = sl; // Copy
        assert_eq!(sl.len(), sl2.len());
    }

    // ── CandleData trait via dyn dispatch ──────────────────────────────

    #[test]
    fn candle_data_trait_on_buffer() {
        let buf = sample_buffer();
        let dyn_ref: &dyn CandleData = &buf;
        assert_eq!(dyn_ref.len(), 5);
        assert!(!dyn_ref.is_empty());
        assert_eq!(dyn_ref.timestamp(0), 1000);
        assert_eq!(dyn_ref.open(2), 102.0);
        assert_eq!(dyn_ref.high(4), 109.0);
        assert_eq!(dyn_ref.low(0), 95.0);
        assert_eq!(dyn_ref.close(3), 104.0);
        assert_eq!(dyn_ref.volume(1), 2000);
    }

    #[test]
    fn candle_data_trait_price_range() {
        let buf = sample_buffer();
        let dyn_ref: &dyn CandleData = &buf;
        let (min, max) = dyn_ref.price_range(1..4);
        assert_eq!(min, 96.0);
        assert_eq!(max, 108.0);
    }

    #[test]
    fn candle_data_trait_find_index() {
        let buf = sample_buffer();
        let dyn_ref: &dyn CandleData = &buf;
        assert_eq!(dyn_ref.find_index_by_time(3000), 2);
        assert_eq!(dyn_ref.find_index_by_time(500), 0);
        assert_eq!(dyn_ref.find_index_by_time(9999), 4);
    }

    #[test]
    fn candle_data_trait_on_slice() {
        let buf = sample_buffer();
        let sl = buf.slice(1..4);
        let dyn_ref: &dyn CandleData = &sl;
        assert_eq!(dyn_ref.len(), 3);
        assert_eq!(dyn_ref.timestamp(0), 2000);
        assert_eq!(dyn_ref.open(1), 102.0);
        assert_eq!(dyn_ref.volume(2), 4000);
    }

    #[test]
    fn candle_data_trait_on_empty_buffer() {
        let buf = CandleBuffer::new();
        let dyn_ref: &dyn CandleData = &buf;
        assert!(dyn_ref.is_empty());
        assert_eq!(dyn_ref.len(), 0);
        assert_eq!(dyn_ref.find_index_by_time(1000), 0);
    }

    /// Verify that a generic function bounded by CandleData works with both
    /// CandleBuffer and CandleSlice.
    #[test]
    fn generic_function_over_candle_data() {
        fn average_close(data: &dyn CandleData) -> f32 {
            if data.is_empty() {
                return 0.0;
            }
            let sum: f32 = (0..data.len()).map(|i| data.close(i)).sum();
            sum / data.len() as f32
        }

        let buf = sample_buffer();
        let avg_buf = average_close(&buf);
        // closes: 101, 102, 103, 104, 105 -> avg = 103.0
        assert!((avg_buf - 103.0).abs() < f32::EPSILON);

        let sl = buf.slice(1..4);
        let avg_sl = average_close(&sl);
        // closes: 102, 103, 104 -> avg = 103.0
        assert!((avg_sl - 103.0).abs() < f32::EPSILON);
    }

    // ── Single candle edge case ────────────────────────────────────────

    #[test]
    fn single_candle_buffer() {
        let mut buf = CandleBuffer::new();
        buf.push(1000, 50.0, 55.0, 45.0, 52.0, 500);
        assert_eq!(buf.len(), 1);
        assert!(!buf.is_empty());

        let (min, max) = buf.price_range(0..1);
        assert_eq!(min, 45.0);
        assert_eq!(max, 55.0);

        assert_eq!(buf.find_index_by_time(500), 0);
        assert_eq!(buf.find_index_by_time(1000), 0);
        assert_eq!(buf.find_index_by_time(2000), 0);
    }

    #[test]
    fn single_candle_slice() {
        let mut buf = CandleBuffer::new();
        buf.push(1000, 50.0, 55.0, 45.0, 52.0, 500);
        let sl = buf.slice(0..1);
        assert_eq!(sl.len(), 1);
        assert_eq!(sl.find_index_by_time(999), 0);
        assert_eq!(sl.find_index_by_time(1000), 0);
        assert_eq!(sl.find_index_by_time(1001), 0);
    }

    // ── Clone ──────────────────────────────────────────────────────────

    #[test]
    fn buffer_clone() {
        let buf = sample_buffer();
        let clone = buf.clone();
        assert_eq!(clone.len(), buf.len());
        assert_eq!(clone.timestamps, buf.timestamps);
        assert_eq!(clone.closes, buf.closes);
    }
}
