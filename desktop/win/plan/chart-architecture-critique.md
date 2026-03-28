# Critical Architectural Review: Chart Component Boundaries

> Agent 4 — Critique of the current plan's chart architecture
> Research conducted 2026-03-25

---

## 1. Component Boundary — There Is No Chart Component

**Finding: The "chart" does not exist as a self-contained component. It is a collaboration between four crates with no single facade or API surface.**

The plan distributes chart responsibilities across the entire crate graph:

- **`midas-core`** owns `ChartState`, `DirtyFlags`, `DirtyTracker`, `Camera2D/ChartCamera`, `Viewport`, `InteractionState`, and coordinate transforms.
- **`midas-data`** owns `CandleBuffer` (SoA), `CandleSlice`, and `find_index_by_time` / `price_range`.
- **`midas-render`** owns `SharedPipelines`, `ChartGpuResources`, `CandleInstance`, all WGSL shaders, and draw call logic.
- **`midas-app`** owns `ChartPanel`, `ChartProgram`, `ChartPrimitive`, the `Message` enum, interaction-to-message translation, animation tick, and `view_chart_panel`.

There is no `Chart::new(config) -> Chart` anywhere. No single struct you can hand data to and get a rendered frame back. Instead, a "chart" is emergent behavior across four crates, four call sites, no boundary.

**Could someone use the chart without iced?** No. `ChartProgram` takes `&MidasApp` directly. The `Primitive` trait implementation is iced-specific. Removing iced means rewriting the entire bridge layer.

**Could someone use the chart without the specific data format?** No. `ChartProgram::draw()` calls methods directly on `CandleBuffer`. The renderer cannot accept any other data shape.

---

## 2. Coupling Analysis

### 2a. Coupling to iced — Structural, Not Incidental

1. **GPU resource lifetime owned by iced.** `ChartPipeline` constructed by iced via `Pipeline::new()`. Application never directly creates GPU resources.
2. **Primitive is the data contract.** `ChartPrimitive` is both data transfer object AND an iced trait implementation. No intermediate representation for non-iced renderers.
3. **Event routing through iced.** `ChartProgram::update()` receives `shader::Event` and emits `Message` variants.

**Backend swap cost:** Rewriting `ChartProgram`, `ChartPrimitive`, `ChartPipeline` (~400 lines), plus event translation, plus GPU lifecycle. WGSL shaders and instance layouts survive.

### 2b. Coupling to CandleBuffer — Direct Struct Access

`ChartProgram::draw()` reaches through `chart.data` to `Arc<CandleBuffer>`, calls `data.find_index_by_time()` and `data.to_candle_instances()`. The crosshair accesses `data.timestamps[idx]` by index. No data abstraction trait.

### 2c. Could You Swap Rendering Backend?

Partially. `CandleInstance`, `VolumeInstance`, `LineInstance` are pure data. A Skia backend could read these. But `build_candle_instance` bakes in wgpu coordinate system assumptions and colors are in linear RGB.

---

## 3. Data Flow — Ownership Split and Unclear

**Who owns the candle data?** The application, not the chart. `MidasApp` owns `DataManager`. `ChartPanel` holds `Option<Arc<CandleBuffer>>`.

**Is there a clean input contract?** No. `ChartProgram::draw()` takes `&MidasApp` (the entire application state) and extracts what it needs from at least four different subsystems.

**What the contract should be:**

```rust
fn prepare_chart_frame(
    data: &CandleSlice,
    camera: &ChartCamera,
    viewport: &Viewport,
    theme: &ChartTheme,
    crosshair: Option<&CrosshairState>,
    levels: &[HorizontalLevel],
    indicators: &[IndicatorOutput],
) -> ChartFrame
```

This function does not exist in the plan.

---

## 4. Interaction Model — Embedded in Application, Not Chart

The 5-state state machine is well-designed but distributed across three layers:
1. iced event → Message translation in `ChartProgram::update()`
2. State machine transitions in `ChartState::handle_input()`
3. Action application in `MidasApp::update()` match arms

**Could you use a different interaction model?** In theory yes (actions are separate from state machine). In practice, `MidasApp::update()` has hardcoded handlers and `Message` conflates chart interactions with app-level concerns.

**Improvement:** The `ChartInputEvent → ChartAction` state machine should be standalone, tested, with zero iced dependency.

---

## 5. Extensibility — Adding Series Requires Core Changes

**Adding a new series type (e.g., line chart, Renko):**

No series abstraction. The plan hardcodes candle rendering. `ChartPrimitive` has typed fields `candle_instances` and `volume_instances`. Adding a line chart requires changes in **seven places across three crates**:
1. New instance struct in `midas-render`
2. New WGSL shader
3. New pipeline in `SharedPipelines`
4. New field in `ChartPrimitive`
5. New draw call in `ChartPrimitive::draw()`
6. New dirty flag
7. New instance buffer in `ChartGpuResources`

**Adding a new overlay (e.g., Fibonacci retracement):**

Current `HorizontalLevel` model only supports single-price horizontal lines. Multi-point drawing tools have no data model or interaction pattern.

---

## 6. Reusability — Not Currently Feasible as Standalone Crate

1. iced dependency is structural (not feature-gatable)
2. `ChartProgram` takes `&MidasApp`
3. No framework-agnostic rendering entry point
4. Interaction state in midas-core consumed in midas-app

---

## 7. Concrete Improvements (Priority Order)

### Priority 1 — `ChartScene` Intermediate Representation

Create a struct holding all computed instance data independent of GPU/framework types. A pure function `compute_chart_scene(data, camera, viewport, theme, ...) → ChartScene` makes chart logic testable and portable.

### Priority 2 — Replace `&MidasApp` with Data-Only Input

`ChartProgram::new()` should take a `ChartInput` struct, not `&MidasApp`. Eliminates the most severe coupling point.

### Priority 3 — `CandleData` Trait

```rust
pub trait CandleData {
    fn len(&self) -> usize;
    fn timestamp(&self, idx: usize) -> i64;
    fn ohlcv(&self, idx: usize) -> (f32, f32, f32, f32, u32);
    fn price_range(&self, range: Range<usize>) -> (f32, f32);
    fn find_index_by_time(&self, ts: i64) -> usize;
}
```

Implement for `CandleBuffer`. Chart accepts any data source.

### Priority 4 — Isolate Interaction State Machine

`ChartState::handle_input() → Vec<ChartAction>` as a tested, standalone module with zero iced imports.

### Priority 5 — Extensible Series Types

Instead of hardcoded fields in `ChartPrimitive`, use `Vec<SeriesRenderData>` where `SeriesRenderData` is an enum. Makes adding series additive.

---

## 8. Overall Assessment

The plan is exceptionally thorough for a v1 application-specific implementation. The problem is not quality — it is that the plan conflates "building a charting application" with "building a chart component."

This is defensible for v1. But introducing the `ChartScene` intermediate representation and `CandleData` trait now — before any code is written — would cost ~2-3 days and save weeks of refactoring later.
