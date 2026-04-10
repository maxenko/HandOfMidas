//! Interaction state machine for chart events.
//!
//! Converts raw input events (`ChartEvent`) into semantic actions (`ChartAction`)
//! that the application layer applies. This is the sans-IO boundary: no iced,
//! no wgpu, no framework types.
//!
//! The state machine prevents ambiguous interactions. When a user presses
//! the mouse button, we do not yet know if they intend to pan, drag a level,
//! or click to select. The `PendingDrag` state resolves this ambiguity once
//! the mouse has moved past the drag threshold (4px).

use crate::state::{ChartState, CursorClaim, InteractionMode};
use crate::widget::AnnotationId;
use midas_core::CandleData;

/// A raw input event delivered to the chart.
///
/// The iced widget adapter normalizes platform events into these before
/// passing them across the sans-IO boundary.
#[derive(Clone, Debug)]
pub enum ChartEvent {
    /// Mouse moved to a new position (logical pixels, chart-widget-local).
    MouseMoved { x: f32, y: f32, alt_held: bool },
    /// Mouse button pressed.
    MousePressed {
        x: f32,
        y: f32,
        button: MouseButton,
        alt_held: bool,
    },
    /// Mouse button released.
    MouseReleased {
        x: f32,
        y: f32,
        button: MouseButton,
        alt_held: bool,
    },
    /// Mouse wheel scrolled. `delta` is positive for scroll-up (forward in time).
    /// `x` and `y` are the cursor position at time of scroll.
    MouseWheel { delta: f32, x: f32, y: f32 },
    /// Double-click detected by the platform / widget adapter.
    DoubleClick { x: f32, y: f32 },
    /// Keyboard key pressed while chart is focused.
    KeyPressed { key: Key },
    /// Viewport resized.
    Resize { width: u32, height: u32 },
    /// Tick the momentum animation. `dt_secs` is the elapsed time since
    /// the last tick (typically 1/60).
    TickMomentum { dt_secs: f32 },
    /// Tick the Y-axis auto-scale animation.
    TickAutoScale { dt_secs: f32 },
    /// Enter level-placement mode (from app layer, e.g., drawing panel button).
    ActivateLevelTool,
}

/// A semantic action produced by the interaction state machine.
///
/// The application layer reads these and applies them to camera state,
/// dirty flags, etc. via `ChartState::apply_action`.
#[derive(Clone, Debug, PartialEq)]
pub enum ChartAction {
    /// Pan the camera by a data-space delta (time ms, price units).
    Pan { dx: f64, dy: f64 },
    /// Zoom the time axis (horizontal), centered on pixel X, by the given factor.
    /// `factor > 1.0` means zoom in (fewer candles visible).
    Zoom { center_x: f32, factor: f64 },
    /// Zoom the price axis (vertical), centered on pixel Y, by the given factor.
    /// `factor > 1.0` means zoom in (narrower price range visible).
    ZoomY { center_y: f32, factor: f64 },
    /// Set the crosshair to the given chart-local pixel position.
    SetCrosshair { x: f32, y: f32 },
    /// Clear the crosshair (mouse left the chart).
    ClearCrosshair,
    /// Smoothly auto-scale the Y axis to the given price range.
    AutoScaleY { target_low: f64, target_high: f64 },
    /// Start a momentum (flick-to-scroll) animation with the given velocity.
    StartMomentum { vx: f64, vy: f64 },
    /// Apply a momentum displacement (used during animation ticks).
    ApplyMomentum { dt: f64, dp: f64 },
    /// Stop any active momentum animation.
    StopMomentum,
    /// Create a new horizontal level at the given price.
    CreateLevel { price: f64 },
    /// Select a horizontal level by its annotation ID.
    SelectLevel { id: AnnotationId },
    /// Drag a horizontal level to a new price.
    DragLevel { id: AnnotationId, new_price: f64 },
    /// Delete the currently selected horizontal level.
    DeleteSelectedLevel,
    /// Deselect any selected horizontal level.
    DeselectLevel,
    /// Right-click on a horizontal level — opens context menu / level editor.
    RightClickLevel { id: AnnotationId, x: f32, y: f32 },
    /// Jump the camera to show the most recent data.
    JumpToEnd,
    /// Jump the camera to show the oldest data.
    JumpToStart,
    /// Set the timeline border ratio (fraction of viewport for the border line).
    SetTimelineBorderRatio { ratio: f64 },
    /// Set the volume scale multiplier.
    SetVolumeScale { scale: f64 },
    /// Request a full redraw.
    Redraw,
    /// Cancel the active drawing/placement mode.
    CancelPlacing,
    /// Report the current preview price during level placement.
    /// Emitted on each in-bounds mouse move while Placing.
    PlacingPreview { price: f64 },
    /// Drag a bracket take-profit or stop-loss leg to a new price.
    ///
    /// Emitted during mouse move while dragging a bracket leg. The entry
    /// line is NOT draggable (market orders fill instantly, entry is terminal).
    DragBracketLeg {
        /// Which annotation (OrderBracket) owns the leg being dragged.
        annotation_id: super::widget::AnnotationId,
        /// Which leg is being dragged.
        leg: super::widget::order_bracket::LegRole,
        /// The new price for this leg (may be clamped by side constraints).
        new_price: f64,
    },
    /// Create an order bracket from the drawing tool (3-click complete).
    CreateBracket {
        /// Entry price.
        entry: f64,
        /// Take-profit price.
        tp: f64,
        /// Stop-loss price.
        sl: f64,
        /// Trade direction.
        side: super::widget::order_bracket::BracketSide,
    },
    /// Submit a Draft bracket to the broker.
    SubmitBracket {
        /// Which annotation (OrderBracket) to submit.
        annotation_id: super::widget::AnnotationId,
    },
    /// Save/pin a Draft bracket.
    SaveBracket {
        /// Which annotation (OrderBracket) to save.
        annotation_id: super::widget::AnnotationId,
    },
    /// Toggle SL on/off for a Draft bracket.
    ToggleBracketSL {
        /// Which annotation (OrderBracket) to toggle SL on.
        annotation_id: super::widget::AnnotationId,
    },
    /// Cancel/remove a Draft bracket.
    CancelBracket {
        /// Which annotation (OrderBracket) to cancel.
        annotation_id: super::widget::AnnotationId,
    },
    /// Remove the SL leg from a Draft bracket.
    CancelBracketSL {
        /// Which annotation (OrderBracket) to remove SL from.
        annotation_id: super::widget::AnnotationId,
    },
    /// Right-click on a bracket leg — opens bracket context menu.
    RightClickBracketLeg {
        /// Which annotation (OrderBracket) owns the leg.
        annotation_id: super::widget::AnnotationId,
        /// Which leg was right-clicked.
        leg: super::widget::order_bracket::LegRole,
        /// Screen X for context menu placement.
        x: f32,
        /// Screen Y for context menu placement.
        y: f32,
    },
}

/// Mouse button discriminant.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum MouseButton {
    /// Left mouse button.
    Left,
    /// Right mouse button.
    Right,
    /// Middle mouse button.
    Middle,
}

/// Keyboard key discriminant for chart interactions.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Key {
    /// Delete / Backspace key.
    Delete,
    /// Escape key.
    Escape,
    /// Home key.
    Home,
    /// End key.
    End,
    /// H key (hotkey for horizontal level).
    H,
    /// B key (hotkey for bracket tool — Long).
    B,
    /// Tab key (toggle bracket side during placement).
    Tab,
}

/// Drag threshold in pixels. A mouse move must exceed this distance from
/// the initial press point to be classified as a drag rather than a click.
const DRAG_THRESHOLD_PX: f32 = 4.0;

/// Hit-test tolerance for horizontal levels, in pixels.
const LEVEL_HIT_TOLERANCE_PX: f32 = 6.0;

/// Dead zone in pixels for middle-mouse axis-lock scaling.
/// Movement must exceed this before axis is determined.
const SCALE_DEAD_ZONE_PX: f32 = 6.0;

/// Ratio threshold for axis-lock determination. If one axis exceeds
/// the other by this factor, that axis wins immediately.
const AXIS_LOCK_RATIO: f32 = 1.5;

/// Sensitivity for middle-drag scaling (per pixel).
const SCALE_SENSITIVITY: f64 = 0.005;

/// Bracket-specific context returned by [`hit_test_annotation()`].
///
/// Carries the entry price and side needed for drag clamping.
/// `None` for non-bracket annotations (levels).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BracketClampCtx {
    /// Entry price of the bracket (for side-constraint clamping).
    pub entry_price: f64,
    /// Trade direction (for side-constraint clamping).
    pub side: crate::widget::order_bracket::BracketSide,
}

/// Unified hit-test for all interactive annotation elements.
///
/// Iterates annotations, checks each element's `price_to_y` distance
/// against `cursor_y`, and returns the closest within `LEVEL_HIT_TOLERANCE_PX`.
/// Handles both levels (`HitZoneKind::LevelLine`) and bracket legs
/// (`BracketTP`, `BracketSL`, `BracketEntry`, `BracketStopTrigger`).
///
/// Replaces `hit_test_levels()` + `hit_test_bracket_legs()`.
fn hit_test_annotation(
    annotations: &[crate::widget::Annotation],
    cursor_y: f32,
    camera: &crate::camera::Camera2D,
) -> Option<(AnnotationId, crate::widget::hit_test::HitZoneKind, f64, Option<BracketClampCtx>)> {
    use crate::widget::hit_test::HitZoneKind;
    use crate::widget::order_bracket::{BracketStatus, EntryType};

    let cursor_price = camera.y_to_price(cursor_y);
    let mut best: Option<(AnnotationId, HitZoneKind, f32, f64, Option<BracketClampCtx>)> = None;

    for ann in annotations {
        if !ann.presence.is_interactive() || ann.locked {
            continue;
        }

        match &ann.kind {
            crate::widget::AnnotationKind::Level(level) => {
                let leg_y = camera.price_to_y(level.price);
                let dist = (cursor_y - leg_y).abs();
                if dist <= LEVEL_HIT_TOLERANCE_PX
                    && best.as_ref().is_none_or(|b| dist < b.2)
                {
                    let offset = level.price - cursor_price;
                    best = Some((ann.id, HitZoneKind::LevelLine, dist, offset, None));
                }
            }
            crate::widget::AnnotationKind::OrderBracket(bracket) => {
                let clamp_ctx = Some(BracketClampCtx {
                    entry_price: bracket.entry.price,
                    side: bracket.side,
                });

                // TP leg.
                if let Some(ref tp) = bracket.take_profit {
                    let leg_y = camera.price_to_y(tp.price);
                    let dist = (cursor_y - leg_y).abs();
                    if dist <= LEVEL_HIT_TOLERANCE_PX
                        && best.as_ref().is_none_or(|b| dist < b.2)
                    {
                        let offset = tp.price - cursor_price;
                        best = Some((
                            ann.id,
                            HitZoneKind::BracketTP,
                            dist,
                            offset,
                            clamp_ctx,
                        ));
                    }
                }
                // SL leg.
                if let Some(ref sl) = bracket.stop_loss {
                    let leg_y = camera.price_to_y(sl.price);
                    let dist = (cursor_y - leg_y).abs();
                    if dist <= LEVEL_HIT_TOLERANCE_PX
                        && best.as_ref().is_none_or(|b| dist < b.2)
                    {
                        let offset = sl.price - cursor_price;
                        best = Some((
                            ann.id,
                            HitZoneKind::BracketSL,
                            dist,
                            offset,
                            clamp_ctx,
                        ));
                    }
                }
                // Entry leg (non-Market Draft only).
                if bracket.entry_type != EntryType::Market
                    && bracket.status == BracketStatus::Draft
                {
                    let leg_y = camera.price_to_y(bracket.entry.price);
                    let dist = (cursor_y - leg_y).abs();
                    if dist <= LEVEL_HIT_TOLERANCE_PX
                        && best.as_ref().is_none_or(|b| dist < b.2)
                    {
                        let offset = bracket.entry.price - cursor_price;
                        best = Some((
                            ann.id,
                            HitZoneKind::BracketEntry,
                            dist,
                            offset,
                            clamp_ctx,
                        ));
                    }
                }
                // StopTrigger leg (StopLimit Draft only).
                if bracket.entry_type == EntryType::StopLimit
                    && bracket.status == BracketStatus::Draft
                {
                    if let Some(stop_price) = bracket.entry_stop_price {
                        let leg_y = camera.price_to_y(stop_price);
                        let dist = (cursor_y - leg_y).abs();
                        if dist <= LEVEL_HIT_TOLERANCE_PX
                            && best.as_ref().is_none_or(|b| dist < b.2)
                        {
                            let offset = stop_price - cursor_price;
                            best = Some((
                                ann.id,
                                HitZoneKind::BracketStopTrigger,
                                dist,
                                offset,
                                clamp_ctx,
                            ));
                        }
                    }
                }
            }
            _ => {}
        }
    }

    best.map(|(id, kind, _, offset, ctx)| (id, kind, offset, ctx))
}

/// Width of the volume handle triangle (pixels).
const VOLUME_HANDLE_WIDTH: f32 = 10.0;
/// Height of the volume handle triangle (pixels).
const VOLUME_HANDLE_HEIGHT: f32 = 14.0;
/// Extra hit-test padding around the volume handle (pixels).
const VOLUME_HANDLE_HIT_PADDING: f32 = 8.0;
/// Fraction of the viewport height reserved for volume bars at the bottom.
const VOLUME_AREA_FRACTION: f32 = 0.20;
/// Vertical hit-test tolerance for the timeline border line (pixels each side).
const TIMELINE_BORDER_HIT_TOLERANCE: f32 = 6.0;

/// Process a chart event and return zero or more actions.
///
/// This is the core state machine. It reads and mutates `state`'s interaction
/// mode and drag tracking, then returns actions for the caller to apply via
/// `ChartState::apply_action`.
///
/// `data` and `is_collapsed` are used by the level tool for OHLC snapping
/// during placement. Pass `None` / `false` if candle data is unavailable.
pub fn handle_event(
    state: &mut ChartState,
    event: ChartEvent,
    data: Option<&dyn CandleData>,
    is_collapsed: bool,
    annotations: &[crate::widget::Annotation],
) -> Vec<ChartAction> {
    match event {
        ChartEvent::MouseMoved { x, y, alt_held } => {
            handle_mouse_moved(state, x, y, alt_held, data, is_collapsed, annotations)
        }

        ChartEvent::MousePressed {
            x,
            y,
            button,
            alt_held,
        } => handle_mouse_pressed(state, x, y, button, alt_held, annotations),

        ChartEvent::MouseReleased { x, y, button, .. } => {
            handle_mouse_released(state, x, y, button, annotations)
        }

        ChartEvent::MouseWheel { delta, x, .. } => handle_mouse_wheel(state, delta, x),

        ChartEvent::DoubleClick { x, y } => {
            let raw_price = state.camera.y_to_price(y);
            let price = if let Some(d) = data {
                state
                    .level_tool
                    .snap_to_ohlc(raw_price, x, &state.camera, d, is_collapsed)
            } else {
                raw_price
            };
            vec![ChartAction::CreateLevel { price }]
        }

        ChartEvent::KeyPressed { key } => handle_key_pressed(state, key),

        ChartEvent::Resize { width, height } => {
            state.camera.viewport_width = width;
            state.camera.viewport_height = height;
            vec![ChartAction::Redraw]
        }

        ChartEvent::TickMomentum { dt_secs } => {
            if state.momentum.is_some() {
                let still_active = state.tick_momentum(dt_secs);
                if still_active {
                    vec![ChartAction::Redraw]
                } else {
                    vec![ChartAction::StopMomentum]
                }
            } else {
                vec![]
            }
        }

        ChartEvent::TickAutoScale { dt_secs } => {
            if state.y_animation.is_some() {
                let _still_active = state.tick_auto_scale(dt_secs);
                vec![ChartAction::Redraw]
            } else {
                vec![]
            }
        }

        ChartEvent::ActivateLevelTool => {
            state.level_tool.activate();
            Vec::new()
        }
    }
}

// ── Private handler functions ────────────────────────────────────────────

fn handle_mouse_moved(
    state: &mut ChartState,
    x: f32,
    y: f32,
    alt_held: bool,
    data: Option<&dyn CandleData>,
    is_collapsed: bool,
    annotations: &[crate::widget::Annotation],
) -> Vec<ChartAction> {
    let mut actions = Vec::new();

    // Dispatch based on the collective cursor claim of all active tools.
    // This replaces tool-specific if/else chains — new tools only need to
    // be added to `active_cursor_claim()`.
    match state.active_cursor_claim() {
        CursorClaim::Suppress => {
            return handle_suppressed_move(state, x, y, alt_held, data, is_collapsed);
        }
        CursorClaim::Preview | CursorClaim::None => {}
    }

    // Update crosshair via CrosshairTool — the tool's mode handles visibility.
    let dragging_handle = matches!(
        state.interaction_mode,
        InteractionMode::DraggingVolumeScale { .. }
            | InteractionMode::DraggingTimelineBorder { .. }
            | InteractionMode::DraggingAnnotation { .. }
    );
    let in_bounds = x >= 0.0
        && y >= 0.0
        && x <= state.camera.viewport_width as f32
        && y <= state.camera.viewport_height as f32;

    let was_visible = state.crosshair.should_render();
    if !dragging_handle {
        state.crosshair.on_mouse_move(x, y, in_bounds);
    }
    let is_visible = state.crosshair.should_render();

    // Keep deprecated field in sync.
    #[allow(deprecated)]
    {
        state.crosshair_pos = state.crosshair.render_pos();
    }

    if is_visible {
        if let Some((rx, ry)) = state.crosshair.render_pos() {
            actions.push(ChartAction::SetCrosshair { x: rx, y: ry });
        }
    } else if was_visible {
        actions.push(ChartAction::ClearCrosshair);
    }

    // State machine transitions based on interaction mode.
    match state.interaction_mode.clone() {
        InteractionMode::Idle => {
            // Nothing to do beyond crosshair update.
        }

        InteractionMode::PendingDrag { start_x, start_y } => {
            let ddx = x - start_x;
            let ddy = y - start_y;
            let dist = (ddx * ddx + ddy * ddy).sqrt();

            if dist >= DRAG_THRESHOLD_PX {
                // Exceeded threshold. Left-drag only initiates annotation
                // drag, never panning (panning is right-mouse only).
                if let Some((ann_id, kind, grab_offset, clamp_ctx)) =
                    hit_test_annotation(annotations, start_y, &state.camera)
                {
                    use crate::widget::hit_test::HitZoneKind;

                    match kind {
                        HitZoneKind::LevelLine
                        | HitZoneKind::BracketTP
                        | HitZoneKind::BracketSL
                        | HitZoneKind::BracketEntry
                        | HitZoneKind::BracketStopTrigger => {
                            state.interaction_mode =
                                InteractionMode::DraggingAnnotation {
                                    annotation_id: ann_id,
                                    element: kind,
                                    grab_offset,
                                    clamp_ctx,
                                };
                            if kind == HitZoneKind::LevelLine {
                                actions.push(ChartAction::SelectLevel { id: ann_id });
                            }
                        }
                        _ => {
                            state.interaction_mode = InteractionMode::Idle;
                        }
                    }
                    state.crosshair.suppress();
                    #[allow(deprecated)]
                    {
                        state.crosshair_pos = None;
                    }
                    actions.push(ChartAction::ClearCrosshair);
                    state.drag_start = Some((x, y));
                } else {
                    // No annotation hit — return to Idle.
                    state.interaction_mode = InteractionMode::Idle;
                }
            }
        }

        InteractionMode::Panning => {
            if let Some((prev_x, prev_y)) = state.drag_start {
                let pixel_dx = x - prev_x;
                let pixel_dy = y - prev_y;

                // Convert pixel delta to data-space delta.
                // Dragging right (positive pixel_dx) should move backward in
                // time (negative dx) — natural "grab and drag" behavior.
                // Positive dx = forward in time in the Pan convention.
                let time_range = state.camera.time_end - state.camera.time_start;
                let price_range = state.camera.price_high - state.camera.price_low;
                let vw = state.camera.viewport_width as f64;
                let vh = state.camera.viewport_height as f64;

                let dx = if vw > 0.0 {
                    -(pixel_dx as f64) * (time_range / vw)
                } else {
                    0.0
                };
                let dy = if vh > 0.0 {
                    pixel_dy as f64 * (price_range / vh)
                } else {
                    0.0
                };

                if dx.abs() > f64::EPSILON || dy.abs() > f64::EPSILON {
                    actions.push(ChartAction::Pan { dx, dy });
                }

                state.drag_start = Some((x, y));
            }
        }

        InteractionMode::PendingScale { start_x, start_y } => {
            let ddx = (x - start_x).abs();
            let ddy = (y - start_y).abs();
            let dist = ddx.max(ddy); // Chebyshev distance

            if dist >= SCALE_DEAD_ZONE_PX {
                // Determine axis lock.
                if ddx > ddy * AXIS_LOCK_RATIO {
                    state.interaction_mode = InteractionMode::HorizontalScaling {
                        anchor_x: start_x,
                        last_x: x,
                    };
                } else if ddy > ddx * AXIS_LOCK_RATIO {
                    state.interaction_mode = InteractionMode::VerticalScaling {
                        anchor_y: start_y,
                        last_y: y,
                    };
                } else if ddx >= ddy {
                    // Dominant axis wins.
                    state.interaction_mode = InteractionMode::HorizontalScaling {
                        anchor_x: start_x,
                        last_x: x,
                    };
                } else {
                    state.interaction_mode = InteractionMode::VerticalScaling {
                        anchor_y: start_y,
                        last_y: y,
                    };
                }
            }
        }

        InteractionMode::HorizontalScaling { anchor_x, last_x } => {
            let pixel_dx = x - last_x;
            if pixel_dx.abs() > 0.5 {
                let factor = (SCALE_SENSITIVITY * pixel_dx as f64).exp();
                actions.push(ChartAction::Zoom {
                    center_x: anchor_x,
                    factor,
                });
                state.interaction_mode = InteractionMode::HorizontalScaling {
                    anchor_x,
                    last_x: x,
                };
            }
        }

        InteractionMode::VerticalScaling { anchor_y, last_y } => {
            let dy = y - last_y;
            if dy.abs() > 0.5 {
                let factor = (SCALE_SENSITIVITY * (-dy) as f64).exp();
                actions.push(ChartAction::ZoomY {
                    center_y: anchor_y,
                    factor,
                });
                state.interaction_mode = InteractionMode::VerticalScaling {
                    anchor_y,
                    last_y: y,
                };
            }
        }

        InteractionMode::RightPanning => {
            if let Some((prev_x, prev_y)) = state.drag_start {
                let pixel_dx = x - prev_x;
                let pixel_dy = y - prev_y;
                let time_range = state.camera.time_end - state.camera.time_start;
                let price_range = state.camera.price_high - state.camera.price_low;
                let vw = state.camera.viewport_width as f64;
                let vh = state.camera.viewport_height as f64;

                let dx = if vw > 0.0 {
                    -(pixel_dx as f64) * (time_range / vw)
                } else {
                    0.0
                };
                let dy = if vh > 0.0 {
                    pixel_dy as f64 * (price_range / vh)
                } else {
                    0.0
                };

                if dx.abs() > 0.001 || dy.abs() > 0.0001 {
                    actions.push(ChartAction::Pan { dx, dy });
                }
                state.drag_start = Some((x, y));
            }
        }

        InteractionMode::DraggingTimelineBorder {
            anchor_y,
            start_ratio,
        } => {
            let vh = state.camera.viewport_height as f32;
            let dy = anchor_y - y;
            let delta_ratio = dy / vh;
            let new_ratio = (start_ratio + delta_ratio).clamp(
                ChartState::TIMELINE_BORDER_MIN,
                ChartState::TIMELINE_BORDER_MAX,
            );
            actions.push(ChartAction::SetTimelineBorderRatio {
                ratio: new_ratio as f64,
            });
        }

        InteractionMode::DraggingVolumeScale { .. } => {
            // Direct Y→scale mapping so the triangle tracks the cursor 1:1.
            // Inverts volume_handle_y: scale = (1.0 - y/vh) / VOLUME_AREA_FRACTION
            let vh = state.camera.viewport_height as f32;
            let fraction = (1.0 - y / vh).clamp(0.0, 1.0);
            let new_scale = (fraction / VOLUME_AREA_FRACTION)
                .clamp(ChartState::VOLUME_SCALE_MIN, ChartState::VOLUME_SCALE_MAX);
            actions.push(ChartAction::SetVolumeScale {
                scale: new_scale as f64,
            });
        }

        InteractionMode::DraggingAnnotation {
            annotation_id,
            element,
            grab_offset,
            clamp_ctx,
        } => {
            use crate::widget::hit_test::HitZoneKind;

            let raw_price = state.camera.y_to_price(y) + grab_offset;

            match element {
                HitZoneKind::LevelLine => {
                    // Level drag: OHLC snap, emit DragLevel.
                    let snapped = if let Some(d) = data {
                        state
                            .level_tool
                            .snap_to_ohlc(raw_price, x, &state.camera, d, is_collapsed)
                    } else {
                        raw_price
                    };
                    state.crosshair.suppress();
                    #[allow(deprecated)]
                    {
                        state.crosshair_pos = None;
                    }
                    actions.push(ChartAction::DragLevel {
                        id: annotation_id,
                        new_price: snapped,
                    });
                    actions.push(ChartAction::ClearCrosshair);
                }
                HitZoneKind::BracketTP
                | HitZoneKind::BracketSL
                | HitZoneKind::BracketEntry
                | HitZoneKind::BracketStopTrigger => {
                    let leg = match element {
                        HitZoneKind::BracketTP => {
                            crate::widget::order_bracket::LegRole::TakeProfit
                        }
                        HitZoneKind::BracketSL => {
                            crate::widget::order_bracket::LegRole::StopLoss
                        }
                        HitZoneKind::BracketEntry => {
                            crate::widget::order_bracket::LegRole::Entry
                        }
                        HitZoneKind::BracketStopTrigger => {
                            crate::widget::order_bracket::LegRole::StopTrigger
                        }
                        _ => unreachable!(),
                    };
                    let ctx = clamp_ctx.unwrap_or(BracketClampCtx {
                        entry_price: 0.0,
                        side: crate::widget::order_bracket::BracketSide::Long,
                    });
                    // Clamp to correct side of entry based on trade direction.
                    let clamped = clamp_bracket_leg_price(
                        raw_price,
                        ctx.entry_price,
                        leg,
                        ctx.side,
                        &state.camera,
                    );
                    // Snap to valid tick increment.
                    let snapped = snap_to_tick(clamped, DEFAULT_TICK_SIZE);

                    actions.push(ChartAction::DragBracketLeg {
                        annotation_id,
                        leg,
                        new_price: snapped,
                    });
                }
                _ => {}
            }
        }
    }

    actions
}

fn handle_mouse_pressed(
    state: &mut ChartState,
    x: f32,
    y: f32,
    button: MouseButton,
    _alt_held: bool,
    annotations: &[crate::widget::Annotation],
) -> Vec<ChartAction> {
    // In bracket drawing mode (via BracketTool):
    // - Left-click: advance the 3-click state machine
    // - Right-click: cancel bracket drawing
    // - Escape: cancel (handled in key press)
    if state.bracket_tool.is_active() {
        match button {
            MouseButton::Left => {
                let price = state.camera.y_to_price(y);
                if let Some(result) = state.bracket_tool.click(price) {
                    use crate::widget::bracket_tool::BracketToolResult;
                    match result {
                        BracketToolResult::NeedMore => {
                            // Still placing — stay in bracket tool mode.
                            return Vec::new();
                        }
                        BracketToolResult::Complete {
                            entry,
                            tp,
                            sl,
                            side,
                        } => {
                            // Bracket complete — emit CreateBracket action.
                            state.crosshair.force_hide();
                            #[allow(deprecated)]
                            {
                                state.crosshair_pos = None;
                            }
                            return vec![
                                ChartAction::CreateBracket {
                                    entry,
                                    tp,
                                    sl,
                                    side,
                                },
                                ChartAction::ClearCrosshair,
                            ];
                        }
                    }
                }
                return Vec::new();
            }
            MouseButton::Right => {
                // Cancel bracket drawing on right-click.
                state.bracket_tool.cancel();
                return Vec::new();
            }
            MouseButton::Middle => {
                return Vec::new();
            }
        }
    }

    // In Placing mode (via LevelTool):
    // - Left-click: place the level at (snapped) cursor price
    // - Right-click / Middle-click: temporarily suspend placement for pan/scale
    //   (placement resumes when pan/scale ends via try_resume_placing)
    if state.level_tool.is_placing() {
        match button {
            MouseButton::Left => {
                let price = state
                    .level_tool
                    .snapped_price
                    .unwrap_or_else(|| state.camera.y_to_price(y));
                state.level_tool.cancel();
                state.crosshair.force_hide();
                #[allow(deprecated)]
                {
                    state.crosshair_pos = None;
                }
                return vec![
                    ChartAction::CreateLevel { price },
                    ChartAction::CancelPlacing,
                    ChartAction::ClearCrosshair,
                ];
            }
            MouseButton::Right => {
                // Temporarily suspend Placing for right-click panning.
                state.level_tool.suspend_placing();
                state.interaction_mode = InteractionMode::RightPanning;
                state.drag_start = Some((x, y));
                return Vec::new();
            }
            MouseButton::Middle => {
                // Temporarily suspend Placing for middle-click scaling.
                state.level_tool.suspend_placing();
                state.interaction_mode = InteractionMode::PendingScale {
                    start_x: x,
                    start_y: y,
                };
                return Vec::new();
            }
        }
    }

    match button {
        MouseButton::Left => {
            let mut actions = Vec::new();

            // Check volume handle triangle first (right edge).
            if is_over_volume_handle(
                x,
                y,
                state.camera.viewport_width,
                state.camera.viewport_height,
                state.volume_scale,
            ) {
                state.interaction_mode = InteractionMode::DraggingVolumeScale {
                    anchor_y: y,
                    start_scale: state.volume_scale,
                };
                return actions;
            }

            // Check timeline border line (full width).
            if is_over_timeline_border(y, state.camera.viewport_height, state.timeline_border_ratio)
            {
                state.interaction_mode = InteractionMode::DraggingTimelineBorder {
                    anchor_y: y,
                    start_ratio: state.timeline_border_ratio,
                };
                return actions;
            }

            state.crosshair.on_left_press(x, y);
            #[allow(deprecated)]
            {
                state.left_mouse_down = true;
            }

            // Don't show crosshair if pressing on a draggable annotation —
            // the PendingDrag will resolve to a drag, not a crosshair.
            let over_draggable = hit_test_annotation(annotations, y, &state.camera).is_some();
            if over_draggable {
                state.crosshair.suppress();
                #[allow(deprecated)]
                {
                    state.crosshair_pos = None;
                }
            } else {
                #[allow(deprecated)]
                {
                    state.crosshair_pos = state.crosshair.render_pos();
                }
                actions.push(ChartAction::SetCrosshair { x, y });
            }

            // Stop any active momentum.
            if state.momentum.is_some() {
                actions.push(ChartAction::StopMomentum);
                state.momentum = None;
            }

            // Enter PendingDrag state.
            state.interaction_mode = InteractionMode::PendingDrag {
                start_x: x,
                start_y: y,
            };
            state.drag_start = Some((x, y));
            actions
        }

        MouseButton::Middle => {
            // Enter pending-scale dead zone; axis determined after 6px of movement.
            state.interaction_mode = InteractionMode::PendingScale {
                start_x: x,
                start_y: y,
            };
            vec![]
        }

        MouseButton::Right => {
            use crate::widget::hit_test::HitZoneKind;

            if let Some((ann_id, kind, _offset, _ctx)) =
                hit_test_annotation(annotations, y, &state.camera)
            {
                match kind {
                    HitZoneKind::LevelLine => {
                        vec![ChartAction::RightClickLevel { id: ann_id, x, y }]
                    }
                    HitZoneKind::BracketTP
                    | HitZoneKind::BracketSL
                    | HitZoneKind::BracketEntry
                    | HitZoneKind::BracketStopTrigger => {
                        let leg = match kind {
                            HitZoneKind::BracketTP => {
                                crate::widget::order_bracket::LegRole::TakeProfit
                            }
                            HitZoneKind::BracketSL => {
                                crate::widget::order_bracket::LegRole::StopLoss
                            }
                            HitZoneKind::BracketEntry => {
                                crate::widget::order_bracket::LegRole::Entry
                            }
                            HitZoneKind::BracketStopTrigger => {
                                crate::widget::order_bracket::LegRole::StopTrigger
                            }
                            _ => unreachable!(),
                        };
                        vec![ChartAction::RightClickBracketLeg {
                            annotation_id: ann_id,
                            leg,
                            x,
                            y,
                        }]
                    }
                    _ => {
                        state.interaction_mode = InteractionMode::RightPanning;
                        state.drag_start = Some((x, y));
                        vec![]
                    }
                }
            } else {
                state.interaction_mode = InteractionMode::RightPanning;
                state.drag_start = Some((x, y));
                vec![]
            }
        }
    }
}

fn handle_mouse_released(
    state: &mut ChartState,
    x: f32,
    y: f32,
    button: MouseButton,
    annotations: &[crate::widget::Annotation],
) -> Vec<ChartAction> {
    // Handle middle button release — exit any scaling mode.
    if button == MouseButton::Middle {
        if matches!(
            state.interaction_mode,
            InteractionMode::PendingScale { .. }
                | InteractionMode::HorizontalScaling { .. }
                | InteractionMode::VerticalScaling { .. }
        ) {
            state.interaction_mode = InteractionMode::Idle;
        }
        state.level_tool.try_resume_placing();
        sync_crosshair_to_claim(state);
        return vec![];
    }

    // Handle right button release — exit right-panning with momentum.
    if button == MouseButton::Right {
        let mut actions = Vec::new();
        if matches!(state.interaction_mode, InteractionMode::RightPanning) {
            // Compute flick-to-scroll momentum from last drag delta.
            if let Some((prev_x, prev_y)) = state.drag_start {
                let pixel_dx = x - prev_x;
                let pixel_dy = y - prev_y;
                let time_range = state.camera.time_end - state.camera.time_start;
                let price_range = state.camera.price_high - state.camera.price_low;
                let vw = state.camera.viewport_width as f64;
                let vh = state.camera.viewport_height as f64;
                let fps = 60.0_f64;
                let vx = if vw > 0.0 {
                    -(pixel_dx as f64) * (time_range / vw) * fps
                } else {
                    0.0
                };
                let vy = if vh > 0.0 {
                    pixel_dy as f64 * (price_range / vh) * fps
                } else {
                    0.0
                };
                if vx.abs() > 1.0 || vy.abs() > 0.001 {
                    actions.push(ChartAction::StartMomentum { vx, vy });
                }
            }
            state.interaction_mode = InteractionMode::Idle;
            state.drag_start = None;
        }
        state.level_tool.try_resume_placing();
        sync_crosshair_to_claim(state);
        return actions;
    }

    if button != MouseButton::Left {
        return vec![];
    }

    // Volume scale / timeline border drag ends without affecting crosshair state.
    if matches!(
        state.interaction_mode,
        InteractionMode::DraggingVolumeScale { .. }
            | InteractionMode::DraggingTimelineBorder { .. }
    ) {
        state.interaction_mode = InteractionMode::Idle;
        state.crosshair.on_left_release();
        #[allow(deprecated)]
        {
            state.left_mouse_down = false;
        }
        return vec![];
    }

    // End annotation dragging on left mouse-up.
    if matches!(
        state.interaction_mode,
        InteractionMode::DraggingAnnotation { .. }
    ) {
        state.interaction_mode = InteractionMode::Idle;
        state.drag_start = None;
        state.crosshair.on_left_release();
        #[allow(deprecated)]
        {
            state.left_mouse_down = false;
        }
        return vec![ChartAction::ClearCrosshair];
    }

    state.crosshair.on_left_release();
    #[allow(deprecated)]
    {
        state.left_mouse_down = false;
    }

    // Hide crosshair on release.
    #[allow(deprecated)]
    {
        state.crosshair_pos = state.crosshair.render_pos();
    }

    let mut actions = vec![ChartAction::ClearCrosshair];
    let prev_mode = state.interaction_mode.clone();

    match prev_mode {
        InteractionMode::PendingDrag { start_x, start_y } => {
            // Released without exceeding drag threshold -- this is a click.
            // Check bracket button hit zones first (higher priority).
            let vp_w = state.camera.viewport_width as f32;
            if let Some(btn_action) =
                hit_test_bracket_buttons(annotations, start_x, start_y, &state.camera, vp_w)
            {
                actions.push(btn_action);
            } else if let Some((ann_id, kind, _, _)) =
                hit_test_annotation(annotations, start_y, &state.camera)
            {
                if kind == crate::widget::hit_test::HitZoneKind::LevelLine {
                    actions.push(ChartAction::SelectLevel { id: ann_id });
                }
            } else if state.selected_level.is_some() {
                actions.push(ChartAction::DeselectLevel);
            }
        }

        InteractionMode::Panning => {
            // Left-drag no longer pans, but handle gracefully if
            // state was entered before the change.
        }

        InteractionMode::PendingScale { .. }
        | InteractionMode::HorizontalScaling { .. }
        | InteractionMode::VerticalScaling { .. } => {
            // Scaling completed — nothing extra to do.
        }

        InteractionMode::RightPanning => {
            // Right-release handled by early return above; unreachable here.
        }

        InteractionMode::Idle => {}

        // Handled by the early return above; unreachable.
        InteractionMode::DraggingVolumeScale { .. } => {}
        InteractionMode::DraggingTimelineBorder { .. } => {}
        InteractionMode::DraggingAnnotation { .. } => {}
    }

    // Always return to Idle on mouse release.
    state.interaction_mode = InteractionMode::Idle;
    state.drag_start = None;

    actions
}

fn handle_mouse_wheel(state: &mut ChartState, delta: f32, _x: f32) -> Vec<ChartAction> {
    if delta.abs() < f32::EPSILON {
        return vec![];
    }

    // Pan: each scroll notch moves ~8% of the visible time range.
    // Positive delta (scroll up) = move forward in time (positive dx).
    let time_range = state.camera.time_end - state.camera.time_start;
    let pan_amount = time_range * 0.08 * delta as f64;

    // Horizontal clamping is enforced centrally in ChartState::apply_action
    // via clamp_pan_dx, so we just emit the raw pan delta here.
    vec![ChartAction::Pan {
        dx: pan_amount,
        dy: 0.0,
    }]
}

fn handle_key_pressed(state: &mut ChartState, key: Key) -> Vec<ChartAction> {
    match key {
        Key::Delete => {
            if state.selected_level.is_some() {
                vec![ChartAction::DeleteSelectedLevel]
            } else {
                vec![]
            }
        }
        Key::Escape => {
            let mut actions = Vec::new();
            // Cancel whichever tool is active (each tool has its own cleanup).
            if state.bracket_tool.is_active() {
                state.bracket_tool.cancel();
                state.crosshair.force_hide();
                #[allow(deprecated)]
                {
                    state.crosshair_pos = None;
                }
                actions.push(ChartAction::ClearCrosshair);
                return actions;
            }
            if state.level_tool.is_active() {
                state.level_tool.cancel();
                state.crosshair.force_hide();
                #[allow(deprecated)]
                {
                    state.crosshair_pos = None;
                }
                actions.push(ChartAction::CancelPlacing);
                actions.push(ChartAction::ClearCrosshair);
                return actions;
            }
            if state.selected_level.is_some() {
                actions.push(ChartAction::DeselectLevel);
            }
            actions.push(ChartAction::ClearCrosshair);
            actions
        }
        Key::Home => vec![ChartAction::JumpToStart],
        Key::End => vec![ChartAction::JumpToEnd],
        Key::H => {
            if matches!(state.interaction_mode, InteractionMode::Idle)
                && !state.bracket_tool.is_active()
            {
                state.level_tool.activate();
            }
            Vec::new()
        }
        Key::B => {
            if matches!(state.interaction_mode, InteractionMode::Idle)
                && !state.level_tool.is_active()
            {
                state.bracket_tool.activate();
            }
            Vec::new()
        }
        Key::Tab => {
            state.bracket_tool.toggle_side();
            Vec::new()
        }
    }
}

// ── Cursor-claim dispatch helpers ────────────────────────────────────────

/// Handle mouse movement when a tool claims `CursorClaim::Suppress`
/// (level placement or dragging). Crosshair is hidden; tool-specific
/// computation (OHLC snap, drag price) runs instead.
fn handle_suppressed_move(
    state: &mut ChartState,
    x: f32,
    y: f32,
    alt_held: bool,
    data: Option<&dyn CandleData>,
    is_collapsed: bool,
) -> Vec<ChartAction> {
    state.level_tool.alt_held = alt_held;

    // Placing mode: compute preview price (with OHLC snap if available).
    // Crosshair stays hidden — preview line rendered via scene.level_preview_y.
    if state.level_tool.is_placing() {
        let in_bounds = x >= 0.0
            && y >= 0.0
            && x <= state.camera.viewport_width as f32
            && y <= state.camera.viewport_height as f32;
        if in_bounds {
            let raw_price = state.camera.y_to_price(y);
            let price = if let Some(d) = data {
                state
                    .level_tool
                    .snap_to_ohlc(raw_price, x, &state.camera, d, is_collapsed)
            } else {
                raw_price
            };
            state.level_tool.preview_price = Some(price);
            state.crosshair.suppress();
            #[allow(deprecated)]
            {
                state.crosshair_pos = None;
            }
            return vec![
                ChartAction::PlacingPreview { price },
                ChartAction::ClearCrosshair,
            ];
        } else {
            state.level_tool.preview_price = None;
        }
        state.crosshair.suppress();
        #[allow(deprecated)]
        {
            state.crosshair_pos = None;
        }
        return vec![ChartAction::ClearCrosshair];
    }

    // Bracket tool active: update preview price for the next leg.
    if state.bracket_tool.is_active() {
        let in_bounds = x >= 0.0
            && y >= 0.0
            && x <= state.camera.viewport_width as f32
            && y <= state.camera.viewport_height as f32;
        if in_bounds {
            let price = state.camera.y_to_price(y);
            state.bracket_tool.set_preview(price);
        } else {
            state.bracket_tool.preview_price = None;
        }
        state.crosshair.suppress();
        #[allow(deprecated)]
        {
            state.crosshair_pos = None;
        }
        return vec![ChartAction::ClearCrosshair];
    }

    Vec::new()
}

/// Sync crosshair state to the current `active_cursor_claim()`.
///
/// Called after tool state transitions (e.g., `try_resume_placing()`)
/// to update crosshair mode without the caller needing to know which
/// specific tool changed.
fn sync_crosshair_to_claim(state: &mut ChartState) {
    match state.active_cursor_claim() {
        CursorClaim::Preview => {
            state.crosshair.resume_preview();
        }
        CursorClaim::Suppress => {
            state.crosshair.suppress();
        }
        CursorClaim::None => {}
    }
    #[allow(deprecated)]
    {
        state.crosshair_pos = state.crosshair.render_pos();
    }
}

/// Compute the Y position of the volume handle triangle.
///
/// The handle tracks the effective volume area top, which depends on
/// `VOLUME_AREA_FRACTION` and `volume_scale`.
pub fn volume_handle_y(viewport_height: u32, volume_scale: f32) -> f32 {
    let vh = viewport_height as f32;
    let max_fraction = 0.80;
    let effective = (VOLUME_AREA_FRACTION * volume_scale).min(max_fraction);
    vh * (1.0 - effective)
}

/// Compute the Y position of the timeline border line.
pub fn timeline_border_y(viewport_height: u32, timeline_border_ratio: f32) -> f32 {
    viewport_height as f32 * (1.0 - timeline_border_ratio)
}

/// Test whether cursor (x, y) is over the volume handle triangle (right edge).
fn is_over_volume_handle(
    x: f32,
    y: f32,
    viewport_width: u32,
    viewport_height: u32,
    volume_scale: f32,
) -> bool {
    let vw = viewport_width as f32;
    let handle_center_y = volume_handle_y(viewport_height, volume_scale);
    let half_h = VOLUME_HANDLE_HEIGHT / 2.0 + VOLUME_HANDLE_HIT_PADDING;
    let x_min = vw - VOLUME_HANDLE_WIDTH - VOLUME_HANDLE_HIT_PADDING;
    x >= x_min && y >= handle_center_y - half_h && y <= handle_center_y + half_h
}

/// Test whether cursor (x, y) is near the timeline border line.
fn is_over_timeline_border(y: f32, viewport_height: u32, timeline_border_ratio: f32) -> bool {
    let border_y = timeline_border_y(viewport_height, timeline_border_ratio);
    (y - border_y).abs() <= TIMELINE_BORDER_HIT_TOLERANCE
}

/// Default tick size for price snapping during bracket leg drags.
/// Used to round dragged TP/SL prices to valid tick increments.
/// Can be made configurable per-instrument in a future iteration.
const DEFAULT_TICK_SIZE: f64 = 0.01;

/// Snap a price to the nearest tick increment.
///
/// Returns `price` unchanged if `tick_size` is zero or negative.
fn snap_to_tick(price: f64, tick_size: f64) -> f64 {
    if tick_size <= 0.0 {
        return price;
    }
    (price / tick_size).round() * tick_size
}

/// Minimum screen-space separation (px) between a bracket leg and its
/// entry line. Guarantees legs are always visually distinct and grabbable,
/// regardless of zoom level or stock price.
const MIN_LEG_SEPARATION_PX: f32 = 15.0;

/// Clamp a bracket leg price so it stays on the correct side of entry,
/// enforcing a minimum separation in **screen space** (pixels).
///
/// - Long TP must be above entry; Long SL must be below entry.
/// - Short TP must be below entry; Short SL must be above entry.
///
/// The minimum offset is derived from the camera's current zoom so that
/// legs never collapse to sub-pixel distances on zoomed-out charts.
fn clamp_bracket_leg_price(
    raw_price: f64,
    entry_price: f64,
    leg: crate::widget::order_bracket::LegRole,
    side: crate::widget::order_bracket::BracketSide,
    camera: &crate::camera::Camera2D,
) -> f64 {
    use crate::widget::order_bracket::{BracketSide, LegRole};

    // Convert minimum pixel separation to a price offset at current zoom.
    let price_range = camera.price_high - camera.price_low;
    let min_offset = if camera.viewport_height > 0 && price_range > 0.0 {
        (MIN_LEG_SEPARATION_PX as f64) * price_range / camera.viewport_height as f64
    } else {
        0.01
    };

    match (side, leg) {
        (BracketSide::Long, LegRole::TakeProfit) => raw_price.max(entry_price + min_offset),
        (BracketSide::Long, LegRole::StopLoss) => raw_price.min(entry_price - min_offset),
        (BracketSide::Short, LegRole::TakeProfit) => raw_price.min(entry_price - min_offset),
        (BracketSide::Short, LegRole::StopLoss) => raw_price.max(entry_price + min_offset),
        (_, LegRole::Entry | LegRole::StopTrigger) => raw_price,
    }
}

/// Button half-height for hit zone rectangles (pixels above/below label y).
const BUTTON_HIT_HALF_H: f32 = 8.0;
/// Right-edge padding for the first button (pixels from viewport right edge).
const BUTTON_RIGHT_PAD: f32 = 8.0;
/// Horizontal gap between adjacent buttons.
const BUTTON_GAP: f32 = 4.0;

/// Estimate pixel width of a button label: char_count * 7.0 + 12.0 padding.
fn button_width(text: &str) -> f32 {
    text.len() as f32 * 7.0 + 12.0
}

/// Hit-test bracket action buttons for a click at `(cx, cy)`.
///
/// For every Draft `OrderBracket` annotation, computes button positions
/// using the same right-to-left layout as `compute_bracket()` and checks
/// whether the click falls inside a button rect. Returns the first
/// matching `ChartAction`, or `None`.
fn hit_test_bracket_buttons(
    annotations: &[crate::widget::Annotation],
    cx: f32,
    cy: f32,
    camera: &crate::camera::Camera2D,
    vp_width: f32,
) -> Option<ChartAction> {
    use crate::widget::order_bracket::BracketStatus;

    for ann in annotations {
        if !ann.presence.is_interactive() {
            continue;
        }
        let bracket = match &ann.kind {
            crate::widget::AnnotationKind::OrderBracket(b) => b,
            _ => continue,
        };
        if bracket.status != BracketStatus::Draft {
            continue;
        }

        let ann_id = ann.id;

        // ── Entry line buttons ────────────────────────────────
        let entry_y = camera.price_to_y(bracket.entry.price);
        let mut cursor = vp_width - BUTTON_RIGHT_PAD;

        // Button order (right to left): [X], [Submit], [Save], [SL]
        // Priority for narrow viewports: drop [SL] first, then
        // [Save], then [X]. [Submit] always emits if it fits.

        // [X] cancel
        // NOTE: overflow logic must mirror emit_entry_buttons() in
        // order_bracket/mod.rs — if a button doesn't fit, ALL further
        // buttons to the left are skipped (early return).
        let x_text = "X";
        let x_w = button_width(x_text);
        let x_right = cursor;
        let x_left = x_right - x_w;
        if x_left < 0.0 {
            // Button doesn't fit — skip all remaining entry buttons.
            // Fall through to SL line button check below.
        } else {
            if hit_rect(cx, cy, x_left, entry_y, x_right) {
                return Some(ChartAction::CancelBracket {
                    annotation_id: ann_id,
                });
            }
            cursor = x_left - BUTTON_GAP;

            // [Submit] — only if entry price != 0.0
            if bracket.entry.price != 0.0 {
                let submit_text = "Submit";
                let submit_w = button_width(submit_text);
                let submit_right = cursor;
                let submit_left = submit_right - submit_w;
                if submit_left < 0.0 {
                    // Stop — no more entry buttons fit.
                } else {
                    if hit_rect(cx, cy, submit_left, entry_y, submit_right) {
                        return Some(ChartAction::SubmitBracket {
                            annotation_id: ann_id,
                        });
                    }
                    cursor = submit_left - BUTTON_GAP;
                }
            }

            // [Save] — only if [Submit] (or all prior) fit
            if cursor > 0.0 {
                let save_text = "Save";
                let save_w = button_width(save_text);
                let save_right = cursor;
                let save_left = save_right - save_w;
                if save_left >= 0.0 {
                    if hit_rect(cx, cy, save_left, entry_y, save_right) {
                        return Some(ChartAction::SaveBracket {
                            annotation_id: ann_id,
                        });
                    }
                    cursor = save_left - BUTTON_GAP;

                    // [SL] — only when SL is not set
                    if bracket.stop_loss.is_none() {
                        let sl_text = "SL";
                        let sl_w = button_width(sl_text);
                        let sl_right = cursor;
                        let sl_left = sl_right - sl_w;
                        if sl_left >= 0.0 && hit_rect(cx, cy, sl_left, entry_y, sl_right) {
                            return Some(ChartAction::ToggleBracketSL {
                                annotation_id: ann_id,
                            });
                        }
                    }
                }
            }
        }

        // ── SL line [X] button ────────────────────────────────
        if let Some(ref sl) = bracket.stop_loss {
            let sl_y = camera.price_to_y(sl.price);
            let sl_x_text = "X";
            let sl_x_w = button_width(sl_x_text);
            let sl_x_right = vp_width - BUTTON_RIGHT_PAD;
            let sl_x_left = sl_x_right - sl_x_w;
            if sl_x_left >= 0.0 && hit_rect(cx, cy, sl_x_left, sl_y, sl_x_right) {
                return Some(ChartAction::CancelBracketSL {
                    annotation_id: ann_id,
                });
            }
        }
    }

    None
}

/// Check if `(cx, cy)` falls within a button rectangle.
fn hit_rect(cx: f32, cy: f32, left: f32, center_y: f32, right: f32) -> bool {
    cx >= left
        && cx <= right
        && cy >= center_y - BUTTON_HIT_HALF_H
        && cy <= center_y + BUTTON_HIT_HALF_H
}

#[cfg(test)]
#[allow(deprecated)]
mod tests;
