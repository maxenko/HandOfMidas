# Cross-Chart Synchronization of Visual Elements in Trading Platforms

> Research report covering Bloomberg Terminal, ThinkOrSwim, NinjaTrader, MetaTrader 4/5,
> Sierra Chart, and TradingView. Focused on how annotations, drawings, indicators, and
> order brackets synchronize across linked chart panels.
>
> Date: 2026-03-30

---

## Table of Contents

1. [Executive Summary](#1-executive-summary)
2. [Bloomberg Terminal](#2-bloomberg-terminal)
3. [ThinkOrSwim (TOS)](#3-thinkorswim-tos)
4. [NinjaTrader 8](#4-ninjatrader-8)
5. [MetaTrader 4/5](#5-metatrader-45)
6. [Sierra Chart](#6-sierra-chart)
7. [TradingView (Bonus)](#7-tradingview-bonus)
8. [Cross-Platform Pattern Analysis](#8-cross-platform-pattern-analysis)
9. [The Same-Annotation-Different-Timeframes Problem](#9-the-same-annotation-different-timeframes-problem)
10. [Order Bracket Display Across Timeframes](#10-order-bracket-display-across-timeframes)
11. [Performance Considerations for 20+ Charts](#11-performance-considerations-for-20-charts)
12. [Recommendations for Hand of Midas](#12-recommendations-for-hand-of-midas)

---

## 1. Executive Summary

Every major platform handles cross-chart sync, but they disagree on the fundamental
data model. The key architectural decision is: **are annotations per-symbol, per-chart,
or per-workspace?** The answer determines everything downstream.

| Platform | Annotation Scope | Linking Mechanism | Sync Model |
|---|---|---|---|
| Bloomberg | Per-security group | Color-coded security groups | Push (group manager) |
| ThinkOrSwim | Per-symbol (drawing sets) | Color-coded link groups + auto-sync | Push (server-mediated) |
| NinjaTrader 8 | Per-chart or per-instrument (global) | `IsGlobal` flag per drawing | Copy-on-create (clones) |
| MetaTrader 4/5 | Per-chart (native) | Event-driven EA/indicator sync | Poll or event (OnChartEvent) |
| Sierra Chart | Per-chart + copy references | Chart number references | Pull (chart update interval) |
| TradingView | Per-symbol (separate storage) | Layout sync + global sync modes | Push (server-side) |

**Dominant pattern**: Annotations are stored **per-symbol**, not per-chart. A horizontal
level at $185.50 on AAPL is meaningful regardless of which chart panel or timeframe
is displaying it. This matches the `LevelStore` design already chosen for Hand of Midas.

---

## 2. Bloomberg Terminal

### Linking Model

Bloomberg uses **Launchpad Groups** with a Group Manager. Two group types exist:

- **Security Group**: Links components by a single security. Changing the security
  in one component changes it in all grouped components. This is the primary
  mechanism for chart linking.
- **Monitor Group**: Links components to a watchlist. Selecting a row in a monitor
  pushes the security to linked charts.

Groups are identified by **color** in the UI. Each component panel has a colored dot
indicating its group membership. Multiple independent groups can coexist.

### Annotation Behavior

- Charts in the same security group share the security context but annotations
  are **per-chart-instance** in the classic GP (Graph Price) function.
- Bloomberg's newer charting (G function) supports **co-editing**: annotations
  can be shared and co-edited with communities in real-time via Instant Bloomberg
  (IB) messaging integration.
- Annotations export with the chart when shared or embedded in reports/presentations.

### Crosshair Linking

- Bloomberg does not have a traditional multi-chart crosshair sync in the way
  retail platforms do. The terminal's panel layout is more document-oriented
  than chart-grid-oriented.

### Architectural Takeaway

Bloomberg's model is **security-centric linking**: the group says "these panels
all show the same security," and each panel independently renders its view. The
linking is about *what* to show, not about syncing *drawings* across panels.
Annotation sharing is handled at a higher level (collaboration/sharing) rather
than automatic per-symbol propagation.

---

## 3. ThinkOrSwim (TOS)

### Linking Model

TOS uses **two independent linking systems**:

1. **Color-coded link groups** (symbol linking): A vertical color strip next to
   the symbol field. Charts/watchlists/trade panels with the same color share
   symbol selection. Clicking a symbol in a linked watchlist changes the symbol
   on all linked charts. This is purely about **symbol routing**, not drawing sync.

2. **Drawing synchronization** (automatic, per-symbol): Enabled in Settings >
   General > Synchronize > Drawings. When enabled, all chart instances showing
   the same symbol automatically share all drawings, regardless of timeframe.

### Drawing Sets Architecture

TOS stores drawings in **drawing sets** keyed by symbol:

- Each symbol has one or more named drawing sets (default: "Default").
- Drawing sets are **cloud-stored** and synced across all devices.
- Users can create multiple drawing sets per symbol (e.g., "long-term trend",
  "support levels", "intraday look") for organizing drawings by purpose.
- Switching drawing sets is done via a dropdown in the chart footer.
- Drawing sets are **timeframe-agnostic** -- the same set applies whether you
  view the symbol on a daily, hourly, or 5-minute chart.

### Cross-Timeframe Behavior

- When drawing sync is enabled, a trendline drawn on a daily AAPL chart
  **immediately appears** on all other open AAPL charts, including 5-minute charts.
- The drawing anchors are stored as **(price, timestamp)** pairs. On different
  timeframes, the platform snaps the timestamp to the nearest available bar.
- Horizontal lines (price-only anchors) translate perfectly across timeframes
  since they have no time dependency.
- Trendlines and other time-anchored drawings may appear slightly shifted on
  different timeframes due to bar boundary alignment, but maintain their
  mathematical definition (same slope between the same two price/time points).

### Crosshair Synchronization

- Separately configurable from drawing sync.
- When enabled, crosshair position transmits to **all open chart instances**.
- Each chart displays the crosshair at the corresponding time position.

### Data Model Summary

```
Symbol ("AAPL")
  └── Drawing Set ("Default")
       ├── Drawing 1 (trendline, anchors: [(t1,p1), (t2,p2)])
       ├── Drawing 2 (horizontal line, price: 185.50)
       └── Drawing 3 (fibonacci, anchors: [(t1,p1), (t2,p2)])
```

**Key insight**: TOS chose per-symbol storage with timeframe-agnostic drawings.
This is the simplest model and works because most drawings (levels, trendlines,
fib retracements) are defined by price/time coordinates that are meaningful
at any timeframe.

---

## 4. NinjaTrader 8

### Linking Model

NinjaTrader uses a **per-drawing visibility flag** rather than a global sync setting:

- Each drawing object has an **"Attach to"** property:
  - **"This chart"** (default): Drawing exists only on the chart where it was created.
  - **"All charts"**: Drawing appears on all charts showing the same instrument.
- The "All charts" option creates **Global Drawing Objects**.

### Global Drawing Objects Architecture

Key architectural details:

- **Scope**: Per-instrument. A horizontal line on ES appears on all ES charts
  (1-min, 5-min, daily, etc.) but never on NQ charts.
- **Workspace scope**: By default, global objects are scoped to the workspace
  they were created in. A global setting (`Tools > Options > General > Global
  drawing objects across workspaces`) extends scope to all workspaces.
- **Storage**: Global drawing objects are serialized as XML files in
  `Documents\NinjaTrader 8\templates\GlobalDrawingObject\`. Each file is named
  by instrument and workspace.
- **Implementation**: Global Drawing Objects are **copies** of the original object,
  not references to a shared instance. When the original is modified, the copies
  are updated. This is a clone-and-sync model.
- **Programmatic API**: `Draw.HorizontalLine()` and similar methods accept an
  `isGlobal` parameter. The `IsGlobal` property can be set on any drawing object.

### Cross-Timeframe Behavior

- Horizontal lines translate perfectly (price-only anchor).
- Time-anchored drawings (trendlines, rectangles) use timestamp anchors.
  NinjaTrader maps these timestamps to the nearest available bar on each
  target chart's timeframe.
- An important limitation: the "Attach to" property is **not saved in drawing
  templates**. Users must manually set it each time.

### Indicator vs Drawing Distinction

- **Drawings** can be global (cross-chart). **Indicators cannot.**
- This is a deliberate architectural separation. Indicators are bound to a
  specific data series and chart panel. Drawings are coordinate-defined
  overlays that can be projected onto any compatible chart.

### Order Display

- NinjaTrader's Chart Trader shows order brackets (entry, TP, SL) visually
  on the chart with colored lines and labels on the price axis.
- **Only one Chart Trader panel per chart window.** Order visualization does
  not automatically propagate to other charts showing the same instrument.
- Orders are managed through the Account/Order management system, and their
  visual representation is per-chart-window.

### Data Model Summary

```
Per-chart storage:
  Workspace XML → Chart Tab → <DrawingTools> section → individual drawings

Global storage:
  GlobalDrawingObject/{instrument}_{workspace}.xml → serialized drawing clones

Each drawing:
  - IsGlobal: bool
  - Anchors: Vec<(DateTime, Price)>
  - Instrument: String
  - DrawingType: enum
  - Visual properties (color, width, style)
```

---

## 5. MetaTrader 4/5

### Native Architecture

MetaTrader has **no built-in cross-chart drawing synchronization**. Each chart
maintains its own independent set of graphical objects. Cross-chart sync is
achieved through **third-party indicators/EAs** using the MQL programming API.

### MQL Event System for Sync

MT5 provides the `OnChartEvent()` callback with object-related events:

- `CHARTEVENT_OBJECT_CREATE` -- fired when a graphical object is created
- `CHARTEVENT_OBJECT_CHANGE` -- fired when object properties change via dialog
- `CHARTEVENT_OBJECT_DELETE` -- fired when an object is deleted
- `CHARTEVENT_OBJECT_DRAG` -- fired when an object is dragged
- `CHARTEVENT_OBJECT_CLICK` -- fired on click, with coordinates

These events must be explicitly enabled per chart via `CHART_EVENT_OBJECT_CREATE`
and `CHART_EVENT_OBJECT_DELETE` chart properties.

### Two Sync Approaches (from MQL5 article by Dmitriy Gizlyk)

**Approach 1: Polling**
- Timer-based periodic scan of all objects on all charts.
- Compare current state against last-known state.
- Clone changed objects to target charts.
- Pro: Simple to implement.
- Con: Synchronization delay proportional to poll interval. CPU cost grows
  with object count. Difficult to determine "last state" when multiple
  charts can edit the same object.

**Approach 2: Event-driven (recommended)**
- Use `OnChartEvent()` to react immediately to object creation/modification.
- On object change: enumerate all charts with the same symbol, clone or
  update the object on each target chart.
- Pro: Minimal delay (reacts on next event loop iteration).
- Con: More complex implementation. Must handle recursion (syncing to chart B
  triggers an event on chart B, which must not re-sync back to chart A).

### Cross-Chart Communication Mechanisms

MT5 provides several IPC primitives:

- **`EventChartCustom()`**: Send a custom event to a specific chart's event queue.
  Used to notify target charts that a sync operation occurred.
- **`GlobalVariableSet/Get()`**: Named global variables shared across all EAs/indicators
  in the terminal. Used as a coordination mechanism (e.g., a "sync_in_progress" flag
  to prevent recursive sync loops).
- **`ChartSetSymbolPeriod()`**: Change a chart's symbol/period programmatically.

### Cross-Timeframe Behavior

- Objects are defined by (time, price) anchor points.
- When cloning an object to a chart with a different timeframe, the time coordinates
  are preserved. The target chart maps them to the nearest available bar.
- Horizontal lines are trivial (price-only).
- Trendlines maintain their slope but may appear to connect different visual bar
  positions due to bar boundary differences.

### Data Model

```
Per-chart object storage (native):
  Chart ID → Object Name → Object Properties
    - type: OBJ_HLINE, OBJ_TREND, OBJ_RECTANGLE, etc.
    - time1, price1 (first anchor)
    - time2, price2 (second anchor, if applicable)
    - color, width, style
    - selectable, selected, hidden

Cross-chart sync (via EA/indicator):
  Source chart detects change → enumerates target charts by symbol →
  creates/modifies objects on targets using ObjectCreate/ObjectSet
```

### Key Architectural Insight

MetaTrader's per-chart object model with event-based sync is the most **transparent**
architecture. The sync logic is entirely in user-space (MQL code), making the
synchronization strategy fully customizable. The recursion-prevention problem
(avoiding infinite sync loops) is the main engineering challenge.

---

## 6. Sierra Chart

### Chartbook Architecture

Sierra Chart organizes everything around **Chartbooks** (workspaces):

- A Chartbook is a `.Cht` file containing all charts, their settings, drawings,
  and studies.
- Each chart within a Chartbook has a unique **Chart Number** (e.g., #1, #2, #3).
- Charts are linked within a Chartbook using a **Link Number** (nonzero integer).
  Charts with the same Link Number form a link group.

### Chart Linking

Linkable properties (when charts share a Link Number):

- **Symbol**: Changing the symbol on one linked chart changes all linked charts.
- **Bar Period**: Changing the timeframe propagates to linked charts.
- **Scroll Position**: Scrolling one chart scrolls linked charts to the same time.

Link Numbers are **scoped to a Chartbook**. They do not cross Chartbook boundaries.

### Drawing Synchronization: Copy Chart Drawings

Sierra Chart uses an explicit **source-destination reference model**:

1. In the destination chart's settings (`Chart > Chart Settings > Chart Drawings`),
   enter the source **Chart Number(s)** in the "Copy Chart Drawings from Chart #'s"
   field (comma-separated for multiple sources).

2. Behavior:
   - All drawings from the source chart appear on the destination chart.
   - When a drawing is modified on the source chart, the change **automatically
     propagates** to the destination chart at the **Chart Update Interval**.
   - Drawings can **also be modified on the destination chart** -- edits on
     the destination are local overrides (they do not propagate back to source).
   - This is a **one-directional pull** model: destination pulls from source.

3. Limitations:
   - Volume Profile drawings cannot be copied.
   - Drawings have Bar Period visibility settings -- a drawing can be set to
     only appear on certain timeframes even when copied.
   - Cross-Chartbook drawing copy is not supported natively.

### Drawing Anchor Model (ACSIL s_UseTool)

Sierra Chart's programmatic drawing API reveals the internal data model:

```c
struct s_UseTool {
    int   ChartNumber;          // Which chart owns this drawing
    int   DrawingType;          // DRAWING_HORIZONTAL_LINE, DRAWING_LINE, etc.
    int   LineNumber;           // Unique ID within the chart

    // Anchor points (two approaches):
    // Option A: DateTime-based
    SCDateTime BeginDateTime;   // First anchor time
    SCDateTime EndDateTime;     // Second anchor time
    float      BeginValue;      // First anchor price
    float      EndValue;        // Second anchor price

    // Option B: Index-based (more efficient, preferred)
    int   BeginIndex;           // Bar index for first anchor
    int   EndIndex;             // Bar index for second anchor

    // Cross-chart properties
    int   AllowCopyToOtherCharts;  // 0 or 1
    int   IsGlobalDrawingTool;     // Scope: global vs local

    // Visual properties
    COLORREF Color;
    int      LineWidth;
    int      LineStyle;
    // ... additional properties
};
```

**Key detail**: Drawings can be anchored by either DateTime or bar Index. The DateTime
approach enables cross-timeframe mapping (the target chart finds the nearest bar to the
given timestamp). The Index approach is more efficient but is chart-specific (bar indices
differ between timeframes).

### Global Cursor

Sierra Chart provides `Tools > Global Cursor On` which displays the crosshair on
multiple charts within the same Chartbook, showing corresponding bars and price levels.

### Data Model Summary

```
Chartbook (.Cht file)
  ├── Chart #1 (AAPL, Daily)
  │    ├── Link Number: 1
  │    ├── Drawings: [line_1, hline_2, ...]
  │    └── Copy From: (none -- this is a source)
  ├── Chart #2 (AAPL, 5-min)
  │    ├── Link Number: 1
  │    ├── Drawings: [local_drawing_3, ...]
  │    └── Copy From: Chart #1  ← pulls drawings from Chart #1
  └── Chart #3 (MSFT, Daily)
       ├── Link Number: 2
       ├── Drawings: [line_4, ...]
       └── Copy From: (none)
```

### Architectural Takeaway

Sierra Chart's model is the most **explicit and manual**. There is no "auto-sync
all same-symbol drawings." Instead, the user explicitly configures which chart is
a source and which is a destination. This gives maximum control but requires more
setup. The one-directional pull model avoids the recursion problems that plague
bidirectional sync.

---

## 7. TradingView (Bonus)

Included because TradingView has the most well-documented charting library API,
which reveals their data model clearly.

### Three Sync Modes

1. **No sync**: Drawings saved per-chart, per-layout. Changing symbol hides drawings.
2. **Layout sync**: Drawings on a symbol sync across all charts in the current layout.
3. **Global sync**: Drawings sync across all charts and all layouts.

### Selective Chart Grouping

TradingView allows **emoji-based chart grouping** within a layout. Charts marked
with the same emoji sync selected parameters (symbol, crosshair, interval, date range).
This is their version of color-coded link groups.

### Drawings Data Model (from Charting Library API)

TradingView's Charting Library reveals the internal storage split:

**Combined storage** (default): Drawings are embedded in the chart layout JSON.
A layout includes all drawings for all symbols/charts in that layout.

**Separate drawings storage** (`saveload_separate_drawings_storage` featureset):
Drawings are stored independently from chart layouts, keyed by symbol.

API endpoints for separate storage:
```
GET  /drawings?client={id}&user={id}&chart={id}    -- load drawings
PUT  /drawings?client={id}&user={id}&chart={id}     -- save drawings
DELETE /drawings?client={id}&user={id}&chart={id}    -- delete
```

The `saveLineToolsAndGroups` method receives state organized as:
```json
{
  "sources": {
    "AAPL": [
      { "id": "abc123", "type": "trendline", "points": [...], "style": {...} },
      { "id": "def456", "type": "horizontal_line", "price": 185.5, "style": {...} }
    ],
    "MSFT": [...]
  }
}
```

**Null values indicate deleted drawings** -- the API uses tombstones.

### Cross-Timeframe Behavior

TradingView anchors drawings at **(timestamp, price)** coordinate pairs:

- Horizontal lines: price-only, perfect across timeframes.
- Trendlines: (t1, p1) to (t2, p2). When the chart resolution changes, the
  library snaps timestamps to the nearest available bar.
- **Important**: A trendline connecting two daily bar highs may not visually
  connect the same candle features on a 1-hour chart because the bar timestamps
  are different (daily bar opens at market open; the exact high may have occurred
  at a different time within the day).
- TradingView's documentation explicitly acknowledges this: "Drawings may be
  displayed differently on various time intervals of the same symbol."

### Server-Side Architecture

- Drawings persist in PostgreSQL (backend: `tradingview/saveload_backend`).
- Cloud-synced across devices.
- Rendering is hybrid: server-side preloading + client-side interactivity.

---

## 8. Cross-Platform Pattern Analysis

### Pattern 1: Linked Chart Groups (Color-Coded)

**Who uses it**: Bloomberg (security groups), TOS (color strips), TradingView (emoji groups).

**How it works**: Each chart panel has a group identifier (color/emoji/number). Panels
in the same group share symbol selection. When you click a symbol in a watchlist linked
to the "blue" group, all "blue" panels switch to that symbol.

**What it syncs**: Symbol selection. Optionally: crosshair position, scroll position,
timeframe. Generally does NOT auto-sync drawings (that is a separate system).

**Implementation pattern**:
```
GroupManager {
    groups: HashMap<GroupId, Vec<PanelId>>

    fn set_symbol(group: GroupId, symbol: &str) {
        for panel in groups[group] {
            panel.set_symbol(symbol);
        }
    }
}
```

### Pattern 2: Per-Symbol Drawing Storage

**Who uses it**: TOS (drawing sets), TradingView (separate storage mode), NinjaTrader (global objects).

**How it works**: Drawings are keyed by symbol, not by chart. Any chart displaying
that symbol renders all drawings from the symbol's drawing collection.

**Advantages**:
- Natural deduplication -- no duplicate drawings on two AAPL charts.
- Adding a new chart for a symbol immediately shows all existing drawings.
- Clean persistence -- one file/record per symbol.

**Disadvantages**:
- Timeframe-specific drawings (e.g., a pattern visible only on 5-min) always
  appear on all timeframes, which can create visual clutter.
- Mitigation: allow per-drawing visibility filters (TOS drawing sets, NinjaTrader
  "Attach to" property).

### Pattern 3: Source-Destination Copy References

**Who uses it**: Sierra Chart (Chart Number references), NinjaTrader (clone model).

**How it works**: Destination chart references a source chart by ID. Drawings from
the source are replicated to the destination. Changes to the source propagate;
changes on the destination are local overrides.

**Advantages**:
- Explicit control over what appears where.
- No surprise drawings appearing on unrelated charts.
- One-directional flow avoids sync loops.

**Disadvantages**:
- Requires manual configuration per chart.
- Adding a new chart requires updating copy references.
- Source deletion can orphan copied drawings.

### Pattern 4: Event-Driven Sync (Push)

**Who uses it**: MetaTrader (OnChartEvent), TOS (automatic sync).

**How it works**: When a drawing is created/modified/deleted, an event fires.
The sync system pushes the change to all target charts immediately.

**Implementation pattern** (event-driven):
```
on_drawing_changed(source_chart, drawing) {
    for chart in all_charts_with_same_symbol(drawing.symbol) {
        if chart != source_chart {
            chart.upsert_drawing(drawing.clone());
        }
    }
}
```

**Recursion prevention** (critical): When chart B receives a synced drawing, it
must not fire another sync event back to chart A. Solutions:
- **Guard flag**: Set `syncing = true` before pushing, skip events while flag is set.
- **Source tracking**: Each drawing carries a `source_chart_id`; skip sync if the
  incoming drawing's source matches the current chart.
- **Generation counter**: Each drawing has a generation number; skip sync if the
  incoming generation matches what we already have.

### Pattern 5: Pull at Update Interval

**Who uses it**: Sierra Chart (chart update interval).

**How it works**: Destination charts periodically poll source charts for drawing
changes. Changes propagate on the next update cycle.

**Advantages**:
- Simple implementation. No event system needed.
- Naturally batches updates, reducing overhead.

**Disadvantages**:
- Visible delay between drawing a line and seeing it on another chart.
- Update interval is a tuning parameter (too fast = CPU waste, too slow = stale).

---

## 9. The Same-Annotation-Different-Timeframes Problem

This is the hardest problem in cross-chart sync. Every platform handles it differently.

### Category A: Price-Only Annotations (Easy)

**Horizontal lines, price alerts, order levels.**

These have no time component. A horizontal line at $185.50 is $185.50 on every
timeframe. These translate perfectly and every platform handles them identically.

**Conclusion**: Price-only annotations should always be per-symbol, shared across
all charts and timeframes. This is exactly what Hand of Midas's `LevelStore` does.

### Category B: Price+Time Annotations (Hard)

**Trendlines, rectangles, Fibonacci retracements, text notes.**

These are anchored at (timestamp, price) points. The problems:

1. **Bar boundary mismatch**: A trendline connecting the high of the 10:00 5-min bar
   to the high of the 14:30 5-min bar. On a daily chart, both of these are within the
   same bar. The daily chart has no concept of "10:00 high" vs "14:30 high." The
   trendline collapses to a single point or a very short line.

2. **Timestamp snapping**: Different timeframes have different bar boundaries. A
   timestamp of 10:17 snaps to the 10:15 bar on a 5-min chart but to the 10:00 bar
   on a 30-min chart. The visual position shifts slightly.

3. **Extended periods**: A multi-month trendline drawn on a daily chart is visually
   meaningful. On a 1-minute chart, it extends far beyond the visible viewport and
   just appears as a nearly flat line.

### Platform Approaches

| Platform | Approach | Result |
|---|---|---|
| TOS | Show on all timeframes, anchor at nearest bar | Good for horizontals, acceptable for trendlines |
| NinjaTrader | Show on all charts of same instrument | Same as TOS |
| MetaTrader | User-controlled (EA logic decides) | Maximum flexibility |
| Sierra Chart | User explicitly chooses which charts copy | Manual control avoids bad matches |
| TradingView | Show on all timeframes, warn about differences | Explicit documentation of the limitation |

### Recommended Approach for Hand of Midas

**Two-tier model**:

1. **Price-only annotations** (horizontal levels, order brackets): Per-symbol,
   automatic sync to all charts of the same symbol, all timeframes. Already
   implemented via `LevelStore`.

2. **Price+time annotations** (trendlines, rectangles, notes): Per-symbol with
   optional timeframe visibility filter. Store with (timestamp, price) anchors.
   Display on all charts by default, but allow users to set `visible_timeframes:
   Option<Vec<Timeframe>>` to restrict display to specific timeframes.

---

## 10. Order Bracket Display Across Timeframes

### How Platforms Handle It

**NinjaTrader**: Order brackets (entry + TP + SL) display as horizontal lines with
price labels on the active chart's price axis. Only one Chart Trader panel per chart
window. Orders do NOT automatically display on other charts of the same instrument.
This is a notable limitation that users frequently request.

**TradingView**: OCO bracket orders display as connected horizontal lines (entry,
stop, target) with colored zones. When bracket orders are enabled, they appear on
the chart where the order was placed. The charting library supports displaying orders
on any chart via the Broker API integration.

**Sierra Chart**: Order lines display on the chart attached to Chart Trader. They
do not automatically propagate to other charts, though the Chart Trader can be
attached to linked charts.

**MultiCharts**: Bracket Strategy displays as green and maroon markers connected
to the order price label with dotted lines.

### Architectural Pattern for Order Brackets

Order brackets are special because they represent **live server-side state**, not
user-drawn annotations:

1. **Orders exist independently of charts.** An order at $185.50 exists in the
   broker's system regardless of which charts are open.

2. **Order visualization should be per-symbol, automatic.** Every chart showing
   AAPL should display active AAPL orders. This is different from user drawings,
   where the user might want control over visibility.

3. **Order brackets are price-only.** Entry, TP, and SL are horizontal price
   levels. They have no time anchor (the order exists NOW, at a price, indefinitely
   until filled or canceled). This makes cross-timeframe display trivial.

4. **Visual distinction from user drawings.** Order levels should be visually
   distinct (different colors, dashed lines, labels showing quantity and order type)
   from user-drawn horizontal levels.

### Recommended Approach for Hand of Midas

Since orders are price-only and broker-owned:
- Store active order levels in a structure analogous to `LevelStore` but managed
  by the order bridge (midas-app), not the user.
- All charts showing the order's symbol render the order levels automatically.
- Order levels are read-only on the chart (modifications go through the order
  management system, not chart drawing tools).
- Visual style: dashed lines, distinct colors (green for TP, red for SL, blue/white
  for entry), with quantity/type labels.

---

## 11. Performance Considerations for 20+ Charts

### Real-Time Sync with Many Open Charts

When syncing across 20+ charts, the main performance concerns are:

**1. Event Storm on Drawing Change**

If a user drags a horizontal level, and 20 charts need to update:
- Naive: Fire 20 redraw events → 20 separate GPU prepare passes.
- Better: **Batch invalidation.** Mark all affected charts as dirty in a single
  pass, then redraw all dirty charts in the next frame. This is already how
  Hand of Midas works with its generation-counter dirty tracking.

**2. Lookup Cost: "Which Charts Show This Symbol?"**

Every drawing change needs to find all charts displaying the affected symbol:
- Naive: O(n) scan of all charts for each change.
- Better: Maintain an **index** `HashMap<Symbol, Vec<ChartId>>`. Update on
  chart open/close/symbol change. Lookup is O(1).

**3. Drawing Cloning vs Reference Sharing**

NinjaTrader's clone model creates N copies of each global drawing for N charts.
This wastes memory and makes updates O(n).

Better approach (used by TOS and Hand of Midas's LevelStore):
- **Single source of truth** per symbol. All charts reference the same data.
- Charts read `LevelStore::levels_for("AAPL")` and get a shared `&[HorizontalLevel]`.
- No cloning. No sync delay. Changes visible on the same frame.

**4. GPU Buffer Updates**

When a shared level changes, all charts showing that symbol need new GPU instances:
- Sierra Chart throttles this with the "Chart Update Interval."
- Hand of Midas uses generation counters: each chart caches the last-seen
  generation. On each frame, compare `store.generation("AAPL")` with the
  cached value. If different, rebuild level instances. Cost: one integer
  comparison per chart per frame (negligible).

**5. Crosshair Sync Overhead**

Crosshair sync is the highest-frequency sync operation (fires on every mouse move):
- Current design: single `crosshair_sync: Option<(ChartId, i64, String)>` on
  MidasApp. One write per mouse move, one read per chart per frame.
- For 20 charts: 1 write + 20 reads = 21 operations per mouse move.
  Each read is an integer comparison + one `time_to_x()` transform.
  Total cost: sub-microsecond. No concern.

### Platform-Specific Performance Strategies

| Platform | Strategy | Notes |
|---|---|---|
| Sierra Chart | Multi-threaded chart updates | Data downloads, chart loading, market data on separate threads |
| TradingView | Server-side pre-rendering + client-side interactivity | Hybrid approach reduces client load |
| MetaTrader | Single-thread per indicator, event queue | Can bottleneck on complex sync indicators |
| NinjaTrader | Clone model with workspace XML | Higher memory cost but isolated updates |

### Summary for 20+ Charts

The Hand of Midas architecture (centralized per-symbol store + generation counters +
shared references) is already optimal for 20+ chart sync. The key invariants:

1. **One source of truth per symbol** (LevelStore, future AnnotationStore).
2. **Generation counter per symbol** for O(1) dirty checking.
3. **No cloning** -- all charts read the same data.
4. **Batch frame updates** -- all dirty charts redraw in the same frame.
5. **Symbol-to-chart index** for O(1) invalidation targeting.

---

## 12. Recommendations for Hand of Midas

Based on this research, here are concrete recommendations mapped to the existing
codebase and architecture plans:

### Already Correct (Validate)

1. **LevelStore is per-symbol** -- matches TOS and TradingView's best-practice
   model. Keep this.
2. **Generation counter dirty tracking** -- matches the pattern used by
   high-performance platforms. Keep this.
3. **Crosshair sync via shared state** -- simple and efficient for the
   single-threaded iced update loop.
4. **Annotations planned as per-symbol per-timeframe (06-persistence.md)** --
   this is a reasonable starting point for price+time annotations.

### Consider Changing

1. **Annotation persistence split**: The current plan stores annotations as
   `AAPL_D1.json` (per-symbol-per-timeframe). Consider instead storing
   annotations per-symbol with an optional `visible_timeframes` field on each
   annotation. This matches the TOS/TradingView model where a drawing is
   defined once and displayed across timeframes:
   ```json
   {
     "symbol": "AAPL",
     "annotations": [
       {
         "kind": "HorizontalLine",
         "price": 185.50,
         "visible_timeframes": null  // null = all timeframes
       },
       {
         "kind": "TrendLine",
         "anchors": [{"t": 1711584000, "p": 180.0}, {"t": 1711670400, "p": 190.0}],
         "visible_timeframes": ["D1", "W1"]  // only show on daily and weekly
       }
     ]
   }
   ```

2. **AnnotationStore should follow LevelStore pattern**: Centralized, per-symbol,
   owned by MidasApp, passed by reference. Not per-chart. The annotation architecture
   plan (01-architecture.md) has `ChartState.annotations: AnnotationStore` which
   is per-chart. Consider lifting this to MidasApp, mirroring LevelStore.

3. **Add link groups for symbol routing**: Implement color-coded (or number-coded)
   chart link groups for symbol switching. This is separate from drawing sync and
   is a workspace/navigation feature. Simple data model:
   ```rust
   struct LinkGroupManager {
       groups: HashMap<LinkGroupId, Vec<ChartId>>,
   }

   fn set_group_symbol(&mut self, group: LinkGroupId, symbol: &str) {
       for chart_id in &self.groups[group] {
           self.charts[chart_id].set_symbol(symbol);
       }
   }
   ```

### Future Features (Not Needed Now)

1. **Bidirectional annotation sync with recursion prevention**: Use source tracking
   (MetaTrader pattern). Each annotation carries `last_modified_by: ChartId`.
   Skip sync if the incoming change originated from the current chart.

2. **Per-drawing timeframe visibility filter**: Allow users to hide a drawing on
   certain timeframes without deleting it.

3. **Drawing templates**: Save and apply drawing property sets (color, width, style)
   across future drawings. Both NinjaTrader and TradingView support this.

4. **Order bracket visualization bridge**: When the broker integration ships,
   active orders should automatically display as horizontal levels on all charts
   of the same symbol, distinct from user-drawn levels.

---

## Sources

### Bloomberg Terminal
- [Bloomberg Charts Product Page](https://www.bloomberg.com/professional/products/bloomberg-terminal/charts/)
- [Bloomberg Launchpad Getting Started (Lerner)](https://my.lerner.udel.edu/wp-content/uploads/BB-Getting-Started-in-Launchpad.pdf)
- [Bloomberg Launchpad Getting Started (IIMA)](https://library.iima.ac.in/public/download/bloomberg/launchpad.pdf)
- [Bloomberg Terminal Essentials](https://www.bloomberg.com/professional/insights/technology/bloomberg-terminal-essentials-ib-worksheets-launchpad/)
- [Bloomberg Launchpad Part III (Wharton)](https://lippincottlibrary.wordpress.com/2013/09/02/bloomberg-launchpad-part-iii/)

### ThinkOrSwim
- [TOS General Settings (Drawing Sync)](https://toslc.thinkorswim.com/center/howToTos/thinkManual/charts/Chart-Style-Settings/general)
- [TOS Synchronization (thinkpipes)](https://tlc.thinkpipes.com/center/charting/charts/Useful-Tools/Synchronization.html)
- [TOS Using Drawings](https://toslc.thinkorswim.com/center/howToTos/thinkManual/charts/Using-Drawings)
- [TOS Drawing Sync Discussion (useThinkScript)](https://usethinkscript.com/threads/how-to-disable-chart-drawing-synchronization.4985/)
- [TOS Link Setup Tutorial (Tackle Trading)](https://tackletrading.com/think-or-swim-tos-link-sync-setup-for-convenience/)
- [TOS Drawing Sets (Schwab/Twitter)](https://twitter.com/thinkorswim/status/865967707056988162)
- [Timeframe Drawing Discussion (useThinkScript)](https://usethinkscript.com/threads/recent-changes-on-tos-tools-trend-lines-levels-on-different-time-frames.16495/)

### NinjaTrader 8
- [NinjaTrader Working with Drawing Tools](https://ninjatrader.com/support/helpguides/nt8/working_with_drawing_tools__ob.htm)
- [Sync Drawings Forum Thread](https://forum.ninjatrader.com/forum/ninjatrader-8/platform-technical-support-aa/1135865-sync-drawings-across-charts)
- [Synchronize Drawing Tools Forum](https://forum.ninjatrader.com/forum/ninjatrader-8/platform-technical-support-aa/1288410-synchronize-drawing-tools-across-multiple-charts)
- [Global Drawing Objects Storage Forum](https://forum.ninjatrader.com/forum/ninjatrader-8/platform-technical-support-aa/1202919-where-are-global-drawing-objects-stored-in-the-nt8-documents-folder)
- [Drawing Objects on All Charts Forum](https://forum.ninjatrader.com/forum/ninjatrader-8/platform-technical-support-aa/1123658-drawing-objects-on-all-charts)
- [Global Draw Objects Forum](https://forum.ninjatrader.com/forum/ninjatrader-8/platform-technical-support-aa/98749-global-draw-objects)
- [NinjaTrader Order & Position Display](https://ninjatrader.com/support/helpGuides/nt8/order__position_display.htm)
- [NinjaTrader Chart Trader (Medium)](https://ninjatrader.medium.com/how-to-trade-from-the-charts-using-ninjatrader-chart-trader-af01732fa538)
- [NinjaTrader Web Drawing Sync](https://support.ninjatrader.com/s/article/How-Do-I-Synchronize-Drawing-Tools-in-NinjaTrader-Web-Charts)

### MetaTrader 4/5
- [MQL5 Article: Synchronizing Same-Symbol Charts (Gizlyk)](https://www.mql5.com/en/articles/4465)
- [MQL5 Article: Chart Synchronization for Technical Analysis](https://www.mql5.com/en/articles/18937)
- [MQL5 OnChartEvent Documentation](https://www.mql5.com/en/docs/event_handlers/onchartevent)
- [MQL5 EventChartCustom Documentation](https://www.mql5.com/en/docs/eventfunctions/eventchartcustom)
- [MQL5 Graphical Object Events](https://www.mql5.com/en/book/applications/events/events_objects)
- [MQL5 Chart Event Types](https://www.mql5.com/en/docs/constants/chartconstants/enum_chartevents)
- [Objects Synchronization MT5 (Market)](https://www.mql5.com/en/market/product/59763)
- [Multi-Chart-Sync MT4 (Forex Factory)](https://www.forexfactory.com/thread/683930-multi-chart-sync-mcs)
- [Synchronise MT5/MT4 Charts (Orchard Forex)](https://orchardforex.com/synchronise-multiple-mt5-4-charts-with-the-same-symbol/)

### Sierra Chart
- [Sierra Chart: Copying Chart Drawings Automatically](https://www.sierrachart.com/index.php?page=doc/CopyingChartDrawingsFromOtherChartsAutomatically.html)
- [Sierra Chart: Chart Drawing Tools](https://www.sierrachart.com/index.php?page=doc/Tools.html)
- [Sierra Chart: Chartbooks](https://www.sierrachart.com/index.php?page=doc/Chartbooks.html)
- [Sierra Chart: Working With Charts](https://www.sierrachart.com/index.php?page=doc/WorkingWithCharts.html)
- [Sierra Chart: ACSIL Drawing Tools (s_UseTool)](https://www.sierrachart.com/index.php?page=doc/ACSILDrawingTools.html)
- [Sierra Chart: Study/Price Overlay](https://www.sierrachart.com/index.php?page=doc/StudyPriceOverlayStudy.php)
- [Sierra Chart: Chart Linking Forum](https://www.sierrachart.com/SupportBoard.php?ThreadID=32281)
- [Sierra Chart: Global Drawing Sync Forum](https://www.sierrachart.com/SupportBoard.php?ThreadID=4899)

### TradingView
- [TradingView: Saving Drawings Separately](https://www.tradingview.com/charting-library-docs/latest/saving_loading/saving_drawings_separately/)
- [TradingView: Save/Load REST API](https://www.tradingview.com/charting-library-docs/latest/saving_loading/save-load-rest-api/)
- [TradingView: Drawing Methods API](https://www.tradingview.com/charting-library-docs/latest/saving_loading/save-load-rest-api/drawing-methods/)
- [TradingView: Drawings API](https://www.tradingview.com/charting-library-docs/latest/ui_elements/drawings/drawings-api/)
- [TradingView: Syncing Charts in Layouts](https://www.tradingview.com/support/solutions/43000629992-how-to-sync-the-charts-of-my-layout/)
- [TradingView: Drawings Sync Across Layouts (Blog)](https://www.tradingview.com/blog/en/drawings-synchronization-across-layouts-39890/)
- [TradingView: Layouts, Charts, Drawings Interaction](https://www.tradingview.com/support/solutions/43000692404-layouts-charts-drawings-indicators-and-their-interaction/)
- [TradingView: Drawings Different on Other Intervals](https://www.tradingview.com/support/solutions/43000477901-drawings-are-passing-through-different-points-on-another-interval/)
- [TradingView: Bracket Orders](https://www.tradingview.com/charting-library-docs/latest/trading_terminal/trading-concepts/brackets/)
- [TradingView: Selective Chart Synchronization (Blog)](https://www.tradingview.com/blog/en/synchronization-of-selected-charts-38335/)
- [TradingView saveload_backend (GitHub)](https://github.com/tradingview/saveload_backend)

### General Architecture
- [GoCharting: Multi-chart Layouts and Sync Settings](https://gocharting.com/docs/general-settings/Multi-chart-layouts-and-sync-settings)
- [Redis: Real-Time Trading Platform](https://redis.io/blog/real-time-trading-platform-with-redis-enterprise/)
- [MultiCharts: Bracket Orders](https://www.multicharts.com/trading-software/index.php?title=Bracket)
