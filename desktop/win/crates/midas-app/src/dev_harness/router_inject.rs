//! S8d: parser + dispatcher for `DevloopCmd::InjectMarketEvent`.
//!
//! Unlike [`super::broker_inject`], this path doesn't synthesise a
//! `BrokerEvent` — it deserialises a `midas_broker_core::market_data::MarketEvent`
//! straight from the JSON value and hands it to the router-owned
//! provider's `inject_for_test`. Only the sim provider routes; the
//! real IB provider is a no-op.

use midas_broker_core::market_data::MarketEvent;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum InjectMarketError {
    #[error("no router mounted on the app yet — cannot inject")]
    NoRouter,
    #[error("malformed MarketEvent payload: {0}")]
    Serde(#[from] serde_json::Error),
}

/// Parse `value` as a `MarketEvent` and push it into the router's
/// underlying provider. The provider is held by the router as
/// `Arc<dyn MarketDataSource>`; we reach it through
/// `MarketDataRouter::source_for_test()` (gated on the `test_inject`
/// feature, enabled transitively by `dev_harness`).
///
/// Returns the variant name for logging / responder bodies.
pub fn apply(
    app: &crate::app::MidasApp,
    value: &serde_json::Value,
) -> Result<&'static str, InjectMarketError> {
    let Some(router) = app.router.as_ref() else {
        return Err(InjectMarketError::NoRouter);
    };
    let event: MarketEvent = serde_json::from_value(value.clone())?;
    let variant = variant_name(&event);
    let source = router.source_for_test();
    source.inject_for_test(event);
    Ok(variant)
}

fn variant_name(event: &MarketEvent) -> &'static str {
    match event {
        MarketEvent::Tick(_) => "Tick",
        MarketEvent::Bar(_) => "Bar",
        MarketEvent::FarmStatus(_) => "FarmStatus",
        MarketEvent::ConnectionState(_) => "ConnectionState",
        MarketEvent::OrderingReady { .. } => "OrderingReady",
        MarketEvent::SubscriptionAccepted { .. } => "SubscriptionAccepted",
        MarketEvent::SubscriptionEnded { .. } => "SubscriptionEnded",
        MarketEvent::Historical(_) => "Historical",
        MarketEvent::HistoricalDataEnd { .. } => "HistoricalDataEnd",
        MarketEvent::HistoricalUpdate(_) => "HistoricalUpdate",
        MarketEvent::Error { .. } => "Error",
    }
}
