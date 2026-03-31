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
use iced::{mouse, Event, Rectangle};

use midas_chart::camera::Camera2D;
use midas_chart::dirty::{DirtyFlags, DirtyTracker};
use midas_chart::input::ChartInput;
use midas_chart::interaction::{handle_event, ChartEvent};
use midas_chart::level_tool::LevelTool;
use midas_chart::levels::HorizontalLevel;
use midas_chart::scene::ChartScene;
use midas_chart::state::{ChartState, InteractionMode};
use midas_chart::{
    compute_chart_scene, CandleInstance, CrosshairRender, GridLineInstance, VolumeInstance,
};
use midas_core::ChartId;
use midas_data::CandleBuffer;
use midas_render::color::dark_theme;
use midas_render::renderer::ChartScene as RenderScene;
use midas_render::ChartRenderer;

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
    /// Timeline border position (fraction of viewport for volume area).
    pub timeline_border_ratio: f32,
    /// Volume bar height multiplier (1.0 = default).
    pub volume_scale: f32,
    /// Whether the Volume Profile overlay is visible.
    pub show_volume_profile: bool,
    /// Whether horizontal price levels are visible.
    pub show_levels: bool,
    /// Data time bounds for scroll clamping (first candle timestamp ms).
    pub data_time_start: f64,
    /// Data time bounds for scroll clamping (last candle timestamp ms).
    pub data_time_end: f64,
    /// ID of the level currently being edited (for highlight/selection).
    pub editing_level_id: Option<u64>,
    /// Self-contained level tool state (placement, drag, OHLC snap).
    pub level_tool: LevelTool,
    /// Whether level placement mode is globally active (all charts).
    pub level_placing: bool,
    /// Ghost crosshair from a sibling chart (same symbol, different chart).
    /// `(pixel_x, pixel_y)` — vertical + horizontal dim lines. `None` if inactive.
    pub ghost_crosshair: Option<(f32, f32)>,
    /// Ghost preview price from a sibling chart (same symbol, different chart).
    /// Rendered as a dim preview line. `None` if not applicable.
    pub ghost_preview_price: Option<f64>,
    /// Which chart currently has the placing cursor. Used to clear stale
    /// previews on non-source charts (handles cross-window cursor jumps).
    pub placing_cursor_chart: Option<ChartId>,
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
    /// Current keyboard modifier state (for Alt detection during level placement).
    modifiers: iced::keyboard::Modifiers,
    /// Set when the interaction layer cancels the level tool this frame.
    /// Prevents the snapshot sync from reverting the cancel before the
    /// message round-trips to the app.
    tool_cancelled_this_frame: bool,
    /// Level price override during drag for immediate visual feedback.
    /// `(level_id, new_price)` — applied to the snapshot levels copy
    /// when building ChartInput. Cleared when drag ends.
    drag_price_override: Option<(u64, f64)>,
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
        // Sync global placement state → per-widget level tool.
        // Don't interfere with dragging (drag is always local).
        if !chart_state.level_tool.is_dragging() {
            if self.snapshot.level_placing
                && !chart_state.level_tool.is_placing()
                && !state.tool_cancelled_this_frame
            {
                chart_state.level_tool.activate();
            } else if !self.snapshot.level_placing && chart_state.level_tool.is_placing() {
                chart_state.level_tool.cancel();
            }
        }
        // Clear cancel-guard once the app has caught up.
        if !self.snapshot.level_placing {
            state.tool_cancelled_this_frame = false;
        }
        chart_state.timeline_border_ratio = self.snapshot.timeline_border_ratio;
        chart_state.volume_scale = self.snapshot.volume_scale;
        chart_state.show_volume_profile = self.snapshot.show_volume_profile;
        chart_state.show_levels = self.snapshot.show_levels;

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
                chart_state.level_tool.cancel();
                chart_state.drag_start = None;
                chart_state.crosshair.force_hide();
                #[allow(deprecated)]
                {
                    chart_state.left_mouse_down = false;
                    chart_state.crosshair_pos = None;
                }

                state.last_viewport = Some(new_vp);
                return Some(shader::Action::publish(Message::ChartViewportChanged(
                    self.chart_id,
                    old_vp.0,
                    old_vp.1,
                    new_vp.0,
                    new_vp.1,
                )));
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
                return Some(shader::Action::publish(Message::ChartViewportChanged(
                    self.chart_id,
                    snap_vp.0,
                    snap_vp.1,
                    new_vp.0,
                    new_vp.1,
                )));
            }
        }

        // Track modifier keys for Alt detection during level placement.
        if let Event::Keyboard(iced::keyboard::Event::ModifiersChanged(mods)) = event {
            state.modifiers = *mods;
        }

        // Convert iced event to ChartEvent(s).
        let alt_held = state.modifiers.alt();
        let chart_events = translate_event(event, bounds, cursor, alt_held);
        if chart_events.is_empty() {
            return None;
        }

        let cursor_in_bounds = cursor.is_over(bounds);

        let mut messages: Vec<Message> = Vec::new();
        let mut captured = false;

        for chart_event in chart_events {
            let data_ref = self
                .snapshot
                .data
                .as_ref()
                .map(|d| d.as_ref() as &dyn midas_core::CandleData);
            let is_collapsed = self.snapshot.collapse_gaps;
            let actions = handle_event(
                chart_state,
                chart_event,
                data_ref,
                is_collapsed,
                &self.snapshot.levels,
            );
            for action in &actions {
                // Apply volume scale locally so the triangle moves
                // immediately during drag (before the message round-trip
                // through the app updates the canonical state).
                if let midas_chart::ChartAction::SetTimelineBorderRatio { ratio } = action {
                    chart_state.timeline_border_ratio = *ratio as f32;
                }
                if let midas_chart::ChartAction::SetVolumeScale { scale } = action {
                    chart_state.volume_scale = *scale as f32;
                }
                // Track when the interaction layer cancels the level tool
                // so the snapshot sync doesn't revert it.
                if matches!(action, midas_chart::ChartAction::CancelPlacing) {
                    state.tool_cancelled_this_frame = true;
                }
                // Store drag price override for immediate visual feedback
                // (before the message round-trip through the app).
                if let midas_chart::ChartAction::DragLevel { id, new_price } = action {
                    state.drag_price_override = Some((*id, *new_price));
                }
                // Clear drag override when drag ends.
                if !chart_state.level_tool.is_dragging() {
                    state.drag_price_override = None;
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
        // Shader widget can only publish ONE message per update(). Wrap
        // multiple messages in a ChartBatch so none are dropped.
        let msg = match messages.len() {
            0 => None,
            1 => Some(messages.into_iter().next().unwrap()),
            _ => Some(Message::ChartBatch(messages)),
        };
        if let Some(msg) = msg {
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

        // Read timeline_border_ratio and volume_scale from widget state
        // (updated locally during drag) for immediate visual feedback,
        // falling back to snapshot values.
        let live_timeline_border_ratio = state
            .chart_state
            .as_ref()
            .map(|cs| cs.timeline_border_ratio)
            .unwrap_or(snap.timeline_border_ratio);
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
                    timeline_border_ratio: 0.20,
                    volume_scale: 1.0,
                    ghost_preview_y: None,
                    ghost_crosshair: None,
                };
            }
        };

        let theme = dark_theme();

        // Use actual widget bounds for the camera viewport so the
        // interaction layer's bounds check matches the real widget size.
        let mut camera = snap.camera.clone();
        camera.viewport_width = bounds.width.max(1.0) as u32;
        camera.viewport_height = bounds.height.max(1.0) as u32;

        // When timeline_border_ratio or volume_scale is being dragged, the
        // local value diverges from the snapshot. Bump grid + crosshair
        // generations so the renderer re-uploads those buffers.
        let mut dirty = snap.dirty.clone();
        if (live_timeline_border_ratio - snap.timeline_border_ratio).abs() > f32::EPSILON {
            dirty.grid += 1;
        }
        if (live_volume_scale - snap.volume_scale).abs() > f32::EPSILON {
            dirty.grid += 1;
        }

        // When the local crosshair state differs from the snapshot (widget
        // updated this frame but the message hasn't round-tripped yet), bump
        // the crosshair generation so the renderer re-uploads the buffer.
        let local_crosshair = state
            .chart_state
            .as_ref()
            .and_then(|cs| cs.crosshair.render_pos());
        if local_crosshair != snap.crosshair_pos {
            dirty.crosshair += 1;
        }

        // Build levels with drag override applied for immediate visual feedback.
        let effective_levels: Vec<midas_chart::HorizontalLevel> = if snap.show_levels {
            if let Some((drag_id, drag_price)) = state.drag_price_override {
                snap.levels
                    .iter()
                    .map(|l| {
                        if l.id == drag_id {
                            let mut clone = l.clone();
                            clone.price = drag_price;
                            clone
                        } else {
                            l.clone()
                        }
                    })
                    .collect()
            } else {
                snap.levels.clone()
            }
        } else {
            Vec::new()
        };

        // Build the level tool for chart scene computation.
        // If this chart is NOT the source of the placing cursor, clear
        // preview_price to avoid stale previews (handles cross-window jumps).
        let is_placing_source = snap
            .placing_cursor_chart
            .map_or(true, |src| src == self.chart_id);
        let mut effective_level_tool = state
            .chart_state
            .as_ref()
            .map(|cs| cs.level_tool.clone())
            .unwrap_or_default();
        if !is_placing_source {
            effective_level_tool.preview_price = None;
        }

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
            crosshair: state
                .chart_state
                .as_ref()
                .and_then(|cs| cs.crosshair.render_pos())
                .or(snap.crosshair_pos),
            levels: &effective_levels,
            collapse_gaps: snap.collapse_gaps,
            timeline_border_ratio: live_timeline_border_ratio,
            volume_scale: live_volume_scale,
            show_volume_profile: snap.show_volume_profile,
            dirty: &dirty,
            level_tool: &effective_level_tool,
        };

        let scene = compute_chart_scene(&input);

        // Pre-compute ghost preview Y from sibling chart price.
        let ghost_preview_y = snap.ghost_preview_price.map(|price| {
            camera.snap_to_pixel(camera.price_to_y(price))
        });

        // Pre-compute ghost crosshair from sibling chart. Filter off-screen.
        let ghost_crosshair = snap.ghost_crosshair.and_then(|(gx, gy)| {
            let sx = camera.snap_to_pixel(gx);
            if sx >= 0.0 && sx <= camera.viewport_width as f32 {
                Some((sx, gy))
            } else {
                None
            }
        });

        ChartPrimitive {
            chart_id: self.chart_id,
            scene: Some(scene),
            viewport_width: bounds.width as u32,
            viewport_height: bounds.height as u32,
            background_color: theme.background,
            timeline_border_ratio: live_timeline_border_ratio,
            volume_scale: live_volume_scale,
            ghost_preview_y,
            ghost_crosshair,
        }
    }

    fn mouse_interaction(
        &self,
        state: &Self::State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if let Some(cs) = &state.chart_state {
            // While actively dragging a level, volume handle, or timeline border,
            // always show vertical resize cursor.
            if matches!(
                cs.interaction_mode,
                InteractionMode::DraggingVolumeScale { .. }
                    | InteractionMode::DraggingTimelineBorder { .. }
            ) || cs.level_tool.is_dragging()
            {
                return mouse::Interaction::ResizingVertically;
            }
        }

        if let Some(pos) = cursor.position_in(bounds) {
            // Check volume handle triangle.
            let vol_scale = state
                .chart_state
                .as_ref()
                .map(|cs| cs.volume_scale)
                .unwrap_or(1.0);
            let handle_y = midas_chart::volume_handle_y(bounds.height as u32, vol_scale);
            let half_h = 7.0 + 8.0;
            let x_min = bounds.width - 10.0 - 8.0;
            if pos.x >= x_min && pos.y >= handle_y - half_h && pos.y <= handle_y + half_h {
                return mouse::Interaction::ResizingVertically;
            }

            // Check timeline border line (full width, generous tolerance).
            let border_ratio = state
                .chart_state
                .as_ref()
                .map(|cs| cs.timeline_border_ratio)
                .unwrap_or(0.20);
            let border_y = midas_chart::timeline_border_y(bounds.height as u32, border_ratio);
            if (pos.y - border_y).abs() <= 6.0 {
                return mouse::Interaction::ResizingVertically;
            }

            // Check horizontal levels — show resize cursor on non-locked levels.
            if let Some(cs) = &state.chart_state {
                for level in &self.snapshot.levels {
                    if level.locked {
                        continue;
                    }
                    let level_y = cs.camera.price_to_y(level.price);
                    if (pos.y - level_y).abs() <= 6.0 {
                        return mouse::Interaction::ResizingVertically;
                    }
                }
            }

            // Hide OS cursor when our custom crosshair is active (left mouse held).
            let crosshair_active = state
                .chart_state
                .as_ref()
                .map(|cs| cs.crosshair.should_render())
                .unwrap_or(false);
            if crosshair_active {
                mouse::Interaction::Hidden
            } else {
                mouse::Interaction::default()
            }
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
    /// Timeline border ratio for separator line position in prepare().
    pub timeline_border_ratio: f32,
    /// Volume scale for handle position in prepare().
    pub volume_scale: f32,
    /// Ghost preview line Y from a sibling chart (same symbol, different chart).
    pub ghost_preview_y: Option<f32>,
    /// Ghost crosshair from a sibling chart `(pixel_x, pixel_y)`.
    pub ghost_crosshair: Option<(f32, f32)>,
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
    /// Cached Volume Profile histogram bars.
    volume_profile_instances: Vec<GridLineInstance>,
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
            volume_profile_instances: Vec::new(),
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
    fn new(_device: &wgpu::Device, _queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
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

        // Grid instances are fully built in compute_chart_scene().
        let vw = self.viewport_width as f32;
        let vh = self.viewport_height as f32;
        resources.grid_lines = scene.grid_instances.clone();

        // Convert horizontal price levels into full-width grid line instances.
        for level in &scene.levels {
            let y = level.screen_y;
            let thickness = level.line_width.max(1.0);
            resources.grid_lines.push(GridLineInstance {
                rect: [0.0, y, vw, y + thickness],
                color: level.color,
            });
            // Selection highlight: thicker semi-transparent glow behind the line.
            if level.is_selected {
                let glow_thickness = thickness + 4.0;
                let glow_y = y - 2.0;
                resources.grid_lines.push(GridLineInstance {
                    rect: [0.0, glow_y, vw, glow_y + glow_thickness],
                    color: [
                        level.color[0],
                        level.color[1],
                        level.color[2],
                        level.color[3] * 0.3,
                    ],
                });
            }
            // Ghost line during drag (original position).
            if let Some(orig_y) = level.original_screen_y {
                resources.grid_lines.push(GridLineInstance {
                    rect: [0.0, orig_y, vw, orig_y + 1.0],
                    color: [
                        level.color[0],
                        level.color[1],
                        level.color[2],
                        level.color[3] * 0.2,
                    ],
                });
            }
        }

        // Preview level line during placement mode (thicker, colored).
        // Reads from scene.level_preview_y — independent of crosshair,
        // which is suppressed during placement.
        if let Some(preview_y) = scene.level_preview_y {
            let preview_color = [0.22, 0.55, 0.95, 0.7];
            // Solid preview line (2px, centered on preview_y).
            resources.grid_lines.push(GridLineInstance {
                rect: [0.0, preview_y - 1.0, vw, preview_y + 1.0],
                color: preview_color,
            });
            // Glow around preview line.
            resources.grid_lines.push(GridLineInstance {
                rect: [0.0, preview_y - 3.0, vw, preview_y + 3.0],
                color: [0.22, 0.55, 0.95, 0.2],
            });
        }

        // Ghost preview line on sibling charts (same symbol, different chart).
        // Dimmer than the active preview to distinguish source from ghost.
        if let Some(ghost_y) = self.ghost_preview_y {
            if ghost_y >= 0.0 && ghost_y <= vh {
                resources.grid_lines.push(GridLineInstance {
                    rect: [0.0, ghost_y - 0.5, vw, ghost_y + 0.5],
                    color: [0.22, 0.55, 0.95, 0.35],
                });
                resources.grid_lines.push(GridLineInstance {
                    rect: [0.0, ghost_y - 2.0, vw, ghost_y + 2.0],
                    color: [0.22, 0.55, 0.95, 0.1],
                });
            }
        }

        // Convert crosshair into two full-width GridLineInstance rectangles.
        resources.crosshair_lines = if let Some(ref ch) = scene.crosshair {
            crosshair_to_instances(ch, vw, vh)
        } else {
            Vec::new()
        };

        // Ghost crosshair from sibling chart (vertical + horizontal, dim).
        if let Some((gx, gy)) = self.ghost_crosshair {
            let ghost_color = [0.5, 0.5, 0.6, 0.25];
            // Vertical ghost line.
            resources.crosshair_lines.push(GridLineInstance {
                rect: [gx, 0.0, gx + 1.0, vh],
                color: ghost_color,
            });
            // Horizontal ghost line.
            if gy >= 0.0 && gy <= vh {
                resources.crosshair_lines.push(GridLineInstance {
                    rect: [0.0, gy, vw, gy + 1.0],
                    color: ghost_color,
                });
            }
        }

        // Render the separator handle triangle on the right edge.
        // Lives in the crosshair overlay layer (drawn on top of everything).
        {
            let handle_y = midas_chart::volume_handle_y(self.viewport_height, self.volume_scale);
            let half_h: f32 = 7.0; // VOLUME_HANDLE_HEIGHT / 2
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

        // Volume Profile instances (pre-computed in the chart scene).
        resources.volume_profile_instances = scene.volume_profile_instances.clone();

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
            volume_profile: &resources.volume_profile_instances,
            dirty: &dirty,
        };

        // Let the per-chart ChartRenderer upload to GPU buffers.
        resources
            .renderer
            .render_prepare(device, queue, &render_scene, &mut resources.tracker);
    }

    fn draw(&self, pipeline: &Self::Pipeline, render_pass: &mut wgpu::RenderPass<'_>) -> bool {
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
        let renderer: &ChartRenderer = unsafe { &*(renderer as *const ChartRenderer) };
        renderer.draw_pass(render_pass);
        true
    }
}

// ── Event translation ────────────────────────────────────────────────

/// Translate an iced `Event` into zero or more `ChartEvent`s.
///
/// Mouse events are translated to chart-local coordinates. Keyboard
/// events are translated to `ChartEvent::KeyPressed` variants so the
/// sans-IO interaction state machine can handle hotkeys (e.g. Escape to
/// cancel placement, H to enter placement mode).
fn translate_event(
    event: &Event,
    bounds: Rectangle,
    cursor: mouse::Cursor,
    alt_held: bool,
) -> Vec<ChartEvent> {
    match event {
        Event::Mouse(mouse_event) => translate_mouse_event(mouse_event, bounds, cursor, alt_held),
        Event::Keyboard(keyboard_event) => translate_keyboard_event(keyboard_event),
        _ => Vec::new(),
    }
}

/// Translate an iced keyboard event to chart events.
///
/// Maps named keys (Delete, Escape, Home, End) and character keys (H)
/// to `ChartEvent::KeyPressed` so the sans-IO interaction state machine
/// can handle keyboard shortcuts for level operations and navigation.
fn translate_keyboard_event(event: &iced::keyboard::Event) -> Vec<ChartEvent> {
    match event {
        iced::keyboard::Event::KeyPressed { key, .. } => {
            let chart_key = match key {
                iced::keyboard::Key::Named(iced::keyboard::key::Named::Delete)
                | iced::keyboard::Key::Named(iced::keyboard::key::Named::Backspace) => {
                    Some(midas_chart::Key::Delete)
                }
                iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape) => {
                    Some(midas_chart::Key::Escape)
                }
                iced::keyboard::Key::Named(iced::keyboard::key::Named::Home) => {
                    Some(midas_chart::Key::Home)
                }
                iced::keyboard::Key::Named(iced::keyboard::key::Named::End) => {
                    Some(midas_chart::Key::End)
                }
                iced::keyboard::Key::Character(c) if c.as_str() == "h" || c.as_str() == "H" => {
                    Some(midas_chart::Key::H)
                }
                _ => None,
            };
            chart_key
                .map(|k| vec![ChartEvent::KeyPressed { key: k }])
                .unwrap_or_default()
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
    alt_held: bool,
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
                vec![ChartEvent::MouseMoved {
                    x: p.x,
                    y: p.y,
                    alt_held,
                }]
            } else {
                vec![ChartEvent::MouseMoved {
                    x: -1.0,
                    y: -1.0,
                    alt_held,
                }]
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
                    alt_held,
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
                alt_held,
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
        ChartAction::Pan { dx, dy } => Some(Message::ChartPan(chart_id, *dx, *dy)),
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
        ChartAction::ClearCrosshair => Some(Message::ChartCrosshair(chart_id, None)),
        ChartAction::CreateLevel { price } => Some(Message::ChartCreateLevel(chart_id, *price)),
        ChartAction::SetTimelineBorderRatio { ratio } => {
            Some(Message::ChartSetTimelineBorderRatio(chart_id, *ratio))
        }
        ChartAction::SetVolumeScale { scale } => {
            Some(Message::ChartSetVolumeScale(chart_id, *scale))
        }
        ChartAction::RightClickLevel { id, x, y } => {
            Some(Message::ChartRightClickLevel(chart_id, *id, *x, *y))
        }
        ChartAction::DragLevel { id, new_price } => {
            Some(Message::ChartDragLevel(chart_id, *id, *new_price))
        }
        ChartAction::SelectLevel { id } => Some(Message::ChartSelectLevel(chart_id, *id)),
        ChartAction::DeselectLevel => Some(Message::ChartDeselectLevel(chart_id)),
        ChartAction::DeleteSelectedLevel => Some(Message::ChartDeleteSelectedLevel(chart_id)),
        ChartAction::CancelPlacing => Some(Message::ChartCancelPlacing(chart_id)),
        ChartAction::PlacingPreview { price } => {
            Some(Message::PlacingCursorMoved(chart_id, *price))
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
