// ============================================================================
// badge.wgsl — SDF-based instanced renderer for decorator badges.
//
// Each badge is a rectangle [left, top, right, bottom] in logical pixels,
// dispatched through one of eight signed-distance primitives via `shape_id`.
// Shape IDs must match `BadgeShape::shape_id()` in `midas-chart`:
//   0=Rect, 1=Rounded, 2=Pill, 3=PointLeft,
//   4=PointRight, 5=DoublePoint, 6=Chevron, 7=Circle.
//
// Anti-aliasing via `fwidth()` screen-space derivative. The `BadgePipeline`
// uses the same straight-alpha blend state as `GridPipeline`
// (SrcAlpha / OneMinusSrcAlpha), so the fragment shader outputs a
// straight-alpha colour (rgb unpremultiplied, `.a` carries the coverage).
// ============================================================================

struct CameraUniforms {
    projection: mat4x4<f32>,
}

@group(0) @binding(0)
var<uniform> camera: CameraUniforms;

// --- Vertex Input (per-vertex from unit quad VBO in slot 0) ---

struct VertexInput {
    @location(0) quad_pos: vec2<f32>, // [0,1] x [0,1] unit quad corner
}

// --- Instance Input (per-instance from BadgeInstance buffer in slot 1) ---
// Matches `BadgeInstance` layout declared in `midas-chart::instances`:
//   rect: [f32;4]            @ offset 0  -> location(1) vec4<f32>
//   fill: [f32;4]            @ offset 16 -> location(2) vec4<f32>
//   border: [f32;4]          @ offset 32 -> location(3) vec4<f32>
//   shape_id: u32            @ offset 48 -> location(4) u32
//   shape_param: f32         @ offset 52 -> location(5) f32
//   border_thickness: f32    @ offset 56 -> location(6) f32
//   _pad: f32                @ offset 60 -> location(7) f32

struct InstanceInput {
    @location(1) rect: vec4<f32>,
    @location(2) fill: vec4<f32>,
    @location(3) border: vec4<f32>,
    @location(4) shape_id: u32,
    @location(5) shape_param: f32,
    @location(6) border_thickness: f32,
    @location(7) pad: f32,
}

// --- Vertex Output ---

struct VertexOutput {
    @builtin(position) clip_pos: vec4<f32>,
    // local_uv in [-1, 1] across the instance's rect
    @location(0) local_uv: vec2<f32>,
    // rect width/height in logical pixels
    @location(1) size: vec2<f32>,
    @location(2) fill: vec4<f32>,
    @location(3) border: vec4<f32>,
    // x = shape_id (as f32), y = shape_param, z = border_thickness, w = unused
    @location(4) shape_data: vec4<f32>,
}

// --- Vertex Shader ---

@vertex
fn vs_main(vert: VertexInput, inst: InstanceInput) -> VertexOutput {
    let x0 = inst.rect.x;
    let y0 = inst.rect.y;
    let x1 = inst.rect.z;
    let y1 = inst.rect.w;

    let px = mix(x0, x1, vert.quad_pos.x);
    let py = mix(y0, y1, vert.quad_pos.y);

    var out: VertexOutput;
    out.clip_pos = camera.projection * vec4<f32>(px, py, 0.0, 1.0);
    out.local_uv = vert.quad_pos * 2.0 - vec2<f32>(1.0, 1.0);
    out.size = vec2<f32>(x1 - x0, y1 - y0);
    out.fill = inst.fill;
    out.border = inst.border;
    out.shape_data = vec4<f32>(
        f32(inst.shape_id),
        inst.shape_param,
        inst.border_thickness,
        0.0,
    );
    return out;
}

// ─── SDF primitives (inigo quilez's 2D SDF library) ─────────────────────

fn sd_rect(p: vec2<f32>, size: vec2<f32>) -> f32 {
    let d = abs(p) - size * 0.5;
    return length(max(d, vec2<f32>(0.0, 0.0))) + min(max(d.x, d.y), 0.0);
}

fn sd_rounded_box(p: vec2<f32>, size: vec2<f32>, r: f32) -> f32 {
    let q = abs(p) - size * 0.5 + vec2<f32>(r, r);
    return length(max(q, vec2<f32>(0.0, 0.0))) + min(max(q.x, q.y), 0.0) - r;
}

fn sd_circle(p: vec2<f32>, size: vec2<f32>) -> f32 {
    return length(p) - min(size.x, size.y) * 0.5;
}

fn sd_triangle(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>, c: vec2<f32>) -> f32 {
    let e0 = b - a;
    let e1 = c - b;
    let e2 = a - c;
    let v0 = p - a;
    let v1 = p - b;
    let v2 = p - c;
    let pq0 = v0 - e0 * clamp(dot(v0, e0) / dot(e0, e0), 0.0, 1.0);
    let pq1 = v1 - e1 * clamp(dot(v1, e1) / dot(e1, e1), 0.0, 1.0);
    let pq2 = v2 - e2 * clamp(dot(v2, e2) / dot(e2, e2), 0.0, 1.0);
    let s = sign(e0.x * e2.y - e0.y * e2.x);
    let d = min(
        min(
            vec2<f32>(dot(pq0, pq0), s * (v0.x * e0.y - v0.y * e0.x)),
            vec2<f32>(dot(pq1, pq1), s * (v1.x * e1.y - v1.y * e1.x)),
        ),
        vec2<f32>(dot(pq2, pq2), s * (v2.x * e2.y - v2.y * e2.x)),
    );
    return -sqrt(d.x) * sign(d.y);
}

fn sd_point_left(p: vec2<f32>, size: vec2<f32>, point_width: f32) -> f32 {
    let body_size = vec2<f32>(size.x - point_width, size.y);
    let body_center = vec2<f32>(point_width * 0.5, 0.0);
    let body = sd_rect(p - body_center, body_size);

    let tip = vec2<f32>(-size.x * 0.5, 0.0);
    let b0 = vec2<f32>(-size.x * 0.5 + point_width, -size.y * 0.5);
    let b1 = vec2<f32>(-size.x * 0.5 + point_width, size.y * 0.5);
    let tri = sd_triangle(p, tip, b1, b0);

    return min(body, tri);
}

fn sd_point_right(p: vec2<f32>, size: vec2<f32>, point_width: f32) -> f32 {
    let body_size = vec2<f32>(size.x - point_width, size.y);
    let body_center = vec2<f32>(-point_width * 0.5, 0.0);
    let body = sd_rect(p - body_center, body_size);

    let tip = vec2<f32>(size.x * 0.5, 0.0);
    let b0 = vec2<f32>(size.x * 0.5 - point_width, -size.y * 0.5);
    let b1 = vec2<f32>(size.x * 0.5 - point_width, size.y * 0.5);
    let tri = sd_triangle(p, tip, b0, b1);

    return min(body, tri);
}

fn sd_double_point(p: vec2<f32>, size: vec2<f32>, point_width: f32) -> f32 {
    let body_size = vec2<f32>(size.x - 2.0 * point_width, size.y);
    let body = sd_rect(p, body_size);

    let tip_l = vec2<f32>(-size.x * 0.5, 0.0);
    let l0 = vec2<f32>(-size.x * 0.5 + point_width, -size.y * 0.5);
    let l1 = vec2<f32>(-size.x * 0.5 + point_width, size.y * 0.5);
    let tri_l = sd_triangle(p, tip_l, l1, l0);

    let tip_r = vec2<f32>(size.x * 0.5, 0.0);
    let r0 = vec2<f32>(size.x * 0.5 - point_width, -size.y * 0.5);
    let r1 = vec2<f32>(size.x * 0.5 - point_width, size.y * 0.5);
    let tri_r = sd_triangle(p, tip_r, r0, r1);

    return min(body, min(tri_l, tri_r));
}

fn sd_chevron(p: vec2<f32>, size: vec2<f32>, point_width: f32) -> f32 {
    let hx = size.x * 0.5;
    let hy = size.y * 0.5;

    let body_size = vec2<f32>(size.x - point_width, size.y);
    let body_center = vec2<f32>(-point_width * 0.5, 0.0);
    let body = sd_rect(p - body_center, body_size);

    let tip = vec2<f32>(hx, 0.0);
    let a0 = vec2<f32>(hx - point_width, -hy);
    let a1 = vec2<f32>(hx - point_width, hy);
    let tri_r = sd_triangle(p, tip, a0, a1);

    let outer = min(body, tri_r);

    let notch_tip = vec2<f32>(-hx + point_width, 0.0);
    let n0 = vec2<f32>(-hx, -hy);
    let n1 = vec2<f32>(-hx, hy);
    let notch = sd_triangle(p, notch_tip, n1, n0);

    return max(outer, -notch);
}

// --- Fragment Shader ---

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Translate local_uv [-1, 1] into half-size-space coordinates.
    let p = in.local_uv * in.size * 0.5;

    let shape_id = u32(in.shape_data.x);
    let shape_param = in.shape_data.y;
    let t = in.shape_data.z;

    var d: f32 = 0.0;
    switch shape_id {
        case 0u: { d = sd_rect(p, in.size); }
        case 1u: { d = sd_rounded_box(p, in.size, shape_param); }
        case 2u: {
            // Pill: rounded with radius = half of the shorter side.
            let r = min(in.size.x, in.size.y) * 0.5;
            d = sd_rounded_box(p, in.size, r);
        }
        case 3u: { d = sd_point_left(p, in.size, shape_param); }
        case 4u: { d = sd_point_right(p, in.size, shape_param); }
        case 5u: { d = sd_double_point(p, in.size, shape_param); }
        case 6u: { d = sd_chevron(p, in.size, shape_param); }
        case 7u: { d = sd_circle(p, in.size); }
        default: { d = sd_rect(p, in.size); }
    }

    let aa = fwidth(d);
    let inside = 1.0 - smoothstep(-aa, aa, d);

    if (inside <= 0.0) {
        discard;
    }

    let has_border = t > 0.0 && in.border.a > 0.0;
    if (has_border) {
        // `inner` is the coverage of the interior (fill-only) region;
        // `inside - inner` is the coverage of the border band.
        let inner = 1.0 - smoothstep(-t - aa, -t + aa, d);
        let border_cov = inside - inner;
        let fill_cov = inside - border_cov;
        // Straight alpha output: blend fill and border into a single
        // rgba by weighting each by its coverage × its own alpha, then
        // dividing the rgb by the combined alpha to keep colours straight.
        let fa = in.fill.a * fill_cov;
        let ba = in.border.a * border_cov;
        let total_a = fa + ba;
        if (total_a <= 0.0) {
            discard;
        }
        let rgb = (in.fill.rgb * fa + in.border.rgb * ba) / total_a;
        return vec4<f32>(rgb, total_a);
    }

    // Straight alpha: coverage folds into `.a` only, rgb stays untouched.
    return vec4<f32>(in.fill.rgb, in.fill.a * inside);
}
