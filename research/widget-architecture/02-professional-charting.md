# Professional Charting Library Patterns: SciChart, Highcharts Stock, D3FC

> Compiled from open-source and renderer architecture research, 2026-03-30

---

## 1. SciChart: Three-Pillar Composition Model

### Architecture

SciChart (commercial, WASM+WebGL for JS, DirectX for WPF) is the most explicitly composition-oriented charting architecture surveyed. `SciChartSurface` is the central container hosting three kinds of composable child objects:

1. **RenderableSeries** -- visual data representations (line, candlestick, scatter, etc.)
2. **Axes** -- coordinate systems (NumericAxis, DateTimeAxis, etc.)
3. **ChartModifiers** -- interaction behaviors (zoom, pan, crosshair, etc.)

```js
surface.xAxes.add(new NumericAxis(wasmContext));
surface.renderableSeries.add(new FastLineRenderableSeries(wasmContext, { dataSeries }));
surface.chartModifiers.add(new ZoomPanModifier());
surface.chartModifiers.add(new MouseWheelZoomModifier());
surface.chartModifiers.add(new RolloverModifier());
```

### Annotations (Fourth Pillar)

In addition to the three core types, SciChart has a robust **Annotations** system:

- **Coordinate Modes**: Annotations can be positioned in multiple coordinate systems:
  - `DataValue` -- positioned at data coordinates (price, time), moves with zoom/pan
  - `Pixel` -- fixed pixel position relative to chart surface, unaffected by zoom/pan
  - `Relative` -- proportional (0.0 to 1.0) position relative to chart bounds

- **Render Layers**: Annotations render at configurable z-order layers:
  - `BelowSeries` -- behind all data series
  - `AboveSeries` -- in front of data series
  - `BelowChart` -- behind the chart surface entirely
  - `AboveChart` -- in front of everything

- **Built-in types**: HorizontalLineAnnotation, VerticalLineAnnotation, BoxAnnotation, LineAnnotation, TextAnnotation, CustomAnnotation (host arbitrary HTML/SVG).

- **Annotation Lifecycle**:
  1. Create annotation with coordinate mode and anchor values
  2. Add to `surface.annotations` collection
  3. Surface manages coordinate transforms per frame
  4. Annotation receives render callback with transformed screen coordinates
  5. User interaction (drag, resize) modifies anchor values
  6. Removal from collection triggers cleanup

### ChartModifier Pattern -- The Gold Standard for Interaction

The ChartModifier system is **the strategy pattern applied to chart interaction**:

- **Fully decomposed**: `ZoomPanModifier`, `RubberBandXyZoomModifier`, `MouseWheelZoomModifier`, `RolloverModifier` (crosshair/tooltips), `CursorModifier`, `XAxisDragModifier`, `YAxisDragModifier`, `LegendModifier`.
- **Composed in a ModifierGroup**: Modifiers process events in priority order.
- **Custom modifiers**: Extend `ChartModifierBase` and override event handlers.
- **Each modifier is self-contained**: owns its state, handles its events, produces its visual output.

Rust translation of this concept:

```rust
pub trait ChartModifier: Send + Sync {
    fn on_event(&mut self, event: &ChartEvent, state: &ChartState) -> ModifierResult;
    fn priority(&self) -> u32 { 100 }
    fn is_active(&self) -> bool { true }
}

pub struct ModifierStack {
    modifiers: Vec<Box<dyn ChartModifier>>,
}

impl ModifierStack {
    pub fn process(&mut self, event: &ChartEvent, state: &ChartState) -> Vec<ChartAction> {
        let mut actions = Vec::new();
        for modifier in self.modifiers.iter_mut().filter(|m| m.is_active()) {
            match modifier.on_event(event, state) {
                ModifierResult::Handled(a) => { actions.extend(a); break; }
                ModifierResult::Continue(a) => { actions.extend(a); }
                ModifierResult::Ignored => {}
            }
        }
        actions
    }
}
```

This separates interaction behaviors (pan, zoom, crosshair, level drag) into composable units. The priority ordering and handled/continue/ignored result enum gives precise control over event propagation.

### Data Ownership

Explicitly separated. `DataSeries` objects are independent and own their data. You create, populate, and assign them to RenderableSeries. Multiple series can share one DataSeries. This is the cleanest data ownership model in the survey.

### Renderer Swappability

- **WPF**: Yes. Ships with `HighQualityRenderSurface` (software), `HighSpeedRenderSurface`, and `Visual Xccelerator Engine` (DirectX 11).
- **JS**: No -- WASM+WebGL only.

---

## 2. Highcharts Stock Binding System

### Architecture

Highcharts Stock is a declarative, option-driven charting library similar to ECharts but specialized for financial time series.

### Binding System

Highcharts uses a **declarative binding** approach where chart behavior is specified through nested option objects:

```js
Highcharts.stockChart('container', {
    series: [{
        type: 'candlestick',
        data: ohlcData
    }],
    navigator: { enabled: true },
    scrollbar: { enabled: true },
    rangeSelector: { selected: 1 },
    yAxis: [{
        labels: { align: 'right' },
        height: '70%'
    }, {
        labels: { align: 'right' },
        top: '70%',
        height: '30%'
    }]
});
```

### Key Patterns

- **Navigator**: A miniature chart below the main chart for range selection. The navigator has its own series data (often downsampled) and bidirectionally syncs the visible range with the main chart.

- **Multi-axis composition**: Multiple y-axes with explicit height/top positioning. This is how indicators in separate panes are implemented -- each pane is a y-axis with a specified height percentage.

- **Event binding**: All interaction is configured through the options object. Events fire callbacks but do not provide composable modifier objects like SciChart.

- **Data grouping**: Automatic downsampling when zoomed out. Groups bars into larger time intervals and applies aggregation (OHLC from tick data). This is transparent to the user and handled by the library.

### Assessment for Hand of Midas

The declarative binding system is less relevant for a Rust GPU application, but the navigator pattern and multi-axis composition with explicit height percentages are worth studying for multi-pane layout.

---

## 3. D3FC: The Decorate Pattern

### Architecture

D3FC (D3 Financial Components) builds on D3.js to provide reusable financial chart components. Its key innovation is the **decorate pattern**.

### The Decorate Pattern

D3FC components expose a `decorate` accessor that lets consumers modify the underlying D3 selection after the component has applied its default behavior:

```js
const candlestick = fc.seriesSvgCandlestick()
    .decorate(selection => {
        selection.enter()
            .select('.up')
            .attr('fill', 'green');
        selection.enter()
            .select('.down')
            .attr('fill', 'red');
    });
```

The pattern works because:
1. The component creates DOM elements with predictable class names
2. `decorate` runs after the component's default rendering
3. The consumer can modify any aspect of the rendered output
4. No need for exhaustive configuration options -- direct DOM access covers every case

### Composition Model

D3FC provides building blocks that compose through D3's data-join pattern:

- `fc.chartCartesian()` -- coordinated chart with axes
- `fc.seriesCanvasLine()`, `fc.seriesSvgCandlestick()` -- series renderers
- `fc.annotationLine()` -- horizontal/vertical annotation lines
- `fc.indicatorBollingerBands()` -- computed indicators
- `fc.seriesMulti()` -- combine multiple series on one chart

Each component is a function that operates on a D3 selection. Composition is function composition.

### Assessment for Hand of Midas

The decorate pattern solves a problem (customizing rendered output after creation) that Rust's type system handles differently. In Rust, you customize through builder patterns, generic parameters, or trait implementations. The underlying principle -- exposing intermediate state for customization rather than trying to predict all configuration options -- is valuable regardless of language.

---

## 4. Cross-Library Comparison

| Dimension | SciChart | Highcharts Stock | D3FC |
|---|---|---|---|
| API style | Imperative composition | Declarative options | Functional composition |
| Interaction | Composable modifiers (gold standard) | Event callbacks | D3 behaviors |
| Annotation system | First-class, multi-coordinate-mode | Series markers + plotLines | `annotationLine` component |
| Data ownership | Explicit DataSeries objects | Chart copies input | D3 data-join (references) |
| Renderer | WASM+WebGL (JS) / DirectX (WPF) | SVG + Canvas | SVG or Canvas (per component) |
| Custom series | Extend base class | Custom series type | Any D3 component |
| z-order control | BelowSeries/AboveSeries/BelowChart/AboveChart | Series `zIndex` property | DOM order |

---

## 5. Key Patterns for Hand of Midas

### From SciChart

1. **The three-pillar model (Series + Axes + Modifiers)** maps to Midas's architecture:
   - Series = `CandlePipeline`, `VolumePipeline`
   - Axes = `GridPipeline` + axis label rendering
   - Modifiers = tool structs on `ChartState` (CrosshairTool, LevelTool, etc.)

2. **Annotation coordinate modes** inform how overlays should be positioned:
   - Data coordinates (price, time) -- levels, trendlines
   - Pixel coordinates -- badges, tooltips
   - Relative coordinates -- watermarks, legends

3. **Render layers** map directly to GPU pipeline execution order:
   - BelowSeries = grid, zone fills
   - Series = candles, volume
   - AboveSeries = levels, annotations
   - AboveChart = crosshair, tooltips

4. **Composable modifiers** should be the target architecture when interaction complexity exceeds 10 types. Current monolithic `handle_event()` is fine under that threshold.

### From D3FC

5. **Function composition over inheritance.** D3FC's approach of composing small functions validates the "free functions returning render data" pattern already used by `compute_gerchik_atr()`.

### From Highcharts Stock

6. **Multi-axis height percentages** for pane layout. When implementing multi-pane support (RSI below price chart), explicit height fractions are simpler than constraint-based layout.
