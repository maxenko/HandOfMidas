# Broker Bridge Implementation Plan

Wire the `midas-broker` engine into the Hand of Midas desktop UI so that orders placed in the order panel flow through `BrokerEngine` and results (fills, status changes) update chart annotations in real time.

## Dependency Graph

```
Slice 0: BracketCreated prices + re-exports (root workspace)
   |
Slice 1: Cargo.toml dependency (no feature gate)
   |
Slice 2: BrokerBridge adapter
   |
Slice 3: Start engine on app startup
   |
Slice 4: Wire OrderPanelConfirmYes (local annotation + engine command)
   |
Slice 5: Broker event subscription (correct iced 0.14 pattern)
   |
Slice 6: Reconcile engine UUIDs (update OrderAnnotationLink)
   |
   +-----+-----+-----+
   |     |     |     |
   v     v     v     v
  S7    S8    S9   S10
```

Slices 0-6 are sequential. Slices 7-10 all depend on Slice 6 but are independent of each other.

---

## Slice 0: Enhance BracketCreated with prices + re-exports

**Goal:** Add `tp_price`, `sl_price`, and `reference_price` fields to `BrokerEvent::BracketCreated` in the root workspace so the engine event carries full data for annotation positioning. Also add missing re-exports to `midas-broker/src/lib.rs`.

### Files to modify

| File | Change |
|------|--------|
| `crates/midas-broker/src/events.rs` | Add price fields to `BracketCreated` variant |
| `crates/midas-broker/src/engine.rs` | Populate new fields where `BracketCreated` is emitted |
| `crates/midas-broker/src/lib.rs` | Add `pub use engine::start_broker_engine;` and `pub use midas_core::SecurityType;` |

### Code changes

**`crates/midas-broker/src/events.rs`** -- replace the `BracketCreated` variant (lines 72-79):

```rust
/// A market bracket was created and submitted.
BracketCreated {
    parent_id: Uuid,
    take_profit_id: Option<Uuid>,
    stop_loss_id: Option<Uuid>,
    symbol: String,
    action: OrderAction,
    quantity: f64,
    /// Take profit limit price (if TP leg exists).
    tp_price: Option<f64>,
    /// Stop loss trigger price (if SL leg exists).
    sl_price: Option<f64>,
    /// Last traded price at submission time (from MarketBracketParams).
    reference_price: Option<f64>,
},
```

**`crates/midas-broker/src/engine.rs`** -- wherever `BracketCreated` is constructed, add the three new fields. The engine has access to the `MarketBracketParams` at creation time:

```rust
BrokerEvent::BracketCreated {
    parent_id,
    take_profit_id,
    stop_loss_id,
    symbol: params.symbol.clone(),
    action: params.action,
    quantity: params.quantity,
    tp_price: params.take_profit.as_ref().map(|tp| tp.price),
    sl_price: params.stop_loss.as_ref().map(|sl| sl.stop_price),
    reference_price: params.reference_price,
}
```

**`crates/midas-broker/src/lib.rs`** -- add after the existing re-exports (line 38):

```rust
pub use engine::start_broker_engine;
pub use midas_core::SecurityType;
pub use orders::types::{OrderAction, TimeInForce};
```

### What to test

```bash
cargo test --workspace
```

All existing tests pass. Any test constructing `BracketCreated` must be updated to include the three new fields.

### Done criteria

`BracketCreated` carries price data. `midas_broker::start_broker_engine`, `midas_broker::SecurityType`, `midas_broker::OrderAction`, and `midas_broker::TimeInForce` are accessible as top-level re-exports.

---

## Slice 1: Add midas-broker dependency to desktop workspace

**Goal:** Make `midas-broker` available to `midas-app` via a path dependency. No feature gating -- broker is core functionality.

### Files to modify

| File | Change |
|------|--------|
| `desktop/win/Cargo.toml` | Add `midas-broker` to `[workspace.dependencies]` |
| `desktop/win/crates/midas-app/Cargo.toml` | Add `midas-broker` to `[dependencies]` |

### Code changes

**`desktop/win/Cargo.toml`** -- add to `[workspace.dependencies]`:

```toml
# Broker engine (root workspace crate, path dependency)
midas-broker = { path = "../../crates/midas-broker" }
```

**`desktop/win/crates/midas-app/Cargo.toml`** -- add to `[dependencies]`:

```toml
midas-broker = { workspace = true }
async-trait = { workspace = true }
```

No `#[cfg(feature = "broker")]` anywhere. Broker is always compiled.

### What to test

```bash
cd desktop/win && cargo check -p midas-app
```

Verifies Cargo resolves the cross-workspace path dependency. The `midas-broker` crate depends on `ibapi`, `rusqlite` (bundled), and `rand`; all pulled transitively.

### Done criteria

`cargo check` succeeds with zero errors. No code changes -- only `Cargo.toml` files touched.

### Risks

**`midas-core` diamond dependency.** Both `midas-broker` and `desktop/win/crates/midas-core` depend on the root `midas-core`. Cargo treats them as separate crates. Types like `SecurityType` exist in both and are not interchangeable at the type level. Slice 2's translation functions handle this. With the new `pub use midas_core::SecurityType` in `midas-broker/src/lib.rs`, the bridge can use `midas_broker::SecurityType` (which is the root `midas-core::SecurityType`).

---

## Slice 2: BrokerBridge adapter

**Goal:** Create `broker_bridge.rs` in `midas-app` that wraps `BrokerHandle`, provides type translation between the two workspaces' mirror types, and includes a `shutdown()` method.

### Files to create/modify

| File | Change |
|------|--------|
| `desktop/win/crates/midas-app/src/broker_bridge.rs` | NEW |
| `desktop/win/crates/midas-app/src/main.rs` | Add `mod broker_bridge;` |

### Code changes

**`desktop/win/crates/midas-app/src/main.rs`** -- add module declaration after `mod annotation_persistence;`:

```rust
mod broker_bridge;
```

**`desktop/win/crates/midas-app/src/broker_bridge.rs`** -- full file:

```rust
//! Adapter between `midas-broker::BrokerHandle` and the desktop workspace.
//!
//! `BrokerBridge` wraps the broker engine's channel handles and translates
//! between the desktop mirror types (`midas_core::broker::*`) and the broker
//! engine types (`midas_broker::*`).

use async_trait::async_trait;
use tokio::sync::{broadcast, mpsc, watch};

use midas_broker::{BrokerCommand, BrokerEvent, BrokerHandle};

/// Desktop-side wrapper around the broker engine's channel handles.
///
/// All methods are non-blocking: they send commands over an mpsc channel
/// and return immediately. Results arrive as `BrokerEvent`s on the
/// broadcast channels.
pub struct BrokerBridge {
    /// Command sender to the broker engine.
    commands: mpsc::Sender<BrokerCommand>,
    /// Order event broadcast sender (kept alive to subscribe new receivers).
    order_events: broadcast::Sender<BrokerEvent>,
    /// Connection state watcher.
    connection_state: watch::Receiver<midas_broker::ConnectionState>,
    /// Display name for the UI.
    name: String,
}

impl BrokerBridge {
    /// Create a bridge from a `BrokerHandle` returned by `start_broker_engine`.
    pub fn new(handle: BrokerHandle, name: impl Into<String>) -> Self {
        Self {
            commands: handle.commands,
            order_events: handle.order_events,
            connection_state: handle.connection_state,
            name: name.into(),
        }
    }

    /// Subscribe to order events. Each call returns a new receiver that
    /// independently tracks its read position in the broadcast buffer.
    pub fn subscribe_order_events(&self) -> broadcast::Receiver<BrokerEvent> {
        self.order_events.subscribe()
    }

    /// Get the order event broadcast sender for subscription closures.
    ///
    /// The caller subscribes inside the async closure to avoid lifetime
    /// issues with receiver creation timing.
    pub fn order_event_sender(&self) -> broadcast::Sender<BrokerEvent> {
        self.order_events.clone()
    }

    /// Get a clone of the connection state watcher.
    pub fn watch_connection_state(&self) -> watch::Receiver<midas_broker::ConnectionState> {
        self.connection_state.clone()
    }

    /// Send a command to the broker engine (non-blocking).
    ///
    /// Returns `Err` only if the engine task has been dropped (shutdown).
    pub fn send_command(&self, cmd: BrokerCommand) -> Result<(), String> {
        self.commands.try_send(cmd).map_err(|e| format!("broker command send failed: {e}"))
    }

    /// Send `BrokerCommand::Connect` to initiate the connection.
    pub fn connect(&self) -> Result<(), String> {
        self.send_command(BrokerCommand::Connect)
    }

    /// Send `BrokerCommand::CreateMarketBracket` with translated params.
    pub fn create_market_bracket(
        &self,
        params: midas_core::broker::MarketBracketParams,
    ) -> Result<(), String> {
        let broker_params = translate_bracket_params(params);
        self.send_command(BrokerCommand::CreateMarketBracket(broker_params))
    }

    /// Send `BrokerCommand::CancelBracket`.
    pub fn cancel_bracket(&self, parent_id: uuid::Uuid) -> Result<(), String> {
        self.send_command(BrokerCommand::CancelBracket { parent_id })
    }

    /// Send `BrokerCommand::ModifyBracketLeg`.
    pub fn modify_bracket_leg(
        &self,
        order_id: uuid::Uuid,
        new_price: f64,
    ) -> Result<(), String> {
        self.send_command(BrokerCommand::ModifyBracketLeg { order_id, new_price })
    }

    /// Whether the broker engine reports a connected state.
    pub fn is_engine_connected(&self) -> bool {
        self.connection_state.borrow().is_connected()
    }

    /// Gracefully shut down the broker engine.
    pub fn shutdown(&self) -> Result<(), String> {
        self.send_command(BrokerCommand::Shutdown)
    }
}

// -- OrderBroker trait implementation (provider) ----------------------------

#[async_trait]
impl midas_core::provider::OrderBroker for BrokerBridge {
    fn name(&self) -> &str {
        &self.name
    }

    fn is_connected(&self) -> bool {
        self.is_engine_connected()
    }

    fn connection_state(&self) -> midas_core::provider::ConnectionState {
        translate_connection_state(&*self.connection_state.borrow())
    }
}

// -- Type translation functions --------------------------------------------

/// Translate desktop `MarketBracketParams` to broker engine
/// `midas_broker::MarketBracketParams`.
///
/// The two structs are structurally identical but live in different crates
/// with different `SecurityType`, `OrderAction`, and `TimeInForce` enums.
fn translate_bracket_params(
    p: midas_core::broker::MarketBracketParams,
) -> midas_broker::MarketBracketParams {
    midas_broker::MarketBracketParams {
        symbol: p.symbol,
        con_id: p.con_id,
        sec_type: translate_security_type(p.sec_type),
        exchange: p.exchange,
        currency: p.currency,
        action: translate_order_action(p.action),
        quantity: p.quantity,
        outside_rth: p.outside_rth,
        take_profit: p.take_profit.map(|tp| {
            midas_broker::TakeProfitParams {
                price: tp.price,
                tif: tp.tif.map(translate_tif),
            }
        }),
        stop_loss: p.stop_loss.map(|sl| {
            midas_broker::StopLossParams {
                stop_price: sl.stop_price,
                limit_price: sl.limit_price,
                tif: sl.tif.map(translate_tif),
            }
        }),
        reference_price: p.reference_price,
        strategy: p.strategy,
        tags: p.tags,
    }
}

/// Desktop `SecurityType` -> broker `SecurityType`.
///
/// Uses `midas_broker::SecurityType` which is re-exported from the root
/// `midas-core`. The desktop `midas_core::SecurityType` is a structurally
/// identical but distinct type.
fn translate_security_type(
    st: midas_core::SecurityType,
) -> midas_broker::SecurityType {
    match st {
        midas_core::SecurityType::Stock => midas_broker::SecurityType::Stock,
        midas_core::SecurityType::Option => midas_broker::SecurityType::Option,
        midas_core::SecurityType::Future => midas_broker::SecurityType::Future,
        midas_core::SecurityType::Forex => midas_broker::SecurityType::Forex,
    }
}

/// Desktop `OrderAction` -> broker `OrderAction`.
fn translate_order_action(
    a: midas_core::broker::OrderAction,
) -> midas_broker::OrderAction {
    match a {
        midas_core::broker::OrderAction::Buy => midas_broker::OrderAction::Buy,
        midas_core::broker::OrderAction::Sell => midas_broker::OrderAction::Sell,
    }
}

/// Desktop `TimeInForce` -> broker `TimeInForce`.
fn translate_tif(
    tif: midas_core::broker::TimeInForce,
) -> midas_broker::TimeInForce {
    match tif {
        midas_core::broker::TimeInForce::Day => midas_broker::TimeInForce::Day,
        midas_core::broker::TimeInForce::Gtc => midas_broker::TimeInForce::Gtc,
        midas_core::broker::TimeInForce::Ioc => midas_broker::TimeInForce::Ioc,
        midas_core::broker::TimeInForce::Gtd => midas_broker::TimeInForce::Gtd,
        midas_core::broker::TimeInForce::Opg => midas_broker::TimeInForce::Opg,
    }
}

/// Broker engine `ConnectionState` -> desktop `ConnectionState`.
fn translate_connection_state(
    cs: &midas_broker::ConnectionState,
) -> midas_core::provider::ConnectionState {
    match cs {
        midas_broker::ConnectionState::Disconnected => {
            midas_core::provider::ConnectionState::Disconnected
        }
        midas_broker::ConnectionState::Connecting => {
            midas_core::provider::ConnectionState::Connecting
        }
        midas_broker::ConnectionState::Connected { server_version } => {
            midas_core::provider::ConnectionState::Connected {
                server_version: *server_version,
            }
        }
        midas_broker::ConnectionState::Ready => {
            midas_core::provider::ConnectionState::Ready
        }
        midas_broker::ConnectionState::Reconnecting { attempt } => {
            midas_core::provider::ConnectionState::Reconnecting { attempt: *attempt }
        }
    }
}

/// Translate a broker `BracketLifecycleStatus` to the desktop mirror type.
pub fn translate_lifecycle_status(
    status: &midas_broker::BracketLifecycleStatus,
) -> midas_core::broker::BracketLifecycleStatus {
    match status {
        midas_broker::BracketLifecycleStatus::Submitted => {
            midas_core::broker::BracketLifecycleStatus::Submitted
        }
        midas_broker::BracketLifecycleStatus::EntryFilled => {
            midas_core::broker::BracketLifecycleStatus::EntryFilled
        }
        midas_broker::BracketLifecycleStatus::TakeProfitHit => {
            midas_core::broker::BracketLifecycleStatus::TakeProfitHit
        }
        midas_broker::BracketLifecycleStatus::StopLossHit => {
            midas_core::broker::BracketLifecycleStatus::StopLossHit
        }
        midas_broker::BracketLifecycleStatus::Cancelled => {
            midas_core::broker::BracketLifecycleStatus::Cancelled
        }
        midas_broker::BracketLifecycleStatus::Rejected => {
            midas_core::broker::BracketLifecycleStatus::Rejected
        }
        midas_broker::BracketLifecycleStatus::Error => {
            midas_core::broker::BracketLifecycleStatus::Error
        }
        midas_broker::BracketLifecycleStatus::Closed => {
            midas_core::broker::BracketLifecycleStatus::Closed
        }
    }
}

/// Translate a broker `OrderAction` to a chart `BracketSide`.
pub fn translate_action_to_side(
    action: &midas_broker::OrderAction,
) -> midas_chart::widget::order_bracket::BracketSide {
    match action {
        midas_broker::OrderAction::Buy => {
            midas_chart::widget::order_bracket::BracketSide::Long
        }
        midas_broker::OrderAction::Sell => {
            midas_chart::widget::order_bracket::BracketSide::Short
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translate_order_action_roundtrip() {
        let buy = translate_order_action(midas_core::broker::OrderAction::Buy);
        assert_eq!(buy, midas_broker::OrderAction::Buy);
        let sell = translate_order_action(midas_core::broker::OrderAction::Sell);
        assert_eq!(sell, midas_broker::OrderAction::Sell);
    }

    #[test]
    fn translate_lifecycle_roundtrip() {
        use midas_broker::BracketLifecycleStatus as Broker;
        use midas_core::broker::BracketLifecycleStatus as Desktop;

        assert_eq!(translate_lifecycle_status(&Broker::Submitted), Desktop::Submitted);
        assert_eq!(translate_lifecycle_status(&Broker::EntryFilled), Desktop::EntryFilled);
        assert_eq!(translate_lifecycle_status(&Broker::TakeProfitHit), Desktop::TakeProfitHit);
        assert_eq!(translate_lifecycle_status(&Broker::StopLossHit), Desktop::StopLossHit);
        assert_eq!(translate_lifecycle_status(&Broker::Cancelled), Desktop::Cancelled);
    }

    #[test]
    fn translate_action_to_side_mapping() {
        use midas_broker::OrderAction;
        use midas_chart::widget::order_bracket::BracketSide;

        assert_eq!(translate_action_to_side(&OrderAction::Buy), BracketSide::Long);
        assert_eq!(translate_action_to_side(&OrderAction::Sell), BracketSide::Short);
    }
}
```

### What to test

```bash
cd desktop/win && cargo test -p midas-app -- broker_bridge
```

All three unit tests pass.

### Done criteria

`broker_bridge.rs` compiles. Type translation functions cover all enum variants. The `shutdown()` method exists. All translation goes through `midas_broker::` top-level re-exports (not deep module paths).

---

## Slice 3: Start engine on app startup

**Goal:** Call `midas_broker::start_broker_engine()` in `MidasApp::new()`, wrap the handle in `BrokerBridge`, store it on `MidasApp`, register it in `ProviderRegistry`, and send `BrokerCommand::Connect`.

### Files to modify

| File | Change |
|------|--------|
| `desktop/win/crates/midas-app/src/app.rs` | Add `broker_bridge` field to `MidasApp`, create in `new()`, register in `ProviderRegistry` |

### Code changes

**`MidasApp` struct** -- add field after `market_cache`:

```rust
/// Bridge to the midas-broker engine. None if engine failed to start.
pub broker_bridge: Option<Arc<crate::broker_bridge::BrokerBridge>>,
```

Add `use std::sync::Arc;` if not already imported (it is -- line 13).

**`MidasApp::new()`** -- after `let store = ...` block (around line 687), before building `let mut app = Self { ... }`:

```rust
// Start the broker engine with TestBroker defaults.
let broker_bridge = {
    use midas_broker::start_broker_engine;
    use crate::broker_bridge::BrokerBridge;

    let broker_config = midas_broker::BrokerConfig::default(); // TestBroker, paper port
    let handle = start_broker_engine(broker_config);
    let bridge = Arc::new(BrokerBridge::new(handle, "Test Broker"));
    tracing::info!("Broker engine started (Test Broker, data_source=test)");
    Some(bridge)
};
```

**`MidasApp::new()`** -- in the `Self { ... }` struct literal, add:

```rust
broker_bridge: broker_bridge.clone(),
```

**After `let mut app = Self { ... };`** -- register the broker and send Connect:

```rust
// Register broker bridge in provider registry.
if let Some(ref bridge) = app.broker_bridge {
    app.providers.register_order_broker(bridge.clone());
    app.providers.set_active_broker(Some(0));
}

// Connect to broker (TestBroker auto-connects, but the command
// ensures the engine state machine transitions properly).
if let Some(ref bridge) = app.broker_bridge {
    if let Err(e) = bridge.connect() {
        tracing::warn!("Failed to send initial Connect: {e}");
    }
}
```

### What to test

```bash
cd desktop/win && cargo run -p midas-app
```

1. Application starts without error.
2. Log output shows: `Broker engine started (Test Broker, data_source=test)`
3. Status bar toolbar broker picker shows "Test Broker".
4. No panics or runtime errors in the console.

### Done criteria

The broker engine is running as a background tokio task. The bridge is stored on `MidasApp` and registered in `ProviderRegistry`.

---

## Slice 4: Wire OrderPanelConfirmYes to broker

**Goal:** When the user confirms an order, create the chart annotation locally (instant UX) AND send the command to the engine. The local annotation uses locally-generated UUIDs. When the engine's `BracketCreated` event arrives later (Slice 6), we reconcile rather than create a second annotation.

### Files to modify

| File | Change |
|------|--------|
| `desktop/win/crates/midas-app/src/app.rs` | Modify `OrderPanelConfirmYes` match arm (lines 2828-2897) |

### Code changes

Replace the block at lines 2870-2880 (the "Broker bridge not connected" warning). The new code sends to the engine AND keeps the existing local `BrokerBracketCreated` self-message for instant annotation:

```rust
// Send to broker engine.
if let Some(ref bridge) = self.broker_bridge {
    let broker_params = midas_core::broker::MarketBracketParams {
        symbol: symbol.clone(),
        con_id: None,
        sec_type: midas_core::SecurityType::Stock,
        exchange: "SMART".to_string(),
        currency: "USD".to_string(),
        action: match panel.side {
            OrderSide::Buy => midas_core::broker::OrderAction::Buy,
            OrderSide::Sell => midas_core::broker::OrderAction::Sell,
        },
        quantity,
        outside_rth: false,
        take_profit: tp_price.map(|p| midas_core::broker::TakeProfitParams {
            price: p,
            tif: None,
        }),
        stop_loss: sl_price.map(|p| midas_core::broker::StopLossParams {
            stop_price: p,
            limit_price: None,
            tif: None,
        }),
        reference_price: Some(last_price),
        strategy: None,
        tags: Vec::new(),
    };
    match bridge.create_market_bracket(broker_params) {
        Ok(()) => {
            tracing::info!(
                "CreateMarketBracket sent to broker engine for {}",
                symbol
            );
        }
        Err(e) => {
            tracing::error!("Failed to send bracket to broker: {e}");
            self.toast_message = Some(format!("Broker error: {e}"));
            self.toast_created_at = Some(Instant::now());
        }
    }
} else {
    tracing::warn!(
        "No broker bridge: CreateMarketBracket for {} not sent",
        symbol,
    );
}
self.status_message = format!(
    "Order submitted: {} {} {}",
    match panel.side {
        OrderSide::Buy => "BUY",
        OrderSide::Sell => "SELL",
    },
    panel.quantity,
    panel.symbol,
);
```

The existing `BrokerBracketCreated` self-message at line 2886 is kept unchanged. It creates the chart annotation with locally-generated UUIDs (`Uuid::now_v7()`). The annotation appears immediately. The `order_panel.visible = false` line is also kept.

### Key design choice

The local annotation uses provisional UUIDs. When the engine's `BracketCreated` event arrives (Slice 6), we do NOT create a second annotation. Instead we UPDATE the existing `OrderAnnotationLink` with the engine's real order UUIDs. This gives instant UX while ensuring modify/cancel commands use the real engine UUIDs.

### Extend OrderAnnotationLink

Add three new fields to `OrderAnnotationLink` so the reconciliation algorithm (Slice 6) can match without reading back from the annotation store:

```rust
pub struct OrderAnnotationLink {
    // ... existing fields ...

    /// Side of the bracket (Long/Short), cached at creation time.
    pub side: midas_chart::widget::order_bracket::BracketSide,
    /// Quantity submitted, cached at creation time.
    pub quantity: f64,
    /// When this link was created, for FIFO ordering during reconciliation.
    pub created_at: std::time::Instant,
}
```

When creating the link in the `BrokerBracketCreated` handler, populate these fields:

```rust
let link = OrderAnnotationLink {
    // ... existing fields ...
    side: match panel.side {
        OrderSide::Buy => midas_chart::widget::order_bracket::BracketSide::Long,
        OrderSide::Sell => midas_chart::widget::order_bracket::BracketSide::Short,
    },
    quantity,
    created_at: std::time::Instant::now(),
};
```

### What to test

1. Open order panel, enter a valid bracket, confirm.
2. Log shows `CreateMarketBracket sent to broker engine for AAPL`.
3. Chart annotation still appears immediately (local self-message).
4. TestBroker processes the bracket -- verify with `RUST_LOG=midas_broker=debug`.

### Done criteria

Confirmation sends the command to the broker engine AND creates the local annotation. Both paths execute.

---

## Slice 5: Broker event subscription (correct iced 0.14 pattern)

**Goal:** Add an iced subscription that drains `BrokerEvent`s from the engine's `order_events` broadcast channel and maps them to `Message::BrokerEventReceived(Box<BrokerEvent>)`.

### iced 0.14 subscription API

The correct iced 0.14 pattern uses:

- `iced::stream::channel(size, async |sender| { ... })` which returns `impl Stream<Item = T>`
- `Subscription::run_with(data, fn_ptr)` where `data: D` must implement `Hash + 'static`

`broadcast::Sender<BrokerEvent>` does NOT implement `Hash`, so we need a newtype wrapper with a manual `Hash` implementation.

### Files to modify

| File | Change |
|------|--------|
| `desktop/win/crates/midas-app/src/broker_bridge.rs` | Add `BrokerEventSource` newtype |
| `desktop/win/crates/midas-app/src/main.rs` | Add broker event subscription using `Subscription::run_with` |
| `desktop/win/crates/midas-app/src/app.rs` | Add `BrokerEventReceived(Box<midas_broker::BrokerEvent>)` message variant |

### Code changes

**New message variant** in `Message` enum (after `BrokerBracketStatusChanged`):

```rust
/// Raw broker event received from the subscription channel.
/// Boxed to keep Message size small (BrokerEvent is large).
BrokerEventReceived(Box<midas_broker::BrokerEvent>),
```

**`broker_bridge.rs`** -- add the hashable wrapper:

```rust
use std::hash::{Hash, Hasher};

/// Wrapper around `broadcast::Sender<BrokerEvent>` that implements `Hash`
/// so it can be used with `Subscription::run_with`.
///
/// The hash is a constant (we only ever have one broker subscription),
/// which tells iced to keep a single instance alive.
#[derive(Clone)]
pub struct BrokerEventSource {
    pub sender: broadcast::Sender<BrokerEvent>,
}

impl Hash for BrokerEventSource {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Constant hash -- one subscription instance.
        "broker-event-source".hash(state);
    }
}
```

**`broker_bridge.rs`** -- add a method to `BrokerBridge`:

```rust
/// Create a `BrokerEventSource` for use with `Subscription::run_with`.
pub fn event_source(&self) -> BrokerEventSource {
    BrokerEventSource {
        sender: self.order_events.clone(),
    }
}
```

**`broker_bridge.rs`** -- add the stream builder function (module-level):

```rust
use crate::app::Message;

/// Build a stream of broker events for iced subscription.
///
/// This is a `fn` pointer (not a closure) as required by `Subscription::run_with`.
pub fn broker_event_stream(
    source: &BrokerEventSource,
) -> impl iced::advanced::graphics::futures::Stream<Item = Message> {
    let sender = source.sender.clone();
    iced::stream::channel(256, async move |mut output| {
        let mut rx = sender.subscribe();
        loop {
            match rx.recv().await {
                Ok(event) => {
                    let _ = output
                        .send(Message::BrokerEventReceived(Box::new(event)))
                        .await;
                }
                Err(tokio::sync::broadcast::RecvError::Lagged(n)) => {
                    tracing::warn!("Broker event subscription lagged by {n} events");
                }
                Err(tokio::sync::broadcast::RecvError::Closed) => {
                    tracing::info!("Broker event channel closed, subscription ending");
                    std::future::pending::<()>().await;
                    break;
                }
            }
        }
    })
}
```

Note: The `iced::stream::channel` closure takes `async |mut output: mpsc::Sender<T>| { ... }` and returns `impl Stream<Item = T>`. Rust 1.91.1 supports `AsyncFnOnce`, so `async move |mut output| { ... }` compiles. The receiver is created *inside* the closure from the cloned sender, avoiding lifetime issues.

**Required import for `.send().await`:** The `output.send(...)` call requires `SinkExt` in scope:
```rust
use iced::futures::SinkExt;
```
Add this to the imports in `broker_bridge.rs`. Without it, the `futures::channel::mpsc::Sender` does not expose `.send()` as an async method.

If the `Stream` trait import is not directly available, use:
```rust
use iced::futures::Stream;
```
as iced 0.14 re-exports `futures` types.

**`main.rs`** -- in the `subscription()` function, add before `Subscription::batch(subs)`:

```rust
// Broker order event subscription.
if let Some(ref bridge) = state.broker_bridge {
    let source = bridge.event_source();
    let broker_sub = Subscription::run_with(
        source,
        crate::broker_bridge::broker_event_stream,
    );
    subs.push(broker_sub);
}
```

### Subscription lifetime

`Subscription::run_with(data, fn_ptr)` calls the function pointer once per unique `data` hash. Since `BrokerEventSource` always hashes to the same value, iced keeps a single subscription alive for the app's lifetime. The `broadcast::Receiver` is created inside the async closure from the cloned `Sender`, so there is no re-subscription issue when `subscription()` is called each frame.

### What to test

1. Place an order via the order panel.
2. TestBroker fills it (instant fill by default).
3. Log shows events received via the subscription.
4. No duplicate subscriptions spawned (check logs for single "Broker event channel closed" on shutdown).

### Done criteria

Broker events flow from the engine through the subscription into the iced update loop.

---

## Slice 6: Reconcile engine UUIDs

**Goal:** When the engine's `BracketCreated` event arrives, do NOT create a second annotation. Instead, UPDATE the existing `OrderAnnotationLink` with the engine's real order UUIDs so that subsequent modify/cancel commands use the correct IDs.

### Files to modify

| File | Change |
|------|--------|
| `desktop/win/crates/midas-app/src/app.rs` | Add `BrokerEventReceived` handler with UUID reconciliation logic |

### Code changes

**`update()` handler for `BrokerEventReceived`** in `app.rs`:

```rust
Message::BrokerEventReceived(boxed_event) => {
    use midas_broker::BrokerEvent;

    match *boxed_event {
        BrokerEvent::BracketCreated {
            parent_id,
            take_profit_id,
            stop_loss_id,
            symbol,
            action,
            quantity,
            tp_price,
            sl_price,
            reference_price,
        } => {
            // Reconcile: find the existing annotation created locally
            // in OrderPanelConfirmYes by matching symbol + side + quantity
            // using the cached fields on OrderAnnotationLink (no annotation
            // store lookup needed).
            //
            // The local link uses provisional UUIDs. Replace them with
            // the engine's real UUIDs.
            let side = crate::broker_bridge::translate_action_to_side(&action);

            // Collect matching links and sort by created_at for FIFO ordering.
            let mut candidates: Vec<_> = self
                .order_annotation_links
                .iter()
                .filter(|(_, link)| {
                    link.symbol == symbol
                        && link.side == side
                        && (link.quantity - quantity).abs() < 0.01
                })
                .collect();
            candidates.sort_by_key(|(_, link)| link.created_at);

            // Take the oldest matching link (FIFO).
            let matching_key = candidates.first().map(|(key, _)| **key);

            if let Some(old_key) = matching_key {
                // Remove the old provisional link and re-insert with engine UUIDs.
                if let Some(mut link) = self.order_annotation_links.remove(&old_key) {
                    link.parent_order_id = parent_id;
                    link.tp_order_id = take_profit_id;
                    link.sl_order_id = stop_loss_id;
                    self.order_annotation_links.insert(parent_id, link);
                    tracing::info!(
                        "Reconciled bracket annotation: provisional {old_key} -> \
                         engine {parent_id} for {symbol}"
                    );
                }
            } else {
                // No local annotation found -- this bracket was created
                // externally (e.g., from a different client). Create a new
                // annotation from the engine event.
                let entry_price = reference_price.unwrap_or(0.0);
                return self.update(Message::BrokerBracketCreated {
                    parent_id,
                    take_profit_id,
                    stop_loss_id,
                    symbol,
                    action: side,
                    quantity,
                    entry_price: Some(entry_price),
                    tp_price,
                    sl_price,
                });
            }
        }
        BrokerEvent::BracketStatusChanged {
            parent_id,
            status,
            entry_fill_price,
        } => {
            use midas_chart::widget::order_bracket::BracketStatus;
            let chart_status = match status {
                midas_broker::BracketLifecycleStatus::Submitted => BracketStatus::Pending,
                midas_broker::BracketLifecycleStatus::EntryFilled => BracketStatus::Active,
                midas_broker::BracketLifecycleStatus::TakeProfitHit => BracketStatus::Closed,
                midas_broker::BracketLifecycleStatus::StopLossHit => BracketStatus::Closed,
                midas_broker::BracketLifecycleStatus::Cancelled => BracketStatus::Cancelled,
                midas_broker::BracketLifecycleStatus::Rejected => BracketStatus::Cancelled,
                midas_broker::BracketLifecycleStatus::Error => BracketStatus::Cancelled,
                midas_broker::BracketLifecycleStatus::Closed => BracketStatus::Closed,
            };
            return self.update(Message::BrokerBracketStatusChanged {
                parent_id,
                status: chart_status,
                entry_fill_price,
            });
        }
        BrokerEvent::OrderFilled {
            order_id,
            shares,
            price,
            commission,
            ..
        } => {
            tracing::info!(
                "Order filled: {order_id} {shares} shares @ {price:.2} \
                 (commission: {commission:?})"
            );
            let msg = format!(
                "Filled: {shares} @ ${price:.2}{}",
                commission
                    .map(|c| format!(" (comm ${c:.2})"))
                    .unwrap_or_default()
            );
            self.toast_message = Some(msg);
            self.toast_created_at = Some(Instant::now());
        }
        BrokerEvent::OrderRejected { order_id, reason } => {
            tracing::warn!("Order rejected: {order_id}: {reason}");
            self.toast_message = Some(format!("Order rejected: {reason}"));
            self.toast_created_at = Some(Instant::now());
        }
        BrokerEvent::OrderCancelled { order_id, reason } => {
            tracing::info!("Order cancelled: {order_id}: {reason}");
        }
        BrokerEvent::Connected { server_version } => {
            tracing::info!("Broker connected (server v{server_version})");
            self.status_message = format!("Broker connected (v{server_version})");
        }
        BrokerEvent::Disconnected { reason } => {
            tracing::warn!("Broker disconnected: {reason}");
            self.status_message = format!("Broker disconnected: {reason}");
        }
        BrokerEvent::OrderValidationFailed { message, code } => {
            tracing::warn!("Order validation failed [{code}]: {message}");
            self.toast_message = Some(format!("Validation: {message}"));
            self.toast_created_at = Some(Instant::now());
        }
        other => {
            tracing::trace!("Unhandled broker event: {other:?}");
        }
    }
    Task::none()
}
```

### Reconciliation algorithm

The matching logic finds a local `OrderAnnotationLink` using cached fields on the link itself (no annotation store lookup needed):

1. **Symbol match** -- `link.symbol == symbol`.
2. **Side match** -- `link.side == side` (cached `BracketSide` from creation time).
3. **Quantity match** -- `(link.quantity - quantity).abs() < 0.01` (cached quantity from creation time).
4. **FIFO ordering** -- candidates are sorted by `link.created_at` and the oldest match wins.

If multiple annotations match (e.g., two BUY 100 AAPL brackets submitted rapidly), the oldest link (by `created_at`) is matched first. The second `BracketCreated` event will match the second-oldest link. This FIFO ordering ensures correct reconciliation even under rapid submission.

### What to test

1. Place a bracket. Log shows `Reconciled bracket annotation: provisional <uuid-A> -> engine <uuid-B>`.
2. After reconciliation, drag a TP leg. Log shows `ModifyBracketLeg sent: order=<engine-uuid>` (not the provisional UUID).
3. Cancel the bracket from the context menu. Log shows `CancelBracket sent to broker engine for <engine-uuid>`.

### Done criteria

Engine UUIDs replace provisional UUIDs in `OrderAnnotationLink`. Modify and cancel operations use the engine's real UUIDs.

---

## Slice 7: Status updates from fills

**Goal:** When TestBroker fills the market order, `BracketStatusChanged(EntryFilled)` updates the annotation from Pending to Active. When TP/SL triggers, the annotation updates to Closed. Show toast messages for each lifecycle transition. Mark charts dirty for GPU re-render.

### Files to modify

| File | Change |
|------|--------|
| `desktop/win/crates/midas-app/src/app.rs` | Enhance `BrokerBracketStatusChanged` handler with toast messages and dirty flag propagation |

### Code changes

Enhance the existing `BrokerBracketStatusChanged` handler (lines 3049-3094):

```rust
Message::BrokerBracketStatusChanged {
    parent_id,
    status,
    entry_fill_price,
} => {
    if let Some(link) = self.order_annotation_links.get(&parent_id).cloned() {
        let ann_id = midas_chart::widget::AnnotationId(link.annotation_id);
        let updated = self.annotation_store.update(
            &link.symbol,
            ann_id,
            |ann| {
                if let midas_chart::widget::AnnotationKind::OrderBracket(
                    ref mut bracket,
                ) = ann.kind
                {
                    bracket.status = status;
                    if let Some(fill_price) = entry_fill_price {
                        bracket.entry.price = fill_price;
                    }
                }
            },
        );
        if updated {
            tracing::info!(
                "Bracket {ann_id} status -> {status:?} (parent={parent_id})"
            );
            // Mark charts dirty so GPU re-renders the bracket lines.
            self.mark_levels_dirty_for_ticker(&link.symbol);

            // Toast notification for significant status changes.
            use midas_chart::widget::order_bracket::BracketStatus;
            let toast = match status {
                BracketStatus::Active => {
                    let price_str = entry_fill_price
                        .map(|p| format!(" @ ${p:.2}"))
                        .unwrap_or_default();
                    Some(format!("{} entry filled{price_str}", link.symbol))
                }
                BracketStatus::Closed => {
                    Some(format!("{} bracket closed", link.symbol))
                }
                BracketStatus::Cancelled => {
                    Some(format!("{} bracket cancelled", link.symbol))
                }
                _ => None,
            };
            if let Some(msg) = toast {
                self.toast_message = Some(msg);
                self.toast_created_at = Some(Instant::now());
            }
        } else {
            tracing::warn!(
                "Bracket annotation {ann_id} not found in store for \
                 symbol {} (parent={parent_id})",
                link.symbol
            );
        }
    } else {
        tracing::warn!("No annotation link found for parent_id={parent_id}");
    }
    Task::none()
}
```

### Key behavior with TestBroker

TestBroker's default config uses `fill_timing = "instant"`. When `CreateMarketBracket` is processed:

1. Engine creates the bracket, emits `BracketCreated`.
2. Engine submits to TestBroker, which fills the market order immediately.
3. Engine emits `BracketStatusChanged { status: EntryFilled, entry_fill_price: Some(185.50) }`.
4. TP/SL children are now live at "exchange" (TestBroker's simulated book).
5. When a TP or SL triggers, engine emits `BracketStatusChanged { status: TakeProfitHit }` or `StopLossHit`.

### Annotation lifecycle on chart

```
OrderPanelConfirmYes -> Annotation created (Pending), local UUIDs
BracketCreated (engine) -> Reconcile UUIDs (no visual change)
BracketStatusChanged(EntryFilled) -> Annotation updated (Active), entry price = fill price
BracketStatusChanged(TakeProfitHit) -> Annotation updated (Closed)
```

### What to test

1. Place a BUY bracket on AAPL at ~185.
2. Within milliseconds, toast shows "AAPL entry filled @ $185.XX".
3. Chart bracket lines change from Pending color to Active color.
4. If TestBroker config has `fill_timing = "delayed"`, verify the bracket sits in Pending until the fill arrives.

### Done criteria

Annotation status transitions are driven by real broker events. Fill prices update the entry line. Toast messages notify the user. Charts are marked dirty for GPU re-render.

---

## Slice 8: Wire bracket leg drag to ModifyBracketLeg

**Goal:** When the user drags a TP or SL bracket leg on the chart, send `BrokerCommand::ModifyBracketLeg` to the engine in addition to updating the local annotation.

### Files to modify

| File | Change |
|------|--------|
| `desktop/win/crates/midas-app/src/app.rs` | Add broker command in `ChartDragBracketLeg` handler (after line 2954) |

### Code changes

In the `ChartDragBracketLeg` handler, after the annotation store update succeeds (line 2954, inside `if updated {`), add after the `mark_levels_dirty_for_ticker` call:

```rust
// Send price modification to broker engine.
if let Some(ref bridge) = self.broker_bridge {
    let order_id = self
        .order_annotation_links
        .values()
        .find(|link| link.annotation_id == annotation_id)
        .and_then(|link| {
            match leg {
                LegRole::TakeProfit => link.tp_order_id,
                LegRole::StopLoss => link.sl_order_id,
                LegRole::Entry => None, // unreachable (guarded above)
            }
        });
    if let Some(order_id) = order_id {
        if let Err(e) = bridge.modify_bracket_leg(order_id, new_price) {
            tracing::error!(
                "Failed to send ModifyBracketLeg to broker: {e}"
            );
        } else {
            tracing::debug!(
                "ModifyBracketLeg sent: order={order_id} price={new_price:.4}"
            );
        }
    } else {
        tracing::debug!(
            "No broker order ID for dragged leg (annotation {annotation_id}, \
             leg {leg:?}) -- visual-only move"
        );
    }
}
```

### Performance consideration

Bracket leg drag fires on every mouse-move pixel. Sending a command per pixel is fine because:
- `mpsc::try_send` is non-blocking (returns immediately).
- The broker engine processes commands sequentially; old modify commands are superseded by newer ones.
- TestBroker does not throttle modifications.

For live IB trading, the engine should debounce `ModifyBracketLeg` commands (batch rapid changes into one IB API call). That debounce belongs in the engine, not the UI.

### What to test

1. Place a bracket, wait for it to become Active (entry filled).
2. Drag the TP leg up. Log shows `ModifyBracketLeg sent: order=<uuid> price=...`.
3. Drag the SL leg down. Log shows similar.
4. Verify the annotation updates visually (already worked before this slice).

### Done criteria

Bracket leg drags produce `BrokerCommand::ModifyBracketLeg` commands. The engine processes them (TestBroker updates its internal price).

---

## Slice 9: Wire context menu cancel to CancelBracket (deferred link removal)

**Goal:** When the user cancels a bracket from the context menu, send `BrokerCommand::CancelBracket` to the engine. Keep the `OrderAnnotationLink` alive until the engine confirms cancellation via `BracketStatusChanged { status: Cancelled }`.

### Files to modify

| File | Change |
|------|--------|
| `desktop/win/crates/midas-app/src/app.rs` | Modify `BracketContextCancel` handler: defer link removal, send broker command |
| `desktop/win/crates/midas-app/src/app.rs` | Modify `BrokerBracketStatusChanged` handler: remove link on `Cancelled` |

### Code changes

**`BracketContextCancel` handler** -- replace lines 2977-2999:

```rust
Message::BracketContextCancel(parent_id) => {
    self.bracket_context_menu = None;

    // Look up the link but do NOT remove it yet.
    // The link stays alive until the engine confirms cancellation.
    if let Some(link) = self.order_annotation_links.get(&parent_id).cloned() {
        // Visually mark the annotation as Cancelled immediately.
        let ann_id = midas_chart::AnnotationId(link.annotation_id);
        self.annotation_store.update(&link.symbol, ann_id, |ann| {
            if let midas_chart::widget::AnnotationKind::OrderBracket(ref mut b) = ann.kind {
                b.status = midas_chart::widget::order_bracket::BracketStatus::Cancelled;
            }
        });
        self.mark_levels_dirty_for_ticker(&link.symbol);
        tracing::info!("Bracket {} cancel requested from context menu", parent_id);

        // Send cancellation to broker engine.
        if let Some(ref bridge) = self.broker_bridge {
            match bridge.cancel_bracket(parent_id) {
                Ok(()) => {
                    tracing::info!(
                        "CancelBracket sent to broker engine for {parent_id}"
                    );
                }
                Err(e) => {
                    tracing::error!(
                        "Failed to send CancelBracket to broker: {e}"
                    );
                }
            }
        } else {
            tracing::warn!(
                "No broker bridge: CancelBracket for {parent_id} not sent"
            );
            // No engine -- remove link now since there will be no confirmation.
            self.order_annotation_links.remove(&parent_id);
        }
    }
    self.toast_message = Some("Bracket cancelled".to_string());
    self.toast_created_at = Some(Instant::now());
    Task::none()
}
```

**`BrokerBracketStatusChanged` handler** -- add link removal when status is `Cancelled`. After the `if updated { ... }` block, inside the `if let Some(link)` scope:

```rust
// Remove the annotation link when the engine confirms cancellation.
use midas_chart::widget::order_bracket::BracketStatus;
if status == BracketStatus::Cancelled {
    self.order_annotation_links.remove(&parent_id);
    tracing::info!(
        "Annotation link removed for cancelled bracket {parent_id}"
    );
}
```

### Why defer removal

If the link is removed eagerly on cancel, the engine's subsequent `BracketStatusChanged { status: Cancelled }` event cannot find the link and logs "No annotation link found". Worse, if the cancel fails at the engine (e.g., the order is already filled), the UI has no link to update the annotation status. Keeping the link alive until confirmation ensures:

1. The engine's status event finds the link and updates the annotation.
2. If the engine rejects the cancel (e.g., already filled), the status event can correct the annotation back to Active.

### What to test

1. Place a bracket, let it fill (become Active).
2. Right-click a bracket leg, select "Cancel Bracket".
3. Annotation visually turns Cancelled immediately.
4. Log shows `CancelBracket sent to broker engine for <uuid>`.
5. Log shows `Annotation link removed for cancelled bracket <uuid>` (after engine confirms).
6. No "No annotation link found" warning.

### Done criteria

Context menu cancellation sends `CancelBracket` to the engine. The link is kept alive until the engine confirms. Link removal happens in the `BracketStatusChanged` handler.

---

## Slice 10: Connection state in status bar

**Goal:** Add a broker connection indicator to the status bar that watches the engine's `connection_state` channel and displays the current state with a colored dot.

### Files to modify

| File | Change |
|------|--------|
| `desktop/win/crates/midas-app/src/broker_bridge.rs` | Add `BrokerConnSource` newtype for connection subscription |
| `desktop/win/crates/midas-app/src/main.rs` | Add connection state subscription using `Subscription::run_with` |
| `desktop/win/crates/midas-app/src/app.rs` | Add `BrokerConnectionChanged(String)` message variant, new field, update handler |
| `desktop/win/crates/midas-app/src/app/views.rs` | Add broker indicator to status bar |

### Code changes

**`broker_bridge.rs`** -- add connection source newtype:

```rust
/// Wrapper around `watch::Receiver<ConnectionState>` that implements `Hash`
/// for `Subscription::run_with`.
#[derive(Clone)]
pub struct BrokerConnSource {
    pub receiver: watch::Receiver<midas_broker::ConnectionState>,
}

impl Hash for BrokerConnSource {
    fn hash<H: Hasher>(&self, state: &mut H) {
        "broker-conn-source".hash(state);
    }
}
```

**`broker_bridge.rs`** -- add method to `BrokerBridge`:

```rust
/// Create a `BrokerConnSource` for the connection state subscription.
pub fn conn_source(&self) -> BrokerConnSource {
    BrokerConnSource {
        receiver: self.connection_state.clone(),
    }
}
```

**`broker_bridge.rs`** -- add stream builder function:

```rust
/// Build a stream of connection state changes for iced subscription.
pub fn broker_conn_stream(
    source: &BrokerConnSource,
) -> impl iced::advanced::graphics::futures::Stream<Item = Message> {
    let mut conn_rx = source.receiver.clone();
    iced::stream::channel(16, async move |mut output| {
        loop {
            if conn_rx.changed().await.is_err() {
                // Sender dropped -- engine shut down.
                std::future::pending::<()>().await;
                break;
            }
            let state_str = conn_rx.borrow_and_update().to_string();
            let _ = output
                .send(Message::BrokerConnectionChanged(state_str))
                .await;
        }
    })
}
```

**New message variant** in `Message` enum:

```rust
/// Broker connection state changed.
BrokerConnectionChanged(String),
```

The payload is a display string (e.g., "Ready", "Disconnected") to avoid pulling `midas_broker::ConnectionState` into the `Message` enum.

**New field on `MidasApp`**:

```rust
/// Current broker connection state display string.
pub broker_connection_display: String,
```

Initialize in `new()`:

```rust
broker_connection_display: "Disconnected".to_string(),
```

**Subscription in `main.rs`** -- add alongside the order event subscription:

```rust
// Broker connection state subscription.
if let Some(ref bridge) = state.broker_bridge {
    let conn_source = bridge.conn_source();
    let conn_sub = Subscription::run_with(
        conn_source,
        crate::broker_bridge::broker_conn_stream,
    );
    subs.push(conn_sub);
}
```

**Update handler**:

```rust
Message::BrokerConnectionChanged(state_str) => {
    self.broker_connection_display = state_str;
    Task::none()
}
```

**Status bar view** (`views.rs`) -- add after the data provider indicator in `view_status_bar`:

```rust
text(" | ").size(12).color(theme::TEXT_MUTED),
{
    let broker_name = self.providers.active_broker_display_name();
    let (dot_color, label) = if self.broker_connection_display == "Ready" {
        (Color::from_rgb(0.2, 0.8, 0.2), format!("Broker: {broker_name}"))
    } else if self.broker_connection_display == "Disconnected" {
        (Color::from_rgb(0.6, 0.6, 0.6), format!("Broker: {}", self.broker_connection_display))
    } else {
        (Color::from_rgb(0.9, 0.7, 0.2), format!("Broker: {}", self.broker_connection_display))
    };
    row![
        text("\u{25CF}").size(10).color(dot_color),
        text(format!(" {label}")).size(12).color(theme::TEXT_SECONDARY),
    ]
    .align_y(iced::Alignment::Center)
},
```

### What to test

1. Start the application.
2. Status bar shows green dot + "Broker: Test Broker" (TestBroker auto-connects to Ready).
3. If you modify TestBroker config to `auto_connect = false`, status bar shows grey "Broker: Disconnected".

### Done criteria

Status bar displays the real-time broker connection state with a colored indicator. State updates are driven by the engine's `watch` channel through `Subscription::run_with`.

---

## Summary: Files Touched Per Slice

| Slice | New Files | Modified Files |
|-------|-----------|----------------|
| 0 | - | `crates/midas-broker/src/events.rs`, `crates/midas-broker/src/engine.rs`, `crates/midas-broker/src/lib.rs` |
| 1 | - | `desktop/win/Cargo.toml`, `desktop/win/crates/midas-app/Cargo.toml` |
| 2 | `desktop/win/crates/midas-app/src/broker_bridge.rs` | `desktop/win/crates/midas-app/src/main.rs` |
| 3 | - | `desktop/win/crates/midas-app/src/app.rs` |
| 4 | - | `desktop/win/crates/midas-app/src/app.rs` |
| 5 | - | `desktop/win/crates/midas-app/src/broker_bridge.rs`, `desktop/win/crates/midas-app/src/main.rs`, `desktop/win/crates/midas-app/src/app.rs` |
| 6 | - | `desktop/win/crates/midas-app/src/app.rs` |
| 7 | - | `desktop/win/crates/midas-app/src/app.rs` |
| 8 | - | `desktop/win/crates/midas-app/src/app.rs` |
| 9 | - | `desktop/win/crates/midas-app/src/app.rs` |
| 10 | - | `desktop/win/crates/midas-app/src/broker_bridge.rs`, `desktop/win/crates/midas-app/src/main.rs`, `desktop/win/crates/midas-app/src/app.rs`, `desktop/win/crates/midas-app/src/app/views.rs` |

## Design Decisions

1. **Local + reconcile pattern (Slices 4+6).** The order panel creates the annotation instantly with provisional UUIDs. The engine event reconciles UUIDs without creating a duplicate. This gives zero-latency UX while ensuring the engine is the source of truth for order IDs.

2. **Deferred link removal on cancel (Slice 9).** The `OrderAnnotationLink` stays alive until the engine confirms cancellation. This prevents "orphaned link" warnings and allows the engine to correct the annotation if cancellation fails.

3. **Boxed BrokerEvent in Message (Slice 5).** `BrokerEvent` is a large enum. Boxing it in `Message::BrokerEventReceived(Box<BrokerEvent>)` keeps the `Message` enum size small.

4. **No feature gating.** Broker is core functionality, always compiled. No `#[cfg(feature = "broker")]` anywhere.

5. **Hashable newtypes for subscriptions (Slices 5+10).** `broadcast::Sender` and `watch::Receiver` do not implement `Hash`. Newtype wrappers with constant hashes satisfy `Subscription::run_with`'s requirement while ensuring a single subscription instance.

6. **BracketCreated carries prices (Slice 0).** Adding `tp_price`, `sl_price`, and `reference_price` to the engine event means the event is self-contained for annotation creation (needed when a bracket is created externally and has no local annotation).

7. **Shutdown method on BrokerBridge (Slice 2).** Enables graceful engine shutdown on app exit.

## Open Questions

1. **TestBroker TP/SL trigger timing.** TestBroker uses `fill_timing = "instant"` for the market order, but TP/SL children only trigger when a price movement crosses their levels. TestBroker generates synthetic ticks, but the timing depends on configuration. For initial testing, manually verify that TP/SL triggers work by setting tight prices.

2. **Order persistence across restarts.** The annotation persistence system saves/loads bracket annotations to disk. After this bridge is wired, annotations will have real broker order UUIDs. On restart, the annotations are restored but the broker engine is a fresh instance with no memory of previous orders. A future "order recovery" feature would re-sync with the engine's SQLite database. For now, restored annotations are visual-only.

3. **Multiple rapid brackets for same symbol.** The reconciliation algorithm (Slice 6) matches by symbol + side + quantity. If two identical brackets are submitted in rapid succession, the first `BracketCreated` event matches the first link and the second matches the second. This works because the events arrive in order. However, if an event is lost (broadcast lag), a mismatch is possible. The lag handler logs a warning; manual intervention would be needed.

4. **Stream trait import.** The `broker_event_stream` and `broker_conn_stream` functions return `impl Stream`. If `iced::advanced::graphics::futures::Stream` is not the correct import path, try `iced::futures::Stream` or `futures::Stream` directly (iced 0.14 re-exports futures types).
