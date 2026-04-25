//! midas-render: wgpu chart rendering pipelines.
//!
//! Depends on: midas-core, midas-data, midas-chart
//!
//! This crate does NOT depend on iced. It produces wgpu render passes
//! that the iced Shader widget in midas-app consumes. This decoupling
//! allows headless rendering and isolated testing.

pub mod color;
pub mod pipelines;
pub mod renderer;

// ── Re-exports ─────────────────────────────────────────────────────

pub use color::{dark_theme, light_theme, ChartTheme};
pub use pipelines::candle::CandlePipeline;
pub use pipelines::grid::GridPipeline;
pub use pipelines::sparkline::SparklinePipeline;
pub use pipelines::volume::VolumePipeline;
pub use pipelines::{CameraUniform, DrawParamsUniform, QuadVertex, UNIT_QUAD_VERTICES};
pub use renderer::{ChartRenderer, ChartScene};

// ── Shader sources (compile-time inclusion) ────────────────────────

/// Shader source for the candlestick pipeline.
pub const CANDLE_SHADER_SRC: &str = include_str!("../shaders/candle.wgsl");

/// Shader source for the volume bar pipeline.
pub const VOLUME_SHADER_SRC: &str = include_str!("../shaders/volume.wgsl");

/// Shader source for the grid line pipeline.
pub const GRID_SHADER_SRC: &str = include_str!("../shaders/grid.wgsl");

/// Shader source for the sparkline (mountain) thumbnail pipeline.
pub const SPARKLINE_SHADER_SRC: &str = include_str!("../shaders/sparkline.wgsl");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candle_shader_source_loads() {
        // The contains() checks below imply non-empty.
        assert!(CANDLE_SHADER_SRC.contains("vs_main"));
        assert!(CANDLE_SHADER_SRC.contains("fs_main"));
        assert!(CANDLE_SHADER_SRC.contains("draw_mode"));
    }

    #[test]
    fn volume_shader_source_loads() {
        assert!(VOLUME_SHADER_SRC.contains("vs_main"));
        assert!(VOLUME_SHADER_SRC.contains("fs_main"));
    }

    #[test]
    fn grid_shader_source_loads() {
        assert!(GRID_SHADER_SRC.contains("vs_main"));
        assert!(GRID_SHADER_SRC.contains("fs_main"));
    }

    #[test]
    fn candle_instance_bytemuck_cast() {
        use midas_gpu_types::CandleInstance;
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
    fn volume_instance_bytemuck_cast() {
        use midas_gpu_types::VolumeInstance;
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
    fn grid_instance_bytemuck_cast() {
        use midas_gpu_types::GridLineInstance;
        let instance = GridLineInstance {
            rect: [0.0, 500.0, 1920.0, 500.667],
            color: [1.0, 1.0, 1.0, 0.1],
        };
        let bytes: &[u8] = bytemuck::bytes_of(&instance);
        assert_eq!(bytes.len(), 32);
    }

    #[test]
    fn struct_sizes_match_gpu_layout() {
        use midas_gpu_types::{CandleInstance, GridLineInstance, VolumeInstance};
        assert_eq!(std::mem::size_of::<CandleInstance>(), 48);
        assert_eq!(std::mem::size_of::<VolumeInstance>(), 32);
        assert_eq!(std::mem::size_of::<GridLineInstance>(), 32);
        assert_eq!(std::mem::size_of::<CameraUniform>(), 64);
        assert_eq!(std::mem::size_of::<DrawParamsUniform>(), 16);
        assert_eq!(std::mem::size_of::<QuadVertex>(), 8);
    }

    #[test]
    fn camera_uniform_from_projection() {
        let proj = glam::Mat4::orthographic_rh(0.0, 1920.0, 1080.0, 0.0, 0.0, 1.0);
        let uniform = CameraUniform {
            projection: proj.to_cols_array_2d(),
        };
        // Verify the matrix round-trips through the Pod type
        let back = glam::Mat4::from_cols_array_2d(&uniform.projection);
        assert_eq!(proj, back);
    }

    // NOTE: GPU integration tests (pipeline creation, draw calls) require
    // a wgpu device. These will be added in the testing phase using the
    // wgpu headless adapter (wgpu::Backends::GL with mesa-llvmpipe or
    // wgpu::Instance::new with features for headless testing).
}
