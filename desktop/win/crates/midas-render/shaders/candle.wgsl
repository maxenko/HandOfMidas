// ============================================================================
// candle.wgsl — Instanced candlestick renderer (wick + body two-pass)
//
// Pass 1 (draw_mode=0): Draws thin wick rectangles
// Pass 2 (draw_mode=1): Draws candle body rectangles
//
// Both passes use the same instance buffer. The vertex shader selects
// which rectangle dimensions to use based on draw_mode read from a
// uniform buffer (swapped via bind group between draw calls).
// ============================================================================

// --- Uniforms ---

struct CameraUniforms {
    /// Orthographic projection: pixel-space -> NDC
    projection: mat4x4<f32>,
}

@group(0) @binding(0)
var<uniform> camera: CameraUniforms;

// --- Draw Parameters (uniform buffer, replaces push constants) ---

struct DrawParams {
    /// 0 = wick pass, 1 = body pass
    draw_mode: u32,
    /// MSDF px_range (unused in candle shader)
    px_range: f32,
    _pad0: u32,
    _pad1: u32,
}

@group(1) @binding(0)
var<uniform> params: DrawParams;

// --- Vertex Input (per-vertex from unit quad VBO) ---

struct VertexInput {
    @location(0) quad_pos: vec2<f32>,  // [0,1] x [0,1]
}

// --- Instance Input (per-instance from instance buffer) ---
// Matches CandleInstance layout from midas-chart::instances:
//   x: f32, body_top: f32, body_bottom: f32, wick_top: f32,
//   wick_bottom: f32, width: f32, wick_width: f32, dim: f32,
//   color: vec4<f32>

struct InstanceInput {
    @location(1) x:           f32,
    @location(2) body_top:    f32,
    @location(3) body_bottom: f32,
    @location(4) wick_top:    f32,
    @location(5) wick_bottom: f32,
    @location(6) width:       f32,
    @location(7) wick_width:  f32,
    @location(9) dim:         f32,   // 0.0 = full brightness, 1.0 = dimmed to 30%
    @location(8) color:       vec4<f32>,
}

// --- Vertex Output ---

struct VertexOutput {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0)       color:    vec4<f32>,
    @location(1)       dim:      f32,
}

// --- Vertex Shader ---

@vertex
fn vs_main(vert: VertexInput, inst: InstanceInput) -> VertexOutput {
    // Select rectangle dimensions based on draw mode (from uniform buffer)
    var rect_width: f32;
    var rect_top: f32;
    var rect_bottom: f32;

    if (params.draw_mode == 0u) {
        // Wick pass: thin vertical line from wick_top to wick_bottom
        rect_width  = inst.wick_width;
        rect_top    = inst.wick_top;
        rect_bottom = inst.wick_bottom;
    } else {
        // Body pass: wide rectangle from body_top to body_bottom
        rect_width  = inst.width;
        rect_top    = inst.body_top;
        rect_bottom = inst.body_bottom;
    }

    // Expand unit quad [0,1]x[0,1] to the rectangle in pixel space.
    //
    // X: center the rectangle on x
    //   quad_pos.x=0 -> left edge  = x - rect_width/2
    //   quad_pos.x=1 -> right edge = x + rect_width/2
    //
    // Y: stretch from rect_top to rect_bottom
    //   quad_pos.y=0 -> rect_top    (top of rect = min Y = higher on screen)
    //   quad_pos.y=1 -> rect_bottom (bottom of rect = max Y = lower on screen)

    let px = inst.x - rect_width * 0.5 + vert.quad_pos.x * rect_width;
    let py = rect_top + vert.quad_pos.y * (rect_bottom - rect_top);

    var out: VertexOutput;
    out.clip_pos = camera.projection * vec4<f32>(px, py, 0.0, 1.0);
    out.color    = inst.color;
    out.dim      = inst.dim;
    return out;
}

// --- Fragment Shader ---

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Apply dim factor: 0.0 = full brightness, 1.0 = 30% brightness.
    let brightness = 1.0 - in.dim * 0.7;
    return vec4(in.color.rgb * brightness, in.color.a);
}
