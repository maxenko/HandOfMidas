# Grid Component Rendering Specification

```
Document:   02-rendering.md
Component:  Grid/Table widget for Hand of Midas watchlists
Authors:    Planning phase
Status:     Draft
```

## Table of Contents

1. [Rendering Strategy Decision](#1-rendering-strategy-decision)
2. [Layer Architecture](#2-layer-architecture)
3. [Virtual Scrolling](#3-virtual-scrolling)
4. [Flash-on-Tick Animation](#4-flash-on-tick-animation)
5. [Conditional Cell Formatting](#5-conditional-cell-formatting)
6. [Drag Visuals](#6-drag-visuals)
7. [Column Resize Visual](#7-column-resize-visual)
8. [Performance Budget](#8-performance-budget)
9. [Theme Integration](#9-theme-integration)
10. [iced Widget Implementation](#10-iced-widget-implementation)

---

## 1. Rendering Strategy Decision

### Option D: iced 0.14 Built-in `Table` Widget (Rejected)

iced 0.14 ships a basic `Table` widget at `iced_widget::table`. It provides columns, rows, cell elements, padding, and separators. However, it lacks virtual scrolling, fixed headers, column resize, column reorder, sort support, row selection, and drag-and-drop — all hard requirements for a trading grid. Building on top of it would require replacing most of its internals. **Rejected**: it is simpler to build the grid from composable iced primitives than to fight the built-in table's limitations.

### Option A: Pure iced Widgets

Compose the grid entirely from iced's widget tree: `Container`, `Row`, `Column`, `Text`, `Button`, `Scrollable`, etc. This is the approach used by the current `view_watchlist_body()` in `crates/midas-app/src/app/views.rs`.

**How it works today**: Each row is a `Row` of cell containers interleaved with 4px spacer widgets for resize handles. The header is a separate `Row`. The body is wrapped in a `Scrollable`. Selection is a `mouse_area` with a styled `Container` background. Column resize uses a `stack` overlay with `mouse_area` tracking `on_move`.

| Criterion | Assessment |
|---|---|
| Arbitrary cell content (text, buttons, toggles, icons) | Excellent. Any iced widget can be a cell. |
| Performance at 100 rows | Good. iced rebuilds the Element tree each frame but diffing is efficient for small counts. |
| Performance at 1000+ rows | Poor. iced has no built-in virtual scrolling. `Scrollable` renders all children. 1000 rows with 7 columns = 7000+ widget nodes. |
| Flash-on-tick animation | Awkward. Must use `iced::time::every()` subscription + per-cell background color state. No GPU shader access. |
| Drag overlays | Possible via `stack` + absolute positioning, but iced lacks a true z-ordered overlay layer outside the widget tree bounds. |
| Text rendering quality | Excellent. iced uses `cosmic-text` / `glyphon` for high-quality text. |
| Development speed | Fastest for Phase 1. Uses well-understood patterns already in the codebase. |

### Option B: Custom wgpu Pipeline

Draw the grid directly with GPU pipelines, analogous to `ChartRenderer` in `midas-render`. The grid body becomes a `shader::Program` widget. All rendering (backgrounds, lines, text, selection) is done via custom wgpu render passes.

| Criterion | Assessment |
|---|---|
| Arbitrary cell content | Poor. Custom widgets (buttons, toggles, dropdowns) cannot be embedded in a raw GPU pipeline. Would require reimplementing every interactive element. |
| Performance at 1000+ rows | Excellent. GPU instanced rendering is trivially fast for rectangle grids. Virtual scrolling is natural (emit quads only for visible rows). |
| Flash-on-tick animation | Excellent. Flash is a per-cell uniform decaying over time in the fragment shader. Zero CPU cost per cell. |
| Drag overlays | Excellent. Full control over layer ordering. Drag ghost is just another render pass. |
| Text rendering | Requires building a text atlas pipeline (MSDF or glyph rasterization). Significant investment. `midas-render` does not currently render text -- all text labels are iced overlays. |
| Development speed | Slowest. Must build text rendering, hit testing, focus management, accessibility, and every interactive widget from scratch. |

### Option C: Hybrid (Recommended)

Use iced widgets for cell content and header elements. Use custom drawing (via iced's Canvas widget or a lightweight custom pipeline) only for:
- Grid lines (horizontal/vertical separators)
- Selection highlight backgrounds
- Flash-on-tick animation overlays
- Drag ghost and drop indicator overlays
- Row stripe backgrounds

The cell content layer remains pure iced widgets (`Text`, `Button`, etc.), inheriting iced's text rendering, hit testing, and accessibility.

| Criterion | Assessment |
|---|---|
| Arbitrary cell content | Excellent. Cells are iced `Element` trees. |
| Performance at 1000+ rows | Good with application-level windowing. The widget only renders visible rows (computed in `view()`). |
| Flash-on-tick animation | Good. Flash overlay can be a colored `Container` background with alpha interpolation driven by `iced::time::every()`. For 100+ simultaneous flashes, the per-frame cost is one background color computation per flashing cell (cheap). |
| Drag overlays | Good. iced's `overlay()` method on the `Widget` trait provides a proper z-ordered layer for drag ghosts and context menus. |
| Text rendering | Excellent. Inherited from iced. |
| Development speed | Moderate. Builds on existing patterns. |

### Decision: Option C -- Hybrid, iced-dominant

**Justification**:

1. **The current codebase already uses this pattern successfully.** The chart widget uses `shader::Program` for GPU rendering while iced overlays handle text labels (date axis, price axis, OHLCV overlay). The watchlist already works as pure iced widgets. The grid component follows the same split.

2. **Text rendering is the deciding factor.** `midas-render` does not have a text pipeline. Building one (MSDF atlas, glyph layout, line breaking, tabular figures) is a multi-week project. iced's `cosmic-text` integration handles all of this. For a data grid where every cell contains text, leveraging iced's text is non-negotiable.

3. **Arbitrary cell content is a hard requirement.** Watchlist cells contain buttons (favorite toggle, delete, drag handle), styled text with conditional coloring, and future interactive elements (inline order entry). These are natural iced widgets. Reimplementing them in a GPU pipeline provides no benefit.

4. **Virtual scrolling at the application level is sufficient.** The watchlist use case has 50-500 rows. Application-level windowing (computing the visible slice in `view()`) keeps the widget tree small. For the order history use case (10,000+ rows), the same pattern scales -- the `view()` function simply passes a smaller slice.

5. **Flash animation does not require a GPU shader.** At 60fps, interpolating a background color in Rust (`Color::from_rgba(r, g, b, alpha * ease_out(t))`) and setting it as a `Container` style is negligible cost. The 100-cell flash scenario costs ~100 `f32` multiplications per frame. The overhead of a custom shader pipeline for this alone is unjustified.

6. **Drag overlays map directly to iced's `overlay()` mechanism.** The `Widget::overlay()` method returns an `Element` that renders in a separate layer above all sibling widgets. This is exactly the WPF AdornerLayer pattern identified in the research.

**Phase 2 escape hatch**: If profiling reveals that iced widget tree rebuilds become a bottleneck at 1000+ visible rows, a `Canvas` or `shader::Program` can replace the body rendering while keeping the header and overlay as iced widgets. The layer architecture (Section 2) is designed to support this migration without changing the external API.

---

## 2. Layer Architecture

The grid renders in 7 conceptual layers, ordered back-to-front. In the hybrid approach, most layers are iced widget styling (backgrounds, borders) and iced overlays.

```
Z-order (back to front):

  +------------------------------------------+
  | Layer 0: Row Background                  |  Alternating stripes, selected row fill
  +------------------------------------------+
  | Layer 1: Grid Lines                      |  Column separators, row dividers
  +------------------------------------------+
  | Layer 2: Cell Content                    |  Text, buttons, icons (iced widgets)
  +------------------------------------------+
  | Layer 3: Flash Overlay                   |  Semi-transparent color wash per cell
  +------------------------------------------+
  | Layer 4: Selection Overlay               |  Focused row border, range highlight
  +------------------------------------------+
  | Layer 5: Header (fixed)                  |  Column headers, sort indicators, resize handles
  +------------------------------------------+
  | Layer 6: Drag Overlay                    |  Ghost row/header, drop indicator (TOPMOST)
  +------------------------------------------+
```

### Layer 0: Row Background

**What is drawn**: Alternating row stripes (even rows get a slightly lighter background). The selected row gets a distinct highlight fill.

**How it is drawn**: Each row is a `Container` with a `style` closure that returns a `container::Style` with the appropriate `background`. The closure receives the row index and selection state as captured variables.

```rust
fn row_background(row_index: usize, is_selected: bool, is_hovered: bool) -> container::Style {
    let base = if row_index % 2 == 0 {
        Color::from_rgba(0.10, 0.10, 0.13, 1.0) // even rows
    } else {
        Color::from_rgba(0.09, 0.09, 0.11, 1.0) // odd rows (matches BACKGROUND)
    };

    let bg = if is_selected {
        Color::from_rgba(0.18, 0.32, 0.55, 0.7) // selection blue
    } else if is_hovered {
        Color::from_rgba(0.14, 0.14, 0.18, 1.0) // hover highlight
    } else {
        base
    };

    container::Style {
        background: Some(iced::Background::Color(bg)),
        ..Default::default()
    }
}
```

**Z-ordering**: Lowest layer (rendered first). The container background paints behind all child widgets.

**Dirty conditions**: Redraw when `selected_row` changes, `hovered_row` changes, or theme changes.

### Layer 1: Grid Lines

**What is drawn**: Thin vertical lines between columns (1px, very subtle alpha). Optional thin horizontal lines between rows.

**How it is drawn**: CSS-style approach using `Container` borders. Each cell container has a right border for the column separator. The header row has a bottom border. Row separators are bottom borders on each row container.

```rust
fn cell_style(is_last_column: bool) -> container::Style {
    container::Style {
        border: iced::Border {
            color: Color::from_rgba(1.0, 1.0, 1.0, 0.06), // subtle grid line
            width: if is_last_column { 0.0 } else { 1.0 },
            radius: 0.0.into(),
        },
        ..Default::default()
    }
}
```

**Z-ordering**: Rendered as part of the Container border, which paints between background (Layer 0) and content (Layer 2).

**Dirty conditions**: Redraw when column widths change, columns reorder, or theme changes.

### Layer 2: Cell Content

**What is drawn**: The actual cell data -- `Text` widgets for prices/percentages, `Button` widgets for actions (favorite, delete, drag handle), future widgets like sparkline `Canvas` or status icons.

**How it is drawn**: Each cell is an iced `Element` returned by the column's `cell()` method (see canonical trait in 03-column-data-model.md §1.1):

```rust
trait GridColumn<T, Message> {
    fn cell<'a>(&'a self, row: &'a T, row_index: usize) -> Element<'a, Message>;
}
```

Cell-specific context (selection state, flash state) is handled externally by the grid widget via container styling, not passed to the column. This keeps the column trait simple and the flash/selection logic in the grid where it belongs.

**Z-ordering**: Paints on top of the container background and border.

**Dirty conditions**: Redraw when row data changes, scroll position changes (different rows visible), or column definitions change.

### Layer 3: Flash Overlay

**What is drawn**: A semi-transparent color wash over individual cells that have recently received a data update. Green for price increase, red for decrease. Fades out over 300ms with ease-out interpolation.

**How it is drawn**: Two implementation strategies, chosen per-cell:

**Strategy A (Container background modulation)**: Instead of a separate overlay, the cell's background color incorporates the flash. The `row_background()` function blends the flash color with the base background. This avoids an extra widget layer.

```rust
fn blended_cell_background(
    base: Color,
    flash: Option<&FlashState>,
    now: Instant,
) -> Color {
    match flash {
        None => base,
        Some(f) => {
            let elapsed = now.duration_since(f.start_time).as_secs_f32();
            let t = (elapsed / f.duration_secs).clamp(0.0, 1.0);
            let alpha = (1.0 - ease_out_cubic(t)) * f.peak_alpha;
            lerp_color(base, f.color, alpha)
        }
    }
}
```

**Strategy B (Overlay Container)**: For cells where the background is already complex (e.g., selected + gradient), layer a semi-transparent `Container` on top using `stack![]`. This is more expensive (extra widget node) and reserved for edge cases.

**Z-ordering**: Conceptually above the base background, below cell content. In Strategy A, it modifies the background color. In Strategy B, it is a `stack` layer between background and content.

**Dirty conditions**: Redraw continuously (every frame) while any flash is active. When all flashes have expired, stop requesting redraws. The grid widget tracks `flash_active_count` and only subscribes to `time::every(Duration::from_millis(16))` when `flash_active_count > 0`.

### Layer 4: Selection Overlay

**What is drawn**: A highlight border around the focused/selected row. For multi-select (future), a translucent blue wash over the range. The current design uses background fill (Layer 0), but a border accent provides stronger visual separation.

**How it is drawn**: The selected row's `Container` gets an additional left-border accent (2px, accent blue) or a full border. This is part of the `row_background()` style:

```rust
if is_selected {
    container::Style {
        background: Some(Color::from_rgba(0.18, 0.32, 0.55, 0.7).into()),
        border: iced::Border {
            color: Color::from_rgb(0.22, 0.55, 0.95), // ACCENT
            width: 2.0,
            radius: 2.0.into(),
        },
        ..Default::default()
    }
}
```

**Z-ordering**: Same as Layer 0 (part of the container style). The border paints around the container bounds.

**Dirty conditions**: Redraw when `selected_row`, `focused_row`, or selection set changes.

### Layer 5: Header (Fixed)

**What is drawn**: Column headers with:
- Column title text
- Sort direction indicator (triangle up/down)
- Resize handle hit areas (invisible, wider than visual)
- Column reorder drag initiation zones

**How it is drawn**: A `Row` of header cell widgets. The header is positioned outside the scrollable body so it remains fixed during vertical scroll. Column widths are shared between header and body via the `GridState.column_widths: HashMap<ColumnId, f32>` map.

```
+-----------------------------------------------------------+
| [Header Row - fixed position, not scrollable]             |
|   [Drag] [Fav] [Ticker v] [Price] [Chg% ^] [G.ATR] [X]  |
+-----------------------------------------------------------+
| [Scrollable Body]                                          |
|   [Row 0]                                                  |
|   [Row 1]                                                  |
|   ...                                                      |
+-----------------------------------------------------------+
```

The header cell for a sortable column:
```rust
fn header_cell(
    title: &str,
    sort_state: Option<SortDirection>,
    col_id: ColumnId,
    width: f32,
) -> Element<'_, Message> {
    let indicator = match sort_state {
        Some(SortDirection::Ascending) => " \u{25B2}",
        Some(SortDirection::Descending) => " \u{25BC}",
        None => "",
    };
    let label = text(format!("{title}{indicator}")).size(12);
    let btn = button(label)
        .on_press(on_msg(GridMessage::SortToggled(col_id)))
        .padding(0)
        .style(header_button_style);

    container(btn)
        .width(width)
        .height(HEADER_HEIGHT)
        .align_y(iced::Alignment::Center)
        .padding([0, CELL_PADDING_H])
        .into()
}
```

Resize handles are 4-8px invisible `mouse_area` widgets interleaved between header cells, matching the current implementation pattern in `view_watchlist_body()`.

**Z-ordering**: Above the scrollable body. Implemented by placing the header `Row` before the body `Scrollable` in a `Column` layout (iced paints children in order).

**Dirty conditions**: Redraw when sort column/direction changes, column widths change, columns reorder, or header drag starts.

### Layer 6: Drag Overlay

**What is drawn**: During an active drag operation:
- **Drag ghost**: A semi-transparent copy of the row or header being dragged, following the cursor.
- **Drop indicator**: A 2px colored horizontal line (row drag) or vertical line (column drag) at the insertion point.
- **Source dimming**: The original item dims to 30% opacity.

**How it is drawn**: Using iced's `Widget::overlay()` method. The grid widget returns an `overlay::Element` that renders a positioned drag ghost and drop indicator on top of all other content.

```rust
fn overlay<'a>(
    &'a mut self,
    tree: &'a mut Tree,
    layout: Layout<'_>,
    _renderer: &Renderer,
    translation: Vector,
    _viewport: &Rectangle,
) -> Option<overlay::Element<'a, Message, Theme, Renderer>> {
    let state = tree.state.downcast_ref::<GridWidgetState>();
    let drag = state.drag_state.as_ref()?;
    // iced 0.14: overlay::Element::new signature — verify exact API during
    // Pre-Phase 2 spike. The API may require a `position: Point` as the first
    // argument (for overlay placement) rather than handling position in layout().
    // If so, pass `drag.cursor_pos - drag.hotspot + translation` as position.
    Some(overlay::Element::new(Box::new(DragOverlay {
        ghost_content: drag.ghost_element(),
        ghost_position: drag.cursor_pos - drag.hotspot + translation,
        ghost_opacity: 0.8,
        drop_indicator: drag.drop_indicator(),
    })))
}
```

The `DragOverlay` is a custom overlay widget that:
1. Renders a drop shadow (subtle dark rectangle behind ghost)
2. Renders the ghost content at the cursor-following position with reduced opacity
3. Renders the drop indicator line at the computed insertion point
4. Does NOT intercept hit tests (pointer events pass through to the underlying grid for drop target detection)

**Z-ordering**: Highest layer. iced's overlay system renders overlays after all normal widget content, ensuring the drag ghost appears above everything including the header.

**Dirty conditions**: Redraw every frame while a drag is active (ghost follows cursor).

---

## 3. Virtual Scrolling

### Strategy: Application-Level Windowing

Since iced 0.14 does not provide built-in virtual scrolling and we are using Option C (iced-dominant hybrid), virtual scrolling is implemented at the application layer. The `view()` function computes the visible row range and only builds `Element` nodes for those rows.

### Row Height Model

**Fixed row height** (Phase 1). All data rows share a single height constant.

```rust
const ROW_HEIGHT: f32 = 28.0;       // data row height in logical pixels
const HEADER_HEIGHT: f32 = 30.0;    // header row height
const OVERSCAN_ROWS: usize = 5;     // extra rows above/below viewport
```

Fixed row height enables O(1) scroll-position-to-row-index mapping, matching the ImGuiListClipper pattern from the research.

Variable row heights (Phase 3, for expanded row details or grouped rows) would require a cumulative height array and binary search, following egui's `heterogeneous_rows()` approach.

### Visible Row Range Calculation

```rust
fn visible_row_range(
    scroll_offset: f32,
    viewport_height: f32,
    total_rows: usize,
    row_height: f32,
    overscan: usize,
) -> Range<usize> {
    let first_visible = (scroll_offset / row_height).floor() as usize;
    let visible_count = (viewport_height / row_height).ceil() as usize + 1;

    let start = first_visible.saturating_sub(overscan);
    let end = (first_visible + visible_count + overscan).min(total_rows);

    start..end
}
```

### Overscan

Render `OVERSCAN_ROWS` (5) extra rows above and below the viewport. This eliminates visual gaps during fast scrolling -- the extra rows are already rendered before they scroll into view.

At `ROW_HEIGHT = 28px` and 5 overscan rows, the extra rendering overhead is 10 rows * 7 columns = 70 widget nodes. Negligible.

### Integration with iced's Layout System

> **Pre-Phase 3 spike required** (2-4 hours): Before implementing virtual scrolling
> in Phase 3, validate that iced 0.14's `Scrollable` correctly handles spacer-based
> virtual scrolling. See 04-implementation-roadmap.md Phase 3 for spike details.
> Build a test with `Column[Space(top_spacer), 8 visible rows, Space(bottom_spacer)]`
> inside a `Scrollable` and verify scrollbar thumb size, scroll-to-position accuracy,
> and scroll event emission. If it fails, the grid must be a custom `Widget` from
> Phase 0 with internal scroll management (the fallback approach below).

**Phase 0-2 approach** (simple): Use iced's `Scrollable` wrapping the body `Column`.
For watchlists with <500 rows, render all rows — no virtual scrolling needed.

**Phase 3 approach** (virtual scrolling): Replace the body with a `Column` containing
`[Space(top_spacer), visible_rows, Space(bottom_spacer)]` inside a `Scrollable`
with an `on_scroll` callback that updates `GridState.scroll_y`. The spacer heights
are computed from `scroll_offset` and `total_rows * ROW_HEIGHT`.

**Fallback** (if Scrollable doesn't handle spacers well): The grid widget manages
its own scroll state with a custom `Widget` implementation:

```rust
struct GridScrollState {
    /// Vertical scroll offset in logical pixels.
    scroll_y: f32,
    /// Total content height (total_rows * ROW_HEIGHT).
    total_content_height: f32,
    /// Viewport height (from layout bounds).
    viewport_height: f32,
}
```

**Why not use iced's `Scrollable`?** The `Scrollable` widget renders all children and clips to the viewport. For virtual scrolling, we need to control which children exist. The grid widget:

1. Claims the full viewport height in `layout()`.
2. In `view()` (or `draw()`), computes the visible range and only emits `Element` nodes for those rows.
3. Positions visible rows with a vertical offset based on `(row_index * ROW_HEIGHT - scroll_y)`.
4. Handles `WheelScrolled` events in `update()` to modify `scroll_y`.
5. Renders a custom scrollbar indicator (a thin vertical track on the right edge).

The total scrollable extent is `total_rows * ROW_HEIGHT`. The scroll thumb position is `scroll_y / (total_content_height - viewport_height)`.

### Scroll Event Handling

```rust
fn handle_scroll(state: &mut GridState, delta_y: f32) {
    let max_scroll = (state.total_content_height - state.viewport_height).max(0.0);
    state.scroll_y = (state.scroll_y - delta_y * SCROLL_SPEED)
        .clamp(0.0, max_scroll);
}
```

`SCROLL_SPEED` is a multiplier (default 3.0) to convert mouse wheel ticks to pixel offset. On Windows, `delta_y` from iced is in logical units (typically 1.0 per tick for a notched wheel, fractional for smooth/precision scrolling).

### Scroll Position Persistence

The `scroll_y` is part of `GridState` and persisted across frames. When the data changes (rows added/removed, sort changes), the scroll position is clamped to the new valid range. After a sort, the scroll jumps to keep the previously-selected row visible:

```rust
fn adjust_scroll_after_sort(state: &mut GridState, selected_row: Option<usize>) {
    if let Some(row_idx) = selected_row {
        let row_top = row_idx as f32 * ROW_HEIGHT;
        let row_bottom = row_top + ROW_HEIGHT;
        if row_top < state.scroll_y {
            state.scroll_y = row_top;
        } else if row_bottom > state.scroll_y + state.viewport_height {
            state.scroll_y = row_bottom - state.viewport_height;
        }
    }
}
```

---

## 4. Flash-on-Tick Animation

### Trigger

A flash triggers when the **application** detects that a cell's underlying data value has changed. The application already maintains current and previous market data for each symbol, so it is the natural place to detect changes. For a trading watchlist:
- **Price cell**: app detects `last_price` changed and sends a flash trigger to the grid.
- **Change % cell**: app detects `change_pct` changed and sends a flash trigger.
- **Volume cell**: app detects `volume` changed (optional, can be noisy).

The flash direction (green/red) is determined by the application comparing the new value to the previous value, not the absolute sign. The grid receives the direction as part of the flash trigger and never inspects data values itself.

**Design principle**: Consistent with the headless core pattern, the grid does not track previous data values. The application detects value changes (it already has current and previous market data) and notifies the grid which cells to flash. The grid manages only the animation state.

### Visual Design

```
+--------------------------------------------------+
|  AAPL   189.50   +1.2%   1.2M   0.85   [x]     |
|          ^^^^^^                                   |
|          Flash: green background, fading out      |
+--------------------------------------------------+

Time 0ms:   Cell background = flash_green at 100% alpha
Time 100ms: Cell background = flash_green at ~60% alpha (ease-out)
Time 200ms: Cell background = flash_green at ~15% alpha
Time 300ms: Cell background = base color (flash complete)
```

### Flash State Data Structure

> **Canonical source**: This section is the canonical definition for `FlashState`
> and `FlashColor`. Other plan documents (04-implementation-roadmap.md) reference
> this section; when in conflict, this document takes precedence.
>
> **Implementation note**: The `FlashMap` wrapper struct shown below is illustrative —
> it groups the flash data and helper methods for exposition. The canonical runtime
> representation is a raw `HashMap<(RowKey, ColumnId), FlashState>` field on `GridState`
> (see 04-implementation-roadmap.md Phase 3a), with a `has_active_flashes()` method
> replacing the `active_count` field. The `FlashState` and `FlashColor` types are
> the implementation targets; the wrapper struct is not.

```rust
/// Tracks active flash animations for the entire grid.
struct FlashMap {
    /// Key: (row_key, column_id). Value: active flash state.
    /// Using a HashMap because flashes are sparse -- typically only a few
    /// cells flash simultaneously even during market open bursts.
    /// Uses `RowKey` (not `usize` index) so flashes survive re-sorts:
    /// after a price update triggers both a flash and a re-sort, the
    /// flash stays on the correct ticker rather than shifting to
    /// whatever row now occupies the old index.
    flashes: HashMap<(RowKey, ColumnId), FlashState>,
    /// Number of currently active flashes. When 0, no animation subscription needed.
    active_count: usize,
}

struct FlashState {
    /// When the flash started.
    start_time: Instant,
    /// Flash duration in seconds (0.3 for standard, configurable).
    duration_secs: f32,
    /// Flash color: green for increase, red for decrease.
    color: FlashColor,
    /// Peak alpha (starting opacity of the flash overlay).
    peak_alpha: f32,
}

enum FlashColor {
    /// Price/value increased.
    Up,
    /// Price/value decreased.
    Down,
}

impl FlashColor {
    fn to_rgba(&self) -> Color {
        match self {
            FlashColor::Up => Color::from_rgba(0.10, 0.75, 0.40, 1.0),   // green
            FlashColor::Down => Color::from_rgba(0.85, 0.20, 0.25, 1.0), // red
        }
    }
}
```

> **Ownership clarification**: The `FlashMap` described above is canonically owned as a
> field of `GridState` (see 04-implementation-roadmap.md Phase 3, which adds
> `flash_state: HashMap<(RowKey, ColumnId), FlashState>` to `GridState`). It is NOT a
> separate structure managed internally by the widget. The application detects price
> changes and sends `GridMessage::FlashCell { column, row_key, direction }` to trigger
> flashes; the grid manages only the animation lifecycle (timestamps, alpha decay) in
> `GridState.flash_state`. The grid does NOT maintain a `PreviousValues` map or perform
> any value comparison. The `FlashMap` name used here is a convenience alias for the
> hash map stored in `GridState`.

### Flash Triggering (App-Side Change Detection)

The grid does **not** maintain a `PreviousValues` shadow copy. Change detection is the application's responsibility. The app already holds both current and previous market data snapshots, so it compares values and tells the grid which cells to flash.

The app triggers flashes via `GridMessage::FlashCell`:

```rust
/// Sent by the application when it detects a value change.
enum GridMessage {
    /// Trigger a flash animation on a specific cell.
    /// Uses `RowKey` (not `usize` index) so flashes survive re-sorts.
    FlashCell {
        column: ColumnId,
        row_key: RowKey,
        direction: FlashDirection,
    },
    /// Fired every ~16ms while flashes are active to decay alpha.
    GridAnimationTick,
    // ... other messages
}

enum FlashDirection {
    Up,
    Down,
}
```

During `update()`, when the app processes a market data batch:
```rust
// In the application's update(), NOT inside the grid:
fn handle_market_data_update(
    &mut self,
    batch: &MarketDataBatch,
) -> Vec<GridMessage> {
    let mut messages = Vec::new();
    for update in &batch.updates {
        let symbol = &update.symbol;
        if let Some(prev) = self.previous_market_data.get(symbol) {
            // Price changed?
            if (update.last_price - prev.last_price).abs() > f64::EPSILON {
                let direction = if update.last_price > prev.last_price {
                    FlashDirection::Up
                } else {
                    FlashDirection::Down
                };
                let row_key = RowKey::new(symbol);
                messages.push(GridMessage::FlashCell {
                    column: ColumnId("price"),
                    row_key,
                    direction,
                });
            }
            // Change % changed? (same pattern)
        }
        // Update previous snapshot
        self.previous_market_data.insert(symbol.clone(), update.clone());
    }
    messages
}
```

When the grid receives `GridMessage::FlashCell`, it inserts into the flash map:
```rust
// In GridState::update():
GridMessage::FlashCell { column, row_key, direction } => {
    let key = (row_key, column);
    let color = match direction {
        FlashDirection::Up => FlashColor::Up,
        FlashDirection::Down => FlashColor::Down,
    };
    self.flash_map.flashes.insert(key, FlashState {
        start_time: Instant::now(),
        duration_secs: 0.3,
        color,
        peak_alpha: 0.45,
    });
    self.flash_map.active_count = self.flash_map.flashes.len();
}
```

### Animation Interpolation

```rust
fn flash_alpha(flash: &FlashState, now: Instant) -> f32 {
    let elapsed = now.duration_since(flash.start_time).as_secs_f32();
    let t = (elapsed / flash.duration_secs).clamp(0.0, 1.0);
    flash.peak_alpha * (1.0 - ease_out_cubic(t))
}

fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}
```

The flash color is blended with the cell's base background:

```rust
fn blended_background(base: Color, flash: &FlashState, now: Instant) -> Color {
    let alpha = flash_alpha(flash, now);
    if alpha < 0.005 {
        return base; // flash expired, avoid unnecessary blending
    }
    let flash_rgb = flash.color.to_rgba();
    Color::from_rgba(
        base.r * (1.0 - alpha) + flash_rgb.r * alpha,
        base.g * (1.0 - alpha) + flash_rgb.g * alpha,
        base.b * (1.0 - alpha) + flash_rgb.b * alpha,
        1.0,
    )
}
```

### Re-Trigger Behavior

If a cell receives a new tick while still flashing:
1. The new flash **replaces** the current flash immediately.
2. The `start_time` resets to `now`.
3. The direction may change (was green, now red).

This matches Bloomberg/TWS behavior: the newest tick always wins.

### Performance: 100+ Simultaneous Flashes

At market open, all rows may update simultaneously. With 200 visible rows and 3 flashable columns per row, up to 600 cells could flash at once.

**Cost per frame with 600 active flashes**:
- 600x `Instant::duration_since()`: ~600ns
- 600x `ease_out_cubic()`: ~200ns
- 600x `blended_background()`: ~1us
- Total: ~2us per frame. Negligible.

The flash map is iterated once per frame to compute backgrounds. Expired flashes (t >= 1.0) are removed during this pass:

```rust
fn tick_flashes(flash_map: &mut FlashMap, now: Instant) {
    flash_map.flashes.retain(|_, flash| {
        let elapsed = now.duration_since(flash.start_time).as_secs_f32();
        elapsed < flash.duration_secs
    });
    flash_map.active_count = flash_map.flashes.len();
}
```

### Animation Subscription

The grid subscribes to a 60fps timer only while flashes are active:

```rust
fn subscription(&self) -> iced::Subscription<Message> {
    if self.flash_map.active_count > 0 {
        iced::time::every(Duration::from_millis(16))
            .map(|_| Message::GridAnimationTick)
    } else {
        iced::Subscription::none()
    }
}
```

`Message::GridAnimationTick` triggers a view rebuild, which recomputes flash backgrounds for all visible cells. When the last flash expires, `active_count` drops to 0 and the subscription stops.

---

## 5. Conditional Cell Formatting

### Rule System

Conditional formatting rules determine text color, background color, or both based on cell value. Phase 1 implements hardcoded rules for trading data. Phase 2 will support user-configurable rules.

### Phase 1: Hardcoded Trading Rules

```rust
/// Compute the foreground (text) color for a cell based on its semantic type.
fn cell_text_color(col_type: &ColumnType, value: f64) -> Color {
    match col_type {
        ColumnType::ChangePercent | ColumnType::Change => {
            if value > 0.0 {
                Color::from_rgb(0.20, 0.80, 0.30)     // green
            } else if value < 0.0 {
                Color::from_rgb(0.90, 0.25, 0.20)     // red
            } else {
                Color::from_rgb(0.60, 0.60, 0.60)     // neutral gray
            }
        }
        ColumnType::Price => {
            // Prices are neutral colored; direction is shown by flash
            Color::from_rgb(0.88, 0.88, 0.92)         // TEXT_PRIMARY
        }
        ColumnType::GerchikATR => {
            // Color from computed GATR analysis (green/yellow/red gradient)
            // Passed through from the computation layer
            Color::from_rgb(0.60, 0.60, 0.60)         // default
        }
        _ => Color::from_rgb(0.88, 0.88, 0.92),       // TEXT_PRIMARY
    }
}
```

### Gradient-Based Coloring

For columns like "Change %" where magnitude matters, use gradient interpolation:

```rust
fn change_percent_color(pct: f64) -> Color {
    let magnitude = pct.abs().min(10.0); // cap at 10% for color scaling
    let intensity = (magnitude / 10.0) as f32;

    if pct > 0.0 {
        // Green: more intense for larger gains
        Color::from_rgb(
            0.20 * (1.0 - intensity) + 0.10 * intensity,
            0.50 + 0.30 * intensity,
            0.20 * (1.0 - intensity) + 0.10 * intensity,
        )
    } else {
        // Red: more intense for larger losses
        Color::from_rgb(
            0.50 + 0.40 * intensity,
            0.20 * (1.0 - intensity) + 0.10 * intensity,
            0.15 * (1.0 - intensity) + 0.10 * intensity,
        )
    }
}
```

### Caching Strategy

Conditional formatting is **computed per visible-cell per frame** during `view()`. With 50 visible rows and 7 columns = 350 cells, the cost is ~350 color computations per frame (~10us). No caching is needed for this scale.

For future user-configurable rules with complex predicates, a dirty flag per row will skip recomputation when data has not changed:

```rust
struct RowFormatCache {
    /// Generation counter -- incremented when row data changes.
    data_generation: u64,
    /// Cached format results, one per column.
    formats: Vec<CellFormat>,
    /// The generation when formats were last computed.
    computed_generation: u64,
}
```

---

## 6. Drag Visuals

### Overview

The grid supports three drag operations:
1. **Row drag reorder**: Drag a row by its grip handle to reorder within the watchlist.
2. **Column header drag reorder**: Drag a column header to reorder columns.
3. **Ticker drag to chart**: Drag a ticker symbol onto a chart panel to change that chart's symbol (existing behavior via `WatchlistDragStart`).

### Drag State

```rust
enum DragKind {
    /// Reordering a row within the grid.
    RowReorder {
        source_row_id: RowKey,
        source_index: usize,
    },
    /// Reordering a column.
    ColumnReorder {
        source_col_id: ColumnId,
        source_index: usize,
    },
    /// Dragging a ticker to an external target (chart panel).
    TickerToChart {
        symbol: String,
        source_watchlist_id: WatchlistId,
    },
}

struct DragState {
    kind: DragKind,
    /// Cursor position at drag start (for distance threshold).
    start_pos: Point,
    /// Current cursor position (updated every frame).
    cursor_pos: Point,
    /// Offset from cursor to the top-left of the dragged element.
    hotspot: Vector,
    /// Whether the distance threshold has been met (drag is "active").
    activated: bool,
    /// Current drop target (computed from hit testing).
    drop_target: Option<DropTarget>,
    /// Animation progress for source dimming (0.0 = not dimmed, 1.0 = fully dimmed).
    source_dim: f32,
}

enum DropTarget {
    /// Insert before this row index.
    RowInsert { index: usize },
    /// Insert before this column index.
    ColumnInsert { index: usize },
    /// Drop on a chart panel (external).
    ChartPanel { chart_id: ChartId },
}
```

### Activation Guard

Drag does not activate immediately on mouse down. A distance threshold of 5px prevents accidental drags during click-to-sort or click-to-select:

```rust
const DRAG_THRESHOLD_PX: f32 = 5.0;

fn check_activation(state: &mut DragState, cursor: Point) -> bool {
    if state.activated {
        return true;
    }
    let delta = cursor - state.start_pos;
    let distance = (delta.x * delta.x + delta.y * delta.y).sqrt();
    if distance >= DRAG_THRESHOLD_PX {
        state.activated = true;
        true
    } else {
        false
    }
}
```

### Row Drag Ghost

When a row drag is active, the overlay renders a ghost:

```
+------ Drag ghost (follows cursor) ------+
| [AAPL   189.50   +1.2%   1.2M   0.85]  |  <-- 80% opacity, 2px drop shadow
+-----------------------------------------+

+------ Grid body ------+
| [NVDA   ...]          |
| [  2px blue line  ]   |  <-- drop indicator at insertion point
| [MSFT   ...]          |
| [AAPL   ...]  <- 30%  |  <-- source row dimmed
| [GOOG   ...]          |
+-----------------------+
```

**Ghost content**: A simplified copy of the row. The `overlay()` method creates the ghost by calling the same cell rendering functions with reduced opacity and a shadow background.

**Ghost styling**:
- Opacity: 0.8
- Background: solid (not transparent), slightly elevated color
- Shadow: Bottom shadow via a second `Container` behind the ghost, offset by 2px down, with a dark semi-transparent background
- Width: Same as the grid body width
- No scale animation (keeping it simple for Phase 1)

```rust
fn build_drag_ghost_row<'a>(
    row: &WatchlistTicker,
    market_data: &TickerMarketData,
    column_widths: &[f32],
) -> Element<'a, Message> {
    // Same cell construction as normal rows but with:
    // - No interactive elements (buttons replaced with labels)
    // - No click handlers
    let cells = build_row_cells_readonly(row, market_data, column_widths);
    container(Row::with_children(cells))
        .style(|_| container::Style {
            background: Some(Color::from_rgba(0.14, 0.14, 0.20, 0.90).into()),
            border: iced::Border {
                color: Color::from_rgba(0.22, 0.55, 0.95, 0.6),
                width: 1.0,
                radius: 3.0.into(),
            },
            shadow: iced::Shadow {
                color: Color::from_rgba(0.0, 0.0, 0.0, 0.3),
                offset: iced::Vector::new(0.0, 4.0),
                blur_radius: 8.0,
            },
            ..Default::default()
        })
        .into()
}
```

### Column Header Drag Ghost

When dragging a column header:
- The ghost is a single header cell, constrained to horizontal movement only (Y is locked to the header row Y position).
- The source column shows a dashed placeholder background.
- A vertical drop indicator line appears between columns at the insertion point.

```
Before drag:
  [Ticker] [Price] [Chg%] [G.ATR]

During drag (dragging "Price"):
  [Ticker] [-----] [Chg%] [G.ATR]
                 |
                 +-- Vertical blue line between Chg% and G.ATR
                     Floating ghost: [Price] at 80% opacity
```

### Drop Indicator

**Row reorder indicator**: A horizontal line spanning the full grid width at the insertion point.

```rust
struct DropIndicatorWidget {
    y: f32,           // Y position in grid-local coordinates
    width: f32,       // Full grid width
    thickness: f32,   // 2.0px
    color: Color,     // accent blue
}
```

Rendered as a `Container` with:
- Height: 2px
- Width: full grid width
- Background: accent blue (`Color::from_rgb(0.22, 0.55, 0.95)`)
- Positioned absolutely at the computed Y offset

**Column reorder indicator**: A vertical line at the column boundary.

### Source Item Dimming

During drag, the source row/column dims to 30% opacity. This is achieved by modifying the row's background and text colors in the `view()` function:

```rust
fn row_opacity_modifier(
    base_color: Color,
    row_id: RowKey,
    drag_state: &Option<DragState>,
) -> Color {
    if let Some(drag) = drag_state {
        if drag.activated {
            if let DragKind::RowReorder { source_row_id, .. } = &drag.kind {
                if *source_row_id == row_id {
                    return Color::from_rgba(
                        base_color.r,
                        base_color.g,
                        base_color.b,
                        base_color.a * 0.30,
                    );
                }
            }
        }
    }
    base_color
}
```

### Implementation via iced `overlay()`

The drag ghost and drop indicator are rendered through the grid widget's `overlay()` method. iced's overlay system is the correct mechanism because:

1. Overlays render above all sibling widgets in the widget tree.
2. Overlays can be positioned independently of the parent layout.
3. Overlays receive their own event handling cycle (for hit test passthrough).

The overlay widget structure:

```rust
struct DragOverlayWidget<'a> {
    ghost: Element<'a, Message>,
    ghost_position: Point,
    drop_indicator: Option<DropIndicatorWidget>,
    grid_bounds: Rectangle,
}

impl<'a> overlay::Overlay<Message, Theme, Renderer> for DragOverlayWidget<'a> {
    fn layout(&self, _renderer: &Renderer, bounds: Size) -> Node {
        // Overlay occupies the full window -- ghost can move anywhere
        Node::new(bounds)
    }

    fn draw(
        &self,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
    ) {
        // 1. Draw drop indicator (if any)
        if let Some(indicator) = &self.drop_indicator {
            indicator.draw(renderer, theme, style, layout, cursor);
        }
        // 2. Draw ghost at cursor-following position
        // ... translate renderer, draw ghost element
    }

    fn is_over(&self, _layout: Layout<'_>, _renderer: &Renderer, _cursor: Point) -> bool {
        false // Never intercept hits -- passthrough to underlying grid
    }
}
```

---

## 7. Column Resize Visual

### Resize Handle Hit Area

Between each pair of column headers, an invisible hit zone enables resize interaction.

```
Visual:    [Ticker]|[Price]|[Chg%]|[G.ATR]
           |       ^ 1px visible column separator
Hit area:  [Ticker] [8px]  [Price] [8px]  [Chg%] [8px]  [G.ATR]
                    ^^^^^^ invisible resize handle
```

**Hit area width**: 8px (4px on each side of the column boundary). This is wider than the 4px used in the current implementation, improving usability.

**Cursor change**: When the cursor enters the hit area, the mouse interaction changes to `mouse::Interaction::ResizingHorizontally` (the `col-resize` CSS cursor equivalent).

### During Resize

The current implementation uses a `stack` overlay with `mouse_area` tracking `on_move` (see `views.rs` line 1391-1403). This pattern is correct and should be preserved.

**Live column width change**: As the user drags the resize handle, the column width updates in real-time. The width is clamped to `[min_width, max_width]`:

```rust
fn handle_column_resize(
    state: &mut GridState,
    col_id: ColumnId,
    delta_x: f32,
    min_width: f32,
    max_width: f32,
) {
    let current = state.column_widths.get(&col_id).copied().unwrap_or(80.0);
    let new_width = (current + delta_x).clamp(min_width, max_width);
    state.column_widths.insert(col_id, new_width);
}
```

**Min/max constraints**:
- Drag handle column: fixed 26px (not resizable)
- Favorite column: fixed 30px (not resizable)
- Ticker column: min 50px, max 200px
- Price/Change/GATR columns: min 40px, max 200px
- Delete column: fixed 30px (not resizable)

### Ghost Line Alternative (Phase 2)

For a more polished feel, instead of live width change during resize, show a ghost vertical line that follows the cursor horizontally while the column boundaries remain unchanged. On mouse release, the column snaps to the new width.

This avoids continuous widget tree rebuilds during resize drag, which can feel laggy with complex cell content.

---

## 8. Performance Budget

### Target Frame Budget

| Scenario | Target frame time | FPS |
|---|---|---|
| 1 watchlist, 50 rows, idle | < 2ms | 60 |
| 1 watchlist, 50 rows, 50 flashes active | < 4ms | 60 |
| 1 watchlist, 200 rows (virtual scroll) | < 4ms | 60 |
| 20 charts + 2 watchlists, all active | < 14ms | 60 |
| Row drag in progress | < 8ms | 60 |
| Column resize in progress | < 8ms | 60 |

### Row Count Targets

| Row count | Behavior | Mechanism |
|---|---|---|
| 0-100 | Smooth, all rows in widget tree | Direct rendering, no virtual scroll |
| 100-500 | Smooth, virtual scroll active | Application-level windowing, ~60 visible rows |
| 500-1000 | Functional, virtual scroll | Overscan covers fast scrolling |
| 1000-10000 | Functional, virtual scroll | Only ~60-70 Element nodes at any time |
| 10000+ | Functional with scrollbar jump | Same mechanism, may need scroll-to-row for usability |

### Memory Budget

| Item | Cost | Notes |
|---|---|---|
| Per-row data (`WatchlistTicker`) | ~80 bytes | Symbol string + metadata |
| Per-row market data cache | ~128 bytes | Prices, change, GATR, colors |
| Per-row format cache | ~64 bytes | Cached conditional formatting results |
| Flash map entry | ~40 bytes | Per active flash (grid-side only; no previous-value storage) |
| Total for 500-row watchlist | ~136 KB | Well within budget (no per-cell previous-value map) |

### Minimizing Per-Frame Allocations

1. **Pre-allocate row element vectors**: The visible row `Vec<Element>` is pre-allocated to `visible_count + 2 * overscan` capacity and reused via `.clear()` + `.push()`.

2. **String formatting**: Price/change strings are formatted into a reusable `String` buffer using `write!()` rather than `format!()` which allocates.

3. **Market data computation**: `compute_all_market_data()` is called once per frame, not once per row. Results are stored in a `HashMap<String, TickerMarketData>` that persists across frames and is updated incrementally.

4. **Flash map**: Uses `HashMap::retain()` for expired flash cleanup (no allocation).

### Dirty Flag System

The grid widget tracks what has changed and skips unnecessary work:

```rust
struct GridDirtyFlags {
    /// Data has changed (new rows, updated values).
    data: bool,
    /// Column configuration changed (widths, order, visibility).
    columns: bool,
    /// Selection changed.
    selection: bool,
    /// Sort changed.
    sort: bool,
    /// Scroll position changed.
    scroll: bool,
    /// Flash animations are active (need continuous redraw).
    flashing: bool,
    /// Drag is active (need continuous redraw for ghost following).
    dragging: bool,
}
```

When nothing is dirty and no animations are active, the grid returns a cached `Element` tree (via iced's `lazy` widget or manual caching).

### Text Measurement Caching

Text layout (glyph shaping, line measurement) is the most expensive per-cell operation. iced's text rendering caches glyph runs internally, but we can further optimize:

1. **Tabular figures**: Use a font with tabular (monospaced) figures for numeric columns. This means all digits have the same width, so text width for a price like "189.50" can be computed as `6 * digit_width` without glyph shaping.

2. **Fixed column widths eliminate line-breaking**: Since cells have a fixed width and text is never wrapped (truncated with ellipsis instead), the text layout computation is minimal.

3. **Stable text content**: Between market data updates, cell text does not change. iced's widget diffing will detect unchanged `Text` widgets and skip re-layout.

---

## 9. Theme Integration

### Color Source

All grid colors derive from the existing theme constants in `crates/midas-app/src/theme.rs` and the trading-specific colors identified in the research.

### Grid-Specific Style Catalog

```rust
/// Style catalog for the grid component.
/// All colors are iced::Color (sRGB space, not linear).
pub struct GridStyle {
    // --- Backgrounds ---
    /// Even row background.
    pub row_even_bg: Color,
    /// Odd row background.
    pub row_odd_bg: Color,
    /// Selected row background.
    pub row_selected_bg: Color,
    /// Hovered row background.
    pub row_hover_bg: Color,
    /// Header row background.
    pub header_bg: Color,

    // --- Grid lines ---
    /// Column separator color.
    pub grid_line_color: Color,
    /// Header bottom border color.
    pub header_border_color: Color,

    // --- Text ---
    /// Primary cell text color.
    pub text_primary: Color,
    /// Secondary/muted text color.
    pub text_secondary: Color,
    /// Positive value color (green).
    pub text_positive: Color,
    /// Negative value color (red).
    pub text_negative: Color,
    /// Neutral value color (gray).
    pub text_neutral: Color,

    // --- Flash ---
    /// Flash color for price increase.
    pub flash_up: Color,
    /// Flash color for price decrease.
    pub flash_down: Color,
    /// Flash peak alpha.
    pub flash_alpha: f32,
    /// Flash duration in seconds.
    pub flash_duration: f32,

    // --- Selection ---
    /// Selection border accent color.
    pub selection_border: Color,

    // --- Drag ---
    /// Drop indicator line color.
    pub drop_indicator: Color,
    /// Drag ghost border color.
    pub drag_ghost_border: Color,
    /// Source item dimming factor during drag (0.0-1.0).
    pub drag_source_dim: f32,

    // --- Scrollbar ---
    /// Scrollbar track color.
    pub scrollbar_track: Color,
    /// Scrollbar thumb color.
    pub scrollbar_thumb: Color,
}
```

### Dark Theme Defaults

```rust
pub fn dark_grid_style() -> GridStyle {
    GridStyle {
        row_even_bg: Color::from_rgba(0.10, 0.10, 0.13, 1.0),
        row_odd_bg: Color::from_rgba(0.09, 0.09, 0.11, 1.0),        // == theme::BACKGROUND
        row_selected_bg: Color::from_rgba(0.18, 0.32, 0.55, 0.7),
        row_hover_bg: Color::from_rgba(0.14, 0.14, 0.18, 1.0),
        header_bg: Color::from_rgba(0.12, 0.12, 0.15, 1.0),         // == theme::TOOLBAR_BG
        grid_line_color: Color::from_rgba(1.0, 1.0, 1.0, 0.06),
        header_border_color: Color::from_rgba(1.0, 1.0, 1.0, 0.12),
        text_primary: Color::from_rgb(0.88, 0.88, 0.92),             // == theme::TEXT_PRIMARY
        text_secondary: Color::from_rgb(0.55, 0.55, 0.60),           // == theme::TEXT_SECONDARY
        text_positive: Color::from_rgb(0.10, 0.75, 0.40),            // == theme::CANDLE_BULL approx
        text_negative: Color::from_rgb(0.85, 0.20, 0.25),            // == theme::CANDLE_BEAR approx
        text_neutral: Color::from_rgb(0.60, 0.60, 0.60),
        flash_up: Color::from_rgba(0.10, 0.75, 0.40, 1.0),
        flash_down: Color::from_rgba(0.85, 0.20, 0.25, 1.0),
        flash_alpha: 0.45,
        flash_duration: 0.3,
        selection_border: Color::from_rgb(0.22, 0.55, 0.95),         // == theme::ACCENT
        drop_indicator: Color::from_rgb(0.22, 0.55, 0.95),           // == theme::ACCENT
        drag_ghost_border: Color::from_rgba(0.22, 0.55, 0.95, 0.6),
        drag_source_dim: 0.30,
        scrollbar_track: Color::from_rgba(1.0, 1.0, 1.0, 0.04),
        scrollbar_thumb: Color::from_rgba(1.0, 1.0, 1.0, 0.15),
    }
}
```

### Consistency with Existing Theme

The grid colors intentionally reference the existing constants from `theme.rs`:
- `BACKGROUND` for odd row backgrounds
- `TOOLBAR_BG` for header background
- `TEXT_PRIMARY` / `TEXT_SECONDARY` for cell text
- `ACCENT` for selection and drop indicators
- `CANDLE_BULL` / `CANDLE_BEAR` for positive/negative coloring

This ensures the grid looks like a natural part of the Hand of Midas application.

---

## 10. iced Widget Implementation (Phase 2+ Target)

> **Phase note**: This section describes the **Phase 2+ architecture** where the grid
> becomes a custom `Widget` implementation. In **Phase 0-1**, the grid uses a simpler
> composition approach: the `grid()` builder function returns an `Element` composed
> from standard iced widgets (`Row`, `Column`, `Scrollable`, `Container`, `mouse_area`).
> The transition to a custom `Widget` is required in Phase 2 to support `Widget::overlay()`
> for drag ghosts and drop indicators. See 04-implementation-roadmap.md Phase 2 for the
> transition plan and estimated additional complexity.

### Widget Trait Overview

The grid is implemented as a custom iced widget implementing the `Widget` trait. This gives full control over layout, rendering, event handling, and overlay management.

```rust
/// Generic over the column type `C` to preserve static dispatch, consistent with
/// the Phase 0-1 `grid()` builder function. The caller passes a slice of concrete
/// column values (e.g., `&[WatchlistColumn]`) rather than trait objects. This
/// enables monomorphization: the compiler generates specialized code for each
/// column type, avoiding vtable indirection. See 00-architecture.md Section 3
/// for the stated preference for static dispatch via monomorphization.
///
/// **Message type**: The Widget is generic over the app's message type `M`:
/// `Widget<M, Theme, Renderer>`. Cell elements are `Element<'a, M>` — they emit
/// `M` directly. Grid chrome (sort buttons, resize handles, selection areas)
/// maps through the stored `on_grid: Box<dyn Fn(GridMessage) -> M + 'a>`
/// callback via `shell.publish((self.on_grid)(grid_msg))`. This two-path design
/// avoids type contradictions: cells can emit arbitrary app messages while grid
/// chrome uses the structured `GridMessage` enum.
///
/// **Child element pattern**: Cell `Element`s are pre-built during construction
/// (from `col.cell()` / `col.header()` calls) and stored in `cells` / `headers`.
/// The Widget's `children()` declares them, `diff()` reconciles their `Tree` state,
/// `layout()` produces child `Node`s, and `draw()` renders via `tree.children[i]`
/// and `layout.children()[i]`. This follows iced's `Table` widget pattern.
/// See `iced_widget-0.14.2/src/table.rs` for the reference implementation.
pub struct GridWidget<'a, Row, M, C: GridColumn<Row, M>> {
    /// Column definitions (static dispatch via monomorphization).
    columns: &'a [C],
    /// Row data (already sorted, already sliced to visible range).
    rows: &'a [Row],
    /// Grid state (scroll, selection, column widths, drag, flash).
    state: &'a GridState,
    /// Grid style (colors, spacing).
    style: &'a GridStyle,
    /// Total row count (for scrollbar computation, may differ from rows.len()
    /// when virtual scrolling is active).
    total_row_count: usize,
    /// Callback mapping grid chrome events to the app's message type.
    on_grid: Box<dyn Fn(GridMessage) -> M + 'a>,
    /// Pre-built header cell Elements, one per column (in display order).
    /// Built during construction from `col.header()` calls.
    headers: Vec<Element<'a, M>>,
    /// Pre-built body cell Elements, row-major order: [row0_col0, row0_col1, ..., row1_col0, ...].
    /// Built during construction from `col.cell(row, idx)` calls for visible rows.
    /// Length = visible_rows * columns.len().
    cells: Vec<Element<'a, M>>,
}
```

> **Note:** The code in this section (Section 10) is pseudocode illustrating the approach.
> Actual method signatures and event forwarding will be verified during the Pre-Phase 2 spike.

### `children()` and `diff()`: Child Widget Tree Management

The grid pre-builds all cell `Element`s during construction, then declares them as children
so iced can manage their `Tree` state (enabling interactive cells like buttons and inputs).

```rust
fn children(&self) -> Vec<Tree> {
    // Headers first, then body cells (row-major order).
    self.headers.iter()
        .chain(self.cells.iter())
        .map(|el| Tree::new(el.as_widget()))
        .collect()
}

fn diff(&self, tree: &mut Tree) {
    let all_elements: Vec<&Element<'_, M>> =
        self.headers.iter().chain(self.cells.iter()).collect();
    tree.diff_children(&all_elements);
}
```

### `layout()`: How Column Widths and Row Heights Are Computed

```rust
fn layout(
    &self,
    tree: &mut Tree,
    renderer: &Renderer,
    limits: &layout::Limits,
) -> layout::Node {
    let max_size = limits.max();
    let width = max_size.width;
    let height = max_size.height;

    // Column widths are pre-computed in GridState (ImGui/egui pattern:
    // widths are explicit state, not derived from content).

    // Build child Node for each header cell, then each body cell.
    // This mirrors iced's Table widget: children() declares the elements,
    // layout() sizes them, draw() renders via tree.children[i] + layout.children()[i].
    let mut child_nodes = Vec::with_capacity(self.headers.len() + self.cells.len());

    // Header cell nodes
    let mut x = 0.0;
    for (col_idx, header_el) in self.headers.iter().enumerate() {
        let col_id = self.state.column_order[col_idx];
        let col_width = *self.state.column_widths.get(&col_id).unwrap_or(&80.0);
        let child_limits = layout::Limits::new(Size::ZERO, Size::new(col_width, HEADER_HEIGHT));
        let mut node = header_el.as_widget().layout(&mut tree.children[col_idx], renderer, &child_limits);
        node = node.move_to(Point::new(x, 0.0));
        child_nodes.push(node);
        x += col_width;
    }

    // Pre-compute column X offsets once (O(C)) to avoid O(C^2) per-cell lookups.
    let num_cols = self.columns.len();
    let column_offsets: Vec<f32> = {
        let mut offsets = Vec::with_capacity(num_cols);
        let mut acc = 0.0f32;
        for i in 0..num_cols {
            offsets.push(acc);
            let cid = self.state.column_order[i];
            acc += *self.state.column_widths.get(&cid).unwrap_or(&80.0);
        }
        offsets
    };

    // Body cell nodes (row-major: row0_col0, row0_col1, ..., row1_col0, ...)
    for (cell_idx, cell_el) in self.cells.iter().enumerate() {
        let row = cell_idx / num_cols;
        let col = cell_idx % num_cols;
        let col_id = self.state.column_order[col];
        let col_width = *self.state.column_widths.get(&col_id).unwrap_or(&80.0);
        let cell_x = column_offsets[col];
        let cell_y = HEADER_HEIGHT + (row as f32 * ROW_HEIGHT) - self.state.scroll_y;

        let child_limits = layout::Limits::new(Size::ZERO, Size::new(col_width, ROW_HEIGHT));
        let tree_idx = self.headers.len() + cell_idx;
        let mut node = cell_el.as_widget().layout(&mut tree.children[tree_idx], renderer, &child_limits);
        node = node.move_to(Point::new(cell_x, cell_y));
        child_nodes.push(node);
    }

    layout::Node::with_children(Size::new(width, height), child_nodes)
}
```

### `draw()`: Rendering Order

```rust
fn draw(
    &self,
    tree: &Tree,
    renderer: &mut Renderer,
    theme: &Theme,
    style: &renderer::Style,
    layout: Layout<'_>,
    cursor: mouse::Cursor,
    viewport: &Rectangle,
) {
    let bounds = layout.bounds();
    let child_layouts = layout.children();
    let num_cols = self.columns.len();

    // --- Layer 0 + 1: Background + Grid Lines ---
    self.draw_backgrounds(renderer, bounds);

    // --- Layer 2: Cell Content ---
    // Cell Elements were pre-built during construction and laid out in layout().
    // Draw them via tree.children[i] and layout.children()[i], following the
    // iced Table pattern. Per-column scissor clipping via renderer.with_layer().

    // Draw header cells (indices 0..num_cols in children)
    for col_idx in 0..num_cols {
        let child_layout = child_layouts[col_idx];
        let child_bounds = child_layout.bounds();
        renderer.with_layer(child_bounds, |renderer| {
            self.headers[col_idx].as_widget().draw(
                &tree.children[col_idx],
                renderer, theme, style,
                child_layout, cursor, viewport,
            );
        });
    }

    // Draw body cells (indices num_cols.. in children, row-major order)
    let header_offset = num_cols;
    for cell_idx in 0..self.cells.len() {
        let tree_idx = header_offset + cell_idx;
        let child_layout = child_layouts[tree_idx];
        let child_bounds = child_layout.bounds();

        // Skip cells outside viewport.
        if child_bounds.y + child_bounds.height < bounds.y
            || child_bounds.y > bounds.y + bounds.height
        {
            continue;
        }

        // Draw flash background if active (Layer 3).
        let row = cell_idx / num_cols;
        let col = cell_idx % num_cols;
        if let Some(flash) = self.state.flash_state.get(&(row, col)) {
            let bg = blended_background(self.row_bg(row), flash, Instant::now());
            renderer.fill_quad(
                renderer::Quad { bounds: child_bounds, ..Default::default() },
                bg,
            );
        }

        // Draw the pre-built cell Element.
        renderer.with_layer(child_bounds, |renderer| {
            self.cells[cell_idx].as_widget().draw(
                &tree.children[tree_idx],
                renderer, theme, style,
                child_layout, cursor, viewport,
            );
        });
    }

    // --- Layer 4: Selection highlight (drawn over cells) ---
    self.draw_selection_highlight(renderer, bounds);

    // --- Scrollbar ---
    self.draw_scrollbar(renderer, bounds);
}
```

### `update()`: Event Handling

> **Method name**: iced 0.14's `Widget` trait uses `update()` for event handling
> (confirmed against iced_core 0.14 source).

```rust
/// The Widget operates on `GridMessage` internally. At the call site, the app
/// uses `Element::map()` to convert: `grid.into().map(|gm| Message::Grid(id, gm))`.
/// This eliminates the need for a mapping closure inside the widget.
///
/// Variant names below are illustrative — canonical definitions are in
/// 00-architecture.md §4.1.
fn update(
    &mut self,
    tree: &mut Tree,
    event: &Event,
    layout: Layout<'_>,
    cursor: mouse::Cursor,
    renderer: &Renderer,
    clipboard: &mut dyn Clipboard,
    shell: &mut Shell<'_, M>,
    viewport: &Rectangle,
) {
    let bounds = layout.bounds();
    let state = tree.state.downcast_mut::<GridWidgetState>();

    // Forward events to child elements (interactive cells like buttons/toggles).
    // Children may publish their own GridMessage variants.
    let child_layouts = layout.children();
    for (i, child_el) in self.headers.iter_mut().chain(self.cells.iter_mut()).enumerate() {
        child_el.as_widget_mut().update(
            &mut tree.children[i], event, child_layouts[i],
            cursor, renderer, clipboard, shell, viewport,
        );
    }

    match event {
        // --- Scroll ---
        Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
            if cursor.is_over(bounds) {
                let dy = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => *y * ROW_HEIGHT,
                    mouse::ScrollDelta::Pixels { y, .. } => *y,
                };
                shell.publish((self.on_grid)(GridMessage::ScrollChanged(dy)));
                shell.capture_event();
            }
        }

        // --- Mouse button ---
        Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
            if let Some(pos) = cursor.position_in(bounds) {
                if pos.y < HEADER_HEIGHT {
                    self.handle_header_press(state, pos, shell);
                } else {
                    let row_idx = self.row_at_y(pos.y, state);
                    if let Some(idx) = row_idx {
                        shell.publish((self.on_grid)(GridMessage::RowSelected(idx)));
                    }
                }
                shell.capture_event();
            }
        }

        // --- Mouse move (drag tracking) ---
        Event::Mouse(mouse::Event::CursorMoved { position }) => {
            if let Some(drag) = &mut state.pending_drag {
                if check_activation(drag, *position) {
                    let target = self.compute_drop_target(*position, bounds);
                    // Canonical: ColumnDragging(f32) / RowDragging(f32)
                    shell.publish((self.on_grid)(GridMessage::ColumnDragging(position.x)));
                }
            }

            // Hover tracking is widget-internal (GridWidgetState), not a GridMessage.
            if cursor.is_over(bounds) {
                let local = Point::new(position.x - bounds.x, position.y - bounds.y);
                state.hovered_row = self.row_at_y(local.y, state);
            }
        }

        // --- Key events for keyboard navigation (Phase 3b) ---
        Event::Keyboard(keyboard::Event::KeyPressed { key, .. }) => {
            // Canonical variants from 00-architecture.md §4.1
            // Not wired until Phase 3b (keyboard navigation).
        }

        _ => {}
    }
}
```

### `mouse_interaction()`: Cursor Changes

```rust
fn mouse_interaction(
    &self,
    tree: &Tree,
    layout: Layout<'_>,
    cursor: mouse::Cursor,
    viewport: &Rectangle,
    renderer: &Renderer,
) -> mouse::Interaction {
    let bounds = layout.bounds();

    // Drag and resize state live in GridState (app-owned), not GridWidgetState.
    // Check the unified ActiveInteraction enum for active interactions.
    match &self.state.interaction {
        ActiveInteraction::ColumnDrag(_) | ActiveInteraction::RowDrag(_) => {
            return mouse::Interaction::Grabbing;
        }
        ActiveInteraction::Resize(_) => {
            return mouse::Interaction::ResizingHorizontally;
        }
        ActiveInteraction::None => {}
    }

    if let Some(pos) = cursor.position_in(bounds) {
        // Header area.
        if pos.y < HEADER_HEIGHT {
            // Check resize handle hit areas.
            let mut x_acc = 0.0;
            for col_id in &self.state.column_order {
                let col_width = *self.state.column_widths.get(col_id).unwrap_or(&80.0);
                x_acc += col_width;
                // 4px hit zone on each side of column boundary.
                if (pos.x - x_acc).abs() < 4.0 {
                    return mouse::Interaction::ResizingHorizontally;
                }
            }
            // Over a draggable header.
            return mouse::Interaction::Grab;
        }

        // Body area: check drag handle column.
        let first_col_id = self.state.column_order.first().copied();
        let first_col_width = first_col_id
            .and_then(|id| self.state.column_widths.get(&id).copied())
            .unwrap_or(26.0);
        if pos.x < first_col_width {
            return mouse::Interaction::Grab;
        }

        // Default: pointer for row selection.
        mouse::Interaction::Pointer
    } else {
        mouse::Interaction::default()
    }
}
```

### `overlay()`: Drag Ghost and Context Menu

```rust
fn overlay<'a>(
    &'a mut self,
    tree: &'a mut Tree,
    layout: Layout<'_>,
    renderer: &Renderer,
    translation: Vector,
) -> Option<overlay::Element<'a, Message, Theme, Renderer>> {
    // Active drag state lives in GridState (app-owned), not GridWidgetState.
    // Check the unified ActiveInteraction enum for active drag interactions.
    let drag_overlay = match &self.state.interaction {
        ActiveInteraction::ColumnDrag(d) => {
            Some((&d.cursor_pos, &d.hotspot, d.drop_target.as_ref()))
        }
        ActiveInteraction::RowDrag(d) => {
            Some((&d.cursor_pos, &d.hotspot, d.drop_target.as_ref()))
        }
        _ => None,
    };

    if let Some((cursor_pos, hotspot, drop_target)) = drag_overlay {
        // iced 0.14: overlay::Element::new takes Box<dyn Overlay>.
        // Position is handled inside DragOverlay::layout().
        return Some(overlay::Element::new(Box::new(DragOverlay {
            ghost: self.build_drag_ghost_from_state(),
            ghost_pos: *cursor_pos - *hotspot + translation,
            indicator: drop_target.map(|t| {
                self.build_drop_indicator(t, layout.bounds())
            }),
        })));
    }

    // Context menu (future).
    None
}
```

### `state()`: Internal Mutable State

```rust
/// Widget-internal ephemeral state. Does NOT hold interaction state (resize, column drag,
/// row drag) — those live in `GridState` (app-owned) so they can be serialized, tested,
/// and shared across panels. See 00-architecture.md Section 2.1.
///
/// Owned by iced's widget tree. Persists across frames via `Tree` state management.
///
/// **Boundary with `GridState`**:
///
///   `GridWidgetState` (widget-internal, ephemeral):
///     - `hovered_row` — transient hover highlight
///     - `pending_drag` — sub-threshold drag detection (before it becomes a real
///       drag in GridState)
///     - `modifiers` — current keyboard modifier snapshot
///
///   `GridState` (app-owned, persistent/meaningful):
///     - `column_order`, `column_widths` — persisted to config
///     - `sort` — persisted to config
///     - `selection` — meaningful app state (drives symbol linking)
///     - `scroll_y` — meaningful viewport position
///     - `flash_state` — animation lifecycle (owned here, not in widget)
///     - `interaction` — active resize/drag interaction via `ActiveInteraction`
///       enum (see 00-architecture.md §2.1)
struct GridWidgetState {
    /// Current hovered row index (for hover highlight).
    hovered_row: Option<usize>,
    /// Pending drag operation (before activation threshold).
    /// Once the threshold is crossed, the widget emits a `GridMessage` and the
    /// real drag state moves to `GridState.column_drag` / `GridState.row_drag`.
    pending_drag: Option<PendingDrag>,
    /// Keyboard modifier state.
    modifiers: keyboard::Modifiers,
}

struct PendingDrag {
    kind: DragKind,
    start_pos: Point,
    hotspot: Vector,
}
```

### Rendering Pipeline Diagram

```
                ┌──────────────────────┐
                │   MidasApp::view()   │
                └──────────┬───────────┘
                           │
                    ┌──────┴──────┐
                    │ GridWidget  │
                    │  (custom)   │
                    └──────┬──────┘
                           │
          ┌────────────────┼────────────────┐
          │                │                │
    ┌─────┴─────┐   ┌─────┴─────┐   ┌─────┴─────┐
    │  layout() │   │  update() │   │   draw()   │
    │           │   │           │   │            │
    │ Claims    │   │ Scroll    │   │ Layer 0:   │
    │ full      │   │ Mouse     │   │  Row BGs   │
    │ available │   │ Keyboard  │   │ Layer 1:   │
    │ space     │   │ → emits   │   │  Grid      │
    │           │   │   Message │   │  lines     │
    └───────────┘   └───────────┘   │ Layer 2:   │
                                    │  Cells     │
                                    │ Layer 3:   │
                                    │  Flash     │
                                    │ Layer 4:   │
                                    │  Selection │
                                    │ Layer 5:   │
                                    │  Header    │
                                    └─────┬──────┘
                                          │
                                    ┌─────┴──────┐
                                    │ overlay()  │
                                    │            │
                                    │ Layer 6:   │
                                    │  Drag      │
                                    │  ghost +   │
                                    │  indicator │
                                    └────────────┘
```

### Data Flow (Elm Architecture)

```
User Action                Message                      State Change
-----------                -------                      ------------
Click header      → GridMessage::SortToggled(col_id) → sort_spec updated, data re-sorted
Click row         → GridRowSelected(row_id)      → selected_row updated
Scroll wheel      → GridScroll(delta_y)          → scroll_y updated
Resize drag       → GridColumnResize(col, dx)    → column width updated
Header drag       → GridColumnReorder(from, to)  → column order updated
Row drag          → GridRowReorder(from, to)     → row order updated
Data update       → MarketDataUpdated(batch)     → app detects changes, emits FlashCell messages
Flash trigger     → GridMessage::FlashCell(...)  → flash_map updated (animation entry inserted)
Animation tick    → GridAnimationTick            → view rebuild (flash alpha recomputed)
Arrow key         → GridSelectNext/Previous      → selected_row updated, scroll adjusted
Enter key         → GridActivateSelection        → symbol link triggered
```

All state lives in `MidasApp`. The grid widget is a pure view function. All mutations go through messages. This is the Elm architecture, consistent with the rest of Hand of Midas.

---

### Critical Files for Implementation
List 3-5 files most critical for implementing this plan:
- D:\GitHub\HandOfMidas\desktop\win\crates\midas-grid\src\widget.rs (new — grid composition / Widget impl)
- D:\GitHub\HandOfMidas\desktop\win\crates\midas-app\src\app\views.rs (existing — watchlist view to replace)
- D:\GitHub\HandOfMidas\desktop\win\crates\midas-app\src\watchlist.rs (existing — state migration)
- D:\GitHub\HandOfMidas\desktop\win\crates\midas-app\src\theme.rs (existing — color constants)