# midas-broker API

Trading engine crate wrapping Interactive Brokers via `rust-ibapi`.

## Documentation Files

| File | Covers |
|---|---|
| [engine.md](engine.md) | BrokerHandle, start_broker_engine, channel architecture |
| [commands-events.md](commands-events.md) | BrokerCommand (17 variants), BrokerEvent (22 variants) |
| [config.md](config.md) | BrokerConfig, DataSourceConfig, live-trading guard |
| [orders.md](orders.md) | LocalOrder, OrderStatus state machine, OrderAction, OrderKind |
| [market-data.md](market-data.md) | MarketDataSource trait, IB string parsers |
| [testdata.md](testdata.md) | TestDataProvider, personalities, generation internals |
| [persistence.md](persistence.md) | BrokerDb, SQLite schema, order_repo functions |

## Quick Start

```rust
use midas_broker::{
    BrokerHandle, BrokerConfig, BrokerCommand, BrokerEvent,
    ConnectionState, OrderStatus, LocalOrder, MarketDataSource,
};
```

## Module Map

```
src/
├── lib.rs            Re-exports
├── engine.rs         BrokerHandle + BrokerEngine
├── commands.rs       BrokerCommand enum
├── events.rs         BrokerEvent enum
├── config.rs         BrokerConfig + DataSourceConfig
├── error.rs          BrokerError (thiserror)
├── connection.rs     ConnectionState (5 states)
├── market_data.rs    MarketDataSource trait
├── ib_strings.rs     parse_bar_size(), duration_to_start()
├── db.rs             BrokerDb (SQLite, WAL mode)
├── orders/
│   ├── types.rs      LocalOrder, OrderAction, OrderKind, TimeInForce
│   └── state.rs      OrderStatus state machine (11 states)
├── persist/
│   └── order_repo.rs OrderRow CRUD, fills, audit
└── testdata/
    ├── mod.rs         TestDataProvider, aggregate_bars
    ├── adapter.rs     impl MarketDataSource for TestDataProvider
    ├── personality.rs StockPersonality, 5 presets, ticker seeding
    └── generate.rs    Daily (GARCH) + intraday (Brownian bridge)
```
