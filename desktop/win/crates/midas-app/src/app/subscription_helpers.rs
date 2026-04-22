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

/// Frame coalescer: accumulates items and flushes on a fixed
/// cadence. Used to fold multiple bar / tick events per frame into
/// a single `Message::ChartBarBatch` / `Message::TickerLastPrice`.
///
/// Holds just state — the flush side calls `drain()` when the
/// coalescing interval ticks.
#[derive(Debug, Default)]
pub struct FrameCoalescer<T> {
    buffer: Vec<T>,
}

impl<T> FrameCoalescer<T> {
    /// Construct a new coalescer with `cap` pre-allocated slots.
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            buffer: Vec::with_capacity(cap),
        }
    }

    /// Push a new item. No size cap — the flush cadence bounds
    /// memory (at typical event rates the window closes before the
    /// buffer grows past a few dozen entries).
    pub fn push(&mut self, item: T) {
        self.buffer.push(item);
    }

    /// Whether the buffer has anything to flush.
    pub fn has_pending(&self) -> bool {
        !self.buffer.is_empty()
    }

    /// Drain the accumulated items into a fresh `Vec`. Called when
    /// the coalescing interval ticks.
    pub fn drain(&mut self) -> Vec<T> {
        std::mem::take(&mut self.buffer)
    }
}
