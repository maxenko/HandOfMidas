# Rust Design Patterns for Extensible Widget/Plugin Systems

> Research report for Hand of Midas chart component architecture
> Prepared 2026-03-30

---

## Table of Contents

1. [Trait Object vs Enum Dispatch](#1-trait-object-vs-enum-dispatch)
2. [Trait Design Patterns for Renderable Components](#2-trait-design-patterns-for-renderable-components)
3. [Sans-IO Patterns in Rust](#3-sans-io-patterns-in-rust)
4. [Real-World Rust Project Analysis](#4-real-world-rust-project-analysis)
5. [The Render Primitive Pattern](#5-the-render-primitive-pattern)
6. [Lifecycle Patterns](#6-lifecycle-patterns)
7. [Recommendations for Hand of Midas](#7-recommendations-for-hand-of-midas)

---

## 1. Trait Object vs Enum Dispatch

### The Core Trade-off: Open World vs Closed World

The fundamental question is whether the set of widget types is **closed** (known at compile time) or **open** (extensible by downstream code). This single distinction drives the entire dispatch architecture decision.

**Enum dispatch** (closed world): All variant types are defined in one place. Adding a new variant requires modifying the enum definition and every `match` arm.

**Trait object dispatch** (open world): Any type implementing the trait can be stored in the collection. New types can be added by downstream crates without modifying the original code.

### Performance Benchmarks

The `enum_dispatch` crate provides authoritative benchmarks. With 1024 mixed-type objects in a `Vec`, iterating and calling methods:

| Technique | Time (ns/iter) | Relative |
|---|---|---|
| `Vec<Box<dyn Trait>>` | 5,900,191 | 12.3x slower |
| Referenced trait objects | 5,658,461 | 11.8x slower |
| `enum_dispatch` enum | **479,630** | **baseline** |

**Why the 10-12x difference:**

1. **Cache locality**: `Vec<MyEnum>` stores values contiguously (each = largest variant + tag byte). `Vec<Box<dyn Trait>>` stores fat pointers (16 bytes each) pointing to heap-scattered objects -- two levels of indirection.

2. **Vtable elimination**: Enum match compiles to a jump table or branch cascade. The compiler can inline variant methods. Trait objects require a vtable load + indirect jump, which blocks inlining.

3. **Branch prediction**: CPU branch predictors learn enum tag patterns. Vtable dispatch is essentially an unpredictable indirect call.

### When Each Is Appropriate

```
Choose ENUM when:
  - The set of types is known and stable
  - You control all variants (same crate or workspace)
  - Tight iteration loops (rendering, hit-testing)
  - You want exhaustive match (compiler catches missing variants)
  - < 20 variants (beyond this, match arms get unwieldy)

Choose TRAIT OBJECT when:
  - Downstream crates must add new types
  - Plugin/extension architecture
  - > 20 types that share a thin interface
  - Types have very different sizes (enum wastes memory on padding)
  - You need type erasure for heterogeneous storage

Choose GENERICS (static dispatch) when:
  - Only one concrete type at a time (no heterogeneous collection)
  - Maximum performance (zero-cost abstraction)
  - Library API boundaries (callers pick the type)
```

### Hybrid Pattern: Enum Core + Trait Extension

The most practical pattern for chart overlays is a **closed enum for built-in types** with an escape hatch for extensions:

```rust
pub enum OverlayKind {
    Level(LevelOverlay),
    Crosshair(CrosshairOverlay),
    VolumeProfile(VolumeProfileOverlay),
    GerchikAtr(GerchikAtrOverlay),
    Bracket(BracketOverlay),
    Marker(MarkerOverlay),
    // Escape hatch for future extensions:
    Custom(Box<dyn CustomOverlay>),
}
```

This gives you enum dispatch performance for the 95% case (built-in types) and trait object flexibility for the 5% case (user-defined extensions). The `Custom` variant pays the vtable cost only when used.

### Relevance to Hand of Midas

Your current architecture already uses this implicitly. `ChartScene` has typed fields (`candles`, `volumes`, `grid_instances`, `levels`, `crosshair`, `volume_profile_instances`). Each is a concrete type, not a trait object. This is effectively enum dispatch -- the "enum" is the struct layout itself.

The planned `AnnotationKind` enum (`Level`, `Bracket`, `Note`, `Marker`) is the correct pattern for your use case: a closed, known set of annotation types that need fast iteration for hit-testing and rendering.

---

## 2. Trait Design Patterns for Renderable Components

### Pattern A: The Compute Trait (Sans-IO Output)

A trait that takes chart state and produces render primitives. No GPU types involved.

```rust
/// Trait for overlay components that compute their visual representation.
/// All types are framework-agnostic (no wgpu, no iced).
pub trait Overlay {
    /// Unique type identifier for dirty-flag routing.
    fn overlay_type(&self) -> &'static str;

    /// Compute GPU-ready primitives from current chart state.
    /// Returns instances that the renderer will upload to GPU buffers.
    fn compute(
        &self,
        camera: &Camera2D,
        viewport_width: u32,
        viewport_height: u32,
    ) -> OverlayOutput;

    /// Hit-test: does the point (px, py) intersect this overlay?
    fn hit_test(&self, px: f32, py: f32, camera: &Camera2D) -> Option<HitResult>;
}

/// Framework-agnostic output. The renderer consumes this.
pub struct OverlayOutput {
    pub lines: Vec<GridLineInstance>,
    pub fills: Vec<GridLineInstance>,
    pub markers: Vec<GridLineInstance>,
    pub labels: Vec<AxisLabel>,
}
```

**Object safety**: This trait IS object-safe. No generics, no `Self` in return position, no associated types. You can have `Vec<Box<dyn Overlay>>` if needed.

**Trade-off**: The output type is fixed (`OverlayOutput`). Every overlay must express itself as lines, fills, markers, and labels. This is a constraint, but a productive one -- it forces overlays to speak the renderer's language.

### Pattern B: Associated Types for Type-Safe GPU Data

When different overlays need different GPU instance types:

```rust
pub trait TypedOverlay {
    /// The GPU instance type this overlay produces.
    type Instance: bytemuck::Pod + bytemuck::Zeroable;

    /// Compute instances for the current frame.
    fn compute(&self, camera: &Camera2D) -> Vec<Self::Instance>;
}
```

**Problem**: This trait is NOT object-safe because of the associated type. You cannot create `Vec<Box<dyn TypedOverlay>>` because the compiler doesn't know the size of `Instance` at the call site.

**When to use**: Only when you have a single concrete overlay type per pipeline. For example, `CandlePipeline` knows it renders `CandleInstance`, `VolumePipeline` knows it renders `VolumeInstance`. The pipeline is generic over the instance type, but each pipeline slot holds exactly one type.

### Pattern C: Object Safety Workarounds

Three techniques for making non-object-safe traits usable with trait objects:

#### C1: Type Erasure via Erased Trait

The `erased-serde` pattern. Create a parallel "erased" trait that replaces generic parameters with trait objects:

```rust
// Original (not object-safe due to generic parameter):
pub trait Serializable {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error>;
}

// Erased version (object-safe):
pub trait ErasedSerializable {
    fn erased_serialize(&self, serializer: &mut dyn ErasedSerializer) -> Result<(), Error>;
}

// Blanket implementation -- anything that impls original also impls erased:
impl<T: Serializable> ErasedSerializable for T {
    fn erased_serialize(&self, s: &mut dyn ErasedSerializer) -> Result<(), Error> {
        // delegate to the original trait
        self.serialize(s)
    }
}
```

**Verdict for charting**: Overkill. Chart overlays don't need generic serialization. The `Overlay` trait from Pattern A is naturally object-safe.

#### C2: Split the Trait

Separate the object-safe parts from the non-object-safe parts:

```rust
// Object-safe: can be used as trait object
pub trait OverlayMeta {
    fn name(&self) -> &str;
    fn visible(&self) -> bool;
    fn hit_test(&self, px: f32, py: f32) -> bool;
}

// Not object-safe: used only with concrete types
pub trait OverlayCompute: OverlayMeta {
    type Instance: bytemuck::Pod;
    fn compute(&self, camera: &Camera2D) -> Vec<Self::Instance>;
}
```

Store `Vec<Box<dyn OverlayMeta>>` for iteration/hit-testing, and use concrete types for compute.

#### C3: Return Boxed Trait Objects Instead of Associated Types

```rust
pub trait Overlay {
    /// Returns opaque byte slice for GPU upload.
    fn compute_bytes(&self, camera: &Camera2D) -> Vec<u8>;
    fn instance_size(&self) -> usize;
    fn instance_count(&self) -> usize;
}
```

**Verdict**: Stringly-typed / byte-soup approach. Loses type safety. Not recommended.

### Pattern D: The Visitor Pattern for Heterogeneous Rendering

When you have a heterogeneous collection and need type-specific rendering logic without downcasting:

```rust
pub trait OverlayVisitor {
    fn visit_level(&mut self, level: &LevelOverlay);
    fn visit_crosshair(&mut self, crosshair: &CrosshairOverlay);
    fn visit_bracket(&mut self, bracket: &BracketOverlay);
    fn visit_marker(&mut self, marker: &MarkerOverlay);
}

pub trait OverlayAccept {
    fn accept(&self, visitor: &mut dyn OverlayVisitor);
}

// Renderer implements the visitor:
struct RenderVisitor<'a> {
    lines: &'a mut Vec<GridLineInstance>,
    fills: &'a mut Vec<GridLineInstance>,
    camera: &'a Camera2D,
}

impl OverlayVisitor for RenderVisitor<'_> {
    fn visit_level(&mut self, level: &LevelOverlay) {
        let y = self.camera.price_to_y(level.price);
        self.lines.push(GridLineInstance {
            rect: [0.0, y - 0.5, self.camera.viewport_width as f32, y + 0.5],
            color: level.color,
        });
    }
    // ... other visit methods
}
```

**When the visitor wins**: Multiple operations over the same data (rendering, hit-testing, serialization, validation). Each operation is a different visitor. Adding a new operation doesn't require changing the data types.

**When the visitor loses**: Adding a new overlay type requires updating every visitor. This is the "expression problem" -- enums make adding operations easy and types hard; trait objects make adding types easy and operations hard. The visitor doesn't solve this; it just shifts the problem.

**Verdict for charting**: The visitor pattern is valuable when you have 3+ operations (render, hit-test, serialize, export). But with an enum, you get the same exhaustiveness checking via `match`. Prefer the enum unless you need cross-crate extensibility.

### Handling Traits That Need GPU Resources

Overlays in your architecture MUST NOT hold GPU resources. The sans-IO boundary is sacred. Instead:

```rust
// WRONG: Overlay holds GPU buffer
pub struct LevelOverlay {
    price: f64,
    gpu_buffer: wgpu::Buffer,  // breaks sans-IO
}

// RIGHT: Overlay is pure data. Renderer manages GPU resources.
pub struct LevelOverlay {
    price: f64,
    color: [f32; 4],
    line_width: f32,
}

// Renderer maps overlay data to GPU resources:
impl ChartRenderer {
    fn update_level_buffer(&mut self, levels: &[LevelRender], device: &wgpu::Device, queue: &wgpu::Queue) {
        let instances: Vec<GridLineInstance> = levels.iter()
            .map(|l| /* compute GridLineInstance from LevelRender */)
            .collect();
        let bytes: &[u8] = bytemuck::cast_slice(&instances);
        queue.write_buffer(&self.level_buffer, 0, bytes);
    }
}
```

The `bytemuck::Pod + Zeroable` requirement is satisfied at the GPU instance level (`GridLineInstance`, `CandleInstance`), not at the overlay level. Overlays produce these instances; they don't own GPU buffers.

---

## 3. Sans-IO Patterns in Rust

### Definition

Sans-IO ("without I/O") separates pure computation from side effects. In charting:

- **Pure computation**: camera transforms, candle positioning, hit-testing, label formatting, price snapping, grid line computation
- **Side effects**: GPU buffer uploads, window management, file I/O, network, user input capture

### Your Current Architecture Already Implements This

`midas-chart` is a textbook sans-IO core:

```
Input:  ChartInput { camera, data, flags, ... }
           |
           v
Pure fn:  compute_chart_scene(&input)
           |
           v
Output: ChartScene { candles, volumes, grid, levels, crosshair, ... }
```

`ChartState` is a pure state machine. `handle_event()` takes `ChartEvent`, returns `Vec<ChartAction>`. No iced, no wgpu, no I/O. This is correct and should be preserved.

### The Three-Layer Sans-IO Stack

```
Layer 3: App Shell (midas-app)
  - iced integration
  - event translation: iced::Event -> ChartEvent
  - scene consumption: ChartScene -> Primitive -> GPU
  - persistence, config I/O

Layer 2: Renderer (midas-render)
  - wgpu pipelines, buffers, shaders
  - reads ChartScene, writes GPU buffers
  - manages pipeline lifecycle
  - ONLY layer that imports wgpu

Layer 1: Chart Core (midas-chart)
  - ChartState: pure state machine
  - compute_chart_scene(): pure function
  - interaction state machine: ChartEvent -> ChartAction
  - coordinate math, grid computation, hit-testing
  - ZERO framework dependencies
```

### Sans-IO Benefits Realized

1. **Testability**: 128+ tests run without GPU, window, or display. `compute_chart_scene()` can be unit-tested with mock data.

2. **Portability**: The chart core could be wrapped by egui, GPUI, or even a terminal renderer. Only layers 2 and 3 change.

3. **Determinism**: Given the same `ChartInput`, `compute_chart_scene()` always produces the same `ChartScene`. No hidden state, no timing dependencies.

4. **Fuzz-ability**: The `ChartEvent -> ChartAction` state machine can be fuzzed with random event sequences to find edge cases (impossible with GPU in the loop).

### The Serialization Boundary

The key insight from iced's Shader widget pattern and Bevy's Extract phase: the **serialization boundary** is a plain data struct that crosses from logic to rendering.

In your architecture, this is `ChartScene`. It is:
- Owned (not borrowed -- no lifetime entanglement with logic)
- Framework-agnostic (no iced or wgpu types)
- GPU-friendly (contains `bytemuck::Pod` instance types)
- Diffable (generation counters in `SceneGenerations` for dirty-flag optimization)

This is the correct design. Do not compromise it.

---

## 4. Real-World Rust Project Analysis

### 4.1 egui: Immediate-Mode Widget Trait

```rust
pub trait Widget {
    fn ui(self, ui: &mut Ui) -> Response;
}
```

**Key architectural decisions:**

- **Widgets are builders, not stateful objects.** A `Button` is created, configured via method chaining, then consumed by `ui()`. No persistent widget identity.
- **Closures are widgets.** `|ui: &mut Ui| -> Response` implements `Widget`. This enables ad-hoc composition without defining a struct.
- **State is external.** Widget state (e.g., is a collapsible section open?) lives in `egui::Memory`, keyed by `Id`. Widgets don't own their persistence.
- **No GPU abstraction.** egui tessellates everything to triangles on CPU. For charting at scale, this is prohibitively expensive (3-4 vertices per point with feathering). Rerun abandoned this for heavy visualization.

**Lesson for Midas**: The builder pattern for tool configuration is worth adopting. `LevelTool::new().with_snap(true).with_color(RED)` is more ergonomic than setting fields directly.

### 4.2 iced: Shader Widget + Program/Primitive/Pipeline

```rust
pub trait Program: Send + Sync + 'static {
    type State: Default + 'static;
    type Primitive: Primitive;

    fn draw(
        &self,
        state: &Self::State,
        cursor: mouse::Cursor,
        bounds: Rectangle,
    ) -> Self::Primitive;

    fn update(
        &self,
        state: &mut Self::State,
        event: Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> (event::Status, Option<Message>);
}

pub trait Primitive: Send + Sync + Debug + 'static {
    fn prepare(
        &self,
        device: &Device,
        queue: &Queue,
        format: TextureFormat,
        storage: &mut Storage,
        bounds: &Rectangle,
        viewport: &Viewport,
    );

    fn render(
        &self,
        encoder: &mut CommandEncoder,
        storage: &Storage,
        target: &TextureView,
        clip_bounds: &Rectangle<u32>,
    );
}
```

**Key architectural decisions:**

- **Three-trait separation**: `Program` (logic) -> `Primitive` (data snapshot) -> `Pipeline` (GPU resources). The Primitive is the serialization boundary.
- **Primitive is owned, not borrowed.** `draw()` returns `Self::Primitive` by value. The GPU thread owns a copy -- no lifetime entanglement.
- **Pipeline is a singleton.** Created once via `Pipeline::new()`, shared across all widget instances. GPU resources live here.
- **Storage is a type-map.** Pipelines store their buffers in `Storage` (keyed by TypeId), enabling multiple independent pipelines to coexist.

**Validation**: Kraken Desktop (crypto exchange) uses this exact pattern for production trading charts at scale.

**Lesson for Midas**: Your current `ChartScene` IS the Primitive concept. The `midas-render` crate IS the Pipeline concept. You've independently converged on the same architecture that iced formalizes.

### 4.3 Bevy: ECS + Plugin System

```rust
pub trait Plugin: Send + Sync + 'static {
    fn build(&self, app: &mut App);
}

// Function plugins (simpler):
fn my_plugin(app: &mut App) {
    app.add_systems(Update, my_system);
}

// Generic plugins (extensible):
pub struct PhysicsPlugin<T: PhysicsBackend> {
    _marker: PhantomData<T>,
}

impl<T: PhysicsBackend> Plugin for PhysicsPlugin<T> {
    fn build(&self, app: &mut App) {
        app.insert_resource(T::default())
           .add_systems(FixedUpdate, T::step_system);
    }
}
```

**The Extract pattern**: Bevy's rendering uses dual worlds. The main world holds game logic; the render world holds GPU data. Each frame, an Extract phase copies relevant data from main to render world.

```
Main World                    Render World
  Entity(Transform, Mesh)  --Extract-->  Entity(GpuMesh, GpuTransform)
                             --Prepare-->  BufferUpload
                             --Queue-->    DrawCommands
                             --Render-->   GPU dispatch
```

**Assessment for charting**: The ECS pattern is over-engineered for charts. Charts have 5-10 visual element types, not thousands of heterogeneous entities. The Extract pattern, however, maps perfectly to `compute_chart_scene() -> ChartScene -> GPU upload`.

**Worth borrowing**:
- `Plugin::build(&self, app: &mut App)` for registering chart overlays
- Extract phase concept (your `compute_chart_scene` is this)
- `PipelineCache` for lazy pipeline creation with deduplication

### 4.4 plotters: DrawingBackend Trait

```rust
pub trait DrawingBackend {
    type ErrorType: Error + Send + Sync;

    fn get_size(&self) -> (u32, u32);
    fn ensure_prepared(&mut self) -> Result<(), DrawingAreaErrorKind<Self::ErrorType>>;
    fn present(&mut self) -> Result<(), DrawingAreaErrorKind<Self::ErrorType>>;

    // Minimum requirement -- everything else has default impls:
    fn draw_pixel(
        &mut self,
        point: BackendCoord,
        color: BackendColor,
    ) -> Result<(), DrawingAreaErrorKind<Self::ErrorType>>;

    // Default impls that can be overridden for performance:
    fn draw_line<S: BackendStyle>(...) { /* pixel-by-pixel default */ }
    fn draw_rect<S: BackendStyle>(...) { /* line-by-line default */ }
    fn draw_circle<S: BackendStyle>(...) { /* Bresenham default */ }
    fn fill_polygon<S: BackendStyle>(...) { /* scanline default */ }
    fn draw_text<TStyle: BackendTextStyle>(...) { /* glyph rasterization */ }
}
```

**Key insight**: A minimal requirement (`draw_pixel`) with progressive override. Simple backends (bitmap) implement only `draw_pixel` and get everything else for free. GPU backends override `draw_rect`, `draw_line` etc. for hardware acceleration.

**Assessment for charting**: This is a render abstraction layer, which your renderer research correctly concluded is unnecessary for v1. Direct wgpu with `ChartScene` as the boundary is more performant. The plotters pattern is valuable if you ever need SVG/PDF export -- at that point, a display list consumed by both GPU and vector backends makes sense.

### 4.5 Slint: Declarative DSL + Compiler

Slint uses a custom `.slint` DSL compiled to Rust code. Widget extensibility is through composition in the DSL, not through Rust traits. Custom Rust widgets require implementing internal `Item` traits, which are not yet stable/public.

**Assessment**: Not directly applicable. Slint's approach (DSL + compiler) is the opposite of trait-based extensibility. However, their reactive property system (change propagation without manual dirty flags) is interesting for future consideration.

### 4.6 SciChart's ChartModifier Pattern (Non-Rust, But Architecturally Relevant)

From the open-source research, SciChart's composable interaction pattern is the gold standard:

```rust
// Rust translation of SciChart's ChartModifier concept:
pub trait ChartModifier: Send + Sync {
    fn on_event(&mut self, event: &ChartEvent, state: &ChartState) -> ModifierResult;
    fn priority(&self) -> u32 { 100 }
    fn is_active(&self) -> bool { true }
}

pub struct ModifierStack {
    modifiers: Vec<Box<dyn ChartModifier>>,
}

impl ModifierStack {
    pub fn process(&mut self, event: &ChartEvent, state: &ChartState) -> Vec<ChartAction> {
        let mut actions = Vec::new();
        for modifier in self.modifiers.iter_mut().filter(|m| m.is_active()) {
            match modifier.on_event(event, state) {
                ModifierResult::Handled(a) => { actions.extend(a); break; }
                ModifierResult::Continue(a) => { actions.extend(a); }
                ModifierResult::Ignored => {}
            }
        }
        actions
    }
}
```

This separates interaction behaviors (pan, zoom, crosshair, level drag) into composable units. Currently, your `handle_event()` is a monolithic function. This is fine for your current scope but worth refactoring when the interaction count exceeds 10.

---

## 5. The Render Primitive Pattern

### Core Concept

Overlays don't draw themselves. They produce a set of **render primitives** -- a common vocabulary of GPU-uploadable shapes -- that a centralized renderer consumes.

### Your Existing Primitives

```rust
// Already defined in midas-chart/src/instances.rs:
GridLineInstance { rect: [f32; 4], color: [f32; 4] }   // 32 bytes, Pod
CandleInstance { x, body_top, body_bottom, ... }        // 48 bytes, Pod
VolumeInstance { x, y_top, y_bottom, width, color }     // 32 bytes, Pod
```

These are your render primitives. `GridLineInstance` is particularly versatile -- it renders axis-aligned filled rectangles, which can represent:
- Grid lines (thin horizontal/vertical rects)
- Level lines (colored horizontal rects)
- Zone fills (wide, semi-transparent rects)
- Marker shapes (small squares)
- Volume Profile bars (horizontal histogram bars)
- Dashed lines (series of short rects)

### The Primitive Vocabulary

For a charting application, the complete primitive vocabulary is small:

```rust
/// Everything a chart can render, expressed as GPU primitives.
pub enum RenderPrimitive {
    /// Axis-aligned filled rectangle. Covers 90% of chart rendering.
    Rect(GridLineInstance),

    /// Candle body + wick. Specialized for candlestick rendering.
    Candle(CandleInstance),

    /// Volume bar. Specialized for volume rendering.
    Volume(VolumeInstance),

    /// Text label with position and styling. Rendered by iced overlay,
    /// not by GPU pipeline (text is always on top of GPU content).
    Label(AxisLabel),
}
```

In practice, you don't need this enum because each pipeline handles one primitive type. The "vocabulary" is implicit in the `ChartScene` struct fields.

### Why This Works for Charts

Charts have a small, stable set of visual primitives:
1. **Rectangles** (bodies, bars, fills, lines, backgrounds)
2. **Lines** (thin rectangles: grid, levels, crosshair)
3. **Text** (labels, badges, tooltips -- iced overlay, not GPU)
4. **Points/Markers** (small rects or future SDF shapes)

You do NOT need:
- Bezier curves (no trendlines yet; when needed, tessellate to line segments)
- Arbitrary polygons (no polygon fills in charting)
- Images/textures (no pattern fills)
- 3D transforms (2D only)

This bounded primitive set means every overlay can express itself as a combination of `GridLineInstance` rects + `AxisLabel` text. No extensible primitive system needed.

### Flattening for GPU Upload

The existing annotations plan correctly identifies the three-buffer flattening:

```rust
struct AnnotationBuffers {
    fills: Vec<GridLineInstance>,    // Layer 6: zone fills (behind lines)
    lines: Vec<GridLineInstance>,    // Layer 7: lines (on top of fills)
    markers: Vec<GridLineInstance>,  // Layer 8: markers (on top of lines)
}
```

Each buffer maps to a separate `GridPipeline` instance rendered at the correct z-order. This is the render primitive pattern applied: overlays produce primitives, the renderer sorts by layer, each layer gets its own GPU buffer.

---

## 6. Lifecycle Patterns

### Widget/Overlay Lifecycle in a Charting Context

Chart overlays have a lifecycle different from GUI widgets:

```
                          ┌─────────────────────────────────────┐
                          │         OVERLAY LIFECYCLE           │
                          │                                     │
   Create ──> Configure ──> Compute ──> Render ──> Interact    │
     │            │           │           │           │         │
     │            │           │           │           ├──> Drag │
     │            │           │           │           ├──> Edit │
     │            │           │           │           └──> Delete
     │            │           │           │                     │
     │            │           ├──> (every frame while visible)  │
     │            │           │                                 │
     │            ├──> (user changes settings)                  │
     │            │                                             │
     ├──> (user creates via tool or app logic)                  │
     │                                                          │
     └──────────────────────────────────────────────────────────┘
```

### Phase Details

**1. Creation**: User action (click, tool activation) or app logic (order fill event). Produces an `Annotation` struct stored in `AnnotationStore`.

```rust
// Current pattern (LevelTool):
ChartAction::CreateLevel { price } -> app inserts into LevelStore

// Planned pattern (AnnotationStore):
ChartAction::CreateAnnotation { kind } -> state.annotations.insert(annotation)
```

**2. Configuration**: User edits properties (color, label, style). Annotation struct is mutated in place.

```rust
impl AnnotationStore {
    pub fn update<F: FnOnce(&mut Annotation)>(&mut self, id: AnnotationId, f: F) -> bool {
        if let Some(ann) = self.get_mut(id) {
            f(ann);
            ann.modified_at = /* current time */;
            self.dirty = true;
            true
        } else {
            false
        }
    }
}
```

**3. Compute (per frame)**: `compute_chart_scene()` iterates visible annotations, transforms to screen coordinates, produces `AnnotationRender` variants.

```rust
fn compute_annotations(
    annotations: &AnnotationStore,
    camera: &Camera2D,
    viewport: (u32, u32),
) -> Vec<AnnotationRender> {
    annotations.visible()
        .filter(|a| a.is_in_view(camera))
        .map(|a| a.to_render(camera, viewport))
        .collect()
}
```

**4. Render**: Renderer reads `Vec<AnnotationRender>` from `ChartScene`, flattens to GPU buffers, issues draw calls. This happens entirely in `midas-render`.

**5. Interaction**: User clicks/drags an annotation. Hit-testing identifies the target. Interaction mode transitions handle the operation.

**6. Destruction**: User deletes via key press or context menu. Annotation removed from store, dirty flag incremented.

### Retained Data, Immediate Composition

The lifecycle pattern is **retained data with immediate composition**:

- **Retained**: Annotations persist in `AnnotationStore` across frames. They have identity (`AnnotationId`) and mutable state.
- **Immediate**: Each frame, `compute_chart_scene()` recomputes ALL visible render primitives from scratch. No cached render state per annotation.

This is the same pattern GPUI uses: entities persist, but `RenderOnce` components rebuild their visual representation every frame.

**Why this works**: Annotation count is small (< 500). Computing `GridLineInstance` from an annotation is trivial (a few multiplications). There is no benefit to caching per-annotation render state -- the computation is cheaper than the cache management overhead.

### Dirty Flag Optimization

The one exception to "compute everything every frame": candle and volume instances. These are expensive to rebuild (thousands of instances) and only change when the camera moves or data updates. Your `DirtyFlags` generation counter pattern correctly gates this:

```rust
if scene.generations.candles != tracker.candles {
    // Rebuild candle instances -- expensive, only when data/camera changed
    tracker.candles = scene.generations.candles;
}
// Annotations: always rebuild (cheap, < 500 instances)
```

---

## 7. Recommendations for Hand of Midas

### 7.1 Dispatch Strategy: Enum, Not Trait Objects

**Recommendation: Use `AnnotationKind` enum (already planned) for all chart overlays.**

Rationale:
- Your overlay set is closed and small (Level, Bracket, Note, Marker, + maybe 3-4 more).
- Hit-testing and rendering iterate all overlays every frame -- cache locality matters.
- Exhaustive `match` catches missing variants at compile time.
- No downstream extensibility needed (this is a proprietary trading app, not a library).

The annotations plan's `AnnotationKind` enum is correct. Do not introduce `Box<dyn Overlay>` unless you later need a plugin system for user-defined indicators (v3+ concern).

### 7.2 Keep the ChartScene as Serialization Boundary

**Recommendation: Expand `ChartScene` with an `annotations` field, not a trait-object-based overlay registry.**

```rust
pub struct ChartScene {
    // ... existing fields ...

    /// Annotation render data (levels, brackets, markers, notes).
    pub annotations: Vec<AnnotationRender>,

    /// Gerchik ATR overlay (always-on indicator).
    pub gerchik_atr: Option<GerchikAtrRender>,
}
```

This is explicit, typed, and zero-cost. Each overlay type gets its own field or contributes to the `annotations` vec. The renderer knows exactly what to expect.

### 7.3 Tool Architecture: Self-Contained Structs on ChartState

**Recommendation: Continue the `CrosshairTool` / `LevelTool` pattern for new tools.**

Your existing pattern is excellent:

```rust
pub struct ChartState {
    pub crosshair: CrosshairTool,   // self-contained state machine
    pub level_tool: LevelTool,      // self-contained state machine
    // Future:
    pub bracket_tool: BracketTool,  // same pattern
    pub marker_tool: MarkerTool,    // same pattern
}
```

Each tool:
- Owns its internal state machine (mode enum)
- Exposes `is_active()`, `is_placing()`, `is_dragging()` predicates
- Has `cancel()` for reset
- Has `suspend_placing()` / `try_resume_placing()` for interruption handling

This is the SciChart `ChartModifier` pattern expressed as Rust structs rather than trait objects. It works because the tool set is small and known.

### 7.4 Interaction: Keep Monolithic handle_event(), Extract When > 10 Interactions

**Recommendation: Do not refactor to composable modifiers yet.**

Your current `handle_event()` function handles ~10 interaction types. This is at the threshold where a monolithic function is still manageable. The function is well-organized with clear state machine transitions.

Refactor to composable modifiers when:
- The function exceeds 1000 lines (currently ~500)
- You add > 3 more interaction modes
- You need to dynamically enable/disable interaction behaviors

### 7.5 Indicator Architecture: Free Functions, Not Traits

**Recommendation: Keep indicators as free functions returning render data.**

Your `compute_gerchik_atr()` pattern is superior to a trait-based indicator system for your current scope:

```rust
// Current (correct):
pub fn compute_gerchik_atr(data: &dyn CandleData, candle_duration_ms: f64) -> Option<GerchikAtrRender>

// Called from compute_chart_scene():
let gerchik_atr = compute_gerchik_atr(input.data, candle_duration);
```

Benefits:
- Pure function, trivially testable
- No state management overhead
- No trait machinery
- Explicit in compute_chart_scene() -- you can see exactly what indicators are computed

When to introduce an `Indicator` trait:
- When you have > 10 indicators with a common interface
- When users need to configure which indicators are active at runtime
- When indicator computation needs to be parallelized (rayon over `Vec<Box<dyn Indicator>>`)

At that point, the trait would look like:

```rust
pub trait Indicator: Send + Sync {
    fn name(&self) -> &str;
    fn compute(&self, data: &dyn CandleData, camera: &Camera2D) -> IndicatorOutput;
    fn is_overlay(&self) -> bool; // true = draws on price chart, false = separate panel
}

pub enum IndicatorOutput {
    TextBadge(GerchikAtrRender),
    LineSeries(Vec<(f32, f32)>),
    Histogram(Vec<GridLineInstance>),
    // ... future variants
}
```

### 7.6 GPU Instance Types: Keep Existing, Add Sparingly

**Recommendation: Resist creating new GPU instance types. Express new overlays as `GridLineInstance` combinations.**

Your `GridLineInstance` (32-byte axis-aligned colored rect) handles:
- Grid lines
- Level lines
- Volume Profile bars
- Crosshair lines
- Bracket legs and zone fills
- Marker approximations (stacked scanlines for circles)
- Note backgrounds

Only create a new instance type when `GridLineInstance` genuinely cannot express the visual:
- Diagonal lines (trendlines -- need a `LineInstance` with endpoints)
- Textured markers (need an SDF marker pipeline)
- Curved elements (Fibonacci arcs -- tessellate to line segments first)

### 7.7 Summary Decision Table

| Decision Point | Recommendation | Rationale |
|---|---|---|
| Overlay dispatch | Enum (`AnnotationKind`) | Closed set, fast iteration, exhaustive match |
| Serialization boundary | `ChartScene` struct | Already working, framework-agnostic, typed |
| Tool state | Struct fields on `ChartState` | Self-contained, testable, no trait overhead |
| Interaction architecture | Monolithic `handle_event()` | Under complexity threshold, refactor later |
| Indicator interface | Free functions | Simple, testable, explicit |
| GPU primitives | Reuse `GridLineInstance` | Versatile, proven, no new shaders |
| State management | Retained data, immediate composition | Matches existing pattern, correct for scale |
| Extensibility strategy | Don't abstract prematurely | Add traits when variant count > 10 |

---

## Appendix: Code Pattern Reference

### A. The enum_dispatch Macro

If you adopt `enum_dispatch` for the `AnnotationKind` enum to avoid manual `match` delegation:

```rust
use enum_dispatch::enum_dispatch;

#[enum_dispatch]
pub trait AnnotationOps {
    fn anchor(&self) -> Anchor;
    fn hit_test(&self, px: f32, py: f32, camera: &Camera2D) -> Option<HitZone>;
    fn to_render(&self, camera: &Camera2D, viewport: (u32, u32)) -> AnnotationRender;
}

#[enum_dispatch(AnnotationOps)]
pub enum AnnotationKind {
    Level(LevelAnnotation),
    Bracket(OrderBracket),
    Note(TextNote),
    Marker(MarkerAnnotation),
}
```

This auto-generates the `match` arms, so calling `kind.hit_test(...)` dispatches to the correct variant without runtime vtable overhead.

**Trade-off**: Adds a proc macro dependency. The generated code is equivalent to what you'd write by hand. Worth it only if you frequently add/remove variants.

### B. The "Primitive Output" Trait (If You Later Need It)

```rust
/// Trait for components that produce GPU-ready render primitives.
/// Object-safe: can be used as `Box<dyn PrimitiveSource>`.
pub trait PrimitiveSource {
    /// Compute render primitives for the current camera view.
    fn emit_primitives(
        &self,
        camera: &Camera2D,
        viewport_width: u32,
        viewport_height: u32,
        out: &mut PrimitiveCollector,
    );
}

/// Accumulator for render primitives, sorted by layer.
pub struct PrimitiveCollector {
    pub fills: Vec<GridLineInstance>,
    pub lines: Vec<GridLineInstance>,
    pub markers: Vec<GridLineInstance>,
    pub labels: Vec<AxisLabel>,
}

impl PrimitiveCollector {
    pub fn add_line(&mut self, rect: [f32; 4], color: [f32; 4]) {
        self.lines.push(GridLineInstance { rect, color });
    }
    pub fn add_fill(&mut self, rect: [f32; 4], color: [f32; 4]) {
        self.fills.push(GridLineInstance { rect, color });
    }
    // ...
}
```

This is the "render primitive" pattern formalized as a trait. Use it when you have 10+ overlay types with a common output interface. Until then, direct struct methods and free functions are simpler.

---

## Sources

### Web Research
- [Rust Dispatch Explained: When Enums Beat dyn Trait](https://www.somethingsblog.com/2025/04/20/rust-dispatch-explained-when-enums-beat-dyn-trait/)
- [Enum or Trait Object - Possible Rust](https://www.possiblerust.com/guide/enum-or-trait-object)
- [3 Things to Try When You Can't Make a Trait Object - Possible Rust](https://www.possiblerust.com/pattern/3-things-to-try-when-you-can-t-make-a-trait-object)
- [enum_dispatch crate](https://docs.rs/enum_dispatch/latest/enum_dispatch/)
- [Three Kinds of Polymorphism in Rust](https://www.brandons.me/blog/polymorphism-in-rust)
- [Polymorphism in Rust: Enums vs Traits](https://www.mattkennedy.io/blog/rust_polymorphism/)
- [Performance implications of Box<Trait> vs enum delegation](https://users.rust-lang.org/t/performance-implications-of-box-trait-vs-enum-delegation/11957)
- [Item 12: Understand the trade-offs between generics and trait objects - Effective Rust](https://www.lurklurk.org/effective-rust/generics.html)
- [Type-erasing trait parameters in Rust](https://freyja.dev/posts/rust-erased-trait-parameters/)
- [Exploring Traits with Erased serde](https://www.thecodedmessage.com/posts/erased-serde/)
- [erased-serde crate](https://github.com/dtolnay/erased-serde)
- [Visitor - Rust Design Patterns](https://rust-unofficial.github.io/patterns/patterns/behavioural/visitor.html)
- [egui Widget trait](https://docs.rs/egui/latest/egui/widgets/trait.Widget.html)
- [iced Shader widget](https://docs.rs/iced/latest/iced/widget/struct.Shader.html)
- [iced Program trait](https://docs.iced.rs/iced/widget/shader/trait.Program.html)
- [Bevy Plugin trait](https://docs.rs/bevy_app/latest/bevy_app/trait.Plugin.html)
- [plotters DrawingBackend](https://docs.rs/plotters-backend/latest/plotters_backend/trait.DrawingBackend.html)
- [Slint UI toolkit](https://github.com/slint-ui/slint)
- [GPUI Component](https://www.blog.brightcoding.dev/2026/02/23/gpui-component-build-stunning-rust-desktop-apps-with-gpu-power)
- [sansio crate](https://docs.rs/sansio)
- [Heterogeneous Collections in Rust](https://elitedev.in/rust/heterogeneous-collections-in-rust-working-with-th/)

### Codebase Analysis
- `desktop/win/crates/midas-chart/src/` -- current sans-IO chart core
- `desktop/win/plan/annotations/` -- planned annotation architecture
- `desktop/win/plan/chart-architecture-research-rust-patterns.md` -- prior research
- `desktop/win/plan/chart-architecture-research-opensource.md` -- prior open-source survey
- `desktop/win/plan/chart-architecture-research-renderers.md` -- prior renderer research
