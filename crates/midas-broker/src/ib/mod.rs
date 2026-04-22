//! Slice 4 — router-era IB backend adapter.
//!
//! Parallel path to the legacy
//! [`IbClient`](crate::ib_client::IbClient): this module implements the
//! new [`MarketDataSource`](crate::MarketDataSource) and
//! [`OrderClient`](crate::OrderClient) traits directly on top of
//! `rust-ibapi` 2.10. The legacy adapter stays in place behind the
//! `#[deprecated]` shim until slice 9.
//!
//! Submodule map:
//!
//! * [`config`] — [`IbMarketDataConfig`] (connection + pacing settings).
//! * [`pacing`] — [`PacingGovernor`] (BR-19) plus the
//!   [`TokenBucket`](pacing::TokenBucket) / [`IdenticalKey`] primitives.
//! * [`translation`] — pure helpers between rust-ibapi types and our
//!   `midas-broker-core::market_data` vocabulary.
//! * [`market_data`] — [`IbMarketData`].
//! * [`order_client`] — [`IbOrderClient`].

pub mod config;
pub mod market_data;
pub mod order_client;
pub mod pacing;
pub mod translation;

pub use config::IbMarketDataConfig;
pub use market_data::IbMarketData;
pub use order_client::IbOrderClient;
pub use pacing::{IdenticalKey, PacingConfig, PacingGovernor, PacingPolicy, TokenBucket};
