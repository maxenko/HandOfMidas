# 03 -- Order Feature Comparison Matrix

> TradingView vs Interactive Brokers API vs Hand of Midas (current state)
>
> Research compiled April 2026. Verify at tradingview.com and interactivebrokers.com before relying on specifics.

---

## Priority Key

| Priority | Meaning |
|---|---|
| **Done** | Already implemented in Hand of Midas |
| **P1** | Must have -- every trading platform needs this |
| **P2** | Should have -- serious traders expect this |
| **P3** | Nice to have -- power user / algorithmic |
| **Skip** | IB-only feature TradingView does not expose, or too niche for our scope |

---

## 1. Order Types

| Feature | TradingView | IB API | Hand of Midas (Current) | Priority |
|---|---|---|---|---|
| **Market Order** | Yes | `MKT` | Yes (`OrderKind::Market`) | Done |
| **Limit Order** | Yes | `LMT` | Enum exists (`OrderKind::Limit`), not wired to UI/submission | P1 |
| **Stop Order** | Yes | `STP` | Enum exists (`OrderKind::Stop`), not wired to UI/submission | P1 |
| **Stop-Limit Order** | Yes (configurable) | `STP LMT` | Enum exists (`OrderKind::StopLimit`), not wired to UI | P1 |
| **Trailing Stop** | Yes (bracket SL only) | `TRAIL` | Enum exists (`OrderKind::TrailingStop`), not wired to UI | P2 |
| **Trailing Stop-Limit** | No | `TRAIL LIMIT` | No | P3 |
| **Market-if-Touched (MIT)** | No | `MIT` | No | P3 |
| **Limit-if-Touched (LIT)** | No | `LIT` | No | P3 |
| **Market-to-Limit (MTL)** | No | `MTL` | No | Skip |
| **Market-on-Close (MOC)** | No | `MOC` | No | P3 |
| **Limit-on-Close (LOC)** | No | `LOC` | No | P3 |
| **Fill-or-Kill (FOK)** | No | `FOK` (TIF variant) | No | Skip |
| **Pegged-to-Primary (REL)** | No | `REL` | No | Skip |
| **Pegged-to-Midpoint** | No | `PEG MID` | No | Skip |
| **Pegged-to-Market** | No | `PEG MKT` | No | Skip |
| **Pegged-to-Benchmark** | No | `PEG BENCH` | No | Skip |
| **Volatility Order** | No | `VOL` | No | Skip |
| **VWAP Order** | No | `VWAP` | No | Skip |
| **Stop with Protection** | No | `STP PRT` | No | Skip |
| **Market with Protection** | No | `MKT PRT` | No | Skip |

### Summary

TradingView exposes 5 order types to users (Market, Limit, Stop, Stop-Limit, Trailing Stop). IB supports 20+ order types. Hand of Midas has all 5 TV-equivalent enums defined but only Market is wired end-to-end.

---

## 2. Bracket / Exit Strategy Orders

| Feature | TradingView | IB API | Hand of Midas (Current) | Priority |
|---|---|---|---|---|
| **Market bracket (MKT + TP + SL)** | Yes | Yes (parent + children + OCA) | Yes -- full pipeline: panel, chart viz, broker params, validation | Done |
| **Limit bracket (LMT entry + TP + SL)** | Yes | Yes | No | P1 |
| **Stop bracket (STP entry + TP + SL)** | No (not standard in TV) | Yes | No | P2 |
| **Take-profit as Limit child** | Yes | Yes (`LMT` child, `parentId` link) | Yes (`TakeProfitParams`, `BracketRole::TakeProfit`) | Done |
| **Stop-loss as Stop child** | Yes | Yes (`STP` child, `parentId` link) | Yes (`StopLossParams.stop_price`) | Done |
| **Stop-loss as Stop-Limit child** | Yes (configurable in TV) | Yes (`STP LMT` child) | Enum/param exists (`StopLossParams.limit_price`), not in UI | P1 |
| **Trailing stop as SL child** | Yes (TV supports trailing SL in brackets) | Yes (`TRAIL` child with `parentId`) | No | P2 |
| **Multiple TP targets (partial exits)** | No (TV = 1 TP per bracket) | Yes (multiple children with OCA) | No | P3 |
| **OCA (One-Cancels-All) grouping** | Implicit (bracket OCO) | Explicit (`ocaGroup`, `ocaType` 1/2/3) | Field exists (`oca_group` on `LocalOrder`), not wired | P2 |
| **Bracket modification (move TP/SL)** | Yes (drag on chart) | Yes (`placeOrder` with same `orderId`) | Draft drag works; live modification not yet wired | P1 |
| **Bracket cancellation** | Yes | Yes (`cancelOrder` per leg or parent) | Draft cancel works; live cancel not yet wired | P1 |

---

## 3. Quantity & Position Sizing

| Feature | TradingView | IB API | Hand of Midas (Current) | Priority |
|---|---|---|---|---|
| **Fixed share quantity** | Yes | Yes (`totalQuantity`) | Yes (text input, f64) | Done |
| **Dollar amount entry** | Yes (some brokers) | No (computed client-side) | No | P2 |
| **Percentage of account** | Yes (position sizing tool) | No (computed client-side) | No | P2 |
| **Risk-based sizing (% of equity at risk)** | Yes (Long/Short position tool) | No (computed client-side) | No | P2 |
| **Fractional shares** | Yes (broker-dependent) | Yes (IBKR supports fractional for US stocks) | Field supports f64 but no fractional logic | P3 |
| **Min tick / lot size validation** | No (broker-side) | Yes (contract details specify min tick) | No | P1 |

---

## 4. Time-in-Force

| Feature | TradingView | IB API | Hand of Midas (Current) | Priority |
|---|---|---|---|---|
| **DAY** | Yes (default) | Yes | Yes (`TimeInForce::Day`, default) | Done |
| **GTC (Good-Til-Cancelled)** | Yes | Yes | Yes (`TimeInForce::Gtc`) | Done |
| **IOC (Immediate-or-Cancel)** | No | Yes | Yes (`TimeInForce::Ioc`) -- enum only, not in UI | P3 |
| **GTD (Good-Til-Date)** | No | Yes (`goodTillDate` field) | Yes (`TimeInForce::Gtd`) -- enum only, not in UI | P3 |
| **OPG (At-the-Open)** | No | Yes | Yes (`TimeInForce::Opg`) -- enum only, not in UI | P3 |
| **FOK (Fill-or-Kill)** | No | Yes (TIF variant) | No | Skip |
| **GoodAfterTime** | No | Yes (`goodAfterTime` field) | No | P3 |
| **ActiveStartTime / ActiveStopTime** | No | Yes (GTC orders only) | No | P3 |

---

## 5. Extended Hours & Routing

| Feature | TradingView | IB API | Hand of Midas (Current) | Priority |
|---|---|---|---|---|
| **Extended hours (pre/post market)** | Yes (broker-dependent toggle) | Yes (`outsideRth = True`) | Yes (`outside_rth` field on `LocalOrder`) | Done |
| **Exchange routing (SMART)** | No (abstracted by broker) | Yes (`exchange` field, SMART default) | Yes (`exchange` field, "SMART" default) | Done |
| **Directed routing (specific exchange)** | No | Yes (e.g., "ARCA", "ISLAND", "BATS") | Field exists but no UI | P3 |
| **Primary exchange disambiguation** | No | Yes (`primaryExchange` field) | No | P2 |

---

## 6. Conditional / Advanced Orders

| Feature | TradingView | IB API | Hand of Midas (Current) | Priority |
|---|---|---|---|---|
| **Price condition (trigger on another instrument)** | No | Yes (`OrderCondition.Price`, `conId`, `triggerMethod`) | No | P3 |
| **Time condition** | No | Yes (`OrderCondition.Time`) | No | P3 |
| **Volume condition** | No | Yes (`OrderCondition.Volume`) | No | Skip |
| **Margin cushion condition** | No | Yes (`OrderCondition.Margin`) | No | Skip |
| **Percent change condition** | No | Yes (`OrderCondition.PercentChange`) | No | Skip |
| **Execution condition** | No | Yes (`OrderCondition.Execution`) | No | Skip |
| **Condition conjunction (AND/OR)** | No | Yes (`isConjunctionConnection`) | No | P3 |
| **Conditions cancel order (inverse)** | No | Yes (`conditionsCancelOrder`) | No | Skip |
| **Adjustable stops (trigger modifies parent)** | No | Yes (attached adjusted orders) | No | P3 |

---

## 7. IB Algo Orders

| Feature | TradingView | IB API | Hand of Midas (Current) | Priority |
|---|---|---|---|---|
| **Adaptive algo** | No | Yes (`algoStrategy = "Adaptive"`) | Fields exist (`algo_strategy`, `algo_params` on `LocalOrder`) | P2 |
| **TWAP** | No | Yes (`algoStrategy = "Twap"`) | No | P3 |
| **VWAP** | No | Yes (`algoStrategy = "Vwap"`) | No | P3 |
| **Arrival Price** | No | Yes (`algoStrategy = "ArrivalPx"`) | No | Skip |
| **Close Price** | No | Yes (`algoStrategy = "ClosePx"`) | No | Skip |
| **Dark Ice** | No | Yes (`algoStrategy = "DarkIce"`) | No | Skip |
| **Pct of Volume** | No | Yes (`algoStrategy = "PctVol"`) | No | Skip |
| **Accumulate/Distribute** | No | Yes (`algoStrategy = "AD"`) | No | Skip |

---

## 8. Order Modification & Lifecycle

| Feature | TradingView | IB API | Hand of Midas (Current) | Priority |
|---|---|---|---|---|
| **Modify price (live order)** | Yes (drag on chart) | Yes (`placeOrder` with same `orderId`) | No (draft-only modification) | P1 |
| **Modify quantity (live order)** | Yes | Yes | No | P1 |
| **Modify TIF (live order)** | No | Yes (limited) | No | P3 |
| **Cancel single order** | Yes | Yes (`cancelOrder`) | No (draft cancel only) | P1 |
| **Cancel all orders** | No (per broker) | Yes (`reqGlobalCancel`) | No | P2 |
| **Order preview / what-if** | No (some brokers) | Yes (`orders/whatif` REST endpoint) | No | P3 |
| **Order deactivation (pull from exchange)** | No | Workaround (cancel + re-place) | State machine supports (`can_deactivate`) | P2 |
| **Order reactivation** | No | Workaround (re-submit) | State machine supports (`can_activate`) | P2 |
| **Partial fill tracking** | Yes (displays filled/remaining) | Yes (`orderStatus` callbacks) | Yes (`filled_qty`, `remaining_qty`, `PartiallyFilled` status) | Done |
| **Commission tracking** | Yes (post-fill) | Yes (`commissionReport` callback) | Field exists (`commission` on `LocalOrder`, `FillInfo`) | Done |

---

## 9. Chart Visualization

| Feature | TradingView | IB API | Hand of Midas (Current) | Priority |
|---|---|---|---|---|
| **Entry line on chart** | Yes (horizontal line at entry price) | N/A (no chart) | Yes (horizontal line, side-colored) | Done |
| **TP line on chart** | Yes (green horizontal line) | N/A | Yes (green line with label) | Done |
| **SL line on chart** | Yes (red horizontal line) | N/A | Yes (red line with label) | Done |
| **TP/SL zone fill (shaded region)** | Yes (green/red shaded zones) | N/A | Yes (Active/PartialFill brackets only) | Done |
| **R:R ratio display** | Yes (on bracket) | N/A | Yes (`risk_reward()`, displayed on entry line) | Done |
| **Projected P&L on TP/SL labels** | Yes (dollar amount) | N/A | Yes (`projected_pnl`, `projected_pnl_pct`) | Done |
| **Drag TP/SL to new price** | Yes (drag handle) | N/A | Yes (draft brackets only -- `DragBracketLeg` action) | Done |
| **Drag entire bracket (Shift+drag)** | Yes | N/A | No | P2 |
| **Draft vs Pending vs Active styling** | Partial (TV uses color changes) | N/A | Yes (6 states: Draft, Pending, PartialFill, Active, Closed, Cancelled) | Done |
| **Partial fill indicator** | Yes (filled/total display) | N/A | Yes (`PartialFill` status, filled_qty label) | Done |
| **Limit order line on chart** | Yes (horizontal line at limit price) | N/A | No | P1 |
| **Stop order line on chart** | Yes (horizontal line at stop trigger) | N/A | No | P1 |
| **Trailing stop animated line** | Yes (moves with price) | N/A | No | P2 |
| **Multiple brackets on same chart** | Yes | N/A | Yes (annotation store supports multiple) | Done |
| **Order buttons on chart lines** | No (TV uses separate panel) | N/A | Yes ([Submit], [Save], [X], [SL] buttons on draft lines) | Done |
| **Right-click context menu on orders** | Yes | N/A | Partial (`RightClickBracketLeg` action emitted, menu not rendered) | P1 |
| **Order status badge/indicator** | Yes (filled/working/cancelled icons) | N/A | Yes (status-driven line style and alpha) | Done |

---

## 10. Order Entry UI

| Feature | TradingView | IB API | Hand of Midas (Current) | Priority |
|---|---|---|---|---|
| **Dockable order panel** | Yes (side panel) | N/A | Yes (`OrderPanel` as first-class pane) | Done |
| **Buy/Sell toggle** | Yes | N/A | Yes (`OrderSide::Buy/Sell`) | Done |
| **Quantity text input** | Yes | N/A | Yes (string input, parsed on submit) | Done |
| **TP/SL toggle** | Yes (checkbox) | N/A | Yes (`tp_enabled`, `sl_enabled`) | Done |
| **TP/SL absolute price input** | Yes | N/A | Yes (`PriceInputMode::Absolute`) | Done |
| **TP/SL offset mode ($)** | Yes | N/A | Enum exists (`PriceInputMode::Offset`), not in UI | P1 |
| **TP/SL percentage mode (%)** | Yes | N/A | Enum exists (`PriceInputMode::Percent`), not in UI | P1 |
| **SL type selector (Stop vs StopLimit)** | Yes | N/A | Enum exists (`StopLossType`), not in UI | P1 |
| **Confirmation dialog** | Yes | N/A | Yes (`showing_confirmation`, `ConfirmYes/No`) | Done |
| **Validation error display** | Yes (inline) | N/A | Yes (`errors: Vec<(String, String)>`) | Done |
| **Order type selector (Market/Limit/Stop)** | Yes (dropdown) | N/A | No (market only) | P1 |
| **Limit price input** | Yes | N/A | No | P1 |
| **Stop price input** | Yes | N/A | No | P1 |
| **Trailing amount/percent input** | Yes (for trailing SL) | N/A | No | P2 |
| **Symbol link (cross-panel sync)** | Yes | N/A | Yes (`LinkMode`, symbol propagation) | Done |
| **Bracket chart toggle** | No (TV auto-shows) | N/A | Yes (`bracket_active`, `SetBracketMode`) | Done |
| **Hotkey for bracket placement** | No (TV: click chart) | N/A | Yes (`B` key, `Tab` to toggle side) | Done |

---

## 11. Risk Management

| Feature | TradingView | IB API | Hand of Midas (Current) | Priority |
|---|---|---|---|---|
| **Risk/reward calculation** | Yes | No (client-side) | Yes (`RiskReward` struct, `calculate_risk_reward()`) | Done |
| **Dollar risk display** | Yes | No | Yes (`dollar_risk()`) | Done |
| **Dollar reward display** | Yes | No | Yes (`dollar_reward()`) | Done |
| **Risk % display** | Yes | No | Yes (`risk_pct` field) | Done |
| **Directional validation (TP/SL on correct side)** | Yes (prevents invalid placement) | No (IB rejects server-side) | Yes (`check_bracket_direction()`, panel validation) | Done |
| **Max position size guard** | No | No (margin check server-side) | Partial (`reference_price` for notional calculation) | P2 |
| **Account equity % risk limit** | No (indicator only) | No (margin system) | No | P2 |

---

## Gap Analysis Summary

### Already Strong (Done)

Hand of Midas has a solid foundation:

- Market bracket orders (entry + TP + SL) with full pipeline
- Chart visualization with 6 lifecycle states and visual differentiation
- Draft bracket interaction (drag TP/SL, submit, save, cancel, toggle SL)
- Order state machine with 11 states and validated transitions
- Risk/reward calculation and display
- Dockable order panel with symbol linking
- Keyboard shortcuts for bracket placement

### Critical Gaps (P1 -- Must Have)

These are table-stakes features that every trading platform needs:

1. **Limit order entry** -- both standalone and as bracket parent
2. **Stop order entry** -- standalone entry orders
3. **Order type selector** in the order panel UI
4. **Live order modification** -- dragging TP/SL on active brackets, modifying via panel
5. **Live order cancellation** -- cancel working orders at IB
6. **Bracket modification for live orders** -- move TP/SL while position is open
7. **TP/SL offset and percentage input modes** in the panel
8. **Stop-Limit as SL type** exposed in the UI
9. **Chart visualization for limit and stop orders** (not just market brackets)
10. **Right-click context menu** for bracket legs (action is emitted but menu not rendered)

### Important Gaps (P2 -- Should Have)

Features serious traders expect:

1. **Trailing stop** as bracket SL child (both amount and percentage)
2. **Limit bracket** (limit entry + TP + SL)
3. **OCA grouping** wired to IB
4. **Adaptive algo** support (IB's most popular algo)
5. **Dollar amount and percentage position sizing**
6. **Risk-based position sizing** (auto-calculate quantity from risk %)
7. **Shift+drag entire bracket** on chart
8. **Cancel all orders** button
9. **Order deactivation/reactivation** cycle
10. **Trailing stop animated chart line**
11. **Primary exchange** disambiguation

### Power User Gaps (P3 -- Nice to Have)

1. Conditional orders (price, time, volume triggers)
2. TWAP/VWAP algo orders
3. Adjustable stops
4. GTD, IOC, OPG time-in-force options in UI
5. MOC/LOC order types
6. Market-if-Touched, Limit-if-Touched
7. Trailing Stop-Limit
8. Directed exchange routing UI
9. Order preview / what-if
10. GoodAfterTime scheduling

---

## Sources

- [TradingView Bracket Orders Documentation](https://www.tradingview.com/charting-library-docs/latest/trading_terminal/trading-concepts/brackets/)
- [TradingView Orders Documentation](https://www.tradingview.com/charting-library-docs/latest/trading_terminal/trading-concepts/orders/)
- [TradingView Order Ticket Documentation](https://www.tradingview.com/charting-library-docs/latest/trading_terminal/order-ticket/)
- [TWS API Basic Orders](https://interactivebrokers.github.io/tws-api/basic_orders.html)
- [TWS API Adjustable Stops](https://interactivebrokers.github.io/tws-api/adjustable_stops.html)
- [TWS API Order Conditioning](https://interactivebrokers.github.io/tws-api/order_conditions.html)
- [TWS API Order Class Reference](https://interactivebrokers.github.io/tws-api/classIBApi_1_1Order.html)
- [IB Order Types and Algos](https://www.interactivebrokers.com/en/trading/orders/fok.php)
- [TradingView TP/SL on Chart](https://www.tradingview.com/support/solutions/43000664282-can-i-add-stop-loss-take-profit-levels-on-the-chart/)
- [TradingView Long/Short Position Tools](https://www.tradingview.com/support/solutions/43000475660-how-to-use-long-and-short-position-drawing-tools/)
