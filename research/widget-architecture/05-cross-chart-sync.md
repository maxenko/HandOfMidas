# Cross-Chart Synchronization Patterns

> Compiled from cross-chart sync research covering Bloomberg, ThinkOrSwim, NinjaTrader,
> MetaTrader 4/5, Sierra Chart, and TradingView, 2026-03-30

---

## 1. The Central Question: Where Do Annotations Live?

The key architectural decision is: **are annotations per-symbol, per-chart, or per-workspace?** The answer determines everything downstream.

| Platform | Annotation Scope | Linking Mechanism | Sync Model |
|---|---|---|---|
| Bloomberg | Per-security group | Color-coded security groups | Push (group manager) |
| ThinkOrSwim | Per-symbol (drawing sets) | Color-coded link groups + auto-sync | Push (server-mediated) |
| NinjaTrader 8 | Per-chart or per-instrument (global) | `IsGlobal` flag per drawing | Copy-on-create (clones) |
| MetaTrader 4/5 | Per-chart (native) | Event-driven EA/indicator sync | Poll or event (OnChartEvent) |
| Sierra Chart | Per-chart + copy references | Chart number references | Pull (chart update interval) |
| TradingView | Per-symbol (separate storage) | Layout sync + global sync modes | Push (server-side) |

**Dominant pattern**: Annotations are stored **per-symbol**, not per-chart. A horizontal level at $185.50 on AAPL is meaningful regardless of which chart panel or timeframe is displaying it.

---

## 2. Per-Symbol Storage Validation

### ThinkOrSwim: Drawing Sets

TOS stores drawings in **drawing sets** keyed by symbol:

```
Symbol ("AAPL")
  +-- Drawing Set ("Default")
       |-- Drawing 1 (trendline, anchors: [(t1,p1), (t2,p2)])
       |-- Drawing 2 (horizontal line, price: 185.50)
       +-- Drawing 3 (fibonacci, anchors: [(t1,p1), (t2,p2)])
```

- Each symbol has one or more named drawing sets (default: "Default").
- Drawing sets are **cloud-stored** and synced across all devices.
- Drawing sets are **timeframe-agnostic** -- the same set applies whether viewing daily, hourly, or 5-minute.
- Users can create multiple drawing sets per symbol for organizational purposes.

### TradingView: Separate Drawings Storage

When `saveload_separate_drawings_storage` is enabled, drawings are stored independently from chart layouts, keyed by symbol:

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

Null values indicate deleted drawings (tombstone pattern).

### NinjaTrader: Global Drawing Objects

Per-drawing visibility flag rather than global sync:

- Each drawing has an "Attach to" property: "This chart" (default) or "All charts"
- "All charts" creates Global Drawing Objects scoped to the instrument
- Storage: XML files in `Documents\NinjaTrader 8\templates\GlobalDrawingObject\`
- Implementation: Global objects are **clones** of the original, updated on modification

### Advantages of Per-Symbol Storage

- Natural deduplication -- no duplicate drawings on two AAPL charts
- Adding a new chart for a symbol immediately shows all existing drawings
- Clean persistence -- one file/record per symbol
- Matches the `LevelStore` design already chosen for Hand of Midas

### Disadvantage: Timeframe Clutter

Timeframe-specific drawings (e.g., a pattern visible only on 5-min) always appear on all timeframes. Mitigation: allow per-drawing visibility filters (TOS drawing sets, NinjaTrader "Attach to" property).

---

## 3. Color-Coded Link Groups

### Pattern

Every major platform uses some form of color/emoji/number-coded chart groups for symbol routing:

- **Bloomberg**: Color-coded Launchpad Groups (security groups and monitor groups)
- **ThinkOrSwim**: Color strip next to symbol field
- **TradingView**: Emoji-based chart grouping within layouts
- **Sierra Chart**: Link Numbers (integers) within Chartbooks

### What Link Groups Sync

Link groups primarily sync **symbol selection**. When you click a symbol in a watchlist linked to the "blue" group, all "blue" panels switch to that symbol.

Optionally synced: crosshair position, scroll position, timeframe. Generally does NOT auto-sync drawings (that is a separate system).

### Implementation Pattern

```rust
struct LinkGroupManager {
    groups: HashMap<LinkGroupId, Vec<ChartId>>,
}

impl LinkGroupManager {
    fn set_symbol(&mut self, group: LinkGroupId, symbol: &str) {
        for chart_id in &self.groups[&group] {
            // notify each chart to switch symbol
        }
    }
}
```

### Bloomberg's Model

Bloomberg's approach is **security-centric linking**: the group says "these panels all show the same security," and each panel independently renders its view. Annotation sharing is handled at a higher level (collaboration/sharing) rather than automatic per-symbol propagation.

---

## 4. Event-Driven Push Sync

### MetaTrader's Event System

MT5 provides `OnChartEvent()` with object-related events:

- `CHARTEVENT_OBJECT_CREATE` -- fired when a graphical object is created
- `CHARTEVENT_OBJECT_CHANGE` -- fired when object properties change
- `CHARTEVENT_OBJECT_DELETE` -- fired when an object is deleted
- `CHARTEVENT_OBJECT_DRAG` -- fired when an object is dragged

### Two Sync Approaches

**Polling** (simple but delayed):
- Timer-based periodic scan of all objects on all charts
- Compare current state against last-known state
- Clone changed objects to target charts
- CPU cost grows with object count

**Event-driven** (recommended):
- React immediately to object creation/modification via events
- On object change: enumerate all charts with same symbol, update each
- Minimal delay (next event loop iteration)
- More complex: must handle recursion prevention

### Push Sync Implementation Pattern

```
on_drawing_changed(source_chart, drawing) {
    for chart in all_charts_with_same_symbol(drawing.symbol) {
        if chart != source_chart {
            chart.upsert_drawing(drawing.clone());
        }
    }
}
```

---

## 5. Recursion Prevention

When chart B receives a synced drawing, it must not fire another sync event back to chart A. This is the main engineering challenge in bidirectional sync.

### Solution 1: Guard Flag

```rust
if self.syncing { return; }
self.syncing = true;
// push changes to other charts
self.syncing = false;
```

### Solution 2: Source Tracking

Each drawing carries a `source_chart_id`. Skip sync if the incoming drawing's source matches the current chart.

### Solution 3: Generation Counter

Each drawing has a generation number. Skip sync if the incoming generation matches what we already have.

### Recommended for Hand of Midas

Since Midas uses a centralized per-symbol store (`LevelStore`, future `AnnotationStore`) rather than per-chart storage with sync, **recursion is not a problem**. All charts read the same data. There is no copying, no sync events, no recursion. This is architecturally superior to MetaTrader's event-driven sync or NinjaTrader's clone model.

---

## 6. Time-Anchored Drawing Challenges Across Timeframes

### Category A: Price-Only Annotations (Trivial)

Horizontal lines, price alerts, order levels. No time component. A horizontal line at $185.50 is $185.50 on every timeframe. Every platform handles these identically.

### Category B: Price+Time Annotations (Hard)

Trendlines, rectangles, Fibonacci retracements, text notes. Anchored at (timestamp, price) points.

**Problem 1: Bar Boundary Mismatch**

A trendline connecting the 10:00 5-min bar high to the 14:30 5-min bar high: on a daily chart, both timestamps are within the same bar. The daily chart has no concept of "10:00 high" vs "14:30 high." The trendline collapses.

**Problem 2: Timestamp Snapping**

A timestamp of 10:17 snaps to the 10:15 bar on a 5-min chart but to the 10:00 bar on a 30-min chart. The visual position shifts.

**Problem 3: Extended Periods**

A multi-month daily trendline on a 1-minute chart extends far beyond the viewport as a nearly flat line.

### How Platforms Handle It

| Platform | Approach |
|---|---|
| TOS | Show on all timeframes, anchor at nearest bar |
| NinjaTrader | Show on all charts of same instrument |
| MetaTrader | User-controlled (EA logic decides) |
| Sierra Chart | User explicitly chooses which charts copy |
| TradingView | Show on all timeframes, warn about differences |

### Recommended Two-Tier Model

1. **Price-only annotations** (horizontal levels, order brackets): Per-symbol, automatic sync to all charts of the same symbol, all timeframes. Already implemented via `LevelStore`.

2. **Price+time annotations** (trendlines, rectangles, notes): Per-symbol with optional timeframe visibility filter:

```rust
struct Annotation {
    kind: AnnotationKind,
    anchors: Vec<(i64, f64)>,  // (timestamp_ms, price)
    // None = visible on all timeframes
    visible_timeframes: Option<Vec<Timeframe>>,
}
```

---

## 7. Order Bracket Display

Order brackets are special because they represent **live server-side state**, not user-drawn annotations:

1. **Orders exist independently of charts.** An order at $185.50 exists in the broker's system regardless of which charts are open.

2. **Visualization should be per-symbol, automatic.** Every chart showing AAPL should display active AAPL orders.

3. **Order brackets are price-only.** Entry, TP, and SL are horizontal price levels with no time anchor (the order exists now, at a price, until filled or canceled). Cross-timeframe display is trivial.

4. **Visual distinction from user drawings.** Different colors (green for TP, red for SL, blue/white for entry), dashed lines, quantity/type labels.

### NinjaTrader Limitation

Only one Chart Trader panel per chart window. Order visualization does not automatically propagate to other charts of the same instrument. Users frequently request this feature.

### Recommended Approach

Store active order levels in a structure analogous to `LevelStore` but managed by the order bridge (midas-app), not the user. All charts showing the order's symbol render order levels automatically. Order levels are read-only on the chart (modifications go through the order management system).

---

## 8. Performance at Scale: 20+ Charts

### Event Storm on Drawing Change

If a user drags a horizontal level and 20 charts need to update:
- **Naive**: Fire 20 redraw events, 20 separate GPU prepare passes
- **Better**: Batch invalidation. Mark all affected charts as dirty in a single pass, redraw all dirty charts in the next frame. This is how Midas works with generation-counter dirty tracking.

### Lookup Cost: "Which Charts Show This Symbol?"

Every drawing change needs to find all charts displaying the affected symbol:
- **Naive**: O(n) scan of all charts for each change
- **Better**: Maintain an index `HashMap<Symbol, Vec<ChartId>>`. Lookup is O(1).

### Drawing Cloning vs Reference Sharing

NinjaTrader's clone model creates N copies of each global drawing for N charts. Wastes memory, makes updates O(n).

**Better approach** (TOS and Midas's LevelStore):
- Single source of truth per symbol
- All charts reference the same data via `LevelStore::levels_for("AAPL")`
- No cloning, no sync delay, changes visible on the same frame

### GPU Buffer Updates

When a shared level changes, all charts showing that symbol need new GPU instances. Use generation counters: each chart caches the last-seen generation. Compare `store.generation("AAPL")` with the cached value. If different, rebuild level instances. Cost: one integer comparison per chart per frame.

### Crosshair Sync Overhead

Single `crosshair_sync: Option<(ChartId, i64, String)>` on MidasApp. One write per mouse move, one read per chart per frame. For 20 charts: 1 write + 20 reads = 21 operations per mouse move. Sub-microsecond total.

---

## 9. Persistence Model Recommendation

### Current Plan

Annotations stored as `AAPL_D1.json` (per-symbol-per-timeframe).

### Recommended Change

Store annotations per-symbol with an optional `visible_timeframes` field on each annotation:

```json
{
  "symbol": "AAPL",
  "annotations": [
    {
      "kind": "HorizontalLine",
      "price": 185.50,
      "visible_timeframes": null
    },
    {
      "kind": "TrendLine",
      "anchors": [{"t": 1711584000, "p": 180.0}, {"t": 1711670400, "p": 190.0}],
      "visible_timeframes": ["D1", "W1"]
    }
  ]
}
```

This matches the TOS/TradingView model where a drawing is defined once and displayed across timeframes. `visible_timeframes: null` means "show on all timeframes."

### AnnotationStore Should Follow LevelStore Pattern

Centralized, per-symbol, owned by MidasApp, passed by reference. Not per-chart. This avoids sync entirely -- all charts read the same source of truth.

---

## 10. Summary: What Every Platform Agrees On

1. **Horizontal price levels belong per-symbol.** Every platform syncs these across timeframes.
2. **Color-coded link groups** are the standard UX for symbol routing.
3. **Time-anchored drawings are inherently imperfect** across timeframes. Accept this.
4. **Order brackets are distinct** from user annotations (server-owned vs user-owned).
5. **Centralized storage beats clone-and-sync.** TOS and TradingView's per-symbol model is cleaner than NinjaTrader's clone model or MetaTrader's event-driven sync.
