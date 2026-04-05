use super::*;

/// Helper to create a standard test camera.
fn test_camera() -> Camera2D {
    Camera2D {
        time_start: 1_700_000_000_000.0, // ~Nov 2023
        time_end: 1_700_100_000_000.0,   // ~27.8 hours later
        price_low: 150.0,
        price_high: 200.0,
        viewport_width: 1920,
        viewport_height: 1080,
        dpi_scale: 1.0,
    }
}

#[test]
fn time_to_x_at_boundaries() {
    let cam = test_camera();
    let x_start = cam.time_to_x(cam.time_start);
    let x_end = cam.time_to_x(cam.time_end);
    assert!((x_start - 0.0).abs() < 1e-3, "x_start = {x_start}");
    assert!(
        (x_end - cam.viewport_width as f32).abs() < 1e-3,
        "x_end = {x_end}"
    );
}

#[test]
fn price_to_y_at_boundaries() {
    let cam = test_camera();
    // High price -> top of screen -> Y near 0.
    let y_high = cam.price_to_y(cam.price_high);
    // Low price -> bottom of screen -> Y near viewport_height.
    let y_low = cam.price_to_y(cam.price_low);
    assert!((y_high - 0.0).abs() < 1e-3, "y_high = {y_high}");
    assert!(
        (y_low - cam.viewport_height as f32).abs() < 1e-3,
        "y_low = {y_low}"
    );
}

#[test]
fn time_round_trip() {
    let cam = test_camera();
    let original_time = 1_700_050_000_000.0; // midpoint
    let x = cam.time_to_x(original_time);
    let recovered_time = cam.x_to_time(x);

    // Should be accurate to within 1 pixel worth of time.
    let time_per_pixel = cam.visible_time_span() / cam.viewport_width as f64;
    let error = (recovered_time - original_time).abs();
    assert!(
        error < time_per_pixel,
        "Round-trip error {error} exceeds 1 pixel ({time_per_pixel})"
    );
}

#[test]
fn price_round_trip() {
    let cam = test_camera();
    let original_price = 175.50;
    let y = cam.price_to_y(original_price);
    let recovered_price = cam.y_to_price(y);

    let price_per_pixel = (cam.price_high - cam.price_low) / cam.viewport_height as f64;
    let error = (recovered_price - original_price).abs();
    assert!(
        error < price_per_pixel,
        "Round-trip error {error} exceeds 1 pixel ({price_per_pixel})"
    );
}

#[test]
fn projection_matrix_is_valid_orthographic() {
    let cam = test_camera();
    let proj = cam.projection_matrix();

    // The projection matrix should map:
    //   (0, 0, 0) -> (-1, +1, 0) in NDC (top-left corner)
    //   (w, h, 0) -> (+1, -1, 0) in NDC (bottom-right corner)
    let top_left = proj * glam::Vec4::new(0.0, 0.0, 0.0, 1.0);
    let bottom_right = proj
        * glam::Vec4::new(
            cam.viewport_width as f32,
            cam.viewport_height as f32,
            0.0,
            1.0,
        );

    assert!(
        (top_left.x - (-1.0)).abs() < 1e-5,
        "top_left.x = {}",
        top_left.x
    );
    assert!(
        (top_left.y - 1.0).abs() < 1e-5,
        "top_left.y = {}",
        top_left.y
    );

    assert!(
        (bottom_right.x - 1.0).abs() < 1e-5,
        "bottom_right.x = {}",
        bottom_right.x
    );
    assert!(
        (bottom_right.y - (-1.0)).abs() < 1e-5,
        "bottom_right.y = {}",
        bottom_right.y
    );
}

#[test]
fn projection_matrix_center_maps_to_origin() {
    let cam = test_camera();
    let proj = cam.projection_matrix();

    let center = proj
        * glam::Vec4::new(
            cam.viewport_width as f32 / 2.0,
            cam.viewport_height as f32 / 2.0,
            0.0,
            1.0,
        );

    assert!(center.x.abs() < 1e-5, "center.x = {}", center.x);
    assert!(center.y.abs() < 1e-5, "center.y = {}", center.y);
}

#[test]
fn pixels_per_candle_basic() {
    let cam = test_camera();
    // Visible range is 100_000_000 ms. With 1920px width and
    // 5-minute candles (300_000 ms):
    let ppc = cam.pixels_per_candle(300_000.0);
    let expected = 300_000.0 / 100_000_000.0 * 1920.0;
    assert!(
        (ppc - expected as f32).abs() < 1e-3,
        "ppc = {ppc}, expected = {expected}"
    );
}

#[test]
fn visible_time_span_basic() {
    let cam = test_camera();
    let span = cam.visible_time_span();
    assert!((span - 100_000_000.0).abs() < 1e-3, "span = {span}");
}

#[test]
fn snap_to_pixel_at_1x_dpi() {
    let cam = test_camera(); // dpi_scale = 1.0
                             // At 1x DPI, snap_to_pixel should floor to integer pixels.
    assert!((cam.snap_to_pixel(10.3) - 10.0).abs() < 1e-5);
    assert!((cam.snap_to_pixel(10.7) - 10.0).abs() < 1e-5);
    assert!((cam.snap_to_pixel(10.0) - 10.0).abs() < 1e-5);
}

#[test]
fn snap_to_pixel_at_1_5x_dpi() {
    let mut cam = test_camera();
    cam.dpi_scale = 1.5;

    // At 1.5x DPI, physical pixels are at 0, 0.667, 1.333, 2.0, ...
    // snap_to_pixel(1.0) => floor(1.0 * 1.5) / 1.5 = floor(1.5) / 1.5
    //                     = 1.0 / 1.5 = 0.6667
    let snapped = cam.snap_to_pixel(1.0);
    let expected = (1.0_f32 * 1.5).floor() / 1.5;
    assert!(
        (snapped - expected).abs() < 1e-5,
        "snapped = {snapped}, expected = {expected}"
    );
}

#[test]
fn snap_to_pixel_at_2x_dpi() {
    let mut cam = test_camera();
    cam.dpi_scale = 2.0;

    // At 2x DPI, physical pixels are at 0, 0.5, 1.0, 1.5, ...
    let snapped = cam.snap_to_pixel(0.3);
    assert!((snapped - 0.0).abs() < 1e-5, "snapped = {snapped}");

    let snapped = cam.snap_to_pixel(0.6);
    assert!((snapped - 0.5).abs() < 1e-5, "snapped = {snapped}");
}

#[test]
fn zero_time_range_does_not_panic() {
    let cam = Camera2D {
        time_start: 1000.0,
        time_end: 1000.0,
        price_low: 100.0,
        price_high: 200.0,
        viewport_width: 1920,
        viewport_height: 1080,
        dpi_scale: 1.0,
    };
    assert_eq!(cam.time_to_x(1000.0), 0.0);
    assert_eq!(cam.pixels_per_candle(300_000.0), 0.0);
}

#[test]
fn zero_price_range_does_not_panic() {
    let cam = Camera2D {
        time_start: 0.0,
        time_end: 1000.0,
        price_low: 100.0,
        price_high: 100.0,
        viewport_width: 1920,
        viewport_height: 1080,
        dpi_scale: 1.0,
    };
    assert_eq!(cam.price_to_y(100.0), 0.0);
}
