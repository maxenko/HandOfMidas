//! [`RealtimeBarStream`] — broadcast receiver of
//! [`Bar`](midas_broker_core::market_data::Bar) values produced by
//! [`MarketDataSource::subscribe_realtime_bars`](crate::MarketDataSource::subscribe_realtime_bars).
//!
//! Same `Drop`-fires-cancel shape as [`TickStream`](super::TickStream); the
//! closure is held in `Option<Box<dyn FnOnce() + Send + Sync>>` (BR-2) so
//! `.take()` inside `Drop::drop` is sound.
//!
//! IB delivers 5-second bars on this channel regardless of consumer
//! downsampling needs — aggregation to higher timeframes is the
//! aggregator's job (slice 6), not the provider's.

use std::sync::{Arc, OnceLock};

use midas_broker_core::market_data::{Bar, MarketDataError, ReqId};
use tokio::sync::broadcast;

/// Handle for a realtime-bar fan-out.
///
/// `!Clone` on purpose — [`Self::resubscribe`] exists for callers who
/// need another receiver without acquiring a second router guard.
pub struct RealtimeBarStream {
    req_id: ReqId,
    rx: broadcast::Receiver<Arc<Bar>>,
    last_error: Arc<OnceLock<MarketDataError>>,
    cancel: Option<Box<dyn FnOnce() + Send + Sync>>,
}

impl RealtimeBarStream {
    /// Build a new stream handle.
    ///
    /// Intended for use by concrete [`MarketDataSource`](crate::MarketDataSource)
    /// implementations. Marked `#[doc(hidden)]` so it does not advertise
    /// itself as a public consumer API.
    #[doc(hidden)]
    pub fn new(
        req_id: ReqId,
        rx: broadcast::Receiver<Arc<Bar>>,
        last_error: Arc<OnceLock<MarketDataError>>,
        cancel: Box<dyn FnOnce() + Send + Sync>,
    ) -> Self {
        Self {
            req_id,
            rx,
            last_error,
            cancel: Some(cancel),
        }
    }

    /// Wire request id of this subscription.
    pub fn req_id(&self) -> ReqId {
        self.req_id
    }

    /// Await the next realtime bar.
    pub async fn next(&mut self) -> Result<Arc<Bar>, broadcast::error::RecvError> {
        self.rx.recv().await
    }

    /// Create an independent broadcast receiver on the same fan-out.
    pub fn resubscribe(&self) -> broadcast::Receiver<Arc<Bar>> {
        self.rx.resubscribe()
    }

    /// Permanent error that closed this stream, if any (NM-5).
    pub fn last_error(&self) -> Option<&MarketDataError> {
        self.last_error.get()
    }
}

impl Drop for RealtimeBarStream {
    fn drop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel();
        }
    }
}
