//! Self-contained horizontal level tool.
//!
//! Owns all state for level placement, dragging, and OHLC snapping.
//! Lives as a field on [`ChartState`]. The interaction layer delegates
//! level-related event handling to this struct.

use crate::camera::Camera2D;
use crate::widget::AnnotationId;
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
        /// ID of the annotation being dragged.
        level_id: AnnotationId,
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
            let vis_end = (data.find_index_by_time(camera.time_end as i64) + 1).min(len);
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

#[cfg(test)]
mod tests;
