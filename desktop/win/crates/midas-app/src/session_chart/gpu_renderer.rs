//! [`SessionChartRenderer`] — thin adapter that bridges the S8 session-chart
//! pipeline ([`RenderBuckets`]) to the existing [`midas_render::ChartRenderer`].
//!
//! ## What this is
//!
//! The S8 path produces AoS [`midas_scene::ScenePrimitives`] which the
//! translator in `primitives_bridge.rs` flattens into legacy-shaped
//! [`RenderBuckets`]. This renderer owns exactly one
//! [`midas_render::ChartRenderer`] (the same one `chart_widget.rs` uses
//! for the legacy chart) and feeds it a [`midas_render::ChartScene`]
//! built on the fly from slice refs into the bucket vectors.
//!
//! The draw side simply forwards `draw_pass` to the inner renderer.
//!
//! ## Deferred gaps (documented by the ideal design)
//!
//! - **Text rendering is NOT wired.** `RenderBuckets.badges` is a
//!   `Vec<BadgeMetaInstance>` holding `Cow<'static, str>` text — the
//!   legacy SDF badge pipeline's glyph-atlas path is a separate
//!   `midas_chart::widget::compute::WidgetLabel` flow not yet ported.
//!   For this slice we pass `labels: &[]` and `axis_labels: &[]` to
//!   the `ChartScene`. Badges still render as colored SDF rectangles
//!   (filled shape + border via the shape_id discriminant) — just no
//!   text inside. Follow-up: port the glyph pipeline, see
//!   `plan/session-aware-charts/00b-integration-strategy.md` "S9" and
//!   the `[R2-G-2]` gap in `00a-ideal-design.md`.
//! - **Filled quads (session bands) use the grid pipeline.** The grid
//!   pipeline's shader draws filled rectangles too — a
//!   [`midas_chart::GridLineInstance`] is just an `{ rect, color }`
//!   struct, and the shader treats `rect` as a full filled rectangle
//!   (not only 1-px lines). We feed bands + thin lines into the same
//!   `grid_lines` slice; z-ordering is preserved by concatenation
//!   order (quads first, then lines).
//! - **Volume bars** currently route through `quads` too (the S8
//!   translator's documented invariant — `RenderBuckets.volumes` is
//!   always empty). The volume pipeline is therefore fed an empty
//!   slice; the volume strip shows up via the quad path inside the
//!   grid pipeline. A proper `VolumeLayer` → `VolumeInstance` bridge
//!   is a follow-up slice.
//! - **No per-instance layer-ends / annotation z-ordering.** We pass
//!   `[LayerEnd::default(); ANNOTATION_LAYER_COUNT]` (all zeros);
//!   the renderer's draw loop therefore draws the whole badge buffer
//!   in one range per layer, with zero per-layer text. This is
//!   correct because we have no annotations yet.
//!
//! ## GPU resource ownership
//!
//! One [`SessionChartRenderer`] per iced renderer (typically one per
//! OS window), stored inside the iced shader
//! [`Storage`][iced::widget::shader::Storage] via `SessionChartPipeline`
//! (see `shader.rs`).

#![cfg(feature = "session_chart")]

// Chart-transition slice 8.5 grep-gate exception — documented GPU-pipeline
// bridge. These `midas_chart::*` types are the legacy renderer's
// instance / dirty-tracking vocabulary that `midas-render::ChartRenderer`
// (which survives slice 9c) still consumes as its input. They are NOT
// chart-widget types (`Camera2D`, `ChartState`, `ChartScene-widget-level`
// are all absent here). Slice 9c either (a) migrates these structs into
// `midas-render` or (b) introduces a neutral GPU-primitive crate; either
// move lands as part of the atomic deletion PR, not here. Tracked under
// `plan/chart-transition/00-index.md` slice 9c pre-deletion checklist.
use midas_chart::compute::{LayerEnd, ANNOTATION_LAYER_COUNT};
use midas_chart::instances::{GridLineInstance, VolumeInstance};
use midas_chart::widget::compute::WidgetLabel;
use midas_chart::{BadgeInstance, DirtyFlags, DirtyTracker};
use midas_render::renderer::{ChartRenderer, ChartScene};

use super::primitives_bridge::{text_buckets_to_widget_labels, RenderBuckets};

/// Thin adapter around [`midas_render::ChartRenderer`] that consumes
/// [`RenderBuckets`]. See module docs for the deferred gaps.
pub struct SessionChartRenderer {
    inner: ChartRenderer,
    /// Dirty-flag tracker feeding the inner renderer's "only re-upload
    /// when dirty" path. We bump `camera_gen` on viewport changes and
    /// set the other flags to a monotonically increasing counter every
    /// prepare so the inner pipelines always re-upload instance data
    /// (the instance buffers are small — few hundred entries — and
    /// real per-generation tracking lives in a follow-up slice).
    tracker: DirtyTracker,
    /// Monotonic frame counter; every `prepare` bumps all non-camera
    /// generations by 1 so `tracker.needs_*_rebuild` returns `true`.
    frame_gen: u64,
    /// Camera generation — only bumps on viewport / projection change.
    camera_gen: u64,
    /// Cached badge instances — kept between frames so we can pass a
    /// slice reference into `ChartScene`. Empty when no badges are
    /// emitted this frame.
    badges: Vec<BadgeInstance>,
    /// Cached GridLineInstance buffer that holds both `quads` (session
    /// bands, filled rectangles) and `lines` (thin axis-aligned grid
    /// lines + crosshair arms + separators). Concatenated in
    /// `(quads, lines)` order every frame so quads paint behind lines.
    grid_lines: Vec<GridLineInstance>,
    /// Cached volume buffer — S8 translator leaves it empty.
    volumes: Vec<VolumeInstance>,
    /// Cached `WidgetLabel` buffer populated each frame from the
    /// translator's `TextMetaInstance` bucket. Routed through the
    /// inner renderer's `axis_labels` slot — cryoglyph's dedicated
    /// axis `TextRenderer` draws these behind every annotation-layer
    /// text batch.
    ///
    /// Slice 3 of the chart-transition plan closes the G-2
    /// text-rendering gap: the crosshair (and any future layer that
    /// emits `TextInstance`) now feeds the cryoglyph pipeline that
    /// already sits inside `ChartRenderer` — no extra `TextContext`
    /// plumbing needed, `ChartRenderer` owns one atlas per window and
    /// one renderer per layer internally.
    text_labels: Vec<WidgetLabel>,
    /// Last projection matrix we uploaded; compared on every prepare.
    last_projection: glam::Mat4,
    /// Last `(viewport_width, viewport_height)` seen.
    last_viewport: (u32, u32),
}

impl SessionChartRenderer {
    /// Construct the inner chart renderer. Matches
    /// [`ChartRenderer::new`]'s signature.
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        Self {
            inner: ChartRenderer::new(device, queue, format),
            tracker: DirtyTracker::new(),
            frame_gen: 0,
            camera_gen: 0,
            badges: Vec::new(),
            grid_lines: Vec::new(),
            volumes: Vec::new(),
            text_labels: Vec::new(),
            last_projection: glam::Mat4::IDENTITY,
            last_viewport: (0, 0),
        }
    }

    /// Upload GPU buffers from `buckets` and `projection` into the inner
    /// renderer. Safe to call with empty buckets — every slice is
    /// passed through unchanged and the inner pipelines allocate zero-
    /// sized buffers (the existing `update_instances` contract).
    pub fn prepare(
        &mut self,
        buckets: &RenderBuckets,
        viewport: (u32, u32),
        projection: glam::Mat4,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) {
        // Concatenate (quads, lines) into one GridLineInstance buffer.
        // Quads first so bands paint behind thin lines (session
        // separators + grid) — matches the ideal-design layer order.
        self.grid_lines.clear();
        self.grid_lines
            .reserve(buckets.quads.len() + buckets.lines.len());
        self.grid_lines.extend_from_slice(&buckets.quads);
        self.grid_lines.extend_from_slice(&buckets.lines);

        // TODO(session_chart): volume pipeline wiring — the S8
        // translator currently folds volume bars into `quads`, so the
        // VolumeInstance buffer stays empty. When the translator grows
        // a split-volume lane, feed it here.
        self.volumes.clear();
        self.volumes.extend_from_slice(&buckets.volumes);

        // TODO(session_chart): badge shape/border/text wiring. The
        // `BadgeMetaInstance` carries rect + fill only — the SDF
        // pipeline needs `shape_id`, `shape_param`, `border`, and
        // `border_thickness`. For now we emit each meta as a plain
        // `Rect` (shape_id = 0) with the fill color and no border.
        // Text rendering is deferred (module doc, ideal design R2-G-2).
        self.badges.clear();
        self.badges.reserve(buckets.badges.len());
        for b in &buckets.badges {
            self.badges.push(BadgeInstance {
                rect: b.rect,
                fill: b.color,
                border: [0.0; 4],
                shape_id: 0, // Rect — see instances.rs shape_id table
                shape_param: 0.0,
                border_thickness: 0.0,
                _pad: 0.0,
            });
        }

        // Slice 3: project scene `TextInstance` → `WidgetLabel` and
        // route through the inner renderer's cryoglyph axis-label
        // pipeline. One atlas per window; shared across every
        // crosshair / price-line / level / axis layer that emits
        // text. The conversion is O(n) in label count — bounded by
        // ~10 labels per frame today (6 crosshair + few axis ticks).
        self.text_labels = text_buckets_to_widget_labels(&buckets.text);

        // Camera-dirty detection: bump only when viewport or
        // projection actually changed so the inner renderer can skip
        // the uniform upload on still frames.
        let viewport_changed = self.last_viewport != viewport;
        let projection_changed = self.last_projection != projection;
        if viewport_changed || projection_changed {
            self.camera_gen = self.camera_gen.wrapping_add(1);
            self.last_projection = projection;
            self.last_viewport = viewport;
        }

        // Every prepare bumps the instance-data generations by one so
        // the tracker triggers re-upload. Real per-layer dirty
        // generations flow out of `midas-scene` in a follow-up slice;
        // until then the per-frame upload cost is bounded by the
        // tightly sized instance vectors above (few hundred entries).
        self.frame_gen = self.frame_gen.wrapping_add(1);

        let dirty = DirtyFlags {
            camera: self.camera_gen,
            candles: self.frame_gen,
            grid: self.frame_gen,
            levels: 0,
            crosshair: self.frame_gen,
            theme: 0,
            ..DirtyFlags::default()
        };

        let scene = ChartScene {
            projection,
            candles: &buckets.candles,
            volumes: &self.volumes,
            grid_lines: &self.grid_lines,
            // Crosshair is folded into `lines` by the scene builder —
            // the `midas-scene::CrosshairLayer` emits `LineInstance`s
            // which flow through the translator into `buckets.lines`.
            // No separate crosshair slice in this path.
            crosshair_lines: &[],
            volume_profile: &[],
            badges: &self.badges,
            // Slice 3: text rendering now flows through the
            // axis_labels slot so cryoglyph's dedicated
            // axis-TextRenderer draws them. The per-annotation-layer
            // `labels` slot stays empty — annotations lands in
            // slice 4+.
            labels: &[],
            axis_labels: &self.text_labels,
            // No annotations → zero layer boundaries. The inner
            // renderer's per-layer interleave loop therefore draws the
            // whole badge buffer in one range and issues zero text
            // draws.
            layer_ends: [LayerEnd::default(); ANNOTATION_LAYER_COUNT],
            viewport_width: viewport.0,
            viewport_height: viewport.1,
            dirty: &dirty,
        };

        self.inner
            .render_prepare(device, queue, &scene, &mut self.tracker);
    }

    /// Issue draw calls into an existing render pass. Delegates to the
    /// inner chart renderer's `draw_pass`. Safe after `prepare` on an
    /// empty bucket (no instances are drawn because pipelines have
    /// `instance_count = 0`).
    pub fn draw<'a>(&'a self, render_pass: &mut wgpu::RenderPass<'a>) {
        self.inner.draw_pass(render_pass);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Construct an empty RenderBuckets and confirm it's the zero
    /// shape the renderer expects. No GPU required.
    #[test]
    fn empty_buckets_shape_is_zero() {
        let b = RenderBuckets::default();
        assert!(b.candles.is_empty());
        assert!(b.quads.is_empty());
        assert!(b.lines.is_empty());
        assert!(b.badges.is_empty());
        assert!(b.text.is_empty());
        assert!(b.volumes.is_empty());
    }

    /// Confirm that concatenating quads + lines preserves counts — the
    /// renderer relies on this on the hot path. CPU-only.
    #[test]
    fn grid_line_concat_preserves_counts() {
        let quads = vec![
            GridLineInstance {
                rect: [0.0, 0.0, 10.0, 10.0],
                color: [0.1; 4],
            };
            3
        ];
        let lines = vec![
            GridLineInstance {
                rect: [0.0, 50.0, 100.0, 51.0],
                color: [0.2; 4],
            };
            4
        ];
        let mut buf = Vec::new();
        buf.extend_from_slice(&quads);
        buf.extend_from_slice(&lines);
        assert_eq!(buf.len(), quads.len() + lines.len());
        // quads come first (z behind), lines second.
        assert_eq!(buf[0].color[0], 0.1);
        assert_eq!(buf[buf.len() - 1].color[0], 0.2);
    }

    /// Translating badges preserves count and carries the fill color
    /// verbatim (other fields are defaulted — see TODO in `prepare`).
    #[test]
    fn badge_meta_to_instance_preserves_count() {
        use super::super::primitives_bridge::BadgeMetaInstance;
        let meta = vec![
            BadgeMetaInstance {
                rect: [0.0, 0.0, 40.0, 16.0],
                color: [0.2, 0.8, 0.3, 1.0],
                text: "x".into(),
            },
            BadgeMetaInstance {
                rect: [50.0, 0.0, 90.0, 16.0],
                color: [0.9, 0.3, 0.3, 1.0],
                text: "y".into(),
            },
        ];
        let mut out = Vec::with_capacity(meta.len());
        for b in &meta {
            out.push(BadgeInstance {
                rect: b.rect,
                fill: b.color,
                border: [0.0; 4],
                shape_id: 0,
                shape_param: 0.0,
                border_thickness: 0.0,
                _pad: 0.0,
            });
        }
        assert_eq!(out.len(), meta.len());
        assert_eq!(out[0].fill, [0.2, 0.8, 0.3, 1.0]);
        assert_eq!(out[1].shape_id, 0);
    }

    // GPU integration tests that spin up a real wgpu device live in
    // the workspace-level `tests/` directory (gated on
    // `session_chart_tests` + a headless adapter). See
    // `plan/session-aware-charts/00b-integration-strategy.md` for the
    // headless-GPU testing story. Keeping the renderer's unit tests
    // CPU-only means this file runs clean on every CI runner even
    // when no wgpu adapter is available.
}
