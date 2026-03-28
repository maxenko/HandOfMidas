# Plan 04: Market Data and Event System

> Part of the `midas-broker` crate design. Covers how market data flows from Interactive Brokers through `rust-ibapi` into the iced desktop application, including subscription management, historical data caching, the typed event system, channel architecture, command interface, and iced integration.
>
> References: `provider-ib.md` (IB API reference), `tech-stack-rust-a.md` (charting platform architecture)

---

## Table of Contents

- [1. Market Data Subscriptions](#1-market-data-subscriptions)
- [2. Historical Data](#2-historical-data)
- [3. Event System Design](#3-event-system-design)
- [4. Channel Architecture](#4-channel-architecture)
- [5. Command Interface](#5-command-interface)
- [6. Integration with iced](#6-integration-with-iced)
- [7. Data Flow for a Chart](#7-data-flow-for-a-chart)
- [Appendix A: rust-ibapi Subscription Lifecycle](#appendix-a-rust-ibapi-subscription-lifecycle)
- [Appendix B: IB Limits Quick Reference](#appendix-b-ib-limits-quick-reference)

---

## 1. Market Data Subscriptions

### 1.1 How the Charting App Requests Market Data

The iced application never talks to IB directly. All market data flows through `midas-broker`, which owns the `ibapi::Client` connection and manages subscription lifecycle. The app sends typed `BrokerCommand` messages over an `mpsc` channel; the broker engine processes them on its own Tokio task, translates them into `rust-ibapi` calls, and publishes `BrokerEvent` messages back over a `broadcast` channel that the iced subscription consumes.

```
  iced UI                midas-broker engine              IB Gateway
 ────────               ───────────────────              ──────────
    │                          │                              │
    │─── BrokerCommand ───────>│                              │
    │   (SubscribeMarketData)  │                              │
    │                          │── client.realtime_bars() ──>│
    │                          │── client.market_data() ────>│
    │                          │                              │
    │                          │<── Subscription<Bar> ────── │
    │                          │<── Subscription<Tick> ────── │
    │                          │                              │
    │<── BrokerEvent ─────────│                              │
    │   (Tick / Bar / Depth)   │                              │
```

### 1.2 Real-Time L1 Streaming (Bid/Ask/Last/Volume)

L1 data comes from `rust-ibapi`'s `client.market_data()` builder, which wraps IB's `reqMktData`. This delivers aggregated snapshots at intra-second intervals (~4 updates/sec) covering bid, ask, last, volume, and configurable generic tick types.

```rust
// Inside the broker engine's subscription handler
let subscription = client
    .market_data(&contract)
    .generic_tick_list("233,236") // RT Volume + Shortable
    .streaming()                   // continuous, not snapshot
    .await?;

// Spawn a task that drains the subscription and publishes BrokerEvents
tokio::spawn(async move {
    while let Some(tick) = subscription.next().await {
        let event = match tick {
            TickPrice { field, price, .. } => translate_tick_price(symbol_key, field, price),
            TickSize { field, size, .. }   => translate_tick_size(symbol_key, field, size),
            TickString { .. }              => translate_tick_string(symbol_key, tick),
            _ => continue,
        };
        let _ = event_tx.send(event);
    }
});
```

The broker engine accumulates individual tick fields into a coherent `TickSnapshot` and publishes consolidated `BrokerEvent::Tick` events. IB sends bid, ask, last, and volume as separate messages, so the engine must buffer and coalesce them into a single struct before forwarding to the UI. A coalescing window of 50ms prevents the UI from receiving half-updated snapshots.

```rust
/// Accumulated from multiple IB tick callbacks for a single symbol
pub struct TickSnapshot {
    pub symbol: SymbolKey,
    pub bid: Option<f64>,
    pub bid_size: Option<i64>,
    pub ask: Option<f64>,
    pub ask_size: Option<i64>,
    pub last: Option<f64>,
    pub last_size: Option<i64>,
    pub volume: Option<i64>,
    pub high: Option<f64>,
    pub low: Option<f64>,
    pub close: Option<f64>,
    pub timestamp: DateTime<Utc>,
}
```

### 1.3 Real-Time Bars

Two mechanisms exist for streaming bars:

**5-Second Real-Time Bars (`reqRealTimeBars`)**

Fixed at 5-second intervals. Only bar size available through this endpoint. Useful for the most granular real-time bar display.

```rust
let bar_sub = client.realtime_bars(
    &contract,
    BarSize::Sec5,
    WhatToShow::Trades,
    TradingHours::Regular,
).await?;

tokio::spawn(async move {
    while let Some(bar) = bar_sub.next().await {
        let _ = event_tx.send(BrokerEvent::RealtimeBar {
            symbol: symbol_key,
            bar: bar.into(),
        });
    }
});
```

**keepUpToDate Historical Bars**

For bar sizes other than 5 seconds (1 min, 5 min, 15 min, 1 hour, daily, etc.), use `historical_data_streaming` with `keep_up_to_date = true`. This returns an initial batch of historical bars followed by a live-updating stream that updates the most recent (forming) bar and emits closed bars as they complete.

```rust
let hist_streaming = client.historical_data_streaming(
    &contract,
    Duration::days(1),
    BarSize::Min5,
    Some(WhatToShow::Trades),
    TradingHours::Regular,
    true, // keep_up_to_date
).await?;

tokio::spawn(async move {
    while let Some(update) = hist_streaming.next().await {
        match update {
            HistoricalBarUpdate::NewBar(bar) => {
                let _ = event_tx.send(BrokerEvent::BarClosed { symbol: symbol_key, bar: bar.into() });
            }
            HistoricalBarUpdate::UpdatedBar(bar) => {
                let _ = event_tx.send(BrokerEvent::BarUpdated { symbol: symbol_key, bar: bar.into() });
            }
        }
    }
});
```

**Which to use when:**

| Scenario | Mechanism | Notes |
|---|---|---|
| Chart displaying 5-second bars | `realtime_bars()` | Only option for 5s granularity |
| Chart displaying 1m / 5m / higher | `historical_data_streaming(keep_up_to_date=true)` | Gets backfill + live in one call |
| Custom tick aggregation (S1, etc.) | `tick_by_tick_all_last()` + local `TickAggregator` | Most flexible, highest data volume |

### 1.4 Tick-by-Tick Data

For strategies or chart modes that need every individual trade or quote change, use tick-by-tick subscriptions. These bypass IB's aggregation and deliver raw events.

```rust
// Every trade (including outside RTH)
let trade_sub = client.tick_by_tick_all_last(&contract, 0, false).await?;

// Every bid/ask change
let ba_sub = client.tick_by_tick_bid_ask(&contract, 0, false).await?;

// Midpoint changes
let mid_sub = client.tick_by_tick_midpoint(&contract, 0, false).await?;
```

**Limits (critical):**
- Maximum 5 simultaneous tick-by-tick subscriptions for US securities
- 1 request per instrument per 15 seconds
- Not available for options in real-time

The broker engine tracks active tick-by-tick subscriptions and rejects requests that would exceed the limit, returning a `BrokerEvent::Error` with a descriptive message.

### 1.5 L2 Depth of Book

Market depth provides multiple price levels of the order book. Requires L2 data subscriptions at IB (NASDAQ TotalView, NYSE ArcaBook, etc.).

```rust
let depth_sub = client.market_depth(
    &contract,
    10,    // number of rows per side
    true,  // is_smart_depth (aggregate across exchanges)
).await?;

tokio::spawn(async move {
    while let Some(depth) = depth_sub.next().await {
        match depth {
            MarketDepths::MarketDepth(update) => {
                let _ = event_tx.send(BrokerEvent::DepthUpdate {
                    symbol: symbol_key,
                    update: update.into(),
                });
            }
            MarketDepths::MarketDepthL2(update) => {
                let _ = event_tx.send(BrokerEvent::DepthL2Update {
                    symbol: symbol_key,
                    update: update.into(),
                });
            }
        }
    }
});
```

**Limits:**
- Minimum 3, maximum 60 simultaneous depth subscriptions
- Consumes streaming lines from the same pool as L1

### 1.6 Subscription Management

#### SubscriptionManager

The broker engine maintains a `SubscriptionManager` that tracks all active subscriptions, provides reference counting for shared symbols, and enforces IB limits.

```rust
pub struct SubscriptionManager {
    /// Active L1 streaming subscriptions, keyed by (contract_id, tick_types)
    l1_subs: HashMap<SubscriptionKey, ManagedSubscription>,

    /// Active real-time bar subscriptions
    bar_subs: HashMap<SubscriptionKey, ManagedSubscription>,

    /// Active tick-by-tick subscriptions
    tick_subs: HashMap<SubscriptionKey, ManagedSubscription>,

    /// Active depth subscriptions
    depth_subs: HashMap<SubscriptionKey, ManagedSubscription>,

    /// Maps consumer IDs to their subscriptions for cleanup
    consumer_subs: HashMap<ConsumerId, Vec<SubscriptionKey>>,

    /// Current line usage
    lines_used: u32,
    lines_limit: u32,
}

struct ManagedSubscription {
    /// The IB request ID for cancellation
    request_id: i32,

    /// Reference count: how many consumers (charts, strategies) use this
    ref_count: u32,

    /// Handle to the tokio task draining the rust-ibapi Subscription
    drain_task: JoinHandle<()>,

    /// When this subscription was created
    created_at: Instant,
}

#[derive(Clone, Hash, Eq, PartialEq)]
pub struct SubscriptionKey {
    pub contract_id: i32,
    pub data_type: SubscriptionDataType,
}

#[derive(Clone, Hash, Eq, PartialEq)]
pub enum SubscriptionDataType {
    L1 { generic_ticks: String },
    RealtimeBars { bar_size: BarSize, what_to_show: WhatToShow },
    HistoricalStreaming { bar_size: BarSize, what_to_show: WhatToShow },
    TickByTick { tick_type: TickByTickType },
    Depth { num_rows: i32, smart_depth: bool },
}
```

#### Subscribe Flow

1. UI sends `BrokerCommand::SubscribeMarketData { consumer_id, contract, data_type }`
2. Engine computes `SubscriptionKey` from contract + data type
3. If key already exists in `l1_subs` (or appropriate map):
   - Increment `ref_count`
   - Register `consumer_id` in `consumer_subs`
   - No new IB request needed (data already flowing)
4. If key does not exist:
   - Check `lines_used < lines_limit`. If at limit, return `BrokerEvent::Error` with code and suggestion to close unused charts
   - Call the appropriate `rust-ibapi` method
   - Spawn a drain task that reads from the `Subscription` and publishes `BrokerEvent`s
   - Store `ManagedSubscription` with `ref_count = 1`
   - Register `consumer_id`

#### Unsubscribe Flow

1. UI sends `BrokerCommand::UnsubscribeMarketData { consumer_id, subscription_key }`
2. Engine decrements `ref_count` for the key
3. Removes `consumer_id` from `consumer_subs`
4. If `ref_count` reaches 0:
   - Abort the drain task (the `rust-ibapi` `Subscription` cancels on drop)
   - Remove from the subscription map
   - Decrement `lines_used`

#### Consumer Cleanup

When a chart closes or a strategy stops, the engine receives `BrokerCommand::UnsubscribeAll { consumer_id }` and iterates all subscriptions for that consumer, decrementing ref counts and cleaning up as needed.

### 1.7 Market Data Line Limits

IB defaults to 100 concurrent streaming lines, calculated monthly as:

```
Lines = MAX(commissions/8, (equity * 100) / 1,000,000, 100)
```

Quote Booster Packs add 100 L1 lines + 1 L2 symbol for $30/month each (max 10 packs).

The `SubscriptionManager` must:

1. **Track line usage** -- each L1 or bar subscription consumes 1 line. Depth consumes lines from the same pool.
2. **Query the limit on connect** -- IB does not expose the line limit programmatically, so it must be configured in `midas-broker` settings (default: 100).
3. **Pre-flight check** -- before calling any `rust-ibapi` subscription method, verify `lines_used + cost <= lines_limit`.
4. **Graceful degradation** -- if the user opens more charts than the line limit allows, prioritize visible/active charts. Charts scrolled off-screen or minimized can have their subscriptions paused (cancelled at IB, data_type remembered for re-subscribe).
5. **LRU eviction** -- when at the limit and a new subscription is requested, offer to cancel the least-recently-interacted subscription, or return an error and let the user decide.

```rust
impl SubscriptionManager {
    pub fn can_subscribe(&self, cost: u32) -> bool {
        self.lines_used + cost <= self.lines_limit
    }

    pub fn available_lines(&self) -> u32 {
        self.lines_limit.saturating_sub(self.lines_used)
    }

    /// Returns subscription keys ordered by last interaction time, oldest first
    pub fn eviction_candidates(&self) -> Vec<SubscriptionKey> {
        // Sort all subscriptions by last_accessed ascending
        // Filter to those not pinned by the user
        todo!()
    }
}
```

---

## 2. Historical Data

### 2.1 Requesting Historical Bars

Historical data requests go through `rust-ibapi`'s `client.historical_data()`. The broker engine wraps this in a caching layer so the iced app never has to think about IB pacing rules or cache management.

```rust
// rust-ibapi call (inside broker engine)
let hist = client.historical_data(
    &contract,
    Some(end_date),           // None = now
    Duration::days(30),       // lookback
    BarSize::Min5,            // bar size
    Some(WhatToShow::Trades), // data type
    TradingHours::Regular,    // RTH only
).await?;

// hist.bars is Vec<Bar> with { date, open, high, low, close, volume, wap, count }
```

**Key parameters:**

| Parameter | Purpose | Constraints |
|---|---|---|
| `end_date` | End of requested range | `None` = current time |
| `duration` | How far back from end_date | Max depends on bar size (see below) |
| `bar_size` | Granularity | Smallest: 1 sec. Largest: 1 month |
| `what_to_show` | Data type | TRADES, MIDPOINT, BID, ASK, etc. |
| `trading_hours` | Filter | Regular = RTH only, All = include extended |

**Max duration by bar size (IB limits):**

| Bar Size | Max Duration |
|---|---|
| 1 sec - 5 sec | 1 day |
| 10 sec - 30 sec | 1 week |
| 1 min | 2 weeks |
| 2 min - 5 min | 1 month |
| 15 min - 30 min | 2 months |
| 1 hour | 6 months |
| 1 day | 1 year |
| 1 week - 1 month | 5+ years |

For longer ranges, the engine must issue multiple requests with sliding windows and concatenate the results.

### 2.2 Local Caching Strategy

All historical bar data is cached locally in the binary file format defined in `tech-stack-rust-a.md` (see Phase 2 / Appendix B of that document). The cache is the first layer checked before any IB request.

**Cache directory structure:**

```
data/
  candles/
    AAPL/
      1m.candles        # Binary format: header + packed BinaryCandle array
      5m.candles
      15m.candles
      1h.candles
      1d.candles
    SPY/
      ...
  cache_meta/
    AAPL/
      1m.meta.json      # { "last_bar_ts": 1711324800, "last_fetched": "2026-03-24T..." }
      5m.meta.json
      ...
```

**Cache lookup flow:**

```rust
pub struct HistoricalDataManager {
    data_dir: PathBuf,
    cache_meta: HashMap<(String, Timeframe), CacheMeta>,
    rate_limiter: HistoricalRateLimiter,
}

#[derive(Serialize, Deserialize)]
struct CacheMeta {
    /// Timestamp of the last bar in the cache file
    last_bar_ts: i64,
    /// When we last fetched from IB
    last_fetched: DateTime<Utc>,
    /// Total bar count in the file
    bar_count: u64,
}

impl HistoricalDataManager {
    /// Primary entry point for the UI. Returns bars for the requested range.
    pub async fn get_bars(
        &mut self,
        client: &ibapi::Client,
        contract: &Contract,
        symbol: &str,
        timeframe: Timeframe,
        start_ts: i64,
        end_ts: i64,
    ) -> Result<CandleBuffer> {
        // 1. Load whatever we have from cache
        let cached = self.load_from_cache(symbol, timeframe)?;

        // 2. Determine what's missing
        let cache_end = self.cache_meta
            .get(&(symbol.to_string(), timeframe))
            .map(|m| m.last_bar_ts)
            .unwrap_or(0);

        if cache_end >= end_ts {
            // Cache fully covers the request
            return Ok(cached.slice_by_time(start_ts, end_ts));
        }

        // 3. Fetch only the missing portion from IB
        let fetch_start = if cache_end > 0 { cache_end } else { start_ts };
        let new_bars = self.fetch_from_ib(client, contract, timeframe, fetch_start, end_ts).await?;

        // 4. Append to cache, update meta
        self.append_to_cache(symbol, timeframe, &new_bars)?;

        // 5. Return the combined result
        let combined = cached.concat(&new_bars);
        Ok(combined.slice_by_time(start_ts, end_ts))
    }
}
```

### 2.3 Cache Invalidation

TTL-based invalidation per bar size, since smaller bars are more likely to receive corrections and larger bars are essentially immutable once closed.

| Bar Size | Cache TTL | Rationale |
|---|---|---|
| 1 sec - 30 sec | 1 hour | Sub-minute bars may be adjusted; rarely needed historically |
| 1 min - 5 min | 4 hours | Intraday corrections possible within the trading day |
| 15 min - 1 hour | 24 hours | Stable after close |
| 1 day | 7 days | Adjusted-close corporate actions may update |
| 1 week - 1 month | 30 days | Very stable |

**Invalidation check:**

```rust
impl HistoricalDataManager {
    fn is_cache_stale(&self, symbol: &str, timeframe: Timeframe) -> bool {
        let meta = match self.cache_meta.get(&(symbol.to_string(), timeframe)) {
            Some(m) => m,
            None => return true, // No cache = stale
        };

        let ttl = match timeframe {
            Timeframe::S1 | Timeframe::S5 | Timeframe::S15 | Timeframe::S30 =>  // sub-minute
                std::time::Duration::from_secs(3600),
            Timeframe::M1 | Timeframe::M5 =>
                std::time::Duration::from_secs(4 * 3600),
            Timeframe::M15 | Timeframe::M30 | Timeframe::H1 =>
                std::time::Duration::from_secs(24 * 3600),
            Timeframe::H4 | Timeframe::D1 =>
                std::time::Duration::from_secs(7 * 24 * 3600),
            Timeframe::W1 | Timeframe::MN1 =>
                std::time::Duration::from_secs(30 * 24 * 3600),
        };

        meta.last_fetched.elapsed() > ttl
    }
}
```

The engine also force-invalidates cache for the current trading session's bars on every reconnect (IB error codes 1100/1101), since data may have been missed during disconnection.

### 2.4 Pacing Rule Compliance

IB enforces strict pacing rules on historical data requests. Violating them results in pacing error messages and temporary blocking. The broker engine must implement a rate limiter that guarantees compliance.

**IB Historical Pacing Rules:**

| Rule | Limit |
|---|---|
| Identical request | Must wait 15 seconds before repeating |
| Same contract/exchange/type | Max 6 requests in 2 seconds |
| Total historical requests | Max 60 in any 10-minute window |
| BID_ASK requests | Count as 2 each |

```rust
pub struct HistoricalRateLimiter {
    /// Sliding window of request timestamps (last 10 minutes)
    request_log: VecDeque<Instant>,

    /// Per-contract burst tracking (last 2 seconds)
    contract_bursts: HashMap<i32, VecDeque<Instant>>,

    /// Dedup tracker: hash of (contract_id, bar_size, what_to_show, end_date) -> last_requested
    recent_requests: HashMap<u64, Instant>,

    /// Maximum requests per 10-minute window
    window_limit: u32,           // Default: 55 (safety margin below IB's 60)

    /// Maximum requests per contract in 2-second burst
    burst_limit: u32,            // Default: 5 (safety margin below IB's 6)
}

impl HistoricalRateLimiter {
    /// Returns Ok(()) immediately if the request can proceed,
    /// or Err(Duration) indicating how long to wait.
    pub fn check(&mut self, contract_id: i32, request_hash: u64, is_bid_ask: bool)
        -> Result<(), Duration>
    {
        let now = Instant::now();
        let cost = if is_bid_ask { 2 } else { 1 };

        // 1. Check identical request (15s dedup)
        if let Some(last) = self.recent_requests.get(&request_hash) {
            let elapsed = now.duration_since(*last);
            if elapsed < Duration::from_secs(15) {
                return Err(Duration::from_secs(15) - elapsed);
            }
        }

        // 2. Check per-contract burst (6 in 2s)
        let burst = self.contract_bursts.entry(contract_id).or_default();
        burst.retain(|t| now.duration_since(*t) < Duration::from_secs(2));
        if burst.len() as u32 >= self.burst_limit {
            let oldest = burst.front().unwrap();
            return Err(Duration::from_secs(2) - now.duration_since(*oldest));
        }

        // 3. Check 10-minute window
        self.request_log.retain(|t| now.duration_since(*t) < Duration::from_secs(600));
        let current_count: u32 = self.request_log.len() as u32; // Simplified; real impl counts BID_ASK as 2
        if current_count + cost > self.window_limit {
            let oldest = self.request_log.front().unwrap();
            return Err(Duration::from_secs(600) - now.duration_since(*oldest));
        }

        Ok(())
    }

    /// Record that a request was sent. Call after check() returns Ok.
    pub fn record(&mut self, contract_id: i32, request_hash: u64) {
        let now = Instant::now();
        self.request_log.push_back(now);
        self.contract_bursts.entry(contract_id).or_default().push_back(now);
        self.recent_requests.insert(request_hash, now);
    }
}
```

**Request queuing:** When `check()` returns `Err(wait_duration)`, the engine enqueues the request into a priority queue ordered by deadline. A background task polls the queue and dispatches requests as pacing windows open. High-priority requests (user-initiated chart loads) sort before low-priority ones (background prefetch).

### 2.5 Incremental Updates

After initial cache population, subsequent requests fetch only the delta (new bars since the cache's last bar timestamp). This minimizes IB API usage and avoids re-downloading data the user already has.

```rust
impl HistoricalDataManager {
    /// Fetch only bars newer than what we have cached
    async fn incremental_update(
        &mut self,
        client: &ibapi::Client,
        contract: &Contract,
        symbol: &str,
        timeframe: Timeframe,
    ) -> Result<CandleBuffer> {
        let meta = self.cache_meta.get(&(symbol.to_string(), timeframe));
        let last_ts = meta.map(|m| m.last_bar_ts).unwrap_or(0);

        if last_ts == 0 {
            // No cache at all -- do a full fetch
            return self.full_fetch(client, contract, symbol, timeframe).await;
        }

        // Request from last cached bar to now
        // Overlap by 1 bar period to catch updates to the last bar
        let overlap = timeframe.as_secs() as i64;
        let fetch_start = last_ts - overlap;

        let new_bars = self.fetch_from_ib(
            client, contract, timeframe, fetch_start, now_epoch()
        ).await?;

        // Deduplicate: remove bars from new_bars whose timestamps already exist
        // in the cache (the overlap bar), then append
        let deduped = new_bars.remove_before(last_ts + 1);
        self.append_to_cache(symbol, timeframe, &deduped)?;

        Ok(deduped)
    }
}
```

For the currently displayed chart, `keepUpToDate` historical streaming handles incremental updates automatically. The explicit incremental update is used when:
- The user opens a chart for a symbol that has stale cache
- The app starts up and needs to bring cached data current
- Background prefetch tasks refresh data for watchlist symbols

---

## 3. Event System Design

### 3.1 BrokerEvent Enum

All events flowing from the broker engine to consumers (UI, logger, strategy engine) are represented as a single typed enum. Each variant carries only the data needed to process it; the UI decides what to display, the logger decides what to record, etc.

```rust
use chrono::{DateTime, Utc};

/// Unique identifier for a symbol within the broker session.
/// Wraps the IB contract ID for efficient comparison.
#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub struct SymbolKey {
    pub contract_id: i32,
    pub symbol: String,
}

/// Every event the broker engine can emit.
#[derive(Clone, Debug)]
pub enum BrokerEvent {
    // ── Connection ──────────────────────────────────────────────────

    /// Successfully connected to IB Gateway/TWS
    Connected {
        server_version: i32,
        connection_time: DateTime<Utc>,
    },

    /// Disconnected (intentional or network failure)
    Disconnected {
        reason: DisconnectReason,
    },

    /// Attempting to reconnect (with attempt number and next retry delay)
    Reconnecting {
        attempt: u32,
        next_retry_in: std::time::Duration,
    },

    /// Connection fully restored after a reconnect; subscriptions re-established
    Reconnected,

    // ── Orders ──────────────────────────────────────────────────────

    /// Order accepted by IB and assigned a permanent ID
    OrderSubmitted {
        local_id: LocalOrderId,
        ib_order_id: i32,
        ib_perm_id: i64,
    },

    /// Order status changed (submitted -> filled, etc.)
    OrderStatusChanged {
        local_id: LocalOrderId,
        old_status: IbRawOrderStatus,
        new_status: IbRawOrderStatus,
        filled_qty: f64,
        remaining_qty: f64,
        avg_fill_price: f64,
    },

    /// A fill (execution) occurred on an order
    OrderFilled {
        local_id: LocalOrderId,
        fill: Fill,
    },

    /// Order rejected by IB or exchange
    OrderRejected {
        local_id: LocalOrderId,
        reason: String,
    },

    /// Order cancelled (by user or by IB)
    OrderCancelled {
        local_id: LocalOrderId,
        reason: String,
    },

    // ── Market Data: L1 ─────────────────────────────────────────────

    /// Consolidated L1 tick snapshot (coalesced from multiple IB tick messages)
    Tick {
        symbol: SymbolKey,
        snapshot: TickSnapshot,
    },

    // ── Market Data: Bars ───────────────────────────────────────────

    /// A real-time 5-second bar
    RealtimeBar {
        symbol: SymbolKey,
        bar: OhlcvBar,
    },

    /// A completed (closed) bar from keepUpToDate streaming
    BarClosed {
        symbol: SymbolKey,
        bar: OhlcvBar,
    },

    /// The currently forming bar was updated (new tick within the bar period)
    BarUpdated {
        symbol: SymbolKey,
        bar: OhlcvBar,
    },

    // ── Market Data: Tick-by-Tick ───────────────────────────────────

    /// Individual trade event
    TradeEvent {
        symbol: SymbolKey,
        price: f64,
        size: i64,
        timestamp: DateTime<Utc>,
        exchange: String,
    },

    /// Individual bid/ask update
    BidAskEvent {
        symbol: SymbolKey,
        bid: f64,
        ask: f64,
        bid_size: i64,
        ask_size: i64,
        timestamp: DateTime<Utc>,
    },

    // ── Market Data: Depth ──────────────────────────────────────────

    /// Depth of book update (L2)
    DepthUpdate {
        symbol: SymbolKey,
        side: DepthSide,
        position: i32,
        operation: DepthOperation,
        price: f64,
        size: i64,
    },

    /// L2 depth update with exchange attribution
    DepthL2Update {
        symbol: SymbolKey,
        side: DepthSide,
        position: i32,
        operation: DepthOperation,
        price: f64,
        size: i64,
        market_maker: String,
        is_smart_depth: bool,
    },

    // ── Historical Data ─────────────────────────────────────────────

    /// Historical bars loaded (response to RequestHistoricalData command)
    HistoricalDataReady {
        request_id: RequestId,
        symbol: SymbolKey,
        timeframe: Timeframe,
        bars: Arc<CandleBuffer>,
    },

    /// Historical data request failed
    HistoricalDataError {
        request_id: RequestId,
        symbol: SymbolKey,
        error: String,
    },

    // ── Account ─────────────────────────────────────────────────────

    /// Position update (new, changed, or closed)
    PositionUpdate {
        account: String,
        symbol: SymbolKey,
        position: f64,
        avg_cost: f64,
    },

    /// Account value changed
    AccountUpdate {
        account: String,
        key: String,
        value: String,
        currency: String,
    },

    /// P&L update (account-level)
    PnLUpdate {
        account: String,
        daily_pnl: f64,
        unrealized_pnl: f64,
        realized_pnl: f64,
    },

    /// P&L update for a single position
    PnLSingleUpdate {
        account: String,
        contract_id: i32,
        daily_pnl: f64,
        unrealized_pnl: f64,
        realized_pnl: f64,
        position: f64,
        value: f64,
    },

    // ── Subscription Management ─────────────────────────────────────

    /// Subscription successfully created
    Subscribed {
        consumer_id: ConsumerId,
        subscription_key: SubscriptionKey,
    },

    /// Subscription removed
    Unsubscribed {
        consumer_id: ConsumerId,
        subscription_key: SubscriptionKey,
    },

    /// Subscription failed (e.g., line limit reached)
    SubscriptionError {
        consumer_id: ConsumerId,
        reason: String,
    },

    // ── System ──────────────────────────────────────────────────────

    /// IB error/warning (error codes with orderId = -1 are system-level)
    IbError {
        code: i32,
        message: String,
        /// If associated with a specific request
        request_id: Option<i32>,
    },

    /// Data farm connection status (2104, 2106, 2108, etc.)
    DataFarmStatus {
        farm: String,
        status: FarmStatus,
    },

    /// Broker engine internal error (not from IB)
    InternalError {
        context: String,
        error: String,
    },
}
```

### 3.2 Supporting Types

```rust
#[derive(Clone, Debug)]
pub enum DisconnectReason {
    UserRequested,
    NetworkError(String),
    GatewayRestart,       // Daily ~11:45 PM ET restart
    AuthExpired,          // Sunday re-auth required
}

/// Raw IB order status strings as received from TWS/Gateway callbacks.
/// This is an internal type — map to the canonical `OrderState` (see 02-order-management.md §1.4)
/// at the engine boundary via `OrderState::from_ib_status()`.
#[derive(Clone, Debug, PartialEq)]
pub enum IbRawOrderStatus {
    ApiPending,
    PendingSubmit,
    PreSubmitted,
    Submitted,
    Filled,
    PendingCancel,
    Cancelled,
    Inactive,
    ApiCancelled,
    Unknown(String),
}

#[derive(Clone, Debug)]
pub struct Fill {
    pub execution_id: String,
    pub price: f64,
    pub quantity: f64,
    pub commission: f64,
    pub timestamp: DateTime<Utc>,
    pub exchange: String,
    pub side: String,
    pub cumulative_qty: f64,
    pub avg_price: f64,
}

#[derive(Clone, Debug)]
pub struct OhlcvBar {
    pub timestamp: i64,     // Epoch seconds
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: i64,
    pub wap: f64,           // Weighted average price
    pub count: i32,         // Number of trades
}

#[derive(Clone, Debug)]
pub enum DepthSide { Bid, Ask }

#[derive(Clone, Debug)]
pub enum DepthOperation { Insert, Update, Delete }

#[derive(Clone, Debug)]
pub enum FarmStatus {
    Ok,                    // 2104, 2106
    Inactive,              // 2108
    ConnectionLost,        // 1100
    RestoredDataLost,      // 1101 -- must re-subscribe
    RestoredDataMaintained, // 1102
}

/// Opaque ID assigned by the UI to track which chart/widget owns a subscription
#[derive(Clone, Copy, Debug, Hash, Eq, PartialEq)]
pub struct ConsumerId(pub u64);

/// Opaque ID for correlating historical data requests with responses
#[derive(Clone, Copy, Debug, Hash, Eq, PartialEq)]
pub struct RequestId(pub u64);

/// Application-local order ID (not the IB order ID).
/// Wraps a UUIDv7 to match the `local_id: Uuid` used in 03-data-layer.md and 02-order-management.md.
#[derive(Clone, Copy, Debug, Hash, Eq, PartialEq)]
pub struct LocalOrderId(pub uuid::Uuid);
```

### 3.3 Design Rationale

**Single enum vs. trait objects:** A single `BrokerEvent` enum is chosen over `Box<dyn Event>` because:
- Pattern matching with exhaustiveness checking catches unhandled variants at compile time
- No heap allocation per event (the enum is `Clone` and stack-friendly for small variants; `Arc<CandleBuffer>` in `HistoricalDataReady` handles the large case)
- `tokio::broadcast` requires `Clone`, which trait objects make awkward

**`SymbolKey` vs. string:** Using `contract_id: i32` as the primary key with the string symbol as a display label. Contract IDs are stable, unique identifiers assigned by IB and never change for the life of the instrument. String comparison is avoided on the hot path.

**`Arc<CandleBuffer>` for historical data:** Historical bar responses can be large (tens of thousands of bars). Wrapping in `Arc` avoids cloning the buffer when broadcasting to multiple consumers.

---

## 4. Channel Architecture

### 4.1 Overview

Three channel types serve three different communication patterns:

```
                    ┌─────────────────────────────────┐
                    │       midas-broker engine        │
                    │                                  │
  BrokerCommand     │   ┌──────────┐  ┌────────────┐  │  BrokerEvent
  ──── mpsc ───────>│   │ Command  │  │   Event    │  │──── broadcast ────> iced subscription
                    │   │ Processor│  │  Publisher  │  │──── broadcast ────> logger
                    │   └──────────┘  └────────────┘  │──── broadcast ────> strategy engine
                    │                                  │
                    │   ┌──────────────────────────┐   │  ConnectionStatus
                    │   │  Connection State Machine │   │──── watch ────────> status bar widget
                    │   └──────────────────────────┘   │──── watch ────────> reconnect guard
                    │                                  │
                    └─────────────────────────────────┘
```

### 4.2 Split Channel Design — Lossy vs. Lossless

Events are split into two channels based on whether they can safely be dropped:

| Channel | Type | Events | Drop behavior |
|---|---|---|---|
| **Market data** | `tokio::broadcast` (4096) | Tick, Bar, Depth, HistoricalData | Lossy — stale ticks are worthless |
| **Order/Account** | `tokio::broadcast` (8192) | OrderStatusChanged, OrderFilled, OrderRejected, PositionUpdate, AccountUpdate | Multiple consumers (UI, logger, strategy). Lag triggers full state re-sync. |

**Why split?** A single broadcast channel risks dropping order events when market data bursts fill the buffer. Order fills and status changes are financially significant. Market data ticks arrive at ~2000/sec and are inherently replaceable — missing a tick is fine because the next one arrives milliseconds later.

Order events use broadcast with a large buffer (8192) to support multiple consumers (UI, logger, strategy). Unlike market data, lag detection on this channel triggers a full state re-sync from SQLite via a `RequestOrderSnapshot` command. Order events should practically never lag — they are infrequent compared to market data, and the broadcast buffer of 8192 provides massive headroom.

```rust
use tokio::sync::broadcast;

// Created once when the broker engine starts
let (market_tx, _) = broadcast::channel::<BrokerEvent>(4096);   // lossy, high throughput
let (order_tx, _) = broadcast::channel::<BrokerEvent>(8192);    // multi-consumer, lag triggers re-sync

// Market data: multiple consumers via subscribe()
let chart_rx = market_tx.subscribe();
let watchlist_rx = market_tx.subscribe();

// Order events: multiple consumers via subscribe()
let ui_order_rx = order_tx.subscribe();
let logger_order_rx = order_tx.subscribe();
let strategy_order_rx = order_tx.subscribe();
```

**Market data channel (broadcast, 4096 buffer):**
- ~100 tick updates/sec across 20 symbols = ~2000 ticks/sec
- At 60 fps, the UI drains ~33 events per frame
- 4096 provides ~2 seconds of buffer at peak, sufficient for brief UI stalls
- Slow receivers get `Lagged` and skip to current data — this is correct for ticks

```rust
// Market data consumer (e.g., inside iced subscription)
loop {
    match market_rx.recv().await {
        Ok(event) => handle_market_event(event),
        Err(broadcast::error::RecvError::Lagged(n)) => {
            tracing::warn!("Market consumer lagged, dropped {n} events -- skipping to current");
        }
        Err(broadcast::error::RecvError::Closed) => break,
    }
}
```

### 4.3 `tokio::mpsc` -- Commands (UI -> Engine)

Commands flow from the iced UI to the broker engine. Only one consumer (the engine) reads commands, but multiple producers may exist (multiple UI components send commands).

```rust
use tokio::sync::mpsc;

// Created once when the broker engine starts
let (command_tx, command_rx) = mpsc::channel::<BrokerCommand>(256);

// The UI holds command_tx (cloneable) and sends commands
// The engine holds command_rx and processes them sequentially
```

**Buffer size: 256 commands.** Commands are infrequent relative to events (user actions, not tick data). 256 is more than sufficient. If the buffer fills (engine is unresponsive), `send()` will await, which provides natural backpressure -- the UI will block on the send, which is acceptable because it means the engine is overloaded and should not accept more work.

For fire-and-forget commands where blocking the UI is unacceptable, use `try_send()`:

```rust
// Non-blocking send -- if engine is overwhelmed, log and drop
if command_tx.try_send(command).is_err() {
    tracing::error!("broker command channel full -- engine may be unresponsive");
}
```

### 4.4 `tokio::watch` -- Connection Status (Single Writer, Multiple Readers)

Connection status is a single value that multiple consumers need to observe (status bar, reconnect guard, pre-flight checks before sending commands). `watch` is ideal: it always holds the latest value, readers never lag, and it is extremely cheap.

```rust
use tokio::sync::watch;

#[derive(Clone, Debug, PartialEq)]
pub enum ConnectionStatus {
    Disconnected,
    Connecting,
    Connected {
        server_version: i32,
        data_farms_ok: bool,
    },
    Reconnecting {
        attempt: u32,
    },
    Error(String),
}

// Created when broker engine is constructed
let (status_tx, status_rx) = watch::channel(ConnectionStatus::Disconnected);

// Engine updates status
status_tx.send(ConnectionStatus::Connected { ... });

// UI reads current status (non-blocking)
let current = status_rx.borrow().clone();

// UI watches for changes (async)
loop {
    status_rx.changed().await?;
    let new_status = status_rx.borrow().clone();
    update_status_bar(new_status);
}
```

### 4.5 Backpressure Summary

| Channel | Type | Buffer | When Full | Rationale |
|---|---|---|---|---|
| Market Events | `broadcast` | 4096 | Drop oldest per lagging receiver | Stale market data is useless |
| Order Events | `broadcast` | 8192 | Drop oldest per lagging receiver; consumer sends `RequestOrderSnapshot` for full re-sync | Order events are infrequent; 8192 practically never lags |
| Commands | `mpsc` | 256 | Await (block sender) | Backpressure: slow down UI if engine overloaded |
| Status | `watch` | 1 (latest only) | Always overwrite | Only current status matters |

### 4.6 Thread/Task Layout

```
┌─────────── Tokio Runtime ──────────────────────────────────┐
│                                                             │
│  Task: engine_main_loop                                     │
│    - Receives BrokerCommands from mpsc                      │
│    - Dispatches to appropriate handler                      │
│    - Owns the ibapi::Client                                 │
│    - Manages SubscriptionManager                            │
│    - Manages HistoricalDataManager                          │
│                                                             │
│  Task: l1_drain_{symbol} (one per L1 subscription)          │
│    - Reads from ibapi Subscription<TickPrice/TickSize/...>  │
│    - Coalesces ticks into TickSnapshot                      │
│    - Publishes BrokerEvent::Tick to broadcast               │
│                                                             │
│  Task: bar_drain_{symbol} (one per bar subscription)        │
│    - Reads from ibapi Subscription<Bar>                     │
│    - Publishes BrokerEvent::RealtimeBar to broadcast        │
│                                                             │
│  Task: depth_drain_{symbol} (one per depth subscription)    │
│    - Reads from ibapi Subscription<MarketDepths>            │
│    - Publishes BrokerEvent::DepthUpdate to broadcast        │
│                                                             │
│  Task: historical_request_queue                             │
│    - Processes queued historical data requests               │
│    - Respects rate limiter pacing                            │
│    - Publishes BrokerEvent::HistoricalDataReady             │
│                                                             │
│  Task: reconnect_monitor                                    │
│    - Watches ibapi::Client::is_connected()                  │
│    - On disconnect: updates watch channel, starts backoff   │
│    - On reconnect: re-subscribes all managed subscriptions  │
│                                                             │
│  Task: account_monitor                                      │
│    - Drains position, PnL, account update subscriptions     │
│    - Publishes BrokerEvent::PositionUpdate etc.             │
│                                                             │
└─────────── iced main thread ───────────────────────────────┘
│  iced event loop (owns the window, GPU, widget tree)        │
│  - iced::Subscription polls broadcast receiver              │
│  - Converts BrokerEvent into iced Message variants          │
│  - update() applies state changes                           │
│  - view() redraws affected widgets                          │
└─────────────────────────────────────────────────────────────┘
```

---

## 5. Command Interface

### 5.1 BrokerCommand Enum

All instructions from the UI to the broker engine are expressed as `BrokerCommand` variants. The engine processes these sequentially from the `mpsc` receiver.

```rust
#[derive(Clone, Debug)]
pub enum BrokerCommand {
    // ── Connection ──────────────────────────────────────────────

    /// Connect to IB Gateway/TWS
    Connect {
        host: String,
        port: u16,
        client_id: i32,
    },

    /// Gracefully disconnect
    Disconnect,

    // ── Orders ──────────────────────────────────────────────────

    /// Submit a new order. The engine assigns an IB order ID and returns
    /// OrderSubmitted event with the mapping.
    PlaceOrder {
        local_id: LocalOrderId,
        contract: Contract,
        order: Order,
    },

    /// Modify an existing order (price, quantity, TIF).
    /// The engine looks up the IB order ID from local_id.
    ModifyOrder {
        local_id: LocalOrderId,
        new_limit_price: Option<f64>,
        new_aux_price: Option<f64>,
        new_quantity: Option<f64>,
        new_tif: Option<String>,
    },

    /// Cancel a specific order
    CancelOrder {
        local_id: LocalOrderId,
    },

    /// Activate a previously deactivated order.
    /// Implementation: re-place the cached order parameters.
    ActivateOrder {
        local_id: LocalOrderId,
    },

    /// Deactivate (park) a live order without losing its parameters.
    /// Implementation: cancel at IB, cache locally for re-activation.
    DeactivateOrder {
        local_id: LocalOrderId,
    },

    /// Cancel all open orders (reqGlobalCancel)
    CancelAllOrders,

    // ── Market Data Subscriptions ───────────────────────────────

    /// Subscribe to L1 streaming data for a contract
    SubscribeL1 {
        consumer_id: ConsumerId,
        contract: Contract,
        generic_ticks: Option<String>,
    },

    /// Subscribe to real-time 5-second bars
    SubscribeRealtimeBars {
        consumer_id: ConsumerId,
        contract: Contract,
        what_to_show: WhatToShow,
    },

    /// Subscribe to keepUpToDate historical streaming bars
    SubscribeHistoricalStreaming {
        consumer_id: ConsumerId,
        contract: Contract,
        bar_size: BarSize,
        what_to_show: WhatToShow,
        duration: Duration,
    },

    /// Subscribe to tick-by-tick data
    SubscribeTickByTick {
        consumer_id: ConsumerId,
        contract: Contract,
        tick_type: TickByTickType,
    },

    /// Subscribe to L2 depth of book
    SubscribeDepth {
        consumer_id: ConsumerId,
        contract: Contract,
        num_rows: i32,
        smart_depth: bool,
    },

    /// Unsubscribe a specific subscription for a consumer
    Unsubscribe {
        consumer_id: ConsumerId,
        subscription_key: SubscriptionKey,
    },

    /// Unsubscribe all subscriptions for a consumer (e.g., chart closing)
    UnsubscribeAll {
        consumer_id: ConsumerId,
    },

    // ── Historical Data ─────────────────────────────────────────

    /// Request historical bars. Response arrives as HistoricalDataReady event.
    /// The engine checks cache first, then fetches from IB if needed.
    RequestHistoricalData {
        request_id: RequestId,
        contract: Contract,
        timeframe: Timeframe,
        start_ts: i64,
        end_ts: i64,
    },

    // ── Account ─────────────────────────────────────────────────

    /// Start streaming positions
    SubscribePositions,

    /// Stop streaming positions
    UnsubscribePositions,

    /// Start streaming account summary
    SubscribeAccountSummary {
        tags: Vec<String>,
    },

    /// Stop streaming account summary
    UnsubscribeAccountSummary,

    /// Start streaming P&L for an account
    SubscribePnL {
        account: String,
    },

    /// Stop streaming P&L
    UnsubscribePnL,

    // ── Contract Lookup ─────────────────────────────────────────

    /// Search for contracts matching a pattern (for symbol search bar)
    SearchContracts {
        request_id: RequestId,
        pattern: String,
    },

    /// Qualify a contract (resolve ambiguities, get conId)
    QualifyContract {
        request_id: RequestId,
        contract: Contract,
    },

    // ── Configuration ───────────────────────────────────────────

    /// Update the streaming line limit (user changed it in settings)
    SetLineLimit {
        limit: u32,
    },

    /// Switch market data type (live, frozen, delayed, delayed-frozen)
    SetMarketDataType {
        data_type: MarketDataType,
    },

    // ── Re-sync ─────────────────────────────────────────────────

    /// Request a full snapshot of current order/account state.
    /// Sent when the order events broadcast channel returns `Lagged`,
    /// triggering the engine to re-emit current state for all orders
    /// (loaded from SQLite + in-memory state).
    RequestOrderSnapshot,
}

#[derive(Clone, Debug)]
pub enum TickByTickType {
    Last,
    AllLast,
    BidAsk,
    MidPoint,
}
```

### 5.2 Command Processing

The engine's main loop processes commands one at a time, maintaining a consistent view of subscription state:

```rust
impl BrokerEngine {
    async fn run(mut self) {
        loop {
            tokio::select! {
                Some(cmd) = self.command_rx.recv() => {
                    if let Err(e) = self.handle_command(cmd).await {
                        let _ = self.event_tx.send(BrokerEvent::InternalError {
                            context: "command processing".into(),
                            error: e.to_string(),
                        });
                    }
                }
                // Also select on other internal events (reconnect signals, etc.)
                _ = self.shutdown_signal.recv() => {
                    self.shutdown().await;
                    break;
                }
            }
        }
    }

    async fn handle_command(&mut self, cmd: BrokerCommand) -> Result<()> {
        match cmd {
            BrokerCommand::Connect { host, port, client_id } => {
                self.connect(&host, port, client_id).await
            }
            BrokerCommand::SubscribeL1 { consumer_id, contract, generic_ticks } => {
                self.subscribe_l1(consumer_id, contract, generic_ticks).await
            }
            BrokerCommand::PlaceOrder { local_id, contract, order } => {
                self.place_order(local_id, contract, order).await
            }
            // ... remaining arms
        }
    }
}
```

### 5.3 Command Validation

The engine validates commands before executing them:

- **Connection required:** All commands except `Connect` check `is_connected()`. If not connected, they return `BrokerEvent::IbError` immediately.
- **Contract qualification:** Commands with a `Contract` field check if the contract has a `con_id`. If not, the engine auto-qualifies it via `client.contract_details()` before proceeding.
- **Line limit check:** Subscription commands call `subscription_manager.can_subscribe()` before issuing the IB request.
- **Order existence:** `ModifyOrder`, `CancelOrder`, `ActivateOrder`, `DeactivateOrder` verify the `local_id` exists in the order tracker.

---

## 6. Integration with iced

### 6.1 iced's Subscription Mechanism

iced provides `iced::Subscription` for bridging async streams into the synchronous `update()` cycle. The app declares subscriptions in `fn subscription(&self) -> Subscription<Message>`, and iced manages the async task lifecycle automatically -- starting the stream when the subscription appears and cancelling it when it disappears.

We use `iced::subscription::channel()` to create a long-lived async task that reads from the broker's `broadcast` channel and yields iced `Message` variants.

```rust
use iced::subscription::{self, Subscription};
use tokio::sync::broadcast;

pub fn broker_subscription(
    event_rx: broadcast::Receiver<BrokerEvent>,
) -> Subscription<Message> {
    subscription::channel(
        std::any::TypeId::of::<BrokerSubscriptionMarker>(),
        100, // iced's internal buffer for this subscription
        |mut output| async move {
            let mut rx = event_rx;
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        let messages = broker_event_to_messages(event);
                        for msg in messages {
                            let _ = output.send(msg).await;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("iced broker subscription lagged {n} events");
                        // Optionally send a Message indicating data gap
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        // Engine shut down -- break out
                        break;
                    }
                }
            }
            // Keep the future alive (iced requires it)
            std::future::pending().await
        },
    )
}

struct BrokerSubscriptionMarker;
```

**Dual-subscription pattern:** Because market data and order events use separate broadcast channels, the iced app must create TWO subscriptions — one for each channel. This is done via `Subscription::batch` in the `Application::subscription()` method:

```rust
// In the iced Application::subscription() method:
fn subscription(&self) -> Subscription<Message> {
    Subscription::batch([
        // Market data — lossy, high throughput
        subscription::channel(Id::unique(), 100, |mut output| async move {
            let mut market_rx = broker_handle.market_events.resubscribe();
            loop {
                match market_rx.recv().await {
                    Ok(event) => { let _ = output.send(Message::MarketEvent(event)).await; }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("Market consumer lagged {n} events");
                    }
                    Err(_) => break,
                }
            }
        }),
        // Order events — must never lag; trigger re-sync if it does
        subscription::channel(Id::unique(), 100, |mut output| async move {
            let mut order_rx = broker_handle.order_events.resubscribe();
            loop {
                match order_rx.recv().await {
                    Ok(event) => { let _ = output.send(Message::OrderEvent(event)).await; }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::error!("Order consumer lagged {n} — requesting full state re-sync");
                        let _ = broker_handle.commands.send(BrokerCommand::RequestOrderSnapshot).await;
                    }
                    Err(_) => break,
                }
            }
        }),
    ])
}
```

Note that `resubscribe()` is used rather than holding onto the original receiver. This is critical: if iced recreates the subscription (e.g., after a view change), `resubscribe()` creates a fresh receiver positioned at the current tail of the broadcast buffer, avoiding the single-consumer fragility of `mpsc::Receiver`.

### 6.2 Converting BrokerEvent into iced Messages

The iced app's `Message` enum includes broker-related variants. The conversion function translates `BrokerEvent` into one or more `Message` values.

```rust
/// Broker-related messages for the iced app
pub enum Message {
    // ... existing chart/UI messages from tech-stack-rust-a.md ...

    // Broker events
    BrokerConnected { server_version: i32 },
    BrokerDisconnected { reason: DisconnectReason },
    BrokerReconnecting { attempt: u32 },

    // Market data
    TickReceived { symbol: SymbolKey, snapshot: TickSnapshot },
    RealtimeBarReceived { symbol: SymbolKey, bar: OhlcvBar },
    BarClosed { symbol: SymbolKey, bar: OhlcvBar },
    BarUpdated { symbol: SymbolKey, bar: OhlcvBar },
    DepthUpdated { symbol: SymbolKey, side: DepthSide, position: i32, price: f64, size: i64 },

    // Historical
    HistoricalDataLoaded { request_id: RequestId, symbol: SymbolKey, bars: Arc<CandleBuffer> },
    HistoricalDataFailed { request_id: RequestId, error: String },

    // Orders
    OrderStatusUpdate { local_id: LocalOrderId, status: IbRawOrderStatus },
    OrderFillReceived { local_id: LocalOrderId, fill: Fill },
    OrderError { local_id: LocalOrderId, reason: String },

    // Account
    PositionChanged { symbol: SymbolKey, position: f64, avg_cost: f64 },
    AccountValueChanged { key: String, value: String },
    PnLChanged { daily: f64, unrealized: f64, realized: f64 },

    // System
    BrokerError { code: i32, message: String },
    ConnectionStatusChanged { status: ConnectionStatus },
    SubscriptionConfirmed { consumer_id: ConsumerId },
    SubscriptionFailed { consumer_id: ConsumerId, reason: String },
}

fn broker_event_to_messages(event: BrokerEvent) -> Vec<Message> {
    match event {
        BrokerEvent::Connected { server_version, .. } => {
            vec![Message::BrokerConnected { server_version }]
        }
        BrokerEvent::Tick { symbol, snapshot } => {
            vec![Message::TickReceived { symbol, snapshot }]
        }
        BrokerEvent::BarClosed { symbol, bar } => {
            vec![Message::BarClosed { symbol, bar }]
        }
        BrokerEvent::OrderStatusChanged { local_id, new_status, .. } => {
            vec![Message::OrderStatusUpdate { local_id, status: new_status }]
        }
        BrokerEvent::OrderFilled { local_id, fill } => {
            vec![
                Message::OrderFillReceived { local_id, fill: fill.clone() },
                // Also update the status
                Message::OrderStatusUpdate {
                    local_id,
                    status: if fill.cumulative_qty >= fill.quantity {
                        IbRawOrderStatus::Filled
                    } else {
                        IbRawOrderStatus::Submitted
                    },
                },
            ]
        }
        // ... remaining conversions
        _ => vec![], // Unhandled events are silently dropped at the UI layer
    }
}
```

### 6.3 How Market Data Ticks Drive Chart Redraws

When `Message::TickReceived` or `Message::BarUpdated` arrives in `update()`, the app must decide which charts need redraws. Not every tick should trigger a full GPU render pass.

**State update path:**

```rust
impl MidasApp {
    fn update(&mut self, message: Message) -> Command<Message> {
        match message {
            Message::TickReceived { symbol, snapshot } => {
                // Update the shared tick state for this symbol
                self.tick_state.insert(symbol.clone(), snapshot.clone());

                // Mark charts displaying this symbol as dirty
                for chart in &mut self.charts {
                    if chart.symbol_key == symbol {
                        chart.mark_tick_dirty();
                    }
                }

                // Do NOT request_redraw() here -- let the batching tick handle it
                Command::none()
            }

            Message::BarClosed { symbol, bar } => {
                // Append the closed bar to the chart's CandleBuffer
                for chart in &mut self.charts {
                    if chart.symbol_key == symbol {
                        chart.append_bar(bar.clone());
                        chart.mark_render_dirty(); // This one needs a real redraw
                    }
                }
                Command::none()
            }

            Message::BarUpdated { symbol, bar } => {
                // Update the forming (last) bar in the chart's CandleBuffer
                for chart in &mut self.charts {
                    if chart.symbol_key == symbol {
                        chart.update_forming_bar(bar.clone());
                        chart.mark_render_dirty();
                    }
                }
                Command::none()
            }

            // ...
        }
    }
}
```

### 6.4 Debouncing and Batching Ticks for UI Performance

IB can deliver ~4 tick updates per second per symbol. With 20 symbols streaming, that is ~80 tick messages per second. Triggering a chart redraw on every tick would waste GPU cycles since most ticks only change the price label overlay, not the candlestick geometry.

**Strategy: Two-tier dirty flags**

```rust
pub struct ChartPanel {
    // ...

    /// Tick data changed (bid/ask/last) -- only needs price label redraw
    tick_dirty: bool,

    /// Bar data changed (new candle, forming candle updated) -- needs full GPU render
    render_dirty: bool,

    /// Accumulated tick count since last render (for adaptive batching)
    pending_tick_count: u32,
}
```

**The animation tick drives rendering, not individual events:**

The iced app already has a `Message::Tick` variant that fires at 60 fps (from `iced::time::every(Duration::from_millis(16))`). During each animation tick, the app checks dirty flags and only redraws charts that have changes.

```rust
Message::AnimationTick => {
    for chart in &mut self.charts {
        if chart.render_dirty {
            chart.rebuild_gpu_data(); // Regenerate CandleInstance buffer
            chart.render_dirty = false;
            chart.tick_dirty = false; // Subsumed by full render
        } else if chart.tick_dirty {
            chart.update_price_overlay_only(); // Just the text labels
            chart.tick_dirty = false;
        }
    }
    Command::none()
}
```

This means:
- At most 60 redraws per second regardless of tick rate
- Multiple ticks within a 16ms frame are coalesced into a single state update
- Full candle geometry rebuild only happens when bar data changes (every 5 seconds at most for 5s bars, every minute for 1m bars, etc.)
- Price label overlays update at display refresh rate for smooth visual feedback

**Adaptive reduction for many charts:** When more than 8 charts are active, reduce the animation tick rate to 30 fps for charts not in the focused/hovered state. Only the active chart gets 60 fps updates.

### 6.5 Connection Status in iced

The `watch` channel for `ConnectionStatus` is consumed via a separate iced subscription:

```rust
pub fn connection_status_subscription(
    status_rx: watch::Receiver<ConnectionStatus>,
) -> Subscription<Message> {
    subscription::channel(
        std::any::TypeId::of::<ConnectionStatusMarker>(),
        10,
        |mut output| async move {
            let mut rx = status_rx;
            loop {
                if rx.changed().await.is_err() {
                    break; // Sender dropped
                }
                let status = rx.borrow().clone();
                let _ = output.send(Message::ConnectionStatusChanged { status }).await;
            }
            std::future::pending().await
        },
    )
}
```

The status bar widget reads `ConnectionStatus` from app state and displays an indicator: green dot for connected, yellow for reconnecting, red for disconnected.

---

## 7. Data Flow for a Chart

### 7.1 Complete Flow: User Opens a Chart for AAPL

This section traces the exact sequence of operations from the moment a user types "AAPL" in the symbol search bar to real-time data flowing into the chart.

#### Step 1: User Input

The user types "AAPL" in the toolbar search box and hits Enter.

```
User → iced toolbar widget → Message::SymbolSearchSubmit("AAPL")
```

#### Step 2: Contract Resolution

The app's `update()` handler sends a command to qualify the contract:

```rust
// In MidasApp::update()
Message::SymbolSearchSubmit(symbol) => {
    let request_id = self.next_request_id();
    let contract = Contract::stock(&symbol).build();
    self.send_command(BrokerCommand::QualifyContract {
        request_id,
        contract,
    });
    // Store pending state: request_id -> active_chart_id
    self.pending_qualifications.insert(request_id, self.active_chart_id);
}
```

The broker engine calls `client.contract_details(&contract)` and returns the resolved contract (with `con_id`, primary exchange, etc.) via a `BrokerEvent`.

#### Step 3: Historical Data Request

Once the contract is qualified, the app requests historical bars for the chart's timeframe:

```rust
Message::ContractQualified { request_id, contract } => {
    let chart_id = self.pending_qualifications.remove(&request_id).unwrap();
    let chart = &mut self.charts[chart_id];
    chart.set_contract(contract.clone());

    let hist_request_id = self.next_request_id();
    self.send_command(BrokerCommand::RequestHistoricalData {
        request_id: hist_request_id,
        contract: contract.clone(),
        timeframe: chart.timeframe,
        start_ts: chart.visible_start_ts(),
        end_ts: now_epoch(),
    });
    self.pending_hist_loads.insert(hist_request_id, chart_id);
}
```

#### Step 4: Cache Check and IB Fetch

Inside the broker engine, `HistoricalDataManager` handles the request:

1. Check if `data/candles/AAPL/5m.candles` exists and covers the requested range
2. If cache is fresh (within TTL) and covers the range: return cached data immediately as `BrokerEvent::HistoricalDataReady`
3. If cache is stale or missing the tail: fetch the delta from IB via `client.historical_data()`, respecting pacing rules
4. Append new bars to the cache file
5. Return combined result as `BrokerEvent::HistoricalDataReady`

```
Engine: cache has AAPL 5m bars up to 2026-03-24 09:00
Engine: requesting 2026-03-24 09:00 to now from IB
Engine: rate limiter check → OK, 12/55 requests used in window
Engine: client.historical_data(AAPL, ...) → 48 new bars
Engine: append to cache, update meta
Engine: broadcast HistoricalDataReady { bars: Arc<CandleBuffer> }
```

#### Step 5: Chart Renders Historical Data

```rust
Message::HistoricalDataLoaded { request_id, symbol, bars } => {
    let chart_id = self.pending_hist_loads.remove(&request_id).unwrap();
    let chart = &mut self.charts[chart_id];
    chart.set_candle_buffer(bars);
    chart.auto_scale_y_axis();
    chart.mark_render_dirty(); // Will trigger GPU rebuild on next animation tick
}
```

The chart widget's `prepare()` method (iced `Shader` trait) converts `CandleBuffer` into `CandleInstance` arrays, uploads to GPU, and renders.

#### Step 6: Subscribe to Streaming Data

After historical data is loaded, the app subscribes to live updates:

```rust
// Immediately after setting the candle buffer
let consumer_id = ConsumerId(chart_id as u64);
self.send_command(BrokerCommand::SubscribeHistoricalStreaming {
    consumer_id,
    contract: chart.contract.clone(),
    bar_size: chart.timeframe.to_ib_bar_size(),
    what_to_show: WhatToShow::Trades,
    duration: Duration::days(1),
});

// Also subscribe to L1 for the price overlay
self.send_command(BrokerCommand::SubscribeL1 {
    consumer_id,
    contract: chart.contract.clone(),
    generic_ticks: Some("233".into()),
});
```

#### Step 7: Real-Time Data Flows

The complete data path for each streaming tick:

```
IB Gateway
  │
  │  [TCP socket, IB binary protocol]
  ▼
rust-ibapi (internal decoder + broadcast channel)
  │
  │  [ibapi::Subscription<T> implements Stream]
  ▼
midas-broker drain task (tokio::spawn)
  │  - Reads from ibapi Subscription
  │  - Translates ibapi types → BrokerEvent
  │  - Coalesces L1 ticks (50ms window)
  │
  │  [tokio::broadcast channel, capacity 4096]
  ▼
iced broker_subscription (subscription::channel)
  │  - Reads from broadcast receiver
  │  - Converts BrokerEvent → Message
  │
  │  [iced internal mpsc, capacity 100]
  ▼
MidasApp::update()
  │  - Updates chart state (tick_dirty / render_dirty flags)
  │
  │  [animation tick at 60 fps]
  ▼
MidasApp::view() → ChartWidget::prepare()
  │  - Reads dirty flags
  │  - Rebuilds CandleInstance buffer if render_dirty
  │  - Uploads to GPU via queue.write_buffer()
  │
  │  [wgpu render pass]
  ▼
Screen pixel
```

**End-to-end latency budget:**

| Stage | Typical Latency |
|---|---|
| IB Gateway → rust-ibapi decode | < 1 ms |
| Drain task + coalesce | ~50 ms (coalescing window) |
| Broadcast → iced subscription | < 1 ms |
| iced message delivery | < 1 ms |
| update() state change | < 0.1 ms |
| Next animation tick (worst case) | 16 ms |
| GPU prepare + render | < 4 ms |
| **Total worst case** | **~72 ms** |
| **Total typical** | **~35 ms** |

For bar-level updates (BarClosed, BarUpdated), the coalescing window does not apply, so latency is typically under 25 ms.

#### Step 8: User Closes the Chart

```rust
Message::CloseChart(chart_id) => {
    let consumer_id = ConsumerId(chart_id as u64);
    self.send_command(BrokerCommand::UnsubscribeAll { consumer_id });
    self.charts.remove(chart_id);
}
```

Inside the broker engine:

1. Look up all subscriptions for `consumer_id` in `consumer_subs`
2. For each subscription key:
   - Decrement `ref_count`
   - If `ref_count == 0`:
     - Abort the drain task
     - The `rust-ibapi` `Subscription` is dropped, which automatically sends the cancel message to IB Gateway
     - Decrement `lines_used`
     - Remove from the subscription map
   - If `ref_count > 0` (another chart is also watching AAPL): do nothing, data keeps flowing
3. Remove `consumer_id` from `consumer_subs`

### 7.2 Reconnection Flow

When the connection drops (network failure or daily IB Gateway restart at ~11:45 PM ET):

1. `reconnect_monitor` task detects `client.is_connected() == false`
2. Updates `watch` channel to `ConnectionStatus::Reconnecting { attempt: 1 }`
3. iced status bar turns yellow, displays "Reconnecting..."
4. Exponential backoff: 1s, 2s, 4s, 8s, 16s, max 30s
5. On successful reconnect:
   - Wait for data farm status messages (2104/2106) before re-subscribing
   - Re-qualify all active contracts (contract IDs are stable but the connection is new)
   - Re-subscribe all `ManagedSubscription` entries in `SubscriptionManager`
   - Incremental-update all cached historical data (fetch bars missed during downtime)
   - Update `watch` to `ConnectionStatus::Connected`
   - iced status bar turns green
6. Charts continue displaying last known data during disconnection (no blank screens)

---

## Appendix A: rust-ibapi Subscription Lifecycle

Key behaviors of `ibapi::Subscription<T>` that the broker engine must account for:

| Behavior | Detail |
|---|---|
| **Async streaming** | Implements `Stream` (via `StreamExt`); use `.next().await` |
| **Cancellation on drop** | When the `Subscription` value is dropped, it sends the cancel request to IB. This is the primary cancellation mechanism. |
| **Broadcast internally** | `rust-ibapi` uses internal broadcast channels, so multiple `Subscription` handles to the same request are possible |
| **Error propagation** | Stream yields `None` when the subscription ends (server-side cancel, error, or disconnect) |
| **No automatic reconnection** | If the connection drops, all subscriptions end. The consumer must re-subscribe after reconnecting. |

The broker engine wraps each `ibapi::Subscription` in a `ManagedSubscription` that holds the `JoinHandle` to the drain task. When the engine needs to cancel:

```rust
// Cancel a managed subscription
managed.drain_task.abort();
// The ibapi::Subscription is dropped inside the aborted task,
// which triggers the cancel message to IB.
```

---

## Appendix B: IB Limits Quick Reference

Consolidated from `provider-ib.md` for quick lookup during implementation.

| Resource | Default Limit | Notes |
|---|---|---|
| Streaming lines (L1 + bars) | 100 | Scales with equity/commissions; +100 per $30/mo booster |
| Depth of book symbols | 3-60 | Uses same line pool |
| Tick-by-tick (US) | 5 simultaneous | 1 request per instrument per 15s |
| Historical requests | 60 per 10 min | BID_ASK counts as 2 |
| Historical burst | 6 per 2s per contract | |
| Identical historical request | 15s cooldown | |
| API messages/sec | 50 | Connection dropped if exceeded |
| Simultaneous connections | 32 | Per TWS/Gateway instance |
| Active scanners | 10 | |
| Account summary subs | 2 | |
| Max API connections | 32 (clientId 0-31) | |
