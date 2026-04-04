# 06 — Orders & Positions Query API

> Make broker order/position/account querying congruent across all layers
> so the UI can consume everything on day one.

## Problem

The broker engine can *place* orders and *push* events, but querying is
incomplete and inconsistent:

| Gap | Details |
|-----|---------|
| **PositionRecord too thin** | Missing account, con_id, sec_type, market_value, unrealized_pnl |
| **AccountSummary too thin** | Missing net_liquidation, buying_power, margin, daily_pnl |
| **No open_orders on BrokerClient** | No way to ask IB "what orders are live right now?" |
| **OrderSnapshotEntry too thin** | Missing order_type, prices, bracket_role, parent_id, tif, timestamps |
| **No batch snapshot events** | Positions arrive as individual events with no end marker |
| **Persistence missing queries** | No get_all_orders, get_brackets, get_recent_orders |
| **Desktop OrderBroker has no queries** | Command-only trait — UI can't pull on demand |
| **RequestOrderSnapshot broken** | Emits individual OrderStatusChanged, not the OrderSnapshot variant |

## Design Principles

1. **Commands in, events out.** Query methods send a command; results return as events.
2. **Snapshot-first.** New queries return atomic batch events, not sprayed individuals.
3. **Backward compatible.** Existing commands/events keep working; new ones added alongside.
4. **Mirror-safe.** Desktop types stay serializable, no ibapi deps, manual sync documented.

---

## Phase 1: Enrich Core Types

### `client.rs` — PositionRecord

```rust
pub struct PositionRecord {
    pub account: String,              // NEW
    pub symbol: String,
    pub con_id: i32,                  // NEW (0 if unknown)
    pub sec_type: String,             // NEW ("STK", "OPT", etc.)
    pub quantity: f64,
    pub avg_cost: f64,
    pub market_value: Option<f64>,    // NEW
    pub unrealized_pnl: Option<f64>,  // NEW
}
```

### `client.rs` — AccountSummary

```rust
pub struct AccountSummary {
    pub account: String,              // NEW
    pub cash_balance: f64,
    pub net_liquidation: f64,         // NEW
    pub unrealized_pnl: f64,
    pub realized_pnl: f64,
    pub buying_power: Option<f64>,    // NEW
    pub excess_liquidity: Option<f64>,// NEW
    pub daily_pnl: Option<f64>,      // NEW
}
```

### `events.rs` — OrderSnapshotEntry

```rust
pub struct OrderSnapshotEntry {
    pub order_id: Uuid,
    pub ib_order_id: Option<i32>,     // NEW
    pub status: String,
    pub symbol: String,
    pub action: String,
    pub order_type: String,           // NEW
    pub quantity: f64,
    pub filled_qty: f64,
    pub remaining_qty: f64,           // NEW
    pub avg_fill_price: Option<f64>,
    pub limit_price: Option<f64>,     // NEW
    pub stop_price: Option<f64>,      // NEW
    pub tif: String,                  // NEW
    pub parent_id: Option<Uuid>,      // NEW
    pub bracket_role: Option<String>, // NEW
    pub created_at: String,           // NEW (ISO 8601)
    pub updated_at: String,           // NEW
}
```

### Update callers

- `IbClient::request_positions` — populate new fields from ibapi Position
- `IbClient::request_account_summary` — add NetLiquidation, BuyingPower, ExcessLiquidity, DailyPnL to tags
- `TestBroker::request_positions` — populate from PositionState
- `TestBroker::request_account_summary` — compute net_liquidation = cash + positions value
- Engine `RequestPositions` handler — stop hardcoding `account: String::new()`

---

## Phase 2: Add `open_orders()` to BrokerClient

```rust
// client.rs — new type
pub struct OpenOrderRecord {
    pub ib_order_id: i32,
    pub symbol: String,
    pub action: String,
    pub order_type: String,
    pub quantity: f64,
    pub limit_price: Option<f64>,
    pub stop_price: Option<f64>,
    pub filled_qty: f64,
    pub remaining_qty: f64,
    pub avg_fill_price: f64,
    pub status: String,
    pub parent_id: Option<i32>,
}

// BrokerClient trait — new optional method
fn open_orders(&self) -> Vec<OpenOrderRecord> { Vec::new() }
```

- **IbClient**: call `client.all_open_orders()`, map results
- **TestBroker**: return from in-memory order state

---

## Phase 3: New Snapshot Events

```rust
// events.rs — new variants on BrokerEvent
PositionSnapshot {
    account: String,
    positions: Vec<PositionSnapshotEntry>,
},
AccountSnapshot(AccountSummary),
BracketSnapshot {
    brackets: Vec<BracketSnapshotEntry>,
},
```

```rust
// events.rs — new type
pub struct PositionSnapshotEntry {
    pub symbol: String,
    pub con_id: i32,
    pub sec_type: String,
    pub quantity: f64,
    pub avg_cost: f64,
    pub market_value: Option<f64>,
    pub unrealized_pnl: Option<f64>,
}

pub struct BracketSnapshotEntry {
    pub parent: OrderSnapshotEntry,
    pub take_profit: Option<OrderSnapshotEntry>,
    pub stop_loss: Option<OrderSnapshotEntry>,
    pub lifecycle_status: String,
}
```

---

## Phase 4: Persistence Queries

`persist/order_repo.rs` — new functions:

```rust
pub fn get_all_orders(conn) -> Result<Vec<OrderRow>>
pub fn get_active_orders(conn) -> Result<Vec<OrderRow>>
pub fn get_recent_orders(conn, limit: usize) -> Result<Vec<OrderRow>>
pub fn get_bracket_groups(conn) -> Result<Vec<(OrderRow, Vec<OrderRow>)>>
```

`get_bracket_groups` queries `WHERE bracket_role = 'Parent'` then loads
children per parent via `get_orders_by_parent_id`. N+1 is fine at expected
scale (dozens of brackets).

---

## Phase 5: New BrokerCommands + Engine Handlers

```rust
// commands.rs — new variants
RequestAllOrders,
RequestBracketSnapshot,
RequestPositionSnapshot,
```

### Engine handlers

| Command | Action | Emits |
|---------|--------|-------|
| `RequestAllOrders` | `get_all_orders` from DB | `OrderSnapshot` |
| `RequestBracketSnapshot` | `get_bracket_groups` from DB, derive lifecycle | `BracketSnapshot` |
| `RequestPositionSnapshot` | `client.request_positions()` | `PositionSnapshot` |
| `RequestAccountSummary` (fix) | Keep existing PnlUpdate, also emit `AccountSnapshot` | both |
| `RequestOrderSnapshot` (fix) | Use `OrderSnapshot` event, not individual StatusChanged | `OrderSnapshot` |

### Helper

```rust
fn order_row_to_snapshot_entry(row: &OrderRow) -> OrderSnapshotEntry
```

Direct field mapping from OrderRow — simpler than going through LocalOrder.

---

## Phase 6: Desktop Bridge

### Mirror type updates (`desktop/win/crates/midas-core/src/broker.rs`)

Update existing mirrors for PositionRecord, AccountSummary to match Phase 1.
Add new mirror types: OrderSnapshotEntry, BracketSnapshotEntry, PositionSnapshotEntry.

### Extend OrderBroker trait

```rust
pub trait OrderBroker: Send + Sync {
    // Existing
    fn name(&self) -> &str;
    fn is_connected(&self) -> bool;
    fn create_market_bracket(&self, params: MarketBracketParams) -> Result<(), String>;
    fn cancel_bracket(&self, parent_id: Uuid) -> Result<(), String>;
    fn modify_bracket_leg(&self, order_id: Uuid, new_price: f64) -> Result<(), String>;

    // NEW — fire-and-forget queries (results come as BrokerEvents)
    fn request_all_orders(&self) -> Result<(), String>;
    fn request_bracket_snapshot(&self) -> Result<(), String>;
    fn request_position_snapshot(&self) -> Result<(), String>;
    fn request_account_summary(&self) -> Result<(), String>;
}
```

---

## Execution Order

```
Phase 1 (types) ─────┐
                      ├──> Phase 4 (engine) ──> Phase 6 (desktop)
Phase 2 (open_orders) ┘         ↑
Phase 3 (events) ───────────────┘
Phase 4 (DB queries) ──────────┘
```

Phases 1 + 2 touch types and must compile together.
Phase 3 (events) and Phase 4 (DB) are independent.
Phase 5 (engine) depends on all prior phases.
Phase 6 (desktop) is last — pure mirror updates.

Each phase = one commit.

## Out of Scope

- **BrokerClient::place_order string args**: Refactoring to typed enums is a
  separate, larger change touching the IB submission path.
- **Date-range order queries**: SQLite can do this with ISO 8601 string
  comparison; add when the UI actually needs it.
- **Streaming position/PnL subscriptions**: IB supports `reqPnL` / `reqPnLSingle`;
  can be added later as a separate feature.
