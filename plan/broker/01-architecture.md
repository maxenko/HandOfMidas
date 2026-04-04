# `midas-broker` Architecture Plan

> v1 Design Document — March 2026
> Crate: `midas-broker` | Runtime: tokio shared with iced | Persistence: SQLite via rusqlite

---

## Table of Contents

- [1. Crate Structure](#1-crate-structure)
- [2. Core Design Principles](#2-core-design-principles)
- [3. Dependency on rust-ibapi](#3-dependency-on-rust-ibapi)
- [4. State Model](#4-state-model)
- [5. Threading / Async Model](#5-threading--async-model)
- [6. Connection Lifecycle](#6-connection-lifecycle)
- [7. Error Handling Strategy](#7-error-handling-strategy)
- [8. Configuration](#8-configuration)
- [9. Crate Boundaries](#9-crate-boundaries)

---

## 1. Crate Structure

```
crates/
  midas-broker/
    Cargo.toml
    src/
      lib.rs                  # Re-exports, BrokerEngine entry point
      engine.rs               # BrokerEngine — owns connection, runs as tokio task
      connection.rs           # IB connection wrapper, reconnect logic
      orders/
        mod.rs
        managed_order.rs      # ManagedOrder — our enriched order type
        builder.rs            # Ergonomic order construction over ibapi::OrderBuilder
        activation.rs         # Activate/deactivate logic (cancel+cache / re-place)
        reconcile.rs          # Reconcile local state with IB on reconnect
      state/
        mod.rs
        order_store.rs        # In-memory order state (HashMap<OrderId, ManagedOrder>)
        position_store.rs     # Positions cache (synced from IB)
        account_store.rs      # Account values cache
      persist/
        mod.rs
        db.rs                 # SQLite connection, migrations, schema
        order_repo.rs         # CRUD for persisted orders
        event_log.rs          # Append-only order event log
      events.rs               # BrokerEvent enum (broadcast to UI)
      commands.rs             # BrokerCommand enum (UI sends to engine)
      types.rs                # ManagedOrderId, OrderTag, shared newtypes
      config.rs               # BrokerConfig (connection, defaults)
      error.rs                # BrokerError enum
```

### Module Responsibilities

| Module | Purpose |
|---|---|
| `engine` | Top-level tokio task. Owns the `ibapi::Client`, processes commands from UI, emits events. Single run loop. |
| `connection` | Wraps `ibapi::Client` lifecycle: connect, health-check, reconnect with backoff. Surfaces connection state as a `tokio::sync::watch` value. |
| `orders::managed_order` | `ManagedOrder` struct: wraps an ibapi `Order` + `Contract` with our metadata (local ID, activation state, tags, user notes, timestamps). |
| `orders::builder` | Thin ergonomic layer over `ibapi::OrderBuilder`. Pre-populates defaults from config. Returns `ManagedOrder`. |
| `orders::activation` | Implements deactivate (cancel at IB, mark `Deactivated` locally, persist params) and activate (re-place from cached params). IB has no native deactivate API — this is purely local bookkeeping. |
| `orders::reconcile` | On reconnect: request open orders from IB, diff against local state, resolve discrepancies (fills that happened while disconnected, orders cancelled externally). |
| `state::order_store` | `HashMap<ManagedOrderId, ManagedOrder>` in memory. Single owner inside `BrokerEngine`. Never shared across threads — accessed only within the engine task. |
| `state::position_store` | Positions received from IB account updates. Read-only snapshot exposed to UI via events. |
| `state::account_store` | Net liquidation, buying power, margin, cash. Updated from IB account summary callbacks. |
| `persist::db` | Opens SQLite file, runs migrations on startup, provides `rusqlite::Connection`. All DB access happens on a dedicated `tokio::task::spawn_blocking` call to avoid blocking the async runtime. |
| `persist::order_repo` | Insert, update, query `ManagedOrder` rows. Stores the full order specification so deactivated orders survive process restarts. |
| `persist::event_log` | Append-only table of every order state transition with timestamps. Audit trail that IB only keeps for the current day. |
| `events` | `BrokerEvent` enum — everything the engine tells the UI. Sent over `tokio::broadcast`. |
| `commands` | `BrokerCommand` enum — everything the UI tells the engine. Sent over `tokio::mpsc`. |
| `types` | `ManagedOrderId` (local UUID), `OrderTag` (user labels), `ActivationState`, `ConnectionState`. |
| `config` | `BrokerConfig`: connection host/port/client_id, default TIF, default order type, account ID, reconnect parameters. Deserialized from TOML. |
| `error` | `BrokerError` enum with variants for connection, order, persistence, and protocol errors. |

### Public API Surface

The crate exposes a deliberately narrow API. Consumers (the iced UI crate) interact with exactly three things:

```rust
// 1. Start the engine — returns channel handles
pub fn start_broker_engine(
    config: BrokerConfig,
    runtime: tokio::runtime::Handle,
) -> BrokerHandle;

// 2. Send commands to the engine
pub struct BrokerHandle {
    pub commands: tokio::sync::mpsc::Sender<BrokerCommand>,
    pub market_events: tokio::sync::broadcast::Receiver<BrokerEvent>,
    pub order_events: tokio::sync::broadcast::Receiver<BrokerEvent>,
    pub connection_state: tokio::sync::watch::Receiver<ConnectionState>,
}

// 3. Types needed to construct commands and interpret events
pub use commands::BrokerCommand;
pub use events::BrokerEvent;
pub use types::*;
pub use config::BrokerConfig;
pub use error::BrokerError;
pub use orders::managed_order::ManagedOrder;
```

Nothing from `ibapi` leaks through the public API. The UI crate never imports `ibapi` directly.

---

## Non-Goals (v1)

The following are explicitly **out of scope** for v1. This prevents scope creep during implementation.

| Non-Goal | Rationale |
|---|---|
| Multi-broker abstraction (`BrokerTrait`) | v1 targets IB exclusively. A generic broker trait would over-abstract before we know other brokers' semantics. |
| Strategy / algo execution engine | `midas-broker` is a connectivity + order management layer, not a strategy runner. Strategy logic belongs in `midas-app` or a future `midas-strategy` crate. |
| Risk management (position limits, max loss, kill switch) | Important but orthogonal. Will be a separate subsystem that consumes `BrokerEvent`s and issues `BrokerCommand`s. |
| Backtesting | The broker crate talks to live IB. A backtest engine would mock IB's behavior — a fundamentally different concern, likely in `midas-backtest`. |
| Multi-account support | v1 supports a single account. Multi-account adds allocation logic and per-account state tracking. |
| Multiple simultaneous IB connections | v1 has one `ibapi::Client` to one Gateway. |

---

## Data Sensitivity & Threat Model

This is a **single-user desktop application**. The threat model for v1:

| Aspect | v1 Stance |
|---|---|
| Access control | Same-user OS context. Any process running as the current user can read the SQLite file. Acceptable for a personal trading tool. |
| Encryption at rest | None. SQLite files are plaintext. Do not place the data directory on shared or cloud-synced drives. SQLCipher is a v2 consideration if needed. |
| Audit trail retention | Indefinite. `order_audit` rows are append-only and never deleted. Over years of trading this table will grow but remain small (tens of thousands of rows). |
| Network security | All IB communication is over localhost TCP to TWS/Gateway. No data leaves the machine except to IB's servers via their encrypted connection. |
| Secrets | No API keys or passwords are stored. Authentication is handled by TWS/Gateway's own login flow. |

---

## 2. Core Design Principles

### Single Process, Shared Runtime

The entire application — iced GUI, broker engine, market data feeds — runs in one OS process sharing a single tokio runtime. iced 0.13+ runs on tokio natively; we reuse that runtime handle rather than spawning a second one.

```
┌─────────────────────────────────────────────────┐
│                   OS Process                     │
│                                                  │
│  ┌──────────────┐         ┌──────────────────┐  │
│  │   iced UI     │◄─events─┤  BrokerEngine    │  │
│  │  (main thread │         │  (tokio task)    │  │
│  │   + tokio)    ├─cmds───►│                  │  │
│  └──────────────┘         └───────┬──────────┘  │
│                                    │             │
│                           ┌────────▼─────────┐  │
│                           │  ibapi::Client    │  │
│                           │  (TCP to Gateway) │  │
│                           └──────────────────┘  │
│                                                  │
│  ┌──────────────┐                                │
│  │  SQLite       │◄── spawn_blocking from engine │
│  │  (file)       │                               │
│  └──────────────┘                                │
└─────────────────────────────────────────────────┘
```

### Channel-Based Communication

All cross-boundary communication uses tokio channels. No shared mutable state between UI and engine.

| Channel | Type | Direction | Purpose |
|---|---|---|---|
| Commands | `tokio::sync::mpsc` | UI --> Engine | Place order, cancel, activate, deactivate, connect, disconnect |
| Market Data Events | `tokio::sync::broadcast` | Engine --> UI(s) | Ticks, bars, depth — lossy (stale data is worthless) |
| Order Events | `tokio::sync::broadcast` | Engine --> UI(s) | Order status changes, fills, rejections — multiple consumers (UI, logger, strategy) |
| Connection State | `tokio::sync::watch` | Engine --> UI | Current `ConnectionState` enum value; UI polls on every frame |

### Why These Channel Types

- **mpsc for commands**: Multiple UI components may send commands (order panel, position panel, hotkeys), but only one engine consumes them. Bounded buffer (capacity 256) provides backpressure.
- **broadcast for market data events**: Market data has multiple subscribers (chart, watchlist, indicators). Broadcast lets each subscriber get every event independently. Bounded (capacity 4096); slow receivers get `Lagged` and skip to current data — stale ticks are worthless.
- **broadcast for order events**: Order status changes, fills, and rejections need multiple consumers (UI, logger, strategy engine). A separate `broadcast` channel with a large buffer (8192) ensures every subscriber gets every event independently. Order events are infrequent compared to market data, so the 8192 buffer provides massive headroom and should practically never lag. If a consumer does receive `Lagged`, it sends a `RequestOrderSnapshot` command to trigger a full state re-sync from SQLite.
- **watch for connection state**: The connection status bar needs only the latest value, never the history. `watch` is zero-allocation for reads and always gives the most recent value.

### No Locks Crossing the UI/Engine Boundary

The engine task owns all mutable broker state. The UI receives immutable snapshots via events. This eliminates deadlock risk and makes the system trivially safe to reason about.

---

## 3. Dependency on rust-ibapi

### Version and Feature Flags

```toml
[dependencies]
ibapi = "2"  # Pin to specific version after Phase 0 evaluation
```

The exact version and feature flags will be determined during Phase 0 evaluation. The plan assumes async usage via `rust-ibapi`'s async `Client` as the primary interface. **If Phase 0 reveals the library is synchronous**, all IB calls will be wrapped in `tokio::task::spawn_blocking` with results bridged back via tokio channels (see 05-implementation-roadmap.md, Risk R7). The sync fallback architecture:

```
Engine select! loop  ──spawn_blocking──>  ibapi::Client (blocking thread)
                     <──oneshot channel──  result / callback
```

This is a known pattern that works well; the engine loop pseudocode in Section 5 remains valid either way.

### What We Use Directly

These `ibapi` types are used inside `midas-broker` but never re-exported:

| ibapi Type | Our Usage |
|---|---|
| `ibapi::Client` (async) | Held by `BrokerEngine`. Single instance per connection. |
| `ibapi::contracts::Contract` | Stored inside `ManagedOrder`. Built via ibapi's builder API. |
| `ibapi::orders::Order` | Stored inside `ManagedOrder`. Built via ibapi's `OrderBuilder`. |
| `ibapi::orders::OrderStatus` | Received from IB; mapped to our `BrokerEvent::OrderStatusChanged`. |
| `ibapi::orders::Execution` | Received from IB; mapped to our `BrokerEvent::Fill`. |
| `ibapi::orders::OrderData` | Received when requesting open orders on reconnect. |
| `ibapi::orders::CommissionReport` | Received after fills; forwarded in `BrokerEvent::Commission`. |
| `ibapi::subscriptions::Subscription` | Async stream from `place_order`, `open_orders`, market data requests. Consumed inside engine task. |
| `ibapi::Error` | Caught and converted to `BrokerError` at the boundary. |
| `ibapi::ConnectionOptions` | Used to configure the client connection. |

### What We Abstract Over

| IB Concept | Our Abstraction | Why |
|---|---|---|
| Order ID management | `ManagedOrderId` (UUID) + internal IB order ID mapping | IB order IDs are sequential integers that reset. We need stable IDs that survive reconnects and restarts. |
| Order deactivation | `ActivationState` enum + cancel/cache/re-place cycle | IB has no deactivate API. We cancel the order, cache all parameters locally in SQLite, and re-place on activation. |
| Contract specification | `ContractSpec` (our simplified enum) | UI doesn't need to know about `conId` resolution. We handle `qualifyContracts` internally and cache the result. |
| Connection state | `ConnectionState` enum with `watch` channel | IB fires error codes (1100/1101/1102/2104/2106) as callbacks. We interpret these into a clean state machine. |
| Order status | `ManagedOrderStatus` enum | Superset of IB's states. Adds `Deactivated`, `ActivationPending`, `PendingLocal` (not yet sent to IB). |
| Reconnection | Automatic with exponential backoff | IB Gateway restarts daily at ~23:45 ET. We handle this transparently. |

### What We Do NOT Abstract (v1)

These pass through with minimal wrapping:

- Order types (`LMT`, `MKT`, `STP`, etc.) — we use ibapi's `OrderBuilder` directly
- Time-in-force values — passed through as-is
- IB algo parameters — passed through as `Vec<TagValue>`
- Bracket order construction — we delegate to ibapi's `BracketOrderBuilder`, wrapping each leg as a `ManagedOrder`
- OCA groups — passed through
- Conditions — passed through

The principle: abstract where IB's model doesn't fit our needs (IDs, activation, connection lifecycle). Pass through where IB's model is already correct (order types, TIF, algos).

---

## 4. State Model

### In-Memory State (owned by BrokerEngine)

```rust
pub struct EngineState {
    // Orders
    orders: HashMap<ManagedOrderId, ManagedOrder>,
    ib_id_map: BiMap<ManagedOrderId, i32>,       // our ID <-> IB orderId
    next_ib_order_id: i32,                        // from IB's nextValidId

    // Positions & Account
    positions: HashMap<PositionKey, Position>,     // (account, conId) -> position
    account: AccountSummary,                       // net liq, buying power, etc.

    // Connection
    connection_state: ConnectionState,
    data_farms_ready: HashSet<String>,             // track 2104/2106 readiness

    // Qualified contract cache
    contract_cache: HashMap<ContractSpec, Contract>, // avoid re-qualifying
}
```

### Persisted State (SQLite)

See 03-data-layer.md §1 for the authoritative SQLite schema. The key tables are: `orders`, `order_audit`, `fills`, `positions`, `account_values`, `contracts`.

### Sync Strategy: Memory <-> SQLite

The flow is always: **IB --> Memory --> SQLite**. Memory is the source of truth during runtime. SQLite is the source of truth on startup.

| Event | Memory Update | SQLite Update |
|---|---|---|
| Order placed | Insert into `orders` + `ib_id_map` | `INSERT managed_orders` + append to `order_events` |
| Status change from IB | Update `orders[id].status` | `UPDATE managed_orders SET status` + append to `order_events` |
| Fill from IB | Update status, filled qty | `UPDATE managed_orders` + append fill event to `order_events` |
| Order deactivated (user) | Cancel at IB, set `activation_state = Deactivated` | `UPDATE managed_orders SET activation_state` + append event |
| Order activated (user) | Re-place at IB, set `activation_state = Active`, update `ib_order_id` | `UPDATE managed_orders` + append event |
| Startup | Load from SQLite into memory | Read-only |
| Reconnect | Reconcile IB state vs memory, update memory | Write any corrections |

**Two-tier DB write policy.** SQLite writes are classified as critical or non-critical:

| Tier | Examples | Behavior |
|---|---|---|
| **Critical** | Order state transitions, fills, audit log entries before IB calls | `spawn_blocking` + **await the JoinHandle**. Errors are propagated. Must complete before the engine proceeds. |
| **Non-critical** | Market data cache, contract cache, position snapshots, metrics | `spawn_blocking` **without** awaiting (fire-and-forget). Errors are logged but do not halt the engine. |

The engine loop must never block on disk I/O for non-critical writes. Critical writes (fills, state transitions) justify a brief await because data loss in these paths is financially significant. The in-memory state remains authoritative during runtime, but critical DB writes ensure the audit trail and order state survive crashes.

**Flush on shutdown:** Before the engine exits, all outstanding `spawn_blocking` handles are joined to ensure SQLite is fully written. See Phase 6 graceful shutdown.

**Startup load is synchronous.** Before the engine enters its main loop, it loads all `managed_orders` where `activation_state != 'Completed'` into memory. This is a blocking read, acceptable because it happens once before the UI is interactive.

---

## 5. Threading / Async Model

### Task Layout

```
tokio runtime (shared with iced)
│
├── BrokerEngine::run()          [spawned task]
│   ├── select! loop:
│   │   ├── recv from command_rx  (mpsc::Receiver<BrokerCommand>)
│   │   ├── recv from ib_orders   (ibapi Subscription stream — order updates)
│   │   ├── recv from ib_executions (ibapi Subscription stream — executions)
│   │   └── tick from interval    (periodic: heartbeat, reconnect check)
│   │
│   └── spawns:
│       ├── spawn_blocking(db_write)   [per-event, fire-and-forget]
│       └── spawn(reconcile_task)      [on reconnect, one-shot]
│
└── iced application                   [main task]
    ├── sends BrokerCommand via command_tx
    ├── receives BrokerEvent via event_rx (broadcast)
    └── reads ConnectionState via watch_rx
```

### The Engine Run Loop (Pseudocode)

```rust
async fn run(mut self) {
    let mut heartbeat = tokio::time::interval(Duration::from_secs(30));

    loop {
        tokio::select! {
            // 1. Commands from UI
            Some(cmd) = self.command_rx.recv() => {
                self.handle_command(cmd).await;
            }

            // 2. Order updates from IB (when connected)
            Some(update) = self.recv_order_update() => {
                self.handle_order_update(update).await;
            }

            // 3. Periodic maintenance
            _ = heartbeat.tick() => {
                self.check_connection_health().await;
            }
        }
    }
}
```

### Why Not a Separate Thread for IB?

The ibapi async `Client` is already designed for tokio. It manages its own TCP socket internally and delivers results via `Subscription` streams that implement `futures::Stream`. There is no need for a dedicated thread — everything runs as cooperating async tasks on the shared runtime.

### SQLite and spawn_blocking

rusqlite is synchronous. Every database call is wrapped in `tokio::task::spawn_blocking`:

```rust
// CRITICAL write — state transition before IB call. Awaited.
async fn persist_order_status(&self, id: ManagedOrderId, status: &str) -> Result<(), BrokerError> {
    let db = self.db.clone(); // Arc<Mutex<rusqlite::Connection>>
    let id = id.clone();
    let status = status.to_string();
    tokio::task::spawn_blocking(move || {
        let conn = db.lock().expect("db lock poisoned");
        order_repo::update_status(&conn, &id, &status)
    })
    .await
    .map_err(|e| BrokerError::Internal(e.to_string()))?
}

// NON-CRITICAL write — binary candle file cache. Fire and forget.
// Historical bar data uses binary .candles files (see tech-stack-rust-a.md),
// not SQLite. This is still a fire-and-forget write on a blocking thread
// because the I/O is synchronous.
fn persist_bar_cache(&self, symbol: &str, bars: &[OhlcvBar]) {
    let cache_dir = self.cache_dir.clone();
    let symbol = symbol.to_string();
    let bars = bars.to_vec();
    tokio::task::spawn_blocking(move || {
        candle_io::write_bars(&cache_dir, &symbol, &bars)
            .unwrap_or_else(|e| tracing::error!("Candle cache write failed: {e}"));
    });
    // Note: we do NOT .await the JoinHandle — fire and forget
}
```

The `Mutex` here is `std::sync::Mutex`, not `tokio::sync::Mutex`, because the lock is only ever held inside `spawn_blocking` tasks (never across await points).

### Channel Capacity Rationale

| Channel | Capacity | Reasoning |
|---|---|---|
| `mpsc` (commands) | 256 | User actions are bursty but bounded. If the engine falls 256 commands behind, something is catastrophically wrong — backpressure is appropriate. |
| `broadcast` (market data) | 4096 | ~2000 ticks/sec peak across 20 symbols. 4096 provides ~2 seconds of buffer. Slow receivers get `Lagged` and skip to current — acceptable for market data. |
| `broadcast` (order events) | 8192 | Order events are infrequent (tens per second at peak) but need multiple consumers (UI, logger, strategy). 8192 provides massive headroom — practically never lags. If `Lagged`, consumer sends `RequestOrderSnapshot` for full state re-sync from SQLite. |
| `watch` (connection) | 1 (inherent) | Only the latest connection state matters. |

---

## 6. Connection Lifecycle

### State Machine

```
                    ┌─────────────┐
          ┌────────►│ Disconnected │◄────────────┐
          │         └──────┬──────┘              │
          │                │ connect cmd         │ max retries
          │                ▼                     │ exceeded
          │         ┌─────────────┐              │
          │         │ Connecting   ├─────────────┘
          │         └──────┬──────┘
          │                │ TCP established +
          │                │ nextValidId received
          │                ▼
          │         ┌─────────────┐
          │    ┌───►│ Connected    │◄──────────┐
          │    │    └──────┬──────┘            │
          │    │           │ 2104/2106         │ 1102 (data ok)
          │    │           ▼                   │
          │    │    ┌─────────────┐            │
          │    │    │ Ready        ├────────────┤
          │    │    └──────┬──────┘            │
          │    │           │ 1100/1101         │
          │    │           ▼                   │
          │    │    ┌───────────────┐          │
          │    └────┤ Reconnecting  ├──────────┘
          │         └───────┬───────┘
          │                 │ connection lost entirely
          │                 │
          └─────────────────┘
```

### Connection States

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Ready,
    Reconnecting { attempt: u32 },
}
```

### IB Error Code Interpretation

| IB Code | Meaning | Our Reaction |
|---|---|---|
| 1100 | Connectivity between IB client and TWS lost | Transition to `Reconnecting`. Pause all outbound requests. |
| 1101 | Connectivity restored, market data lost | Transition to `Connected`. Re-subscribe market data. Run reconciliation. Wait for 2104 before `Ready`. |
| 1102 | Connectivity restored, market data maintained | Transition to `Ready` directly. Run reconciliation. |
| 2104 | Market data farm connection OK | Mark data farm ready. If all expected farms ready, transition to `Ready`. |
| 2106 | Historical data farm connection OK | Mark historical farm ready. |
| 2108 | Market data farm inactive | Mark farm not ready. Stay in `Connected`. |

### Daily Restart Handling

IB Gateway restarts daily at approximately 23:45 ET. This causes a clean TCP disconnect.

1. Engine detects disconnect (TCP read returns EOF or error).
2. Transition to `Reconnecting { attempt: 0 }`.
3. Exponential backoff: 2s, 4s, 8s, 16s, 30s, 30s, 30s, ... (capped at 30s).
4. Max retries: 60 (covers a ~30-minute outage window).
5. On successful reconnect: run reconciliation, re-subscribe market data, transition through `Connected` to `Ready`.

### Reconnect Reconciliation

On every reconnect:

1. Request all open orders from IB (`reqOpenOrders`).
2. Diff IB's open orders against our in-memory `orders` map.
3. For each order we have locally that IB does not:
   - If we expected it to be active: it was filled or cancelled while disconnected. Request execution reports.
   - Update local state accordingly.
4. For each order IB has that we do not recognize:
   - Placed manually in TWS or by another client. Log and optionally import.
5. Request execution reports for the current day to catch missed fills.
6. Persist all state corrections to SQLite.

### Sunday Authentication

IB requires manual 2FA re-authentication after the Saturday night weekly reset. `midas-broker` cannot automate this. On Sunday, the engine will exhaust its reconnect retries and settle in `Disconnected`. The UI should show: "IB Gateway requires manual login. Please authenticate in IB Gateway and click Connect."

---

## 7. Error Handling Strategy

### Error Enum

```rust
#[derive(Debug, thiserror::Error)]
pub enum BrokerError {
    #[error("Not connected to IB Gateway")]
    NotConnected,

    #[error("Connection failed: {0}")]
    ConnectionFailed(String),

    #[error("Connection lost: {0}")]
    ConnectionLost(String),

    #[error("Order rejected by IB (code {code}): {message}")]
    OrderRejected { code: i32, message: String },

    #[error("Order not found: {0}")]
    OrderNotFound(ManagedOrderId),

    #[error("Invalid order state transition: {from} -> {to}")]
    InvalidStateTransition { from: String, to: String },

    #[error("Order validation failed: {0}")]
    ValidationFailed(String),

    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("IB API error (code {code}): {message}")]
    IbApi { code: i32, message: String },

    #[error("Internal error: {0}")]
    Internal(String),
}
```

### How IB Errors Propagate to the UI

```
IB error (code + message)
  │
  ├─ Connection error (1100, 1101, 1102, 2103-2108)?
  │   └─ Update ConnectionState via watch channel
  │
  ├─ Order error (103, 104, 110, 161, 200, 201, 202, 203, 399)?
  │   └─ BrokerEvent::OrderError { order_id, error }
  │      Update ManagedOrder.status in memory + SQLite
  │
  ├─ Warning (code >= 2000, not connection-related)?
  │   └─ BrokerEvent::Warning { code, message }
  │
  └─ Unknown?
      └─ Log at warn level, BrokerEvent::Warning
```

### Panics

The engine catches no panics. If the engine task panics, the iced app detects it (the watch channel closes, command sends fail) and shows a fatal error screen. A broker engine panic indicates a logic bug that must not be silently swallowed.

---

## 8. Configuration

### BrokerConfig

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrokerConfig {
    pub connection: ConnectionConfig,
    pub defaults: OrderDefaults,
    pub persistence: PersistenceConfig,
    pub reconnect: ReconnectConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionConfig {
    pub host: String,          // Default: "127.0.0.1"
    pub port: u16,             // 4001=live, 4002=paper
    pub client_id: i32,        // 0-31. 0 sees manual TWS orders.
    pub account_id: Option<String>,
    pub allow_live: bool,      // Default: false. Must be explicitly set to true
                               // to connect to port 4001 (live trading).
                               // If false and port == 4001, engine refuses to connect.
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderDefaults {
    pub tif: String,           // Default: "DAY"
    pub order_type: String,    // Default: "LMT"
    pub outside_rth: bool,     // Default: false
    pub algo_strategy: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistenceConfig {
    pub db_path: PathBuf,      // Default: {data_dir}/midas-broker.db
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReconnectConfig {
    pub initial_delay_secs: u64,  // Default: 2
    pub max_delay_secs: u64,      // Default: 30
    pub max_retries: u32,         // Default: 60
}
```

### TOML Example

```toml
[connection]
host = "127.0.0.1"
port = 4002
client_id = 1

[defaults]
tif = "DAY"
order_type = "LMT"
outside_rth = false

[persistence]
db_path = "data/midas-broker.db"

[reconnect]
initial_delay_secs = 2
max_delay_secs = 30
max_retries = 60
```

---

## 9. Crate Boundaries

### What Goes in `midas-broker`

Everything related to **order lifecycle, account state, and IB connection management**:

- IB Gateway connection, reconnection, health monitoring
- Order placement, modification, cancellation
- Order deactivation/activation (local bookkeeping)
- Bracket and OCA order construction
- Order state tracking and reconciliation
- Position and account summary caching
- SQLite persistence of order state and event log
- Qualified contract caching
- `BrokerCommand` / `BrokerEvent` channel protocol

### What Stays in the iced UI Crate

Everything related to **presentation, user interaction, and layout**:

- Order entry form, order blotter, position panel, account summary display
- Connection status indicator
- Confirmation dialogs
- Notification toasts for fills, errors, warnings
- Keyboard shortcuts and hotkeys
- Visual formatting of prices, quantities, P&L colors

### What Goes in Other Crates

| Concern | Crate | Why Separate |
|---|---|---|
| Market data streaming (L1, L2, ticks) | `midas-feed` | Independent IB API surface. Different subscription lifecycles. |
| Historical data fetching | `midas-data` | Consumed by charting engine, not broker. Different caching strategy. |
| Chart rendering | `midas-render` | Zero overlap with order management. |
| Indicators | `midas-indicators` | Pure computation, no I/O. |
| Shared types, events, IDs | `midas-core` | Types shared between broker, feed, and app. |

### Dependency Direction

```
midas-app (binary)
├── midas-broker      (order management)
│   └── midas-core    (shared types)
├── midas-feed        (market data)
│   └── midas-core
├── midas-render      (GPU)
│   └── midas-core
├── midas-indicators  (computation)
│   └── midas-core
└── midas-data        (storage)
    └── midas-core
```

`midas-broker` depends only on `midas-core` and external crates (`ibapi`, `rusqlite`, `tokio`, `serde`, `thiserror`, `tracing`, `uuid`). It has no dependency on any UI, rendering, or market data crate.

### The ContractSpec Boundary

`midas-core` defines `ContractSpec` — a simplified, serializable instrument identifier:

```rust
#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub enum ContractSpec {
    Stock { symbol: String, exchange: String, currency: String },
    // OrderedFloat<f64> implements Hash + Eq, which bare f64 does not.
    // Requires: ordered-float = "4" in midas-core's Cargo.toml
    Option { symbol: String, expiry: String, strike: OrderedFloat<f64>, right: OptionRight, exchange: String },
    Future { symbol: String, expiry: String, exchange: String },
    Forex { pair: String },
}
```

Both `midas-broker` and `midas-feed` convert this to/from `ibapi::contracts::Contract` internally. The UI crate only ever sees `ContractSpec`.

---

## Appendix: v1 Scope Boundary

| Feature | v1 | v2 |
|---|---|---|
| Order types | LMT, MKT, STP, STP LMT | All IB order types |
| Bracket orders | Single bracket (entry + TP + SL) | Nested brackets, multi-leg |
| OCA groups | Pass-through to IB | Local OCA management |
| Conditional orders | Pass-through to IB | Local condition evaluation |
| IB Algo orders | Adaptive only | Full algo parameter UI |
| Multiple accounts | Single account | Multi-account with allocation |
| Multiple connections | Single Gateway | Multiple Gateways |
| Order templates | Hardcoded defaults | User-defined templates in SQLite |
| Trade analytics | Raw event log | Aggregated P&L, win rate, stats |
