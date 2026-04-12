use super::*;
use crate::camera::Camera2D;
use crate::state::{ChartState, InteractionMode, Momentum, YAnimation};
use crate::widget::price_line::{LineExtent, LineStroke, PriceLine};
use crate::widget::{
    hit_test::HitZoneKind, Annotation, AnnotationId, AnnotationKind, LineStyle, Presence,
};

fn test_state() -> ChartState {
    ChartState::new(Camera2D {
        time_start: 1_000_000.0,
        time_end: 2_000_000.0,
        price_low: 100.0,
        price_high: 200.0,
        viewport_width: 1920,
        viewport_height: 1080,
        dpi_scale: 1.0,
    })
}

fn test_level(id: u64, price: f64) -> Annotation {
    Annotation {
        id: AnnotationId(id),
        kind: AnnotationKind::Level(crate::widget::HorizontalLevel {
            id,
            line: PriceLine {
                price,
                extent: LineExtent::default(),
                stroke: LineStroke {
                    color: [1.0, 0.0, 0.0, 1.0],
                    width: 1.0,
                    style: LineStyle::default(),
                },
            },
            label: None,
            icon: crate::levels::LevelIcon::None,
        }),
        presence: Presence::Active,
        visible_timeframes: None,
        locked: false,
        created_at: 0,
        modified_at: 0,
    }
}

// ── Crosshair tests ──────────────────────────────────────────────

#[test]
fn mouse_move_in_bounds_sets_crosshair() {
    let mut state = test_state();
    // Crosshair only shows when left mouse is held down.
    state.crosshair.on_left_press(500.0, 300.0);
    let actions = handle_event(
        &mut state,
        ChartEvent::MouseMoved {
            x: 500.0,
            y: 300.0,
            alt_held: false,
        },
        None,
        false,
        &[],
    );
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0], ChartAction::SetCrosshair { x: 500.0, y: 300.0 });
    assert!(state.crosshair.should_render());
}

#[test]
fn mouse_move_without_button_does_not_set_crosshair() {
    let mut state = test_state();
    let actions = handle_event(
        &mut state,
        ChartEvent::MouseMoved {
            x: 500.0,
            y: 300.0,
            alt_held: false,
        },
        None,
        false,
        &[],
    );
    assert!(actions.is_empty());
    assert!(!state.crosshair.should_render());
}

#[test]
fn mouse_move_out_of_bounds_clears_crosshair() {
    let mut state = test_state();
    state.crosshair.on_left_press(500.0, 300.0);
    handle_event(
        &mut state,
        ChartEvent::MouseMoved {
            x: 500.0,
            y: 300.0,
            alt_held: false,
        },
        None,
        false,
        &[],
    );
    let actions = handle_event(
        &mut state,
        ChartEvent::MouseMoved {
            x: -10.0,
            y: 300.0,
            alt_held: false,
        },
        None,
        false,
        &[],
    );
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0], ChartAction::ClearCrosshair);
    assert!(!state.crosshair.should_render());
}

// ── Scroll pan tests ─────────────────────────────────────────────

#[test]
fn scroll_up_pans_forward_in_time() {
    let mut state = test_state();
    let orig_start = state.camera.time_start;

    let actions = handle_event(
        &mut state,
        ChartEvent::MouseWheel {
            delta: 1.0,
            x: 960.0,
            y: 540.0,
        },
        None,
        false,
        &[],
    );
    assert_eq!(actions.len(), 1);
    match &actions[0] {
        ChartAction::Pan { dx, dy } => {
            assert!(*dx > 0.0, "scroll up should produce positive dx (forward)");
            assert!(dy.abs() < f64::EPSILON, "scroll should not pan vertically");
        }
        other => panic!("expected Pan, got {:?}", other),
    }

    for a in &actions {
        state.apply_action(a);
    }
    assert!(
        state.camera.time_start > orig_start,
        "time_start should have moved forward: {} vs {}",
        state.camera.time_start,
        orig_start
    );
}

#[test]
fn scroll_down_pans_backward_in_time() {
    let mut state = test_state();
    let orig_start = state.camera.time_start;

    let actions = handle_event(
        &mut state,
        ChartEvent::MouseWheel {
            delta: -1.0,
            x: 960.0,
            y: 540.0,
        },
        None,
        false,
        &[],
    );
    assert_eq!(actions.len(), 1);
    match &actions[0] {
        ChartAction::Pan { dx, dy } => {
            assert!(
                *dx < 0.0,
                "scroll down should produce negative dx (backward)"
            );
            assert!(dy.abs() < f64::EPSILON, "scroll should not pan vertically");
        }
        other => panic!("expected Pan, got {:?}", other),
    }

    for a in &actions {
        state.apply_action(a);
    }
    assert!(
        state.camera.time_start < orig_start,
        "time_start should have moved backward: {} vs {}",
        state.camera.time_start,
        orig_start
    );
}

#[test]
fn mouse_wheel_zero_delta_produces_nothing() {
    let mut state = test_state();
    let actions = handle_event(
        &mut state,
        ChartEvent::MouseWheel {
            delta: 0.0,
            x: 960.0,
            y: 540.0,
        },
        None,
        false,
        &[],
    );
    assert!(actions.is_empty());
}

#[test]
fn scroll_pan_preserves_time_range() {
    let mut state = test_state();
    let orig_range = state.camera.time_end - state.camera.time_start;

    let actions = handle_event(
        &mut state,
        ChartEvent::MouseWheel {
            delta: 1.0,
            x: 960.0,
            y: 540.0,
        },
        None,
        false,
        &[],
    );
    for a in &actions {
        state.apply_action(a);
    }

    let new_range = state.camera.time_end - state.camera.time_start;
    assert!(
        (new_range - orig_range).abs() < 1.0,
        "scroll pan should not change range: {} vs {}",
        new_range,
        orig_range
    );
}

// ── State machine transition tests ──────────────────────────────

#[test]
fn left_press_enters_pending_drag() {
    let mut state = test_state();
    let actions = handle_event(
        &mut state,
        ChartEvent::MousePressed {
            x: 100.0,
            y: 200.0,
            button: MouseButton::Left,
            alt_held: false,
        },
        None,
        false,
        &[],
    );
    // Left press now also sets crosshair.
    assert!(actions.contains(&ChartAction::SetCrosshair { x: 100.0, y: 200.0 }));
    assert!(state.left_mouse_down);
    assert_eq!(
        state.interaction_mode,
        InteractionMode::PendingDrag {
            start_x: 100.0,
            start_y: 200.0,
        }
    );
    assert_eq!(state.drag_start, Some((100.0, 200.0)));
}

#[test]
fn small_move_stays_in_pending_drag() {
    let mut state = test_state();
    handle_event(
        &mut state,
        ChartEvent::MousePressed {
            x: 100.0,
            y: 200.0,
            button: MouseButton::Left,
            alt_held: false,
        },
        None,
        false,
        &[],
    );
    handle_event(
        &mut state,
        ChartEvent::MouseMoved {
            x: 102.0,
            y: 201.0,
            alt_held: false,
        },
        None,
        false,
        &[],
    );
    assert_eq!(
        state.interaction_mode,
        InteractionMode::PendingDrag {
            start_x: 100.0,
            start_y: 200.0,
        }
    );
}

#[test]
fn large_left_drag_does_not_pan() {
    let mut state = test_state();
    handle_event(
        &mut state,
        ChartEvent::MousePressed {
            x: 100.0,
            y: 200.0,
            button: MouseButton::Left,
            alt_held: false,
        },
        None,
        false,
        &[],
    );
    handle_event(
        &mut state,
        ChartEvent::MouseMoved {
            x: 110.0,
            y: 200.0,
            alt_held: false,
        },
        None,
        false,
        &[],
    );
    // Left-drag returns to Idle (no panning), not Panning.
    assert_eq!(state.interaction_mode, InteractionMode::Idle);
}

#[test]
fn right_click_pans() {
    let mut state = test_state();
    handle_event(
        &mut state,
        ChartEvent::MousePressed {
            x: 100.0,
            y: 200.0,
            button: MouseButton::Right,
            alt_held: false,
        },
        None,
        false,
        &[],
    );
    assert_eq!(state.interaction_mode, InteractionMode::RightPanning);

    handle_event(
        &mut state,
        ChartEvent::MouseReleased {
            x: 200.0,
            y: 200.0,
            button: MouseButton::Right,
            alt_held: false,
        },
        None,
        false,
        &[],
    );
    assert_eq!(state.interaction_mode, InteractionMode::Idle);
}

#[test]
fn full_cycle_right_pan() {
    let mut state = test_state();
    assert_eq!(state.interaction_mode, InteractionMode::Idle);

    handle_event(
        &mut state,
        ChartEvent::MousePressed {
            x: 100.0,
            y: 200.0,
            button: MouseButton::Right,
            alt_held: false,
        },
        None,
        false,
        &[],
    );
    assert_eq!(state.interaction_mode, InteractionMode::RightPanning);

    // Drag moves the camera.
    let actions = handle_event(
        &mut state,
        ChartEvent::MouseMoved {
            x: 200.0,
            y: 200.0,
            alt_held: false,
        },
        None,
        false,
        &[],
    );
    let has_pan = actions.iter().any(|a| matches!(a, ChartAction::Pan { .. }));
    assert!(
        has_pan,
        "expected Pan action during right-drag, got {:?}",
        actions
    );

    handle_event(
        &mut state,
        ChartEvent::MouseReleased {
            x: 200.0,
            y: 200.0,
            button: MouseButton::Right,
            alt_held: false,
        },
        None,
        false,
        &[],
    );
    assert_eq!(state.interaction_mode, InteractionMode::Idle);
}

// ── Pan tests ───────────────────────────────────────────────────

#[test]
fn right_pan_drag_100px_shifts_time() {
    let mut state = test_state();
    let orig_start = state.camera.time_start;

    // Right-press starts panning immediately.
    handle_event(
        &mut state,
        ChartEvent::MousePressed {
            x: 500.0,
            y: 540.0,
            button: MouseButton::Right,
            alt_held: false,
        },
        None,
        false,
        &[],
    );
    assert_eq!(state.interaction_mode, InteractionMode::RightPanning);

    // Drag 100px to the right.
    let actions = handle_event(
        &mut state,
        ChartEvent::MouseMoved {
            x: 600.0,
            y: 540.0,
            alt_held: false,
        },
        None,
        false,
        &[],
    );

    let pan_action = actions
        .iter()
        .find(|a| matches!(a, ChartAction::Pan { .. }));
    assert!(
        pan_action.is_some(),
        "expected a Pan action, got {:?}",
        actions
    );

    for a in &actions {
        state.apply_action(a);
    }

    // Dragging right shifts time_start backward.
    assert!(
        state.camera.time_start < orig_start,
        "time_start={} should be < orig={}",
        state.camera.time_start,
        orig_start
    );
}

// ── Momentum tests ─────────────────────────────────────────────

#[test]
fn right_pan_release_emits_start_momentum() {
    let mut state = test_state();

    handle_event(
        &mut state,
        ChartEvent::MousePressed {
            x: 500.0,
            y: 540.0,
            button: MouseButton::Right,
            alt_held: false,
        },
        None,
        false,
        &[],
    );
    handle_event(
        &mut state,
        ChartEvent::MouseMoved {
            x: 610.0,
            y: 540.0,
            alt_held: false,
        },
        None,
        false,
        &[],
    );

    let actions = handle_event(
        &mut state,
        ChartEvent::MouseReleased {
            x: 710.0,
            y: 540.0,
            button: MouseButton::Right,
            alt_held: false,
        },
        None,
        false,
        &[],
    );

    let has_momentum = actions
        .iter()
        .any(|a| matches!(a, ChartAction::StartMomentum { .. }));
    assert!(
        has_momentum,
        "expected StartMomentum action, got {:?}",
        actions
    );
}

#[test]
fn momentum_tick_moves_camera_and_eventually_stops() {
    let mut state = test_state();
    let orig_start = state.camera.time_start;

    state.momentum = Some(Momentum {
        vx: 500_000.0,
        vy: 0.0,
    });

    let mut moved = false;
    for _ in 0..5 {
        let actions = handle_event(
            &mut state,
            ChartEvent::TickMomentum {
                dt_secs: 1.0 / 60.0,
            },
            None,
            false,
            &[],
        );
        if !actions.is_empty() {
            moved = true;
        }
    }
    assert!(moved, "momentum should have produced actions");
    assert!(
        (state.camera.time_start - orig_start).abs() > 1.0,
        "camera should have moved from momentum"
    );

    for _ in 0..600 {
        handle_event(
            &mut state,
            ChartEvent::TickMomentum {
                dt_secs: 1.0 / 60.0,
            },
            None,
            false,
            &[],
        );
    }
    assert!(
        state.momentum.is_none(),
        "momentum should have stopped after enough ticks"
    );
}

#[test]
fn momentum_converges_to_stop() {
    let mut state = test_state();
    state.momentum = Some(Momentum { vx: 1.0, vy: 0.0 });
    for _ in 0..600 {
        if !state.tick_momentum(1.0 / 60.0) {
            break;
        }
    }
    assert!(state.momentum.is_none(), "momentum should have stopped");
}

// ── Auto-scale tests ───────────────────────────────────────────

#[test]
fn auto_scale_tick_approaches_target() {
    let mut state = test_state();
    state.y_animation = Some(YAnimation {
        target_low: 50.0,
        target_high: 250.0,
    });

    let actions = handle_event(
        &mut state,
        ChartEvent::TickAutoScale {
            dt_secs: 1.0 / 60.0,
        },
        None,
        false,
        &[],
    );
    assert!(!actions.is_empty(), "should produce Redraw");

    assert!(
        state.camera.price_low < 100.0,
        "price_low={} should have decreased toward 50",
        state.camera.price_low
    );
    assert!(
        state.camera.price_high > 200.0,
        "price_high={} should have increased toward 250",
        state.camera.price_high
    );
}

#[test]
fn auto_scale_converges_and_stops() {
    let mut state = test_state();
    state.y_animation = Some(YAnimation {
        target_low: 90.0,
        target_high: 210.0,
    });

    for _ in 0..600 {
        handle_event(
            &mut state,
            ChartEvent::TickAutoScale {
                dt_secs: 1.0 / 60.0,
            },
            None,
            false,
            &[],
        );
    }

    assert!(
        state.y_animation.is_none(),
        "animation should have converged"
    );
    assert!(
        (state.camera.price_low - 90.0).abs() < 0.1,
        "price_low={}, expected ~90.0",
        state.camera.price_low
    );
    assert!(
        (state.camera.price_high - 210.0).abs() < 0.1,
        "price_high={}, expected ~210.0",
        state.camera.price_high
    );
}

// ── Level creation tests ───────────────────────────────────────

#[test]
fn double_click_creates_level() {
    let mut state = test_state();
    let actions = handle_event(
        &mut state,
        ChartEvent::DoubleClick { x: 960.0, y: 540.0 },
        None,
        false,
        &[],
    );
    assert_eq!(actions.len(), 1);
    match &actions[0] {
        ChartAction::CreateLevel { price } => {
            let expected = state.camera.y_to_price(540.0);
            assert!(
                (price - expected).abs() < 0.01,
                "price={}, expected={}",
                price,
                expected
            );
        }
        other => panic!("expected CreateLevel, got {:?}", other),
    }
}

#[test]
fn middle_press_enters_pending_scale() {
    let mut state = test_state();
    let actions = handle_event(
        &mut state,
        ChartEvent::MousePressed {
            x: 960.0,
            y: 540.0,
            button: MouseButton::Middle,
            alt_held: false,
        },
        None,
        false,
        &[],
    );
    assert!(actions.is_empty());
    assert_eq!(
        state.interaction_mode,
        InteractionMode::PendingScale {
            start_x: 960.0,
            start_y: 540.0,
        },
        "expected PendingScale, got {:?}",
        state.interaction_mode
    );
}

#[test]
fn middle_drag_dead_zone_no_action() {
    let mut state = test_state();
    handle_event(
        &mut state,
        ChartEvent::MousePressed {
            x: 500.0,
            y: 500.0,
            button: MouseButton::Middle,
            alt_held: false,
        },
        None,
        false,
        &[],
    );
    // Move less than 6px — should stay in PendingScale.
    let actions = handle_event(
        &mut state,
        ChartEvent::MouseMoved {
            x: 504.0,
            y: 502.0,
            alt_held: false,
        },
        None,
        false,
        &[],
    );
    // No scaling actions should be produced in the dead zone.
    let has_zoom = actions
        .iter()
        .any(|a| matches!(a, ChartAction::Zoom { .. } | ChartAction::ZoomY { .. }));
    assert!(
        !has_zoom,
        "should not produce zoom actions in dead zone, got {:?}",
        actions
    );
    assert_eq!(
        state.interaction_mode,
        InteractionMode::PendingScale {
            start_x: 500.0,
            start_y: 500.0,
        },
        "should remain in PendingScale"
    );
}

#[test]
fn middle_drag_horizontal_scales_time_axis() {
    let mut state = test_state();
    handle_event(
        &mut state,
        ChartEvent::MousePressed {
            x: 500.0,
            y: 500.0,
            button: MouseButton::Middle,
            alt_held: false,
        },
        None,
        false,
        &[],
    );
    // Move 20px to the right (strongly horizontal).
    handle_event(
        &mut state,
        ChartEvent::MouseMoved {
            x: 520.0,
            y: 501.0,
            alt_held: false,
        },
        None,
        false,
        &[],
    );
    assert!(
        matches!(
            state.interaction_mode,
            InteractionMode::HorizontalScaling { .. }
        ),
        "expected HorizontalScaling, got {:?}",
        state.interaction_mode
    );

    // Further horizontal movement should produce Zoom actions.
    let actions = handle_event(
        &mut state,
        ChartEvent::MouseMoved {
            x: 540.0,
            y: 501.0,
            alt_held: false,
        },
        None,
        false,
        &[],
    );
    let has_zoom = actions
        .iter()
        .any(|a| matches!(a, ChartAction::Zoom { .. }));
    assert!(
        has_zoom,
        "horizontal scaling should produce Zoom, got {:?}",
        actions
    );
}

#[test]
fn middle_drag_vertical_scales_price_axis() {
    let mut state = test_state();
    handle_event(
        &mut state,
        ChartEvent::MousePressed {
            x: 500.0,
            y: 500.0,
            button: MouseButton::Middle,
            alt_held: false,
        },
        None,
        false,
        &[],
    );
    // Move 20px upward (strongly vertical).
    handle_event(
        &mut state,
        ChartEvent::MouseMoved {
            x: 501.0,
            y: 480.0,
            alt_held: false,
        },
        None,
        false,
        &[],
    );
    assert!(
        matches!(
            state.interaction_mode,
            InteractionMode::VerticalScaling { .. }
        ),
        "expected VerticalScaling, got {:?}",
        state.interaction_mode
    );

    // Further vertical movement should produce ZoomY actions.
    let actions = handle_event(
        &mut state,
        ChartEvent::MouseMoved {
            x: 501.0,
            y: 460.0,
            alt_held: false,
        },
        None,
        false,
        &[],
    );
    let has_zoom_y = actions
        .iter()
        .any(|a| matches!(a, ChartAction::ZoomY { .. }));
    assert!(
        has_zoom_y,
        "vertical scaling should produce ZoomY, got {:?}",
        actions
    );
}

#[test]
fn middle_drag_axis_lock_persists() {
    let mut state = test_state();
    handle_event(
        &mut state,
        ChartEvent::MousePressed {
            x: 500.0,
            y: 500.0,
            button: MouseButton::Middle,
            alt_held: false,
        },
        None,
        false,
        &[],
    );
    // Move 20px to the right to lock horizontal.
    handle_event(
        &mut state,
        ChartEvent::MouseMoved {
            x: 520.0,
            y: 501.0,
            alt_held: false,
        },
        None,
        false,
        &[],
    );
    assert!(
        matches!(
            state.interaction_mode,
            InteractionMode::HorizontalScaling { .. }
        ),
        "should be HorizontalScaling"
    );

    // Now move mostly vertical — axis should STAY horizontal.
    handle_event(
        &mut state,
        ChartEvent::MouseMoved {
            x: 520.0,
            y: 600.0,
            alt_held: false,
        },
        None,
        false,
        &[],
    );
    assert!(
        matches!(
            state.interaction_mode,
            InteractionMode::HorizontalScaling { .. }
        ),
        "axis lock should persist even with vertical movement, got {:?}",
        state.interaction_mode
    );
}

#[test]
fn middle_release_returns_to_idle() {
    let mut state = test_state();
    handle_event(
        &mut state,
        ChartEvent::MousePressed {
            x: 500.0,
            y: 500.0,
            button: MouseButton::Middle,
            alt_held: false,
        },
        None,
        false,
        &[],
    );
    // Move to lock horizontal.
    handle_event(
        &mut state,
        ChartEvent::MouseMoved {
            x: 520.0,
            y: 501.0,
            alt_held: false,
        },
        None,
        false,
        &[],
    );
    // Release middle button.
    handle_event(
        &mut state,
        ChartEvent::MouseReleased {
            x: 520.0,
            y: 501.0,
            button: MouseButton::Middle,
            alt_held: false,
        },
        None,
        false,
        &[],
    );
    assert_eq!(
        state.interaction_mode,
        InteractionMode::Idle,
        "middle release should return to Idle"
    );
}

#[test]
fn create_level_and_apply_emits_action() {
    let mut state = test_state();
    let actions = handle_event(
        &mut state,
        ChartEvent::DoubleClick { x: 960.0, y: 540.0 },
        None,
        false,
        &[],
    );
    assert_eq!(actions.len(), 1);
    assert!(matches!(actions[0], ChartAction::CreateLevel { .. }));
}

// ── Level selection tests ──────────────────────────────────────

#[test]
fn click_near_level_selects_it() {
    let mut state = test_state();
    let levels = vec![test_level(1, 150.0)];
    let level_y = state.camera.price_to_y(150.0);

    handle_event(
        &mut state,
        ChartEvent::MousePressed {
            x: 500.0,
            y: level_y,
            button: MouseButton::Left,
            alt_held: false,
        },
        None,
        false,
        &levels,
    );
    let actions = handle_event(
        &mut state,
        ChartEvent::MouseReleased {
            x: 500.0,
            y: level_y,
            button: MouseButton::Left,
            alt_held: false,
        },
        None,
        false,
        &levels,
    );

    let has_select = actions
        .iter()
        .any(|a| matches!(a, ChartAction::SelectLevel { id } if *id == AnnotationId(1)));
    assert!(has_select, "expected SelectLevel action, got {:?}", actions);
}

#[test]
fn click_far_from_level_deselects() {
    let mut state = test_state();
    let levels = vec![test_level(1, 150.0)];
    state.selected_level = Some(AnnotationId(1));

    handle_event(
        &mut state,
        ChartEvent::MousePressed {
            x: 500.0,
            y: 10.0,
            button: MouseButton::Left,
            alt_held: false,
        },
        None,
        false,
        &levels,
    );
    let actions = handle_event(
        &mut state,
        ChartEvent::MouseReleased {
            x: 500.0,
            y: 10.0,
            button: MouseButton::Left,
            alt_held: false,
        },
        None,
        false,
        &levels,
    );

    let has_deselect = actions
        .iter()
        .any(|a| matches!(a, ChartAction::DeselectLevel));
    assert!(
        has_deselect,
        "expected DeselectLevel action, got {:?}",
        actions
    );
}

// ── Level drag tests ───────────────────────────────────────────

#[test]
fn drag_near_level_transitions_to_dragging_level() {
    let mut state = test_state();
    let levels = vec![test_level(1, 150.0)];
    let level_y = state.camera.price_to_y(150.0);

    handle_event(
        &mut state,
        ChartEvent::MousePressed {
            x: 500.0,
            y: level_y,
            button: MouseButton::Left,
            alt_held: false,
        },
        None,
        false,
        &levels,
    );
    handle_event(
        &mut state,
        ChartEvent::MouseMoved {
            x: 500.0,
            y: level_y + 10.0,
            alt_held: false,
        },
        None,
        false,
        &levels,
    );

    assert!(
        matches!(
            state.interaction_mode,
            InteractionMode::DraggingAnnotation {
                element: HitZoneKind::LevelLine,
                ..
            }
        ),
        "expected DraggingAnnotation(LevelLine), got {:?}",
        state.interaction_mode
    );
}

#[test]
fn dragging_level_updates_price() {
    let mut state = test_state();
    let levels = vec![test_level(1, 150.0)];
    let level_y = state.camera.price_to_y(150.0);

    handle_event(
        &mut state,
        ChartEvent::MousePressed {
            x: 500.0,
            y: level_y,
            button: MouseButton::Left,
            alt_held: false,
        },
        None,
        false,
        &levels,
    );
    handle_event(
        &mut state,
        ChartEvent::MouseMoved {
            x: 500.0,
            y: level_y + 10.0,
            alt_held: false,
        },
        None,
        false,
        &levels,
    );

    let new_y = level_y + 50.0;
    let actions = handle_event(
        &mut state,
        ChartEvent::MouseMoved {
            x: 500.0,
            y: new_y,
            alt_held: false,
        },
        None,
        false,
        &levels,
    );

    let has_drag = actions
        .iter()
        .any(|a| matches!(a, ChartAction::DragLevel { id, .. } if *id == AnnotationId(1)));
    assert!(has_drag, "expected DragLevel action, got {:?}", actions);
}

// ── Level delete tests ─────────────────────────────────────────

#[test]
fn delete_key_removes_selected_level() {
    let mut state = test_state();
    state.selected_level = Some(AnnotationId(1));

    let actions = handle_event(
        &mut state,
        ChartEvent::KeyPressed { key: Key::Delete },
        None,
        false,
        &[],
    );

    let has_delete = actions
        .iter()
        .any(|a| matches!(a, ChartAction::DeleteSelectedLevel));
    assert!(
        has_delete,
        "expected DeleteSelectedLevel, got {:?}",
        actions
    );
}

#[test]
fn delete_key_without_selection_does_nothing() {
    let mut state = test_state();
    let actions = handle_event(
        &mut state,
        ChartEvent::KeyPressed { key: Key::Delete },
        None,
        false,
        &[],
    );
    assert!(actions.is_empty());
}

// ── Escape key tests ───────────────────────────────────────────

#[test]
fn escape_deselects_level_and_clears_crosshair() {
    let mut state = test_state();
    state.selected_level = Some(AnnotationId(1));
    state.crosshair_pos = Some((100.0, 200.0));

    let actions = handle_event(
        &mut state,
        ChartEvent::KeyPressed { key: Key::Escape },
        None,
        false,
        &[],
    );

    let has_deselect = actions
        .iter()
        .any(|a| matches!(a, ChartAction::DeselectLevel));
    let has_clear = actions
        .iter()
        .any(|a| matches!(a, ChartAction::ClearCrosshair));
    assert!(has_deselect, "expected DeselectLevel in {:?}", actions);
    assert!(has_clear, "expected ClearCrosshair in {:?}", actions);
}

// ── Resize test ────────────────────────────────────────────────

#[test]
fn resize_updates_camera_viewport() {
    let mut state = test_state();
    let actions = handle_event(
        &mut state,
        ChartEvent::Resize {
            width: 2560,
            height: 1440,
        },
        None,
        false,
        &[],
    );
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0], ChartAction::Redraw);
    assert_eq!(state.camera.viewport_width, 2560);
    assert_eq!(state.camera.viewport_height, 1440);
}

// ── Home/End key tests ─────────────────────────────────────────

#[test]
fn home_key_emits_jump_to_start() {
    let mut state = test_state();
    let actions = handle_event(
        &mut state,
        ChartEvent::KeyPressed { key: Key::Home },
        None,
        false,
        &[],
    );
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0], ChartAction::JumpToStart);
}

#[test]
fn end_key_emits_jump_to_end() {
    let mut state = test_state();
    let actions = handle_event(
        &mut state,
        ChartEvent::KeyPressed { key: Key::End },
        None,
        false,
        &[],
    );
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0], ChartAction::JumpToEnd);
}

// ── Momentum stops on new press ────────────────────────────────

#[test]
fn left_press_stops_active_momentum() {
    let mut state = test_state();
    state.momentum = Some(Momentum {
        vx: 100_000.0,
        vy: 0.0,
    });

    let actions = handle_event(
        &mut state,
        ChartEvent::MousePressed {
            x: 500.0,
            y: 500.0,
            button: MouseButton::Left,
            alt_held: false,
        },
        None,
        false,
        &[],
    );

    let has_stop = actions
        .iter()
        .any(|a| matches!(a, ChartAction::StopMomentum));
    assert!(
        has_stop,
        "pressing during momentum should stop it, got {:?}",
        actions
    );
}

// ── Hit-test annotation (unified) tests ────────────────────────

#[test]
fn hit_test_annotation_finds_closest_level() {
    let camera = Camera2D {
        time_start: 0.0,
        time_end: 1000.0,
        price_low: 0.0,
        price_high: 100.0,
        viewport_width: 1000,
        viewport_height: 1000,
        dpi_scale: 1.0,
    };
    let levels = vec![test_level(1, 50.0), test_level(2, 55.0)];

    let result = hit_test_annotation(&levels, 500.0, &camera);
    assert!(result.is_some());
    let (id, kind, _, _) = result.unwrap();
    assert_eq!(id, AnnotationId(1));
    assert_eq!(kind, HitZoneKind::LevelLine);

    let result = hit_test_annotation(&levels, 450.0, &camera);
    assert!(result.is_some());
    assert_eq!(result.unwrap().0, AnnotationId(2));

    let result = hit_test_annotation(&levels, 200.0, &camera);
    assert!(result.is_none());
}

// ── Release in PendingDrag (click without drag) ────────────────

#[test]
fn click_on_empty_deselects() {
    let mut state = test_state();
    state.selected_level = Some(AnnotationId(42));

    handle_event(
        &mut state,
        ChartEvent::MousePressed {
            x: 500.0,
            y: 500.0,
            button: MouseButton::Left,
            alt_held: false,
        },
        None,
        false,
        &[],
    );
    let actions = handle_event(
        &mut state,
        ChartEvent::MouseReleased {
            x: 500.0,
            y: 500.0,
            button: MouseButton::Left,
            alt_held: false,
        },
        None,
        false,
        &[],
    );

    let has_deselect = actions
        .iter()
        .any(|a| matches!(a, ChartAction::DeselectLevel));
    assert!(
        has_deselect,
        "click on empty space should deselect, got {:?}",
        actions
    );
}

#[test]
fn mouse_release_clears_drag() {
    let mut state = test_state();
    state.drag_start = Some((100.0, 200.0));
    state.interaction_mode = InteractionMode::Panning;
    let _actions = handle_event(
        &mut state,
        ChartEvent::MouseReleased {
            x: 100.0,
            y: 200.0,
            button: MouseButton::Left,
            alt_held: false,
        },
        None,
        false,
        &[],
    );
    assert_eq!(state.drag_start, None);
    assert_eq!(state.interaction_mode, InteractionMode::Idle);
}

// ── Bracket leg drag tests ──────────────────────────────────────

use crate::widget::order_bracket::{
    BracketLeg, BracketSide, BracketStatus, EntryType, LegRole, OrderBracket,
};

fn make_bracket_leg(price: f64) -> BracketLeg {
    use crate::widget::price_line::{LineExtent, LineStroke, PriceLine};
    BracketLeg {
        line: PriceLine {
            price,
            extent: LineExtent::FullWidth,
            stroke: LineStroke {
                color: [0.0, 0.0, 0.0, 1.0],
                width: 1.0,
                style: crate::widget::level::LineStyle::default(),
            },
        },
        role: LegRole::Entry,
        projected_pnl: None,
        projected_pnl_pct: None,
    }
}

fn test_bracket_annotation(id: u64, entry: f64, tp: f64, sl: f64) -> Annotation {
    Annotation {
        id: AnnotationId(id),
        kind: AnnotationKind::OrderBracket(Box::new(OrderBracket {
            entry: make_bracket_leg(entry),
            take_profit: Some(make_bracket_leg(tp)),
            stop_loss: Some(make_bracket_leg(sl)),
            side: BracketSide::Long,
            status: BracketStatus::Active,
            quantity: Some(100.0),
            saved: false,
            filled_qty: None,
            entry_type: EntryType::Market,
            entry_stop_price: None,
            wrong_side_warning: false,
        })),
        presence: Presence::Active,
        visible_timeframes: None,
        locked: false,
        created_at: 0,
        modified_at: 0,
    }
}

#[test]
fn test_drag_bracket_leg_action_created() {
    let action = ChartAction::DragBracketLeg {
        annotation_id: AnnotationId(1),
        leg: LegRole::TakeProfit,
        new_price: 195.0,
    };
    match action {
        ChartAction::DragBracketLeg { new_price, .. } => {
            assert!((new_price - 195.0).abs() < f64::EPSILON);
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn test_drag_bracket_leg_action_stop_loss() {
    let action = ChartAction::DragBracketLeg {
        annotation_id: AnnotationId(2),
        leg: LegRole::StopLoss,
        new_price: 178.0,
    };
    match action {
        ChartAction::DragBracketLeg {
            annotation_id,
            leg,
            new_price,
        } => {
            assert_eq!(annotation_id, AnnotationId(2));
            assert_eq!(leg, LegRole::StopLoss);
            assert!((new_price - 178.0).abs() < f64::EPSILON);
        }
        _ => panic!("wrong variant"),
    }
}

/// Camera with $30 range over 1080px — 36px/dollar.
/// MIN_LEG_SEPARATION_PX (15) → ~$0.42 minimum offset.
/// Test prices are 5-10 dollars apart, so the pixel minimum doesn't interfere.
fn clamp_test_camera() -> Camera2D {
    Camera2D {
        viewport_width: 1920,
        viewport_height: 1080,
        time_start: 0.0,
        time_end: 100_000.0,
        price_low: 170.0,
        price_high: 200.0,
        dpi_scale: 1.0,
    }
}

#[test]
fn test_clamp_bracket_leg_long_tp_above_entry() {
    let cam = clamp_test_camera();
    let clamped =
        clamp_bracket_leg_price(195.0, 185.0, LegRole::TakeProfit, BracketSide::Long, &cam);
    assert!((clamped - 195.0).abs() < f64::EPSILON);
}

#[test]
fn test_clamp_bracket_leg_long_tp_below_entry_clamped() {
    let cam = clamp_test_camera();
    let clamped =
        clamp_bracket_leg_price(180.0, 185.0, LegRole::TakeProfit, BracketSide::Long, &cam);
    assert!(
        clamped > 185.0,
        "Long TP must be above entry, got {}",
        clamped
    );
}

#[test]
fn test_clamp_bracket_leg_long_sl_below_entry() {
    let cam = clamp_test_camera();
    let clamped = clamp_bracket_leg_price(175.0, 185.0, LegRole::StopLoss, BracketSide::Long, &cam);
    assert!((clamped - 175.0).abs() < f64::EPSILON);
}

#[test]
fn test_clamp_bracket_leg_long_sl_above_entry_clamped() {
    let cam = clamp_test_camera();
    let clamped = clamp_bracket_leg_price(190.0, 185.0, LegRole::StopLoss, BracketSide::Long, &cam);
    assert!(
        clamped < 185.0,
        "Long SL must be below entry, got {}",
        clamped
    );
}

#[test]
fn test_clamp_bracket_leg_short_tp_below_entry() {
    let cam = clamp_test_camera();
    let clamped =
        clamp_bracket_leg_price(175.0, 185.0, LegRole::TakeProfit, BracketSide::Short, &cam);
    assert!((clamped - 175.0).abs() < f64::EPSILON);
}

#[test]
fn test_clamp_bracket_leg_short_tp_above_entry_clamped() {
    let cam = clamp_test_camera();
    let clamped =
        clamp_bracket_leg_price(190.0, 185.0, LegRole::TakeProfit, BracketSide::Short, &cam);
    assert!(
        clamped < 185.0,
        "Short TP must be below entry, got {}",
        clamped
    );
}

#[test]
fn test_clamp_bracket_leg_short_sl_above_entry() {
    let cam = clamp_test_camera();
    let clamped =
        clamp_bracket_leg_price(195.0, 185.0, LegRole::StopLoss, BracketSide::Short, &cam);
    assert!((clamped - 195.0).abs() < f64::EPSILON);
}

#[test]
fn test_clamp_bracket_leg_short_sl_below_entry_clamped() {
    let cam = clamp_test_camera();
    let clamped =
        clamp_bracket_leg_price(180.0, 185.0, LegRole::StopLoss, BracketSide::Short, &cam);
    assert!(
        clamped > 185.0,
        "Short SL must be above entry, got {}",
        clamped
    );
}

#[test]
fn test_clamp_bracket_leg_pixel_minimum_enforced() {
    // Zoomed-out camera: $200 range over 1080px → ~5.4 px/dollar.
    // MIN_LEG_SEPARATION_PX (15) → ~$2.78 minimum offset.
    let cam = Camera2D {
        viewport_width: 1920,
        viewport_height: 1080,
        time_start: 0.0,
        time_end: 100_000.0,
        price_low: 100.0,
        price_high: 300.0,
        dpi_scale: 1.0,
    };
    // Try to place Long TP at entry + $0.50 — should be pushed out to ~entry + $2.78.
    let clamped =
        clamp_bracket_leg_price(185.50, 185.0, LegRole::TakeProfit, BracketSide::Long, &cam);
    let offset = clamped - 185.0;
    assert!(
        offset > 2.0,
        "Pixel minimum should push TP further from entry, offset = {offset}"
    );
}

#[test]
fn hit_test_annotation_finds_tp() {
    let state = test_state();
    let bracket = test_bracket_annotation(10, 150.0, 170.0, 130.0);
    let tp_y = state.camera.price_to_y(170.0);

    let result = hit_test_annotation(&[bracket], tp_y, &state.camera);
    assert!(result.is_some(), "should hit TP leg");
    let (id, kind, _, _) = result.unwrap();
    assert_eq!(id, AnnotationId(10));
    assert_eq!(kind, HitZoneKind::BracketTP);
}

#[test]
fn hit_test_annotation_finds_sl() {
    let state = test_state();
    let bracket = test_bracket_annotation(10, 150.0, 170.0, 130.0);
    let sl_y = state.camera.price_to_y(130.0);

    let result = hit_test_annotation(&[bracket], sl_y, &state.camera);
    assert!(result.is_some(), "should hit SL leg");
    let (id, kind, _, _) = result.unwrap();
    assert_eq!(id, AnnotationId(10));
    assert_eq!(kind, HitZoneKind::BracketSL);
}

#[test]
fn hit_test_annotation_misses_market_entry() {
    let state = test_state();
    let bracket = test_bracket_annotation(10, 150.0, 170.0, 130.0);
    let entry_y = state.camera.price_to_y(150.0);

    // Market entry should NOT be hit-testable (not draggable).
    let result = hit_test_annotation(&[bracket], entry_y, &state.camera);
    // May hit TP or SL if they're nearby; check it's not BracketEntry.
    if let Some((_, kind, _, _)) = result {
        assert_ne!(
            kind,
            HitZoneKind::BracketEntry,
            "Market entry should not be draggable"
        );
    }
}

#[test]
fn hit_test_annotation_finds_entry_for_draft_limit() {
    let state = test_state();
    let mut bracket = test_bracket_annotation(10, 150.0, 170.0, 130.0);
    if let AnnotationKind::OrderBracket(ref mut b) = bracket.kind {
        b.status = BracketStatus::Draft;
        b.entry_type = EntryType::Limit;
    }
    let entry_y = state.camera.price_to_y(150.0);

    let result = hit_test_annotation(&[bracket], entry_y, &state.camera);
    assert!(result.is_some(), "Draft Limit entry should be hit-testable");
    assert_eq!(result.unwrap().1, HitZoneKind::BracketEntry);
}

#[test]
fn hit_test_annotation_misses_entry_for_draft_market() {
    let state = test_state();
    let mut bracket = test_bracket_annotation(10, 150.0, 170.0, 130.0);
    if let AnnotationKind::OrderBracket(ref mut b) = bracket.kind {
        b.status = BracketStatus::Draft;
        b.entry_type = EntryType::Market;
    }
    let entry_y = state.camera.price_to_y(150.0);

    let result = hit_test_annotation(&[bracket], entry_y, &state.camera);
    if let Some((_, kind, _, _)) = result {
        assert_ne!(
            kind,
            HitZoneKind::BracketEntry,
            "Draft Market entry should not be draggable"
        );
    }
}

#[test]
fn hit_test_annotation_misses_entry_for_pending_limit() {
    let state = test_state();
    let mut bracket = test_bracket_annotation(10, 150.0, 170.0, 130.0);
    if let AnnotationKind::OrderBracket(ref mut b) = bracket.kind {
        b.status = BracketStatus::Pending;
        b.entry_type = EntryType::Limit;
    }
    let entry_y = state.camera.price_to_y(150.0);

    let result = hit_test_annotation(&[bracket], entry_y, &state.camera);
    if let Some((_, kind, _, _)) = result {
        assert_ne!(
            kind,
            HitZoneKind::BracketEntry,
            "Pending Limit entry should not be draggable"
        );
    }
}

#[test]
fn hit_test_annotation_finds_entry_for_draft_stop() {
    let state = test_state();
    let mut bracket = test_bracket_annotation(10, 150.0, 170.0, 130.0);
    if let AnnotationKind::OrderBracket(ref mut b) = bracket.kind {
        b.status = BracketStatus::Draft;
        b.entry_type = EntryType::Stop;
    }
    let entry_y = state.camera.price_to_y(150.0);

    let result = hit_test_annotation(&[bracket], entry_y, &state.camera);
    assert!(result.is_some(), "Draft Stop entry should be hit-testable");
    assert_eq!(result.unwrap().1, HitZoneKind::BracketEntry);
}

#[test]
fn hit_test_annotation_finds_entry_for_draft_stop_limit() {
    let state = test_state();
    let mut bracket = test_bracket_annotation(10, 150.0, 170.0, 130.0);
    if let AnnotationKind::OrderBracket(ref mut b) = bracket.kind {
        b.status = BracketStatus::Draft;
        b.entry_type = EntryType::StopLimit;
    }
    let entry_y = state.camera.price_to_y(150.0);

    let result = hit_test_annotation(&[bracket], entry_y, &state.camera);
    assert!(
        result.is_some(),
        "Draft StopLimit entry should be hit-testable"
    );
    assert_eq!(result.unwrap().1, HitZoneKind::BracketEntry);
}

#[test]
fn hit_test_annotation_skips_locked() {
    let state = test_state();
    let mut bracket = test_bracket_annotation(10, 150.0, 170.0, 130.0);
    bracket.locked = true;
    let tp_y = state.camera.price_to_y(170.0);

    let result = hit_test_annotation(&[bracket], tp_y, &state.camera);
    assert!(result.is_none(), "locked bracket legs should not be hit");
}

#[test]
fn hit_test_annotation_skips_ghost() {
    let state = test_state();
    let mut bracket = test_bracket_annotation(10, 150.0, 170.0, 130.0);
    bracket.presence = Presence::Ghost;
    let tp_y = state.camera.price_to_y(170.0);

    let result = hit_test_annotation(&[bracket], tp_y, &state.camera);
    assert!(result.is_none(), "ghost bracket legs should not be hit");
}

#[test]
fn pending_drag_transitions_to_dragging_bracket_leg() {
    let mut state = test_state();
    // Place bracket with TP at price 170 (well above price_low=100).
    let bracket = test_bracket_annotation(10, 150.0, 170.0, 130.0);
    let tp_y = state.camera.price_to_y(170.0);
    let annotations = [bracket];

    // Press left button on TP line.
    handle_event(
        &mut state,
        ChartEvent::MousePressed {
            x: 500.0,
            y: tp_y,
            button: MouseButton::Left,
            alt_held: false,
        },
        None,
        false,
        &annotations,
    );
    assert!(
        matches!(state.interaction_mode, InteractionMode::PendingDrag { .. }),
        "should enter PendingDrag, got {:?}",
        state.interaction_mode
    );

    // Move past drag threshold (>4px).
    let actions = handle_event(
        &mut state,
        ChartEvent::MouseMoved {
            x: 500.0,
            y: tp_y + 10.0,
            alt_held: false,
        },
        None,
        false,
        &annotations,
    );

    assert!(
        matches!(
            state.interaction_mode,
            InteractionMode::DraggingAnnotation { .. }
        ),
        "should transition to DraggingAnnotation, got {:?}",
        state.interaction_mode
    );
    // Should emit ClearCrosshair (crosshair suppressed during drag).
    assert!(
        actions.contains(&ChartAction::ClearCrosshair),
        "should emit ClearCrosshair"
    );
}

#[test]
fn dragging_bracket_leg_emits_drag_action() {
    let mut state = test_state();
    let bracket = test_bracket_annotation(10, 150.0, 170.0, 130.0);
    let tp_y = state.camera.price_to_y(170.0);
    let annotations = [bracket];

    // Press and drag past threshold to enter DraggingAnnotation.
    handle_event(
        &mut state,
        ChartEvent::MousePressed {
            x: 500.0,
            y: tp_y,
            button: MouseButton::Left,
            alt_held: false,
        },
        None,
        false,
        &annotations,
    );
    handle_event(
        &mut state,
        ChartEvent::MouseMoved {
            x: 500.0,
            y: tp_y + 10.0,
            alt_held: false,
        },
        None,
        false,
        &annotations,
    );
    assert!(matches!(
        state.interaction_mode,
        InteractionMode::DraggingAnnotation { .. }
    ));

    // Continue dragging — should emit DragBracketLeg action.
    let actions = handle_event(
        &mut state,
        ChartEvent::MouseMoved {
            x: 500.0,
            y: tp_y + 20.0,
            alt_held: false,
        },
        None,
        false,
        &annotations,
    );

    let has_drag = actions.iter().any(|a| {
        matches!(
            a,
            ChartAction::DragBracketLeg {
                annotation_id: AnnotationId(10),
                leg: LegRole::TakeProfit,
                ..
            }
        )
    });
    assert!(has_drag, "should emit DragBracketLeg, got: {:?}", actions);
}

#[test]
fn releasing_bracket_leg_drag_returns_to_idle() {
    let mut state = test_state();
    let bracket = test_bracket_annotation(10, 150.0, 170.0, 130.0);
    let tp_y = state.camera.price_to_y(170.0);
    let annotations = [bracket];

    // Press, drag past threshold, then release.
    handle_event(
        &mut state,
        ChartEvent::MousePressed {
            x: 500.0,
            y: tp_y,
            button: MouseButton::Left,
            alt_held: false,
        },
        None,
        false,
        &annotations,
    );
    handle_event(
        &mut state,
        ChartEvent::MouseMoved {
            x: 500.0,
            y: tp_y + 10.0,
            alt_held: false,
        },
        None,
        false,
        &annotations,
    );
    assert!(matches!(
        state.interaction_mode,
        InteractionMode::DraggingAnnotation { .. }
    ));

    // Release.
    let actions = handle_event(
        &mut state,
        ChartEvent::MouseReleased {
            x: 500.0,
            y: tp_y + 20.0,
            button: MouseButton::Left,
            alt_held: false,
        },
        None,
        false,
        &annotations,
    );

    assert_eq!(
        state.interaction_mode,
        InteractionMode::Idle,
        "should return to Idle"
    );
    assert!(
        actions.contains(&ChartAction::ClearCrosshair),
        "should emit ClearCrosshair on release"
    );
}

// ── Slice 6: decorator click routing ─────────────────────────────

#[test]
fn hit_to_chart_action_emits_decorator_click() {
    use crate::widget::decorator::DecoratorAction;
    use crate::widget::hit_test::{HitResult, HitZoneKind, ItemPath};
    use crate::widget::AnnotationId;

    let hit = HitResult {
        annotation_id: AnnotationId(42),
        zone: HitZoneKind::Decorator {
            group_id: 7,
            item_path: ItemPath::new(&[2, 0]),
            action: DecoratorAction::CloseAnnotation,
        },
        distance: 0.0,
    };

    let action = super::hit_to_chart_action(&hit)
        .expect("decorator hit should lower to a ChartAction::DecoratorClick");

    match action {
        ChartAction::DecoratorClick {
            annotation_id,
            group_id,
            item_path,
            action,
        } => {
            assert_eq!(annotation_id, AnnotationId(42));
            assert_eq!(group_id, 7);
            assert_eq!(item_path.as_slice(), &[2, 0]);
            assert_eq!(action, DecoratorAction::CloseAnnotation);
        }
        other => panic!("expected DecoratorClick, got {other:?}"),
    }
}

#[test]
fn hit_to_chart_action_passes_through_every_decorator_action_variant() {
    use crate::widget::decorator::DecoratorAction;
    use crate::widget::hit_test::{HitResult, HitZoneKind, ItemPath};
    use crate::widget::AnnotationId;

    // Every variant should round-trip through `hit_to_chart_action`
    // so the Slice 6 routing is total across the `DecoratorAction`
    // vocabulary, not just `CloseAnnotation`.
    let variants = [
        DecoratorAction::CloseAnnotation,
        DecoratorAction::CreateTakeProfit,
        DecoratorAction::CreateStopLoss,
        DecoratorAction::CycleEntryType,
        DecoratorAction::EditQuantity,
        DecoratorAction::EditPrice,
        DecoratorAction::ToggleLocked,
        DecoratorAction::Submit,
        DecoratorAction::Save,
        DecoratorAction::Custom(99),
    ];

    for expected in variants {
        let hit = HitResult {
            annotation_id: AnnotationId(1),
            zone: HitZoneKind::Decorator {
                group_id: 0,
                item_path: ItemPath::new(&[0]),
                action: expected,
            },
            distance: 0.0,
        };
        match super::hit_to_chart_action(&hit) {
            Some(ChartAction::DecoratorClick { action, .. }) => {
                assert_eq!(action, expected, "variant should pass through unchanged");
            }
            other => panic!("expected DecoratorClick for {expected:?}, got {other:?}"),
        }
    }
}

#[test]
fn hit_to_chart_action_returns_none_for_non_decorator_zones() {
    use crate::widget::hit_test::{HitResult, HitZoneKind};
    use crate::widget::AnnotationId;

    // Line and legacy-bracket-button zones are still handled by the
    // per-shape hit-test paths. `hit_to_chart_action` must stay silent
    // so it does not steal clicks from them before Slice 8b.
    let non_decorator = [
        HitZoneKind::LevelLine,
        HitZoneKind::BracketEntry,
        HitZoneKind::BracketTP,
        HitZoneKind::BracketSL,
        HitZoneKind::BracketStopTrigger,
        HitZoneKind::BracketZone,
        HitZoneKind::MarkerIcon,
        HitZoneKind::NoteBody,
        HitZoneKind::VolumeProfileBar,
    ];

    for zone in non_decorator {
        let hit = HitResult {
            annotation_id: AnnotationId(1),
            zone,
            distance: 0.0,
        };
        assert!(
            super::hit_to_chart_action(&hit).is_none(),
            "zone {zone:?} must not lower to a decorator click"
        );
    }
}

#[test]
fn apply_drag_bracket_leg_marks_dirty() {
    let mut state = test_state();
    let gen_before = state.dirty.candles;

    state.apply_action(&ChartAction::DragBracketLeg {
        annotation_id: AnnotationId(1),
        leg: LegRole::TakeProfit,
        new_price: 195.0,
    });

    assert!(
        state.dirty.candles > gen_before,
        "DragBracketLeg should mark data dirty"
    );
}

// ── Slice 8b: decorator-click routing regression suite ──────────

/// Clone of `test_bracket_annotation` that produces a Draft Long bracket.
/// Draft is required for `Submit` / `Save` / `RemoveStopLoss` decorator
/// buttons to emit.
fn draft_bracket_annotation(id: u64, entry: f64, tp: f64, sl: f64) -> Annotation {
    Annotation {
        id: AnnotationId(id),
        kind: AnnotationKind::OrderBracket(Box::new(OrderBracket {
            entry: make_bracket_leg(entry),
            take_profit: Some(make_bracket_leg(tp)),
            stop_loss: Some(make_bracket_leg(sl)),
            side: BracketSide::Long,
            status: BracketStatus::Draft,
            quantity: Some(100.0),
            saved: false,
            filled_qty: None,
            entry_type: EntryType::Market,
            entry_stop_price: None,
            wrong_side_warning: false,
        })),
        presence: Presence::Active,
        visible_timeframes: None,
        locked: false,
        created_at: 0,
        modified_at: 0,
    }
}

/// Camera sized 1920x1080 over a $30 price range so the three test
/// legs of `draft_bracket_annotation` land on well-separated rows.
fn decorator_test_camera() -> Camera2D {
    Camera2D {
        viewport_width: 1920,
        viewport_height: 1080,
        time_start: 0.0,
        time_end: 100_000.0,
        price_low: 170.0,
        price_high: 200.0,
        dpi_scale: 1.0,
    }
}

/// Run a mouse press + release at `(x, y)` through the interaction
/// state machine and return the collected actions.
fn click_at(
    state: &mut ChartState,
    x: f32,
    y: f32,
    annotations: &[Annotation],
) -> Vec<ChartAction> {
    let _ = handle_event(
        state,
        ChartEvent::MousePressed {
            x,
            y,
            button: MouseButton::Left,
            alt_held: false,
        },
        None,
        false,
        annotations,
    );
    handle_event(
        state,
        ChartEvent::MouseReleased {
            x,
            y,
            button: MouseButton::Left,
            alt_held: false,
        },
        None,
        false,
        annotations,
    )
}

/// Locate the hit-rect center of the first decorator hit zone whose
/// action equals `target` within the given annotation's groups. Uses
/// the same compute path the click router uses so the test clicks on
/// exactly the same pixel the runtime does.
fn decorator_button_center(
    ann: &Annotation,
    target: crate::widget::decorator::DecoratorAction,
    camera: &Camera2D,
) -> (f32, f32) {
    use crate::widget::compute::{ComputeContext, Viewport};
    use crate::widget::decorator::compute_decorator_group;
    use crate::widget::order_bracket::decorators::{
        entry_decorator_group, sl_decorator_group, tp_decorator_group,
    };
    use crate::widget::theme::Theme;

    let theme = Theme::default();
    let data = super::EmptyCandleData;
    let hovered_groups: [(AnnotationId, u16); 3] = [(ann.id, 0), (ann.id, 1), (ann.id, 2)];
    let ctx = ComputeContext {
        camera,
        data: &data,
        viewport: Viewport {
            width: camera.viewport_width,
            height: camera.viewport_height,
        },
        theme: &theme,
        snap_fn: &|_| None,
        candle_duration_ms: 0.0,
        collapse_gaps: false,
        separator_y: camera.viewport_height as f32,
        dpi_scale: 1.0,
        hovered_annotation: Some((ann.id, HitZoneKind::LevelLine)),
        hovered_decorator_groups: &hovered_groups,
        selected_annotation: None,
        drag_ghost: None,
        pinned: false,
    };

    let bracket = match &ann.kind {
        AnnotationKind::OrderBracket(b) => b.as_ref(),
        _ => panic!("draft_bracket_annotation must be an OrderBracket"),
    };

    let mut outputs = Vec::new();
    outputs.push(compute_decorator_group(
        &entry_decorator_group(bracket),
        &bracket.entry.line,
        ann.id,
        &ctx,
        1.0,
    ));
    if let Some(tp) = bracket.take_profit.as_ref() {
        if let Some(g) = tp_decorator_group(bracket) {
            outputs.push(compute_decorator_group(&g, &tp.line, ann.id, &ctx, 1.0));
        }
    }
    if let Some(sl) = bracket.stop_loss.as_ref() {
        if let Some(g) = sl_decorator_group(bracket) {
            outputs.push(compute_decorator_group(&g, &sl.line, ann.id, &ctx, 1.0));
        }
    }

    for out in outputs {
        for z in out.hit_zones {
            if let HitZoneKind::Decorator { action, .. } = z.kind {
                if action == target {
                    let cx = (z.rect[0] + z.rect[2]) / 2.0;
                    let cy = (z.rect[1] + z.rect[3]) / 2.0;
                    return (cx, cy);
                }
            }
        }
    }
    panic!("no decorator hit zone found for action {target:?}");
}

fn assert_decorator_click_routes(
    target: crate::widget::decorator::DecoratorAction,
    annotations: &[Annotation],
) {
    let camera = decorator_test_camera();
    let mut state = ChartState::new(camera.clone());
    let (cx, cy) = decorator_button_center(&annotations[0], target, &camera);
    let actions = click_at(&mut state, cx, cy, annotations);
    let hit = actions.iter().find_map(|a| match a {
        ChartAction::DecoratorClick { action, .. } => Some(*action),
        _ => None,
    });
    assert_eq!(
        hit,
        Some(target),
        "click at ({cx},{cy}) should route through DecoratorClick with action {target:?}, got {actions:?}"
    );
}

#[test]
fn bracket_submit_click_routes_through_decorator_click() {
    let annotations = vec![draft_bracket_annotation(10, 185.0, 192.0, 182.0)];
    assert_decorator_click_routes(
        crate::widget::decorator::DecoratorAction::Submit,
        &annotations,
    );
}

#[test]
fn bracket_save_click_routes_through_decorator_click() {
    let annotations = vec![draft_bracket_annotation(10, 185.0, 192.0, 182.0)];
    assert_decorator_click_routes(
        crate::widget::decorator::DecoratorAction::Save,
        &annotations,
    );
}

#[test]
fn bracket_cancel_click_routes_through_decorator_click() {
    let annotations = vec![draft_bracket_annotation(10, 185.0, 192.0, 182.0)];
    assert_decorator_click_routes(
        crate::widget::decorator::DecoratorAction::CloseAnnotation,
        &annotations,
    );
}

#[test]
fn bracket_cancel_sl_click_routes_through_decorator_click() {
    let annotations = vec![draft_bracket_annotation(10, 185.0, 192.0, 182.0)];
    assert_decorator_click_routes(
        crate::widget::decorator::DecoratorAction::RemoveStopLoss,
        &annotations,
    );
}

#[test]
fn bracket_toggle_sl_click_routes_through_decorator_click() {
    // Draft bracket with no SL — the entry decorator group still
    // emits the hover-only `CreateStopLoss` stack button (successor
    // to the legacy `[SL]` toggle), which is what this test clicks.
    let mut ann = draft_bracket_annotation(10, 185.0, 192.0, 182.0);
    if let AnnotationKind::OrderBracket(ref mut b) = ann.kind {
        b.stop_loss = None;
    }
    assert_decorator_click_routes(
        crate::widget::decorator::DecoratorAction::CreateStopLoss,
        &[ann],
    );
}

// ── Fix 3: hit-test priority for overlapping bracket legs ────────────

/// Build a Draft Limit bracket where every leg can be positioned by
/// argument. Used by the overlap tie-break tests.
fn overlap_draft_bracket(
    id: u64,
    entry: f64,
    tp: f64,
    sl: f64,
    entry_type: crate::widget::order_bracket::EntryType,
    entry_stop_price: Option<f64>,
) -> Annotation {
    use crate::widget::order_bracket::{
        BracketLeg, BracketSide, BracketStatus, LegRole, OrderBracket,
    };

    let make_leg = |price: f64, role: LegRole| BracketLeg {
        line: PriceLine {
            price,
            extent: LineExtent::FullWidth,
            stroke: LineStroke {
                color: [1.0, 1.0, 1.0, 1.0],
                width: 1.0,
                style: LineStyle::Solid,
            },
        },
        role,
        projected_pnl: None,
        projected_pnl_pct: None,
    };

    Annotation {
        id: AnnotationId(id),
        kind: AnnotationKind::OrderBracket(Box::new(OrderBracket {
            entry: make_leg(entry, LegRole::Entry),
            take_profit: Some(make_leg(tp, LegRole::TakeProfit)),
            stop_loss: Some(make_leg(sl, LegRole::StopLoss)),
            side: BracketSide::Long,
            status: BracketStatus::Draft,
            quantity: Some(100.0),
            saved: false,
            filled_qty: None,
            entry_type,
            entry_stop_price,
            wrong_side_warning: false,
        })),
        presence: Presence::Active,
        visible_timeframes: None,
        locked: false,
        created_at: 0,
        modified_at: 0,
    }
}

/// When Entry and SL collapse to the same price, the hit-test should
/// pick the Entry leg (the higher-priority drag target).
#[test]
fn hit_test_picks_entry_when_entry_and_sl_overlap_in_price() {
    use crate::widget::order_bracket::EntryType;
    let state = test_state();
    // Entry == SL == 150.0; TP separated above.
    let bracket = overlap_draft_bracket(10, 150.0, 170.0, 150.0, EntryType::Limit, None);
    let y = state.camera.price_to_y(150.0);
    let result = hit_test_annotation(&[bracket], y, &state.camera);
    let (_, kind, _, _) = result.expect("should hit one leg");
    assert_eq!(
        kind,
        HitZoneKind::BracketEntry,
        "Entry should win the tie-break against SL"
    );
}

/// Degenerate case: all four legs at the same price. Entry wins.
#[test]
fn hit_test_picks_entry_when_all_four_legs_collapse() {
    use crate::widget::order_bracket::EntryType;
    let state = test_state();
    let bracket = overlap_draft_bracket(
        10,
        150.0,
        150.0,
        150.0,
        EntryType::StopLimit,
        Some(150.0),
    );
    let y = state.camera.price_to_y(150.0);
    let result = hit_test_annotation(&[bracket], y, &state.camera);
    let (_, kind, _, _) = result.expect("should hit one leg");
    assert_eq!(kind, HitZoneKind::BracketEntry);
}

/// Stop trigger and TP overlap → StopTrigger wins (priority 3 > 2).
#[test]
fn hit_test_picks_stop_trigger_over_take_profit_when_they_overlap() {
    use crate::widget::order_bracket::EntryType;
    let state = test_state();
    // Entry far away at 130, stop trigger == TP == 170, SL at 120.
    let bracket = overlap_draft_bracket(
        10,
        130.0,
        170.0,
        120.0,
        EntryType::StopLimit,
        Some(170.0),
    );
    let y = state.camera.price_to_y(170.0);
    let result = hit_test_annotation(&[bracket], y, &state.camera);
    let (_, kind, _, _) = result.expect("should hit one leg");
    assert_eq!(kind, HitZoneKind::BracketStopTrigger);
}

/// When the cursor clicks in the shared region of two overlapping
/// zones, the returned result must be a single deterministic leg.
#[test]
fn drag_start_with_two_overlapping_zones_picks_only_one() {
    use crate::widget::order_bracket::EntryType;
    let state = test_state();
    // Entry and TP collapsed at 160. Entry wins (priority 4 > 2).
    let bracket = overlap_draft_bracket(10, 160.0, 160.0, 130.0, EntryType::Limit, None);
    let y = state.camera.price_to_y(160.0);
    let result = hit_test_annotation(&[bracket], y, &state.camera);
    assert!(result.is_some(), "shared click should still hit something");
    let (id, kind, _, _) = result.unwrap();
    assert_eq!(id, AnnotationId(10));
    assert_eq!(
        kind,
        HitZoneKind::BracketEntry,
        "tie must resolve deterministically to Entry"
    );
}
