//! SDF decorator badge rendering pipeline.
//!
//! Mirrors [`GridPipeline`](super::grid::GridPipeline): one unit-quad
//! vertex buffer in slot 0, a growable `BadgeInstance` buffer in slot 1,
//! one camera uniform bind group, a single instanced draw call.
//!
//! The fragment shader (`shaders/badge.wgsl`) dispatches through eight
//! signed-distance primitives keyed on `BadgeInstance::shape_id`. Draw
//! order is back-to-front and sits between candle bodies and the crosshair
//! overlay — see `ChartRenderer::render()`.

use midas_gpu_types::BadgeInstance;
use wgpu::util::DeviceExt;

use super::{quad_vertex_buffer_layout, CameraUniform, UNIT_QUAD_VERTICES};

/// Shader source included at compile time.
const SHADER_SRC: &str = include_str!("../../shaders/badge.wgsl");

/// Initial instance buffer capacity (number of badges).
const INITIAL_CAPACITY: u32 = 64;

/// SDF decorator badge rendering pipeline.
///
/// Owns its own wgpu render pipeline, vertex / instance buffers, and a
/// camera uniform bind group. The layout of the camera bind group is
/// created via [`super::create_camera_bind_group_layout`] so the same
/// projection matrix can feed both the grid and badge pipelines.
pub struct BadgePipeline {
    render_pipeline: wgpu::RenderPipeline,
    vertex_buffer: wgpu::Buffer,
    instance_buffer: wgpu::Buffer,
    instance_count: u32,
    instance_capacity: u32,
    uniform_buffer: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,
    #[allow(dead_code)]
    camera_bind_group_layout: wgpu::BindGroupLayout,
}

impl BadgePipeline {
    /// Create a new badge pipeline with alpha blending.
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("badge_shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER_SRC.into()),
        });

        let camera_bind_group_layout = super::create_camera_bind_group_layout(device);

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("badge_pipeline_layout"),
            bind_group_layouts: &[&camera_bind_group_layout],
            push_constant_ranges: &[],
        });

        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("badge_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[quad_vertex_buffer_layout(), badge_instance_buffer_layout()],
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
                    // Straight-alpha blending: SDF shaders output straight-alpha
                    // color; matches iced's primary render pass.
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
            label: Some("badge_quad_vbo"),
            contents: bytemuck::cast_slice(&UNIT_QUAD_VERTICES),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let instance_capacity = INITIAL_CAPACITY;
        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("badge_instance_buffer"),
            size: instance_capacity as u64 * std::mem::size_of::<BadgeInstance>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("badge_camera_ubo"),
            contents: bytemuck::bytes_of(&CameraUniform {
                projection: glam::Mat4::IDENTITY.to_cols_array_2d(),
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("badge_camera_bind_group"),
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

    /// Upload new badge instance data.
    ///
    /// Grows the instance buffer if capacity is exceeded.
    pub fn update_instances(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        instances: &[BadgeInstance],
    ) {
        self.instance_count = instances.len() as u32;
        if instances.is_empty() {
            return;
        }

        if self.instance_count > self.instance_capacity {
            self.instance_capacity = (self.instance_count * 2).max(INITIAL_CAPACITY);
            self.instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("badge_instance_buffer"),
                size: self.instance_capacity as u64 * std::mem::size_of::<BadgeInstance>() as u64,
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

    /// Draw all badges in a single instanced draw call.
    pub fn draw<'a>(&'a self, render_pass: &mut wgpu::RenderPass<'a>) {
        self.draw_range(render_pass, 0..self.instance_count);
    }

    /// Draw a sub-range of the instance buffer.
    ///
    /// The renderer uses this to interleave badge draws with text
    /// draws per z-layer (see `ChartRenderer::draw_pass`) so each
    /// annotation's shape and text composite over lower-z layers'
    /// shape and text as one unit.
    pub fn draw_range<'a>(
        &'a self,
        render_pass: &mut wgpu::RenderPass<'a>,
        instances: core::ops::Range<u32>,
    ) {
        if instances.is_empty() || instances.end > self.instance_count {
            if instances.end > self.instance_count {
                // Asked for more than we have — clamp silently. Better
                // to render what we can than to panic on an off-by-one.
                let clamped = instances.start..self.instance_count;
                if clamped.is_empty() {
                    return;
                }
                render_pass.set_pipeline(&self.render_pipeline);
                render_pass.set_bind_group(0, &self.uniform_bind_group, &[]);
                render_pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
                render_pass.set_vertex_buffer(1, self.instance_buffer.slice(..));
                render_pass.draw(0..6, clamped);
            }
            return;
        }
        render_pass.set_pipeline(&self.render_pipeline);
        render_pass.set_bind_group(0, &self.uniform_bind_group, &[]);
        render_pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        render_pass.set_vertex_buffer(1, self.instance_buffer.slice(..));
        render_pass.draw(0..6, instances);
    }

    /// Return the current number of instances.
    pub fn instance_count(&self) -> u32 {
        self.instance_count
    }
}

/// Instance buffer layout for `BadgeInstance`.
///
/// `BadgeInstance` fields (64 bytes total):
///   rect:             [f32;4] @ offset 0  -> location(1) vec4<f32>
///   fill:             [f32;4] @ offset 16 -> location(2) vec4<f32>
///   border:           [f32;4] @ offset 32 -> location(3) vec4<f32>
///   shape_id:         u32     @ offset 48 -> location(4) u32
///   shape_param:      f32     @ offset 52 -> location(5) f32
///   border_thickness: f32     @ offset 56 -> location(6) f32
///   _pad:             f32     @ offset 60 -> location(7) f32
fn badge_instance_buffer_layout() -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<BadgeInstance>() as u64,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &[
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x4,
                offset: 0,
                shader_location: 1,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x4,
                offset: 16,
                shader_location: 2,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x4,
                offset: 32,
                shader_location: 3,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Uint32,
                offset: 48,
                shader_location: 4,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32,
                offset: 52,
                shader_location: 5,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32,
                offset: 56,
                shader_location: 6,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32,
                offset: 60,
                shader_location: 7,
            },
        ],
    }
}
