# 05 -- Chart Interaction Patterns for Order Types

> How different order types appear, animate, and respond to user interaction on the chart.
>
> References TradingView patterns and identifies where Hand of Midas can improve.

---

## Table of Contents

- [Design Principles](#design-principles)
- [Visual Language](#visual-language)
- [Order Type Visualizations](#order-type-visualizations)
- [Bracket Order Patterns](#bracket-order-patterns)
- [Drag Interaction Patterns](#drag-interaction-patterns)
- [Lifecycle Visual States](#lifecycle-visual-states)
- [Trailing Stop Animation](#trailing-stop-animation)
- [Context Menu Patterns](#context-menu-patterns)
- [Improvements Over TradingView](#improvements-over-tradingview)

---

## Design Principles

### 1. Order type is instantly identifiable from line style alone

A trader glancing at a chart with multiple orders must immediately distinguish a limit order from a stop order from a trailing stop without reading labels. This is achieved through consistent line styles.

### 2. Status is communicated through alpha and width, not color

Color encodes direction (green = bullish / buy, red = bearish / sell) and leg role (green = TP, red = SL). Status (draft, pending, active, closed) is communicated through line style, width, and opacity. This avoids overloading color semantics.

### 3. Draggable elements have visible affordances

Any line that can be dragged shows a cursor change on hover (`ResizeNS` for vertical price movement). Active brackets show hit zones extending 6px above and below the line for comfortable grab targets.

### 4. Dangerous actions require confirmation

Cancelling an active order, removing an SL from an open position, or submitting a large order all require confirmation. This is standard in TradingView and must be preserved.

### 5. Chart crate stays sans-IO

All visual computations (line positions, colors, hit zones, labels) live in `midas-chart`. The chart emits semantic `ChartAction` variants. The app layer handles broker communication, confirmation dialogs, and order routing. The chart never imports broker types.

---

## Visual Language

### Line Style Encoding

| Order Type | Line Style | Rationale |
|---|---|---|
| **Market entry** | Solid, 1.5px | Immediate execution -- solid = definite |
| **Limit order** | Dashed (6px dash, 4px gap) | Resting order -- dashes suggest "waiting" |
| **Stop order** | Dotted (4px dot spacing) | Trigger order -- dots suggest "conditional" |
| **Stop-Limit order** | Dash-dot (6px dash, 2px gap, 2px dot, 2px gap) | Hybrid -- combines dash (resting) and dot (trigger) |
| **Trailing stop** | Animated dashed (moving dash pattern) | Dynamic -- motion suggests the stop is alive and moving |
| **Take-profit child** | Inherits parent line style at 1.0px width | Thinner than entry to show subordination |
| **Stop-loss child** | Inherits parent line style at 1.0px width | Thinner than entry to show subordination |

### Color Encoding

| Element | Color (RGBA linear) | Hex Approx |
|---|---|---|
| Buy / Long entry | `[0.20, 0.78, 0.35, 1.0]` | #33C759 (green) |
| Sell / Short entry | `[0.90, 0.25, 0.25, 1.0]` | #E64040 (red) |
| Take-profit line | `[0.20, 0.78, 0.35, 1.0]` | #33C759 (green) |
| Stop-loss line | `[0.90, 0.25, 0.25, 1.0]` | #E64040 (red) |
| Trailing stop line | `[0.90, 0.65, 0.20, 1.0]` | #E6A633 (amber) |
| TP zone fill | `[0.20, 0.78, 0.35, 0.06]` | Green at 6% alpha |
| SL zone fill | `[0.90, 0.25, 0.25, 0.06]` | Red at 6% alpha |
| Neutral / info | `[0.70, 0.70, 0.70, 1.0]` | #B3B3B3 (gray) |

### Alpha Encoding by Status

| Status | Alpha Multiplier | Visual Effect |
|---|---|---|
| Draft (unsaved) | 0.50 | Barely visible -- exploratory |
| Draft (saved) | 0.65 | Slightly more prominent -- intentional |
| Pending | 0.80 | Noticeable but not commanding |
| Partial Fill | 0.90 | Nearly full -- attention-worthy |
| Active | 1.00 | Full opacity -- this is real money |
| Closed | 0.30 | Faded -- historical |
| Cancelled | 0.20 | Nearly invisible -- dismissed |

This system is already implemented in `OrderBracket::leg_style()` and works well.

---

## Order Type Visualizations

### Market Order (Current -- Already Implemented)

```
Price
  ^
  |  ================================ TP 192.00  +$650       (green, solid 1.0px)
  |  ::::::::::::::::::::::::::::::::  (green zone fill 6%)
  |  ================================ BUY @ 185.50  100sh    (green, solid 1.5px)
  |  ::::::::::::::::::::::::::::::::  (red zone fill 6%)
  |  ================================ SL 180.00  -$550       (red, solid 1.0px)
  |
  +-----------------------------------------------------------> Time
```

Entry price snaps to the current market price. Entry line is NOT draggable (market orders fill at whatever price is available). TP and SL lines are draggable in draft status.

### Limit Order (New)

```
Price
  ^
  |           Current Price: 185.50
  |  - - - - - - - - - - - - - - - -  LMT BUY @ 182.00      (green, dashed 1.5px)
  |
  +-----------------------------------------------------------> Time
```

The limit order line sits at the user-specified limit price, which is typically away from the current price:
- Buy limit: BELOW current price (buying on a dip)
- Sell limit: ABOVE current price (selling at a target)

**Key difference from market**: the entry line IS draggable because the price is user-defined, not market-defined.

**Label format by status**:
- Draft: `"LMT BUY @ 182.00"`
- Pending: `"LMT BUY @ 182.00  [hourglass]"`
- Submitted: `"LMT BUY @ 182.00  [working]"`
- Filled: `"[arrow] 182.00  100sh"`

### Limit Bracket (New)

```
Price
  ^
  |  - - - - - - - - - - - - - - - -  TP 192.00  +$1,000    (green, dashed 1.0px)
  |  ::::::::::::::::::::::::::::::::  (green zone fill 6%)
  |  - - - - - - - - - - - - - - - -  LMT BUY @ 182.00      (green, dashed 1.5px)
  |  ::::::::::::::::::::::::::::::::  (red zone fill 6%)
  |  - - - - - - - - - - - - - - - -  SL 175.00  -$700       (red, dashed 1.0px)
  |
  |           Current Price: 185.50
  |
  +-----------------------------------------------------------> Time
```

All three lines use dashed style (inheriting from the limit entry). Zone fills only appear when the bracket is Active (entry filled). While entry is pending, lines are dashed with 0.80 alpha.

**Critical interaction**: dragging the entry line updates TP and SL if they were specified as offsets (e.g., "TP = entry + $10"), but NOT if they were specified as absolute prices.

### Stop Order (New)

```
Price
  ^
  |  . . . . . . . . . . . . . . . .  STP BUY @ 190.00      (green, dotted 1.5px)
  |
  |           Current Price: 185.50
  |
  +-----------------------------------------------------------> Time
```

Stop orders are triggers:
- Buy stop: ABOVE current price (breakout entry)
- Sell stop: BELOW current price (breakdown entry)

**Visual distinction from limit**: dotted line vs dashed line. Dots convey "conditional" -- the order is not resting on the book, it triggers.

### Stop-Limit Order (New)

```
Price
  ^
  |  -.-.-.-.-.-.-.-.-.-.-.-.-.-.-.-  STP LMT BUY @ 190.00  (green, dash-dot 1.5px)
  |                                   Trigger: 190.00
  |                                   Limit: 191.00
  |
  +-----------------------------------------------------------> Time
```

The dash-dot line style distinguishes this from both limit (dashed) and stop (dotted). If the trigger and limit prices differ significantly, show two lines: a dotted trigger line and a dashed limit line.

### Trailing Stop (New)

```
Price
  ^
  |           Current Price: 192.50     (market has moved up from entry)
  |  ~ ~ ~ ~ ~ ~ ~ ~ ~ ~ ~ ~ ~ ~ ~ ~  TSL $2.50 @ 190.00   (amber, animated dashed)
  |
  |  ================================  Entry filled @ 185.50  (green, solid, dimmed)
  |
  +-----------------------------------------------------------> Time
```

The trailing stop line is distinctive:
- **Amber color** (not red) to distinguish from fixed SL
- **Animated dashes**: the dash pattern scrolls slowly rightward (2px/second), creating a subtle motion effect that communicates "this line is alive and tracking"
- **Label**: shows both the trail parameter ("TSL $2.50") and the current calculated stop price ("@ 190.00")
- **The line moves upward** as price makes new highs (for a long position). It never moves down.

**Animation implementation**:
- The dash pattern offset is a function of wall-clock time, not price
- `dash_offset = (time_seconds * 2.0) % (dash_len + gap_len)`
- This requires the chart to request redraws at ~10fps while a trailing stop is visible (not 60fps -- subtle motion is sufficient)

---

## Bracket Order Patterns

### Bracket with Market Entry (Current)

```
  [Save] [Submit] [X]
  ================================ BUY @ 185.50  R:R 1.86:1    (entry)
  ::::::::::::::::::::::::::::::::  (red zone)
  ================================ SL @ 180.00                   (SL)  [X]
```

Buttons appear only on Draft status. Entry line has [Save], [Submit], [X]. SL line has its own [X] to remove the SL leg.

**TradingView comparison**: TV does not show action buttons on chart lines. Instead, it uses a separate order ticket panel. Hand of Midas's on-chart buttons are an improvement -- they reduce the click distance for common actions.

### Bracket with Limit Entry (New)

```
  [Save] [Submit] [X]
  - - - - - - - - - - - - - - - -  TP 192.00                    (TP)
  ::::::::::::::::::::::::::::::::  (green zone, only if draft-saved or active)
  - - - - - - - - - - - - - - - -  LMT BUY @ 182.00  R:R 2.00:1 (entry)
  ::::::::::::::::::::::::::::::::  (red zone, only if draft-saved or active)
  - - - - - - - - - - - - - - - -  SL @ 177.00                   (SL)  [X]
```

Same button layout as market bracket. All lines inherit dashed style from the limit entry type. The entry line is draggable (unlike market entry).

### Bracket with Trailing Stop SL (New)

```
  ================================ TP 195.00  +$950              (TP, green solid)
  ::::::::::::::::::::::::::::::::  (green zone)
  ================================ Entry @ 185.50  100sh         (entry, green solid)
  ::::::::::::::::::::::::::::::::  (amber zone -- not red)
  ~ ~ ~ ~ ~ ~ ~ ~ ~ ~ ~ ~ ~ ~ ~   TSL $2.50 @ 183.00           (amber, animated)
```

The zone between entry and trailing SL uses amber fill instead of red, matching the trailing stop line color. This distinguishes "dynamic risk" from "fixed risk" at a glance.

### Bracket Lifecycle Transitions

```
Draft (unsaved)    Draft (saved)      Pending           Active
alpha=0.50         alpha=0.65         alpha=0.80        alpha=1.00
lines=dashed       lines=dashed       lines=dotted      lines=solid
buttons=visible    buttons=visible    buttons=hidden    buttons=[Cancel]
zones=hidden       zones=hidden       zones=hidden      zones=visible

Closed             Cancelled
alpha=0.30         alpha=0.20
lines=solid        lines=solid
buttons=hidden     buttons=hidden
zones=hidden       zones=hidden
```

The progression from dashed to dotted to solid mirrors the order's journey from "idea" to "working" to "real".

---

## Drag Interaction Patterns

### General Drag Rules

1. **Drag starts after 4px threshold** (already implemented via `PendingDrag` state). This prevents accidental drags during clicks.
2. **Cursor changes to `ResizeNS`** when hovering over a draggable line (already implemented via `HitZone`).
3. **Price snaps to tick size** during drag (not yet implemented -- requires contract tick size data from IB).
4. **Drag is constrained by directional rules**: a buy TP cannot be dragged below the entry price.

### Drag Behaviors by Order Type and Status

| Element | Draft | Pending | Active | Closed/Cancelled |
|---|---|---|---|---|
| Market entry | Not draggable | Not draggable | Not draggable | Not draggable |
| Limit entry | Draggable | Draggable (sends modify) | N/A (filled) | Not draggable |
| Stop entry | Draggable | Draggable (sends modify) | N/A (filled) | Not draggable |
| TP child | Draggable | Draggable (sends modify) | Draggable (sends modify) | Not draggable |
| SL child | Draggable | Draggable (sends modify) | Draggable (sends modify) | Not draggable |
| Trailing SL | Not draggable (set via params) | Not applicable | Not draggable (trails automatically) | Not draggable |

### Drag Constraints

**TP Constraints (Long/Buy)**:
- TP must remain ABOVE entry price
- Minimum distance: 1 tick above entry
- If dragged below entry: clamp to entry + 1 tick

**TP Constraints (Short/Sell)**:
- TP must remain BELOW entry price
- Minimum distance: 1 tick below entry
- If dragged above entry: clamp to entry - 1 tick

**SL Constraints (Long/Buy)**:
- SL must remain BELOW entry price
- If dragged above entry: clamp to entry - 1 tick

**SL Constraints (Short/Sell)**:
- SL must remain ABOVE entry price
- If dragged below entry: clamp to entry + 1 tick

**Limit Entry Constraints (Buy)**:
- Should warn (not prevent) if dragged above current price (becomes a marketable limit)
- If bracket: TP/SL move with entry if in offset mode

**Stop Entry Constraints (Buy)**:
- Must remain ABOVE current price
- If bracket: TP/SL move with entry if in offset mode

### Shift+Drag (Move Entire Bracket)

When Shift is held during a drag on any bracket leg:
1. Compute `delta = new_price - original_price` of the dragged leg
2. Apply `delta` to all three legs: `entry += delta`, `tp += delta`, `sl += delta`
3. All three lines move in lockstep
4. Directional constraints apply to the entry (e.g., limit buy cannot go above current price)
5. Release sends modification requests for all changed legs

**Visual feedback during shift-drag**: all three lines highlight (slightly brighter alpha) to indicate they are moving together. A thin vertical connecting line appears between the three price levels.

### Drag Feedback for Live Orders

When dragging a live order (Active/Submitted status):

1. **Optimistic update**: the line moves to the new position immediately during drag
2. **Ghost line**: a thin ghost of the original position remains visible (dashed, low alpha) until IB confirms the modification
3. **On confirmation**: ghost line fades out, solid line stays at new position
4. **On rejection**: line snaps back to original position with a brief red flash, error tooltip appears

This is an improvement over TradingView, which sometimes shows a delay between drag and confirmation without clear feedback.

---

## Lifecycle Visual States

### State Transition Diagram (Visual)

```
  +--------+          +---------+          +-----------+
  | Draft  |  Submit  | Pending |  IB ACK  | Submitted |
  | dashed |--------->| dotted  |--------->|  solid    |
  | a=0.50 |          | a=0.80  |          |  a=0.90   |
  +--------+          +---------+          +-----------+
                                                |
                                           Fill |
                                                v
  +-----------+         +--------+          +--------+
  | Cancelled |<--------|Active  |--------->| Closed |
  | solid     |  Cancel | solid  | TP/SL    | solid  |
  | a=0.20    |         | a=1.00 | fill     | a=0.30 |
  +-----------+         +--------+          +--------+
                            |
                       Partial|
                         Fill |
                            v
                      +----------+
                      | Partial  |
                      | solid    |
                      | a=0.90   |
                      +----------+
```

### Per-State Detail

#### Draft (Unsaved)

- **Lines**: dashed, 1.0px width, alpha 0.50
- **Labels**: `"BUY @ 185.50"`, `"SL @ 180.00"`, `"TP 192.00"`
- **Buttons**: [Save], [Submit], [X] on entry; [X] on SL
- **Zones**: hidden (no shaded regions)
- **Hit zones**: active on all lines (TP, SL draggable; entry draggable for limit/stop)
- **Behavior**: disappears when bracket mode is toggled off (unless saved)

#### Draft (Saved)

- **Lines**: dashed, 1.0px width, alpha 0.65
- **Labels**: same as unsaved with "saved" indicator
- **Buttons**: same
- **Zones**: hidden
- **Hit zones**: active
- **Behavior**: persists when bracket mode is toggled off; survives panel state reset

#### Pending (Submitted to IB)

- **Lines**: dotted, 1.0px width, alpha 0.80
- **Labels**: `"BUY @ 185.50  [hourglass]"` (hourglass unicode)
- **Buttons**: [Cancel] only (no more [Submit] or [Save])
- **Zones**: hidden
- **Hit zones**: entry not draggable (market) or draggable (limit/stop with modify); TP/SL draggable (sends modify)
- **Transition**: typically brief for market orders (sub-second to fill)

#### Partial Fill

- **Lines**: solid, 1.5px width, alpha 0.90
- **Labels**: `"BUY @ 185.50  [half-circle] 50/100sh"`
- **Buttons**: [Cancel] (cancels remaining)
- **Zones**: visible (entry to TP green, entry to SL red) -- same as Active
- **Hit zones**: TP/SL draggable
- **Special**: the partial fill icon ([half-circle]) draws attention to an order that needs monitoring

#### Active (Entry Filled, TP/SL Live)

- **Lines**: solid, 1.5px width, alpha 1.00
- **Labels**: `"[arrow] 185.50  100sh"`, `"TP 192.00  +$650"`, `"SL 180.00  -$550"`
- **Buttons**: [Cancel] on entry (cancels entire bracket)
- **Zones**: visible (green TP zone, red SL zone at 6% alpha)
- **Hit zones**: TP and SL draggable (sends modify to IB); entry NOT draggable (already filled)
- **This is the primary state** during a live trade

#### Closed (TP or SL Hit)

- **Lines**: solid, 1.0px width, alpha 0.30
- **Labels**: dimmed, show final P&L: `"TP HIT +$650"` or `"SL HIT -$550"`
- **Buttons**: none
- **Zones**: hidden
- **Hit zones**: none (not interactive)
- **Behavior**: fades out over time or remains as trade history (user preference)

#### Cancelled

- **Lines**: solid, 1.0px width, alpha 0.20
- **Labels**: `"CANCELLED"` or strikethrough
- **Buttons**: none
- **Zones**: hidden
- **Hit zones**: none
- **Behavior**: fades out quickly; option to remove from chart entirely

---

## Trailing Stop Animation

### Rendering Algorithm

The trailing stop requires special rendering because it moves with price:

```rust
// Pseudocode for trailing stop line position
fn trailing_stop_y(
    trail_amount: f64,
    highest_price_since_entry: f64,  // for long
    camera: &Camera2D,
) -> f32 {
    let trailing_level = highest_price_since_entry - trail_amount;
    camera.price_to_y(trailing_level)
}
```

### Animation Details

**Dash scroll animation**:
```rust
// In the chart's compute pass (runs every frame when trailing stop visible)
let time_seconds = wall_clock.elapsed_secs();
let dash_offset = (time_seconds * TRAIL_SCROLL_SPEED) % (DASH_LEN + GAP_LEN);

// The GridLineInstance for trailing stop includes a dash_offset field
// The shader shifts the dash pattern by this offset
```

Current `GridLineInstance` is `{ rect, color }`. For trailing stop animation, either:
1. Add a `dash_offset: f32` field (requires shader modification), or
2. Render the trailing stop as a separate draw call with a shifted UV, or
3. Use multiple short line segments computed on CPU (simpler, no shader change)

**Recommendation**: Option 3 (CPU-computed segments) for Phase 2. The trailing stop is a single line; the CPU cost of computing ~100 short segments is negligible. Shader-based animation can be optimized later.

### Trail History Visualization

An improvement over TradingView: show the trailing stop's path as a faint stepped line:

```
Price
  ^
  |  Current price: 195.00
  |  ~ ~ ~ ~ ~ ~ ~ ~ ~ ~ ~ ~ ~  TSL @ 192.50 (current)
  |                      .......  (trail history, very faint)
  |                .......
  |  ..............
  |  Entry @ 185.50
  +---------------------------------------------> Time
```

The trail history is a stepped line (horizontal until price makes a new high, then jumps up) rendered at alpha 0.15. This shows the trader how much the trailing stop has already protected.

---

## Context Menu Patterns

### Right-Click Menu Structure

#### On Draft Entry Line

```
+----------------------------+
| Modify Entry Price...      |
| Submit Bracket             |
| Save Bracket               |
| ----                       |
| Add Stop Loss              | (if SL missing)
| Add Take Profit            | (if TP missing)
| ----                       |
| Cancel Bracket             |
+----------------------------+
```

#### On Active Entry Line

```
+----------------------------+
| Cancel Bracket             |
| ----                       |
| Add Stop Loss              | (if SL missing)
| Add Take Profit            | (if TP missing)
| ----                       |
| Flatten Position           | (market close)
+----------------------------+
```

#### On Active TP Line

```
+----------------------------+
| Modify TP Price...         |
| Remove Take Profit         |
| ----                       |
| Convert to Trailing Stop   | (future)
+----------------------------+
```

#### On Active SL Line

```
+----------------------------+
| Modify SL Price...         |
| Remove Stop Loss           | (confirm!)
| ----                       |
| Change to Stop-Limit       |
| Change to Trailing Stop    |
+----------------------------+
```

### Implementation Notes

- Context menus are iced overlays, rendered by the app layer
- The chart emits `ChartAction::RightClickBracketLeg { annotation_id, leg, x, y }`
- The app layer determines which menu items are available based on bracket status, leg role, and order state
- Menu items map to app-layer commands (modify, cancel, etc.)
- "Modify Price..." opens an inline text input on the chart (or focuses the corresponding panel field)

---

## Improvements Over TradingView

### 1. On-Chart Action Buttons (Already Implemented)

TradingView requires navigating to a separate order ticket to submit or cancel a bracket. Hand of Midas places [Submit], [Save], [X], and [SL] buttons directly on the chart lines. This reduces the action distance for the most common bracket operations.

### 2. Six-State Lifecycle Visualization (Already Implemented)

TradingView uses approximately three visual states (pending, active, closed). Hand of Midas uses six (Draft, Pending, PartialFill, Active, Closed, Cancelled), each with distinct visual treatment. This gives traders more information at a glance.

### 3. Draft Save/Pin (Already Implemented)

TradingView brackets are either placed or not. Hand of Midas allows saving drafts that persist across panel toggles, enabling "what-if" exploration of trade ideas before committing.

### 4. Trailing Stop History Line (Planned)

TradingView shows only the current trailing stop level. Hand of Midas can show the trailing stop's path over time as a faint stepped line, giving visual feedback on how much profit the trail has protected.

### 5. Ghost Line on Modification (Planned)

When dragging a live order to modify it, Hand of Midas shows a ghost of the original position until IB confirms the change. TradingView has latency between drag and visual update with no clear feedback.

### 6. Amber Color for Trailing Stops (Planned)

TradingView uses red for all stop-loss types. Hand of Midas will use amber for trailing stops to distinguish "dynamic risk" from "fixed risk" at a glance. This small color distinction is meaningful during fast markets.

### 7. R:R Ratio on Entry Line (Already Implemented)

TradingView shows R:R in the order ticket. Hand of Midas shows it directly on the entry line on the chart, visible at all times without opening a panel.

### 8. Keyboard-First Bracket Placement (Already Implemented)

TradingView requires clicking the chart to start bracket placement. Hand of Midas supports the `B` key to activate bracket mode and `Tab` to toggle side, enabling keyboard-driven workflow without leaving the chart.

---

## Rendering Implementation Notes

### Current Architecture (midas-chart + midas-render)

The rendering pipeline for order annotations is:

1. `midas-chart` computes `WidgetOutput` via `compute_bracket()`:
   - `lines: Vec<GridLineInstance>` -- horizontal lines with rect and color
   - `labels: Vec<WidgetLabel>` -- text labels with position, color, font size
   - `hit_zones: Vec<HitZone>` -- interactive regions for mouse events
   - `fills: Vec<GridLineInstance>` -- zone fill rectangles

2. `midas-render` consumes these via `ChartScene` and renders:
   - Lines and fills via the grid line pipeline (instanced rects)
   - Labels via the text pipeline (glyph atlas)

### Changes Needed for New Order Types

**Limit/Stop line styles**: The current `GridLineInstance` renders solid rectangles only. For dashed and dotted lines, two approaches:

1. **CPU-side segmentation** (recommended for Phase 1): decompose a dashed line into multiple short `GridLineInstance` rects spaced by gaps. Simple, no shader changes. Cost: ~50-100 instances per dashed line.

2. **Shader-based dash pattern** (optimization for later): add a `dash_pattern: u32` field to `GridLineInstance` where bits encode the dash/gap pattern. The fragment shader discards fragments in gap regions. More efficient but requires shader modification.

Note: the `LineStyle` enum already exists in `midas-chart` (`Solid`, `Dashed`, `Dotted`) -- it just is not used in the rendering pipeline yet. The `leg_style()` method returns the style, but `compute_bracket()` ignores it and always renders solid lines. This is the gap to close.

**Trailing stop animation**: As discussed above, CPU-computed segments with a time-varying offset are the simplest approach. The chart's `TickMomentum` event can be reused to drive the animation at 10fps.

**Zone fills for non-Active brackets**: Currently zones are only drawn for `Active` and `PartialFill` brackets. For saved drafts, consider drawing zones at a very low alpha (0.03) to preview the risk/reward zones during planning.

### Label Format Specification

| Status | Entry Label | TP Label | SL Label |
|---|---|---|---|
| Draft | `"[TYPE] BUY @ {price:.2}"` | `"TP {price:.2}"` | `"SL @ {price:.2}"` |
| Pending | `"[TYPE] BUY @ {price:.2}  [hourglass]"` | `"TP {price:.2}"` | `"SL {price:.2}"` |
| PartialFill | `"[TYPE] BUY @ {price:.2}  [half] {filled}/{total}sh"` | `"TP {price:.2}  +${pnl}"` | `"SL {price:.2}  -${pnl}"` |
| Active | `"[arrow] {price:.2}  {qty}sh"` | `"TP {price:.2}  +${pnl}"` | `"SL {price:.2}  -${pnl}"` |
| Closed | `"[arrow] {price:.2}  {qty}sh"` | `"TP HIT +${pnl}"` or hidden | `"SL HIT -${pnl}"` or hidden |
| Cancelled | `"CANCELLED {price:.2}"` | hidden | hidden |

Where `[TYPE]` is: empty for Market, `"LMT"` for Limit, `"STP"` for Stop, `"STP LMT"` for Stop-Limit.

---

## Summary: Chart Annotation Type Hierarchy

```
AnnotationKind
  |-- Level (horizontal price level -- existing)
  |-- OrderBracket (unified order annotation -- existing, to be extended)
       |-- entry: BracketLeg (always present)
       |   |-- entry_type: OrderKind (NEW: Market/Limit/Stop/StopLimit)
       |-- take_profit: Option<BracketLeg>
       |-- stop_loss: Option<BracketLeg>
       |   |-- sl_type: StopLossKind (NEW: Fixed/TrailingAmount/TrailingPercent)
       |-- side: BracketSide
       |-- status: BracketStatus
       |-- quantity: Option<f64>
       |-- saved: bool
       |-- filled_qty: Option<f64>
```

Adding `entry_type` and `sl_type` fields to the existing `OrderBracket` struct is sufficient to support all new order types without creating new annotation types. This keeps the rendering and interaction code unified.
