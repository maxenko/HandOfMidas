//! Dirty flags and dirty tracker using generation counters.
//!
//! Uses generation counters (u64) instead of booleans to solve the
//! "who clears the flag" problem. The writer increments the counter;
//! the reader ([`DirtyTracker`]) remembers the last-seen generation
//! and compares.

/// Canonical dirty flags for a single chart.
///
/// Each field is a generation counter (u64) that is incremented when
/// the corresponding aspect of the chart changes. Readers compare
/// their last-seen generation against the current generation to
/// determine if work needs to be done.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DirtyFlags {
    /// Viewport / zoom / pan changed.
    pub camera: u64,
    /// Candle data changed (new data, LOD change).
    pub candles: u64,
    /// Indicator output changed.
    pub indicators: u64,
    /// Crosshair position changed.
    pub crosshair: u64,
    /// Horizontal levels changed.
    pub levels: u64,
    /// Grid needs recalc (zoom changed grid density).
    pub grid: u64,
    /// Theme / colors changed.
    pub theme: u64,
}

impl DirtyFlags {
    /// Create a new `DirtyFlags` with all counters at zero.
    pub fn new() -> Self {
        Self::default()
    }

    /// Camera moved (pan/zoom/resize). Also invalidates grid
    /// (grid density depends on zoom level) and candle instances
    /// (pixel positions and widths depend on the camera time range).
    pub fn mark_camera(&mut self) {
        self.camera += 1;
        self.candles += 1;
        self.grid += 1;
    }

    /// Candle data changed (new data loaded, symbol change, LOD change).
    /// Also invalidates indicators (they depend on candle data).
    pub fn mark_data(&mut self) {
        self.candles += 1;
        self.indicators += 1;
    }

    /// Indicator output changed (indicator added/removed/recalculated).
    pub fn mark_indicators(&mut self) {
        self.indicators += 1;
    }

    /// Crosshair position changed (mouse moved).
    pub fn mark_crosshair(&mut self) {
        self.crosshair += 1;
    }

    /// Horizontal levels changed (added/moved/deleted).
    pub fn mark_levels(&mut self) {
        self.levels += 1;
    }

    /// Theme/colors changed. Requires full instance rebuild since
    /// colors are baked into instance data.
    pub fn mark_theme(&mut self) {
        self.theme += 1;
        // Theme change invalidates all instance data (colors are baked in).
        self.candles += 1;
        self.indicators += 1;
        self.levels += 1;
        self.grid += 1;
    }

    /// Mark everything as dirty. Useful for initial setup or full reset.
    pub fn mark_all(&mut self) {
        self.camera += 1;
        self.candles += 1;
        self.indicators += 1;
        self.crosshair += 1;
        self.levels += 1;
        self.grid += 1;
        self.theme += 1;
    }
}

/// Tracks which generation counters a consumer has last processed.
///
/// Each GPU consumer (or any reader) owns a `DirtyTracker` and compares
/// its last-seen generations against the current [`DirtyFlags`]. After
/// processing all updates, call [`acknowledge`](DirtyTracker::acknowledge)
/// to record the current generations.
#[derive(Clone, Debug, Default)]
pub struct DirtyTracker {
    last_seen: DirtyFlags,
}

impl DirtyTracker {
    /// Create a new tracker with all last-seen counters at zero.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `true` if the camera generation has changed since last acknowledgment.
    pub fn needs_camera_update(&self, current: &DirtyFlags) -> bool {
        self.last_seen.camera != current.camera
    }

    /// Returns `true` if the candle data generation has changed since last acknowledgment.
    pub fn needs_candle_rebuild(&self, current: &DirtyFlags) -> bool {
        self.last_seen.candles != current.candles
    }

    /// Returns `true` if the indicator generation has changed since last acknowledgment.
    pub fn needs_indicator_rebuild(&self, current: &DirtyFlags) -> bool {
        self.last_seen.indicators != current.indicators
    }

    /// Returns `true` if the crosshair generation has changed since last acknowledgment.
    pub fn needs_crosshair_update(&self, current: &DirtyFlags) -> bool {
        self.last_seen.crosshair != current.crosshair
    }

    /// Returns `true` if the levels generation has changed since last acknowledgment.
    pub fn needs_level_rebuild(&self, current: &DirtyFlags) -> bool {
        self.last_seen.levels != current.levels
    }

    /// Returns `true` if the grid generation has changed since last acknowledgment.
    pub fn needs_grid_rebuild(&self, current: &DirtyFlags) -> bool {
        self.last_seen.grid != current.grid
    }

    /// Returns `true` if the theme generation has changed since last acknowledgment.
    pub fn needs_theme_rebuild(&self, current: &DirtyFlags) -> bool {
        self.last_seen.theme != current.theme
    }

    /// Returns `true` if ANY counter has changed since last acknowledgment.
    pub fn any_dirty(&self, current: &DirtyFlags) -> bool {
        self.needs_camera_update(current)
            || self.needs_candle_rebuild(current)
            || self.needs_indicator_rebuild(current)
            || self.needs_crosshair_update(current)
            || self.needs_level_rebuild(current)
            || self.needs_grid_rebuild(current)
            || self.needs_theme_rebuild(current)
    }

    /// Record that we have processed all current generations.
    ///
    /// Call this at the end of `Primitive::prepare()` after all
    /// GPU uploads are done.
    pub fn acknowledge(&mut self, current: &DirtyFlags) {
        self.last_seen = current.clone();
    }
}

#[cfg(test)]
mod tests {
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
}
