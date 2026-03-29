# Pluggable Rendering Backend Architectures for Charting

> Agent 3 — Should a charting component use a render abstraction layer?
> Research conducted 2026-03-25

---

## Verdict: No Render Abstraction Layer for v1 — Direct wgpu Is Correct

---

## 1. Vello/Piet Scene Abstraction

### The Scene Builder Pattern

Vello's `Scene` struct accumulates drawing commands into a compact binary `Encoding` optimized for GPU consumption. Key methods: `fill()`, `stroke()`, `push_layer()`/`pop_layer()`, `draw_glyphs()`, `append()` (for multithreaded encoding).

### How Rendering Happens

The Scene's binary encoding is uploaded to GPU as a buffer. Four sequential compute shader stages run: flatten (curves → segments) → binning (spatial sort via prefix-sum) → coarse (per-tile command lists) → fine (pixel rasterization). Fundamentally different from traditional vertex/fragment pipelines.

### Multiple Backends

- **vello** (GPU compute via wgpu)
- **vello_cpu** (pure software, SIMD + multithreaded)
- **vello_hybrid** (CPU preprocessing + GPU rasterization)

All consume the same Scene encoding.

| Criterion | Assessment |
|---|---|
| Overhead | Scene encoding lightweight. Compute dispatch has fixed cost — may exceed direct instanced draw for simple scenes. |
| Text | GPU glyph rasterization. Resolution-independent but less mature than atlas-based for small fixed sizes. |
| Pixel-perfect 1px lines | Analytical AA. Needs careful coordinate alignment — not inherently snapped. |
| Headless | Excellent via vello_cpu. No GPU needed. |

---

## 2. Skia's SkCanvas Abstraction

SkCanvas renders to completely different backends: Raster (CPU), Ganesh (legacy GPU), Graphite (modern GPU), SkPicture (display list), PDF, SVG. Backend determined at SkSurface creation time. Drawing code is identical.

**Graphite's deferred model**: Commands recorded, sorted by z-value, submitted to GPU. Designed for complex scenes with significant overdraw — unnecessary optimization for charting.

| Criterion | Assessment |
|---|---|
| Overhead | Ganesh near-zero for simple draws. Graphite adds sorting passes. |
| Text | World-class (HarfBuzz, FreeType/CoreText, subpixel). Best in any 2D engine. |
| Pixel-perfect 1px lines | Excellent. Decades of pixel-snapping logic. |
| Worth it? | Excellent but heavy C++ build chain. For wgpu-only, abstraction without reward. |

---

## 3. wgpu's Own Abstraction

wgpu IS a rendering backend abstraction: Vulkan, Metal, DX12, WebGPU, OpenGL/GLES. Shader translation via Naga: WGSL → SPIR-V/HLSL/MSL/GLSL.

**What wgpu does NOT provide**: CPU/software fallback, vector output (SVG/PDF), a 2D drawing API.

| Criterion | Assessment |
|---|---|
| Overhead | Negligible. Validation per-call, charting has few calls. |
| Text | BYO (glyphon/MSDF). |
| Pixel-perfect | Full control. 1px filled rectangles, snap in vertex shader. |
| Headless | Native support. No window needed. |

---

## 4. The Render Command / Display List Pattern

Application produces draw commands, renderer consumes:

- **WebRender (Firefox)** — Display list → GPU batches. Items grouped into slices and tiles.
- **Vello** — Scene encoding IS a display list.
- **Skia SkPicture** — Records SkCanvas calls for replay.
- **Plotters (Rust)** — `DrawingBackend` trait. Backends: SVG, bitmap, Canvas.

**Performance for charting**: Command list is tiny (~50-200 commands per chart frame). Overhead measured in microseconds — completely irrelevant at 60fps. The question is whether the abstraction enables anything useful, not whether it costs anything.

---

## 5. Dear ImGui's ImDrawList

ImDrawList accumulates vertices, indices, and draw commands. Every backend implements the same loop: upload buffers, iterate commands, set scissor, bind texture, draw indexed. Implemented for OpenGL 2/3, Vulkan, DX9/10/11/12, Metal, WebGPU.

**For charting**: ImGui's pre-triangulated vertex buffers are less efficient than instanced rendering. A chart draws thousands of identical rectangles — instanced draw beats per-vertex submission.

---

## 6. Raph Levien's Architectural Insights

### "Requiem for piet-gpu-hal" (2023)

**Don't build your own HAL.** wgpu is good-enough with massive ecosystem. Fighting custom HAL lifetime issues not worth marginal performance. If higher abstraction needed, build a declarative scene encoding, not a GPU API wrapper.

### "Fast 2D Rendering on GPU" (2020)

CPU should upload a scene description as a "straightforward binary encoding." GPU does all hard work. Encoding on CPU should be "as light as possible."

---

## 7. Game Engine Render Graphs

**Bevy**: Extract → Prepare → Queue → PhaseSort → Render. Designed for game scenes with thousands of heterogeneous entities. Chart is a handful of draw calls — ECS abstraction adds indirection without benefit.

**WebRender**: Picture tree, slices, tile-level invalidation. Designed for complex web layouts. Charts change every frame when scrolling — tiling/invalidation overhead without benefit.

---

## 8. Recommendation for Hand of Midas

### Direct wgpu Is Correct. Here's Why:

1. **wgpu IS already the abstraction layer.** Adding another is abstraction of an abstraction.

2. **Charts don't benefit from scene abstraction.** Dynamic composition (fixed draw order), transparent sorting (minimal transparency), incremental invalidation (charts change every frame) — charts don't have these problems.

3. **Direct instanced drawing is optimal.** ChartGPU demonstrates: 5M candlesticks at 100+ FPS with instanced drawing. A command list would decompose this into N rectangle commands the renderer re-batches — strictly worse.

4. **Abstraction cost is not zero.** Memory (command buffers), CPU (iteration/dispatch/batching), indirection (loss of pipeline state control). For charting, this buys nothing.

5. **Text is orthogonal.** MSDF atlas works with direct wgpu. If needs grow, glyphon integrates directly.

6. **Headless testing works.** wgpu offscreen rendering + software Vulkan/WARP.

### What Would Change This Recommendation

- **Software fallback needed** → follow iced pattern: wgpu primary, tiny-skia fallback
- **Non-GPU output** (SVG/PDF) → add command list consumed by both GPU and vector backends
- **Scope expands dramatically** → Vello scene abstraction for maintainability (v3+ concern)
- **Vello reaches 1.0** → could replace custom text/grid pipelines

### The Right Internal Boundary

Even without formal abstraction, maintain clean separation:

```
ChartState (logic)        →  ChartRenderer (GPU)
  - visible_candles[]          - encode_candles() → instance buffer
  - camera                     - encode_grid() → instance buffer
  - grid_ticks[]               - encode_labels() → instance buffer
  - axis_labels[]              - render(pass) → draw calls
```

If abstraction ever needed: replace ChartRenderer internals with command list generation. Chart logic (ChartState) would not change.
