//! `midas-market-data` — per-symbol fan-out router for the Hand of
//! Midas trading platform.
//!
//! This crate sits between the provider layer (`midas-broker`) and
//! app-side consumers. It exposes [`MarketDataRouter`], which
//! refcounts upstream subscriptions, fans one tick out to every
//! subscriber on a per-symbol broadcast, coalesces ticks into a
//! per-symbol watched [`Quote`], and provides a `history_then_live`
//! seam utility that guarantees no gap and no duplicate at the
//! history/live boundary.
//!
//! See the [plan](../../../plan/market-data-router/06-slice-5-router.md)
//! for the authoritative design.
//!
//! # Disconnect policy (slice B2)
//!
//! When a publisher observes `RecvError::Closed` on its upstream
//! stream — i.e. the underlying [`MarketDataSource`] dropped the
//! corresponding broadcast sender — the publisher fires
//! `RouterMsg::UpstreamClosed { symbol, reason }` to the control
//! actor. The actor flips the per-hub `end_reason` watch to
//! `Some(EndReason::Disconnected)` (consumers observe this via
//! [`SubscriptionHandle::end_reason`]) BEFORE removing the hub from
//! `state.per_symbol`, then aborts the sibling publisher and emits a
//! structured `tracing::warn!` with `symbol`, `subscriber_count`,
//! `hub_uptime_ms`, and `reason`. The next subscribe for that symbol
//! goes through the first-subscribe path, spawns a fresh hub, and
//! emits a matching `tracing::info!` "upstream reopened; new hub" for
//! diagnostic symmetry. Farm-status transitions
//! (`FarmStatus { code: MarketDataFarmInactive, .. }`) do NOT trigger
//! this tear-down — they remain a side-channel signal on
//! `farm_status_tx` to avoid resubscribe churn during routine
//! IB-gateway hiccups.
//!
//! [`MarketDataSource`]: midas_broker_core::provider::MarketDataSource
//! [`SubscriptionHandle::end_reason`]: crate::router::SubscriptionHandle::end_reason
//!
//! ```no_run
//! # async fn example(router: std::sync::Arc<midas_market_data::MarketDataRouter>) {
//! use midas_broker_core::SymbolKey;
//!
//! let sym = SymbolKey { contract_id: 265598, symbol: "AAPL".into() };
//! let mut handle = router.subscribe_ticks(sym).await.expect("subscribe");
//! // Dropping `handle` DecRefs upstream automatically (RAII).
//! let _ = handle.recv().await;
//! # }
//! ```
//!
//! [`Quote`]: midas_broker_core::market_data::Quote

#![deny(missing_docs)]

pub mod aggregator;
pub mod error;
pub mod router;

pub use aggregator::BarAggregatorRegistry;
pub use error::RouterError;
pub use router::{
    DynMarketDataSource, Guard, GuardedStream, MarketDataRouter, QuoteHandle, SubscriptionHandle,
    SymbolDebugInfo,
};
