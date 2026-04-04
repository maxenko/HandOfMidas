# Grid Component: Interactions and UX Specification

> Hand of Midas -- GPU-rendered trading grid for iced 0.14 / wgpu 27
> Revision: 2026-04-01

---

## Table of Contents

1. [Shared Concepts](#1-shared-concepts)
2. [Column Resizing](#2-column-resizing)
3. [Column Reordering](#3-column-reordering)
4. [Row Selection](#4-row-selection)
5. [Row Drag-and-Drop](#5-row-drag-and-drop)
6. [Sorting](#6-sorting)
7. [Keyboard Navigation](#7-keyboard-navigation)
8. [Scrolling](#8-scrolling)
9. [Context Menu](#9-context-menu)
10. [Cell Interactions](#10-cell-interactions)
11. [Trading-Specific Interactions](#11-trading-specific-interactions)
12. [Interaction Conflict Resolution](#12-interaction-conflict-resolution)

---

## 1. Shared Concepts

### 1.1 Coordinate Spaces

All interactions operate in two coordinate spaces:

| Space | Origin | Usage |
|---|---|---|
| **Window** | Top-left of the iced window | Raw cursor events from iced |
| **Grid-local** | Top-left of the grid widget | Hit-testing, layout, column boundaries |

Every incoming cursor position must be translated from window space to grid-local
space before hit-testing. The grid widget provides `screen_to_grid(Point) -> Point`.

### 1.2 Hit Zones

The grid is divided into non-overlapping hit zones tested in priority order:

```
+------------------------------------------+
|           HEADER ROW (zone H)            |
|  [H1] [|] [H2] [|] [H3] [|] [H4]       |   [|] = resize handle (zone R)
+------------------------------------------+
|           BODY AREA (zone B)             |
|  [drag] [fav] [AAPL] [182.50] [+1.2%]   |   [drag] = drag handle (zone D)
|  [drag] [fav] [MSFT] [415.30] [-0.4%]   |
|  ...                                     |
+------------------------------------------+
|           ADD-TICKER ROW (zone A)        |
+------------------------------------------+
```

Priority order for hit-testing a cursor position:

1. **Drag overlay** -- if a drag is active, the overlay is excluded (hit-test-invisible)
2. **Context menu** -- if open, consumes click or dismisses
3. **Resize handle (R)** -- 8px-wide vertical strips between header cells (4px each side of column boundary)
4. **Header cell (H)** -- the header cell body (excluding resize handles)
5. **Drag handle column (D)** -- the leftmost grip column in the body
6. **Interactive cell widget** -- buttons, toggles within cells
7. **Body row (B)** -- the remaining row area
8. **Add-ticker row (A)** -- the input row at the bottom
9. **Empty area** -- below all rows, above add-ticker row

### 1.3 Drag Threshold

All drag operations share a common activation threshold:

- **Distance**: 5 logical pixels of cursor movement from the initial press point
- **Calculation**: Euclidean distance `sqrt((dx*dx) + (dy*dy)) >= 5.0`
- **Purpose**: Prevents accidental drags during click/sort/select gestures
- **Implementation**: All drag state machines begin in a `Pending` state that
  accumulates cursor deltas until the threshold is crossed

### 1.4 Animation Constants

| Animation | Duration | Easing | Usage |
|---|---|---|---|
| Column slide on reorder | 200ms | ease-out (cubic-bezier 0.0, 0.0, 0.2, 1.0) | Columns sliding to make room |
| Row slide on reorder | 200ms | ease-out | Rows sliding to make room |
| Ghost pickup scale | 120ms | ease-out | Scale 1.0 -> 1.03 on drag start |
| Ghost drop settle | 150ms | ease-in-out | Ghost snaps to final position |
| Flash-on-tick | 300ms total | ease-out (cubic) | Cell highlight fades from peak to base |
| Sort indicator appear | 100ms | ease-out | Arrow glyph fades in |
| Selection highlight | 50ms | linear | Row background color change |

### 1.5 Z-Order (Render Layers)

> See 02-rendering.md §2 for the canonical 7-layer rendering architecture.
> The interaction-relevant layers are summarized here for quick reference:

```
Layer 0: Row background (stripes, selection highlight)
Layer 1: Grid lines (column/row separators)
Layer 2: Cell content (text, icons, buttons)
Layer 3: Flash overlay (flash-on-tick color wash)
Layer 4: Selection overlay (focused row border)
Layer 5: Header (fixed, sort indicators, resize handles)
Layer 6: Drag overlay + drop indicators + context menu (topmost)
```

### 1.6 Message Naming Convention

All grid messages follow the existing project convention from `app.rs`:
`Grid<Action>(GridId, ...params)`. The grid component itself emits typed messages
that the parent application maps into its own `Message` enum.

> **Canonical definition**: See 00-architecture.md §4.1 for the canonical `GridMessage`
> enum with authoritative variant names. The listing below shows the **all-phases-complete
> target shape** with expanded variants for each interaction category. Variant names
> use this document's descriptive convention; the canonical names in 00-architecture.md
> take precedence during implementation (e.g., `ResizeStarted` not `ColumnResizeStart`,
> `RowSelected` not `RowClicked`).

| Category | Variants (all phases) | Phase |
|---|---|---|
| Column resize | `ResizeStarted(ColumnId)`, `Resizing(f32)`, `ResizeEnded` | 1 |
| Column auto-fit | `AutoFitColumn(ColumnId)` | 4 |
| Column reorder | `ColumnDragStarted(ColumnId, f32)`, `ColumnDragging(f32)`, `ColumnDragEnded`, `ColumnDragCancelled` | 2 |
| Sort | `SortToggled(ColumnId)` | 0 |
| Multi-sort | `SortByMulti(ColumnId)` (Shift+click) | 4 |
| Row selection | `RowSelected(usize)`, `RowToggled(usize)`, `RowRangeSelected(usize)`, `SelectAll`, `DeselectAll` | 0-1 |
| Row drag | `RowDragStarted(usize)`, `RowDragging(f32)`, `RowDragEnded(usize)`, `RowDragCancelled`, `RowDragExternal(usize, ExternalDropTarget)` | 2 |
| Keyboard | `FocusMove(Direction)`, `ActivateRow(usize)`, `DeleteRows(Vec<usize>)`, `PageScroll(Direction)` | 3 |
| Scroll | `ScrollChanged(f32)` | 0 |
| Context menu | `ContextMenuOpen(usize, Point)`, `ContextMenuHeaderOpen(ColumnId, Point)`, `ContextMenuAction(ContextAction)`, `ContextMenuDismiss` | 4 |
| Trading | `SymbolActivated(String)`, `FlashCell { column: ColumnId, row_key: RowKey, direction: FlashDirection }`, `FlashTick` | 3 |

Cell widget interactions (button clicks, toggles, star) are **not** `GridMessage`
variants — they emit the application's message type `M` directly via the cell's
`Element<M>`. See 00-architecture.md §5.3.

**Descriptive → Canonical Name Mapping**: This document uses descriptive names in
prose and state machine diagrams. During implementation, use the canonical names
from 00-architecture.md §4.1. Key mappings:

| This document (descriptive) | Canonical (00-architecture.md §4.1) |
|---|---|
| `ColumnResizeStart { col, x }` | `ResizeStarted(ColumnId)` |
| `SortBy { col }` | `SortToggled(ColumnId)` |
| `RowClicked { row }` | `RowSelected(usize)` |
| `RowCtrlClicked { row }` | `RowToggled(usize)` |
| `RowShiftClicked { row }` | `RowRangeSelected(usize)` |
| `RowDragDrop { from, to }` | `RowDragEnded(usize)` |
| `ColumnReorderStart { col }` | `ColumnDragStarted(ColumnId, f32)` |
| `SortByMulti { col }` | `SortByMulti(ColumnId)` (Phase 4) |

---

## 2. Column Resizing

### 2.1 Trigger

**What starts it**: The user positions the cursor over the vertical divider between
two adjacent column headers and presses the left mouse button.

**Detection**: The resize handle is an 8px-wide invisible hit zone centered on the
column boundary (4px on each side). When the cursor enters this zone, the cursor changes to a
horizontal-resize indicator (`ew-resize` / `col-resize`). Canonical width: 02-rendering.md §2.

> **Migration note**: The current `views.rs` implementation uses 4px resize handles. Phase 0 migrates these as-is. Phase 1 widens to 8px as specified here.

```
         col_right_edge
              |
    [ Header A ]|[ Header B ]
              |
        <--3px--3px-->
        resize hit zone
```

### 2.2 State Machine

```
                     cursor enters 6px zone
          Idle ─────────────────────────────> HoverHandle(col)
           ^                                      |
           |  cursor leaves zone                  | mousedown
           +──────────────────────────────────────+
                                                  |
                                                  v
                                          Resizing(state)
                                           |         |
                                  mousemove|         | mouseup
                                           v         v
                                    (update width)  Idle
                                           |
                                           +──> Resizing(state)
```

**States**:

| State | Data | Cursor |
|---|---|---|
| `Idle` | none | default arrow |
| `HoverHandle(col_index)` | column index of the right-edge being hovered | `ew-resize` |
| `Resizing { col, start_x, start_width, min_width, max_width }` | all resize context | `ew-resize` |

### 2.3 Behavior

**Press**: Record `start_x` (grid-local X of the cursor), `start_width` of the
column, and the column's `min_width` / `max_width` constraints. Note: `start_x`
is `Option<f32>` (None until first mouse move) because iced 0.14's
`mouse_area::on_press` does not provide cursor coordinates. See 00-architecture.md §6.4.

**Drag**: On each `mousemove`:
1. Compute `delta_x = current_x - start_x`.
2. Compute `new_width = (start_width + delta_x).clamp(min_width, max_width)`.
3. Update the column width in the grid state.
4. The column to the right is **not** affected (resize-only-this-column mode). The
   grid total width changes, and a horizontal scrollbar appears if content overflows.

**Release**: Finalize the width. Emit `ColumnResizeEnd`. Mark config dirty for
persistence.

### 2.4 Double-Click to Auto-Fit

**Trigger**: Double-click (two clicks within 300ms, within 4px of each other) on a
resize handle.

**Behavior**: Measure the maximum content width across all visible rows for that
column (including the header text), add 16px padding (8px each side), and set the
column width to `max(measured + padding, min_width)`.

**Message**: `ColumnAutoFit { col }`.

**Implementation note**: Auto-fit requires measuring text widths. Since the grid
uses iced widgets for cell content (not a custom wgpu text pipeline), direct text
measurement is not available via a public iced API. Use a character-count heuristic
(characters × average character width for the font size, e.g., ~8px per character
at size 13). For Phase 3+, investigate `iced_core::text::Renderer::measure` if
it becomes available. The measurement iterates ALL rows (not just visible), since
watchlists have at most a few hundred rows and string-length iteration is trivial.

### 2.5 Visual Feedback

**Live preview**: The column width updates in real time during the drag. There is no
ghost line -- the header and all visible body cells resize live as the cursor moves.
This matches the behavior of AG Grid, WPF DataGrid, and all professional trading
platforms.

**Minimum width enforcement**: If the user drags below `min_width`, the column snaps
to `min_width` and stays there. The cursor can move further left, but the column does
not shrink below the minimum. When the cursor moves back to the right, resizing
resumes immediately (no dead zone).

### 2.6 Adjacent Column Behavior

**Mode: Resize-only-this-column** (the default, matching AG Grid and Bloomberg).

Only the column whose right edge is being dragged changes width. All columns to the
right shift horizontally but retain their own widths. The total grid content width
changes. If it exceeds the viewport, a horizontal scrollbar appears.

The alternative mode (push adjacent column, as in some spreadsheet applications) is
not implemented because trading grids prioritize independent column sizing.

### 2.7 Width Constraints

Each column exposes width constraints via the `GridColumn` trait methods
(see 03-column-data-model.md §1.1 for the canonical trait definition):

- `min_width() -> f32` — minimum width in logical pixels (default: 20.0)
- `max_width() -> Option<f32>` — maximum width (default: None = unbounded)
- `resizable() -> bool` — whether the user can resize this column (default: true)

Non-resizable columns (e.g., the drag handle column, the favorite star column, the
delete button column) return `resizable() -> false` and do not show resize handles or
change cursor on hover.

### 2.8 Messages Emitted

| User Action | Message |
|---|---|
| Cursor enters resize zone | *(no message -- internal cursor state change only)* |
| Mousedown on resize handle | `ColumnResizeStart { col, x }` |
| Mousemove during resize | `ColumnResizing { x }` |
| Mouseup | `ColumnResizeEnd` |
| Double-click on resize handle | `ColumnAutoFit { col }` |

### 2.9 Edge Cases

| Scenario | Behavior |
|---|---|
| Column at min_width, user drags left | Column stays at min_width; cursor moves freely |
| Column at max_width, user drags right | Column stays at max_width; cursor moves freely |
| Cursor leaves grid area during resize | Resize continues (mouse is captured); release anywhere ends it |
| Right-most column resize handle | Resize handle exists on its right edge; resizing extends total grid width |
| Left-most column has no left resize handle | Only right edges have handles |
| Non-resizable column between resizable ones | Its right-edge handle is hidden; the left column's right-edge handle on the non-resizable column's left boundary is also hidden |
| Window resize during column resize | Abort the resize operation cleanly |

---

## 3. Column Reordering

### 3.1 Trigger

**What starts it**: The user presses the left mouse button on a column header cell
body (not the resize handle zone) and moves the cursor at least 5px from the press
point.

**Distinction from sort click**: A click-and-release without crossing the 5px
threshold is treated as a sort click (see Section 6). Only after the threshold is
crossed does the interaction become a drag.

### 3.2 State Machine

```
                   mousedown on header cell body
         Idle ──────────────────────────────────> DragPending {
           ^                                        col, press_pos
           |                                      }
           |                                      |
           | mouseup (< 5px)                      | mousemove (>= 5px from press)
           | => emit SortBy                       |
           +──────────────────────────────────────+
                                                  |
                                                  v
                                           Dragging {
                                             col,
                                             ghost_x,
                                             insertion_index,
                                             hotspot_offset
                                           }
                                            |    |    |
                                   mousemove|    |    | Escape key
                                            v    |    v
                                    (update pos) |   Cancel => Idle
                                            |    |      (snap-back anim)
                                            +----+
                                                 |
                                                 | mouseup over valid position
                                                 v
                                              Drop {
                                                from: col,
                                                to: insertion_index
                                              }
                                                 |
                                                 v
                                               Idle
                                          (commit reorder)
```

### 3.3 Drag Activation

1. On `mousedown` over a header cell body: enter `DragPending`. Record the column
   index, press position, and the cursor's offset from the header cell's left edge
   (the hotspot).
2. On each `mousemove` in `DragPending`: compute distance from press point. If
   `distance >= 5.0`, transition to `Dragging`.
3. On `mouseup` in `DragPending` (distance < 5px): this was a click, not a drag.
   Emit `SortBy { col }` and return to `Idle`.

### 3.4 Drag Preview Visual (Ghost)

When entering `Dragging`:

1. **Create ghost**: A semi-transparent (0.75 opacity) floating rectangle containing
   the column header text and sort indicator (if any). Dimensions match the header
   cell's width and height.
2. **Drop shadow**: 2px offset, 8px blur radius, rgba(0,0,0,0.25) -- rendered via
   iced's `Shadow` style on the ghost `Container`.
3. **Scale**: Animate from 1.0 to 1.03 over 120ms ease-out.
4. **Vertical lock**: The ghost moves only horizontally. Its Y position is fixed to
   the header row's Y position.
5. **Hotspot**: The ghost's left edge is at `cursor_x - hotspot_offset`, so the grab
   point stays consistent.

```
During drag (dragging column "Price" rightward):

  [Symbol] [  ···  ] [Volume] [Change]     <- header row (source slot dimmed)
                    ^
                    | insertion indicator (2px, accent color)

                         [Price]            <- floating ghost at cursor X
                         (0.75 opacity, drop shadow)
```

### 3.5 Source Column Visual

The original column header slot shows a dimmed placeholder:
- Background: 30% opacity of the normal header background
- Text: hidden
- Borders: dashed outline in muted color

### 3.6 Drop Indicators

A **vertical insertion line** appears between columns at the computed drop position:

- **Width**: 2px
- **Color**: Accent color (the application's link/accent blue)
- **Height**: Full grid height (header + visible body rows)
- **Position**: Centered on the boundary between two columns
- **Terminal dots**: 4px radius circles at top and bottom ends of the line

**Insertion index calculation**: Compare the ghost's center X against column boundary
midpoints. The column boundary midpoints are at `col_left + col_width / 2`. The
insertion index is the boundary whose midpoint is closest to the ghost center.

```rust
fn compute_insertion_index(&self, cursor_x: f32) -> usize {
    let mut best = 0;
    let mut best_dist = f32::MAX;
    let mut x = 0.0;
    for (i, &width) in self.column_widths.iter().enumerate() {
        let midpoint = x + width / 2.0;
        let dist = (cursor_x - midpoint).abs();
        if dist < best_dist {
            best_dist = dist;
            best = i;
        }
        x += width;
    }
    // Insert before `best` if cursor is left of midpoint, after if right
    let col_left = self.column_x(best);
    let midpoint = col_left + self.column_widths[best] / 2.0;
    if cursor_x < midpoint { best } else { best + 1 }
}
```

### 3.7 Animation

**Columns slide to make room**: When the insertion index changes (the ghost moves
past a column midpoint), the columns between the old and new positions animate their
X offset over 200ms ease-out. This creates the visual effect of columns smoothly
sliding apart to make room.

Implementation: Each column maintains an `x_offset_anim: f32` that is added to its
layout X. When the insertion index changes, affected columns' target offsets are set
to `+/- dragged_column_width`, and the animation interpolates from current to target.

### 3.8 Cancel

**Escape key**: While in `Dragging`, pressing Escape cancels the reorder:
1. Animate the ghost back to its original position over 200ms ease-out.
2. Fade ghost opacity from 0.75 to 0.0 over the same duration.
3. Restore the source column slot to full opacity.
4. Reset all column X offsets to 0.
5. Return to `Idle`.

**Release outside valid area**: If the cursor is released outside the header row area
or outside the grid entirely, treat as cancel (same animation).

### 3.9 Drop

On `mouseup` over a valid insertion position:
1. If `from == to` or `from + 1 == to` (dropped back in same position), treat as no-op.
2. Otherwise: update the `display_order: Vec<usize>` mapping.
3. Animate the ghost to the target slot position over 150ms ease-out.
4. Fade ghost out as the real header fades in (crossfade over 100ms).
5. Emit `ColumnReorderDrop { from, to }`.
6. Mark config dirty.

### 3.10 Messages Emitted

| User Action | Message |
|---|---|
| Mousedown on header body | *(internal: enter DragPending)* |
| Mousemove crosses 5px threshold | `ColumnReorderStart { col, x }` |
| Mousemove during drag | `ColumnReorderMove { x }` |
| Mouseup on valid position | `ColumnReorderDrop { from, to }` |
| Escape key or release outside | `ColumnReorderCancel` |
| Mouseup without crossing threshold | `SortBy { col }` (see Section 6) |

### 3.11 Edge Cases

| Scenario | Behavior |
|---|---|
| Column has `reorderable: false` | Mousedown on its header enters sort-click, never drag |
| Dragging over a non-reorderable column | The insertion indicator can appear on either side of the locked column, but the locked column does not move |
| Only one column in the grid | Drag is possible but always results in no-op |
| Column resize starts during reorder | Not possible -- resize handle and header body are mutually exclusive hit zones |
| Sort click intent but user slightly moves mouse | DragPending absorbs small movements; only 5px+ triggers drag |
| Drop at same position | No-op, no message emitted, ghost snaps back |

---

## 4. Row Selection

### 4.1 Trigger

**What starts it**: The user clicks (mousedown + mouseup without crossing drag
threshold) on a row in the grid body area, outside of interactive cell widgets and
the drag handle column.

### 4.2 State Machine

Row selection does not have a complex state machine. It is an immediate response to
click events, modified by held modifier keys.

```
Click (no modifier)  =>  Select single row, deselect all others
Ctrl+Click           =>  Toggle selection of clicked row (others unchanged)
Shift+Click          =>  Range select from anchor to clicked row
Click on empty area  =>  Deselect all
```

### 4.3 Selection Model

> **Phase annotation**: The `BTreeSet<usize>` shape shown below is the
> **all-phases-complete target shape**. Phases 0--2 use the simpler
> `Option<usize>` form defined in 00-architecture.md (single-row selection
> only, no multi-select). The full `BTreeSet` / multi-select shape arrives
> in Phase 3a when `RowKey`-based selection is introduced.

```rust
pub struct SelectionState {
    /// Set of selected row indices (using a BTreeSet for ordered iteration).
    pub selected: BTreeSet<usize>,
    /// The anchor row for Shift+click range selection.
    /// Set to the last row that was single-clicked or Ctrl+clicked.
    pub anchor: Option<usize>,
    /// The focused row (receives keyboard events). May differ from selection.
    pub focused: Option<usize>,
}
```

### 4.4 Single Click (No Modifier)

1. Clear all existing selections.
2. Add the clicked row to `selected`.
3. Set `anchor = clicked_row`.
4. Set `focused = clicked_row`.
5. Emit `RowClicked { row }`.
6. **Trading-specific**: If symbol linking is active, also emit
   `SymbolActivated { symbol }` which propagates to linked chart panels.

### 4.5 Ctrl+Click

1. If the clicked row is currently selected: remove it from `selected`.
2. If the clicked row is not selected: add it to `selected`.
3. Set `anchor = clicked_row`.
4. Set `focused = clicked_row`.
5. Emit `RowCtrlClicked { row }`.

### 4.6 Shift+Click

1. If `anchor` is `None`: treat as single click.
2. If `anchor` is `Some(a)`:
   a. Determine the range `[min(a, clicked_row)..=max(a, clicked_row)]`.
   b. Clear all existing selections.
   c. Select all rows in the range.
   d. Keep `anchor` unchanged (it stays at `a`).
   e. Set `focused = clicked_row`.
3. Emit `RowShiftClicked { row }`.

**Note**: Ctrl+Shift+Click is treated as Shift+Click (range select). The Ctrl
modifier is ignored when Shift is also held.

### 4.7 Click on Empty Area

**Empty area** = the region below the last data row and above the add-ticker input,
or any area to the right of all columns.

1. Clear all selections.
2. Clear `anchor`.
3. `focused` remains unchanged (keyboard focus does not move to "nothing").
4. Emit `DeselectAll`.

### 4.8 Visual Feedback

| Element | Appearance |
|---|---|
| **Selected row background** | Accent color at 15% opacity overlaid on the normal row background |
| **Selected + focused row** | Accent color at 20% opacity + 1px left border accent line |
| **Focused but unselected row** | Subtle dotted 1px border (focus ring) -- visible only during keyboard navigation |
| **Hover row** | Row background lightens by 5% (or darkens by 5% in dark theme) |
| **Hover + selected** | Accent color at 22% opacity (slightly brighter than selected-only) |

Selection highlights are rendered in the selection overlay layer (Layer 4 per
02-rendering.md §2) so that text remains fully legible.

### 4.9 Keyboard Selection

Arrow Up/Down keys move `focused` and, when no modifier is held, also move
`selected` (single-row select follows focus). See Section 7 for full keyboard
navigation.

- **Arrow without modifier**: Move focus, select focused row, deselect others.
- **Shift+Arrow**: Extend selection range from anchor to new focus.
- **Ctrl+Arrow**: Move focus without changing selection.
- **Ctrl+Space**: Toggle selection of focused row (equivalent to Ctrl+Click).

### 4.10 Messages Emitted

| User Action | Message |
|---|---|
| Click on row (no modifier) | `RowClicked { row }` |
| Ctrl+Click on row | `RowCtrlClicked { row }` |
| Shift+Click on row | `RowShiftClicked { row }` |
| Click on empty area | `DeselectAll` |
| Ctrl+A | `SelectAll` |
| Escape (when selection exists) | `DeselectAll` |

### 4.11 Edge Cases

| Scenario | Behavior |
|---|---|
| Click on interactive cell widget (button, toggle) | Widget receives the click; row selection does NOT change |
| Click on drag handle column | Enters drag-pending, not selection |
| Shift+Click with no anchor | Treated as single click (sets anchor) |
| Row is removed while selected | Remove from `selected` set; if it was `anchor`, clear anchor |
| Sort reorders rows while selected | **Phase 0-2**: Selection uses index-based tracking with `selected_symbol` as authoritative identity; selection may shift to the wrong row after re-sort (known limitation, see 04-implementation-roadmap.md Phase 0 acceptance criteria). **Phase 3a+**: Selection follows data identity via `RowKey` extraction function (see 03-column-data-model.md §4.4), which provides stable selection across sort and data changes. |
| All rows deselected, user presses Arrow Down | Focus moves to row 0, selects it |

---

## 5. Row Drag-and-Drop

### 5.1 Trigger

**What starts it**: The user presses the left mouse button on the **drag handle
column** (leftmost column, showing a six-dot grip icon) and moves the cursor at
least 5px.

The drag handle column is a dedicated non-data column with fixed 26px width,
`reorderable: false`, `resizable: false`, `sortable: false`.

### 5.2 State Machine

```
                    mousedown on drag handle cell
          Idle ────────────────────────────────────> DragPending {
            ^                                          row, press_pos
            |                                        }
            |                                        |
            | mouseup (< 5px)                        | mousemove (>= 5px)
            | => no-op                               |
            +────────────────────────────────────────+
                                                     |
                                                     v
                                              Dragging {
                                                row,
                                                ghost_y,
                                                insertion_index,
                                                hotspot_offset,
                                                over_external_target: bool
                                              }
                                               |    |    |     |
                                      mousemove|    |    |     | cursor leaves grid
                                               v    |    |     v
                                         (update)  |    |  ExternalDrag {
                                               |    |    |    target
                                               +----+    |  }
                                                    |    |     |
                                                    |    |     | mouseup on external
                                                    |    |     v
                                                    |    |  RowDragExternal
                                                    |    |     |
                                                    |    |     v
                                                    |    |   Idle
                                                    |    |
                                                    |    | Escape key
                                                    |    v
                                                    | Cancel => Idle
                                                    |    (source row restores)
                                                    |
                                                    | mouseup in grid body
                                                    v
                                                 Drop {
                                                   from: row,
                                                   to: insertion_index
                                                 }
                                                    |
                                                    v
                                                  Idle
```

### 5.3 Drag Activation

1. On `mousedown` in drag handle cell: enter `DragPending`. Record row index, press
   position, hotspot (cursor Y offset from row top edge).
2. On `mousemove`: if distance >= 5px from press, transition to `Dragging`.
3. On `mouseup` without threshold: no-op (return to Idle).

### 5.4 Drag Preview Visual (Ghost Row)

The ghost row is a semi-transparent copy of the full row content:

| Property | Value |
|---|---|
| **Content** | All visible column cells for the dragged row |
| **Opacity** | 0.80 |
| **Drop shadow** | 4px Y-offset, 12px blur, rgba(0,0,0,0.15) |
| **Scale** | 1.03 (animated from 1.0 over 120ms on pickup) |
| **Width** | Same as the grid viewport width |
| **Background** | Solid (the row's normal background color at full opacity within the ghost) |
| **Border** | 1px accent-colored border, corner radius 2px |
| **Horizontal lock** | The ghost moves only vertically. X position is locked to the grid's left edge. |

**Customizable representation**: The ghost content can be overridden per use case.
For a watchlist, the default is the full row. For other grid uses, it could be a
simplified version (e.g., just the symbol name and an icon).

The grid provides a `drag_preview` callback in its configuration:

```rust
pub type DragPreviewFn = Box<dyn Fn(&RowData, Rect) -> DragVisual>;
```

If not provided, the default full-row ghost is used.

### 5.5 Source Row Visual

While dragging, the source row in its original position is dimmed:
- Opacity: 30% of normal
- Background: unchanged (but visible through the reduced opacity)
- No content change -- the row stays in place but is clearly "vacated"

### 5.6 Drop Indicator

A horizontal insertion line appears between rows at the computed drop position:

| Property | Value |
|---|---|
| **Thickness** | 2px |
| **Color** | Accent blue |
| **Width** | Full grid body width |
| **Terminal dots** | 4px radius circles at left and right ends, bleeding 4px outward |
| **Position** | Between two rows, at the boundary `row_y - 1px` |

**Insertion index calculation**: Compare the ghost's center Y against row boundary
midpoints. Midpoint = `row_top + row_height / 2`. If the cursor is above the
midpoint, insert before that row; if below, insert after.

### 5.7 Animated Reorder

As the drag moves over a new insertion position, rows smoothly slide out of the way:

1. Rows that need to move up (because the insertion point moved down past them)
   animate `translateY(-row_height)` over 200ms ease-out.
2. Rows that need to move down (because the insertion point moved up past them)
   animate `translateY(+row_height)` over 200ms ease-out.
3. Only rows between the source position and the current insertion position animate.
4. Rows outside that range have zero offset.

Performance: Only visible rows participate in the animation. With virtualized
scrolling, this is at most `viewport_height / row_height + 2` rows.

### 5.8 Auto-Scroll

When the cursor is within 40px of the grid's top or bottom edge during a drag, the
grid auto-scrolls:

| Zone | Behavior |
|---|---|
| 0-20px from top edge | Scroll up at 200px/sec |
| 20-40px from top edge | Scroll up at 100px/sec |
| 0-20px from bottom edge | Scroll down at 200px/sec |
| 20-40px from bottom edge | Scroll down at 100px/sec |

Auto-scroll is continuous (applied each frame while the cursor remains in the zone).
The insertion indicator updates as new rows scroll into view.

### 5.9 Cancel

**Escape key**: Cancel the drag:
1. Animate ghost back to the source row's current visual position (200ms ease-out).
2. Restore source row to full opacity (100ms ease-out).
3. Reset all row Y offsets to 0 (200ms ease-out).
4. Return to `Idle`.

**Release on invalid area**: Same as Escape.

### 5.10 Drop

On `mouseup` in the grid body:
1. If `from == to`: no-op.
2. Animate ghost to the target insertion position (150ms ease-in-out).
3. Simultaneously animate all offset rows to their final positions.
4. Fade ghost out (100ms).
5. Restore source row opacity.
6. Update the data model (reorder `tickers` vec).
7. Emit `RowDragDrop { from, to }`.
8. Mark config dirty.

### 5.11 External Drop (Future)

When the cursor leaves the grid area during a row drag:
- The ghost continues to follow the cursor across the application window.
- External drop zones (chart panels) highlight when the cursor enters them.
- On release over a chart panel: emit `RowDragExternal { row, target }`, which the
  application handles (e.g., load the dragged symbol into the target chart).
- On release over non-drop-zone: cancel.

This matches the existing `DragTickerState` and `WatchlistDragStart` pattern already
implemented in `app.rs`.

### 5.12 Multi-Row Drag

When multiple rows are selected and the user drags from the drag handle of any
selected row:

1. All selected rows participate in the drag.
2. The ghost shows a **stacked visual**: the top row is rendered in full, with 2
   additional offset rectangles (4px and 8px offset) behind it, plus a count badge.
3. The count badge is a circle (18px diameter, accent color) with white text showing
   the number of dragged rows, positioned at the top-right corner of the ghost.
4. All source rows dim to 30% opacity.
5. Drop inserts all selected rows at the insertion point, preserving their relative
   order.

### 5.13 Messages Emitted

| User Action | Message |
|---|---|
| Mousedown on drag handle | *(internal: enter DragPending)* |
| Mousemove crosses threshold | `RowDragStart { row }` |
| Mousemove during drag | `RowDragMove { y }` |
| Mouseup on valid body position | `RowDragDrop { from, to }` |
| Escape key or invalid release | `RowDragCancel` |
| Release over external target | `RowDragExternal { row, target }` |

### 5.14 Edge Cases

| Scenario | Behavior |
|---|---|
| Grid is sorted (sort_column is Some) | Row drag is **disabled** -- grip icon is hidden, cursor does not change. Manual reorder conflicts with sort order. Display a tooltip "Remove sort to enable reorder" on hover. |
| Grid is filtered | Row drag is disabled (same rationale) |
| Drag handle clicked on unselected row when others are selected | Deselect others, select this row, begin drag of single row |
| Drag handle clicked on selected row (multi-select active) | Begin multi-row drag of all selected rows |
| Only one row in grid | Drag is possible but drop always results in no-op |
| Row being dragged is deleted by external event | Cancel drag immediately, restore state |
| Drag across monitor boundary | Ghost continues to follow cursor (iced handles multi-monitor coordinates) |

---

## 6. Sorting

### 6.1 Trigger

**What starts it**: The user clicks a column header cell body and releases without
crossing the 5px drag threshold.

### 6.2 Sort Cycle

Clicking a column header cycles through three states:

```
  Unsorted ──click──> Ascending ──click──> Descending ──click──> Unsorted
```

**Exception for the Ticker column**: Default direction on first click is Ascending
(A-Z). For all numeric columns (Price, Change%, G.ATR), default direction on first
click is Descending (highest first), because traders typically want to see the
biggest movers at the top.

This is controlled per column via the `GridColumn::default_sort_direction()` trait
method (Phase 1 deliverable — implemented alongside the three-state sort cycle.
See 04-implementation-roadmap.md Phase 1 step 3):

```rust
/// Phase 1 feature: per-column default sort direction.
pub enum DefaultSortDirection {
    /// First click sorts ascending (A-Z, low-high)
    Ascending,
    /// First click sorts descending (Z-A, high-low)
    Descending,
}
```

### 6.3 Sort Direction Indicators

An arrow glyph appears in the header cell next to the column name text:

| State | Indicator | Unicode |
|---|---|---|
| Unsorted | No indicator | (none) |
| Ascending | Upward triangle | `U+25B2` (▲) |
| Descending | Downward triangle | `U+25BC` (▼) |

The indicator appears with a 100ms fade-in animation. It is right-aligned within the
header cell, with 4px padding from the text.

When a column is sorted, the header text may be rendered in a slightly bolder weight
or brighter color to indicate active sort status.

### 6.4 Multi-Column Sort

**Shift+Click** on a column header adds it as a secondary (or tertiary, etc.) sort
key instead of replacing the existing sort:

1. If the column is not yet in the sort stack: append it with default direction.
2. If the column is already in the sort stack: cycle its direction
   (ascending -> descending -> remove from stack).
3. The header shows a small numeric badge (1, 2, 3...) next to the arrow indicating
   sort priority.

**Sort state structure**:

```rust
/// See canonical definition in 03-column-data-model.md §3.
/// Sort state is stored as `Vec<SortSpec>` (index 0 = primary sort).
///
/// **Phase annotation**: This `Vec<SortSpec>` definition is the **Phase 4
/// target shape** (multi-column sort). Phases 0--3 use `Option<SortSpec>`
/// (single-column sort) as defined in 00-architecture.md.
pub type SortState = Vec<SortSpec>;
```

**Click without Shift**: Replaces the entire sort state with a single-column sort.

### 6.5 Specs-Only Sorting

The grid does **not** sort data internally. Following the ImGui/specs-only pattern
established in the research:

1. Grid tracks `SortState` (which columns, which directions).
2. On sort change, grid emits `SortBy { col }` or `SortByMulti { col }`.
3. The **application** receives the message, reads the grid's `SortState`, sorts its
   own `Vec<WatchlistTicker>`, and provides the sorted data back to the grid.
4. Grid re-renders with the new data order.

This keeps the grid component free of data ownership and allows the application to
implement custom sort logic (e.g., always pin favorites to the top, use locale-aware
string comparison, sort by computed values not displayed in any column).

### 6.6 Stable Sort

The application must use a **stable sort** (`slice::sort_by` in Rust, which uses
merge sort) to preserve the relative order of rows with equal sort keys. This is
critical for multi-column sort: a stable sort by the secondary key, then by the
primary key, produces the correct multi-key ordering.

### 6.7 Continuous Sort (Future)

When enabled, the grid re-sorts automatically as data values change (e.g., price
updates from a live feed). This is an opt-in behavior that the application controls:

1. Application receives a data update.
2. Application checks if continuous sort is enabled.
3. If yes: re-sort data, provide to grid.
4. If no: update values in place without reordering.

The grid itself does not trigger continuous sort -- it is entirely an application-side
decision.

### 6.8 Messages Emitted

| User Action | Message |
|---|---|
| Click on sortable header (no Shift) | `SortBy { col }` |
| Shift+Click on sortable header | `SortByMulti { col }` |
| Click on non-sortable header | *(no message)* |

### 6.9 Edge Cases

| Scenario | Behavior |
|---|---|
| Click on header while column reorder drag is active | Not possible -- drag takes priority over click |
| Sort while row drag is active | Not possible -- row drag disables sort clicks |
| Click header of column with `sortable: false` | No sort change, no visual feedback (cursor does not change to pointer) |
| Multi-sort with 4+ columns | Supported but rare; badges show 1, 2, 3, 4 |
| Remove all sort keys (click sorted column a third time, or clear via context menu) | Grid returns to insertion order |
| Data is empty (no rows) | Sort state changes but no visible effect |

---

## 7. Keyboard Navigation

### 7.1 Focus Model

The grid maintains a `focused_row: Option<usize>` that tracks which row has keyboard
focus. Focus is distinct from selection (a row can be focused without being selected
when the user Ctrl+Arrows to move focus without changing selection).

The grid must be focusable within iced's widget tree. When the grid receives iced
keyboard focus (e.g., user clicks on it, or tabs into it), it becomes the active
keyboard target.

### 7.2 Key Bindings

| Key | Modifier | Action |
|---|---|---|
| `Arrow Down` | none | Move focus to next row; select it (deselect others) |
| `Arrow Up` | none | Move focus to previous row; select it (deselect others) |
| `Arrow Down` | Shift | Extend selection range downward from anchor |
| `Arrow Up` | Shift | Extend selection range upward from anchor |
| `Arrow Down` | Ctrl | Move focus down without changing selection |
| `Arrow Up` | Ctrl | Move focus up without changing selection |
| `Space` | Ctrl | Toggle selection of focused row |
| `Enter` | none | Activate the focused row (symbol link to chart) |
| `Delete` | none | Remove focused/selected row(s) from watchlist |
| `Home` | none | Move focus to first row; select it |
| `End` | none | Move focus to last row; select it |
| `Home` | Ctrl | Move focus to first row (no selection change) |
| `End` | Ctrl | Move focus to last row (no selection change) |
| `Home` | Shift | Extend selection from anchor to first row |
| `End` | Shift | Extend selection from anchor to last row |
| `Page Up` | none | Move focus up by `visible_rows - 1`; select it |
| `Page Down` | none | Move focus down by `visible_rows - 1`; select it |
| `Page Up` | Shift | Extend selection upward by one page |
| `Page Down` | Shift | Extend selection downward by one page |
| `Escape` | none | (1) If dragging: cancel drag. (2) If selection exists: deselect all. (3) If context menu open: close it. |
| `Ctrl+A` | none | Select all rows |
| `Tab` | none | Move focus to the next interactive cell within the row (for editable grids). If no editable cells, move to the add-ticker input. |
| `Shift+Tab` | none | Move focus to the previous interactive cell, or back to the grid body |
| `/` or any letter | none | Focus the add-ticker input and begin typing (quick-add, TradingView-style) |

### 7.3 Arrow Key Behavior Detail

**Arrow Down (no modifier)**:
1. If `focused` is `None`: set `focused = 0` (first row).
2. If `focused == last_row`: no-op (do not wrap).
3. Otherwise: `focused = focused + 1`.
4. Clear selection, select `focused`, set `anchor = focused`.
5. If the newly focused row is outside the visible viewport: auto-scroll to reveal it
   (centered in the viewport, or at the edge if near top/bottom).
6. Emit `FocusMove { direction: Down }`.

**Arrow Up (no modifier)**: Mirror of Arrow Down.

**Shift+Arrow Down**:
1. If `anchor` is `None`: set `anchor = focused` (or 0 if both None).
2. Move `focused` down by 1.
3. Recompute selection as `[min(anchor, focused)..=max(anchor, focused)]`.
4. Emit `FocusMove { direction: Down }`.

### 7.4 Enter Key (Activate Row)

Pressing Enter on a focused row triggers **symbol activation**:
1. Emit `ActivateRow { row: focused }`.
2. The application reads the ticker symbol from the row.
3. If symbol linking is active: propagate to all linked chart panels.
4. Visual feedback: the row briefly flashes with an accent color (100ms) to confirm
   activation.

This is the keyboard equivalent of single-clicking a row when symbol linking is
active. It is the primary keyboard workflow for scanning through a watchlist:
Arrow Down, Arrow Down, Enter (loads chart), Arrow Down, Enter, etc.

### 7.5 Delete Key

1. Collect all selected rows.
2. If no rows are selected but a row is focused: treat focused row as the target.
3. Emit `DeleteRows { rows }`.
4. Application removes the rows from the data model.
5. Focus moves to the row that was below the last deleted row (or the new last row
   if the deleted rows included the bottom of the list).
6. If all rows were deleted: `focused = None`, `selected = empty`.

### 7.6 Scroll Behavior During Keyboard Navigation

When keyboard navigation moves focus to a row outside the visible viewport:

| Scenario | Scroll behavior |
|---|---|
| Focused row is 1 row below viewport bottom | Scroll down by 1 row (minimum scroll) |
| Focused row is 1 row above viewport top | Scroll up by 1 row |
| Page Down jumps 20 rows | Scroll so the new focused row is at the top of the viewport |
| Home/End | Scroll to the absolute top/bottom |

The scroll is immediate (no smooth animation) for keyboard navigation to keep the
interface feeling snappy and responsive.

### 7.7 Messages Emitted

| Key Combination | Message |
|---|---|
| Arrow Down/Up (any modifier) | `FocusMove { direction }` |
| Enter | `ActivateRow { row }` |
| Delete | `DeleteRows { rows }` |
| Home/End | `FocusMove { direction }` |
| Page Up/Down | `PageScroll { direction }` |
| Ctrl+A | `SelectAll` |
| Escape | `DeselectAll` or `ColumnReorderCancel` or `RowDragCancel` or `ContextMenuDismiss` (priority order) |
| `/` or letter key | *(internal: focus add-ticker input)* |

### 7.8 Edge Cases

| Scenario | Behavior |
|---|---|
| Grid is empty (no rows) | All navigation keys are no-ops except `/` (which focuses add-ticker input) |
| Grid has 1 row | Arrow Up/Down on that row are no-ops; the row stays selected |
| Focus is on last row, user presses Arrow Down | No-op |
| Focus is on first row, user presses Arrow Up | No-op |
| Grid does not have iced keyboard focus | Keys are not captured; they propagate to the parent widget |
| User is typing in add-ticker input | Arrow keys control the text cursor, not the grid; Escape or Enter returns focus to grid body |
| User presses Delete while add-ticker input is focused | Deletes character in input, not grid rows |

---

## 8. Scrolling

### 8.1 Vertical Scroll (Mouse Wheel)

**Trigger**: Mouse wheel event while the cursor is over the grid body or header area.

**Behavior**:
- Each scroll tick scrolls by `3 * row_height` pixels (matching OS convention for
  3-line scroll).
- Scrolling is **smooth** by default: the scroll target is animated over 100ms
  ease-out, producing a fluid deceleration effect.
- The header row does **not** scroll vertically. It is pinned to the top of the grid.
- The body area scrolls vertically behind the header.

**Row-snapping mode** (optional, disabled by default): When enabled, scroll position
snaps to the nearest row boundary after the scroll animation completes. This ensures
rows are always pixel-aligned, which prevents sub-pixel text rendering artifacts.

### 8.2 Horizontal Scroll

> **Phase note**: Horizontal scrolling (Sections 8.2, 8.3, 8.5) is **Phase 4** functionality, introduced alongside column pinning. During Phases 0-3, all columns are assumed to fit within the panel viewport. If column resize (Section 2.6) causes total width to exceed the viewport, columns should proportionally shrink to fit rather than overflow.

**Trigger**: Shift+Mouse wheel, or horizontal scroll gesture on a trackpad.

**Behavior**:
- Scrolls the header and body together horizontally.
- Pinned columns (drag handle, favorite) do not scroll -- they stay at the left edge.
- Each scroll tick scrolls by 60 logical pixels.

### 8.3 Fixed Header

The header row is always visible at the top of the grid:
- Vertical scroll moves the body; the header stays in place.
- Horizontal scroll moves both header and body together.
- The header casts a subtle 2px shadow onto the body area to visually separate them
  when scrolled.

### 8.4 Scroll Position State

```rust
pub struct ScrollState {
    /// Current vertical scroll offset in logical pixels.
    pub scroll_y: f32,
    /// Current horizontal scroll offset in logical pixels.
    pub scroll_x: f32,
    /// Target vertical scroll offset (for smooth scrolling animation).
    pub target_scroll_y: f32,
    /// Target horizontal scroll offset.
    pub target_scroll_x: f32,
    /// Total content height (all rows * row_height).
    pub content_height: f32,
    /// Total content width (sum of all column widths).
    pub content_width: f32,
    /// Viewport dimensions.
    pub viewport: Size,
}
```

> **Phasing note**: Phase 0 uses a simple `scroll_y: f32` on `GridState` (simple vertical offset, see 00-architecture.md Section 2.1). Phase 3b introduces `VirtualScrollState` (defined in 04-implementation-roadmap.md) which replaces `scroll_y` with a richer structure supporting virtual scrolling, smooth scroll animation, and viewport tracking. The full `ScrollState` shown here is the Phase 3b+ target shape and should be considered illustrative of the final design; the canonical runtime state definition lives in 00-architecture.md.

### 8.5 Scrollbar

**Vertical scrollbar**: A thin (6px wide, expands to 10px on hover) scrollbar track
on the right edge of the grid body. The thumb size is proportional to
`viewport_height / content_height`. The scrollbar is always visible when content
overflows; it does not auto-hide.

**Horizontal scrollbar**: Same treatment, on the bottom edge. Only visible when
content width exceeds viewport width.

**Interaction**: Click-and-drag the thumb to scroll. Click on the track (outside the
thumb) to page-scroll in that direction.

### 8.6 Programmatic Scroll

The grid provides a `ScrollToRow { row }` message that scrolls to make the specified
row visible:

1. If the row is above the viewport: scroll up so the row is at the top.
2. If the row is below the viewport: scroll down so the row is at the bottom.
3. If the row is already visible: no scroll.

This is used by keyboard navigation, search results, and programmatic focus.

### 8.7 Messages Emitted

| User Action | Message |
|---|---|
| Mouse wheel (vertical) | `ScrollVertical { delta }` |
| Shift+Mouse wheel (horizontal) | `ScrollHorizontal { delta }` |
| Scrollbar drag | `ScrollVertical { delta }` or `ScrollHorizontal { delta }` |
| Programmatic | `ScrollToRow { row }` |

### 8.8 Edge Cases

| Scenario | Behavior |
|---|---|
| Content fits in viewport (no overflow) | No scrollbar, wheel events are no-ops |
| Scroll past the end | Clamp to `[0, content_height - viewport_height]` |
| Window resize shrinks viewport | Content that was visible may now be below fold; scrollbar recalculates |
| Scroll during row drag | Auto-scroll zones near edges handle this (Section 5.8) |
| Scroll while column resize is active | Horizontal scroll is blocked during column resize |
| Very fast scroll (high-DPI trackpad) | Accumulate deltas per frame; apply once per render |

---

## 9. Context Menu

### 9.1 Row Context Menu

**Trigger**: Right-click on a row in the grid body.

**Behavior**:
1. If the right-clicked row is not selected: select it (deselect others), then open menu.
2. If the right-clicked row is selected (possibly among a multi-selection): open menu
   for the entire selection.
3. The context menu appears at the cursor position.
4. The menu is rendered in Layer 6 (the topmost overlay layer, shared with drag ghost and drop indicators).

**Menu items for a watchlist row**:

| Item | Action | Shortcut hint |
|---|---|---|
| Load in Chart | Emit `SymbolActivated` for the ticker | Enter |
| Add to Watchlist... | Open sub-menu of available watchlists | |
| Remove from Watchlist | Delete this ticker row | Delete |
| Set Alert... | Open alert configuration dialog (future) | |
| Toggle Favorite | Flip the favorite star | |
| Copy Symbol | Copy ticker to clipboard | Ctrl+C |
| Separator | --- | |
| View Details | Open detailed view for this ticker (future) | |

### 9.2 Header Context Menu

**Trigger**: Right-click on a column header.

**Menu items**:

| Item | Action |
|---|---|
| Sort Ascending | Set column to ascending sort |
| Sort Descending | Set column to descending sort |
| Clear Sort | Remove sort on this column |
| Separator | --- |
| Auto-fit Column Width | Auto-size this column to content |
| Auto-fit All Columns | Auto-size all columns |
| Reset Column Widths | Restore all columns to default widths |
| Separator | --- |
| Hide Column | Hide this column (remove from display_order) |
| Show Columns... | Open a checkbox list of all columns to toggle visibility |

### 9.3 Dismissal

The context menu is dismissed by:
- Clicking outside the menu
- Pressing Escape
- Selecting a menu item
- Starting any drag operation
- Scrolling the grid

### 9.4 State

```rust
pub enum ContextMenuState {
    Closed,
    RowMenu {
        position: Point,
        target_rows: Vec<usize>,
    },
    HeaderMenu {
        position: Point,
        target_col: usize,
    },
}
```

### 9.5 Messages Emitted

| User Action | Message |
|---|---|
| Right-click on row | `ContextMenuOpen { row, position }` |
| Right-click on header | `ContextMenuHeaderOpen { col, position }` |
| Select menu item | `ContextMenuAction { action }` |
| Dismiss (click outside, Escape) | `ContextMenuDismiss` |

### 9.6 Edge Cases

| Scenario | Behavior |
|---|---|
| Right-click on empty area (no row) | No context menu (or a grid-level menu with "Add row" if applicable) |
| Right-click on drag handle column | Open row context menu (the handle itself has no special context menu) |
| Context menu open, user left-clicks elsewhere | Close menu, then process the left-click normally |
| Context menu open, user right-clicks on a different row | Close old menu, select new row, open new menu |
| Menu extends beyond grid bounds | Menu is rendered in an overlay layer that can extend beyond the grid widget's clip rect |

---

## 10. Cell Interactions

### 10.1 Interactive Cell Widgets

Cells can host interactive elements that consume clicks instead of propagating them
to row selection. The cell content is determined by the column's `GridColumn::cell()`
method, which returns any iced `Element<'a, M>`. Common interactive cell types include:

- **Plain text** — display-only text (`Text` widget)
- **Clickable button** — emits an app-level message on click (`Button` widget)
- **Toggle switch** — boolean on/off (`Toggler` or styled `Button`)
- **Star/favorite** — toggle with icon (`Button` with conditional styling)
- **Icon button** — e.g., delete X (`Button` with icon content)

There is no separate enum for cell content types — the `cell()` return type
(`Element<'a, M>`) is the only abstraction needed.

### 10.2 Click Propagation Rules

When the user clicks within a cell:

1. **Hit-test the cell content**: Check if the click is on an interactive widget
   (button, toggle, star, icon button).
2. **If interactive widget**: The widget consumes the click. The cell's own `Element<M>`
   emits the application's message type directly (e.g., `Message::ToggleFavorite(symbol)`),
   **not** a `GridMessage` variant. The grid is transparent to cell widget messages
   (see 00-architecture.md §5.3). Row selection does **not** change.
3. **If plain text cell**: The click propagates to row selection (Section 4).

This prevents the common UX error where clicking a "delete" button also selects the
row, or clicking a favorite star changes the selected row.

### 10.3 Hover Effects

| Cell type | Hover behavior |
|---|---|
| Plain text | No cell-level hover (row hover handles it) |
| Button | Button background lightens; cursor becomes pointer |
| Toggle | Toggle track highlights; cursor becomes pointer |
| Star | Star fills with a preview color; cursor becomes pointer |
| Icon button | Icon brightens; cursor becomes pointer |

### 10.4 Cursor Changes Summary (All Interactions)

| Context | Cursor |
|---|---|
| Default (body area, non-interactive cell) | Default arrow |
| Over row in body | Default arrow (row hover handled via highlight) |
| Over interactive cell widget | Pointer (hand) |
| Over column resize handle | `ew-resize` (horizontal double-arrow) |
| Over column header body (sortable) | Pointer (hand) |
| Over column header body (non-sortable, reorderable) | `grab` (open hand) |
| During column reorder drag | `grabbing` (closed hand) |
| Over drag handle column | `grab` (open hand) |
| During row drag | `grabbing` (closed hand) |
| Over drag ghost on invalid drop zone | `not-allowed` (circle with line) |
| During column resize | `ew-resize` |
| Over scrollbar | Default arrow |
| During scrollbar drag | Default arrow |

### 10.5 Messages Emitted

Cell widget interactions bypass `GridMessage` entirely. The cell's `Element<M>` emits
the application's own message type `M` directly:

| User Action | Message (application type `M`, not `GridMessage`) |
|---|---|
| Click on button cell | e.g., `Message::WatchlistRemoveTicker(wl_id, symbol)` |
| Click on toggle cell | e.g., `Message::ToggleFavorite(wl_id, symbol)` |
| Click on star cell | e.g., `Message::ToggleFavorite(wl_id, symbol)` |
| Click on icon button cell | e.g., `Message::WatchlistRemoveTicker(wl_id, symbol)` |

The specific message variants are defined by the column's `cell()` implementation,
not by the grid. See 00-architecture.md §5.3.

---

## 11. Trading-Specific Interactions

### 11.1 Flash-on-Tick

When a cell's data value changes (e.g., a price update from a live feed):

**Trigger**: Application provides a new value for a cell that differs from the
previous value.

**Visual feedback**:
1. **Immediate**: Cell background changes to the flash color at peak alpha.
   - Uptick (value increased): green flash (`rgba(0, 200, 100, 0.3)`)
   - Downtick (value decreased): red flash (`rgba(255, 60, 60, 0.3)`)
2. **Fade**: Flash color fades from peak to 0.0 over 300ms with ease-out cubic easing.
3. **Total duration**: 300ms from tick to fully faded.

This matches the 300ms ease-out specified in 02-rendering.md §4 and is consistent
with professional trading platforms (subtle, not distracting).

**Interruption**: If a new tick arrives during the fade:
- The flash **resets** immediately to the new direction's color at peak alpha.
- The fade timer restarts.
- There is no blending between old and new flash colors.

**Scope**: Only the specific cell that changed flashes, not the entire row.

**Implementation** (container background modulation, see 02-rendering.md §4): The flash
color is blended with the cell's base background color in Rust during `view()`. Each
flashing cell's `Container` style returns a background that interpolates from the flash
color at peak alpha to the base color over the decay period:

```rust
let alpha = (1.0 - ease_out_cubic(t)) * peak_alpha;
let bg = lerp_color(base_color, flash_color, alpha);
```

**State per cell**:

```rust
pub struct CellFlashState {
    pub flash_start: Option<Instant>,
    pub direction: FlashDirection,  // Up or Down
}
```

### 11.2 Symbol Linking

**Trigger**: User clicks a row (single select) or presses Enter on a focused row,
and the grid's `symbol_link` mode is not `Unlinked`.

**Behavior**:
1. Grid emits `SymbolActivated { symbol }`.
2. Application receives the message.
3. Application finds all chart panels with the same `LinkMode` color group.
4. Application calls `load_symbol_for_chart(chart_id, symbol)` on each linked chart.
5. Linked charts update their display to show the new symbol.

**Visual indicator**: The grid's title bar shows a colored circle (the link group
color) indicating which group it belongs to. This uses the existing `LinkMode` and
`LinkColor` system from `link.rs`.

**Link groups** (from the existing implementation):

| LinkMode | Color | Meaning |
|---|---|---|
| `Unlinked` | Gray | No linking |
| `Color(LinkColor::Red)` | Red | Link group 1 |
| `Color(LinkColor::Green)` | Green | Link group 2 |
| `Color(LinkColor::Blue)` | Blue | Link group 3 |
| `Color(LinkColor::Yellow)` | Yellow | Link group 4 |

### 11.3 Quick-Add Symbol

**Trigger**: User presses `/` or begins typing a letter key while the grid body has
keyboard focus.

**Behavior**:
1. Focus jumps to the add-ticker input field at the bottom of the grid.
2. If triggered by a letter key: that letter is inserted as the first character.
3. If triggered by `/`: the input is focused but empty.
4. User types a ticker symbol.
5. **Enter**: Add the ticker to the watchlist, clear the input, return focus to the
   grid body (focus the newly added row).
6. **Escape**: Clear the input, return focus to the grid body (focus unchanged).

**Auto-complete** (future): As the user types, a dropdown shows matching symbols
from a symbol database.

### 11.4 Favorite/Star Toggle

**Trigger**: Click on the star icon in the favorite column.

**Behavior**:
1. Toggle the `favorite: bool` field on the ticker.
2. The star icon changes between filled (gold) and outline (gray).
3. If the grid is sorted, favorited rows can optionally be pinned to the top
   (application-level sort logic).
4. The star cell's `Element<M>` emits an application message directly
   (e.g., `Message::ToggleFavorite(wl_id, symbol, new_state)`).
5. Mark config dirty.

**Visual**:
- Unfavorited: Outline star, gray color, subtle opacity.
- Favorited: Filled star, gold color (`#FFD700`), full opacity.
- Hover (unfavorited): Outline star fills with a preview gold at 50% opacity.
- Hover (favorited): Star slightly brightens.

### 11.5 Conditional Cell Coloring

Price and change cells use semantic coloring:

| Value | Text color | Background |
|---|---|---|
| Positive change | Green (`#00C805`) | None (or very subtle green tint) |
| Negative change | Red (`#FF4444`) | None (or very subtle red tint) |
| Zero change | Default text color | None |
| Price (absolute) | Default text color | None |

The application controls coloring via a `CellStyle` callback in the column definition:

```rust
pub type CellStyleFn = Box<dyn Fn(&RowData, usize) -> CellStyle>;

pub struct CellStyle {
    pub text_color: Option<Color>,
    pub background: Option<Color>,
    pub font_weight: Option<FontWeight>,
}
```

### 11.6 Messages Emitted (Trading-Specific)

| User Action / Event | Message | Type |
|---|---|---|
| Click row / Enter on row (linked) | `SymbolActivated { symbol }` | `GridMessage` |
| Data value changes | `FlashCell { column: ColumnId, row_key: RowKey, direction: FlashDirection }` | `GridMessage` |
| Star toggle | e.g., `Message::ToggleFavorite(...)` | Application `M` (not `GridMessage`) |
| Quick-add submit | *(handled via existing WatchlistAddTicker flow)* | Application `M` |

---

## 12. Interaction Conflict Resolution

Multiple interactions can potentially compete for the same input event. This section
defines the resolution rules.

### 12.1 Priority Matrix

When a mousedown or mousemove event occurs, the system processes it through these
checks in order. The first matching handler consumes the event.

```
1. Context menu open?
   -> Click outside: dismiss menu, consume event
   -> Click on menu item: execute action, consume event

2. Active drag in progress? (column reorder or row drag)
   -> Mousemove: update drag state, consume event
   -> Mouseup: complete/cancel drag, consume event
   -> Escape: cancel drag, consume event

3. Active column resize?
   -> Mousemove: update resize, consume event
   -> Mouseup: end resize, consume event

4. Hit on resize handle?
   -> Mousedown: begin resize, consume event
   -> Double-click: auto-fit, consume event

5. Hit on interactive cell widget?
   -> Click: activate widget, consume event (no row selection)

6. Hit on column header body?
   -> Mousedown: enter DragPending for reorder, consume event
   -> (Mouseup without threshold -> sort click)

7. Hit on drag handle cell?
   -> Mousedown: enter DragPending for row drag, consume event

8. Hit on body row?
   -> Click: row selection (with modifier logic), consume event
   -> Right-click: context menu, consume event

9. Hit on empty area?
   -> Click: deselect all, consume event
   -> Right-click: no-op (or grid-level menu)

10. Hit on add-ticker input?
    -> Standard text input handling
```

### 12.2 Specific Conflict Resolutions

| Conflict | Resolution |
|---|---|
| Column resize handle overlaps header cell body | Resize handle has higher priority (8px zone takes precedence) |
| Click on header could be sort or reorder | DragPending handles both: mouseup < 5px = sort, >= 5px = reorder |
| Row click could be selection or drag | Drag only starts from the dedicated drag handle column, not from the row body |
| Star click in a selected row | Star widget consumes the click; selection does not change |
| Right-click during a left-click drag | Cancel the drag, then open context menu |
| Scroll wheel during drag | For row drag: triggers auto-scroll. For column drag: ignored. |
| Escape key during drag with selection | Escape cancels the drag first; a second Escape clears selection |
| Keyboard event while context menu is open | Escape closes menu; all other keys are ignored by the grid |
| Tab key on grid vs. add-ticker input | Tab from last cell in grid moves focus to add-ticker input; Tab from input moves back to grid |

### 12.3 Mouse Capture

When a drag or resize operation begins (transitions from Pending to Active), the grid
captures mouse events. This means:
- Mousemove events are received even if the cursor leaves the grid widget bounds.
- Mouseup events are received even if the cursor is over a different widget.
- Other widgets do not receive mouse events during capture.

This is implemented using iced's `mouse::Interaction` and event capture mechanism.
The capture is released when the operation completes or is cancelled.

---

## Appendix A: State Structure Reference

> **Canonical source**: The runtime `GridState` definition lives in 00-architecture.md §2.1.
> This appendix summarizes the interaction-relevant subset for quick reference.
> When in conflict, 00-architecture.md takes precedence.

The grid's runtime state uses a **unified `ActiveInteraction` enum** for transient
interaction state. Only one drag/resize interaction can be active at a time — the
enum makes this a compile-time guarantee (see 00-architecture.md §2.1).

```rust
/// Interaction-relevant fields on GridState (see 00-architecture.md §2.1 for full definition).
pub struct GridState {
    // Persistent state
    pub column_order: Vec<ColumnId>,
    pub column_widths: HashMap<ColumnId, f32>,
    pub sort: Option<SortSpec>,
    pub selection: SelectionState,
    pub scroll_y: f32,

    // Transient interaction state (unified enum — at most one active at a time)
    pub interaction: ActiveInteraction,
}
```

Supporting types used in interaction sections above:

```rust
pub struct ResizeState {
    pub column_id: ColumnId,
    pub start_x: Option<f32>,
    pub start_width: f32,
}

pub struct ColumnDragState {
    pub source_id: ColumnId,
    pub start_x: f32,
    pub current_x: f32,
    pub drop_target_index: Option<usize>,
}

pub struct RowDragState {
    pub source_index: usize,
    pub start_y: f32,
    pub current_y: f32,
    pub drop_target_index: Option<usize>,
}

/// See 00-architecture.md §6.2 for the full SelectionState definition
/// including mode, focused, anchor, and selected fields.
```

## Appendix B: Existing Code Integration Points

The current codebase already has a basic watchlist with column resize, sort, and
ticker drag. The grid component specified in this document is the next-generation
replacement. Key integration points (using canonical names from 00-architecture.md):

| Existing code | New grid equivalent |
|---|---|
| `WatchlistPanel.column_widths: [f32; 7]` | `GridState.column_widths: HashMap<ColumnId, f32>` + `GridState.column_order: Vec<ColumnId>` |
| `WatchlistPanel.sort_column / sort_direction` | `GridState.sort: Option<SortSpec>` |
| `WatchlistPanel.selected_symbol` | `GridState.selection: SelectionState` |
| `MidasApp.resizing_column: Option<(WatchlistId, usize, f32, f32)>` | `GridState.interaction: ActiveInteraction::Resize(ResizeState)` |
| `MidasApp.dragging_ticker: Option<DragTickerState>` | `GridState.interaction: ActiveInteraction::RowDrag(RowDragState)` |
| Inline sort logic in `views.rs` (no dedicated message variant) | `GridMessage::SortToggled(ColumnId)` |
| `Message::WatchlistColumnResizeStart/Resizing/End` | `GridMessage::ResizeStarted/Resizing/ResizeEnded` |
| `Message::WatchlistDragStart/Cancel` | `GridMessage::RowDragStarted/RowDragCancelled/RowDragEnded` |
| `Message::WatchlistTickerSelected` | `GridMessage::RowSelected(usize)` + `SymbolActivated` |
