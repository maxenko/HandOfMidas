# TradingView Order Entry UX -- Comprehensive Reference

> Research compiled April 2026 from TradingView screenshots (IB-connected account) and official TradingView documentation. This document catalogs every order-entry surface, field, mode, and interaction pattern observable in TradingView's trading terminal, for use as a UX reference when designing Hand of Midas order entry.

---

## Table of Contents

- [1. Order Panel Layout](#1-order-panel-layout)
- [2. Order Types](#2-order-types)
- [3. Quantity / Sizing Modes](#3-quantity--sizing-modes)
- [4. Exit Strategies (Take Profit / Stop Loss)](#4-exit-strategies-take-profit--stop-loss)
- [5. Multiple Exit Levels (Partial Exits)](#5-multiple-exit-levels-partial-exits)
- [6. Price Input Modes](#6-price-input-modes)
- [7. Time in Force](#7-time-in-force)
- [8. Exchange Routing](#8-exchange-routing)
- [9. Outside RTH (Extended Hours)](#9-outside-rth-extended-hours)
- [10. DOM (Depth of Market)](#10-dom-depth-of-market)
- [11. Order Modification](#11-order-modification)
- [12. Chart Integration](#12-chart-integration)
- [13. Order Presets](#13-order-presets)
- [14. Order Confirmation and Instant Mode](#14-order-confirmation-and-instant-mode)
- [15. UX Design Patterns Worth Adopting](#15-ux-design-patterns-worth-adopting)

---

## 1. Order Panel Layout

The TradingView order panel is a right-docked panel that appears when a broker is connected. It has two main tabs at the top: **Order** and **DOM**.

### Panel Anatomy (top to bottom)

| Section | Contents |
|---|---|
| **Header** | Ticker symbol, Order/DOM tab selector, preset selector (gear icon) |
| **Bid/Ask Bar** | Two-button bar: `Sell [bid]` (red) / `Buy [ask]` (blue), with spread displayed between them |
| **Order Type Tabs** | Four tabs: Market, Limit, Stop, Stop Limit |
| **Price Input(s)** | One or two price fields depending on order type, with reference labels (e.g., "Ask", "Bid") |
| **Quantity Section** | Mode selector dropdown + numeric input + total value display |
| **Exits Section** | Take Profit and Stop Loss rows, each with toggle, mode dropdown, price input, and tick offset |
| **Settings Section** | Time in Force, Routing, Outside RTH checkboxes |
| **Order Info** | Tick value, trade value |
| **Action Button** | Blue "Start creating order" or "Place order" button |

### Bid/Ask Bar Detail

The bar is split into two halves:

```
[ Sell 315.50 ] 0.25 [ Buy 315.75 ]
   (red bg)    spread    (blue bg)
```

- Left half (red): sell at bid price
- Right half (blue): buy at ask price
- Center: spread value
- Clicking either half sets the order side (buy/sell) and pre-fills the price
- Real-time price updates reflected in the buttons

### Panel Modes

The panel can operate in two modes:

- **Docked Panel** -- stays pinned to the right side of the chart, does not obscure the chart area
- **Dialog Mode** -- floating window that can be repositioned, undocked from the chart edge

Users switch between modes by docking/undocking the panel.

---

## 2. Order Types

TradingView exposes exactly **four order types** through its order ticket when connected to Interactive Brokers. These appear as horizontally arranged tabs at the top of the order form.

| Order Type | Tab Label | Price Fields | Behavior |
|---|---|---|---|
| **Market** | `Market` | None | Executes immediately at current market price. No price guarantee. |
| **Limit** | `Limit` | 1: Limit price | Executes only at the specified price or better. Not guaranteed to fill. |
| **Stop** | `Stop` | 1: Stop (trigger) price | Becomes a market order when stop price is reached. Used for breakout entries or stop-loss exits. |
| **Stop Limit** | `Stop Limit` | 2: Stop price + Limit price | Becomes a limit order when stop price is reached. Combines trigger control with price precision. |

### Stop Limit Field Relationship

From screenshots, the Stop Limit order shows both fields with a reference offset between them:

```
Stop price:  3.72  (Bid + 73 ticks)
Limit price: 3.71  (Stop - 1 tick)
```

The limit price is expressed as an offset from the stop price (e.g., "Stop - 1 tick"), making it easy for the user to define the maximum acceptable slippage from the trigger.

### Order Types NOT Present

The following order types available through IB's native TWS are **not** exposed in TradingView's UI:

- Trailing Stop / Trailing Stop Limit
- Market-on-Close (MOC) / Limit-on-Close (LOC)
- Market-on-Open (MOO) / Limit-on-Open (LOO)
- Adaptive Algo orders
- VWAP orders
- Pegged orders
- IB Algo orders (Accumulate/Distribute, TWAP, etc.)

This represents a deliberate simplification -- TradingView focuses on the four most commonly used order types and handles complexity through bracket exits rather than exotic order types.

---

## 3. Quantity / Sizing Modes

The quantity section provides a dropdown selector with **five distinct modes** for specifying position size. This is one of TradingView's most sophisticated UX features.

| Mode | Label | Input | Calculation | Example |
|---|---|---|---|---|
| **Shares** | `Shares` | Number of shares | Direct -- user specifies exact share count | `100 shares` |
| **USD** | `USD` | Dollar amount | `shares = amount / price` | `$5,000 -> 15 shares @ 315.75` |
| **% Balance** | `% balance` | Percentage of account | `shares = (balance * pct) / price` | `10% of $50k = $5k -> 15 shares` |
| **Risk USD** | `Risk, USD` | Dollar risk amount | `shares = risk / (entry - stop)` | `$100 risk, $2 stop distance -> 50 shares` |
| **Risk % Balance** | `Risk, % balance` | Risk as % of account | `shares = (balance * pct) / (entry - stop)` | `1% of $50k = $500 risk, $2 stop -> 250 shares` |

### Risk Mode Mechanics

The two risk-based modes are the most interesting from a UX perspective. They require a Stop Loss to be defined in order to calculate position size.

**Formula:** `Position Size = Risk Amount / |Entry Price - Stop Loss Price|`

From the AVGO screenshot:
```
Entry:     315.75 (Ask)
Stop Loss: 314.52 (118 ticks below)
Risk:      14.46 USD
Shares:    = 14.46 / (315.75 - 314.52) = 14.46 / 1.23 ~= 11.76 -> rounds to shares
Total:     3,868.67 USD
```

Key behaviors observed:
- The risk field and the stop loss field are **bidirectionally linked** -- changing either one recalculates the other
- When risk mode is active, the total trade value is displayed alongside the risk amount
- Fractional shares are supported (observed: "Buy -1.4285 ASPN" in screenshot 6)

### Display Format

The quantity section shows:
```
Risk, USD    [ 14.46 ]    3868.67 USD
              ^risk         ^total value
```

---

## 4. Exit Strategies (Take Profit / Stop Loss)

The **Exits** section appears below the quantity section and contains two rows: Take Profit (TP) and Stop Loss (SL). Each row is independently togglable.

### Exit Row Anatomy

Each exit row contains:

```
[Toggle] Take Profit  [Mode ▼]  [ Price ]  ( offset ticks )
```

| Component | Description |
|---|---|
| **Toggle** | On/off switch. When off, the exit is not included with the order. |
| **Mode dropdown** | Selects how the price is specified: Price, Ticks, or Percentage |
| **Price input** | The absolute price or offset value |
| **Tick offset** | Shows the distance from entry in ticks (read-only when in Price mode, editable when in Ticks mode) |

### Exit Price Modes

| Mode | Input | Reference Point | Example |
|---|---|---|---|
| **Price** | Absolute price | N/A | TP at 316.10 |
| **Ticks** | Number of ticks from entry | Entry price | TP at +75 ticks from entry |
| **Percentage** | % distance from entry | Entry price | TP at +1.5% from entry |

### Bracket Order Behavior

When both TP and SL are enabled, TradingView creates a **bracket order** (also called OCO -- One-Cancels-Other):

- Entry order is the parent
- TP and SL are child orders that bracket the position
- When one child fills, the other is automatically canceled
- If the entry order is canceled, both children are also canceled

### TP/SL Direction Logic

| Side | Take Profit | Stop Loss |
|---|---|---|
| **BUY** | Above entry price | Below entry price |
| **SELL** | Below entry price | Above entry price |

From screenshot 5 (SELL Stop Limit):
```
Entry (Stop): 3.72
TP:           2.97  (below entry -- profit on short)
SL:           3.69  (above? No -- 3.69 is BELOW 3.72, -2 ticks -- unusual, may be a protective stop for a short below entry)
```

Note: The SL at 3.69 for a SELL at 3.72 means the SL is -2 ticks below the entry, which would represent a very tight stop on a short position (the position would be stopped out for a small profit if price drops to 3.69 and then reverses). This is an uncommon but valid configuration.

### Observed TP/SL Toggle States

From the screenshots, TP and SL can be independently enabled or disabled:

| Screenshot | TP | SL |
|---|---|---|
| AVGO Limit BUY | Enabled (316.10, +75 ticks) | Enabled (314.52, -118 ticks) |
| ASPN Stop Limit SELL | Disabled (2.97, +75 ticks) | Enabled (3.69, -2 ticks) |
| ASPN Stop Limit BUY | Disabled | Enabled (4.42, -143 ticks) |

---

## 5. Multiple Exit Levels (Partial Exits)

TradingView supports **up to 4 exit levels** per order in Paper Trading mode (and with supported brokers). This enables partial position exits at different price targets.

### How It Works

1. Click the **"Add level"** button in the Exits section of the order ticket
2. In the dialog, set quantity (as % of total position) and price for each TP/SL pair
3. Each level specifies what percentage of the position to close at that price

### Configuration Rules

- Multiple exit levels always come as **paired TP/SL**
- Each level shows the percentage of the main order's quantity allocated to that level
- To use one SL with multiple TPs: set all SL levels to the same price
- The percentages across all levels must sum to 100%

### Example: 3-Level Partial Exit

| Level | % of Position | Take Profit | Stop Loss |
|---|---|---|---|
| 1 | 33% | +50 ticks | -25 ticks |
| 2 | 33% | +100 ticks | -25 ticks |
| 3 | 34% | +200 ticks | -25 ticks |

This creates three separate bracket orders, each managing a portion of the position.

### Saving Multi-Level Configurations

Multi-level exit configurations can be saved as part of an **order preset**, allowing quick reuse of complex exit strategies.

---

## 6. Price Input Modes

TradingView uses context-aware price inputs that show both the absolute price and its relationship to a reference point.

### Reference Points

| Order Type | Field | Reference | Example Display |
|---|---|---|---|
| Limit BUY | Limit price | Ask | `315.75 (Ask)` |
| Limit SELL | Limit price | Bid | `315.50 (Bid)` |
| Stop BUY | Stop price | Ask + offset | `2.99 (Ask - 72)` |
| Stop SELL | Stop price | Bid + offset | `3.72 (Bid + 73 ticks)` |
| Stop Limit | Limit price | Stop + offset | `3.71 (Stop - 1 tick)` |
| TP (BUY) | TP price | Entry + offset | `316.10 (75 ticks)` |
| SL (BUY) | SL price | Entry - offset | `314.52 (118 ticks)` |

### Key UX Patterns

1. **Dual display**: Every price field simultaneously shows the absolute value and the tick offset from its reference point
2. **Reference auto-selection**: The reference point changes based on order side and type
3. **Tick arithmetic**: Users can think in ticks (relative) or absolute price -- the UI shows both
4. **Smart defaults**: When switching order types, prices are pre-filled relative to current bid/ask

### Tick Value Display

The order info section at the bottom shows the tick value in account currency:

```
Tick value: 0.1 USD
```

This tells the user the dollar impact of each tick change, which is essential for risk calculations on instruments with varying tick values.

---

## 7. Time in Force

TradingView exposes **two time-in-force options** through its order panel when connected to Interactive Brokers:

| TIF | Label | Behavior |
|---|---|---|
| **DAY** | `Day` | Order expires at end of current trading session |
| **GTC** | `Good Till Cancel` | Order persists until filled or manually canceled (IB auto-cancels after 90 calendar days) |

### Time-in-Force Options Available via IB (but NOT in TradingView UI)

Interactive Brokers natively supports additional TIF types that TradingView does not expose:

| TIF | Description | In TV? |
|---|---|---|
| DAY | Expires end of session | Yes |
| GTC | Persists until filled/canceled | Yes |
| IOC | Immediate or Cancel -- fill immediately or cancel unfilled portion | No |
| FOK | Fill or Kill -- fill entirely immediately or cancel entirely | No |
| GTD | Good Till Date -- expires on a specific date | No |
| OPG | At the Opening -- execute at market open | No |
| DTC | Day Till Cancelled -- complex variant | No |

This is another area where TradingView simplifies -- the two most common TIF options cover the vast majority of retail trading scenarios.

### Default Behavior

- **Market orders**: Always effectively IOC (immediate execution)
- **Limit/Stop orders**: Default to DAY unless user selects GTC
- **Bracket exits**: Inherit TIF from parent order in most cases

---

## 8. Exchange Routing

The TradingView order panel provides a **Routing** dropdown with extensive exchange selection when connected to Interactive Brokers.

### Observed Exchanges (from screenshot)

| Exchange | Full Name | Type |
|---|---|---|
| **SMART** | IB SmartRouting | Algorithmic (default) |
| AMEX | NYSE American | Stock exchange |
| NYSE | New York Stock Exchange | Stock exchange |
| CBOE | Chicago Board Options Exchange | Options/stock exchange |
| PHLX | Philadelphia Stock Exchange | Options exchange |
| ISE | International Securities Exchange | Options exchange |
| CHX | Chicago Stock Exchange | Stock exchange |
| ARCA | NYSE Arca | ECN/exchange |
| NASDAQ | Nasdaq Stock Market | Stock exchange |
| DRCTEDGE | Direct Edge | ECN |
| BEX | Nasdaq BX | Stock exchange |
| BATS | Cboe BZX Exchange | Stock exchange |
| EDGEA | Cboe EDGA Exchange | Stock exchange |
| BYX | Cboe BYX Exchange | Stock exchange |
| IEX | Investors Exchange | Stock exchange |
| EDGX | Cboe EDGX Exchange | Stock exchange |
| FOXRIVER | Fox River | ATS |
| PEARL | MIAX PEARL | Options/equities exchange |
| NYSENAT | NYSE National | Stock exchange |
| LTSE | Long-Term Stock Exchange | Stock exchange |
| MEMX | Members Exchange | Stock exchange |
| PSX | Nasdaq PSX | Stock exchange |
| T24X | 24 Exchange | 24-hour trading venue |

### SMART Routing Explained

IB's SMART routing is the default and recommended setting. It is IB's proprietary order-routing algorithm that:

1. **Scans all connected exchanges** in real-time for the best available price
2. **Considers transaction costs** -- includes exchange fees/rebates in routing decisions
3. **Dynamically re-routes** -- if a better price appears while the order is pending, SMART will redirect
4. **Includes dark pools** -- routes to 8+ dark pools for potential price improvement on large orders
5. **Handles spread legs independently** -- each leg of a multi-leg order can be routed to different venues

### When to Choose a Specific Exchange

| Scenario | Routing Choice | Reason |
|---|---|---|
| General trading | SMART | Best execution, default |
| Price improvement on large orders | SMART | Dark pool access |
| Maker rebate hunting | Specific exchange | Some exchanges pay higher maker rebates |
| Options (specific exchange) | PHLX, ISE, CBOE | Specific option class may have better liquidity |
| Extended hours (24h) | T24X | 24-hour trading venue |
| IEX philosophy | IEX | Anti-HFT exchange with speed bump |
| Listed stock preference | NYSE, NASDAQ | Route to listing exchange |

### Note for Hand of Midas

Most retail traders should never change from SMART. The exchange dropdown is a power-user feature. Consider defaulting to SMART and making the full list accessible but not prominent.

---

## 9. Outside RTH (Extended Hours)

The order panel provides **two separate checkboxes** for extended-hours execution:

| Checkbox | Label | Applies To |
|---|---|---|
| **Outside RTH** | `Outside RTH` | The main order (entry) |
| **Outside RTH Take Profit** | `Outside RTH Take Profit` | The take profit bracket only |

### What "Outside RTH" Means

Regular Trading Hours (RTH) for US equities are 9:30 AM - 4:00 PM ET. "Outside RTH" allows orders to execute during:

- **Pre-market**: 4:00 AM - 9:30 AM ET
- **After-hours**: 4:00 PM - 8:00 PM ET

### Behavior by Order Type

| Order Type | Outside RTH Available? | Notes |
|---|---|---|
| Limit | Yes | Most common use case for extended hours |
| Market | No | Market orders in extended hours are too risky |
| Stop | Limited | Trigger may not fire outside RTH depending on broker |
| Stop Limit | Limited | Same limitation as stop orders |

### Why Separate TP Checkbox?

The separate "Outside RTH Take Profit" checkbox exists because:

1. A trader may want their entry to execute only during RTH but allow the take profit to fill in extended hours (e.g., earnings play where price gaps overnight)
2. Stop losses outside RTH are generally not offered separately -- the risk of extended hours stop execution (thin liquidity, wide spreads) is considered too dangerous for automatic protective exits

### Visibility in Order List

Orders eligible for extended hours execution display an **"Outside RTH" instruction** label in the Orders tab of the Trading Panel, making it easy to identify which orders can fill outside normal hours.

---

## 10. DOM (Depth of Market)

The DOM tab in TradingView provides a Level 2 order book view alongside order placement capabilities.

### DOM Layout

The DOM is a vertical price ladder with three columns:

```
 Buy Vol  |  Price  |  Sell Vol
----------|---------|----------
          |  316.00 |  1,200
          |  315.90 |    800
          |  315.80 |    450
   300    |  315.75 |         <-- Ask (best offer)
   500    |  315.50 |         <-- Bid (best bid)
   200    |  315.40 |
   150    |  315.30 |
```

| Component | Description |
|---|---|
| **Price column** | Central column showing price levels, each row = one tick |
| **Buy volume** | Left column -- aggregate buy order volume at each price level |
| **Sell volume** | Right column -- aggregate sell order volume at each price level |
| **Bid highlight** | Best bid price row highlighted (blue) |
| **Ask highlight** | Best ask price row highlighted (red) |
| **Position indicator** | Shows total open position size at bottom of DOM |

### DOM Order Placement

| Action | How | Result |
|---|---|---|
| Place limit BUY | Click left column cell at desired price | Limit buy order at that price |
| Place limit SELL | Click right column cell at desired price | Limit sell order at that price |
| Place stop BUY | Ctrl+Click (Win) / Cmd+Click (Mac) on right column | Stop buy at that price |
| Place stop SELL | Ctrl+Click (Win) / Cmd+Click (Mac) on left column | Stop sell at that price |

### DOM Position Management

| Button | Location | Action |
|---|---|---|
| **Flatten** | Bottom of DOM | Close entire position at market |
| **Reverse** | Bottom of DOM | Close position and open opposite at market |
| **Cancel (X)** | Next to order label | Cancel individual pending order |

### Requirements

- Requires a connected broker that provides Level 2 data
- Not all brokers/instruments support DOM
- Interactive Brokers provides Level 2 for most US equities and futures

---

## 11. Order Modification

TradingView provides multiple methods for modifying pending (working) orders after placement.

### Modification Methods

| Method | Where | How |
|---|---|---|
| **Order Panel** | Right panel | Click "Modify Order" to reopen the order ticket with current values |
| **Chart drag** | On chart | Drag the order's horizontal price line up or down |
| **Pencil icon** | Order list | Click pencil icon next to any working order |
| **Mobile: long press** | Mobile app | Tap and hold an order to open modification menu |
| **Mobile: drag** | Mobile chart | Drag order line directly on mobile chart |

### What Can Be Modified

| Field | Modifiable? | Notes |
|---|---|---|
| Price | Yes | Drag on chart or edit in panel |
| Quantity | Yes | Edit in order ticket |
| Order type | Yes | Can change between limit/stop/etc. |
| Time in Force | Yes | Can switch between DAY and GTC |
| Take Profit | Yes | Can add, remove, or change price |
| Stop Loss | Yes | Can add, remove, or change price |
| Side (Buy/Sell) | No | Must cancel and create new order |

### Important Limitations

1. **Market orders** cannot be modified (they execute immediately)
2. **Some brokers** (e.g., Binance) do not support modification -- the order must be canceled and re-placed
3. **Bracket sensitivity**: Some brokers (e.g., Bitget) cancel brackets when the parent order's price or quantity is modified, requiring TP/SL to be re-set
4. **Filled orders** obviously cannot be modified -- they become positions

### Modification Confirmation

By default, modifications require confirmation. In "instant mode" (no confirmation), dragging an order on the chart immediately submits the modification.

---

## 12. Chart Integration

TradingView's chart-based order management is one of its strongest UX features. Orders are fully visual and interactive.

### Order Visualization on Chart

| Element | Appearance | Information Shown |
|---|---|---|
| **Pending order** | Horizontal dashed line with label | Order type, side, quantity, price |
| **Take Profit** | Green horizontal line above/below entry | TP price, profit in ticks/USD |
| **Stop Loss** | Red horizontal line below/above entry | SL price, loss in ticks/USD |
| **Position** | Solid horizontal line at entry price | Entry price, P&L, quantity |
| **Order projection** | Semi-transparent preview | Shows where the order will be before placement |

### Order Projection System

An **order projection** is a visual preview of an order that has not yet been submitted. Key characteristics:

1. Appears on the chart as soon as the user starts configuring an order
2. Updates in real-time as the user changes price, quantity, or brackets in the order ticket
3. Can be manipulated directly on the chart by dragging
4. **Bidirectional sync**: changes on the chart are reflected in the order ticket and vice versa
5. Does not send anything to the broker until the user confirms placement

### Chart Interaction Patterns

| Action | How | Result |
|---|---|---|
| **Create order from chart** | Click "+" on price scale at a price level | Opens order ticket pre-filled at that price |
| **Drag order price** | Click and drag order line | Moves order to new price level |
| **Drag bracket** | Click and drag TP or SL line | Adjusts that bracket independently |
| **Drag all together** | Shift + drag order line | Moves order and all brackets together, preserving offsets |
| **Quick Buy/Sell** | Click Buy/Sell buttons in chart legend | Opens order ticket or places instant order |

### Buy/Sell Buttons on Chart

The chart legend area displays two buttons:

```
AVGO  O: 315.20  H: 316.50  L: 314.80  C: 315.75   [Sell 315.50] [Buy 315.75]
                                                        (red)         (blue)
```

| Button State | Meaning |
|---|---|
| Filled background (red/blue) | Broker connected, trading active |
| White/unfilled background | Broker not connected or non-tradable symbol |

### Bid/Ask Visualization

TradingView can display bid/ask levels directly on the chart:

- **Bid line**: Horizontal line on chart at current bid price
- **Ask line**: Horizontal line on chart at current ask price
- **Bid/Ask labels**: Price labels on the price scale showing bid and ask
- Configurable in chart settings

### Position P&L Display

When a position is open, the chart shows:

- Entry price line (solid)
- Current P&L in dollars and percentage
- P&L color: green for profit, red for loss
- Unrealized P&L updates in real-time with price movement

---

## 13. Order Presets

Order presets allow traders to save complete order configurations for quick reuse.

### What Is Saved in a Preset

| Parameter | Saved? |
|---|---|
| Order type (Market/Limit/Stop/Stop Limit) | Yes |
| Price (as relative offset, not absolute) | Yes |
| Quantity mode and value | Yes |
| Take Profit (mode, offset) | Yes |
| Stop Loss (mode, offset) | Yes |
| Multiple exit levels | Yes |
| Time in Force | Yes |
| Routing | Yes |
| Outside RTH settings | Yes |

### Creating a Preset

1. Configure the order ticket with desired parameters
2. Click the **Order presets** button in the order ticket header
3. Select **"Save order preset"**
4. Name the preset and verify selected parameters
5. Click **Save**

### Applying a Preset

- Select from the preset menu in the order ticket header
- Presets appear as a dropdown list
- Selecting a preset fills all saved parameters into the order ticket

### Hotkey Integration

With **"placing orders without confirmation" mode** enabled:

| Hotkey | Action |
|---|---|
| `Shift+B` | Place BUY order with the currently applied preset |
| `Shift+S` | Place SELL order with the currently applied preset |

This enables extremely fast order placement for scalpers and active traders who use consistent position sizing.

---

## 14. Order Confirmation and Instant Mode

TradingView supports two order submission modes.

### Standard Mode (Confirmation Required)

1. User configures order in ticket or on chart
2. Clicks "Place order" or equivalent action
3. **Confirmation dialog** appears showing order summary
4. User reviews and clicks "Confirm" or "Cancel"
5. Order is submitted to broker

### Instant Mode (No Confirmation)

When enabled in trading settings:

1. User configures order
2. Clicks "Place order" or uses hotkey
3. **Order is immediately submitted** -- no confirmation dialog
4. Applies to chart-based actions too (dragging to modify sends immediately)

### Order Summary Display

Before confirmation (or in the action button), TradingView shows a human-readable order summary:

```
"Sell -50 ASPN @ 3.72 STOP 3.71 LIMIT"
"Buy -1.4285 ASPN @ 2.99 STOP 2.99 LIMIT"
```

Format: `[Side] [Quantity] [Symbol] @ [Price] [Type]`

Note: negative quantity appears in some displays (likely indicating the total value or a short position context).

---

## 15. UX Design Patterns Worth Adopting

Based on this analysis, the following TradingView UX patterns are particularly well-designed and worth considering for Hand of Midas.

### High-Priority Patterns

| Pattern | Why It Works | Complexity |
|---|---|---|
| **Dual price display** (absolute + tick offset) | Eliminates mental math; traders think in both frames | Medium |
| **Risk-based sizing modes** | Professional risk management built into order flow | Medium |
| **Bidirectional chart-panel sync** | Users can work in whichever mode feels natural | High |
| **Order projection** (preview before submit) | Reduces errors; builds confidence | High |
| **Independent TP/SL toggles** | Not every order needs both exits | Low |
| **Bracket drag on chart** | Fastest way to set TP/SL visually | High |
| **Shift+drag for grouped move** | Power user efficiency | Medium |

### Medium-Priority Patterns

| Pattern | Why It Works | Complexity |
|---|---|---|
| **Order presets** | Consistency and speed for repeated strategies | Medium |
| **Hotkey order placement** | Sub-second order entry for active traders | Medium |
| **Bid/Ask in chart legend** | Always-visible context without panel | Low |
| **Multiple exit levels** | Professional partial-exit strategies | High |
| **Outside RTH per-component** | Granular control over extended hours behavior | Low |

### Patterns to Simplify or Skip

| TradingView Pattern | Recommendation | Reason |
|---|---|---|
| 23 exchange routing options | Default to SMART, hide rest behind "Advanced" | Overwhelming for most users |
| Fractional share quantities | Support but don't emphasize | Edge case |
| DOM tab | Defer to later phase | Requires Level 2 data infrastructure |
| Instant mode (no confirmation) | Default OFF, allow opt-in | Safety first for new platform |

### Key Insight: Progressive Disclosure

TradingView's order panel succeeds because it uses **progressive disclosure** effectively:

- **Default view**: Order type tabs, price, quantity, and a simple TP/SL toggle
- **One click deeper**: Risk sizing modes, tick offsets, multiple exit levels
- **Settings menu**: TIF, routing, RTH options
- **Advanced**: DOM, presets, instant mode, hotkeys

This means a beginner can place a market order in 2 clicks, while a professional can configure a multi-level bracket order with risk-based sizing and specific exchange routing -- all in the same panel.

---

## Appendix: Field-by-Field Screenshot Annotations

### Screenshot 1: AVGO Limit Order BUY

```
Order Type:       Limit (tab selected)
Side:             BUY (blue "Buy 315.75" button active)
Bid/Ask:          Sell 315.50 / Buy 315.75, spread 0.25
Price:            315.75 (labeled "Ask")
Quantity Mode:    Risk, USD
Risk Amount:      14.46
Trade Value:      3,868.67 USD
Take Profit:      ENABLED -- Price mode, 316.10, +75 ticks
Stop Loss:        ENABLED -- Price mode, 314.52, -118 ticks
Time in Force:    Day
Routing:          SMART
Outside RTH:      Unchecked
Outside RTH TP:   Unchecked
Tick Value:       0.1 USD
Action:           "Start creating order" (blue button)
```

### Screenshot 5: ASPN Stop Limit SELL

```
Order Type:       Stop Limit (tab selected)
Side:             SELL
Stop Price:       3.72 (Bid + 73 ticks)
Limit Price:      3.71 (Stop - 1 tick)
Quantity Mode:    Risk, USD (implied)
Risk Amount:      1.00
Trade Value:      -185.50 USD
Take Profit:      DISABLED (2.97, +75 ticks -- shown but toggled off)
Stop Loss:        ENABLED (3.69, -2 ticks)
Order Summary:    "Sell -50 ASPN @ 3.72 STOP 3.71 LIMIT"
```

### Screenshot 6: ASPN Stop Limit BUY

```
Order Type:       Stop Limit (tab selected)
Side:             BUY
Stop Price:       2.99 (Ask - 72)
Limit Price:      2.99 (Stop -- same as stop, zero slippage)
Quantity Mode:    Risk (implied)
Risk Amount:      2.04
Trade Value:      -4.27 USD
Take Profit:      DISABLED
Stop Loss:        ENABLED (4.42, -143 ticks)
Order Summary:    "Buy -1.4285 ASPN @ 2.99 STOP 2.99 LIMIT"
```

---

## Sources

- [TradingView: Depth of Market (DOM)](https://www.tradingview.com/support/solutions/43000516459-depth-of-market-dom-what-it-is-and-how-traders-can-use-it/)
- [TradingView: Chart Trading - Order Projection](https://www.tradingview.com/support/solutions/43000736178-chart-trading-order-projection/)
- [TradingView: Multiple Take Profit and Stop Loss Levels](https://www.tradingview.com/support/solutions/43000772334-multiple-take-profit-and-stop-loss-levels/)
- [TradingView: How to Trade on TradingView](https://www.tradingview.com/support/solutions/43000756695-how-to-trade-on-tradingview/)
- [TradingView: Stop Limit Orders](https://www.tradingview.com/support/solutions/43000754945-stop-limit-orders/)
- [TradingView: Brackets on the Chart](https://www.tradingview.com/support/solutions/43000659802-i-want-to-use-brackets-on-the-chart/)
- [TradingView: How to Trade During Extended Hours](https://www.tradingview.com/support/solutions/43000647250-how-to-trade-during-extended-hours/)
- [TradingView: Chart Trading Key Features and Advantages](https://www.tradingview.com/support/solutions/43000766334-chart-trading-on-tradingview-key-features-and-advantages/)
- [TradingView: Order Presets](https://www.tradingview.com/support/solutions/43000742709-what-are-order-presets/)
- [TradingView: Buy/Sell Button Style](https://www.tradingview.com/blog/en/explaining-the-new-buy-sell-button-style-19793/)
- [TradingView: Seamless Order Design on Chart](https://www.tradingview.com/blog/en/seamless-order-design-on-chart-46889/)
- [TradingView: Bracket Orders (Charting Library Docs)](https://www.tradingview.com/charting-library-docs/latest/trading_terminal/trading-concepts/brackets/)
- [TradingView: Order Ticket (Charting Library Docs)](https://www.tradingview.com/charting-library-docs/latest/trading_terminal/order-ticket/)
- [TradingView: Orders (Charting Library Docs)](https://www.tradingview.com/charting-library-docs/latest/trading_terminal/trading-concepts/orders/)
- [TradingView: Extended Trading Hours in Paper Trading](https://www.tradingview.com/blog/en/extended-trading-hours-in-paper-trading-49738/)
- [TradingView: Optimize Trading with Order Presets](https://www.tradingview.com/blog/en/optimize-trading-with-order-presets-48237/)
- [Interactive Brokers: SMART Routing / Best Price Execution](https://www.interactivebrokers.com/en/trading/smart-routing.php)
- [Interactive Brokers: Order Types and Algos](https://www.interactivebrokers.com/en/trading/ordertypes.php)
- [Interactive Brokers: Time in Force](https://www.interactivebrokers.com/en/software/tws/usersguidebook/ordertypes/time_in_force_for_orders.htm)
- [Interactive Brokers: TradingView Integration](https://www.interactivebrokers.com/en/trading/tradingview-landing.php)
- [TradingView Order Book Guide (Financial Tech Wiz)](https://www.financialtechwiz.com/post/tradingview-order-book-dom/)
- [TradingView Order Execution Guide (Optimus Futures)](https://optimusfutures.com/blog/tradingview-order-execution/)
