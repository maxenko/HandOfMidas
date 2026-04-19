// midas-broker: IB trading engine for Hand of Midas
//
// Public API: start_broker_engine() returns a BrokerHandle with channels.
// No ibapi types leak through this API — the UI crate never imports ibapi.

pub mod client;
pub mod commands;
pub mod config;
pub mod connection;
pub mod db;
pub mod engine;
pub mod error;
pub mod events;
pub mod ib_client;
pub mod ib_data_source;
pub mod ib_strings;
pub mod market_data;
pub mod orders;
pub mod persist;
pub mod test_broker;
pub mod testdata;

// Re-exports for the public API surface
pub use client::{AccountSummary, BrokerCallback, BrokerClient, PositionRecord, TestBrokerClient};
pub use commands::BrokerCommand;
pub use config::BrokerConfig;
pub use connection::ConnectionState;
pub use engine::BrokerHandle;
pub use error::BrokerError;
pub use events::BrokerEvent;
pub use market_data::MarketDataSource;
pub use orders::bracket::{
    BracketGroup, BracketLifecycleStatus, BracketParams, StopLossParams, TakeProfitParams,
};
pub use orders::state::OrderStatus;
pub use orders::types::{BracketRole, LocalOrder, OrderKind};
pub use test_broker::{TestBroker, TestBrokerConfig};

pub use engine::start_broker_engine;
pub use midas_broker_core::SecurityType;
pub use orders::types::{OrderAction, TimeInForce};
