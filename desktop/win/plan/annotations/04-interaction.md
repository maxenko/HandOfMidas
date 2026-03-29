# 04 — Interaction

## Interaction Modes

Extend the existing `InteractionMode` state machine in `midas-chart/src/interaction.rs`.

```rust
pub enum InteractionMode {
    // ─── Existing modes (unchanged) ─────────────────────
    Idle,
    PendingDrag { start_x: f32, start_y: f32 },
    Panning,
    DraggingLevel { level_id: u64, grab_offset: f64 },
    PendingScale,
    HorizontalScaling { anchor_x: f32, last_x: f32 },
    VerticalScaling { anchor_y: f32, last_y: f32 },
    RightPanning,
    DraggingTimelineBorder { anchor_y: f32, start_ratio: f32 },
    DraggingVolumeScale { anchor_y: f32, start_scale: f32 },

    // ─── New annotation modes ───────────────────────────

    /// Drawing a bracket: multi-click sequence.
    DrawingBracket {
        side: BracketSide,
        phase: BracketDrawPhase,
    },

    /// Dragging a single bracket leg to adjust its price.
    DraggingBracketLeg {
        annotation_id: AnnotationId,
        leg: BracketLegKind,
        grab_offset: f64,
    },

    /// Dragging a text note to reposition it.
    DraggingNote {
        annotation_id: AnnotationId,
        grab_offset_price: f64,
        grab_offset_time: i64,
    },

    /// Placing a marker (single click to place).
    PlacingMarker {
        icon: MarkerIcon,
    },
}

#[derive(Clone, Debug)]
pub enum BracketDrawPhase {
    /// Waiting for user to click entry price.
    WaitingEntry,
    /// Entry set, waiting for TP click. Shows preview line.
    WaitingTP { entry_price: f64, entry_time: i64 },
    /// Entry + TP set, waiting for SL click. Shows preview bracket.
    WaitingSL { entry_price: f64, entry_time: i64, tp_price: f64 },
}

#[derive(Clone, Copy, Debug)]
pub enum BracketLegKind {
    Entry,
    TakeProfit,
    StopLoss,
}
```

## New ChartActions

```rust
pub enum ChartAction {
    // ─── Existing (unchanged) ───────────────────────────
    Pan { dx: f64, dy: f64 },
    Zoom { center_x: f64, factor: f64 },
    ZoomY { center_y: f64, factor: f64 },
    SetCrosshair { x: f32, y: f32 },
    ClearCrosshair,
    CreateLevel { price: f64 },
    SelectLevel { id: u64 },
    DragLevel { id: u64, new_price: f64 },
    DeleteSelectedLevel,
    // ...

    // ─── New annotation actions ─────────────────────────

    /// Begin drawing a bracket. Transitions to DrawingBracket mode.
    StartDrawBracket { side: BracketSide },

    /// Set the entry price during bracket drawing.
    SetBracketEntry { price: f64, timestamp: i64 },

    /// Set the TP price during bracket drawing.
    SetBracketTP { price: f64 },

    /// Set the SL price during bracket drawing. Completes the bracket.
    SetBracketSL { price: f64 },

    /// Cancel an in-progress bracket drawing.
    CancelDrawing,

    /// Create a complete bracket annotation.
    CreateBracket { bracket: OrderBracket },

    /// Drag a bracket leg to a new price.
    DragBracketLeg { id: AnnotationId, leg: BracketLegKind, new_price: f64 },

    /// Select an annotation (any type).
    SelectAnnotation { id: AnnotationId },

    /// Deselect current annotation.
    DeselectAnnotation,

    /// Delete the selected annotation.
    DeleteSelectedAnnotation,

    /// Toggle visibility of an annotation.
    ToggleAnnotationVisibility { id: AnnotationId },

    /// Toggle lock state of an annotation.
    ToggleAnnotationLock { id: AnnotationId },

    /// Create a text note at a point.
    CreateNote { price: f64, timestamp: i64, text: String },

    /// Move a note to a new position.
    DragNote { id: AnnotationId, new_price: f64, new_timestamp: i64 },

    /// Place a marker at a point.
    PlaceMarker { price: f64, timestamp: i64, icon: MarkerIcon },

    /// Update annotation tags.
    SetAnnotationTags { id: AnnotationId, tags: Vec<String> },

    /// Batch action for app-layer order bridge updates.
    UpdateAnnotationStatus { id: AnnotationId, status: BracketStatus },
}
```

## Hit-Testing

### Priority Order

When the user clicks, hit-test in this order (most specific wins):

1. **Volume handle triangle** (existing, right edge only)
2. **Timeline border** (existing, full width, ±6px)
3. **Bracket legs** (horizontal lines, ±6px)
4. **Markers** (point objects, ±8px radius)
5. **Notes** (bounding box, exact)
6. **Levels** (horizontal lines, ±6px — existing)
7. **Empty space** → begin PendingDrag (pan/zoom)

### Hit Zone

```rust
/// Describes which part of an annotation was hit.
#[derive(Clone, Debug)]
pub enum HitZone {
    /// A level's horizontal line.
    LevelLine,
    /// A bracket's entry line.
    BracketEntry,
    /// A bracket's TP line.
    BracketTP,
    /// A bracket's SL line.
    BracketSL,
    /// A bracket's TP zone fill (between entry and TP).
    BracketTPZone,
    /// A bracket's SL zone fill (between entry and SL).
    BracketSLZone,
    /// A marker's icon area.
    MarkerIcon,
    /// A note's text bounding box.
    NoteBody,
}
```

The `HitZone` tells the interaction handler what drag behavior to use:
- `BracketEntry` → drag moves the entire bracket (all legs shift by same delta)
- `BracketTP` / `BracketSL` → drag moves only that leg
- `BracketTPZone` / `BracketSLZone` → select the bracket (no drag)
- `NoteBody` → drag repositions the note

### Hit-Test Implementation

```rust
/// Test all annotations at a screen coordinate.
/// Returns the hit annotation and which zone was clicked.
pub fn hit_test_annotations(
    store: &AnnotationStore,
    x: f32,
    y: f32,
    camera: &Camera2D,
    viewport_width: u32,
    viewport_height: u32,
) -> Option<(AnnotationId, HitZone)> {
    // Iterate in reverse (topmost first, since later annotations render on top)
    for annotation in store.iter().rev() {
        if !annotation.visible || annotation.locked {
            continue;
        }
        if let Some(zone) = hit_test_single(annotation, x, y, camera, viewport_width, viewport_height) {
            return Some((annotation.id, zone));
        }
    }
    None
}
```

## Bracket Drawing Flow

### State Transitions

```
Idle
  │  ← user activates bracket tool (keyboard B or toolbar)
  ▼
DrawingBracket { side: Long, phase: WaitingEntry }
  │  ← click at price P1
  │  → emit SetBracketEntry { price: P1 }
  ▼
DrawingBracket { side: Long, phase: WaitingTP { entry: P1 } }
  │  ← click at price P2 (must be > P1 for Long)
  │  → emit SetBracketTP { price: P2 }
  ▼
DrawingBracket { side: Long, phase: WaitingSL { entry: P1, tp: P2 } }
  │  ← click at price P3 (must be < P1 for Long)
  │  → emit SetBracketSL { price: P3 }
  │  → emit CreateBracket { ... }
  ▼
Idle
```

### Preview During Drawing

While in `WaitingTP` or `WaitingSL`, the compute pipeline renders a preview:
- Dashed lines at the already-set prices
- A "ghost" line following the cursor at the current mouse Y position
- Zone fill preview between set lines and cursor

This is purely visual — no annotation is created until all three legs are set
(or two, if the user presses Enter to skip SL/TP).

### Escape / Cancel

- `Escape` at any phase → `CancelDrawing` → return to `Idle`
- `Right-click` at any phase → same as Escape

### Modifier Keys

| Key | Effect During Drawing |
|---|---|
| `Shift` | Snap to nearest price grid level |
| `Ctrl` | Skip current leg (e.g., skip TP, go to SL) |
| `Enter` | Complete bracket with legs set so far |
| `Escape` | Cancel drawing |
| `Tab` | Toggle Long ↔ Short side |

## Bracket Leg Dragging

When a bracket leg is hit and dragged:

```rust
// In handle_event, during DraggingBracketLeg:
let new_price = camera.y_to_price(mouse_y) + grab_offset;

// Enforce constraints based on side and leg:
match (side, leg) {
    (Long, TakeProfit) => new_price = new_price.max(entry.price + min_tick),
    (Long, StopLoss)   => new_price = new_price.min(entry.price - min_tick),
    (Short, TakeProfit) => new_price = new_price.min(entry.price - min_tick),
    (Short, StopLoss)   => new_price = new_price.max(entry.price + min_tick),
    (_, Entry)          => { /* unconstrained, but TP/SL swap if crossed */ },
}
```

If the user drags an entry line past TP or SL, the legs swap (TP becomes SL, etc.)
rather than clamping. This is the most natural behavior from a trading UX perspective.

## Cursor Changes

| Hover Target | Cursor |
|---|---|
| Level line | `ResizeUpDown` (existing) |
| Bracket leg line | `ResizeUpDown` |
| Bracket zone fill | `Pointer` (hand) |
| Note body | `Move` (four arrows) |
| Marker icon | `Pointer` |
| Drawing mode (no target) | `Crosshair` |

## Keyboard Shortcuts

| Key | Action | Context |
|---|---|---|
| `B` | Start bracket drawing (Long) | Idle |
| `Shift+B` | Start bracket drawing (Short) | Idle |
| `N` | Start note placement | Idle |
| `M` | Start marker placement | Idle |
| `Delete` | Delete selected annotation | Annotation selected |
| `Escape` | Cancel drawing / deselect | Drawing or selected |
| `L` | Toggle lock on selected | Annotation selected |
| `H` | Toggle visibility on selected | Annotation selected |
| `Ctrl+A` | Select all annotations | Idle (future) |

## Context Menu (App Layer)

Right-clicking an annotation opens a context menu (implemented in midas-app, not midas-chart):

```
┌──────────────────────────┐
│ Edit Label...            │
│ Change Color...          │
│ ──────────────────────── │
│ Submit as Order →        │  ← only for Draft brackets
│ Cancel Order             │  ← only for Pending/Active brackets
│ Modify Order...          │  ← only for Pending/Active brackets
│ ──────────────────────── │
│ Lock                     │
│ Hide                     │
│ Delete                   │
│ ──────────────────────── │
│ Properties...            │
└──────────────────────────┘
```

Context menus require iced overlay widgets — pure UI, not part of the sans-IO chart.
