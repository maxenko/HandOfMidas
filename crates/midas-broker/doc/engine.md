# Engine & Channel Architecture

## start_broker_engine

```rust
pub fn start_broker_engine(config: BrokerConfig) -> BrokerHandle
```

Spawns the engine as a tokio task. Returns channel handles immediately.

## BrokerHandle

```rust
pub struct BrokerHandle {
    pub commands: mpsc::Sender<BrokerCommand>,          // send commands (256 capacity)
    pub market_events: broadcast::Sender<BrokerEvent>,  // ticks, bars, depth (4096)
    pub order_events: broadcast::Sender<BrokerEvent>,   // fills, status changes (8192)
    pub connection_state: watch::Receiver<ConnectionState>,
}
```

- **commands** — mpsc(256), backpressure-aware. Send `BrokerCommand` variants.
- **market_events** — broadcast(4096), lossy OK. Subscribe via `.subscribe()`.
- **order_events** — broadcast(8192), lossless. Subscribe via `.subscribe()`.
- **connection_state** — watch channel. `.borrow()` for current state.

## Channel Flow

```
UI / Strategy                    Engine                         IB / TestData
      │                            │                               │
      ├─ BrokerCommand ──────────► │                               │
      │  (mpsc)                    ├─ MarketDataSource.bars() ───► │
      │                            │◄── Vec<OhlcvBar> ────────────┤
      │◄── BrokerEvent::BarClosed  │                               │
      │◄── HistoricalDataComplete  │                               │
      │  (broadcast)               │                               │
```

## ConnectionState

```rust
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected { server_version: i32 },
    Ready,
    Reconnecting { attempt: u32 },
}
```

Predicates: `is_connected()`, `is_ready()`.
