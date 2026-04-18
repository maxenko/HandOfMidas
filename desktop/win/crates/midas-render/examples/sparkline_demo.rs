//! Sparkline demo — renders three thumbnails (up / down / flat) into
//! 100x30 PNGs on disk so a human can eyeball them.
//!
//! Outputs (CWD-relative):
//!   sparkline_demo_up.png
//!   sparkline_demo_down.png
//!   sparkline_demo_flat.png
//!
//! Run with:
//!   cargo run -p midas-render --example sparkline_demo
//!
//! Skips gracefully with exit code 0 if no wgpu adapter is available.
//!
//! The demo uses a dark background + a bright sparkline color so the
//! difference between the filled region and the background is immediately
//! visible. The flat case verifies the `y_max == y_min` guard in the
//! shader (no NaN, fills nothing above the baseline).

use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;

use midas_render::SparklinePipeline;

const WIDTH: u32 = 100;
const HEIGHT: u32 = 30;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

fn main() {
    let Some((device, queue)) = pollster::block_on(try_request_device()) else {
        println!("no adapter available, skipping");
        return;
    };

    // Three thumbnails: up-trend, down-trend, flat.
    let cases: [(&str, &[f32], [f32; 4]); 3] = [
        (
            "sparkline_demo_up.png",
            &[1.0, 2.0, 3.0, 4.0, 5.0],
            [0.20, 0.85, 0.40, 1.0],
        ),
        (
            "sparkline_demo_down.png",
            &[5.0, 4.0, 3.0, 2.0, 1.0],
            [0.90, 0.25, 0.30, 1.0],
        ),
        (
            "sparkline_demo_flat.png",
            &[2.0, 2.1, 2.0, 2.1, 2.0],
            [0.70, 0.70, 0.75, 1.0],
        ),
    ];

    let mut pipeline = SparklinePipeline::new(&device, FORMAT);

    for (filename, closes, color) in cases {
        let (y_min, y_max) = bounds_with_padding(closes);
        pipeline.update_buffer(&device, &queue, closes, y_min, y_max, color);

        let pixels = render_to_rgba(&device, &queue, &pipeline);
        let path = PathBuf::from(filename);
        write_png(&path, &pixels, WIDTH, HEIGHT).expect("write PNG");
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        println!("wrote {} ({} bytes)", path.display(), size);
    }
}

/// Compute `(y_min, y_max)` with small padding above/below so the mountain
/// never kisses the viewport edges. For flat inputs, expand the range so
/// the shader's divide-by-zero guard is never the only thing protecting us.
fn bounds_with_padding(closes: &[f32]) -> (f32, f32) {
    let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
    for &c in closes {
        if c < lo {
            lo = c;
        }
        if c > hi {
            hi = c;
        }
    }
    if !lo.is_finite() || !hi.is_finite() {
        return (0.0, 1.0);
    }
    let span = (hi - lo).max(1e-3);
    let pad = span * 0.1;
    (lo - pad, hi + pad)
}

/// Render the current pipeline state into an off-screen RGBA8 texture and
/// return the unpadded pixel buffer.
fn render_to_rgba(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipeline: &SparklinePipeline,
) -> Vec<u8> {
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("sparkline_demo_target"),
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

    let bytes_per_pixel = 4u32;
    let unpadded_row = WIDTH * bytes_per_pixel;
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let padded_row = unpadded_row.div_ceil(align) * align;

    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("sparkline_demo_readback"),
        size: (padded_row as u64) * (HEIGHT as u64),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("sparkline_demo_encoder"),
    });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("sparkline_demo_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                ops: wgpu::Operations {
                    // Dark navy background for contrast.
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.06,
                        g: 0.07,
                        b: 0.10,
                        a: 1.0,
                    }),
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
    let mut out = Vec::with_capacity((WIDTH * HEIGHT * 4) as usize);
    for y in 0..HEIGHT as usize {
        let row_start = y * padded_row as usize;
        let row_end = row_start + unpadded_row as usize;
        out.extend_from_slice(&data[row_start..row_end]);
    }
    drop(data);
    readback.unmap();
    out
}

/// Write `pixels` (RGBA8, tightly packed) to a PNG file on disk.
fn write_png(
    path: &std::path::Path,
    pixels: &[u8],
    width: u32,
    height: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    let file = File::create(path)?;
    let writer = BufWriter::new(file);

    let mut encoder = png::Encoder::new(writer, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(pixels)?;
    writer.finish()?;
    Ok(())
}

/// Best-effort adapter request — identical policy to the smoke test.
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
        Err(_) => instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                force_fallback_adapter: true,
                compatible_surface: None,
            })
            .await
            .ok()?,
    };

    adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("sparkline_demo_device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_defaults(),
            memory_hints: wgpu::MemoryHints::default(),
            trace: wgpu::Trace::Off,
            ..Default::default()
        })
        .await
        .ok()
}
