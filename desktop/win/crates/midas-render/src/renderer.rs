//! Chart renderer orchestrator.
//!
//! [`ChartRenderer`] owns all GPU pipelines and coordinates the
//! draw order for a single chart. It consumes a [`ChartScene`] and
//! a [`DirtyTracker`] to minimize GPU uploads.

use midas_chart::{
    CandleInstance, DirtyFlags, DirtyTracker, GridLineInstance, VolumeInstance,
};

use crate::pipelines::{
    candle::CandlePipeline,
    grid::GridPipeline,
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
    /// Crosshair overlay pipeline (reuses the grid shader for thin lines).
    crosshair_pipeline: GridPipeline,
}

impl ChartRenderer {
    /// Create a new chart renderer with all pipelines.
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        Self {
            candle_pipeline: CandlePipeline::new(device, format),
            volume_pipeline: VolumePipeline::new(device, format),
            grid_pipeline: GridPipeline::new(device, format),
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
            self.candle_pipeline.update_projection(queue, &scene.projection);
            self.volume_pipeline.update_projection(queue, &scene.projection);
            self.grid_pipeline.update_projection(queue, &scene.projection);
            self.crosshair_pipeline.update_projection(queue, &scene.projection);
        }

        // -- Update instance buffers only if dirty --
        if tracker.needs_candle_rebuild(scene.dirty) {
            self.candle_pipeline.update_instances(device, queue, scene.candles);
            self.volume_pipeline.update_instances(device, queue, scene.volumes);
        }

        if tracker.needs_grid_rebuild(scene.dirty) {
            self.grid_pipeline.update_instances(device, queue, scene.grid_lines);
        }

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

        // Layer 3: Candle wicks (opaque, behind bodies)
        self.candle_pipeline.draw_wicks(render_pass);

        // Layer 4: Candle bodies (opaque, on top of wicks)
        self.candle_pipeline.draw_bodies(render_pass);

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
            self.candle_pipeline.update_projection(queue, &scene.projection);
            self.volume_pipeline.update_projection(queue, &scene.projection);
            self.grid_pipeline.update_projection(queue, &scene.projection);
            self.crosshair_pipeline.update_projection(queue, &scene.projection);
        }

        // -- Update instance buffers only if dirty --
        if tracker.needs_candle_rebuild(scene.dirty) {
            self.candle_pipeline.update_instances(device, queue, scene.candles);
            self.volume_pipeline.update_instances(device, queue, scene.volumes);
        }

        if tracker.needs_grid_rebuild(scene.dirty) {
            self.grid_pipeline.update_instances(device, queue, scene.grid_lines);
        }

        // Crosshair lines update whenever the crosshair generation changes.
        if tracker.needs_crosshair_update(scene.dirty) {
            self.crosshair_pipeline
                .update_instances(device, queue, scene.crosshair_lines);
        }

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
        self.candle_pipeline.draw_wicks(render_pass);
        self.candle_pipeline.draw_bodies(render_pass);
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
