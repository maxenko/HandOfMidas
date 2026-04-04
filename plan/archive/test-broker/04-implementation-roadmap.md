# 04 — Implementation Roadmap

> Phased delivery, test matrix, and acceptance criteria for the test broker.

---

## Table of Contents

- [1. Implementation Phases](#1-implementation-phases)
- [2. Test Matrix](#2-test-matrix)
- [3. Acceptance Criteria](#3-acceptance-criteria)
- [4. File Map](#4-file-map)

---

## 1. Implementation Phases

### Phase 1: Core Fill Engine (~500 lines, +~300 lines tests)

**Goal**: Market orders fill instantly. Bracket lifecycle works end-to-end.

1. Create `crates/midas-broker/src/test_broker.rs`
   - `TestBroker` struct with order book, callback queue, market prices
   - Implement `BrokerClient` trait
   - `SimulatedOrder` with status tracking
   - Bracket link tracking (parent → children)

2. Bracket activation logic
   - `transmit=false` orders go to Held state
   - `transmit=true` activates entire bracket
   - Parent fill → children activate
   - OCA: one child fills → sibling auto-cancels

3. Instant fill mode for market orders
   - `place_order(MKT)` → immediate fill at seeded price
   - Status callback sequence: Submitted → Execution → Filled

4. `simulate_fill()` public method for test control
   - Manually trigger fills at specific prices
   - Drives bracket lifecycle from tests

5. Wire into engine — poll interval
   - Add `poll_callbacks()` arm to engine's `tokio::select!` loop
   - Poll interval is always active (default `poll_callbacks()` returns
     empty vec for non-test clients, so the cost is negligible)

6. Wire into engine — callback translation
   - Implement `handle_broker_callback()` method on `BrokerEngine`
   - Match on `BrokerCallback` variants (OrderStatus, Execution, OrderRejected)
   - Look up local UUID via `ib_to_local` map
   - Validate state transitions, update DB, emit `BrokerEvent`s
   - Call `check_bracket_status_change()` for bracket members

> **Note**: Only `OrderStatus`, `Execution`, and `OrderRejected` callbacks
> are needed for Phase 1. The remaining variants (`Tick`, `BarUpdated`,
> `BarClosed`, `ConnectionStatus`, `Position`, `AccountValue`) are added
> to `BrokerCallback` in their respective phases (4–6).

**Prerequisite**: State machine fix (`PreSubmitted → Cancelled` and
`Submitted → Cancelled` transitions) -- already implemented.

**Done when**: `test_full_bracket_lifecycle_tp_hit` passes:
create bracket → parent fills → TP fills → SL auto-cancels →
BracketStatusChanged { TakeProfitHit } received.

> **Recommendation**: Phase 1 can be split into (a) TestBroker in isolation
> (struct, fill engine, bracket links, unit tests) and (b) engine integration
> (poll_callbacks wiring, callback translation, integration tests), for
> shorter feedback loops.

### Phase 2: Limit & Stop Orders (~200 lines)

**Goal**: Price-triggered fills for all order types.

1. Limit order fill logic
   - BUY LMT fills when price ≤ limit
   - SELL LMT fills when price ≥ limit
   - Marketable limits fill immediately

2. Stop order trigger logic
   - STP stays PreSubmitted until triggered
   - SELL STP triggers when price ≤ stop_price
   - BUY STP triggers when price ≥ stop_price
   - After trigger → market fill (with optional slippage)

3. Stop Limit orders
   - Trigger like stop, then limit order behavior
   - May not fill if price gaps through limit

4. `set_market_price()` public method
   - Updates price for a symbol
   - Checks all pending limit/stop orders

**Done when**: `test_limit_bracket_fills_on_price_cross` passes.

### Phase 3: Partial Fills & Timing (~150 lines)

**Goal**: Realistic fill behavior for large orders.

1. Partial fill support
   - Split fills into configurable tranches
   - Separate Execution callback per tranche
   - PartiallyFilled status between tranches

2. Delayed fill mode
   - Configurable delay before fills
   - Timer-driven callback delivery

3. Slippage model
   - Configurable max slippage in basis points
   - Random slippage per fill (seeded for determinism)

**Done when**: `test_partial_fill_three_tranches` passes.

### Phase 4: Market Data (~200 lines)

**Goal**: Tick generation and bar streaming from TestDataProvider.

1. Tick generation from OHLCV data
   - Interpolate within bar range
   - Configurable spread
   - Seed market prices from daily closes

2. Auto-tick mode
   - Generate ticks at configurable interval
   - Drives price-triggered fills

3. Forming bar tracking
   - BarUpdated on tick
   - BarClosed on period completion

**Done when**: `test_auto_tick_triggers_stop_loss_fill` passes.

### Phase 5: Account & Positions (~150 lines)

**Goal**: Position tracking and P&L on fills.

1. Position updates on fills
   - Weighted average cost
   - Long/short tracking
   - Position flip handling

2. Account values
   - Cash balance (decreases on buy, increases on sell)
   - Net liquidation (cash + position value)
   - Buying power (cash × margin)

3. P&L
   - Unrealized (mark-to-market)
   - Realized (on position close)

**Done when**: `test_position_updates_on_bracket_fill` passes.

### Phase 6: Error Injection & Connection (~100 lines)

**Goal**: Simulate failures for resilience testing.

1. Configurable rejection rate
2. Connection loss/reconnect simulation
3. Cancel race condition simulation
4. Missing reference price scenarios

**Done when**: `test_reconnect_reconciles_open_orders` passes.

---

## 2. Test Matrix

### Unit Tests (in test_broker.rs)

| Test | Phase | What It Verifies |
|---|---|---|
| `test_next_order_id_increments` | 1 | ID allocation |
| `test_place_market_order_instant_fill` | 1 | MKT → Submitted → Filled |
| `test_bracket_held_until_transmit` | 1 | transmit=false holds, transmit=true activates |
| `test_bracket_parent_fill_activates_children` | 1 | Parent Filled → TP/SL Submitted |
| `test_bracket_tp_fill_cancels_sl` | 1 | OCA: TP Filled → SL Cancelled |
| `test_bracket_sl_fill_cancels_tp` | 1 | OCA: SL Filled → TP Cancelled |
| `test_bracket_parent_cancel_cancels_children` | 1 | Cancel parent → all cancelled |
| `test_simulate_fill_manual` | 1 | Explicit fill control |
| `test_limit_buy_fills_at_price` | 2 | BUY LMT fills when price drops |
| `test_limit_sell_fills_at_price` | 2 | SELL LMT fills when price rises |
| `test_stop_triggers_at_price` | 2 | STP triggers and fills |
| `test_stop_limit_may_not_fill` | 2 | STP LMT gaps through limit |
| `test_set_market_price_triggers_fills` | 2 | Price update drives fills |
| `test_partial_fill_tranches` | 3 | Large order → multiple executions |
| `test_delayed_fill_mode` | 3 | Fill after configurable delay |
| `test_slippage_within_bounds` | 3 | Fill price within max_bps |
| `test_tick_generation_from_ohlcv` | 4 | Ticks interpolate within bar |
| `test_auto_tick_drives_fills` | 4 | Periodic ticks trigger limit fills |
| `test_position_long_buy` | 5 | BUY → position increases |
| `test_position_close_sell` | 5 | SELL to close → position flat |
| `test_account_cash_decreases_on_buy` | 5 | Cash accounting |
| `test_unrealized_pnl_calculation` | 5 | Mark-to-market P&L |
| `test_rejection_by_configuration` | 6 | Configurable rejection rate |
| `test_disconnect_reconnect` | 6 | Connection simulation |

### Integration Tests (in engine.rs)

| Test | Phase | What It Verifies |
|---|---|---|
| `test_full_bracket_lifecycle_tp_hit` | 1 | Create → fill → TP hit → SL cancelled → BracketStatusChanged |
| `test_full_bracket_lifecycle_sl_hit` | 1 | Create → fill → SL hit → TP cancelled |
| `test_bracket_cancelled_before_fill` | 1 | Create → cancel → all cancelled |
| `test_bracket_parent_rejected` | 6 | Create → reject → children cancelled |
| `test_modify_tp_after_fill` | 2 | Fill → modify TP price → verify DB/IB updated |
| `test_modify_sl_after_fill` | 2 | Fill → modify SL price |
| `test_naked_market_fills_instantly` | 1 | No TP/SL → single fill |
| `test_concurrent_brackets_independent` | 1 | Two brackets don't interfere |
| `test_bracket_status_events_sequence` | 1 | Submitted → EntryFilled → TakeProfitHit in order |
| `test_position_after_bracket_close` | 5 | Bracket fills → position exists → TP hits → position flat |

---

## 3. Acceptance Criteria

### Must Have

- [ ] Market orders fill instantly (or with configurable delay)
- [ ] Bracket parent fill activates children
- [ ] OCA: one child fills → sibling auto-cancels
- [ ] Parent cancel → children auto-cancel
- [ ] Correct status callback sequence (matches IB exactly)
- [ ] `simulate_fill()` for explicit test control
- [ ] `set_market_price()` for price-triggered fills
- [ ] Limit orders fill on price cross
- [ ] Stop orders trigger on price cross, then fill
- [ ] `poll_callbacks()` integrates with engine event loop
- [ ] All existing tests continue to pass
- [ ] Full bracket lifecycle test (create → fill → TP hit → verify)
- [ ] Deterministic (seeded RNG, reproducible fill prices)

### Should Have

- [ ] Partial fill support with configurable tranches
- [ ] Slippage model (configurable max basis points)
- [ ] Position tracking on fills
- [ ] Account cash balance updates
- [ ] Tick generation from TestDataProvider OHLCV
- [ ] Stop Limit order support

### Nice to Have

- [ ] Auto-tick mode driving price-triggered fills
- [ ] Unrealized/realized P&L calculation
- [ ] Connection loss/reconnect simulation
- [ ] Configurable rejection rate
- [ ] Bar streaming (forming + closed)
- [ ] Cancel race condition simulation

---

## 4. File Map

| File | Lines (est.) | Description |
|---|---|---|
| `crates/midas-broker/src/test_broker.rs` | ~600 | TestBroker struct, fill engine, order book |
| `crates/midas-broker/src/client.rs` | ~50 (additions) | Extend BrokerClient trait with poll_callbacks, connect, etc. |
| `crates/midas-broker/src/engine.rs` | ~50 (additions) | Add poll interval, callback translation |
| `crates/midas-broker/src/config.rs` | ~30 (additions) | TestBrokerConfig |
| **Total new code** | **~730** | |
| **Total new tests** | **~500** | 24 unit + 10 integration |

### Dependency Graph

```
TestBroker (new)
  ├── BrokerClient trait (existing, extended)
  ├── TestDataProvider (existing, composed)
  ├── BrokerCallback enum (new)
  └── SimulatedOrder (new, internal)

BrokerEngine (existing, modified)
  ├── client: Box<dyn BrokerClient>
  ├── poll_callbacks() in select! loop (new)
  └── handle_broker_callback() (new)
```

### Migration from TestBrokerClient

The current `TestBrokerClient` (accept-only) stays as-is for simple tests.
The new `TestBroker` is used when full simulation is needed:

```rust
// config.toml
[data_source]
type = "test"

[test_broker]
fill_timing = "instant"    # or "delayed" or "price_triggered"
```

```rust
// In start_broker_engine():
let client: Option<Box<dyn BrokerClient>> = match &config.data_source {
    DataSourceConfig::Test => {
        if config.test_broker.fill_timing == "none" {
            Some(Box::new(TestBrokerClient::new()))  // accept-only stub
        } else {
            Some(Box::new(TestBroker::new(config.test_broker.clone())))  // full sim
        }
    }
    DataSourceConfig::Live => None,
};
```
