//! [`HistoricalStream`] — mpsc channel of
//! [`HistoricalStreamEvent`] messages produced by
//! [`MarketDataSource::historical_stream`](crate::MarketDataSource::historical_stream).
//!
//! Unlike [`TickStream`](super::TickStream) and
//! [`RealtimeBarStream`](super::RealtimeBarStream) this one is mpsc,
//! not broadcast — there is only ever one consumer per historical
//! subscription, so the broadcast ring buffer adds no value.
//!
//! Per M-16 and rust-ibapi 2.10's shape, the stream emits a single
//! bulk [`HistoricalStreamEvent::Historical`] payload, then a
//! [`HistoricalStreamEvent::End`] seam marker, then zero or more
//! [`HistoricalStreamEvent::Update`] live bars while `keep_up_to_date`
//! is in effect, and optionally a final
//! [`HistoricalStreamEvent::Error`].

use chrono::{DateTime, Utc};
use midas_broker_core::market_data::{Bar, MarketDataError, ReqId};
use tokio::sync::mpsc;

/// Event yielded by [`HistoricalStream`].
#[derive(Debug)]
pub enum HistoricalStreamEvent {
    /// Initial bulk payload. Emitted exactly once, before any
    /// [`HistoricalStreamEvent::End`] or later events.
    Historical(Vec<Bar>),
    /// Seam marker. `last_ts` is the `t_server` boundary — the first
    /// live [`HistoricalStreamEvent::Update`] will have `ts_open >
    /// last_ts`.
    End {
        /// First bar timestamp in the bulk payload.
        first_ts: DateTime<Utc>,
        /// Last bar timestamp in the bulk payload (the seam boundary).
        last_ts: DateTime<Utc>,
    },
    /// Trailing live bar emitted while `keep_up_to_date = true`
    /// (rust-ibapi 2.10 "update" event).
    Update(Bar),
    /// Permanent error that terminated the stream.
    Error(MarketDataError),
}

/// Handle for a historical data stream.
///
/// `!Clone` on purpose. `Drop` invokes the cancel closure (BR-2) which
/// signals the upstream publisher to stop emitting further events.
pub struct HistoricalStream {
    req_id: ReqId,
    rx: mpsc::Receiver<HistoricalStreamEvent>,
    cancel: Option<Box<dyn FnOnce() + Send + Sync>>,
}

impl HistoricalStream {
    /// Build a new stream handle.
    ///
    /// Intended for use by concrete
    /// [`MarketDataSource`](crate::MarketDataSource) implementations.
    /// Marked `#[doc(hidden)]` so it does not advertise itself as a
    /// public consumer API.
    #[doc(hidden)]
    pub fn new(
        req_id: ReqId,
        rx: mpsc::Receiver<HistoricalStreamEvent>,
        cancel: Box<dyn FnOnce() + Send + Sync>,
    ) -> Self {
        Self {
            req_id,
            rx,
            cancel: Some(cancel),
        }
    }

    /// Wire request id of this subscription.
    pub fn req_id(&self) -> ReqId {
        self.req_id
    }

    /// Await the next historical event.
    ///
    /// Returns `None` when the upstream publisher closes the channel.
    pub async fn next(&mut self) -> Option<HistoricalStreamEvent> {
        self.rx.recv().await
    }
}

impl Drop for HistoricalStream {
    fn drop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel();
        }
    }
}
