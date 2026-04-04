# 01 — Architecture

> Core design of the test broker: trait hierarchy, simulation engine,
> channel model, and integration with the existing engine.

---

## Table of Contents

- [1. Trait Hierarchy](#1-trait-hierarchy)
- [2. Simulation Engine](#2-simulation-engine)
- [3. Fill Model](#3-fill-model)
- [4. Channel Integration](#4-channel-integration)
- [5. Configuration](#5-configuration)
- [6. Composition with TestDataProvider](#6-composition-with-testdataprovider)

---

## 1. Trait Hierarchy

### 1.1 Extended BrokerClient Trait

The current `BrokerClient` trait handles order placement only. Extend it to
cover the full IB API surface needed by the engine:

```rust
/// Extended broker client that supports order execution, market data,
/// and account queries. The real IB implementation wraps rust-ibapi;
/// the test broker simulates everything locally.
pub trait BrokerClient: Send + Sync {
    // -- Identity --
    fn name(&self) -> &str;

    // -- Order Management --
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
    ) -> Result<PlaceOrderResult, String>;

    fn cancel_order(&self, ib_order_id: i32) -> Result<CancelOrderResult, String>;

    // -- Status Callbacks (test broker drives these) --
    /// Poll for pending status callbacks. Returns events that should be
    /// emitted by the engine. Called from the engine's event loop.
    fn poll_callbacks(&self) -> Vec<BrokerCallback>;

    // -- Connection --
    fn connect(&self) -> Result<i32, String>;  // Returns server_version
    fn disconnect(&self);
    fn is_connected(&self) -> bool;

    // -- Market Data --
    fn subscribe_market_data(&self, symbol: &str, con_id: i32);
    fn unsubscribe_market_data(&self, symbol: &str);

    // -- Account --
    fn request_positions(&self);
    fn request_account_summary(&self);

    // -- Reconciliation --
    fn request_open_orders(&self) -> Vec<OpenOrderInfo>;
}
```

### 1.2 BrokerCallback Enum

The test broker produces callbacks that the engine translates to `BrokerEvent`s:

```rust
/// Callbacks produced by the broker client (IB status updates, fills, etc.).
/// The engine polls these and translates to BrokerEvents.
pub enum BrokerCallback {
    /// Order status changed at IB.
    OrderStatus {
        ib_order_id: i32,
        status: String,       // IB status string
        filled: f64,
        remaining: f64,
        avg_fill_price: f64,
    },
    /// An execution (fill) occurred.
    Execution {
        ib_order_id: i32,
        exec_id: String,
        shares: f64,
        price: f64,
        commission: f64,
        side: String,
    },
    /// Order was rejected by IB.
    OrderRejected {
        ib_order_id: i32,
        reason: String,
    },
    /// Market data tick.
    Tick {
        symbol: String,
        con_id: i32,
        bid: Option<f64>,
        ask: Option<f64>,
        last: Option<f64>,
        volume: Option<i64>,
    },
    /// Connection state change.
    ConnectionStatus {
        connected: bool,
        server_version: Option<i32>,
    },
    /// Position update.
    Position {
        symbol: String,
        con_id: i32,
        quantity: f64,
        avg_cost: f64,
    },
    /// Account value update.
    AccountValue {
        key: String,
        value: String,
        currency: String,
    },
    /// Streaming bar update (forming bar, updated on each tick).
    BarUpdated { symbol: String, timestamp: i64, open: f64, high: f64, low: f64, close: f64, volume: i64 },
    /// Bar period completed (closed bar, final values).
    BarClosed { symbol: String, timestamp: i64, open: f64, high: f64, low: f64, close: f64, volume: i64 },
}
```

### 1.3 Backward Compatibility

The existing `TestBrokerClient` (accept-only) remains as-is for simple
unit tests that only need order recording. The new `TestBroker` (full
simulation) is a separate struct that implements the extended trait.

```
BrokerClient (trait)
├── TestBrokerClient   — accept-only stub (existing, for unit tests)
├── TestBroker         — full simulation (new, for integration/E2E tests)
└── IbBrokerClient     — real IB connection (future)
```

### 1.4 Alternatives Considered

**Callback delivery: Polling vs. Channel Push**

| Approach | Pros | Cons |
|---|---|---|
| Polling (chosen) | Trait stays sync (no async_trait), simple Mutex-based state, works for future IB client (cross-thread push to queue, engine polls) | 10ms poll latency, wasted cycles when idle |
| Channel push (mpsc) | Event-driven, no wasted polls, idiomatic Tokio | Requires async trait methods or channel handle in trait, complex lifetime management |

**Decision**: Polling. The BrokerClient trait must be object-safe and sync for
the real IB client (which pushes from a separate TCP reader thread). Polling
at 10ms adds negligible overhead for a test tool.

**State management: Multiple Mutexes vs. Single Inner**

Single `Mutex<TestBrokerInner>` chosen to eliminate deadlock risk from nested
locking. Since all access comes from one async task (the engine's poll loop),
contention is zero and a single lock is simpler to reason about.

---

## 2. Simulation Engine

### 2.1 Core Architecture

```
┌─────────────────────────────────────────────────┐
│                  TestBroker                       │
│                                                   │
│  ┌──────────┐  ┌──────────────┐  ┌────────────┐ │
│  │  Order    │  │  Fill        │  │  Market    │ │
│  │  Book     │  │  Engine      │  │  Data Gen  │ │
│  │          │  │              │  │            │ │
│  │ pending  │  │ price model  │  │ tick gen   │ │
│  │ working  │  │ slippage     │  │ bar stream │ │
│  │ filled   │  │ partial fill │  │ from       │ │
│  │ bracket  │  │ timing       │  │ TestData   │ │
│  │ links    │  │              │  │ Provider   │ │
│  └──────────┘  └──────────────┘  └────────────┘ │
│                                                   │
│  ┌──────────────┐  ┌─────────────────────────┐   │
│  │  Account     │  │  Callback Queue         │   │
│  │  Simulator   │  │                         │   │
│  │              │  │  Vec<BrokerCallback>     │   │
│  │  positions   │  │  polled by engine loop   │   │
│  │  cash        │  │                         │   │
│  │  P&L         │  │                         │   │
│  └──────────────┘  └─────────────────────────┘   │
└─────────────────────────────────────────────────┘
```

### 2.2 Internal State

```rust
pub struct TestBroker {
    config: TestBrokerConfig,
    inner: Mutex<TestBrokerInner>,
    next_id: AtomicI32,
    connected: AtomicBool,
}

struct TestBrokerInner {
    orders: HashMap<i32, SimulatedOrder>,
    bracket_links: HashMap<i32, Vec<i32>>,
    market_prices: HashMap<String, f64>,
    callbacks: VecDeque<BrokerCallback>,
    positions: HashMap<String, PositionState>,
    cash: f64,
    subscriptions: HashSet<String>,
    data_provider: TestDataProvider,
}
```

### 2.3 SimulatedOrder

```rust
struct SimulatedOrder {
    ib_order_id: i32,
    symbol: String,
    action: String,        // "BUY" or "SELL"
    order_type: String,    // "MKT", "LMT", "STP", "STP LMT"
    quantity: f64,
    limit_price: Option<f64>,
    stop_price: Option<f64>,
    parent_id: Option<i32>,
    transmit: bool,
    status: SimOrderStatus,
    filled_qty: f64,
    avg_fill_price: f64,
    /// Timestamp when order was placed (for timing simulation).
    placed_at: Instant,
}

enum SimOrderStatus {
    /// Queued but not transmitted (transmit=false, waiting for bracket completion).
    Held,
    /// Transmitted, working at simulated exchange.
    Working,
    /// Stop/conditional order waiting for trigger.
    Triggered,  // maps to IB "PreSubmitted"
    /// Partially filled.
    PartialFill,
    /// Completely filled (terminal).
    Filled,
    /// Cancelled (terminal).
    Cancelled,
    /// Rejected (terminal).
    Rejected,
}
```

---

## 3. Fill Model

### 3.1 Fill Timing Modes

```rust
pub enum FillTiming {
    /// Fills happen immediately on place_order (for unit tests).
    Instant,
    /// Fills happen after a configurable delay (for UI testing).
    Delayed { ms: u64 },
    /// Fills happen when market price crosses the order price
    /// (requires market data ticks to drive).
    PriceTriggered,
}
```

### 3.2 Fill Price Calculation

```rust
/// How fill prices are determined.
pub enum FillPriceModel {
    /// Fill at exactly the limit/stop price (ideal execution).
    Exact,
    /// Fill at market price ± random slippage within bounds.
    Slippage { max_bps: f64 },
    /// Fill at the last known market price (for market orders).
    MarketPrice,
}
```

### 3.3 Market Order Fill Logic

```
place_order(MKT, BUY, 100 AAPL)
  │
  ├─ If FillTiming::Instant:
  │    Push OrderStatus { Submitted } callback
  │    Push Execution { shares=100, price=last_price } callback
  │    Push OrderStatus { Filled, remaining=0 } callback
  │
  ├─ If FillTiming::Delayed:
  │    Push OrderStatus { Submitted } callback
  │    Schedule fill after delay_ms
  │    On tick: push Execution + OrderStatus { Filled }
  │
  └─ If FillTiming::PriceTriggered:
       Push OrderStatus { Submitted } callback
       Fill on next market data tick
```

### 3.4 Limit Order Fill Logic

```
place_order(LMT, BUY, 100 AAPL @ 180.00)
  │
  ├─ If FillTiming::Instant:
  │    Fill immediately at limit price
  │
  ├─ If FillTiming::PriceTriggered:
  │    Push OrderStatus { Submitted } callback
  │    Wait until market price <= 180.00
  │    Then fill at 180.00 (or better)
  │
  └─ If current market price <= 180.00:
       Fill immediately (marketable limit)
```

### 3.5 Stop Order Fill Logic

```
place_order(STP, SELL, 100 AAPL @ 175.00)
  │
  Push OrderStatus { PreSubmitted } callback  (simulated order)
  Wait until market price <= 175.00  (stop triggered)
  │
  Push OrderStatus { Submitted } callback  (now a market order)
  Fill at market price (with optional slippage)
  Push Execution callback
  Push OrderStatus { Filled } callback
```

### 3.6 Partial Fill Support

For quantities > a configurable threshold, split into multiple fills:

```rust
pub struct PartialFillConfig {
    /// Orders above this quantity may be partially filled.
    pub threshold: f64,
    /// Number of fill tranches (2-5 typical).
    pub tranches: u32,
    /// Delay between tranches (ms). 0 = instant.
    pub tranche_delay_ms: u64,
}
```

---

## 4. Channel Integration

### 4.1 Polling Model

The engine's `run()` loop already uses `tokio::select!`. Add a poll interval
for test broker callbacks:

```rust
async fn run(&mut self) {
    let mut heartbeat = tokio::time::interval(Duration::from_secs(30));
    let mut poll_interval = tokio::time::interval(Duration::from_millis(10));

    loop {
        tokio::select! {
            cmd = self.command_rx.recv() => { /* handle command */ }
            _ = heartbeat.tick() => { /* health check */ }
            _ = poll_interval.tick() => {
                // Poll test broker for callbacks
                if let Some(ref client) = self.client {
                    for cb in client.poll_callbacks() {
                        self.handle_broker_callback(cb).await;
                    }
                }
            }
        }
    }
}
```

The poll interval is always active. Since `poll_callbacks()` has a default
implementation returning an empty `Vec`, the cost for non-test clients
(including the future IB client) is a single empty-vec allocation per 10ms
— negligible. No feature flag or name check needed.

### 4.2 Callback → Event Translation

The engine maintains a `ib_to_local: HashMap<i32, Uuid>` mapping, populated
during `submit_bracket_to_ib()` when IB order IDs are assigned. This enables
O(1) lookup from IB callback (which carries ib_order_id) to local order UUID.

```rust
fn handle_broker_callback(&mut self, cb: BrokerCallback) {
    match cb {
        BrokerCallback::OrderStatus { ib_order_id, status, filled, remaining, avg_fill_price } => {
            // Look up local order by ib_order_id (via ib_to_local map)
            // Translate IB status string to OrderStatus
            // Validate transition
            // Update DB
            // Emit BrokerEvent::OrderStatusChanged
            // If bracket member, call check_bracket_status_change()
        }
        BrokerCallback::Execution { ib_order_id, exec_id, shares, price, commission, side } => {
            // Emit BrokerEvent::OrderFilled
            // Update fill tracking (filled_qty, avg_fill_price)
            // Update account positions
        }
        // ... etc
    }
}
```

---

## 5. Configuration

```toml
[test_broker]
# Fill timing: "instant" (default), "delayed", or "price_triggered"
fill_timing = "instant"
# Delay in ms (only for "delayed" mode)
fill_delay_ms = 100

# Fill price model: "exact", "slippage", or "market" (default)
fill_price_model = "market"
# Max slippage in basis points (only for "slippage" model)
max_slippage_bps = 5.0

# Starting cash balance
initial_cash = 100000.0

# Partial fill configuration
partial_fill_threshold = 1000.0
partial_fill_tranches = 3
partial_fill_tranche_delay_ms = 50

# Market data tick interval (ms). 0 = no auto ticks.
tick_interval_ms = 0

# Default bid-ask spread (USD)
default_spread = 0.01

# Auto-connect on engine start
auto_connect = true

# Simulate order rejections (probability 0.0-1.0)
rejection_rate = 0.0

# Probability of cancel-race (fill arrives after cancel request)
cancel_race_probability = 0.0

# Commission per share (USD, IB Pro tiered minimum)
commission_per_share = 0.005
```

The default fill timing is `Instant` -- market orders fill immediately
when `place_order` is called, with no delay or price trigger required.

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct TestBrokerConfig {
    #[serde(default = "default_fill_timing")]
    pub fill_timing: String,          // "instant" (default), "delayed", "price_triggered"
    #[serde(default)]
    pub fill_delay_ms: u64,
    #[serde(default = "default_fill_price_model")]
    pub fill_price_model: String,     // "exact", "slippage", "market" (default)
    #[serde(default)]
    pub max_slippage_bps: f64,
    #[serde(default = "default_initial_cash")]
    pub initial_cash: f64,            // default 100_000.0
    #[serde(default)]
    pub partial_fill_threshold: f64,
    #[serde(default)]
    pub partial_fill_tranches: u32,
    #[serde(default)]
    pub partial_fill_tranche_delay_ms: u64,
    #[serde(default)]
    pub tick_interval_ms: u64,        // 0 = no auto ticks (default)
    #[serde(default = "default_spread")]
    pub default_spread: f64,          // default 0.01
    #[serde(default)]
    pub rejection_rate: f64,          // default 0.0
    #[serde(default)]
    pub cancel_race_probability: f64, // default 0.0
    #[serde(default = "default_auto_connect")]
    pub auto_connect: bool,           // default true
    #[serde(default = "default_commission")]
    pub commission_per_share: f64,    // default 0.005
}
```

---

## 6. Composition with TestDataProvider

The test broker composes with the existing `TestDataProvider` for market
data generation:

```
TestBroker
  │
  ├─ Uses TestDataProvider for:
  │   - Historical bar requests (already works)
  │   - Initial market prices (last close from daily data)
  │   - Tick generation (interpolate between OHLCV bars)
  │
  └─ Generates synthetic ticks by:
      1. Loading the current day's bar from TestDataProvider
      2. Interpolating price between open→high→low→close
      3. Adding random noise within the bar's range
      4. Publishing at configured tick_interval_ms
```

### 6.1 Price Seeding

On first order for a symbol, if no market price is set:

```rust
fn get_or_seed_price(&self, symbol: &str) -> f64 {
    let mut inner = self.inner.lock().unwrap();
    *inner.market_prices.entry(symbol.to_string()).or_insert_with(|| {
        // Use TestDataProvider's last daily close
        let bars = inner.data_provider.daily_bars(symbol);
        bars.last().map(|b| b.close).unwrap_or(100.0)
    })
}
```

This ensures fills happen at realistic prices matching the test data.
