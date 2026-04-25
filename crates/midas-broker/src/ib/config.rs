//! [`IbMarketDataConfig`] — connection + pacing settings for
//! [`IbMarketData`](super::market_data::IbMarketData) and
//! [`IbOrderClient`](super::order_client::IbOrderClient).

use std::time::Duration;

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
    /// Per-operation upstream deadline for every `client.xxx().await`
    /// call routed through `rust-ibapi`. The router already wraps
    /// provider methods with its own 10 s actor timeout
    /// (`ROUTER_ACTOR_OP_TIMEOUT`), but the provider itself also
    /// needs a hard deadline so a stuck TWS handshake / place_order
    /// doesn't hold the router's handler past its own budget. 10 s
    /// is the default, matching the router.
    pub ib_op_timeout: Duration,
    /// Per-leg deadline applied by `BracketSubmitter::cancel_bracket`
    /// on each individual `OrderClient::cancel_order` await. Kept
    /// shorter than `ib_op_timeout` so a stuck leg doesn't lock out
    /// cancellation of the other two.
    pub cancel_leg_timeout: Duration,
    /// Must be `true` to allow connecting to the live-trading port
    /// (`4001`). Defence-in-depth complement to
    /// [`BrokerConfig::validate`](crate::BrokerConfig::validate) — the
    /// TOML-load path already rejects this misconfig, but this field
    /// catches programmatic construction or post-construction mutation
    /// (e.g. `cfg.port = 4001`) that never flows through `validate()`.
    ///
    /// Defaults to `false`. When constructing from
    /// [`BrokerConfig`](crate::BrokerConfig), copy
    /// `connection.allow_live` over.
    pub allow_live: bool,
}

impl Default for IbMarketDataConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 4002,
            client_id: 100,
            pacing: PacingConfig::default(),
            default_exchange: "SMART".to_string(),
            ib_op_timeout: Duration::from_secs(10),
            cancel_leg_timeout: Duration::from_secs(5),
            allow_live: false,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_ib_op_timeout_is_ten_seconds() {
        let cfg = IbMarketDataConfig::default();
        assert_eq!(cfg.ib_op_timeout, Duration::from_secs(10));
    }

    #[test]
    fn default_cancel_leg_timeout_is_five_seconds() {
        let cfg = IbMarketDataConfig::default();
        assert_eq!(cfg.cancel_leg_timeout, Duration::from_secs(5));
    }

    #[test]
    fn timeouts_are_configurable() {
        let cfg = IbMarketDataConfig {
            ib_op_timeout: Duration::from_millis(250),
            cancel_leg_timeout: Duration::from_millis(100),
            ..IbMarketDataConfig::default()
        };
        assert_eq!(cfg.ib_op_timeout, Duration::from_millis(250));
        assert_eq!(cfg.cancel_leg_timeout, Duration::from_millis(100));
    }
}
