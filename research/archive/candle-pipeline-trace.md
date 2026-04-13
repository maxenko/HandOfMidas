# Hand of Midas Candle Rendering Pipeline Trace

## Complete End-to-End Rendering Pipeline

This document traces the complete pipeline from CandleBuffer entry through GPU pixels, with special focus on ticker-switch behavior and the frozen candles symptom.

### 1. DATA FLOW: CandleBuffer Ownership and Entry

**File: midas-app/src/app.rs**
- Line 99: ChartPanel struct member: data: Option<Arc<CandleBuffer>>
- Line 1908-1945: apply_candle_data() function handles ticker switches:
  - Line 1909: chart.data = Some(Arc::clone(&buffer)) assigns new ticker buffer
  - Line 1911: chart.chart_state.dirty.mark_data() marks both candles and indicators dirty
  - Line 1944: chart.chart_state.dirty.mark_camera() marks camera, candles, grid dirty

**File: midas-app/src/chart_widget.rs**
- Line 54-119: ChartRenderSnapshot captures data: Option<Arc<CandleBuffer>>
- Snapshot constructed fresh each frame in view() function
- Data passed through ChartInput to compute phase

### 2. COMPUTE PASS: CandleBuffer + Camera2D to CandleInstance Array

**File: midas-chart/src/compute/mod.rs**
- Line 46-62: compute_chart_scene entry point reads from input.data
- Line 510-593: build_candle_instances loop:
  - For each visible candle, computes x (screen pixel), body_top/bottom (screen pixel), color
  - x computed via x_for_candle closure (timestamp or index to pixel)
  - Y coordinates via camera.price_to_y() conversion
  - Result: Vec<CandleInstance> or None if empty
  - Instances are COMPUTED FRESH every frame, no caching

### 3. SCENE OUTPUT: ChartScene with Generation Counters

**File: midas-chart/src/scene.rs**
- Line 22: projection: glam::Mat4 computed fresh from camera
- Line 31: candles: Option<Vec<CandleInstance>> None if unchanged
- Line 81: generations: SceneGenerations carries dirty flag snapshots
  - generations.candles, generations.camera, generations.grid, etc.

### 4. DIRTY FLAG SYNCHRONIZATION

**File: midas-chart/src/dirty/mod.rs**
- Line 14-30: DirtyFlags struct with 7 generation counters
- Line 41-45: mark_camera() increments camera, candles, AND grid together (ATOMIC CASCADE)
- Line 49-52: mark_data() increments candles and indicators
- Line 104-168: DirtyTracker reads needs_candle_rebuild() and needs_grid_rebuild()

**CRITICAL**: Both candle and grid invalidation come from same mark_camera() call, preventing desync.

### 5. GPU RENDERER: Atomic Projection Upload

**File: midas-render/src/renderer.rs**
- Line 94-108: if tracker.needs_camera_update(scene.dirty) block updates projection on ALL 6 pipelines
- Line 111-121: needs_candle_rebuild() and needs_grid_rebuild() check same dirty object
- Line 140: tracker.acknowledge() updates all trackers atomically

**File: midas-chart/src/camera/mod.rs**
- Line 95-114: projection_matrix() computed fresh each frame, never cached
- Uses only Camera2D viewport_width/height for orthographic projection

### 6. TICKER SWITCH SEQUENCE: NVDA to AMD

1. apply_candle_data() (app.rs:1909): chart.data = AMD_buffer
2. dirty.mark_data() (app.rs:1911): candles counter incremented
3. dirty.mark_camera() (app.rs:1944): camera, candles, grid counters incremented
4. Next frame snapshot (chart_widget.rs): captures new AMD_buffer and updated dirty flags
5. compute_chart_scene() (compute/mod.rs:46): reads AMD_buffer, generates new instances
6. Renderer (renderer.rs:95): needs_camera_update returns true, projection uploaded to all pipelines
7. Renderer (renderer.rs:111): needs_candle_rebuild returns true, new AMD instances uploaded
8. Renderer (renderer.rs:118): needs_grid_rebuild returns true, grid uploaded
9. All trackers acknowledged with new generation values

**Result**: Complete synchronization. No frozen candles possible.

### 7. FROZEN CANDLES ANALYSIS

**Cannot occur because:**
1. mark_camera() increments both dirty.candles and dirty.grid simultaneously
2. Renderer checks both in same atomic block against same dirty object
3. Both pipelines receive same projection computed once

**If symptom appears**: Investigate widget snapshot staleness - if snapshot.data is old but snapshot.camera is new.

### SUMMARY: KEY FILES AND LINE NUMBERS

| Component | File | Key Lines |
|-----------|------|-----------|
| Data ownership | midas-app/src/app.rs | 99, 1908-1945 |
| Dirty system | midas-chart/src/dirty/mod.rs | 41-45, 104-168 |
| Compute | midas-chart/src/compute/mod.rs | 46-62, 510-593 |
| Scene | midas-chart/src/scene.rs | 22, 31, 81 |
| Projection | midas-chart/src/camera/mod.rs | 95-114 |
| Renderer | midas-render/src/renderer.rs | 94-140 |
| Widget | midas-app/src/chart_widget.rs | 54-119 |

All three paths (compute/projection/grid) are synchronized through generation counters. Ticker switches cascade through all paths atomically in one frame.
