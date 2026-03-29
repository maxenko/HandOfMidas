//! Self-contained horizontal level tool.
//!
//! Owns all state for level placement, dragging, and OHLC snapping.
//! Lives as a field on [`ChartState`]. The interaction layer delegates
//! level-related event handling to this struct.

use crate::camera::Camera2D;
use midas_core::CandleData;

// ── Constants ─────────────────────────────────────────────────────

/// Minimum pixel distance (Y-axis) for OHLC snap.
const SNAP_THRESHOLD_MIN_PX: f32 = 15.0;
/// Maximum pixel distance (Y-axis) for OHLC snap.
const SNAP_THRESHOLD_MAX_PX: f32 = 40.0;

// ── Types ─────────────────────────────────────────────────────────

/// The level tool's internal state machine.
#[derive(Clone, Debug, PartialEq)]
pub enum LevelToolMode {
    /// Tool is not active. No preview, no drag.
    Idle,
    /// User activated the tool. Preview line follows cursor Y
    /// (snapped to OHLC unless Alt held). Single click places.
    Placing,
    /// User is dragging an existing level to a new price.
    Dragging {
        /// ID of the level being dragged.
        level_id: u64,
        /// Price offset between level and cursor at grab time
        /// (so the level doesn't jump to the cursor).
        grab_offset: f64,
    },
}

/// Self-contained horizontal level tool.
///
/// Owns all state for level placement, dragging, and OHLC snapping.
/// Lives as a field on `ChartState`. The interaction layer delegates
/// level-related event handling to this struct.
#[derive(Clone, Debug)]
pub struct LevelTool {
    /// Current tool mode.
    pub mode: LevelToolMode,
    /// Whether Alt is held (disables OHLC snap).
    pub alt_held: bool,
    /// OHLC-snapped price for the current preview/drag position.
    /// `None` if no snap was computed (no data, or Alt held).
    ///
    /// Callers MUST use the return value of `snap_to_ohlc()` for the
    /// placement/drag price. This field exists for the compute layer
    /// to read during the same frame (price label adjustment).
    pub snapped_price: Option<f64>,
    /// Current preview price during placement (snapped or raw).
    /// Always `Some` while placing and cursor is in bounds.
    /// The compute layer reads this for the preview line position.
    pub preview_price: Option<f64>,
    /// Whether the tool was in `Placing` mode before a temporary
    /// pan/scale interruption. When the pan/scale ends and mode
    /// returns to `Idle`, this flag causes automatic re-entry
    /// to `Placing`.
    ///
    /// Any new `InteractionMode` that can interrupt `Placing` must
    /// call `try_resume_placing()` in its release path.
    pub was_placing: bool,
}

impl Default for LevelTool {
    fn default() -> Self {
        Self {
            mode: LevelToolMode::Idle,
            alt_held: false,
            snapped_price: None,
            preview_price: None,
            was_placing: false,
        }
    }
}

impl LevelTool {
    /// Snap a raw price to the nearest OHLC value within threshold.
    ///
    /// Searches candles within +/- 1 of the candle nearest to `cursor_x`.
    /// Returns the snapped price, or `raw_price` if no OHLC value is
    /// close enough.
    ///
    /// Also updates `self.snapped_price` as a side effect.
    pub fn snap_to_ohlc(
        &mut self,
        raw_price: f64,
        cursor_x: f32,
        camera: &Camera2D,
        data: &dyn CandleData,
        is_collapsed: bool,
    ) -> f64 {
        if self.alt_held || data.is_empty() {
            self.snapped_price = None;
            return raw_price;
        }

        let cursor_y = camera.price_to_y(raw_price);
        let len = data.len();

        // Find nearest candle index to cursor X.
        let nearest_idx = if is_collapsed {
            let idx_f = camera.x_to_time(cursor_x);
            (idx_f.round() as isize).clamp(0, len as isize - 1) as usize
        } else {
            let cursor_time = camera.x_to_time(cursor_x);
            data.find_index_by_time(cursor_time as i64)
        };

        // Search radius: nearest candle +/- 1.
        let search_start = nearest_idx.saturating_sub(1);
        let search_end = (nearest_idx + 2).min(len);

        // Adaptive snap threshold based on candle density.
        let visible_candles = if is_collapsed {
            (camera.time_end - camera.time_start).max(1.0)
        } else {
            let vis_start = data.find_index_by_time(camera.time_start as i64);
            let vis_end =
                (data.find_index_by_time(camera.time_end as i64) + 1).min(len);
            (vis_end.saturating_sub(vis_start)).max(1) as f64
        };
        let candle_width_px = camera.viewport_width as f64 / visible_candles;
        let snap_threshold_px =
            (candle_width_px as f32).clamp(SNAP_THRESHOLD_MIN_PX, SNAP_THRESHOLD_MAX_PX);

        let mut best_price = raw_price;
        let mut best_dist = f32::MAX;

        for i in search_start..search_end {
            for &p in &[
                data.open(i) as f64,
                data.high(i) as f64,
                data.low(i) as f64,
                data.close(i) as f64,
            ] {
                let py = camera.price_to_y(p);
                let dist = (py - cursor_y).abs();
                if dist < best_dist {
                    best_dist = dist;
                    best_price = p;
                }
            }
        }

        if best_dist <= snap_threshold_px {
            self.snapped_price = Some(best_price);
            best_price
        } else {
            self.snapped_price = None;
            raw_price
        }
    }

    /// Clear all tool state, returning to Idle.
    pub fn cancel(&mut self) {
        self.mode = LevelToolMode::Idle;
        self.alt_held = false;
        self.snapped_price = None;
        self.preview_price = None;
        self.was_placing = false;
    }

    /// Activate level placement mode.
    ///
    /// No-op if currently dragging (prevents H-key during active drag).
    pub fn activate(&mut self) {
        if self.is_dragging() {
            return;
        }
        self.mode = LevelToolMode::Placing;
        self.alt_held = false;
        self.snapped_price = None;
        self.preview_price = None;
        self.was_placing = false;
    }

    /// Returns true if the tool is in Placing or Dragging mode.
    pub fn is_active(&self) -> bool {
        !matches!(self.mode, LevelToolMode::Idle)
    }

    /// Returns true if in Placing mode.
    pub fn is_placing(&self) -> bool {
        matches!(self.mode, LevelToolMode::Placing)
    }

    /// Returns true if in Dragging mode.
    pub fn is_dragging(&self) -> bool {
        matches!(self.mode, LevelToolMode::Dragging { .. })
    }

    /// Temporarily suspend Placing for a pan/scale operation.
    ///
    /// Sets `was_placing` so the tool can resume after.
    pub fn suspend_placing(&mut self) {
        if matches!(self.mode, LevelToolMode::Placing) {
            self.was_placing = true;
            self.mode = LevelToolMode::Idle;
        }
    }

    /// If the tool was suspended from Placing, resume it.
    ///
    /// Called when a pan/scale operation ends.
    pub fn try_resume_placing(&mut self) {
        if self.was_placing && matches!(self.mode, LevelToolMode::Idle) {
            self.mode = LevelToolMode::Placing;
            self.was_placing = false;
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal mock implementing CandleData for snap tests.
    struct MockCandles {
        timestamps: Vec<i64>,
        open: Vec<f32>,
        high: Vec<f32>,
        low: Vec<f32>,
        close: Vec<f32>,
    }

    impl MockCandles {
        fn new(prices: &[(f32, f32, f32, f32)], timestamps: &[i64]) -> Self {
            Self {
                timestamps: timestamps.to_vec(),
                open: prices.iter().map(|p| p.0).collect(),
                high: prices.iter().map(|p| p.1).collect(),
                low: prices.iter().map(|p| p.2).collect(),
                close: prices.iter().map(|p| p.3).collect(),
            }
        }
    }

    impl CandleData for MockCandles {
        fn len(&self) -> usize {
            self.timestamps.len()
        }
        fn timestamp(&self, idx: usize) -> i64 {
            self.timestamps[idx]
        }
        fn open(&self, idx: usize) -> f32 {
            self.open[idx]
        }
        fn high(&self, idx: usize) -> f32 {
            self.high[idx]
        }
        fn low(&self, idx: usize) -> f32 {
            self.low[idx]
        }
        fn close(&self, idx: usize) -> f32 {
            self.close[idx]
        }
        fn volume(&self, idx: usize) -> u32 {
            let _ = idx;
            1000
        }
        fn price_range(&self, range: std::ops::Range<usize>) -> (f32, f32) {
            let mut lo = f32::MAX;
            let mut hi = f32::MIN;
            for i in range {
                lo = lo.min(self.low[i]);
                hi = hi.max(self.high[i]);
            }
            (lo, hi)
        }
        fn find_index_by_time(&self, ts: i64) -> usize {
            self.timestamps
                .binary_search(&ts)
                .unwrap_or_else(|i| i.min(self.len().saturating_sub(1)))
        }
    }

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

    /// 3 candles at timestamps 1.2M, 1.5M, 1.8M.
    fn test_data() -> MockCandles {
        MockCandles::new(
            &[
                (150.0, 160.0, 140.0, 155.0), // candle 0
                (155.0, 170.0, 145.0, 165.0), // candle 1
                (165.0, 175.0, 150.0, 160.0), // candle 2
            ],
            &[1_200_000, 1_500_000, 1_800_000],
        )
    }

    // ── snap_to_ohlc tests ────────────────────────────────────────

    #[test]
    fn snap_to_ohlc_finds_nearest_ohlc() {
        let mut tool = LevelTool::default();
        let camera = test_camera();
        let data = test_data();

        // Cursor near candle 1 (x at midpoint of viewport),
        // price near candle 1's high (170.0).
        let cursor_x = 960.0; // middle of viewport
        let raw_price = 169.0; // close to 170.0 high
        let snapped = tool.snap_to_ohlc(raw_price, cursor_x, &camera, &data, false);

        assert_eq!(snapped, 170.0);
        assert_eq!(tool.snapped_price, Some(170.0));
    }

    #[test]
    fn snap_to_ohlc_beyond_threshold_returns_raw() {
        let mut tool = LevelTool::default();
        let camera = test_camera();
        let data = test_data();

        // Price far from any OHLC value.
        let cursor_x = 960.0;
        let raw_price = 120.0; // far from all candle OHLC values
        let snapped = tool.snap_to_ohlc(raw_price, cursor_x, &camera, &data, false);

        assert_eq!(snapped, raw_price);
        assert_eq!(tool.snapped_price, None);
    }

    #[test]
    fn snap_to_ohlc_alt_held_returns_raw() {
        let mut tool = LevelTool::default();
        tool.alt_held = true;
        let camera = test_camera();
        let data = test_data();

        let cursor_x = 960.0;
        let raw_price = 170.0; // exactly on an OHLC value
        let snapped = tool.snap_to_ohlc(raw_price, cursor_x, &camera, &data, false);

        assert_eq!(snapped, raw_price);
        assert_eq!(tool.snapped_price, None);
    }

    #[test]
    fn snap_to_ohlc_empty_data_returns_raw() {
        let mut tool = LevelTool::default();
        let camera = test_camera();
        let data = MockCandles::new(&[], &[]);

        let snapped = tool.snap_to_ohlc(150.0, 960.0, &camera, &data, false);

        assert_eq!(snapped, 150.0);
        assert_eq!(tool.snapped_price, None);
    }

    #[test]
    fn snap_to_ohlc_updates_snapped_price_field() {
        let mut tool = LevelTool::default();
        let camera = test_camera();
        let data = test_data();

        // First snap — should set snapped_price.
        let cursor_x = 960.0;
        tool.snap_to_ohlc(169.0, cursor_x, &camera, &data, false);
        assert!(tool.snapped_price.is_some());

        // Move far away — should clear snapped_price.
        tool.snap_to_ohlc(120.0, cursor_x, &camera, &data, false);
        assert_eq!(tool.snapped_price, None);
    }

    #[test]
    fn snap_to_ohlc_collapsed_mode() {
        let mut tool = LevelTool::default();
        // In collapsed mode, x_to_time returns an index-like value.
        let camera = Camera2D {
            time_start: 0.0,
            time_end: 3.0, // 3 candles visible
            price_low: 100.0,
            price_high: 200.0,
            viewport_width: 1920,
            viewport_height: 1080,
            dpi_scale: 1.0,
        };
        let data = test_data();

        // cursor_x at 960px => x_to_time => 1.5, rounds to index 2
        // Search candles 1..3 (indices 1 and 2).
        let raw_price = 174.0; // close to candle 2's high (175.0)
        let snapped = tool.snap_to_ohlc(raw_price, 960.0, &camera, &data, true);

        assert_eq!(snapped, 175.0);
    }

    #[test]
    fn snap_to_ohlc_ignores_distant_candles() {
        let mut tool = LevelTool::default();
        let camera = test_camera();
        // 6 candles spread across time range.
        let data = MockCandles::new(
            &[
                (150.0, 160.0, 140.0, 155.0), // candle 0 at 1.1M
                (155.0, 165.0, 145.0, 160.0), // candle 1 at 1.2M
                (160.0, 170.0, 150.0, 165.0), // candle 2 at 1.4M
                (165.0, 175.0, 155.0, 170.0), // candle 3 at 1.6M
                (170.0, 180.0, 160.0, 175.0), // candle 4 at 1.8M
                (175.0, 199.0, 165.0, 180.0), // candle 5 at 1.9M
            ],
            &[
                1_100_000, 1_200_000, 1_400_000, 1_600_000, 1_800_000,
                1_900_000,
            ],
        );

        // Cursor near candle 0 (left side of viewport).
        // Candle 5 has high=199.0 which is close to price 199.0,
        // but it's 5 candles away — should NOT snap to it.
        let cursor_x = 192.0; // ~10% across viewport, near candle 0
        let raw_price = 199.0;
        let snapped = tool.snap_to_ohlc(raw_price, cursor_x, &camera, &data, false);

        // Should NOT snap to candle 5's high (199.0) because it's
        // outside the +/-1 search radius from nearest candle.
        assert_ne!(tool.snapped_price, Some(199.0));
        // Should return raw since no nearby OHLC is close in Y.
        assert_eq!(snapped, raw_price);
    }

    // ── State transition tests ────────────────────────────────────

    #[test]
    fn activate_sets_placing_mode() {
        let mut tool = LevelTool::default();
        tool.activate();
        assert!(tool.is_placing());
        assert!(tool.is_active());
        assert!(!tool.is_dragging());
    }

    #[test]
    fn activate_noop_during_drag() {
        let mut tool = LevelTool::default();
        tool.mode = LevelToolMode::Dragging {
            level_id: 1,
            grab_offset: 0.0,
        };
        tool.activate();
        assert!(tool.is_dragging()); // still dragging
    }

    #[test]
    fn cancel_clears_all_state() {
        let mut tool = LevelTool::default();
        tool.mode = LevelToolMode::Placing;
        tool.alt_held = true;
        tool.snapped_price = Some(150.0);
        tool.was_placing = true;

        tool.cancel();

        assert_eq!(tool.mode, LevelToolMode::Idle);
        assert!(!tool.alt_held);
        assert_eq!(tool.snapped_price, None);
        assert!(!tool.was_placing);
        assert!(!tool.is_active());
    }

    #[test]
    fn suspend_and_resume_placing() {
        let mut tool = LevelTool::default();
        tool.activate();
        assert!(tool.is_placing());

        tool.suspend_placing();
        assert!(!tool.is_active());
        assert!(tool.was_placing);

        tool.try_resume_placing();
        assert!(tool.is_placing());
        assert!(!tool.was_placing);
    }

    #[test]
    fn suspend_when_not_placing_is_noop() {
        let mut tool = LevelTool::default();
        tool.suspend_placing();
        assert!(!tool.was_placing);
        assert!(!tool.is_active());
    }

    #[test]
    fn resume_when_not_suspended_is_noop() {
        let mut tool = LevelTool::default();
        tool.try_resume_placing();
        assert!(!tool.is_active());
    }

    #[test]
    fn is_active_predicates() {
        let mut tool = LevelTool::default();

        // Idle
        assert!(!tool.is_active());
        assert!(!tool.is_placing());
        assert!(!tool.is_dragging());

        // Placing
        tool.mode = LevelToolMode::Placing;
        assert!(tool.is_active());
        assert!(tool.is_placing());
        assert!(!tool.is_dragging());

        // Dragging
        tool.mode = LevelToolMode::Dragging {
            level_id: 42,
            grab_offset: 1.5,
        };
        assert!(tool.is_active());
        assert!(!tool.is_placing());
        assert!(tool.is_dragging());
    }
}
