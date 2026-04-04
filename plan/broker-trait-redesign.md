# Broker Trait Redesign

> Widen `BrokerClient` to match the capabilities TestBroker already has,
> wire stubbed engine commands to real handlers, and establish a trait
> surface that a future IB adapter can implement without surprises.
>
> **Scope:** Reorganize existing code only. No new features, no new order
> types, no margin logic. Everything here is code that already exists but
> is stranded behind concrete types instead of the trait.
>
> **Status:** Implemented
> **Date:** 2026-04-04

---

## 1. Problem

`BrokerClient` has 4 required methods (`next_order_id`, `place_order`,
`cancel_order`, `name`) and 4 optional ones (`poll_callbacks`, `connect`,
`disconnect`, `is_connected`). Meanwhile `TestBroker` exposes 6 additional
public methods that the engine can never call because they aren't on the
trait:

| Method on TestBroker | On trait? | Engine needs it? |
|---|---|---|
| `subscribe_market_data(symbol, con_id)` | No | Yes — `SubscribeMarketData` command exists |
| `unsubscribe_market_data(symbol)` | No | Yes — `UnsubscribeMarketData` command exists |
| `positions()` | No | Yes — `RequestPositions` command exists |
| `cash_balance()` | No | Yes — `RequestAccountSummary` command exists |
| `unrealized_pnl()` | No | Yes — used with `RequestAccountSummary` |
| `set_market_price(symbol, price)` | No | No — test helper only |

The engine has 17 `BrokerCommand` variants but only handles 5. The other
12 hit the `_ =>` stub arm and are silently ignored. Six of those have
matching implementations *already written* in TestBroker — they just can't
be reached through the trait.

`MarketDataSource` is a separate trait with one method (`historical_bars`)
injected as a second `Option<Box<dyn MarketDataSource>>`. This split is
intentional (historical data is a one-shot query, not a streaming
subscription) and should be preserved.

---

## 2. Design

### 2.1 Widened BrokerClient trait

Add methods with default no-op implementations so existing
`TestBrokerClient` (the recording stub) keeps compiling unchanged.

```rust
pub trait BrokerClient: Send + Sync {
    // ── Identity ──────────────────────────────────────────────
    fn name(&self) -> &str;

    // ── Connection ────────────────────────────────────────────
    fn connect(&self) -> Result<i32, String> { Ok(0) }
    fn disconnect(&self) {}
    fn is_connected(&self) -> bool { true }

    // ── Order lifecycle ───────────────────────────────────────
    fn next_order_id(&self) -> i32;
    fn place_order(
        &self,
        ib_order_id: i32,
        symbol: &str,
        action: &str,
        order_type: &str,
        quantity: f64,
        limit_price: Option<f64>,
        stop_price: Option<f64>,
        parent_id: Option<i32>,
        transmit: bool,
        tif: &str,
        outside_rth: bool,
    ) -> Result<PlaceOrderResult, String>;
    fn cancel_order(&self, ib_order_id: i32) -> Result<CancelOrderResult, String>;

    // ── Market data subscriptions (NEW) ───────────────────────
    fn subscribe_market_data(&self, _symbol: &str, _con_id: i32) {}
    fn unsubscribe_market_data(&self, _symbol: &str) {}

    // ── Account queries (NEW) ─────────────────────────────────
    fn request_positions(&self) -> Vec<PositionRecord> { Vec::new() }
    fn request_account_summary(&self) -> AccountSummary { AccountSummary::default() }

    // ── Polling ───────────────────────────────────────────────
    fn poll_callbacks(&self) -> Vec<BrokerCallback> { Vec::new() }
}
```

**New supporting types** (in `client.rs`):

```rust
/// A single position as reported by the broker.
#[derive(Debug, Clone)]
pub struct PositionRecord {
    pub symbol: String,
    pub quantity: f64,
    pub avg_cost: f64,
}

/// Snapshot of account values.
#[derive(Debug, Clone, Default)]
pub struct AccountSummary {
    pub cash_balance: f64,
    pub unrealized_pnl: f64,
    pub realized_pnl: f64,
}
```

### 2.2 TestBroker: route existing methods through the trait

`TestBroker` already has `subscribe_market_data`, `unsubscribe_market_data`,
`positions`, `cash_balance`, `unrealized_pnl`. These become the trait impl
body. The old public methods either become the trait methods directly, or
thin wrappers that delegate.

No new logic. Just moving method signatures from `impl TestBroker` to
`impl BrokerClient for TestBroker`.

### 2.3 TestBrokerClient: unchanged

`TestBrokerClient` is the minimal recording stub. All new trait methods
have defaults, so it compiles without changes.

### 2.4 Engine: wire stubbed commands

Add handlers for commands that already exist in the enum but hit the
`_ =>` stub. Each handler delegates to the corresponding trait method.

| Command | Handler logic |
|---|---|
| `Connect` | Call `client.connect()`, emit `Connected` or `Error` event |
| `Disconnect` | Call `client.disconnect()`, emit `Disconnected` event |
| `SubscribeMarketData` | Call `client.subscribe_market_data(symbol, con_id)` |
| `UnsubscribeMarketData` | Call `client.unsubscribe_market_data(symbol)` |
| `RequestPositions` | Call `client.request_positions()`, emit `PositionUpdate` per position |
| `RequestAccountSummary` | Call `client.request_account_summary()`, emit `AccountValueUpdate` + `PnlUpdate` |

These are all 2–10 line handlers. No new business logic.

### 2.5 Prune dead command variants

These `BrokerCommand` variants have no implementation path and overlap
with bracket commands that supersede them. Remove them:

| Variant | Reason to remove |
|---|---|
| `CreateOrder` | Superseded by `CreateMarketBracket` (all orders are brackets) |
| `ActivateOrder` | No activate/deactivate model exists in the implementation |
| `DeactivateOrder` | Same — design doc mentions it but it was never built |
| `CreateBracketOrder` | Old API, replaced by `CreateMarketBracket` |
| `Reconnect` | Should be internal engine behavior, not a user command |

Keep `CancelOrder` and `ModifyOrder` as stubs with a `tracing::debug!`
— these are legitimate future needs for single-leg operations.

### 2.6 MarketDataSource: keep separate

`MarketDataSource` stays as a separate trait. It serves a different
purpose (one-shot historical queries vs. streaming subscriptions) and
has a different mutability model (`&mut self` vs `&self`). The engine
already injects them separately and this is correct.

### 2.7 BrokerCallback: add PositionUpdate and AccountSummary variants

TestBroker already tracks positions and cash internally. To emit them
through `poll_callbacks()`, add two callback variants:

```rust
pub enum BrokerCallback {
    // ... existing variants ...

    /// Position snapshot (one per symbol).
    Position {
        symbol: String,
        quantity: f64,
        avg_cost: f64,
    },

    /// Account summary snapshot.
    Account {
        cash_balance: f64,
        unrealized_pnl: f64,
        realized_pnl: f64,
    },
}
```

The engine's `handle_broker_callback` match already handles unknown
variants via the exhaustive match — adding these requires new match arms
that emit the corresponding `BrokerEvent::PositionUpdate` and
`BrokerEvent::PnlUpdate`.

---

## 3. Implementation order

Each step is independently compilable and testable. No step breaks
existing tests.

### Step 1 — Add types and widen trait (client.rs)

1. Add `PositionRecord` and `AccountSummary` structs
2. Add `Position` and `Account` variants to `BrokerCallback`
3. Add 4 new methods to `BrokerClient` trait with default impls
4. **Tests pass unchanged** — all new methods have defaults

### Step 2 — Route TestBroker methods through trait (test_broker.rs)

1. Move `subscribe_market_data` and `unsubscribe_market_data` from
   `impl TestBroker` to `impl BrokerClient for TestBroker`
2. Implement `request_positions` by wrapping existing `positions()` logic
3. Implement `request_account_summary` by wrapping existing `cash_balance()`
   and `unrealized_pnl()` logic
4. Keep the old public methods as convenience wrappers that call the
   trait methods (avoids breaking test code that calls them directly)
5. **All 266 broker tests pass unchanged**

### Step 3 — Wire engine command handlers (engine.rs)

1. Add `Connect` handler: call `client.connect()`, update connection
   state, emit event
2. Add `Disconnect` handler: call `client.disconnect()`, emit event
3. Add `SubscribeMarketData` handler: call `client.subscribe_market_data()`
4. Add `UnsubscribeMarketData` handler: call `client.unsubscribe_market_data()`
5. Add `RequestPositions` handler: call `client.request_positions()`,
   emit `PositionUpdate` per entry
6. Add `RequestAccountSummary` handler: call `client.request_account_summary()`,
   emit `AccountValueUpdate` + `PnlUpdate`
7. Add `Position` and `Account` match arms in `handle_broker_callback`
8. **Write tests for each new handler** (same pattern as existing bracket tests)

### Step 4 — Prune dead command variants (commands.rs, engine.rs)

1. Remove `CreateOrder`, `ActivateOrder`, `DeactivateOrder`,
   `CreateBracketOrder`, `Reconnect` from `BrokerCommand` enum
2. Keep `CancelOrder` and `ModifyOrder` as stubs
3. Update `handle_command` match — remove dead arms
4. Update `lib.rs` exports if any were re-exported
5. `cargo test --workspace` to confirm nothing depended on them

### Step 5 — Update desktop bridge types (desktop/win/crates/midas-core/src/broker.rs)

1. Add `PositionRecord` and `AccountSummary` mirror types
2. These are manually synced types (desktop can't depend on midas-broker)
3. No functional change — just keeps the type bridge in sync

---

## 4. What this does NOT do

- No new order types (trailing stops, algos)
- No TIF enforcement
- No margin/buying power checks
- No bar streaming
- No real IB adapter
- No changes to the desktop app's UI or message handling
- No changes to MarketDataSource trait

---

## 5. Verification

After all steps:

```
cargo test -p midas-broker           # 266+ tests pass
cd desktop/win && cargo test -p midas-app  # 97+ tests pass
cd desktop/win && cargo build        # desktop builds
```

The trait surface should now match what TestBroker can do. A future IB
adapter implementing `BrokerClient` would have clear method contracts for
every operation the engine can request.
