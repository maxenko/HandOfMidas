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

use super::subscription_helpers::{FrameCoalescer, TICKER_EMIT_MS};
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
            None => {
                let Some(router) = subscription_registry::router() else {
                    return;
                };
                match router.subscribe_ticks(key.symbol.clone()).await {
                    Ok(handle) => {
                        subscription_registry::install_ticker_handle(key.symbol.clone(), handle);
                        match subscription_registry::get_ticker_handle(&key.symbol) {
                            Some(e) => e,
                            None => return,
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            symbol = %key.symbol.symbol,
                            "subscribe_ticks failed: {e}"
                        );
                        return;
                    }
                }
            }
        };
        let mut rx = entry.resubscribe().await;
        // Ticker emits at most one `UpdateMarketData` per 33 ms
        // window, so the coalescer is a single-slot buffer on the
        // *latest* observed Last price — we don't care about any
        // older prices in the same window. `FrameCoalescer`'s
        // M-30 early-flush is configured aggressively (1) so that
        // any accumulated window emits on the very next drain
        // without waiting; this matches the pre-refactor behaviour
        // of `Option<f64>::take`.
        let mut coalescer: FrameCoalescer<f64> =
            FrameCoalescer::with_capacity_and_max_batch(1, usize::MAX);
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
                            let price = match arc_tick.value {
                                TickValue::Price(p) => Some(p),
                                TickValue::PriceSize { price, .. } => Some(price),
                                _ => None,
                            };
                            if let Some(p) = price {
                                // Keep only the latest observation
                                // per window — drop any prior value
                                // still sitting in the buffer.
                                let _ = coalescer.drain();
                                coalescer.push(p);
                            }
                        }
                    }
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => break,
                },
                _ = interval.tick() => {
                    if let Some(p) = coalescer.drain().into_iter().next_back() {
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
