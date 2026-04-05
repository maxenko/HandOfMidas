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
mod tests;
