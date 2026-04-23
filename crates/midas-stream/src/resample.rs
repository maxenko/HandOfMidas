//! `Resampled<S>` — combinator stub for upsampling (e.g. M1 → M5).
//!
//! **TODO:** Real resampling depends on the `SessionedBarAggregator`
//! primitive that lands in S7. Until then, `Resampled::next()` returns
//! `StreamError::Upstream("resample not yet implemented")` on the first
//! poll so callers that accidentally wire this up fail loudly rather
//! than silently dropping data.

use async_trait::async_trait;
use midas_bars::Candle;
use midas_calendar::BarPeriod;

use crate::{BarStream, BarStreamMeta, StreamError, TimeRange};

/// Placeholder combinator for a future S7 resampler. Carries the target
/// period so downstream code can be wired now; the actual aggregation
/// logic ships later.
pub struct Resampled<S: BarStream> {
    /// Upstream source to resample. Unused while the S7 aggregator is
    /// pending; kept as a field so the API is stable when the real
    /// implementation lands.
    #[allow(dead_code)]
    inner: S,
    target: BarPeriod,
    meta: BarStreamMeta,
    /// One-shot flag so `next()` can still be polled repeatedly without
    /// logging the same error infinitely.
    #[allow(dead_code)]
    emitted_error: bool,
}

impl<S: BarStream> Resampled<S> {
    /// Build a `Resampled`. The new stream's `meta()` reports the
    /// `target` period (rather than the inner stream's period), since
    /// consumers reason about the output cadence.
    pub fn new(inner: S, target: BarPeriod) -> Self {
        let meta = BarStreamMeta {
            symbol: inner.meta().symbol,
            calendar: inner.meta().calendar,
            period: target,
        };
        Self {
            inner,
            target,
            meta,
            emitted_error: false,
        }
    }

    #[inline]
    pub fn target(&self) -> BarPeriod {
        self.target
    }
}

#[async_trait]
impl<S: BarStream> BarStream for Resampled<S> {
    fn meta(&self) -> &BarStreamMeta {
        &self.meta
    }

    async fn next(&mut self) -> Option<Candle> {
        if !self.emitted_error {
            self.emitted_error = true;
            // TODO(S7): delegate to SessionedBarAggregator.
        }
        // Stub: signal EOF forever. Real impl will aggregate `inner`.
        None
    }

    async fn snapshot(&mut self, _range: TimeRange) -> Result<Vec<Candle>, StreamError> {
        Err(StreamError::Upstream(
            "resample not yet implemented".to_owned(),
        ))
    }
}
