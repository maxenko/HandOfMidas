//! Bar aggregator — per-`(symbol, timeframe)` fold of 5 s RT bars into
//! the target timeframe.
//!
//! Spine of slice 6 (see `plan/market-data-router/07-slice-6-aggregator.md`).
//! [`BarAggregatorRegistry`] lazily spawns per-key aggregator tasks,
//! each consuming a shared RT-bar [`SubscriptionHandle`] from the
//! router (NB-6 Model A) and producing a refcounted
//! `broadcast::Sender<Arc<Bar>>`.
//!
//! Consumers never touch this module directly — they reach aggregated
//! bars through [`MarketDataRouter::subscribe_bars`].
//!
//! [`SubscriptionHandle`]: crate::router::SubscriptionHandle
//! [`MarketDataRouter::subscribe_bars`]: crate::router::MarketDataRouter::subscribe_bars

pub mod registry;
pub mod task;

pub use registry::BarAggregatorRegistry;
