//! [`IbMarketDataConfig`] — connection + pacing settings for
//! [`IbMarketData`](super::market_data::IbMarketData) and
//! [`IbOrderClient`](super::order_client::IbOrderClient).

use super::pacing::PacingConfig;

/// Configuration for the router-era IB adapter.
///
/// Carries the TWS/Gateway endpoint, API client id, and an embedded
/// [`PacingConfig`]. Connection itself is deferred to
/// [`IbMarketData::connect`](super::market_data::IbMarketData::connect).
///
/// The default port is `4002` (IB Gateway paper) rather than `4001`
/// (live) — the live port refusal lives in the top-level
/// [`BrokerConfig`](crate::BrokerConfig); this struct stays low-level
/// and does not enforce the live/paper split itself.
#[derive(Debug, Clone)]
pub struct IbMarketDataConfig {
    /// TWS / IB Gateway host.
    pub host: String,
    /// TWS / IB Gateway port (`4002` paper, `4001` live).
    pub port: u16,
    /// API client id.
    pub client_id: i32,
    /// Pacing governor configuration (BR-19).
    pub pacing: PacingConfig,
    /// Default exchange used when building `Contract`s from a raw
    /// [`SymbolKey`](midas_broker_core::SymbolKey) without a prior
    /// `resolve_contract` round-trip.
    pub default_exchange: String,
}

impl Default for IbMarketDataConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 4002,
            client_id: 100,
            pacing: PacingConfig::default(),
            default_exchange: "SMART".to_string(),
        }
    }
}

impl IbMarketDataConfig {
    /// Convenience constructor for the default-configured paper
    /// endpoint with a caller-supplied client id.
    pub fn paper(client_id: i32) -> Self {
        Self {
            client_id,
            ..Self::default()
        }
    }

    /// The `host:port` string expected by rust-ibapi's `Client::connect`.
    pub fn address(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}
