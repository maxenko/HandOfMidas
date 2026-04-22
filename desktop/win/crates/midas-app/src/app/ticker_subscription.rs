// S7d: per-active-symbol tick subscription stream.
//
// Drives `TickerState::UpdateMarketData` off the router's tick
// broadcast. One `Subscription::run_with` per symbol that has an
// active ticker state. The stream accumulates the latest trade
// price observed inside each 33 ms window and emits at most one
// `Message::TickerLastPrice` per window.
//
// Resolving `SubscriptionHandle<Tick>` follows the same
// registry-per-symbol pattern used by charts: the router-aware
// bind path installs the handle; the `fn`-pointer closure here
// looks it up.

#![allow(dead_code)]

use iced::futures::SinkExt;
use midas_broker_core::market_data::{TickType, TickValue};
use midas_broker_core::SymbolKey;
use tokio::sync::broadcast::error::RecvError;

use super::subscription_helpers::TICKER_EMIT_MS;
use super::subscription_registry;
use crate::app::Message;

/// Hashable key for `Subscription::run_with`.
#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub struct TickerSubKey {
    pub symbol: SymbolKey,
}

pub fn ticker_stream_builder(key: &TickerSubKey) -> impl iced::futures::Stream<Item = Message> {
    let key = key.clone();
    iced::stream::channel(128, async move |mut output| {
        let entry = match subscription_registry::get_ticker_handle(&key.symbol) {
            Some(e) => e,
            None => return,
        };
        let mut rx = entry.resubscribe().await;
        let mut last_price: Option<f64> = None;
        let mut interval = tokio::time::interval(TICKER_EMIT_MS);
        interval.tick().await;
        loop {
            tokio::select! {
                r = rx.recv() => match r {
                    Ok(arc_tick) => {
                        // Only `Last`-typed price ticks drive the
                        // price snapshot; Bid/Ask are folded into
                        // the watchlist path through the router's
                        // Quote watch, not here.
                        if matches!(arc_tick.tick_type, TickType::Last) {
                            match arc_tick.value {
                                TickValue::Price(p) => last_price = Some(p),
                                TickValue::PriceSize { price, .. } => last_price = Some(price),
                                _ => {}
                            }
                        }
                    }
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => break,
                },
                _ = interval.tick() => {
                    if let Some(p) = last_price.take() {
                        if output.send(Message::TickerLastPrice {
                            symbol: key.symbol.clone(),
                            last_price: p,
                        }).await.is_err() {
                            break;
                        }
                    }
                }
            }
        }
    })
}
