# TradingView Architecture Patterns

> Compiled from open-source and cross-chart sync research, 2026-03-30

---

## Overview

TradingView Lightweight Charts (v5, ~35 kB, Apache 2.0) is a Canvas 2D charting library with a handle-based imperative API. `createChart(container)` returns an `IChartApi` handle; the chart owns its Canvas element and render loop. The host provides only a `<div>`.

TradingView's commercial platform and Charting Library build on this foundation with server-side persistence, cross-layout sync, and a full drawing/annotation system.

---

## 1. Series, Studies, and Drawings Separation

### Three-Layer Visual Model

TradingView separates visual elements into distinct categories:

1. **Series** -- the primary price data (candlestick, line, area, bar). Each series is created via `chart.addSeries()` and owns its own data via `setData()` / `update()`. Multiple series can share or use independent price scales.

2. **Studies/Indicators** -- computed overlays (moving averages, Bollinger Bands, RSI). In the Charting Library, these are configured through the study API. They compute from series data and render on price or separate panes.

3. **Drawings/Annotations** -- user-created visual elements (trendlines, horizontal lines, Fibonacci retracements). These are stored separately from series data and have their own persistence layer.

### API Pattern

```
IChartApi
  ├── addSeries(type, options) → ISeriesApi
  │     ├── setData(data[])
  │     ├── update(bar)
  │     ├── setMarkers(markers[])
  │     ├── attachPrimitive(primitive)   // v5 plugin model
  │     └── applyOptions(options)
  ├── timeScale() → ITimeScaleApi
  ├── priceScale(id) → IPriceScaleApi
  ├── subscribeCrosshairMove(handler)
  └── subscribeClick(handler)
```

The boundary is **data in, rendered pixels out, events back via callbacks**. The chart controls the render cycle; external code never initiates redraws.

---

## 2. Plugin Model with ISeriesPrimitive

v5 introduced two extension mechanisms:

### ICustomSeriesPaneView

Full custom series types. Plugins implement `updateAllViews()` and provide renderers that receive a `CanvasRenderingTarget2D`.

### ISeriesPrimitive

Drawing primitives attached to existing series. This is the mechanism for overlays, annotations, and custom visual elements. Key characteristics:

- **Attached to a series**, not free-floating. Each primitive has a host series that determines its coordinate system.
- **Chart-controlled render cycle.** Primitives never initiate redraws. The chart calls `updateAllViews()` when the view changes.
- **Renderer receives a canvas target.** The primitive produces renderers that draw into the canvas at the appropriate z-order.

### hitTest for Interactivity

TradingView's plugin model includes hit-testing support:

- Each primitive renderer can implement a `hitTest(x, y)` method.
- The chart walks primitives in reverse z-order to find the topmost hit.
- Hit results carry metadata (which part of the drawing was hit, e.g., endpoint vs. body).
- This enables drag, resize, and selection behaviors without the chart knowing the primitive's shape.

The hitTest pattern is the critical bridge between rendering (visual output) and interaction (user input). By placing hitTest on the primitive rather than on a separate interaction layer, TradingView ensures that visual and interactive boundaries always match.

---

## 3. zOrder System

TradingView uses a layered rendering order:

- **Background** -- chart background, watermarks
- **Grid** -- grid lines
- **Series** -- candlesticks, lines, areas (primary data)
- **Overlays** -- additional series on shared axes
- **Primitives** -- attached plugins, markers
- **Axes** -- price and time axis labels
- **Crosshair** -- always on top

Within the primitives layer, individual `ISeriesPrimitive` instances can specify relative z-order to control stacking among themselves. This maps directly to the GPU pipeline execution order in a wgpu-based renderer.

---

## 4. Per-Symbol Drawing Storage

TradingView's Charting Library API reveals two storage models:

### Combined Storage (Default)

Drawings are embedded in the chart layout JSON. A layout includes all drawings for all symbols/charts in that layout. Changing the symbol hides drawings from the previous symbol.

### Separate Drawings Storage (`saveload_separate_drawings_storage` featureset)

Drawings are stored independently from chart layouts, keyed by symbol:

```
GET  /drawings?client={id}&user={id}&chart={id}    -- load drawings
PUT  /drawings?client={id}&user={id}&chart={id}     -- save drawings
DELETE /drawings?client={id}&user={id}&chart={id}    -- delete
```

The `saveLineToolsAndGroups` method receives state organized per-symbol:

```json
{
  "sources": {
    "AAPL": [
      { "id": "abc123", "type": "trendline", "points": [...], "style": {...} },
      { "id": "def456", "type": "horizontal_line", "price": 185.5, "style": {...} }
    ],
    "MSFT": [...]
  }
}
```

**Null values indicate deleted drawings** -- the API uses tombstones for deletion tracking.

**Key insight**: Per-symbol storage means a horizontal level at $185.50 on AAPL exists once and appears on every chart displaying AAPL, regardless of timeframe. This is the dominant pattern across professional platforms.

---

## 5. Cross-Chart Sync

### Three Sync Modes

1. **No sync**: Drawings saved per-chart, per-layout. Changing symbol hides drawings.
2. **Layout sync**: Drawings on a symbol sync across all charts in the current layout.
3. **Global sync**: Drawings sync across all charts and all layouts.

### Selective Chart Grouping

TradingView allows **emoji-based chart grouping** within a layout. Charts marked with the same emoji sync selected parameters (symbol, crosshair, interval, date range). This is their version of color-coded link groups used by Bloomberg and ThinkOrSwim.

### Cross-Timeframe Behavior

Drawings are anchored at **(timestamp, price)** coordinate pairs:

- **Horizontal lines**: price-only, perfect across timeframes.
- **Trendlines**: (t1, p1) to (t2, p2). When the chart resolution changes, the library snaps timestamps to the nearest available bar.
- **Acknowledged limitation**: A trendline connecting two daily bar highs may not visually connect the same candle features on a 1-hour chart because the bar timestamps differ. TradingView's documentation explicitly states: "Drawings may be displayed differently on various time intervals of the same symbol."

### Server-Side Architecture

- Drawings persist in PostgreSQL (backend: `tradingview/saveload_backend`).
- Cloud-synced across devices.
- Rendering is hybrid: server-side preloading + client-side interactivity.

---

## 6. Multi-Pane Support

v5 added `IPaneApi` for vertically stacked sub-charts with a shared time axis. This enables indicators like RSI or MACD in separate panes below the main price chart, all sharing the same horizontal scroll and zoom state.

---

## 7. Data Ownership

Chart owns copies. `setData()` fully replaces internal data. Documentation advises keeping your own copy if you need it later. This is the same pattern used by ECharts and Qt Charts -- copy on ingestion is the safest model for avoiding lifetime entanglement between application state and rendering state.

---

## 8. Key Takeaways for Hand of Midas

1. **Per-symbol drawing storage is the correct model.** The `LevelStore` pattern already implements this for horizontal levels. Extend to all annotation types.

2. **hitTest on visual primitives** bridges rendering and interaction. Each overlay type should implement hit-testing against its own geometry rather than relying on a separate spatial index.

3. **zOrder as pipeline execution order** maps cleanly to wgpu. The fixed layer stack (grid -> series -> overlays -> axes -> crosshair) becomes pipeline execution order.

4. **Separate drawings from series data.** Series data is temporal and large. Drawings are spatial and small. Different storage, different update cadences, different persistence.

5. **Emoji/color link groups** for symbol routing are a workspace-level concern, separate from drawing sync. Both are needed but are independent systems.

6. **Cross-timeframe drawing display is inherently imperfect** for time-anchored drawings. Accept this limitation explicitly, as TradingView does, rather than trying to solve it perfectly.
