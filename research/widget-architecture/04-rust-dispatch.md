# Rust-Specific Trait and Dispatch Patterns

> Compiled from Rust widget patterns research, 2026-03-30

---

## 1. Enum vs Trait Object: The Core Trade-off

The fundamental question is whether the set of widget types is **closed** (known at compile time) or **open** (extensible by downstream code). This single distinction drives the entire dispatch architecture decision.

### Enum Dispatch (Closed World)

All variant types are defined in one place. Adding a new variant requires modifying the enum definition and every `match` arm.

### Trait Object Dispatch (Open World)

Any type implementing the trait can be stored in the collection. New types can be added by downstream crates without modifying the original code.

---

## 2. Performance Benchmarks

The `enum_dispatch` crate provides authoritative benchmarks. With 1024 mixed-type objects in a `Vec`, iterating and calling methods:

| Technique | Time (ns/iter) | Relative |
|---|---|---|
| `Vec<Box<dyn Trait>>` | 5,900,191 | 12.3x slower |
| Referenced trait objects | 5,658,461 | 11.8x slower |
| `enum_dispatch` enum | **479,630** | **baseline** |

### Why the 10-12x Difference

1. **Cache locality**: `Vec<MyEnum>` stores values contiguously (each = largest variant + tag byte). `Vec<Box<dyn Trait>>` stores fat pointers (16 bytes each) pointing to heap-scattered objects -- two levels of indirection.

2. **Vtable elimination**: Enum match compiles to a jump table or branch cascade. The compiler can inline variant methods. Trait objects require a vtable load + indirect jump, which blocks inlining.

3. **Branch prediction**: CPU branch predictors learn enum tag patterns. Vtable dispatch is essentially an unpredictable indirect call.

---

## 3. Decision Framework

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

---

## 4. The Hybrid Pattern: Enum Core + Trait Extension

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

---

## 5. The `enum_dispatch` Macro Pattern

If you adopt `enum_dispatch` to avoid manual `match` delegation:

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

---

## 6. Trait Design Patterns for Renderable Components

### Pattern A: The Compute Trait (Sans-IO Output)

A trait that takes chart state and produces render primitives. No GPU types involved.

```rust
pub trait Overlay {
    /// Unique type identifier for dirty-flag routing.
    fn overlay_type(&self) -> &'static str;

    /// Compute GPU-ready primitives from current chart state.
    fn compute(
        &self,
        camera: &Camera2D,
        viewport_width: u32,
        viewport_height: u32,
    ) -> OverlayOutput;

    /// Hit-test: does the point (px, py) intersect this overlay?
    fn hit_test(&self, px: f32, py: f32, camera: &Camera2D) -> Option<HitResult>;
}

pub struct OverlayOutput {
    pub lines: Vec<GridLineInstance>,
    pub fills: Vec<GridLineInstance>,
    pub markers: Vec<GridLineInstance>,
    pub labels: Vec<AxisLabel>,
}
```

**Object safety**: This trait IS object-safe. No generics, no `Self` in return position, no associated types. You can have `Vec<Box<dyn Overlay>>` if needed.

**Trade-off**: The output type is fixed (`OverlayOutput`). Every overlay must express itself as lines, fills, markers, and labels. This is a constraint, but a productive one -- it forces overlays to speak the renderer's language.

### Pattern B: Associated Types (Not Object-Safe)

```rust
pub trait TypedOverlay {
    type Instance: bytemuck::Pod + bytemuck::Zeroable;
    fn compute(&self, camera: &Camera2D) -> Vec<Self::Instance>;
}
```

**Problem**: NOT object-safe because of the associated type. Cannot create `Vec<Box<dyn TypedOverlay>>`.

**When to use**: Only when you have a single concrete overlay type per pipeline. For example, `CandlePipeline` knows it renders `CandleInstance`.

### Pattern C: Object Safety Workarounds

#### C1: Type Erasure via Erased Trait

The `erased-serde` pattern. Create a parallel "erased" trait that replaces generic parameters with trait objects:

```rust
// Original (not object-safe):
pub trait Serializable {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error>;
}

// Erased version (object-safe):
pub trait ErasedSerializable {
    fn erased_serialize(&self, serializer: &mut dyn ErasedSerializer) -> Result<(), Error>;
}

// Blanket implementation:
impl<T: Serializable> ErasedSerializable for T {
    fn erased_serialize(&self, s: &mut dyn ErasedSerializer) -> Result<(), Error> {
        self.serialize(s)
    }
}
```

**Verdict for charting**: Overkill. The `Overlay` trait from Pattern A is naturally object-safe.

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

#### C3: Return Byte Slices

```rust
pub trait Overlay {
    fn compute_bytes(&self, camera: &Camera2D) -> Vec<u8>;
    fn instance_size(&self) -> usize;
    fn instance_count(&self) -> usize;
}
```

**Verdict**: Stringly-typed / byte-soup approach. Loses type safety. Not recommended.

### Pattern D: The Visitor Pattern

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

**When the visitor wins**: Multiple operations over the same data (rendering, hit-testing, serialization). Each operation is a different visitor.

**When the visitor loses**: Adding a new overlay type requires updating every visitor. This is the "expression problem."

**Verdict**: With an enum, you get the same exhaustiveness checking via `match`. Prefer the enum unless you need cross-crate extensibility.

---

## 7. Sans-IO Boundary Pattern

### Definition

Sans-IO ("without I/O") separates pure computation from side effects:

- **Pure computation**: camera transforms, candle positioning, hit-testing, label formatting, price snapping, grid line computation
- **Side effects**: GPU buffer uploads, window management, file I/O, network, user input capture

### The Three-Layer Stack

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

### The Serialization Boundary

The key insight from iced's Shader widget pattern and Bevy's Extract phase: the **serialization boundary** is a plain data struct that crosses from logic to rendering.

In Midas, this is `ChartScene`. It is:
- **Owned** (not borrowed -- no lifetime entanglement with logic)
- **Framework-agnostic** (no iced or wgpu types)
- **GPU-friendly** (contains `bytemuck::Pod` instance types)
- **Diffable** (generation counters in `SceneGenerations` for dirty-flag optimization)

### Sans-IO Benefits

1. **Testability**: Tests run without GPU, window, or display. `compute_chart_scene()` can be unit-tested with mock data.
2. **Portability**: The chart core could be wrapped by egui, GPUI, or a terminal renderer.
3. **Determinism**: Given the same input, always the same output.
4. **Fuzz-ability**: The `ChartEvent -> ChartAction` state machine can be fuzzed with random event sequences.

---

## 8. The Render Primitive Pattern

### Core Concept

Overlays don't draw themselves. They produce a set of **render primitives** -- a common vocabulary of GPU-uploadable shapes -- that a centralized renderer consumes.

### Existing Primitives in Midas

```rust
GridLineInstance { rect: [f32; 4], color: [f32; 4] }   // 32 bytes, Pod
CandleInstance { x, body_top, body_bottom, ... }        // 48 bytes, Pod
VolumeInstance { x, y_top, y_bottom, width, color }     // 32 bytes, Pod
```

`GridLineInstance` is particularly versatile -- it renders axis-aligned filled rectangles covering:
- Grid lines (thin horizontal/vertical rects)
- Level lines (colored horizontal rects)
- Zone fills (wide, semi-transparent rects)
- Marker shapes (small squares)
- Volume Profile bars (horizontal histogram bars)
- Dashed lines (series of short rects)

### Three-Buffer Flattening

```rust
struct AnnotationBuffers {
    fills: Vec<GridLineInstance>,    // Layer 6: zone fills (behind lines)
    lines: Vec<GridLineInstance>,    // Layer 7: lines (on top of fills)
    markers: Vec<GridLineInstance>,  // Layer 8: markers (on top of lines)
}
```

Each buffer maps to a separate `GridPipeline` instance rendered at the correct z-order.

### What Charts Do NOT Need

- Bezier curves (no trendlines yet; when needed, tessellate to line segments)
- Arbitrary polygons (no polygon fills in charting)
- Images/textures (no pattern fills)
- 3D transforms (2D only)

Only create a new instance type when `GridLineInstance` genuinely cannot express the visual:
- Diagonal lines (trendlines -- need a `LineInstance` with endpoints)
- Textured markers (need an SDF marker pipeline)
- Curved elements (Fibonacci arcs -- tessellate to line segments first)

---

## 9. Lifecycle Pattern: Retained Data, Immediate Composition

Chart overlays follow a specific lifecycle:

```
Create -> Configure -> Compute -> Render -> Interact -> (Modify/Delete)
                         ^                     |
                         |_____________________|
                            (every frame)
```

The pattern is **retained data with immediate composition**:

- **Retained**: Annotations persist in `AnnotationStore` across frames. They have identity (`AnnotationId`) and mutable state.
- **Immediate**: Each frame, `compute_chart_scene()` recomputes ALL visible render primitives from scratch. No cached render state per annotation.

This works because annotation count is small (< 500). Computing `GridLineInstance` from an annotation is trivial (a few multiplications). The computation is cheaper than cache management overhead.

### Dirty Flag Exception

Candle and volume instances are expensive to rebuild (thousands of instances). The `DirtyFlags` generation counter pattern gates this:

```rust
if scene.generations.candles != tracker.candles {
    // Rebuild candle instances -- expensive, only when data/camera changed
    tracker.candles = scene.generations.candles;
}
// Annotations: always rebuild (cheap, < 500 instances)
```

---

## 10. GPU Resource Ownership Rule

Overlays MUST NOT hold GPU resources. The sans-IO boundary is sacred:

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
```

The `bytemuck::Pod + Zeroable` requirement is satisfied at the GPU instance level (`GridLineInstance`, `CandleInstance`), not at the overlay level. Overlays produce these instances; they don't own GPU buffers.

---

## 11. When to Use Each Approach

| Decision Point | Recommendation | Rationale |
|---|---|---|
| Overlay dispatch | Enum (`AnnotationKind`) | Closed set, fast iteration, exhaustive match |
| Serialization boundary | `ChartScene` struct | Framework-agnostic, typed, owned |
| Tool state | Struct fields on `ChartState` | Self-contained, testable, no trait overhead |
| Interaction architecture | Monolithic `handle_event()` for now | Under complexity threshold (<10 types) |
| Indicator interface | Free functions | Simple, testable, explicit |
| GPU primitives | Reuse `GridLineInstance` | Versatile, proven, no new shaders needed |
| State management | Retained data, immediate composition | Correct for current scale |
| Extensibility | Don't abstract prematurely | Add traits when variant count > 10 |
| ChartModifier pattern | Defer until > 10 interaction modes | Current monolithic approach is manageable |
| `enum_dispatch` macro | Optional convenience | Same performance as hand-written match |
