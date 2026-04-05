use super::*;

#[test]
fn new_starts_hidden() {
    let tool = CrosshairTool::new();
    assert_eq!(*tool.mode(), CrosshairMode::Hidden);
    assert_eq!(tool.render_pos(), None);
    assert!(!tool.should_render());
    assert!(!tool.left_mouse_down());
}

#[test]
fn on_left_press_transitions_hidden_to_tracking() {
    let mut tool = CrosshairTool::new();
    let became_visible = tool.on_left_press(100.0, 200.0);
    assert!(became_visible);
    assert_eq!(*tool.mode(), CrosshairMode::Tracking);
    assert_eq!(tool.render_pos(), Some((100.0, 200.0)));
    assert!(tool.left_mouse_down());
}

#[test]
fn on_left_release_transitions_tracking_to_hidden() {
    let mut tool = CrosshairTool::new();
    tool.on_left_press(100.0, 200.0);
    let became_hidden = tool.on_left_release();
    assert!(became_hidden);
    assert_eq!(*tool.mode(), CrosshairMode::Hidden);
    assert!(!tool.should_render());
    assert!(!tool.left_mouse_down());
}

#[test]
fn on_mouse_move_updates_position_when_tracking() {
    let mut tool = CrosshairTool::new();
    tool.on_left_press(100.0, 200.0);
    tool.on_mouse_move(150.0, 250.0, true);
    assert_eq!(tool.render_pos(), Some((150.0, 250.0)));
}

#[test]
fn on_mouse_move_out_of_bounds_hides_in_tracking() {
    let mut tool = CrosshairTool::new();
    tool.on_left_press(100.0, 200.0);
    tool.on_mouse_move(-10.0, 200.0, false);
    assert_eq!(*tool.mode(), CrosshairMode::Tracking);
    assert_eq!(tool.render_pos(), None);
    // Re-entering bounds restores visibility without another press.
    tool.on_mouse_move(100.0, 200.0, true);
    assert_eq!(tool.render_pos(), Some((100.0, 200.0)));
}

#[test]
fn on_mouse_move_out_of_bounds_does_not_hide_in_preview() {
    let mut tool = CrosshairTool::new();
    tool.enter_preview(100.0, 200.0);
    tool.on_mouse_move(-10.0, 200.0, false);
    assert_eq!(*tool.mode(), CrosshairMode::Preview);
    // Preview always updates position, even out-of-bounds.
    assert_eq!(tool.render_pos(), Some((-10.0, 200.0)));
}

#[test]
fn enter_preview_from_hidden() {
    let mut tool = CrosshairTool::new();
    tool.enter_preview(300.0, 400.0);
    assert_eq!(*tool.mode(), CrosshairMode::Preview);
    assert_eq!(tool.render_pos(), Some((300.0, 400.0)));
    assert!(tool.is_preview());
}

#[test]
fn enter_preview_from_tracking() {
    let mut tool = CrosshairTool::new();
    tool.on_left_press(100.0, 200.0);
    tool.enter_preview(300.0, 400.0);
    assert_eq!(*tool.mode(), CrosshairMode::Preview);
    assert_eq!(tool.render_pos(), Some((300.0, 400.0)));
}

#[test]
fn exit_preview_to_hidden_when_no_mouse() {
    let mut tool = CrosshairTool::new();
    tool.enter_preview(100.0, 200.0);
    tool.exit_preview();
    assert_eq!(*tool.mode(), CrosshairMode::Hidden);
    assert!(!tool.should_render());
}

#[test]
fn exit_preview_to_tracking_when_mouse_held() {
    let mut tool = CrosshairTool::new();
    tool.on_left_press(100.0, 200.0);
    tool.enter_preview(300.0, 400.0);
    tool.exit_preview();
    assert_eq!(*tool.mode(), CrosshairMode::Tracking);
    assert!(tool.should_render());
}

#[test]
fn on_left_release_does_not_exit_preview() {
    let mut tool = CrosshairTool::new();
    tool.enter_preview(100.0, 200.0);
    let became_hidden = tool.on_left_release();
    assert!(!became_hidden);
    assert_eq!(*tool.mode(), CrosshairMode::Preview);
    assert!(tool.should_render());
}

#[test]
fn on_left_press_does_not_override_preview() {
    let mut tool = CrosshairTool::new();
    tool.enter_preview(100.0, 200.0);
    let became_visible = tool.on_left_press(300.0, 400.0);
    // Was already visible in Preview, so did not "become visible".
    assert!(!became_visible);
    assert_eq!(*tool.mode(), CrosshairMode::Preview);
    // Position updated though.
    assert_eq!(tool.render_pos(), Some((300.0, 400.0)));
}

#[test]
fn force_hide_from_any_mode() {
    for initial_mode in [
        CrosshairMode::Hidden,
        CrosshairMode::Tracking,
        CrosshairMode::Preview,
    ] {
        let mut tool = CrosshairTool::new();
        match initial_mode {
            CrosshairMode::Hidden => {}
            CrosshairMode::Tracking => {
                tool.on_left_press(100.0, 200.0);
            }
            CrosshairMode::Preview => {
                tool.enter_preview(100.0, 200.0);
            }
        }
        tool.force_hide();
        assert_eq!(*tool.mode(), CrosshairMode::Hidden);
        assert_eq!(tool.render_pos(), None);
        assert!(!tool.left_mouse_down());
    }
}

#[test]
fn render_pos_returns_none_when_hidden() {
    let tool = CrosshairTool::new();
    assert_eq!(tool.render_pos(), None);
}

#[test]
fn render_pos_returns_position_when_tracking() {
    let mut tool = CrosshairTool::new();
    tool.on_left_press(100.0, 200.0);
    assert_eq!(tool.render_pos(), Some((100.0, 200.0)));
}

#[test]
fn render_pos_returns_position_when_preview() {
    let mut tool = CrosshairTool::new();
    tool.enter_preview(100.0, 200.0);
    assert_eq!(tool.render_pos(), Some((100.0, 200.0)));
}

#[test]
fn resume_preview_from_stored_position() {
    let mut tool = CrosshairTool::new();
    tool.enter_preview(100.0, 200.0);
    // Simulate suspend: go to Hidden.
    tool.mode = CrosshairMode::Hidden;
    tool.resume_preview();
    assert_eq!(*tool.mode(), CrosshairMode::Preview);
    assert_eq!(tool.render_pos(), Some((100.0, 200.0)));
}

#[test]
fn resume_preview_noop_when_no_position() {
    let mut tool = CrosshairTool::new();
    tool.resume_preview();
    assert_eq!(*tool.mode(), CrosshairMode::Hidden);
}

#[test]
fn set_pos_makes_visible() {
    let mut tool = CrosshairTool::new();
    tool.set_pos(100.0, 200.0);
    assert_eq!(*tool.mode(), CrosshairMode::Tracking);
    assert_eq!(tool.render_pos(), Some((100.0, 200.0)));
}

#[test]
fn set_pos_does_not_change_preview_mode() {
    let mut tool = CrosshairTool::new();
    tool.enter_preview(100.0, 200.0);
    tool.set_pos(300.0, 400.0);
    assert_eq!(*tool.mode(), CrosshairMode::Preview);
    assert_eq!(tool.render_pos(), Some((300.0, 400.0)));
}

#[test]
fn default_matches_new() {
    let tool = CrosshairTool::default();
    assert_eq!(*tool.mode(), CrosshairMode::Hidden);
    assert!(!tool.left_mouse_down());
}

#[test]
fn tracking_to_preview_to_tracking() {
    let mut tool = CrosshairTool::new();
    tool.on_left_press(100.0, 200.0);
    assert_eq!(*tool.mode(), CrosshairMode::Tracking);
    tool.enter_preview(300.0, 400.0);
    assert_eq!(*tool.mode(), CrosshairMode::Preview);
    tool.exit_preview();
    // Left mouse still held -> Tracking.
    assert_eq!(*tool.mode(), CrosshairMode::Tracking);
    assert!(tool.should_render());
}

#[test]
fn suppress_hides_but_preserves_left_mouse_down() {
    let mut tool = CrosshairTool::new();
    tool.on_left_press(100.0, 200.0);
    assert!(tool.left_mouse_down());
    assert!(tool.should_render());

    tool.suppress();
    assert_eq!(*tool.mode(), CrosshairMode::Hidden);
    assert_eq!(tool.render_pos(), None);
    // left_mouse_down is preserved -- the button is still physically held.
    assert!(tool.left_mouse_down());

    // on_left_release correctly transitions from the suppressed state.
    let became_hidden = tool.on_left_release();
    assert!(!became_hidden); // was already hidden
    assert!(!tool.left_mouse_down());
}
