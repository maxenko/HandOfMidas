//! Stream handles returned by [`MarketDataSource`](crate::MarketDataSource).
//!
//! Every handle owns a typed receiver plus a `Drop`-fired cancel closure
//! (BR-2). The closure is held as `Option<Box<dyn FnOnce() + Send + Sync>>`
//! so `Drop::drop` (which takes `&mut self`) can `.take()` and invoke it
//! exactly once.
//!
//! Three flavours live here:
//!
//! * [`TickStream`] — fan-out of [`Tick`](midas_broker_core::market_data::Tick)
//!   values from `subscribe_ticks` / `subscribe_tick_by_tick`.
//! * [`RealtimeBarStream`] — fan-out of
//!   [`Bar`](midas_broker_core::market_data::Bar) values from
//!   `subscribe_realtime_bars`.
//! * [`HistoricalStream`] — mpsc channel of
//!   [`HistoricalStreamEvent`](historical_stream::HistoricalStreamEvent)
//!   messages from `historical_stream`.
//!
//! The three handles are intentionally narrow — no clone, no pub fields —
//! so the refcount-on-drop invariant cannot be sidestepped by callers.

pub mod historical_stream;
pub mod realtime_bar_stream;
pub mod tick_stream;

pub use historical_stream::{HistoricalStream, HistoricalStreamEvent};
pub use realtime_bar_stream::RealtimeBarStream;
pub use tick_stream::TickStream;
