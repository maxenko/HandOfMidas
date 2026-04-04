# Drag & Drop Patterns for Grid Components

Research on drag-and-drop visual patterns across frameworks, with focus on custom drag visuals applicable to a Rust/wgpu-rendered grid.

---

## Table of Contents

1. [Unreal UMG Drag & Drop](#1-unreal-umg-drag--drop)
2. [HTML5 Drag and Drop](#2-html5-drag-and-drop)
3. [React DnD / dnd-kit](#3-react-dnd--dnd-kit)
4. [Row Drag Patterns](#4-row-drag-patterns)
5. [Column Header Drag Patterns](#5-column-header-drag-patterns)
6. [Drag Visual Design](#6-drag-visual-design)
7. [Native Desktop Drag Patterns](#7-native-desktop-drag-patterns)
8. [Implementation in GPU-Rendered UI](#8-implementation-in-gpu-rendered-ui)

---

## 1. Unreal UMG Drag & Drop

UMG (Unreal Motion Graphics) provides the gold standard for custom drag visuals in a native GPU-rendered context. The entire system is built around the concept of a **DragDropOperation** object that carries both data and visual representation.

### 1.1 UDragDropOperation

The core class `UDragDropOperation` is a specialized UObject that encapsulates everything about a drag operation. Key properties:

| Property | Type | Description |
|---|---|---|
| `Tag` | FString | A simple string tag for identifying the operation type |
| `Payload` | UObject* | Arbitrary data payload carried with the drag |
| `DefaultDragVisual` | UUserWidget* | The widget displayed under the cursor during drag |
| `Pivot` | EDragPivot | Where the drag visual appears relative to the cursor |
| `Offset` | FVector2D | Percentage offset (-1..+1) from the Pivot location |

### 1.2 EDragPivot Enum

The Pivot controls visual placement relative to the pointer:

```
MouseDown       -- Appears where the mouse pressed (most natural)
TopLeft         -- Upper-left corner of the visual aligns to cursor
TopCenter       -- Top-center aligns to cursor
TopRight        -- Upper-right corner aligns to cursor
CenterLeft      -- Left-center aligns to cursor
CenterCenter    -- Center of the visual aligns to cursor
CenterRight     -- Right-center aligns to cursor
BottomLeft      -- Lower-left corner aligns to cursor
BottomCenter    -- Bottom-center aligns to cursor
BottomRight     -- Lower-right corner aligns to cursor
```

The `Offset` property provides additional fine-tuning as a percentage of the drag visual's size. An offset of `(0.5, 0.5)` with `CenterCenter` pivot would shift the visual half its own width and height away from the cursor.

### 1.3 Creating the Drag Visual (OnDragDetected)

The drag flow begins when UMG detects a drag gesture:

1. **OnDragDetected** fires on the source widget.
2. Inside that handler, you create a **DragWidget** -- a separate UUserWidget designed specifically to be the floating visual. This is typically a simplified or styled version of the thing being dragged (e.g., a row snapshot, a card, an icon).
3. You instantiate a `UDragDropOperation` subclass via `CreateDragDropOperation`.
4. You assign the DragWidget to `DefaultDragVisual`.
5. You set `Pivot` and `Offset` to control cursor-relative placement.
6. Return the operation from OnDragDetected.

```cpp
// C++ pattern
FReply UMyWidget::OnDragDetected(const FGeometry& MyGeometry,
                                  const FPointerEvent& MouseEvent)
{
    UMyDragVisual* Visual = CreateWidget<UMyDragVisual>(this);
    Visual->SetRowData(RowData); // Configure the visual

    UDragDropOperation* Op = NewObject<UDragDropOperation>();
    Op->DefaultDragVisual = Visual;
    Op->Pivot = EDragPivot::MouseDown;
    Op->Offset = FVector2D(0.f, 0.f);
    Op->Payload = RowData;

    return FReply::Handled().BeginDragDrop(Op);
}
```

### 1.4 The DragWidget (Custom Drag Visual)

The DragWidget is a full UMG widget -- it can contain images, text, progress bars, anything renderable in UMG. Common patterns:

- **Snapshot approach**: Render the source item into a texture, display it in a simple Image widget at reduced opacity (0.7-0.8 alpha).
- **Simplified representation**: A compact version of the source (e.g., just the symbol name and icon for a watchlist row, omitting columns like price/volume).
- **Styled ghost**: Same layout as the source but with a drop shadow, slight scale-up (1.05x), and reduced opacity.

The DragWidget is rendered by the Slate compositor in a **top-level overlay layer** that sits above all other UMG content. UMG handles the per-frame positioning, updating the widget's screen position to follow the cursor every tick.

### 1.5 OnDrop and OnDragCancelled

- **OnDrop(DragDropEvent)**: Fires on the widget that receives the drop. Receives the full `UDragDropOperation` including `Payload`. The receiving widget decides what to do (reorder, move, link, etc.).
- **OnDragCancelled(DragDropEvent)**: Fires if the drag is released over an invalid target. Used for "snap-back" animations.
- **OnDragEnter / OnDragLeave**: Fire as the cursor enters/exits potential drop targets. Used to show hover highlights, insertion indicators, etc.

### 1.6 Key Takeaways for GPU-Rendered UI

- The DragWidget is a **completely separate widget instance**, not the original item. This means the original stays in place (possibly dimmed) while a custom visual follows the cursor.
- The visual is rendered in a **dedicated overlay layer** above all normal UI content.
- **Pivot + Offset** give precise control over cursor-to-visual alignment without complex coordinate math.
- The drag visual can be **any arbitrary widget tree**, not just a static image.

---

## 2. HTML5 Drag and Drop

The native browser Drag and Drop API is the most widely deployed DnD system but has significant limitations for custom visuals.

### 2.1 Core API

The native DnD flow involves these events:

| Event | Target | Purpose |
|---|---|---|
| `dragstart` | Source element | Initialize drag, set data and image |
| `drag` | Source element | Fires continuously during drag |
| `dragenter` | Drop target | Cursor enters a drop zone |
| `dragover` | Drop target | Cursor moves within drop zone (must preventDefault to allow drop) |
| `dragleave` | Drop target | Cursor exits drop zone |
| `drop` | Drop target | Element is dropped |
| `dragend` | Source element | Drag operation complete (regardless of outcome) |

### 2.2 DataTransfer and setDragImage()

```javascript
element.addEventListener('dragstart', (e) => {
    // Set transfer data
    e.dataTransfer.setData('text/plain', rowId);
    e.dataTransfer.effectAllowed = 'move';

    // Set custom drag image
    const ghost = createGhostElement(row);
    document.body.appendChild(ghost);
    e.dataTransfer.setDragImage(ghost, offsetX, offsetY);

    // Clean up the off-screen element after a frame
    requestAnimationFrame(() => ghost.remove());
});
```

**setDragImage(element, xOffset, yOffset)**:
- `element`: An `<img>`, `<canvas>`, or any visible DOM element.
- `xOffset`: Horizontal pixel offset from cursor to image origin.
- `yOffset`: Vertical pixel offset from cursor to image origin.
- Must be called in `dragstart` handler only.

### 2.3 Limitations

| Limitation | Impact |
|---|---|
| **Static preview** | The drag image is captured once at `dragstart` and cannot change during the drag. No live updating. |
| **Timing constraint** | `setDragImage()` only works in `dragstart`. The data store is read-only in all other drag events. |
| **Element must be visible** | The element used for `setDragImage` must be in the DOM and visible (not `display:none` or `visibility:hidden`). Common workaround: position it off-screen (`left: -9999px`). |
| **No opacity control** | You cannot control the opacity of the native drag image after creation. Browsers apply their own semi-transparency (typically ~70% opacity). |
| **No animation** | The drag image cannot be animated -- no scale transitions, no rotation, no shadow changes. |
| **Cross-browser rendering** | Different browsers render the drag image at different DPIs and may clip large elements. |
| **No cursor customization** | The cursor during native drag is OS-controlled (copy/move/no-drop). Limited to `effectAllowed` / `dropEffect`. |
| **No rich drop zones** | `dragover` only gives cursor position, not detailed geometry of the dragged item relative to the drop target. |

### 2.4 Workaround Patterns

Because of these limitations, many web frameworks bypass the native API entirely:

- **Mouse-event-based DnD**: Track `mousedown`, `mousemove`, `mouseup` manually. Render a custom floating element positioned via CSS `transform`. This is what dnd-kit, react-beautiful-dnd, and Atlassian's pragmatic-drag-and-drop all do for their overlay visuals.
- **Hybrid approach**: Use native DnD for the data transfer semantics (`dragstart`/`drop`) but render a custom overlay element alongside the invisible native drag image (set drag image to a transparent 1x1 pixel).

---

## 3. React DnD / dnd-kit

### 3.1 react-dnd

**Architecture**: Backend-agnostic library. The HTML5 backend uses native DnD; the Touch backend uses pointer events.

**useDragLayer Hook**: The key to custom drag visuals in react-dnd.

```jsx
function CustomDragLayer() {
    const { item, itemType, isDragging, currentOffset } = useDragLayer(
        (monitor) => ({
            item: monitor.getItem(),
            itemType: monitor.getItemType(),
            isDragging: monitor.isDragging(),
            currentOffset: monitor.getSourceClientOffset(),
        })
    );

    if (!isDragging) return null;

    return (
        <div style={{
            position: 'fixed',
            pointerEvents: 'none',
            zIndex: 9999,
            left: 0, top: 0,
            transform: `translate(${currentOffset.x}px, ${currentOffset.y}px)`,
        }}>
            <RowDragPreview item={item} />
        </div>
    );
}
```

**DragLayerMonitor** provides:
- `getItemType()` -- string/symbol identifying what is being dragged
- `getItem()` -- the drag payload object
- `isDragging()` -- boolean
- `getInitialClientOffset()` -- cursor position at drag start
- `getSourceClientOffset()` -- projected position of the drag source DOM node
- `getDifferenceFromInitialOffset()` -- delta from start position
- `getClientOffset()` -- current cursor position

### 3.2 dnd-kit

Modern, lightweight, zero-dependency DnD toolkit for React. Preferred over react-dnd for new projects.

**Core Architecture**:
- `DndContext` -- provider that wraps the DnD-enabled tree
- `useDraggable` -- hook for drag sources
- `useDroppable` -- hook for drop targets
- `DragOverlay` -- dedicated component for custom drag visuals
- Sensors, Modifiers, Collision Detection -- composable extension points

#### DragOverlay Component

```jsx
<DndContext onDragStart={handleDragStart} onDragEnd={handleDragEnd}>
    {items.map(item => <SortableItem key={item.id} item={item} />)}

    <DragOverlay dropAnimation={customAnimation}>
        {activeItem ? <RowPreview item={activeItem} /> : null}
    </DragOverlay>
</DndContext>
```

Key properties of DragOverlay:

| Property | Type | Description |
|---|---|---|
| `dropAnimation` | DropAnimation or null | Animation config for the drop (duration, easing, keyframes) |
| `modifiers` | Modifier[] | Transform modifiers (restrict to axis, snap to grid, etc.) |
| `style` | CSSProperties | Inline styles for the overlay container |
| `transition` | string | CSS transition for overlay movement |
| `zIndex` | number | Stack order (default: very high) |
| `wrapperElement` | string | The DOM element type for the wrapper |

**Critical rule**: Components inside `DragOverlay` must NOT use `useDraggable`. They are purely visual.

**Rendering approach**: `DragOverlay` is removed from normal document flow and positioned relative to the viewport using `position: fixed`. It can optionally be rendered through a React portal to ensure it appears above all other content.

#### Sensors

Sensors abstract input methods:

| Sensor | Activation | Use Case |
|---|---|---|
| `PointerSensor` | Pointer events | General purpose, works with mouse and touch |
| `MouseSensor` | Mouse events only | Desktop-specific |
| `TouchSensor` | Touch events only | Mobile-specific |
| `KeyboardSensor` | Keyboard events | Accessibility, arrow-key reordering |

Sensors support **activation constraints** such as distance threshold (pixels the pointer must move before drag starts) and delay (ms to hold before drag activates). These prevent accidental drags on click.

#### Collision Detection Algorithms

| Algorithm | Strategy |
|---|---|
| `rectIntersection` | Standard bounding-box overlap |
| `closestCenter` | Closest distance from drag center to droppable center |
| `closestCorners` | Closest distance from drag corners to droppable corners |
| `pointerWithin` | Whether the pointer is within a droppable bounding rect |
| Custom | You can write your own collision detection function |

#### Modifiers

Transform the coordinates of the drag overlay:

- `restrictToWindowEdges` -- keep overlay within viewport
- `restrictToParentElement` -- constrain to a parent container
- `restrictToVerticalAxis` / `restrictToHorizontalAxis` -- lock to one axis
- `snapCenterToCursor` -- center the overlay on the cursor
- Custom modifiers for grid snapping, boundary clamping, etc.

### 3.3 Atlassian Pragmatic Drag and Drop

Atlassian's library takes a different philosophy: framework-agnostic, small core, opinionated visual guidelines.

**Visual guidelines (design system)**:
- **Drop indicator line**: A colored line (typically blue) between items showing where the drop will land. Terminal bleeds 4px outward on the left for visibility.
- **Drag preview**: The original item dims to ~40% opacity; a ghost preview follows the cursor.
- **Combine indicator**: A border/outline around a target item indicates "combine with this" rather than "insert before/after."
- **Edge detection**: Hitbox is divided into regions (top half = "insert before", bottom half = "insert after") for reorder operations within lists.

---

## 4. Row Drag Patterns

### 4.1 Drag Handle Column

The most common pattern uses a dedicated "grip" column at the far left of each row.

**Visual design**:
- Icon: Six-dot grip pattern (two columns of three dots, `⠿` or similar)
- Cursor: `grab` on hover, `grabbing` once pressed
- Width: Typically 24-40px, non-resizable
- Hover state: Icon may become more prominent (higher contrast) on row hover

**Behavior**:
- Drag initiates only from the handle, not the full row
- Prevents conflict with text selection, cell editing, and link clicking in other columns
- The handle column is typically non-sortable and pinned to the left edge

### 4.2 Full-Row Drag

Some interfaces allow dragging from anywhere on the row.

**When appropriate**:
- Simple lists without editable cells
- Card-based layouts
- When the row has no interactive content

**Activation guard**: Use a distance threshold (5-8px) or hold delay (150-200ms) to distinguish click from drag intent.

### 4.3 Visual Feedback During Row Drag

#### Ghost Row (Drag Preview)

The floating element that follows the cursor:

| Aspect | Recommended Approach |
|---|---|
| **Content** | Full row content, or a simplified version (symbol + key columns only) |
| **Opacity** | 0.7-0.85 alpha on the ghost |
| **Shadow** | `box-shadow: 0 4px 12px rgba(0,0,0,0.15)` or equivalent elevation |
| **Scale** | Optional slight scale-up (1.02-1.05x) on pickup for "lift" effect |
| **Border** | Optional accent-colored border (1-2px) to distinguish from in-place content |
| **Width** | Same as the row, or constrained to a max width |
| **Background** | Solid (not transparent) so text remains readable over underlying content |

#### Source Row (Where It Was)

The original row position while dragging:

| Pattern | Description |
|---|---|
| **Dimmed** | Row stays in place at 30-40% opacity |
| **Placeholder gap** | Row is replaced by an empty space of the same height, showing where it came from |
| **Collapsed** | Row height animates to 0, other rows fill the gap |
| **Highlighted** | Row shows a colored background (light blue/gray) indicating "this is being moved" |

#### Drop Target Indicators

Visual cues on the destination:

| Indicator | Description |
|---|---|
| **Insertion line** | Horizontal line (2-3px) between rows at the drop position. Most common and clearest. |
| **Gap/spacer** | An animated gap opens between rows at the insertion point (more immersive, more expensive to render) |
| **Row highlight** | The row above/below the insertion point gets a colored top/bottom border |
| **Background highlight** | The entire drop zone area gets a subtle background tint |

### 4.4 Animated Reorder

The premium UX pattern where rows smoothly slide out of the way as the drag moves:

**How it works**:
1. As the ghost row moves over a new position, calculate the insertion index.
2. Rows above the new position that were below the original position slide **up** by one row height.
3. Rows below the new position that were above the original position slide **down** by one row height.
4. Use CSS `transform: translateY()` with `transition: transform 200ms ease` (or equivalent GPU-rendered animation).

**Performance considerations**:
- Only animate rows that change position (not all rows).
- Use `transform` only (not `top`/`margin`) to stay on the GPU compositor.
- Batch position updates to avoid layout thrashing.
- For large grids with virtualization, only animate visible rows.

**Timing**: 150-250ms transition duration. Shorter feels snappy, longer feels smoother. 200ms is the sweet spot.

---

## 5. Column Header Drag Patterns

### 5.1 Initiating Column Header Drag

**Activation**:
- User presses on a column header and moves the pointer 5+ pixels.
- Must distinguish from click-to-sort and click-to-select.
- A distance threshold (5-8px) prevents accidental drags during sort clicks.

**Cursor**: `grab` on header hover, `grabbing` once drag starts.

### 5.2 Floating Header Ghost

When the drag starts:

1. **Capture the header appearance**: Create a visual snapshot of the column header.
2. **Render a floating ghost**: Position it under the cursor, semi-transparent (0.7-0.8 alpha).
3. **Add elevation**: Drop shadow to indicate it is "lifted" above the header bar.
4. **Constrain vertically**: The ghost should only move horizontally (lock Y axis to the header row).
5. **Show the source column**: Dim the original header or replace it with a placeholder (dashed outline, gray background).

```
Before drag:
  [Symbol] [Price] [Volume] [Change]

During drag (dragging "Price"):
  [Symbol] [  ?  ] [Volume] [Change]
                           ^
       Floating ghost: [Price] follows cursor horizontally
       Drop indicator line between Volume and Change
```

### 5.3 Drop Indicators

| Indicator Type | Visual Description |
|---|---|
| **Vertical line** | A 2-3px colored vertical line between adjacent headers, extending the full height of the header row (or the full grid height for maximum clarity). |
| **Gap** | Adjacent headers slide apart to create a gap the width of the dragged column, showing where it will be inserted. |
| **Arrow/caret** | A small downward-pointing triangle above or below the header row at the insertion point. |
| **Highlight zone** | The space between two headers gets a colored background highlight. |

### 5.4 Snap-to-Position Animation on Drop

When the user releases:

1. **Animate the ghost** to the target position (the gap or indicator location) over 150-200ms with ease-out easing.
2. **Simultaneously animate other headers** sliding left or right to their new positions.
3. **Fade out the ghost** as the real header fades in at the new position (crossfade, ~100ms).
4. Alternatively, skip the crossfade and simply **snap** the ghost into place, then replace it with the real header instantly.

**Spring animation** (if available): For a more polished feel, use a spring/elastic easing that slightly overshoots the target position then settles. Parameters: damping 0.7-0.8, stiffness ~300, mass 1.0.

### 5.5 Invalid Drop Zones

If the user drags a header outside the header area:
- Show a **forbidden** cursor (not-allowed).
- The ghost may become **more transparent** (0.4 alpha) or show a red tint.
- On release, animate the ghost **back to its original position** ("snap-back" / "rubber-band" animation, ~300ms ease-out).

---

## 6. Drag Visual Design

Comprehensive catalog of visual treatments during drag operations.

### 6.1 Opacity / Transparency

| Element | Opacity | Purpose |
|---|---|---|
| Drag ghost (floating) | 0.7-0.85 | Indicates it is "in transit," allows seeing content beneath |
| Source placeholder | 0.3-0.4 | Shows origin location without competing visually with the ghost |
| Valid drop zone | 1.0 (highlighted) | Full opacity with accent background to attract attention |
| Invalid drop zone | 0.5-0.6 | Faded out to communicate "not here" |

### 6.2 Drop Shadow

The drag ghost should have elevation to look "picked up":

```
// Light theme
box-shadow: 0 8px 24px rgba(0, 0, 0, 0.12),
            0 2px 8px rgba(0, 0, 0, 0.08);

// Dark theme
box-shadow: 0 8px 24px rgba(0, 0, 0, 0.40),
            0 2px 8px rgba(0, 0, 0, 0.25);
```

In a GPU-rendered context, this translates to rendering a blurred rectangle behind the drag visual with appropriate alpha falloff. A multi-pass Gaussian blur on a solid rect, or a pre-computed shadow texture stretched to match the ghost size.

### 6.3 Scale Animation on Pickup

When drag starts, the ghost should "lift" off the surface:

| Phase | Scale | Duration | Easing |
|---|---|---|---|
| At rest | 1.0 | -- | -- |
| Pickup | 1.0 -> 1.03-1.05 | 100-150ms | ease-out |
| During drag | 1.03-1.05 (hold) | -- | -- |
| Drop | 1.05 -> 1.0 | 150ms | ease-in-out |

The scale should originate from the cursor position (transform-origin at the grab point) for a natural feel. Avoid scaling larger than 1.05x as it becomes distracting.

### 6.4 Tilt / Rotation

Trello and similar card-based UIs apply a slight rotation (2-5 degrees) to the ghost to reinforce the "picked up" metaphor. For grid rows, rotation is less appropriate -- rows should remain horizontal. Use rotation only for card or tile drag.

### 6.5 Cursor Changes

| State | Cursor | Notes |
|---|---|---|
| Hovering over draggable | `grab` (open hand) | Indicates the item can be picked up |
| Actively dragging | `grabbing` (closed hand) | Confirms drag is in progress |
| Over valid drop zone | `grabbing` or `copy` | Depends on operation (move vs. copy) |
| Over invalid drop zone | `not-allowed` (circle with line) | Clear prohibition signal |
| Over edge of scrollable area | `grabbing` + auto-scroll | Cursor stays `grabbing`; the container scrolls automatically |

### 6.6 Drop Zone Feedback

**Valid drop zone** activation progression:
1. **Idle**: No special visual.
2. **Drag active** (something is being dragged, not yet over this zone): Subtle dashed border or light background tint to indicate "I am a potential target."
3. **Drag over** (cursor is within this zone): Stronger visual -- solid border, brighter background, insertion indicator appears.
4. **Drag over + can accept** (type checking passes): Full highlight. Green-tinted or accent-colored.
5. **Drag over + cannot accept**: Red tint, `not-allowed` cursor.

**Proximity activation**: Some implementations increase the visual intensity as the cursor approaches the center of the drop zone, creating a "magnetic" or "gravitational" feel.

### 6.7 Animation on Drop

| Animation | Duration | Description |
|---|---|---|
| Snap to position | 150-200ms | Ghost animates to final resting position |
| Fade out ghost | 100-150ms | Ghost fades as the real element appears |
| Scale down | 150ms | Ghost scales from 1.05 to 1.0 as it settles |
| Reorder slide | 200-250ms | Other items slide to their new positions |
| Bounce/spring | 200-300ms | Overshoots slightly then settles (spring physics) |

### 6.8 Multi-Item Drag

When multiple rows are selected and dragged:

- **Stacked ghost**: Show 2-3 layered row ghosts, slightly offset (4-8px each), with a count badge (e.g., "3 items").
- **Only the top ghost** has full detail; underlying ghosts show only colored rectangles or simplified content.
- The count badge is typically a circle with a number, positioned at the top-right corner of the ghost stack.

---

## 7. Native Desktop Drag Patterns

### 7.1 Qt (QDrag)

Qt's drag system uses OS-level drag support with a pixmap-based visual.

**Setup**:
```cpp
QDrag* drag = new QDrag(this);
QMimeData* mimeData = new QMimeData;
mimeData->setText(rowData);
drag->setMimeData(mimeData);

// Custom pixmap
QPixmap pixmap = renderRowToPixmap(row);
drag->setPixmap(pixmap);
drag->setHotSpot(QPoint(mouseOffset.x(), mouseOffset.y()));

Qt::DropAction result = drag->exec(Qt::MoveAction);
```

**Key characteristics**:
- `setPixmap()`: Must be set before `exec()`. The pixmap is a static image -- no live widget rendering during drag.
- `setHotSpot(QPoint)`: Position relative to the pixmap's top-left that aligns with the cursor. Equivalent to UMG's Pivot+Offset.
- **Platform rendering**: Qt delegates the pixmap rendering to the OS compositor. On Windows, the OS renders the drag image. On X11, the pixmap may lag behind the cursor. On macOS, the compositing is handled by the window server.
- **Transparency limitation**: `QDrag::setPixmap()` has known issues with transparency on some platforms. The pixmap alpha channel may not be respected. Workaround: render the drag visual as a separate `QLabel` widget positioned at the cursor, bypassing the OS drag image entirely.
- **Blocking behavior**: On Windows, `drag->exec()` blocks the Qt event loop during the operation. On Linux and macOS, it does not block.

**Workaround for rich visuals (the QLabel approach)**:
```cpp
// Instead of using QDrag's pixmap:
QLabel* floatingLabel = new QLabel(nullptr, Qt::ToolTip | Qt::FramelessWindowHint);
floatingLabel->setPixmap(renderRowToPixmap(row));
floatingLabel->setWindowOpacity(0.8);
floatingLabel->show();

// Move it in mouseMoveEvent:
floatingLabel->move(QCursor::pos() - hotspot);
```

This bypasses the OS drag image entirely and gives full control over the visual, but loses cross-application drop support.

### 7.2 GTK4 (GtkDragSource + DragIcon)

GTK4 redesigned drag-and-drop with `GtkDragSource` (an event controller) and `GtkDragIcon` (a special root widget for the drag visual).

**Setup**:
```c
GtkDragSource* source = gtk_drag_source_new();
gtk_drag_source_set_actions(source, GDK_ACTION_MOVE);

// Set paintable icon
GdkPaintable* paintable = render_row_to_paintable(row);
gtk_drag_source_set_icon(source, paintable, hotspot_x, hotspot_y);

g_signal_connect(source, "drag-begin", G_CALLBACK(on_drag_begin), data);
gtk_widget_add_controller(widget, GTK_EVENT_CONTROLLER(source));
```

**Key characteristics**:
- **GtkDragIcon**: A `GtkRoot` implementation specifically for drag icons. Moves with the pointer during a drag operation and is destroyed when drag ends.
- **set_from_paintable()**: Uses `GdkPaintable`, a general-purpose rendering interface. The paintable can be a `GdkTexture` (static image), a `GtkWidgetPaintable` (snapshot of a widget), or a custom paintable that draws anything.
- **Hotspot coordinates**: Determine the point on the icon that aligns with the cursor.
- **Drag surface**: GTK4 renders the drag icon using a native drag surface (platform-specific). On Wayland, this is a `wl_surface` with the drag role. On X11, it's a specially configured top-level window.
- **No user input**: Drag icons cannot receive mouse/keyboard input. They are purely visual.

**Dynamic visual via drag-begin signal**: You can customize the drag icon dynamically at the moment the drag begins:
```c
void on_drag_begin(GtkDragSource* source, GdkDrag* drag, gpointer data) {
    GtkDragIcon* icon = gtk_drag_icon_get_for_drag(drag);
    GtkWidget* child = create_custom_drag_widget(data);
    gtk_drag_icon_set_child(icon, child);
}
```

### 7.3 WPF (AdornerLayer)

WPF provides the most architecturally elegant solution for custom drag visuals: the **Adorner Layer**.

**Adorner Architecture**:
- An `Adorner` is a `FrameworkElement` bound to another `UIElement`.
- Adorners are rendered in an `AdornerLayer`, a rendering surface guaranteed to be at a **higher Z-order** than the adorned elements.
- Rendering is independent -- the adorner's paint cycle is separate from its bound element.
- Adorners are positioned using the adorned element's coordinate system (origin at adorned element's top-left).

**DragAdorner pattern**:
```csharp
public class DragAdorner : Adorner
{
    private ContentPresenter _contentPresenter;
    private double _leftOffset, _topOffset;

    public DragAdorner(UIElement adornedElement, object data,
                       DataTemplate template) : base(adornedElement)
    {
        _contentPresenter = new ContentPresenter
        {
            Content = data,
            ContentTemplate = template,
            Opacity = 0.8
        };
        IsHitTestVisible = false; // Critical: don't block drops
    }

    public void UpdatePosition(double left, double top)
    {
        _leftOffset = left;
        _topOffset = top;
        var layer = Parent as AdornerLayer;
        layer?.Update(AdornedElement);
    }

    public override GeneralTransform GetDesiredTransform(GeneralTransform transform)
    {
        return new GeneralTransformGroup
        {
            Children = {
                base.GetDesiredTransform(transform),
                new TranslateTransform(_leftOffset, _topOffset)
            }
        };
    }

    // ... MeasureOverride, ArrangeOverride, VisualChildrenCount, GetVisualChild
}
```

**Key characteristics**:
- `IsHitTestVisible = false`: Essential. Without this, the adorner intercepts mouse events and blocks drop targets from receiving them.
- **PreviewGiveFeedback event**: Fires during drag, provides cursor position updates for moving the adorner.
- **AdornerLayer.GetAdornerLayer(element)**: Retrieves the adorner layer for the visual tree containing the element.
- **Arbitrary content**: The adorner can render any WPF visual tree via `ContentPresenter` + `DataTemplate`. Full data binding, animations, effects are available.
- **Independent rendering**: The adorner layer composites on top of the normal visual tree, similar to a CSS `position: fixed` layer with `z-index: 9999`.

### 7.4 Comparison of Native Desktop Approaches

| Feature | Qt | GTK4 | WPF |
|---|---|---|---|
| Visual type | Pixmap (static image) | Paintable/Widget | Full visual tree (Adorner) |
| Live rendering | No (static pixmap) | Partial (paintable can animate) | Yes (full WPF rendering) |
| Opacity control | Limited (platform-dependent) | Yes (via widget properties) | Yes (Adorner.Opacity) |
| Hit test passthrough | N/A (OS manages) | N/A (drag surface) | IsHitTestVisible = false |
| Cross-app drag | Yes (OS-level) | Yes (platform drag protocol) | Yes (OLE DnD) |
| Rendering layer | OS compositor | Native drag surface | AdornerLayer (in-app) |
| Custom shape | Rectangle only (pixmap) | Arbitrary (paintable) | Arbitrary (XAML visual tree) |

---

## 8. Implementation in GPU-Rendered UI

When you own the entire render pipeline (as in a Rust/wgpu application), you have maximum control. Here is an architecture for drag visuals.

### 8.1 Layered Rendering Architecture

The render pipeline should support distinct **ordered layers** (similar to WPF's AdornerLayer concept):

```
Z-order (back to front):
  ┌─────────────────────────────┐
  │  Layer 0: Background        │  Grid background, alternating row stripes
  ├─────────────────────────────┤
  │  Layer 1: Grid Content      │  Cell text, icons, data values
  ├─────────────────────────────┤
  │  Layer 2: Selection/Focus   │  Selection highlights, focus rings
  ├─────────────────────────────┤
  │  Layer 3: Headers           │  Column headers (pinned, always visible)
  ├─────────────────────────────┤
  │  Layer 4: Scrollbars        │  Scroll indicators
  ├─────────────────────────────┤
  │  Layer 5: Drop Indicators   │  Insertion lines, gap spacers, zone highlights
  ├─────────────────────────────┤
  │  Layer 6: Drag Overlay      │  The floating ghost widget (TOPMOST)
  └─────────────────────────────┘
```

Each layer is a conceptual render pass (or a set of draw commands with specific depth/order). The drag overlay (Layer 6) renders last, so it always appears on top.

### 8.2 The Drag Overlay Layer

**What it renders**: A snapshot or live re-render of the dragged element (row, column header, or arbitrary content).

**Positioning**: Follow cursor position minus a hotspot offset. The hotspot is typically the cursor's position relative to the top-left of the dragged element at the moment drag was initiated.

```rust
struct DragState {
    /// What is being dragged
    payload: DragPayload,
    /// Offset from cursor to the top-left of the drag visual
    hotspot: Vec2,
    /// Current cursor position (screen/window coords)
    cursor_pos: Vec2,
    /// Opacity of the drag visual (animated on pickup/drop)
    opacity: f32,
    /// Scale of the drag visual (animated on pickup/drop)
    scale: f32,
    /// Whether a valid drop target is under the cursor
    can_drop: bool,
}

impl DragState {
    fn visual_position(&self) -> Vec2 {
        self.cursor_pos - self.hotspot
    }

    fn visual_transform(&self) -> Mat3 {
        // Scale around the hotspot point
        let pos = self.visual_position();
        Mat3::from_translation(pos)
            * Mat3::from_scale(Vec2::splat(self.scale))
    }
}
```

### 8.3 Rendering the Drag Widget

Two approaches:

#### Approach A: Snapshot Texture (Simpler)

1. When drag starts, render the dragged element (row/header) into an off-screen render target (wgpu texture).
2. During drag, draw this texture as a textured quad at the cursor-following position.
3. Apply opacity and shadow as post-processing or blending.

**Pros**: Simple, one-time rendering cost, works for any element.
**Cons**: Static -- cannot reflect live data changes (e.g., price updates). Text may look blurry if scaled.

```rust
// On drag start
let snapshot_texture = render_row_to_texture(&gpu, &row_data, row_bounds);

// Each frame while dragging
draw_shadow_rect(&gpu, drag_pos, drag_size, shadow_params);
draw_textured_quad(&gpu, &snapshot_texture, drag_pos, drag_size, opacity);
```

#### Approach B: Live Re-render (Richer)

1. During each frame, re-render the dragged element using the same rendering code as normal grid content, but at the overlay layer position.
2. Apply transform (position + scale) and opacity as part of the render.

**Pros**: Live content updates, pixel-perfect rendering at any scale, animation-ready.
**Cons**: Re-renders the element every frame. More complex coordinate management.

```rust
// Each frame while dragging
fn render_drag_overlay(&self, encoder: &mut RenderEncoder) {
    // Push the drag overlay transform
    let transform = self.drag_state.visual_transform();
    let opacity = self.drag_state.opacity;

    // Render shadow first (below the ghost)
    self.render_shadow(encoder, transform, self.drag_visual_size());

    // Render the row content at the overlay position
    encoder.push_layer(Layer::DragOverlay);
    encoder.set_transform(transform);
    encoder.set_opacity(opacity);
    self.render_row(encoder, &self.drag_state.payload.row_data);
    encoder.pop_layer();
}
```

#### Approach C: Hybrid (Recommended)

Use a snapshot texture for the visual content, but render the shadow, border, and animations live:

1. Capture a snapshot texture at drag start.
2. Each frame: render a live shadow (Gaussian blur or 9-slice shadow texture), then render the snapshot quad on top, then render any live overlay elements (count badge for multi-select, status icons).
3. On drop, animate position/scale/opacity of the snapshot quad.

### 8.4 Shadow Rendering for Drag Elevation

In a GPU pipeline, shadows can be rendered several ways:

| Method | Quality | Cost | Approach |
|---|---|---|---|
| **Pre-baked 9-slice** | Good | Very low | A shadow texture with transparent center, stretched to match ghost size. 4 corners + 4 edges + 1 center. |
| **Box blur** | Good | Low | Render a solid rect to a small off-screen target, apply a separable box blur, composite behind the ghost. |
| **Gaussian blur** | Excellent | Medium | Same as box blur but with Gaussian kernel. Can be approximated with 2-3 box blur passes. |
| **SDF shadow** | Excellent | Low | Compute shadow analytically from a rounded-rect signed distance field in the fragment shader. Best for rounded-corner ghosts. |

The SDF approach is ideal for a wgpu renderer:

```wgsl
// Fragment shader for rounded-rect shadow
fn shadow(uv: vec2<f32>, rect_size: vec2<f32>, radius: f32, blur: f32) -> f32 {
    let d = sd_rounded_box(uv - rect_size * 0.5, rect_size * 0.5, vec4(radius));
    return 1.0 - smoothstep(-blur, blur, d);
}
```

### 8.5 Drop Indicator Rendering

**Insertion line**: A horizontal (or vertical for columns) line rendered at the drop position.

```rust
struct DropIndicator {
    position: Vec2,      // Where the line starts
    length: f32,         // How long the line is
    thickness: f32,      // Line thickness (2-3px)
    orientation: Axis,   // Horizontal (row reorder) or Vertical (column reorder)
    color: Color,        // Accent color
    terminal_radius: f32, // Circle at the end of the line
}
```

The indicator should include a small **terminal circle** (4-6px radius) at one or both ends, bleeding slightly past the edge of the grid content. This is a pattern from Atlassian's design system that significantly improves visibility.

Render order: The drop indicator should be in Layer 5 (above content but below the drag ghost), so the ghost can pass over it without the indicator appearing on top of the ghost.

### 8.6 Hit Testing During Drag

When rendering the drag overlay, the overlay itself must be **excluded from hit testing**. The cursor's position should be tested against the underlying grid content, not the floating ghost.

```rust
fn hit_test_for_drop(&self, cursor: Vec2) -> Option<DropTarget> {
    // Ignore the drag overlay -- test against the grid layout
    let grid_pos = self.screen_to_grid(cursor);

    if self.header_area.contains(grid_pos) {
        // Column reorder target
        let col_index = self.column_at_x(grid_pos.x);
        let edge = self.nearest_column_edge(grid_pos.x);
        return Some(DropTarget::ColumnInsert { index: edge });
    }

    if self.body_area.contains(grid_pos) {
        // Row reorder target
        let row_index = self.row_at_y(grid_pos.y);
        let edge = self.nearest_row_edge(grid_pos.y);
        return Some(DropTarget::RowInsert { index: edge });
    }

    None // Over invalid area
}
```

### 8.7 Animation System

Drag animations require interpolation of position, scale, and opacity over time:

```rust
struct DragAnimation {
    property: AnimatedProperty, // Position, Scale, Opacity
    from: f32,
    to: f32,
    start_time: Instant,
    duration: Duration,
    easing: EasingFunction,
}

enum EasingFunction {
    Linear,
    EaseOut,       // Decelerating -- good for snap-to-position
    EaseInOut,     // Smooth -- good for reorder slides
    Spring {       // Organic -- good for drop animations
        damping: f32,
        stiffness: f32,
    },
}
```

**Pickup animation** (drag start):
- Scale: 1.0 -> 1.03 over 120ms ease-out
- Shadow: 0 -> full over 120ms ease-out
- Source opacity: 1.0 -> 0.35 over 100ms ease-out

**Drop animation** (successful drop):
- Position: current -> target over 180ms ease-out
- Scale: 1.03 -> 1.0 over 180ms ease-in-out
- Opacity: 0.8 -> 1.0 over 120ms ease-out
- Shadow: full -> 0 over 180ms ease-out
- Reorder slide: other rows translate to new positions over 200ms ease-in-out

**Cancel animation** (invalid drop):
- Position: current -> original over 250ms ease-out (rubber-band back)
- Scale: 1.03 -> 1.0 over 200ms ease-out
- Opacity: 0.8 -> 1.0 over 150ms ease-out
- Source opacity: 0.35 -> 1.0 over 150ms ease-out

### 8.8 Auto-Scroll During Drag

When the cursor approaches the edge of the grid viewport during a drag:

```rust
const SCROLL_ZONE_PX: f32 = 40.0;     // Distance from edge to activate
const SCROLL_SPEED_MAX: f32 = 600.0;   // Pixels per second at edge
const SCROLL_SPEED_MIN: f32 = 60.0;    // Pixels per second at zone boundary

fn auto_scroll_speed(&self, cursor_y: f32, viewport: Rect) -> f32 {
    let top_dist = cursor_y - viewport.min_y;
    let bottom_dist = viewport.max_y - cursor_y;

    if top_dist < SCROLL_ZONE_PX {
        // Scroll up, speed increases as cursor approaches edge
        let t = 1.0 - (top_dist / SCROLL_ZONE_PX);
        return -lerp(SCROLL_SPEED_MIN, SCROLL_SPEED_MAX, t);
    }
    if bottom_dist < SCROLL_ZONE_PX {
        // Scroll down
        let t = 1.0 - (bottom_dist / SCROLL_ZONE_PX);
        return lerp(SCROLL_SPEED_MIN, SCROLL_SPEED_MAX, t);
    }
    0.0 // No auto-scroll
}
```

The scroll speed should increase smoothly as the cursor gets closer to the edge (not a sudden jump). The easing function should be linear or slight ease-in.

### 8.9 Reference: GPUI (Zed Editor)

Zed's GPUI framework is the closest real-world example of a Rust GPU-rendered UI with drag support:

- **Hybrid immediate/retained mode**: Layout is computed each frame, but visual layers are retained.
- **Stacking contexts**: Elements can push a `Layer` to the scene, which creates a new stacking context (similar to CSS `z-index`). Drag overlays use this to render above all content.
- **Hit testing**: `Frame::hit_test` processes hitboxes in reverse order (front to back). During drag, the dragged element's hitbox is set to `None` so it does not interfere with drop target detection.
- **Cursor management**: During drag, the cursor style is forced to `grabbing` regardless of what is under the cursor.
- **Drag payload**: Uses `AnyDrag` / typed drag payloads with `can_drop_predicate` for type-safe drop validation.

### 8.10 Recommended Architecture for Rust/wgpu Grid

```
DragDropSystem
├── DragDetector
│   ├── Handles mousedown/mousemove threshold detection
│   ├── Determines drag source type (RowHandle, ColumnHeader, Cell)
│   └── Creates DragPayload with source data
│
├── DragState
│   ├── payload: DragPayload
│   ├── hotspot: Vec2
│   ├── cursor_pos: Vec2 (updated each frame from input)
│   ├── animations: Vec<DragAnimation> (pickup, drop, cancel)
│   ├── snapshot_texture: Option<wgpu::Texture>
│   └── drop_target: Option<DropTarget>
│
├── DropTargetResolver
│   ├── hit_test(cursor_pos) -> Option<DropTarget>
│   ├── validate(payload, target) -> bool
│   └── compute_indicator(target) -> DropIndicator
│
├── DragRenderer (Layer 6)
│   ├── render_shadow(transform, size, shadow_params)
│   ├── render_ghost(texture_or_live, transform, opacity)
│   └── render_badge(count, position) // multi-select
│
├── IndicatorRenderer (Layer 5)
│   ├── render_insertion_line(indicator)
│   ├── render_zone_highlight(target_rect, valid)
│   └── render_gap_spacer(position, size) // animated reorder
│
└── AnimationController
    ├── tick(dt) -- advance all active animations
    ├── start_pickup_animation()
    ├── start_drop_animation(target_pos)
    └── start_cancel_animation(origin_pos)
```

---

## Sources

### Unreal Engine UMG
- [Creating Drag and Drop UI - UE5 Documentation](https://dev.epicgames.com/documentation/en-us/unreal-engine/creating-drag-and-drop-ui-in-unreal-engine)
- [UDragDropOperation API Reference](https://dev.epicgames.com/documentation/en-us/unreal-engine/API/Runtime/UMG/Blueprint/UDragDropOperation)
- [EDragPivot Enum Reference](https://dev.epicgames.com/documentation/en-us/unreal-engine/API/Runtime/UMG/EDragPivot)
- [DefaultDragVisual Property](https://docs.unrealengine.com/5.0/en-US/API/Runtime/UMG/Blueprint/UDragDropOperation/DefaultDragVisual/)
- [DragDropOperation Python API](https://docs.unrealengine.com/5.4/en-US/PythonAPI/class/DragDropOperation.html)

### HTML5 Drag and Drop
- [HTML Drag and Drop API - MDN](https://developer.mozilla.org/en-US/docs/Web/API/HTML_Drag_and_Drop_API)
- [DataTransfer.setDragImage() - MDN](https://developer.mozilla.org/en-US/docs/Web/API/DataTransfer/setDragImage)
- [DataTransfer - MDN](https://developer.mozilla.org/en-US/docs/Web/API/DataTransfer)
- [HTML5 Drag & Drop: Not the API You're Looking For](https://www.sam.today/blog/html5-dnd-the-api-that-is-gaslighting-you)

### React DnD Libraries
- [dnd-kit Documentation](https://docs.dndkit.com/)
- [dnd-kit DragOverlay API](https://dndkit.com/legacy/api-documentation/draggable/drag-overlay)
- [dnd-kit GitHub Repository](https://github.com/clauderic/dnd-kit)
- [react-dnd useDragLayer API](https://react-dnd.github.io/react-dnd/docs/api/use-drag-layer)
- [react-dnd DragLayerMonitor API](https://react-dnd.github.io/react-dnd/docs/api/drag-layer-monitor/)
- [Atlassian Pragmatic Drag and Drop](https://atlassian.design/components/pragmatic-drag-and-drop/)
- [Atlassian DnD Design Guidelines](https://atlassian.design/components/pragmatic-drag-and-drop/design-guidelines/)
- [Atlassian React Drop Indicator](https://atlassian.design/components/pragmatic-drag-and-drop/optional-packages/react-drop-indicator/)

### Row and Column Drag Patterns
- [MUI X Data Grid Row Ordering](https://mui.com/x/react-data-grid/row-ordering/)
- [MUI X Data Grid Column Ordering](https://mui.com/x/react-data-grid/column-ordering/)
- [Infragistics Row Dragging](https://www.infragistics.com/products/ignite-ui-angular/angular/components/grid/row-drag)
- [Framer Motion Reorder](https://motion.dev/docs/react-reorder)
- [Column Header Drag and Drop Implementation](https://tschumacher.net/column-header-drag-drop/)

### UX Design Guidelines
- [NN/g: Drag-and-Drop Design for Ease of Use](https://www.nngroup.com/articles/drag-drop/)
- [SAP Fiori: Drag and Drop Reorder](https://www.sap.com/design-system/fiori-design-web/v1-136/foundations/interaction/drag-and-drop/reorder)
- [Smart Interface Design Patterns: Drag-and-Drop UX](https://smart-interface-design-patterns.com/articles/drag-and-drop-ux/)
- [Pencil & Paper: Drag & Drop UX Design Best Practices](https://www.pencilandpaper.io/articles/ux-pattern-drag-and-drop)
- [GitLab Pajamas: Drag and Drop](https://design.gitlab.com/patterns/drag-and-drop/)
- [Cloudscape: Drag-and-Drop Pattern](https://cloudscape.design/patterns/general/drag-and-drop/)
- [Eleken: Drag and Drop UI Examples](https://www.eleken.co/blog-posts/drag-and-drop-ui)

### Native Desktop Frameworks
- [Qt QDrag Class Documentation](https://doc.qt.io/qt-6/qdrag.html)
- [Qt Drag and Drop Overview](https://doc.qt.io/qt-6/dnd.html)
- [GTK4 DragIcon Class](https://docs.gtk.org/gtk4/class.DragIcon.html)
- [GTK4 DragSource Class](https://docs.gtk.org/gtk4/class.DragSource.html)
- [GTK4 DragIcon.set_from_paintable](https://docs.gtk.org/gtk4/type_func.DragIcon.set_from_paintable.html)
- [WPF Adorners Overview - Microsoft Learn](https://learn.microsoft.com/en-us/dotnet/desktop/wpf/controls/adorners)
- [WPF Drag/Drop Feedback on Adorner Layer - Microsoft Learn](https://learn.microsoft.com/en-us/archive/blogs/marcelolr/showing-dragdrop-feedback-on-the-wpf-adorner-layer)

### GPU-Rendered UI Frameworks
- [Zed Blog: Rendering UI at 120 FPS with Rust and GPU](https://zed.dev/blog/videogame)
- [GPUI Framework (Zed) - DeepWiki](https://deepwiki.com/zed-industries/zed/2.2-ui-framework-(gpui))
- [egui: Immediate Mode GUI in Rust](https://github.com/emilk/egui)
- [egui Drag and Drop Discussion](https://github.com/emilk/egui/discussions/3869)
- [egui DragAndDrop API](https://docs.rs/egui/latest/egui/struct.DragAndDrop.html)
- [GUI on the GPU - Nical](https://nical.github.io/drafts/gui-gpu-notes.html)
- [Raph Levien: 2D Graphics on Modern GPU](https://raphlinus.github.io/rust/graphics/gpu/2019/05/08/modern-2d.html)
