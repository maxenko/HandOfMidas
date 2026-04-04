# Grid Component Architecture — `midas-grid`

**Author**: Architecture planning session
**Date**: 2026-04-01
**Status**: Draft
**Scope**: Core architecture for a reusable, headless grid/table widget for Hand of Midas

> **Canonical source**: This document is the canonical source for all **runtime type
> definitions** (`GridState`, `SortSpec`, `SelectionState`, `ResizeState`, `GridMessage`).
> Other plan documents may show simplified or phase-specific subsets; when in conflict,
> this document takes precedence. The canonical source for **persisted config types**
> (`GridConfig`, `SortSpecConfig`, `ColumnConfig`) is 03-column-data-model.md.

---

## Table of Contents

1. [Crate Placement](#1-crate-placement)
2. [Headless Core Pattern](#2-headless-core-pattern)
3. [Trait-Based Column System](#3-trait-based-column-system)
4. [Message / Event Architecture](#4-message--event-architecture)
5. [Component Hosting](#5-component-hosting)
6. [State Management](#6-state-management)
7. [Public API Sketch](#7-public-api-sketch)
8. [Dependency Direction](#8-dependency-direction)

---

## 1. Crate Placement

### Decision: New crate `midas-grid` under `crates/`

The grid component will be a new workspace member crate at `crates/midas-grid/`.

**Justification**:

The grid is a reusable UI component, not application-specific logic. It will be consumed by `midas-app` for the watchlist panel today, and potentially for order blotters, position tables, scanner results, and trade history views tomorrow. Placing it in its own crate provides:

1. **Clear dependency boundary** — `midas-grid` depends only on `iced` (and optionally `midas-core` for ID types). It does not depend on `midas-data`, `midas-chart`, `midas-render`, or `midas-feed`. This keeps recompilation scope narrow.

2. **Independent testability** — Grid logic (state transforms, column ordering, selection, resize math) can be unit tested without spinning up the full application.

3. **Separation from `midas-ui`** — The existing `midas-ui` crate holds simple widget wrappers (buttons, labels, tooltips). The grid is a complex composite widget with its own state machine, trait system, and message protocol. Mixing it into `midas-ui` would bloat a currently-clean crate.

4. **Not inside `midas-app`** — The app crate is a binary crate. Library code in binary crates cannot be reused, tested in isolation, or depended upon by other crates.

**Crate structure**:

```
crates/midas-grid/
  Cargo.toml
  src/
    lib.rs           -- Public API re-exports
    column.rs        -- GridColumn trait and ColumnId
    state.rs         -- GridState, SortSpec, SelectionState, ColumnState
    message.rs       -- GridMessage enum
    widget.rs        -- grid() builder function, produces Element
    selection.rs     -- Selection model (single, multi, range)
    resize.rs        -- Column resize state machine
    reorder.rs       -- Column reorder logic
    drag.rs          -- Row drag-and-drop state machine
    config.rs        -- Serializable grid configuration (persist/restore)
```

**Cargo.toml**:

```toml
[package]
name = "midas-grid"
version = "0.1.0"
edition = "2021"
publish = false

[dependencies]
iced = { workspace = true }
serde = { workspace = true }
```

Note: `midas-grid` does NOT depend on `midas-core`. The grid is generic over any row data type `T` and any message type `M`. Grid-specific IDs (`ColumnId`) are defined within the crate. Application-level IDs (`WatchlistId`) stay in `midas-core`. This keeps the grid maximally reusable.

---

## 2. Headless Core Pattern

Inspired by TanStack Table's headless architecture and Dear ImGui's specs-only sorting, the grid separates state management from rendering. The grid never owns, sorts, or filters data. It holds UI state and emits intents.

### 2.1 GridState

`GridState` is a plain data structure holding all grid-related UI state. It is owned by the application (inside `WatchlistPanel` or equivalent), not by the grid widget itself. The grid widget receives a reference to it during `view()`.

```rust
/// Unique identifier for a column within a grid instance.
/// Uses `&'static str` for zero-cost Copy + human-readable TOML serialization.
/// See 03-column-data-model.md for the canonical definition.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, Ord, PartialOrd)]
pub struct ColumnId(pub &'static str);

/// Complete UI state for one grid instance.
///
/// Owned by the application. Passed by reference to the grid widget
/// during `view()`. Mutated only in the application's `update()` via
/// grid messages.
///
/// **Phased construction**: Fields are introduced incrementally. See per-field
/// phase annotations below. Phase 0 code should only reference Phase 0 fields;
/// later-phase fields exist in the struct from the start (with inert defaults)
/// but their functionality is not wired up until the indicated phase.
///
/// **NOT `Serialize`/`Deserialize`**: `ColumnId(&'static str)` cannot be
/// deserialized as a `HashMap` key (serde requires `DeserializeOwned` keys,
/// and `&'static str` has no deserializer). Use `GridConfig` for persistence:
/// `to_config()` converts to serializable form, `from_config()` restores state.
#[derive(Debug, Clone)]
pub struct GridState {
    // -- Phase 0 --

    /// Ordered list of column IDs defining display order.
    /// When empty, columns render in definition order.
    pub column_order: Vec<ColumnId>,

    /// Column widths keyed by column ID (logical pixels).
    ///
    /// This map stores **resolved pixel widths** for all visible columns after layout.
    /// During `view()`, the grid's layout pass resolves `ColumnWidth::Flex` proportionally
    /// and populates this map. Only `Fixed` width overrides from `ColumnConfig` are restored
    /// at load time; Flex columns are re-resolved each frame. The map acts as both a cache
    /// and an override store: user-resized widths persist here as `Fixed` overrides.
    pub column_widths: HashMap<ColumnId, f32>,

    /// Active sort specification, or `None` for no sorting.
    pub sort: Option<SortSpec>,

    /// Row selection state (Phase 0: single selection only).
    pub selection: SelectionState,

    /// Vertical scroll offset (logical pixels).
    pub scroll_y: f32,

    // -- Phase 1+ --

    /// Active drag/resize interaction, if any. The enum makes mutual exclusion
    /// of interactions a compile-time guarantee — only one can be active at a time.
    /// Phase 0 defines `ActiveInteraction` with only the `None` variant.
    /// Phase 1 adds `Resize`; Phase 2 adds `ColumnDrag` and `RowDrag`.
    pub interaction: ActiveInteraction,
}
```

```rust
/// Unified interaction state. Only one drag/resize interaction can be active
/// at a time — the enum makes this a compile-time guarantee.
///
/// Phase 0: only `None` exists.
/// Phase 1: adds `Resize(ResizeState)`.
/// Phase 2: adds `ColumnDrag(ColumnDragState)` and `RowDrag(RowDragState)`.
#[derive(Debug, Clone, Default)]
pub enum ActiveInteraction {
    #[default]
    None,
    /// Phase 1+
    Resize(ResizeState),
    /// Phase 2+
    ColumnDrag(ColumnDragState),
    /// Phase 2+
    RowDrag(RowDragState),
}

impl ActiveInteraction {
    pub fn resize(&self) -> Option<&ResizeState> {
        match self {
            Self::Resize(s) => Some(s),
            _ => None,
        }
    }

    pub fn resize_mut(&mut self) -> Option<&mut ResizeState> {
        match self {
            Self::Resize(s) => Some(s),
            _ => None,
        }
    }

    pub fn column_drag(&self) -> Option<&ColumnDragState> {
        match self {
            Self::ColumnDrag(s) => Some(s),
            _ => None,
        }
    }

    pub fn row_drag(&self) -> Option<&RowDragState> {
        match self {
            Self::RowDrag(s) => Some(s),
            _ => None,
        }
    }
}
```

> **Persistence note**: `GridState` intentionally does **not** derive `Serialize` or `Deserialize`. The `column_widths: HashMap<ColumnId, f32>` field uses `ColumnId(&'static str)` as its key, and serde cannot deserialize `&'static str` map keys (it requires owned types like `String`). To persist grid state, convert to `GridConfig` via `to_config()`, which maps `ColumnId` values to `String`. To restore, use `from_config()`, which maps known column name strings back to `ColumnId(&'static str)` constants. See 03-column-data-model.md for the `GridConfig` definition.

```rust
/// Which column is sorted and in what direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SortSpec {
    pub column_id: ColumnId,
    pub direction: SortDirection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDirection {
    Ascending,
    Descending,
}

impl SortDirection {
    pub fn toggle(self) -> Self {
        match self {
            Self::Ascending => Self::Descending,
            Self::Descending => Self::Ascending,
        }
    }

    /// Unicode arrow for header display.
    pub fn indicator(self) -> &'static str {
        match self {
            Self::Ascending => " \u{25B2}",
            Self::Descending => " \u{25BC}",
        }
    }
}
```

### 2.2 Pure State Transforms

All state mutations are pure functions that take the current state and a message, returning the new state (or mutating in place via `&mut`). The grid widget itself is stateless — it reads `GridState` to render and emits `GridMessage` values that the application routes to update functions.

```rust
impl GridState {
    /// **Phase note**: This shows the Phase 1+ three-state cycle (Asc → Desc → None).
    /// Phase 0 preserves the existing two-state toggle (Asc ↔ Desc) for feature parity;
    /// Phase 1 replaces it with this version alongside `DefaultSortDirection`.
    ///
    /// Apply a sort toggle. If the same column is clicked, flip direction.
    /// If a different column, sort ascending by that column.
    /// If clicking a sorted column that is already descending, clear sort.
    pub fn toggle_sort(&mut self, column: ColumnId) {
        self.sort = match self.sort {
            Some(spec) if spec.column_id == column => {
                match spec.direction {
                    SortDirection::Ascending => Some(SortSpec {
                        column,
                        direction: SortDirection::Descending,
                    }),
                    SortDirection::Descending => None, // Third click clears
                }
            }
            _ => Some(SortSpec {
                column,
                direction: SortDirection::Ascending,
            }),
        };
    }

    /// Move a column from one display position to another.
    pub fn move_column(&mut self, from: usize, to: usize) {
        if from < self.column_order.len() && to < self.column_order.len() {
            let col = self.column_order.remove(from);
            self.column_order.insert(to, col);
        }
    }

    /// Commit a column resize. Clamps to min/max.
    pub fn set_column_width(&mut self, column: ColumnId, width: f32, min: f32, max: f32) {
        let clamped = width.max(min).min(max);
        self.column_widths.insert(column, clamped);
    }
}
```

### 2.3 Grid Never Owns Data (Specs-Only Sorting)

Following Dear ImGui's pattern, the grid does NOT sort or filter data. When a user clicks a header, the grid emits `GridMessage::SortToggled(ColumnId)`. The application receives this message, updates its `GridState.sort` field, and then sorts its own data array. On the next `view()` call, the application passes the sorted data slice to the grid.

This design means:

- The grid is generic over any data type `T`. It does not need `Ord` bounds on `T`.
- Sort comparators are defined by the column trait implementations, but the application chooses when and how to apply them.
- The same data can be displayed in multiple grids with different sort orders simultaneously.
- Complex sort logic (favorites-first pinning, multi-column sort, custom tiebreakers) lives in application code where it belongs.

### 2.4 Alternatives Considered

**Stateful widget (iced widget-internal state)**: iced widgets can carry state via the `widget::State` mechanism. The grid would own its sort, selection, and resize state internally, exposing it through query methods. **Rejected** because: (a) column widths and sort order must persist to TOML config — internal state is inaccessible for serialization, (b) symbol linking requires cross-panel state propagation — impossible from inside a widget, (c) all state mutations must go through `update()` for predictability (the Elm guarantee). **Trade-off accepted**: the app must manually route every `GridMessage` variant, which adds ~30 lines of boilerplate per grid instance.

**Trait objects for column storage**: Storing columns as `Vec<Box<dyn GridColumn<T, M>>>` avoids the enum approach but introduces heap allocation per column and makes static dispatch impossible. **Evaluated and deferred**: the enum approach (e.g., `WatchlistColumn`) gives zero-cost dispatch and exhaustive matching. If a future use case needs runtime-extensible columns, `Box<dyn GridColumn>` can be added without changing the trait.

---

## 3. Trait-Based Column System

Inspired by `iced_table`'s trait-based columns and AG Grid's ColDef, each column is defined as an enum variant or struct implementing the `GridColumn` trait. This gives type-safe, per-column control over extraction, formatting, comparison, rendering, and sizing.

### 3.1 The GridColumn Trait

> **Note**: The canonical trait definition is in `03-column-data-model.md §1.1`.
> The sketch below is illustrative; see 03 for the authoritative version.

```rust
/// Defines how a single column extracts, formats, compares, and renders
/// data from row type `T`, producing iced `Element`s with message type `M`.
pub trait GridColumn<T, M> {
    /// Unique identifier for this column.
    fn id(&self) -> ColumnId;

    /// Header content (label only — the grid composites sort indicators).
    fn header(&self) -> Element<'_, M>;

    /// Cell content for the given row. Returns any iced widget.
    fn cell<'a>(&'a self, row: &'a T, row_index: usize) -> Element<'a, M>;

    /// Width specification for this column.
    fn width(&self) -> ColumnWidth { ColumnWidth::Flex(1.0) }

    /// Minimum width during resize (default: 20.0).
    fn min_width(&self) -> f32 { 20.0 }

    /// Maximum width (default: None = unbounded).
    fn max_width(&self) -> Option<f32> { None }

    /// Whether this column can be resized by dragging (default: true).
    fn resizable(&self) -> bool { true }

    /// Whether clicking the header toggles sort (default: false).
    fn sortable(&self) -> bool { false }

    /// Whether this column can be reordered by dragging (default: true).
    fn reorderable(&self) -> bool { true }

    /// Compare two rows for sorting. Always ascending order;
    /// grid handles direction reversal. Default: Equal (stable no-op).
    fn compare(&self, _a: &T, _b: &T) -> std::cmp::Ordering {
        std::cmp::Ordering::Equal
    }
}
```

### 3.2 Why a Trait, Not a Struct

AG Grid uses a flat `ColDef` object with many optional fields. TanStack Table uses column definition objects with accessor functions. For Rust, a trait is the right choice because:

1. **Type safety** — Each column implementation is statically dispatched. The compiler verifies that `cell()` returns a valid `Element` for the correct message type.

2. **Per-column closures are awkward** — Storing `Box<dyn Fn(&T) -> Element>` for every column loses type information and requires lifetime gymnastics. A trait impl is cleaner.

3. **Pattern match dispatch** — The idiomatic Rust approach is an enum implementing the trait, which gives exhaustive matching and zero-cost dispatch.

4. **Default methods** — `min_width()`, `max_width()`, `resizable()`, etc. have sensible defaults. Columns only override what they need.

### 3.3 Example: Watchlist Column Enum

```rust
use midas_grid::{ColumnId, GridColumn, SortDirection};

/// All columns in the watchlist grid.
///
/// **Context threading**: Interactive columns (DragHandle, Favorite, Delete)
/// need a `WatchlistId` to emit app-level messages like
/// `Message::WatchlistRemoveTicker(wl_id, symbol)`. Since `cell()` only
/// receives `(&self, &T, usize)`, the `WatchlistId` is stored as a field
/// on each column instance. Columns are constructed per-view call with
/// the current watchlist ID baked in. This makes WatchlistColumn non-Copy
/// but avoids polluting the GridColumn trait with context parameters.
#[derive(Debug, Clone)]
pub struct WatchlistColumn {
    pub variant: WatchlistColumnKind,
    pub wl_id: WatchlistId,
}

#[derive(Debug, Clone, Copy)]
pub enum WatchlistColumnKind {
    DragHandle,
    Favorite,
    Ticker,
    Price,
    ChangePercent,
    GATR,
    Delete,
}

impl WatchlistColumn {
    /// Build all columns for a specific watchlist instance.
    pub fn all(wl_id: WatchlistId) -> [WatchlistColumn; 7] {
        use WatchlistColumnKind::*;
        [DragHandle, Favorite, Ticker, Price, ChangePercent, GATR, Delete]
            .map(|variant| WatchlistColumn { variant, wl_id })
    }
}

/// Row data that the watchlist grid operates on.
pub struct WatchlistRow {
    pub symbol: String,
    pub favorite: bool,
    pub last_price: Option<f64>,
    pub change_pct: Option<f64>,
    pub gatr_text: Option<String>,
    pub gatr_color: Option<[f32; 4]>,
}

impl GridColumn<WatchlistRow, Message> for WatchlistColumn {
    fn id(&self) -> ColumnId {
        use WatchlistColumnKind::*;
        match self.variant {
            DragHandle => ColumnId("drag"),
            Favorite => ColumnId("favorite"),
            Ticker => ColumnId("ticker"),
            Price => ColumnId("price"),
            ChangePercent => ColumnId("change_pct"),
            GATR => ColumnId("gatr"),
            Delete => ColumnId("delete"),
        }
    }

    fn header(&self) -> Element<'_, Message> {
        use WatchlistColumnKind::*;
        match self.variant {
            DragHandle => Space::new().into(),
            Favorite => text("\u{2605}").size(12).into(),
            Ticker => text("Ticker").size(12).into(),
            Price => text("Price").size(12).into(),
            ChangePercent => text("Chg%").size(12).into(),
            GATR => text("G.ATR").size(12).into(),
            Delete => Space::new().into(),
        }
    }

    fn cell<'a>(&'a self, row: &'a WatchlistRow, _idx: usize) -> Element<'a, Message> {
        use WatchlistColumnKind::*;
        let wl_id = self.wl_id;
        match self.variant {
            DragHandle => {
                button(text("\u{2807}").size(12))
                    .padding([2, 4])
                    .on_press(Message::WatchlistDragStart(wl_id, row.symbol.clone()))
                    .style(hover_text_button_style)
                    .into()
            }
            Ticker => text(&row.symbol).size(13).into(),
            Price => {
                let s = row.last_price
                    .map(|p| format!("{p:.2}"))
                    .unwrap_or_else(|| "--".into());
                text(s).size(13).into()
            }
            ChangePercent => {
                let s = row.change_pct
                    .map(|c| format!("{c:+.2}%"))
                    .unwrap_or_else(|| "--".into());
                let color = match row.change_pct {
                    Some(c) if c > 0.0 => Color::from_rgb(0.2, 0.8, 0.3),
                    Some(c) if c < 0.0 => Color::from_rgb(0.9, 0.25, 0.2),
                    _ => Color::from_rgb(0.6, 0.6, 0.6),
                };
                text(s).size(13).color(color).into()
            }
            // ... other columns
        }
    }

    fn width(&self) -> ColumnWidth {
        use WatchlistColumnKind::*;
        match self.variant {
            DragHandle => ColumnWidth::Fixed(26.0),
            Favorite => ColumnWidth::Fixed(30.0),
            Ticker => ColumnWidth::Fixed(70.0),
            Price => ColumnWidth::Fixed(80.0),
            ChangePercent => ColumnWidth::Fixed(65.0),
            GATR => ColumnWidth::Fixed(70.0),
            Delete => ColumnWidth::Fixed(30.0),
        }
    }

    fn sortable(&self) -> bool {
        use WatchlistColumnKind::*;
        matches!(self.variant, Ticker | Price | ChangePercent | GATR)
    }

    fn reorderable(&self) -> bool {
        use WatchlistColumnKind::*;
        !matches!(self.variant, DragHandle | Delete)
    }

    fn compare(&self, a: &WatchlistRow, b: &WatchlistRow) -> std::cmp::Ordering {
        use WatchlistColumnKind::*;
        match self.variant {
            Ticker => a.symbol.cmp(&b.symbol),
            Price => a.last_price.partial_cmp(&b.last_price)
                .unwrap_or(std::cmp::Ordering::Equal),
            ChangePercent => a.change_pct.partial_cmp(&b.change_pct)
                .unwrap_or(std::cmp::Ordering::Equal),
            GATR => a.gatr_text.cmp(&b.gatr_text),
            _ => std::cmp::Ordering::Equal,
        }
    }
}
```

---

## 4. Message / Event Architecture

The grid follows iced's Elm architecture. The grid widget emits `GridMessage` values. The application maps these into its own `Message` enum and handles them in `update()`. The grid never mutates state directly.

### 4.1 GridMessage Enum

```rust
/// Messages emitted by the grid widget.
///
/// The application wraps these in its own `Message` enum via a mapping
/// closure provided to the grid builder.
#[derive(Debug, Clone)]
pub enum GridMessage {
    // -- Sorting --
    /// User clicked a sortable column header.
    SortToggled(ColumnId),

    // -- Selection --
    /// User clicked a row (single select).
    RowSelected(usize),
    /// User Ctrl+clicked a row (toggle in multi-select).
    RowToggled(usize),
    /// User Shift+clicked a row (range select from anchor).
    RowRangeSelected(usize),

    // -- Column resize --
    /// User started dragging a column resize handle.
    ResizeStarted(ColumnId, f32),
    /// User is dragging a resize handle (current x position).
    Resizing(f32),
    /// User released the resize handle.
    ResizeEnded,

    // -- Column reorder --
    /// User started dragging a column header.
    ColumnDragStarted(ColumnId, f32),
    /// User is dragging a column header (current x position).
    ColumnDragging(f32),
    /// User dropped a column header at a new position.
    ColumnDragEnded,
    /// User cancelled column drag (Escape or drag out of bounds).
    ColumnDragCancelled,

    // -- Row drag --
    /// User started dragging a row (via the drag handle column).
    RowDragStarted(usize),
    /// User is dragging a row (current y position).
    RowDragging(f32),
    /// User dropped a row at a new position.
    RowDragEnded(usize),
    /// User cancelled row drag.
    RowDragCancelled,

    // -- Scroll --
    /// Vertical scroll offset changed.
    ScrollChanged(f32),
}
```

### 4.2 Application-Side Message Mapping

The grid widget accepts a closure that maps `GridMessage` into the application's `Message` type:

```rust
// In midas-app's Message enum:
pub enum Message {
    // ... existing variants ...
    WatchlistGrid(WatchlistId, GridMessage),
}

// In view code:
let wl_id = wl.id;
midas_grid::grid(&columns, &rows, &wl.grid_state)
    .on_message(move |msg| Message::WatchlistGrid(wl_id, msg))
```

### 4.3 Application-Side Update Handler

```rust
Message::WatchlistGrid(wl_id, grid_msg) => {
    let Some(wl) = self.watchlists.get_mut(&wl_id) else { return };
    match grid_msg {
        GridMessage::SortToggled(col_id) => {
            wl.grid_state.toggle_sort(col_id);
            wl.sort_tickers_by_grid_state();
        }
        GridMessage::RowSelected(idx) => {
            wl.grid_state.selection.select_single(idx);
            if let Some(symbol) = wl.ticker_at(idx) {
                self.propagate_symbol_link(wl.symbol_link, symbol);
            }
        }
        GridMessage::ResizeStarted(col_id, x) => {
            wl.grid_state.begin_resize(col_id, x);
        }
        GridMessage::Resizing(x) => {
            wl.grid_state.update_resize(x);
        }
        GridMessage::ResizeEnded => {
            wl.grid_state.commit_resize();
            self.mark_config_dirty();
        }
        GridMessage::RowDragEnded(to_index) => {
            if let Some(from_index) = wl.grid_state.row_drag_source() {
                wl.move_ticker(from_index, to_index);
            }
            wl.grid_state.interaction = ActiveInteraction::None;
            self.mark_config_dirty();
        }
        // ... handle remaining variants
    }
}
```

### 4.4 Why Not Internal State?

iced widgets can carry internal state via the `widget::State` mechanism. We deliberately avoid this for the grid because:

- **Persistence** — Column widths, sort order, and column order must persist across sessions via TOML config. If state lives inside the widget, the application cannot serialize it.
- **Shared state** — Symbol linking means selecting a row in the watchlist must update chart panels. This cross-panel coordination is impossible if selection lives inside a widget.
- **Testability** — `GridState` is a plain struct that can be constructed and asserted in unit tests without any widget tree.
- **Predictability** — The Elm architecture guarantees that all state mutations go through `update()`. Internal widget state creates hidden mutation paths.

---

## 5. Component Hosting

A key requirement is that grid cells can host ANY iced widget: text, buttons, input fields, toggles, sparklines, icons, or custom drawn content.

### 5.1 Cell Content as `Element`

The `GridColumn::cell()` method returns `Element<'a, M>`, which is iced's type-erased widget wrapper. This means any iced widget can live in a cell:

```rust
fn cell<'a>(&self, row: &'a WatchlistRow, _idx: usize) -> Element<'a, Message> {
    match self {
        // Text cell
        Self::Ticker => text(&row.symbol).size(13).into(),

        // Button cell
        Self::Favorite => {
            let label = if row.favorite { "\u{2605}" } else { "\u{2606}" };
            button(text(label).size(12))
                .on_press(Message::ToggleFavorite(row.symbol.clone()))
                .padding([2, 4])
                .into()
        }

        // Input field cell (for inline editing)
        Self::Notes => {
            text_input("", &row.notes)
                .on_input(|val| Message::NoteChanged(row.symbol.clone(), val))
                .size(12)
                .into()
        }

        // Custom canvas cell (for sparklines)
        Self::Trend => {
            canvas(TrendCanvas::new(&row.price_history))
                .width(60)
                .height(20)
                .into()
        }
    }
}
```

### 5.2 Layout Delegation

The grid widget wraps each cell's `Element` in a `container` with the column's current width. The cell content handles its own internal layout. The grid only controls:

- **Width**: Set from `GridState.column_widths[col_id]` (or `default_width()` as fallback)
- **Height**: Uniform row height set on the grid builder (e.g., 28px)
- **Padding**: Configurable cell padding (default: `[2, 4]`)
- **Clipping**: Content exceeding the cell bounds is clipped by the container

The grid does NOT impose alignment on cell content. Columns that want right-aligned prices do so in their `cell()` implementation:

```rust
Self::Price => {
    container(text(price_str).size(13))
        .width(Fill)
        .align_x(Horizontal::Right)
        .into()
}
```

### 5.3 Messages From Cell Content

Cell widgets emit the application's message type `M` directly, not `GridMessage`. A favorite toggle button emits `Message::ToggleFavorite(symbol)`, which the application handles in its own update logic. The grid is transparent — it passes cell messages through without interception.

The grid's own interactions (resize handles, header clicks, row selection areas) emit `GridMessage` values that are mapped via the `on_message` closure. Cell content messages bypass this mapping entirely because they are already of type `M`.

---

## 6. State Management

### 6.1 What Lives Where

| State | Location | Reason |
|---|---|---|
| Column display order | `GridState.column_order` | UI concern, persisted per-grid |
| Column widths | `GridState.column_widths` | UI concern, persisted per-grid |
| Sort spec | `GridState.sort` | UI concern; app uses it to sort data |
| Row selection | `GridState.selection` | UI concern + cross-panel linking |
| Scroll offset | `GridState.scroll_y` | UI concern, transient |
| Active interaction | `GridState.interaction` | Transient interaction state (Resize / ColumnDrag / RowDrag) |
| Row data (`Vec<T>`) | App state (`WatchlistPanel`) | Business data, app-owned |
| Market data | App state (computed from charts) | Derived data |
| Ticker list / ordering | App state (`WatchlistPanel.tickers`) | Business data |
| Symbol link mode | App state (`WatchlistPanel.symbol_link`) | Cross-panel concern |

### 6.2 Selection State

#### Phase 0: Single Selection

Phase 0 uses a minimal `SelectionState` that supports only single-row selection.
This avoids over-engineering for multi-select that is not needed until Phase 3.

```rust
/// Phase 0: single selection only.
/// Phase 3a replaces this with `BTreeSet<RowKey>`, `SelectionMode`, and
/// `anchor` for Ctrl+click / Shift+click multi-selection.
#[derive(Debug, Clone, Default)]
pub struct SelectionState {
    pub selected: Option<usize>,
    pub focused: Option<usize>,
}

impl SelectionState {
    pub fn select_single(&mut self, index: usize) {
        self.selected = Some(index);
        self.focused = Some(index);
    }

    pub fn is_selected(&self, index: usize) -> bool {
        self.selected == Some(index)
    }

    pub fn clear(&mut self) {
        self.selected = None;
        self.focused = None;
    }
}
```

#### Phase 3a: Multi-Selection (Future)

Phase 3a introduces `SelectionMode` and expands `SelectionState` for
Ctrl+click toggle and Shift+click range selection:

```rust
// Phase 3a+
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionMode {
    None,
    Single,
    Multi,
}

#[derive(Debug, Clone)]
pub struct SelectionState {
    pub mode: SelectionMode,
    /// BTreeSet<RowKey> for ordered iteration and stable identity across sorts.
    pub selected: BTreeSet<RowKey>,
    pub anchor: Option<RowKey>,
    pub focused: Option<RowKey>,
}
```

### 6.3 Column Configuration Persistence

Column state serializes to TOML for session persistence via `GridConfig`
(see canonical definition in 03-column-data-model.md §3.2). Key design:

- `GridConfig` uses `String` for column IDs in serialized fields (serde-compatible)
- At load time, `from_config()` maps known column name strings to `ColumnId(&'static str)` constants
- Unknown column names in the config file are silently ignored (forward-compatible)
- At save time, `to_config()` converts `ColumnId` back to `String` via the `Display` impl

### 6.4 Resize State Machine

The current codebase initializes `start_x` as `f32::NAN` on press and sets the real value on first move (because `mouse_area::on_press` does not provide cursor coordinates). The grid replaces this with `Option<f32>` to make the two-step initialization type-safe.

```rust
#[derive(Debug, Clone)]
pub struct ResizeState {
    pub column_id: ColumnId,
    pub start_x: Option<f32>,  // None until first mouse move (replaces NAN sentinel)
    pub start_width: f32,
}

impl GridState {
    pub fn begin_resize(&mut self, column: ColumnId) {
        let start_width = self.column_widths
            .get(&column)
            .copied()
            .unwrap_or(100.0);
        self.interaction = ActiveInteraction::Resize(ResizeState {
            column,
            start_x: None,
            start_width,
        });
    }

    pub fn update_resize(&mut self, current_x: f32) -> Option<f32> {
        self.interaction.resize_mut().map(|r| {
            let start = *r.start_x.get_or_insert(current_x);
            let delta = current_x - start;
            (r.start_width + delta).max(20.0)
        })
    }

    pub fn commit_resize(&mut self) {
        self.interaction = ActiveInteraction::None;
    }
}
```

---

## 7. Public API Sketch

### 7.1 Creating a Grid (Consumer Perspective)

```rust
use midas_grid::grid;

fn view_watchlist_body(&self, wl_id: WatchlistId) -> Element<'_, Message> {
    let wl = &self.watchlists[&wl_id];

    // Build row data (app-owned)
    let rows: Vec<WatchlistRow> = wl.tickers.iter().map(|t| {
        WatchlistRow {
            symbol: t.symbol.clone(),
            favorite: t.favorite,
            last_price: self.market_data.get(&t.symbol).and_then(|m| m.last_price),
            change_pct: self.market_data.get(&t.symbol).and_then(|m| m.change_pct),
            // ...
        }
    }).collect();

    // Sort by grid state (app sorts its own data)
    let mut sorted_rows = rows;
    if let Some(sort) = &wl.grid_state.sort {
        let col = WatchlistColumn::from_id(sort.column_id);
        sorted_rows.sort_by(|a, b| {
            let fav = b.favorite.cmp(&a.favorite); // favorites first
            if fav != std::cmp::Ordering::Equal { return fav; }
            let ord = col.compare(a, b);
            match sort.direction {
                SortDirection::Ascending => ord,
                SortDirection::Descending => ord.reverse(),
            }
        });
    }

    let columns = WatchlistColumn::all(wl_id);

    grid(&columns, &sorted_rows, &wl.grid_state)
        .row_height(28.0)
        .header_height(26.0)
        .cell_padding([2, 4])
        .row_background(|idx, selected| {
            if selected {
                Color::from_rgba(0.2, 0.35, 0.55, 0.6)
            } else if idx % 2 == 0 {
                Color::TRANSPARENT
            } else {
                Color::from_rgba(1.0, 1.0, 1.0, 0.02)
            }
        })
        .on_message(move |msg| Message::WatchlistGrid(wl_id, msg))
        .into()
}
```

### 7.2 Grid Builder API

```rust
pub fn grid<'a, T, M, C>(
    columns: &'a [C],
    rows: &'a [T],
    state: &'a GridState,
) -> Grid<'a, T, M, C>
where
    C: GridColumn<T, M>,
    M: Clone + 'a,
{
    Grid {
        columns,
        rows,
        state,
        row_height: 28.0,
        header_height: 26.0,
        cell_padding: [2, 4],
        row_bg: None,
        on_message: None,
    }
}

pub struct Grid<'a, T, M, C> {
    columns: &'a [C],
    rows: &'a [T],
    state: &'a GridState,
    row_height: f32,
    header_height: f32,
    cell_padding: [u16; 2],
    row_bg: Option<Box<dyn Fn(usize, bool) -> Color + 'a>>,
    on_message: Option<Box<dyn Fn(GridMessage) -> M + 'a>>,
}

impl<'a, T, M, C> Grid<'a, T, M, C>
where
    C: GridColumn<T, M>,
    M: Clone + 'a,
{
    pub fn row_height(mut self, h: f32) -> Self { self.row_height = h; self }
    pub fn header_height(mut self, h: f32) -> Self { self.header_height = h; self }
    pub fn cell_padding(mut self, pad: [u16; 2]) -> Self { self.cell_padding = pad; self }

    pub fn row_background(
        mut self,
        f: impl Fn(usize, bool) -> Color + 'a,
    ) -> Self {
        self.row_bg = Some(Box::new(f));
        self
    }

    pub fn on_message(
        mut self,
        f: impl Fn(GridMessage) -> M + 'a,
    ) -> Self {
        self.on_message = Some(Box::new(f));
        self
    }
}
```

---

## 8. Dependency Direction

### 8.1 Updated Workspace Crate Graph

```
midas-core          (leaf — shared types, IDs, config)
  |
  +-- midas-data    (SoA candle buffers, mmap)
  +-- midas-chart   (sans-IO chart logic)
  +-- midas-feed    (data providers)
  +-- midas-grid    (NEW — headless grid widget)  [depends on: iced, serde]
  |
  +-- midas-render  (wgpu GPU pipelines)
  |
  +-- midas-app     (binary — ties everything together)
        depends on: ALL above
```

**Critical**: `midas-grid` does NOT depend on `midas-core`. The grid crate depends only on `iced` and `serde`. It defines its own `ColumnId` type. Application-level types stay in `midas-core` and `midas-app`.

### 8.2 What Goes in Each Crate

| Concern | Crate |
|---|---|
| `ColumnId`, `GridState`, `GridMessage`, `GridColumn` trait | `midas-grid` |
| `WatchlistId`, `LinkMode`, `WatchlistConfig` | `midas-core` |
| `WatchlistPanel`, `WatchlistTicker`, `WatchlistRow` | `midas-app` |
| `WatchlistColumn` enum (implements `GridColumn`) | `midas-app` |
| Sort comparators for watchlist data | `midas-app` (via `WatchlistColumn::compare()`) |
| Grid widget rendering (iced Element tree) | `midas-grid` |

---

## Appendix: Error Handling

The grid crate follows the project's error handling convention (`thiserror` in library crates):

- **`view()` cannot return `Result`**: iced's `view()` is infallible. If a column's `cell()` encounters bad data (e.g., `None` for a required field), it returns a placeholder `Element` (e.g., `text("--")`), not an error.
- **Config deserialization**: `GridState::from_config()` returns `GridState` (not `Result`), falling back to defaults for missing or invalid fields. Unknown column names are silently ignored.
- **Width clamping**: NaN/Inf inputs to `set_column_width()` are clamped to `min_width`. No error type needed.
- **Grid-level errors**: The grid crate does not define its own error enum. All fallible operations are handled with defaults rather than propagated errors. This is intentional — the grid is a UI widget, not a service, and must always render something.

---

## Appendix A: Migration Path from Current Watchlist

The current watchlist in `views.rs` is a hand-built grid using raw iced `Row`, `Column`, `container`, `mouse_area`, and `Space` widgets. Migrating to `midas-grid`:

1. **Extract `WatchlistRow`** — Combine `WatchlistTicker` + `TickerMarketData` into a single struct.
2. **Implement `GridColumn` for `WatchlistColumn`** — Move per-cell rendering from `view_watchlist_body()` into `WatchlistColumn::cell()`.
3. **Replace `column_widths: [f32; 7]`** with `GridState` — Fixed-size array becomes `HashMap<ColumnId, f32>`.
4. **Replace `sort_column`/`sort_direction`** with `GridState.sort`.
5. **Replace manual resize overlay** — Grid handles resize handles internally.
6. **Replace `Watchlist*` message variants** with `WatchlistGrid(WatchlistId, GridMessage)` — Consolidates ~10 message variants into one.
7. **Keep favorites-first pinning** — Custom sort tiebreaker stays in application code.

The migration is incremental. The grid can be developed and tested independently, then swapped in.

## Appendix B: Trading-Specific Considerations

### Flash-on-Tick

- Flash state lives in `GridState`, not in application state. The grid owns a `flash_state: HashMap<(usize, ColumnId), FlashState>` that tracks flash timestamps and provides the flash background color with decaying alpha. (See 04-implementation-roadmap.md Phase 3a for the canonical flash state definition.)
- The application is responsible for detecting price changes and notifying the grid via `GridMessage::FlashCell { column, row, direction }`.
- A timer message (`GridMessage::FlashTick`) fires every ~50ms to decay flash alpha values within the grid.
- See **02-rendering.md Section 4** and **04-implementation-roadmap.md Phase 3** for the canonical flash design.

### Conditional Formatting

Color-coding cells is handled entirely in `WatchlistColumn::cell()` via `text(...).color(...)` or `container(...).style(...)`. No grid-level API needed.

### Symbol Linking

When a row is selected, the application checks `WatchlistPanel.symbol_link` and propagates the symbol to linked charts. This happens in the `RowSelected` message handler, not inside the grid.
