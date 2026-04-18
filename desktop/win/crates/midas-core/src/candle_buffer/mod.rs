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
use std::sync::atomic::{AtomicU64, Ordering};

use crate::CandleData;

// ─── CandleBuffer ──────────────────────────────────────────────────────

/// Structure-of-Arrays candle buffer. Cache-friendly for rendering and
/// indicator computation. Each `Vec` has the same length.
///
/// # Invariants
///
/// - All six `Vec`s always have the same length.
/// - `timestamps` is monotonically increasing (enforced by `debug_assert!`
///   in [`push`](CandleBuffer::push)).
///
/// # Version counter
///
/// Every mutation method ([`push`](CandleBuffer::push),
/// [`update_last`](CandleBuffer::update_last)) bumps a monotonically
/// increasing `version: AtomicU64`. Downstream caches (e.g.
/// `ThumbnailDataStore`) store the version at slice time and reslice
/// when [`version`](CandleBuffer::version) has advanced. The counter
/// uses `Ordering::Relaxed` — it orders no other memory, it only
/// signals "something changed". Mirrors the `midas-chart::dirty`
/// generation-counter idiom.
#[derive(Debug, Default)]
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
    /// Monotonic mutation counter. Bumped on every `push` /
    /// `update_last`. Readers compare a saved value to detect change.
    /// Not `Clone`; see the manual `Clone` impl below.
    version: AtomicU64,
}

impl Clone for CandleBuffer {
    /// Clone the buffer, copying the current version counter so the
    /// clone starts at the same generation as the source. This matches
    /// the expectation that a clone is observably identical — a
    /// version-aware reader that has already synced to the source
    /// should also be synced to the clone.
    fn clone(&self) -> Self {
        Self {
            timestamps: self.timestamps.clone(),
            opens: self.opens.clone(),
            highs: self.highs.clone(),
            lows: self.lows.clone(),
            closes: self.closes.clone(),
            volumes: self.volumes.clone(),
            version: AtomicU64::new(self.version.load(Ordering::Relaxed)),
        }
    }
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
            version: AtomicU64::new(0),
        }
    }

    /// Monotonic version counter. Bumped on every mutation
    /// (`push`, `update_last`). Readers can compare a saved
    /// value to detect whether the buffer has changed since
    /// they last read it. Uses `Ordering::Relaxed` — the
    /// counter is not ordering any other memory, it only
    /// signals "something changed".
    #[inline]
    pub fn version(&self) -> u64 {
        self.version.load(Ordering::Relaxed)
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
        self.version.fetch_add(1, Ordering::Relaxed);
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
            self.version.fetch_add(1, Ordering::Relaxed);
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

#[cfg(test)]
mod tests;
