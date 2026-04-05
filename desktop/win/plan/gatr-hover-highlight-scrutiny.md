# Scrutiny Report: G.ATR Hover Highlight Feature

17 files changed across 6 slices implementing hover-triggered candle dimming. The core algorithm (index mapping, cursor scan, shader) is solid — all GPU math and data structures are correct. However, the app-layer plumbing has **two critical integration bugs** that prevent the feature from actually working: bright_ranges is always empty (never computed from chart data), and floating window hover messages are silently lost.

## Critical Issues

**C1** (high confidence) — `views.rs` snapshot construction — `gatr_bright_ranges` is always empty

**Scope note**: The feature only needs to work on daily candle charts. On a daily chart each candle IS a daily bar, so `selected_bars` from `gerchik_gatr_detail` maps directly to candle indices — no intraday aggregation needed.

The current code sources `gatr_bright_ranges` from `gatr_render_from_cache`, which always returns `bright_ranges: Vec::new()` because it only has the cached percentage, not the candle buffer.

**Fix**: Compute bright_ranges directly from the chart's `CandleBuffer` at snapshot construction time. Add a helper function:
```rust
fn compute_daily_bright_ranges(data: &midas_core::CandleBuffer) -> Vec<(usize, usize)> {
    let highs: Vec<f64> = data.highs.iter().map(|&h| h as f64).collect();
    let lows: Vec<f64> = data.lows.iter().map(|&l| l as f64).collect();
    let closes: Vec<f64> = data.closes.iter().map(|&c| c as f64).collect();
    let Some(result) = midas_core::gerchik_gatr_detail(&highs, &lows, &closes) else {
        return Vec::new();
    };
    let mut ranges: Vec<(usize, usize)> = result
        .selected_bars
        .iter()
        .map(|&idx| (idx, idx))
        .collect();
    // Always include today (last bar).
    let last = data.len().saturating_sub(1);
    ranges.push((last, last));
    ranges.sort_unstable_by_key(|r| r.0);
    ranges
}
```
In both snapshot construction sites (`view_floating_chart` and `view_pane_body`), replace:
```rust
gatr_bright_ranges: if chart.gatr_hover {
    gerchik_atr.as_ref().map_or(Vec::new(), |g| g.bright_ranges.clone())
} else {
    Vec::new()
},
```
with:
```rust
gatr_bright_ranges: if chart.gatr_hover {
    chart.data.as_ref().map_or(Vec::new(), |d| compute_daily_bright_ranges(d))
} else {
    Vec::new()
},
```

Files to modify:
- `desktop/win/crates/midas-app/src/app/views.rs` — add helper, update both snapshot sites

---

**C2** (high confidence) — `app.rs:3095-3107` `GatrHoverEnter`/`Leave` handler only checks `self.charts`

Floating charts are stored in `self.floating_charts` (keyed by `window::Id`), not `self.charts`. The handler `self.charts.get_mut(&chart_id)` returns `None` for `ChartId(0)`, so the hover state is never toggled for floating windows. **The feature is completely dead for pop-out charts.**

**Fix**: After the `self.charts.get_mut` miss, fall back to checking `self.floating_charts`. The floating window uses `ChartId::new(0)` as its chart_id. Add to both `GatrHoverEnter` and `GatrHoverLeave` handlers:
```rust
Message::GatrHoverEnter(chart_id) => {
    if let Some(chart) = self.charts.get_mut(&chart_id) {
        chart.gatr_hover = true;
        chart.chart_state.dirty.mark_candles();
    } else {
        // Floating window: ChartId(0) is not in self.charts.
        for (_, fc) in self.floating_charts.iter_mut() {
            fc.gatr_hover = true;
            fc.chart_state.dirty.mark_candles();
        }
    }
    Task::none()
}
```
Same pattern for `GatrHoverLeave`.

Files to modify:
- `desktop/win/crates/midas-app/src/app.rs` — update both handlers

## Warnings

**W1** (high confidence) — `app.rs` — `gatr_hover` not reset on floating chart symbol/timeframe propagation

The resets in `propagate_symbol_change` and `propagate_timeframe_change` cover docked charts but skip the floating chart loops. Could leave stale hover state on floating charts after symbol/timeframe change.

**Fix**: Add `chart.gatr_hover = false;` in the floating chart mutation blocks of:
- `propagate_symbol_change` (floating loop)
- `propagate_timeframe_change` (floating loop)

Files to modify:
- `desktop/win/crates/midas-app/src/app.rs`

---

**W2** (medium confidence) — `atr.rs` doc comment inaccuracy

`gerchik_gatr_detail` doc says "Returns `None` if there are no non-paranormal history bars" but the all-paranormal fallback returns `Some` with `pct` using `raw_avg` and empty `selected_bars`.

**Fix**: Update doc to: "Returns `None` if there are fewer than 2 bars or no history TRs. When all history bars are paranormal, returns `Some` with `pct` based on the raw average and empty `selected_bars`."

Files to modify:
- `desktop/win/crates/midas-core/src/atr.rs`

## Suggestions

**S1** (high confidence) — `candle.wgsl:43-46` stale comment

The struct layout comment still says `_pad0: f32`. Should say `dim: f32`.

---

**S2** (medium confidence) — No test for all-paranormal fallback with `gerchik_gatr_detail`

Add a test where all history bars are paranormal, confirming `selected_bars` is empty and `pct` uses `raw_avg` as denominator.

---

**S3** (low confidence) — `indicators/gerchik_atr.rs` DailyBar fields `start_idx`/`end_idx` are unused

The `compute()` function in this module never reads the index fields — it produces `IndicatorOutput::TextBadge` only. The fields are harmless dead data but add noise.

## What's Done Well

**P1** — The cursor-scan algorithm in `build_candle_instances` (compute.rs:517-575) is elegant. The monotonic `range_idx` pointer with the `while` advance correctly handles all edge cases (before/between/after/at-boundary ranges) in O(n) with zero allocations. The `debug_assert!` for the sorted non-overlapping precondition is a good defensive touch.

**P2** — Repurposing `_pad0` as `dim` is a zero-cost change that preserves the 48-byte struct layout, requires no pipeline layout restructuring, and the default `dim: 0.0` means zero visual regression. The shader math `rgb * (1.0 - dim * 0.7)` is correct in linear color space.

**P3** — The `gerchik_gatr_detail` refactoring cleanly separates index tracking from the original algorithm without duplicating logic. The wrapper `gerchik_gatr_pct` calling `.map(|r| r.pct)` is as thin as possible.

## Verdict

**NEEDS CHANGES** — The core plumbing (Slices 1-4, 6) is high quality and correct, but the two critical integration bugs (C1: empty bright_ranges, C2: broken floating window hover) mean the feature won't produce any visible effect. These need fixing before the change is shippable.
