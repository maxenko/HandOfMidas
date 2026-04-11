# Rendering Pipeline

This file covers the GPU pipeline additions needed to render decorator
shapes. The instance struct itself is defined in
[03-data-model.md](03-data-model.md). The implementation slice that lands
this pipeline is Slice 4 in [06-implementation.md](06-implementation.md).

Hit zones and pointer interaction are out of scope here — see
[05-interaction.md](05-interaction.md).

---

## Existing pipeline layout

The chart render orchestrator is `ChartRenderer` at
`desktop/win/crates/midas-render/src/renderer.rs:41`. It holds one field
per GPU pipeline:

```rust
pub struct ChartRenderer {
    candle_pipeline: CandlePipeline,
    volume_pipeline: VolumePipeline,
    grid_pipeline: GridPipeline,
    volume_profile_pipeline: GridPipeline,
    crosshair_pipeline: GridPipeline,
}
```

`ChartRenderer::new()` at `renderer.rs:51` constructs each pipeline with
the device and surface format. `ChartRenderer::render()` at
`renderer.rs:77-144` runs the per-frame work: camera uniform updates,
dirty-tracked instance buffer uploads, then a strict back-to-front draw
sequence.

### Draw order (verified against `renderer.rs:127-143`)

1. `grid_pipeline.draw()` — grid lines (semi-transparent, lowest layer)
2. `volume_pipeline.draw()` — volume bars
3. `volume_profile_pipeline.draw()` — volume profile histogram
4. `candle_pipeline.draw_wicks()` — wicks (opaque, thin)
5. `candle_pipeline.draw_bodies()` — bodies (opaque, wide)
6. `crosshair_pipeline.draw()` — crosshair overlay (always on top)

There is **no GPU text pipeline**. All text — axis labels, crosshair
readouts, decorator labels — is drawn by iced as an HTML-style overlay
pass on top of the GPU frame. Decorator badges must respect this
constraint; see the "Text inside badges" section below.

### Pipeline template to follow

`desktop/win/crates/midas-render/src/pipelines/grid.rs` is the closest
analogue for what `BadgePipeline` needs to do: one pipeline struct, a
growable instance buffer, a `new()` that compiles shaders and builds the
bind group layouts, an `update_instances()` that resizes the buffer on
capacity overflow, and a `draw()` that binds the pipeline and issues a
single instanced draw call. Read it end-to-end before writing
`badge.rs`.

---

## Two `ChartScene` types — CRITICAL structural note

This is the most important section in this document. The plan-eval
caught an earlier draft that only updated one of the two `ChartScene`
types; the app would not have compiled.

There are **two** distinct types named `ChartScene` in the codebase:

- **Owned IR** at `desktop/win/crates/midas-chart/src/scene.rs:20`:
  `pub struct ChartScene` — no lifetime, owns its instance vectors.
  This is what `midas-chart`'s compute pipeline builds every frame
  inside `compute_frame()`.
- **Borrowed render view** at
  `desktop/win/crates/midas-render/src/renderer.rs:20`:
  `pub struct ChartScene<'a>` — has a lifetime, holds slice references.
  This is what `ChartRenderer::render()` consumes.

`desktop/win/crates/midas-app/src/chart_widget.rs` imports both under
aliases:

```rust
use midas_chart::scene::ChartScene;                     // line 33
use midas_render::renderer::ChartScene as RenderScene;  // line 45
```

The widget builds the chart-IR `ChartScene` first, then constructs the
`RenderScene` by borrowing slices out of the chart-IR's owned fields
before passing it to `ChartRenderer::render()`.

### Implication for decorators

Adding badge rendering requires adding a `badges` field to **both**
types, and updating the `chart_widget.rs` alias site to plumb the slice
across.

**Chart-side owned IR**
(`desktop/win/crates/midas-chart/src/scene.rs`):

```rust
pub struct ChartScene {
    // ... existing fields: candles, volumes, grid_lines, ...
    pub badges: Vec<BadgeInstance>,
}
```

**Render-side borrowed view**
(`desktop/win/crates/midas-render/src/renderer.rs`):

```rust
pub struct ChartScene<'a> {
    // ... existing borrowed fields: candles: &'a [..], ...
    pub badges: &'a [BadgeInstance],
}
```

**Widget alias site**
(`desktop/win/crates/midas-app/src/chart_widget.rs`), inside the
function that builds the `RenderScene` from the chart-IR:

```rust
let render_scene = RenderScene {
    // ... existing fields
    badges: &chart_scene.badges,
};
```

Omitting either type, or forgetting the borrow in `chart_widget.rs`, is
the structural bug to watch for. The test
`scene_badges_merged_from_widget_output` listed in Slice 4 of
[06-implementation.md](06-implementation.md) exercises the full
`WidgetOutput → chart ChartScene → render ChartScene` copy path and
should fail loudly if anything is dropped.

---

## `BadgePipeline` design

**New file**:
`desktop/win/crates/midas-render/src/pipelines/badge.rs`. Add
`pub mod badge;` to `pipelines/mod.rs`.

The struct mirrors `GridPipeline`:

```rust
pub struct BadgePipeline {
    pipeline: wgpu::RenderPipeline,
    instance_buffer: wgpu::Buffer,
    instance_capacity: usize,
    // Camera uniform bind group (shared layout with GridPipeline).
    camera_bind_group: wgpu::BindGroup,
    camera_buffer: wgpu::Buffer,
}
```

### Methods

- `pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self`
  - Creates shader module from `include_str!("../../shaders/badge.wgsl")`.
  - Builds bind group layout for the camera uniform (64 bytes, same
    layout as `GridPipeline` so the same projection matrix can feed
    both).
  - Declares the vertex state: unit-quad vertex buffer (8 bytes
    per vertex, `[f32; 2]` positions) in slot 0, `BadgeInstance` in
    slot 1 with `step_mode: Instance`. The vertex attribute array for
    slot 1 must match the byte layout declared in
    `BadgeInstance::desc()` in
    `desktop/win/crates/midas-chart/src/instances.rs` — see
    [03-data-model.md](03-data-model.md) for the exact field ordering.
  - Uses the standard premultiplied-alpha blend state — badges have
    soft SDF edges, so they *must* alpha-blend against the chart
    content below.

- `pub fn update_instances(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, instances: &[BadgeInstance])`
  - Short path: if `instances.len() <= self.instance_capacity`, just
    `queue.write_buffer(...)` the head of the existing buffer.
  - Long path: if the length exceeds capacity, reallocate the buffer
    at `max(instances.len(), old_capacity * 2)` and write. Identical
    pattern to `grid.rs:147-170`.

- `pub fn draw<'a>(&'a self, render_pass: &mut wgpu::RenderPass<'a>)`
  - `set_pipeline`, `set_bind_group(0, camera_bind_group, ...)`,
    `set_vertex_buffer(0, unit_quad)`, `set_vertex_buffer(1, instance_buffer)`,
    `draw(0..4, 0..instance_count)`.
  - Assumes the render pass's render target has the same format used
    at `new()` time.

### Camera + quad sharing

The camera uniform layout and the unit quad vertex buffer already exist
in `GridPipeline`. For the first pass, duplicate the setup inside
`BadgePipeline::new()` — fast to write and keeps the two pipelines
independent. A later refactor can hoist shared resources into a
`CommonGpuResources` struct if it becomes painful, but that is out of
scope for Slice 4.

---

## Shader: `badge.wgsl`

**New file**:
`desktop/win/crates/midas-render/shaders/badge.wgsl`.

### Vertex stage

- Reads per-vertex unit quad corner in `[0, 1]^2` from slot 0.
- Reads per-instance `rect`, `fill`, `border`, `shape_id`, `shape_param`,
  `border_thickness` from slot 1.
- Expands the corner into screen-space position using `rect.xy` + corner
  times `rect.zw`, then projects through the camera uniform.
- Computes `local_uv` in `[-1, 1]` (corner mapped `0 -> -1`, `1 -> 1`).
- Passes `size = rect.zw`, `fill`, `border`, and
  `shape_data = vec4<f32>(f32(shape_id), shape_param, border_thickness, 0.0)`
  to the fragment stage.

### Fragment stage

- Recovers the local position in logical pixels:
  `let p = in.local_uv * in.size * 0.5;`
- Dispatches by `shape_id` through a `switch`:

```wgsl
var d: f32 = 0.0;
switch u32(in.shape_data.x) {
    case 0u: { d = sd_rect(p, in.size); }
    case 1u: { d = sd_rounded_box(p, in.size, in.shape_data.y); }
    case 2u: { d = sd_rounded_box(p, in.size, min(in.size.x, in.size.y) * 0.5); }
    case 3u: { d = sd_point_left(p, in.size, in.shape_data.y); }
    case 4u: { d = sd_point_right(p, in.size, in.shape_data.y); }
    case 5u: { d = sd_double_point(p, in.size, in.shape_data.y); }
    case 6u: { d = sd_chevron(p, in.size, in.shape_data.y); }
    case 7u: { d = sd_circle(p, in.size); }
    default: { d = sd_rect(p, in.size); }
}
```

- AA via screen-space derivative:
  `let aa = fwidth(d);`
  `let fill_alpha = 1.0 - smoothstep(-aa, aa, d);`
- Border via two smoothstep bands when `border_thickness > 0.0` and
  `border.a > 0.0`:
  `border_alpha = smoothstep(-t - aa, -t + aa, d) * (1.0 - smoothstep(-aa, aa, d));`
- Final colour is
  `fill * fill_alpha * (1.0 - border_alpha) + border * border_alpha`,
  otherwise `fill * fill_alpha`.

### SDF skeletons

- `sd_rect(p, size)` — standard `abs(p) - size*0.5` box SDF.
- `sd_rounded_box(p, size, r)` — box SDF shrunk by `r` then expanded.
  Covers `Rounded` (arbitrary radius) and `Pill` (radius = `min(size)/2`).
- `sd_circle(p, size)` — `length(p) - min(size.x, size.y) * 0.5`.
- `sd_point_left(p, size, point_width)` — union of a rect body
  (covering `[-size.x/2 + point_width, size.x/2]`) with a left-pointing
  triangle (tip at `(-size.x/2, 0)`, base at
  `(-size.x/2 + point_width, ±size.y/2)`) via `min(body, tri)`.
- `sd_point_right(p, size, point_width)` — mirror of `sd_point_left`.
- `sd_double_point(p, size, point_width)` — union of a rect body with
  triangles on both ends.
- `sd_chevron(p, size, point_width)` — one slanted rhombus body, no
  rect core.

The triangle helper uses iq's standard 3-edge signed-distance formula:
project onto each edge, clamp, pick the minimum absolute value, apply
sign from the winding test.

### SDF caveats

- **Triangle tip aliasing** is the one area with real shader risk.
  Standard triangle SDFs have a kink right at the tip where `fwidth`
  spikes, producing a fuzzy pixel or two. This is covered by the SDF
  spike (Slice 0 in [06-implementation.md](06-implementation.md)), not
  left to be discovered during Slice 4.
- **`fwidth()` AA** is reliable at badge heights of 12 logical pixels
  or more. For very small shapes (under 8 px) the derivative becomes
  unstable; the shader should fall back to a hard threshold
  `d < 0.0 ? 1.0 : 0.0` on a short-circuit path. The decorator layout
  pass never produces badges smaller than 12 px in practice, so this
  fallback is insurance, not the hot path.
- **`switch shape_id` branch divergence** is a non-issue at the
  decorator scale: all fragments of a single badge instance take the
  same branch because they share the per-instance `shape_id`, so the
  warp is coherent within a badge. Divergence only occurs at boundary
  tiles between badges of different shapes, which is a tiny fraction
  of total fragments.

---

## Integration point in `ChartRenderer::render()`

Inside `ChartRenderer::render()` at `renderer.rs:77-144`, add the badge
upload between the existing upload blocks and the badge draw between
`candle_pipeline.draw_bodies()` and `crosshair_pipeline.draw()`:

```rust
// Upload badges if present (cheap if empty).
if !scene.badges.is_empty() {
    self.badge_pipeline
        .update_instances(device, queue, scene.badges);
}

// ... existing draw sequence: grid -> volume -> volume_profile ->
//     candle wicks -> candle bodies ...

self.candle_pipeline.draw_bodies(render_pass);

// Layer 4.5: Decorator badges — in front of chart content,
// behind the crosshair.
if !scene.badges.is_empty() {
    self.badge_pipeline.draw(render_pass);
}

// Layer 5: Crosshair overlay (unchanged).
self.crosshair_pipeline.draw(render_pass);
```

The badge draw is placed between `candle_bodies` and `crosshair` — **not**
between `grid` and `volume` (that was a bug in the earlier draft of the
plan). Putting badges below volume would bury them under the histogram
bars, defeating the whole point of the decorator system.

The matching upload block in `render_prepare()` (the split
prepare/render path that iced's `Primitive::prepare()` uses) gets the
same `update_instances()` call. Do not duplicate the draw — that
belongs only in the pass that owns the render pass.

---

## z-order rationale

Decorator badges sit in front of the chart content (candles, wicks,
volume, volume profile, grid) so they are always visible against the
price data they decorate, but behind the crosshair so the user's
current pointer is never obscured by a static marker. This matches how
TradingView, thinkorswim, and other professional charting platforms
layer interactive markers: annotations ride on top of data, but the
cursor always wins. The crosshair is the one element that must never
be covered because it represents the user's live intent; everything
else, including decorators, is chart state and belongs below it.

---

## Text inside badges

The decorator system does **not** rasterize text on the GPU. Text
segments inside a decorator item emit `WidgetLabel` entries into
`WidgetOutput.labels`, and iced renders those in a separate overlay
pass on top of the GPU output — the same path used by
`build_crosshair_label_overlay()` at
`desktop/win/crates/midas-app/src/app/views.rs:2986`.

This has one consequence that leaks back into the renderer: **text
inside a badge cannot be clipped by the badge shape**. iced draws each
label in its own layer after `ChartRenderer::render()` has already
finished, so the GPU pipeline has no way to mask pixels the label
draws outside the badge bounds. The decorator layout pass in
`widget/decorator/compute.rs` is therefore responsible for sizing each
badge's `rect` to fully contain its text segments, not the other way
around. Badges fit the text; text never relies on the badge to hide
overflow.

Cross-references:
- Instance struct fields and byte layout: [03-data-model.md](03-data-model.md)
- `WidgetOutput` shape and `merge()` semantics (where `WidgetLabel`s accumulate): [01-research.md](01-research.md)
- Implementation slicing and test matrix: [06-implementation.md](06-implementation.md)
- Hit zones layered against this same pipeline: [05-interaction.md](05-interaction.md)
