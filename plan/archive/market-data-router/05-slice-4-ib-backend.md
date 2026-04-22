# Slice 4 — IB Backend

**Goal.** Implement `IbMarketData` and `IbOrderClient` against the new traits from S2, wrapping `rust-ibapi` cleanly. The existing `IbClient` stays in place (S9 removes it).

## Scope

### A. `IbMarketData`

`crates/midas-broker/src/ib/market_data.rs`.

Fields:
- `client: Arc<ibapi::Client>` — shared rust-ibapi client.
- `runtime: Handle` — tokio runtime handle for `block_on` bridges where needed.
- `req_id_counter: AtomicU64`.
- `farm_status_tx: broadcast::Sender<FarmStatus>`.
- `conn_state_tx: watch::Sender<ConnectionState>`.
- `subscriptions: DashMap<ReqId, IbSubscriptionHandle>`.
- `error_watcher_task: JoinHandle<()>` — consumes global error stream, dispatches to per-reqId channels or farm-status broadcast based on code.

### B. `subscribe_ticks` implementation (BR-10)

rust-ibapi 2.10 exposes `reqMktData` via a builder. The current `ib_client.rs` does NOT call `market_data` at all — it calls `realtime_bars`. This slice adds the tick path for the first time:

```rust
let tick_sub = client
    .market_data(contract)
    .with_generic_ticks(generic_ticks.0.clone())   // BR-10: e.g. vec![233, 293]
    .snapshot(false)
    .regulatory_snapshot(false)
    .subscribe()
    .await?;
```

The `Cargo.toml` pin `ibapi = "=2.10"` prevents drift (handled in S0 prep). If 2.11+ reshapes the builder, the lock must be updated in a separate change.

Translate each `TickTypes` variant into our `Tick` event:
- `TickTypes::Price { tick_type, price, attrs }` → `Tick { tick_type: translate(tick_type), value: TickValue::Price(price), attrs: translate(attrs) }`.
- `TickTypes::Size { tick_type, size }` → `Tick { value: Size(size) }`.
- `TickTypes::PriceSize { price, size, tick_type, attrs }` → single `Tick { kind: PriceSize, value: TickValue::PriceSize { price, size } }` (M-17 atomic pair — NOT split).
- `TickTypes::String { ... }` → `Tick { kind: String, value: Text(s) }`.
- `TickTypes::Generic { ... }` → `Tick { kind: Generic, value: Generic(v) }`.
- `TickTypes::Params { ... }` → `Tick { kind: Params, ... }`.

Wrap the rust-ibapi `Subscription` drop-cancel in our `TickStream` Drop closure (`BR-2` Option<Box> shape). rust-ibapi's `Subscription` already auto-cancels on drop; our wrapper just holds it until dropped.

### B.1 `subscribe_tick_by_tick` (BR-11)

Calls `client.tick_by_tick_last(contract)` / `tick_by_tick_bid_ask` / `tick_by_tick_mid_point` / `tick_by_tick_all_last` depending on `kind`. Translate each event into `Tick` with `kind = PriceSize` or `Price`. Gate the call through the `PacingGovernor` (§D.1) because IB caps 5 concurrent + 15 s identical-throttle.

### C. `subscribe_realtime_bars`

Calls `client.realtime_bars(contract, BarSize::Sec5, what_to_show.into(), TradingHours::Regular).await`. Returns rust-ibapi `Subscription<Bar>`.

Translate each `ibapi::Bar` → our `Bar` event. Emit via broadcast. Drop the rust-ibapi subscription on handle drop.

### D. Historical APIs (BR-9 split)

rust-ibapi 2.10 exposes two separate methods, reflected in our trait:

1. **One-shot `historical_bars`** — maps to `client.historical_data(contract, end_date, duration, bar_size, what_to_show, use_rth).await`. Returns all bars synchronously; build `HistoricalBarsResult { bars, first_ts, last_ts }`. No live tail, no mpsc.
2. **Streaming `historical_stream`** — maps to `client.historical_data_streaming(contract, duration, bar_size, what_to_show, use_rth).await` (no `end_date`). Returns `Subscription<HistoricalBarsUpdate>`. Drain:
   - Collect the initial batch until the first `End` signal; emit `HistoricalStreamEvent::Historical(bars)` (M-16, single batched event) then `End { first_ts, last_ts }`.
   - Subsequent updates → `HistoricalStreamEvent::Update(bar)` loop until handle drops.

Both paths flow through the `PacingGovernor` (§D.1).

### D.1 PacingGovernor (BR-19) — new sub-section

IB enforces strict rate limits:
- 6 identical historical requests per 2 s (by `(contract, bar_size, what_to_show, use_rth)` key), with a 15 s cooldown on identical-request repeats.
- 60 historical requests total in any 10-minute rolling window.
- ~100 concurrent streaming lines (reqMktData / reqRealTimeBars).

Missing any of these yields IB error 162 / 322 / soft pacing warnings, which cascade into dropped subscriptions and reconnects.

```rust
pub struct PacingGovernor {
    // 60 tokens / 10 minutes, refilled continuously.
    historical_total: RwLock<TokenBucket>,
    // Per-identical-key bucket: 6 tokens / 2 s + 15 s cooldown.
    historical_per_key: DashMap<IdenticalKey, TokenBucket>,
    // Concurrent streaming lines (ticks + rt bars + tbt).
    streaming_lines: AtomicU32,
    config: PacingConfig,
}

pub struct PacingConfig {
    pub historical_max_in_10min: u32,   // 60
    pub identical_burst: u32,           // 6
    pub identical_cooldown: Duration,   // 15 s
    pub streaming_line_limit: u32,      // 95 (leave headroom under IB's ~100)
    pub on_violation: PacingPolicy,     // Queue | Reject
}

pub enum PacingPolicy { Queue, Reject }
```

Behaviour:
- `subscribe_ticks` / `subscribe_tick_by_tick` / `subscribe_realtime_bars` increment `streaming_lines`; on drop, decrement. If limit would be exceeded, return `Err(MarketDataError::StreamingLineLimitExceeded)`. Handles decrement via their `Drop` closure.
- `historical_bars` / `historical_stream` consult `historical_total` + `historical_per_key`; exceeded → `PacingPolicy::Queue` waits until a token is available (with a 2 s upper delay), `PacingPolicy::Reject` returns `Err(MarketDataError::PacingViolation)`. Default: `Queue` (better UX).
- Governor is owned by `IbMarketData`; tests construct it standalone with shrunken config to verify token accounting.

### E. Farm-status watcher

`error_watcher_task` consumes the global error stream from `client.errors().await` (or whatever rust-ibapi exposes). For each error:
- If `reqId == -1` and `code` is in {2103, 2104, 2105, 2106, 2108, 2158, 1100, 1101, 1102}: convert to `FarmStatus` variant (M-13), send on `farm_status_tx`.
- If `code` is in {354, 10089, 10167}: look up `reqId` in `subscriptions` map, deliver as `SubscriptionEnded` or `MarketEvent::Error` on that subscription's channel. Distinguish 354 (delayed-subscribed) from 10167 (requires-additional-subscription) (M-15).
- M-14: on first `nextValidId` arrival, emit `MarketEvent::OrderingReady { next_order_id }` via a separate channel; do NOT fold it into `FarmStatus`.
- Other errors: log at `warn` level, continue.

### F. Connection lifecycle

- `connect()` implemented as `IbOrderClient::connect` (below). Updates `conn_state_tx`.
- On reconnect after farm drop: no automatic resubscribe. The router's retry policy handles that (S5).

### G. `IbOrderClient`

`crates/midas-broker/src/ib/order_client.rs`. Port the existing `IbClient` order-management logic. Use rust-ibapi `Subscription<PlaceOrder>` per-order stream to route `OrderStatus`, `ExecutionData`, `CommissionReport`, etc. Translate each into `OrderEvent`.

Positions: use rust-ibapi `client.positions().await` returning `Subscription<PositionUpdate>`. Route to `position_events` broadcast.

Account: use `client.account_updates()` or `client.account_summary()`. Route to `account_events`.

## Tests

### Live-IB integration tests

Keep these minimal and behind `#[ignore]` unless a special feature flag is set. Running them requires a real IB connection. The ones we do want:

1. `ib_connect_reports_server_version` — connect to local TWS paper, assert `connection_state` reaches `Connected` with a reasonable server_version.
2. `ib_subscribe_ticks_emits_bursts` — subscribe to SPY, assert at least `Bid`, `Ask`, `Last`, and `Params` tick types arrive within 2 s.
3. `ib_historical_stream_transitions` (BR-9) — call `historical_stream` for 1-hour 1-min bars, assert `Historical(bars)` then `End` then `Update` arrives within 15 s.
4. `ib_farm_status_fires_on_connect` — assert at least one `MarketDataFarmOk` within 5 s of connect.

Gate with `#[cfg_attr(not(feature = "ib_live_tests"), ignore)]`.

### Isomorphism tests

Un-ignore `ib_sim_isomorphism.rs` from S3. Test structure: for each scenario (subscribe, get initial burst, get periodic ticks, cancel), run against both `SimMarketData` (fast) and `IbMarketData` (real IB, ignored unless feature). Assert the *shape* of the event stream matches — not timing, not exact values, but "sequence of event kinds and their attributes."

Example:
```rust
#[tokio::test]
async fn subscribe_ticks_initial_burst_shape() {
    let sim = SimMarketData::new(SimConfig::default());
    assert_burst_shape(&sim, "AAPL").await;

    #[cfg(feature = "ib_live_tests")]
    {
        let ib = IbMarketData::new(/* local TWS */).await;
        assert_burst_shape(&ib, "SPY").await;
    }
}
```

`assert_burst_shape` asserts "within 2 s of subscribe, we receive at least one each of {Bid, Ask, Last, BidSize, AskSize, LastSize, Volume, Params} events."

## Acceptance

- `cargo test -p midas-broker --lib` passes all non-ignored tests.
- `cargo test -p midas-broker --test '*' -- --include-ignored ib_live_tests` passes if a paper-trading IB session is available. **BR-24: Tier 2 live-IB tests are developer-local, NOT CI-gated.**
- `cargo clippy -p midas-broker --features ib_live_tests -- -D warnings` clean.
- M-31: update `.github/workflows/rust.yml` in this slice to add a non-blocking job running `cargo clippy -p midas-broker --features ib_live_tests -- -D warnings`. This verifies the feature compiles even though tests are gated; it does NOT require a real IB connection because the feature only toggles `#[ignore]` on tests. Do not defer to S9.
- `cargo fmt --all`.
- Existing `IbClient` still compiles.
- PacingGovernor unit tests cover: historical burst-then-block, streaming line limit, identical-key cooldown, queue vs reject policy.

## Risks

- `rust-ibapi` version drift: lock to whatever version is in `Cargo.lock` right now; don't upgrade in this slice.
- `Subscription<T>` drop semantics vary across rust-ibapi versions — confirm behavior by reading the source. On drop, it sends `cancel_mkt_data`/`cancel_real_time_bars`/`cancel_historical_data` automatically (per research notes).
- The `error()` callback stream is how rust-ibapi surfaces per-reqId errors. Ensure we route those, not `panic!`.
- Dead-letter subscriptions after 1101 — on receiving 1101, emit `SubscriptionEnded` for every active sub and drop our internal handles; consumers will get `Err(Closed)` and must re-subscribe.
