# 02 — Broker Engine

> Engine command handling, IB API submission, and bracket lifecycle management
> for Market Order brackets.
>
> **Implementation Status (2026-04-02):**
> - Command variants (`CreateMarketBracket`, `CancelBracket`, `ModifyBracketLeg`): **COMPLETE**
> - Event variants (`BracketCreated`, `BracketStatusChanged`): **COMPLETE**
> - Engine handlers, IB submission, bracket builder: **NOT STARTED**
> - `engine.rs` currently has a `_ => debug!("handler not yet implemented")`
>   catch-all for all order/bracket commands

---

## Table of Contents

- [0. Engine Infrastructure Prerequisites](#0-engine-infrastructure-prerequisites)
- [1. Command Handler](#1-command-handler)
- [2. Bracket Builder](#2-bracket-builder)
- [3. IB API Submission](#3-ib-api-submission)
- [4. Bracket Lifecycle State Machine](#4-bracket-lifecycle-state-machine)
- [5. Status Callback Handler](#5-status-callback-handler)
- [6. Bracket Modification](#6-bracket-modification)
- [7. Bracket Cancellation](#7-bracket-cancellation)
- [8. Edge Cases](#8-edge-cases)
- [9. Error Recovery](#9-error-recovery)
- [10. Order Size Guard](#10-order-size-guard)

---

## 0. Engine Infrastructure Prerequisites

> Added 2026-04-02 during plan refinement. The engine currently lacks the
> foundational infrastructure that bracket handling depends on.

The `BrokerEngine` struct in `crates/midas-broker/src/engine.rs` currently
has no order management infrastructure. Before bracket-specific handlers
can be implemented, the engine needs:

1. **`store` field** — A `BrokerDb` or persistence handle for SQLite access.
   The bracket persistence functions (`persist_and_transition_to_pending_submit`,
   etc.) require a `&Connection`.

2. **`client` field** — An `Arc<ibapi::Client>` (or trait-abstracted equivalent)
   for IB API calls. `place_order`, `cancel_order`, `next_order_id` all need this.

3. **`bracket_status_cache` field** — `HashMap<Uuid, BracketLifecycleStatus>`
   to track last-emitted bracket status and prevent duplicate events.

4. **`ibapi` crate version** — Pin to verified version. The plan's IB API
   patterns were verified against `rust-ibapi` v2.10.0. `Cargo.toml` currently
   specifies `ibapi = "2"` which could resolve to any 2.x. **Action (Phase 2
   task)**: Change `crates/midas-broker/Cargo.toml` to `ibapi = "2.10"` to
   prevent API surprises from `cargo update`.

5. **`spawn_blocking` for IB API calls** — The `rust-ibapi` crate uses
   synchronous TCP I/O. All IB API calls (`place_order`, `cancel_order`,
   `next_order_id`) must be wrapped in `tokio::task::spawn_blocking()` to
   avoid blocking the async runtime. For bracket submission, wrap the entire
   `submit_bracket_to_ib` function body in a single `spawn_blocking` call
   (not per-call wrapping) — the calls are sequential with shared state,
   total duration is <100ms, and this is consistent with the existing
   `BrokerDb` pattern for SQLite access.

These fields and the basic `handle_command` dispatch for `CreateOrder`,
`ActivateOrder`, `CancelOrder`, and `ModifyOrder` may be implemented as
part of the general order management infrastructure or as part of this
plan's Phase 2. Either way, they are prerequisites for the bracket handlers
described in §1 below.

---

## 1. Command Handler

### 1.1 Entry Point

When the engine receives `BrokerCommand::CreateMarketBracket(params)`:

```
CreateMarketBracket(params)
    │
    ▼
validate_params(&params)
    │ ── fail ──> emit OrderError, return
    ▼
build_bracket_orders(&params) -> BracketGroup (all legs in Inactive status)
    │
    ▼
persist_and_transition_to_pending_submit(group)
    │   Single atomic SQLite transaction:
    │   INSERT all legs + transition Inactive → PendingSubmit + audit log
    │   (orders are in PendingSubmit when this returns)
    │
    ▼
emit BracketCreated event
    │
    ▼
submit_bracket_to_ib(group)
    │ ── fail ──> cancel any already-placed IB orders,
    │              transition all from PendingSubmit to Error, emit OrderError
    ▼
emit OrderSubmitted for each leg
```

**Key ordering**: Orders are persisted to SQLite as `PendingSubmit` in a
**single atomic transaction** before any `place_order` call to IB. This is
the persist-first pattern from the existing architecture
(`02-order-management.md` §4.1). There is no intermediate state between
persist and PendingSubmit — both happen atomically, so a crash cannot leave
orders in `Inactive` with IB order IDs assigned. If the process crashes
after persisting but before IB placement, reconciliation on restart finds the
orders in `PendingSubmit` and can clean them up. If IB placement partially
fails (e.g., parent placed but TP fails), we cancel the already-placed orders
at IB and transition all legs from `PendingSubmit` to `Error` — we do NOT
roll back SQLite.

### 1.2 Validation

```rust
fn validate_market_bracket(params: &MarketBracketParams) -> Result<(), Vec<ValidationError>> {
    let mut errors = Vec::new();

    // Contract
    if params.symbol.is_empty() {
        errors.push(ValidationError::MissingSymbol);
    }

    // Quantity
    if params.quantity <= 0.0 {
        errors.push(ValidationError::InvalidQuantity);
    }

    // TP price sanity
    if let Some(ref tp) = params.take_profit {
        if tp.price <= 0.0 {
            errors.push(ValidationError::InvalidPrice("take_profit"));
        }
    }

    // SL price sanity
    if let Some(ref sl) = params.stop_loss {
        if sl.stop_price <= 0.0 {
            errors.push(ValidationError::InvalidPrice("stop_loss"));
        }
        // StopLimit: limit must be at or below stop for sells, at or above for buys
        if let Some(limit) = sl.limit_price {
            if limit <= 0.0 {
                errors.push(ValidationError::InvalidPrice("stop_loss_limit"));
            }
        }
    }

    // Warn (not error) if no TP and no SL — naked market order
    // The engine logs a warning but proceeds.

    if errors.is_empty() { Ok(()) } else { Err(errors) }
}
```

### 1.3 Directional Validation

Validate that TP/SL are on the correct side relative to a reference price.

**For market orders**: Directional validation is performed **client-side in
the order panel** (using the last traded price) as a hard reject. At the
**engine level**, it is a **warning only** (logged, not rejected). This is
because the last price can move between the UI click and engine receipt —
a transient price spike could cause a valid bracket to be rejected. The
fill price is unknown at submission time for market orders.

**For limit orders** (future): Directional validation at the engine level
is a hard reject because the entry price is known.

```rust
/// Returns a list of directional warnings. Empty list = all OK.
/// For market orders, these are warnings (logged, not rejected).
/// For limit orders, these would be errors.
fn check_bracket_direction(
    action: OrderAction,
    reference_price: f64,  // last traded price (estimate for market)
    tp_price: Option<f64>,
    sl_price: Option<f64>,
) -> Vec<DirectionWarning> {
    let mut warnings = Vec::new();
    match action {
        OrderAction::Buy => {
            if let Some(tp) = tp_price {
                if tp <= reference_price {
                    warnings.push(DirectionWarning::TpBelowReference { tp, reference_price });
                }
            }
            if let Some(sl) = sl_price {
                if sl >= reference_price {
                    warnings.push(DirectionWarning::SlAboveReference { sl, reference_price });
                }
            }
        }
        OrderAction::Sell => {
            if let Some(tp) = tp_price {
                if tp >= reference_price {
                    warnings.push(DirectionWarning::TpAboveReference { tp, reference_price });
                }
            }
            if let Some(sl) = sl_price {
                if sl <= reference_price {
                    warnings.push(DirectionWarning::SlBelowReference { sl, reference_price });
                }
            }
        }
    }
    warnings
}
```

If warnings are non-empty, the engine logs them at `warn!` level and
proceeds with submission. The UI's client-side validation (which uses the
freshest price) is the primary guard.

---

## 2. Bracket Builder

### 2.1 Building the Three Orders

```rust
fn build_market_bracket(params: &MarketBracketParams) -> BracketGroup {
    let now = Utc::now();
    let parent_id = Uuid::now_v7();

    // -- Parent: Market Order --
    // IMPORTANT: We use new_draft() then immediately set status to Inactive.
    // This respects the state machine: Draft is transient (never persisted),
    // Inactive is the first persisted state, and Inactive->PendingSubmit is
    // a legal transition. We skip the Draft state entirely.
    let mut parent = LocalOrder::new_draft(
        &params.symbol,
        params.action,
        OrderKind::Market,
        params.quantity,
    );
    parent.id = parent_id;
    parent.status = OrderStatus::Inactive; // Skip Draft — goes straight to Inactive
    parent.con_id = params.con_id;
    parent.sec_type = params.sec_type;
    parent.exchange = params.exchange.clone();
    parent.currency = params.currency.clone();
    parent.outside_rth = params.outside_rth;
    parent.bracket_role = Some(BracketRole::Parent);
    parent.strategy = params.strategy.clone();
    parent.tags = params.tags.clone();

    // -- Take Profit: Limit Order (opposite side) --
    let take_profit = params.take_profit.as_ref().map(|tp| {
        let opposite = match params.action {
            OrderAction::Buy => OrderAction::Sell,
            OrderAction::Sell => OrderAction::Buy,
        };
        let mut order = LocalOrder::new_draft(
            &params.symbol,
            opposite,
            OrderKind::Limit,
            params.quantity,
        );
        order.status = OrderStatus::Inactive;
        order.con_id = params.con_id;
        order.sec_type = params.sec_type;
        order.exchange = params.exchange.clone();
        order.currency = params.currency.clone();
        order.limit_price = Some(tp.price);
        order.tif = tp.tif.unwrap_or(TimeInForce::Gtc);
        order.parent_id = Some(parent_id);
        order.bracket_role = Some(BracketRole::TakeProfit);
        order.strategy = params.strategy.clone();
        order.tags = params.tags.clone();
        order
    });

    // -- Stop Loss: Stop or StopLimit Order (opposite side) --
    let stop_loss = params.stop_loss.as_ref().map(|sl| {
        let opposite = match params.action {
            OrderAction::Buy => OrderAction::Sell,
            OrderAction::Sell => OrderAction::Buy,
        };
        let kind = if sl.limit_price.is_some() {
            OrderKind::StopLimit
        } else {
            OrderKind::Stop
        };
        let mut order = LocalOrder::new_draft(
            &params.symbol,
            opposite,
            kind,
            params.quantity,
        );
        order.status = OrderStatus::Inactive;
        order.con_id = params.con_id;
        order.sec_type = params.sec_type;
        order.exchange = params.exchange.clone();
        order.currency = params.currency.clone();
        order.stop_price = Some(sl.stop_price);
        order.limit_price = sl.limit_price;  // None for Stop, Some for StopLimit
        order.tif = sl.tif.unwrap_or(TimeInForce::Gtc);
        order.parent_id = Some(parent_id);
        order.bracket_role = Some(BracketRole::StopLoss);
        order.strategy = params.strategy.clone();
        order.tags = params.tags.clone();
        order
    });

    BracketGroup { parent, take_profit, stop_loss }
}
```

**State machine compliance**: All orders are created in `Inactive` status.
The engine then transitions `Inactive -> PendingSubmit` (a legal transition
per the existing state machine in `state.rs`) before calling `place_order`.
No new state transitions need to be added to `validate_transition()`.

---

## 3. IB API Submission

### 3.1 Transmission Strategy

IB brackets use the `transmit` flag to send all legs atomically:

1. **Parent** (Market): `transmit = false` — queued at IB, not yet sent to exchange
2. **TP** (Limit): `transmit = false`, `parent_id = parent.ib_order_id`
3. **SL** (Stop): `transmit = true`, `parent_id = parent.ib_order_id` — triggers all three

If there is no SL (only TP), the TP gets `transmit = true`.
If there is no TP (only SL), the SL gets `transmit = true`.
If neither TP nor SL, the parent itself gets `transmit = true` (plain market order).

### 3.2 Why Not `ibapi::BracketOrderBuilder`?

The `rust-ibapi` crate provides a `BracketOrderBuilder` with a `submit_all()`
method. This plan constructs `ibapi::Order` structs manually instead because:

1. **Partial-failure cleanup**: `submit_all()` does not cancel already-placed
   legs when a subsequent `place_order` fails. Our error handling (cancel
   queued orders + transition all to Error) requires per-leg control.
2. **Optional TP/SL**: The builder always creates exactly 3 legs. Our brackets
   allow TP-only, SL-only, or even naked market orders (1 or 2 legs).
3. **Persist-before-submit**: We must persist all legs as `PendingSubmit` in a
   single atomic SQLite transaction between building and placing. The builder
   has no hook for this intermediate step.

The manual approach follows the same per-leg `next_order_id()` pattern that
`BracketOrderBuilder::submit_all()` uses internally (verified in `rust-ibapi`
v2.10.0 source), so IB compatibility is identical.

### 3.3 Submission Flow

> **Implementation note**: The `rust-ibapi` crate uses synchronous TCP I/O.
> The entire body of `submit_bracket_to_ib` (Steps 1-7) must execute inside
> a single `tokio::task::spawn_blocking()` call. The code below is shown
> without the wrapper for readability. See §0 prerequisite 5 for details.

```rust
async fn submit_bracket_to_ib(
    &self,
    group: &mut BracketGroup,
    client: &Arc<Client>,
) -> Result<()> {
    // Entire body runs inside spawn_blocking — see §0.5
    // ── Step 1: Acquire IB order IDs (one per leg) ─────────────────
    // IMPORTANT: Call next_order_id() once per leg. Do NOT assume
    // consecutive IDs by calling once and adding offsets — concurrent
    // ActivateOrder commands could claim intervening IDs.
    let parent_ib_id = client.next_order_id();
    group.parent.ib_order_id = Some(parent_ib_id);

    if let Some(ref mut tp) = group.take_profit {
        tp.ib_order_id = Some(client.next_order_id());
    }
    if let Some(ref mut sl) = group.stop_loss {
        sl.ib_order_id = Some(client.next_order_id());
    }

    // ── Step 2: Persist all legs as PendingSubmit (BEFORE IB calls) ─
    // Single SQLite transaction: Inactive -> PendingSubmit for all legs.
    // This is the persist-first pattern. If the process crashes after
    // this but before IB placement, reconciliation on restart finds the
    // orders in PendingSubmit and cleans them up.
    self.store.transition_bracket_to_pending_submit(group)?;

    // ── Step 3: Determine which leg transmits ──────────────────────
    let has_children = group.take_profit.is_some() || group.stop_loss.is_some();

    // ── Step 4: Build IB contract ──────────────────────────────────
    let contract = build_ib_contract(&group.parent);

    // ── Step 5: Place parent ───────────────────────────────────────
    let mut parent_ib = build_ib_order(&group.parent);
    parent_ib.transmit = !has_children; // true only if no children
    if let Err(e) = client.place_order(
        group.parent.ib_order_id.unwrap(),
        &contract,
        &parent_ib,
    ) {
        // Parent failed — no orders at IB, transition all to Error
        self.store.transition_bracket_to_error(group, &e.to_string())?;
        return Err(e.into());
    }

    // ── Step 6: Place TP (if present) ──────────────────────────────
    if let Some(ref tp) = group.take_profit {
        let mut tp_ib = build_ib_order(tp);
        tp_ib.parent_id = group.parent.ib_order_id.unwrap();
        tp_ib.transmit = group.stop_loss.is_none(); // last child?
        if let Err(e) = client.place_order(
            tp.ib_order_id.unwrap(),
            &contract,
            &tp_ib,
        ) {
            // TP failed — parent is at IB with transmit=false. Cancel it.
            // NOTE: rust-ibapi v2 cancel_order takes only the order ID.
            let _ = client.cancel_order(parent_ib_id);
            self.store.transition_bracket_to_error(group, &e.to_string())?;
            return Err(e.into());
        }
    }

    // ── Step 7: Place SL (if present — always last, transmit=true) ─
    if let Some(ref sl) = group.stop_loss {
        let mut sl_ib = build_ib_order(sl);
        sl_ib.parent_id = group.parent.ib_order_id.unwrap();
        sl_ib.transmit = true;
        if let Err(e) = client.place_order(
            sl.ib_order_id.unwrap(),
            &contract,
            &sl_ib,
        ) {
            // SL failed — parent (and maybe TP) are at IB. Cancel them.
            let _ = client.cancel_order(parent_ib_id);
            if let Some(ref tp) = group.take_profit {
                let _ = client.cancel_order(tp.ib_order_id.unwrap());
            }
            self.store.transition_bracket_to_error(group, &e.to_string())?;
            return Err(e.into());
        }
    }

    Ok(())
}
```

**Error handling during submission**: If any `place_order` call fails mid-way:
1. Cancel any orders already placed at IB (they have `transmit=false`, so they
   are queued but not at the exchange).
2. Transition all legs to `Error` in SQLite (do NOT rollback — the persist
   already happened and the audit trail should reflect the attempt).
3. Return the error. The user can review and retry via the UI.

### 3.4 IB Order Field Mapping

| LocalOrder field | ibapi::Order field | Notes |
|---|---|---|
| `order_type: Market` | `order_type: "MKT"` | |
| `order_type: Limit` | `order_type: "LMT"` | TP child |
| `order_type: Stop` | `order_type: "STP"` | SL child |
| `order_type: StopLimit` | `order_type: "STP LMT"` | SL child with limit |
| `action: Buy` | `action: "BUY"` | |
| `action: Sell` | `action: "SELL"` | |
| `quantity` | `total_quantity` | |
| `limit_price` | `lmt_price` | TP child, SL StopLimit |
| `stop_price` | `aux_price` | SL child |
| `tif` | `tif` | |
| `outside_rth` | `outside_rth` | |
| `parent_id` → ib_order_id | `parent_id` (i32) | Children reference parent's IB ID |

---

## 4. Bracket Lifecycle State Machine

### 4.1 Bracket-Level States

These are **derived** from the individual order statuses, not stored. The
engine computes them when emitting `BracketStatusChanged`:

```
                     ┌────────────┐
                     │  Submitted │  Parent is PendingSubmit/Submitted
                     │            │  Children are PendingSubmit/PreSubmitted
                     └─────┬──────┘
                           │
                           │ Parent fills (remaining_qty == 0)
                           ▼
                     ┌────────────┐
                     │   Active   │  Parent is Filled
                     │            │  Children are Submitted (live at exchange)
                     └─────┬──────┘
                           │
                ┌──────────┼──────────┐
                │          │          │
                ▼          ▼          ▼
         ┌──────────┐ ┌────────┐ ┌───────────┐
         │ TP Hit   │ │SL Hit  │ │ Cancelled │
         │(Closed)  │ │(Closed)│ │           │
         └──────────┘ └────────┘ └───────────┘
```

### 4.2 Deriving Bracket Status

```rust
/// Derive bracket lifecycle status from individual order statuses.
/// Returns a typed enum — no string matching needed in the UI layer.
///
/// SAFETY: This function must NEVER panic. It runs on every order status
/// callback in the trading engine. An unexpected state logs a warning
/// and returns Closed as a safe fallback.
fn derive_bracket_status(group: &BracketGroup) -> BracketLifecycleStatus {
    let parent = &group.parent;

    // Parent cancelled
    if parent.status == OrderStatus::Cancelled {
        return BracketLifecycleStatus::Cancelled;
    }

    // Parent rejected
    if parent.status == OrderStatus::Rejected {
        return BracketLifecycleStatus::Rejected;
    }

    // IMPORTANT: Error check must precede is_terminal() check below.
    // Error is non-terminal in the state machine (it's retryable), but
    // should NOT map to Submitted. Without this ordering, Error parents
    // would fall through to the is_terminal() gate and return Submitted.
    if parent.status == OrderStatus::Error {
        return BracketLifecycleStatus::Error;
    }

    // Parent still working (not terminal — and not Error, handled above)
    if !parent.status.is_terminal() {
        return BracketLifecycleStatus::Submitted;
    }

    // Parent is terminal — should be Filled at this point.
    // Use match instead of assert to avoid panics in the trading engine.
    match parent.status {
        OrderStatus::Filled => { /* expected — continue checking children */ }
        other => {
            tracing::warn!(
                "Unexpected terminal parent status in bracket {}: {:?}",
                group.parent.id, other
            );
            return BracketLifecycleStatus::Closed;
        }
    }

    // Check if any child is in Error state (e.g., modification failed locally).
    // Surface this so the UI can alert the user.
    let child_error = [&group.take_profit, &group.stop_loss]
        .iter()
        .filter_map(|c| c.as_ref())
        .any(|c| c.status == OrderStatus::Error);
    if child_error {
        return BracketLifecycleStatus::Error;
    }

    // Check if TP hit
    if let Some(ref tp) = group.take_profit {
        if tp.status == OrderStatus::Filled {
            return BracketLifecycleStatus::TakeProfitHit;
        }
    }

    // Check if SL hit
    if let Some(ref sl) = group.stop_loss {
        if sl.status == OrderStatus::Filled {
            return BracketLifecycleStatus::StopLossHit;
        }
    }

    // Children still live at exchange
    let any_live = group.legs().iter().any(|o| o.status.is_live_at_ib());
    if any_live {
        return BracketLifecycleStatus::EntryFilled;
    }

    // All terminal but not caught above — edge case
    BracketLifecycleStatus::Closed
}
```

---

## 5. Status Callback Handler

### 5.1 Per-Order Processing

The existing `handle_order_status_update()` in the engine processes IB callbacks
for each order individually. For bracket support, add a **post-processing step**
that checks if the updated order is part of a bracket:

```rust
async fn handle_order_status(&self, ib_order_id: i32, status: &str, ...) {
    // 0. GUARD: Ignore IB callbacks for orders already in a terminal state.
    // This handles the race where the engine force-transitions children
    // (e.g., to Rejected on parent rejection) and IB subsequently sends
    // its own Cancelled callback for those same children. Without this
    // guard, validate_transition(Rejected, Cancelled) would fail.
    let order = self.store.get_by_ib_order_id(ib_order_id)?;
    if order.status.is_terminal() {
        tracing::debug!(
            "Ignoring IB status '{}' for order {} — already terminal ({:?})",
            status, order.id, order.status
        );
        return;
    }

    // 1. Existing logic: map status, transition, emit OrderStatusChanged
    // ... existing transition logic ...

    // 2. NEW: If this order is part of a bracket, check bracket-level status
    if order.bracket_role.is_some() {
        self.check_bracket_status_change(&order).await;
    }
}

async fn check_bracket_status_change(&self, order: &LocalOrder) {
    // Determine parent_id
    let parent_id = match order.bracket_role {
        Some(BracketRole::Parent) => order.id,
        Some(_) => order.parent_id.expect("child must have parent_id"),
        None => return,
    };

    // Load full bracket group
    let group = self.load_bracket_group(parent_id)?;
    let new_status = derive_bracket_status(&group);

    // Compare with last emitted status (cached in engine state)
    let prev = self.bracket_status_cache.get(&parent_id);
    if prev.copied() != Some(new_status) {
        self.bracket_status_cache.insert(parent_id, new_status);

        let entry_fill_price = group.parent.avg_fill_price;

        self.order_events.send(BrokerEvent::BracketStatusChanged {
            parent_id,
            status: new_status,
            entry_fill_price,
        })?;
    }
}
```

### 5.2 Market Order Fill Handling

Market orders typically fill instantly (single fill, `remaining == 0`).
But they can have multiple partial fills if the quantity is large or
liquidity is thin. The engine handles this correctly because:

1. Each fill triggers `OrderStatusChanged` → `PartiallyFilled` or `Filled`
2. The bracket status check only transitions to `EntryFilled` when
   `parent.status == Filled` (all quantity executed)
3. IB activates children only after the parent fully fills

**Edge case**: If a market order partially fills and then is cancelled
(extremely rare), the children never activate. IB auto-cancels them.

---

## 6. Bracket Modification

### 6.1 Modifying TP Price

After the parent fills and TP/SL are live:

```
ModifyBracketLeg { order_id: tp_uuid, new_price: 195.00 }
    │
    ▼
Load order (assert bracket_role == TakeProfit or StopLoss)
    │
    ▼
Bracket-level policy check:
    Load parent order via parent_id
    Assert parent.status == Filled
    (This is a bracket policy, not a state machine constraint —
     the per-order state machine allows modifying PreSubmitted orders
     via can_modify_at_ib(), but bracket UX requires the parent to
     fill first so the entry price is known for R:R display.)
    │
    ▼
Assert child status is Submitted or PreSubmitted (live/queued at exchange)
    (Stop orders sit in PreSubmitted until their trigger price is hit —
     this is their normal resting state. can_modify_at_ib() returns
     true for both PreSubmitted and Submitted.)
    │
    ▼
Update limit_price = 195.00
    │
    ▼
Send modification to IB:
    client.place_order(same ib_order_id, &contract, &updated_order)
    │
    ▼
Emit OrderStatusChanged
Update annotation in chart (via app layer)
```

### 6.2 Modifying SL Price

Same flow but updates `stop_price` (and `limit_price` for StopLimit).

### 6.3 Modification Constraints

- Cannot modify the parent after fill (it's terminal)
- Cannot modify TP/SL while parent is still working — the bracket-level
  policy requires `parent.status == Filled` before allowing child
  modifications (§6.1). This is a UX constraint, not a state machine
  constraint — `can_modify_at_ib()` returns true for PreSubmitted
  children, but modifying before the parent fills is confusing because
  there is no entry price for R:R display.
- Quantity modification requires changing both TP and SL simultaneously
  (they must match). Use `ModifyOrder` on each leg.
- **PartiallyFilled children**: In rare cases (large orders, illiquid
  instruments), a TP or SL child could be in `PartiallyFilled` status.
  The current `can_modify_at_ib()` predicate returns `false` for
  `PartiallyFilled`, but IB does allow modifying partially filled orders.
  For bracket children this could prevent a trader from adjusting risk
  when they need to most. **Resolution**: Extend `can_modify_at_ib()` in
  `state.rs` to include `PartiallyFilled` — this matches IB's actual
  behavior and is safe because the modification is a `place_order` call
  with the same `ib_order_id` (IB handles the remaining quantity).

---

## 7. Bracket Cancellation

### 7.1 Cancel Entire Bracket

```
CancelBracket { parent_id }
    │
    ▼
Load bracket group
    │
    ▼
If parent is live (not yet filled):
    Cancel parent → IB auto-cancels children
    Transition parent: current → PendingCancel
    (children will transition via IB callbacks)
    │
If parent is filled (TP/SL are live):
    Cancel TP (if live)
    Cancel SL (if live)
    (parent stays Filled — it's terminal)
    │
    ▼
Emit BracketStatusChanged { status: Cancelled }
```

### 7.2 Cancel Individual Leg

Cancelling a single TP or SL while the other remains:

- If TP is cancelled and SL is still live → SL remains as standalone protection
- If SL is cancelled and TP is still live → TP remains (risky — no downside protection)
- The bracket is effectively degraded but not fully cancelled

The engine logs a warning when SL is cancelled individually (leaving position
unprotected).

---

## 8. Edge Cases

### 8.1 Market Order Rejected

If IB rejects the market order (insufficient margin, market closed, etc.):
- Parent → `Rejected`
- Children remain in `PendingSubmit` (if the parent was rejected before
  IB transmitted the bracket) or `PreSubmitted` (if the parent was
  rejected by the exchange after bracket transmission). Because the
  parent is placed with `transmit=false`, the full bracket is not
  transmitted until the final child with `transmit=true` is placed —
  so children cannot advance beyond `PreSubmitted` before the parent
  is validated by IB/the exchange.
- Engine force-transitions children to `Rejected` (valid from both
  `PendingSubmit` and `PreSubmitted` per the state machine; using `Rejected`
  rather than `Cancelled` because `PendingSubmit → Cancelled` is not a legal
  transition — it must go through `PendingCancel` first)
- **IB callback race**: After the engine force-transitions children to
  `Rejected`, IB may subsequently send its own `Cancelled` callbacks for
  those children. The terminal-state guard in `handle_order_status` (§5.1)
  silently ignores these late callbacks.
- Emit `BracketStatusChanged { status: Rejected }`

### 8.2 Market Order Partially Fills Then Gets Cancelled

Extremely rare for market orders, but possible during halt/close:
- Parent → `Cancelled` with `filled_qty > 0`
- IB auto-cancels children
- Position exists but bracket is dead
- Engine emits warning: "partial fill on market bracket — position open without protection"

### 8.3 Child Order Rejected After Parent Fills

If IB rejects a TP or SL child (e.g., price too far from market):
- The rejected child → `Rejected`
- The other child remains live
- Engine emits `OrderRejected` for the failed child
- App layer should notify user: "TP rejected — position has SL only"

### 8.4 Network Disconnect During Submission

If connection drops between placing parent and placing SL:
- Parent may be at IB (with `transmit=false`, so not yet at exchange)
- Children may not have been sent
- On reconnect: reconcile by checking open orders at IB
- **Bracket-specific reconciliation**: Check for orders in `PendingSubmit`
  with `bracket_role = PARENT` where not all expected children (based on
  the original `MarketBracketParams` — or simply checking `parent_id`
  references in the DB) have been placed at IB. If a parent has no
  children at IB despite expecting them, it is stale — cancel it at IB.
- Transition all to `Error` and let user retry

### 8.5 Race: Fill Arrives Before All place_order Calls Complete

With `transmit=false` on parent, IB queues it but does not send to exchange.
Only when the final leg with `transmit=true` arrives does IB transmit the
full bracket. So the parent cannot fill before all legs are placed.

This is the entire point of the `transmit` flag — it prevents race conditions.

---

## 9. Error Recovery

### 9.1 Retry Strategy

Market brackets are **not retried automatically**. Market conditions change
too fast for a stale bracket to be valid. On error:

1. All legs transition to `Error`
2. `BracketStatusChanged { status: Error }` emitted
3. User must review and decide whether to re-submit

### 9.2 Reconciliation on Startup

When the engine reconnects after a restart:

1. Request open orders from IB (`reqOpenOrders`)
2. For each open order with a `parentId`, look up the parent in our DB
3. If the parent exists locally: rebuild the `BracketGroup` and cache status
4. If the parent is unknown: log warning (order placed outside our system)

This is part of the general reconciliation flow (§8 in `02-order-management.md`).
Bracket-specific reconciliation (detecting stale un-transmitted parents) is
described in §8.4 above.

---

## 10. Order Size Guard

> Added 2026-04-02 during plan refinement. Market orders execute immediately
> with no review step — an engine-level guard against catastrophically large
> orders is essential.

### 10.1 Configurable Limits

Add engine-level maximum order size validation that runs **before** bracket
building and IB submission. This is a hard reject, not a UI-only warning.

```toml
[trading.limits]
# Maximum quantity per order (shares/contracts). 0 = no limit.
max_order_quantity = 10000
# Maximum notional value per order (quantity × reference price). 0 = no limit.
max_notional_value = 500000
```

### 10.2 Reference Price Source

For market orders, the entry price is unknown at submission time. The
`reference_price` for notional calculation comes from the order panel's
`last_price` field (populated from chart candle data — see §1.4 in
`04-order-entry-ui.md`). This price may lag by the bar interval but is
accurate enough for a safety guard.

To pass the reference price through to the engine, add an optional
`reference_price: Option<f64>` field to `MarketBracketParams`.

**When reference_price is None** (no chart loaded, pre-market, no data):
The engine rejects the order if `max_notional_value > 0`. Rationale: a
safety feature that can be silently bypassed defeats its purpose. The
quantity-only guard still applies regardless.

### 10.3 Validation

```rust
/// Engine-level order size guard. Hard reject — not bypassable from UI.
/// Runs before build_market_bracket().
fn validate_order_size(
    params: &MarketBracketParams,
    limits: &TradingLimits,
) -> Result<(), OrderSizeError> {
    if limits.max_order_quantity > 0.0 && params.quantity > limits.max_order_quantity {
        return Err(OrderSizeError::QuantityExceedsLimit {
            quantity: params.quantity,
            limit: limits.max_order_quantity,
        });
    }

    if limits.max_notional_value > 0.0 {
        match params.reference_price {
            Some(price) => {
                let notional = params.quantity * price;
                if notional > limits.max_notional_value {
                    return Err(OrderSizeError::NotionalExceedsLimit {
                        notional,
                        limit: limits.max_notional_value,
                    });
                }
            }
            None => {
                tracing::warn!(
                    "No reference price for {} — rejecting (notional guard requires price)",
                    params.symbol
                );
                return Err(OrderSizeError::MissingReferencePrice {
                    symbol: params.symbol.clone(),
                });
            }
        }
    }

    Ok(())
}
```

### 10.4 Rationale

This guard protects against:
- **UI bugs**: A malformed quantity field sending 1,000,000 instead of 100
- **Programmatic misuse**: Future strategy automation submitting oversized orders
- **Quick trade mode**: Bypasses the order panel entirely — no human review

The confirmation dialog (§5 in `04-order-entry-ui.md`) can be disabled via
settings, and quick trade mode (§6) is designed for speed over review. An
engine-level guard is the last line of defense before real money is at risk.

The limits are configurable (not hardcoded) because appropriate limits vary
by account size and instrument. Defaults are conservative.
