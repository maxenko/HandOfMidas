# 02 — Order Simulation

> Order lifecycle simulation, fill engine, bracket mechanics, and edge cases.

---

## Table of Contents

- [1. Order State Machine](#1-order-state-machine)
- [2. Bracket Lifecycle](#2-bracket-lifecycle)
- [3. Fill Execution](#3-fill-execution)
- [4. Order Modification](#4-order-modification)
- [5. Cancellation](#5-cancellation)
- [6. Edge Cases](#6-edge-cases)
- [7. Error Injection](#7-error-injection)

---

## 1. Order State Machine

The test broker simulates the exact IB order state machine:

```
place_order(transmit=false)
    → Held (not transmitted, waiting for bracket completion)

place_order(transmit=true)  [or bracket's last child transmitted]
    │
    ├─ MKT order:
    │   → Submitted → Filled (instant or delayed)
    │
    ├─ LMT order:
    │   ├─ Marketable (price crosses market): → Submitted → Filled
    │   └─ Away from market: → Submitted → [waits for price trigger]
    │
    └─ STP/STP LMT order:
        → PreSubmitted → [waits for stop trigger] → Submitted → Filled
```

### 1.1 Status Callback Sequence

For each transition, the test broker pushes callbacks to the queue in the
exact order IB would:

**Market order (instant fill):**
```
1. OrderStatus { status: "Submitted", filled: 0, remaining: 100 }
2. Execution  { shares: 100, price: 185.50, commission: 1.00 }
3. OrderStatus { status: "Filled", filled: 100, remaining: 0, avg_fill_price: 185.50 }
```

**Limit order (away from market, then fills):**
```
1. OrderStatus { status: "Submitted", filled: 0, remaining: 100 }
   ... [time passes, price moves to limit] ...
2. Execution  { shares: 100, price: 180.00 }
3. OrderStatus { status: "Filled", filled: 100, remaining: 0 }
```

**Stop order:**
```
1. OrderStatus { status: "PreSubmitted", filled: 0, remaining: 100 }
   ... [price hits stop trigger] ...
2. OrderStatus { status: "Submitted", filled: 0, remaining: 100 }
3. Execution  { shares: 100, price: 174.80 }  (slippage on stop)
4. OrderStatus { status: "Filled", filled: 100, remaining: 0 }
```

**Partial fill (large order):**
```
1. OrderStatus { status: "Submitted", filled: 0, remaining: 1000 }
2. Execution  { shares: 400, price: 185.50 }
3. OrderStatus { status: "Submitted", filled: 400, remaining: 600, avg_fill_price: 185.50 }
   ... [tranche delay] ...
4. Execution  { shares: 350, price: 185.52 }
5. OrderStatus { status: "Submitted", filled: 750, remaining: 250, avg_fill_price: 185.51 }
   ... [tranche delay] ...
6. Execution  { shares: 250, price: 185.48 }
7. OrderStatus { status: "Filled", filled: 1000, remaining: 0, avg_fill_price: 185.50 }
```

> **Note**: IB does not change the status string from "Submitted" until the
> order is fully filled. The engine derives PartiallyFilled internally from
> `filled > 0 && remaining > 0`.

### 1.2 Execution ID Generation

Each fill gets a unique execution ID matching IB's format:

```rust
fn next_exec_id(&self) -> String {
    let n = self.exec_counter.fetch_add(1, Ordering::SeqCst);
    format!("TEST.{}.{n}", chrono::Utc::now().format("%Y%m%d"))
}
```

---

## 2. Bracket Lifecycle

### 2.1 Bracket Submission (transmit=false → true)

When the engine calls `place_order` for bracket legs:

```
place_order(parent_id=100, MKT BUY 100 AAPL, transmit=false)
  → Store as Held, no callbacks yet

place_order(tp_id=101, LMT SELL 100 AAPL @ 192, parent=100, transmit=false)
  → Store as Held, link to parent

place_order(sl_id=102, STP SELL 100 AAPL @ 182, parent=100, transmit=true)
  → Store as Held, link to parent
  → transmit=true: ACTIVATE entire bracket
  → Push callbacks for all three legs
```

### 2.2 Bracket Activation Sequence

When the last child (transmit=true) arrives, the test broker:

1. **Activate parent (MKT):**
   ```
   OrderStatus { ib_order_id: 100, status: "Submitted" }
   ```

2. **Activate children as PreSubmitted (waiting for parent fill):**
   ```
   OrderStatus { ib_order_id: 101, status: "PreSubmitted" }
   OrderStatus { ib_order_id: 102, status: "PreSubmitted" }
   ```

3. **Fill parent (market order fills immediately):**
   ```
   Execution { ib_order_id: 100, shares: 100, price: 185.50 }
   OrderStatus { ib_order_id: 100, status: "Filled", filled: 100, remaining: 0 }
   ```

4. **Activate children at exchange (parent filled):**
   ```
   OrderStatus { ib_order_id: 101, status: "Submitted" }  (TP limit now live)
   OrderStatus { ib_order_id: 102, status: "Submitted" }  (SL stop now live — but IB keeps stops as PreSubmitted)
   ```
   
   Note: IB keeps stop orders in PreSubmitted until the stop price is
   triggered. The test broker mirrors this:
   - TP (LMT): → Submitted (working at exchange)
   - SL (STP): stays PreSubmitted (simulated, trigger pending)

### 2.3 TP Fill → SL Auto-Cancel (OCA)

When TP fills:

```
Execution { ib_order_id: 101, shares: 100, price: 192.00 }
OrderStatus { ib_order_id: 101, status: "Filled" }
OrderStatus { ib_order_id: 102, status: "Cancelled" }  (OCA: sibling auto-cancelled)
```

### 2.4 SL Fill → TP Auto-Cancel (OCA)

When SL triggers and fills:

```
OrderStatus { ib_order_id: 102, status: "Submitted" }  (stop triggered → market order)
Execution { ib_order_id: 102, shares: 100, price: 181.80 }  (slippage)
OrderStatus { ib_order_id: 102, status: "Filled" }
OrderStatus { ib_order_id: 101, status: "Cancelled" }  (OCA: sibling auto-cancelled)
```

> **Prerequisite**: `validate_transition()` in `state.rs` must allow
> `PreSubmitted → Cancelled` and `Submitted → Cancelled` for server-side
> OCA cancellations. This has been implemented.

### 2.5 Parent Cancelled Before Fill

```
cancel_order(100)  (cancel parent)
  → OrderStatus { ib_order_id: 100, status: "Cancelled" }
  → OrderStatus { ib_order_id: 101, status: "Cancelled" }  (children auto-cancel)
  → OrderStatus { ib_order_id: 102, status: "Cancelled" }
```

### 2.6 Bracket State Derivation

After each callback, the engine's `check_bracket_status_change()` runs.
The test broker doesn't derive bracket status — the engine does that.
The test broker only simulates individual order status transitions.

---

## 3. Fill Execution

### 3.1 Market Order Fill

```rust
fn fill_market_order(config: &TestBrokerConfig, inner: &mut TestBrokerInner, order: &mut SimulatedOrder) {
    let price = Self::get_or_seed_price_inner(inner, &order.symbol);
    let fill_price = match config.fill_price_model {
        FillPriceModel::Exact => price,
        FillPriceModel::MarketPrice => price,
        FillPriceModel::Slippage { max_bps } => {
            let slip = inner.rng.gen_range(-max_bps..max_bps);
            price * (1.0 + slip / 10_000.0)
        }
    };

    Self::push_execution_inner(inner, order, order.quantity, fill_price);
    order.status = SimOrderStatus::Filled;
    order.filled_qty = order.quantity;
    order.avg_fill_price = fill_price;
}
```

### 3.2 Limit Order Fill Check

Called on each price update (tick or manual):

```rust
fn check_limit_fills(&self, symbol: &str, new_price: f64) {
    let mut inner = self.inner.lock().unwrap();
    for order in inner.orders.values_mut() {
        if order.symbol != symbol || order.status != SimOrderStatus::Working {
            continue;
        }
        if order.order_type != "LMT" {
            continue;
        }

        let should_fill = match order.action.as_str() {
            "BUY" => new_price <= order.limit_price.unwrap(),
            "SELL" => new_price >= order.limit_price.unwrap(),
            _ => false,
        };

        if should_fill {
            Self::push_execution_inner(&mut inner, order, order.quantity - order.filled_qty, order.limit_price.unwrap());
            order.status = SimOrderStatus::Filled;
        }
    }
}
```

### 3.3 Stop Order Trigger Check

```rust
fn check_stop_triggers(&self, symbol: &str, new_price: f64) {
    let mut inner = self.inner.lock().unwrap();
    for order in inner.orders.values_mut() {
        if order.symbol != symbol || order.status != SimOrderStatus::Triggered {
            continue;
        }

        let triggered = match order.action.as_str() {
            "SELL" => new_price <= order.stop_price.unwrap(),  // SL for long
            "BUY" => new_price >= order.stop_price.unwrap(),   // SL for short
            _ => false,
        };

        if triggered {
            // Stop triggered → becomes market order
            inner.callbacks.push_back(BrokerCallback::OrderStatus {
                ib_order_id: order.ib_order_id,
                status: "Submitted".to_string(),
                filled: order.filled_qty,
                remaining: order.quantity - order.filled_qty,
                avg_fill_price: order.avg_fill_price,
            });

            // Fill at market (with slippage for stops)
            let fill_price = new_price; // or apply slippage
            Self::push_execution_inner(&mut inner, order, order.quantity - order.filled_qty, fill_price);
            order.status = SimOrderStatus::Filled;
        }
    }
}
```

### 3.4 Manual Fill Simulation

For tests that need explicit control:

```rust
impl TestBroker {
    /// Manually trigger a fill for a specific order.
    /// Used in integration tests to drive the bracket lifecycle.
    pub fn simulate_fill(&self, ib_order_id: i32, price: f64, quantity: f64) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(order) = inner.orders.get_mut(&ib_order_id) {
            Self::push_execution_inner(&mut inner, order, quantity, price);
            if order.filled_qty >= order.quantity {
                order.status = SimOrderStatus::Filled;
            }
            // Check bracket OCA
            Self::check_bracket_oca_inner(&mut inner, ib_order_id);
        }
    }

    /// Set the market price for a symbol, triggering any pending
    /// limit/stop orders that should fill.
    pub fn set_market_price(&self, symbol: &str, price: f64) {
        let mut inner = self.inner.lock().unwrap();
        inner.market_prices.insert(symbol.to_string(), price);
        Self::check_limit_fills_inner(&mut inner, symbol, price);
        Self::check_stop_triggers_inner(&mut inner, symbol, price);
    }
}
```

---

## 4. Order Modification

### 4.1 Price Modification

IB modifies orders by calling `place_order` with the same `ib_order_id`:

```rust
fn place_order(&self, ib_order_id: i32, ..., limit_price: Option<f64>, ...) {
    let mut inner = self.inner.lock().unwrap();
    if let Some(existing) = inner.orders.get_mut(&ib_order_id) {
        // Modification: update price, keep everything else
        if let Some(new_limit) = limit_price {
            existing.limit_price = Some(new_limit);
        }
        if let Some(new_stop) = stop_price {
            existing.stop_price = Some(new_stop);
        }
        // Don't change status — order stays live
        return Ok(PlaceOrderResult { ib_order_id });
    }
    // New order: store it
    // ...
}
```

### 4.2 Quantity Modification

Same pattern — update quantity, adjust remaining:

```rust
existing.quantity = quantity;
existing.remaining = quantity - existing.filled_qty;
```

---

## 5. Cancellation

### 5.1 Single Order Cancel

```rust
fn cancel_order(&self, ib_order_id: i32) -> Result<CancelOrderResult, String> {
    let mut inner = self.inner.lock().unwrap();
    if let Some(order) = inner.orders.get_mut(&ib_order_id) {
        if order.status.is_terminal() {
            return Err(format!("order {ib_order_id} is already terminal"));
        }
        order.status = SimOrderStatus::Cancelled;
        inner.callbacks.push_back(BrokerCallback::OrderStatus {
            ib_order_id,
            status: "Cancelled".to_string(),
            filled: order.filled_qty,
            remaining: 0.0,
            avg_fill_price: order.avg_fill_price,
        });
        Ok(CancelOrderResult { ib_order_id })
    } else {
        Err(format!("order {ib_order_id} not found"))
    }
}
```

### 5.2 Bracket Cancel (parent cancel → children auto-cancel)

When a parent is cancelled, the test broker auto-cancels all children.
Since we use a single `Mutex<TestBrokerInner>`, the cancel logic acquires the
lock once at the top level to avoid nested locking:

```rust
fn cancel_with_children(&self, ib_order_id: i32) {
    let mut inner = self.inner.lock().unwrap();
    // Cancel the order itself
    Self::cancel_order_inner(&mut inner, ib_order_id);
    // Cancel all children
    if let Some(children) = inner.bracket_links.get(&ib_order_id).cloned() {
        for child_id in children {
            Self::cancel_order_inner(&mut inner, child_id);
        }
    }
}
```

---

## 6. Edge Cases

### 6.1 Race: Fill Arrives After Cancel Request

IB can fill an order between the cancel request and confirmation.
The test broker simulates this with a configurable probability:

```rust
/// Probability that a cancel arrives "too late" and the order fills instead.
pub cancel_race_probability: f64,  // default 0.0
```

### 6.2 Partial Fill Then Cancel

```
Order: BUY 1000 AAPL @ MKT
1. Execution { shares: 400, price: 185.50 }
2. OrderStatus { status: "Submitted", filled: 400, remaining: 600 }  (PartiallyFilled)
3. cancel_order()
4. OrderStatus { status: "Cancelled", filled: 400, remaining: 0 }
   → Position: +400 AAPL @ 185.50
```

> IB reports status "Submitted" even when partially filled. The engine
> derives PartiallyFilled from `filled > 0 && remaining > 0`.

### 6.3 Stop Limit Order

```
place_order(STP LMT, SELL 100 AAPL, stop=182, limit=181.50)

1. OrderStatus { status: "PreSubmitted" }  (waiting for stop trigger)
   ... price drops to 182 ...
2. OrderStatus { status: "Submitted" }  (now a limit order @ 181.50)
   ... price at or above 181.50 ...
3. Execution { shares: 100, price: 181.50 }
4. OrderStatus { status: "Filled" }
```

If price gaps below 181.50 without filling at that price, the limit order
sits unfilled (realistic IB behavior for stop-limits in fast markets).

### 6.4 Market Order Outside RTH

If `outside_rth = false` and the simulated market is closed:
- Order stays in Submitted but doesn't fill until market opens
- Test broker tracks "market hours" (configurable, default: always open)

---

## 7. Error Injection

### 7.1 Configurable Rejection

```rust
/// If > 0.0, random orders are rejected with this probability.
pub rejection_rate: f64,

fn maybe_reject(&self, order: &SimulatedOrder) -> bool {
    if self.config.rejection_rate > 0.0 {
        let mut inner = self.inner.lock().unwrap();
        inner.rng.gen_bool(self.config.rejection_rate)
    } else {
        false
    }
}
```

Rejection reasons cycle through realistic IB messages:
- "Order would trigger IB-level Order Size Limits"
- "No trading permissions for SMART"
- "The contract is not available for short sale"

### 7.2 Connection Loss Simulation

```rust
impl TestBroker {
    /// Simulate a connection loss. Pending orders remain in their current
    /// state. Reconnection must be triggered manually via connect().
    pub fn simulate_disconnect(&self) {
        self.connected.store(false, Ordering::SeqCst);
        let mut inner = self.inner.lock().unwrap();
        inner.callbacks.push_back(BrokerCallback::ConnectionStatus {
            connected: false,
            server_version: None,
        });
    }

    /// Simulate reconnection. Resumes order monitoring.
    pub fn simulate_reconnect(&self) {
        self.connected.store(true, Ordering::SeqCst);
        let mut inner = self.inner.lock().unwrap();
        inner.callbacks.push_back(BrokerCallback::ConnectionStatus {
            connected: true,
            server_version: Some(176),
        });
    }
}
```
