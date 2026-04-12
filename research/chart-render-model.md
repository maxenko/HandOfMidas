# Chart Render Model — Architecture Research

Research into clean, isolated chart rendering in Rust with wgpu. Goal: a self-contained
ChartView with atomic data binding and no way to reach an inconsistent state.

---

## 1. iced `shader::Program` Pattern

iced 0.14 provides `widget::shader::Shader<P: Program>` for custom GPU rendering inside the widget tree. The contract:

- **`Program::draw(state, cursor, bounds) -> Primitive`** — called each frame from `view()`. Produces an immutable primitive describing what to render. No side effects.
- **`Primitive::prepare(device, queue, format, storage, bounds, viewport)`** — upload GPU data. Called before render passes begin. Receives a type-map `Storage` for pipeline caches.
- **`Primitive::render(encoder, storage, target, clip_bounds, viewport)`** — issue draw commands. Data is already on GPU from prepare.

Key insight: the Primitive is an **immutable snapshot** created during the view phase. By the time `prepare()` runs, the app state may have moved on — but the primitive holds exactly the data it needs. This is the pattern Hand of Midas already uses (`ChartRenderSnapshot` -> `ChartPrimitive`).

Implication for ChartView: the snapshot boundary is the right place to enforce atomicity. If data + camera are bundled into one immutable struct at snapshot time, desync is structurally impossible.

---

## 2. wgpu Middleware Pattern (prepare/render split)

The wgpu wiki's "Encapsulating Graphics Work" recommends:

1. **`prepare(&self, device, queue) -> Option<CommandBuffer>`** — stage all buffer writes, bind group updates. Returns optional command buffer for the caller's batch submission.
2. **`render(&self, render_pass)`** — issue draw calls into a caller-provided pass. Borrows `&self` (not `&mut`), so the caller retains flexibility.

Best practices for buffer management:
- Use `queue.write_buffer()` for small/frequent updates (camera uniforms, instance data under ~64KB).
- Grow instance buffers with a capacity strategy (double on overflow, never shrink mid-session).
- One bind group per "version" of uniform data — swap bind groups rather than rewriting buffer contents when supporting multi-view.

Frame consistency rule: all `write_buffer` calls for a frame happen in `prepare()` before any render pass begins. The GPU sees a coherent set of buffers per submission.

---

## 3. Render Snapshot Pattern

The pattern: produce an immutable `RenderFrame` struct containing everything the renderer needs, then hand it off. The renderer never reaches back into app state.

```
App State (mutable) --[snapshot]--> RenderFrame (frozen) --[prepare]--> GPU buffers --[render]--> pixels
```

This is a recognized pattern in game engines (Bevy calls it "Extract", Unreal calls it "Scene Proxy"). Tradeoffs:

| Pro | Con |
|-----|-----|
| Zero desync by construction | Memory cost of the snapshot copy |
| Renderer can run on a separate thread | One frame of latency if pipelined |
| Testable: assert on snapshot contents | Snapshot struct grows with features |

For a chart widget rendering <100K instances, the copy cost is negligible (<1ms for 50K candles at 48 bytes each = 2.4 MB). Arc-wrapping the candle buffer eliminates the largest copy entirely.

The key API decision: the snapshot must be **opaque to the caller**. The caller sets data + viewport atomically; they cannot reach inside and modify the camera without also providing matching data bounds.

---

## 4. Presentation Model / MVVM Applied to Charts

Martin Fowler's Presentation Model: "a fully self-contained class that represents all the data and behavior of the UI, but without any of the controls used to render it."

Mapping to chart rendering:

| Layer | Role | Chart equivalent |
|-------|------|-----------------|
| Model | Domain data | `CandleBuffer`, order state, levels |
| Presentation Model (ViewModel) | UI-ready projection | `ChartScene` — positions, colors, matrices |
| View | Pixel output | wgpu pipelines |

The compute pass (`compute_chart_scene()`) IS the ViewModel transform. It converts domain data (timestamps, prices) into screen-space primitives (pixel positions, colors). The renderer never interprets domain semantics — it just draws what the scene says.

This separation prevents desync because the ViewModel is recomputed atomically from the Model each frame. There is no persistent "render state" that can drift from the model.

---

## 5. Bevy Extract Pattern (ECS Inspiration)

Bevy maintains two separate ECS Worlds: Main World (app logic) and Render World (GPU state). Each frame:

1. **Extract** — copy relevant data from Main to Render world. This is the ONLY sync point. Runs exclusively (no parallelism during extract).
2. **Prepare** — create/update GPU resources from extracted data.
3. **Queue** — build draw commands.
4. **Render** — submit to GPU.

After extract, the main world continues simulating the next frame while the render world draws the current one (pipelined rendering).

Applicable insight for ChartView: the "extract" boundary should be a **single function call** that captures everything needed. Once extracted, the render path is self-sufficient. This maps directly to building the `ChartRenderSnapshot` in `view()`.

---

## 6. Proposed API Design

```rust
/// A self-contained chart rendering unit.
/// All state is internal. External code interacts only through
/// atomic setter methods and render().
pub struct ChartView {
    // Private: data, camera, dirty flags, GPU resources
}

impl ChartView {
    /// Atomic data + viewport binding.
    /// Camera is auto-fitted to the data bounds.
    /// Cannot set data without also establishing a valid viewport.
    pub fn set_data(&mut self, data: Arc<CandleBuffer>) { .. }

    /// Position the visible window. Clamps to data bounds automatically.
    /// No-op if no data is bound.
    pub fn position_view(&mut self, time_start: f64, time_end: f64,
                         price_low: f64, price_high: f64) { .. }

    /// Resize the viewport (logical pixels). Preserves the visible
    /// price/time range by adjusting the camera.
    pub fn resize(&mut self, width: u32, height: u32) { .. }

    /// Produce a frozen render frame. Computes all instances from
    /// current state. The returned Frame is Send + 'static.
    pub fn snapshot(&self) -> ChartFrame { .. }
}

/// Immutable, self-contained render payload.
/// Contains everything needed to draw one frame.
/// Produced by ChartView::snapshot(), consumed by ChartRenderer.
pub struct ChartFrame {
    pub projection: Mat4,
    pub candles: Arc<[CandleInstance]>,
    pub volumes: Arc<[VolumeInstance]>,
    pub grid: Arc<[GridLineInstance]>,
    // ... other layers
}

impl ChartFrame {
    /// Upload to GPU and draw. The frame is consumed.
    pub fn render(&self, renderer: &mut ChartRenderer,
                  device: &Device, queue: &Queue,
                  pass: &mut RenderPass) { .. }
}
```

Design invariants enforced by this API:
- `set_data()` always leaves the camera valid (auto-fit on first bind).
- `position_view()` clamps to data bounds — cannot scroll past data.
- `snapshot()` captures a coherent state — camera and data are from the same moment.
- `ChartFrame` is immutable and self-contained — the renderer cannot cause desync.
- No public access to internal camera or buffer state — callers describe intent, not mechanism.

---

## Summary of Recommendations

1. **Keep the snapshot boundary** (already in place as `ChartRenderSnapshot`). Make it the sole interface between app logic and render.
2. **Make the snapshot opaque** — remove any public fields that allow partial mutation. Replace with intent-based methods (`position_view`, `zoom_to_range`).
3. **Compute scene inside the snapshot** — the `ChartFrame` should contain GPU-ready instances, not raw data. The compute step belongs to the producing side, not the consuming side.
4. **Arc-wrap large buffers** — candle data is shared, not copied. Only instance arrays (computed from data + camera) are per-frame.
5. **Single extraction point** — one function that captures all mutable state into an immutable frame. After that call, the renderer is self-sufficient.
