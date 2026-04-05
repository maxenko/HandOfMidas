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
    assert_eq!(state.crosshair_pos, None);
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
fn apply_select_and_deselect_level() {
    let mut state = ChartState::new(test_camera());
    state.apply_action(&ChartAction::SelectLevel {
        id: AnnotationId(1),
    });
    assert_eq!(state.selected_level, Some(AnnotationId(1)));
    state.apply_action(&ChartAction::DeselectLevel);
    assert_eq!(state.selected_level, None);
}

#[test]
fn apply_set_and_clear_crosshair() {
    let mut state = ChartState::new(test_camera());
    state.apply_action(&ChartAction::SetCrosshair { x: 100.0, y: 200.0 });
    assert_eq!(state.crosshair_pos, Some((100.0, 200.0)));

    state.apply_action(&ChartAction::ClearCrosshair);
    assert_eq!(state.crosshair_pos, None);
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
