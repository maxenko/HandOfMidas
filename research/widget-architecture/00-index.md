# Widget Architecture Research

> Compiled research on chart component architecture for Hand of Midas.
> Source material from open-source surveys, renderer analysis, Rust pattern research,
> and cross-platform trading platform analysis.
>
> Date: 2026-03-30

---

## Documents

| # | File | Description |
|---|---|---|
| 01 | [01-tradingview.md](01-tradingview.md) | TradingView Lightweight Charts architecture: series/studies/drawings separation, ISeriesPrimitive plugin model, hitTest interactivity, zOrder system, per-symbol drawing storage, cross-chart sync modes |
| 02 | [02-professional-charting.md](02-professional-charting.md) | SciChart three-pillar model (series/axes/modifiers), annotation coordinate modes and render layers, ChartModifier interaction pattern, Highcharts Stock binding system, D3FC decorate pattern |
| 03 | [03-game-engine-patterns.md](03-game-engine-patterns.md) | Bevy ECS three-tier visibility and RenderLayers bitmask, Unity CanvasGroup opacity propagation, egui layer ordering and cautionary tale, iced Shader widget three-trait pattern, GPUI flat scene rendering |
| 04 | [04-rust-dispatch.md](04-rust-dispatch.md) | Enum vs trait object benchmarks (10-12x difference), enum_dispatch pattern, hybrid enum+trait approach, sans-IO boundary patterns, render primitive vocabulary, lifecycle and dirty-flag patterns |
| 05 | [05-cross-chart-sync.md](05-cross-chart-sync.md) | Per-symbol storage validation across 6 platforms, color-coded link groups, event-driven push sync, recursion prevention, time-anchored drawing challenges across timeframes, order bracket display |
| 06 | [06-synthesis.md](06-synthesis.md) | Synthesis and concrete recommendations: core principles, Presence enum (Active/Ghost/Hidden), per-ticker storage, compute-scene-GPU pipeline, interaction separation, enum dispatch, Rust struct/enum sketches |

## Source Material

These documents were compiled from the following research files:

- `desktop/win/plan/chart-architecture-research-opensource.md` -- Survey of TradingView LC, ECharts, Plotly, D3, SciChart, Grafana, QCustomPlot
- `desktop/win/plan/chart-architecture-research-renderers.md` -- Render abstraction analysis: Vello, Skia, wgpu, display lists, game engine render graphs
- `desktop/win/plan/chart-architecture-research-rust-patterns.md` -- Rust GUI framework comparison: iced Shader widget, egui PaintCallback, Bevy ECS, GPUI, Makepad
- `desktop/win/plan/rust-widget-patterns-research.md` -- Rust dispatch patterns, trait design, sans-IO, render primitives, lifecycle patterns
- `plan/cross-chart-sync-research.md` -- Cross-chart sync across Bloomberg, ThinkOrSwim, NinjaTrader, MetaTrader, Sierra Chart, TradingView
