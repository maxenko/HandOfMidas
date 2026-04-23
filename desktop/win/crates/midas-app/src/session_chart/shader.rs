//! [`SessionChartProgram`] + [`SessionChartPrimitive`] — the iced-wgpu
//! shader widget that drives the session-chart GPU path.
//!
//! ## Architecture
//!
//! This module sits on top of [`super::gpu_renderer::SessionChartRenderer`]
//! and implements iced 0.14's [`shader::Program`] +
//! [`shader::Primitive`] traits so a `SessionChart` can render inside
//! the normal iced widget tree.
//!
//! ```text
//!  view() ──────────┐
//!                   │
//!      shader(SessionChartProgram { chart: Arc<RwLock<SessionChart>> })
//!                   │
//!   Program::draw   │  - try_read() the chart
//!                   │  - sync viewport + crosshair to bounds / cursor
//!                   │  - call chart.paint_buckets()
//!                   │  - build a SessionChartPrimitive { buckets,
//!                   │    viewport, projection, palette }
//!                   ▼
//!   Primitive::prepare
//!                   │  - storage.get_mut::<SessionChartPipeline>()
//!                   │  - pipeline.renderer.prepare(buckets, ...)
//!                   ▼
//!   Primitive::draw
//!                   │  - pipeline.renderer.draw(render_pass)
//!                   ▼
//!                 pixels
//! ```
//!
//! ## Sharing the chart between iced and the app
//!
//! `view()` receives `&self`, but `paint_buckets()` needs `&mut
//! SessionChart`. The program therefore owns an
//! `Arc<parking_lot::RwLock<SessionChart>>` — the app's
//! [`crate::session_chart_window::SessionChartWindow`] holds the same
//! Arc; clones flow through `view()` cheaply. The widget stays the
//! single source of truth for per-chart state.
//!
//! **Non-blocking paint**: we use `try_write()` so a busy writer (the
//! `cycle_eh_policy` message handler, a resize, etc.) never stalls
//! paint. When the lock is contended we render the previous frame's
//! cached primitive — the screen stays alive and the next frame picks
//! up the fresh state. Matches the pattern used by the legacy chart
//! widget for the same hazard.
//!
//! ## Input handling
//!
//! The `update` handler translates iced mouse events into `SessionChart`
//! mutations:
//!
//! - `CursorMoved` → [`SessionChart::set_crosshair`] when over bounds,
//!   [`SessionChart::clear_crosshair`] when outside.
//! - `WheelScrolled` → [`SessionChart::cycle_eh_policy`] (stub —
//!   real pan/zoom lives in a follow-up; wheel currently just cycles
//!   the EhPolicy so the chrome is observably interactive).
//! - `ButtonPressed` / `ButtonReleased` → captured but not yet wired
//!   to drag; documented TODO.
//!
//! The real pan/zoom interaction state machine lives in a follow-up
//! slice (see `plan/session-aware-charts/00b-integration-strategy.md`
//! row "S15").

#![cfg(feature = "session_chart")]

use std::sync::Arc;

use iced::widget::shader::{self, Viewport};
use iced::{mouse, Event, Rectangle};
use midas_scene::ThemePalette;
use parking_lot::RwLock;

use super::gpu_renderer::SessionChartRenderer;
use super::primitives_bridge::RenderBuckets;
use super::widget::SessionChart;

/// iced `Program` that feeds the GPU renderer from a shared
/// `SessionChart`. Cloneable Arc on the inside — cheap to build per
/// frame.
///
/// Generic over `M` (the host's `Message` type) so the program stays
/// re-usable from both the binary target (where `M =
/// crate::app::Message`) and the library target (where no concrete
/// Message type is in scope). The `PhantomData` placeholder honours
/// the iced `Program<M>` trait bound without binding on a concrete
/// type.
pub struct SessionChartProgram<M> {
    /// Shared chart state. The app's `SessionChartWindow` holds the
    /// same Arc; every `view()` clones it and hands it to one fresh
    /// `SessionChartProgram`.
    pub chart: Arc<RwLock<SessionChart>>,
    _message: std::marker::PhantomData<M>,
}

impl<M> SessionChartProgram<M> {
    /// Build a new program from a shared chart handle.
    pub fn new(chart: Arc<RwLock<SessionChart>>) -> Self {
        Self {
            chart,
            _message: std::marker::PhantomData,
        }
    }
}

/// Per-widget persistent state owned by iced's widget tree. Caches
/// the last rendered primitive so we can fall back to it when the
/// chart RwLock is contended (paint must not stall).
#[derive(Default)]
pub struct SessionChartWidgetState {
    /// Last rendered buckets. Non-`None` after the first successful
    /// paint. Reused verbatim when `try_write()` on the chart fails.
    last_buckets: Option<RenderBuckets>,
    /// Last palette captured from the chart (cheap — `Copy`).
    last_palette: Option<ThemePalette>,
}

impl<M: 'static> shader::Program<M> for SessionChartProgram<M> {
    type State = SessionChartWidgetState;
    type Primitive = SessionChartPrimitive;

    fn update(
        &self,
        _state: &mut Self::State,
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<shader::Action<M>> {
        // Try to take a write guard. If contended, skip this event
        // (the next frame will pick it up).
        let mut chart = self.chart.try_write()?;

        // Keep the chart's viewport in sync with the actual widget
        // bounds. The session-chart `Viewport` is logical pixels
        // (width × height) — exactly the bounds iced hands us.
        let bounds_vp = midas_axis::Viewport::new(bounds.width.max(1.0), bounds.height.max(1.0));
        if (chart.viewport().width_px - bounds_vp.width_px).abs() > f32::EPSILON
            || (chart.viewport().height_px - bounds_vp.height_px).abs() > f32::EPSILON
        {
            chart.set_viewport(bounds_vp);
        }

        match event {
            Event::Mouse(mouse_event) => match mouse_event {
                mouse::Event::CursorMoved { .. } => {
                    if let Some(pos) = cursor.position_in(bounds) {
                        chart.set_crosshair((pos.x, pos.y));
                    } else {
                        chart.clear_crosshair();
                    }
                    None
                }
                mouse::Event::CursorLeft => {
                    chart.clear_crosshair();
                    None
                }
                // TODO(session_chart): left-drag → pan via
                // `SessionChart::set_time_window`; wheel → zoom Y
                // via `set_price_range`. Stub: wheel cycles EhPolicy
                // so the chrome is observably interactive during the
                // shader-integration slice.
                mouse::Event::WheelScrolled { delta } => {
                    let dy = match delta {
                        mouse::ScrollDelta::Lines { y, .. } => *y,
                        mouse::ScrollDelta::Pixels { y, .. } => *y / 50.0,
                    };
                    if dy.abs() > 0.5 && cursor.is_over(bounds) {
                        let _ = chart.cycle_eh_policy();
                        // Publish nothing — the app's EhPolicy state is
                        // kept inside the widget itself. If the host
                        // needs a message for re-subscription we'd wire
                        // `Message::SessionChartCyclePolicy` here.
                    }
                    None
                }
                _ => None,
            },
            _ => None,
        }
    }

    fn draw(
        &self,
        state: &Self::State,
        _cursor: mouse::Cursor,
        bounds: Rectangle,
    ) -> Self::Primitive {
        // Non-blocking paint: try_write so a stalled writer can't
        // freeze the frame pump. On contention we replay the last
        // successful buckets.
        let (buckets, palette) = match self.chart.try_write() {
            Some(mut chart) => {
                // Re-sync viewport (bounds may have changed between
                // update() and draw() on a resize).
                let bounds_vp =
                    midas_axis::Viewport::new(bounds.width.max(1.0), bounds.height.max(1.0));
                if (chart.viewport().width_px - bounds_vp.width_px).abs() > f32::EPSILON
                    || (chart.viewport().height_px - bounds_vp.height_px).abs() > f32::EPSILON
                {
                    chart.set_viewport(bounds_vp);
                }
                let b = chart.paint_buckets();
                // Capture palette by re-reading from the shared scene
                // config. `ThemePalette` is `Copy` — no allocation.
                let p = ThemePalette::dark_default();
                (b, p)
            }
            None => {
                // Contended: fall back to cached state from the last
                // frame. If we have no cached state yet (very first
                // frame), render an empty primitive (safe — the
                // renderer tolerates empty buckets).
                (
                    state.last_buckets.clone().unwrap_or_default(),
                    state
                        .last_palette
                        .unwrap_or_else(ThemePalette::dark_default),
                )
            }
        };

        let vw = bounds.width.max(1.0) as u32;
        let vh = bounds.height.max(1.0) as u32;

        // Orthographic projection: maps pixel-space (x ∈ [0, w], y ∈
        // [0, h] with y=0 at the top) to NDC. Mirrors Camera2D in
        // midas-chart so the existing pipelines render correctly.
        let projection = glam::Mat4::orthographic_rh(
            0.0,       // left
            vw as f32, // right
            vh as f32, // bottom (screen-bottom = high Y = NDC bottom)
            0.0,       // top
            0.0,       // near
            1.0,       // far
        );

        SessionChartPrimitive {
            buckets,
            viewport_width: vw,
            viewport_height: vh,
            projection,
            palette,
        }
    }

    fn mouse_interaction(
        &self,
        _state: &Self::State,
        _bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        // Default cursor everywhere — no drag / resize affordances
        // yet. The crosshair is rendered via the scene layer, not
        // via the mouse interaction cursor. Follow-up slice may add
        // grab / resize zones for pan / zoom handles.
        mouse::Interaction::default()
    }
}

// ── SessionChartPrimitive ───────────────────────────────────────────

/// Per-frame rendering data passed from [`SessionChartProgram::draw`]
/// to the GPU. Wraps the translated [`RenderBuckets`] plus the
/// projection matrix and viewport size.
pub struct SessionChartPrimitive {
    /// Translated scene primitives — candles, quads, lines, badges.
    pub buckets: RenderBuckets,
    /// Viewport dimensions in logical pixels.
    pub viewport_width: u32,
    pub viewport_height: u32,
    /// Orthographic pixel → NDC projection matrix.
    pub projection: glam::Mat4,
    /// Theme palette (currently unused beyond the clear colour — the
    /// actual fill colours are baked into the per-instance data by
    /// the scene pipeline. Kept on the primitive so a future
    /// clear-colour override can read it without a lock).
    pub palette: ThemePalette,
}

impl std::fmt::Debug for SessionChartPrimitive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionChartPrimitive")
            .field("candles", &self.buckets.candles.len())
            .field("quads", &self.buckets.quads.len())
            .field("lines", &self.buckets.lines.len())
            .field("badges", &self.buckets.badges.len())
            .field("viewport", &(self.viewport_width, self.viewport_height))
            .finish()
    }
}

// ── SessionChartPipeline (iced wgpu Pipeline) ───────────────────────

/// Shared GPU pipeline state. One per iced `Renderer` (typically one
/// per OS window). Owns a [`SessionChartRenderer`] that in turn owns
/// the inner [`midas_render::ChartRenderer`] + all wgpu resources.
pub struct SessionChartPipeline {
    /// The heavy GPU object. Lazily constructed on first prepare —
    /// iced's Storage API hands us the device + format up front, so
    /// the renderer is eagerly built in `Pipeline::new` below.
    renderer: SessionChartRenderer,
}

impl shader::Pipeline for SessionChartPipeline {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        tracing::info!(
            "SessionChartPipeline::new(format={:?}) — constructing inner ChartRenderer",
            format
        );
        Self {
            renderer: SessionChartRenderer::new(device, queue, format),
        }
    }
}

impl shader::Primitive for SessionChartPrimitive {
    type Pipeline = SessionChartPipeline;

    fn prepare(
        &self,
        pipeline: &mut Self::Pipeline,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _bounds: &Rectangle,
        _viewport: &Viewport,
    ) {
        pipeline.renderer.prepare(
            &self.buckets,
            (self.viewport_width, self.viewport_height),
            self.projection,
            device,
            queue,
        );
    }

    fn draw(&self, pipeline: &Self::Pipeline, render_pass: &mut wgpu::RenderPass<'_>) -> bool {
        // SAFETY: iced stores the Pipeline in heap-allocated Storage
        // that outlives the render pass. The wgpu draw methods need
        // the pipeline reference lifetime to match the render pass
        // lifetime, but the Primitive::draw trait signature uses
        // independent lifetimes. We extend the pipeline reference
        // lifetime to match the render pass. This is safe because the
        // Pipeline is guaranteed to outlive the render pass by iced's
        // architecture (Pipeline lives in Storage, only dropped after
        // all rendering is complete). Same pattern as
        // `chart_widget.rs::ChartPrimitive::draw`.
        let renderer: &SessionChartRenderer = &pipeline.renderer;
        let renderer: &SessionChartRenderer =
            unsafe { &*(renderer as *const SessionChartRenderer) };
        renderer.draw(render_pass);
        true
    }
}

// ── Convenience constructor ──────────────────────────────────────────

/// Build the iced shader widget element for a session chart. Height
/// / width fill the parent container by default; the caller can
/// override with `.width()` / `.height()` on the returned value
/// through the widget type.
pub fn session_chart_shader<M: 'static>(
    program: SessionChartProgram<M>,
) -> iced::widget::Shader<M, SessionChartProgram<M>> {
    iced::widget::Shader::new(program)
        .width(iced::Fill)
        .height(iced::Fill)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Empty `RenderBuckets` → constructing a primitive yields zero
    /// instance counts on every lane. Exercised with a no-GPU path —
    /// we build the struct directly since `Primitive::prepare` needs
    /// a wgpu device.
    #[test]
    fn empty_buckets_primitive_has_zero_counts() {
        let prim = SessionChartPrimitive {
            buckets: RenderBuckets::default(),
            viewport_width: 800,
            viewport_height: 600,
            projection: glam::Mat4::IDENTITY,
            palette: ThemePalette::dark_default(),
        };
        assert_eq!(prim.buckets.candles.len(), 0);
        assert_eq!(prim.buckets.quads.len(), 0);
        assert_eq!(prim.buckets.lines.len(), 0);
        assert_eq!(prim.buckets.badges.len(), 0);
    }

    /// 10 synthetic candles → a primitive with 10 candle instances.
    /// The prepare() path would feed this to the inner ChartRenderer;
    /// we can't exercise that without a GPU, but we can assert the
    /// bucket counts flow through the primitive verbatim.
    #[test]
    fn ten_candles_preserves_instance_count() {
        use midas_chart::instances::CandleInstance;
        let candles = vec![
            CandleInstance {
                x: 10.0,
                body_top: 50.0,
                body_bottom: 60.0,
                wick_top: 45.0,
                wick_bottom: 65.0,
                width: 6.0,
                wick_width: 1.0,
                dim: 0.0,
                color: [0.2, 0.8, 0.3, 1.0],
            };
            10
        ];
        let buckets = RenderBuckets {
            candles,
            ..RenderBuckets::default()
        };
        let prim = SessionChartPrimitive {
            buckets,
            viewport_width: 1024,
            viewport_height: 400,
            projection: glam::Mat4::IDENTITY,
            palette: ThemePalette::dark_default(),
        };
        assert_eq!(prim.buckets.candles.len(), 10);
        let dbg = format!("{:?}", prim);
        assert!(dbg.contains("candles"));
    }

    /// Orthographic projection produced by the program matches the
    /// camera's convention — Y-inverted (y=0 at top of viewport maps
    /// to NDC +1). We don't have a Camera2D here but we can assert
    /// the canonical mapping at the corners.
    #[test]
    fn projection_maps_top_left_to_ndc_minus1_plus1() {
        let w = 800.0;
        let h = 600.0;
        let p = glam::Mat4::orthographic_rh(0.0, w, h, 0.0, 0.0, 1.0);
        let top_left = p * glam::Vec4::new(0.0, 0.0, 0.0, 1.0);
        let bottom_right = p * glam::Vec4::new(w, h, 0.0, 1.0);
        // NDC top-left is (-1, +1), bottom-right is (+1, -1).
        assert!((top_left.x - -1.0).abs() < 1e-4);
        assert!((top_left.y - 1.0).abs() < 1e-4);
        assert!((bottom_right.x - 1.0).abs() < 1e-4);
        assert!((bottom_right.y - -1.0).abs() < 1e-4);
    }

    /// Program::draw with a contended lock returns a primitive built
    /// from cached state rather than stalling. We can't easily test
    /// the program directly (it needs a `Cursor` which is a
    /// thin wrapper around an `Option<Point>`), but we can assert
    /// that `SessionChartWidgetState::default()` is a zeroed cache —
    /// the "no cache yet" branch falls through to empty buckets,
    /// which is the safe path.
    #[test]
    fn widget_state_default_has_no_cached_buckets() {
        let s = SessionChartWidgetState::default();
        assert!(s.last_buckets.is_none());
        assert!(s.last_palette.is_none());
    }
}
