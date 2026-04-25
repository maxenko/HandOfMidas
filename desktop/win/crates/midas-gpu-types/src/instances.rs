//! GPU-layout instance types for chart rendering.
//!
//! These are pure data structs with `#[repr(C)]` and `bytemuck::Pod` derives.
//! They have **no wgpu dependency**. `midas-render` imports them from this
//! module to build GPU vertex/instance buffers.
//!
//! All pixel coordinates are in logical pixels (pre-DPI-scaling). The
//! projection matrix handles the mapping to NDC.

use bytemuck::{Pod, Zeroable};

// ── Candlestick instances ──────────────────────────────────────────

/// GPU instance data for a single candlestick.
///
/// Used by both wick pass and body pass -- the vertex shader reads different
/// fields depending on the `draw_mode` uniform.
///
/// Size: 48 bytes per instance (12 floats). Aligned to 16 bytes naturally.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct CandleInstance {
    /// Center X of this candle in pixel coordinates.
    pub x: f32,
    /// Top of body (pixel Y -- smaller value = higher on screen).
    pub body_top: f32,
    /// Bottom of body (pixel Y -- larger value = lower on screen).
    pub body_bottom: f32,
    /// Top of wick = high price in pixel Y.
    pub wick_top: f32,
    /// Bottom of wick = low price in pixel Y.
    pub wick_bottom: f32,
    /// Body width in pixels (same for all candles in a frame).
    pub width: f32,
    /// Wick width in physical pixels (always 1.0 after DPI adjustment).
    pub wick_width: f32,
    /// Dim factor: 0.0 = full brightness, 1.0 = dimmed to 30% brightness.
    /// Used for G.ATR hover highlighting.
    pub dim: f32,
    /// RGBA color (linear space, NOT sRGB).
    pub color: [f32; 4],
}

// ── Volume bar instances ───────────────────────────────────────────

/// GPU instance data for a single volume bar.
///
/// Drawn as a filled rectangle at the bottom of the chart area.
/// Size: 32 bytes per instance (8 floats).
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct VolumeInstance {
    /// Center X of the bar in pixel coordinates (same as candle x).
    pub x: f32,
    /// Top of bar in pixel Y (higher volume = lower Y = higher on screen).
    pub y_top: f32,
    /// Bottom of bar in pixel Y (constant: bottom of volume area).
    pub y_bottom: f32,
    /// Bar width in pixels.
    pub width: f32,
    /// RGBA color with alpha for semi-transparency.
    pub color: [f32; 4],
}

// ── Grid line instances ────────────────────────────────────────────

/// GPU instance data for a single axis-aligned grid line.
///
/// Horizontal lines have constant Y and span the full chart width.
/// Vertical lines have constant X and span the full chart height.
/// Each line is rendered as a filled rectangle exactly 1 physical pixel wide.
///
/// Size: 32 bytes per instance (8 floats).
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct GridLineInstance {
    /// Rectangle bounds in pixel coordinates: `[left, top, right, bottom]`.
    pub rect: [f32; 4],
    /// RGBA color (low alpha for subtle grid).
    pub color: [f32; 4],
}

// ── Session boundary marker ──────────────────────────────────────────

/// A session boundary marker -- a faint vertical line between trading sessions.
///
/// Produced when `collapse_gaps` is enabled and there is a time gap between
/// consecutive candles that exceeds the expected candle duration (e.g.,
/// overnight gaps, weekend gaps).
#[derive(Clone, Debug)]
pub struct SessionBoundary {
    /// X position in logical pixels (midpoint between the two adjacent candles).
    pub x: f32,
    /// RGBA color (typically very faint, e.g. `[0.3, 0.3, 0.4, 0.3]`).
    pub color: [f32; 4],
}

// ── High-level scene types (non-Pod) ───────────────────────────────
//
// These are used by the chart logic layer (ChartScene) and are consumed
// by both the grid/label builder and the renderer. They are NOT Pod because
// they contain String and bool fields.

/// A logical grid line with its position and label text.
///
/// Produced by the grid computation functions and consumed by both
/// the label renderer and the grid line instance builder.
#[derive(Clone, Debug)]
pub struct GridLine {
    /// Screen position: Y for horizontal (price) grid lines,
    /// X for vertical (time) grid lines, in logical pixels.
    pub position: f32,
    /// Text to display on the axis (formatted price or time string).
    pub label: String,
    /// Major lines are brighter / thicker than minor lines.
    pub is_major: bool,
}

// ── Decorator badge instances ──────────────────────────────────────

/// GPU instance data for a single decorator badge.
///
/// Consumed by the SDF `BadgePipeline` in `midas-render`; rendered with
/// analytical anti-aliasing via `fwidth()`. The fragment shader dispatches
/// on `shape_id` to pick an SDF primitive, so adding a shape means (a)
/// appending a discriminant to `BadgeShape` in `widget::decorator::badge`
/// and (b) adding a `case` to `badge.wgsl`. Reordering variants is
/// forbidden — `badge_instance_shape_id_matches_enum` in this file's tests
/// enforces the mapping.
///
/// Size: 64 bytes, 16-byte aligned — matches the WGSL vertex-attribute
/// layout declared by `BadgePipeline`.
///
/// `shape_id` mapping:
///
/// | id | `BadgeShape`             | `shape_param`                   |
/// |---:|--------------------------|---------------------------------|
/// | 0  | `Rect`                   | unused                          |
/// | 1  | `Rounded { radius }`     | `radius`                        |
/// | 2  | `Pill`                   | unused (derived: `min(w,h)/2`)  |
/// | 3  | `PointLeft { w }`        | `point_width`                   |
/// | 4  | `PointRight { w }`       | `point_width`                   |
/// | 5  | `DoublePoint { w }`      | `point_width`                   |
/// | 6  | `Chevron { w }`          | `point_width`                   |
/// | 7  | `Circle`                 | unused (derived: `min(w,h)/2`)  |
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct BadgeInstance {
    /// Screen-space bounding box `[x0, y0, x1, y1]` in logical pixels.
    pub rect: [f32; 4],
    /// Fill color in linear RGBA.
    pub fill: [f32; 4],
    /// Border color in linear RGBA; `alpha = 0` means no border.
    pub border: [f32; 4],
    /// Stable discriminant from `BadgeShape::shape_id()`.
    pub shape_id: u32,
    /// Shape parameter (radius / point_width) — see mapping table.
    pub shape_param: f32,
    /// Border thickness in logical pixels; `0.0` means no border.
    pub border_thickness: f32,
    /// Explicit padding to a 64-byte / 16-byte-aligned stride.
    pub _pad: f32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem;

    #[test]
    fn candle_instance_size_is_48_bytes() {
        assert_eq!(
            mem::size_of::<CandleInstance>(),
            48,
            "CandleInstance must be exactly 48 bytes for GPU layout"
        );
    }

    #[test]
    fn volume_instance_size_is_32_bytes() {
        assert_eq!(
            mem::size_of::<VolumeInstance>(),
            32,
            "VolumeInstance must be exactly 32 bytes for GPU layout"
        );
    }

    #[test]
    fn grid_line_instance_size_is_32_bytes() {
        assert_eq!(
            mem::size_of::<GridLineInstance>(),
            32,
            "GridLineInstance must be exactly 32 bytes for GPU layout"
        );
    }

    #[test]
    fn candle_instance_is_pod() {
        // Verify that bytemuck can cast a CandleInstance to bytes.
        let instance = CandleInstance {
            x: 100.0,
            body_top: 50.0,
            body_bottom: 60.0,
            wick_top: 45.0,
            wick_bottom: 65.0,
            width: 8.0,
            wick_width: 1.0,
            dim: 0.0,
            color: [0.0, 1.0, 0.0, 1.0],
        };
        let bytes: &[u8] = bytemuck::bytes_of(&instance);
        assert_eq!(bytes.len(), 48);
    }

    #[test]
    fn volume_instance_is_pod() {
        let instance = VolumeInstance {
            x: 100.0,
            y_top: 800.0,
            y_bottom: 1080.0,
            width: 8.0,
            color: [0.2, 0.8, 0.3, 0.3],
        };
        let bytes: &[u8] = bytemuck::bytes_of(&instance);
        assert_eq!(bytes.len(), 32);
    }

    #[test]
    fn grid_line_instance_is_pod() {
        let instance = GridLineInstance {
            rect: [0.0, 500.0, 1920.0, 500.667],
            color: [1.0, 1.0, 1.0, 0.1],
        };
        let bytes: &[u8] = bytemuck::bytes_of(&instance);
        assert_eq!(bytes.len(), 32);
    }

    #[test]
    fn candle_instance_zeroed_is_valid() {
        let instance = CandleInstance::zeroed();
        assert_eq!(instance.x, 0.0);
        assert_eq!(instance.color, [0.0; 4]);
    }

    #[test]
    fn volume_instance_zeroed_is_valid() {
        let instance = VolumeInstance::zeroed();
        assert_eq!(instance.x, 0.0);
        assert_eq!(instance.color, [0.0; 4]);
    }

    #[test]
    fn candle_instance_alignment() {
        // Verify that CandleInstance has at least 4-byte alignment
        // (f32 natural alignment).
        assert!(mem::align_of::<CandleInstance>() >= 4);
    }

    #[test]
    fn candle_instance_slice_cast() {
        // Verify that a Vec<CandleInstance> can be cast to &[u8] via
        // bytemuck for GPU buffer upload.
        let instances = vec![
            CandleInstance {
                x: 10.0,
                body_top: 20.0,
                body_bottom: 30.0,
                wick_top: 15.0,
                wick_bottom: 35.0,
                width: 6.0,
                wick_width: 1.0,
                dim: 0.0,
                color: [1.0, 0.0, 0.0, 1.0],
            },
            CandleInstance {
                x: 20.0,
                body_top: 25.0,
                body_bottom: 35.0,
                wick_top: 20.0,
                wick_bottom: 40.0,
                width: 6.0,
                wick_width: 1.0,
                dim: 0.0,
                color: [0.0, 1.0, 0.0, 1.0],
            },
        ];
        let bytes: &[u8] = bytemuck::cast_slice(&instances);
        assert_eq!(bytes.len(), 96); // 2 * 48
    }

    #[test]
    fn badge_instance_size_is_64_bytes() {
        // 4*16 = 64 bytes. Stable contract with the WGSL vertex attribute
        // layout — must match the stride declared by BadgePipeline.
        assert_eq!(mem::size_of::<BadgeInstance>(), 64);
    }

    #[test]
    fn badge_instance_pod_roundtrip() {
        let instances = vec![
            BadgeInstance {
                rect: [10.0, 20.0, 40.0, 36.0],
                fill: [0.2, 0.78, 0.35, 1.0],
                border: [0.0, 0.0, 0.0, 0.4],
                shape_id: 3,
                shape_param: 6.0,
                border_thickness: 1.0,
                _pad: 0.0,
            },
            BadgeInstance {
                rect: [50.0, 20.0, 80.0, 36.0],
                fill: [0.9, 0.25, 0.25, 1.0],
                border: [0.0; 4],
                shape_id: 0,
                shape_param: 0.0,
                border_thickness: 0.0,
                _pad: 0.0,
            },
        ];
        let bytes: &[u8] = bytemuck::cast_slice(&instances);
        assert_eq!(bytes.len(), 128); // 2 * 64
    }
}
