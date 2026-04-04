# WPF DataGrid and Commercial Grid Controls -- Design Pattern Research

Research into the WPF DataGrid and commercial WPF grid controls (DevExpress GridControl,
Telerik RadGridView) with focus on design patterns transferable to a Rust/GPU-rendered grid widget.

---

## 1. WPF DataGrid Overview

The WPF `DataGrid` (`System.Windows.Controls.DataGrid`) is a XAML-based tabular data control
that displays and edits data from any bindable source implementing `IEnumerable`. It is the
canonical .NET desktop grid and the design baseline that all commercial WPF grids extend.

### XAML Column Definitions

Columns are declared either automatically (via `AutoGenerateColumns="True"`, the default) or
manually in XAML. When auto-generated, column types are inferred from data types:

| Data Type   | Generated Column Type        |
|-------------|------------------------------|
| `String`    | `DataGridTextColumn`         |
| `Boolean`   | `DataGridCheckBoxColumn`     |
| `Enum`      | `DataGridComboBoxColumn`     |
| `Uri`       | `DataGridHyperlinkColumn`    |

Manual column definition gives full control:

```xml
<DataGrid AutoGenerateColumns="False" ItemsSource="{Binding Instruments}">
    <DataGrid.Columns>
        <DataGridTextColumn Header="Symbol" Binding="{Binding Symbol}" Width="80" />
        <DataGridTextColumn Header="Last"   Binding="{Binding LastPrice, StringFormat=F2}" Width="*" />
        <DataGridCheckBoxColumn Header="Active" Binding="{Binding IsActive}" Width="60" />
        <DataGridTemplateColumn Header="Spark">
            <DataGridTemplateColumn.CellTemplate>
                <DataTemplate>
                    <local:SparklineControl Data="{Binding PriceHistory}" />
                </DataTemplate>
            </DataGridTemplateColumn.CellTemplate>
        </DataGridTemplateColumn>
    </DataGrid.Columns>
</DataGrid>
```

Key properties per column: `Header`, `Binding`, `Width`, `MinWidth`, `MaxWidth`,
`CanUserResize`, `CanUserReorder`, `CanUserSort`, `SortMemberPath`, `DisplayIndex`,
`IsReadOnly`, `Visibility`.

### DataTemplate System

The `DataTemplate` is WPF's mechanism for defining the visual tree used to render a data object.
In a DataGrid context:

- **CellTemplate** -- the read-only visual for a cell (any XAML element tree).
- **CellEditingTemplate** -- the editing visual, activated on F2 or double-click.
- **RowDetailsTemplate** -- an expandable detail pane below each row.
- **HeaderTemplate** -- custom column header content.

Templates receive their `DataContext` from the bound row object, so bindings like
`{Binding PropertyName}` resolve against the row's data item.

### Binding Model

The DataGrid binds to its data through `ItemsSource`, which accepts any `IEnumerable`. For
live updates the source should implement `INotifyCollectionChanged` (typically
`ObservableCollection<T>`), and each item should implement `INotifyPropertyChanged`.

For sorting/filtering/grouping, a `CollectionView` (or `CollectionViewSource`) wraps the
source collection, acting as an intermediary view layer that transforms display without
modifying the underlying data. This is a central MVVM pattern.

**Transferable pattern for Rust:** Separate the data model (`Vec<Row>` or a typed collection)
from a "view projection" layer that holds sort order, filter predicates, and visible-row
indices. The view projection is what the renderer iterates over.

---

## 2. Column Types

### DataGridTextColumn

The simplest column type. Creates a `TextBlock` in display mode and a `TextBox` in edit mode.
Properties: `Binding`, `FontStyle`, `FontWeight`, `Foreground`, `ElementStyle`,
`EditingElementStyle`.

```xml
<DataGridTextColumn Header="Price"
                    Binding="{Binding Price, StringFormat=N2}"
                    ElementStyle="{StaticResource RightAlignedStyle}" />
```

### DataGridCheckBoxColumn

Renders a `CheckBox` bound to a `Boolean` property. In display mode the checkbox is read-only;
in edit mode it becomes interactive. Properties: `IsThreeState` for nullable booleans.

### DataGridComboBoxColumn

Renders a drop-down list in edit mode. Commonly used for enum or lookup values. Properties:
`ItemsSource` (the list of choices), `SelectedValueBinding`, `DisplayMemberPath`,
`SelectedValuePath`.

### DataGridHyperlinkColumn

Renders a clickable `Hyperlink` element. Binding resolves to a `Uri`.

### DataGridTemplateColumn

The escape hatch for arbitrary cell content. You supply `CellTemplate` and optionally
`CellEditingTemplate` as full `DataTemplate` XAML trees. This is how custom controls --
sparklines, progress bars, color swatches, icon indicators -- are placed inside cells.

```xml
<DataGridTemplateColumn Header="Change">
    <DataGridTemplateColumn.CellTemplate>
        <DataTemplate>
            <TextBlock Text="{Binding Change, StringFormat=+0.00;-0.00;0.00}"
                       Foreground="{Binding Change, Converter={StaticResource SignToColorConverter}}" />
        </DataTemplate>
    </DataGridTemplateColumn.CellTemplate>
</DataGridTemplateColumn>
```

**Transferable pattern for Rust:** Define a `ColumnRenderer` trait with methods like
`measure_cell(row, col) -> Size` and `paint_cell(row, col, rect, canvas)`. Built-in
implementations cover text, checkbox, and icon columns. Custom renderers implement the trait
for sparklines, colored bars, etc.

---

## 3. Column Resizing

### Sizing Modes (DataGridLengthUnitType)

WPF DataGrid columns use `DataGridLength` values, which encode both a numeric value and a
unit type:

| Unit Type      | Behavior                                                                      |
|----------------|-------------------------------------------------------------------------------|
| `Pixel`        | Absolute width in device-independent pixels.                                  |
| `Auto`         | Sizes to the wider of cell content and header content. Grows on scroll but never shrinks. |
| `SizeToCells`  | Sizes based on cell content only, ignoring header width.                      |
| `SizeToHeader` | Sizes based on header content only, ignoring cell content.                    |
| `Star` (`*`)   | Distributes remaining space proportionally. `2*` gets twice the space of `1*`. |

Default: `DataGrid.ColumnWidth` is `SizeToHeader`; individual `DataGridColumn.Width` is `Auto`.

Star sizing is the mechanism that makes a grid "fill" its container. When the grid is resized,
star-sized columns expand or contract proportionally. Pixel-sized and auto-sized columns hold
their widths.

### Width Constraints

Each column supports `MinWidth` and `MaxWidth` to clamp sizing. The DataGrid also exposes
global defaults: `DataGrid.MinColumnWidth`, `DataGrid.MaxColumnWidth`, `DataGrid.ColumnWidth`.
Per-column values override global defaults.

### Resize Handles

Users resize columns by dragging the divider between column headers. Double-clicking the
divider auto-fits the column to its content (similar to Excel). The resize behavior is
controlled by:

- `DataGrid.CanUserResizeColumns` -- global toggle (default `true`).
- `DataGridColumn.CanUserResize` -- per-column toggle.
- `DataGrid.CanUserResizeRows` -- controls row height resizing via row header dividers.

The resize operation internally adjusts the `DataGridLength` value. If a star-sized column is
resized by the user, it becomes pixel-sized (the star proportion is lost).

**Transferable pattern for Rust:** Implement three column width modes: `Fixed(f32)`,
`Auto { min, max }`, and `Proportional(f32)`. On layout, first allocate fixed and auto columns,
then distribute remaining space among proportional columns. Store a `user_resized: bool` flag
that converts proportional to fixed when the user drags a resize handle. Hit-test the column
header divider region (a thin vertical strip, typically 4-6 px wide) to detect resize cursor
activation.

---

## 4. Column Reordering

### CanUserReorderColumns

Column reordering is enabled by default (`DataGrid.CanUserReorderColumns = true`). Individual
columns can opt out via `DataGridColumn.CanUserReorder = false`. When both the global and
per-column values conflict, `false` takes precedence.

Users reorder columns by dragging a column header. The underlying property is
`DataGridColumn.DisplayIndex`, an integer that maps the column's position in the visual layout
independently of its position in the `Columns` collection.

### Drag Visual During Reorder

WPF provides two customizable visuals during column reorder:

1. **DragIndicator** -- The floating visual that follows the cursor, representing the column
   being dragged. Customized via `DataGrid.DragIndicatorStyle` (targets `Control`).

2. **DropLocationIndicator** -- A vertical bar showing where the column will land if dropped.
   Customized via `DataGrid.DropLocationIndicatorStyle` (targets `Separator`).

### Column Reorder Events

- `DataGrid.ColumnReordering` -- fires before the reorder begins; can be cancelled.
- `DataGrid.ColumnReordered` -- fires after the column has been moved.
- `DataGrid.ColumnDisplayIndexChanged` -- fires when any column's `DisplayIndex` changes.

### Telerik RadGridView Reorder Behavior

Telerik extends this with `CanUserReorderColumns` at both the grid and column level, and adds
a visual column header "ghost" that shows the full header content while dragging, plus animated
drop-position indicators.

**Transferable pattern for Rust:** Maintain a `display_order: Vec<usize>` that maps visual
position to column index. On drag start, create a "ghost" overlay (a semi-transparent snapshot
of the column header) that follows the cursor. During drag, compute the insertion index by
comparing the cursor x-position against column boundary midpoints. Render a vertical insertion
indicator (2px wide, accent color) at the computed drop position. On drop, update
`display_order` and emit a `ColumnReordered` event.

---

## 5. Sorting

### Built-in Sort Behavior

Clicking a column header sorts the DataGrid by that column. The first click sorts ascending,
the second sorts descending, and the third clears the sort. The `CanUserSortColumns` property
(default `true`) enables or disables this globally; `DataGridColumn.CanUserSort` does so per
column.

### SortMemberPath

By default, sorting uses the column's `Binding` path. `SortMemberPath` overrides this, letting
a column display one property while sorting by another:

```xml
<DataGridTextColumn Header="Name"
                    Binding="{Binding DisplayName}"
                    SortMemberPath="SortKey" />
```

### Custom Sorting

For custom sort logic, handle the `DataGrid.Sorting` event:

```csharp
private void DataGrid_Sorting(object sender, DataGridSortingEventArgs e) {
    e.Handled = true; // prevent default sort
    var view = CollectionViewSource.GetDefaultView(dataGrid.ItemsSource) as ListCollectionView;
    view.CustomSort = new MyComparer(e.Column.SortMemberPath);
    e.Column.SortDirection = (e.Column.SortDirection == ListSortDirection.Ascending)
        ? ListSortDirection.Descending
        : ListSortDirection.Ascending;
}
```

The `ListCollectionView.CustomSort` property accepts an `IComparer` for full control.

### Multi-Column Sort

Hold `Shift` while clicking additional column headers to add secondary, tertiary, etc. sort
keys. Under the hood this adds multiple `SortDescription` entries to the `CollectionView`:

```csharp
ICollectionView view = CollectionViewSource.GetDefaultView(dataGrid.ItemsSource);
view.SortDescriptions.Add(new SortDescription("Sector", ListSortDirection.Ascending));
view.SortDescriptions.Add(new SortDescription("MarketCap", ListSortDirection.Descending));
```

### Sort Direction Indicators

The column header displays an arrow glyph (triangle) indicating sort direction:

- **Ascending**: upward-pointing triangle.
- **Descending**: downward-pointing triangle.
- **Unsorted**: no indicator.

The indicator is controlled by the `DataGridColumn.SortDirection` property (nullable
`ListSortDirection`). The default column header template includes a `Path` element (triangle
geometry `M0,0 L1,0 0.5,1 z`) with `LayoutTransform` triggers that rotate 180 degrees for
ascending. Custom header templates can replace this with any visual.

### ICollectionView Integration

When the DataGrid's `ItemsSource` is an `ICollectionView` that supports sorting (`CanSort`),
the DataGrid delegates all sort operations to the view. `SortDescriptions` added to the view
are automatically reflected in the DataGrid's header indicators, and vice versa.

**Transferable pattern for Rust:** Maintain `sort_state: Vec<(ColumnId, SortDirection)>` in
the view projection. On header click: if Shift is held, append; otherwise replace. Generate a
stable multi-key comparator from the sort state and apply it to produce `sorted_indices:
Vec<usize>`. Render triangle glyphs in column headers using the sort direction. For custom
sort, accept a `Fn(&Row, &Row) -> Ordering` callback per column.

---

## 6. Row Drag and Drop

WPF does not have built-in row drag-and-drop in the standard DataGrid. It must be implemented
manually using the WPF drag-and-drop framework, or via third-party libraries.

### WPF Drag-and-Drop Framework

The core API lives in `System.Windows.DragDrop`:

1. **Initiation**: In a `MouseMove` handler, detect drag threshold, then call
   `DragDrop.DoDragDrop(source, dataObject, allowedEffects)`. This is a **blocking call** --
   it enters a modal drag loop and does not return until the drag completes or is cancelled.

2. **Drop Target Setup**: Set `AllowDrop="True"` on the target element.

3. **Event Flow on Drop Target**:
   - `DragEnter` -- cursor enters the element boundary.
   - `DragOver` -- fires continuously while cursor is over the target.
   - `DragLeave` -- cursor exits without dropping.
   - `Drop` -- user releases the mouse button over the target.

4. **Event Flow on Drag Source**:
   - `GiveFeedback` -- fires continuously, used to set cursor appearance.
   - `QueryContinueDrag` -- fires on keyboard/mouse state change, used to cancel (ESC).

5. **Data Transfer**: Data is wrapped in a `DataObject`, which stores one or more
   format/object pairs. The drop target extracts data using `GetData(format)`.

6. **DragDropEffects**: An enum (`None`, `Copy`, `Move`, `Link`, `All`) communicated between
   source and target to indicate the intended operation.

### Implementing Row Drag in DataGrid

A typical implementation:

1. Handle `PreviewMouseLeftButtonDown` on a `DataGridRow` to record the start position.
2. In `MouseMove`, if the mouse has moved beyond `SystemParameters.MinimumHorizontalDragDistance`
   or `MinimumVerticalDragDistance`, call `DoDragDrop` with the row's data item.
3. In the DataGrid's `DragOver`, perform hit-testing to find the row under the cursor, then
   render a drop indicator (horizontal line above or below the target row).
4. In `Drop`, extract the data item, remove it from its old position, and insert it at the
   new position.

### Visual Feedback

WPF offers several approaches for drag visuals:

- **Cursor change only** -- The simplest, using `GiveFeedback` to set custom cursors.
- **Popup window** -- Create a borderless `Popup` that follows the mouse, containing a
  rendered preview of the dragged row.
- **Adorner** -- The recommended pattern (see Section 7 below).

### Drop Indicator Styles

Two common drop indicator patterns:

1. **Insertion line** -- A horizontal line (typically 2px, accent color) rendered between rows
   at the intended drop position. Syncfusion's `SfDataGrid.RowDropIndicatorMode = Line`.
2. **Arrow indicators** -- Small triangular arrows on the left and right edges pointing at the
   insertion point. This is Syncfusion's default style.

### Third-Party Libraries

**GongSolutions.WPF.DragDrop** is the most widely used open-source drag-and-drop framework
for WPF. It provides:

- `IDragSource` and `IDropTarget` interfaces for MVVM-compatible drag logic.
- `DefaultDragAdorner` -- a semi-transparent clone of the dragged element (default opacity 0.8).
- `DropTargetAdorners.Highlight` and `DropTargetAdorners.Insert` -- pre-built drop indicators.
- Automatic scroll during drag when near container edges.
- Full customization via `DragAdornerTemplate` and `DropAdornerTemplate`.

**Transferable pattern for Rust:** Implement row drag using a state machine:
`Idle -> DragDetecting(start_pos) -> Dragging(drag_state) -> Idle`. In `Dragging`, render a
ghost row (semi-transparent copy of the row content at 50-80% opacity) offset from the cursor.
Compute the target insertion index from the cursor's y-position relative to row boundaries
(using midpoint comparison). Render an insertion indicator line at the computed position. On
drop, reorder the data model and rebuild the view projection.

---

## 7. Custom Drag Visuals (DragAdorner Pattern)

### WPF Adorner System

Adorners in WPF are custom `FrameworkElement` instances rendered on an `AdornerLayer`, which
sits above the adorned element's visual tree in z-order. This makes them ideal for overlays
like drag previews, resize handles, selection rectangles, and validation indicators.

### Creating a DragAdorner

The standard pattern:

```csharp
public class DragAdorner : Adorner {
    private readonly Rectangle _child;
    private double _offsetX, _offsetY;

    public DragAdorner(UIElement adornedElement, UIElement draggedElement, double opacity)
        : base(adornedElement) {
        // Clone the visual using a VisualBrush
        var brush = new VisualBrush(draggedElement) {
            Opacity = opacity,
            Stretch = Stretch.None,
            AlignmentX = AlignmentX.Left,
            AlignmentY = AlignmentY.Top,
        };
        _child = new Rectangle {
            Width = draggedElement.RenderSize.Width,
            Height = draggedElement.RenderSize.Height,
            Fill = brush,
        };
        IsHitTestVisible = false; // Critical: don't intercept mouse events
    }

    public void UpdatePosition(double x, double y) {
        _offsetX = x;
        _offsetY = y;
        var layer = AdornerLayer.GetAdornerLayer(AdornedElement);
        layer?.Update(AdornedElement);
    }

    public override GeneralTransform GetDesiredTransform(GeneralTransform transform) {
        var result = new GeneralTransformGroup();
        result.Children.Add(base.GetDesiredTransform(transform));
        result.Children.Add(new TranslateTransform(_offsetX, _offsetY));
        return result;
    }

    protected override Size MeasureOverride(Size constraint) {
        _child.Measure(constraint);
        return _child.DesiredSize;
    }

    protected override Size ArrangeOverride(Size finalSize) {
        _child.Arrange(new Rect(_child.DesiredSize));
        return finalSize;
    }

    protected override Visual GetVisualChild(int index) => _child;
    protected override int VisualChildrenCount => 1;
}
```

### Adorner Lifecycle During Drag

1. **DragEnter / MouseDown**: Create the `DragAdorner` and add it to `AdornerLayer.GetAdornerLayer(element)`.
2. **DragOver / MouseMove**: Call `UpdatePosition()` with cursor coordinates translated to the adorner layer's coordinate space.
3. **DragLeave / Drop / Cancel**: Remove the adorner from the layer via `layer.Remove(adorner)`.

### Key Implementation Details

- **VisualBrush cloning**: The `VisualBrush` creates a visual snapshot of the source element.
  This is lightweight because it renders from the existing visual tree rather than creating
  new elements.
- **IsHitTestVisible = false**: Without this, the adorner would intercept mouse events and
  block drag-over detection on the target.
- **Coordinate translation**: Positions must be translated from screen/window coordinates to the
  adorner layer's coordinate space using `TranslatePoint` or `PointFromScreen`.
- **Performance**: Adorners should avoid complex layouts. For DataGrid row drags, render a
  simplified version of the row (just text values) rather than cloning the full visual tree.

### Relevance to Unreal UMG-Style Drag Widgets

Unreal Engine's UMG drag-and-drop uses a "DragVisualWidget" concept: when a drag starts, a
widget is created and rendered at the cursor position each frame, independent of the widget
hierarchy. This maps directly to the WPF Adorner pattern:

| WPF Concept              | UMG Equivalent                    | Rust Grid Equivalent           |
|--------------------------|-----------------------------------|--------------------------------|
| `Adorner`                | `UDragDropOperation::DefaultDragVisual` | `DragOverlay` struct    |
| `AdornerLayer`           | Viewport overlay pass             | Post-render overlay pass       |
| `VisualBrush` snapshot   | Widget snapshot texture           | Render-to-texture or cached draw commands |
| `IsHitTestVisible=false` | `Visibility::HitTestInvisible`    | Skip in hit-test traversal     |
| `UpdatePosition()`       | Tick update of widget position    | Per-frame position update      |

**Transferable pattern for Rust:** Create a `DragOverlay` that renders after the main widget
pass. When a drag starts, capture the dragged element's draw commands (or render to an
offscreen texture). Each frame, draw the overlay at `cursor_pos + offset` with reduced opacity.
The overlay must be excluded from hit-testing so that the target beneath the cursor can respond
to drag-over events.

---

## 8. Virtualization

### UI Virtualization

WPF's `VirtualizingStackPanel` is the default items panel for `DataGrid`. With virtualization
enabled (the default), only the visible rows have `DataGridRow` elements instantiated. Rows
scrolled out of view have their UI elements destroyed (Standard mode) or recycled (Recycling
mode).

Configuration:

```xml
<DataGrid VirtualizingStackPanel.IsVirtualizing="True"
          VirtualizingStackPanel.VirtualizationMode="Recycling"
          ScrollViewer.CanContentScroll="True">
```

### Standard vs. Recycling Mode

| Aspect           | Standard (Default)              | Recycling                        |
|------------------|---------------------------------|----------------------------------|
| Container life   | Created when visible, destroyed when scrolled away | Reused across rows |
| Memory           | Higher churn, more GC pressure  | Stable allocation after initial fill |
| Performance      | Adequate for small datasets     | 4-5x better scrolling for large datasets |
| State leakage    | No risk                         | Requires careful binding; avoid storing state in containers |
| Template cost    | Paid on every new row           | Paid once; subsequent rows only rebind data |

### Critical Configuration: CanContentScroll

`ScrollViewer.CanContentScroll="True"` (the default) enables **logical scrolling** -- the
scroll unit is one item (row). This is required for virtualization to function.

Setting `CanContentScroll="False"` switches to **physical (pixel) scrolling**, which gives
smoother scroll but **completely disables virtualization**, causing all rows to be
instantiated. This is a common anti-pattern that causes severe performance degradation with
large datasets.

### Column Virtualization

In addition to row virtualization, `DataGrid.EnableColumnVirtualization` (default `false`)
controls whether off-screen columns have their cell elements created. For wide grids with many
columns, enabling this improves performance:

```xml
<DataGrid EnableColumnVirtualization="True" />
```

### Data Virtualization

Data virtualization is distinct from UI virtualization. While UI virtualization only creates
UI elements for visible items, data virtualization loads data objects themselves on demand.
WPF does not provide built-in data virtualization, but it can be implemented through:

- **Custom `IList` implementations** that fetch pages from a backing store.
- **DevExpress Virtual Sources** (see Section 10).
- **Third-party libraries** like DevZest.DataVirtualization.

### Performance Numbers

| Dataset            | No Virtualization | UI Virtualization (Standard) | UI Virtualization (Recycling) |
|--------------------|-------------------|------------------------------|-------------------------------|
| 1,000 rows         | ~28 seconds       | ~200ms                       | ~150ms                        |
| 40,000 rows, 20 cols | Not feasible    | ~1 second                    | ~250ms                        |

**Transferable pattern for Rust:** Compute the visible row range from scroll offset and
viewport height: `first_visible = scroll_y / row_height`, `visible_count = viewport_height /
row_height + 1`. Only iterate and render rows in `[first_visible..first_visible+visible_count]`.
For column virtualization, similarly compute visible columns from horizontal scroll offset.
Maintain a small buffer (1-2 rows/columns) above and below the viewport for smooth scrolling.
For data virtualization, implement a page cache: `HashMap<PageIndex, Vec<Row>>` with an async
page-fetch mechanism.

---

## 9. Selection Model

### SelectionUnit

`DataGrid.SelectionUnit` controls the granularity of selection:

| Value          | Behavior                                                             |
|----------------|----------------------------------------------------------------------|
| `FullRow`      | Default. Clicking any cell selects the entire row.                   |
| `Cell`         | Only the clicked cell is selected.                                   |
| `CellOrRowHeader` | Clicking a cell selects the cell; clicking the row header selects the row. |

### SelectionMode

`DataGrid.SelectionMode` controls how many items can be selected:

| Value      | Behavior                                                                          |
|------------|-----------------------------------------------------------------------------------|
| `Single`   | Only one row/cell at a time.                                                      |
| `Extended` | Default. Single click selects one item. Ctrl+click toggles individual items. Shift+click selects a range. |

Note: WPF DataGrid does not expose a `Multiple` mode (click-to-toggle without modifier keys)
by default. Telerik's RadGridView adds `Multiple` as a distinct selection mode.

### Selection Properties and Events

- `SelectedItem` -- The first selected data object (for row selection).
- `SelectedItems` -- Collection of all selected data objects.
- `SelectedCells` -- Collection of `DataGridCellInfo` structs (for cell selection).
- `SelectedIndex` -- Index of the first selected item.
- `CurrentItem` / `CurrentCell` -- The cell with keyboard focus (distinct from selection).
- `SelectedCellsChanged` event -- fires when cell selection changes.
- `SelectionChanged` event -- fires when row selection changes.

### Programmatic Selection

```csharp
// Select all cells
dataGrid.SelectAllCells();

// Unselect all
dataGrid.UnselectAllCells();

// Select a specific row
dataGrid.SelectedItem = myDataObject;

// Select a specific cell
dataGrid.SelectedCells.Add(new DataGridCellInfo(myDataObject, myColumn));

// Focus a cell programmatically
dataGrid.CurrentCell = new DataGridCellInfo(myDataObject, myColumn);
dataGrid.BeginEdit();
```

### IsSynchronizedWithCurrentItem

When set to `true`, the DataGrid's selection is synchronized with the `ICollectionView.CurrentItem`.
This enables multiple controls to share the same "current item" pointer through the collection view.

**Transferable pattern for Rust:** Define selection state as:

```rust
enum SelectionUnit { FullRow, Cell, CellOrRowHeader }
enum SelectionMode { Single, Extended, Multiple }

struct SelectionState {
    unit: SelectionUnit,
    mode: SelectionMode,
    anchor: Option<CellCoord>,         // start of range selection
    selected_rows: BTreeSet<usize>,    // for FullRow mode
    selected_cells: BTreeSet<CellCoord>, // for Cell mode
    current: Option<CellCoord>,        // keyboard focus cell
}
```

On click: if `Single`, clear and select; if `Extended`, check Ctrl/Shift modifiers to toggle
or range-select; if `Multiple`, toggle the clicked item. Emit `SelectionChanged` events for
external consumers.

---

## 10. DevExpress / Telerik Extras

Commercial WPF grids build substantially on the base DataGrid, adding features critical for
financial and data-intensive applications.

### DevExpress GridControl

**Views**: The DevExpress GridControl supports multiple view types for the same data:

- **TableView** -- Standard flat grid (equivalent to DataGrid).
- **TreeListView** -- Hierarchical rows with expand/collapse.
- **CardView** -- Each row rendered as a card with vertical property layout.
- **BandedView** -- Columns organized into multi-level header bands.

**Band Headers**: Columns can be grouped under parent "band" headers, creating a multi-tier
header row. Bands are stored in `GridControl.Bands` and each band contains child columns or
nested sub-bands. This is critical for financial applications (e.g., grouping "Bid Price",
"Bid Size", "Bid Time" under a "Bid" band).

```xml
<dxg:GridControl.Bands>
    <dxg:GridControlBand Header="Bid">
        <dxg:GridColumn FieldName="BidPrice" />
        <dxg:GridColumn FieldName="BidSize" />
    </dxg:GridControlBand>
    <dxg:GridControlBand Header="Ask">
        <dxg:GridColumn FieldName="AskPrice" />
        <dxg:GridColumn FieldName="AskSize" />
    </dxg:GridControlBand>
</dxg:GridControl.Bands>
```

**Fixed (Frozen) Columns**: Anchor columns to the left or right edge so they remain visible
during horizontal scrolling. Set via `BaseColumn.Fixed = FixedStyle.Left | FixedStyle.Right`.

**Summary Rows**: Display aggregate values (Sum, Avg, Min, Max, Count, custom) in a footer row
below the grid or in group footers. Summary functions can be defined per-column.

**Conditional Formatting**: Rule-based visual changes (background color, font, icon sets,
data bars) applied to cells or rows. Uses a Criteria Language Syntax to express conditions:
`[Price] > 100`, `[Change] < 0`.

**Virtual Sources**: For very large or remote datasets, `InfiniteAsyncSource` and
`PagedAsyncSource` support on-demand data loading with full MVVM support. Only visible data is
fetched; sorting, filtering, and summaries can be delegated to the server.

**Server Mode**: Direct integration with Entity Framework, XPO, EF Core, and OData for
server-side data processing. The grid sends queries to the data source and only retrieves
the results needed for the current view.

### Telerik RadGridView

**Frozen Columns**: `FrozenColumnCount` (now `LeftFrozenColumnCount` / `RightFrozenColumnCount`
as of R1 2018) pins columns to either edge. Frozen columns remain fixed while the rest scroll
horizontally.

**Column Groups**: `GridViewColumnGroup` instances organize columns under shared headers.
Groups can be nested. The group header stays visible while any of its child columns are in the
viewport:

```xml
<telerik:RadGridView.ColumnGroups>
    <telerik:GridViewColumnGroup Name="MarketData" Header="Market Data">
        <telerik:GridViewColumnGroup Name="Bid" Header="Bid" />
        <telerik:GridViewColumnGroup Name="Ask" Header="Ask" />
    </telerik:GridViewColumnGroup>
</telerik:RadGridView.ColumnGroups>
```

**Aggregate Functions**: Built-in aggregates (Sum, Count, Min, Max, Average, First, Last) plus
custom aggregate functions. Results display in column footers and/or group footers. Controlled
by `ShowColumnFooters` and `ShowGroupFooters` properties.

**Grouping**: Drag column headers to a "group panel" to create row groups. Multiple grouping
levels with expandable/collapsible group headers showing aggregate values.

**Mixed Selection**: Unique to Telerik -- supports simultaneous cell and row selection with
`SelectionUnit = Mixed`.

**Data Export**: Built-in export to Excel, CSV, PDF, and HTML with formatting preservation.

**Conditional Formatting**: Via `CellStyleSelector` and `CellTemplateSelector` -- assign
different styles or templates to cells based on their data values.

**Row Details**: Expandable detail panes under each row, configurable via `RowDetailsTemplate`.

**UI Virtualization**: Telerik implements its own virtualization engine independent of
`VirtualizingStackPanel`, with both row and column virtualization enabled by default and
optimized for their specific layout pipeline.

### Feature Comparison Summary

| Feature                   | WPF DataGrid | DevExpress GridControl | Telerik RadGridView |
|---------------------------|:---:|:---:|:---:|
| Basic columns             | Y | Y | Y |
| Template columns          | Y | Y | Y |
| Band/group headers        | N | Y | Y |
| Frozen columns            | Y (limited) | Y (left+right) | Y (left+right) |
| Sorting (single)          | Y | Y | Y |
| Sorting (multi)           | Y | Y | Y |
| Custom sort               | Y | Y | Y |
| Grouping with DnD         | N | Y | Y |
| Aggregate footer           | N | Y | Y |
| Conditional formatting    | Manual | Built-in rules | Via selectors |
| Row details               | Y | Y | Y |
| Column virtualization     | Y | Y | Y |
| Data virtualization       | N | Y (Virtual Sources) | N |
| Server mode               | N | Y | N |
| Row drag-and-drop         | Manual | Y | Y |
| Export (Excel/CSV)        | N | Y | Y |
| Multiple views (Card/Tree)| N | Y | N |
| MVVM command binding      | Limited | Extensive | Extensive |

**Transferable patterns for Rust:**

- **Band headers**: Model as a tree of `HeaderNode` (leaf = column, branch = band with
  children). Render header rows recursively, with bands spanning the combined width of their
  children.
- **Frozen columns**: Partition columns into three zones: `left_frozen`, `scrollable`,
  `right_frozen`. Render frozen zones first (clipped to their area), then render the scrollable
  zone between them. Frozen columns are excluded from horizontal scroll offset calculations.
- **Aggregates**: Accept `Fn(&[Row]) -> String` per column for footer computation. Recompute
  on data change or group collapse/expand. Render in a fixed footer row.
- **Conditional formatting**: Accept `Fn(&Row, ColumnId) -> Option<CellStyle>` callbacks.
  Evaluate per-cell during rendering to determine background color, text color, font weight.

---

## 11. Design Patterns

### MVVM Binding Pattern

The Model-View-ViewModel (MVVM) pattern is central to WPF DataGrid usage:

- **Model**: Data classes (`INotifyPropertyChanged`).
- **ViewModel**: Exposes an `ObservableCollection<T>` and an `ICollectionView` for
  sorting/filtering/grouping. Also exposes commands for add, delete, edit.
- **View**: The DataGrid in XAML binds `ItemsSource` to the collection, columns bind to
  individual properties.

The ViewModel never references the DataGrid directly. All communication is through data binding
and the `ICollectionView` abstraction.

```csharp
public class WatchlistViewModel : INotifyPropertyChanged {
    public ObservableCollection<Instrument> Instruments { get; }
    public ICollectionView InstrumentsView { get; }

    public WatchlistViewModel() {
        Instruments = new ObservableCollection<Instrument>();
        InstrumentsView = CollectionViewSource.GetDefaultView(Instruments);
        InstrumentsView.SortDescriptions.Add(
            new SortDescription("Symbol", ListSortDirection.Ascending));
        InstrumentsView.Filter = obj => {
            var inst = (Instrument)obj;
            return inst.IsActive || ShowInactive;
        };
    }

    private bool _showInactive;
    public bool ShowInactive {
        get => _showInactive;
        set {
            _showInactive = value;
            InstrumentsView.Refresh();
            OnPropertyChanged();
        }
    }
}
```

### ICollectionView for Sorting and Filtering

`ICollectionView` is the most important abstraction in WPF data presentation. It wraps a
source collection and provides:

- **SortDescriptions**: Ordered list of `(PropertyName, Direction)` pairs.
- **GroupDescriptions**: Ordered list of `PropertyGroupDescription` for hierarchical grouping.
- **Filter**: A `Predicate<object>` that controls item visibility.
- **CurrentItem / MoveCurrentTo***: A cursor for "current item" navigation.
- **DeferRefresh()**: Batches multiple changes (add sort + add group + set filter) into a
  single refresh pass.

The collection view is a **non-destructive projection** -- the source data is never modified.
Multiple views can wrap the same source collection with different sort/filter/group settings.

**Transferable pattern for Rust:**

```rust
pub struct CollectionView<T> {
    source: Vec<T>,
    sort_keys: Vec<SortKey>,
    filter: Option<Box<dyn Fn(&T) -> bool>>,
    group_by: Option<Box<dyn Fn(&T) -> String>>,
    // Computed on refresh:
    visible_indices: Vec<usize>,
    groups: Vec<Group>,
}

impl<T> CollectionView<T> {
    pub fn refresh(&mut self) {
        // 1. Apply filter
        let filtered: Vec<usize> = self.source.iter().enumerate()
            .filter(|(_, item)| self.filter.as_ref().map_or(true, |f| f(item)))
            .map(|(i, _)| i)
            .collect();
        // 2. Apply sort
        let mut sorted = filtered;
        sorted.sort_by(|&a, &b| self.compare(&self.source[a], &self.source[b]));
        // 3. Apply grouping
        self.groups = self.compute_groups(&sorted);
        self.visible_indices = sorted;
    }

    pub fn iter_visible(&self) -> impl Iterator<Item = &T> {
        self.visible_indices.iter().map(|&i| &self.source[i])
    }
}
```

### DataTemplate for Cell Customization

The template pattern decouples rendering from data:

1. Each column type has a **default template** (TextBlock for text, CheckBox for bool).
2. Developers override with custom templates via `CellTemplate`.
3. Templates resolve bindings against the row's data context.
4. **Template selectors** (`DataTemplateSelector`) can choose different templates per-row
   based on the data, enabling heterogeneous cell rendering within a single column.

This corresponds to the Strategy pattern: the grid delegates cell rendering to interchangeable
template objects.

**Transferable pattern for Rust:** Define cell rendering as a trait:

```rust
pub trait CellRenderer {
    fn measure(&self, data: &dyn Any, ctx: &MeasureContext) -> Size;
    fn paint(&self, data: &dyn Any, rect: Rect, canvas: &mut Canvas);
    fn hit_test(&self, data: &dyn Any, rect: Rect, point: Point) -> Option<HitResult>;
}
```

Register renderers per column. The grid's paint loop calls `column.renderer.paint(row_data,
cell_rect, canvas)` for each visible cell. Built-in renderers handle text, numeric (with
alignment and formatting), boolean (checkbox icon), and color-coded change values. Custom
renderers handle sparklines, mini-charts, and composite cells.

---

## Summary of Transferable Patterns

| WPF Concept           | Rust Grid Equivalent                                              |
|-----------------------|-------------------------------------------------------------------|
| `DataTemplate`        | `CellRenderer` trait with `measure` / `paint` methods             |
| `ICollectionView`     | `CollectionView<T>` with filter/sort/group producing index arrays |
| `DataGridLength`      | `ColumnWidth` enum: `Fixed(f32)`, `Auto`, `Proportional(f32)`    |
| `VirtualizingStackPanel` | Viewport-based row range calculation with buffer                |
| `Adorner` + `AdornerLayer` | `DragOverlay` rendered in post-widget pass, hit-test excluded |
| `DragIndicatorStyle`  | Ghost column header rendered at cursor during reorder             |
| `SelectionUnit/Mode`  | `SelectionState` struct with set-based tracking                   |
| `SortDescription`     | `Vec<(ColumnId, SortDirection)>` multi-key sort state             |
| `ObservableCollection`| Data source with change notification channel                      |
| Band headers          | `HeaderNode` tree: leaf = column, branch = band                   |
| Frozen columns        | Three-zone layout: left-frozen, scrollable, right-frozen          |
| Conditional formatting| `Fn(&Row, ColumnId) -> Option<CellStyle>` per-cell callback       |

---

## Sources

- [DataGrid - WPF | Microsoft Learn](https://learn.microsoft.com/en-us/dotnet/desktop/wpf/controls/datagrid)
- [Sizing Options in the DataGrid Control - WPF | Microsoft Learn](https://learn.microsoft.com/en-us/dotnet/desktop/wpf/controls/sizing-options-in-the-datagrid-control)
- [How to: Group, Sort, and Filter Data in the DataGrid Control - WPF | Microsoft Learn](https://learn.microsoft.com/en-us/dotnet/desktop/wpf/controls/how-to-group-sort-and-filter-data-in-the-datagrid-control)
- [DataGrid Styles and Templates - WPF | Microsoft Learn](https://learn.microsoft.com/en-us/dotnet/desktop/wpf/controls/datagrid-styles-and-templates)
- [Drag and Drop Overview - WPF | Microsoft Learn](https://learn.microsoft.com/en-us/dotnet/desktop/wpf/advanced/drag-and-drop-overview)
- [Adorners Overview - WPF | Microsoft Learn](https://learn.microsoft.com/en-us/dotnet/desktop/wpf/controls/adorners-overview)
- [DataGrid.SelectionUnit Property | Microsoft Learn](https://learn.microsoft.com/en-us/dotnet/api/system.windows.controls.datagrid.selectionunit)
- [DataGrid.CanUserReorderColumns Property | Microsoft Learn](https://learn.microsoft.com/en-us/dotnet/api/system.windows.controls.datagrid.canuserreordercolumns)
- [DataGrid.ColumnReordering Event | Microsoft Learn](https://learn.microsoft.com/en-us/dotnet/api/system.windows.controls.datagrid.columnreordering)
- [DataGridTemplateColumn Class | Microsoft Learn](https://learn.microsoft.com/en-us/dotnet/api/system.windows.controls.datagridtemplatecolumn)
- [Data Templating Overview - WPF | Microsoft Learn](https://learn.microsoft.com/en-us/dotnet/desktop/wpf/data/data-templating-overview)
- [DataGrid columns - The complete WPF tutorial](https://wpf-tutorial.com/datagrid-control/custom-columns/)
- [Virtualization in WPF Using VirtualizingStackPanel](https://www.c-sharpcorner.com/UploadFile/mahesh/virtualization-in-wpf-using-virtualizingstackpanel/)
- [XAML Anti-Patterns: Virtualization](https://codemag.com/Article/1407081/XAML-Anti-Patterns-Virtualization)
- [WPF DataGrid - UI Virtualization | Telerik](https://docs.telerik.com/devtools/wpf/controls/radgridview/features/ui-virtualization)
- [Data Grid | DevExpress WPF Documentation](https://docs.devexpress.com/WPF/6084/controls-and-libraries/data-grid)
- [Fixed Columns and Bands | DevExpress WPF Documentation](https://docs.devexpress.com/WPF/6302/controls-and-libraries/data-grid/grid-view-data-layout/columns-and-card-fields/fixed-columns-and-bands)
- [Band Column | DevExpress WPF Documentation](https://docs.devexpress.com/WPF/117796/controls-and-libraries/data-grid/visual-elements/common-elements/band-column)
- [Conditional Formatting | DevExpress WPF Documentation](https://docs.devexpress.com/WPF/17130/controls-and-libraries/data-grid/conditional-formatting)
- [Virtual Sources Overview | DevExpress WPF Documentation](https://docs.devexpress.com/WPF/120193/controls-and-libraries/data-grid/bind-to-data/bind-to-any-data-source-with-virtual-sources/virtual-sources-overview)
- [Sorting Modes and Custom Sorting | DevExpress WPF Documentation](https://docs.devexpress.com/WPF/6142/controls-and-libraries/data-grid/sorting/sorting-modes-and-custom-sorting)
- [RadGridView Key Features | Telerik UI for WPF](https://docs.telerik.com/devtools/wpf/controls/radgridview/general-information/key-features)
- [RadGridView Reordering Columns | Telerik UI for WPF](https://docs.telerik.com/devtools/wpf/controls/radgridview/columns/reordering-columns)
- [RadGridView Frozen Columns | Telerik UI for WPF](https://docs.telerik.com/devtools/wpf/controls/radgridview/columns/frozen-columns)
- [RadGridView Column Groups | Telerik UI for WPF](https://docs.telerik.com/devtools/wpf/controls/radgridview/columns/column-groups)
- [RadGridView Basic Sorting | Telerik UI for WPF](https://docs.telerik.com/devtools/wpf/controls/radgridview/sorting/basics)
- [RadGridView Custom Sorting | Telerik UI for WPF](https://docs.telerik.com/devtools/wpf/controls/radgridview/sorting/custom)
- [RadGridView Selection Basics | Telerik UI for WPF](https://docs.telerik.com/devtools/wpf/controls/radgridview/selection/basics)
- [Sorting in WPF DataGrid | Syncfusion](https://help.syncfusion.com/wpf/datagrid/sorting)
- [Row Drag and Drop in WPF DataGrid | Syncfusion](https://help.syncfusion.com/wpf/datagrid/drag-and-drop)
- [Selection in WPF DataGrid | Syncfusion](https://help.syncfusion.com/wpf/datagrid/selection)
- [GongSolutions.WPF.DragDrop (GitHub)](https://github.com/punker76/gong-wpf-dragdrop)
- [WPF: Drag Drop Adorner - Code Blitz](https://codeblitz.wordpress.com/2009/06/17/wpf-drag-drop-adorner/)
- [Showing Drag/Drop Feedback on the WPF Adorner Layer | Microsoft Learn](https://learn.microsoft.com/en-us/archive/blogs/marcelolr/showing-dragdrop-feedback-on-the-wpf-adorner-layer)
- [WPF DataGrid Binding - DEV Community](https://dev.to/jwp/wpf-datagrid-binding-3cpc)
- [Multi-filtered WPF DataGrid with MVVM - CodeProject](https://www.codeproject.com/Articles/442498/Multi-filtered-WPF-DataGrid-with-MVVM)
