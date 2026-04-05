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
mod tests;
