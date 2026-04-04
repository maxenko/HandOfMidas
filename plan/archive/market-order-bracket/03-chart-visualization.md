# 03 — Chart Visualization

> Chart-level bracket rendering, interaction, and status-driven styling
> for Market Order brackets.
>
> **Implementation Status (2026-04-02):**
> - Data types (`BracketLeg` P&L fields, `LegRole`, `dollar_risk()`/`dollar_reward()`): **COMPLETE**
> - Rendering functions (`leg_style()`, `bracket_labels()`, `bracket_zone_rects()`): **NOT STARTED**

---

## Table of Contents

- [1. Market Bracket vs Limit Bracket Visuals](#1-market-bracket-vs-limit-bracket-visuals)
- [2. Rendering Layers](#2-rendering-layers)
- [3. Line Styling by Status](#3-line-styling-by-status)
- [4. Price Labels and P&L Badges](#4-price-labels-and-pnl-badges)
- [5. Zone Fills](#5-zone-fills)
- [6. Drag Interaction](#6-drag-interaction)
- [7. Context Menu](#7-context-menu)
- [8. Chart Lifecycle](#8-chart-lifecycle)

---

## 1. Market Bracket vs Limit Bracket Visuals

Market brackets differ from limit-entry brackets in one key way: **there is
no pre-fill entry line to draw**. The entry price is unknown until the market
order fills.

| Aspect | Limit Entry Bracket | Market Entry Bracket |
|---|---|---|
| Entry line before fill | Solid at limit price | None (or dotted at last price as estimate) |
| Entry line after fill | Solid at fill price | Solid at fill price |
| TP/SL lines before fill | Dashed (waiting) | Dashed (waiting, <1 sec for market) |
| TP/SL lines after fill | Solid (live at exchange) | Solid (live at exchange) |
| Drawing flow | 3-click on chart | Order panel submission |
| Time in "pending" state | Seconds to hours | Milliseconds |

### 1.1 The Transient Pending State

For market orders, the `Pending` status (entry not yet filled) typically
lasts under 1 second during market hours. The visual sequence is:

1. User clicks "Place Order" in the order panel
2. `BracketStatus::Pending` — TP/SL lines appear as dashed
3. Market order fills (near-instant)
4. `BracketStatus::Active` — entry line appears at fill price, TP/SL go solid
5. Bracket is now visually identical to a filled limit bracket

Because the pending state is so brief, we optimize for the `Active` state
as the primary visual mode.

---

## 2. Rendering Layers

Market bracket rendering uses the existing annotation layer system from the
widget architecture:

```
Layer 6: Zone fills (TP zone green, SL zone red, α=0.04-0.08)
Layer 7: Bracket leg lines (entry, TP, SL)
Layer 8: (unused by brackets — markers layer)
Layer 9: Crosshair (always topmost GPU)
Layer 10: iced overlay (price labels, P&L badges, R:R ratio)
```

All bracket rendering reuses existing GPU pipelines:
- **Lines**: `HLinePipeline` (horizontal lines at price levels)
- **Zone fills**: `GridPipeline` (wide rectangle with low alpha)
- **Text**: iced overlay (same as axis labels)

No new shaders or GPU pipelines are needed.

---

## 3. Line Styling by Status

### 3.1 Style Table

| BracketStatus | Entry Line | TP Line | SL Line | Zone Fill | Opacity |
|---|---|---|---|---|---|
| `Pending` | Dotted, blue | Dashed, green | Dashed, red | None | 0.7 |
| `Active` | Solid, blue-gray | Solid, green | Solid, red | Yes | 1.0 |
| `Closed` | Solid, dimmed | Solid, dimmed | Solid, dimmed | Dimmed | 0.3 |
| `Cancelled` | Solid, dimmed | Solid, dimmed | Solid, dimmed | None | 0.2 |

### 3.2 Color Palette

Using the existing chart theme colors (from `midas-core` config):

```rust
// Default bracket colors (RGBA, linear space)
const BRACKET_ENTRY_COLOR: [f32; 4]  = [0.55, 0.65, 0.80, 1.0]; // Blue-gray
const BRACKET_TP_COLOR: [f32; 4]     = [0.20, 0.78, 0.35, 1.0]; // Green
const BRACKET_SL_COLOR: [f32; 4]     = [0.90, 0.25, 0.25, 1.0]; // Red
const BRACKET_TP_ZONE: [f32; 4]      = [0.20, 0.78, 0.35, 0.06]; // Green, 6% alpha
const BRACKET_SL_ZONE: [f32; 4]      = [0.90, 0.25, 0.25, 0.06]; // Red, 6% alpha
```

### 3.3 LineStyle Application

```rust
/// Chart-local leg role enum. Defined in midas-chart — NOT imported from
/// midas-broker. The chart crate must not depend on broker types.
/// The app layer maps BracketRole (broker) to LegRole (chart) when
/// creating annotations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LegRole {
    Entry,
    TakeProfit,
    StopLoss,
}

impl OrderBracket {
    /// Compute line style for a leg based on bracket status.
    /// Uses chart-local LegRole, NOT broker BracketRole, to respect
    /// the dependency boundary (midas-chart has zero broker imports).
    pub fn leg_style(&self, role: LegRole) -> (LineStyle, f32, [f32; 4]) {
        let base_color = match role {
            LegRole::Entry => BRACKET_ENTRY_COLOR,
            LegRole::TakeProfit => BRACKET_TP_COLOR,
            LegRole::StopLoss => BRACKET_SL_COLOR,
        };

        let (style, width, alpha_mult) = match self.status {
            BracketStatus::Draft => (LineStyle::Dashed, 1.0, 0.8),
            BracketStatus::Pending => (LineStyle::Dotted, 1.0, 0.7),
            BracketStatus::PartialFill => (LineStyle::Solid, 1.5, 0.9),
            BracketStatus::Active => (LineStyle::Solid, 1.5, 1.0),
            BracketStatus::Closed => (LineStyle::Solid, 1.0, 0.3),
            BracketStatus::Cancelled => (LineStyle::Solid, 1.0, 0.2),
        };

        let mut color = base_color;
        color[3] *= alpha_mult;
        (style, width, color)
    }
}
```

> **Note (2026-04-02)**: `LegRole` has been implemented in the codebase.
> The alternative approach (app layer sets color directly) was considered
> but `LegRole` was chosen for its explicit semantics.

---

## 4. Price Labels and P&L Badges

### 4.1 Label Layout

Each bracket leg displays a label on the right side of the chart (in the
Y-axis margin area), similar to TradingView:

```
                                                    ┌──────────────────┐
───────── TP ─────────────────────────────────────── │ TP 192.00 +$650 │
                                                    └──────────────────┘
                        TP zone (green, α=0.06)

                                                    ┌──────────────────┐
───────── Entry ────────────────────────────────────│ ▲ 185.50  100sh  │
                                                    └──────────────────┘
                        SL zone (red, α=0.06)

                                                    ┌──────────────────┐
───────── SL ─────────────────────────────────────── │ SL 182.00 -$350 │
                                                    └──────────────────┘
```

### 4.2 Label Content

**Entry label**: `▲ {fill_price}  {qty}sh` (▲ for buy, ▼ for sell)

**TP label**: `TP {price}  +${pnl}` or `TP {price}  +{pct}%`

**SL label**: `SL {price}  -${pnl}` or `SL {price}  -{pct}%`

The display mode (dollar P&L vs percentage) is a user preference stored
in app config. Default: dollar P&L.

### 4.3 R:R Badge

When both TP and SL are present, display the risk:reward ratio as a
compact badge near the entry line:

```
R:R 1.86:1
```

Positioned just to the left of the entry price label.

### 4.4 Label Rendering

Labels render as iced overlay elements (Layer 10), not GPU text. This
keeps the GPU pipeline simple and matches how axis labels are already
rendered:

```rust
/// Generate overlay labels for a bracket annotation.
pub fn bracket_labels(
    bracket: &OrderBracket,
    camera: &Camera2D,
    chart_bounds: Rectangle,
) -> Vec<WidgetLabel> {
    let mut labels = Vec::new();

    // Entry label
    let entry_y = camera.price_to_screen_y(bracket.entry.price);
    labels.push(WidgetLabel {
        text: format_entry_label(bracket),
        position: Point::new(chart_bounds.x + chart_bounds.width - LABEL_MARGIN, entry_y),
        color: BRACKET_ENTRY_COLOR,
        anchor: Anchor::RightCenter,
        background: Some(LABEL_BG_COLOR),
    });

    // TP label
    if let Some(ref tp) = bracket.take_profit {
        let y = camera.price_to_screen_y(tp.price);
        labels.push(WidgetLabel {
            text: format_tp_label(tp, bracket),
            position: Point::new(chart_bounds.x + chart_bounds.width - LABEL_MARGIN, y),
            color: BRACKET_TP_COLOR,
            anchor: Anchor::RightCenter,
            background: Some(LABEL_BG_COLOR),
        });
    }

    // SL label
    if let Some(ref sl) = bracket.stop_loss {
        let y = camera.price_to_screen_y(sl.price);
        labels.push(WidgetLabel {
            text: format_sl_label(sl, bracket),
            position: Point::new(chart_bounds.x + chart_bounds.width - LABEL_MARGIN, y),
            color: BRACKET_SL_COLOR,
            anchor: Anchor::RightCenter,
            background: Some(LABEL_BG_COLOR),
        });
    }

    labels
}
```

---

## 5. Zone Fills

### 5.1 Zone Regions

Two filled zones between the entry and TP/SL:

- **TP zone**: Rectangle from entry price to TP price, full chart width.
  Color: green with α=0.06.
- **SL zone**: Rectangle from SL price to entry price, full chart width.
  Color: red with α=0.06.

Zones are only drawn when `status == Active` (entry has filled, TP/SL live).

### 5.2 Zone Rendering

Zones render as wide rectangles on Layer 6 using `GridPipeline`:

```rust
fn bracket_zone_rects(
    bracket: &OrderBracket,
    camera: &Camera2D,
    chart_width: f32,
) -> Vec<RectInstance> {
    let mut rects = Vec::new();

    if bracket.status != BracketStatus::Active {
        return rects;
    }

    let entry_y = camera.price_to_screen_y(bracket.entry.price);

    // TP zone
    if let Some(ref tp) = bracket.take_profit {
        let tp_y = camera.price_to_screen_y(tp.price);
        let (top, bottom) = if tp_y < entry_y {
            (tp_y, entry_y)
        } else {
            (entry_y, tp_y)
        };
        rects.push(RectInstance {
            x: 0.0,
            y: top,
            width: chart_width,
            height: bottom - top,
            color: BRACKET_TP_ZONE,
        });
    }

    // SL zone
    if let Some(ref sl) = bracket.stop_loss {
        let sl_y = camera.price_to_screen_y(sl.price);
        let (top, bottom) = if sl_y < entry_y {
            (sl_y, entry_y)
        } else {
            (entry_y, sl_y)
        };
        rects.push(RectInstance {
            x: 0.0,
            y: top,
            width: chart_width,
            height: bottom - top,
            color: BRACKET_SL_ZONE,
        });
    }

    rects
}
```

---

## 6. Drag Interaction

### 6.1 Draggable Legs

TP and SL lines are draggable when the bracket is `Active` (entry filled,
children are live at exchange). The entry line is **never draggable** because
the parent market order is terminal (Filled).

**Hit zone**: ±6px from the line's Y position (same as horizontal levels).

### 6.2 Drag Flow

```
MouseDown on TP/SL hit zone
    │
    ▼
InteractionState → DraggingBracketLeg { bracket_id, leg: TakeProfit|StopLoss }
    │
    ▼
MouseMove → update leg.price via camera.screen_y_to_price(mouse_y)
    │         Recalculate P&L labels in real-time
    │         Enforce directional constraints:
    │           - Long TP must stay above entry
    │           - Long SL must stay below entry
    │           - Short TP must stay below entry
    │           - Short SL must stay above entry
    ▼
MouseUp → finalize
    │
    ▼
Emit ChartAction::DragBracketLeg { annotation_id, leg, new_price }
    │
    ▼
midas-app receives action:
    │
    ▼
Look up OrderAnnotationLink by annotation_id
    │
    ▼
Send BrokerCommand::ModifyBracketLeg { order_id, new_price }
    │
    ▼
Engine modifies order at IB (same ib_order_id, updated price)
```

### 6.3 Snap Behavior

While dragging, the price snaps to the nearest tick increment for the
instrument. For stocks, this is typically $0.01. The snap value comes
from the contract metadata (resolved during order creation).

If tick size is unknown, no snapping is applied (freeform drag).

### 6.4 Drag Visual Feedback

During drag:
- The dragged line becomes thicker (2.0px → 3.0px)
- A translucent preview line shows the new position
- The P&L badge updates in real-time
- The zone fill resizes to match the new TP/SL position
- The R:R badge updates in real-time

---

## 7. Context Menu

### 7.1 Right-Click on Bracket Leg

When the user right-clicks on any bracket leg (entry, TP, or SL), show a
context menu:

**On Entry line (Active bracket)**:
```
┌─────────────────────────┐
│ BUY 100 AAPL @ 185.50   │
│─────────────────────────│
│ Cancel Bracket           │
│ Close Position (Market)  │
│─────────────────────────│
│ Hide Bracket             │
└─────────────────────────┘
```

**On TP line**:
```
┌─────────────────────────┐
│ TP: SELL 100 @ 192.00    │
│─────────────────────────│
│ Modify Price...          │
│ Cancel Take Profit       │
│─────────────────────────│
│ Hide Bracket             │
└─────────────────────────┘
```

**On SL line**:
```
┌─────────────────────────┐
│ SL: SELL 100 @ 182.00    │
│─────────────────────────│
│ Modify Price...          │
│ Cancel Stop Loss         │
│─────────────────────────│
│ Hide Bracket             │
└─────────────────────────┘
```

### 7.2 "Close Position (Market)"

Sends a market order to close the entire position. This is a convenience
shortcut equivalent to cancelling TP/SL and placing a new market order.

Implementation:
1. Cancel TP and SL children
2. Place a new market order (opposite side, same quantity)

This is a separate `BrokerCommand::ClosePosition` (future scope — not part of
this plan's implementation, but the context menu slot is reserved).

---

## 8. Chart Lifecycle

### 8.1 When Bracket Appears on Chart

1. `BracketCreated` event received by midas-app
2. App creates `OrderAnnotationLink` mapping
3. App creates `OrderBracket` annotation with `status: Pending`
4. Annotation added to chart's `AnnotationStore` for the bracket's symbol
5. All charts displaying that symbol show the bracket (per-symbol sharing)

### 8.2 When Bracket Updates

1. `BracketStatusChanged` event received
2. App looks up `OrderAnnotationLink` by `parent_id`
3. App updates `OrderBracket.status` in the `AnnotationStore`
4. If `entry_fill_price` is provided, update `entry.price`
5. Recalculate P&L labels using fill price and quantity
6. Charts redraw (dirty flag: `annotations` generation incremented)

### 8.3 When Bracket is Closed/Cancelled

1. `BracketStatusChanged { status: TakeProfitHit | StopLossHit | Cancelled }`
2. App updates `OrderBracket.status` to `Closed` or `Cancelled`
3. Lines dim to α × 0.3 (closed) or α × 0.2 (cancelled)
4. Zone fills disappear
5. Bracket remains visible on chart (dimmed) until user hides or a new session

### 8.4 Historical Brackets

On app startup, load all brackets from the database where the parent has
`bracket_role = "PARENT"`:
- Filled brackets → `Active` or `Closed` based on child statuses
- Cancelled brackets → `Cancelled`
- This provides visual history of past trades on the chart

Closed/cancelled brackets older than N days can be auto-hidden (configurable).
Default: show last 30 days of bracket history.
