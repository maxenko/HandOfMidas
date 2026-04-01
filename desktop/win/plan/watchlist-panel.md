# Feature: Ticker Watchlist Panel

## Overview

A new panel type — a persistent, editable grid of tickers — that lives alongside chart panels in the workspace's pane grid. Users see columns for Favorite, Ticker, Price, Change%, and G.ATR. The watchlist persists across sessions. Users can drag a ticker row from the watchlist and drop it onto any chart pane to load that symbol.

This is the first non-chart panel type, so it also introduces the `PanelContent` abstraction that allows the pane grid to host heterogeneous panel types.

**Assumptions**:
- Price, Change%, and G.ATR columns exist in the data model but are populated from available test data when a symbol has been loaded in any chart. Blank otherwise (no live feed yet).
- A single default watchlist for now (multi-watchlist support is out of scope).
- Drag-drop is from watchlist → chart only (not chart → watchlist, not reorder within watchlist).

## Codebase Analysis

**Tech stack**: Rust, iced 0.14, wgpu 27, tokio, serde, TOML config.

**Architecture pattern**: Elm (Message → update → view), sans-IO chart core (`midas-chart`), binary split-tree pane layout via iced's `pane_grid`.

**Key files the feature will touch or extend**:

| File | Role |
|---|---|
| `crates/midas-core/src/id.rs` | Add `WatchlistId` newtype |
| `crates/midas-core/src/config.rs` | Add `WatchlistConfig` to `AppConfig` |
| `crates/midas-core/src/lib.rs` | Re-export `WatchlistId` |
| `crates/midas-app/src/layout.rs` | Refactor `PaneState` to `PanelContent` enum |
| `crates/midas-app/src/app.rs` | Add watchlist state, new `Message` variants, update handlers |
| `crates/midas-app/src/app/views.rs` | Add watchlist panel view, toolbar button, drag overlay |
| `crates/midas-app/src/app/persistence.rs` | Save/restore watchlists in config |
| `crates/midas-app/src/main.rs` | Register `watchlist` module |
| `crates/midas-app/src/watchlist.rs` (new) | `WatchlistPanel`, `WatchlistTicker`, persistence |

**Existing patterns the feature must follow**:
- ID newtypes: `ChartId(u32)` pattern in `id.rs` — derive Copy, Clone, Hash, Eq, Serialize, Deserialize, Display
- Config: TOML via serde with `#[serde(default)]` for backward compat, atomic writes via tempfile
- State ownership: `MidasApp` owns `HashMap<K, Panel>`, pane grid maps pane → ID
- Message flow: widget event → `Message` variant → `update()` → state mutation → `mark_config_dirty()`
- View dispatch: `view(window_id)` checks floating charts first, then builds main window

**Blast radius**:
- `layout.rs`: `PaneState.chart_id` → `PanelContent` enum — breaks every callsite that reads `pane_state.chart_id`
- `app/views.rs`: pane grid body must dispatch on panel type
- `app.rs` update: every `Message` handler that accesses a chart via pane's `chart_id` needs updating
- `app/persistence.rs`: config builder must handle both panel types
- All existing layout tests in `layout.rs` need updating for the new `PaneState` shape

## Design Decisions

### Decision: Panel type representation in PaneState

**Context**: Currently `PaneState` stores `chart_id: ChartId`. We need it to also support watchlist panels. This is the single most impactful refactor.

**Options considered**:
1. **Enum in PaneState** — `PanelContent::Chart(ChartId) | PanelContent::Watchlist(WatchlistId)`. Clean, exhaustive match forces handling both types everywhere. Moderate refactor (every `.chart_id` access becomes a match).
2. **Separate pane grids** — One pane grid for charts, another for watchlists. Avoids refactoring PaneState but prevents mixing panel types in the same layout and splits the layout logic.
3. **Trait object** — `Box<dyn Panel>` in PaneState. Over-engineered for 2 variants, loses exhaustive matching.

**Recommendation**: Option 1 (enum). Follows the project's existing preference for enum dispatch over trait objects (per widget system docs). The compiler will catch every callsite that needs updating.

**Confidence**: high

### Decision: Watchlist data persistence

**Context**: The watchlist must survive across sessions. Need to choose storage format and location.

**Options considered**:
1. **Extend AppConfig (TOML)** — Add `[[watchlists]]` table to config.toml alongside `[[charts]]`. Simple, consistent with existing chart config persistence.
2. **Separate JSON files** — Following the annotation_persistence.rs pattern. Provides schema versioning and atomic writes.

**Recommendation**: Option 1 (TOML config). The watchlist is small metadata (list of ticker strings + favorite booleans), not large data. It fits naturally alongside `[[charts]]` in config.toml. This avoids a new persistence subsystem. Annotations use separate JSON because they're per-symbol and potentially large; watchlists are per-workspace and small.

**Confidence**: high

### Decision: Drag-and-drop implementation

**Context**: iced 0.14 has no built-in cross-widget drag-and-drop. Need to implement app-level drag state tracking.

**Options considered**:
1. **App-level drag state** — `MidasApp` tracks `dragging_ticker: Option<DragState>`. Mouse-down on ticker row sets it, mouse-up clears it. During drag, a floating label follows the cursor. On release over a chart pane, emit `LoadSymbolInChart`.
2. **Clipboard/double-click** — Simpler: double-click loads in focused chart. No drag visual needed. But user explicitly requested drag-drop.

**Recommendation**: Option 1 (app-level drag state). The user specifically requested drag-drop. Track drag state centrally in `MidasApp`. Use iced's mouse subscription to detect release position, then hit-test against pane regions.

**Confidence**: medium — iced's pane_grid doesn't expose per-pane screen bounds natively. We may need to track pane bounds during view layout or use a simpler heuristic (drop on focused chart if cursor isn't tracked per-pane). Fallback: release over any chart loads it; if over the pane grid area, use pane-under-cursor from iced's internal focus tracking.

### Decision: Price/Change%/G.ATR data source

**Context**: These columns need values, but there's no live data feed yet. Only `TestDataProvider` exists.

**Options considered**:
1. **Populate from loaded chart data** — If any chart has the same symbol loaded, use its last close price and compute change%. G.ATR computed from candle data. Blank for symbols not loaded in any chart.
2. **Static placeholder** — Show `--` for all price columns until live feed exists.

**Recommendation**: Option 1, with graceful fallback to `--`. When a symbol is loaded in any chart, we can grab the last close from `CandleBuffer`. This gives real feedback with test data. G.ATR can be computed from the candle buffer's ATR. The watchlist doesn't own or load data — it reads from whatever charts have loaded.

**Confidence**: high

## Implementation Plan

### Slice 1: Data Model + WatchlistId

**Goal**: Define the watchlist data types and persistence config structs.
**Depends on**: None

**Files to create or modify**:
- `crates/midas-core/src/id.rs` — Add `WatchlistId(u32)` following exact `ChartId` pattern
- `crates/midas-core/src/lib.rs` — Re-export `WatchlistId`
- `crates/midas-core/src/config.rs` — Add `WatchlistConfig` and `WatchlistTickerConfig` structs, add `watchlists: Vec<WatchlistConfig>` to `AppConfig`
- `crates/midas-app/src/watchlist.rs` (new) — `WatchlistPanel` (runtime state) and `WatchlistTicker` (per-row data)

**Key implementation details**:

```rust
// id.rs — follow ChartId pattern exactly
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, Ord, PartialOrd,
         serde::Serialize, serde::Deserialize)]
pub struct WatchlistId(pub u32);

impl WatchlistId {
    pub const fn new(id: u32) -> Self { Self(id) }
}
impl fmt::Display for WatchlistId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Watchlist({})", self.0)
    }
}
```

```rust
// config.rs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchlistConfig {
    pub name: String,
    #[serde(default)]
    pub tickers: Vec<WatchlistTickerConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchlistTickerConfig {
    pub symbol: String,
    #[serde(default)]
    pub favorite: bool,
}

// In AppConfig:
#[serde(default)]
pub watchlists: Vec<WatchlistConfig>,
```

```rust
// watchlist.rs
pub struct WatchlistPanel {
    pub id: WatchlistId,
    pub name: String,
    pub tickers: Vec<WatchlistTicker>,
    /// Text in the "add ticker" input field.
    pub add_ticker_input: String,
}

pub struct WatchlistTicker {
    pub symbol: String,
    pub favorite: bool,
}

impl WatchlistPanel {
    pub fn new(id: WatchlistId, name: String) -> Self { ... }
    pub fn from_config(id: WatchlistId, config: &WatchlistConfig) -> Self { ... }
    pub fn to_config(&self) -> WatchlistConfig { ... }
    pub fn add_ticker(&mut self, symbol: &str) { ... }
    pub fn remove_ticker(&mut self, symbol: &str) { ... }
    pub fn toggle_favorite(&mut self, symbol: &str) { ... }
    pub fn has_ticker(&self, symbol: &str) -> bool { ... }
}
```

**Testing**:
- Unit tests in `id.rs`: equality, hashing, display, serde roundtrip (follow existing pattern)
- Unit tests in `watchlist.rs`: add/remove/toggle/duplicate prevention, `from_config`/`to_config` roundtrip
- Unit test in `config.rs`: TOML roundtrip with watchlists field present and absent

**Done when**: `WatchlistId` compiles, `WatchlistPanel` CRUD works, config round-trips with watchlist data. All existing tests pass.

---

### Slice 2: PanelContent Abstraction (Critical Refactor)

**Goal**: Replace `PaneState.chart_id: ChartId` with a `PanelContent` enum so panes can hold either charts or watchlists.
**Depends on**: Slice 1

**Files to create or modify**:
- `crates/midas-app/src/layout.rs` — Introduce `PanelContent` enum, update `PaneState`, update `WorkspaceLayout` methods
- `crates/midas-app/src/app.rs` — Update all `Message` handlers that read `pane_state.chart_id`
- `crates/midas-app/src/app/views.rs` — Update pane grid body dispatch
- `crates/midas-app/src/app/persistence.rs` — Update config builder

**Key implementation details**:

```rust
// layout.rs
#[derive(Debug, Clone)]
pub enum PanelContent {
    Chart(ChartId),
    Watchlist(WatchlistId),
}

#[derive(Debug, Clone)]
pub struct PaneState {
    pub content: PanelContent,
    pub is_focused: bool,
}

impl PaneState {
    pub fn chart(chart_id: ChartId) -> Self {
        Self { content: PanelContent::Chart(chart_id), is_focused: false }
    }
    pub fn watchlist(id: WatchlistId) -> Self {
        Self { content: PanelContent::Watchlist(id), is_focused: false }
    }
    /// Convenience: returns Some(chart_id) if this pane holds a chart.
    pub fn chart_id(&self) -> Option<ChartId> {
        match self.content {
            PanelContent::Chart(id) => Some(id),
            _ => None,
        }
    }
}
```

Update `WorkspaceLayout`:
- `focused_chart_id()` → returns `Option<ChartId>` (only if focused pane is a chart)
- `chart_ids()` → filters to chart panes only
- `find_pane(ChartId)` → searches chart panes only
- Add `find_watchlist_pane(WatchlistId)`, `watchlist_ids()`
- `split()` and `close()` work on `PanelContent` (not just ChartId)
- `close()` returns `Option<PanelContent>` instead of `Option<ChartId>`
- Keep `next_chart_id` counter; add `next_watchlist_id` counter

**Refactor pattern for callsites**: Every place that does `pane_state.chart_id` becomes:
```rust
match &pane_state.content {
    PanelContent::Chart(chart_id) => { /* existing logic */ }
    PanelContent::Watchlist(wl_id) => { /* skip or handle */ }
}
```

The compiler will flag every broken callsite when we change the struct — this is the safety net.

**Testing**:
- Update all 11 existing layout tests to use `PaneState::chart()` constructor
- Add test: watchlist pane in layout (create, find, close)
- Add test: `focused_chart_id()` returns None when watchlist is focused
- Verify all existing tests still pass (no chart behavior regression)

**Done when**: Build succeeds with new `PanelContent` enum. All existing chart functionality unchanged. New layout tests pass.

---

### Slice 3: Watchlist Panel View + Toolbar Integration

**Goal**: Render the watchlist as a scrollable table inside a pane, with an "Add Watchlist" button in the toolbar.
**Depends on**: Slice 2

**Files to create or modify**:
- `crates/midas-app/src/app.rs` — Add `watchlists: HashMap<WatchlistId, WatchlistPanel>` to `MidasApp`, add Message variants (`AddWatchlist`, `WatchlistAddTicker`, `WatchlistRemoveTicker`, `WatchlistToggleFavorite`, `WatchlistTickerInputChanged`), implement update handlers
- `crates/midas-app/src/app/views.rs` — Add `view_watchlist_panel()`, add "Watchlist" button to toolbar, dispatch in pane grid body
- `crates/midas-app/src/app/persistence.rs` — Save/restore watchlists from config

**Key implementation details**:

Watchlist view structure (iced widget tree):
```
container (fill, dark background)
├── title_bar: row!["Watchlist", Space::fill, close_button]
└── body: scrollable column
    ├── header_row: row!["★", "Ticker", "Price", "Chg%", "G.ATR"]
    ├── ticker_row[0]: row![fav_btn, "AAPL", "185.50", "+1.2%", "4.30"]
    ├── ticker_row[1]: row![fav_btn, "MSFT", "--", "--", "--"]
    ├── ...
    └── add_row: row![text_input("Add ticker...", &input), add_button]
```

Price/Change%/G.ATR columns: lookup from loaded chart data:
```rust
fn ticker_market_data(&self, symbol: &str) -> (Option<f64>, Option<f64>, Option<f64>) {
    // Search all charts for one with this symbol loaded
    for chart in self.charts.values() {
        if chart.symbol.eq_ignore_ascii_case(symbol) {
            if let Some(ref data) = chart.data {
                let len = data.len();
                if len == 0 { continue; }
                let last_close = data.closes[len - 1] as f64;
                let prev_close = if len >= 2 { data.closes[len - 2] as f64 } else { last_close };
                let change_pct = ((last_close - prev_close) / prev_close) * 100.0;
                // G.ATR: simple ATR over last 14 bars
                let gatr = compute_gatr(data, 14);
                return (Some(last_close), Some(change_pct), gatr);
            }
        }
    }
    (None, None, None)
}
```

New `Message` variants:
```rust
// Watchlist management
AddWatchlist,
CloseWatchlist(WatchlistId),
// Per-watchlist editing
WatchlistTickerInputChanged(WatchlistId, String),
WatchlistAddTicker(WatchlistId),
WatchlistRemoveTicker(WatchlistId, String),
WatchlistToggleFavorite(WatchlistId, String),
```

Toolbar addition — add button next to "Add Chart":
```rust
button("Watchlist").on_press(Message::AddWatchlist)
```

Config persistence — in `build_config()`:
```rust
config.watchlists = self.watchlists.values()
    .map(|wl| wl.to_config())
    .collect();
```

In `new()` (app init), restore from config:
```rust
for wl_config in &config.watchlists {
    let id = layout.next_watchlist_id();
    watchlists.insert(id, WatchlistPanel::from_config(id, wl_config));
}
```

**Testing**:
- Integration: add watchlist via Message, verify it appears in state
- Add/remove ticker, verify persistence roundtrip
- Verify toolbar button creates a watchlist pane

**Done when**: Watchlist panel renders in the pane grid with header + ticker rows. "Add Watchlist" button works. Watchlist persists across app restarts. Favorites toggle. Add/remove tickers works.

---

### Slice 4: Drag-and-Drop Ticker to Chart

**Goal**: Drag a ticker row from the watchlist and drop it on a chart pane to load that symbol.
**Depends on**: Slice 3

**Files to create or modify**:
- `crates/midas-app/src/app.rs` — Add drag state fields to `MidasApp`, new Message variants, update handlers
- `crates/midas-app/src/app/views.rs` — Drag initiation on ticker rows, drop overlay on chart panes, floating drag label
- `crates/midas-app/src/main.rs` — Add mouse tracking subscription during drag (optional, may use pane_grid focus events)

**Key implementation details**:

App-level drag state:
```rust
// In MidasApp:
pub dragging_ticker: Option<DragTickerState>,

pub struct DragTickerState {
    pub symbol: String,
    pub source_watchlist: WatchlistId,
}
```

Message variants:
```rust
/// User started dragging a ticker row.
WatchlistDragStart(WatchlistId, String),
/// User released the drag (on any pane or outside).
WatchlistDragEnd,
/// User dropped a ticker onto a chart pane.
WatchlistDropOnChart(ChartId, String),
```

**Drag initiation**: On each ticker row, wrap in a `mouse_area` (iced 0.14) or use `button().on_press()` as drag start trigger. Since iced doesn't have native drag start/end for arbitrary widgets, use a practical approach:

Option A — **Click-to-grab, click-to-drop**: Click a drag handle on the ticker row → enters drag mode → click on a chart pane → drops. This is simpler than true mouse-drag and avoids fighting iced's event model.

Option B — **Use pane_grid drag events**: When drag mode is active and user clicks a chart pane (PaneFocused), check if drag is active and treat focus-click as a drop.

**Recommended approach (Option B hybrid)**:
1. Each ticker row has a grip/drag button. Clicking it sets `dragging_ticker = Some(...)`.
2. A visual indicator (highlighted status bar or floating banner) shows "Drop AAPL on a chart".
3. The next `PaneFocused` event on a chart pane triggers the drop: loads the symbol in that chart.
4. Press Escape or click the cancel button to cancel the drag.

This avoids complex mouse tracking and works within iced's event model.

Drop handler in `update()`:
```rust
Message::PaneFocused(pane) => {
    // Check if we're in drag-drop mode
    if let Some(drag) = self.dragging_ticker.take() {
        if let Some(PanelContent::Chart(chart_id)) = self.workspace.panes.get(pane)
            .map(|s| &s.content)
        {
            // Load the dragged symbol into the target chart
            return self.load_symbol_for_chart(*chart_id, &drag.symbol);
        }
    }
    // Normal focus behavior...
    self.workspace.set_focus(pane);
    Task::none()
}
```

Visual feedback during drag:
- Highlight the active drag in the status bar: `"Drop AAPL on a chart panel (Esc to cancel)"`
- Optional: tint chart panes with a subtle highlight border to indicate valid drop targets

**Testing**:
- Unit test: drag state lifecycle (start → drop → cleared, start → cancel → cleared)
- Integration test: DragStart + PaneFocused(chart_pane) → symbol loaded in chart

**Done when**: User can click drag handle on a watchlist ticker → click on any chart pane → chart loads that symbol. Escape cancels. Status bar shows feedback during drag.

---

### Slice 5: Polish and Edge Cases

**Goal**: Handle edge cases, improve UX, ensure robustness.
**Depends on**: Slice 4

**Files to create or modify**:
- `crates/midas-app/src/watchlist.rs` — Duplicate prevention, sort favorites to top
- `crates/midas-app/src/app/views.rs` — Empty state, color coding for change%
- `crates/midas-app/src/app.rs` — Keyboard shortcut for add watchlist

**Key implementation details**:
- **Duplicate prevention**: `add_ticker()` checks case-insensitive match before inserting
- **Favorites sort**: Favorited tickers sort to top of the list, maintaining insertion order within each group
- **Change% color**: Green for positive, red for negative, neutral for zero
- **Empty state**: When watchlist has no tickers, show centered "Add tickers to get started" message
- **Input validation**: Uppercase the ticker on submit, strip whitespace, reject empty
- **Keyboard shortcut**: Ctrl+W to toggle add-watchlist (or assign to a free key)
- **Close behavior**: Closing the last pane is already prevented by `WorkspaceLayout::close()`; if the watchlist pane is closed, the watchlist data persists in config and can be re-added

**Testing**:
- Duplicate ticker prevention
- Favorite sorting
- Empty string rejection
- Config roundtrip with empty watchlist

**Done when**: All edge cases handled, UX polished, no panics on any interaction path.

---

### Dependency Summary

```
Slice 1 (Data Model)
    ↓
Slice 2 (PanelContent Refactor)
    ↓
Slice 3 (Watchlist View + Toolbar)
    ↓
Slice 4 (Drag-Drop to Chart)
    ↓
Slice 5 (Polish)
```

All slices are sequential — each builds on the previous. Slice 1 and Slice 2 are the foundation. Slice 3 produces the first visible result. Slice 4 adds the key interaction. Slice 5 is cleanup.

## Risks & Unknowns

### Known Risks

1. **PanelContent refactor blast radius** (Slice 2): Changing `PaneState` from `chart_id: ChartId` to `content: PanelContent` will touch nearly every function in `app.rs` and `views.rs` that accesses pane state. Mitigation: the compiler catches all broken callsites; add `.chart_id()` convenience method for common case.

2. **iced pane_grid drag conflict**: The pane_grid has built-in pane-to-pane drag (for reordering). Our ticker drag-to-drop-on-chart needs to coexist with this. Mitigation: use a distinct "drag mode" (click-to-activate) rather than mouse-hold drag, avoiding conflict with pane_grid's native drag.

3. **No iced table widget**: iced 0.14 has no `Table` or `DataGrid` widget. The watchlist grid is built from `row![]` and `column![]` with manual column alignment. This works for small lists (~50 tickers) but won't scale to hundreds with virtual scrolling. Mitigation: acceptable for v1; revisit if performance is an issue.

### Unknowns

1. **Pane bounds for drop targeting**: It's unclear whether iced's `PaneFocused` event fires reliably when clicking inside a pane during our "drag mode". **Spike**: test that clicking a chart pane while app-level drag state is set correctly triggers `PaneFocused`. If not, fall back to "drop on focused chart" button in the drag banner. (Time-boxed: 1 hour)

2. **G.ATR computation**: Need to confirm what "G.ATR" means in this context (likely Global ATR — Average True Range). The computation from candle data is straightforward (14-period ATR), but the exact formula and period should be confirmed with the user. Proceeding with standard 14-period ATR.

### Dependencies

- No external library additions needed. iced 0.14's `scrollable`, `row`, `column`, `button`, `text`, `text_input` are sufficient for the grid UI.
- `serde` already in the dependency tree for config serialization.

## Testing Strategy

- **Unit tests**: Data model (watchlist CRUD, config roundtrip, ID types) — follow existing `id.rs` and `layout.rs` test patterns
- **Layout tests**: PanelContent enum in pane grid (create, find, close watchlist panes) — extend existing `layout.rs` tests
- **Integration gate**: Extend `tests/integration_gate.rs` if needed to verify watchlist panel doesn't break chart scene computation
- **Manual testing**: Drag-drop flow, persistence across restart, add/remove tickers

## Out of Scope

- **Multiple watchlists**: Only one default watchlist per workspace. Multi-watchlist tabs are a separate feature.
- **Live price data**: Price/Change%/G.ATR populated from loaded chart data only. Real-time feed integration is Phase 1 (IB API).
- **Chart-to-watchlist linking**: User explicitly said "don't link it to anything yet."
- **Symbol search/autocomplete**: Simple text input for now. Autocomplete is a separate feature.
- **Watchlist-to-watchlist drag**: No reordering rows by drag within the list (use favorites to pin to top).
- **Column sorting by click**: Headers are display-only for now.
- **Column resizing**: Fixed column widths.
- **Floating watchlist windows**: Pop-out support can be added later following the existing floating chart pattern.
- **Right-click context menu on ticker rows**: Future enhancement.
