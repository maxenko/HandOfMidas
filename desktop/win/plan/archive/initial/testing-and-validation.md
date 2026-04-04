# Testing & Validation Strategy -- Complete Architecture

> Midas Desktop (Rust + iced + wgpu) -- QA & Automated Verification Plan
> Authored 2026-03-24. Target: wgpu 27+, iced 0.14+, Windows 11, DX12/Vulkan/CPU backends.
>
> Primary constraint: development is AI-assisted (Claude Code). The AI cannot observe
> a live window. Every visual artifact, every pixel alignment error, every rendering
> regression must be caught by automated tests that produce machine-diffable output.

---

## Table of Contents

1. [Headless Rendering Harness](#1-headless-rendering-harness)
2. [Screenshot Comparison Tests](#2-screenshot-comparison-tests)
3. [Pixel-Perfect Alignment Tests](#3-pixel-perfect-alignment-tests)
4. [Data Layer Tests](#4-data-layer-tests)
5. [Interaction Tests](#5-interaction-tests)
6. [Layout Tests](#6-layout-tests)
7. [Criterion Benchmarks](#7-criterion-benchmarks)
8. [Integration Test: Full Chart Render](#8-integration-test-full-chart-render)
9. [Test Data](#9-test-data)
10. [CI Considerations](#10-ci-considerations)
11. [Visual Regression Workflow](#11-visual-regression-workflow)

---

## 1. Headless Rendering Harness

### 1.1 The Problem

The application renders charts via wgpu's `RenderPass` inside iced's `Shader` widget. During
normal operation, a human stares at the window. During AI-assisted development and CI, nobody
is watching. We need to render charts to an offscreen texture, read the pixels back to the CPU,
and save them as PNG files that automated tests (and the AI) can inspect.

wgpu provides everything we need:
- `wgpu::Instance` can be created without a surface (headless).
- A `wgpu::Texture` with `RENDER_ATTACHMENT | COPY_SRC` usage serves as our framebuffer.
- After rendering, we copy the texture to a `wgpu::Buffer` with `MAP_READ | COPY_DST` usage.
- Map the buffer, read the pixels, write a PNG.

### 1.2 Crate Layout

The headless harness lives in `crates/midas-render/src/headless.rs` and is gated behind
a `test-harness` feature flag so it does not ship in the release binary. Test utilities
go in `crates/midas-render/src/test_utils.rs` (also feature-gated).

```toml
# In crates/midas-render/Cargo.toml

[features]
default = []
test-harness = ["png", "image"]

[dependencies]
# ... existing deps ...
png = { version = "0.17", optional = true }
image = { version = "0.25", optional = true }

[dev-dependencies]
midas-render = { path = ".", features = ["test-harness"] }
pollster = "0.4"
```

### 1.3 HeadlessRenderer Struct

```rust
// crates/midas-render/src/headless.rs

use wgpu;

/// GPU-backed offscreen renderer for tests and CI.
/// Creates its own wgpu device, renders to an offscreen texture,
/// and reads pixels back to CPU memory.
pub struct HeadlessRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// The offscreen render target.
    target_texture: wgpu::Texture,
    target_view: wgpu::TextureView,
    /// A staging buffer for reading pixels back to CPU.
    readback_buffer: wgpu::Buffer,
    /// Dimensions of the render target.
    width: u32,
    height: u32,
    /// Bytes per row, padded to wgpu's COPY_BYTES_PER_ROW_ALIGNMENT (256).
    padded_bytes_per_row: u32,
    /// The texture format (Bgra8UnormSrgb to match iced's production surface format).
    format: wgpu::TextureFormat,
}

/// Raw pixel data read back from the GPU.
/// Note: Data is stored as RGBA after a BGRA→RGBA swizzle during readback,
/// so downstream code (PNG encoding, pixel probing) always works in RGBA order.
pub struct CpuPixels {
    /// RGBA pixel data, row-major, tightly packed (no row padding).
    /// Swizzled from the GPU's native BGRA during readback.
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
}
```

### 1.4 Initialization -- Device and Adapter Without a Surface

```rust
impl HeadlessRenderer {
    /// Create a new headless renderer with the given dimensions.
    ///
    /// `backend_hint` controls which wgpu backend to use:
    /// - `None` -> wgpu picks the best available (Vulkan/DX12 on Windows)
    /// - `Some(wgpu::Backends::VULKAN)` -> force Vulkan
    /// - `Some(wgpu::Backends::GL)` -> force OpenGL (useful for CI without GPU)
    ///
    /// On CI without a physical GPU, pass `force_cpu = true` to request
    /// a software rasterizer (Mesa llvmpipe / SwiftShader / warp).
    pub async fn new(
        width: u32,
        height: u32,
        backend_hint: Option<wgpu::Backends>,
        force_cpu: bool,
    ) -> Result<Self, HeadlessError> {
        // --- 1. Create instance ---
        let backends = backend_hint.unwrap_or(wgpu::Backends::all());
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends,
            ..Default::default()
        });

        // --- 2. Request adapter ---
        // No surface -- headless. Power preference selects discrete GPU
        // unless force_cpu, in which case we want the software adapter.
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: if force_cpu {
                    wgpu::PowerPreference::LowPower
                } else {
                    wgpu::PowerPreference::HighPerformance
                },
                force_fallback_adapter: force_cpu,
                compatible_surface: None, // No surface -- headless
            })
            .await
            .ok_or(HeadlessError::NoAdapter)?;

        // Log the adapter for debugging
        let info = adapter.get_info();
        eprintln!(
            "[HeadlessRenderer] Adapter: {} ({:?}, {:?})",
            info.name, info.backend, info.device_type,
        );

        // --- 3. Request device ---
        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("headless_device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::downlevel_defaults(),
                    memory_hints: Default::default(),
                },
                None, // No trace path
            )
            .await
            .map_err(HeadlessError::DeviceRequest)?;

        // --- 4. Create render target texture ---
        let format = wgpu::TextureFormat::Bgra8UnormSrgb;

        let target_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("headless_render_target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            // RENDER_ATTACHMENT: we render into this texture.
            // COPY_SRC: we copy from this texture to the readback buffer.
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                 | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });

        let target_view = target_texture.create_view(&wgpu::TextureViewDescriptor::default());

        // --- 5. Create readback buffer ---
        // wgpu requires rows to be aligned to COPY_BYTES_PER_ROW_ALIGNMENT (256).
        let unpadded_bytes_per_row = width * 4; // RGBA = 4 bytes per pixel
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded_bytes_per_row = (unpadded_bytes_per_row + align - 1) / align * align;
        let buffer_size = (padded_bytes_per_row * height) as u64;

        let readback_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("headless_readback"),
            size: buffer_size,
            // MAP_READ: CPU can read from this buffer.
            // COPY_DST: GPU can copy into this buffer.
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Ok(Self {
            device,
            queue,
            target_texture,
            target_view,
            readback_buffer,
            width,
            height,
            padded_bytes_per_row,
            format,
        })
    }
}
```

### 1.5 Rendering and Pixel Readback

```rust
impl HeadlessRenderer {
    /// Get references to device and queue for pipeline creation.
    pub fn device(&self) -> &wgpu::Device { &self.device }
    pub fn queue(&self) -> &wgpu::Queue { &self.queue }
    pub fn format(&self) -> wgpu::TextureFormat { self.format }
    pub fn width(&self) -> u32 { self.width }
    pub fn height(&self) -> u32 { self.height }

    /// Execute a rendering closure against the offscreen target, then
    /// read the pixels back to CPU memory.
    ///
    /// The closure receives a `CommandEncoder` and the target `TextureView`.
    /// It should create render passes against the target view, exactly as
    /// the iced Shader widget's `render()` method does -- except the target
    /// is our offscreen texture instead of the window surface.
    pub async fn render_and_readback<F>(&self, render_fn: F) -> Result<CpuPixels, HeadlessError>
    where
        F: FnOnce(&mut wgpu::CommandEncoder, &wgpu::TextureView),
    {
        // --- Step 1: Encode render commands ---
        let mut encoder = self.device.create_command_encoder(
            &wgpu::CommandEncoderDescriptor {
                label: Some("headless_encoder"),
            },
        );

        // Let the caller record their render passes.
        render_fn(&mut encoder, &self.target_view);

        // --- Step 2: Copy texture to readback buffer ---
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.target_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.readback_buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.padded_bytes_per_row),
                    rows_per_image: Some(self.height),
                },
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );

        // --- Step 3: Submit and wait ---
        self.queue.submit(std::iter::once(encoder.finish()));

        // --- Step 4: Map the readback buffer ---
        let buffer_slice = self.readback_buffer.slice(..);

        // wgpu async map: request mapping, then await the callback.
        let (tx, rx) = tokio::sync::oneshot::channel();
        buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });

        // Poll the device until the map completes.
        self.device.poll(wgpu::Maintain::Wait);
        rx.await
            .map_err(|_| HeadlessError::MapCancelled)?
            .map_err(HeadlessError::MapFailed)?;

        // --- Step 5: Read pixels, strip row padding ---
        let mapped = buffer_slice.get_mapped_range();
        let unpadded_bytes_per_row = (self.width * 4) as usize;
        let padded = self.padded_bytes_per_row as usize;

        let mut pixels = Vec::with_capacity((unpadded_bytes_per_row * self.height as usize) as usize);
        for row in 0..self.height as usize {
            let start = row * padded;
            let end = start + unpadded_bytes_per_row;
            pixels.extend_from_slice(&mapped[start..end]);
        }

        drop(mapped);
        self.readback_buffer.unmap();

        // BGRA → RGBA byte swizzle.
        // The render target uses Bgra8UnormSrgb (matching iced's production
        // surface format), but PNG and all downstream pixel inspection code
        // expects RGBA channel order. Swap B and R in-place.
        for pixel in pixels.chunks_exact_mut(4) {
            pixel.swap(0, 2); // B ↔ R
        }

        Ok(CpuPixels {
            data: pixels,
            width: self.width,
            height: self.height,
        })
    }

    /// Convenience: render, readback, and save to PNG in one call.
    pub async fn render_to_png<F>(
        &self,
        render_fn: F,
        output_path: &std::path::Path,
    ) -> Result<CpuPixels, HeadlessError>
    where
        F: FnOnce(&mut wgpu::CommandEncoder, &wgpu::TextureView),
    {
        let pixels = self.render_and_readback(render_fn).await?;
        pixels.save_png(output_path)?;
        Ok(pixels)
    }
}
```

### 1.6 CpuPixels Utility Methods

```rust
impl CpuPixels {
    /// Save as a PNG file.
    ///
    /// The internal `data` is already RGBA (swizzled from BGRA during readback),
    /// so we write it directly as RGBA to the PNG encoder.
    pub fn save_png(&self, path: &std::path::Path) -> Result<(), HeadlessError> {
        let file = std::fs::File::create(path)
            .map_err(|e| HeadlessError::Io(e))?;
        let writer = std::io::BufWriter::new(file);
        let mut encoder = png::Encoder::new(writer, self.width, self.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_srgb(png::SrgbRenderingIntent::Perceptual);
        let mut writer = encoder.write_header()
            .map_err(|e| HeadlessError::PngEncode(e))?;
        writer.write_image_data(&self.data)
            .map_err(|e| HeadlessError::PngEncode(e))?;
        Ok(())
    }

    /// Read a pixel at (x, y). Returns [R, G, B, A].
    #[inline]
    pub fn pixel_at(&self, x: u32, y: u32) -> [u8; 4] {
        assert!(x < self.width && y < self.height, "pixel ({x}, {y}) out of bounds");
        let idx = ((y * self.width + x) * 4) as usize;
        [
            self.data[idx],
            self.data[idx + 1],
            self.data[idx + 2],
            self.data[idx + 3],
        ]
    }

    /// Check if a pixel matches an expected color within a per-channel tolerance.
    pub fn pixel_matches(&self, x: u32, y: u32, expected: [u8; 4], tolerance: u8) -> bool {
        let actual = self.pixel_at(x, y);
        actual.iter().zip(expected.iter()).all(|(a, e)| {
            (*a as i16 - *e as i16).unsigned_abs() <= tolerance as u16
        })
    }

    /// Scan a column (fixed x) and return all y positions where the pixel
    /// matches the given color within tolerance.
    pub fn scan_column(&self, x: u32, color: [u8; 4], tolerance: u8) -> Vec<u32> {
        (0..self.height)
            .filter(|&y| self.pixel_matches(x, y, color, tolerance))
            .collect()
    }

    /// Scan a row (fixed y) and return all x positions where the pixel
    /// matches the given color within tolerance.
    pub fn scan_row(&self, y: u32, color: [u8; 4], tolerance: u8) -> Vec<u32> {
        (0..self.width)
            .filter(|&x| self.pixel_matches(x, y, color, tolerance))
            .collect()
    }

    /// Count the number of pixels matching a color within tolerance.
    pub fn count_color(&self, color: [u8; 4], tolerance: u8) -> usize {
        (0..self.height)
            .flat_map(|y| (0..self.width).map(move |x| (x, y)))
            .filter(|&(x, y)| self.pixel_matches(x, y, color, tolerance))
            .count()
    }

    /// Extract a rectangular sub-region as a new CpuPixels.
    pub fn crop(&self, x: u32, y: u32, w: u32, h: u32) -> CpuPixels {
        assert!(x + w <= self.width && y + h <= self.height);
        let mut data = Vec::with_capacity((w * h * 4) as usize);
        for row in y..(y + h) {
            let start = ((row * self.width + x) * 4) as usize;
            let end = start + (w * 4) as usize;
            data.extend_from_slice(&self.data[start..end]);
        }
        CpuPixels { data, width: w, height: h }
    }
}
```

### 1.7 Synchronous Wrapper for Tests

Tests use `pollster::block_on` or `tokio::test` to avoid async noise:

```rust
// crates/midas-render/src/test_utils.rs

/// Create a HeadlessRenderer using pollster for synchronous tests.
/// Uses whatever GPU is available, falling back to software.
pub fn create_test_renderer(width: u32, height: u32) -> HeadlessRenderer {
    pollster::block_on(async {
        // Try hardware first, fall back to CPU.
        match HeadlessRenderer::new(width, height, None, false).await {
            Ok(r) => r,
            Err(_) => HeadlessRenderer::new(width, height, None, true)
                .await
                .expect("Failed to create headless renderer even with CPU fallback"),
        }
    })
}

/// Render a chart scene and return the pixels synchronously.
pub fn render_sync<F>(renderer: &HeadlessRenderer, render_fn: F) -> CpuPixels
where
    F: FnOnce(&mut wgpu::CommandEncoder, &wgpu::TextureView),
{
    pollster::block_on(renderer.render_and_readback(render_fn))
        .expect("Headless render failed")
}
```

### 1.8 Error Types

```rust
#[derive(Debug, thiserror::Error)]
pub enum HeadlessError {
    #[error("No suitable GPU adapter found")]
    NoAdapter,

    #[error("Failed to request device: {0}")]
    DeviceRequest(wgpu::RequestDeviceError),

    #[error("Buffer map was cancelled")]
    MapCancelled,

    #[error("Buffer map failed: {0}")]
    MapFailed(wgpu::BufferAsyncError),

    #[error("PNG encoding error: {0}")]
    PngEncode(png::EncodingError),

    #[error("I/O error: {0}")]
    Io(std::io::Error),
}
```

### 1.9 Bypass of iced -- Direct Pipeline Usage

The headless renderer does NOT go through iced. It calls the same `SharedPipelines`
creation code and the same instance-buffer-building code, but drives `wgpu` directly
instead of through iced's `Shader` widget. This means:

- `SharedPipelines::new(device, format)` -- same code path as production.
- `ChartGpuResources::new(device, &shared_pipelines, chart_id)` -- same.
- `build_candle_instances(...)`, `build_volume_instances(...)` -- same.
- The `render_fn` closure passed to `render_and_readback` creates a `RenderPass`
  and calls the same `draw_candles`, `draw_volume`, `draw_grid` functions.

The only difference is the render target (our offscreen texture vs. the iced surface).
This tests the actual GPU code, not a mock.

---

## 2. Screenshot Comparison Tests

### 2.1 Overview

The screenshot comparison test pattern:

1. Prepare known input data (synthetic CandleBuffer with deterministic values).
2. Configure a known camera (exact time range, price range, viewport size).
3. Render headlessly to CpuPixels.
4. Compare against a stored reference PNG.
5. If the diff exceeds the tolerance threshold, fail the test and write:
   - The actual PNG (`{test_name}_actual.png`)
   - A diff PNG (`{test_name}_diff.png`) highlighting changed pixels
   - A text summary of the diff stats

### 2.2 The Diff Algorithm

Pixel comparison with per-channel tolerance and a percentage threshold:

```rust
// crates/midas-render/src/test_utils.rs

/// Result of comparing two images.
#[derive(Debug)]
pub struct DiffResult {
    /// Number of pixels that differ beyond the per-channel tolerance.
    pub different_pixels: usize,
    /// Total number of pixels in the image.
    pub total_pixels: usize,
    /// Percentage of pixels that differ (0.0 to 100.0).
    pub diff_percentage: f64,
    /// Maximum per-channel delta observed across all pixels.
    pub max_channel_delta: u8,
    /// Optional diff image: red = changed pixels, green = unchanged.
    pub diff_image: Option<CpuPixels>,
}

/// Compare two CpuPixels images.
///
/// `channel_tolerance`: per-channel allowed delta (0-255). A tolerance of 2
/// accounts for GPU rounding differences across hardware.
///
/// `generate_diff_image`: if true, builds a visual diff image.
pub fn compare_images(
    actual: &CpuPixels,
    expected: &CpuPixels,
    channel_tolerance: u8,
    generate_diff_image: bool,
) -> DiffResult {
    assert_eq!(actual.width, expected.width, "Width mismatch");
    assert_eq!(actual.height, expected.height, "Height mismatch");

    let total = (actual.width * actual.height) as usize;
    let mut diff_count = 0usize;
    let mut max_delta = 0u8;
    let mut diff_data = if generate_diff_image {
        Some(vec![0u8; actual.data.len()])
    } else {
        None
    };

    for i in 0..total {
        let base = i * 4;
        let mut pixel_differs = false;

        for c in 0..4 {
            let a = actual.data[base + c];
            let e = expected.data[base + c];
            let delta = (a as i16 - e as i16).unsigned_abs() as u8;
            if delta > max_delta {
                max_delta = delta;
            }
            if delta > channel_tolerance {
                pixel_differs = true;
            }
        }

        if pixel_differs {
            diff_count += 1;
            if let Some(ref mut d) = diff_data {
                // Red pixel for diff
                d[base] = 255;     // R
                d[base + 1] = 0;   // G
                d[base + 2] = 0;   // B
                d[base + 3] = 255; // A
            }
        } else if let Some(ref mut d) = diff_data {
            // Dim green for matching pixel
            d[base] = 0;
            d[base + 1] = 64;
            d[base + 2] = 0;
            d[base + 3] = 255;
        }
    }

    DiffResult {
        different_pixels: diff_count,
        total_pixels: total,
        diff_percentage: (diff_count as f64 / total as f64) * 100.0,
        max_channel_delta: max_delta,
        diff_image: diff_data.map(|data| CpuPixels {
            data,
            width: actual.width,
            height: actual.height,
        }),
    }
}
```

### 2.3 Reference Image Management

Reference images live in the repository under `tests/reference_images/`:

```
tests/
  reference_images/
    candle_basic_100.png          # 100 candles, default camera
    candle_doji.png               # Doji candle edge case
    candle_zoomed_out_10k.png     # 10K candles with LOD
    grid_lines.png                # Grid line alignment
    volume_bars.png               # Volume overlay
    full_chart_aapl.png           # End-to-end AAPL chart
    crosshair.png                 # Crosshair rendering
```

**Generating/updating reference images:**

```bash
# Run the test suite with the MIDAS_UPDATE_REFS environment variable set.
# When set, tests save their actual output AS the new reference image
# instead of comparing against the existing one.
MIDAS_UPDATE_REFS=1 cargo test --features test-harness -p midas-render
```

Implementation:

```rust
/// Check if we should update reference images instead of comparing.
fn should_update_refs() -> bool {
    std::env::var("MIDAS_UPDATE_REFS").is_ok()
}

/// Path to a reference image for a given test name.
fn reference_path(test_name: &str) -> std::path::PathBuf {
    let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent().unwrap()  // crates/
        .parent().unwrap(); // workspace root
    workspace_root
        .join("tests")
        .join("reference_images")
        .join(format!("{test_name}.png"))
}

/// Path to a test output image (actual, diff).
fn output_path(test_name: &str, suffix: &str) -> std::path::PathBuf {
    let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent().unwrap()
        .parent().unwrap();
    let dir = workspace_root.join("tests").join("output");
    std::fs::create_dir_all(&dir).ok();
    dir.join(format!("{test_name}_{suffix}.png"))
}

/// Assert that a rendered image matches its reference within tolerance.
///
/// - `max_diff_percent`: maximum allowed percentage of differing pixels.
/// - `channel_tolerance`: per-channel allowed delta (typically 2-3).
///
/// If MIDAS_UPDATE_REFS is set, saves actual as the new reference instead.
pub fn assert_screenshot(
    test_name: &str,
    actual: &CpuPixels,
    max_diff_percent: f64,
    channel_tolerance: u8,
) {
    let ref_path = reference_path(test_name);

    if should_update_refs() {
        std::fs::create_dir_all(ref_path.parent().unwrap()).ok();
        actual.save_png(&ref_path).expect("Failed to save reference image");
        eprintln!("[UPDATED] Reference image: {}", ref_path.display());
        return;
    }

    // Always save the actual output for inspection.
    let actual_path = output_path(test_name, "actual");
    actual.save_png(&actual_path).ok();

    // Load reference image.
    if !ref_path.exists() {
        panic!(
            "Reference image not found: {}\n\
             Run with MIDAS_UPDATE_REFS=1 to generate it.",
            ref_path.display()
        );
    }

    let ref_image = load_png_as_cpu_pixels(&ref_path)
        .expect("Failed to load reference image");

    let diff = compare_images(actual, &ref_image, channel_tolerance, true);

    // Save diff image if there are differences.
    if diff.different_pixels > 0 {
        if let Some(ref diff_img) = diff.diff_image {
            let diff_path = output_path(test_name, "diff");
            diff_img.save_png(&diff_path).ok();
            eprintln!("[DIFF] Diff image saved: {}", diff_path.display());
        }
    }

    // Assert within tolerance.
    assert!(
        diff.diff_percentage <= max_diff_percent,
        "Screenshot mismatch for '{test_name}':\n\
         {}/{} pixels differ ({:.2}%), max allowed: {:.2}%\n\
         Max channel delta: {}\n\
         Actual:    {}\n\
         Reference: {}\n\
         Diff:      {}",
        diff.different_pixels,
        diff.total_pixels,
        diff.diff_percentage,
        max_diff_percent,
        diff.max_channel_delta,
        actual_path.display(),
        ref_path.display(),
        output_path(test_name, "diff").display(),
    );
}

/// Load a PNG file as CpuPixels (RGBA8).
fn load_png_as_cpu_pixels(path: &std::path::Path) -> Result<CpuPixels, HeadlessError> {
    let decoder = png::Decoder::new(
        std::io::BufReader::new(
            std::fs::File::open(path).map_err(HeadlessError::Io)?
        )
    );
    let mut reader = decoder.read_info().map_err(|e| {
        HeadlessError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    })?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).map_err(|e| {
        HeadlessError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    })?;
    buf.truncate(info.buffer_size());

    Ok(CpuPixels {
        data: buf,
        width: info.width,
        height: info.height,
    })
}
```

### 2.4 Example Screenshot Test

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::*;
    use crate::headless::HeadlessRenderer;
    use crate::pipelines::SharedPipelines;
    use crate::instance_builder::build_candle_instances;
    use midas_data::candle::CandleBuffer;

    #[test]
    fn screenshot_100_candles_basic() {
        let renderer = create_test_renderer(800, 600);

        // Create shared pipelines using the headless device.
        let shared = SharedPipelines::new(renderer.device(), renderer.format());

        // Build deterministic test data.
        let candles = generate_ascending_candles(100, 150.0, 0.50);
        let camera = Camera2D::for_test(
            &candles,
            800, 600,
            1.0, // DPI scale
        );

        // Build GPU instance data.
        let instances = build_candle_instances(
            &candles.slice(0..candles.len()),
            0..candles.len(),
            &camera,
            1.0, // dpi_scale
        );

        // Upload instances to GPU and render.
        let chart_resources = ChartGpuResources::new_for_test(
            renderer.device(),
            &shared,
            &instances,
        );

        let pixels = render_sync(&renderer, |encoder, target| {
            // Clear to background color.
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("test_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.10, g: 0.10, b: 0.12, a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                ..Default::default()
            });

            // Draw candles using the real draw function.
            draw_candles(&mut pass, &shared, &chart_resources);
        });

        // Compare against reference.
        assert_screenshot(
            "candle_basic_100",
            &pixels,
            0.5,  // max 0.5% pixels may differ (GPU rounding)
            3,    // per-channel tolerance of 3/255
        );
    }
}
```

---

## 3. Pixel-Perfect Alignment Tests

### 3.1 Philosophy

Screenshot comparison catches regressions but does not assert specific geometric
properties. Pixel-perfect alignment tests read the rendered pixels and verify exact
geometric invariants:

- Candle body edges land on exact pixel boundaries (no sub-pixel blur).
- Wick lines are exactly 1 physical pixel wide.
- Grid lines are exactly 1 physical pixel wide.
- Candle body widths are consistent (all the same width in a frame).
- No gaps or overlaps between adjacent candle bodies.

These tests use `CpuPixels::pixel_at`, `scan_column`, and `scan_row` to probe
specific locations in the rendered output.

### 3.2 Wick Width Verification

A wick should be exactly 1 physical pixel wide. At DPI 1.0, this means exactly 1 column
of colored pixels at the wick center X.

```rust
#[test]
fn wick_is_exactly_1px_wide() {
    let renderer = create_test_renderer(400, 300);
    let shared = SharedPipelines::new(renderer.device(), renderer.format());

    // A single tall candle. Open=100, High=110, Low=90, Close=105.
    // The wick extends above and below the body.
    let candles = single_candle_buffer(100.0, 110.0, 90.0, 105.0, 1000);
    let camera = Camera2D::for_test(&candles, 400, 300, 1.0);

    let instances = build_candle_instances(
        &candles.slice(0..1),
        0..1,
        &camera,
        1.0,
    );

    let chart_res = ChartGpuResources::new_for_test(
        renderer.device(), &shared, &instances,
    );

    let pixels = render_sync(&renderer, |encoder, target| {
        let mut pass = begin_clear_pass(encoder, target, BG_COLOR);
        draw_candles(&mut pass, &shared, &chart_res);
    });

    // Find the wick by scanning for candle-colored pixels in the wick region.
    // The wick extends above the body (high > max(open, close)).
    let body_top_y = camera.price_to_y(105.0) as u32; // close > open, so body top = close
    let wick_top_y = camera.price_to_y(110.0) as u32; // high

    // Pick a Y coordinate in the wick-only zone (above the body).
    let probe_y = (wick_top_y + body_top_y) / 2;

    // Scan the row for non-background pixels.
    let bg = srgb_to_u8(BG_COLOR);
    let colored_xs = pixels.scan_row(probe_y, bg, 5)
        .into_iter()
        .filter(|&x| !pixels.pixel_matches(x, probe_y, bg, 5))
        .collect::<Vec<_>>();

    // At DPI 1.0, wick must be exactly 1 pixel wide.
    // Allow the scan to return the actual colored pixels, which should be
    // a contiguous run of exactly 1 pixel.
    assert!(!colored_xs.is_empty(), "Wick not found in rendered image");

    // The non-background pixels at the wick location:
    let wick_row: Vec<u32> = (0..pixels.width)
        .filter(|&x| !pixels.pixel_matches(x, probe_y, bg, 5))
        .collect();

    assert_eq!(
        wick_row.len(), 1,
        "Wick should be exactly 1px wide at DPI 1.0, found {} colored pixels at y={}: {:?}",
        wick_row.len(), probe_y, wick_row,
    );
}
```

### 3.3 Candle Body Edge Alignment

Body edges should land on pixel boundaries, meaning the transition from background to
candle color should be a hard step (no intermediate values).

```rust
#[test]
fn candle_body_edges_are_pixel_aligned() {
    let renderer = create_test_renderer(400, 300);
    let shared = SharedPipelines::new(renderer.device(), renderer.format());

    let candles = single_candle_buffer(100.0, 110.0, 90.0, 105.0, 1000);
    let camera = Camera2D::for_test(&candles, 400, 300, 1.0);

    let instances = build_candle_instances(
        &candles.slice(0..1), 0..1, &camera, 1.0,
    );
    let chart_res = ChartGpuResources::new_for_test(
        renderer.device(), &shared, &instances,
    );

    let pixels = render_sync(&renderer, |encoder, target| {
        let mut pass = begin_clear_pass(encoder, target, BG_COLOR);
        draw_candles(&mut pass, &shared, &chart_res);
    });

    let bg = srgb_to_u8(BG_COLOR);

    // Find the body region by scanning horizontally through the body Y range.
    let body_mid_y = camera.price_to_y(102.5) as u32; // midpoint of open..close

    // Collect all non-background pixels in this row.
    let body_pixels: Vec<u32> = (0..pixels.width)
        .filter(|&x| !pixels.pixel_matches(x, body_mid_y, bg, 5))
        .collect();

    assert!(body_pixels.len() >= 3, "Body too narrow: {:?}", body_pixels);

    // Check that the body is a contiguous block (no gaps).
    for i in 1..body_pixels.len() {
        assert_eq!(
            body_pixels[i], body_pixels[i - 1] + 1,
            "Gap in body pixels at x={}: {:?}", body_pixels[i], body_pixels,
        );
    }

    // Check hard edges: the pixel immediately left of the body and immediately
    // right of the body should be the background color.
    let left_edge = body_pixels[0];
    let right_edge = *body_pixels.last().unwrap();

    if left_edge > 0 {
        assert!(
            pixels.pixel_matches(left_edge - 1, body_mid_y, bg, 5),
            "Left edge is not a hard transition at x={}. \
             Pixel to the left: {:?}, background: {:?}",
            left_edge - 1,
            pixels.pixel_at(left_edge - 1, body_mid_y),
            bg,
        );
    }

    if right_edge + 1 < pixels.width {
        assert!(
            pixels.pixel_matches(right_edge + 1, body_mid_y, bg, 5),
            "Right edge is not a hard transition at x={}. \
             Pixel to the right: {:?}, background: {:?}",
            right_edge + 1,
            pixels.pixel_at(right_edge + 1, body_mid_y),
            bg,
        );
    }
}
```

### 3.4 Grid Line Width Verification

```rust
#[test]
fn horizontal_grid_line_is_1px_tall() {
    let renderer = create_test_renderer(400, 300);
    let shared = SharedPipelines::new(renderer.device(), renderer.format());

    // Place a single horizontal grid line at price 100.0.
    let camera = Camera2D {
        time_start: 0.0,
        time_end: 1_000_000.0,
        price_low: 90.0,
        price_high: 110.0,
        chart_width: 400.0,
        chart_height: 300.0,
        viewport_width: 400,
        viewport_height: 300,
        dpi_scale: 1.0,
        ..Camera2D::default_test()
    };

    let grid_y = camera.price_to_y(100.0);
    let snapped_y = snap_to_pixel(grid_y, 1.0);
    let one_px = 1.0; // 1.0 / dpi_scale at DPI 1.0

    let grid_instance = GridLineInstance {
        rect: [0.0, snapped_y, 400.0, snapped_y + one_px],
        color: [0.3, 0.3, 0.3, 1.0], // Gray, fully opaque for this test
    };

    let chart_res = build_grid_test_resources(
        renderer.device(), &shared, &[grid_instance], &camera,
    );

    let pixels = render_sync(&renderer, |encoder, target| {
        let mut pass = begin_clear_pass(encoder, target, BG_COLOR);
        draw_grid(&mut pass, &shared, &chart_res);
    });

    let bg = srgb_to_u8(BG_COLOR);
    let grid_color_srgb = linear_to_srgb_u8([0.3, 0.3, 0.3, 1.0]);

    // Scan a column in the middle of the chart.
    let probe_x = 200;
    let grid_rows: Vec<u32> = (0..pixels.height)
        .filter(|&y| !pixels.pixel_matches(probe_x, y, bg, 5))
        .collect();

    assert_eq!(
        grid_rows.len(), 1,
        "Horizontal grid line should be exactly 1px tall at DPI 1.0, \
         found {} colored rows at x={}: {:?}",
        grid_rows.len(), probe_x, grid_rows,
    );
}
```

### 3.5 Consistent Candle Body Width

All candle bodies in a single frame must have the same pixel width.

```rust
#[test]
fn all_candles_same_body_width() {
    let renderer = create_test_renderer(800, 400);
    let shared = SharedPipelines::new(renderer.device(), renderer.format());

    let candles = generate_ascending_candles(50, 100.0, 1.0);
    let camera = Camera2D::for_test(&candles, 800, 400, 1.0);

    let instances = build_candle_instances(
        &candles.slice(0..candles.len()),
        0..candles.len(),
        &camera,
        1.0,
    );
    let chart_res = ChartGpuResources::new_for_test(
        renderer.device(), &shared, &instances,
    );

    let pixels = render_sync(&renderer, |encoder, target| {
        let mut pass = begin_clear_pass(encoder, target, BG_COLOR);
        draw_candles(&mut pass, &shared, &chart_res);
    });

    // Measure body widths by scanning a row through the body region of each candle.
    let bg = srgb_to_u8(BG_COLOR);

    // Pick a Y that crosses all candle bodies (middle of the price range).
    let mid_price = 125.0; // ascending from 100, 50 candles at 1.0 step
    let probe_y = camera.price_to_y(mid_price) as u32;

    // Find all contiguous runs of non-background pixels.
    let mut runs: Vec<usize> = Vec::new();
    let mut in_run = false;
    let mut run_len = 0;

    for x in 0..pixels.width {
        let is_bg = pixels.pixel_matches(x, probe_y, bg, 5);
        if !is_bg {
            run_len += 1;
            in_run = true;
        } else if in_run {
            runs.push(run_len);
            run_len = 0;
            in_run = false;
        }
    }
    if in_run {
        runs.push(run_len);
    }

    // All runs should have the same width.
    assert!(!runs.is_empty(), "No candle bodies found at y={probe_y}");

    let expected_width = runs[0];
    for (i, &w) in runs.iter().enumerate() {
        assert_eq!(
            w, expected_width,
            "Candle body {} has width {} but expected {} (from candle 0). All widths: {:?}",
            i, w, expected_width, runs,
        );
    }
}
```

### 3.6 Doji Minimum Height

A doji (open == close) must still be at least 1 physical pixel tall.

```rust
#[test]
fn doji_candle_has_minimum_1px_body() {
    let renderer = create_test_renderer(400, 300);
    let shared = SharedPipelines::new(renderer.device(), renderer.format());

    // Doji: open == close
    let candles = single_candle_buffer(100.0, 105.0, 95.0, 100.0, 500);
    let camera = Camera2D::for_test(&candles, 400, 300, 1.0);

    let instances = build_candle_instances(
        &candles.slice(0..1), 0..1, &camera, 1.0,
    );
    let chart_res = ChartGpuResources::new_for_test(
        renderer.device(), &shared, &instances,
    );

    let pixels = render_sync(&renderer, |encoder, target| {
        let mut pass = begin_clear_pass(encoder, target, BG_COLOR);
        // Only draw body pass (draw_mode=1), skip wick for clarity.
        draw_candle_bodies_only(&mut pass, &shared, &chart_res);
    });

    let bg = srgb_to_u8(BG_COLOR);

    // Find the candle center X.
    let center_x = camera.time_to_x(candles.timestamps[0] as f64) as u32;

    // Scan the column at center_x for non-background pixels.
    let body_ys: Vec<u32> = (0..pixels.height)
        .filter(|&y| !pixels.pixel_matches(center_x, y, bg, 5))
        .collect();

    assert!(
        !body_ys.is_empty(),
        "Doji candle body not visible at x={center_x}",
    );

    // At DPI 1.0, the body should be exactly 1 pixel tall.
    assert!(
        body_ys.len() >= 1,
        "Doji body is {} pixels tall, expected >= 1. Rows: {:?}",
        body_ys.len(), body_ys,
    );
}
```

---

## 4. Data Layer Tests

### 4.1 Binary File Round-Trip

Write a `.midas` file, read it back, verify every field matches.

```rust
// crates/midas-data/tests/binary_roundtrip.rs

use midas_data::binary::{FileHeader, CandleRecord, MmapCandleWriter, MmapCandleFile};
use midas_data::binary::{MAGIC, FLAG_DENSE};
use tempfile::NamedTempFile;

#[test]
fn write_then_read_roundtrip() {
    let tmp = NamedTempFile::new().unwrap();
    let path = tmp.path().to_owned();

    // Write 1000 candles.
    let records: Vec<CandleRecord> = (0..1000)
        .map(|i| CandleRecord {
            timestamp: 1_700_000_000_000 + i * 60_000,
            open: 150.0 + (i as f32) * 0.01,
            high: 150.5 + (i as f32) * 0.01,
            low: 149.5 + (i as f32) * 0.01,
            close: 150.2 + (i as f32) * 0.01,
            volume: 1000 + i as u32,
            _padding: 0,
        })
        .collect();

    {
        let mut writer = MmapCandleWriter::create(
            &path,
            1,       // symbol_id
            60,      // timeframe_secs (1m)
            0,       // flags (sparse)
            "TEST",  // symbol_ascii
        ).unwrap();

        for rec in &records {
            writer.append(rec).unwrap();
        }
    }

    // Read back.
    let reader = MmapCandleFile::open(&path).unwrap();

    // Verify header.
    assert_eq!(reader.header().magic, MAGIC);
    assert_eq!(reader.header().version, 1);
    assert_eq!(reader.header().candle_count, 1000);
    assert_eq!(reader.header().timeframe_secs, 60);
    assert_eq!(reader.header().record_size, 32);
    assert_eq!(reader.header().start_ts, records[0].timestamp);
    assert_eq!(reader.header().end_ts, records[999].timestamp);

    // Verify every record.
    for (i, expected) in records.iter().enumerate() {
        let actual = reader.get(i).unwrap();
        assert_eq!(actual.timestamp, expected.timestamp, "record {i} timestamp");
        assert_eq!(actual.open, expected.open, "record {i} open");
        assert_eq!(actual.high, expected.high, "record {i} high");
        assert_eq!(actual.low, expected.low, "record {i} low");
        assert_eq!(actual.close, expected.close, "record {i} close");
        assert_eq!(actual.volume, expected.volume, "record {i} volume");
    }
}

#[test]
fn header_size_is_128_bytes() {
    assert_eq!(std::mem::size_of::<FileHeader>(), 128);
}

#[test]
fn record_size_is_32_bytes() {
    assert_eq!(std::mem::size_of::<CandleRecord>(), 32);
}

#[test]
fn record_alignment_is_8() {
    assert_eq!(std::mem::align_of::<CandleRecord>(), 8);
}

#[test]
fn dense_mode_o1_access() {
    let tmp = NamedTempFile::new().unwrap();
    let path = tmp.path().to_owned();

    // Write dense data: every minute slot filled.
    let start_ts: i64 = 1_700_000_000_000;
    let tf_ms: i64 = 60_000;
    let count = 500;

    {
        let mut writer = MmapCandleWriter::create(
            &path, 1, 60, FLAG_DENSE, "TEST",
        ).unwrap();

        for i in 0..count {
            writer.append(&CandleRecord {
                timestamp: start_ts + i * tf_ms,
                open: 100.0,
                high: 101.0,
                low: 99.0,
                close: 100.5,
                volume: 1000,
                _padding: 0,
            }).unwrap();
        }
    }

    let reader = MmapCandleFile::open(&path).unwrap();

    // O(1) random access: query timestamp at slot 250.
    let target_ts = start_ts + 250 * tf_ms;
    let range = reader.time_range(target_ts, target_ts);
    assert_eq!(range, 250..251, "Dense O(1) lookup failed");
}
```

### 4.2 SoA CandleBuffer Operations

```rust
// crates/midas-data/tests/candle_buffer.rs

use midas_data::candle::{CandleBuffer, CandleRecord};

#[test]
fn push_and_len() {
    let mut buf = CandleBuffer::new();
    assert!(buf.is_empty());

    buf.push(1000, 100.0, 105.0, 95.0, 102.0, 500);
    buf.push(2000, 102.0, 108.0, 101.0, 106.0, 600);

    assert_eq!(buf.len(), 2);
    assert_eq!(buf.timestamps[0], 1000);
    assert_eq!(buf.timestamps[1], 2000);
}

#[test]
fn price_range_scan() {
    let mut buf = CandleBuffer::new();
    for i in 0..100 {
        buf.push(
            i * 60_000,
            100.0 + i as f32,           // open
            110.0 + i as f32,           // high (max at i=99: 209)
            90.0 + i as f32,            // low  (min at i=0: 90)
            105.0 + i as f32,           // close
            1000,
        );
    }

    let (min_low, max_high) = buf.price_range(0..100);
    assert_eq!(min_low, 90.0);
    assert_eq!(max_high, 209.0);
}

#[test]
fn price_range_subrange() {
    let mut buf = CandleBuffer::new();
    for i in 0..100 {
        buf.push(
            i * 60_000,
            100.0 + i as f32,
            110.0 + i as f32,
            90.0 + i as f32,
            105.0 + i as f32,
            1000,
        );
    }

    // Range 50..60: lows from 140..150, highs from 160..170
    let (min_low, max_high) = buf.price_range(50..60);
    assert_eq!(min_low, 140.0);
    assert_eq!(max_high, 169.0);
}

#[test]
fn visible_range_binary_search() {
    let mut buf = CandleBuffer::new();
    let base_ts: i64 = 1_700_000_000_000;
    for i in 0..1000 {
        buf.push(
            base_ts + i * 60_000,
            100.0, 101.0, 99.0, 100.5,
            1000,
        );
    }

    // Query a window that covers candles 100..200.
    let start = base_ts + 100 * 60_000;
    let end = base_ts + 199 * 60_000;
    let range = buf.visible_range(start, end);
    assert_eq!(range, 100..200);
}

#[test]
fn from_records_skips_sentinels() {
    let records = vec![
        CandleRecord {
            timestamp: 1000, open: 100.0, high: 105.0, low: 95.0,
            close: 102.0, volume: 500, _padding: 0,
        },
        CandleRecord {
            // Sentinel: NaN open, zero volume
            timestamp: 2000, open: f32::NAN, high: f32::NAN, low: f32::NAN,
            close: f32::NAN, volume: 0, _padding: 0,
        },
        CandleRecord {
            timestamp: 3000, open: 103.0, high: 108.0, low: 101.0,
            close: 106.0, volume: 600, _padding: 0,
        },
    ];

    let buf = CandleBuffer::from_records(&records);
    assert_eq!(buf.len(), 2, "Sentinel should be skipped");
    assert_eq!(buf.timestamps[0], 1000);
    assert_eq!(buf.timestamps[1], 3000);
}

#[test]
fn slice_view_no_allocation() {
    let mut buf = CandleBuffer::new();
    for i in 0..100 {
        buf.push(i * 1000, 100.0, 101.0, 99.0, 100.5, 1000);
    }

    let slice = buf.slice(10..20);
    assert_eq!(slice.timestamps.len(), 10);
    assert_eq!(slice.timestamps[0], 10_000);
    assert_eq!(slice.timestamps[9], 19_000);

    // Verify it is a true borrow (same memory).
    assert_eq!(
        slice.opens.as_ptr(),
        buf.opens[10..20].as_ptr(),
        "CandleSlice should borrow directly from CandleBuffer",
    );
}

#[test]
fn update_last() {
    let mut buf = CandleBuffer::new();
    buf.push(1000, 100.0, 105.0, 95.0, 102.0, 500);
    buf.push(2000, 102.0, 108.0, 101.0, 106.0, 600);

    buf.update_last(2000, 103.0, 110.0, 100.0, 109.0, 700);

    assert_eq!(buf.len(), 2);
    assert_eq!(buf.opens[1], 103.0);
    assert_eq!(buf.highs[1], 110.0);
    assert_eq!(buf.lows[1], 100.0);
    assert_eq!(buf.closes[1], 109.0);
    assert_eq!(buf.volumes[1], 700);
}
```

### 4.3 LOD Downsampling Tests

```rust
// crates/midas-data/tests/lod.rs

use midas_data::candle::CandleBuffer;
use midas_data::lod::{downsample_minmax, compute_lod, lttb_indices};

#[test]
fn downsample_preserves_price_envelope() {
    let mut buf = CandleBuffer::new();
    // 1000 candles with known extremes.
    for i in 0..1000 {
        let base = 100.0 + (i as f32 * 0.1);
        buf.push(
            i * 60_000,
            base,
            base + 5.0 + (i % 10) as f32,   // high: varies 5..14 above base
            base - 3.0 - (i % 7) as f32,     // low: varies 3..9 below base
            base + 1.0,
            1000 + i as u32,
        );
    }

    let slice = buf.slice(0..buf.len());
    let downsampled = downsample_minmax(&slice, 100);

    assert_eq!(downsampled.len(), 100, "Should produce exactly 100 super-candles");

    // The global high of the downsampled data must equal the global high
    // of the source data.
    let (src_low, src_high) = buf.price_range(0..buf.len());
    let (ds_low, ds_high) = downsampled.price_range(0..downsampled.len());

    assert_eq!(ds_high, src_high, "Downsampling must preserve max high");
    assert_eq!(ds_low, src_low, "Downsampling must preserve min low");
}

#[test]
fn downsample_first_open_last_close() {
    let mut buf = CandleBuffer::new();
    for i in 0..100 {
        buf.push(i * 60_000, 100.0 + i as f32, 150.0, 50.0, 110.0 + i as f32, 1000);
    }

    let slice = buf.slice(0..100);
    let ds = downsample_minmax(&slice, 10);

    // Bucket 0 covers candles 0..10.
    assert_eq!(ds.opens[0], 100.0, "First open of bucket 0");
    assert_eq!(ds.closes[0], 119.0, "Last close of bucket 0 (candle 9)");

    // Bucket 9 covers candles 90..100.
    assert_eq!(ds.opens[9], 190.0, "First open of bucket 9");
    assert_eq!(ds.closes[9], 209.0, "Last close of bucket 9 (candle 99)");
}

#[test]
fn downsample_volume_is_sum() {
    let mut buf = CandleBuffer::new();
    for i in 0..100 {
        buf.push(i * 60_000, 100.0, 101.0, 99.0, 100.5, 100 + i as u32);
    }

    let slice = buf.slice(0..100);
    let ds = downsample_minmax(&slice, 10);

    // Bucket 0: volumes 100..110, sum = 1045
    let expected_vol: u32 = (100..110).sum();
    assert_eq!(ds.volumes[0], expected_vol);
}

#[test]
fn downsample_noop_when_already_small() {
    let mut buf = CandleBuffer::new();
    for i in 0..50 {
        buf.push(i * 60_000, 100.0, 101.0, 99.0, 100.5, 1000);
    }

    let slice = buf.slice(0..50);
    let ds = downsample_minmax(&slice, 100); // target > source

    assert_eq!(ds.len(), 50, "Should return source unchanged when no downsampling needed");
}

#[test]
fn compute_lod_thresholds() {
    // 10K candles, 1920px viewport -> 2*1920 = 3840 max useful.
    // 10K > 3840, so downsampling is needed.
    let (target, bucket_size) = compute_lod(10_000, 1920);
    assert_eq!(target, 3840);
    assert!(bucket_size >= 2);

    // 2000 candles, 1920px viewport -> 2*1920 = 3840. 2000 < 3840 -> no LOD.
    let (target, bucket_size) = compute_lod(2_000, 1920);
    assert_eq!(target, 2_000);
    assert_eq!(bucket_size, 1);
}

#[test]
fn lttb_preserves_endpoints() {
    let timestamps: Vec<i64> = (0..1000).map(|i| i * 1000).collect();
    let values: Vec<f32> = (0..1000).map(|i| (i as f32 * 0.1).sin()).collect();

    let indices = lttb_indices(&timestamps, &values, 100);

    assert_eq!(indices.len(), 100);
    assert_eq!(indices[0], 0, "LTTB must keep first point");
    assert_eq!(indices[99], 999, "LTTB must keep last point");
}
```

### 4.4 CSV Import Tests

```rust
// crates/midas-feed/tests/csv_import.rs

use midas_feed::csv::import_csv;
use midas_data::candle::CandleBuffer;
use std::io::Write;
use tempfile::NamedTempFile;

#[test]
fn import_standard_ohlcv_csv() {
    let csv_content = "\
Date,Open,High,Low,Close,Volume
2024-01-02,150.00,152.50,149.50,151.80,5000000
2024-01-03,151.80,153.00,150.20,152.90,4500000
2024-01-04,152.90,154.10,152.00,153.50,4800000
";
    let mut tmp = NamedTempFile::new().unwrap();
    tmp.write_all(csv_content.as_bytes()).unwrap();
    tmp.flush().unwrap();

    let buf = import_csv(tmp.path(), "AAPL", 86400).unwrap();

    assert_eq!(buf.len(), 3);
    assert_eq!(buf.opens[0], 150.00);
    assert_eq!(buf.highs[0], 152.50);
    assert_eq!(buf.lows[0], 149.50);
    assert_eq!(buf.closes[0], 151.80);
    assert_eq!(buf.volumes[0], 5_000_000);
}

#[test]
fn import_handles_bom_and_trailing_whitespace() {
    // UTF-8 BOM + trailing whitespace on header line
    let csv_content = "\u{FEFF}Date , Open , High , Low , Close , Volume \n\
2024-01-02,100.0,101.0,99.0,100.5,1000\n";

    let mut tmp = NamedTempFile::new().unwrap();
    tmp.write_all(csv_content.as_bytes()).unwrap();

    let buf = import_csv(tmp.path(), "TEST", 86400).unwrap();
    assert_eq!(buf.len(), 1);
}

#[test]
fn import_rejects_out_of_order_timestamps() {
    let csv_content = "\
Date,Open,High,Low,Close,Volume
2024-01-03,100.0,101.0,99.0,100.5,1000
2024-01-02,100.0,101.0,99.0,100.5,1000
";
    let mut tmp = NamedTempFile::new().unwrap();
    tmp.write_all(csv_content.as_bytes()).unwrap();

    let result = import_csv(tmp.path(), "TEST", 86400);
    assert!(result.is_err(), "Should reject out-of-order timestamps");
}

#[test]
fn import_empty_csv_returns_empty_buffer() {
    let csv_content = "Date,Open,High,Low,Close,Volume\n";
    let mut tmp = NamedTempFile::new().unwrap();
    tmp.write_all(csv_content.as_bytes()).unwrap();

    let buf = import_csv(tmp.path(), "TEST", 86400).unwrap();
    assert!(buf.is_empty());
}

#[test]
fn import_various_date_formats() {
    // Test that common date formats are parsed correctly.
    let csv_content = "\
Date,Open,High,Low,Close,Volume
2024-01-02 09:30:00,100.0,101.0,99.0,100.5,1000
2024-01-02 09:31:00,100.5,101.5,100.0,101.0,1100
";
    let mut tmp = NamedTempFile::new().unwrap();
    tmp.write_all(csv_content.as_bytes()).unwrap();

    let buf = import_csv(tmp.path(), "TEST", 60).unwrap();
    assert_eq!(buf.len(), 2);

    // Timestamps should be 60 seconds apart.
    assert_eq!(buf.timestamps[1] - buf.timestamps[0], 60_000);
}
```

### 4.5 Timeframe Boundary Tests

```rust
// crates/midas-core/tests/timeframe.rs

use midas_core::timeframe::Timeframe;

#[test]
fn timeframe_seconds() {
    assert_eq!(Timeframe::M1.seconds(), 60);
    assert_eq!(Timeframe::M5.seconds(), 300);
    assert_eq!(Timeframe::M15.seconds(), 900);
    assert_eq!(Timeframe::H1.seconds(), 3600);
    assert_eq!(Timeframe::H4.seconds(), 14400);
    assert_eq!(Timeframe::D1.seconds(), 86400);
    assert_eq!(Timeframe::W1.seconds(), 604800);
}

#[test]
fn timeframe_floor_timestamp() {
    let ts = 1_700_000_123_456i64; // Some arbitrary timestamp in ms

    // Floor to 1-minute boundary.
    let floored = Timeframe::M1.floor_timestamp(ts);
    assert_eq!(floored % 60_000, 0);
    assert!(floored <= ts);
    assert!(floored + 60_000 > ts);
}

#[test]
fn timeframe_ceil_timestamp() {
    let ts = 1_700_000_123_456i64;

    let ceiled = Timeframe::M5.ceil_timestamp(ts);
    assert_eq!(ceiled % 300_000, 0);
    assert!(ceiled >= ts);
    assert!(ceiled - 300_000 < ts);
}

#[test]
fn timeframe_parsing() {
    assert_eq!(Timeframe::from_str("1m"), Ok(Timeframe::M1));
    assert_eq!(Timeframe::from_str("5m"), Ok(Timeframe::M5));
    assert_eq!(Timeframe::from_str("1H"), Ok(Timeframe::H1));
    assert_eq!(Timeframe::from_str("1D"), Ok(Timeframe::D1));
    assert_eq!(Timeframe::from_str("1W"), Ok(Timeframe::W1));
    assert!(Timeframe::from_str("invalid").is_err());
}
```

---

## 5. Interaction Tests

### 5.1 Design: Pure State Tests

Interaction tests verify the ChartState / Camera2D mutation logic without any GPU
rendering. They apply input operations (zoom, pan, auto-scale) to a ChartState and
verify the resulting camera parameters. These are pure Rust unit tests with no wgpu
dependency.

### 5.2 Zoom Tests

```rust
// crates/midas-chart/tests/camera_zoom.rs

use midas_chart::camera::Camera2D;

fn test_camera() -> Camera2D {
    let mut cam = Camera2D {
        time_start: 0.0,
        time_end: 1_000_000.0,
        price_low: 90.0,
        price_high: 110.0,
        chart_width: 800.0,
        chart_height: 600.0,
        viewport_width: 800,
        viewport_height: 600,
        dpi_scale: 1.0,
        y_axis_width: 0.0,
        x_axis_height: 0.0,
        ..Camera2D::default_test()
    };
    cam.recalculate();
    cam
}

#[test]
fn zoom_in_narrows_time_range() {
    let mut cam = test_camera();
    let original_range = cam.time_end - cam.time_start;

    // Zoom in 2x centered at the middle of the chart.
    cam.zoom_time(400.0, 2.0);

    let new_range = cam.time_end - cam.time_start;
    assert!(
        (new_range - original_range / 2.0).abs() < 1.0,
        "Zoom 2x should halve the time range. Was {original_range}, now {new_range}",
    );
}

#[test]
fn zoom_out_widens_time_range() {
    let mut cam = test_camera();
    let original_range = cam.time_end - cam.time_start;

    cam.zoom_time(400.0, 0.5);

    let new_range = cam.time_end - cam.time_start;
    assert!(
        (new_range - original_range * 2.0).abs() < 1.0,
        "Zoom 0.5x should double the time range. Was {original_range}, now {new_range}",
    );
}

#[test]
fn zoom_preserves_center_point() {
    let mut cam = test_camera();

    // Zoom centered at pixel 300 (not the midpoint).
    let center_x = 300.0;
    let time_at_center_before = cam.x_to_time(center_x);

    cam.zoom_time(center_x, 2.0);

    let time_at_center_after = cam.x_to_time(center_x);

    assert!(
        (time_at_center_before - time_at_center_after).abs() < 1.0,
        "Zoom pivot should preserve the time at the zoom center. \
         Before: {time_at_center_before}, After: {time_at_center_after}",
    );
}

#[test]
fn zoom_has_minimum_candle_count() {
    let mut cam = test_camera();

    // Zoom in many times. Should not zoom past the minimum.
    for _ in 0..100 {
        cam.zoom_time(400.0, 2.0);
    }

    let range = cam.time_end - cam.time_start;
    assert!(range > 0.0, "Time range must never collapse to zero");
    // With a reasonable minimum (e.g., 5 candles of 60s = 300,000ms):
    // The exact minimum depends on implementation, but range should be positive.
}
```

### 5.3 Pan Tests

```rust
// crates/midas-chart/tests/camera_pan.rs

use midas_chart::camera::Camera2D;

#[test]
fn pan_right_shifts_time_earlier() {
    let mut cam = test_camera();
    let original_start = cam.time_start;

    // Pan right by 100 pixels.
    cam.pan(100.0, 0.0);

    assert!(
        cam.time_start < original_start,
        "Panning right (dragging to the right) should shift time earlier. \
         Original start: {original_start}, New start: {}",
        cam.time_start,
    );
}

#[test]
fn pan_preserves_time_range_width() {
    let mut cam = test_camera();
    let original_range = cam.time_end - cam.time_start;

    cam.pan(50.0, 30.0);

    let new_range = cam.time_end - cam.time_start;
    assert!(
        (new_range - original_range).abs() < 0.001,
        "Pan should not change the time range width. Was {original_range}, now {new_range}",
    );
}

#[test]
fn pan_down_shifts_price_up() {
    let mut cam = test_camera();
    let original_low = cam.price_low;

    // Pan down by 50 pixels.
    cam.pan(0.0, 50.0);

    assert!(
        cam.price_low > original_low,
        "Panning down should shift prices up (price_low increases). \
         Was {original_low}, now {}",
        cam.price_low,
    );
}
```

### 5.4 Auto-Scale Tests

```rust
// crates/midas-chart/tests/auto_scale.rs

use midas_chart::camera::Camera2D;
use midas_data::candle::CandleBuffer;

#[test]
fn auto_scale_fits_visible_price_range() {
    let mut cam = test_camera();
    let mut candles = CandleBuffer::new();

    // Visible candles span price range 95..115.
    for i in 0..100 {
        candles.push(
            (cam.time_start as i64) + i * 10_000,
            100.0 + (i % 10) as f32,
            115.0,
            95.0,
            105.0,
            1000,
        );
    }

    let visible = candles.visible_range(cam.time_start as i64, cam.time_end as i64);
    let (data_low, data_high) = candles.price_range(visible);

    // Apply auto-scale with 5% padding.
    let padding = 0.05;
    let data_range = data_high as f64 - data_low as f64;
    let padded_low = data_low as f64 - data_range * padding;
    let padded_high = data_high as f64 + data_range * padding;

    cam.price_low = padded_low;
    cam.price_high = padded_high;
    cam.recalculate();

    // The camera should now contain all prices.
    assert!(cam.price_low < data_low as f64);
    assert!(cam.price_high > data_high as f64);

    // The padding should be approximately 5%.
    let actual_padding_low = (data_low as f64 - cam.price_low) / data_range;
    let actual_padding_high = (cam.price_high - data_high as f64) / data_range;

    assert!(
        (actual_padding_low - padding).abs() < 0.001,
        "Low padding: expected {padding}, got {actual_padding_low}",
    );
    assert!(
        (actual_padding_high - padding).abs() < 0.001,
        "High padding: expected {padding}, got {actual_padding_high}",
    );
}
```

### 5.5 Coordinate Transform Round-Trip

```rust
// crates/midas-chart/tests/coordinate_transforms.rs

use midas_chart::camera::Camera2D;

#[test]
fn time_x_roundtrip() {
    let cam = test_camera();

    for x in [0.0f32, 100.0, 400.0, 799.0] {
        let time = cam.x_to_time(x);
        let x_back = cam.time_to_x(time);
        assert!(
            (x - x_back).abs() < 0.01,
            "Round-trip failed for x={x}: time={time}, x_back={x_back}",
        );
    }
}

#[test]
fn price_y_roundtrip() {
    let cam = test_camera();

    for y in [0.0f32, 100.0, 300.0, 599.0] {
        let price = cam.y_to_price(y);
        let y_back = cam.price_to_y(price);
        assert!(
            (y - y_back).abs() < 0.01,
            "Round-trip failed for y={y}: price={price}, y_back={y_back}",
        );
    }
}

#[test]
fn price_to_y_is_inverted() {
    let cam = test_camera();

    let y_high = cam.price_to_y(110.0); // high price = top of screen = low Y
    let y_low = cam.price_to_y(90.0);   // low price = bottom of screen = high Y

    assert!(
        y_high < y_low,
        "Higher prices should map to lower Y values. y(110)={y_high}, y(90)={y_low}",
    );
}

#[test]
fn projection_matrix_maps_corners_to_ndc() {
    let cam = test_camera();
    let proj = cam.projection_matrix();

    // Top-left pixel (0, 0) should map to NDC (-1, +1).
    let tl = proj * glam::Vec4::new(0.0, 0.0, 0.0, 1.0);
    assert!((tl.x - (-1.0)).abs() < 0.001, "Top-left x: expected -1, got {}", tl.x);
    assert!((tl.y - 1.0).abs() < 0.001, "Top-left y: expected +1, got {}", tl.y);

    // Bottom-right pixel (800, 600) should map to NDC (+1, -1).
    let br = proj * glam::Vec4::new(800.0, 600.0, 0.0, 1.0);
    assert!((br.x - 1.0).abs() < 0.001, "Bottom-right x: expected +1, got {}", br.x);
    assert!((br.y - (-1.0)).abs() < 0.001, "Bottom-right y: expected -1, got {}", br.y);
}
```

---

## 6. Layout Tests

### 6.1 Split-Tree Layout: Pure Geometry Tests

These tests verify the `WorkspaceLayout` binary split tree without any rendering
or iced dependency. They create layouts, perform operations (split, resize, close),
and verify the computed pixel rectangles.

```rust
// crates/midas-core/tests/layout.rs

use midas_core::layout::{WorkspaceLayout, PaneId, PaneRect, Axis, NodeId};

#[test]
fn single_pane_fills_bounds() {
    let mut layout = WorkspaceLayout::empty();
    let pane = layout.next_pane_id();
    layout.add_single(pane);

    let bounds = PaneRect { x: 0.0, y: 0.0, width: 1920.0, height: 1080.0 };
    let rects = layout.compute_rects(bounds);

    let r = rects.get(&pane).expect("Pane not found in layout");
    assert_eq!(r.x, 0.0);
    assert_eq!(r.y, 0.0);
    assert_eq!(r.width, 1920.0);
    assert_eq!(r.height, 1080.0);
}

#[test]
fn horizontal_split_divides_width() {
    let mut layout = WorkspaceLayout::empty();
    let pane_a = layout.next_pane_id();
    layout.add_single(pane_a);

    let pane_b = layout.next_pane_id();
    layout.split(pane_a, Axis::Horizontal, pane_b, 0.5);

    let bounds = PaneRect { x: 0.0, y: 0.0, width: 1000.0, height: 500.0 };
    let rects = layout.compute_rects(bounds);

    let a = rects.get(&pane_a).unwrap();
    let b = rects.get(&pane_b).unwrap();

    // Accounting for border width (4px):
    // First child gets 50% of (1000 - 4) = 498, positioned at x=0.
    // Second child gets remaining 498, positioned at x=502.
    let border = 4.0;
    let available = 1000.0 - border;
    let half = available * 0.5;

    assert!((a.width - half).abs() < 1.0, "Pane A width: {}", a.width);
    assert!((b.width - half).abs() < 1.0, "Pane B width: {}", b.width);
    assert_eq!(a.height, 500.0);
    assert_eq!(b.height, 500.0);
    assert!(b.x > a.x + a.width, "B should be to the right of A with a border gap");
}

#[test]
fn vertical_split_divides_height() {
    let mut layout = WorkspaceLayout::empty();
    let pane_a = layout.next_pane_id();
    layout.add_single(pane_a);

    let pane_b = layout.next_pane_id();
    layout.split(pane_a, Axis::Vertical, pane_b, 0.5);

    let bounds = PaneRect { x: 0.0, y: 0.0, width: 1000.0, height: 800.0 };
    let rects = layout.compute_rects(bounds);

    let a = rects.get(&pane_a).unwrap();
    let b = rects.get(&pane_b).unwrap();

    assert_eq!(a.width, 1000.0);
    assert_eq!(b.width, 1000.0);
    assert!(b.y > a.y, "B should be below A");
}

#[test]
fn nested_splits_produce_grid() {
    // Split A horizontally to get A|B, then split A vertically to get:
    //   C
    //   -- | B
    //   D
    let mut layout = WorkspaceLayout::empty();
    let a = layout.next_pane_id();
    layout.add_single(a);

    let b = layout.next_pane_id();
    layout.split(a, Axis::Horizontal, b, 0.5);

    let d = layout.next_pane_id();
    layout.split(a, Axis::Vertical, d, 0.5);

    let bounds = PaneRect { x: 0.0, y: 0.0, width: 1000.0, height: 1000.0 };
    let rects = layout.compute_rects(bounds);

    // Should have 3 panes.
    assert_eq!(rects.len(), 3);

    // A (now top-left) and D (bottom-left) should have the same width.
    let ra = rects.get(&a).unwrap();
    let rd = rects.get(&d).unwrap();
    assert!((ra.width - rd.width).abs() < 1.0);

    // B should be on the right half.
    let rb = rects.get(&b).unwrap();
    assert!(rb.x > ra.x + ra.width);
}

#[test]
fn resize_changes_ratio() {
    let mut layout = WorkspaceLayout::empty();
    let a = layout.next_pane_id();
    layout.add_single(a);

    let b = layout.next_pane_id();
    layout.split(a, Axis::Horizontal, b, 0.5);

    // Resize: A gets 70%, B gets 30%.
    layout.resize_split(a, 0.7);

    let bounds = PaneRect { x: 0.0, y: 0.0, width: 1000.0, height: 500.0 };
    let rects = layout.compute_rects(bounds);

    let ra = rects.get(&a).unwrap();
    let rb = rects.get(&b).unwrap();

    // A should be wider than B now.
    assert!(ra.width > rb.width, "A({}) should be wider than B({})", ra.width, rb.width);
}

#[test]
fn close_pane_promotes_sibling() {
    let mut layout = WorkspaceLayout::empty();
    let a = layout.next_pane_id();
    layout.add_single(a);

    let b = layout.next_pane_id();
    layout.split(a, Axis::Horizontal, b, 0.5);

    // Close A. B should fill the entire space.
    layout.close(a);

    let bounds = PaneRect { x: 0.0, y: 0.0, width: 1000.0, height: 500.0 };
    let rects = layout.compute_rects(bounds);

    assert_eq!(rects.len(), 1);
    let rb = rects.get(&b).unwrap();
    assert_eq!(rb.width, 1000.0);
    assert_eq!(rb.height, 500.0);
}

#[test]
fn minimum_panel_size_enforced() {
    let mut layout = WorkspaceLayout::empty();
    let a = layout.next_pane_id();
    layout.add_single(a);

    let b = layout.next_pane_id();
    layout.split(a, Axis::Horizontal, b, 0.5);

    // Try to resize A to 1% (should be clamped to MIN_RATIO=10%).
    layout.resize_split(a, 0.01);

    let bounds = PaneRect { x: 0.0, y: 0.0, width: 1000.0, height: 500.0 };
    let rects = layout.compute_rects(bounds);

    let ra = rects.get(&a).unwrap();
    assert!(ra.width >= 80.0, "Pane A width {} is below minimum 80px", ra.width);
}

#[test]
fn layout_serialization_roundtrip() {
    let mut layout = WorkspaceLayout::empty();
    let a = layout.next_pane_id();
    layout.add_single(a);

    let b = layout.next_pane_id();
    layout.split(a, Axis::Horizontal, b, 0.6);

    let c = layout.next_pane_id();
    layout.split(b, Axis::Vertical, c, 0.4);

    let json = serde_json::to_string(&layout).unwrap();
    let restored: WorkspaceLayout = serde_json::from_str(&json).unwrap();

    let bounds = PaneRect { x: 0.0, y: 0.0, width: 1920.0, height: 1080.0 };
    let rects_original = layout.compute_rects(bounds);
    let rects_restored = restored.compute_rects(bounds);

    assert_eq!(rects_original.len(), rects_restored.len());

    for (pane_id, rect) in &rects_original {
        let restored_rect = rects_restored.get(pane_id).unwrap();
        assert!((rect.x - restored_rect.x).abs() < 0.01);
        assert!((rect.y - restored_rect.y).abs() < 0.01);
        assert!((rect.width - restored_rect.width).abs() < 0.01);
        assert!((rect.height - restored_rect.height).abs() < 0.01);
    }
}
```

---

## 7. Criterion Benchmarks

### 7.1 Organization

Benchmarks live in each crate's `benches/` directory and use Criterion 0.5 with
HTML reports. The workspace Cargo.toml already declares criterion in
`[workspace.dependencies]`.

### 7.2 Rendering N Candles (midas-render)

```rust
// crates/midas-render/benches/render_bench.rs

use criterion::{criterion_group, criterion_main, Criterion, BenchmarkId};
use midas_render::headless::HeadlessRenderer;
use midas_render::pipelines::SharedPipelines;
use midas_render::instance_builder::build_candle_instances;
use midas_render::test_utils::*;
use midas_data::candle::CandleBuffer;

fn bench_render_candles(c: &mut Criterion) {
    let renderer = create_test_renderer(1920, 1080);
    let shared = SharedPipelines::new(renderer.device(), renderer.format());

    let mut group = c.benchmark_group("render_candles");

    for count in [100, 500, 1_000, 5_000, 10_000] {
        let candles = generate_ascending_candles(count, 100.0, 0.1);
        let camera = Camera2D::for_test(&candles, 1920, 1080, 1.0);

        let instances = build_candle_instances(
            &candles.slice(0..candles.len()),
            0..candles.len(),
            &camera,
            1.0,
        );

        let chart_res = ChartGpuResources::new_for_test(
            renderer.device(), &shared, &instances,
        );

        group.bench_with_input(
            BenchmarkId::from_parameter(count),
            &count,
            |b, _| {
                b.iter(|| {
                    pollster::block_on(renderer.render_and_readback(|encoder, target| {
                        let mut pass = begin_clear_pass(encoder, target, BG_COLOR);
                        draw_candles(&mut pass, &shared, &chart_res);
                    }))
                    .unwrap()
                });
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_render_candles);
criterion_main!(benches);
```

### 7.3 Instance Buffer Construction (CPU-side)

```rust
// crates/midas-render/benches/render_bench.rs (continued)

fn bench_build_instances(c: &mut Criterion) {
    let mut group = c.benchmark_group("build_candle_instances");

    for count in [1_000, 5_000, 10_000, 50_000] {
        let candles = generate_ascending_candles(count, 100.0, 0.1);
        let camera = Camera2D::for_test(&candles, 1920, 1080, 1.0);

        group.bench_with_input(
            BenchmarkId::from_parameter(count),
            &count,
            |b, _| {
                b.iter(|| {
                    build_candle_instances(
                        &candles.slice(0..candles.len()),
                        0..candles.len(),
                        &camera,
                        1.0,
                    )
                });
            },
        );
    }

    group.finish();
}
```

### 7.4 LOD Downsampling (midas-data)

```rust
// crates/midas-data/benches/data_bench.rs

use criterion::{criterion_group, criterion_main, Criterion, BenchmarkId};
use midas_data::candle::CandleBuffer;
use midas_data::lod::downsample_minmax;

fn bench_downsample(c: &mut Criterion) {
    let mut group = c.benchmark_group("downsample_minmax");

    for source_count in [10_000, 50_000, 100_000, 500_000] {
        let candles = generate_random_candles(source_count);
        let target = 4000; // Typical target for 1920px viewport

        group.bench_with_input(
            BenchmarkId::from_parameter(source_count),
            &source_count,
            |b, _| {
                let slice = candles.slice(0..candles.len());
                b.iter(|| downsample_minmax(&slice, target));
            },
        );
    }

    group.finish();
}

fn generate_random_candles(count: usize) -> CandleBuffer {
    let mut buf = CandleBuffer::with_capacity(count);
    let mut price = 100.0f32;
    for i in 0..count {
        let delta = ((i * 7 + 3) % 100) as f32 * 0.01 - 0.5; // Deterministic pseudo-random
        price += delta;
        buf.push(
            i as i64 * 60_000,
            price,
            price + 2.0,
            price - 2.0,
            price + delta * 0.5,
            1000 + (i % 500) as u32,
        );
    }
    buf
}

criterion_group!(benches, bench_downsample);
criterion_main!(benches);
```

### 7.5 SoA Price Range Scan (midas-data)

```rust
// crates/midas-data/benches/data_bench.rs (continued)

fn bench_price_range_scan(c: &mut Criterion) {
    let mut group = c.benchmark_group("price_range_scan");

    for count in [1_000, 10_000, 100_000] {
        let candles = generate_random_candles(count);

        group.bench_with_input(
            BenchmarkId::from_parameter(count),
            &count,
            |b, _| {
                b.iter(|| candles.price_range(0..candles.len()));
            },
        );
    }

    group.finish();
}
```

### 7.6 Binary File Read (midas-data)

```rust
fn bench_mmap_read(c: &mut Criterion) {
    // Pre-create a temporary .midas file with 100K records.
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let path = tmp.path().to_owned();

    {
        let mut writer = MmapCandleWriter::create(&path, 1, 60, 0, "BENCH").unwrap();
        for i in 0..100_000u64 {
            writer.append(&CandleRecord {
                timestamp: 1_700_000_000_000 + i as i64 * 60_000,
                open: 100.0, high: 101.0, low: 99.0, close: 100.5,
                volume: 1000, _padding: 0,
            }).unwrap();
        }
    }

    c.bench_function("mmap_open_100k", |b| {
        b.iter(|| {
            let _reader = MmapCandleFile::open(&path).unwrap();
        });
    });

    let reader = MmapCandleFile::open(&path).unwrap();

    c.bench_function("mmap_slice_1000_records", |b| {
        b.iter(|| {
            let _slice = reader.slice(50_000..51_000);
        });
    });

    c.bench_function("mmap_to_candle_buffer_5000", |b| {
        b.iter(|| {
            let records = reader.slice(0..5_000);
            CandleBuffer::from_records(records)
        });
    });
}
```

### 7.7 Running Benchmarks

```bash
# Run all benchmarks.
cargo bench --workspace

# Run a specific benchmark group.
cargo bench -p midas-data -- downsample_minmax

# Generate HTML reports (default with criterion).
# Reports go to target/criterion/{benchmark_name}/report/index.html
```

---

## 8. Integration Test: Full Chart Render

### 8.1 End-to-End Pipeline

This test exercises the full pipeline: CSV import, binary write, mmap read, SoA
conversion, camera setup, GPU instance building, headless render, pixel verification.

```rust
// tests/integration/full_chart_render.rs
// (workspace-level integration test)

use midas_feed::csv::import_csv;
use midas_data::binary::{MmapCandleWriter, MmapCandleFile};
use midas_data::candle::CandleBuffer;
use midas_render::headless::HeadlessRenderer;
use midas_render::pipelines::SharedPipelines;
use midas_render::instance_builder::{build_candle_instances, build_volume_instances, build_grid_instances};
use midas_render::draw::{draw_candles, draw_volume, draw_grid};
use midas_chart::camera::Camera2D;
use midas_render::test_utils::*;
use tempfile::NamedTempFile;

#[test]
fn full_pipeline_csv_to_png() {
    // --- Step 1: Import CSV ---
    let csv_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("data")
        .join("AAPL_1D_sample.csv");

    let candle_buffer = import_csv(&csv_path, "AAPL", 86400)
        .expect("CSV import failed");

    assert!(candle_buffer.len() > 50, "Expected at least 50 candles from sample CSV");

    // --- Step 2: Write to binary .midas file ---
    let tmp = NamedTempFile::new().unwrap();
    let midas_path = tmp.path().to_owned();

    {
        let mut writer = MmapCandleWriter::create(
            &midas_path, 1, 86400, 0, "AAPL",
        ).unwrap();

        for i in 0..candle_buffer.len() {
            writer.append(&midas_data::binary::CandleRecord {
                timestamp: candle_buffer.timestamps[i],
                open: candle_buffer.opens[i],
                high: candle_buffer.highs[i],
                low: candle_buffer.lows[i],
                close: candle_buffer.closes[i],
                volume: candle_buffer.volumes[i],
                _padding: 0,
            }).unwrap();
        }
    }

    // --- Step 3: Read back via mmap ---
    let reader = MmapCandleFile::open(&midas_path).unwrap();
    let all_records = reader.slice(0..reader.record_count());
    let reloaded_buffer = CandleBuffer::from_records(all_records);

    assert_eq!(reloaded_buffer.len(), candle_buffer.len(),
        "Round-trip through binary file lost candles");

    // --- Step 4: Set up camera ---
    let width = 1280;
    let height = 720;
    let dpi_scale = 1.0;

    let mut camera = Camera2D {
        time_start: *reloaded_buffer.timestamps.first().unwrap() as f64,
        time_end: *reloaded_buffer.timestamps.last().unwrap() as f64,
        price_low: 0.0,
        price_high: 0.0,
        chart_width: width as f32 - 80.0,  // Minus Y-axis width
        chart_height: height as f32 - 30.0, // Minus X-axis height
        viewport_width: width,
        viewport_height: height,
        dpi_scale,
        y_axis_width: 80.0,
        x_axis_height: 30.0,
        ..Camera2D::default_test()
    };

    // Auto-scale Y to fit data.
    let visible = reloaded_buffer.visible_range(
        camera.time_start as i64,
        camera.time_end as i64,
    );
    let (data_low, data_high) = reloaded_buffer.price_range(visible.clone());
    let data_range = data_high as f64 - data_low as f64;
    camera.price_low = data_low as f64 - data_range * 0.05;
    camera.price_high = data_high as f64 + data_range * 0.05;
    camera.recalculate();

    // --- Step 5: Build GPU instances ---
    let slice = reloaded_buffer.slice(visible.clone());
    let candle_instances = build_candle_instances(&slice, 0..slice.timestamps.len(), &camera, dpi_scale);
    let volume_instances = build_volume_instances(&slice, 0..slice.timestamps.len(), &camera, dpi_scale, &default_theme());
    let grid_instances = build_grid_instances(&camera, dpi_scale, &default_theme());

    // --- Step 6: Headless render ---
    let renderer = create_test_renderer(width, height);
    let shared = SharedPipelines::new(renderer.device(), renderer.format());

    let chart_res = ChartGpuResources::new_for_test_full(
        renderer.device(),
        &shared,
        &candle_instances,
        &volume_instances,
        &grid_instances,
        &camera,
    );

    let pixels = render_sync(&renderer, |encoder, target| {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("full_chart_test"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.10, g: 0.10, b: 0.12, a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            ..Default::default()
        });

        draw_grid(&mut pass, &shared, &chart_res);
        draw_candles(&mut pass, &shared, &chart_res);
        draw_volume(&mut pass, &shared, &chart_res);
    });

    // --- Step 7: Save and verify ---
    let output_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("output");
    std::fs::create_dir_all(&output_dir).ok();

    let png_path = output_dir.join("full_chart_aapl.png");
    pixels.save_png(&png_path).expect("Failed to save PNG");

    // Basic sanity checks on the rendered output.
    assert_ne!(
        pixels.count_color(srgb_to_u8(BG_COLOR), 5),
        (width * height) as usize,
        "Rendered image is entirely background color -- nothing was drawn",
    );

    // Check that bull and bear candle colors are present.
    let bull_color = linear_to_srgb_u8(BULL_CANDLE_COLOR);
    let bear_color = linear_to_srgb_u8(BEAR_CANDLE_COLOR);

    let bull_count = pixels.count_color(bull_color, 10);
    let bear_count = pixels.count_color(bear_color, 10);

    assert!(bull_count > 0, "No bull (green) candle pixels found");
    assert!(bear_count > 0, "No bear (red) candle pixels found");

    eprintln!(
        "[FULL CHART] Bull pixels: {bull_count}, Bear pixels: {bear_count}, \
         Total non-bg: {}, Output: {}",
        (width * height) as usize - pixels.count_color(srgb_to_u8(BG_COLOR), 5),
        png_path.display(),
    );

    // Screenshot comparison (generates/compares reference).
    assert_screenshot("full_chart_aapl", &pixels, 1.0, 5);
}
```

---

## 9. Test Data

### 9.1 Sample CSV Files

Checked into `tests/data/`:

| File | Description | Records |
|------|-------------|---------|
| `AAPL_1D_sample.csv` | Apple daily OHLCV, 1 year | ~252 rows |
| `SPY_5m_sample.csv` | S&P 500 ETF 5-minute, 1 week | ~1,950 rows |
| `DOJI_edge_cases.csv` | Synthetic: dojis, gaps, zero volume | 50 rows |
| `SINGLE_CANDLE.csv` | One candle only | 1 row |
| `LARGE_10K.csv` | Synthetic daily data | 10,000 rows |
| `EXTREME_PRICES.csv` | Prices from 0.001 to 999,999 | 100 rows |

### 9.2 CSV Generation Script

A Rust binary in `tests/` that generates deterministic synthetic CSV files:

```rust
// tests/generate_test_data.rs
// Run: cargo run --bin generate_test_data

use std::io::Write;

fn main() {
    generate_ascending("tests/data/ASCENDING_100.csv", 100, 150.0, 0.50);
    generate_ascending("tests/data/ASCENDING_1000.csv", 1000, 100.0, 0.10);
    generate_ascending("tests/data/LARGE_10K.csv", 10_000, 50.0, 0.02);
    generate_doji_cases("tests/data/DOJI_edge_cases.csv");
    generate_extreme_prices("tests/data/EXTREME_PRICES.csv");
    generate_single("tests/data/SINGLE_CANDLE.csv");

    eprintln!("Test data generated successfully.");
}

fn generate_ascending(path: &str, count: usize, start_price: f64, step: f64) {
    let mut f = std::fs::File::create(path).unwrap();
    writeln!(f, "Date,Open,High,Low,Close,Volume").unwrap();

    let base_ts = chrono::NaiveDate::from_ymd_opt(2023, 1, 2).unwrap();

    for i in 0..count {
        let date = base_ts + chrono::Duration::days(i as i64);
        let open = start_price + step * i as f64;
        let close = open + step * 0.6;
        let high = close + step * 0.4;
        let low = open - step * 0.3;
        let volume = 1_000_000 + (i % 500) * 10_000;

        writeln!(
            f,
            "{},{:.2},{:.2},{:.2},{:.2},{}",
            date.format("%Y-%m-%d"),
            open, high, low, close, volume,
        ).unwrap();
    }
}

fn generate_doji_cases(path: &str) {
    let mut f = std::fs::File::create(path).unwrap();
    writeln!(f, "Date,Open,High,Low,Close,Volume").unwrap();

    let base = chrono::NaiveDate::from_ymd_opt(2023, 6, 1).unwrap();

    for i in 0..50 {
        let date = base + chrono::Duration::days(i);
        let price = 100.0 + i as f64;

        // Every 5th candle is a perfect doji (open == close).
        let (open, close) = if i % 5 == 0 {
            (price, price)
        } else {
            (price, price + 1.0)
        };

        // Every 10th candle has very high volume.
        let volume = if i % 10 == 0 { 50_000_000 } else { 1_000_000 };

        // Every 7th candle has a tiny price range (near-zero body + wick).
        let (high, low) = if i % 7 == 0 {
            (open + 0.01, open - 0.01)
        } else {
            (open.max(close) + 3.0, open.min(close) - 3.0)
        };

        writeln!(f, "{},{:.4},{:.4},{:.4},{:.4},{}", date.format("%Y-%m-%d"), open, high, low, close, volume).unwrap();
    }
}

fn generate_extreme_prices(path: &str) {
    let mut f = std::fs::File::create(path).unwrap();
    writeln!(f, "Date,Open,High,Low,Close,Volume").unwrap();

    let base = chrono::NaiveDate::from_ymd_opt(2023, 1, 2).unwrap();

    let prices = [
        0.001, 0.01, 0.1, 1.0, 10.0, 100.0, 1000.0, 10000.0, 100000.0, 999999.0,
    ];

    for (i, &price) in prices.iter().enumerate() {
        let date = base + chrono::Duration::days(i as i64);
        writeln!(
            f,
            "{},{:.4},{:.4},{:.4},{:.4},1000000",
            date.format("%Y-%m-%d"),
            price,
            price * 1.05,
            price * 0.95,
            price * 1.02,
        ).unwrap();
    }
}

fn generate_single(path: &str) {
    let mut f = std::fs::File::create(path).unwrap();
    writeln!(f, "Date,Open,High,Low,Close,Volume").unwrap();
    writeln!(f, "2024-01-02,150.00,152.50,149.50,151.80,5000000").unwrap();
}
```

### 9.3 Deterministic Test Data Builders (In-Memory)

Used by unit and rendering tests that do not need CSV files:

```rust
// crates/midas-render/src/test_utils.rs

use midas_data::candle::CandleBuffer;

/// Generate N candles with steadily ascending prices.
/// Deterministic: same inputs always produce the same output.
pub fn generate_ascending_candles(
    count: usize,
    start_price: f32,
    step: f32,
) -> CandleBuffer {
    let mut buf = CandleBuffer::with_capacity(count);
    let base_ts: i64 = 1_700_000_000_000; // Fixed epoch

    for i in 0..count {
        let open = start_price + step * i as f32;
        let close = open + step * 0.6;
        let high = close + step * 0.4;
        let low = open - step * 0.3;
        buf.push(
            base_ts + i as i64 * 60_000,
            open, high, low, close,
            1000 + (i % 500) as u32,
        );
    }
    buf
}

/// Generate a CandleBuffer with a single candle.
pub fn single_candle_buffer(open: f32, high: f32, low: f32, close: f32, volume: u32) -> CandleBuffer {
    let mut buf = CandleBuffer::with_capacity(1);
    buf.push(1_700_000_000_000, open, high, low, close, volume);
    buf
}

/// Create a Camera2D configured to show all candles in the buffer.
/// Deterministic: always produces the same camera for the same input.
impl Camera2D {
    pub fn for_test(
        candles: &CandleBuffer,
        viewport_w: u32,
        viewport_h: u32,
        dpi_scale: f32,
    ) -> Self {
        let ts_first = *candles.timestamps.first().unwrap() as f64;
        let ts_last = *candles.timestamps.last().unwrap() as f64;
        let (data_low, data_high) = candles.price_range(0..candles.len());
        let data_range = data_high as f64 - data_low as f64;

        let mut cam = Camera2D {
            time_start: ts_first - 60_000.0, // Small padding
            time_end: ts_last + 60_000.0,
            price_low: data_low as f64 - data_range * 0.05,
            price_high: data_high as f64 + data_range * 0.05,
            chart_width: viewport_w as f32,
            chart_height: viewport_h as f32,
            viewport_width: viewport_w,
            viewport_height: viewport_h,
            dpi_scale,
            y_axis_width: 0.0,
            x_axis_height: 0.0,
            ..Self::default_test()
        };
        cam.recalculate();
        cam
    }

    pub fn default_test() -> Self {
        Self {
            time_start: 0.0,
            time_end: 1_000_000.0,
            price_low: 0.0,
            price_high: 200.0,
            chart_width: 800.0,
            chart_height: 600.0,
            viewport_width: 800,
            viewport_height: 600,
            dpi_scale: 1.0,
            px_per_ms: 0.0,
            px_per_price: 0.0,
            y_axis_width: 0.0,
            x_axis_height: 0.0,
            target_price_low: 0.0,
            target_price_high: 200.0,
            animating: false,
        }
    }
}
```

---

## 10. CI Considerations

### 10.1 GitHub Actions: wgpu on a Headless Linux Runner

GitHub Actions Linux runners have no physical GPU. wgpu can still run using a
software rasterizer. The options:

| Backend | Software Rasterizer | Setup |
|---------|-------------------|-------|
| Vulkan  | **Mesa llvmpipe** | `sudo apt-get install mesa-vulkan-drivers` + `VK_ICD_FILENAMES` |
| Vulkan  | **SwiftShader** | Google's CPU Vulkan. Higher fidelity but slower. |
| GL      | Mesa llvmpipe (GL) | `LIBGL_ALWAYS_SOFTWARE=1` |
| DX12    | **WARP** | Windows only. Microsoft's reference rasterizer. |

**Recommended CI configuration**: Mesa llvmpipe (Vulkan) on Linux, WARP on Windows.

> **Texture format note**: The headless renderer uses `Bgra8UnormSrgb` to match
> iced 0.14's production surface format on Windows (DX12 and Vulkan both prefer BGRA).
> CI runners must use the same format so that reference images are pixel-comparable
> between local development and CI. If a CI adapter does not support `Bgra8UnormSrgb`,
> the test harness should fail loudly rather than silently fall back to a different
> format, since format mismatches cause systematic channel-swap differences that
> defeat screenshot comparison.

### 10.2 GitHub Actions Workflow (Linux)

```yaml
# .github/workflows/ci.yml

name: CI

on:
  push:
    branches: [main]
  pull_request:

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Install Rust
        uses: dtolnay/rust-toolchain@stable
        with:
          components: clippy, rustfmt

      - name: Install Mesa Vulkan drivers (llvmpipe)
        run: |
          sudo apt-get update
          sudo apt-get install -y mesa-vulkan-drivers libvulkan1 vulkan-tools

      - name: Verify Vulkan
        run: vulkaninfo --summary || echo "Vulkan info unavailable (non-fatal)"

      - name: Cache cargo
        uses: actions/cache@v4
        with:
          path: |
            ~/.cargo/bin/
            ~/.cargo/registry/index/
            ~/.cargo/registry/cache/
            ~/.cargo/git/db/
            target/
          key: ${{ runner.os }}-cargo-${{ hashFiles('**/Cargo.lock') }}

      - name: Check formatting
        run: cargo fmt --all -- --check
        working-directory: desktop/win

      - name: Clippy
        run: cargo clippy --workspace --all-targets -- -D warnings
        working-directory: desktop/win

      - name: Run unit tests
        run: cargo test --workspace --exclude midas-render
        working-directory: desktop/win

      - name: Run render tests (headless, llvmpipe)
        env:
          # Force wgpu to use Vulkan with llvmpipe.
          WGPU_BACKEND: vulkan
          # Ensure Mesa uses software rendering.
          LIBGL_ALWAYS_SOFTWARE: "1"
          # Tell the headless renderer to use the CPU fallback.
          MIDAS_FORCE_CPU_RENDERER: "1"
        run: cargo test --features test-harness -p midas-render
        working-directory: desktop/win

      - name: Run integration tests
        env:
          WGPU_BACKEND: vulkan
          LIBGL_ALWAYS_SOFTWARE: "1"
          MIDAS_FORCE_CPU_RENDERER: "1"
        run: cargo test --features test-harness --test '*'
        working-directory: desktop/win

      - name: Upload test artifacts (screenshots)
        if: always()
        uses: actions/upload-artifact@v4
        with:
          name: test-screenshots
          path: desktop/win/tests/output/
          retention-days: 14

  test-windows:
    runs-on: windows-latest
    steps:
      - uses: actions/checkout@v4

      - name: Install Rust
        uses: dtolnay/rust-toolchain@stable

      - name: Run tests (WARP software renderer)
        env:
          # WARP is available on all Windows machines.
          WGPU_BACKEND: dx12
          MIDAS_FORCE_CPU_RENDERER: "1"
        run: cargo test --workspace --features test-harness
        working-directory: desktop/win

      - name: Upload test artifacts
        if: always()
        uses: actions/upload-artifact@v4
        with:
          name: test-screenshots-windows
          path: desktop/win/tests/output/
          retention-days: 14

  bench:
    runs-on: ubuntu-latest
    if: github.event_name == 'push' && github.ref == 'refs/heads/main'
    steps:
      - uses: actions/checkout@v4

      - name: Install Rust
        uses: dtolnay/rust-toolchain@stable

      - name: Install Mesa
        run: sudo apt-get update && sudo apt-get install -y mesa-vulkan-drivers libvulkan1

      - name: Run benchmarks
        env:
          WGPU_BACKEND: vulkan
          LIBGL_ALWAYS_SOFTWARE: "1"
        run: cargo bench --workspace
        working-directory: desktop/win

      - name: Upload benchmark results
        uses: actions/upload-artifact@v4
        with:
          name: benchmarks
          path: desktop/win/target/criterion/
          retention-days: 30
```

### 10.3 Handling Cross-Platform Pixel Differences

Software rasterizers (llvmpipe, WARP, SwiftShader) do not produce byte-identical
output to hardware GPUs. Differences arise from:

- Floating-point rounding in rasterization.
- sRGB conversion precision.
- Subpixel coverage calculation differences.

**Strategy**:

1. **Reference images are generated on the CI environment itself.** The first CI
   run after a rendering change updates the reference images (triggered by
   `MIDAS_UPDATE_REFS=1`). Subsequent CI runs compare against these references.

2. **Per-platform reference images.** If Linux and Windows produce different output,
   maintain separate reference directories:
   ```
   tests/reference_images/linux/
   tests/reference_images/windows/
   ```
   The test harness selects the correct directory based on `cfg!(target_os)`.

3. **Generous tolerance for CI, strict for local.** The `channel_tolerance` and
   `max_diff_percent` thresholds are higher on CI (where software rendering runs)
   than on the developer's machine (where hardware rendering runs):
   ```rust
   fn ci_tolerances() -> (f64, u8) {
       if std::env::var("CI").is_ok() {
           (2.0, 5) // 2% diff allowed, 5 channel tolerance
       } else {
           (0.5, 3) // 0.5% diff, 3 channel tolerance
       }
   }
   ```

### 10.4 Environment Variable Summary

| Variable | Effect |
|----------|--------|
| `MIDAS_UPDATE_REFS=1` | Save actual screenshots as new reference images |
| `MIDAS_FORCE_CPU_RENDERER=1` | Force wgpu software/fallback adapter |
| `WGPU_BACKEND=vulkan` | Force Vulkan backend (useful on Linux CI) |
| `WGPU_BACKEND=dx12` | Force DX12 backend (useful on Windows CI) |
| `CI=true` | Detected automatically by GitHub Actions. Used for tolerance adjustment |

---

## 11. Visual Regression Workflow

### 11.1 Day-to-Day AI-Assisted Development

When Claude Code modifies rendering code (shaders, instance builders, camera math,
draw calls), the workflow is:

1. **Make the change.** Claude edits the shader or Rust rendering code.

2. **Run the render tests locally.**
   ```bash
   cargo test --features test-harness -p midas-render
   ```

3. **If tests pass**, the change did not break any known visual output.

4. **If tests fail**, Claude examines the diff output:
   - `tests/output/{test_name}_actual.png` -- what the new code produces.
   - `tests/output/{test_name}_diff.png` -- red pixels show changes.
   - The test failure message includes exact pixel counts and percentages.

5. **If the change is intentional** (e.g., new feature, color tweak):
   ```bash
   MIDAS_UPDATE_REFS=1 cargo test --features test-harness -p midas-render
   ```
   This updates the reference images. Claude then commits the new references
   alongside the code change.

6. **If the change is unintentional** (regression), Claude fixes the rendering code.

### 11.2 Validating GPU Output Without Human Eyes

The headless rendering + screenshot pipeline gives Claude Code the ability to
"see" the chart output by reasoning about it in multiple ways:

**Structural verification** (pixel-perfect tests, Section 3):
- Assert exact pixel widths, heights, and positions.
- Verify 1px line widths, edge alignment, contiguous body fills.
- Catch sub-pixel rendering artifacts (blurry lines, off-by-one errors).

**Statistical verification** (color counting):
- Count how many pixels of each expected color are present.
- Verify that bull candles (green) and bear candles (red) appear in expected proportions.
- Check that the background is not covering the entire image (nothing drawn).

**Reference comparison** (screenshot tests, Section 2):
- Catch any unexpected change in visual output.
- Percentage-based threshold catches both major and minor regressions.
- Diff images pinpoint exactly which pixels changed.

**Direct pixel probing** (CpuPixels API):
- Read specific pixel coordinates to verify expected colors.
- Scan rows/columns to find edges, measure widths.
- Crop sub-regions for focused analysis.

### 11.3 PR Review Checklist for Rendering Changes

When a PR modifies rendering code, the review process should include:

- [ ] All existing screenshot tests pass, OR reference images are intentionally updated.
- [ ] New reference images are committed alongside code changes.
- [ ] Pixel-perfect alignment tests pass (wick width, grid line width, body edges).
- [ ] The full chart integration test passes.
- [ ] CI passes on both Linux (llvmpipe) and Windows (WARP).
- [ ] Benchmark results are within acceptable range of previous run.
- [ ] Diff images (if any) are reviewed for unexpected artifacts.

### 11.4 Adding a New Visual Test

When adding a new rendering feature (e.g., crosshair overlay):

1. Write the rendering code.
2. Write a pixel-perfect alignment test that verifies the geometric properties
   (e.g., crosshair is 1px wide, spans full chart width/height).
3. Write a screenshot comparison test with a descriptive name.
4. Run with `MIDAS_UPDATE_REFS=1` to generate the initial reference image.
5. Inspect the reference image manually (or have Claude describe what it should contain
   based on the input data and camera configuration).
6. Commit the reference image.
7. All subsequent runs verify the output has not regressed.

### 11.5 Test Execution Summary

```
cargo test --workspace                                  # Unit + integration (no GPU)
cargo test --features test-harness -p midas-render      # Render tests (needs GPU or CPU fallback)
cargo test --features test-harness --test '*'            # Workspace integration tests
cargo bench --workspace                                 # Performance benchmarks
MIDAS_UPDATE_REFS=1 cargo test --features test-harness  # Update reference images
```

### 11.6 Expected Test Counts by Module

| Crate | Test Category | Approximate Count |
|-------|--------------|-------------------|
| `midas-core` | Camera transforms, coordinate round-trips | 15-20 |
| `midas-core` | Timeframe operations | 8-10 |
| `midas-core` | Layout split-tree | 12-15 |
| `midas-data` | Binary file round-trip | 8-10 |
| `midas-data` | CandleBuffer operations | 10-12 |
| `midas-data` | LOD downsampling | 8-10 |
| `midas-feed` | CSV import | 8-10 |
| `midas-render` | Pixel-perfect alignment | 8-10 |
| `midas-render` | Screenshot comparison | 10-15 |
| `workspace` | Full chart integration | 2-4 |
| **Total** | | **~80-120 tests** |

---

## Appendix A: Helper Function Reference

Summary of test utility functions defined throughout this document:

| Function | Location | Purpose |
|----------|----------|---------|
| `create_test_renderer(w, h)` | `test_utils.rs` | Create HeadlessRenderer with GPU/CPU fallback |
| `render_sync(renderer, fn)` | `test_utils.rs` | Render and readback synchronously |
| `compare_images(actual, expected, tol, diff)` | `test_utils.rs` | Pixel diff with tolerance |
| `assert_screenshot(name, pixels, max_pct, tol)` | `test_utils.rs` | Reference image comparison |
| `generate_ascending_candles(n, start, step)` | `test_utils.rs` | Deterministic test candle data |
| `single_candle_buffer(o, h, l, c, v)` | `test_utils.rs` | Single-candle test data |
| `Camera2D::for_test(candles, w, h, dpi)` | `camera.rs` | Camera fitted to test data |
| `Camera2D::default_test()` | `camera.rs` | Default camera for tests |
| `srgb_to_u8(color)` | `test_utils.rs` | Convert wgpu::Color to [u8; 4] |
| `linear_to_srgb_u8(color)` | `test_utils.rs` | Linear float RGBA to sRGB u8 |
| `begin_clear_pass(enc, target, bg)` | `test_utils.rs` | Start a render pass with clear |
| `should_update_refs()` | `test_utils.rs` | Check MIDAS_UPDATE_REFS env var |
| `reference_path(name)` | `test_utils.rs` | Path to reference PNG |
| `output_path(name, suffix)` | `test_utils.rs` | Path to test output PNG |

## Appendix B: Color Space Helpers

Rendering uses linear-space colors internally, but PNG files and pixel comparisons
operate in sRGB space. These helpers bridge the gap:

```rust
/// Convert a wgpu::Color (linear, 0.0-1.0) to sRGB u8 [R, G, B, A].
pub fn srgb_to_u8(color: wgpu::Color) -> [u8; 4] {
    [
        (color.r * 255.0) as u8,
        (color.g * 255.0) as u8,
        (color.b * 255.0) as u8,
        (color.a * 255.0) as u8,
    ]
}

/// Convert linear-space [f32; 4] RGBA to sRGB [u8; 4].
/// Applies the sRGB gamma curve: out = in^(1/2.2) approximately,
/// or the exact piecewise sRGB transfer function.
pub fn linear_to_srgb_u8(linear: [f32; 4]) -> [u8; 4] {
    fn linear_to_srgb(c: f32) -> u8 {
        let s = if c <= 0.0031308 {
            c * 12.92
        } else {
            1.055 * c.powf(1.0 / 2.4) - 0.055
        };
        (s.clamp(0.0, 1.0) * 255.0).round() as u8
    }

    [
        linear_to_srgb(linear[0]),
        linear_to_srgb(linear[1]),
        linear_to_srgb(linear[2]),
        (linear[3].clamp(0.0, 1.0) * 255.0).round() as u8, // Alpha is linear
    ]
}
```
