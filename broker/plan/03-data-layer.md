# Data Persistence Layer: `midas-broker`

> Plan document for the SQLite-backed persistence layer in the `midas-broker` crate.
> This crate wraps Interactive Brokers via `rust-ibapi` and manages order lifecycle,
> position tracking, and account data caching. Historical bar data uses a binary candle
> file format (see §1f) rather than SQLite.

---

## Table of Contents

- [1. SQLite Schema](#1-sqlite-schema)
  - [1a. `orders`](#1a-orders)
  - [1b. `order_audit`](#1b-order_audit)
  - [1c. `fills`](#1c-fills)
  - [1d. `positions`](#1d-positions)
  - [1e. `account_values`](#1e-account_values)
  - [1f. Historical Bar Cache (binary candle files)](#1f-historical-bar-cache-binary-candle-files)
  - [1g. `contracts`](#1g-contracts)
- [2. Rust Types](#2-rust-types)
  - [Key Structs](#key-structs)
  - [Enums and Storage Strategy](#enums-and-storage-strategy)
  - [Serde for JSON Fields](#serde-for-json-fields)
- [3. Database Access Pattern](#3-database-access-pattern)
  - [Connection Management](#connection-management)
  - [WAL Mode](#wal-mode)
  - [Prepared Statements](#prepared-statements)
  - [Migrations](#migrations)
  - [Transaction Boundaries](#transaction-boundaries)
- [4. Data Sync Strategy](#4-data-sync-strategy)
  - [Startup Reconciliation](#startup-reconciliation)
  - [Runtime Updates](#runtime-updates)
  - [Periodic Full Reconciliation](#periodic-full-reconciliation)
  - [Market Data Cache TTL](#market-data-cache-ttl)
- [5. Backup and Recovery](#5-backup-and-recovery)
  - [File Location](#file-location)
  - [WAL Checkpointing](#wal-checkpointing)
  - [Corruption Recovery](#corruption-recovery)

---

## 1. SQLite Schema

All tables use strict typing where possible. Timestamps are stored as RFC 3339 strings
(`2026-03-24T14:30:00.000Z`) for human readability and unambiguous timezone handling.
Monetary values (prices, commissions) are stored as `REAL` (f64) since SQLite does not
have a native decimal type; the broker layer does not perform arithmetic on prices in SQL.

### 1a. `orders`

The primary order table. One row per order managed by midas-broker. The `local_id` is
generated locally (UUIDv7 for time-sortability) before the order is ever sent to IB.

```sql
CREATE TABLE orders (
    -- Identity
    local_id        TEXT    NOT NULL PRIMARY KEY,  -- UUIDv7 (our internal ID)
    ib_order_id     INTEGER,                       -- IB's orderId (set when sent to IB)
    ib_perm_id      INTEGER,                       -- IB's permanent ID (set after first ack)

    -- Status
    status          TEXT    NOT NULL DEFAULT 'Draft',
        -- CHECK(status IN (
        --     'Draft',            -- created locally, not yet sent
        --     'Inactive',         -- locally parked, not sent to IB (or deactivated)
        --     'PendingSubmit',    -- sent from TWS, awaiting destination confirmation
        --     'PreSubmitted',     -- simulated order accepted, not yet triggered
        --     'Submitted',        -- working at the exchange
        --     'PartiallyFilled',  -- some shares filled, order still working
        --     'Filled',           -- completely filled
        --     'PendingCancel',    -- cancel request sent, not yet confirmed
        --     'Cancelled',        -- confirmed cancelled (includes ApiCancelled)
        --     'Rejected',         -- rejected by IB or exchange (terminal)
        --     'Error'             -- local/internal error (non-terminal, retryable)
        -- ))

    -- Contract identification
    symbol          TEXT    NOT NULL,
    sec_type        TEXT    NOT NULL DEFAULT 'STK',  -- STK, OPT, FUT, CASH, etc.
    exchange        TEXT    NOT NULL DEFAULT 'SMART',
    currency        TEXT    NOT NULL DEFAULT 'USD',
    con_id          INTEGER,                          -- IB contract ID, set after qualification

    -- Order parameters
    action          TEXT    NOT NULL,                  -- BUY, SELL
    order_type      TEXT    NOT NULL,                  -- MKT, LMT, STP, STP_LMT, TRAIL, TRAIL_LIMIT,
                                                      -- MIT, LIT, MTL, MOC, LOC, REL, VOL, VWAP, etc.
    quantity        REAL    NOT NULL,                  -- total order quantity
    filled_qty      REAL    NOT NULL DEFAULT 0.0,
    remaining_qty   REAL    NOT NULL,                  -- initially = quantity

    -- Price fields (nullable — not all order types use all fields)
    limit_price     REAL,
    stop_price      REAL,
    trail_amount    REAL,                              -- absolute trailing amount
    trail_percent   REAL,                              -- trailing percentage (0-100)

    -- Time in force
    tif             TEXT    NOT NULL DEFAULT 'DAY',    -- DAY, GTC, IOC, FOK, OPG, GTD

    -- Bracket / OCA linkage
    parent_id       TEXT,                              -- local_id of parent order (bracket)
    bracket_role    TEXT,                              -- NULL for standalone, 'parent'/'take_profit'/'stop_loss'
    oca_group       TEXT,                              -- OCA group name

    -- Strategy / user metadata
    strategy        TEXT,                              -- user-defined strategy tag (nullable)
    tags            TEXT    NOT NULL DEFAULT '[]',      -- JSON array of strings, e.g. ["swing","tech"]

    -- Algo
    algo_strategy   TEXT,                              -- e.g. "Adaptive", "Vwap", "Twap"
    algo_params     TEXT,                              -- JSON object, e.g. {"adaptivePriority":"Patient"}

    -- Extended hours / scheduling
    outside_rth     INTEGER NOT NULL DEFAULT 0,        -- boolean: 0 = false, 1 = true
    good_after_time TEXT,                              -- RFC 3339 or IB format timestamp
    good_till_date  TEXT,                              -- RFC 3339 or IB format timestamp

    -- Fill info (updated as fills come in)
    avg_fill_price  REAL,
    last_fill_price REAL,
    commission      REAL,                              -- total accumulated commission

    -- Activation history
    activation_count INTEGER NOT NULL DEFAULT 0,       -- how many times this order was activated
    last_activated_at   TEXT,                           -- RFC 3339, nullable
    last_deactivated_at TEXT,                           -- RFC 3339, nullable

    -- Timestamps
    created_at      TEXT    NOT NULL,                   -- RFC 3339
    updated_at      TEXT    NOT NULL,                   -- RFC 3339, updated on every change
    submitted_at    TEXT,                               -- when first sent to IB
    filled_at       TEXT,                               -- when status became Filled
    cancelled_at    TEXT,                               -- when status became Cancelled

    -- Constraints
    FOREIGN KEY (parent_id) REFERENCES orders(local_id)
);

-- Indices
CREATE INDEX idx_orders_status        ON orders(status);
CREATE INDEX idx_orders_symbol        ON orders(symbol);
CREATE INDEX idx_orders_ib_order_id   ON orders(ib_order_id);
CREATE INDEX idx_orders_ib_perm_id    ON orders(ib_perm_id);
CREATE INDEX idx_orders_parent_id     ON orders(parent_id);
CREATE INDEX idx_orders_oca_group     ON orders(oca_group);
CREATE INDEX idx_orders_created_at    ON orders(created_at);
CREATE INDEX idx_orders_status_symbol ON orders(status, symbol);

-- Composite index for the most common query: "show me all active orders"
-- Active = NOT IN ('Draft', 'Filled', 'Cancelled', 'Rejected')
-- SQLite can use the status index for this; no partial index needed.
```

**Design decisions:**

- `local_id` is a UUIDv7 string (not integer) so we can generate it before any IB round-trip. UUIDv7 embeds a timestamp, giving natural time-ordering in the primary key.
- `ib_order_id` is nullable because Draft orders have not yet been assigned an IB order ID. This is set when `placeOrder()` is called.
- `ib_perm_id` is nullable and set on the first `orderStatus` callback from IB. The permId is globally unique and survives reconnections, unlike `ib_order_id` which is session-scoped.
- `quantity`, `filled_qty`, and `remaining_qty` are `REAL` to support fractional shares (IB supports them for some instruments).
- `tags` is a JSON array stored inline rather than a separate table. Tag-based filtering is infrequent and the expected tag count per order is small (< 10). This avoids join overhead for the common case.
- `con_id` is stored for fast contract lookups and joins with the `contracts` cache table.
- `status` uses a CHECK-compatible set of values. The CHECK constraint is commented out for clarity; it is enforced at the Rust layer via the `OrderStatus` enum.

### 1b. `order_audit`

Every state change is logged here. This is an append-only table; rows are never updated or deleted.

```sql
CREATE TABLE order_audit (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    order_local_id  TEXT    NOT NULL,
    timestamp       TEXT    NOT NULL,                  -- RFC 3339, when the transition occurred
    from_status     TEXT    NOT NULL,
    to_status       TEXT    NOT NULL,
    details         TEXT    NOT NULL DEFAULT '{}',     -- JSON: reason, error codes, fill info, etc.
    source          TEXT    NOT NULL,                  -- 'user', 'ib', 'system'

    FOREIGN KEY (order_local_id) REFERENCES orders(local_id)
);

CREATE INDEX idx_order_audit_order    ON order_audit(order_local_id);
CREATE INDEX idx_order_audit_ts       ON order_audit(timestamp);
CREATE INDEX idx_order_audit_order_ts ON order_audit(order_local_id, timestamp);
```

**`details` JSON examples:**

```json
// IB rejection
{"ib_error_code": 201, "message": "Order rejected - reason: ..."}

// Partial fill
{"filled_qty": 50, "price": 152.30, "ib_exec_id": "0001f4e8.660a1b2c.01.01"}

// User modification
{"field": "limit_price", "old_value": "150.00", "new_value": "152.00"}

// System reconciliation
{"action": "reconcile", "ib_status": "Submitted", "local_status": "PendingSubmit"}
```

### 1c. `fills`

Individual execution records. One fill per partial or full execution event received from IB.

```sql
CREATE TABLE fills (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    order_local_id  TEXT    NOT NULL,
    ib_exec_id      TEXT    NOT NULL UNIQUE,           -- IB execution ID (globally unique)
    timestamp       TEXT    NOT NULL,                   -- execution time, RFC 3339
    shares          REAL    NOT NULL,                   -- quantity filled in this execution
    price           REAL    NOT NULL,                   -- fill price
    commission      REAL,                               -- commission for this fill (from commissionReport)
    exchange        TEXT,                               -- exchange where filled
    side            TEXT    NOT NULL,                   -- BUY, SELL (from execution, not order)
    account         TEXT,                               -- IB account ID
    realized_pnl    REAL,                               -- realized P&L from this fill, if available
    liquidity       INTEGER,                            -- IB liquidity indicator (1=added, 2=removed)

    FOREIGN KEY (order_local_id) REFERENCES orders(local_id)
);

CREATE INDEX idx_fills_order       ON fills(order_local_id);
CREATE INDEX idx_fills_timestamp   ON fills(timestamp);
CREATE UNIQUE INDEX idx_fills_exec ON fills(ib_exec_id);
```

**Design decisions:**

- `ib_exec_id` has a UNIQUE constraint to prevent duplicate fill recording. IB can re-deliver execution reports on reconnection; the insert should use `INSERT OR IGNORE` to handle this idempotently.
- `commission` may be null initially because IB delivers `execDetails` and `commissionReport` as separate callbacks. The commission is updated via a subsequent `UPDATE` when the commission report arrives.
- `realized_pnl` is included because IB provides it on the commission report for closing trades.

### 1d. `positions`

Current position snapshot. This table is a cache of the most recent position state from IB.
It is fully overwritten on each reconciliation cycle.

```sql
CREATE TABLE positions (
    -- Composite primary key: account + con_id uniquely identifies a position
    account         TEXT    NOT NULL,
    con_id          INTEGER NOT NULL,

    -- Contract info (denormalized for convenience)
    symbol          TEXT    NOT NULL,
    sec_type        TEXT    NOT NULL,
    exchange        TEXT,
    currency        TEXT    NOT NULL,

    -- Position data
    quantity        REAL    NOT NULL,                   -- signed: positive = long, negative = short
    avg_cost        REAL    NOT NULL,                   -- average cost per share (IB-reported)
    market_value    REAL,                               -- current market value (from portfolio update)
    unrealized_pnl  REAL,                               -- unrealized P&L (from portfolio update)
    realized_pnl    REAL,                               -- session realized P&L

    -- Metadata
    updated_at      TEXT    NOT NULL,

    PRIMARY KEY (account, con_id)
);

CREATE INDEX idx_positions_symbol ON positions(symbol);
```

**Design decisions:**

- Uses a composite primary key `(account, con_id)` rather than a synthetic ID because
  positions are identified by this pair in IB's model.
- Market value and unrealized P&L may be null if only position data (not portfolio data)
  has been received.
- On full reconciliation, this table is rebuilt via `DELETE + INSERT` inside a transaction,
  or via `INSERT OR REPLACE` for each position received from IB.

### 1e. `account_values`

Account data cache. IB delivers account values as tag/value pairs via `accountValues()`.

```sql
CREATE TABLE account_values (
    account         TEXT    NOT NULL,
    tag             TEXT    NOT NULL,                   -- e.g. "NetLiquidation", "BuyingPower", "TotalCashValue"
    value           TEXT    NOT NULL,                   -- stored as text (IB sends strings; some are numeric, some are not)
    currency        TEXT    NOT NULL DEFAULT '',        -- e.g. "USD", "EUR", or "" for dimensionless values
    updated_at      TEXT    NOT NULL,

    PRIMARY KEY (account, tag, currency)
);
```

**Design decisions:**

- `value` is stored as TEXT because IB account values include both numeric values
  (e.g., `"1234567.89"`) and non-numeric values (e.g., `"true"`, `"DayTrader"`).
  Parsing to the appropriate Rust type happens in the application layer.
- The composite primary key `(account, tag, currency)` matches IB's model where the same
  tag can appear multiple times with different currencies (e.g., `NetLiquidation` in USD and
  `NetLiquidation` in BASE).

### 1f. Historical Bar Cache (binary candle files)

> **No SQLite table.** Historical OHLCV bar data is stored in the binary `.candles`
> file format defined in `tech-stack-rust-a.md` and `04-market-data-and-events.md` §2.2.
> This format is designed for the GPU charting pipeline and avoids the overhead of
> row-per-bar SQL storage.
>
> The TTL / invalidation rules described in §4 ("Market Data Cache TTL") apply to
> the binary file metadata (file modification timestamps or a sidecar metadata file),
> not to SQLite rows.

### 1g. `contracts`

Cached contract definitions. Qualification results from IB (`reqContractDetails`) are
expensive (rate-limited, requires round-trip). Cache them locally.

```sql
CREATE TABLE contracts (
    con_id              INTEGER PRIMARY KEY,             -- IB's unique contract ID
    symbol              TEXT    NOT NULL,
    sec_type            TEXT    NOT NULL,                 -- STK, OPT, FUT, CASH, etc.
    exchange            TEXT    NOT NULL,
    currency            TEXT    NOT NULL,
    primary_exchange    TEXT,                             -- e.g. "NASDAQ", "NYSE" for stocks
    local_symbol        TEXT,                             -- exchange-specific symbol
    trading_class       TEXT,                             -- IB trading class
    multiplier          TEXT,                             -- contract multiplier (options: "100")
    last_trade_date     TEXT,                             -- for options/futures: expiry
    strike              REAL,                             -- for options: strike price
    right               TEXT,                             -- for options: "C" or "P"
    details_json        TEXT    NOT NULL DEFAULT '{}',    -- full ContractDetails as JSON blob
    cached_at           TEXT    NOT NULL
);

CREATE INDEX idx_contracts_symbol     ON contracts(symbol);
CREATE INDEX idx_contracts_sym_sec    ON contracts(symbol, sec_type);
CREATE INDEX idx_contracts_cached_at  ON contracts(cached_at);
```

**Design decisions:**

- `con_id` is the primary key because it is globally unique within IB's system and is the
  canonical way to refer to a contract.
- The most commonly needed fields are broken out as columns for efficient querying. The
  full `ContractDetails` object (which includes trading hours, order types, min tick, etc.)
  is serialized to `details_json` for cases where the full blob is needed.
- `cached_at` supports TTL-based invalidation. Contract definitions rarely change, but
  options chains roll, so expiry-based contracts should be refreshed periodically.

---

## 2. Rust Types

### Key Structs

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Primary order representation in midas-broker.
/// Every field maps directly to a column in the `orders` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalOrder {
    pub local_id: Uuid,               // UUIDv7
    pub ib_order_id: Option<i32>,
    pub ib_perm_id: Option<i64>,

    pub status: OrderStatus,

    // Contract
    pub symbol: String,
    pub sec_type: SecType,
    pub exchange: String,
    pub currency: String,
    pub con_id: Option<i32>,

    // Order params
    pub action: Action,
    pub order_type: OrderType,
    pub quantity: f64,
    pub filled_qty: f64,
    pub remaining_qty: f64,

    // Prices
    pub limit_price: Option<f64>,
    pub stop_price: Option<f64>,
    pub trail_amount: Option<f64>,
    pub trail_percent: Option<f64>,

    // Time in force
    pub tif: TimeInForce,

    // Linkage
    pub parent_id: Option<Uuid>,
    pub oca_group: Option<String>,

    // Tags
    pub tags: Vec<String>,

    // Algo
    pub algo_strategy: Option<String>,
    pub algo_params: Option<serde_json::Value>,

    // Extended hours / scheduling
    pub outside_rth: bool,
    pub good_after_time: Option<DateTime<Utc>>,
    pub good_till_date: Option<DateTime<Utc>>,

    // Fill info
    pub avg_fill_price: Option<f64>,
    pub last_fill_price: Option<f64>,
    pub commission: Option<f64>,

    // Timestamps
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub submitted_at: Option<DateTime<Utc>>,
    pub filled_at: Option<DateTime<Utc>>,
    pub cancelled_at: Option<DateTime<Utc>>,
}

/// Immutable audit log entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderAuditEntry {
    pub id: i64,
    pub order_local_id: Uuid,
    pub timestamp: DateTime<Utc>,
    pub from_status: OrderStatus,
    pub to_status: OrderStatus,
    pub details: serde_json::Value,
    pub source: AuditSource,
}

/// A single execution event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fill {
    pub id: i64,
    pub order_local_id: Uuid,
    pub ib_exec_id: String,
    pub timestamp: DateTime<Utc>,
    pub shares: f64,
    pub price: f64,
    pub commission: Option<f64>,
    pub exchange: Option<String>,
    pub side: Action,
    pub account: Option<String>,
    pub realized_pnl: Option<f64>,
    pub liquidity: Option<i32>,
}

/// Current position state (mirrors IB's position model).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Position {
    pub account: String,
    pub con_id: i32,
    pub symbol: String,
    pub sec_type: SecType,
    pub exchange: Option<String>,
    pub currency: String,
    pub quantity: f64,
    pub avg_cost: f64,
    pub market_value: Option<f64>,
    pub unrealized_pnl: Option<f64>,
    pub realized_pnl: Option<f64>,
    pub updated_at: DateTime<Utc>,
}

/// A single account value tag.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountValue {
    pub account: String,
    pub tag: String,
    pub value: String,
    pub currency: String,
    pub updated_at: DateTime<Utc>,
}

/// A cached OHLCV bar.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedBar {
    pub symbol: String,
    pub bar_size: String,
    pub timestamp: DateTime<Utc>,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
    pub bar_count: Option<i32>,
    pub avg_price: Option<f64>,
    pub cached_at: DateTime<Utc>,
}

/// Cached contract definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedContract {
    pub con_id: i32,
    pub symbol: String,
    pub sec_type: SecType,
    pub exchange: String,
    pub currency: String,
    pub primary_exchange: Option<String>,
    pub local_symbol: Option<String>,
    pub trading_class: Option<String>,
    pub multiplier: Option<String>,
    pub last_trade_date: Option<String>,
    pub strike: Option<f64>,
    pub right: Option<String>,
    pub details_json: serde_json::Value,
    pub cached_at: DateTime<Utc>,
}
```

### Enums and Storage Strategy

Enums are stored as `TEXT` in SQLite and converted via `Display`/`FromStr` implementations.
This makes the database human-readable and debuggable. The performance cost of string
comparison vs integer comparison is negligible for the query volumes involved.

```rust
use std::fmt;
use std::str::FromStr;

/// Order status enum. Variants match the string values stored in SQLite.
///
/// The IB API uses different names in some cases (e.g., "ApiPending", "ApiCancelled",
/// "Inactive"). Our mapping layer normalizes these:
///   IB "ApiPending"   -> PendingSubmit (order not yet acknowledged by IB server)
///   IB "ApiCancelled" -> Cancelled
///   IB "Inactive"     -> Rejected (NOT our Inactive — see 02-order-management.md §1.5)
///
/// NOTE: The canonical OrderStatus definition and IB mapping live in
/// 02-order-management.md Section 1.4-1.5. This file mirrors that definition
/// for the persistence layer. If they diverge, 02-order-management.md wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OrderStatus {
    Draft,
    Inactive,
    PendingSubmit,
    PreSubmitted,
    Submitted,
    PartiallyFilled,
    Filled,
    PendingCancel,
    Cancelled,
    Rejected,       // IB rejected the order (terminal — cannot retry)
    Error,          // Local/internal error (non-terminal — user can fix and retry)
}

impl fmt::Display for OrderStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Draft => "Draft",
            Self::Inactive => "Inactive",
            Self::PendingSubmit => "PendingSubmit",
            Self::PreSubmitted => "PreSubmitted",
            Self::Submitted => "Submitted",
            Self::PartiallyFilled => "PartiallyFilled",
            Self::Filled => "Filled",
            Self::PendingCancel => "PendingCancel",
            Self::Cancelled => "Cancelled",
            Self::Rejected => "Rejected",
            Self::Error => "Error",
        };
        write!(f, "{s}")
    }
}

impl FromStr for OrderStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Draft" => Ok(Self::Draft),
            "Inactive" => Ok(Self::Inactive),
            "PendingSubmit" => Ok(Self::PendingSubmit),
            "PreSubmitted" => Ok(Self::PreSubmitted),
            "Submitted" => Ok(Self::Submitted),
            "PartiallyFilled" => Ok(Self::PartiallyFilled),
            "Filled" => Ok(Self::Filled),
            "PendingCancel" => Ok(Self::PendingCancel),
            "Cancelled" => Ok(Self::Cancelled),
            "Rejected" => Ok(Self::Rejected),
            "Error" => Ok(Self::Error),
            other => Err(format!("unknown OrderStatus: {other}")),
        }
    }
}

/// IB status string -> our OrderStatus.
/// Called when processing orderStatus callbacks.
///
/// **CRITICAL**: IB's "Inactive" means rejected/error/conditions-not-met.
/// It must map to `Rejected`, NOT to our `Inactive` (which means locally parked).
/// See 02-order-management.md Section 1.5 for the canonical mapping.
impl OrderStatus {
    pub fn from_ib_status(ib_status: &str) -> Self {
        match ib_status {
            "ApiPending" => Self::PendingSubmit,
            "PendingSubmit" => Self::PendingSubmit,
            "PreSubmitted" => Self::PreSubmitted,
            "Submitted" => Self::Submitted,
            "Filled" => Self::Filled,
            "PendingCancel" => Self::PendingCancel,
            "Cancelled" | "ApiCancelled" => Self::Cancelled,
            "Inactive" => Self::Rejected,
            _ => Self::Rejected, // Unknown states treated as Rejected for safety
        }
    }
}

/// Order action: buy or sell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Action {
    Buy,
    Sell,
}
// Display: "BUY" / "SELL"
// FromStr: "BUY" | "Buy" | "buy" -> Buy, etc.

/// Order type.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OrderType {
    Market,         // MKT
    Limit,          // LMT
    Stop,           // STP
    StopLimit,      // STP_LMT
    Trail,          // TRAIL
    TrailLimit,     // TRAIL_LIMIT
    MarketIfTouched,// MIT
    LimitIfTouched, // LIT
    MarketToLimit,  // MTL
    MarketOnClose,  // MOC
    LimitOnClose,   // LOC
    Relative,       // REL
    Volatility,     // VOL
    Vwap,           // VWAP
}
// Display/FromStr use the IB short codes: "MKT", "LMT", "STP", "STP_LMT", etc.
// This keeps the DB values identical to what IB expects and returns.

/// Time in force.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TimeInForce {
    Day,    // DAY
    Gtc,    // GTC
    Ioc,    // IOC
    Fok,    // FOK
    Opg,    // OPG
    Gtd,    // GTD
}
// Display: "DAY", "GTC", "IOC", "FOK", "OPG", "GTD"

/// Security type.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SecType {
    Stock,      // STK
    Option,     // OPT
    Future,     // FUT
    Forex,      // CASH
    Index,      // IND
    Bond,       // BOND
    Commodity,  // CMDTY
    Crypto,     // CRYPTO
}
// Display: "STK", "OPT", "FUT", "CASH", "IND", "BOND", "CMDTY", "CRYPTO"

/// Source of an audit event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AuditSource {
    User,    // user-initiated action (place, cancel, modify)
    Ib,      // callback from IB (status change, fill, rejection)
    System,  // internal system action (reconciliation, startup recovery)
}
// Display: "user", "ib", "system"
```

**Why `Display`/`FromStr` instead of integer codes?**

1. SQLite queries in development/debugging are readable (`WHERE status = 'Submitted'`).
2. The enum variant count is small (< 15); string comparison is not a bottleneck.
3. Adding new variants does not require schema migration (no integer mapping table to update).
4. The string representation is the same as IB's wire format, reducing conversion layers.

### Serde for JSON Fields

JSON fields (`tags`, `algo_params`, `details`, `details_json`) use `serde_json::Value` for
flexible schema. For `tags`, we use `Vec<String>` and serialize to/from JSON text:

```rust
use rusqlite::types::{FromSql, FromSqlResult, ToSql, ToSqlOutput, ValueRef};

/// Helper to store Vec<String> as a JSON array in SQLite.
pub struct JsonVec(pub Vec<String>);

impl ToSql for JsonVec {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        let json = serde_json::to_string(&self.0)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        Ok(ToSqlOutput::from(json))
    }
}

impl FromSql for JsonVec {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let text = value.as_str()?;
        let vec: Vec<String> = serde_json::from_str(text)
            .map_err(|e| rusqlite::types::FromSqlError::Other(Box::new(e)))?;
        Ok(JsonVec(vec))
    }
}

/// Helper to store serde_json::Value as TEXT in SQLite.
pub struct JsonValue(pub serde_json::Value);

impl ToSql for JsonValue {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        let json = serde_json::to_string(&self.0)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        Ok(ToSqlOutput::from(json))
    }
}

impl FromSql for JsonValue {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let text = value.as_str()?;
        let val: serde_json::Value = serde_json::from_str(text)
            .map_err(|e| rusqlite::types::FromSqlError::Other(Box::new(e)))?;
        Ok(JsonValue(val))
    }
}
```

---

## 3. Database Access Pattern

### Connection Management

Use `rusqlite` directly with `Arc<Mutex<Connection>>`. A full connection pool
(`r2d2-sqlite`) is unnecessary because:

1. SQLite with WAL mode supports only one writer at a time regardless of pool size.
2. The broker crate has a single event loop processing IB callbacks sequentially.
3. Read concurrency (from the UI thread) is handled by WAL mode's lock-free reads.

```rust
use std::sync::{Arc, Mutex};
use rusqlite::Connection;

pub struct BrokerDb {
    conn: Arc<Mutex<Connection>>,
}

impl BrokerDb {
    pub fn open(path: &std::path::Path) -> Result<Self, BrokerDbError> {
        let conn = Connection::open(path)?;

        // Enable WAL mode for concurrent reads
        conn.pragma_update(None, "journal_mode", "WAL")?;

        // Performance pragmas
        conn.pragma_update(None, "synchronous", "NORMAL")?;     // Safe with WAL
        conn.pragma_update(None, "cache_size", -8000)?;          // 8 MB page cache
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;         // 5 second busy timeout

        let db = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        db.run_migrations()?;
        Ok(db)
    }
}
```

**If read concurrency becomes a bottleneck** (e.g., UI polling frequently while IB
callbacks are writing), upgrade to a two-connection model:

```rust
pub struct BrokerDb {
    writer: Mutex<Connection>,   // exclusive writer
    reader: Connection,          // WAL allows concurrent reads
}
```

This avoids the Mutex contention for pure reads without introducing a full pool.

### WAL Mode

WAL (Write-Ahead Logging) is critical for our use case:

- **IB callback thread** writes order updates, fills, position snapshots.
- **UI thread** reads current order state, positions, account values.
- WAL allows readers and the writer to operate concurrently without blocking.

WAL is set once on connection open. It persists in the database file and does not need to
be re-enabled on subsequent connections.

The `synchronous = NORMAL` pragma is safe with WAL mode. It means the WAL file is synced
at each checkpoint but not on every commit. In the worst case (power loss), the last few
transactions may be lost, but the database will not be corrupted. This is acceptable
because the true source of truth is the IB server; lost local state is recoverable via
reconciliation.

### Prepared Statements

Hot-path queries are prepared once and reused. The hottest paths are:

1. **Update order status** (every IB callback):
   ```sql
   UPDATE orders
   SET status = ?1, filled_qty = ?2, remaining_qty = ?3,
       avg_fill_price = ?4, last_fill_price = ?5, updated_at = ?6
   WHERE local_id = ?7
   ```

2. **Insert audit entry** (every status change):
   ```sql
   INSERT INTO order_audit (order_local_id, timestamp, from_status, to_status, details, source)
   VALUES (?1, ?2, ?3, ?4, ?5, ?6)
   ```

3. **Insert fill** (every execution):
   ```sql
   INSERT OR IGNORE INTO fills
       (order_local_id, ib_exec_id, timestamp, shares, price, commission, exchange, side, account, realized_pnl, liquidity)
   VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
   ```

4. **Read active orders** (UI polling):
   ```sql
   SELECT * FROM orders
   WHERE status NOT IN ('Draft', 'Filled', 'Cancelled', 'Rejected')
   ORDER BY created_at DESC
   ```

5. **Upsert position** (portfolio updates):
   ```sql
   INSERT OR REPLACE INTO positions
       (account, con_id, symbol, sec_type, exchange, currency, quantity, avg_cost,
        market_value, unrealized_pnl, realized_pnl, updated_at)
   VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
   ```

In `rusqlite`, prepared statements are created via `conn.prepare_cached()` which
maintains an internal LRU cache of compiled statements. This is preferred over manual
statement management.

### Migrations

Migrations are embedded in the binary using `include_str!` and executed on startup.
A simple version-tracking approach:

```sql
-- Tracked automatically via PRAGMA user_version
-- Migration 1: initial schema
-- Migration 2: add column X
-- etc.
```

```rust
const MIGRATIONS: &[&str] = &[
    include_str!("../migrations/001_initial.sql"),
    // include_str!("../migrations/002_add_whatever.sql"),
];

impl BrokerDb {
    fn run_migrations(&self) -> Result<(), BrokerDbError> {
        let conn = self.conn.lock().unwrap();
        let current_version: i32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;

        for (i, migration) in MIGRATIONS.iter().enumerate() {
            let version = (i + 1) as i32;
            if version > current_version {
                let tx = conn.unchecked_transaction()?;
                tx.execute_batch(migration)?;
                tx.pragma_update(None, "user_version", version)?;
                tx.commit()?;
                tracing::info!("Applied migration {version}");
            }
        }
        Ok(())
    }
}
```

**Rollback strategy:** For v1, the pragmatic rollback approach is: backup before migration, restore on failure. The `backup_database()` function (Section 5) should be called before `run_migrations()`.

**Why not `refinery` or `diesel` migrations?**

- The broker crate targets a single embedded SQLite file. The migration set is small
  and known at compile time. A full migration framework adds dependency weight without
  proportional benefit.
- `PRAGMA user_version` is an atomic, zero-overhead version counter built into SQLite.

### Transaction Boundaries

Use explicit transactions for operations that must be atomic:

| Operation | Transaction Required? | Why |
|---|---|---|
| Insert new order | Yes | `orders` INSERT + `order_audit` INSERT must both succeed |
| Update order status | Yes | `orders` UPDATE + `order_audit` INSERT are a pair |
| Record fill | Yes | `fills` INSERT + `orders` UPDATE (filled_qty, remaining_qty) + `order_audit` INSERT |
| Position reconciliation | Yes | `DELETE` all + `INSERT` all must be atomic |
| Account values refresh | Yes | Same: bulk replace must be atomic |
| Market data bar insert | No | Individual bar inserts are idempotent (`INSERT OR REPLACE`) |
| Contract cache write | No | Individual upserts are idempotent |

Transaction pattern:

```rust
impl BrokerDb {
    pub fn record_fill(&self, fill: &Fill, order_update: &OrderUpdate) -> Result<(), BrokerDbError> {
        let conn = self.conn.lock().unwrap();
        let tx = conn.unchecked_transaction()?;

        // 1. Insert fill (idempotent via ib_exec_id UNIQUE)
        tx.execute(
            "INSERT OR IGNORE INTO fills (...) VALUES (...)",
            params![...],
        )?;

        // 2. Update order quantities and price
        tx.execute(
            "UPDATE orders SET filled_qty = ?1, remaining_qty = ?2, ... WHERE local_id = ?3",
            params![...],
        )?;

        // 3. Audit trail
        tx.execute(
            "INSERT INTO order_audit (...) VALUES (...)",
            params![...],
        )?;

        tx.commit()?;
        Ok(())
    }
}
```

**`unchecked_transaction()`** is used instead of `transaction()` because we already hold
the `Mutex` lock, and `rusqlite`'s checked transaction would try to enforce borrow rules
that conflict with the `MutexGuard` lifetime. This is safe because the `Mutex` guarantees
single-writer access.

---

## 4. Data Sync Strategy

### Startup Reconciliation

On startup, the broker must reconcile local SQLite state with the live IB state. This
handles scenarios like: broker crashed, IB filled an order while we were offline, manual
TWS trades.

**Sequence:**

```
1. Open SQLite database, run migrations.
2. Load all orders with status NOT IN ('Filled', 'Cancelled', 'Rejected') from SQLite.
   These are "locally active" orders.
3. Connect to IB Gateway.
4. Wait for connection confirmation (error codes 2104/2106 — data farms OK).
5. Request open orders from IB:  reqAllOpenOrders() / reqOpenOrders()
6. Request executions since last known fill:  reqExecutions(filter)
7. Request current positions:  reqPositions()
8. Request account values:  reqAccountUpdates()

Reconciliation logic:

FOR EACH locally-active order:
    IF found in IB open orders (match by ib_perm_id or ib_order_id):
        Update local status to match IB status.
        Log audit entry with source = "system".
    ELSE IF found in IB executions (fully filled while offline):
        Update local status to Filled.
        Insert fill records.
        Log audit entry.
    ELSE:
        Mark as Cancelled with audit note "not found on IB after reconnect".
        (IB may have cancelled it during the disconnect.)

FOR EACH IB open order NOT in local database:
    This is an "orphan" — placed via TWS manually or by another clientId.
    Option A: Import it with a new local_id and status matching IB.
    Option B: Ignore it (only track orders placed by this broker instance).
    Decision: Option B for v1. Log a warning. Users use the broker crate
    exclusively for order management, not as a sync-everything tool.

FOR EACH position from IB:
    INSERT OR REPLACE into positions table.

FOR EACH account value from IB:
    INSERT OR REPLACE into account_values table.
```

### Runtime Updates

During normal operation, IB delivers callbacks asynchronously. Each callback type maps to
a database operation:

| IB Callback | DB Operation |
|---|---|
| `orderStatus(orderId, status, filled, remaining, avgFillPrice, ...)` | Update `orders` row, insert `order_audit` entry. If `status` differs from current local status, this is a state transition. |
| `execDetails(reqId, contract, execution)` | Insert into `fills` (using `INSERT OR IGNORE` on `ib_exec_id`). Update `orders.filled_qty` and `orders.remaining_qty`. |
| `commissionReport(report)` | Update `fills.commission` where `ib_exec_id = report.execId`. Update `orders.commission` (sum of all fills' commissions). |
| `position(account, contract, pos, avgCost)` | `INSERT OR REPLACE` into `positions`. |
| `accountValue(tag, value, currency, account)` | `INSERT OR REPLACE` into `account_values`. |
| `openOrder(orderId, contract, order, orderState)` | Update `orders` if we track this order. May set `ib_perm_id` if it was null. |

**Callback processing is single-threaded.** The IB client runs on a dedicated tokio task,
and all database writes happen on that task. This eliminates the need for complex
concurrency control beyond the `Mutex<Connection>`.

**Status transition validation:**

Before applying a status change, validate it against the allowed transition graph:

```
Draft -> PendingSubmit, Inactive, Cancelled
PendingSubmit -> PreSubmitted, Submitted, Inactive, Cancelled, Rejected
PreSubmitted -> Submitted, Filled, Cancelled, Inactive
Submitted -> PartiallyFilled, Filled, PendingCancel, Cancelled
PartiallyFilled -> Filled, PendingCancel, Cancelled
PendingCancel -> Cancelled, Filled (race condition: fill arrived before cancel confirmed)
Inactive -> PendingSubmit, Cancelled (reactivation or user cancels)
```

If IB reports a transition not in this graph (e.g., `Filled -> Submitted`), log a warning
and accept IB's status as authoritative. IB is the source of truth for order state.

### Periodic Full Reconciliation

Every N minutes (configurable, default: 5 minutes), perform a lightweight reconciliation:

```
1. reqAllOpenOrders()  -- compare with locally-active orders
2. reqPositions()      -- full position snapshot refresh
3. reqAccountUpdates() -- account values refresh
```

This catches any callbacks that were dropped due to network issues. The reconciliation
runs as a background tokio task on a `tokio::time::interval`.

**Reconciliation is idempotent.** All writes use `INSERT OR REPLACE` or conditional
updates (`UPDATE ... WHERE local_id = ? AND status != 'Filled'`). Running it multiple
times produces the same result.

### Market Data Cache TTL

The binary candle file cache (see §1f) uses TTL-based invalidation. Files older than the
TTL are considered stale and will be re-fetched from IB on next request. Staleness is
determined by the file's modification timestamp (or a sidecar metadata file).

| Bar Size | Default TTL | Rationale |
|---|---|---|
| `1 day`, `1 week`, `1 month` | 7 days | Daily bars rarely change after market close. Weekend refresh is sufficient. |
| `1 hour`, `4 hours` | 24 hours | Intraday bars are final after the trading day. |
| `5 mins`, `15 mins`, `30 mins` | 6 hours | Short intraday bars: re-fetch during the trading day for accuracy. |
| `1 min` | 2 hours | Minute bars: keep fresh during active trading. |
| `1 secs` - `30 secs` | 30 minutes | Sub-minute bars: always near-fresh. Primarily for recent history. |

TTL cleanup runs as a periodic task (e.g., every 30 minutes). The cleanup is done in Rust
by checking file modification timestamps per bar size and deleting stale `.candles` files.

Contract cache TTL:

| Contract Type | TTL | Rationale |
|---|---|---|
| Stocks (STK) | 30 days | Symbol mappings rarely change. |
| Options (OPT) | 1 day | Options chains roll; strike/expiry info changes. |
| Futures (FUT) | 7 days | Front-month rolls periodically. |
| Forex (CASH) | 30 days | Stable. |

---

## 5. Backup and Recovery

### File Location

The SQLite database file lives in a platform-appropriate data directory:

| Platform | Path |
|---|---|
| Windows | `%LOCALAPPDATA%\HandOfMidas\broker\midas-broker.db` |
| macOS | `~/Library/Application Support/HandOfMidas/broker/midas-broker.db` |
| Linux | `~/.local/share/HandOfMidas/broker/midas-broker.db` |

Resolved at runtime using the `dirs` crate:

```rust
use dirs::data_local_dir;

fn db_path() -> PathBuf {
    let base = data_local_dir()
        .expect("could not determine local data directory");
    let dir = base.join("HandOfMidas").join("broker");
    std::fs::create_dir_all(&dir).expect("could not create database directory");
    dir.join("midas-broker.db")
}
```

**Why not next to the .exe?**

- On Windows, the Program Files directory is read-only without elevation.
- AppData/Local is the standard location for application-specific mutable data.
- The `dirs` crate handles platform differences.

The WAL file (`midas-broker.db-wal`) and shared memory file (`midas-broker.db-shm`) will
be created alongside the main database file automatically by SQLite.

### WAL Checkpointing

SQLite's WAL file grows as writes accumulate. Checkpointing merges WAL contents back into
the main database file.

**Automatic checkpointing:** SQLite auto-checkpoints when the WAL reaches 1000 pages
(~4 MB with default page size). This is sufficient for normal operation.

**Manual checkpointing on graceful shutdown:**

```rust
impl BrokerDb {
    pub fn shutdown(&self) -> Result<(), BrokerDbError> {
        let conn = self.conn.lock().unwrap();
        // TRUNCATE mode: checkpoint and delete the WAL file
        conn.pragma_update(None, "wal_checkpoint", "TRUNCATE")?;
        Ok(())
    }
}
```

This ensures the database is in a clean single-file state after shutdown, which simplifies
backup and file copying.

**Periodic passive checkpoint** (every 10 minutes during operation):

```rust
// PASSIVE mode: does not block writers, checkpoints what it can
conn.pragma_update(None, "wal_checkpoint", "PASSIVE")?;
```

### Corruption Recovery

SQLite databases can become corrupted due to hardware failure, filesystem bugs, or
ungraceful process termination (though WAL mode makes this very rare).

**Detection:**

On startup, run an integrity check:

```sql
PRAGMA quick_check;
```

(`quick_check` is faster than `integrity_check` and catches most corruption. Full
`integrity_check` can be offered as a manual diagnostic command.)

**Recovery strategy:**

If the database is corrupt:

```
1. Log an error with full details.
2. Rename the corrupt file to midas-broker.db.corrupt.<timestamp>
3. Create a fresh database with the current schema.
4. Attempt to recover what we can:
   a. Try opening the corrupt file in read-only mode.
   b. If readable, copy contracts and market data cache (non-critical, but saves API calls).
   c. Skip orders/fills/positions — these will be rebuilt from IB.
5. On next IB connection, full reconciliation rebuilds:
   - Open orders from reqAllOpenOrders()
   - Recent executions from reqExecutions()
   - Current positions from reqPositions()
   - Account values from reqAccountUpdates()
6. Historical orders/fills before the corruption are lost locally.
   They remain available in IB's Flex Query / Activity Statements.
```

**Why this is acceptable:**

The SQLite database is a **local cache and audit trail**, not the source of truth. IB's
servers are the authoritative record for:
- Order state (open, filled, cancelled)
- Executions and commissions
- Positions and account values

The only data truly local to us is:
- `local_id` (UUID mappings) — these can be regenerated for active orders
- `tags` (user-assigned labels) — lost on corruption; this is the only non-recoverable data
- `order_audit` history — lost on corruption; operational but not critical

For users who need audit history durability, a future enhancement could write audit entries
to a separate append-only log file (plain text or JSON lines) in addition to SQLite. This
provides a secondary recovery source at negligible cost.

---

## Appendix A: Entity Relationship Diagram

```
+------------------+       1:N        +------------------+
|     orders       |<-----------------| order_audit      |
|                  |                  |                  |
| local_id (PK)   |                  | order_local_id   |
| ib_order_id      |                  | from_status      |
| ib_perm_id       |    1:N           | to_status        |
| status           |<---------+       | details (JSON)   |
| symbol           |          |       | source           |
| action           |          |       +------------------+
| order_type       |          |
| quantity         |          |       +------------------+
| parent_id (FK)---+--self    +------>| fills            |
| tags (JSON)      |                  |                  |
| ...              |                  | order_local_id   |
+------------------+                  | ib_exec_id (UQ)  |
        |                             | shares           |
        | con_id                      | price            |
        v                             | commission       |
+------------------+                  +------------------+
| contracts        |
|                  |
| con_id (PK)      |       +------------------+
| symbol           |       | positions        |
| sec_type         |       |                  |
| details_json     |       | account + con_id |
+------------------+       | quantity         |
                           | avg_cost         |
                           +------------------+

+------------------+
| account_values   |
|                  |
| account+tag+ccy  |       (Historical bar cache uses binary .candles files,
| value            |        not SQLite — see §1f)
+------------------+
```

## Appendix B: Full Initial Migration SQL

This is the complete `001_initial.sql` file that creates all tables and indices:

```sql
-- midas-broker: initial schema
-- Applied via PRAGMA user_version tracking

-- ===========================================================================
-- orders
-- ===========================================================================
CREATE TABLE IF NOT EXISTS orders (
    local_id        TEXT    NOT NULL PRIMARY KEY,
    ib_order_id     INTEGER,
    ib_perm_id      INTEGER,
    status          TEXT    NOT NULL DEFAULT 'Draft',
    symbol          TEXT    NOT NULL,
    sec_type        TEXT    NOT NULL DEFAULT 'STK',
    exchange        TEXT    NOT NULL DEFAULT 'SMART',
    currency        TEXT    NOT NULL DEFAULT 'USD',
    con_id          INTEGER,
    action          TEXT    NOT NULL,
    order_type      TEXT    NOT NULL,
    quantity        REAL    NOT NULL,
    filled_qty      REAL    NOT NULL DEFAULT 0.0,
    remaining_qty   REAL    NOT NULL,
    limit_price     REAL,
    stop_price      REAL,
    trail_amount    REAL,
    trail_percent   REAL,
    tif             TEXT    NOT NULL DEFAULT 'DAY',
    parent_id       TEXT    REFERENCES orders(local_id),
    oca_group       TEXT,
    tags            TEXT    NOT NULL DEFAULT '[]',
    algo_strategy   TEXT,
    algo_params     TEXT,
    outside_rth     INTEGER NOT NULL DEFAULT 0,
    good_after_time TEXT,
    good_till_date  TEXT,
    avg_fill_price  REAL,
    last_fill_price REAL,
    commission      REAL,
    created_at      TEXT    NOT NULL,
    updated_at      TEXT    NOT NULL,
    submitted_at    TEXT,
    filled_at       TEXT,
    cancelled_at    TEXT
);

CREATE INDEX IF NOT EXISTS idx_orders_status        ON orders(status);
CREATE INDEX IF NOT EXISTS idx_orders_symbol        ON orders(symbol);
CREATE INDEX IF NOT EXISTS idx_orders_ib_order_id   ON orders(ib_order_id);
CREATE INDEX IF NOT EXISTS idx_orders_ib_perm_id    ON orders(ib_perm_id);
CREATE INDEX IF NOT EXISTS idx_orders_parent_id     ON orders(parent_id);
CREATE INDEX IF NOT EXISTS idx_orders_oca_group     ON orders(oca_group);
CREATE INDEX IF NOT EXISTS idx_orders_created_at    ON orders(created_at);
CREATE INDEX IF NOT EXISTS idx_orders_status_symbol ON orders(status, symbol);

-- ===========================================================================
-- order_audit
-- ===========================================================================
CREATE TABLE IF NOT EXISTS order_audit (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    order_local_id  TEXT    NOT NULL REFERENCES orders(local_id),
    timestamp       TEXT    NOT NULL,
    from_status     TEXT    NOT NULL,
    to_status       TEXT    NOT NULL,
    details         TEXT    NOT NULL DEFAULT '{}',
    source          TEXT    NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_order_audit_order    ON order_audit(order_local_id);
CREATE INDEX IF NOT EXISTS idx_order_audit_ts       ON order_audit(timestamp);
CREATE INDEX IF NOT EXISTS idx_order_audit_order_ts ON order_audit(order_local_id, timestamp);

-- ===========================================================================
-- fills
-- ===========================================================================
CREATE TABLE IF NOT EXISTS fills (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    order_local_id  TEXT    NOT NULL REFERENCES orders(local_id),
    ib_exec_id      TEXT    NOT NULL UNIQUE,
    timestamp       TEXT    NOT NULL,
    shares          REAL    NOT NULL,
    price           REAL    NOT NULL,
    commission      REAL,
    exchange        TEXT,
    side            TEXT    NOT NULL,
    account         TEXT,
    realized_pnl    REAL,
    liquidity       INTEGER
);

CREATE INDEX IF NOT EXISTS idx_fills_order     ON fills(order_local_id);
CREATE INDEX IF NOT EXISTS idx_fills_timestamp ON fills(timestamp);

-- ===========================================================================
-- positions
-- ===========================================================================
CREATE TABLE IF NOT EXISTS positions (
    account         TEXT    NOT NULL,
    con_id          INTEGER NOT NULL,
    symbol          TEXT    NOT NULL,
    sec_type        TEXT    NOT NULL,
    exchange        TEXT,
    currency        TEXT    NOT NULL,
    quantity        REAL    NOT NULL,
    avg_cost        REAL    NOT NULL,
    market_value    REAL,
    unrealized_pnl  REAL,
    realized_pnl    REAL,
    updated_at      TEXT    NOT NULL,
    PRIMARY KEY (account, con_id)
);

CREATE INDEX IF NOT EXISTS idx_positions_symbol ON positions(symbol);

-- ===========================================================================
-- account_values
-- ===========================================================================
CREATE TABLE IF NOT EXISTS account_values (
    account         TEXT    NOT NULL,
    tag             TEXT    NOT NULL,
    value           TEXT    NOT NULL,
    currency        TEXT    NOT NULL DEFAULT '',
    updated_at      TEXT    NOT NULL,
    PRIMARY KEY (account, tag, currency)
);

-- ===========================================================================
-- NOTE: Historical bar cache uses binary .candles files, not SQLite.
-- See tech-stack-rust-a.md and 04-market-data-and-events.md §2.2.
-- ===========================================================================

-- ===========================================================================
-- contracts
-- ===========================================================================
CREATE TABLE IF NOT EXISTS contracts (
    con_id              INTEGER PRIMARY KEY,
    symbol              TEXT    NOT NULL,
    sec_type            TEXT    NOT NULL,
    exchange            TEXT    NOT NULL,
    currency            TEXT    NOT NULL,
    primary_exchange    TEXT,
    local_symbol        TEXT,
    trading_class       TEXT,
    multiplier          TEXT,
    last_trade_date     TEXT,
    strike              REAL,
    right               TEXT,
    details_json        TEXT    NOT NULL DEFAULT '{}',
    cached_at           TEXT    NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_contracts_symbol    ON contracts(symbol);
CREATE INDEX IF NOT EXISTS idx_contracts_sym_sec   ON contracts(symbol, sec_type);
CREATE INDEX IF NOT EXISTS idx_contracts_cached_at ON contracts(cached_at);
```

## Appendix C: Crate Dependencies

```toml
[dependencies]
rusqlite = { version = "0.32", features = ["bundled"] }
uuid = { version = "1", features = ["v7", "serde"] }
chrono = { version = "0.4", features = ["serde"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
tracing = "0.1"
dirs = "5"
```

- `rusqlite` with `bundled` feature compiles SQLite from source, ensuring a consistent
  version across all platforms and avoiding system library version mismatches.
- `uuid` v7 feature provides time-sortable UUIDs for `local_id`.
- `dirs` provides cross-platform data directory resolution.
