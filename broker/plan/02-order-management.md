# Order Management Subsystem — `midas-broker`

> Design plan for the order management layer of the `midas-broker` crate.
> Wraps [rust-ibapi](https://github.com/wboayue/rust-ibapi) for Interactive Brokers trading.
> March 2026.

---

## Table of Contents

- [1. Order Lifecycle States](#1-order-lifecycle-states)
- [2. Local Order Store](#2-local-order-store)
- [3. Order Types Support](#3-order-types-support)
- [4. Activate / Deactivate](#4-activate--deactivate)
- [5. Order Modification](#5-order-modification)
- [6. Bracket Order Management](#6-bracket-order-management)
- [7. Order Groups and Tags](#7-order-groups-and-tags)
- [8. Reconciliation on Startup](#8-reconciliation-on-startup)

---

## 1. Order Lifecycle States

### 1.1 State Definitions

Every order managed by `midas-broker` exists in exactly one of the following states at any point in time. The first two states (`Draft`, `Inactive`) are purely local — IB has no knowledge of orders in these states. All other states involve IB.

| State | Owner | Description |
|---|---|---|
| **Draft** | Local only | Created in memory but not yet persisted to SQLite. Exists only during construction via the builder API. Discarded if the user abandons without saving. |
| **Inactive** | Local (SQLite) | Persisted locally. NOT sent to IB. This is the "deactivated" / "parked" state. The order has all its parameters defined and is ready to be activated at any time. Orders return here after deactivation. |
| **PendingSubmit** | Local + IB | `place_order` has been called on the `rust-ibapi` Client. Waiting for IB to acknowledge receipt. Transient state — typically lasts milliseconds to low single-digit seconds. |
| **PreSubmitted** | Local + IB | IB has accepted the order but it is not yet working at the exchange. Applies to simulated order types (stop orders, conditional orders) that IB holds on its servers until the trigger condition is met. |
| **Submitted** | Local + IB | Order is confirmed working at the exchange. Actively seeking a fill. |
| **PartiallyFilled** | Local + IB | Order has received at least one execution but `remaining > 0`. Still working at the exchange for the unfilled portion. |
| **Filled** | Local + IB | All shares/contracts have been executed. `remaining == 0`. Terminal state. |
| **PendingCancel** | Local + IB | A cancel request has been sent to IB. Waiting for confirmation. Transient state. |
| **Cancelled** | Local + IB | IB has confirmed the order is cancelled. Terminal state (but the order record persists locally forever). |
| **Rejected** | Local + IB | IB rejected the order (invalid contract, insufficient margin, price out of range, etc.). The rejection reason is stored. Terminal state — the order cannot be retried without creating a new one. |
| **Error** | Local | A local/internal error occurred (network failure, serialization error, etc.). The error reason is stored. Non-terminal — the user can fix the issue and retry by editing and re-activating. |

### 1.2 State Machine Diagram

```
                    +-----------+
                    |   Draft   |
                    +-----+-----+
                          |
                          | save()
                          v
                    +-----------+
        +---------->| Inactive  |<--------------------------+
        |           +-----+-----+                           |
        |                 |                                  |
        |                 | activate()                       | deactivate()
        |                 v                                  | (cancel confirmed)
        |         +---------------+                          |
        |         | PendingSubmit |-------+                  |
        |         +-------+-------+       |                  |
        |                 |               | error/reject     |
        |     ib confirms |               v                  |
        |                 |          +----------+            |
        |          +------+------+   |  Error   |            |
        |          |             |   +----+-----+            |
        |          v             v        |                  |
        |   +-----------+  +----------+   | edit + reactivate|
        |   |PreSubmitted|  |Submitted |  +---------+        |
        |   +-----+------+ +----+-----+            |        |
        |         |              |                  |        |
        |         | trigger      |                  v        |
        |         +------>-------+          (goes to Inactive|
        |                |                   then activate)  |
        |                |                                   |
        |         +------+--------+                          |
        |         |               |                          |
        |         v               v                          |
        |  +-------------+  +---------+                      |
        |  |PartiallyFill|  |  Filled |                      |
        |  +------+------+  +---------+                      |
        |         |          (terminal)                      |
        |         |                                          |
        |         +--------> Filled  (remaining fills)       |
        |         |                                          |
        |         v                                          |
        |  +---------------+                                 |
        |  | PendingCancel |                                 |
        |  +-------+-------+                                 |
        |          |                                         |
        |          +---> Cancelled (terminal)                |
        |          |                                         |
        |          +---> (partial cancel) -----> Inactive ---+
        |                 (cancelled with fills:              |
        |                  remains Cancelled terminal)        |
        |                                                    |
        +----------------------------------------------------+
               deactivate() from Submitted/PreSubmitted
               (sends cancel to IB, on confirm -> Inactive)
```

### 1.3 Transition Rules

| From | To | Trigger | Notes |
|---|---|---|---|
| Draft | Inactive | `save()` | Persists to SQLite. No IB interaction. |
| Draft | (discarded) | Drop/abandon | No persistence. Builder goes out of scope. |
| Inactive | PendingSubmit | `activate()` | Validates order, calls `client.place_order()`. |
| Inactive | Draft | (not allowed) | Once saved, an order is always in SQLite. Edit in place. |
| PendingSubmit | PreSubmitted | IB status callback | Simulated orders (stops, conditionals). |
| PendingSubmit | Submitted | IB status callback | Direct market/limit orders. |
| PendingSubmit | Rejected | IB rejection | IB rejected the order (terminal). Invalid parameters, insufficient funds, etc. |
| PendingSubmit | Error | Local/internal error | Network failure, serialization error, etc. (non-terminal, retryable). |
| PreSubmitted | Submitted | IB trigger fires | Stop price hit, condition met. |
| PreSubmitted | PendingCancel | `deactivate()` or `cancel()` | User requests cancellation. |
| Submitted | PartiallyFilled | IB execution | `filled > 0 && remaining > 0`. |
| Submitted | Filled | IB execution | `remaining == 0`. |
| Submitted | Rejected | IB rejection | IB rejected the order after it was submitted (terminal). |
| Submitted | PendingCancel | `deactivate()` or `cancel()` | User requests cancellation. |
| PartiallyFilled | Filled | IB execution | Final fill arrives. |
| PartiallyFilled | PendingCancel | `cancel()` | User cancels remainder. |
| PendingCancel | Cancelled | IB confirms cancel | Terminal. Fills received before cancel are kept. |
| PendingCancel | Inactive | `deactivate()` confirmed | Only if zero fills. Order returns to parked state. |
| PendingCancel | Filled | IB execution | Race condition: fill arrives before cancel processes. |
| Error | Inactive | `edit() + save()` | User fixes the order, saves it back to Inactive for retry. |
| Filled | (none) | -- | Terminal state. Immutable. |
| Cancelled | (none) | -- | Terminal state. Immutable. |
| Rejected | (none) | -- | Terminal state. Immutable. IB rejected this order. |

### 1.4 Rust Enum

```rust
/// **CANONICAL DEFINITION** — All other documents reference this enum.
/// Stored as TEXT in SQLite via Display/FromStr (see 03-data-layer.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OrderState {
    Draft          = 0,
    Inactive       = 1,
    PendingSubmit  = 2,
    PreSubmitted   = 3,
    Submitted      = 4,
    PartiallyFilled = 5,
    Filled         = 6,
    PendingCancel  = 7,
    Cancelled      = 8,
    Rejected       = 9,  // IB rejected the order (terminal — cannot retry)
    Error          = 10, // Local/internal error (non-terminal — user can fix and retry)
}

impl OrderState {
    /// Returns true if the order is in a terminal state (no further transitions possible).
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Filled | Self::Cancelled | Self::Rejected)
    }

    /// Returns true if the order is live at IB (may have working quantity).
    pub fn is_live(&self) -> bool {
        matches!(
            self,
            Self::PendingSubmit
                | Self::PreSubmitted
                | Self::Submitted
                | Self::PartiallyFilled
                | Self::PendingCancel
        )
    }

    /// Returns true if the order can be activated (sent to IB).
    pub fn can_activate(&self) -> bool {
        matches!(self, Self::Inactive | Self::Error)
    }

    /// Returns true if the order can be deactivated (cancelled at IB, returned to Inactive).
    pub fn can_deactivate(&self) -> bool {
        matches!(self, Self::PreSubmitted | Self::Submitted)
    }

    /// Returns true if the order can be modified locally without IB interaction.
    pub fn can_modify_locally(&self) -> bool {
        matches!(self, Self::Inactive | Self::Error)
    }

    /// Returns true if the order can be modified at IB (requires sending modification).
    pub fn can_modify_at_ib(&self) -> bool {
        matches!(self, Self::PreSubmitted | Self::Submitted)
    }
}
```

### 1.5 Mapping IB Status Strings to `OrderState`

IB's `OrderStatus.status` field is a `String`. The mapping to our enum:

| IB Status String | `OrderState` | Notes |
|---|---|---|
| `"ApiPending"` | `PendingSubmit` | Not yet sent to IB server from TWS/Gateway. |
| `"PendingSubmit"` | `PendingSubmit` | Sent from TWS, awaiting destination confirmation. |
| `"PreSubmitted"` | `PreSubmitted` | Simulated order accepted, not yet triggered. |
| `"Submitted"` | `Submitted` | Working at the exchange. |
| `"Filled"` | `Filled` | Check `remaining == 0` to distinguish from partial. |
| `"PendingCancel"` | `PendingCancel` | Cancel request sent, not yet confirmed. |
| `"Cancelled"` | `Cancelled` | Confirmed cancelled. |
| `"ApiCancelled"` | `Cancelled` | Cancelled via API. Same terminal state. |
| `"Inactive"` | `Rejected` | IB uses "Inactive" to mean rejected/error/conditions not met. Maps to `Rejected` (terminal), NOT `Error` (which is for local errors). Do NOT confuse with our `Inactive` (which means locally parked). |
| (partial fill detected) | `PartiallyFilled` | When `filled > 0 && remaining > 0`. IB does not have a distinct status string for this — we derive it. |

**Critical distinction:** IB's `"Inactive"` status means the order was rejected or has unmet conditions — it maps to our `Rejected` (terminal). Our `Inactive` state means the order is parked locally and was never sent (or was deactivated). Our `Error` state is for local/internal errors (non-terminal, retryable). These are three completely different concepts. The mapping function must never confuse them.

---

## 2. Local Order Store

### 2.1 Core Concept

The local SQLite database is the single source of truth for all order state within `midas-broker`. Orders are always created locally first, and IB is treated as an external execution venue that we synchronize with.

**Principles:**

1. Every order that has ever been saved exists in the `orders` table forever. No deletions.
2. Every state transition is recorded in the `order_audit` table with a timestamp.
3. IB's `orderId` is stored alongside our internal UUID. The `orderId` is assigned at activation time (from `client.next_order_id()`).
4. The `permId` (IB's permanent order identifier) is captured on first IB callback and stored for reconciliation.
5. All order parameters are stored locally so that a deactivated order can be re-activated without any external lookups.

### 2.2 SQLite Schema

See 03-data-layer.md §1a for the authoritative `orders` table DDL. Key columns used by order management: `status`, `bracket_role`, `oca_group`, `strategy`, `activation_count`, `last_activated_at`, `last_deactivated_at`.

See 03-data-layer.md §1b-§1c for the `order_audit` and `fills` tables.

### 2.3 Order Store API

```rust
/// Manages order persistence in SQLite via rusqlite.
/// All methods are synchronous and called from within `spawn_blocking`.
/// See 03-data-layer.md for the full database access pattern.
pub struct OrderStore {
    db: Arc<Mutex<rusqlite::Connection>>,
}

impl OrderStore {
    pub fn new(db: Arc<Mutex<rusqlite::Connection>>) -> Self;

    // CRUD (synchronous — called inside spawn_blocking)
    pub fn insert(&self, order: &MidasOrder) -> Result<()>;
    pub fn update(&self, order: &MidasOrder) -> Result<()>;
    pub fn get(&self, id: &OrderId) -> Result<Option<MidasOrder>>;
    pub fn get_by_ib_order_id(&self, ib_order_id: i32) -> Result<Option<MidasOrder>>;
    pub fn get_by_ib_perm_id(&self, ib_perm_id: i64) -> Result<Option<MidasOrder>>;

    // Queries
    pub fn list_by_state(&self, state: OrderState) -> Result<Vec<MidasOrder>>;
    pub fn list_by_symbol(&self, symbol: &str) -> Result<Vec<MidasOrder>>;
    pub fn list_by_tag(&self, tag: &str) -> Result<Vec<MidasOrder>>;
    pub fn list_by_strategy(&self, strategy: &str) -> Result<Vec<MidasOrder>>;
    pub fn list_live_orders(&self) -> Result<Vec<MidasOrder>>;
    pub fn list_inactive_orders(&self) -> Result<Vec<MidasOrder>>;
    pub fn list_bracket_children(&self, parent_id: &OrderId) -> Result<Vec<MidasOrder>>;

    // State transitions (all log to audit table in the same transaction)
    pub fn transition(
        &self,
        id: &OrderId,
        from: OrderState,
        to: OrderState,
        trigger: &str,
        details: Option<&serde_json::Value>,
    ) -> Result<()>;

    // Fills
    pub fn record_fill(&self, fill: &OrderFill) -> Result<()>;
    pub fn get_fills(&self, order_id: &OrderId) -> Result<Vec<OrderFill>>;

    // Audit
    pub fn get_audit_log(&self, order_id: &OrderId) -> Result<Vec<AuditEntry>>;
}
```

### 2.4 Activate / Deactivate Flow Through the Store

**Activate:**
1. Load order from SQLite (must be `Inactive` or `Error`).
2. Validate all required fields (contract, action, quantity, order type, prices).
3. Obtain `next_order_id()` from the IB client.
4. Store the `ib_order_id` on the order record.
5. Transition state: `Inactive -> PendingSubmit` (in SQLite, with audit log).
6. Call `client.place_order(ib_order_id, &contract, &ib_order)`.
7. Monitor `OrderUpdate` stream for status callbacks.
8. On each callback, update SQLite state and fill information.

**Deactivate:**
1. Load order from SQLite (must be `PreSubmitted`, `Submitted`, or `PartiallyFilled`).
2. For `PartiallyFilled`: warn user that fills already received are permanent. Only the unfilled remainder will be cancelled. The order will go to `Cancelled` (not `Inactive`) because it has partial fills.
3. For `PreSubmitted` / `Submitted` (zero fills): transition to `PendingCancel`, call `client.cancel_order()`. On IB cancel confirmation, transition to `Inactive` (not `Cancelled`). The order is parked, not terminated.
4. Clear the `ib_order_id` on the order record (it will get a new one on re-activation).
5. Increment `activation_count`, set `last_deactivated_at`.

---

## 3. Order Types Support

### 3.1 v1 Order Types

These order types are supported in the initial release:

| Order Type | `order_type` value | Required Price Fields | Notes |
|---|---|---|---|
| **Market** | `"MKT"` | None | Immediate execution at best available price. |
| **Limit** | `"LMT"` | `limit_price` | Execute at limit price or better. |
| **Stop** | `"STP"` | `aux_price` (stop trigger) | Becomes market order when stop price is hit. IB simulates these server-side (PreSubmitted). |
| **Stop Limit** | `"STP LMT"` | `limit_price` + `aux_price` | Becomes limit order when stop price is hit. |
| **Trailing Stop** | `"TRAIL"` | `aux_price` (trailing amount) OR `trailing_percent` | Stop price trails the market by a fixed amount or percentage. |
| **Trailing Stop Limit** | `"TRAIL LIMIT"` | `limit_price` + `aux_price` + `trail_stop_price` | Trailing stop that becomes a limit order. |

### 3.2 v1 Composite Order Structures

| Structure | Description | Implementation |
|---|---|---|
| **Bracket Order** | Parent entry + Take Profit (LMT) + Stop Loss (STP). Three linked orders. | Built via `BracketOrderBuilder`. Parent `transmit=false`, TP `transmit=false`, SL `transmit=true`. Children reference parent via `parent_id`. |
| **OCA Group** | N unrelated orders where one filling cancels the others. | All orders share the same `oca_group` string. `oca_type` controls behavior (cancel vs reduce). |

### 3.3 v1 Algo Support

| Algo | `algo_strategy` | Key Parameters | Use Case |
|---|---|---|---|
| **Adaptive** | `"Adaptive"` | `adaptivePriority`: `"Urgent"` / `"Normal"` / `"Patient"` | Default algo for most orders. Reduces market impact. |

Adaptive is the only algo in v1 because it is the most broadly useful and requires minimal parameterization. It can be applied to any limit order.

### 3.4 v1 Time-in-Force Support

| TIF | Description |
|---|---|
| `DAY` | Valid for the current trading day only. Default. |
| `GTC` | Good-Til-Cancelled. Persists across sessions. |
| `IOC` | Immediate-or-Cancel. Fill what you can, cancel the rest. |
| `GTD` | Good-Til-Date. Requires `good_till_date` field. |
| `OPG` | At-the-Open. Participates in the opening auction. |

### 3.5 Deferred to v2

The following order types and features are explicitly deferred to v2:

| Feature | Reason for Deferral |
|---|---|
| Market-if-Touched (`MIT`) | Niche use case. |
| Limit-if-Touched (`LIT`) | Niche use case. |
| Market-on-Close (`MOC`) / Limit-on-Close (`LOC`) | Requires special handling around market close. |
| Pegged orders (`REL`, `PEG MKT`, `PEG MID`) | Complex pricing logic. |
| Volatility orders (`VOL`) | Options-specific, complex. |
| VWAP / TWAP algos | Require time window parameterization and more complex UI. |
| Arrival Price / Close Price algos | Institutional-oriented. |
| Conditional orders (price/time/volume conditions) | Significant additional state machine complexity. |
| Multi-leg combo orders | Requires combo contract support in the contract layer. |
| Forex orders | Different contract semantics. |
| FOK (Fill-or-Kill) | Rarely used, trivial to add later. |

### 3.6 Order Validation Rules

Before activation, every order must pass validation:

```rust
pub struct OrderValidator;

impl OrderValidator {
    pub fn validate(order: &MidasOrder) -> Result<(), Vec<ValidationError>> {
        let mut errors = Vec::new();

        // Contract validation
        if order.symbol.is_empty() { errors.push(ValidationError::MissingSymbol); }
        if order.con_id.is_none() { errors.push(ValidationError::UnresolvedContract); }

        // Price validation by order type
        match order.order_type.as_str() {
            "LMT" => {
                if order.limit_price.is_none() {
                    errors.push(ValidationError::MissingLimitPrice);
                }
            }
            "STP" => {
                if order.aux_price.is_none() {
                    errors.push(ValidationError::MissingStopPrice);
                }
            }
            "STP LMT" => {
                if order.limit_price.is_none() {
                    errors.push(ValidationError::MissingLimitPrice);
                }
                if order.aux_price.is_none() {
                    errors.push(ValidationError::MissingStopPrice);
                }
            }
            "TRAIL" => {
                if order.aux_price.is_none() && order.trailing_percent.is_none() {
                    errors.push(ValidationError::MissingTrailingAmount);
                }
            }
            "MKT" => { /* No price fields required */ }
            other => errors.push(ValidationError::UnsupportedOrderType(other.to_string())),
        }

        // Quantity validation
        if order.total_quantity <= 0.0 {
            errors.push(ValidationError::InvalidQuantity);
        }

        // Bracket validation
        if order.bracket_role == Some(BracketRole::TakeProfit)
            || order.bracket_role == Some(BracketRole::StopLoss)
        {
            if order.parent_order_id.is_none() {
                errors.push(ValidationError::BracketChildMissingParent);
            }
        }

        if errors.is_empty() { Ok(()) } else { Err(errors) }
    }
}
```

---

## 4. Activate / Deactivate

### 4.1 Activate Flow

Activation is the process of taking a locally-parked order and sending it to IB for execution.

```
User calls activate(order_id)
        |
        v
  Load order from SQLite
        |
        v
  Assert state is Inactive or Error
        |
        v
  Run OrderValidator::validate()
        |
        v  (validation fails -> return Err, stay in current state)
  Resolve contract (qualifyContracts if con_id missing)
        |
        v
  Acquire next_order_id from IB client
        |
        v
  Set order.ib_order_id = next_order_id
  Set order.state = PendingSubmit
  Set order.activation_count += 1
  Set order.last_activated_at = now()
        |
        v
  BEGIN TRANSACTION
    UPDATE orders SET state=PendingSubmit, ib_order_id=...
    INSERT INTO order_audit (...)
  COMMIT
        |
        v
  Build ibapi::Order from MidasOrder fields
        |
        v
  Call client.place_order(ib_order_id, &contract, &ib_order)
        |
        v
  Subscribe to OrderUpdate stream for this order
        |
        v
  Return Ok(ActivationResult { order_id, ib_order_id })
```

### 4.2 Deactivate Flow

Deactivation cancels a live order at IB and returns it to the `Inactive` state locally, preserving all order parameters for future re-activation.

```
User calls deactivate(order_id)
        |
        v
  Load order from SQLite
        |
        v
  Assert state is PreSubmitted or Submitted
  (PartiallyFilled cannot be deactivated — must use cancel() instead)
        |
        v
  Transition state: current -> PendingCancel
  Log to audit: "deactivate requested"
        |
        v
  Call client.cancel_order(ib_order_id, "")
        |
        v
  (Async) IB confirms cancellation via OrderUpdate::OrderStatus
        |
        v
  On cancel confirmation:
    Assert filled_quantity == 0 (otherwise this becomes Cancelled, not Inactive)
    Transition state: PendingCancel -> Inactive
    Clear ib_order_id (will get fresh one on re-activation)
    Set last_deactivated_at = now()
    Log to audit: "deactivated — returned to Inactive"
```

**Key rule:** If the order received any fills before deactivation completed (race condition), it transitions to `Cancelled` instead of `Inactive`. A partially-filled order cannot be deactivated — it can only be cancelled. This prevents data integrity issues where fill records reference an order that claims to have never been sent.

### 4.3 Re-Activate Flow

Re-activation is identical to initial activation. The order is in `Inactive` state and follows the same `activate()` path. The only differences:

- A new `ib_order_id` is obtained (IB requires unique order IDs).
- `activation_count` increments again.
- The user may have modified order parameters while the order was Inactive.

### 4.4 Bulk Operations

```rust
pub struct OrderManager {
    store: OrderStore,
    ib_client: Arc<Client>,
}

impl OrderManager {
    /// Activate all Inactive orders matching the given symbol.
    pub async fn activate_by_symbol(&self, symbol: &str) -> Result<BulkResult>;

    /// Activate all Inactive orders matching the given tag.
    pub async fn activate_by_tag(&self, tag: &str) -> Result<BulkResult>;

    /// Activate all Inactive orders in the given strategy.
    pub async fn activate_by_strategy(&self, strategy: &str) -> Result<BulkResult>;

    /// Deactivate all live orders matching the given symbol.
    /// Skips PartiallyFilled orders (logs a warning for each).
    pub async fn deactivate_by_symbol(&self, symbol: &str) -> Result<BulkResult>;

    /// Deactivate all live orders matching the given tag.
    pub async fn deactivate_by_tag(&self, tag: &str) -> Result<BulkResult>;

    /// Deactivate all live orders in the given strategy.
    pub async fn deactivate_by_strategy(&self, strategy: &str) -> Result<BulkResult>;

    /// Deactivate ALL live orders. Emergency kill switch.
    /// Calls client.global_cancel() for speed, then reconciles local state.
    pub async fn deactivate_all(&self) -> Result<BulkResult>;
}

pub struct BulkResult {
    pub succeeded: Vec<OrderId>,
    pub failed: Vec<(OrderId, String)>,  // (id, error reason)
    pub skipped: Vec<(OrderId, String)>, // (id, skip reason)
}
```

### 4.5 Concurrency and Ordering

- Bulk activate sends orders sequentially with a configurable delay between them (default: 20ms) to stay under IB's 50 messages/second rate limit.
- Each `activate()` call is atomic at the SQLite level (single transaction for state change + audit log).
- The IB `place_order` call happens after the SQLite transaction commits. If `place_order` fails (network error), the order is transitioned to `Error` with the failure reason.
- A background task processes the `order_update_stream` and dispatches state transitions as IB callbacks arrive. This task holds no locks — it reads the callback, loads the order by `ib_order_id`, and performs the transition in a SQLite transaction.

---

## 5. Order Modification

### 5.1 Modification When Inactive (Local Only)

When an order is in `Inactive` or `Error` state, modifications are purely local SQLite updates. No IB interaction occurs.

**Modifiable fields (local):**
- All order parameters: `order_type`, `action`, `total_quantity`, `limit_price`, `aux_price`, `trail_stop_price`, `trailing_percent`
- Time-in-force: `tif`, `good_till_date`, `good_after_time`
- Algo: `algo_strategy`, `algo_params`
- Extended hours: `outside_rth`
- Contract: `symbol`, `sec_type`, `exchange`, `con_id` (changing the contract effectively makes this a different order, but we allow it while Inactive)
- Metadata: `tags`, `strategy`, `notes`

**Flow:**
```
modify_inactive(order_id, changes) ->
    Load order, assert Inactive or Error
    Apply changes to in-memory struct
    Run OrderValidator::validate() on modified order
    UPDATE orders SET ... WHERE id = ?
    INSERT INTO order_audit (trigger = "modified_locally", details = JSON diff)
```

### 5.2 Modification When Live at IB

When an order is `PreSubmitted` or `Submitted`, modification requires sending the change to IB.

**IB-modifiable fields (via `place_order` with same `orderId`):**
- `limit_price`
- `aux_price` (stop price)
- `total_quantity`
- `tif`

**IB recommendation:** Only modify price, quantity, and TIF. For anything else, cancel and re-place.

**Fields that require cancel + re-place:**
- `order_type` (cannot change from LMT to STP in place)
- `action` (cannot change from BUY to SELL in place)
- `contract` (cannot change the instrument)
- `algo_strategy` or `algo_params`

**Flow for IB-modifiable fields:**
```
modify_live(order_id, changes) ->
    Load order, assert PreSubmitted or Submitted
    Validate that only IB-modifiable fields are changing
    Apply changes to in-memory struct
    Run OrderValidator::validate()
    BEGIN TRANSACTION
      UPDATE orders SET limit_price=?, aux_price=?, total_quantity=?, tif=?
      INSERT INTO order_audit (trigger = "modify_sent_to_ib", details = JSON diff)
    COMMIT
    Build ibapi::Order from updated MidasOrder
    Call client.place_order(same ib_order_id, &contract, &ib_order)
    // IB treats same orderId as modification, not new order
    Monitor OrderUpdate for confirmation
```

**Flow for fields requiring cancel + re-place:**
```
modify_live_destructive(order_id, changes) ->
    Load order, assert PreSubmitted or Submitted
    Detect that changes include non-modifiable fields
    Deactivate the order (cancel at IB -> Inactive)
    Wait for cancel confirmation
    Apply all changes locally (now Inactive, so any field is fair game)
    Re-activate the order (new ib_order_id)
```

### 5.3 Modification Safety

```rust
#[derive(Debug)]
pub enum ModifyScope {
    /// Changes can be applied purely locally. No IB interaction.
    LocalOnly,
    /// Changes can be sent as an in-place modification to IB (same orderId).
    IbInPlace,
    /// Changes require cancel + re-place at IB (new orderId).
    CancelReplace,
}

impl MidasOrder {
    /// Determines the required modification scope given a set of proposed changes.
    pub fn modification_scope(&self, changes: &OrderChanges) -> ModifyScope {
        if self.state.can_modify_locally() {
            return ModifyScope::LocalOnly;
        }

        if self.state.can_modify_at_ib() {
            let ib_safe = changes.only_touches(&[
                "limit_price", "aux_price", "total_quantity", "tif",
            ]);
            if ib_safe {
                ModifyScope::IbInPlace
            } else {
                ModifyScope::CancelReplace
            }
        } else {
            // Cannot modify orders in PendingSubmit, PendingCancel, Filled, Cancelled
            panic!("Cannot modify order in state {:?}", self.state);
        }
    }
}
```

---

## 6. Bracket Order Management

### 6.1 Creating a Bracket Order

A bracket order consists of three linked orders stored as separate rows in the `orders` table:

1. **Parent** — The entry order (typically LMT or MKT).
2. **Take Profit (TP)** — A LMT order on the opposite side, closing the position at a profit target.
3. **Stop Loss (SL)** — A STP order on the opposite side, closing the position at a loss limit.

All three orders share a relationship through `parent_order_id` and `bracket_role`:

| Order | `bracket_role` | `parent_order_id` |
|---|---|---|
| Parent | `"parent"` | `NULL` |
| Take Profit | `"take_profit"` | Parent's `id` |
| Stop Loss | `"stop_loss"` | Parent's `id` |

### 6.2 Bracket Builder

```rust
pub struct BracketBuilder {
    symbol: String,
    action: Action,        // BUY or SELL for the parent
    quantity: f64,
    entry_type: EntryType, // Market or Limit(price)
    take_profit_price: f64,
    stop_loss_price: f64,
    // Optional
    stop_loss_type: StopLossType, // Stop (default) or StopLimit(limit_price)
    tif: TimeInForce,
    algo: Option<AlgoConfig>,
    tags: Vec<String>,
    strategy: Option<String>,
}

pub enum EntryType {
    Market,
    Limit(f64),
}

pub enum StopLossType {
    Stop,                    // STP order
    StopLimit { limit: f64 }, // STP LMT order
}

impl BracketBuilder {
    /// Creates three MidasOrder records (all in Draft state).
    /// Caller must call save() on each, then activate_bracket() on the parent.
    pub fn build(self) -> Result<(MidasOrder, MidasOrder, MidasOrder), ValidationError>;
}
```

### 6.3 Activating a Bracket

When activating a bracket, all three orders are sent to IB as a unit using the `transmit` flag:

```
activate_bracket(parent_order_id) ->
    Load parent order (assert Inactive)
    Load TP child (assert Inactive)
    Load SL child (assert Inactive)

    Acquire 3 consecutive ib_order_ids from IB client
    Assign: parent.ib_order_id = id,  tp.ib_order_id = id+1,  sl.ib_order_id = id+2

    Build ibapi::Order for parent:
        transmit = false
    Build ibapi::Order for TP:
        parent_id = parent.ib_order_id
        transmit = false
    Build ibapi::Order for SL:
        parent_id = parent.ib_order_id
        transmit = true   // Triggers transmission of all three

    Transition all three: Inactive -> PendingSubmit (in single SQLite transaction)

    Call client.place_order for parent
    Call client.place_order for TP
    Call client.place_order for SL   // This one triggers the bracket
```

### 6.4 Bracket Lifecycle After Activation

**When the parent fills:**
- IB automatically activates the TP and SL child orders.
- The TP and SL form an implicit OCA group — when one fills, IB cancels the other.
- Our `OrderUpdate` stream receives status changes for all three orders.
- The parent transitions: `PendingSubmit -> Submitted -> Filled`.
- The children transition: `PendingSubmit -> PreSubmitted -> Submitted` (after parent fills).

**When TP fills:**
- IB cancels the SL automatically (implicit OCA).
- TP transitions: `Submitted -> Filled`.
- SL transitions: `Submitted -> Cancelled`.

**When SL fills:**
- IB cancels the TP automatically.
- SL transitions: `Submitted -> Filled`.
- TP transitions: `Submitted -> Cancelled`.

**When the parent is cancelled before filling:**
- IB cancels all children automatically.
- All three transition to `Cancelled`.

### 6.5 Modifying Individual Bracket Legs

Individual legs of a live bracket can be modified independently:

- **Modifying TP price:** Call `modify_live()` on the TP order with new `limit_price`. IB accepts this as an in-place modification (same `orderId`).
- **Modifying SL price:** Call `modify_live()` on the SL order with new `aux_price`.
- **Modifying parent price (before fill):** Call `modify_live()` on the parent with new `limit_price`.
- **Modifying quantity:** Must modify all three legs simultaneously (quantity must match). Use `modify_bracket_quantity()` which updates all three in one operation.

```rust
impl OrderManager {
    /// Modify the take profit price of a bracket.
    pub async fn modify_bracket_tp(
        &self,
        parent_id: &OrderId,
        new_tp_price: f64,
    ) -> Result<()>;

    /// Modify the stop loss price of a bracket.
    pub async fn modify_bracket_sl(
        &self,
        parent_id: &OrderId,
        new_sl_price: f64,
    ) -> Result<()>;

    /// Modify quantity across all legs of a bracket.
    pub async fn modify_bracket_quantity(
        &self,
        parent_id: &OrderId,
        new_quantity: f64,
    ) -> Result<()>;

    /// Deactivate an entire bracket (cancel all legs at IB, return to Inactive).
    /// Only possible if parent has not yet filled.
    pub async fn deactivate_bracket(&self, parent_id: &OrderId) -> Result<()>;
}
```

### 6.6 Deactivating a Bracket

Deactivation of a bracket is only allowed if the parent has not filled:

- If the parent is `PreSubmitted` or `Submitted`: cancel all three at IB, transition all to `Inactive`.
- If the parent is `Filled`: the children are live exit orders. They cannot be "deactivated" back to Inactive because the position is real. They can only be individually cancelled or modified.
- If one child has already filled (position closed partially): the other child and the bracket structure are in a mixed state. The remaining child can be cancelled but the bracket is effectively dissolved.

---

## 7. Order Groups and Tags

### 7.1 Tags

Tags are user-defined string labels attached to orders. An order can have zero or more tags. Tags are stored as a JSON array in the `tags` column.

**Use cases:**
- `["earnings-play", "AAPL"]` — Categorize by trade thesis.
- `["scalp", "morning-session"]` — Categorize by strategy style and time.
- `["batch-2026-03-24"]` — Group orders created together.

**Operations:**
```rust
impl OrderManager {
    pub async fn add_tag(&self, order_id: &OrderId, tag: &str) -> Result<()>;
    pub async fn remove_tag(&self, order_id: &OrderId, tag: &str) -> Result<()>;
    pub async fn list_by_tag(&self, tag: &str) -> Result<Vec<MidasOrder>>;

    // Bulk operations by tag
    pub async fn activate_by_tag(&self, tag: &str) -> Result<BulkResult>;
    pub async fn deactivate_by_tag(&self, tag: &str) -> Result<BulkResult>;
    pub async fn cancel_by_tag(&self, tag: &str) -> Result<BulkResult>;
}
```

### 7.2 Strategy Grouping

The `strategy` field is a single string identifier that groups orders belonging to the same trading strategy. Unlike tags (which are ad-hoc), strategy is a structured first-class concept.

**Examples:** `"mean-reversion-spy"`, `"momentum-breakout"`, `"earnings-straddle"`.

**Operations:**
```rust
impl OrderManager {
    pub async fn list_by_strategy(&self, strategy: &str) -> Result<Vec<MidasOrder>>;
    pub async fn list_strategies(&self) -> Result<Vec<String>>;

    // Strategy-level bulk operations
    pub async fn activate_strategy(&self, strategy: &str) -> Result<BulkResult>;
    pub async fn deactivate_strategy(&self, strategy: &str) -> Result<BulkResult>;
    pub async fn cancel_strategy(&self, strategy: &str) -> Result<BulkResult>;

    // Strategy-level summary
    pub async fn strategy_summary(&self, strategy: &str) -> Result<StrategySummary>;
}

pub struct StrategySummary {
    pub strategy: String,
    pub total_orders: usize,
    pub by_state: HashMap<OrderState, usize>,
    pub total_filled_value: f64,
    pub total_commission: f64,
    pub realized_pnl: f64,
}
```

### 7.3 OCA Groups (IB-Level Grouping)

OCA (One-Cancels-All) groups are distinct from tags and strategies. They are an IB-level linking mechanism where filling one order causes the others to be cancelled or reduced.

**Local representation:**
- The `oca_group` field on each order stores a local group name (e.g., `"oca-entry-levels-AAPL"`).
- At activation time, the local group name is mapped to an IB-compatible OCA group string (typically the same string, but namespaced to avoid collisions across clients).
- The `oca_type` field controls behavior: `1` = CancelWithBlock, `2` = ReduceWithBlock, `3` = ReduceWithoutBlock.

**Activation of OCA groups:**
- All orders in the same OCA group must be activated together.
- The last order in the group has `transmit = true`; all preceding have `transmit = false`.
- If one order in the group is already live and a new order is added, the new order references the same `oca_group` string — IB will merge it into the existing group.

```rust
impl OrderManager {
    /// Activate all Inactive orders in the given OCA group as a unit.
    pub async fn activate_oca_group(&self, oca_group: &str) -> Result<BulkResult>;

    /// Deactivate all live orders in the given OCA group.
    pub async fn deactivate_oca_group(&self, oca_group: &str) -> Result<BulkResult>;
}
```

---

## 8. Reconciliation on Startup

### 8.1 The Problem

When `midas-broker` starts (or reconnects after a disconnect), the local SQLite state may be stale:

- Orders may have filled while disconnected.
- Orders may have been cancelled by IB (e.g., DAY orders expired overnight).
- Orders may have been placed or modified manually via TWS.
- The app may have crashed mid-state-transition, leaving orders in transient states (`PendingSubmit`, `PendingCancel`).

### 8.2 Reconciliation Flow

```
on_connect() ->
    |
    v
  Phase 1: Recover Transient States
    Load all orders in PendingSubmit or PendingCancel from SQLite
    These are orders that were mid-transition when the app stopped
    Mark them as needing reconciliation
    |
    v
  Phase 2: Fetch IB State
    Call client.all_open_orders() -> Vec<OrderData>
    Call client.completed_orders(api_only: false) -> Vec<OrderData>
    Build a HashMap<i32, IbOrderInfo> keyed by ib_order_id
    Also build a HashMap<i32, IbOrderInfo> keyed by ib_perm_id
    |
    v
  Phase 3: Match Local to IB
    For each local order with a non-NULL ib_order_id:
        Look up in IB open orders by ib_order_id
        If not found, look up in IB completed orders by ib_order_id
        If not found by ib_order_id, try ib_perm_id
        |
        +-> MATCHED to IB open order:
        |     Update local state to match IB status
        |     Update fill quantities from IB
        |     Log reconciliation to audit
        |
        +-> MATCHED to IB completed order:
        |     Update local state (Filled or Cancelled)
        |     Update fill quantities from IB
        |     Fetch executions for fill details
        |     Log reconciliation to audit
        |
        +-> NOT FOUND in IB:
              If local state was PendingSubmit:
                  -> IB never received it. Transition to Error("Lost during disconnect")
              If local state was Submitted/PreSubmitted:
                  -> IB cancelled it (DAY order expired, etc.)
                  -> Transition to Cancelled
                  -> Log "cancelled by IB while disconnected"
              If local state was PendingCancel:
                  -> Cancel was processed. Transition to Cancelled or Inactive
    |
    v
  Phase 4: Detect Orphan IB Orders
    For each IB open order NOT matched to a local order:
        This is an "orphan" — placed manually via TWS or by another client
        If auto_adopt is enabled:
            Create a new MidasOrder from the IB order data
            State = Submitted (or whatever IB reports)
            ib_order_id and ib_perm_id populated
            tags = ["adopted", "orphan"]
            Log to audit: "adopted orphan order from IB"
        Else:
            Log warning: "Untracked IB order: orderId={}, symbol={}, status={}"
            Add to reconciliation report for user review
    |
    v
  Phase 5: Subscribe to Updates
    Call client.order_update_stream() to begin receiving real-time updates
    From this point forward, all state transitions are driven by callbacks
    |
    v
  Phase 6: Emit Reconciliation Report
    Return ReconciliationReport to caller
```

### 8.3 Reconciliation Report

```rust
pub struct ReconciliationReport {
    pub timestamp: DateTime<Utc>,
    pub local_orders_checked: usize,
    pub ib_open_orders_found: usize,
    pub ib_completed_orders_found: usize,

    /// Orders whose local state was updated to match IB.
    pub state_corrections: Vec<StateCorrection>,

    /// Orders in IB with no local match.
    pub orphan_ib_orders: Vec<OrphanOrder>,

    /// Local orders with ib_order_id that could not be found in IB.
    pub missing_from_ib: Vec<MissingOrder>,

    /// Fill quantities that were updated during reconciliation.
    pub fill_corrections: Vec<FillCorrection>,

    /// Orders that were stuck in transient states and resolved.
    pub transient_resolutions: Vec<TransientResolution>,
}

pub struct StateCorrection {
    pub order_id: OrderId,
    pub ib_order_id: i32,
    pub local_state_before: OrderState,
    pub ib_reported_state: String,
    pub local_state_after: OrderState,
}

pub struct OrphanOrder {
    pub ib_order_id: i32,
    pub ib_perm_id: i64,
    pub symbol: String,
    pub action: String,
    pub order_type: String,
    pub quantity: f64,
    pub status: String,
    pub adopted: bool,
}

pub struct MissingOrder {
    pub order_id: OrderId,
    pub ib_order_id: i32,
    pub local_state: OrderState,
    pub resolution: MissingResolution,
}

pub enum MissingResolution {
    TransitionedToError(String),
    TransitionedToCancelled(String),
    TransitionedToInactive,
}
```

### 8.4 Reconciliation Configuration

```rust
pub struct ReconciliationConfig {
    /// Whether to automatically adopt orphan IB orders into the local store.
    /// Default: false. When false, orphans are reported but not adopted.
    pub auto_adopt_orphans: bool,

    /// Whether to automatically resolve orders stuck in PendingSubmit.
    /// Default: true. When true, PendingSubmit orders not found in IB -> Error.
    pub auto_resolve_pending_submit: bool,

    /// Whether to automatically resolve orders stuck in PendingCancel.
    /// Default: true. When true, PendingCancel orders not found in IB -> Cancelled/Inactive.
    pub auto_resolve_pending_cancel: bool,

    /// Whether to use clientId 0 to also capture manually-placed TWS orders.
    /// Default: false. When true, the reconciliation sees all orders, not just API orders.
    pub capture_manual_orders: bool,

    /// Maximum age of completed orders to fetch from IB (IB retains ~24 hours).
    /// Used to limit the completed_orders query scope.
    pub completed_orders_lookback: Duration,
}

impl Default for ReconciliationConfig {
    fn default() -> Self {
        Self {
            auto_adopt_orphans: false,
            auto_resolve_pending_submit: true,
            auto_resolve_pending_cancel: true,
            capture_manual_orders: false,
            completed_orders_lookback: Duration::from_secs(24 * 3600),
        }
    }
}
```

### 8.5 Ongoing Reconciliation

Beyond startup, a lightweight reconciliation runs periodically (default: every 60 seconds) while connected:

1. Call `client.open_orders()` to get current open order snapshot.
2. Compare against local orders in live states (`Submitted`, `PreSubmitted`, `PartiallyFilled`).
3. If any discrepancies are found (state mismatch, fill quantity mismatch), apply corrections and log to audit.
4. This catches edge cases where an `OrderUpdate` callback was missed (IB does not guarantee delivery of every intermediate status).

```rust
impl OrderManager {
    /// Run a lightweight reconciliation check against IB open orders.
    /// Called periodically by the background task.
    pub async fn periodic_reconciliation(&self) -> Result<ReconciliationReport>;
}
```

### 8.6 Handling the Daily Restart

IB Gateway/TWS performs a mandatory daily restart around 11:45 PM ET. The reconnection handler:

1. Detects disconnect via the `rust-ibapi` connection status.
2. Waits for reconnection (with exponential backoff).
3. On reconnect, runs the full startup reconciliation (Phase 1-6).
4. Re-subscribes to the `order_update_stream`.
5. Emits a `ReconciliationReport` to the application event bus.
6. GTC orders survive the restart at IB. DAY orders do not — they will appear as `Cancelled` in the completed orders query.

---

## Appendix A: Key `rust-ibapi` Types and Mapping

### A.1 Types Used from `rust-ibapi`

| `rust-ibapi` Type | Usage in `midas-broker` |
|---|---|
| `Client` | Held in `Arc<Client>` by `OrderManager`. All IB communication goes through this. |
| `Order` | Built from `MidasOrder` fields at activation time. Not stored — rebuilt each time. |
| `Contract` | Built from contract fields on `MidasOrder`. Qualified via `client.contract_details()`. |
| `OrderStatus` | Received via `OrderUpdate::OrderStatus`. Drives state machine transitions. |
| `OrderData` | Received via `OrderUpdate::OpenOrder`. Contains full order + contract + state. |
| `ExecutionData` | Received via `OrderUpdate::ExecutionData`. Used to create `OrderFill` records. |
| `CommissionReport` | Received via `OrderUpdate::CommissionReport`. Updates commission on fill records. |
| `PlaceOrder` | Subscription type returned by `client.place_order()`. |
| `CancelOrder` | Subscription type returned by `client.cancel_order()`. |
| `OrderUpdate` | Subscription type from `client.order_update_stream()`. Global order event feed. |
| `Action` | Enum: `Buy`, `Sell`, `SShort`, `SLong`. Mapped from `MidasOrder.action` string. |
| `TimeInForce` | Enum. Mapped from `MidasOrder.tif` string. |
| `OcaType` | Enum. Mapped from `MidasOrder.oca_type` integer. |
| `OrderBuilder` | Fluent builder for constructing `ibapi::Order`. Used internally. |
| `BracketOrderBuilder` | Builder for bracket orders. Used by our `BracketBuilder`. |
| `TagValue` | Key-value pair for algo params. Built from JSON in `MidasOrder.algo_params`. |

### A.2 ID Mapping

| Concept | Midas | IB | Notes |
|---|---|---|---|
| Internal order identity | `OrderId` (UUID v7) | N/A | Stable across activations. Never changes. |
| IB order identity | `ib_order_id: Option<i32>` | `orderId: i32` | Assigned at activation. Changes on each re-activation. |
| IB permanent identity | `ib_perm_id: Option<i64>` | `permId: i32` | Assigned by IB. Stable for the life of the order at IB. Stored as `i64` locally (widening from IB's `i32`) for consistency with SQLite INTEGER and future-proofing. Used as fallback reconciliation key. |
| IB client identity | `ib_client_id: Option<i32>` | `clientId: i32` | Identifies which API connection placed the order. |

### A.3 Converting `MidasOrder` to `ibapi::Order`

```rust
impl MidasOrder {
    /// Build an ibapi::Order from local order parameters.
    /// Called at activation time and for live modifications.
    pub fn to_ib_order(&self, transmit: bool) -> ibapi::orders::Order {
        let mut order = ibapi::orders::Order::default();

        order.order_id = self.ib_order_id.unwrap_or(0);
        order.action = match self.action.as_str() {
            "BUY" => ibapi::orders::Action::Buy,
            "SELL" => ibapi::orders::Action::Sell,
            _ => panic!("Invalid action: {}", self.action),
        };
        order.total_quantity = self.total_quantity;
        order.order_type = self.order_type.clone();
        order.limit_price = self.limit_price;
        order.aux_price = self.aux_price;
        order.tif = self.tif_to_ib();
        order.transmit = transmit;
        order.outside_rth = self.outside_rth;

        if let Some(ref parent_ib_id) = self.resolve_parent_ib_order_id() {
            order.parent_id = *parent_ib_id;
        }

        if !self.oca_group.is_empty() {
            order.oca_group = self.oca_group.clone();
            order.oca_type = self.oca_type_to_ib();
        }

        if let Some(ref algo) = self.algo_strategy {
            order.algo_strategy = algo.clone();
            order.algo_params = self.algo_params_to_ib();
        }

        order
    }
}
```

---

## Appendix B: Error Handling Strategy

### B.1 Error Types

```rust
#[derive(Debug, thiserror::Error)]
pub enum OrderError {
    #[error("Order not found: {0}")]
    NotFound(OrderId),

    #[error("Invalid state transition: {from:?} -> {to:?} for order {order_id}")]
    InvalidTransition {
        order_id: OrderId,
        from: OrderState,
        to: OrderState,
    },

    #[error("Order validation failed: {0:?}")]
    Validation(Vec<ValidationError>),

    #[error("IB rejected order {order_id}: {reason}")]
    IbRejected {
        order_id: OrderId,
        ib_order_id: i32,
        reason: String,
    },

    #[error("IB communication error: {0}")]
    IbConnection(#[from] ibapi::Error),

    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("Cannot deactivate partially filled order {0} (use cancel instead)")]
    CannotDeactivatePartialFill(OrderId),

    #[error("Cannot modify order in state {state:?}: {order_id}")]
    CannotModify {
        order_id: OrderId,
        state: OrderState,
    },

    #[error("Bracket integrity error: {0}")]
    BracketIntegrity(String),
}
```

### B.2 IB Error Code Handling

IB errors arrive as `OrderUpdate::Message(Notice)` with numeric error codes. Key codes to handle:

| Code | Meaning | Action |
|---|---|---|
| 103 | Duplicate order ID | Re-acquire `next_order_id`, retry activation. |
| 110 | Price out of range | Transition to `Error`. User must fix price. |
| 135 | Can't find order with ID | Order was already cancelled. Update local state. |
| 161 | Cancel attempted when not connected | Retry on reconnect. |
| 200 | No security definition found | Contract resolution failed. Transition to `Error`. |
| 201 | Order rejected — reason given | Transition to `Error` with reason. |
| 202 | Order cancelled | Transition to `Cancelled`. |
| 203 | Insufficient margin | Transition to `Error`. |
| 399 | Order message (warning, not error) | Log but do not change state. |
| 10147 | OrderId ... that needs to be cancelled is not found | Already gone. Update local state. |

---

## Appendix C: SQLite Performance Considerations

### C.1 Write-Ahead Logging

Enable WAL mode for concurrent reads during writes:

```sql
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;  -- Safe with WAL. FULL for maximum durability.
PRAGMA busy_timeout = 5000;   -- Wait up to 5s for locks.
```

### C.2 Connection Management

Uses `rusqlite` with `Arc<Mutex<Connection>>` (see 03-data-layer.md §3 for full pattern).
A connection pool is unnecessary because SQLite WAL mode supports only one writer at a time,
and the broker crate has a single event loop. Read concurrency is handled by WAL lock-free reads.

```rust
use std::sync::{Arc, Mutex};
use rusqlite::Connection;

let conn = Connection::open("data/midas-broker.db")?;
conn.pragma_update(None, "journal_mode", "WAL")?;
conn.pragma_update(None, "synchronous", "NORMAL")?;
conn.pragma_update(None, "busy_timeout", 5000)?;
let db = Arc::new(Mutex::new(conn));
```

All state transitions use this single connection (serialized via the Mutex). DB writes are
dispatched via `tokio::task::spawn_blocking` from the engine loop. Critical writes (state
transitions, fills) are awaited; non-critical writes (cache, metrics) are fire-and-forget.
See 01-architecture.md §5 for the two-tier write policy.

### C.3 Transaction Pattern for State Transitions

Every state transition is atomic: the order update and audit log insert happen in the same transaction.

```rust
fn transition(
    &self,
    id: &OrderId,
    from: OrderState,
    to: OrderState,
    trigger: &str,
    details: Option<&serde_json::Value>,
) -> Result<()> {
    let conn = self.db.lock().unwrap();
    let tx = conn.unchecked_transaction()?;

    // Optimistic concurrency: WHERE status = from ensures no race condition
    let rows = tx.execute(
        "UPDATE orders SET status = ?1, updated_at = ?2 WHERE id = ?3 AND status = ?4",
        rusqlite::params![
            to.to_string(),
            chrono::Utc::now().to_rfc3339(),
            id.as_str(),
            from.to_string(),
        ],
    )?;

    if rows == 0 {
        return Err(OrderError::InvalidTransition {
            order_id: id.clone(),
            from,
            to,
        });
    }

    tx.execute(
        "INSERT INTO order_audit (order_id, old_status, new_status, detail, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![
            id.as_str(),
            from.to_string(),
            to.to_string(),
            details.map(|d| d.to_string()),
            chrono::Utc::now().to_rfc3339(),
        ],
    )?;

    tx.commit()?;
    Ok(())
}
```

The `WHERE status = ?` clause implements optimistic concurrency control. If two threads try to transition the same order simultaneously, only one succeeds; the other receives `InvalidTransition` and must re-read the order to see the current state. Uses `unchecked_transaction()` because we already hold the Mutex lock (see 03-data-layer.md §3).
