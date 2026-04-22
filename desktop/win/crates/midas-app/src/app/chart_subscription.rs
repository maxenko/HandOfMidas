// S7b: per-chart bar subscription stream.
//
// Each visible chart has its own iced subscription keyed by
// `(ChartId, SymbolKey, Timeframe)`. Inside the stream we resolve
// the `SubscriptionHandle<Bar>` from the static
// `subscription_registry::CHART_REGISTRY`, which is populated by
// `bind_chart_to_symbol` when the router is present. The builder
// is a `fn` pointer — `Subscription::run_with`'s constraint — and
// therefore cannot capture an `Arc<MarketDataRouter>` directly.
//
// Audit P1 refactor 2: the select-loop scaffolding moved to
// `subscription_stream::drive_subscription`; this file now owns
// only the chart-specific resolve + message-shaping glue.

#![allow(dead_code)]

use std::time::Duration;

use midas_broker_core::market_data::Bar;
use midas_broker_core::{SymbolKey, Timeframe};
use midas_core::ChartId;

use super::subscription_helpers::{FrameCoalescer, FRAME_COALESCE_MS};
use super::subscription_registry;
use super::subscription_stream::{drive_subscription, BatchEmit};
use crate::app::Message;

/// Hashable key carried into `Subscription::run_with` as the `data`
/// parameter; combined with the `fn` pointer below it forms the
/// subscription identity iced diffs on.
#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub struct ChartSubKey {
    pub chart_id: ChartId,
    pub symbol: SymbolKey,
    pub timeframe: Timeframe,
}

/// Build a stream of coalesced bar batches for a single chart.
///
/// Looks up the `SubscriptionHandle<Bar>` in
/// `subscription_registry::CHART_REGISTRY` on first tick. If no
/// handle is registered (router not yet ready, or the chart was
/// closed between the subscription dispatch and this closure running)
/// the stream exits immediately — iced re-diffs on the next `view()`
/// and re-runs the builder if the chart is still alive.
pub fn chart_stream_builder(key: &ChartSubKey) -> impl iced::futures::Stream<Item = Message> {
    let key = key.clone();
    iced::stream::channel(64, async move |output| {
        let reg_key = subscription_registry::ChartKey {
            symbol: key.symbol.clone(),
            timeframe: key.timeframe,
            chart_id: key.chart_id,
        };
        // Look up or lazy-subscribe: iced's `fn`-pointer builders
        // can't capture the router, so we resolve it out of the
        // process-scoped `subscription_registry::router()` slot
        // and call `subscribe_bars` on first run. Subsequent
        // re-diffs for the same key reuse the installed handle.
        let entry = if let Some(e) = subscription_registry::get_chart_handle(&reg_key) {
            e
        } else {
            let Some(router) = subscription_registry::router() else {
                return;
            };
            match router
                .subscribe_bars(key.symbol.clone(), key.timeframe)
                .await
            {
                Ok(handle) => {
                    subscription_registry::install_chart_handle(reg_key.clone(), handle);
                    let Some(e) = subscription_registry::get_chart_handle(&reg_key) else {
                        return;
                    };
                    e
                }
                Err(e) => {
                    tracing::warn!(
                        chart_id = ?key.chart_id,
                        symbol = %key.symbol.symbol,
                        "subscribe_bars failed: {e}"
                    );
                    return;
                }
            }
        };
        let rx = entry.resubscribe().await;
        let chart_id = key.chart_id;
        drive_subscription(
            output,
            rx,
            FrameCoalescer::<Bar>::with_capacity(8),
            FRAME_COALESCE_MS,
            |buf, arc_bar| buf.push((*arc_bar).clone()),
            |buf| {
                let bars = buf.drain();
                BatchEmit::One(Message::ChartBarBatch { chart_id, bars })
            },
            move |_n| Some(Message::ChartResync { chart_id }),
        )
        .await;
    })
}

// `run_with` takes `D: Hash + 'static` + `fn(&D) -> S` where S is a
// Stream. Keep `data` ownership in the caller; this crate only
// provides the builder.

/// Duration constant re-exposed so `MidasApp::chart_subscriptions`
/// can state it explicitly in its doc tree.
pub const COALESCE_WINDOW: Duration = FRAME_COALESCE_MS;
