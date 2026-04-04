# GPU Rendering Architecture — Midas Chart Engine

> The rendering bible for Hand of Midas. Covers every GPU pipeline, shader, buffer layout,
> coordinate transform, and draw call needed to render TC2000-level pixel-perfect multi-chart
> stock charts using wgpu inside iced's Shader widget.
>
> Authored 2026-03-24. Updated 2026-03-25. Target: wgpu 27, iced 0.14, Windows 11, DX12/Vulkan backend.

---

## Table of Contents

1. [Per-Chart Render Architecture](#1-per-chart-render-architecture)
   - [1.6 ChartScene — Framework-Agnostic IR](#16-chartscene--framework-agnostic-intermediate-representation)
2. [Candlestick Pipeline](#2-candlestick-pipeline)
3. [Volume Bar Pipeline](#3-volume-bar-pipeline)
4. [Grid Line Pipeline](#4-grid-line-pipeline)
5. [Axis Label Rendering](#5-axis-label-rendering)
6. [Camera / Coordinate System](#6-camera--coordinate-system)
7. [GPU Resource Management](#7-gpu-resource-management)
8. [Render Order](#8-render-order)
9. [Performance Targets](#9-performance-targets)
10. [Color System](#10-color-system)

---

## 1. Per-Chart Render Architecture

### 1.1 iced Shader Widget Integration Model

Each chart panel is an `iced::widget::shader::Shader<Message>` widget. iced gives us:

- A `wgpu::RenderPass` scoped to the widget's clip rectangle within the window surface
- Access to the shared `wgpu::Device` and `wgpu::Queue`
- The target `wgpu::TextureFormat` (typically `Bgra8UnormSrgb` on Windows)
- A `Viewport` with physical size and scale factor

The iced 0.14 Shader widget trait has these key methods:

```rust
pub trait Program<Message> {
    /// Persistent GPU state — created once, shared across frames
    type State;

    /// Per-frame data prepared on the CPU, passed to prepare()/draw()
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
        shell: &mut Shell<'_, Message>,
    ) -> (event::Status, Option<Message>);

    fn mouse_interaction(
        &self,
        state: &Self::State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction;
}

/// iced 0.14 Primitive trait — uses associated Pipeline type instead of Storage.
/// The Pipeline is created once via Pipeline::new() and shared across all
/// widget instances that use the same Primitive type.
pub trait Primitive: Send + Debug + 'static {
    /// The GPU pipeline state. Created once and reused for all widgets
    /// of this Primitive type. This is where render pipelines, shared
    /// buffers, and per-chart GPU resources live.
    type Pipeline: Pipeline;

    fn prepare(
        &self,
        pipeline: &mut Self::Pipeline,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bounds: &Rectangle,
        viewport: &Viewport,
    );

    fn render(
        &self,
        pipeline: &Self::Pipeline,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        clip_bounds: &Rectangle<u32>,
    );

    /// New in iced 0.14: called with a live RenderPass for direct draw commands.
    /// Return true if draw calls were issued.
    fn draw(
        &self,
        pipeline: &Self::Pipeline,
        render_pass: &mut wgpu::RenderPass<'_>,
    ) -> bool;
}

/// iced 0.14 Pipeline trait — constructed once with GPU device access.
pub trait Pipeline {
    fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
    ) -> Self;
}
```

**Key change from iced 0.13**: The old `Storage` type-keyed map is gone. Instead,
each `Primitive` declares an associated `Pipeline` type that iced constructs once
via `Pipeline::new(device, queue, format)`. Pipeline instances are shared across all
widget instances of the same Primitive type. This is exactly what we need: our
`SharedPipelines` (render pipelines, unit quad VBO, font atlas) become the `Pipeline`
associated type, with per-chart state stored inside it in a `HashMap<ChartId, ChartGpuResources>`.

### 1.2 Direct Rendering vs Render-to-Texture

**Decision: Direct rendering into the iced surface — no intermediate FBO.**

Rationale:
- iced's Shader widget already provides a render pass targeting the window surface
- An FBO adds a full-screen texture copy per chart per frame (20 charts = 20 extra blits)
- The DX12/Vulkan scissor rect from iced's clip bounds handles chart isolation
- We get native-resolution rendering for free — no texture size management

When to add FBO (future optimization only):
- If we implement chart thumbnail caching (minimized charts render to 256x128 FBO once)
- If we need post-processing effects (glow on crosshair, blur on inactive charts)
- If dirty-flagging shows that caching to texture saves significant GPU time

### 1.3 Dirty Flagging (Generation Counters)

A chart re-renders only when its state changes. The canonical `DirtyFlags` struct is
defined in `midas-chart::dirty` (see chart-interaction-system.md for the full
definition and rationale). Summary:

```rust
/// Canonical definition lives in midas-chart::dirty. Uses generation counters (u64)
/// instead of booleans. See chart-interaction-system.md for the full struct.
///
/// Fields: camera, candles, indicators, crosshair, levels, grid, theme
/// Mutation methods: mark_camera(), mark_data(), mark_indicators(),
///                   mark_crosshair(), mark_levels(), mark_theme()
///
/// Why counters, not booleans:
/// iced's Primitive::prepare() takes &self (immutable). Boolean flags
/// that need clearing after consumption are incompatible with this API.
/// Generation counters solve this: the writer increments the counter, and
/// the DirtyTracker (owned by ChartGpuResources, which IS mutable in
/// prepare() via &mut Pipeline) remembers the last-seen
/// generation.
pub struct DirtyFlags { /* see chart-interaction-system.md */ }

/// Tracks which generations the GPU pipeline has already processed.
/// Owned by ChartGpuResources, NOT by application state.
pub struct DirtyTracker {
    last_seen: DirtyFlags,
}

impl DirtyTracker {
    pub fn needs_camera_update(&self, current: &DirtyFlags) -> bool { ... }
    pub fn needs_candle_rebuild(&self, current: &DirtyFlags) -> bool { ... }
    // ... etc for each counter ...
    pub fn any_dirty(&self, current: &DirtyFlags) -> bool { ... }
    pub fn acknowledge(&mut self, current: &DirtyFlags) { ... }
    // NOTE: there is no clear() method. Counters are never reset.
}
```

**Dirty tiers** (from cheapest to most expensive GPU work):

| Tier | Condition | GPU Work |
|---|---|---|
| 0 | No generation changed | Zero — replay last frame's draw calls |
| 1 | Only `crosshair` changed | Re-upload 32-byte crosshair UBO |
| 2 | Only `camera` changed | Re-upload camera UBO + rebuild grid instances |
| 3 | `candles` or `indicators` changed | Rebuild instance buffers (expensive) |
| 4 | `theme` changed | Full instance rebuild (colors baked in) |

When no generation counter has changed since the tracker's last acknowledgment,
`prepare()` skips all GPU uploads and `render()` re-issues the same draw calls
with cached GPU buffers. The GPU work is near-zero.

### 1.4 Resource Sharing Between Charts

**Shared across ALL charts (created once in `Pipeline::new()`, stored in `ChartPipeline`):**

| Resource | Lifetime | Why Shared |
|---|---|---|
| `wgpu::RenderPipeline` (candle) | App lifetime | Identical shader for all charts |
| `wgpu::RenderPipeline` (volume) | App lifetime | Identical shader for all charts |
| `wgpu::RenderPipeline` (grid) | App lifetime | Identical shader for all charts |
| `wgpu::RenderPipeline` (text/msdf) | App lifetime | Identical shader for all charts |
| `wgpu::RenderPipeline` (crosshair) | App lifetime | Identical shader for all charts |
| `wgpu::RenderPipeline` (hline) | App lifetime | Identical shader for all charts |
| Unit quad vertex buffer (6 vertices) | App lifetime | Every instanced pipeline uses it |
| MSDF font atlas texture + sampler | App lifetime | All text rendering shares one atlas |
| `wgpu::BindGroupLayout` objects | App lifetime | Layouts are shared, bind groups are per-chart |

**Per-chart resources (one set per ChartPanel):**

| Resource | Lifetime | Size Estimate |
|---|---|---|
| Candle instance buffer | Per-chart, resizable | 48 bytes x 10,000 = 480 KB |
| Volume instance buffer | Per-chart, resizable | 32 bytes x 10,000 = 320 KB |
| Grid line instance buffer | Per-chart, per-frame | 32 bytes x 100 = 3.2 KB |
| Crosshair uniform buffer | Per-chart | 32 bytes |
| Camera uniform buffer | Per-chart | 80 bytes |
| Horizontal level buffer | Per-chart | 48 bytes x 50 = 2.4 KB |
| Indicator line buffers | Per-chart, per-indicator | 16 bytes x 10,000 per series |
| Text instance buffer (axis labels) | Per-chart, per-frame | 48 bytes x 200 = 9.6 KB |

**Total GPU memory per chart**: ~830 KB typical, ~2 MB worst case.
**Total for 20 charts**: ~17 MB typical, ~40 MB worst case. Trivial.

### 1.5 Pipeline Architecture (iced 0.14 Pattern)

In iced 0.14, the `Primitive` trait has an associated type `Pipeline` that implements
`trait Pipeline { fn new(device, queue, format) -> Self; }`. The Pipeline instance is
created once by iced and shared across ALL widget instances of the same Primitive type.
This is a perfect fit for our architecture: shared GPU pipelines live at the Pipeline
level, and per-chart state is stored in a `HashMap` inside the Pipeline.

```rust
/// The Pipeline associated type for ChartPrimitive.
/// Created once by iced via Pipeline::new(device, queue, format).
/// Shared across ALL chart widget instances.
///
/// Contains both the shared render pipelines (created once) and
/// per-chart GPU resources (created/destroyed as charts are added/removed).
pub struct ChartPipeline {
    // ── Shared render pipelines (created once in Pipeline::new) ──
    pub shared: SharedPipelines,

    // ── Per-chart GPU resources ──────────────────────────────────
    /// Keyed by ChartId. Created lazily on first prepare() for each chart.
    /// Removed when a chart is closed.
    pub charts: HashMap<ChartId, ChartGpuResources>,
}

impl shader::Pipeline for ChartPipeline {
    fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
    ) -> Self {
        Self {
            shared: SharedPipelines::new(device, queue, format),
            charts: HashMap::new(),
        }
    }
}

/// Shared GPU resources — identical across all charts.
/// Created once in Pipeline::new().
pub struct SharedPipelines {
    pub candle_pipeline: wgpu::RenderPipeline,
    pub volume_pipeline: wgpu::RenderPipeline,
    pub grid_pipeline: wgpu::RenderPipeline,
    pub text_pipeline: wgpu::RenderPipeline,
    pub crosshair_pipeline: wgpu::RenderPipeline,
    pub hline_pipeline: wgpu::RenderPipeline,

    // Shared geometry
    pub unit_quad_vbo: wgpu::Buffer,   // 6 vertices: two triangles forming [0,1]x[0,1]
    pub unit_line_vbo: wgpu::Buffer,   // 2 vertices: (0,0) and (1,0) for line segments

    // Shared layouts
    pub camera_bind_group_layout: wgpu::BindGroupLayout,
    pub texture_bind_group_layout: wgpu::BindGroupLayout,

    // Shared uniform buffers for draw-mode / px_range (replaces push constants).
    // Separate buffers and bind groups for each parameter variant, because
    // queue.write_buffer() writes are staged and NOT visible within the same
    // render pass. We swap bind groups via set_bind_group() between draws.
    pub draw_params_bind_group_layout: wgpu::BindGroupLayout,
    pub wick_params_ubo: wgpu::Buffer,
    pub wick_params_bind_group: wgpu::BindGroup,   // draw_mode=0
    pub body_params_ubo: wgpu::Buffer,
    pub body_params_bind_group: wgpu::BindGroup,   // draw_mode=1
    pub text_params_ubo: wgpu::Buffer,
    pub text_params_bind_group: wgpu::BindGroup,   // px_range=4.0

    // Shared textures
    pub msdf_atlas: MsdfAtlas,
}

/// Per-chart GPU resources. Stored inside ChartPipeline::charts HashMap.
pub struct ChartGpuResources {
    pub chart_id: ChartId,

    // Uniform buffers
    pub camera_ubo: wgpu::Buffer,
    pub camera_bind_group: wgpu::BindGroup,

    // Instance buffers (resizable)
    pub candle_instance_buf: GrowableBuffer,
    pub volume_instance_buf: GrowableBuffer,
    pub grid_instance_buf: GrowableBuffer,
    pub text_instance_buf: GrowableBuffer,
    pub hline_instance_buf: GrowableBuffer,
    pub crosshair_ubo: wgpu::Buffer,

    // Instance counts (set each frame)
    pub candle_count: u32,
    pub volume_count: u32,
    pub grid_count: u32,
    pub text_glyph_count: u32,
    pub hline_count: u32,

    // Dirty tracking — generation counter comparison
    /// Tracks which DirtyFlags generations this chart's GPU resources
    /// have already processed. Compared against the current DirtyFlags
    /// snapshot each frame; only changed resources are re-uploaded.
    pub dirty_tracker: DirtyTracker,
}
```

**Why this resolves Open Question #1 from the iced application shell plan**:
Pipeline instances ARE shared across all widget instances of the same Primitive type.
This means `SharedPipelines` (render pipelines, unit quad VBO, font atlas) is
created exactly once and reused by every chart. Per-chart state (instance buffers,
camera UBOs) is stored in the `charts: HashMap<ChartId, ChartGpuResources>` inside
the Pipeline struct. No `Storage` type-keyed map is needed.

### 1.6 ChartScene — Framework-Agnostic Intermediate Representation

`ChartScene` is the central data contract between chart logic and GPU rendering. It describes
**everything** a single chart frame needs to draw, without referencing any iced types or wgpu
types. It is a plain Rust struct containing only numeric data (positions, colors, matrices).

```rust
/// The output of chart logic — a complete description of what to render.
/// Framework-agnostic: no iced, no wgpu types. Just data.
/// Lives in midas-chart crate.
pub struct ChartScene {
    /// Camera/projection for this frame
    pub projection: [[f32; 4]; 4],
    pub viewport: ViewportInfo,

    /// Candle rendering data (only Some when candles changed)
    pub candles: Option<Vec<CandleInstance>>,
    pub candle_count: usize,

    /// Volume rendering data (only Some when volume changed)
    pub volumes: Option<Vec<VolumeInstance>>,
    pub volume_count: usize,

    /// Grid lines (rebuilt on camera change)
    pub grid_lines: Vec<GridLine>,

    /// Axis labels (rebuilt on camera change)
    pub x_labels: Vec<AxisLabel>,
    pub y_labels: Vec<AxisLabel>,

    /// Horizontal levels
    pub levels: Vec<LevelRender>,

    /// Crosshair (if active)
    pub crosshair: Option<CrosshairRender>,

    /// Dirty generation counters — renderer compares to decide what to upload
    pub generations: SceneGenerations,
}

pub struct SceneGenerations {
    pub candles: u64,
    pub camera: u64,
    pub levels: u64,
    pub crosshair: u64,
    pub theme: u64,
}
```

**ChartScene is produced by `midas_chart::compute_chart_scene()` (a pure function) and consumed
by the wgpu renderer in `midas-render`.** It is the serialization boundary between chart logic
and GPU execution. The iced `ChartPrimitive` wraps a `ChartScene` plus a `ChartId` — it does
not carry loose `Vec<CandleInstance>` fields or access `&MidasApp`. This means:

- Chart logic (`midas-chart`) has **zero dependencies** on iced or wgpu.
- The renderer (`midas-render`) has **zero dependencies** on application state.
- The `ChartScene` is trivially unit-testable: call `compute_chart_scene()` with test inputs
  and assert on candle positions, grid line spacing, label text, etc.

The `ChartPrimitive` that flows through iced's Shader widget is now a thin wrapper:

```rust
/// The Primitive passed from Program::draw() to Primitive::prepare()/draw().
/// Wraps a ChartScene produced by compute_chart_scene().
pub struct ChartPrimitive {
    pub chart_id: ChartId,
    pub scene: ChartScene,
}
```

`Primitive::prepare()` reads `self.scene` to decide which GPU buffers to upload. The
`scene.generations` counters are compared against the `DirtyTracker` in `ChartGpuResources`
to implement the dirty-tier optimization described in Section 1.3.

---

## 2. Candlestick Pipeline

### 2.1 Design Decision: Two-Pass (Wicks Then Bodies)

**Decision: Two instanced draw calls from the SAME instance buffer, controlled by a
small uniform buffer `draw_mode` flag (updated between draw calls).**

Why two-pass over single-pass:
- Single-pass requires the fragment shader to branch on every fragment to decide wick vs body.
  This creates divergent branching across the wave/warp, which kills GPU parallelism.
- Single-pass also requires the bounding quad to cover the full wick height at the full body
  width, wasting fill rate on transparent fragments outside the wick's 1px column.
- Two-pass: Pass 1 draws thin wick rectangles (1px wide, wick_top to wick_bottom). Pass 2
  draws body rectangles (candle_width wide, body_top to body_bottom). Both read from the same
  instance buffer. The vertex shader selects which rectangle to expand based on `draw_mode`.
- No fragment shader branching. No wasted fill rate. Simpler shader.
- Cost: 2 draw calls instead of 1. Negligible — draw call overhead is ~2 microseconds each
  on modern GPUs. We are instance-bound, not draw-call-bound.

### 2.2 Instance Data Layout

> **Note**: `CandleInstance`, `VolumeInstance`, and other instance types live in
> `midas-chart::instances`. They are defined here for reference alongside the GPU pipeline
> that consumes them. `midas-render` imports them from `midas-chart`. These are pure data
> structs with `#[repr(C)]` and `bytemuck::Pod` derives -- they have no wgpu dependency.

```rust
/// GPU instance data for a single candlestick.
/// Used by both wick pass and body pass — the vertex shader reads different
/// fields depending on the draw_mode uniform.
///
/// Size: 48 bytes per instance (12 floats). Aligned to 16 bytes naturally.
/// Lives in midas-chart::instances.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CandleInstance {
    // -- Position (pixel-space, pre-snapped to physical pixels) --
    /// Center X of this candle in pixel coordinates
    pub x_center: f32,
    /// Body width in pixels (same for all candles in a frame)
    pub body_width: f32,
    /// Top of body (pixel Y — smaller value = higher on screen)
    pub body_top: f32,
    /// Bottom of body (pixel Y — larger value = lower on screen)
    pub body_bottom: f32,

    // -- Wick (pixel-space) --
    /// Top of wick = high price in pixel Y
    pub wick_top: f32,
    /// Bottom of wick = low price in pixel Y
    pub wick_bottom: f32,
    /// Wick width in physical pixels (always 1.0 after DPI adjustment)
    pub wick_width: f32,

    // -- Padding to align color to 16-byte boundary --
    pub _pad0: f32,

    // -- Color --
    /// RGBA color (linear space, NOT sRGB)
    pub color: [f32; 4],
}
// static_assert: size_of::<CandleInstance>() == 48
```

### 2.3 Vertex Buffer Layout (Unit Quad)

The unit quad is shared by all instanced pipelines:

```rust
/// A single vertex of the unit quad.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct QuadVertex {
    /// Position in [0,1] x [0,1] space
    pub position: [f32; 2],
}

/// The 6 vertices forming two triangles of the unit quad.
/// Winding order: counter-clockwise.
pub const UNIT_QUAD_VERTICES: [QuadVertex; 6] = [
    QuadVertex { position: [0.0, 0.0] }, // bottom-left
    QuadVertex { position: [1.0, 0.0] }, // bottom-right
    QuadVertex { position: [1.0, 1.0] }, // top-right
    QuadVertex { position: [0.0, 0.0] }, // bottom-left
    QuadVertex { position: [1.0, 1.0] }, // top-right
    QuadVertex { position: [0.0, 1.0] }, // top-left
];
```

### 2.4 Vertex Attribute Layout

```rust
fn candle_vertex_buffer_layout() -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<QuadVertex>() as u64,  // 8 bytes
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &[
            // location(0): position vec2<f32>
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x2,
                offset: 0,
                shader_location: 0,
            },
        ],
    }
}

fn candle_instance_buffer_layout() -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<CandleInstance>() as u64, // 48 bytes
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &[
            // location(1): x_center f32
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32,
                offset: 0,
                shader_location: 1,
            },
            // location(2): body_width f32
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32,
                offset: 4,
                shader_location: 2,
            },
            // location(3): body_top f32
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32,
                offset: 8,
                shader_location: 3,
            },
            // location(4): body_bottom f32
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32,
                offset: 12,
                shader_location: 4,
            },
            // location(5): wick_top f32
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32,
                offset: 16,
                shader_location: 5,
            },
            // location(6): wick_bottom f32
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32,
                offset: 20,
                shader_location: 6,
            },
            // location(7): wick_width f32
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32,
                offset: 24,
                shader_location: 7,
            },
            // location(8) is _pad0 — skipped (not bound to shader)
            // location(8): color vec4<f32>
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x4,
                offset: 32,
                shader_location: 8,
            },
        ],
    }
}
```

### 2.5 Draw Parameters Uniform Buffers (replaces push constants)

Push constants (`set_push_constants()`) require `Features::PUSH_CONSTANTS`, which is
not universally supported (some integrated GPUs and WebGPU backends lack it). Instead,
we use small uniform buffers with pre-written parameter sets and swap bind groups
between draw calls within the render pass.

> **Why bind group swapping instead of `queue.write_buffer()` between draws?**
> In wgpu, `queue.write_buffer()` writes are *staged* — they are not applied until the
> next `queue.submit()`. This means calling `queue.write_buffer()` between two draw
> calls within the same render pass has NO effect: both draws see the value that was
> present at the time the render pass began. To pass different uniform values to
> different draws within a single render pass, we pre-write all parameter variants
> to separate buffers (or separate offsets in one buffer) BEFORE the render pass,
> create a bind group for each variant, and swap bind groups via `set_bind_group()`
> between draws. `set_bind_group()` is a render pass command and takes effect
> immediately for subsequent draw calls.

```rust
/// Uniform buffer for per-draw-call parameters.
/// Used by the candle shader (draw_mode) and text shader (px_range).
/// Replaces push constants for maximum compatibility.
///
/// Size: 16 bytes (padded to uniform buffer alignment).
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct DrawParamsUniform {
    /// For candle shader: 0 = wick pass, 1 = body pass.
    /// For text shader: reinterpreted as px_range (f32 via bitcast).
    pub draw_mode: u32,
    /// MSDF pixel range (used by text pipeline). Stored as f32 bits.
    pub px_range: f32,
    pub _pad: [u32; 2], // Pad to 16 bytes for alignment
}
```

This uniform is bound as `@group(1) @binding(0)` in pipelines that need it (candle,
text). For the candle pipeline, TWO separate uniform buffers and bind groups are
created at initialization time — one for wick parameters (`draw_mode=0`) and one for
body parameters (`draw_mode=1`). Both are written once during `SharedPipelines::new()`
and never updated again (the values are constant). During the render pass, the correct
bind group is selected via `render_pass.set_bind_group()` before each draw call.

```rust
// Created once in SharedPipelines::new():

// Wick params buffer (draw_mode=0)
let wick_params = DrawParamsUniform { draw_mode: 0, px_range: 0.0, _pad: [0; 2] };
let wick_params_ubo = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
    label: Some("wick_params_ubo"),
    contents: bytemuck::bytes_of(&wick_params),
    usage: wgpu::BufferUsages::UNIFORM,
});
let wick_params_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
    label: Some("wick_params_bind_group"),
    layout: &draw_params_bind_group_layout,
    entries: &[wgpu::BindGroupEntry {
        binding: 0,
        resource: wick_params_ubo.as_entire_binding(),
    }],
});

// Body params buffer (draw_mode=1)
let body_params = DrawParamsUniform { draw_mode: 1, px_range: 0.0, _pad: [0; 2] };
let body_params_ubo = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
    label: Some("body_params_ubo"),
    contents: bytemuck::bytes_of(&body_params),
    usage: wgpu::BufferUsages::UNIFORM,
});
let body_params_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
    label: Some("body_params_bind_group"),
    layout: &draw_params_bind_group_layout,
    entries: &[wgpu::BindGroupEntry {
        binding: 0,
        resource: body_params_ubo.as_entire_binding(),
    }],
});

// Text params buffer (px_range=4.0, draw_mode unused by text shader)
let text_params = DrawParamsUniform { draw_mode: 0, px_range: 4.0, _pad: [0; 2] };
let text_params_ubo = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
    label: Some("text_params_ubo"),
    contents: bytemuck::bytes_of(&text_params),
    usage: wgpu::BufferUsages::UNIFORM,
});
let text_params_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
    label: Some("text_params_bind_group"),
    layout: &draw_params_bind_group_layout,
    entries: &[wgpu::BindGroupEntry {
        binding: 0,
        resource: text_params_ubo.as_entire_binding(),
    }],
});
```

### 2.6 WGSL Shader — candle.wgsl (Complete Source)

```wgsl
// ============================================================================
// candle.wgsl — Instanced candlestick renderer (wick + body two-pass)
//
// Pass 1 (draw_mode=0): Draws thin wick rectangles
// Pass 2 (draw_mode=1): Draws candle body rectangles
//
// Both passes use the same instance buffer. The vertex shader selects
// which rectangle dimensions to use based on draw_mode read from a
// uniform buffer (swapped via bind group between draw calls).
// ============================================================================

// --- Uniforms ---

struct CameraUniforms {
    /// Orthographic projection: pixel-space -> NDC
    projection: mat4x4<f32>,
};

@group(0) @binding(0)
var<uniform> camera: CameraUniforms;

// --- Draw Parameters (uniform buffer, replaces push constants) ---

struct DrawParams {
    /// 0 = wick pass, 1 = body pass
    draw_mode: u32,
    /// MSDF px_range (unused in candle shader, used by text shader)
    px_range: f32,
    _pad0: u32,
    _pad1: u32,
};

@group(1) @binding(0)
var<uniform> params: DrawParams;

// --- Vertex Input (per-vertex from unit quad VBO) ---

struct VertexInput {
    @location(0) quad_pos: vec2<f32>,  // [0,1] x [0,1]
};

// --- Instance Input (per-instance from instance buffer) ---

struct InstanceInput {
    @location(1) x_center:    f32,
    @location(2) body_width:  f32,
    @location(3) body_top:    f32,
    @location(4) body_bottom: f32,
    @location(5) wick_top:    f32,
    @location(6) wick_bottom: f32,
    @location(7) wick_width:  f32,
    // _pad0 is not bound
    @location(8) color:       vec4<f32>,
};

// --- Vertex Output ---

struct VertexOutput {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0)       color:    vec4<f32>,
};

// --- Vertex Shader ---

@vertex
fn vs_main(vert: VertexInput, inst: InstanceInput) -> VertexOutput {
    // Select rectangle dimensions based on draw mode (from uniform buffer)
    var rect_width: f32;
    var rect_top: f32;
    var rect_bottom: f32;

    if (params.draw_mode == 0u) {
        // Wick pass: thin vertical line from wick_top to wick_bottom
        rect_width  = inst.wick_width;
        rect_top    = inst.wick_top;
        rect_bottom = inst.wick_bottom;
    } else {
        // Body pass: wide rectangle from body_top to body_bottom
        rect_width  = inst.body_width;
        rect_top    = inst.body_top;
        rect_bottom = inst.body_bottom;
    }

    // Expand unit quad [0,1]x[0,1] to the rectangle in pixel space.
    //
    // X: center the rectangle on x_center
    //   quad_pos.x=0 -> left edge  = x_center - rect_width/2
    //   quad_pos.x=1 -> right edge = x_center + rect_width/2
    //
    // Y: stretch from rect_top to rect_bottom
    //   quad_pos.y=0 -> rect_top    (top of rect = min Y = higher on screen)
    //   quad_pos.y=1 -> rect_bottom (bottom of rect = max Y = lower on screen)

    let px = inst.x_center - rect_width * 0.5 + vert.quad_pos.x * rect_width;
    let py = rect_top + vert.quad_pos.y * (rect_bottom - rect_top);

    var out: VertexOutput;
    out.clip_pos = camera.projection * vec4<f32>(px, py, 0.0, 1.0);
    out.color    = inst.color;
    return out;
}

// --- Fragment Shader ---

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // No branching. Flat color. Pixel-perfect hard edges.
    return in.color;
}
```

### 2.7 Doji Handling (body_top == body_bottom)

When open == close, the body collapses to zero height and becomes invisible. We handle
this by enforcing a minimum body height of 1 physical pixel during instance buffer
construction on the CPU side:

```rust
fn build_candle_instance(
    candle: &CandleSlice,
    index: usize,
    camera: &Camera2D,
    dpi_scale: f32,
) -> CandleInstance {
    let open_y = camera.price_to_y(candle.opens[index] as f64);
    let close_y = camera.price_to_y(candle.closes[index] as f64);
    let high_y = camera.price_to_y(candle.highs[index] as f64);
    let low_y = camera.price_to_y(candle.lows[index] as f64);

    let one_pixel = 1.0 / dpi_scale;

    let (mut body_top, mut body_bottom) = if open_y < close_y {
        (open_y, close_y)
    } else {
        (close_y, open_y)
    };

    // Doji: ensure minimum 1 physical pixel height
    if (body_bottom - body_top) < one_pixel {
        let center = (body_top + body_bottom) * 0.5;
        body_top = center - one_pixel * 0.5;
        body_bottom = center + one_pixel * 0.5;
    }

    // Pixel-snap all edges (see Section 6 for snap_to_pixel)
    let body_top = snap_to_pixel(body_top, dpi_scale);
    let body_bottom = snap_to_pixel(body_bottom, dpi_scale);
    let wick_top = snap_to_pixel(high_y, dpi_scale);
    let wick_bottom = snap_to_pixel(low_y, dpi_scale);
    let x_center = snap_to_pixel(
        camera.time_to_x(candle.timestamps[index] as f64),
        dpi_scale,
    );

    let is_bull = candle.closes[index] >= candle.opens[index];
    let color = if is_bull {
        theme::BULL_CANDLE_COLOR
    } else {
        theme::BEAR_CANDLE_COLOR
    };

    CandleInstance {
        x_center,
        body_width: camera.candle_body_width(),
        body_top,
        body_bottom,
        wick_top,
        wick_bottom,
        wick_width: 1.0 / dpi_scale, // Exactly 1 physical pixel
        _pad0: 0.0,
        color,
    }
}
```

### 2.8 Pipeline Descriptor (Relevant Fields)

```rust
fn create_candle_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    camera_bind_group_layout: &wgpu::BindGroupLayout,
    draw_params_bind_group_layout: &wgpu::BindGroupLayout,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("candle_shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("shaders/candle.wgsl").into()),
    });

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("candle_pipeline_layout"),
        bind_group_layouts: &[
            camera_bind_group_layout,        // group(0): camera projection
            draw_params_bind_group_layout,   // group(1): draw_mode uniform
        ],
        push_constant_ranges: &[], // No push constants — using uniform buffer instead
    });

    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("candle_pipeline"),
        layout: Some(&pipeline_layout),

        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[
                candle_vertex_buffer_layout(),   // slot 0: unit quad
                candle_instance_buffer_layout(),  // slot 1: per-instance
            ],
        },

        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            // No culling — we want both faces (quads may face either way)
            cull_mode: None,
            polygon_mode: wgpu::PolygonMode::Fill,
            unclipped_depth: false,
            conservative: false,
        },

        depth_stencil: None, // 2D rendering — no depth buffer

        multisample: wgpu::MultisampleState {
            count: 1,       // NO MSAA — we want hard pixel edges
            mask: !0,
            alpha_to_coverage_enabled: false,
        },

        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                // Opaque blending for candles — no transparency
                blend: Some(wgpu::BlendState::REPLACE),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),

        multiview: None,
        cache: None,
    })
}
```

### 2.9 Draw Calls

```rust
fn draw_candles(
    render_pass: &mut wgpu::RenderPass<'_>,
    shared: &SharedPipelines,
    chart: &ChartGpuResources,
) {
    if chart.candle_count == 0 {
        return;
    }

    render_pass.set_pipeline(&shared.candle_pipeline);
    render_pass.set_bind_group(0, &chart.camera_bind_group, &[]);
    render_pass.set_vertex_buffer(0, shared.unit_quad_vbo.slice(..));
    render_pass.set_vertex_buffer(1, chart.candle_instance_buf.buffer().slice(..));

    // Pass 1: Wicks (draw_mode = 0)
    // Swap to wick_params_bind_group — this bind group references the uniform
    // buffer pre-written with draw_mode=0 at SharedPipelines creation time.
    // NOTE: We do NOT use queue.write_buffer() here because wgpu staged writes
    // are not visible until queue.submit(). Bind group swapping takes effect
    // immediately within the render pass.
    render_pass.set_bind_group(1, &shared.wick_params_bind_group, &[]);
    render_pass.draw(0..6, 0..chart.candle_count);

    // Pass 2: Bodies (draw_mode = 1)
    // Swap to body_params_bind_group — same layout, different buffer with draw_mode=1.
    render_pass.set_bind_group(1, &shared.body_params_bind_group, &[]);
    render_pass.draw(0..6, 0..chart.candle_count);
}
```

---

## 3. Volume Bar Pipeline

### 3.1 Instance Data Layout

```rust
/// GPU instance data for a single volume bar.
/// Drawn as a filled rectangle at the bottom of the chart area.
/// Size: 32 bytes per instance (8 floats).
/// Lives in midas-chart::instances.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct VolumeInstance {
    /// Center X of the bar in pixel coordinates (same as candle x_center)
    pub x_center: f32,
    /// Bar width in pixels (same as candle body_width, or slightly narrower)
    pub width: f32,
    /// Top of bar in pixel Y (higher volume = lower Y value = higher on screen)
    pub y_top: f32,
    /// Bottom of bar in pixel Y (constant: bottom of volume area)
    pub y_bottom: f32,
    /// RGBA color with alpha for semi-transparency
    pub color: [f32; 4],
}
```

### 3.2 WGSL Shader — volume.wgsl

```wgsl
// ============================================================================
// volume.wgsl — Instanced semi-transparent volume bar renderer
// ============================================================================

struct CameraUniforms {
    projection: mat4x4<f32>,
};

@group(0) @binding(0)
var<uniform> camera: CameraUniforms;

struct VertexInput {
    @location(0) quad_pos: vec2<f32>,
};

struct InstanceInput {
    @location(1) x_center: f32,
    @location(2) width:    f32,
    @location(3) y_top:    f32,
    @location(4) y_bottom: f32,
    @location(5) color:    vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0)       color:    vec4<f32>,
};

@vertex
fn vs_main(vert: VertexInput, inst: InstanceInput) -> VertexOutput {
    let px = inst.x_center - inst.width * 0.5 + vert.quad_pos.x * inst.width;
    let py = inst.y_top + vert.quad_pos.y * (inst.y_bottom - inst.y_top);

    var out: VertexOutput;
    out.clip_pos = camera.projection * vec4<f32>(px, py, 0.0, 1.0);
    out.color    = inst.color;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return in.color;
}
```

### 3.3 Volume Scaling

Volume bars occupy the bottom 20% of the chart area. Volume is scaled relative
to the maximum volume in the visible range:

```rust
fn build_volume_instances(
    candles: &CandleSlice,
    visible_range: Range<usize>,
    camera: &Camera2D,
    dpi_scale: f32,
    theme: &ChartTheme,
) -> Vec<VolumeInstance> {
    let volume_area_top = camera.viewport_height as f32 * 0.80;
    let volume_area_bottom = camera.viewport_height as f32;

    // Find max volume in visible range for normalization
    let max_volume = candles.volumes[visible_range.clone()]
        .iter()
        .copied()
        .max()
        .unwrap_or(1) as f32;

    visible_range.map(|i| {
        let normalized = candles.volumes[i] as f32 / max_volume;
        let bar_height = normalized * (volume_area_bottom - volume_area_top);
        let y_top = snap_to_pixel(volume_area_bottom - bar_height, dpi_scale);

        let is_bull = candles.closes[i] >= candles.opens[i];
        let color = if is_bull {
            theme.volume_bull_color // e.g., [0.18, 0.80, 0.44, 0.30]
        } else {
            theme.volume_bear_color // e.g., [0.91, 0.30, 0.24, 0.30]
        };

        VolumeInstance {
            x_center: snap_to_pixel(
                camera.time_to_x(candles.timestamps[i] as f64),
                dpi_scale,
            ),
            width: camera.candle_body_width(), // Same width as candle bodies
            y_top,
            y_bottom: volume_area_bottom,
            color,
        }
    }).collect()
}
```

### 3.4 Blending Mode

Volume bars are semi-transparent so price data behind them remains visible:

```rust
// In the volume pipeline descriptor:
fragment: Some(wgpu::FragmentState {
    // ...
    targets: &[Some(wgpu::ColorTargetState {
        format,
        blend: Some(wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::SrcAlpha,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
        }),
        write_mask: wgpu::ColorWrites::ALL,
    })],
}),
```

Volume alpha values: 0.25-0.35 for normal bars. The exact alpha depends on the theme —
dark themes use slightly higher alpha (0.30) for visibility, light themes use lower
alpha (0.20) to avoid overwhelming the price data.

### 3.5 Volume Bar Pipeline Descriptor Differences

The volume pipeline is nearly identical to the candle pipeline with these differences:

| Setting | Candle Pipeline | Volume Pipeline |
|---|---|---|
| Blend state | `REPLACE` (opaque) | `SrcAlpha/OneMinusSrcAlpha` (transparent) |
| Draw params uniform | Yes (draw_mode: 0=wick, 1=body) | No (single pass) |
| Instance stride | 48 bytes | 32 bytes |
| Instance attributes | 8 locations | 5 locations |

---

## 4. Grid Line Pipeline

### 4.1 Design: Axis-Aligned Thin Rectangles

Grid lines are NOT rendered using `wgpu::PrimitiveTopology::LineList` because GPU line
rasterization varies across hardware and does not guarantee exact 1px width. Instead,
each grid line is a filled rectangle that is exactly 1 physical pixel wide (horizontal lines)
or 1 physical pixel tall (vertical lines).

### 4.2 Instance Data Layout

```rust
/// A single grid line (horizontal or vertical).
/// Size: 32 bytes per instance.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GridLineInstance {
    /// Start point in pixel coordinates
    pub start: [f32; 2],
    /// End point in pixel coordinates
    pub end: [f32; 2],
    /// Thickness in logical pixels (1.0 / dpi_scale for 1 physical pixel)
    pub thickness: f32,
    /// Padding
    pub _pad: f32,
    /// RGBA color (low alpha for subtle grid)
    pub color: [f32; 4],
}
// Note: 32 bytes = 8 floats, but with padding it's 40 bytes.
// Let's restructure for exact 32 bytes:
```

Actually, let me design a more efficient layout. Since grid lines are always axis-aligned,
we can simplify:

```rust
/// A single axis-aligned grid line.
/// Horizontal lines: constant Y, span full chart width.
/// Vertical lines: constant X, span full chart height.
/// Size: 32 bytes per instance (8 floats).
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GridLineInstance {
    /// Rectangle bounds in pixel coordinates: [left, top, right, bottom]
    pub rect: [f32; 4],
    /// RGBA color
    pub color: [f32; 4],
}
```

For a horizontal grid line at price Y on a chart that is `w` pixels wide:

```rust
let y = snap_to_pixel(camera.price_to_y(price), dpi_scale);
let one_px = 1.0 / dpi_scale;
GridLineInstance {
    rect: [0.0, y, chart_width, y + one_px],
    color: theme.grid_line_color,
}
```

For a vertical grid line at time X on a chart that is `h` pixels tall:

```rust
let x = snap_to_pixel(camera.time_to_x(timestamp), dpi_scale);
let one_px = 1.0 / dpi_scale;
GridLineInstance {
    rect: [x, 0.0, x + one_px, chart_height],
    color: theme.grid_line_color,
}
```

### 4.3 WGSL Shader — grid.wgsl

```wgsl
// ============================================================================
// grid.wgsl — Instanced axis-aligned rectangle renderer for grid lines
// ============================================================================

struct CameraUniforms {
    projection: mat4x4<f32>,
};

@group(0) @binding(0)
var<uniform> camera: CameraUniforms;

struct VertexInput {
    @location(0) quad_pos: vec2<f32>, // [0,1]x[0,1] unit quad
};

struct InstanceInput {
    @location(1) rect:  vec4<f32>,  // [left, top, right, bottom] in pixel space
    @location(2) color: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0)       color:    vec4<f32>,
};

@vertex
fn vs_main(vert: VertexInput, inst: InstanceInput) -> VertexOutput {
    // Expand unit quad to the rectangle
    let px = inst.rect.x + vert.quad_pos.x * (inst.rect.z - inst.rect.x);
    let py = inst.rect.y + vert.quad_pos.y * (inst.rect.w - inst.rect.y);

    var out: VertexOutput;
    out.clip_pos = camera.projection * vec4<f32>(px, py, 0.0, 1.0);
    out.color    = inst.color;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return in.color;
}
```

### 4.4 Pixel-Snapping Math for Crisp 1px Lines

This is the most critical section for pixel-perfect rendering. A grid line that falls
on a sub-pixel boundary will be antialiased by the rasterizer into two adjacent pixels,
each at ~50% intensity, creating a blurry 2px-wide line instead of a crisp 1px line.

**The core pixel-snapping function:**

```rust
/// Snap a logical-pixel coordinate to the nearest physical pixel boundary,
/// then convert back to logical pixels.
///
/// At DPI 1.0: snaps to integer pixels. 50.3 -> 50.0
/// At DPI 1.5: snaps to 2/3-pixel grid. 50.3 -> 50.333...
/// At DPI 2.0: snaps to half-pixel grid. 50.3 -> 50.5
///
/// IMPORTANT: This snaps to the TOP/LEFT edge of a physical pixel.
/// For a 1px-wide line, the rectangle spans [snapped, snapped + 1/dpi_scale].
#[inline]
pub fn snap_to_pixel(logical_px: f32, dpi_scale: f32) -> f32 {
    (logical_px * dpi_scale).floor() / dpi_scale
}

/// Snap a coordinate to the CENTER of the nearest physical pixel.
/// Used for wick center-X to ensure the 1px wick line lands on a full pixel.
#[inline]
pub fn snap_to_pixel_center(logical_px: f32, dpi_scale: f32) -> f32 {
    ((logical_px * dpi_scale).floor() + 0.5) / dpi_scale
}
```

**Why `floor()` and not `round()`?**

`round()` can cause adjacent elements to snap to the same pixel or skip a pixel,
creating irregular spacing. `floor()` gives deterministic, monotonic results:
if A < B in continuous space, snap(A) <= snap(B) in pixel space. This matters for
candle spacing — candles must never overlap or have inconsistent gaps.

**Constructing a crisp horizontal grid line at price `p`:**

```rust
fn make_horizontal_grid_line(
    price: f64,
    camera: &Camera2D,
    chart_width: f32,
    dpi_scale: f32,
    color: [f32; 4],
) -> GridLineInstance {
    // Convert price to logical pixel Y
    let y_logical = camera.price_to_y(price);

    // Snap to physical pixel boundary
    let y_snapped = snap_to_pixel(y_logical, dpi_scale);

    // The line is a rectangle from y_snapped to y_snapped + 1_physical_pixel
    let one_physical_px = 1.0 / dpi_scale;

    GridLineInstance {
        rect: [0.0, y_snapped, chart_width, y_snapped + one_physical_px],
        color,
    }
}
```

**DPI correctness proof:**

| DPI Scale | 1 physical pixel in logical px | Line rect [y, y+h] | Physical pixels covered |
|---|---|---|---|
| 1.0x | 1.0 | [50.0, 51.0] | Row 50 only |
| 1.25x | 0.8 | [40.0, 40.8] | Physical row 50 only (40.0 * 1.25 = 50.0, 40.8 * 1.25 = 51.0) |
| 1.5x | 0.667 | [33.333, 34.0] | Physical row 50 only |
| 2.0x | 0.5 | [25.0, 25.5] | Physical row 50 only |

In all cases, exactly ONE physical pixel row is covered. No sub-pixel AA blur.

### 4.5 Adaptive Grid Density

Grid lines should be dense enough to be useful but not so dense that they clutter
the chart. The algorithm:

```rust
/// Compute price grid line spacing for the Y-axis.
/// Returns the price interval between grid lines.
///
/// Rules:
/// - Minimum 40 logical pixels between adjacent grid lines
/// - Grid lines at "nice" intervals: 0.01, 0.02, 0.05, 0.10, 0.25, 0.50,
///   1, 2, 5, 10, 25, 50, 100, 250, 500, 1000...
/// - Adapts to zoom level and price range
pub fn compute_price_grid_interval(
    price_range: f64,
    viewport_height: f32,
    min_pixel_spacing: f32, // typically 40.0
) -> f64 {
    let pixels_per_price = viewport_height as f64 / price_range;
    let min_price_spacing = min_pixel_spacing as f64 / pixels_per_price;

    // "Nice number" rounding: find the smallest nice number >= min_price_spacing
    nice_ceil(min_price_spacing)
}

/// Round up to the nearest "nice" number.
/// Nice numbers are: 1, 2, 2.5, 5 (times powers of 10).
fn nice_ceil(value: f64) -> f64 {
    if value <= 0.0 {
        return 1.0;
    }
    let exponent = value.log10().floor();
    let fraction = value / 10.0_f64.powf(exponent);
    let nice_fraction = if fraction <= 1.0 {
        1.0
    } else if fraction <= 2.0 {
        2.0
    } else if fraction <= 2.5 {
        2.5
    } else if fraction <= 5.0 {
        5.0
    } else {
        10.0
    };
    nice_fraction * 10.0_f64.powf(exponent)
}

/// Generate all horizontal grid line Y positions for the visible price range.
pub fn generate_price_grid_lines(
    camera: &Camera2D,
    chart_width: f32,
    dpi_scale: f32,
    theme: &ChartTheme,
) -> Vec<GridLineInstance> {
    let price_range = camera.price_high - camera.price_low;
    let interval = compute_price_grid_interval(
        price_range,
        camera.viewport_height as f32,
        40.0,
    );

    let first_line = (camera.price_low / interval).ceil() * interval;
    let mut lines = Vec::new();
    let mut price = first_line;

    while price <= camera.price_high {
        lines.push(make_horizontal_grid_line(
            price, camera, chart_width, dpi_scale, theme.grid_line_color,
        ));
        price += interval;
    }

    lines
}

/// Compute time grid line spacing for the X-axis.
/// Uses natural time boundaries: minutes, 5min, 15min, 30min, 1h, 4h,
/// 1d, 1w, 1mo depending on zoom level.
pub fn compute_time_grid_interval_ms(
    time_range_ms: f64,
    viewport_width: f32,
    min_pixel_spacing: f32, // typically 80.0 for time labels
) -> i64 {
    let pixels_per_ms = viewport_width as f64 / time_range_ms;
    let min_ms_spacing = min_pixel_spacing as f64 / pixels_per_ms;

    // Time intervals in milliseconds (ascending)
    const INTERVALS: &[i64] = &[
        1_000,          // 1 second
        5_000,          // 5 seconds
        10_000,         // 10 seconds
        15_000,         // 15 seconds
        30_000,         // 30 seconds
        60_000,         // 1 minute
        300_000,        // 5 minutes
        900_000,        // 15 minutes
        1_800_000,      // 30 minutes
        3_600_000,      // 1 hour
        14_400_000,     // 4 hours
        86_400_000,     // 1 day
        604_800_000,    // 1 week
        2_592_000_000,  // ~30 days (1 month)
    ];

    // Find the smallest interval >= min_ms_spacing
    for &interval in INTERVALS {
        if interval as f64 >= min_ms_spacing {
            return interval;
        }
    }

    // Beyond 1 month: use monthly intervals
    2_592_000_000
}
```

### 4.6 Grid Pipeline Descriptor

Same as the candle pipeline except:
- Blend: `SrcAlpha/OneMinusSrcAlpha` (grid lines are semi-transparent)
- No draw params uniform binding needed (single pass)
- Instance buffer layout uses the `GridLineInstance` struct

---

## 5. Axis Label Rendering

### 5.1 Text Rendering Strategy: MSDF Font Atlas

We use a Multi-channel Signed Distance Field (MSDF) font atlas for all text rendering.
MSDF provides resolution-independent crisp text at any size without needing a separate
texture per font size.

**Why MSDF over alternatives:**

| Approach | Pros | Cons |
|---|---|---|
| CPU-rasterized bitmap atlas | Simple, pixel-perfect at target size | Blurry at other sizes, per-DPI atlas needed |
| SDF (single channel) | Resolution-independent | Blurry corners on small text |
| **MSDF (multi-channel)** | **Resolution-independent, sharp corners** | Slightly more complex shader |
| Vello/Parley | Full Unicode, advanced shaping | Extra dependency, alpha-stage |
| Platform text (DirectWrite) | Perfect hinting | Not in our wgpu pipeline |

For axis labels (numbers: 0-9, decimal point, colon, slash, hyphen, space, AM/PM letters),
we need a tiny character set. A single MSDF atlas generated at build time covers everything.

### 5.2 Atlas Generation (Build-Time or Startup)

Generate the MSDF atlas using `msdfgen` (or `msdf-atlas-gen` via build script) for
a monospace or tabular-figures font (so numbers align vertically in the Y-axis).

**Recommended fonts (with tabular figures):**
- **JetBrains Mono** — excellent at 10-12px, open source, tabular figures
- **Inter** — designed for screens, has tabular figures via OpenType feature
- **Roboto Mono** — monospace, good at small sizes

**Atlas specifications:**

| Property | Value |
|---|---|
| Character set | `0123456789.:,/-+ AaMmPpSsTtWwFfJjDdOoNn` (digits + axis label chars) |
| MSDF pixel range | 4px (distance field spread) |
| Atlas texture size | 512x256 pixels (generous for our small character set) |
| Glyph size in atlas | 32x32 pixels per glyph cell |
| Texture format | `Rgba8Unorm` (R, G, B = distance channels, A = opacity/mask) |
| Font size for generation | 32px (high quality source for downscaling) |

**Atlas data structure:**

```rust
pub struct MsdfAtlas {
    /// GPU texture containing the MSDF atlas
    pub texture: wgpu::Texture,
    pub texture_view: wgpu::TextureView,
    pub sampler: wgpu::Sampler,
    pub bind_group: wgpu::BindGroup,

    /// Atlas dimensions in pixels
    pub atlas_width: u32,
    pub atlas_height: u32,

    /// Per-glyph metrics
    pub glyphs: HashMap<char, GlyphMetrics>,
}

pub struct GlyphMetrics {
    /// UV coordinates of the glyph in the atlas [u_min, v_min, u_max, v_max]
    pub uv_rect: [f32; 4],

    /// Glyph advance width (for layout), in font units normalized to 1.0 = em
    pub advance: f32,

    /// Bearing (offset from baseline to top-left of glyph bounding box)
    pub bearing_x: f32,
    pub bearing_y: f32,

    /// Glyph bounding box size in font units
    pub width: f32,
    pub height: f32,
}
```

**Atlas generation at startup (using `msdf-atlas-gen` crate or pre-baked):**

Option A — Pre-baked: Generate the atlas as a PNG + JSON at build time, embed with
`include_bytes!`. This is fastest startup.

Option B — Runtime generation: Use `ab_glyph` to parse the font, `msdfgen` to generate
per-glyph MSDF bitmaps, `etagere` to pack them into an atlas. This is ~50ms at startup.

**Recommendation: Pre-bake at build time.** The character set is fixed. Ship a
`font_atlas.png` + `font_atlas.json` (glyph metrics) embedded in the binary.

### 5.3 Text Instance Data Layout

Each visible glyph is one instance:

```rust
/// A single glyph to render.
/// Size: 48 bytes per glyph.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GlyphInstance {
    /// Screen-space position of the glyph quad [left, top, right, bottom] in logical pixels
    pub rect: [f32; 4],
    /// UV coordinates in the atlas [u_min, v_min, u_max, v_max]
    pub uv_rect: [f32; 4],
    /// RGBA text color
    pub color: [f32; 4],
}
```

### 5.4 WGSL Shader — msdf_text.wgsl

**Bind group index note:** Each pipeline has its own `PipelineLayout` with bind groups
at potentially different indices. The candle pipeline uses group(0)=camera, group(1)=draw_params.
The text pipeline uses group(0)=camera, group(1)=MSDF texture+sampler, group(2)=draw_params.
The shared `DrawParams` bind group *object* (created once in `SharedPipelines`) can be bound
at any group index via `render_pass.set_bind_group(index, &bg, &[])` -- the `@group(N)`
annotations in the WGSL must match the `PipelineLayout`'s `bind_group_layouts` array order,
but the actual `wgpu::BindGroup` object is reusable across pipelines regardless of index.

```wgsl
// ============================================================================
// msdf_text.wgsl — Multi-channel Signed Distance Field text renderer
//
// Renders axis labels (price, time) using an MSDF font atlas.
// Each glyph is one instance of a unit quad, stretched to glyph dimensions.
// The fragment shader samples the MSDF texture and applies thresholding
// for crisp edges at any size.
// ============================================================================

struct CameraUniforms {
    projection: mat4x4<f32>,
};

@group(0) @binding(0)
var<uniform> camera: CameraUniforms;

@group(1) @binding(0)
var msdf_texture: texture_2d<f32>;

@group(1) @binding(1)
var msdf_sampler: sampler;

// --- Draw Parameters (uniform buffer, replaces push constants) ---

struct DrawParams {
    /// draw_mode (unused in text shader)
    draw_mode: u32,
    /// Pixel range of the MSDF (must match atlas generation parameter).
    /// Used to scale the distance threshold for screen-pixel-size rendering.
    px_range: f32,
    _pad0: u32,
    _pad1: u32,
};

@group(2) @binding(0)
var<uniform> params: DrawParams;

// --- Inputs ---

struct VertexInput {
    @location(0) quad_pos: vec2<f32>, // [0,1]x[0,1] unit quad
};

struct InstanceInput {
    @location(1) rect:    vec4<f32>, // [left, top, right, bottom] screen px
    @location(2) uv_rect: vec4<f32>, // [u0, v0, u1, v1] atlas UVs
    @location(3) color:   vec4<f32>, // text color
};

struct VertexOutput {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) uv:    vec2<f32>,
    @location(1) color: vec4<f32>,
};

// --- Vertex Shader ---

@vertex
fn vs_main(vert: VertexInput, inst: InstanceInput) -> VertexOutput {
    // Expand unit quad to the glyph rectangle
    let px = inst.rect.x + vert.quad_pos.x * (inst.rect.z - inst.rect.x);
    let py = inst.rect.y + vert.quad_pos.y * (inst.rect.w - inst.rect.y);

    // Interpolate UVs across the quad
    let u = inst.uv_rect.x + vert.quad_pos.x * (inst.uv_rect.z - inst.uv_rect.x);
    let v = inst.uv_rect.y + vert.quad_pos.y * (inst.uv_rect.w - inst.uv_rect.y);

    var out: VertexOutput;
    out.clip_pos = camera.projection * vec4<f32>(px, py, 0.0, 1.0);
    out.uv    = vec2<f32>(u, v);
    out.color = inst.color;
    return out;
}

// --- Fragment Shader ---

/// Compute the median of three values (core MSDF operation).
fn median3(r: f32, g: f32, b: f32) -> f32 {
    return max(min(r, g), min(max(r, g), b));
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Sample the MSDF atlas
    let msdf = textureSample(msdf_texture, msdf_sampler, in.uv);

    // Compute the signed distance as the median of R, G, B channels.
    // Each channel encodes a distance field; the median reconstructs sharp corners.
    let sd = median3(msdf.r, msdf.g, msdf.b);

    // Convert signed distance to screen-pixel distance.
    // The MSDF was generated with a px_range (typically 4px), meaning
    // the distance field spans px_range pixels in the atlas texture.
    // We need to scale this to the actual screen size of the glyph.
    //
    // fwidth(in.uv) gives the UV change per screen pixel.
    // Multiplying by atlas dimensions gives atlas pixels per screen pixel.
    // We divide px_range by this to get the threshold sharpness.

    let screen_px_size = fwidth(in.uv);
    let atlas_px_per_screen_px = max(
        screen_px_size.x * f32(textureDimensions(msdf_texture).x),
        screen_px_size.y * f32(textureDimensions(msdf_texture).y),
    );
    let screen_px_distance = params.px_range * (sd - 0.5) / atlas_px_per_screen_px;

    // Clamp to [0, 1] for anti-aliased edge
    let alpha = clamp(screen_px_distance + 0.5, 0.0, 1.0);

    if (alpha < 0.01) {
        discard;
    }

    return vec4<f32>(in.color.rgb, in.color.a * alpha);
}
```

### 5.5 Glyph Layout for Axis Labels

**Y-Axis (Price Labels):**

```rust
/// Layout price labels along the right edge of the chart.
/// Labels are placed at grid line positions.
pub fn layout_price_labels(
    camera: &Camera2D,
    grid_interval: f64,
    atlas: &MsdfAtlas,
    dpi_scale: f32,
    theme: &ChartTheme,
) -> Vec<GlyphInstance> {
    let mut glyphs = Vec::new();

    let font_size = 11.0; // Logical pixels
    let label_x = camera.viewport_width as f32 - 75.0; // 75px from right edge
    let decimal_places = compute_decimal_places(grid_interval);

    let first_price = (camera.price_low / grid_interval).ceil() * grid_interval;
    let mut price = first_price;

    while price <= camera.price_high {
        let y_pixel = camera.price_to_y(price);
        let label_text = format_price(price, decimal_places);

        // Center the label vertically on the grid line
        let label_y = snap_to_pixel(y_pixel - font_size * 0.5, dpi_scale);

        layout_string(
            &label_text,
            label_x,
            label_y,
            font_size,
            dpi_scale,
            atlas,
            theme.axis_label_color,
            &mut glyphs,
        );

        price += grid_interval;
    }

    glyphs
}

/// Convert a string to a sequence of GlyphInstances.
fn layout_string(
    text: &str,
    x: f32,
    y: f32,
    font_size: f32,
    dpi_scale: f32,
    atlas: &MsdfAtlas,
    color: [f32; 4],
    out: &mut Vec<GlyphInstance>,
) {
    let scale = font_size; // Font metrics are normalized to 1.0 = font_size
    let mut cursor_x = x;

    for ch in text.chars() {
        if let Some(metrics) = atlas.glyphs.get(&ch) {
            if ch != ' ' {
                // Compute screen-space glyph rectangle
                let gx = snap_to_pixel(cursor_x + metrics.bearing_x * scale, dpi_scale);
                let gy = snap_to_pixel(y + (1.0 - metrics.bearing_y) * scale, dpi_scale);
                let gw = metrics.width * scale;
                let gh = metrics.height * scale;

                out.push(GlyphInstance {
                    rect: [gx, gy, gx + gw, gy + gh],
                    uv_rect: metrics.uv_rect,
                    color,
                });
            }

            cursor_x += metrics.advance * scale;
        }
    }
}
```

**X-Axis (Time Labels):**

```rust
/// Layout time labels along the bottom edge of the chart.
pub fn layout_time_labels(
    camera: &Camera2D,
    time_grid_interval_ms: i64,
    atlas: &MsdfAtlas,
    dpi_scale: f32,
    theme: &ChartTheme,
) -> Vec<GlyphInstance> {
    let mut glyphs = Vec::new();

    let font_size = 10.0;
    let label_y = camera.viewport_height as f32 - 18.0; // 18px from bottom

    // Align to interval boundaries
    let first_ts = ((camera.time_start as i64) / time_grid_interval_ms + 1)
        * time_grid_interval_ms;
    let mut ts = first_ts;

    while (ts as f64) <= camera.time_end {
        let x_pixel = camera.time_to_x(ts as f64);
        let label_text = format_timestamp(ts, time_grid_interval_ms);

        // Center label horizontally on the grid line
        let text_width = measure_string(&label_text, font_size, atlas);
        let label_x = snap_to_pixel(x_pixel - text_width * 0.5, dpi_scale);

        layout_string(
            &label_text,
            label_x,
            label_y,
            font_size,
            dpi_scale,
            atlas,
            theme.axis_label_color,
            &mut glyphs,
        );

        ts += time_grid_interval_ms;
    }

    glyphs
}

/// Format a timestamp for the X-axis based on the grid interval.
fn format_timestamp(ts_ms: i64, interval_ms: i64) -> String {
    let dt = chrono::NaiveDateTime::from_timestamp_millis(ts_ms).unwrap();

    if interval_ms >= 86_400_000 {
        // Daily or longer: "Jan 15" or "2024"
        if interval_ms >= 2_592_000_000 {
            dt.format("%b %Y").to_string()     // "Jan 2024"
        } else {
            dt.format("%b %d").to_string()      // "Jan 15"
        }
    } else if interval_ms >= 3_600_000 {
        // Hourly: "14:00"
        dt.format("%H:%M").to_string()
    } else if interval_ms >= 60_000 {
        // Minutes: "14:30"
        dt.format("%H:%M").to_string()
    } else {
        // Seconds: "14:30:15"
        dt.format("%H:%M:%S").to_string()
    }
}
```

### 5.6 Crisp Small Text at Any DPI

The key to crisp MSDF text at small sizes (8-12px):

1. **High-resolution atlas**: Generate MSDF at 32px or 48px in the atlas. When rendering
   at 10px logical, the MSDF interpolation preserves sharp edges.

2. **px_range scaling**: The `px_range` uniform (in the draw params buffer) must match
   the atlas generation parameter (typically 4.0). The fragment shader uses `fwidth()`
   to automatically adapt the threshold to the screen pixel density.

3. **Pixel-snap glyph positions**: Every glyph's top-left corner is snapped to a physical
   pixel boundary. This prevents sub-pixel shifting that causes asymmetric glyph rendering.

4. **Tabular figures font**: For the Y-axis, all digits must have the same advance width.
   When a price changes from "150.25" to "149.75", the digits should not shift horizontally.
   Use a font with tabular figures (most monospace fonts, or enable `tnum` OpenType feature).

5. **Hinting compensation**: MSDF does not use font hinting. At very small sizes (< 9px),
   this can cause slightly softer rendering than platform text. Mitigation: use a font
   designed for screen readability (JetBrains Mono, Inter). At our target sizes (10-12px),
   MSDF quality is excellent.

6. **DPI-aware font size**: The font size is always in logical pixels. At DPI 2.0, a 10px
   logical label renders as 20 physical pixels, which is very sharp. At DPI 1.0, the
   same label is 10 physical pixels — still readable but less defined. Consider bumping
   font size by 1px at DPI 1.0 for better readability.

```rust
fn adjusted_font_size(base_size: f32, dpi_scale: f32) -> f32 {
    if dpi_scale < 1.25 {
        base_size + 1.0  // Slightly larger at low DPI for readability
    } else {
        base_size
    }
}
```

### 5.7 Text Pipeline Descriptor Differences

| Setting | Text Pipeline | Candle Pipeline |
|---|---|---|
| Blend state | `SrcAlpha/OneMinusSrcAlpha` | `REPLACE` |
| Bind groups | Group 0: camera, Group 1: atlas texture + sampler, Group 2: draw params | Group 0: camera, Group 1: draw params |
| Draw params uniform | `px_range: f32` | `draw_mode: u32` |
| Fragment shader | MSDF sampling + threshold | Flat color |
| Instance attributes | `rect`, `uv_rect`, `color` | Candle-specific fields |

---

## 6. Camera / Coordinate System

### 6.1 Coordinate Spaces

The rendering pipeline uses four coordinate spaces:

```
Data Space (f64)       Logical Pixel Space (f32)    Physical Pixel Space    NDC
─────────────────      ──────────────────────       ────────────────        ─────
timestamp (ms)    ──>  x: 0..viewport_width    ──>  x * dpi_scale     ──>  [-1,+1]
price (dollars)   ──>  y: 0..viewport_height   ──>  y * dpi_scale     ──>  [-1,+1]
                  ^                            ^                       ^
              Camera2D                   Rasterizer               Projection
              transforms               (automatic)                 matrix
```

**Data Space**: Timestamps in epoch milliseconds (i64/f64), prices in dollars (f64).
High precision — no float truncation for 100+ years of timestamps.

**Logical Pixel Space**: The coordinate system the GPU shaders operate in. Origin at
top-left of the chart widget. X increases rightward. Y increases downward (screen
convention). Units are logical (CSS-like) pixels.

**Physical Pixel Space**: Logical pixels * dpi_scale. This is what the GPU rasterizer
actually uses. We do NOT work in this space directly — the projection matrix handles
the conversion. But we THINK in this space when computing pixel snapping.

**NDC (Normalized Device Coordinates)**: [-1, +1] in both axes. The projection matrix
converts logical pixel space to NDC for the GPU.

### 6.2 Camera Struct

> **Note**: `Camera2D` lives in `midas-chart::camera`. See chart-interaction-system.md for
> the canonical definition. The definition below is reproduced here for reference alongside
> the GPU pipeline that consumes it. `midas-render` imports `Camera2D` from `midas-chart`.

```rust
/// 2D orthographic camera for a single chart panel.
/// Maps a rectangular region of data space (time x price) to pixel space.
/// Lives in midas-chart::camera. See chart-interaction-system.md for the canonical definition.
/// midas-render imports it from midas-chart.
pub struct Camera2D {
    // --- Data-space visible region ---

    /// Leftmost visible timestamp (epoch milliseconds)
    pub time_start: f64,
    /// Rightmost visible timestamp (epoch milliseconds)
    pub time_end: f64,
    /// Lowest visible price
    pub price_low: f64,
    /// Highest visible price
    pub price_high: f64,

    // --- Viewport ---

    /// Width of the chart area in logical pixels (excludes Y-axis label area)
    pub chart_width: f32,
    /// Height of the chart area in logical pixels (excludes X-axis label area)
    pub chart_height: f32,
    /// Total viewport width in logical pixels (includes Y-axis label area)
    pub viewport_width: u32,
    /// Total viewport height in logical pixels (includes X-axis label area)
    pub viewport_height: u32,

    // --- DPI ---

    /// Device pixel ratio (1.0, 1.25, 1.5, 2.0 on Windows)
    pub dpi_scale: f32,

    // --- Derived (cached, recomputed on change) ---

    /// Precomputed: logical pixels per millisecond
    pub px_per_ms: f64,
    /// Precomputed: logical pixels per price unit
    pub px_per_price: f64,

    // --- Layout constants ---

    /// Width of Y-axis label area in logical pixels
    pub y_axis_width: f32,
    /// Height of X-axis label area in logical pixels
    pub x_axis_height: f32,

    // --- Animation ---
    // NOTE: Animation state (target_price_low, target_price_high, animating)
    // is owned by ChartPanel in iced-application-shell.md, NOT by Camera2D.
    // Camera2D is a pure coordinate-transform struct. The fields below are
    // retained here as a reference for the data flow; the authoritative
    // animation fields live on ChartPanel, which interpolates toward targets
    // and writes the converged values into camera.price_low / price_high.
    pub target_price_low: f64,
    pub target_price_high: f64,
    pub animating: bool,
}

impl Camera2D {
    /// Chart area width = total viewport width - Y axis label width
    pub fn recalculate(&mut self) {
        self.chart_width = self.viewport_width as f32 - self.y_axis_width;
        self.chart_height = self.viewport_height as f32 - self.x_axis_height;
        self.px_per_ms = self.chart_width as f64 / (self.time_end - self.time_start);
        self.px_per_price = self.chart_height as f64 / (self.price_high - self.price_low);
    }

    /// Convert a timestamp to logical pixel X.
    #[inline]
    pub fn time_to_x(&self, timestamp: f64) -> f32 {
        ((timestamp - self.time_start) * self.px_per_ms) as f32
    }

    /// Convert a price to logical pixel Y.
    /// Price axis is inverted: high prices = low Y (top of screen).
    #[inline]
    pub fn price_to_y(&self, price: f64) -> f32 {
        ((self.price_high - price) * self.px_per_price) as f32
    }

    /// Inverse: pixel X to timestamp.
    #[inline]
    pub fn x_to_time(&self, x: f32) -> f64 {
        self.time_start + (x as f64) / self.px_per_ms
    }

    /// Inverse: pixel Y to price.
    #[inline]
    pub fn y_to_price(&self, y: f32) -> f64 {
        self.price_high - (y as f64) / self.px_per_price
    }

    /// Compute the width of one candle body in logical pixels.
    /// All candles have identical width (prevents jitter).
    pub fn candle_body_width(&self) -> f32 {
        let candle_spacing_ms = self.candle_period_ms();
        let total_px = (candle_spacing_ms as f64 * self.px_per_ms) as f32;
        // Body is 70% of total candle slot width. Remaining 30% is gap.
        let body_width = total_px * 0.70;
        // Snap to an ODD number of physical pixels for symmetric wick centering
        let physical = (body_width * self.dpi_scale).round();
        let physical_odd = if physical as u32 % 2 == 0 {
            physical + 1.0
        } else {
            physical
        };
        // Ensure minimum 1 physical pixel
        physical_odd.max(1.0) / self.dpi_scale
    }

    /// Compute the period between adjacent candles in milliseconds.
    /// This is determined by the timeframe, not by the data.
    pub fn candle_period_ms(&self) -> i64 {
        // Stored externally — passed in from chart state
        // Placeholder: determined by timeframe
        unimplemented!("set from ChartState.timeframe")
    }

    /// Visible time range in milliseconds.
    pub fn time_range_ms(&self) -> f64 {
        self.time_end - self.time_start
    }

    /// Visible price range.
    pub fn price_range(&self) -> f64 {
        self.price_high - self.price_low
    }

    /// Number of candles potentially visible (approximate).
    pub fn visible_candle_count(&self, candle_period_ms: i64) -> usize {
        (self.time_range_ms() / candle_period_ms as f64).ceil() as usize + 2
    }
}
```

### 6.3 Projection Matrix

The orthographic projection maps logical pixel space to NDC:

```rust
impl Camera2D {
    /// Build the orthographic projection matrix.
    ///
    /// Maps [0, viewport_width] x [0, viewport_height] -> [-1, +1] x [-1, +1]
    ///
    /// Note: Y is inverted (top=0, bottom=height maps to top=+1, bottom=-1)
    /// to match screen coordinates where Y increases downward.
    pub fn projection_matrix(&self) -> glam::Mat4 {
        let w = self.viewport_width as f32;
        let h = self.viewport_height as f32;

        // Standard orthographic projection:
        // x_ndc = 2*x/w - 1
        // y_ndc = 1 - 2*y/h   (Y inverted: pixel 0 -> NDC +1, pixel h -> NDC -1)
        // z_ndc = 0 (2D, no depth)

        glam::Mat4::from_cols(
            glam::Vec4::new(2.0 / w, 0.0,      0.0, 0.0),
            glam::Vec4::new(0.0,     -2.0 / h, 0.0, 0.0),
            glam::Vec4::new(0.0,     0.0,       1.0, 0.0),
            glam::Vec4::new(-1.0,    1.0,       0.0, 1.0),
        )
    }
}
```

**Why this matrix works:**

The vertex shader receives pixel coordinates (e.g., x=500, y=300) and multiplies by
this projection to get NDC. The GPU clips to [-1,+1] and then rasterizes at the physical
pixel resolution (viewport_width * dpi_scale physical pixels wide).

The iced Shader widget sets the wgpu viewport to the physical pixel dimensions of the
widget area, so our logical-pixel projection automatically maps to the correct physical
pixels.

### 6.4 Camera Uniform Buffer

```rust
/// GPU-side camera uniform (uploaded to uniform buffer).
/// Must match the WGSL struct CameraUniforms.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CameraUniform {
    pub projection: [[f32; 4]; 4], // mat4x4<f32>
}

impl Camera2D {
    pub fn to_uniform(&self) -> CameraUniform {
        CameraUniform {
            projection: self.projection_matrix().to_cols_array_2d(),
        }
    }
}
```

### 6.5 Zoom and Pan

```rust
impl Camera2D {
    /// Pan the camera by a pixel delta.
    pub fn pan(&mut self, dx_logical_px: f32, dy_logical_px: f32) {
        let dt = dx_logical_px as f64 / self.px_per_ms;
        let dp = dy_logical_px as f64 / self.px_per_price;

        // Dragging right = moving earlier in time = decreasing time_start
        self.time_start -= dt;
        self.time_end -= dt;

        // Dragging down = moving to higher prices = increasing price range
        self.price_low += dp;
        self.price_high += dp;

        self.recalculate();
    }

    /// Zoom the time axis, centered on a pixel X position.
    /// factor > 1.0 = zoom in, factor < 1.0 = zoom out.
    pub fn zoom_time(&mut self, center_x: f32, factor: f64) {
        let center_time = self.x_to_time(center_x);

        let left_dt = center_time - self.time_start;
        let right_dt = self.time_end - center_time;

        self.time_start = center_time - left_dt / factor;
        self.time_end = center_time + right_dt / factor;

        // Clamp: don't allow zooming in beyond 5 candles visible,
        // or out beyond 50 years
        let min_range_ms = 5.0 * self.candle_period_ms() as f64;
        let max_range_ms = 50.0 * 365.25 * 86400.0 * 1000.0;
        let range = self.time_end - self.time_start;
        if range < min_range_ms {
            let center = (self.time_start + self.time_end) * 0.5;
            self.time_start = center - min_range_ms * 0.5;
            self.time_end = center + min_range_ms * 0.5;
        } else if range > max_range_ms {
            let center = (self.time_start + self.time_end) * 0.5;
            self.time_start = center - max_range_ms * 0.5;
            self.time_end = center + max_range_ms * 0.5;
        }

        self.recalculate();
    }

    /// Auto-scale the Y-axis to fit visible data, with animated transition.
    pub fn auto_scale_y(&mut self, candles: &CandleSlice, visible_range: Range<usize>) {
        if visible_range.is_empty() {
            return;
        }

        let mut min_low = f32::MAX;
        let mut max_high = f32::MIN;

        for i in visible_range {
            min_low = min_low.min(candles.lows[i]);
            max_high = max_high.max(candles.highs[i]);
        }

        let range = (max_high - min_low) as f64;
        let padding = range * 0.05; // 5% padding

        self.target_price_low = min_low as f64 - padding;
        self.target_price_high = max_high as f64 + padding;
        self.animating = true;
    }

    /// Tick the Y-axis animation (call once per frame).
    pub fn tick_animation(&mut self, dt_seconds: f32) {
        if !self.animating {
            return;
        }

        let t = (dt_seconds * 10.0).min(1.0); // Fast exponential approach

        self.price_low += (self.target_price_low - self.price_low) * t as f64;
        self.price_high += (self.target_price_high - self.price_high) * t as f64;

        // Stop animating when close enough (within 0.01% of range)
        let range = self.price_high - self.price_low;
        if (self.price_low - self.target_price_low).abs() < range * 0.0001
            && (self.price_high - self.target_price_high).abs() < range * 0.0001
        {
            self.price_low = self.target_price_low;
            self.price_high = self.target_price_high;
            self.animating = false;
        }

        self.recalculate();
    }
}
```

### 6.6 Pixel-Snapping Functions (Complete Reference)

```rust
/// Pixel snapping utilities.
/// All functions take logical pixel coordinates and the DPI scale factor.

/// Snap to the LEFT/TOP edge of the nearest physical pixel.
/// Use for: rectangle left edges, glyph positions, grid line starts.
#[inline]
pub fn snap_to_pixel(logical_px: f32, dpi_scale: f32) -> f32 {
    (logical_px * dpi_scale).floor() / dpi_scale
}

/// Snap to the CENTER of the nearest physical pixel.
/// Use for: wick X positions (ensures the 1px line is centered).
#[inline]
pub fn snap_to_pixel_center(logical_px: f32, dpi_scale: f32) -> f32 {
    ((logical_px * dpi_scale).floor() + 0.5) / dpi_scale
}

/// Round a size to the nearest integer number of physical pixels.
/// Use for: candle body width, ensuring consistent sizing.
#[inline]
pub fn snap_size(logical_size: f32, dpi_scale: f32) -> f32 {
    (logical_size * dpi_scale).round().max(1.0) / dpi_scale
}

/// Ensure a size is an ODD number of physical pixels.
/// Use for: candle body width (so the wick can be centered exactly).
#[inline]
pub fn snap_size_odd(logical_size: f32, dpi_scale: f32) -> f32 {
    let physical = (logical_size * dpi_scale).round().max(1.0);
    let odd = if physical as u32 % 2 == 0 { physical + 1.0 } else { physical };
    odd / dpi_scale
}

/// One physical pixel in logical coordinates.
#[inline]
pub fn one_physical_px(dpi_scale: f32) -> f32 {
    1.0 / dpi_scale
}
```

**When to use which function:**

| Scenario | Function | Why |
|---|---|---|
| Candle body left/right edge | `snap_to_pixel` | Hard edges on pixel boundaries |
| Candle wick X center | `snap_to_pixel_center` | 1px wick centered on physical pixel |
| Candle body width | `snap_size_odd` | Odd width = wick has exact center pixel |
| Grid line position | `snap_to_pixel` | 1px line starts on pixel boundary |
| Grid line thickness | `one_physical_px` | Always exactly 1 physical pixel |
| Glyph position | `snap_to_pixel` | Prevent sub-pixel text blur |
| Price label alignment | `snap_to_pixel` | Consistent label positions |

---

## 7. GPU Resource Management

### 7.1 Buffer Allocation Strategy: GrowableBuffer

Instance buffers may need to grow as the user zooms out and more candles become visible.
We use a `GrowableBuffer` that allocates with headroom and grows by 2x when exceeded.

```rust
pub struct GrowableBuffer {
    buffer: wgpu::Buffer,
    /// Allocated capacity in bytes
    capacity: u64,
    /// Currently used bytes
    used: u64,
    /// Buffer usage flags (stored for reallocation)
    usage: wgpu::BufferUsages,
    /// Human-readable label
    label: String,
}

impl GrowableBuffer {
    /// Create a new buffer with initial capacity for `initial_count` elements
    /// of `element_size` bytes.
    pub fn new(
        device: &wgpu::Device,
        label: &str,
        element_size: u64,
        initial_count: u64,
        usage: wgpu::BufferUsages,
    ) -> Self {
        let capacity = element_size * initial_count;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: capacity,
            usage: usage | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            buffer,
            capacity,
            used: 0,
            usage: usage | wgpu::BufferUsages::COPY_DST,
            label: label.to_string(),
        }
    }

    /// Upload data to the buffer. Reallocates if needed.
    /// Returns true if the buffer was reallocated (bind groups need updating).
    pub fn write<T: bytemuck::Pod>(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        data: &[T],
    ) -> bool {
        let byte_data = bytemuck::cast_slice(data);
        let needed = byte_data.len() as u64;
        self.used = needed;

        if needed > self.capacity {
            // Grow: 2x the needed size (amortized O(1) reallocations)
            let new_capacity = (needed * 2).max(256);
            self.buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(&self.label),
                size: new_capacity,
                usage: self.usage,
                mapped_at_creation: false,
            });
            self.capacity = new_capacity;
            queue.write_buffer(&self.buffer, 0, byte_data);
            return true; // Caller must recreate bind groups
        }

        queue.write_buffer(&self.buffer, 0, byte_data);
        false
    }

    pub fn buffer(&self) -> &wgpu::Buffer {
        &self.buffer
    }

    pub fn used_bytes(&self) -> u64 {
        self.used
    }
}
```

### 7.2 Initial Allocation Sizes

| Buffer | Element Size | Initial Count | Initial Size | Growth Trigger |
|---|---|---|---|---|
| Candle instances | 48 B | 10,000 | 480 KB | Zoom out beyond 10K visible |
| Volume instances | 32 B | 10,000 | 320 KB | Same as candles |
| Grid lines | 32 B | 200 | 6.4 KB | Extreme zoom in (unlikely) |
| Text glyphs | 48 B | 500 | 24 KB | Many labels at high zoom |
| H-level instances | 48 B | 100 | 4.8 KB | User adds many levels |
| Indicator line segs | 16 B | 10,000 | 160 KB | Many indicators at zoom out |

### 7.3 When to Reallocate

Buffer reallocation is rare after initial sizing. It happens when:

1. **Zoom out significantly**: More candles visible than the buffer can hold.
   Solution: Double the buffer. This happens at most log2(max_candles/initial_size) times.

2. **Add many indicators**: Each indicator series needs a line segment buffer.
   Solution: Allocate indicator buffers lazily when indicators are added.

3. **Window resize to much larger size**: More grid lines and labels needed.
   Solution: Grid/text buffers are small — reallocation is cheap.

**Reallocation cost**: Creating a new `wgpu::Buffer` takes ~10-50us. Uploading data takes
~50-200us for a 1MB buffer. Both are well within frame budget. But reallocation invalidates
`wgpu::BindGroup` references to the old buffer — bind groups must be recreated.

### 7.4 Shared vs Per-Chart Resource Lifecycle

```
Application Start (iced calls Pipeline::new(device, queue, format)):
  ├── Create ChartPipeline containing:
  │   ├── SharedPipelines (all render pipelines, unit quad VBO, atlas, wick/body/text params UBOs)
  │   │   └── Lifetime: entire application (owned by iced's Pipeline instance)
  │   └── charts: HashMap<ChartId, ChartGpuResources> (initially empty)
  │
  ├── For each chart panel (lazily on first prepare()):
  │   ├── Create ChartGpuResources (uniform buffers, instance buffers)
  │   │   └── Stored in ChartPipeline::charts HashMap
  │   └── Create wgpu::BindGroup for camera uniform
  │       └── Lifetime: until buffer reallocation
  │
  └── On chart panel close:
      └── Remove from ChartPipeline::charts (Drop = GPU dealloc)

Per Frame (for dirty charts only):
  ├── CPU: Build instance arrays from CandleBuffer + Camera
  ├── GPU: queue.write_buffer() for each dirty buffer
  ├── GPU: Encode render pass with draw calls
  └── GPU: Submit command buffer
```

### 7.5 Minimizing GPU Uploads Per Frame

Strategies to reduce CPU-to-GPU data transfer:

1. **Dirty flagging** (Section 1.3): Only upload buffers that changed.
   - Camera move with auto-Y: upload candle + volume + grid + text instances, camera UBO
   - Crosshair only: upload crosshair UBO only (32 bytes)
   - Nothing changed: upload nothing, replay last frame's draw calls

2. **Partial buffer writes**: When only the last candle updates (real-time tick), write
   only the last element of the instance buffer:
   ```rust
   // Update only the last candle instance (48 bytes at specific offset)
   let offset = (candle_count - 1) as u64 * std::mem::size_of::<CandleInstance>() as u64;
   queue.write_buffer(&instance_buf, offset, bytemuck::bytes_of(&last_candle));
   ```

3. **Frame skipping**: If the app is in the background or the window is occluded,
   skip rendering entirely. iced handles this via its event loop.

4. **Batch uploads**: All `queue.write_buffer()` calls are batched before
   `queue.submit()`. The driver coalesces these into minimal DMA transfers.

### 7.6 Font Atlas Lifetime

The MSDF font atlas is created once and never modified:

```rust
impl MsdfAtlas {
    pub fn from_embedded(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        // Embedded at compile time
        let atlas_png = include_bytes!("../assets/font_atlas.png");
        let metrics_json = include_str!("../assets/font_atlas.json");

        // Decode PNG to RGBA pixels
        let image = image::load_from_memory(atlas_png).unwrap().to_rgba8();

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("msdf_font_atlas"),
            size: wgpu::Extent3d {
                width: image.width(),
                height: image.height(),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &image,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * image.width()),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: image.width(),
                height: image.height(),
                depth_or_array_layers: 1,
            },
        );

        let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("msdf_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,  // MUST be linear for MSDF
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        // Parse glyph metrics from JSON
        let glyphs: HashMap<char, GlyphMetrics> =
            serde_json::from_str(metrics_json).unwrap();

        let bind_group = /* ... create bind group with texture_view + sampler ... */;

        Self {
            texture,
            texture_view,
            sampler,
            bind_group,
            atlas_width: image.width(),
            atlas_height: image.height(),
            glyphs,
        }
    }
}
```

---

## 8. Render Order

### 8.1 Complete Draw Order Per Chart

Each chart panel renders in this exact order, back to front:

```
Layer 0: Background Clear
   └── Clear the chart area to the background color.
       Not a draw call — set as the render pass load operation.
       Color: theme.chart_background (e.g., #1a1a2e for dark theme)

Layer 1: Grid Lines (semi-transparent)
   └── Instanced draw: all horizontal + vertical grid lines
       Blend: SrcAlpha over background
       Typical count: 10-30 lines per chart
       Why first: Grid is the lowest visual layer. Everything draws over it.

Layer 2: Volume Bars (semi-transparent)
   └── Instanced draw: all visible volume bars
       Blend: SrcAlpha over grid + background
       Typical count: same as visible candles (500-5000)
       Why before candles: Volume is supplementary data, should not obscure price.

Layer 3: Indicator Fills (semi-transparent)
   └── Bollinger band fills, VWAP deviation zones, etc.
       Blend: SrcAlpha
       Why before lines: Fills are background context for line indicators.

Layer 4: Candle Wicks
   └── Instanced draw: thin vertical lines (1px)
       Blend: REPLACE (opaque)
       Typical count: same as visible candles
       Why before bodies: Bodies are drawn ON TOP of wicks. If a wick pokes out
       beyond the body, it's visible. The body-wick overlap is correct because
       the body (drawn next) covers the wick in the body region.

Layer 5: Candle Bodies
   └── Instanced draw: candle body rectangles
       Blend: REPLACE (opaque)
       Typical count: same as visible candles
       Why after wicks: Bodies occlude the wick in their region, which is correct.

Layer 6: Indicator Lines
   └── Line strips for SMA, EMA, Bollinger bands, etc.
       Blend: REPLACE or SrcAlpha (depends on indicator)
       Why after candles: Indicators overlay price data. They should be visible
       even where they cross candle bodies.

Layer 7: Horizontal Price Levels
   └── User-drawn horizontal lines (support/resistance)
       Blend: SrcAlpha (can be semi-transparent with dashed pattern)
       Why after indicators: User-drawn levels are interactive foreground elements.

Layer 8: Crosshair
   └── Vertical + horizontal dashed lines at cursor position
       Blend: SrcAlpha
       Why near-last: Crosshair is a transient overlay that should be visible
       above all chart data.

Layer 9: Axis Background
   └── Opaque rectangles for Y-axis (right) and X-axis (bottom) label areas
       Blend: REPLACE (opaque)
       Color: theme.axis_background (slightly different from chart bg)
       Why here: Axis backgrounds must occlude chart data that extends into
       the axis area (candle clipping is handled by scissor rect, but labels
       need a clean background).

Layer 10: Axis Labels (text)
   └── Instanced MSDF text glyphs for price and time labels
       Blend: SrcAlpha (MSDF alpha for antialiased edges)
       Why last: Text is the topmost visual element. Must not be occluded.

Layer 11: Axis Highlight (current price)
   └── Small filled rectangle + text showing the last price on the Y-axis
       Blend: REPLACE
       Why absolute last: Current price indicator is the most important
       single element on the chart. Must be above everything.
```

### 8.2 Why This Order Matters

**Transparency correctness**: Semi-transparent elements (grid, volume, crosshair) must
be rendered AFTER the opaque elements behind them for correct alpha blending. But we
also have opaque elements (candle bodies) that should occlude transparent elements
(volume bars) at the same Z-depth. The draw order resolves this:
- Volume bars blend over the grid (correct: semi-transparent over semi-transparent)
- Candle bodies overwrite volume bars (correct: opaque price data dominates)

**No depth buffer needed**: By drawing in strict back-to-front order, we avoid the
complexity and memory cost of a depth buffer. This is correct for 2D rendering where
every element has a fixed layer.

**Performance**: Opaque draws before transparent draws would be better for GPU early-Z
optimization, but our transparent layers (grid, volume) are behind the opaque layers
(candles), so we must draw them first for visual correctness. The performance difference
is negligible for 2D chart rendering.

### 8.3 Render Pass Structure

All layers share a SINGLE render pass per chart. We do NOT create separate render
passes per layer — that would flush the GPU pipeline between each layer.

```rust
fn render_chart(
    encoder: &mut wgpu::CommandEncoder,
    shared: &SharedPipelines,
    chart: &ChartGpuResources,
    target: &wgpu::TextureView,
    clip: &Rectangle<u32>,
    theme: &ChartTheme,
) {
    let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("chart_render_pass"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            resolve_target: None,
            ops: wgpu::Operations {
                // DO NOT clear here — iced has already cleared the surface.
                // We draw into our clipped region.
                load: wgpu::LoadOp::Load,
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
    });

    // Set scissor to our chart area (physical pixels)
    render_pass.set_scissor_rect(clip.x, clip.y, clip.width, clip.height);

    // Layer 1: Grid lines
    draw_grid(&mut render_pass, shared, chart);

    // Layer 2: Volume bars
    draw_volume(&mut render_pass, shared, chart);

    // Layer 3: Indicator fills (future)
    // draw_indicator_fills(&mut render_pass, shared, chart);

    // Layer 4: Candle wicks (draw_mode=0 via bind group swap)
    // Layer 5: Candle bodies (draw_mode=1 via bind group swap)
    draw_candles(&mut render_pass, shared, chart);

    // Layer 6: Indicator lines (future)
    // draw_indicator_lines(&mut render_pass, shared, chart);

    // Layer 7: Horizontal levels
    draw_hlines(&mut render_pass, shared, chart);

    // Layer 8: Crosshair
    draw_crosshair(&mut render_pass, shared, chart);

    // Layer 9: Axis backgrounds
    draw_axis_backgrounds(&mut render_pass, shared, chart, theme);

    // Layer 10: Axis labels
    draw_axis_labels(&mut render_pass, shared, chart);

    // Layer 11: Current price highlight
    draw_current_price(&mut render_pass, shared, chart);
}
```

---

## 9. Performance Targets

### 9.1 Frame Time Budget

| Scenario | Target Frame Time | Target FPS | GPU Budget | CPU Budget |
|---|---|---|---|---|
| 1 chart, 5K candles | < 4 ms | 250+ | 1.5 ms | 2.5 ms |
| 4 charts, 5K candles each | < 8 ms | 120+ | 3 ms | 5 ms |
| 20 charts, 5K candles each | < 14 ms | 60+ | 6 ms | 8 ms |
| 1 chart, 20K candles (extreme zoom out) | < 6 ms | 166+ | 2 ms | 4 ms |

### 9.2 Per-Operation Time Budget

| Operation | Time Budget | Notes |
|---|---|---|
| Camera uniform upload (1 chart) | 5 us | 64 bytes, single write_buffer |
| Instance buffer build (5K candles) | 150-250 us | CPU: iterate SoA, build CandleInstance |
| Instance buffer upload (5K candles) | 50-100 us | 240 KB via write_buffer |
| Grid line generation (20 lines) | 10 us | CPU: simple loop |
| Text layout (30 labels) | 20 us | CPU: string formatting + glyph lookup |
| Text instance upload | 10 us | ~15 KB |
| wgpu draw call (1 instanced draw) | 2-5 us | GPU command recording |
| GPU execution (all layers, 1 chart) | 500-1500 us | Actual GPU rendering time |
| Total CPU prepare (1 chart) | 300-500 us | All instance building + uploads |
| Total CPU prepare (20 charts) | 4-8 ms | 20 x 300-400us (parallelizable) |

### 9.3 Bottleneck Analysis

**Where the time actually goes (in order of cost):**

1. **CPU: Instance buffer construction** (largest cost)
   - Building CandleInstance arrays from CandleBuffer + Camera
   - For 5K candles: ~200us per chart
   - For 20 charts: 4ms total (dominating the frame)
   - Optimization: Skip if camera and data generation counters unchanged (DirtyTracker)
   - Optimization: Compute on a worker thread, swap via triple buffer
   - Optimization: Only rebuild visible range + 10% overscan

2. **GPU: Fragment shading of volume bars** (transparent fill rate)
   - Volume bars cover 20% of chart area with alpha blending
   - At 4K resolution with 20 charts: significant fill rate
   - Optimization: Volume bars are drawn BEFORE candles and don't need
     per-pixel depth testing, so the GPU can batch efficiently

3. **GPU: Text rendering** (MSDF sampling)
   - Each glyph is a textured quad with dependent texture reads
   - ~200 glyphs per chart, 4000 total at 20 charts
   - Trivial GPU cost — MSDF sampling is bandwidth-bound, not ALU-bound

4. **CPU: Text layout** (string formatting)
   - `format!()` and `chrono` formatting for axis labels
   - ~20us per chart, 400us for 20 charts
   - Optimization: Cache formatted strings, only regenerate when grid changes

5. **GPU-CPU synchronization** (pipeline stalls)
   - If CPU waits for GPU to finish previous frame before uploading new data
   - Mitigation: Double-buffer instance data (write to buffer B while GPU reads A)
   - iced + wgpu handle this internally with command buffer queuing

### 9.4 Dirty Flagging Strategy

The most impactful optimization. Categorize frame updates:

**Tier 0 — Nothing changed** (0.0 ms CPU, 0.0 ms GPU)
- No mouse movement, no data update, no animation
- Action: Skip `prepare()` and `render()` entirely
- iced only redraws when it receives events or `request_redraw()`

**Tier 1 — Crosshair only** (0.01 ms CPU, 0.1 ms GPU)
- Mouse moved over chart, nothing else changed
- Action: Update crosshair UBO (32 bytes). Re-render full chart but all
  instance buffers are cached from last frame.
- Fastest interactive path.

**Tier 2 — Camera changed** (0.3-0.5 ms CPU, 1.0 ms GPU)
- User panned or zoomed. Auto-Y animation in progress.
- Action: Rebuild all instance buffers (candles, volume, grid, text).
  Upload everything. Full render.
- Most common interactive update.

**Tier 3 — Data changed** (0.3-0.5 ms CPU, 1.0 ms GPU)
- New candle arrived via WebSocket. Or last candle updated (forming bar).
- Action: Same as Tier 2, but may also need to extend the buffer.
- Optimization: If only the last candle changed, partial write (48 bytes).

**Tier 4 — Full rebuild** (0.5-1.0 ms CPU, 1.0 ms GPU)
- Symbol changed, timeframe changed, DPI changed, theme changed.
- Action: Rebuild everything. May reallocate buffers. Reload data.
- Rare event.

### 9.5 Multi-Chart Parallelism

With 20 charts, CPU instance building is the bottleneck. Parallelize using Rayon
or manual thread pool:

```rust
/// Prepare all dirty charts in parallel.
/// Each chart's instance building is independent.
fn prepare_all_charts(
    charts: &mut [ChartGpuResources],
    states: &[ChartState],
    data_manager: &DataManager,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) {
    // Phase 1: Build instance data on CPU (parallelizable)
    // Only process charts where the tracker detects a generation change.
    let instance_data: Vec<ChartFrameData> = charts
        .par_iter()
        .zip(states.par_iter())
        .filter(|(chart, state)| chart.dirty_tracker.any_dirty(&state.dirty))
        .map(|(chart, state)| {
            // Each chart independently builds its instance arrays
            build_chart_frame_data(state, data_manager)
        })
        .collect();

    // Phase 2: Upload to GPU (must be sequential — wgpu queue is single-threaded)
    // After uploading, acknowledge the current generation counters.
    for (chart, frame_data) in charts.iter_mut().zip(instance_data.iter()) {
        let state_dirty = &frame_data.dirty_snapshot;
        if chart.dirty_tracker.any_dirty(state_dirty) {
            upload_chart_frame_data(chart, frame_data, device, queue);
            chart.dirty_tracker.acknowledge(state_dirty);
        }
    }
}
```

Note: wgpu's `queue.write_buffer()` is safe to call from any thread, but the actual
DMA transfer is batched at `queue.submit()`. The sequential upload phase is fast
because it's just memory copies into staging buffers.

---

## 10. Color System

### 10.1 Color Space

All colors in the rendering pipeline are in **linear RGB space**, not sRGB. The GPU
blending operations (alpha blending) must operate in linear space to be physically
correct. The window surface is sRGB (`Bgra8UnormSrgb`), so the GPU automatically
applies gamma correction on output.

**Conversion:**

```rust
/// Convert an sRGB hex color to linear RGB [0,1] for GPU use.
pub fn srgb_hex_to_linear(hex: u32) -> [f32; 4] {
    let r = ((hex >> 16) & 0xFF) as f32 / 255.0;
    let g = ((hex >> 8) & 0xFF) as f32 / 255.0;
    let b = (hex & 0xFF) as f32 / 255.0;

    // sRGB -> linear conversion
    fn to_linear(c: f32) -> f32 {
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }

    [to_linear(r), to_linear(g), to_linear(b), 1.0]
}

/// Same but with explicit alpha.
pub fn srgb_hex_alpha_to_linear(hex: u32, alpha: f32) -> [f32; 4] {
    let mut c = srgb_hex_to_linear(hex);
    c[3] = alpha;
    c
}
```

### 10.2 Chart Theme Struct

```rust
/// Complete color theme for chart rendering.
/// All colors are in LINEAR RGB space (not sRGB).
pub struct ChartTheme {
    // --- Background ---
    pub chart_background: [f32; 4],     // Main chart area background
    pub axis_background: [f32; 4],      // Y-axis and X-axis label area

    // --- Candles ---
    pub bull_body: [f32; 4],            // Bullish candle body (close > open)
    pub bear_body: [f32; 4],            // Bearish candle body (close < open)
    pub bull_wick: [f32; 4],            // Bullish wick (typically same as body)
    pub bear_wick: [f32; 4],            // Bearish wick (typically same as body)
    pub doji_body: [f32; 4],            // Doji (open == close) body color

    // --- Volume ---
    pub volume_bull: [f32; 4],          // Bullish volume bar (with alpha)
    pub volume_bear: [f32; 4],          // Bearish volume bar (with alpha)

    // --- Grid ---
    pub grid_line_color: [f32; 4],      // Minor grid lines (very faint)
    pub grid_major_color: [f32; 4],     // Major grid lines (slightly brighter)

    // --- Text ---
    pub axis_label_color: [f32; 4],     // Price and time label text
    pub crosshair_label_bg: [f32; 4],  // Background behind crosshair price/time labels
    pub crosshair_label_text: [f32; 4], // Crosshair label text color

    // --- Crosshair ---
    pub crosshair_color: [f32; 4],      // Crosshair line color

    // --- Horizontal Levels ---
    pub default_hline_color: [f32; 4],  // Default horizontal level color

    // --- Current Price ---
    pub current_price_bg: [f32; 4],     // Background of current price label
    pub current_price_text: [f32; 4],   // Text color of current price label
    pub current_price_line: [f32; 4],   // Dashed line from chart to Y-axis
}
```

### 10.3 Dark Theme (Default)

```rust
pub fn dark_theme() -> ChartTheme {
    ChartTheme {
        // Background: very dark blue-gray
        chart_background:   srgb_hex_to_linear(0x131722),
        axis_background:    srgb_hex_to_linear(0x1a1e2e),

        // Candles: green for bull, red for bear
        // Using TradingView-style colors for familiarity
        bull_body:          srgb_hex_to_linear(0x26a69a), // Teal-green
        bear_body:          srgb_hex_to_linear(0xef5350), // Red
        bull_wick:          srgb_hex_to_linear(0x26a69a),
        bear_wick:          srgb_hex_to_linear(0xef5350),
        doji_body:          srgb_hex_to_linear(0x787b86), // Gray

        // Volume: same hue as candles but 25% opacity
        volume_bull:        srgb_hex_alpha_to_linear(0x26a69a, 0.25),
        volume_bear:        srgb_hex_alpha_to_linear(0xef5350, 0.25),

        // Grid: very faint white
        grid_line_color:    srgb_hex_alpha_to_linear(0xffffff, 0.06),
        grid_major_color:   srgb_hex_alpha_to_linear(0xffffff, 0.12),

        // Text: muted white
        axis_label_color:   srgb_hex_alpha_to_linear(0xd1d4dc, 1.0),
        crosshair_label_bg: srgb_hex_alpha_to_linear(0x363a45, 1.0),
        crosshair_label_text: srgb_hex_alpha_to_linear(0xd1d4dc, 1.0),

        // Crosshair: medium gray
        crosshair_color:    srgb_hex_alpha_to_linear(0x9598a1, 0.60),

        // Horizontal levels: default blue
        default_hline_color: srgb_hex_alpha_to_linear(0x2962ff, 0.80),

        // Current price
        current_price_bg:   srgb_hex_to_linear(0x2962ff),
        current_price_text: srgb_hex_to_linear(0xffffff),
        current_price_line: srgb_hex_alpha_to_linear(0x2962ff, 0.50),
    }
}
```

### 10.4 Light Theme

```rust
pub fn light_theme() -> ChartTheme {
    ChartTheme {
        chart_background:   srgb_hex_to_linear(0xffffff),
        axis_background:    srgb_hex_to_linear(0xf0f3fa),

        bull_body:          srgb_hex_to_linear(0x089981), // Darker green on light bg
        bear_body:          srgb_hex_to_linear(0xf23645), // Darker red on light bg
        bull_wick:          srgb_hex_to_linear(0x089981),
        bear_wick:          srgb_hex_to_linear(0xf23645),
        doji_body:          srgb_hex_to_linear(0x9598a1),

        // Volume: lower alpha on light background
        volume_bull:        srgb_hex_alpha_to_linear(0x089981, 0.18),
        volume_bear:        srgb_hex_alpha_to_linear(0xf23645, 0.18),

        // Grid: very faint black
        grid_line_color:    srgb_hex_alpha_to_linear(0x000000, 0.06),
        grid_major_color:   srgb_hex_alpha_to_linear(0x000000, 0.12),

        // Text: dark gray on light background
        axis_label_color:   srgb_hex_alpha_to_linear(0x131722, 1.0),
        crosshair_label_bg: srgb_hex_alpha_to_linear(0x131722, 1.0),
        crosshair_label_text: srgb_hex_alpha_to_linear(0xffffff, 1.0),

        // Crosshair: medium gray
        crosshair_color:    srgb_hex_alpha_to_linear(0x9598a1, 0.50),

        // Horizontal levels
        default_hline_color: srgb_hex_alpha_to_linear(0x2962ff, 0.80),

        // Current price
        current_price_bg:   srgb_hex_to_linear(0x2962ff),
        current_price_text: srgb_hex_to_linear(0xffffff),
        current_price_line: srgb_hex_alpha_to_linear(0x2962ff, 0.50),
    }
}
```

### 10.5 Color Adaptation Rules

| Element | Dark Theme | Light Theme | Why Different |
|---|---|---|---|
| Candle body | Lighter green/red | Darker green/red | Sufficient contrast against background |
| Volume alpha | 0.25 | 0.18 | Light bg needs less opacity to be visible |
| Grid alpha | 0.06 (white) | 0.06 (black) | Same perceived brightness against opposite bg |
| Text color | Light gray (#d1d4dc) | Dark gray (#131722) | Contrast against background |
| Axis bg | Slightly lighter than chart | Slightly darker than chart | Subtle separation |

### 10.6 Color Uniform Strategy

Theme colors are baked into instance data at instance-build time (on the CPU), NOT
passed as a separate uniform. This means:

- No per-fragment uniform lookup for color
- Each CandleInstance already has its final color
- Theme change = full instance rebuild (Tier 4 — theme generation counter changed)
- Simpler shader (no theme uniform buffer, no color lookup table)

Alternative considered (color palette uniform): Pass a small uniform buffer with
bull/bear/grid colors, and each instance stores a `color_index: u32`. The fragment
shader looks up the color from the palette. This saves 12 bytes per instance (vec4 -> u32)
but adds a dependent read in the fragment shader. Not worth it — 12 bytes per instance
is negligible, and the simpler shader is faster.

---

## Appendix A: Complete Bind Group Layouts

### Camera Bind Group (Group 0 — used by ALL pipelines)

```rust
fn create_camera_bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("camera_bind_group_layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: std::num::NonZeroU64::new(
                        std::mem::size_of::<CameraUniform>() as u64,
                    ),
                },
                count: None,
            },
        ],
    })
}
```

### MSDF Texture Bind Group (Group 1 — text pipeline only)

```rust
fn create_texture_bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("texture_bind_group_layout"),
        entries: &[
            // Binding 0: MSDF texture
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            // Binding 1: Sampler
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    })
}
```

---

## Appendix B: Crosshair Pipeline Detail

The crosshair is two full-width/full-height lines (1px each) plus optional price/time
labels. It reuses the grid line pipeline (same shader) but with a different color and
is drawn at a later layer.

```rust
/// Crosshair state — updated every frame the mouse moves over the chart.
pub struct CrosshairState {
    /// Mouse position in logical pixels (relative to chart widget)
    pub x: f32,
    pub y: f32,
    /// Whether to show the crosshair
    pub visible: bool,
    /// Snapped X position (nearest candle center)
    pub snapped_x: f32,
    /// Price at mouse Y
    pub price: f64,
    /// Timestamp at mouse X (snapped to candle)
    pub timestamp: i64,
}

fn build_crosshair_instances(
    state: &CrosshairState,
    camera: &Camera2D,
    dpi_scale: f32,
    theme: &ChartTheme,
) -> Vec<GridLineInstance> {
    if !state.visible {
        return Vec::new();
    }

    let one_px = one_physical_px(dpi_scale);
    let chart_w = camera.chart_width;
    let chart_h = camera.chart_height;

    let x = snap_to_pixel(state.snapped_x, dpi_scale);
    let y = snap_to_pixel(state.y, dpi_scale);

    vec![
        // Vertical crosshair line (full height)
        GridLineInstance {
            rect: [x, 0.0, x + one_px, chart_h],
            color: theme.crosshair_color,
        },
        // Horizontal crosshair line (full width)
        GridLineInstance {
            rect: [0.0, y, chart_w, y + one_px],
            color: theme.crosshair_color,
        },
    ]
}
```

For dashed crosshairs (TC2000 style), the fragment shader can be extended with a
discard based on position modulo dash length:

```wgsl
// In the crosshair fragment shader variant:
@fragment
fn fs_crosshair(in: VertexOutput) -> @location(0) vec4<f32> {
    // Dashed pattern: 4px on, 4px off
    let pos = in.clip_pos.xy;
    // Use whichever axis the line extends along
    let along = max(pos.x, pos.y); // Simplified — actual impl checks orientation
    let dash = along % 8.0;
    if (dash >= 4.0) {
        discard;
    }
    return in.color;
}
```

---

## Appendix C: HiDPI Handling Summary

### How DPI Scale Flows Through the System

```
1. Windows reports DPI:
   └── winit::WindowEvent::ScaleFactorChanged { scale_factor: 1.5 }

2. iced receives it:
   └── iced::widget::shader::Viewport { scale_factor: 1.5, physical_width: 2880, physical_height: 1620 }

3. Our code stores it:
   └── Camera2D { dpi_scale: 1.5, viewport_width: 1920, viewport_height: 1080 }
   └── (viewport dimensions are LOGICAL pixels — physical / scale)

4. Instance building uses it:
   └── snap_to_pixel(value, 1.5)  → snaps to 1/1.5 = 0.667px grid
   └── one_physical_px(1.5)       → 0.667 logical px
   └── snap_size_odd(width, 1.5)  → rounds to odd multiples of 0.667px

5. Projection matrix uses logical pixels:
   └── Maps [0, 1920] x [0, 1080] -> NDC [-1, +1]

6. wgpu viewport is set to physical pixels:
   └── render_pass.set_viewport(0, 0, 2880, 1620, ...)
   └── (iced handles this automatically via the Shader widget)

7. Result: 1 logical pixel in our coordinate system = 1.5 physical pixels on screen
   └── Our snap_to_pixel() ensures values land on physical pixel boundaries
   └── Lines are exactly 1 physical pixel wide (0.667 logical pixels)
   └── No sub-pixel blur
```

### DPI Change Handling

When the DPI changes (window moved to a different monitor):

1. Dirty flag: `viewport = true` (full rebuild)
2. Update `camera.dpi_scale`
3. Update `camera.viewport_width` and `camera.viewport_height` (logical)
4. Recompute projection matrix
5. Rebuild ALL instance buffers (pixel snapping changes)
6. Re-render

This is a Tier 4 event — full rebuild. It happens rarely (monitor switch) so
the cost is acceptable.

---

## Appendix D: wgpu Feature Requirements

```rust
/// Features and limits required for the Midas rendering pipeline.
pub fn required_features() -> wgpu::Features {
    wgpu::Features::empty()
    // No special features required. We use uniform buffers instead of push
    // constants for maximum compatibility across all backends (DX12, Vulkan,
    // Metal, WebGPU). The draw_mode and px_range parameters are passed via
    // a small 16-byte uniform buffer updated between draw calls.
}

pub fn required_limits() -> wgpu::Limits {
    wgpu::Limits {
        // All limits use defaults, which are sufficient:
        // max_bind_groups: 4 (we use 3 max: camera + draw_params + texture)
        // max_vertex_buffers: 8 (we use 2)
        // max_vertex_attributes: 16 (we use 9 max)
        // max_buffer_size: 256 MB (we use < 2 MB per chart)
        ..wgpu::Limits::default()
    }
}
```

**Design decision: uniform buffers over push constants**. Push constants
(`Features::PUSH_CONSTANTS`) are not universally supported — some integrated GPUs,
the WebGPU backend, and certain driver versions lack support. Using tiny 16-byte
uniform buffers provides identical functionality with zero feature requirements.

**Design decision: bind group swapping over `queue.write_buffer()` between draws**.
wgpu's `queue.write_buffer()` writes are *staged* and only applied at `queue.submit()`.
Calling `queue.write_buffer()` between two draw calls within the same render pass has
no effect — both draws see the same pre-submit buffer contents. Instead, we pre-write
each parameter variant to its own buffer at initialization time and swap bind groups
via `render_pass.set_bind_group()`, which takes effect immediately. The cost is one
extra bind group switch per candle draw (wick vs body) — negligible overhead.

---

## Appendix E: Quick Reference — All Draw Calls Per Chart Frame

| # | Pipeline | Instances | Blend | Draw Params Uniform | Notes |
|---|---|---|---|---|---|
| 1 | Grid | 10-30 | SrcAlpha | None | Grid lines |
| 2 | Volume | 500-5000 | SrcAlpha | None | Volume bars |
| 3 | Candle (wick) | 500-5000 | Replace | draw_mode=0 | Thin vertical lines |
| 4 | Candle (body) | 500-5000 | Replace | draw_mode=1 | Wide rectangles |
| 5 | Grid (reused) | 2 | SrcAlpha | None | Crosshair lines |
| 6 | Grid (reused) | 2-4 | Replace | None | Axis background rects |
| 7 | Text | 100-300 | SrcAlpha | px_range=4.0 | All axis labels |
| 8 | Grid (reused) | 1 | Replace | None | Current price bg |
| 9 | Text | 5-10 | SrcAlpha | px_range=4.0 | Current price label |

**Total draw calls per chart: 9** (typical). Pipeline switches: 4 (grid, volume,
candle, text). Bind group switches: 2 (wick-to-body params swap within candle draws,
add text atlas for text draws).

**Total draw calls for 20 charts: 180**. Well within the 10,000+ draw call budget
of any modern GPU.

---

## Appendix F: Future Optimization Paths

These are NOT needed for v1 but are documented for future reference:

1. **Indirect drawing**: Use `draw_indirect()` with a GPU-side instance count buffer.
   Allows the GPU to determine how many instances to draw without CPU readback.
   Useful if we add GPU-side frustum culling.

2. **Compute shader instance building**: Move instance buffer construction from CPU
   to a compute shader. Upload raw SoA candle data to storage buffers, let the GPU
   compute pixel positions. Eliminates CPU-GPU data transfer for candle positions.
   Only useful if CPU instance building becomes a bottleneck (unlikely for < 100K candles).

3. **Texture caching**: Render static chart elements (grid, volume, historical candles)
   to an offscreen texture. Only re-render the forming candle and crosshair each frame.
   Useful for extremely large multi-chart layouts where most charts are static.

4. **Bindless textures**: Use a texture array or bindless textures to render multiple
   indicator line styles (solid, dashed, dotted) without pipeline switches.

5. **Multi-draw indirect**: Pack all charts' instance data into a single large buffer
   and use multi-draw indirect to render all charts in one draw call. Reduces draw
   call overhead from 180 to ~10. Only matters if draw calls become a bottleneck.
