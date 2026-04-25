//! Candlestick rendering pipeline — two-pass instanced renderer.
//!
//! Pass 1 (wick): draws thin vertical lines (1px wide) from wick_top to wick_bottom.
//! Pass 2 (body): draws wider rectangles from body_top to body_bottom.
//!
//! Both passes share the same instance buffer. A `draw_mode` uniform
//! (swapped via bind group) controls which rectangle dimensions the
//! vertex shader uses.

use midas_gpu_types::CandleInstance;
use wgpu::util::DeviceExt;

use super::{quad_vertex_buffer_layout, CameraUniform, DrawParamsUniform, UNIT_QUAD_VERTICES};

/// Shader source included at compile time.
const SHADER_SRC: &str = include_str!("../../shaders/candle.wgsl");

/// Initial instance buffer capacity (number of candles).
const INITIAL_CAPACITY: u32 = 4096;

/// Candle rendering pipeline.
///
/// Owns the wgpu render pipeline, vertex/instance buffers, and the
/// pre-written draw parameter bind groups for wick and body passes.
pub struct CandlePipeline {
    render_pipeline: wgpu::RenderPipeline,
    vertex_buffer: wgpu::Buffer,
    instance_buffer: wgpu::Buffer,
    instance_count: u32,
    instance_capacity: u32,
    uniform_buffer: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,
    // RAII: buffers must outlive their bind groups.
    #[allow(dead_code)]
    wick_params_buffer: wgpu::Buffer,
    wick_params_bind_group: wgpu::BindGroup,
    #[allow(dead_code)]
    body_params_buffer: wgpu::Buffer,
    body_params_bind_group: wgpu::BindGroup,
    // Kept for future bind group creation (e.g., per-chart camera).
    #[allow(dead_code)]
    camera_bind_group_layout: wgpu::BindGroupLayout,
    #[allow(dead_code)]
    draw_params_bind_group_layout: wgpu::BindGroupLayout,
}

impl CandlePipeline {
    /// Create a new candle pipeline.
    ///
    /// Allocates all GPU resources and pre-writes the draw parameter
    /// buffers for wick (`draw_mode=0`) and body (`draw_mode=1`) passes.
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        // Shader module
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("candle_shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER_SRC.into()),
        });

        // Bind group layouts
        let camera_bind_group_layout = super::create_camera_bind_group_layout(device);
        let draw_params_bind_group_layout = super::create_draw_params_bind_group_layout(device);

        // Pipeline layout
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("candle_pipeline_layout"),
            bind_group_layouts: &[&camera_bind_group_layout, &draw_params_bind_group_layout],
            push_constant_ranges: &[],
        });

        // Render pipeline
        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("candle_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[quad_vertex_buffer_layout(), candle_instance_buffer_layout()],
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
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        });

        // Unit quad vertex buffer
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("candle_quad_vbo"),
            contents: bytemuck::cast_slice(&UNIT_QUAD_VERTICES),
            usage: wgpu::BufferUsages::VERTEX,
        });

        // Instance buffer (pre-allocated)
        let instance_capacity = INITIAL_CAPACITY;
        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("candle_instance_buffer"),
            size: instance_capacity as u64 * std::mem::size_of::<CandleInstance>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Camera uniform buffer
        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("candle_camera_ubo"),
            contents: bytemuck::bytes_of(&CameraUniform {
                projection: glam::Mat4::IDENTITY.to_cols_array_2d(),
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("candle_camera_bind_group"),
            layout: &camera_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });

        // Wick draw params (draw_mode=0, written once, never updated)
        let wick_params = DrawParamsUniform {
            draw_mode: 0,
            px_range: 0.0,
            _pad: [0; 2],
        };
        let wick_params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("candle_wick_params_ubo"),
            contents: bytemuck::bytes_of(&wick_params),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let wick_params_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("candle_wick_params_bind_group"),
            layout: &draw_params_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wick_params_buffer.as_entire_binding(),
            }],
        });

        // Body draw params (draw_mode=1, written once, never updated)
        let body_params = DrawParamsUniform {
            draw_mode: 1,
            px_range: 0.0,
            _pad: [0; 2],
        };
        let body_params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("candle_body_params_ubo"),
            contents: bytemuck::bytes_of(&body_params),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let body_params_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("candle_body_params_bind_group"),
            layout: &draw_params_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: body_params_buffer.as_entire_binding(),
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
            wick_params_buffer,
            wick_params_bind_group,
            body_params_buffer,
            body_params_bind_group,
            camera_bind_group_layout,
            draw_params_bind_group_layout,
        }
    }

    /// Upload new candle instance data.
    ///
    /// If the instance count exceeds the current buffer capacity, the
    /// buffer is reallocated with 2x headroom.
    pub fn update_instances(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        instances: &[CandleInstance],
    ) {
        self.instance_count = instances.len() as u32;
        if instances.is_empty() {
            return;
        }

        // Grow buffer if needed
        if self.instance_count > self.instance_capacity {
            self.instance_capacity = (self.instance_count * 2).max(INITIAL_CAPACITY);
            self.instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("candle_instance_buffer"),
                size: self.instance_capacity as u64 * std::mem::size_of::<CandleInstance>() as u64,
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

    /// Draw candle wicks (pass 1, draw_mode=0).
    ///
    /// Sets the wick parameters bind group and issues an instanced draw.
    /// The render pipeline and vertex buffers must be set by the caller
    /// via [`prepare_pass`].
    pub fn draw_wicks<'a>(&'a self, render_pass: &mut wgpu::RenderPass<'a>) {
        if self.instance_count == 0 {
            return;
        }
        render_pass.set_pipeline(&self.render_pipeline);
        render_pass.set_bind_group(0, &self.uniform_bind_group, &[]);
        render_pass.set_bind_group(1, &self.wick_params_bind_group, &[]);
        render_pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        render_pass.set_vertex_buffer(1, self.instance_buffer.slice(..));
        render_pass.draw(0..6, 0..self.instance_count);
    }

    /// Draw candle bodies (pass 2, draw_mode=1).
    ///
    /// Sets the body parameters bind group and issues an instanced draw.
    pub fn draw_bodies<'a>(&'a self, render_pass: &mut wgpu::RenderPass<'a>) {
        if self.instance_count == 0 {
            return;
        }
        render_pass.set_pipeline(&self.render_pipeline);
        render_pass.set_bind_group(0, &self.uniform_bind_group, &[]);
        render_pass.set_bind_group(1, &self.body_params_bind_group, &[]);
        render_pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        render_pass.set_vertex_buffer(1, self.instance_buffer.slice(..));
        render_pass.draw(0..6, 0..self.instance_count);
    }

    /// Return the current number of instances.
    pub fn instance_count(&self) -> u32 {
        self.instance_count
    }
}

/// Instance buffer layout for `CandleInstance`.
///
/// Matches the WGSL `InstanceInput` struct in `candle.wgsl`.
/// CandleInstance fields (48 bytes total):
///   x:           f32 @ offset 0   -> location(1)
///   body_top:    f32 @ offset 4   -> location(2)
///   body_bottom: f32 @ offset 8   -> location(3)
///   wick_top:    f32 @ offset 12  -> location(4)
///   wick_bottom: f32 @ offset 16  -> location(5)
///   width:       f32 @ offset 20  -> location(6)
///   wick_width:  f32 @ offset 24  -> location(7)
///   dim:         f32 @ offset 28  -> location(9)
///   color:       [f32;4] @ offset 32 -> location(8)
fn candle_instance_buffer_layout() -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<CandleInstance>() as u64,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &[
            // location(1): x
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32,
                offset: 0,
                shader_location: 1,
            },
            // location(2): body_top
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32,
                offset: 4,
                shader_location: 2,
            },
            // location(3): body_bottom
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32,
                offset: 8,
                shader_location: 3,
            },
            // location(4): wick_top
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32,
                offset: 12,
                shader_location: 4,
            },
            // location(5): wick_bottom
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32,
                offset: 16,
                shader_location: 5,
            },
            // location(6): width
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32,
                offset: 20,
                shader_location: 6,
            },
            // location(7): wick_width
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32,
                offset: 24,
                shader_location: 7,
            },
            // location(9): dim (was _pad0; 0.0 = full brightness, 1.0 = dimmed)
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32,
                offset: 28,
                shader_location: 9,
            },
            // location(8): color vec4<f32>
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x4,
                offset: 32,
                shader_location: 8,
            },
        ],
    }
}
