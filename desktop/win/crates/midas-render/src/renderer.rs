//! Chart renderer orchestrator.
//!
//! [`ChartRenderer`] owns all GPU pipelines and coordinates the
//! draw order for a single chart. It consumes a [`ChartScene`] and
//! a [`DirtyTracker`] to minimize GPU uploads.

use midas_chart::compute::{LayerEnd, ANNOTATION_LAYER_COUNT};
use midas_chart::widget::compute::WidgetLabel;
use midas_chart::{
    BadgeInstance, CandleInstance, DirtyFlags, DirtyTracker, GridLineInstance, VolumeInstance,
};

use crate::pipelines::{
    badge::BadgePipeline, candle::CandlePipeline, grid::GridPipeline, text::TextPipeline,
    volume::VolumePipeline,
};

/// Scene data for a single chart frame.
///
/// This is a lightweight container passed to [`ChartRenderer::render`].
/// It references the instance arrays and the current dirty flags so
/// the renderer can decide which buffers to update.
///
/// `ChartScene` will eventually live in `midas-chart` as a full
/// framework-agnostic IR. For now this minimal version suffices for
/// the Phase 1 rendering pipeline.
pub struct ChartScene<'a> {
    /// Projection matrix (orthographic pixel-space to NDC).
    pub projection: glam::Mat4,
    /// Candle instance data (may be empty if unchanged).
    pub candles: &'a [CandleInstance],
    /// Volume instance data (may be empty if unchanged).
    pub volumes: &'a [VolumeInstance],
    /// Grid line instance data.
    pub grid_lines: &'a [GridLineInstance],
    /// Crosshair overlay lines (0 or 2 grid line instances: vertical + horizontal).
    pub crosshair_lines: &'a [GridLineInstance],
    /// Volume Profile histogram bars (empty if VP disabled).
    pub volume_profile: &'a [GridLineInstance],
    /// SDF decorator badge instances (drawn between candle bodies and
    /// the crosshair overlay). Empty when no decorators are active.
    pub badges: &'a [BadgeInstance],
    /// Widget text labels (price text inside decorator badges, etc.).
    /// Rendered in the same render pass as `badges` via the cryoglyph
    /// text pipeline so per-element z-order can be preserved.
    pub labels: &'a [WidgetLabel],
    /// Axis-area text labels (priceline numbers on the right side of
    /// the chart). Drawn by the text pipeline in a dedicated pre-
    /// annotation pass so axis labels sit behind every decorator,
    /// indicator, and other annotation.
    pub axis_labels: &'a [WidgetLabel],
    /// End-exclusive indices into `badges` / `labels` for each z-layer
    /// emitted by `compute_widget_annotations`. Drives the per-layer
    /// interleaved draw order in [`ChartRenderer::draw_pass`].
    pub layer_ends: [LayerEnd; ANNOTATION_LAYER_COUNT],
    /// Logical-pixel viewport size — cryoglyph's `Viewport` requires it.
    pub viewport_width: u32,
    pub viewport_height: u32,
    /// Current dirty flags snapshot.
    pub dirty: &'a DirtyFlags,
}

/// Chart renderer orchestrator.
///
/// Owns the four core GPU pipelines (candle, volume, grid, crosshair)
/// and coordinates the draw order within a single render pass.
pub struct ChartRenderer {
    candle_pipeline: CandlePipeline,
    volume_pipeline: VolumePipeline,
    grid_pipeline: GridPipeline,
    /// Volume Profile histogram pipeline (reuses grid shader for horizontal bars).
    volume_profile_pipeline: GridPipeline,
    /// SDF decorator badge pipeline (rendered above candles, below crosshair).
    badge_pipeline: BadgePipeline,
    /// GPU text pipeline (cryoglyph). Draws decorator labels in the
    /// same render pass as the badge pipeline so per-element z-order
    /// can be achieved by interleaving badge + text draws per layer.
    text_pipeline: TextPipeline,
    /// Layer boundaries recorded at prepare time — drives the per-
    /// layer interleave in `draw_pass`. Updated every frame; starts
    /// zeroed so the renderer is safe to draw from even before its
    /// first prepare.
    layer_ends: [LayerEnd; ANNOTATION_LAYER_COUNT],
    /// Crosshair overlay pipeline (reuses the grid shader for thin lines).
    crosshair_pipeline: GridPipeline,
}

impl ChartRenderer {
    /// Create a new chart renderer with all pipelines.
    ///
    /// `queue` is required because the text pipeline (cryoglyph) queues
    /// its initial atlas-texture upload during construction.
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        Self {
            candle_pipeline: CandlePipeline::new(device, format),
            volume_pipeline: VolumePipeline::new(device, format),
            grid_pipeline: GridPipeline::new(device, format),
            volume_profile_pipeline: GridPipeline::new(device, format),
            badge_pipeline: BadgePipeline::new(device, format),
            text_pipeline: TextPipeline::new(device, queue, format),
            layer_ends: [LayerEnd::default(); ANNOTATION_LAYER_COUNT],
            crosshair_pipeline: GridPipeline::new(device, format),
        }
    }

    /// Update GPU buffers and issue draw calls for one chart frame.
    ///
    /// **Buffer updates**: Only uploads data for categories that the
    /// `tracker` reports as dirty. After all uploads, the tracker is
    /// acknowledged so subsequent frames skip unchanged data.
    ///
    /// **Draw order** (back to front):
    /// 1. Grid lines (semi-transparent, lowest layer)
    /// 2. Volume bars (semi-transparent)
    /// 3. Candle wicks (opaque, thin)
    /// 4. Candle bodies (opaque, wide, on top of wicks)
    ///
    /// The `render_pass` must already be created by the caller. This
    /// method only sets pipelines, bind groups, and issues draw calls.
    pub fn render<'a>(
        &'a mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        render_pass: &mut wgpu::RenderPass<'a>,
        scene: &ChartScene<'_>,
        tracker: &mut DirtyTracker,
    ) {
        // -- Update projection on all pipelines if camera changed --
        if tracker.needs_camera_update(scene.dirty) {
            self.candle_pipeline
                .update_projection(queue, &scene.projection);
            self.volume_pipeline
                .update_projection(queue, &scene.projection);
            self.grid_pipeline
                .update_projection(queue, &scene.projection);
            self.volume_profile_pipeline
                .update_projection(queue, &scene.projection);
            self.badge_pipeline
                .update_projection(queue, &scene.projection);
            self.crosshair_pipeline
                .update_projection(queue, &scene.projection);
        }

        // -- Update instance buffers only if dirty --
        if tracker.needs_candle_rebuild(scene.dirty) {
            self.candle_pipeline
                .update_instances(device, queue, scene.candles);
            self.volume_pipeline
                .update_instances(device, queue, scene.volumes);
        }

        if tracker.needs_grid_rebuild(scene.dirty) {
            self.grid_pipeline
                .update_instances(device, queue, scene.grid_lines);
        }

        // Volume Profile always re-uploaded (small buffer, tracks data changes).
        self.volume_profile_pipeline
            .update_instances(device, queue, scene.volume_profile);

        // Decorator badges re-uploaded every frame they're present.
        // Empty slices are cheap: the pipeline zeroes its instance count
        // and the draw call is skipped below.
        self.badge_pipeline
            .update_instances(device, queue, scene.badges);

        // Text labels shape + upload (cryoglyph). Parallels the badge
        // upload above; `draw` below issues the text render call right
        // after badges so labels composite on top of their shapes.
        self.text_pipeline.prepare(
            device,
            queue,
            scene.viewport_width,
            scene.viewport_height,
            scene.labels,
            scene.layer_ends,
            scene.axis_labels,
        );
        // Cache the boundaries for `draw_pass`'s interleave loop.
        self.layer_ends = scene.layer_ends;

        // Crosshair lines update on every frame they are present (mouse moves).
        if tracker.needs_crosshair_update(scene.dirty) {
            self.crosshair_pipeline
                .update_instances(device, queue, scene.crosshair_lines);
        }

        // -- Acknowledge all current generations --
        tracker.acknowledge(scene.dirty);

        // -- Draw in strict back-to-front order --

        // Layer 1: Grid lines (semi-transparent, behind everything)
        self.grid_pipeline.draw(render_pass);

        // Layer 2: Volume bars (semi-transparent, behind candles)
        self.volume_pipeline.draw(render_pass);

        // Layer 2.5: Volume Profile histogram (semi-transparent, on top of volume bars)
        self.volume_profile_pipeline.draw(render_pass);

        // Layer 3: Candle wicks (opaque, behind bodies)
        self.candle_pipeline.draw_wicks(render_pass);

        // Layer 4: Candle bodies (opaque, on top of wicks)
        self.candle_pipeline.draw_bodies(render_pass);

        // Layer 4.4: Axis text (priceline numbers) — behind everything
        // the chart annotates.
        self.text_pipeline.draw_axis(render_pass);

        // Layer 4.5–4.6: decorator badges + text, interleaved per
        // z-layer (see `draw_pass` for the canonical implementation).
        let mut prev_badge: u32 = 0;
        for (layer_idx, end) in scene.layer_ends.iter().enumerate() {
            let end_u32 = end.badge_end as u32;
            self.badge_pipeline
                .draw_range(render_pass, prev_badge..end_u32);
            self.text_pipeline.draw_layer(layer_idx, render_pass);
            prev_badge = end_u32;
        }

        // Layer 5: Crosshair overlay (semi-transparent, on top of everything)
        self.crosshair_pipeline.draw(render_pass);
    }

    /// Upload GPU buffers without issuing draw calls.
    ///
    /// This is the "prepare" half of a split render cycle, suitable for
    /// use in iced's `Primitive::prepare()` where we only have device/queue
    /// access but no render pass.
    pub fn render_prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene: &ChartScene<'_>,
        tracker: &mut DirtyTracker,
    ) {
        // -- Update projection on all pipelines if camera changed --
        if tracker.needs_camera_update(scene.dirty) {
            self.candle_pipeline
                .update_projection(queue, &scene.projection);
            self.volume_pipeline
                .update_projection(queue, &scene.projection);
            self.grid_pipeline
                .update_projection(queue, &scene.projection);
            self.volume_profile_pipeline
                .update_projection(queue, &scene.projection);
            self.badge_pipeline
                .update_projection(queue, &scene.projection);
            self.crosshair_pipeline
                .update_projection(queue, &scene.projection);
        }

        // -- Update instance buffers only if dirty --
        if tracker.needs_candle_rebuild(scene.dirty) {
            self.candle_pipeline
                .update_instances(device, queue, scene.candles);
            self.volume_pipeline
                .update_instances(device, queue, scene.volumes);
        }

        // Grid lines always re-uploaded: the buffer is small and includes
        // the timeline border line which tracks user drag in real-time.
        self.grid_pipeline
            .update_instances(device, queue, scene.grid_lines);

        // Volume Profile always re-uploaded (small buffer, tracks data changes).
        self.volume_profile_pipeline
            .update_instances(device, queue, scene.volume_profile);

        // Decorator badges re-uploaded every frame they're present.
        self.badge_pipeline
            .update_instances(device, queue, scene.badges);

        // Text labels shape + upload. cryoglyph needs its own
        // CommandEncoder so the call doesn't share a queue submission
        // with the other pipelines — it issues its own `queue.submit`.
        self.text_pipeline.prepare(
            device,
            queue,
            scene.viewport_width,
            scene.viewport_height,
            scene.labels,
            scene.layer_ends,
            scene.axis_labels,
        );
        // Cache the boundaries for `draw_pass`'s interleave loop.
        self.layer_ends = scene.layer_ends;

        // Crosshair overlay always re-uploaded: the buffer is tiny
        // (~16 instances for the volume handle + 2 for crosshair lines)
        // and contains persistent UI elements (volume handle triangle)
        // that must appear even when the crosshair generation is unchanged.
        self.crosshair_pipeline
            .update_instances(device, queue, scene.crosshair_lines);

        // -- Acknowledge all current generations --
        tracker.acknowledge(scene.dirty);
    }

    /// Issue draw calls into an existing render pass.
    ///
    /// This is the "draw" half of a split render cycle, suitable for
    /// use in iced's `Primitive::draw()` where we have a live render pass.
    ///
    /// **Draw order** (back to front):
    /// 1. Grid lines (semi-transparent, lowest layer)
    /// 2. Volume bars (semi-transparent)
    /// 3. Candle wicks (opaque, thin)
    /// 4. Candle bodies (opaque, wide, on top of wicks)
    /// 5. Crosshair overlay (semi-transparent, on top of everything)
    pub fn draw_pass<'a>(&'a self, render_pass: &mut wgpu::RenderPass<'a>) {
        self.grid_pipeline.draw(render_pass);
        self.volume_pipeline.draw(render_pass);
        self.volume_profile_pipeline.draw(render_pass);
        self.candle_pipeline.draw_wicks(render_pass);
        self.candle_pipeline.draw_bodies(render_pass);
        // Layer 4.4: axis text (priceline numbers). Drawn BEFORE any
        // annotation/decorator so these labels sit behind everything
        // the chart annotates.
        self.text_pipeline.draw_axis(render_pass);

        // Layer 4.5–4.6: decorator badges + text, interleaved per
        // z-layer. For each layer k we draw its badge instance range
        // then its text batch, so an annotation's shape and label
        // composite as one unit over lower-z layers' shape and label.
        let mut prev_badge: u32 = 0;
        for (layer_idx, end) in self.layer_ends.iter().enumerate() {
            let end_u32 = end.badge_end as u32;
            self.badge_pipeline
                .draw_range(render_pass, prev_badge..end_u32);
            self.text_pipeline.draw_layer(layer_idx, render_pass);
            prev_badge = end_u32;
        }
        self.crosshair_pipeline.draw(render_pass);
    }

    /// Access the candle pipeline.
    pub fn candle_pipeline(&self) -> &CandlePipeline {
        &self.candle_pipeline
    }

    /// Access the volume pipeline.
    pub fn volume_pipeline(&self) -> &VolumePipeline {
        &self.volume_pipeline
    }

    /// Access the grid pipeline.
    pub fn grid_pipeline(&self) -> &GridPipeline {
        &self.grid_pipeline
    }
}
