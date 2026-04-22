# midas-broker API

Trading engine crate wrapping Interactive Brokers via `rust-ibapi`.

> **Router note.** The market-data streaming pipeline now flows through `midas-market-data::MarketDataRouter` (root workspace). The router holds `Arc<dyn MarketDataSource>` where the backend is either `SimMarketData` (`midas-broker::sim`) or `IbMarketData` (`midas-broker::ib`). Order flow is on a parallel `OrderClient` trait with the same sim/IB split. The legacy `BrokerEngine` + `BrokerCallback` + `BrokerClient` path documented in `engine.md` is still live for bracket-order management and will be retired in a follow-up slice (see `plan/archive/market-data-router/10-slice-9-cleanup.md`). New consumers should go through the router + `OrderClient` traits, not through `BrokerEngine`.

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
