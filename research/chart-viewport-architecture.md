# Chart Viewport/Data Synchronization Architecture Research

> Research conducted 2026-04-11
> Context: Ticker-switch desync bug where candle data and camera update at different times.

---

## 1. TradingView Lightweight Charts — DataLayer Transaction

**Architecture**: `ChartModel` orchestrates `DataLayer`, `TimeScale`, `PriceScale`, and `Series`. Updates flow through an invalidation mask system.

**Key mechanism**: `DataLayer.setSeriesData()` performs a **single-pass atomic update** that returns a `DataUpdateResponse` bundling:
- New series data (only changed series)
- New time scale points (sorted, re-indexed)
- Base index (latest point with data)

The caller (ChartModel) applies this response atomically — time scale and series data are never in an inconsistent state because both are derived from the same response object. There is no window where the time scale reflects ticker A while series data reflects ticker B.

**Insight**: The "transaction response" pattern. Data mutation returns a single struct describing all side effects. The coordinator applies them in one pass.

---

## 2. Plotters (Rust) — Ephemeral Context, No Retained State

**Architecture**: `ChartBuilder` creates a `ChartContext` that binds a coordinate system (data range mapped to pixel range) to a `DrawingArea`. Series are drawn immediately via `draw_series()`.

**Key mechanism**: There is no retained chart state that can desync. Each render is a fresh construction: build context with ranges, draw series, done. The `ChartState` struct exists only to enable incremental updates — it stores enough to reconstruct the coordinate mapping.

**Insight**: The "rebuild from scratch" pattern. When data changes, reconstruct the entire chart context with the new data and new ranges in one call. Desync is impossible because there is no mutable state persisting between frames.

---

## 3. D3-FC Financial Charts — Declarative Re-render

**Architecture**: Data is bound to the DOM via `selection.datum(data).call(chart)`. Scales (`xScale`, `yScale`) are configured with `domain` (data range) and `range` (pixel extent). Components receive scales automatically.

**Key mechanism**: On any state change (data swap, viewport change, interaction), the entire chart is re-rendered by calling the root render function. The render function recomputes scales from current data, then passes those scales to all child components. There is no separate "update viewport" step.

**Insight**: The "single render entry point" pattern. One function owns both data-to-scale derivation and scale-to-pixel rendering. You cannot call one without the other.

---

## 4. MetaTrader — Indicator Buffer Architecture

**Architecture**: Each chart owns indicator buffers (fixed-size arrays). When the instrument changes, all buffers are cleared and `OnCalculate` is called with the new data series. The viewport (visible range) is recomputed from the new bar count.

**Key mechanism**: Instrument switching is a **hard reset**. The chart does not attempt to preserve viewport state across ticker switches. Buffer clear + full recalculation is atomic from the indicator's perspective.

**Insight**: The "scorched earth" pattern. On ticker switch, destroy all derived state and rebuild. Simple, impossible to desync, but loses scroll position.

---

## 5. Bevy Engine — Extract/Prepare/Render Pipeline

**Architecture**: Two separate ECS Worlds — Main World (app logic) and Render World (GPU state). Every frame, the Extract phase copies data from Main into Render. Then Prepare builds GPU resources. Then Render draws.

**Key mechanism**: The render world is **wiped clean every frame** (entities deleted, resources kept). Extract systems copy only what's needed for this frame. The render world never holds stale references to data that changed mid-frame. Data and derived GPU state are always from the same temporal snapshot.

**Insight**: The "extract snapshot" pattern. The renderer never reads live application state. It receives a frozen copy at a defined synchronization point. All GPU resources (instance buffers, uniforms) are built from that single snapshot.

---

## 6. wgpu Idiomatic Pattern — Queue Writes

**Architecture**: Instance buffers and uniform buffers (projection matrix) are written via `queue.write_buffer()` before the render pass. Both writes happen in the same frame's command sequence.

**Key mechanism**: The correct pattern is: (1) compute instances from data + camera, (2) write instance buffer, (3) write projection uniform, (4) submit render pass. All three inputs to the frame (data, camera, instances) must be captured at the same logical moment.

**Insight**: The "prepare phase" pattern. All GPU buffer writes happen after all state mutations are complete and before any draw calls begin. Never write a buffer from state that is still being mutated.

---

## Synthesis: The Frame Snapshot Pattern

All systems converge on the same principle: **data, viewport, and derived GPU state must be captured at a single synchronization point per frame.**

### Concrete fix for the ticker-switch desync:

```
pub struct ChartFrame {
    candles: Arc<CandleBuffer>,   // which data
    camera: Camera2D,              // which viewport
    dirty: DirtyFlags,             // what changed
}
```

**The rule**: When `candles` changes (ticker switch), `camera` and `dirty` MUST be updated in the same struct assignment. The renderer consumes `ChartFrame` as an immutable snapshot. It never observes candles from ticker A with camera from ticker B because both fields are set together before the frame begins.

### Pattern taxonomy:

| Pattern | Used by | Tradeoff |
|---|---|---|
| Transaction response | TradingView | Data mutation returns all side effects; coordinator applies atomically |
| Rebuild from scratch | Plotters, D3-FC | No retained state = no desync; costs CPU each frame |
| Scorched earth | MetaTrader | Hard reset on switch; simple but loses position |
| Extract snapshot | Bevy | Separate worlds with defined sync point; enables pipelining |
| Prepare phase | wgpu idiom | All buffer writes after all state mutations, before draw |

### Recommended approach for Hand of Midas:

Combine **Transaction response** + **Extract snapshot**:
1. Ticker switch produces a `ChartFrame` struct atomically (new buffer + reset camera + mark all-dirty)
2. The compute pipeline reads only from `ChartFrame` (never from live mutable state)
3. Instance buffer + projection uniform are written together in a prepare phase
4. The renderer draws from that frame's buffers only

This eliminates the temporal gap between data swap and camera update.
