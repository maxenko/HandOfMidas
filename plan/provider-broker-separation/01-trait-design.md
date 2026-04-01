# 01 -- Trait Design: DataProvider and OrderBroker

**Parent:** [00-index.md](./00-index.md) | **Phase:** 1 of 6

This document defines every type that will be added to `midas-core` to support pluggable data providers and order brokers. All code in this document lives in a new `midas-core::provider` module.

---

## Table of Contents

1. [DataProvider trait](#1-dataprovider-trait)
2. [OrderBroker trait](#2-orderbroker-trait)
3. [ProviderError enum](#3-providererror-enum)
4. [ProviderInfo and ProviderType](#4-providerinfo-and-providertype)
5. [OrderParams and OrderId](#5-orderparams-and-orderid)
6. [Position](#6-position)
7. [Why two traits instead of one](#7-why-two-traits-instead-of-one)
8. [Compatibility with existing code](#8-compatibility-with-existing-code)
9. [async-trait vs native async](#9-async-trait-vs-native-async)
10. [Module layout](#10-module-layout)
11. [Full source listing](#11-full-source-listing)

---

## 1. DataProvider Trait

The `DataProvider` trait is the central abstraction for all historical candle data sources. It lives in `midas-core` so that any crate in the workspace can implement it without circular dependencies.

### Signature

```rust
/// A source of historical candle data.
///
/// Implementors include:
/// - `TestDataProvider` (synthetic data, instant, in `midas-feed`)
/// - `IbDataProvider` (Interactive Brokers historical bars, future)
/// - `PolygonDataProvider` (Polygon.io REST API, future)
/// - `CachingProvider<P>` (DuckDB read-through cache wrapping any `P`)
///
/// # Object Safety
///
/// This trait is object-safe via `async_trait`. It can be used as
/// `Arc<dyn DataProvider>` for runtime polymorphism.
///
/// # Concurrency
///
/// All methods take `&self`, not `&mut self`. Providers are shared across
/// chart panels via `Arc` and may receive concurrent calls. Implementations
/// that need mutable internal state must use interior mutability
/// (`Mutex`, `RwLock`, `DashMap`, etc.).
///
/// # Examples
///
/// ```no_run
/// use std::sync::Arc;
/// use midas_core::provider::DataProvider;
/// use midas_core::Timeframe;
///
/// async fn load_chart(provider: &dyn DataProvider) {
///     let candles = provider
///         .get_candles("AAPL", Timeframe::D1, 365)
///         .await
///         .expect("failed to load AAPL daily");
///     println!("loaded {} candles", candles.len());
/// }
/// ```
#[async_trait::async_trait]
pub trait DataProvider: Send + Sync {
    /// Human-readable name for display in UI (e.g., "Test Data", "Interactive Brokers").
    ///
    /// Borrowed from the implementor -- no allocation on each call.
    fn name(&self) -> &str;

    /// Whether the provider is currently able to serve data.
    ///
    /// For offline providers like `TestDataProvider`, this always returns `true`.
    /// For network providers, this reflects the connection state.
    fn is_connected(&self) -> bool;

    /// Fetch historical candles for a symbol.
    ///
    /// # Parameters
    ///
    /// - `symbol`: Ticker symbol (e.g., `"AAPL"`, `"ESZ4"`). Provider-specific
    ///   resolution: IB resolves to contract ID internally, Polygon uses the
    ///   ticker directly.
    /// - `timeframe`: Candle period. See [`Timeframe`] for all supported values.
    /// - `days`: Number of calendar days of history to fetch. For daily data,
    ///   `365` returns roughly one year. The provider may return fewer candles
    ///   than the date range implies if less history is available.
    ///
    /// # Returns
    ///
    /// A [`CandleBuffer`] containing the requested candles in chronological
    /// order (oldest first), or a [`ProviderError`] on failure.
    ///
    /// # Cancellation
    ///
    /// Dropping the returned future cancels the request. Network providers
    /// should respect cancellation by using `tokio::select!` internally.
    async fn get_candles(
        &self,
        symbol: &str,
        timeframe: Timeframe,
        days: u32,
    ) -> Result<CandleBuffer, ProviderError>;

    // ── Future extensions (not implemented in this phase) ────────────
    //
    // /// Subscribe to real-time candle updates for a symbol.
    // ///
    // /// Returns a channel receiver that yields `CandleUpdate` messages
    // /// as new bars close or existing bars update.
    // async fn subscribe(
    //     &self,
    //     symbol: &str,
    //     timeframe: Timeframe,
    // ) -> Result<tokio::sync::broadcast::Receiver<CandleUpdate>, ProviderError>;
    //
    // /// Unsubscribe from real-time updates.
    // async fn unsubscribe(&self, symbol: &str) -> Result<(), ProviderError>;
}
```

### Design Rationale

#### Why async

Every non-trivial provider involves I/O:

- **IbDataProvider**: Sends a `reqHistoricalData` message over TCP, waits for bar callbacks over seconds.
- **PolygonDataProvider**: Issues an HTTP GET to `api.polygon.io`, parses JSON response.
- **CachingProvider**: Queries DuckDB via the async `DbHandle::query_candles()` method.

Making the trait async from the start means no refactoring when network providers arrive. `TestDataProvider` wraps its synchronous generation in a `tokio::task::spawn_blocking` (or simply returns immediately since the work is CPU-bound and fast).

#### Why Send + Sync

The provider is stored as `Arc<dyn DataProvider>` inside `MidasApp` and cloned into async tasks spawned for each chart's data load. `Send` is required because the `Arc` crosses a thread boundary when moved into `tokio::spawn`. `Sync` is required because multiple tasks may call `&self` methods concurrently through the same `Arc`.

#### Why &self not &mut self

Multiple chart panels may trigger `get_candles()` concurrently:

1. User switches layout from 1-chart to 4-chart -- all four load simultaneously.
2. User changes the active provider -- all charts reload in parallel.
3. User types a new ticker while another chart is still loading.

With `&mut self`, only one chart could load at a time (or the app would need a `Mutex<Box<dyn DataProvider>>` at the call site, which is worse). `&self` pushes interior mutability into the provider implementation where it belongs.

#### Why in midas-core

`midas-core` is the leaf crate that every other crate depends on. Placing `DataProvider` here means:

- `midas-feed` can implement it for `TestDataProvider`.
- `midas-broker` can implement it for a future `IbDataProvider`.
- A future `midas-polygon` crate can implement it.
- `midas-app` can consume it without knowing the concrete type.

If the trait lived in `midas-feed`, then `midas-broker` would need to depend on `midas-feed` to implement `DataProvider`, which is architecturally wrong (the broker should not depend on the feed crate).

#### Why CandleBuffer return type

`CandleBuffer` (from `midas-data`) is the universal in-memory representation. Charts hold `Option<Arc<CandleBuffer>>`. The `CandleData` trait is implemented by `CandleBuffer`. Returning `CandleBuffer` directly means zero conversion overhead between provider and consumer.

Note: This means `midas-core` must re-export or reference `CandleBuffer` from `midas-data`. Since `midas-core` is the leaf crate, we considered four options:

1. **Option A:** Move `CandleBuffer` into `midas-core`. This would make `midas-data` thinner but `midas-core` heavier.
2. **Option B:** Use a type parameter or associated type on the trait instead of a concrete type.
3. **Option C:** Keep `CandleBuffer` in `midas-data` and have `midas-core` depend on `midas-data` for this one type. But this reverses the dependency direction.
4. **Option D:** Place the `DataProvider` trait in `midas-feed` (which depends on both `midas-core` and `midas-data`).

> **RESOLVED: Option A is the answer.** Move `CandleBuffer` struct definition (just the Vec fields and methods) to `midas-core`. LOD/mmap/binary functionality remains in `midas-data` as extension traits/functions. This lets `DataProvider` trait live in `midas-core/src/provider.rs` with zero circular dependencies. `CachingProvider` lives in `midas-store` (which already depends on `midas-core` and `midas-data`). No `midas-store -> midas-feed` dependency needed.

```
midas-core           (CandleBuffer struct, CandleData trait, Timeframe, IDs,
                      DataProvider trait, OrderBroker trait)
    ^
    |
midas-data           (CandleBuffer extension traits: LOD, mmap, binary format)
    ^
    |
midas-feed           (TestDataProvider, TestProvider: impl DataProvider)
    ^
    |
midas-store          (CachingProvider: impl DataProvider, DbHandle, DuckDB cache)
    ^
    |
midas-app            (ProviderRegistry, MidasApp)
```

---

## 2. OrderBroker Trait

The `OrderBroker` trait abstracts order execution. It is separate from `DataProvider` because:

- Not all data providers can execute orders (Polygon provides data only).
- Not all order brokers provide historical data (a broker might only handle execution).
- The app may have data but no order routing (paper trading mode, replay mode).

The `OrderBroker` trait lives in `midas-core` alongside `DataProvider` for consistency, though it will only be implemented by `midas-broker` in the future.

### Signature

```rust
/// An order execution backend.
///
/// Implementors include:
/// - `IbOrderBroker` (Interactive Brokers order routing, future)
/// - `PaperBroker` (simulated fills for testing, future)
///
/// # Object Safety
///
/// This trait is object-safe via `async_trait`. The app holds
/// `Option<Arc<dyn OrderBroker>>` -- `None` when no broker is configured.
///
/// # Lifecycle
///
/// Unlike `DataProvider`, an `OrderBroker` has a connection lifecycle:
/// connecting, authenticating, maintaining heartbeats, handling disconnects.
/// The `connection_state()` method exposes this to the UI for status display.
///
/// # Not Implemented Yet
///
/// This trait is defined for design validation only. No concrete
/// implementation is built in this plan phase.
#[async_trait::async_trait]
pub trait OrderBroker: Send + Sync {
    /// Human-readable name (e.g., "Interactive Brokers", "Paper Trading").
    fn name(&self) -> &str;

    /// Whether the broker is currently connected.
    fn is_connected(&self) -> bool;

    /// Current connection state. The UI renders this as a status indicator.
    ///
    /// Providers that are always "connected" (like a paper broker) return
    /// `ConnectionState::Ready`.
    fn connection_state(&self) -> ConnectionState;

    /// Place a new order.
    ///
    /// Returns the assigned `OrderId` on success. The order may not be
    /// filled immediately -- status updates arrive via the event channel
    /// (out of scope for this trait; handled by `BrokerEvent` broadcast).
    async fn place_order(&self, params: OrderParams) -> Result<OrderId, ProviderError>;

    /// Cancel an existing order.
    ///
    /// Returns `Ok(())` when the cancellation request has been submitted.
    /// Actual cancellation confirmation arrives asynchronously via events.
    async fn cancel_order(&self, order_id: OrderId) -> Result<(), ProviderError>;

    /// Query current positions.
    ///
    /// Returns a snapshot of all open positions at the time of the call.
    async fn get_positions(&self) -> Result<Vec<Position>, ProviderError>;
}
```

### Connection State Reuse

The `OrderBroker` trait references `ConnectionState` from `midas-broker::connection`. Since `midas-broker` is a separate workspace from the desktop app, we define a compatible `ConnectionState` in `midas-core::provider` that mirrors the existing enum:

```rust
/// Connection lifecycle state for order brokers.
///
/// Mirrors `midas_broker::connection::ConnectionState` for compatibility.
/// When `midas-broker` implements `OrderBroker`, it maps its internal
/// state to this enum.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectionState {
    /// No connection established.
    Disconnected,
    /// Connection attempt in progress.
    Connecting,
    /// TCP connected, API negotiated, not yet fully ready.
    Connected { server_version: i32 },
    /// Fully operational.
    Ready,
    /// Connection lost, automatic reconnection in progress.
    Reconnecting { attempt: u32 },
}

impl ConnectionState {
    /// Whether the broker has at least a TCP connection.
    pub fn is_connected(&self) -> bool {
        matches!(self, Self::Connected { .. } | Self::Ready)
    }

    /// Whether the broker is fully operational.
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready)
    }
}
```

---

## 3. ProviderError Enum

A unified error type for all provider operations. Lives alongside the traits in `midas-core::provider`.

```rust
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("provider not connected")]
    NotConnected,
    #[error("unknown symbol: {symbol}")]
    UnknownSymbol { symbol: String },
    #[error("unsupported timeframe: {timeframe}")]
    UnsupportedTimeframe { timeframe: String },
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("internal error: {0}")]
    Internal(String),
    #[error("cache error: {0}")]
    Store(String),
}
```

### ProviderError Conversions

```rust
impl From<midas_store::StoreError> for ProviderError {
    fn from(e: midas_store::StoreError) -> Self {
        ProviderError::Store(e.to_string())
    }
}
```

This conversion lives in `midas-store` (where `CachingProvider` is defined). Since `ProviderError` is defined in `midas-core` and `StoreError` in `midas-store`, and `midas-store` depends on `midas-core`, the `From` impl can live in `midas-store`. Alternatively, `CachingProvider` can map `StoreError` to `ProviderError::Store` manually at the call site:

```rust
// In CachingProvider::get_candles:
self.store
    .query_candles(key)
    .await
    .map_err(|e| ProviderError::Store(e.to_string()))
```

---

## 4. ProviderInfo and ProviderType

> `ProviderInfo` and `ProviderType` were considered during design but removed.
> Providers identify themselves via `fn name(&self) -> &str` only. No separate
> metadata struct is needed. See Resolved Decisions in [00-index.md](00-index.md).

---

## 5. OrderParams and OrderId

Simplified order types for the `OrderBroker` trait. These are intentionally simpler than `midas-broker`'s `CreateOrderParams` and `LocalOrder`, because the trait interface should be provider-agnostic. Provider-specific details are handled inside the implementation.

```rust
/// Unique identifier for an order within the system.
///
/// Wraps a UUID (v7, time-sortable). This is the same ID format used by
/// `midas-broker`'s `LocalOrder`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OrderId(pub uuid::Uuid);

impl OrderId {
    /// Generate a new time-sortable order ID.
    pub fn new() -> Self {
        Self(uuid::Uuid::now_v7())
    }
}

impl std::fmt::Display for OrderId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Direction of a trade.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderSide {
    /// Buying to open or add to a position.
    Buy,
    /// Selling to close or reduce a position.
    Sell,
}

/// Type of order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderType {
    /// Execute at the current market price.
    Market,
    /// Execute at the specified price or better.
    Limit,
    /// Trigger a market order when the stop price is reached.
    Stop,
    /// Trigger a limit order when the stop price is reached.
    StopLimit,
}

/// Parameters for placing a new order via `OrderBroker::place_order()`.
///
/// This is a simplified, provider-agnostic order specification. The
/// `OrderBroker` implementation maps it to provider-specific structures
/// (e.g., IB's contract + order objects).
///
/// # Examples
///
/// ```
/// use midas_core::provider::{OrderParams, OrderSide, OrderType};
///
/// let params = OrderParams {
///     symbol: "AAPL".to_string(),
///     side: OrderSide::Buy,
///     order_type: OrderType::Limit,
///     quantity: 100.0,
///     limit_price: Some(175.50),
///     stop_price: None,
/// };
/// ```
#[derive(Debug, Clone)]
pub struct OrderParams {
    /// Ticker symbol (e.g., "AAPL").
    pub symbol: String,
    /// Buy or sell.
    pub side: OrderSide,
    /// Market, limit, stop, or stop-limit.
    pub order_type: OrderType,
    /// Number of shares or contracts.
    pub quantity: f64,
    /// Limit price (required for `Limit` and `StopLimit` orders).
    pub limit_price: Option<f64>,
    /// Stop trigger price (required for `Stop` and `StopLimit` orders).
    pub stop_price: Option<f64>,
}

/// A snapshot of one open position.
///
/// Returned by `OrderBroker::get_positions()`.
#[derive(Debug, Clone)]
pub struct Position {
    /// Ticker symbol.
    pub symbol: String,
    /// Signed quantity: positive for long, negative for short.
    pub quantity: f64,
    /// Average cost basis per share/contract.
    pub avg_cost: f64,
    /// Current market value of the position, if available.
    pub market_value: Option<f64>,
    /// Unrealized profit/loss, if available.
    pub unrealized_pnl: Option<f64>,
}
```

---

## 6. Position

Defined above in section 5 alongside `OrderParams`.

---

## 7. Why Two Traits Instead of One

A combined trait would look like:

```rust
// NOT what we are doing:
pub trait Provider: Send + Sync {
    fn name(&self) -> &str;
    async fn get_candles(...) -> Result<CandleBuffer, ProviderError>;
    async fn place_order(...) -> Result<OrderId, ProviderError>;
    async fn cancel_order(...) -> Result<(), ProviderError>;
    async fn get_positions(...) -> Result<Vec<Position>, ProviderError>;
    fn connection_state(&self) -> ConnectionState;
}
```

Problems with this approach:

| Issue | Impact |
|---|---|
| **Polygon cannot place orders** | Every data-only provider must stub `place_order`, `cancel_order`, `get_positions` with `Err(ProviderError::Internal("not supported"))`. This is a code smell (ISP violation). |
| **App cannot express "no order routing"** | The app holds `Arc<dyn Provider>` which always has order methods. There is no way to statically express "orders are not available." With two traits, `Option<Arc<dyn OrderBroker>>` makes this explicit. |
| **Different lifecycles** | Data providers may be stateless (test, polygon with API key). Order brokers maintain persistent connections with heartbeats, reconnection logic, and session management. Combining them forces shared lifecycle management. |
| **Different switching semantics** | Switching data providers reloads charts. Switching order brokers requires draining open orders and confirming with the user. These are fundamentally different operations. |
| **Testing complexity** | A test that only verifies chart data loading should not need to mock order execution methods. Separate traits enable focused mocking. |

The two-trait design follows the Interface Segregation Principle. Each trait has a single, cohesive responsibility.

---

## 8. Compatibility with Existing Code

### CandleBuffer and CandleData are unchanged

The `DataProvider` trait returns `CandleBuffer` (from `midas-data`). `CandleBuffer` implements `CandleData` (from `midas-core`). Charts render via `&dyn CandleData`. This chain is completely unaffected:

```
DataProvider::get_candles() -> CandleBuffer
                                    |
                              impl CandleData
                                    |
                           &dyn CandleData (used by chart)
```

### TestDataProvider's existing API

The current `TestDataProvider::get_candles(&mut self, ticker: &str, tf: Timeframe, days: u32) -> CandleBuffer` is preserved for backward compatibility. The new `DataProvider` impl wraps this with interior mutability:

```rust
#[async_trait::async_trait]
impl DataProvider for TestDataProvider {
    async fn get_candles(
        &self,
        symbol: &str,
        timeframe: Timeframe,
        days: u32,
    ) -> Result<CandleBuffer, ProviderError> {
        // TestDataProvider uses interior Mutex for its cache.
        // Generation is CPU-bound and fast (~1ms for 10 years of daily data).
        Ok(self.get_candles_inner(symbol, timeframe, days))
    }
    // ...
}
```

Details of this adaptation are in [02-test-provider.md](./02-test-provider.md).

### MarketDataSource in midas-broker

The existing `MarketDataSource` trait in `midas-broker` is not modified. It serves a different purpose: dispatching within the broker engine's internal loop (synchronous, takes `con_id`). When `IbDataProvider` is built, it will use `BrokerHandle` (mpsc commands, broadcast events) rather than calling `MarketDataSource` directly. The two traits coexist:

| Trait | Location | Purpose | Sync/Async | Takes |
|---|---|---|---|---|
| `MarketDataSource` | `midas-broker` | Internal broker engine dispatch | Sync (`&mut self`) | `con_id: i32, request_id: u64` |
| `DataProvider` | `midas-core` | App-level data abstraction | Async (`&self`) | `symbol: &str, days: u32` |

### Existing MidasApp field

`MidasApp` currently has `test_data: TestDataProvider`. Phase 5 replaces this with `registry: ProviderRegistry` (which internally holds `Arc<dyn DataProvider>` entries). The `TestDataProvider` is still used -- it is just wrapped in `CachingProvider` and registered as the "test" provider.

---

## 9. async-trait vs Native Async

Rust 1.75 stabilized `async fn` in traits, but with limitations for dynamic dispatch:

| Feature | `async fn` in trait (native) | `async_trait` crate |
|---|---|---|
| `dyn Trait` support | Requires `#[trait_variant::make]` or manual `Box<dyn Future>` | Works out of the box |
| Heap allocation | None (monomorphized) | One `Box::pin()` per call |
| Ergonomics | Cleaner syntax | Proc macro attribute |
| Ecosystem maturity | New, some rough edges | Battle-tested since 2019 |
| iced compatibility | Untested with iced 0.14's Task | Known to work |

We choose `async_trait` because:

1. **Object safety is required.** The app holds `Arc<dyn DataProvider>`, which demands `dyn`-safe traits. Native async in traits does not support `dyn` without extra machinery.
2. **The performance cost is negligible.** One `Box::pin()` allocation per `get_candles()` call is dwarfed by the actual I/O (network roundtrip, DuckDB query, or even TestDataProvider's synthetic generation).
3. **Ecosystem compatibility.** `async_trait` is a known quantity with tokio, iced, and tower. No risk of subtle incompatibilities.
4. **Migration path.** When native async traits gain full `dyn` support, `#[async_trait]` can be removed with a mechanical find-and-replace.

### Dependency Addition

In `midas-core/Cargo.toml`:

```toml
[dependencies]
async-trait = "0.1"
```

The trait definitions live in `midas-core`, so `async-trait` is added there. `midas-feed` also uses `async-trait` for its `impl DataProvider` blocks.

---

## 10. Module Layout

### New file: `desktop/win/crates/midas-core/src/provider.rs`

Contains:
- `DataProvider` trait
- `OrderBroker` trait
- `ProviderError` enum
- `ConnectionState` enum
- `OrderParams` struct
- `OrderId` newtype
- `OrderSide` enum
- `OrderType` enum
- `Position` struct

### Updated file: `desktop/win/crates/midas-core/src/lib.rs`

Add:
```rust
pub mod provider;

pub use provider::{
    ConnectionState, DataProvider, OrderBroker, OrderId, OrderParams,
    OrderSide, OrderType, Position, ProviderError,
};
```

### Unchanged files

- `midas-feed/src/lib.rs` -- no changes to provider module (trait is imported from `midas-core`, not defined here)
- `midas-core/src/candle_data.rs` -- no changes
- `midas-data/src/candle.rs` -- no changes
- `midas-broker/src/market_data.rs` -- no changes

---

## 11. Full Source Listing

The complete `provider.rs` module to be added to `midas-core`:

```rust
//! Provider traits for data acquisition and order execution.
//!
//! This module defines the `DataProvider` and `OrderBroker` traits that
//! abstract over different data sources (test data, IB, Polygon) and
//! order execution backends (IB, paper trading).
//!
//! # Architecture
//!
//! ```text
//! DataProvider (trait)          OrderBroker (trait)
//!       |                            |
//!   TestDataProvider            IbOrderBroker (future)
//!   IbDataProvider (future)     PaperBroker (future)
//!   PolygonDataProvider (future)
//!   CachingProvider<P>
//! ```
//!
//! Both traits are object-safe via `async_trait` and can be used as
//! `Arc<dyn DataProvider>` / `Arc<dyn OrderBroker>`.

use crate::Timeframe;
use crate::CandleBuffer;

// ── ConnectionState ─────────────────────────────────────────────────

/// Connection lifecycle state for order brokers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectionState {
    /// No connection established.
    Disconnected,
    /// Connection attempt in progress.
    Connecting,
    /// TCP connected, API negotiated.
    Connected { server_version: i32 },
    /// Fully operational.
    Ready,
    /// Connection lost, reconnecting.
    Reconnecting { attempt: u32 },
}

impl ConnectionState {
    /// Whether the broker has at least a TCP connection.
    pub fn is_connected(&self) -> bool {
        matches!(self, Self::Connected { .. } | Self::Ready)
    }

    /// Whether the broker is fully operational.
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready)
    }
}

impl std::fmt::Display for ConnectionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Disconnected => f.write_str("Disconnected"),
            Self::Connecting => f.write_str("Connecting"),
            Self::Connected { server_version } => {
                write!(f, "Connected (v{server_version})")
            }
            Self::Ready => f.write_str("Ready"),
            Self::Reconnecting { attempt } => {
                write!(f, "Reconnecting (attempt {attempt})")
            }
        }
    }
}

// ── ProviderError ───────────────────────────────────────────────────

/// Errors from data provider or order broker operations.
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("provider not connected")]
    NotConnected,
    #[error("unknown symbol: {symbol}")]
    UnknownSymbol { symbol: String },
    #[error("unsupported timeframe: {timeframe}")]
    UnsupportedTimeframe { timeframe: String },
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("internal error: {0}")]
    Internal(String),
    #[error("cache error: {0}")]
    Store(String),
}

// ── OrderId ─────────────────────────────────────────────────────────

/// Unique order identifier (UUID v7, time-sortable).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OrderId(pub uuid::Uuid);

impl OrderId {
    /// Generate a new time-sortable order ID.
    pub fn new() -> Self {
        Self(uuid::Uuid::now_v7())
    }
}

impl Default for OrderId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for OrderId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ── OrderSide ───────────────────────────────────────────────────────

/// Direction of a trade.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderSide {
    Buy,
    Sell,
}

impl std::fmt::Display for OrderSide {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Buy => f.write_str("BUY"),
            Self::Sell => f.write_str("SELL"),
        }
    }
}

// ── OrderType ───────────────────────────────────────────────────────

/// Type of order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderType {
    Market,
    Limit,
    Stop,
    StopLimit,
}

impl std::fmt::Display for OrderType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Market => f.write_str("MKT"),
            Self::Limit => f.write_str("LMT"),
            Self::Stop => f.write_str("STP"),
            Self::StopLimit => f.write_str("STP LMT"),
        }
    }
}

// ── OrderParams ─────────────────────────────────────────────────────

/// Provider-agnostic order specification.
#[derive(Debug, Clone)]
pub struct OrderParams {
    /// Ticker symbol (e.g., "AAPL").
    pub symbol: String,
    /// Buy or sell.
    pub side: OrderSide,
    /// Market, limit, stop, or stop-limit.
    pub order_type: OrderType,
    /// Number of shares or contracts.
    pub quantity: f64,
    /// Limit price (required for Limit and StopLimit).
    pub limit_price: Option<f64>,
    /// Stop trigger price (required for Stop and StopLimit).
    pub stop_price: Option<f64>,
}

// ── Position ────────────────────────────────────────────────────────

/// Snapshot of one open position.
#[derive(Debug, Clone)]
pub struct Position {
    /// Ticker symbol.
    pub symbol: String,
    /// Signed quantity (positive = long, negative = short).
    pub quantity: f64,
    /// Average cost per share/contract.
    pub avg_cost: f64,
    /// Current market value, if available.
    pub market_value: Option<f64>,
    /// Unrealized P&L, if available.
    pub unrealized_pnl: Option<f64>,
}

// ── DataProvider trait ──────────────────────────────────────────────

/// A source of historical candle data.
///
/// Implementors:
/// - `TestDataProvider` (synthetic data, always connected)
/// - `CachingProvider<P>` (DuckDB read-through cache)
/// - `IbDataProvider` (future)
/// - `PolygonDataProvider` (future)
///
/// Object-safe via `async_trait`. Used as `Arc<dyn DataProvider>`.
/// All methods take `&self` for concurrent access behind `Arc`.
#[async_trait::async_trait]
pub trait DataProvider: Send + Sync {
    /// Human-readable name for UI display.
    ///
    /// Borrowed from the implementor -- no allocation on each call.
    fn name(&self) -> &str;

    /// Whether the provider can currently serve data.
    fn is_connected(&self) -> bool;

    /// Fetch historical candles for a symbol.
    ///
    /// `days` is the number of calendar days of history to fetch.
    /// The provider may return fewer candles if less history exists.
    async fn get_candles(
        &self,
        symbol: &str,
        timeframe: Timeframe,
        days: u32,
    ) -> Result<CandleBuffer, ProviderError>;
}

// ── OrderBroker trait ───────────────────────────────────────────────

/// An order execution backend.
///
/// Object-safe via `async_trait`. The app holds
/// `Option<Arc<dyn OrderBroker>>` (None when no broker is configured).
///
/// Not implemented in this phase -- trait definition only.
#[async_trait::async_trait]
pub trait OrderBroker: Send + Sync {
    /// Human-readable name.
    fn name(&self) -> &str;

    /// Whether the broker is currently connected.
    fn is_connected(&self) -> bool;

    /// Current connection state.
    fn connection_state(&self) -> ConnectionState;

    /// Place a new order. Returns the assigned OrderId.
    async fn place_order(
        &self,
        params: OrderParams,
    ) -> Result<OrderId, ProviderError>;

    /// Cancel an existing order.
    async fn cancel_order(
        &self,
        order_id: OrderId,
    ) -> Result<(), ProviderError>;

    /// Query current open positions.
    async fn get_positions(&self) -> Result<Vec<Position>, ProviderError>;
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_state_predicates() {
        assert!(!ConnectionState::Disconnected.is_connected());
        assert!(!ConnectionState::Disconnected.is_ready());
        assert!(!ConnectionState::Connecting.is_connected());
        assert!(ConnectionState::Connected { server_version: 176 }.is_connected());
        assert!(!ConnectionState::Connected { server_version: 176 }.is_ready());
        assert!(ConnectionState::Ready.is_connected());
        assert!(ConnectionState::Ready.is_ready());
        assert!(!ConnectionState::Reconnecting { attempt: 1 }.is_connected());
    }

    #[test]
    fn connection_state_display() {
        assert_eq!(ConnectionState::Ready.to_string(), "Ready");
        assert_eq!(
            ConnectionState::Reconnecting { attempt: 3 }.to_string(),
            "Reconnecting (attempt 3)"
        );
    }

    #[test]
    fn provider_error_display() {
        let err = ProviderError::NotConnected;
        assert_eq!(err.to_string(), "provider not connected");

        let err = ProviderError::UnknownSymbol {
            symbol: "XYZZY".into(),
        };
        assert_eq!(err.to_string(), "unknown symbol: XYZZY");

        let err = ProviderError::UnsupportedTimeframe {
            timeframe: "T3".into(),
        };
        assert_eq!(err.to_string(), "unsupported timeframe: T3");
    }

    #[test]
    fn order_id_display() {
        let id = OrderId::new();
        let s = id.to_string();
        // UUID v7 format: 8-4-4-4-12
        assert_eq!(s.len(), 36);
        assert!(s.contains('-'));
    }

    #[test]
    fn order_side_display() {
        assert_eq!(OrderSide::Buy.to_string(), "BUY");
        assert_eq!(OrderSide::Sell.to_string(), "SELL");
    }

    #[test]
    fn order_type_display() {
        assert_eq!(OrderType::Market.to_string(), "MKT");
        assert_eq!(OrderType::Limit.to_string(), "LMT");
        assert_eq!(OrderType::Stop.to_string(), "STP");
        assert_eq!(OrderType::StopLimit.to_string(), "STP LMT");
    }

    #[test]
    fn position_construction() {
        let pos = Position {
            symbol: "AAPL".into(),
            quantity: 100.0,
            avg_cost: 175.50,
            market_value: Some(17_800.0),
            unrealized_pnl: Some(250.0),
        };
        assert_eq!(pos.symbol, "AAPL");
        assert!(pos.quantity > 0.0);
    }

    /// Verify that DataProvider is object-safe by constructing a trait object.
    /// This is a compile-time check -- if it compiles, the trait is object-safe.
    #[allow(dead_code)]
    fn assert_data_provider_object_safe(
        _p: &dyn DataProvider,
    ) {}

    /// Verify that OrderBroker is object-safe.
    #[allow(dead_code)]
    fn assert_order_broker_object_safe(
        _p: &dyn OrderBroker,
    ) {}
}
```

---

## Summary of Changes

| File | Action | What |
|---|---|---|
| `midas-core/Cargo.toml` | Modify | Add `async-trait = "0.1"`, `uuid = { version = "1", features = ["v7"] }` |
| `midas-core/src/provider.rs` | Create | All types and traits from section 11 |
| `midas-core/src/lib.rs` | Modify | Add `pub mod provider;` and re-exports |

No existing files are modified beyond `midas-core/src/lib.rs` gaining one new module declaration and re-exports. All existing tests continue to pass.

---

## Next

[02-test-provider.md](./02-test-provider.md) -- Wrapping `TestDataProvider` behind the `DataProvider` trait.
