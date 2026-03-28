# Configuration

## BrokerConfig

TOML-loadable. All sections have defaults.

```rust
pub struct BrokerConfig {
    pub connection: ConnectionConfig,
    pub order_defaults: OrderDefaults,
    pub persistence: PersistenceConfig,
    pub reconnect: ReconnectConfig,
    pub data_source: DataSourceConfig,
}
```

### DataSourceConfig

```rust
pub enum DataSourceConfig {
    Live,  // default — real IB connection (not yet implemented)
    Test,  // deterministic test data per ticker
}
```

In TOML:
```toml
data_source = "test"   # or "live" (default)
```

### ConnectionConfig

```toml
[connection]
host = "127.0.0.1"    # default
port = 4002            # 4002=paper (default), 4001=live
client_id = 1          # default
account_id = "DU1234"  # optional
allow_live = false      # must be true for port 4001
```

**Live-trading guard:** port 4001 + `allow_live = false` → config validation fails.

### OrderDefaults

```toml
[order_defaults]
tif = "DAY"            # default
order_type = "LMT"     # default
outside_rth = false     # default
```

### PersistenceConfig

```toml
[persistence]
db_path = "~/.local/share/midas/broker.db"  # platform-specific default
```

### ReconnectConfig

```toml
[reconnect]
initial_delay_secs = 1   # default
max_delay_secs = 60      # default
max_retries = 10          # default
```

## Loading

```rust
// From file
let config = BrokerConfig::load_from_file(Path::new("broker.toml"))?;

// Programmatic with test data
let mut config = BrokerConfig::default();
config.data_source = DataSourceConfig::Test;
```
