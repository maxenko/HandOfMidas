# Game Engine and GUI Framework Patterns: Bevy ECS, egui, iced

> Compiled from Rust component architecture and renderer research, 2026-03-30

---

## 1. Bevy ECS: Three-Tier Visibility System

### Dual-World Architecture

Bevy splits computation into two worlds:

- **Main World** -- application logic, game state, user input
- **Render World** -- GPU data, buffers, draw commands

Each frame, an **Extract** phase copies relevant data from the Main World to the Render World. This is a serialization boundary identical in concept to iced's Primitive pattern and Midas's `ChartScene`.

```
Main World                    Render World
  Entity(Transform, Mesh)  --Extract-->  Entity(GpuMesh, GpuTransform)
                             --Prepare-->  BufferUpload
                             --Queue-->    DrawCommands
                             --Render-->   GPU dispatch
```

### Three-Tier Visibility

Bevy implements visibility with three distinct levels:

1. **`Visibility`** (user intent) -- an enum with `Inherited`, `Visible`, `Hidden`. This is what the application sets. `Inherited` means "use my parent's visibility."

2. **`InheritedVisibility`** (computed propagation) -- a boolean computed by propagating `Visibility` down the entity hierarchy. If a parent is hidden, all children inherit hidden regardless of their own setting.

3. **`ViewVisibility`** (render decision) -- the final boolean that the renderer checks. Set by visibility systems after considering:
   - `InheritedVisibility` (is the entity or any ancestor hidden?)
   - Frustum culling (is the entity in the camera's view?)
   - RenderLayer membership (does the entity's layer match the camera's layer mask?)

This three-tier system separates **intent** (what the user wants), **propagation** (hierarchical computation), and **decision** (what actually renders). For charting, this maps to:

- Intent: user toggles an overlay visible/hidden
- Propagation: N/A for charts (flat hierarchy)
- Decision: is the overlay in the viewport? Does the current timeframe match the overlay's visibility filter?

### RenderLayers Bitmask

Bevy uses a `RenderLayers` component that is a **bitmask** (up to 32 layers by default, extendable). Each camera has its own `RenderLayers` mask. An entity renders only if its layers and the camera's layers share at least one set bit.

```rust
// Entity visible on layers 0 and 2:
RenderLayers::layer(0).with(2)

// Camera renders layers 0 and 1:
camera.render_layers = RenderLayers::layer(0).with(1)

// Entity renders because layer 0 is shared.
```

**Relevance to charting**: This bitmask pattern could map to overlay categories:

```rust
const LAYER_GRID: u32     = 1 << 0;
const LAYER_SERIES: u32   = 1 << 1;
const LAYER_OVERLAYS: u32 = 1 << 2;
const LAYER_AXES: u32     = 1 << 3;
const LAYER_CROSSHAIR: u32 = 1 << 4;
```

Each pipeline checks its layer bit against the chart's active layer mask. Toggling "show grid" flips a bit rather than rebuilding data.

### Plugin Pattern

Bevy's `Plugin` trait provides a registration mechanism:

```rust
pub trait Plugin: Send + Sync + 'static {
    fn build(&self, app: &mut App);
}

// Function plugins (simpler):
fn my_plugin(app: &mut App) {
    app.add_systems(Update, my_system);
}
```

**Assessment for charting**: The ECS pattern is over-engineered for charts. Charts have 5-10 visual element types, not thousands of heterogeneous entities. The render graph solves problems (transparent sorting, incremental invalidation, thousands of draw calls) that charts don't have.

**Worth borrowing**:
- The Extract pattern (snapshot state into GPU-friendly format)
- PipelineCache (lazy pipeline creation with deduplication)
- `Plugin::build(&self, app: &mut App)` for registering chart components

---

## 2. Unity CanvasGroup: Opacity Propagation

### The Pattern

Unity's `CanvasGroup` component provides group-level control over opacity, interactability, and raycast blocking for all child UI elements:

- Setting `CanvasGroup.alpha = 0.5` makes all children render at 50% opacity
- Setting `CanvasGroup.interactable = false` disables input for all children
- Setting `CanvasGroup.blocksRaycasts = false` makes all children click-through

The key insight is **multiplicative propagation**: a child's effective alpha = its own alpha * parent CanvasGroup alpha * grandparent CanvasGroup alpha. This cascade is computed once per frame and applied during rendering.

### Relevance to Charting: The Presence Enum

This propagation model suggests a three-state presence system for chart overlays:

```rust
pub enum Presence {
    /// Fully active: rendered, interactive, part of hit-testing
    Active,
    /// Visible but non-interactive: rendered at reduced opacity,
    /// excluded from hit-testing. Used for ghost previews,
    /// locked annotations, cross-chart sync display.
    Ghost,
    /// Completely hidden: not rendered, not interactive,
    /// no GPU cost. Still stored in data model.
    Hidden,
}
```

**Active** is the normal state. **Ghost** is like Unity's CanvasGroup with `interactable = false` and reduced alpha -- the overlay is visible but cannot be selected, dragged, or edited. This is ideal for:
- Showing synced drawings from other charts (visible but not editable on this chart)
- Preview mode when a tool is active (showing where an annotation would be placed)
- Locked annotations that should be visible but not accidentally modified

**Hidden** removes the overlay from rendering entirely, like Bevy's `Visibility::Hidden`.

---

## 3. egui: Immediate-Mode Widget Trait and Layer Ordering

### Widget Trait

```rust
pub trait Widget {
    fn ui(self, ui: &mut Ui) -> Response;
}
```

Key architectural decisions:

- **Widgets are builders, not stateful objects.** A `Button` is created, configured via method chaining, then consumed by `ui()`. No persistent widget identity.
- **Closures are widgets.** `|ui: &mut Ui| -> Response` implements `Widget`. Ad-hoc composition without defining a struct.
- **State is external.** Widget state lives in `egui::Memory`, keyed by `Id`. Widgets don't own their persistence.

### Layer Ordering

egui uses a `LayerId` system combining an `Order` enum with an `Id`:

```rust
pub struct LayerId {
    pub order: Order,
    pub id: Id,
}

pub enum Order {
    Background,
    PanelResizeLine,
    Middle,
    Foreground,
    Tooltip,
    Debug,
}
```

Layers within the same `Order` are rendered in creation order. The `Order` enum provides coarse z-ordering; the `Id` provides fine-grained ordering within a level.

### egui_plot: The Cautionary Tale

egui_plot tessellates ALL geometry on CPU every frame (3-4 vertices per point for feathering), then uploads. This works for small datasets but breaks at 1M+ points. The fix requires breaking immediate-mode purity by caching tessellated geometry -- exactly the retained-resource hybrid that Midas already implements.

### Rerun's Split Renderer Approach

Rerun (whose CTO Emilk created egui) separates:
- **egui** for 2D UI controls (buttons, panels, text)
- **re_renderer** (custom wgpu) for heavy visualization

Custom visualizers have their own archetypes, visualizer systems, and optional GPU renderers.

**Lesson**: For serious visualization performance, use your own renderer alongside the UI framework, not inside it. This validates Midas's architecture of custom wgpu pipelines inside the iced shell.

---

## 4. iced: Shader Widget Pattern

### Three-Trait Architecture

iced 0.14 provides a `Shader<Message>` widget with three traits that form the serialization boundary:

**Program** -- Orchestrator. Owns semantic logic.
```rust
pub trait Program: Send + Sync + 'static {
    type State: Default + 'static;
    type Primitive: Primitive;

    fn draw(&self, state: &State, cursor: Cursor, bounds: Rectangle) -> Self::Primitive;
    fn update(&self, state: &mut State, event: Event, bounds: Rectangle,
              cursor: Cursor) -> (Status, Option<Message>);
}
```

**Primitive** -- Per-frame data envelope (the serialization boundary).
```rust
pub trait Primitive: Send + Sync + Debug + 'static {
    fn prepare(&self, device: &Device, queue: &Queue, format: TextureFormat,
               storage: &mut Storage, bounds: &Rectangle, viewport: &Viewport);
    fn render(&self, encoder: &mut CommandEncoder, storage: &Storage,
              target: &TextureView, clip_bounds: &Rectangle<u32>);
}
```

**Pipeline** -- Constructed once. Shared across all widget instances.
```rust
pub trait Pipeline {
    fn new(device: &Device, queue: &Queue, format: TextureFormat) -> Self;
}
```

### Data Flow

Strictly push-based, unidirectional (Elm architecture):

1. App state -> `view()` -> `Shader` widget
2. `Program::draw()` -> `Primitive` (owned snapshot)
3. `Primitive::prepare()` -> GPU upload
4. `Primitive::draw()` -> draw calls
5. Events -> `Program::update()` -> `Message` back to app

**Key insight**: The `Primitive` is the serialization boundary. GPU code never borrows application data -- it receives an owned snapshot. This eliminates lifetime entanglement between logic and rendering.

### Storage as Type-Map

Pipelines store their buffers in `Storage` (keyed by `TypeId`), enabling multiple independent pipelines to coexist without knowing about each other.

### Production Validation

Kraken Desktop (crypto exchange) is built entirely on iced and uses this exact pattern for production trading charts. This validates the Shader widget as production-viable for high-performance financial charting.

---

## 5. Makepad: Draw/Handle Split

### Two-Pass Architecture

Every Makepad widget has exactly two functions:

```rust
fn draw_walk(&mut self, cx: &mut Cx2d, scope, walk) -> DrawStep
fn handle_event(&mut self, cx: &mut Cx, event, scope)
```

Events flow through `handle_event()` first, then drawing in `draw_walk()` -- two separate passes per frame.

### GPU Details

- Retained-mode render with hierarchical draw passes
- Instanced-array draw calls
- Vertex-shader clipping (no stencil)
- Almost entirely matrix-free
- Custom shader DSL

---

## 6. GPUI (Zed): Three-Phase Rendering

### Layout -> Prepaint -> Paint

1. **Layout**: Constraints flow down, sizes flow up (Taffy/Flexbox)
2. **Prepaint**: State snapshot
3. **Paint**: Elements add primitives to Scene

The Scene is a flat, sorted, batchable list -- not a scene graph. Primitives sorted by type for GPU batching. Rendered in fixed order: shadows -> quads -> paths -> underlines -> sprites.

### Shader-Per-Primitive Philosophy

One custom shader per primitive type. Rounded rectangles with borders and shadows are a single shader computing SDF per pixel. Same philosophy as Midas's per-pipeline approach (CandlePipeline, GridPipeline, etc.).

### Stateless Components (Longbridge)

The `gpui-component` crate provides 60+ widgets including charts as **stateless `RenderOnce` components** -- no hidden mutable state. Caller provides all data through properties.

---

## 7. Key Takeaways for Hand of Midas

### From Bevy
1. **Three-tier visibility** (intent -> propagation -> decision) informs the `Presence` enum: `Active`, `Ghost`, `Hidden`.
2. **RenderLayers bitmask** is efficient for toggling overlay categories.
3. **Extract pattern** is conceptually identical to `compute_chart_scene()` -> `ChartScene` -> GPU upload. Already implemented correctly.

### From egui
4. **Builder pattern** for tool configuration: `LevelTool::new().with_snap(true).with_color(RED)`.
5. **Split renderer** (Rerun pattern): own the GPU pipeline, let the UI framework handle controls. Already the Midas architecture.

### From iced
6. **Primitive as serialization boundary** matches `ChartScene`. The architecture has independently converged on the same pattern that iced formalizes.
7. **Push-based data flow** with owned snapshots avoids lifetime complexity.

### From GPUI
8. **Flat sorted scene** (not a scene graph) is the right model for charts. Fixed render order, batch by primitive type.
9. **Shader-per-primitive** matches the per-pipeline approach.

### From Unity
10. **Multiplicative opacity propagation** informs how Ghost-state overlays should render: multiply the overlay's color alpha by a ghost factor (e.g., 0.4).
