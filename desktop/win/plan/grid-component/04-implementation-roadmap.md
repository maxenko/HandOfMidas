# 04 -- Grid Component: Implementation Roadmap

**Project**: Hand of Midas -- Native Rust Desktop Trading Application
**Component**: Professional-grade grid/table widget for watchlists
**Stack**: iced 0.14, wgpu 27, Rust 2021 stable, Windows 11
**Date**: 2026-04-01

---

## File Structure

All grid code lives in a new crate `crates/midas-grid/`. This follows the workspace's multi-crate convention and matches the architecture document's decision (see 00-architecture.md §1).

> **Path convention**: All `crates/` paths in this document are relative to the
> desktop workspace root at `desktop/win/`. The full repository path for the new
> crate is `desktop/win/crates/midas-grid/`. This matches the existing crates
> (`midas-app`, `midas-chart`, `midas-core`, etc.) which are all under `desktop/win/crates/`.

```
crates/midas-grid/
  Cargo.toml                    -- depends on iced, serde only (NOT midas-core)
  src/
    lib.rs                      -- public re-exports, Grid widget constructor fn
    state.rs                    -- GridState, ColumnState, ScrollState
    column.rs                   -- GridColumn<T,M> trait, ColumnId, ColumnWidth, SortDirection
    message.rs                  -- GridMessage enum
    widget.rs                   -- grid() builder, composes header + body into Element
    header.rs                   -- header row layout + rendering helpers
    body.rs                     -- body row layout + rendering helpers
    style.rs                    -- GridStyle, cell/header style constants
    columns/
      mod.rs                    -- re-exports
      text.rs                   -- TextColumn (display-only text cell)
      numeric.rs                -- NumericColumn (right-aligned, formatted numbers)
      button.rs                 -- ButtonColumn (clickable icon/text button cell)
      toggle.rs                 -- ToggleColumn (star/favorite toggle)
      drag_handle.rs            -- DragHandleColumn (grip icon for row drag)
    interactions/
      mod.rs                    -- shared types (drag threshold, coordinate helpers), re-exports
      resize.rs                 -- column resize state machine
      reorder.rs                -- column reorder drag state machine
      sort.rs                   -- sort click handler, direction cycling
      selection.rs              -- row selection model (single + multi)
      drag.rs                   -- row drag-and-drop state machine
      keyboard.rs               -- keyboard navigation handler
    render.rs                   -- custom rendering helpers (overlays, drop indicators) [Phase 2]
    flash.rs                    -- flash-on-tick background interpolation [Phase 3]
    context_menu.rs             -- right-click context menu overlay
    persistence.rs              -- GridColumnState serialization for TOML config
```

---

## Pre-Phase 0: No Spike Required

Phase 0-2 use iced's `Scrollable` directly, rendering all rows (no virtual scrolling).
For watchlists with <500 rows, this is sufficient. Virtual scrolling is deferred to
Phase 3, at which point a **Pre-Phase 3 spike** validates spacer-based virtual scrolling
(see Phase 3 section below).

---

## Phase 0: Foundation

**Goal**: A working grid widget that replaces the current watchlist's inline view code with identical functionality. No new features. The grid renders a fixed header and scrollable body with text cells, buttons, and the existing sort/select/resize behavior, but through the new `GridColumn` trait abstraction. Resize is migrated as-is from the existing inline `mouse_area` logic in `views.rs` (the formal `ResizeState` state machine is a Phase 1 deliverable).

**Dependencies on previous phases**: None (this is the first phase).

### Files to Create

| File | Purpose |
|------|---------|
| `crates/midas-grid/src/lib.rs` | Module root. Re-exports `GridState`, `GridColumn`, `grid()` constructor, column types. |
| `crates/midas-grid/src/state.rs` | `GridState` struct holding column configs, scroll offset, sort spec, selection. |
| `crates/midas-grid/src/column.rs` | `GridColumn<T, Message>` trait, `ColumnId`, `ColumnWidth`, `SortDirection`, `SortSpec`. |
| `crates/midas-grid/src/widget.rs` | `grid()` function returning an `Element`. Builds header `Row` + `scrollable` body `Column` from trait-provided cells. |
| `crates/midas-grid/src/header.rs` | `grid_header()` function: builds one header row from column definitions, inserting resize handles between cells. |
| `crates/midas-grid/src/body.rs` | `grid_body()` function: iterates sorted row slice, builds one `Row` per data row using `GridColumn::cell()`. |
| `crates/midas-grid/src/message.rs` | `GridMessage` enum — grid chrome event type, mapped to the app's message type via the `on_grid` callback. Phase 0 defines `SortToggled` and `RowSelected`; later phases extend. Cell content emits the app's message type `M` directly. |
| `crates/midas-grid/src/style.rs` | `GridStyle` struct and constants migrated from `views.rs` (`GRID_BORDER_COLOR`, `GRID_HEADER_BORDER_COLOR`, cell/header container styles). |
| `crates/midas-grid/src/columns/mod.rs` | Re-exports. Phase 0 ships empty (placeholder for Phase 2+ pre-built column types). |

> **Deferred to Phase 2+**: Pre-built column types (`TextColumn`, `NumericColumn`, `ButtonColumn`, `ToggleColumn`, `DragHandleColumn`) are not needed in Phase 0 because the watchlist uses a single `WatchlistColumn` enum implementing `GridColumn`. Pre-built types will be added when a second grid consumer exists (order blotter, scanner).

### Files to Modify

| File | Change |
|------|--------|
| `desktop/win/Cargo.toml` | Add `midas-grid` as workspace member. |
| `crates/midas-grid/Cargo.toml` | New file: `[dependencies] iced = { workspace = true }, serde = { workspace = true }` |
| `crates/midas-app/Cargo.toml` | Add `midas-grid = { path = "../midas-grid" }` dependency. |
| `crates/midas-app/src/app/views.rs` | Replace `view_watchlist_body()` implementation: call `midas_grid::grid()` instead of building rows inline. Remove `grid_cell()`, `grid_header_cell()` helpers (moved to grid crate). Keep `view_watchlist_title_bar()` unchanged. |
| `crates/midas-app/src/watchlist.rs` | Replace `column_widths: [f32; 7]` with `grid_state: GridState`. Replace `sort_column`/`sort_direction` with grid state's `SortSpec`. |
| `crates/midas-app/src/app.rs` | Add `Message::WatchlistGrid(WatchlistId, GridMessage)` variant. Add handler that dispatches to `GridState` methods. |

### Types to Define

| Type | Location | Description |
|------|----------|-------------|
| `ColumnId` | `column.rs` | `#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)] pub struct ColumnId(pub &'static str)` — see canonical definition in 03-column-data-model.md §1.1. |
| `ColumnWidth` | `column.rs` | `enum ColumnWidth { Fixed(f32), Flex(f32), Auto }` — see canonical definition in 03-column-data-model.md §2. Min/max constraints come from the trait methods, not the enum. |
| `SortDirection` | `column.rs` | `enum SortDirection { Ascending, Descending }` with `toggle()` and `indicator()` methods. Replaces `watchlist::SortDirection`. |
| `SortSpec` | `column.rs` | `struct SortSpec { column_id: ColumnId, direction: SortDirection }` -- single-column sort initially. |
| `GridState` | `state.rs` | **Phase 0 shape** (see 00-architecture.md §2.1 for the final Phase 2+ shape). Phase 0 fields: `column_order: Vec<ColumnId>`, `column_widths: HashMap<ColumnId, f32>`, `sort: Option<SortSpec>`, `selection: SelectionState`, `scroll_y: f32`, `interaction: ActiveInteraction`. The `ActiveInteraction` enum provides compile-time mutual exclusion of transient interactions: Phase 0 defines it with only the `None` variant; Phase 1 adds `Resizing(ResizeState)`; Phase 2 adds `DraggingColumn(ColumnDragState)` and `DraggingRow(RowDragState)`. Does NOT hold data. |
| `SelectionState` | `state.rs` | **Phase 0 shape (single-selection only)**: `struct SelectionState { selected: Option<usize>, focused: Option<usize> }`. Methods: `select(index)`, `is_selected(index) -> bool`, `clear()`. Phase 0 uses single selection only. Phase 3a introduces `BTreeSet<RowKey>` for multi-selection, replacing this simple struct entirely (see Phase 3a notes). |
| `Alignment` | `column.rs` | `enum Alignment { Start, Center, End }` — horizontal cell content alignment used by `GridColumn::align()`. |
| `ColumnState` | `state.rs` | `struct ColumnState { id: ColumnId, width: f32, display_order: usize, visible: bool }` -- **runtime** column state, keyed by `ColumnId`. Note: the serde-compatible **persistence** type `ColumnConfig` (defined in 03-column-data-model.md) is a separate struct introduced in Phase 3a's `config.rs`/`persistence.rs`, keyed by string column name. The runtime type is deliberately named `ColumnState` (not `ColumnConfig`) to avoid collision with the persistence type. |
| `GridStyle` | `style.rs` | `struct GridStyle { header_border_color, cell_border_color, row_height, header_height, selected_bg, hover_bg, resize_handle_width }` |
| `GridMessage` | `message.rs` | `enum GridMessage { SortToggled(ColumnId), RowSelected(usize) }` — Phase 0 variants. Extended in Phase 1 (resize) and Phase 2 (drag). |
| `GridColumn<T, Message>` | `column.rs` | Core trait (see below). |

### GridColumn Trait (Phase 0 Signature)

> See 03-column-data-model.md §1.1 for the canonical trait definition.
> Phase 0 uses the full trait from the start to avoid rework in Phase 1.

```rust
pub trait GridColumn<T, Message> {
    /// Stable identifier for this column.
    fn id(&self) -> ColumnId;

    /// Header content (label only — grid composites sort indicators).
    fn header(&self) -> Element<'_, Message>;

    /// Cell content for one row.
    fn cell<'a>(&'a self, row: &'a T, row_index: usize) -> Element<'a, Message>;

    /// Width specification.
    fn width(&self) -> ColumnWidth { ColumnWidth::Flex(1.0) }

    /// Minimum allowed width (for resize clamping).
    fn min_width(&self) -> f32 { 20.0 }

    /// Maximum width (None = unbounded).
    fn max_width(&self) -> Option<f32> { None }

    /// Whether this column can be resized by dragging.
    fn resizable(&self) -> bool { true }

    /// Whether clicking the header triggers sort.
    fn sortable(&self) -> bool { false }

    /// Whether this column can be reordered by header drag.
    fn reorderable(&self) -> bool { true }

    /// Compare two rows for ascending sort. Default: Equal (stable no-op).
    fn compare(&self, _a: &T, _b: &T) -> std::cmp::Ordering {
        std::cmp::Ordering::Equal
    }

    /// Horizontal alignment of cell content. Default: Start (left-aligned).
    fn align(&self) -> Alignment {
        Alignment::Start
    }
}
```

### GridMessage (Introduced in Phase 0)

> `GridMessage` is defined in Phase 0 (not Phase 1) to avoid rebuilding
> the sort path. Phase 0 uses only `SortToggled` and `RowSelected`;
> Phase 1 adds resize variants; Phase 2 adds drag variants.

```rust
#[derive(Debug, Clone)]
pub enum GridMessage {
    SortToggled(ColumnId),
    RowSelected(usize),
    // Phase 1 adds: ResizeStarted, Resizing, ResizeEnded
    // Phase 2 adds: ColumnDragStarted, RowDragStarted, etc.
}
```

### Key Implementation Steps

1. **Create `crates/midas-grid/src/lib.rs`** with `pub mod state; pub mod column; pub mod message; pub mod widget; pub mod header; pub mod body; pub mod style; pub mod columns;` and public re-exports.

2. **Define core types in `column.rs`**: `ColumnId`, `ColumnWidth`, `SortDirection`, `SortSpec`, and the `GridColumn<T, Message>` trait. `SortDirection` must provide `toggle()` and `indicator()` identical to the existing `watchlist::SortDirection` so behavior is unchanged. **Phase 0 uses a two-state toggle (Asc <-> Desc, no clear-to-None)**; clicking a column header flips direction, clicking a different column switches to that column with the current direction. The three-state cycle (Asc -> Desc -> None) is introduced in Phase 1.

   > **Per-column default direction (Phase 0 migration note)**: Phase 0's sort toggle must preserve the existing per-column default direction behavior: when switching to a *new* column, numeric columns (Price, ChangePercent, GATR) start Descending and text columns (Ticker) start Ascending. **Important**: This logic lives in the **app's update handler** (matching on `WatchlistColumn` kind), NOT inside `GridState::toggle_sort()`. The grid crate's `toggle_sort()` takes a `default_direction: SortDirection` parameter — the app passes the appropriate direction based on the column. This keeps `midas-grid` generic and free of column-specific knowledge. Phase 1's `DefaultSortDirection` (via `GridColumn::default_sort_direction()`) formalizes this into a trait method, replacing the app-side match.

3. **Define `GridState` in `state.rs`**: Implements the canonical definition from 00-architecture.md §2.1 — stores `column_order: Vec<ColumnId>`, `column_widths: HashMap<ColumnId, f32>`, `sort: Option<SortSpec>`, `selection: SelectionState`, `scroll_y: f32`, `interaction: ActiveInteraction`. Phase 0 defines `ActiveInteraction` with only the `None` variant (later phases add interaction variants). Provides methods: `column_width(id) -> f32`, `set_column_width(id, f32, min, max)`, `toggle_sort(col)`, `move_column(from, to)`. The grid state is owned by `WatchlistPanel` (replacing the current `column_widths: [f32; 7]` array).

4. **Define `GridStyle` in `style.rs`**: Migrate `GRID_BORDER_COLOR`, `GRID_HEADER_BORDER_COLOR`, and the cell/header container style closures from `views.rs`.

5. **Implement `header.rs`**: A function `grid_header<'a, T, M, C>(columns: &'a [C], state: &GridState, on_grid: &dyn Fn(GridMessage) -> M) -> Element<'a, M>` where `C: GridColumn<T, M>`. Iterates columns, calls `col.header()`, composites sort indicators separately, wraps each in a container with the column width, interleaves 4px `mouse_area` resize handles. Grid chrome interactions call `on_grid(GridMessage::SortToggled(col))` to produce `M` values. This is a direct extraction from `let header_labels: Vec<Element>` through `let header = Row::with_children` in `views.rs`.

6. **Implement `body.rs`**: A function `grid_body<'a, T, M, C>(rows: &'a [T], columns: &'a [C], state: &GridState, on_grid: &dyn Fn(GridMessage) -> M) -> Element<'a, M>` where `C: GridColumn<T, M>`. Iterates rows, builds a `Row` per data row with cells from `col.cell(row, i)`. Cell widgets emit `M` directly (the grid is transparent to cell messages). Grid chrome (row selection areas) calls `on_grid(GridMessage::RowSelected(idx))` to produce `M` values. This extracts from `let mut rows = Column::new()` through the end of the sorted-rows loop in `views.rs`.

7. **Implement `widget.rs`**: The `grid()` constructor function takes `on_grid` as a required parameter (not a builder method) — see 00-architecture.md §7.2. The `into() -> Element<'a, M>` method composes `grid_header()` + `scrollable(grid_body())` + add-ticker row into a `column![]`. Cell content emits `M` directly; grid chrome maps through `on_grid`. In Phase 2, the custom `Widget<M>` stores `on_grid` as `Box<dyn Fn(GridMessage) -> M + 'a>` and calls it in `update()` via `shell.publish((self.on_grid)(grid_msg))`. No message-type refactor is needed at the Widget transition because the grid is generic over `M` from Phase 0.

8. **Implement `WatchlistColumn` enum** in `midas-app`: Implement `GridColumn<WatchlistRow, Message>` for the `WatchlistColumn` enum (see 00-architecture.md §3.3 for the full example). This moves per-cell rendering from `view_watchlist_body()` into `WatchlistColumn::cell()`. Pre-built generic column types (`TextColumn`, `NumericColumn`, etc.) are deferred to Phase 2+.

9. **Wire up in `views.rs`**: Replace `view_watchlist_body()` to construct column definitions and call `grid::widget::grid()`. The watchlist-specific data (market data map, sort comparator, favorites-first logic) stays in `views.rs`; only rendering delegates to the grid.

10. **Migrate `WatchlistPanel`**: Replace `column_widths: [f32; 7]` with `grid_state: GridState`. Replace `sort_column: Option<SortColumn>` and `sort_direction: SortDirection` with the grid state's `SortSpec`. Update `from_config` / `to_config` to serialize the new `GridConfig` format directly (no backward-compatible dual-format loading needed — no shipped releases exist).

### Testing Strategy

- **Unit tests for `GridState`**: Test `set_column_width`, `column_width`, width clamping at `min_width`, sort spec set/clear/toggle.
- **Unit tests for `SortDirection`**: Toggle, indicator strings.
- **Unit tests for `ColumnState`**: Display order, visibility.
- **Integration test**: Build a grid with mock data and 3 test columns (a mock enum implementing `GridColumn<T, M>`). Assert that `grid()` returns an `Element` without panic. (Cannot assert pixel output, but can verify the construction path. Note: `TextColumn` and other pre-built types are deferred to Phase 2+; use a test-specific column enum here.)
- **Manual regression test**: Launch the app, verify the watchlist looks identical to the pre-refactor version. Use the structured checklist below.
- **Manual test checklist** (visual regression testing is a known gap; this checklist is the mitigation):
  - [ ] Header text alignment with body column content (within 1px -- no visible misalignment between header labels and the data cells beneath them)
  - [ ] Sort arrow glyph appears at the correct position relative to header text (immediately after label, not overlapping or clipped)
  - [ ] Row selection highlight covers full row width (no gap at left/right edges)
  - [ ] Column resize handles respond in the correct hit zone (divider +/-2px in Phase 0, widened to +/-4px in Phase 1 -- cursor changes to resize icon only near the divider). Phase 0 uses the migrated inline resize logic from `views.rs`; the formal `ResizeState` machine with 8px hit zone replaces this in Phase 1.
  - [ ] Alternating row background colors match pre-refactor appearance (same colors, same row parity)
  - [ ] Grid scrolls smoothly with no visual tearing (test with >20 rows)
  - [ ] Watchlist data (prices, change%) renders identically to pre-refactor (same number formatting, same color coding for positive/negative values)
  - [ ] Favorite toggle works (star icon toggles on click)
  - [ ] Delete button works (row removed on click)
  - [ ] Drag grip button renders (visual presence only -- drag behavior is Phase 2)
  - [ ] Add-ticker input and submit work (type symbol, press Enter, row appears)
- **Config roundtrip test**: Save config with new `GridState` format, reload, verify widths and sort are restored.
- **`from_config()` edge case tests**:
  - Config contains unknown column ID (silently dropped, no panic).
  - Column definition exists but is not in saved config (appended to end of `column_order`).
  - Full round-trip with reordered columns (save → load → verify order matches).

### Migration Plan

The migration is split into **4 commits** to reduce blast radius and make regressions easier to diagnose:

1. **Commit 1: Build `midas-grid` crate** — New files only; no changes to existing code. Verify it compiles and grid unit tests pass.
2. **Commit 2: Introduce `GridState` in `WatchlistPanel`** — Replace `column_widths: [f32; 7]`, `sort_column`, and `sort_direction` with `grid_state: GridState`. Write the new `GridConfig` format directly (no backward-compatible dual-format loading needed — the codebase has no shipped releases, so there are no existing config files to migrate). Add `Message::WatchlistGrid(WatchlistId, GridMessage)` variant. At this point both the old view code and new state coexist — the old `view_watchlist_body()` reads from `grid_state` fields instead of the old flat fields.
3a. **Commit 3a: Add `WatchlistColumn` enum** — Implement the `WatchlistColumn` enum with `GridColumn` trait implementation, with unit tests verifying each column variant's `cell()` and `header()` output independently. This commit does not yet change the view layer.
3b. **Commit 3b: Swap the view** — Replace `view_watchlist_body()` to call `midas_grid::grid()`. Remove dead code (`grid_cell`, `grid_header_cell` helpers). Deprecate `watchlist::SortDirection` and `watchlist::SortColumn` in favor of `midas_grid::SortDirection` and `midas_grid::ColumnId`.

**Rationale for 3a/3b split**: Reduces blast radius — if a column implementation has a bug in `cell()` or `header()`, Commit 3a catches it via unit tests before the view swap in 3b. Rolling back 3b restores the old view while keeping the tested column enum.

> **Resize migration note**: The existing resize uses a `f32::NAN` sentinel for `start_x` (set on press, replaced with real cursor position on first move). This workaround exists because iced 0.14's `mouse_area::on_press` does not provide cursor coordinates. The grid's `ResizeState` replaces this with `Option<f32>` — see `00-architecture.md` Section 6.4.

### Parallelism Within Phase 0

```
   column.rs + style.rs     (independent, can be done in parallel)
         |           |                              Pre-Phase 2 Spike
         v           v                              (standalone iced prototype,
      state.rs   message.rs  (depends on column.rs   runs concurrently with all
         |       for ColumnId)|                       Phase 0 work — see Phase 2
         |           |                               section for details)
         +-----------+
               |
               v
     header.rs + body.rs     (both depend on column.rs + state.rs, independent of each other)
         |           |
         +-----------+
               |
               v
          widget.rs          (composes header + body; cells emit M, chrome maps via on_grid callback)
               |
               v
    WatchlistColumn enum     (in midas-app, implements GridColumn trait)
               |
               v
    views.rs + app.rs        (integration commits 2 & 3)
```

### Estimated Complexity

- 5 new files with types in `midas-grid` (state, column, style, header, body)
- 1 module placeholder (columns/mod.rs)
- 1 widget composition file
- 1 `WatchlistColumn` enum impl in `midas-app`
- ~12 significant types/structs
- ~20 functions
- ~1000 lines of new code: ~400 lines extracted/adapted from `views.rs` and `watchlist.rs`, ~600 lines of new generic grid infrastructure (trait definitions, state management, widget composition)

### Acceptance Criteria

- [ ] `cargo build --workspace` succeeds with no warnings.
- [ ] `cargo clippy --workspace -- -D warnings` passes.
- [ ] `cargo test --workspace` passes, including new grid unit tests.
- [ ] The watchlist panel renders identically to the pre-refactor version.
- [ ] Column resize by dragging header dividers works as before. (**Note**: Phase 0 migrates the existing inline `mouse_area`-based resize code from `views.rs` as-is, without the formal `ResizeState` machine. Phase 1 replaces this inline code with the proper `ResizeState` state machine in `interactions/resize.rs`.)
- [ ] Sort by clicking column headers works with direction indicators.
- [ ] The `SortToggled` handler clears `GridState.selection` after re-sorting to prevent stale index lookups. The app's `selected_symbol: Option<String>` remains the authoritative selection identity and is used to re-select the correct row index in the next `view()` call.
- [ ] Favorites-first pinning is preserved: favorited tickers always float to top regardless of sort column.
- [ ] Row selection with highlight works.
- [ ] Favorite toggle, delete button, drag grip button all work.
- [ ] Add-ticker input and submit work.
- [ ] Config save/load preserves column widths.
- [ ] No `unwrap()` in any grid module file (library-quality code).
- **Selection identity**: Row selection uses `SelectionState` with `Option<usize>` for rendering highlights (single selection only in Phase 0). `WatchlistPanel` continues to maintain `selected_symbol: Option<String>` as the authoritative selection identity through Phase 0. The app's `update()` handler maps `GridMessage::RowSelected(idx)` to a symbol lookup in the sorted data slice, storing the result in `selected_symbol`. On re-sort, the grid's index-based highlight may briefly point to a different row until the next view rebuild, but the app-level symbol identity is never lost. This avoids regressing from the current symbol-based selection. Selection remains index-based with `selected_symbol: Option<String>` as authoritative identity through Phase 0-2. The brief visual artifact after re-sort (index points to wrong row until next `view()`) is accepted for Phase 0-2. Phase 3a introduces `RowKey` alongside multi-selection, replacing `Option<usize>` with `BTreeSet<RowKey>`.

---

## Phase 1: Core Interactions

**Goal**: Add column resize as a proper interaction state machine, click-to-sort with three-state cycling (asc / desc / none), single row selection with symbol linking, and interactive cells (buttons that emit messages). This phase brings the grid to feature parity with the current watchlist plus proper resize and sort.

**Dependencies on previous phases**: Phase 0 must be complete. The `GridColumn` trait, `GridState`, and the widget composition function must exist.

### Files to Create

| File | Purpose |
|------|---------|
| `crates/midas-grid/src/interactions/mod.rs` | Re-exports sub-modules. Shared interaction types (drag threshold constant, coordinate helpers). Transient interaction state uses the unified `ActiveInteraction` enum on `GridState` (see 00-architecture.md §2.1), providing compile-time mutual exclusion. |
| `crates/midas-grid/src/interactions/resize.rs` | `ResizeState` struct, `handle_resize_start()`, `handle_resize_move()`, `handle_resize_end()` pure functions. |
| `crates/midas-grid/src/interactions/sort.rs` | `handle_sort_click()`: cycles None -> Default -> Opposite -> None, using `DefaultSortDirection` from the column. Emits `SortSpec` or clears sort. |
| `crates/midas-grid/src/interactions/selection.rs` | Interaction handlers for the `SelectionState` defined in Phase 0 (`selected: Option<usize>`, `focused: Option<usize>`): `handle_row_click()`, `handle_row_press()`. Phase 1 uses single selection only. |

### Files to Modify

| File | Change |
|------|--------|
| `crates/midas-grid/src/state.rs` | Add the `Resizing(ResizeState)` variant to the `ActiveInteraction` enum on `GridState` (matching canonical definition in 00-architecture.md §2.1). Selection is already in `GridState.selection: SelectionState` from Phase 0. |
| `crates/midas-grid/src/widget.rs` | Add resize overlay logic (currently the `stack![]` overlay block following the resize-handle `mouse_area` in `views.rs`). When `interaction` is `ActiveInteraction::Resizing`, render a full-area `mouse_area` overlay that captures mouse move/release. |
| `crates/midas-grid/src/header.rs` | Resize handles emit grid-level messages instead of app-level `WatchlistColumnResizeStart`. Sort buttons use `handle_sort_click` logic. |
| `crates/midas-grid/src/body.rs` | Row click emits selection message. Selected row gets highlighted background. |
| `crates/midas-app/src/app.rs` | Simplify watchlist message variants: `WatchlistColumnResizeStart/Resizing/End` are replaced by `WatchlistGrid(WatchlistId, GridMessage)`. The grid emits `GridMessage` variants; the app dispatches to the correct `GridState`. |
| `crates/midas-app/src/app/views.rs` | Remove inline resize overlay logic (now inside grid widget). Remove inline sort handler (now in grid). |

### Types to Define

| Type | Location | Description |
|------|----------|-------------|
| `ActiveInteraction::Resizing(ResizeState)` | `state.rs` | Phase 1 adds the `Resizing(ResizeState)` variant to the `ActiveInteraction` enum (which was defined with only `None` in Phase 0). Only one transient interaction can be active at a time -- this is a compile-time guarantee via the enum. Phase 2 adds `DraggingColumn(ColumnDragState)` and `DraggingRow(RowDragState)` variants. See 01-interactions.md §12 for the conflict resolution rules. |
| `ResizeState` | `interactions/resize.rs` | `struct ResizeState { column_id: ColumnId, start_x: Option<f32>, start_width: f32 }` -- `start_x` is `Option<f32>` (None until first mouse move, replacing the NAN sentinel), `start_width` is the column width when resize began. See 00-architecture.md §6.4. |
| (no new type) | `interactions/selection.rs` | `SelectionState` already exists from Phase 0 in `state.rs` with the simple shape (`selected: Option<usize>`, `focused: Option<usize>`). Phase 1 creates `interactions/selection.rs` with handler functions (`handle_row_click`, `handle_row_press`) that operate on the existing `SelectionState`. Single-selection only in Phase 0-2. |
| `GridMessage` | `message.rs` | Extend with resize variants: `ResizeStarted(ColumnId), Resizing(f32), ResizeEnded`. No cursor position in `ResizeStarted` — iced 0.14's `mouse_area::on_press` does not provide it; `start_x` is set on first `Resizing` move. `SortToggled` and `RowSelected` already exist from Phase 0. Phase 3 adds `RowToggled(usize)` and `RowRangeSelected(usize)` for multi-select. |
| ~~`RowKey`~~ | ~~`state.rs`~~ | **Deferred to Phase 3a.** `RowKey` is introduced in Phase 3a alongside multi-selection, where it is required for `BTreeSet<RowKey>` persistence across re-sorts. In Phase 0-2, selection remains index-based with `selected_symbol: Option<String>` as authoritative identity (see Phase 0 acceptance criteria). The brief visual artifact after re-sort (index points to wrong row until next `view()`) is accepted for Phase 0-2. |

### Key Implementation Steps

1. **Extend `GridMessage` enum** in `message.rs` with resize and selection variants. The Phase 0 `SortToggled` and `RowSelected` variants already exist.

2. **Implement `ResizeState`** in `interactions/resize.rs`:
   - `start(column_id, cursor_x, current_width) -> ResizeState`
   - `update(cursor_x) -> (ColumnId, new_width)` -- clamps to `min_width`.
   - `finish() -> (ColumnId, final_width)`.
   - These are pure functions; the widget calls them and updates `GridState`.

3. **Implement sort cycling** in `interactions/sort.rs`:
   - `cycle_sort(current: Option<SortSpec>, clicked: ColumnId, default_dir: SortDirection) -> Option<SortSpec>`.
   - If `current` column matches `clicked`: Default -> Opposite -> None.
   - If different column: start at `default_dir` for the new column.
   - Returns the new sort spec; the app uses it to re-sort data.
   - **Deliberate UX change (Phase 1 only, not Phase 0)**: The current watchlist uses a two-state toggle (Asc/Desc only, never clears). Phase 0 preserves the existing two-state toggle for feature parity. Phase 1 introduces:
     1. **Three-state cycle**: Default -> Opposite -> None (clearing sort is now possible).
     2. **`DefaultSortDirection`**: Each column declares its preferred first-click direction via a new `GridColumn` trait method `fn default_sort_direction(&self) -> SortDirection { SortDirection::Ascending }`. Numeric columns (Price, Change%, G.ATR) override this to return `Descending` — traders want biggest movers at the top. Text columns (Ticker) keep the default `Ascending` (A-Z). This aligns with the behavior specified in 01-interactions.md §6.2.
   - **Phase 1 deliverable** (not deferred to Phase 4): `DefaultSortDirection` is implemented in Phase 1 alongside the three-state cycle, since the sort interaction module is being built from scratch. Deferring it would ship a deliberate regression for trading UX.

4. **Implement selection handlers** in `interactions/selection.rs`:
   - `handle_row_click(&mut SelectionState, row_index)` -- calls `select(index)` on the Phase 0 `SelectionState` (simple `Option<usize>` single-selection).
   - `handle_row_press(&mut SelectionState, row_index)` -- for press-and-hold interactions.
   - `clear()` -- sets `selected` and `focused` to `None`.

5. **Update `widget.rs`** to handle the resize overlay internally. When `GridState.interaction` is `ActiveInteraction::Resizing(ResizeState)`, the grid widget wraps its content in a `stack![]` with a transparent `mouse_area` overlay that captures `on_move` and `on_release`, converting them to `GridMessage::Resizing` and `GridMessage::ResizeEnded`.

6. **Update `app.rs`**: The `Message::WatchlistGrid(WatchlistId, GridMessage)` variant already exists from Phase 0. Extend the match arm to handle new variants (`ResizeStarted`, `Resizing`, `ResizeEnded`, `RowSelected`). Each arm updates `GridState` and performs app-side effects (persist widths, propagate symbol link).

7. **Symbol linking on row select**: When `GridMessage::RowSelected(row_index)` is received, the app looks up the ticker at that index in the sorted list and propagates via the existing `symbol_link` mechanism. This replaces the current `WatchlistTickerSelected` handler.

8. **Selection remains index-based**: `RowKey` is deferred to Phase 3a (where it is required for multi-selection with `BTreeSet<RowKey>`). In Phase 0-2, selection uses `Option<usize>` with `selected_symbol: Option<String>` as authoritative identity (see Phase 0). The brief visual artifact after re-sort (index points to wrong row until next `view()`) is accepted for Phase 0-2.

### Testing Strategy

- **Unit test `cycle_sort`**: All transitions (None->Default, Default->Opposite, Opposite->None, different column).
- **Unit test `DefaultSortDirection`**: Numeric column starts Descending, text column starts Ascending.
- **Unit test `ResizeState`**: Start, update with movement, clamp at min_width.
- **Unit test `SelectionState`**: Select, re-select same (deselect?), select different.
- **Integration test**: Construct a grid with sort-enabled columns, simulate sort click sequence, verify `GridState` sort spec.
- **Manual test**: Resize columns in the running app, verify widths persist. Click sort headers, verify arrow indicators cycle correctly. Click rows, verify selection highlight and symbol linking.

### Migration Plan

1. Implement `interactions/` module and `GridMessage`.
2. Update `grid/widget.rs` to use the new interaction system.
3. Replace watchlist-specific message variants (`WatchlistColumnResizeStart`, `WatchlistColumnResizing`, `WatchlistColumnResizeEnd`, `WatchlistTickerSelected`) with the generic `Message::WatchlistGrid(WatchlistId, GridMessage)`. Note: sort column switching is currently handled inline in `views.rs` header button callbacks (no dedicated `WatchlistSortBy` message variant exists); the grid's `SortToggled` message replaces this inline logic.
4. Remove `resizing_column: Option<(WatchlistId, usize, f32, f32)>` from `MidasApp` -- this state now lives in `GridState.interaction: ActiveInteraction::Resizing(ResizeState)`.

### Estimated Complexity

- 4 new files (interactions module + 3 state machines)
- ~8 new types
- ~15 new functions
- ~600 lines of new code
- ~200 lines removed from `views.rs` and `app.rs`

### Acceptance Criteria

- [ ] Column resize works via drag, with cursor changing to `ResizingHorizontally`.
- [ ] Resize overlay captures mouse globally (no losing the drag if cursor moves off the handle).
- [ ] Sort cycles through Default -> Opposite -> None on repeated clicks (three-state, deliberate change from Phase 0's two-state toggle).
- [ ] Numeric columns (Price, Change%, G.ATR) start Descending on first click; text columns (Ticker) start Ascending. Controlled by `GridColumn::default_sort_direction()`.
- [ ] Sort arrows display correctly in headers.
- [ ] Clicking a different column switches sort to that column ascending.
- [ ] Row selection highlights the row and triggers symbol linking.
- [ ] Favorite toggle and delete button still work in cells.
- [ ] `MidasApp` no longer has `resizing_column` field.
- [ ] All `WatchlistColumnResize*` message variants removed.
- [ ] Config persistence still saves/restores column widths.
- [ ] Selection remains index-based (`Option<usize>`) with `selected_symbol: Option<String>` as authoritative identity (consistent with Phase 0). `RowKey` is deferred to Phase 3a.

---

## Phase 2: Drag & Drop

**Goal**: Column reorder by dragging headers, and row reorder by dragging row handles. Both with visual feedback (drag ghosts, drop indicators).

**Dependencies on previous phases**: Phase 1 must be complete. The interaction state fields on `GridState` and the event handling pattern in `widget.rs` must exist.

**State ownership note**: During the Widget transition, transient interaction state (`resize`, `column_drag`, `row_drag`) stays in `GridState` (app-owned), because the app's `update()` must read and mutate it. `GridWidgetState` (widget-internal, created by `Widget::state()`) holds only ephemeral rendering state such as hover positions and animation frame data that do not need to survive across `view()` calls. See 02-rendering.md Section 10 for the full `GridWidgetState` definition.

### Files to Create

| File | Purpose |
|------|---------|
| `crates/midas-grid/src/interactions/reorder.rs` | Column reorder state machine: `ColumnReorderState { source: ColumnId, cursor_x, ghost_offset }`. |
| `crates/midas-grid/src/interactions/drag.rs` | Row drag state machine: `RowDragState { source_index, cursor_y, ghost_offset }`. |
| `crates/midas-grid/src/render.rs` | Custom rendering helpers: `draw_drop_indicator()` (2px colored line), `draw_drag_ghost()` (semi-transparent overlay). |
| `crates/midas-grid/src/columns/drag_handle.rs` | `DragHandleColumn<T, M>`: renders a grip icon that initiates row drag. |

### Files to Modify

| File | Change |
|------|--------|
| `crates/midas-grid/src/interactions/mod.rs` | Add sub-module re-exports for `reorder.rs` and `drag.rs`. |
| `crates/midas-grid/src/state.rs` | Add `DraggingColumn(ColumnDragState)` and `DraggingRow(RowDragState)` variants to the `ActiveInteraction` enum on `GridState` (matching canonical definition). Add `reorder_columns(source: ColumnId, target_index: usize)` method. Add `column_display_order() -> Vec<ColumnId>`. |
| `crates/midas-grid/src/widget.rs` | **Architectural transition**: Rewrite from a composition function (Phase 0-1) to a custom `Widget<M>` impl. This is required to support `Widget::overlay()` for drag ghosts that render above all sibling widgets. Estimated additional complexity: ~500-700 lines for `layout()`, `draw()`, `update()`, `mouse_interaction()`, `overlay()`, and child widget tree state management (`Tag`, `State`, `children()`, event forwarding). This is the largest risk in Phase 2 — see the **Pre-Phase 2 spike** below. When `interaction` is `DraggingColumn` or `DraggingRow`, render the drag ghost via `overlay()` and the drop indicator at the calculated insertion point. **No message mapping refactor needed**: The `Widget<M>` stores the `on_grid` callback as `Box<dyn Fn(GridMessage) -> M + 'a>` and calls it in `update()` via `shell.publish((self.on_grid)(grid_msg))`. Cell elements are already `Element<'a, M>` — they emit M directly. The two-path message design (cells emit M, chrome maps via `on_grid`) works identically for composition functions and custom Widgets. |
| `crates/midas-grid/src/header.rs` | Header cells become draggable: `mouse_area` wrapping each header cell detects press-and-drag (threshold: 5px movement before transitioning from click to drag). |
| `crates/midas-grid/src/message.rs` | Add `GridMessage::ColumnDragStart`, `ColumnDragging(f32)`, `ColumnDragEnd`, `RowDragStart(usize)`, `RowDragging(f32)`, `RowDragEnd`. |
| `crates/midas-app/src/app.rs` | Handle `GridMessage::ColumnDragEnded` and `GridMessage::RowDragEnded` in the existing `WatchlistGrid` match arm. For columns, update `GridState.column_order`. For rows, reorder `WatchlistPanel::tickers` vec. |
| `crates/midas-app/src/watchlist.rs` | Add `reorder_ticker(from: usize, to: usize)` method. |

### Types to Define

| Type | Location | Description |
|------|----------|-------------|
| `ColumnReorderState` | `interactions/reorder.rs` | `struct { source_id: ColumnId, start_x: f32, current_x: f32, drop_target_index: Option<usize> }` |
| `RowDragState` | `interactions/drag.rs` | `struct { source_index: usize, start_y: f32, current_y: f32, drop_target_index: Option<usize> }` |
| `DropIndicator` | `render.rs` | `struct { position: f32, orientation: Horizontal or Vertical, color: Color }` |

### Pre-Phase 2 Spike: Custom Widget Validation (1-2 days) -- Run Concurrently with Phase 0

> **Scheduling**: This spike runs **in parallel with Phase 0**, not after it. Running it
> early (1-2 days) informs Phase 2 architecture before committing to Phases 0-1. If the
> spike fails, Phase 2 can be re-scoped during Phase 0-1 development rather than after.
> The spike has zero dependency on Phase 0 deliverables -- it uses a standalone iced
> prototype, not the midas-grid crate.

Before implementing drag-and-drop, validate that a custom `Widget` can compose inner
`Element` trees (from `header()` and `body()` helpers) while correctly forwarding events,
managing child widget tree state, and rendering an overlay.

**Build a minimal prototype**:
- A custom `Widget` struct with a fixed header row (2 columns) and 2 body rows with clickable buttons.
- `layout()` delegates to child elements.
- `draw()` renders all children.
- `update()` forwards `Event::Mouse` and `Event::Keyboard` to children.
- `overlay()` returns a simple positioned overlay element.

**Pass/fail criteria**:
- **Pass** if all five conditions hold:
  1. Buttons inside cells receive click events correctly (press + release both dispatched to the correct child widget).
  2. `overlay()` renders an element positioned above siblings at arbitrary coordinates (not clipped to the widget's own layout bounds).
  3. Event forwarding to child elements works without manual `Tree`/`State` management hacks or reliance on undocumented iced internals.
  4. Child element count changes between frames (add/remove rows dynamically) without panic or state corruption — validates `Tree::diff()` with dynamic children.
  5. A `text_input` widget inside a cell retains its cursor position and content across frame rebuilds — validates child widget state persistence.
- **Fail** if any of the above require more than ~50 lines of `unsafe` code or depend on undocumented iced internals (i.e., behavior not covered by iced's public API docs or official examples).

**If the spike fails**: The `stack![]` fallback becomes the **primary** approach for Phase 2.
Phase 2 acceptance criteria for drag ghost visuals should be relaxed: ghosts clipped to
grid bounds is acceptable (no overlay escape required). `render.rs` would provide drop
indicators and ghost rendering within the grid's own bounds using `stack![]` layering.
If the spike fails, the custom Widget transition becomes **Phase 3b's first deliverable** (not deferred beyond it). Phase 3b would begin with the Widget transition as its first task, followed by virtual scrolling and keyboard navigation. This extends Phase 3b's estimated timeline by ~800-1100 lines (child widget tree management — `children()`, `diff()`, `state()`, event forwarding with translated bounds — accounts for the increase) but preserves the overall dependency chain. Phase 2 proceeds with the `stack![]` fallback for drag overlays.

**Phase 2 under Plan B (commit decomposition)**: If the spike fails, the three-commit
structure adapts as follows:

1. **Commit 1: `stack![]` drag overlay infrastructure** — Add `render.rs` with `draw_drop_indicator()` and `draw_drag_ghost()` helpers that render within the grid's `stack![]` bounds. Add `DragOverlayState` to track ghost position and opacity. Acceptance criteria: a colored rectangle renders at an arbitrary position within the grid using `stack![]` + padding-based positioning.
2. **Commit 2: Column reorder** — Same as Plan A Commit 2, but drag ghost is clipped to grid bounds (acceptable visual limitation).
3. **Commit 3: Row drag-and-drop** — Same as Plan A Commit 3, with the same clipping constraint.

**Post-Spike Checkpoint: Heavy Spike (1 day)**: If the minimal spike passes, build a
production-complexity prototype before committing to the full Commit 1 rewrite. The
minimal spike uses 2 columns and 2 rows; production has 7 columns, dynamic row counts,
nested interactive widgets (buttons, toggles), and the existing resize overlay. The
heavy spike validates that `Tree::diff()` handles dynamic children at scale, that nested
buttons receive events correctly in a 7-column layout, and that the resize overlay
coexists with the custom Widget. If the heavy spike fails on issues the minimal spike
missed, adopt Plan B before investing ~800-1100 lines in Commit 1.

**Plan C — if custom Widget composition proves unworkable in both the spike AND the Phase 3b retry**: If the custom `Widget` approach is fundamentally incompatible with iced 0.14's child widget tree model (e.g., `Tree::diff()` cannot handle dynamic children without unsound hacks), the project accepts the following permanent constraints:
- Drag ghosts and drop indicators remain implemented via `stack![]` overlays, accepting that ghosts are clipped to grid bounds rather than floating above siblings.
- Context menus (Phase 4) use iced's built-in `overlay::Element` on the host `Container` rather than the grid's own `Widget::overlay()`.
- Virtual scrolling (Phase 3b) is unaffected — it depends on `Widget::layout()` / `Widget::draw()`, not `Widget::overlay()`.
- This bounds the visual degradation to drag previews only; all functional behavior is preserved. The grid remains a composition function returning `Element` rather than a custom `Widget` impl.

### Key Implementation Steps

1. **Column reorder state machine** (`reorder.rs`):
   - `start(column_id, cursor_x)`: Record source column and start position.
   - `update(cursor_x, column_positions: &[(ColumnId, f32, f32)])`: Calculate which gap the cursor is over. Return `drop_target_index`.
   - `finish()`: Return `(source_id, target_index)` for the reorder operation.
   - `cancel()`: Return to idle with no changes.
   - Drag threshold: 5px horizontal movement before transitioning from potential-click to drag.

2. **Column drag ghost rendering**: When `ReorderingColumn` is active, render a semi-transparent container with the column header text, positioned at cursor_x with a small offset. Use `stack![]` overlay layer. Ghost style: 90% opacity, 2px drop shadow (via border), 1.02x scale is not feasible in iced so use a subtle border highlight instead.

3. **Column drop indicator**: Render a 2px-wide vertical colored line between column headers at the calculated drop position. Use a positioned `Space` or `container` within the header `Row`.

4. **Row drag state machine** (`drag.rs`):
   - `start(row_index, cursor_y)`: Record source row and start position.
   - `update(cursor_y, row_height: f32, total_rows: usize)`: Calculate target insertion index from cursor position relative to row positions.
   - `finish()`: Return `(source_index, target_index)`.
   - `cancel()`: Return to idle.
   - Drag threshold: 5px vertical movement (matches shared 5px threshold in 01-interactions.md §1.3).

5. **Row drag ghost**: Render a semi-transparent copy of the dragged row's content at the cursor's y-position. The source row dims to 30% opacity.

6. **Row drag vs. ticker-to-chart drag disambiguation**: Both use the grip icon as the
   drag handle. Disambiguation rule: dragging within the grid body area triggers row
   reorder; dragging the row outside the grid bounds (onto a chart pane drop target)
   triggers cross-panel ticker drag (existing `WatchlistDragStart` behavior). The
   grid emits `RowDragExternal` when the cursor leaves the grid bounds during a row
   drag. The app handles this by initiating the existing ticker-to-chart flow.
   When a sort is active, row reorder drag is disabled (grip icon grayed out) since
   reordering a sorted list is meaningless.

7. **Row drop indicator**: Render a 2px-high horizontal colored line between rows at the insertion point.

8. **Press vs. drag disambiguation**: The header `mouse_area` must distinguish between a click (for sort) and a drag (for reorder). Strategy: on `on_press`, record the position but do NOT enter reorder mode. On `on_move`, if distance exceeds threshold, transition to reorder. On `on_release` without exceeding threshold, treat as click (sort). Same pattern for row drag handles vs. other row interactions.

9. **Column display order tracking**: `GridState` maintains `column_order: Vec<ColumnId>` representing the visual order. `reorder_columns(source, target_index)` removes `source` and inserts at `target_index`. All rendering iterates `column_order` to determine left-to-right layout.

### Testing Strategy

- **Unit test `ColumnReorderState`**: Start drag, update positions, verify drop target calculation at column boundaries.
- **Unit test `RowDragState`**: Start drag, update positions, verify insertion index calculation.
- **Unit test `GridState::reorder_columns`**: Move column from index 0 to 3, verify order. Move last to first. Move to same position (no-op).
- **Unit test `WatchlistPanel::reorder_ticker`**: Move ticker from index 1 to index 4. Verify vec order.
- **Unit test drag threshold**: Verify that small movements (< 5px) do not trigger drag.
- **Manual test**: Drag a column header, verify ghost follows cursor, verify drop indicator appears between correct columns, verify column order changes on drop. Same for row drag.

### Migration Plan (Commit Decomposition)

Phase 2 is the largest delivery (~1500-1900 lines). The migration is split into **3 commits** to reduce blast radius:

1. **Commit 1: Widget transition** — Rewrite `widget.rs` from a composition function to a custom `Widget<M>` impl with identical visual output and no new features. The `Widget<M>` stores the `on_grid` callback as `Box<dyn Fn(GridMessage) -> M + 'a>`. Cell elements remain `Element<'a, M>`. No message-type refactor is needed because the grid is generic over `M` from Phase 0. All existing tests must pass. This is the highest-risk commit and should be reviewed carefully before proceeding.
   **Commit 1 intermediate acceptance criteria** (must all pass before proceeding to Commit 2):
   - [ ] All existing `GridState` unit tests pass unchanged.
   - [ ] The Phase 0 manual test checklist (see Phase 0 Acceptance Criteria) passes — re-run every item.
   - [ ] `text_input` state persistence: the add-ticker input retains cursor position and entered text across frame rebuilds (validates child widget state persistence in the custom Widget tree).
   - [ ] Interactive cells: favorite toggle and delete button still receive click events correctly (validates event forwarding to child widgets).
   - [ ] No visual regression in header/body alignment, row backgrounds, or sort indicators.

2. **Commit 2: Column reorder** — Implement `interactions/reorder.rs` with `ColumnDragState`, drag detection, header drag handlers, drop indicators, and column reorder logic in `state.rs`. Add `DraggingColumn(ColumnDragState)` variant to `ActiveInteraction`. Wire up `GridMessage::ColumnDragEnded` in `app.rs`. New tests for column reorder state machine and `GridState::reorder_columns`.

3. **Commit 3: Row drag-and-drop** — Implement `interactions/drag.rs` with `RowDragState`, row drag detection, `DragHandleColumn`, drop indicators, and row reorder. Add `DraggingRow(RowDragState)` variant to `ActiveInteraction`. Implement row drag vs. ticker-to-chart drag disambiguation. Wire up `GridMessage::RowDragEnded` in `app.rs`. New tests for row DnD state machine. The existing ticker-to-chart drag feature (dragging FROM watchlist TO chart pane) must coexist: the `DragHandleColumn` initiates a row reorder within the watchlist, while the existing `WatchlistDragStart` mechanism is triggered by dragging a row onto a chart pane. Decision: keep both. Row drag reorder within the grid is Phase 2. Cross-panel ticker drag remains on the existing mechanism.

### Estimated Complexity

- 4 new files
- ~6 new types
- ~20 new functions
- ~1500-1900 lines of new code (including ~800-1100 for the Widget transition; child widget tree management — `children()`, `diff()`, `state()`, event forwarding with translated bounds — accounts for the increase over the original estimate)

### Acceptance Criteria

- [ ] Dragging a column header moves the column to a new position.
- [ ] A floating ghost with the column name follows the cursor during drag.
- [ ] A 2px vertical drop indicator appears at valid drop positions.
- [ ] Columns that cannot be reordered (drag handle, delete) stay in place.
- [ ] Row drag via grip handle reorders tickers within the watchlist.
- [ ] A semi-transparent row ghost follows the cursor during row drag.
- [ ] A 2px horizontal drop indicator appears between rows.
- [ ] Source row dims during drag.
- [ ] Quick clicks on headers still trigger sort (drag threshold not exceeded).
- [ ] Column order is persisted across sessions.
- [ ] Row reorder is persisted (tickers vec order saved to config).
- [ ] Dragging a row handle outside the grid boundary initiates the existing ticker-to-chart drop flow.
- [ ] Escape key cancels any active drag.

---

## Phase 3: Polish & Performance

**Goal**: Virtual scrolling for large datasets, flash-on-tick animation, conditional formatting, keyboard navigation, multi-row selection, and column configuration persistence.

**Sub-phase decomposition**: Phase 3 is split into two sub-phases with different dependency chains:

- **Phase 3a (Widget-independent)**: Flash-on-tick, conditional formatting, multi-selection expansion, column persistence. Depends on **Phase 1 only**. Can begin as soon as Phase 1 is complete, even if Phase 2 is still in progress.
- **Phase 3b (Widget-dependent)**: Virtual scrolling, keyboard navigation. Depends on **Phase 2** (requires the custom `Widget` impl for `Event::Keyboard` dispatch and body composition changes).

**Dependencies on previous phases**: Phase 1 must be complete for Phase 3a. Phase 2 must be complete for Phase 3b.

**Parallelism boundary with Phase 2**: `widget.rs` is a serialization point. Phase 2
rewrites it from a composition function into a custom `Widget` impl. Phase 3b features
that modify `widget.rs` (keyboard event dispatch, virtual scrolling body composition)
**cannot** begin until the Phase 2 Widget transition is settled.
Phase 3a is further divided into **parallel** and **sequential** sub-groups based on
actual file-level conflicts with Phase 2:

| Feature | Sub-group | Can parallelize with Phase 2? | Why |
|---|---|---|---|
| Flash-on-tick pure functions (`flash.rs`) | 3a-parallel | Yes | `flash.rs` is a new file with pure functions (interpolation, color blending). No file-level conflict with Phase 2. |
| Flash-on-tick state wiring (`state.rs`, `message.rs`) | 3a-sequential | No | Adds `flash_state` field to `GridState` in `state.rs` and `FlashTick` variant to `message.rs`. Phase 2 also modifies both files (adds `ActiveInteraction` variants and drag messages). Concurrent branches produce merge conflicts. |
| Conditional formatting (`style.rs`, `column.rs`) | 3a-parallel | Yes | Only adds trait methods and style types in files Phase 2 does not touch |
| Multi-selection (`interactions/selection.rs`, `state.rs`) | 3a-sequential | No | Replaces `SelectionState` (from `Option<usize>` to `BTreeSet<RowKey>`) in `state.rs`. Phase 2 references selection by index. Must be sequenced AFTER Phase 2 commits. |
| Column persistence (`persistence.rs`, `config.rs`) | 3a-parallel | Yes | Only touches serialization, no widget code |
| Virtual scrolling (`body.rs`, `widget.rs`) | 3b | No | Modifies body composition and widget layout |
| Keyboard navigation (`interactions/keyboard.rs`, `widget.rs`) | 3b | No | Requires `Event::Keyboard` dispatch in custom Widget |

**Merge protocol for `state.rs` / `message.rs`**: These files are modified by every
phase and are serialization chokepoints for parallel work. Concrete workflow:

1. **Phase 3a-parallel** branches off `main` after Phase 1 merges. Work in `flash.rs`,
   `style.rs`, `persistence.rs`, `config.rs` — no `state.rs`/`message.rs` conflicts.
2. **Phase 2** merges to `main` first (it is on the critical path).
3. **Phase 3a-parallel** merges to `main` next. If it touched no shared files, this is
   conflict-free. If minor imports changed, resolve trivially.
4. **Phase 3a-sequential** branches off `main` AFTER both Phase 2 and Phase 3a-parallel
   have merged. This avoids merge conflicts entirely — sequential work starts from a
   clean base that already contains both Phase 2's `ActiveInteraction` variants and
   Phase 3a-parallel's flash pure functions.
5. For `state.rs`: new fields are always appended to the `GridState` struct
   (never inserted mid-struct). For `message.rs`: new enum variants are appended to the
   end of `GridMessage`. This minimizes diff overlap if two branches must merge concurrently.
6. Use **merge commits** (not squash) for phase branches so that individual commit history
   is preserved for debugging regressions.

> **Pre-Phase 3b spike (2-4 hours)**: Before implementing virtual scrolling, validate
> that iced 0.14's `Scrollable` correctly handles spacer-based virtual scrolling.
> Build a minimal test: `Column[Space(top), 8 visible rows, Space(bottom)]` inside a
> `Scrollable`. Verify scrollbar thumb size, scroll-to-position accuracy, and scroll
> event emission. If it fails, virtual scrolling must use the custom `Widget`'s
> internal scroll management (see 02-rendering.md §3 fallback). This spike should run
> after the Phase 2 Widget transition is complete, since virtual scrolling depends on
> the custom Widget architecture.

### Phase 3a Files (Widget-independent -- can begin after Phase 1)

**Files to Create:**

| File | Purpose |
|------|---------|
| `crates/midas-grid/src/persistence.rs` | `GridColumnState` serde struct, `save_grid_state()`, `load_grid_state()` for TOML config. |
| `crates/midas-grid/src/flash.rs` | `flash_background()`: given `FlashState` and elapsed time, return interpolated background color (green/red -> transparent over 300ms). Separate from `render.rs` (which is Phase 2, drag/drop visuals) to avoid file-level conflicts. |

**Files to Modify:**

| File | Change |
|------|--------|
| `crates/midas-grid/src/state.rs` | Add `flash_state: HashMap<(RowKey, ColumnId), FlashState>` as a field of `GridState` (the canonical, app-owned state struct from 00-architecture.md). Uses `RowKey` (not `usize`) so flashes survive data re-sorts. This is the canonical flash state location. See also 02-rendering.md Section 4 for animation details. |
| `crates/midas-grid/src/style.rs` | Add `FlashStyle { positive_color, negative_color, duration_ms }`. Add conditional formatting types: `CellFormat { text_color, background_color }`, `FormatRule`. |
| `crates/midas-grid/src/interactions/selection.rs` | **Replace** the Phase 0 simple `SelectionState` (`Option<usize>`) with the full multi-selection struct: `SelectionState { selected: BTreeSet<RowKey>, anchor: Option<RowKey>, focused: Option<usize> }`. Introduce `RowKey` (`#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd)] pub struct RowKey(pub String)` with `RowKey::new(key: impl Into<String>)` constructor) in `state.rs`. The grid builder gains a required `.row_key(fn(&T) -> RowKey)` parameter for multi-selection. Add `handle_ctrl_click(key)`, `handle_shift_click(key, total_rows)` handler functions. Single-selection callers that do not need multi-select can continue using `select(key)` which clears the set and inserts one element. |
| `crates/midas-grid/src/column.rs` | Add optional method to `GridColumn` trait: `fn format(&self, row: &T) -> Option<CellFormat>` with default `None`. (Flash detection is app-side -- no `flash_value()` method needed on the trait. See flash-on-tick implementation step.) |
| `crates/midas-grid/src/message.rs` | Add `GridMessage::RowToggled(usize)`, `RowRangeSelected(usize)`, `FlashTick`. |
| `crates/midas-grid/src/lib.rs` | Add `pub mod` declarations for new modules (`flash`, `persistence`). |
| `crates/midas-core/src/config.rs` | Add `GridColumnConfig` struct to `WatchlistConfig` for persisting column order, widths, visibility. |
| `crates/midas-app/src/app.rs` | Add conditional `iced::time::every(Duration::from_millis(16))` subscription in `subscription()` when any grid has active flashes. Import grid state check. |

### Phase 3b Files (Widget-dependent -- requires Phase 2 complete)

**Files to Create:**

| File | Purpose |
|------|---------|
| `crates/midas-grid/src/interactions/keyboard.rs` | Keyboard event handler: arrow key navigation, Enter to activate, Delete to remove, Escape to deselect. |

**Files to Modify:**

| File | Change |
|------|--------|
| `crates/midas-grid/src/body.rs` | Virtual scrolling: calculate visible row range from scroll offset and viewport height, only create `Element` for visible rows. Use `Space` of calculated height above and below for correct scrollbar behavior. |
| `crates/midas-grid/src/state.rs` | Add `scroll_offset: f32`, `viewport_height: f32`, `focused_row: Option<usize>`. |
| `crates/midas-grid/src/widget.rs` | Handle keyboard events (`Event::Keyboard`). Dispatch to `keyboard.rs` handler. |
| `crates/midas-grid/src/message.rs` | Add `GridMessage::KeyPressed(Key)`, `ScrollChanged(f32)`, `ViewportResized(f32)`. |

### Types to Define

| Type | Location | Description |
|------|----------|-------------|
| `FlashState` | `state.rs` | See **02-rendering.md Section 4** for the canonical definition: `struct FlashState { start_time: Instant, duration_secs: f32, color: FlashColor, peak_alpha: f32 }`. The app detects changes externally and calls `grid_state.trigger_flash()` with the direction. The grid only manages animation state (timestamps, alpha decay). |
| `FlashDirection` | `state.rs` | `enum FlashDirection { Up, Down }` |
| `CellFormat` | `style.rs` | `struct CellFormat { text_color: Option<Color>, background: Option<Color> }` |
| `FormatRule` | `style.rs` | `enum FormatRule { Positive(CellFormat), Negative(CellFormat), Threshold { above: f64, format: CellFormat } }` |
| `GridColumnState` | `persistence.rs` | **Same as `ColumnConfig` from 03-column-data-model.md §3.1** — this is a re-export or type alias, not a new struct. Uses `ColumnConfig { id: String, visible: bool, width_override: Option<ColumnWidth>, order: usize, pinned: Option<PinSide> }` for serialization. Phase 3's `persistence.rs` provides `save_grid_state()` and `load_grid_state()` convenience functions that operate on the existing `GridConfig` / `ColumnConfig` types. |
| `VirtualScrollState` | `state.rs` | `struct VirtualScrollState { offset: f32, viewport_height: f32, row_height: f32, total_rows: usize }` with `visible_range() -> Range<usize>`. |
| `GridState::has_active_flashes()` | `state.rs` | `pub fn has_active_flashes(&self) -> bool` — returns `true` if any entry in `flash_state` has not yet expired. Used by `midas-app` to conditionally activate the 60fps redraw subscription. |

### Key Implementation Steps

1. **Virtual scrolling** in `body.rs`:
   - `GridState` tracks `scroll_offset` and `viewport_height`.
   - `visible_range(row_height, total_rows)` returns `start..end` indices.
   - The body function creates a `Space` with height `start * row_height` as a top spacer, then elements for visible rows, then a bottom `Space` with height `(total - end) * row_height`.
   - Wrap in `scrollable` with `on_scroll` callback to update `scroll_offset`.
   - Row height is uniform (required for O(1) index calculation). Default: 28px.

2. **Flash-on-tick** (Phase 3a):
   - **Headless principle**: The app (not the grid) detects value changes. The grid manages only animation state.
   - The app detects value changes when market data updates and calls `grid_state.trigger_flash(column_id, row_key, direction)` where `row_key` is obtained from the `.row_key()` closure (e.g., `RowKey::new(&row.symbol)`) and `direction` is `FlashDirection::Up` or `FlashDirection::Down`.
   - The grid does NOT maintain a `PreviousValues` map. Change detection is the app's responsibility (it already has the old and new market data).
   - `trigger_flash()` creates a `FlashState { direction, started_at: Instant::now() }` in `grid_state.flash_state: HashMap<(RowKey, ColumnId), FlashState>`. The app provides the `RowKey` via the same `.row_key()` closure used for multi-selection, ensuring flashes track the correct row across re-sorts.
   - A `GridMessage::FlashTick` fires every ~50ms (via iced subscription) while any flash is active, driving the animation.
   - During rendering, `flash_background(flash_state, now)` returns a color interpolated from full flash color to transparent over 300ms.
   - Flash color: green (`rgba(0.1, 0.8, 0.3, alpha)`) for price up, red (`rgba(0.9, 0.2, 0.2, alpha)`) for price down.
   - Alpha decays linearly: `1.0 - (elapsed_ms / 300.0)`.
   - Expired flash entries are garbage-collected on each `FlashTick`.
   - The `GridColumn` trait's `fn flash_value()` method is removed (not needed -- the app drives flash triggers externally).

3. **Conditional formatting**:
   - The `GridColumn` trait gains `fn format(&self, row: &T) -> Option<CellFormat>`.
   - `NumericColumn` provides a default implementation: positive values get green text, negative get red, zero gets muted. This replaces the inline color logic in the `change_color` / `pnl_color` selection block within `view_watchlist_body()` in `views.rs`.
   - Custom formatting is possible by implementing the trait method with arbitrary rules.

4. **Keyboard navigation** (`keyboard.rs`):
   - Arrow Up/Down: Move `focused_row` up/down. If selection mode is Single, also select.
   - Home/End: Jump to first/last row.
   - Enter: Emit `GridMessage::RowActivated(index)` (the app triggers symbol load).
   - Delete: Emit `GridMessage::RowsDeleted(Vec<usize>)` (the app removes the tickers).
   - Escape: Clear selection and focus.
   - The grid must be focusable. Use iced's focus system or track focus state manually.

5. **Multi-row selection** (`selection.rs`):
   - Ctrl+click: Toggle individual row in/out of selection set.
   - Shift+click: Select range from anchor to clicked row.
   - Click without modifier: Single select (clear others).
   - `SelectionState` is replaced from the Phase 0 simple struct (`Option<usize>`) with the full multi-selection shape: `BTreeSet<RowKey>` for selected rows (stable across re-sorts), `Option<RowKey>` for anchor. Resolves keys to indices during `view()` via the `.row_key()` function.
   - **`RowKey` introduced here**: `RowKey(pub String)` with `RowKey::new(key: impl Into<String>)` is defined in Phase 3a (deferred from earlier phases where index-based selection was sufficient). The grid builder gains a `.row_key(fn(&T) -> RowKey)` parameter. The watchlist provides `|row| RowKey::new(&row.symbol)`. This replaces `Option<usize>` with `BTreeSet<RowKey>` for both single and multi-selection.

6. **Column persistence** (`persistence.rs`):
   - `GridColumnState` serializes: `{ id, width, order, visible }`.
   - On save: `grid_state.to_column_states() -> Vec<GridColumnState>`.
   - On load: `GridState::from_column_states(states, default_columns)`.
   - Stored in the `WatchlistConfig` TOML section.
   - No backward-compatible dual-format loading needed (Phase 0 already writes the new `GridConfig` format directly, and no shipped releases exist with the old format).

### Testing Strategy

- **Unit test virtual scrolling**: Given 100 rows, row_height 28, viewport 200, scroll offset 0: visible range 0..8. Scroll offset 280: visible range 10..18. Edge cases: scroll past end, negative offset.
- **Unit test flash state**: Create flash, advance time by 150ms, verify alpha is ~0.5. Advance to 300ms, verify alpha is 0. Advance past 300ms, verify flash is expired.
- **Unit test multi-selection**: Click row 3 (anchor=3, selected={3}). Shift+click row 7 (selected={3,4,5,6,7}). Ctrl+click row 5 (selected={3,4,6,7}). Click row 1 (anchor=1, selected={1}).
- **Unit test keyboard navigation**: Focus row 3, arrow down -> focus row 4. Arrow up from 0 -> stays at 0. End -> last row.
- **Config roundtrip test**: Serialize grid state with reordered columns, deserialize, verify order matches.
- **Performance test**: Create grid with 10,000 rows. Measure time to compute visible range and build elements for visible rows. Target: < 1ms.
- **Visual regression testing**: Investigate iced's test rendering capabilities or screenshot-comparison approaches for automated visual regression. If no viable approach exists for iced 0.14, document the decision and continue with manual visual verification.

### Migration Plan

**Phase 3a-parallel** (can begin immediately after Phase 1, runs concurrently with Phase 2):
1. Add flash-on-tick pure functions in `flash.rs` (interpolation, color blending, alpha decay).
2. Add conditional formatting (trait methods in `column.rs`, style types in `style.rs`).
3. Add column persistence (`persistence.rs`, config structs).

**Phase 3a-sequential** (begins after Phase 2 merges — branches off `main` after Phase 2 + 3a-parallel merge):
4. Add multi-selection: introduce `RowKey` type and replace the Phase 0 simple `SelectionState` (`Option<usize>`) with `BTreeSet<RowKey>` in `interactions/selection.rs` and `state.rs`. **Must come first** — step 5 depends on the `RowKey` type.
5. Wire flash state into `GridState` (`state.rs`) using `HashMap<(RowKey, ColumnId), FlashState>` and add `FlashTick` to `GridMessage` (`message.rs`). Wire subscription in `app.rs`. Depends on step 4 for `RowKey`.

**Phase 3b** (begins after Phase 2 is complete):
5. Implement virtual scrolling (most impactful for performance, modifies `body.rs` + `widget.rs`). Phase 0's `scroll_y: f32` is replaced by `VirtualScrollState`, which subsumes it. The `scroll_y` field is removed from `GridState` and its value migrates to `VirtualScrollState.offset`.
6. Add keyboard navigation (requires `Event::Keyboard` dispatch in custom Widget).

### Estimated Complexity

**Phase 3a:**
- 2 new files (`flash.rs`, `persistence.rs`)
- ~6 new types
- ~18 new functions
- ~600 lines of new code
- Modifications across 6 existing files

**Phase 3b:**
- 1 new file (`interactions/keyboard.rs`)
- ~2 new types (`VirtualScrollState`, keyboard handler)
- ~12 new functions
- ~400 lines of new code
- Modifications across 4 existing files

**Combined:** ~3 new files, ~8 new types, ~30 new functions, ~1000 lines

### Acceptance Criteria

**Phase 3a (Widget-independent):**
- [ ] Price changes trigger a 300ms green/red flash on the affected cell.
- [ ] Positive change% values are green, negative are red.
- [ ] Ctrl+click toggles individual rows in multi-select.
- [ ] Shift+click selects a contiguous range.
- [ ] Column widths, order, and visibility persist across app restarts.
- [ ] Config loading works with the `GridConfig` format introduced in Phase 0.

**Phase 3b (Widget-dependent):**
- [ ] Grid with 1000+ rows scrolls smoothly (only visible rows rendered).
- [ ] Scrollbar size and position correctly reflect total row count.
- [ ] Arrow keys move selection up/down through rows.
- [ ] Enter on a selected row loads that symbol in the linked chart.
- [ ] Delete on a selected row removes the ticker.

---

## Phase 4: Advanced Features

**Goal**: Right-click context menu, column presets, auto-fit column width, column pinning, multi-column sort, and copy/paste support. These are professional-grade features that complete the trading terminal experience.

**Dependencies on previous phases**: Phase 3 is the nominal prerequisite, but several Phase 4 features have earlier dependency chains, enabling parallelism:

| Phase 4 feature | Actual dependency | Can begin after |
|---|---|---|
| Context menus | Phase 2 (requires `Widget::overlay()`) | Phase 2 |
| Multi-column sort | Phase 3b (replaces `Option<SortSpec>` with `Vec<SortSpec>` in `GridState.sort`) | Phase 3b |
| Copy/paste | Phase 3b (requires `Event::Keyboard` dispatch in `widget.rs`) | Phase 3b |
| Column presets | Phase 3a (extends persistence) | Phase 3a |
| Auto-fit column width | Phase 1 (extends resize) | Phase 1 |
| Column pinning | Phase 3b (benefits from virtual scrolling layout) | Phase 3b |

Features with earlier dependencies can begin in parallel with Phase 3 work.

### Files to Create

| File | Purpose |
|------|---------|
| `crates/midas-grid/src/context_menu.rs` | Context menu overlay: `ContextMenu` struct, `ContextMenuItem` enum, rendering as a floating `column` of buttons. |

### Files to Modify

| File | Change |
|------|--------|
| `crates/midas-grid/src/widget.rs` | Right-click handling: on `mouse::Button::Right` press, emit `GridMessage::ContextMenu(row_index, position)`. Render context menu overlay when active. |
| `crates/midas-grid/src/state.rs` | Add `context_menu: Option<ContextMenuState>`. Add `pinned_left: Vec<ColumnId>`, `pinned_right: Vec<ColumnId>`. Add `sort_specs: Vec<SortSpec>` (replacing single `Option<SortSpec>`). |
| `crates/midas-grid/src/interactions/sort.rs` | Multi-column sort: Shift+click adds secondary sort. Plain click replaces sort. Sort priority badges (1, 2, 3) rendered in headers. |
| `crates/midas-grid/src/header.rs` | Double-click on resize handle triggers auto-fit. Pinned columns render in a separate fixed container outside the scrollable area. |
| `crates/midas-grid/src/body.rs` | Pinned columns render in a fixed container; remaining columns scroll horizontally. |
| `crates/midas-grid/src/persistence.rs` | Add preset save/load: `GridPreset { name, columns: Vec<GridColumnState>, sort: Vec<SortSpec> }`. |
| `crates/midas-grid/src/message.rs` | Add `GridMessage::ContextMenuAction`, `AutoFitColumn(ColumnId)`, `PinColumn(ColumnId, PinSide)`, `UnpinColumn(ColumnId)`. |
| `crates/midas-core/src/config.rs` | Add `grid_presets: Vec<GridPresetConfig>` to app config. |

### Types to Define

| Type | Location | Description |
|------|----------|-------------|
| `ContextMenuState` | `context_menu.rs` | `struct { position: Point, row_index: Option<usize>, items: Vec<ContextMenuItem> }` |
| `ContextMenuItem` | `context_menu.rs` | `enum { Action { label, message }, Separator, SubMenu { label, items } }` |
| `PinSide` | `state.rs` | `enum PinSide { Left, Right }` |
| `GridPreset` | `persistence.rs` | `struct { name: String, columns: Vec<GridColumnState>, sort: Vec<SortSpec> }` |

### Key Implementation Steps

1. **Context menu** (`context_menu.rs`):
   - Right-click on a row: show menu with actions (Remove, Copy Symbol, Pin to Top, etc.).
   - Right-click on a header: show menu with column actions (Sort Asc, Sort Desc, Auto-Fit, Pin Left, Pin Right, Hide Column).
   - Rendered as a positioned overlay in `stack![]`. Dismiss on click outside or Escape.
   - Menu items are `button` widgets with `on_press` emitting `GridMessage::ContextMenuAction(action_id)`.
   - The app maps `action_id` to concrete actions.

2. **Column presets** (`persistence.rs`):
   - Save: serialize current `GridState` column order, widths, visibility, sort to a named preset.
   - Load: deserialize and apply a preset.
   - Presets stored in `data/config.toml` under `[grid_presets]`.
   - Expose via context menu: "Save Column Layout As...", "Load Layout > [preset names]".

3. **Auto-fit column width**:
   - On double-click of resize handle, measure the maximum rendered width of all visible cells in that column.
   - Strategy: iterate visible rows, call a new trait method `fn content_width(&self, row: &T) -> f32` which estimates text width (characters x average character width for the font size). Rough but sufficient.
   - Alternative: measure the widest text string using iced's text measurement if available, or use a fixed-width font assumption (8px per character at size 13).

4. **Column pinning**:
   - Pinned-left columns render in a fixed `Row` before the scrollable area.
   - Pinned-right columns render in a fixed `Row` after the scrollable area.
   - Layout: `row![pinned_left_cols, scrollable(center_cols), pinned_right_cols]`.
   - Header and body must both respect this three-zone layout.
   - Pinned columns do not participate in horizontal scroll.

5. **Multi-column sort**:
   - Shift+click on a header adds it to the sort specs with the next priority.
   - Plain click replaces all sort specs with a single-column sort.
   - Each sorted header shows a priority badge (small "1", "2", "3" text).
   - `GridState::sort_specs: Vec<SortSpec>` ordered by priority.
   - The app handles `GridMessage::SortToggled` with multi-column logic (Shift+click appends) and performs a stable multi-key sort.

6. **Copy/paste**:
   - Ctrl+C: Copy selected row(s) data as tab-separated text to clipboard.
   - Uses iced's clipboard API (`iced::clipboard::write`).
   - Format: header row + data rows, tab-separated columns, newline-separated rows.
   - Ctrl+V: Paste ticker symbols from clipboard (one per line) into the watchlist.

### Testing Strategy

- **Unit test context menu**: Create menu with 3 items, verify rendering produces 3 buttons.
- **Unit test multi-sort**: Click col A (sorts=[A asc]). Shift+click col B (sorts=[A asc, B asc]). Click col C (sorts=[C asc]). Shift+click col A desc (sorts=[C asc, A desc]).
- **Unit test auto-fit**: Given column with values ["AAPL", "MSFT", "GOOGL"], estimate width should be max("GOOGL") * char_width + padding.
- **Unit test column pinning**: Pin col A left. Verify `pinned_left` contains A. Verify `column_display_order()` excludes A from the scrollable section.
- **Unit test preset save/load roundtrip**: Save preset, modify state, load preset, verify state matches saved state.
- **Manual test**: Right-click row, verify context menu appears with correct actions. Test all menu items. Right-click header, verify column menu. Test pin left/right, auto-fit, hide/show.

### Migration Plan

Phase 4 features are additive. No existing functionality needs migration. Implement in order:
1. Context menu (foundational for other features' discoverability).
2. Multi-column sort (extends existing sort).
3. Auto-fit (extends existing resize).
4. Column pinning (significant layout change in header/body).
5. Presets (extends persistence).
6. Copy/paste (standalone feature).

### Estimated Complexity

- 1 new file
- ~6 new types
- ~25 new functions
- ~900 lines of new code
- Modifications across 7 existing files

### Acceptance Criteria

- [ ] Right-click on a row shows a context menu with relevant actions.
- [ ] Right-click on a header shows column-specific actions.
- [ ] Context menu dismisses on click outside or Escape.
- [ ] Shift+click on sorted column adds multi-sort with priority badges.
- [ ] Double-click resize handle auto-fits column to content width.
- [ ] Columns can be pinned left/right and remain visible during horizontal scroll.
- [ ] Column presets can be saved and loaded by name.
- [ ] Ctrl+C copies selected rows as tab-separated text.
- [ ] Ctrl+V pastes ticker symbols from clipboard.

---

## Critical Path

The longest sequential dependency chain determines the minimum project duration:

```
Phase 0 → Phase 1 → Phase 2 → Phase 3b → Phase 4
  (Foundation)  (Interactions)  (DnD + Widget)  (Virtual scroll, keyboard)  (Advanced)
```

The parallel path (Phase 0 → Phase 1 → Phase 3a-parallel) is shorter and can
absorb schedule slack. Phase 3a-sequential work (multi-selection, flash wiring
into `state.rs`) sits between the critical path and the parallel path: it depends
on Phase 2 but does not block Phase 3b or Phase 4.

The Pre-Phase 2 spike runs concurrently with Phase 0 and can shift the critical
path if it fails (Phase 2 adopts Plan B, Widget transition moves to Phase 3b).

### Rough Time Estimates (single developer)

| Phase | Estimate | Notes |
|---|---|---|
| Pre-Phase 2 Spike | 1-2 days | Runs concurrently with Phase 0 |
| Phase 0 (Foundation) | 3-5 days | ~1000 lines + migration across 4 commits |
| Phase 1 (Interactions) | 3-4 days | ~800-1000 lines, resize/sort/selection |
| Phase 2 (DnD + Widget) | 5-8 days | ~1500-1900 lines, highest risk (Widget transition) |
| Phase 3a (Polish) | 3-5 days | Split across parallel and sequential sub-phases |
| Phase 3b (Virtual scroll + kbd) | 3-4 days | Depends on Phase 2 completion |
| Phase 4 (Advanced) | 4-6 days | Context menus, multi-sort, pinning, presets |

**Critical path total**: ~17-27 days (Phase 0 → 1 → 2 → 3b → 4).
These are rough ranges, not commitments — actual duration depends on iced framework surprises and spike outcomes.

---

## Risk Assessment

### Phase 0 Risks

| Risk | Severity | Mitigation |
|------|----------|------------|
| **`GridColumn` trait design is wrong**: If the trait signature does not accommodate future needs (flash values, format rules, content width measurement), it must be changed in all column implementations. | HIGH | Design the trait with `default` method implementations for every optional capability. New methods added in later phases have defaults returning `None`, so existing column types compile without changes. |
| **Lifetime complexity in `GridColumn::cell()`**: The trait returns `Element<'a, Message>` which borrows from `&'a T`. If `T` is a reference type (`&WatchlistTicker`), double-reference issues arise. | MEDIUM | Constrain `T` to be the owned data row type. The caller passes `&[T]` to the grid; the grid calls `cell(&row[i], i)`. Keep the borrow simple. |
| **Performance of trait object dispatch**: Using `&[&dyn GridColumn<T, M>]` incurs vtable overhead per cell per frame. | LOW | For a watchlist with ~7 columns and ~50 visible rows, this is 350 vtable calls per frame (nanoseconds). Not a concern. If it becomes an issue, switch to enum dispatch. |
| **Breaking config format**: Changing `column_widths: Vec<f32>` to `GridState` in `WatchlistConfig` breaks existing config files. | LOW | No shipped releases exist, so no config files need migration. Write the new `GridConfig` format directly, replacing the old `column_widths` vec. No backward-compatible dual-format loading needed. |

### Phase 1 Risks

| Risk | Severity | Mitigation |
|------|----------|------------|
| **Press vs. click ambiguity in iced**: iced's `mouse_area` provides `on_press` and `on_release` but distinguishing click from drag requires manual state tracking. | MEDIUM | Track press position in `GridState.interaction` (via `ActiveInteraction` variants, or a pending drag field). On move, check distance. If threshold exceeded, transition to drag mode. If released without exceeding, treat as click. This is a known pattern. |
| **Global mouse capture during resize**: If the user drags fast, the cursor can leave the resize handle area. The overlay must capture globally. | LOW | Already solved in the current `stack![]` overlay block following the resize-handle `mouse_area` in `views.rs`. Migrate that overlay pattern into the grid widget. |

### Phase 2 Risks

| Risk | Severity | Mitigation |
|------|----------|------------|
| **Drag ghost rendering in iced**: iced has no built-in floating/overlay widget that follows the cursor at an arbitrary position. | HIGH | Use `stack![]` with a container that has absolute positioning via padding/margin calculated from cursor position. Alternatively, use iced's `overlay` mechanism. If neither works cleanly, render the ghost as a container at the top of the stack with dynamic padding. Test early. |
| **Column reorder + resize handle interaction**: During column reorder, the resize handles between headers must not interfere with the drag. | MEDIUM | Disable resize handles when `GridState.interaction` is `DraggingColumn`. The `ActiveInteraction` enum guarantees mutual exclusion -- resize and column drag cannot be active simultaneously. The overlay captures all mouse events during drag, so handles underneath are inactive. |
| **Row reorder conflicts with sort**: If the user has an active sort, reordering rows by drag is meaningless (the sort will re-sort them). | LOW | Disable row drag when a sort is active. Show a tooltip or visual indicator that drag requires unsorted mode. Clear sort on first drag attempt with a confirmation. |
| **Accessibility regression from custom Widget**: The Phase 2 custom `Widget` impl bypasses iced's default widget accessibility infrastructure (keyboard tab ordering, focus management). Since accessibility is a stated non-goal for Phases 0-4, this is accepted debt, but the Widget transition is the point where that debt is incurred. | LOW | Acknowledged and tracked. If accessibility becomes a requirement post-Phase 4, the custom Widget will need explicit focus/tab support added to its `update()` method. No action needed now. |

### Phase 3 Risks

| Risk | Severity | Mitigation |
|------|----------|------------|
| **Virtual scrolling breaks iced scrollable**: iced's `scrollable` widget expects its children to report their full size. Spacers for virtual scrolling must correctly report height so the scrollbar thumb size is accurate. | HIGH | Use fixed-height `Space` widgets as top/bottom spacers. Their combined height plus visible rows' height must equal `total_rows * row_height`. Test with large row counts (10k) to verify scrollbar behavior. If iced's scrollable does not handle this well, consider a custom scrollable implementation. |
| **Flash-on-tick requires continuous redraw**: While flashes are active, the grid must redraw at ~60fps even without user interaction. | MEDIUM | Use an iced `Subscription` that emits tick events at 60fps while any flash is active. When all flashes expire, stop the subscription. This avoids permanent polling. |
| **Keyboard focus model**: iced does not have a robust focus management system for custom widgets. The grid must compete for keyboard events with text inputs (add-ticker input). | MEDIUM | Track grid focus explicitly in `GridState`. The grid gains focus on mouse click inside the grid body. It loses focus when the user clicks outside or tabs to the text input. Filter keyboard events only when focused. |

### Phase 4 Risks

| Risk | Severity | Mitigation |
|------|----------|------------|
| **Context menu positioning**: The menu must appear at the cursor position and stay within the window bounds. iced has no built-in popup/dropdown mechanism beyond overlay. | MEDIUM | Calculate menu position, clamp to window bounds. Render as a `stack![]` overlay positioned via padding. The menu is a simple `column` of buttons. A backdrop `mouse_area` catches clicks outside to dismiss. |
| **Column pinning layout complexity**: Splitting the grid into three horizontal zones (pinned-left, scrollable, pinned-right) while keeping header and body columns aligned is structurally complex. | HIGH | Implement as three synchronized sub-grids sharing the same `GridState`. Each sub-grid renders its subset of columns. Vertical scroll is shared. This is the approach used by AG Grid internally. Defer this to late Phase 4 if time is constrained. |
| **Clipboard API availability**: iced's clipboard support may be limited on Windows. | LOW | Test `iced::clipboard::write` early. If it does not work, use the `clipboard` crate directly via `arboard`. |

### API Design Decisions to Get Right in Phase 0

These decisions are difficult to change later because they propagate through all column implementations and the app integration layer:

1. **`GridColumn` trait signature**: The generic parameters `<T, Message>` must be correct. `T` is the row data type, `Message` is the app's message type. If we later need the grid to be generic over theme or renderer, adding parameters is a breaking change. **Decision**: Keep it `<T, Message>` only. Theme and renderer generics are unnecessary because we use a single dark theme.

2. **`ColumnId` representation**: Using `&'static str` is zero-allocation but requires compile-time-known column names. Using `String` allows runtime-generated IDs but requires cloning. **Decision**: Use `&'static str` wrapped in a newtype. All watchlist columns have known names at compile time. If runtime columns are needed later (computed columns), introduce `ColumnId::Dynamic(String)` as a variant.

3. **Grid message wrapping pattern**: The grid is generic over the app's message type `M`. Cell content emits `M` directly. Grid chrome events are mapped to `M` via a required `on_grid: Fn(GridMessage) -> M` callback. **Decision**: Two-path message design — cell path (`Element<'a, M>` emits `M` directly) and chrome path (`on_grid(GridMessage::SortToggled(col))` produces `M`). This keeps the grid decoupled from any specific app message type while allowing cells to emit arbitrary app-level messages (e.g., `Message::ToggleFavorite`). In Phase 0-1, `on_grid` is `&dyn Fn`; in Phase 2+, the custom `Widget<M>` stores it as `Box<dyn Fn>`. No message-type refactor is needed at the Widget transition.

4. **Data access pattern**: The grid receives `&[T]` -- a pre-sorted, pre-filtered slice. The grid never sorts or filters. **Decision**: This is correct and must not change. The app is responsible for data transformation. The grid is a pure view.

5. **Row identity**: The grid identifies rows by index into the provided slice. This is fragile if the slice changes between frames (row inserted/removed shifts indices). For selection persistence across data updates, we need a stable row identity. **Decision**: Phase 0-2 use index-based selection with `selected_symbol: Option<String>` as the authoritative identity at the app level (the brief visual artifact after re-sort is accepted). Phase 3a introduces `RowKey(pub String)` with `RowKey::new(key: impl Into<String>)` constructor and a `.row_key(fn(&T) -> RowKey)` builder parameter alongside multi-selection, replacing `Option<usize>` with `BTreeSet<RowKey>` (see canonical definition in 03-column-data-model.md §4.4). This defers the complexity until it is actually required.

---

### Critical Files for Implementation
- `D:\GitHub\HandOfMidas\desktop\win\crates\midas-grid\src\column.rs`
- `D:\GitHub\HandOfMidas\desktop\win\crates\midas-grid\src\state.rs`
- `D:\GitHub\HandOfMidas\desktop\win\crates\midas-grid\src\widget.rs`
- `D:\GitHub\HandOfMidas\desktop\win\crates\midas-app\src\app\views.rs`
- `D:\GitHub\HandOfMidas\desktop\win\crates\midas-app\src\watchlist.rs`