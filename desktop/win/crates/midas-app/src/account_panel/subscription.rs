//! Coalesced broker-event subscription for the Positions store.
//!
//! Wraps the broker engine's broadcast channel in a `BroadcastStream`,
//! filters for [`midas_broker::BrokerEvent::PositionUpdate`]s, and buckets
//! them into 50 ms windows of up to 256 events. Each window is folded
//! into a last-wins-per-symbol `Vec<PositionRaw>` so the iced `update()`
//! loop receives at most one message per window even during a reconnect
//! backfill storm.
//!
//! Wire this via iced's `Subscription::run_with` at the `main.rs` sub
//! site, mapping each batch to the top-level
//! `Message::AccountPositionsBatch(batch)` variant.
//!
//! Lagged `BroadcastStream` items are silently dropped for v1 —
//! if the `PositionStore` ever falls behind we'll add a resubscribe
//! primitive. In the meantime the broker-events subscription in
//! `broker_bridge::broker_event_stream` logs a `warn!` on lag, so the
//! condition is still observable.

use std::collections::HashMap;
use std::time::Duration;

use iced::futures::StreamExt;
use iced::Subscription;
use tokio_stream::wrappers::{errors::BroadcastStreamRecvError, BroadcastStream};
use tokio_stream::StreamExt as TokioStreamExt;

use midas_broker::BrokerEvent;

use crate::broker_bridge::BrokerEventSource;

use super::positions_store::PositionRaw;

/// Maximum events per coalesced window. Large enough to swallow a full
/// reconnect backfill in a single pass at 200 positions/sec.
const CHUNK_CAP: usize = 256;

/// Window duration for batching. Matches the plan's 50 ms target; short
/// enough to keep Positions tab snappy, long enough to collapse a burst.
const CHUNK_INTERVAL: Duration = Duration::from_millis(50);

/// Build the Positions subscription stream.
///
/// The caller is responsible for mapping each yielded `Vec<PositionRaw>`
/// into whatever top-level `Message` variant carries the batch — this
/// module intentionally stays agnostic of the app's `Message` enum.
pub fn positions_subscription(source: BrokerEventSource) -> Subscription<Vec<PositionRaw>> {
    Subscription::run_with(source, positions_stream)
}

/// Implementation hook for `Subscription::run_with`. Public so iced's
/// internal typing can name it; not intended for direct callers.
///
/// NOTE: `Subscription::run_with` requires a `fn` pointer (not a
/// closure), which is why this is a free function rather than a
/// method on `BrokerEventSource`.
pub fn positions_stream(
    source: &BrokerEventSource,
) -> impl iced::futures::Stream<Item = Vec<PositionRaw>> {
    let rx = source.sender.subscribe();
    // Use the tokio_stream method name explicitly — `StreamExt` is in
    // scope from both `iced::futures` and `tokio_stream`, and they both
    // provide `filter_map`. The tokio variant accepts an `async` closure
    // body, which is what we want here.
    //
    // `Lagged(n)` surfaces when the broadcast receiver falls behind the
    // 4096-slot channel. For ops visibility we log a `warn!` — the
    // positions store tolerates gaps because `PositionUpdate` is a full
    // snapshot, not a delta, so the next event restores the correct
    // state. The broker-event subscription in `broker_bridge` logs the
    // same condition but on its own stream; we log here so Positions
    // operators aren't blind to their receiver lagging independently.
    let raw = tokio_stream::StreamExt::filter_map(
        BroadcastStream::new(rx),
        |r: Result<BrokerEvent, BroadcastStreamRecvError>| match r {
            Ok(ev) => Some(ev),
            Err(BroadcastStreamRecvError::Lagged(n)) => {
                tracing::warn!(
                    skipped = n,
                    "positions_subscription: broadcast lagged; \
                     next PositionUpdate will refresh from broker"
                );
                None
            }
        },
    );
    let positions = tokio_stream::StreamExt::filter_map(raw, |event: BrokerEvent| {
        if let BrokerEvent::PositionUpdate {
            symbol,
            quantity,
            avg_cost,
            ..
        } = event
        {
            Some(PositionRaw {
                symbol,
                qty: quantity,
                avg_cost,
                last_price: None,
                last_price_ts: None,
                session_open_price: None,
            })
        } else {
            None
        }
    });
    let batched = positions.chunks_timeout(CHUNK_CAP, CHUNK_INTERVAL);
    StreamExt::map(batched, fold_latest_per_symbol)
}

// ── Router-era path (S7d / BR-14) ────────────────────────────────────

/// Router-era positions subscription source. Wraps an
/// `Arc<dyn OrderClient>` so the stream builder can call
/// `position_events()` to obtain a fresh broadcast receiver each
/// time iced re-diffs. Parallel to [`BrokerEventSource`]; the
/// legacy variant is removed in S9.
#[derive(Clone)]
pub struct PositionEventsSource {
    pub order_client: std::sync::Arc<dyn midas_broker::OrderClient>,
}

impl std::hash::Hash for PositionEventsSource {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        // Name-based identity is stable across Arc clones of the
        // same order client; iced uses this to decide whether two
        // consecutive subscription calls reference the same stream.
        "router-position-events-source".hash(state);
        self.order_client.name().hash(state);
    }
}

/// Router-era positions subscription (BR-14).
///
/// Same fold + window shape as [`positions_subscription`] but sourced
/// from `OrderClient::position_events()` instead of the legacy
/// `BrokerEventSource`.
pub fn router_positions_subscription(
    source: PositionEventsSource,
) -> Subscription<Vec<PositionRaw>> {
    Subscription::run_with(source, router_positions_stream)
}

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
