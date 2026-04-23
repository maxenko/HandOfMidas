//! Default timeout constants for upstream broker calls.
//!
//! App-harden M1: `subscribe_ticks`, `historical_bars`, and
//! `subscribe_realtime_bars` are `async fn`s with no intrinsic
//! deadline. A stalled IB gateway or a sim that loses its transport
//! would otherwise leave the UI hung on a future that never resolves.
//! The adapter wraps each call in `tokio::time::timeout` with the
//! constant below; on elapse the call returns
//! [`AdapterError::Timeout`](crate::AdapterError::Timeout) and the
//! caller closes the half-opened widget / window.

use std::time::Duration;

/// Default deadline for an upstream broker call. 30 s is generous
/// enough for a cold-started IB Gateway (`reqContractDetails` +
/// subscribe handshake) while still bounding the UI's hang window.
pub const BROKER_CALL_TIMEOUT_SECS: u64 = 30;

/// `Duration` form of [`BROKER_CALL_TIMEOUT_SECS`].
pub const BROKER_CALL_TIMEOUT: Duration = Duration::from_secs(BROKER_CALL_TIMEOUT_SECS);
