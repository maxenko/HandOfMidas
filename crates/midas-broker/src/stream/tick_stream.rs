//! [`TickStream`] — broadcast receiver of
//! [`Tick`](midas_broker_core::market_data::Tick) values produced by
//! [`MarketDataSource::subscribe_ticks`](crate::MarketDataSource::subscribe_ticks)
//! and
//! [`MarketDataSource::subscribe_tick_by_tick`](crate::MarketDataSource::subscribe_tick_by_tick).
//!
//! Owns three pieces of state:
//!
//! * `rx` — `tokio::sync::broadcast::Receiver<Arc<Tick>>`, the consumer
//!   end of the per-subscription fan-out.
//! * `last_error` — `Arc<OnceLock<MarketDataError>>` shared with the
//!   publisher (NM-5). On a permanent error (IB reject, wire drop) the
//!   publisher sets this before closing the broadcast; consumers can
//!   call [`TickStream::last_error`] after a `RecvError::Closed` to
//!   distinguish a clean end from an error-triggered end.
//! * `cancel` — `Option<Box<dyn FnOnce() + Send + Sync>>` (BR-2).
//!   Invoked by [`Drop`] to unsubscribe upstream.

use std::sync::{Arc, OnceLock};

use midas_broker_core::market_data::{MarketDataError, ReqId, Tick};
use tokio::sync::broadcast;

/// Handle for a per-subscription tick fan-out.
///
/// `TickStream` is `!Clone` on purpose; clones would not increment the
/// router-side refcount and would destabilise the cleanup model.
/// Consumers who need a second view call [`TickStream::resubscribe`],
/// which adds a broadcast receiver but does NOT add a router guard —
/// the original handle still drives cancellation.
pub struct TickStream {
    req_id: ReqId,
    rx: broadcast::Receiver<Arc<Tick>>,
    last_error: Arc<OnceLock<MarketDataError>>,
    cancel: Option<Box<dyn FnOnce() + Send + Sync>>,
}

impl TickStream {
    /// Build a new stream handle.
    ///
    /// Intended for use by concrete [`MarketDataSource`](crate::MarketDataSource)
    /// implementations (sim + IB adapter). Marked `#[doc(hidden)]` so it
    /// does not advertise itself as a public consumer API.
    #[doc(hidden)]
    pub fn new(
        req_id: ReqId,
        rx: broadcast::Receiver<Arc<Tick>>,
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

    /// Await the next tick.
    ///
    /// Forwards to [`broadcast::Receiver::recv`]. On
    /// [`broadcast::error::RecvError::Closed`], [`Self::last_error`] may
    /// carry the permanent error that triggered the close (NM-5).
    pub async fn next(&mut self) -> Result<Arc<Tick>, broadcast::error::RecvError> {
        self.rx.recv().await
    }

    /// Create an independent broadcast receiver on the same fan-out.
    ///
    /// Does NOT add a router-side refcount. The original handle retains
    /// sole responsibility for calling the cancel closure on drop.
    pub fn resubscribe(&self) -> broadcast::Receiver<Arc<Tick>> {
        self.rx.resubscribe()
    }

    /// Permanent error that closed this stream, if any (NM-5).
    ///
    /// `None` means either the stream is still live OR it closed
    /// cleanly (consumer dropped its handle).
    pub fn last_error(&self) -> Option<&MarketDataError> {
        self.last_error.get()
    }
}

impl Drop for TickStream {
    fn drop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel();
        }
    }
}
