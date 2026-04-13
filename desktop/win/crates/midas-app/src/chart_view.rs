//! Per-(symbol, timeframe) chart view state.
//!
//! `ChartViewState` is the single authority for how a chart's camera
//! should be positioned when data loads. It stores the user's zoom
//! levels (X = visible candle count, Y = price zoom factor) and applies
//! them consistently regardless of which code path triggers a data load.
//!
//! Stored in [`ChartViewStore`], a HashMap keyed by `(symbol, timeframe)`.

use midas_chart::camera::Camera2D;
use midas_core::{CandleBuffer, Timeframe};

/// Default number of visible candles when no saved zoom exists.
const DEFAULT_VISIBLE_CANDLES: usize = 200;

/// Fraction of data span added as right padding.
/// Places the last candle at ~87.5% of the viewport
/// (middle of the 4th horizontal quadrant).
const RIGHT_PADDING_FRACTION: f64 = 0.14;

/// Default price zoom factor: 1.0 = auto-fit to data range, plus 10%
/// padding on each side. Values < 1.0 = zoomed in (squeezed Y),
/// values > 1.0 = zoomed out (stretched Y).
const DEFAULT_PRICE_ZOOM: f64 = 1.2;

/// Per-(symbol, timeframe) view settings.
#[derive(Clone, Debug, Default)]
pub struct ChartViewState {
    /// Number of candles the user wants visible in the viewport.
    /// `None` = use [`DEFAULT_VISIBLE_CANDLES`].
    visible_candles: Option<usize>,

    /// Price zoom factor: ratio of visible price range to the data's
    /// natural high-low range for the visible candles.
    ///
    /// - `None` = use [`DEFAULT_PRICE_ZOOM`] (auto-fit + padding)
    /// - `1.0` = exactly the data range, no padding
    /// - `< 1.0` = zoomed in (user squeezed Y axis)
    /// - `> 1.0` = zoomed out (user stretched Y axis or has padding)
    price_zoom_factor: Option<f64>,
}

impl ChartViewState {
    /// The effective visible candle count.
    pub fn candle_count(&self) -> usize {
        self.visible_candles.unwrap_or(DEFAULT_VISIBLE_CANDLES)
    }

    /// Record the current X and Y zoom levels from a live camera + data.
    ///
    /// **X zoom**: counts visible candles in the camera's time window.
    /// **Y zoom**: computes the ratio of the camera's price range to
    /// the natural data range of those visible candles.
    pub fn capture_from_camera(
        &mut self,
        camera: &Camera2D,
        buf: &CandleBuffer,
        collapse_gaps: bool,
    ) {
        if buf.is_empty() {
            return;
        }

        // X zoom: visible candle count.
        let (vis_start, vis_end) = if collapse_gaps {
            let s = (camera.time_start.floor() as usize).min(buf.len());
            let e = (camera.time_end.ceil() as usize).min(buf.len());
            (s, e)
        } else {
            let s = buf.find_index_by_time(camera.time_start as i64);
            let e = (buf.find_index_by_time(camera.time_end as i64) + 1).min(buf.len());
            (s, e)
        };
        let count = vis_end.saturating_sub(vis_start);
        if count > 0 {
            self.visible_candles = Some(count);
        }

        // Y zoom: price range ratio.
        if vis_start < vis_end {
            let (lo, hi) = buf.price_range(vis_start..vis_end);
            let data_range = (hi - lo) as f64;
            if data_range > 0.0 {
                let camera_range = camera.price_high - camera.price_low;
                if camera_range > 0.0 {
                    self.price_zoom_factor = Some(camera_range / data_range);
                }
            }
        }
    }

    /// Position the camera to show the last N candles at the sweet spot.
    ///
    /// This is the **single authority** for camera positioning on data load.
    /// - **X**: last `candle_count()` candles, last candle at ~87.5%
    /// - **Y**: auto-scaled to visible range, then stretched/squeezed
    ///   by the saved `price_zoom_factor` (preserves user's Y zoom)
    /// - `data_time_start/end`: set for scroll clamping
    pub fn position_camera(
        &self,
        camera: &mut Camera2D,
        buf: &CandleBuffer,
        collapse_gaps: bool,
        data_time_start: &mut f64,
        data_time_end: &mut f64,
    ) {
        if buf.is_empty() {
            return;
        }
        let len = buf.len();

        // Set data bounds for scroll clamping.
        if collapse_gaps {
            *data_time_start = 0.0;
            *data_time_end = len as f64;
        } else {
            *data_time_start = buf.timestamps[0] as f64;
            *data_time_end = buf.timestamps[len - 1] as f64;
        }

        // X positioning.
        let vc = self.candle_count().min(len).max(1);

        if collapse_gaps {
            let start_idx = (len - vc) as f64;
            let data_span = len as f64 - start_idx;
            camera.time_start = start_idx;
            camera.time_end = len as f64 + data_span * RIGHT_PADDING_FRACTION;
        } else {
            let last_ts = buf.timestamps[len - 1] as f64;
            let first_vis = buf.timestamps[len - vc] as f64;
            let data_span = last_ts - first_vis;
            camera.time_start = first_vis;
            camera.time_end = last_ts + data_span * RIGHT_PADDING_FRACTION;
        }

        // Y positioning: center on the last candle's close price,
        // then apply the saved zoom factor to the visible data range.
        let range = (len - vc)..len;
        let (lo, hi) = buf.price_range(range);
        let data_range = (hi - lo) as f64;
        let last_close = buf.closes[len - 1] as f64;

        let factor = self.price_zoom_factor.unwrap_or(DEFAULT_PRICE_ZOOM);
        let visible_range = if data_range > 0.0 {
            data_range * factor
        } else {
            // Flat data (single price) — use a small default range.
            last_close * 0.02
        };

        camera.price_low = last_close - visible_range / 2.0;
        camera.price_high = last_close + visible_range / 2.0;
    }
}

/// Central store for per-(symbol, timeframe) view settings.
///
/// Session-scoped (not persisted to disk). Resets on app restart.
#[derive(Default, Debug)]
pub struct ChartViewStore {
    views: std::collections::HashMap<(String, Timeframe), ChartViewState>,
}

impl ChartViewStore {
    /// Get or create the view state for a (symbol, timeframe) pair.
    pub fn get_or_default(&mut self, symbol: &str, tf: Timeframe) -> &mut ChartViewState {
        self.views
            .entry((symbol.to_uppercase(), tf))
            .or_default()
    }

    /// Get the view state if it exists (read-only).
    pub fn get(&self, symbol: &str, tf: Timeframe) -> Option<&ChartViewState> {
        self.views.get(&(symbol.to_uppercase(), tf))
    }
}
