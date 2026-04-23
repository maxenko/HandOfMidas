# Slice 7 — Session background bands

**Goal.** Render faint background-colored rectangles for pre-market and post-market windows (TradingView-adjacent convention). Reuses existing `GridLineInstance` pipeline.

## Scope

### `compute/build_grid_instances` modification

Inside `build_grid_instances()` in `desktop/win/crates/midas-chart/src/compute/mod.rs`:

```rust
if input.show_extended_hours {
    let bands = compute_session_bands(
        data,
        camera,
        vis_start,
        vis_end,
        separator_y,
        input.viewport_width as f32,
        params,
    );
    out.extend(bands);
}

fn compute_session_bands(
    data: &dyn CandleData,
    camera: &Camera2D,
    vis_start: usize,
    vis_end: usize,
    separator_y: f32,
    viewport_width: f32,
    params: &Params,
) -> Vec<GridLineInstance> {
    let mut out = Vec::new();
    let mut current_kind: Option<SessionKind> = None;
    let mut band_start_idx = vis_start;

    for i in vis_start..=vis_end {
        let kind = if i < vis_end { data.session(i) } else { SessionKind::Closed };
        if Some(kind) != current_kind {
            if let Some(prev) = current_kind {
                if matches!(prev, SessionKind::PreMarket | SessionKind::PostMarket) {
                    let x_start = camera.time_to_x(data.timestamp(band_start_idx) as f64);
                    let x_end = camera.time_to_x(data.timestamp(i.saturating_sub(1).max(band_start_idx)) as f64);
                    let color = match prev {
                        SessionKind::PreMarket => params.pre_market_bg_color,
                        SessionKind::PostMarket => params.post_market_bg_color,
                        _ => continue,
                    };
                    out.push(GridLineInstance {
                        rect: [x_start, 0.0, x_end, separator_y],
                        color,
                    });
                }
            }
            current_kind = Some(kind);
            band_start_idx = i;
        }
    }
    out
}
```

### Params extension

`ChartParams` gains:

```rust
pub pre_market_bg_color: [f32; 4],    // default [0.85, 0.35, 0.35, 0.06]
pub post_market_bg_color: [f32; 4],   // default [0.35, 0.55, 0.95, 0.06]
```

### ChartInput extension

`ChartInput` gains `show_extended_hours: bool`. Wired from `ChartPanel.show_extended_hours` in `midas-app`.

### Render order

`ChartRenderer::render` already draws grid → volume → candles → badges. Bands land in the grid layer; drawn before candles so candles appear on top of the band tint. No render-order change needed.

## Files touched

- `desktop/win/crates/midas-chart/src/compute/mod.rs` — band computation + integration into `build_grid_instances`.
- `desktop/win/crates/midas-chart/src/scene.rs` — `ChartInput` gains `show_extended_hours`.
- `desktop/win/crates/midas-chart/src/style.rs` — new palette entries.

## Tests

- `session_bands_rendered_for_pre_post`: CandleBuffer with alternating PreMarket/Regular/PostMarket candles; assert `compute_session_bands` emits 2 instances (pre band + post band).
- `no_bands_when_toggle_off`: with `show_extended_hours = false`, `build_grid_instances` doesn't emit bands.
- `crypto_symbol_no_bands`: CandleBuffer where all candles are Regular → zero band instances regardless of toggle.
- `band_colors_match_params`: override defaults; assert GridLineInstance colors reflect override.

## Acceptance

- Tests pass.
- Clippy / fmt clean.
- Visual smoke: run app on AAPL, toggle EH on, observe faint reddish band 04:00–09:30 ET and bluish band 16:00–20:00 ET per day.

## Commit

Single commit: `feat(chart): session background bands for pre/post markets`.
