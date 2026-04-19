# Stage 04 — Order Lifecycle Simulator

*The order state machine, synthetic fill model, bracket parent-child semantics, and — critically — the `execDetails`-before-`orderStatus` event ordering that real IB exhibits.*

**Depends on**: 02 (protocol, for `OrderSpec` / `Execution` types), 08 (clock)
**Blocks**: 06, 09
**Parallel-safe with**: 03, 05, 07

## Scope

For each `PLACE_ORDER` command, drive a realistic fill lifecycle:
- Honor order type semantics (Market, Limit, Stop, StopLimit)
- Fill against a moving market mid-price (from market data engine)
- Emit events in real-IB order: sometimes `ExecutionData` before `OrderStatus`, sometimes without an intermediate status
- Handle parent-child bracket OCA semantics with non-atomic ordering
- Emit `CommissionReport` independently and slightly after fills

## Public API

```rust
pub struct OrderSimulator {
    orders: BTreeMap<OrderId, OrderState>,
    brackets: BTreeMap<OrderId /* parent */, BracketGroup>,
    clock: Arc<dyn Clock>,
    scheduler: EventSchedulerHandle,
    rng: SmallRng, // deterministic per seed
}

impl OrderSimulator {
    pub fn place(&mut self, req: PlaceOrderReq) -> Vec<OrderEmission>;
    pub fn cancel(&mut self, order_id: OrderId) -> Vec<OrderEmission>;
    /// Called by the engine loop each time a MarketSnapshot arrives
    /// (projected from a MarketEmission by the engine). Evaluates all resting
    /// orders for fill eligibility and emits the resulting order events.
    pub fn on_market_snapshot(&mut self, snap: &MarketSnapshot) -> Vec<OrderEmission>;
    pub fn open_orders_snapshot(&self) -> Vec<OutgoingMsg>;
}

// OrderEmission is defined in crate::engine (Stage 01 §Central types).
// Re-declared here for reference.
pub enum OrderEmission {
    OpenOrder(OpenOrder),
    OrderStatus(OrderStatus),
    Execution(Execution),
    Commission(CommissionReport),
    Reject { order_id: OrderId, code: i32, message: String },
    Position(PositionUpdate),
    PortfolioValue(PortfolioValueUpdate),
    AcctValue(AcctValueUpdate),
    AcctDownloadEnd(String),
    PositionEnd,
}
```

## Order state machine

Follows IB's documented states + additions for bracket semantics:

```rust
pub enum OrderStatusCode {
    ApiPending,     // our internal staging before submission
    PendingSubmit,  // sent to "exchange" (sim)
    PreSubmitted,   // accepted, pre-activation (e.g., stop not yet triggered)
    Submitted,      // active at exchange (limit resting)
    Filled,
    PartiallyFilled, // internal — IB uses Filled with remaining > 0
    Cancelled,
    ApiCancelled,
    Inactive,       // bracket child waiting for parent fill
}
```

### Transition rules per order type

- **Market** → `PreSubmitted` → immediate fill at next tick's ask (buy) or bid (sell). Critical: **often no `Submitted` event is emitted** — this is the real-IB quirk we model.
- **Limit** → `PreSubmitted` → `Submitted` (resting) → fills when market crosses limit.
- **Stop** → `PreSubmitted` (trigger not hit) → `Submitted` when triggered → immediate market-like fill.
- **StopLimit** → `PreSubmitted` (trigger not hit) → `Submitted` (limit posted) → fills when market crosses.

### Non-atomic event ordering (T1 quirk)

Real IB intersperses callbacks. Our sim models three canonical patterns as **explicit scripted sequences** — event order is a property of the sequence, not an outcome of scheduler jitter. Jitter only controls the *magnitude* of inter-event gaps; it can never reorder events.

**Pattern A — clean path** (40% of fills, for limit orders that rest and eventually fill):
```
OpenOrder(Submitted) → [market crosses] → OrderStatus(Filled) → ExecutionData → CommissionReport
```

**Pattern B — fast-market fills** (50% of market orders, 20% of limits):
```
OpenOrder(PreSubmitted) → ExecutionData → CommissionReport → OrderStatus(Filled)
```
`execDetails` arrives before any `OrderStatus`. This is documented IB behavior.

**Pattern C — partial fill with drift** (10% of orders, mostly large market orders):
```
OpenOrder(PreSubmitted) → ExecutionData(50 shares) → CommissionReport → OrderStatus(Filled, remaining=50) → ExecutionData(50 shares) → CommissionReport → OrderStatus(Filled, remaining=0)
```

The pattern is chosen by `hash(order_id) mod 100` against the distribution — deterministic per seed, but looks random across orders.

### Scheduled sequences (how the ordering is actually produced)

When a fill fires, the simulator emits the entire pattern into the scheduler as a **sequence of (relative_offset, event)** tuples where the offsets are strictly monotonic:

```rust
pub struct FillPattern {
    pub kind: PatternKind,
    pub steps: Vec<(Duration, EngineAction)>, // (offset_from_trigger, action)
}

// Pattern B, example:
fn pattern_b(order: &Order, fill: Fill) -> FillPattern {
    FillPattern {
        kind: PatternKind::FastMarket,
        steps: vec![
            (Duration::from_millis(0),   emit_open_order(order, PreSubmitted)),
            (Duration::from_millis(80),  emit_execution_data(order, fill)),
            (Duration::from_millis(140), emit_commission_report(fill)),
            (Duration::from_millis(210), emit_order_status(order, Filled)),
        ],
    }
}
```

The offsets above (80/140/210 ms) are **base offsets** deterministic per pattern. Jitter is applied as a per-step *additive perturbation*:

```rust
// Canonical jitter seed — used consistently across all pattern steps.
// The step_idx must be part of the seed so each step gets an independent draw;
// without it, all four steps of a pattern would draw the same jitter magnitude
// and the per-step gap analysis would be vacuous.
let jitter_magnitude = truncated_exp(
    mean = 15ms,
    max  = PATTERN_MAX_JITTER_MS,  // compile-time constant; runtime check below
    rng  = seeded_rng(hash(order_id, step_idx))
);
actual_offset(step_idx) = base_offset(step_idx) + jitter_magnitude;
```

**Seed discipline (authoritative)**: the jitter RNG stream is seeded from `hash(order_id, step_idx)` everywhere — the pattern-ordering analysis depends on it. This supersedes the earlier mention of `hash(event_type, order_id)` in the Determinism Guarantees section below; the canonical seed is `(order_id, step_idx)`. Fill-pattern event sequence is the only place that needs per-step independence; all other per-order draws (e.g., slippage amount, commission variance) use `(order_id, draw_kind)` where `draw_kind` is a named enum variant, never `event_type` as a string.

**Critical invariant**: `PATTERN_MAX_JITTER_MS < min(base_offset_gap_between_consecutive_steps)`. Base offsets are spaced so even with max jitter applied, consecutive steps cannot reorder. For Pattern B above: gaps are 80ms, 60ms, 70ms; `PATTERN_MAX_JITTER_MS = 50ms`; worst case consecutive pair becomes (80+50) vs (60+0) = 130 vs 60 — still ordered.

**Two-layer enforcement**:

1. **Compile-time**: `const_assert!` (via the `static_assertions` crate) pins the invariant on the declared base offset tables + the declared max jitter constant:
   ```rust
   const_assert!(PATTERN_B_MIN_GAP_MS > PATTERN_MAX_JITTER_MS);
   const_assert!(PATTERN_C_MIN_GAP_MS > PATTERN_MAX_JITTER_MS);
   ```
   Catches any new pattern whose declared base offsets are spaced too tightly.
2. **Runtime**: the jitter sampler itself has a `debug_assert!(jitter <= PATTERN_MAX_JITTER_MS)` — belt-and-braces guard for the case where `PATTERN_MAX_JITTER_MS` is ever made runtime-configurable (via scenario YAML, for instance). The const-assert becomes vacuous then; the runtime assert becomes the actual safety net.
3. **Property test** enumerates `(pattern, step, jitter ∈ {0, mean, max})^4` and asserts strict ordering of `actual_offset` across the 3^4 = 81 permutations per pattern.

**Scheduler sub-millisecond ordering**: when jitter compresses an inter-event gap below 1ms (rare but possible — two steps at 130.1ms and 130.2ms), the scheduler's monotonic `seq` tie-breaker (see 08-deterministic-clock.md §Determinism invariant) preserves declared order. Events scheduled in sequence (lower `seq` first) always dispatch in `seq` order at equal deadlines.

**Rollback signal**: if the per-pattern invariant `PATTERN_MAX_JITTER_MS < min(gap)` ever fails — either at compile time via const-assert or at runtime via debug-assert — Pattern B becomes non-deterministic in its ordering. Either widen the base gaps for that pattern or narrow the max jitter; do not ship with the invariant violated.

## Fill model

**Default: pessimistic**. Don't optimize-fill against the last tick; require the market to actually cross.

```rust
fn maybe_fill(&mut self, order: &mut OrderState, mid: f64, bid: f64, ask: f64) -> Option<Fill> {
    let fill_price = match order.kind {
        OrderKind::Market => match order.side {
            Side::Buy => ask,  // cross the spread
            Side::Sell => bid,
        },
        OrderKind::Limit => match order.side {
            Side::Buy if ask <= order.limit_price => order.limit_price.min(ask), // buy: market went to us or better
            Side::Sell if bid >= order.limit_price => order.limit_price.max(bid),
            _ => return None,
        },
        OrderKind::Stop => {
            if !order.stop_triggered {
                if triggered(order, mid) {
                    order.stop_triggered = true;
                    order.status = OrderStatusCode::Submitted;
                    // fall through to market-like fill
                } else { return None; }
            }
            match order.side { Side::Buy => ask, Side::Sell => bid }
        }
        OrderKind::StopLimit => {
            if !order.stop_triggered {
                if triggered(order, mid) { order.stop_triggered = true; }
                else { return None; }
            }
            // Now behaves like Limit
            match order.side {
                Side::Buy if ask <= order.limit_price => order.limit_price.min(ask),
                Side::Sell if bid >= order.limit_price => order.limit_price.max(bid),
                _ => return None,
            }
        }
    };
    Some(Fill { price: fill_price, shares: order.remaining_qty, ts: self.clock.now() })
}

fn triggered(order: &OrderState, mid: f64) -> bool {
    match order.side {
        Side::Buy => mid >= order.stop_price,    // buy stop: trigger when mid ≥ stop
        Side::Sell => mid <= order.stop_price,   // sell stop: trigger when mid ≤ stop
    }
}
```

### Partial fills

For orders with `quantity > partial_threshold` (configurable, default 1000 shares), split into N fills with random sizes summing to total. Emit each fill as a separate `ExecutionData` + `CommissionReport`, interleaved with `OrderStatus` updates that show `filled += chunk, remaining -= chunk`.

### Slippage

Configurable per scenario:

```rust
pub struct SlippageModel {
    pub kind: SlippageKind, // None | FixedBps(f64) | VolAware | Random(f64)
}
```

Default: 1 basis point for market orders, zero for limit orders that fill at the limit.

## Bracket semantics

```rust
pub struct BracketGroup {
    pub parent_id: OrderId,
    pub tp_id: Option<OrderId>,        // take profit (limit)
    pub sl_id: Option<OrderId>,        // stop loss (stop or stop-limit)
    pub state: BracketLifecycle,
}

pub enum BracketLifecycle {
    ParentWorking,       // parent Submitted, children Inactive
    ParentFilled,        // parent done, children now Submitted
    OneChildFilled,      // OCA cancelling the other
    Complete,            // all legs terminal
}
```

Critical quirks modeled:

1. **Children start as `Inactive`** while parent is working.
2. **After parent `Filled`**, children transition to `Submitted` — but with **scheduler jitter** so they arrive 5–50ms after the parent's `OrderStatus(Filled)`. This is the non-atomic ordering real IB exhibits.
3. **OCA on child fill**: when one child fills, emit `OrderStatus(Cancelled)` on the other with `reason="OCA group cancelled by sibling fill"` — jittered 10–100ms after the filling sibling's `Filled` event.

## Rejection paths

Modeled reject codes (from [research/ib-quirks-and-limits.md](research/ib-quirks-and-limits.md)):

| Code | Trigger |
|------|---------|
| 103 | Duplicate orderId (sim rejects if it's seen this ID this session) |
| 104 | Modify attempted on a filled order |
| 110 | Limit price doesn't conform to min tick (contract's tick size from `ContractSpec`) |
| 200 | No security definition (sim has no contract for the symbol) |
| 201 | Order rejected — triggered by scenario script |
| 202 | Cancelled — normal |
| 10147 | Cancel arrived before place confirmation |

`OrderValidationFailed` (our project-local) → `ErrMsg` code in the 1000+ range.

## Request-position/account events

Handled here since they're tied to fills:

- **On fill**: emit `Position` update + `PortfolioValue` update (if account subscription active)
- **`REQ_POSITIONS`**: emit full snapshot of open positions then `PositionEnd`
- **`REQ_ACCOUNT_DATA`**: stream `AcctValue` + `PortfolioValue` + `AcctDownloadEnd`

Account tracking state:

```rust
pub struct AccountState {
    pub cash: f64,
    pub equity: f64,
    pub positions: BTreeMap<SymbolKey, Position>,
    pub realized_pnl: f64,
    pub unrealized_pnl: f64,
}
```

P&L marks-to-market from `mid_price` of subscribed symbols.

## Parallelism within this stage

| Sub-team | Scope | LOC |
|----------|-------|-----|
| **A** | State machine + transition tests | ~500 |
| **B** | Fill model + slippage + partial fills | ~400 |
| **C** | Bracket OCA lifecycle + jitter | ~350 |
| **D** | Account/position state + snapshot emission | ~300 |

Merge order: A first (types), then B/C/D in parallel, then integration tests.

## Determinism guarantees

- Every RNG draw is seeded from `hash(order_id, seed)` — two sims with the same seed + same order IDs produce identical event sequences.
- Scheduler jitter draws use a separate RNG stream seeded from `hash(event_type, order_id)` — scheduling determinism is independent of fill determinism.
- No `HashMap` iteration affects output order — use `BTreeMap` everywhere state is enumerated.

## Rollback signals

- Tests assert specific event orderings and start failing flaky → non-determinism crept in; find the `HashMap::iter()` or `tokio::join!` of ordered work.
- Fill prices drift far from mid → scope creep in slippage model; revert to fixed-bps.
- Bracket children activate before parent emits `Filled` → ordering invariant broken; find the path that synchronously emits both.

## Kill criteria

- **Can't reproduce Pattern B (execDetails-before-OrderStatus) after 1 week** → event-ordering jitter harness is wrong; rip out jitter and model as explicit scripted sequences in scenarios instead.
- **Determinism regressions across test runs** → audit for `HashMap`, `SystemTime::now()`, `rand::thread_rng()` leaks.

## Deliverables

- Order lifecycle E2E test: `rust-ibapi` client places bracket order, fills happen at scripted mid-price, client sees realistic event sequence including Pattern B for at least one market-order test case.
- Determinism test: same seed + same commands → byte-identical event stream, 3 runs.
- `cargo bench -p midas-ib-sim orders` — 1000 orders placed, filled, and closed in < 100ms virtual time.
