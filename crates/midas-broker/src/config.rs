use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::BrokerError;

/// Selects the market data source for the broker engine.
#[derive(Debug, Clone, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DataSourceConfig {
    /// Real IB connection (not yet implemented).
    #[default]
    Live,
    /// Deterministic test data — per-ticker seeded generation.
    Test,
}

/// Top-level broker configuration, typically loaded from a TOML file.
#[derive(Debug, Clone, Deserialize)]
pub struct BrokerConfig {
    #[serde(default)]
    pub connection: ConnectionConfig,

    #[serde(default)]
    pub order_defaults: OrderDefaults,

    #[serde(default)]
    pub persistence: PersistenceConfig,

    #[serde(default)]
    pub reconnect: ReconnectConfig,

    /// Market data source: "live" (default) or "test".
    #[serde(default)]
    pub data_source: DataSourceConfig,
}

/// TWS / IB Gateway connection parameters.
#[derive(Debug, Clone, Deserialize)]
pub struct ConnectionConfig {
    /// TWS/Gateway hostname.
    #[serde(default = "default_host")]
    pub host: String,

    /// TWS/Gateway port. 4001 = live, 4002 = paper.
    #[serde(default = "default_port")]
    pub port: u16,

    /// Unique client identifier for this connection.
    #[serde(default = "default_client_id")]
    pub client_id: i32,

    /// IB account identifier (e.g. "DU1234567"). None = first account.
    #[serde(default)]
    pub account_id: Option<String>,

    /// Must be set to `true` to allow connecting to a live trading port.
    /// If `false` and `port == 4001`, the engine refuses to connect.
    #[serde(default)]
    pub allow_live: bool,
}

fn default_host() -> String {
    "127.0.0.1".to_string()
}

fn default_port() -> u16 {
    4002
}

fn default_client_id() -> i32 {
    1
}

impl Default for ConnectionConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            client_id: default_client_id(),
            account_id: None,
            allow_live: false,
        }
    }
}

/// Default values applied to every new order unless explicitly overridden.
#[derive(Debug, Clone, Deserialize)]
pub struct OrderDefaults {
    /// Time-in-force string, e.g. "DAY", "GTC".
    #[serde(default = "default_tif")]
    pub tif: String,

    /// IB order type string, e.g. "LMT", "MKT".
    #[serde(default = "default_order_type")]
    pub order_type: String,

    /// Allow order fills outside regular trading hours.
    #[serde(default)]
    pub outside_rth: bool,
}

fn default_tif() -> String {
    "DAY".to_string()
}

fn default_order_type() -> String {
    "LMT".to_string()
}

impl Default for OrderDefaults {
    fn default() -> Self {
        Self {
            tif: default_tif(),
            order_type: default_order_type(),
            outside_rth: false,
        }
    }
}

/// SQLite persistence settings.
#[derive(Debug, Clone, Deserialize)]
pub struct PersistenceConfig {
    /// Path to the SQLite database file.
    #[serde(default = "default_db_path")]
    pub db_path: PathBuf,
}

fn default_db_path() -> PathBuf {
    let data_dir = dirs::data_local_dir().unwrap_or_else(|| PathBuf::from("."));
    data_dir.join("midas").join("broker.db")
}

impl Default for PersistenceConfig {
    fn default() -> Self {
        Self {
            db_path: default_db_path(),
        }
    }
}

/// Exponential-backoff reconnection parameters.
#[derive(Debug, Clone, Deserialize)]
pub struct ReconnectConfig {
    /// Delay (seconds) before the first reconnection attempt.
    #[serde(default = "default_initial_delay")]
    pub initial_delay_secs: u64,

    /// Maximum delay (seconds) between reconnection attempts.
    #[serde(default = "default_max_delay")]
    pub max_delay_secs: u64,

    /// Give up after this many consecutive failures.
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
}

fn default_initial_delay() -> u64 {
    1
}

fn default_max_delay() -> u64 {
    60
}

fn default_max_retries() -> u32 {
    10
}

impl Default for ReconnectConfig {
    fn default() -> Self {
        Self {
            initial_delay_secs: default_initial_delay(),
            max_delay_secs: default_max_delay(),
            max_retries: default_max_retries(),
        }
    }
}

impl Default for BrokerConfig {
    fn default() -> Self {
        Self {
            connection: ConnectionConfig::default(),
            order_defaults: OrderDefaults::default(),
            persistence: PersistenceConfig::default(),
            reconnect: ReconnectConfig::default(),
            data_source: DataSourceConfig::default(),
        }
    }
}

impl BrokerConfig {
    /// Load configuration from a TOML file at `path`.
    pub fn load_from_file(path: &Path) -> Result<Self, BrokerError> {
        let content = std::fs::read_to_string(path).map_err(|e| {
            BrokerError::Config(format!("failed to read config file {}: {e}", path.display()))
        })?;
        let config: BrokerConfig = toml::from_str(&content).map_err(|e| {
            BrokerError::Config(format!(
                "failed to parse config file {}: {e}",
                path.display()
            ))
        })?;
        config.validate()?;
        Ok(config)
    }

    /// Validate configuration invariants.
    ///
    /// **Live trading guard**: if `connection.port == 4001` (the live trading
    /// port) and `connection.allow_live` is `false`, validation fails. This
    /// prevents accidentally connecting to a live account.
    pub fn validate(&self) -> Result<(), BrokerError> {
        if self.connection.port == 4001 && !self.connection.allow_live {
            return Err(BrokerError::Config(
                "port 4001 is the live trading port but allow_live is false; \
                 set allow_live = true in [connection] to confirm live trading"
                    .to_string(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    #[test]
    fn default_config_is_paper() {
        let cfg = BrokerConfig::default();
        assert_eq!(cfg.connection.host, "127.0.0.1");
        assert_eq!(cfg.connection.port, 4002);
        assert_eq!(cfg.connection.client_id, 1);
        assert!(!cfg.connection.allow_live);
        cfg.validate().expect("default config should be valid");
    }

    #[test]
    fn live_port_without_allow_live_fails() {
        let mut cfg = BrokerConfig::default();
        cfg.connection.port = 4001;
        cfg.connection.allow_live = false;
        let err = cfg.validate().unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("allow_live"), "error should mention allow_live: {msg}");
    }

    #[test]
    fn live_port_with_allow_live_succeeds() {
        let mut cfg = BrokerConfig::default();
        cfg.connection.port = 4001;
        cfg.connection.allow_live = true;
        cfg.validate().expect("live port with allow_live should be valid");
    }

    #[test]
    fn load_from_toml_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broker.toml");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(
            f,
            r#"
[connection]
host = "10.0.0.5"
port = 4002
client_id = 7

[order_defaults]
tif = "GTC"

[persistence]
db_path = "/tmp/test.db"

[reconnect]
max_retries = 5
"#
        )
        .unwrap();

        let cfg = BrokerConfig::load_from_file(&path).unwrap();
        assert_eq!(cfg.connection.host, "10.0.0.5");
        assert_eq!(cfg.connection.client_id, 7);
        assert_eq!(cfg.order_defaults.tif, "GTC");
        assert_eq!(cfg.persistence.db_path, PathBuf::from("/tmp/test.db"));
        assert_eq!(cfg.reconnect.max_retries, 5);
    }

    #[test]
    fn load_minimal_toml_uses_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("minimal.toml");
        std::fs::write(&path, "").unwrap();
        let cfg = BrokerConfig::load_from_file(&path).unwrap();
        assert_eq!(cfg.connection.port, 4002);
        assert_eq!(cfg.order_defaults.order_type, "LMT");
    }

    #[test]
    fn load_live_port_toml_without_allow_live_fails() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("live.toml");
        std::fs::write(
            &path,
            r#"
[connection]
port = 4001
"#,
        )
        .unwrap();
        let err = BrokerConfig::load_from_file(&path).unwrap_err();
        assert!(err.to_string().contains("allow_live"));
    }
}
