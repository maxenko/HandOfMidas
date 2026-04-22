//! Slice 3: IB-faithful sim implementation of [`MarketDataSource`] and
//! [`OrderClient`].
//!
//! See `plan/market-data-router/04-slice-3-sim-backend.md` for the
//! authoritative design. The sim lives alongside — but does not
//! replace — the legacy [`TestBroker`](crate::test_broker::TestBroker);
//! both compile simultaneously until slice 9 retires the legacy path.
//!
//! Module layout:
//!
//! | Module | Purpose |
//! |--------|---------|
//! | [`config`] | [`SimConfig`](config::SimConfig) + per-slice sub-configs |
//! | [`rng`] | Xorshift64 RNG helper |
//! | [`market_data`] | [`SimMarketData`] implementing [`MarketDataSource`](crate::MarketDataSource) |
//! | [`order_client`] | [`SimOrderClient`] implementing [`OrderClient`](crate::OrderClient) |
//! | [`tick_emitter`] | Background tick-loop driving [`SimMarketData`] |
//! | `market_data_helpers` | Tiny shared helpers used across modules |

pub mod config;
pub mod market_data;
pub(crate) mod market_data_helpers;
pub mod order_client;
pub mod rng;
pub(crate) mod tick_emitter;

pub use config::{SimConfig, SimMarketDataConfig, SimOrderConfig};
pub use market_data::{OrderingReadyEvent, SimMarketData};
pub use order_client::SimOrderClient;
pub use rng::Xorshift64;
