//! Market-data error classification.
//!
//! The router surfaces every upstream IB failure through two layers:
//!
//! * [`ErrorCode`] — classified code, used on
//!   `MarketEvent::Error { code, .. }` for router observability. Codes
//!   cover the M-15 set: `10089`, `354`, `10167` (`354` and `10167` are
//!   deliberately distinct — "delayed subscription" vs "requires
//!   additional subscription"), `300`, `200`, `201`, `202`, `162`,
//!   `322`, `10147`, `321`, plus router-internal kinds.
//! * [`MarketDataError`] — `thiserror` enum returned from provider
//!   methods. The router converts these back into `MarketEvent::Error`
//!   where appropriate.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::bar::Timeframe;
use super::req_id::ReqId;

/// Classified market-data / order error code.
///
/// Non-exhaustive so new IB codes can be recognised without breaking
/// consumers. `Other(i32)` is the catch-all for codes we haven't
/// classified yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ErrorCode {
    /// IB 10089 — symbol lacks the subscription entitlement.
    NoMarketDataPermission,
    /// IB 354 — delayed market data is subscribed (informational, not
    /// the same as `RequiresAdditionalSubscription`).
    DelayedMarketDataSubscribed,
    /// IB 10167 — requires an additional data-subscription bundle.
    RequiresAdditionalSubscription,
    /// IB 300 — invalid `reqId`.
    InvalidReqId,
    /// IB 200 — no security definition found.
    NoSecurityDefinition,
    /// IB 201 — order size invalid.
    OrderSizeInvalid,
    /// IB 202 — order rejected.
    OrderRejected,
    /// IB 162 — historical data service error.
    HistoricalDataServiceError,
    /// IB 322 — duplicate ticker id.
    DuplicateTickerId,
    /// IB 10147 — order cancellation failed (not found).
    OrderCancelNotFound,
    /// IB 321 — validation failure.
    Validation,
    /// IB 100-family — pacing violation.
    PacingViolation,
    /// Router-internal — streaming line limit reached (BR-19).
    StreamingLineLimitExceeded,
    /// Router-internal — request used a timeframe the aggregator
    /// cannot synthesise (BR-22).
    UnsupportedTimeframe(Timeframe),
    /// Router-internal — router is shutting down.
    ShuttingDown,
    /// Any other IB numeric code, kept raw.
    Other(i32),
}

/// Error returned from provider + router market-data methods.
///
/// Uses `thiserror` so `#[from]` conversions from `std::io::Error` work
/// out of the box. `Timeframe` and `ReqId` are carried typed so the
/// caller can re-classify without string parsing.
#[derive(Debug, Error)]
pub enum MarketDataError {
    /// Symbol lacks the subscription entitlement.
    #[error("no market data permission: {symbol}")]
    NoPermission {
        /// Symbol that triggered the rejection.
        symbol: String,
    },
    /// Requires an additional subscription bundle (IB 10167).
    #[error("requires additional subscription: {symbol}")]
    RequiresAdditionalSubscription {
        /// Symbol that triggered the rejection.
        symbol: String,
    },
    /// Invalid `reqId` — typically due to a use-after-cancel race.
    #[error("invalid reqId: {0}")]
    InvalidReqId(ReqId),
    /// Underlying broker connection is down.
    #[error("disconnected")]
    Disconnected,
    /// IB pacing-violation family (100, 101, 102, …).
    #[error("pacing violation: {0}")]
    PacingViolation(String),
    /// Router-internal — streaming line limit reached.
    #[error("streaming line limit exceeded")]
    StreamingLineLimitExceeded,
    /// Aggregator cannot synthesise the requested timeframe.
    #[error("unsupported timeframe: {0:?}")]
    UnsupportedTimeframe(Timeframe),
    /// Router is shutting down; caller should bail.
    #[error("shutting down")]
    ShuttingDown,
    /// Operation is not supported on this source (e.g.
    /// `inject_for_test` on the real IB adapter).
    #[error("unsupported on this source")]
    Unsupported,
    /// Raw I/O failure bubbled up from a provider.
    #[error("i/o: {0}")]
    Io(#[from] std::io::Error),
    /// Anything else, kept as a message string.
    #[error("{0}")]
    Other(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_code_serde_roundtrip() {
        for code in [
            ErrorCode::NoMarketDataPermission,
            ErrorCode::DelayedMarketDataSubscribed,
            ErrorCode::RequiresAdditionalSubscription,
            ErrorCode::InvalidReqId,
            ErrorCode::NoSecurityDefinition,
            ErrorCode::OrderSizeInvalid,
            ErrorCode::OrderRejected,
            ErrorCode::HistoricalDataServiceError,
            ErrorCode::DuplicateTickerId,
            ErrorCode::OrderCancelNotFound,
            ErrorCode::Validation,
            ErrorCode::PacingViolation,
            ErrorCode::StreamingLineLimitExceeded,
            ErrorCode::UnsupportedTimeframe(Timeframe::M1),
            ErrorCode::ShuttingDown,
            ErrorCode::Other(9999),
        ] {
            let json = serde_json::to_string(&code).unwrap();
            let back: ErrorCode = serde_json::from_str(&json).unwrap();
            assert_eq!(code, back);
        }
    }

    #[test]
    fn error_code_354_vs_10167_are_distinct() {
        // M-15: these must be separate variants to keep caller logic
        // that differs between "delayed data" and "needs upgrade"
        // tractable.
        assert_ne!(
            ErrorCode::DelayedMarketDataSubscribed,
            ErrorCode::RequiresAdditionalSubscription
        );
    }

    #[test]
    fn market_data_error_display() {
        let e = MarketDataError::NoPermission {
            symbol: "AAPL".into(),
        };
        assert_eq!(e.to_string(), "no market data permission: AAPL");
        let e = MarketDataError::InvalidReqId(ReqId(7));
        assert_eq!(e.to_string(), "invalid reqId: 7");
        let e = MarketDataError::UnsupportedTimeframe(Timeframe::H4);
        assert!(e.to_string().contains("H4"));
    }

    #[test]
    fn market_data_error_from_io() {
        let io = std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "nope");
        let e: MarketDataError = io.into();
        assert!(matches!(e, MarketDataError::Io(_)));
    }

    #[test]
    fn error_code_hash_eq_consistency() {
        use std::collections::HashSet;
        let mut set: HashSet<ErrorCode> = HashSet::new();
        set.insert(ErrorCode::NoMarketDataPermission);
        set.insert(ErrorCode::NoMarketDataPermission);
        set.insert(ErrorCode::InvalidReqId);
        assert_eq!(set.len(), 2);
    }
}
