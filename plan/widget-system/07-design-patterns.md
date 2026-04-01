# 07 -- Design Patterns & Conventions

> Reusable patterns for implementing widgets in the Hand of Midas chart system.
> Reference implementations: `CrosshairTool`, `HorizontalLevel` / `LevelTool`, `GerchikAtr`.
> Target audience: developers adding new interactive or display-only chart widgets.

---

## 1. The Seven-Layer Widget Architecture

Every widget in the system follows a layered decomposition. Not every widget
needs all seven layers -- a display-only indicator like GerchikAtr skips
layers 2, 3, and 4 -- but the layers always appear in this order when present.

```
Layer 1: Data Type          -- what the widget IS (pure data, serde)
Layer 2: State Machine      -- how the tool BEHAVES (mode enum, private)
Layer 3: Interaction        -- how the user DRIVES it (ChartAction + handle_event)
Layer 4: Computation        -- how it becomes GEOMETRY (sans-IO, no wgpu)
Layer 5: Render Types       -- what the GPU CONSUMES (Pod) + overlay DISPLAYS (Clone)
Layer 6: Scene Assembly     -- where it APPEARS in ChartScene
Layer 7: View Overlay       -- how iced DRAWS labels/badges on top
```

---

### Layer 1: Data Type

**File:** `midas-chart/src/widget/your_widget.rs` (or `levels.rs`, `gerchik_atr.rs`)

The data type defines the persistent, serializable identity of the widget.
It is a plain Rust struct with no behavior beyond construction and serde.

```rust
/// A user-defined horizontal price level.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct HorizontalLevel {
    pub id: u64,
    pub price: f64,
    /// RGBA color in LINEAR space (not sRGB).
    pub color: [f32; 4],
    pub line_width: f32,
    /// Optional text label displayed on the chart.
    #[serde(default)]
    pub label: Option<String>,
    /// Icon displayed next to the label.
    #[serde(default)]
    pub icon: LevelIcon,
    /// Whether this level is locked (prevents drag and delete).
    #[serde(default)]
    pub locked: bool,
}
```

**Rules:**

1. **`#[serde(default)]` on every optional or added-later field.** This is how
   we maintain backward compatibility when loading old configs. If a user's saved
   JSON predates the `locked` field, deserialization must not fail:

   ```rust
   // This must work:
   let json = r#"{"id":1,"price":100.0,"color":[1,0,0,1],"line_width":1.0}"#;
   let level: HorizontalLevel = serde_json::from_str(json).unwrap();
   assert!(!level.locked);  // default
   ```

2. **Colors are linear RGB, not sRGB.** All `[f32; 4]` color fields use
   linear-space values. The GPU shaders and iced overlay both expect linear.
   If you have an sRGB hex color, convert it at definition time:

   ```rust
   // WRONG: these are sRGB values
   pub color: [f32; 4] = [0.2, 0.8, 0.3, 0.9];

   // RIGHT: convert from sRGB to linear
   // For hand-authored constants, the crate midas-render::color provides helpers.
   // For data types in midas-chart, just document that values are linear.
   ```

3. **No `unwrap()` in library crates.** Data types are defined in `midas-chart`,
   which is a library crate. Use `?`, `unwrap_or`, or explicit error handling.

4. **IDs are allocated by the store, not the widget.** The `LevelStore` owns a
   `next_id: u64` counter and assigns IDs on `create()`. Widget structs never
   generate their own IDs.

5. **Keep types small and cheap to clone.** Target < 256 bytes per instance.
   Heap allocations (`String`, `Vec<String>`) are acceptable for persisted data
   that is cloned only on save/load, never in the render hot path.

---

### Layer 2: State Machine

**File:** `midas-chart/src/crosshair_tool.rs` or `midas-chart/src/level_tool.rs`

Interactive widgets need a state machine that tracks what the tool is doing.
The mode enum is the source of truth for the tool's behavior.

```rust
/// The level tool's internal state machine.
#[derive(Clone, Debug, PartialEq)]
pub enum LevelToolMode {
    /// Tool is not active. No preview, no drag.
    Idle,
    /// User activated the tool. Preview line follows cursor Y.
    Placing,
    /// User is dragging an existing level to a new price.
    Dragging {
        level_id: u64,
        /// Price offset between level and cursor at grab time.
        grab_offset: f64,
    },
}

pub struct LevelTool {
    /// Current tool mode -- PRIVATE, never accessed directly.
    mode: LevelToolMode,  // <-- not `pub`
    pub alt_held: bool,
    pub snapped_price: Option<f64>,
    pub preview_price: Option<f64>,
    pub was_placing: bool,
}
```

**Rules:**

1. **Mode is ALWAYS private.** External code queries the tool through predicate
   methods, never by matching on the mode directly. This lets you refactor mode
   variants without breaking callers.

   ```rust
   impl LevelTool {
       pub fn is_idle(&self) -> bool { matches!(self.mode, LevelToolMode::Idle) }
       pub fn is_placing(&self) -> bool { matches!(self.mode, LevelToolMode::Placing) }
       pub fn is_dragging(&self) -> bool { matches!(self.mode, LevelToolMode::Dragging { .. }) }
       pub fn is_active(&self) -> bool { !self.is_idle() }
   }
   ```

   **Exception:** The `LevelTool` in the current codebase has `pub mode` for
   historical reasons. New tools MUST use private mode with predicates. The
   `CrosshairTool` is the canonical example -- its `mode` field is private.

2. **Named transition methods, not direct field assignment.** Every state
   transition goes through a method that enforces invariants:

   ```rust
   impl LevelTool {
       /// Activate level placement mode.
       /// No-op if currently dragging (prevents H-key during active drag).
       pub fn activate(&mut self) {
           if self.is_dragging() { return; }
           self.mode = LevelToolMode::Placing;
           self.snapped_price = None;
           self.preview_price = None;
           self.was_placing = false;
       }

       /// Clear all tool state, returning to Idle.
       pub fn cancel(&mut self) {
           self.mode = LevelToolMode::Idle;
           self.alt_held = false;
           self.snapped_price = None;
           self.preview_price = None;
           self.was_placing = false;
       }
   }
   ```

3. **`suppress()` vs `force_hide()` distinction** (CrosshairTool pattern).
   These serve different purposes during drag interactions:

   - `suppress()` -- Hide the crosshair but preserve `left_mouse_down` state.
     Used when entering a drag: the mouse button is still physically held, and
     we need `on_left_release()` to fire correctly when it is released.

   - `force_hide()` -- Full reset. Clears mode, `left_mouse_down`, and position.
     Used for Escape key, viewport resize, or tool completion.

   ```rust
   impl CrosshairTool {
       /// Hide without clearing left_mouse_down. Used during drag.
       pub fn suppress(&mut self) {
           self.mode = CrosshairMode::Hidden;
           self.cursor_pos = None;
           // left_mouse_down is PRESERVED
       }

       /// Full reset. Used for Escape or tool completion.
       pub fn force_hide(&mut self) {
           self.mode = CrosshairMode::Hidden;
           self.left_mouse_down = false;
           self.cursor_pos = None;
       }
   }
   ```

4. **Resume flag for tools that survive pan interruption.** When a user is in
   Placing mode and starts panning (middle-drag or edge-scroll), the tool
   suspends and sets `was_placing = true`. When the pan ends, the tool resumes:

   ```rust
   impl LevelTool {
       pub fn suspend_placing(&mut self) {
           if matches!(self.mode, LevelToolMode::Placing) {
               self.was_placing = true;
               self.mode = LevelToolMode::Idle;
           }
       }

       pub fn try_resume_placing(&mut self) {
           if self.was_placing && self.is_idle() {
               self.mode = LevelToolMode::Placing;
               self.was_placing = false;
           }
       }
   }
   ```

---

### Layer 3: Interaction

**File:** `midas-chart/src/interaction.rs`

The interaction layer translates raw `ChartEvent` inputs into semantic
`ChartAction` outputs. It is the glue between the user's mouse/keyboard and
the tool state machines.

**Step 1: Add ChartAction variants.**

Every widget operation that mutates state gets its own action variant:

```rust
pub enum ChartAction {
    // ... existing variants ...

    /// Create a new horizontal level at the given price.
    CreateLevel { price: f64 },
    /// Select a horizontal level by its ID.
    SelectLevel { id: u64 },
    /// Drag a horizontal level to a new price.
    DragLevel { id: u64, new_price: f64 },
    /// Delete the currently selected horizontal level.
    DeleteSelectedLevel,
    /// Deselect any selected level.
    DeselectLevel,
    /// Right-click on a level (opens editor).
    RightClickLevel { id: u64, x: f32, y: f32 },
}
```

**Step 2: Wire into `handle_event()`.**

The `handle_event()` function is the central event dispatcher. It produces
a `Vec<ChartAction>` that the app layer applies. The pattern is a large
`match` on `(event, interaction_mode)` pairs.

**Step 3: Hit test integration.**

Hit testing determines what the user clicked on. The pattern is reverse
iteration (topmost first), with a pixel tolerance threshold:

```rust
/// Hit-test tolerance for horizontal levels, in pixels.
const LEVEL_HIT_TOLERANCE_PX: f32 = 6.0;

fn hit_test_level(
    levels: &[HorizontalLevel],
    camera: &Camera2D,
    cursor_y: f32,
) -> Option<(u64, f64)> {
    // Reverse iteration: later levels render on top, so they win hit tests.
    for level in levels.iter().rev() {
        let level_y = camera.price_to_y(level.price);
        if (cursor_y - level_y).abs() <= LEVEL_HIT_TOLERANCE_PX {
            return Some((level.id, level.price));
        }
    }
    None
}
```

**Interaction checklist for a new interactive widget:**

- [ ] Mouse press: hit test against widget geometry, enter PendingDrag or start tool
- [ ] Mouse move: check drag threshold (4px) before committing to drag
- [ ] Mouse move during drag: update position using grab offset
- [ ] Mouse release: finalize action, restore crosshair
- [ ] Delete key: remove selected widget (check `locked` first)
- [ ] Escape key: cancel current tool action
- [ ] Crosshair: suppress during drag, restore on release
- [ ] Lock guard: check `locked` field before allowing drag or delete

**The 4px drag threshold.** This is critical for disambiguating clicks from
drags. When the user presses the mouse button, we enter `PendingDrag` and
do NOT start panning or dragging until the cursor moves 4px from the initial
press point:

```rust
const DRAG_THRESHOLD_PX: f32 = 4.0;

// In handle_event, during PendingDrag:
let dist = ((x - start_x).powi(2) + (y - start_y).powi(2)).sqrt();
if dist >= DRAG_THRESHOLD_PX {
    // NOW commit to the drag/pan
}
```

---

### Layer 4: Computation

**File:** `midas-chart/src/compute.rs`

The computation layer transforms widget data + camera state into screen-space
geometry. This is the core of the sans-IO architecture: ZERO GPU types, ZERO
framework dependencies. Just pure math.

```rust
/// Compute screen-space render data for a horizontal level.
fn compute_level_render(
    level: &HorizontalLevel,
    camera: &Camera2D,
    selected_level: Option<u64>,
    dragging_level_id: Option<u64>,
    viewport_width: u32,
) -> LevelRender {
    let screen_y = camera.price_to_y(level.price);
    let is_selected = selected_level == Some(level.id);
    let is_being_dragged = dragging_level_id == Some(level.id);

    LevelRender {
        id: level.id,
        price: level.price,
        screen_y,
        color: level.color,
        line_width: level.line_width,
        is_selected,
        is_being_dragged,
        original_screen_y: None,
        label_text: format!("{:.2}", level.price),
        label: level.label.clone(),
        icon: level.icon.clone(),
        locked: level.locked,
    }
}
```

**Rules:**

1. **ZERO GPU types.** No `wgpu::Buffer`, no `wgpu::Device`, no
   `wgpu::RenderPass`. The compute layer works only with the types defined
   in `midas-chart/src/instances.rs`.

2. **Use `Camera2D` for all coordinate transforms.** Never hand-roll the
   price-to-pixel or time-to-pixel math. Use the camera methods:

   ```rust
   camera.price_to_y(price)   // price -> logical pixel Y
   camera.y_to_price(y)       // logical pixel Y -> price
   camera.time_to_x(time)     // timestamp -> logical pixel X
   camera.x_to_time(x)        // logical pixel X -> timestamp
   ```

3. **Snap closure for collapsed-gaps mode.** The same widget must work in
   both timestamp-space (normal mode) and index-space (collapsed-gaps mode).
   See Section 3 below for the full pattern.

4. **Presence-aware rendering.** Widgets can have different visual states:
   - Normal: full opacity, full interaction
   - Ghost: reduced alpha (used for cross-chart sync preview)
   - Hidden: skip entirely (used for `visible: false` annotations)

---

### Layer 5: Render Types

**File:** `midas-chart/src/instances.rs`

Render types come in two categories: **GPU types** that go into wgpu instance
buffers, and **overlay types** that drive iced widget construction.

**GPU types must be Pod:**

```rust
/// GPU instance data for a single axis-aligned grid line.
/// Size: 32 bytes per instance (8 floats).
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GridLineInstance {
    /// Rectangle bounds: [left, top, right, bottom] in logical pixels.
    pub rect: [f32; 4],
    /// RGBA color (linear space).
    pub color: [f32; 4],
}
```

GPU type requirements:
- `#[repr(C)]` -- C-compatible memory layout for GPU
- `bytemuck::Pod` + `bytemuck::Zeroable` -- safe transmute to `&[u8]`
- `Copy` + `Clone` -- value semantics
- **Size assertion test** -- prevents accidental layout changes:

```rust
#[test]
fn grid_line_instance_size_is_32_bytes() {
    assert_eq!(
        std::mem::size_of::<GridLineInstance>(),
        32,
        "GridLineInstance must be exactly 32 bytes for GPU layout"
    );
}
```

**Overlay types are NOT Pod:**

```rust
/// Render data for a single horizontal price level.
/// Produced by compute, consumed by the iced overlay builder.
#[derive(Clone, Debug)]
pub struct LevelRender {
    pub id: u64,
    pub price: f64,
    pub screen_y: f32,
    pub color: [f32; 4],
    pub line_width: f32,
    pub is_selected: bool,
    pub is_being_dragged: bool,
    pub label_text: String,    // String is NOT Pod
    pub label: Option<String>,
    pub locked: bool,
}
```

Overlay types can contain `String`, `Vec`, `bool`, `Option` -- anything that
iced needs to build the overlay widgets. They are `Clone + Debug` but NOT
`Copy` or `Pod`.

**Prefer reusing `GridLineInstance` over creating new GPU types.** The
`GridPipeline` renders axis-aligned colored rectangles. Horizontal lines,
vertical lines, zone fills, and background rects all use this one type.
Only create a new GPU instance type if `GridLineInstance` genuinely cannot
express the shape (e.g., a future SDF marker pipeline).

---

### Layer 6: Scene Assembly

**File:** `midas-chart/src/scene.rs` + `midas-chart/src/compute.rs`

The `ChartScene` struct is the complete description of what to render for one
chart frame. Adding a new widget means adding a field here:

```rust
pub struct ChartScene {
    // ... existing fields ...

    /// Horizontal price levels.
    pub levels: Vec<LevelRender>,
    /// Crosshair overlay (if active).
    pub crosshair: Option<CrosshairRender>,
    /// Y position of the level placement preview line (if placing).
    pub level_preview_y: Option<f32>,

    // --- Add new widget here ---
    // pub your_widgets: Vec<YourWidgetRender>,
}
```

Then wire the computation into `compute_chart_scene()`. This function has
two code paths -- normal mode and collapsed-gaps mode. Your widget MUST
appear in BOTH paths:

```rust
fn compute_normal_scene(input: &ChartInput<'_>, ...) -> ChartScene {
    // ... existing computation ...
    let your_widgets = compute_your_widgets(&input.your_data, camera);
    ChartScene { your_widgets, .. }
}

fn compute_collapsed_scene(input: &ChartInput<'_>, ...) -> ChartScene {
    // ... existing computation ...
    let your_widgets = compute_your_widgets(&input.your_data, camera);
    ChartScene { your_widgets, .. }
}
```

If the widget's computation is identical in both modes (like HorizontalLevel,
which only cares about price, not time), you can factor it into a shared
helper. If it needs different snap behavior, use the snap closure pattern
(Section 3).

---

### Layer 7: View Overlay

**File:** `midas-app/src/app/views.rs`

The view overlay builds iced widgets on top of the GPU-rendered chart. This is
where text labels, badges, buttons, and editors live.

**Positioning patterns:**

Right-aligned badge (like a price label):

```rust
use iced::widget::{row, container, text, Space};
use iced::Length::Fill;

let badge = container(
    text(&level.label_text).size(11).color(Color::WHITE)
)
.padding([2, 6])
.style(|_| container::Style {
    background: Some(iced::Background::Color(bg_color)),
    ..Default::default()
});

// Right-align: push badge to the right with a spacer
row![Space::with_width(Fill), badge, Space::with_width(4)]
```

Positioned at a specific Y coordinate:

```rust
use iced::widget::{container, Column};
use iced::Padding;

let y_offset = level_render.screen_y as u16;
container(your_label_widget)
    .padding(Padding::ZERO.top(y_offset))
    .width(Fill)
```

**Stack layer ordering (z-order).** The chart is built as an iced `stack![]`
with layers ordered from bottom to top:

```
Layer 0: GPU shader widget (candles, grid, wicks, volume, level lines, crosshair)
Layer 1: Date label overlay (X-axis labels)
Layer 2: Price label overlay (Y-axis labels)
Layer 3: Indicator overlays (GerchikAtr badge)
Layer 4: Widget label overlays (level labels, annotation badges)  <-- insert here
Layer 5: Crosshair axis label overlay (price/time badges at crosshair)
Layer 6: Drawing panel (tool activation buttons)
Layer 7: Popups / editors (level editor, color picker)
```

New widget overlays go at Layer 4. This ensures they appear above the static
axis labels but below the crosshair badges and interactive panels.

---

## 2. The Grab Offset Pattern

When a user clicks on a level line that is 10px below the cursor, the level
should NOT teleport to the cursor position. Instead, it should maintain the
same 10px offset throughout the drag.

**On drag start -- capture the offset:**

```rust
// In handle_event, when transitioning from PendingDrag to Dragging:
let level_price = level.price;
let cursor_price = camera.y_to_price(cursor_y);
let grab_offset = level_price - cursor_price;

tool.mode = LevelToolMode::Dragging {
    level_id: level.id,
    grab_offset,
};
```

**During drag -- apply the offset:**

```rust
// In handle_event, during MouseMoved while Dragging:
let cursor_price = camera.y_to_price(mouse_y);
let new_price = cursor_price + grab_offset;

actions.push(ChartAction::DragLevel {
    id: level_id,
    new_price,
});
```

**Why this matters:**

Without grab offset, the user sees the level "jump" to the cursor on the first
frame of the drag. This feels broken, especially on high-resolution charts
where price levels are close together. The grab offset pattern preserves the
spatial relationship between cursor and widget throughout the entire drag.

**For 2D widgets** (like notes or markers anchored to both price and time),
you need two grab offsets:

```rust
Dragging {
    annotation_id: AnnotationId,
    grab_offset_price: f64,
    grab_offset_time: i64,
}

// During drag:
let new_price = cursor_price + grab_offset_price;
let new_time = cursor_time + grab_offset_time;
```

---

## 3. The Snap Closure Pattern

The chart system has two X-axis modes: **normal mode** (timestamps on X) and
**collapsed-gaps mode** (sequential index on X). Widget computation must work
in both modes without duplication.

The solution is a closure that abstracts the snap behavior:

**Normal mode (timestamp-space):**

```rust
let snap_fn = |px: f32| -> Option<(f32, usize)> {
    let time = camera.x_to_time(px);
    let idx = data.find_index_by_time(time as i64);
    if idx >= data.len() { return None; }
    let snap_x = camera.time_to_x(data.timestamp(idx) as f64);
    Some((snap_x, idx))
};
```

**Collapsed-gaps mode (index-space):**

```rust
let snap_fn = |px: f32| -> Option<(f32, usize)> {
    let float_idx = camera.x_to_time(px);
    let idx = float_idx.round().max(0.0) as usize;
    if idx >= data.len() { return None; }
    let snap_x = camera.time_to_x(idx as f64);
    Some((snap_x, idx))
};
```

**Usage in widget computation:**

```rust
fn compute_your_widget(
    widget: &YourWidget,
    camera: &Camera2D,
    snap_fn: impl Fn(f32) -> Option<(f32, usize)>,
) -> YourWidgetRender {
    // Use snap_fn to convert a pixel X to a snapped X + candle index.
    // The same code works in both normal and collapsed modes.
    let (snapped_x, candle_idx) = match (snap_fn)(cursor_x) {
        Some(pair) => pair,
        None => return default_render(),
    };
    // ... use snapped_x for positioning, candle_idx for data lookup
}
```

**Key insight:** In normal mode, `camera.x_to_time()` returns a timestamp
(epoch ms). In collapsed mode, it returns a floating-point index. The snap
closure hides this difference from the widget code.

The crosshair's vertical line snap uses this exact pattern. When you see
`x_to_time` return values that look like small integers (0.0, 1.0, 2.0, ...),
you are in collapsed mode and those are candle indices.

---

## 4. Generation Counter Pattern

Boolean dirty flags have a "who clears the flag" problem: if two consumers
both need to react to a change, the first consumer to clear the flag starves
the second. Generation counters solve this by letting each consumer track its
own last-seen generation.

**The writer increments:**

```rust
pub struct DirtyFlags {
    pub camera: u64,
    pub candles: u64,
    pub levels: u64,
    // ... one counter per concern
}

impl DirtyFlags {
    /// Camera moved. Also invalidates candles (pixel positions) and grid.
    pub fn mark_camera(&mut self) {
        self.camera += 1;
        self.candles += 1;
        self.grid += 1;
    }

    /// Horizontal levels changed (added/moved/deleted).
    pub fn mark_levels(&mut self) {
        self.levels += 1;
    }
}
```

**Each reader tracks its own last-seen generation:**

```rust
pub struct DirtyTracker {
    last_seen: DirtyFlags,
}

impl DirtyTracker {
    pub fn needs_level_rebuild(&self, current: &DirtyFlags) -> bool {
        self.last_seen.levels != current.levels
    }

    pub fn acknowledge(&mut self, current: &DirtyFlags) {
        self.last_seen = current.clone();
    }
}
```

**Adding a new counter for your widget:**

1. Add the counter to `DirtyFlags`:

   ```rust
   pub struct DirtyFlags {
       // ... existing ...
       pub your_widget: u64,
   }
   ```

2. Add a `mark_*()` method with cascading rules. Think about what else changes
   when your widget changes:

   ```rust
   impl DirtyFlags {
       pub fn mark_your_widget(&mut self) {
           self.your_widget += 1;
       }
   }
   ```

3. If `mark_theme()` should cascade to your widget (it probably should, since
   theme changes affect colors which are baked into instances), add it:

   ```rust
   pub fn mark_theme(&mut self) {
       self.theme += 1;
       self.candles += 1;
       self.indicators += 1;
       self.levels += 1;
       self.grid += 1;
       self.your_widget += 1;  // <-- add this
   }
   ```

4. Add `needs_*_rebuild()` to `DirtyTracker`:

   ```rust
   pub fn needs_your_widget_rebuild(&self, current: &DirtyFlags) -> bool {
       self.last_seen.your_widget != current.your_widget
   }
   ```

5. Wire into `SceneGenerations` if the renderer needs to skip uploads:

   ```rust
   pub struct SceneGenerations {
       // ... existing ...
       pub your_widget: u64,
   }
   ```

6. Update `any_dirty()` and `acknowledge()` (they already copy all fields, so
   no change needed if you used `clone()` in acknowledge).

**Cascading rules cheat sheet:**

| Event | Cascades to |
|---|---|
| Camera (pan/zoom) | candles, grid |
| Data (new candles) | candles, indicators |
| Theme | candles, indicators, levels, grid, your_widget |
| Your widget CRUD | your_widget only |

---

## 5. Tool Activation Pattern

Only one drawing/interaction tool can be active at a time. The current system
manages this through the `InteractionMode` enum on `ChartState` combined with
dedicated tool structs.

**Current architecture:**

```rust
pub struct ChartState {
    pub interaction_mode: InteractionMode,
    pub level_tool: LevelTool,
    pub crosshair: CrosshairTool,
    // Future: pub bracket_tool: BracketTool,
}
```

The `InteractionMode` enum handles chart-level interactions (panning, scaling),
while dedicated tool structs handle widget-specific behavior. The
`CursorClaim` system coordinates between them:

```rust
/// What the active tool needs from the crosshair.
pub enum CursorClaim {
    /// No tool claims the cursor. Normal crosshair rules apply.
    None,
    /// A placement tool wants a preview line. Crosshair visible
    /// regardless of mouse button, Y may be snapped.
    Preview,
    /// An active drag/edit wants the crosshair hidden entirely.
    Suppress,
}

impl ChartState {
    pub fn active_cursor_claim(&self) -> CursorClaim {
        // Priority: Suppress > Preview > None
        if self.level_tool.is_active() {
            return CursorClaim::Suppress;
        }
        // Future tools check here:
        // if self.bracket_tool.is_active() { return CursorClaim::Suppress; }
        CursorClaim::None
    }
}
```

**Adding a new tool:**

1. Create a dedicated tool struct (e.g., `BracketTool`) following the Layer 2
   pattern (private mode, predicates, named transitions).

2. Add it as a field on `ChartState`:

   ```rust
   pub struct ChartState {
       // ... existing ...
       pub bracket_tool: BracketTool,
   }
   ```

3. Add it to `active_cursor_claim()`.

4. Wire tool activation/deactivation into `handle_event()`:
   - Keyboard shortcut (e.g., `B` for bracket tool) activates
   - `Escape` cancels the current tool
   - Starting a new tool cancels any active tool first

5. Handle tool suspension during pan. When the user starts panning while a
   tool is active, suspend the tool. When panning ends, resume it:

   ```rust
   // In handle_event, on pan start:
   state.bracket_tool.suspend();

   // In handle_event, on pan end:
   state.bracket_tool.try_resume();
   ```

---

## 6. Crosshair Suppression Pattern

The crosshair is the most visible widget on the chart, and it must correctly
yield to other interactions. Three methods control this:

**`suppress()` -- Hide during drag (preserves mouse state).**

Called when transitioning from PendingDrag to a widget drag. The mouse button
is still held, so we cannot reset `left_mouse_down`:

```rust
// Transitioning to level drag:
state.crosshair.suppress();
state.level_tool.mode = LevelToolMode::Dragging { level_id, grab_offset };
```

When the drag ends and the mouse is released, `on_left_release()` correctly
transitions from the suppressed Hidden state because `left_mouse_down` was
preserved.

**`force_hide()` -- Full reset (for completion or cancellation).**

Called when a tool finishes or the user presses Escape:

```rust
// Tool completed or cancelled:
state.crosshair.force_hide();
state.level_tool.cancel();
```

This resets everything: mode, position, and `left_mouse_down`.

**`enter_preview()` / `exit_preview()` -- Tool override mode.**

Used by placement tools that want the crosshair visible at a controlled
position regardless of mouse button state:

```rust
// Entering level placement mode:
state.crosshair.enter_preview(cursor_x, cursor_y);

// Each mouse move during placement:
state.crosshair.on_mouse_move(x, y, in_bounds);
// Preview mode always updates position, even out-of-bounds.

// Exiting placement:
state.crosshair.exit_preview();
// Returns to Tracking or Hidden based on left_mouse_down.
```

**Decision table for crosshair during widget interaction:**

| Event | Crosshair action |
|---|---|
| Enter placement tool | `enter_preview(x, y)` |
| Mouse move during placement | `on_mouse_move(x, y, in_bounds)` (Preview mode handles it) |
| Click to place widget | `exit_preview()` if done, stay in Preview if multi-step |
| Start dragging widget | `suppress()` |
| Mouse move during drag | Do nothing (crosshair already hidden) |
| Release after drag | `on_left_release()` handles transition |
| Escape during any tool | `force_hide()` |
| Tool suspended for pan | Crosshair state preserved (pan has its own mode) |
| Tool resumed after pan | `resume_preview()` restores Preview at last position |

---

## 7. Testing Patterns

Every layer has specific testing requirements. Here are the templates.

### Data type serde round-trip test

```rust
#[test]
fn your_widget_serde_round_trip() {
    let widget = YourWidget {
        id: 42,
        price: 175.50,
        color: [0.0, 1.0, 0.5, 0.8],
        label: Some("Resistance".into()),
        locked: true,
    };
    let json = serde_json::to_string(&widget).expect("serialize");
    let decoded: YourWidget = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(decoded.id, 42);
    assert!((decoded.price - 175.50).abs() < f64::EPSILON);
    assert_eq!(decoded.label.as_deref(), Some("Resistance"));
    assert!(decoded.locked);
}
```

### Backward compatibility test (serde defaults)

```rust
#[test]
fn your_widget_serde_defaults_for_new_fields() {
    // Simulate loading old data without the newer fields.
    let json = r#"{"id":1,"price":100.0,"color":[1,0,0,1]}"#;
    let decoded: YourWidget = serde_json::from_str(json).expect("deserialize");
    assert_eq!(decoded.label, None);     // serde(default)
    assert!(!decoded.locked);            // serde(default)
}
```

### State machine transition test

```rust
#[test]
fn tool_transitions_idle_to_placing_to_idle() {
    let mut tool = YourTool::default();
    assert!(tool.is_idle());
    assert!(!tool.is_active());

    tool.activate();
    assert!(!tool.is_idle());
    assert!(tool.is_active());
    assert!(tool.is_placing());

    tool.cancel();
    assert!(tool.is_idle());
    assert!(!tool.is_active());
}
```

### State machine guard test

```rust
#[test]
fn activate_noop_during_drag() {
    let mut tool = YourTool::default();
    tool.start_drag(42, 0.0);
    assert!(tool.is_dragging());

    tool.activate();  // should be no-op
    assert!(tool.is_dragging());  // still dragging, not reset to Placing
}
```

### Suspend/resume test

```rust
#[test]
fn suspend_and_resume_placing() {
    let mut tool = YourTool::default();
    tool.activate();
    assert!(tool.is_placing());

    tool.suspend_placing();
    assert!(tool.is_idle());
    assert!(tool.was_placing);

    tool.try_resume_placing();
    assert!(tool.is_placing());
    assert!(!tool.was_placing);
}
```

### GPU type size assertion test

```rust
#[test]
fn your_gpu_instance_size_is_32_bytes() {
    assert_eq!(
        std::mem::size_of::<YourGpuInstance>(),
        32,
        "YourGpuInstance must be exactly 32 bytes for GPU layout"
    );
}

#[test]
fn your_gpu_instance_is_pod() {
    let instance = YourGpuInstance {
        rect: [0.0, 500.0, 1920.0, 500.667],
        color: [1.0, 0.0, 0.0, 1.0],
    };
    let bytes: &[u8] = bytemuck::bytes_of(&instance);
    assert_eq!(bytes.len(), 32);
}
```

### Crosshair suppression test

```rust
#[test]
fn suppress_hides_but_preserves_left_mouse_down() {
    let mut crosshair = CrosshairTool::new();
    crosshair.on_left_press(100.0, 200.0);
    assert!(crosshair.left_mouse_down());
    assert!(crosshair.should_render());

    crosshair.suppress();
    assert!(!crosshair.should_render());
    assert!(crosshair.left_mouse_down());  // preserved

    let became_hidden = crosshair.on_left_release();
    assert!(!became_hidden);  // was already hidden
    assert!(!crosshair.left_mouse_down());
}
```

### Indicator compute test (GerchikAtr pattern)

For display-only widgets that compute from candle data:

```rust
#[test]
fn returns_none_for_insufficient_data() {
    let data = empty_candle_data();
    assert!(compute_your_indicator(&data).is_none());
}

#[test]
fn returns_valid_render_for_normal_data() {
    let data = multi_day_fixture(20);
    let result = compute_your_indicator(&data);
    assert!(result.is_some());
    let render = result.unwrap();
    assert!(!render.text.is_empty());
    assert!(render.color[3] > 0.0);  // non-zero alpha
}
```

### Mock CandleData fixture

The project uses a lightweight mock pattern for `CandleData` in tests:

```rust
struct MockCandles {
    timestamps: Vec<i64>,
    open: Vec<f32>,
    high: Vec<f32>,
    low: Vec<f32>,
    close: Vec<f32>,
}

impl CandleData for MockCandles {
    fn len(&self) -> usize { self.timestamps.len() }
    fn timestamp(&self, idx: usize) -> i64 { self.timestamps[idx] }
    fn open(&self, idx: usize) -> f32 { self.open[idx] }
    fn high(&self, idx: usize) -> f32 { self.high[idx] }
    fn low(&self, idx: usize) -> f32 { self.low[idx] }
    fn close(&self, idx: usize) -> f32 { self.close[idx] }
    fn volume(&self, _idx: usize) -> u32 { 1000 }
    fn price_range(&self, range: std::ops::Range<usize>) -> (f32, f32) {
        let mut lo = f32::MAX;
        let mut hi = f32::MIN;
        for i in range {
            lo = lo.min(self.low[i]);
            hi = hi.max(self.high[i]);
        }
        (lo, hi)
    }
    fn find_index_by_time(&self, ts: i64) -> usize {
        self.timestamps
            .binary_search(&ts)
            .unwrap_or_else(|i| i.min(self.len().saturating_sub(1)))
    }
}
```

And a standard test camera:

```rust
fn test_camera() -> Camera2D {
    Camera2D {
        time_start: 1_000_000.0,
        time_end: 2_000_000.0,
        price_low: 100.0,
        price_high: 200.0,
        viewport_width: 1920,
        viewport_height: 1080,
        dpi_scale: 1.0,
    }
}
```

---

## 8. Checklist: Adding a New Widget

Complete step-by-step for implementing any new widget from scratch.

### Data Layer

- [ ] 1. **Define data type** in `midas-chart/src/widget/your_widget.rs`
  - `#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]`
  - `#[serde(default)]` on all optional/added-later fields
  - Colors in linear RGB `[f32; 4]`
  - No `unwrap()` in the module

- [ ] 2. **Add variant to `AnnotationKind`** enum (if this is an annotation)
  - Or add to the appropriate collection type if it is a standalone widget

- [ ] 3. **Export from `midas-chart/src/lib.rs`**
  - `pub mod your_widget;`
  - `pub use your_widget::YourWidget;`

### Interaction Layer (skip for display-only widgets)

- [ ] 4. **Define tool state machine** in `midas-chart/src/your_tool.rs`
  - Private mode enum
  - Predicate methods: `is_idle()`, `is_active()`, `is_placing()`, `is_dragging()`
  - Named transition methods: `activate()`, `cancel()`, `suspend_placing()`, `try_resume_placing()`

- [ ] 5. **Add `ChartAction` variants** in `interaction.rs`
  - `CreateYourWidget { ... }`
  - `DragYourWidget { id, new_price }`
  - `DeleteYourWidget { id }`
  - `SelectYourWidget { id }` (if selectable)

- [ ] 6. **Wire into `handle_event()`** in `interaction.rs`
  - Hit test in the mouse press path
  - Drag threshold before committing
  - Grab offset on drag start
  - Crosshair suppress/restore
  - Lock guard on drag/delete
  - Escape cancellation

- [ ] 7. **Add tool field to `ChartState`**
  - Add to `active_cursor_claim()`
  - Handle in `apply_action()`

### Computation Layer

- [ ] 8. **Implement `compute_*()` function** in `compute.rs` or widget module
  - Pure function: data + camera -> render data
  - No GPU types, no framework types
  - Handle both normal and collapsed-gaps modes

- [ ] 9. **Add render types** in `instances.rs`
  - GPU type (if needed): `#[repr(C)]`, `Pod`, `Zeroable`, size test
  - Overlay type: `Clone`, `Debug`, can contain `String`

### Scene Assembly

- [ ] 10. **Add field to `ChartScene`** in `scene.rs`
  - `pub your_widgets: Vec<YourWidgetRender>`

- [ ] 11. **Wire into both compute paths** in `compute.rs`
  - `compute_normal_scene()` -- add your computation
  - `compute_collapsed_scene()` -- add your computation

### Dirty Tracking

- [ ] 12. **Add generation counter** to `DirtyFlags`
  - Add `pub your_widget: u64` field
  - Add `mark_your_widget()` method
  - Add to `mark_theme()` cascade (if colors are baked)
  - Add to `mark_all()`

- [ ] 13. **Add `needs_*_rebuild()`** to `DirtyTracker`
  - Add to `any_dirty()`

- [ ] 14. **Add to `SceneGenerations`** if the renderer gates on it

### View Layer

- [ ] 15. **Build iced overlay** in `midas-app/src/app/views.rs`
  - Construct overlay from render data
  - Insert at correct z-order in the stack
  - Handle both main window and floating window paths

### Persistence (if needed)

- [ ] 16. **Add persistence support** in `AnnotationStore` or `LevelStore`
  - CRUD methods: `create()`, `update()`, `delete()`
  - Generation counter bump on mutation
  - Serde round-trip in file format

### GPU Rendering (if new pipeline needed)

- [ ] 17. **Add pipeline** in `midas-render/src/pipelines/`
  - Typically reuse `GridPipeline` with a new instance buffer
  - Add to `ChartRenderer` and wire into draw call order

### Testing

- [ ] 18. **Unit tests**
  - Serde round-trip + backward compatibility
  - State machine transitions (all paths)
  - Computation correctness (screen positions, colors)
  - GPU type size assertions (if applicable)

- [ ] 19. **Integration tests**
  - CRUD through the store
  - Persistence save/load round-trip
  - Cross-chart behavior (if applicable)

---

## 9. Anti-Patterns to Avoid

### DO NOT hold GPU resources in data types

```rust
// WRONG: breaks sans-IO
pub struct MyWidget {
    buffer: wgpu::Buffer,  // GPU resource in a midas-chart type
}

// RIGHT: data type is pure, GPU buffer lives in midas-render
pub struct MyWidget {
    pub price: f64,
    pub color: [f32; 4],
}
```

The sans-IO boundary is the most important architectural constraint in the
project. `midas-chart` must compile and test without any GPU context.

### DO NOT use trait objects for widget dispatch

```rust
// WRONG: dynamic dispatch, heap allocation, no exhaustive matching
let widgets: Vec<Box<dyn Widget>> = vec![...];

// RIGHT: enum dispatch, stack-allocated, exhaustive match
pub enum AnnotationKind {
    Level(HorizontalLevel),
    OrderBracket(OrderBracket),
    Note(TextNote),
}
```

Enum dispatch is faster (no vtable indirection), allows exhaustive `match`
(compiler catches missing arms), and avoids heap allocation.

### DO NOT store annotations per-chart

```rust
// WRONG: annotations duplicated across charts showing the same symbol
chart_a.levels = vec![...];
chart_b.levels = vec![...];  // same symbol, separate copy

// RIGHT: centralized per-symbol store, shared by reference
level_store.levels_for("AAPL")  // one source of truth
```

The `LevelStore` (and future `AnnotationStore`) is keyed by ticker symbol,
not by chart ID. Multiple charts showing the same symbol share the same
annotations.

### DO NOT use boolean dirty flags

```rust
// WRONG: who clears it? If two consumers need to react, one starves.
pub levels_dirty: bool,

// RIGHT: each consumer tracks independently
pub levels: u64,  // generation counter
```

See Section 4 for the full pattern.

### DO NOT access tool mode directly

```rust
// WRONG: tight coupling to internal representation
if let LevelToolMode::Dragging { level_id, .. } = &tool.mode {
    // ...
}

// RIGHT: use predicates
if tool.is_dragging() {
    // ...
}
```

Predicates let you refactor the mode enum without breaking callers.

### DO NOT use `unwrap()` in library crates

```rust
// WRONG: panics on bad data
let price = data.get(idx).unwrap();

// RIGHT: propagate the error or provide a default
let price = data.get(idx).copied().unwrap_or(0.0);
// or
let price = data.get(idx).ok_or(ChartError::IndexOutOfBounds)?;
```

`unwrap()` is acceptable in tests and in `midas-app/src/main.rs` during early
development. Never in `midas-chart`, `midas-core`, `midas-data`, `midas-render`,
or `midas-feed`.

### DO NOT use sRGB colors

```rust
// WRONG: sRGB gamma values (too bright after GPU linear blending)
pub color: [f32; 4] = [1.0, 0.5, 0.0, 1.0]; // looks like sRGB orange

// RIGHT: linear-space values
pub color: [f32; 4] = [1.0, 0.214, 0.0, 1.0]; // linear orange
```

All colors in the system are linear RGB. The GPU blending and iced rendering
both operate in linear space. If you are eyeballing hex colors from a design
tool, convert them from sRGB to linear first.

### DO NOT create separate compute paths for normal/collapsed-gaps

```rust
// WRONG: duplicated logic, diverges over time
fn compute_normal(&self) -> Render { ... }
fn compute_collapsed(&self) -> Render { ... } // copy-pasted with subtle differences

// RIGHT: shared compute with snap closure
fn compute(&self, snap_fn: impl Fn(f32) -> Option<(f32, usize)>) -> Render { ... }
```

The snap closure pattern (Section 3) abstracts the only difference between
normal and collapsed-gaps modes. Use it for any widget that needs X-axis
positioning.

### DO NOT add composable modifiers (yet)

The event system currently uses a monolithic `handle_event()` function. This
is intentional: with fewer than 10 interactive widget types, the flat `match`
is easier to reason about than a middleware/interceptor chain. Do not add an
`EventHandler` trait, a `Middleware` stack, or a `Behavior` composable system.
If we reach 15+ widget types and the `match` becomes unmanageable, we will
refactor to a dispatched system at that point.

### DO NOT create new GPU instance types unless necessary

```rust
// WRONG: new type just for annotation lines
#[repr(C)]
pub struct AnnotationLineInstance {
    pub rect: [f32; 4],
    pub color: [f32; 4],
}
// This is identical to GridLineInstance.

// RIGHT: reuse GridLineInstance
pub type AnnotationLineInstance = GridLineInstance;
```

`GridLineInstance` (32 bytes: rect + color) can express horizontal lines,
vertical lines, zone fills, background rects, and any axis-aligned colored
rectangle. Only create a new GPU instance type when you need fundamentally
different vertex attributes (e.g., texture coordinates, SDF parameters).

---

## 10. Reference Implementation Map

When implementing a new widget, use these existing implementations as
templates based on what your widget needs:

| If your widget... | Study this reference | Files |
|---|---|---|
| Is a display-only indicator | **GerchikAtr** | `gerchik_atr.rs`, `views.rs` |
| Has a data type with serde | **HorizontalLevel** | `levels.rs` |
| Needs a tool state machine | **LevelTool** | `level_tool.rs` |
| Needs crosshair coordination | **CrosshairTool** | `crosshair_tool.rs` |
| Needs grab-offset dragging | **LevelTool::Dragging** | `level_tool.rs`, `interaction.rs` |
| Needs per-symbol storage | **LevelStore** | `level_store.rs` |
| Needs GPU line rendering | **GridLineInstance** | `instances.rs`, grid pipeline |
| Needs an iced overlay label | **LevelRender** | `instances.rs`, `views.rs` |
| Is a multi-step drawing tool | **BracketTool** (planned) | `04-interaction-system.md` Section 3 |

The GerchikAtr indicator is the simplest end-to-end example: it reads candle
data, computes a single render struct, and produces an iced text badge. Start
there if you are building a display-only widget.

The HorizontalLevel + LevelTool + LevelStore combination is the complete
reference for an interactive, persistent, cross-chart-synced widget. Study
all three files together to understand the full lifecycle.
