//! Pluggable data provider and order broker traits.
//!
//! [`DataProvider`] abstracts historical candle data retrieval.
//! [`OrderBroker`] abstracts order execution (trait-only for now).
//!
//! Both traits live in `midas-core` (the leaf crate) so that any crate in the
//! workspace can implement them without circular dependencies.

use async_trait::async_trait;
use thiserror::Error;

use crate::candle_buffer::CandleBuffer;
use crate::Timeframe;

/// Errors that can occur during data provider or broker operations.
#[derive(Debug, Error)]
pub enum ProviderError {
    /// The provider is not connected and cannot serve the request.
    #[error("provider not connected")]
    NotConnected,
    /// The requested symbol is not recognized or not available.
    #[error("unknown symbol: {symbol}")]
    UnknownSymbol { symbol: String },
    /// The requested timeframe is not supported by this provider.
    #[error("unsupported timeframe: {timeframe}")]
    UnsupportedTimeframe { timeframe: String },
    /// A network or I/O error occurred.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// An internal error with a free-form message.
    #[error("internal error: {0}")]
    Internal(String),
    /// Wraps a store (caching) error transparently.
    #[error("cache error: {0}")]
    Store(String),
}

/// Connection lifecycle states for providers that maintain persistent
/// connections (IB Gateway, WebSocket feeds, etc.).
///
/// Mirrors the existing `ConnectionState` in `midas-broker` but lives
/// in `midas-core` so the UI crate can reference it without depending
/// on `midas-broker`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionState {
    /// No connection established.
    Disconnected,
    /// Connection attempt in progress.
    Connecting,
    /// TCP connected, API negotiated, not yet fully ready.
    Connected { server_version: i32 },
    /// Fully operational.
    Ready,
    /// Connection lost, automatic reconnection in progress.
    Reconnecting { attempt: u32 },
}

impl ConnectionState {
    /// Whether the broker has at least a TCP connection.
    pub fn is_connected(&self) -> bool {
        matches!(self, Self::Connected { .. } | Self::Ready)
    }

    /// Whether the broker is fully operational.
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready)
    }
}

/// Uniform interface for historical candle data retrieval.
///
/// Implementors: `TestProvider`, `CachingProvider`, future `IbDataProvider`,
/// future `PolygonDataProvider`.
///
/// The `&self` signature enables sharing behind `Arc<dyn DataProvider>`.
/// Providers needing mutable state use interior mutability.
#[async_trait]
pub trait DataProvider: Send + Sync {
    /// Human-readable name for UI display.
    ///
    /// Borrowed from the implementor -- no allocation on each call.
    fn name(&self) -> &str;

    /// Whether the provider is currently able to serve requests.
    ///
    /// For local/test providers this always returns `true`.
    /// For network providers, returns `true` only when connected.
    fn is_connected(&self) -> bool;

    /// Retrieve historical candle data.
    ///
    /// # Arguments
    /// - `symbol`: Ticker symbol (e.g. "AAPL").
    /// - `timeframe`: Bar duration.
    /// - `days`: Number of calendar days of history to retrieve.
    ///
    /// Returns a `CandleBuffer` (SoA format) on success.
    async fn get_candles(
        &self,
        symbol: &str,
        timeframe: Timeframe,
        days: u32,
    ) -> Result<CandleBuffer, ProviderError>;
}

/// Uniform interface for order execution.
///
/// Trait-only for now -- no concrete implementation is built in this plan.
/// Future implementor: `IbOrderBroker` in `midas-broker`.
#[async_trait]
pub trait OrderBroker: Send + Sync {
    /// Human-readable name for UI display.
    fn name(&self) -> &str;

    /// Whether the broker is currently connected.
    fn is_connected(&self) -> bool;

    /// Current connection state. The UI renders this as a status indicator.
    ///
    /// Providers that are always "connected" (like a paper broker) return
    /// `ConnectionState::Ready`.
    fn connection_state(&self) -> ConnectionState;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_provider_is_object_safe() {
        fn _assert_object_safe(_: &dyn DataProvider) {}
    }

    #[test]
    fn order_broker_is_object_safe() {
        fn _assert_object_safe(_: &dyn OrderBroker) {}
    }

    #[test]
    fn provider_error_display() {
        let err = ProviderError::UnknownSymbol {
            symbol: "XYZ".into(),
        };
        assert!(err.to_string().contains("XYZ"));
    }

    #[test]
    fn connection_state_eq() {
        assert_eq!(ConnectionState::Disconnected, ConnectionState::Disconnected);
        assert_ne!(ConnectionState::Connecting, ConnectionState::Disconnected);
    }

    #[test]
    fn connection_state_is_connected() {
        assert!(!ConnectionState::Disconnected.is_connected());
        assert!(!ConnectionState::Connecting.is_connected());
        assert!(ConnectionState::Connected { server_version: 1 }.is_connected());
        assert!(ConnectionState::Ready.is_connected());
        assert!(!ConnectionState::Reconnecting { attempt: 1 }.is_connected());
    }

    #[test]
    fn connection_state_is_ready() {
        assert!(!ConnectionState::Disconnected.is_ready());
        assert!(ConnectionState::Ready.is_ready());
    }
}
