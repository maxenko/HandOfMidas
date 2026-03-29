# 07 — Order Bridge

## Principle

**midas-chart is a drawing tool. midas-broker is an execution engine. midas-app is the glue.**

The chart crate never imports broker types. The broker crate never imports chart types.
All mapping between annotations and orders happens in midas-app.

## Bridge Data Model

```rust
// In midas-app (NOT in midas-chart or midas-broker):

/// Maps a chart annotation to a broker order.
/// Lives in the app layer — not persisted in annotation files.
struct OrderAnnotationLink {
    /// The annotation in the chart.
    annotation_id: AnnotationId,
    /// The chart containing this annotation.
    chart_id: ChartId,
    /// The broker order(s) mapped to this annotation.
    orders: BracketOrders,
}

struct BracketOrders {
    /// Entry order UUID (always present once submitted).
    entry_order_id: Uuid,
    /// TP order UUID (present if bracket has TP leg).
    tp_order_id: Option<Uuid>,
    /// SL order UUID (present if bracket has SL leg).
    sl_order_id: Option<Uuid>,
    /// IB OCA group ID linking TP and SL.
    oca_group: Option<String>,
}
```

## Lifecycle Flow

### 1. User Creates Bracket on Chart

```
User draws bracket (entry=185.50, TP=190.00, SL=182.00, Long)
    → midas-chart: Annotation { kind: Bracket { status: Draft }, external_id: None }
    → No broker involvement yet
```

### 2. User Submits Bracket as Order

```
User right-clicks bracket → "Submit Order"
    → midas-app builds orders:

    entry_order = LocalOrder {
        action: Buy,
        order_type: Limit,
        limit_price: 185.50,
        quantity: 100,
        tif: Day,
    };

    tp_order = LocalOrder {
        action: Sell,
        order_type: Limit,
        limit_price: 190.00,
        parent_id: Some(entry_order.id),
        tif: Gtc,
    };

    sl_order = LocalOrder {
        action: Sell,
        order_type: Stop,
        stop_price: 182.00,
        parent_id: Some(entry_order.id),
        oca_group: Some("bracket_001"),
        tif: Gtc,
    };

    → midas-broker: submit_bracket(entry, tp, sl)
    → midas-app: annotation.external_id = Some(entry_order.id.to_string())
    → midas-app: bracket.status = BracketStatus::Pending
    → midas-app: store OrderAnnotationLink
```

### 3. Entry Fills

```
midas-broker emits: FillEvent { order_id: entry_order.id, price: 185.48, qty: 100 }
    → midas-app receives on fill broadcast channel
    → midas-app looks up OrderAnnotationLink by entry_order_id
    → midas-app:
        bracket.status = BracketStatus::Active
        bracket.entry.label = Some("Filled @ 185.48")
        dirty.annotations += 1
    → midas-app creates fill marker:
        Annotation {
            kind: Marker {
                price: 185.48,
                timestamp: fill_time,
                icon: MarkerIcon::FilledCircle,
                color: BULL_COLOR,
                label: Some("Buy 100 @ 185.48"),
            },
            tags: vec!["fill".into()],
            external_id: Some(entry_order.id.to_string()),
        }
```

### 4. TP or SL Fills

```
midas-broker emits: FillEvent { order_id: tp_order.id, price: 190.05, qty: 100 }
    → midas-app:
        bracket.status = BracketStatus::Closed
        bracket dims (all lines α × 0.3)
    → midas-app creates exit marker:
        Marker { price: 190.05, icon: FilledCircle, color: BULL_COLOR, label: "TP Hit +2.5%" }
    → IB automatically cancels SL (OCA group)
    → midas-broker emits: CancelEvent { order_id: sl_order.id }
    → midas-app acknowledges, no visual change needed
```

### 5. User Modifies Bracket Leg

```
User drags TP line from 190.00 to 192.00
    → midas-chart: DragBracketLeg { leg: TakeProfit, new_price: 192.00 }
    → midas-app checks: is this bracket linked to live orders?
        YES → midas-broker: modify_order(tp_order_id, new_limit_price: 192.00)
        NO  → just update the annotation
```

### 6. User Cancels Order

```
User right-clicks bracket → "Cancel Order"
    → midas-app: midas-broker.cancel_order(entry_order_id)
    → (IB cascades to child orders)
    → midas-broker emits: CancelEvent for each order
    → midas-app: bracket.status = BracketStatus::Cancelled
    → bracket dims on chart
```

## State Mapping

| BracketStatus (chart) | OrderStatus (broker) | Visual |
|---|---|---|
| Draft | — (no order exists) | Dashed lines, full opacity |
| Pending | PendingSubmit, PreSubmitted, Submitted | Dotted lines, full opacity |
| PartialFill | PartiallyFilled | Solid lines, blinking entry |
| Active | Filled (entry), Submitted (TP/SL) | Solid lines, full opacity |
| Closed | Filled (TP or SL) | Solid lines, dimmed α×0.3 |
| Cancelled | Cancelled | Solid lines, dimmed α×0.3 |

The mapping is not 1:1. `BracketStatus` is a simplified visual state.
The app layer resolves the actual broker state into the appropriate visual.

## Order History Visualization

When the app loads historical fills from the database:

```rust
fn create_fill_markers(fills: &[FillRecord]) -> Vec<Annotation> {
    fills.iter().map(|fill| {
        let is_buy = fill.action == OrderAction::Buy;
        Annotation {
            id: AnnotationId(0), // store assigns real ID
            kind: AnnotationKind::Marker(MarkerAnnotation {
                price: fill.price,
                timestamp: fill.timestamp,
                icon: if is_buy { MarkerIcon::Triangle } else { MarkerIcon::InvTriangle },
                color: if is_buy { BULL_COLOR } else { BEAR_COLOR },
                size: 8.0,
                label: Some(format!("{} {} @ {:.2}", fill.action, fill.quantity, fill.price)),
            }),
            created_at: fill.timestamp,
            modified_at: fill.timestamp,
            visible: true,
            locked: true,     // historical fills are immutable
            tags: vec!["fill".into(), "history".into()],
            external_id: Some(fill.order_id.to_string()),
        }
    }).collect()
}
```

Historical fills are locked and tagged "history" so users can filter them.

## Safety Guards

### Live Trading Protection

The existing `allow_live = true` config guard in midas-broker applies.
Additionally, midas-app adds:

1. **Confirmation dialog** before submitting any bracket as an order
2. **Paper trading indicator** visible in the UI when connected to TWS paper (port 7497)
3. **No auto-submit** — brackets are always created as Draft, never auto-sent

### Order Modification Guard

When a user drags a bracket leg that's linked to a live order:

1. Show a warning: "This will modify your live order at IB. Continue?"
2. If the user confirms, send the modification
3. If the modification is rejected by IB, revert the bracket leg to its previous price
4. Log all modifications to the order audit trail

### Bracket Deletion Guard

Deleting a bracket linked to a live order:

1. "This bracket has active orders at IB. Cancel orders and delete?"
2. If confirmed: cancel all linked orders first, then delete the annotation
3. If orders can't be cancelled (filled/terminal): dim the annotation but don't delete

## Future: Multi-Leg Strategies

The `OrderBracket` model supports extension to complex strategies:

```rust
// Future — not v1:
pub struct OrderBracket {
    pub legs: Vec<BracketLeg>,     // N legs instead of fixed entry/TP/SL
    pub side: BracketSide,
    pub status: BracketStatus,
    pub strategy: Option<String>,   // "iron_condor", "calendar_spread", etc.
    pub quantity: Option<f64>,
}
```

The current 3-leg model (entry + TP + SL) is sufficient for v1. Multi-leg
strategies will require option chain integration (midas-broker already supports
`SecurityType::Option`).
