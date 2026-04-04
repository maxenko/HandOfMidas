# AG Grid -- The Gold Standard Data Grid

> Research compiled from AG Grid documentation, blog posts, and community resources, 2026-04-01

---

## 1. Overview

AG Grid is a high-performance, feature-rich data grid component for JavaScript applications. It supports React, Angular, Vue, and vanilla JavaScript. Originally created by Niall Crosby in 2015, it has grown into the most widely adopted enterprise data grid in the JavaScript ecosystem.

### Who Uses It

Over 90% of Fortune 500 companies reportedly use AG Grid, with notable adopters including:

- **J.P. Morgan** -- Salt, their open-source design system for financial services, uses AG Grid as its Data Grid component
- **MongoDB** -- Compass, the GUI for MongoDB, uses AG Grid to visualize and manage database contents
- **NASA AMMOS** -- Planning, scheduling, and sequencing tools for space missions use AG Grid to visualize mission data
- **Adobe, Microsoft, PayPal, IBM** -- All enterprise license holders

AG Grid has signed over 1,000 enterprise customers.

### Why It Is the Gold Standard

1. **Performance** -- Handles millions of rows via DOM virtualization. Only visible rows and columns are rendered.
2. **Feature depth** -- 63+ features out of the box: sorting, filtering, grouping, pivoting, aggregation, tree data, master/detail, sparklines, clipboard, Excel export.
3. **Framework-native** -- True React/Angular/Vue components, not wrappers. Cell renderers are native framework components.
4. **Two editions** -- Community (free, MIT) covers core grid features. Enterprise adds pivoting, tree data, aggregation, row grouping, range selection, sparklines, Excel export, and server-side row model.
5. **API surface** -- Both declarative (column definitions, grid options) and imperative (Grid API for programmatic control).

### Relevance to a Native Desktop Trading Watchlist

AG Grid's design is directly relevant even when building a native (non-web) grid because it codifies decades of UX patterns for financial data display:

- Column definitions as a declarative schema
- Virtual scrolling for large symbol lists
- Real-time cell updates with selective refresh
- Column resize, reorder, pin patterns
- Row selection for order entry
- Sparkline integration for inline trend visualization
- Sorting with custom comparators (e.g., sort by % change magnitude)

---

## 2. Core Features

### 2.1 Column Definitions (ColDef)

Every column in AG Grid is described by a `ColDef` object. An array of `ColDef` objects is passed as the `columnDefs` grid option.

```typescript
const columnDefs: ColDef[] = [
  { field: 'symbol', headerName: 'Symbol', width: 100 },
  { field: 'last',   headerName: 'Last',   width: 80,  valueFormatter: priceFormatter },
  { field: 'change', headerName: 'Chg',    width: 70,  cellRenderer: ChangeRenderer },
  {
    colId: 'volume',
    headerName: 'Volume',
    valueGetter: (params) => params.data.bid * params.data.ask,
    valueFormatter: (params) => params.value.toLocaleString(),
  },
];
```

**Key ColDef properties:**

| Property | Purpose |
|---|---|
| `field` | Maps to a property on the row data object. Supports dot notation (`medals.gold`). |
| `colId` | Unique identifier when `field` is not provided or not unique. |
| `headerName` | Display name in the column header. |
| `width` / `minWidth` / `maxWidth` | Pixel dimensions. |
| `flex` | Proportional sizing -- distributes remaining space among flex columns. |
| `type` | References a named column type (see below). |
| `sortable` | Enable/disable sorting on this column. |
| `resizable` | Enable/disable resize handle on this column. |
| `pinned` | `'left'` or `'right'` to pin the column. |
| `lockPosition` | `'left'` or `'right'` to permanently lock position, preventing user drag. |
| `suppressMovable` | Prevents the user from dragging this column. |
| `cellRenderer` | Component reference for custom cell rendering. |
| `cellRendererParams` | Props passed to the cell renderer component. |
| `valueGetter` | Function to compute cell value from row data. |
| `valueFormatter` | Function to format the cell value for display (text only, no HTML). |
| `comparator` | Custom sort comparator function. |
| `cellStyle` / `cellClass` | Inline styles or CSS classes applied to cells. |

### 2.2 Default Column Definitions and Column Types

The `defaultColDef` grid option applies shared properties to all columns, reducing repetition:

```typescript
const gridOptions = {
  defaultColDef: {
    sortable: true,
    resizable: true,
    minWidth: 60,
    flex: 1,
  },
  columnDefs: [ /* ... */ ],
};
```

**Column Types** define reusable property bundles:

```typescript
const columnTypes = {
  currency: {
    valueFormatter: currencyFormatter,
    cellClass: 'align-right',
    width: 90,
  },
  pctChange: {
    valueFormatter: pctFormatter,
    cellRenderer: ChangeRenderer,
    width: 80,
  },
};

// Usage in colDef:
{ field: 'last', type: 'currency' }
{ field: 'change', type: ['pctChange'] }  // Can apply multiple types as array
```

**Application order:** defaultColDef -> columnTypes -> individual colDef. Later layers override earlier ones.

### 2.3 Data Type Inference

AG Grid automatically infers Cell Data Types from row data for common types (`text`, `number`, `boolean`, `date`, `dateString`), configuring appropriate rendering, editing, filtering, and sorting without explicit configuration.

### 2.4 Row Model

AG Grid provides four row models:

| Row Model | Data Location | Use Case |
|---|---|---|
| **Client-Side** (default) | All data in browser memory | Small-to-medium datasets. Sorting, filtering, grouping all happen client-side. |
| **Infinite** | Server, loaded in blocks | Large flat lists. Rows fetched as user scrolls. |
| **Server-Side (SSRM)** | Server, lazy-loaded | Very large grouped/pivoted datasets. Groups expand on demand, aggregation on server. |
| **Viewport** | Server knows exact visible range | Real-time data push. Server sends only what the user sees. Ideal for live trading feeds. |

For a trading watchlist, the **Client-Side** model suffices (watchlists rarely exceed a few thousand symbols). The **Viewport** model is noteworthy for real-time streaming scenarios where the server pushes only data for visible rows.

### 2.5 Cell Rendering Pipeline

The rendering pipeline has three stages, each addressing a different concern:

1. **Value Getter** -- Extracts or computes the raw value from row data.
   - `field: 'price'` is the simple case.
   - `valueGetter: (params) => params.data.bid + params.data.ask` for computed values.

2. **Value Formatter** -- Transforms the raw value into a display string. Text only, no HTML.
   - Used for number formatting, currency symbols, date formatting.
   - Applied to CSV export and clipboard copy as well.

3. **Cell Renderer** -- Replaces the entire cell content with a custom component (HTML/React/Angular/Vue).
   - Used for buttons, icons, sparklines, progress bars, color-coded badges.
   - NOT applied to exports or clipboard.

**Rule of thumb:** Use `valueFormatter` for text transformations (punctuation, units). Use `cellRenderer` when you need HTML markup or interactive elements.

---

## 3. Column Management

### 3.1 Resizing

Columns are resizable by default (controlled by `resizable: true` on the colDef or defaultColDef).

**Resize handle:** A thin draggable region appears at the right edge of each column header. The user drags it horizontally to resize.

**Double-click to auto-size:** Double-clicking the resize handle auto-sizes the column to fit its content (header and cell values).

**Flex vs. Fixed sizing:**

- **Fixed** (`width: 150`): Column is exactly 150px. Manual resize changes `width` and disables `flex`.
- **Flex** (`flex: 1`): Column takes a proportional share of remaining space. `flex` and `width` cannot coexist on the same column. Use `minWidth` / `maxWidth` to constrain flex columns.

```typescript
// Flex example: three columns, last takes 2x the space of others
{ field: 'symbol', flex: 1, minWidth: 80 },
{ field: 'last',   flex: 1, minWidth: 60 },
{ field: 'volume', flex: 2, minWidth: 100 },
```

If a user manually resizes a flex column (via drag or API), flex is automatically disabled for that column and it becomes fixed-width.

**Programmatic sizing:**

```typescript
// Size all columns to fit grid width
api.sizeColumnsToFit();

// Auto-size specific columns to fit content
api.autoSizeColumns(['symbol', 'last', 'volume']);

// Auto-size all columns
api.autoSizeAllColumns();
```

**`suppressSizeToFit`:** Set on a colDef to exclude that column from `sizeColumnsToFit()`.

**`suppressAutoSize`:** Set on a colDef to exclude that column from `autoSizeColumns()`.

**`autoSizeStrategy`:** Grid option to auto-size on initial load:

```typescript
autoSizeStrategy: {
  type: 'fitGridWidth',  // or 'fitProvidedWidth'
  // defaultMinWidth: 80,
}
```

**Pinned column resize limit:** When resizing pinned columns, the pinned area's width is limited to the grid width minus 50px, preventing the pinned section from consuming the entire grid.

### 3.2 Reordering (Drag Headers)

Columns can be reordered by dragging their headers. This is enabled by default.

**How it works:**
1. User clicks and holds a column header.
2. A drag ghost (semi-transparent copy of the header) follows the cursor.
3. As the ghost moves over other headers, the grid shows animated position transitions -- columns slide left/right to indicate where the drop will place the column.
4. On release, the column moves to the new position.

**Animation:** Column move animations transition only the position property. To disable: `suppressColumnMoveAnimation: true`.

**Preventing reorder:**
- `suppressMovable: true` on a colDef prevents the user from dragging that specific column.
- `lockPosition: 'left'` or `'right'` permanently locks the column to one side and prevents any drag in or out.

**Programmatic column move:**

```typescript
api.moveColumn('volume', 2);           // Move 'volume' column to index 2
api.moveColumns(['bid', 'ask'], 3);    // Move multiple columns
api.moveColumnByIndex(0, 4);           // Move column at index 0 to index 4
```

**Custom drag image:** The drag ghost image can be customized via `dragAndDropImageComponent` and `dragAndDropImageComponentParams` grid options. The grid uses its own internal drag implementation (not native browser drag-and-drop) for finer control.

### 3.3 Pinning

Columns can be pinned to the left or right edge of the grid, remaining visible as the user scrolls horizontally.

```typescript
{ field: 'symbol', pinned: 'left' },   // Always visible on left
{ field: 'actions', pinned: 'right' },  // Always visible on right
```

**User-initiated pinning:**
- When other columns are already pinned, the user can drag a column into the pinned area.
- When no columns are pinned, dragging a column to the grid edge and holding for ~1 second will create a pinned area.

**API:**

```typescript
api.setColumnPinned('symbol', 'left');   // Pin programmatically
api.setColumnPinned('symbol', null);     // Unpin
```

### 3.4 Column Groups

Columns can be grouped under a shared header. There is no limit to nesting depth.

```typescript
const columnDefs = [
  { field: 'symbol' },
  {
    headerName: 'Pricing',
    children: [
      { field: 'bid' },
      { field: 'ask' },
      { field: 'last' },
    ],
  },
  {
    headerName: 'Performance',
    children: [
      { field: 'change' },
      { field: 'changePct' },
    ],
  },
];
```

**Group header height:** `groupHeaderHeight` grid option (defaults to `headerHeight`).

**Collapsible groups:** If a group contains columns whose visibility depends on the group's open/closed state, the group header shows an expand/collapse icon. Custom group header components (`headerGroupComponent`) can override this behavior.

**Custom group headers:** Use `innerHeaderGroupComponent` when you only need to customize the group name text without reimplementing expand/collapse logic.

### 3.5 Auto-Size Columns

```typescript
// Auto-size to fit cell content
api.autoSizeColumns(['symbol', 'last']);

// Auto-size to fit cell content, including header
api.autoSizeColumns(['symbol', 'last'], { skipHeader: false });
```

---

## 4. Sorting

### 4.1 Basic Sorting

Sorting is enabled per-column via `sortable: true` (typically set in `defaultColDef`).

**Click cycle:**
1. First click -- ascending (arrow up indicator)
2. Second click -- descending (arrow down indicator)
3. Third click -- no sort (indicator removed)

This cycle is controlled by `sortingOrder`:

```typescript
// Default cycle
sortingOrder: ['asc', 'desc', null]

// Only allow ascending and descending (no unsorted state)
sortingOrder: ['asc', 'desc']

// Start with descending
sortingOrder: ['desc', 'asc', null]
```

**`unSortIcon: true`:** Displays a sort icon even in the unsorted state, improving visual feedback by showing that the column is sortable.

### 4.2 Multi-Column Sort

By default, the user holds **Shift** and clicks additional column headers to add secondary, tertiary, etc. sort levels. A small number badge appears on each sorted column indicating its sort priority (1, 2, 3...).

```typescript
// Change multi-sort key to Ctrl/Cmd instead of Shift
multiSortKey: 'ctrl'

// Always apply multi-sort (no modifier key needed)
alwaysMultiSort: true

// Disable multi-sort entirely
suppressMultiSort: true
```

### 4.3 Custom Comparators

Custom sort logic is specified per-column via the `comparator` property:

```typescript
{
  field: 'date',
  comparator: (valueA, valueB, nodeA, nodeB, isDescending) => {
    const dateA = new Date(valueA);
    const dateB = new Date(valueB);
    return dateA.getTime() - dateB.getTime();
  },
}
```

The comparator receives:
- `valueA`, `valueB` -- the cell values being compared
- `nodeA`, `nodeB` -- the full row nodes (access to all row data)
- `isDescending` -- boolean indicating sort direction

Return negative, zero, or positive (standard comparator contract).

### 4.4 Post-Sort Processing

The `postSortRows` grid callback allows additional manipulation after sorting completes:

```typescript
postSortRows: (params) => {
  // Example: always keep a specific row at the top
  const rowNodes = params.nodes;
  const pinnedIdx = rowNodes.findIndex(n => n.data.pinned);
  if (pinnedIdx > 0) {
    const [pinned] = rowNodes.splice(pinnedIdx, 1);
    rowNodes.unshift(pinned);
  }
}
```

This is useful in a watchlist for pinning a "totals" or "benchmark" row at the top regardless of sort.

### 4.5 Programmatic Sort

```typescript
// Apply sort via column state API
api.applyColumnState({
  state: [
    { colId: 'changePct', sort: 'desc', sortIndex: 0 },
    { colId: 'volume',    sort: 'desc', sortIndex: 1 },
  ],
  defaultState: { sort: null },  // Clear sort on other columns
});
```

### 4.6 Accented / Absolute Sort

`accentedSort: true` enables locale-sensitive comparison for accented characters. AG Grid also supports absolute sorting (sorting by magnitude, ignoring sign), useful for ranking values by size regardless of positive/negative.

---

## 5. Row Drag and Drop

### 5.1 Enabling Row Drag

Row dragging is enabled by setting `rowDrag: true` on a column definition (typically the first column). This adds a drag handle icon to each row in that column.

```typescript
const columnDefs = [
  { field: 'symbol', rowDrag: true },
  { field: 'last' },
  { field: 'change' },
];
```

**`rowDragEntireRow: true`:** Allows dragging by clicking anywhere on the row, not just the drag handle. Useful for grids where the entire row represents a draggable entity.

**Conditional drag:** `rowDrag` can be a function:

```typescript
{
  field: 'symbol',
  rowDrag: (params) => params.data.isEditable,  // Only some rows are draggable
}
```

### 5.2 Managed vs. Unmanaged Dragging

**Managed Row Dragging** (`rowDragManaged: true`):
- The grid handles reordering automatically.
- As the user drags, rows animate to show the new position.
- Simple and requires no custom code.
- Does NOT work when the grid is sorted, filtered, or grouped (because the visual order is controlled by the sort/filter/group state, not the user).

**Unmanaged Row Dragging** (`rowDragManaged: false`, the default):
- The grid fires events but does NOT move rows.
- The application listens to drag events and updates row data/order itself.
- More complex but fully flexible.
- Works regardless of sort/filter/group state.

### 5.3 Visual Feedback During Drag

**Row animation:** With managed dragging, rows slide up/down with CSS transitions to preview the drop position.

**Highlight mode:** `suppressMoveWhenRowDragging: true` changes the visual feedback. Instead of rows sliding around during the drag, the grid highlights the target drop position with a horizontal line. Rows only actually move when the drop occurs. This provides a calmer, more predictable experience:

```typescript
const gridOptions = {
  rowDragManaged: true,
  suppressMoveWhenRowDragging: true,  // Highlight instead of live reorder
};
```

**Drag ghost:** A semi-transparent representation of the dragged row follows the cursor.

### 5.4 Row Drag Events

| Event | When |
|---|---|
| `onRowDragEnter` | Drag starts (mouse down + initial move) |
| `onRowDragMove` | Fired continuously as the row is dragged |
| `onRowDragLeave` | Dragged row leaves the grid area |
| `onRowDragEnd` | Drop occurs (mouse up) |

Each event includes:
- `event.node` -- the row node being dragged
- `event.overNode` -- the row node being hovered over
- `event.overIndex` -- the index of the target position
- `event.y` -- vertical mouse position
- `event.vDirection` -- `'up'` or `'down'`

### 5.5 External Drop Zones

Rows can be dragged OUT of the grid to external DOM elements:

```typescript
// Register an external drop zone
api.addRowDropZone({
  getContainer: () => document.getElementById('drop-target'),
  onDragStop: (params) => {
    // params.node contains the dragged row data
    handleExternalDrop(params.node.data);
  },
});

// Remove it later
api.removeRowDropZone(dropZoneParams);
```

**Grid-to-Grid dragging:** Rows can be dragged between two AG Grid instances. A line indicator shows the insertion point in the target grid.

**DnD source column:** Setting `dndSource: true` on a column enables native HTML5 drag-and-drop FROM the grid to arbitrary external targets.

---

## 6. Custom Cell Renderers

### 6.1 The Rendering Pipeline

Cell renderers replace the default text content of a cell with arbitrary HTML or framework components. They sit at the end of the rendering pipeline:

```
Row Data -> valueGetter -> valueFormatter -> cellRenderer
                (compute)      (format text)     (render DOM)
```

### 6.2 Plain JavaScript Cell Renderer (ICellRendererComp)

```typescript
class MedalRenderer implements ICellRendererComp {
  eGui!: HTMLElement;

  init(params: ICellRendererParams) {
    this.eGui = document.createElement('span');
    this.eGui.innerHTML = `<img src="/medal.png" /> ${params.value}`;
  }

  getGui(): HTMLElement {
    return this.eGui;
  }

  refresh(params: ICellRendererParams): boolean {
    // Return true if you handled the update, false to destroy and recreate
    this.eGui.innerHTML = `<img src="/medal.png" /> ${params.value}`;
    return true;
  }

  destroy(): void {
    // Cleanup event listeners, timers, etc.
  }
}
```

**Lifecycle:**
1. `init(params)` -- Called once when the cell is first created. Build the DOM.
2. `getGui()` -- Returns the root DOM element.
3. `refresh(params)` -- Called when the cell value changes. Return `true` if you handled the update in place (efficient). Return `false` to force destroy + recreate (expensive).
4. `destroy()` -- Called when the cell is removed from DOM (row scrolled out of view, or row removed).

The `refresh` method is critical for performance in real-time scenarios (e.g., streaming price updates). An efficient refresh updates only the changed text/style without rebuilding the DOM.

### 6.3 React Cell Renderer

In React, cell renderers are standard functional components:

```tsx
const ChangeRenderer = (props: ICellRendererParams) => {
  const value = props.value;
  const color = value >= 0 ? 'green' : 'red';
  const arrow = value >= 0 ? '\u25B2' : '\u25BC';
  return (
    <span style={{ color }}>
      {arrow} {Math.abs(value).toFixed(2)}%
    </span>
  );
};

// Column definition
{ field: 'change', cellRenderer: ChangeRenderer }
```

React functional components are re-rendered by React when props change, so there is no need to implement `refresh` -- the framework handles it.

### 6.4 cellRendererParams

Pass additional props to cell renderers:

```typescript
{
  field: 'action',
  cellRenderer: ButtonRenderer,
  cellRendererParams: {
    label: 'Trade',
    onClick: (data) => openOrderDialog(data),
  },
}
```

### 6.5 cellRendererSelector

Dynamically choose different renderers based on row data:

```typescript
{
  field: 'type',
  cellRendererSelector: (params) => {
    if (params.data.type === 'equity') return { component: EquityRenderer };
    if (params.data.type === 'option') return { component: OptionRenderer };
    return undefined;  // Use default text rendering
  },
}
```

### 6.6 Built-in Sparklines (Enterprise)

AG Grid includes a built-in sparkline cell renderer for inline mini-charts:

```typescript
{
  headerName: 'Trend',
  field: 'priceHistory',
  cellRenderer: 'agSparklineCellRenderer',
  cellRendererParams: {
    sparklineOptions: {
      type: 'line',        // 'line' | 'column' | 'area' | 'bar'
      line: {
        stroke: '#2196F3',
        strokeWidth: 2,
      },
      highlightStyle: {
        size: 4,
        fill: '#2196F3',
      },
      xKey: 'date',
      yKey: 'price',
      axis: {
        type: 'time',      // 'category' | 'number' | 'time'
      },
    },
  },
}
```

**Sparkline types:**
- **Line** -- Best for price trends
- **Area** -- Line with filled area below
- **Column** -- Vertical bars, good for volume
- **Bar** -- Horizontal bars

**Points of interest:** Sparklines support highlighting special data points (min, max, first, last, negative values) with custom styles. For example, negative value columns can be colored red.

**Data formats:** Sparklines accept arrays of numbers, arrays of tuples `[x, y]`, or arrays of objects `{ x, y }`.

### 6.7 Practical Examples for a Trading Watchlist

| Cell Type | Implementation |
|---|---|
| **Change indicator** (green/red arrow + value) | Custom `cellRenderer` checking sign of value |
| **Bid/Ask with spread** | `valueGetter` computes spread, `cellRenderer` shows two prices + spread |
| **Mini price chart** | `agSparklineCellRenderer` with `type: 'line'` |
| **Volume bar** | `agSparklineCellRenderer` with `type: 'column'`, or custom renderer with inline `<div>` width based on relative volume |
| **Trade button** | Custom renderer with `<button>` and click handler from `cellRendererParams` |
| **Status dot** (connected/delayed/stale) | Custom renderer returning a colored circle SVG/div |

---

## 7. Virtual Scrolling

### 7.1 Row Virtualization

AG Grid renders only the rows visible in the viewport, plus a configurable buffer. This is the key mechanism enabling performance with 100K+ rows.

**How it works:**
1. The grid calculates which rows are visible based on scroll position and row height.
2. Only those rows (plus the buffer) have DOM elements created.
3. As the user scrolls, rows exiting the viewport are destroyed and new rows entering are created.
4. The grid maintains a "spacer" div whose height equals the total height of all rows, providing a correctly-sized scrollbar.

**`rowBuffer`** (default: 10): Number of rows rendered outside the visible area on each side.

```typescript
// If 50 rows are visible, 70 total are rendered (10 above + 50 visible + 10 below)
const gridOptions = {
  rowBuffer: 10,
};
```

- **Low buffer** (0-5): Faster initial render, but may show blank rows during fast scrolling on slow machines.
- **High buffer** (20+): Smoother scrolling experience, but more DOM elements to manage.

The buffer is actually calculated as a **pixel range**: `rowBuffer * defaultRowHeight`. For a buffer of 10 and default row height of 42px, the grid extends 420px beyond the viewport in both directions. With variable row heights, this means a different number of actual rows may fall within the buffer zone.

### 7.2 Column Virtualization

Columns are also virtualized. Only columns whose horizontal position falls within the viewport (plus buffer) are rendered. This matters for grids with many columns (e.g., 100+ fields per instrument).

Column virtualization can be disabled with `suppressColumnVirtualisation: true` if needed (e.g., for print layouts).

### 7.3 Disabling Row Virtualization

`suppressRowVirtualisation: true` renders ALL rows. The `rowBuffer` property is ignored. Only use this for small datasets or special cases (e.g., PDF export).

### 7.4 Massive Row Count -- The Stretching Technique

Browsers impose a maximum height on DOM `div` elements. At the time of writing, Chrome v118 allows a maximum of ~32,000,000 pixels. With a row height of 100px, this limits a standard approach to ~320,000 rows.

AG Grid overcomes this with **Row Offset Stretching:**

1. The grid detects when the total row height exceeds the browser's div height limit.
2. It applies an **amplifier** to the vertical scroll position.
3. As the user scrolls, rows are repositioned with an offset that effectively compresses the scroll range.
4. The visual effect is that scrolling moves faster than normal, but all rows (e.g., 1,000,000) are reachable.

**Trade-off:** Rows scroll faster than natural when stretching is active. In practice, this is rarely noticeable because datasets large enough to trigger stretching are navigated via search/filter, not manual scrolling.

### 7.5 Performance Tips

| Technique | Effect |
|---|---|
| Use `getRowId` | Allows the grid to reuse DOM nodes when data updates (avoids full re-render) |
| Return `true` from `refresh()` in cell renderers | In-place update instead of destroy + create |
| Set `animateRows: false` | Eliminates row animation overhead |
| Use `valueFormatter` instead of `cellRenderer` when possible | Simpler DOM, faster rendering |
| Avoid `suppressRowVirtualisation` | Never disable virtualization for large datasets |
| Use `rowBuffer: 0` for initial load performance | Trade scrolling smoothness for faster first paint |

---

## 8. Selection

### 8.1 Row Selection

Configured via the `rowSelection` grid option:

```typescript
// Single row selection
rowSelection: { mode: 'singleRow' }

// Multi-row selection
rowSelection: { mode: 'multiRow' }
```

**Click behavior:**
- Single mode: Clicking a row selects it and deselects others.
- Multi mode: Click selects, Ctrl+Click adds to selection, Shift+Click selects a range.

**`enableClickSelection: true`:** Enables selection by clicking anywhere on the row (not just checkboxes).

### 8.2 Checkbox Selection

```typescript
rowSelection: {
  mode: 'multiRow',
  checkboxes: true,       // Show checkboxes in each row
  headerCheckbox: true,   // Show select-all checkbox in header
}
```

**Conditional checkboxes:** Pass a function to dynamically enable/disable:

```typescript
rowSelection: {
  mode: 'multiRow',
  checkboxes: (params) => params.data.isSelectable,
}
```

**Shift+Click on checkboxes** selects a range of adjacent rows.

### 8.3 Row Selectability

```typescript
rowSelection: {
  mode: 'multiRow',
  isRowSelectable: (node) => node.data.status !== 'disabled',
}
```

### 8.4 Cell Selection / Range Selection (Enterprise)

Cell selection allows Excel-like rectangular range selection:

```typescript
cellSelection: true
// or with configuration:
cellSelection: {
  handle: { mode: 'range' },  // Show resize handle on selection
}
```

**Selection methods:**
- **Mouse drag** -- Click and drag across cells
- **Shift+Arrow keys** -- Extend selection from focused cell
- **Ctrl+Mouse drag** -- Add a new range without clearing existing ranges

**Fill Handle** (Excel-like auto-fill):

```typescript
cellSelection: {
  handle: { mode: 'fill' },
}
```

Dragging the fill handle copies or extends values into new cells, similar to Excel's fill-down behavior.

**Range Handle:** Allows resizing the selected range by dragging a handle in the bottom-right corner.

### 8.5 Selection API

```typescript
// Get selected rows
const selectedRows = api.getSelectedRows();        // Returns data objects
const selectedNodes = api.getSelectedNodes();       // Returns row nodes

// Programmatic selection
api.setNodesSelected({ nodes: [node1, node2], newValue: true });

// Select all / deselect all
api.selectAll();
api.deselectAll();

// Get cell ranges (Enterprise)
const ranges = api.getCellRanges();
```

---

## 9. Resizing UX

### 9.1 Resize Handle Appearance

The resize handle is a thin (typically 4-8px) invisible hit zone at the right edge of each column header. The cursor changes to `col-resize` (double-headed horizontal arrow) when hovering over the handle.

### 9.2 Resize Behavior

- **Dragging right** expands the column; **dragging left** shrinks it.
- Other columns adjust based on their sizing strategy (flex columns redistribute, fixed columns remain unchanged).
- `minWidth` and `maxWidth` constrain resize range.
- If a flex column is manually resized, flex is disabled and it becomes fixed-width.

### 9.3 Double-Click Auto-Size

Double-clicking the resize handle auto-sizes the column to fit its widest content. By default, this includes header text. Set `skipHeader: true` to only consider cell content.

### 9.4 Resize Events

```typescript
onColumnResized: (event: ColumnResizedEvent) => {
  // event.column -- the column being resized
  // event.finished -- true when drag ends (false during drag)
  // event.source -- 'uiColumnDragged', 'api', 'autosizeColumns', etc.
  if (event.finished) {
    saveColumnWidths(event.columns);
  }
}
```

The `finished` flag is important -- it is `false` during drag (fired on every pixel of movement) and `true` only when the user releases the mouse. Use `finished: true` for persistence to avoid excessive writes.

---

## 10. Column Reorder UX

### 10.1 How Header Dragging Works

1. **Mouse down** on a column header begins tracking.
2. After a small movement threshold, the grid enters drag mode.
3. A **drag ghost** (customizable via `dragAndDropImageComponent`) appears attached to the cursor.
4. The grid internally tracks the horizontal position and calculates the target drop index.
5. **Other columns animate** (slide left/right) to preview the new layout as the cursor moves.
6. On **mouse up**, the column is placed at the calculated index.

### 10.2 Internal Drag Implementation

AG Grid does NOT use the browser's native HTML5 Drag and Drop API for column reordering. It implements its own drag system for finer control over:
- Drag threshold before activation
- Visual feedback during drag
- Drop zone calculation
- Animation of neighboring columns

### 10.3 Drop Indicators

During a column drag, the grid provides visual feedback through:
- **Column animation** -- adjacent columns slide to open a gap where the drop will occur.
- **Drag ghost** -- semi-transparent image following the cursor showing the dragged column header.
- `suppressColumnMoveAnimation: true` disables the sliding animation (useful on low-performance platforms).

### 10.4 Column Move Events

```typescript
onColumnMoved: (event: ColumnMovedEvent) => {
  // event.column -- the moved column
  // event.toIndex -- new position index
  // event.source -- 'uiColumnMoved', 'api', etc.
  // event.finished -- true when the move is complete
  saveColumnOrder(api.getColumnState());
}
```

### 10.5 Preventing Moves

| Property | Effect |
|---|---|
| `suppressMovable: true` | This column cannot be dragged |
| `lockPosition: 'left'` | Column is locked to left edge, cannot be moved by any means |
| `lockPosition: 'right'` | Column is locked to right edge |

Locked columns also cannot be displaced by dragging other columns past them.

---

## 11. API Design Patterns

### 11.1 Declarative Configuration (Grid Options)

The grid is configured through a single `GridOptions` object containing:
- `columnDefs` -- column definitions array
- `rowData` -- the data array (client-side model)
- `defaultColDef` -- shared column defaults
- `columnTypes` -- named column type bundles
- `rowSelection` -- selection configuration
- `cellSelection` -- range selection configuration
- Event callbacks (`onSortChanged`, `onColumnResized`, etc.)
- Behavioral flags (`animateRows`, `suppressColumnMoveAnimation`, etc.)

```typescript
const gridOptions: GridOptions = {
  columnDefs: [...],
  rowData: [...],
  defaultColDef: { sortable: true, resizable: true, flex: 1 },
  rowSelection: { mode: 'singleRow' },
  animateRows: true,
  onGridReady: (params) => { /* store params.api */ },
  onSelectionChanged: (event) => { /* handle selection */ },
};
```

### 11.2 Imperative API (Grid API)

After initialization, the `api` object provides programmatic control. It is obtained via:
- The `onGridReady` event: `event.api`
- React ref: directly from `AgGridReact` ref
- Angular `@ViewChild`

**Key API methods:**

```typescript
// Data
api.setGridOption('rowData', newData);    // Replace all row data
api.applyTransaction({ add, remove, update }); // Incremental update
api.getDisplayedRowAtIndex(0);            // Get specific row

// Columns
api.sizeColumnsToFit();
api.autoSizeAllColumns();
api.setColumnPinned('symbol', 'left');
api.moveColumn('volume', 3);
api.applyColumnState({ state: [...] });
api.getColumnState();                     // For persistence

// Selection
api.getSelectedRows();
api.selectAll();
api.deselectAll();

// Sorting & Filtering
api.applyColumnState({ state: [{ colId: 'price', sort: 'desc' }] });
api.setFilterModel({ symbol: { type: 'contains', filter: 'AAPL' } });

// Scrolling
api.ensureIndexVisible(42);               // Scroll to row index
api.ensureColumnVisible('volume');        // Scroll to column

// Refresh
api.refreshCells({ rowNodes: [node], columns: ['last', 'change'] });
api.redrawRows({ rowNodes: [node] });
```

### 11.3 Transaction API for Real-Time Updates

The `applyTransaction` method is critical for real-time data grids (trading watchlists, dashboards):

```typescript
// Add rows
api.applyTransaction({ add: [{ symbol: 'AAPL', last: 150.25 }] });

// Update existing rows (requires getRowId to identify rows)
api.applyTransaction({ update: [{ symbol: 'AAPL', last: 151.00 }] });

// Remove rows
api.applyTransaction({ remove: [{ symbol: 'AAPL' }] });
```

**`getRowId`** tells the grid how to identify rows for efficient updates:

```typescript
getRowId: (params) => params.data.symbol,  // Use 'symbol' as unique key
```

With `getRowId`, the grid can update individual cells in-place rather than re-rendering entire rows. This is essential for streaming price data.

**`applyTransactionAsync`** batches multiple updates into a single render cycle, crucial for high-frequency data feeds:

```typescript
// These are batched and applied together on the next animation frame
api.applyTransactionAsync({ update: [{ symbol: 'AAPL', last: 151.00 }] });
api.applyTransactionAsync({ update: [{ symbol: 'GOOG', last: 2800.50 }] });
api.applyTransactionAsync({ update: [{ symbol: 'MSFT', last: 310.75 }] });
```

### 11.4 Event Model

Events follow the pattern `onEventName` in grid options. All events provide a params object with context.

**Key events for a trading watchlist:**

| Event | Fires When |
|---|---|
| `onGridReady` | Grid is initialized and API is available |
| `onRowClicked` | User clicks a row |
| `onRowDoubleClicked` | User double-clicks a row |
| `onSelectionChanged` | Row selection changes |
| `onCellClicked` | User clicks a specific cell |
| `onCellValueChanged` | Cell value is edited |
| `onSortChanged` | Sort state changes |
| `onColumnResized` | Column width changes (has `finished` flag) |
| `onColumnMoved` | Column order changes (has `finished` flag) |
| `onColumnPinned` | Column pinning changes |
| `onColumnVisible` | Column visibility changes |
| `onRowDragMove` | During row drag |
| `onRowDragEnd` | Row drop completes |
| `onFirstDataRendered` | First batch of data is rendered |
| `onModelUpdated` | Row model is updated (after sort, filter, etc.) |

**Two ways to listen:**

```typescript
// 1. Via gridOptions (one handler per event)
gridOptions.onSelectionChanged = (event) => { ... };

// 2. Via addEventListener (multiple handlers per event)
api.addEventListener('selectionChanged', handler);
api.removeEventListener('selectionChanged', handler);
```

### 11.5 Column State for Persistence

The Column State API enables saving and restoring the user's column configuration:

```typescript
// Save
const state = api.getColumnState();
localStorage.setItem('watchlist-columns', JSON.stringify(state));

// Restore
const saved = JSON.parse(localStorage.getItem('watchlist-columns'));
api.applyColumnState({ state: saved, applyOrder: true });
```

Column state captures: width, flex, sort, sortIndex, pinned, hide, aggFunc, rowGroup, pivot, and column order.

---

## 12. Design Lessons for a Native Trading Watchlist

The following patterns from AG Grid translate directly to a native Rust/GPU-rendered watchlist:

1. **Column definitions as data** -- Define columns as a serializable schema (equivalent to `ColDef[]`). This decouples layout from rendering.

2. **Three-stage rendering pipeline** -- Separate value extraction (getter), text formatting (formatter), and visual rendering (renderer). This keeps each stage testable and composable.

3. **Virtual scrolling with row buffer** -- Only render visible rows plus a buffer. Track scroll offset and compute visible range from `scroll_y / row_height`.

4. **Row identity via key** -- Assign each row a stable ID (`getRowId` equivalent) so that updates can target specific cells without full re-render.

5. **Transaction-based updates** -- Batch add/remove/update operations and apply them in a single render pass. Essential for streaming market data.

6. **Column state as separate concern** -- Store column widths, order, pinned state, and sort state independently from column definitions. This enables user customization persistence.

7. **Flex layout for columns** -- Implement both fixed-width and flex-proportional column sizing. When a user manually resizes a flex column, convert it to fixed.

8. **Resize via `finished` flag** -- Distinguish between in-progress drag (for live visual feedback) and completed resize (for persistence/state save).

9. **Sort indicators with priority numbers** -- Show sort direction arrows and priority badges for multi-column sort.

10. **Custom cell renderers as composable units** -- Define a trait/interface for cell rendering with `init`, `render`, `update`, and `destroy` lifecycle methods. The `update` path should be cheaper than `init + render`.

---

## Sources

- [AG Grid Official Site](https://www.ag-grid.com/)
- [AG Grid Key Features](https://www.ag-grid.com/javascript-data-grid/key-features/)
- [Column Definitions](https://www.ag-grid.com/javascript-data-grid/column-definitions/)
- [Column Sizing](https://www.ag-grid.com/javascript-data-grid/column-sizing/)
- [Column Moving](https://www.ag-grid.com/javascript-data-grid/column-moving/)
- [Column Pinning](https://www.ag-grid.com/javascript-data-grid/column-pinning/)
- [Column Groups](https://www.ag-grid.com/javascript-data-grid/column-groups/)
- [Row Sorting](https://www.ag-grid.com/javascript-data-grid/row-sorting/)
- [Row Dragging](https://www.ag-grid.com/javascript-data-grid/row-dragging/)
- [Row Dragging to External DropZone](https://www.ag-grid.com/javascript-data-grid/row-dragging-to-external-dropzone/)
- [Cell Components (Cell Renderer)](https://www.ag-grid.com/javascript-data-grid/component-cell-renderer/)
- [Sparklines Overview](https://www.ag-grid.com/javascript-data-grid/sparklines-overview/)
- [DOM Virtualisation](https://www.ag-grid.com/javascript-data-grid/dom-virtualisation/)
- [Massive Row Count](https://www.ag-grid.com/javascript-data-grid/massive-row-count/)
- [Scrolling Performance](https://www.ag-grid.com/javascript-data-grid/scrolling-performance/)
- [Row Selection](https://www.ag-grid.com/javascript-data-grid/row-selection/)
- [Cell Selection (Range Selection)](https://www.ag-grid.com/javascript-data-grid/cell-selection/)
- [Grid Overview / API Interface](https://www.ag-grid.com/javascript-data-grid/grid-interface/)
- [Grid Events Reference](https://www.ag-grid.com/javascript-data-grid/grid-events/)
- [Row Models](https://www.ag-grid.com/javascript-data-grid/row-models/)
- [Value Getters](https://www.ag-grid.com/javascript-data-grid/value-getters/)
- [Column Properties Reference](https://www.ag-grid.com/javascript-data-grid/column-properties/)
- [About AG Grid](https://www.ag-grid.com/about/)
- [AG Grid Enterprise Features](https://www.ag-grid.com/landing-pages/enterprise-data-grid/)
- [AG Grid Blog: Sparklines](https://blog.ag-grid.com/introducing-ag-grid-sparklines/)
- [AG Grid Blog: Cell Customization](https://blog.ag-grid.com/heres-how-cell-customization-in-ag-grid-wins-over-competition/)
