//! Stream handles returned by [`MarketDataSource`](crate::MarketDataSource).
//!
//! After the audit-P1 refactor the three handle types live in
//! `midas-broker-core::provider`; this module re-exports them so every
//! existing `crate::stream::{TickStream, RealtimeBarStream,
//! HistoricalStream, HistoricalStreamEvent}` call site keeps compiling.
//!
//! Every handle owns a typed receiver plus a `Drop`-fired cancel closure
//! (BR-2). See the `midas-broker-core` provider module for the full
//! contract.

pub use midas_broker_core::provider::{
    HistoricalStream, HistoricalStreamEvent, RealtimeBarStream, TickStream,
};
