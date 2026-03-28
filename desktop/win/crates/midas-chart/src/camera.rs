//! 2D orthographic camera for a single chart panel.
//!
//! Maps a rectangular region of data space (time in epoch ms, price in
//! dollars) to pixel space. This is the canonical definition used
//! throughout the chart system; `midas-render` imports it from here.

/// 2D orthographic camera for a single chart panel.
///
/// Maps a rectangular region of data space (time x price) to pixel space.
/// The camera defines the visible time and price window. All time values
/// are epoch milliseconds as `f64`; all price values are `f64`.
///
/// # Invariants
///
/// - `time_start < time_end` — always.
/// - `price_low < price_high` — always.
/// - `viewport_width > 0` and `viewport_height > 0`.
/// - `dpi_scale > 0.0`.
#[derive(Clone, Debug)]
pub struct Camera2D {
    /// Visible time range start (epoch milliseconds).
    pub time_start: f64,
    /// Visible time range end (epoch milliseconds).
    pub time_end: f64,
    /// Visible price range bottom.
    pub price_low: f64,
    /// Visible price range top.
    pub price_high: f64,
    /// Viewport width in logical pixels.
    pub viewport_width: u32,
    /// Viewport height in logical pixels.
    pub viewport_height: u32,
    /// Display DPI scale factor (e.g., 1.0, 1.5, 2.0).
    pub dpi_scale: f32,
}

impl Camera2D {
    /// Convert a timestamp (epoch ms, `f64`) to logical pixel X.
    ///
    /// The result is in `[0, viewport_width]` for timestamps within the
    /// visible range, but may extend outside for timestamps off-screen.
    #[inline]
    pub fn time_to_x(&self, timestamp: f64) -> f32 {
        let time_range = self.time_end - self.time_start;
        if time_range == 0.0 {
            return 0.0;
        }
        let fraction = (timestamp - self.time_start) / time_range;
        (fraction * self.viewport_width as f64) as f32
    }

    /// Convert a price to logical pixel Y.
    ///
    /// Price axis is inverted: higher prices map to lower Y values
    /// (top of screen). The result is in `[0, viewport_height]` for
    /// prices within the visible range.
    #[inline]
    pub fn price_to_y(&self, price: f64) -> f32 {
        let price_range = self.price_high - self.price_low;
        if price_range == 0.0 {
            return 0.0;
        }
        let fraction = (self.price_high - price) / price_range;
        (fraction * self.viewport_height as f64) as f32
    }

    /// Convert a logical pixel X to a timestamp (epoch ms).
    ///
    /// Inverse of [`time_to_x`](Self::time_to_x).
    #[inline]
    pub fn x_to_time(&self, x: f32) -> f64 {
        let time_range = self.time_end - self.time_start;
        let fraction = x as f64 / self.viewport_width as f64;
        self.time_start + fraction * time_range
    }

    /// Convert a logical pixel Y to a price.
    ///
    /// Inverse of [`price_to_y`](Self::price_to_y).
    #[inline]
    pub fn y_to_price(&self, y: f32) -> f64 {
        let price_range = self.price_high - self.price_low;
        let fraction = y as f64 / self.viewport_height as f64;
        self.price_high - fraction * price_range
    }

    /// Build the orthographic projection matrix.
    ///
    /// Maps `[0, viewport_width] x [0, viewport_height]` to NDC
    /// `[-1, +1] x [-1, +1]`. Y is inverted so that screen Y=0
    /// (top) maps to NDC Y=+1 and Y=viewport_height (bottom) maps
    /// to NDC Y=-1.
    ///
    /// Uses right-handed coordinates with Z in `[0, 1]` (wgpu convention).
    pub fn projection_matrix(&self) -> glam::Mat4 {
        let w = self.viewport_width as f32;
        let h = self.viewport_height as f32;

        // glam::Mat4::orthographic_rh maps:
        //   x: [left, right] -> [-1, +1]
        //   y: [bottom, top] -> [-1, +1]
        //   z: [near, far]   -> [0, 1] (reversed Z for wgpu)
        //
        // We want Y=0 (top of screen) to be at the top of NDC (+1),
        // so we set bottom=h, top=0.
        glam::Mat4::orthographic_rh(
            0.0, // left
            w,   // right
            h,   // bottom (screen bottom = high Y = NDC bottom)
            0.0, // top (screen top = Y=0 = NDC top)
            0.0, // near
            1.0, // far
        )
    }

    /// Compute how many logical pixels each candle time slot occupies.
    ///
    /// This is the full slot width (not the body width, which is typically
    /// 70% of this). Pass `candle_duration_ms` as the time span of one
    /// candle in milliseconds.
    pub fn pixels_per_candle(&self, candle_duration_ms: f64) -> f32 {
        let time_range = self.time_end - self.time_start;
        if time_range == 0.0 {
            return 0.0;
        }
        (candle_duration_ms / time_range * self.viewport_width as f64) as f32
    }

    /// Return the visible time span in milliseconds.
    #[inline]
    pub fn visible_time_span(&self) -> f64 {
        self.time_end - self.time_start
    }

    /// Snap a logical pixel value to the nearest physical pixel boundary.
    ///
    /// This prevents sub-pixel rendering artifacts (blurry lines, fuzzy text).
    /// The result lands on the left/top edge of the nearest physical pixel.
    #[inline]
    pub fn snap_to_pixel(&self, value: f32) -> f32 {
        (value * self.dpi_scale).floor() / self.dpi_scale
    }
}

#[cfg(test)]
mod tests {
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

        let price_per_pixel =
            (cam.price_high - cam.price_low) / cam.viewport_height as f64;
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
        assert!(
            (span - 100_000_000.0).abs() < 1e-3,
            "span = {span}"
        );
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
}
