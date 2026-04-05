use super::*;

#[test]
fn default_flags_are_zero() {
    let flags = DirtyFlags::default();
    assert_eq!(flags.camera, 0);
    assert_eq!(flags.candles, 0);
    assert_eq!(flags.indicators, 0);
    assert_eq!(flags.crosshair, 0);
    assert_eq!(flags.levels, 0);
    assert_eq!(flags.grid, 0);
    assert_eq!(flags.theme, 0);
}

#[test]
fn mark_camera_increments_camera_candles_and_grid() {
    let mut flags = DirtyFlags::new();
    flags.mark_camera();
    assert_eq!(flags.camera, 1);
    assert_eq!(flags.candles, 1);
    assert_eq!(flags.grid, 1);
    // Others unchanged.
    assert_eq!(flags.indicators, 0);
    assert_eq!(flags.crosshair, 0);
    assert_eq!(flags.levels, 0);
    assert_eq!(flags.theme, 0);
}

#[test]
fn mark_data_increments_candles_and_indicators() {
    let mut flags = DirtyFlags::new();
    flags.mark_data();
    assert_eq!(flags.candles, 1);
    assert_eq!(flags.indicators, 1);
    // Others unchanged.
    assert_eq!(flags.camera, 0);
    assert_eq!(flags.crosshair, 0);
    assert_eq!(flags.levels, 0);
    assert_eq!(flags.grid, 0);
    assert_eq!(flags.theme, 0);
}

#[test]
fn mark_theme_cascades_to_candles_indicators_levels_grid() {
    let mut flags = DirtyFlags::new();
    flags.mark_theme();
    assert_eq!(flags.theme, 1);
    assert_eq!(flags.candles, 1);
    assert_eq!(flags.indicators, 1);
    assert_eq!(flags.levels, 1);
    assert_eq!(flags.grid, 1);
    // Camera and crosshair not affected.
    assert_eq!(flags.camera, 0);
    assert_eq!(flags.crosshair, 0);
}

#[test]
fn mark_all_increments_everything() {
    let mut flags = DirtyFlags::new();
    flags.mark_all();
    assert_eq!(flags.camera, 1);
    assert_eq!(flags.candles, 1);
    assert_eq!(flags.indicators, 1);
    assert_eq!(flags.crosshair, 1);
    assert_eq!(flags.levels, 1);
    assert_eq!(flags.grid, 1);
    assert_eq!(flags.theme, 1);
}

#[test]
fn mark_crosshair_only_increments_crosshair() {
    let mut flags = DirtyFlags::new();
    flags.mark_crosshair();
    assert_eq!(flags.crosshair, 1);
    assert_eq!(flags.camera, 0);
    assert_eq!(flags.candles, 0);
}

#[test]
fn mark_levels_only_increments_levels() {
    let mut flags = DirtyFlags::new();
    flags.mark_levels();
    assert_eq!(flags.levels, 1);
    assert_eq!(flags.camera, 0);
    assert_eq!(flags.candles, 0);
}

#[test]
fn mark_indicators_only_increments_indicators() {
    let mut flags = DirtyFlags::new();
    flags.mark_indicators();
    assert_eq!(flags.indicators, 1);
    assert_eq!(flags.candles, 0);
}

#[test]
fn multiple_marks_accumulate() {
    let mut flags = DirtyFlags::new();
    flags.mark_camera();
    flags.mark_camera();
    flags.mark_camera();
    assert_eq!(flags.camera, 3);
    assert_eq!(flags.candles, 3);
    assert_eq!(flags.grid, 3);
}

#[test]
fn tracker_starts_clean() {
    let flags = DirtyFlags::new();
    let tracker = DirtyTracker::new();
    assert!(!tracker.any_dirty(&flags));
}

#[test]
fn tracker_detects_camera_change() {
    let mut flags = DirtyFlags::new();
    let tracker = DirtyTracker::new();

    flags.mark_camera();
    assert!(tracker.needs_camera_update(&flags));
    assert!(tracker.needs_candle_rebuild(&flags));
    assert!(tracker.needs_grid_rebuild(&flags));
    assert!(tracker.any_dirty(&flags));
}

#[test]
fn tracker_detects_data_change() {
    let mut flags = DirtyFlags::new();
    let tracker = DirtyTracker::new();

    flags.mark_data();
    assert!(tracker.needs_candle_rebuild(&flags));
    assert!(tracker.needs_indicator_rebuild(&flags));
    assert!(tracker.any_dirty(&flags));
}

#[test]
fn acknowledge_clears_dirty_state() {
    let mut flags = DirtyFlags::new();
    let mut tracker = DirtyTracker::new();

    flags.mark_camera();
    flags.mark_data();
    flags.mark_crosshair();
    assert!(tracker.any_dirty(&flags));

    tracker.acknowledge(&flags);
    assert!(!tracker.any_dirty(&flags));
    assert!(!tracker.needs_camera_update(&flags));
    assert!(!tracker.needs_candle_rebuild(&flags));
    assert!(!tracker.needs_crosshair_update(&flags));
}

#[test]
fn acknowledge_then_new_change_is_detected() {
    let mut flags = DirtyFlags::new();
    let mut tracker = DirtyTracker::new();

    flags.mark_camera();
    tracker.acknowledge(&flags);
    assert!(!tracker.any_dirty(&flags));

    flags.mark_crosshair();
    assert!(tracker.any_dirty(&flags));
    assert!(tracker.needs_crosshair_update(&flags));
    // Camera not changed since last acknowledge.
    assert!(!tracker.needs_candle_rebuild(&flags));
}

#[test]
fn tracker_detects_theme_cascade() {
    let mut flags = DirtyFlags::new();
    let mut tracker = DirtyTracker::new();

    // Acknowledge clean state.
    tracker.acknowledge(&flags);

    flags.mark_theme();
    assert!(tracker.needs_theme_rebuild(&flags));
    assert!(tracker.needs_candle_rebuild(&flags));
    assert!(tracker.needs_indicator_rebuild(&flags));
    assert!(tracker.needs_level_rebuild(&flags));
    assert!(tracker.needs_grid_rebuild(&flags));
    // Camera and crosshair not affected by mark_theme.
    assert!(!tracker.needs_camera_update(&flags));
    assert!(!tracker.needs_crosshair_update(&flags));
}

#[test]
fn partial_acknowledge_not_possible() {
    // acknowledge() always copies all flags, so there is no partial ack.
    let mut flags = DirtyFlags::new();
    let mut tracker = DirtyTracker::new();

    flags.mark_camera();
    flags.mark_data();
    tracker.acknowledge(&flags);

    // Both should now be clean.
    assert!(!tracker.needs_camera_update(&flags));
    assert!(!tracker.needs_candle_rebuild(&flags));
}
