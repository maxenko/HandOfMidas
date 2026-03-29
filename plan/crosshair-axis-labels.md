# Plan: Crosshair Axis Labels (White Background)

## Context

The crosshair currently draws two lines (horizontal + vertical) but has no axis labels showing the price/time values at the cursor. The user wants white-background badge labels anchored at the ends of the crosshair arms:
- **Price label**: right end of horizontal arm (Y axis edge), shows price at cursor Y
- **Time label**: bottom end of vertical arm (X axis edge), shows detailed datetime of snapped candle

Both update dynamically as the crosshair moves. Visible only when crosshair is active (left mouse down).

## Approach

Follow the existing overlay pattern: **compute in midas-chart, render as iced widgets in views.rs**.

The `CrosshairRender` already computes `price_label` and `time_label` inside the GPU pipeline, but these never reach the view layer. Rather than restructuring the data flow, add a parallel public computation function that the view calls directly — identical to how `compute_y_labels()` and `date_labels::compute()` work.

## Changes

### 1. `midas-chart/src/compute.rs`
- Make `format_datetime_long` public (line 986)
- Add `CrosshairLabels` struct and `compute_crosshair_labels()` public function
  - Takes `cursor_pos: Option<(f32, f32)>`, `camera`, `data`, `collapse_gaps`
  - Returns `Option<CrosshairLabels>` with two `AxisLabel`s
  - Handles both normal (timestamp-space) and collapsed-gaps (index-space) snapping
  - White bg `[1.0, 1.0, 1.0, 0.95]`, dark text `[0.1, 0.1, 0.1, 1.0]`

### 2. `midas-chart/src/lib.rs`
- Re-export `CrosshairLabels`, `compute_crosshair_labels`, `format_datetime_long`

### 3. `midas-app/src/app/views.rs`
- Add `build_crosshair_label_overlay()` function:
  - Price label: right-aligned at right edge, vertically centered on `screen_y`
  - Time label: horizontally centered on `screen_x`, positioned just above timeline border
  - Style: white background, dark text, rounded 3px badge, `[3, 8]` padding, 12pt font
- Wire into both `view_pane_body()` and `view_floating_chart()` — insert into `chart_layers` after price/level overlays, before drawing panel

### Layer order in stack
1. shader (GPU)  →  2. date_overlay  →  3. price_overlay  →  4. level_labels  →  **5. crosshair_labels**  →  6. drawing_panel  →  7. level_editor

## Verification
- `cargo build --workspace` — compiles cleanly
- `cargo test --workspace` — all tests pass
- `cargo clippy --workspace -- -D warnings` — no warnings
- Visual: run app, hold left mouse on chart, verify white badges track cursor at arm endpoints
