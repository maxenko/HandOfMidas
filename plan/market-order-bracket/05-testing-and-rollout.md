# 05 — Testing & Rollout

> Test strategy, phased implementation, and acceptance criteria for
> Market Order brackets.

---

## Table of Contents

- [1. Test Strategy](#1-test-strategy)
- [2. Unit Tests](#2-unit-tests)
- [3. Integration Tests](#3-integration-tests)
- [4. Manual Test Scenarios](#4-manual-test-scenarios)
- [5. Implementation Phases](#5-implementation-phases)
- [6. Acceptance Criteria](#6-acceptance-criteria)
- [7. Risks and Mitigations](#7-risks-and-mitigations)

---

## 1. Test Strategy

### 1.1 Testing Pyramid

```
         ┌──────────┐
         │ Manual   │  5-10 scenarios: IB paper trading
         │ E2E      │  Real bracket lifecycle with TWS
         ├──────────┤
         │Integration│  15-20 tests: bracket builder + engine
         │          │  SQLite in-memory, mock IB client
         ├──────────┤
         │ Unit     │  40-60 tests: types, validation,
         │          │  state derivation, price resolution
         └──────────┘
```

### 1.2 Test Crate Map

| Crate | Test Focus | Count |
|---|---|---|
| `midas-broker` | BracketRole enum, BracketGroup, validation, state derivation, builder | 30-40 |
| `midas-chart` | OrderBracket P&L methods, label formatting, zone geometry | 10-15 |
| `midas-app` | Price resolution, OrderAnnotationLink, state mapping | 5-10 |
| Integration | Full bracket lifecycle (create → submit → fill → TP/SL → close) | 15-20 |

---

## 2. Unit Tests

### 2.1 BracketRole Tests (`midas-broker`)

```rust
#[cfg(test)]
mod bracket_role_tests {
    #[test] fn display_parent()       { assert_eq!(BracketRole::Parent.to_string(), "PARENT"); }
    #[test] fn display_take_profit()  { assert_eq!(BracketRole::TakeProfit.to_string(), "TAKE_PROFIT"); }
    #[test] fn display_stop_loss()    { assert_eq!(BracketRole::StopLoss.to_string(), "STOP_LOSS"); }
    #[test] fn parse_parent()         { assert_eq!("PARENT".parse::<BracketRole>().unwrap(), BracketRole::Parent); }
    #[test] fn parse_take_profit()    { assert_eq!("TAKE_PROFIT".parse::<BracketRole>().unwrap(), BracketRole::TakeProfit); }
    #[test] fn parse_legacy_profit()  { assert_eq!("PROFIT".parse::<BracketRole>().unwrap(), BracketRole::TakeProfit); }
    #[test] fn parse_legacy_stop()    { assert_eq!("STOP".parse::<BracketRole>().unwrap(), BracketRole::StopLoss); }
    #[test] fn parse_lowercase_parent()    { assert_eq!("parent".parse::<BracketRole>().unwrap(), BracketRole::Parent); }
    #[test] fn parse_lowercase_take_profit() { assert_eq!("take_profit".parse::<BracketRole>().unwrap(), BracketRole::TakeProfit); }
    #[test] fn parse_lowercase_stop_loss()   { assert_eq!("stop_loss".parse::<BracketRole>().unwrap(), BracketRole::StopLoss); }
    #[test] fn parse_unknown_fails()  { assert!("UNKNOWN".parse::<BracketRole>().is_err()); }
    #[test] fn serde_round_trip()     { /* JSON serialize + deserialize */ }
}
```

### 2.2 MarketBracketParams Validation Tests

```rust
#[cfg(test)]
mod validation_tests {
    #[test] fn valid_full_bracket()         { /* BUY 100 AAPL, TP 192, SL 182 */ }
    #[test] fn valid_tp_only()              { /* No SL — should pass with warning */ }
    #[test] fn valid_sl_only()              { /* No TP — should pass */ }
    #[test] fn valid_naked_market()          { /* No TP, no SL — should pass with warning */ }
    #[test] fn reject_empty_symbol()         { /* symbol: "" */ }
    #[test] fn reject_zero_quantity()        { /* quantity: 0.0 */ }
    #[test] fn reject_negative_quantity()    { /* quantity: -100.0 */ }
    #[test] fn reject_negative_tp_price()    { /* tp.price: -10.0 */ }
    #[test] fn reject_negative_sl_price()    { /* sl.stop_price: -10.0 */ }
}
```

### 2.3 Directional Validation Tests

```rust
#[cfg(test)]
mod direction_tests {
    #[test] fn buy_tp_above_entry()         { /* BUY, entry ~185, TP 192 → OK */ }
    #[test] fn buy_tp_below_entry_rejected() { /* BUY, entry ~185, TP 180 → Err */ }
    #[test] fn buy_sl_below_entry()         { /* BUY, entry ~185, SL 182 → OK */ }
    #[test] fn buy_sl_above_entry_rejected() { /* BUY, entry ~185, SL 190 → Err */ }
    #[test] fn sell_tp_below_entry()        { /* SELL, entry ~185, TP 178 → OK */ }
    #[test] fn sell_tp_above_entry_rejected(){ /* SELL, entry ~185, TP 190 → Err */ }
    #[test] fn sell_sl_above_entry()        { /* SELL, entry ~185, SL 190 → OK */ }
    #[test] fn sell_sl_below_entry_rejected(){ /* SELL, entry ~185, SL 180 → Err */ }
}
```

### 2.4 BracketGroup Tests

```rust
#[cfg(test)]
mod bracket_group_tests {
    #[test] fn legs_count_full()      { /* parent + TP + SL = 3 legs */ }
    #[test] fn legs_count_tp_only()   { /* parent + TP = 2 legs */ }
    #[test] fn legs_count_sl_only()   { /* parent + SL = 2 legs */ }
    #[test] fn can_activate_all_inactive() { /* All Inactive → true */ }
    #[test] fn can_activate_one_error()    { /* One Error → still true (Error can_activate) */ }
    #[test] fn cannot_activate_one_submitted() { /* One Submitted → false */ }
    #[test] fn is_active_when_parent_filled()  { /* Parent Filled → true */ }
    #[test] fn is_closed_all_terminal()        { /* All Filled/Cancelled → true */ }
    #[test] fn not_closed_child_live()         { /* TP Submitted → false */ }
}
```

### 2.5 Bracket Status Derivation Tests

```rust
#[cfg(test)]
mod derive_status_tests {
    #[test] fn parent_pending()       { /* Parent PendingSubmit → Submitted */ }
    #[test] fn parent_submitted()     { /* Parent Submitted → Submitted */ }
    #[test] fn parent_filled()        { /* Parent Filled, children Submitted → EntryFilled */ }
    #[test] fn tp_filled()            { /* Parent Filled, TP Filled, SL Cancelled → TakeProfitHit */ }
    #[test] fn sl_filled()            { /* Parent Filled, SL Filled, TP Cancelled → StopLossHit */ }
    #[test] fn parent_cancelled()     { /* Parent Cancelled → Cancelled */ }
    #[test] fn parent_rejected()      { /* Parent Rejected → Rejected */ }
    #[test] fn parent_error()         { /* Parent Error → Error */ }
    #[test] fn all_closed()           { /* All terminal → Closed */ }
    #[test] fn unexpected_parent_status_does_not_panic() {
        // Verify: if parent status is somehow unexpected, function returns
        // Closed with a warning log — does NOT panic.
    }
}
```

### 2.6 Bracket Builder Tests

```rust
#[cfg(test)]
mod builder_tests {
    #[test] fn builds_three_orders_full_bracket() {
        // Verify: parent is MKT, TP is LMT opposite side, SL is STP opposite side
        // All have correct parent_id linkage
        // All have correct bracket_role
        // All have correct quantity
        // All are in Inactive status (not Draft) for state machine compliance
    }

    #[test] fn builds_two_orders_tp_only() {
        // Verify: parent + TP, no SL
    }

    #[test] fn builds_two_orders_sl_only() {
        // Verify: parent + SL, no TP
    }

    #[test] fn sl_stop_limit_has_both_prices() {
        // Verify: SL with limit_price creates StopLimit order
    }

    #[test] fn children_have_gtc_default() {
        // Verify: TP and SL default to GTC
    }

    #[test] fn tags_propagate_to_children() {
        // Verify: parent tags are copied to children
    }

    #[test] fn strategy_propagates_to_children() {
        // Verify: parent strategy is copied to children
    }

    #[test] fn sell_bracket_children_are_buy() {
        // Verify: SELL entry → BUY TP, BUY SL
    }
}
```

### 2.7 Chart P&L Tests (`midas-chart`)

```rust
#[cfg(test)]
mod pnl_tests {
    #[test] fn dollar_risk_long()     { /* entry 185, SL 182, qty 100 → $300 */ }
    #[test] fn dollar_reward_long()   { /* entry 185, TP 192, qty 100 → $700 */ }
    #[test] fn dollar_risk_short()    { /* entry 185, SL 188, qty 100 → $300 */ }
    #[test] fn dollar_reward_short()  { /* entry 185, TP 178, qty 100 → $700 */ }
    #[test] fn dollar_risk_no_sl()    { /* No SL → None */ }
    #[test] fn dollar_reward_no_tp()  { /* No TP → None */ }
    #[test] fn dollar_risk_zero_qty() { /* qty None → None */ }
}
```

### 2.8 Price Resolution Tests (`midas-app`)

```rust
#[cfg(test)]
mod price_resolution_tests {
    #[test] fn absolute_returns_value()   { /* mode=Absolute, value=192 → 192 */ }
    #[test] fn offset_buy_tp()            { /* mode=Offset, +6.50, last=185 → 191.50 */ }
    #[test] fn offset_buy_sl()            { /* mode=Offset, -3.00, last=185 → 182.00 */ }
    #[test] fn percent_buy_tp()           { /* mode=Percent, +3.5%, last=185 → 191.475 */ }
    #[test] fn percent_buy_sl()           { /* mode=Percent, -1.9%, last=185 → 181.485 */ }
    #[test] fn offset_sell_tp()           { /* mode=Offset, +6.50, last=185, SELL → 178.50 */ }
    #[test] fn percent_sell_sl()          { /* mode=Percent, -1.9%, last=185, SELL → 188.515 */ }
}
```

---

## 3. Integration Tests

### 3.1 Test Infrastructure

Integration tests use:
- **In-memory SQLite**: `rusqlite::Connection::open_in_memory()`
- **Mock IB client**: Simulates `place_order`, `cancel_order`, and callbacks
- **Tokio test runtime**: `#[tokio::test]` for async engine tests

```rust
struct MockIbClient {
    orders_placed: Arc<Mutex<Vec<PlacedOrder>>>,
    next_id: AtomicI32,
    /// Queued callbacks to simulate IB responses.
    callbacks: Arc<Mutex<VecDeque<MockCallback>>>,
}

struct PlacedOrder {
    ib_order_id: i32,
    contract_symbol: String,
    order_type: String,
    action: String,
    quantity: f64,
    parent_id: Option<i32>,
    transmit: bool,
}
```

### 3.2 Integration Test Scenarios

```rust
#[tokio::test]
async fn full_bracket_lifecycle_tp_hit() {
    // 1. Create market bracket: BUY 100 AAPL, TP 192, SL 182
    // 2. Verify 3 orders placed to IB with correct transmit flags
    // 3. Simulate parent fill at 185.50
    // 4. Verify BracketStatusChanged { status: "entry_filled" }
    // 5. Simulate TP fill at 192.00
    // 6. Verify SL auto-cancelled by IB
    // 7. Verify BracketStatusChanged { status: "take_profit_hit" }
    // 8. Verify all orders terminal in SQLite
}

#[tokio::test]
async fn full_bracket_lifecycle_sl_hit() {
    // Same as above but SL fills instead of TP
    // Verify BracketStatusChanged { status: "stop_loss_hit" }
}

#[tokio::test]
async fn bracket_cancelled_before_fill() {
    // 1. Create and submit bracket
    // 2. Cancel bracket before parent fills
    // 3. Verify all three cancelled
    // 4. Verify BracketStatusChanged { status: "cancelled" }
}

#[tokio::test]
async fn bracket_parent_rejected() {
    // 1. Create bracket, IB rejects parent
    // 2. Verify children force-cancelled
    // 3. Verify BracketStatusChanged { status: "rejected" }
}

#[tokio::test]
async fn modify_tp_price_after_fill() {
    // 1. Create bracket, parent fills
    // 2. Modify TP price from 192 to 195
    // 3. Verify IB receives modification
    // 4. Verify SQLite updated
}

#[tokio::test]
async fn modify_sl_price_after_fill() {
    // Same as above but for SL
}

#[tokio::test]
async fn tp_only_bracket() {
    // 1. Create bracket with TP but no SL
    // 2. Verify only 2 orders placed (parent + TP)
    // 3. TP gets transmit=true
}

#[tokio::test]
async fn sl_only_bracket() {
    // 1. Create bracket with SL but no TP
    // 2. Verify only 2 orders placed (parent + SL)
    // 3. SL gets transmit=true
}

#[tokio::test]
async fn naked_market_order() {
    // 1. Create bracket with no TP, no SL
    // 2. Verify only 1 order placed (parent with transmit=true)
}

#[tokio::test]
async fn transmit_flag_ordering() {
    // Verify: parent transmit=false, TP transmit=false, SL transmit=true
    // Verify: with only TP, TP gets transmit=true
    // Verify: with only SL, SL gets transmit=true
}

#[tokio::test]
async fn ib_order_id_assignment() {
    // Verify unique IB order IDs assigned per leg (one next_order_id() call each)
    // Verify no ID collisions under concurrent bracket submission
}

#[tokio::test]
async fn concurrent_bracket_submission_no_id_collision() {
    // Submit two brackets concurrently
    // Verify all 6 orders have distinct IB order IDs
}

#[tokio::test]
async fn bracket_status_cache_prevents_duplicate_events() {
    // Verify BracketStatusChanged is only emitted when status actually changes
}

#[tokio::test]
async fn cancel_individual_tp_leaves_sl_live() {
    // Cancel TP after parent fills → SL remains live
}

#[tokio::test]
async fn cancel_individual_sl_logs_warning() {
    // Cancel SL after parent fills → warning logged, TP remains
}

#[tokio::test]
async fn stop_limit_sl_has_correct_order_type() {
    // Verify SL with limit_price creates "STP LMT" order at IB
}
```

---

## 4. Manual Test Scenarios

### 4.1 Paper Trading (IB Gateway)

These tests require a live IB paper trading account:

| # | Scenario | Steps | Expected |
|---|---|---|---|
| M1 | Basic market bracket | BUY 10 SPY, TP +2%, SL -1% | All three legs appear in TWS, parent fills instantly, TP/SL become live |
| M2 | TP hit | Set tight TP (+$0.10 from fill) | TP fills, SL auto-cancelled, bracket shows "Closed" on chart |
| M3 | SL hit | Set tight SL (-$0.10 from fill) | SL fills, TP auto-cancelled |
| M4 | Cancel bracket | Submit bracket, cancel before TP/SL hit | All children cancelled, position remains |
| M5 | Modify TP by drag | Drag TP line on chart after fill | IB order modified, new price reflected in TWS |
| M6 | Modify SL by drag | Drag SL line on chart | Same |
| M7 | Extended hours | Enable outside_rth, trade after market | Bracket created, fills may not happen until market open |
| M8 | Multiple brackets | Place 3 brackets on different symbols | Each bracket independent, correct lifecycle |
| M9 | Reconnect | Place bracket, disconnect/reconnect | Bracket state reconciles correctly |
| M10 | Sell bracket | SELL 10 SPY (short), TP below, SL above | Correct side logic for short position |

---

## 5. Implementation Phases

> **Updated 2026-04-02**: Audited against codebase. Completed items marked.
> Estimates revised to reflect remaining work only.

### Phase 1: Data Model — MOSTLY COMPLETE

**Goal**: BracketRole enum, MarketBracketParams, BracketGroup, persistence support — all with tests.

**Status**: ~90% complete. Types, enums, methods, and 61 broker tests + 15 chart tests implemented.

1. ~~Create `crates/midas-broker/src/orders/bracket.rs`~~ **DONE**
   - `MarketBracketParams`, `TakeProfitParams`, `StopLossParams` ✓
   - `BracketGroup` struct and methods ✓
   - `BracketLifecycleStatus` enum with `Display`/`FromStr`/`Serialize`/`Deserialize` ✓
   - `derive_bracket_status()` function (returns typed enum, no panics) ✓
   - `validate_market_bracket()` and `check_bracket_direction()` ✓
   - Unit tests (42 tests) ✓

2. ~~Modify `crates/midas-broker/src/orders/types.rs`~~ **DONE**
   - `BracketRole` enum (with case-insensitive `FromStr` accepting all variants) ✓
   - `bracket_role: Option<BracketRole>` (typed, not String) ✓
   - `FillInfo` struct ✓
   - `LocalOrder` new fields (algo, activation tracking) ✓
   - Unit tests (19 tests) ✓

3. ~~Modify `crates/midas-broker/src/orders/mod.rs`~~ **DONE**
   - `pub mod bracket;` ✓

4. Modify `crates/midas-broker/src/persist/order_repo.rs` — **PARTIALLY DONE**
   - `OrderRow` already contains required fields (bracket_role, parent_id) — confirmed ✓
   - Module header comment updated to reflect `BracketRole` dependency ✓
   - `get_orders_by_parent_id()` query ✓
   - **REMAINING** (see `01-data-model.md` §8 for full scope):
     - Add `reference_price: Option<f64>` field to `MarketBracketParams` in `bracket.rs`
       (required by §10 order size guard — update `sample_params()` test helper too)
     - `order_row_to_local()` / `local_to_order_row()` conversion functions (~100-150 lines)
       (named to avoid collision with existing `row_to_order` which maps `rusqlite::Row -> OrderRow`)
     - `persist_and_transition_to_pending_submit(group)` (~30 lines)
     - `transition_bracket_to_error(group, reason)` (~20 lines)
     - `transition_bracket_to_rejected(group, reason)` (~20 lines)
     - Unit tests for conversions and helpers (~30-50 lines)

**Remaining estimate**: ~250-300 lines (persistence conversion + helpers + tests)

**Verify**: `cargo test -p midas-broker` — all existing 180 unit tests pass + new tests pass.
(Note: 1 doc test in `testdata/mod.rs` is ignored — this is expected.)

### Phase 2: Broker Engine (3-4 files, ~500-600 lines remaining)

**Goal**: Engine handles CreateMarketBracket, builds orders, submits to IB.

1. ~~Modify `crates/midas-broker/src/commands.rs`~~ **DONE**
   - `CreateMarketBracket(MarketBracketParams)` ✓
   - `CancelBracket { parent_id: Uuid }` ✓
   - `ModifyBracketLeg { order_id: Uuid, new_price: f64 }` ✓

2. ~~Modify `crates/midas-broker/src/events.rs`~~ **DONE**
   - `BracketCreated { ... }` ✓
   - `BracketStatusChanged { ... }` (uses typed `BracketLifecycleStatus` enum) ✓

3. **REMAINING**: Engine infrastructure (~150-200 lines)
   - Pin `ibapi = "2.10"` in `crates/midas-broker/Cargo.toml`
   - Add `store` (BrokerDb handle), `client` (Arc\<ibapi::Client\>),
     `bracket_status_cache` (HashMap) fields to `BrokerEngine`
   - Wire fields through `start_broker_engine` initialization
   - Add `from_ib_status("PartiallyFilled")` mapping in `state.rs` ✓ (done in code, needs plan acknowledgment)
   - Extend `can_modify_at_ib()` for `PartiallyFilled` ✓ (done in code)

4. **REMAINING**: Bracket handlers (~200-250 lines)
   - `build_market_bracket()` — construct 3 linked LocalOrders from params
   - `validate_order_size()` — engine-level max quantity/notional guard
     (see `02-broker-engine.md` §10)
   - `handle_create_market_bracket()` — validate, build, persist, submit
   - `submit_bracket_to_ib()` — per-leg place_order with transmit flags
     (entire body in single `spawn_blocking` — see §0.5)
   - `check_bracket_status_change()` — post-processing on status callbacks
   - `handle_cancel_bracket()` — cancel all legs
   - `handle_modify_bracket_leg()` — modify single leg

5. **REMAINING**: Mock IB client test infrastructure
   - `MockIbClient` struct (with `orders_placed`, `next_id`, `callbacks`)
   - `PlacedOrder` and `MockCallback` types
   - One smoke test: `full_bracket_lifecycle_tp_hit`

6. **REMAINING**: Integration tests with mock IB client (14-19 tests)

**Remaining estimate**: ~150-200 lines infrastructure + ~200-250 lines handlers + ~350 lines (mock + tests)

**Verify**: `cargo test -p midas-broker` — all tests pass.

### Parallelization and Critical Path

> Updated 2026-04-02: Phase 4.0 (bridge) moved off the Phase 2 dependency.

**Critical path**:
```
Phase 1 remainder → Phase 2 ──────────────────────────→ Phase 5 → Phase 6
                 ╲                                     ╱
                  ├→ Phase 3 (can start now) ─────────╱
                  ╲                                  ╱
                   → Phase 4.0 → Phase 4a/4b ──────╱
```

**Phase 3 (chart rendering) can start immediately.** Its remaining work
(`leg_style()`, `bracket_labels()`, `bracket_zone_rects()`) is entirely in
`midas-chart` — a sans-IO crate with zero dependency on `midas-broker`. All
types Phase 3 needs (`BracketStatus`, `LegRole`, `OrderBracket`) are already
complete. Phase 3 does NOT depend on Phase 2 engine handlers.

**Phase 4.0 (bridge) can also start immediately.** It depends only on type
definitions (`MarketBracketParams`, `BracketLifecycleStatus`, command/event
variants) which are already complete in the codebase. It does NOT depend on
engine handlers (Phase 2).

**Net effect**: Three work streams can run in parallel after Phase 1 remainder
completes: Phase 2 (engine), Phase 3 (chart rendering), and Phase 4.0 (bridge).
Phase 5 (drag interaction) is the join point — it depends on all three.

**Phases 3, 4a, and 4b** touch different crates (`midas-chart` vs `midas-app`)
with no shared files. Merge conflicts are limited to `Message` enum variant
additions in `midas-app`, which are trivially resolvable (append-only).

**Phase 3+4 Integration Checkpoint**: Before starting Phase 5, merge all
branches, resolve any conflicts, run `cargo test --workspace`, and verify
bracket annotations render with the new P&L fields populated. Add a specific
test that verifies P&L labels render with non-None values.

### Phase 3: Chart Visualization — TYPES COMPLETE, RENDERING REMAINING

**Goal**: OrderBracket shows P&L, zone fills respond to status changes.

1. ~~Modify `desktop/win/crates/midas-chart/src/widget/order_bracket.rs`~~ **DONE**
   - `projected_pnl`, `projected_pnl_pct` on `BracketLeg` (with `#[serde(default)]`) ✓
   - `LegRole` enum (chart-local, not imported from broker) ✓
   - `dollar_risk()`, `dollar_reward()` on `OrderBracket` ✓
   - Unit tests (15 tests, including backward-compatibility) ✓

2. **REMAINING**: Rendering functions and status-driven styling
   - `leg_style()` method on `OrderBracket` — status-driven line style/width/color
   - `bracket_labels()` function — overlay labels with P&L badges
   - `bracket_zone_rects()` function — TP/SL zone fill rectangles
   - Color constants, label formatting helpers (`format_entry_label`, etc.)
   - Pipeline integration (connecting to chart draw call)

**Remaining estimate**: ~120-160 lines

**Verify**: `cargo test --workspace` — all tests pass.

### Phase 4: Order Entry UI (5-6 files, ~800 lines) — NOT STARTED

**Goal**: Order panel widget in midas-app, functional submission flow.

**Sub-phase 4.0 — Cross-Workspace Bridge** (~150-200 lines):

> Updated 2026-04-02: Promoted from "prerequisite" to explicit sub-phase.
> **Can start immediately** — depends only on type definitions that are
> already complete, NOT on Phase 2 engine handlers.

`midas-app` lives in the desktop workspace (`desktop/win/Cargo.toml`) and
has no dependency on `midas-broker` (root workspace). The plan's bracket
types (`MarketBracketParams`, `BracketLifecycleStatus`, `BrokerCommand`,
`BrokerEvent`) live in `midas-broker`. To bridge the gap, extend the
existing pattern used by `OrderBroker` trait in desktop `midas-core`.

**Current state of the bridge**: The `OrderBroker` trait in
`desktop/win/crates/midas-core/src/provider.rs` currently has only
`name()`, `is_connected()`, and `connection_state()` methods — no order
submission methods at all. There is no existing mechanism for `midas-app`
to send `BrokerCommand`s to the broker engine. `ConnectionState` is already
mirrored in the desktop workspace but lacks a `MIRROR OF:` comment — add
one retroactively.

**Alternative evaluated and rejected**: Making `midas-broker` a dependency
of the desktop workspace was considered. Rejected because `midas-broker`
depends on `ibapi` (which pulls in TCP networking, IB protocol handling)
and the desktop workspace should not transitively depend on IB API details.
The clean boundary is: desktop workspace defines the *interface*
(`OrderBroker` trait), root workspace provides the *implementation*.

> **Future alternative**: A `midas-broker-types` crate (containing only
> the shared types — `MarketBracketParams`, `BracketLifecycleStatus`,
> `OrderAction`, etc. — with zero `ibapi` dependency) could live in the
> root workspace and be referenced by the desktop workspace as a path
> dependency. This would eliminate mirroring entirely.
> **Migration trigger**: If mirrored types exceed 10 structs/enums, create
> the shared `midas-broker-types` crate.

**Types to mirror** (audited against desktop `midas-core`):

| Type | Source | Already in desktop? | Action |
|------|--------|---------------------|--------|
| `SecurityType` | `midas-core` (shared) | Yes (shared crate) | None |
| `OrderAction` | `midas-broker::orders::types` | No | Mirror |
| `TimeInForce` | `midas-broker::orders::types` | No | Mirror |
| `MarketBracketParams` | `midas-broker::orders::bracket` | No | Mirror |
| `TakeProfitParams` | `midas-broker::orders::bracket` | No | Mirror |
| `StopLossParams` | `midas-broker::orders::bracket` | No | Mirror |
| `BracketLifecycleStatus` | `midas-broker::orders::bracket` | No | Mirror |
| `ConnectionState` | `midas-broker` | Yes (mirrored, no comment) | Add `MIRROR OF:` comment |

Total: 6 new mirrored types + 1 retroactive comment.

**Implementation steps**:

1. Add `MIRROR OF:` comment to existing `ConnectionState` in desktop `midas-core`.
2. Add bracket parameter types to `desktop/win/crates/midas-core/src/`
   (mirror of `MarketBracketParams`, `TakeProfitParams`, `StopLossParams`).
   Each mirrored type must have a doc comment cross-referencing its source:
   `// MIRROR OF: crates/midas-broker/src/orders/bracket.rs::MarketBracketParams`
3. Add `create_market_bracket()`, `cancel_bracket()`, `modify_bracket_leg()`
   methods to `OrderBroker` trait
4. Add `BracketCreated` / `BracketStatusChanged` variants to the desktop
   workspace's event types (or add a `BracketEvent` enum)
5. Implement the adapter in `midas-app` that translates desktop types to
   `midas-broker` types and forwards via the mpsc channel. Use exhaustive
   destructuring in the `From` impl so that adding a field to either side
   causes a compile error:
   ```rust
   // Exhaustive destructure — NO `..` rest pattern, so adding a field
   // to either side causes a compile error until both are updated.
   let DesktopMarketBracketParams {
       symbol, con_id, sec_type, exchange, currency,
       action, quantity, outside_rth,
       take_profit, stop_loss, strategy, tags,
   } = params;
   ```

**Done when**: `midas-app` can construct a `DesktopMarketBracketParams`,
call `broker.create_market_bracket(params)`, and receive `BracketCreated`
events back — verified by a compile-only test that exercises every `From`
conversion (ensuring exhaustive destructuring catches field drift in CI).

This sub-phase must complete before Sub-phase 4a can begin.

**Sub-phase 4a** (core panel):
1. Create `desktop/win/crates/midas-app/src/order_panel.rs`
   - `OrderPanelState` struct (with account type indicator)
   - `view()` function (iced layout with BUY/SELL, qty, absolute-price TP/SL)
   - `update()` handler for panel messages

2. Add Message variants to midas-app
   - All `OrderPanel*` variants

3. Wire up broker bridge
   - `OrderAnnotationLink` struct
   - Map `BracketCreated` → create annotation
   - Map `BracketStatusChanged` → update annotation (using typed enum matching)

**Sub-phase 4b** (enhanced input):
4. Add price resolution logic
   - `resolve_price()` function with tests
   - Offset and percentage input modes
   - Risk/reward calculator

5. Keyboard shortcuts

**Verify**: `cargo build --workspace`, manual UI testing.

### Phase 5: Drag Interaction (2-3 files, ~250 lines)

**Depends on**: Both Phase 3 and Phase 4 must be complete.

**Goal**: TP/SL lines draggable on chart, modifications sent to broker.

1. Add `DraggingBracketLeg` to interaction state machine
2. Add `ChartAction::DragBracketLeg` variant to ChartAction enum
   - **Why not reuse `DragLevel`?** Bracket legs require directional constraints
     (TP must stay above/below entry), zone-fill resizing during drag, R:R badge
     updates, and broker modification on release. Standard levels have none of
     these. The actions also route to different broker commands (`ModifyBracketLeg`
     vs no broker interaction for levels). If bracket legs are registered as
     annotations, `DragLevel` could be reused for the hit-test and drag
     mechanics, with the app layer distinguishing by annotation kind — evaluate
     during implementation. Budget ~50 extra lines if a parallel hit-test path
     is needed.
3. Handle mouse events for bracket leg hit zones
4. Enforce directional constraints during drag (TP above/below entry)
5. Price snap logic (using tick size from contract metadata)
6. Wire to `BrokerCommand::ModifyBracketLeg` in midas-app

**Done when**: TP line draggable on Active bracket, SL line draggable,
directional constraints enforced (TP stays above/below entry per side),
`ModifyBracketLeg` command emitted on mouse release. Manual scenarios M5
and M6 from the test table pass.

**Verify**: Manual testing with paper trading.

### Phase 6: Polish (1-2 files, ~100 lines)

**Goal**: Confirmation dialog and quality-of-life improvements for production use.

1. Confirmation dialog
2. Quick trade mode (if settings enabled)
3. Context menu on bracket legs
4. Toast notifications for bracket events
5. Historical bracket loading on startup

**Verify**: Manual testing of each polish item with paper trading.

---

## 6. Acceptance Criteria

### Must Have (Phase 1-3)

- [x] `BracketRole` enum with `Display`/`FromStr`/`Serialize`/`Deserialize`
- [x] `MarketBracketParams` validation passes/fails correctly
- [x] `BracketGroup` correctly derives bracket-level status
- [x] `derive_bracket_status()` never panics, even with unexpected states
- [x] `BracketCreated` and `BracketStatusChanged` event variants defined
- [x] `from_ib_status("PartiallyFilled")` mapped correctly
- [x] `can_modify_at_ib()` includes `PartiallyFilled`
- [x] `dollar_risk()` / `dollar_reward()` on `OrderBracket`
- [x] `LegRole` chart-local enum implemented
- [x] 76 bracket-related unit tests pass (42 bracket + 19 types + 15 chart)
- [ ] `order_row_to_local()` / `local_to_order_row()` persistence conversion
- [ ] `persist_and_transition_to_pending_submit()` atomic transaction
- [ ] `transition_bracket_to_error()` / `transition_bracket_to_rejected()`
- [ ] Engine creates 3 linked LocalOrders with correct roles and parent_id
- [ ] Engine-level order size guard (max quantity / max notional)
- [ ] IB submission uses correct `transmit` flag ordering
- [ ] `BracketCreated` and `BracketStatusChanged` events emitted at runtime
- [ ] IB order IDs allocated per-leg (no consecutive assumption)
- [ ] OrderBracket annotation created on chart after BracketCreated
- [ ] Bracket status visually updates on chart (Pending → Active → Closed)
- [ ] Zone fills appear between entry and TP/SL when Active
- [ ] P&L labels show correct dollar amounts
- [ ] All existing 180 unit tests continue to pass
- [ ] 15+ new integration tests pass

### Should Have (Phase 4-5)

- [ ] Order panel widget with BUY/SELL, quantity, TP, SL inputs
- [ ] Price input modes (absolute, offset, percentage)
- [ ] Risk/reward calculator updates in real-time
- [ ] TP/SL lines draggable on chart
- [ ] Drag sends ModifyBracketLeg to broker engine

### Nice to Have (Phase 6)

- [ ] Confirmation dialog before submission
- [ ] Quick trade mode via chart context menu
- [ ] Toast notifications for bracket events
- [ ] Historical brackets loaded on startup
- [ ] Context menu on bracket legs (cancel, modify)

---

## 7. Risks and Mitigations

| Risk | Impact | Likelihood | Mitigation |
|---|---|---|---|
| IB rejects bracket due to price constraints | Bracket fails to submit | Medium | Validate TP/SL against current market price before submission; show clear error |
| Market order gets partial fill | TP/SL don't activate | Low | Engine handles partial fills; bracket status stays "submitted" until full fill |
| Network disconnect during submission | Orphaned orders at IB | Low | Reconciliation on reconnect; `transmit=false` prevents partial bracket execution |
| Existing tests break from BracketRole change | Regression | Medium | `FromStr` accepts legacy strings ("PROFIT", "STOP"); database reads are backward-compatible |
| Performance: bracket status checks on every order update | Slow callback processing | Low | Status check is O(1) cache lookup + O(3) order load; negligible vs. IB latency |
| Race between TP fill and SL modification | SL modified after it should be cancelled | Low | IB parent-child auto-cancellation is atomic; our modification will fail gracefully if order is already cancelled |
| IB callbacks for force-transitioned orders | Runtime error in status handler | Medium | Terminal-state guard in `handle_order_status` (§5.1 in `02-broker-engine.md`) ignores callbacks for orders already in Rejected/Filled/Cancelled |
| Accidental live order during development | Real money at risk | Low | Port-based live-trading guard (`allow_live = true` required for port 4001), UI account type indicator (paper/live badge near submit button), IB Gateway paper account as default |
| Catastrophically large order (UI bug or automation) | Severe financial loss | Low | Engine-level order size guard (`max_order_quantity`, `max_notional_value`) as hard reject before IB submission (§10 in `02-broker-engine.md`) |
| `ibapi` crate API change on `cargo update` | Compilation or runtime failure | Low | Pin `ibapi = "2.10"` in `Cargo.toml`; plan verified against v2.10.0 source |
| Blocking IB API calls in async context | Tokio worker thread starvation | Medium | Wrap `place_order`/`cancel_order`/`next_order_id` in `tokio::task::spawn_blocking()`; bracket submission makes 3 sequential calls which amplifies blocking |
