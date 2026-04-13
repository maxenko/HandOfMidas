# Bug Hunt: Frozen Candle Ghost on Ticker Switch

**Date:** 2026-04-12
**Status:** FIXED (verified by user)
**Scope:** Full rendering pipeline trace: `load_symbol_for_chart` -> `bind_chart_to_symbol` -> `view()` -> `ChartRenderSnapshot` -> `Program::draw()` -> `compute_chart_scene()` -> `ChartPrimitive::prepare()` -> `render_prepare()` -> GPU upload. Examined `chart_widget.rs`, `app.rs`, `ticker_wiring.rs`, `views.rs`, `compute/mod.rs`, `scene.rs`, `dirty/mod.rs`, `renderer.rs`, `candle.rs`, and iced 0.14 source (`shader.rs`, `primitive.rs`, `lib.rs`, `layer.rs`).

**Lenses run:** null/boundary, error-path, inconsistency, invariants, concurrency
**Result:** 1 bug (confirmed), 1 likely bug, 1 smell

---

## Symptom

Switch tickers -> old candles freeze as a permanent imprint on the GPU surface. Grid/labels respond to pan/zoom. Candles don't. Brackets update correctly. Eventually loading the right data after clicking many tickers.

## Bug (confirmed)

**[high] [high] `chart_widget.rs:1132` + `app.rs:1619-1622` -- prepare() / load_symbol_for_chart**

### Witness

User switches from AAPL (loaded) to NVDA. `load_symbol_for_chart` at `:1619` calls `mark_data()` (bumps `dirty.candles`) but does NOT clear `chart.data`. Between the switch and `DataLoaded`, one or more frames render with: `chart.data = Some(AAPL_buffer)` + `dirty.candles` incremented. `draw()` calls `compute_chart_scene` with AAPL data -> produces `scene.candles = Some(AAPL_instances)` with pixel positions baked from the AAPL camera. `prepare()` at `:1132` updates `resources.candles = AAPL_instances`. `render_prepare` sees the new generation -> uploads AAPL instances to GPU and acknowledges the generation.

When `DataLoaded(NVDA)` arrives, `apply_candle_data` resets camera to NVDA's range and bumps `dirty.candles` again. `compute_chart_scene` runs with NVDA data + NVDA camera. **BUT:** if the NVDA camera points to a time range where `visible_candle_range` returns an empty range for the NVDA data (e.g., `camera_restored_pending` restores a stale saved camera, or the auto-scale range lands at an edge), then `build_candle_instances` returns `None` (`compute/mod.rs:522-523`). In `prepare()` at `:1132`, `if let Some(ref candles) = scene.candles` is `false` -> `resources.candles` retains the AAPL instances. The GPU buffer still has AAPL candle pixel positions. Since candle instances are in pixel space (not world space), the AAPL candles render at fixed screen positions. Grid lines are always re-uploaded (`:1142`, `renderer.rs:205-206`) so they update normally, creating the visual split described.

The "eventually loading the right data after clicking many tickers" symptom occurs because each switch fires `mark_data()` and occasionally the camera and data align to produce `vis_start < vis_end`, yielding `Some(new_instances)` that overwrites the stale cache.

### Harm

Permanent ghost candles from previous ticker. Candles frozen at pixel positions unresponsive to pan/zoom (because new projections don't affect the old baked pixel coordinates). Grid/brackets/crosshair all update correctly. User must click many tickers randomly until one produces a non-empty visible range.

### Reachability

Any user switching tickers where the camera restore or auto-scale produces a visible range that doesn't overlap the new data initially.

### Fix sketch

**Option A (recommended):** Clear `chart.data = None` in `load_symbol_for_chart` immediately on switch (before the async load). This causes `draw()` to return `scene: None` (`chart_widget.rs:748-762`), which triggers the explicit buffer clear at `:1118-1119`. When `DataLoaded` arrives, `apply_candle_data` sets `chart.data = Some(new_buffer)` and the next frame renders correctly. The `LoadState::Loading` check can show a placeholder if desired.

**Option B (minimal change):** In `prepare()` after `:1134`, add an `else { resources.candles.clear(); }` so that `scene.candles == None` always clears the GPU cache rather than preserving stale data:

```rust
if let Some(ref candles) = scene.candles {
    resources.candles = candles.clone();
} else {
    resources.candles.clear();
}
if let Some(ref volumes) = scene.volumes {
    resources.volumes = volumes.clone();
} else {
    resources.volumes.clear();
}
```

Option B prevents stale data from persisting but doesn't address the root cause. Option A ensures `draw()` produces `scene: None` during the loading window, giving the user a clear visual signal and preventing any ghost rendering.

**Applied fix: all three changes plus a fourth.**

1. Option A: `app.rs` `load_symbol_for_chart` — `chart.data = None`
2. Option B: `chart_widget.rs` `prepare()` — `else { clear() }` defense-in-depth
3. Error handler: `app.rs` DataLoaded `Err` — `chart.data = None`
4. Camera restore: removed `camera_restored_pending = true` from `bind_chart_to_symbol` (`ticker_wiring.rs`). With `chart.data = None` on switch, the original reason for deferred camera restore (avoid stale candle/camera desync) no longer applies. `apply_candle_data`'s "last 200 candles" reset is now always used for interactive switches. Without this fix, the saved camera override in DataLoaded could position the viewport where no candles were visible, requiring the user to press R to reset.

**Best approach: both.** Option A as the primary fix, Option B as defense-in-depth.

---

## Likely bug (needs author confirmation)

**[medium] [medium] `app.rs:2246-2249` -- DataLoaded error handler**

### Witness

Data load for NVDA fails (network error, missing file). `chart.load_state = LoadState::Error(...)` but `chart.data` retains the previous ticker's (AAPL) buffer. `view()` at `:851` checks `if let Some(ref data) = chart.data` -- the old AAPL data is still present, so the shader widget renders AAPL candles under NVDA's symbol name.

### Harm

Wrong chart data displayed with wrong symbol label after load error. User sees AAPL candles but the symbol header says NVDA.

### Reachability

Any load failure (missing file, invalid format, network timeout). Depends on whether the data provider can actually fail -- author should confirm.

### Fix sketch

Add `chart.data = None;` in the error handler at `:2248`.

---

## Smell (no witness -- worth a look)

**[low] [low] `compute/mod.rs` + `scene.rs:30-31` -- misleading Option semantics**

The comments document `scene.candles: Option<Vec<CandleInstance>>` as a dirty-flag optimization where `None = reuse cached GPU buffer`. But `compute_chart_scene` NEVER uses dirty flags to decide whether to compute candle instances -- `build_candle_instances` always computes when `vis_start < vis_end` and returns `None` only when the visible range is empty. The `Option` wrapper is functioning as "empty result" not "unchanged, reuse cache." The comments create a false mental model that has already led to bugs (the `if let Some` in `prepare()` was written assuming dirty-flag semantics but the compute pipeline doesn't implement them). Consider removing the `Option` wrapper and always returning `Vec<CandleInstance>` (empty when no visible candles), eliminating the ambiguity entirely.

---

## Clean areas

- **Dirty flag system** (`dirty/mod.rs`): Generation counter design is sound. `DirtyTracker` comparison and acknowledgment logic is correct.
- **GPU pipeline** (`candle.rs`, `renderer.rs`): `update_instances` / `update_projection` / `draw_wicks` / `draw_bodies` are all correct. `instance_count = 0` correctly skips draw calls.
- **iced integration**: `Shader` widget always calls `draw()` every frame. No primitive caching. `prepare()` is called on every new primitive. Pipeline storage is type-keyed and shared correctly.
- **Data loading** (`load_chart_async`, stale-load guard): Symbol + generation double-check correctly discards stale loads.
- **Camera restore** (`bind_chart_to_symbol`, `DataLoaded`): Deferred restore with `camera_restored_pending` is correctly designed to avoid the visual desync the comments describe.

---

## Key files

| File | Lines | Role |
|------|-------|------|
| `desktop/win/crates/midas-app/src/app.rs` | 1607-1631 | `load_symbol_for_chart` -- does NOT clear `chart.data` |
| `desktop/win/crates/midas-app/src/app.rs` | 1930-1967 | `apply_candle_data` -- sets data + resets camera |
| `desktop/win/crates/midas-app/src/app.rs` | 2134-2254 | `DataLoaded` handler + error path |
| `desktop/win/crates/midas-app/src/app/ticker_wiring.rs` | 39-113 | `bind_chart_to_symbol` -- deferred camera restore |
| `desktop/win/crates/midas-app/src/app/views.rs` | 845-1018 | `view_pane_body` -- snapshot creation |
| `desktop/win/crates/midas-app/src/chart_widget.rs` | 725-913 | `Program::draw()` -- scene computation |
| `desktop/win/crates/midas-app/src/chart_widget.rs` | 1096-1266 | `Primitive::prepare()` -- GPU upload |
| `desktop/win/crates/midas-app/src/chart_widget.rs` | 1132-1137 | **Bug site**: `if let Some` preserves stale cache |
| `desktop/win/crates/midas-chart/src/compute/mod.rs` | 47-63 | `compute_chart_scene` -- pure, no caching |
| `desktop/win/crates/midas-chart/src/compute/mod.rs` | 510-593 | `build_candle_instances` -- returns `None` on empty range |
| `desktop/win/crates/midas-chart/src/dirty/mod.rs` | 1-168 | Dirty flags + tracker (correct) |
| `desktop/win/crates/midas-render/src/renderer.rs` | 172-225 | `render_prepare` -- dirty-gated GPU upload |
| `desktop/win/crates/midas-render/src/pipelines/candle.rs` | 204-227 | `update_instances` -- GPU buffer write |
