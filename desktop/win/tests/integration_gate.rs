// Phase 2.5 Integration Gate
// This test verifies the complete data pipeline:
// CSV file -> CandleBuffer -> .midas binary -> mmap read -> CandleBuffer -> compute_chart_scene -> verify
//
// Run with:  cargo test --test integration_gate
//
// Every assertion here must pass BEFORE interaction work (Phase 3) begins.

use std::path::Path;

use midas_chart::camera::Camera2D;
use midas_chart::compute::compute_chart_scene;
use midas_chart::dirty::DirtyFlags;
use midas_chart::input::ChartInput;
use midas_chart::instances::CandleInstance;
use midas_chart::levels::HorizontalLevel;
use midas_chart::scene::ChartScene;
use midas_data::binary::{write_midas_file, MmapCandleFile, MIDAS_MAGIC, MIDAS_VERSION};
use midas_data::candle::CandleBuffer;
use midas_data::lod::downsample_minmax;
use midas_feed::import_csv;

// ─── Default colors (dark theme) ────────────────────────────────────────

const BG_COLOR: [f32; 4] = [0.1, 0.1, 0.1, 1.0];
const BULL_COLOR: [f32; 4] = [0.0, 0.8, 0.0, 1.0];
const BEAR_COLOR: [f32; 4] = [0.8, 0.0, 0.0, 1.0];
const VOLUME_BULL_COLOR: [f32; 4] = [0.0, 0.5, 0.0, 0.3];
const VOLUME_BEAR_COLOR: [f32; 4] = [0.5, 0.0, 0.0, 0.3];
const GRID_COLOR: [f32; 4] = [0.3, 0.3, 0.3, 0.2];

// ─── Helper: build a ChartInput from defaults ───────────────────────────

/// Construct a [`ChartInput`] from a [`CandleBuffer`] with sensible defaults.
///
/// Reusable across integration tests. Creates a [`Camera2D`] that covers the
/// full data range with 10 % padding on each side, a 1280x800 viewport, and
/// dark-theme colors.
fn make_default_chart_input<'a>(
    data: &'a CandleBuffer,
    camera: &'a Camera2D,
    dirty: &'a DirtyFlags,
    levels: &'a [HorizontalLevel],
) -> ChartInput<'a> {
    ChartInput {
        symbol: "TEST",
        data,
        camera,
        viewport_width: camera.viewport_width,
        viewport_height: camera.viewport_height,
        dpi_scale: camera.dpi_scale,
        background_color: BG_COLOR,
        bull_color: BULL_COLOR,
        bear_color: BEAR_COLOR,
        volume_bull_color: VOLUME_BULL_COLOR,
        volume_bear_color: VOLUME_BEAR_COLOR,
        grid_color: GRID_COLOR,
        crosshair: None,
        levels,
        collapse_gaps: false,
        timeline_border_ratio: 0.20,
        volume_scale: 1.0,
        show_volume_profile: false,
        dirty,
        placing_level: false,
        placing_alt_held: false,
    }
}

/// Build a Camera2D that covers the full data range of a CandleBuffer with
/// 10 % time padding and 10 % price padding.
fn camera_for_buffer(buf: &CandleBuffer) -> Camera2D {
    assert!(!buf.is_empty(), "cannot build camera for empty buffer");

    let t0 = *buf.timestamps.first().unwrap() as f64;
    let t1 = *buf.timestamps.last().unwrap() as f64;
    let time_pad = (t1 - t0) * 0.1;

    let (min_low, max_high) = buf.price_range(0..buf.len());
    let price_pad = (max_high - min_low) as f64 * 0.1;

    Camera2D {
        time_start: t0 - time_pad,
        time_end: t1 + time_pad,
        price_low: min_low as f64 - price_pad,
        price_high: max_high as f64 + price_pad,
        viewport_width: 1280,
        viewport_height: 800,
        dpi_scale: 1.0,
    }
}

// ─── 1. CSV import ──────────────────────────────────────────────────────

#[test]
fn step1_csv_import() {
    let csv_path = Path::new("crates/midas-feed/tests/data/aapl_daily_sample.csv");
    let buf = import_csv(csv_path).expect("import_csv should succeed");

    // The sample CSV has 20 data rows.
    assert_eq!(buf.len(), 20, "sample CSV should contain 20 candles");

    // Timestamps must be monotonically increasing.
    for i in 1..buf.len() {
        assert!(
            buf.timestamps[i] > buf.timestamps[i - 1],
            "timestamps not sorted at index {i}"
        );
    }

    // Sanity: prices should be positive and in a reasonable range for AAPL.
    for i in 0..buf.len() {
        assert!(buf.opens[i] > 100.0 && buf.opens[i] < 300.0);
        assert!(buf.highs[i] >= buf.opens[i] || buf.highs[i] >= buf.closes[i]);
        assert!(buf.lows[i] <= buf.opens[i] || buf.lows[i] <= buf.closes[i]);
        assert!(buf.volumes[i] > 0);
    }
}

// ─── 2. Write to .midas binary ─────────────────────────────────────────

#[test]
fn step2_write_midas_binary() {
    let csv_path = Path::new("crates/midas-feed/tests/data/aapl_daily_sample.csv");
    let buf = import_csv(csv_path).unwrap();

    let dir = tempfile::tempdir().expect("create tempdir");
    let midas_path = dir.path().join("aapl_daily.midas");

    write_midas_file(&midas_path, 1, 86400, "AAPL", &buf).expect("write_midas_file should succeed");

    assert!(midas_path.exists(), ".midas file was not created");

    let metadata = std::fs::metadata(&midas_path).unwrap();
    assert!(metadata.len() > 0, ".midas file should not be empty");
}

// ─── 3. Read back via mmap and verify round-trip ────────────────────────

#[test]
fn step3_mmap_roundtrip() {
    let csv_path = Path::new("crates/midas-feed/tests/data/aapl_daily_sample.csv");
    let original = import_csv(csv_path).unwrap();

    let dir = tempfile::tempdir().unwrap();
    let midas_path = dir.path().join("aapl_daily.midas");
    write_midas_file(&midas_path, 42, 86400, "AAPL", &original).unwrap();

    // Open via memory-mapped I/O.
    let mmap = MmapCandleFile::open(&midas_path).expect("MmapCandleFile::open should succeed");

    // Verify header fields.
    let header = mmap.header();
    assert_eq!(header.magic, MIDAS_MAGIC, "magic mismatch");
    assert_eq!(header.version, MIDAS_VERSION, "version mismatch");
    assert_eq!(
        header.candle_count,
        original.len() as u64,
        "candle_count mismatch"
    );
    assert_eq!(header.symbol_id, 42, "symbol_id mismatch");
    assert_eq!(header.timeframe_secs, 86400, "timeframe_secs mismatch");
    assert_eq!(header.start_ts, original.timestamps[0], "start_ts mismatch");
    assert_eq!(
        header.end_ts,
        *original.timestamps.last().unwrap(),
        "end_ts mismatch"
    );
    assert_eq!(&header.symbol_ascii[..4], b"AAPL", "symbol_ascii mismatch");

    // Convert mmap back to CandleBuffer and verify exact round-trip.
    let loaded = mmap.to_candle_buffer();
    assert_eq!(
        loaded.len(),
        original.len(),
        "candle count mismatch after mmap round-trip"
    );

    assert_eq!(
        loaded.timestamps, original.timestamps,
        "timestamps mismatch"
    );
    assert_eq!(loaded.opens, original.opens, "opens mismatch");
    assert_eq!(loaded.highs, original.highs, "highs mismatch");
    assert_eq!(loaded.lows, original.lows, "lows mismatch");
    assert_eq!(loaded.closes, original.closes, "closes mismatch");
    assert_eq!(loaded.volumes, original.volumes, "volumes mismatch");
}

// ─── 4. Compute ChartScene ─────────────────────────────────────────────

#[test]
fn step4_compute_chart_scene() {
    let csv_path = Path::new("crates/midas-feed/tests/data/aapl_daily_sample.csv");
    let buf = import_csv(csv_path).unwrap();

    let camera = camera_for_buffer(&buf);
    let dirty = DirtyFlags::new();
    let input = make_default_chart_input(&buf, &camera, &dirty, &[]);

    let scene: ChartScene = compute_chart_scene(&input);

    // ── Candle instances exist ──────────────────────────────────────
    assert!(
        scene.candles.is_some(),
        "scene should contain candle instances"
    );
    let candles: &Vec<CandleInstance> = scene.candles.as_ref().unwrap();
    assert!(!candles.is_empty(), "candle instance count must be > 0");
    assert_eq!(
        candles.len(),
        20,
        "all 20 candles should be visible with a full-range camera"
    );

    // ── body_top <= body_bottom (Y axis inverted) ───────────────────
    for (i, c) in candles.iter().enumerate() {
        assert!(
            c.body_top <= c.body_bottom,
            "candle {i}: body_top ({}) > body_bottom ({})",
            c.body_top,
            c.body_bottom,
        );
    }

    // ── X positions monotonically increasing ────────────────────────
    for i in 1..candles.len() {
        assert!(
            candles[i].x > candles[i - 1].x,
            "candle {i}: x ({}) not > candle {} x ({})",
            candles[i].x,
            i - 1,
            candles[i - 1].x,
        );
    }

    // ── Candle colors are either bull or bear ───────────────────────
    for (i, c) in candles.iter().enumerate() {
        assert!(
            c.color == BULL_COLOR || c.color == BEAR_COLOR,
            "candle {i}: unexpected color {:?}",
            c.color,
        );
    }

    // ── Volume instances exist ──────────────────────────────────────
    assert!(
        scene.volumes.is_some(),
        "scene should contain volume instances"
    );
    let volumes = scene.volumes.as_ref().unwrap();
    assert!(!volumes.is_empty(), "volume instance count must be > 0");

    // ── Grid lines exist ────────────────────────────────────────────
    assert!(
        !scene.grid_instances.is_empty(),
        "scene should contain grid lines"
    );

    // ── Projection matrix is not identity ───────────────────────────
    let identity = glam::Mat4::IDENTITY;
    assert_ne!(
        scene.projection, identity,
        "projection matrix should not be identity"
    );

    // ── Projection matches camera's own projection ──────────────────
    let expected_proj = camera.projection_matrix();
    assert_eq!(
        scene.projection, expected_proj,
        "projection should match camera.projection_matrix()"
    );

    // ── Viewport dimensions passed through correctly ────────────────
    assert_eq!(scene.viewport_width, 1280);
    assert_eq!(scene.viewport_height, 800);
}

// ─── 5. LOD downsample ─────────────────────────────────────────────────

#[test]
fn step5_lod_downsample() {
    let csv_path = Path::new("crates/midas-feed/tests/data/aapl_daily_sample.csv");
    let original = import_csv(csv_path).unwrap();

    let downsampled = downsample_minmax(&original, 10);
    assert_eq!(
        downsampled.len(),
        10,
        "downsampled buffer should have 10 candles"
    );

    // Price envelope must be preserved exactly.
    let orig_min_low = original.lows.iter().copied().fold(f32::INFINITY, f32::min);
    let orig_max_high = original
        .highs
        .iter()
        .copied()
        .fold(f32::NEG_INFINITY, f32::max);

    let ds_min_low = downsampled
        .lows
        .iter()
        .copied()
        .fold(f32::INFINITY, f32::min);
    let ds_max_high = downsampled
        .highs
        .iter()
        .copied()
        .fold(f32::NEG_INFINITY, f32::max);

    assert_eq!(
        orig_min_low, ds_min_low,
        "min low must be preserved after downsample"
    );
    assert_eq!(
        orig_max_high, ds_max_high,
        "max high must be preserved after downsample"
    );

    // Timestamps must remain monotonically increasing.
    for i in 1..downsampled.len() {
        assert!(
            downsampled.timestamps[i] > downsampled.timestamps[i - 1],
            "downsampled timestamps not sorted at index {i}"
        );
    }

    // First and last boundaries.
    assert_eq!(
        downsampled.timestamps[0], original.timestamps[0],
        "first timestamp must match"
    );
    assert_eq!(
        downsampled.opens[0], original.opens[0],
        "first open must match"
    );
    assert_eq!(
        *downsampled.closes.last().unwrap(),
        *original.closes.last().unwrap(),
        "last close must match"
    );
}

// ─── Full pipeline end-to-end in one shot ───────────────────────────────

#[test]
fn full_pipeline_end_to_end() {
    // 1. CSV -> CandleBuffer
    let csv_path = Path::new("crates/midas-feed/tests/data/aapl_daily_sample.csv");
    let original = import_csv(csv_path).expect("CSV import failed");
    assert_eq!(original.len(), 20);

    // 2. CandleBuffer -> .midas binary
    let dir = tempfile::tempdir().unwrap();
    let midas_path = dir.path().join("aapl_gate.midas");
    write_midas_file(&midas_path, 1, 86400, "AAPL", &original).expect("write_midas_file failed");
    assert!(midas_path.exists());

    // 3. .midas binary -> mmap -> CandleBuffer
    let mmap = MmapCandleFile::open(&midas_path).expect("mmap open failed");
    let loaded = mmap.to_candle_buffer();
    assert_eq!(loaded.len(), original.len());
    assert_eq!(loaded.timestamps, original.timestamps);
    assert_eq!(loaded.opens, original.opens);
    assert_eq!(loaded.highs, original.highs);
    assert_eq!(loaded.lows, original.lows);
    assert_eq!(loaded.closes, original.closes);
    assert_eq!(loaded.volumes, original.volumes);

    // 4. CandleBuffer -> ChartScene
    let camera = camera_for_buffer(&loaded);
    let mut dirty = DirtyFlags::new();
    dirty.mark_all();
    let input = make_default_chart_input(&loaded, &camera, &dirty, &[]);
    let scene = compute_chart_scene(&input);

    // Verify the scene is fully populated.
    let candles = scene.candles.as_ref().expect("candle instances missing");
    assert_eq!(candles.len(), 20, "all candles visible");

    let volumes = scene.volumes.as_ref().expect("volume instances missing");
    assert_eq!(volumes.len(), 20, "all volume bars present");

    assert!(!scene.grid_instances.is_empty(), "grid lines present");
    assert!(!scene.y_labels.is_empty(), "y labels present");
    assert!(!scene.x_labels.is_empty(), "x labels present");

    assert_ne!(scene.projection, glam::Mat4::IDENTITY);

    // 5. LOD round-trip
    let downsampled = downsample_minmax(&loaded, 10);
    assert_eq!(downsampled.len(), 10);

    let (orig_lo, orig_hi) = original.price_range(0..original.len());
    let (ds_lo, ds_hi) = downsampled.price_range(0..downsampled.len());
    assert_eq!(orig_lo, ds_lo, "LOD min low preserved");
    assert_eq!(orig_hi, ds_hi, "LOD max high preserved");

    // The downsampled data should also produce a valid scene.
    let ds_camera = camera_for_buffer(&downsampled);
    let ds_input = make_default_chart_input(&downsampled, &ds_camera, &dirty, &[]);
    let ds_scene = compute_chart_scene(&ds_input);
    let ds_candles = ds_scene
        .candles
        .as_ref()
        .expect("LOD scene missing candles");
    assert_eq!(ds_candles.len(), 10, "LOD scene should have 10 candles");
}
