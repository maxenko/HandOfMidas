# Feature: Ticker Watchlist Panel

## Overview

A new panel type — a persistent, editable grid of tickers — that lives alongside chart panels in the workspace's pane grid. Users see columns for Favorite, Ticker, Price, Change%, and G.ATR. The watchlist persists across sessions. Users can drag a ticker row from the watchlist and drop it onto any chart pane to call `load_symbol_for_chart` on that chart.

This is the first non-chart panel type, so it also introduces the `PanelContent` abstraction that allows the pane grid to host heterogeneous panel types.

**Assumptions**:
- Price, Change%, and G.ATR columns exist in the data model but are populated from available test data when a symbol has been loaded in any chart. Blank otherwise (no live feed yet). G.ATR uses the existing `compute_gerchik_atr()` function (already implemented in `midas-chart`).
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
1. **App-level drag state** — `MidasApp` tracks `dragging_ticker: Option<DragState>`. Mouse-down on ticker row sets it, mouse-up clears it. During drag, a floating label follows the cursor. On release over a chart pane, call `load_symbol_for_chart()`.
2. **Clipboard/double-click** — Simpler: double-click loads in focused chart. No drag visual needed. But user explicitly requested drag-drop.

**Recommendation**: Option 1 (app-level drag state). The user specifically requested drag-drop. Track drag state centrally in `MidasApp`. Use a click-to-grab, click-to-drop interaction (not true mouse-hold drag) to work within iced's event model.

**Confidence**: medium — iced 0.14.2's `PaneGrid` has `.on_click()`, but it was previously tried and removed from this codebase because it consumed the initial mouse-press, breaking title-bar buttons on unfocused panes (see `app.rs:660`). A 1-hour spike (Slice 4a) will test conditional `.on_click()` during drag mode only, or `mouse_area` wrapping as fallback. Last resort: explicit drop-target buttons in a drag banner.

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

/// Records the type of panel in a pane position for layout restoration.
/// Used in `AppConfig::panel_order` to reconstruct the pane grid with
/// the correct mix of chart and watchlist panels.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum PanelSlot {
    /// A chart panel — index into `AppConfig::charts`.
    Chart { chart_index: usize },
    /// A watchlist panel — index into `AppConfig::watchlists`.
    Watchlist { watchlist_index: usize },
}

// In AppConfig:
#[serde(default)]
pub watchlists: Vec<WatchlistConfig>,
/// Ordered list of panel types in the pane grid, in BTreeMap key order
/// (pane creation order — NOT spatial position). Save and restore both
/// use the same iteration order, so the mapping is self-consistent.
/// Full spatial layout topology is not preserved (same as chart-only restore).
/// If absent or empty, falls back to charts-only restoration (backward compat).
#[serde(default)]
pub panel_order: Vec<PanelSlot>,
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
**Depends on**: `WatchlistId` type in `id.rs` (can run in parallel with Slice 1 after the WatchlistId micro-step)

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
- `split()` always creates a **chart** pane (regardless of what panel type is being split)
- `close()` returns `Option<PanelContent>` instead of `Option<ChartId>`
- Keep `next_chart_id` counter; add `next_watchlist_id` counter

**Policy decisions for the refactor**:
- **Splitting a watchlist pane**: Always creates a new chart pane in the other half. Splitting does not clone the watchlist.
- **Layout presets** (`apply_preset`): Presets create chart-only layouts. Applying a preset destroys any watchlist pane in the layout. The watchlist data persists in `MidasApp::watchlists` and config, and can be re-added via toolbar. This is acceptable for v1.
- **`PaneClose` handler dead arm**: After this refactor, `close()` returns `Option<PanelContent>`. The `PaneClose` handler must match both variants. The `PanelContent::Watchlist` arm should be a no-op with `tracing::warn!("Closing watchlist pane — watchlist data preserved")` until Slice 3 wires `self.watchlists.remove()`. Do NOT use `unreachable!()` or `todo!()` — the path may be hit during testing.
- **Callsites to enumerate**: `split()` (3 callers in app.rs), `close()` (1 caller), `chart_ids()` (persistence + view), `focused_chart_id()` (toolbar, keyboard, presets), `find_pane()` (pop-out, focus), `apply_preset()` and its 4 helper methods (`preset_single`, `preset_split`, `preset_grid_2x2`), and the pane_grid closure in `view_content()`.

**Refactor pattern for callsites**: Every place that does `pane_state.chart_id` becomes:
```rust
match &pane_state.content {
    PanelContent::Chart(chart_id) => { /* existing logic */ }
    PanelContent::Watchlist(wl_id) => { /* skip or handle */ }
}
```

The compiler will flag every broken callsite when we change the struct — this is the safety net.

**Testing**:
- Update all 11 existing layout tests to use `PaneState::chart()` constructor (most are just constructor changes; `close_removes_pane` also needs its return-type assertion updated from `Option<ChartId>` to `Option<PanelContent>`)
- Add test: watchlist pane in layout (create, find, close)
- Add test: `focused_chart_id()` returns None when watchlist is focused
- Add test: `split()` on a watchlist pane creates a chart pane
- Add test: `apply_preset()` produces only chart panes (no watchlist panes survive)
- Verify all existing tests still pass (no chart behavior regression)

**Done when**: Build succeeds with new `PanelContent` enum. All existing chart functionality unchanged. New layout tests pass. Clippy clean (no dead-code warnings from Watchlist arms).

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
/// Market data snapshot for one watchlist ticker, derived from loaded chart data.
struct TickerMarketData {
    last_price: Option<f64>,
    change_pct: Option<f64>,
    gatr_text: Option<String>,     // e.g. "ATR 67%"
    gatr_color: Option<[f32; 4]>,  // Green or red
}

fn ticker_market_data(&self, symbol: &str) -> TickerMarketData {
    // Search all charts for one with this symbol loaded
    for chart in self.charts.values() {
        if chart.symbol.eq_ignore_ascii_case(symbol) {
            if let Some(ref data) = chart.data {
                let len = data.len();
                if len == 0 { continue; }
                let last_close = data.closes[len - 1] as f64;
                let prev_close = if len >= 2 { data.closes[len - 2] as f64 } else { last_close };
                let change_pct = ((last_close - prev_close) / prev_close) * 100.0;

                // G.ATR: use existing compute_gerchik_atr() from midas_chart.
                // Requires candle duration estimation (same as chart overlay).
                let candle_duration = midas_chart::estimate_candle_duration(data.as_ref());
                let gatr = midas_chart::gerchik_atr::compute_gerchik_atr(
                    data.as_ref(), candle_duration,
                );
                // GerchikAtrRender has: pct (f32), text (String), color ([f32; 4])

                return TickerMarketData {
                    last_price: Some(last_close),
                    change_pct: Some(change_pct),
                    gatr_text: gatr.as_ref().map(|g| g.text.clone()),
                    gatr_color: gatr.as_ref().map(|g| g.color),
                };
            }
        }
    }
    TickerMarketData { last_price: None, change_pct: None, gatr_text: None, gatr_color: None }
}
```

Note: `compute_gerchik_atr()` returns `None` for daily+ timeframes (only produces
output for intraday charts). The watchlist will show `--` for G.ATR when only daily
data is loaded, which is correct behavior.

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

// Record the panel order so we can restore watchlist pane positions.
// Iterate panes in BTreeMap key order (pane creation order, not spatial).
let mut chart_idx = 0;
let mut wl_idx = 0;
config.panel_order = self.workspace.panes.panes.values().map(|ps| {
    match &ps.content {
        PanelContent::Chart(_) => {
            let slot = PanelSlot::Chart { chart_index: chart_idx };
            chart_idx += 1;
            slot
        }
        PanelContent::Watchlist(_) => {
            let slot = PanelSlot::Watchlist { watchlist_index: wl_idx };
            wl_idx += 1;
            slot
        }
    }
}).collect();
```

In `new()` (app init), restore from config:
```rust
// If panel_order is present, use it to reconstruct the mixed layout.
// If absent (old config), fall back to charts-only restoration.
if config.panel_order.is_empty() {
    // Legacy: all panes are charts
    for chart_config in &config.charts { /* existing chart restore logic */ }
} else {
    for slot in &config.panel_order {
        match slot {
            PanelSlot::Chart { chart_index } => {
                // Bounds-check: config may be hand-edited or stale.
                let Some(chart_config) = config.charts.get(*chart_index) else {
                    tracing::warn!(
                        "panel_order references invalid chart index {chart_index}, skipping"
                    );
                    continue;
                };
                // Restore chart from chart_config
            }
            PanelSlot::Watchlist { watchlist_index } => {
                let Some(wl_config) = config.watchlists.get(*watchlist_index) else {
                    tracing::warn!(
                        "panel_order references invalid watchlist index {watchlist_index}, skipping"
                    );
                    continue;
                };
                let id = layout.next_watchlist_id();
                watchlists.insert(id, WatchlistPanel::from_config(id, wl_config));
                // Split pane with PaneState::watchlist(id)
            }
        }
    }
}
```

**Input validation (correctness, not polish)**:
- `add_ticker()` normalizes to uppercase and trims whitespace before inserting
- `add_ticker()` rejects empty strings after trimming
- `add_ticker()` checks case-insensitive duplicate before inserting (returns early if duplicate)
- Empty watchlist shows centered "Add tickers to get started" placeholder message

**Testing**:
- Integration: add watchlist via Message, verify it appears in state
- Add/remove ticker, verify persistence roundtrip
- Verify toolbar button creates a watchlist pane
- Duplicate ticker prevention (case-insensitive)
- Empty string rejection
- Config roundtrip with empty watchlist

**Done when**: Watchlist panel renders in the pane grid with header + ticker rows. "Add Watchlist" button works. Watchlist persists across app restarts. Favorites toggle. Add/remove tickers works. Duplicates and empty inputs are rejected. Empty watchlist shows placeholder text.

---

### Slice 4a: Drag-and-Drop Spike (Pre-requisite)

**Goal**: Validate that the click-to-grab, click-to-drop mechanism works within iced's pane_grid event model.
**Depends on**: Slice 2 (needs PanelContent enum so we can distinguish chart vs watchlist panes)
**Time-box**: 1 hour

**Context**: iced 0.14.2's `PaneGrid` API **does** have `.on_click()` (confirmed at `iced_widget-0.14.2/src/pane_grid.rs:238`). However, it was previously tried in this codebase and deliberately removed — see `app.rs:660`: *"the `PaneGrid::on_click` approach consumed the initial mouse-press and prevented title-bar buttons on unfocused panes from registering clicks."* Focus is instead handled implicitly via `focus_chart()` calls inside chart message handlers. We need to validate an approach that doesn't reintroduce this known issue.

**Spike tasks** (in priority order):
1. **Conditional `.on_click()` during drag mode only**: Add `.on_click(Message::PaneFocused)` to the `PaneGrid` only when `self.dragging_ticker.is_some()`. When not in drag mode, omit it (preserving current behavior). Test that title-bar buttons still work on unfocused panes when drag mode is inactive, and that pane identification works when drag mode is active.
2. **`mouse_area` wrapper**: If conditional on_click is impractical (iced may rebuild the widget tree each frame anyway), wrap each chart pane body in a `mouse_area` that emits `PaneClicked(ChartId)` on press — but only when drag mode is active. Test that this fires reliably and does NOT conflict with `PaneDragged` (pane reorder) events or the shader widget's mouse handling.
3. **Explicit drop buttons**: If neither works, the drag banner includes explicit "Drop on [Chart1 AAPL] [Chart2 MSFT] ..." buttons for each open chart pane.

**Success criteria**: Clicking a chart pane while app-level drag state is `Some(...)` reliably triggers a message identifying which chart was clicked, AND does not trigger `PaneDragged`, AND does not break title-bar buttons on unfocused panes when drag mode is inactive.

**Done when**: Approach validated. If primary approach fails, fallback documented and Slice 4b adapted.

---

### Slice 4b: Drag-and-Drop Ticker to Chart

**Goal**: Drag a ticker row from the watchlist and drop it on a chart pane to load that symbol.
**Depends on**: Slice 3, Slice 4a (spike must validate approach first)

**Files to create or modify**:
- `crates/midas-app/src/app.rs` — Add drag state fields to `MidasApp`, new Message variants, update handlers
- `crates/midas-app/src/app/views.rs` — Drag initiation on ticker rows, drop target on chart panes, drag banner
- `crates/midas-app/src/main.rs` — Wire pane click if needed (based on spike results)

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
/// User cancelled the drag (Escape or cancel button).
WatchlistDragCancel,
/// User clicked a chart pane while in drag mode (drop target).
WatchlistDropOnChart(ChartId, String),
```

**Drag initiation**: Each ticker row has a grip/drag button. Clicking it sets `dragging_ticker = Some(...)`.

**Drop detection** (approach depends on Slice 4a spike result):
- **Primary** (if `.on_click()` or `mouse_area` works): The pane click event checks `dragging_ticker` state and, if active and the target is a `PanelContent::Chart`, consumes it as a drop.
- **Fallback** (if pane click is unreliable): The drag banner shows explicit "Drop on [Chart1 AAPL] [Chart2 MSFT] ..." buttons for each open chart pane. Clicking a button emits `WatchlistDropOnChart(chart_id, symbol)`.

**Modal drag state edge cases**:
- Clicking a **non-chart pane** (another watchlist) during drag: do NOT consume the drag state. Keep it active and show feedback "Drop on a chart pane, not a watchlist".
- `PaneDragged` event during drag mode: cancel the ticker drag to prevent confusing dual-drag state.
- Escape key: cancel drag mode.
- Visible "Cancel" button in the drag banner (not just Escape — status bar text is easy to miss).

Drop handler in `update()`:
```rust
Message::WatchlistDropOnChart(chart_id, symbol) => {
    self.dragging_ticker = None;
    self.load_symbol_for_chart(chart_id, &symbol)
}

Message::WatchlistDragCancel => {
    self.dragging_ticker = None;
    Task::none()
}

// If PaneDragged fires while in drag mode, cancel the ticker drag:
Message::PaneDragged(event) => {
    if self.dragging_ticker.is_some() {
        self.dragging_ticker = None;
    }
    self.workspace.panes.drop(/* ... */);
    Task::none()
}
```

Visual feedback during drag:
- Drag banner at top of workspace: `"Drop AAPL on a chart panel"` + `[Cancel]` button
- Optional: tint chart pane borders with a subtle highlight to indicate valid drop targets

**Testing**:
- Unit test: drag state lifecycle (start → drop → cleared, start → cancel → cleared, start → PaneDragged → cancelled)
- Unit test: drop on non-chart pane keeps drag active
- Integration test: DragStart + drop on chart → symbol loaded in chart

**Done when**: User can click drag handle on a watchlist ticker → click on any chart pane → chart loads that symbol. Escape and Cancel button cancel. Pane-reorder drag cancels ticker drag. Status bar/banner shows feedback during drag.

---

### Slice 5: Polish and Cosmetics

**Goal**: Visual polish and minor UX enhancements (correctness items already handled in Slice 3).
**Depends on**: Slice 4b

**Files to create or modify**:
- `crates/midas-app/src/watchlist.rs` — Sort favorites to top
- `crates/midas-app/src/app/views.rs` — Color coding for change%
- `crates/midas-app/src/app.rs` — Keyboard shortcut for add watchlist

**Key implementation details**:
- **Favorites sort**: Favorited tickers sort to top of the list, maintaining insertion order within each group
- **Change% color**: Green for positive, red for negative, neutral for zero
- **Keyboard shortcut**: `W` key (no modifier, matching existing bare-key pattern for `1`-`7` timeframes, `H` for levels) to add/focus watchlist panel. No focus guard needed — iced's `keyboard::listen()` only receives `Status::Ignored` events, and `text_input` calls `shell.capture_event()` for all printable characters when focused, so bare-key shortcuts never fire during text editing.
- **Close behavior**: Closing the last pane is already prevented by `WorkspaceLayout::close()`; if the watchlist pane is closed, the watchlist data persists in config and can be re-added via toolbar

**Testing**:
- Favorite sorting (favorites appear above non-favorites)
- Change% renders correct color
- `W` key adds/focuses watchlist panel

**Done when**: Favorites sort to top, change% is color-coded, keyboard shortcut works.

---

### Dependency Summary

```
WatchlistId micro-step (5 min: add WatchlistId to id.rs + lib.rs re-export)
    ↓
Slice 1 (Data Model) ──────────┐
    (in parallel)               ├──→ Slice 3 (Watchlist View + Toolbar)
Slice 2 (PanelContent Refactor)┘         ↓
                                    Slice 4a (DnD Spike, can overlap with Slice 3)
                                         ↓
                                    Slice 4b (Drag-Drop to Chart)
                                         ↓
                                    Slice 5 (Polish)
```

**Parallelization**: Slices 1 and 2 can run in parallel after extracting the `WatchlistId` newtype as a 5-minute micro-step (it's the only shared artifact — 8 lines of code). This removes Slice 1 from the critical path. Slice 4a (spike) can overlap with the tail end of Slice 3 since it only needs the PanelContent enum from Slice 2.

**Critical path**: WatchlistId → Slice 2 → Slice 3 → Slice 4a → Slice 4b → Slice 5.

## Risks & Unknowns

### Known Risks

1. **PanelContent refactor blast radius** (Slice 2): Changing `PaneState` from `chart_id: ChartId` to `content: PanelContent` will touch nearly every function in `app.rs` and `views.rs` that accesses pane state. Mitigation: the compiler catches all broken callsites; add `.chart_id()` convenience method for common case.

2. **iced pane_grid drag conflict**: The pane_grid has built-in pane-to-pane drag (for reordering via `.on_drag()`). Our ticker drag-to-drop-on-chart needs to coexist with this. Mitigation: use a distinct "drag mode" (click-to-activate) rather than mouse-hold drag, avoiding conflict with pane_grid's native drag. If a `PaneDragged` event fires during ticker drag mode, cancel the ticker drag immediately (specified in Slice 4b).

3. **No iced table widget**: iced 0.14 has no `Table` or `DataGrid` widget. The watchlist grid is built from `row![]` and `column![]` with manual column alignment. This works for small lists (~50 tickers) but won't scale to hundreds with virtual scrolling. Mitigation: acceptable for v1; revisit if performance is an issue.

### Unknowns

1. **Pane click detection for drag-drop targeting**: The `PaneGrid` API has `.on_click()` (iced 0.14.2), but it was previously tried and removed from this codebase because it consumed the initial mouse-press, breaking title-bar buttons on unfocused panes (see `app.rs:660`). **Spike (Slice 4a)**: Test whether `.on_click()` can be conditionally added only during drag mode, or whether `mouse_area` wrapping is needed. Fallback: explicit drop-target buttons in the drag banner. (Time-boxed: 1 hour, scheduled as Slice 4a.)

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
