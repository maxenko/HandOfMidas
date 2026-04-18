//! GPU text pipeline using cryoglyph (iced 0.14's glyphon fork).
//!
//! Renders decorator labels (level price text, bracket segment text,
//! etc.) directly on the GPU, in the same render pass as the SDF
//! [`BadgePipeline`]. Labels are split into `ANNOTATION_LAYER_COUNT`
//! sub-groups matching the compute pipeline's z-layers (background /
//! proximity-promoted / hovered / dragged); the renderer interleaves
//! per-layer `BadgePipeline::draw_range` + `TextPipeline::draw_layer`
//! calls so each layer's shapes and text composite over lower layers'
//! shapes and text as one unit.
//!
//! ## Why one `TextRenderer` per layer
//!
//! cryoglyph's `TextRenderer::prepare` overwrites its internal vertex
//! buffer on every call. A single renderer therefore can only carry
//! one batch of text at a time. To get N independent text batches in
//! one frame we need N renderers — they share the heavy resources
//! (font system, swash cache, texture atlas, viewport uniform).

use cryoglyph::cosmic_text::Align;
use cryoglyph::{
    Attrs, Buffer, Cache, Color, Family, FontSystem, Metrics, Resolution, Shaping, SwashCache,
    TextArea, TextAtlas, TextBounds, TextRenderer, Viewport,
};
use midas_chart::compute::{LayerEnd, ANNOTATION_LAYER_COUNT};
use midas_chart::widget::compute::{LabelAnchor, WidgetLabel};
use wgpu::{
    CommandEncoderDescriptor, Device, MultisampleState, Queue, RenderPass, TextureFormat,
};

/// Line-height multiplier applied on top of `font_size` when building
/// cosmic-text [`Metrics`]. 1.2× covers typical ascender/descender
/// metrics for sans-serif fonts so labels don't clip vertically.
const LINE_HEIGHT_FACTOR: f32 = 1.2;

/// Pipeline that draws text via cryoglyph on the chart render pass.
pub struct TextPipeline {
    font_system: FontSystem,
    swash_cache: SwashCache,
    atlas: TextAtlas,
    viewport: Viewport,
    /// One renderer per z-layer. See the module doc for why.
    renderers: [TextRenderer; ANNOTATION_LAYER_COUNT],
    /// Per-layer buffer cache — rebuilt each frame. Held on the
    /// pipeline so the borrows handed to `TextArea` outlive the
    /// `prepare` call.
    buffers: [Vec<Buffer>; ANNOTATION_LAYER_COUNT],
    /// Index-parallel to `buffers`: resolved position + colour for
    /// each label, used to build `TextArea`s at prepare time.
    placements: [Vec<LabelPlacement>; ANNOTATION_LAYER_COUNT],
    /// Dedicated renderer for axis text (priceline numbers). Drawn
    /// BEFORE the annotation layers so priceline labels always sit
    /// behind decorators and indicators.
    axis_renderer: TextRenderer,
    axis_buffers: Vec<Buffer>,
    axis_placements: Vec<LabelPlacement>,
}

/// Resolved per-label layout — computed once in `prepare` from the
/// label's anchor and measured text extent, stored so `TextArea`s
/// can be rebuilt on demand without re-measuring.
#[derive(Clone, Copy)]
struct LabelPlacement {
    left: f32,
    top: f32,
    color: Color,
}

impl TextPipeline {
    /// Allocate the pipeline against `format` (must match the
    /// swapchain format the chart renders into). `queue` is required
    /// because the initial atlas texture upload is queued immediately.
    pub fn new(device: &Device, queue: &Queue, format: TextureFormat) -> Self {
        // `Cache` memoises the atlas's wgpu pipeline/bind-group-layout
        // pair; only needed during construction, safely dropped after.
        let cache = Cache::new(device);
        let mut atlas = TextAtlas::new(device, queue, &cache, format);
        let viewport = Viewport::new(device, &cache);

        // Allocate N renderers up-front. Cheap — each wraps a vertex
        // buffer + a staging belt; they all share the atlas's pipeline.
        let renderers: [TextRenderer; ANNOTATION_LAYER_COUNT] =
            std::array::from_fn(|_| {
                TextRenderer::new(&mut atlas, device, MultisampleState::default(), None)
            });
        let axis_renderer =
            TextRenderer::new(&mut atlas, device, MultisampleState::default(), None);

        // `FontSystem::new` populates the database with system fonts.
        // On Windows that picks up Segoe UI Variable / Segoe UI, which
        // matches what iced uses by default.
        let font_system = FontSystem::new();
        let swash_cache = SwashCache::new();

        Self {
            font_system,
            swash_cache,
            atlas,
            viewport,
            renderers,
            buffers: std::array::from_fn(|_| Vec::new()),
            placements: std::array::from_fn(|_| Vec::new()),
            axis_renderer,
            axis_buffers: Vec::new(),
            axis_placements: Vec::new(),
        }
    }

    /// Notify the pipeline of the current viewport size (logical
    /// pixels). Cheap; safe to call every frame.
    pub fn update_viewport(&mut self, queue: &Queue, width: u32, height: u32) {
        self.viewport.update(
            queue,
            Resolution {
                width: width.max(1),
                height: height.max(1),
            },
        );
    }

    /// Shape each label (both annotation + axis), split annotation
    /// labels by `layer_ends`, and hand each per-layer `TextArea`
    /// list to the matching `TextRenderer`. The dedicated axis
    /// renderer picks up `axis_labels` verbatim.
    ///
    /// cryoglyph requires a `CommandEncoder`; `prepare` creates and
    /// submits its own one-shot encoder so callers on iced's
    /// `Primitive::prepare` path (which doesn't expose an encoder)
    /// can drive text upload without extra plumbing.
    // 8 parameters is over the clippy default — justified here because
    // grouping them into a struct would make every caller do a local
    // construction-move dance for what is genuinely one ordered list
    // of render-pass inputs (device/queue/viewport/layers/axis).
    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        &mut self,
        device: &Device,
        queue: &Queue,
        viewport_width: u32,
        viewport_height: u32,
        labels: &[WidgetLabel],
        layer_ends: [LayerEnd; ANNOTATION_LAYER_COUNT],
        axis_labels: &[WidgetLabel],
    ) {
        self.update_viewport(queue, viewport_width, viewport_height);

        // Rebuild per-layer buffer + placement caches from scratch.
        for slot in &mut self.buffers {
            slot.clear();
        }
        for slot in &mut self.placements {
            slot.clear();
        }
        self.axis_buffers.clear();
        self.axis_placements.clear();

        // Walk the label slice layer-by-layer. Each layer owns
        // `labels[prev..layer_ends[k].label_end]`. Labels outside any
        // recorded span (shouldn't happen in normal flow) are silently
        // dropped — the renderer only draws what the compute pass
        // explicitly classified.
        let mut cursor = 0_usize;
        for (layer_idx, end) in layer_ends.iter().enumerate() {
            let layer_slice = &labels[cursor..end.label_end.min(labels.len())];
            for label in layer_slice {
                let (buffer, placement) = shape_label(
                    label,
                    &mut self.font_system,
                );
                self.buffers[layer_idx].push(buffer);
                self.placements[layer_idx].push(placement);
            }
            cursor = end.label_end.min(labels.len());
        }
        for label in axis_labels {
            let (buffer, placement) = shape_label(label, &mut self.font_system);
            self.axis_buffers.push(buffer);
            self.axis_placements.push(placement);
        }

        // One encoder covers every renderer's upload. Cheaper than
        // submitting per-renderer — cryoglyph's staging belt is per
        // renderer, so encoding is local but the submit is shared.
        let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("text_pipeline_prepare"),
        });

        let vp_w = viewport_width.max(1) as i32;
        let vp_h = viewport_height.max(1) as i32;
        let bounds = TextBounds {
            left: 0,
            top: 0,
            right: vp_w,
            bottom: vp_h,
        };

        for layer_idx in 0..ANNOTATION_LAYER_COUNT {
            // Indexing the two arrays separately avoids a compound
            // `&mut self.renderers[..]` + `&self.buffers[..]` borrow.
            let renderer = &mut self.renderers[layer_idx];
            let areas = self.buffers[layer_idx]
                .iter()
                .zip(self.placements[layer_idx].iter())
                .map(|(buf, p)| make_area(buf, *p, bounds));
            if let Err(err) = renderer.prepare(
                device,
                queue,
                &mut encoder,
                &mut self.font_system,
                &mut self.atlas,
                &self.viewport,
                areas,
                &mut self.swash_cache,
            ) {
                tracing::warn!(
                    layer = layer_idx,
                    ?err,
                    "cryoglyph prepare failed — skipping layer",
                );
            }
        }

        // Axis labels: one batch, prepared on the dedicated axis
        // renderer, drawn before any annotation layer.
        let axis_areas = self
            .axis_buffers
            .iter()
            .zip(self.axis_placements.iter())
            .map(|(buf, p)| make_area(buf, *p, bounds));
        if let Err(err) = self.axis_renderer.prepare(
            device,
            queue,
            &mut encoder,
            &mut self.font_system,
            &mut self.atlas,
            &self.viewport,
            axis_areas,
            &mut self.swash_cache,
        ) {
            tracing::warn!(?err, "cryoglyph prepare_axis failed");
        }

        queue.submit(Some(encoder.finish()));
    }

    /// Render the axis-layer text (priceline numbers). Call BEFORE
    /// any annotation-layer draws so priceline labels sit behind all
    /// decorators, indicators, and other annotations.
    pub fn draw_axis(&self, render_pass: &mut RenderPass<'_>) {
        if let Err(err) = self
            .axis_renderer
            .render(&self.atlas, &self.viewport, render_pass)
        {
            tracing::warn!(?err, "cryoglyph draw_axis failed");
        }
    }

    /// Render just the text for one z-layer. Call after that layer's
    /// badges have been drawn so text composites on top of its own
    /// shapes while still staying beneath the next layer's shapes.
    pub fn draw_layer(&self, layer_idx: usize, render_pass: &mut RenderPass<'_>) {
        if let Some(renderer) = self.renderers.get(layer_idx) {
            if let Err(err) = renderer.render(&self.atlas, &self.viewport, render_pass) {
                tracing::warn!(layer = layer_idx, ?err, "cryoglyph render failed");
            }
        }
    }

    /// Render every layer back-to-back. Used for the
    /// non-interleaved draw path (e.g. `ChartRenderer::render`); the
    /// interleaved `draw_pass` calls `draw_layer` per z-level.
    pub fn draw(&self, render_pass: &mut RenderPass<'_>) {
        for layer_idx in 0..ANNOTATION_LAYER_COUNT {
            self.draw_layer(layer_idx, render_pass);
        }
    }
}

/// Shape a single `WidgetLabel` into a cosmic-text buffer and a
/// resolved placement. Shared between the annotation-layer and the
/// axis-layer prepare loops.
fn shape_label(label: &WidgetLabel, font_system: &mut FontSystem) -> (Buffer, LabelPlacement) {
    let metrics = Metrics::new(label.font_size, label.font_size * LINE_HEIGHT_FACTOR);
    let mut buffer = Buffer::new(font_system, metrics);
    let attrs = Attrs::new().family(Family::SansSerif);
    buffer.set_text(
        font_system,
        &label.text,
        &attrs,
        Shaping::Advanced,
        None::<Align>,
    );
    buffer.shape_until_scroll(font_system, false);

    let (text_w, text_h) = measure_buffer(&buffer);
    let (left, top) = resolve_anchor(label, text_w, text_h);
    let color = rgba_to_color(label.text_color);
    (buffer, LabelPlacement { left, top, color })
}

/// Build a cryoglyph `TextArea` from a shaped buffer + placement.
fn make_area<'a>(buffer: &'a Buffer, p: LabelPlacement, bounds: TextBounds) -> TextArea<'a> {
    TextArea {
        buffer,
        left: p.left,
        top: p.top,
        scale: 1.0,
        bounds,
        default_color: p.color,
    }
}

/// Measure the shaped `buffer`'s max line width and total height.
/// Walks `layout_runs` — cheap, already-shaped data.
fn measure_buffer(buffer: &Buffer) -> (f32, f32) {
    let mut max_w = 0.0_f32;
    let mut count = 0_usize;
    let line_height = buffer.metrics().line_height;
    for run in buffer.layout_runs() {
        if run.line_w > max_w {
            max_w = run.line_w;
        }
        count += 1;
    }
    let total_h = (count.max(1) as f32) * line_height;
    (max_w, total_h)
}

/// Resolve a [`WidgetLabel`]'s position into the `(left, top)` corner
/// cryoglyph's [`TextArea`] expects.
fn resolve_anchor(label: &WidgetLabel, text_w: f32, text_h: f32) -> (f32, f32) {
    match label.anchor {
        LabelAnchor::TopLeft => (label.screen_x, label.screen_y),
        LabelAnchor::Center => (
            label.screen_x - text_w * 0.5,
            label.screen_y - text_h * 0.5,
        ),
        LabelAnchor::Left => (label.screen_x, label.screen_y - text_h * 0.5),
        LabelAnchor::Right => (label.screen_x - text_w, label.screen_y - text_h * 0.5),
    }
}

/// Convert linear `[f32; 4]` RGBA (the widget-compute convention) to
/// cryoglyph's packed [`Color`]. sRGB channels are stored straight —
/// cryoglyph does its own gamma handling in-shader.
fn rgba_to_color(rgba: [f32; 4]) -> Color {
    let to_u8 = |c: f32| (c.clamp(0.0, 1.0) * 255.0) as u8;
    Color::rgba(to_u8(rgba[0]), to_u8(rgba[1]), to_u8(rgba[2]), to_u8(rgba[3]))
}
