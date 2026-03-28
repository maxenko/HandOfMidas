//! `compute_chart_scene()` -- pure function that transforms chart input into
//! a framework-agnostic [`ChartScene`].
//!
//! This is the heart of the chart component. It takes a [`ChartInput`] and
//! produces a [`ChartScene`] containing all the data needed to render a single
//! chart frame. No GPU, no framework -- just math.

use crate::camera::Camera2D;
use crate::input::ChartInput;
use crate::instances::{
    AxisLabel, CandleInstance, CrosshairRender, GridLine, LevelRender, OhlcvOverlay,
    SessionBoundary, VolumeInstance,
};
use crate::scene::{ChartScene, SceneGenerations};
use midas_core::CandleData;

/// Fraction of the viewport height reserved for volume bars at the bottom.
pub const VOLUME_AREA_FRACTION: f32 = 0.20;

/// Body width as a fraction of the full candle slot width.
const BODY_WIDTH_FRACTION: f32 = 0.70;

/// Minimum body width in logical pixels (so tiny candles are still visible).
const MIN_BODY_WIDTH: f32 = 1.0;

/// Maximum number of grid lines on either axis to prevent over-density.
const MAX_GRID_LINES: usize = 50;

/// Color for session boundary lines (faint blue-gray, semi-transparent).
const SESSION_BOUNDARY_COLOR: [f32; 4] = [0.3, 0.3, 0.5, 0.30];

/// A time gap between consecutive candles is considered a session boundary
/// if it exceeds this multiple of the expected candle duration.
const SESSION_GAP_THRESHOLD: f64 = 1.5;

/// Pure function: chart input -> renderable scene.
///
/// This function is unit-testable without any GPU context. You can assert
/// on candle positions, grid line spacing, label text, crosshair snap
/// behavior, etc.
///
/// When `input.collapse_gaps` is `true`, candle X positions are based on
/// their sequential index in the visible range rather than their timestamp.
/// This eliminates overnight and weekend gaps. Session boundary markers
/// are inserted where the time gap exceeds the expected candle duration.
pub fn compute_chart_scene(input: &ChartInput<'_>) -> ChartScene {
    let camera = input.camera;
    let data = input.data;

    // 2. Compute candle duration for spacing.
    let candle_duration = estimate_candle_duration(data);

    if input.collapse_gaps {
        // In collapsed mode, camera X axis is in index-space (not timestamps).
        let (vis_start, vis_end) = visible_candle_range_collapsed(data, camera);
        compute_collapsed_scene(input, camera, data, vis_start, vis_end, candle_duration)
    } else {
        // 1. Compute visible candle index range from camera time bounds.
        let (vis_start, vis_end) = visible_candle_range(data, camera);
        compute_normal_scene(input, camera, data, vis_start, vis_end, candle_duration)
    }
}

/// Compute the chart scene with normal (linear time) positioning.
fn compute_normal_scene(
    input: &ChartInput<'_>,
    camera: &Camera2D,
    data: &dyn CandleData,
    vis_start: usize,
    vis_end: usize,
    candle_duration: f64,
) -> ChartScene {
    let slot_width = camera.pixels_per_candle(candle_duration);
    let body_width = (slot_width * BODY_WIDTH_FRACTION).max(MIN_BODY_WIDTH);
    let wick_width = (1.0_f32 / input.dpi_scale).max(1.0 / input.dpi_scale);

    let candle_count = vis_end.saturating_sub(vis_start);

    // Normal mode: X from timestamp via camera mapping.
    let x_from_ts = |i: usize| -> f32 {
        let ts = data.timestamp(i) as f64;
        camera.snap_to_pixel(camera.time_to_x(ts))
    };

    let candles = build_candle_instances(
        data,
        camera,
        vis_start,
        vis_end,
        body_width,
        wick_width,
        &input.bull_color,
        &input.bear_color,
        &x_from_ts,
    );

    let volumes = build_volume_instances(
        data,
        camera,
        vis_start,
        vis_end,
        body_width,
        input.viewport_height,
        &input.volume_bull_color,
        &input.volume_bear_color,
        input.volume_scale,
        &x_from_ts,
    );
    let volume_count = volumes.as_ref().map_or(0, |v| v.len());

    let grid_lines = compute_grid_lines(camera, &input.grid_color);
    let y_labels = compute_y_labels(camera);
    let x_labels = compute_x_labels(camera, candle_duration);
    let levels = compute_levels(input.levels, camera);
    let crosshair = compute_crosshair(input.crosshair, data, camera, candle_duration, input.symbol);
    let date_labels = crate::date_labels::for_normal_mode(camera, candle_duration);
    let effective_vol = (VOLUME_AREA_FRACTION * input.volume_scale).min(0.80);
    let separator_y = input.viewport_height as f32 * (1.0 - effective_vol);
    let projection = camera.projection_matrix();

    ChartScene {
        projection,
        viewport_width: input.viewport_width,
        viewport_height: input.viewport_height,
        background_color: input.background_color,
        candles,
        candle_count,
        volumes,
        volume_count,
        grid_lines,
        x_labels,
        y_labels,
        levels,
        crosshair,
        session_boundaries: Vec::new(),
        separator_y,
        date_labels,
        generations: SceneGenerations {
            candles: input.dirty.candles,
            camera: input.dirty.camera,
            grid: input.dirty.grid,
            levels: input.dirty.levels,
            crosshair: input.dirty.crosshair,
            theme: input.dirty.theme,
        },
    }
}

/// Compute the chart scene with gap-collapsed (index-based) positioning.
///
/// In collapsed mode the camera's X axis represents candle **indices**
/// (fractional, e.g. 50.0 .. 250.0) instead of timestamps.  Each candle at
/// global index `i` is centred at `camera.time_to_x(i + 0.5)`, giving
/// uniform spacing that is invariant to calendar gaps.  Panning and zooming
/// therefore work identically to normal mode — only the coordinate system
/// changes.
fn compute_collapsed_scene(
    input: &ChartInput<'_>,
    camera: &Camera2D,
    data: &dyn CandleData,
    vis_start: usize,
    vis_end: usize,
    candle_duration: f64,
) -> ChartScene {
    let visible_count = vis_end.saturating_sub(vis_start);

    // Slot width derived from the camera's linear mapping: 1 index unit = N pixels.
    let index_range = camera.time_end - camera.time_start;
    let slot_width = if index_range > 0.0 {
        (camera.viewport_width as f64 / index_range) as f32
    } else {
        0.0
    };
    let body_width = (slot_width * BODY_WIDTH_FRACTION).max(MIN_BODY_WIDTH);
    let wick_width = (1.0_f32 / input.dpi_scale).max(1.0 / input.dpi_scale);

    // Collapsed mode: X from sequential candle index via camera mapping.
    let x_from_idx = |i: usize| -> f32 {
        camera.snap_to_pixel(camera.time_to_x(i as f64 + 0.5))
    };

    // Helper for functions that still take local-index closures (grid, labels, boundaries).
    let index_to_x = |local_idx: usize| -> f32 {
        x_from_idx(vis_start + local_idx)
    };

    let candles = build_candle_instances(
        data,
        camera,
        vis_start,
        vis_end,
        body_width,
        wick_width,
        &input.bull_color,
        &input.bear_color,
        &x_from_idx,
    );
    let candle_count = visible_count;

    let volumes = build_volume_instances(
        data,
        camera,
        vis_start,
        vis_end,
        body_width,
        input.viewport_height,
        &input.volume_bull_color,
        &input.volume_bear_color,
        input.volume_scale,
        &x_from_idx,
    );
    let volume_count = volumes.as_ref().map_or(0, |v| v.len());

    // Detect session boundaries.
    let session_boundaries = detect_session_boundaries(
        data,
        vis_start,
        vis_end,
        candle_duration,
        &index_to_x,
    );

    // Grid lines: Y-axis (price) lines work as normal. X-axis (time) labels
    // are positioned at collapsed X coordinates for selected candles.
    let grid_lines = compute_collapsed_grid_lines(
        camera,
        data,
        vis_start,
        vis_end,
        candle_duration,
        &index_to_x,
    );

    let y_labels = compute_y_labels(camera);
    let x_labels = compute_collapsed_x_labels(
        camera,
        data,
        vis_start,
        vis_end,
        candle_duration,
        &index_to_x,
    );

    let levels = compute_levels(input.levels, camera);

    // Crosshair in collapsed mode: convert cursor X to the nearest candle index.
    let crosshair = compute_collapsed_crosshair(
        input.crosshair,
        data,
        camera,
        vis_start,
        vis_end,
        input.symbol,
        &index_to_x,
    );

    let date_labels = crate::date_labels::for_collapsed_mode(
        camera,
        data,
        vis_start,
        vis_end,
        candle_duration,
        &index_to_x,
    );
    let effective_vol = (VOLUME_AREA_FRACTION * input.volume_scale).min(0.80);
    let separator_y = input.viewport_height as f32 * (1.0 - effective_vol);
    let projection = camera.projection_matrix();

    ChartScene {
        projection,
        viewport_width: input.viewport_width,
        viewport_height: input.viewport_height,
        background_color: input.background_color,
        candles,
        candle_count,
        volumes,
        volume_count,
        grid_lines,
        x_labels,
        y_labels,
        levels,
        crosshair,
        session_boundaries,
        separator_y,
        date_labels,
        generations: SceneGenerations {
            candles: input.dirty.candles,
            camera: input.dirty.camera,
            grid: input.dirty.grid,
            levels: input.dirty.levels,
            crosshair: input.dirty.crosshair,
            theme: input.dirty.theme,
        },
    }
}

/// Compute the visible candle index range `[start, end)` from the camera's
/// time bounds. Clamps to `[0, data.len())`.
fn visible_candle_range(data: &dyn CandleData, camera: &Camera2D) -> (usize, usize) {
    if data.is_empty() {
        return (0, 0);
    }
    let start = data.find_index_by_time(camera.time_start as i64);
    // Add 1 to end index so we include the candle at the right boundary.
    let end = (data.find_index_by_time(camera.time_end as i64) + 1).min(data.len());
    // Ensure start is always before or equal to end.
    let start = start.min(end);
    (start, end)
}

/// Compute the visible candle index range when in collapsed (index-space)
/// mode. The camera's `time_start` / `time_end` represent fractional candle
/// indices, not timestamps.
fn visible_candle_range_collapsed(data: &dyn CandleData, camera: &Camera2D) -> (usize, usize) {
    if data.is_empty() {
        return (0, 0);
    }
    let len = data.len();
    let start = (camera.time_start.floor() as isize).max(0) as usize;
    let end = (camera.time_end.ceil() as usize).min(len);
    let start = start.min(end);
    (start, end)
}

/// Estimate the candle duration in milliseconds from data spacing.
///
/// Uses the time difference between the first two candles if available.
/// Falls back to 60_000 ms (1-minute candles) if there are fewer than 2 candles.
pub fn estimate_candle_duration(data: &dyn CandleData) -> f64 {
    if data.len() < 2 {
        return 60_000.0;
    }
    let dt = (data.timestamp(1) - data.timestamp(0)).abs() as f64;
    if dt > 0.0 {
        dt
    } else {
        60_000.0
    }
}

/// Build CandleInstance array for visible candles.
///
/// `x_for_candle(data_index)` returns the pixel X position for each candle.
/// In normal mode this maps timestamps; in collapsed mode it maps indices.
#[allow(clippy::too_many_arguments)]
fn build_candle_instances(
    data: &dyn CandleData,
    camera: &Camera2D,
    vis_start: usize,
    vis_end: usize,
    body_width: f32,
    wick_width: f32,
    bull_color: &[f32; 4],
    bear_color: &[f32; 4],
    x_for_candle: &dyn Fn(usize) -> f32,
) -> Option<Vec<CandleInstance>> {
    if vis_start >= vis_end {
        return None;
    }

    let mut instances = Vec::with_capacity(vis_end - vis_start);

    for i in vis_start..vis_end {
        let open = data.open(i) as f64;
        let close = data.close(i) as f64;
        let high = data.high(i) as f64;
        let low = data.low(i) as f64;

        let x = x_for_candle(i);
        let body_top_price = open.max(close);
        let body_bottom_price = open.min(close);

        let body_top = camera.snap_to_pixel(camera.price_to_y(body_top_price));
        let body_bottom = camera.snap_to_pixel(camera.price_to_y(body_bottom_price));
        let wick_top = camera.snap_to_pixel(camera.price_to_y(high));
        let wick_bottom = camera.snap_to_pixel(camera.price_to_y(low));

        // Ensure body has at least 1 physical pixel height for doji candles.
        let body_bottom = if (body_bottom - body_top).abs() < 1.0 / camera.dpi_scale {
            body_top + (1.0 / camera.dpi_scale)
        } else {
            body_bottom
        };

        let is_bull = close >= open;
        let color = if is_bull { *bull_color } else { *bear_color };

        instances.push(CandleInstance {
            x,
            body_top,
            body_bottom,
            wick_top,
            wick_bottom,
            width: body_width,
            wick_width,
            _pad0: 0.0,
            color,
        });
    }

    Some(instances)
}

/// Build VolumeInstance array for visible candles.
///
/// `x_for_candle(data_index)` returns the pixel X position for each bar.
/// Volume bars occupy the bottom `VOLUME_AREA_FRACTION` of the viewport.
#[allow(clippy::too_many_arguments)]
fn build_volume_instances(
    data: &dyn CandleData,
    camera: &Camera2D,
    vis_start: usize,
    vis_end: usize,
    bar_width: f32,
    viewport_height: u32,
    volume_bull_color: &[f32; 4],
    volume_bear_color: &[f32; 4],
    volume_scale: f32,
    x_for_candle: &dyn Fn(usize) -> f32,
) -> Option<Vec<VolumeInstance>> {
    if vis_start >= vis_end {
        return None;
    }

    // Find max volume in visible range for normalization.
    let mut max_volume: u32 = 0;
    for i in vis_start..vis_end {
        max_volume = max_volume.max(data.volume(i));
    }

    if max_volume == 0 {
        return None;
    }

    let vh = viewport_height as f32;
    let volume_area_top = vh * (1.0 - VOLUME_AREA_FRACTION);
    let volume_area_bottom = vh;
    let volume_area_height = volume_area_bottom - volume_area_top;

    let mut instances = Vec::with_capacity(vis_end - vis_start);

    for i in vis_start..vis_end {
        let x = x_for_candle(i);
        let vol_fraction = data.volume(i) as f32 / max_volume as f32;
        let bar_height = vol_fraction * volume_area_height * volume_scale;
        let y_top = camera.snap_to_pixel(volume_area_bottom - bar_height);
        let y_bottom = volume_area_bottom;

        let is_bull = data.close(i) >= data.open(i);
        let color = if is_bull {
            *volume_bull_color
        } else {
            *volume_bear_color
        };

        instances.push(VolumeInstance {
            x,
            y_top,
            y_bottom,
            width: bar_width,
            color,
        });
    }

    Some(instances)
}

/// Detect session boundaries in the visible range.
///
/// A session boundary is placed between candle `i-1` and candle `i` when
/// `timestamp[i] - timestamp[i-1] > candle_duration * SESSION_GAP_THRESHOLD`.
fn detect_session_boundaries(
    data: &dyn CandleData,
    vis_start: usize,
    vis_end: usize,
    candle_duration: f64,
    index_to_x: &dyn Fn(usize) -> f32,
) -> Vec<SessionBoundary> {
    if vis_end.saturating_sub(vis_start) < 2 {
        return Vec::new();
    }

    let threshold = candle_duration * SESSION_GAP_THRESHOLD;
    let mut boundaries = Vec::new();

    for i in (vis_start + 1)..vis_end {
        let gap = (data.timestamp(i) - data.timestamp(i - 1)).abs() as f64;
        if gap > threshold {
            let local_idx = i - vis_start;
            // Place the boundary line at the midpoint between the two candle slots.
            let x_prev = index_to_x(local_idx - 1);
            let x_curr = index_to_x(local_idx);
            let boundary_x = (x_prev + x_curr) / 2.0;

            boundaries.push(SessionBoundary {
                x: boundary_x,
                color: SESSION_BOUNDARY_COLOR,
            });
        }
    }

    boundaries
}

/// Compute grid lines in gap-collapsed mode.
///
/// Horizontal (price) grid lines work identically to normal mode.
/// Vertical (time) grid lines are placed at collapsed X coordinates
/// for evenly-spaced candle indices, with labels showing actual timestamps.
/// Compute horizontal price grid lines for collapsed mode.
///
/// Produces ONLY horizontal (price) grid lines. Vertical time lines
/// are handled by the `date_labels` module.
fn compute_collapsed_grid_lines(
    camera: &Camera2D,
    _data: &dyn CandleData,
    _vis_start: usize,
    _vis_end: usize,
    _candle_duration: f64,
    _index_to_x: &dyn Fn(usize) -> f32,
) -> Vec<GridLine> {
    let mut lines = Vec::new();

    let price_range = camera.price_high - camera.price_low;
    if price_range > 0.0 {
        let price_step = nice_step(price_range, camera.viewport_height as f64 / 80.0);
        if price_step > 0.0 {
            let first = (camera.price_low / price_step).ceil() * price_step;
            let mut price = first;
            let mut count = 0;
            while price < camera.price_high && count < MAX_GRID_LINES {
                let y = camera.snap_to_pixel(camera.price_to_y(price));
                let is_major = is_major_grid_step(price, price_step);
                lines.push(GridLine {
                    position: y,
                    label: format_price(price),
                    is_major,
                });
                price += price_step;
                count += 1;
            }
        }
    }

    lines
}

/// Compute X-axis (time) labels in gap-collapsed mode.
fn compute_collapsed_x_labels(
    camera: &Camera2D,
    data: &dyn CandleData,
    vis_start: usize,
    vis_end: usize,
    _candle_duration: f64,
    index_to_x: &dyn Fn(usize) -> f32,
) -> Vec<AxisLabel> {
    let visible_count = vis_end.saturating_sub(vis_start);
    if visible_count == 0 {
        return Vec::new();
    }

    let desired_labels = (camera.viewport_width as f64 / 150.0).max(1.0) as usize;
    let step = (visible_count / desired_labels).max(1);

    let mut labels = Vec::new();
    let mut idx = 0;
    let mut count = 0;
    while idx < visible_count && count < MAX_GRID_LINES {
        let x = index_to_x(idx);
        let data_idx = vis_start + idx;
        let ts = data.timestamp(data_idx);
        labels.push(AxisLabel {
            text: format_time_ms(ts),
            screen_x: x,
            screen_y: camera.viewport_height as f32,
            bg_color: [0.15, 0.15, 0.15, 0.9],
            text_color: [0.8, 0.8, 0.8, 1.0],
        });
        idx += step;
        count += 1;
    }
    labels
}

/// Compute crosshair in gap-collapsed mode.
///
/// The cursor X is converted to the nearest candle index via the camera's
/// index-space mapping, then snapped to that candle's collapsed X position.
#[allow(clippy::too_many_arguments)]
fn compute_collapsed_crosshair(
    crosshair: Option<(f32, f32)>,
    data: &dyn CandleData,
    camera: &Camera2D,
    vis_start: usize,
    vis_end: usize,
    symbol: &str,
    index_to_x: &dyn Fn(usize) -> f32,
) -> Option<CrosshairRender> {
    let (cx, cy) = crosshair?;

    let visible_count = vis_end.saturating_sub(vis_start);
    if visible_count == 0 || data.is_empty() {
        return None;
    }

    // Convert cursor X to a global candle index via the camera's index-space.
    let global_idx_f = camera.x_to_time(cx);
    let global_idx = (global_idx_f.round() as usize)
        .max(vis_start)
        .min(vis_end.saturating_sub(1));
    let local_idx = global_idx - vis_start;
    let data_idx = global_idx;
    let snap_x = index_to_x(local_idx);
    let snap_y = camera.snap_to_pixel(cy);
    let snap_ts = data.timestamp(data_idx);

    // Build price label at cursor Y.
    let cursor_price = camera.y_to_price(cy);
    let price_label = AxisLabel {
        text: format_price(cursor_price),
        screen_x: camera.viewport_width as f32,
        screen_y: snap_y,
        bg_color: [0.2, 0.2, 0.2, 0.95],
        text_color: [1.0, 1.0, 1.0, 1.0],
    };

    // Build time label at snap X.
    let time_label = AxisLabel {
        text: format_time_ms(snap_ts),
        screen_x: snap_x,
        screen_y: camera.viewport_height as f32,
        bg_color: [0.2, 0.2, 0.2, 0.95],
        text_color: [1.0, 1.0, 1.0, 1.0],
    };

    // Build OHLCV overlay.
    let o = data.open(data_idx);
    let h = data.high(data_idx);
    let l = data.low(data_idx);
    let c = data.close(data_idx);
    let v = data.volume(data_idx);
    let is_bullish = c >= o;

    let (change, change_pct) = if data_idx > 0 {
        let prev_close = data.close(data_idx - 1);
        let chg = c - prev_close;
        let pct = if prev_close.abs() > f32::EPSILON {
            (chg / prev_close) * 100.0
        } else {
            0.0
        };
        (Some(chg), Some(pct))
    } else {
        (None, None)
    };

    let ohlcv_overlay = Some(OhlcvOverlay {
        symbol: symbol.to_string(),
        datetime: format_datetime_long(snap_ts),
        open: o,
        high: h,
        low: l,
        close: c,
        volume: v,
        is_bullish,
        change,
        change_pct,
    });

    Some(CrosshairRender {
        vertical_x: snap_x,
        horizontal_y: snap_y,
        price_label,
        time_label,
        line_color: [0.7, 0.7, 0.7, 0.5],
        ohlcv_overlay,
    })
}

/// Compute horizontal price grid lines with adaptive density.
///
/// Produces ONLY horizontal (price) grid lines. Vertical time lines
/// are handled by the `date_labels` module.
fn compute_grid_lines(camera: &Camera2D, grid_color: &[f32; 4]) -> Vec<GridLine> {
    let mut lines = Vec::new();

    let price_range = camera.price_high - camera.price_low;
    if price_range > 0.0 {
        let price_step = nice_step(price_range, camera.viewport_height as f64 / 80.0);
        if price_step > 0.0 {
            let first = (camera.price_low / price_step).ceil() * price_step;
            let mut price = first;
            let mut count = 0;
            while price < camera.price_high && count < MAX_GRID_LINES {
                let y = camera.snap_to_pixel(camera.price_to_y(price));
                let is_major = is_major_grid_step(price, price_step);
                lines.push(GridLine {
                    position: y,
                    label: format_price(price),
                    is_major,
                });
                price += price_step;
                count += 1;
            }
        }
    }

    let _ = grid_color;
    lines
}

/// Compute Y-axis (price) labels.
fn compute_y_labels(camera: &Camera2D) -> Vec<AxisLabel> {
    let price_range = camera.price_high - camera.price_low;
    if price_range <= 0.0 {
        return Vec::new();
    }
    let price_step = nice_step(price_range, camera.viewport_height as f64 / 80.0);
    if price_step <= 0.0 {
        return Vec::new();
    }

    let mut labels = Vec::new();
    let first = (camera.price_low / price_step).ceil() * price_step;
    let mut price = first;
    let mut count = 0;
    while price < camera.price_high && count < MAX_GRID_LINES {
        let y = camera.snap_to_pixel(camera.price_to_y(price));
        labels.push(AxisLabel {
            text: format_price(price),
            screen_x: camera.viewport_width as f32,
            screen_y: y,
            bg_color: [0.15, 0.15, 0.15, 0.9],
            text_color: [0.8, 0.8, 0.8, 1.0],
        });
        price += price_step;
        count += 1;
    }
    labels
}

/// Compute X-axis (time) labels.
fn compute_x_labels(camera: &Camera2D, candle_duration: f64) -> Vec<AxisLabel> {
    let time_range = camera.time_end - camera.time_start;
    if time_range <= 0.0 {
        return Vec::new();
    }
    let time_step = nice_time_step(time_range, camera.viewport_width as f64 / 150.0);
    if time_step <= 0.0 {
        return Vec::new();
    }

    let _ = candle_duration;

    let mut labels = Vec::new();
    let first = (camera.time_start / time_step).ceil() * time_step;
    let mut t = first;
    let mut count = 0;
    while t < camera.time_end && count < MAX_GRID_LINES {
        let x = camera.snap_to_pixel(camera.time_to_x(t));
        labels.push(AxisLabel {
            text: format_time_ms(t as i64),
            screen_x: x,
            screen_y: camera.viewport_height as f32,
            bg_color: [0.15, 0.15, 0.15, 0.9],
            text_color: [0.8, 0.8, 0.8, 1.0],
        });
        t += time_step;
        count += 1;
    }
    labels
}

/// Compute render data for horizontal levels.
fn compute_levels(
    levels: &[crate::levels::HorizontalLevel],
    camera: &Camera2D,
) -> Vec<LevelRender> {
    levels
        .iter()
        .map(|lev| {
            let y = camera.snap_to_pixel(camera.price_to_y(lev.price));
            LevelRender {
                price: lev.price,
                screen_y: y,
                color: lev.color,
                line_width: lev.line_width,
                is_selected: false,
                is_being_dragged: false,
                original_screen_y: None,
                label_text: format_price(lev.price),
            }
        })
        .collect()
}

/// Compute crosshair render data.
///
/// The vertical line snaps to the center of the nearest candle. The horizontal
/// line follows the cursor Y position exactly. Includes OHLCV overlay data
/// for the candle under the cursor (TC2000-style data box).
fn compute_crosshair(
    crosshair: Option<(f32, f32)>,
    data: &dyn CandleData,
    camera: &Camera2D,
    candle_duration: f64,
    symbol: &str,
) -> Option<CrosshairRender> {
    let (cx, cy) = crosshair?;

    if data.is_empty() {
        return None;
    }

    // Convert cursor X to time, then find the nearest candle.
    let cursor_time = camera.x_to_time(cx);
    let nearest_idx = data.find_index_by_time(cursor_time as i64);
    let snap_ts = data.timestamp(nearest_idx) as f64;
    let snap_x = camera.snap_to_pixel(camera.time_to_x(snap_ts));
    let snap_y = camera.snap_to_pixel(cy);

    // Build price label at cursor Y.
    let cursor_price = camera.y_to_price(cy);
    let price_label = AxisLabel {
        text: format_price(cursor_price),
        screen_x: camera.viewport_width as f32,
        screen_y: snap_y,
        bg_color: [0.2, 0.2, 0.2, 0.95],
        text_color: [1.0, 1.0, 1.0, 1.0],
    };

    // Build time label at snap X.
    let time_label = AxisLabel {
        text: format_time_ms(snap_ts as i64),
        screen_x: snap_x,
        screen_y: camera.viewport_height as f32,
        bg_color: [0.2, 0.2, 0.2, 0.95],
        text_color: [1.0, 1.0, 1.0, 1.0],
    };

    let _ = candle_duration;

    // Build OHLCV overlay for the candle under the crosshair.
    let o = data.open(nearest_idx);
    let h = data.high(nearest_idx);
    let l = data.low(nearest_idx);
    let c = data.close(nearest_idx);
    let v = data.volume(nearest_idx);
    let is_bullish = c >= o;

    // Compute change from previous candle (if available).
    let (change, change_pct) = if nearest_idx > 0 {
        let prev_close = data.close(nearest_idx - 1);
        let chg = c - prev_close;
        let pct = if prev_close.abs() > f32::EPSILON {
            (chg / prev_close) * 100.0
        } else {
            0.0
        };
        (Some(chg), Some(pct))
    } else {
        (None, None)
    };

    let ohlcv_overlay = Some(OhlcvOverlay {
        symbol: symbol.to_string(),
        datetime: format_datetime_long(snap_ts as i64),
        open: o,
        high: h,
        low: l,
        close: c,
        volume: v,
        is_bullish,
        change,
        change_pct,
    });

    Some(CrosshairRender {
        vertical_x: snap_x,
        horizontal_y: snap_y,
        price_label,
        time_label,
        line_color: [0.7, 0.7, 0.7, 0.5],
        ohlcv_overlay,
    })
}

/// Format a timestamp as a long date/time string (e.g. "Fri 3/27/26 02:40:00 PM").
fn format_datetime_long(ts_ms: i64) -> String {
    use chrono::{DateTime, Utc, Datelike, Timelike};
    let dt: DateTime<Utc> = DateTime::from_timestamp_millis(ts_ms)
        .unwrap_or_else(|| DateTime::from_timestamp(0, 0).unwrap());
    let weekday = match dt.weekday() {
        chrono::Weekday::Mon => "Mon",
        chrono::Weekday::Tue => "Tue",
        chrono::Weekday::Wed => "Wed",
        chrono::Weekday::Thu => "Thu",
        chrono::Weekday::Fri => "Fri",
        chrono::Weekday::Sat => "Sat",
        chrono::Weekday::Sun => "Sun",
    };
    let hour = dt.hour();
    let (period, h12) = if hour == 0 {
        ("AM", 12)
    } else if hour < 12 {
        ("AM", hour)
    } else if hour == 12 {
        ("PM", 12)
    } else {
        ("PM", hour - 12)
    };
    format!(
        "{} {}/{}/{} {:02}:{:02}:{:02} {}",
        weekday,
        dt.month(),
        dt.day(),
        dt.year() % 100,
        h12,
        dt.minute(),
        dt.second(),
        period
    )
}

// ── Helper functions ─────────────────────────────────────────────────

/// Compute a "nice" step size for grid lines given a data range and desired
/// number of divisions.
///
/// Returns a step that is 1, 2, or 5 times a power of 10, producing
/// human-friendly grid intervals.
fn nice_step(range: f64, desired_divisions: f64) -> f64 {
    if desired_divisions <= 0.0 || range <= 0.0 {
        return 0.0;
    }
    let raw_step = range / desired_divisions;
    let magnitude = 10_f64.powf(raw_step.log10().floor());
    let normalized = raw_step / magnitude;

    let nice = if normalized <= 1.5 {
        1.0
    } else if normalized <= 3.5 {
        2.0
    } else if normalized <= 7.5 {
        5.0
    } else {
        10.0
    };
    nice * magnitude
}

/// Standard time intervals for grid lines (in milliseconds).
const TIME_STEPS_MS: &[f64] = &[
    1_000.0,              // 1 second
    5_000.0,              // 5 seconds
    10_000.0,             // 10 seconds
    30_000.0,             // 30 seconds
    60_000.0,             // 1 minute
    300_000.0,            // 5 minutes
    600_000.0,            // 10 minutes
    1_800_000.0,          // 30 minutes
    3_600_000.0,          // 1 hour
    14_400_000.0,         // 4 hours
    86_400_000.0,         // 1 day
    604_800_000.0,        // 1 week
    2_592_000_000.0,      // ~30 days (1 month)
    5_184_000_000.0,      // ~60 days (2 months)
    7_776_000_000.0,      // ~90 days (1 quarter)
    15_552_000_000.0,     // ~180 days (6 months)
    31_536_000_000.0,     // ~365 days (1 year)
    63_072_000_000.0,     // ~2 years
    157_680_000_000.0,    // ~5 years
];

/// Choose a time step from the standard intervals.
fn nice_time_step(time_range: f64, desired_divisions: f64) -> f64 {
    if desired_divisions <= 0.0 || time_range <= 0.0 {
        return 0.0;
    }
    let target = time_range / desired_divisions;
    // Find the first standard step >= target.
    for &step in TIME_STEPS_MS {
        if step >= target {
            return step;
        }
    }
    // Fall back: if target exceeds all predefined steps, use a nice
    // multiple of years (round up to nearest 1/2/5 × 10^N years).
    let year_ms = 31_536_000_000.0_f64;
    let years = target / year_ms;
    let nice_years = nice_step(years, 1.0).max(1.0);
    nice_years * year_ms
}

/// Returns `true` if this price is at a major grid step (every 5th step).
fn is_major_grid_step(price: f64, step: f64) -> bool {
    if step <= 0.0 {
        return false;
    }
    let n = (price / step).round() as i64;
    n % 5 == 0
}

/// Format a price value for display.
fn format_price(price: f64) -> String {
    if price.abs() >= 1000.0 {
        format!("{:.0}", price)
    } else if price.abs() >= 100.0 {
        format!("{:.1}", price)
    } else if price.abs() >= 1.0 {
        format!("{:.2}", price)
    } else {
        format!("{:.4}", price)
    }
}

/// Format an epoch-millisecond timestamp for display.
///
/// Produces a simple `HH:MM` or `MM-DD` format depending on the magnitude.
fn format_time_ms(ts_ms: i64) -> String {
    // Simple formatting without chrono dependency.
    // Convert epoch ms to seconds, then extract hours/minutes.
    let ts_sec = ts_ms / 1000;
    let seconds_in_day = ts_sec % 86400;
    let hours = seconds_in_day / 3600;
    let minutes = (seconds_in_day % 3600) / 60;

    if hours == 0 && minutes == 0 {
        // Midnight boundary -- show a date-like label.
        let days_since_epoch = ts_sec / 86400;
        format!("D{}", days_since_epoch)
    } else {
        format!("{:02}:{:02}", hours, minutes)
    }
}

// Date label types and computation moved to `crate::date_labels` module.
pub use crate::date_labels::DateLabel;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::Camera2D;
    use crate::dirty::DirtyFlags;
    use crate::input::ChartInput;
    use crate::levels::HorizontalLevel;
    use midas_core::CandleData;
    use std::ops::Range;

    // ── Test fixture ─────────────────────────────────────────────────

    struct TestCandles {
        timestamps: Vec<i64>,
        opens: Vec<f32>,
        highs: Vec<f32>,
        lows: Vec<f32>,
        closes: Vec<f32>,
        volumes: Vec<u32>,
    }

    impl TestCandles {
        /// 5 candles, 1-minute apart starting at epoch 1_000_000 ms.
        fn sample() -> Self {
            Self {
                timestamps: vec![1_000_000, 1_060_000, 1_120_000, 1_180_000, 1_240_000],
                opens: vec![100.0, 102.0, 101.0, 103.0, 105.0],
                highs: vec![104.0, 106.0, 105.0, 108.0, 110.0],
                lows: vec![98.0, 100.0, 99.0, 101.0, 103.0],
                closes: vec![102.0, 101.0, 103.0, 105.0, 108.0],
                volumes: vec![1000, 2000, 1500, 3000, 2500],
            }
        }

        fn empty() -> Self {
            Self {
                timestamps: vec![],
                opens: vec![],
                highs: vec![],
                lows: vec![],
                closes: vec![],
                volumes: vec![],
            }
        }
    }

    impl CandleData for TestCandles {
        fn len(&self) -> usize {
            self.timestamps.len()
        }
        fn timestamp(&self, idx: usize) -> i64 {
            self.timestamps[idx]
        }
        fn open(&self, idx: usize) -> f32 {
            self.opens[idx]
        }
        fn high(&self, idx: usize) -> f32 {
            self.highs[idx]
        }
        fn low(&self, idx: usize) -> f32 {
            self.lows[idx]
        }
        fn close(&self, idx: usize) -> f32 {
            self.closes[idx]
        }
        fn volume(&self, idx: usize) -> u32 {
            self.volumes[idx]
        }
        fn price_range(&self, range: Range<usize>) -> (f32, f32) {
            let mut min = f32::MAX;
            let mut max = f32::MIN;
            for i in range {
                min = min.min(self.lows[i]);
                max = max.max(self.highs[i]);
            }
            (min, max)
        }
        fn find_index_by_time(&self, ts: i64) -> usize {
            match self.timestamps.binary_search(&ts) {
                Ok(idx) => idx,
                Err(idx) => idx.min(self.len().saturating_sub(1)),
            }
        }
    }

    // ── Helper ───────────────────────────────────────────────────────

    fn make_camera_for_data(data: &TestCandles) -> Camera2D {
        if data.is_empty() {
            return Camera2D {
                time_start: 0.0,
                time_end: 1.0,
                price_low: 0.0,
                price_high: 1.0,
                viewport_width: 1920,
                viewport_height: 1080,
                dpi_scale: 1.0,
            };
        }
        let t0 = *data.timestamps.first().unwrap() as f64;
        let t1 = *data.timestamps.last().unwrap() as f64;
        // Add some padding to time range.
        let time_pad = (t1 - t0) * 0.1;
        let (pmin, pmax) = data.price_range(0..data.len());
        let price_pad = (pmax - pmin) as f64 * 0.1;
        Camera2D {
            time_start: t0 - time_pad,
            time_end: t1 + time_pad,
            price_low: pmin as f64 - price_pad,
            price_high: pmax as f64 + price_pad,
            viewport_width: 1920,
            viewport_height: 1080,
            dpi_scale: 1.0,
        }
    }

    /// Build a camera in index-space for collapsed mode.
    /// X axis = [−padding, len + padding] where `len` = number of candles.
    fn make_collapsed_camera_for_data(data: &TestCandles) -> Camera2D {
        let len = data.len() as f64;
        let pad = len * 0.1;
        let (pmin, pmax) = if data.is_empty() {
            (0.0_f32, 1.0_f32)
        } else {
            data.price_range(0..data.len())
        };
        let price_pad = (pmax - pmin) as f64 * 0.1;
        Camera2D {
            time_start: -pad,
            time_end: len + pad,
            price_low: pmin as f64 - price_pad,
            price_high: pmax as f64 + price_pad,
            viewport_width: 1920,
            viewport_height: 1080,
            dpi_scale: 1.0,
        }
    }

    fn make_input<'a>(
        data: &'a dyn CandleData,
        camera: &'a Camera2D,
        dirty: &'a DirtyFlags,
        levels: &'a [HorizontalLevel],
        crosshair: Option<(f32, f32)>,
    ) -> ChartInput<'a> {
        ChartInput {
            symbol: "TEST",
            data,
            camera,
            viewport_width: camera.viewport_width,
            viewport_height: camera.viewport_height,
            dpi_scale: camera.dpi_scale,
            background_color: [0.1, 0.1, 0.1, 1.0],
            bull_color: [0.0, 0.8, 0.0, 1.0],
            bear_color: [0.8, 0.0, 0.0, 1.0],
            volume_bull_color: [0.0, 0.5, 0.0, 0.3],
            volume_bear_color: [0.5, 0.0, 0.0, 0.3],
            grid_color: [0.3, 0.3, 0.3, 0.2],
            crosshair,
            levels,
            collapse_gaps: false,
            volume_scale: 1.0,
            dirty,
        }
    }

    /// Like `make_input` but with `collapse_gaps` set to a given value.
    fn make_input_with_collapse<'a>(
        data: &'a dyn CandleData,
        camera: &'a Camera2D,
        dirty: &'a DirtyFlags,
        levels: &'a [HorizontalLevel],
        crosshair: Option<(f32, f32)>,
        collapse_gaps: bool,
    ) -> ChartInput<'a> {
        ChartInput {
            symbol: "TEST",
            data,
            camera,
            viewport_width: camera.viewport_width,
            viewport_height: camera.viewport_height,
            dpi_scale: camera.dpi_scale,
            background_color: [0.1, 0.1, 0.1, 1.0],
            bull_color: [0.0, 0.8, 0.0, 1.0],
            bear_color: [0.8, 0.0, 0.0, 1.0],
            volume_bull_color: [0.0, 0.5, 0.0, 0.3],
            volume_bear_color: [0.5, 0.0, 0.0, 0.3],
            grid_color: [0.3, 0.3, 0.3, 0.2],
            crosshair,
            levels,
            collapse_gaps,
            volume_scale: 1.0,
            dirty,
        }
    }

    // ── Tests ────────────────────────────────────────────────────────

    #[test]
    fn empty_data_produces_empty_scene() {
        let data = TestCandles::empty();
        let camera = make_camera_for_data(&data);
        let dirty = DirtyFlags::new();
        let input = make_input(&data, &camera, &dirty, &[], None);

        let scene = compute_chart_scene(&input);

        assert!(scene.candles.is_none());
        assert_eq!(scene.candle_count, 0);
        assert!(scene.volumes.is_none());
        assert_eq!(scene.volume_count, 0);
        assert!(scene.crosshair.is_none());
    }

    #[test]
    fn all_candles_visible_count_matches() {
        let data = TestCandles::sample();
        let camera = make_camera_for_data(&data);
        let dirty = DirtyFlags::new();
        let input = make_input(&data, &camera, &dirty, &[], None);

        let scene = compute_chart_scene(&input);

        assert!(scene.candles.is_some());
        let candles = scene.candles.as_ref().unwrap();
        assert_eq!(candles.len(), 5, "all 5 candles should be visible");
        assert_eq!(scene.candle_count, 5);
    }

    #[test]
    fn candle_positions_are_pixel_snapped() {
        let data = TestCandles::sample();
        let mut camera = make_camera_for_data(&data);
        camera.dpi_scale = 2.0;
        let dirty = DirtyFlags::new();
        let input = make_input(&data, &camera, &dirty, &[], None);

        let scene = compute_chart_scene(&input);
        let candles = scene.candles.as_ref().unwrap();

        for (i, c) in candles.iter().enumerate() {
            // At 2x DPI, all coordinates should be on half-pixel boundaries.
            let x_phys = c.x * 2.0;
            assert!(
                (x_phys - x_phys.round()).abs() < 1e-3,
                "candle {} x={} not pixel-snapped at 2x DPI",
                i,
                c.x
            );
            let bt_phys = c.body_top * 2.0;
            assert!(
                (bt_phys - bt_phys.round()).abs() < 1e-3,
                "candle {} body_top={} not pixel-snapped at 2x DPI",
                i,
                c.body_top
            );
        }
    }

    #[test]
    fn candle_body_top_above_bottom() {
        let data = TestCandles::sample();
        let camera = make_camera_for_data(&data);
        let dirty = DirtyFlags::new();
        let input = make_input(&data, &camera, &dirty, &[], None);

        let scene = compute_chart_scene(&input);
        let candles = scene.candles.as_ref().unwrap();

        for (i, c) in candles.iter().enumerate() {
            // body_top should be <= body_bottom (Y increases downward).
            assert!(
                c.body_top <= c.body_bottom,
                "candle {}: body_top={} > body_bottom={}",
                i,
                c.body_top,
                c.body_bottom
            );
            // wick_top should be <= body_top (wick extends above body).
            assert!(
                c.wick_top <= c.body_top,
                "candle {}: wick_top={} > body_top={}",
                i,
                c.wick_top,
                c.body_top
            );
            // body_bottom should be <= wick_bottom (wick extends below body).
            assert!(
                c.body_bottom <= c.wick_bottom,
                "candle {}: body_bottom={} > wick_bottom={}",
                i,
                c.body_bottom,
                c.wick_bottom
            );
        }
    }

    #[test]
    fn bull_candles_get_bull_color() {
        let data = TestCandles::sample();
        let camera = make_camera_for_data(&data);
        let dirty = DirtyFlags::new();
        let bull_color = [0.0, 0.8, 0.0, 1.0];
        let bear_color = [0.8, 0.0, 0.0, 1.0];
        let input = make_input(&data, &camera, &dirty, &[], None);

        let scene = compute_chart_scene(&input);
        let candles = scene.candles.as_ref().unwrap();

        // Candle 0: open=100, close=102 -> bull.
        assert_eq!(candles[0].color, bull_color, "candle 0 should be bull");
        // Candle 1: open=102, close=101 -> bear.
        assert_eq!(candles[1].color, bear_color, "candle 1 should be bear");
        // Candle 2: open=101, close=103 -> bull.
        assert_eq!(candles[2].color, bull_color, "candle 2 should be bull");
    }

    #[test]
    fn volume_bars_are_in_bottom_20_percent() {
        let data = TestCandles::sample();
        let camera = make_camera_for_data(&data);
        let dirty = DirtyFlags::new();
        let input = make_input(&data, &camera, &dirty, &[], None);

        let scene = compute_chart_scene(&input);
        let volumes = scene.volumes.as_ref().unwrap();

        let vh = camera.viewport_height as f32;
        let volume_area_top = vh * (1.0 - VOLUME_AREA_FRACTION);

        for (i, v) in volumes.iter().enumerate() {
            assert!(
                v.y_top >= volume_area_top - 1.0,
                "volume {} y_top={} above volume area (top={})",
                i,
                v.y_top,
                volume_area_top
            );
            assert!(
                (v.y_bottom - vh).abs() < 1.0,
                "volume {} y_bottom={} should be at viewport bottom ({})",
                i,
                v.y_bottom,
                vh
            );
        }
    }

    #[test]
    fn volume_count_matches() {
        let data = TestCandles::sample();
        let camera = make_camera_for_data(&data);
        let dirty = DirtyFlags::new();
        let input = make_input(&data, &camera, &dirty, &[], None);

        let scene = compute_chart_scene(&input);

        assert!(scene.volumes.is_some());
        assert_eq!(scene.volume_count, 5);
    }

    #[test]
    fn crosshair_snaps_to_nearest_candle() {
        let data = TestCandles::sample();
        let camera = make_camera_for_data(&data);
        let dirty = DirtyFlags::new();

        // Place crosshair near the 3rd candle (index 2, timestamp 1_120_000).
        let target_x = camera.time_to_x(1_115_000.0);
        let target_y = 540.0;
        let input = make_input(&data, &camera, &dirty, &[], Some((target_x, target_y)));

        let scene = compute_chart_scene(&input);

        let ch = scene.crosshair.as_ref().expect("crosshair should be set");
        // The vertical line should snap to the actual candle center.
        let expected_x = camera.snap_to_pixel(camera.time_to_x(1_120_000.0));
        assert!(
            (ch.vertical_x - expected_x).abs() < 1.0,
            "crosshair vertical_x={} should be near expected={}",
            ch.vertical_x,
            expected_x
        );
    }

    #[test]
    fn crosshair_none_when_no_mouse() {
        let data = TestCandles::sample();
        let camera = make_camera_for_data(&data);
        let dirty = DirtyFlags::new();
        let input = make_input(&data, &camera, &dirty, &[], None);

        let scene = compute_chart_scene(&input);
        assert!(scene.crosshair.is_none());
    }

    #[test]
    fn crosshair_none_for_empty_data() {
        let data = TestCandles::empty();
        let camera = make_camera_for_data(&data);
        let dirty = DirtyFlags::new();
        let input = make_input(&data, &camera, &dirty, &[], Some((500.0, 300.0)));

        let scene = compute_chart_scene(&input);
        assert!(scene.crosshair.is_none());
    }

    #[test]
    fn grid_lines_are_produced() {
        let data = TestCandles::sample();
        let camera = make_camera_for_data(&data);
        let dirty = DirtyFlags::new();
        let input = make_input(&data, &camera, &dirty, &[], None);

        let scene = compute_chart_scene(&input);

        assert!(
            !scene.grid_lines.is_empty(),
            "should have at least one grid line"
        );
    }

    #[test]
    fn grid_lines_bounded_by_max() {
        let data = TestCandles::sample();
        let camera = make_camera_for_data(&data);
        let dirty = DirtyFlags::new();
        let input = make_input(&data, &camera, &dirty, &[], None);

        let scene = compute_chart_scene(&input);

        assert!(
            scene.grid_lines.len() <= MAX_GRID_LINES * 2,
            "grid lines {} exceeds max {}",
            scene.grid_lines.len(),
            MAX_GRID_LINES * 2
        );
    }

    #[test]
    fn y_labels_are_produced() {
        let data = TestCandles::sample();
        let camera = make_camera_for_data(&data);
        let dirty = DirtyFlags::new();
        let input = make_input(&data, &camera, &dirty, &[], None);

        let scene = compute_chart_scene(&input);

        assert!(
            !scene.y_labels.is_empty(),
            "should have at least one Y label"
        );
    }

    #[test]
    fn x_labels_are_produced() {
        let data = TestCandles::sample();
        let camera = make_camera_for_data(&data);
        let dirty = DirtyFlags::new();
        let input = make_input(&data, &camera, &dirty, &[], None);

        let scene = compute_chart_scene(&input);

        assert!(
            !scene.x_labels.is_empty(),
            "should have at least one X label"
        );
    }

    #[test]
    fn levels_rendered_at_correct_y() {
        let data = TestCandles::sample();
        let camera = make_camera_for_data(&data);
        let dirty = DirtyFlags::new();
        let levels = vec![HorizontalLevel {
            id: 1,
            price: 105.0,
            color: [1.0, 0.0, 0.0, 1.0],
            line_width: 1.0,
        }];
        let input = make_input(&data, &camera, &dirty, &levels, None);

        let scene = compute_chart_scene(&input);

        assert_eq!(scene.levels.len(), 1);
        let expected_y = camera.snap_to_pixel(camera.price_to_y(105.0));
        assert!(
            (scene.levels[0].screen_y - expected_y).abs() < 1.0,
            "level y={} should be near expected={}",
            scene.levels[0].screen_y,
            expected_y
        );
    }

    #[test]
    fn generations_match_dirty_flags() {
        let data = TestCandles::sample();
        let camera = make_camera_for_data(&data);
        let mut dirty = DirtyFlags::new();
        dirty.mark_camera();
        dirty.mark_data();
        dirty.mark_levels();
        let input = make_input(&data, &camera, &dirty, &[], None);

        let scene = compute_chart_scene(&input);

        assert_eq!(scene.generations.camera, dirty.camera);
        assert_eq!(scene.generations.candles, dirty.candles);
        assert_eq!(scene.generations.levels, dirty.levels);
        assert_eq!(scene.generations.crosshair, dirty.crosshair);
        assert_eq!(scene.generations.theme, dirty.theme);
    }

    #[test]
    fn projection_matrix_is_valid() {
        let data = TestCandles::sample();
        let camera = make_camera_for_data(&data);
        let dirty = DirtyFlags::new();
        let input = make_input(&data, &camera, &dirty, &[], None);

        let scene = compute_chart_scene(&input);

        // The projection should be the same as camera's own projection.
        let expected = camera.projection_matrix();
        assert_eq!(scene.projection, expected);
    }

    #[test]
    fn nice_step_produces_1_2_5_multiples() {
        // Range 100, ~10 divisions => step ~10 => nice 10.
        let step = nice_step(100.0, 10.0);
        assert!((step - 10.0).abs() < 1e-10, "step = {}", step);

        // Range 100, ~6 divisions => step ~16.7 => nice 20.
        let step = nice_step(100.0, 6.0);
        assert!((step - 20.0).abs() < 1e-10, "step = {}", step);

        // Range 50, ~10 divisions => step ~5 => nice 5.
        let step = nice_step(50.0, 10.0);
        assert!((step - 5.0).abs() < 1e-10, "step = {}", step);
    }

    #[test]
    fn format_price_various_magnitudes() {
        assert_eq!(format_price(1500.0), "1500");
        assert_eq!(format_price(150.0), "150.0");
        assert_eq!(format_price(15.50), "15.50");
        assert_eq!(format_price(0.1234), "0.1234");
    }

    #[test]
    fn candle_x_positions_increase_monotonically() {
        let data = TestCandles::sample();
        let camera = make_camera_for_data(&data);
        let dirty = DirtyFlags::new();
        let input = make_input(&data, &camera, &dirty, &[], None);

        let scene = compute_chart_scene(&input);
        let candles = scene.candles.as_ref().unwrap();

        for i in 1..candles.len() {
            assert!(
                candles[i].x > candles[i - 1].x,
                "candle {} x={} not > candle {} x={}",
                i,
                candles[i].x,
                i - 1,
                candles[i - 1].x,
            );
        }
    }

    #[test]
    fn viewport_dimensions_passed_through() {
        let data = TestCandles::sample();
        let camera = make_camera_for_data(&data);
        let dirty = DirtyFlags::new();
        let input = make_input(&data, &camera, &dirty, &[], None);

        let scene = compute_chart_scene(&input);

        assert_eq!(scene.viewport_width, 1920);
        assert_eq!(scene.viewport_height, 1080);
    }

    #[test]
    fn background_color_passed_through() {
        let data = TestCandles::sample();
        let camera = make_camera_for_data(&data);
        let dirty = DirtyFlags::new();
        let input = make_input(&data, &camera, &dirty, &[], None);

        let scene = compute_chart_scene(&input);

        assert_eq!(scene.background_color, [0.1, 0.1, 0.1, 1.0]);
    }

    // ── Gap-collapsing tests ────────────────────────────────────────

    /// 5 candles, 1-minute apart, with a large gap between candles 2 and 3.
    /// Candles at ts: 1_000_000, 1_060_000, 1_120_000, (gap of 16h), 1_057_720_000, 1_057_780_000.
    /// The gap between index 2 and 3 is 1_056_600_000 ms (~293.5 hours).
    fn sample_with_gap() -> TestCandles {
        TestCandles {
            timestamps: vec![
                1_000_000,
                1_060_000,
                1_120_000,
                // Large overnight-style gap:
                1_057_720_000,
                1_057_780_000,
            ],
            opens: vec![100.0, 102.0, 101.0, 103.0, 105.0],
            highs: vec![104.0, 106.0, 105.0, 108.0, 110.0],
            lows: vec![98.0, 100.0, 99.0, 101.0, 103.0],
            closes: vec![102.0, 101.0, 103.0, 105.0, 108.0],
            volumes: vec![1000, 2000, 1500, 3000, 2500],
        }
    }

    #[test]
    fn collapse_gaps_positions_by_index() {
        let data = sample_with_gap();
        let camera = make_collapsed_camera_for_data(&data);
        let dirty = DirtyFlags::new();
        let input = make_input_with_collapse(&data, &camera, &dirty, &[], None, true);

        let scene = compute_chart_scene(&input);

        assert!(scene.candles.is_some());
        let candles = scene.candles.as_ref().unwrap();
        assert_eq!(candles.len(), 5, "all 5 candles should be visible");

        // X positions should be evenly spaced (index-based).
        // The spacing between consecutive candles should be constant.
        let spacing_01 = candles[1].x - candles[0].x;
        let spacing_12 = candles[2].x - candles[1].x;
        let spacing_23 = candles[3].x - candles[2].x;
        let spacing_34 = candles[4].x - candles[3].x;

        // Due to pixel snapping, allow tolerance of 2 pixels.
        let tolerance = 2.0;
        assert!(
            (spacing_01 - spacing_12).abs() < tolerance,
            "spacing 0-1 ({}) vs 1-2 ({}): not evenly spaced",
            spacing_01,
            spacing_12
        );
        assert!(
            (spacing_12 - spacing_23).abs() < tolerance,
            "spacing 1-2 ({}) vs 2-3 ({}): not evenly spaced (gap should be collapsed)",
            spacing_12,
            spacing_23
        );
        assert!(
            (spacing_23 - spacing_34).abs() < tolerance,
            "spacing 2-3 ({}) vs 3-4 ({}): not evenly spaced",
            spacing_23,
            spacing_34
        );

        // X positions should be monotonically increasing.
        for i in 1..candles.len() {
            assert!(
                candles[i].x > candles[i - 1].x,
                "collapsed candle {} x={} not > candle {} x={}",
                i,
                candles[i].x,
                i - 1,
                candles[i - 1].x,
            );
        }
    }

    #[test]
    fn collapse_gaps_session_boundaries() {
        let data = sample_with_gap();
        let camera = make_collapsed_camera_for_data(&data);
        let dirty = DirtyFlags::new();
        let input = make_input_with_collapse(&data, &camera, &dirty, &[], None, true);

        let scene = compute_chart_scene(&input);

        // There should be exactly one session boundary between candle 2 and 3
        // (the large gap).
        assert_eq!(
            scene.session_boundaries.len(),
            1,
            "expected 1 session boundary, got {}",
            scene.session_boundaries.len()
        );

        let boundary = &scene.session_boundaries[0];
        let candles = scene.candles.as_ref().unwrap();

        // The boundary X should be between candle 2's X and candle 3's X.
        assert!(
            boundary.x > candles[2].x && boundary.x < candles[3].x,
            "session boundary x={} should be between candle 2 (x={}) and candle 3 (x={})",
            boundary.x,
            candles[2].x,
            candles[3].x,
        );

        // The boundary should have a valid color (non-zero alpha).
        assert!(
            boundary.color[3] > 0.0,
            "session boundary should have non-zero alpha"
        );
    }

    #[test]
    fn collapse_gaps_false_uses_time() {
        // With collapse_gaps = false, candle positions should be based on timestamps.
        // Using the data with a gap, candle 2 and candle 3 should be far apart.
        let data = sample_with_gap();
        let camera = make_camera_for_data(&data);
        let dirty = DirtyFlags::new();
        let input = make_input_with_collapse(&data, &camera, &dirty, &[], None, false);

        let scene = compute_chart_scene(&input);

        assert!(scene.candles.is_some());
        let candles = scene.candles.as_ref().unwrap();
        assert_eq!(candles.len(), 5);

        // The gap between candle 2 and candle 3 should be MUCH larger than
        // the gap between candle 0 and candle 1 (because of the time gap).
        let spacing_01 = candles[1].x - candles[0].x;
        let spacing_23 = candles[3].x - candles[2].x;

        assert!(
            spacing_23 > spacing_01 * 10.0,
            "in normal mode, gap 2-3 ({}) should be much larger than gap 0-1 ({})",
            spacing_23,
            spacing_01
        );

        // No session boundaries in normal mode.
        assert!(
            scene.session_boundaries.is_empty(),
            "normal mode should have no session boundaries"
        );
    }

    #[test]
    fn collapse_gaps_no_gaps_no_boundaries() {
        // With evenly-spaced candles (no gaps), there should be no session boundaries.
        let data = TestCandles::sample();
        let camera = make_collapsed_camera_for_data(&data);
        let dirty = DirtyFlags::new();
        let input = make_input_with_collapse(&data, &camera, &dirty, &[], None, true);

        let scene = compute_chart_scene(&input);

        assert!(
            scene.session_boundaries.is_empty(),
            "evenly-spaced candles should have no session boundaries, got {}",
            scene.session_boundaries.len()
        );
    }

    #[test]
    fn collapse_gaps_volumes_use_index_x() {
        let data = sample_with_gap();
        let camera = make_collapsed_camera_for_data(&data);
        let dirty = DirtyFlags::new();
        let input = make_input_with_collapse(&data, &camera, &dirty, &[], None, true);

        let scene = compute_chart_scene(&input);

        assert!(scene.volumes.is_some());
        let volumes = scene.volumes.as_ref().unwrap();
        let candles = scene.candles.as_ref().unwrap();

        // Each volume bar X should match its corresponding candle X.
        assert_eq!(volumes.len(), candles.len());
        for (i, (vol, candle)) in volumes.iter().zip(candles.iter()).enumerate() {
            assert!(
                (vol.x - candle.x).abs() < 1.0,
                "volume {} x={} should match candle x={}",
                i,
                vol.x,
                candle.x,
            );
        }

        // Volume bar X spacing should be evenly spaced.
        if volumes.len() >= 3 {
            let s01 = volumes[1].x - volumes[0].x;
            let s23 = volumes[3].x - volumes[2].x;
            assert!(
                (s01 - s23).abs() < 2.0,
                "volume spacing 0-1 ({}) vs 2-3 ({}) should be equal in collapsed mode",
                s01,
                s23
            );
        }
    }

    #[test]
    fn collapse_gaps_empty_data() {
        let data = TestCandles::empty();
        let camera = make_camera_for_data(&data);
        let dirty = DirtyFlags::new();
        let input = make_input_with_collapse(&data, &camera, &dirty, &[], None, true);

        let scene = compute_chart_scene(&input);

        assert!(scene.candles.is_none());
        assert_eq!(scene.candle_count, 0);
        assert!(scene.volumes.is_none());
        assert!(scene.session_boundaries.is_empty());
    }

    #[test]
    fn collapse_gaps_crosshair_snaps() {
        let data = sample_with_gap();
        let camera = make_collapsed_camera_for_data(&data);
        let dirty = DirtyFlags::new();

        // Place crosshair near the 3rd candle slot (index 2).
        // With index-space camera covering [-0.5, 5.5] over 1920px,
        // candle 2 center is at time_to_x(2.5).
        let cx = camera.time_to_x(2.5);
        let cy = 540.0;
        let input = make_input_with_collapse(&data, &camera, &dirty, &[], Some((cx, cy)), true);

        let scene = compute_chart_scene(&input);

        let ch = scene.crosshair.as_ref().expect("crosshair should be set");
        let candles = scene.candles.as_ref().unwrap();

        // The crosshair vertical_x should snap to one of the collapsed candle positions.
        let distances: Vec<f32> = candles.iter().map(|c| (c.x - ch.vertical_x).abs()).collect();
        let min_dist = distances.iter().cloned().fold(f32::INFINITY, f32::min);
        assert!(
            min_dist < 2.0,
            "crosshair vertical_x={} should snap to a candle position, min dist={}",
            ch.vertical_x,
            min_dist,
        );

        // OHLCV overlay should be present.
        assert!(ch.ohlcv_overlay.is_some(), "OHLCV overlay should exist");
    }
}
