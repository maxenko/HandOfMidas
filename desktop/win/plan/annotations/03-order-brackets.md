# 03 — Order Brackets

## Overview

An `OrderBracket` is a compound annotation representing a trade idea: an entry price,
an optional take-profit (TP), and an optional stop-loss (SL). Each leg is a horizontal
line anchored to a price and optionally a time. The area between TP and SL is shaded.

Brackets are the primary bridge between chart drawings and broker orders.
The chart crate knows them only as visual geometry. The app layer maps them to
`LocalOrder` instances in midas-broker.

## Data Model

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OrderBracket {
    /// The entry price line.
    pub entry: BracketLeg,
    /// Take-profit target. None if user hasn't set one yet.
    pub take_profit: Option<BracketLeg>,
    /// Stop-loss level. None if user hasn't set one yet.
    pub stop_loss: Option<BracketLeg>,
    /// Trade direction — determines which side TP/SL go on.
    pub side: BracketSide,
    /// Visual status. The chart crate uses this for styling only.
    pub status: BracketStatus,
    /// Display quantity (informational label, not order routing).
    pub quantity: Option<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BracketLeg {
    /// Price level for this leg.
    pub price: f64,
    /// Optional time anchor. None = full-width ray from left edge.
    /// Some(ts) = ray starting at timestamp, extending right.
    pub timestamp: Option<i64>,
    /// Override color. If None, derived from BracketSide + leg role.
    pub color: Option<[f32; 4]>,
    /// Line style for this leg.
    pub style: LineStyle,
    /// Line thickness in logical pixels.
    pub line_width: f32,
    /// Text shown next to the price label (e.g., "Entry 185.50", "TP +2.5%").
    pub label: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BracketSide {
    Long,   // entry below TP, above SL
    Short,  // entry above TP, below SL
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum BracketStatus {
    /// Being drawn on chart, not yet actionable.
    #[default]
    Draft,
    /// Submitted to broker, awaiting entry fill.
    Pending,
    /// Entry partially filled.
    PartialFill,
    /// Entry filled, TP/SL orders live at broker.
    Active,
    /// TP or SL triggered, position closed.
    Closed,
    /// User or broker cancelled.
    Cancelled,
}
```

## Visual Design

### Default Colors (Dark Theme)

| Element | Color | Alpha | Rationale |
|---|---|---|---|
| Long entry line | `[0.15, 0.65, 0.35, 0.9]` | 0.9 | Green, prominent |
| Long TP line | `[0.15, 0.65, 0.35, 0.5]` | 0.5 | Green, lighter |
| Long SL line | `[0.65, 0.15, 0.15, 0.5]` | 0.5 | Red, lighter |
| Long TP zone fill | `[0.15, 0.65, 0.35, 0.06]` | 0.06 | Green tint, very subtle |
| Long SL zone fill | `[0.65, 0.15, 0.15, 0.06]` | 0.06 | Red tint, very subtle |
| Short entry line | `[0.65, 0.15, 0.15, 0.9]` | 0.9 | Red, prominent |
| Short TP/SL | mirror of Long | | |
| Draft lines | Dashed | | Indicates not yet submitted |
| Pending lines | Dotted | | Awaiting fill |
| Active lines | Solid | | Position is live |
| Closed/Cancelled | Solid, dimmed α×0.3 | | Historical, faded |

### Zone Fill

The rectangular area between TP and SL is filled with a semi-transparent gradient:

```
TP ─────────────── (green zone, α = 0.06)
                   ← profit zone
Entry ─────────── (solid line)
                   ← risk zone
SL ─────────────── (red zone, α = 0.06)
```

Zone fills are `GridLineInstance` rects — same pipeline as VP bars. No new shader needed.

### Price Labels

Each leg gets a price badge on the Y axis (same style as current crosshair price badge):
- Background: leg color at α = 0.8
- Text: white
- Content: price formatted to tick size, optionally with P&L percentage

```
┌─────────┐
│ 185.50  │  ← entry badge
└─────────┘
┌─────────┐
│ 190.00  │  ← TP badge (+2.4%)
│  +2.4%  │
└─────────┘
```

Labels are rendered as iced overlay widgets (same mechanism as date labels and Y-axis labels),
not GPU text.

## Bracket Leg Constraints

| Constraint | Enforced | How |
|---|---|---|
| Long: TP > entry > SL | Yes | Swap if user drags past entry |
| Short: SL > entry > TP | Yes | Swap if user drags past entry |
| Entry required | Yes | Bracket can't exist without entry |
| TP/SL optional | Yes | User can add later |
| Min leg distance | No | Allow zero-distance (will be a horizontal line) |
| Max legs | 3 | Entry + TP + SL, no partial-TP chains yet |

## R:R Ratio Display

When both TP and SL are set, compute and display Risk:Reward ratio:

```rust
fn risk_reward(bracket: &OrderBracket) -> Option<f64> {
    let tp = bracket.take_profit.as_ref()?;
    let sl = bracket.stop_loss.as_ref()?;
    let risk = (bracket.entry.price - sl.price).abs();
    let reward = (tp.price - bracket.entry.price).abs();
    if risk < f64::EPSILON { return None; }
    Some(reward / risk)
}
```

Displayed as "R:R 2.5:1" near the entry line label.

## Interaction (Drawing a Bracket)

See [04-interaction.md](04-interaction.md) for the full interaction model.
Summary of the drawing flow:

1. User activates bracket tool (toolbar button or keyboard shortcut)
2. Click 1 → sets entry price (snaps to price grid)
3. Click 2 → sets TP (constrained to correct side)
4. Click 3 → sets SL (constrained to correct side)
5. Bracket created with `BracketStatus::Draft`
6. Escape at any point cancels drawing

After creation, individual legs can be dragged to adjust.

## Order Mapping (App Layer Only)

The chart crate produces `OrderBracket` as pure geometry. The app layer bridges to midas-broker:

```
OrderBracket.entry       → LocalOrder { order_type: Limit, limit_price: entry.price }
OrderBracket.take_profit → LocalOrder { order_type: Limit, parent_id: entry_order_id }
OrderBracket.stop_loss   → LocalOrder { order_type: Stop,  parent_id: entry_order_id }
```

IB bracket orders use OCA (One-Cancels-All) groups: when TP fills, SL is cancelled and vice versa.

The mapping is bidirectional:
- Chart → Broker: user submits bracket, app creates orders
- Broker → Chart: fill/cancel events update bracket status and create fill markers
