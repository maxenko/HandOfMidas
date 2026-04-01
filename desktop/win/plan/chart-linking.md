# Feature: Chart Linking (Color-Coded Link Groups)

## Overview

TC2000-style color-coded link groups that synchronize symbol and/or timeframe across chart panels (and future panel types like watchlists). Each panel gets two small link buttons — **[S]** (Symbol) and **[T]** (TimeFrame) — in its title bar. Clicking either opens a color picker with 8 color channels, a "Listen for any changes" mode, and "Not Linked". Panels sharing the same link color stay in sync: changing the symbol on any Blue-linked panel automatically pushes that symbol to all other Blue-linked panels.

**Who benefits**: Traders monitoring the same symbol across multiple timeframes (e.g., daily + hourly + 5-min) or quickly scanning a watchlist while all linked charts follow the selected ticker.

**Problem solved**: Without linking, switching between symbols requires manually updating each chart's ticker input. With 4-8 open charts, this is tedious. Linking automates the synchronization and mirrors the workflow every professional charting platform provides.

## Codebase Analysis

### Tech Stack
- Rust 2021, iced 0.14 (Elm architecture), wgpu 27, tokio, serde, TOML config

### Architecture Pattern
- Elm (Message → update → view), sans-IO chart core (`midas-chart`), binary split-tree pane layout via iced `pane_grid`

### Key Files the Feature Will Touch

| File | Role | Change Type |
|---|---|---|
| `crates/midas-core/src/link.rs` (new) | Link enums (`LinkColor`, `LinkMode`) | Pure data types for config + app use |
| `crates/midas-core/src/config.rs` | Config types | Add `LinkMode` fields to `ChartConfig` (typed enum, not strings) |
| `crates/midas-core/src/lib.rs` | Re-exports | Add `LinkColor`, `LinkMode` exports |
| `crates/midas-app/src/app.rs` | App state, Message enum, update handlers | Add link state to `ChartPanel`, new Message variants, propagation methods |
| `crates/midas-app/src/app/views.rs` | Title bar rendering | Add [S] and [T] link buttons + color picker dropdown |
| `crates/midas-app/src/app/persistence.rs` | Config build/restore | Serialize/deserialize link state per chart |
| `crates/midas-app/src/link.rs` (new) | UI-specific link helpers | Color rendering, `LinkDimension`, target-matching pure function |

### Existing Patterns the Feature Must Follow

- **Elm Message flow**: User event → `Message` variant → `update()` → state mutation → `mark_config_dirty()`
- **Config persistence**: TOML via serde with `#[serde(default)]` for backward compat, atomic writes, 2-second debounce
- **ID newtypes**: Copy + Clone + Hash + Eq + serde, defined in `midas-core/src/id.rs`
- **Title bar buttons**: Small `button(text("X").size(10))` with `.padding([1, 4])` and `button::primary` / `button::text` styles (see `views.rs:400-466`)
- **Cross-chart state**: `crosshair_sync: Option<(ChartId, i64, f64, String)>` on `MidasApp` is the existing precedent for cross-chart coordination

### Related Existing Features

- **Crosshair sync**: Already broadcasts crosshair position across same-symbol charts via `crosshair_sync` field on `MidasApp`. Same single-field-on-app-state pattern.
- **Level placement preview**: `placing_preview: Option<(ChartId, String, f64)>` — another cross-chart broadcast.
- **Watchlist panel** (planned, not yet implemented): The `watchlist-panel.md` plan describes ticker-to-chart symbol propagation via drag-drop. Chart linking subsumes and generalizes this — when a watchlist joins a color group, ticker selection automatically pushes to all linked charts.

### Blast Radius

- `ChartPanel` struct: 2 new fields (`symbol_link`, `timeframe_link`)
- `ChartConfig` struct: 2 new optional fields (backward compat via `#[serde(default)]`)
- `Message` enum: 4 new variants
- `views.rs`: Title bar content function gains ~60 lines for link buttons + picker
- `persistence.rs`: `build_config()` gains 2 fields per chart
- `app.rs` update handlers: `PanelSymbolSubmitted` and `PanelTimeframeSelected` gain propagation calls (~5 lines each)
- **No existing tests break** — all changes are additive, defaults preserve existing behavior

### Industry Precedent (from `plan/cross-chart-sync-research.md`)

| Platform | Linking Model | Key Insight |
|---|---|---|
| Bloomberg Terminal | Color-coded Launchpad Groups | Security groups = purely symbol routing by color |
| ThinkOrSwim | Color-coded link groups (symbol) + separate drawing sync | Symbol linking is orthogonal to annotation sync |
| NinjaTrader 8 | Per-drawing `IsGlobal` flag | Linking is per-drawing, not per-chart |
| TC2000 | Color-coded [S] + [T] buttons per panel | **Our reference model** — independent symbol and timeframe linking |

TC2000's model (independent S and T link dimensions, 8 colors + listen-all + unlinked) is the most refined for multi-timeframe workflows and is what we implement here.

---

## Design Decisions

### Decision: Link state storage location

**Context**: Link mode must be stored somewhere for each chart. Options differ in how they handle floating (pop-out) charts and future panel types.

**Options considered**:
1. **On PaneState** — Link state lives alongside `chart_id` and `is_focused` in the pane grid. Simple, but floating charts (stored in `floating_charts: HashMap<window::Id, ChartPanel>`) have no `PaneState`, so they'd lose link state on pop-out.
2. **On ChartPanel** — Link state lives alongside `symbol` and `timeframe`. Floating charts keep their ChartPanel, so link state survives pop-out. Future panel types (watchlist) would each have their own link fields.

**Recommendation**: Option 2 (on ChartPanel). Link state is a property of the chart, not the pane slot. Floating charts automatically participate in link groups. When the watchlist panel is added, it gets its own `symbol_link: LinkMode` field — only symbol linking is meaningful for watchlists (not timeframe).

**Confidence**: high

### Decision: Propagation model (push vs pull)

**Context**: When a symbol changes on a Blue-linked chart, how do other Blue-linked charts learn about it?

**Options considered**:
1. **Direct push** — The `PanelSymbolSubmitted` handler calls `propagate_symbol_change()`, which iterates all charts and calls `load_symbol_for_chart()` on matching ones. Simple, O(n) with n = chart count.
2. **Event bus / generation counter** — Post a `LinkEvent` and let each chart poll it (like annotation sync). More decoupled but over-engineered for <20 charts.

**Recommendation**: Option 1 (direct push). With <20 charts, iteration is trivial. The iced update loop is single-threaded, so there are no concurrency concerns. This matches the existing `crosshair_sync` pattern: the handler directly sets state on sibling charts.

**Confidence**: high

### Decision: Behavior on joining a link group

**Context**: When a user changes an unlinked chart to Blue Symbol Link, should the chart immediately adopt the Blue group's current symbol?

**Options considered**:
1. **No immediate sync** — The chart keeps its current symbol. Only future changes propagate.
2. **Adopt group symbol** — If any chart in the Blue group already exists, the newly-joined chart switches to that chart's symbol.

**Recommendation**: Option 2 (adopt on join). This matches TC2000 behavior and is intuitive — joining a group means "show what the group shows." Implementation is a simple lookup: find the first chart with matching `LinkMode::Color(c)` and use its symbol. No extra state structures needed.

**Confidence**: high

### Decision: "Listen for any" behavior (broadcast vs receive-only)

**Context**: The yellow "Listen For any Symbol Changes" mode — does it only receive, or does it also broadcast?

**Options considered**:
1. **Receive-only** — ListenAll panels update when ANY color group changes, but typing a symbol into a ListenAll panel does not push to any group.
2. **Bidirectional** — ListenAll both receives from all and broadcasts to all.

**Recommendation**: Option 1 (receive-only). This matches TC2000's semantics. ListenAll is a "slave" mode — useful for a detail panel that always follows whatever the user last selected. Broadcasting from ListenAll would create confusing cross-group pollution (changing symbol on a ListenAll panel would ripple to every color group simultaneously).

**Confidence**: high

### Decision: UI for color picker

**Context**: iced 0.14 has no native dropdown/popup widget. Need to implement the color picker flyout.

**Options considered**:
1. **iced `pick_list`** — Built-in dropdown, but renders as a native selector with text-only items. No colored squares.
2. **App-level toggle state** — A boolean `link_picker_open: Option<(ChartId, LinkDimension)>` on MidasApp. When set, the title bar view renders a column of colored buttons below/adjacent to the link button. Clicking a color or clicking elsewhere dismisses it.
3. **iced overlay** — Custom overlay widget. More complex, better Z-order handling.

**Recommendation**: Option 2 (app-level toggle). Simplest approach that matches existing patterns (e.g., level editor popup uses `editing_level_id: Option<u64>` as toggle state). The picker renders inline in the pane's title bar area. Dismiss on selection, Escape, or focus change.

**Confidence**: medium — The picker may overlap other UI elements if the title bar is tight. If this proves problematic, migrate to an iced overlay in a follow-up. Acceptable for v1.

### Decision: Recursion prevention

**Context**: When propagation loads a symbol on a target chart, the handler must not re-propagate (infinite loop).

**Options considered**:
1. **Guard flag** — `propagating_link: bool` on MidasApp. Set before propagation, cleared after.
2. **Separate code path** — Propagation calls `load_symbol_for_chart()` directly, which does NOT trigger propagation. Only user-initiated handlers (`PanelSymbolSubmitted`, `PanelTimeframeSelected`, keyboard shortcuts) call the propagation wrapper.

**Recommendation**: Option 2 (separate code path). Cleaner than a flag — the architecture naturally prevents recursion because `load_symbol_for_chart()` has no linking logic. Only the 3 user-initiated entry points call `propagate_*()` after the local load. No flag to forget to clear.

**Confidence**: high

---

## Implementation Plan

### Slice 1: Core Types — LinkColor, LinkMode

**Goal**: Define the linking type system in `midas-core` (following the "enum over strings" rule from CLAUDE.md) and add typed config persistence fields.
**Depends on**: None

**Files to create or modify**:
- `crates/midas-core/src/link.rs` (new) — `LinkColor` enum, `LinkMode` enum, `display_name()` helper
- `crates/midas-core/src/lib.rs` — Register `link` module, re-export `LinkColor`, `LinkMode`
- `crates/midas-core/src/config.rs` — Add typed `symbol_link: LinkMode` and `timeframe_link: LinkMode` fields to `ChartConfig`
- `crates/midas-app/src/link.rs` (new) — UI-specific helpers: `indicator_rgba()`, `LinkDimension` enum, `find_link_targets()` pure function

**Key implementation details**:

The enums live in `midas-core` (not `midas-app`) because they are pure data types with
zero UI dependencies — same crate as `Timeframe`, `ChartId`, and `ChartConfig`. This
follows the project's own rule: **"SecurityType enum over strings"** (CLAUDE.md rule 5).
Using typed enums directly in `ChartConfig` eliminates the string conversion layer and
its bug surface (a new color variant forgotten in a match arm is a compile error, not
a silent fallback to `Unlinked`).

```rust
// crates/midas-core/src/link.rs

/// The 8 link group colors, matching TC2000's palette.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkColor {
    Blue,
    Red,
    Orange,
    Green,
    Purple,
    Violet,
    Teal,
    Brown,
}

/// How a panel participates in link group synchronization.
#[derive(Copy, Clone, Eq, PartialEq, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkMode {
    /// Not linked — independent panel.
    #[default]
    Unlinked,
    /// Linked to a specific color group — sends and receives changes.
    Color(LinkColor),
    /// Listens to ALL color groups — receive-only, never broadcasts.
    ListenAll,
}

impl LinkColor {
    /// All 8 colors in display order.
    pub const ALL: [LinkColor; 8] = [
        Self::Blue, Self::Red, Self::Orange, Self::Green,
        Self::Purple, Self::Violet, Self::Teal, Self::Brown,
    ];

    /// Display name for the UI.
    pub const fn display_name(&self) -> &'static str {
        match self {
            Self::Blue => "Blue",
            Self::Red => "Red",
            Self::Orange => "Orange",
            Self::Green => "Green",
            Self::Purple => "Purple",
            Self::Violet => "Violet",
            Self::Teal => "Teal",
            Self::Brown => "Brown",
        }
    }
}
```

UI-specific rendering helpers in `crates/midas-app/src/link.rs` as **free functions**
(cannot add inherent impls to types from another crate):

```rust
// crates/midas-app/src/link.rs — UI helpers, not in midas-core

use midas_core::{LinkColor, LinkMode};

/// RGBA color for a link color (sRGB space).
pub const fn link_color_rgba(c: LinkColor) -> [f32; 4] {
    match c {
        LinkColor::Blue   => [0.20, 0.40, 0.90, 1.0],
        LinkColor::Red    => [0.90, 0.15, 0.15, 1.0],
        LinkColor::Orange => [0.95, 0.55, 0.05, 1.0],
        LinkColor::Green  => [0.15, 0.75, 0.25, 1.0],
        LinkColor::Purple => [0.55, 0.15, 0.75, 1.0],
        LinkColor::Violet => [0.70, 0.35, 0.85, 1.0],
        LinkColor::Teal   => [0.15, 0.75, 0.80, 1.0],
        LinkColor::Brown  => [0.55, 0.35, 0.15, 1.0],
    }
}

/// RGBA color for the link button indicator.
/// Unlinked = gray, ListenAll = yellow/gold, Color = that color.
pub fn link_mode_indicator_rgba(mode: LinkMode) -> [f32; 4] {
    match mode {
        LinkMode::Unlinked => [0.40, 0.40, 0.40, 1.0],
        LinkMode::ListenAll => [0.95, 0.85, 0.10, 1.0],
        LinkMode::Color(c) => link_color_rgba(c),
    }
}
```

All view code must call these as `link_mode_indicator_rgba(mode)` and
`link_color_rgba(color)`, never as method calls.

Config changes in `crates/midas-core/src/config.rs`:
```rust
// In ChartConfig — add at the end:
/// Symbol link mode for cross-chart symbol synchronization.
#[serde(default)]
pub symbol_link: LinkMode,
/// Timeframe link mode for cross-chart timeframe synchronization.
#[serde(default)]
pub timeframe_link: LinkMode,
```

Since `LinkMode` derives `Default` (= `Unlinked`) and `Serialize`/`Deserialize`,
`#[serde(default)]` handles backward compatibility automatically. Old config files
without these fields will deserialize to `Unlinked`. The TOML representation is:

```toml
# Color-linked chart:
symbol_link = { color = "blue" }
timeframe_link = "unlinked"

# Listen-all chart:
symbol_link = "listen_all"
```

**Testing**:
- Unit tests in `midas-core/src/link.rs`: `LinkColor::ALL` has 8 elements, serde roundtrip for each `LinkMode` variant, `Default` for `LinkMode` is `Unlinked`
- Config roundtrip test in `config.rs`: TOML with `symbol_link = { color = "blue" }` and `timeframe_link = "listen_all"` deserializes and re-serializes correctly
- Backward compat test: TOML without `symbol_link`/`timeframe_link` fields deserializes to `LinkMode::Unlinked`

**Done when**: Types compile, all tests pass, config roundtrip works with and without link fields.

---

### Slice 2: State Integration — ChartPanel + Persistence

**Goal**: Add link state to `ChartPanel`, wire persistence so link assignments survive across sessions.
**Depends on**: Slice 1

**Files to modify**:
- `crates/midas-app/src/app.rs` — Add `symbol_link: LinkMode` and `timeframe_link: LinkMode` to `ChartPanel`, initialize as `Unlinked` in `make_empty_panel()`
- `crates/midas-app/src/app/persistence.rs` — Add link fields to `build_config()`, restore link state in `new()` / `restore_panel()`

**Key implementation details**:

ChartPanel additions:
```rust
pub struct ChartPanel {
    // ... existing fields ...
    /// Symbol link group for cross-chart symbol synchronization.
    pub symbol_link: LinkMode,
    /// Timeframe link group for cross-chart timeframe synchronization.
    pub timeframe_link: LinkMode,
}
```

In `make_empty_panel()`:
```rust
symbol_link: LinkMode::Unlinked,
timeframe_link: LinkMode::Unlinked,
```

In `build_config()` (`persistence.rs`):
```rust
ChartConfig {
    // ... existing fields ...
    symbol_link: panel.symbol_link,
    timeframe_link: panel.timeframe_link,
}
```

In `restore_panel()`:
```rust
panel.symbol_link = cfg.symbol_link;
panel.timeframe_link = cfg.timeframe_link;
```

Note: No conversion needed — `LinkMode` is used directly in `ChartConfig` with
serde derives. Copy semantics make this trivial.

**Testing**:
- Create a ChartPanel, set link modes, build config, restore — verify roundtrip
- Verify old config files (no link fields) load with `Unlinked` defaults
- Verify `cargo test --workspace` — no regressions

**Done when**: Link state is on ChartPanel, persists across restarts, backward compatible.

---

### Slice 3: Propagation Engine

**Goal**: Implement the core linking logic — when a chart changes symbol/timeframe, propagate to linked charts. Includes a pure target-matching function for testability and a floating chart data-loading helper.
**Depends on**: Slice 2

**Files to create or modify**:
- `crates/midas-app/src/link.rs` — Add `find_link_targets()` pure function, `LinkDimension` enum
- `crates/midas-app/src/app.rs` — Add `propagate_symbol_change()`, `propagate_timeframe_change()`, `load_data_for_floating_chart()` methods. Wire propagation into `PanelSymbolSubmitted`, `PanelTimeframeSelected`, and `set_active_timeframe` (keyboard shortcuts, app.rs:1438). Add `SetSymbolLink` and `SetTimeframeLink` Message variants.

**Key implementation details**:

**Sub-task 3a: Pure target-matching function** (in `midas-app/src/link.rs`):

Extract the "who receives this propagation?" logic into a pure function that
is trivially unit-testable without constructing `MidasApp`:

```rust
use midas_core::{ChartId, LinkColor, LinkMode};

/// Given a source's link mode, find which chart IDs should receive the
/// propagated change. Returns an empty vec if the source doesn't broadcast.
///
/// `panels` is an iterator of (id, link_mode) for all candidate charts
/// (excluding the source). This works for both symbol and timeframe linking.
pub fn find_link_targets<I>(source_link: LinkMode, panels: I) -> Vec<ChartId>
where
    I: IntoIterator<Item = (ChartId, LinkMode)>,
{
    let source_color = match source_link {
        LinkMode::Color(c) => c,
        // Unlinked and ListenAll do not broadcast
        _ => return Vec::new(),
    };

    panels
        .into_iter()
        .filter(|(_, link)| match link {
            LinkMode::Color(c) => *c == source_color,
            LinkMode::ListenAll => true,
            LinkMode::Unlinked => false,
        })
        .map(|(id, _)| id)
        .collect()
}
```

**Sub-task 3b: Floating chart data loader**:

`load_test_data_for_floating_chart` does not currently exist in the codebase.
Add it as a new method on `MidasApp`, mirroring `load_test_data_for_chart`
(app.rs:577-649) but indexing into `self.floating_charts` by `window::Id`:

```rust
/// Load test data for a floating chart. Mirrors load_test_data_for_chart
/// but operates on self.floating_charts instead of self.charts.
fn load_data_for_floating_chart(
    &mut self,
    wid: window::Id,
    symbol: &str,
    tf: Timeframe,
    reset_camera: bool,
) {
    // Same logic as load_test_data_for_chart:
    // 1. Generate data via self.test_data.get_candles(symbol, tf, days)
    // 2. Set panel.data = Some(Arc::new(buffer))
    // 3. Set panel.load_state = LoadState::Loaded
    // 4. Mark dirty flags, optionally reset camera
    // But operating on self.floating_charts.get_mut(&wid) instead of self.charts.get_mut(&id)
    //
    // Future refactor: extract shared logic into a helper that takes &mut ChartPanel.
}
```

**Sub-task 3c: Propagation methods on `MidasApp`**:

```rust
/// After a user-initiated symbol change on `source_id`, push the new symbol
/// to all charts in the same symbol link group. Returns batched tasks for
/// any async data loads (currently Task::none, but future-proof for IB API).
///
/// Called ONLY from user-initiated handlers (PanelSymbolSubmitted, watchlist
/// ticker selection, set_active_timeframe). NOT called from
/// load_symbol_for_chart itself — this prevents infinite recursion.
fn propagate_symbol_change(&mut self, source_id: ChartId, new_symbol: &str) -> Task<Message> {
    let source_link = self.charts.get(&source_id)
        .map(|c| c.symbol_link)
        .unwrap_or(LinkMode::Unlinked);

    // Build target list using the pure function
    let pane_targets = find_link_targets(
        source_link,
        self.charts.iter()
            .filter(|(id, _)| **id != source_id)
            .map(|(id, panel)| (*id, panel.symbol_link)),
    );

    // Collect tasks from pane grid chart loads (future-proof for async)
    let mut tasks: Vec<Task<Message>> = Vec::new();
    for id in pane_targets {
        tasks.push(self.load_symbol_for_chart(id, new_symbol));
    }

    // Floating charts — use find_link_targets on floating panels too
    let floating_targets: Vec<window::Id> = self.floating_charts.iter()
        .filter(|(_, panel)| match (source_link, panel.symbol_link) {
            (LinkMode::Color(src), LinkMode::Color(tgt)) => src == tgt,
            (LinkMode::Color(_), LinkMode::ListenAll) => true,
            _ => false,
        })
        .map(|(wid, _)| *wid)
        .collect();

    for wid in floating_targets {
        if let Some(panel) = self.floating_charts.get_mut(&wid) {
            let tf = panel.timeframe;
            panel.symbol = new_symbol.to_uppercase();
            panel.symbol_input = panel.symbol.clone();
        }
        self.load_data_for_floating_chart(wid, new_symbol,
            self.floating_charts.get(&wid).map(|p| p.timeframe).unwrap_or(Timeframe::D1),
            true);
    }

    if tasks.is_empty() { Task::none() } else { Task::batch(tasks) }
}

/// After a user-initiated timeframe change on `source_id`, push the new
/// timeframe to all charts in the same timeframe link group.
fn propagate_timeframe_change(&mut self, source_id: ChartId, new_tf: Timeframe) {
    let source_link = self.charts.get(&source_id)
        .map(|c| c.timeframe_link)
        .unwrap_or(LinkMode::Unlinked);

    let targets = find_link_targets(
        source_link,
        self.charts.iter()
            .filter(|(id, _)| **id != source_id)
            .map(|(id, panel)| (*id, panel.timeframe_link)),
    );

    for id in targets {
        if let Some(chart) = self.charts.get_mut(&id) {
            chart.timeframe = new_tf;
            chart.chart_state.dirty.mark_camera();
        }
        let symbol = self.charts.get(&id)
            .map(|c| c.symbol.clone())
            .unwrap_or_default();
        if !symbol.is_empty() {
            self.load_test_data_for_chart(id, &symbol, new_tf, true);
        }
    }

    // Floating charts
    let floating_targets: Vec<window::Id> = self.floating_charts.iter()
        .filter(|(_, panel)| match (source_link, panel.timeframe_link) {
            (LinkMode::Color(src), LinkMode::Color(tgt)) => src == tgt,
            (LinkMode::Color(_), LinkMode::ListenAll) => true,
            _ => false,
        })
        .map(|(wid, _)| *wid)
        .collect();

    for wid in floating_targets {
        if let Some(panel) = self.floating_charts.get_mut(&wid) {
            panel.timeframe = new_tf;
            panel.chart_state.dirty.mark_camera();
            let symbol = panel.symbol.clone();
            if !symbol.is_empty() {
                self.load_data_for_floating_chart(wid, &symbol, new_tf, true);
            }
        }
    }
}
```

**Sub-task 3d: Wire into all 3 user-initiated entry points**:

```rust
// 1. PanelSymbolSubmitted handler — add after load_symbol_for_chart:
Message::PanelSymbolSubmitted(chart_id) => {
    let symbol = /* existing logic */;
    if !symbol.is_empty() {
        self.load_symbol_for_chart(chart_id, &symbol);
        let task = self.propagate_symbol_change(chart_id, &symbol);  // NEW
        self.mark_config_dirty();
        return task;  // return batched propagation tasks
    }
    Task::none()
}

// 2. PanelTimeframeSelected handler — add after data reload:
Message::PanelTimeframeSelected(chart_id, tf) => {
    // ... existing timeframe change logic ...
    self.propagate_timeframe_change(chart_id, tf);  // NEW
    self.mark_config_dirty();
    Task::none()
}

// 3. Keyboard shortcut — modify set_active_timeframe (app.rs:1438):
fn set_active_timeframe(&mut self, tf: Timeframe) {
    if let Some(id) = self.active_chart_id() {
        let symbol = self.charts.get(&id)
            .map(|c| c.symbol.clone())
            .unwrap_or_default();

        if let Some(chart) = self.charts.get_mut(&id) {
            chart.timeframe = tf;
            chart.chart_state.dirty.mark_camera();
        }

        if !symbol.is_empty() {
            self.load_test_data_for_chart(id, &symbol, tf, true);
        }

        self.propagate_timeframe_change(id, tf);  // NEW — propagate to linked charts
    }
}
```

New Message variants for link assignment:
```rust
/// Set the symbol link mode for a chart (docked pane grid charts).
SetSymbolLink(ChartId, LinkMode),
/// Set the timeframe link mode for a chart (docked pane grid charts).
SetTimeframeLink(ChartId, LinkMode),
```

Set-link handler with adopt-on-join (searches **both** docked and floating charts):
```rust
Message::SetSymbolLink(chart_id, mode) => {
    if let Some(chart) = self.charts.get_mut(&chart_id) {
        chart.symbol_link = mode;
    }
    // Adopt group symbol when joining a color group.
    // Search both docked and floating charts for an existing group member.
    if let LinkMode::Color(color) = mode {
        let group_symbol = self.charts.iter()
            .filter(|(id, _)| **id != chart_id)
            .map(|(_, panel)| panel)
            .chain(self.floating_charts.values())
            .find(|panel| {
                matches!(panel.symbol_link, LinkMode::Color(c) if c == color)
                && !panel.symbol.is_empty()
            })
            .map(|panel| panel.symbol.clone());
        if let Some(symbol) = group_symbol {
            self.load_symbol_for_chart(chart_id, &symbol);
        }
    }
    self.mark_config_dirty();
    Task::none()
}

Message::SetTimeframeLink(chart_id, mode) => {
    if let Some(chart) = self.charts.get_mut(&chart_id) {
        chart.timeframe_link = mode;
    }
    // Adopt group timeframe — search both docked and floating charts.
    if let LinkMode::Color(color) = mode {
        let group_tf = self.charts.iter()
            .filter(|(id, _)| **id != chart_id)
            .map(|(_, panel)| panel)
            .chain(self.floating_charts.values())
            .find(|panel| {
                matches!(panel.timeframe_link, LinkMode::Color(c) if c == color)
            })
            .map(|panel| panel.timeframe);
        if let Some(tf) = group_tf {
            let symbol = self.charts.get(&chart_id)
                .map(|c| c.symbol.clone())
                .unwrap_or_default();
            if let Some(chart) = self.charts.get_mut(&chart_id) {
                chart.timeframe = tf;
                chart.chart_state.dirty.mark_camera();
            }
            if !symbol.is_empty() {
                self.load_test_data_for_chart(chart_id, &symbol, tf, true);
            }
        }
    }
    self.mark_config_dirty();
    Task::none()
}
```

**Testing**:

The `find_link_targets` pure function is tested directly in `midas-app/src/link.rs`
without needing `MidasApp` scaffolding:
- Unit test: 3 panels (A=Blue, B=Blue, C=Red). Source=Blue → returns [B], not [C].
- Unit test: Panel with ListenAll. Source=Blue → ListenAll is in targets.
- Unit test: Source=ListenAll → returns empty (receive-only).
- Unit test: Source=Unlinked → returns empty.
- Unit test: No matching panels → returns empty.

Integration tests (require `MidasApp` or manual testing):
- Join group (set A to Blue when B is already Blue with AAPL) → A adopts AAPL.
- Join group when only group member is a floating chart → still adopts.
- Keyboard shortcut (press "6" on Blue-linked chart) → linked charts switch to D1.
- Floating chart in Blue group receives propagated symbol.

**Done when**: Symbol and timeframe changes propagate correctly within color groups across docked and floating charts. ListenAll is receive-only. Join-group adoption works (including floating charts). Keyboard shortcuts propagate. All existing tests pass.

---

### Slice 4: UI — Link Buttons and Color Picker

**Goal**: Add visible [S] and [T] link buttons to each chart's title bar with a color picker dropdown.
**Depends on**: Slice 3

**Files to modify**:
- `crates/midas-app/src/app.rs` — Add `link_picker_open: Option<(ChartId, LinkDimension)>` to `MidasApp`, add `LinkDimension` enum, add `ToggleLinkPicker` and `DismissLinkPicker` Message variants
- `crates/midas-app/src/app/views.rs` — Add link buttons in `view_title_bar_content()`, render color picker when open
- `crates/midas-app/src/link.rs` — Add `LinkDimension` enum

**Key implementation details**:

```rust
// link.rs
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum LinkDimension {
    Symbol,
    Timeframe,
}

/// Identifies which panel's picker is open — docked or floating.
/// Needed because docked charts use ChartId and floating charts use window::Id.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum PickerTarget {
    Docked(ChartId),
    Floating(window::Id),
}
```

App state:
```rust
// In MidasApp:
/// Which link picker dropdown is currently open, if any.
/// Uses PickerTarget to support both docked and floating charts.
pub link_picker_open: Option<(PickerTarget, LinkDimension)>,
```

New Message variants:
```rust
/// Toggle the link color picker for any panel (docked or floating).
ToggleLinkPicker(PickerTarget, LinkDimension),
/// Dismiss any open link picker (click-outside, Escape, or focus change).
DismissLinkPicker,
```

Title bar rendering — add [S] and [T] buttons between ticker input and timeframe buttons:

```rust
fn view_title_bar_content(&self, chart_id: ChartId) -> Element<'_, Message> {
    let chart = self.charts.get(&chart_id);

    // ... existing ticker_input ...

    // Symbol link button — small colored square with "S"
    let sym_link = chart.map(|c| c.symbol_link).unwrap_or(LinkMode::Unlinked);
    let sym_color = link_mode_indicator_rgba(sym_link);  // free function, not method
    let s_btn = button(text("S").size(9).color(Color::WHITE))
        .on_press(Message::ToggleLinkPicker(
            PickerTarget::Docked(chart_id), LinkDimension::Symbol))
        .padding([1, 3])
        .style(move |theme, status| {
            let mut style = button::text(theme, status);
            style.background = Some(Color::from_rgba(
                sym_color[0], sym_color[1], sym_color[2], sym_color[3],
            ).into());
            style.border = iced::Border {
                radius: 2.0.into(),
                ..style.border
            };
            style
        });

    // Timeframe link button
    let tf_link = chart.map(|c| c.timeframe_link).unwrap_or(LinkMode::Unlinked);
    let tf_color = link_mode_indicator_rgba(tf_link);  // free function, not method
    let t_btn = button(text("T").size(9).color(Color::WHITE))
        .on_press(Message::ToggleLinkPicker(
            PickerTarget::Docked(chart_id), LinkDimension::Timeframe))
        .padding([1, 3])
        .style(move |theme, status| {
            let mut style = button::text(theme, status);
            style.background = Some(Color::from_rgba(
                tf_color[0], tf_color[1], tf_color[2], tf_color[3],
            ).into());
            style.border = iced::Border {
                radius: 2.0.into(),
                ..style.border
            };
            style
        });

    // ... existing tf_buttons, collapse_btn, vp_btn, levels_btn, reset_btn ...

    row![
        ticker_input,
        s_btn,          // NEW
        tf_row,
        t_btn,          // NEW
        collapse_btn,
        vp_btn,
        levels_btn,
        reset_btn
    ]
    .spacing(4)
    .align_y(iced::Alignment::Center)
    .height(24)
    .into()
}
```

Color picker dropdown (rendered conditionally when `link_picker_open` matches):

```rust
fn view_link_picker(
    &self,
    chart_id: ChartId,
    dimension: LinkDimension,
) -> Element<'_, Message> {
    let dim_label = match dimension {
        LinkDimension::Symbol => "Symbol",
        LinkDimension::Timeframe => "TimeFrame",
    };

    let mut items: Vec<Element<'_, Message>> = Vec::with_capacity(10);

    // 8 color options
    for color in LinkColor::ALL {
        let mode = LinkMode::Color(color);
        let rgba = link_color_rgba(color);
        let label = format!("{} {} Link", color.display_name(), dim_label);
        let msg = match dimension {
            LinkDimension::Symbol => Message::SetSymbolLink(chart_id, mode),
            LinkDimension::Timeframe => Message::SetTimeframeLink(chart_id, mode),
        };

        let color_swatch = container(Space::new(12, 12))
            .style(move |_| container::Style {
                background: Some(Color::from_rgba(rgba[0], rgba[1], rgba[2], rgba[3]).into()),
                border: iced::Border { radius: 2.0.into(), ..Default::default() },
                ..Default::default()
            });

        items.push(
            button(row![color_swatch, text(label).size(11)].spacing(6).align_y(Alignment::Center))
                .on_press(msg)
                .padding([3, 8])
                .width(Fill)
                .style(button::text)
                .into()
        );
    }

    // "Listen for any changes"
    let listen_msg = match dimension {
        LinkDimension::Symbol => Message::SetSymbolLink(chart_id, LinkMode::ListenAll),
        LinkDimension::Timeframe => Message::SetTimeframeLink(chart_id, LinkMode::ListenAll),
    };
    let listen_rgba = link_mode_indicator_rgba(LinkMode::ListenAll);
    let listen_swatch = container(Space::new(12, 12))
        .style(move |_| container::Style {
            background: Some(Color::from_rgba(
                listen_rgba[0], listen_rgba[1], listen_rgba[2], listen_rgba[3],
            ).into()),
            border: iced::Border { radius: 2.0.into(), ..Default::default() },
            ..Default::default()
        });
    items.push(
        button(row![listen_swatch, text(format!("Listen For any {} Changes", dim_label)).size(11)]
            .spacing(6).align_y(Alignment::Center))
            .on_press(listen_msg)
            .padding([3, 8])
            .width(Fill)
            .style(button::text)
            .into()
    );

    // "Not Linked"
    let unlinked_msg = match dimension {
        LinkDimension::Symbol => Message::SetSymbolLink(chart_id, LinkMode::Unlinked),
        LinkDimension::Timeframe => Message::SetTimeframeLink(chart_id, LinkMode::Unlinked),
    };
    let gray_rgba = link_mode_indicator_rgba(LinkMode::Unlinked);
    let gray_swatch = container(Space::new(12, 12))
        .style(move |_| container::Style {
            background: Some(Color::from_rgba(
                gray_rgba[0], gray_rgba[1], gray_rgba[2], gray_rgba[3],
            ).into()),
            border: iced::Border { radius: 2.0.into(), ..Default::default() },
            ..Default::default()
        });
    items.push(
        button(row![gray_swatch, text("Not Linked").size(11)]
            .spacing(6).align_y(Alignment::Center))
            .on_press(unlinked_msg)
            .padding([3, 8])
            .width(Fill)
            .style(button::text)
            .into()
    );

    container(column(items).spacing(1).width(220))
        .style(|_| container::Style {
            background: Some(Color::from_rgb(0.15, 0.15, 0.18).into()),
            border: iced::Border {
                color: Color::from_rgb(0.3, 0.3, 0.35),
                width: 1.0,
                radius: 4.0.into(),
            },
            ..Default::default()
        })
        .padding(4)
        .into()
}
```

Picker lifecycle:
```rust
Message::ToggleLinkPicker(target, dimension) => {
    if self.link_picker_open == Some((target, dimension)) {
        self.link_picker_open = None; // toggle off
    } else {
        self.link_picker_open = Some((target, dimension));
    }
    Task::none()
}

Message::DismissLinkPicker => {
    self.link_picker_open = None;
    Task::none()
}

// In SetSymbolLink and SetTimeframeLink handlers, dismiss picker:
Message::SetSymbolLink(chart_id, mode) => {
    self.link_picker_open = None;  // dismiss
    // ... existing logic ...
}

// On Escape key, dismiss picker if open:
// In KeyPressed handler:
if self.link_picker_open.is_some() {
    self.link_picker_open = None;
    return Task::none();
}

// On PaneFocused, dismiss picker:
Message::PaneFocused(pane) => {
    self.link_picker_open = None;  // dismiss on focus change
    // ... existing logic ...
}
```

**Picker positioning — concrete strategy**:

The picker **cannot** render inside the title bar (24px height would expand to ~260px,
breaking layout). Instead, render it in the **pane body** as the top layer of a `Stack`.

**15-minute spike** at the start of Slice 4: Prototype this approach:
1. In `view_pane_body(chart_id)`, wrap the existing chart shader widget in a `Stack`.
2. When `link_picker_open == Some((chart_id, dim))`, push `view_link_picker(chart_id, dim)`
   as a second `Stack` layer, aligned to the top-left of the pane body.
3. Verify the picker renders visibly above the chart content.

**Fallback** (if `Stack` layering clips or doesn't work): Render the picker as a
column at the very top of the pane body, pushing the chart content down. This is
less elegant but functionally correct.

```rust
fn view_pane_body(&self, chart_id: ChartId) -> Element<'_, Message> {
    let chart_widget = /* existing chart shader widget */;

    // If link picker is open for this docked chart, overlay it
    if let Some((PickerTarget::Docked(picker_id), dim)) = self.link_picker_open {
        if picker_id == chart_id {
            let picker = self.view_link_picker(chart_id, dim);
            return Stack::new()
                .push(chart_widget)
                .push(
                    container(picker)
                        .align_x(iced::alignment::Horizontal::Left)
                        .align_y(iced::alignment::Vertical::Top)
                        .padding([4, 4])
                )
                .into();
        }
    }

    chart_widget
}
```

**Testing**:

Unit tests for picker state machine (in `app.rs` or `link.rs`):
- `ToggleLinkPicker(id, Symbol)` → `link_picker_open` is `Some((id, Symbol))`
- Same toggle again → `link_picker_open` is `None`
- `ToggleLinkPicker(id, Timeframe)` while Symbol picker is open → switches to Timeframe
- `DismissLinkPicker` → `link_picker_open` is `None`
- `SetSymbolLink` → `link_picker_open` is `None` (auto-dismiss)
- `PaneFocused` → `link_picker_open` is `None` (auto-dismiss)
- Escape key when picker open → `link_picker_open` is `None`, event consumed

Manual testing:
- Click [S] → picker appears above chart, select Blue → button turns blue, picker dismisses
- Click [T] → picker appears, select Red → button turns red
- Escape dismisses picker, clicking another pane dismisses picker
- Verify picker doesn't visually break at narrow pane widths

**Done when**: Link buttons render in all chart title bars. Color picker opens/closes as a Stack overlay in the pane body. Selecting a color updates the link state and closes the picker. Visual indicators show the current link color.

---

### Slice 5: Floating Chart Link UI

**Goal**: Add link buttons to floating chart title bars so users can view and change link state on pop-out charts.
**Depends on**: Slice 4

**Files to modify**:
- `crates/midas-app/src/app.rs` — Add `FloatingSetSymbolLink`, `FloatingSetTimeframeLink` Message variants with handlers (picker toggle uses unified `ToggleLinkPicker(PickerTarget::Floating(wid), dim)`)
- `crates/midas-app/src/app/views.rs` — Add [S] and [T] link buttons to floating chart title bars, floating picker rendering

**Key implementation details**:

Floating chart pop-out preserves link state automatically (link state is on
`ChartPanel`, which moves to `floating_charts` during pop-out). Pop-in
moves it back. No extra work needed for state preservation.

Floating charts use `window::Id` (not `ChartId`) for identification. The
existing `SetSymbolLink(ChartId, LinkMode)` message cannot address floating
charts. Add dedicated message variants for set-link (the picker toggle is
already handled by `ToggleLinkPicker(PickerTarget::Floating(wid), dim)`
via the unified `PickerTarget` enum from Slice 4):

```rust
/// Set the symbol link mode for a floating chart.
FloatingSetSymbolLink(window::Id, LinkMode),
/// Set the timeframe link mode for a floating chart.
FloatingSetTimeframeLink(window::Id, LinkMode),
```

Handlers mirror the docked chart handlers but index into `self.floating_charts`.
The adopt-on-join search **excludes the panel being modified** to prevent
adopting its own stale symbol:

```rust
Message::FloatingSetSymbolLink(wid, mode) => {
    self.link_picker_open = None;
    if let Some(panel) = self.floating_charts.get_mut(&wid) {
        panel.symbol_link = mode;
    }
    // Adopt group symbol — search docked charts + OTHER floating charts.
    // Exclude the floating panel at `wid` (it was just set and may have stale symbol).
    if let LinkMode::Color(color) = mode {
        let group_symbol = self.charts.values()
            .chain(
                self.floating_charts.iter()
                    .filter(|(id, _)| **id != wid)
                    .map(|(_, panel)| panel)
            )
            .find(|panel| {
                matches!(panel.symbol_link, LinkMode::Color(c) if c == color)
                && !panel.symbol.is_empty()
            })
            .map(|panel| panel.symbol.clone());
        if let Some(ref symbol) = group_symbol {
            if let Some(panel) = self.floating_charts.get_mut(&wid) {
                panel.symbol = symbol.to_uppercase();
                panel.symbol_input = panel.symbol.clone();
            }
            let tf = self.floating_charts.get(&wid)
                .map(|p| p.timeframe).unwrap_or(Timeframe::D1);
            self.load_data_for_floating_chart(wid, symbol, tf, true);
        }
    }
    self.mark_config_dirty();
    Task::none()
}

// FloatingSetTimeframeLink — same pattern with timeframe adoption + self-exclusion
```

Floating title bar rendering: The [S] and [T] buttons emit
`ToggleLinkPicker(PickerTarget::Floating(wid), dim)` (unified message).
The picker dropdown emits `FloatingSetSymbolLink(wid, mode)` instead of
`SetSymbolLink(chart_id, mode)`. Extract the button rendering and picker
widget into shared helpers parameterized by the message constructor to
avoid duplicating the view code.

Floating picker rendering in `view_floating_body`: Same `Stack` overlay
pattern as docked charts. Check `link_picker_open` for
`PickerTarget::Floating(wid)` match:

```rust
if let Some((PickerTarget::Floating(picker_wid), dim)) = self.link_picker_open {
    if picker_wid == wid {
        // render picker overlay, same as docked but with Floating* messages
    }
}
```

**Testing**:
- Pop out a Blue-linked chart → change symbol on another Blue chart → floating chart updates
- Click [S] on floating chart → picker appears, select Red → floating chart joins Red group
- Pop-in a floating chart → link state preserved
- Adopt-on-join: floating chart joins Blue group → adopts Blue group's symbol
- Verify link buttons appear on floating chart title bars

**Done when**: Floating charts have fully functional link buttons. Pop-out preserves link state. Link changes on floating charts trigger adopt-on-join. Propagation reaches floating charts bidirectionally.

---

### Dependency Summary

```
Slice 1 (Core Types)
    ↓
Slice 2 (State Integration)
    ↓
Slice 3 (Propagation Engine)
    ↓
Slice 4 (UI Controls)
    ↓
Slice 5 (Floating + Polish)
```

All slices are sequential — each builds on the previous. No parallelization opportunities within slices (small feature, linear dependency chain).

**Critical path**: Slice 1 → 2 → 3 → 4. Slice 3 is the core logic. Slice 5 is nice-to-have polish.

---

## Risks & Unknowns

### Known Risks

1. **Picker Z-order / overlap**: The color picker renders as a `Stack` layer in the pane body, not inside the title bar. This avoids expanding the title bar but may obscure chart content. **Mitigation**: Picker is dismissed on any click outside it (Escape, focus change, selection). Brief overlap is acceptable. If clipping at pane edges is severe, upgrade to iced's overlay system in a follow-up.

2. **Floating chart dual iteration**: Floating charts are keyed by `window::Id`, not `ChartId`. Propagation methods iterate `self.floating_charts` separately from `self.charts`, and floating charts need their own Message variants (`FloatingSetSymbolLink`, etc.). **Mitigation**: The `find_link_targets` pure function handles docked charts; floating chart iteration is explicit in the propagation methods. Some duplication is acceptable for 2 propagation methods + 3 floating message handlers.

3. **Config backward compatibility**: Adding `LinkMode` fields to `ChartConfig`. **Mitigation**: `LinkMode` derives `Default` (= `Unlinked`), and `#[serde(default)]` on both fields ensures old configs without these fields load correctly. Verified by unit test.

4. **Async task discarding**: `propagate_symbol_change` collects `Task<Message>` returns from `load_symbol_for_chart` and batches them. Today these are `Task::none()`, but when IB API integration arrives (Phase 1), they will be real async tasks. **Mitigation**: The propagation method returns `Task::batch(tasks)` which the caller must return from the update handler. This is future-proof.

### Unknowns

1. **iced 0.14 button style closures**: The [S] and [T] buttons use dynamic background colors via style closures. Verify that iced 0.14's `button::style()` accepts closures that capture runtime values (color arrays). If not, pre-define a fixed set of 10 button styles (one per LinkMode variant). **Time to validate**: 15 minutes during Slice 4.

2. **Picker Stack positioning**: The 15-minute spike at the start of Slice 4 validates that `Stack` layering in the pane body works for overlaying the picker above the chart shader widget. **Fallback**: Render picker as a column at the top of the pane body, pushing chart content down.

3. **Title bar space pressure**: With [S] and [T] buttons added, the title bar contains: ticker input (70px) + S button (~22px) + 7 timeframe buttons (~180px) + T button (~22px) + 4 toggle buttons (~90px) + pop-out + close. Total: ~400px+. Panes narrower than ~450px may overflow. **Mitigation**: Test at narrow widths. If overflow occurs, the link buttons could move into the controls (right) area of the title bar, or use `Responsive` to hide them below a threshold.

### Dependencies

- No new external crate dependencies. All implementation uses existing iced 0.14 widgets + serde.
- No dependency on the watchlist panel feature — chart linking is self-contained. When watchlist is added later, it simply calls the same `propagate_symbol_change()`.

---

## Testing Strategy

- **Unit tests (midas-core)**: `LinkColor`/`LinkMode` serde roundtrip, equality, defaults — following `id.rs` test pattern.
- **Unit tests (midas-app/link.rs)**: `find_link_targets()` pure function — color matching, ListenAll behavior, empty results for Unlinked/ListenAll sources. No `MidasApp` scaffolding needed.
- **Unit tests (midas-app)**: Picker state machine (`link_picker_open` transitions) — toggle, dismiss, auto-dismiss on selection/focus/escape.
- **Config roundtrip tests**: TOML serialize/deserialize with and without link fields, following existing `config.rs` test pattern.
- **Manual integration testing**: Multi-chart layout with various link configurations — verify symbol/timeframe sync, keyboard shortcut propagation (1-7 keys), floating chart behavior, adopt-on-join, picker UX.
- **Regression**: `cargo test --workspace` must pass at every slice boundary.

---

## Out of Scope

| Item | Why Excluded | Reconsider When |
|---|---|---|
| Watchlist panel integration | Watchlist doesn't exist yet | After watchlist panel is implemented |
| Crosshair sync by link group | Currently syncs by symbol; link-group crosshair is a separate concern | User requests it post-v1 |
| Link groups in layout presets | Presets create fresh charts; link state is user-configured | If users want "linked preset" templates |
| Multi-workspace link scope | NinjaTrader supports cross-workspace globals | Multiple workspace support is added |
| Keyboard shortcut for link toggle | Infrequently changed setting | User demand |
| Link group indicator in status bar | e.g., "Blue: AAPL, Red: MSFT" summary | After v1 ships, if users want global visibility |
| Undo/redo for link changes | Full undo system is Phase 8 of widget plan | Widget system Phase 8 |
| Annotation sync by link group | Annotations already sync by symbol (AnnotationStore) | If users want cross-symbol annotation sharing |
