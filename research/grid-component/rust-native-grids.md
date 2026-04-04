# Rust & Native Grid/Table Implementations: Architecture Research

> Research date: April 2026
> Purpose: Inform the design of Hand of Midas's watchlist/order grid components built on iced + wgpu.

---

## Table of Contents

1. [iced Ecosystem](#1-iced-ecosystem)
2. [egui Tables](#2-egui-tables)
3. [Slint](#3-slint)
4. [Qt TableView / QAbstractTableModel](#4-qt-tableview--qabstracttablemodel)
5. [GTK4 ColumnView](#5-gtk4-columnview)
6. [Dear ImGui Tables](#6-dear-imgui-tables)
7. [Architecture Comparison](#7-architecture-comparison)
8. [What We Can Learn for Our Grid](#8-what-we-can-learn-for-our-grid)

---

## 1. iced Ecosystem

### 1.1 Built-in `table` Widget (iced 0.14+)

Iced 0.14 introduced a first-party `table` widget (`iced::widget::table`). It provides a grid-like visual representation of data distributed in columns and rows.

**Core API:**

```rust
// Construction
fn table<T>(
    columns: impl IntoIterator<Item = Column<'a, 'b, T, Message, Theme, Renderer>>,
    rows: impl IntoIterator<Item = T>,
) -> Table<'a, Message, Theme, Renderer>

// Column definition
fn column<T>(
    header: impl Into<Element<'a, Message>>,
    view: impl Fn(&T) -> Element<'b, Message>,
) -> Column<'a, 'b, T, Message, Theme, Renderer>
```

Each `Column` accepts:
- A **header** element (any iced widget)
- A **view function** `Fn(&T) -> Element` that renders each row's cell for that column
- Configuration: `width(Length)`, `align_x(Horizontal)`, `align_y(Vertical)`

The `Table` struct provides:
- `width()`, `padding()` / `padding_x()` / `padding_y()`
- `separator()` / `separator_x()` / `separator_y()` for grid lines
- Implements the standard `Widget` trait (`layout`, `update`, `draw`)
- Styling via a `Catalog` trait and `StyleFn`

**What it does NOT have (as of 0.14):**
- No built-in column resizing
- No built-in sorting
- No built-in row selection
- No virtual scrolling (renders all rows; wrapping in a `Scrollable` does not virtualize)
- No column reordering or drag-and-drop
- No frozen rows/columns

This is a foundational building block -- minimal by design, following iced's philosophy of composable primitives.

### 1.2 `iced_table` (Community Crate by tarkah)

The `iced_table` crate (crates.io) provides a more feature-rich table widget, built as an external library. It defines a **trait-based column architecture**:

```rust
trait Column {
    fn header(&self) -> Element;
    fn cell(&self, row: &RowData) -> Element;
    fn footer(&self) -> Element;
    fn width(&self) -> f32;
    fn resize_offset(&self) -> Option<f32>;
}
```

**Key features:**
- **Column resizing**: Two-phase system -- `Resizing(col_index, offset)` tracks drag, `Resized` commits the width change
- **Header/footer sync**: Three separate `Scrollable` widgets (header, body, footer) are kept in horizontal sync via `SyncHeader` messages that batch scroll commands
- **Custom cell content**: Each cell is an arbitrary iced `Element`, so any widget (checkbox, dropdown, text input, button) can be a cell
- **Follows Elm architecture**: State is in the `App`, messages describe interactions, `update` is pure, `view` renders from state

**Limitations:**
- No virtual scrolling -- all rows are rendered (performance degrades at ~1000+ rows)
- No built-in sorting (must be done in application code)
- No row selection model
- No frozen columns
- Horizontal scroll sync requires manual message plumbing

### 1.3 `tabular` (Community Crate by airstrike)

A newer community crate providing a table widget generic over the cell data type and cell reference system.

**Key features:**
- Cell selection (click-to-select)
- Editable cells
- Sorting support
- Generic: works with `Vec<Vec<T>>` or custom data structures
- Custom cell formatting

**Architecture:** The widget is generic over both the data type `T` in cells and the cell reference type, allowing custom referencing systems. Default implementations exist for `Vec<Vec<T>>`.

### 1.4 `iced_aw` (Additional Widgets)

The `iced_aw` crate provides extra widgets behind feature gates but does **not** include a dedicated table/grid widget for tabular data. It provides a `Grid` layout widget (for arranging children in a grid pattern), but this is a layout container, not a data grid.

### 1.5 Virtual Scrolling in iced

Iced does not yet have built-in virtual scrolling for lists or tables. The standard `Scrollable` widget renders all children. Community workarounds include:

- **Manual windowing**: Render a fixed number of rows based on scroll position, using a slider or scroll offset to control which slice of data is visible
- **`lazy` widget**: Defers computation until dependencies change, reducing re-renders but not virtualizing
- **Upcoming `List` widget**: An official virtual list widget has been in development (feature branch `Feature/list-widget-reloaded`) but is not yet released

For a trading watchlist with 50-500 symbols, the lack of virtualization is tolerable. For order history with thousands of rows, it becomes a real constraint.

---

## 2. egui Tables

egui (via `egui_extras`) provides a mature, immediate-mode table system built around `TableBuilder`.

### 2.1 API Architecture

```rust
// Builder pattern
TableBuilder::new(ui)
    .striped(true)
    .resizable(true)
    .column(Column::auto())              // auto-fit content
    .column(Column::initial(100.0))      // initial width
    .column(Column::remainder())         // fills remaining space
    .header(20.0, |mut header| {
        header.col(|ui| { ui.label("Name"); });
        header.col(|ui| { ui.label("Price"); });
        header.col(|ui| { ui.label("Change"); });
    })
    .body(|body| {
        body.rows(ROW_HEIGHT, data.len(), |mut row| {
            let item = &data[row.index()];
            row.col(|ui| { ui.label(&item.name); });
            row.col(|ui| { ui.label(format!("{:.2}", item.price)); });
            row.col(|ui| { ui.label(format!("{:+.2}%", item.change)); });
        });
    });
```

### 2.2 Column System

Four column sizing modes:
- `Column::auto()` -- fits content
- `Column::initial(width)` -- starts at given width
- `Column::exact(width)` -- fixed, cannot resize
- `Column::remainder()` -- uses remaining space (must be last, not resizable)

All columns can be made resizable with `Column::resizable()` or globally with `TableBuilder::resizable(true)`.

### 2.3 Virtual Scrolling (Built-in)

egui's table has **three row-rendering methods** with fundamentally different performance characteristics:

| Method | Virtual Scroll | Row Heights | Performance |
|--------|---------------|-------------|-------------|
| `body.row(height, closure)` | No | Per-row | O(n) -- renders every row |
| `body.rows(height, count, closure)` | Yes | Uniform | O(visible) -- only visible rows rendered |
| `body.heterogeneous_rows(heights, closure)` | Yes | Variable | O(visible) + height iteration |

`rows()` is the key method: it accepts a total row count and only calls the closure for visible rows. This enables efficient rendering of 100,000+ rows.

`heterogeneous_rows()` accepts an iterator of row heights and only renders visible rows, though it must iterate all heights to calculate scroll position. Benchmarks show ~9-10ms/frame for 100k heterogeneous rows vs ~4-5ms for homogeneous.

### 2.4 Scrolling Configuration

- `vscroll(bool)` -- vertical scrolling (default: true)
- `drag_to_scroll(bool)` -- mouse drag scrolling
- `scroll_to_row(row, alignment)` -- programmatic scroll
- `vertical_scroll_offset(f32)` -- set scroll position directly
- `stick_to_bottom(bool)` -- auto-scroll for streaming data (useful for order logs)
- `animate_scrolling(bool)` -- smooth scroll animation
- `min_scrolled_height` / `max_scroll_height` -- control when scrollbars appear

### 2.5 What egui Tables Lack

- **No built-in sorting** -- header clicks do not sort; must be implemented in application code (egui-selectable-table adds this)
- **No column reordering** via drag
- **No frozen columns** -- no split-pane scrolling
- **No built-in selection model** -- must track selection state manually
- **No column state persistence** -- widths reset on restart unless manually saved
- **Immediate-mode overhead** -- entire table closure re-executes every frame

### 2.6 Strengths

- Extremely simple API -- a table can be defined in 20 lines
- Built-in virtual scrolling that actually works at scale
- Resizable columns out of the box
- Any egui widget can be placed in any cell (buttons, sliders, plots, etc.)
- Striped rows, cell interaction sensing, custom cell layout
- Mature and battle-tested in production tools

---

## 3. Slint

Slint provides `StandardTableView` as a high-level widget and `ListView` as the lower-level building block.

### 3.1 StandardTableView

A pre-built table component with columns and rows. Defined in Slint's declarative markup language:

```slint
StandardTableView {
    columns: [
        { title: "Symbol" },
        { title: "Price" },
        { title: "Change" },
    ];
    rows: [
        [ { text: "AAPL" }, { text: "189.50" }, { text: "+1.2%" } ],
        [ { text: "MSFT" }, { text: "420.30" }, { text: "-0.3%" } ],
    ];
}
```

**Properties:**
- `columns`: Array of `TableColumn` objects (title, width, etc.)
- `rows`: 2D model -- `ModelRc<ModelRc<StandardListViewItem>>`
- `current-row`: Index of selected row (-1 = none)

**Callbacks:**
- `sort-ascending(column_index)` -- emitted when user clicks header for ascending sort
- `sort-descending(column_index)` -- emitted for descending sort
- `row-pointer-event(row_index, PointerEvent, Point)` -- mouse events on rows

**Key limitation:** Cells are `StandardListViewItem`, which is essentially a text-only item. You cannot place arbitrary widgets (checkboxes, color indicators, sparklines) in cells using `StandardTableView`.

### 3.2 ListView as Alternative

For custom cell rendering, Slint's recommended approach is to use `ListView` directly:

```slint
ListView {
    for row in data_model: Rectangle {
        HorizontalLayout {
            Text { text: row.symbol; }
            Text { text: row.price; color: row.price_color; }
            CheckBox { checked: row.enabled; }
        }
    }
}
```

`ListView` provides built-in virtual scrolling -- elements are only instantiated when visible. This is Slint's core strength: the `for` element inside a `ListView` automatically manages lifecycle.

### 3.3 Model Architecture

Slint uses a `Model` trait (Rust side) that provides data to repeaters:

```rust
trait Model {
    type Data;
    fn row_count(&self) -> usize;
    fn row_data(&self, row: usize) -> Option<Self::Data>;
    fn set_row_data(&self, row: usize, data: Self::Data);
    fn model_tracker(&self) -> &dyn ModelTracker;
}
```

The `ModelTracker` enables fine-grained change notification. When data changes, only affected rows are re-bound. This is similar to Qt's model notification system.

### 3.4 Limitations

- `StandardTableView` is text-only cells -- no custom widgets
- Column resizing is limited
- No frozen columns/rows
- The 2D model type (`ModelRc<ModelRc<StandardListViewItem>>`) is awkward to work with from Rust
- For anything beyond basic text tables, you must build a custom component from `ListView` + delegates
- Issues reported with >2M rows in custom ListView models (panics and strange behavior)

---

## 4. Qt TableView / QAbstractTableModel

Qt's model/view architecture is the gold standard for retained-mode table implementations.

### 4.1 Model-View-Delegate Separation

Qt separates three concerns:

```
Model (data)  <-->  View (display)  <-->  Delegate (cell rendering/editing)
      \                   |                    /
       \---- signals & slots ---- /
```

**Model** (`QAbstractTableModel`):
- `rowCount()`, `columnCount()` -- dimensions
- `data(index, role)` -- returns data for a given cell and role
- `headerData(section, orientation, role)` -- column/row headers
- `setData(index, value, role)` -- editing
- Emits `dataChanged`, `rowsInserted`, `rowsRemoved` signals for incremental updates

**Role system**: A single cell can provide multiple "roles" of data:
- `Qt::DisplayRole` -- text to display
- `Qt::EditRole` -- data for editing
- `Qt::DecorationRole` -- icon
- `Qt::ForegroundRole` -- text color
- `Qt::BackgroundRole` -- cell background
- `Qt::TextAlignmentRole` -- alignment
- Custom roles (e.g., `SortRole` for numeric sorting vs. display formatting)

This role system is powerful: a price cell can display "$189.50" (DisplayRole) while sorting on the raw float 189.5 (UserRole).

**View** (`QTableView`):
- Manages viewport, scrollbars, selection
- Only renders cells visible in the viewport
- Recycles painting -- as cells scroll out, new cells scroll in without widget creation/destruction
- Headers are separate `QHeaderView` widgets with built-in resize, reorder, sort-indicator support

**Delegate** (`QStyledItemDelegate`):
- `paint(QPainter*, QStyleOptionViewItem, QModelIndex)` -- custom cell rendering
- `createEditor()` / `setEditorData()` / `setModelData()` -- inline editing
- QPainter gives full drawing control: gradients, icons, custom shapes, mini-charts

### 4.2 Virtual Scrolling Architecture

Qt's `QTableView` inherits from `QAbstractScrollArea`:
- The view maintains a **viewport widget** that is smaller than the full data extent
- Only cells intersecting the viewport are painted
- `QTableView::paintEvent` iterates visible rows/columns using `rowAt()` / `columnAt()`
- No cell widgets are created -- the delegate paints directly with QPainter
- This means 1M rows cost zero memory beyond the model data itself

For the QML `TableView` (Qt Quick):
- Uses a **recycling delegate** system
- Delegates are created for visible cells, then reused (re-bound to new model indices) as the user scrolls
- `reuseItems` property controls recycling behavior
- The view maintains a reuse pool of recycled delegates for quick re-creation when new cells scroll into view

### 4.3 Column State Management

`QHeaderView` provides:
- `setSectionResizeMode()` -- Fixed, Stretch, ResizeToContents, Interactive
- `setSortIndicator(column, order)` -- visual sort arrows
- `setSectionsMovable(true)` -- drag-to-reorder columns
- `sectionResized` / `sectionMoved` signals
- `saveState()` / `restoreState()` -- serializes column widths, order, visibility to QByteArray

### 4.4 Proxy Models

Qt's `QSortFilterProxyModel` sits between model and view:
- Sorts by any role without touching source data
- Filters rows by regex or custom predicate
- Maps between source and proxy indices
- Chainable -- multiple proxies can stack

This is a key architectural pattern: the view never touches the real data order. Sorting is a transformation layer.

### 4.5 Strengths for Grid Design

- Proven at scale (millions of rows, used in Bloomberg Terminal, trading desks)
- Role system cleanly separates display from data
- Delegate painting gives pixel-perfect control
- Proxy models make sort/filter composable
- Column state persistence is built-in
- Selection model is separate and reusable (`QItemSelectionModel`)

---

## 5. GTK4 ColumnView

GTK4 replaced GTK3's `TreeView` with `ColumnView`, using a factory pattern for cell creation.

### 5.1 Factory Pattern

`ColumnView` uses `GtkListItemFactory` to create cell widgets. The primary implementation is `GtkSignalListItemFactory` with four lifecycle signals:

```
setup  -->  bind  -->  [visible]  -->  unbind  -->  bind (recycled)  -->  ...
  |                                                                        |
  +--- teardown (final cleanup) <------------------------------------------+
```

**setup**: Create the widget structure (e.g., a `GtkLabel`). This is called once per visible slot. The widget is retained across recycling.

**bind**: Populate the widget with data from the current model item. Called when a new row scrolls into view and an existing widget is recycled to display it.

**unbind**: Clear the widget's data bindings. Called when the row scrolls out of view.

**teardown**: Destroy the widget. Only called during final cleanup.

### 5.2 Widget Recycling

GTK4's list architecture is designed for unlimited scale:
- Widgets are created only for the visible viewport (typically <200 items even for million-item lists)
- When an item scrolls out of view, its widget is **recycled** -- unbound from the old item and re-bound to the new item scrolling in
- Memory consumption stays nearly constant regardless of list size
- The model provides data via `GListModel`, which avoids iterating all items

### 5.3 Selection Model

GTK4 separates selection from the data model:
- `GtkSingleSelection` -- single row selection
- `GtkMultiSelection` -- multi-select with Ctrl/Shift
- `GtkNoSelection` -- read-only display
- Rubberband selection available via `enable-rubberband` property

The selection model wraps the data model, so the same data can be displayed with different selection behaviors in different views.

### 5.4 Sorting

`ColumnView` has built-in sort infrastructure:
- Each `ColumnViewColumn` can have a `GtkSorter` attached via `set_sorter()`
- Clicking a column header activates that column's sorter
- The `ColumnView` exposes a combined `sorter` property
- This sorter must be attached to a `GtkSortListModel` wrapping the data model
- Sort order indicators appear automatically in headers

```
Data Model --> GtkSortListModel --> GtkSingleSelection --> ColumnView
                    ^
                    |
              ColumnView.sorter (composed from column sorters)
```

### 5.5 Per-Column Factories

Each `GtkColumnViewColumn` has its own factory. This means different columns can have completely different widget types:
- Column 1: `GtkLabel` for text
- Column 2: `GtkImage` for status icons
- Column 3: `GtkProgressBar` for progress
- Column 4: Custom `GtkDrawingArea` for sparklines

### 5.6 Strengths and Limitations

**Strengths:**
- True widget recycling at massive scale
- Clean separation of model, selection, sorting, and cell rendering
- Per-column factory allows heterogeneous cell widgets
- Composable model pipeline (filter -> sort -> select -> view)

**Limitations:**
- Factory pattern is verbose -- 4 signal handlers per column type
- No frozen columns/rows
- Column resizing is limited
- Header customization is constrained
- Performance depends on widget complexity in cells
- Rust bindings (gtk4-rs) work well but factory setup is boilerplate-heavy

---

## 6. Dear ImGui Tables

Dear ImGui's table system (`imgui_tables.cpp`, ~4,400 lines) is the most complete immediate-mode table implementation. It is GPU-rendered (via draw lists that map to GPU vertex/index buffers) and runs in immediate mode.

### 6.1 Core API Pattern

```cpp
if (ImGui::BeginTable("watchlist", 5, flags)) {
    // 1. Column setup (once, or when columns change)
    ImGui::TableSetupColumn("Symbol",  ImGuiTableColumnFlags_DefaultSort);
    ImGui::TableSetupColumn("Last",    ImGuiTableColumnFlags_None, 80.0f);
    ImGui::TableSetupColumn("Change",  ImGuiTableColumnFlags_None, 60.0f);
    ImGui::TableSetupColumn("Volume",  ImGuiTableColumnFlags_None, 80.0f);
    ImGui::TableSetupColumn("Action",  ImGuiTableColumnFlags_NoSort);
    ImGui::TableSetupScrollFreeze(1, 1); // Freeze 1 col, 1 row

    // 2. Header row
    ImGui::TableHeadersRow();

    // 3. Data rows (with clipper for virtual scrolling)
    ImGuiListClipper clipper;
    clipper.Begin(row_count);
    while (clipper.Step()) {
        for (int row = clipper.DisplayStart; row < clipper.DisplayEnd; row++) {
            ImGui::TableNextRow();
            ImGui::TableNextColumn(); ImGui::Text("%s", data[row].symbol);
            ImGui::TableNextColumn(); ImGui::Text("%.2f", data[row].last);
            ImGui::TableNextColumn(); ImGui::TextColored(color, "%+.2f%%", data[row].change);
            ImGui::TableNextColumn(); ImGui::Text("%s", format_volume(data[row].volume));
            ImGui::TableNextColumn();
            if (ImGui::SmallButton("Buy")) { /* handle */ }
        }
    }

    ImGui::EndTable();
}
```

### 6.2 Feature Flags

All features are opt-in via flags:

| Category | Flags |
|----------|-------|
| **Sizing** | `SizingFixedFit`, `SizingFixedSame`, `SizingStretchSame`, `SizingStretchWeight` |
| **Interaction** | `Resizable`, `Reorderable`, `Hideable`, `Sortable`, `SortMulti`, `SortTristate` |
| **Scrolling** | `ScrollX`, `ScrollY` |
| **Visual** | `RowBg`, `Borders*`, `NoBordersInBody`, `HighlightHoveredColumn` |
| **Clipping** | `NoClip`, `PreciseWidths` |
| **Padding** | `PadOuterX`, `NoPadOuterX`, `NoPadInnerX` |

### 6.3 Sorting Architecture

ImGui does NOT sort data. It provides:
- Sort specifications via `TableGetSortSpecs()` returning `ImGuiTableSortSpecs`
- Contains array of `ImGuiTableColumnSortSpecs` (column index, sort direction, sort order for multi-sort)
- A `SpecsDirty` flag that is set when the user changes sort -- the application checks this flag, sorts its data, and clears it
- Multi-column sorting is supported

This "specs-only" approach is the same pattern we should use: the grid tells you what the user wants; the application sorts the data.

### 6.4 Column Resizing

- Drag the border between column headers
- Hit-test width: 4.0px (configurable via `TABLE_RESIZE_SEPARATOR_HALF_THICKNESS`)
- Visual feedback delay: 0.06s
- After resizing, stretch columns recalculate weights via `TableUpdateColumnsWeightFromWidth()`

### 6.5 Frozen Rows/Columns

```cpp
ImGui::TableSetupScrollFreeze(freeze_cols, freeze_rows);
```

- Frozen rows/columns remain visible during scrolling
- Implemented via separate draw channels in the `ImDrawListSplitter`
- Frozen areas get their own clipping rectangles

### 6.6 Draw List Splitting (Rendering Architecture)

This is the most architecturally interesting aspect. ImGui tables use `ImDrawListSplitter` to create multiple rendering channels:

```
Channel 0: Background (alternating row colors)
Channel 1: Frozen area background
Channel 2: Unclipped content (when NoClip flag is set)
Channel 3+: Per-column channels (1-2 per visible column)
```

Each cell's draw commands go to its column's channel. This enables **per-column clipping** without requiring separate draw calls per cell. At `EndTable()`, all channels are merged back into the main draw list.

This is how ImGui achieves efficient rendering with many columns -- each column has its own clip rect, and draw commands are batched per-column rather than per-cell.

### 6.7 Virtual Scrolling via ImGuiListClipper

`ImGuiListClipper` calculates visible rows based on scroll position and row height:
- Assumes uniform row heights (required for O(1) row-to-position mapping)
- Only calls the row-rendering closure for `DisplayStart..DisplayEnd`
- Combined with column clipping, only visible cells are rendered

**Limitation**: Variable row heights are not well supported. The clipper needs to know all heights upfront or assumes uniformity.

### 6.8 Settings Persistence

ImGui auto-saves table state to `.ini` files:
- Column widths, order, visibility, sort direction
- Serialized via `ImGuiTableSettings` / `ImGuiTableColumnSettings`
- Opt-out via flags (disabled when no interactive features are enabled)

### 6.9 Context Menu

Right-clicking a header opens a built-in context menu with:
- Column visibility toggles
- Sizing policy options
- Hide current column option

### 6.10 Key Takeaways

- The "specs-only" sorting pattern is clean and transferable
- Draw list splitting for per-column clipping is a powerful GPU-friendly pattern
- Frozen rows/columns via separate draw channels is elegant
- Feature flags make the table progressively complex
- The ListClipper pattern for virtual scrolling is simple and effective for uniform rows
- Settings persistence is automatic -- a good UX pattern

---

## 7. Architecture Comparison

### 7.1 Retained vs Immediate vs Reactive

| Aspect | Retained (Qt/GTK) | Immediate (ImGui/egui) | Reactive/Elm (iced) |
|--------|-------------------|----------------------|---------------------|
| **State ownership** | Framework owns widget tree | No widget state; rebuilt every frame | App owns state; framework manages widget tree |
| **Update model** | Signals/slots mutate widgets | Full rebuild each frame | Messages trigger state updates; view re-derived |
| **Cell rendering** | Delegate paints or factory creates widgets | Closure renders inline | View function returns Element |
| **Data flow** | Model pushes changes to view | App pushes data each frame | State changes trigger view rebuild |
| **Memory** | Widget objects for visible cells (Qt paint) or recycled slots (GTK factory) | Minimal -- no widget objects | Element tree rebuilt each frame |
| **Complexity** | High boilerplate, powerful | Low boilerplate, simple | Medium boilerplate, safe |

### 7.2 Custom Cell Content

| Framework | Approach | Flexibility |
|-----------|----------|-------------|
| **Qt (Widgets)** | `QStyledItemDelegate::paint()` -- direct QPainter calls | Pixel-perfect; can draw anything |
| **Qt (Quick)** | Delegate component -- any QML item | Full QML widget tree per cell |
| **GTK4** | Factory creates any GtkWidget per cell | Full widget per cell, but recycled |
| **ImGui** | Any ImGui widget call between `TableNextColumn()` and next column | Any ImGui widget; custom draw via `ImDrawList` |
| **egui** | Any egui widget in row closure | Any egui widget; custom paint via `Painter` |
| **iced** | View function returns any `Element` | Any iced widget; custom via `Canvas` |
| **Slint** | `StandardTableView`: text only; `ListView`: any Slint element | Full flexibility only with custom ListView |

### 7.3 Virtual Scrolling Approaches

| Framework | Mechanism | Row Height Constraint | Scale |
|-----------|-----------|----------------------|-------|
| **Qt (Widgets)** | Viewport-based painting; only visible rows painted | Any (delegates report `sizeHint`) | Millions of rows |
| **Qt (Quick)** | Delegate recycling pool; create/destroy at viewport edges | Any (delegates size themselves) | Millions of rows |
| **GTK4** | Factory bind/unbind/recycle; widgets reused across rows | Any (widgets size themselves) | Millions of rows |
| **ImGui** | `ImGuiListClipper`; skips rows outside viewport | Uniform preferred; variable possible with effort | Hundreds of thousands |
| **egui** | `rows()` / `heterogeneous_rows()`; built into TableBody | Uniform (`rows`) or variable (`heterogeneous_rows`) | 100k+ rows |
| **iced (built-in)** | None -- all rows rendered | N/A | Hundreds of rows |
| **iced (iced_table)** | None -- all rows rendered | N/A | Hundreds of rows |
| **Slint** | `ListView` auto-virtualizes `for` elements | Any (elements size themselves) | Millions (issues at >2M) |

### 7.4 Column State Management

| Framework | Resize | Reorder | Hide | Sort Indicator | Persist | Freeze |
|-----------|--------|---------|------|---------------|---------|--------|
| **Qt** | Built-in | Built-in | Built-in | Built-in | `saveState()`/`restoreState()` | Manual (split views) |
| **GTK4** | Limited | No | No | Built-in | Manual | No |
| **ImGui** | Built-in | Built-in | Built-in | Built-in | Auto (.ini) | Built-in |
| **egui** | Built-in | No | No | No | Manual | No |
| **iced (built-in)** | No | No | No | No | No | No |
| **iced (iced_table)** | Built-in | No | No | No | Manual | No |
| **Slint** | Limited | No | No | Sort callbacks | Manual | No |

### 7.5 Selection Models

| Framework | Single | Multi | Range | Rubberband | Separate from View |
|-----------|--------|-------|-------|------------|-------------------|
| **Qt** | `QItemSelectionModel` | Yes | Yes | Yes | Yes -- reusable across views |
| **GTK4** | `GtkSingleSelection` | `GtkMultiSelection` | Yes | Yes | Yes -- wraps model |
| **ImGui** | Manual | Manual | Manual | No | N/A (no state) |
| **egui** | Manual | Manual | Manual | No | N/A |
| **iced** | Manual | Manual | Manual | No | N/A |
| **Slint** | `current-row` | No | No | No | No |

---

## 8. What We Can Learn for Our Grid

Given that Hand of Midas is built on iced (Elm architecture) with wgpu rendering, here are the patterns that transfer best and a recommended architecture.

### 8.1 Patterns to Adopt

**1. Specs-Only Sorting (from ImGui)**

The grid should not sort data. It should expose sort specifications:

```rust
struct SortSpec {
    column_id: ColumnId,
    direction: SortDirection,
    priority: usize, // for multi-column sort
}

enum Message {
    SortChanged(Vec<SortSpec>),
    // ...
}
```

The application sorts its data model and the grid re-renders. This keeps the grid stateless with respect to data ordering.

**2. Role-Based Data Access (from Qt)**

Instead of a single view function per column, consider a trait that provides multiple "aspects" of a cell:

```rust
trait CellData {
    fn display_text(&self) -> &str;
    fn sort_key(&self) -> SortKey;
    fn foreground_color(&self) -> Option<Color>;
    fn background_color(&self) -> Option<Color>;
    fn alignment(&self) -> Alignment;
}
```

This separates display formatting from sort ordering (e.g., "$1,234.56" displays but 1234.56 sorts).

**3. Column Trait (from iced_table)**

A trait-based column definition is the right pattern for iced:

```rust
trait GridColumn<T, Message> {
    fn id(&self) -> ColumnId;
    fn header(&self) -> Element<Message>;
    fn cell(&self, row: &T, row_index: usize) -> Element<Message>;
    fn width(&self) -> ColumnWidth;
    fn min_width(&self) -> f32;
    fn resizable(&self) -> bool;
    fn sortable(&self) -> bool;
}
```

**4. Separate Selection Model (from Qt/GTK)**

Selection should be a separate piece of state, not baked into the grid:

```rust
struct SelectionModel {
    mode: SelectionMode,        // Single, Multi, Range, None
    selected: HashSet<RowId>,
    anchor: Option<RowId>,      // for range selection
    focused: Option<RowId>,     // keyboard focus
}
```

This allows the same selection to be shared across views (e.g., selecting in watchlist highlights on chart).

**5. Column State Persistence (from ImGui/Qt)**

Column state (widths, order, visibility, sort) should serialize/deserialize:

```rust
struct ColumnState {
    id: ColumnId,
    width: f32,
    visible: bool,
    display_order: usize,
}

struct GridState {
    columns: Vec<ColumnState>,
    sort: Vec<SortSpec>,
}
```

**6. Draw-Channel Clipping (from ImGui)**

For our wgpu rendering, per-column clip rects are essential for performance. Each column should have its own scissor rect, and cells should only render within their column's bounds. This is the wgpu equivalent of ImGui's draw list splitting.

### 8.2 Virtual Scrolling Strategy

Since iced lacks built-in virtual scrolling, we have two options:

**Option A: Application-Level Windowing**

Calculate visible rows from scroll offset and viewport height. Only pass the visible slice to the table widget. Manage a virtual scroll position in application state.

```rust
fn visible_rows(&self) -> &[RowData] {
    let start = (self.scroll_offset / ROW_HEIGHT) as usize;
    let visible_count = (self.viewport_height / ROW_HEIGHT) as usize + 2;
    &self.rows[start..min(start + visible_count, self.rows.len())]
}
```

**Option B: Custom Widget with wgpu Rendering**

Build the grid as a custom iced widget that directly manages a wgpu render pipeline. This bypasses iced's element tree entirely for the grid body, giving us full control over:
- What rows are rendered (virtual scrolling)
- How cells are painted (direct GPU rendering)
- Clip rects per column
- Frozen rows/columns via separate render passes

This is the more complex option but gives the best performance for a trading grid.

**Recommendation**: Start with Option A for the watchlist (50-500 rows, performance is fine). Plan for Option B for order history and large data views.

### 8.3 Frozen Columns for Trading

A watchlist typically needs the Symbol column frozen while prices scroll horizontally. ImGui's approach (separate draw channels for frozen vs. scrollable areas) maps well to wgpu:

- Render frozen columns with one scissor rect (left side, fixed)
- Render scrollable columns with another scissor rect (right side, scrolled)
- Both share the same row data and vertical scroll position

### 8.4 Recommended Architecture

```
                    Application
                        |
              +---------+---------+
              |                   |
         GridState           DataModel
         (columns,           (Vec<Row>)
          sort specs,            |
          selection,       [sort/filter]
          scroll pos)            |
              |            Visible Slice
              +--------+--------+
                       |
                   GridWidget
                       |
              +--------+--------+
              |        |        |
           Header   Body     Footer
           (frozen  (virtual  (summary
            row)    scrolled)  row)
              |        |        |
           Column   Column   Column
           render   render   render
```

**Data flow (Elm style):**
1. User clicks header -> `Message::SortChanged(specs)`
2. `update` sorts `DataModel`, stores specs in `GridState`
3. `view` reads `GridState` + `DataModel` -> renders grid
4. User resizes column -> `Message::ColumnResized(id, width)`
5. `update` modifies `GridState.columns`
6. `view` reads new widths -> renders grid

**Key principle**: The grid widget is a pure view. All state lives in the application. All mutations go through messages. This is the Elm way, and it maps cleanly to iced's architecture.

### 8.5 What We Should NOT Try to Replicate

- **Qt's signal/slot mutation model**: Iced is not retained-mode; we should not try to make the grid mutate itself
- **GTK's factory pattern**: Overkill for iced; view functions accomplish the same thing more simply
- **ImGui's frame-by-frame rebuild**: Iced already handles diffing; we do not need to manually manage it
- **Slint's 2D model type**: Our data model should be a flat `Vec<T>` with column definitions as separate concerns

---

## Sources

### iced
- [iced::widget::table module](https://docs.iced.rs/iced/widget/table/index.html)
- [iced::widget::table::Table struct](https://docs.rs/iced/latest/iced/widget/table/struct.Table.html)
- [iced::widget::table::Column struct](https://docs.iced.rs/iced/widget/table/struct.Column.html)
- [iced_table crate on crates.io](https://crates.io/crates/iced_table)
- [iced_table example source](https://github.com/tarkah/iced_table/blob/master/example/src/main.rs)
- [tabular crate (airstrike)](https://github.com/airstrike/tabular)
- [iced_aw crate](https://github.com/iced-rs/iced_aw)
- [Table widget discussion #1234](https://github.com/iced-rs/iced/discussions/1234)
- [Large table discussion on iced discourse](https://discourse.iced.rs/t/a-table-scrollable-that-can-handle-thousands-of-items/93)
- [iced 0.14.0 release notes](https://github.com/iced-rs/iced/releases/tag/0.14.0)
- [iced wgpu custom rendering](https://deepwiki.com/iced-rs/iced/9.4-integration-and-custom-rendering)

### egui
- [egui_extras::TableBuilder](https://docs.rs/egui_extras/latest/egui_extras/struct.TableBuilder.html)
- [egui_extras::TableBody](https://docs.rs/egui_extras/latest/egui_extras/struct.TableBody.html)
- [egui table demo source](https://github.com/emilk/egui/blob/main/crates/egui_demo_lib/src/demo/table_demo.rs)
- [egui heterogeneous rows PR #1444](https://github.com/emilk/egui/pull/1444)
- [egui-selectable-table crate](https://docs.rs/egui-selectable-table)

### Slint
- [StandardTableView reference](https://docs.slint.dev/latest/docs/slint/reference/std-widgets/views/standardtableview/)
- [Slint Model trait](https://releases.slint.dev/1.0.0/docs/rust/slint/trait.model)
- [TableView discussion #574](https://github.com/sixtyfpsui/sixtyfps/discussions/574)
- [ListView >2M rows issue #3700](https://github.com/slint-ui/slint/issues/3700)

### Qt
- [Qt Model/View Programming](https://doc.qt.io/qt-6/model-view-programming.html)
- [QTableView class](https://doc.qt.io/qt-6/qtableview.html)
- [QStyledItemDelegate class](https://doc.qt.io/qt-6/qstyleditemdelegate.html)
- [Qt Quick TableView](https://doc.qt.io/qt-6/qml-qtquick-tableview.html)
- [Model/View Tutorial](https://doc.qt.io/qt-6/modelview.html)

### GTK4
- [GtkColumnView](https://docs.gtk.org/gtk4/class.ColumnView.html)
- [GtkListItemFactory](https://docs.gtk.org/gtk4/class.ListItemFactory.html)
- [GtkSignalListItemFactory](https://docs.gtk.org/gtk4/class.SignalListItemFactory.html)
- [GTK ListView primer](https://blog.gtk.org/2020/09/05/a-primer-on-gtklistview/)
- [Scalable lists in GTK4](https://blog.gtk.org/2020/06/07/scalable-lists-in-gtk-4/)
- [GTK4 ColumnView tutorial](https://github.com/ToshioCP/Gtk4-tutorial/blob/main/gfm/sec29.md)

### Dear ImGui
- [ImGui Tables System (DeepWiki)](https://deepwiki.com/ocornut/imgui/2.5-tables-system)
- [ImGui Tables API announcement](https://www.geeks3d.com/hacklab/20210129/dear-imgui-new-table-api/)
- [imgui_tables.cpp source](https://github.com/ocornut/imgui/blob/master/imgui_tables.cpp)
- [New Tables API issue #3740](https://github.com/ocornut/imgui/issues/3740)
- [imgui Rust bindings tables.rs](https://docs.rs/crate/imgui/latest/source/src/tables.rs)

### Architecture
- [IMGUI paradigm (ocornut wiki)](https://github.com/ocornut/imgui/wiki/About-the-IMGUI-paradigm)
- [Proving IMGUI performance](https://www.forrestthewoods.com/blog/proving-immediate-mode-guis-are-performant/)
- [Statefulness in GUIs](https://samsartor.com/guis-1/)
