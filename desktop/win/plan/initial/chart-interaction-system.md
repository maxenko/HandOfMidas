# Chart Interaction System --- Complete Design Plan

> Midas Desktop (Rust + iced + wgpu) --- Phase 3 / Phase 6 Deep Specification
> Authored 2026-03-24. Covers chart state, input handling, zoom, pan, crosshair, horizontal levels, multi-chart sync, animation, and coordinate transforms.

---

## Table of Contents

- [1. ChartState Struct](#1-chartstate-struct)
- [1b. ChartInput and compute_chart_scene](#1b-chartinput-and-compute_chart_scene)
- [2. Input Event Handling](#2-input-event-handling)
- [3. Zoom Mechanics](#3-zoom-mechanics)
- [4. Pan Mechanics](#4-pan-mechanics)
- [5. Y-Axis Auto-Scaling](#5-y-axis-auto-scaling)
- [6. Crosshair System](#6-crosshair-system)
- [7. Horizontal Levels](#7-horizontal-levels)
- [8. Multi-Chart Synchronization](#8-multi-chart-synchronization)
- [9. Animation System](#9-animation-system)
- [10. Coordinate Transforms](#10-coordinate-transforms)

---

## 1. ChartState Struct

Every chart panel in the application owns exactly one `ChartState`. This struct is the single source of truth for what a chart displays and how the user is interacting with it. It lives in `midas-chart/src/state.rs` and is referenced by both the iced widget layer (`chart_widget.rs`) and the GPU renderer (`midas-render`).

### Design Principles

- **No interior mutability.** The iced Elm architecture means `ChartState` is mutated exclusively in the `update()` function via messages. No `RefCell`, no `Mutex`.
- **All animation targets stored explicitly.** Current values and target values are separate fields so the animation system can lerp between them.
- **Derived values are NOT stored.** Pixels-per-candle, projection matrices, and visible candle indices are computed on demand from the canonical state. This avoids stale-cache bugs.
- **ChartState is `Clone + Debug`.** Must be cheaply inspectable for logging and serializable for config persistence.

### Complete Struct

```rust
use std::collections::BTreeMap;

/// Unique identifier for a chart panel.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub struct ChartId(pub u32);

/// Unique identifier for a horizontal level within one chart.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, Ord, PartialOrd)]
pub struct LevelId(pub u64);

/// Complete state for a single chart panel.
#[derive(Clone, Debug)]
pub struct ChartState {
    // --- Identity ---
    pub id: ChartId,
    pub symbol: String,           // e.g. "AAPL"
    pub timeframe: Timeframe,     // e.g. Timeframe::M5

    // --- Camera: Visible Window ---
    pub camera: Camera2D,

    // --- Scroll / Pan Momentum ---
    pub momentum: PanMomentum,

    // --- Y-Axis Animation ---
    pub y_axis: YAxisState,

    // --- Interaction State ---
    pub interaction: InteractionState,

    // --- Crosshair ---
    pub crosshair: CrosshairState,

    // --- Horizontal Levels ---
    pub levels: BTreeMap<LevelId, HorizontalLevel>,
    pub next_level_id: u64,

    // --- Sync ---
    pub sync_group: Option<SyncGroupId>,   // None = unlinked

    // --- Data Info (for bounds checking) ---
    pub data_time_min: f64,       // Earliest available timestamp (epoch ms)
    pub data_time_max: f64,       // Latest available timestamp (epoch ms)
    pub total_candle_count: usize, // For LOD decisions

    // --- Viewport (set by layout system, read-only to chart logic) ---
    pub viewport: Viewport,

    // --- Dirty Flags ---
    pub dirty: DirtyFlags,
}

/// The camera defines the visible time and price window.
/// All values in data coordinates (epoch ms for time, price for y).
/// Lives in midas-chart::camera. This is the canonical definition.
#[derive(Clone, Debug)]
pub struct Camera2D {
    /// Visible time range (x-axis), epoch milliseconds as f64.
    /// time_start < time_end always.
    pub time_start: f64,
    pub time_end: f64,

    /// Visible price range (y-axis). price_low < price_high always.
    /// These are the CURRENT (possibly mid-animation) values.
    pub price_low: f64,
    pub price_high: f64,
}

/// Momentum state for flick-to-scroll pan.
#[derive(Clone, Debug, Default)]
pub struct PanMomentum {
    /// Current velocity in pixels/second (signed: negative = scroll left/future)
    pub vx: f32,
    pub vy: f32,

    /// Whether momentum is actively decelerating
    pub active: bool,

    /// Timestamp of last velocity sample (for computing acceleration)
    pub last_sample_time: f64,
}

/// Y-axis auto-scale animation state.
#[derive(Clone, Debug)]
pub struct YAxisState {
    /// Target price range that auto-scale is animating toward.
    pub target_low: f64,
    pub target_high: f64,

    /// Is an animation currently in progress?
    pub animating: bool,

    /// Manual Y-axis lock: if true, auto-scale is disabled.
    pub locked: bool,

    /// Padding factor applied to data range (0.05 = 5% top and bottom).
    pub padding_factor: f64,
}

/// State machine for mouse/keyboard interaction.
#[derive(Clone, Debug)]
pub enum InteractionState {
    /// No active interaction. Default state.
    Idle,

    /// Mouse button is down but we haven't moved enough to commit to a drag type.
    /// Stores the initial mouse position and timestamp.
    PendingDrag {
        start_screen: ScreenPoint,
        start_time: f64,       // epoch ms of mousedown
        button: MouseButton,
    },

    /// User is panning the chart (click+drag on chart area).
    Panning {
        /// Screen position of last processed mouse event (for delta computation).
        last_screen: ScreenPoint,
        /// Accumulated velocity samples for momentum calculation.
        velocity_samples: VelocityBuffer,
    },

    /// User is dragging a horizontal level to a new price.
    DraggingLevel {
        level_id: LevelId,
        /// The Y offset between the mouse and the level's price line,
        /// so the level doesn't "jump" to cursor on drag start.
        grab_offset_pixels: f32,
    },

    /// A double-click was detected; waiting to see if the user does anything else.
    /// (Used to place a horizontal level.)
    AwaitingLevelPlacement {
        screen_pos: ScreenPoint,
    },
}

/// Crosshair state. Updated every mouse move.
#[derive(Clone, Debug, Default)]
pub struct CrosshairState {
    /// Is the mouse currently over this chart's content area?
    pub active: bool,

    /// Mouse position in screen-local coordinates (relative to chart widget).
    pub screen_pos: ScreenPoint,

    /// Snapped candle index (index into the data buffer).
    pub snapped_candle_index: Option<usize>,

    /// Snapped candle center X in screen-local pixels.
    pub snapped_x: f32,

    /// The actual price at the cursor Y (unsnapped, for the horizontal line).
    pub cursor_price: f64,

    /// OHLCV data for the candle under the cursor (cached to avoid re-lookup).
    pub hover_ohlcv: Option<OhlcvSnapshot>,

    /// Is this a "remote" crosshair synced from another chart?
    /// If true, only the vertical line is drawn (no horizontal, no tooltip).
    pub is_synced_remote: bool,

    /// For synced crosshairs: the time coordinate being synced.
    pub synced_time: Option<f64>,
}

/// Snapshot of OHLCV data for tooltip display.
#[derive(Clone, Debug, Default)]
pub struct OhlcvSnapshot {
    pub timestamp: i64,
    pub open: f32,
    pub high: f32,
    pub low: f32,
    pub close: f32,
    pub volume: u32,
    pub change_pct: f32,    // (close - open) / open * 100
}

/// Circular buffer for velocity sampling (used for pan momentum).
#[derive(Clone, Debug)]
pub struct VelocityBuffer {
    /// Ring buffer of (timestamp_ms, dx_pixels, dy_pixels) samples.
    samples: [(f64, f32, f32); 8],
    head: usize,
    count: usize,
}

/// Pixel-space point relative to the chart widget's top-left corner.
#[derive(Copy, Clone, Debug, Default)]
pub struct ScreenPoint {
    pub x: f32,
    pub y: f32,
}

/// Viewport geometry provided by the layout system.
#[derive(Clone, Debug)]
pub struct Viewport {
    /// Width and height of the entire chart widget in logical pixels.
    pub width: f32,
    pub height: f32,

    /// DPI scale factor (1.0, 1.5, 2.0, etc.)
    pub dpi_scale: f32,

    /// Insets for axes (chart content area is smaller than the widget).
    pub margin_left: f32,    // Space for labels (if any); usually 0
    pub margin_right: f32,   // Y-axis width (80px)
    pub margin_top: f32,     // Space for symbol/OHLCV header (20px)
    pub margin_bottom: f32,  // X-axis height (30px)
}

/// Canonical DirtyFlags -- lives in midas-chart::dirty.
/// Uses generation counters instead of booleans to solve the
/// "who clears the flag" problem (iced's Primitive::prepare() has &self).
/// Writer increments the counter; reader (DirtyTracker) remembers
/// last-seen generation and compares.
#[derive(Clone, Debug, Default)]
pub struct DirtyFlags {
    pub camera: u64,      // Viewport/zoom/pan changed
    pub candles: u64,     // Candle data changed (new data, LOD change)
    pub indicators: u64,  // Indicator output changed
    pub crosshair: u64,   // Crosshair position changed
    pub levels: u64,      // Horizontal levels changed
    pub grid: u64,        // Grid needs recalc (zoom changed grid density)
    pub theme: u64,       // Theme/colors changed
}

impl DirtyFlags {
    pub fn new() -> Self { Self::default() }

    /// Camera moved (pan/zoom/resize). Also invalidates grid (grid
    /// density depends on zoom level).
    pub fn mark_camera(&mut self) { self.camera += 1; self.grid += 1; }

    /// Candle data changed (new data loaded, symbol change, LOD change).
    /// Also invalidates indicators (they depend on candle data).
    pub fn mark_data(&mut self) { self.candles += 1; self.indicators += 1; }

    /// Indicator output changed (indicator added/removed/recalculated).
    pub fn mark_indicators(&mut self) { self.indicators += 1; }

    /// Crosshair position changed (mouse moved).
    pub fn mark_crosshair(&mut self) { self.crosshair += 1; }

    /// Horizontal levels changed (added/moved/deleted).
    pub fn mark_levels(&mut self) { self.levels += 1; }

    /// Theme/colors changed. Requires full instance rebuild.
    pub fn mark_theme(&mut self) {
        self.theme += 1;
        // Theme change invalidates all instance data (colors are baked in)
        self.candles += 1;
        self.indicators += 1;
        self.levels += 1;
        self.grid += 1;
    }
}

/// Each GPU consumer tracks what generation it last processed.
/// Owned by ChartGpuResources (in the Shader's Pipeline), NOT by
/// the application state. This means Primitive::prepare() (which
/// takes &self) can still compare and update via &mut Pipeline.
#[derive(Clone, Debug, Default)]
pub struct DirtyTracker {
    last_seen: DirtyFlags,
}

impl DirtyTracker {
    pub fn new() -> Self { Self::default() }

    pub fn needs_camera_update(&self, current: &DirtyFlags) -> bool {
        self.last_seen.camera != current.camera
    }
    pub fn needs_candle_rebuild(&self, current: &DirtyFlags) -> bool {
        self.last_seen.candles != current.candles
    }
    pub fn needs_indicator_rebuild(&self, current: &DirtyFlags) -> bool {
        self.last_seen.indicators != current.indicators
    }
    pub fn needs_crosshair_update(&self, current: &DirtyFlags) -> bool {
        self.last_seen.crosshair != current.crosshair
    }
    pub fn needs_level_rebuild(&self, current: &DirtyFlags) -> bool {
        self.last_seen.levels != current.levels
    }
    pub fn needs_grid_rebuild(&self, current: &DirtyFlags) -> bool {
        self.last_seen.grid != current.grid
    }
    pub fn needs_theme_rebuild(&self, current: &DirtyFlags) -> bool {
        self.last_seen.theme != current.theme
    }

    /// Returns true if ANY counter has changed since last acknowledgment.
    pub fn any_dirty(&self, current: &DirtyFlags) -> bool {
        self.needs_camera_update(current)
            || self.needs_candle_rebuild(current)
            || self.needs_indicator_rebuild(current)
            || self.needs_crosshair_update(current)
            || self.needs_level_rebuild(current)
            || self.needs_grid_rebuild(current)
            || self.needs_theme_rebuild(current)
    }

    /// Record that we have processed all current generations.
    /// Call this at the end of Primitive::prepare() after all
    /// GPU uploads are done.
    pub fn acknowledge(&mut self, current: &DirtyFlags) {
        self.last_seen = current.clone();
    }
}

impl Viewport {
    /// The content area where candles are drawn (excludes axis margins).
    pub fn content_rect(&self) -> ContentRect {
        ContentRect {
            x: self.margin_left,
            y: self.margin_top,
            width: self.width - self.margin_left - self.margin_right,
            height: self.height - self.margin_top - self.margin_bottom,
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub struct ContentRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}
```

### Key Invariants

1. `camera.time_start < camera.time_end` --- always. Enforced after every mutation.
2. `camera.price_low < camera.price_high` --- always. Enforced after every mutation.
3. `y_axis.target_low < y_axis.target_high` --- always.
4. `InteractionState` is a strict state machine --- transitions are explicit (see Section 2).
5. `DirtyFlags` use generation counters incremented by mutation functions. The renderer's `DirtyTracker` compares last-seen vs current generations -- no clearing is needed.
6. `VelocityBuffer` only holds the last 8 samples within a 150ms window. Older samples are discarded on insertion to prevent stale velocity data from affecting momentum calculations.

---

## 1b. ChartInput and compute_chart_scene

### ChartInput — Clean Input Contract

`ChartInput` defines the exact data needed to produce a renderable `ChartScene`. It replaces
the old pattern of `ChartProgram` taking `&MidasApp` (the entire application state) and
cherry-picking fields. By making the input explicit, we gain:

- **Testability**: construct a `ChartInput` in a test without any iced/wgpu context.
- **Decoupling**: chart logic has zero dependencies on iced or wgpu.
- **Clarity**: you can read the function signature and know exactly what data flows in.

```rust
/// Clean input contract for chart scene computation.
/// Replaces the old pattern of ChartProgram taking &MidasApp.
/// Lives in midas-chart crate.
pub struct ChartInput<'a> {
    pub data: &'a dyn CandleData,
    pub camera: &'a Camera2D,
    pub viewport: &'a Viewport,
    pub theme: &'a ChartTheme,
    pub crosshair: Option<&'a CrosshairState>,
    pub levels: &'a [HorizontalLevel],
    pub indicators: &'a [IndicatorOutput],
    pub dirty: &'a DirtyFlags,
}
```

### compute_chart_scene — Pure Function

This function is the heart of the chart component. It transforms chart state into a
framework-agnostic `ChartScene` (defined in gpu-rendering-architecture.md Section 1.6).

```rust
/// Pure function: chart logic → renderable scene. No GPU, no framework.
/// Lives in midas-chart crate.
pub fn compute_chart_scene(input: &ChartInput) -> ChartScene {
    // 1. Compute visible candle range from camera time bounds
    // 2. Apply LOD downsampling if needed
    // 3. Build CandleInstance array (pixel positions from camera transforms)
    // 4. Build VolumeInstance array
    // 5. Compute grid line positions (adaptive density)
    // 6. Compute axis labels (price formatting, time formatting)
    // 7. Compute horizontal level render data
    // 8. Compute crosshair render data (snap to nearest candle)
    // 9. Compute projection matrix
    // 10. Return ChartScene with generation counters from DirtyFlags
}
```

**This function is unit-testable without any GPU context.** You can assert on the output
`CandleInstance` positions, verify grid line spacing, check label formatting, validate
crosshair snap behavior, and test LOD transitions — all in pure Rust with no iced or
wgpu dependency. The function has zero framework dependencies.

The call site (in iced's `Program::draw()`) constructs a `ChartInput` from the widget's
state and calls `compute_chart_scene()`. The result is wrapped in a `ChartPrimitive`
and handed off to iced's rendering pipeline. See iced-application-shell.md Section 5
for the updated `ChartProgram` implementation.

---

## 2. Input Event Handling

### Event Flow Architecture

```
iced runtime
    |
    v
chart_widget.rs :: update(event, bounds, cursor)       ← midas-app (iced adapter)
    |
    |-- Translates shader::Event into ChartInputEvent
    |-- Adjusts coordinates: screen -> chart-widget-local
    |
    v
─────────────── Sans-IO boundary ───────────────────
    |
    v
chart_state.rs :: handle_input(event) -> Vec<ChartAction>   ← midas-chart (no iced dep)
    |
    |-- State machine transitions
    |-- Produces ChartAction variants
    |
─────────────── Sans-IO boundary ───────────────────
    |
    v
app.rs :: update(Message::ChartAction(chart_id, action))    ← midas-app
    |
    |-- Applies actions: camera mutation, level creation, sync propagation
    |-- Sets dirty flags
    |-- Returns iced Command (subscriptions, data loading, etc.)
```

### ChartInputEvent Enum

The widget layer normalizes iced events into a chart-specific event type. This decouples chart logic from iced's event representation.

```rust
#[derive(Clone, Debug)]
pub enum ChartInputEvent {
    /// Mouse moved to a new position (always fired, even without button down).
    MouseMoved {
        pos: ScreenPoint,
    },

    /// Mouse button pressed.
    MouseDown {
        pos: ScreenPoint,
        button: MouseButton,
    },

    /// Mouse button released.
    MouseUp {
        pos: ScreenPoint,
        button: MouseButton,
    },

    /// Mouse wheel scrolled. delta_y is positive for "scroll up" (zoom in or
    /// scroll right, depending on modifiers). delta_x for horizontal scroll
    /// (trackpad).
    MouseWheel {
        pos: ScreenPoint,
        delta_x: f32,
        delta_y: f32,
        modifiers: Modifiers,
    },

    /// Double-click detected (iced fires this as a separate event on some
    /// platforms; on others we detect it ourselves via timing).
    DoubleClick {
        pos: ScreenPoint,
        button: MouseButton,
    },

    /// Mouse left the chart widget area.
    MouseLeft,

    /// Keyboard key pressed while chart is focused.
    KeyDown {
        key: Key,
        modifiers: Modifiers,
    },

    /// Keyboard key released.
    KeyUp {
        key: Key,
        modifiers: Modifiers,
    },
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum MouseButton { Left, Right, Middle }

#[derive(Copy, Clone, Debug, Default)]
pub struct Modifiers {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}
```

### ChartAction Enum

Output actions produced by the state machine. The application layer applies these.

```rust
#[derive(Clone, Debug)]
pub enum ChartAction {
    /// Pan the camera by a pixel delta.
    Pan { dx: f32, dy: f32 },

    /// Zoom the time axis centered on a pixel X position.
    Zoom { center_x: f32, factor: f64 },

    /// Update crosshair position.
    UpdateCrosshair { screen_pos: ScreenPoint },

    /// Clear crosshair (mouse left chart).
    ClearCrosshair,

    /// Create a new horizontal level at the given price.
    CreateLevel { price: f64 },

    /// Begin dragging a level.
    BeginLevelDrag { level_id: LevelId, grab_offset: f32 },

    /// Move a level being dragged to a new price.
    MoveLevel { level_id: LevelId, new_price: f64 },

    /// Finish dragging a level (commit position).
    EndLevelDrag { level_id: LevelId },

    /// Select a level (for deletion, style changes).
    SelectLevel { level_id: LevelId },

    /// Deselect all levels.
    DeselectLevels,

    /// Delete the currently selected level.
    DeleteSelectedLevel,

    /// Start pan momentum (released after drag).
    StartMomentum { vx: f32, vy: f32 },

    /// Sync crosshair time to linked charts.
    SyncCrosshairTime { time: f64 },

    /// Request to load more data (panned past available data boundary).
    RequestDataLoad { direction: DataLoadDirection },

    /// Show context menu for a level.
    ShowLevelContextMenu { level_id: LevelId, screen_pos: ScreenPoint },

    /// No-op (event consumed, no action needed).
    None,
}

#[derive(Clone, Debug)]
pub enum DataLoadDirection { Past, Future }
```

### Interaction State Machine

> **Sans-IO Design**: The interaction state machine (`handle_input() → Vec<ChartAction>`)
> lives in `midas-chart` with ZERO iced dependencies. It takes framework-agnostic
> `ChartInputEvent` values (not iced events). The iced adapter in `midas-app`
> (`chart_widget.rs`) translates `shader::Event` → `ChartInputEvent` before calling
> the state machine. This means the entire interaction model — pan, zoom, drag,
> crosshair, level placement — is testable without an iced runtime or a GPU context.

The state machine prevents ambiguous interactions. The critical problem it solves: when a user presses the mouse button, we do not yet know if they intend to **pan the chart**, **drag a level**, or **click to select/deselect**.

#### State Transitions (ASCII Diagram)

```
                           MouseDown (on chart area)
                                    |
                                    v
    +-------+           +-----------------------+
    |       |           |     PendingDrag       |
    | Idle  |---------->|  start_screen, time   |
    |       |           +-----------------------+
    +-------+                |            |
       ^  ^                  |            |
       |  |     MouseMoved   |            | MouseMoved
       |  |     (< 4px,      |            | (>= 4px from start)
       |  |      < 200ms)    |            |
       |  |                  |            +---> Hit-test levels at start_screen:
       |  |                  |                    |
       |  |                  |              +-----+------+
       |  |                  |              |            |
       |  |                  |          hit level    no hit
       |  |                  |              |            |
       |  |                  |              v            v
       |  |                  |      +------------+  +----------+
       |  |                  |      | Dragging   |  | Panning  |
       |  |                  |      | Level      |  |          |
       |  |                  |      +------------+  +----------+
       |  |                  |           |               |
       |  |                  |  MouseUp  |      MouseUp  |
       |  |                  |           v               v
       |  |    MouseUp       |    EndLevelDrag    StartMomentum
       |  |    (< 200ms,     |           |          (if velocity
       |  |     < 4px moved) |           |           > threshold)
       |  |         |        |           |               |
       |  |         v        |           |               |
       |  |   Single Click   |           |               |
       |  |   (hit-test      |           |               |
       |  |    levels ->     |           |               |
       |  |    select or     |           |               |
       |  |    deselect)     |           |               |
       |  |         |        |           |               |
       |  +---------+--------+-----------+---------------+
       |                                 |
       |   DoubleClick event             |
       +<--- CreateLevel ---------------+

                           (all terminal transitions go back to Idle)
```

#### Double-Click Detection

If the platform (iced / winit) does not provide a dedicated double-click event, we detect it manually:

```rust
const DOUBLE_CLICK_TIME_MS: f64 = 400.0;
const DOUBLE_CLICK_DISTANCE_PX: f32 = 6.0;

pub struct DoubleClickDetector {
    last_click_time: f64,
    last_click_pos: ScreenPoint,
}

impl DoubleClickDetector {
    pub fn on_click(&mut self, pos: ScreenPoint, time: f64) -> bool {
        let dt = time - self.last_click_time;
        let dist = ((pos.x - self.last_click_pos.x).powi(2)
                   + (pos.y - self.last_click_pos.y).powi(2)).sqrt();
        let is_double = dt < DOUBLE_CLICK_TIME_MS && dist < DOUBLE_CLICK_DISTANCE_PX;

        self.last_click_time = time;
        self.last_click_pos = pos;
        is_double
    }
}
```

#### Drag Threshold

A drag is not initiated until the mouse moves at least 4 logical pixels from the mousedown point. This prevents accidental micro-drags when the user intends to click.

```rust
const DRAG_THRESHOLD_PX: f32 = 4.0;

fn exceeds_drag_threshold(current: ScreenPoint, start: ScreenPoint) -> bool {
    let dx = current.x - start.x;
    let dy = current.y - start.y;
    (dx * dx + dy * dy).sqrt() >= DRAG_THRESHOLD_PX
}
```

#### Level Hit-Testing

When transitioning from `PendingDrag` to either `DraggingLevel` or `Panning`, we need to determine if the mousedown was on a horizontal level:

```rust
const LEVEL_HIT_TOLERANCE_PX: f32 = 6.0;  // pixels above/below the line

fn hit_test_levels(
    levels: &BTreeMap<LevelId, HorizontalLevel>,
    cursor_price: f64,
    transforms: &CoordinateTransforms,
) -> Option<(LevelId, f32)> {
    let cursor_y = transforms.price_to_y(cursor_price);

    let mut closest: Option<(LevelId, f32)> = None;
    for (id, level) in levels {
        let level_y = transforms.price_to_y(level.price);
        let dist = (cursor_y - level_y).abs();
        if dist <= LEVEL_HIT_TOLERANCE_PX {
            match closest {
                None => closest = Some((*id, cursor_y - level_y)),
                Some((_, prev_dist)) if dist < prev_dist.abs() => {
                    closest = Some((*id, cursor_y - level_y));
                }
                _ => {}
            }
        }
    }
    closest
}
```

The `grab_offset` (distance from cursor to the line center) is preserved so the level does not "jump" to the cursor when dragging begins.

### Cursor Icon Feedback

The chart widget returns different cursor icons depending on state:

| State | Cursor Icon |
|---|---|
| Idle, mouse over chart area | Crosshair |
| Idle, mouse over horizontal level | ResizeVertical (double-headed arrow) |
| Idle, mouse over Y-axis | ResizeVertical |
| Panning | Grabbing (closed hand) |
| DraggingLevel | ResizeVertical |
| Idle, mouse over X-axis | ResizeHorizontal |

---

## 3. Zoom Mechanics

### Zoom-to-Cursor Algorithm

Zooming must be anchored to the cursor position so the data point under the cursor stays fixed on screen. This is the single most important property for zoom to feel correct.

```
Before zoom:
  time_start ----[.......C............]---- time_end
                         ^
                    cursor_time (anchor)

After zoom (in):
  time_start' --------[..C......]---------- time_end'
                         ^
                    cursor_time (unchanged screen position)
```

The math: the ratio of (cursor_time - time_start) to the total visible range must be preserved.

```rust
impl Camera2D {
    /// Zoom the time axis centered on cursor_x (in screen-local pixels).
    /// factor > 1.0 = zoom in (less time visible), factor < 1.0 = zoom out.
    pub fn zoom_time_at(
        &mut self,
        cursor_x: f32,
        factor: f64,
        viewport: &Viewport,
        limits: &ZoomLimits,
    ) {
        let content = viewport.content_rect();

        // Where is the cursor in the content area? (0.0 = left edge, 1.0 = right edge)
        let cursor_ratio = ((cursor_x - content.x) / content.width) as f64;
        let cursor_ratio = cursor_ratio.clamp(0.0, 1.0);

        // What time is under the cursor?
        let visible_duration = self.time_end - self.time_start;
        let cursor_time = self.time_start + cursor_ratio * visible_duration;

        // New visible duration after zoom.
        let new_duration = visible_duration / factor;

        // Clamp to zoom limits.
        let new_duration = new_duration.clamp(
            limits.min_visible_duration_ms,
            limits.max_visible_duration_ms,
        );

        // Recompute time_start and time_end preserving cursor_ratio.
        self.time_start = cursor_time - cursor_ratio * new_duration;
        self.time_end = cursor_time + (1.0 - cursor_ratio) * new_duration;
    }
}
```

### Zoom Limits

```rust
pub struct ZoomLimits {
    /// Minimum visible time range: ~10 candles at current timeframe.
    /// For 1-minute candles: 10 * 60_000 = 600_000ms = 10 minutes.
    pub min_visible_duration_ms: f64,

    /// Maximum visible time range: ~50 years (effectively no limit).
    pub max_visible_duration_ms: f64,

    /// Minimum candle width in logical pixels before LOD kicks in.
    pub min_candle_width_px: f32,

    /// Maximum candle width in logical pixels (extreme zoom-in).
    pub max_candle_width_px: f32,
}

impl ZoomLimits {
    pub fn for_timeframe(tf: Timeframe) -> Self {
        let candle_ms = tf.as_secs() as f64 * 1000.0;
        Self {
            min_visible_duration_ms: candle_ms * 10.0,
            max_visible_duration_ms: 50.0 * 365.25 * 24.0 * 3600.0 * 1000.0,
            min_candle_width_px: 1.0,
            max_candle_width_px: 80.0,
        }
    }
}
```

### Zoom Speed Curve (Non-Linear)

Mouse wheel `delta_y` should not map linearly to zoom factor. Small wheel ticks give fine control; repeated fast scrolling accelerates. We use an exponential mapping:

```rust
/// Convert raw mouse wheel delta to zoom factor.
/// delta > 0 means "scroll up" = zoom in.
/// Returns factor > 1.0 for zoom-in, < 1.0 for zoom-out.
pub fn wheel_delta_to_zoom_factor(delta_y: f32) -> f64 {
    // Base zoom rate: each "click" of the wheel (delta_y ~ +/-1.0)
    // changes the visible range by ~12%.
    const BASE_RATE: f64 = 0.12;

    // Clamp raw delta to prevent insane zoom from high-resolution trackpads.
    let clamped = (delta_y as f64).clamp(-5.0, 5.0);

    // Exponential mapping: factor = e^(rate * delta)
    // Positive delta -> factor > 1 -> zoom in (less visible).
    // Negative delta -> factor < 1 -> zoom out (more visible).
    let exponent = BASE_RATE * clamped;
    exponent.exp()

    // Result examples:
    //   delta = +1.0 -> factor = 1.127 -> visible range shrinks by 11.3%
    //   delta = +3.0 -> factor = 1.433 -> visible range shrinks by 30.2%
    //   delta = -1.0 -> factor = 0.887 -> visible range grows by 12.7%
    //   delta = -3.0 -> factor = 0.698 -> visible range grows by 43.3%
}
```

This curve means:
- Single wheel clicks: fine, precise zoom (~12% per click).
- Fast spinning: accelerating zoom that covers large time ranges quickly.
- The exponential never reaches zero or infinity within the clamp range, so it is always invertible.

### Candle Width and LOD Transitions

The candle width in pixels is a derived value, not stored:

```rust
impl ChartState {
    /// Logical pixels per candle at the current zoom level.
    pub fn candle_width_px(&self) -> f32 {
        let content = self.viewport.content_rect();
        let visible_duration = self.camera.time_end - self.camera.time_start;
        let candle_duration = self.timeframe.as_secs() as f64 * 1000.0;
        let visible_candle_count = visible_duration / candle_duration;
        if visible_candle_count <= 0.0 { return self.viewport.width; }
        content.width / visible_candle_count as f32
    }
}
```

LOD transitions are triggered based on candle width:

| Candle Width (px) | Rendering Mode | LOD Level |
|---|---|---|
| >= 6.0 | Full candle (body + wick + gap between candles) | Full resolution (1:1) |
| 3.0 -- 5.99 | Thin candle (body only, no wick separation) | Full resolution (1:1) |
| 1.5 -- 2.99 | Single vertical line per candle (high-low range) | Full resolution (1:1) |
| 0.75 -- 1.49 | MinMax downsampled (2:1) | 2x downsampled |
| 0.375 -- 0.749 | MinMax downsampled (4:1) | 4x downsampled |
| < 0.375 | MinMax downsampled (Nx1, N doubles each step) | Nx downsampled |

The transition between LOD levels is seamless: the user sees the same price envelope (highs and lows are preserved by MinMax downsampling). The visual change is that individual candles merge into a continuous filled range.

```rust
pub fn select_lod_bucket_size(candle_width_px: f32) -> usize {
    if candle_width_px >= 1.5 {
        1  // No downsampling
    } else {
        // Each halving of candle_width doubles the bucket.
        // bucket_size = ceil(1.5 / candle_width_px)
        let raw = (1.5 / candle_width_px).ceil() as usize;
        // Round up to next power of 2 for cache-friendly access.
        raw.next_power_of_two()
    }
}
```

### Zoom Smoothing (Optional Enhancement)

For a premium feel, zoom can be smoothed so the camera eases into the target range rather than jumping instantly. This is implemented by storing a `zoom_target` and lerping toward it:

```rust
pub struct ZoomAnimation {
    pub target_time_start: f64,
    pub target_time_end: f64,
    pub active: bool,
    pub progress: f32,  // 0.0..1.0
}

impl ZoomAnimation {
    /// Advance the animation by dt seconds.
    /// Uses exponential ease-out for responsive feel.
    pub fn tick(&mut self, camera: &mut Camera2D, dt: f32) {
        if !self.active { return; }

        // Exponential ease-out: spring-like, most of the motion in first frames.
        let t = 1.0 - (-12.0 * dt).exp();  // ~12x per second convergence rate

        camera.time_start += (self.target_time_start - camera.time_start) * t as f64;
        camera.time_end += (self.target_time_end - camera.time_end) * t as f64;

        // Stop when close enough (< 1 pixel of difference).
        let remaining = (self.target_time_end - camera.time_end).abs()
                      + (self.target_time_start - camera.time_start).abs();
        if remaining < 1.0 {
            camera.time_start = self.target_time_start;
            camera.time_end = self.target_time_end;
            self.active = false;
        }
    }
}
```

**Decision**: For v1, apply zoom instantly (no smoothing). Smoothing adds latency between input and response, and financial chart users generally prefer immediate feedback. The zoom animation can be enabled as a user preference later.

---

## 4. Pan Mechanics

### Direct Pan (Click+Drag)

During a `Panning` interaction, each `MouseMoved` event produces a pixel delta which is converted to a time/price delta and applied to the camera:

```rust
impl Camera2D {
    /// Pan by a pixel delta. Converts pixels to data coordinates.
    pub fn pan_by_pixels(
        &mut self,
        dx: f32,
        dy: f32,
        viewport: &Viewport,
        y_locked: bool,
    ) {
        let content = viewport.content_rect();

        // Time: pixels -> milliseconds
        let visible_duration = self.time_end - self.time_start;
        let time_per_pixel = visible_duration / content.width as f64;
        let dt = dx as f64 * time_per_pixel;

        // Note: dx > 0 means mouse moved right, which means we're dragging
        // the chart right, which means we're moving BACKWARD in time.
        self.time_start -= dt;
        self.time_end -= dt;

        // Price: pixels -> price (only if Y is not locked to auto-scale)
        if !y_locked {
            let visible_price_range = self.price_high - self.price_low;
            let price_per_pixel = visible_price_range / content.height as f64;
            // dy > 0 means mouse moved down, which means drag down, which
            // means prices shift UP (screen Y is inverted from price).
            let dp = dy as f64 * price_per_pixel;
            self.price_low += dp;
            self.price_high += dp;
        }
    }
}
```

### Mouse Wheel Horizontal Scroll

When `Ctrl` is NOT held, mouse wheel scrolls through time (horizontal pan):

```rust
/// Convert mouse wheel delta to time-axis pan amount.
/// delta_y > 0 = scroll up = move forward in time (show more recent data).
pub fn wheel_delta_to_pan_pixels(delta_y: f32, viewport: &Viewport) -> f32 {
    let content = viewport.content_rect();
    // Each wheel click scrolls ~8% of the visible range.
    const SCROLL_FRACTION: f32 = 0.08;
    -delta_y * content.width * SCROLL_FRACTION
}
```

### Momentum / Inertia (Flick-to-Scroll)

When the user releases a pan drag with velocity, the chart continues scrolling with decelerating momentum. This creates the "flick" feel.

#### Velocity Sampling

During a `Panning` drag, we sample the velocity over the most recent 150ms window (not the entire drag, which would dilute flick speed):

```rust
impl VelocityBuffer {
    pub fn new() -> Self {
        Self {
            samples: [(0.0, 0.0, 0.0); 8],
            head: 0,
            count: 0,
        }
    }

    pub fn push(&mut self, time_ms: f64, dx: f32, dy: f32) {
        self.samples[self.head] = (time_ms, dx, dy);
        self.head = (self.head + 1) % 8;
        if self.count < 8 { self.count += 1; }
    }

    /// Compute average velocity (px/sec) from samples within the last 150ms.
    pub fn average_velocity(&self, current_time_ms: f64) -> (f32, f32) {
        const WINDOW_MS: f64 = 150.0;

        let mut sum_dx: f32 = 0.0;
        let mut sum_dy: f32 = 0.0;
        let mut total_dt: f64 = 0.0;
        let mut prev_time: Option<f64> = None;

        // Walk backward through ring buffer.
        for i in 0..self.count {
            let idx = (self.head + 8 - 1 - i) % 8;
            let (t, dx, dy) = self.samples[idx];
            if current_time_ms - t > WINDOW_MS { break; }

            sum_dx += dx;
            sum_dy += dy;

            if let Some(pt) = prev_time {
                total_dt += pt - t;
            }
            prev_time = Some(t);
        }

        if total_dt < 1.0 {
            // Not enough time data; return zero velocity.
            return (0.0, 0.0);
        }

        let dt_sec = total_dt / 1000.0;
        (sum_dx / dt_sec as f32, sum_dy / dt_sec as f32)
    }
}
```

#### Deceleration

Momentum uses exponential decay (not linear friction), which feels natural and always converges:

```rust
impl PanMomentum {
    /// Friction coefficient. Higher = faster deceleration.
    /// 6.0 gives roughly a 0.5-second coast for a medium flick.
    const FRICTION: f32 = 6.0;

    /// Minimum velocity threshold; below this, snap to zero.
    const MIN_VELOCITY_PX_PER_SEC: f32 = 5.0;

    /// Start momentum with an initial velocity.
    pub fn start(&mut self, vx: f32, vy: f32) {
        // Only start if velocity exceeds minimum threshold.
        if vx.abs() < Self::MIN_VELOCITY_PX_PER_SEC
            && vy.abs() < Self::MIN_VELOCITY_PX_PER_SEC
        {
            self.active = false;
            return;
        }
        self.vx = vx;
        self.vy = vy;
        self.active = true;
    }

    /// Advance momentum by dt seconds. Returns pixel displacement this frame.
    pub fn tick(&mut self, dt: f32) -> (f32, f32) {
        if !self.active { return (0.0, 0.0); }

        // Exponential decay: v(t+dt) = v(t) * e^(-friction * dt)
        let decay = (-Self::FRICTION * dt).exp();

        // Displacement is integral of v(t) from 0 to dt:
        //   integral = v * (1 - e^(-friction*dt)) / friction
        let factor = (1.0 - decay) / Self::FRICTION;
        let dx = self.vx * factor;
        let dy = self.vy * factor;

        // Update velocity.
        self.vx *= decay;
        self.vy *= decay;

        // Stop when velocity drops below threshold.
        if self.vx.abs() < Self::MIN_VELOCITY_PX_PER_SEC
            && self.vy.abs() < Self::MIN_VELOCITY_PX_PER_SEC
        {
            self.vx = 0.0;
            self.vy = 0.0;
            self.active = false;
        }

        (dx, dy)
    }

    /// Immediately stop momentum (e.g., user clicks during coast).
    pub fn stop(&mut self) {
        self.vx = 0.0;
        self.vy = 0.0;
        self.active = false;
    }
}
```

**Deceleration curve visualization:**

```
Velocity (px/s)
  1000 |*
       | *
       |  *
   500 |   *
       |     *
       |       *
   100 |          *  *  *  *
       |                      *  *  (below threshold, stop)
     0 +--|--|--|--|--|--|--|--|--|-->  Time (seconds)
       0  0.1 0.2 0.3 0.4 0.5 0.6 0.7
```

### Boundary Behavior (Past-Data Panning)

When the user pans past the available data, we allow limited over-scroll with an elastic bounce-back:

```rust
/// How far past the data boundary the user can pan, as a fraction of the
/// visible range. 0.5 = can pan half a screen past the data edge.
const MAX_OVERSCROLL_FRACTION: f64 = 0.5;

/// Elastic resistance factor. How much harder it gets to pan further past
/// the boundary. 0.0 = no resistance, 1.0 = full wall.
const ELASTIC_RESISTANCE: f64 = 0.7;

impl Camera2D {
    /// Apply elastic boundary enforcement. Call after every pan/momentum tick.
    /// Returns true if the camera was clamped (triggers bounce-back animation).
    pub fn enforce_boundaries(
        &mut self,
        data_time_min: f64,
        data_time_max: f64,
    ) -> bool {
        let visible = self.time_end - self.time_start;
        let max_overscroll = visible * MAX_OVERSCROLL_FRACTION;

        let mut clamped = false;

        // Panned too far into the past?
        if self.time_end < data_time_min + visible * (1.0 - MAX_OVERSCROLL_FRACTION) {
            clamped = true;
        }

        // Panned too far into the future?
        if self.time_start > data_time_max - visible * (1.0 - MAX_OVERSCROLL_FRACTION) {
            clamped = true;
        }

        clamped
    }

    /// Apply elastic "rubber-band" effect: dampen movement past boundaries.
    /// Call during active pan to reduce dx when beyond data bounds.
    pub fn apply_elastic_resistance(
        &self,
        dx_ms: f64,
        data_time_min: f64,
        data_time_max: f64,
    ) -> f64 {
        let visible = self.time_end - self.time_start;

        // How far past the boundary are we?
        let past_overshoot = data_time_min - self.time_start;   // positive if overscrolled
        let future_overshoot = self.time_end - data_time_max;   // positive if overscrolled

        if (past_overshoot > 0.0 && dx_ms > 0.0)     // panning further past
           || (future_overshoot > 0.0 && dx_ms < 0.0) // panning further future
        {
            let overshoot = past_overshoot.max(future_overshoot).max(0.0);
            let ratio = (overshoot / (visible * MAX_OVERSCROLL_FRACTION)).min(1.0);
            // Reduce movement exponentially as we go further out.
            dx_ms * (1.0 - ratio * ELASTIC_RESISTANCE)
        } else {
            dx_ms
        }
    }
}
```

When the user releases a pan (or momentum stops) and the camera is past a data boundary, a **bounce-back animation** springs it back:

```rust
pub struct BounceBackAnimation {
    pub target_time_start: f64,
    pub target_time_end: f64,
    pub active: bool,
}

impl BounceBackAnimation {
    /// Tick: spring back toward target.
    pub fn tick(&mut self, camera: &mut Camera2D, dt: f32) {
        if !self.active { return; }

        let t = 1.0 - (-10.0 * dt).exp();  // Fast spring-back

        camera.time_start += (self.target_time_start - camera.time_start) * t as f64;
        camera.time_end += (self.target_time_end - camera.time_end) * t as f64;

        let remaining = (self.target_time_end - camera.time_end).abs()
                      + (self.target_time_start - camera.time_start).abs();
        if remaining < 0.5 {
            camera.time_start = self.target_time_start;
            camera.time_end = self.target_time_end;
            self.active = false;
        }
    }
}
```

### Data Loading Trigger

When the user pans close to the edge of loaded data, we proactively request more:

```rust
/// Fraction of visible range. When the camera edge is within this fraction
/// of the data boundary, trigger a data load.
const PREFETCH_THRESHOLD: f64 = 0.25;

impl ChartState {
    pub fn check_data_prefetch(&self) -> Option<DataLoadDirection> {
        let visible = self.camera.time_end - self.camera.time_start;
        let threshold = visible * PREFETCH_THRESHOLD;

        if self.camera.time_start - self.data_time_min < threshold {
            Some(DataLoadDirection::Past)
        } else if self.data_time_max - self.camera.time_end < threshold {
            Some(DataLoadDirection::Future)
        } else {
            None
        }
    }
}
```

---

## 5. Y-Axis Auto-Scaling

### Algorithm

Auto-scaling computes the visible price range from the data and animates the camera to fit it.

```rust
impl ChartState {
    /// Compute the target Y-axis range for the currently visible data.
    /// Returns (target_low, target_high) with padding applied.
    pub fn compute_auto_scale_target(
        &self,
        data: &CandleBuffer,
    ) -> Option<(f64, f64)> {
        // Step 1: Find the indices of visible candles.
        let start_idx = data.find_index_by_time(self.camera.time_start as i64);
        let end_idx = data.find_index_by_time(self.camera.time_end as i64);

        if start_idx >= end_idx { return None; }

        // Step 2: Compute min(low) and max(high) over the visible range.
        // This must be FAST --- called every frame during pan.
        let (data_low, data_high) = data.price_range(start_idx..end_idx);

        if data_low >= data_high { return None; }

        // Step 3: Apply padding.
        let range = (data_high - data_low) as f64;
        let padding = range * self.y_axis.padding_factor;

        let target_low = data_low as f64 - padding;
        let target_high = data_high as f64 + padding;

        Some((target_low, target_high))
    }
}
```

### The price_range Scan (SIMD-Friendly)

This is the hottest function during pan: it scans the `lows[]` and `highs[]` arrays to find min/max. The SoA layout of `CandleBuffer` makes this maximally cache-friendly.

```rust
impl CandleBuffer {
    /// Find min(lows) and max(highs) over a range.
    /// The SoA layout means lows[] and highs[] are contiguous in memory.
    /// For 5000 candles: lows is 20KB, highs is 20KB --- both fit in L1 cache.
    /// The compiler auto-vectorizes this to SIMD on x86_64 with -C target-cpu=native.
    pub fn price_range(&self, range: std::ops::Range<usize>) -> (f32, f32) {
        let lows = &self.lows[range.clone()];
        let highs = &self.highs[range];

        let mut min_low = f32::INFINITY;
        let mut max_high = f32::NEG_INFINITY;

        // This loop will auto-vectorize to 8-wide AVX2 f32 operations.
        for (&lo, &hi) in lows.iter().zip(highs.iter()) {
            if lo < min_low { min_low = lo; }
            if hi > max_high { max_high = hi; }
        }

        (min_low, max_high)
    }
}
```

### Outlier Handling

Extreme outlier candles (flash crashes, data errors) can cause the Y-axis to zoom out excessively. We use percentile-based clamping:

```rust
/// If the price range exceeds this multiple of the median candle range,
/// assume outliers and use a tighter bound.
const OUTLIER_RANGE_MULTIPLE: f64 = 10.0;

pub fn price_range_with_outlier_filtering(
    data: &CandleBuffer,
    range: std::ops::Range<usize>,
) -> (f64, f64) {
    let (raw_low, raw_high) = data.price_range(range.clone());
    let raw_range = (raw_high - raw_low) as f64;

    // Compute median candle range (high - low) for the visible set.
    let candle_ranges: Vec<f32> = data.highs[range.clone()].iter()
        .zip(data.lows[range].iter())
        .map(|(h, l)| h - l)
        .collect();

    if candle_ranges.is_empty() { return (raw_low as f64, raw_high as f64); }

    let mut sorted = candle_ranges;
    sorted.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());
    let median_range = sorted[sorted.len() / 2] as f64;

    if median_range > 0.0 && raw_range > median_range * OUTLIER_RANGE_MULTIPLE {
        // Use 2nd and 98th percentile of lows/highs instead.
        let mut sorted_lows: Vec<f32> = data.lows[range.clone()].to_vec();
        let mut sorted_highs: Vec<f32> = data.highs[range].to_vec();
        sorted_lows.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());
        sorted_highs.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());

        let p2 = sorted_lows[sorted_lows.len() * 2 / 100] as f64;
        let p98 = sorted_highs[sorted_highs.len() * 98 / 100] as f64;
        (p2, p98)
    } else {
        (raw_low as f64, raw_high as f64)
    }
}
```

### Animated Transition

The Y-axis transitions smoothly between auto-scale targets using exponential ease-out:

```rust
impl YAxisState {
    /// Tick the Y-axis animation. Called every frame.
    /// dt is in seconds (typically 1/60 = 0.0167).
    pub fn tick(
        &mut self,
        camera: &mut Camera2D,
        dt: f32,
    ) {
        if !self.animating || self.locked { return; }

        // Exponential ease-out with convergence rate ~8x/sec.
        // This means after 1 frame at 60fps (dt=0.0167s):
        //   t = 1 - e^(-8*0.0167) = 0.125 -> moves 12.5% of remaining distance.
        // After 5 frames: ~50% of distance covered.
        // After 15 frames (~250ms): ~90% covered. Feels snappy.
        let t = 1.0 - (-8.0 * dt as f64).exp();

        camera.price_low += (self.target_low - camera.price_low) * t;
        camera.price_high += (self.target_high - camera.price_high) * t;

        // Convergence check: within 0.01% of price range.
        let target_range = self.target_high - self.target_low;
        let remaining = (self.target_low - camera.price_low).abs()
                      + (self.target_high - camera.price_high).abs();

        if remaining < target_range * 0.0001 {
            camera.price_low = self.target_low;
            camera.price_high = self.target_high;
            self.animating = false;
        }
    }

    /// Set a new auto-scale target. If already animating, the target changes
    /// smoothly (no discontinuity).
    pub fn set_target(&mut self, target_low: f64, target_high: f64) {
        // Guard: ignore targets where the range is non-positive.
        if target_high <= target_low { return; }

        self.target_low = target_low;
        self.target_high = target_high;
        self.animating = true;
    }
}
```

### Manual Y-Axis Lock

The user can lock the Y-axis by double-clicking the Y-axis area, or via a toolbar toggle. When locked:
- Auto-scale animation stops.
- Y-axis follows drag input (Shift+drag to scale Y, regular drag to pan Y).
- A lock icon appears on the Y-axis.
- Auto-scale resumes when unlocked.

```rust
impl YAxisState {
    pub fn toggle_lock(&mut self, camera: &Camera2D) {
        self.locked = !self.locked;
        if !self.locked {
            // Re-enable auto-scale: target snaps to current values
            // (next frame's auto-scale computation will update the target).
            self.animating = false;
        }
    }
}
```

---

## 6. Crosshair System

### Snap-to-Candle Logic

The vertical crosshair line snaps to the nearest candle center, not to the raw cursor position. This ensures the OHLCV tooltip always shows a specific candle's data.

```rust
impl ChartState {
    /// Given a screen X coordinate, find the nearest candle index and its
    /// snapped screen X position.
    pub fn snap_to_nearest_candle(
        &self,
        screen_x: f32,
        data: &CandleBuffer,
        transforms: &CoordinateTransforms,
    ) -> Option<(usize, f32)> {
        // Convert screen X to time.
        let time = transforms.x_to_time(screen_x);

        // Binary search for the nearest candle by timestamp.
        let idx = data.find_nearest_index_by_time(time as i64);
        if idx >= data.len() { return None; }

        // Compute the screen X of that candle's center.
        let candle_time = data.timestamps[idx] as f64;
        let snapped_x = transforms.time_to_x(candle_time);

        // Reject if the snapped position is outside the content area
        // (candle is off-screen).
        let content = self.viewport.content_rect();
        if snapped_x < content.x || snapped_x > content.x + content.width {
            return None;
        }

        Some((idx, snapped_x))
    }
}

impl CandleBuffer {
    /// Find the index of the candle nearest to the given timestamp.
    /// Uses binary search for O(log n).
    pub fn find_nearest_index_by_time(&self, ts: i64) -> usize {
        match self.timestamps.binary_search(&ts) {
            Ok(exact) => exact,
            Err(insert_pos) => {
                if insert_pos == 0 { return 0; }
                if insert_pos >= self.len() { return self.len() - 1; }
                // Choose the closer of the two neighbors.
                let before = self.timestamps[insert_pos - 1];
                let after = self.timestamps[insert_pos];
                if (ts - before).abs() <= (after - ts).abs() {
                    insert_pos - 1
                } else {
                    insert_pos
                }
            }
        }
    }
}
```

### OHLCV Tooltip Content

The tooltip displays data for the candle under the crosshair:

```
  AAPL  5m  2026-03-24 14:35
  O 178.42   H 178.89
  L 178.21   C 178.67
  Vol 1,234,567   +0.14%
```

Content fields:
- Symbol and timeframe (from ChartState).
- Date/time formatted for the current timeframe (e.g., `HH:MM` for intraday, `YYYY-MM-DD` for daily).
- Open, High, Low, Close formatted to the appropriate tick size (2 decimals for stocks > $1, 4 for < $1).
- Volume with thousands separators.
- Change percentage: `(close - open) / open * 100`, colored green/red.

### Tooltip Positioning

The tooltip must never obscure the candle it describes and must stay within the chart bounds:

```
  Positioning rules:
  1. Default: top-left corner of chart content area (fixed position,
     not following cursor). This is the TC2000 style --- OHLCV data in
     the chart header, not a floating tooltip.
  2. Alternative (user preference): floating near cursor.
     - Place tooltip 20px above and 20px right of cursor.
     - If that would go off the right edge, flip to left of cursor.
     - If that would go off the top edge, flip to below cursor.
```

For v1, use the **header style** (fixed position): OHLCV data renders at the top of the chart content area, always visible, updated as the crosshair moves. This avoids all tooltip positioning complexity and is what professional charting apps use.

### Performance: Overlay-Only Rendering

The crosshair must NOT trigger a full chart re-render (candle data, indicators, grid). It is rendered as a separate overlay pass:

```
Render order (per frame):
  1. [SKIP if candles not dirty] Render candles + volume + grid + indicators
     -> Result stored in chart texture (persistent between frames)
  2. [ALWAYS] Composite chart texture to screen
  3. [ALWAYS if crosshair active] Render crosshair overlay (2 lines + labels)
  4. [ALWAYS] Render horizontal levels overlay
  5. [ALWAYS] Render axis labels
```

The crosshair pipeline renders at most:
- 2 full-width/full-height lines (1px each).
- 2 small label backgrounds (rectangles).
- ~20 glyph quads (text characters).

This is trivially fast --- effectively free even at 144fps.

The key optimization: the main chart content (candles, volume, indicators) is rendered to a texture (or rendered via a persistent command buffer) that is only rebuilt when the `candles` generation counter has changed. Mouse movement only increments `dirty.crosshair`, which triggers only steps 2-5.

### Crosshair Rendering Details

```rust
/// Data passed to the crosshair GPU pipeline each frame.
pub struct CrosshairRenderData {
    /// Vertical line X position (snapped to candle center), in screen pixels.
    pub vertical_x: f32,

    /// Horizontal line Y position (at cursor), in screen pixels.
    pub horizontal_y: f32,

    /// Content area bounds (lines are clipped to this region).
    pub content_rect: ContentRect,

    /// Price label text and position (on Y-axis).
    pub price_label: AxisLabel,

    /// Time label text and position (on X-axis).
    pub time_label: AxisLabel,

    /// Line color (typically semi-transparent white or gray).
    pub line_color: [f32; 4],

    /// Line style: solid for now, dashed in future.
    pub dash_pattern: DashPattern,
}

pub struct AxisLabel {
    pub text: String,
    pub screen_x: f32,
    pub screen_y: f32,
    pub bg_color: [f32; 4],
    pub text_color: [f32; 4],
}
```

---

## 7. Horizontal Levels

### Data Model

```rust
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct HorizontalLevel {
    pub id: LevelId,

    /// Price at which the line is drawn.
    pub price: f64,

    /// Visual properties.
    pub color: [f32; 4],
    pub line_width: f32,         // Logical pixels (1.0, 2.0, 3.0)
    pub line_style: LineStyle,

    /// Optional text label displayed on the line.
    pub label: Option<String>,

    /// If true, user can drag this level. (Always true for user-created levels;
    /// could be false for system-generated levels like support/resistance.)
    pub draggable: bool,

    /// Selection state (not persisted).
    #[serde(skip)]
    pub selected: bool,

    /// Timestamp when created (for ordering in persistence).
    pub created_at: i64,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum LineStyle {
    Solid,
    Dashed,    // 8px on, 6px off
    Dotted,    // 2px on, 4px off
    DashDot,   // 8px on, 4px off, 2px on, 4px off
}
```

### Creation (Double-Click)

When the user double-clicks on the chart content area (not on an existing level, not on an axis):

```rust
impl ChartState {
    pub fn create_level_at_price(&mut self, price: f64) -> LevelId {
        let id = LevelId(self.next_level_id);
        self.next_level_id += 1;

        let level = HorizontalLevel {
            id,
            price: self.snap_price_to_tick(price),
            color: [0.0, 0.63, 0.94, 0.8],  // Default: semi-transparent blue
            line_width: 1.0,
            line_style: LineStyle::Solid,
            label: None,
            draggable: true,
            selected: true,  // Newly created levels start selected
            created_at: chrono::Utc::now().timestamp_millis(),
        };

        // Deselect all other levels.
        for existing in self.levels.values_mut() {
            existing.selected = false;
        }

        self.levels.insert(id, level);
        self.dirty.mark_levels();
        id
    }
}
```

### Tick-Size Price Snapping

Prices snap to the instrument's tick size to avoid displaying nonsensical precision:

```rust
impl ChartState {
    /// Snap a price to the nearest tick for the current symbol.
    pub fn snap_price_to_tick(&self, price: f64) -> f64 {
        let tick = self.tick_size();
        (price / tick).round() * tick
    }

    /// Determine tick size from price level (simplified US equity rules).
    fn tick_size(&self) -> f64 {
        // Stocks > $1: tick = $0.01
        // Stocks < $1: tick = $0.0001
        // This is a simplification; real tick sizes depend on exchange rules.
        if self.camera.price_high > 1.0 {
            0.01
        } else {
            0.0001
        }
    }
}
```

### Selection (Click on Line)

A single click near a horizontal level selects it:

```rust
impl ChartState {
    pub fn select_level(&mut self, level_id: LevelId) {
        for (id, level) in self.levels.iter_mut() {
            level.selected = *id == level_id;
        }
        self.dirty.mark_levels();
    }

    pub fn deselect_all_levels(&mut self) {
        for level in self.levels.values_mut() {
            level.selected = false;
        }
        self.dirty.mark_levels();
    }

    pub fn selected_level(&self) -> Option<LevelId> {
        self.levels.iter()
            .find(|(_, level)| level.selected)
            .map(|(id, _)| *id)
    }
}
```

### Dragging

During a `DraggingLevel` interaction state:

```rust
impl ChartState {
    /// Move a level to a new price during drag.
    pub fn move_level(
        &mut self,
        level_id: LevelId,
        screen_y: f32,
        grab_offset: f32,
        transforms: &CoordinateTransforms,
    ) {
        let adjusted_y = screen_y - grab_offset;
        let raw_price = transforms.y_to_price(adjusted_y);
        let snapped_price = self.snap_price_to_tick(raw_price);

        if let Some(level) = self.levels.get_mut(&level_id) {
            level.price = snapped_price;
            self.dirty.mark_levels();
        }
    }
}
```

### Deletion

The selected level is deleted by pressing `Delete` or `Backspace`, or via right-click context menu:

```rust
impl ChartState {
    pub fn delete_selected_level(&mut self) -> Option<LevelId> {
        if let Some(id) = self.selected_level() {
            self.levels.remove(&id);
            self.dirty.mark_levels();
            Some(id)
        } else {
            None
        }
    }

    pub fn delete_level(&mut self, id: LevelId) {
        self.levels.remove(&id);
        self.dirty.mark_levels();
    }
}
```

### Visual Feedback During Drag

While dragging a level:
1. The line renders with increased opacity (1.0 instead of 0.8).
2. The price label on the Y-axis updates in real-time as the level moves.
3. A ghost line shows the original position (at 30% opacity) so the user can see how far they have moved it.
4. When the drag ends, the ghost line disappears.

```rust
pub struct LevelRenderData {
    pub price: f64,
    pub screen_y: f32,
    pub color: [f32; 4],
    pub line_width: f32,
    pub line_style: LineStyle,
    pub is_selected: bool,
    pub is_being_dragged: bool,
    pub original_screen_y: Option<f32>,  // Ghost line position during drag
    pub label_text: String,              // Price formatted to tick size
}
```

### Persistence

Horizontal levels are persisted as part of the chart config:

```rust
// In ChartConfig (serialized to TOML):
#[derive(Serialize, Deserialize)]
pub struct ChartConfig {
    pub symbol: String,
    pub timeframe: Timeframe,
    pub horizontal_levels: Vec<HorizontalLevel>,  // Serializable
    // ...
}
```

Save is debounced (at most once per second) and triggered on:
- Level creation.
- Level deletion.
- Level drag end (not during drag --- too many events).
- Level style change.

---

## 8. Multi-Chart Synchronization

### Architecture

Synchronization is managed by a `TimeAxisController` that lives in the application state (not inside any individual chart). It maintains the shared time window for a group of linked charts.

```rust
use std::collections::HashSet;

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub struct SyncGroupId(pub u32);

pub struct TimeAxisController {
    groups: HashMap<SyncGroupId, SyncGroup>,
}

pub struct SyncGroup {
    pub id: SyncGroupId,

    /// The canonical shared time range for this group.
    pub time_start: f64,
    pub time_end: f64,

    /// Which charts belong to this group.
    pub members: HashSet<ChartId>,

    /// The chart that most recently initiated a pan/zoom (to avoid echo).
    pub last_source: Option<ChartId>,
}
```

### Sync Propagation Flow

```
User pans Chart A (member of SyncGroup 1)
    |
    v
ChartState A updates camera.time_start, camera.time_end
    |
    v
App::update produces SyncAction::TimeRangeChanged {
    source: ChartId(A),
    group: SyncGroupId(1),
    time_start: ...,
    time_end: ...,
}
    |
    v
TimeAxisController::propagate(group, source, time_start, time_end)
    |
    v
For each member chart B, C, D, ... (excluding source A):
    |
    +---> ChartState B: set camera.time_start/time_end, dirty.mark_camera()
    +---> ChartState C: set camera.time_start/time_end, dirty.mark_camera()
    +---> ChartState D: set camera.time_start/time_end, dirty.mark_camera()

Each chart's Y-axis auto-scales INDEPENDENTLY based on its own data at the new time range.
```

### Implementation

```rust
impl TimeAxisController {
    pub fn new() -> Self {
        Self { groups: HashMap::new() }
    }

    /// Create a new sync group and return its ID.
    pub fn create_group(&mut self) -> SyncGroupId {
        let id = SyncGroupId(self.groups.len() as u32);
        self.groups.insert(id, SyncGroup {
            id,
            time_start: 0.0,
            time_end: 0.0,
            members: HashSet::new(),
            last_source: None,
        });
        id
    }

    /// Add a chart to a sync group.
    pub fn link_chart(&mut self, group: SyncGroupId, chart: ChartId) {
        if let Some(g) = self.groups.get_mut(&group) {
            g.members.insert(chart);
        }
    }

    /// Remove a chart from its sync group.
    pub fn unlink_chart(&mut self, group: SyncGroupId, chart: ChartId) {
        if let Some(g) = self.groups.get_mut(&group) {
            g.members.remove(&chart);
        }
    }

    /// Called when a chart's time range changes. Returns the list of other
    /// charts that need to be updated.
    pub fn propagate(
        &mut self,
        group: SyncGroupId,
        source: ChartId,
        time_start: f64,
        time_end: f64,
    ) -> Vec<(ChartId, f64, f64)> {
        let Some(g) = self.groups.get_mut(&group) else {
            return vec![];
        };

        g.time_start = time_start;
        g.time_end = time_end;
        g.last_source = Some(source);

        g.members.iter()
            .filter(|&&id| id != source)
            .map(|&id| (id, time_start, time_end))
            .collect()
    }
}
```

### Crosshair Sync

When the user hovers on any chart in a sync group, all other charts in the group show a vertical time marker:

```rust
pub struct CrosshairSync {
    /// Current hover time across the sync group. None if no chart is being hovered.
    pub active_time: Option<f64>,
    pub source_chart: Option<ChartId>,
}

impl CrosshairSync {
    /// Called when a chart's crosshair snaps to a candle.
    pub fn on_crosshair_update(
        &mut self,
        source: ChartId,
        time: f64,
        group: SyncGroupId,
    ) -> Vec<(ChartId, f64)> {
        self.active_time = Some(time);
        self.source_chart = Some(source);

        // All other members receive a "synced crosshair" at this time.
        // (The actual member list is looked up via TimeAxisController.)
        vec![]  // Filled in by the caller using group membership.
    }

    /// Called when the mouse leaves a chart.
    pub fn on_crosshair_clear(
        &mut self,
        source: ChartId,
    ) -> Vec<ChartId> {
        if self.source_chart == Some(source) {
            self.active_time = None;
            self.source_chart = None;
        }
        vec![]  // All other members clear their synced crosshair.
    }
}
```

On each non-source chart, the synced crosshair renders:
1. A vertical line at the time coordinate (snapped to the nearest candle in that chart's data).
2. A time label on the X-axis.
3. NO horizontal line (different symbols have different price scales).
4. OHLCV data for the candle at that time in THAT chart's data (shown in the header area).

### Performance: Sync Without Full Re-Render

Sync propagation MUST NOT trigger a full candle re-render on every mouse move. The performance contract:

| Event | Source Chart | Linked Charts |
|---|---|---|
| Pan (drag) | Full re-render (candles generation changed) | Camera update + Y-axis recompute + mark_data() + mark_camera() |
| Zoom | Full re-render (candles generation changed) | Camera update + Y-axis recompute + mark_data() + mark_camera() |
| Crosshair move | Crosshair overlay only | Crosshair overlay only (vertical line + header) |
| Level drag | Level overlay only | No effect |

For pan/zoom sync, the linked charts do need a candle re-render because the visible candle set changes. But this is already optimized: `to_candle_instances()` is fast (~200us for 5K candles), and the GPU upload is a single `write_buffer` call. At 60fps with 20 charts, this is `20 * 200us = 4ms` of CPU work for instance generation --- well within budget.

For crosshair sync, linked charts do NOT re-render candles. Only the crosshair overlay layer is redrawn.

### Link/Unlink UI

Each chart has a link icon in its header bar:

```
  [AAPL  5m]  [linked-icon] [settings-icon]
```

- Click the link icon to cycle: Linked -> Unlinked -> Linked.
- Unlinked charts have a broken-chain icon and a different header border color.
- When a chart is unlinked, its time axis is independent.
- When re-linked, it snaps to the current group time range (with animation).

```rust
impl ChartState {
    pub fn toggle_sync(&mut self, controller: &mut TimeAxisController, default_group: SyncGroupId) {
        match self.sync_group {
            Some(group) => {
                controller.unlink_chart(group, self.id);
                self.sync_group = None;
            }
            None => {
                controller.link_chart(default_group, self.id);
                self.sync_group = Some(default_group);
                // Snap to group's time range.
                if let Some(g) = controller.groups.get(&default_group) {
                    self.camera.time_start = g.time_start;
                    self.camera.time_end = g.time_end;
                    self.dirty.mark_data();
                    self.dirty.mark_camera();
                }
            }
        }
    }
}
```

---

## 9. Animation System

### What Gets Animated

| Animation | Duration | Easing | Trigger |
|---|---|---|---|
| Y-axis auto-scale | ~250ms (asymptotic) | Exponential ease-out | Pan, zoom, data change |
| Pan momentum | 500ms--2000ms | Exponential decay | Release pan drag with velocity |
| Bounce-back | ~200ms | Exponential ease-out | Pan/momentum past data boundary |
| Zoom smoothing (optional) | ~150ms | Exponential ease-out | Mouse wheel zoom |
| Level drag feedback | Immediate | N/A (direct manipulation) | Mouse move during drag |

### Time-Based vs Frame-Based

All animations use **time-based** computation (delta-time), NOT frame-based (fixed step per frame). This ensures consistent behavior regardless of frame rate (60Hz, 144Hz, 240Hz, or dropped frames).

```rust
// CORRECT: time-based
fn tick(&mut self, dt_seconds: f32) {
    let t = 1.0 - (-8.0 * dt_seconds).exp();
    self.value += (self.target - self.value) * t as f64;
}

// WRONG: frame-based (inconsistent at different frame rates)
fn tick_bad(&mut self) {
    self.value += (self.target - self.value) * 0.125;  // Assumes 60fps!
}
```

### Integration with iced Subscriptions

iced drives animation via a subscription that emits `Tick` messages at a target frame rate. When no animations are active, the subscription is idle (no unnecessary redraws).

```rust
use iced::time;
use std::time::Duration;

impl MidasApp {
    fn subscription(&self) -> iced::Subscription<Message> {
        if self.any_animation_active() {
            // Request 60fps ticks while animating.
            time::every(Duration::from_millis(16))
                .map(|_| Message::AnimationTick)
        } else {
            iced::Subscription::none()
        }
    }

    fn any_animation_active(&self) -> bool {
        self.charts.iter().any(|chart| {
            chart.state.y_axis.animating
            || chart.state.momentum.active
            || chart.state.bounce_back.active
        })
    }
}
```

### Tick Handler

```rust
impl MidasApp {
    fn handle_animation_tick(&mut self, now: std::time::Instant) {
        let dt = now.duration_since(self.last_tick).as_secs_f32();
        self.last_tick = now;

        // Clamp dt to prevent huge jumps (e.g., after app was suspended).
        let dt = dt.min(0.1);  // Max 100ms step

        for chart in &mut self.charts {
            let state = &mut chart.state;

            // 1. Pan momentum.
            if state.momentum.active {
                let (dx, dy) = state.momentum.tick(dt);
                state.camera.pan_by_pixels(
                    dx, dy, &state.viewport, state.y_axis.locked,
                );
                state.dirty.mark_data();
                state.dirty.mark_camera();

                // Check boundary bounce-back.
                if state.momentum.active == false {
                    // Momentum just stopped --- check if we need to bounce back.
                    self.check_bounce_back(chart.id);
                }
            }

            // 2. Bounce-back.
            if state.bounce_back.active {
                state.bounce_back.tick(&mut state.camera, dt);
                state.dirty.mark_data();
                state.dirty.mark_camera();
            }

            // 3. Y-axis auto-scale animation.
            if state.y_axis.animating && !state.y_axis.locked {
                // Recompute target from visible data (it may have changed
                // if momentum is also active).
                if let Some((lo, hi)) = state.compute_auto_scale_target(&chart.data) {
                    state.y_axis.set_target(lo, hi);
                }
                state.y_axis.tick(&mut state.camera, dt);
                state.dirty.mark_data();
                state.dirty.mark_camera();
            }

            // 4. Propagate sync if this chart's camera changed.
            if state.dirty.camera != last_camera_gen {  // camera generation changed
                if let Some(group) = state.sync_group {
                    let updates = self.sync_controller.propagate(
                        group, state.id,
                        state.camera.time_start, state.camera.time_end,
                    );
                    for (target_id, ts, te) in updates {
                        self.apply_synced_camera(target_id, ts, te);
                    }
                }
            }
        }
    }
}
```

### Animation Convergence Guarantee

Every animation uses exponential decay toward a target, which mathematically converges but never actually reaches zero in finite time. We use explicit convergence thresholds to snap to the target and stop:

```rust
// Y-axis: snap when remaining < 0.01% of range
// Pan momentum: snap when velocity < 5 px/sec
// Bounce-back: snap when remaining < 0.5 ms of time
```

These thresholds are small enough to be invisible (sub-pixel) but prevent animations from running indefinitely and burning CPU/GPU.

---

## 10. Coordinate Transforms

### Transform Stack

Every chart has a set of transforms that convert between four coordinate spaces:

```
1. DATA SPACE (time in epoch ms, price in dollars)
       |
       v  [time_to_x, price_to_y]
2. CONTENT SPACE (logical pixels, origin at top-left of content area)
       |
       v  [+ content_rect.x, + content_rect.y]
3. WIDGET SPACE (logical pixels, origin at top-left of chart widget)
       |
       v  [+ widget position on screen, * dpi_scale]
4. PHYSICAL PIXEL SPACE (physical pixels, used for GPU rendering)
```

For most operations, we work in **content space** (steps 1-2) because that is where candles live. The widget-space and physical-pixel-space transforms are applied at the rendering boundary.

### CoordinateTransforms Struct

This struct is recomputed from `Camera2D` + `Viewport` whenever either changes. It is NOT stored in `ChartState` (it is derived, and storing derived state invites bugs). Instead, it is computed at the top of each update/render cycle.

```rust
/// Precomputed transform coefficients for a single chart.
/// Compute once per frame, pass by reference to all functions that need it.
pub struct CoordinateTransforms {
    // --- Content area geometry (logical pixels) ---
    pub content_x: f32,
    pub content_y: f32,
    pub content_width: f32,
    pub content_height: f32,

    // --- Time axis coefficients ---
    /// time_to_x(t) = (t - time_offset) * time_scale + content_x
    pub time_offset: f64,    // = camera.time_start
    pub time_scale: f64,     // = content_width / (time_end - time_start)

    // --- Price axis coefficients ---
    /// price_to_y(p) = content_y + content_height - (p - price_offset) * price_scale
    /// (Note: Y is inverted --- higher price = lower pixel Y)
    pub price_offset: f64,   // = camera.price_low
    pub price_scale: f64,    // = content_height / (price_high - price_low)

    // --- DPI ---
    pub dpi_scale: f32,
}

impl CoordinateTransforms {
    pub fn new(camera: &Camera2D, viewport: &Viewport) -> Self {
        let content = viewport.content_rect();
        let time_range = camera.time_end - camera.time_start;
        let price_range = camera.price_high - camera.price_low;

        Self {
            content_x: content.x,
            content_y: content.y,
            content_width: content.width,
            content_height: content.height,

            time_offset: camera.time_start,
            time_scale: if time_range > 0.0 {
                content.width as f64 / time_range
            } else {
                1.0
            },

            price_offset: camera.price_low,
            price_scale: if price_range > 0.0 {
                content.height as f64 / price_range
            } else {
                1.0
            },

            dpi_scale: viewport.dpi_scale,
        }
    }

    // =================================================================
    //  FORWARD TRANSFORMS: Data -> Screen
    // =================================================================

    /// Convert a timestamp (epoch ms, f64) to content-space X (logical pixels).
    #[inline]
    pub fn time_to_x(&self, time: f64) -> f32 {
        ((time - self.time_offset) * self.time_scale) as f32 + self.content_x
    }

    /// Convert a price (f64) to content-space Y (logical pixels).
    /// Higher prices map to lower Y values (screen Y is inverted).
    #[inline]
    pub fn price_to_y(&self, price: f64) -> f32 {
        self.content_y + self.content_height
            - ((price - self.price_offset) * self.price_scale) as f32
    }

    /// Convert a time duration (ms) to a pixel width.
    #[inline]
    pub fn time_duration_to_pixels(&self, duration_ms: f64) -> f32 {
        (duration_ms * self.time_scale) as f32
    }

    /// Convert a price difference to a pixel height.
    #[inline]
    pub fn price_range_to_pixels(&self, price_diff: f64) -> f32 {
        (price_diff * self.price_scale) as f32
    }

    // =================================================================
    //  INVERSE TRANSFORMS: Screen -> Data
    // =================================================================

    /// Convert content-space X (logical pixels) to timestamp (epoch ms, f64).
    #[inline]
    pub fn x_to_time(&self, x: f32) -> f64 {
        (x - self.content_x) as f64 / self.time_scale + self.time_offset
    }

    /// Convert content-space Y (logical pixels) to price (f64).
    #[inline]
    pub fn y_to_price(&self, y: f32) -> f64 {
        (self.content_y + self.content_height - y) as f64 / self.price_scale
            + self.price_offset
    }

    // =================================================================
    //  PIXEL SNAPPING
    // =================================================================

    /// Snap a logical-pixel value to the nearest physical pixel boundary.
    #[inline]
    pub fn snap_to_pixel(&self, value: f32) -> f32 {
        (value * self.dpi_scale).round() / self.dpi_scale
    }

    /// Snap a time-to-X result to the nearest physical pixel.
    #[inline]
    pub fn time_to_x_snapped(&self, time: f64) -> f32 {
        self.snap_to_pixel(self.time_to_x(time))
    }

    /// Snap a price-to-Y result to the nearest physical pixel.
    #[inline]
    pub fn price_to_y_snapped(&self, price: f64) -> f32 {
        self.snap_to_pixel(self.price_to_y(price))
    }

    // =================================================================
    //  HIT TESTING & REGION CHECKS
    // =================================================================

    /// Is a screen point inside the content area?
    #[inline]
    pub fn is_in_content(&self, point: ScreenPoint) -> bool {
        point.x >= self.content_x
            && point.x <= self.content_x + self.content_width
            && point.y >= self.content_y
            && point.y <= self.content_y + self.content_height
    }

    /// Is a screen point inside the Y-axis area?
    #[inline]
    pub fn is_in_y_axis(&self, point: ScreenPoint, viewport: &Viewport) -> bool {
        point.x > self.content_x + self.content_width
            && point.x <= viewport.width
            && point.y >= self.content_y
            && point.y <= self.content_y + self.content_height
    }

    /// Is a screen point inside the X-axis area?
    #[inline]
    pub fn is_in_x_axis(&self, point: ScreenPoint, viewport: &Viewport) -> bool {
        point.x >= self.content_x
            && point.x <= self.content_x + self.content_width
            && point.y > self.content_y + self.content_height
            && point.y <= viewport.height
    }

    // =================================================================
    //  GPU PROJECTION MATRIX
    // =================================================================

    /// Compute the orthographic projection matrix for the GPU.
    /// Maps content-space logical pixels to NDC (-1..1 for X and Y).
    /// This is uploaded as a uniform to all render pipelines.
    pub fn projection_matrix(&self, viewport: &Viewport) -> glam::Mat4 {
        // Map the full widget area (not just content) to NDC,
        // because grid lines and axes extend outside the content rect.
        glam::Mat4::orthographic_rh(
            0.0,                          // left
            viewport.width * self.dpi_scale,   // right (physical pixels)
            viewport.height * self.dpi_scale,  // bottom (physical pixels)
            0.0,                          // top
            -1.0,                         // near
            1.0,                          // far
        )
    }
}
```

### Round-Trip Accuracy

The transforms must be perfectly invertible within pixel precision:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_time_round_trip() {
        let camera = Camera2D {
            time_start: 1_700_000_000_000.0,  // Nov 2023
            time_end: 1_700_100_000_000.0,    // ~27.8 hours later
            price_low: 150.0,
            price_high: 200.0,
        };
        let viewport = Viewport {
            width: 1200.0, height: 800.0, dpi_scale: 1.0,
            margin_left: 0.0, margin_right: 80.0,
            margin_top: 20.0, margin_bottom: 30.0,
        };
        let t = CoordinateTransforms::new(&camera, &viewport);

        // Round-trip: time -> x -> time
        let original_time = 1_700_050_000_000.0;
        let x = t.time_to_x(original_time);
        let recovered_time = t.x_to_time(x);

        // Should be accurate to within 1 pixel of time.
        let one_pixel_time = (camera.time_end - camera.time_start)
            / viewport.content_rect().width as f64;
        assert!((original_time - recovered_time).abs() < one_pixel_time);
    }

    #[test]
    fn test_price_round_trip() {
        let camera = Camera2D {
            time_start: 0.0, time_end: 1000.0,
            price_low: 150.0, price_high: 200.0,
        };
        let viewport = Viewport {
            width: 1200.0, height: 800.0, dpi_scale: 2.0,
            margin_left: 0.0, margin_right: 80.0,
            margin_top: 20.0, margin_bottom: 30.0,
        };
        let t = CoordinateTransforms::new(&camera, &viewport);

        let original_price = 175.50;
        let y = t.price_to_y(original_price);
        let recovered_price = t.y_to_price(y);

        let one_pixel_price = (camera.price_high - camera.price_low)
            / viewport.content_rect().height as f64;
        assert!((original_price - recovered_price).abs() < one_pixel_price);
    }
}
```

### Candle Geometry Computation

Transforms are used to compute `CandleInstance` data for the GPU. This function runs once per frame when the `candles` generation counter has changed:

```rust
/// Convert visible candle data to GPU instances.
/// This is the bridge between the data layer and the render layer.
pub fn compute_candle_instances(
    data: &CandleBuffer,
    start_idx: usize,
    end_idx: usize,
    transforms: &CoordinateTransforms,
    timeframe: Timeframe,
    theme: &MidasTheme,
) -> Vec<CandleInstance> {
    let candle_duration_ms = timeframe.as_secs() as f64 * 1000.0;

    // Candle body width = 70% of the time slot width.
    let slot_width_px = transforms.time_duration_to_pixels(candle_duration_ms);
    let body_width = transforms.snap_to_pixel(slot_width_px * 0.7);
    let body_width = body_width.max(1.0);  // Minimum 1 physical pixel

    // Wick width: 1 physical pixel (or 2 if candle body is very wide).
    let wick_width = if body_width > 20.0 {
        transforms.snap_to_pixel(2.0)
    } else {
        1.0 / transforms.dpi_scale  // Exactly 1 physical pixel
    };

    let mut instances = Vec::with_capacity(end_idx - start_idx);

    for i in start_idx..end_idx {
        let ts = data.timestamps[i] as f64;
        let o = data.opens[i];
        let h = data.highs[i];
        let l = data.lows[i];
        let c = data.closes[i];

        // Skip sentinel candles (no volume, all prices equal).
        if data.volumes[i] == 0 && o == h && h == l && l == c {
            continue;
        }

        let is_bullish = c >= o;

        // Center X of the candle (snapped to pixel).
        let center_x = transforms.time_to_x_snapped(
            ts + candle_duration_ms * 0.5,  // Center of time slot
        );

        // Body top and bottom (snapped).
        let body_top = transforms.price_to_y_snapped(
            if is_bullish { c as f64 } else { o as f64 }
        );
        let body_bottom = transforms.price_to_y_snapped(
            if is_bullish { o as f64 } else { c as f64 }
        );

        // Ensure body has at least 1 physical pixel height (doji candles).
        let body_bottom = if (body_bottom - body_top).abs() < 1.0 / transforms.dpi_scale {
            body_top + 1.0 / transforms.dpi_scale
        } else {
            body_bottom
        };

        // Wick top and bottom (snapped).
        let wick_top = transforms.price_to_y_snapped(h as f64);
        let wick_bottom = transforms.price_to_y_snapped(l as f64);

        let color = if is_bullish {
            theme.bull.to_array()
        } else {
            theme.bear.to_array()
        };

        instances.push(CandleInstance {
            x: center_x,
            body_top,
            body_bottom,
            wick_top,
            wick_bottom,
            width: body_width,
            color,
        });
    }

    instances
}
```

### Axis Label Positioning

Grid lines and axis labels use the transforms to determine placement:

```rust
/// Compute Y-axis (price) grid lines and labels for the current view.
pub fn compute_price_grid(
    transforms: &CoordinateTransforms,
    camera: &Camera2D,
) -> Vec<GridLine> {
    // Choose a "nice" step size for price grid lines.
    let price_range = camera.price_high - camera.price_low;
    let target_line_count = 8.0;  // Aim for ~8 grid lines in the visible area.
    let raw_step = price_range / target_line_count;
    let step = nice_step(raw_step);

    let first_price = (camera.price_low / step).ceil() * step;
    let mut lines = Vec::new();
    let mut price = first_price;

    while price <= camera.price_high {
        let y = transforms.price_to_y_snapped(price);
        lines.push(GridLine {
            position: y,
            label: format_price(price, step),
            is_major: (price / (step * 5.0)).fract().abs() < 0.01,
        });
        price += step;
    }

    lines
}

/// Round a step size to a "nice" number (1, 2, 5, 10, 20, 50, ...).
fn nice_step(raw: f64) -> f64 {
    let magnitude = 10.0_f64.powf(raw.log10().floor());
    let normalized = raw / magnitude;  // 1.0..10.0

    let nice = if normalized < 1.5 {
        1.0
    } else if normalized < 3.5 {
        2.0
    } else if normalized < 7.5 {
        5.0
    } else {
        10.0
    };

    nice * magnitude
}

/// Format a price for display, with appropriate decimal places.
fn format_price(price: f64, step: f64) -> String {
    let decimals = if step >= 1.0 {
        0
    } else if step >= 0.1 {
        1
    } else if step >= 0.01 {
        2
    } else if step >= 0.001 {
        3
    } else {
        4
    };
    format!("{:.decimals$}", price, decimals = decimals)
}

pub struct GridLine {
    pub position: f32,       // Screen Y (for price grid) or X (for time grid)
    pub label: String,       // Text to display on the axis
    pub is_major: bool,      // Major lines are brighter / thicker
}
```

### Summary of Transform Usage by System

| System | Forward (data->screen) | Inverse (screen->data) |
|---|---|---|
| Candle rendering | `time_to_x`, `price_to_y` | --- |
| Grid lines | `price_to_y`, `time_to_x` | --- |
| Crosshair snap | --- | `x_to_time` (to find candle), then `time_to_x` (to snap) |
| Crosshair display | `time_to_x` (vertical), raw Y (horizontal) | `y_to_price` (for label) |
| Pan interaction | --- | implicit via `time_per_pixel`, `price_per_pixel` |
| Zoom interaction | --- | `x_to_time` (for anchor point) |
| Level hit-test | `price_to_y` (for each level) | `y_to_price` (cursor to price) |
| Level drag | --- | `y_to_price` (new price from cursor) |
| Level render | `price_to_y` | --- |
| OHLCV tooltip | `time_to_x` (to find candle index) | --- |
| Projection matrix | Full viewport to NDC | --- |

---

## Appendix: Complete Event Processing Example

To illustrate how all systems work together, here is the complete flow for a **Ctrl+Wheel Zoom** event on a linked chart:

```
1. iced runtime delivers Event::Mouse(WheelScrolled { delta, position })
   with modifiers.ctrl = true.

2. chart_widget.rs::update()
   a. Converts position to widget-local ScreenPoint.
   b. Checks that cursor is within widget bounds.
   c. Creates ChartInputEvent::MouseWheel { pos, delta_x: 0, delta_y, modifiers }.

3. chart_state.rs::handle_input(event)
   a. Detects Ctrl modifier -> this is a zoom, not a pan scroll.
   b. Computes zoom_factor = wheel_delta_to_zoom_factor(delta_y).
   c. Returns ChartAction::Zoom { center_x: pos.x, factor: zoom_factor }.

4. app.rs::update(Message::ChartAction(chart_id, Zoom { center_x, factor }))
   a. Looks up ChartState for chart_id.
   b. Stops any active momentum (user interaction interrupts coast).
   c. Creates CoordinateTransforms from current camera + viewport.
   d. Calls camera.zoom_time_at(center_x, factor, &viewport, &limits).
   e. Calls compute_auto_scale_target() to get new Y target.
   f. Calls y_axis.set_target(target_low, target_high) -> starts Y animation.
   g. Calls dirty.mark_data() and dirty.mark_camera() (increments generation counters).
   h. If chart is linked (sync_group is Some):
      i.  Calls sync_controller.propagate(group, chart_id, time_start, time_end).
      ii. For each (target_id, ts, te) in result:
          - Updates target chart's camera time range.
          - Computes target chart's auto-scale target.
          - Sets target chart's Y animation target.
          - Calls target chart's dirty.mark_data() and dirty.mark_camera().

5. Next frame: iced calls view().
   a. Each dirty chart's ChartWidget::prepare() is called.
   b. Prepare rebuilds CandleInstance array from data + new camera.
   c. Uploads instance buffer to GPU.
   d. Uploads new projection matrix uniform.
   e. ChartWidget::render() draws: clear -> grid -> volume -> wicks -> bodies.
   f. Crosshair overlay drawn on top (if active).
   g. Axis labels drawn on top.

6. If Y-axis animation is active:
   a. any_animation_active() returns true.
   b. iced subscription fires AnimationTick messages at 60fps.
   c. Each tick: y_axis.tick() lerps camera price range toward target.
   d. dirty.mark_data() each tick (candle Y positions change).
   e. Animation converges and stops after ~15 frames (~250ms).

7. Total latency: input event -> pixel = 1 frame (16ms at 60fps).
   The zoom is applied immediately in step 4. Step 5 renders on the next frame.
   Y-axis animation runs over subsequent frames but the time-axis zoom is instant.
```

---

## Appendix: Constants Reference

All tunable constants in one place for easy adjustment during development:

```rust
pub mod interaction_constants {
    // --- Drag detection ---
    pub const DRAG_THRESHOLD_PX: f32 = 4.0;
    pub const DOUBLE_CLICK_TIME_MS: f64 = 400.0;
    pub const DOUBLE_CLICK_DISTANCE_PX: f32 = 6.0;

    // --- Level hit-testing ---
    pub const LEVEL_HIT_TOLERANCE_PX: f32 = 6.0;

    // --- Zoom ---
    pub const ZOOM_BASE_RATE: f64 = 0.12;
    pub const ZOOM_DELTA_CLAMP: f64 = 5.0;

    // --- Pan / Scroll ---
    pub const SCROLL_FRACTION: f32 = 0.08;
    pub const VELOCITY_WINDOW_MS: f64 = 150.0;
    pub const VELOCITY_SAMPLE_COUNT: usize = 8;

    // --- Momentum ---
    pub const MOMENTUM_FRICTION: f32 = 6.0;
    pub const MOMENTUM_MIN_VELOCITY: f32 = 5.0;

    // --- Boundaries ---
    pub const MAX_OVERSCROLL_FRACTION: f64 = 0.5;
    pub const ELASTIC_RESISTANCE: f64 = 0.7;

    // --- Y-axis ---
    pub const Y_AXIS_PADDING_FACTOR: f64 = 0.05;
    pub const Y_AXIS_ANIMATION_RATE: f64 = 8.0;
    pub const Y_AXIS_CONVERGENCE_THRESHOLD: f64 = 0.0001;

    // --- Outlier filtering ---
    pub const OUTLIER_RANGE_MULTIPLE: f64 = 10.0;

    // --- Data prefetch ---
    pub const PREFETCH_THRESHOLD_FRACTION: f64 = 0.25;

    // --- LOD ---
    pub const LOD_MIN_CANDLE_WIDTH_FOR_FULL: f32 = 6.0;
    pub const LOD_MIN_CANDLE_WIDTH_FOR_THIN: f32 = 3.0;
    pub const LOD_MIN_CANDLE_WIDTH_FOR_LINE: f32 = 1.5;

    // --- Candle rendering ---
    pub const CANDLE_BODY_WIDTH_FRACTION: f32 = 0.7;
    pub const CANDLE_MIN_BODY_HEIGHT_PHYSICAL_PX: f32 = 1.0;
    pub const CANDLE_THICK_WICK_THRESHOLD_PX: f32 = 20.0;

    // --- Grid ---
    pub const TARGET_PRICE_GRID_LINES: f64 = 8.0;
    pub const TARGET_TIME_GRID_LINES: f64 = 10.0;

    // --- Animation frame ---
    pub const MAX_DT_SECONDS: f32 = 0.1;
    pub const BOUNCE_BACK_RATE: f32 = 10.0;
    pub const BOUNCE_BACK_CONVERGENCE_MS: f64 = 0.5;

    // --- Viewport defaults ---
    pub const DEFAULT_MARGIN_RIGHT: f32 = 80.0;   // Y-axis width
    pub const DEFAULT_MARGIN_BOTTOM: f32 = 30.0;   // X-axis height
    pub const DEFAULT_MARGIN_TOP: f32 = 20.0;      // OHLCV header
    pub const DEFAULT_MARGIN_LEFT: f32 = 0.0;
}
```
