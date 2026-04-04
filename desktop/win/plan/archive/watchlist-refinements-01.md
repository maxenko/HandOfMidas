# Watchlist Refinements — Phase 1

## Goal

Transform the watchlist from a static list into an interactive, professional-grade ticker grid with row selection, symbol linking, grid lines, and resizable columns.

## Current State

The watchlist is a manual `Row`/`Column` layout with 7 fixed-width columns (drag grip 26px, fav 30px, ticker 70px, price 80px, chg% 65px, G.ATR 70px, delete 30px). No row selection, no grid lines, no column resizing. Tickers are added/removed/favorited with instant persistence. Drag-drop onto chart panes already works.

**Existing infrastructure to leverage:**
- `find_link_targets<K, I>(source_link, panels) -> Vec<K>` — generic over key type, reusable for watchlist→chart propagation
- `load_symbol_for_chart()` / `load_data_for_floating_chart()` — proven data loading pipeline
- `LinkMode` / `LinkColor` — 8-color linking system already working for chart↔chart
- `build_link_picker(dimension, msg_builder)` — unified link picker dropdown, reusable for watchlist
- `mouse_area` — used in views.rs for click/scroll interception (link picker backdrop, scroll handling)
- `mouse_area.interaction()` — sets cursor type on hover (verified: exists in iced_widget 0.14.2)

**Framework options evaluated:**
- `iced::widget::table` (built-in 0.14) — has `.separator_x()` / `.separator_y()` for grid lines, but `Style` struct only exposes separator colors — no per-row or per-cell background styling, so selection highlight is impossible without cell-level container wrappers
- `iced_table` (tarkah, MIT, iced 0.13) — resize + selection + header click, but needs 0.13→0.14 port
- `iced_table2` (GPL-3.0, iced 0.14) — full-featured but GPL license is incompatible with proprietary app
- Custom build — most control, uses existing codebase patterns

**Decision: Custom build using iced primitives with container borders for grid lines.**

Each cell wrapped in a `container` with subtle border styling. This gives full control over per-row backgrounds (selection highlight), per-cell borders (grid lines), and dynamic widths (resize). Avoids external dependencies and license risk. Estimated ~400 lines across 3 slices.

---

## Slice 1: Row Selection + Symbol Linking

**What:** Clicking a ticker row selects it (highlighted background). The selected ticker's symbol is broadcast to all linked charts via the existing link infrastructure. This makes the watchlist the primary symbol navigation tool.

### State Changes

**watchlist.rs — `WatchlistPanel`:**
```rust
/// Currently selected ticker symbol, if any (transient, not persisted).
pub selected_symbol: Option<String>,

/// Symbol link group for watchlist→chart symbol propagation.
pub symbol_link: LinkMode,
```

**config.rs — `WatchlistConfig`:**
```rust
/// Symbol link mode for cross-chart symbol synchronization.
#[serde(default)]
pub symbol_link: LinkMode,
```
`#[serde(default)]` is required for backward compatibility — existing configs
without this field will deserialize to `LinkMode::Unlinked`.

**app.rs — `Message` enum:**
```rust
/// User clicked a ticker row in a watchlist.
WatchlistTickerSelected(WatchlistId, String),
/// Set the symbol link mode for a watchlist panel.
WatchlistSetSymbolLink(WatchlistId, LinkMode),
```

### Update Handler

Note: `propagate_symbol_change()` cannot be reused here — it takes a
`ChartId` source and looks it up in `self.charts`. The watchlist handler
calls `find_link_targets` directly with its own `symbol_link` mode.

```rust
Message::WatchlistTickerSelected(wl_id, symbol) => {
    if let Some(wl) = self.watchlists.get_mut(&wl_id) {
        wl.selected_symbol = Some(symbol.clone());
    }

    // Propagate to linked charts using watchlist's own link mode.
    let wl_link = self.watchlists.get(&wl_id)
        .map(|wl| wl.symbol_link)
        .unwrap_or(LinkMode::Unlinked);

    // Docked charts.
    let targets: Vec<ChartId> = find_link_targets(
        wl_link,
        self.charts.iter().map(|(id, p)| (*id, p.symbol_link)),
    );
    let mut tasks = Vec::new();
    for id in targets {
        tasks.push(self.load_symbol_for_chart(id, &symbol));
    }

    // Floating charts.
    let floating_targets: Vec<window::Id> = find_link_targets(
        wl_link,
        self.floating_charts.iter().map(|(wid, p)| (*wid, p.symbol_link)),
    );
    for wid in floating_targets {
        let tf = self.floating_charts.get(&wid)
            .map(|c| c.timeframe).unwrap_or(Timeframe::D1);
        if let Some(chart) = self.floating_charts.get_mut(&wid) {
            chart.symbol = symbol.clone();
            chart.symbol_input = symbol.clone();
        }
        self.load_data_for_floating_chart(wid, &symbol, tf);
    }

    if tasks.is_empty() { Task::none() } else { Task::batch(tasks) }
}

Message::WatchlistSetSymbolLink(wl_id, mode) => {
    self.link_picker_open = None;
    if let Some(wl) = self.watchlists.get_mut(&wl_id) {
        wl.symbol_link = mode;
    }
    self.flush_config()
}
```

### View Changes

**views.rs — `view_watchlist_body`:**

Use `mouse_area.on_release` (not `on_press`) for row selection. `on_press`
calls `shell.capture_event()` which prevents the parent `scrollable` from
receiving click events, breaking click-drag scrolling. `on_release` does
NOT capture, so scrolling works normally and selection fires on release
(standard desktop list behavior).

```rust
let is_selected = wl.selected_symbol.as_deref() == Some(&ticker.symbol);
let row_bg = if is_selected {
    Color::from_rgba(0.2, 0.35, 0.55, 0.6) // blue highlight
} else {
    Color::TRANSPARENT
};

let ticker_row = mouse_area(
    container(
        row![ /* existing columns */ ]
    )
    .style(move |_| container::Style {
        background: Some(row_bg.into()),
        ..Default::default()
    })
)
.on_release(Message::WatchlistTickerSelected(wl_id, ticker.symbol.clone()));
```

**Watchlist title bar — S link button:**

Add an S link button to the watchlist pane title bar (in `view_content`'s
Watchlist branch), using the existing `build_link_picker` with a closure
that emits `WatchlistSetSymbolLink`:

```rust
PanelContent::Watchlist(wl_id) => {
    let wl_link = self.watchlists.get(&wl_id)
        .map(|wl| wl.symbol_link).unwrap_or(LinkMode::Unlinked);
    let link_color = link_mode_indicator_rgba(wl_link);
    let s_btn = button(text("S").size(10).color(Color::WHITE).font(bold_font))
        .on_press(Message::ToggleLinkPicker(
            PickerTarget::Watchlist(wl_id),  // new PickerTarget variant
            LinkDimension::Symbol,
        ))
        .padding([2, 5])
        .style(/* colored background from link_color */);

    // Title bar: [Watchlist] [S] [spacer] [X]
    let tb = pane_grid::TitleBar::new(
        row![text("Watchlist").size(14), s_btn, Space::new().width(Fill)]
    ).controls(/* close button */);
}
```

**Note:** Requires adding `PickerTarget::Watchlist(WatchlistId)` variant to
the `PickerTarget` enum in `link.rs`, and handling it in the picker overlay
rendering (same pattern as `PickerTarget::Docked`).

### Persistence

- `watchlist.rs` `to_config()` — serialize `symbol_link`
- `watchlist.rs` `from_config()` — restore `symbol_link`
- `selected_symbol` is NOT persisted (transient)

### Files Changed
- `watchlist.rs` — add `selected_symbol`, `symbol_link` fields, update to/from_config
- `link.rs` — add `PickerTarget::Watchlist(WatchlistId)` variant
- `app.rs` — add `WatchlistTickerSelected`, `WatchlistSetSymbolLink` messages + handlers
- `views.rs` — row selection highlight (on_release), S link button in watchlist title bar, picker overlay for watchlist
- `config.rs` — add `#[serde(default)] pub symbol_link: LinkMode` to `WatchlistConfig`

### Tests
- `watchlist.rs`: selecting/deselecting ticker, `selected_symbol` not in `to_config()`, `symbol_link` roundtrip
- `config.rs`: watchlist config roundtrip with `symbol_link` field, backward compat (missing field defaults to Unlinked)

---

## Slice 2: Subtle Grid Lines

**What:** Add horizontal and vertical separator lines between rows and columns for a professional data grid appearance.

### Approach: Container Borders

Each cell wrapped in a `container` with subtle border. Header row gets a
stronger bottom border to visually separate it from data.

```rust
fn grid_cell<'a>(content: Element<'a, Message>, width: f32) -> Element<'a, Message> {
    container(content)
        .width(width)
        .padding([2, 4])
        .style(|_| container::Style {
            border: iced::Border {
                color: Color::from_rgba(1.0, 1.0, 1.0, 0.06),
                width: 1.0,
                radius: 0.0.into(),
            },
            ..Default::default()
        })
        .into()
}

fn grid_header_cell<'a>(content: Element<'a, Message>, width: f32) -> Element<'a, Message> {
    container(content)
        .width(width)
        .padding([2, 4])
        .style(|_| container::Style {
            border: iced::Border {
                color: Color::from_rgba(1.0, 1.0, 1.0, 0.12),
                width: 1.0,
                radius: 0.0.into(),
            },
            ..Default::default()
        })
        .into()
}
```

Apply `grid_header_cell` to header row items, `grid_cell` to data row items.
The container border gives all 4 sides — adjacent cells share borders,
producing a clean grid appearance.

### Files Changed
- `views.rs` — `grid_cell()` / `grid_header_cell()` helpers, updated `view_watchlist_body`

### Tests
- Visual only — no logic tests needed

---

## Slice 3: Resizable Columns

**What:** Users can drag the divider between column headers to resize columns. Widths persist across sessions.

### State

**watchlist.rs — `WatchlistPanel`:**
```rust
/// Column widths in logical pixels. Indexed by column position.
/// Defaults: [26, 30, 70, 80, 65, 70, 30]
pub column_widths: Vec<f32>,
```

Persisted in config for session restore.

**app.rs — transient drag state in `MidasApp`:**
```rust
/// Active column resize: (watchlist_id, column_index, start_x, original_width).
pub resizing_column: Option<(WatchlistId, usize, f32, f32)>,
```

**Messages:**
```rust
/// User started dragging a column divider.
WatchlistColumnResizeStart(WatchlistId, usize, f32),
/// User is dragging — cursor at this x position.
WatchlistColumnResizing(f32),
/// User released the drag.
WatchlistColumnResizeEnd,
```

### View: Resize Handles + Global Drag Overlay

**Handle placement:** Between each header cell, render a narrow `mouse_area`
(4px wide, full header height) as the drag initiation target:

```rust
mouse_area(Space::new().width(4).height(Fill))
    .interaction(mouse::Interaction::ResizingHorizontally)
    .on_press(Message::WatchlistColumnResizeStart(wl_id, col_idx, cursor_x))
```

**Global drag overlay:** The 4px handle only initiates the drag. Once the
user starts dragging, the cursor immediately leaves the handle. To keep
receiving mouse events during the drag, push a **full-pane invisible
`mouse_area` overlay** (same pattern as the link picker backdrop at
views.rs:857):

```rust
// When resizing_column is Some, render a full-pane overlay:
if self.resizing_column.is_some() {
    chart_layers.push(  // or watchlist_layers
        mouse_area(Space::new().width(Fill).height(Fill))
            .interaction(mouse::Interaction::ResizingHorizontally)
            .on_move(|point| Message::WatchlistColumnResizing(point.x))
            .on_release(Message::WatchlistColumnResizeEnd)
            .into(),
    );
}
```

This overlay captures all mouse movement and release events regardless of
cursor position, then removes itself when the drag ends. The cursor stays
as `ResizingHorizontally` throughout the drag.

### Update Handlers

```rust
WatchlistColumnResizeStart(wl_id, col, x) => {
    let width = self.watchlists.get(&wl_id)
        .map(|wl| wl.column_widths[col])
        .unwrap_or(70.0);
    self.resizing_column = Some((wl_id, col, x, width));
    Task::none()
}

WatchlistColumnResizing(current_x) => {
    if let Some((wl_id, col, start_x, orig_w)) = self.resizing_column {
        let delta = current_x - start_x;
        let new_w = (orig_w + delta).max(20.0); // minimum 20px
        if let Some(wl) = self.watchlists.get_mut(&wl_id) {
            wl.column_widths[col] = new_w;
        }
    }
    Task::none()
}

WatchlistColumnResizeEnd => {
    self.resizing_column = None;
    self.flush_config() // persist widths
}
```

### Config Persistence

Add `column_widths: Vec<f32>` to `WatchlistConfig` with `#[serde(default)]`.
Default to `[26.0, 30.0, 70.0, 80.0, 65.0, 70.0, 30.0]`.

### Files Changed
- `watchlist.rs` — add `column_widths` field, default values
- `app.rs` — add `resizing_column` state, 3 new messages + handlers
- `views.rs` — dynamic widths from state, resize handles between header cells, global drag overlay
- `config.rs` — add `column_widths` to `WatchlistConfig`

### Tests
- `watchlist.rs`: column_widths config roundtrip, default values, minimum width clamping
- `config.rs`: watchlist config roundtrip with column_widths

---

## Slice Dependency Graph

```
Slice 1 (selection + linking)     ← independent
Slice 2 (grid lines)              ← independent
Slice 3 (resizable columns)       ← independent
```

All three slices are independent and can be developed in parallel.
Resize handles (Slice 3) are interleaved between header cells regardless
of whether cells have border styling (Slice 2). Row selection backgrounds
(Slice 1) are per-row container styles independent of cell borders.

## Risk Notes

- **`on_release` inside `scrollable`**: Verified that `mouse_area.on_release` does NOT call `capture_event()`, so scroll-drag propagation is preserved. Wheel scrolling is unaffected regardless.
- **Resize overlay z-ordering**: The full-pane overlay must be the topmost layer during resize to receive all mouse events. Use the same stack/layer pattern as the link picker backdrop.
