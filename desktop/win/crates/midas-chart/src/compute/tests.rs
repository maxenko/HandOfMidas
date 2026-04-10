use super::*;
use crate::camera::Camera2D;
use crate::dirty::DirtyFlags;
use crate::input::ChartInput;
use crate::level_tool::LevelTool;
use crate::widget::{
    level::{LevelExtend, LineStyle},
    Annotation, AnnotationId, AnnotationKind, Presence,
};
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
    annotations: &'a [Annotation],
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
        annotations,
        collapse_gaps: false,
        timeline_border_ratio: 0.20,
        volume_scale: 1.0,
        show_volume_profile: false,
        level_tool: &DEFAULT_LEVEL_TOOL,
        dirty,
        gatr_bright_ranges: &[],
        hovered_annotation: None,
        selected_annotation: None,
        drag_ghost: None,
    }
}

/// Like `make_input` but with `collapse_gaps` set to a given value.
fn make_input_with_collapse<'a>(
    data: &'a dyn CandleData,
    camera: &'a Camera2D,
    dirty: &'a DirtyFlags,
    annotations: &'a [Annotation],
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
        annotations,
        collapse_gaps,
        timeline_border_ratio: 0.20,
        volume_scale: 1.0,
        show_volume_profile: false,
        level_tool: &DEFAULT_LEVEL_TOOL,
        dirty,
        gatr_bright_ranges: &[],
        hovered_annotation: None,
        selected_annotation: None,
        drag_ghost: None,
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
fn priceline_labels_are_produced() {
    let data = TestCandles::sample();
    let camera = make_camera_for_data(&data);
    let dirty = DirtyFlags::new();
    let input = make_input(&data, &camera, &dirty, &[], None);

    let scene = compute_chart_scene(&input);

    assert!(
        !scene.priceline_labels.is_empty(),
        "should have at least one priceline label"
    );
}

#[test]
fn timeline_ticks_are_produced() {
    let data = TestCandles::sample();
    let camera = make_camera_for_data(&data);
    let dirty = DirtyFlags::new();
    let input = make_input(&data, &camera, &dirty, &[], None);

    let scene = compute_chart_scene(&input);

    assert!(
        !scene.timeline_ticks.is_empty(),
        "should have at least one timeline tick"
    );
}

#[test]
fn levels_rendered_via_widget_output() {
    let data = TestCandles::sample();
    let camera = make_camera_for_data(&data);
    let dirty = DirtyFlags::new();
    let annotations = vec![Annotation {
        id: AnnotationId(1),
        kind: AnnotationKind::Level(crate::widget::HorizontalLevel {
            price: 105.0,
            color: [1.0, 0.0, 0.0, 1.0],
            line_width: 1.0,
            style: LineStyle::default(),
            label: None,
            extend: LevelExtend::default(),
            icon: crate::levels::LevelIcon::None,
        }),
        presence: Presence::Active,
        visible_timeframes: None,
        locked: false,
        created_at: 0,
        modified_at: 0,
    }];
    let input = make_input(&data, &camera, &dirty, &annotations, None);

    let scene = compute_chart_scene(&input);

    // Levels now render via widget_output (not scene.levels which is deprecated).
    assert!(
        !scene.widget_output.lines.is_empty(),
        "widget_output should have level lines"
    );
    assert!(
        !scene.widget_output.hit_zones.is_empty(),
        "widget_output should have level hit zones"
    );
    assert!(
        !scene.widget_output.labels.is_empty(),
        "widget_output should have level labels"
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
        labels.priceline_lens.text, expected_text,
        "price lens text should match format_price(camera.y_to_price(cursor_y))"
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
        labels.timeline_lens.text, expected_text,
        "timeline label should snap to nearest candle timestamp"
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
        labels.priceline_lens.bg_color, expected_bg,
        "price lens bg_color should be white [1.0, 1.0, 1.0, 0.95]"
    );
    assert_eq!(
        labels.timeline_lens.bg_color, expected_bg,
        "timeline label bg_color should be white [1.0, 1.0, 1.0, 0.95]"
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
        labels.priceline_lens.text, expected_text,
        "collapsed mode price lens should match format_price"
    );

    // Time label text should be non-empty (formatted datetime of
    // the nearest candle).
    assert!(
        !labels.timeline_lens.text.is_empty(),
        "timeline label text should be non-empty in collapsed mode"
    );

    // Time label should snap to candle index 3's timestamp.
    let expected_time_text = format_datetime_long(data.timestamp(3));
    assert_eq!(
        labels.timeline_lens.text, expected_time_text,
        "collapsed timeline label should snap to nearest candle's timestamp"
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
        (labels.priceline_lens.screen_x - expected_x).abs() < f32::EPSILON,
        "price lens screen_x={} should equal viewport_width={}",
        labels.priceline_lens.screen_x,
        expected_x
    );
}
