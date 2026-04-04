# 04 — Order Entry UI

> Order panel widget, TradingView-style input, and broker bridge for
> Market Order brackets.
>
> **Implementation Status (2026-04-02):** NOT STARTED.
> The cross-workspace bridge (Phase 4.0) is the critical prerequisite.
> See `05-testing-and-rollout.md` §5 Phase 4 for updated scope and estimates.

---

## Table of Contents

- [1. Order Panel Widget](#1-order-panel-widget)
- [2. Input Modes](#2-input-modes)
- [3. Risk/Reward Calculator](#3-riskreward-calculator)
- [4. Submission Flow](#4-submission-flow)
- [5. Confirmation Dialog](#5-confirmation-dialog)
- [6. Quick Trade Mode](#6-quick-trade-mode)
- [7. State Management](#7-state-management)
- [8. Keyboard Shortcuts](#8-keyboard-shortcuts)

---

## 1. Order Panel Widget

### 1.1 Panel Location

The order panel is a floating widget that appears when the user presses
the **Trade** button on the toolbar or uses the `T` keyboard shortcut on
a chart. It is **not** a permanent panel in the layout — it floats over the
active chart and can be dismissed.

Alternatively, for frequent traders, the order panel can be docked as a
side panel (right edge) via workspace layout settings.

### 1.2 Layout Design (TradingView Reference)

```
┌─────────────────────────────────────────────┐
│  ┌─────────┐ ┌─────────┐                   │
│  │   BUY   │ │  SELL   │   [ Market  ▾ ]   │   ← Side toggle + type
│  └─────────┘ └─────────┘                   │
├─────────────────────────────────────────────┤
│  Symbol:  AAPL          Last: 185.50        │   ← From active chart
├─────────────────────────────────────────────┤
│  Qty:     [ 100      ]   shares             │
├─────────────────────────────────────────────┤
│                                             │
│  ☑ Take Profit                              │
│     [ Price ▾ ]  [ 192.00     ]             │
│     +3.50%  |  +$6.50/sh  |  +$650.00      │   ← Auto-calculated
│                                             │
├─────────────────────────────────────────────┤
│                                             │
│  ☑ Stop Loss                                │
│     [ Price ▾ ]  [ 182.00     ]             │
│     Type: [ Stop ▾ ]                        │
│     -1.89%  |  -$3.50/sh  |  -$350.00      │   ← Auto-calculated
│                                             │
├─────────────────────────────────────────────┤
│  Risk: $350     Reward: $650    R:R 1.86:1  │
├─────────────────────────────────────────────┤
│  ⚠ PAPER TRADING                            │   ← Account type indicator
│            [ Place Market Order ]            │
└─────────────────────────────────────────────┘
```

### 1.3 Side Toggle

Two large buttons at the top: **BUY** (green background) and **SELL**
(red background). Only one is active at a time. Clicking one deselects the
other and flips the TP/SL direction constraints.

### 1.4 Symbol and Price

Auto-populated from the active chart:
- **Symbol**: The chart's current symbol
- **Last price**: Most recent price from the chart's candle data (updated
  on each new bar or intra-bar update from the data provider). This is NOT
  a real-time tick stream — `midas-feed` streaming is not yet built (see
  Non-Goals). The price is accurate enough for bracket direction validation
  and R:R estimation but may lag by the bar interval.

These fields are read-only. To trade a different symbol, switch charts.

### 1.5 Account Type Indicator

The order panel prominently displays the account type (paper vs. live)
near the submit button. This is a standard safety practice for trading
platforms.

- **Paper trading**: Show "PAPER TRADING" label in yellow/orange background
- **Live trading**: Show "LIVE" label in red background with bold text
- **Disconnected**: Show "DISCONNECTED" in gray, disable submit button

The account type is derived from the broker connection state:
- Port 7497 (TWS paper) or 4002 (Gateway paper) → Paper
- Port 7496 (TWS live) or 4001 (Gateway live) → Live (only if `allow_live = true`)

### 1.6 Order Type Dropdown

For this plan, only "Market" is available. The dropdown exists as a
placeholder for future order types (Limit, Stop, Stop Limit) which will
be implemented in subsequent plans.

---

## 2. Input Modes

### 2.1 TP/SL Price Input Modes

Each of TP and SL has a mode dropdown with three options:

| Mode | Input | Example | How It Resolves |
|---|---|---|---|
| **Price** | Absolute price level | `192.00` | Used directly |
| **Offset** | Dollar offset from last | `+6.50` | `last_price + offset` |
| **Percent** | Percentage from last | `+3.50%` | `last_price * (1 + pct/100)` |

Default mode: **Price** (absolute).

### 2.2 Resolution to Absolute Price

All modes resolve to an absolute price before submission. The resolution
uses the **last traded price** at the moment of submission (not the price
when the field was entered).

```rust
fn resolve_price(
    mode: PriceInputMode,
    value: f64,
    last_price: f64,
    action: OrderAction,
    is_tp: bool,
) -> f64 {
    match mode {
        PriceInputMode::Absolute => value,
        PriceInputMode::Offset => {
            if (action == OrderAction::Buy) == is_tp {
                last_price + value.abs()   // TP above for buy, SL above for sell
            } else {
                last_price - value.abs()   // SL below for buy, TP below for sell
            }
        }
        PriceInputMode::Percent => {
            let factor = value.abs() / 100.0;
            if (action == OrderAction::Buy) == is_tp {
                last_price * (1.0 + factor)
            } else {
                last_price * (1.0 - factor)
            }
        }
    }
}
```

### 2.3 Auto-Fill Behavior

When the user enables the TP or SL checkbox, auto-fill a sensible default:
- **TP default**: Last price + 2% (configurable in settings)
- **SL default**: Last price - 1% (configurable in settings)

For SELL orders, the directions are reversed.

### 2.4 SL Type Sub-Selector

The Stop Loss section has an additional dropdown:

| SL Type | Behavior |
|---|---|
| **Stop** (default) | Becomes market order when stop price is hit |
| **Stop Limit** | Becomes limit order when stop price is hit. Shows additional "Limit price" field. |

When "Stop Limit" is selected, an additional input appears:
```
     Stop: [ 182.00  ]
     Limit: [ 181.50  ]    ← How far below stop to set the limit
```

The limit price defaults to `stop_price - 0.50` for stocks (configurable
tick offset).

---

## 3. Risk/Reward Calculator

### 3.1 Real-Time Calculation

As the user adjusts TP, SL, or quantity, the risk/reward section updates
in real-time:

```rust
struct RiskReward {
    risk_per_share: f64,     // |entry - SL|
    reward_per_share: f64,   // |TP - entry|
    total_risk: f64,         // risk_per_share * quantity
    total_reward: f64,       // reward_per_share * quantity
    risk_pct: f64,           // risk_per_share / entry * 100
    reward_pct: f64,         // reward_per_share / entry * 100
    ratio: f64,              // reward / risk
}

fn calculate_risk_reward(
    entry_price: f64,       // last traded price (estimate for market order)
    tp_price: Option<f64>,
    sl_price: Option<f64>,
    quantity: f64,
) -> Option<RiskReward> {
    let sl = sl_price?;
    let risk_per_share = (entry_price - sl).abs();
    if risk_per_share < f64::EPSILON {
        return None;
    }

    let reward_per_share = tp_price
        .map(|tp| (tp - entry_price).abs())
        .unwrap_or(0.0);

    Some(RiskReward {
        risk_per_share,
        reward_per_share,
        total_risk: risk_per_share * quantity,
        total_reward: reward_per_share * quantity,
        risk_pct: risk_per_share / entry_price * 100.0,
        reward_pct: reward_per_share / entry_price * 100.0,
        ratio: if risk_per_share > 0.0 { reward_per_share / risk_per_share } else { 0.0 },
    })
}
```

### 3.2 Display Format

```
Risk: $350.00 (1.89%)    Reward: $650.00 (3.51%)    R:R 1.86:1
```

Color coding:
- Risk amount: red
- Reward amount: green
- R:R ratio: green if ≥ 2.0, yellow if 1.0-2.0, red if < 1.0

### 3.3 Warnings

Display inline warnings for risky configurations:

| Condition | Warning |
|---|---|
| No TP and no SL | "No protection — naked market order" |
| No SL | "No stop loss — unlimited downside risk" |
| R:R < 1.0 | "Risk exceeds reward" |
| SL distance > 5% | "Large stop loss distance" |
| Quantity × price > account 5% | "Position exceeds 5% of account" (requires account data) |

---

## 4. Submission Flow

### 4.1 Submission Sequence

```
User clicks "Place Market Order"
    │
    ▼
Resolve TP/SL to absolute prices (using current last price)
    │
    ▼
Client-side validation:
    - Quantity > 0
    - TP on correct side (if enabled)
    - SL on correct side (if enabled)
    - SL limit price valid for StopLimit (if applicable)
    │ ── fail ──> show inline error, don't submit
    ▼
Show confirmation dialog (if enabled in settings)
    │ ── user cancels ──> return to panel
    ▼
Build MarketBracketParams:
    symbol: from active chart
    con_id: from contract cache (or None for engine to resolve)
    sec_type: Stock (default, or from chart context)
    exchange: "SMART"
    currency: "USD"
    action: from BUY/SELL toggle
    quantity: from input
    take_profit: { price } if enabled
    stop_loss: { stop_price, limit_price } if enabled
    │
    ▼
Send BrokerCommand::CreateMarketBracket(params) via mpsc channel
    │
    ▼
Dismiss order panel (or keep open for next trade, configurable)
    │
    ▼
Listen for BracketCreated event → show toast notification:
    "BUY 100 AAPL bracket submitted"
    │
    ▼
Listen for BracketStatusChanged → update toast:
    "BUY 100 AAPL filled @ 185.52  TP: 192.00  SL: 182.00"
```

### 4.2 Error Handling

If the engine returns an error (via `BrokerEvent::OrderError` or
`BrokerEvent::OrderRejected`):

1. Show error toast: "Order rejected: {reason}"
2. Re-open the order panel with previous values filled in
3. Highlight the problematic field (if determinable from error)

---

## 5. Confirmation Dialog

### 5.1 Dialog Content

```
┌────────────────────────────────────────────────┐
│  Confirm Market Order                          │
│                                                │
│  BUY 100 AAPL at Market                        │
│                                                │
│  Take Profit:  SELL 100 @ $192.00  (+$650.00)  │
│  Stop Loss:    SELL 100 @ $182.00  (-$350.00)  │
│                                                │
│  Risk/Reward: 1.86:1                           │
│                                                │
│  ☐ Don't show again                            │
│                                                │
│        [ Cancel ]    [ Confirm & Submit ]       │
└────────────────────────────────────────────────┘
```

### 5.2 Settings

```toml
[trading]
# Show confirmation dialog before submitting orders
confirm_orders = true
# Show confirmation when modifying live orders via drag
confirm_modifications = true
```

---

## 6. Quick Trade Mode

### 6.1 Overview

For experienced traders, a "quick trade" mode bypasses the full order panel.
The user can place a market bracket from the chart with minimal clicks:

1. Right-click on chart at a price level → context menu
2. Select "Buy Market Here" or "Sell Market Here"
3. TP and SL are auto-calculated using default offsets (from settings)
4. Bracket is submitted immediately (no panel, no confirmation if disabled)

### 6.2 Default Offsets

```toml
[trading.quick_trade]
enabled = false
default_quantity = 100
tp_offset_pct = 2.0   # TP at +2% from entry
sl_offset_pct = 1.0   # SL at -1% from entry
```

### 6.3 Chart Context Menu Integration

When right-clicking on a chart:

```
┌─────────────────────────────┐
│ Buy Market (100 AAPL)       │   ← Uses default quantity
│ Sell Market (100 AAPL)      │
│─────────────────────────────│
│ Place Order...              │   ← Opens full order panel
│─────────────────────────────│
│ Add Level                   │
│ Add Note                    │
└─────────────────────────────┘
```

Quick trade is disabled by default to prevent accidental orders.

---

## 7. State Management

### 7.1 OrderPanelState

```rust
/// State for the floating order entry panel.
pub struct OrderPanelState {
    /// Whether the panel is visible.
    pub visible: bool,
    /// Current side selection.
    pub side: OrderAction,
    /// Quantity input value.
    pub quantity: String,  // String for text input; parsed to f64 on submit
    /// Take profit enabled.
    pub tp_enabled: bool,
    /// Take profit input mode.
    pub tp_mode: PriceInputMode,
    /// Take profit input value (meaning depends on tp_mode).
    pub tp_value: String,
    /// Stop loss enabled.
    pub sl_enabled: bool,
    /// Stop loss input mode.
    pub sl_mode: PriceInputMode,
    /// Stop loss input value.
    pub sl_value: String,
    /// Stop loss type.
    pub sl_type: StopLossType,
    /// Stop limit price (only when sl_type == StopLimit).
    pub sl_limit_value: String,
    /// Validation errors to display inline.
    pub errors: Vec<(String, String)>,  // (field_name, error_message)
    /// Symbol (from active chart).
    pub symbol: String,
    /// Last known price (from chart candle data, not real-time tick stream).
    pub last_price: Option<f64>,
}

pub enum PriceInputMode {
    Absolute,
    Offset,
    Percent,
}

pub enum StopLossType {
    Stop,
    StopLimit,
}
```

### 7.2 Message Variants

Add to the app's `Message` enum:

```rust
// Order panel
OrderPanelToggle,
OrderPanelSetSide(OrderAction),
OrderPanelSetQuantity(String),
OrderPanelToggleTp(bool),
OrderPanelSetTpMode(PriceInputMode),
OrderPanelSetTpValue(String),
OrderPanelToggleSl(bool),
OrderPanelSetSlMode(PriceInputMode),
OrderPanelSetSlValue(String),
OrderPanelSetSlType(StopLossType),
OrderPanelSetSlLimit(String),
OrderPanelSubmit,
OrderPanelDismiss,
```

### 7.3 Lifecycle

1. `OrderPanelToggle` → set `visible = true`, populate `symbol` and `last_price` from active chart
2. User fills in fields → messages update `OrderPanelState`
3. `OrderPanelSubmit` → validate, build params, send command
4. `OrderPanelDismiss` → set `visible = false`, clear errors
5. On chart switch: update `symbol` and `last_price` if panel is open

---

## 8. Keyboard Shortcuts

| Key | Action | Context |
|---|---|---|
| `T` | Toggle order panel | Chart focused |
| `Enter` | Submit order | Order panel focused |
| `Escape` | Dismiss order panel | Order panel focused |
| `Tab` | Cycle between fields | Order panel focused |
| `B` | Set side to BUY | Order panel focused |
| `S` | Set side to SELL | Order panel focused |

These shortcuts are active only when the order panel is visible and focused,
to avoid conflicts with chart interaction shortcuts.
