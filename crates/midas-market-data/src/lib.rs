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

pub mod error;
pub mod router;

pub use error::RouterError;
pub use router::{
    DynMarketDataSource, Guard, GuardedStream, MarketDataRouter, QuoteHandle, SubscriptionHandle,
    SymbolDebugInfo,
};
