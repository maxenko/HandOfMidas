//! Router-era IB backend adapter.
//!
//! Implements the [`MarketDataSource`](crate::MarketDataSource) and
//! [`OrderClient`](crate::OrderClient) traits directly on top of
//! `rust-ibapi` 2.10. Successor to the retired `IbClient` /
//! `IbDataSource` standalone adapters.
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
