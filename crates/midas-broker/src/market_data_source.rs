//! Router-era [`MarketDataSource`] provider trait.
//!
//! After the audit-P1 refactor the trait itself lives in
//! `midas-broker-core::provider`; this module re-exports it so all
//! existing `crate::market_data_source::{MarketDataSource,
//! HistoricalBarsResult, DynMarketDataSource}` call sites inside
//! `midas-broker` keep compiling unchanged.
//!
//! The pre-existing historical-only trait in
//! [`crate::market_data`](crate::market_data) (also called
//! `MarketDataSource` inside that module) is unused after slice 10g;
//! see that module's **OPEN (post-refactor)** note.

pub use midas_broker_core::provider::{
    DynMarketDataSource, HistoricalBarsResult, MarketDataSource,
};
