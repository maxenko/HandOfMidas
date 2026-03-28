//! Shader widget bridge between midas-chart (sans-IO) and midas-render (GPU).
//!
//! This module implements iced 0.14's `shader::Program` and `shader::Primitive`
//! traits to render wgpu-based charts inside the iced widget tree.
//!
//! ## Data flow
//!
//! 1. `view()` constructs a `ChartProgram` with a `ChartRenderSnapshot`
//!    capturing the current chart panel state.
//! 2. `Program::draw()` calls `compute_chart_scene()` to produce a `ChartScene`.
//! 3. The result is wrapped in `ChartPrimitive`.
//! 4. `Primitive::prepare()` uploads scene data to GPU buffers via `ChartRenderer`.
//! 5. `Primitive::draw()` issues wgpu draw calls.
//!
//! ## Interaction flow
//!
//! `Program::update()` translates iced events into `ChartEvent`, calls
//! `midas_chart::handle_event()`, and emits `Message` variants for each
//! resulting `ChartAction`.

use std::collections::HashMap;
use std::sync::Arc;

use iced::widget::shader::{self, Viewport};
use iced::{Event, Rectangle, mouse};

use midas_chart::camera::Camera2D;
use midas_chart::dirty::{DirtyFlags, DirtyTracker};
use midas_chart::input::ChartInput;
use midas_chart::interaction::{ChartEvent, handle_event};
use midas_chart::levels::HorizontalLevel;
use midas_chart::scene::ChartScene;
use midas_chart::state::{ChartState, InteractionMode};
use midas_chart::{
    CandleInstance, CrosshairRender, GridLineInstance, VolumeInstance, compute_chart_scene,
};
use midas_core::ChartId;
use midas_data::CandleBuffer;
use midas_render::ChartRenderer;
use midas_render::color::dark_theme;
use midas_render::renderer::ChartScene as RenderScene;

use crate::app::Message;

// ── ChartRenderSnapshot ──────────────────────────────────────────────

/// Immutable data snapshot captured in `view()` for the shader widget.
///
/// Contains everything needed to compute a `ChartScene` and render it.
/// Built fresh each frame from the `ChartPanel`'s current state.
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct ChartRenderSnapshot {
    /// Symbol name for the OHLCV overlay (e.g. "AAPL").
    pub symbol: String,
    /// Candle data buffer (shared via Arc for zero-copy).
    pub data: Option<Arc<CandleBuffer>>,
    /// Camera defining the visible region.
    pub camera: Camera2D,
    /// Current dirty flags snapshot.
    pub dirty: DirtyFlags,
    /// Crosshair position in chart-local pixels.
    pub crosshair_pos: Option<(f32, f32)>,
    /// Horizontal price levels.
    pub levels: Vec<HorizontalLevel>,
    /// Viewport width in logical pixels.
    pub viewport_width: u32,
    /// Viewport height in logical pixels.
    pub viewport_height: u32,
    /// Whether session gaps are collapsed (index-based X positioning).
    pub collapse_gaps: bool,
    /// Volume bar height multiplier (1.0 = default).
    pub volume_scale: f32,
    /// Data time bounds for scroll clamping (first candle timestamp ms).
    pub data_time_start: f64,
    /// Data time bounds for scroll clamping (last candle timestamp ms).
    pub data_time_end: f64,
}

// ── ChartProgram ─────────────────────────────────────────────────────

/// Implements `shader::Program` for one chart panel.
///
/// Created each frame in `view()` with a fresh `ChartRenderSnapshot`.
/// The `chart_id` is used to route interaction events back to the correct
/// chart in the application state.
pub struct ChartProgram {
    /// Which chart this widget renders.
    pub chart_id: ChartId,
    /// Snapshot of the chart state for this frame.
    pub snapshot: ChartRenderSnapshot,
}

/// Per-widget persistent state owned by iced's widget tree.
///
/// Holds a clone of the `ChartState` for processing events inside
/// `Program::update()`. This allows the sans-IO interaction state machine
/// to run within the widget without requiring `&mut MidasApp`.
#[derive(Default)]
pub struct ChartWidgetState {
    /// Cloned chart state for interaction processing.
    /// Initialized on first use; refreshed from snapshot each frame.
    chart_state: Option<ChartState>,
    /// Last known viewport dimensions, used to detect resize and emit
    /// a scale-preserving camera adjustment message.
    last_viewport: Option<(u32, u32)>,
}

impl shader::Program<Message> for ChartProgram {
    type State = ChartWidgetState;
    type Primitive = ChartPrimitive;

    fn update(
        &self,
        state: &mut Self::State,
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<shader::Action<Message>> {
        // Ensure the widget state has a ChartState for interaction.
        if state.chart_state.is_none() {
            state.chart_state = Some(ChartState::new(self.snapshot.camera.clone()));
        }

        let chart_state = state.chart_state.as_mut().unwrap();
        // Sync camera from latest snapshot so interactions use current view.
        chart_state.camera = self.snapshot.camera.clone();
        // CRITICAL: Update camera viewport to match actual widget bounds.
        // The snapshot camera may have stale viewport dimensions from init.
        let new_w = bounds.width.max(1.0) as u32;
        let new_h = bounds.height.max(1.0) as u32;
        chart_state.camera.viewport_width = new_w;
        chart_state.camera.viewport_height = new_h;
        chart_state.dirty = self.snapshot.dirty.clone();
        chart_state.data_time_start = self.snapshot.data_time_start;
        chart_state.data_time_end = self.snapshot.data_time_end;
        chart_state.levels = self.snapshot.levels.clone();
        chart_state.volume_scale = self.snapshot.volume_scale;

        // Detect viewport resize and emit a scale-preserving adjustment.
        // Compare against the canonical camera viewport (from snapshot),
        // which the app keeps in sync after processing ChartViewportChanged.
        let snap_vp = (
            self.snapshot.camera.viewport_width,
            self.snapshot.camera.viewport_height,
        );
        let new_vp = (new_w, new_h);
        if let Some(old_vp) = state.last_viewport {
            if old_vp != new_vp && old_vp.0 > 0 && old_vp.1 > 0 {
                // Viewport is changing (window resize or pane divider drag).
                // Reset any active interaction so stale pan/scale state
                // doesn't linger — the early return below eats all events
                // (including mouse release) during the resize.
                chart_state.interaction_mode = InteractionMode::Idle;
                chart_state.drag_start = None;
                chart_state.left_mouse_down = false;
                chart_state.crosshair_pos = None;

                state.last_viewport = Some(new_vp);
                return Some(shader::Action::publish(
                    Message::ChartViewportChanged(
                        self.chart_id,
                        old_vp.0, old_vp.1,
                        new_vp.0, new_vp.1,
                    ),
                ));
            }
        }
        // Seed the initial viewport from snapshot so the first real
        // resize triggers correctly.  We use the snapshot's viewport
        // (canonical) rather than bounds so the very first frame
        // correctly detects that the canonical viewport differs from
        // widget bounds and emits the adjustment message.
        if state.last_viewport.is_none() {
            state.last_viewport = Some(snap_vp);
            // If snapshot viewport already differs from bounds, emit
            // an adjustment on this first frame.
            if snap_vp != new_vp && snap_vp.0 > 0 && snap_vp.1 > 0 {
                return Some(shader::Action::publish(
                    Message::ChartViewportChanged(
                        self.chart_id,
                        snap_vp.0, snap_vp.1,
                        new_vp.0, new_vp.1,
                    ),
                ));
            }
        }

        // Convert iced event to ChartEvent(s).
        let chart_events = translate_event(event, bounds, cursor);
        if chart_events.is_empty() {
            return None;
        }

        let cursor_in_bounds = cursor.is_over(bounds);

        let mut messages: Vec<Message> = Vec::new();
        let mut captured = false;

        for chart_event in chart_events {
            let actions = handle_event(chart_state, chart_event);
            for action in &actions {
                // Apply volume scale locally so the triangle moves
                // immediately during drag (before the message round-trip
                // through the app updates the canonical state).
                if let midas_chart::ChartAction::SetVolumeScale { scale } = action {
                    chart_state.volume_scale = *scale as f32;
                }
                if let Some(msg) = action_to_message(self.chart_id, action, &chart_state.camera) {
                    messages.push(msg);
                }
            }
            if !actions.is_empty() {
                captured = true;
            }
        }

        if messages.is_empty() && !captured {
            return None;
        }

        // Only capture the event when the cursor is inside this widget's
        // bounds.  When the cursor is outside (e.g. over another pane's
        // title bar), we still publish housekeeping messages like
        // ClearCrosshair but must NOT capture — otherwise sibling panes'
        // buttons and controls would be blocked from processing the event.
        if let Some(msg) = messages.into_iter().next() {
            if cursor_in_bounds {
                Some(shader::Action::publish(msg).and_capture())
            } else {
                Some(shader::Action::publish(msg))
            }
        } else if cursor_in_bounds {
            Some(shader::Action::capture())
        } else {
            None
        }
    }

    fn draw(
        &self,
        state: &Self::State,
        _cursor: mouse::Cursor,
        bounds: Rectangle,
    ) -> Self::Primitive {
        let snap = &self.snapshot;

        // Read volume_scale from widget state (updated locally during drag)
        // for immediate visual feedback, falling back to snapshot value.
        let live_volume_scale = state
            .chart_state
            .as_ref()
            .map(|cs| cs.volume_scale)
            .unwrap_or(snap.volume_scale);

        // If no data, return an empty primitive.
        let data = match &snap.data {
            Some(d) => d,
            None => {
                return ChartPrimitive {
                    chart_id: self.chart_id,
                    scene: None,
                    viewport_width: bounds.width as u32,
                    viewport_height: bounds.height as u32,
                    background_color: dark_theme().background,
                    volume_scale: 1.0,
                };
            }
        };

        let theme = dark_theme();

        // Use actual widget bounds for the camera viewport so the
        // interaction layer's bounds check matches the real widget size.
        let mut camera = snap.camera.clone();
        camera.viewport_width = bounds.width.max(1.0) as u32;
        camera.viewport_height = bounds.height.max(1.0) as u32;

        // Build the clean input contract for chart scene computation.
        let input = ChartInput {
            symbol: &snap.symbol,
            data: data.as_ref(),
            camera: &camera,
            viewport_width: camera.viewport_width,
            viewport_height: camera.viewport_height,
            dpi_scale: camera.dpi_scale,
            background_color: theme.background,
            bull_color: theme.bull,
            bear_color: theme.bear,
            volume_bull_color: theme.volume_bull,
            volume_bear_color: theme.volume_bear,
            grid_color: theme.grid,
            crosshair: snap.crosshair_pos,
            levels: &snap.levels,
            collapse_gaps: snap.collapse_gaps,
            volume_scale: live_volume_scale,
            dirty: &snap.dirty,
        };

        let scene = compute_chart_scene(&input);

        ChartPrimitive {
            chart_id: self.chart_id,
            scene: Some(scene),
            viewport_width: bounds.width as u32,
            viewport_height: bounds.height as u32,
            background_color: theme.background,
            volume_scale: live_volume_scale,
        }
    }

    fn mouse_interaction(
        &self,
        state: &Self::State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        // While actively dragging the volume handle, always show resize cursor.
        if let Some(cs) = &state.chart_state {
            if matches!(
                cs.interaction_mode,
                InteractionMode::DraggingVolumeScale { .. }
            ) {
                return mouse::Interaction::ResizingVertically;
            }
        }

        if let Some(pos) = cursor.position_in(bounds) {
            // Hit-test the volume scale handle triangle zone.
            let vol_scale = state.chart_state.as_ref()
                .map(|cs| cs.volume_scale)
                .unwrap_or(1.0);
            let handle_y = midas_chart::volume_handle_y(
                bounds.height as u32,
                vol_scale,
            );
            let half_h = 7.0 + 8.0; // VOLUME_HANDLE_HEIGHT/2 + HIT_PADDING
            let x_min = bounds.width - 10.0 - 8.0; // HANDLE_WIDTH + HIT_PADDING
            if pos.x >= x_min
                && pos.y >= handle_y - half_h
                && pos.y <= handle_y + half_h
            {
                return mouse::Interaction::ResizingVertically;
            }
            mouse::Interaction::Crosshair
        } else {
            mouse::Interaction::default()
        }
    }
}

// ── ChartPrimitive ───────────────────────────────────────────────────

/// Per-frame rendering data passed from `Program::draw()` to the GPU.
///
/// Wraps a `ChartScene` (the midas-chart framework-agnostic IR) for
/// GPU upload and rendering.
#[allow(dead_code)]
pub struct ChartPrimitive {
    /// Chart identity for per-chart GPU resource lookup.
    pub chart_id: ChartId,
    /// The computed chart scene (None if no data).
    pub scene: Option<ChartScene>,
    /// Viewport dimensions from iced layout bounds.
    pub viewport_width: u32,
    pub viewport_height: u32,
    /// Background color for clear.
    pub background_color: [f32; 4],
    /// Volume scale for handle position in prepare().
    pub volume_scale: f32,
}

impl std::fmt::Debug for ChartPrimitive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChartPrimitive")
            .field("chart_id", &self.chart_id)
            .field("has_scene", &self.scene.is_some())
            .field("viewport", &(self.viewport_width, self.viewport_height))
            .finish()
    }
}

// ── ChartGpuResources ────────────────────────────────────────────────

/// Per-chart GPU state: renderer, dirty tracker, and cached instance data.
///
/// Lives inside the `ChartPipeline` in a HashMap keyed by `ChartId`.
/// Each chart owns its own `ChartRenderer` so that multiple charts
/// have independent GPU buffers and do not overwrite each other.
struct ChartGpuResources {
    /// Per-chart wgpu renderer with its own GPU pipelines and buffers.
    renderer: ChartRenderer,
    /// Tracks which generations have been uploaded to the GPU.
    tracker: DirtyTracker,
    /// Cached candle instances for re-use when not dirty.
    candles: Vec<CandleInstance>,
    /// Cached volume instances.
    volumes: Vec<VolumeInstance>,
    /// Cached grid line instances.
    grid_lines: Vec<GridLineInstance>,
    /// Cached crosshair overlay lines (0 or 2 instances).
    crosshair_lines: Vec<GridLineInstance>,
}

impl ChartGpuResources {
    fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        Self {
            renderer: ChartRenderer::new(device, format),
            tracker: DirtyTracker::new(),
            candles: Vec::new(),
            volumes: Vec::new(),
            grid_lines: Vec::new(),
            crosshair_lines: Vec::new(),
        }
    }
}

// ── ChartPipeline (iced Pipeline) ────────────────────────────────────

/// Shared GPU pipeline state for all chart widgets.
///
/// Created once by iced via `Pipeline::new()`. Each chart gets its own
/// `ChartRenderer` (with independent GPU buffers) inside `ChartGpuResources`,
/// so multiple charts render independently without overwriting each other.
pub struct ChartPipeline {
    /// Texture format for creating new per-chart renderers on demand.
    texture_format: wgpu::TextureFormat,
    /// Per-chart GPU state (renderer, dirty tracker, cached instances).
    chart_resources: HashMap<ChartId, ChartGpuResources>,
}

impl shader::Pipeline for ChartPipeline {
    fn new(
        _device: &wgpu::Device,
        _queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
    ) -> Self {
        tracing::info!("Creating ChartPipeline with format {:?}", format);
        Self {
            texture_format: format,
            chart_resources: HashMap::new(),
        }
    }
}

impl shader::Primitive for ChartPrimitive {
    type Pipeline = ChartPipeline;

    fn prepare(
        &self,
        pipeline: &mut Self::Pipeline,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _bounds: &Rectangle,
        _viewport: &Viewport,
    ) {
        let scene = match &self.scene {
            Some(s) => s,
            None => return,
        };

        // Get or create per-chart GPU resources (each chart owns its own renderer).
        let format = pipeline.texture_format;
        let resources = pipeline
            .chart_resources
            .entry(self.chart_id)
            .or_insert_with(|| ChartGpuResources::new(device, format));

        // Update cached instance data from the scene.
        if let Some(ref candles) = scene.candles {
            resources.candles = candles.clone();
        }
        if let Some(ref volumes) = scene.volumes {
            resources.volumes = volumes.clone();
        }

        // ── Build all grid lines ─────────────────────────────────────
        let vw = self.viewport_width as f32;
        let vh = self.viewport_height as f32;
        let sep_y = scene.separator_y;

        // 1. Horizontal price grid lines (from scene, price-only).
        //    scene.grid_lines now contains ONLY horizontal price lines
        //    (vertical time lines removed from compute_grid_lines).
        resources.grid_lines = Vec::new();
        for gl in &scene.grid_lines {
            if gl.position < 0.0 || gl.position > sep_y {
                continue;
            }
            let (color, thickness) = if gl.is_major {
                ([0.40, 0.40, 0.45, 0.14], 1.0_f32)
            } else {
                ([0.30, 0.30, 0.35, 0.07], 0.5_f32)
            };
            resources.grid_lines.push(GridLineInstance {
                rect: [0.0, gl.position, vw, gl.position + thickness],
                color,
            });
        }

        // 2. Separator line between price and volume areas.
        resources.grid_lines.push(GridLineInstance {
            rect: [0.0, sep_y, vw, sep_y + 1.0],
            color: [0.45, 0.45, 0.50, 0.35],
        });

        // 3. Vertical date lines (from date_labels module).
        for dl in &scene.date_labels {
            let (color, thickness) = if dl.is_boundary {
                ([0.45, 0.45, 0.50, 0.25], 1.0_f32)
            } else {
                ([0.30, 0.30, 0.35, 0.12], 0.5_f32)
            };
            resources.grid_lines.push(GridLineInstance {
                rect: [dl.screen_x, 0.0, dl.screen_x + thickness, sep_y],
                color,
            });
        }

        // 4. Session boundary lines (collapsed mode only).
        //    Skip boundaries that overlap with a date-label boundary line
        //    (both fire at day transitions, producing a double line).
        //    The session boundary sits at the midpoint between two candles
        //    while the date label sits at the candle center, so the gap
        //    equals half a candle slot — use full slot width as tolerance.
        let slot_tol = if scene.candle_count > 1 {
            vw / scene.candle_count as f32
        } else {
            40.0
        };
        for sb in &scene.session_boundaries {
            let dominated = scene.date_labels.iter().any(|dl| {
                dl.is_boundary && (dl.screen_x - sb.x).abs() < slot_tol
            });
            if dominated {
                continue;
            }
            resources.grid_lines.push(GridLineInstance {
                rect: [sb.x, 0.0, sb.x + 1.0, sep_y],
                color: sb.color,
            });
        }

        // Convert crosshair into two full-width GridLineInstance rectangles.
        resources.crosshair_lines = if let Some(ref ch) = scene.crosshair {
            crosshair_to_instances(ch, vw, vh)
        } else {
            Vec::new()
        };

        // Render the volume scale handle triangle on the right edge.
        // Position tracks the effective volume area top based on volume_scale.
        {
            let handle_y = midas_chart::volume_handle_y(
                self.viewport_height,
                self.volume_scale,
            );
            let half_h: f32 = 7.0;   // VOLUME_HANDLE_HEIGHT / 2
            let tri_width: f32 = 10.0; // VOLUME_HANDLE_WIDTH
            let color = [0.55, 0.55, 0.55, 0.35];
            let num_slices = (half_h * 2.0) as i32;
            for i in 0..num_slices {
                let y = handle_y - half_h + i as f32;
                let dist = ((i as f32 + 0.5) - half_h).abs();
                let w = (1.0 - dist / half_h) * tri_width;
                if w <= 0.0 {
                    continue;
                }
                resources.crosshair_lines.push(GridLineInstance {
                    rect: [vw - w, y, vw, y + 1.0],
                    color,
                });
            }
        }

        // Build the render scene from cached data.
        let dirty = DirtyFlags {
            camera: scene.generations.camera,
            candles: scene.generations.candles,
            grid: scene.generations.grid,
            levels: scene.generations.levels,
            crosshair: scene.generations.crosshair,
            theme: scene.generations.theme,
            ..DirtyFlags::default()
        };

        let render_scene = RenderScene {
            projection: scene.projection,
            candles: &resources.candles,
            volumes: &resources.volumes,
            grid_lines: &resources.grid_lines,
            crosshair_lines: &resources.crosshair_lines,
            dirty: &dirty,
        };

        // Let the per-chart ChartRenderer upload to GPU buffers.
        resources.renderer.render_prepare(
            device,
            queue,
            &render_scene,
            &mut resources.tracker,
        );
    }

    fn draw(
        &self,
        pipeline: &Self::Pipeline,
        render_pass: &mut wgpu::RenderPass<'_>,
    ) -> bool {
        if self.scene.is_none() {
            return false;
        }

        // Look up the per-chart renderer. If it doesn't exist yet
        // (prepare() hasn't run for this chart), skip drawing.
        let resources = match pipeline.chart_resources.get(&self.chart_id) {
            Some(r) => r,
            None => return false,
        };

        // SAFETY: iced stores the Pipeline in heap-allocated Storage that
        // outlives the render pass. The wgpu draw methods need the pipeline
        // reference lifetime to match the render pass lifetime, but the
        // Primitive::draw trait signature uses independent lifetimes. We
        // extend the pipeline reference lifetime to match the render pass.
        // This is safe because the Pipeline is guaranteed to outlive the
        // render pass by iced's architecture (Pipeline lives in Storage,
        // which is only dropped after all rendering is complete).
        let renderer: &ChartRenderer = &resources.renderer;
        let renderer: &ChartRenderer =
            unsafe { &*(renderer as *const ChartRenderer) };
        renderer.draw_pass(render_pass);
        true
    }
}

// ── Event translation ────────────────────────────────────────────────

/// Translate an iced `Event` into zero or more `ChartEvent`s.
///
/// Only mouse events within the chart bounds are translated.
fn translate_event(
    event: &Event,
    bounds: Rectangle,
    cursor: mouse::Cursor,
) -> Vec<ChartEvent> {
    match event {
        Event::Mouse(mouse_event) => {
            translate_mouse_event(mouse_event, bounds, cursor)
        }
        _ => Vec::new(),
    }
}

/// Translate an iced mouse event to chart-local coordinates.
///
/// For move and release events, we compute widget-local coordinates even
/// when the cursor is outside the widget bounds. This allows drag operations
/// (pan, scale) to continue tracking smoothly when the cursor leaves the
/// chart area or the application window.
fn translate_mouse_event(
    event: &mouse::Event,
    bounds: Rectangle,
    cursor: mouse::Cursor,
) -> Vec<ChartEvent> {
    // position_in returns None when cursor is outside bounds.
    // For drags we need coords even outside bounds, so compute manually.
    let pos_in_bounds = cursor.position_in(bounds);
    let pos_unclamped = cursor.position().map(|p| iced::Point {
        x: p.x - bounds.x,
        y: p.y - bounds.y,
    });

    match event {
        mouse::Event::CursorMoved { .. } => {
            // Use unclamped position so drags continue outside bounds.
            if let Some(p) = pos_unclamped {
                vec![ChartEvent::MouseMoved { x: p.x, y: p.y }]
            } else {
                vec![ChartEvent::MouseMoved { x: -1.0, y: -1.0 }]
            }
        }

        mouse::Event::ButtonPressed(button) => {
            // Only start interactions when clicking INSIDE the chart.
            if let Some(p) = pos_in_bounds {
                let button = translate_mouse_button(*button);
                vec![ChartEvent::MousePressed {
                    x: p.x,
                    y: p.y,
                    button,
                }]
            } else {
                Vec::new()
            }
        }

        mouse::Event::ButtonReleased(button) => {
            // Release must ALWAYS be delivered so the interaction state
            // machine exits Panning / Scaling — even when the cursor has
            // left the application window entirely (cursor.position() = None).
            let p = pos_unclamped.unwrap_or(iced::Point { x: -1.0, y: -1.0 });
            let button = translate_mouse_button(*button);
            vec![ChartEvent::MouseReleased {
                x: p.x,
                y: p.y,
                button,
            }]
        }

        mouse::Event::WheelScrolled { delta } => {
            if let Some(p) = pos_in_bounds {
                let scroll_delta = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => *y,
                    mouse::ScrollDelta::Pixels { y, .. } => *y / 50.0,
                };
                if scroll_delta.abs() > f32::EPSILON {
                    vec![ChartEvent::MouseWheel {
                        delta: scroll_delta,
                        x: p.x,
                        y: p.y,
                    }]
                } else {
                    Vec::new()
                }
            } else {
                Vec::new()
            }
        }

        _ => Vec::new(),
    }
}

/// Map iced mouse button to chart mouse button.
fn translate_mouse_button(button: mouse::Button) -> midas_chart::MouseButton {
    match button {
        mouse::Button::Left => midas_chart::MouseButton::Left,
        mouse::Button::Right => midas_chart::MouseButton::Right,
        mouse::Button::Middle => midas_chart::MouseButton::Middle,
        _ => midas_chart::MouseButton::Left,
    }
}

// ── Action to Message conversion ─────────────────────────────────────

/// Convert a `ChartAction` into an application `Message`.
///
/// Some actions (like `SetCrosshair`) are handled locally by the widget
/// state and do not need to produce a message. Others (like `Pan`, `Zoom`)
/// must be propagated to the application to update the canonical `ChartPanel`.
///
/// The `camera` parameter is the widget's local camera with accurate
/// viewport dimensions (from the actual widget bounds). It is used to
/// convert pixel coordinates (center_x, center_y) into data-space values
/// (pivot_time, pivot_price) so the app handler does not need to know the
/// widget's viewport size.
fn action_to_message(
    chart_id: ChartId,
    action: &midas_chart::ChartAction,
    camera: &Camera2D,
) -> Option<Message> {
    use midas_chart::ChartAction;

    match action {
        ChartAction::Pan { dx, dy } => {
            Some(Message::ChartPan(chart_id, *dx, *dy))
        }
        ChartAction::Zoom { center_x, factor } => {
            // Convert pixel X to data-space time using the widget's camera
            // which has the correct viewport dimensions from actual bounds.
            let pivot_time = camera.x_to_time(*center_x);
            Some(Message::ChartZoom(chart_id, pivot_time, *factor))
        }
        ChartAction::ZoomY { center_y, factor } => {
            // Convert pixel Y to data-space price.
            let pivot_price = camera.y_to_price(*center_y);
            Some(Message::ChartZoomY(chart_id, pivot_price, *factor))
        }
        ChartAction::SetCrosshair { x, y } => {
            Some(Message::ChartCrosshair(chart_id, Some((*x, *y))))
        }
        ChartAction::ClearCrosshair => {
            Some(Message::ChartCrosshair(chart_id, None))
        }
        ChartAction::CreateLevel { price } => {
            Some(Message::ChartCreateLevel(chart_id, *price))
        }
        ChartAction::SetVolumeScale { scale } => {
            Some(Message::ChartSetVolumeScale(chart_id, *scale))
        }
        ChartAction::Redraw => None,
        _ => None,
    }
}

// ── Crosshair line conversion ─────────────────────────────────────────

/// Convert a `CrosshairRender` into two full-width `GridLineInstance`s.
///
/// - **Vertical line**: 1px wide, spans the entire viewport height at the
///   snapped candle X position.
/// - **Horizontal line**: 1px tall, spans the entire viewport width at the
///   cursor Y position.
fn crosshair_to_instances(
    ch: &CrosshairRender,
    viewport_width: f32,
    viewport_height: f32,
) -> Vec<GridLineInstance> {
    let color = ch.line_color;
    vec![
        // Vertical line: from top to bottom of chart.
        GridLineInstance {
            rect: [ch.vertical_x, 0.0, ch.vertical_x + 1.0, viewport_height],
            color,
        },
        // Horizontal line: from left to right of chart.
        GridLineInstance {
            rect: [0.0, ch.horizontal_y, viewport_width, ch.horizontal_y + 1.0],
            color,
        },
    ]
}

// ── Convenience constructor ──────────────────────────────────────────

/// Create a `Shader` widget for a chart panel.
pub fn chart_shader(program: ChartProgram) -> iced::widget::Shader<Message, ChartProgram> {
    iced::widget::Shader::new(program)
        .width(iced::Fill)
        .height(iced::Fill)
}
