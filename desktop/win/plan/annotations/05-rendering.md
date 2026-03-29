# 05 — Rendering

## Layer Order

Annotations render as a dedicated layer between candle bodies and the crosshair:

```
Layer 1: Grid lines (horizontal price, vertical time, separator)
Layer 2: Volume bars
Layer 3: Volume Profile histogram
Layer 4: Candle wicks
Layer 5: Candle bodies
Layer 6: ★ Annotation fills (bracket zones — behind lines)
Layer 7: ★ Annotation lines (levels, bracket legs — on top of fills)
Layer 8: ★ Annotation markers (icons, points — on top of lines)
Layer 9: Crosshair (always topmost GPU layer)
Layer 10: iced overlay (labels, tooltips, context menus — above GPU)
```

Annotations are split into sub-layers so zone fills don't obscure lines,
and markers (small, important) are never hidden behind lines.

## GPU Pipeline Reuse

No new shaders needed. All annotation rendering uses existing pipelines:

| Annotation Element | Pipeline | Instance Type |
|---|---|---|
| Level lines | GridPipeline | `GridLineInstance` (thin rect) |
| Bracket leg lines | GridPipeline | `GridLineInstance` |
| Bracket zone fills | GridPipeline | `GridLineInstance` (wide rect, low α) |
| Bracket preview ghost | GridPipeline | `GridLineInstance` (dashed via α) |
| Marker circles | GridPipeline | `GridLineInstance` (small square + shader) |
| Note backgrounds | GridPipeline | `GridLineInstance` (rect behind text) |

The `GridPipeline` renders axis-aligned rectangles with color — exactly what we need
for horizontal/vertical lines, zone fills, and background rects.

For markers that need non-rectangular shapes (circles, triangles), we have two options:
1. **Approximate with small rects** (like the VP POC circle — stacked scanlines)
2. **Add a MarkerPipeline** later with a simple SDF shader

Start with option 1 (no new shader), upgrade to option 2 when marker variety demands it.

## AnnotationRender

The compute pipeline produces `AnnotationRender` variants that map directly to GPU instances:

```rust
/// GPU-ready render data for a single annotation.
/// Produced by compute_annotations(), consumed by the renderer.
pub enum AnnotationRender {
    /// A horizontal line (level or bracket leg).
    Line {
        /// GridLineInstance for the line itself.
        line: GridLineInstance,
        /// Whether this annotation is currently selected.
        selected: bool,
        /// Optional selection highlight (slightly thicker, brighter).
        selection_highlight: Option<GridLineInstance>,
    },

    /// A bracket (entry + optional TP/SL + zone fills).
    Bracket {
        /// Lines for each leg (1-3 lines).
        lines: Vec<GridLineInstance>,
        /// Zone fill rects (0-2 rects: TP zone and/or SL zone).
        fills: Vec<GridLineInstance>,
        /// Whether this bracket is selected.
        selected: bool,
    },

    /// A point marker (icon at a specific location).
    Marker {
        /// Small rect(s) approximating the icon shape.
        instances: Vec<GridLineInstance>,
    },

    /// A text note (background rect only — text is iced overlay).
    Note {
        /// Background rect for the note.
        background: GridLineInstance,
        /// Screen position for iced text overlay.
        text_x: f32,
        text_y: f32,
        text: String,
    },
}
```

### Flattening for GPU Upload

The renderer flattens all `AnnotationRender` variants into three buffers:

```rust
struct AnnotationBuffers {
    fills: Vec<GridLineInstance>,    // zone fills → Layer 6
    lines: Vec<GridLineInstance>,    // lines → Layer 7
    markers: Vec<GridLineInstance>,  // markers → Layer 8
}

fn flatten_annotations(renders: &[AnnotationRender]) -> AnnotationBuffers {
    let mut buffers = AnnotationBuffers::default();
    for render in renders {
        match render {
            AnnotationRender::Line { line, selection_highlight, .. } => {
                buffers.lines.push(*line);
                if let Some(highlight) = selection_highlight {
                    buffers.lines.push(*highlight);
                }
            }
            AnnotationRender::Bracket { lines, fills, .. } => {
                buffers.fills.extend_from_slice(fills);
                buffers.lines.extend_from_slice(lines);
            }
            AnnotationRender::Marker { instances } => {
                buffers.markers.extend_from_slice(instances);
            }
            AnnotationRender::Note { background, .. } => {
                buffers.fills.push(*background);
            }
        }
    }
    buffers
}
```

### Renderer Changes

```rust
pub struct ChartRenderer {
    // ─── Existing pipelines ────────────
    candle_pipeline: CandlePipeline,
    volume_pipeline: VolumePipeline,
    grid_pipeline: GridPipeline,
    volume_profile_pipeline: GridPipeline,
    crosshair_pipeline: GridPipeline,

    // ─── New pipelines (all reuse GridPipeline) ────────────
    annotation_fill_pipeline: GridPipeline,    // Layer 6: zone fills
    annotation_line_pipeline: GridPipeline,    // Layer 7: lines
    annotation_marker_pipeline: GridPipeline,  // Layer 8: markers
}
```

Three separate `GridPipeline` instances for annotations so each sub-layer
gets its own instance buffer and draws at the correct z-order.

## Selection Highlight

When an annotation is selected, it renders with:
1. A slightly thicker line (line_width + 2px) at the selection color
2. The original line on top, creating a "glow" effect
3. Small drag handles at line endpoints (6×6px squares)

```rust
fn selection_highlight(line: &GridLineInstance, extra_px: f32) -> GridLineInstance {
    let mut highlight = *line;
    // Expand rect by extra_px in each direction
    highlight.rect[0] -= extra_px; // left
    highlight.rect[1] -= extra_px; // top
    highlight.rect[2] += extra_px; // right
    highlight.rect[3] += extra_px; // bottom
    // Brighten color
    highlight.color = [
        (line.color[0] + 0.3).min(1.0),
        (line.color[1] + 0.3).min(1.0),
        (line.color[2] + 0.3).min(1.0),
        0.4,
    ];
    highlight
}
```

## Dashed Lines

For draft/pending bracket legs that need dashed rendering, we approximate with
multiple short `GridLineInstance` segments:

```rust
fn dashed_line(
    y: f32,
    x_start: f32,
    x_end: f32,
    dash_len: f32,
    gap_len: f32,
    thickness: f32,
    color: [f32; 4],
) -> Vec<GridLineInstance> {
    let mut segments = Vec::new();
    let mut x = x_start;
    while x < x_end {
        let seg_end = (x + dash_len).min(x_end);
        segments.push(GridLineInstance {
            rect: [x, y - thickness * 0.5, seg_end, y + thickness * 0.5],
            color,
        });
        x += dash_len + gap_len;
    }
    segments
}
```

This is slightly memory-heavy for very long dashed lines across the viewport,
but at typical dash/gap ratios (8px/4px), a 1280px viewport produces ~107 segments
= ~3.4 KB. Negligible.

## Text Labels (iced Overlay)

Annotation text (bracket labels, note text, marker tooltips) renders as iced overlay
widgets, following the same pattern as date labels and Y-axis labels:

```rust
// In chart_widget.rs, during overlay construction:
fn build_annotation_labels(
    annotations: &[AnnotationRender],
    viewport_width: u32,
    viewport_height: u32,
) -> Vec<OverlayLabel> {
    let mut labels = Vec::new();
    for render in annotations {
        match render {
            AnnotationRender::Line { line, .. } => {
                // Price badge on Y axis
                // ...
            }
            AnnotationRender::Bracket { .. } => {
                // Entry/TP/SL price badges + R:R ratio label
                // ...
            }
            AnnotationRender::Note { text_x, text_y, text, .. } => {
                labels.push(OverlayLabel {
                    x: *text_x,
                    y: *text_y,
                    text: text.clone(),
                    // ...
                });
            }
            _ => {}
        }
    }
    labels
}
```

## Dirty Tracking

Add `annotations: u64` to `DirtyFlags`:

```rust
pub struct DirtyFlags {
    // existing...
    pub camera: u64,
    pub candles: u64,
    pub indicators: u64,
    pub crosshair: u64,
    pub levels: u64,
    pub grid: u64,
    pub theme: u64,
    // new:
    pub annotations: u64,
}
```

Annotation pipelines always-upload (like crosshair and VP) since the instance
count is small and the dirty-flag timing issues we solved for grid apply here too.

When annotations stabilize and we want to optimize, gate uploads behind
`tracker.needs_annotation_rebuild()`. But start with always-upload.

## Performance Budget

| Metric | Budget | Rationale |
|---|---|---|
| Max annotations per chart | 500 | Plenty for manual trading |
| Max GridLineInstances for annotations | ~2000 | 500 annotations × ~4 instances avg |
| GPU upload per frame | ~64 KB | 2000 × 32 bytes = 64 KB, negligible |
| Compute time for annotations | < 0.5ms | Simple coordinate transforms |
| Memory per annotation (Rust) | < 256 bytes | Small structs, few allocations |
