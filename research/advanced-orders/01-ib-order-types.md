# Interactive Brokers TWS API -- Order Types & Capabilities Reference

> Research compiled April 2026. Verify against current IB documentation before relying on specifics.
> Primary sources: TWS API v9.72+ GitHub docs, IBKR Campus, rust-ibapi v2.10 docs.

---

## Table of Contents

- [1. All Order Types](#1-all-order-types)
  - [1.1 Basic Order Types](#11-basic-order-types)
  - [1.2 Trailing Order Types](#12-trailing-order-types)
  - [1.3 Pegged Order Types](#13-pegged-order-types)
  - [1.4 Auction Order Types](#14-auction-order-types)
  - [1.5 Volatility & Options-Specific](#15-volatility--options-specific)
  - [1.6 Protected Order Types](#16-protected-order-types)
  - [1.7 Combo / Multi-Leg Order Types](#17-combo--multi-leg-order-types)
  - [1.8 IBKRATS Dark Pool Order Types](#18-ibkrats-dark-pool-order-types)
- [2. Order Attributes & Modifiers](#2-order-attributes--modifiers)
  - [2.1 Time in Force (TIF)](#21-time-in-force-tif)
  - [2.2 Display & Visibility Modifiers](#22-display--visibility-modifiers)
  - [2.3 Execution Modifiers](#23-execution-modifiers)
  - [2.4 Timing Modifiers](#24-timing-modifiers)
  - [2.5 Risk & Override Modifiers](#25-risk--override-modifiers)
- [3. Bracket Orders](#3-bracket-orders)
- [4. OCA (One-Cancels-All) Groups](#4-oca-one-cancels-all-groups)
- [5. Trailing Stop Mechanics](#5-trailing-stop-mechanics)
- [6. Adjustable Stops](#6-adjustable-stops)
- [7. Conditional Orders](#7-conditional-orders)
- [8. IB Algorithmic Orders](#8-ib-algorithmic-orders)
- [9. Exchange Routing](#9-exchange-routing)
- [10. Risk Controls & Order Limitations](#10-risk-controls--order-limitations)
- [11. Order Preview (What-If)](#11-order-preview-what-if)
- [12. Trigger Methods](#12-trigger-methods)
- [13. Delta Neutral / Hedge Orders](#13-delta-neutral--hedge-orders)
- [14. Scale Orders](#14-scale-orders)
- [15. rust-ibapi Crate Coverage](#15-rust-ibapi-crate-coverage)
- [16. Sources](#16-sources)

---

## 1. All Order Types

### 1.1 Basic Order Types

| `orderType` | Name | Required Fields | Description | Products |
|---|---|---|---|---|
| `MKT` | Market | `action`, `totalQuantity` | Executes immediately at best available price. No price protection. | All |
| `LMT` | Limit | + `lmtPrice` | Executes at specified price or better. | All |
| `STP` | Stop | + `auxPrice` | Becomes market order when stop price is reached. `auxPrice` = stop trigger price. | All |
| `STP LMT` | Stop Limit | + `auxPrice`, `lmtPrice` | Becomes limit order when stop price is reached. `auxPrice` = stop trigger, `lmtPrice` = limit. | All |
| `MIT` | Market-if-Touched | + `auxPrice` | Becomes market order when trigger price is touched. Buy triggers on decline, sell on rise. | STK, OPT, FUT, FOP, WAR |
| `LIT` | Limit-if-Touched | + `auxPrice`, `lmtPrice` | Becomes limit order when trigger price is touched. `auxPrice` = trigger, `lmtPrice` = limit. | STK, OPT, FUT, FOP, WAR |
| `MTL` | Market-to-Limit | `action`, `totalQuantity` | Executes at market; any unfilled remainder converts to limit at last fill price. | STK, OPT, FUT |
| `MOC` | Market-on-Close | `action`, `totalQuantity` | Executes near closing price in closing auction. Must be submitted before exchange cutoff. | STK, FUT |
| `LOC` | Limit-on-Close | + `lmtPrice` | Executes at close if closing price meets limit. Must be submitted before cutoff. | STK, FUT |
| `MIDPRICE` | Midprice | + `lmtPrice` (optional price cap) | Fills at current NBBO midpoint or better. US stocks/ETFs only. RTH only. Requires TWS 975+. | STK |
| `BOX TOP` | Box Top | `action`, `totalQuantity` | BOX exchange: market order, partial fill remainder converts to limit. | OPT (BOX) |

**Market-on-Open / Limit-on-Open**: These are not separate `orderType` values. Instead, combine `MKT` or `LMT` with `tif = "OPG"`.

### 1.2 Trailing Order Types

| `orderType` | Name | Required Fields | Description |
|---|---|---|---|
| `TRAIL` | Trailing Stop | `auxPrice` or `trailingPercent`, optionally `trailStopPrice` | Stop price trails the market by a fixed amount or percentage. Becomes market order when triggered. |
| `TRAIL LIMIT` | Trailing Stop Limit | `auxPrice` (trail amount), `trailStopPrice`, `lmtPriceOffset` | Like TRAIL but becomes limit order when triggered. `lmtPriceOffset` = distance from trail stop to limit. |

**Field details for trailing orders:**

| Field | Type | Description |
|---|---|---|
| `auxPrice` | double | The trailing amount in absolute terms (e.g., $2.00) |
| `trailingPercent` | double | The trailing amount as a percentage (e.g., 5.0 = 5%) |
| `trailStopPrice` | double | The initial stop price. For TRAIL LIMIT, this is the current stop level. |
| `lmtPriceOffset` | double | For TRAIL LIMIT only: offset from the stop price to determine the limit price. |

**Behavior**: Use `auxPrice` for absolute trailing OR `trailingPercent` for percentage-based. Do not set both. If `trailStopPrice` is set, it serves as the initial stop; thereafter, the trail amount governs adjustments.

### 1.3 Pegged Order Types

| `orderType` | Name | Required Fields | Description |
|---|---|---|---|
| `REL` | Relative / Pegged-to-Primary | `lmtPrice` (cap), `auxPrice` (offset) | Pegged to NBB (buy) or NBO (sell) with aggressive offset. Price cap via `lmtPrice`. |
| `PEG MKT` | Pegged-to-Market | `auxPrice` (offset) | Maintains fixed offset from NBB (buy) or NBO (sell). |
| `PEG MID` | Pegged-to-Midpoint | `auxPrice` (offset), `lmtPrice` (cap) | Tracks NBBO midpoint with offset. Price cap via `lmtPrice`. |
| `PEG BENCH` | Pegged-to-Benchmark | `startingPrice`, `peggedChangeAmount`, `referenceChangeAmount`, `referenceContractId`, `referenceExchange`, `stockRefPrice`, `stockRangeLower`, `stockRangeUpper`, `isPeggedChangeAmountDecrease` | Tracks a reference (benchmark) contract's price movements. Complex multi-field configuration. |
| `PEG STK` | Pegged-to-Stock | `delta`, `stockRefPrice`, `startingPrice` | Options only. Adjusts by delta x stock price change. |
| `PASSV REL` | Passive Relative | `auxPrice` (offset) | Less aggressive offset from best bid/ask. Similar to REL but passively positioned. |

**Relative/Pegged-to-Primary behavior (REL):**
- BUY: price pegged to NBB minus offset. If NBB rises, bid rises. If NBB falls, no adjustment (order becomes aggressive and may fill).
- SELL: price pegged to NBO plus offset. If NBO falls, offer falls. If NBO rises, no adjustment.

**IBKRATS-specific pegged types** (see section 1.8).

### 1.4 Auction Order Types

| `orderType` | Name | Required Fields | Description |
|---|---|---|---|
| `MTL` (with `tif="AUC"`) | Auction | `lmtPrice`, `tif="AUC"` | Pre-market auction execution at Calculated Opening Price (COP). Re-submits as limit if unfilled. |
| `LMT` (with `auctionStrategy`) | Auction Limit | `lmtPrice`, `auctionStrategy` | BOX exchange price improvement auction. |
| `PEG STK` (auction variant) | Auction Pegged-to-Stock | `delta`, `startingPrice` | BOX auction with stock-pegging adjustment. |
| `REL` (auction variant) | Auction Relative | `auxPrice` | BOX auction with relative offset. |

**`auctionStrategy` values (BOX exchange):**

| Value | Name | Description |
|---|---|---|
| 1 | `AuctionMatch` | Match the current auction price |
| 2 | `AuctionImprovement` | Improve upon the current auction price |
| 3 | `AuctionTransparent` | Transparent auction |

### 1.5 Volatility & Options-Specific

| `orderType` | Name | Required Fields | Description |
|---|---|---|---|
| `VOL` | Volatility | `volatility`, `volatilityType` | Options: limit price calculated from specified implied volatility. |

**Volatility order fields:**

| Field | Type | Values | Description |
|---|---|---|---|
| `volatility` | double | e.g., 0.40 = 40% | The implied volatility to use for pricing |
| `volatilityType` | int | 1 = Daily, 2 = Annual | Whether volatility is daily or annualized |
| `continuousUpdate` | int | 0 or 1 | If 1, TWS continuously updates limit price as underlying moves |
| `referencePriceType` | int | 1 = Average NBBO, 2 = NBB or NBO | How to compute the reference price |
| `stockRefPrice` | double | | Reference stock price for computation |
| `stockRangeLower` | double | | Cancel if underlying falls below this |
| `stockRangeUpper` | double | | Cancel if underlying rises above this |
| `delta` | double | | Delta value (BOX exchange only) |

### 1.6 Protected Order Types

| `orderType` | Name | Required Fields | Description |
|---|---|---|---|
| `MKT PRT` | Market with Protection | `action`, `totalQuantity` | Futures only. Market order with price protection range. |
| `STP PRT` | Stop with Protection | + `auxPrice` | Futures only. Stop that becomes protected market order when triggered. |

### 1.7 Combo / Multi-Leg Order Types

Combo orders use `secType = "BAG"` on the contract, with legs defined via `ComboLeg` objects.

| `orderType` | Name | Key Fields | Description |
|---|---|---|---|
| `LMT` (combo) | Combo Limit | `lmtPrice`, `smartComboRoutingParams` | Multi-leg combination with net limit price. |
| `MKT` (combo) | Combo Market | `smartComboRoutingParams` | Multi-leg at market. |
| `LMT` (per-leg) | Combo Limit per Leg | `orderComboLegs` | Individual limit price per leg. |
| `REL + LMT` | Relative + Limit Combo | `lmtPrice` | Combination of relative and limit strategy. |
| `REL + MKT` | Relative + Market Combo | | Combination of relative and market strategy. |

**`smartComboRoutingParams` key tags:**

| Tag | Value | Description |
|---|---|---|
| `NonGuaranteed` | `"0"` or `"1"` | `"1"` = non-guaranteed combo (legs may fill independently). `"0"` = guaranteed (all legs fill together or not at all). |

**ComboLeg fields:**

| Field | Type | Description |
|---|---|---|
| `conId` | int | Contract ID of the leg |
| `ratio` | int | Leg ratio |
| `action` | string | `"BUY"` or `"SELL"` for this leg |
| `exchange` | string | Exchange for this leg (or `"SMART"`) |
| `openClose` | int | 0 = Same, 1 = Open, 2 = Close |
| `shortSaleSlot` | int | Short sale designation |
| `designatedLocation` | string | Location for short sale |

### 1.8 IBKRATS Dark Pool Order Types

Orders routed to IBKR's Alternative Trading System (ATS). Requires `exchange = "IBKRATS"` and `notHeld = true`.

| Order Type | Key Parameters | Description |
|---|---|---|
| Pegged-to-Best | `minTradeQty`, `minCompeteSize`, `competeAgainstBestOffset` | Competes against best displayed price |
| Pegged-to-Best (offset) | + `midOffsetAtWhole`, `midOffsetAtHalf`, `competeAgainstBestOffset = COMPETE_AGAINST_BEST_OFFSET_UP_TO_MID` | With midpoint offset variants |
| Pegged-to-Midpoint | `minTradeQty`, `midOffsetAtWhole`, `midOffsetAtHalf` | Rests at NBBO midpoint with optional offsets |

---

## 2. Order Attributes & Modifiers

### 2.1 Time in Force (TIF)

Set via `order.tif` field.

| TIF Value | Name | Description | Notes |
|---|---|---|---|
| `DAY` | Day | Valid for current trading session only. | Default if not specified. |
| `GTC` | Good-Til-Canceled | Remains active until filled or manually canceled. | Auto-canceled under certain IB conditions (corporate actions, etc.). |
| `IOC` | Immediate-or-Cancel | Any portion not immediately fillable is canceled. | Partial fills allowed. |
| `FOK` | Fill-or-Kill | Entire order must fill immediately or entire order is canceled. | No partial fills. |
| `OPG` | At-the-Open | Executes in the opening auction. | Use with `MKT` for MOO, `LMT` for LOO. |
| `GTD` | Good-Til-Date | Active until specified date/time. | Set `order.goodTillDate = "yyyyMMdd HH:mm:ss timezone"`. |
| `DTC` | Day-Til-Canceled | Like DAY but deactivated at EOD instead of canceled. Order remains on TWS screen for re-transmission. | Order canceled at exchange but held locally in TWS. |
| `AUC` | Auction | For auction orders. | Used with `MTL` orderType for auction execution. |

**Additional timing fields:**

| Field | Type | Format | Description |
|---|---|---|---|
| `goodAfterTime` | string | `"yyyyMMdd HH:mm:ss timezone"` | Order activates after this time. |
| `goodTillDate` | string | `"yyyyMMdd HH:mm:ss timezone"` | Order expires at this time (requires `tif = "GTD"`). |
| `activeStartTime` | string | `"yyyyMMdd HH:mm:ss timezone"` | GTC orders: defines daily start of active window. |
| `activeStopTime` | string | `"yyyyMMdd HH:mm:ss timezone"` | GTC orders: defines daily end of active window. |

### 2.2 Display & Visibility Modifiers

| Field | Type | Description |
|---|---|---|
| `displaySize` | int | Iceberg/reserve: publicly displayed portion of total order size. Remainder hidden. |
| `hidden` | bool | If `true`, order is completely invisible in market depth. |
| `randomizeSize` | bool | Randomizes displayed order size. Volatility and Pegged-to-Volatility orders only. |
| `randomizePrice` | bool | Randomizes order price. Volatility and Pegged-to-Volatility orders only. |

**Iceberg behavior**: Set `displaySize` to a value smaller than `totalQuantity`. The exchange shows only `displaySize` shares at a time; when that tranche fills, the next `displaySize` tranche is displayed. The full order is never visible.

### 2.3 Execution Modifiers

| Field | Type | Description |
|---|---|---|
| `allOrNone` | bool | Entire order must fill on a single execution. Not all exchanges support this. |
| `minQty` | int | Minimum acceptable fill quantity per execution. |
| `sweepToFill` | bool | Limit order sweeps across multiple price levels to fill immediately. |
| `blockOrder` | bool | ISE block order. Minimum 50 contracts (options). |
| `outsideRth` | bool | If `true`, order can trigger/fill during pre-market and after-hours sessions. |
| `notHeld` | bool | Designates order as "not held" -- used for IBKRATS routing and broker discretion. |
| `discretionaryAmt` | double | Hidden additional amount off limit price the exchange may use to fill. |
| `percentOffset` | double | For relative orders: offset from NBBO as a percentage. |
| `imbalanceOnly` | bool | For imbalance-only open/close orders. |
| `routeMarketableToBbo` | bool | Routes marketable orders to Best Bid/Offer. |

### 2.4 Timing Modifiers

| Field | Type | Description |
|---|---|---|
| `goodAfterTime` | string | Delayed activation: order becomes active at specified time. |
| `goodTillDate` | string | Auto-expiration: order canceled at specified time (requires `tif = "GTD"`). |
| `activeStartTime` | string | Daily activation window start (GTC orders). |
| `activeStopTime` | string | Daily activation window end (GTC orders). |
| `autoCancelDate` | string | Date to automatically cancel the order. |

### 2.5 Risk & Override Modifiers

| Field | Type | Description |
|---|---|---|
| `overridePercentageConstraints` | bool | If `true`, bypasses TWS precautionary validation checks (price deviation, size limits). |
| `whatIf` | bool | If `true`, order is not placed -- returns estimated margin and commission impact instead. |
| `usePriceMgmtAlgo` | bool | If `true`, IB applies price management algo (price collar) to cap order price. Default is `true`. |
| `autoCancelParent` | bool | If `true`, cancels parent order when a child order is canceled. |

---

## 3. Bracket Orders

A bracket order = parent entry order + take-profit (limit) + stop-loss (stop), linked together.

### Structure

```
Parent Order (entry)
  |-- Child 1: Take-Profit (LMT, opposite side)    } OCA group
  |-- Child 2: Stop-Loss (STP, opposite side)       } (implicit)
```

### Linking Mechanism

| Field | Set On | Value | Purpose |
|---|---|---|---|
| `orderId` | Parent | N | Unique ID for parent |
| `orderId` | Child 1 (TP) | N+1 | Sequential ID |
| `orderId` | Child 2 (SL) | N+2 | Sequential ID |
| `parentId` | Child 1 | N | Links to parent |
| `parentId` | Child 2 | N | Links to parent |
| `transmit` | Parent | `false` | Holds until last child sent |
| `transmit` | Child 1 | `false` | Holds until last child sent |
| `transmit` | Child 2 | `true` | Triggers transmission of entire bracket |

### Key Behaviors

1. **`transmit` flag**: Setting `transmit = false` on parent and first child prevents them from going live until the last child sets `transmit = true`. TWS interprets this as the signal to transmit all siblings together.

2. **Child activation**: Children become active (submitted to exchange) only after the parent fills. Until then, they are held in `PreSubmitted` state.

3. **Implicit OCA group**: The two children (TP and SL) form an automatic OCA group. When one fills, the other is canceled.

4. **Partial fills on parent**: If the parent partially fills, the children's quantities are **not** automatically reduced. They remain at original size. This means you could have a 100-share bracket where parent fills 50 shares but TP/SL are still for 100 shares.

5. **Modifying legs**: Call `placeOrder()` with the same `orderId` of the specific leg you want to change. Other legs remain untouched.

6. **Sequential order IDs**: Parent, TP, and SL must use consecutive order IDs.

### Bracket with Trailing Stop

Replace the STP child with a TRAIL child:

```
Parent: LMT BUY 100 @ 150.00 (orderId=N, transmit=false)
TP:     LMT SELL 100 @ 160.00 (orderId=N+1, parentId=N, transmit=false)
SL:     TRAIL SELL 100, auxPrice=3.00 (orderId=N+2, parentId=N, transmit=true)
```

### rust-ibapi Bracket Builder

The `ibapi` crate (v2.10) provides `BracketOrderBuilder` and `BracketOrderIds` types for constructing bracket orders using a fluent API.

---

## 4. OCA (One-Cancels-All) Groups

OCA groups link multiple orders so that filling one affects the others.

### Configuration Fields

| Field | Type | Description |
|---|---|---|
| `ocaGroup` | string | Unique group identifier string (e.g., `"MyOCA_123"`) |
| `ocaType` | int | Determines behavior when one order fills (see below) |

### OCA Type Values

| `ocaType` | Name | Behavior | Overfill Protection |
|---|---|---|---|
| 1 | Cancel with Block | When one fills, all others are canceled. Only one order is routed to exchange at a time. | Yes (blocking) |
| 2 | Reduce with Block | When one fills, others' quantities are proportionally reduced. One routed at a time. | Yes (blocking) |
| 3 | Reduce without Block | Same as 2 but multiple orders can be live simultaneously. | No (overfill risk) |

### Behavior on Partial Fill

- **Type 1 (CancelWithBlock)**: Partial fill of one order cancels all remaining orders in the group.
- **Type 2 (ReduceWithBlock)**: Partial fill causes remaining orders to have their quantities reduced proportionally. The group "re-balances."
- **Type 3 (ReduceWithoutBlock)**: Same proportional reduction as Type 2, but without blocking -- multiple orders may be live on exchanges simultaneously, creating potential overfill risk.

### OCA vs Bracket

Bracket orders use `parentId` to create a parent-child hierarchy. The children implicitly form an OCA group. Standalone OCA groups use `ocaGroup` and `ocaType` fields directly and do not require a parent-child relationship. OCA groups can contain unrelated orders on different instruments.

### Transmit Pattern

Same as brackets: set `transmit = false` on all orders except the last one, which uses `transmit = true` to trigger transmission of the entire group.

---

## 5. Trailing Stop Mechanics

### How Trailing Stops Work

A trailing stop maintains a dynamic stop price that follows the market in a favorable direction:

- **BUY trailing stop**: Stop price trails *above* the market low. As price drops, stop drops. As price rises, stop stays fixed.
- **SELL trailing stop**: Stop price trails *below* the market high. As price rises, stop rises. As price drops, stop stays fixed.

### Configuration Options

| Mode | Fields to Set | Example |
|---|---|---|
| **Absolute trail** | `auxPrice` = trail amount | `auxPrice = 2.00` means $2 trailing distance |
| **Percentage trail** | `trailingPercent` = trail % | `trailingPercent = 5.0` means 5% trailing distance |
| **With initial stop** | + `trailStopPrice` | Sets initial stop level; trail governs subsequent adjustments |

### Trailing Stop Limit Additional Fields

| Field | Description |
|---|---|
| `trailStopPrice` | The initial/current trailing stop price |
| `lmtPriceOffset` | Distance between stop price and limit price (positive = more favorable than stop) |
| `auxPrice` | The trailing amount (absolute) |

**Example**: SELL trailing stop limit with trail = $2.00, limit offset = $0.50, initial stop at $148.00:
- Stop trails at $2 below highest price seen
- When triggered, limit order is placed at stop price - $0.50

### Trail Step Size

IB does not expose an explicit "trail step size" parameter in the TWS API. The trailing stop adjusts continuously (tick-by-tick) as market prices update. The minimum adjustment is one tick of the instrument.

### Simulated vs Native

Most trailing stops are **simulated by IB's servers** (not native exchange orders). The `triggerMethod` field (see section 12) controls how the simulated stop is triggered. If the exchange natively supports the stop variant, the `triggerMethod` is ignored.

---

## 6. Adjustable Stops

Adjustable stops allow a one-time modification of a stop order when a trigger price is hit. The parent stop converts into a different order type upon trigger activation.

### Fields

| Field | Type | Description |
|---|---|---|
| `triggerPrice` | double | When this price is penetrated, the adjustment activates |
| `adjustedOrderType` | string | New order type after trigger: `"STP"`, `"STP LMT"`, `"TRAIL"` |
| `adjustedStopPrice` | double | New stop price after adjustment |
| `adjustedStopLimitPrice` | double | New limit price for stop-limit adjusted orders |
| `adjustedTrailingAmount` | double | Trailing distance for TRAIL adjusted orders |
| `adjustableTrailingUnit` | int | 0 = amount, 100 = percentage |
| `parentId` | int | Links attached order to parent |

### Adjustment Variations

| Adjustment Target | `adjustedOrderType` | Additional Fields Needed |
|---|---|---|
| Adjust to Stop | `"STP"` | `adjustedStopPrice` |
| Adjust to Stop Limit | `"STP LMT"` | `adjustedStopPrice`, `adjustedStopLimitPrice` |
| Adjust to Trailing Stop | `"TRAIL"` | `adjustedTrailingAmount`, `adjustableTrailingUnit` |

**Example**: A stop at $145 that adjusts to a trailing stop of 3% when the stock reaches $155:
- `orderType = "STP"`, `auxPrice = 145.0`
- `triggerPrice = 155.0`
- `adjustedOrderType = "TRAIL"`
- `adjustedTrailingAmount = 3.0`
- `adjustableTrailingUnit = 100` (percentage)

---

## 7. Conditional Orders

Conditions control when an order activates or is canceled. Multiple conditions can be combined with AND/OR logic.

### Condition Types

| Condition Type | Key Fields | Description |
|---|---|---|
| **Price** | `conId`, `exchange`, `isMore`, `triggerMethod`, `price` | Activate/cancel when a specified instrument hits a price |
| **Time** | `isMore`, `time` | Activate/cancel at a specified date/time |
| **Volume** | `conId`, `exchange`, `isMore`, `volume` | Activate/cancel when trading volume exceeds threshold |
| **Margin** | `isMore`, `percent` | Activate/cancel when account margin cushion hits percentage |
| **PercentChange** | `conId`, `exchange`, `isMore`, `changePercent` | Activate/cancel when price change percentage is exceeded |
| **Execution** | `symbol`, `exchange`, `secType` | Activate when a trade occurs on a specific product |

### Price Condition Fields

| Field | Type | Description |
|---|---|---|
| `conId` | int | Contract ID of the reference instrument |
| `exchange` | string | Exchange of reference instrument (e.g., `"SMART"`) |
| `isMore` | bool | `true` = trigger when price > threshold, `false` = when price < threshold |
| `price` | double | The threshold price |
| `triggerMethod` | enum | How to evaluate (Last, Bid, Ask, etc.) -- see trigger methods table |

### Time Condition Fields

| Field | Type | Description |
|---|---|---|
| `isMore` | bool | `true` = after this time, `false` = before this time |
| `time` | string | Format: `"yyyyMMdd HH:mm:ss timezone"` (e.g., `"20260401 09:30:00 US/Eastern"`) |

### Volume Condition Fields

| Field | Type | Description |
|---|---|---|
| `conId` | int | Contract ID |
| `exchange` | string | Exchange |
| `isMore` | bool | `true` = volume above threshold, `false` = below |
| `volume` | int | Volume threshold |

### Margin Condition Fields

| Field | Type | Description |
|---|---|---|
| `isMore` | bool | `true` = margin cushion above %, `false` = below |
| `percent` | double | Margin cushion percentage threshold |

### Percent Change Condition Fields

| Field | Type | Description |
|---|---|---|
| `conId` | int | Contract ID |
| `exchange` | string | Exchange |
| `isMore` | bool | `true` = change above threshold, `false` = below |
| `changePercent` | double | Percentage change threshold |

### Combining Conditions

| Field | Type | Description |
|---|---|---|
| `isConjunctionConnection` | bool | On each condition: `true` = AND with next condition, `false` = OR |
| `conditionsCancelOrder` | bool | On the order: if `true`, cancel (instead of activate) when conditions are met |
| `conditionsIgnoreRth` | bool | On the order: if `true`, evaluate conditions outside regular trading hours |

### rust-ibapi Condition Builders

The `ibapi` crate provides typed condition builders:

| Builder | Method | Usage |
|---|---|---|
| `PriceConditionBuilder` | `price(contract_id, exchange).greater_than(value)` / `.less_than(value)` | Price conditions |
| `TimeConditionBuilder` | `time().greater_than(timestamp)` | Time conditions |
| `VolumeConditionBuilder` | `volume(contract_id, exchange).greater_than(value)` | Volume conditions |
| `MarginConditionBuilder` | `margin().greater_than(percentage)` | Margin conditions |
| `PercentChangeConditionBuilder` | `percent_change(contract_id, exchange).greater_than(value)` | Percent change |
| `ExecutionConditionBuilder` | `execution(symbol, exchange, sec_type)` | Execution conditions |

Conditions are chained on the `OrderBuilder`:
- `.condition(first)` -- sets initial condition
- `.and_condition(next)` -- AND logic
- `.or_condition(next)` -- OR logic

---

## 8. IB Algorithmic Orders

All IB algos use `order.algoStrategy` and `order.algoParams` (list of `TagValue` pairs).

### IB Algo Strategies -- Complete Reference

#### 8.1 Adaptive

**`algoStrategy = "Adaptive"`**

Attempts to fill between the spread for better execution than basic market/limit orders.

| Parameter | Tag | Values | Description |
|---|---|---|---|
| Priority | `adaptivePriority` | `"Urgent"`, `"Normal"`, `"Patient"` | Urgency of fill |

**Notes**: Can be used with `MKT` or `LMT` order types. Most commonly used algo for retail-size orders.

#### 8.2 VWAP

**`algoStrategy = "Vwap"`**

Seeks to achieve Volume-Weighted Average Price from submission to market close.

| Parameter | Tag | Values | Description |
|---|---|---|---|
| Max % of volume | `maxPctVol` | `"0.1"` to `"0.5"` | Max contribution to ADV (10-50%) |
| Start time | `startTime` | `"HH:mm:ss TMZ"` or `"YYYYMMDD-HH:mm:ss TMZ"` | Algo start |
| End time | `endTime` | `"HH:mm:ss TMZ"` or `"YYYYMMDD-HH:mm:ss TMZ"` | Algo end |
| Past end time | `allowPastEndTime` | `"1"` or `"0"` | Continue after end time |
| No take liquidity | `noTakeLiq` | `"1"` or `"0"` | Only add liquidity |
| Speed up | `speedUp` | `"1"` or `"0"` | Accelerate execution |

#### 8.3 TWAP

**`algoStrategy = "Twap"`**

Achieves Time-Weighted Average Price over specified period.

| Parameter | Tag | Values | Description |
|---|---|---|---|
| Strategy type | `strategyType` | `"Marketable"`, `"Matching"`, `"Midpoint"`, `"Matching Same Side"`, `"Matching Last"` | Execution style |
| Start time | `startTime` | `"HH:mm:ss TMZ"` | Algo start |
| End time | `endTime` | `"HH:mm:ss TMZ"` | Algo end |
| Past end time | `allowPastEndTime` | `"1"` or `"0"` | Continue after end time |

#### 8.4 Arrival Price

**`algoStrategy = "ArrivalPx"`**

Minimizes market impact relative to the arrival price (price at time of order submission).

| Parameter | Tag | Values | Description |
|---|---|---|---|
| Max % of volume | `maxPctVol` | `"0.1"` to `"0.5"` | Max ADV participation |
| Risk aversion | `riskAversion` | `"Get Done"`, `"Aggressive"`, `"Neutral"`, `"Passive"` | Urgency |
| Start time | `startTime` | `"HH:mm:ss TMZ"` | Algo start |
| End time | `endTime` | `"HH:mm:ss TMZ"` | Algo end |
| Force completion | `forceCompletion` | `"1"` or `"0"` | Force fill by end time |
| Past end time | `allowPastEndTime` | `"1"` or `"0"` | Continue after end time |

**Products**: US Stocks, some European stocks, major FX pairs on IDEALPRO.

#### 8.5 Close Price

**`algoStrategy = "ClosePx"`**

Targets the closing price.

| Parameter | Tag | Values | Description |
|---|---|---|---|
| Max % of volume | `maxPctVol` | `"0.1"` to `"0.5"` | Max ADV participation |
| Risk aversion | `riskAversion` | `"Get Done"`, `"Aggressive"`, `"Neutral"`, `"Passive"` | Urgency |
| Start time | `startTime` | `"HH:mm:ss TMZ"` | Algo start |
| Force completion | `forceCompletion` | `"1"` or `"0"` | Force fill at close |

#### 8.6 Dark Ice

**`algoStrategy = "DarkIce"`**

Advanced iceberg. Randomizes display size +/- 50% based on fill probability. May price one tick inside NBBO.

| Parameter | Tag | Values | Description |
|---|---|---|---|
| Display size | `displaySize` | integer string | Size shown to market |
| Start time | `startTime` | `"HH:mm:ss TMZ"` | Algo start |
| End time | `endTime` | `"HH:mm:ss TMZ"` | Algo end |
| Past end time | `allowPastEndTime` | `"1"` or `"0"` | Continue after end time |

#### 8.7 Percentage of Volume (PctVol)

**`algoStrategy = "PctVol"`**

Executes at a specified percentage of market volume.

| Parameter | Tag | Values | Description |
|---|---|---|---|
| Pct of volume | `pctVol` | `"0.1"` to `"0.5"` | Target % of volume (10-50%) |
| Start time | `startTime` | `"HH:mm:ss TMZ"` | Algo start |
| End time | `endTime` | `"HH:mm:ss TMZ"` | Algo end |
| No take liquidity | `noTakeLiq` | `"1"` or `"0"` | Only add liquidity |

#### 8.8 Balance Impact Risk

**`algoStrategy = "BalanceImpactRisk"`**

Balances market impact against timing risk.

| Parameter | Tag | Values | Description |
|---|---|---|---|
| Max % of volume | `maxPctVol` | `"0.1"` to `"0.5"` | Max ADV participation |
| Risk aversion | `riskAversion` | `"Get Done"`, `"Aggressive"`, `"Neutral"`, `"Passive"` | Urgency |
| Force completion | `forceCompletion` | `"1"` or `"0"` | Force fill |

#### 8.9 Minimize Impact

**`algoStrategy = "MinImpact"`**

Minimizes market impact with volume participation.

| Parameter | Tag | Values | Description |
|---|---|---|---|
| Max % of volume | `maxPctVol` | `"0.1"` to `"0.5"` | Max ADV participation |

#### 8.10 Accumulate/Distribute

**`algoStrategy = "AD"`**

Breaks large order into smaller components over time.

| Parameter | Tag | Values | Description |
|---|---|---|---|
| Component size | `componentSize` | integer string | Size of each sub-order (cannot exceed total) |
| Time between orders | `timeBetweenOrders` | seconds string | Interval between components |
| Randomize time | `randomizeTime20` | `"1"` or `"0"` | Randomize interval +/- 20% |
| Randomize size | `randomizeSize55` | `"1"` or `"0"` | Randomize size +/- 55% |
| Give up | `giveUp` | integer string | Clearing number |
| Catch up | `catchUp` | `"1"` or `"0"` | Catch up if behind schedule |
| Wait for fill | `waitForFill` | `"1"` or `"0"` | Wait for component fill before next |
| Active start | `activeTimeStart` | `"YYYYMMDD-HH:mm:ss TMZ"` | Window start |
| Active end | `activeTimeEnd` | `"YYYYMMDD-HH:mm:ss TMZ"` | Window end |

#### 8.11 Price Variant PctVol

**`algoStrategy = "PctVolPx"`**

Volume participation that varies with price.

| Parameter | Tag | Values | Description |
|---|---|---|---|
| Base pct vol | `pctVol` | `"0.1"` to `"0.5"` | Base participation rate |
| Delta pct vol | `deltaPctVol` | `"0.1"` to `"0.5"` | Adjustment factor |
| Min pct vol for price | `minPctVol4Px` | `"0.1"` to `"0.5"` | Floor participation |
| Max pct vol for price | `maxPctVol4Px` | `"0.1"` to `"0.5"` | Ceiling participation |
| Start / End time | `startTime`, `endTime` | time format | Time window |
| No take liquidity | `noTakeLiq` | `"1"` or `"0"` | Passive only |

#### 8.12 Size Variant PctVol

**`algoStrategy = "PctVolSz"`**

Volume participation that changes over size.

| Parameter | Tag | Values | Description |
|---|---|---|---|
| Start pct vol | `startPctVol` | `"0.1"` to `"0.5"` | Initial participation |
| End pct vol | `endPctVol` | `"0.1"` to `"0.5"` | Final participation |
| Start / End time | `startTime`, `endTime` | time format | Time window |
| No take liquidity | `noTakeLiq` | `"1"` or `"0"` | Passive only |

#### 8.13 Time Variant PctVol

**`algoStrategy = "PctVolTm"`**

Volume participation that changes over time.

| Parameter | Tag | Values |
|---|---|---|
| Start pct vol | `startPctVol` | `"0.1"` to `"0.5"` |
| End pct vol | `endPctVol` | `"0.1"` to `"0.5"` |
| Start / End time | `startTime`, `endTime` | time format |
| No take liquidity | `noTakeLiq` | `"1"` or `"0"` |

#### 8.14 Midprice (special -- not algoStrategy)

**`orderType = "MIDPRICE"`** (not via algoStrategy/algoParams)

| Field | Description |
|---|---|
| `lmtPrice` | Optional price cap (required in practice despite being documented as optional) |

US stocks/ETFs only, RTH only, requires TWS 975+.

### Third-Party Algos

Also supported via the same `algoStrategy`/`algoParams` mechanism:

| Provider | Example Strategies |
|---|---|
| **Jefferies** | `"JEFF_VWAP"`, `"JEFF_TWAP"`, `"JEFF_POST"`, etc. |
| **Quantitative Brokers** | `"STROBE"`, etc. |
| **CSFB** | Various algos |

### rust-ibapi Algo Builders

The crate provides dedicated builder types for common algos:
- `TwapBuilder`
- `VwapBuilder`
- `ArrivalPriceBuilder`
- `PctVolBuilder`

---

## 9. Exchange Routing

### SMART Routing

Default routing. IB's SmartRouting algorithm seeks best execution across all available exchanges.

| Strategy | Description |
|---|---|
| SMART Multipurpose | Default -- routes to whichever exchange provides best execution |
| SMART Maximize Rebate | Routes to exchange offering highest rebate |
| SMART Prefer Rebate | Bias toward rebate-offering exchanges |
| SMART Prefer Fill | Bias toward fastest fill |
| SMART Maximize Fill | Maximize fill probability |

**API**: Set `contract.exchange = "SMART"`. For primary exchange disambiguation: `contract.primaryExchange = "NASDAQ"`.

Composite format: `contract.exchange = "SMART:ARCA"` (routes via SMART, primary = ARCA).

### Direct Exchange Routing

Set `contract.exchange` to a specific exchange code:

| Exchange | Code | Products |
|---|---|---|
| NASDAQ | `"ISLAND"` | STK |
| NYSE | `"NYSE"` | STK |
| NYSE Arca | `"ARCA"` | STK, ETF |
| BATS | `"BATS"` | STK |
| IEX | `"IEX"` | STK |
| BOX | `"BOX"` | OPT |
| CBOE | `"CBOE"` | OPT |
| CME | `"CME"` | FUT |
| GLOBEX | `"GLOBEX"` | FUT |
| IDEALPRO | `"IDEALPRO"` | FX |
| IBKRATS | `"IBKRATS"` | STK (dark pool) |

### IBKRATS (IB ATS) Routing

For dark pool execution:
- Set `contract.exchange = "IBKRATS"`
- Set `order.notHeld = true` (required, or order is rejected)
- Supports Pegged-to-Best, Pegged-to-Midpoint with offsets

### smartComboRoutingParams

For combo/spread orders, `smartComboRoutingParams` uses tag/value pairs:

| Tag | Values | Description |
|---|---|---|
| `NonGuaranteed` | `"0"`, `"1"` | `"1"` = legs may fill independently |
| `LegsExchanges` | exchange codes | Specify exchanges for individual legs |

### postToAts

`order.postToAts` (int): Number of seconds a SMART order is parked at IBKRATS before routing to other venues. Must be positive.

---

## 10. Risk Controls & Order Limitations

### Hard Limits (IB Server)

| Limit | Value | Notes |
|---|---|---|
| Message rate | 50 messages/second | Maximum order submission rate |
| Active orders per contract per side | 20 | Per account, per contract, per side (buy/sell) |
| Simultaneous API connections | 32 | clientId 0-31 per TWS/Gateway |

### Price Protection (Collars)

IB applies automatic price collars to prevent erroneous orders:
- **Stocks**: ~10% deviation from NBBO causes rejection or collar
- **Options**: ~20% deviation from NBBO
- `order.usePriceMgmtAlgo = true` (default): IB caps order at their calculated collar price
- `order.usePriceMgmtAlgo = false`: Disables IB price management

### TWS Precautionary Settings

Configurable in TWS: **Global Configuration > Presets > [Asset Class] > Precautionary Settings**:

| Setting | Default | Description |
|---|---|---|
| Size Limit | Default Size x 5 | Maximum order size |
| Total Value Limit | $100,000 USD | Maximum order dollar value |
| Bypass for API | Configurable | "Bypass Order Precautions for API Orders" checkbox |

### API Overrides

| Field | Description |
|---|---|
| `overridePercentageConstraints` | Bypasses TWS percentage-based constraint checks |
| Bypass checkbox in TWS | Allows API orders exceeding precautionary limits without confirmation dialogs |

### Order Efficiency Ratio (OER)

IB monitors the ratio of order submissions to fills. Excessive order modifications/cancellations relative to fills can result in throttling or penalties. See IB's "Considerations for Optimizing Order Efficiency" for thresholds.

---

## 11. Order Preview (What-If)

Setting `order.whatIf = true` simulates the order without placing it. The response (via `openOrder` callback) returns an `OrderState` object with:

| Field | Description |
|---|---|
| `initMarginBefore` | Initial margin requirement before this order |
| `initMarginAfter` | Initial margin requirement after this order |
| `maintMarginBefore` | Maintenance margin before |
| `maintMarginAfter` | Maintenance margin after |
| `equityWithLoanBefore` | Equity with loan value before |
| `equityWithLoanAfter` | Equity with loan value after |
| `commission` | Estimated commission |
| `commissionCurrency` | Commission currency |
| `minCommission` | Minimum estimated commission |
| `maxCommission` | Maximum estimated commission |
| `warningText` | Any warnings |

This is equivalent to the TWS "Preview" / "Margin Impact" panel.

---

## 12. Trigger Methods

Controls how IB simulates stop/stop-limit/trailing stop orders.

### Trigger Method Values

| Code | Name | Description |
|---|---|---|
| 0 | Default | Default trigger method for the instrument type |
| 1 | Double Bid/Ask | Requires two consecutive bid (sell stop) or ask (buy stop) prices at or through the stop |
| 2 | Last | Triggers on last trade price |
| 3 | Double Last | Requires two consecutive last trade prices at or through the stop |
| 4 | Bid/Ask | Triggers on single bid (sell) or ask (buy) price |
| 7 | Last or Bid/Ask | Triggers on either last trade or bid/ask (used by iBot alerts) |
| 8 | Mid-point | Triggers at NBBO midpoint |

**Set via**: `order.triggerMethod = N`

**Important notes:**
- These only apply to **simulated** stop orders (where IB holds the order and monitors the market). Most US stock stops are simulated by IB.
- If the exchange natively handles the stop variant, `triggerMethod` is **ignored**.
- Using an incompatible `triggerMethod` with a `secType` may cause the order to **never trigger**.
- Also applies to conditional orders' price conditions.

### Default Trigger Methods by Product

| Product | Default Method |
|---|---|
| US Stocks | Double Bid/Ask (1) |
| US Options | Double Bid/Ask (1) |
| US Futures | Last (2) |
| Non-US products | Varies by exchange |

---

## 13. Delta Neutral / Hedge Orders

Volatility orders can include an automatic delta hedge that executes when the options order fills.

### Delta Neutral Fields (on Order)

| Field | Type | Description |
|---|---|---|
| `deltaNeutralOrderType` | string | Order type for the hedging leg (`"MKT"`, `"LMT"`, `"REL"`) |
| `deltaNeutralAuxPrice` | double | Aux price if hedge order type requires it |
| `deltaNeutralConId` | int | Contract ID of the hedging instrument |
| `deltaNeutralSettlingFirm` | string | Settlement firm (institutional) |
| `deltaNeutralClearingAccount` | string | Clearing account (institutional) |
| `deltaNeutralClearingIntent` | string | `"IB"`, `"Away"`, or `"PTA"` |
| `deltaNeutralOpenClose` | string | `"O"` (open) or `"C"` (close) for CFD hedges |
| `deltaNeutralShortSale` | bool | Whether hedge involves short selling stock |
| `deltaNeutralShortSaleSlot` | int | 1 = broker holds shares, 2 = shares from elsewhere |
| `deltaNeutralDesignatedLocation` | string | Third-party origin for short sale |

### Behavior

When attached to a VOL order with `continuousUpdate = 1`:
- The system uses the delta from the parent order (based on user-defined IV at time of last modification)
- Not the current market-derived delta
- Hedge order auto-executes when the options order fills

---

## 14. Scale Orders

Scale orders break a large order into smaller increments ("scales") that execute as price levels are reached.

### Scale Fields (on Order)

| Field | Type | Description |
|---|---|---|
| `scaleInitLevelSize` | int | Size of the first component |
| `scaleSubsLevelSize` | int | Size of subsequent components |
| `scalePriceIncrement` | double | Price increment between components |
| `scalePriceAdjustValue` | double | Price adjustment for subsequent fills |
| `scalePriceAdjustInterval` | int | Number of filled components before price adjusts |
| `scaleProfitOffset` | double | Profit offset for scale profit orders |
| `scaleAutoReset` | bool | Auto-reset scale counter after all components fill |
| `scaleInitPosition` | int | Initial position for the scale |
| `scaleInitFillQty` | int | Initial filled quantity for scale |
| `scaleRandomPercent` | bool | Randomize component sizes |
| `scaleTable` | string | Custom scale definition table |

---

## 15. rust-ibapi Crate Coverage

The project uses `ibapi = "2.10"` (crate: [rust-ibapi](https://github.com/wboayue/rust-ibapi)).

### Supported Order Types (via OrderBuilder fluent API)

| Category | Order Types | Builder Method |
|---|---|---|
| **Basic** | Market, Limit, Stop, Stop-Limit | `.market()`, `.limit(price)`, `.stop(price)`, `.stop_limit(stop, limit)` |
| **Trailing** | Trailing Stop, Trailing Stop Limit | `.trailing_stop(trail, initial)`, `.trailing_stop_limit(trail, stop, offset)` |
| **At-Close** | MOC, LOC | `.market_on_close()`, `.limit_on_close(price)` |
| **At-Open** | MOO, LOO | `.market_on_open()`, `.limit_on_open(price)` |
| **Touched** | MIT, LIT | `.market_if_touched(trigger)`, `.limit_if_touched(trigger, limit)` |
| **Auction** | At Auction | `.at_auction(price)` |
| **Protected** | Market with Protection, Stop with Protection | Referenced in docs |
| **Advanced** | Market-to-Limit, Midprice, Relative, Passive Relative, Discretionary, Sweep-to-Fill, Block | Referenced in docs |
| **Complex** | Bracket, OCA | `BracketOrderBuilder`, OCA support |
| **Conditional** | All 6 condition types | `PriceConditionBuilder`, `TimeConditionBuilder`, `VolumeConditionBuilder`, `MarginConditionBuilder`, `PercentChangeConditionBuilder`, `ExecutionConditionBuilder` |
| **Algo** | VWAP, TWAP, Arrival Price, PctVol | `VwapBuilder`, `TwapBuilder`, `ArrivalPriceBuilder`, `PctVolBuilder` |

### Key Enums

| Enum | Variants |
|---|---|
| `Action` | `Buy`, `Sell`, `SShort`, `SLong` |
| `TimeInForce` | `Day`, `GoodTilCanceled`, `ImmediateOrCancel`, `GoodTilDate`, `OnOpen`, `FillOrKill`, `DayTilCanceled`, `Auction` |
| `OcaType` | Cancel with block (1), Reduce with block (2), Reduce without block (3) |
| `AuctionStrategy` | `AuctionMatch` (1), `AuctionImprovement` (2), `AuctionTransparent` (3) |
| `VolatilityType` | `Daily` (1), `Annual` (2) |
| `ReferencePriceType` | `AverageNBBO` (1), `NBBOrNBO` (2) |
| `OrderCondition` | `Price`, `Time`, `Margin`, `Execution`, `Volume`, `PercentChange` |

### Key Structs

| Struct | Purpose |
|---|---|
| `Order` | Full order specification (all fields from IB Order class) |
| `OrderBuilder` | Fluent API for constructing orders |
| `BracketOrderBuilder` | Builder for bracket order triplets |
| `BracketOrderIds` | Container for parent + child order IDs |
| `OrderState` | Current state of active orders (including what-if margin data) |
| `OrderStatus` | Execution status (filled, remaining, avgFillPrice) |
| `OrderData` | Combined order + contract + state |
| `Execution` | Fill details |
| `ExecutionFilter` | Filter criteria for execution reports |
| `CommissionReport` | Commission per execution |
| `OrderComboLeg` | Per-leg pricing for combo orders |
| `SoftDollarTier` | Soft dollar tier information |

### Known Gaps / Considerations

1. **Parity target**: The crate states future updates focus on "maintaining parity with the official API," implying some features may lag behind the latest TWS API additions.
2. **IBKRATS advanced types**: Pegged-to-Best and Pegged-to-Midpoint with offsets (newer IBKRATS features) -- verify support in current version.
3. **Third-party algos**: Jefferies, CSFB, QB algos may require manual `algoStrategy`/`algoParams` construction rather than typed builders.
4. **Scale orders**: Not listed in fluent builder docs -- may require manual field setting on the `Order` struct.
5. **Pegged-to-Benchmark**: Complex multi-field setup -- likely requires manual `Order` struct configuration.

---

## 16. Sources

### Official IB Documentation

- [TWS API: Order Class Reference](https://interactivebrokers.github.io/tws-api/classIBApi_1_1Order.html) -- complete field listing
- [TWS API: Basic Orders](https://interactivebrokers.github.io/tws-api/basic_orders.html) -- order type code samples
- [TWS API: Bracket Orders](https://interactivebrokers.github.io/tws-api/bracket_order.html) -- bracket implementation
- [TWS API: One-Cancels-All](https://interactivebrokers.github.io/tws-api/oca.html) -- OCA groups
- [TWS API: Adjustable Stops](https://interactivebrokers.github.io/tws-api/adjustable_stops.html) -- adjustable stop orders
- [TWS API: Order Conditioning](https://interactivebrokers.github.io/tws-api/order_conditions.html) -- conditional orders
- [TWS API: IB Algorithms](https://interactivebrokers.github.io/tws-api/ibalgos.html) -- all IB algo strategies with parameters
- [TWS API: Trigger Methods](https://interactivebrokers.github.io/tws-api/trigger_method_limit.html) -- trigger method values
- [TWS API: Order Limitations](https://interactivebrokers.github.io/tws-api/order_limitations.html) -- rate limits, constraints
- [TWS API: IBKRATS Orders](https://interactivebrokers.github.io/tws-api/ibkrats.html) -- dark pool routing
- [TWS API: Checking Margin](https://interactivebrokers.github.io/tws-api/margin.html) -- what-if preview
- [TWS API: Placing Orders](https://interactivebrokers.github.io/tws-api/order_submission.html) -- order submission overview
- [TWS API: Spreads](https://interactivebrokers.github.io/tws-api/spread_contracts.html) -- combo/multi-leg orders

### IBKR Campus

- [IBKR Campus: Order Types](https://www.interactivebrokers.com/campus/ibkr-api-page/order-types/) -- order types overview
- [IBKR Campus: TWS API Documentation](https://www.interactivebrokers.com/campus/ibkr-api-page/twsapi-doc/) -- current API docs hub
- [IBKR Campus: Trailing Stop Order](https://www.interactivebrokers.com/campus/glossary-terms/trailing-stop-order/) -- trailing stop glossary
- [IBKR Campus: Precautionary Settings](https://www.interactivebrokers.com/campus/glossary-terms/precautionary-settings/) -- risk controls
- [IBKR Campus: OCA Order Type](https://www.interactivebrokers.com/campus/trading-lessons/tws-mosaic-one-cancels-all-oca-order-type/) -- OCA tutorial
- [IBKR Campus: Complex Orders (Python)](https://www.interactivebrokers.com/campus/trading-lessons/python-complex-orders/) -- bracket/OCA examples

### IB Order Type Product Pages

- [Iceberg/Reserve Orders](https://www.interactivebrokers.com/en/trading/orders/iceberg.php)
- [Volatility Orders](https://www.interactivebrokers.com/en/trading/orders/volatility.php)
- [Direct Routing](https://www.interactivebrokers.com/en/trading/orders/direct-routing.php)
- [Pegged-to-Midpoint](https://www.interactivebrokers.com/en/trading/orders/pegged-to-midpoint.php)
- [TWAP Algo](https://www.interactivebrokers.com/en/trading/orders/twap-algo.php)
- [Time in Force](https://www.interactivebrokers.com/en/software/tws/usersguidebook/ordertypes/time_in_force_for_orders.htm)

### rust-ibapi

- [GitHub: wboayue/rust-ibapi](https://github.com/wboayue/rust-ibapi) -- source repository
- [docs.rs: ibapi](https://docs.rs/ibapi/latest/ibapi/) -- API documentation
- [Order Types Guide](https://github.com/wboayue/rust-ibapi/blob/main/docs/order-types.md) -- supported order types
- [crates.io: ibapi](https://crates.io/crates/ibapi) -- crate registry

### IB Guides & Misc

- [IBKR Guides: Order Types](https://www.ibkrguides.com/traderworkstation/order-types.htm) -- full order type reference
- [IBKR Guides: Precautionary Settings](https://www.interactivebrokers.co.uk/en/software/tws.bak/usersguidebook/configuretws/define_precautionary_settings.htm)
- [IBKR Guides: Trigger Methods](https://ibkrguides.com/tws/usersguidebook/configuretws/modify%20the%20stop%20trigger%20method.htm)
- [TWS API 2025 Release Notes](https://www.ibkrguides.com/releasenotes/prod-2025.htm)
- [Disclosure: Price Cap Notices](https://www.ibkrguides.com/kb/article-3449.htm)
