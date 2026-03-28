// ============================================================================
// volume.wgsl — Instanced semi-transparent volume bar renderer
//
// Each volume bar is a filled rectangle with alpha blending.
// Single pass — no draw_mode switching needed.
// ============================================================================

struct CameraUniforms {
    projection: mat4x4<f32>,
}

@group(0) @binding(0)
var<uniform> camera: CameraUniforms;

// --- Vertex Input (per-vertex from unit quad VBO) ---

struct VertexInput {
    @location(0) quad_pos: vec2<f32>,
}

// --- Instance Input (per-instance from instance buffer) ---
// Matches VolumeInstance layout from midas-chart::instances:
//   x: f32, y_top: f32, y_bottom: f32, width: f32,
//   color: vec4<f32>

struct InstanceInput {
    @location(1) x:        f32,
    @location(2) y_top:    f32,
    @location(3) y_bottom: f32,
    @location(4) width:    f32,
    @location(5) color:    vec4<f32>,
}

// --- Vertex Output ---

struct VertexOutput {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0)       color:    vec4<f32>,
}

// --- Vertex Shader ---

@vertex
fn vs_main(vert: VertexInput, inst: InstanceInput) -> VertexOutput {
    // Expand unit quad to the volume bar rectangle in pixel space.
    // X: center the bar on inst.x
    let px = inst.x - inst.width * 0.5 + vert.quad_pos.x * inst.width;
    // Y: stretch from y_top to y_bottom
    let py = inst.y_top + vert.quad_pos.y * (inst.y_bottom - inst.y_top);

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
