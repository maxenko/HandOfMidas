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
}
