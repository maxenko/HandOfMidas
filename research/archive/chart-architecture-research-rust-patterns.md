# Reusable GPU-Rendered Chart Components in Rust: Research Report

> Agent 2 — How Rust GUI projects structure reusable, pluggable components
> Research conducted 2026-03-25

---

## 1. iced's Shader Widget Pattern

### Architecture

iced 0.14 provides a `Shader<Message>` widget with three traits:

**Program** — Orchestrator. Owns semantic logic.
- `type State` — Persistent CPU state across frames
- `type Primitive: Primitive` — Per-frame data blob for GPU
- `fn draw(&self, state: &State, cursor: Cursor, bounds: Rectangle) -> Self::Primitive`
- `fn update(&self, state: &mut State, event: Event, ...) -> (Status, Option<Message>)`

**Primitive** — Per-frame GPU data envelope.
- `type Pipeline: Pipeline`
- `fn prepare(&self, pipeline: &mut Self::Pipeline, device, queue, bounds, viewport)`
- `fn draw(&self, pipeline: &Self::Pipeline, render_pass: &mut RenderPass) -> bool`

**Pipeline** — Constructed once. Shared across all widget instances.
- `fn new(device: &Device, queue: &Queue, format: TextureFormat) -> Self`

### Data Flow

Strictly **push-based, unidirectional** (Elm architecture):
1. App state → `view()` → `Shader` widget
2. `Program::draw()` → `Primitive` (owned snapshot)
3. `Primitive::prepare()` → GPU upload
4. `Primitive::draw()` → draw calls
5. Events → `Program::update()` → `Message` back to app

**Key insight**: The `Primitive` is the serialization boundary. GPU code never borrows application data — it receives an owned snapshot.

### Production Validation: Kraken Desktop

Kraken Desktop is built entirely on iced and uses this pattern for trading charts. Validates that the Shader widget is production-viable for high-performance financial charting.

---

## 2. egui's Custom Painting (PaintCallback)

### Architecture

Custom GPU rendering injected via `egui_wgpu::CallbackTrait`:
- `fn prepare(&self, device, queue, screen_descriptor, encoder, callback_resources)`
- `fn paint(&self, info, render_pass, callback_resources)`

Resources stored in a `TypeMap` called `callback_resources`, keyed by type.

### egui_plot: The Cautionary Tale

egui_plot tessellates ALL geometry on CPU every frame (3-4 vertices per point for feathering), then uploads. Works for small datasets, breaks at 1M+ points. The fix requires breaking immediate-mode purity by caching tessellated geometry — exactly the retained-resource hybrid.

### Rerun's Split Renderer Approach

Rerun (whose CTO created egui) separates: **egui** for 2D UI controls, **re_renderer** (custom wgpu) for heavy visualization. Custom visualizers have their own archetypes, visualizer systems, and optional GPU renderers.

**Lesson**: For serious visualization performance, use your own renderer alongside the UI framework, not inside it.

---

## 3. Zed's GPUI

### Three-Phase Rendering

1. **Layout**: Constraints flow down, sizes flow up (Taffy/Flexbox)
2. **Prepaint**: State snapshot
3. **Paint**: Elements add primitives to Scene

The Scene is a flat, sorted, batchable list — not a scene graph. Primitives sorted by type for GPU batching. Rendered in fixed order: shadows → quads → paths → underlines → sprites.

### Shader-Per-Primitive Philosophy

One custom shader per primitive type. Rounded rectangles with borders and shadows are a single shader computing SDF per pixel. Same philosophy as Midas's per-pipeline approach.

### Stateless Components (Longbridge)

The `gpui-component` crate provides 60+ widgets including charts as **stateless `RenderOnce` components** — no hidden mutable state. Caller provides all data through properties.

---

## 4. Bevy's Plugin Pattern

### Dual-World Architecture

**Main World** (app logic) → **Extract** (copy to render world) → **Prepare** → **Queue** → **Render**

The Extract phase is the serialization boundary — conceptually identical to iced's Primitive concept.

**Assessment**: Over-engineered for charting. The ECS overhead, dual-world extraction, and render graph solve problems charts don't have.

**Worth borrowing**: The Extract pattern (snapshot state into GPU-friendly format) and PipelineCache (lazy creation with caching).

---

## 5. Makepad's Widget System

### Draw/Handle Split

Every widget has exactly two functions:
- `fn draw_walk(&mut self, cx: &mut Cx2d, scope, walk) -> DrawStep`
- `fn handle_event(&mut self, cx: &mut Cx, event, scope)`

Events flow through `handle_event()` first, then drawing in `draw_walk()` — two separate passes per frame.

### GPU Details

Retained-mode render with hierarchical draw passes. Instanced-array draw calls. Vertex-shader clipping (no stencil). Almost entirely matrix-free.

---

## 6. Framework-Agnostic Crate Design: The Sans-IO Pattern

### Applied to a Chart Component

```rust
// Core chart logic — no GPU, no framework, no I/O
struct ChartState { candles, visible_range, camera, crosshair, ... }

impl ChartState {
    fn handle_event(&mut self, event: ChartEvent) -> Vec<ChartAction> { ... }
    fn render(&self) -> ChartScene { ... }
}

// Framework-agnostic render commands
struct ChartScene { candle_instances, line_segments, labels, grid_lines, ... }
```

The `ChartScene` is the serialization boundary. The `ChartState` is a pure state machine that can be unit tested without GPU, fuzzed, benchmarked, and reused across frameworks with thin adapters.

### Recommended Crate Boundary

```
midas-chart-core/     # No GPU, no framework. Pure state machine + scene output.
midas-chart-wgpu/     # wgpu implementation consuming ChartScene.
midas-chart-iced/     # Thin adapter: iced Program/Primitive/Pipeline.
```

The `core` crate has zero dependencies beyond `std`.

---

## 7. Retained vs Immediate for Charts

### The Recommended Hybrid

**Retained GPU resources, immediate composition logic.**

GPU buffers (candle instances, line vertices) are persistent and dirty-flagged. Each frame, composition logic runs from scratch producing a list of draw commands referencing retained GPU resources. Only changed buffers are re-uploaded.

This is what Midas already does: `Primitive::prepare()` conditionally uploads, `Primitive::draw()` issues draw calls every frame. The Primitive is "immediate composition"; the Pipeline's HashMap is "retained resources."

---

## 8. Cross-Framework Comparison

### Rendering Decoupled from Framework

| Framework | Decoupling Mechanism | Portability |
|---|---|---|
| iced | Primitive data struct as boundary | wgpu code portable; orchestration iced-specific |
| egui | PaintCallback with TypeMap resources | wgpu code portable; registration egui-specific |
| GPUI | Scene primitives in paint() | Not portable — monolithic |
| Bevy | Extract phase copies to Render World | Render systems Bevy-specific |
| Makepad | DrawList in draw_walk() | Not portable — tied to shader DSL |

**Best practice**: Use a plain data struct (like iced's Primitive or sans-IO ChartScene) as the boundary.

### Data Flow Patterns

| Framework | Push/Pull | Ownership |
|---|---|---|
| iced | Push (Elm: state → view → Primitive) | Primitive owns snapshot |
| egui | Pull (widget reads inline) | Callback captures by value |
| GPUI | Push (Render from entity) | Elements borrow during render |
| Bevy | Push (Extract copies) | Render world owns copies |

**Best practice for charts**: Push with owned snapshots. Avoids lifetime complexity.

---

## 9. Key Recommendations for Midas

1. **Adopt the sans-IO core pattern** — single most impactful change. Chart logic as pure function with zero framework deps.

2. **Keep the current iced integration** — Program/Primitive/Pipeline is well-validated (Kraken Desktop).

3. **Use the hybrid retained/immediate approach** — already planned. Validated by GPUI and Bevy patterns.

4. **Structure the Primitive as a scene descriptor** — make it essentially the `ChartScene` from the sans-IO core.

5. **Learn from Rerun's split renderer** — if iced's Shader widget becomes limiting, own the wgpu device directly and compose at the texture level.
