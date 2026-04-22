// S7a: helpers are defined here and used starting in S7b. The
// `dead_code` warnings are otherwise unavoidable across the split
// commits.
#![allow(dead_code)]

//! Helpers shared by the per-consumer subscription streams
//! (`chart_subscriptions`, `watchlist_subscription`,
//! `ticker_subscription`).
//!
//! Keeps the stream builders readable — each lives behind a `fn`
//! pointer because `Subscription::run_with` requires that, so they
//! lean on small free helpers instead of capturing lambdas.

use std::time::Duration;

/// Fixed coalescing window used by the chart + ticker streams. At
/// 60 Hz iced refresh the UI can consume one batch per frame; longer
/// windows drop smoothness, shorter windows amplify iced update
/// churn with no perceptible benefit.
pub const FRAME_COALESCE_MS: Duration = Duration::from_millis(16);

/// Watchlist polling cadence. 50 ms is the watch-poll target from
/// the plan's §Watchlist — short enough to feel live, long enough
/// to batch bursty reconnect backfills.
pub const WATCHLIST_POLL_MS: Duration = Duration::from_millis(50);

/// Ticker-state `UpdateMarketData` emit cadence. 33 ms ~= 30 Hz —
/// matches the "at most one UI-visible update per third frame"
/// budget for per-ticker state transitions.
pub const TICKER_EMIT_MS: Duration = Duration::from_millis(33);

/// Minimum interval between `ChartResync` commits per chart (M-29).
pub const RESYNC_THROTTLE: Duration = Duration::from_secs(5);

/// Default `max_batch_size` used by chart / ticker coalescers.
/// Bursty backfills can briefly outrun the 16 ms flush cadence; we
/// flush early once the buffer crosses this threshold so memory
/// stays bounded even if the producer temporarily goes wild.
pub const DEFAULT_MAX_BATCH: usize = 256;

/// Frame coalescer: accumulates items and flushes on a fixed
/// cadence OR when the pending buffer crosses `max_batch_size`
/// (M-30) — whichever trips first.
///
/// Used by the chart / ticker / watchlist subscription streams to
/// fold multiple events per frame into a single `Message::*` batch.
/// Holds just state — the flush side calls `drain()` when the
/// coalescing interval ticks, and [`Self::should_flush_early`]
/// when the caller wants to check the size threshold between
/// interval ticks.
#[derive(Debug)]
pub struct FrameCoalescer<T> {
    buffer: Vec<T>,
    max_batch_size: usize,
}

impl<T> Default for FrameCoalescer<T> {
    fn default() -> Self {
        Self {
            buffer: Vec::new(),
            max_batch_size: DEFAULT_MAX_BATCH,
        }
    }
}

impl<T> FrameCoalescer<T> {
    /// Construct a new coalescer with `cap` pre-allocated slots and
    /// [`DEFAULT_MAX_BATCH`] as the size-based flush threshold.
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            buffer: Vec::with_capacity(cap),
            max_batch_size: DEFAULT_MAX_BATCH,
        }
    }

    /// Construct a new coalescer with an explicit `max_batch_size`
    /// threshold. Once `buffer.len() >= max_batch_size`,
    /// [`Self::should_flush_early`] reports `true` and the caller
    /// should drain without waiting for the next interval tick.
    pub fn with_capacity_and_max_batch(cap: usize, max_batch_size: usize) -> Self {
        Self {
            buffer: Vec::with_capacity(cap),
            max_batch_size: max_batch_size.max(1),
        }
    }

    /// Push a new item.
    ///
    /// Memory is bounded by the combination of the flush cadence
    /// and `max_batch_size` — the caller polls
    /// [`Self::should_flush_early`] after each push to detect the
    /// size trip point.
    pub fn push(&mut self, item: T) {
        self.buffer.push(item);
    }

    /// Whether the buffer has anything to flush.
    pub fn has_pending(&self) -> bool {
        !self.buffer.is_empty()
    }

    /// Whether the buffer has reached the size-based flush
    /// threshold and the caller should drain now instead of waiting
    /// for the interval tick (M-30).
    pub fn should_flush_early(&self) -> bool {
        self.buffer.len() >= self.max_batch_size
    }

    /// Drain the accumulated items into a fresh `Vec`. Called when
    /// the coalescing interval ticks, or when
    /// [`Self::should_flush_early`] reports `true`.
    pub fn drain(&mut self) -> Vec<T> {
        std::mem::take(&mut self.buffer)
    }

    /// Configured size-based flush threshold. Test-only accessor.
    #[cfg(test)]
    pub(crate) fn max_batch_size(&self) -> usize {
        self.max_batch_size
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn early_flush_triggers_when_buffer_reaches_threshold() {
        let mut c: FrameCoalescer<u32> = FrameCoalescer::with_capacity_and_max_batch(4, 3);
        assert!(!c.should_flush_early());
        c.push(1);
        c.push(2);
        assert!(!c.should_flush_early(), "two items below threshold");
        c.push(3);
        assert!(c.should_flush_early(), "three items hits threshold");
    }

    #[test]
    fn drain_returns_all_pending_and_resets() {
        let mut c: FrameCoalescer<u32> = FrameCoalescer::with_capacity(8);
        for i in 0..5 {
            c.push(i);
        }
        let out = c.drain();
        assert_eq!(out, vec![0, 1, 2, 3, 4]);
        assert!(!c.has_pending());
        assert!(!c.should_flush_early());
    }

    #[test]
    fn default_max_batch_is_applied() {
        let c: FrameCoalescer<u32> = FrameCoalescer::with_capacity(1);
        assert_eq!(c.max_batch_size(), DEFAULT_MAX_BATCH);
    }

    #[test]
    fn zero_threshold_is_clamped_to_one() {
        let c: FrameCoalescer<u32> = FrameCoalescer::with_capacity_and_max_batch(4, 0);
        // Zero would make `should_flush_early` always true even when
        // the buffer is empty, defeating the whole point of the
        // interval cadence. Clamp to 1.
        assert_eq!(c.max_batch_size(), 1);
    }
}
