//! GPU-layout instance types for chart rendering.
//!
//! As of slice A2, the actual type definitions live in the
//! [`midas_gpu_types`] leaf crate so the chart crate can be retired
//! without dragging the GPU wire-format types with it. This module is a
//! pass-through re-export; the layout-regression tests below still run
//! against the `midas-chart` build to keep the size/alignment guard in
//! both crates' test surfaces.
//!
//! Slice A2b will add `#[deprecated]` to the re-export once consumer
//! migration is complete.

#[deprecated(
    note = "import from midas_gpu_types directly; midas-chart will be deleted in slice 9c"
)]
pub use midas_gpu_types::{
    AxisLabel, BadgeInstance, CandleInstance, CrosshairRender, GridLine, GridLineInstance,
    OhlcvOverlay, SessionBoundary, VolumeInstance,
};

#[cfg(test)]
mod tests {
    use super::*;
    use bytemuck::Zeroable;
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
