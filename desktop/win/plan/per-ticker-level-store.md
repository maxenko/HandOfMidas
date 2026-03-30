# Per-Ticker Level Store — Shared Levels Across Charts

**Status:** SOLID (post-evaluation, all findings addressed)
**Date:** 2026-03-29
**Scope:** `midas-core` (config), `midas-chart` (state, compute, interaction), `midas-app` (store, persistence, message routing)

> Move horizontal levels from per-chart ownership to a centralized per-ticker
> store so that all charts displaying the same symbol share one canonical set
> of levels. Edits on any chart propagate instantly to every other chart
> showing that ticker.

---

## 0. Goals and Non-Goals

### Goals

1. **Single source of truth per ticker.** One `Vec<HorizontalLevel>` per
   symbol, not one per chart panel. Two AAPL charts see identical levels.
2. **Instant cross-chart propagation.** Drag a level on chart A, chart B
   redraws it at the new price on the same frame — no message round-trip
   or eventual-consistency delay.
3. **Centralized ID allocation.** One monotonic counter for the entire
   store consolidates the redundant ID allocation code paths between
   `ChartState::apply_action` and the app-layer message handler into
   a single authoritative path. Per-chart counters are eliminated.
4. **Clean persistence.** Levels serialize once per ticker in config.toml
   (not duplicated across every chart showing the same symbol).
5. **Zero user-visible behavior regression.** All existing level features
   (placement, drag, OHLC snap, lock, editor popup, delete, visibility
   toggle) work identically.

### Non-Goals

- **Per-timeframe level scoping.** Levels are per-ticker, not per
  `(ticker, timeframe)`. A support at $185 is meaningful regardless of
  whether you're viewing 1D or 5m. If this proves wrong, adding an
  optional `visible_timeframes` filter to `HorizontalLevel` later is
  trivial without a schema change.
- **Undo/redo.** Out of scope; unchanged from today.
- **Migration to annotations.** The annotations plan (08-implementation-order)
  has its own Phase 1 for migrating levels into `AnnotationStore`. This
  refactor is compatible with that — the annotations migration simply moves
  levels out of `LevelStore` into `AnnotationStore` using the same
  per-ticker key. The two plans can execute in either order.
- **Multi-tool abstraction.** `LevelTool` remains a concrete struct.
- **JSON file persistence.** Levels stay in config.toml for now. The
  annotations plan handles the JSON migration separately.

## 0.1 Alternatives Considered

**Alternative A: Keep per-chart, add a sync layer.**
Each chart still owns its levels. A background sync process copies
changes from one chart to all other charts showing the same symbol.
Pro: minimal structural change. Con: introduces consistency bugs (sync
timing, conflicting edits, duplicate levels after add), doesn't solve
the duplicate-ID problem, and config bloats with N copies of the same
levels. Rejected.

**Alternative B: Per-ticker-per-timeframe store.**
Key the store on `(symbol, timeframe)` instead of `symbol`.
Pro: some traders draw timeframe-specific levels. Con: the common case
is that a support/resistance level is relevant at all timeframes. Users
who want per-timeframe scoping can use labels or visibility toggles.
Starting with per-ticker is simpler and can be extended later. Deferred.

**Alternative C: Shared `Arc<RwLock<LevelStore>>` for thread safety.**
Pro: future-proofs for multi-threaded access. Con: all level access is
single-threaded today (iced update loop). `RwLock` adds contention risk
and API noise for no benefit. Using `&mut LevelStore` references is
simpler and idiomatic. Rejected.

**Chosen: `LevelStore` as a flat `HashMap<String, Vec<HorizontalLevel>>`
owned by `MidasApp`, passed by `&`/`&mut` reference through the call
chain.** Simplest design that achieves all goals.

## 0.2 Known Behavioral Changes

**Level IDs become globally unique, not per-chart unique.** Previously
two charts could each have a level with `id: 1`. After this change, IDs
are allocated from a single counter on `LevelStore`. This is invisible
to the user but affects how `selected_level: Option<u64>` works — a
chart can select a level that "belongs" to the store, not to the chart.
This is correct behavior.

**show_levels becomes per-chart, levels become per-ticker.** A user can
hide levels on one chart while they remain visible on another chart
showing the same ticker. This is the right UX: visibility is a view
preference, not a data property.

**Config format changes.** Levels move from `[[charts]].levels` to a
new `[levels]` table. A one-time migration reads existing per-chart
levels and deduplicates them into the new format. Old configs without
the `[levels]` table continue to load via backward-compat migration.

---

## 1. Type Design

### 1.1 LevelStore (new — `midas-app`)

```rust
/// Centralized store for horizontal price levels, keyed by ticker symbol.
///
/// Owned by `MidasApp`. Passed as `&LevelStore` for reads (compute,
/// render snapshot) and `&mut LevelStore` for writes (create, drag,
/// delete, edit).
pub struct LevelStore {
    /// Ticker symbol → levels for that ticker.
    levels: HashMap<String, Vec<HorizontalLevel>>,
    /// Monotonically incrementing ID counter, shared across all tickers.
    next_id: u64,
}

impl LevelStore {
    pub fn new() -> Self;

    // ── Queries ──────────────────────────────────────────────────

    /// Returns the levels for a ticker, or an empty slice if none exist.
    pub fn levels_for(&self, ticker: &str) -> &[HorizontalLevel];

    /// Returns a mutable reference to the levels for a ticker,
    /// creating an empty entry if needed.
    pub fn levels_for_mut(&mut self, ticker: &str) -> &mut Vec<HorizontalLevel>;

    /// Finds a level by ID across all tickers. O(n) but n is small
    /// (typically < 50 levels total). Used only for editor lookups.
    pub fn find_level(&self, id: u64) -> Option<(&str, &HorizontalLevel)>;

    /// Mutable lookup by ID within a known ticker. O(n) in that
    /// ticker's levels, typically < 20.
    pub fn find_level_mut(&mut self, ticker: &str, id: u64)
        -> Option<&mut HorizontalLevel>;

    // ── Mutations ────────────────────────────────────────────────

    /// Allocates a globally unique level ID.
    pub fn alloc_id(&mut self) -> u64;

    /// Adds a level to a ticker's list.
    pub fn add_level(&mut self, ticker: &str, level: HorizontalLevel);

    /// Removes a level by ID from a ticker's list. Returns the removed
    /// level, or None if not found.
    pub fn remove_level(&mut self, ticker: &str, id: u64)
        -> Option<HorizontalLevel>;

    /// Removes all levels for a ticker.
    pub fn clear_levels(&mut self, ticker: &str);

    // ── Persistence ──────────────────────────────────────────────

    /// Reconstructs a `LevelStore` from persisted config.
    pub fn from_config(levels: &HashMap<String, Vec<LevelConfig>>) -> Self;

    /// Serializes to a config-ready map.
    pub fn to_config(&self) -> HashMap<String, Vec<LevelConfig>>;
}
```

### 1.2 ChartState Changes (midas-chart)

```rust
// REMOVED from ChartState:
//   levels: Vec<HorizontalLevel>    ← moves to LevelStore
//   next_level_id: u64              ← moves to LevelStore
//   alloc_level_id()                ← moves to LevelStore

// STAYS on ChartState (view-specific):
pub selected_level: Option<u64>,     // which level is selected on THIS chart
pub level_tool: LevelTool,           // placement/drag state for THIS chart
pub show_levels: bool,               // visibility toggle for THIS chart
```

### 1.3 Config Schema Changes (midas-core)

```rust
/// Root application configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub window: WindowConfig,
    pub theme: ThemeConfig,
    #[serde(default)]
    pub charts: Vec<ChartConfig>,
    /// Per-ticker horizontal levels, keyed by symbol.
    /// Example: `[levels.AAPL]`, `[levels.MSFT]`.
    #[serde(default)]
    pub levels: HashMap<String, Vec<LevelConfig>>,
}

/// Per-chart configuration — levels REMOVED.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChartConfig {
    pub symbol: String,
    pub timeframe: String,
    // levels: Vec<LevelConfig>,     ← REMOVED
    pub camera_time_start: Option<f64>,
    pub camera_time_end: Option<f64>,
    // ... all other fields unchanged
    pub show_levels: bool,            // stays per-chart
}
```

TOML output example:

```toml
[window]
width = 1920
height = 1080

[theme]
mode = "dark"

[[charts]]
symbol = "AAPL"
timeframe = "1D"
show_levels = true
# ... camera, collapse_gaps, etc.

[[charts]]
symbol = "AAPL"
timeframe = "5m"
show_levels = true

[[levels.AAPL]]
price = 185.50
color = [0.0, 0.8, 0.0, 1.0]
line_width = 1.0
label = "Support"
icon = "arrow_up"
locked = false

[[levels.AAPL]]
price = 192.30
color = [0.85, 0.85, 0.85, 0.8]
line_width = 1.0
icon = "none"
locked = false

[[levels.MSFT]]
price = 420.00
color = [0.8, 0.0, 0.0, 1.0]
line_width = 1.5
label = "Resistance"
icon = "warning"
locked = true
```

---

## 2. Data Flow Changes

### 2.1 Creation (before → after)

**Before:**
```
click → ChartAction::CreateLevel { price }
  → widget sends Message::ChartCreateLevel(chart_id, price)
  → app: chart.chart_state.alloc_level_id()
  → app: chart.chart_state.levels.push(HorizontalLevel { ... })
  (Note: ChartState::apply_action also has a CreateLevel arm with its
   own alloc_level_id() — redundant code path used only in tests)
```

**After:**
```
click → ChartAction::CreateLevel { price }
  → Message::ChartCreateLevel(chart_id, price)
  → app: level_store.alloc_id()
  → app: level_store.add_level(ticker, HorizontalLevel { ... })
  → app: mark_config_dirty()
  → all charts showing ticker see new level on next frame
```

Key change: `ChartState::apply_action()` no longer creates the level
locally. It emits the action, and the app layer performs the mutation
on the shared store. This consolidates the two redundant code paths
into one.

### 2.2 Drag (before → after)

**Before:**
```
drag → ChartAction::DragLevel { id, new_price }
  → ChartState.levels[id].price = new_price   ← local only
  → Message::ChartDragLevel(chart_id, id, new_price)
  → app: chart.chart_state.levels[id].price = new_price  ← redundant
```

**After:**
```
drag → ChartAction::DragLevel { id, new_price }
  → Message::ChartDragLevel(chart_id, id, new_price)
  → app: level_store.find_level_mut(ticker, id).price = new_price
  → all charts showing ticker see updated price on next frame
```

### 2.3 Compute/Render (before → after)

**Before:**
```
compute_chart_scene(input)
  → input.levels = &chart_state.levels    ← per-chart data
  → compute_levels(input.levels, camera) → Vec<LevelRender>
```

**After:**
```
compute_chart_scene(input)
  → input.levels = level_store.levels_for(ticker)   ← shared data
  → compute_levels(input.levels, camera) → Vec<LevelRender>
```

The compute pipeline is unchanged — `compute_levels()` still receives
`&[HorizontalLevel]`. Only the source of that slice changes.

### 2.4 Widget Sync (before → after)

**Before:**
```
ChartRenderSnapshot {
    levels: chart_state.levels.clone(),   ← per-chart copy
    ...
}
// Widget sync: chart_state.levels = snapshot.levels.clone()
```

**After:**
```
ChartRenderSnapshot {
    levels: level_store.levels_for(ticker).to_vec(),  ← shared copy
    ...
}
// Widget sync: unchanged (snapshot still carries the data)
```

The snapshot clone is unchanged in shape. The source just changes from
`chart_state.levels` to `level_store.levels_for(ticker)`.

### 2.5 Persistence (before → after)

**Before:**
```
build_config():
  for chart in charts:
    chart_cfg.levels = chart.chart_state.levels → Vec<LevelConfig>

restore:
  for chart_cfg in config.charts:
    restore_levels(&chart_cfg.levels, &mut panel)
```

**After:**
```
build_config():
  app_cfg.levels = level_store.to_config()    ← HashMap<String, Vec<LevelConfig>>
  // chart_cfg no longer has levels

restore:
  level_store = LevelStore::from_config(&config.levels)
  // backward compat: if config.levels is empty, migrate from chart configs
```

---

## 3. Dirty Flag Strategy

### Problem

Previously, each chart tracked its own `dirty.levels` generation counter.
When a chart's levels changed, only that chart recomputed. Now, a single
level edit must mark all charts showing that ticker as dirty.

### Solution

`LevelStore` maintains a per-ticker generation counter:

```rust
pub struct LevelStore {
    levels: HashMap<String, Vec<HorizontalLevel>>,
    /// Per-ticker generation counter. Incremented on any mutation.
    generations: HashMap<String, u64>,
    next_id: u64,
}

impl LevelStore {
    /// Returns the current generation for a ticker.
    pub fn generation(&self, ticker: &str) -> u64;
}
```

Each chart caches the last generation it computed against:

```rust
// In ChartState or wherever dirty flags live:
pub last_level_generation: u64,
```

During compute:
```rust
let current_gen = level_store.generation(ticker);
if current_gen != chart_state.last_level_generation {
    // recompute levels
    chart_state.last_level_generation = current_gen;
}
```

This is O(1) per chart per frame — no full Vec comparison.

### Bridging with Existing DirtyFlags

The existing GPU renderer depends on `DirtyFlags.levels` (per-chart
generation counter) via `DirtyTracker.needs_level_rebuild()`. This
mechanism must continue to work after the refactor.

**Rule:** After every `LevelStore` mutation, the app layer must call
`chart_state.dirty.mark_levels()` on **every chart** displaying the
affected ticker (both in `self.charts` and `self.floating_charts`).

```rust
// Helper added in Phase 3:
fn mark_levels_dirty_for_ticker(&mut self, ticker: &str) {
    for chart in self.charts.values_mut() {
        if chart.symbol == ticker {
            chart.chart_state.dirty.mark_levels();
        }
    }
    for chart in self.floating_charts.values_mut() {
        if chart.symbol == ticker {
            chart.chart_state.dirty.mark_levels();
        }
    }
}
```

The `LevelStore.generations` counter is the **source** of truth for
"did this ticker's levels change?" The per-chart `DirtyFlags.levels`
is the **signal** to the GPU renderer to rebuild level primitives.
The app layer bridges the two: store mutation → bump generation →
mark dirty on all affected charts.

---

## 4. Interaction Layer Changes

### 4.1 What Stays in midas-chart

The interaction layer (`handle_event()`) continues to:
- Manage `LevelTool` state machine (placing, dragging, idle)
- Perform hit-testing against a `&[HorizontalLevel]` slice
- Compute OHLC snap prices
- Emit `ChartAction::CreateLevel`, `DragLevel`, `SelectLevel`, etc.

The interaction layer does NOT need to know about `LevelStore`. It
receives `&[HorizontalLevel]` (from the store, via the snapshot) and
emits actions. The app layer routes those actions to the store.

### 4.2 What Changes in midas-chart

`ChartState::apply_action()` no longer handles `CreateLevel` or
`DragLevel` mutations on a local `levels` vec, because there is no
local vec. These actions pass through as messages to the app layer.

Actions that remain local to `ChartState`:
- `SelectLevel { id }` → sets `self.selected_level`
- `DeselectLevel` → clears `self.selected_level`
- `CancelPlacing` → calls `self.level_tool.cancel()`

Actions that become app-layer-only:
- `CreateLevel { price }` → app: `level_store.add_level()`
- `DragLevel { id, new_price }` → app: `level_store.find_level_mut()`
- `DeleteSelectedLevel` → app: `level_store.remove_level()`

### 4.3 hit_test_levels Signature

No change:
```rust
fn hit_test_levels(
    levels: &[HorizontalLevel],
    cursor_y: f32,
    camera: &Camera2D,
) -> Option<(u64, f64)>
```

The caller passes `level_store.levels_for(ticker)` instead of
`chart_state.levels`. The function itself is unchanged.

---

## 5. Message Routing Changes (midas-app)

### 5.1 Affected Messages

Every message that currently takes `ChartId` and modifies
`chart.chart_state.levels` must instead route through `level_store`:

| Message | Before | After |
|---|---|---|
| `ChartCreateLevel(ChartId, f64)` | `chart.chart_state.levels.push(...)` | `level_store.add_level(ticker, ...)` |
| `ChartDragLevel(ChartId, u64, f64)` | `chart.chart_state.levels[id].price = p` | `level_store.find_level_mut(ticker, id).price = p` |
| `ChartDeleteSelectedLevel(ChartId)` | `chart.chart_state.levels.retain(...)` | `level_store.remove_level(ticker, id)` |
| `ChartDeleteLevel(ChartId, u64)` | `chart.chart_state.levels.retain(...)` | `level_store.remove_level(ticker, id)` |
| `ChartClearAllLevels(ChartId)` | `chart.chart_state.levels.clear()` | `level_store.clear_levels(ticker)` (clears for ALL charts of that ticker — see Section 5.3) |
| `LevelEditorPriceChanged(...)` | `chart.chart_state.levels[id].price = p` | `level_store.find_level_mut(ticker, id).price = p` |
| `LevelEditorPriceStep(...)` | `chart.chart_state.levels[id].price += d` | `level_store.find_level_mut(ticker, id).price += d` |
| `LevelEditorLabelChanged(...)` | `chart.chart_state.levels[id].label = s` | `level_store.find_level_mut(ticker, id).label = s` |
| `LevelEditorColorChanged(...)` | `chart.chart_state.levels[id].color = c` | `level_store.find_level_mut(ticker, id).color = c` |
| `LevelEditorThicknessChanged(...)` | `chart.chart_state.levels[id].line_width` | `level_store.find_level_mut(ticker, id).line_width` |
| `LevelEditorIconChanged(...)` | `chart.chart_state.levels[id].icon = i` | `level_store.find_level_mut(ticker, id).icon = i` |
| `LevelEditorToggleLock(...)` | `chart.chart_state.levels[id].locked ^= 1` | `level_store.find_level_mut(ticker, id).locked ^= 1` |

All mutations go through `level_store`, which bumps the generation
counter for the affected ticker, triggering dirty recompute on all
charts showing it.

### 5.2 Helper: Resolving Ticker from ChartId

Every message handler needs `ticker` to address the store. Add a helper:

```rust
impl MidasApp {
    fn chart_ticker(&self, id: ChartId) -> Option<&str> {
        // First check the pane_grid charts (normal case).
        if let Some(chart) = self.charts.get(&id) {
            return Some(chart.symbol.as_str());
        }
        // Floating windows use ChartId::new(0) as a sentinel — they
        // don't exist in self.charts. Fall back to searching floating
        // charts by their stored symbol.
        // NOTE: If multiple floating windows exist for different tickers,
        // this returns the first match. In practice only one floating
        // chart is active at a time, so this is sufficient.
        if id == ChartId::new(0) {
            // The message context (e.g. editing_level_id) disambiguates
            // which floating chart. For ticker resolution, any floating
            // chart with that level ID is correct — levels are per-ticker,
            // so the ticker is unambiguous given a level ID.
            for chart in self.floating_charts.values() {
                return Some(chart.symbol.as_str());
            }
        }
        None
    }
}
```

**Note on floating chart IDs:** Floating windows use `ChartId::new(0)`
(see `views.rs` line 64) because they don't participate in the pane_grid
chart map. This sentinel means `chart_ticker()` must search
`self.floating_charts` as a fallback. If multiple floating windows for
different tickers are ever supported, the floating chart message routing
will need a `window::Id` → ticker lookup instead of `ChartId`.

### 5.3 ClearAllLevels Semantics

`ClearAllLevels` clears all levels for the ticker in the shared store.
This is consistent with the principle that levels are shared data: if a
user says "clear all levels" while viewing AAPL, they mean "remove all
AAPL levels," not "remove only the ones I can see on this specific chart
panel." The action is destructive but reversible (user can re-create
levels). The existing lock guard still prevents clearing locked levels
if that check is desired — but currently the handler clears all
unconditionally, and that behavior is preserved.

### 5.4 Floating Chart Windows

`MidasApp.floating_charts: HashMap<window::Id, ChartPanel>` contains
popped-out chart panels. These must participate in the shared store:

- **Snapshot construction** (`views.rs` line 48): Currently reads
  `chart.chart_state.levels.clone()`. After Phase 3, reads from
  `level_store.levels_for(&chart.symbol).to_vec()`.
- **Message routing**: Level messages from floating windows use
  `ChartId::new(0)` (sentinel). The `chart_ticker()` helper (Section 5.2)
  falls back to searching `self.floating_charts` for this sentinel ID.
- **Dirty marking**: `mark_levels_dirty_for_ticker()` (Section 3) must
  iterate both `self.charts` and `self.floating_charts`.

---

## 6. Backward-Compatible Config Migration

### Problem

Existing users have levels inside `[[charts]].levels`. After the change,
levels live in `[levels.SYMBOL]`. We need a seamless one-time migration.

### Strategy

On `AppConfig::load()`:
1. Deserialize normally. Both `charts[].levels` and top-level `levels`
   are `#[serde(default)]`, so old configs (without `[levels]`) load fine.
2. If `config.levels` is empty but any `chart.levels` is non-empty,
   run migration:
   - For each chart config with non-empty levels, merge them into
     `config.levels[chart.symbol]`, deduplicating by price (within a
     tolerance of 0.0001 to handle float comparison).
   - Clear `chart.levels` on the chart configs.
3. On next save, the new format is written. Old `[[charts]].levels`
   fields are no longer serialized (field removed from `ChartConfig`).

### Deduplication Logic

When two AAPL charts have levels at the same price (within 0.0001
tolerance for float comparison), keep the first encountered and skip
subsequent duplicates. This is simple and sufficient — users rarely
have conflicting customizations at the exact same price.

```rust
fn migrate_levels(config: &mut AppConfig) {
    if !config.levels.is_empty() {
        return; // already migrated or new config
    }
    // Collect chart data first to avoid borrow conflict
    // (&config.charts immutable vs &mut config.levels).
    let chart_data: Vec<_> = config.charts.iter()
        .filter(|c| !c.levels.is_empty())
        .map(|c| (c.symbol.clone(), c.levels.clone()))
        .collect();
    for (symbol, levels) in chart_data {
        let ticker_levels = config.levels
            .entry(symbol)
            .or_default();
        for level in &levels {
            let is_dup = ticker_levels.iter().any(|existing|
                (existing.price - level.price).abs() < 0.0001
            );
            if !is_dup {
                ticker_levels.push(level.clone());
            }
        }
    }
}
```

### Backward-Compat Field on ChartConfig

During migration, `ChartConfig` temporarily retains a `levels` field
with `#[serde(default, skip_serializing)]` — it can still be read from
old configs but is never written back.

```rust
pub struct ChartConfig {
    // ...
    /// DEPRECATED: Levels migrated to top-level `[levels]` table.
    /// Retained for one-time migration from old config format.
    #[serde(default, skip_serializing)]
    pub levels: Vec<LevelConfig>,
    // ...
}
```

---

## 7. Phased Implementation

Each phase is independently shippable and verifiable.

---

### Phase 1: LevelStore Type and Unit Tests

**Goal:** Create the `LevelStore` struct with full CRUD API and
generation tracking. Verify with unit tests. No integration yet.

**Scope:**
- New file: `crates/midas-app/src/level_store.rs`
- `LevelStore` struct: `HashMap<String, Vec<HorizontalLevel>>`,
  `HashMap<String, u64>` generations, `next_id: u64`
- All methods from Section 1.1
- `from_config()` and `to_config()` conversion
- Unit tests: insert, remove, clear, find, alloc_id uniqueness,
  generation bumping, config round-trip

**Verification:**
- `cargo test -p midas-app` — new tests pass
- `cargo clippy --workspace` — clean
- Existing tests unaffected (LevelStore not wired in yet)

**Estimated:** 1 new file (~250 lines), 0 modified

---

### Phase 2: Config Schema Migration

**Goal:** Move levels from `ChartConfig.levels` to `AppConfig.levels`
(per-ticker map). Existing configs migrate transparently.

**Scope:**
- Modify `AppConfig`: add `levels: HashMap<String, Vec<LevelConfig>>`
- Modify `ChartConfig`: mark `levels` as `skip_serializing` (read-only
  for migration)
- Implement `migrate_levels()` in config.rs (Section 6)
- Call migration in `AppConfig::load()` after deserialization
- Update `AppConfig` unit tests for both old and new format

**Verification:**
- Old config.toml loads correctly, levels migrated to new location
- New config.toml round-trips cleanly (levels under `[levels.SYMBOL]`)
- `ChartConfig.levels` is never serialized
- `cargo test -p midas-core` — all pass including new migration tests

**Estimated:** 0 new files, 1 modified (config.rs ~80 lines changed)

---

### Phase 3: Wire LevelStore into MidasApp

**Goal:** Replace per-chart level storage with the centralized
`LevelStore`. All existing functionality preserved.

**Scope:**
- Add `level_store: LevelStore` field to `MidasApp`
- Initialize from `AppConfig.levels` in `MidasApp::new()`
- Remove `restore_levels()` per-chart function
- Update `build_config()` to serialize from `level_store.to_config()`
- Update all message handlers (Section 5.1 table) to route through
  `level_store` instead of `chart.chart_state.levels`
- Add `chart_ticker()` helper
- Add `mark_levels_dirty_for_ticker()` helper (Section 3) — call it
  after every `level_store` mutation to bridge with existing
  `DirtyFlags.levels` renderer mechanism
- Update `ChartRenderSnapshot` construction to read from `level_store`
  instead of `chart_state` — both in `views.rs` line 485 (main panes)
  and `views.rs` line 48 (floating chart windows, see Section 5.4)
- Route floating chart level messages through `level_store` (Section 5.4)
- **Update app-layer level read sites.** Since `chart.chart_state.levels`
  is no longer populated (mutations go to `level_store`, `restore_levels()`
  is removed), any app-layer code that reads levels must switch to the
  store. Affected sites:
  - `ChartRightClickLevel` handler (`app.rs` ~line 983): reads
    `chart.chart_state.levels.iter().find()` to initialize the editor
    popup price input → change to `level_store.find_level_mut(ticker, id)`
  - Level editor popup lookups in `views.rs` (~lines 112, 544): reads
    `chart.chart_state.levels` to render the popup → pass levels from
    `level_store.levels_for(ticker)` instead
  - `compute_level_renders` in `views.rs` (~line 1278): reads
    `chart.chart_state.levels.iter()` to build iced-layer level label
    overlays. Called from floating windows (~line 100) and main panes
    (~line 532). Change signature to accept `&[HorizontalLevel]` from
    `level_store.levels_for(ticker)` instead of reading from `ChartPanel`

**Transitional state note:** During this phase, `ChartState.levels`
still exists and is populated via the snapshot sync in
`chart_widget.rs`. The widget interaction layer reads from this local
copy for hit-testing and drag. Do NOT remove `ChartState.levels` yet
— that cleanup happens in Phase 4.

**Verification:**
- App starts, levels loaded from config appear on correct charts
- Create level on AAPL chart → appears on all AAPL charts
- Drag level on one chart → updates on all charts showing that ticker
- Delete, lock, color change, label edit — all propagate
- Level editor popup works as before
- Floating (pop-out) chart windows show shared levels correctly
- Close and reopen app — levels persist correctly in new format
- `cargo test --workspace` — all existing tests pass
- `cargo clippy --workspace` — clean

**Estimated:** 0 new files, 5 modified (app.rs ~180 lines, persistence.rs
~30 lines, chart_widget.rs ~20 lines, views.rs ~10 lines, app/mod.rs
~5 lines)

---

### Phase 4: Remove Levels from ChartState

**Goal:** Clean up `ChartState` by removing the now-unused `levels` vec
and `next_level_id` counter. Update midas-chart's internal API.

**Scope:**
- Remove `levels: Vec<HorizontalLevel>` from `ChartState`
- Remove `next_level_id: u64` from `ChartState`
- Remove `alloc_level_id()` method
- Update `ChartState::apply_action()`: remove local-mutation arms for
  `CreateLevel`, `DragLevel`, `DeleteSelectedLevel` (these are now
  handled exclusively at the app layer)
- Update `compute_chart_scene()` input: levels now come from an external
  `&[HorizontalLevel]` parameter, not from `ChartState`
- Update `ChartInput` (or equivalent) to accept an external levels slice
- Add `last_level_generation: u64` to dirty tracking
- Update widget sync: remove `chart_state.levels = snapshot.levels.clone()`
  guard (no local levels to sync)
- **Replace drag visual feedback path.** Currently `chart_widget.rs`
  applies `DragLevel` to `chart_state.levels` locally (line 266-269) for
  immediate visual feedback, and `draw()` reads from `cs.levels` during
  drag (line 399-405). With `chart_state.levels` removed, add
  `drag_price_override: Option<(u64, f64)>` to `ChartWidgetState`.
  During drag, apply this override to the snapshot levels copy when
  building `ChartInput`. Clear the override on drag end or next snapshot
  sync when not dragging. This preserves zero-latency drag feedback
  without requiring a local levels vec.
- Update midas-chart tests that construct `ChartState` with levels:
  - `state.rs` tests (`apply_create_level`, `apply_select_and_delete_level`,
    `apply_drag_level`): These test `apply_action(CreateLevel)` etc. which
    are being removed. Delete these tests — equivalent coverage now lives
    in `LevelStore` unit tests (Phase 1). Keep tests for `SelectLevel`,
    `DeselectLevel`, `CancelPlacing` which remain on `ChartState`.
  - `interaction.rs` tests: Most already assert on returned `ChartAction`
    variants without applying them. Tests that call `apply_action` after
    `handle_event` should instead verify the correct action is returned.
  - `compute.rs` tests: Already pass `levels: &[HorizontalLevel]` via
    `make_input` — no change needed.

**Verification:**
- Compilation succeeds with no warnings
- All midas-chart unit tests updated and passing
- All midas-app integration behavior unchanged
- Level tool (place, drag, snap) works correctly
- `cargo test --workspace` — all pass
- `cargo clippy --workspace` — clean

**Estimated:** 0 new files, 6 modified (state.rs, interaction.rs,
compute.rs, chart_widget.rs, input.rs or equivalent, test files ~30
lines of test changes)

---

### Phase 5: Polish and Edge Cases

**Goal:** Handle edge cases, verify cross-chart scenarios, clean up any
remaining per-chart level artifacts.

**Scope:**
- Verify: two charts showing AAPL, different timeframes → same levels
- Verify: chart changes symbol from AAPL to MSFT → levels update
- Verify: last chart for a ticker is closed → levels persist in store
  (not garbage-collected)
- Verify: new chart opened for ticker with existing levels → levels
  appear immediately
- Verify: `ClearAllLevels` on one chart clears for all charts of that
  ticker (per Section 5.3 — this is the decided behavior)
- Remove any dead code, unused imports, stale comments
- Update `level-tool-refactor.md` non-goals to note this is now done
- Final manual regression test (Section 8)

**Verification:**
- Full manual regression per Section 8
- `cargo test --workspace` — all pass
- `cargo clippy --workspace -- -D warnings` — zero warnings
- `cargo fmt --all --check` — clean

**Estimated:** 0 new files, 2-3 modified (cleanup only)

---

## 8. Manual Regression Checklist

- [ ] Open two charts for the same ticker (e.g., AAPL 1D and AAPL 5m)
- [ ] Press H on chart 1, place a level → appears on both charts
- [ ] Drag the level on chart 2 → updates on chart 1 in real time
- [ ] Right-click level on chart 1, change color → chart 2 updates
- [ ] Right-click level, change label → chart 2 shows new label
- [ ] Right-click level, toggle lock → drag is blocked on both charts
- [ ] Select level on chart 1, press Delete → removed from both charts
- [ ] Create multiple levels, "Clear All" → clears on both charts
- [ ] Open a third chart for a different ticker (MSFT) → no AAPL levels
- [ ] Create MSFT level → does not appear on AAPL charts
- [ ] Toggle level visibility on chart 1 → chart 2 unaffected
- [ ] Close all AAPL charts, reopen one → levels still present
- [ ] Close and reopen the app entirely → levels persist
- [ ] Alt-held during placement disables OHLC snap
- [ ] Double-click to create level still works
- [ ] Level editor price stepping (arrows) updates across charts
- [ ] Escape cancels placement mode cleanly
- [ ] Config.toml shows levels under `[levels.AAPL]`, not under `[[charts]]`
- [ ] Load an old config.toml (with per-chart levels) → migration works

---

## 9. Dependency Graph

```
Phase 1: LevelStore type + tests
    │
    ├─→ Phase 2: Config schema migration
    │       │
    │       └─→ Phase 3: Wire into MidasApp
    │               │
    │               └─→ Phase 4: Remove from ChartState
    │                       │
    │                       └─→ Phase 5: Polish + regression
    │
    └─→ (all phases are sequential — each depends on the previous)
```

Phases are strictly sequential. No parallelism — each phase modifies
the same files and depends on the previous phase's output.

---

## 10. Architectural Decision Table

| Decision | Choice | Rationale |
|---|---|---|
| Store key | `String` (ticker symbol) | Simple, matches existing `ChartPanel.symbol` |
| Store location | `MidasApp` field | App layer owns all shared state; no new crate needed |
| ID allocation | Global monotonic `u64` | Eliminates duplicate-allocation bug, IDs unique across tickers |
| Dirty tracking | Per-ticker generation counter | O(1) check per chart per frame, no Vec diff |
| show_levels scope | Per-chart (stays on ChartState) | Visibility is a view preference, not shared data |
| selected_level scope | Per-chart (stays on ChartState) | Selection is cursor-local, not shared |
| LevelTool scope | Per-chart (stays on ChartState) | Drag/placement is per-interaction, not shared |
| Config format | `[levels.SYMBOL]` table in TOML | Groups by ticker naturally, HashMap serialization |
| Migration | One-time on load, deduplicate by price | Seamless upgrade, no user action required |
| Per-timeframe scoping | Deferred (not in v1) | Per-ticker covers the common case; extend later if needed |

---

## 11. Future Compatibility

### Annotations Plan Integration

The annotations plan (Phase 1) migrates `HorizontalLevel` into
`AnnotationStore`. With this per-ticker refactor in place, that
migration becomes simpler:

- `LevelStore` already groups by ticker → `AnnotationStore` adopts
  the same per-ticker keying
- Global IDs are already centralized → `AnnotationStore` inherits
  the counter from `LevelStore`
- The persistence format (`[levels.SYMBOL]`) maps cleanly to
  annotation JSON files keyed by symbol

### Potential Extensions

- **Timeframe-specific visibility:** Add `visible_timeframes: Option<Vec<String>>`
  to `HorizontalLevel`. If `None`, visible on all timeframes. If `Some`,
  only on listed timeframes. No store-level change needed.
- **Level grouping/tagging:** Add `tags: Vec<String>` to `HorizontalLevel`
  for filtering ("support", "resistance", "earnings", etc.).
- **Cross-ticker levels:** For index/component correlation, a level could
  reference multiple tickers. Out of scope but the store design doesn't
  prevent it.
