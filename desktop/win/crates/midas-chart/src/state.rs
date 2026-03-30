//! ChartState -- per-chart pure state machine.
//!
//! Contains all mutable state for a single chart panel: camera, dirty flags,
//! horizontal levels, crosshair position, interaction mode, momentum,
//! and Y-axis animation. This is the single source of truth for what a
//! chart displays.

use crate::camera::Camera2D;
use crate::crosshair_tool::CrosshairTool;
use crate::dirty::DirtyFlags;
use crate::interaction::ChartAction;
use crate::level_tool::LevelTool;
use crate::levels::HorizontalLevel;

/// What the active tool needs from the crosshair.
///
/// The interaction layer queries this via [`ChartState::active_cursor_claim()`]
/// instead of checking each tool individually. Priority order:
/// `Suppress > Preview > None`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorClaim {
    /// No tool claims the cursor. Normal crosshair rules apply.
    None,
    /// A placement tool wants a preview line. Crosshair visible
    /// regardless of mouse button, Y may be snapped.
    Preview,
    /// An active drag/edit wants the crosshair hidden entirely.
    Suppress,
}

/// The interaction mode state machine.
///
/// Transitions:
/// - `Idle` -> `PendingDrag` on left mouse press
/// - `PendingDrag` -> `Panning` when mouse moves >= 4px from start
/// - `PendingDrag` -> `Idle` + `LevelTool::Dragging` when near a level
/// - `Panning` -> `Idle` on mouse release
/// - `Idle` -> `PendingScale` on middle mouse press
/// - `PendingScale` -> `HorizontalScaling` or `VerticalScaling` after 6px movement
/// - `HorizontalScaling` / `VerticalScaling` -> `Idle` on middle mouse release
/// - Any -> `Idle` on Escape key
#[derive(Clone, Debug, PartialEq)]
pub enum InteractionMode {
    /// No active interaction.
    Idle,
    /// Mouse button is down but we haven't exceeded the drag threshold.
    PendingDrag { start_x: f32, start_y: f32 },
    /// User is panning the chart via click+drag.
    Panning,
    /// Middle mouse is down but axis not yet determined (dead zone).
    PendingScale { start_x: f32, start_y: f32 },
    /// Horizontal scaling (time axis) via middle-drag.
    HorizontalScaling { anchor_x: f32, last_x: f32 },
    /// Vertical scaling (price axis) via middle-drag.
    VerticalScaling { anchor_y: f32, last_y: f32 },
    /// Right-click XY panning.
    RightPanning,
    /// Dragging the timeline border line up/down.
    DraggingTimelineBorder {
        /// Y pixel position at drag start.
        anchor_y: f32,
        /// Timeline border ratio value at drag start.
        start_ratio: f32,
    },
    /// Dragging the volume scale handle up/down.
    DraggingVolumeScale {
        /// Y pixel position at drag start.
        anchor_y: f32,
        /// Volume scale value at drag start.
        start_scale: f32,
    },
}

/// Momentum state for flick-to-scroll after a pan drag release.
///
/// Velocity is in data-space units per second (time ms/s for vx, price/s for vy).
/// The animation loop ticks this with `dt_secs` and applies exponential decay.
#[derive(Clone, Debug)]
pub struct Momentum {
    /// Velocity along the time axis (ms per second).
    pub vx: f64,
    /// Velocity along the price axis (price units per second).
    pub vy: f64,
}

impl Momentum {
    /// Friction coefficient for exponential decay. Higher = faster deceleration.
    /// 6.0 gives roughly a 0.5-second coast for a medium flick.
    const FRICTION: f64 = 6.0;

    /// Minimum velocity magnitude below which momentum stops.
    const MIN_VELOCITY: f64 = 0.01;
}

/// Y-axis auto-scale animation state.
///
/// When set, the camera's price_low/price_high are lerped toward
/// `target_low`/`target_high` each frame.
#[derive(Clone, Debug)]
pub struct YAnimation {
    /// Target price range bottom.
    pub target_low: f64,
    /// Target price range top.
    pub target_high: f64,
}

/// Complete mutable state for a single chart panel.
///
/// This is the Phase 3 version with full interaction support: state machine,
/// momentum, Y-axis animation, and level management.
#[derive(Clone, Debug)]
pub struct ChartState {
    /// Camera defining the visible time/price window and viewport size.
    pub camera: Camera2D,
    /// Generation-counter dirty flags.
    pub dirty: DirtyFlags,
    /// User-defined horizontal price levels.
    pub levels: Vec<HorizontalLevel>,
    /// Self-contained crosshair component.
    pub crosshair: CrosshairTool,
    /// Currently selected level ID, or `None` if no level is selected.
    pub selected_level: Option<u64>,
    /// Current interaction mode (state machine state).
    pub interaction_mode: InteractionMode,
    /// Last known mouse position for drag delta computation. Set on mouse press,
    /// updated on each mouse move during drag.
    pub drag_start: Option<(f32, f32)>,
    /// Active momentum animation, or `None` if not coasting.
    pub momentum: Option<Momentum>,
    /// Active Y-axis auto-scale animation, or `None` if not animating.
    pub y_animation: Option<YAnimation>,
    /// Data time bounds (first and last candle timestamps in ms).
    /// Used to clamp scroll pan so the user can't scroll past the data edges.
    /// Set by the app when data is loaded.
    pub data_time_start: f64,
    pub data_time_end: f64,
    /// Whether session gaps are collapsed (index-based X positioning).
    ///
    /// When `true`, candle X positions are based on their sequential index
    /// rather than timestamp, eliminating overnight/weekend gaps. A faint
    /// vertical separator is drawn at session boundaries.
    pub collapse_gaps: bool,
    /// Fraction of viewport height at which the timeline border line sits (0.0–1.0).
    ///
    /// Adjusted by dragging the timeline border line (full-width hit zone).
    /// Range: [`TIMELINE_BORDER_MIN`, `TIMELINE_BORDER_MAX`].
    pub timeline_border_ratio: f32,
    /// Volume bar height multiplier (1.0 = auto-normalized to visible range).
    ///
    /// Adjusted by dragging the triangle handle on the right edge.
    /// Range: [`VOLUME_SCALE_MIN`, `VOLUME_SCALE_MAX`].
    pub volume_scale: f32,
    /// Whether the Volume Profile overlay is visible.
    pub show_volume_profile: bool,
    /// Whether horizontal price levels are visible.
    pub show_levels: bool,
    /// Self-contained level tool state machine (Phase 2+).
    pub level_tool: LevelTool,
    /// Next level ID to assign (monotonically increasing).
    next_level_id: u64,
}

impl ChartState {
    /// Create a new `ChartState` with the given camera and default values.
    pub fn new(camera: Camera2D) -> Self {
        Self {
            camera,
            dirty: DirtyFlags::new(),
            levels: Vec::new(),
            crosshair: CrosshairTool::new(),
            selected_level: None,
            interaction_mode: InteractionMode::Idle,
            drag_start: None,
            momentum: None,
            y_animation: None,
            data_time_start: 0.0,
            data_time_end: f64::MAX,
            collapse_gaps: false,
            timeline_border_ratio: 0.20,
            volume_scale: 1.0,
            show_volume_profile: false,
            show_levels: true,
            level_tool: LevelTool::default(),
            next_level_id: 1,
        }
    }

    /// Allocate and return the next unique level ID.
    pub fn alloc_level_id(&mut self) -> u64 {
        let id = self.next_level_id;
        self.next_level_id += 1;
        id
    }

    /// Query the collective cursor requirement of all active tools.
    ///
    /// The interaction layer calls this instead of checking each tool
    /// individually. New tools only need to be added here.
    /// Priority: Suppress > Preview > None.
    pub fn active_cursor_claim(&self) -> CursorClaim {
        if self.level_tool.is_active() {
            return CursorClaim::Suppress;
        }
        CursorClaim::None
    }

    /// Fraction of the visible range allowed as padding beyond data edges.
    const PAN_EDGE_PADDING: f64 = 0.05;

    /// Minimum timeline border ratio (at least 5% of viewport).
    pub const TIMELINE_BORDER_MIN: f32 = 0.05;
    /// Maximum timeline border ratio (at most 80% of viewport).
    pub const TIMELINE_BORDER_MAX: f32 = 0.80;
    /// Minimum volume scale factor.
    pub const VOLUME_SCALE_MIN: f32 = 0.1;
    /// Maximum volume scale factor.
    pub const VOLUME_SCALE_MAX: f32 = 4.0;

    /// Clamp a horizontal pan delta so the camera can't scroll past the
    /// **beginning** of the data (left edge). The right edge is unclamped
    /// so the user can freely scroll forward past the last candle.
    fn clamp_pan_dx(&self, dx: f64) -> f64 {
        let visible_span = self.camera.time_end - self.camera.time_start;
        let padding = visible_span * Self::PAN_EDGE_PADDING;

        // Only clamp when panning backward (negative dx = moving left).
        if dx < 0.0 {
            let min_start = self.data_time_start - padding;
            let new_start = self.camera.time_start + dx;
            if new_start < min_start {
                // Reduce the magnitude so we stop exactly at the limit.
                let max_backward = min_start - self.camera.time_start;
                max_backward.min(0.0)
            } else {
                dx
            }
        } else {
            dx
        }
    }

    /// Apply a single `ChartAction` to mutate this state.
    ///
    /// This is the central state reducer. The interaction layer produces actions;
    /// the application layer calls this to commit them.
    pub fn apply_action(&mut self, action: &ChartAction) {
        match action {
            ChartAction::Pan { dx, dy } => {
                // Positive dx = move the visible window forward in time (right).
                let dx = self.clamp_pan_dx(*dx);
                self.camera.time_start += dx;
                self.camera.time_end += dx;
                self.camera.price_low += dy;
                self.camera.price_high += dy;
                self.dirty.mark_camera();
            }

            ChartAction::Zoom { center_x, factor } => {
                // Convert pixel X to time, then preserve the ratio: center_time
                // sits at the same fraction of the visible range after zoom.
                let center_time = self.camera.x_to_time(*center_x);
                let old_range = self.camera.time_end - self.camera.time_start;
                let ratio = if old_range.abs() > f64::EPSILON {
                    (center_time - self.camera.time_start) / old_range
                } else {
                    0.5
                };
                let new_range = old_range / factor;
                self.camera.time_start = center_time - ratio * new_range;
                self.camera.time_end = center_time + (1.0 - ratio) * new_range;
                self.dirty.mark_camera();
            }

            ChartAction::ZoomY { center_y, factor } => {
                // Vertical zoom: scale price range around the anchor Y pixel.
                let center_price = self.camera.y_to_price(*center_y);
                let old_range = self.camera.price_high - self.camera.price_low;
                let ratio = if old_range.abs() > f64::EPSILON {
                    (self.camera.price_high - center_price) / old_range
                } else {
                    0.5
                };
                let new_range = old_range / factor;
                self.camera.price_high = center_price + ratio * new_range;
                self.camera.price_low = center_price - (1.0 - ratio) * new_range;
                self.dirty.mark_camera();
            }

            ChartAction::SetCrosshair { x, y } => {
                self.crosshair.set_pos(*x, *y);
                self.dirty.mark_crosshair();
            }

            ChartAction::ClearCrosshair => {
                self.crosshair.force_hide();
                self.dirty.mark_crosshair();
            }

            ChartAction::AutoScaleY {
                target_low,
                target_high,
            } => {
                self.y_animation = Some(YAnimation {
                    target_low: *target_low,
                    target_high: *target_high,
                });
            }

            ChartAction::StartMomentum { vx, vy } => {
                self.momentum = Some(Momentum { vx: *vx, vy: *vy });
            }

            ChartAction::ApplyMomentum { dt, dp } => {
                // Positive dt = move the visible window forward in time (right).
                let dt = self.clamp_pan_dx(*dt);
                self.camera.time_start += dt;
                self.camera.time_end += dt;
                self.camera.price_low += dp;
                self.camera.price_high += dp;
                self.dirty.mark_camera();
            }

            ChartAction::StopMomentum => {
                self.momentum = None;
            }

            ChartAction::CreateLevel { price } => {
                let id = self.alloc_level_id();
                self.levels.push(HorizontalLevel {
                    id,
                    price: *price,
                    color: [0.85, 0.85, 0.85, 0.8], // gray (last palette color)
                    line_width: 1.0,
                    label: None,
                    icon: crate::levels::LevelIcon::None,
                    locked: false,
                });
                self.dirty.mark_levels();
            }

            ChartAction::SelectLevel { id } => {
                self.selected_level = Some(*id);
            }

            ChartAction::DragLevel { id, new_price } => {
                if let Some(level) = self.levels.iter_mut().find(|l| l.id == *id) {
                    level.price = *new_price;
                    self.dirty.mark_levels();
                }
            }

            ChartAction::DeleteSelectedLevel => {
                if let Some(sel_id) = self.selected_level {
                    let is_locked = self.levels.iter().any(|l| l.id == sel_id && l.locked);
                    if !is_locked {
                        self.selected_level = None;
                        self.levels.retain(|l| l.id != sel_id);
                        self.dirty.mark_levels();
                    }
                }
            }

            ChartAction::DeselectLevel => {
                self.selected_level = None;
            }

            ChartAction::RightClickLevel { id, .. } => {
                self.selected_level = Some(*id);
            }

            ChartAction::JumpToEnd => {
                let span = self.camera.time_end - self.camera.time_start;
                // This is a placeholder; in practice the caller would set
                // time_end to data_time_max. For now, shift is a no-op marker.
                // The application layer should handle this with actual data bounds.
                let _ = span;
            }

            ChartAction::JumpToStart => {
                let span = self.camera.time_end - self.camera.time_start;
                let _ = span;
            }

            ChartAction::SetTimelineBorderRatio { ratio } => {
                self.timeline_border_ratio = *ratio as f32;
                self.dirty.mark_data();
                self.dirty.grid += 1;
            }

            ChartAction::SetVolumeScale { scale } => {
                self.volume_scale = *scale as f32;
                self.dirty.mark_data();
            }

            ChartAction::Redraw => {
                // Mark everything dirty to force a full redraw.
                self.dirty.mark_all();
            }

            ChartAction::CancelPlacing => {
                self.level_tool.cancel();
                self.crosshair.force_hide();
                self.interaction_mode = InteractionMode::Idle;
                self.dirty.mark_crosshair();
            }
        }
    }

    /// Advance the momentum simulation by `dt` seconds.
    ///
    /// Applies exponential decay to the velocity and moves the camera.
    /// Returns `true` if momentum is still active, `false` if it has
    /// converged and been stopped.
    pub fn tick_momentum(&mut self, dt: f32) -> bool {
        let (vx, vy) = match &self.momentum {
            Some(m) => (m.vx, m.vy),
            None => return false,
        };

        let dt64 = dt as f64;
        let decay = (-Momentum::FRICTION * dt64).exp();

        // Displacement is the integral of v*e^(-f*t) from 0 to dt:
        //   = v * (1 - e^(-f*dt)) / f
        let factor = (1.0 - decay) / Momentum::FRICTION;
        let raw_dx = vx * factor;
        let dy = vy * factor;

        // Apply displacement to camera with left-edge clamping.
        let dx = self.clamp_pan_dx(raw_dx);
        self.camera.time_start += dx;
        self.camera.time_end += dx;
        self.camera.price_low += dy;
        self.camera.price_high += dy;
        self.dirty.mark_camera();

        // Decay velocity.
        let momentum = self.momentum.as_mut().unwrap();
        momentum.vx *= decay;
        momentum.vy *= decay;

        // Stop momentum if it was fully clamped (hit the edge).
        if dx.abs() < f64::EPSILON && momentum.vx.abs() > Momentum::MIN_VELOCITY {
            momentum.vx = 0.0;
        }

        // Stop when velocity is negligible.
        if momentum.vx.abs() < Momentum::MIN_VELOCITY && momentum.vy.abs() < Momentum::MIN_VELOCITY
        {
            self.momentum = None;
            false
        } else {
            true
        }
    }

    /// Advance the Y-axis auto-scale animation by `dt` seconds.
    ///
    /// Lerps `price_low` and `price_high` toward the target using
    /// exponential ease-out. Returns `true` if the animation is still
    /// active, `false` if it has converged and been removed.
    pub fn tick_auto_scale(&mut self, dt: f32) -> bool {
        let anim = match &self.y_animation {
            Some(a) => a.clone(),
            None => return false,
        };

        // Exponential ease-out: converge at ~12x per second.
        let t = 1.0 - (-12.0 * dt as f64).exp();

        self.camera.price_low += (anim.target_low - self.camera.price_low) * t;
        self.camera.price_high += (anim.target_high - self.camera.price_high) * t;
        self.dirty.mark_camera();

        // Check convergence: within 0.01 price units of target.
        let remaining = (anim.target_low - self.camera.price_low).abs()
            + (anim.target_high - self.camera.price_high).abs();

        if remaining < 0.01 {
            self.camera.price_low = anim.target_low;
            self.camera.price_high = anim.target_high;
            self.y_animation = None;
            false
        } else {
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_camera() -> Camera2D {
        Camera2D {
            time_start: 1_000_000.0,
            time_end: 2_000_000.0,
            price_low: 100.0,
            price_high: 200.0,
            viewport_width: 1920,
            viewport_height: 1080,
            dpi_scale: 1.0,
        }
    }

    #[test]
    fn new_state_has_defaults() {
        let state = ChartState::new(test_camera());
        assert!(state.levels.is_empty());
        assert_eq!(state.crosshair.render_pos(), None);
        assert_eq!(state.drag_start, None);
        assert_eq!(state.selected_level, None);
        assert_eq!(state.interaction_mode, InteractionMode::Idle);
        assert!(state.momentum.is_none());
        assert!(state.y_animation.is_none());
        assert_eq!(state.dirty, DirtyFlags::new());
    }

    #[test]
    fn state_is_clone_and_debug() {
        let state = ChartState::new(test_camera());
        let cloned = state.clone();
        assert_eq!(cloned.camera.viewport_width, 1920);
        let _ = format!("{:?}", cloned);
    }

    #[test]
    fn state_camera_accessors() {
        let state = ChartState::new(test_camera());
        assert_eq!(state.camera.viewport_width, 1920);
        assert_eq!(state.camera.viewport_height, 1080);
        assert!((state.camera.price_low - 100.0).abs() < f64::EPSILON);
        assert!((state.camera.price_high - 200.0).abs() < f64::EPSILON);
    }

    #[test]
    fn alloc_level_id_increments() {
        let mut state = ChartState::new(test_camera());
        assert_eq!(state.alloc_level_id(), 1);
        assert_eq!(state.alloc_level_id(), 2);
        assert_eq!(state.alloc_level_id(), 3);
    }

    // --- apply_action tests ---

    #[test]
    fn apply_pan_shifts_camera() {
        let mut state = ChartState::new(test_camera());
        let orig_start = state.camera.time_start;
        let orig_end = state.camera.time_end;
        let orig_low = state.camera.price_low;
        let orig_high = state.camera.price_high;

        state.apply_action(&ChartAction::Pan {
            dx: 1000.0,
            dy: 5.0,
        });

        // Positive dx moves the visible window forward in time (right).
        assert!((state.camera.time_start - (orig_start + 1000.0)).abs() < f64::EPSILON);
        assert!((state.camera.time_end - (orig_end + 1000.0)).abs() < f64::EPSILON);
        // dy is added to price.
        assert!((state.camera.price_low - (orig_low + 5.0)).abs() < f64::EPSILON);
        assert!((state.camera.price_high - (orig_high + 5.0)).abs() < f64::EPSILON);
    }

    #[test]
    fn apply_zoom_at_center_narrows_symmetrically() {
        let mut state = ChartState::new(test_camera());
        // center_x = viewport_width / 2 = 960.0 (pixel center)
        let center_x = state.camera.viewport_width as f32 / 2.0;
        let center_time = state.camera.x_to_time(center_x);
        let orig_range = state.camera.time_end - state.camera.time_start;

        state.apply_action(&ChartAction::Zoom {
            center_x,
            factor: 2.0,
        });

        let new_range = state.camera.time_end - state.camera.time_start;
        assert!(
            (new_range - orig_range / 2.0).abs() < 1.0,
            "new_range={}, expected={}",
            new_range,
            orig_range / 2.0
        );
        // Center should be preserved.
        let new_center = (state.camera.time_start + state.camera.time_end) / 2.0;
        assert!(
            (new_center - center_time).abs() < 1.0,
            "center shifted from {} to {}",
            center_time,
            new_center
        );
    }

    #[test]
    fn apply_zoom_at_left_edge() {
        let mut state = ChartState::new(test_camera());
        let orig_start = state.camera.time_start;
        let orig_range = state.camera.time_end - state.camera.time_start;

        // Zoom at left edge: center_x = 0.0 (pixel left edge).
        state.apply_action(&ChartAction::Zoom {
            center_x: 0.0,
            factor: 2.0,
        });

        let new_range = state.camera.time_end - state.camera.time_start;
        assert!(
            (new_range - orig_range / 2.0).abs() < 1.0,
            "new_range={}, expected={}",
            new_range,
            orig_range / 2.0
        );
        // Left edge should be essentially unchanged.
        assert!(
            (state.camera.time_start - orig_start).abs() < 1.0,
            "left edge moved from {} to {}",
            orig_start,
            state.camera.time_start
        );
    }

    #[test]
    fn apply_create_level() {
        let mut state = ChartState::new(test_camera());
        state.apply_action(&ChartAction::CreateLevel { price: 150.0 });
        assert_eq!(state.levels.len(), 1);
        assert_eq!(state.levels[0].id, 1);
        assert!((state.levels[0].price - 150.0).abs() < f64::EPSILON);
    }

    #[test]
    fn apply_select_and_delete_level() {
        let mut state = ChartState::new(test_camera());
        state.apply_action(&ChartAction::CreateLevel { price: 150.0 });
        state.apply_action(&ChartAction::CreateLevel { price: 175.0 });
        assert_eq!(state.levels.len(), 2);

        state.apply_action(&ChartAction::SelectLevel { id: 1 });
        assert_eq!(state.selected_level, Some(1));

        state.apply_action(&ChartAction::DeleteSelectedLevel);
        assert_eq!(state.levels.len(), 1);
        assert_eq!(state.levels[0].id, 2);
        assert_eq!(state.selected_level, None);
    }

    #[test]
    fn apply_drag_level() {
        let mut state = ChartState::new(test_camera());
        state.apply_action(&ChartAction::CreateLevel { price: 150.0 });
        state.apply_action(&ChartAction::DragLevel {
            id: 1,
            new_price: 160.0,
        });
        assert!((state.levels[0].price - 160.0).abs() < f64::EPSILON);
    }

    #[test]
    fn apply_deselect_level() {
        let mut state = ChartState::new(test_camera());
        state.apply_action(&ChartAction::CreateLevel { price: 150.0 });
        state.apply_action(&ChartAction::SelectLevel { id: 1 });
        assert_eq!(state.selected_level, Some(1));
        state.apply_action(&ChartAction::DeselectLevel);
        assert_eq!(state.selected_level, None);
    }

    #[test]
    fn apply_set_and_clear_crosshair() {
        let mut state = ChartState::new(test_camera());
        state.apply_action(&ChartAction::SetCrosshair { x: 100.0, y: 200.0 });
        assert_eq!(state.crosshair.render_pos(), Some((100.0, 200.0)));

        state.apply_action(&ChartAction::ClearCrosshair);
        assert_eq!(state.crosshair.render_pos(), None);
    }

    #[test]
    fn apply_start_momentum() {
        let mut state = ChartState::new(test_camera());
        state.apply_action(&ChartAction::StartMomentum {
            vx: 100.0,
            vy: 10.0,
        });
        assert!(state.momentum.is_some());
        let m = state.momentum.as_ref().unwrap();
        assert!((m.vx - 100.0).abs() < f64::EPSILON);
        assert!((m.vy - 10.0).abs() < f64::EPSILON);
    }

    #[test]
    fn apply_stop_momentum() {
        let mut state = ChartState::new(test_camera());
        state.momentum = Some(Momentum {
            vx: 100.0,
            vy: 10.0,
        });
        state.apply_action(&ChartAction::StopMomentum);
        assert!(state.momentum.is_none());
    }

    #[test]
    fn apply_auto_scale_y() {
        let mut state = ChartState::new(test_camera());
        state.apply_action(&ChartAction::AutoScaleY {
            target_low: 90.0,
            target_high: 210.0,
        });
        assert!(state.y_animation.is_some());
        let a = state.y_animation.as_ref().unwrap();
        assert!((a.target_low - 90.0).abs() < f64::EPSILON);
        assert!((a.target_high - 210.0).abs() < f64::EPSILON);
    }

    // --- tick_momentum tests ---

    #[test]
    fn tick_momentum_moves_camera() {
        let mut state = ChartState::new(test_camera());
        let orig_start = state.camera.time_start;
        state.momentum = Some(Momentum {
            vx: 100_000.0, // 100k ms/sec
            vy: 0.0,
        });
        let active = state.tick_momentum(1.0 / 60.0);
        assert!(active);
        // Camera should have moved.
        assert!(
            (state.camera.time_start - orig_start).abs() > 1.0,
            "camera did not move: start={}",
            state.camera.time_start
        );
    }

    #[test]
    fn tick_momentum_decays_to_stop() {
        let mut state = ChartState::new(test_camera());
        state.momentum = Some(Momentum {
            vx: 1.0, // tiny velocity
            vy: 0.0,
        });
        // With tiny initial velocity and friction=6.0, should converge quickly.
        for _ in 0..600 {
            if !state.tick_momentum(1.0 / 60.0) {
                break;
            }
        }
        assert!(state.momentum.is_none(), "momentum should have stopped");
    }

    #[test]
    fn tick_momentum_no_momentum_returns_false() {
        let mut state = ChartState::new(test_camera());
        assert!(!state.tick_momentum(1.0 / 60.0));
    }

    // --- tick_auto_scale tests ---

    #[test]
    fn tick_auto_scale_converges() {
        let mut state = ChartState::new(test_camera());
        state.y_animation = Some(YAnimation {
            target_low: 90.0,
            target_high: 210.0,
        });

        // Run many frames.
        for _ in 0..600 {
            if !state.tick_auto_scale(1.0 / 60.0) {
                break;
            }
        }

        assert!(
            state.y_animation.is_none(),
            "animation should have converged"
        );
        assert!(
            (state.camera.price_low - 90.0).abs() < 0.1,
            "price_low={}, expected=90.0",
            state.camera.price_low
        );
        assert!(
            (state.camera.price_high - 210.0).abs() < 0.1,
            "price_high={}, expected=210.0",
            state.camera.price_high
        );
    }

    #[test]
    fn tick_auto_scale_approaches_target() {
        let mut state = ChartState::new(test_camera());
        state.y_animation = Some(YAnimation {
            target_low: 50.0,
            target_high: 250.0,
        });

        // After one tick, should be closer to the target but not there yet.
        let active = state.tick_auto_scale(1.0 / 60.0);
        assert!(active);
        // price_low should have moved toward 50, i.e., decreased from 100.
        assert!(
            state.camera.price_low < 100.0,
            "price_low={} should be < 100.0",
            state.camera.price_low
        );
        // price_high should have moved toward 250, i.e., increased from 200.
        assert!(
            state.camera.price_high > 200.0,
            "price_high={} should be > 200.0",
            state.camera.price_high
        );
    }

    #[test]
    fn tick_auto_scale_no_animation_returns_false() {
        let mut state = ChartState::new(test_camera());
        assert!(!state.tick_auto_scale(1.0 / 60.0));
    }
}
