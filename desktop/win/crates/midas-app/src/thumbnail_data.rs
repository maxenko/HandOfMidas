//! Lazy per-(symbol, timeframe) cache of the last-N close prices used
//! by the thumbnail widget.
//!
//! Data is loaded on demand through the existing `DataProvider` trait
//! (`TestProvider` today, real IB tomorrow). When the underlying
//! [`CandleBuffer`] mutates (via the broker bridge), its
//! [`CandleBuffer::version`] advances and the next
//! [`fetch`](ThumbnailDataStore::fetch) reslices.
//!
//! See `plan/feature-chart-thumbnail-cells.md` Decision 4 + 9.
//!
//! ## Slice 8d — `CandleSeries` alternate source
//!
//! The store accepts two source shapes:
//!
//! - Legacy [`CandleBuffer`] — primary path today, retained through
//!   slice 9c so the transition window still services the main chart.
//! - New-stack `Arc<RwLock<CandleSeries>>` via [`Self::install_series`]
//!   / [`Self::fetch_from_series`] — lands in slice 8d so the
//!   watchlist can seed thumbnails from a live session-chart panel's
//!   series without going through the legacy buffer path.
//!
//! Both paths produce an identical `Vec<f32>` of closes for the same
//! underlying data. The `ThemePalette` that tints the thumbnail is
//! shared with the main chart (plan R9) — see [`crate::theme::ThumbnailPalette`].

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use midas_core::{CandleBuffer, Timeframe};

/// Default tail length — the number of trailing closes retained per
/// entry. 100 is enough for a readable thumbnail at the target cell
/// width (~100 px) while keeping per-entry memory ~400 bytes of f32s.
pub const DEFAULT_TAIL_LEN: usize = 100;

/// Maximum number of thumbnail loads allowed in flight at once.
///
/// Bounds concurrent calls into `DataProvider::get_candles`. `TestProvider`
/// is CPU-bound and serialises through a `parking_lot::Mutex`, so the cap
/// chiefly prevents burst backpressure that would stall the UI thread; for
/// a real rate-limited provider (e.g. IB with ~50 msg/s), the cap keeps
/// startup prewarm from exceeding the budget. Overflow is queued in
/// [`ThumbnailDataStore::waiting`] and drained as loads complete.
pub const DEFAULT_MAX_CONCURRENT_LOADS: usize = 6;

/// Cached entry for a single (symbol, timeframe) pair.
///
/// `closes` is wrapped in [`Arc`] so `fetch` can hand out cheap
/// pointer-copies to multiple callers without cloning the `Vec` — a
/// re-slice only occurs when the source buffer's version advances.
#[derive(Debug, Clone)]
#[allow(dead_code)] // fields consumed by Slice 4 (thumbnail widget snapshot)
pub struct Entry {
    /// Last-N close prices, oldest first. Empty when the entry is a
    /// placeholder (source buffer not yet loaded).
    pub closes: Arc<Vec<f32>>,
    /// Minimum low price over the sliced range, for Y-axis auto-scale.
    /// `0.0` for empty entries.
    pub y_min: f32,
    /// Maximum high price over the sliced range, for Y-axis auto-scale.
    /// `1.0` for empty entries.
    pub y_max: f32,
    /// [`CandleBuffer::version`] at slice time. Compared against the
    /// live counter to detect stale slices. `0` for empty placeholder
    /// entries that were created before any data loaded.
    pub source_version: u64,
}

/// Describes a load that the caller (app shell) should kick off.
///
/// Returned by [`ThumbnailDataStore::request_load`] when no load is
/// in flight for the given `(symbol, tf)`. The app converts this to
/// `tokio::spawn(provider.get_candles(symbol, tf, days))` and feeds
/// the resulting buffer back via [`ThumbnailDataStore::install`].
#[derive(Debug, Clone)]
#[allow(dead_code)] // fields consumed by Slice 4 (load-dispatch site)
pub struct LoadTask {
    /// Symbol to load.
    pub symbol: String,
    /// Timeframe to load.
    pub tf: Timeframe,
    /// Number of days of history the provider should return. The
    /// caller chooses the heuristic (e.g. `M1 → 1`, `D1 → 180`).
    pub days: u32,
}

/// Per-(symbol, timeframe) close-slice cache with generation-counter
/// invalidation.
///
/// The store is mutated only on the main (view) thread; async loads
/// return via a `Message::ThumbnailDataReady` handler which also runs
/// on the main thread, so no internal locking is required.
#[derive(Debug)]
pub struct ThumbnailDataStore {
    /// Sliced-close cache, keyed by (symbol, timeframe).
    cache: HashMap<(String, Timeframe), Entry>,
    /// Keys for which an async load has been dispatched but not yet
    /// resolved. Prevents dispatching a duplicate load.
    pending: HashSet<(String, Timeframe)>,
    /// Queued loads waiting for a `pending` slot to free up. Drained
    /// FIFO via [`drain_next`](Self::drain_next) after each completed
    /// load. Deduped on enqueue against both `pending` and existing
    /// queue entries, so repeated [`request_load`](Self::request_load)
    /// calls for the same key never produce duplicates.
    waiting: VecDeque<LoadTask>,
    /// Maximum number of loads permitted in `pending` at once.
    /// Defaults to [`DEFAULT_MAX_CONCURRENT_LOADS`].
    max_concurrent: usize,
    /// Tail length the store reslices to on every refresh.
    tail_len: usize,
}

// The methods below (other than `new`/`default` + `install`, which
// Slice 3 wires via `Message::ThumbnailDataReady`) are consumed by
// Slice 4's grid integration. They are covered by the unit tests in
// this file and intentionally exposed ahead of that wiring.
#[allow(dead_code)]
impl ThumbnailDataStore {
    /// Create a new store with the default [`DEFAULT_TAIL_LEN`] and
    /// [`DEFAULT_MAX_CONCURRENT_LOADS`].
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
            pending: HashSet::new(),
            waiting: VecDeque::new(),
            max_concurrent: DEFAULT_MAX_CONCURRENT_LOADS,
            tail_len: DEFAULT_TAIL_LEN,
        }
    }

    /// Create a new store with an explicit tail length.
    pub fn with_tail_len(n: usize) -> Self {
        Self {
            cache: HashMap::new(),
            pending: HashSet::new(),
            waiting: VecDeque::new(),
            max_concurrent: DEFAULT_MAX_CONCURRENT_LOADS,
            tail_len: n,
        }
    }

    /// Override the concurrent-load cap. Primarily for tests.
    pub fn with_max_concurrent(mut self, max: usize) -> Self {
        self.max_concurrent = max.max(1);
        self
    }

    /// Return the cached [`Entry`] for `(symbol, tf)`, reslicing if
    /// `source` has advanced past the cached [`Entry::source_version`].
    ///
    /// - If `source` is `Some(buf)` and no cache entry exists, or the
    ///   entry is stale, reslice from `buf` and store.
    /// - If `source` is `None` and no cache entry exists, return an
    ///   empty placeholder (version `0`) without inserting — the
    ///   caller should trigger [`request_load`](Self::request_load).
    /// - Otherwise return the existing entry unchanged (cheap
    ///   [`Arc`] clone).
    pub fn fetch(&mut self, symbol: &str, tf: Timeframe, source: Option<&CandleBuffer>) -> Entry {
        let key = (symbol.to_string(), tf);

        if let Some(buf) = source {
            let needs_reslice = self
                .cache
                .get(&key)
                .is_none_or(|entry| entry.source_version != buf.version());
            if needs_reslice {
                let entry = slice_from_buffer(buf, self.tail_len);
                self.cache.insert(key.clone(), entry);
            }
        }

        match self.cache.get(&key) {
            Some(entry) => entry.clone(),
            None => empty_entry(),
        }
    }

    /// Read-only variant of [`fetch`](Self::fetch) — never mutates the
    /// cache. Used by the iced `view()` path where only `&self` is
    /// available. If no entry exists for `(symbol, tf)`, returns an
    /// [`empty_entry`] placeholder; callers trigger loads via
    /// [`request_load`](Self::request_load) from a `&mut self`
    /// context (the message-dispatch side).
    pub fn peek(&self, symbol: &str, tf: Timeframe) -> Entry {
        let key = (symbol.to_string(), tf);
        match self.cache.get(&key) {
            Some(entry) => entry.clone(),
            None => empty_entry(),
        }
    }

    /// Return a [`LoadTask`] the caller should spawn, or `None` if
    /// the request is redundant or the concurrent-load cap is saturated
    /// (in which case the task is queued for later
    /// [`drain_next`](Self::drain_next)).
    ///
    /// A return of `None` means either:
    /// - the entry is already loaded with real data (`source_version > 0`), or
    /// - a load is already in flight for the same key, or
    /// - the same key is already queued, or
    /// - the queue just accepted this task because `pending` is at capacity.
    pub fn request_load(&mut self, symbol: &str, tf: Timeframe, days: u32) -> Option<LoadTask> {
        let key = (symbol.to_string(), tf);

        // Already loaded with real data — nothing to do.
        if let Some(entry) = self.cache.get(&key) {
            if entry.source_version > 0 {
                return None;
            }
        }

        // Already in flight.
        if self.pending.contains(&key) {
            return None;
        }

        // Already queued.
        if self
            .waiting
            .iter()
            .any(|t| t.symbol == symbol && t.tf == tf)
        {
            return None;
        }

        // At capacity — enqueue and let `drain_next` pick it up later.
        if self.pending.len() >= self.max_concurrent {
            self.waiting.push_back(LoadTask {
                symbol: symbol.to_string(),
                tf,
                days,
            });
            return None;
        }

        self.pending.insert(key);
        Some(LoadTask {
            symbol: symbol.to_string(),
            tf,
            days,
        })
    }

    /// Pop the next queued [`LoadTask`] if the concurrent-load cap has
    /// room. Called after each [`install`](Self::install) or
    /// [`install_empty`](Self::install_empty) to keep the pipeline fed.
    /// Returns `None` when the queue is empty or `pending` is still full.
    pub fn drain_next(&mut self) -> Option<LoadTask> {
        while self.pending.len() < self.max_concurrent {
            let task = self.waiting.pop_front()?;
            let key = (task.symbol.clone(), task.tf);

            // Skip if it completed while queued.
            if let Some(entry) = self.cache.get(&key) {
                if entry.source_version > 0 {
                    continue;
                }
            }
            if self.pending.contains(&key) {
                continue;
            }

            self.pending.insert(key);
            return Some(task);
        }
        None
    }

    /// Current count of in-flight loads. Primarily for tests.
    #[cfg(test)]
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Current count of queued loads waiting for a slot. Primarily for tests.
    #[cfg(test)]
    pub fn waiting_count(&self) -> usize {
        self.waiting.len()
    }

    /// Install a freshly-loaded buffer for `(symbol, tf)`. Clears the
    /// pending marker and inserts a resliced entry.
    pub fn install(&mut self, symbol: &str, tf: Timeframe, buffer: &CandleBuffer) {
        let key = (symbol.to_string(), tf);
        self.pending.remove(&key);
        let entry = slice_from_buffer(buffer, self.tail_len);
        self.cache.insert(key, entry);
    }

    /// Install an explicit empty placeholder for `(symbol, tf)`.
    ///
    /// Useful when a load completed with zero rows and the caller
    /// wants `fetch` to observe a present-but-empty cache entry.
    /// Because the entry's `source_version` is `0`,
    /// [`request_load`](Self::request_load) will still permit a
    /// re-dispatch — this mirrors the empty-state spec in Decision 8,
    /// where the widget renders a placeholder while a retry is in
    /// flight.
    pub fn install_empty(&mut self, symbol: &str, tf: Timeframe) {
        let key = (symbol.to_string(), tf);
        self.pending.remove(&key);
        self.cache.insert(key, empty_entry());
    }

    /// Drop all cached entries and pending markers. Intended for tests
    /// and for future "clear session" actions.
    pub fn clear(&mut self) {
        self.cache.clear();
        self.pending.clear();
        self.waiting.clear();
    }

    // ── Slice 8d — `CandleSeries` alternate source ───────────────────────

    /// Fetch (or reslice) the thumbnail entry for `(symbol, tf)` from a
    /// new-stack [`midas_bars::CandleSeries`] handle instead of the
    /// legacy [`CandleBuffer`].
    ///
    /// Produces an identical `Vec<f32>` of closes to
    /// [`Self::fetch`] on equivalent data. The series' `version()` is
    /// compared against the cached `source_version`, matching the
    /// buffer-source invalidation semantics.
    ///
    /// Feature-gated because `midas_bars` is only linked under the
    /// `session_chart` feature. Slice 9c drops the gate once the
    /// new-stack becomes unconditional.
    #[cfg(feature = "session_chart")]
    pub fn fetch_from_series(
        &mut self,
        symbol: &str,
        tf: Timeframe,
        source: Option<&parking_lot::RwLock<midas_bars::CandleSeries>>,
    ) -> Entry {
        let key = (symbol.to_string(), tf);

        if let Some(series_lock) = source {
            // Short-lived read guard — matches the driver's "read one
            // scalar then drop" pattern so the write side (tick fold)
            // doesn't wait.
            let guard = series_lock.read();
            let live_version = guard.version();
            let needs_reslice = self
                .cache
                .get(&key)
                .is_none_or(|entry| entry.source_version != live_version);
            if needs_reslice {
                let entry = slice_from_series(&guard, self.tail_len);
                self.cache.insert(key.clone(), entry);
            }
        }

        match self.cache.get(&key) {
            Some(entry) => entry.clone(),
            None => empty_entry(),
        }
    }

    /// Install a freshly-loaded `CandleSeries` slice for `(symbol, tf)`.
    /// Mirrors [`Self::install`] for the new-stack source shape.
    ///
    /// Useful when the app seeds the thumbnail cache from a snapshot
    /// the session-chart driver already produced — avoids a redundant
    /// provider round-trip.
    #[cfg(feature = "session_chart")]
    pub fn install_series(
        &mut self,
        symbol: &str,
        tf: Timeframe,
        series: &midas_bars::CandleSeries,
    ) {
        let key = (symbol.to_string(), tf);
        self.pending.remove(&key);
        let entry = slice_from_series(series, self.tail_len);
        self.cache.insert(key, entry);
    }
}

impl Default for ThumbnailDataStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Extract the last `tail_len` closes from `buf`, compute the price
/// range over that window, and package into an [`Entry`].
fn slice_from_buffer(buf: &CandleBuffer, tail_len: usize) -> Entry {
    let len = buf.len();
    let version = buf.version();

    if len == 0 {
        return Entry {
            closes: Arc::new(Vec::new()),
            y_min: 0.0,
            y_max: 1.0,
            source_version: version,
        };
    }

    let start = len.saturating_sub(tail_len);
    let closes: Vec<f32> = buf.closes[start..len].to_vec();
    let (y_min, y_max) = buf.price_range(start..len);

    Entry {
        closes: Arc::new(closes),
        y_min,
        y_max,
        source_version: version,
    }
}

/// Non-cached empty placeholder returned when `fetch` is called with
/// no source and no cache entry.
#[allow(dead_code)] // consumed by Slice 4 (grid empty-state)
fn empty_entry() -> Entry {
    Entry {
        closes: Arc::new(Vec::new()),
        y_min: 0.0,
        y_max: 1.0,
        source_version: 0,
    }
}

/// Slice 8d: extract the last `tail_len` closes from a
/// [`midas_bars::CandleSeries`] + compute the price range over that
/// window. Semantically equivalent to [`slice_from_buffer`] — both
/// paths produce an identical `Vec<f32>` for the same data.
///
/// Reads happen through [`midas_bars::CandleSeries::iter`] +
/// [`midas_bars::CandleRef`] accessors. The closes column is stored
/// as `f32` on both stacks so no precision loss occurs.
#[cfg(feature = "session_chart")]
fn slice_from_series(series: &midas_bars::CandleSeries, tail_len: usize) -> Entry {
    let len = series.len();
    let version = series.version();

    if len == 0 {
        return Entry {
            closes: Arc::new(Vec::new()),
            y_min: 0.0,
            y_max: 1.0,
            source_version: version,
        };
    }

    let start = len.saturating_sub(tail_len);
    // Walk the tail directly via CandleRef so we don't materialise a
    // full Vec<Candle> on the hot path. Closes are already f32 in the
    // SoA storage; CandleRef::close returns f64 for API uniformity —
    // we narrow back to f32 for thumbnail storage (match the legacy
    // source contract exactly).
    let mut closes: Vec<f32> = Vec::with_capacity(len - start);
    let mut y_min = f32::INFINITY;
    let mut y_max = f32::NEG_INFINITY;
    for idx in start..len {
        let row = series.at(idx).expect("idx in bounds");
        let c = row.close() as f32;
        closes.push(c);
        let h = row.high() as f32;
        let l = row.low() as f32;
        if h > y_max {
            y_max = h;
        }
        if l < y_min {
            y_min = l;
        }
    }
    if !y_min.is_finite() {
        y_min = 0.0;
    }
    if !y_max.is_finite() {
        y_max = 1.0;
    }

    Entry {
        closes: Arc::new(closes),
        y_min,
        y_max,
        source_version: version,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buf_with(n: usize, base: f32) -> CandleBuffer {
        let mut buf = CandleBuffer::new();
        for i in 0..n {
            let ts = 1000 + i as i64 * 1000;
            let o = base + i as f32;
            buf.push(ts, o, o + 1.0, o - 1.0, o + 0.5, 100);
        }
        buf
    }

    #[test]
    fn fetch_empty_source_returns_empty_arc() {
        let mut store = ThumbnailDataStore::new();
        let entry = store.fetch("AAPL", Timeframe::M5, None);
        assert!(entry.closes.is_empty());
        assert_eq!(entry.source_version, 0);
        // Not cached — subsequent fetch should still miss.
        // (We cannot observe this directly without an introspection
        // hook; instead verify request_load still returns Some.)
        assert!(store.request_load("AAPL", Timeframe::M5, 1).is_some());
    }

    #[test]
    fn fetch_reslices_when_version_advances() {
        let mut store = ThumbnailDataStore::new();
        let mut buf = buf_with(5, 100.0);

        let a = store.fetch("AAPL", Timeframe::M5, Some(&buf));
        assert_eq!(a.closes.len(), 5);
        let v_a = a.source_version;

        // Mutate — version advances.
        buf.push(10_000, 200.0, 201.0, 199.0, 200.5, 500);
        let b = store.fetch("AAPL", Timeframe::M5, Some(&buf));
        assert_eq!(b.closes.len(), 6);
        assert!(b.source_version > v_a);
    }

    #[test]
    fn fetch_returns_same_arc_when_version_unchanged() {
        let mut store = ThumbnailDataStore::new();
        let buf = buf_with(10, 50.0);

        let a = store.fetch("AAPL", Timeframe::M5, Some(&buf));
        let b = store.fetch("AAPL", Timeframe::M5, Some(&buf));
        assert!(Arc::ptr_eq(&a.closes, &b.closes));
        assert_eq!(a.source_version, b.source_version);
    }

    #[test]
    fn fetch_clamps_to_tail_len() {
        let mut store = ThumbnailDataStore::with_tail_len(100);
        let buf = buf_with(150, 10.0);

        let entry = store.fetch("AAPL", Timeframe::M5, Some(&buf));
        assert_eq!(entry.closes.len(), 100);
        // The last close from the 150-row buffer should be present.
        assert_eq!(*entry.closes.last().expect("non-empty"), buf.closes[149]);
    }

    #[test]
    fn fetch_handles_shorter_than_tail() {
        let mut store = ThumbnailDataStore::with_tail_len(100);
        let buf = buf_with(7, 1.0);

        let entry = store.fetch("AAPL", Timeframe::M5, Some(&buf));
        assert_eq!(entry.closes.len(), 7);
    }

    #[test]
    fn request_load_deduplicates() {
        let mut store = ThumbnailDataStore::new();

        let first = store.request_load("AAPL", Timeframe::M5, 1);
        assert!(first.is_some());

        // Second call before install -> None (in flight).
        let second = store.request_load("AAPL", Timeframe::M5, 1);
        assert!(second.is_none());

        // Install the load result.
        let buf = buf_with(3, 1.0);
        store.install("AAPL", Timeframe::M5, &buf);

        // Third call after install -> None (already loaded).
        let third = store.request_load("AAPL", Timeframe::M5, 1);
        assert!(third.is_none());
    }

    #[test]
    fn install_clears_pending() {
        let mut store = ThumbnailDataStore::new();
        assert!(store.request_load("AAPL", Timeframe::M5, 1).is_some());

        let buf = buf_with(3, 1.0);
        store.install("AAPL", Timeframe::M5, &buf);

        // After install, the entry exists with source_version > 0,
        // so request_load returns None.
        assert!(store.request_load("AAPL", Timeframe::M5, 1).is_none());
    }

    #[test]
    fn request_load_per_tf_independent() {
        let mut store = ThumbnailDataStore::new();
        assert!(store.request_load("AAPL", Timeframe::M1, 1).is_some());
        // Different tf is a different key.
        assert!(store.request_load("AAPL", Timeframe::M5, 1).is_some());
        // Same tf still dedup'd.
        assert!(store.request_load("AAPL", Timeframe::M1, 1).is_none());
    }

    #[test]
    fn install_empty_clears_pending_and_exposes_empty_entry() {
        let mut store = ThumbnailDataStore::new();
        assert!(store.request_load("AAPL", Timeframe::M5, 1).is_some());
        store.install_empty("AAPL", Timeframe::M5);

        // Cache now holds an empty entry observable via fetch(None).
        let entry = store.fetch("AAPL", Timeframe::M5, None);
        assert!(entry.closes.is_empty());
        assert_eq!(entry.source_version, 0);

        // install_empty cleared the pending marker, so a fresh
        // request_load fires again (retry path).
        assert!(store.request_load("AAPL", Timeframe::M5, 1).is_some());
    }

    #[test]
    fn clear_empties_cache_and_pending() {
        let mut store = ThumbnailDataStore::new();
        let buf = buf_with(3, 1.0);
        store.install("AAPL", Timeframe::M5, &buf);
        assert!(store.request_load("MSFT", Timeframe::M1, 1).is_some());

        store.clear();

        // After clear, AAPL can be requested again and MSFT is no
        // longer pending.
        assert!(store.request_load("AAPL", Timeframe::M5, 1).is_some());
        assert!(store.request_load("MSFT", Timeframe::M1, 1).is_some());
    }

    #[test]
    fn request_load_caps_concurrency_and_queues_overflow() {
        let mut store = ThumbnailDataStore::new().with_max_concurrent(2);

        // First two admit immediately.
        assert!(store.request_load("A", Timeframe::M1, 1).is_some());
        assert!(store.request_load("B", Timeframe::M1, 1).is_some());
        assert_eq!(store.pending_count(), 2);
        assert_eq!(store.waiting_count(), 0);

        // Third is queued — returns None but nothing else happens.
        assert!(store.request_load("C", Timeframe::M1, 1).is_none());
        assert_eq!(store.pending_count(), 2);
        assert_eq!(store.waiting_count(), 1);

        // Duplicate enqueue is rejected (same key already queued).
        assert!(store.request_load("C", Timeframe::M1, 1).is_none());
        assert_eq!(store.waiting_count(), 1);

        // drain_next yields None while at capacity.
        assert!(store.drain_next().is_none());

        // Completing A frees a slot and drain_next pops C into pending.
        let buf = buf_with(3, 1.0);
        store.install("A", Timeframe::M1, &buf);
        assert_eq!(store.pending_count(), 1);
        let drained = store.drain_next().expect("C should drain");
        assert_eq!(drained.symbol, "C");
        assert_eq!(store.pending_count(), 2);
        assert_eq!(store.waiting_count(), 0);
    }

    #[test]
    fn drain_next_skips_keys_that_completed_while_queued() {
        let mut store = ThumbnailDataStore::new().with_max_concurrent(1);

        // Admit A, queue B, queue C.
        assert!(store.request_load("A", Timeframe::M1, 1).is_some());
        assert!(store.request_load("B", Timeframe::M1, 1).is_none());
        assert!(store.request_load("C", Timeframe::M1, 1).is_none());
        assert_eq!(store.waiting_count(), 2);

        // B's data arrives out-of-band (e.g. a live tick) before its
        // queued slot opened — mark it installed.
        let buf = buf_with(3, 1.0);
        store.install("B", Timeframe::M1, &buf);

        // Finish A. drain_next should skip B (already installed) and
        // return C.
        store.install("A", Timeframe::M1, &buf);
        let drained = store.drain_next().expect("C should be next");
        assert_eq!(drained.symbol, "C");
        assert_eq!(store.waiting_count(), 0);
    }

    #[test]
    fn drain_next_returns_none_when_queue_empty() {
        let mut store = ThumbnailDataStore::new();
        assert!(store.drain_next().is_none());
        assert!(store.request_load("A", Timeframe::M1, 1).is_some());
        assert!(store.drain_next().is_none());
    }

    // ── Slice 8d — `CandleSeries` alternate source + palette share ──────

    /// Shared `ThumbnailPalette` yields the same tint for the same
    /// first/last pair regardless of which source the caller seeded
    /// from. Plan R9 invariant: thumbnail sparkline and main chart
    /// read from the SAME palette so they cannot drift.
    #[test]
    fn thumbnail_palette_produces_consistent_colors() {
        use crate::theme::ThumbnailPalette;
        let pal = ThumbnailPalette::dark_default();
        // Same palette, same closes → same color.
        let up_a = pal.color_for_closes(Some(100.0), Some(110.0));
        let up_b = pal.color_for_closes(Some(100.0), Some(110.0));
        assert_eq!(up_a, up_b);
        // Direction changes route to the expected face.
        assert_eq!(pal.color_for_closes(Some(100.0), Some(110.0)), pal.up);
        assert_eq!(pal.color_for_closes(Some(110.0), Some(100.0)), pal.down);
        assert_eq!(pal.color_for_closes(Some(100.0), Some(100.0)), pal.flat);
        assert_eq!(pal.color_for_closes(None, None), pal.flat);
    }

    /// Both halves of the R9 invariant: the legacy chart surface (via
    /// `thumbnail_color` in views.rs) and the thumbnail widget (via
    /// `ThumbnailPalette::color_for_closes`) agree on identical input.
    /// Seed both with the same palette → assert same color.
    #[test]
    fn main_chart_and_thumbnail_share_palette() {
        use crate::theme::{ThumbnailPalette, THUMBNAIL_DOWN, THUMBNAIL_FLAT, THUMBNAIL_UP};
        let pal = ThumbnailPalette::dark_default();
        // The legacy constants MUST equal the palette faces — the
        // "shared palette" invariant (R9).
        assert_eq!(pal.up, THUMBNAIL_UP);
        assert_eq!(pal.down, THUMBNAIL_DOWN);
        assert_eq!(pal.flat, THUMBNAIL_FLAT);
    }

    /// Slice 8d: the `CandleSeries` source produces an identical
    /// `Vec<f32>` of closes to the `CandleBuffer` source on
    /// equivalent data. Parity is the migration gate — a differing
    /// close here means the thumbnail would flicker colour across a
    /// backend swap.
    #[cfg(feature = "session_chart")]
    #[test]
    fn candle_series_source_matches_buffer_source() {
        use chrono::TimeZone;
        use midas_bars::{Candle, CandleSeries, Completeness, Ohlcv, Symbol};
        use midas_calendar::{xnys, BarPeriod, Timestamp};

        // Build a 20-candle CandleSeries with recognisable closes.
        let cal = xnys();
        let sym = Symbol::new("SPY", cal.id());
        let mut series = CandleSeries::new(cal.id(), BarPeriod::m1(), sym);
        let start: Timestamp = chrono::Utc
            .with_ymd_and_hms(2024, 1, 17, 14, 30, 0)
            .unwrap();
        for i in 0..20 {
            let ts = start + chrono::Duration::minutes(i);
            let p = 100.0 + i as f64 * 0.5;
            let ohlcv = Ohlcv::new(p, p + 0.1, p - 0.1, p, 100, 1, None).unwrap();
            let c = Candle::new(
                sym,
                cal,
                BarPeriod::m1(),
                cal.classify(ts),
                cal.bar_window(ts, BarPeriod::m1()).unwrap(),
                ohlcv,
                Completeness::Completed,
            )
            .unwrap();
            series.push(c);
        }

        // Build a matching CandleBuffer with identical closes. The
        // timestamps differ in encoding (ms for legacy buffer vs ns
        // for the new-stack series) — we only need the CLOSES to line
        // up bit-identically, which is the invariant thumbnails
        // depend on.
        let mut buf = CandleBuffer::new();
        for i in 0..20 {
            let p = 100.0 + i as f32 * 0.5;
            buf.push(
                start.timestamp_millis() + (i as i64) * 60_000,
                p,
                p + 0.1,
                p - 0.1,
                p,
                100,
            );
        }

        // Slice both via store — same tail length, same `tail_len`.
        let mut store = ThumbnailDataStore::with_tail_len(20);
        let from_buf = store.fetch("SPY", Timeframe::M1, Some(&buf));
        assert_eq!(from_buf.closes.len(), 20);

        // Clear so the series-based slice doesn't short-circuit on
        // the buf entry.
        store.clear();
        let lock = parking_lot::RwLock::new(series);
        let from_series = store.fetch_from_series("SPY", Timeframe::M1, Some(&lock));
        assert_eq!(from_series.closes.len(), 20);

        assert_eq!(
            from_buf.closes.as_slice(),
            from_series.closes.as_slice(),
            "closes MUST match across source shapes"
        );
    }

    /// Slice 8d: empty `CandleSeries` yields an empty entry with
    /// `source_version == 0`, matching the buffer path.
    #[cfg(feature = "session_chart")]
    #[test]
    fn empty_candle_series_yields_empty_entry() {
        use midas_bars::{CandleSeries, Symbol};
        use midas_calendar::{xnys, BarPeriod};

        let cal = xnys();
        let sym = Symbol::new("SPY", cal.id());
        let series = CandleSeries::new(cal.id(), BarPeriod::m1(), sym);
        let lock = parking_lot::RwLock::new(series);

        let mut store = ThumbnailDataStore::new();
        let entry = store.fetch_from_series("SPY", Timeframe::M1, Some(&lock));
        assert!(entry.closes.is_empty());
        // Version is 0 for an empty series because nothing has been
        // pushed yet.
        assert_eq!(entry.source_version, 0);
    }

    /// Slice 8d: `install_series` clears the pending marker + inserts
    /// a sliced entry, matching the buffer-source `install` contract.
    #[cfg(feature = "session_chart")]
    #[test]
    fn install_series_clears_pending() {
        use chrono::TimeZone;
        use midas_bars::{Candle, CandleSeries, Completeness, Ohlcv, Symbol};
        use midas_calendar::{xnys, BarPeriod};

        let cal = xnys();
        let sym = Symbol::new("SPY", cal.id());
        let mut series = CandleSeries::new(cal.id(), BarPeriod::m1(), sym);
        let ts = chrono::Utc
            .with_ymd_and_hms(2024, 1, 17, 14, 30, 0)
            .unwrap();
        let ohlcv = Ohlcv::new(100.0, 101.0, 99.0, 100.5, 100, 1, None).unwrap();
        series.push(
            Candle::new(
                sym,
                cal,
                BarPeriod::m1(),
                cal.classify(ts),
                cal.bar_window(ts, BarPeriod::m1()).unwrap(),
                ohlcv,
                Completeness::Completed,
            )
            .unwrap(),
        );

        let mut store = ThumbnailDataStore::new();
        assert!(store.request_load("SPY", Timeframe::M1, 1).is_some());
        assert_eq!(store.pending_count(), 1);

        store.install_series("SPY", Timeframe::M1, &series);
        assert_eq!(store.pending_count(), 0);
        // request_load now declines because the entry has real data.
        assert!(store.request_load("SPY", Timeframe::M1, 1).is_none());
    }

    /// Slice 8d: version-bump on the series invalidates the cached
    /// slice, matching the buffer's `version()` invariant.
    #[cfg(feature = "session_chart")]
    #[test]
    fn candle_series_version_bump_triggers_reslice() {
        use chrono::TimeZone;
        use midas_bars::{Candle, CandleSeries, Completeness, Ohlcv, Symbol};
        use midas_calendar::{xnys, BarPeriod};

        let cal = xnys();
        let sym = Symbol::new("SPY", cal.id());
        let mut series = CandleSeries::new(cal.id(), BarPeriod::m1(), sym);
        let start = chrono::Utc
            .with_ymd_and_hms(2024, 1, 17, 14, 30, 0)
            .unwrap();
        for i in 0..3 {
            let ts = start + chrono::Duration::minutes(i);
            let p = 100.0 + i as f64;
            let ohlcv = Ohlcv::new(p, p + 0.1, p - 0.1, p, 100, 1, None).unwrap();
            series.push(
                Candle::new(
                    sym,
                    cal,
                    BarPeriod::m1(),
                    cal.classify(ts),
                    cal.bar_window(ts, BarPeriod::m1()).unwrap(),
                    ohlcv,
                    Completeness::Completed,
                )
                .unwrap(),
            );
        }
        let lock = parking_lot::RwLock::new(series);

        let mut store = ThumbnailDataStore::new();
        let a = store.fetch_from_series("SPY", Timeframe::M1, Some(&lock));
        assert_eq!(a.closes.len(), 3);

        // Mutate under the write lock — version bumps.
        {
            let mut g = lock.write();
            let ts = start + chrono::Duration::minutes(3);
            let p = 200.0;
            let ohlcv = Ohlcv::new(p, p + 0.1, p - 0.1, p, 100, 1, None).unwrap();
            g.push(
                Candle::new(
                    sym,
                    cal,
                    BarPeriod::m1(),
                    cal.classify(ts),
                    cal.bar_window(ts, BarPeriod::m1()).unwrap(),
                    ohlcv,
                    Completeness::Completed,
                )
                .unwrap(),
            );
        }
        let b = store.fetch_from_series("SPY", Timeframe::M1, Some(&lock));
        assert_eq!(b.closes.len(), 4);
        assert!(b.source_version > a.source_version);
    }

    /// Slice 8d: `fetch_from_series` honours `tail_len`, clamping a
    /// 30-candle series to the last 10 when the tail is shorter than
    /// the series.
    #[cfg(feature = "session_chart")]
    #[test]
    fn candle_series_tail_len_is_respected() {
        use chrono::TimeZone;
        use midas_bars::{Candle, CandleSeries, Completeness, Ohlcv, Symbol};
        use midas_calendar::{xnys, BarPeriod};

        let cal = xnys();
        let sym = Symbol::new("SPY", cal.id());
        let mut series = CandleSeries::new(cal.id(), BarPeriod::m1(), sym);
        let start = chrono::Utc
            .with_ymd_and_hms(2024, 1, 17, 14, 30, 0)
            .unwrap();
        for i in 0..30 {
            let ts = start + chrono::Duration::minutes(i);
            let p = 100.0 + i as f64;
            let ohlcv = Ohlcv::new(p, p + 0.1, p - 0.1, p, 100, 1, None).unwrap();
            series.push(
                Candle::new(
                    sym,
                    cal,
                    BarPeriod::m1(),
                    cal.classify(ts),
                    cal.bar_window(ts, BarPeriod::m1()).unwrap(),
                    ohlcv,
                    Completeness::Completed,
                )
                .unwrap(),
            );
        }
        let lock = parking_lot::RwLock::new(series);

        let mut store = ThumbnailDataStore::with_tail_len(10);
        let entry = store.fetch_from_series("SPY", Timeframe::M1, Some(&lock));
        assert_eq!(entry.closes.len(), 10);
        // Last close in the tail is bar 29 (0-indexed).
        assert!((entry.closes.last().unwrap() - 129.0).abs() < 1e-3);
    }
}
