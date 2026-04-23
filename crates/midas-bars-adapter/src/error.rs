//! Adapter-level error enum.
//!
//! Bridges errors from three upstream vocabularies:
//! - [`ResolveError`](crate::ResolveError) from the symbol-resolver layer.
//! - [`midas_broker_core::MarketDataError`] from the provider.
//! - [`midas_stream::StreamError`] from the stream stack (history
//!   construction can fail on coverage or ordering).
//!
//! The `NoTimeframeMapping` variant covers the specific asymmetry in
//! [`period_to_timeframe`](crate::period_to_timeframe): the legacy
//! `Timeframe` enum does not express every [`midas_calendar::BarPeriod`]
//! (`Session(Extended)`, `Session(Eth)`, `Calendar(Quarter)`,
//! `Calendar(Year)` have no counterpart).

use midas_calendar::BarPeriod;

use crate::ResolveError;

/// Unified error surface for adapter operations.
#[derive(Debug, thiserror::Error)]
pub enum AdapterError {
    /// The supplied [`BarPeriod`] has no legacy [`midas_broker_core::Timeframe`]
    /// equivalent — typically a session-extended or sub-month calendar
    /// span the old enum never modelled.
    #[error("no Timeframe mapping for BarPeriod: {0:?}")]
    NoTimeframeMapping(BarPeriod),

    /// Symbol resolution failure (unknown ticker, lookup error).
    #[error("resolver error: {0}")]
    Resolve(#[from] ResolveError),

    /// Underlying provider bubbled an error.
    #[error("provider error: {0}")]
    Provider(#[from] midas_broker_core::market_data::MarketDataError),

    /// Stream-layer error (coverage, upstream format, etc.).
    #[error("stream error: {0}")]
    Stream(#[from] midas_stream::StreamError),

    /// Upstream broker call did not complete within the adapter's
    /// timeout budget. Surfaces `subscribe_ticks` /
    /// `historical_bars` / `subscribe_realtime_bars` stalls so the UI
    /// can close the pending window instead of hanging on a future
    /// that never resolves.
    #[error("upstream broker call timed out after {secs}s: {op}")]
    Timeout { op: &'static str, secs: u64 },
}
