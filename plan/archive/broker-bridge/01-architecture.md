# Broker Bridge Architecture

Wiring the broker engine (`midas-broker`) into the desktop UI (`desktop/win/`)
so the user can place market order brackets against the TestBroker, see
simulated fills, and iterate on UX.

## 1. Component Diagram

```
desktop/win workspace                          root workspace
+---------------------------------------------------+  +---------------------------+
|                                                   |  |                           |
|  MidasApp                                         |  |  BrokerEngine             |
|  +----------------------------------------------+ |  |  +---------------------+  |
|  | order_panel: OrderPanelState                  | |  |  | TestBroker          |  |
|  | annotation_store: AnnotationStore             | |  |  | (BrokerClient impl) |  |
|  | order_annotation_links: HashMap<Uuid, Link>   | |  |  +---------------------+  |
|  | providers: ProviderRegistry                   | |  |                           |
|  | broker_bridge: Option<Arc<BrokerBridge>> -+---+-+--+-> BrokerHandle            |
|  +----------------------------------------------+ |  |    .commands (mpsc tx)     |
|                                                   |  |    .order_events (bcast)   |
|  subscription()                                   |  |    .market_events (bcast)  |
|  +----------------------------------------------+ |  |    .connection_state (watch)|
|  | broker_order_subscription                     | |  |                           |
|  |   broadcast::Receiver<BrokerEvent>            | |  +---------------------------+
|  |   --> maps to Message::BrokerEventReceived    | |
|  |       (Box<BrokerEvent>)                      | |
|  +----------------------------------------------+ |
|                                                   |
|  BrokerBridge (in midas-app)                      |
|  +----------------------------------------------+ |
|  | commands: mpsc::Sender<BrokerCommand>         | |
|  | order_events: broadcast::Sender<BrokerEvent>  | |
|  | connection_state: watch::Receiver<Conn..>     | |
|  +----------------------------------------------+ |
|  | Direct method calls (concrete type):          | |
|  |   create_market_bracket() -> send command     | |
|  |   cancel_bracket()        -> send command     | |
|  |   modify_bracket_leg()    -> send command     | |
|  +----------------------------------------------+ |
|  | impl provider::OrderBroker (display/status):  | |
|  |   name()                                      | |
|  |   is_connected()                              | |
|  |   connection_state()                          | |
|  +----------------------------------------------+ |
+---------------------------------------------------+
```

### Data Flow Summary

```
User clicks "Confirm & Submit"
  |
  v
MidasApp::update(OrderPanelConfirmYes)
  |-- Creates annotation locally (instant UX)
  |-- Stores OrderAnnotationLink with local UUIDs
  v
BrokerBridge::create_market_bracket(desktop::MarketBracketParams)
  |  translates to broker::MarketBracketParams
  v
mpsc::Sender::try_send(BrokerCommand::CreateMarketBracket(params))
  |
  v
BrokerEngine::run() loop receives command
  |
  v
TestBroker::place_order() -> callbacks -> engine emits BrokerEvent
  |
  v
broadcast::Sender<BrokerEvent>::send(BracketCreated { .. })
  |
  v
iced subscription drains broadcast::Receiver
  |
  v
Message::BrokerEventReceived(Box<BrokerEvent::BracketCreated { .. }>)
  |
  v
MidasApp::update() -> replaces local UUIDs in OrderAnnotationLink
  with engine's real UUIDs (matching by symbol + side + quantity)
```

## 2. BrokerBridge Adapter

The `BrokerBridge` struct lives in `midas-app` (the binary crate). It holds
the channel handles from `BrokerHandle` and translates between the two
workspace type systems.

**Two OrderBroker traits -- not unified.** The desktop workspace has two
`OrderBroker` traits that serve different purposes:

- `midas_core::provider::OrderBroker` -- display name, connection status.
  Used by `ProviderRegistry` (`Vec<Arc<dyn provider::OrderBroker>>`).
- `midas_core::broker::OrderBroker` -- order operations
  (`create_market_bracket`, `cancel_bracket`, `modify_bracket_leg`).

This plan does NOT unify them. Instead, all order operations go through the
`broker_bridge` field directly (concrete `Option<Arc<BrokerBridge>>`). The
`ProviderRegistry` registration is purely for display name and connection
status in the UI. `BrokerBridge` implements only `provider::OrderBroker`
for the registry. Order operations are called as concrete methods on
`BrokerBridge`, not through a trait.

### 2.1 Cross-Workspace Dependency

Add `midas-broker` as a direct dependency of the desktop `midas-app` crate.
No feature gating -- the broker is core functionality.

```toml
# desktop/win/crates/midas-app/Cargo.toml
[dependencies]
midas-broker = { path = "../../../../crates/midas-broker" }
```

This is valid because:
- `midas-broker` is NOT a member of the desktop workspace (it does not appear
  in `desktop/win/Cargo.toml [workspace] members`).
- Cargo resolves path dependencies that point outside the workspace as external
  crates. The ibapi transitive dependency is pulled in, but it only appears in
  `midas-app`'s build graph, not in any of the library crates.
- The architecture rule "no ibapi types leak through public API" is preserved:
  ibapi is a transitive dep of `midas-app` (binary crate) only. No library
  crate in the desktop workspace gains an ibapi dependency.

### 2.2 Import Paths and Re-exports

`midas-broker` re-exports these types at the crate root:
`BrokerCommand`, `BrokerConfig`, `BrokerEvent`, `BrokerHandle`,
`ConnectionState`, `MarketBracketParams`, `StopLossParams`,
`TakeProfitParams`, `BracketLifecycleStatus`, `OrderStatus`, `LocalOrder`,
`BrokerClient`, `TestBroker`, `TestBrokerConfig`, `start_broker_engine`.

Does NOT re-export:
- `SecurityType` -- lives in root `midas-core`, NOT in `midas-broker`.
  **Solution:** add `pub use midas_core::SecurityType;` to
  `crates/midas-broker/src/lib.rs` so the bridge can use
  `midas_broker::SecurityType`.
- `OrderAction` -- use `midas_broker::orders::types::OrderAction`
- `TimeInForce` -- use `midas_broker::orders::types::TimeInForce`

### 2.3 Struct Definition

```rust
// desktop/win/crates/midas-app/src/broker_bridge.rs

use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, watch};

use midas_broker::{
    BrokerCommand, BrokerConfig, BrokerEvent, BrokerHandle,
    ConnectionState as BrokerConnectionState,
    MarketBracketParams as BrokerMarketBracketParams,
    SecurityType as BrokerSecurityType,
    StopLossParams as BrokerStopLossParams,
    TakeProfitParams as BrokerTakeProfitParams,
    start_broker_engine,
};
use midas_broker::orders::types::{
    OrderAction as BrokerOrderAction,
    TimeInForce as BrokerTif,
};

use midas_core::broker::{
    MarketBracketParams as DesktopMarketBracketParams,
    OrderAction as DesktopOrderAction,
    StopLossParams as DesktopStopLossParams,
    TakeProfitParams as DesktopTakeProfitParams,
    TimeInForce as DesktopTif,
};
use midas_core::provider::{
    ConnectionState as DesktopConnectionState,
    OrderBroker as ProviderOrderBroker,
};

/// Bridges the broker engine channels into the desktop UI.
///
/// Created once at app startup. Registered in `ProviderRegistry` for
/// display name / connection status (via `provider::OrderBroker` impl).
/// Order operations are called directly on the concrete type, NOT through
/// a trait. Commands are sent via `try_send` (non-blocking).
/// Events are received via a subscription that drains the broadcast receiver.
pub struct BrokerBridge {
    /// Send commands to the broker engine. Cloned from BrokerHandle.
    commands: mpsc::Sender<BrokerCommand>,
    /// Order event broadcast sender -- used to create new Receivers for
    /// the iced subscription. Cloned from BrokerHandle.
    order_events: broadcast::Sender<BrokerEvent>,
    /// Latest connection state from the engine.
    connection_state: watch::Receiver<BrokerConnectionState>,
    /// Display name for the UI.
    name: String,
}
```

### 2.4 Constructor

```rust
impl BrokerBridge {
    /// Wrap a `BrokerHandle` in a bridge with the given display name.
    ///
    /// The caller is responsible for starting the engine
    /// (`start_broker_engine`) and passing the resulting handle here.
    pub fn new(handle: BrokerHandle, name: String) -> Self {
        Self {
            commands: handle.commands,
            order_events: handle.order_events,
            connection_state: handle.connection_state,
            name,
        }
    }

    /// Create a new broadcast::Receiver for order events.
    ///
    /// Called by the iced subscription to get its own receiver handle.
    /// Each subscriber gets events independently (broadcast semantics).
    pub fn subscribe_order_events(&self) -> broadcast::Receiver<BrokerEvent> {
        self.order_events.subscribe()
    }

    /// Clone the command sender for use in closures.
    pub fn command_sender(&self) -> mpsc::Sender<BrokerCommand> {
        self.commands.clone()
    }

    /// Clone the order events broadcast sender.
    ///
    /// Used by the subscription wrapper (which needs a `Hash`-able handle).
    pub fn order_events_sender(&self) -> broadcast::Sender<BrokerEvent> {
        self.order_events.clone()
    }
}
```

### 2.5 Order Operations (Direct Methods, Not Trait)

Order operations are concrete methods on `BrokerBridge`. The app calls
these through the `broker_bridge: Option<Arc<BrokerBridge>>` field directly.

```rust
impl BrokerBridge {
    /// Create and submit a market bracket order.
    pub fn create_market_bracket(
        &self,
        params: DesktopMarketBracketParams,
    ) -> Result<(), String> {
        let broker_params = translate_bracket_params(params);
        self.commands
            .try_send(BrokerCommand::CreateMarketBracket(broker_params))
            .map_err(|e| format!("failed to send CreateMarketBracket: {e}"))
    }

    /// Cancel an entire bracket.
    pub fn cancel_bracket(&self, parent_id: uuid::Uuid) -> Result<(), String> {
        self.commands
            .try_send(BrokerCommand::CancelBracket { parent_id })
            .map_err(|e| format!("failed to send CancelBracket: {e}"))
    }

    /// Modify a bracket leg's price.
    pub fn modify_bracket_leg(
        &self,
        order_id: uuid::Uuid,
        new_price: f64,
    ) -> Result<(), String> {
        self.commands
            .try_send(BrokerCommand::ModifyBracketLeg { order_id, new_price })
            .map_err(|e| format!("failed to send ModifyBracketLeg: {e}"))
    }
}
```

**Why `try_send` instead of `.send().await`?**

The iced update function is synchronous. The mpsc channel has capacity 256,
which is far more than a human can saturate. If the channel is full, the send
fails with an error that is displayed as a toast.

### 2.6 Provider OrderBroker Impl (Display/Status Only)

`BrokerBridge` implements `provider::OrderBroker` purely for
`ProviderRegistry` registration. This trait provides display name and
connection status -- no order operations.

```rust
#[async_trait]
impl ProviderOrderBroker for BrokerBridge {
    fn name(&self) -> &str {
        &self.name
    }

    fn is_connected(&self) -> bool {
        self.connection_state.borrow().is_connected()
    }

    fn connection_state(&self) -> DesktopConnectionState {
        translate_connection_state(&self.connection_state.borrow())
    }
}
```

### 2.7 Registration

During `MidasApp::new()`, after config is loaded:

```rust
// In MidasApp::new(), after providers are set up:
let broker_config = build_broker_config(&app_config);
let handle = start_broker_engine(broker_config);
let bridge = Arc::new(BrokerBridge::new(handle, "Test Broker".to_string()));
app.providers.register_order_broker(bridge.clone());
app.providers.set_active_broker(Some(0));
app.broker_bridge = Some(bridge);
```

`MidasApp` gains a new field:

```rust
pub struct MidasApp {
    // ... existing fields ...
    /// Broker bridge handle. `None` until the engine is started.
    /// Used directly for order operations and registered in
    /// ProviderRegistry for display name / status.
    pub broker_bridge: Option<Arc<BrokerBridge>>,
}
```

## 3. Event Flow: Broker --> UI

### 3.1 Message Enum

Box the `BrokerEvent` inside the `Message` to avoid bloating the enum
(BrokerEvent has many large variants with Strings, Vecs, etc.):

```rust
pub enum Message {
    // ... existing variants ...

    /// A broker event received from the engine via the subscription.
    BrokerEventReceived(Box<midas_broker::BrokerEvent>),

    /// The broker engine died (broadcast channel closed).
    BrokerEngineDied,
}
```

### 3.2 Subscription Design

Use `iced::stream::channel` + `Subscription::run_with` to drain the broker's
broadcast receiver and emit iced `Message` variants.

**Important:** `iced::subscription::channel(id, size, closure)` does NOT
exist in iced 0.14. The correct API is:

- `iced::stream::channel(size, async |sender| { ... })` returns
  `impl Stream<Item = T>`
- `Subscription::run_with(data, fn_ptr)` where `data: D: Hash + 'static`
  and `fn_ptr: fn(&D) -> impl Stream<Item = T>`

**Problem:** `broadcast::Sender<BrokerEvent>` does NOT implement `Hash`,
which `Subscription::run_with` requires. **Solution:** wrap it in a newtype
with a manual `Hash` impl:

```rust
// desktop/win/crates/midas-app/src/broker_bridge.rs (continued)

use iced::Subscription;
use iced::futures::SinkExt;
use std::hash::{Hash, Hasher};

/// Wrapper around broadcast::Sender that implements Hash for iced's
/// Subscription::run_with. The hash is a fixed constant because there
/// is exactly one broker subscription. Iced uses the hash to deduplicate
/// subscriptions; a constant value means "this subscription is always
/// the same identity".
struct BrokerEventSender(broadcast::Sender<BrokerEvent>);

impl Hash for BrokerEventSender {
    fn hash<H: Hasher>(&self, state: &mut H) {
        "broker_order_subscription".hash(state);
    }
}

/// Create an iced subscription that drains broker order events and maps
/// them to application Messages.
///
/// Called from the top-level `subscription()` function.
pub fn broker_order_subscription(
    order_events: broadcast::Sender<BrokerEvent>,
) -> Subscription<super::app::Message> {
    Subscription::run_with(
        BrokerEventSender(order_events),
        broker_event_stream,
    )
}

fn broker_event_stream(
    sender_wrapper: &BrokerEventSender,
) -> impl iced::advanced::graphics::futures::Stream<Item = super::app::Message> + 'static
{
    let mut rx = sender_wrapper.0.subscribe();

    iced::stream::channel(256, async move |mut output| {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    if let Some(msg) = translate_broker_event(event) {
                        // Await delivery -- order events are trading-critical
                        // and must not be silently dropped.
                        // Requires `use iced::futures::SinkExt;`
                        let _ = output.send(msg).await;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(
                        "Broker order subscription lagged by {n} events"
                    );
                    // Continue -- next recv() will return the latest.
                }
                Err(broadcast::error::RecvError::Closed) => {
                    tracing::error!("Broker engine died: order event channel closed");
                    let _ = output.send(
                        super::app::Message::BrokerEngineDied
                    ).await;
                    // Engine shut down. The subscription completes.
                    // Use futures::future::pending() to keep the
                    // subscription alive without busy-spinning.
                    futures::future::pending::<()>().await;
                }
            }
        }
    })
}
```

### 3.3 Event Translation

All broker events are wrapped in `Box` and forwarded as
`Message::BrokerEventReceived`. The `update()` handler does the routing.
The `translate_broker_event` function filters which events the UI cares
about:

```rust
fn translate_broker_event(event: BrokerEvent) -> Option<Message> {
    match &event {
        BrokerEvent::BracketCreated { .. }
        | BrokerEvent::BracketStatusChanged { .. }
        | BrokerEvent::OrderFilled { .. }
        | BrokerEvent::OrderRejected { .. }
        | BrokerEvent::OrderCancelled { .. }
        | BrokerEvent::OrderValidationFailed { .. } => {
            Some(Message::BrokerEventReceived(Box::new(event)))
        }
        // Connection events update status bar via connection_state watch,
        // not through order_events. Ignore here.
        _ => None,
    }
}
```

### 3.4 Update Handler for BrokerEventReceived

The `MidasApp::update()` match arm for `BrokerEventReceived` routes by
variant:

```rust
Message::BrokerEventReceived(boxed_event) => {
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
            // Replace local UUIDs in the OrderAnnotationLink with the
            // engine's real UUIDs. Match by symbol + side + quantity.
            self.replace_annotation_link_ids(
                &symbol,
                &action,
                quantity,
                parent_id,
                take_profit_id,
                stop_loss_id,
            );
        }

        BrokerEvent::BracketStatusChanged {
            parent_id,
            status,
            entry_fill_price,
        } => {
            let chart_status = translate_lifecycle_status(status);
            self.update_annotation_status(parent_id, chart_status, entry_fill_price);
        }

        BrokerEvent::OrderFilled {
            shares, price, commission, ..
        } => {
            let msg = format!(
                "Filled: {shares} @ {price:.2}{}",
                commission
                    .map(|c| format!(" (comm: ${c:.2})"))
                    .unwrap_or_default()
            );
            self.show_toast(msg);
        }

        BrokerEvent::OrderRejected { reason, .. } => {
            self.show_toast(format!("Order rejected: {reason}"));
        }

        BrokerEvent::OrderCancelled { reason, .. } => {
            self.show_toast(format!("Order cancelled: {reason}"));
        }

        BrokerEvent::OrderValidationFailed { message, .. } => {
            self.show_toast(format!("Validation failed: {message}"));
        }

        _ => {}
    }
    Task::none()
}

Message::BrokerEngineDied => {
    tracing::error!("Broker engine died unexpectedly");
    self.status_message = "Broker: ENGINE DIED".to_string();
    // UI shows red indicator in status bar.
    Task::none()
}
```

### 3.5 Lifecycle Status Translation

```rust
fn translate_lifecycle_status(
    status: midas_broker::BracketLifecycleStatus,
) -> midas_chart::widget::order_bracket::BracketStatus {
    use midas_broker::BracketLifecycleStatus as BLS;
    use midas_chart::widget::order_bracket::BracketStatus;

    match status {
        BLS::Submitted          => BracketStatus::Pending,
        BLS::EntryFilled        => BracketStatus::Active,
        BLS::TakeProfitHit      => BracketStatus::Closed,
        BLS::StopLossHit        => BracketStatus::Closed,
        BLS::Cancelled          => BracketStatus::Cancelled,
        BLS::Rejected           => BracketStatus::Cancelled,
        BLS::Error              => BracketStatus::Cancelled,
        BLS::Closed             => BracketStatus::Closed,
    }
}
```

This replaces the existing string-based `map_lifecycle_to_chart_status()` in
`order_panel.rs` with a type-safe version.

### 3.6 Wiring Into the Subscription Function

```rust
// desktop/win/crates/midas-app/src/main.rs -- subscription()

fn subscription(state: &MidasApp) -> Subscription<Message> {
    let mut subs = vec![
        keyboard_sub,
        tick_sub,
        close_sub,
        window_events_sub,
        market_refresh,
        cursor_sub,
    ];

    // ... existing drag subs ...

    // Broker event subscription.
    if let Some(ref bridge) = state.broker_bridge {
        subs.push(broker_bridge::broker_order_subscription(
            bridge.order_events_sender(),
        ));
    }

    Subscription::batch(subs)
}
```

**Note:** The `broker_order_subscription` takes a `broadcast::Sender` (not
`Receiver`) because the `run_with` builder function creates its own
`Receiver` via `.subscribe()` inside. This avoids lifetime issues with
borrowing from `MidasApp`. The `BrokerEventSender` newtype wrapper provides
the `Hash` impl that `Subscription::run_with` requires.

## 4. Command Flow: UI --> Broker

### 4.1 OrderPanelConfirmYes -- Create Annotation Locally, Then Send Command

The annotation is created immediately in the `OrderPanelConfirmYes` handler
for instant UX feedback. Local UUIDs are generated for the annotation link.
When the engine's `BracketCreated` event arrives, the link's UUIDs are
replaced with the engine's real order IDs.

```rust
Message::OrderPanelConfirmYes => {
    self.order_panel.showing_confirmation = false;

    let panel = &self.order_panel;
    let last_price = panel.last_price.unwrap_or(0.0);

    // Resolve TP/SL prices (existing code, unchanged).
    let tp_price = if panel.tp_enabled {
        panel.tp_value.parse::<f64>().ok().map(|val| {
            resolve_price(panel.tp_mode, val, last_price, panel.side, true)
        })
    } else {
        None
    };
    let sl_price = if panel.sl_enabled {
        panel.sl_value.parse::<f64>().ok().map(|val| {
            resolve_price(panel.sl_mode, val, last_price, panel.side, false)
        })
    } else {
        None
    };

    // Build desktop-side params.
    let desktop_params = DesktopMarketBracketParams {
        symbol: panel.symbol.clone(),
        con_id: None,
        sec_type: midas_core::SecurityType::Stock,
        exchange: "SMART".to_string(),
        currency: "USD".to_string(),
        action: match panel.side {
            OrderSide::Buy => DesktopOrderAction::Buy,
            OrderSide::Sell => DesktopOrderAction::Sell,
        },
        quantity: panel.quantity.parse().unwrap_or(100.0),
        outside_rth: false,
        take_profit: tp_price.map(|p| DesktopTakeProfitParams {
            price: p,
            tif: None,
        }),
        stop_loss: sl_price.map(|p| DesktopStopLossParams {
            stop_price: p,
            limit_price: None,
            tif: None,
        }),
        reference_price: Some(last_price),
        strategy: None,
        tags: Vec::new(),
    };

    // --- Create annotation locally for instant UX ---
    let local_parent_id = uuid::Uuid::new_v4();
    let local_tp_id = tp_price.map(|_| uuid::Uuid::new_v4());
    let local_sl_id = sl_price.map(|_| uuid::Uuid::new_v4());

    let side = match panel.side {
        OrderSide::Buy => BracketSide::Long,
        OrderSide::Sell => BracketSide::Short,
    };

    self.create_bracket_annotation(
        &panel.symbol,
        side,
        panel.quantity.parse().unwrap_or(100.0),
        last_price,       // entry (MKT, use reference price for line)
        tp_price,
        sl_price,
    );

    // Store the annotation link with LOCAL UUIDs. These will be replaced
    // with the engine's real UUIDs when BracketCreated arrives.
    self.order_annotation_links.insert(local_parent_id, OrderAnnotationLink {
        annotation_id: self.last_created_annotation_id(),
        parent_order_id: local_parent_id,
        tp_order_id: local_tp_id,
        sl_order_id: local_sl_id,
        symbol: panel.symbol.clone(),
        side,
        quantity: panel.quantity.parse().unwrap_or(100.0),
    });

    // --- Send command to broker engine ---
    if let Some(ref bridge) = self.broker_bridge {
        match bridge.create_market_bracket(desktop_params) {
            Ok(()) => {
                tracing::info!(
                    "CreateMarketBracket sent for {}",
                    panel.symbol,
                );
                self.status_message = format!(
                    "Order submitted: {} {} {}",
                    match panel.side {
                        OrderSide::Buy => "BUY",
                        OrderSide::Sell => "SELL",
                    },
                    panel.quantity,
                    panel.symbol,
                );
            }
            Err(e) => {
                tracing::error!("Failed to send bracket command: {e}");
                self.status_message = format!("Order failed: {e}");
                // Annotation is already created. It will stay in Pending
                // state until manually removed or engine comes back.
            }
        }
    } else {
        self.status_message = "No broker connected".to_string();
    }

    // Close panel.
    self.order_panel.visible = false;
    Task::none()
}
```

**Key design:** The annotation is created locally in `OrderPanelConfirmYes`
for instant visual feedback. The `OrderAnnotationLink` is stored with local
UUIDs and extra matching fields (symbol, side, quantity). When
`BrokerEvent::BracketCreated` arrives, the handler calls
`replace_annotation_link_ids` to find the matching link and replace the
local UUIDs with the engine's real order IDs.

```rust
impl MidasApp {
    /// Replace local UUIDs in an annotation link with engine-assigned UUIDs.
    /// Matches by symbol + action side + quantity.
    fn replace_annotation_link_ids(
        &mut self,
        symbol: &str,
        action: &BrokerOrderAction,
        quantity: f64,
        engine_parent_id: uuid::Uuid,
        engine_tp_id: Option<uuid::Uuid>,
        engine_sl_id: Option<uuid::Uuid>,
    ) {
        let expected_side = match action {
            BrokerOrderAction::Buy => BracketSide::Long,
            BrokerOrderAction::Sell => BracketSide::Short,
        };

        // Find the first link that matches symbol + side + quantity and
        // still has local (non-engine) UUIDs.
        let local_key = self.order_annotation_links.iter()
            .find(|(_, link)| {
                link.symbol == symbol
                    && link.side == expected_side
                    && (link.quantity - quantity).abs() < f64::EPSILON
            })
            .map(|(k, _)| *k);

        if let Some(old_key) = local_key {
            if let Some(mut link) = self.order_annotation_links.remove(&old_key) {
                link.parent_order_id = engine_parent_id;
                link.tp_order_id = engine_tp_id;
                link.sl_order_id = engine_sl_id;
                self.order_annotation_links.insert(engine_parent_id, link);
            }
        } else {
            tracing::warn!(
                "BracketCreated for {symbol} but no matching local annotation link"
            );
        }
    }
}
```

### 4.2 ChartDragBracketLeg

When the user drags a TP or SL leg on the chart, the new price is sent to
the broker engine to modify the order:

```rust
Message::ChartDragBracketLeg(chart_id, annotation_id, leg, new_price) => {
    // ... existing annotation update code (unchanged) ...

    // Find the broker order ID for this leg from the annotation link.
    let order_id = self.find_order_id_for_leg(annotation_id, leg);
    if let (Some(order_id), Some(ref bridge)) = (order_id, &self.broker_bridge) {
        if let Err(e) = bridge.modify_bracket_leg(order_id, new_price) {
            tracing::error!("Failed to send ModifyBracketLeg: {e}");
        }
    }

    Task::none()
}
```

Helper method on `MidasApp`:

```rust
impl MidasApp {
    /// Find the broker order UUID for a bracket leg, given its annotation ID
    /// and leg role.
    fn find_order_id_for_leg(
        &self,
        annotation_id: u64,
        leg: LegRole,
    ) -> Option<uuid::Uuid> {
        self.order_annotation_links
            .values()
            .find(|link| link.annotation_id == annotation_id)
            .and_then(|link| match leg {
                LegRole::Entry => Some(link.parent_order_id),
                LegRole::TakeProfit => link.tp_order_id,
                LegRole::StopLoss => link.sl_order_id,
            })
    }
}
```

### 4.3 BracketContextCancel

```rust
Message::BracketContextCancel(parent_id) => {
    self.bracket_context_menu = None;

    if let Some(ref bridge) = self.broker_bridge {
        if let Err(e) = bridge.cancel_bracket(parent_id) {
            tracing::error!("Failed to send CancelBracket: {e}");
        }
    }

    // Do NOT remove the annotation link here. The link stays until the
    // engine confirms cancellation via BracketStatusChanged { status:
    // Cancelled }. This prevents the UI from losing track of the bracket
    // if the cancel command fails or is rejected.
    Task::none()
}
```

**Deferred removal:** The `order_annotation_links` entry is only removed (or
marked as terminal) when `BracketStatusChanged` arrives with `Cancelled`,
`Rejected`, `Closed`, `StopLossHit`, or `TakeProfitHit`. This ensures the
UI stays consistent if a cancel command is rejected by the engine.

## 5. Lifecycle

### 5.1 Startup Sequence

```
main()
  |
  v
MidasApp::new()
  |-- Load AppConfig from TOML
  |-- Read [broker] section (or use defaults)
  |-- Build BrokerConfig
  |-- start_broker_engine(config) spawns tokio task
  |     |-- TestBroker with auto_connect=true transitions to Ready
  |     '-- Returns BrokerHandle with channels
  |-- BrokerBridge::new(handle, name)
  |-- Wrap in Some(Arc<BrokerBridge>)
  |-- Register in ProviderRegistry (for display name / status)
  |-- Store in app.broker_bridge
  |
  v
iced::daemon starts event loop
  |
  v
subscription() called -- includes broker_order_subscription
  |
  v
Subscription creates broadcast::Receiver, begins draining
```

**Timing:** `start_broker_engine` returns immediately. The engine's tokio task
begins its `run()` loop. With `TestBrokerConfig::auto_connect = true` (the
default), the engine transitions to `ConnectionState::Ready` within its first
poll cycle (~10ms). By the time the first iced frame renders, the broker is
ready.

### 5.2 Connection State Display

The status bar already shows `status_message`. Add a broker connection
indicator that reads from the watch channel:

```rust
// In the status bar view:
let conn_text = if let Some(ref bridge) = state.broker_bridge {
    let cs = bridge.connection_state();
    match cs {
        DesktopConnectionState::Ready => "Broker: Ready",
        DesktopConnectionState::Connected { .. } => "Broker: Connected",
        DesktopConnectionState::Connecting => "Broker: Connecting...",
        DesktopConnectionState::Reconnecting { attempt } => {
            &format!("Broker: Reconnecting ({})", attempt)
        }
        DesktopConnectionState::Disconnected => "Broker: Disconnected",
    }
} else {
    "Broker: Not started"
};
// Render conn_text in the status bar
```

The `connection_state()` call reads the `watch::Receiver` via `.borrow()`,
which is non-blocking and always returns the latest value.

### 5.3 Engine Crash Detection

When the broadcast receiver closes (engine died), the subscription emits
`Message::BrokerEngineDied`. The update handler sets a status message and
the status bar shows a red indicator:

```rust
Message::BrokerEngineDied => {
    tracing::error!("Broker engine died unexpectedly");
    self.status_message = "Broker: ENGINE DIED".to_string();
    // Status bar can check this flag to show red indicator.
    self.broker_engine_alive = false;
    Task::none()
}
```

### 5.4 Graceful Shutdown

When the user closes the main window (`Message::WindowCloseRequested`):

```rust
Message::WindowCloseRequested => {
    // Send shutdown command to broker engine.
    if let Some(ref bridge) = self.broker_bridge {
        let _ = bridge.commands.try_send(BrokerCommand::Shutdown);
    }

    // Save config and exit.
    self.flush_config().chain(iced::exit())
}
```

The engine's `run()` loop handles `BrokerCommand::Shutdown` by breaking out
of the loop, which drops the tokio task. All channel senders are dropped,
causing receivers to see `Closed` errors and complete gracefully.

No explicit join or timeout is needed: the engine has no persistent state
that requires flushing (the TestBroker operates in-memory). For future IB
connections, the shutdown command will close the TCP socket first.

## 6. Config Integration

### 6.1 AppConfig Extension

Add a `[broker]` section to `AppConfig`:

```rust
// desktop/win/crates/midas-core/src/config.rs

/// Broker engine configuration.
///
/// Serialized as the `[broker]` section in `config.toml`.
/// All fields have sensible defaults for TestBroker development.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BrokerAppConfig {
    /// Broker mode: "test" (default) or "paper" or "live".
    #[serde(default = "default_broker_mode")]
    pub mode: String,
    /// Whether to auto-start the broker engine on app launch.
    #[serde(default = "default_auto_start")]
    pub auto_start: bool,
}

fn default_broker_mode() -> String {
    "test".to_string()
}

fn default_auto_start() -> bool {
    true
}

// In AppConfig:
pub struct AppConfig {
    // ... existing fields ...
    /// Broker engine settings.
    #[serde(default)]
    pub broker: BrokerAppConfig,
}
```

### 6.2 BrokerConfig Construction

In `midas-app`, translate `BrokerAppConfig` into `midas_broker::BrokerConfig`:

```rust
fn build_broker_config(app_config: &AppConfig) -> BrokerConfig {
    let mut config = BrokerConfig::default();

    match app_config.broker.mode.as_str() {
        "test" => {
            config.data_source = DataSourceConfig::Test;
            config.test_broker.auto_connect = true;
        }
        "paper" => {
            config.data_source = DataSourceConfig::Live;
            config.connection.port = 4002;
            config.connection.allow_live = false;
        }
        "live" => {
            config.data_source = DataSourceConfig::Live;
            config.connection.port = 4001;
            config.connection.allow_live = true;
        }
        other => {
            tracing::warn!("Unknown broker mode '{other}', defaulting to test");
            config.data_source = DataSourceConfig::Test;
            config.test_broker.auto_connect = true;
        }
    }

    config
}
```

### 6.3 Example config.toml

```toml
[broker]
mode = "test"
auto_start = true
```

For paper trading (Phase 1 of the roadmap):

```toml
[broker]
mode = "paper"
auto_start = true
```

## 7. Type Translation Layer

The desktop workspace has mirror types in `desktop/win/crates/midas-core/src/broker.rs`
that replicate the broker engine types. The `BrokerBridge` translates between
them at the boundary.

### 7.1 SecurityType

Both workspaces have their own `SecurityType` enum. The root `midas-core`
defines the canonical one. `midas-broker` re-exports it (after adding
`pub use midas_core::SecurityType;` to its `lib.rs`). The desktop
`midas-core` has a mirror copy. The translation function bridges them:

```rust
fn translate_security_type(
    st: midas_core::SecurityType,   // desktop's SecurityType
) -> BrokerSecurityType {           // midas_broker::SecurityType (= root midas_core's)
    match st {
        midas_core::SecurityType::Stock  => BrokerSecurityType::Stock,
        midas_core::SecurityType::Option => BrokerSecurityType::Option,
        midas_core::SecurityType::Future => BrokerSecurityType::Future,
        midas_core::SecurityType::Forex  => BrokerSecurityType::Forex,
    }
}
```

Note: `midas_core` in the bridge file refers to the desktop workspace's
`midas-core` (`desktop/win/crates/midas-core/`). `BrokerSecurityType` is
`midas_broker::SecurityType`, which is the root `crates/midas-core/`
`SecurityType` re-exported through `midas-broker`.

### 7.2 MarketBracketParams

```
Desktop (midas_core::broker)         Broker (midas_broker::orders::bracket)
---------------------------------    ------------------------------------
symbol: String                   --> symbol: String
con_id: Option<i32>              --> con_id: Option<i32>
sec_type: desktop::SecurityType  --> sec_type: root::SecurityType     (*)
exchange: String                 --> exchange: String
currency: String                 --> currency: String
action: desktop::OrderAction     --> action: broker::OrderAction
quantity: f64                    --> quantity: f64
outside_rth: bool                --> outside_rth: bool
take_profit: Option<DesktopTP>   --> take_profit: Option<BrokerTP>
stop_loss: Option<DesktopSL>     --> stop_loss: Option<BrokerSL>
reference_price: Option<f64>     --> reference_price: Option<f64>
strategy: Option<String>         --> strategy: Option<String>
tags: Vec<String>                --> tags: Vec<String>
```

(*) See section 7.1 for the SecurityType translation.

### 7.3 OrderAction

```rust
fn translate_order_action(
    action: DesktopOrderAction,
) -> BrokerOrderAction {
    match action {
        DesktopOrderAction::Buy  => BrokerOrderAction::Buy,
        DesktopOrderAction::Sell => BrokerOrderAction::Sell,
    }
}
```

### 7.4 TimeInForce

```rust
fn translate_tif(tif: DesktopTif) -> BrokerTif {
    match tif {
        DesktopTif::Day => BrokerTif::Day,
        DesktopTif::Gtc => BrokerTif::Gtc,
        DesktopTif::Ioc => BrokerTif::Ioc,
        DesktopTif::Gtd => BrokerTif::Gtd,
        DesktopTif::Opg => BrokerTif::Opg,
    }
}
```

### 7.5 TakeProfitParams / StopLossParams

```rust
fn translate_tp(tp: DesktopTakeProfitParams) -> BrokerTakeProfitParams {
    BrokerTakeProfitParams {
        price: tp.price,
        tif: tp.tif.map(translate_tif),
    }
}

fn translate_sl(sl: DesktopStopLossParams) -> BrokerStopLossParams {
    BrokerStopLossParams {
        stop_price: sl.stop_price,
        limit_price: sl.limit_price,
        tif: sl.tif.map(translate_tif),
    }
}
```

### 7.6 Full Bracket Params Translation

```rust
fn translate_bracket_params(
    params: DesktopMarketBracketParams,
) -> BrokerMarketBracketParams {
    BrokerMarketBracketParams {
        symbol: params.symbol,
        con_id: params.con_id,
        sec_type: translate_security_type(params.sec_type),
        exchange: params.exchange,
        currency: params.currency,
        action: translate_order_action(params.action),
        quantity: params.quantity,
        outside_rth: params.outside_rth,
        take_profit: params.take_profit.map(translate_tp),
        stop_loss: params.stop_loss.map(translate_sl),
        reference_price: params.reference_price,
        strategy: params.strategy,
        tags: params.tags,
    }
}
```

### 7.7 ConnectionState Translation

```rust
fn translate_connection_state(
    cs: &BrokerConnectionState,
) -> DesktopConnectionState {
    match cs {
        BrokerConnectionState::Disconnected => {
            DesktopConnectionState::Disconnected
        }
        BrokerConnectionState::Connecting => {
            DesktopConnectionState::Connecting
        }
        BrokerConnectionState::Connected { server_version } => {
            DesktopConnectionState::Connected {
                server_version: *server_version,
            }
        }
        BrokerConnectionState::Ready => {
            DesktopConnectionState::Ready
        }
        BrokerConnectionState::Reconnecting { attempt } => {
            DesktopConnectionState::Reconnecting { attempt: *attempt }
        }
    }
}
```

### 7.8 BracketLifecycleStatus Translation

```
Desktop (midas_core::broker)         Broker (midas_broker::orders::bracket)
---------------------------------    ------------------------------------
Submitted                        <-- Submitted
EntryFilled                      <-- EntryFilled
TakeProfitHit                    <-- TakeProfitHit
StopLossHit                      <-- StopLossHit
Cancelled                        <-- Cancelled
Rejected                         <-- Rejected
Error                            <-- Error
Closed                           <-- Closed
```

All variants are 1:1 mirrors. The translation in section 3.5 maps them to
the coarser chart-side `BracketStatus` enum for rendering.

## 8. BrokerEvent Enhancement: Carrying Prices

The current `BrokerEvent::BracketCreated` (in `crates/midas-broker/src/events.rs`)
does not carry TP/SL prices or the reference price. The UI needs these to
verify annotation line positions and for the UUID replacement match.

**Required change** to `crates/midas-broker/src/events.rs`:

```rust
BracketCreated {
    parent_id: Uuid,
    take_profit_id: Option<Uuid>,
    stop_loss_id: Option<Uuid>,
    symbol: String,
    action: OrderAction,
    quantity: f64,
    // New fields:
    tp_price: Option<f64>,
    sl_price: Option<f64>,
    reference_price: Option<f64>,
}
```

This change is in `midas-broker` (root workspace) and is backward-compatible
(new fields are additive). Every place that constructs `BracketCreated` must
be updated to include the three new fields.

## 9. SecurityType Re-export

Add to `crates/midas-broker/src/lib.rs`:

```rust
pub use midas_core::SecurityType;
```

This allows the bridge to use `midas_broker::SecurityType` instead of
reaching into the transitive dependency `midas_core` (which is the ROOT
`midas-core`, not the desktop one). Without this re-export, the bridge
would need to add root `midas-core` as a separate dependency, which is
confusing because the desktop workspace already has its own `midas-core`.

## 10. File Manifest

New and modified files for implementation:

```
NEW:  desktop/win/crates/midas-app/src/broker_bridge.rs
        BrokerBridge struct
        provider::OrderBroker impl (display/status only)
        Direct order methods (create_market_bracket, cancel_bracket, modify_bracket_leg)
        BrokerEventSender newtype (Hash impl for Subscription::run_with)
        broker_order_subscription() using Subscription::run_with
        translate_* functions
        replace_annotation_link_ids()

MOD:  desktop/win/crates/midas-app/Cargo.toml
        Add midas-broker dependency (no feature gating)

MOD:  desktop/win/crates/midas-app/src/main.rs
        mod broker_bridge
        Add broker subscription to subscription()

MOD:  desktop/win/crates/midas-app/src/app.rs
        Add broker_bridge: Option<Arc<BrokerBridge>> field to MidasApp
        Add broker_engine_alive: bool field to MidasApp
        Wire start_broker_engine + BrokerBridge::new in MidasApp::new()
        Rewrite OrderPanelConfirmYes (create annotation locally, send command)
        Add BrokerEventReceived handler (UUID replacement, toasts)
        Add BrokerEngineDied handler
        Wire ChartDragBracketLeg to send ModifyBracketLeg
        Wire BracketContextCancel to send CancelBracket (deferred link removal)
        Add find_order_id_for_leg helper
        Add replace_annotation_link_ids helper

MOD:  desktop/win/crates/midas-core/src/config.rs
        Add BrokerAppConfig struct
        Add broker field to AppConfig

MOD:  crates/midas-broker/src/lib.rs
        Add `pub use midas_core::SecurityType;` re-export

MOD:  crates/midas-broker/src/events.rs
        Add tp_price, sl_price, reference_price to BracketCreated

MOD:  desktop/win/crates/midas-app/src/app/views.rs
        Add broker connection indicator to status bar
        Add red indicator for BrokerEngineDied state
```

## 11. Testing Strategy

### 11.1 Unit Tests (broker_bridge.rs)

- `translate_bracket_params` round-trip: build desktop params, translate,
  verify all fields match.
- `translate_lifecycle_status` for each variant.
- `translate_connection_state` for each variant.
- `translate_order_action` for Buy and Sell.
- `translate_security_type` for each variant.
- `BrokerEventSender` hash consistency (same sender hashes the same).

### 11.2 Integration Test

```rust
#[tokio::test]
async fn broker_bridge_submit_and_receive() {
    let config = BrokerConfig {
        data_source: DataSourceConfig::Test,
        test_broker: TestBrokerConfig {
            auto_connect: true,
            fill_timing: "instant".to_string(),
            ..Default::default()
        },
        ..Default::default()
    };

    let handle = start_broker_engine(config);
    let bridge = BrokerBridge::new(handle, "Test Broker".to_string());
    let mut rx = bridge.subscribe_order_events();

    // Wait for Ready
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(bridge.is_connected());

    // Submit a bracket
    let params = DesktopMarketBracketParams { /* ... */ };
    bridge.create_market_bracket(params).unwrap();

    // Drain events until we see BracketCreated
    let mut saw_created = false;
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        match tokio::time::timeout(
            Duration::from_millis(100),
            rx.recv(),
        ).await {
            Ok(Ok(BrokerEvent::BracketCreated {
                tp_price, sl_price, reference_price, ..
            })) => {
                // Verify the new price fields are populated.
                assert!(reference_price.is_some());
                saw_created = true;
                break;
            }
            _ => continue,
        }
    }
    assert!(saw_created, "expected BracketCreated event");
}
```

### 11.3 Manual Smoke Test

1. Run `cargo run -p midas-app`.
2. Load a chart with AAPL data.
3. Open Order Panel, set BUY 100 shares, TP 192.00, SL 182.00.
4. Click Confirm & Submit.
5. Verify: bracket annotation appears on chart **immediately** (before engine confirms).
6. Verify: status bar shows "Broker: Ready".
7. Verify: toast notification shows fill price.
8. Verify: bracket status transitions to Active (entry filled).
9. Drag TP leg to new price. Verify status_message updates.
10. Right-click bracket, Cancel. Verify annotation link stays until engine confirms.
11. Verify: annotation changes to Cancelled state only after engine event.

## 12. Open Questions

1. **Shared midas-core type unification.** Both workspaces have their own
   `SecurityType`, `OrderAction`, etc. Long-term, these should be unified so
   the translation layer becomes zero-cost. This requires making the desktop
   workspace's `midas-core` depend on the root `midas-core`, or extracting a
   shared `midas-types` crate that both workspaces reference.

2. **OrderAnnotationLink struct.** The link now carries `symbol`, `side`,
   and `quantity` fields for matching against engine events. This struct
   definition needs to be added to the codebase (likely in
   `desktop/win/crates/midas-app/src/app.rs` or a dedicated module).

3. **Multiple pending brackets for same symbol.** The
   `replace_annotation_link_ids` matching by symbol + side + quantity could
   be ambiguous if the user submits two identical brackets before the first
   `BracketCreated` arrives. Mitigation: use FIFO ordering (find the
   *first* matching link). In practice this is unlikely for manual trading.
