//! Tiny helpers shared by the sim tick-emitter and market-data
//! subsystems.
//!
//! Kept in its own module so the two mutually-using siblings
//! ([`market_data`](super::market_data), [`tick_emitter`](super::tick_emitter))
//! don't grow a cyclic import.

/// Wall-clock milliseconds since the Unix epoch.
///
/// Used by the cancel drain-window GC sweep; the tick emitter itself
/// is pause-time-driven (`tokio::time::interval`).
pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
