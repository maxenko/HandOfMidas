//! Smoke test for [`midas_render::SparklinePipeline`].
//!
//! Spins up a headless wgpu instance, renders a small mountain into a
//! 100x30 RGBA texture, and asserts that at least one pixel ended up
//! meaningfully green — i.e. the pipeline actually produced fragments.
//!
//! If no wgpu adapter is available (common in sandboxed CI runners with
//! no GPU and no software rasterizer) the test prints a notice and
//! returns success rather than failing, so CI without a GPU stays green.

use midas_render::SparklinePipeline;

/// Off-screen target size for the smoke render.
const WIDTH: u32 = 100;
const HEIGHT: u32 = 30;

/// Framebuffer format — deliberately not `_Srgb` so the exact byte values
/// we write out equal the linear color we set in the uniform, which keeps
/// the pixel-check assertion trivially true.
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

#[test]
fn sparkline_renders_at_least_one_green_pixel() {
    let Some((device, queue)) = pollster::block_on(try_request_device()) else {
        println!("no adapter available, skipping");
        return;
    };

    // Build pipeline.
    let mut pipeline = SparklinePipeline::new(&device, FORMAT);

    // Upload 4 close samples in a mountain-ish shape.
    let closes: [f32; 4] = [1.0, 2.0, 1.5, 3.0];
    pipeline.update_buffer(
        &device,
        &queue,
        &closes,
        0.9,                  // y_min (slightly below the lowest close)
        3.1,                  // y_max (slightly above the highest close)
        [0.0, 1.0, 0.0, 1.0], // full-green fill
    );

    // Off-screen color target.
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("sparkline_smoke_target"),
        size: wgpu::Extent3d {
            width: WIDTH,
            height: HEIGHT,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());

    // Readback buffer. Row pitch must be padded to COPY_BYTES_PER_ROW_ALIGNMENT.
    let bytes_per_pixel = 4u32;
    let unpadded_row = WIDTH * bytes_per_pixel;
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let padded_row = unpadded_row.div_ceil(align) * align;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("sparkline_smoke_readback"),
        size: (padded_row as u64) * (HEIGHT as u64),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    // Encode: clear + render sparkline + copy to buffer.
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("sparkline_smoke_encoder"),
    });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("sparkline_smoke_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pipeline.render(&mut pass);
    }
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &target,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_row),
                rows_per_image: Some(HEIGHT),
            },
        },
        wgpu::Extent3d {
            width: WIDTH,
            height: HEIGHT,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(Some(encoder.finish()));

    // Map buffer and count green pixels.
    let slice = readback.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .expect("poll failed");
    rx.recv()
        .expect("map_async channel dropped")
        .expect("readback map failed");

    let data = slice.get_mapped_range();
    let mut green_pixels = 0usize;
    for y in 0..HEIGHT as usize {
        let row_start = y * padded_row as usize;
        for x in 0..WIDTH as usize {
            let px = row_start + x * 4;
            let r = data[px];
            let g = data[px + 1];
            let b = data[px + 2];
            // Green pixel = G high, R/B low (distinct from the black clear).
            if g > 128 && r < 64 && b < 64 {
                green_pixels += 1;
            }
        }
    }
    drop(data);
    readback.unmap();

    assert!(
        green_pixels > 0,
        "expected at least one green pixel from sparkline render, found none"
    );
    println!("sparkline smoke: {green_pixels} green pixels rendered");
}

/// Try to spin up a wgpu adapter + device. Returns `None` when the host
/// has no usable adapter (e.g. CI without a GPU and no fallback).
async fn try_request_device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::all(),
        ..Default::default()
    });

    let adapter = match instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            force_fallback_adapter: false,
            compatible_surface: None,
        })
        .await
    {
        Ok(a) => a,
        Err(_) => {
            // Fall back to software / swiftshader if available.
            instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::LowPower,
                    force_fallback_adapter: true,
                    compatible_surface: None,
                })
                .await
                .ok()?
        }
    };

    adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("sparkline_smoke_device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_defaults(),
            memory_hints: wgpu::MemoryHints::default(),
            trace: wgpu::Trace::Off,
            ..Default::default()
        })
        .await
        .ok()
}
