# Slice 3 — Sim Backend (IB-Faithful)

**Goal.** Implement `SimMarketData` and `SimOrderClient` against the new traits from S2, with IB-faithful wire behavior. The existing `TestBroker` stays in place (S9 removes it).

## Scope

New crate `midas-sim-backend` under root workspace, OR new module `crates/midas-broker/src/sim/` — pick based on whether tests from other crates need to depend on it. Recommend new module for now; a crate-level split can happen in S9 if needed.

### A. `SimMarketData`

`crates/midas-broker/src/sim/market_data.rs`.

Fields:
- `rng: Arc<Mutex<Xorshift64>>` — random-walk RNG (same as current test broker).
- `subscriptions: DashMap<ReqId, SimSubscription>` — reqId-keyed; multiple per symbol.
- `symbol_state: DashMap<SymbolKey, SymbolSimState>` — base price, RNG drift, last emit time.
- `farm_status_tx: broadcast::Sender<FarmStatus>` (cap 16).
- `conn_state_tx: watch::Sender<ConnectionState>`.
- `req_id_counter: AtomicU64`.
- `config: SimConfig` — `tick_cadence_ms` (default 250), `tick_drift_bps` (10), `burst_enabled` (true), `farm_up_delay_ms` (100), `late_tick_window_ms` (default **200** per M-24; `late_tick_window_ms_max` 500 for slow-network simulation), `historical_seam_mode` (Aligned | Overlap1 | Gap1), `historical_last_ts: Option<DateTime<Utc>>` (BR-21: explicit override for deterministic seam tests).
- `tick_loop_task: JoinHandle<()>` — background task driving the tick emitter loop. **BR-20**: its internal timer uses `tokio::time::interval`, not `std::thread::sleep` / `std::time::Instant`, so `tokio::time::pause()` works in tests.

Subscription state:
```rust
struct SimSubscription {
    req_id: ReqId,
    symbol: SymbolKey,
    con_id: i32,
    kind: StreamKind,
    tx: broadcast::Sender<Arc<Tick>>,   // per-subscription broadcast (4096 cap)
    cancelled: AtomicBool,
}
```

### B. IB-faithful event sequences

**On `subscribe_ticks(symbol, con_id)`:**
1. Allocate reqId from counter.
2. Spawn a task that, after a small delay (50–100 ms), emits the initial burst:
   - `TickPrice(Bid, base_price - spread/2)` + `TickSize(BidSize, 100)`
   - `TickPrice(Ask, base_price + spread/2)` + `TickSize(AskSize, 100)`
   - `TickPrice(Last, base_price)` + `TickSize(LastSize, 100)`
   - `TickSize(Volume, 0)`
   - `TickPrice(High, base_price)`, `TickPrice(Low, base_price)`, `TickPrice(Close, base_price)`, `TickPrice(Open, base_price)`
   - `TickParams { min_tick: 0.01, bbo_exchange: "SMART", snapshot_permissions: 0 }`
3. Register subscription in `subscriptions` map so the tick-loop task picks it up.
4. Return `TickStream { req_id, rx: tx.subscribe(), cancel: move || { cancel_sub(req_id) } }`.

**Tick-loop task** (`tick_emitter_loop`):
- Wakes every `config.tick_cadence_ms` (default 250 ms) via `tokio::time::interval` (BR-20 — paused-time-friendly).
- BR-11 sampling emulation: internal RNG drift may advance many times per cadence window, but the loop emits **at most one Last event + one paired (Bid/Ask, BidSize/AskSize) set per window**, regardless of how many drift steps occurred internally. Matches IB's reqMktData sampling (~250 ms aggregation).
- Emission schedule per window:
  - If random price-moved: emit a single `TickValue::PriceSize { price, size }` for Last (M-17 atomic pair). `Volume` sends an additive delta.
  - Emit one paired Bid snapshot: `PriceSize { Bid, BidSize }`.
  - Emit one paired Ask snapshot: `PriceSize { Ask, AskSize }`.
  - Occasionally (1/20): emit `TickString(LastTimestamp, ...)` for completeness.
  - The `tick_fold_count` / `ticks_folded` concept is removed (M-36) — sim does not expose this to public API.

**On `cancel` closure firing (handle dropped):**
- Set `cancelled = true`.
- M-25 (deferred-removal): move the SymbolHub to a "draining" state for ~`late_tick_window_ms` (default 200 ms per M-24); during the drain window, the hub still absorbs late ticks so in-flight publishes don't hit a dangling sender. After the window, a GC sweep drops the sub from the map.
- Send `SubscriptionEnded { req_id, reason: Cancelled }`.

**On `subscribe_realtime_bars(symbol, con_id, what_to_show)`:**
- Allocate reqId.
- Internally open a tick subscription for the symbol (reuse the logic). Do NOT expose ticks.
- Spawn aggregator task on the subscription: every 5 seconds wall clock, emit a `Bar` with o/h/l/c/v from the last 5s of ticks.
- First bar emits 5–10 s after subscribe (wait for the first 5s boundary).
- Return `RealtimeBarStream`.

**BR-9 split: `historical_bars` (one-shot):**
- Synthesize bars using the existing `TestDataProvider` logic (or port it inline).
- Return `HistoricalBarsResult { bars, first_ts, last_ts }` — `last_ts` uses `config.historical_last_ts` if set (BR-21), else `Utc::now()`.
- No stream; no update tail.

**BR-9 split: `historical_stream` (live-tail):**
- Emit `HistoricalStreamEvent::Historical(bars)` (single batch, M-16).
- Emit `HistoricalStreamEvent::End { first_ts, last_ts }` — again using `config.historical_last_ts` when set for determinism.
- Keep the mpsc open and start emitting `HistoricalStreamEvent::Update(bar)` at the requested bar cadence for the current partial bar.
- Stop when the handle drops (`BR-2` cancel closure fires on Drop).

**Connection lifecycle:**
- **NM-2: `SimMarketData::new` eagerly drives the connect sequence.** The constructor spawns a background task that walks `conn_state_tx` through `Disconnected` → `Connected { server_version: 176 }` → (after `farm_up_delay_ms`) → farm-up events → `OrderingReady` → `Ready`. No explicit `connect()` call is needed — `SimMarketData::new` is the entry point; anything holding an `Arc<dyn MarketDataSource>` to a sim backend can assume the state machine is already in motion. (`SimOrderClient` does NOT drive the sim's market-data connection; each impl owns its own lifecycle.)
- Sequence driven by `SimMarketData::new`:
  - Set `conn_state_tx` → `Connected { server_version: 176 }`.
  - After `farm_up_delay_ms` (100 ms default): emit `FarmStatus { code: MarketDataFarmOk, connected: true }`, `HistoricalDataFarmOk`, `SecDefFarmOk` (M-13).
  - Emit `MarketEvent::OrderingReady { next_order_id: <seed> }` — M-14: NOT a FarmCode.
  - After all of the above are sent: set `conn_state_tx` → `Ready`.
- M-20 — `simulate_connection_lost(code: FarmCode)` takes the code explicitly:
  - `ConnectionRestoredDataLost` (1101): emit `SubscriptionEnded { reason: FarmDropped }` on every active subscription. Consumers must re-subscribe.
  - `ConnectionRestoredDataKept` (1102): log, no-op. Subs continue.
  - `ConnectionLost` (1100): emit 1100 farm event; drop all subs (as 1101); set `conn_state_tx` → `Reconnecting { attempt: n }`.
- On simulated reconnect: re-run the connect path; emit `ConnectionRestoredDataLost` first if the loss was 1100/1101.

### C. `SimOrderClient`

`crates/midas-broker/src/sim/order_client.rs`.

Port the existing `TestBroker` order-management logic: `place_order`, `cancel_order`, bracket tracking, fill simulation, `OrderStatus` / `Execution` emission.

Replace the `poll_callbacks` drain model with direct `broadcast::Sender<OrderEvent>`. No more 10 ms poll loop; events fire as they occur.

Keep the `fill_timing`, `partial_fill_tranches`, `rejection_rate`, `initial_cash` config knobs. They're all test-harness features we still want.

### D. Config

```rust
pub struct SimConfig {
    pub market_data: SimMarketDataConfig,
    pub orders: SimOrderConfig,
}

pub struct SimMarketDataConfig {
    pub tick_cadence_ms: u64,         // 250
    pub tick_drift_bps: f64,          // 10
    pub burst_enabled: bool,          // true
    pub farm_up_delay_ms: u64,        // 100
    pub late_tick_window_ms: u64,     // 50
    pub historical_seam_mode: SeamMode, // Aligned
    pub realtime_bar_size_secs: u64,  // 5
}
```

## Tests

### Fidelity tests (new — use these as acceptance gates)

All in `crates/midas-broker/tests/sim_fidelity.rs`. **BR-20: every timing-sensitive test uses `#[tokio::test(start_paused = true)]` + `tokio::time::advance` (not wall-clock `sleep`). Document this as a sim-internals invariant in `91-testing.md`.**

1. `initial_burst_emits_all_tick_types` — after `subscribe_ticks`, advance time 200 ms, receive Bid, Ask, Last, BidSize, AskSize, LastSize, Volume, TickParams. Exactly once each.

2. `multiple_subs_fan_out` — subscribe twice to AAPL with different reqIds. Advance 5 s. Each receives its own ticks; counts within ±10%.

3. `cancel_terminates_stream_within_500ms` (BR-20 rename) — drop handle, assert stream yields `Err(Closed)` within 500 ms of `advance`.

4. `tick_cadence_is_in_range` — BR-20: paused-time; advance 5 s in increments; assert tick count lands between 10 and 40 (no wall-clock noise).

5. `historical_stream_transitions` — call `historical_stream`, receive `Historical(bars)` + `End`, advance 5 s, receive `Update` at bar cadence. (BR-9 split method.)

6. `reconnect_drops_all_subscriptions` — subscribe, `simulate_connection_lost(ConnectionRestoredDataLost)`, then reconnect. Assert: original handle's next returns `Err(Closed)`. Fresh `subscribe_ticks` works.

7. `farm_status_sequence_on_connect` — BR-20 paused-time. On connect, observe MarketDataFarmOk, HistoricalDataFarmOk, SecDefFarmOk (M-13). `OrderingReady` arrives as a separate `MarketEvent::OrderingReady` (M-14), NOT as a FarmStatus.

8. `order_place_and_fill_flow` — place a market buy, observe `OrderEvent::Submitted`, `StatusChanged(Submitted)`, `ExecutionDetails { exec_id }`, `Commission { exec_id }` (M-19), `StatusChanged(Filled)`.

9. `connection_lost_1101_vs_1102` (M-20) — call `simulate_connection_lost(ConnectionRestoredDataLost)` with one open sub, assert `SubscriptionEnded { reason: FarmDropped }` arrives. Repeat with `ConnectionRestoredDataKept` — no SubscriptionEnded fires; the sub keeps delivering once farm is back.

10. `deterministic_historical_seam` (BR-21) — with `config.historical_last_ts = Some(t0)`, call `historical_stream`, assert `End { last_ts: t0 }`. Follow-up bars from the live tail are filtered by the router's `ts_open > t0` seam.

### Isomorphism tests

`crates/midas-broker/tests/ib_sim_isomorphism.rs` — these test that `IbMarketData` (slice 4) and `SimMarketData` produce the same sequence of `MarketEvent`s given identical synthetic inputs. Stub / skip these until S4 is also landed; mark `#[ignore]` and leave stubs for the S4 implementer to un-ignore.

## Acceptance

- All 8 fidelity tests pass.
- `cargo test -p midas-broker` all existing + new tests pass.
- `cargo clippy --workspace -- -D warnings` clean.
- `cargo fmt --all`.
- Existing `TestBroker` still compiles and its tests still pass (we haven't touched it).

## Risks

- The tick emitter loop is a new long-running task; ensure it shuts down when the sim is dropped (use `CancellationToken`).
- Poisson-distributed emission is tricky; a simpler "with probability p, emit; else don't" with `p = 0.3` per cadence window gives realistic-looking bursts without needing a real Poisson sampler.
- `broadcast::Sender::receiver_count()` needs to be checked for zero to avoid generating ticks for dead subscriptions. Auto-exit after N consecutive zero-send cycles (M-4).
- `OrderingReady` is its own `MarketEvent` variant (M-14), not a FarmCode — make sure the router and consumers treat it consistently.
- **OPEN**: `inject_for_test` path requires an exposed push-point into the tick hubs/farm-status broadcast that bypasses the emitter loop. Implementer must wire this so BR-15's dev-harness migration works. See S7 §G.
