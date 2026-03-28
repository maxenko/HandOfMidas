# midas-broker: Implementation Roadmap

> Crate: `midas-broker` -- IB trading engine for Hand of Midas
> Target runtime: Tokio (shared with iced)
> Primary platform: Windows 11, cross-platform secondary
> Timeline: 8+ weeks from first commit

---

## Table of Contents

- [Architecture Overview](#architecture-overview)
- [Phase 0: Foundation (Week 1)](#phase-0-foundation-week-1)
- [Phase 1: Order Basics (Week 2-3)](#phase-1-order-basics-week-2-3)
- [Phase 2: Market Data (Week 3-4)](#phase-2-market-data-week-3-4)
- [Phase 3: Account & Positions (Week 4-5)](#phase-3-account--positions-week-4-5)
- [Phase 4: Advanced Orders (Week 5-6)](#phase-4-advanced-orders-week-5-6)
- [Phase 5: iced Integration (Week 6-7)](#phase-5-iced-integration-week-6-7)
- [Phase 6: Resilience (Week 7-8)](#phase-6-resilience-week-7-8)
- [Phase 7: Polish (Week 8+)](#phase-7-polish-week-8)
- [Risk Register](#risk-register)
- [Dependencies](#dependencies)
- [Testing Strategy](#testing-strategy)
- [Appendix A: SQLite Schema](#appendix-a-sqlite-schema)
- [Appendix B: State Machine Diagrams](#appendix-b-state-machine-diagrams)
- [Appendix C: Event and Command Catalog](#appendix-c-event-and-command-catalog)

---

## Architecture Overview

### Crate Position in Workspace

```
midas/
├── Cargo.toml                    # Workspace root
├── crates/
│   ├── midas-app/                # iced application shell
│   ├── midas-render/             # GPU chart renderer
│   ├── midas-data/               # Candle storage, SoA buffers
│   ├── midas-feed/               # Market data ingest (Polygon, etc.)
│   ├── midas-indicators/         # Technical indicators
│   ├── midas-core/               # Shared types, events
│   └── midas-broker/             # <<< THIS CRATE — IB trading engine
│       ├── Cargo.toml
│       ├── migrations/           # SQL migration files
│       └── src/
│           ├── lib.rs            # Public API surface
│           ├── engine.rs         # BrokerEngine — async task coordinator
│           ├── connection.rs     # IB Gateway connection management
│           ├── orders/
│           │   ├── mod.rs
│           │   ├── manager.rs    # OrderManager — CRUD + state machine
│           │   ├── types.rs      # LocalOrder, OrderStatus, OrderKind
│           │   ├── state.rs      # State machine transitions
│           │   ├── bracket.rs    # Bracket order logic
│           │   └── group.rs      # OCA groups, bulk operations
│           ├── market_data/
│           │   ├── mod.rs
│           │   ├── subscriptions.rs  # Subscription manager, ref-counting
│           │   ├── cache.rs          # Historical data SQLite cache
│           │   └── rate_limiter.rs   # Pacing rule enforcement
│           ├── account/
│           │   ├── mod.rs
│           │   ├── positions.rs  # Position tracking
│           │   ├── summary.rs    # Account values
│           │   └── pnl.rs        # P&L tracking
│           ├── reconcile.rs      # Startup + reconnect reconciliation
│           ├── watchdog.rs       # Connection watchdog, auto-reconnect
│           ├── db.rs             # SQLite connection pool, migrations
│           ├── events.rs         # BrokerEvent enum
│           ├── commands.rs       # BrokerCommand enum
│           ├── config.rs         # BrokerConfig (TOML-driven)
│           └── error.rs          # BrokerError (thiserror)
│
├── data/
│   └── broker.db                 # SQLite database (gitignored)
└── tests/
    └── broker/                   # Integration tests
        ├── order_lifecycle.rs
        ├── market_data.rs
        └── reconnection.rs
```

### Communication Model

```
                    ┌─────────────────────────┐
                    │      iced UI Layer       │
                    │   (midas-app crate)      │
                    └─────┬───────────┬────────┘
              BrokerCommand│           │BrokerEvent
              (mpsc tx)    │           │(broadcast rx)
                           ▼           │
                    ┌──────────────────┴────────┐
                    │      BrokerEngine         │
                    │  (tokio::spawn, runs in   │
                    │   shared Tokio runtime)   │
                    ├───────────────────────────┤
                    │  OrderManager             │
                    │  SubscriptionManager      │
                    │  AccountManager           │
                    │  Watchdog                 │
                    │  Reconciler               │
                    ├───────────┬───────────────┤
                    │  SQLite   │  rust-ibapi    │
                    │ (rusqlite)│  connection    │
                    └───────────┴───────┬───────┘
                                        │ TCP socket
                                        ▼
                              ┌──────────────────┐
                              │   IB Gateway     │
                              │  (port 4001/4002)│
                              └──────────────────┘
```

The `BrokerEngine` runs as a long-lived Tokio task. The iced application sends `BrokerCommand` values through a bounded `mpsc` channel and receives `BrokerEvent` values through a `broadcast` channel. This decouples the UI thread from IB's asynchronous callback model entirely.

---

## Phase 0: Foundation (Week 1)

> **Goal**: Crate compiles, SQLite database creates all tables, can connect to IB Gateway and confirm connectivity.
> **Effort**: 5-7 days
> **Risk gate**: Determine whether rust-ibapi is viable as a dependency or requires forking.

### 0.1 -- Evaluate rust-ibapi

Before writing any application code, validate that rust-ibapi meets our needs.

**Tasks:**

1. Clone the `ibapi` crate repository: `https://github.com/wboayue/rust-ibapi`
2. Read through the source to understand:
   - How it manages the TCP socket and message framing
   - Whether it uses blocking I/O or async (current: blocking with internal threading)
   - Which TWS API server versions it supports
   - Coverage gaps: conditional orders, algo parameters, OCA groups, order conditions
3. Set up IB Gateway in paper trading mode (port 4002)
4. Write a throwaway test binary that:
   - Connects to Gateway
   - Requests contract details for `AAPL` (STK, SMART, USD)
   - Places a limit buy order below market, observes status callbacks
   - Cancels the order
   - Requests 30 days of daily historical bars
   - Subscribes to real-time market data for 10 seconds
5. Document findings in a brief evaluation note

**Decision gate:** Choose one of:

| Option | When to choose |
|---|---|
| **Use as dependency** | Core order placement, market data, and account APIs work. Minor gaps can be worked around at our layer. |
| **Fork and extend** | Fundamental gaps (missing message types, wrong server version, broken async) but overall architecture is sound. |
| **Write our own TWS protocol layer** | Last resort. Only if rust-ibapi's architecture is fundamentally incompatible (unlikely). |

The most probable outcome is "use as dependency" with a thin adapter layer in `connection.rs` that wraps rust-ibapi's blocking calls in `tokio::task::spawn_blocking`.

### 0.2 -- Create `midas-core` Shared Types Crate

Create the `midas-core` crate with types shared across the workspace. This must exist before `midas-broker` so that types conform to the architecture's contract boundary from the start (see 01-architecture.md §9).

```toml
# crates/midas-core/Cargo.toml
[package]
name = "midas-core"
version = "0.1.0"
edition = "2021"

[dependencies]
serde = { version = "1", features = ["derive"] }
chrono = { version = "0.4", features = ["serde"] }
ordered-float = "4"
```

**Types to define:**

```rust
// crates/midas-core/src/lib.rs

/// Simplified, serializable instrument identifier.
/// Both midas-broker and midas-feed convert to/from ibapi::Contract internally.
#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub enum ContractSpec {
    Stock { symbol: String, exchange: String, currency: String },
    // OrderedFloat<f64> implements Hash + Eq, which bare f64 does not.
    // Requires: ordered-float = "4" in midas-core's Cargo.toml
    Option { symbol: String, expiry: String, strike: OrderedFloat<f64>, right: OptionRight, exchange: String },
    Future { symbol: String, expiry: String, exchange: String },
    Forex { pair: String },
}

/// Compact symbol key for lookups (wraps IB contract ID).
#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub struct SymbolKey {
    pub contract_id: i32,
    pub symbol: String,
}

/// Standard timeframes for bar data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Timeframe { S1, S5, S15, S30, M1, M5, M15, M30, H1, H4, D1, W1, MN1 }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OptionRight { Call, Put }
```

**Acceptance**: `cargo build -p midas-core` succeeds. Types are importable from other crates.

### 0.3 -- Crate Structure and Cargo.toml

Create the `midas-broker` crate within the workspace.

```toml
# crates/midas-broker/Cargo.toml
[package]
name = "midas-broker"
version = "0.1.0"
edition = "2021"

[dependencies]
# IB connectivity
ibapi = "2"                         # rust-ibapi — pin to specific version after eval

# Database
rusqlite = { version = "0.32", features = ["bundled"] }

# Async runtime (workspace shared)
tokio = { version = "1", features = ["full"] }

# Serialization
serde = { version = "1", features = ["derive"] }
serde_json = "1"

# Identity
uuid = { version = "1", features = ["v7", "serde"] }

# Time
chrono = { version = "0.4", features = ["serde"] }

# Logging
tracing = "0.1"
tracing-subscriber = "0.3"

# Error handling
thiserror = "2"

# Config
toml = "0.8"
```

Add `midas-broker` to the workspace `Cargo.toml` members list. Verify `cargo build --workspace` succeeds with stub `lib.rs`.

### 0.4 -- SQLite Schema and Migrations

Use `PRAGMA user_version` for migration tracking (see 03-data-layer.md §3 for the full pattern). Migrations are embedded in the binary via `include_str!` and applied on startup. No external migration framework is needed.

**Initial tables** (see 03-data-layer.md §1 for authoritative DDL):

| Table | Purpose |
|---|---|
| `orders` | Canonical order records with all fields needed to re-place at IB |
| `order_audit` | Append-only log of every state transition |
| `fills` | Execution records (one row per partial/full fill) |
| `positions` | Current position snapshot per symbol |
| `account_values` | Latest account summary key-value pairs |
| `contracts` | Cached IB contract details (conId, symbol, exchange, etc.) |

**Tasks:**

1. Create `migrations/001_initial.sql` with all table DDL
2. Implement `db.rs`: open/create SQLite file, run migrations via `PRAGMA user_version` on startup, return connection
3. Write unit test: create in-memory DB, run migrations, verify tables exist

**Acceptance**: `cargo test` passes. In-memory SQLite creates all tables. Schema matches 03-data-layer.md §1.

### 0.5 -- Core Types

Define the foundational types that every subsequent phase depends on.

**`orders/types.rs`:**

```rust
/// A locally-managed order that wraps the IB order concept with
/// application-level lifecycle (draft, inactive, active).
pub struct LocalOrder {
    pub id: Uuid,                    // Our stable ID (survives deactivate/reactivate)
    pub ib_order_id: Option<i32>,    // IB's orderId — None when draft/inactive
    pub ib_perm_id: Option<i64>,     // IB's permanent ID — assigned after first submission
    pub symbol: String,
    pub con_id: i32,                 // IB contract ID
    pub action: OrderAction,         // Buy / Sell
    pub order_type: OrderKind,       // Market, Limit, Stop, StopLimit, TrailingStop, etc.
    pub quantity: f64,
    pub limit_price: Option<f64>,
    pub stop_price: Option<f64>,
    pub trail_amount: Option<f64>,
    pub tif: TimeInForce,
    pub status: OrderStatus,
    pub parent_id: Option<Uuid>,     // For bracket children
    pub oca_group: Option<String>,
    pub tag: Option<String>,         // User-defined tag for grouping
    pub filled_qty: f64,
    pub avg_fill_price: Option<f64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
```

**`orders/state.rs` -- OrderStatus enum:**

> The canonical definition is in 02-order-management.md §1.4 (`OrderState`).
> This mirrors it. Stored as TEXT in SQLite via Display/FromStr (see 03-data-layer.md §2).

```rust
pub enum OrderStatus {
    Draft,           // Created locally, never sent to IB
    Inactive,        // Locally parked (NOT same as IB "Inactive" — see below)
    PendingSubmit,   // Sent to IB, awaiting confirmation
    Submitted,       // Working at the exchange
    PreSubmitted,    // Simulated order accepted, not yet triggered (e.g., stop)
    PartiallyFilled, // Some shares filled
    Filled,          // Completely filled
    PendingCancel,   // Cancel request sent
    Cancelled,       // Confirmed cancelled at IB (terminal — not same as Inactive)
    Rejected,        // IB rejected the order (terminal — IB calls this "Inactive" — DO NOT CONFUSE)
    Error,           // Local/internal error (non-terminal — user can fix and retry)
}
```

**`events.rs` -- BrokerEvent:**

```rust
pub enum BrokerEvent {
    // Connection
    Connected,
    Disconnected { reason: String },
    Reconnecting { attempt: u32 },

    // Orders
    OrderCreated { order_id: Uuid },
    OrderStatusChanged { order_id: Uuid, old: OrderStatus, new: OrderStatus },
    OrderFilled { order_id: Uuid, fill: FillInfo },
    OrderRejected { order_id: Uuid, reason: String },
    OrderError { order_id: Uuid, code: i32, message: String },

    // Market data
    Tick { symbol: String, tick: TickData },
    Bar { symbol: String, bar: BarData },
    HistoricalDataComplete { request_id: Uuid, symbol: String },

    // Account
    PositionUpdate { position: Position },
    AccountUpdate { key: String, value: String, currency: String },
    PnlUpdate { daily_pnl: f64, unrealized_pnl: f64, realized_pnl: f64 },

    // System
    Error { code: i32, message: String },
    DataFarmStatus { farm: String, ok: bool },
}
```

> **Important:** Define the COMPLETE `BrokerEvent` enum with all ~25 variants from 04-market-data-and-events.md section 3.1 (with placeholder handler bodies). This prevents merge conflicts when Phases 1 and 2 extend the enum in parallel. The stub shown above is illustrative; the actual implementation must include every variant (connection, order, market data L1/bars/tick-by-tick/depth, historical data, account/position/PnL, subscription management, and system events) from the start.

**`commands.rs` -- BrokerCommand:**

```rust
pub enum BrokerCommand {
    // Orders
    CreateOrder(LocalOrder),
    ActivateOrder { order_id: Uuid },
    DeactivateOrder { order_id: Uuid },
    CancelOrder { order_id: Uuid },
    ModifyOrder { order_id: Uuid, new_price: Option<f64>, new_qty: Option<f64> },

    // Brackets
    CreateBracketOrder { entry: LocalOrder, take_profit: f64, stop_loss: f64 },

    // Market data
    SubscribeMarketData { symbol: String, con_id: i32 },
    UnsubscribeMarketData { symbol: String },
    RequestHistoricalData { symbol: String, con_id: i32, duration: String, bar_size: String },

    // Account
    RequestPositions,
    RequestAccountSummary,

    // System
    Reconnect,
    Shutdown,

    // Re-sync (sent when order events broadcast channel returns Lagged)
    RequestOrderSnapshot,
}
```

**`error.rs`:**

```rust
#[derive(Debug, thiserror::Error)]
pub enum BrokerError {
    #[error("IB connection error: {0}")]
    Connection(String),

    #[error("IB API error (code {code}): {message}")]
    IbApi { code: i32, message: String },

    #[error("Order not found: {0}")]
    OrderNotFound(Uuid),

    #[error("Invalid state transition: {from:?} -> {to:?}")]
    InvalidTransition { from: OrderStatus, to: OrderStatus },

    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Rate limit exceeded: {0}")]
    RateLimit(String),

    #[error("Configuration error: {0}")]
    Config(String),
}
```

### 0.5 -- Proof of Connectivity

Write a minimal `BrokerEngine` skeleton that:

1. Opens (or creates) the SQLite database and runs migrations
2. Connects to IB Gateway via rust-ibapi (wrapped in `spawn_blocking` if blocking)
3. Logs the managed account ID
4. Disconnects cleanly

This does not yet process commands or emit events -- it just proves the plumbing works.

**Acceptance criteria:**

- `cargo build -p midas-broker` succeeds with zero warnings
- `cargo test -p midas-broker` passes (in-memory DB, type construction, state machine validation)
- Running the connectivity smoke test with IB Gateway on port 4002 prints the paper account ID
- SQLite file at `data/broker.db` contains all tables after first run

### 0.6 -- Async Adapter for rust-ibapi

Build the async wrapper layer that the engine's `select!` loop depends on. All ibapi calls are routed through this adapter.

If rust-ibapi provides a native async `Client` (likely per Phase 0.1 evaluation), wrap it in a thin adapter that normalizes the API. If the library is synchronous, use the `spawn_blocking` + channel bridge pattern from 01-architecture.md section 3.

**Acceptance criteria:**

- Can connect to IB Gateway via the async adapter
- Can place and cancel a test order, receiving status callbacks as async stream items
- Can subscribe to market data and receive ticks as async stream items
- All blocking calls (if any) are confined to `spawn_blocking` — the engine's `select!` loop never blocks

---

## Phase 1: Order Basics (Week 2-3)

> **Goal**: Full order lifecycle -- create, save, activate (send to IB), track status, cancel, deactivate, re-activate -- with audit logging.
> **Effort**: 8-10 days
> **Depends on**: Phase 0 complete

### 1.1 -- Order Persistence Layer

Implement CRUD operations in `orders/manager.rs` backed by SQLite.

**Functions:**

```rust
impl OrderManager {
    pub fn create_order(&self, order: LocalOrder) -> Result<Uuid, BrokerError>;
    pub fn get_order(&self, id: Uuid) -> Result<LocalOrder, BrokerError>;
    pub fn get_orders_by_status(&self, status: &[OrderStatus]) -> Result<Vec<LocalOrder>, BrokerError>;
    pub fn get_orders_by_tag(&self, tag: &str) -> Result<Vec<LocalOrder>, BrokerError>;
    pub fn update_order(&self, order: &LocalOrder) -> Result<(), BrokerError>;
    pub fn save_fill(&self, order_id: Uuid, fill: &FillInfo) -> Result<(), BrokerError>;
    fn write_audit(&self, order_id: Uuid, old: OrderStatus, new: OrderStatus, detail: &str) -> Result<(), BrokerError>;
}
```

Every call to `update_order` that changes `status` must also write to `order_audit`. This is enforced at the manager level, not left to callers.

**Tests:**

- Create order -> verify in DB
- Update order status -> verify audit row written
- Query by status returns correct subset
- Query by tag returns correct subset

### 1.2 -- Order State Machine

Implement the state machine in `orders/state.rs` with explicit transition validation.

```
                 ┌──────────┐
                 │  Draft    │
                 └────┬─────┘
                      │ activate()
                      ▼
              ┌───────────────┐
       ┌──────│ PendingSubmit  │
       │      └───────┬───────┘
       │              │ IB confirms
       │              ▼
       │      ┌───────────────┐
       │      │  Submitted /   │◄──── modify()
       │      │ PreSubmitted   │
       │      └───┬───────┬───┘
       │          │       │
       │   partial fill   full fill
       │          │       │
       │          ▼       ▼
       │  ┌──────────┐ ┌────────┐
       │  │ Partially │ │ Filled │ (terminal)
       │  │  Filled   │ └────────┘
       │  └──────────┘
       │
       │  cancel() / deactivate()
       │          │
       │          ▼
       │  ┌───────────────┐
       │  │ PendingCancel  │
       │  └───────┬───────┘
       │          │ IB confirms
       │          ▼
       │  ┌───────────────┐     ┌──────────┐
       └──│  Cancelled     │     │ Inactive  │
          └───────────────┘     └─────┬────┘
                (terminal)            │ activate()
                                      │ (creates new IB order)
                                      └──► PendingSubmit
```

Key distinction: **Cancelled** is terminal (the user explicitly cancelled and does not want this order). **Inactive** means the order was deactivated -- cancelled at IB, but preserved locally for re-activation. When re-activated, we cancel the old IB order (if still live), assign a new `ib_order_id`, and re-submit.

```rust
impl OrderStatus {
    /// Returns Ok(new_status) if the transition is valid, Err otherwise.
    pub fn transition(&self, event: StatusEvent) -> Result<OrderStatus, BrokerError>;

    /// Returns true if this is a terminal state (no further transitions).
    pub fn is_terminal(&self) -> bool;

    /// Returns true if the order is live at IB.
    pub fn is_active_at_ib(&self) -> bool;
}
```

**Tests:**

- Valid transitions all succeed
- Invalid transitions return `BrokerError::InvalidTransition`
- Terminal state check is correct
- Exhaustive test of all (state, event) pairs

### 1.3 -- IB Order Submission

Wire the `OrderManager` to rust-ibapi for actual order placement.

**Activate flow:**

1. Caller sends `BrokerCommand::ActivateOrder { order_id }`
2. Engine loads `LocalOrder` from DB
3. Validate state is `Draft` or `Inactive`
4. Transition to `PendingSubmit`, write audit
5. Request next valid order ID from IB
6. Convert `LocalOrder` -> ibapi `Order` struct
7. Call `client.place_order(ib_order_id, contract, order)` (via `spawn_blocking`)
8. Store assigned `ib_order_id` on the `LocalOrder`
9. Emit `BrokerEvent::OrderStatusChanged`

**Cancel flow:**

1. Caller sends `BrokerCommand::CancelOrder { order_id }`
2. Load order, validate state is cancellable
3. Transition to `PendingCancel`, write audit
4. Call `client.cancel_order(ib_order_id)` (via `spawn_blocking`)
5. Wait for IB status callback confirming cancellation
6. Transition to `Cancelled`, write audit
7. Emit `BrokerEvent::OrderStatusChanged`

**Deactivate flow:**

1. Same as cancel, but transition to `Inactive` instead of `Cancelled`
2. The `LocalOrder` retains all parameters for future re-activation
3. `ib_order_id` is cleared (will get a new one on re-activate)

**Modify flow:**

1. Caller sends `BrokerCommand::ModifyOrder { order_id, new_price, new_qty }`
2. Load order, validate state is `Submitted` or `PreSubmitted`
3. Update local fields
4. Call `client.place_order(same_ib_order_id, contract, modified_order)`
5. Write audit with old/new values
6. Emit event

### 1.4 -- Status Callbacks from IB

Set up a listener loop that processes IB's order status callbacks.

rust-ibapi likely delivers these via a callback mechanism or a channel. The engine must:

1. Map `ib_order_id` back to our `Uuid` (maintained in an in-memory `HashMap<i32, Uuid>`)
2. Map IB's status string to our `OrderStatus` enum
3. Validate the transition
4. Update DB
5. Write audit
6. Emit `BrokerEvent`

**IB status string mapping:**

| IB Status | Our OrderStatus |
|---|---|
| `"ApiPending"` | PendingSubmit |
| `"PendingSubmit"` | PendingSubmit |
| `"PreSubmitted"` | PreSubmitted |
| `"Submitted"` | Submitted |
| `"Filled"` | Filled (if remaining == 0) or PartiallyFilled |
| `"PendingCancel"` | PendingCancel |
| `"Cancelled"` | Cancelled or Inactive (depending on deactivate flag) |
| `"ApiCancelled"` | Cancelled or Inactive |
| `"Inactive"` | Rejected (IB uses "Inactive" for rejected/invalid orders) |

Note: IB's `"Inactive"` status means "order not working due to error or unmet conditions" -- this maps to our `Rejected`, not our `Inactive`. Our `Inactive` is an application-level concept for deactivated orders.

### 1.5 -- Audit Logging

Every state change writes to `order_audit`:

```sql
INSERT INTO order_audit (id, order_id, old_status, new_status, ib_order_id, detail, created_at)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7);
```

The `detail` column stores JSON with contextual info: fill price, rejection reason, modification diff, etc.

### 1.6 -- CLI Test Harness

Create `examples/broker_cli.rs` (or a binary feature-gated target) that:

- Connects to IB Gateway
- Accepts interactive commands:
  - `create AAPL BUY 10 LMT 150.00` -- creates a draft order
  - `activate <uuid>` -- sends to IB
  - `cancel <uuid>` -- cancels at IB
  - `deactivate <uuid>` -- deactivates (cancel + preserve)
  - `activate <uuid>` -- re-activates a deactivated order
  - `modify <uuid> price=151.00` -- modifies price
  - `list` -- shows all orders with status
  - `audit <uuid>` -- shows audit trail for an order
- Prints all `BrokerEvent` values as they arrive

This harness is invaluable for manual testing throughout all subsequent phases.

**Acceptance criteria:**

- Can create a draft order, activate it (appears in TWS/Gateway), cancel it
- Can deactivate an order and re-activate it (new IB order ID, same local UUID)
- Can modify price on a working order
- Audit trail captures every state change with timestamps
- All operations survive a restart (orders persist in SQLite)

---

## Phase 2: Market Data (Week 3-4)

> **Goal**: Stream live ticks, request historical bars, cache in binary candle files, enforce pacing rules.
> **Effort**: 7-9 days
> **Depends on**: Phase 0 complete (can overlap with Phase 1)

### 2.1 -- Market Data Subscription Manager

`market_data/subscriptions.rs` manages active subscriptions with reference counting.

Multiple UI components may want data for the same symbol (e.g., a chart and an order panel). We should only have one IB subscription per symbol, with a reference count.

```rust
pub struct SubscriptionManager {
    active: HashMap<String, SubscriptionEntry>,
}

struct SubscriptionEntry {
    con_id: i32,
    ib_req_id: i32,
    ref_count: usize,
    subscribed_at: DateTime<Utc>,
}

impl SubscriptionManager {
    /// Increment ref count. If new, send reqMktData to IB.
    pub fn subscribe(&mut self, symbol: &str, con_id: i32) -> Result<(), BrokerError>;

    /// Decrement ref count. If zero, send cancelMktData to IB.
    pub fn unsubscribe(&mut self, symbol: &str) -> Result<(), BrokerError>;

    /// Re-subscribe all active subscriptions (used after reconnect).
    pub fn resubscribe_all(&mut self) -> Result<(), BrokerError>;

    /// Returns list of currently subscribed symbols.
    pub fn active_symbols(&self) -> Vec<String>;
}
```

### 2.2 -- Live Market Data Flow

Wire rust-ibapi's market data callbacks into the event system.

**`reqMktData` flow:**

1. UI sends `BrokerCommand::SubscribeMarketData { symbol, con_id }`
2. SubscriptionManager increments ref count, calls `client.req_mkt_data(...)` if new
3. IB sends tick callbacks (bid, ask, last, volume, etc.)
4. Engine converts to `TickData` struct, emits `BrokerEvent::Tick`
5. Broadcast channel delivers to all subscribers

**`reqRealTimeBars` flow (5-second bars):**

1. Useful for chart real-time updates
2. IB delivers 5-second OHLCV bars
3. Engine converts to `BarData`, emits `BrokerEvent::Bar`

**Data types:**

```rust
pub struct TickData {
    pub timestamp: DateTime<Utc>,
    pub bid: Option<f64>,
    pub ask: Option<f64>,
    pub last: Option<f64>,
    pub volume: Option<i64>,
    pub bid_size: Option<i64>,
    pub ask_size: Option<i64>,
}

pub struct BarData {
    pub timestamp: DateTime<Utc>,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: i64,
    pub wap: Option<f64>,
    pub count: Option<i32>,
}
```

### 2.3 -- Historical Data with Caching

`market_data/cache.rs` caches historical bars in SQLite to avoid redundant IB requests.

**Request flow:**

1. UI sends `BrokerCommand::RequestHistoricalData { symbol, con_id, duration, bar_size }`
2. Check SQLite cache: do we already have this data range?
3. If fully cached: emit `BrokerEvent::HistoricalDataComplete` immediately with cached bars
4. If partially cached: request only the missing range from IB
5. If not cached: request full range from IB
6. On response: write bars to SQLite, emit event

**Cache key**: `(symbol, bar_size, timestamp)`. The cache stores bars individually, allowing partial cache hits.

**Invalidation**: Daily bars are immutable after market close. Intraday bars for the current day should be re-fetched if older than 5 minutes (configurable).

### 2.4 -- Pacing Rule Rate Limiter

`market_data/rate_limiter.rs` enforces IB's historical data pacing rules.

| Rule | Implementation |
|---|---|
| Max 60 requests per 10-minute window | Sliding window counter |
| Same contract/exchange/type: max 6 in 2 seconds | Per-contract sliding window |
| Identical request repeat: 15-second cooldown | Request fingerprint cache with timestamps |
| BID_ASK counts as 2 | Weight multiplier |

```rust
pub struct PacingLimiter {
    global_window: SlidingWindow,         // 60 per 600s
    per_contract: HashMap<i64, SlidingWindow>,  // 6 per 2s
    request_cache: HashMap<u64, Instant>,  // fingerprint -> last request time
}

impl PacingLimiter {
    /// Returns Ok(()) if request can proceed, or Err with wait duration.
    pub fn check(&mut self, req: &HistoricalRequest) -> Result<(), Duration>;

    /// Record that a request was sent.
    pub fn record(&mut self, req: &HistoricalRequest);
}
```

When a request is rate-limited, the engine queues it and retries after the indicated delay. The caller receives the data asynchronously via `BrokerEvent` regardless.

**Acceptance criteria:**

- Can subscribe to live ticks for a symbol, see `BrokerEvent::Tick` flow through broadcast channel
- Can request historical bars, get cached results on subsequent requests
- Pacing limiter correctly delays requests that would violate IB rules
- `resubscribe_all()` correctly restores all subscriptions after disconnect
- CLI harness extended: `subscribe AAPL`, `unsubscribe AAPL`, `history AAPL 30D 1hour`

### ~~2.5 -- Integration with midas-feed~~ (deferred)

> **Deferred.** The `midas-feed` crate does not exist yet. Integration with its
> `DataProvider` trait will be implemented when `midas-feed` is created. This is not
> part of v1 scope.

---

### Integration Checkpoint: Phase 1 + Phase 2

If Phases 1 and 2 were developed with any overlap, merge both command handler sets into the unified engine `select!` loop before proceeding. Verify:
- Both order commands and market data subscriptions work in the same session
- The CLI harness can place an order AND stream ticks simultaneously
- Connection state changes correctly gate both subsystems

**Effort**: 0.5-1 day.

---

## Phase 3: Account & Positions (Week 4-5)

> **Goal**: Accurate position, account value, and P&L tracking that survives restarts.
> **Effort**: 6-8 days
> **Depends on**: Phase 1 (order fills drive position changes)

### 3.1 -- Position Tracking

`account/positions.rs` maintains a local cache of positions, backed by SQLite.

**Data flow:**

1. On startup (and reconnect): call `client.req_positions()`
2. IB sends position callbacks for each held position
3. Engine updates local cache and SQLite
4. Emit `BrokerEvent::PositionUpdate` for each change

**Position struct:**

```rust
pub struct Position {
    pub account: String,
    pub symbol: String,
    pub con_id: i32,
    pub quantity: f64,
    pub avg_cost: f64,
    pub market_value: Option<f64>,
    pub unrealized_pnl: Option<f64>,
    pub realized_pnl: Option<f64>,
    pub updated_at: DateTime<Utc>,
}
```

Positions are also updated incrementally from fill events. When an order fills, we update the corresponding position immediately (without waiting for the next `reqPositions` response).

### 3.2 -- Account Summary

`account/summary.rs` tracks account-level values.

**Key values to track:**

- `NetLiquidation` -- total account value
- `TotalCashValue` -- cash balance
- `BuyingPower` -- available buying power
- `AvailableFunds` -- funds available for trading
- `MaintMarginReq` -- current margin requirement
- `ExcessLiquidity` -- excess liquidity (margin cushion)
- `Cushion` -- margin cushion percentage
- `DayTradesRemaining` -- PDT rule tracking

**Data flow:**

1. On startup: call `client.req_account_summary("All", tags)`
2. IB sends account value callbacks
3. Engine updates local cache (HashMap) and SQLite
4. Emit `BrokerEvent::AccountUpdate`
5. IB continues to push updates as values change

### 3.3 -- P&L Tracking

`account/pnl.rs` uses IB's `reqPnL` for account-level P&L and `reqPnLSingle` for per-position P&L.

```rust
pub struct PnlSnapshot {
    pub daily_pnl: f64,
    pub unrealized_pnl: f64,
    pub realized_pnl: f64,
    pub timestamp: DateTime<Utc>,
}
```

P&L updates flow through `BrokerEvent::PnlUpdate`.

### 3.4 -- Startup Reconciliation

`reconcile.rs` runs on every startup (and reconnect) to synchronize local state with IB's actual state.

**Reconciliation steps:**

1. **Request open orders from IB:** `client.req_open_orders()`
2. **For each IB open order:**
   - Look up by `ib_perm_id` in our DB (permanent ID survives reconnect)
   - If found: update local status to match IB's status
   - If not found: create a new `LocalOrder` record (order was placed outside our system, e.g., via TWS GUI)
3. **For each local order in `Submitted`/`PreSubmitted` status:**
   - If not found in IB's open orders: mark as `Filled` or `Cancelled` (check execution reports)
4. **Request executions since last known fill:** `client.req_executions()`
   - Match against local fills, record any we missed
5. **Request positions and account values** (as above)
6. **Log all reconciliation actions** to audit table

**Edge cases:**

- Order partially filled during disconnect: update `filled_qty` and `avg_fill_price`
- Order cancelled during disconnect: transition to `Cancelled`
- Manual order placed via TWS: create `LocalOrder` with status mirroring IB
- Order modified via TWS: update local fields to match

**Acceptance criteria:**

- After restart, all positions match IB's actual positions
- Account values are current and refreshing
- Orders placed before restart show correct status
- P&L values updating in real-time
- CLI harness extended: `positions`, `account`, `pnl`, `reconcile`

---

## Phase 4: Advanced Orders (Week 5-6)

> **Goal**: Bracket orders, OCA groups, Adaptive algo, trailing stops.
> **Effort**: 7-10 days
> **Depends on**: Phase 1 complete
>
> **Scope note**: Conditional orders and advanced algos (VWAP, TWAP, Arrival Price) are
> deferred to v2 per 02-order-management.md §3.5. Only Adaptive algo is included in v1.

### 4.1 -- Bracket Orders

`orders/bracket.rs` manages parent + take-profit + stop-loss as a unit.

**Design:**

A bracket is three `LocalOrder` records linked by `parent_id`. The parent's UUID is stored on both children. We expose a single `BrokerCommand::CreateBracketOrder` that creates all three atomically.

**Submission to IB:**

1. Allocate three consecutive IB order IDs
2. Set `transmit = false` on parent and first child
3. Set `transmit = true` on last child (triggers entire bracket)
4. Place all three via `client.place_order()`

**Lifecycle management:**

- If parent fills: children become active (IB handles this)
- If one child fills: IB cancels the other child (implicit OCA)
- If parent is cancelled: cancel both children
- Deactivate bracket: cancel all three at IB, mark all as `Inactive`
- Re-activate bracket: re-submit all three with new IB order IDs

**Bracket modification:**

- Modify take-profit price: `place_order()` with same `ib_order_id` on the TP child
- Modify stop-loss price: same for the SL child
- Modify parent (while unfilled): same for parent

### 4.2 -- OCA Groups

`orders/group.rs` supports OCA (One-Cancels-All) groups.

```rust
pub struct OcaGroup {
    pub name: String,
    pub oca_type: OcaType,     // CancelWithBlock, ReduceWithBlock, ReduceWithoutBlock
    pub order_ids: Vec<Uuid>,
}

pub enum OcaType {
    CancelWithBlock,     // One fills -> all others cancelled
    ReduceWithBlock,     // One fills -> others' qty reduced
    ReduceWithoutBlock,  // Same, but multiple can be live
}
```

OCA groups are created by setting `oca_group` and `oca_type` on each order before submission. The `OrderManager` ensures all orders in a group use the same `oca_group` string.

### 4.3 -- Adaptive Algo Orders

Support IB's Adaptive algo — the only algo in v1 (see 02-order-management.md §3.3).

```rust
pub enum AlgoStrategy {
    Adaptive { priority: AdaptivePriority },
}

pub enum AdaptivePriority { Urgent, Normal, Patient }
```

Adaptive can be applied to any limit order. It reduces market impact with minimal parameterization. Algo parameters are set on the IB `Order` object during submission and stored as JSON in the `orders` table.

> **Deferred to v2**: VWAP, TWAP, Arrival Price, PctOfVolume, Conditional orders.
> See 02-order-management.md §3.5 for rationale.

### 4.4 -- Trailing Stops

Trailing stops use `orderType = "TRAIL"` or `"TRAIL LIMIT"` with either:

- `trail_amount`: absolute dollar trailing distance
- `trail_percent`: percentage trailing distance

These are straightforward to implement as another `OrderKind` variant. IB manages the actual trailing logic server-side.

### 4.5 -- Order Tags and Bulk Operations

Support user-defined tags for grouping orders.

```rust
impl OrderManager {
    /// Cancel all orders with a given tag.
    pub fn cancel_by_tag(&self, tag: &str) -> Result<Vec<Uuid>, BrokerError>;

    /// Deactivate all orders with a given tag.
    pub fn deactivate_by_tag(&self, tag: &str) -> Result<Vec<Uuid>, BrokerError>;

    /// Activate all inactive orders with a given tag.
    pub fn activate_by_tag(&self, tag: &str) -> Result<Vec<Uuid>, BrokerError>;
}
```

Tags are stored in the `tag` column of the `orders` table. This enables scenarios like "cancel all my AAPL orders" or "deactivate everything tagged 'earnings-play'".

**Acceptance criteria:**

- Can create and submit bracket orders (parent + TP + SL)
- Modifying a bracket leg updates correctly at IB
- OCA groups work: filling one cancels the others
- Adaptive algo orders submit and fill
- Trailing stops trail correctly in paper trading
- Bulk cancel/deactivate by tag works
- All advanced order types persist and survive restart

---

## Phase 5: iced Integration (Week 6-7)

> **Goal**: Broker engine running as an iced Subscription, with UI panels for orders, positions, and account.
> **Effort**: 7-9 days
> **Depends on**: Phases 1-3 complete, midas-app shell exists

### 5.1 -- BrokerEngine as iced Subscription

The `BrokerEngine` must integrate with iced's event loop. iced uses a `Subscription` model where background tasks produce `Message` values.

```rust
// In midas-app:
pub enum Message {
    // ... existing chart/UI messages ...
    BrokerEvent(BrokerEvent),
    BrokerCommand(BrokerCommand),  // From UI interactions
}

// Subscription that bridges BrokerEngine -> iced Messages
pub fn broker_subscription(
    event_rx: broadcast::Receiver<BrokerEvent>,
) -> Subscription<Message> {
    subscription::channel(
        std::any::TypeId::of::<BrokerSubscription>(),
        100,
        |mut output| async move {
            let mut rx = event_rx;
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        let _ = output.send(Message::BrokerEvent(event)).await;
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("Broker event subscriber lagged by {n} messages");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        },
    )
}
```

Commands flow the other direction: UI interactions create `BrokerCommand` values, which the app sends through the `mpsc` channel to the engine.

### 5.2 -- Order Panel

A sidebar or panel in the iced app showing:

- List of all orders (scrollable, filterable by status/symbol/tag)
- Each row shows: symbol, action, type, qty, price, status, filled qty
- Color-coded status: green (filled), yellow (submitted), red (rejected/cancelled), gray (draft/inactive)
- Action buttons per order:
  - `Activate` (for Draft/Inactive)
  - `Deactivate` (for Submitted/PreSubmitted)
  - `Cancel` (for any active state)
  - `Modify` (opens price/qty edit)

The panel maintains local state derived from `BrokerEvent::OrderStatusChanged` events. On startup, it loads the full order list from the engine.

### 5.3 -- Position Panel

Displays current positions:

- Symbol, quantity, avg cost, market value, unrealized P&L, daily P&L
- Color: green for profit, red for loss
- Updated in real-time from `BrokerEvent::PositionUpdate`

### 5.4 -- Account Summary Panel

Compact display of account values:

- Net liquidation, cash, buying power, margin used, excess liquidity
- Day trades remaining (PDT tracking)
- Connection status indicator (green/yellow/red)
- Updated in real-time from `BrokerEvent::AccountUpdate`

### 5.5 -- Quick Order Entry

A compact widget for placing orders directly from the chart or a toolbar:

- Symbol (auto-filled from active chart)
- Action toggle: Buy / Sell
- Quantity input
- Order type selector: Market, Limit, Stop
- Price input (for limit/stop)
- "Send" button -> creates Draft + immediately activates
- "Stage" button -> creates Draft only

**Acceptance criteria:**

- Broker engine starts automatically with the iced app
- Orders visible in the order panel, updating in real-time
- Can activate/deactivate/cancel orders from the UI
- Positions and account values display and update live
- Connection status indicator shows green/yellow/red
- Quick order entry can place a market order from the chart

---

## Phase 6: Resilience (Week 7-8)

> **Goal**: Engine runs reliably through IB Gateway daily restarts, connection drops, and error conditions.
> **Effort**: 7-9 days
> **Depends on**: Phases 1-3 complete

### 6.1 -- Connection Watchdog

`watchdog.rs` monitors the connection to IB Gateway and handles reconnection.

**Watchdog loop:**

```
loop {
    1. Check connection health (heartbeat / ping)
    2. If disconnected:
       a. Emit BrokerEvent::Disconnected
       b. Wait backoff_delay (exponential: 2s, 4s, 8s, ... max 60s)
       c. Attempt reconnect
       d. If connected:
          - Emit BrokerEvent::Connected
          - Run reconciliation (Phase 3.4)
          - Resubscribe market data (Phase 2.1)
          - Reset backoff
       e. If failed: increment backoff, loop
    3. Sleep 5 seconds
}
```

**Health check:** The watchdog tracks the last received message from IB. If no message received for `health_timeout` seconds (configurable, default 30), assume disconnected and trigger reconnect.

### 6.2 -- Daily Restart Handling

IB Gateway performs a mandatory restart at approximately 11:45 PM ET on weekdays. With auto-restart enabled, Gateway comes back within 1-3 minutes.

**Strategy:**

1. **Pre-restart (11:40 PM ET):** Watchdog enters "expecting restart" mode
   - Pause new order submissions
   - Cache all current subscription state
   - Emit `BrokerEvent::Disconnected { reason: "Daily restart (expected)" }`
2. **During restart:** Watchdog attempts reconnect every 10 seconds (no exponential backoff -- we know it is coming back)
3. **Post-restart:** Full reconciliation
   - Resubscribe all market data
   - Request open orders and match against local state
   - Request positions and account values
   - Resume normal operation
   - Emit `BrokerEvent::Connected`

**Sunday restart:** The weekly server reset on Saturday night requires manual re-authentication (2FA). The watchdog should detect this and emit a `BrokerEvent::Error` with a clear message that manual intervention is required. The UI should show a prominent notification.

### 6.3 -- Data Farm Status Tracking

IB sends error codes 2104/2106 (farms OK) and 2103/2105/2108 (farms down/inactive).

```rust
pub struct DataFarmTracker {
    market_data_ok: bool,
    historical_data_ok: bool,
}
```

- **Do not send market data requests until `market_data_ok == true`** (code 2104 received)
- **Do not send historical data requests until `historical_data_ok == true`** (code 2106 received)
- Queue requests made while farms are down, send when farms come back

### 6.4 -- Error Recovery

**Pacing violations:**

- If IB returns a pacing error, the rate limiter should add a penalty delay and retry
- Log the violation for monitoring

**Rejected orders:**

- Transition to `Rejected` status
- Write audit with rejection reason (from IB error message)
- Emit `BrokerEvent::OrderRejected`
- Do not auto-retry (human should review)

**Connection drops mid-order:**

- Order was sent but we never received confirmation
- On reconnect, reconciliation will find the order at IB and update local state
- If order was not received by IB, it will not appear in open orders -- transition to `Error` status

**Duplicate order ID:**

- IB error 103. Request a new order ID and retry once.

**Message rate throttling:**

- Track outgoing message rate
- If approaching 50 msg/sec limit, queue messages with a small delay
- Never exceed 45 msg/sec to maintain safety margin

### 6.5 -- Graceful Shutdown

When the application closes:

1. Cancel all market data subscriptions (clean up at IB)
2. Wait for any pending order operations to complete (with timeout)
3. Disconnect from IB Gateway cleanly
4. Close SQLite connection
5. Flush all tracing logs

**Important:** Do NOT cancel open orders on shutdown. The user's orders should remain working at IB even if the application is closed.

**Acceptance criteria:**

- Engine reconnects automatically after IB Gateway restart
- All subscriptions restored after reconnect
- Order state is correct after reconnect (including orders that filled during disconnect)
- Pacing violations handled gracefully (retry with delay)
- Application can close and reopen with full state restoration
- 24-hour stability test: run through at least one daily restart cycle in paper trading

---

## Phase 7: Polish (Week 8+)

> **Goal**: Production readiness -- configuration, logging, metrics, documentation.
> **Effort**: Ongoing

### 7.1 -- Configuration File

`config.rs` loads settings from TOML.

```toml
# broker.toml

[connection]
host = "127.0.0.1"
port = 4002                # 4002 = paper, 4001 = live
client_id = 1
allow_live = false         # SAFETY: must be explicitly set to true to connect to live (port 4001).
                           # If false and port == 4001, the engine refuses to connect.
                           # Implemented in Phase 0 — not deferred.
auto_reconnect = true
reconnect_delay_secs = 5
max_reconnect_delay_secs = 60
health_timeout_secs = 30

[database]
path = "data/broker.db"

[market_data]
default_bar_size = "5 mins"
cache_intraday_ttl_secs = 300
max_streaming_lines = 100

[orders]
default_tif = "DAY"
confirm_live_orders = true  # Require explicit confirmation before sending to live account
# NOTE: The live-trading safety guard (allow_live) is in [connection], implemented in Phase 0.
# This confirm_live_orders is an additional UI-level confirmation dialog.

[logging]
level = "info"              # trace, debug, info, warn, error
file = "logs/broker.log"
json_format = false
```

### 7.2 -- Structured Logging

Use `tracing` with structured fields for all significant operations.

```rust
tracing::info!(
    order_id = %order.id,
    symbol = %order.symbol,
    action = ?order.action,
    status = ?order.status,
    ib_order_id = ?order.ib_order_id,
    "Order status changed"
);
```

**Log categories:**

- `midas_broker::connection` -- connect/disconnect/reconnect events
- `midas_broker::orders` -- order lifecycle events
- `midas_broker::market_data` -- subscription changes, pacing events
- `midas_broker::account` -- position/account updates
- `midas_broker::reconcile` -- reconciliation actions
- `midas_broker::watchdog` -- health checks, restart detection

### 7.3 -- Metrics (Optional)

If we decide to add observability:

```rust
pub struct BrokerMetrics {
    pub orders_placed: u64,
    pub orders_filled: u64,
    pub orders_cancelled: u64,
    pub orders_rejected: u64,
    pub avg_fill_latency_ms: f64,    // Time from submit to first fill
    pub reconnect_count: u64,
    pub pacing_violations: u64,
    pub messages_per_second: f64,
}
```

Exposed via `BrokerCommand::GetMetrics` -> `BrokerEvent::Metrics(BrokerMetrics)`. Could also integrate with `metrics` crate for Prometheus export if we ever add a monitoring dashboard.

### 7.4 -- Performance Testing

Run extended paper trading sessions to validate:

- Order placement latency (command sent -> IB confirmation received)
- Market data throughput (ticks/second sustained)
- SQLite write throughput under heavy tick data
- Memory usage over 8+ hour sessions
- CPU usage during market hours vs. off-hours

**Mitigation for SQLite under heavy tick data:**

If SQLite becomes a bottleneck for tick storage (which is likely above ~1000 ticks/second):

1. Batch inserts with WAL mode (already default)
2. Separate tick cache from order/position DB
3. Use a ring buffer for recent ticks, write to DB in batches
4. Consider an in-memory buffer that flushes to disk periodically

### 7.5 -- Documentation

- `README.md` for the crate: architecture overview, how to run, configuration
- Rustdoc on all public types and methods
- Architecture decision records for key choices (rust-ibapi evaluation, deactivation strategy, etc.)
- Runbook: common issues and how to resolve (2FA problems, pacing violations, order rejections)

**Acceptance criteria:**

- Application is configurable via TOML without recompilation
- Logs are structured and filterable
- Runs stably through a full trading day in paper trading
- All public API surface has documentation

---

## Risk Register

| # | Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|---|
| R1 | **rust-ibapi has gaps for our needs** (missing message types for conditional orders, algo params, or newer TWS server versions) | Medium | High | Evaluate in Phase 0. Fork if needed -- the crate is open source. Contribute fixes upstream where possible. |
| R2 | **IB Gateway daily restart + 2FA** creates reliability challenges for unattended operation | High | Medium | Implement watchdog with daily restart awareness (Phase 6). Use secondary IB user without 2FA for API connections. Document Sunday re-auth requirement. |
| R3 | **rust-ibapi v3 in development** -- API surface may change, breaking our integration | Low | Medium | Pin to a specific version in Cargo.toml. Monitor the repository for breaking changes. If forking, we control our own timeline. |
| R4 | **iced async integration edge cases** -- iced's Subscription model may not handle high-frequency events well | Medium | Medium | Use broadcast channel with bounded capacity. Accept message lag for non-critical events (ticks). Order events must never be dropped -- use a separate guaranteed-delivery channel if needed. |
| R5 | **SQLite performance under heavy tick data** -- writing every tick to SQLite could bottleneck | High | Low | Only cache historical bars and order data in SQLite. Use in-memory ring buffers for live ticks. Batch writes with WAL mode. Separate tick DB if needed. |
| R6 | **Order state divergence** between local DB and IB's actual state | Medium | High | Full reconciliation on every connect/reconnect. Use `ib_perm_id` as the stable identifier across sessions. Audit log enables forensic analysis of divergence. |
| R7 | **rust-ibapi uses blocking I/O** conflicting with our async Tokio runtime | Low | Medium | rust-ibapi v2.10+ provides a native async `Client` as the default interface, so this is unlikely to be an issue. If needed, wrap calls in `tokio::task::spawn_blocking` and route callbacks through Tokio channels. |
| R8 | **IB rate limits** -- accidental violation leads to connection drops or temporary bans | Medium | Medium | Pacing rate limiter (Phase 2.4). Message rate throttling (Phase 6.4). Conservative defaults with configurable overrides. |
| R9 | **Paper trading behavior differs from live** -- fills are more optimistic, some order types unavailable | Medium | Low | Document differences. Test with small live orders before full deployment. Paper trading is sufficient for integration testing. |
| R10 | **Cross-platform SQLite bundling** on Windows may have build issues | Low | Low | Use `rusqlite` with `bundled` feature (compiles SQLite from source). This is well-tested on Windows. |

---

## Dependencies

### Required Crates

```toml
[dependencies]
# IB connectivity
ibapi = "2"                                    # rust-ibapi — TWS protocol implementation
                                                # Pin exact version after Phase 0 evaluation

# Database
rusqlite = { version = "0.32", features = ["bundled"] }  # SQLite with bundled C library
# Migrations use PRAGMA user_version (see 03-data-layer.md §3) — no external framework needed

# Async runtime (workspace-shared)
tokio = { version = "1", features = [
    "rt-multi-thread", "sync", "time", "macros", "signal"
] }

# Serialization
serde = { version = "1", features = ["derive"] }
serde_json = "1"

# Identity
uuid = { version = "1", features = ["v7", "serde"] }

# Time
chrono = { version = "0.4", features = ["serde"] }

# Logging
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }

# Error handling
thiserror = "2"

# Config
toml = "0.8"

# Platform paths (XDG / AppData / Library)
dirs = "5"
```

### Dev / Test Dependencies

```toml
[dev-dependencies]
tokio-test = "0.4"
tempfile = "3"          # Temp directories for test DBs
assert_matches = "1"    # Readable assertion macros
```

### Workspace Integration

`midas-broker` depends on `midas-core` (created in Phase 0.2) for shared types (`ContractSpec`, `SymbolKey`, `Timeframe`). In Phase 5, `midas-app` will depend on `midas-broker`.

```
midas-app
├── midas-broker        (Phase 5+)
│   └── midas-core
├── midas-render
├── midas-core
├── midas-data
├── midas-feed
└── midas-indicators
```

---

## Testing Strategy

### Unit Tests

Run without IB Gateway. Use in-memory SQLite.

| Area | What to test |
|---|---|
| **Order state machine** | Every valid transition succeeds. Every invalid transition returns `BrokerError::InvalidTransition`. Terminal state detection. `is_active_at_ib()` correctness. |
| **Order persistence** | Create, read, update, delete. Query by status, tag, symbol. Fill recording. Audit trail written on every status change. |
| **Rate limiter** | Sliding window enforces limits. Per-contract limits enforced. Identical request cooldown works. BID_ASK double-counting. |
| **Subscription manager** | Ref counting: first subscribe creates, second increments. Unsubscribe decrements. Zero ref count removes. Resubscribe restores all. |
| **Configuration** | TOML parsing. Default values. Validation of port ranges, timeouts, etc. |
| **Error types** | All error variants constructible. Display messages are useful. Conversion from rusqlite errors works. |
| **Reconciliation logic** | Given local state X and IB state Y, reconciliation produces correct updates Z. (Test with mock data, no IB connection.) |

**Target: 80%+ code coverage on non-IB code paths.**

### Integration Tests

Require IB Gateway running in paper trading mode on port 4002. Gated behind a feature flag or environment variable so CI can skip them.

```rust
#[cfg(feature = "ib-integration")]
#[tokio::test]
async fn test_order_lifecycle() {
    // Connect to IB Gateway on port 4002
    // Place a limit buy order well below market
    // Verify status transitions: Draft -> PendingSubmit -> Submitted
    // Cancel the order
    // Verify status: PendingCancel -> Cancelled
    // Verify audit trail has all transitions
}
```

| Test | What it validates |
|---|---|
| `test_connect_disconnect` | Can connect to Gateway, receive account ID, disconnect cleanly |
| `test_order_lifecycle` | Create -> activate -> cancel flow with real IB callbacks |
| `test_order_deactivate_reactivate` | Deactivate -> Inactive -> re-activate with new IB order ID |
| `test_bracket_order` | Parent + TP + SL submitted as a unit, children linked correctly |
| `test_modify_order` | Modify limit price on a working order |
| `test_historical_data` | Request 30 days of daily bars, verify data received and cached |
| `test_market_data_subscription` | Subscribe, receive ticks, unsubscribe |
| `test_positions_and_account` | Request positions and account values, verify non-empty |
| `test_reconnect_reconciliation` | Disconnect, reconnect, verify state is consistent |
| `test_pacing_limiter` | Rapid historical data requests are properly throttled |

### Mock IB Server (Future / Optional)

For CI without IB Gateway, we could build a mock server that speaks enough of the TWS protocol to test our code. This is a significant effort and should only be pursued if integration testing becomes a bottleneck.

**Simpler alternative:** Record/replay. Capture IB message sequences during manual testing and replay them in tests. This tests our parsing and state management without requiring a live connection.

### Manual Testing Checklist

Before each phase is considered complete:

- [ ] Run the CLI test harness through all new features
- [ ] Observe behavior in TWS (visual confirmation that orders appear/disappear correctly)
- [ ] Restart the application and verify state is preserved
- [ ] Let it run through at least one IB daily restart cycle (for Phase 6+)
- [ ] Review audit log for any unexpected transitions

---

## Appendix A: SQLite Schema

> **Note:** 03-data-layer.md is the canonical source for all SQLite schemas. This appendix retains only the `positions`, `account_values`, and `contracts` tables for quick reference.

```sql
-- V001__initial_schema.sql

-- See 03-data-layer.md §1a for the authoritative orders table DDL.

-- See 03-data-layer.md §1b for the authoritative order_audit table DDL.

-- See 03-data-layer.md §1c for the authoritative fills table DDL.

-- See 03-data-layer.md §1d for the authoritative positions table DDL.

-- Account summary key-value store
CREATE TABLE IF NOT EXISTS account_values (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    account         TEXT NOT NULL,
    tag             TEXT NOT NULL,               -- 'NetLiquidation', 'BuyingPower', etc.
    value           TEXT NOT NULL,
    currency        TEXT NOT NULL DEFAULT 'USD',
    updated_at      TEXT NOT NULL,
    UNIQUE(account, tag, currency)
);

-- Cached IB contract details
CREATE TABLE IF NOT EXISTS contracts (
    con_id          INTEGER PRIMARY KEY,
    symbol          TEXT NOT NULL,
    sec_type        TEXT NOT NULL,               -- 'STK', 'OPT', 'FUT', 'CASH'
    exchange        TEXT NOT NULL,
    primary_exchange TEXT,
    currency        TEXT NOT NULL,
    local_symbol    TEXT,
    multiplier      TEXT,
    last_trade_date TEXT,
    strike          REAL,
    right_          TEXT,                        -- 'C' | 'P' (options only)
    details_json    TEXT,                        -- Full contract details as JSON
    cached_at       TEXT NOT NULL
);

CREATE INDEX idx_contracts_symbol ON contracts(symbol);

-- Historical bar cache uses binary .candles files, not SQLite. See 04-market-data-and-events.md §2.2.

-- No config table in v1. Configuration is via TOML file (see 01-architecture.md §8).

-- Enable WAL mode for concurrent read/write performance
PRAGMA journal_mode = WAL;
```

---

## Appendix B: State Machine Diagrams

### Order Status State Machine

```
                     ┌──────────┐
          create()   │          │
         ──────────► │  Draft   │
                     │          │
                     └────┬─────┘
                          │
                          │ activate()
                          ▼
                  ┌───────────────┐
                  │ PendingSubmit  │◄─────────────── activate() from Inactive
                  └───────┬───────┘
                          │
                 IB confirms submission
                          │
              ┌───────────┴───────────┐
              ▼                       ▼
      ┌───────────────┐       ┌───────────────┐
      │  Submitted     │       │ PreSubmitted   │ (simulated, e.g. stop order)
      │  (working at   │       │ (waiting for   │
      │   exchange)    │       │  trigger)      │
      └───┬─────┬──┬──┘       └──┬──────┬──┬──┘
          │     │  │              │      │  │
          │     │  │  modify()    │      │  │  modify()
          │     │  └──────┐       │      │  └──────┐
          │     │         │       │      │         │
          │     │         ▼       │      │         ▼
          │     │    (same state, │      │    (same state,
          │     │     new params) │      │     new params)
          │     │                 │      │
     partial    │ full fill  trigger    │ full fill
      fill      │                │      │
          │     │                ▼      │
          ▼     │        ┌──────────┐   │
  ┌────────────┐│        │Submitted │───┘
  │ Partially  ││        └──────────┘
  │  Filled    ││
  └──────┬─────┘│
         │      │
    full fill   │
         │      │
         ▼      ▼
      ┌────────────┐
      │   Filled    │ (terminal)
      └────────────┘

  --- Cancellation / Deactivation ---

  From Submitted, PreSubmitted, PartiallyFilled:

      cancel()             deactivate()
         │                      │
         ▼                      ▼
  ┌───────────────┐     ┌───────────────┐
  │ PendingCancel  │     │ PendingCancel  │ (flag: is_deactivate=true)
  └───────┬───────┘     └───────┬───────┘
          │                     │
     IB confirms           IB confirms
          │                     │
          ▼                     ▼
  ┌───────────────┐     ┌───────────────┐
  │  Cancelled     │     │   Inactive     │
  │  (terminal)    │     │ (can reactivate│
  └───────────────┘     │  via activate())│
                        └───────────────┘

  --- Error / Rejection ---

  From PendingSubmit:

     IB rejects
         │
         ▼
  ┌───────────────┐
  │   Rejected     │ (terminal)
  └───────────────┘

  From any non-terminal state:

     unrecoverable error
         │
         ▼
  ┌───────────────┐
  │    Error       │ (requires manual resolution)
  └───────────────┘
```

### Connection State Machine

```
  ┌───────────────┐
  │ Disconnected   │◄──────────────────────────┐
  └───────┬───────┘                            │
          │                                    │
     connect()                           connection lost
          │                              or health timeout
          ▼                                    │
  ┌───────────────┐                            │
  │  Connecting    │                            │
  └───────┬───────┘                            │
          │                                    │
     success / failure                         │
          │                                    │
     ┌────┴─────┐                              │
     ▼          ▼                              │
  ┌──────┐  ┌────────────────┐                 │
  │Failed│  │  Connected      │────────────────┘
  └──┬───┘  │  (data farms    │
     │      │   may still be   │
     │      │   initializing)  │
     │      └────────┬────────┘
     │               │
     │          2104/2106 received
     │               │
     │               ▼
     │      ┌────────────────┐
     │      │    Ready        │  (fully operational)
     │      └────────────────┘
     │
     │ retry after backoff
     └──────────► Connecting
```

---

## Appendix C: Event and Command Catalog

### BrokerCommand (UI -> Engine)

| Command | Parameters | Description |
|---|---|---|
| `CreateOrder` | `LocalOrder` | Create a new draft order (not yet sent to IB) |
| `ActivateOrder` | `order_id: Uuid` | Send a Draft or Inactive order to IB |
| `DeactivateOrder` | `order_id: Uuid` | Cancel at IB, preserve locally as Inactive |
| `CancelOrder` | `order_id: Uuid` | Cancel at IB permanently |
| `ModifyOrder` | `order_id, new_price, new_qty` | Modify a working order |
| `CreateBracketOrder` | `entry, take_profit, stop_loss` | Create a bracket (3 linked orders) |
| `CancelByTag` | `tag: String` | Cancel all orders with this tag |
| `DeactivateByTag` | `tag: String` | Deactivate all orders with this tag |
| `ActivateByTag` | `tag: String` | Re-activate all inactive orders with this tag |
| `SubscribeMarketData` | `symbol, con_id` | Start streaming live ticks |
| `UnsubscribeMarketData` | `symbol` | Stop streaming live ticks |
| `RequestHistoricalData` | `symbol, con_id, duration, bar_size` | Request historical bars (cached if possible) |
| `RequestPositions` | -- | Refresh position snapshot |
| `RequestAccountSummary` | -- | Refresh account values |
| `Reconnect` | -- | Force a reconnect |
| `Shutdown` | -- | Clean shutdown |

### BrokerEvent (Engine -> UI)

| Event | Parameters | Description |
|---|---|---|
| `Connected` | -- | Connection to IB Gateway established |
| `Disconnected` | `reason: String` | Connection lost |
| `Reconnecting` | `attempt: u32` | Reconnect attempt in progress |
| `Ready` | -- | Data farms OK, fully operational |
| `OrderCreated` | `order_id: Uuid` | New order saved to DB |
| `OrderStatusChanged` | `order_id, old, new` | Order state transition |
| `OrderFilled` | `order_id, fill: FillInfo` | Order (partially or fully) filled |
| `OrderRejected` | `order_id, reason` | IB rejected the order |
| `OrderError` | `order_id, code, message` | IB error related to an order |
| `Tick` | `symbol, tick: TickData` | Live tick update |
| `Bar` | `symbol, bar: BarData` | Live 5-second bar |
| `HistoricalDataComplete` | `request_id, symbol` | Historical data request fulfilled |
| `PositionUpdate` | `position: Position` | Position changed |
| `AccountUpdate` | `key, value, currency` | Account value changed |
| `PnlUpdate` | `daily_pnl, unrealized, realized` | P&L update |
| `DataFarmStatus` | `farm, ok` | Data farm status change |
| `Error` | `code, message` | System-level error |
| `ReconciliationComplete` | `changes: Vec<String>` | Startup reconciliation finished |
