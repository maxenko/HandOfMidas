//! Iced subscription that fans the router's
//! [`MarketDataSource::connection_state`] watch into
//! [`Message::BrokerConnectionChanged`].
//!
//! Without this, the disconnect banner stays up forever on every chart
//! load. The legacy `BrokerEvent::Connected/Disconnected` path only
//! dispatched `BrokerConnectionChanged` for the retired broker engine —
//! the router-era construction path (`Sim` synchronous, `IB` via
//! `Message::RouterReady`) never told the UI it was Ready, so
//! `broker_connection_display` stayed pinned at its initial
//! `"Connecting"` value and the banner predicate
//! `matches!(state, "Ready" | "Connected")` never flipped true.
//!
//! This module subscribes to `router.connection_state()` once and
//! emits one `BrokerConnectionChanged(<display>)` per state change.
//! Sim arrives `Ready` immediately on construction; IB walks
//! `Connecting → Connected{..} → Ready` driven by the connect handshake.

use std::sync::Arc;

use iced::futures::Stream;
use midas_broker_core::market_data::ConnectionState;
use midas_market_data::MarketDataRouter;
use tokio_stream::wrappers::WatchStream;
use tokio_stream::StreamExt;

use crate::app::Message;

/// Source for the connection-state subscription.
///
/// Wraps an [`Arc<MarketDataRouter>`] so iced can hash + clone it
/// across re-diffs. Identity is stable across `Arc` clones of the
/// same router, so the subscription does not get re-created on
/// every render.
#[derive(Clone)]
pub struct ConnectionStateSource {
    pub router: Arc<MarketDataRouter>,
}

impl std::hash::Hash for ConnectionStateSource {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        // Static name — there's exactly one connection-state stream
        // per router instance, and `MidasApp` holds at most one
        // router. Adding the `Arc` pointer would also be valid but
        // adds nothing here.
        "router-connection-state-source".hash(state);
    }
}

/// Convert a [`ConnectionState`] into the display string the
/// view-layer's banner predicate keys off
/// (`matches!(state, "Ready" | "Connected")`).
fn display_for(state: &ConnectionState) -> String {
    match state {
        ConnectionState::Disconnected => "Disconnected".to_string(),
        ConnectionState::Connecting => "Connecting".to_string(),
        ConnectionState::Reconnecting { .. } => "Reconnecting".to_string(),
        // Pre-Ready handshake — banner predicate treats this as
        // healthy because order routing is unblocked.
        ConnectionState::Connected { .. } => "Connected".to_string(),
        ConnectionState::Ready => "Ready".to_string(),
    }
}

/// Implementation hook for `Subscription::run_with`. Returns a stream
/// that emits one [`Message::BrokerConnectionChanged`] per
/// distinct state on the router's `connection_state` watch.
pub fn connection_state_stream(
    source: &ConnectionStateSource,
) -> impl Stream<Item = Message> + Send + 'static {
    let rx = source.router.connection_state();
    // `WatchStream::new` emits the CURRENT value first, then every
    // subsequent `changed`. That immediate first emit is exactly
    // what we want — it carries the `"Ready"` (sim) or
    // `"Connecting"` (IB) state into the UI on the first iced diff
    // after the router lands.
    WatchStream::new(rx).map(|state| Message::BrokerConnectionChanged(display_for(&state)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_strings_match_banner_predicate() {
        // The banner predicate in `app/views.rs::view_account_panel`:
        //   matches!(state.as_str(), "Ready" | "Connected")
        // is healthy. Anything else is unhealthy. Pin every variant
        // here so a future ConnectionState change can't silently
        // wrong-foot the banner.
        assert_eq!(display_for(&ConnectionState::Disconnected), "Disconnected");
        assert_eq!(display_for(&ConnectionState::Connecting), "Connecting");
        assert_eq!(
            display_for(&ConnectionState::Reconnecting { attempt: 1 }),
            "Reconnecting"
        );
        assert_eq!(
            display_for(&ConnectionState::Connected {
                server_version: 176
            }),
            "Connected"
        );
        assert_eq!(display_for(&ConnectionState::Ready), "Ready");
    }
}
