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

use midas_bars::SessionKindByte;

use crate::CandleData;

/// Convert a raw `sessions[idx]` byte back into a [`SessionKindByte`].
///
/// `SessionKind` is `#[repr(u8)]` and `#[non_exhaustive]`. The wildcard
/// arm guards against future variants leaking in via legacy data —
/// unknown bytes degrade to `Regular`, which makes them visually
/// indistinguishable from RTH on the band-render path. Anything written
/// through [`CandleBuffer::push_with_session`] uses `as u8` on a known
/// variant, so corruption is the only path that hits the wildcard.
#[inline]
fn session_kind_from_u8(b: u8) -> SessionKindByte {
    match b {
        x if x == SessionKindByte::Regular as u8 => SessionKindByte::Regular,
        x if x == SessionKindByte::PreMarket as u8 => SessionKindByte::PreMarket,
        x if x == SessionKindByte::PostMarket as u8 => SessionKindByte::PostMarket,
        x if x == SessionKindByte::Break as u8 => SessionKindByte::Break,
        x if x == SessionKindByte::Overnight as u8 => SessionKindByte::Overnight,
        x if x == SessionKindByte::Closed as u8 => SessionKindByte::Closed,
        _ => SessionKindByte::Regular,
    }
}

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
    /// Trading session kind, one byte per row (`SessionKind as u8`).
    /// Drives the legacy chart's session-band overlay (ETH shading);
    /// see `compute_session_bands` in `midas-chart`. Loaders that lack
    /// symbol context (`midas-data` binary readers) populate this with
    /// `SessionKind::Regular as u8`. The host classifies via the
    /// resolved exchange calendar at conversion time
    /// (`bars_to_candle_buffer`).
    pub sessions: Vec<u8>,
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
            sessions: self.sessions.clone(),
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
            sessions: Vec::with_capacity(n),
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
    /// Defaults the new row's session to [`SessionKindByte::Regular`].
    /// Callers that know the session kind (host conversion via the
    /// resolved exchange calendar) should use
    /// [`push_with_session`](Self::push_with_session) instead.
    ///
    /// # Panics (debug only)
    ///
    /// Panics in debug builds if `ts` is not strictly greater than the last
    /// timestamp, violating the monotonically-increasing invariant.
    pub fn push(&mut self, ts: i64, o: f32, h: f32, l: f32, c: f32, v: u32) {
        self.push_with_session(ts, o, h, l, c, v, SessionKindByte::Regular);
    }

    /// Append one candle to the buffer with an explicit session kind.
    ///
    /// # Panics (debug only)
    ///
    /// Panics in debug builds if `ts` is not strictly greater than the last
    /// timestamp, violating the monotonically-increasing invariant.
    #[allow(clippy::too_many_arguments)]
    pub fn push_with_session(
        &mut self,
        ts: i64,
        o: f32,
        h: f32,
        l: f32,
        c: f32,
        v: u32,
        session: SessionKindByte,
    ) {
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
        self.sessions.push(session as u8);
        self.version.fetch_add(1, Ordering::Relaxed);
    }

    /// Read the session kind for the candle at `idx`.
    ///
    /// Bytes that don't match a known [`SessionKindByte`] variant
    /// degrade to `Regular` — see `session_kind_from_u8`.
    ///
    /// # Panics
    ///
    /// Panics if `idx` is out of bounds.
    #[inline]
    pub fn session_kind(&self, idx: usize) -> SessionKindByte {
        session_kind_from_u8(self.sessions[idx])
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
            volumes: &self.volumes[range.clone()],
            sessions: &self.sessions[range],
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

    /// Fold a completed (or partial) bar into the buffer.
    ///
    /// If the last candle's open timestamp matches `bar_ts_open_ms`,
    /// the last candle is overwritten in place (the aggregator has
    /// re-emitted the same window with updated OHLCV). Otherwise a
    /// new candle is appended. In either case the version counter
    /// advances so downstream caches reslice.
    ///
    /// Timestamps are in epoch milliseconds. Volume is saturated at
    /// `u32::MAX` — upstream volumes are `u64`, but the buffer's
    /// storage is `u32` (equities rarely approach 4 B shares in a
    /// single bar).
    ///
    /// Introduced in S7b as the replacement for the removed
    /// `apply_tick`: the router emits per-bar events (from the
    /// aggregator or the realtime-bar publisher), not ticks.
    ///
    /// Callers supply a `session` value already classified by the
    /// host's resolved exchange calendar (see
    /// `bars_to_candle_buffer` and `apply_bar_to_buffer`). The
    /// overwrite-in-place branch refreshes `sessions[last]` so an
    /// aggregator re-emit that crosses a session boundary
    /// (e.g. the 09:30 ET pre-market → regular flip on the bar
    /// that brackets it) lands the new classification rather than
    /// being pinned to whatever the first emit reported.
    #[allow(clippy::too_many_arguments)]
    pub fn apply_bar(
        &mut self,
        bar_ts_open_ms: i64,
        o: f32,
        h: f32,
        l: f32,
        c: f32,
        v: u32,
        session: SessionKindByte,
    ) {
        match self.timestamps.last().copied() {
            Some(ts) if ts == bar_ts_open_ms => {
                *self.opens.last_mut().expect("opens out of sync") = o;
                *self.highs.last_mut().expect("highs out of sync") = h;
                *self.lows.last_mut().expect("lows out of sync") = l;
                *self.closes.last_mut().expect("closes out of sync") = c;
                *self.volumes.last_mut().expect("volumes out of sync") = v;
                *self.sessions.last_mut().expect("sessions out of sync") = session as u8;
                self.version.fetch_add(1, Ordering::Relaxed);
            }
            Some(ts) if ts > bar_ts_open_ms => {
                // Out-of-order bar — ignore. The router's aggregator
                // is expected to emit monotonically, so this branch
                // firing indicates a bug upstream; log and drop.
                tracing::warn!(
                    last_ts = ts,
                    incoming = bar_ts_open_ms,
                    "apply_bar: dropping out-of-order bar"
                );
            }
            _ => {
                self.push_with_session(bar_ts_open_ms, o, h, l, c, v, session);
            }
        }
    }

    /// Tick-rate update of the last candle's close price.
    ///
    /// Extends the current candle in place: `close = price`, `high =
    /// max(high, price)`, `low = min(low, price)`. Volume, open, and
    /// timestamp are not touched. Drives the chart at quote cadence
    /// (~250 ms default — the same funnel the watchlist reads from)
    /// between authoritative bar emissions so the last candle visibly
    /// tracks the watchlist price instead of lagging by the
    /// bar-stream sampling interval (typically 5 s on the sim and
    /// `reqRealTimeBars`).
    ///
    /// No-op when the buffer is empty — the first bar has to arrive
    /// through `push` / `apply_bar` / `merge_bar` before ticks can
    /// refine it. No-op on non-finite prices to stay robust against
    /// upstream NaN / Inf.
    pub fn update_last_price(&mut self, price: f32) {
        if !price.is_finite() {
            return;
        }
        if self.closes.is_empty() {
            return;
        }
        let h_last = self.highs.last_mut().expect("highs out of sync");
        if price > *h_last {
            *h_last = price;
        }
        let l_last = self.lows.last_mut().expect("lows out of sync");
        if price < *l_last {
            *l_last = price;
        }
        *self.closes.last_mut().expect("closes out of sync") = price;
        self.version.fetch_add(1, Ordering::Relaxed);
    }

    /// Incrementally merge a sub-bucket bar into the current bucket.
    ///
    /// Where `apply_bar` is an authoritative overwrite (the aggregator
    /// re-emits the same window with refreshed OHLCV), `merge_bar` is
    /// the accumulator: callers feeding 5-second RT bars into a D1
    /// chart floor the incoming timestamp to the chart's bucket and
    /// call here. If the last candle's ts matches, OHLC folds as
    /// "open stays, high = max, low = min, close = incoming close,
    /// volume += incoming". If not, the bucket is new and the bar is
    /// pushed with the incoming values as-is.
    ///
    /// Used by the chart-subscription fallback for timeframes the
    /// aggregator rejects (D1/W1/MN1/H4/S1) — the legacy aggregator
    /// can't synthesise those from 5 s bars without a trading
    /// calendar, so the chart does the merge itself until it migrates
    /// to the session-aware aggregator path.
    ///
    /// `session` is classified by the host's resolved calendar from
    /// the *incoming sub-bar's* timestamp; the same-bucket branch
    /// keeps the existing `sessions[last]` (the bucket inherits the
    /// session of its first sub-bar, which matches the legacy chart's
    /// "the bar belongs to its open" convention).
    #[allow(clippy::too_many_arguments)]
    pub fn merge_bar(
        &mut self,
        bucket_ts_open_ms: i64,
        o: f32,
        h: f32,
        l: f32,
        c: f32,
        v: u32,
        session: SessionKindByte,
    ) {
        match self.timestamps.last().copied() {
            Some(ts) if ts == bucket_ts_open_ms => {
                // Same bucket — accumulate OHLCV.
                let h_last = self.highs.last_mut().expect("highs out of sync");
                if h > *h_last {
                    *h_last = h;
                }
                let l_last = self.lows.last_mut().expect("lows out of sync");
                if l < *l_last {
                    *l_last = l;
                }
                *self.closes.last_mut().expect("closes out of sync") = c;
                let v_last = self.volumes.last_mut().expect("volumes out of sync");
                *v_last = v_last.saturating_add(v);
                self.version.fetch_add(1, Ordering::Relaxed);
            }
            Some(ts) if ts > bucket_ts_open_ms => {
                tracing::warn!(
                    last_ts = ts,
                    incoming = bucket_ts_open_ms,
                    "merge_bar: dropping out-of-order sub-bar"
                );
            }
            _ => {
                self.push_with_session(bucket_ts_open_ms, o, h, l, c, v, session);
            }
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

    fn session_kind(&self, idx: usize) -> SessionKindByte {
        self.session_kind(idx)
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
    /// Trading session kinds, one byte per row (`SessionKind as u8`).
    pub sessions: &'a [u8],
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
            volumes: &self.volumes[range.clone()],
            sessions: &self.sessions[range],
        }
    }

    /// Read the session kind for the candle at `idx` (relative to this slice).
    ///
    /// # Panics
    ///
    /// Panics if `idx` is out of bounds.
    #[inline]
    pub fn session_kind(&self, idx: usize) -> SessionKindByte {
        session_kind_from_u8(self.sessions[idx])
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

    fn session_kind(&self, idx: usize) -> SessionKindByte {
        self.session_kind(idx)
    }
}

#[cfg(test)]
mod tests;
