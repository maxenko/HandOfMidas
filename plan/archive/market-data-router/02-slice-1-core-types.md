# Slice 1 — Core Types

**Goal.** Land the shared streaming types (`Tick`, `Bar`, `MarketEvent`, `FarmStatus`, `SubscriptionHandle`, etc.) in `midas-broker-core` so both the provider layer and the router layer can reference them without duplication. This slice is pure type work — no behavior change, no wiring.

## Scope

### A. New module `midas-broker-core::market_data`

Add these files to `crates/midas-broker-core/src/market_data/`:

- `mod.rs` — re-exports.
- `tick.rs` — `Tick`, `TickKind`, `TickType`, `TickValue`, `TickAttributes`, `TickByTickKind`, `GenericTicks`.
- `bar.rs` — `Bar`, `BarCompleteness`, `Timeframe` (move from midas-core if currently lives there; keep a `pub use` alias for back-compat).
- `event.rs` — `MarketEvent` enum covering every event kind the router can emit.
- `farm.rs` — `FarmStatus`, `FarmCode` (2104/2106/2108/1100/1101/1102/2103/2105/2158).
- `req_id.rs` — `ReqId(i32)` newtype (BR-8 — IB wire is `i32`), `Copy + Hash + Eq + Display`. Separate `RouterSubId(u64)` for router-internal bookkeeping.
- `error.rs` — `MarketDataError`, `ErrorCode` (10089, 10167, 354, 200, 201, 202, 300, 162, 322, 10147, 321).
- `what_to_show.rs` — `WhatToShow::{Trades, Midpoint, Bid, Ask, ...}` mirroring IB exactly.
- `connection.rs` — `ConnectionState`, `Quote`, `IbDuration` (M-2, M-23).
- `contract.rs` — `ContractDetails`, `SecurityType` (M-34).

### B. Types

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct Tick {
    pub symbol: SymbolKey,
    pub req_id: ReqId,
    pub kind: TickKind,
    pub tick_type: TickType,
    pub value: TickValue,
    pub attrs: TickAttributes,
    pub ts: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickKind { Price, Size, PriceSize, Generic, String, Params }   // M-17

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TickType {
    Bid,        // = 1
    Ask,        // = 2
    Last,       // = 4
    BidSize,    // = 0
    AskSize,    // = 3
    LastSize,   // = 5
    Volume,     // = 8
    High,       // = 6
    Low,        // = 7
    Close,      // = 9
    Open,       // = 14
    HaltedState, // = 49
    // ...full set from IB tick_types.html; exhaustively represented
}

#[derive(Debug, Clone, PartialEq)]
pub enum TickValue {
    Price(f64),
    Size(i64),
    PriceSize { price: f64, size: i64 },   // M-17: atomic pair avoids the "size before price" ordering trap
    Generic(f64),
    Text(String),
    Bool(bool),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TickAttributes {
    pub can_auto_execute: bool,
    pub past_limit: bool,
    pub pre_open: bool,
    // M-22: match rust-ibapi 2.10 attribute set.
    pub unreported: bool,
    pub bid_past_low: bool,
    pub ask_past_high: bool,
}

/// BR-11: tick-by-tick subscription flavour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickByTickKind { Last, AllLast, BidAsk, MidPoint }

/// BR-10: generic tick list passed to reqMktData (e.g. 233 = RT Volume, 293 = Trade Count).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GenericTicks(pub Vec<u32>);
```

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct Bar {
    pub symbol: SymbolKey,
    pub timeframe: Timeframe,
    pub ts_open: DateTime<Utc>,
    pub ts_close: DateTime<Utc>,
    pub o: f64,
    pub h: f64,
    pub l: f64,
    pub c: f64,
    pub volume: u64,
    pub trade_count: u32,
    pub wap: Option<f64>,
    pub completeness: BarCompleteness,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarCompleteness {
    Completed,
    Partial,    // M-36: `ticks_folded` was sim-only; removed from public API.
}
```

```rust
// M-16: single historical shape — Historical(Vec<Bar>) | End | Update(Bar).
#[derive(Debug, Clone, PartialEq)]
pub enum MarketEvent {
    Tick(Tick),
    Bar(Bar),
    FarmStatus(FarmStatus),
    ConnectionState(ConnectionState),
    OrderingReady { next_order_id: i32 },   // M-14
    SubscriptionAccepted { req_id: ReqId, symbol: SymbolKey, kind: StreamKind },
    SubscriptionEnded { req_id: ReqId, reason: EndReason },
    Historical(Vec<Bar>),
    HistoricalDataEnd {
        req_id: ReqId,
        first_ts: DateTime<Utc>,
        last_ts: DateTime<Utc>,
    },
    HistoricalUpdate(Bar),
    Error { req_id: Option<ReqId>, code: ErrorCode, message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamKind { Tick, TickByTick, RealtimeBar, Bar(Timeframe), Historical }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndReason { Cancelled, Disconnected, FarmDropped, Error }
```

### B.1 Connection + Quote (M-23)

```rust
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Quote {
    pub bid: Option<f64>,
    pub ask: Option<f64>,
    pub last: Option<f64>,
    pub ts: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected { server_version: i32 },
    /// Connected AND all farms reported up AND `nextValidId` received.
    Ready,
    Reconnecting { attempt: u32 },
}
```

### B.2 IbDuration (M-2)

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IbDuration {
    Seconds(u32),
    Days(u32),
    Weeks(u32),
    Months(u32),
    Years(u32),
}

impl IbDuration {
    pub fn to_ib_string(&self) -> String {
        match self {
            IbDuration::Seconds(n) => format!("{n} S"),
            IbDuration::Days(n)    => format!("{n} D"),
            IbDuration::Weeks(n)   => format!("{n} W"),
            IbDuration::Months(n)  => format!("{n} M"),
            IbDuration::Years(n)   => format!("{n} Y"),
        }
    }

    /// Round a wall-clock lookback into IB's durational language.
    pub fn from_lookback(d: std::time::Duration) -> Self {
        // map small → Seconds, up to 60 days, else Days, else Weeks, …
        todo!()
    }
}
```

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct FarmStatus {
    pub code: FarmCode,
    pub connected: bool,
    pub detail: String,
}

// M-13, M-14: full farm-code set; `NextValidId` removed (lives on `MarketEvent::OrderingReady`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FarmCode {
    MarketDataFarmOk,            // 2104
    HistoricalDataFarmOk,        // 2106
    MarketDataFarmInactive,      // 2108
    MarketDataFarmBroken,        // 2103
    HistoricalDataFarmBroken,    // 2105
    SecDefFarmOk,                // 2158
    ConnectionLost,              // 1100
    ConnectionRestoredDataLost,  // 1101
    ConnectionRestoredDataKept,  // 1102
}
```

```rust
// BR-8: IB wire uses i32 reqId. Keep it wire-accurate; introduce a separate
// RouterSubId(u64) for router-internal counters that need wider range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct ReqId(pub i32);

impl ReqId {
    pub fn next(counter: &AtomicI32) -> Self {
        // IB reqIds should stay positive; a 2^31 counter is fine for any app lifetime.
        Self(counter.fetch_add(1, Ordering::Relaxed))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct RouterSubId(pub u64);

impl RouterSubId {
    pub fn next(counter: &AtomicU64) -> Self {
        Self(counter.fetch_add(1, Ordering::Relaxed))
    }
}
```

```rust
// M-15: distinguish 354 ("subscribed delayed") from 10167 ("requires add'l subscription").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorCode {
    NoMarketDataPermission,            // 10089
    DelayedMarketDataSubscribed,       // 354
    RequiresAdditionalSubscription,    // 10167
    InvalidReqId,                      // 300
    NoSecurityDefinition,              // 200
    OrderSizeInvalid,                  // 201
    OrderRejected,                     // 202
    PacingViolation,                   // 100 family
    HistoricalDataServiceError,        // 162
    DuplicateTickerId,                 // 322
    OrderCancelNotFound,               // 10147
    Validation,                        // 321
    Other(i32),
}

#[derive(Debug, thiserror::Error)]
pub enum MarketDataError {
    #[error("no market data permission: {symbol}")]
    NoPermission { symbol: String },
    #[error("invalid reqId: {0}")]
    InvalidReqId(ReqId),
    #[error("disconnected from broker")]
    Disconnected,
    #[error("router shutting down")]
    ShuttingDown,
    #[error("pacing violation: {0}")]
    PacingViolation(String),
    #[error("streaming line limit exceeded")]
    StreamingLineLimitExceeded,        // BR-19
    #[error("unsupported timeframe: {0:?}")]
    UnsupportedTimeframe(Timeframe),   // BR-22
    #[error("unsupported on this source")]
    Unsupported,
    #[error("i/o: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Other(String),
}
```

### C. Handles

`SubscriptionHandle<T>` lives in `midas-market-data` (slice 5), NOT in core. But core exposes a trait / struct shape that providers produce.

Providers return their own concrete `TickStream`, `RealtimeBarStream`, `HistoricalStream` types from slice 2. Those are `Stream<Item = MarketEvent>`-flavored — exact shape defined in slice 2.

### C.1 SymbolKey migration (M-32) — split into S1a and S1b

- **S1a**: add the canonical `SymbolKey` in `midas-broker-core`. Keep the existing `midas-app::annotation_store::SymbolKey` working (re-export or alias).
- **S1b**: migrate every desktop import to `midas_broker_core::SymbolKey`. Delete the desktop copy. Done as its own commit, verified by `cargo build --workspace` on both sides.

S1b can land after S1a but before S7; it is prerequisite for any router-facing code in the app.

## Files touched

- `crates/midas-broker-core/src/lib.rs` — add `pub mod market_data;`.
- `crates/midas-broker-core/src/market_data/*.rs` — new files.
- `crates/midas-broker-core/Cargo.toml` — add `thiserror`, `chrono`, `serde` (already present if used elsewhere).

Check if `Timeframe` currently lives in `midas-core` (desktop). If yes:
- Move the canonical definition to `midas-broker-core` (this is shared between root and desktop workspaces? — it's in `desktop/win/crates/midas-core/src/timeframe.rs` per the deep-map). Root workspace `midas-broker-core` may need to add a fresh `Timeframe` copy, OR depend on the desktop-workspace `midas-core` if the dep graph allows.

  **Resolution**: `midas-broker-core` is in the root workspace; `midas-core` is in the desktop workspace. They are independent. Introduce `Timeframe` in `midas-broker-core` as the canonical definition; have `desktop/win/crates/midas-core` re-export from it. If that dependency doesn't exist yet, add it.

  *Investigation required* — slice 1 implementer must confirm the dep direction is legal (root → desktop cannot happen; desktop depends on root via `path = "../../.."` already for `midas-broker-core`?). Check `desktop/win/Cargo.toml` and `Cargo.lock`.

## Tests

Pure type tests. For each type:
- `serde` roundtrip (serialize + deserialize = original).
- `Debug` doesn't panic on edge values.
- `Hash` / `Eq` for keys work as expected.
- `ReqId::next` produces monotonic unique values under concurrent `fetch_add`.
- `RouterSubId::next` ditto.

M-33: **Timeframe-serde legacy fixture test.** Check in `crates/midas-broker-core/tests/fixtures/legacy_config.toml` (and mirror a small `.devloop/fixtures` snippet) containing a serialized `Timeframe` taken BEFORE the move to `midas-broker-core`. After the move, the deserialize-then-reserialize roundtrip must be binary-identical. Guards against serde-shape drift breaking persisted configs and fixtures.

No integration tests yet — nothing consumes these types.

## Acceptance

- `cargo build -p midas-broker-core` passes.
- `cargo test -p midas-broker-core` passes (new + existing).
- `cargo clippy -p midas-broker-core -- -D warnings` clean.
- `cargo fmt --all`.
- No existing test broken in root or desktop workspaces.

## Risks / surprises the implementer should flag

- If `Timeframe` already exists in both crates with slightly different representations (epoch ms vs seconds, serde shape), unifying risks breaking serde-persisted configs. Confirm before moving.
- `TickType` enum needs to be exhaustive enough for IB; IB has ~90 tick types. Start with the top 20 (see list in type definition) and mark `#[non_exhaustive]` so we can extend without breaking API.
- If `SymbolKey` doesn't exist yet in `midas-broker-core`, add it. Currently desktop has one in `midas-app::annotation_store::SymbolKey`. The broker-core version must match behavior (trim + uppercase normalization).
