//! Coalesced positions subscription for the `PositionStore`.
//!
//! Wraps the router-era [`OrderClient::position_events`] broadcast
//! channel in a `BroadcastStream`, buckets updates into 50 ms windows
//! of up to 256 events, and folds each window to a last-wins-per-symbol
//! `Vec<PositionRaw>` so the iced `update()` loop receives at most one
//! message per window even during a reconnect backfill storm.
//!
//! Wire this via iced's `Subscription::run_with` at the `main.rs` sub
//! site, mapping each batch to the top-level
//! `Message::AccountPositionsBatch(batch)` variant.
//!
//! Lagged `BroadcastStream` items are silently dropped for v1 —
//! `PositionUpdate` is a full snapshot (not a delta), so the next event
//! restores the correct state.

use std::collections::HashMap;
use std::time::Duration;

use iced::futures::StreamExt;
use iced::Subscription;
use tokio_stream::wrappers::{errors::BroadcastStreamRecvError, BroadcastStream};
use tokio_stream::StreamExt as TokioStreamExt;

use super::positions_store::PositionRaw;

/// Maximum events per coalesced window. Large enough to swallow a full
/// reconnect backfill in a single pass at 200 positions/sec.
const CHUNK_CAP: usize = 256;

/// Window duration for batching. Matches the plan's 50 ms target; short
/// enough to keep the Positions tab snappy, long enough to collapse a
/// burst.
const CHUNK_INTERVAL: Duration = Duration::from_millis(50);

// ── Router-era path (BR-14) ─────────────────────────────────────────

/// Source for the router-era positions subscription. Wraps an
/// `Arc<dyn OrderClient>` so the stream builder can call
/// `position_events()` to obtain a fresh broadcast receiver each time
/// iced re-diffs.
#[derive(Clone)]
pub struct PositionEventsSource {
    /// Shared order client the app swapped in on `Message::RouterReady`
    /// (sim constructs synchronously).
    pub order_client: std::sync::Arc<dyn midas_broker::OrderClient>,
}

impl std::hash::Hash for PositionEventsSource {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        // Name-based identity is stable across Arc clones of the same
        // order client; iced uses this to decide whether two
        // consecutive subscription calls reference the same stream.
        "router-position-events-source".hash(state);
        self.order_client.name().hash(state);
    }
}

/// Router-era positions subscription (BR-14).
///
/// Coalesces `OrderClient::position_events()` into 50 ms windows of at
/// most 256 events, folding each window to one update per symbol.
pub fn router_positions_subscription(
    source: PositionEventsSource,
) -> Subscription<Vec<PositionRaw>> {
    Subscription::run_with(source, router_positions_stream)
}

/// Implementation hook for `Subscription::run_with`. Public so iced's
/// internal typing can name it; not intended for direct callers.
pub fn router_positions_stream(
    source: &PositionEventsSource,
) -> impl iced::futures::Stream<Item = Vec<PositionRaw>> {
    let rx = source.order_client.position_events();
    let raw = tokio_stream::StreamExt::filter_map(
        BroadcastStream::new(rx),
        |r: Result<midas_broker::PositionUpdate, BroadcastStreamRecvError>| match r {
            Ok(ev) => Some(ev),
            Err(BroadcastStreamRecvError::Lagged(n)) => {
                tracing::warn!(
                    skipped = n,
                    "router_positions_subscription: broadcast lagged"
                );
                None
            }
        },
    );
    let positions =
        tokio_stream::StreamExt::map(raw, |update: midas_broker::PositionUpdate| PositionRaw {
            symbol: update.symbol,
            qty: update.quantity,
            avg_cost: update.avg_cost,
            last_price: None,
            last_price_ts: None,
            session_open_price: None,
        });
    let batched = positions.chunks_timeout(CHUNK_CAP, CHUNK_INTERVAL);
    StreamExt::map(batched, fold_latest_per_symbol)
}

/// Collapse a window of `PositionRaw` updates so each symbol appears at
/// most once. Later entries win — `PositionUpdate` is a full snapshot
/// of the current position, not a delta, so discarding intermediate
/// values is lossless.
pub fn fold_latest_per_symbol(batch: Vec<PositionRaw>) -> Vec<PositionRaw> {
    let mut latest: HashMap<String, PositionRaw> = HashMap::with_capacity(batch.len());
    for raw in batch {
        latest.insert(raw.symbol.clone(), raw);
    }
    latest.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(symbol: &str, qty: f64, avg_cost: f64) -> PositionRaw {
        PositionRaw {
            symbol: symbol.to_string(),
            qty,
            avg_cost,
            last_price: None,
            last_price_ts: None,
            session_open_price: None,
        }
    }

    #[test]
    fn fold_empty_is_empty() {
        let out = fold_latest_per_symbol(Vec::new());
        assert!(out.is_empty());
    }

    #[test]
    fn fold_single_symbol_last_wins() {
        let batch = vec![raw("AAPL", 100.0, 150.0), raw("AAPL", 200.0, 160.0)];
        let out = fold_latest_per_symbol(batch);
        assert_eq!(out.len(), 1);
        let row = &out[0];
        assert_eq!(row.qty, 200.0);
        assert_eq!(row.avg_cost, 160.0);
    }

    #[test]
    fn fold_preserves_distinct_symbols() {
        let batch = vec![
            raw("AAPL", 100.0, 150.0),
            raw("GME", -50.0, 18.0),
            raw("AS", 200.0, 12.0),
        ];
        let out = fold_latest_per_symbol(batch);
        assert_eq!(out.len(), 3);
        let syms: std::collections::HashSet<_> = out.iter().map(|r| r.symbol.clone()).collect();
        assert!(syms.contains("AAPL"));
        assert!(syms.contains("GME"));
        assert!(syms.contains("AS"));
    }

    #[test]
    fn fold_mixed_duplicates_and_uniques() {
        let batch = vec![
            raw("AAPL", 100.0, 150.0),
            raw("GME", -50.0, 18.0),
            raw("AAPL", 150.0, 152.0),
            raw("AS", 200.0, 12.0),
            raw("GME", -25.0, 18.5),
        ];
        let out = fold_latest_per_symbol(batch);
        assert_eq!(out.len(), 3);
        let by_sym: std::collections::HashMap<_, _> =
            out.into_iter().map(|r| (r.symbol.clone(), r)).collect();
        assert_eq!(by_sym["AAPL"].qty, 150.0);
        assert_eq!(by_sym["GME"].qty, -25.0);
        assert_eq!(by_sym["AS"].qty, 200.0);
    }
}
