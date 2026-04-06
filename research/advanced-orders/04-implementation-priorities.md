# 04 -- Implementation Priorities

> Phased implementation plan for advanced order types in Hand of Midas.
>
> Each feature includes: IB API fields/methods needed, chart UI changes required,
> and architectural considerations.

---

## Phase 1: Core Order Types

**Goal**: Every trading platform must have these. Without them, Hand of Midas cannot be used for real trading beyond market orders.

**Estimated scope**: 6-8 focused implementation sprints.

---

### 1.1 Order Type Selector in Order Panel

**What**: Add a dropdown/radio group to the order panel allowing the user to select Market, Limit, Stop, or Stop-Limit as the entry order type.

**IB API fields**:
- `order.orderType` = `"MKT"` | `"LMT"` | `"STP"` | `"STP LMT"`
- `order.lmtPrice` (for Limit and Stop-Limit)
- `order.auxPrice` (for Stop and Stop-Limit -- this is the stop trigger price)

**Chart UI changes**:
- `OrderPanelState` needs an `order_type: OrderKind` field (currently hardcoded to Market)
- Conditionally show/hide price inputs: Limit shows limit price, Stop shows stop price, Stop-Limit shows both
- Panel validation must enforce non-zero prices for non-Market types

**Architectural considerations**:
- The `MarketBracketParams` struct must be generalized. Either rename to `BracketParams` with an `entry_type: OrderKind` field and optional `entry_limit_price` / `entry_stop_price`, or create parallel `LimitBracketParams` / `StopBracketParams` structs. The first approach (single generalized struct) is recommended to avoid combinatorial explosion.
- `validate_market_bracket()` becomes `validate_bracket()` with type-aware rules.
- The directional validation (`check_bracket_direction`) needs a reference price strategy: for Limit orders, reference_price is the limit price itself (not last traded price); for Stop orders, it is the stop trigger price.

---

### 1.2 Limit Order Entry (Standalone + Bracket)

**What**: Place a limit order at a specific price, optionally with TP and SL children forming a bracket.

**IB API fields**:
- Parent: `orderType = "LMT"`, `lmtPrice`, `action`, `totalQuantity`
- Children: same as market bracket (`parentId`, OCA group, `transmit` chain)

**Chart UI changes**:
- New chart annotation type or mode: a limit order shows a horizontal line at the limit price (distinct from market entry which uses "current price")
- The entry line for a limit bracket must be **draggable** on the chart (unlike market brackets where entry is the current price)
- Line style: dashed line with price label, colored by side (green=buy limit, red=sell limit)
- Draft limit brackets need the limit price as the reference for TP/SL offset calculations

**Architectural considerations**:
- Chart interaction state machine (`interaction/mod.rs`) needs a new `LimitPlacing` mode where the first click sets the entry price (not auto-snapped to current price)
- `BracketLeg.price` for entry is user-specified, not auto-populated from candle data
- `ChartAction::CreateBracket` needs an `entry_type: OrderKind` field
- The `create_bracket_annotation()` bridge function needs to accept an order type parameter

---

### 1.3 Stop Order Entry

**What**: Place a stop order that triggers a market order when the stop price is hit.

**IB API fields**:
- `orderType = "STP"`, `auxPrice` = stop trigger price
- Children: same bracket mechanics

**Chart UI changes**:
- Stop entry line has a distinct visual treatment from limit: suggested style is a dotted line (vs dashed for limit) to indicate it is a trigger, not a resting order
- Color convention: buy stop above current price (bullish breakout), sell stop below (bearish breakdown)
- Entry line is draggable, same as limit

**Architectural considerations**:
- Stop orders have opposite directional rules from limit orders: a buy stop must be ABOVE current price, a sell stop must be BELOW. The directional validation system needs type-aware logic.
- For brackets with stop entry: TP is still on the profitable side of the stop price, SL is on the losing side.

---

### 1.4 Stop-Limit SL Type Exposed in UI

**What**: Allow the user to choose Stop vs Stop-Limit for the stop-loss leg of any bracket.

**IB API fields**:
- SL child: `orderType = "STP LMT"`, `auxPrice` = stop trigger, `lmtPrice` = limit price after trigger
- The `StopLossParams.limit_price` field already exists in the broker crate

**Chart UI changes**:
- `StopLossType` enum (`Stop` / `StopLimit`) already exists in the order panel but is `#[allow(dead_code)]`
- Add a dropdown or toggle in the SL section of the order panel
- When StopLimit is selected, show a second price input for the limit price
- On chart: show a second thin line below the SL trigger line indicating the limit floor

**Architectural considerations**:
- The `StopLossParams` struct already supports `limit_price: Option<f64>` -- just needs UI wiring
- Validation: limit price must be on the same side as the stop price but closer to entry (for buy SL: stop below entry, limit further below stop; for sell SL: stop above entry, limit further above stop)

---

### 1.5 TP/SL Offset and Percentage Input Modes

**What**: Allow specifying TP and SL as dollar offsets ($6.50) or percentages (3.5%) from the entry price, not just absolute prices.

**IB API fields**: None -- this is purely a client-side calculation. The resolved absolute price is sent to IB.

**Chart UI changes**:
- `PriceInputMode::Offset` and `PriceInputMode::Percent` already exist but are `#[allow(dead_code)]`
- Add a mode selector (tabs or dropdown) next to each TP/SL price input
- `resolve_price()` already handles all three modes correctly
- Chart bracket annotation prices are always absolute (after resolution)

**Architectural considerations**:
- When the user drags a TP/SL line on the chart, the panel input should update to reflect the new value in whichever mode is active (reverse resolution)
- Need a `reverse_resolve_price()` function: given absolute price, entry price, side, and is_tp, compute the offset or percentage value

---

### 1.6 Live Order Modification

**What**: Modify price and quantity of working orders at IB. This is the critical bridge between the draft UI and real trading.

**IB API fields**:
- Call `placeOrder()` with the **same** `orderId` and updated `lmtPrice` / `auxPrice` / `totalQuantity`
- Same `clientId` that placed the original order
- IB recommends limiting modifications to price, quantity, and TIF

**Chart UI changes**:
- `DragBracketLeg` action currently only updates draft brackets. For `BracketStatus::Active` brackets, the drag must trigger a modification request to the broker engine.
- Need a visual "pending modification" state (e.g., the line blinks or shows a spinner icon while the modification request is in flight)
- Optimistic UI: update the line position immediately, revert if IB rejects

**Architectural considerations**:
- The state machine already allows modification from `PreSubmitted`, `Submitted`, and `PartiallyFilled` states (`can_modify_at_ib()` returns true)
- Need a `ModifyOrder` command in the broker engine channel
- The `OrderAnnotationLink` maps annotation IDs to broker order UUIDs, enabling the chart-to-broker bridge
- Race conditions: if a fill arrives while a modification is in flight, the modification may be rejected. The app layer must handle this gracefully.
- Debouncing: during rapid drag, only send modification requests at most every ~200ms to avoid overwhelming IB's rate limits

---

### 1.7 Live Order Cancellation

**What**: Cancel working orders at IB from the chart or order panel.

**IB API fields**:
- `cancelOrder(orderId)` -- single order
- For bracket: cancel the parent cancels all children (IB propagates)

**Chart UI changes**:
- Add a [Cancel] button on Active/Pending bracket entry lines (similar to [X] on Draft)
- Right-click context menu with "Cancel Order" / "Cancel Bracket" options
- Visual transition: line animates to `Cancelled` status (dimmed, 0.20 alpha)

**Architectural considerations**:
- Cancelling a bracket parent should cancel all children. IB handles this via `parentId` propagation.
- Cancelling a single child (e.g., removing SL from an active bracket) is possible but dangerous -- the position becomes unprotected. Require confirmation dialog.
- State machine: `PendingCancel` intermediate state handles the async gap between cancel request and IB confirmation.

---

### 1.8 Right-Click Context Menu on Bracket Legs

**What**: The `RightClickBracketLeg` action is already emitted by the interaction state machine but no context menu is rendered.

**IB API fields**: None (pure UI).

**Chart UI changes**:
- Render a floating context menu at the right-click position
- Menu items vary by bracket status and leg role:
  - **Draft entry**: Modify Price, Submit, Save, Cancel
  - **Draft TP/SL**: Modify Price, Remove Leg
  - **Active entry**: Cancel Bracket, Flatten Position
  - **Active TP**: Modify TP Price, Remove TP
  - **Active SL**: Modify SL Price, Remove SL, Change to Trailing Stop

**Architectural considerations**:
- Context menu is an iced overlay, not a chart-crate concern. The chart emits the action; the app layer renders the menu.
- Menu state lives in `midas-app`, not `midas-chart` (sans-IO boundary).

---

### 1.9 Chart Visualization for Limit and Stop Orders

**What**: Standalone limit and stop orders (not just bracket children) need chart representation.

**IB API fields**: N/A (visualization only).

**Chart UI changes**:
- A standalone limit buy at $150 shows as a green dashed line at $150 with label "LMT BUY @ 150.00"
- A standalone sell stop at $145 shows as a red dotted line at $145 with label "STP SELL @ 145.00"
- These are separate from bracket annotations -- need a new `StandaloneOrder` annotation type or extend `OrderBracket` to handle no-children cases
- Draggable for modification (same as bracket leg dragging)
- Right-click for context menu

**Architectural considerations**:
- Consider whether standalone orders and bracket orders share the same annotation type or are separate. Recommendation: use `OrderBracket` with `take_profit: None, stop_loss: None` as a standalone order. This avoids a second annotation type and reuses all existing rendering/interaction code.

---

## Phase 2: Professional Features

**Goal**: Features that serious, active traders expect. These differentiate Hand of Midas from a toy.

**Estimated scope**: 8-12 sprints after Phase 1.

---

### 2.1 Trailing Stop as Bracket SL Child

**What**: Instead of a fixed stop price, the SL leg trails the market price by a fixed dollar amount or percentage.

**IB API fields**:
- Child: `orderType = "TRAIL"`, `auxPrice` = trailing amount (dollars), OR `trailingPercent` = trailing percentage
- These fields are mutually exclusive -- send one or the other
- `parentId` links to the entry order as with any bracket child

**Chart UI changes**:
- The SL line moves dynamically as the price moves in the favorable direction
- Need a new line rendering mode: the line animates to follow the calculated trailing stop level
- Label shows "TSL $2.50" or "TSL 1.5%" plus the current calculated stop price
- Trail amount/percent input fields in the order panel (when trailing SL is selected)

**Architectural considerations**:
- The trailing stop level is computed server-side by IB. The client receives updates via `orderStatus` callbacks with the current stop price.
- Between callbacks, the client can compute an approximate trailing level locally for smooth animation: `trailing_stop = max_favorable_price - trail_amount`
- `StopLossParams` needs a `trailing_amount: Option<f64>` and `trailing_percent: Option<f64>` alongside the existing `stop_price`
- The `OrderKind::TrailingStop` enum variant already exists

---

### 2.2 Limit Bracket (Limit Entry + TP + SL)

**What**: Full bracket with a limit entry order. This is TradingView's default bracket when placing an order away from the current price.

**IB API fields**:
- Parent: `orderType = "LMT"`, `lmtPrice`, `transmit = False`
- TP child: `orderType = "LMT"`, `lmtPrice`, `parentId`, `transmit = False`
- SL child: `orderType = "STP"`, `auxPrice`, `parentId`, `transmit = True`
- Children go `PreSubmitted` immediately but only activate when parent fills

**Chart UI changes**:
- Entry line at the limit price (draggable)
- TP and SL lines relative to the limit price (not current price)
- While parent is `PreSubmitted` (waiting to fill), entry line shows "LMT BUY @ 150.00 -- Waiting"
- Zone fills only appear after parent fills (transition to Active)

**Architectural considerations**:
- This is mostly a combination of Phase 1 features (1.1 + 1.2 + existing bracket pipeline)
- The key new element is the chart interaction for placing all three levels when entry is not at current price
- Bracket status derivation (`derive_bracket_status`) already handles `PreSubmitted` parent correctly

---

### 2.3 OCA Grouping Wired to IB

**What**: The `oca_group` field on `LocalOrder` exists but is not sent to IB. Wire it.

**IB API fields**:
- `order.ocaGroup` = unique string identifying the group
- `order.ocaType` = 1 (CancelWithBlock -- safest, prevents overfill)

**Chart UI changes**: Minimal. OCA groups are implicit in brackets (TP and SL are already OCA-linked by `parentId`). For standalone OCA groups, need a way to visually link orders on the chart (e.g., a thin connecting line or shared highlight color).

**Architectural considerations**:
- For bracket orders, IB automatically creates an OCA group from `parentId` linkage. Explicit `ocaGroup` is needed only for custom multi-order grouping.
- `LocalOrder.oca_group` is `Option<String>` -- generate a UUID-based group name when creating brackets.
- `ocaType = 1` (CancelWithBlock) should be the default. Type 2/3 (reduce instead of cancel) are niche.

---

### 2.4 Adaptive Algo Support

**What**: IB's Adaptive algo improves execution quality. It is the most commonly used IB algo and suitable for retail orders.

**IB API fields**:
- `order.algoStrategy = "Adaptive"`
- `order.algoParams = [TagValue("adaptivePriority", "Patient")]`
- Priority levels: `"Urgent"`, `"Normal"`, `"Patient"`

**Chart UI changes**:
- Add an "Execution" section to the order panel with an "Adaptive" toggle
- When enabled, show a priority dropdown (Patient / Normal / Urgent)
- Chart label suffix: "ADAPTIVE" badge on the order line

**Architectural considerations**:
- `LocalOrder.algo_strategy` and `LocalOrder.algo_params` already exist
- The IB submission layer needs to map these fields to the IB Order object
- Adaptive algo works with Limit, Market, and Stop orders
- Patient = best price improvement but slowest fill; Urgent = fastest fill, least improvement

---

### 2.5 Dollar Amount and Percentage Position Sizing

**What**: Enter a position as "$5,000" or "5% of account" instead of "100 shares".

**IB API fields**: None -- purely client-side computation. `totalQuantity` is always sent as shares.

**Chart UI changes**:
- Add a quantity mode selector: Shares | Dollar Amount | % of Account
- Dollar mode: compute `quantity = dollar_amount / last_price`, round to nearest share (or lot)
- % of Account: requires account equity from the broker engine (`account_summary.equity * pct / 100 / last_price`)

**Architectural considerations**:
- Need account equity data flowing from the broker to the app layer. This is available via `reqAccountSummary` in the IB API.
- The quantity must be recalculated whenever the reference price changes (for dollar/% modes)
- Display the computed share count alongside the dollar/% input for confirmation

---

### 2.6 Risk-Based Position Sizing

**What**: "Risk 1% of account equity" -- automatically calculate position size from the SL distance.

**IB API fields**: None -- client-side computation.

**Chart UI changes**:
- Add a "Risk" mode to the quantity selector
- Inputs: risk amount ($ or % of equity), SL price (auto-populated from bracket SL)
- Computed quantity: `risk_amount / abs(entry_price - sl_price)`, rounded to valid lot size
- Real-time update: dragging SL on the chart recalculates quantity in the panel

**Architectural considerations**:
- This ties the quantity to the SL distance, creating a bidirectional dependency: moving SL changes quantity, changing quantity does NOT move SL
- Requires the same account equity data as 2.5
- The `RiskReward` struct already computes `risk_per_share` -- extend it to compute suggested quantity

---

### 2.7 Shift+Drag Entire Bracket

**What**: Holding Shift while dragging a bracket leg moves the entire bracket (entry + TP + SL) by the same delta.

**IB API fields**: None (chart interaction only). Modification requests sent for all three legs.

**Chart UI changes**:
- Detect Shift key during `DragBracketLeg` action
- Compute price delta from drag movement
- Apply delta to all three legs simultaneously
- Visual: all three lines move together during drag

**Architectural considerations**:
- The `ChartEvent::MouseMoved` already carries `alt_held`. Need to add `shift_held` to mouse events.
- `ChartAction::DragBracketLeg` needs a `shift_held: bool` field or a new `DragEntireBracket` action
- For live brackets, this triggers three simultaneous modification requests to IB

---

### 2.8 Cancel All Orders

**What**: Emergency button to cancel all working orders.

**IB API fields**: `reqGlobalCancel()` -- cancels ALL open orders across all clients.

**Chart UI changes**:
- Red "Cancel All" button in the order panel or toolbar
- Confirmation dialog: "This will cancel ALL open orders. Are you sure?"
- All order lines on all charts transition to Cancelled state

**Architectural considerations**:
- `reqGlobalCancel()` is a blunt instrument -- it cancels manual TWS orders too. Consider also implementing per-symbol cancel (filter `openOrders` by symbol).
- After cancel-all, the broker engine needs to reconcile all local order states.

---

### 2.9 Order Deactivation / Reactivation

**What**: Pull an order from the exchange without cancelling it (save parameters locally for later reactivation).

**IB API fields**: No direct "deactivate" API. Implementation: cancel at IB, cache parameters locally, re-submit when reactivating.

**Chart UI changes**:
- "Deactivate" button or context menu item on active orders
- Deactivated orders show as `Inactive` status (distinct styling -- very dim, dashed)
- "Activate" button on inactive orders to re-submit

**Architectural considerations**:
- The state machine already supports `can_deactivate()` (from `PreSubmitted`/`Submitted`) and `can_activate()` (from `Inactive`/`Error`)
- The `PendingCancel -> Inactive` transition is already allowed
- Key challenge: on reactivation, IB assigns a NEW `orderId`. The local `ib_order_id` must be updated while keeping the same local `id` (UUID).
- `activation_count` and `last_activated_at` / `last_deactivated_at` fields already exist for this purpose.

---

### 2.10 Primary Exchange Disambiguation

**What**: When trading stocks that are listed on multiple exchanges (e.g., AAPL on NASDAQ and other venues), specify the primary exchange to avoid ambiguous contract resolution.

**IB API fields**:
- `contract.primaryExchange = "NASDAQ"` (or "NYSE", "ARCA", etc.)

**Chart UI changes**: Minimal -- a secondary field in the order panel or symbol search.

**Architectural considerations**:
- `LocalOrder` needs a `primary_exchange: Option<String>` field
- This is primarily needed during contract qualification (`reqContractDetails`), not order placement itself

---

## Phase 3: Advanced / Power User Features

**Goal**: Algorithmic trading, conditional logic, and advanced IB-specific features. These are differentiators, not essentials.

**Estimated scope**: Ongoing, implement as demand warrants.

---

### 3.1 Conditional Orders

**What**: Orders that only submit (or cancel) when conditions are met: price of another instrument, time, volume.

**IB API fields**:
- `order.conditions` = list of condition objects
- `PriceCondition`: `conId`, `exchange`, `triggerMethod`, `price`, `isMore`
- `TimeCondition`: `time`, `isMore`
- `VolumeCondition`: `conId`, `exchange`, `volume`, `isMore`
- `isConjunctionConnection` (AND/OR chaining)
- `order.conditionsCancelOrder` (invert: cancel instead of submit)
- `order.conditionsIgnoreRth` (evaluate outside RTH)

**Chart UI changes**:
- New "Conditions" section in the order panel (collapsible)
- Visual indicator on chart lines: a condition badge (e.g., "IF SPY > 450" tag)
- Condition status feedback: "Condition not yet met" / "Condition met, order submitted"

**Architectural considerations**:
- `LocalOrder` needs a `conditions: Vec<OrderCondition>` field
- Define an `OrderCondition` enum in `midas-broker` matching IB's condition types
- The IB submission layer maps these to `ibapi` condition objects
- Complex UI: consider a separate "Condition Builder" dialog

---

### 3.2 TWAP / VWAP Algo Orders

**What**: Execute large orders over time to minimize market impact.

**IB API fields**:
- TWAP: `algoStrategy = "Twap"`, params: `strategyType`, `startTime`, `endTime`, `allowPastEndTime`
- VWAP: `algoStrategy = "Vwap"`, params: `maxPctVol`, `startTime`, `endTime`, `noTakeLiq`, `speedUp`

**Chart UI changes**:
- Algo configuration panel with time range picker and parameter inputs
- Chart: show the algo execution window as a shaded time region
- Progress bar or fill counter showing algo progress

**Architectural considerations**:
- Time fields need timezone handling (IB expects "US/Eastern" format)
- These algos make sense only for orders of significant size (hundreds to thousands of shares)

---

### 3.3 Adjustable Stops

**What**: Attach a one-time adjustment to a stop order: when a trigger price is hit, the stop parameters change (e.g., tighten the stop after the price moves favorably).

**IB API fields**:
- `order.adjustedOrderType` = new order type after trigger
- `order.triggerPrice` = price that triggers the adjustment
- `order.adjustedStopPrice` = new stop price after trigger
- `order.adjustedStopLimitPrice` = new limit price (if stop-limit)
- `order.adjustedTrailingAmount` = new trailing amount

**Chart UI changes**:
- On the SL line, a secondary "adjusted" line shows where the stop will move to after the trigger
- The trigger price shown as a thin marker line

**Architectural considerations**:
- This is an IB-specific feature with no TradingView equivalent
- Adds 4-5 new fields to `LocalOrder`

---

### 3.4 Time-in-Force Options in UI

**What**: Expose GTD, IOC, OPG in the order panel.

**IB API fields**:
- `order.tif` = `"GTD"` / `"IOC"` / `"OPG"`
- GTD requires `order.goodTillDate = "20260401 16:00:00 US/Eastern"`

**Chart UI changes**:
- TIF dropdown in the order panel (currently defaults to DAY, GTC available)
- GTD needs a date/time picker

**Architectural considerations**:
- `TimeInForce` enum already has all variants
- The main work is UI: date/time picker widget in iced

---

### 3.5 Market-on-Close / Limit-on-Close

**What**: Orders that execute at or near the closing price.

**IB API fields**:
- MOC: `orderType = "MOC"`
- LOC: `orderType = "LOC"`, `lmtPrice`
- Must be submitted before exchange cutoff (typically 3:50 PM ET for NYSE)

**Chart UI changes**:
- Add to order type selector
- Visual: order line with "MOC" or "LOC" badge and a time countdown to cutoff

**Architectural considerations**:
- Need to add `Moc` and `Loc` variants to `OrderKind`
- Cutoff time validation: warn if submitting close to the cutoff

---

### 3.6 Market-if-Touched / Limit-if-Touched

**What**: Orders that trigger when the market touches a specific price level, then execute as market or limit.

**IB API fields**:
- MIT: `orderType = "MIT"`, `auxPrice` = trigger price
- LIT: `orderType = "LIT"`, `auxPrice` = trigger price, `lmtPrice` = limit after trigger

**Chart UI changes**:
- Similar to stop orders but used for entering on pullbacks (buy MIT below current, sell MIT above)
- Dotted trigger line with label "MIT BUY @ 148.00"

**Architectural considerations**:
- Behaviorally similar to stop orders but with inverted directional logic (buy MIT is BELOW price, buy stop is ABOVE)
- Need to add `MarketIfTouched` and `LimitIfTouched` variants to `OrderKind`

---

### 3.7 GoodAfterTime Scheduling

**What**: Submit an order now but have it activate at a future time.

**IB API fields**:
- `order.goodAfterTime = "20260325 09:30:00 US/Eastern"`

**Chart UI changes**:
- "Activate at" time picker in the order panel
- Chart: order line with clock icon and countdown

**Architectural considerations**:
- `LocalOrder` needs a `good_after_time: Option<DateTime<Utc>>` field
- Must convert to IB's timezone format on submission

---

### 3.8 Trailing Stop-Limit

**What**: A trailing stop that converts to a limit order (not market) when triggered, preventing adverse fills in fast markets.

**IB API fields**:
- `orderType = "TRAIL LIMIT"`
- `trailingPercent` or `auxPrice` (trail amount)
- `lmtPrice` = initial limit offset
- The limit price trails in parallel with the stop price

**Chart UI changes**:
- Two animated lines: the trailing stop trigger and the trailing limit price
- Both move together as the price moves favorably

**Architectural considerations**:
- Need to add a `TrailingStopLimit` variant to `OrderKind`
- Two price fields needed: trail amount and limit offset

---

## Implementation Order Recommendation

The phases above are roughly ordered by priority, but within each phase the recommended implementation order is:

### Phase 1 -- Start Here

1. **1.1 Order Type Selector** + **1.2 Limit Order** (together -- the selector is useless without at least one new type)
2. **1.6 Live Order Modification** + **1.7 Live Order Cancellation** (together -- these are the IB connection bridge)
3. **1.5 TP/SL Input Modes** (quick win, mostly UI wiring of existing code)
4. **1.4 Stop-Limit SL Type** (quick win, field already exists)
5. **1.3 Stop Order Entry** (builds on 1.2 infrastructure)
6. **1.9 Chart Viz for Limit/Stop** (builds on 1.2 + 1.3)
7. **1.8 Right-Click Context Menu** (prerequisite for good UX on modification/cancel)

### Phase 2 -- After IB Paper Trading Works

1. **2.2 Limit Bracket** (combines Phase 1 features)
2. **2.1 Trailing Stop SL** (high user demand)
3. **2.4 Adaptive Algo** (low effort, high value)
4. **2.5 + 2.6 Position Sizing** (requires account data flow)
5. **2.3 OCA Grouping** (correctness feature)
6. **2.7 Shift+Drag** (UX polish)
7. **2.8 Cancel All** (safety feature)
8. **2.9 Deactivation** (leverages existing state machine)
9. **2.10 Primary Exchange** (correctness feature)

### Phase 3 -- Ongoing

Implement based on user demand. 3.1 (Conditional Orders) is the highest-value item. 3.4 (TIF options) and 3.5 (MOC/LOC) are relatively low effort.

---

## Cross-Cutting Concerns

### Broker Engine Channel Architecture

All Phase 1 features that interact with IB require commands flowing through the broker engine's split-channel architecture:

- **Order commands** (submit, modify, cancel): sent via the `broadcast(8192)` order channel
- **Order status updates**: received via the same channel, dispatched to the app layer
- **Market data** (for trailing stops, position sizing): via the `broadcast(4096)` market data channel
- **Account data** (for position sizing): via a new `watch` channel for account summaries

### Rate Limiting

IB imposes rate limits on order operations:
- Max ~50 order messages per second
- Modification requests should be debounced (especially during chart dragging)
- Implement a local rate limiter in the broker engine

### Error Handling

All order operations can fail. Common failure modes:
- IB rejects modification (order already filled)
- IB rejects order (insufficient margin, invalid price)
- Network timeout during submission
- IB Gateway disconnection mid-operation

The state machine's `Error` state and `Rejected` state handle these, but the UI needs to surface error messages clearly.

### Testing Strategy

- Unit tests: state machine transitions, validation logic, price resolution
- Integration tests: bracket construction, OCA grouping, modification flows
- Simulation tests: use the existing test broker (`MarketDataSource` trait) to verify end-to-end flows without IB connection
- Manual tests: IB paper trading (port 4002 / 7497)
