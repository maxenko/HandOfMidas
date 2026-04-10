use super::*;
use crate::camera::Camera2D;
use crate::state::{ChartState, InteractionMode, Momentum, YAnimation};
use crate::widget::{
    hit_test::HitZoneKind,
    level::{LevelExtend, LineStyle},
    Annotation, AnnotationId, AnnotationKind, Presence,
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
            price,
            color: [1.0, 0.0, 0.0, 1.0],
            line_width: 1.0,
            style: LineStyle::default(),
            label: None,
            extend: LevelExtend::default(),
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
    BracketLeg {
        price,
        timestamp: None,
        color: None,
        style: crate::widget::level::LineStyle::default(),
        line_width: 1.0,
        label: None,
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
    let clamped =
        clamp_bracket_leg_price(175.0, 185.0, LegRole::StopLoss, BracketSide::Long, &cam);
    assert!((clamped - 175.0).abs() < f64::EPSILON);
}

#[test]
fn test_clamp_bracket_leg_long_sl_above_entry_clamped() {
    let cam = clamp_test_camera();
    let clamped =
        clamp_bracket_leg_price(190.0, 185.0, LegRole::StopLoss, BracketSide::Long, &cam);
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
        assert_ne!(kind, HitZoneKind::BracketEntry, "Market entry should not be draggable");
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
        assert_ne!(kind, HitZoneKind::BracketEntry, "Draft Market entry should not be draggable");
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
        assert_ne!(kind, HitZoneKind::BracketEntry, "Pending Limit entry should not be draggable");
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
    assert!(result.is_some(), "Draft StopLimit entry should be hit-testable");
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
