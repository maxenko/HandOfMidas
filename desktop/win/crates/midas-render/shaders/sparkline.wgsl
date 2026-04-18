// ============================================================================
// sparkline.wgsl — Mountain (area-fill) renderer for thumbnail chart cells.
//
// Consumes an arbitrary-length `array<f32>` of close prices via a read-only
// storage buffer and emits a non-indexed triangle strip that fills the region
// between the baseline (y = y_min in data space) and the polyline that walks
// across the closes.
//
// Vertex layout for a draw of `2 * count` vertices, using `@builtin(vertex_index)`:
//
//     vid=0  ->  (i=0, baseline=false)   top     at closes[0]
//     vid=1  ->  (i=0, baseline=true)    bottom  at y_min
//     vid=2  ->  (i=1, baseline=false)   top     at closes[1]
//     vid=3  ->  (i=1, baseline=true)    bottom  at y_min
//     ...
//
// With `PrimitiveTopology::TriangleStrip`, every new vertex forms a triangle
// with the previous two, so alternating top/bottom vertices tile out a
// ribbon whose upper edge traces the closes and whose lower edge is flat at
// the data-space baseline. NDC output is generated with clip-space y flipped
// so that `close == y_max` renders at the top of the viewport (the usual
// chart convention).
// ============================================================================

// --- Storage buffer: close prices ---

@group(0) @binding(0)
var<storage, read> closes: array<f32>;

// --- Uniform: normalization bounds, count, fill color ---

struct Uniforms {
    /// RGBA fill color for the mountain area.
    color: vec4<f32>,
    /// Minimum close in data space (baseline; renders at viewport bottom).
    y_min: f32,
    /// Maximum close in data space (renders at viewport top).
    y_max: f32,
    /// Number of close samples that the vertex shader should consume.
    count: u32,
    /// Padding — keep the struct at 32 bytes so std140/std430 agree.
    _pad: u32,
}

@group(0) @binding(1)
var<uniform> uniforms: Uniforms;

// --- Vertex output ---

struct VertexOutput {
    @builtin(position) clip_pos: vec4<f32>,
}

// --- Vertex Shader ---

@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> VertexOutput {
    // Each pair of vertex indices corresponds to one close sample:
    //   even index -> top vertex at the close price
    //   odd  index -> baseline vertex at y_min
    let i = vid / 2u;
    let is_baseline = (vid & 1u) == 1u;

    // Guard: if there are fewer than two samples we can't form a strip.
    // Emit a degenerate vertex at the origin and let the caller avoid
    // issuing the draw in the CPU path.
    let count = uniforms.count;
    if (count < 2u) {
        var degenerate: VertexOutput;
        degenerate.clip_pos = vec4<f32>(0.0, 0.0, 0.0, 1.0);
        return degenerate;
    }

    // Horizontal position in [0, 1] across the viewport.
    let denom = f32(count - 1u);
    let x_norm = f32(i) / denom;

    // Vertical position in [0, 1] — baseline at 0, top at (close - y_min) /
    // (y_max - y_min). A flat slice would yield zero; clamp the denominator
    // to avoid a divide-by-zero and render as a flat baseline in that case.
    let span = max(uniforms.y_max - uniforms.y_min, 1e-6);
    let close = closes[i];
    let top = clamp((close - uniforms.y_min) / span, 0.0, 1.0);
    let y_norm = select(top, 0.0, is_baseline);

    // Map [0, 1] -> clip-space [-1, 1]. Y is flipped so that y_norm=1
    // (close at y_max) lands at clip.y = +1, i.e. the top of the viewport.
    let clip_x = x_norm * 2.0 - 1.0;
    let clip_y = y_norm * 2.0 - 1.0;

    var out: VertexOutput;
    out.clip_pos = vec4<f32>(clip_x, clip_y, 0.0, 1.0);
    return out;
}

// --- Fragment Shader ---

@fragment
fn fs_main(_in: VertexOutput) -> @location(0) vec4<f32> {
    return uniforms.color;
}
