//! iced subscription source for the router's `OrderClient::order_events`
//! broadcast.
//!
//! Audit P1 refactor 4: extracted from the old
//! `crate::bracket_submit` module so the iced-adjacent glue lives
//! next to the other per-consumer stream builders under
//! `src/app/`. The pure broker-side bracket-submission helper now
//! lives in [`midas_broker::bracket`].
//!
//! [`OrderEventsSource`] must implement `Hash` for
//! `Subscription::run_with` diffing, which is why it can't move
//! alongside [`midas_broker::BracketSubmitter`].

#![allow(dead_code)]

use std::sync::Arc;

use midas_broker::{OrderClient, OrderEvent};

/// Source for an iced subscription that fans every [`OrderEvent`]
/// emitted by the router's [`OrderClient::order_events`] broadcast
/// into [`crate::app::Message::RouterOrderEvent`].
///
/// Shape mirrors [`crate::account_panel::PositionEventsSource`] —
/// `Clone + Hash` so iced's `Subscription::run_with` diff keeps a
/// single stream alive across `update()` iterations.
#[derive(Clone)]
pub struct OrderEventsSource {
    /// Shared order-client handle; the stream builder calls
    /// [`OrderClient::order_events`] on it each time iced re-diffs.
    pub order_client: Arc<dyn OrderClient>,
}

impl std::hash::Hash for OrderEventsSource {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        "router-order-events-source".hash(state);
        self.order_client.name().hash(state);
    }
}

/// Stream builder for [`OrderEventsSource`]. Subscribes fresh to the
/// order-client's broadcast channel on every iced re-diff, filter-maps
/// `Lagged` errors into a `warn!` (the blotter tolerates gaps — the
/// next status callback is authoritative), and yields each surviving
/// [`OrderEvent`] wrapped in [`crate::app::Message::RouterOrderEvent`].
pub fn order_events_stream(
    source: &OrderEventsSource,
) -> impl iced::futures::Stream<Item = crate::app::Message> {
    use iced::futures::StreamExt;
    use tokio_stream::wrappers::{errors::BroadcastStreamRecvError, BroadcastStream};
    BroadcastStream::new(source.order_client.order_events())
        .filter_map(|r| {
            std::future::ready(match r {
                Ok(ev) => Some(ev),
                Err(BroadcastStreamRecvError::Lagged(n)) => {
                    tracing::warn!(skipped = n, "order_events_stream: broadcast lagged");
                    None
                }
            })
        })
        .map(|ev: OrderEvent| crate::app::Message::RouterOrderEvent(Box::new(ev)))
}
