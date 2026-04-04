# 01 — Data Model Changes

> Type changes to `midas-broker` and `midas-chart` for Market Order bracket support.
>
> **Implementation Status (2026-04-02):**
> - Sections 1-5 (broker types, chart types): **COMPLETE** — all types, enums,
>   methods, and tests implemented in codebase
> - Section 6 (app-layer bridge): **NOT STARTED**
> - Section 7 (database schema): No changes needed (confirmed)

---

## Table of Contents

- [1. New Types in midas-broker](#1-new-types-in-midas-broker)
- [2. Changes to Existing Types](#2-changes-to-existing-types)
- [3. BrokerCommand Changes](#3-brokercommand-changes)
- [4. BrokerEvent Changes](#4-brokerevent-changes)
- [5. Chart-Layer Changes](#5-chart-layer-changes)
- [6. App-Layer Bridge Types](#6-app-layer-bridge-types)
- [7. Database Schema](#7-database-schema)

---

## 1. New Types in midas-broker

### 1.1 MarketBracketParams

A dedicated parameter struct for creating market order brackets. This replaces
the generic `CreateBracketOrder` command variant for the market-order case,
making the intent explicit and validation straightforward.

**File**: `crates/midas-broker/src/orders/bracket.rs` (new file)

```rust
use midas_core::SecurityType;
use serde::{Deserialize, Serialize};

use super::types::{OrderAction, TimeInForce};

/// Parameters for creating a Market Order bracket.
///
/// A market bracket consists of:
/// - Parent: Market order (BUY or SELL)
/// - Take Profit: Limit order on the opposite side (optional)
/// - Stop Loss: Stop or StopLimit order on the opposite side (optional)
///
/// At least one of `take_profit` or `stop_loss` should be present for
/// the bracket to be meaningful, though a naked market order is valid.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketBracketParams {
    // -- Contract --
    pub symbol: String,
    pub con_id: Option<i32>,
    pub sec_type: SecurityType,
    pub exchange: String,
    pub currency: String,

    // -- Entry --
    pub action: OrderAction,
    pub quantity: f64,
    pub outside_rth: bool,

    // -- Take Profit --
    pub take_profit: Option<TakeProfitParams>,

    // -- Stop Loss --
    pub stop_loss: Option<StopLossParams>,

    // -- Risk Guard --
    /// Last traded price at submission time. Used by the engine-level
    /// order size guard (§10 in `02-broker-engine.md`) for notional value
    /// calculation. Populated by the order panel from chart candle data.
    pub reference_price: Option<f64>,

    // -- Metadata --
    pub strategy: Option<String>,
    pub tags: Vec<String>,
}

/// Take profit configuration for a bracket.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TakeProfitParams {
    /// Limit price for the take profit order.
    pub price: f64,
    /// Time-in-force for the TP leg. Defaults to GTC.
    pub tif: Option<TimeInForce>,
}

/// Stop loss configuration for a bracket.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StopLossParams {
    /// Trigger price for the stop.
    pub stop_price: f64,
    /// If set, creates a StopLimit instead of a Stop. This is the limit
    /// price that the order converts to after the stop triggers.
    pub limit_price: Option<f64>,
    /// Time-in-force for the SL leg. Defaults to GTC.
    pub tif: Option<TimeInForce>,
}
```

### 1.2 BracketRole Enum

Replace the current `bracket_role: Option<String>` with a proper enum.
The string-based approach invites typos and makes pattern matching awkward.

**File**: `crates/midas-broker/src/orders/types.rs`

```rust
/// Role of an order within a bracket group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BracketRole {
    /// The entry order (parent). Children reference this via `parent_id`.
    Parent,
    /// Take-profit child. Limit order on the opposite side.
    TakeProfit,
    /// Stop-loss child. Stop or StopLimit on the opposite side.
    StopLoss,
}

impl fmt::Display for BracketRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parent => f.write_str("PARENT"),
            Self::TakeProfit => f.write_str("TAKE_PROFIT"),
            Self::StopLoss => f.write_str("STOP_LOSS"),
        }
    }
}

impl FromStr for BracketRole {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Accept all known variants: uppercase canonical, lowercase from
        // architecture plan (02-order-management.md), and abbreviated legacy.
        match s {
            "PARENT" | "parent" => Ok(Self::Parent),
            "TAKE_PROFIT" | "take_profit" | "PROFIT" => Ok(Self::TakeProfit),
            "STOP_LOSS" | "stop_loss" | "STOP" => Ok(Self::StopLoss),
            other => Err(format!("unknown BracketRole: {other}")),
        }
    }
}
```

### 1.3 BracketGroup

A convenience struct returned by bracket queries. Groups the parent with its
children for bracket-level operations (activate, cancel, status display).

```rust
/// A complete bracket: parent + optional TP + optional SL.
/// Loaded from the database by querying children of a parent_id.
#[derive(Debug, Clone)]
pub struct BracketGroup {
    pub parent: LocalOrder,
    pub take_profit: Option<LocalOrder>,
    pub stop_loss: Option<LocalOrder>,
}

impl BracketGroup {
    /// All orders in activation order (parent first, SL last for transmit=true).
    pub fn legs(&self) -> Vec<&LocalOrder> {
        let mut legs = vec![&self.parent];
        if let Some(ref tp) = self.take_profit {
            legs.push(tp);
        }
        if let Some(ref sl) = self.stop_loss {
            legs.push(sl);
        }
        legs
    }

    /// True if all legs are in Inactive state (ready to activate).
    pub fn can_activate(&self) -> bool {
        self.legs().iter().all(|o| o.status.can_activate())
    }

    /// True if the parent has filled (TP/SL should be live or terminal).
    pub fn is_active(&self) -> bool {
        self.parent.status == OrderStatus::Filled
    }

    /// True if the bracket is fully resolved (all legs terminal).
    pub fn is_closed(&self) -> bool {
        self.legs().iter().all(|o| o.status.is_terminal())
    }
}
```

---

## 2. Changes to Existing Types

### 2.1 LocalOrder Field Change

In `crates/midas-broker/src/orders/types.rs`, change:

```rust
// BEFORE
pub bracket_role: Option<String>,

// AFTER
pub bracket_role: Option<BracketRole>,
```

This is a breaking change to the struct. The `new_draft()` constructor already
sets this to `None`, so no change there. The database persistence layer
(`order_repo.rs`) stores it as TEXT via `Display`/`FromStr`, which the new
enum implements.

### 2.2 Database Compatibility

The `FromStr` impl accepts all known string variants:
- **Canonical** (new writes): `"PARENT"`, `"TAKE_PROFIT"`, `"STOP_LOSS"`
- **Lowercase** (from `broker/plan/02-order-management.md`): `"parent"`, `"take_profit"`, `"stop_loss"`
- **Abbreviated legacy**: `"PROFIT"`, `"STOP"`

The `Display` impl always writes the uppercase canonical form. This means:
- Existing rows in any known format are read correctly.
- New writes use `"TAKE_PROFIT"` and `"STOP_LOSS"`.
- No migration needed.

### 2.3 OrderRow Mapping in Persistence Layer

**File**: `crates/midas-broker/src/persist/order_repo.rs`

The `OrderRow` struct stores `bracket_role: Option<String>` (TEXT column).
The mapping between `OrderRow` and `LocalOrder` must convert via
`Display`/`FromStr`:

```rust
// OrderRow → LocalOrder (reading from DB)
fn row_to_order(row: &OrderRow) -> LocalOrder {
    // ...
    bracket_role: row.bracket_role.as_deref()
        .map(|s| s.parse::<BracketRole>())
        .transpose()
        .unwrap_or_else(|e| {
            tracing::warn!("Unknown bracket_role in DB: {e}");
            None
        }),
    // ...
}

// LocalOrder → OrderRow (writing to DB)
fn order_to_row(order: &LocalOrder) -> OrderRow {
    // ...
    bracket_role: order.bracket_role.map(|r| r.to_string()),
    // ...
}
```

This must be updated in Phase 1 alongside the `LocalOrder` field change.

---

## 3. BrokerCommand Changes

### 3.1 New Command Variant

Add to `BrokerCommand` in `crates/midas-broker/src/commands.rs`:

```rust
/// Create and immediately submit a market order bracket.
///
/// Unlike `CreateBracketOrder` (which creates in Draft and requires
/// separate activation), this command creates the bracket AND submits
/// the market order to IB in a single step. The market order is
/// expected to fill near-instantly; TP/SL children activate on fill.
///
/// Rationale: Market orders don't benefit from a Draft→Inactive→Activate
/// workflow because there's no entry price to review. The user sets
/// TP/SL and hits "go".
CreateMarketBracket(MarketBracketParams),

/// Cancel an entire bracket (parent + all children) as a unit.
CancelBracket { parent_id: Uuid },

/// Modify a bracket leg's price without affecting other legs.
ModifyBracketLeg {
    order_id: Uuid,
    new_price: f64,
},
```

### 3.2 Deprecation

The existing `CreateBracketOrder` variant remains for limit-entry brackets.
For market-entry brackets, prefer `CreateMarketBracket` which has:

- Typed params instead of generic `CreateOrderParams`
- Explicit TP/SL configuration
- Immediate submission semantics

---

## 4. BrokerEvent Changes

### 4.1 New Event Variants

Add to `BrokerEvent` in `crates/midas-broker/src/events.rs`:

```rust
/// Bracket lifecycle status. Derived from individual order statuses, not stored.
/// Lives in midas-broker (NOT midas-chart). The app layer maps this to the
/// chart-layer BracketStatus enum (which is a coarser visual enum).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BracketLifecycleStatus {
    /// Parent is PendingSubmit/Submitted, children are PreSubmitted.
    Submitted,
    /// Parent filled, TP/SL are live at exchange.
    EntryFilled,
    /// Take profit child filled, stop loss auto-cancelled.
    TakeProfitHit,
    /// Stop loss child filled, take profit auto-cancelled.
    StopLossHit,
    /// All legs cancelled (parent cancelled before fill, or user cancelled).
    Cancelled,
    /// Parent rejected by IB.
    Rejected,
    /// Local/internal error during submission.
    Error,
    /// All legs terminal but doesn't fit above categories.
    Closed,
}

/// A market bracket was created and submitted.
/// Contains the UUIDs of all three legs for UI tracking.
BracketCreated {
    parent_id: Uuid,
    take_profit_id: Option<Uuid>,
    stop_loss_id: Option<Uuid>,
    symbol: String,
    action: OrderAction,
    quantity: f64,
},

/// A bracket's lifecycle status changed (derived from leg statuses).
/// Emitted when the bracket transitions between phases.
/// Uses a typed enum instead of strings for compile-time safety.
BracketStatusChanged {
    parent_id: Uuid,
    status: BracketLifecycleStatus,
    /// Fill price of the parent (available after entry fill).
    entry_fill_price: Option<f64>,
},
```

These bracket-level events are **derived** by the engine from individual order
status changes. They simplify the UI layer — instead of correlating three
separate `OrderStatusChanged` events, the UI receives one `BracketStatusChanged`.

---

## 5. Chart-Layer Changes

### 5.1 OrderBracket — No Structural Changes

The existing `OrderBracket` struct in `midas-chart` already supports market
bracket visualization:

```rust
pub struct OrderBracket {
    pub entry: BracketLeg,              // Fill price (set after market fill)
    pub take_profit: Option<BracketLeg>, // TP line
    pub stop_loss: Option<BracketLeg>,   // SL line
    pub side: BracketSide,
    pub status: BracketStatus,
    pub quantity: Option<f64>,
}
```

For market brackets:
- `entry.price` is set to the **fill price** (not a pre-set level)
- `status` starts at `Pending` (market order in-flight) and moves to `Active`
  within milliseconds of submission
- No `Draft` phase (market orders bypass chart drawing)

### 5.2 New: PnL Label Fields

Add projected P&L to `BracketLeg` for chart display:

```rust
pub struct BracketLeg {
    pub price: f64,
    pub timestamp: Option<i64>,
    pub color: Option<[f32; 4]>,
    pub style: LineStyle,
    pub line_width: f32,
    pub label: Option<String>,
    // NEW — #[serde(default)] required on both fields to preserve
    // backward compatibility with existing serialized BracketLeg data
    // in the annotation persistence layer (JSON files per symbol/timeframe).
    /// Projected dollar P&L at this leg's price level.
    /// Computed by the app layer using entry fill price and quantity.
    #[serde(default)]
    pub projected_pnl: Option<f64>,
    /// Projected percentage P&L.
    #[serde(default)]
    pub projected_pnl_pct: Option<f64>,
}
```

These are **display-only** fields computed by the app layer after the entry
fills. The chart crate renders them as part of the price badge but does not
compute them (sans-IO principle).

### 5.3 R:R Display Enhancement

The existing `risk_reward()` method already works. For market brackets we
also want to display absolute dollar risk/reward:

```rust
impl OrderBracket {
    /// Absolute dollar risk. Returns None if SL is missing or qty is zero.
    pub fn dollar_risk(&self) -> Option<f64> {
        let sl = self.stop_loss.as_ref()?;
        let qty = self.quantity?;
        Some((self.entry.price - sl.price).abs() * qty)
    }

    /// Absolute dollar reward. Returns None if TP is missing or qty is zero.
    pub fn dollar_reward(&self) -> Option<f64> {
        let tp = self.take_profit.as_ref()?;
        let qty = self.quantity?;
        Some((tp.price - self.entry.price).abs() * qty)
    }
}
```

---

## 6. App-Layer Bridge Types

### 6.1 OrderAnnotationLink

This struct lives in `midas-app` (the only crate that depends on both
`midas-chart` and `midas-broker`). It maps visual annotations to broker orders.

```rust
/// Maps a chart OrderBracket annotation to its broker order legs.
/// Stored in midas-app's annotation manager.
pub struct OrderAnnotationLink {
    /// Annotation ID in the chart's AnnotationStore.
    pub annotation_id: u64,
    /// Broker order UUID of the parent (entry) order.
    pub parent_order_id: Uuid,
    /// Broker order UUID of the TP child (if any).
    pub tp_order_id: Option<Uuid>,
    /// Broker order UUID of the SL child (if any).
    pub sl_order_id: Option<Uuid>,
    /// Symbol (for quick lookup without loading orders).
    pub symbol: String,
}
```

### 6.2 State Mapping

The app layer translates `BracketStatusChanged` events to `BracketStatus`
updates on the chart annotation:

| BracketLifecycleStatus | Chart BracketStatus | Visual Effect |
|---|---|---|
| (on `BracketCreated`) | `Pending` | Entry line dotted, TP/SL dashed |
| `EntryFilled` | `Active` | Entry line solid at fill price, TP/SL solid |
| `TakeProfitHit` | `Closed` | All lines dimmed (alpha × 0.3) |
| `StopLossHit` | `Closed` | All lines dimmed |
| `Cancelled` | `Cancelled` | All lines dimmed |
| `Rejected` | `Cancelled` | All lines dimmed |
| `Error` | `Cancelled` | All lines dimmed, error toast shown |

> **Note**: `BracketStatus::PartialFill` exists in the chart enum but is not
> mapped by any `BracketLifecycleStatus` variant for market brackets. It is
> reserved for future limit-entry brackets where partial entry fills are more
> common and the visual distinction is meaningful.

---

## 7. Database Schema

### 7.1 No DDL Changes Required

The existing `orders` table already has all necessary columns:

| Column | Used For |
|---|---|
| `parent_id` | Links TP/SL children to the parent market order |
| `bracket_role` | `"PARENT"`, `"TAKE_PROFIT"`, `"STOP_LOSS"` |
| `oca_group` | Not used for standard brackets (IB manages implicit OCA) |
| `order_type` | `"MKT"` for parent, `"LMT"` for TP, `"STP"` or `"STP LMT"` for SL |

### 7.2 Example Rows After Bracket Creation

A BUY 100 AAPL market bracket with TP at $192 and SL at $182:

```
| local_id  | symbol | action | order_type | quantity | limit_price | stop_price | parent_id | bracket_role |
|-----------|--------|--------|------------|----------|-------------|------------|-----------|--------------|
| uuid-A    | AAPL   | BUY    | MKT        | 100      | NULL        | NULL       | NULL      | PARENT       |
| uuid-B    | AAPL   | SELL   | LMT        | 100      | 192.00      | NULL       | uuid-A    | TAKE_PROFIT  |
| uuid-C    | AAPL   | SELL   | STP        | 100      | NULL        | 182.00     | uuid-A    | STOP_LOSS    |
```

### 7.3 Recommended Index

Add an index for bracket group lookups (querying all children of a parent):

```sql
-- Already exists in 001_initial.sql:
-- CREATE INDEX idx_orders_parent ON orders(parent_id);
```

No new indices needed. The existing `idx_orders_parent` covers the
`list_bracket_children(parent_id)` query.

---

## 8. Persistence Conversion Layer (NOT YET IMPLEMENTED)

> Added 2026-04-02 during plan refinement. This was undersized in the
> original Phase 1 scope and is now explicitly scoped.

### 8.1 The Gap

The persistence layer (`order_repo.rs`) operates on `OrderRow` (stringly-typed
SQLite mirror). `LocalOrder` uses typed enums (`OrderStatus`, `OrderAction`,
`OrderKind`, `SecurityType`, `TimeInForce`, `BracketRole`, `Uuid`,
`DateTime<Utc>`, JSON tags/algo_params). **No bidirectional conversion
between `OrderRow` and `LocalOrder` exists today.** The bracket persistence
functions (`persist_and_transition_to_pending_submit`, `transition_bracket_to_error`)
require this conversion as a prerequisite.

### 8.2 Scope

Create two functions in `crates/midas-broker/src/persist/order_repo.rs`:

```rust
/// Convert a database OrderRow to a typed LocalOrder.
///
/// **Error policy**: Hard-fail on order-critical fields (`status`, `action`,
/// `order_type`, `tif`) — returns `Err(ConversionError)` if these cannot be
/// parsed. Soft-fail on non-routing fields (`bracket_role`, `strategy`, `tags`)
/// — logs a warning and falls back to defaults.
///
/// Named `order_row_to_local` to avoid collision with the existing
/// `row_to_order` function (which converts `rusqlite::Row -> OrderRow`).
pub fn order_row_to_local(row: &OrderRow) -> Result<LocalOrder, ConversionError>;

/// Convert a typed LocalOrder to a database OrderRow for persistence.
/// Serializes all typed fields via Display/ToString.
pub fn local_to_order_row(order: &LocalOrder) -> OrderRow;
```

**Estimated size**: 100-150 lines for the two functions + 30-50 lines of
tests covering round-trip conversion and error cases for each enum field.

**Fields requiring conversion** (non-trivial):

*Hard-fail fields* — `Err(ConversionError)` if unparseable (order-critical):
- `status: String` ↔ `OrderStatus` (via `FromStr`/`Display`)
- `action: String` ↔ `OrderAction`
- `order_type: String` ↔ `OrderKind`
- `tif: String` ↔ `TimeInForce`
- `sec_type: String` ↔ `SecurityType`
- `local_id: String` ↔ `id: Uuid`

*Soft-fail fields* — log warning, use default if unparseable:
- `bracket_role: Option<String>` ↔ `Option<BracketRole>` (default: `None`)
- `parent_id: Option<String>` ↔ `Option<Uuid>` (default: `None`)
- `tags: Option<String>` ↔ `Vec<String>` (JSON array, default: `[]`)
- `algo_params: Option<String>` ↔ `Option<serde_json::Value>` (default: `None`)
- `created_at/updated_at: String` ↔ `DateTime<Utc>` (RFC 3339)
- `last_activated_at/last_deactivated_at: Option<String>` ↔ `Option<DateTime<Utc>>`

*Not modeled* — `good_after_time` and `good_till_date` exist in the SQL
schema but are absent from both `OrderRow` and `LocalOrder`. Currently
hardcoded as `None` in `insert_order`. The `local_to_order_row` function
should continue this pattern. No conversion needed.

### 8.3 Bracket-Specific Persistence Helpers

These depend on `order_row_to_local`/`local_to_order_row` above:

```rust
/// Persist all bracket legs and atomically transition Inactive → PendingSubmit.
/// Single SQLite transaction. Called BEFORE any IB API calls.
pub fn persist_and_transition_to_pending_submit(
    conn: &Connection,
    group: &mut BracketGroup,
) -> Result<(), rusqlite::Error>;

/// Transition all bracket legs to Error status with a reason.
/// Called when IB submission fails partway through.
pub fn transition_bracket_to_error(
    conn: &Connection,
    group: &BracketGroup,
    reason: &str,
) -> Result<(), rusqlite::Error>;

/// Transition all bracket legs to Rejected status.
/// Called when IB rejects the parent order.
pub fn transition_bracket_to_rejected(
    conn: &Connection,
    group: &BracketGroup,
    reason: &str,
) -> Result<(), rusqlite::Error>;
```

**Estimated size**: 80-100 lines for the three helpers + 20-30 lines of tests.
