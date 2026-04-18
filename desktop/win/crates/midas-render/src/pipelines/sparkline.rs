//! Sparkline (mountain) rendering pipeline — single, reusable pipeline
//! shared across every thumbnail widget in the app.
//!
//! The pipeline consumes a `&[f32]` of close prices via a read-only storage
//! buffer and emits a non-indexed triangle strip that fills the region
//! between a flat baseline and the polyline tracing those closes. It is
//! intentionally minimal: one bind group with two entries (storage buffer
//! of closes + uniform of bounds/color/count), no camera matrix, no
//! per-instance data. All normalization happens inside the shader.
//!
//! Typical caller flow:
//!
//! ```ignore
//! let mut pipe = SparklinePipeline::new(&device, format);
//! pipe.update_buffer(&device, &queue, &closes, y_min, y_max, color);
//! // inside a render pass targeting `format`:
//! pipe.render(&mut pass);
//! ```
//!
//! See `shaders/sparkline.wgsl` for the shader-side layout.
//!
//! Design references: `candle.rs` for overall pipeline scaffolding and
//! feature plan `plan/feature-chart-thumbnail-cells.md` (Slice 1).

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

/// Shader source included at compile time.
const SHADER_SRC: &str = include_str!("../../shaders/sparkline.wgsl");

/// Initial storage buffer capacity, measured in `f32` slots (closes).
///
/// Sized for the v1 thumbnail default of 100 closes per ticker; grows on
/// demand if a caller uploads a larger slice.
const INITIAL_CAPACITY: u32 = 128;

/// GPU-side uniform layout — must match the `Uniforms` struct in
/// `sparkline.wgsl` (16-byte alignment, 32 bytes total).
///
/// Field order matches WGSL so `bytemuck::bytes_of` writes directly.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct SparklineUniform {
    /// RGBA fill color.
    color: [f32; 4],
    /// Minimum close in data space — baseline of the mountain.
    y_min: f32,
    /// Maximum close in data space — top of the mountain.
    y_max: f32,
    /// Number of valid close samples in the storage buffer.
    count: u32,
    /// Padding to keep the struct at 32 bytes.
    _pad: u32,
}

/// Sparkline rendering pipeline.
///
/// Owns the wgpu render pipeline, a growable storage buffer of f32 closes,
/// a uniform buffer for per-draw parameters, and the combined bind group.
///
/// The storage buffer is recreated (destroy + allocate) when the caller
/// uploads more closes than the current capacity; otherwise uploads use
/// `Queue::write_buffer` in place.
pub struct SparklinePipeline {
    /// Compiled render pipeline — fixed for the lifetime of this object.
    pipeline: wgpu::RenderPipeline,
    /// Bind group layout kept around for storage buffer recreation.
    bind_group_layout: wgpu::BindGroupLayout,
    /// Uniform buffer (32 bytes) — color, bounds, count.
    uniform_buf: wgpu::Buffer,
    /// Storage buffer holding up to `capacity` f32 closes.
    storage_buf: wgpu::Buffer,
    /// Bind group referencing the current `storage_buf` and `uniform_buf`.
    ///
    /// Must be recreated whenever `storage_buf` is replaced.
    bind_group: wgpu::BindGroup,
    /// Current storage buffer capacity measured in f32 slots.
    capacity: u32,
    /// Number of valid close samples for the next draw. Zero means skip.
    count: u32,
}

impl SparklinePipeline {
    /// Create a new sparkline pipeline targeting the given color format.
    ///
    /// Allocates a small initial storage buffer (`INITIAL_CAPACITY` slots)
    /// and a 32-byte uniform buffer. Both grow-on-demand rules live in
    /// [`update_buffer`](Self::update_buffer).
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sparkline_shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER_SRC.into()),
        });

        let bind_group_layout = create_bind_group_layout(device);

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sparkline_pipeline_layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sparkline_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                // No vertex buffers — the shader synthesizes positions from
                // `@builtin(vertex_index)` alone.
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState {
                count: 1,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    // Alpha-blend so semi-transparent fills compose over
                    // whatever background the thumbnail cell paints.
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::SrcAlpha,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        });

        let capacity = INITIAL_CAPACITY;
        let storage_buf = create_storage_buffer(device, capacity);

        let uniform_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sparkline_uniform_buf"),
            contents: bytemuck::bytes_of(&SparklineUniform {
                color: [0.0; 4],
                y_min: 0.0,
                y_max: 1.0,
                count: 0,
                _pad: 0,
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let bind_group = create_bind_group(device, &bind_group_layout, &storage_buf, &uniform_buf);

        Self {
            pipeline,
            bind_group_layout,
            uniform_buf,
            storage_buf,
            bind_group,
            capacity,
            count: 0,
        }
    }

    /// Upload a new set of closes plus the associated normalization bounds
    /// and fill color.
    ///
    /// Grows the storage buffer (destroy + recreate) when `closes.len()`
    /// exceeds the current capacity. The bind group is recreated in the
    /// same path because it references the replaced buffer.
    ///
    /// An empty `closes` slice is legal — it parks the pipeline in the
    /// "nothing to draw" state and later calls to [`render`](Self::render)
    /// short-circuit without issuing a draw call.
    pub fn update_buffer(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        closes: &[f32],
        y_min: f32,
        y_max: f32,
        color: [f32; 4],
    ) {
        let count = closes.len() as u32;
        self.count = count;

        // Always refresh the uniform — color / bounds can change even when
        // the close slice does not.
        let uniform = SparklineUniform {
            color,
            y_min,
            y_max,
            count,
            _pad: 0,
        };
        queue.write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&uniform));

        if closes.is_empty() {
            return;
        }

        if count > self.capacity {
            // Grow with 2x headroom to amortize repeated resizes.
            self.capacity = (count * 2).max(INITIAL_CAPACITY);
            self.storage_buf = create_storage_buffer(device, self.capacity);
            self.bind_group = create_bind_group(
                device,
                &self.bind_group_layout,
                &self.storage_buf,
                &self.uniform_buf,
            );
        }

        queue.write_buffer(&self.storage_buf, 0, bytemuck::cast_slice(closes));
    }

    /// Issue the draw call for the previously uploaded closes.
    ///
    /// Binds the pipeline + bind group and dispatches `2 * count` vertices
    /// as a triangle strip. A draw requires at least two closes — fewer
    /// samples would degenerate the strip, so the call is skipped.
    pub fn render<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        if self.count < 2 {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.draw(0..(2 * self.count), 0..1);
    }

    /// Number of valid close samples currently uploaded.
    pub fn count(&self) -> u32 {
        self.count
    }

    /// Current storage-buffer capacity measured in f32 slots.
    pub fn capacity(&self) -> u32 {
        self.capacity
    }
}

// ── Helpers ────────────────────────────────────────────────────────

/// Create the bind group layout used by the sparkline pipeline.
///
/// Layout:
///   binding 0 — read-only storage buffer, vertex stage only
///   binding 1 — uniform buffer, vertex + fragment stages
fn create_bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("sparkline_bind_group_layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<f32>() as u64),
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(
                        std::mem::size_of::<SparklineUniform>() as u64,
                    ),
                },
                count: None,
            },
        ],
    })
}

/// Allocate a fresh storage buffer sized for `capacity` f32 samples.
fn create_storage_buffer(device: &wgpu::Device, capacity: u32) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("sparkline_storage_buf"),
        size: capacity as u64 * std::mem::size_of::<f32>() as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// Build the combined bind group that references the current storage +
/// uniform buffers.
fn create_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    storage_buf: &wgpu::Buffer,
    uniform_buf: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("sparkline_bind_group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: storage_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: uniform_buf.as_entire_binding(),
            },
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uniform_is_32_bytes() {
        assert_eq!(std::mem::size_of::<SparklineUniform>(), 32);
    }

    #[test]
    fn uniform_is_pod() {
        let u = SparklineUniform {
            color: [0.1, 0.2, 0.3, 0.4],
            y_min: -1.0,
            y_max: 1.0,
            count: 42,
            _pad: 0,
        };
        let bytes: &[u8] = bytemuck::bytes_of(&u);
        assert_eq!(bytes.len(), 32);
    }

    #[test]
    fn shader_source_contains_expected_entry_points() {
        assert!(SHADER_SRC.contains("vs_main"));
        assert!(SHADER_SRC.contains("fs_main"));
        assert!(SHADER_SRC.contains("closes"));
    }
}
