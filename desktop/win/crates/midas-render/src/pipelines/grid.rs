//! Grid line rendering pipeline — single-pass instanced renderer.
//!
//! Each grid line is a filled axis-aligned rectangle defined by
//! `[left, top, right, bottom]` bounds. Uses alpha blending for
//! subtle grid line appearance.

use midas_chart::GridLineInstance;
use wgpu::util::DeviceExt;

use super::{quad_vertex_buffer_layout, CameraUniform, UNIT_QUAD_VERTICES};

/// Shader source included at compile time.
const SHADER_SRC: &str = include_str!("../../shaders/grid.wgsl");

/// Initial instance buffer capacity (number of grid lines).
const INITIAL_CAPACITY: u32 = 128;

/// Grid line rendering pipeline.
///
/// Owns the wgpu render pipeline, vertex/instance buffers, and the
/// camera uniform bind group.
pub struct GridPipeline {
    render_pipeline: wgpu::RenderPipeline,
    vertex_buffer: wgpu::Buffer,
    instance_buffer: wgpu::Buffer,
    instance_count: u32,
    instance_capacity: u32,
    uniform_buffer: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,
    // Kept for future bind group creation (e.g., per-chart camera).
    #[allow(dead_code)]
    camera_bind_group_layout: wgpu::BindGroupLayout,
}

impl GridPipeline {
    /// Create a new grid pipeline with alpha blending.
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("grid_shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER_SRC.into()),
        });

        let camera_bind_group_layout = super::create_camera_bind_group_layout(device);

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("grid_pipeline_layout"),
            bind_group_layouts: &[&camera_bind_group_layout],
            push_constant_ranges: &[],
        });

        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("grid_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[quad_vertex_buffer_layout(), grid_instance_buffer_layout()],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
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
                    // Alpha blending for semi-transparent grid lines
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

        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("grid_quad_vbo"),
            contents: bytemuck::cast_slice(&UNIT_QUAD_VERTICES),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let instance_capacity = INITIAL_CAPACITY;
        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("grid_instance_buffer"),
            size: instance_capacity as u64 * std::mem::size_of::<GridLineInstance>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("grid_camera_ubo"),
            contents: bytemuck::bytes_of(&CameraUniform {
                projection: glam::Mat4::IDENTITY.to_cols_array_2d(),
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("grid_camera_bind_group"),
            layout: &camera_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });

        Self {
            render_pipeline,
            vertex_buffer,
            instance_buffer,
            instance_count: 0,
            instance_capacity,
            uniform_buffer,
            uniform_bind_group,
            camera_bind_group_layout,
        }
    }

    /// Upload new grid line instance data.
    ///
    /// Grows the instance buffer if capacity is exceeded.
    pub fn update_instances(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        instances: &[GridLineInstance],
    ) {
        self.instance_count = instances.len() as u32;
        if instances.is_empty() {
            return;
        }

        if self.instance_count > self.instance_capacity {
            self.instance_capacity = (self.instance_count * 2).max(INITIAL_CAPACITY);
            self.instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("grid_instance_buffer"),
                size: self.instance_capacity as u64
                    * std::mem::size_of::<GridLineInstance>() as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }

        queue.write_buffer(&self.instance_buffer, 0, bytemuck::cast_slice(instances));
    }

    /// Upload a new projection matrix.
    pub fn update_projection(&self, queue: &wgpu::Queue, projection: &glam::Mat4) {
        let uniform = CameraUniform {
            projection: projection.to_cols_array_2d(),
        };
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniform));
    }

    /// Draw all grid lines in a single instanced draw call.
    pub fn draw<'a>(&'a self, render_pass: &mut wgpu::RenderPass<'a>) {
        if self.instance_count == 0 {
            return;
        }
        render_pass.set_pipeline(&self.render_pipeline);
        render_pass.set_bind_group(0, &self.uniform_bind_group, &[]);
        render_pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        render_pass.set_vertex_buffer(1, self.instance_buffer.slice(..));
        render_pass.draw(0..6, 0..self.instance_count);
    }

    /// Return the current number of instances.
    pub fn instance_count(&self) -> u32 {
        self.instance_count
    }
}

/// Instance buffer layout for `GridLineInstance`.
///
/// GridLineInstance fields (32 bytes total):
///   rect:  [f32;4] @ offset 0   -> location(1) as vec4<f32>
///   color: [f32;4] @ offset 16  -> location(2) as vec4<f32>
fn grid_instance_buffer_layout() -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<GridLineInstance>() as u64,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &[
            // location(1): rect vec4<f32> [left, top, right, bottom]
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x4,
                offset: 0,
                shader_location: 1,
            },
            // location(2): color vec4<f32>
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x4,
                offset: 16,
                shader_location: 2,
            },
        ],
    }
}
