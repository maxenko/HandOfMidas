# React Grid Libraries -- Research for Rust Native Grid Design

> Research date: 2026-04-01
> Focus: Architectural patterns that transfer to a headless Rust grid engine

---

## Table of Contents

1. [TanStack Table Overview](#1-tanstack-table-overview)
2. [Core Architecture](#2-core-architecture)
3. [Column Features](#3-column-features)
4. [Sorting](#4-sorting)
5. [Row Drag-and-Drop](#5-row-drag-and-drop)
6. [Custom Cell Rendering](#6-custom-cell-rendering)
7. [Column Resizing](#7-column-resizing)
8. [Column Reordering](#8-column-reordering)
9. [Virtualization](#9-virtualization)
10. [MUI DataGrid](#10-mui-datagrid)
11. [React Data Grid (Adazzle)](#11-react-data-grid-adazzle)
12. [Key Design Insight -- The Headless Pattern](#12-key-design-insight----the-headless-pattern)

---

## 1. TanStack Table Overview

**Repository**: [TanStack/table](https://github.com/TanStack/table)
**Package**: `@tanstack/table-core` (framework-agnostic) + adapters (`@tanstack/react-table`, `@tanstack/vue-table`, `@tanstack/solid-table`, `@tanstack/svelte-table`, `@tanstack/qwik-table`)

### The Headless UI Philosophy

TanStack Table (formerly React Table) is a **headless UI library**. It provides zero DOM elements, zero CSS, and zero opinions about rendering. Instead it gives you:

- **State management** -- sorting state, filter state, pagination state, column sizing state, etc.
- **Data processing** -- row models that transform raw data through a pipeline of operations.
- **Event handlers** -- resize handlers, sort toggle handlers, selection handlers.
- **Computed values** -- visible columns, sorted rows, paginated rows, pinned column groups.

You, the developer, bring your own `<table>`, `<div>`, canvas, or any rendering target. The library returns objects and functions; you decide what to do with them.

### Why Developers Choose It

1. **Total rendering control** -- No fighting a component library's CSS. Any design system works.
2. **Framework-agnostic core** -- The same mental model (and most of the same API) works in React, Vue, Solid, Svelte, and Qwik. Skills transfer across frameworks.
3. **Tree-shakeable** -- Only import the row models you use. Skip filtering? That code never enters the bundle. The core is ~12-16 KB.
4. **TypeScript-first** -- Column helpers, generic row types, and inferred cell value types provide deep type safety.
5. **Modular feature system** -- Each feature (sorting, filtering, pinning, etc.) is a plugin that extends the core. You compose only what you need.
6. **Proven at scale** -- Used in thousands of production apps, from simple lists to financial dashboards with millions of rows (via virtualization).

### What It Is NOT

TanStack Table is not a drop-in `<DataGrid />` component. There is no "render a grid in one line" API. You must write your own table markup and wire up event handlers. This is the trade-off for total flexibility.

---

## 2. Core Architecture

### 2.1 The Table Instance

Everything begins with creating a **table instance**. In React, this is the `useReactTable` hook. In the framework-agnostic core, it is the `createTable` function.

```typescript
const table = useReactTable({
  data,                          // TData[] -- your row data
  columns,                       // ColumnDef<TData>[] -- column definitions
  getCoreRowModel: getCoreRowModel(),
  getSortedRowModel: getSortedRowModel(),
  getFilteredRowModel: getFilteredRowModel(),
  getPaginationRowModel: getPaginationRowModel(),
  state: { sorting, columnVisibility },       // controlled state slices
  onSortingChange: setSorting,                // state updater callbacks
  onColumnVisibilityChange: setColumnVisibility,
})
```

The table instance is the single object you interact with. It exposes:

- `table.getHeaderGroups()` -- header rows (supports multi-row grouped headers)
- `table.getRowModel()` -- the final, processed rows after all transforms
- `table.getState()` -- the current table state snapshot
- `table.setColumnOrder()`, `table.setSorting()`, etc. -- imperative state setters

### 2.2 State Management: Controlled vs. Uncontrolled

TanStack Table supports a hybrid state model, identical in concept to React's controlled/uncontrolled input pattern:

| Mode | How It Works |
|------|-------------|
| **Uncontrolled** | Pass nothing. The table manages state internally. Read it with `table.getState()`. |
| **Controlled** | Pass a `state` slice and the corresponding `on[State]Change` callback. You own the state. |
| **Mixed** | Some state controlled (e.g., sorting), other state uncontrolled (e.g., column sizing). |
| **Full control** | Use `onStateChange` to receive every state mutation. You are responsible for the entire state object. |

The `on[State]Change` callbacks receive an **updater** -- either a new value or a function `(prev) => next`, exactly like React's `setState`. This pattern is crucial for the Rust port: the grid engine emits state-change intents, and the host decides what to do with them.

### 2.3 Column Definitions

Column definitions are the schema of the table. Three types exist:

| Type | Purpose | Can Sort/Filter? |
|------|---------|-----------------|
| **Accessor Column** | Maps to a data field via `accessorKey` (string path) or `accessorFn` (function). | Yes -- has a data model. |
| **Display Column** | Arbitrary content (buttons, checkboxes, expanders). No data model. | No. |
| **Group Column** | Groups other columns under a shared header. Contains `columns` array. | N/A. |

```typescript
const columnHelper = createColumnHelper<Person>()

// Accessor column -- data-backed
columnHelper.accessor('firstName', {
  header: 'First Name',
  cell: info => info.getValue(),
  sortingFn: 'alphanumeric',
})

// Accessor column with derived value
columnHelper.accessor(row => `${row.firstName} ${row.lastName}`, {
  id: 'fullName',
  header: 'Full Name',
})

// Display column -- no data model
columnHelper.display({
  id: 'actions',
  header: 'Actions',
  cell: props => <ActionMenu row={props.row} />,
})

// Group column
columnHelper.group({
  id: 'name',
  header: 'Name',
  columns: [firstNameCol, lastNameCol],
})
```

**Critical rule**: Accessor functions must return **primitive values** (string, number, boolean, Date). Never return JSX from an accessor. Rendering is the job of the `cell` function.

### 2.4 The Row Model Pipeline

Row models are the heart of TanStack Table's data processing. They form a **pipeline** where each stage transforms the output of the previous one:

```
Raw Data (TData[])
  |
  v
getCoreRowModel()          -- 1:1 mapping of data to Row objects
  |
  v
getFilteredRowModel()      -- removes rows that don't match filters
  |
  v
getGroupedRowModel()       -- groups rows by column values, creates sub-rows
  |
  v
getSortedRowModel()        -- sorts rows (respects groups)
  |
  v
getExpandedRowModel()      -- flattens expanded group rows for display
  |
  v
getPaginationRowModel()    -- slices to current page
  |
  v
table.getRowModel()        -- FINAL output: the rows to render
```

Each row model is a **factory function** you pass as an option. If you don't pass `getFilteredRowModel`, filtering code is tree-shaken out of the bundle entirely. The pipeline output (`table.getRowModel().rows`) is what you iterate over to render `<tr>` elements.

Each `Row` object carries:
- `row.id` -- stable identifier
- `row.original` -- the original `TData` object
- `row.getValue(columnId)` -- the accessor value for a given column
- `row.getVisibleCells()` -- cells to render, respecting column visibility and ordering
- `row.subRows` -- child rows (for grouping/tree data)
- `row.getIsSelected()`, `row.toggleSelected()` -- selection state

### 2.5 The Feature Plugin System

Internally, TanStack Table uses a `TableFeature` interface. Each feature (Sorting, Filtering, ColumnSizing, Pinning, etc.) implements hooks that extend:

- `createTable()` -- adds methods to the table instance (e.g., `table.setSorting()`)
- `createColumn()` -- adds methods to each column (e.g., `column.getIsSorted()`)
- `createRow()` -- adds methods to each row (e.g., `row.getIsSelected()`)
- `createCell()` -- adds methods to each cell
- `createHeader()` -- adds methods to each header (e.g., `header.getResizeHandler()`)

This plugin architecture means features are composable and self-contained. A Rust port could mirror this with a trait-based feature system.

---

## 3. Column Features

### 3.1 Column Sizing

Every column has three size properties:

| Property | Default | Purpose |
|----------|---------|---------|
| `size` | 150 | The target width in pixels. |
| `minSize` | 20 | Floor for resize operations. |
| `maxSize` | Number.MAX_SAFE_INTEGER | Ceiling for resize operations. |

These can be set per-column in the column definition or globally via `defaultColumn`:

```typescript
const table = useReactTable({
  defaultColumn: { size: 200, minSize: 50, maxSize: 500 },
  columns: [
    { accessorKey: 'name', size: 300 },  // override default
    { accessorKey: 'age', size: 80 },
  ],
})
```

The table tracks sizing state in `columnSizing` (a `Record<string, number>` of column ID to width) and `columnSizingInfo` (metadata about an active resize operation -- which column, start position, delta, etc.).

**Reading computed sizes**: Use `header.getSize()`, `column.getSize()`, or `cell.column.getSize()` to get the final pixel width accounting for all constraints.

### 3.2 Column Ordering

Column order is tracked as an array of column IDs:

```typescript
const [columnOrder, setColumnOrder] = useState<string[]>([])

const table = useReactTable({
  state: { columnOrder },
  onColumnOrderChange: setColumnOrder,
})
```

When `columnOrder` is empty (default), columns render in the order they appear in the `columns` definition. When populated, it controls the display order. This state is what DnD integrations mutate (see Section 8).

### 3.3 Column Pinning

Column pinning splits the table into three regions: **left**, **center** (unpinned), and **right**.

```typescript
// State shape
type ColumnPinningState = {
  left?: string[]   // column IDs pinned left
  right?: string[]  // column IDs pinned right
}
```

Key APIs for rendering pinned layouts:

| Method | Returns |
|--------|---------|
| `table.getLeftHeaderGroups()` | Header groups for left-pinned columns |
| `table.getCenterHeaderGroups()` | Header groups for unpinned columns |
| `table.getRightHeaderGroups()` | Header groups for right-pinned columns |
| `row.getLeftVisibleCells()` | Cells for left-pinned columns |
| `row.getCenterVisibleCells()` | Cells for unpinned columns |
| `row.getRightVisibleCells()` | Cells for right-pinned columns |
| `table.getLeftVisibleLeafColumns()` | Flat array of visible left-pinned leaf columns |
| `table.getRightVisibleLeafColumns()` | Flat array of visible right-pinned leaf columns |

A common rendering pattern is three separate `<table>` elements (or three `<div>` columns) for left/center/right, with the center section scrolling horizontally while the pinned sections stay fixed. The headless design lets you implement this however you want -- CSS `position: sticky`, absolute positioning, or separate scroll containers.

### 3.4 Column Visibility

Column visibility is a map of column ID to boolean:

```typescript
type ColumnVisibilityState = Record<string, boolean>
// { firstName: true, age: false }  -- age is hidden
```

Key APIs:

| API | Purpose |
|-----|---------|
| `column.getIsVisible()` | Is this column currently visible? |
| `column.getCanHide()` | Can this column be hidden? (respects `enableHiding`) |
| `column.toggleVisibility(value?)` | Toggle or set visibility |
| `column.getToggleVisibilityHandler()` | Returns an `onChange` handler for checkbox binding |
| `table.toggleAllColumnsVisible(value?)` | Show/hide all columns at once |
| `table.getIsAllColumnsVisible()` | Are all columns visible? |
| `table.getIsSomeColumnsVisible()` | Are some (but not all) columns visible? |

A column with `enableHiding: false` in its definition will always be visible and cannot be toggled.

---

## 4. Sorting

### 4.1 Setup

Sorting requires two things:

1. Pass `getSortedRowModel: getSortedRowModel()` to enable client-side sorting.
2. The table automatically manages `sorting` state, or you control it.

```typescript
type SortingState = ColumnSort[]
type ColumnSort = { id: string; desc: boolean }
// Example: [{ id: 'age', desc: true }, { id: 'name', desc: false }]
```

The array represents sort precedence: index 0 is the primary sort, index 1 is the secondary sort (tiebreaker), and so on.

### 4.2 Sort Toggle Behavior

Clicking a column header cycles through states:

```
unsorted -> asc -> desc -> unsorted (if enableSortingRemoval is true)
                        -> asc      (if enableSortingRemoval is false)
```

Hold **Shift** (by default) to add a column to the multi-sort stack instead of replacing the current sort. This behavior is controlled by `isMultiSortEvent`:

```typescript
isMultiSortEvent: (e: unknown) => boolean  // default: checks e.shiftKey
```

You can also limit multi-sort depth:

```typescript
maxMultiSortColCount: 3  // at most 3 columns sorted simultaneously
```

### 4.3 Built-in Sort Functions

| Function | Behavior |
|----------|----------|
| `alphanumeric` | Mixed string/number sort. "A1" < "A2" < "A10" < "B1". |
| `alphanumericCaseSensitive` | Same, but case-sensitive. |
| `text` | Locale-aware string sort. |
| `textCaseSensitive` | Same, but case-sensitive. |
| `datetime` | Compares Date objects. |
| `basic` | Uses `<` / `>` / `===` operators directly. Fastest. |

### 4.4 Custom Sort Functions

Define custom sort functions at the table level and reference them by key:

```typescript
const table = useReactTable({
  sortingFns: {
    currencySort: (rowA, rowB, columnId) => {
      const a = parseCurrency(rowA.getValue(columnId))
      const b = parseCurrency(rowB.getValue(columnId))
      return a - b
    },
  },
  columns: [
    { accessorKey: 'price', sortingFn: 'currencySort' },
  ],
})
```

Or define inline on a column:

```typescript
columnHelper.accessor('price', {
  sortingFn: (rowA, rowB, columnId) => {
    return rowA.original.priceNum - rowB.original.priceNum
  },
})
```

Sort functions receive `(rowA, rowB, columnId)` and must return `-1 | 0 | 1`. The library handles direction inversion for descending sorts automatically.

### 4.5 Sorting APIs on Column/Header

| API | Purpose |
|-----|---------|
| `column.getIsSorted()` | Returns `'asc'`, `'desc'`, or `false` |
| `column.getCanSort()` | Is sorting enabled for this column? |
| `column.toggleSorting(desc?, isMulti?)` | Imperatively toggle sort |
| `column.getToggleSortingHandler()` | Returns an onClick handler for headers |
| `column.getSortIndex()` | Position in the multi-sort stack (-1 if not sorted) |
| `column.getNextSortingOrder()` | What will the next click produce? |

---

## 5. Row Drag-and-Drop

TanStack Table does **not** include built-in drag-and-drop. It provides the data model; you bring the DnD library. The official examples use [dnd-kit](https://dndkit.com/), a modern, modular, lightweight DnD toolkit for React.

### 5.1 Architecture Pattern

```
DndContext                          -- dnd-kit top-level provider
  |
  SortableContext                   -- wraps the list of sortable items
    rows={table.getRowModel().rows} -- items are the table's row IDs
    strategy={verticalListSortingStrategy}
    |
    <table>
      <tbody>
        {rows.map(row => (
          <DraggableRow key={row.id} row={row} />
        ))}
      </tbody>
    </table>
```

### 5.2 The DraggableRow Component

Each row uses dnd-kit's `useSortable` hook:

```typescript
function DraggableRow({ row }: { row: Row<Person> }) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } =
    useSortable({ id: row.id })

  const style = {
    transform: CSS.Transform.toString(transform),
    transition,
    opacity: isDragging ? 0.5 : 1,
    zIndex: isDragging ? 1 : 0,
  }

  return (
    <tr ref={setNodeRef} style={style}>
      {row.getVisibleCells().map(cell => (
        <td key={cell.id}>
          {cell.column.id === 'dragHandle'
            ? <button {...attributes} {...listeners}>Drag</button>
            : flexRender(cell.column.columnDef.cell, cell.getContext())
          }
        </td>
      ))}
    </tr>
  )
}
```

### 5.3 Handling the Drop

On `DndContext`'s `onDragEnd`, reorder the **data array** (not the table state):

```typescript
function handleDragEnd(event: DragEndEvent) {
  const { active, over } = event
  if (active && over && active.id !== over.id) {
    setData(prev => {
      const oldIndex = prev.findIndex(r => r.id === active.id)
      const newIndex = prev.findIndex(r => r.id === over.id)
      return arrayMove(prev, oldIndex, newIndex)
    })
  }
}
```

The table re-renders with the reordered data. No special table state is needed -- the order is implicit in the data array.

### 5.4 Drag Handle Column

A common pattern is a dedicated **display column** for drag handles:

```typescript
columnHelper.display({
  id: 'dragHandle',
  header: 'Move',
  cell: ({ row }) => <RowDragHandleCell rowId={row.id} />,
  size: 60,
})
```

The `RowDragHandleCell` component uses `useSortable` to get `attributes` and `listeners`, which are spread onto a button or icon. This restricts dragging to the handle rather than the entire row.

### 5.5 Key Takeaway for Rust

Row DnD is entirely outside the table engine. The engine only needs to accept reordered data. The DnD interaction (hit testing, drag preview, drop zones) belongs to the UI layer. In a Rust native grid, this means the GPU renderer handles drag visuals while the grid state engine simply receives a "move row from index A to index B" command.

---

## 6. Custom Cell Rendering

### 6.1 The Cell Render Function

Every column definition can provide a `cell` property -- a function that receives a `CellContext` and returns renderable output:

```typescript
columnHelper.accessor('price', {
  cell: info => {
    const value = info.getValue()
    return new Intl.NumberFormat('en-US', {
      style: 'currency',
      currency: 'USD',
    }).format(value)
  },
})
```

The `CellContext` (`info` argument) provides:

| Property | Type | Purpose |
|----------|------|---------|
| `getValue()` | `() => TValue` | The accessor value for this cell |
| `row` | `Row<TData>` | The full row object |
| `column` | `Column<TData>` | The column definition and computed properties |
| `table` | `Table<TData>` | The entire table instance |
| `cell` | `Cell<TData>` | The cell object itself |
| `renderValue()` | `() => TValue` | Like getValue but falls back to aggregated value |

### 6.2 Any React Component as a Cell

Because cells are just functions, you can render anything:

```typescript
columnHelper.accessor('status', {
  cell: info => <StatusBadge status={info.getValue()} />,
})

columnHelper.accessor('sparkline', {
  cell: info => <SparklineChart data={info.getValue()} width={120} height={30} />,
})

columnHelper.display({
  id: 'select',
  header: ({ table }) => (
    <Checkbox
      checked={table.getIsAllRowsSelected()}
      onChange={table.getToggleAllRowsSelectedHandler()}
    />
  ),
  cell: ({ row }) => (
    <Checkbox
      checked={row.getIsSelected()}
      onChange={row.getToggleSelectedHandler()}
    />
  ),
})
```

### 6.3 The flexRender Utility

Framework adapters provide a `flexRender` function that handles both string values and component functions:

```typescript
// In your render loop:
{row.getVisibleCells().map(cell => (
  <td key={cell.id}>
    {flexRender(cell.column.columnDef.cell, cell.getContext())}
  </td>
))}
```

`flexRender` checks if the cell definition is a string (render as-is) or a function (call with context). This is the bridge between headless state and rendered output.

### 6.4 Header and Footer Rendering

Headers and footers use the same pattern:

```typescript
columnHelper.accessor('total', {
  header: () => <span className="font-bold">Total</span>,
  footer: info => {
    const sum = info.table.getFilteredRowModel().rows
      .reduce((acc, row) => acc + row.getValue<number>('total'), 0)
    return <strong>${sum.toFixed(2)}</strong>
  },
})
```

### 6.5 Key Takeaway for Rust

The cell render function pattern maps perfectly to a callback/closure system in Rust. The grid engine computes the cell context (value, row data, column metadata), and a user-supplied render callback converts it into GPU draw commands. The engine never knows or cares what the cell looks like.

---

## 7. Column Resizing

### 7.1 Enabling Resizing

Column resizing is enabled by setting `enableColumnResizing` (per-column or global). Then you wire up the resize handler in your header markup:

```typescript
<th style={{ width: header.getSize() }}>
  {flexRender(header.column.columnDef.header, header.getContext())}
  <div
    onMouseDown={header.getResizeHandler()}
    onTouchStart={header.getResizeHandler()}
    className={`resize-handle ${header.column.getIsResizing() ? 'active' : ''}`}
  />
</th>
```

`header.getResizeHandler()` returns an event handler that:
1. Captures the initial pointer position.
2. Tracks mouse/touch movement.
3. Updates `columnSizingInfo` state during the drag.
4. On release, commits the final size to `columnSizing` state.

### 7.2 Resize Modes

| Mode | Behavior |
|------|----------|
| `"onEnd"` (default) | Column width updates only when the user releases the resize handle. The `columnSizingInfo.deltaOffset` tracks the drag delta for preview rendering. |
| `"onChange"` | Column width updates on every pointer move event. Immediate visual feedback but more re-renders. |

```typescript
const table = useReactTable({
  columnResizeMode: 'onChange',
  // or
  columnResizeMode: 'onEnd',
})
```

### 7.3 Resize Direction

The `columnResizeDirection` option supports `"ltr"` (left-to-right) and `"rtl"` (right-to-left), affecting which side of the column the resize handle appears on and the drag direction.

### 7.4 Performant Resizing

For high-performance scenarios, TanStack Table recommends using CSS variables or inline styles driven by `table.getCenterTotalSize()` and `header.getSize()`, avoiding React re-renders during the drag:

```typescript
// Generate CSS variables from column sizes
const columnSizeVars = useMemo(() => {
  const headers = table.getFlatHeaders()
  const vars: Record<string, number> = {}
  for (const header of headers) {
    vars[`--header-${header.id}-size`] = header.getSize()
    vars[`--col-${header.column.id}-size`] = header.column.getSize()
  }
  return vars
}, [table.getState().columnSizingInfo, table.getState().columnSizing])
```

This way the DOM updates via CSS variables without triggering React's reconciliation on every pixel of drag movement.

### 7.5 Column Sizing Info State

During an active resize, `table.getState().columnSizingInfo` contains:

| Field | Type | Purpose |
|-------|------|---------|
| `startOffset` | `number | null` | Pointer position at drag start |
| `startSize` | `number | null` | Column width at drag start |
| `deltaOffset` | `number | null` | Current drag delta (for preview) |
| `deltaPercentage` | `number | null` | Delta as percentage of start size |
| `columnSizingStart` | `[string, number][]` | All columns and their start sizes |
| `isResizingColumn` | `string | false` | ID of column being resized, or false |

### 7.6 Key Takeaway for Rust

The resize interaction is a classic pointer-drag state machine: `idle -> dragging(startX, columnId) -> committed(newWidth)`. The grid engine manages the state; the renderer draws the handle and preview line. The `onEnd` vs `onChange` mode maps to "commit on mouse-up" vs "commit on every frame."

---

## 8. Column Reordering

### 8.1 State Management

Column order is tracked as an array of column IDs in `columnOrder` state:

```typescript
type ColumnOrderState = string[]
```

When empty, columns render in their definition order. When populated, this array dictates display order. The table provides:

- `table.setColumnOrder(updater)` -- set or update column order
- `table.resetColumnOrder()` -- reset to definition order

### 8.2 DnD Integration Pattern (dnd-kit)

The official TanStack Table column DnD example uses the same dnd-kit library as row DnD, but with a **horizontal** strategy:

```typescript
<DndContext
  sensors={useSensors(
    useSensor(PointerSensor),
    useSensor(KeyboardSensor)
  )}
  collisionDetection={closestCenter}
  modifiers={[restrictToHorizontalAxis]}
  onDragEnd={handleDragEnd}
>
  <SortableContext
    items={columnOrder}
    strategy={horizontalListSortingStrategy}
  >
    <thead>
      <tr>
        {headerGroup.headers.map(header => (
          <DraggableHeader key={header.id} header={header} />
        ))}
      </tr>
    </thead>
  </SortableContext>
</DndContext>
```

### 8.3 The DraggableHeader Component

```typescript
function DraggableHeader({ header }) {
  const { attributes, listeners, setNodeRef, transform, isDragging } =
    useSortable({ id: header.column.id })

  const style = {
    transform: CSS.Translate.toString(transform),
    opacity: isDragging ? 0.5 : 1,
    cursor: 'grab',
  }

  return (
    <th ref={setNodeRef} style={style} {...attributes} {...listeners}>
      {flexRender(header.column.columnDef.header, header.getContext())}
    </th>
  )
}
```

### 8.4 Handling the Reorder

```typescript
function handleDragEnd(event: DragEndEvent) {
  const { active, over } = event
  if (active && over && active.id !== over.id) {
    setColumnOrder(prev => {
      const oldIndex = prev.indexOf(active.id as string)
      const newIndex = prev.indexOf(over.id as string)
      return arrayMove(prev, oldIndex, newIndex)
    })
  }
}
```

### 8.5 Key Takeaway for Rust

Column reordering is a mutation of an index array. The grid engine stores `column_order: Vec<ColumnId>` and exposes a `move_column(from_index, to_index)` method. The renderer draws the drag preview and drop indicator; the engine just receives the final reorder command.

---

## 9. Virtualization

### 9.1 TanStack Virtual

TanStack Table has **no built-in virtualization**. Instead, it pairs with [TanStack Virtual](https://tanstack.com/virtual/latest), a separate headless virtualization library (also framework-agnostic).

TanStack Virtual provides:
- Row virtualization (vertical windowing)
- Column virtualization (horizontal windowing)
- Grid virtualization (both axes)
- Variable row/column sizes
- Smooth scrolling at 60 FPS
- Overscan (render extra items beyond the viewport for smoother scrolling)

### 9.2 Integration Pattern

```typescript
const { rows } = table.getRowModel()

const rowVirtualizer = useVirtualizer({
  count: rows.length,
  getScrollElement: () => tableContainerRef.current,
  estimateSize: () => 35,       // estimated row height in px
  overscan: 10,                  // render 10 extra rows above/below viewport
})

const virtualRows = rowVirtualizer.getVirtualItems()
const totalSize = rowVirtualizer.getTotalSize()

return (
  <div ref={tableContainerRef} style={{ height: 600, overflow: 'auto' }}>
    <table>
      <tbody style={{ height: totalSize, position: 'relative' }}>
        {virtualRows.map(virtualRow => {
          const row = rows[virtualRow.index]
          return (
            <tr
              key={row.id}
              style={{
                position: 'absolute',
                top: virtualRow.start,
                height: virtualRow.size,
              }}
            >
              {row.getVisibleCells().map(cell => (
                <td key={cell.id}>
                  {flexRender(cell.column.columnDef.cell, cell.getContext())}
                </td>
              ))}
            </tr>
          )
        })}
      </tbody>
    </table>
  </div>
)
```

### 9.3 How It Works

1. **The table engine** produces the full row model (all filtered, sorted, paginated rows).
2. **The virtualizer** takes the row count and a scroll container reference.
3. On scroll, the virtualizer computes which row indices are in the viewport.
4. Only those rows are rendered in the DOM, positioned absolutely within a container sized to the total height.
5. The scroll container's scrollbar reflects the full dataset size.

### 9.4 Column Virtualization

For tables with many columns, column virtualization works the same way but on the horizontal axis:

```typescript
const columnVirtualizer = useVirtualizer({
  horizontal: true,
  count: visibleColumns.length,
  getScrollElement: () => tableContainerRef.current,
  estimateSize: index => visibleColumns[index].getSize(),
  overscan: 3,
})
```

### 9.5 Performance Characteristics

- TanStack Virtual can handle **hundreds of thousands of rows** with smooth 60 FPS scrolling.
- The DOM typically contains 20-50 row elements at any time, regardless of dataset size.
- Row height can be fixed or dynamic (measured after render via `measureElement`).
- The virtualizer is **reactive** -- when the row model changes (due to filtering, sorting), the virtualizer automatically adjusts.

### 9.6 Key Takeaway for Rust

In a GPU-rendered grid, "virtualization" is less about DOM management and more about which rows to compute cell layouts for. The concept still applies: only compute and render the visible window. The Rust grid engine should expose `get_visible_row_range(scroll_offset, viewport_height) -> Range<usize>` and the renderer draws only those rows.

---

## 10. MUI DataGrid

**Package**: `@mui/x-data-grid` (Community), `@mui/x-data-grid-pro` (Pro), `@mui/x-data-grid-premium` (Premium)
**Repository**: [mui/mui-x](https://github.com/mui/mui-x)
**License**: MIT (Community) / Commercial (Pro, Premium)

MUI DataGrid is the **opposite** of TanStack Table's philosophy. It is an opinionated, batteries-included, fully-rendered grid component. You pass `rows` and `columns` and get a complete interactive grid.

### 10.1 Architecture

```typescript
<DataGrid
  rows={rows}                    // { id, ...fields }[]
  columns={columns}              // GridColDef[]
  pageSizeOptions={[25, 50]}
  checkboxSelection
  disableRowSelectionOnClick
/>
```

The DataGrid renders its own DOM structure, manages its own state, and provides an **imperative API** via `apiRef`:

```typescript
const apiRef = useGridApiRef()

// Later:
apiRef.current.updateRows([{ id: 1, name: 'Updated' }])
apiRef.current.setPage(2)
apiRef.current.exportDataAsCsv()
```

### 10.2 Tier Comparison

| Feature | Community (Free) | Pro ($) | Premium ($$) |
|---------|-----------------|---------|--------------|
| Sorting | Yes | Yes | Yes |
| Filtering | Basic | Advanced (multi-filter) | Advanced |
| Pagination | Yes | Yes | Yes |
| Column pinning | No | Yes | Yes |
| Column reordering | No | Yes | Yes |
| Row reordering | No | Yes | Yes |
| Tree data | No | Yes | Yes |
| Master-detail | No | Yes | Yes |
| Virtualization | Basic | Advanced | Advanced |
| Lazy loading | No | Yes | Yes |
| Row grouping | No | No | Yes |
| Aggregation | No | No | Yes |
| Excel export | No | No | Yes |
| Clipboard paste | No | No | Yes |
| Cell selection (range) | No | No | Yes |

### 10.3 Row Grouping (Premium)

Rows can be grouped by one or more columns, creating collapsible tree structures:

```typescript
<DataGridPremium
  rows={rows}
  columns={columns}
  initialState={{
    rowGrouping: {
      model: ['company', 'department'],  // group by company, then department
    },
  }}
/>
```

Aggregation functions (sum, avg, min, max, size, custom) can be applied to grouped rows:

```typescript
columns={[
  { field: 'revenue', aggregable: true },
  { field: 'quantity', aggregable: true },
]}
initialState={{
  aggregation: {
    model: { revenue: 'sum', quantity: 'avg' },
  },
}}
```

### 10.4 Tree Data (Pro)

For pre-hierarchical data, Tree Data mode lets rows have parent-child relationships:

```typescript
<DataGridPro
  treeData
  getTreeDataPath={(row) => row.hierarchy}  // e.g., ['USA', 'California', 'LA']
  rows={rows}
  columns={columns}
/>
```

### 10.5 Inline Editing

MUI DataGrid supports cell and row editing modes:

```typescript
columns={[
  { field: 'name', editable: true },
  { field: 'age', editable: true, type: 'number' },
  { field: 'role', editable: true, type: 'singleSelect',
    valueOptions: ['Admin', 'User', 'Manager'] },
]}
processRowUpdate={(newRow, oldRow) => {
  // Validate, call API, return final row
  return saveToServer(newRow)
}}
```

The `processRowUpdate` callback is called when editing ends. It receives the new row data and must return the row object (possibly modified by server response) to commit to internal state. Return a rejected promise to revert the edit.

### 10.6 Excel Export (Premium)

```typescript
apiRef.current.exportDataAsExcel({
  fileName: 'report',
  includeHeaders: true,
  includeColumnGroupsHeaders: true,
})
```

Column types (number, date, boolean, singleSelect) are preserved in the Excel output.

### 10.7 Server-Side Data

MUI DataGrid Pro/Premium supports a **Data Source** abstraction for server-side data:

```typescript
<DataGridPro
  unstable_dataSource={{
    getRows: async (params: GridGetRowsParams) => {
      // params contains sortModel, filterModel, paginationModel
      const response = await fetchFromAPI(params)
      return { rows: response.data, rowCount: response.total }
    },
  }}
/>
```

When sort, filter, or pagination state changes, the grid calls `getRows()` with the updated parameters, and the server returns the appropriate slice.

### 10.8 Key Takeaway for Rust

MUI DataGrid demonstrates what a **fully-integrated** grid looks like. It bundles rendering, state, and interaction into one package. The commercial features (row grouping, aggregation, Excel export) represent the "enterprise" feature set that trading platforms often need. For the Hand of Midas grid, these features may be needed eventually but the headless approach gives more control over GPU rendering. The `processRowUpdate` callback pattern and the `apiRef` imperative API are patterns worth considering.

---

## 11. React Data Grid (Adazzle)

**Package**: `react-data-grid`
**Repository**: [adazzle/react-data-grid](https://github.com/adazzle/react-data-grid) (now under Comcast org)
**License**: MIT

React Data Grid aims to be an **Excel-like** grid for React, with rich cell editing and interaction patterns that mimic spreadsheet behavior.

### 11.1 Core Design

Unlike TanStack Table (headless) and MUI DataGrid (Material Design), React Data Grid provides a minimal, spreadsheet-focused component with a thin API:

```typescript
<DataGrid
  columns={columns}
  rows={rows}
  onRowsChange={setRows}      // the grid mutates rows and calls back
  rowKeyGetter={row => row.id}
/>
```

### 11.2 Key Features

| Feature | Details |
|---------|---------|
| **Virtualization** | Built-in. Only visible rows and columns are rendered. Handles 100K+ rows smoothly. |
| **Cell Editing** | Double-click or type to enter edit mode. Custom editors via `renderEditCell`. |
| **Cell Formatters** | Custom cell rendering via `renderCell` on column definition. |
| **Copy/Paste** | Ctrl+C copies cell value. Ctrl+V pastes into selected cell. |
| **Drag Fill** | Drag the fill handle (bottom-right corner of a cell) to apply value to adjacent cells. |
| **Frozen Columns** | `frozen: true` on column def pins it to the left. |
| **Column Resizing** | Built-in. Drag column borders to resize. |
| **Sorting** | Multi-column sorting with `onSortColumnsChange` callback. |
| **Row Selection** | Checkbox selection with `rowSelectionType: 'checkbox'`. |
| **Grouping** | Row grouping with expandable groups via `groupBy` prop. |
| **Tree Data** | Hierarchical rows with expand/collapse. |
| **Summary Rows** | Top and bottom summary rows for aggregations. |
| **Column Spanning** | `colSpan` function on column defs for merged cells. |

### 11.3 Cell Editors

React Data Grid has a rich editor system:

```typescript
const columns = [
  {
    key: 'name',
    name: 'Name',
    renderEditCell: ({ row, onRowChange }) => (
      <input
        value={row.name}
        onChange={e => onRowChange({ ...row, name: e.target.value })}
        autoFocus
      />
    ),
  },
  {
    key: 'priority',
    name: 'Priority',
    renderEditCell: ({ row, onRowChange, onClose }) => (
      <select
        value={row.priority}
        onChange={e => {
          onRowChange({ ...row, priority: e.target.value }, true)
          // true = commit immediately
        }}
      >
        <option>Low</option>
        <option>Medium</option>
        <option>High</option>
      </select>
    ),
  },
]
```

The `onRowChange(updatedRow, commitChanges?)` callback either stages changes (on each keystroke) or commits them (when the second argument is `true` or when the editor closes).

### 11.4 Drag Fill

When a cell is selected, a small handle appears at its bottom-right corner. Dragging this handle vertically fills adjacent cells with the source cell's value:

```typescript
<DataGrid
  columns={columns}
  rows={rows}
  onRowsChange={setRows}
  onFill={({ columnKey, sourceRow, targetRow }) => {
    // Return the updated target row
    return { ...targetRow, [columnKey]: sourceRow[columnKey] }
  }}
/>
```

The `onFill` callback receives the source cell's row and the target row, and returns the modified target row. This enables computed fills (e.g., incrementing values, applying formulas).

### 11.5 Custom Cell Rendering

```typescript
const columns = [
  {
    key: 'progress',
    name: 'Progress',
    renderCell: ({ row }) => (
      <div className="progress-bar">
        <div style={{ width: `${row.progress}%` }} />
      </div>
    ),
  },
  {
    key: 'flag',
    name: 'Country',
    renderCell: ({ row }) => <CountryFlag code={row.countryCode} />,
  },
]
```

### 11.6 Architecture Notes

- **Minimal dependencies**: Only one npm dependency (`clsx`).
- **React 18/19 compatible**: Uses modern React features.
- **Unidirectional data flow**: The grid never mutates your data directly. It calls `onRowsChange` with the updated rows array, and you decide whether to accept the changes.
- **Event-driven**: Callbacks for sort, fill, paste, selection, scroll, and editing.
- **TypeScript-first**: Full type safety with generic row types.

### 11.7 Key Takeaway for Rust

React Data Grid's Excel-like interaction model (drag fill, copy/paste, cell navigation with arrow keys) is directly relevant to a trading watchlist grid. The `onFill` callback pattern -- where the engine computes the fill operation and the host decides the result -- is a clean way to handle spreadsheet interactions in a headless engine.

---

## 12. Key Design Insight -- The Headless Pattern

### 12.1 What "Headless" Really Means

The headless pattern separates **table logic** from **table rendering** completely:

```
+-------------------------------------------+
|            HEADLESS ENGINE                 |
|                                           |
|  State:     sorting, filtering, sizing,   |
|             visibility, pinning, order,   |
|             selection, expansion, ...     |
|                                           |
|  Pipeline:  raw data -> core rows ->      |
|             filtered -> sorted ->         |
|             grouped -> paginated          |
|                                           |
|  Output:    row model, header groups,     |
|             cell values, computed sizes   |
|                                           |
|  API:       toggle sort, resize column,   |
|             set filter, select row, ...   |
+-------------------------------------------+
              |                ^
    computed  |                |  events / state changes
    output    v                |
+-------------------------------------------+
|            RENDERER                        |
|                                           |
|  Consumes:  row model, column sizes,      |
|             sort indicators, cell values  |
|                                           |
|  Produces:  DOM elements, canvas draws,   |
|             GPU vertices, native widgets  |
|                                           |
|  Delegates: click handlers, resize drag,  |
|             DnD operations, scroll events |
+-------------------------------------------+
```

### 12.2 Why This Matters for a Rust Native Grid

The Hand of Midas grid will render with GPU shaders (wgpu), not HTML elements. TanStack Table proves that the grid engine can be **entirely decoupled** from the renderer:

| TanStack Table (JS) | Rust Grid Engine (proposed) |
|---------------------|----------------------------|
| `useReactTable({ data, columns })` | `GridEngine::new(data, columns)` |
| `table.getState().sorting` | `engine.state().sorting` |
| `table.getRowModel().rows` | `engine.visible_rows()` |
| `header.getSize()` | `engine.column_width(col_id)` |
| `header.getResizeHandler()` | `engine.begin_resize(col_id, start_x)` / `engine.update_resize(delta_x)` / `engine.end_resize()` |
| `column.getToggleSortingHandler()` | `engine.toggle_sort(col_id)` |
| `flexRender(cell.column.columnDef.cell, ctx)` | User-supplied `CellRenderer` trait impl |
| `onSortingChange(updater)` | `engine.on_state_change(|old, new| { ... })` |

### 12.3 The State Management Insight

TanStack Table's most portable idea is its **state management model**:

1. **State is a plain data structure** -- no hidden internal state, no closures, no framework magic. Just a struct with fields like `sorting: Vec<ColumnSort>`, `column_visibility: HashMap<ColumnId, bool>`, etc.

2. **State changes flow through updater functions** -- the engine never mutates state directly. It proposes changes via callbacks. The host can accept, reject, or modify them.

3. **Computed values derive from state** -- row models, column sizes, and visible cells are pure functions of (data + state). When state changes, recompute.

4. **Features extend state and computations** -- each feature (sorting, filtering, etc.) adds its own state slice, its own pipeline step, and its own API methods. This maps to Rust traits.

### 12.4 The Row Model Pipeline Insight

The row model pipeline is a **functional transformation chain**:

```rust
// Pseudo-Rust
let core_rows = to_row_model(&data);
let filtered  = apply_filters(&core_rows, &state.filters);
let sorted    = apply_sorting(&filtered, &state.sorting, &sort_fns);
let grouped   = apply_grouping(&sorted, &state.grouping);
let expanded  = apply_expansion(&grouped, &state.expanded);
let paginated = apply_pagination(&expanded, &state.pagination);
// paginated = final visible rows
```

Each step is a pure function. Steps that aren't needed are simply skipped (no-ops). This is trivially parallelizable and cache-friendly in Rust.

### 12.5 Comparison Matrix

| Aspect | TanStack Table | MUI DataGrid | React Data Grid |
|--------|---------------|--------------|-----------------|
| **Philosophy** | Headless engine | Batteries-included component | Excel-like component |
| **Rendering** | You provide it | Built-in (Material UI) | Built-in (minimal) |
| **Customization** | Total control | Slot-based overrides | Render callbacks |
| **Bundle size** | ~12-16 KB core | ~100+ KB | ~40-60 KB |
| **Learning curve** | Higher (must build UI) | Lower (drop-in) | Medium |
| **TypeScript** | Excellent | Good | Excellent |
| **Framework** | React, Vue, Solid, Svelte | React only | React only |
| **Virtualization** | Via TanStack Virtual | Built-in | Built-in |
| **Cell editing** | Build your own | Built-in (rich) | Built-in (Excel-like) |
| **DnD** | Via dnd-kit | Built-in (Pro) | Drag-fill only |
| **Excel export** | No | Premium only | No |
| **License** | MIT | MIT / Commercial | MIT |
| **Best for** | Custom grid UIs, non-React targets | Enterprise React apps with Material | Spreadsheet-like React apps |

### 12.6 Recommendations for Hand of Midas

1. **Adopt the headless pattern** -- Build a `GridEngine` struct in Rust that manages all state and row models. The wgpu renderer consumes its output.

2. **Mirror TanStack's state model** -- Use `GridState` as a plain struct with sub-states for each feature. State changes emit events the UI layer handles.

3. **Implement the row model pipeline** -- Pure functions chained together. Only compute what's needed.

4. **Column definitions as data** -- `ColumnDef` struct with accessor (field path or function), sort function key, size constraints, visibility flag, pin position.

5. **Steal React Data Grid's interaction model** -- Cell navigation via arrow keys, copy/paste, drag-fill. These are essential for a trading watchlist.

6. **Expose an `apiRef`-like imperative API** -- For programmatic control from the broker engine (e.g., "scroll to this symbol", "highlight this row").

7. **Keep DnD in the renderer** -- The engine only receives commands like `move_row(from, to)` and `move_column(from, to)`. The renderer handles the drag interaction.

---

## Sources

- [TanStack Table Overview](https://tanstack.com/table/v8/docs/overview)
- [TanStack Table Introduction](https://tanstack.com/table/latest/docs/introduction)
- [Table Instance Guide](https://tanstack.com/table/v8/docs/guide/tables)
- [Column Defs Guide](https://tanstack.com/table/v8/docs/guide/column-defs)
- [Cells Guide](https://tanstack.com/table/latest/docs/guide/cells)
- [Row Models Guide](https://tanstack.com/table/v8/docs/guide/row-models)
- [Column Sizing Guide](https://tanstack.com/table/v8/docs/guide/column-sizing)
- [Column Sizing APIs](https://tanstack.com/table/v8/docs/api/features/column-sizing)
- [Column Ordering Guide](https://tanstack.com/table/v8/docs/guide/column-ordering)
- [Column Pinning Guide](https://tanstack.com/table/v8/docs/guide/column-pinning)
- [Column Pinning APIs](https://tanstack.com/table/v8/docs/api/features/pinning)
- [Column Visibility Guide](https://tanstack.com/table/v8/docs/guide/column-visibility)
- [Column Visibility APIs](https://tanstack.com/table/v8/docs/api/features/column-visibility)
- [Sorting Guide](https://tanstack.com/table/v8/docs/guide/sorting)
- [Sorting APIs](https://tanstack.com/table/v8/docs/api/features/sorting)
- [Virtualization Guide](https://tanstack.com/table/v8/docs/guide/virtualization)
- [Row DnD Example](https://tanstack.com/table/v8/docs/framework/react/examples/row-dnd)
- [Column DnD Example](https://tanstack.com/table/latest/docs/framework/react/examples/column-dnd)
- [Table State Guide (React)](https://tanstack.com/table/v8/docs/framework/react/guide/table-state)
- [Table APIs](https://tanstack.com/table/v8/docs/api/core/table)
- [ColumnDef APIs](https://tanstack.com/table/v8/docs/api/core/column-def)
- [Column Resizing Performant Example](https://tanstack.com/table/v8/docs/framework/react/examples/column-resizing-performant)
- [TanStack Virtual](https://tanstack.com/virtual/latest)
- [TanStack/table GitHub](https://github.com/TanStack/table)
- [Core Architecture (DeepWiki)](https://deepwiki.com/tanstack/table/2.1-core-architecture)
- [Column Management (DeepWiki)](https://deepwiki.com/tanstack/table/4.5-column-management)
- [Sorting (DeepWiki)](https://deepwiki.com/tanstack/table/4.1-sorting)
- [Table Instance and State Management (DeepWiki)](https://deepwiki.com/TanStack/table/2.2-table-instance-and-state-management)
- [MUI X Data Grid](https://mui.com/x/react-data-grid/)
- [MUI DataGrid Feature Showcase](https://mui.com/x/react-data-grid/features/)
- [MUI DataGrid Row Grouping](https://mui.com/x/react-data-grid/row-grouping/)
- [MUI DataGrid Aggregation](https://mui.com/x/react-data-grid/aggregation/)
- [MUI DataGrid Server-Side Data](https://mui.com/x/react-data-grid/server-side-data/)
- [MUI DataGrid Editing](https://mui.com/x/react-data-grid/editing/)
- [MUI DataGrid API Object](https://mui.com/x/react-data-grid/api-object/)
- [MUI DataGrid Export](https://mui.com/x/react-data-grid/export/)
- [React Data Grid (adazzle/Comcast)](https://github.com/Comcast/react-data-grid)
- [React Data Grid (DeepWiki)](https://deepwiki.com/adazzle/react-data-grid)
- [dnd-kit](https://dndkit.com/)
