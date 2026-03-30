# Cross-Chart Crosshair Sync

## Context

When the user hovers a crosshair on one chart, all other charts showing the **same symbol** should display a ghosted vertical line at the corresponding time — regardless of timeframe. This is standard multi-chart sync behavior in trading platforms.

## Approach: Global sync state on MidasApp

Store a single `crosshair_sync: Option<(ChartId, i64, String)>` on `MidasApp` — `(source_id, timestamp_ms, symbol)`.

- **SetCrosshair(X, pos)**: compute snapped timestamp from X's data/camera, set `crosshair_sync = Some((X, ts, symbol))`
- **ClearCrosshair(X)**: if sync source == X, clear sync. Otherwise leave it (another chart may now be the source).
- **View layer**: each chart checks if sync applies (same symbol, different source), computes ghost pixel x from the timestamp, passes to snapshot.
- **GPU layer**: ghost line rendered as a dim `GridLineInstance` appended to `crosshair_lines` in `prepare()`.

Ghost line: vertical only, no horizontal, no labels. Color: `[0.5, 0.5, 0.6, 0.25]` (dimmer than real crosshair `[0.7, 0.7, 0.7, 0.5]`).

## Files to modify (3 files, all in midas-app)

### 1. `crates/midas-app/src/app.rs`
- Add field `crosshair_sync: Option<(ChartId, i64, String)>` to `MidasApp`, init `None`
- Rewrite `Message::ChartCrosshair` handler:
  - On `Some((x,y))`: compute snapped timestamp, set sync, mark sibling charts' crosshair dirty
  - On `None`: if source matches, clear sync and mark siblings dirty
- Add helper `crosshair_to_timestamp(x, data, camera, collapse_gaps) -> Option<i64>`

### 2. `crates/midas-app/src/chart_widget.rs`
- Add `ghost_crosshair_x: Option<f32>` to `ChartRenderSnapshot`
- Add `ghost_crosshair_x: Option<f32>` to `ChartPrimitive`
- Pass through in `draw()`: `ghost_crosshair_x: snap.ghost_crosshair_x`
- In `prepare()` after crosshair_lines are built (~line 696): if ghost_crosshair_x is set and on-screen, append a dim vertical `GridLineInstance`

### 3. `crates/midas-app/src/app/views.rs`
- In `view_pane_body` and `view_floating_chart`: compute `ghost_crosshair_x` from `self.crosshair_sync`
  - Check: same symbol, different source chart
  - Convert timestamp → pixel x using target chart's camera + data + collapse_gaps mode
  - `camera.snap_to_pixel(camera.time_to_x(...))` for normal mode
  - `find_index_by_time` + `camera.time_to_x(idx + 0.5)` for collapsed mode
- Pass to snapshot

## Timestamp conversion helpers

**Source chart (pixel → timestamp):**
```
Normal:  camera.x_to_time(x) → ts_f → data.find_index_by_time(ts_f as i64) → data.timestamp(idx)
Collapsed: camera.x_to_time(x) → idx_f → round, clamp → data.timestamp(idx)
```

**Target chart (timestamp → pixel):**
```
Normal:  camera.snap_to_pixel(camera.time_to_x(ts as f64))
Collapsed: data.find_index_by_time(ts) → camera.snap_to_pixel(camera.time_to_x(idx as f64 + 0.5))
```

## Edge cases
- Ghost off-screen: skip if `ghost_x < 0 || ghost_x > viewport_width`
- No data on target: `ghost_crosshair_x = None`
- Same chart: filter by `src_id != chart_id`
- Race condition (click B while A active): ClearCrosshair(A) guard `src == A` prevents clearing B's sync
- Floating charts (ChartId(0)): receive ghosts via symbol match, but currently can't be sync sources

## Verification
- `cargo check -p midas-app`
- `cargo clippy --workspace`
- `cargo fmt --all`
- Manual test: open two panes with same symbol, different timeframes. Click-hold on one, verify ghost vertical line on the other.
