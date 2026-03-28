// ============================================================================
// grid.wgsl — Instanced axis-aligned rectangle renderer for grid lines
//
// Each grid line is a filled rectangle specified by [left, top, right, bottom].
// Supports semi-transparent colors for subtle grid appearance.
// ============================================================================

struct CameraUniforms {
    projection: mat4x4<f32>,
}

@group(0) @binding(0)
var<uniform> camera: CameraUniforms;

// --- Vertex Input (per-vertex from unit quad VBO) ---

struct VertexInput {
    @location(0) quad_pos: vec2<f32>, // [0,1]x[0,1] unit quad
}

// --- Instance Input (per-instance from instance buffer) ---
// Matches GridLineInstance layout from midas-chart::instances:
//   rect: [f32; 4] (left, top, right, bottom)
//   color: [f32; 4]

struct InstanceInput {
    @location(1) rect:  vec4<f32>,  // [left, top, right, bottom] in pixel space
    @location(2) color: vec4<f32>,
}

// --- Vertex Output ---

struct VertexOutput {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0)       color:    vec4<f32>,
}

// --- Vertex Shader ---

@vertex
fn vs_main(vert: VertexInput, inst: InstanceInput) -> VertexOutput {
    // Expand unit quad to the rectangle.
    // rect.x = left, rect.y = top, rect.z = right, rect.w = bottom
    let px = inst.rect.x + vert.quad_pos.x * (inst.rect.z - inst.rect.x);
    let py = inst.rect.y + vert.quad_pos.y * (inst.rect.w - inst.rect.y);

    var out: VertexOutput;
    out.clip_pos = camera.projection * vec4<f32>(px, py, 0.0, 1.0);
    out.color    = inst.color;
    return out;
}

// --- Fragment Shader ---

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return in.color;
}
