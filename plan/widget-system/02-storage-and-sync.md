# 02 -- Storage, Synchronization, and Persistence

**Status:** DRAFT
**Date:** 2026-03-30
**Scope:** `midas-chart` (types, store), `midas-app` (ownership, persistence, order bridge)

> Defines how annotations are stored per-symbol, synchronized across charts
> via generation counters, and persisted to disk as per-symbol JSON files.
> Replaces the per-chart `HashMap<ChartId, Vec<HorizontalLevel>>` pattern
> with a centralized `AnnotationStore` that all charts read from.

---

## Table of Contents

1. [Storage Architecture](#1-storage-architecture)
2. [Cross-Chart Synchronization](#2-cross-chart-synchronization)
3. [Annotation Categories](#3-annotation-categories)
4. [Persistence](#4-persistence)
5. [Order Bracket Integration](#5-order-bracket-integration)
6. [Performance Analysis](#6-performance-analysis)
7. [Ownership and Lifetimes](#7-ownership-and-lifetimes)
8. [Appendix: Migration from LevelStore](#appendix-migration-from-levelstore)

---

## 1. Storage Architecture

### 1.1 AnnotationStore (Central, Per-Symbol)

The `AnnotationStore` is the single source of truth for all user-drawn
annotations across the entire application. It is keyed by symbol, not by
chart. Two charts displaying AAPL see exactly the same annotations because
they read from the same `SymbolAnnotations` entry.

```rust
/// Centralized annotation storage, owned by MidasApp.
///
/// All charts read from this store during scene computation. Mutations
/// happen exclusively in the iced update() phase. No interior mutability
/// needed -- ownership follows iced's Elm architecture.
pub struct AnnotationStore {
    /// Per-symbol annotation collections.
    by_symbol: HashMap<SymbolKey, SymbolAnnotations>,

    /// Global generation counter. Incremented on ANY mutation to ANY
    /// symbol. Used as a fast "has anything changed at all" check.
    global_generation: u64,
}

/// All annotations for a single symbol.
pub struct SymbolAnnotations {
    /// The annotations themselves. Linear scan is fine for n < 100.
    annotations: Vec<Annotation>,

    /// Per-symbol generation counter. Incremented when this symbol's
    /// annotations change. Charts compare their last-seen generation
    /// against this to decide whether to recompute annotation instances.
    generation: u64,

    /// Monotonically increasing ID counter for this symbol.
    /// IDs are scoped to the store, not to a single symbol, to ensure
    /// global uniqueness when the user selects an annotation by ID.
    next_id: u64,
}
```

**Why a single `next_id` is not on `AnnotationStore`:** The existing
`LevelStore` uses a single counter on the store itself. That works but
creates a subtle ordering dependency when loading from disk -- the counter
must be initialized to `max(all_ids_across_all_symbols) + 1`. Placing
`next_id` on `SymbolAnnotations` is simpler for persistence (each file
carries its own counter) but risks ID collisions across symbols when the
user selects an annotation by raw `u64`.

**Resolution:** Keep one global `next_id` on `AnnotationStore` itself,
matching the proven `LevelStore` pattern. The per-symbol struct does not
carry `next_id`:

```rust
pub struct AnnotationStore {
    by_symbol: HashMap<SymbolKey, SymbolAnnotations>,
    global_generation: u64,
    next_id: u64,
}

pub struct SymbolAnnotations {
    annotations: Vec<Annotation>,
    generation: u64,
}
```

On load from disk, `next_id` is set to the maximum ID found across all
loaded symbols plus one:

```rust
impl AnnotationStore {
    /// Reconstruct from loaded files. Sets next_id to one past the
    /// highest ID found across all symbols.
    pub fn from_files(files: Vec<AnnotationFile>) -> Self {
        let mut store = Self::new();
        let mut max_id: u64 = 0;
        for file in files {
            for ann in &file.annotations {
                max_id = max_id.max(ann.id.0);
            }
            store.by_symbol.insert(
                SymbolKey::new(&file.symbol),
                SymbolAnnotations {
                    annotations: file.annotations,
                    generation: 0,
                },
            );
        }
        store.next_id = max_id + 1;
        store
    }
}
```

### 1.2 SymbolKey Design

The key for per-symbol storage. Three options were considered:

| Option | Type | Pros | Cons |
|---|---|---|---|
| Raw `String` | `"AAPL"` | Simple, matches `LevelStore` | No exchange disambiguation |
| Tuple | `(Exchange, String)` | Disambiguates `AAPL` on NYSE vs. NASDAQ | Over-engineering for v1; IB resolves exchange server-side |
| Newtype | `SymbolKey(String)` | Type safety, prevents accidental use of ChartId as key | Tiny API surface cost |

**Decision: Newtype `SymbolKey(String)`** for type safety, with a cheap
`&str` lookup via `Borrow<str>`:

```rust
/// Interned symbol key for annotation storage lookups.
///
/// A thin newtype over `String` to prevent mixing up symbol strings
/// with other strings (chart titles, file paths, etc.). Implements
/// `Borrow<str>` so `HashMap::get("AAPL")` works without allocating.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SymbolKey(String);

impl SymbolKey {
    pub fn new(symbol: &str) -> Self {
        // Normalize to uppercase. "aapl" and "AAPL" are the same symbol.
        Self(symbol.to_uppercase())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::borrow::Borrow<str> for SymbolKey {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SymbolKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for SymbolKey {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}
```

The `Borrow<str>` impl is critical. Without it, every lookup would
require allocating a `SymbolKey` just to query the HashMap. With it:

```rust
// No allocation -- HashMap uses Borrow<str> to compare keys.
let levels = store.get("AAPL");
```

**Future extension:** If exchange disambiguation becomes necessary (e.g.,
dual-listed symbols), `SymbolKey` can be extended to `SymbolKey { ticker:
String, exchange: Option<String> }` with a matching `Hash`/`Eq` impl
that ignores `exchange` when it is `None`. This is backward-compatible
with existing JSON files because `SymbolKey` serializes as a string today.

### 1.3 CRUD Operations

Complete public API. All mutations bump the generation counter. No
mutation method returns `&mut Annotation` -- instead, `update()` takes
a closure. This prevents holding a mutable borrow across unrelated
operations.

```rust
impl AnnotationStore {
    /// Creates an empty store with no annotations.
    pub fn new() -> Self {
        Self {
            by_symbol: HashMap::new(),
            global_generation: 0,
            next_id: 1,
        }
    }

    // ── Queries ──────────────────────────────────────────────────

    /// Returns all annotations for a symbol, or an empty slice if none.
    ///
    /// This is the primary read path during scene computation. O(1)
    /// HashMap lookup, returns a slice reference with zero allocation.
    pub fn get(&self, symbol: &str) -> &[Annotation] {
        self.by_symbol
            .get(symbol)
            .map_or(&[], |sa| sa.annotations.as_slice())
    }

    /// Returns annotations visible on a specific timeframe.
    ///
    /// Filters by `visible_timeframes`: annotations with `None` are
    /// always visible; annotations with `Some(tfs)` are visible only
    /// if `timeframe` is in the list.
    ///
    /// Returns references to avoid cloning annotation data.
    pub fn get_visible<'a>(
        &'a self,
        symbol: &str,
        timeframe: Timeframe,
    ) -> Vec<&'a Annotation> {
        self.get(symbol)
            .iter()
            .filter(|ann| ann.should_render_on(timeframe))
            .collect()
    }

    /// Returns the generation counter for a symbol. Returns 0 if the
    /// symbol has no annotations.
    ///
    /// Charts compare their last-seen generation against this value
    /// to determine if annotation instances need rebuilding.
    pub fn generation(&self, symbol: &str) -> u64 {
        self.by_symbol
            .get(symbol)
            .map_or(0, |sa| sa.generation)
    }

    /// Returns the global generation counter. Useful for a quick
    /// "has anything changed anywhere" check.
    pub fn global_generation(&self) -> u64 {
        self.global_generation
    }

    /// Finds an annotation by ID across all symbols. O(n*m) where
    /// n = symbols and m = annotations per symbol, but both are small.
    /// Used for editor lookups and drag operations where the symbol
    /// context may not be known.
    pub fn find(&self, id: AnnotationId) -> Option<(&str, &Annotation)> {
        for (key, sa) in &self.by_symbol {
            if let Some(ann) = sa.annotations.iter().find(|a| a.id == id) {
                return Some((key.as_str(), ann));
            }
        }
        None
    }

    // ── Mutations ────────────────────────────────────────────────

    /// Adds an annotation to a symbol. Returns the assigned ID.
    ///
    /// The caller provides an `AnnotationKind`; the store wraps it
    /// in a full `Annotation` with generated ID, timestamps, and
    /// default visibility.
    pub fn add(
        &mut self,
        symbol: &str,
        kind: AnnotationKind,
    ) -> AnnotationId {
        let id = AnnotationId(self.next_id);
        self.next_id += 1;

        let now = epoch_millis();
        let annotation = Annotation {
            id,
            kind,
            presence: Presence::Active,
            created_at: now,
            modified_at: now,
            locked: false,
            visible_timeframes: None,
        };

        let key = SymbolKey::new(symbol);
        self.by_symbol
            .entry(key)
            .or_insert_with(SymbolAnnotations::new)
            .annotations
            .push(annotation);

        self.bump_generation(symbol);
        id
    }

    /// Removes an annotation by ID from a symbol. Returns `true` if
    /// the annotation was found and removed.
    pub fn remove(&mut self, symbol: &str, id: AnnotationId) -> bool {
        let Some(sa) = self.by_symbol.get_mut(symbol) else {
            return false;
        };
        let Some(idx) = sa.annotations.iter().position(|a| a.id == id) else {
            return false;
        };
        sa.annotations.remove(idx);
        self.bump_generation(symbol);
        true
    }

    /// Updates an annotation in place via a closure. The closure
    /// receives `&mut Annotation` and can modify any field.
    ///
    /// The generation counter is bumped unconditionally. If the
    /// closure makes no changes, the cost is one wasted u64 increment
    /// (negligible) vs. tracking whether anything actually changed
    /// (complex, error-prone).
    ///
    /// Returns `true` if the annotation was found.
    pub fn update(
        &mut self,
        symbol: &str,
        id: AnnotationId,
        f: impl FnOnce(&mut Annotation),
    ) -> bool {
        let Some(sa) = self.by_symbol.get_mut(symbol) else {
            return false;
        };
        let Some(ann) = sa.annotations.iter_mut().find(|a| a.id == id) else {
            return false;
        };
        ann.modified_at = epoch_millis();
        f(ann);
        self.bump_generation(symbol);
        true
    }

    /// Removes all annotations for a symbol.
    pub fn clear(&mut self, symbol: &str) {
        if let Some(sa) = self.by_symbol.get_mut(symbol) {
            if !sa.annotations.is_empty() {
                sa.annotations.clear();
                self.bump_generation(symbol);
            }
        }
    }

    /// Removes annotations matching a predicate for a symbol.
    /// Returns the number of annotations removed.
    pub fn retain(
        &mut self,
        symbol: &str,
        pred: impl FnMut(&Annotation) -> bool,
    ) -> usize {
        let Some(sa) = self.by_symbol.get_mut(symbol) else {
            return 0;
        };
        let before = sa.annotations.len();
        sa.annotations.retain(pred);
        let removed = before - sa.annotations.len();
        if removed > 0 {
            self.bump_generation(symbol);
        }
        removed
    }

    /// Returns an iterator over all symbols that have annotations.
    pub fn symbols(&self) -> impl Iterator<Item = &str> {
        self.by_symbol.keys().map(|k| k.as_str())
    }

    // ── Internal ─────────────────────────────────────────────────

    fn bump_generation(&mut self, symbol: &str) {
        if let Some(sa) = self.by_symbol.get_mut(symbol) {
            sa.generation += 1;
        }
        self.global_generation += 1;
    }
}

impl SymbolAnnotations {
    fn new() -> Self {
        Self {
            annotations: Vec::new(),
            generation: 0,
        }
    }
}
```

**Design note on `update()` taking a closure:** This is a deliberate
choice over returning `&mut Annotation`. The closure pattern:

1. Ensures `modified_at` is always updated (the caller cannot forget).
2. Ensures `bump_generation()` is always called after the mutation.
3. Prevents the caller from holding a mutable borrow that would
   conflict with other store operations.

The existing `LevelStore.find_level_mut()` returns a raw `&mut` and
relies on the caller to remember to bump generation. That pattern works
for a 160-line module but does not scale to the full annotation system
where multiple mutation paths exist.

---

## 2. Cross-Chart Synchronization

### 2.1 How Charts Read Annotations

Charts do not own annotations. During scene computation, each chart
queries the `AnnotationStore` by its current symbol. The call chain:

```
MidasApp::view()
    -> for each chart panel:
        -> ChartRenderSnapshot captures (&symbol, &levels)
        -> ChartProgram::draw() builds ChartInput
            -> ChartInput.levels = annotation_store.get(symbol)
        -> compute_chart_scene(&input)
```

The critical insight: because `AnnotationStore` is immutable during the
view/draw phase (mutations only happen in `update()`), every chart that
displays AAPL sees exactly the same `&[Annotation]` slice. There is no
copying, no syncing, no event propagation. The data is shared by
reference.

```rust
// In MidasApp::view() -- conceptual, simplified:
fn build_chart_input<'a>(
    panel: &'a ChartPanel,
    annotations: &'a AnnotationStore,
) -> ChartInput<'a> {
    ChartInput {
        symbol: &panel.symbol,
        levels: annotations.get(&panel.symbol),
        // ... camera, colors, etc.
    }
}
```

All charts showing AAPL receive the same `&[Annotation]` slice. Chart A
and Chart B both render levels at $185.50 and $192.00 because they read
from the same memory. No copying, no event bus, no reconciliation.

### 2.2 Generation-Based Dirty Detection

Each chart maintains a `last_seen_annotation_generation: u64` in its
per-chart state (either in `DirtyTracker` or as a standalone field on
`ChartState`). The dirty check is a single integer comparison per chart
per frame:

```rust
// In DirtyFlags, add a new field:
pub struct DirtyFlags {
    pub camera: u64,
    pub candles: u64,
    pub indicators: u64,
    pub crosshair: u64,
    pub levels: u64,       // Retained for backward compat during migration
    pub annotations: u64,  // NEW: annotation generation tracker
    pub grid: u64,
    pub theme: u64,
}
```

The flow during each frame:

```
1. Chart reads annotation_store.generation("AAPL")  -> current_gen
2. Chart reads its own dirty.annotations             -> last_seen_gen
3. If current_gen != last_seen_gen:
     a. Rebuild annotation GPU instances
     b. Set dirty.annotations = current_gen
4. If current_gen == last_seen_gen:
     a. Skip annotation rebuild (reuse cached GPU data)
```

However, there is a subtlety. The `DirtyFlags` on `ChartState` are
per-chart counters that the chart increments itself (e.g.,
`dirty.mark_camera()` when the user pans). The annotation generation
lives on `AnnotationStore`, not on the chart. So the chart needs a
separate tracking field:

```rust
/// Per-chart tracking of the last annotation generation this chart
/// processed. Stored on ChartState or on ChartPanel.
pub struct AnnotationTracker {
    /// The annotation store generation last seen by this chart.
    last_seen_generation: u64,
}

impl AnnotationTracker {
    pub fn new() -> Self {
        Self { last_seen_generation: 0 }
    }

    /// Returns true if the store's generation for this symbol has
    /// changed since the last acknowledgment.
    pub fn needs_rebuild(&self, store: &AnnotationStore, symbol: &str) -> bool {
        store.generation(symbol) != self.last_seen_generation
    }

    /// Record that we have processed the current generation.
    pub fn acknowledge(&mut self, store: &AnnotationStore, symbol: &str) {
        self.last_seen_generation = store.generation(symbol);
    }
}
```

This integrates with the existing `DirtyTracker` pattern from
`midas-chart/src/dirty.rs`. The chart's `DirtyTracker` already stores
`last_seen: DirtyFlags` and compares against the current `DirtyFlags`.
The annotation tracker follows the same idiom but reads from a different
source (the store, not the chart's own flags).

**Relationship between `AnnotationTracker` and `DirtyFlags.annotations`:**
`AnnotationTracker` (reading from the store's generation counter) gates
the **compute** decision — whether to re-run `compute_widget_outputs()`.
`DirtyFlags.annotations` gates the **GPU upload** decision — whether to
re-upload annotation instance buffers. In practice, `mark_camera()` bumps
`DirtyFlags.annotations` (since screen positions change), while
`AnnotationTracker` detects data-level changes (CRUD operations).
Both must trigger recomputation; the AnnotationTracker is the primary
mechanism for data changes, while DirtyFlags handles camera/theme cascades.

**Where `AnnotationTracker` lives:** On `ChartPanel` in `midas-app`,
not on `ChartState` in `midas-chart`. Rationale: `ChartState` is a
sans-IO pure state machine. It should not know about the `AnnotationStore`
type. The app layer owns the relationship between charts and the store.

```rust
pub struct ChartPanel {
    pub symbol: String,
    pub timeframe: Timeframe,
    pub data: Option<Arc<CandleBuffer>>,
    pub chart_state: ChartState,
    pub load_state: LoadState,
    pub symbol_input: String,
    // ... existing fields ...

    /// Tracks whether this chart needs to rebuild annotation instances.
    pub annotation_tracker: AnnotationTracker,
}
```

### 2.3 Why No Event System Needed

With centralized storage and generation counters, an event-driven sync
system would add complexity for zero benefit. Consider the alternatives:

**Event-driven approach (rejected):**
```
User creates level on Chart A
  -> LevelCreated event emitted
  -> Event bus dispatches to Chart B, Chart C, Chart D
  -> Each chart processes the event, rebuilds instances
  -> Need to handle: duplicate events, ordering, recursion
     (what if processing an event triggers another event?),
     event storms (100 charts * 10 annotations = 1000 events)
```

**Generation counter approach (chosen):**
```
User creates level on Chart A
  -> AnnotationStore.add("AAPL", level) bumps AAPL generation to 7
  -> Next frame:
     Chart A: generation("AAPL") == 7, last_seen == 6 -> rebuild
     Chart B: generation("AAPL") == 7, last_seen == 6 -> rebuild
     Chart C (shows MSFT): generation("MSFT") == 3, last_seen == 3 -> skip
```

Properties of the generation approach:

1. **No recursion.** There are no events to handle, so there is no
   possibility of event handlers triggering further events.
2. **No event storms.** If the user rapidly drags a level through 60
   positions in one second, the generation counter reaches 66. Each
   chart simply compares `66 != 60` and rebuilds once per frame.
3. **No ordering bugs.** All charts read the same authoritative state.
   There is no question of "did Chart B see the create before the
   update?" -- it sees whatever the store contains at read time.
4. **O(1) per chart per frame.** One HashMap lookup + one integer
   comparison. The existing `DirtyTracker` already proves this pattern
   at 60fps with 20 charts.
5. **No wasted work.** Charts showing a different symbol skip the
   rebuild entirely. Charts showing the same symbol rebuild only when
   the generation changes.

### 2.4 Link Groups (Future)

Color-coded chart groups are the professional mechanism for routing
symbol changes across panels. Bloomberg, ThinkOrSwim, and NinjaTrader
all use colored dots or strips next to the symbol field.

**Concept:** Charts in the same link group share a symbol. Changing the
symbol on one chart changes it on all charts in the group. Charts with
no group assignment are independent.

```rust
/// Color-coded link group for symbol routing.
///
/// Charts in the same link group share a symbol. Changing the symbol
/// on one chart in the group changes it on all charts in the group.
///
/// FUTURE: Not implemented in v1. Defined here for data model
/// compatibility so the config schema does not need to change when
/// link groups are added.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LinkColor {
    Red,
    Green,
    Blue,
    Yellow,
    Orange,
    Purple,
    Cyan,
    Magenta,
}

/// Data model for link group membership. Stored on the chart panel,
/// not on the annotation store (link groups affect symbol routing,
/// not annotation storage).
///
/// FUTURE: Not active in v1. Present in ChartPanel for forward-
/// compatible config serialization.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LinkGroupMembership {
    /// The link color this chart belongs to, or None for independent.
    pub color: Option<LinkColor>,
}
```

Link groups interact with annotations as follows: when a link group
changes symbol (user types "MSFT" in a Red-group chart), all Red-group
charts switch to MSFT and start reading from
`annotation_store.get("MSFT")`. The annotations themselves do not move
or change -- only the charts' symbol pointers change. This is the same
mechanism as manually changing a chart's symbol today, just automated
across a group.

---

## 3. Annotation Categories

### 3.1 Price-Only Annotations

Annotations anchored solely by price. These have no time coordinate and
span the full width of the chart.

**Examples:** Horizontal support/resistance levels, alert price lines,
moving average price markers.

```rust
/// Horizontal line at a specific price. The simplest annotation type.
/// Wraps the existing level concept in the annotation system.
pub struct HorizontalLevel {
    pub price: f64,
    pub color: [f32; 4],
    pub line_width: f32,
    pub style: LineStyle,
    pub label: Option<String>,
    pub icon: LevelIcon,
}
```

**Cross-timeframe behavior:** Trivial. $185.50 is $185.50 whether the
chart shows 1-minute bars or daily bars. The price coordinate maps
through `Camera2D::price_to_y()` identically regardless of the time
axis configuration.

**Timeframe filtering:** Price-only annotations use
`visible_timeframes: None`, meaning they appear on all timeframes.
There is no use case for a horizontal level that is visible on the
daily chart but hidden on the 5-minute chart. If a user wants this,
they can set `visible: false` on individual charts (a per-chart view
preference, not a per-annotation property).

**Order brackets from the broker** are also price-only in their display:
the entry, take-profit, and stop-loss legs are horizontal lines at
specific prices. They have an additional `timestamp` on the `Ray` anchor
for rendering purposes (the entry line starts at the fill time and
extends right), but the price is the primary coordinate.

### 3.2 Time-Anchored Annotations

Annotations with both price and time coordinates. These render at
specific positions in the chart's price/time space.

**Examples:** Trendlines (future), rectangles (future), text notes,
fill markers, signal arrows.

```rust
// TextNote: see canonical definition in 01-core-architecture.md Section 2.2.5.
// Key fields: price, timestamp, text, font_size, text_color, background_color,
// max_width, show_border, show_connector.

/// An icon/stamp at a specific chart coordinate.
pub struct MarkerAnnotation {
    pub price: f64,
    pub timestamp: i64,    // epoch millis
    pub icon: MarkerIcon,
    pub size: f32,
}
```

**Cross-timeframe behavior:** Non-trivial. A marker at timestamp
`2026-03-29T14:30:00Z` exists on the 5-minute chart at a specific bar.
On the daily chart, that timestamp falls within the March 29 daily bar.
The marker still renders, but its X position snaps to the nearest
available bar.

**Timeframe filtering:** Time-anchored annotations should carry an
optional `visible_timeframes` field:

```rust
pub struct Annotation {
    pub id: AnnotationId,
    pub kind: AnnotationKind,
    // ...
    /// If None, visible on all timeframes.
    /// If Some, visible only on listed timeframes.
    /// Price-only annotations typically use None.
    /// Time-anchored annotations may restrict to specific timeframes.
    pub visible_timeframes: Option<Vec<Timeframe>>,
}
```

**Timestamp snapping logic for cross-timeframe display:**

```rust
/// Find the bar index closest to a timestamp on a given timeframe.
///
/// Used when a time-anchored annotation from a 5m chart needs to
/// render on a daily chart. The daily chart finds the daily bar
/// containing the 5m timestamp and renders the annotation there.
fn snap_timestamp_to_bar(
    data: &dyn CandleData,
    timestamp: i64,
) -> Option<usize> {
    // Binary search for the bar whose timestamp range contains
    // or is closest to the target timestamp.
    let timestamps = data.timestamps();
    match timestamps.binary_search(&timestamp) {
        Ok(idx) => Some(idx),
        Err(idx) => {
            if idx == 0 {
                Some(0)
            } else if idx >= timestamps.len() {
                Some(timestamps.len() - 1)
            } else {
                // Pick the closer bar.
                let before = timestamps[idx - 1];
                let after = timestamps[idx];
                if (timestamp - before) <= (after - timestamp) {
                    Some(idx - 1)
                } else {
                    Some(idx)
                }
            }
        }
    }
}
```

### 3.3 Server-Owned Annotations (Order Brackets)

Order brackets from the broker engine are a distinct category. They
look like annotations on the chart but have fundamentally different
ownership:

| Property | User Annotations | Order Brackets |
|---|---|---|
| Created by | User drawing on chart | BrokerEngine fill/order events |
| Editable on chart | Yes (drag, delete, edit) | Read-only display |
| Persisted to disk | Yes (JSON files) | No (reconstructed from broker state) |
| Lifecycle | Until user deletes | Until order fills/cancels |
| Visual style | User-configured colors | Fixed style (dashed, quantity labels) |
| Stored in | AnnotationStore | AnnotationStore (tagged) |

Order brackets are stored in the same `AnnotationStore`. Broker-owned
brackets are distinguished by their `BracketStatus` (Active, Pending,
etc.) and by an `OrderAnnotationLink` mapping maintained in the app
layer (see `01-core-architecture.md` "Why external_id is absent").
The rendering pipeline treats all annotations uniformly while the
interaction layer checks `bracket.status` to prevent editing live orders.

```rust
// An order bracket annotation. Created by the app's order bridge,
// not by user drawing tools.
let ann_id = store.add("AAPL", AnnotationKind::OrderBracket(OrderBracket {
    entry: BracketLeg { price: 185.50, /* ... */ },
    take_profit: Some(BracketLeg { price: 192.00, /* ... */ }),
    stop_loss: Some(BracketLeg { price: 182.00, /* ... */ }),
    side: BracketSide::Long,
    status: BracketStatus::Active,
    quantity: Some(100.0),
}));

// App-layer mapping: order_id -> annotation_id
// NOT stored on the Annotation struct.
order_link_map.insert(broker_order_id, ann_id);
```

**Visual distinction:** The rendering pipeline checks the
`BracketStatus` to apply order-specific styling:

- Dashed lines instead of solid (entry, TP, SL legs)
- Muted colors (not user-configurable -- fixed palette)
- Quantity labels ("100 @ 185.50")
- Semi-transparent fill between TP and SL zones
- Right-click context menu offers "Modify Order" and "Cancel Order"
  instead of "Edit" and "Delete"

---

## 4. Persistence

### 4.1 File Format

Annotations are persisted as per-symbol JSON files. One file per symbol
that has at least one annotation.

```
data/annotations/
    AAPL.json
    MSFT.json
    ES_FUT_202406.json
```

**File naming convention:** The filename is the `SymbolKey` with
characters unsafe for filenames replaced:

```rust
/// Convert a SymbolKey to a safe filename stem.
///
/// Replaces `/` and `:` with `_` to handle futures contract symbols
/// like "ES/FUT/202406" -> "ES_FUT_202406".
fn symbol_to_filename(symbol: &SymbolKey) -> String {
    symbol
        .as_str()
        .replace('/', "_")
        .replace(':', "_")
        .replace('\\', "_")
}
```

**Why per-symbol, not per-symbol-per-timeframe:**

An earlier (superseded) annotations plan proposed per-symbol-per-timeframe
files (`AAPL_D1.json`, `AAPL_M5.json`). This document supersedes that
decision based on the research synthesis:

1. **Per-symbol storage is the universal professional pattern.** Bloomberg,
   ThinkOrSwim, and TradingView all store annotations per-symbol. The
   `visible_timeframes` field handles per-timeframe filtering without
   splitting the storage.

2. **Price-only annotations (the majority) are timeframe-agnostic.** A
   support level at $185.50 belongs to AAPL, not to AAPL-on-daily. Per-
   timeframe files would duplicate these across every timeframe file.

3. **Simpler persistence.** One file to load, one file to save, one file
   to back up. No need to merge or split when the user changes timeframes.

4. **The `visible_timeframes` field handles the exception.** A trendline
   the user drew on the 5-minute chart can carry
   `visible_timeframes: Some(vec![Timeframe::M5])` and remain invisible
   on other timeframes, all within the single per-symbol file.

**File schema:**

```json
{
    "version": 1,
    "symbol": "AAPL",
    "annotations": [
        {
            "id": 12,
            "kind": {
                "Level": {
                    "price": 185.50,
                    "color": [0.0, 0.8, 0.0, 1.0],
                    "line_width": 1.0,
                    "style": "Solid",
                    "label": "Support",
                    "icon": "none"
                }
            },
            "presence": "Active",
            "created_at": 1711584000000,
            "modified_at": 1711584000000,
            "locked": false,
            "visible_timeframes": null
        },
        {
            "id": 15,
            "kind": {
                "TextNote": {
                    "price": 190.25,
                    "timestamp": 1711584000000,
                    "text": "Earnings gap up",
                    "font_size": 12.0,
                    "background_color": null
                }
            },
            "presence": "Active",
            "created_at": 1711584000000,
            "modified_at": 1711584000000,
            "locked": false,
            "visible_timeframes": [{ "Minutes": 5 }]
        }
    ]
}
```

### 4.2 Rust Types for Persistence

```rust
/// Top-level file format for annotation persistence.
#[derive(Debug, Serialize, Deserialize)]
pub struct AnnotationFile {
    /// Schema version for forward compatibility.
    pub version: u32,
    /// The symbol these annotations belong to.
    pub symbol: String,
    /// The annotations themselves.
    pub annotations: Vec<Annotation>,
}

impl AnnotationFile {
    pub const CURRENT_VERSION: u32 = 1;
}
```

### 4.3 Save Strategy

**Debounced writes:** Annotations are not saved on every individual
change. A debounce timer fires 500ms after the last mutation. If the
user is rapidly dragging a level, only the final position is saved.

**Exception: `BracketStatus` transitions bypass the debounce timer.**
Mutations to `BracketStatus` (Draft→Pending, Pending→Active) trigger an
immediate write because these represent financial intent that must survive
a crash.

```rust
/// Save debounce configuration for annotation persistence.
const ANNOTATION_SAVE_DEBOUNCE_MS: u64 = 500;

/// Tracks which symbols need saving and when.
pub struct AnnotationPersistence {
    /// Symbols with unsaved changes.
    dirty_symbols: HashSet<SymbolKey>,
    /// When the last mutation occurred (for debounce).
    last_mutation: Option<Instant>,
    /// Path to the annotations directory.
    annotations_dir: PathBuf,
}

impl AnnotationPersistence {
    /// Called after any annotation mutation. Marks the symbol as dirty
    /// and resets the debounce timer.
    pub fn mark_dirty(&mut self, symbol: &SymbolKey) {
        self.dirty_symbols.insert(symbol.clone());
        self.last_mutation = Some(Instant::now());
    }

    /// Called on each frame tick. If enough time has passed since the
    /// last mutation, saves all dirty symbols.
    pub fn tick(&mut self, store: &AnnotationStore) {
        let Some(last) = self.last_mutation else { return };
        if last.elapsed().as_millis() < ANNOTATION_SAVE_DEBOUNCE_MS as u128 {
            return;
        }
        self.flush(store);
    }

    /// Force-save all dirty symbols. Called on application shutdown.
    pub fn flush(&mut self, store: &AnnotationStore) {
        for symbol in self.dirty_symbols.drain() {
            let annotations = store.get(symbol.as_str());
            if annotations.is_empty() {
                // Delete the file if no annotations remain.
                let path = self.file_path(&symbol);
                let _ = std::fs::remove_file(&path);
                continue;
            }
            let file = AnnotationFile {
                version: AnnotationFile::CURRENT_VERSION,
                symbol: symbol.as_str().to_owned(),
                annotations: annotations
                    .iter()
                    .filter(|a| !is_broker_owned(a)) // Skip server-owned brackets
                    .cloned()
                    .collect(),
            };
            if let Err(e) = self.write_atomic(&symbol, &file) {
                tracing::error!("Failed to save annotations for {}: {}", symbol, e);
            }
        }
        self.last_mutation = None;
    }

    // ── Crash recovery note ───────────────────────────────────
    //
    // If the application crashes between a mutation and the debounce
    // flush, unsaved annotation changes are lost. This is an accepted
    // trade-off: annotations are non-critical analysis data (not
    // financial records), and the 500ms debounce window limits the
    // maximum data loss to ~0.5 seconds of edits. Order brackets are
    // server-owned and reconstructed from broker state on reconnect,
    // so they are never at risk.
    //
    // If this proves insufficient, reduce ANNOTATION_SAVE_DEBOUNCE_MS
    // for order-related annotations or add immediate-flush for bracket
    // creation/deletion.

    /// Atomic write: serialize to .tmp file, then rename.
    /// This prevents data loss if the process crashes mid-write.
    fn write_atomic(
        &self,
        symbol: &SymbolKey,
        file: &AnnotationFile,
    ) -> std::io::Result<()> {
        let path = self.file_path(symbol);
        let tmp_path = path.with_extension("json.tmp");

        let json = serde_json::to_string_pretty(file)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(&tmp_path, json.as_bytes())?;
        std::fs::rename(&tmp_path, &path)?;
        Ok(())
    }

    fn file_path(&self, symbol: &SymbolKey) -> PathBuf {
        let filename = symbol_to_filename(symbol);
        self.annotations_dir.join(format!("{}.json", filename))
    }
}
```

**What is saved:** Only user-created annotations. Server-owned order
brackets (identified by `BracketStatus::Active/Pending/Filled` or via
`is_broker_owned()` helper using the app-layer `OrderAnnotationLink`) are
excluded from persistence because they are reconstructed from broker state
on each session.

**When saving occurs:**
- 500ms debounce after last mutation (normal operation)
- On application shutdown (forced flush)
- Never during rapid drag sequences (debounce prevents this)

### 4.4 Schema Versioning

The `version` field in the JSON file enables forward-compatible schema
evolution without data loss:

```rust
/// Load annotations from a JSON file, handling version migration.
pub fn load_annotation_file(path: &Path) -> Result<AnnotationFile> {
    let bytes = std::fs::read(path)?;
    let raw: serde_json::Value = serde_json::from_slice(&bytes)?;

    let version = raw.get("version")
        .and_then(|v| v.as_u64())
        .unwrap_or(1) as u32;

    match version {
        1 => {
            let file: AnnotationFile = serde_json::from_value(raw)?;
            Ok(file)
        }
        // Future versions:
        // 2 => migrate_v2_to_current(raw),
        v => {
            tracing::warn!(
                "Unknown annotation file version {}, attempting best-effort load",
                v
            );
            let file: AnnotationFile = serde_json::from_value(raw)?;
            Ok(file)
        }
    }
}
```

**serde attributes for forward compatibility:** All annotation types
use `#[serde(default)]` on optional fields so that a v1 file missing a
field added in v2 loads successfully with the default value.

The canonical `Annotation` struct is defined in `01-core-architecture.md`
Section 2.4. Key serde attributes:

```rust
/// Canonical definition — see 01-core-architecture.md Section 2.4.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Annotation {
    pub id: AnnotationId,
    pub kind: AnnotationKind,
    #[serde(default)]
    pub presence: Presence,  // Defaults to Active for v1 files
    #[serde(default)]
    pub visible_timeframes: Option<Vec<Timeframe>>,
    #[serde(default)]
    pub locked: bool,
    pub created_at: i64,
    pub modified_at: i64,
}
```

**Forward compatibility for new `AnnotationKind` variants:** When a
new variant is added (e.g., `Trendline` in Phase 5), files saved with
that variant will fail to deserialize on older code. To handle this
gracefully, the JSON loading code uses `serde_json::Value` as an
intermediate step (see `load_annotation_file()` above) — unknown
`kind` variants produce a serde error that is logged and the
individual annotation is skipped, not the entire file.

Each phase that adds a new `AnnotationKind` variant MUST also add a
persistence round-trip test for that variant.
```

### 4.5 Loading at Startup

On application startup, all `.json` files in the annotations directory
are loaded and merged into a single `AnnotationStore`:

```rust
/// Load all annotation files from the annotations directory.
pub fn load_all_annotations(dir: &Path) -> AnnotationStore {
    let mut files = Vec::new();

    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map_or(false, |ext| ext == "json") {
                match load_annotation_file(&path) {
                    Ok(file) => files.push(file),
                    Err(e) => {
                        tracing::error!(
                            "Failed to load annotation file {:?}: {}",
                            path,
                            e
                        );
                    }
                }
            }
        }
    }

    AnnotationStore::from_files(files)
}
```

**Error handling:** Corrupt files are logged and skipped. The
application starts with whatever annotations loaded successfully. No
single corrupt file prevents the entire application from starting.

---

## 5. Order Bracket Integration

### 5.1 BrokerEvent to AnnotationStore Flow

Order events flow from the `BrokerEngine` through iced's message system
into the `AnnotationStore`. The app layer is the bridge -- `midas-chart`
and `midas-broker` never depend on each other.

```
BrokerEngine (midas-broker)
    -> broadcast channel: OrderEvent { order_id, status, fill_price, ... }
    -> midas-app polls channel in subscription()
    -> Message::BrokerEvent(OrderEvent)
    -> MidasApp::update() matches BrokerEvent:

    OrderSubmitted { order_id, symbol, side, entry, tp, sl, qty }
        -> annotation_store.add(symbol, AnnotationKind::OrderBracket(OrderBracket {
               status: BracketStatus::Pending,
               entry, tp, sl, side, qty,
           }))
        -> order_link_map.insert(order_id, annotation_id)
           // OrderAnnotationLink lives in midas-app, not on Annotation.
           // See 01-core-architecture.md "Why external_id is absent".

    OrderFilled { order_id, fill_price, fill_qty }
        -> find annotation via order_link_map
        -> annotation_store.update(symbol, id, |ann| {
               if let AnnotationKind::OrderBracket(ref mut b) = ann.kind {
                   b.status = BracketStatus::Active;
                   b.entry.label = Some(format!("Filled @ {:.2}", fill_price));
               }
           })
        -> optionally add a Marker annotation at fill_price/fill_time

    OrderCancelled { order_id }
        -> find annotation via order_link_map
        -> annotation_store.remove(symbol, id)

    OrderModified { order_id, new_price, leg }
        -> find annotation via order_link_map
        -> annotation_store.update(symbol, id, |ann| {
               // update the appropriate leg price
           })
```

### 5.2 Lifecycle

Order bracket annotations are transient. Their lifecycle mirrors the
broker order's lifecycle:

```
 Draft       -> User draws bracket, not yet submitted to broker
 Pending     -> Submitted to IB, awaiting fill (entry order working)
 Active      -> Entry filled, TP/SL orders working
 PartialFill -> Entry partially filled
 Filled      -> All legs complete (bracket removed or converted to marker)
 Cancelled   -> User cancelled, bracket removed from store
```

On application restart, order brackets are NOT loaded from annotation
files (they are excluded from persistence). Instead, the app queries
the broker for active orders and reconstructs bracket annotations from
the broker's current state. This ensures the chart always reflects the
actual broker state, not a potentially stale snapshot.

### 5.3 Read-Only Nature and Future Interaction

Charts display order brackets but cannot directly edit them. The
interaction layer checks the bracket status and the app-layer
`OrderAnnotationLink` before allowing drag operations:

```rust
fn can_drag(annotation: &Annotation, order_links: &OrderAnnotationLink) -> bool {
    // Broker-owned brackets: not draggable (modifications go through broker).
    if order_links.contains(annotation.id) {
        return false;
    }
    // User annotations: draggable (unless locked).
    annotation.is_draggable_on(current_timeframe)
}
```

**Future drag-to-modify flow (not v1):**

When drag-to-modify is implemented, dragging an order bracket leg on the
chart will NOT directly modify the annotation. Instead, it will:

1. Show a ghost preview line at the drag destination.
2. On mouse release, send a `BrokerCommand::ModifyOrder` through the
   broker command channel.
3. The broker processes the modification and emits an `OrderModified`
   event.
4. The app layer receives the event and updates the annotation in the
   store.

This ensures the chart always reflects confirmed broker state, not
optimistic local edits that might be rejected by the broker.

```
User drags TP leg to $195.00
  -> ChartAction::DragBracketLeg { id, leg: TakeProfit, new_price: 195.0 }
  -> app: do NOT update annotation
  -> app: send BrokerCommand::ModifyOrder { order_id, new_tp: 195.0 }
  -> broker: IB API modifies order
  -> broker: emits OrderModified { order_id, new_price: 195.0, leg: TP }
  -> app: annotation_store.update(symbol, id, |ann| { ... })
  -> chart: generation changed -> rebuild -> shows TP at $195.00
```

---

## 6. Performance Analysis

### 6.1 Scaling Expectations

| Metric | Expected Value | Worst Case |
|---|---|---|
| Annotations per symbol | 5-20 | 100 |
| Active symbols | 5-10 | 50 |
| Open charts | 4-8 | 20 |
| Charts per symbol | 1-3 | 10 |
| Total annotations in memory | 25-200 | 5,000 |

**Memory estimate:** Each `Annotation` struct is approximately 200-400
bytes (depending on label strings and tag vectors). At 5,000 annotations
(extreme worst case), total memory is under 2 MB. Negligible compared to
candle data (which can reach tens of MB per symbol at high resolution).

**Generation comparison cost:** O(1) per chart per frame.
- One `HashMap::get()` on `by_symbol` (amortized O(1)).
- One `u64 != u64` comparison.
- Total: ~5ns per chart. At 20 charts * 60fps = 1200 checks/second = 6us.

**Annotation filtering cost:** O(n) where n is annotations per symbol.
- `get_visible()` iterates the slice, checking `visible` and
  `visible_timeframes`.
- At n=100: ~100 comparisons * ~2ns each = 200ns. Negligible.

**No spatial indexing needed:** With n < 100 annotations per symbol,
linear scan for hit-testing (checking if a mouse click is near any
annotation) takes under 1us. Spatial indexing (R-tree, grid) would add
complexity for zero measurable benefit.

### 6.2 20-Chart Scenario: Frame-by-Frame Analysis

**Setup:** 20 charts open. 8 show AAPL, 6 show MSFT, 4 show TSLA, 2
show ES futures. User creates a horizontal level at $185.50 on one AAPL
chart.

**Frame N (mutation frame):**

```
1. User double-clicks on AAPL chart (Chart #3).
2. MidasApp::update() processes CreateLevel message.
3. annotation_store.add("AAPL", Level { price: 185.50, ... })
   -> AAPL generation bumps from 4 to 5
   -> global_generation bumps from 27 to 28
4. annotation_persistence.mark_dirty("AAPL")
5. update() returns. iced calls view().
```

**Frame N+1 (render frame):**

```
For each of 20 charts, during view/draw:

Charts #1-#8 (AAPL):
  - annotation_tracker.needs_rebuild(store, "AAPL")
  - store.generation("AAPL") == 5, last_seen == 4 -> true
  - Rebuild annotation GPU instances (including new level at $185.50)
  - annotation_tracker.acknowledge() -> last_seen = 5
  - Cost: 8 * (1 HashMap lookup + 1 comparison + ~8 annotation rebuilds)

Charts #9-#14 (MSFT):
  - annotation_tracker.needs_rebuild(store, "MSFT")
  - store.generation("MSFT") == 3, last_seen == 3 -> false
  - Skip annotation rebuild entirely
  - Cost: 6 * (1 HashMap lookup + 1 comparison) = ~30ns

Charts #15-#18 (TSLA):
  - Same as MSFT: skip. Cost: ~20ns

Charts #19-#20 (ES):
  - Same: skip. Cost: ~10ns

Total annotation-related work for this frame:
  - 8 charts rebuild annotation instances (~200ns each) = 1.6us
  - 12 charts skip (~5ns each) = 60ns
  - Grand total: ~1.7us << 14ms frame budget
```

**Frame N+2 (steady state):**

```
All charts: last_seen matches store generation. Zero rebuilds.
Cost: 20 * ~5ns = 100ns.
```

**Debounce save (Frame N + ~30 more frames):**

```
500ms after last mutation, annotation_persistence.tick() fires.
Saves AAPL.json with the new level. ~1ms for JSON serialization
and atomic file write. Does not block the render loop (could be
made async if needed, but 1ms is within frame budget on a non-
render frame).
```

---

## 7. Ownership and Lifetimes

### 7.1 Where AnnotationStore Lives

`AnnotationStore` is owned by `MidasApp`, the top-level iced application
state struct. This is the same ownership pattern as the existing
`LevelStore`:

```rust
pub struct MidasApp {
    pub charts: HashMap<ChartId, ChartPanel>,
    pub workspace: WorkspaceLayout,
    // ...existing fields...

    /// Centralized per-symbol annotation store. Replaces level_store.
    pub annotation_store: AnnotationStore,

    /// Persistence manager for annotation save/load.
    pub annotation_persistence: AnnotationPersistence,

    // DEPRECATED: retained during migration, then removed.
    // pub level_store: LevelStore,
}
```

### 7.2 Borrow Patterns: No RefCell, No Mutex

The iced Elm architecture guarantees a clean separation between mutation
(`update()`) and rendering (`view()`). This eliminates the need for
interior mutability:

**During `update()`:** `MidasApp` has exclusive `&mut self`. The
annotation store can be mutated freely:

```rust
fn update(&mut self, message: Message) -> Task<Message> {
    match message {
        Message::CreateLevel(chart_id, price) => {
            let symbol = &self.charts[&chart_id].symbol;
            let id = self.annotation_store.add(
                symbol,
                AnnotationKind::Level(HorizontalLevel {
                    price,
                    color: DEFAULT_LEVEL_COLOR,
                    line_width: 1.0,
                    style: LineStyle::Solid,
                    label: None,
                    icon: LevelIcon::None,
                }),
            );
            self.annotation_persistence.mark_dirty(
                &SymbolKey::new(symbol),
            );
            // ...
        }
        // ...
    }
}
```

**During `view()`:** `MidasApp` has shared `&self`. The annotation
store is read-only. `AnnotationStore::get()` returns `&[Annotation]`,
which lives as long as `&self`:

```rust
fn view(&self, _id: window::Id) -> Element<Message> {
    // For each chart panel:
    let levels = self.annotation_store.get(&panel.symbol);
    // `levels` is &[Annotation], borrows from &self.annotation_store.
    // Passed into ChartInput by reference. No clone.
    let input = ChartInput {
        levels,
        // ...
    };
    // ...
}
```

**Why this is safe without RefCell or Mutex:**

1. iced's runtime calls `update()` and `view()` sequentially, never
   concurrently. There is no data race.
2. `update()` takes `&mut self`, so it has exclusive access to the
   store for writes.
3. `view()` takes `&self`, so all reads are shared borrows. Multiple
   charts can read simultaneously because `&[Annotation]` is `Send +
   Sync`.
4. No background thread accesses `AnnotationStore`. The debounced save
   in `AnnotationPersistence::tick()` is called from `update()` (which
   has `&mut self`), and it reads from the store via a shared reference
   within the same call.

**Edge case: ChartRenderSnapshot.** The current architecture creates a
`ChartRenderSnapshot` during `view()` that captures state needed for
`draw()` (which runs on the render thread). Today this snapshot clones
the level data. With `AnnotationStore`, the snapshot should capture a
`Vec<Annotation>` clone for the relevant symbol, or use `Arc<[Annotation]>`
if the clone cost matters:

```rust
pub struct ChartRenderSnapshot {
    pub symbol: String,
    pub timeframe: Timeframe,
    pub data: Option<Arc<CandleBuffer>>,
    pub chart_state: Option<ChartState>,

    /// Annotations for this chart's symbol, cloned from the store.
    /// Cloned because draw() runs on the render thread and cannot
    /// hold a borrow into MidasApp.
    pub annotations: Vec<Annotation>,

    // ... other fields ...
}
```

The clone cost is minimal: 20 annotations at ~300 bytes each = 6 KB.
At 20 charts with 3 symbols, the worst case is 3 unique clones (one
per symbol, shared via Arc if optimized) = 18 KB. This is within noise.

### 7.3 compute_chart_scene() Signature

The scene computation function receives annotations as a borrowed slice,
fitting cleanly into the existing `ChartInput` lifetime:

```rust
pub struct ChartInput<'a> {
    pub symbol: &'a str,
    pub data: &'a dyn CandleData,
    pub camera: &'a Camera2D,
    // ...existing fields...

    /// Annotations to render. Comes from AnnotationStore::get().
    /// Replaces the current `levels: &'a [HorizontalLevel]` field.
    pub annotations: &'a [Annotation],
}
```

The `'a` lifetime ties everything to the view/draw scope. No
annotation data outlives the frame that reads it. No `'static`
bounds, no `Arc`, no heap allocation beyond what `Vec<Annotation>`
already provides.

---

## Appendix: Migration from LevelStore

### Current State

`LevelStore` exists in `midas-app/src/level_store.rs` as a
`HashMap<String, Vec<HorizontalLevel>>` with per-ticker generation
counters and a global `next_id`. It was built as the first step toward
per-symbol storage (superseding the original per-chart level ownership).

### Migration Strategy

The migration is mechanical. `LevelStore` and `AnnotationStore` share
the same fundamental design (per-symbol HashMap, generation counters,
global ID allocation). The steps:

1. **Create `AnnotationStore` alongside `LevelStore`.** Both exist
   temporarily. New code uses `AnnotationStore`; existing level code
   continues using `LevelStore`.

2. **Migrate `HorizontalLevel` to `HorizontalLevel`.** A 1:1 mapping:

   ```rust
   fn migrate_level(level: &HorizontalLevel) -> AnnotationKind {
       AnnotationKind::Level(HorizontalLevel {
           price: level.price,
           color: level.color,
           line_width: level.line_width,
           style: LineStyle::Solid,
           label: level.label.clone(),
           icon: level.icon,
       })
   }
   ```

3. **Update `ChartInput` to accept `&[Annotation]` instead of
   `&[HorizontalLevel]`.** The compute pipeline extracts
   `HorizontalLevel` data from each `Annotation` where
   `kind` matches `AnnotationKind::Level`.

4. **Update persistence.** Load from config.toml level entries, convert
   to `AnnotationFile` format, save as JSON. On next startup, load from
   JSON. The config.toml `[levels]` section becomes deprecated.

5. **Remove `LevelStore`.** Once all level operations route through
   `AnnotationStore`, delete `level_store.rs` and its imports.

### Data Preservation

No user data is lost during migration. The existing `LevelStore::to_config()`
serializes all levels to config.toml. The migration reads this, converts
to `Annotation` structs, and writes per-symbol JSON files. Both the old
config.toml entries and the new JSON files exist simultaneously until the
migration is confirmed successful. Only then are the config.toml level
entries removed.

```rust
/// One-time migration from LevelStore config to AnnotationStore JSON.
///
/// Called on startup if annotation JSON files do not exist but
/// config.toml contains level entries.
pub fn migrate_levels_to_annotations(
    level_config: &HashMap<String, Vec<LevelConfig>>,
    annotations_dir: &Path,
) -> Result<AnnotationStore> {
    let mut store = AnnotationStore::new();

    for (ticker, configs) in level_config {
        for cfg in configs {
            store.add(
                ticker,
                AnnotationKind::Level(HorizontalLevel {
                    price: cfg.price,
                    color: cfg.color,
                    line_width: cfg.line_width,
                    style: LineStyle::Solid,
                    label: cfg.label.clone(),
                    icon: LevelIcon::from_str_id(&cfg.icon),
                }),
            );
        }
    }

    // Save each symbol's annotations as a JSON file.
    for symbol in store.symbols().collect::<Vec<_>>() {
        let key = SymbolKey::new(symbol);
        let file = AnnotationFile {
            version: AnnotationFile::CURRENT_VERSION,
            symbol: symbol.to_owned(),
            annotations: store.get(symbol).to_vec(),
        };
        let path = annotations_dir.join(format!(
            "{}.json",
            symbol_to_filename(&key)
        ));
        let json = serde_json::to_string_pretty(&file)?;
        std::fs::create_dir_all(annotations_dir)?;
        std::fs::write(&path, json)?;
    }

    Ok(store)
}
```

---

## Summary of Key Decisions

| Decision | Choice | Rationale |
|---|---|---|
| Storage granularity | Per-symbol | Universal professional pattern; price levels are symbol-scoped |
| Key type | `SymbolKey` newtype | Type safety + `Borrow<str>` for allocation-free lookups |
| ID allocation | Global `next_id` on store | Matches proven LevelStore pattern; globally unique IDs |
| Sync mechanism | Generation counters | O(1) dirty check, no events, no recursion, proven in DirtyTracker |
| Persistence format | Per-symbol JSON files | Human-readable, serde-native, easy backup |
| Persistence timing | 500ms debounce + shutdown flush | Avoids write storms during drag; no data loss |
| Order brackets | Same store, tagged, read-only | Uniform rendering pipeline; edit goes through broker |
| Interior mutability | None (Elm architecture) | update() has &mut, view() has &; no RefCell needed |
| Timeframe scoping | `visible_timeframes` field | Per-symbol storage with per-timeframe filtering |
| Link groups | Data model defined, not active | Forward-compatible config; implementation deferred |
