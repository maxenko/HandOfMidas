use crate::orders::state::OrderStatus;
use thiserror::Error;
use uuid::Uuid;

/// All errors that can occur in the midas-broker crate.
#[derive(Debug, Error)]
pub enum BrokerError {
    /// TCP / WebSocket connection failure.
    #[error("connection error: {0}")]
    Connection(String),

    /// Error code returned by the IB TWS / Gateway API.
    #[error("IB API error {code}: {message}")]
    IbApi { code: i32, message: String },

    /// Looked up an order by UUID but it does not exist.
    #[error("order not found: {0}")]
    OrderNotFound(Uuid),

    /// Attempted a state transition that the state machine forbids.
    #[error("invalid order status transition: {from} -> {to}")]
    InvalidTransition { from: OrderStatus, to: OrderStatus },

    /// SQLite persistence error.
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),

    /// JSON serialization / deserialization error.
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// Rate-limit exceeded (IB pacing violations, etc.).
    #[error("rate limit: {0}")]
    RateLimit(String),

    /// Configuration is invalid or missing.
    #[error("config error: {0}")]
    Config(String),

    /// Catch-all for unexpected internal errors.
    #[error("internal error: {0}")]
    Internal(String),

    /// Attempted an operation that requires a live IB connection.
    #[error("not connected to IB gateway")]
    NotConnected,
}
