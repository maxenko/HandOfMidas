# Chart Component Architecture Research: Open-Source Projects

> Agent 1 — How professional charting projects structure their chart components
> Research conducted 2026-03-25

---

## 1. TradingView Lightweight Charts

**Repo:** github.com/tradingview/lightweight-charts | **License:** Apache 2.0 | **Size:** ~35 kB (v5) | **Renderer:** Canvas 2D

**Standalone?** Yes. `createChart(container)` returns an `IChartApi` handle. The chart owns its Canvas element and render loop. The host provides only a `<div>`.

**API pattern:** Handle-based imperative. `IChartApi` exposes `addSeries()`, `timeScale()`, `priceScale()`, `subscribeCrosshairMove()`, `subscribeClick()`. `ISeriesApi` exposes `setData()`, `update()`, `setMarkers()`, `attachPrimitive()`, `applyOptions()`. The boundary is data in, rendered pixels out, events back via callbacks.

**Axes, overlays, interaction:** Axes are internal, configurable but not replaceable. Overlays are additional series on shared or overlay price scales. Interaction (crosshair, pan, zoom) is built-in and monolithic — you subscribe to events, you do not inject custom interaction handlers. v5 added multi-pane support (`IPaneApi`) for vertically stacked sub-charts with a shared time axis.

**Renderer swappable?** No. Canvas 2D only. Deliberate choice for simplicity and bundle size.

**Custom series:** v5 introduced `ICustomSeriesPaneView` (full custom series types) and `ISeriesPrimitive` (drawing primitives attached to series). Plugins implement `updateAllViews()` and provide renderers that receive a `CanvasRenderingTarget2D`. The chart controls the render cycle; plugins never initiate redraws.

**Data ownership:** Chart owns copies. `setData()` fully replaces internal data. Documentation advises keeping your own copy if you need it later.

---

## 2. Apache ECharts

**Repo:** github.com/apache/echarts | **License:** Apache 2.0 | **Renderer:** Canvas or SVG via ZRender

**Standalone?** Yes. `echarts.init(dom, theme?, { renderer })` returns an instance. The instance is self-contained.

**API pattern:** Declarative option-driven. The single primary method is `setOption(option)` where `option` is a deeply nested specification object covering series, axes, tooltip, legend, dataZoom, toolbox, dataset, etc. ECharts diffs new options against previous state and computes add/update/remove transitions for data-driven animation.

**Axes, overlays, interaction:** All declarative components within the option object. Axes (`xAxis`, `yAxis`) are first-class. Overlays are additional series on shared axes. Interaction is specified via `dataZoom: [{ type: 'inside' }]`, `tooltip`, `brush`, `toolbox` — you declare desired behaviors, you do not write event handlers for zoom/pan.

**Renderer swappable?** Yes — the cleanest abstraction in this survey. ECharts delegates all rendering to **ZRender**, a separate 2D engine that manages a scene graph of displayable elements and renders to Canvas, SVG, or VML. Renderer is selected at init time. With tree-shaking, you import only the renderer you need.

**Custom series:** The `custom` series type accepts a `renderItem(params, api)` callback that returns ZRender graphic element descriptors. v6.0 added `echarts.registerCustomSeries(name, renderItem)` — publish reusable custom series as npm packages.

**Data ownership:** Chart owns copies. `setOption()` ingests data. `getOption()` returns clones.

---

## 3. Plotly.js

**Repo:** github.com/plotly/plotly.js | **License:** MIT | **Renderer:** SVG (default) + WebGL (per trace type)

**Standalone?** Yes. `Plotly.newPlot(div, data, layout, config)` takes over a DOM element.

**API pattern:** Declarative figure-centric with functional updates. The figure is `{ data: [traces], layout: {...} }`. Updates via `Plotly.react()` (efficient diff), `Plotly.restyle()` (trace attributes), `Plotly.relayout()` (layout attributes), `Plotly.extendTraces()` (append data).

**Renderer swappable?** No — it is per-trace-type. Standard traces use SVG via D3.js. Large-data traces (`scattergl`, `heatmapgl`) use WebGL via `regl`. You cannot render a `scatter` with WebGL; you must use `scattergl`.

**Custom series:** The internal trace module system is well-structured but **there is no public API for registering custom trace types.** Extension requires forking. Least extensible library in the survey.

**Data ownership:** Ambiguous. Plotly may mutate the objects you pass in. This mutation ambiguity causes practical problems in React integration.

---

## 4. D3.js

**Repo:** github.com/d3 (suite of ~30 modules) | **License:** ISC | **Renderer:** Any

**Standalone?** N/A — D3 is not a chart component. It is a toolkit of composable primitives for data visualization. There is no `createChart()` function.

**Why this choice:** Mike Bostock deliberately chose **representation transparency** — D3 manipulates the standard DOM directly rather than hiding output behind a toolkit abstraction. This means output is inspectable with browser dev tools, computation modules are pure functions usable with any renderer, and the library ages well because it builds on web standards.

**API pattern:** Composable primitives. `d3.scaleLinear()`, `d3.axisBottom()`, `d3.line()`, `d3.zoom()`, `d3.brush()` are all independent tools. You compose them yourself.

**Data ownership:** D3 does not own your data. `selection.data(array)` stores references to individual data objects on DOM elements. No copying.

---

## 5. SciChart (JS and WPF)

**Website:** scichart.com | **License:** Commercial | **JS Renderer:** WASM + WebGL | **WPF Renderer:** DirectX / software fallback

**Standalone?** Yes. `SciChartSurface` is the central container, hosting three kinds of composable child objects: **RenderableSeries**, **Axes**, and **ChartModifiers**. This is the most explicitly composition-oriented architecture in the survey.

**API pattern:** Imperative composition. You create a surface, then add axes, series, and modifiers individually:

```js
surface.xAxes.add(new NumericAxis(wasmContext));
surface.renderableSeries.add(new FastLineRenderableSeries(wasmContext, { dataSeries }));
surface.chartModifiers.add(new ZoomPanModifier());
surface.chartModifiers.add(new MouseWheelZoomModifier());
surface.chartModifiers.add(new RolloverModifier());
```

**Interaction decomposition — the gold standard:** Fully decomposed into **ChartModifiers**: `ZoomPanModifier`, `RubberBandXyZoomModifier`, `MouseWheelZoomModifier`, `RolloverModifier` (crosshair/tooltips), `CursorModifier`, `XAxisDragModifier`, `YAxisDragModifier`, `LegendModifier`. Composed in a `ModifierGroup`, they process events in order. Custom modifiers by extending `ChartModifierBase`. **This is the strategy pattern applied to chart interaction.**

**Renderer swappable?** WPF: Yes. Ships with `HighQualityRenderSurface` (software), `HighSpeedRenderSurface`, and `Visual Xccelerator Engine` (DirectX 11). JS: No — WASM+WebGL only.

**Custom series:** Extend `BaseRenderableSeries`. Override rendering methods.

**Data ownership:** Explicitly separated. `DataSeries` objects are independent and own their data. You create, populate, and assign them to RenderableSeries. Multiple series can share one DataSeries.

---

## 6. Grafana Panel Plugin Architecture

**Repo:** github.com/grafana/grafana | **License:** AGPL-3.0

**API contract — the cleanest typed interface in this survey.** Every panel plugin is a React component receiving `PanelProps<TOptions>`:

```typescript
{
  data: PanelData;          // query results as DataFrame[]
  options: TOptions;        // user-configured panel options
  fieldConfig: FieldConfigSource;
  width: number;            // panel pixel dimensions
  height: number;
  timeRange: TimeRange;
  onChangeTimeRange: (tr: TimeRange) => void;
}
```

**Renderer:** The plugin chooses. SVG, Canvas, WebGL, pure HTML — Grafana imposes no rendering abstraction. The panel is a black box.

**Data ownership:** Host (Grafana) owns the data pipeline. Panels receive immutable snapshots as props. One-directional data flow.

---

## 7. Qt Charts / QCustomPlot

### QCustomPlot

**Layer-based compositing architecture**: Everything visible inherits `QCPLayerable`, organized into `QCPLayer` objects controlling draw order. Layers can be **buffered** (`lmBuffered`) with dedicated paint buffers, enabling selective replot. Default layers: background → grid → main → axes → legend → overlay.

Custom plottables by subclassing `QCPAbstractPlottable` and overriding `draw(QCPPainter*)`.

**Data ownership:** Sophisticated shared-pointer model. `QSharedPointer<QCPDataContainer<T>>`. Multiple graphs can share data containers. Copy-on-write safe.

---

## 8. Cross-Project Comparison Matrix

| Dimension | TV LC | ECharts | Plotly | D3 | SciChart | Grafana | QCustomPlot |
|---|---|---|---|---|---|---|---|
| Standalone | Yes | Yes | Yes | N/A | Yes | Plugin | Widget |
| API pattern | Imperative handles | Declarative option | Declarative+functional | Primitives | Imperative composition | React props | OOP widget |
| Data ownership | Copy | Copy | Mutates input | References | Separate DataSeries | Immutable props | Shared pointer |
| Renderer swap | No | Yes (ZRender) | No | N/A | Yes (WPF) | N/A | No |
| Custom series | v5 plugins | registerCustomSeries | No public API | Everything custom | Extend base class | Any React | Subclass |
| Interaction | Monolithic | Declarative | Built-in | Behaviors | **Composable modifiers** | Panel's job | Flags+signals |

---

## 9. Key Takeaways for Hand of Midas

1. **Separate DataSeries from renderable pipeline (SciChart pattern).** Already done: `CandleBuffer` is separate from render pipelines.

2. **The ChartModifier pattern is the gold standard for interaction decomposition.** SciChart's composable modifiers are cleaner than monolithic interaction or declarative components.

3. **Layer-based compositing (QCustomPlot pattern) maps to wgpu.** Default layer stack (grid → main → axes → overlay → crosshair) maps directly to pipeline execution order.

4. **Chart should own data copies.** TV LC, ECharts, and Qt Charts all copy on ingestion. The `CandleBuffer` pattern is correct.

5. **Rendering abstraction unnecessary for v1.** TV LC, QCustomPlot, and SciChart JS all ship with a single renderer.

6. **Plugin extensibility is v2+.** TV LC added plugins in v4/v5. ECharts added registerable series in v6. Hard-coding series types for v1 is appropriate.

7. **Grafana model is relevant for multi-chart coordination.** The PanelProps contract models how the iced shell should communicate with chart panels.
