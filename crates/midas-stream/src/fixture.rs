//! [`FixtureBarStream`] — replay from an in-memory `Vec<Candle>`.
//!
//! Used by golden fixtures and tests. Implements both [`BarStream`] and
//! [`SeekableBarStream`] — the full seekable feature set.
//!
//! Construction validates that every candle's `(calendar, period,
//! symbol)` matches the stream's [`BarStreamMeta`] and that the vec is
//! sorted by `window.open`. Callers that fail validation get a
//! descriptive error, not a runtime panic deep inside `next()`.

use async_trait::async_trait;
use midas_bars::Candle;
use midas_calendar::ExchangeCalendar;

use crate::{BarStream, BarStreamMeta, SeekableBarStream, StreamError, TimeRange, Timestamp};

/// In-memory seekable bar stream. Replays a `Vec<Candle>` sorted by
/// `window.open`.
pub struct FixtureBarStream {
    meta: BarStreamMeta,
    candles: Vec<Candle>,
    cursor: usize,
}

impl std::fmt::Debug for FixtureBarStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FixtureBarStream")
            .field("meta", &self.meta)
            .field("len", &self.candles.len())
            .field("cursor", &self.cursor)
            .finish()
    }
}

impl FixtureBarStream {
    /// Build a `FixtureBarStream`. Validates that:
    /// - All candles share `(calendar.id(), period, symbol)` with `meta`.
    /// - The vec is sorted non-descending by `window.open`.
    /// - Every candle's `window.open` lies inside the calendar's
    ///   `covers()` range — else `StreamError::CoverageExceeded`.
    ///
    /// Returns the validated stream on success.
    pub fn new(meta: BarStreamMeta, candles: Vec<Candle>) -> Result<Self, StreamError> {
        let cal_id = meta.calendar.id();
        let cov = meta.calendar.covers();

        for (i, c) in candles.iter().enumerate() {
            if c.calendar != cal_id {
                return Err(StreamError::Upstream(format!(
                    "fixture candle #{i}: calendar {} != meta {}",
                    c.calendar, cal_id,
                )));
            }
            if c.period != meta.period {
                return Err(StreamError::Upstream(format!(
                    "fixture candle #{i}: period {:?} != meta {:?}",
                    c.period, meta.period,
                )));
            }
            if c.symbol != meta.symbol {
                return Err(StreamError::Upstream(format!(
                    "fixture candle #{i}: symbol {} != meta {}",
                    c.symbol, meta.symbol,
                )));
            }
            // Coverage check — `covers()` is half-open `[start, end)`.
            let date = c.window.open.date_naive();
            if date < cov.start || date >= cov.end {
                let range_from = candles
                    .first()
                    .map(|x| x.window.open)
                    .unwrap_or(c.window.open);
                let range_to = candles
                    .last()
                    .map(|x| x.window.open)
                    .unwrap_or(c.window.open);
                let range = TimeRange::new(range_from, range_to).unwrap_or(TimeRange {
                    from: range_from,
                    to: range_from,
                });
                return Err(StreamError::CoverageExceeded {
                    calendar: cal_id,
                    range,
                });
            }
        }

        // Sortedness check.
        for w in candles.windows(2) {
            if w[1].window.open < w[0].window.open {
                return Err(StreamError::Upstream(format!(
                    "fixture candles not sorted: {} > {}",
                    w[0].window.open, w[1].window.open,
                )));
            }
        }

        Ok(Self {
            meta,
            candles,
            cursor: 0,
        })
    }

    /// Current cursor index (for tests / diagnostics).
    #[inline]
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Number of candles in the fixture.
    #[inline]
    pub fn len(&self) -> usize {
        self.candles.len()
    }

    /// True when empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.candles.is_empty()
    }

    /// Access to the underlying calendar (re-exposed for test brevity).
    #[inline]
    pub fn calendar(&self) -> &'static dyn ExchangeCalendar {
        self.meta.calendar
    }
}

#[async_trait]
impl BarStream for FixtureBarStream {
    fn meta(&self) -> &BarStreamMeta {
        &self.meta
    }

    async fn next(&mut self) -> Option<Candle> {
        if self.cursor >= self.candles.len() {
            return None;
        }
        let c = self.candles[self.cursor].clone();
        self.cursor += 1;
        Some(c)
    }

    async fn snapshot(&mut self, range: TimeRange) -> Result<Vec<Candle>, StreamError> {
        // Half-open `[from, to)` applied to `window.open`.
        let out: Vec<Candle> = self
            .candles
            .iter()
            .filter(|c| range.contains(c.window.open))
            .cloned()
            .collect();
        Ok(out)
    }
}

#[async_trait]
impl SeekableBarStream for FixtureBarStream {
    async fn seek(&mut self, to: Timestamp) -> Result<(), StreamError> {
        // Move cursor to the first candle with `window.open >= to`.
        // Binary search over the sorted window.opens.
        let idx = self.candles.partition_point(|c| c.window.open < to);
        self.cursor = idx;
        Ok(())
    }
}
