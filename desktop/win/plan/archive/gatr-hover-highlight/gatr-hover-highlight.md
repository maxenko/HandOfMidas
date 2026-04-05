# Feature: G.ATR Hover Highlight

## Overview

When the user hovers over the G.ATR badge (top-right corner of an intraday chart), dim all candles that were **not** part of the ATR calculation. The 7 non-paranormal daily bars used for the denominator plus today's bar remain at full brightness; everything else dims to ~30% brightness. Moving the mouse away restores normal rendering. This gives immediate visual feedback about which trading sessions the G.ATR metric is based on.

## Research Summary

### Codebase Analysis

**Architecture**: midas-chart is sans-IO (no GPU/framework deps). It produces a `ChartScene` consumed by midas-render (wgpu pipelines). The iced app layer overlays widgets (including the G.ATR badge) on top of the GPU-rendered chart via stacked layers. Data flows through `ChartInput` → `compute_chart_scene()` → `ChartScene`.

**CandleInstance** (`instances.rs:22-41`): 48-byte `#[repr(C)]` Pod struct. Has `_pad0: f32` at offset 28, currently unbound in the shader. Can be repurposed as a dim factor at zero memory cost — same struct size, same alignment.

**Candle shader** (`candle.wgsl`): Two-pass instanced renderer (wick + body). `InstanceInput` skips `_pad0` with a comment at line 56. Fragment shader returns `in.color` flat — adding a dim multiply is one line.

**Instance buffer layout** (`candle.rs:288-343`): Locations 1-7 bound for geometry fields, offset 28 skipped, location 8 for color at offset 32. Adding location 9 at offset 28 for `dim` is mechanical.

**G.ATR badge** (`views.rs:2949-2976`): Rendered as iced `text` widget in a `container`, positioned top-right via flexbox. **Not** GPU-rendered. `mouse_area` is used 10+ times in this codebase for hover/click detection. iced 0.14 `MouseArea` confirms `on_enter` and `on_exit` methods are available.

**Daily bar aggregation**: **Two copies** exist:
- `gerchik_atr.rs:89-125` (old overlay, DailyBar has f32 fields)
- `indicators/gerchik_atr.rs:75-109` (newer indicator architecture, DailyBar has f64 fields)
Both return `Vec<DailyBar>` without index mapping. Both need updating.

**ATR selection** (`midas-core/atr.rs:69-115`): `gerchik_gatr_pct()` walks backward through history TRs, collecting 7 non-paranormal bars. Returns only `Option<f32>`. Does not expose which bar indices were selected.

**ChartInput** (`input.rs:17-58`): Clean input contract for `compute_chart_scene`. Currently has no hover/highlight fields. This is where `gatr_hover` and `bright_ranges` should be added.

**DirtyFlags** (`dirty.rs`): Generation-counter system. Has `mark_camera` (cascades to candles), `mark_data`, `mark_theme`, etc. No standalone `mark_candles()` — needs adding so hover toggle doesn't needlessly rebuild indicators.

### Best Practices & Idiomatic Approach

- **Per-instance dim via repurposed padding**: Zero memory overhead. Standard approach for per-element emphasis in instanced GPU rendering.
- **CPU-side dim assignment**: At 5K candles, iterating and setting a float is sub-microsecond. No compute shader needed.
- **Coarse dirty gating**: Only rebuild candle instances when hover state *changes* (binary toggle), not on every mouse move. At most 2 rebuilds per hover interaction.
- **iced `mouse_area` for hover**: Proven pattern in this codebase, `on_enter`/`on_exit` confirmed available.

## Design Decisions

### Decision: How to expose ATR-selected bar indices

**Context**: `gerchik_gatr_pct` returns only a percentage. The chart needs to know which 7+1 bars were selected to map them back to intraday candle ranges.

**Options**:
1. Rich return struct from `gerchik_gatr_pct` — changes signature, watchlist doesn't need indices.
2. Separate companion function — duplicates walk logic.
3. New `gerchik_gatr_detail()` returning struct, `gerchik_gatr_pct` becomes a thin wrapper.

**Recommendation**: Option 3. No duplication, backward-compatible. Chart calls `detail`, watchlist keeps calling `pct`.

**Confidence**: high

### Decision: How to dim candles

**Context**: Need per-candle brightness reduction in the GPU pipeline.

**Options**:
1. Repurpose `_pad0` as `dim: f32` — zero memory cost, one shader line.
2. Modify color alpha — requires blend state change from `REPLACE` to alpha blending.
3. Separate dim buffer — new pipeline layout, more complex.

**Recommendation**: Option 1. `_pad0` exists as padding; repurposing it is the highest-value use.

**Confidence**: high

### Decision: How to detect badge hover

**Options**:
1. Wrap badge in `mouse_area` with `on_enter`/`on_exit` — proven pattern in codebase.
2. Hit-test in chart_widget `Program::update()` — badge is iced widget, bounds unknown to GPU program.

**Recommendation**: Option 1.

**Confidence**: high

### Decision: Data flow for highlight state

**Context**: `build_candle_instances` needs to know `gatr_hover` and `bright_ranges` to set dim values. These must flow through `ChartInput`.

**Recommendation**: Add `gatr_bright_ranges: &'a [(usize, usize)]` to `ChartInput`. When empty (hover inactive or no ATR data), no dimming. When non-empty, candles outside these ranges get `dim = 1.0`. This avoids a separate boolean — empty slice IS the "off" state.

**Confidence**: high

## Implementation Plan

### Slice 1: Expose ATR-selected bar indices from core algorithm

**Goal**: `gerchik_gatr_pct` exposes which history bars were selected so the chart can identify them.

**Depends on**: None

**Files to modify**:
- `desktop/win/crates/midas-core/src/atr.rs` — Add `GatrResult` struct and `gerchik_gatr_detail()`. Refactor `gerchik_gatr_pct` to delegate.
- `desktop/win/crates/midas-core/src/lib.rs` — Re-export `GatrResult` and `gerchik_gatr_detail`.

**Key implementation details**:
```rust
pub struct GatrResult {
    /// G.ATR percentage (today_tr / avg × 100).
    pub pct: f32,
    /// Indices into the input highs/lows/closes arrays for the 7 (or fewer)
    /// non-paranormal history bars used in the average. Does NOT include today.
    pub selected_bars: Vec<usize>,
}
```
- The walk-backwards loop already identifies passing bars. Collect their original indices: bar at `all_trs[j]` corresponds to input array index `j + 1` (since TRs start at bar 1).
- `gerchik_gatr_pct` becomes: `gerchik_gatr_detail(h, l, c).map(|r| r.pct)`.
- Today = the last daily bar in the input arrays (the most recent calendar day in the loaded data, regardless of actual date). It is implicitly always bright — not in `selected_bars`.

**Testing**:
- Uniform bars: `selected_bars` contains 7 most recent history bar indices.
- Paranormal bars: skipped in `selected_bars`, bars behind them included.
- Existing `gerchik_gatr_pct` tests pass unchanged (it's a wrapper now).

**Done when**: `gerchik_gatr_detail` returns correct indices; `gerchik_gatr_pct` unchanged behavior.

---

### Slice 2: Add daily-to-intraday index mapping in chart overlays

**Goal**: `aggregate_daily_bars()` returns intraday candle index ranges per daily bar, enabling "daily bar N is ATR-selected" → "intraday candles X..Y should be bright".

**Depends on**: None (parallel with Slice 1)

**Files to modify**:
- `desktop/win/crates/midas-chart/src/gerchik_atr.rs` — Add `start_idx: usize` and `end_idx: usize` fields to `DailyBar`. Track indices during aggregation.
- `desktop/win/crates/midas-chart/src/indicators/gerchik_atr.rs` — Same change to the duplicate `DailyBar` struct and `aggregate_daily_bars()` in this module.

**Key implementation details**:
- Track `day_start_idx` when a new day begins. On day boundary or end of data, set `end_idx = i - 1` (for boundary) or `end_idx = data.len() - 1` (for final bar).
- `DailyBar { high, low, close, start_idx, end_idx }` — ranges are inclusive.
- Existing callers extract high/low/close and ignore indices unless needed.

**Testing**:
- 3 days × 4 candles: ranges are `(0,3)`, `(4,7)`, `(8,11)`.
- Single day: one bar with `(0, N-1)`.
- Existing aggregation tests continue to pass.

**Done when**: Index ranges correct for multi-day fixtures in both modules.

---

### Slice 3: Compute bright ranges and expose on GerchikAtrRender

**Goal**: Combine Slices 1+2 to produce intraday candle index ranges that should remain bright.

**Depends on**: Slice 1, Slice 2

**Files to modify**:
- `desktop/win/crates/midas-chart/src/gerchik_atr.rs` — Update `compute_gerchik_atr()` to call `gerchik_gatr_detail` and map selected daily bar indices through daily-to-intraday ranges. Add `bright_ranges: Vec<(usize, usize)>` to `GerchikAtrRender`.
- `desktop/win/crates/midas-chart/src/indicators/gerchik_atr.rs` — Same pattern for `compute()`.

**Key implementation details**:
- Call `gerchik_gatr_detail` instead of `gerchik_gatr_pct`.
- For each index in `result.selected_bars`, look up the daily bar's `(start_idx, end_idx)`.
- Always include today's bar range (last daily bar = the most recent calendar day in the aggregated data).
- Sort bright_ranges by start_idx for efficient scan in Slice 6. Ranges must be non-overlapping.
- `GerchikAtrRender` gains: `pub bright_ranges: Vec<(usize, usize)>`.

**Testing**:
- Multi-day fixture with paranormal bars: verify bright_ranges covers exactly the expected intraday indices for 7 selected days + today.
- Edge case: fewer than 7 non-paranormal bars available.

**Done when**: `GerchikAtrRender.bright_ranges` is populated correctly.

---

### Slice 4: Repurpose `_pad0` as `dim` in CandleInstance and shader

**Goal**: GPU pipeline can render candles at reduced brightness via per-instance dim factor.

**Depends on**: None (parallel with Slices 1-3)

**Files to modify**:
- `desktop/win/crates/midas-chart/src/instances.rs` — Rename `_pad0` to `dim`. Update doc comment.
- `desktop/win/crates/midas-chart/src/compute.rs` — Change `_pad0: 0.0` to `dim: 0.0` at all CandleInstance construction sites.
- `desktop/win/crates/midas-render/shaders/candle.wgsl` — Add `@location(9) dim: f32` to `InstanceInput`. Add `dim: f32` to `VertexOutput`. Pass `dim` from vertex to fragment. Modify fragment: `return vec4(in.color.rgb * (1.0 - in.dim * 0.7), in.color.a);`.
- `desktop/win/crates/midas-render/src/pipelines/candle.rs` — Add `VertexAttribute { format: Float32, offset: 28, shader_location: 9 }` to `candle_instance_buffer_layout()`. Update comment.

**Key implementation details**:
- `dim = 0.0` → full brightness (backward compatible). `dim = 1.0` → 30% brightness.
- Formula: `rgb * (1.0 - dim * 0.7)`. The 0.7 constant means dim=1 → 30% of original brightness.
- No blend state change. This is RGB multiplication, not alpha blending.
- All existing code sets `dim: 0.0`, so zero visual change until Slice 6 activates it.
- Struct stays 48 bytes — field was already there as padding.

**Testing**:
- `candle_instance_size_is_48_bytes` test passes (no layout change).
- Update `candle_instance_is_pod` test to use `dim` field name.
- Shader compiles (verified by existing render pipeline init test / app startup).

**Done when**: Shader compiles, `dim: 0.0` candles render identically to before.

---

### Slice 5: Badge hover messages, ChartInput field, and dirty flag

**Goal**: Hovering the G.ATR badge populates `bright_ranges` on `ChartInput` and triggers candle rebuild.

**Depends on**: Slice 3 (needs `GerchikAtrRender.bright_ranges`)

**Files to modify**:
- `desktop/win/crates/midas-chart/src/dirty.rs` — Add `pub fn mark_candles(&mut self) { self.candles += 1; }`.
- `desktop/win/crates/midas-chart/src/input.rs` — Add `pub gatr_bright_ranges: &'a [(usize, usize)]` field to `ChartInput`.
- `desktop/win/crates/midas-app/src/app.rs` — Add `Message::GatrHoverEnter(ChartId)` and `Message::GatrHoverLeave(ChartId)`. Handle by toggling a `gatr_hover: bool` on the chart's state and calling `state.dirty.mark_candles()`.
- `desktop/win/crates/midas-app/src/app/views.rs` — Update `build_gerchik_atr_overlay` to accept `chart_id: ChartId` and return a `mouse_area`-wrapped badge. Wire `on_enter`/`on_exit` messages. Update both call sites (floating window line ~168, main charts line ~852) to pass `chart_id`. When building `ChartInput`, populate `gatr_bright_ranges` from the `GerchikAtrRender.bright_ranges` if `gatr_hover` is true, else pass `&[]`.

**Key implementation details**:
- `mouse_area` wraps just the label text (not the full-width container), so hover triggers only on the visible text.
- State cleanup: reset `gatr_hover = false` in these specific Message handlers:
  - `Message::PanelSymbolSubmitted(chart_id)` (user changes symbol)
  - `Message::PanelTimeframeSelected(chart_id, tf)` (user changes timeframe)
  - `propagate_symbol_change()` (linked symbol propagation)
  - `propagate_timeframe_change()` (linked timeframe propagation)
  These all trigger `mark_data()` which rebuilds candles, so the stale bright_ranges won't be used.
- When `gatr_hover` is true but `GerchikAtrRender` is `None` (e.g., daily chart, insufficient data), pass `&[]` — no dimming occurs.

**Testing**:
- Message round-trip: `GatrHoverEnter` → `gatr_hover` is true, candles dirty. `GatrHoverLeave` → false.
- Symbol change resets `gatr_hover` to false.

**Done when**: Hovering badge toggles `gatr_hover`; `ChartInput.gatr_bright_ranges` is populated.

---

### Slice 6: Wire dim factor into candle and volume instance computation

**Goal**: When `gatr_bright_ranges` is non-empty, `build_candle_instances` sets `dim = 1.0` on candles outside those ranges.

**Depends on**: Slice 4 (dim field), Slice 5 (ChartInput field)

**Files to modify**:
- `desktop/win/crates/midas-chart/src/compute.rs` — Add `bright_ranges: &[(usize, usize)]` parameter to `build_candle_instances`. When non-empty, set `dim = 1.0` for candles whose data index is NOT in any range; `dim = 0.0` for those inside. Same for `build_volume_instances`: multiply the volume color alpha by 0.3 for dimmed candles. Update both call sites (`compute_normal_scene` and `compute_collapsed_scene`) to pass `input.gatr_bright_ranges`.

**Key implementation details**:
- `bright_ranges` is sorted, non-overlapping. Add a `debug_assert!` validating this invariant before the scan (zero cost in release builds):
  ```rust
  debug_assert!(bright_ranges.windows(2).all(|w| w[0].1 < w[1].0),
      "bright_ranges must be sorted and non-overlapping");
  ```
  Use a cursor scan: maintain a `range_idx` pointer, advance as candle index passes each range's end. O(n) total for n candles.
- `collapse_gaps` does not affect correctness: ranges are indices into the `CandleData` array, and both collapsed and normal modes iterate the same data indices (vis_start..vis_end). Only the X positioning differs, not the data index.
- Volume dimming: multiply `volume_color.a` by 0.3 for dimmed bars. The `0.3` factor intentionally matches the candle shader's 30% brightness target (`1.0 - dim * 0.7` at `dim=1.0`). If the candle constant is tuned later, update both. Volume bars already use alpha blending, so this works naturally.
- When `bright_ranges` is empty (`&[]`), all candles get `dim = 0.0` — zero overhead (fast path).

**Testing**:
- Unit test: `bright_ranges = [(10,20), (30,40)]` → candles 0-9 dim=1.0, 10-20 dim=0.0, 21-29 dim=1.0, 30-40 dim=0.0, 41+ dim=1.0.
- Empty `bright_ranges` → all dim=0.0.
- Integration: `compute_chart_scene` with non-empty `gatr_bright_ranges` produces correct dim values.

**Done when**: With hover active, only ATR-selected candles + today render at full brightness.

### Dependency Summary

```
Slice 1 (core indices) ──┐
                          ├── Slice 3 (combine) ── Slice 5 (hover + input) ──┐
Slice 2 (daily mapping) ─┘                                                   ├── Slice 6 (wire)
Slice 4 (GPU dim field) ─────────────────────────────────────────────────────┘
```

**Parallel starts**: Slices 1, 2, and 4 have zero dependencies — all three can start simultaneously.
**Critical path**: 1 → 3 → 5 → 6 (or 2 → 3 → 5 → 6).
**Slice 4** is independent and can complete anytime before Slice 6.

## Risks & Unknowns

1. **Badge hover Z-order** (verified safe): The G.ATR badge is an iced widget layered above the chart's `shader::Program`. Plan evaluation confirmed that `mouse_area` with only `on_enter`/`on_exit` (no `on_press`) does **not** consume press/drag events — the chart's pan/zoom below works normally. iced stacks widgets in layer order; the badge layer receives hover events first. No fallback needed.

2. **Multi-chart isolation**: Each chart has its own G.ATR badge and `gatr_hover` state. Messages include `ChartId`. No cross-chart interference.

3. **Stale hover state**: If `gatr_hover` is true and the user switches symbol/timeframe, the bright_ranges would be stale. **Mitigation**: Reset `gatr_hover = false` on symbol/timeframe change (Slice 5).

## Testing Strategy

- **Unit tests**: `gerchik_gatr_detail` for index correctness, `aggregate_daily_bars` for range mapping, dim assignment logic for cursor scan.
- **Integration tests**: Full `compute_chart_scene` with populated `gatr_bright_ranges` produces correct `dim` values on CandleInstance output.
- **Visual verification**: Manual — hover the badge, confirm correct candles dim.

## Non-Goals / Out of Scope

- Dimming in the watchlist grid (no candles to dim).
- Animated fade transition (can be added later).
- Configurable bar count or paranormal thresholds via hover UI.
- Distinguishing paranormal bars from "too old" bars (all non-selected get same dim).
- Hover tooltip showing ATR detail breakdown.

## Review Notes

- The `indicators/gerchik_atr.rs` module has a **duplicate** `aggregate_daily_bars` and `DailyBar` struct (f64 vs f32 types). Both must be updated in Slice 2. Consider consolidating into a single shared implementation in a future refactor, but that's out of scope for this feature.
- The `dim` field replaces `_pad0` permanently. If future features need another per-instance float (e.g., animation progress), a new approach would be needed (enlarge the struct or use a separate buffer). This is unlikely to be a problem.
- Volume bars are dimmed via alpha multiplication (not a `dim` field) since `VolumeInstance` has no padding to repurpose. This is adequate because volume bars already use alpha blending.
