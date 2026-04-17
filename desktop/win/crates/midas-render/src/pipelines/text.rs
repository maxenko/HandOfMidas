//! GPU text pipeline using cryoglyph (iced 0.14's glyphon fork).
//!
//! Renders decorator labels (level price text, bracket segment text,
//! etc.) directly on the GPU, in the same render pass as the SDF
//! [`BadgePipeline`]. This is the architectural fix that replaces the
//! former iced-text-overlay approach — labels can now be interleaved
//! per z-layer with their owning shapes instead of always drawing on
//! top of the whole GPU pass.
//!
//! ## Lifecycle
//!
//! - [`TextPipeline::new`] allocates the shared atlas + viewport +
//!   one [`TextRenderer`].
//! - [`TextPipeline::prepare`] shapes each [`WidgetLabel`] into a
//!   cosmic-text [`Buffer`] and pushes a [`TextArea`] into the
//!   renderer's vertex buffer. Buffers are re-shaped each frame —
//!   simple and good enough for the small label counts a chart uses
//!   (< 100 per frame).
//! - [`TextPipeline::draw`] issues `TextRenderer::render` on the live
//!   render pass.

use cryoglyph::cosmic_text::Align;
use cryoglyph::{
    Attrs, Buffer, Cache, Color, Family, FontSystem, Metrics, Resolution, Shaping, SwashCache,
    TextArea, TextAtlas, TextBounds, TextRenderer, Viewport,
};
use midas_chart::widget::compute::{LabelAnchor, WidgetLabel};
use wgpu::{
    CommandEncoderDescriptor, Device, MultisampleState, Queue, RenderPass, TextureFormat,
};

/// Minimum line-height multiplier applied on top of `font_size` when
/// constructing cosmic-text [`Metrics`]. 1.2× covers typical ascender
/// / descender metrics for sans-serif fonts so labels don't clip.
const LINE_HEIGHT_FACTOR: f32 = 1.2;

/// Pipeline that draws text via cryoglyph on the chart render pass.
///
/// Owns the persistent GPU resources (atlas, viewport, renderer) plus
/// the CPU-side shaping caches (font DB, swash cache). All fields are
/// per-chart — each [`ChartRenderer`](crate::ChartRenderer) owns one
/// `TextPipeline`.
pub struct TextPipeline {
    font_system: FontSystem,
    swash_cache: SwashCache,
    atlas: TextAtlas,
    viewport: Viewport,
    renderer: TextRenderer,
    /// Re-shaped each frame from the scene's labels. Held on the
    /// pipeline so the borrows handed to `TextArea` outlive the
    /// `prepare` call. Buffers are cheap to reallocate — cosmic-text
    /// caches the expensive part (font shaping) inside `FontSystem`.
    buffers: Vec<Buffer>,
    /// Per-buffer placement + bounds. Index-parallel to `buffers` so
    /// `prepare` can zip them into `TextArea`s without extra state.
    placements: Vec<LabelPlacement>,
}

/// Resolved per-label layout — computed once in `prepare` and reused
/// by `draw`-adjacent code that needs to know where a label lives.
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
        let renderer =
            TextRenderer::new(&mut atlas, device, MultisampleState::default(), None);

        // `FontSystem::new` populates the database with system fonts.
        // On Windows that picks up Segoe UI Variable / Segoe UI, which
        // matches the default iced uses.
        let font_system = FontSystem::new();
        let swash_cache = SwashCache::new();

        Self {
            font_system,
            swash_cache,
            atlas,
            viewport,
            renderer,
            buffers: Vec::new(),
            placements: Vec::new(),
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

    /// Shape each label and hand the resulting `TextArea` list to the
    /// cryoglyph renderer. Rebuilds the per-frame buffer list from
    /// scratch — fine for the small label counts a chart produces.
    ///
    /// cryoglyph requires a `CommandEncoder` for its staging-belt
    /// copies. `prepare` creates and submits its own local encoder so
    /// callers on iced's `Primitive::prepare` path (which doesn't
    /// expose an encoder) can drive text upload without plumbing.
    pub fn prepare(
        &mut self,
        device: &Device,
        queue: &Queue,
        viewport_width: u32,
        viewport_height: u32,
        labels: &[WidgetLabel],
    ) {
        self.update_viewport(queue, viewport_width, viewport_height);

        self.buffers.clear();
        self.buffers.reserve(labels.len());
        self.placements.clear();
        self.placements.reserve(labels.len());

        // First pass: shape each label into its own `Buffer`, then
        // compute placement from the anchor and measured text width.
        for label in labels {
            let metrics = Metrics::new(label.font_size, label.font_size * LINE_HEIGHT_FACTOR);
            let mut buffer = Buffer::new(&mut self.font_system, metrics);
            let attrs = Attrs::new().family(Family::SansSerif);
            buffer.set_text(
                &mut self.font_system,
                &label.text,
                &attrs,
                Shaping::Advanced,
                None::<Align>,
            );
            buffer.shape_until_scroll(&mut self.font_system, false);

            let (text_w, text_h) = measure_buffer(&buffer);
            let (left, top) = resolve_anchor(label, text_w, text_h);
            let color = rgba_to_color(label.text_color);

            self.buffers.push(buffer);
            self.placements.push(LabelPlacement { left, top, color });
        }

        // Second pass: zip buffers + placements into `TextArea`s for
        // cryoglyph. A local encoder wraps the upload; submitting it
        // immediately is the cheapest way to satisfy cryoglyph's API
        // when we only have device/queue access.
        let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("text_pipeline_prepare"),
        });

        let vp_w = viewport_width.max(1) as i32;
        let vp_h = viewport_height.max(1) as i32;
        let areas = self
            .buffers
            .iter()
            .zip(self.placements.iter())
            .map(|(buf, p)| TextArea {
                buffer: buf,
                left: p.left,
                top: p.top,
                scale: 1.0,
                bounds: TextBounds {
                    left: 0,
                    top: 0,
                    right: vp_w,
                    bottom: vp_h,
                },
                default_color: p.color,
            });

        if let Err(err) = self.renderer.prepare(
            device,
            queue,
            &mut encoder,
            &mut self.font_system,
            &mut self.atlas,
            &self.viewport,
            areas,
            &mut self.swash_cache,
        ) {
            tracing::warn!(?err, "cryoglyph prepare failed — skipping text frame");
        }

        queue.submit(Some(encoder.finish()));
    }

    /// Issue the text draw call against the active render pass.
    pub fn draw(&self, render_pass: &mut RenderPass<'_>) {
        if let Err(err) = self
            .renderer
            .render(&self.atlas, &self.viewport, render_pass)
        {
            tracing::warn!(?err, "cryoglyph render failed");
        }
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

/// Resolve a `WidgetLabel`'s position into the `(left, top)` corner
/// cryoglyph's `TextArea` expects.
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

/// Convert linear `[f32; 4]` RGBA (same format used across widget
/// compute) to cryoglyph's packed `Color`. sRGB channels are stored
/// straight — cryoglyph does its own gamma handling in-shader.
fn rgba_to_color(rgba: [f32; 4]) -> Color {
    let to_u8 = |c: f32| (c.clamp(0.0, 1.0) * 255.0) as u8;
    Color::rgba(to_u8(rgba[0]), to_u8(rgba[1]), to_u8(rgba[2]), to_u8(rgba[3]))
}
