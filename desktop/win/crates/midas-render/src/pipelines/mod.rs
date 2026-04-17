//! GPU render pipelines for chart elements.
//!
//! Each pipeline module encapsulates a `wgpu::RenderPipeline`, its bind
//! group layouts, and the logic to upload instance data and issue draw calls.
//!
//! The pipelines share a common unit-quad vertex buffer and camera bind
//! group layout. Per-pipeline differences are in instance layouts, shaders,
//! blend states, and draw parameters.

pub mod badge;
pub mod candle;
pub mod grid;
pub mod text;
pub mod volume;

use bytemuck::{Pod, Zeroable};

// ── Shared vertex type ─────────────────────────────────────────────

/// A single vertex of the unit quad.
///
/// The unit quad spans `[0,1] x [0,1]` and is shared by all instanced
/// pipelines. Each pipeline's vertex shader expands it to the desired
/// screen-space rectangle using per-instance data.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct QuadVertex {
    /// Position in `[0,1] x [0,1]` space.
    pub position: [f32; 2],
}

/// The 6 vertices forming two triangles of the unit quad.
///
/// Winding order: counter-clockwise.
/// Triangle 1: bottom-left, bottom-right, top-right
/// Triangle 2: bottom-left, top-right, top-left
pub const UNIT_QUAD_VERTICES: [QuadVertex; 6] = [
    QuadVertex {
        position: [0.0, 0.0],
    },
    QuadVertex {
        position: [1.0, 0.0],
    },
    QuadVertex {
        position: [1.0, 1.0],
    },
    QuadVertex {
        position: [0.0, 0.0],
    },
    QuadVertex {
        position: [1.0, 1.0],
    },
    QuadVertex {
        position: [0.0, 1.0],
    },
];

// ── Draw parameters uniform ────────────────────────────────────────

/// Uniform buffer for per-draw-call parameters.
///
/// Used by the candle shader (`draw_mode`). Replaces push constants
/// for maximum compatibility. Pre-written to separate buffers; bind
/// groups are swapped between draw calls.
///
/// Size: 16 bytes (padded to uniform buffer minimum alignment).
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct DrawParamsUniform {
    /// For candle shader: 0 = wick pass, 1 = body pass.
    pub draw_mode: u32,
    /// MSDF pixel range (reserved for future text pipeline).
    pub px_range: f32,
    /// Padding to 16 bytes.
    pub _pad: [u32; 2],
}

// ── Camera uniform ─────────────────────────────────────────────────

/// GPU-side camera uniform uploaded to the uniform buffer.
///
/// Must match the WGSL `CameraUniforms` struct in all shaders.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct CameraUniform {
    /// Orthographic projection matrix: pixel-space to NDC.
    pub projection: [[f32; 4]; 4],
}

/// Vertex buffer layout for the unit quad (slot 0).
pub fn quad_vertex_buffer_layout() -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<QuadVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &[wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x2,
            offset: 0,
            shader_location: 0,
        }],
    }
}

/// Create the bind group layout for camera uniforms.
///
/// Used as `@group(0)` in all chart shaders.
pub fn create_camera_bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("camera_bind_group_layout"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: wgpu::BufferSize::new(
                    std::mem::size_of::<CameraUniform>() as u64
                ),
            },
            count: None,
        }],
    })
}

/// Create the bind group layout for draw parameters.
///
/// Used as `@group(1)` in the candle shader.
pub fn create_draw_params_bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("draw_params_bind_group_layout"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: wgpu::BufferSize::new(
                    std::mem::size_of::<DrawParamsUniform>() as u64
                ),
            },
            count: None,
        }],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quad_vertex_size_is_8_bytes() {
        assert_eq!(std::mem::size_of::<QuadVertex>(), 8);
    }

    #[test]
    fn draw_params_uniform_size_is_16_bytes() {
        assert_eq!(std::mem::size_of::<DrawParamsUniform>(), 16);
    }

    #[test]
    fn camera_uniform_size_is_64_bytes() {
        assert_eq!(std::mem::size_of::<CameraUniform>(), 64);
    }

    #[test]
    fn unit_quad_has_6_vertices() {
        assert_eq!(UNIT_QUAD_VERTICES.len(), 6);
    }

    #[test]
    fn unit_quad_vertices_in_range() {
        for v in &UNIT_QUAD_VERTICES {
            assert!((0.0..=1.0).contains(&v.position[0]));
            assert!((0.0..=1.0).contains(&v.position[1]));
        }
    }

    #[test]
    fn draw_params_is_pod() {
        let params = DrawParamsUniform {
            draw_mode: 0,
            px_range: 0.0,
            _pad: [0; 2],
        };
        let bytes: &[u8] = bytemuck::bytes_of(&params);
        assert_eq!(bytes.len(), 16);
    }

    #[test]
    fn camera_uniform_is_pod() {
        let uniform = CameraUniform {
            projection: [[0.0; 4]; 4],
        };
        let bytes: &[u8] = bytemuck::bytes_of(&uniform);
        assert_eq!(bytes.len(), 64);
    }
}
