# 04 -- Interaction System

> Widget interaction architecture: selection, dragging, hit testing, drawing tools,
> order brackets, and the path from monolithic handler to composable modifiers.
>
> Builds on the existing `handle_event()` state machine in
> `midas-chart/src/interaction.rs` and the tool patterns established by
> `CrosshairTool` and `LevelTool`.

---

## 1. Interaction Architecture Overview

### 1.1 Event Flow (Extended for Widgets)

The existing sans-IO event pipeline is unchanged in structure. Widgets
extend it by adding new `ChartEvent` variants, new `InteractionMode` states,
and new `ChartAction` variants -- but the pipeline shape is identical:

```
Mouse/Key input (iced)
    |
    v
ChartEvent                          -- normalized input, no framework types
    |
    v
handle_event(&mut ChartState, ...)  -- pure state machine, returns actions
    |
    v
Vec<ChartAction>                    -- semantic intentions
    |
    v
MidasApp::update()                  -- applies actions to state + AnnotationStore
    |
    v
state mutation                      -- camera, dirty flags, annotations
    |
    v
compute_chart_scene()               -- reads state, produces ChartScene
    |                                  (including WidgetOutput + HitZones)
    v
ChartRenderer::render()             -- GPU draw calls from ChartScene
```

Key extension points for widgets:

1. **AnnotationStore** lives on `MidasApp` (app layer), NOT on `ChartState`.
   `ChartState` is a sans-IO pure state machine with zero knowledge of the
   store. Actions that modify annotations (create, drag, delete) flow through
   `ChartAction` variants, which `MidasApp::update()` applies to the store.
   Charts receive `&[Annotation]` slices through `ChartInput` during scene
   computation.

2. **HitZones** are computed during `compute_chart_scene()` and cached on
   `ChartScene`. The interaction layer reads last frame's hit zones for the
   current frame's hit testing. This avoids re-querying annotation geometry
   during input handling.

3. **Selection state** lives on `ChartState` (not on the annotation itself),
   because selection is view-specific -- the same annotation could be visible
   on two charts but selected on only one.

### 1.2 Tool vs Widget Distinction

These terms have specific meanings in this architecture:

| Concept | Definition | Examples |
|---|---|---|
| **Tool** | Active state machine that captures input until completed or cancelled. Modal -- only one tool active at a time. Transient. | LevelTool, BracketTool, MeasureTool |
| **Widget** | Persistent data stored in AnnotationStore. Passive recipient of selection, drag, edit, delete. | HorizontalLevel, OrderBracket, TextNote, Marker |
| **Interaction** | Any user gesture that modifies chart state. May or may not involve a tool. | Pan, zoom, click-to-select, drag-to-move |

The lifecycle relationship:

```
  Tool (transient)              Widget (persistent)
  ==================            ====================
  User activates tool
       |
  Tool captures clicks -----> Tool emits CreateAnnotation action
       |                           |
  Tool deactivates                 v
                             AnnotationStore.insert()
                                   |
                             Widget exists independently
                                   |
                             User can select, drag, edit, delete
                             without any tool being active
```

A tool CREATES widgets. After creation, the widget exists independently of the
tool. The tool's job is done. This is why `LevelTool` transitions to `Idle`
after placing a level -- the level persists, but the tool does not "own" it.

### 1.3 Why Monolithic handle_event() (For Now)

The existing `handle_event()` function is ~850 lines and handles ~10 interaction
modes. Research into SciChart's ChartModifier pattern confirms it is the gold
standard for composable interaction behaviors, but it introduces indirection that
is premature for our current complexity.

**Migration triggers** (any one of these means it is time to refactor):
- `handle_event()` exceeds 1500 lines
- More than 15 InteractionMode variants
- Multiple modes need identical sub-behaviors (e.g., three tools all need
  the same "snap to OHLC" logic but the logic cannot be extracted to a
  shared function because it depends on modal state)

Until then, the monolithic function is easier to read, debug, and modify.
Section 8 describes the migration path when the time comes.

---

## 2. Widget Interaction Modes

### 2.1 Selection

Selection is the most common interaction. Any click that hits an annotation
(and is not captured by an active tool) selects that annotation.

**Selection rules:**

1. Click on annotation body/line/marker -> select it.
2. Click on empty chart space -> deselect.
3. Selected annotation shows handles and a highlight glow.
4. Only one annotation selected at a time (v1). Multi-select is future work.
5. Selection state lives on `ChartState.selected_annotation: Option<AnnotationId>`,
   NOT on the `Annotation` struct itself.

```rust
// In ChartState:
pub struct ChartState {
    // ... existing fields ...

    /// Currently selected annotation, if any.
    /// Selection is view-specific -- two charts showing the same symbol
    /// can have different selections.
    pub selected_annotation: Option<AnnotationId>,

    /// Cached hit zones from last compute pass.
    /// Updated by compute_chart_scene(), read by handle_event().
    pub hit_zones: Vec<HitZone>,
}
```

**Why selection is view-specific, not annotation-specific:**

If annotations gain a `selected: bool` field, then two charts displaying the
same AnnotationStore would fight over selection state. Since selection is a UI
concern (highlight rendering, handle display), it belongs to the view.

**Selection action flow:**

```
Click at (x, y)
    |
    v
hit_test_zones(&state.hit_zones, x, y)
    |
    +-- Some((id, zone)) --> ChartAction::SelectAnnotation { id }
    |                            |
    |                            v
    |                        state.selected_annotation = Some(id)
    |                        dirty.annotations += 1
    |
    +-- None --> ChartAction::DeselectAnnotation
                     |
                     v
                 state.selected_annotation = None
                 dirty.annotations += 1
```

### 2.2 Hit Testing

Hit testing converts a screen-space point into the annotation and zone that
the user intended to interact with. It operates on precomputed `HitZone`
structs from the most recent compute pass.

#### HitZone Definition

```rust
/// Describes a clickable/draggable region on the chart.
/// Produced by compute_chart_scene(), consumed by handle_event().
pub struct HitZone {
    /// Which annotation owns this zone.
    pub annotation_id: AnnotationId,
    /// What part of the annotation this zone represents.
    pub kind: HitZoneKind,
    /// Screen-space bounding rect [left, top, right, bottom] in pixels.
    pub rect: [f32; 4],
    /// Cursor icon to show when hovering this zone.
    pub cursor: CursorIcon,
}

/// Which part of an annotation was hit.
///
/// The interaction layer uses this to determine drag behavior:
/// - `LevelLine` -> vertical drag (price only)
/// - `BracketEntry` -> vertical drag (moves entire bracket)
/// - `BracketTP` / `BracketSL` -> vertical drag (moves single leg)
/// - `BracketZone` -> select only (no drag)
/// - `MarkerIcon` -> select only
/// - `NoteBody` -> 2D drag (price + time)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HitZoneKind {
    /// A level's horizontal line.
    LevelLine,
    /// A bracket's entry line.
    BracketEntry,
    /// A bracket's take-profit line.
    BracketTP,
    /// A bracket's stop-loss line.
    BracketSL,
    /// A bracket's zone fill (between entry and TP or SL).
    BracketZone,
    /// A marker's icon area.
    MarkerIcon,
    /// A text note's bounding box.
    NoteBody,
    /// A volume profile's histogram area.
    VolumeProfileBar,
}

/// Cursor icon hint for the iced widget layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorIcon {
    /// Default chart crosshair.
    Crosshair,
    /// Grab/move (open hand).
    Grab,
    /// Grabbing (closed hand) -- during active drag.
    Grabbing,
    /// Vertical resize (N-S arrows).
    ResizeNS,
    /// Horizontal resize (E-W arrows).
    ResizeEW,
    /// Clickable pointer (hand with finger).
    Pointer,
    /// Text input cursor.
    Text,
}
```

#### Hit Zone Production (Compute Phase)

Hit zones are produced as part of `WidgetOutput` during
`compute_widget_outputs()`. This means hit testing uses the same geometry
as rendering -- no divergence possible. See `01-core-architecture.md`
Section 4 for the canonical compute pipeline.

```rust
/// Compute all annotation outputs for a symbol's visible annotations.
/// Returns a merged WidgetOutput containing fills, lines, markers,
/// labels, and hit_zones for all visible annotations.
fn compute_widget_outputs(
    annotations: &[Annotation],
    ctx: &ComputeContext,
    selected: Option<AnnotationId>,
) -> WidgetOutput {
    let mut merged = WidgetOutput::default();
    for annotation in annotations {
        if !annotation.presence.is_visible() {
            continue;
        }
        let mut output = match &annotation.kind {
            AnnotationKind::Level(level) => {
                compute_level(annotation.id, level, ctx, selected)
            }
            AnnotationKind::OrderBracket(bracket) => {
                compute_bracket(annotation.id, bracket, ctx, selected)
            }
            AnnotationKind::TextNote(note) => {
                compute_note(annotation.id, note, ctx, selected)
            }
            AnnotationKind::Marker(marker) => {
                compute_marker(annotation.id, marker, ctx, selected)
            }
        };
        // Apply presence alpha (Ghost = 0.4)
        if annotation.presence == Presence::Ghost {
            output.apply_alpha(0.4);
        }
        merged.merge(output);
    }
    merged
}
```

#### Hit Zone for a Horizontal Level

A horizontal level produces a single hit zone: a thin horizontal strip
spanning the full viewport width, centered on the level's Y position.

```rust
fn level_hit_zones(
    id: AnnotationId,
    level: &HorizontalLevel,
    camera: &Camera2D,
    viewport_width: u32,
) -> Vec<HitZone> {
    let y = camera.price_to_y(level.price);
    let tolerance = 6.0_f32; // LEVEL_HIT_TOLERANCE_PX

    vec![HitZone {
        annotation_id: id,
        kind: HitZoneKind::LevelLine,
        rect: [0.0, y - tolerance, viewport_width as f32, y + tolerance],
        cursor: CursorIcon::ResizeNS,
    }]
}
```

#### Hit Zone for an Order Bracket

A bracket produces multiple zones with different behaviors:

```rust
fn bracket_hit_zones(
    id: AnnotationId,
    bracket: &OrderBracket,
    camera: &Camera2D,
    viewport_width: u32,
    viewport_height: u32,
) -> Vec<HitZone> {
    let mut zones = Vec::with_capacity(6);
    let tolerance = 6.0_f32;
    let vw = viewport_width as f32;

    // Entry line -- dragging moves entire bracket.
    let entry_y = camera.price_to_y(bracket.entry.price);
    zones.push(HitZone {
        annotation_id: id,
        kind: HitZoneKind::BracketEntry,
        rect: [0.0, entry_y - tolerance, vw, entry_y + tolerance],
        cursor: CursorIcon::ResizeNS,
    });

    // TP line -- dragging adjusts TP only.
    if let Some(ref tp) = bracket.take_profit {
        let tp_y = camera.price_to_y(tp.price);
        zones.push(HitZone {
            annotation_id: id,
            kind: HitZoneKind::BracketTP,
            rect: [0.0, tp_y - tolerance, vw, tp_y + tolerance],
            cursor: CursorIcon::ResizeNS,
        });
    }

    // SL line -- dragging adjusts SL only.
    if let Some(ref sl) = bracket.stop_loss {
        let sl_y = camera.price_to_y(sl.price);
        zones.push(HitZone {
            annotation_id: id,
            kind: HitZoneKind::BracketSL,
            rect: [0.0, sl_y - tolerance, vw, sl_y + tolerance],
            cursor: CursorIcon::ResizeNS,
        });
    }

    // Zone fill regions (between entry and TP/SL) -- click to select, no drag.
    // These are lower priority than line hit zones.
    if let Some(ref tp) = bracket.take_profit {
        let entry_y = camera.price_to_y(bracket.entry.price);
        let tp_y = camera.price_to_y(tp.price);
        let top = entry_y.min(tp_y);
        let bottom = entry_y.max(tp_y);
        zones.push(HitZone {
            annotation_id: id,
            kind: HitZoneKind::BracketZone,
            rect: [0.0, top, vw, bottom],
            cursor: CursorIcon::Pointer,
        });
    }

    zones
}
```

#### Hit Test Query

The interaction layer queries hit zones in reverse order (topmost layer wins,
since later annotations render on top). Line zones take priority over fill
zones because they are more specific.

```rust
/// Priority order for hit zone resolution.
/// Lower number = higher priority (tested first).
/// Line-level zones (levels, bracket legs) beat zone fills.
fn zone_priority(kind: &HitZoneKind) -> u8 {
    match kind {
        // Individual lines are most specific -- highest priority
        HitZoneKind::LevelLine
        | HitZoneKind::BracketEntry
        | HitZoneKind::BracketTP
        | HitZoneKind::BracketSL => 0,
        // Point elements
        HitZoneKind::MarkerIcon => 0,
        // Text regions
        HitZoneKind::NoteBody => 1,
        // Fill/zone regions are least specific -- lowest priority
        HitZoneKind::BracketZone
        | HitZoneKind::VolumeProfileBar => 2,
    }
}

/// Find the highest-priority hit zone at screen position (x, y).
/// Returns None if the click is on empty chart space.
pub fn hit_test_zones(
    zones: &[HitZone],
    x: f32,
    y: f32,
) -> Option<(AnnotationId, HitZoneKind, f64)> {
    let mut best: Option<(AnnotationId, HitZoneKind, f64, u8)> = None;

    // Reverse iteration: later zones (rendered on top) checked first.
    for zone in zones.iter().rev() {
        let [left, top, right, bottom] = zone.rect;
        if x >= left && x <= right && y >= top && y <= bottom {
            let priority = zone_priority(&zone.kind);
            let is_better = match &best {
                None => true,
                Some((_, _, _, prev_priority)) => priority < *prev_priority,
            };
            if is_better {
                // Compute grab offset for drag operations.
                // For vertical drags, offset is the Y distance from zone center.
                let zone_center_y = (top + bottom) / 2.0;
                let grab_offset_px = y - zone_center_y;
                best = Some((zone.annotation_id, zone.kind, grab_offset_px as f64, priority));
            }
        }
    }

    best.map(|(id, kind, offset, _)| (id, kind, offset))
}
```

#### Full Hit Test Priority Order

When the user clicks, test in this order (most specific wins):

```
1. Volume handle triangle     (existing, right edge only)
2. Timeline border line        (existing, full width, +/-6px)
3. Annotation buttons          (close, flip -- small, high priority)
4. Annotation handles          (bracket legs, level lines -- +/-6px)
5. Annotation edges            (resize handles -- corner/edge boxes)
6. Annotation bodies           (fill zones, note bounding boxes)
7. Empty space                 -> begin PendingDrag (pan/zoom)
```

Steps 1-2 are existing checks in `handle_mouse_pressed()`. Steps 3-6 are a
single call to `hit_test_zones()`. Step 7 is the existing fallthrough.

### 2.3 Dragging

Dragging modifies annotation geometry in response to mouse movement. The
established grab-offset pattern prevents the annotation from jumping to the
cursor position on drag start.

#### Grab Offset Pattern

```
Before grab:        After grab (WITHOUT offset):    After grab (WITH offset):

  cursor             cursor                           cursor
    x                  x                                x
    |                  |                                |
    |  +-- level       +-- level (jumped!)              |  +-- level (stayed!)
    |                                                   |
```

The offset is the difference between the annotation's price and the cursor's
price at the moment the drag starts. On every subsequent move, the annotation
price is set to `cursor_price + offset`, preserving the relative position.

```rust
// At drag start:
let cursor_price = camera.y_to_price(cursor_y);
let grab_offset = annotation_price - cursor_price;

// During drag:
let new_cursor_price = camera.y_to_price(new_cursor_y);
let new_annotation_price = new_cursor_price + grab_offset;
```

For bracket entry lines where dragging moves the entire bracket, all legs
shift by the same delta:

```rust
// At drag start:
let entry_price = bracket.entry.price;
let grab_offset = entry_price - camera.y_to_price(cursor_y);

// During drag:
let new_entry = camera.y_to_price(cursor_y) + grab_offset;
let delta = new_entry - bracket.entry.price;

bracket.entry.price = new_entry;
if let Some(ref mut tp) = bracket.take_profit {
    tp.price += delta;
}
if let Some(ref mut sl) = bracket.stop_loss {
    sl.price += delta;
}
```

#### Drag Threshold

The existing 4px drag threshold (`DRAG_THRESHOLD_PX`) applies to annotation
drags. This prevents accidental drags when the user intends to click-select.

```
MousePressed at (x0, y0)
    |
    v
InteractionMode::PendingAnnotationDrag { start_x, start_y, id, zone, grab_offset }
    |
    +-- mouse moves < 4px --> still PendingAnnotationDrag
    |
    +-- mouse moves >= 4px --> InteractionMode::DraggingAnnotation { id, zone, grab_offset }
    |                              |
    |                              v  (on each subsequent MouseMoved)
    |                          emit ChartAction::DragAnnotation { id, zone, new_price }
    |
    +-- mouse released < 4px --> InteractionMode::Idle + ChartAction::SelectAnnotation { id }
```

#### Crosshair Suppression During Drag

The existing crosshair suppress/force_hide pattern applies. When a drag begins:

1. `state.crosshair.suppress()` -- hides the crosshair but preserves
   `left_mouse_down` so `on_left_release()` works correctly when the drag ends.
2. The crosshair resumes normal behavior after the drag via
   `state.crosshair.on_left_release()`.

This is identical to the existing level-drag behavior. No new crosshair
logic needed.

#### OHLC Snap During Drag

Annotation drags support OHLC snap using the same `LevelTool::snap_to_ohlc()`
logic. Since snap is a pure function of (raw_price, cursor_x, camera, data),
it can be extracted to a standalone function:

```rust
/// Snap a price to the nearest OHLC value within threshold.
/// Reusable by any tool or drag operation.
pub fn snap_to_ohlc(
    raw_price: f64,
    cursor_x: f32,
    camera: &Camera2D,
    data: &dyn CandleData,
    is_collapsed: bool,
    alt_held: bool,
) -> (f64, Option<f64>) {
    // Returns (effective_price, snapped_price_if_any)
    // Implementation identical to LevelTool::snap_to_ohlc() but
    // without mutating tool state.
    // ...
}
```

Alt key bypasses snap (existing behavior, carried forward).

#### Locked Annotations

Annotations with `locked: true` cannot be dragged. The hit test still finds
them (for selection and right-click context menu), but the drag initiation
path checks the lock flag:

```rust
if annotation.locked {
    // Can select, cannot drag.
    actions.push(ChartAction::SelectAnnotation { id });
    return actions;
}
```

### 2.4 Editing

Double-click on certain annotations enters edit mode:

| Annotation Type | Double-Click Behavior |
|---|---|
| Level | Opens inline price editor (iced text input overlay) |
| Bracket | Opens inline price editor on the clicked leg |
| Note | Enters text editing mode (cursor in text body) |
| Marker | Opens label editor popup |

**In-chart text input** uses iced overlay widgets, not GPU-rendered text.
The chart widget spawns an `iced::widget::TextInput` at the annotation's
screen position. The text input captures keyboard events until Enter (confirm)
or Escape (cancel).

```rust
pub enum ChartAction {
    // ... existing ...

    /// Enter edit mode for an annotation. The app layer spawns
    /// the appropriate iced overlay widget.
    BeginEdit {
        id: AnnotationId,
        /// Screen position for the editor widget.
        screen_x: f32,
        screen_y: f32,
        /// What to edit -- price, label, text body.
        field: EditField,
    },

    /// Commit an edit. The app layer updates the annotation.
    CommitEdit {
        id: AnnotationId,
        field: EditField,
        value: String,
    },

    /// Cancel an edit. Discard changes.
    CancelEdit,
}

#[derive(Clone, Debug, PartialEq)]
pub enum EditField {
    Price,
    Label,
    TextBody,
    Quantity,
}
```

### 2.5 Deletion

Selected annotations are deleted via the Delete key or context menu.

```rust
// In handle_key_pressed:
Key::Delete => {
    if let Some(id) = state.selected_annotation {
        let annotation = store.get(id);
        if annotation.map(|a| !a.locked).unwrap_or(false) {
            vec![ChartAction::DeleteAnnotation { id }]
        } else {
            vec![] // locked -- refuse to delete
        }
    } else if state.selected_level.is_some() {
        // Existing level deletion -- backward compat during migration.
        vec![ChartAction::DeleteSelectedLevel]
    } else {
        vec![]
    }
}
```

**Deletion guard for live orders:** When a bracket annotation is linked to
live broker orders, the app layer intercepts `DeleteAnnotation` and shows a
confirmation dialog before cancelling orders. This logic lives entirely in
`midas-app` -- the chart crate deletes without hesitation.

#### Undo/Redo (Future)

Actions are designed to be invertible for a future undo stack. Each action
that modifies annotations stores enough information to reverse itself:

```rust
/// A reversible action for the undo stack. Not implemented yet,
/// but the action format is designed to support it.
pub enum UndoableAction {
    CreateAnnotation { annotation: Annotation },
    DeleteAnnotation { annotation: Annotation }, // stores full copy for redo
    MoveAnnotation { id: AnnotationId, old_price: f64, new_price: f64 },
    EditAnnotation { id: AnnotationId, old_value: String, new_value: String, field: EditField },
}
```

Implementation is deferred. The action format is documented here so current
code does not introduce patterns that make undo impossible (e.g., destructive
deletes without storing the deleted annotation).

---

## 3. Drawing Tools

### 3.1 Tool State Machine Pattern

Every tool follows the pattern established by `CrosshairTool` and `LevelTool`:

1. **Self-contained struct** with a mode enum and tool-specific state.
2. **Lives as a field on ChartState** -- one instance per chart.
3. **Predicate methods** (`is_active()`, `is_placing()`, `is_dragging()`)
   instead of direct field access.
4. **Clean cancel/activate/suspend/resume API** for interaction with the
   modal system.
5. **No iced, no wgpu, no framework types** -- pure Rust data.

```rust
/// Template for a new drawing tool.
///
/// Each concrete tool follows this pattern:
///
/// pub struct FooTool {
///     pub mode: FooToolMode,
///     // ... tool-specific state ...
/// }
///
/// enum FooToolMode {
///     Idle,
///     PlacingFoo { ... },  // one or more placement steps
///     Dragging { ... },    // adjusting existing annotation
/// }
///
/// impl FooTool {
///     pub fn is_active(&self) -> bool { ... }
///     pub fn is_placing(&self) -> bool { ... }
///     pub fn is_dragging(&self) -> bool { ... }
///     pub fn activate(&mut self) { ... }
///     pub fn cancel(&mut self) { ... }
///     pub fn suspend_placing(&mut self) { ... }
///     pub fn try_resume_placing(&mut self) { ... }
/// }
```

### 3.2 Level Tool (Existing, Enhanced)

The current `LevelTool` remains unchanged in its state machine. When the
annotation system is implemented, the only change is where the placed level
is stored:

```
Before:  ChartAction::CreateLevel { price }
         -> state.levels.push(HorizontalLevel { ... })

After:   ChartAction::CreateLevel { price }
         -> state.annotations.insert(Annotation {
                kind: AnnotationKind::Level(HorizontalLevel { price, ... }),
                ...
            })
```

The tool itself does not know or care about storage. It produces the same
`ChartAction::CreateLevel` and the apply_action handler routes to the
appropriate store.

### 3.3 Order Bracket Tool

The bracket tool is the most complex drawing tool, requiring a multi-step
click sequence with preview rendering at each stage.

```rust
pub struct BracketTool {
    /// Current tool mode.
    pub mode: BracketToolMode,
    /// Trade direction -- set at activation time.
    pub side: BracketSide,
    /// Whether Alt is held (disables price snap).
    pub alt_held: bool,
    /// Preview price for the current step (snapped or raw).
    /// The compute layer reads this to render a ghost line at cursor.
    pub preview_price: Option<f64>,
    /// Whether the tool was in a placement phase before a pan/scale
    /// interruption. Mirrors LevelTool::was_placing.
    pub was_placing: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum BracketToolMode {
    /// Tool is not active.
    Idle,
    /// Waiting for the user to click the entry price.
    /// Preview: single ghost line at cursor Y.
    PlacingEntry,
    /// Entry price is set. Waiting for take-profit click.
    /// Preview: solid entry line + ghost TP line at cursor Y.
    PlacingTakeProfit {
        entry_price: f64,
        entry_time: i64,
    },
    /// Entry + TP set. Waiting for stop-loss click.
    /// Preview: solid entry + TP lines + ghost SL line at cursor Y
    /// + zone fills between entry-TP and entry-cursor.
    PlacingStopLoss {
        entry_price: f64,
        entry_time: i64,
        tp_price: f64,
    },
}

impl BracketTool {
    pub fn new() -> Self {
        Self {
            mode: BracketToolMode::Idle,
            side: BracketSide::Long,
            alt_held: false,
            preview_price: None,
            was_placing: false,
        }
    }

    pub fn is_active(&self) -> bool {
        !matches!(self.mode, BracketToolMode::Idle)
    }

    pub fn is_placing(&self) -> bool {
        matches!(
            self.mode,
            BracketToolMode::PlacingEntry
                | BracketToolMode::PlacingTakeProfit { .. }
                | BracketToolMode::PlacingStopLoss { .. }
        )
    }

    pub fn activate(&mut self, side: BracketSide) {
        self.mode = BracketToolMode::PlacingEntry;
        self.side = side;
        self.alt_held = false;
        self.preview_price = None;
        self.was_placing = false;
    }

    pub fn cancel(&mut self) {
        self.mode = BracketToolMode::Idle;
        self.alt_held = false;
        self.preview_price = None;
        self.was_placing = false;
    }

    /// Suspend placement during panning. The mode is kept intact
    /// (preserving the accumulated state like entry_price/tp_price).
    /// The interaction layer skips BracketTool input while
    /// InteractionMode is Panning -- no mode change needed here.
    pub fn suspend_placing(&mut self) {
        if self.is_placing() {
            self.was_placing = true;
        }
    }

    pub fn try_resume_placing(&mut self) {
        if self.was_placing {
            self.was_placing = false;
            // Mode was never changed -- just clear the flag.
        }
    }

    /// Validate a TP price relative to the entry and side.
    /// Returns true if the price is on the correct side.
    pub fn is_valid_tp(&self, tp_price: f64, entry_price: f64) -> bool {
        match self.side {
            BracketSide::Long => tp_price > entry_price,
            BracketSide::Short => tp_price < entry_price,
        }
    }

    /// Validate a SL price relative to the entry and side.
    pub fn is_valid_sl(&self, sl_price: f64, entry_price: f64) -> bool {
        match self.side {
            BracketSide::Long => sl_price < entry_price,
            BracketSide::Short => sl_price > entry_price,
        }
    }
}
```

#### Bracket Drawing State Machine

```
                    ┌──────────────────────────────┐
                    │                              │
                    │         BracketTool          │
                    │                              │
                    └──────────────────────────────┘

  ┌───────┐   activate(Long)   ┌───────────────┐
  │       │ ─────────────────> │               │
  │ Idle  │                    │ PlacingEntry  │
  │       │ <───────────────── │               │
  └───────┘   Escape/cancel    └───────┬───────┘
      ^                                │
      │                          click at P1
      │                                │
      │                                v
      │   Escape/cancel    ┌───────────────────┐
      │ <───────────────── │                   │
      │                    │ PlacingTakeProfit  │
      │                    │ { entry: P1 }     │
      │                    └─────────┬─────────┘
      │                              │
      │                        click at P2
      │                     (P2 > P1 for Long)
      │                              │
      │                              v
      │   Escape/cancel    ┌───────────────────┐
      │ <───────────────── │                   │
      │                    │ PlacingStopLoss   │
      │                    │ { entry: P1,      │
      │                    │   tp: P2 }        │
      │                    └─────────┬─────────┘
      │                              │
      │                        click at P3
      │                     (P3 < P1 for Long)
      │                              │
      │                              v
      │                    emit CreateBracket { entry: P1, tp: P2, sl: P3 }
      │                              │
      └──────────────────────────────┘
```

**Shortcuts within the state machine:**

| Key | In PlacingTakeProfit | In PlacingStopLoss |
|---|---|---|
| Enter | Skip TP, go to PlacingStopLoss | Complete bracket (no SL) |
| Tab | Toggle side (Long <-> Short) | Toggle side (Long <-> Short) |
| Escape | Cancel entire drawing | Cancel entire drawing |
| Ctrl+click | Skip current leg | Skip current leg |

### 3.4 Tool Activation

Tools are activated from the toolbar, keyboard shortcuts, or programmatically
(e.g., quick-bracket from watchlist). Only one tool is active at a time.
Activating a new tool deactivates the current one.

```rust
/// Which drawing tool is currently active.
/// Lives on ChartState. Only one active at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActiveTool {
    /// No tool active. Default state.
    None,
    /// Horizontal level placement (existing).
    Level,
    /// Order bracket drawing (new).
    OrderBracket,
    /// Trend line drawing (future).
    TrendLine,
    /// Text note placement (future).
    TextNote,
    /// Measurement tool (future).
    Measure,
}
```

Tool activation flow:

```rust
fn activate_tool(state: &mut ChartState, tool: ActiveTool) {
    // Deactivate current tool first.
    deactivate_current_tool(state);

    match tool {
        ActiveTool::None => {}
        ActiveTool::Level => {
            state.level_tool.activate();
        }
        ActiveTool::OrderBracket => {
            state.bracket_tool.activate(BracketSide::Long);
        }
        ActiveTool::TrendLine => { /* future */ }
        ActiveTool::TextNote => { /* future */ }
        ActiveTool::Measure => { /* future */ }
    }

    state.active_tool = tool;
}

fn deactivate_current_tool(state: &mut ChartState) {
    match state.active_tool {
        ActiveTool::None => {}
        ActiveTool::Level => state.level_tool.cancel(),
        ActiveTool::OrderBracket => state.bracket_tool.cancel(),
        ActiveTool::TrendLine => { /* future */ }
        ActiveTool::TextNote => { /* future */ }
        ActiveTool::Measure => { /* future */ }
    }
    state.active_tool = ActiveTool::None;
}
```

### 3.5 Tool Preview Rendering

While a tool is active, the compute phase renders a "ghost" preview of the
annotation being created. This preview follows the cursor and shows what
will be placed on the next click.

Previews are:
- Computed by `compute_chart_scene()` like any annotation.
- Styled differently (lower opacity, dashed lines) to distinguish from
  committed annotations.
- NOT persisted to AnnotationStore -- they exist only in the ChartScene
  render output.
- NOT hit-testable -- they produce no HitZones.

```rust
/// Produce preview WidgetOutput for the active drawing tool.
/// Returns empty output if no tool is active or preview is not applicable.
fn compute_tool_preview(
    state: &ChartState,
    camera: &Camera2D,
    viewport_width: u32,
) -> WidgetOutput {
    match state.active_tool {
        ActiveTool::Level => {
            let mut output = WidgetOutput::default();
            if let Some(price) = state.level_tool.preview_price {
                let y = camera.price_to_y(price);
                output.lines.push(GridLineInstance {
                    rect: [0.0, y - 0.5, viewport_width as f32, y + 0.5],
                    color: [0.5, 0.5, 0.5, 0.4], // ghost gray
                });
            }
            output
        }
        ActiveTool::OrderBracket => {
            compute_bracket_preview(&state.bracket_tool, camera, viewport_width)
        }
        _ => WidgetOutput::default(),
    }
}

fn compute_bracket_preview(
    tool: &BracketTool,
    camera: &Camera2D,
    viewport_width: u32,
) -> WidgetOutput {
    let vw = viewport_width as f32;
    let ghost_color = [0.5, 0.5, 0.5, 0.3];
    let entry_color = match tool.side {
        BracketSide::Long => [0.15, 0.65, 0.35, 0.6],
        BracketSide::Short => [0.65, 0.15, 0.15, 0.6],
    };
    let mut output = WidgetOutput::default();

    match &tool.mode {
        BracketToolMode::Idle => {}
        BracketToolMode::PlacingEntry => {
            if let Some(price) = tool.preview_price {
                let y = camera.price_to_y(price);
                output.lines.push(GridLineInstance {
                    rect: [0.0, y - 0.5, vw, y + 0.5],
                    color: ghost_color,
                });
            }
        }
        BracketToolMode::PlacingTakeProfit { entry_price, .. } => {
            let entry_y = camera.price_to_y(*entry_price);
            output.lines.push(GridLineInstance {
                rect: [0.0, entry_y - 0.5, vw, entry_y + 0.5],
                color: entry_color,
            });
            if let Some(price) = tool.preview_price {
                let y = camera.price_to_y(price);
                output.lines.push(GridLineInstance {
                    rect: [0.0, y - 0.5, vw, y + 0.5],
                    color: ghost_color,
                });
            }
        }
        BracketToolMode::PlacingStopLoss { entry_price, tp_price, .. } => {
            let entry_y = camera.price_to_y(*entry_price);
            output.lines.push(GridLineInstance {
                rect: [0.0, entry_y - 0.5, vw, entry_y + 0.5],
                color: entry_color,
            });
            let tp_y = camera.price_to_y(*tp_price);
            let tp_color = match tool.side {
                BracketSide::Long => [0.15, 0.65, 0.35, 0.5],
                BracketSide::Short => [0.65, 0.15, 0.15, 0.5],
            };
            output.lines.push(GridLineInstance {
                rect: [0.0, tp_y - 0.5, vw, tp_y + 0.5],
                color: tp_color,
            });
            if let Some(price) = tool.preview_price {
                let y = camera.price_to_y(price);
                output.lines.push(GridLineInstance {
                    rect: [0.0, y - 0.5, vw, y + 0.5],
                    color: ghost_color,
                });
            }
        }
    }

    output
}
```

### 3.6 CursorClaim Extension

The existing `CursorClaim` system on `ChartState` must account for the
bracket tool:

```rust
impl ChartState {
    /// What does the currently active tool need from the crosshair?
    pub fn active_cursor_claim(&self) -> CursorClaim {
        // Check each tool in priority order.
        // Suppress takes priority over Preview.
        if self.level_tool.is_dragging() {
            return CursorClaim::Suppress;
        }
        if self.level_tool.is_placing() {
            return CursorClaim::Suppress;
        }
        if self.bracket_tool.is_placing() {
            return CursorClaim::Suppress;
        }
        // Future tools checked here.

        CursorClaim::None
    }
}
```

---

## 4. Order Bracket Interaction (Complex Case)

### 4.1 Components of an Order Bracket

An order bracket is the most interaction-rich annotation type. It produces
multiple independent hit zones, each with different drag semantics.

```rust
pub struct OrderBracket {
    /// Entry price line. Always present.
    pub entry: BracketLeg,
    /// Take-profit target. Optional -- user can add/remove later.
    pub take_profit: Option<BracketLeg>,
    /// Stop-loss level. Optional -- user can add/remove later.
    pub stop_loss: Option<BracketLeg>,
    /// Trade direction. Determines which side TP/SL fall on.
    pub side: BracketSide,
    /// Visual status. Chart crate uses for styling only.
    /// App layer maps this to/from broker order state.
    pub status: BracketStatus,
    /// Display quantity. Informational label only.
    pub quantity: Option<f64>,
}
```

### 4.2 Interactive Elements Per Bracket

Each bracket produces up to 6 hit zones:

| Element | HitZoneKind | Drag Behavior | Cursor |
|---|---|---|---|
| Entry line | Handle(0) | Move entire bracket (all legs shift) | ResizeNS |
| TP line | Handle(1) | Move TP only | ResizeNS |
| SL line | Handle(2) | Move SL only | ResizeNS |
| TP zone fill | Body | Select bracket (no drag) | Pointer |
| SL zone fill | Body | Select bracket (no drag) | Pointer |
| Close button | Button(0) | Click to delete/cancel | Pointer |

### 4.3 Bracket Leg Drag Constraints

When a user drags a bracket leg, constraints enforce that legs stay on the
correct side of the entry:

```rust
/// Constrain a bracket leg price after a drag.
/// If the user drags past the entry, legs swap rather than clamp.
fn constrain_bracket_leg(
    bracket: &mut OrderBracket,
    leg: BracketLegKind,
    new_price: f64,
) {
    match leg {
        BracketLegKind::Entry => {
            // Entry moves freely. If it crosses TP or SL, swap them.
            let delta = new_price - bracket.entry.price;
            bracket.entry.price = new_price;

            // Check if entry crossed TP.
            if let Some(ref tp) = bracket.take_profit {
                let should_swap = match bracket.side {
                    BracketSide::Long => new_price >= tp.price,
                    BracketSide::Short => new_price <= tp.price,
                };
                if should_swap {
                    // TP becomes SL, SL becomes TP.
                    std::mem::swap(&mut bracket.take_profit, &mut bracket.stop_loss);
                    bracket.side = match bracket.side {
                        BracketSide::Long => BracketSide::Short,
                        BracketSide::Short => BracketSide::Long,
                    };
                }
            }
        }
        BracketLegKind::TakeProfit => {
            if let Some(ref mut tp) = bracket.take_profit {
                tp.price = new_price;
                // If TP crossed entry, swap it with SL.
                let crossed = match bracket.side {
                    BracketSide::Long => new_price <= bracket.entry.price,
                    BracketSide::Short => new_price >= bracket.entry.price,
                };
                if crossed {
                    std::mem::swap(&mut bracket.take_profit, &mut bracket.stop_loss);
                }
            }
        }
        BracketLegKind::StopLoss => {
            if let Some(ref mut sl) = bracket.stop_loss {
                sl.price = new_price;
                let crossed = match bracket.side {
                    BracketSide::Long => new_price >= bracket.entry.price,
                    BracketSide::Short => new_price <= bracket.entry.price,
                };
                if crossed {
                    std::mem::swap(&mut bracket.take_profit, &mut bracket.stop_loss);
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BracketLegKind {
    Entry,
    TakeProfit,
    StopLoss,
}
```

The swap-on-cross behavior is natural from a trading UX perspective: the user
drags a level past the entry, and it transforms from TP to SL (or vice versa).
No leg is ever "lost" or clamped.

### 4.4 In-Chart UI Elements (iced Overlay)

Order brackets can spawn iced overlay widgets for elements that cannot be
GPU-rendered (text input fields, buttons, dropdowns):

```
┌──────────────────────────────────────────────────────────────┐
│                                                              │
│  TP ────────────────────────────  ┌─────────┐                │
│                                   │ 192.00  │  <-- price     │
│                                   │  R:R 3:1│      badge     │
│                    (green zone)   └─────────┘                │
│                                                              │
│  Entry ─────────────────────────  ┌─────────┐  [x]          │
│                                   │ 185.50  │  <-- close     │
│                                   │  100 qty│      button    │
│                    (red zone)     └─────────┘                │
│                                                              │
│  SL ────────────────────────────  ┌─────────┐                │
│                                   │ 182.00  │                │
│                                   └─────────┘                │
└──────────────────────────────────────────────────────────────┘
```

The iced overlay elements:
- **Price badges** on the Y axis (same as crosshair price badge mechanism).
- **R:R ratio label** between TP and entry.
- **Quantity label** next to entry.
- **Close button** [x] at the right end of the entry line.

These are positioned using screen coordinates from `compute_chart_scene()`.
The chart crate provides the positions; `midas-app` renders the widgets.

### 4.5 Bracket <-> Broker Connection

The lifecycle is fully documented in `02-storage-and-sync.md` Section 5.
Summary relevant to interaction:

| Bracket Status | Interaction Allowed |
|---|---|
| Draft | Full: drag, edit, delete, submit |
| Pending | Drag legs (triggers order modify), cancel, delete |
| Active | Drag TP/SL (triggers order modify), cannot move entry |
| PartialFill | Same as Active |
| Closed | Read-only. Cannot drag, edit, or delete. |
| Cancelled | Read-only. Can delete (removes from chart). |

The app layer checks `bracket.status` before allowing interaction:

```rust
// In midas-app, when handling ChartAction::DragAnnotation:
fn handle_drag_annotation(
    id: AnnotationId,
    new_price: f64,
    store: &mut AnnotationStore,
    links: &OrderAnnotationLinks,
) -> Result<()> {
    let annotation = store.get_mut(id).context("annotation not found")?;
    if let AnnotationKind::OrderBracket(ref bracket) = annotation.kind {
        match bracket.status {
            BracketStatus::Closed | BracketStatus::Cancelled => {
                return Ok(()); // read-only, ignore drag
            }
            BracketStatus::Active | BracketStatus::PartialFill => {
                // Modifying a live order -- will need broker command.
                // Handled after annotation update.
            }
            BracketStatus::Draft | BracketStatus::Pending => {
                // Free to modify.
            }
        }
    }
    // ... apply the drag ...
    Ok(())
}
```

---

## 5. Widget-Specific Interactions

### 5.1 Horizontal Level

Horizontal levels are the simplest annotation type and serve as the
reference implementation for the interaction pattern.

| Gesture | Behavior |
|---|---|
| H key | Activate level placement tool |
| Mouse move (placing) | Preview line follows cursor Y, OHLC snap |
| Left click (placing) | Place level at (snapped) price |
| Alt+move (placing) | Bypass OHLC snap |
| Left click on level | Select it |
| Left drag on level | Drag to new price (grab offset pattern) |
| Right click on level | Context menu: edit color, label, delete, lock |
| Delete key (selected) | Remove the level |
| Double-click on level | Inline price editor |
| Escape (placing) | Cancel placement, return to Idle |

All of this behavior already exists in the current codebase. The annotation
system wraps it without changing the user-facing interaction.

### 5.2 Volume Profile

Volume Profile annotations are non-interactive display elements -- they
behave more like indicators than user-drawn widgets.

| Gesture | Behavior |
|---|---|
| Hover over VP bar | Highlight individual bar, show value tooltip |
| Click on VP | No selection (VP is not selectable in v1) |
| Right-click on VP | Context menu: settings panel (period, colors) |
| Drag on VP | No drag behavior |

Future work: click-drag to adjust the VP's visible time range (select a
custom period). This requires a `Span` anchor type.

### 5.3 Indicator Overlays (ATR, Moving Averages)

Indicator overlays are purely display. They have no annotation backing --
they are computed from candle data each frame.

| Gesture | Behavior |
|---|---|
| Hover over indicator line | Tooltip with indicator name + value at cursor time |
| Click on indicator line | No selection |
| Right-click on indicator line | Context menu: settings (period, color), toggle visibility |

Indicators do not produce HitZones. Their tooltip behavior is handled
separately by the overlay system, which checks proximity to indicator
values at the current cursor X position.

### 5.4 Text Notes

| Gesture | Behavior |
|---|---|
| Click on note | Select it |
| Drag on note body | Move to new price/time (grab offset in both axes) |
| Double-click on note | Enter text editing mode (iced TextInput overlay) |
| Delete key (selected) | Remove the note |
| Right-click on note | Context menu: edit, change color, delete |

Notes use a `Point` anchor (price + timestamp) and produce a single Body
hit zone sized to the text bounding box.

### 5.5 Markers

| Gesture | Behavior |
|---|---|
| Hover over marker | Tooltip with label text |
| Click on marker | Select it |
| Drag on marker | Move to new price/time (if not locked) |
| Delete key (selected) | Remove the marker (if not locked) |
| Right-click on marker | Context menu: edit label, change icon, delete |

Historical fill markers are `locked: true` and cannot be moved or deleted.

---

## 6. Keyboard Shortcuts

### 6.1 Global Chart Shortcuts

These work regardless of tool state:

| Key | Action | Notes |
|---|---|---|
| Delete | Remove selected annotation(s) | Respects lock flag |
| Escape | Cancel current tool / deselect | Cascading: tool first, then selection |
| Home | Jump to oldest data | Existing |
| End | Jump to newest data | Existing |
| Ctrl+Z | Undo | Future |
| Ctrl+Y | Redo | Future |

### 6.2 Tool Activation Shortcuts

These activate a drawing tool. Only work when no tool is active (Idle).

| Key | Action |
|---|---|
| H | Activate horizontal level tool (existing) |
| L | Activate horizontal level tool (alias) |
| B | Activate bracket tool (Long side) |
| Shift+B | Activate bracket tool (Short side) |
| N | Activate text note placement (future) |
| M | Activate marker placement (future) |

### 6.3 In-Tool Shortcuts

These work only while a specific tool is active:

| Key | Tool | Action |
|---|---|---|
| Enter | Bracket | Complete bracket with legs placed so far |
| Tab | Bracket | Toggle Long <-> Short side |
| Ctrl+click | Bracket | Skip current leg |
| Alt (held) | Level/Bracket | Bypass OHLC snap |
| Escape | Any | Cancel tool, return to Idle |

### 6.4 Selection Navigation (Future)

| Key | Action |
|---|---|
| Tab | Cycle forward through annotations |
| Shift+Tab | Cycle backward through annotations |
| Ctrl+A | Select all annotations |

### 6.5 Key Enum Extension

```rust
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Key {
    // Existing:
    Delete,
    Escape,
    Home,
    End,
    H,
    // New:
    L,
    B,
    N,
    M,
    Enter,
    Tab,
}
```

Modifier state (Shift, Ctrl, Alt) is tracked separately per event, not
encoded into the Key enum. This matches the existing `alt_held: bool`
pattern on `ChartEvent::MouseMoved`.

---

## 7. Cursor Management

The cursor icon communicates what interaction is available before the user
acts. The iced widget adapter reads the `CursorIcon` from the chart state
and applies it via `iced::mouse::Interaction`.

### 7.1 Cursor Priority

Multiple systems may want to set the cursor. Priority order (highest first):

```
1. Active tool (placing/drawing)     -> Crosshair
2. Active drag (panning, scaling)    -> Grabbing / ResizeNS / ResizeEW
3. Hover over hit zone               -> Zone's cursor icon
4. Default chart area                -> Crosshair
```

```rust
impl ChartState {
    /// Determine the cursor icon for the current frame.
    /// Called by the iced widget adapter during view().
    pub fn cursor_icon(&self) -> CursorIcon {
        // Priority 1: active tool always shows crosshair.
        if self.level_tool.is_active() || self.bracket_tool.is_active() {
            return CursorIcon::Crosshair;
        }

        // Priority 2: active drag.
        match &self.interaction_mode {
            InteractionMode::Panning | InteractionMode::RightPanning => {
                return CursorIcon::Grabbing;
            }
            InteractionMode::HorizontalScaling { .. } => {
                return CursorIcon::ResizeEW;
            }
            InteractionMode::VerticalScaling { .. } => {
                return CursorIcon::ResizeNS;
            }
            InteractionMode::DraggingTimelineBorder { .. }
            | InteractionMode::DraggingVolumeScale { .. } => {
                return CursorIcon::ResizeNS;
            }
            _ => {}
        }

        // Priority 3: hover over hit zone.
        if let Some(ref hover) = self.hovered_zone {
            return hover.cursor;
        }

        // Priority 4: default.
        CursorIcon::Crosshair
    }
}
```

### 7.2 Hover Tracking

The `MouseMoved` handler tests hit zones each frame to track which zone the
cursor is over. This drives the cursor icon AND enables tooltip display.

```rust
/// Updated on each MouseMoved. Stores the zone the cursor is over,
/// if any. Used for cursor icon and tooltip display.
pub struct HoverState {
    pub annotation_id: AnnotationId,
    pub kind: HitZoneKind,
    pub cursor: CursorIcon,
}

// In handle_mouse_moved, after crosshair update:
if matches!(state.interaction_mode, InteractionMode::Idle) {
    state.hovered_zone = hit_test_zones(&state.hit_zones, x, y)
        .map(|(id, kind, _offset)| {
            let cursor = state.hit_zones.iter()
                .find(|hz| hz.annotation_id == id && hz.kind == kind)
                .map(|hz| hz.cursor)
                .unwrap_or(CursorIcon::Crosshair);
            HoverState { annotation_id: id, kind, cursor }
        });
}
```

### 7.3 Cursor Icon Mapping

| Context | Cursor | Rationale |
|---|---|---|
| Default chart area | Crosshair | Standard charting cursor |
| Over level line | ResizeNS | Indicates vertical drag |
| Over bracket entry line | ResizeNS | Indicates vertical drag |
| Over bracket TP/SL line | ResizeNS | Indicates vertical drag |
| Over bracket zone fill | Pointer | Click to select, no drag |
| Over note body | Grab | Indicates draggable |
| Over marker icon | Pointer | Click to select |
| Over close button | Pointer | Click to activate |
| During pan drag | Grabbing | Active drag in progress |
| During annotation drag | Grabbing | Active drag in progress |
| During horizontal scale | ResizeEW | Stretching time axis |
| During vertical scale | ResizeNS | Stretching price axis |
| Tool active (placing) | Crosshair | Precise placement |

---

## 8. Future: Composable ChartModifier

This section documents the migration path from the current monolithic
`handle_event()` to a composable modifier system. This is NOT immediate
work -- it is reference material for when the triggers from section 1.3
are hit.

### 8.1 The Problem That Triggers Migration

As tools multiply, `handle_event()` grows a match arm for every combination
of (event, mode, tool). The function becomes a combinatorial explosion:

```
                 Current (manageable):
                 10 modes x 5 event types = ~50 branches

                 After 5 more tools:
                 25 modes x 8 event types = ~200 branches
```

More critically, sub-behaviors start duplicating. Three different tools all
need "OHLC snap on mouse move while placing" but with slightly different
follow-up actions. Extracting to a shared function works until the function
needs access to tool-specific state.

### 8.2 The ChartModifier Pattern

Inspired by SciChart's approach. Each modifier is a trait object that
handles a subset of events and produces actions:

```rust
/// A composable behavior that handles a subset of chart events.
/// Multiple modifiers can be stacked on a chart.
trait ChartModifier: Send + Sync {
    /// Priority for event dispatch. Lower = handled first.
    fn priority(&self) -> u32;

    /// Can this modifier handle the given event in the current state?
    /// If true, the event is dispatched to handle_event().
    /// If false, the event is passed to the next modifier.
    fn wants_event(&self, event: &ChartEvent, state: &ChartState) -> bool;

    /// Handle an event. Returns actions and whether the event was consumed.
    /// Consumed events are not passed to lower-priority modifiers.
    fn handle_event(
        &mut self,
        event: ChartEvent,
        state: &mut ChartState,
        data: Option<&dyn CandleData>,
    ) -> (Vec<ChartAction>, bool);

    /// Called each frame to produce hover/preview state.
    fn on_hover(
        &self,
        x: f32,
        y: f32,
        state: &ChartState,
    ) -> Option<CursorIcon>;
}
```

### 8.3 Modifier Stack Example

```rust
struct ChartModifierStack {
    modifiers: Vec<Box<dyn ChartModifier>>,
}

impl ChartModifierStack {
    fn handle_event(
        &mut self,
        event: ChartEvent,
        state: &mut ChartState,
        data: Option<&dyn CandleData>,
    ) -> Vec<ChartAction> {
        let mut all_actions = Vec::new();
        for modifier in &mut self.modifiers {
            if modifier.wants_event(&event, state) {
                let (actions, consumed) = modifier.handle_event(
                    event.clone(),
                    state,
                    data,
                );
                all_actions.extend(actions);
                if consumed {
                    break;
                }
            }
        }
        all_actions
    }
}
```

### 8.4 Migration Steps

1. **Extract sub-behaviors to functions** (already partially done with
   `handle_mouse_moved`, `handle_mouse_pressed`, etc.).
2. **Group related behaviors** into structs: PanModifier, ZoomModifier,
   LevelToolModifier, BracketToolModifier, SelectionModifier.
3. **Implement ChartModifier trait** for each struct.
4. **Replace handle_event()** with a ChartModifierStack.
5. **Test thoroughly** -- the behavior must be identical before and after.

The key insight is that this is a mechanical refactor, not a redesign. All
the behaviors already exist. The modifier pattern just gives them a uniform
interface and explicit priority ordering.

### 8.5 When NOT to Migrate

Do not migrate to ChartModifier if:
- The monolithic function is under 1500 lines and readable.
- There are fewer than 15 interaction modes.
- New tools can still be added by inserting a match arm without touching
  existing arms.

The simpler architecture wins until it demonstrably does not.

---

## 9. Extended InteractionMode Enum

For reference, the complete `InteractionMode` enum after widget integration.
New variants are marked with `[NEW]`.

```rust
#[derive(Clone, Debug, PartialEq)]
pub enum InteractionMode {
    // ── Existing (unchanged) ─────────────────────────────────
    Idle,
    PendingDrag { start_x: f32, start_y: f32 },
    Panning,
    PendingScale { start_x: f32, start_y: f32 },
    HorizontalScaling { anchor_x: f32, last_x: f32 },
    VerticalScaling { anchor_y: f32, last_y: f32 },
    RightPanning,
    DraggingTimelineBorder { anchor_y: f32, start_ratio: f32 },
    DraggingVolumeScale { anchor_y: f32, start_scale: f32 },

    // ── New annotation modes ─────────────────────────────── [NEW]

    /// Mouse is down on an annotation but has not yet moved past the
    /// drag threshold. Will resolve to either DraggingAnnotation
    /// (if moved >= 4px) or SelectAnnotation (if released < 4px).
    PendingAnnotationDrag {
        start_x: f32,
        start_y: f32,
        annotation_id: AnnotationId,
        kind: HitZoneKind,
        grab_offset: f64,
    },

    /// Actively dragging an annotation element.
    DraggingAnnotation {
        annotation_id: AnnotationId,
        kind: HitZoneKind,
        grab_offset: f64,
    },

    /// Multi-step bracket drawing. The BracketTool struct holds the
    /// phase details (PlacingEntry, PlacingTP, PlacingSL).
    /// This mode exists so the interaction layer knows to route events
    /// to the bracket tool rather than normal click/drag handling.
    DrawingBracket,

    /// Placing a text note at a point. Single click to place,
    /// then iced overlay opens for text input.
    PlacingNote,

    /// Placing a marker at a point. Single click to place.
    PlacingMarker { icon: MarkerIcon },
}
```

### 9.1 Extended ChartAction Enum

New action variants for the annotation system:

```rust
pub enum ChartAction {
    // ── Existing (unchanged) ─────────────────────────────────
    Pan { dx: f64, dy: f64 },
    Zoom { center_x: f32, factor: f64 },
    ZoomY { center_y: f32, factor: f64 },
    SetCrosshair { x: f32, y: f32 },
    ClearCrosshair,
    AutoScaleY { target_low: f64, target_high: f64 },
    StartMomentum { vx: f64, vy: f64 },
    ApplyMomentum { dt: f64, dp: f64 },
    StopMomentum,
    CreateLevel { price: f64 },
    SelectLevel { id: u64 },
    DragLevel { id: u64, new_price: f64 },
    DeleteSelectedLevel,
    DeselectLevel,
    RightClickLevel { id: u64, x: f32, y: f32 },
    JumpToEnd,
    JumpToStart,
    SetTimelineBorderRatio { ratio: f64 },
    SetVolumeScale { scale: f64 },
    Redraw,
    CancelPlacing,
    PlacingPreview { price: f64 },

    // ── New annotation actions ──────────────────────────────

    /// Select an annotation. Replaces current selection.
    SelectAnnotation { id: AnnotationId },

    /// Clear the current annotation selection.
    DeselectAnnotation,

    /// Delete a specific annotation.
    DeleteAnnotation { id: AnnotationId },

    /// Drag an annotation element to a new price.
    DragAnnotation {
        id: AnnotationId,
        kind: HitZoneKind,
        new_price: f64,
    },

    /// Create a bracket annotation from the drawing tool.
    CreateBracket {
        side: BracketSide,
        entry_price: f64,
        entry_time: i64,
        tp_price: Option<f64>,
        sl_price: Option<f64>,
    },

    /// Create a text note at a chart point.
    CreateNote {
        price: f64,
        timestamp: i64,
    },

    /// Create a marker at a chart point.
    CreateMarker {
        price: f64,
        timestamp: i64,
        icon: MarkerIcon,
    },

    /// Right-click on an annotation -- context menu.
    RightClickAnnotation {
        id: AnnotationId,
        screen_x: f32,
        screen_y: f32,
    },

    /// Begin inline editing of an annotation field.
    BeginEdit {
        id: AnnotationId,
        screen_x: f32,
        screen_y: f32,
        field: EditField,
    },

    /// Toggle the lock state of an annotation.
    ToggleLock { id: AnnotationId },

    /// Toggle the visibility of an annotation.
    ToggleVisibility { id: AnnotationId },
}
```

---

## 10. Implementation Sequence

This document describes the target architecture. Implementation should be
phased to maintain a working system at each step.

### Phase 1: Annotation Selection and Dragging

1. Add `selected_annotation` and `hit_zones` to `ChartState`.
2. Add `HitZone` and `HitZoneKind` types to `annotations/hit_test.rs`.
3. Produce hit zones from `compute_chart_scene()` for existing levels.
4. Add `PendingAnnotationDrag` and `DraggingAnnotation` to `InteractionMode`.
5. Handle annotation hit testing in `handle_mouse_pressed()`.
6. Handle annotation dragging in `handle_mouse_moved()`.
7. **Tests**: selection click, deselection click, drag with offset,
   drag threshold, locked annotation refuses drag.

### Phase 2: BracketTool Drawing

1. Implement `BracketTool` struct and `BracketToolMode` enum.
2. Add `bracket_tool` field to `ChartState`.
3. Add `DrawingBracket` to `InteractionMode`.
4. Handle bracket drawing events in `handle_mouse_pressed()` and
   `handle_mouse_moved()`.
5. Produce bracket preview in `compute_tool_preview()`.
6. Emit `CreateBracket` action on completion.
7. **Tests**: full drawing flow (3 clicks), escape at each phase,
   side constraint validation, side toggle with Tab.

### Phase 3: Bracket Leg Dragging

1. Produce per-leg hit zones from `compute_bracket()`.
2. Route `DraggingAnnotation { zone: Handle(N) }` to bracket-specific
   drag logic.
3. Implement `constrain_bracket_leg()` with swap-on-cross.
4. **Tests**: drag entry (whole bracket moves), drag TP/SL individually,
   cross-entry swap behavior.

### Phase 4: Cursor Management and Polish

1. Implement `cursor_icon()` on `ChartState`.
2. Wire `CursorIcon` to `iced::mouse::Interaction` in the widget adapter.
3. Implement hover tracking and tooltip positions.
4. Add keyboard shortcuts for tool activation.
5. **Tests**: cursor icon changes on hover, keyboard shortcuts activate
   correct tools.

---

## Appendix A: Interaction State Machine Diagram

Complete state transition diagram for the annotation interaction system:

```
                              ┌─────────────────┐
                              │                 │
                   ┌─────────>│      Idle       │<───────────────────┐
                   │          │                 │                    │
                   │          └───┬───┬───┬─────┘                    │
                   │              │   │   │                          │
                   │   left-click │   │   │ activate tool            │
                   │   on ann.    │   │   │                          │
                   │              v   │   v                          │
                   │   ┌──────────────┐   ┌──────────────────┐      │
                   │   │PendingAnn-   │   │DrawingBracket    │      │
           release │   │Drag          │   │(tool owns phase) │      │
           <4px    │   │{id,zone,off} │   └────────┬─────────┘      │
           =select │   └───┬──────────┘            │                │
                   │       │ move >= 4px       complete/cancel      │
                   │       v                       │                │
                   │   ┌──────────────┐            │                │
                   │   │DraggingAnn-  │            │                │
                   │   │otation       │────────────┘                │
                   │   │{id,zone,off} │                             │
                   │   └──────┬───────┘                             │
                   │          │ release                             │
                   │          │                                     │
                   └──────────┘─────────────────────────────────────┘
```

## Appendix B: Design Decisions Log

| Decision | Rationale | Alternative Considered |
|---|---|---|
| Selection on ChartState, not Annotation | View-specific; two charts can differ | `selected: bool` on Annotation |
| HitZones computed in compute phase | Same geometry as rendering; no divergence | Re-compute in interaction handler |
| Swap-on-cross for bracket legs | Natural trading UX; no leg "lost" | Clamp to entry price |
| Monolithic handler (for now) | Simpler to read/debug at current scale | ChartModifier trait objects |
| Grab offset in price space, not pixels | Stable across zoom changes during drag | Pixel-space offset |
| Preview annotations not in AnnotationStore | Previews are transient; store is persistent | Store with `preview: bool` flag |
| Single selection (v1) | Simplicity; multi-select adds significant complexity | Immediate multi-select |
| Tool deactivation is explicit | Prevents orphaned tool state | Auto-deactivate on click |
