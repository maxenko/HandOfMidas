//! Self-contained crosshair component.
//!
//! Owns all crosshair state and provides a clean API for show/hide/update.
//! Lives as a field on [`ChartState`](crate::state::ChartState). The
//! interaction layer delegates crosshair-related event handling to this
//! struct instead of manipulating raw fields.

// ── Types ─────────────────────────────────────────────────────────

/// The crosshair's operational mode.
#[derive(Clone, Debug, PartialEq)]
pub enum CrosshairMode {
    /// Crosshair is hidden (default state, no mouse button held).
    Hidden,
    /// Crosshair tracks the cursor while left mouse is held.
    /// This is the standard charting crosshair behavior.
    Tracking,
    /// Crosshair provides a preview line for an external tool
    /// (e.g., level placement). Visible regardless of mouse button.
    /// The tool is responsible for entering/exiting this mode.
    Preview,
}

/// Self-contained crosshair component.
///
/// Owns all crosshair state and provides a clean API for show/hide/update.
/// Lives as a field on `ChartState`.
#[derive(Clone, Debug, PartialEq)]
pub struct CrosshairTool {
    /// Current operational mode.
    mode: CrosshairMode,
    /// Cursor position in chart-local pixels (always tracked, even when hidden).
    cursor_pos: Option<(f32, f32)>,
    /// Whether the left mouse button is currently held.
    left_mouse_down: bool,
}

impl Default for CrosshairTool {
    fn default() -> Self {
        Self::new()
    }
}

impl CrosshairTool {
    /// Create a new CrosshairTool in Hidden mode.
    pub fn new() -> Self {
        Self {
            mode: CrosshairMode::Hidden,
            cursor_pos: None,
            left_mouse_down: false,
        }
    }

    /// Current mode.
    pub fn mode(&self) -> &CrosshairMode {
        &self.mode
    }

    /// The cursor position that should be used for rendering,
    /// or `None` if the crosshair should not be rendered.
    /// This is the single source of truth for crosshair visibility.
    pub fn render_pos(&self) -> Option<(f32, f32)> {
        match self.mode {
            CrosshairMode::Hidden => None,
            CrosshairMode::Tracking | CrosshairMode::Preview => self.cursor_pos,
        }
    }

    /// Whether the crosshair should be rendered this frame.
    /// Equivalent to `self.render_pos().is_some()`.
    pub fn should_render(&self) -> bool {
        self.render_pos().is_some()
    }

    // ── Mutation methods (called by interaction layer) ──

    /// Record that the left mouse button was pressed at (x, y).
    /// Transitions from Hidden to Tracking if not in Preview mode.
    /// Returns true if the crosshair became visible.
    pub fn on_left_press(&mut self, x: f32, y: f32) -> bool {
        self.left_mouse_down = true;
        self.cursor_pos = Some((x, y));
        let was_visible = self.should_render();
        if self.mode == CrosshairMode::Hidden {
            self.mode = CrosshairMode::Tracking;
        }
        !was_visible && self.should_render()
    }

    /// Record that the left mouse button was released.
    /// Transitions from Tracking to Hidden.
    /// Returns true if the crosshair became hidden.
    pub fn on_left_release(&mut self) -> bool {
        self.left_mouse_down = false;
        let was_visible = self.should_render();
        if self.mode == CrosshairMode::Tracking {
            self.mode = CrosshairMode::Hidden;
        }
        was_visible && !self.should_render()
    }

    /// Update cursor position. Called on every mouse move.
    /// Only updates the visible position if the crosshair is active.
    /// `in_bounds` indicates whether the cursor is within the chart area.
    pub fn on_mouse_move(&mut self, x: f32, y: f32, in_bounds: bool) {
        match self.mode {
            CrosshairMode::Tracking => {
                if in_bounds {
                    self.cursor_pos = Some((x, y));
                } else {
                    self.cursor_pos = None;
                }
            }
            CrosshairMode::Preview => {
                // In Preview mode, always update position (ignore in_bounds).
                self.cursor_pos = Some((x, y));
            }
            CrosshairMode::Hidden => {
                // Store position even when hidden so resume_preview works.
                if in_bounds {
                    self.cursor_pos = Some((x, y));
                }
            }
        }
    }

    /// Enter preview mode (for level tool). Crosshair becomes visible
    /// at the given position regardless of mouse button state.
    pub fn enter_preview(&mut self, x: f32, y: f32) {
        self.mode = CrosshairMode::Preview;
        self.cursor_pos = Some((x, y));
    }

    /// Exit preview mode. Returns to Hidden or Tracking based on
    /// whether the left mouse button is held.
    pub fn exit_preview(&mut self) {
        if self.mode == CrosshairMode::Preview {
            if self.left_mouse_down {
                self.mode = CrosshairMode::Tracking;
            } else {
                self.mode = CrosshairMode::Hidden;
            }
        }
    }

    /// Force-hide the crosshair (e.g., Escape key, viewport resize).
    /// Resets to Hidden mode and clears left_mouse_down.
    pub fn force_hide(&mut self) {
        self.mode = CrosshairMode::Hidden;
        self.left_mouse_down = false;
        self.cursor_pos = None;
    }

    /// Hide the crosshair without clearing `left_mouse_down`.
    ///
    /// Used when the crosshair must be hidden while the mouse button is
    /// still physically held (e.g., PendingDrag→LevelTool::Dragging
    /// transition, or during an active level drag). The `left_mouse_down`
    /// state is preserved so `on_left_release()` can correctly transition
    /// when the button is finally released.
    pub fn suppress(&mut self) {
        self.mode = CrosshairMode::Hidden;
        self.cursor_pos = None;
    }

    /// Whether the left mouse button is currently held.
    /// Exposed so interaction.rs can check without a separate field.
    pub fn left_mouse_down(&self) -> bool {
        self.left_mouse_down
    }

    /// Whether we are in Preview mode (for level tool queries).
    pub fn is_preview(&self) -> bool {
        self.mode == CrosshairMode::Preview
    }

    /// Re-enter preview mode using the last known cursor position.
    /// Called after `level_tool.try_resume_placing()` restores placing
    /// mode — the crosshair should return to Preview at the stored
    /// position without the caller needing to track coordinates.
    /// No-op if `cursor_pos` is `None`.
    pub fn resume_preview(&mut self) {
        if self.cursor_pos.is_some() {
            self.mode = CrosshairMode::Preview;
        }
    }

    /// Unconditionally set the crosshair position and make it visible.
    /// Used by `apply_action(SetCrosshair)` for backward compatibility
    /// with external callers that set crosshair via the action system
    /// rather than through `on_left_press` / `on_mouse_move`.
    /// Transitions to Tracking if currently Hidden.
    pub fn set_pos(&mut self, x: f32, y: f32) {
        self.cursor_pos = Some((x, y));
        if self.mode == CrosshairMode::Hidden {
            self.mode = CrosshairMode::Tracking;
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
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
        for initial_mode in [CrosshairMode::Hidden, CrosshairMode::Tracking, CrosshairMode::Preview] {
            let mut tool = CrosshairTool::new();
            match initial_mode {
                CrosshairMode::Hidden => {}
                CrosshairMode::Tracking => { tool.on_left_press(100.0, 200.0); }
                CrosshairMode::Preview => { tool.enter_preview(100.0, 200.0); }
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
        // Left mouse still held → Tracking.
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
        // left_mouse_down is preserved — the button is still physically held.
        assert!(tool.left_mouse_down());

        // on_left_release correctly transitions from the suppressed state.
        let became_hidden = tool.on_left_release();
        assert!(!became_hidden); // was already hidden
        assert!(!tool.left_mouse_down());
    }
}
