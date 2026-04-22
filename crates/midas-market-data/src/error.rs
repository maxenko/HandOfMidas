//! Router-level error surface.
//!
//! The router mostly re-uses [`midas_broker_core::market_data::MarketDataError`]
//! verbatim — that enum is already the wire-accurate shape a
//! provider returns. [`RouterError`] wraps it with a couple of
//! router-specific constructors the provider trait cannot express
//! (shutdown race, missing hub, etc.) so callers can match on the
//! router's own API surface without pulling in the full provider
//! vocabulary.
//!
//! Every [`RouterError`] can be round-tripped into [`MarketDataError`]
//! via the `From` impl — the router keeps the public API unified around
//! `MarketDataError` for now (S5), and [`RouterError`] exists to keep
//! internal control-actor error paths expressive.

use midas_broker_core::market_data::MarketDataError;
use thiserror::Error;

/// Router-internal error.
///
/// Not part of the public `subscribe_*` surface (those methods return
/// [`MarketDataError`]); surfaced only through internal `Result` types.
#[derive(Debug, Error)]
pub enum RouterError {
    /// The router's control actor has shut down — the caller raced a
    /// drop. Callers should treat this as a clean teardown.
    #[error("router control actor is shutting down")]
    ShuttingDown,

    /// Underlying provider error.
    #[error(transparent)]
    MarketData(#[from] MarketDataError),
}

impl From<RouterError> for MarketDataError {
    fn from(e: RouterError) -> Self {
        match e {
            RouterError::ShuttingDown => MarketDataError::ShuttingDown,
            RouterError::MarketData(m) => m,
        }
    }
}
