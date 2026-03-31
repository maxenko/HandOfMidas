//! `compute_chart_scene()` -- pure function that transforms chart input into
//! a framework-agnostic [`ChartScene`].
//!
//! This is the heart of the chart component. It takes a [`ChartInput`] and
//! produces a [`ChartScene`] containing all the data needed to render a single
//! chart frame. No GPU, no framework -- just math.

use crate::camera::Camera2D;
use crate::date_labels::{DateLabel, Tier};
use crate::input::ChartInput;
use crate::instances::{
    AxisLabel, CandleInstance, CrosshairRender, GridLine, GridLineInstance, LevelRender,
    OhlcvOverlay, SessionBoundary, VolumeInstance,
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

/// Build Volume Profile GPU instances for the visible range, if enabled.
fn build_volume_profile(
    input: &ChartInput<'_>,
    data: &dyn CandleData,
    camera: &Camera2D,
    vis_start: usize,
    vis_end: usize,
) -> Vec<GridLineInstance> {
    if !input.show_volume_profile {
        return Vec::new();
    }
    let num_bins = ((input.viewport_height as f32 * 0.8) / 3.0)
        .min(200.0)
        .max(20.0) as usize;
    match crate::volume_profile::compute_volume_profile(
        data,
        vis_start,
        vis_end,
        camera.price_low as f32,
        camera.price_high as f32,
        num_bins,
    ) {
        Some(profile) => {
            crate::volume_profile::profile_to_instances(&profile, camera, input.viewport_width)
        }
        None => Vec::new(),
    }
}

/// Build ALL grid line GPU instances from the component pieces.
///
/// This is the single source of truth for grid rendering. It assembles:
/// 1. Horizontal price grid lines (clipped to price area above separator)
/// 2. Separator line (timeline border)
/// 3. Vertical time lines at every date label (opacity/thickness by tier)
/// 4. Session boundary lines (collapsed mode, deduplicated against date labels)
fn build_grid_instances(
    price_grid_lines: &[GridLine],
    date_labels: &[DateLabel],
    session_boundaries: &[SessionBoundary],
    separator_y: f32,
    viewport_width: f32,
    candle_count: usize,
) -> Vec<GridLineInstance> {
    let mut out = Vec::new();

    // 1. Horizontal price grid lines (above separator only).
    for gl in price_grid_lines {
        if gl.position < 0.0 || gl.position > separator_y {
            continue;
        }
        let (color, thickness) = if gl.is_major {
            ([0.40, 0.40, 0.45, 0.14], 1.0_f32)
        } else {
            ([0.30, 0.30, 0.35, 0.07], 0.5_f32)
        };
        out.push(GridLineInstance {
            rect: [0.0, gl.position, viewport_width, gl.position + thickness],
            color,
        });
    }

    // 2. Separator line (timeline border between price and volume areas).
    out.push(GridLineInstance {
        rect: [0.0, separator_y, viewport_width, separator_y + 1.0],
        color: [0.45, 0.45, 0.50, 0.35],
    });

    // 3. Vertical time lines at every date label position.
    //    Opacity and thickness scale with tier so hourly lines stay faint
    //    while daily/monthly boundaries stand out.
    for dl in date_labels {
        let (color, thickness) = if dl.is_boundary {
            ([0.45, 0.45, 0.50, 0.30], 1.0_f32)
        } else {
            match dl.tier {
                Tier::Month => ([0.40, 0.40, 0.45, 0.25], 1.0_f32),
                Tier::Day => ([0.35, 0.35, 0.40, 0.18], 1.0_f32),
                Tier::Hour => ([0.35, 0.35, 0.40, 0.12], 1.0_f32),
                Tier::Minute => ([0.30, 0.30, 0.35, 0.08], 1.0_f32),
            }
        };
        out.push(GridLineInstance {
            rect: [dl.screen_x, 0.0, dl.screen_x + thickness, separator_y],
            color,
        });
    }

    // 4. Session boundary lines (collapsed mode only).
    //    Skip boundaries that overlap with a date-label boundary line
    //    (both fire at day transitions, producing a double line).
    let slot_tol = if candle_count > 1 {
        viewport_width / candle_count as f32
    } else {
        40.0
    };
    for sb in session_boundaries {
        let dominated = date_labels
            .iter()
            .any(|dl| dl.is_boundary && (dl.screen_x - sb.x).abs() < slot_tol);
        if dominated {
            continue;
        }
        out.push(GridLineInstance {
            rect: [sb.x, 0.0, sb.x + 1.0, separator_y],
            color: sb.color,
        });
    }

    out
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

    let price_grid_lines = compute_grid_lines(camera, &input.grid_color);
    let y_labels = compute_y_labels(camera);
    let x_labels = compute_x_labels(camera, candle_duration);
    let levels = compute_levels(input.levels, camera);
    let crosshair = compute_crosshair_impl(input.crosshair, data, camera, input.symbol, &|cx| {
        let cursor_time = camera.x_to_time(cx);
        let idx = data.find_index_by_time(cursor_time as i64);
        let ts = data.timestamp(idx);
        let sx = camera.snap_to_pixel(camera.time_to_x(ts as f64));
        Some((sx, idx))
    });
    // Level placement preview: compute Y from snapped price, independent of crosshair.
    let level_preview_y = if input.level_tool.is_placing() {
        input
            .level_tool
            .preview_price
            .map(|p| camera.snap_to_pixel(camera.price_to_y(p)))
    } else {
        None
    };
    let date_labels = crate::date_labels::for_normal_mode(camera, candle_duration);
    let separator_y = input.viewport_height as f32 * (1.0 - input.timeline_border_ratio);
    let vw = input.viewport_width as f32;
    let projection = camera.projection_matrix();

    let grid_instances = build_grid_instances(
        &price_grid_lines,
        &date_labels,
        &[],
        separator_y,
        vw,
        candle_count,
    );
    let volume_profile_instances = build_volume_profile(input, data, camera, vis_start, vis_end);

    ChartScene {
        projection,
        viewport_width: input.viewport_width,
        viewport_height: input.viewport_height,
        background_color: input.background_color,
        candles,
        candle_count,
        volumes,
        volume_count,
        grid_instances,
        x_labels,
        y_labels,
        levels,
        crosshair,
        level_preview_y,
        separator_y,
        date_labels,
        volume_profile_instances,
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
    let x_from_idx = |i: usize| -> f32 { camera.snap_to_pixel(camera.time_to_x(i as f64 + 0.5)) };

    // Helper for functions that still take local-index closures (grid, labels, boundaries).
    let index_to_x = |local_idx: usize| -> f32 { x_from_idx(vis_start + local_idx) };

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
    let session_boundaries =
        detect_session_boundaries(data, vis_start, vis_end, candle_duration, &index_to_x);

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
    let crosshair = compute_crosshair_impl(input.crosshair, data, camera, input.symbol, &|cx| {
        if vis_end <= vis_start {
            return None;
        }
        let global_idx_f = camera.x_to_time(cx);
        let idx = (global_idx_f.round().max(0.0) as usize)
            .clamp(vis_start, vis_end - 1);
        let local_idx = idx - vis_start;
        let sx = index_to_x(local_idx);
        Some((sx, idx))
    });
    // Level placement preview: compute Y from snapped price, independent of crosshair.
    let level_preview_y = if input.level_tool.is_placing() {
        input
            .level_tool
            .preview_price
            .map(|p| camera.snap_to_pixel(camera.price_to_y(p)))
    } else {
        None
    };

    let date_labels = crate::date_labels::for_collapsed_mode(
        camera,
        data,
        vis_start,
        vis_end,
        candle_duration,
        &index_to_x,
    );
    let separator_y = input.viewport_height as f32 * (1.0 - input.timeline_border_ratio);
    let vw = input.viewport_width as f32;
    let projection = camera.projection_matrix();

    let grid_instances = build_grid_instances(
        &grid_lines,
        &date_labels,
        &session_boundaries,
        separator_y,
        vw,
        candle_count,
    );
    let volume_profile_instances = build_volume_profile(input, data, camera, vis_start, vis_end);

    ChartScene {
        projection,
        viewport_width: input.viewport_width,
        viewport_height: input.viewport_height,
        background_color: input.background_color,
        candles,
        candle_count,
        volumes,
        volume_count,
        grid_instances,
        x_labels,
        y_labels,
        levels,
        crosshair,
        level_preview_y,
        separator_y,
        date_labels,
        volume_profile_instances,
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

/// Compute crosshair render data.
///
/// The `snap_fn` closure converts a cursor X pixel coordinate into
/// `(snap_x, data_idx)` — the snapped X position and the candle index.
/// This abstracts the difference between normal (timestamp-space) and
/// collapsed (index-space) modes.
fn compute_crosshair_impl(
    crosshair: Option<(f32, f32)>,
    data: &dyn CandleData,
    camera: &Camera2D,
    symbol: &str,
    snap_fn: &dyn Fn(f32) -> Option<(f32, usize)>,
) -> Option<CrosshairRender> {
    let (cx, cy) = crosshair?;

    if data.is_empty() {
        return None;
    }

    let (snap_x, data_idx) = snap_fn(cx)?;
    let snap_y = camera.snap_to_pixel(cy);
    let snap_ts = data.timestamp(data_idx);

    Some(build_crosshair_data(
        data, camera, data_idx, snap_x, cy, snap_y, snap_ts, symbol,
    ))
}

/// Build the labels and OHLCV overlay for a crosshair at the given
/// candle index and snap positions.
///
/// Shared between normal and collapsed crosshair computation.
/// `cursor_y` is the raw (unsnapped) cursor Y used for the price label;
/// `snap_y` is the pixel-snapped Y used for the horizontal line position.
#[allow(clippy::too_many_arguments)]
fn build_crosshair_data(
    data: &dyn CandleData,
    camera: &Camera2D,
    data_idx: usize,
    snap_x: f32,
    cursor_y: f32,
    snap_y: f32,
    snap_ts: i64,
    symbol: &str,
) -> CrosshairRender {
    // Price label uses the raw cursor Y so the displayed price matches
    // the user's exact cursor position, not the pixel-snapped line.
    let cursor_price = camera.y_to_price(cursor_y);
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

    CrosshairRender {
        vertical_x: snap_x,
        horizontal_y: snap_y,
        price_label,
        time_label,
        line_color: [0.7, 0.7, 0.7, 0.5],
        ohlcv_overlay,
    }
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
///
/// Labels are placed at "nice" price intervals (1-2-5 multiples of powers
/// of 10) targeting roughly one label per 80 logical pixels of viewport
/// height. The labels include the formatted price string and their
/// screen-Y position.
pub fn compute_y_labels(camera: &Camera2D) -> Vec<AxisLabel> {
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

/// Label data for crosshair axis overlays (price on Y axis, time on X axis).
///
/// Produced by [`compute_crosshair_labels()`] and consumed by the iced
/// overlay builder in midas-app.
#[derive(Clone, Debug)]
pub struct CrosshairLabels {
    /// Price label: formatted price text, positioned at the right edge of
    /// the chart, vertically centered on the cursor Y.
    pub price_label: AxisLabel,
    /// Time label: formatted date/time text, positioned at the bottom of
    /// the price area, horizontally centered on the snapped cursor X.
    pub time_label: AxisLabel,
}

/// Compute crosshair axis label data for the iced widget overlay.
///
/// Returns `None` if the crosshair is not active (position is `None`) or
/// the data source is empty.
///
/// When `collapse_gaps` is true, `cursor_x` maps to a candle index via
/// the camera's index-space; otherwise it maps to a timestamp.
pub fn compute_crosshair_labels(
    cursor_pos: Option<(f32, f32)>,
    camera: &Camera2D,
    data: &dyn CandleData,
    collapse_gaps: bool,
) -> Option<CrosshairLabels> {
    let (cx, cy) = cursor_pos?;

    if data.is_empty() {
        return None;
    }

    // Price label uses the raw cursor Y so the displayed price matches
    // the user's exact cursor position, not a snapped value.
    let cursor_price = camera.y_to_price(cy);
    let snap_y = camera.snap_to_pixel(cy);
    let price_label = AxisLabel {
        text: format_price(cursor_price),
        screen_x: camera.viewport_width as f32,
        screen_y: snap_y,
        bg_color: [1.0, 1.0, 1.0, 0.95],
        text_color: [0.1, 0.1, 0.1, 1.0],
    };

    // Time label — snap to nearest candle, show detailed datetime.
    let (snap_x, snap_ts) = if collapse_gaps {
        // In collapsed mode, camera X axis is index-space.
        let global_idx_f = camera.x_to_time(cx);
        let idx = (global_idx_f.round().max(0.0) as usize).min(data.len().saturating_sub(1));
        let ts = data.timestamp(idx);
        let sx = camera.snap_to_pixel(camera.time_to_x(idx as f64));
        (sx, ts)
    } else {
        // Normal mode: camera X axis is timestamp-space.
        let cursor_time = camera.x_to_time(cx);
        let nearest_idx = data.find_index_by_time(cursor_time as i64);
        let ts = data.timestamp(nearest_idx);
        let sx = camera.snap_to_pixel(camera.time_to_x(ts as f64));
        (sx, ts)
    };

    let time_label = AxisLabel {
        text: format_datetime_long(snap_ts),
        screen_x: snap_x,
        screen_y: camera.viewport_height as f32,
        bg_color: [1.0, 1.0, 1.0, 0.95],
        text_color: [0.1, 0.1, 0.1, 1.0],
    };

    Some(CrosshairLabels {
        price_label,
        time_label,
    })
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
                id: lev.id,
                price: lev.price,
                screen_y: y,
                color: lev.color,
                line_width: lev.line_width,
                is_selected: false,
                is_being_dragged: false,
                original_screen_y: None,
                label_text: format_price(lev.price),
                label: lev.label.clone(),
                icon: lev.icon.clone(),
                locked: lev.locked,
            }
        })
        .collect()
}

/// Format a timestamp as a long date/time string (e.g. "Fri 3/27/26 02:40:00 PM").
pub fn format_datetime_long(ts_ms: i64) -> String {
    use chrono::{Datelike, Timelike};
    let utc = chrono::DateTime::from_timestamp_millis(ts_ms)
        .unwrap_or_else(|| chrono::DateTime::from_timestamp(0, 0).unwrap());
    let dt = utc.with_timezone(&chrono::Local);
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
    1_000.0,           // 1 second
    5_000.0,           // 5 seconds
    10_000.0,          // 10 seconds
    30_000.0,          // 30 seconds
    60_000.0,          // 1 minute
    300_000.0,         // 5 minutes
    600_000.0,         // 10 minutes
    1_800_000.0,       // 30 minutes
    3_600_000.0,       // 1 hour
    14_400_000.0,      // 4 hours
    86_400_000.0,      // 1 day
    604_800_000.0,     // 1 week
    2_592_000_000.0,   // ~30 days (1 month)
    5_184_000_000.0,   // ~60 days (2 months)
    7_776_000_000.0,   // ~90 days (1 quarter)
    15_552_000_000.0,  // ~180 days (6 months)
    31_536_000_000.0,  // ~365 days (1 year)
    63_072_000_000.0,  // ~2 years
    157_680_000_000.0, // ~5 years
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
///
/// Adapts decimal places to the magnitude: no decimals above $1000,
/// one decimal above $100, two below $100, four below $1.
pub fn format_price(price: f64) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::Camera2D;
    use crate::dirty::DirtyFlags;
    use crate::input::ChartInput;
    use crate::level_tool::LevelTool;
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

    /// Shared idle LevelTool for test helpers that don't need placement mode.
    static DEFAULT_LEVEL_TOOL: std::sync::LazyLock<LevelTool> =
        std::sync::LazyLock::new(LevelTool::default);

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
            timeline_border_ratio: 0.20,
            volume_scale: 1.0,
            show_volume_profile: false,
            level_tool: &DEFAULT_LEVEL_TOOL,
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
            timeline_border_ratio: 0.20,
            volume_scale: 1.0,
            show_volume_profile: false,
            level_tool: &DEFAULT_LEVEL_TOOL,
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
            !scene.grid_instances.is_empty(),
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
            scene.grid_instances.len() <= MAX_GRID_LINES * 2,
            "grid lines {} exceeds max {}",
            scene.grid_instances.len(),
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
            label: None,
            icon: crate::levels::LevelIcon::None,
            locked: false,
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
    fn collapse_gaps_produces_vertical_grid_lines() {
        let data = sample_with_gap();
        let camera = make_collapsed_camera_for_data(&data);
        let dirty = DirtyFlags::new();
        let input = make_input_with_collapse(&data, &camera, &dirty, &[], None, true);

        let scene = compute_chart_scene(&input);

        // Collapsed mode should produce vertical grid lines (from date labels
        // and/or session boundaries, deduplicated).
        let vertical_lines = scene
            .grid_instances
            .iter()
            .filter(|gl| (gl.rect[2] - gl.rect[0]) < 2.0 && (gl.rect[3] - gl.rect[1]) > 10.0)
            .count();
        assert!(
            vertical_lines > 0,
            "collapsed mode should produce vertical grid lines from date labels"
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

        // No session boundary grid lines in normal mode.
        let session_lines = scene.grid_instances.iter().any(|gl| {
            let is_vertical = (gl.rect[2] - gl.rect[0]) < 2.0 && (gl.rect[3] - gl.rect[1]) > 10.0;
            // SESSION_BOUNDARY_COLOR is [0.3, 0.3, 0.5, 0.30].
            // Date-label boundaries use [0.45, 0.45, 0.50, 0.30].
            // Distinguish by the R channel (0.3 vs 0.45).
            let is_session_color = (gl.color[0] - 0.3).abs() < 0.05
                && (gl.color[2] - 0.5).abs() < 0.1
                && gl.color[3] > 0.25;
            is_vertical && is_session_color
        });
        assert!(
            !session_lines,
            "normal mode should have no session boundary grid lines"
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

        // No session-colored grid lines when candles are evenly spaced.
        let session_lines = scene.grid_instances.iter().any(|gl| {
            let is_vertical = (gl.rect[2] - gl.rect[0]) < 2.0 && (gl.rect[3] - gl.rect[1]) > 10.0;
            let is_session_color = (gl.color[2] - 0.5).abs() < 0.1 && gl.color[3] > 0.25;
            is_vertical && is_session_color
        });
        assert!(
            !session_lines,
            "evenly-spaced candles should have no session boundary grid lines"
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
        // Grid should at least contain the separator line.
        assert!(!scene.grid_instances.is_empty());
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
        let distances: Vec<f32> = candles
            .iter()
            .map(|c| (c.x - ch.vertical_x).abs())
            .collect();
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

    // ── compute_crosshair_impl tests ────────────────────────────────

    #[test]
    fn crosshair_impl_returns_none_when_crosshair_is_none() {
        let data = TestCandles::sample();
        let camera = make_camera_for_data(&data);
        let snap_fn = |_cx: f32| -> Option<(f32, usize)> { Some((100.0, 0)) };

        let result = compute_crosshair_impl(None, &data, &camera, "TEST", &snap_fn);
        assert!(
            result.is_none(),
            "should return None when crosshair is None"
        );
    }

    #[test]
    fn crosshair_impl_returns_none_when_data_is_empty() {
        let data = TestCandles::empty();
        let camera = make_camera_for_data(&data);
        let snap_fn = |_cx: f32| -> Option<(f32, usize)> { Some((100.0, 0)) };

        let result = compute_crosshair_impl(Some((500.0, 300.0)), &data, &camera, "TEST", &snap_fn);
        assert!(result.is_none(), "should return None when data is empty");
    }

    #[test]
    fn crosshair_impl_normal_snap_to_nearest_candle_center() {
        let data = TestCandles::sample();
        let camera = make_camera_for_data(&data);

        // Snap function mimics normal mode: find nearest candle by timestamp.
        let snap_fn = |cx: f32| -> Option<(f32, usize)> {
            let cursor_time = camera.x_to_time(cx);
            let idx = data.find_index_by_time(cursor_time as i64);
            let ts = data.timestamp(idx);
            let sx = camera.snap_to_pixel(camera.time_to_x(ts as f64));
            Some((sx, idx))
        };

        // Place cursor near the 4th candle (index 3, timestamp 1_180_000).
        let cursor_x = camera.time_to_x(1_175_000.0);
        let cursor_y = 540.0;

        let result =
            compute_crosshair_impl(Some((cursor_x, cursor_y)), &data, &camera, "TEST", &snap_fn);
        let ch = result.expect("crosshair should be Some");

        // vertical_x should snap to candle 3's center.
        let expected_x = camera.snap_to_pixel(camera.time_to_x(1_180_000.0));
        assert!(
            (ch.vertical_x - expected_x).abs() < 1.0,
            "vertical_x={} should snap to candle center={}",
            ch.vertical_x,
            expected_x
        );
    }

    #[test]
    fn crosshair_impl_returns_correct_data_idx() {
        let data = TestCandles::sample();
        let camera = make_camera_for_data(&data);

        // Snap function that returns the index it resolves to.
        let snap_fn = |cx: f32| -> Option<(f32, usize)> {
            let cursor_time = camera.x_to_time(cx);
            let idx = data.find_index_by_time(cursor_time as i64);
            let ts = data.timestamp(idx);
            let sx = camera.snap_to_pixel(camera.time_to_x(ts as f64));
            Some((sx, idx))
        };

        // Place cursor near candle 2 (index 2, timestamp 1_120_000).
        let cursor_x = camera.time_to_x(1_115_000.0);
        let cursor_y = 540.0;

        let result =
            compute_crosshair_impl(Some((cursor_x, cursor_y)), &data, &camera, "TEST", &snap_fn);
        let ch = result.expect("crosshair should be Some");

        // The OHLCV overlay should reflect candle index 2's data.
        let overlay = ch
            .ohlcv_overlay
            .as_ref()
            .expect("OHLCV overlay should exist");
        assert!(
            (overlay.open - 101.0).abs() < f32::EPSILON,
            "open={} should match candle 2 open=101.0",
            overlay.open
        );
        assert!(
            (overlay.close - 103.0).abs() < f32::EPSILON,
            "close={} should match candle 2 close=103.0",
            overlay.close
        );
    }

    #[test]
    fn crosshair_impl_horizontal_y_matches_pixel_snapped_cursor() {
        let data = TestCandles::sample();
        let mut camera = make_camera_for_data(&data);
        camera.dpi_scale = 2.0;

        let snap_fn = |cx: f32| -> Option<(f32, usize)> {
            let cursor_time = camera.x_to_time(cx);
            let idx = data.find_index_by_time(cursor_time as i64);
            let ts = data.timestamp(idx);
            let sx = camera.snap_to_pixel(camera.time_to_x(ts as f64));
            Some((sx, idx))
        };

        let cursor_x = camera.time_to_x(1_060_000.0);
        let cursor_y = 543.3; // Non-integer Y to test snapping.

        let result =
            compute_crosshair_impl(Some((cursor_x, cursor_y)), &data, &camera, "TEST", &snap_fn);
        let ch = result.expect("crosshair should be Some");

        let expected_y = camera.snap_to_pixel(cursor_y);
        assert!(
            (ch.horizontal_y - expected_y).abs() < f32::EPSILON,
            "horizontal_y={} should equal snap_to_pixel(cursor_y)={}",
            ch.horizontal_y,
            expected_y
        );
    }

    // ── compute_crosshair_labels tests ──────────────────────────────

    #[test]
    fn crosshair_labels_returns_none_when_cursor_pos_is_none() {
        let data = TestCandles::sample();
        let camera = make_camera_for_data(&data);

        let result = compute_crosshair_labels(None, &camera, &data, false);
        assert!(
            result.is_none(),
            "should return None when cursor_pos is None"
        );
    }

    #[test]
    fn crosshair_labels_returns_none_when_data_is_empty() {
        let data = TestCandles::empty();
        let camera = make_camera_for_data(&data);

        let result = compute_crosshair_labels(Some((500.0, 300.0)), &camera, &data, false);
        assert!(result.is_none(), "should return None when data is empty");
    }

    #[test]
    fn crosshair_labels_price_text_matches_format_price() {
        let data = TestCandles::sample();
        let camera = make_camera_for_data(&data);

        let cursor_y = 540.0;
        let cursor_x = camera.time_to_x(1_060_000.0);

        let result = compute_crosshair_labels(Some((cursor_x, cursor_y)), &camera, &data, false);
        let labels = result.expect("labels should be Some");

        let expected_price = camera.y_to_price(cursor_y);
        let expected_text = format_price(expected_price);
        assert_eq!(
            labels.price_label.text, expected_text,
            "price label text should match format_price(camera.y_to_price(cursor_y))"
        );
    }

    #[test]
    fn crosshair_labels_time_snaps_to_nearest_candle_timestamp() {
        let data = TestCandles::sample();
        let camera = make_camera_for_data(&data);

        // Place cursor near candle 1 (timestamp 1_060_000).
        let cursor_x = camera.time_to_x(1_055_000.0);
        let cursor_y = 540.0;

        let result = compute_crosshair_labels(Some((cursor_x, cursor_y)), &camera, &data, false);
        let labels = result.expect("labels should be Some");

        // Time label text should use the nearest candle's timestamp.
        let expected_text = format_datetime_long(1_060_000);
        assert_eq!(
            labels.time_label.text, expected_text,
            "time label should snap to nearest candle timestamp"
        );
    }

    #[test]
    fn crosshair_labels_white_background_colors() {
        let data = TestCandles::sample();
        let camera = make_camera_for_data(&data);

        let cursor_x = camera.time_to_x(1_060_000.0);
        let cursor_y = 540.0;

        let result = compute_crosshair_labels(Some((cursor_x, cursor_y)), &camera, &data, false);
        let labels = result.expect("labels should be Some");

        let expected_bg = [1.0_f32, 1.0, 1.0, 0.95];
        assert_eq!(
            labels.price_label.bg_color, expected_bg,
            "price label bg_color should be white [1.0, 1.0, 1.0, 0.95]"
        );
        assert_eq!(
            labels.time_label.bg_color, expected_bg,
            "time label bg_color should be white [1.0, 1.0, 1.0, 0.95]"
        );
    }

    #[test]
    fn crosshair_labels_collapsed_gaps_produces_valid_labels() {
        let data = sample_with_gap();
        let camera = make_collapsed_camera_for_data(&data);

        // Place cursor near candle index 3 in collapsed mode.
        let cursor_x = camera.time_to_x(3.2);
        let cursor_y = 540.0;

        let result = compute_crosshair_labels(Some((cursor_x, cursor_y)), &camera, &data, true);
        let labels = result.expect("labels should be Some in collapsed mode");

        // Price label text should be non-empty and match format_price.
        let expected_price = camera.y_to_price(cursor_y);
        let expected_text = format_price(expected_price);
        assert_eq!(
            labels.price_label.text, expected_text,
            "collapsed mode price label should match format_price"
        );

        // Time label text should be non-empty (formatted datetime of
        // the nearest candle).
        assert!(
            !labels.time_label.text.is_empty(),
            "time label text should be non-empty in collapsed mode"
        );

        // Time label should snap to candle index 3's timestamp.
        let expected_time_text = format_datetime_long(data.timestamp(3));
        assert_eq!(
            labels.time_label.text, expected_time_text,
            "collapsed time label should snap to nearest candle's timestamp"
        );
    }

    #[test]
    fn crosshair_labels_price_screen_x_equals_viewport_width() {
        let data = TestCandles::sample();
        let camera = make_camera_for_data(&data);

        let cursor_x = camera.time_to_x(1_120_000.0);
        let cursor_y = 540.0;

        let result = compute_crosshair_labels(Some((cursor_x, cursor_y)), &camera, &data, false);
        let labels = result.expect("labels should be Some");

        let expected_x = camera.viewport_width as f32;
        assert!(
            (labels.price_label.screen_x - expected_x).abs() < f32::EPSILON,
            "price label screen_x={} should equal viewport_width={}",
            labels.price_label.screen_x,
            expected_x
        );
    }
}
