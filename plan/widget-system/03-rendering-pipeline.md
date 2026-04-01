# 03 -- Rendering Pipeline

How widgets get from data to pixels in Hand of Midas. Covers the
three-phase architecture (Compute, Scene, GPU), buffer management,
dirty-flag tiers, draw ordering, and the render primitive vocabulary.

> Depends on: `01-core-architecture.md` (WidgetOutput, ComputeContext),
> `02-storage-and-sync.md` (Annotation, Presence, AnnotationStore).
> References: `desktop/win/plan/initial/gpu-rendering-architecture.md` (existing pipeline).
>
> Date: 2026-03-30

---

## Table of Contents

1. [Pipeline Overview](#1-pipeline-overview)
2. [Phase 1: Compute (Sans-IO)](#2-phase-1-compute-sans-io)
   - 2.1 Widget Scene Assembly
   - 2.2 WidgetScene Structure
   - 2.3 Presence-Aware Rendering
   - 2.4 Collapsed-Gaps Mode
3. [Phase 2: Scene Upload (Conditional GPU Work)](#3-phase-2-scene-upload-conditional-gpu-work)
   - 3.1 Dirty Flag Tiers for Widgets
   - 3.2 Buffer Management
   - 3.3 Integration with Existing Pipeline prepare()
4. [Phase 3: GPU Dispatch (Fixed Order)](#4-phase-3-gpu-dispatch-fixed-order)
   - 4.1 Complete Render Order
   - 4.2 Pipeline Reuse
   - 4.3 Future: MarkerInstance Pipeline
   - 4.4 Future: LineInstance Pipeline
5. [Render Primitive Vocabulary](#5-render-primitive-vocabulary)
   - 5.1 GridLineInstance (Existing, Reused)
   - 5.2 MarkerInstance (Future)
   - 5.3 What Each Widget Type Produces
6. [Text and Label Rendering](#6-text-and-label-rendering)
   - 6.1 Current Approach
   - 6.2 GPU Text (Future)
   - 6.3 Widget Labels
7. [Performance Budget](#7-performance-budget)
   - 7.1 Instance Count Estimates
   - 7.2 Frame Budget

---

## 1. Pipeline Overview

Every frame, each chart transforms annotation data into pixels through
three clearly separated phases. The phases are decoupled by data
contracts -- no phase reaches into another's domain.

```
                         PHASE 1                       PHASE 2                    PHASE 3
                        (Sans-IO)                  (Conditional Upload)         (GPU Dispatch)

 AnnotationStore ───► compute_widget_scene() ───► WidgetScene ──┐
                            │                                    │
                      ComputeContext                             ▼
                      (Camera2D,                         prepare() compares
                       viewport,                         generation counters
                       snap_fn)                                  │
                                                                 ▼
                                                     ┌──── dirty? ────┐
                                                     │  YES       NO  │
                                                     ▼                │
                                               write_buffer()     skip │
                                                     │                │
                                                     ▼                ▼
                                              ┌─────────────────────────┐
                                              │   GPU Instance Buffers  │
                                              │  fills | lines | marks  │
                                              └───────────┬─────────────┘
                                                          │
                                                          ▼
                                                    draw() issues
                                                    render_pass.draw()
                                                    in fixed layer order
                                                          │
                                                          ▼
                                                       Pixels
```

### Why Three Phases

Validated by SciChart, iced, Bevy, GPUI, and Rerun -- all separate
compute, scene-upload, and GPU-dispatch. Benefits:

1. **Testability.** Phase 1 is a pure function -- assert on
   `WidgetScene` without a GPU context.
2. **Dirty optimization.** Phase 2 skips uploads via O(1) generation
   counter comparison.
3. **Decoupling.** midas-chart has zero wgpu dependency. midas-render
   has zero app-state dependency. `WidgetScene` is the boundary.

### Data Flow End-to-End

`compute_chart_scene()` calls `compute_widget_scene()` alongside the
existing `compute_candle_instances()`, `compute_volume_instances()`,
`compute_grid()`, `compute_levels()`, and `compute_crosshair()`.
The `ChartScene` struct gains a `pub widget_scene: WidgetScene` field
and `SceneGenerations` gains `pub widgets: u64`.

---

## 2. Phase 1: Compute (Sans-IO)

### 2.1 Widget Scene Assembly

Each visible annotation produces a `WidgetOutput` via its kind's
`compute()` method. The assembly function collects all outputs into
a single `WidgetScene`, respecting visibility, timeframe filtering,
and presence state.

```rust
// ComputeContext: see canonical definition in 01-core-architecture.md Section 3.
// Key fields: camera, data, viewport, theme, snap_fn, dpi_scale, separator_y,
// candle_duration_ms, collapse_gaps.
```

The top-level assembly function:

```rust
/// Assemble all widget render data for one chart frame.
///
/// This is a pure function: no GPU, no framework, no side effects.
/// Each annotation is independently computed and merged into the
/// shared WidgetScene.
pub fn compute_widget_scene(
    annotations: &[Annotation],
    ctx: &ComputeContext,
) -> WidgetScene {
    let mut scene = WidgetScene::new();

    for ann in annotations {
        // Hidden annotations skip computation entirely.
        if !ann.presence.is_visible() {
            continue;
        }

        // Timeframe filter: annotation only shows on configured timeframes.
        if !ann.visible_on_timeframe(ctx.timeframe) {
            continue;
        }

        // Compute the widget output for this annotation.
        let output = ann.kind.compute(ctx);

        // Merge into the scene, applying presence modifiers (e.g. ghost alpha).
        scene.merge(output, ann.presence);
    }

    scene
}
```

No sorting step needed -- fills and lines go into separate buffers,
and order within a layer does not affect visual correctness. O(N)
complexity, no allocations beyond the output vectors. Typical N = 20-50.

### 2.2 WidgetScene Structure

```rust
/// All widget-produced render data for a single chart frame.
///
/// Three GPU instance buffers (fills, lines, markers) plus overlay
/// metadata (labels, hit zones). The GPU buffers contain contiguous
/// instances ready for a single draw call per layer.
pub struct WidgetScene {
    /// Filled rectangles rendered at Layer 6 in draw order.
    /// Covers: bracket zone fills, volume profile bars, note backgrounds.
    pub fills: Vec<GridLineInstance>,

    /// Lines rendered at Layer 7 in draw order.
    /// Covers: level lines, bracket legs, alert lines, order lines.
    pub lines: Vec<GridLineInstance>,

    /// Markers rendered at Layer 8 in draw order.
    /// Covers: fill markers, signal icons, alert diamonds.
    /// Uses GridLineInstance initially (small squares), upgrades to
    /// MarkerInstance when the marker pipeline is added.
    pub markers: Vec<GridLineInstance>,

    /// Text labels for iced overlay rendering.
    /// Not uploaded to GPU -- consumed by the overlay widget builder.
    pub labels: Vec<WidgetLabel>,

    /// Interaction hit zones for mouse picking.
    /// Not uploaded to GPU -- consumed by the hit-test system.
    pub hit_zones: Vec<HitZone>,

    /// Generation counter. Incremented whenever an annotation mutates.
    /// The renderer compares this against its DirtyTracker to decide
    /// whether to re-upload widget buffers.
    pub generation: u64,
}

// WidgetLabel: see canonical definition in 01-core-architecture.md Section 4.
// Fields: text, screen_x, screen_y, bg_color, text_color, font_size, anchor.

pub struct HitZone {
    pub annotation_id: AnnotationId,
    pub rect: [f32; 4],                  // [left, top, right, bottom]
    pub kind: HitZoneKind,               // LevelLine | BracketEntry | BracketTP | ...
    pub cursor: CursorIcon,
}
```

### WidgetScene::merge()

`merge()` takes a single widget's `WidgetOutput` and appends it to
the scene's buffers, applying presence modifiers:

```rust
impl WidgetScene {
    pub fn merge(&mut self, output: WidgetOutput, presence: Presence) {
        let alpha_mul = presence.alpha();
        if !presence.is_visible() {
            return;
        }

        for mut fill in output.fills {
            fill.color[3] *= alpha_mul;
            self.fills.push(fill);
        }
        for mut line in output.lines {
            line.color[3] *= alpha_mul;
            self.lines.push(line);
        }
        for mut marker in output.markers {
            marker.color[3] *= alpha_mul;
            self.markers.push(marker);
        }

        // Ghost annotations: visual only -- no labels, no interaction.
        if matches!(presence, Presence::Active) {
            self.labels.extend(output.labels);
            self.hit_zones.extend(output.hit_zones);
        }
    }
}
```

**Key design choice**: Ghost annotations produce GPU primitives at
reduced alpha but no labels and no hit zones. Ghost annotations are
visible but not interactive -- correct behavior for cross-timeframe
reference marks.

### 2.3 Presence-Aware Rendering

The canonical `Presence` enum is defined in `01-core-architecture.md`
Section 2.3 as a unit-variant enum (Active / Ghost / Hidden).
`Ghost` has a fixed alpha of 0.4 via `Presence::alpha()`.

Presence is handled at three pipeline levels:

| Level | Action | Hidden | Ghost | Active |
|-------|--------|--------|-------|--------|
| `compute_widget_scene()` | Skip/compute | Skip | Compute | Compute |
| `WidgetScene::merge()` | Alpha multiply | -- | `color[3] *= 0.4` | Unchanged |
| `WidgetScene::merge()` | Labels/hit zones | -- | Omitted | Included |

Ghost annotations are visible but not interactive. A bracket zone
fill at `[0.0, 0.5, 1.0, 0.15]` with Ghost alpha 0.4 becomes
`[0.0, 0.5, 1.0, 0.06]` -- barely visible, correct for
cross-timeframe reference.

### 2.4 Collapsed-Gaps Mode

When `collapse_gaps` is enabled, candle X positions are based on
sequential index rather than timestamp. This affects all widgets
that have time-anchored positions (bracket legs with start times,
markers at specific timestamps, ray-style levels).

The abstraction is the same `snap_fn` closure used by the crosshair:

```rust
/// The snap_fn closure abstracts collapsed-gaps vs normal mode.
///
/// In normal mode:
///   snap_fn = |cx| { find candle by timestamp, return center_x }
///
/// In collapsed mode:
///   snap_fn = |cx| { round to nearest index, return center_x }
///
/// Widgets that anchor to timestamps use snap_fn to find the
/// correct pixel X. Widgets that anchor to price only (e.g., full-
/// width levels) ignore snap_fn.
```

A `timestamp_to_pixel_x()` helper converts timestamp anchors to
pixel X, respecting the mode: in normal mode it calls
`camera.time_to_x(ts)`; in collapsed mode it finds the candle index
via `data.find_index_by_time(ts)` and computes X from the index.

| Widget Type | Collapsed-Gaps Impact |
|---|---|
| Price-only (full-width levels) | Unaffected -- no timestamp involved |
| Time-anchored (markers, notes) | Uses `timestamp_to_pixel_x()` |
| Ray-anchored (bracket legs) | Start X snaps to index; extends to viewport right |

```rust
fn timestamp_to_pixel_x(
    timestamp: i64, ctx: &ComputeContext, data: &dyn CandleData,
) -> Option<f32> {
    if ctx.collapse_gaps {
        let idx = data.find_index_by_time(timestamp)?;
        Some(ctx.camera.time_to_x(idx as f64) + ctx.camera.pixels_per_unit_x() * 0.5)
    } else {
        Some(ctx.camera.time_to_x(timestamp as f64))
    }
}
```

---

## 3. Phase 2: Scene Upload (Conditional GPU Work)

### 3.1 Dirty Flag Tiers for Widgets

`DirtyFlags` gains a `widgets: u64` generation counter, incremented
when any annotation is created, modified, deleted, or changes presence.

| Tier | Trigger | GPU Work | Cost |
|------|---------|----------|------|
| 0 | No generation changed | Zero | 1 u64 comparison |
| 1 | Widget gen changed, camera same | Rebuild widget buffers only | ~16 KB upload |
| 2 | Camera changed (pan/zoom) | Recompute + rebuild all widgets | compute + upload |
| 3 | Theme changed | Recompute colors + rebuild | compute + upload |

```rust
impl DirtyTracker {
    pub fn needs_widget_rebuild(&self, current: &DirtyFlags) -> bool {
        self.last_seen.widgets != current.widgets
    }

    pub fn needs_widget_recompute(&self, current: &DirtyFlags) -> bool {
        self.last_seen.camera != current.camera
            || self.last_seen.widgets != current.widgets
            || self.last_seen.theme != current.theme
    }
}
```

**Why widgets always recompute on camera change**: Widget positions
are baked into pixel coordinates during compute. Camera pan/zoom
invalidates every price-to-pixel and time-to-pixel mapping. This
matches existing behavior for grid lines, levels, and the crosshair.

**Initial strategy: always-upload.** Like the crosshair and volume
profile pipelines, widget buffers are small enough (<20 KB typical)
that precise dirty tracking is not worth the complexity. Upload every
frame that has widgets. Gate behind `needs_widget_rebuild()` only
if profiling reveals a bottleneck (unlikely before 1000+ annotations).

### 3.2 Buffer Management

Three dedicated instance buffers for widgets, separate from existing
candle/volume/grid buffers:

```rust
/// Per-chart GPU resources for widget rendering.
/// Stored inside ChartGpuResources.
pub struct WidgetGpuBuffers {
    /// Instance buffer for widget fills (GridLineInstance data).
    pub fill_buffer: GrowableBuffer,
    /// Number of fill instances uploaded.
    pub fill_count: u32,

    /// Instance buffer for widget lines (GridLineInstance data).
    pub line_buffer: GrowableBuffer,
    /// Number of line instances uploaded.
    pub line_count: u32,

    /// Instance buffer for widget markers (GridLineInstance data now,
    /// MarkerInstance data when marker pipeline is added).
    pub marker_buffer: GrowableBuffer,
    /// Number of marker instances uploaded.
    pub marker_count: u32,
}
```

**Buffer sizing strategy**:

| Buffer  | Initial Capacity | Growth Strategy       | Max Expected |
|---------|------------------|-----------------------|--------------|
| Fills   | 64 instances     | Double when exceeded  | ~200         |
| Lines   | 128 instances    | Double when exceeded  | ~500         |
| Markers | 32 instances     | Double when exceeded  | ~100         |

The `GrowableBuffer` type (already in the codebase) handles growth:
if the new data exceeds current capacity, it allocates a new buffer
at 2x the needed size. Otherwise it calls `queue.write_buffer()` into
the existing allocation.

**Why separate buffers, not one big buffer?** Each layer draws at a
different point in the render pass. Fills draw before candles, lines
draw after candles, markers draw after lines. If all widgets shared
one buffer, we would need to sort instances by layer and track
sub-ranges for each draw call. Three buffers is simpler: each draw
call binds its buffer and draws all instances in it.

**Memory overhead**: 7 KB per chart, 140 KB for 20 charts. Negligible.

### 3.3 Integration with Existing Pipeline prepare()

The widget buffers are uploaded in the existing `render_prepare()`
flow, alongside candle/volume/grid buffers:

The changes to `render_prepare()`:

1. **Camera update**: Add `update_projection()` calls for the three
   widget pipelines alongside the existing five pipelines.

2. **Instance upload**: After the existing crosshair upload, add:
   ```rust
   // Always-upload strategy: widget buffers are small.
   self.widget_fill_pipeline.update_instances(device, queue, &scene.widget_scene.fills);
   self.widget_line_pipeline.update_instances(device, queue, &scene.widget_scene.lines);
   self.widget_marker_pipeline.update_instances(device, queue, &scene.widget_scene.markers);
   ```

**No new shader compilation.** All three widget pipelines are
`GridPipeline` instances -- they reuse the exact same `grid.wgsl`
shader. The only difference is which instance buffer is bound at
draw time.

---

## 4. Phase 3: GPU Dispatch (Fixed Order)

### 4.1 Complete Render Order

All draws happen in a single render pass per chart. The fixed
back-to-front order ensures correct visual layering:

```
Pass  Layer  Pipeline         Buffer                   What
────  ─────  ───────────────  ───────────────────────  ─────────────────────────
  1     0    (clear)          n/a                      Background color
  2     1    grid_pipeline    grid_instance_buf        Price/time grid lines
  3     2    volume_pipeline  volume_instance_buf      Volume bars
  4     3    vol_prof_pipe    volume_profile_buf       Volume Profile histogram
  5     4    widget_fill_pipe widget_fill_buf   ←NEW   Bracket zones, note bgs
  6     5    candle_pipeline  candle_instance_buf      Candle wicks (pass 1)
  7     6    candle_pipeline  candle_instance_buf      Candle bodies (pass 2)
  8     7    widget_line_pipe widget_line_buf   ←NEW   Levels, bracket legs
  9     8    widget_mark_pipe widget_marker_buf ←NEW   Markers (squares for now)
 10     9    (future)         indicator_buf            Indicator overlays
 11    10    crosshair_pipe   crosshair_buf            Crosshair lines
 12    11    (iced overlay)   n/a                      Labels, tooltips, menus
```

**Visual rationale**: Fills behind candles (zone fills should not
obscure price data). Lines above candles (reference marks must be
visible). Markers above lines (small, high-importance). Crosshair
always topmost (live cursor tracking).

```rust
impl ChartRenderer {
    pub fn draw_pass<'a>(&'a self, render_pass: &mut wgpu::RenderPass<'a>) {
        // Layer 1: Grid
        self.grid_pipeline.draw(render_pass);

        // Layer 2-3: Volume + Volume Profile
        self.volume_pipeline.draw(render_pass);
        self.volume_profile_pipeline.draw(render_pass);

        // Layer 4: Widget fills (behind candles)
        self.widget_fill_pipeline.draw(render_pass);

        // Layer 5-6: Candle wicks + bodies
        self.candle_pipeline.draw_wicks(render_pass);
        self.candle_pipeline.draw_bodies(render_pass);

        // Layer 7: Widget lines (above candles)
        self.widget_line_pipeline.draw(render_pass);

        // Layer 8: Widget markers (above lines)
        self.widget_marker_pipeline.draw(render_pass);

        // Layer 9: (future) Indicator overlays

        // Layer 10: Crosshair
        self.crosshair_pipeline.draw(render_pass);
    }
}
```

**Total draw calls per chart**: 10 (up from 7). Additional CPU
overhead: ~6 microseconds total. Imperceptible.

### 4.2 Pipeline Reuse

The widget pipelines reuse the existing grid render pipeline. The
grid shader renders axis-aligned rectangles defined by
`[left, top, right, bottom]` with RGBA color and alpha blending --
exactly what level lines, bracket legs, zone fills, note backgrounds,
and dashed line segments need.

`ChartRenderer::new()` adds three `GridPipeline::new(device, format)`
calls for `widget_fill_pipeline`, `widget_line_pipeline`, and
`widget_marker_pipeline`. Same shader, same vertex layout, different
instance buffers.

**Why three separate GridPipeline instances instead of sharing one?**
Each `GridPipeline` owns its own instance buffer and count. They draw
at different points in the render pass (fills before candles, lines
after candles). Sharing one pipeline would require sub-range tracking
for zero performance gain. The pipeline object is ~200 bytes of state;
the compiled `wgpu::RenderPipeline` (shader) is shared by reference.

### 4.3 Future: MarkerInstance Pipeline

When the set of marker shapes needed exceeds what small squares can
approximate, a dedicated MarkerPipeline with an SDF fragment shader
will be added. This is **not needed for v1** -- small colored squares
at 6-8 pixel diameter are sufficient for fill event markers and
signal indicators.

The SDF approach: a fragment shader evaluates circle/diamond/triangle
SDFs and applies `smoothstep` anti-aliasing. Each `MarkerInstance`
specifies center, size, shape index, fill color, and border color.
The vertex shader expands a unit quad around the center.

**Trigger for adding this pipeline**: When any marker shapes beyond
small squares are needed (circles, diamonds, triangles, arrows).
Estimated timeline: Phase 2 or Phase 3 of the widget system.

### 4.4 Future: LineInstance Pipeline

Diagonal lines (trendlines, Fibonacci retracements, measured moves)
cannot be rendered with axis-aligned rectangles. They need a dedicated
pipeline that computes perpendicular thickness from two endpoints.

```rust
/// GPU instance data for a diagonal line segment.
/// Size: 48 bytes per instance.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct LineInstance {
    /// Start point in pixel coordinates.
    pub start: [f32; 2],
    /// End point in pixel coordinates.
    pub end: [f32; 2],
    /// Line thickness in pixels.
    pub thickness: f32,
    /// Padding.
    pub _pad: [f32; 3],
    /// RGBA color.
    pub color: [f32; 4],
}
```

The vertex shader takes a unit quad and transforms it into a
screen-space rectangle oriented along the line direction: compute
`dir = normalize(end - start)`, `perp = vec2(-dir.y, dir.x)`, then
expand `quad_pos.x` along `dir` and `quad_pos.y` along `perp * thickness`.

**Trigger for adding this pipeline**: When trendlines or diagonal
drawing tools are implemented. Estimated timeline: Phase 3 or later.

**Not needed for v1** because all current annotation types use
horizontal lines (levels, brackets), zone fills (brackets), or
point markers -- all of which are axis-aligned rectangles.

---

## 5. Render Primitive Vocabulary

### 5.1 GridLineInstance (Existing, Reused)

The universal primitive. Covers approximately 90% of all visual
elements in the widget system.

```rust
/// GPU instance data for a single axis-aligned filled rectangle.
///
/// Size: 32 bytes per instance (8 floats).
/// Alignment: 4 bytes (f32 natural alignment).
///
/// The grid shader expands a unit quad [0,1]x[0,1] to fill the
/// rectangle defined by [left, top, right, bottom] in pixel space,
/// then applies the orthographic projection to NDC.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GridLineInstance {
    /// Rectangle bounds in pixel coordinates: [left, top, right, bottom].
    ///
    /// Coordinate system: origin at top-left, Y increases downward.
    /// - left < right (horizontal extent)
    /// - top < bottom (vertical extent, note: top is numerically smaller)
    pub rect: [f32; 4],

    /// RGBA color in linear space (NOT sRGB).
    ///
    /// Alpha: 1.0 = fully opaque, 0.0 = fully transparent.
    /// The grid pipeline uses standard alpha blending:
    ///   src_factor: SrcAlpha, dst_factor: OneMinusSrcAlpha
    pub color: [f32; 4],
}
```

**What it covers:**

| Widget Element           | rect Configuration                            | color Example           |
|--------------------------|-----------------------------------------------|-------------------------|
| Full-width level line    | `[0, y-0.5, vw, y+0.5]`                      | `[1, 0.8, 0, 0.7]`     |
| Bracket entry leg        | `[start_x, y-1, vw, y+1]`                    | `[0, 0.8, 1, 0.9]`     |
| Bracket TP leg           | `[start_x, y-0.5, vw, y+0.5]`                | `[0.2, 0.9, 0.2, 0.7]` |
| Bracket SL leg           | `[start_x, y-0.5, vw, y+0.5]`                | `[0.9, 0.2, 0.2, 0.7]` |
| Bracket TP zone fill     | `[start_x, entry_y, vw, tp_y]`               | `[0, 1, 0, 0.06]`      |
| Bracket SL zone fill     | `[start_x, sl_y, vw, entry_y]`               | `[1, 0, 0, 0.06]`      |
| Note background          | `[x, y, x+w, y+h]`                           | `[0.15, 0.15, 0.2, 0.85]` |
| Volume profile bar       | `[0, row_top, bar_width, row_bottom]`         | `[0.3, 0.5, 0.8, 0.3]` |
| Alert line               | `[0, y-0.5, vw, y+0.5]`                      | `[1, 1, 0, 0.5]`       |
| Dashed line segment      | `[x, y-0.5, x+dash, y+0.5]`                  | `[0.5, 0.5, 0.5, 0.6]` |
| Selection highlight      | `[left-2, top-2, right+2, bottom+2]`          | `[1, 1, 1, 0.2]`       |
| Drag handle              | `[x-3, y-3, x+3, y+3]`                       | `[1, 1, 1, 0.8]`       |
| Marker (square approx)   | `[cx-3, cy-3, cx+3, cy+3]`                   | `[0, 0.8, 0, 1.0]`     |

### 5.2 MarkerInstance (Future)

For SDF-rendered markers with shape variety and anti-aliased edges.
Not needed for v1.

```rust
/// GPU instance data for an SDF-rendered marker.
///
/// Size: 48 bytes per instance (12 floats).
/// Alignment: 4 bytes (f32 natural alignment).
///
/// The marker shader expands a unit quad centered on `center`,
/// then evaluates an SDF in the fragment shader based on `shape`.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MarkerInstance {
    /// Center position in pixel coordinates.
    pub center: [f32; 2],
    /// Diameter in logical pixels.
    pub size: f32,
    /// Shape index: 0=circle, 1=diamond, 2=triangle_up,
    /// 3=triangle_down, 4=square, 5=cross, 6=star.
    pub shape: u32,
    /// Fill color (RGBA linear).
    pub color: [f32; 4],
    /// Border color (RGBA linear). Set to transparent for no border.
    pub border_color: [f32; 4],
}

#[cfg(test)]
#[test]
fn marker_instance_size_is_48_bytes() {
    assert_eq!(
        std::mem::size_of::<MarkerInstance>(),
        48,
        "MarkerInstance must be exactly 48 bytes for GPU layout"
    );
}
```

### 5.3 What Each Widget Type Produces

Complete mapping from annotation kind to render primitives:

```
AnnotationKind::Level
├── lines[]: 1 GridLineInstance (the level line itself)
├── lines[]: 0-1 GridLineInstance (selection highlight, if selected)
├── lines[]: 0-2 GridLineInstance (drag handles, if selected)
├── fills[]: 0 (levels have no fill)
├── markers[]: 0 (levels have no markers)
├── labels[]: 1 WidgetLabel (price badge on Y axis)
└── hit_zones[]: 1 HitZone (line body for click/drag)

AnnotationKind::OrderBracket
├── lines[]: 1-3 GridLineInstance (entry + optional TP + optional SL legs)
├── lines[]: 0-3 GridLineInstance (selection highlights, if selected)
├── lines[]: 0-6 GridLineInstance (drag handles per leg, if selected)
├── fills[]: 0-2 GridLineInstance (TP zone fill + SL zone fill)
├── markers[]: 0 (brackets have no markers)
├── labels[]: 1-3 WidgetLabel (price badges per leg)
├── labels[]: 0-1 WidgetLabel (R:R ratio label)
└── hit_zones[]: 1-3 HitZone (one per leg for drag)

AnnotationKind::Marker
├── lines[]: 0 (markers have no lines)
├── fills[]: 0 (markers have no fills)
├── markers[]: 1 GridLineInstance (small square, v1)
│   └── (future: 1 MarkerInstance with SDF shape)
├── labels[]: 0-1 WidgetLabel (tooltip, if configured)
└── hit_zones[]: 1 HitZone (marker body for click)

AnnotationKind::TextNote
├── lines[]: 0 (notes have no lines)
├── fills[]: 1 GridLineInstance (background rectangle)
├── markers[]: 0 (notes have no markers)
├── labels[]: 1 WidgetLabel (note text content)
└── hit_zones[]: 1 HitZone (note body for click/drag)
```

**Instance count formula per annotation type:**

| Kind    | fills    | lines       | markers | Total Instances |
|---------|----------|-------------|---------|-----------------|
| Level   | 0        | 1-3         | 0       | 1-3             |
| Bracket | 0-2      | 1-12        | 0       | 1-14            |
| Marker  | 0        | 0           | 1       | 1               |
| Note    | 1        | 0           | 0       | 1               |

Bracket worst case (1-14): selected, all three legs with highlights
and drag handles, plus two zone fills. Typical: 5-6 instances.

---

## 6. Text and Label Rendering

### 6.1 Current Approach

All text in Hand of Midas renders through iced overlay widgets, not
GPU text shaders. This includes:

- X-axis date labels
- Y-axis price labels
- Crosshair axis badges
- OHLCV data overlay (top-left corner)
- Level price badges

This works well for <100 labels per chart. iced uses cosmic-text
with HarfBuzz shaping -- high quality, no atlas management needed.

### 6.2 GPU Text (Future)

If label count exceeds ~200 per chart or overlay rendering becomes a
bottleneck (>1ms), switch to MSDF font atlas rendering. The MSDF
infrastructure is already designed in the existing GPU architecture
document (`msdf_atlas` in `SharedPipelines`, `text_pipeline`,
`GlyphInstance` at 48 bytes). Not needed before Phase 3.

### 6.3 Widget Labels

Widget labels integrate with the existing iced overlay system. The
`WidgetScene.labels` vector is consumed by the same code path that
builds level price badges and crosshair axis labels.

The integration point: `build_overlays()` in `chart_widget.rs`
iterates `scene.widget_scene.labels` and creates `OverlayElement::WidgetLabel`
entries alongside existing axis labels and crosshair badges. No new
overlay rendering infrastructure is needed.

**Label positioning**: Y-axis price badges use `anchor = Right` at
`x = viewport_width - axis_width`. Bracket R:R ratio labels use
`anchor = Left` at the midpoint between entry and TP, inside the
chart area.

---

## 7. Performance Budget

### 7.1 Instance Count Estimates

Typical trading scenario: active day trader with 20 charts, each
showing the same symbol at different timeframes.

**Per-chart instance breakdown (typical):**

| Widget Type      | Typical Count | Instances Per Widget | Total Instances |
|------------------|---------------|----------------------|-----------------|
| Levels           | 20            | 1-2                  | 30              |
| Order Brackets   | 5             | 5-6                  | 28              |
| Volume Profile   | 1             | 50-200               | 150             |
| Markers          | 10            | 1                    | 10              |
| Notes            | 3             | 1                    | 3               |
| **Total**        | **39**        |                      | **~221**        |

**Per-chart instance breakdown (heavy):**

| Widget Type      | Heavy Count | Instances Per Widget | Total Instances |
|------------------|-------------|----------------------|-----------------|
| Levels           | 50          | 2-3                  | 125             |
| Order Brackets   | 15          | 6                    | 90              |
| Volume Profile   | 1           | 200                  | 200             |
| Markers          | 30          | 1                    | 30              |
| Notes            | 10          | 1                    | 10              |
| Indicators       | 5           | 50-200               | 500             |
| **Total**        | **111**     |                      | **~955**        |

For reference, existing candle/volume buffers are ~400 KB per chart.
Even the "heavy" widget scenario at ~955 instances is 30.5 KB -- a
rounding error.

### 7.2 Frame Budget

**Target**: Widget compute + upload < 0.5ms per chart.

**Per-phase cost estimate** (per annotation: ~400ns compute, ~50ns
merge; per write_buffer: ~1 us per 1 KB; per draw call: ~2 us CPU):

**Total per chart:**

| Scenario | Compute | Upload | Draw | Total    |
|----------|---------|--------|------|----------|
| Typical  | 20 us   | 6 us   | 6 us | **32 us** |
| Heavy    | 75 us   | 14 us  | 6 us | **95 us** |

**For 20 charts**: 640 us typical, 1,900 us heavy. The 20-chart
total budget is ~10ms (14ms frame budget minus ~4ms existing pipeline).
Both scenarios are comfortably within budget.

**Mitigation for heavy scenarios:**

1. **Tier 0 short-circuit**: Static chart with no interaction = zero
   widget cost (most common case).
2. **Visible-range culling**: Skip annotations outside the viewport.
3. **Instance budget cap**: Cull least-important annotations if total
   exceeds 2000 instances (safety valve, not expected to trigger).
4. **Parallel compute** (future): Rayon over annotations if count
   exceeds 200 per chart.

### GPU Memory Summary (20 Charts)

| Resource                | Per Chart | 20 Charts |
|-------------------------|-----------|-----------|
| Widget fill buffer      | 2 KB      | 40 KB     |
| Widget line buffer      | 4 KB      | 80 KB     |
| Widget marker buffer    | 1 KB      | 20 KB     |
| Camera UBO (shared)     | 80 bytes  | 1.6 KB    |
| **Widget GPU total**    | **~7 KB** | **~142 KB** |
| Existing GPU total      | ~830 KB   | ~17 MB    |
| **Percentage increase** |           | **< 1%**  |

Widget rendering adds less than 1% to existing GPU memory usage.
It is, by every measure, trivial for the GPU.

---

## Appendix: Migration Path

The widget rendering pipeline is additive -- it does not modify any
existing rendering code. The migration is:

```
Step 1: Add WidgetScene struct and compute_widget_scene() to midas-chart.
        No GPU changes. Unit test the compute output.

Step 2: Add widget_scene field to ChartScene.
        Initialize as WidgetScene::new() (empty).
        No visual change.

Step 3: Add three GridPipeline instances to ChartRenderer.
        Wire up prepare() and draw_pass().
        Empty buffers = zero draw calls. No visual change.

Step 4: Wire compute_widget_scene() into compute_chart_scene().
        Existing levels migrate to annotation-based compute.
        Visual output should be identical to current levels.

Step 5: Add bracket, marker, note compute implementations.
        New visual elements appear.
```

Each step is independently testable and shippable. No step breaks
existing functionality.
